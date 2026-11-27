//! 本机运行实例注册、有限快照与命令回执；不接触终端输入缓冲。
use super::protocol::*;
use serde_json::{json, Value};
use std::{collections::{HashMap, VecDeque}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::sync::Notify;

const MAX_ITEMS: usize = 800;
const MAX_TEXT: usize = 64 * 1024;
const MAX_COMMANDS: usize = 128;
const MAX_PENDING: usize = 32;
const MAX_SNAPSHOT_TEXT: usize = 2 * 1024 * 1024;

pub struct Record {
    pub session: Session,
    token: String,
    last_seen: Instant,
    pub pending: VecDeque<Command>,
    commands: HashMap<String, (Command, Receipt, bool)>,
    order: VecDeque<String>,
    changes: VecDeque<Change>,
}

#[derive(Default)]
struct Inner { records: HashMap<String, Record>, revision: u64 }

#[derive(Default)]
pub struct Hub { inner: Mutex<Inner>, pub changed: Notify }

fn clipped(text: &str, limit: usize) -> String {
    if text.len() <= limit { return text.into(); }
    let suffix = "\n…内容已截断";
    let mut end = limit.saturating_sub(suffix.len());
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}{suffix}", &text[..end])
}

fn trim(session: &mut Session) -> bool {
    let mut changed = false;
    for item in &mut session.items {
        changed |= item.text.len() > MAX_TEXT || item.title.len() > 512;
        item.text = clipped(&item.text, MAX_TEXT);
        item.title = clipped(&item.title, 512);
    }
    for task in &mut session.tasks{
        changed|=task.title.len()>512||task.message.len()>MAX_TEXT;
        task.title=clipped(&task.title,512);task.message=clipped(&task.message,MAX_TEXT);
    }
    let mut bytes: usize = session.items.iter().map(|item| item.text.len() + item.title.len()).sum();
    while session.items.len() > MAX_ITEMS || bytes > MAX_SNAPSHOT_TEXT {
        if session.items.is_empty() { break; }
        changed = true;
        let item = session.items.remove(0);
        bytes = bytes.saturating_sub(item.text.len() + item.title.len());
    }
    changed
}

impl Hub {
    pub fn register(&self, agent: &str, tab_id: u32, generation: u32, cwd: String) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let session = Session {
            protocol_version: 1,
            runtime_id: id.clone(), session_id: String::new(), tab_id, generation, agent: agent.into(),
            title: format!("{agent} · {}", std::path::Path::new(&cwd).file_name().unwrap_or_default().to_string_lossy()),
            cwd, model: String::new(), models: Vec::new(), policy: String::new(), state: "connecting".into(),
            turn_id: String::new(), seq: 0, settings_revision: 0, shared: false, allow_control: false,
            capabilities: Capabilities::default(), items: Vec::new(), approvals: Vec::new(), error: None,
            interactions:Vec::new(), tasks:Vec::new(), permissions:json!({}), queued_inputs:Vec::new(), activity:json!({}),
        };
        let mut inner = self.inner.lock().unwrap();
        inner.records.insert(id.clone(), Record { session, token: token.clone(), last_seen: Instant::now(),
            pending: VecDeque::new(), commands: HashMap::new(), order: VecDeque::new(), changes: VecDeque::new() });
        inner.revision += 1;
        drop(inner); self.changed.notify_waiters();
        (id, token)
    }

    pub fn list(&self, remote: bool) -> Value {
        let inner = self.inner.lock().unwrap();
        let mut rows: Vec<_> = inner.records.values().filter(|r| !remote || r.session.shared).map(|r| {
            let s = &r.session;
            json!({"runtimeId":s.runtime_id,"sessionId":s.session_id,"tabId":s.tab_id,"generation":s.generation,
                "agent":s.agent,"title":s.title,"cwd":s.cwd,"model":s.model,"state":s.state,
                "shared":s.shared,"allowControl":s.allow_control,"seq":s.seq})
        }).collect();
        rows.sort_by_key(|s| s["runtimeId"].as_str().unwrap_or_default().to_string());
        json!({ "revision": inner.revision, "sessions": rows })
    }

    pub fn snapshot(&self, id: &str, remote: bool) -> Result<Session, String> {
        let inner = self.inner.lock().unwrap();
        let record = inner.records.get(id).ok_or("会话已结束")?;
        if remote && !record.session.shared { return Err("会话尚未分享".into()); }
        Ok(record.session.clone())
    }

    pub fn share(&self, id: &str, generation: u32, shared: bool, control: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner.records.get_mut(id).ok_or("会话已结束")?;
        if record.session.generation != generation { return Err("会话已替换".into()); }
        record.session.shared = shared;
        record.session.allow_control = shared && control;
        if !record.session.allow_control {
            Self::cancel_pending(record, "网页操作权限已撤销");
        }
        // 分享权限变化使旧游标回退到快照，不能只发送 Agent 内容增量。
        record.changes.clear();
        record.session.seq += 1; inner.revision += 1;
        drop(inner); self.changed.notify_waiters(); Ok(())
    }

    pub fn authorize(&self, id: &str, token: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(record) = inner.records.get_mut(id) else { return false };
        // 令牌有固定长度，比较摘要避免以字符串前缀决定匹配时间。
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, record.token.as_bytes());
        let expected = ring::hmac::sign(&key, b"sterm-agent");
        let supplied = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, token.as_bytes());
        if ring::hmac::verify(&supplied, b"sterm-agent", expected.as_ref()).is_err() { return false; }
        record.last_seen = Instant::now(); true
    }

    pub fn commands(&self, id: &str) -> Vec<Command> {
        let mut inner = self.inner.lock().unwrap();
        inner.records.get_mut(id).map(|r| r.pending.drain(..).collect()).unwrap_or_default()
    }

    /// Claude 的阻塞回答由等待 hook 独占消费，普通定时轮询只消费其他命令。
    pub fn bridge_commands(&self, id: &str) -> Vec<Command> {
        let mut inner = self.inner.lock().unwrap();
        let Some(record) = inner.records.get_mut(id) else { return vec![] };
        let (waiting, commands) = record.pending.drain(..).partition(|c| matches!(c.action, Action::RespondInteraction { .. }));
        record.pending = waiting;
        commands.into()
    }

    pub fn interaction_command(&self, id: &str, request: &str) -> Result<Option<Command>, String> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner.records.get_mut(id).ok_or("会话已结束")?;
        if !record.session.interactions.iter().any(|i| i.id == request) { return Err("交互已结束".into()); }
        let index = record.pending.iter().position(|c| matches!(&c.action, Action::RespondInteraction{request_id,..} if request_id==request));
        Ok(index.and_then(|index| record.pending.remove(index)))
    }

    pub fn submit(&self, command: Command, remote: bool) -> Result<Receipt, String> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner.records.get_mut(&command.runtime_id).ok_or("会话已结束")?;
        let s = &record.session;
        if s.generation != command.generation || s.session_id != command.session_id { return Err("会话已替换，请刷新".into()); }
        if remote && (!s.shared || !s.allow_control) { return Err("电脑端尚未允许操作此会话".into()); }
        if command.id.is_empty() || command.id.len() > 100 { return Err("无效的操作标识".into()); }
        if let Some((old, receipt, _)) = record.commands.get(&command.id) {
            if serde_json::to_value(old).ok() != serde_json::to_value(&command).ok() { return Err("操作标识已用于其他请求".into()); }
            return Ok(receipt.clone());
        }
        if s.state == "connecting" || s.state == "offline" || s.state == "exited" { return Err("Agent 尚未连接".into()); }
        Self::validate_action(s, &command.action)?;
        match &command.action {
            Action::SendText { text } if !s.capabilities.send || text.trim().is_empty() || text.len() > 32 * 1024 => return Err("文字为空、过长或不支持发送".into()),
            Action::StopTurn { turn_id } if !s.capabilities.stop || turn_id.is_empty() || *turn_id != s.turn_id => return Err("此回合已结束".into()),
            Action::SetModel { model } if !s.capabilities.models || !s.models.contains(model) || !["idle","failed","interrupted"].contains(&s.state.as_str()) => return Err("请在空闲时选择可用模型".into()),
            Action::SetPolicy { sandbox, approval_policy } if !s.capabilities.policy || !["read-only", "workspace-write"].contains(&sandbox.as_str()) || approval_policy != "on-request" || !["idle","failed","interrupted"].contains(&s.state.as_str()) => return Err("请选择空闲会话支持的权限策略".into()),
            Action::EnterPlan if !s.capabilities.plan || !["idle","failed","interrupted"].contains(&s.state.as_str()) => return Err("当前不支持切换 Plan".into()),
            Action::RespondApproval { request_id, .. } if !s.capabilities.approvals || !s.approvals.iter().any(|a| a.id == *request_id) => return Err("审批已处理或失效".into()),
            _ => {}
        }
        if matches!(command.action, Action::SetModel { .. } | Action::SetPolicy { .. } | Action::SetPermissions { .. } | Action::EnterPlan) && command.settings_revision != Some(s.settings_revision) { return Err("设置已变化，请刷新后重试".into()); }
        if record.pending.len() >= MAX_PENDING { return Err("待处理操作过多，请稍后重试".into()); }
        while record.order.len() >= MAX_COMMANDS {
            let Some(old) = record.order.front() else { break };
            if record.commands.get(old).is_some_and(|(_, r, _)| r.status == "pending" || r.status == "dispatching") { return Err("待确认操作过多，请稍后重试".into()); }
            let old = record.order.pop_front().unwrap(); record.commands.remove(&old);
        }
        let receipt = Receipt { id: command.id.clone(), status: "pending".into(), message: None };
        record.order.push_back(command.id.clone());
        record.commands.insert(command.id.clone(), (command.clone(), receipt.clone(), remote));
        record.pending.push_back(command);
        drop(inner); self.changed.notify_waiters();
        Ok(receipt)
    }

    fn validate_action(session:&Session, action:&Action)->Result<(),String>{
        if session.state=="blocked" && matches!(action,Action::SendText{..}|Action::Queue{..}) { return Err("当前会话被系统阻断，请在电脑端查看恢复选项".into()); }
        match action {
            Action::SetPermissions{profile,approval_policy,reviewer}=>{
                if !session.capabilities.policy||!["idle","failed","interrupted"].contains(&session.state.as_str()){return Err("请在空闲时修改权限".into())}
                super::interactions::validate_permissions(session,profile,approval_policy,reviewer)?;
            }
            Action::RespondInteraction{request_id,response}=>{
                let interaction=session.interactions.iter().find(|i|i.id==*request_id).ok_or("审批已处理或失效")?;
                super::interactions::validate(interaction,response)?;
            }
            Action::Queue{operation,id,text,ids}=>{
                if !session.capabilities.queue{return Err("当前 Agent 未提供队列接口".into())}
                if !["add","update","delete","reorder","start"].contains(&operation.as_str()){return Err("无效队列操作".into())}
                if ["add","update"].contains(&operation.as_str())&&(text.trim().is_empty()||text.len()>32768){return Err("队列文字为空或过长".into())}
                if ["update","delete"].contains(&operation.as_str())&&!session.queued_inputs.iter().any(|i|i["id"]==*id){return Err("队列项目已执行或删除".into())}
                if operation=="add"&&session.queued_inputs.len()>=100{return Err("原生队列已达显示上限，请先处理已有输入".into())}
                if operation=="start"&&!id.is_empty()&&!session.queued_inputs.iter().any(|i|i["id"]==*id){return Err("队列项目已执行或删除".into())}
                if operation=="reorder"&&(ids.len()!=session.queued_inputs.len()||ids.iter().enumerate().any(|(index,id)|ids[..index].contains(id)||!session.queued_inputs.iter().any(|i|i["id"]==*id))){return Err("队列已变化，请刷新".into())}
            }
            _=>{}
        }
        Ok(())
    }
    pub fn receipt(&self, id: &str, command: &str, remote: bool) -> Result<Receipt, String> {
        let inner = self.inner.lock().unwrap();
        let record = inner.records.get(id).ok_or("会话已结束")?;
        if remote && !record.session.shared { return Err("会话尚未分享".into()); }
        record.commands.get(command).map(|(_, r, _)| r.clone()).ok_or("操作记录已过期，请核对会话后再操作".into())
    }

    pub fn apply(&self, id: &str, session_id: &str, events: Vec<Event>) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner.records.get_mut(id).ok_or("会话已结束")?;
        if record.session.state=="exited"{return Err("Agent 已结束，拒绝迟到事件".into())}
        if events.iter().any(|e| matches!(e, Event::Ready { session_id: id, .. } if id != session_id)) { return Err("会话身份不匹配".into()); }
        let changed_session = events.iter().any(|e| matches!(e, Event::Ready { session_id: new, .. } if new != &record.session.session_id));
        if !changed_session && !record.session.session_id.is_empty() && record.session.session_id != session_id { return Err("旧会话事件".into()); }
        // 先验证整个批次，拒绝时不能留下部分应用但游标未前进的状态。
        for event in &events{
            let (value,limit)=match event{
                Event::Interaction{interaction}=>{
                    if interaction.id.len()>1024||interaction.title.len()>1024||interaction.detail.len()>MAX_TEXT{return Err("交互内容过长".into())}
                    (&interaction.data,MAX_TEXT)
                },
                Event::Permissions{value}=>(value,128*1024),
                Event::Task{task}=>{if task.id.len()>1024||task.parent_id.len()>1024||task.title.len()>512||task.message.len()>MAX_TEXT{return Err("子任务内容过长".into())}continue},
                Event::Queue{items}=>{if items.len()>100||serde_json::to_vec(items).map_or(true,|v|v.len()>256*1024){return Err("队列快照过长".into())}continue},
                Event::Activity{value}=>(value,MAX_TEXT),
                _=>continue,
            };
            if serde_json::to_vec(value).map_or(true,|v|v.len()>limit){return Err("Agent 事件过长".into())}
        }
        if changed_session { Self::cancel_pending(record, "原生会话已切换"); record.changes.clear(); }
        let changes = events.clone();
        for event in events {
            let s = &mut record.session;
            match event {
                Event::Ready { session_id, model, models, capabilities } => {
                    if s.session_id != session_id { s.permissions=json!({}); s.items.clear(); s.approvals.clear(); s.interactions.clear(); s.tasks.clear(); s.queued_inputs.clear(); s.activity=json!({}); s.turn_id.clear(); }
                    let can_end=s.capabilities.end;
                    s.session_id = session_id; s.model = model; s.models = models; s.capabilities = capabilities;
                    s.capabilities.end=can_end;
                    s.state = "idle".into(); s.error = None; s.settings_revision += 1;
                }
                Event::State { state, turn_id } => {
                    if !["idle","running","waiting","offline","exited","retrying","failed","interrupted","blocked"].contains(&state.as_str()) { continue; }
                    if state == "running" { s.error = None; }
                    s.state = if state=="running"&&s.interactions.iter().any(|i|i.blocking&&i.agent_id.is_empty()){"waiting".into()}else{state};
                    s.turn_id = turn_id;
                    if ["idle", "offline", "exited"].contains(&s.state.as_str()) { s.approvals.clear(); }
                }
                Event::Items { items } => { s.items.retain(|item| !item.agent_id.is_empty()); s.items.extend(items); }
                Event::Upsert { item } => {
                    if let Some(old) = s.items.iter_mut().find(|old| old.id == item.id) { *old = item; } else { s.items.push(item); }
                }
                Event::Delta { id, delta } => {
                    if let Some(item) = s.items.iter_mut().find(|old| old.id == id) {
                        if item.text.len() < MAX_TEXT { item.text.push_str(&delta); }
                    }
                }
                Event::Settings { model, policy } => {
                    if let Some(model) = model { s.model = model; }
                    if let Some(policy) = policy { s.policy = policy; }
                    s.settings_revision += 1;
                }
                Event::TaskItems { agent_id, items } => { s.items.retain(|item| item.agent_id!=agent_id); s.items.extend(items); }
                Event::Permissions { value } => { if s.permissions!=value { s.permissions=value; s.settings_revision+=1; } }
                Event::Queue { items } => { s.queued_inputs=items.into_iter().take(100).collect(); }
                Event::Activity { value } => { if serde_json::to_vec(&value).is_ok_and(|v|v.len()<=MAX_TEXT){s.activity=value;} }
                Event::Task { task } => {
                    if let Some(old)=s.tasks.iter_mut().find(|old|old.id==task.id){*old=task;}
                    else if s.tasks.len()<64{s.tasks.push(task);}
                }
                Event::Interaction { mut interaction } => {
                    interaction.detail=clipped(&interaction.detail,MAX_TEXT);
                    if let Some(old)=s.interactions.iter_mut().find(|old|old.id==interaction.id){*old=interaction;}
                    else if s.interactions.len()<64{s.interactions.push(interaction);}
                    if s.interactions.iter().any(|i| i.blocking && i.agent_id.is_empty()) && !["offline","exited","blocked"].contains(&s.state.as_str()) { s.state="waiting".into(); }
                }                Event::Approval { mut approval } => {
                    approval.detail = clipped(&approval.detail, MAX_TEXT);
                    // 待审批输入和错误会截断，统一回退快照避免客户端状态分歧。
                    if s.approvals.len() < 32 && !s.approvals.iter().any(|a| a.id == approval.id) { s.approvals.push(approval); }
                    s.state = "waiting".into();
                }
                Event::Resolved { id } => { s.interactions.retain(|i|i.id!=id); s.approvals.retain(|a| a.id != id); if s.approvals.is_empty() && !s.interactions.iter().any(|i|i.blocking&&i.agent_id.is_empty()) && s.state == "waiting" { s.state = "running".into(); } }
                Event::Result { id, ok, message } => {
                    if let Some((_, receipt, _)) = record.commands.get_mut(&id) {
                        if receipt.status == "pending" || receipt.status == "dispatching" {
                            receipt.status = if ok { "accepted" } else { "rejected" }.into(); receipt.message = message;
                        }
                    }
                }
                Event::Error { message } => { s.error = Some(clipped(&message, 2000)); }
            }
        }
        let clipped = trim(&mut record.session) || record.session.items.iter().any(|item| item.text.len() >= MAX_TEXT);
        record.session.seq += 1;
        if clipped || changes.iter().any(|event| matches!(event, Event::Items { .. } | Event::TaskItems { .. } | Event::Ready { .. } | Event::Approval { .. } | Event::Error { .. })) {
            record.changes.clear();
        } else {
            record.changes.push_back(Change { seq: record.session.seq, events: changes });
        }
        let mut bytes = record.changes.iter().map(|change| serde_json::to_vec(change).map_or(0, |v| v.len())).sum::<usize>();
        while record.changes.len() > 256 || bytes > MAX_SNAPSHOT_TEXT {
            let Some(change) = record.changes.pop_front() else { break };
            bytes = bytes.saturating_sub(serde_json::to_vec(&change).map_or(0, |v| v.len()));
        }
        record.last_seen = Instant::now();
        inner.revision += 1;
        drop(inner); self.changed.notify_waiters(); Ok(())
    }

    fn cancel_pending(record: &mut Record, reason: &str) {
        record.pending.clear();
        for (_, receipt, remote) in record.commands.values_mut() {
            if *remote && receipt.status == "pending" {
                receipt.status = "rejected".into();
                receipt.message = Some(reason.into());
            }
        }
    }

    pub fn revoke_operations(&self) {
        let mut inner = self.inner.lock().unwrap();
        for record in inner.records.values_mut() { Self::cancel_pending(record, "浏览器授权已撤销"); }
    }

    pub fn claim(&self, runtime: &str, command: &Command) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner.records.get_mut(runtime).ok_or("会话已结束")?;
        let (original, receipt, remote) = record.commands.get(&command.id).ok_or("操作已过期")?;
        if receipt.status != "pending" { return Err("操作已取消或已执行".into()); }
        if command.runtime_id != runtime || serde_json::to_value(original).ok() != serde_json::to_value(command).ok() { return Err("操作内容不匹配".into()); }
        let session = &record.session;
        let validation=Self::validate_action(&record.session,&command.action).err();
        let reason = if let Some(reason)=validation.as_deref(){Some(reason)} else if original.session_id != session.session_id || original.generation != session.generation {
            Some("网页操作权限已撤销")
        } else if *remote && (!session.shared || !session.allow_control) {
            Some("网页操作权限已撤销")
        } else if ["offline", "exited", "connecting"].contains(&session.state.as_str()) {
            Some("Agent 尚未连接")
        } else {
            match &original.action {
                Action::StopTurn { turn_id } if session.turn_id != *turn_id => Some("回合已结束"),
                Action::SetModel { .. } | Action::SetPolicy { .. } | Action::SetPermissions { .. } | Action::EnterPlan if !["idle","failed","interrupted"].contains(&session.state.as_str()) => Some("Agent 已开始运行，请稍后重试"),
                Action::SetModel { .. } | Action::SetPolicy { .. } | Action::SetPermissions { .. } | Action::EnterPlan if original.settings_revision != Some(session.settings_revision) => Some("设置已变化，请刷新后重试"),
                Action::RespondApproval { request_id, .. } if !session.approvals.iter().any(|a| a.id == *request_id) => Some("审批已处理或失效"),
                Action::RespondApproval { request_id, .. } if record.commands.values().any(|(other, receipt, _)| {
                    receipt.status == "dispatching" && matches!(&other.action, Action::RespondApproval { request_id: id, .. } if id == request_id)
                }) => Some("已有审批回复正在确认"),
                Action::RespondInteraction { request_id, .. } if record.commands.values().any(|(other, receipt, _)| receipt.status=="dispatching" && matches!(&other.action,Action::RespondInteraction{request_id:id,..} if id==request_id)) => Some("已有回答正在确认"),
                _ => None,
            }
        };
        let receipt = &mut record.commands.get_mut(&command.id).unwrap().1;
        if let Some(reason) = reason {
            receipt.status = "rejected".into(); receipt.message = Some(reason.into()); return Err(reason.into());
        }
        receipt.status = "dispatching".into();
        Ok(())
    }
    pub fn update(&self, id: &str, after: Option<u64>) -> Result<Value, String> {
        let inner = self.inner.lock().unwrap();
        let record = inner.records.get(id).ok_or("会话已结束")?;
        if !record.session.shared { return Err("会话尚未分享".into()); }
        if after == Some(record.session.seq) { return Ok(json!({"unchanged":true,"seq":record.session.seq})); }
        if let Some(after) = after {
            if after < record.session.seq && record.changes.front().is_some_and(|change| change.seq <= after + 1) {
                return Ok(json!({"changes":record.changes.iter().filter(|change| change.seq > after).collect::<Vec<_>>()}));
            }
        }
        Ok(json!({"snapshot":record.session}))
    }

    pub fn set_end_capability(&self,id:&str){
        let mut inner=self.inner.lock().unwrap();
        if let Some(record)=inner.records.get_mut(id){
            record.session.capabilities.end=true;record.changes.clear();record.session.seq+=1;inner.revision+=1;
        }
    }
    pub fn finish(&self,id:&str){
        let mut inner=self.inner.lock().unwrap();
        if let Some(record)=inner.records.get_mut(id){
            Self::cancel_pending(record,"Agent 已结束");
            record.session.state="exited".into();record.session.turn_id.clear();record.session.approvals.clear();record.session.interactions.clear();
            record.changes.clear();record.session.seq+=1;inner.revision+=1;
        }
        drop(inner);self.changed.notify_waiters();
    }

    pub fn retire(&self, tab: u32, generation: u32) {
        let mut inner = self.inner.lock().unwrap();
        inner.records.retain(|_, r| r.session.tab_id != tab || r.session.generation != generation);
        inner.revision += 1; drop(inner); self.changed.notify_waiters();
    }

    pub fn remove(&self, id: &str) { let mut inner = self.inner.lock().unwrap(); inner.records.remove(id); inner.revision += 1; drop(inner); self.changed.notify_waiters(); }

    pub fn disconnect(&self, id: &str) {
        let session = self.snapshot(id, false).ok();
        if let Some(s) = session {
            if let Some(record) = self.inner.lock().unwrap().records.get_mut(id) {
                Self::cancel_pending(record, "Agent 连接已断开，未执行操作已取消");
                record.session.approvals.clear(); record.session.interactions.retain(|i|i.kind=="asyncQuestion"); record.changes.clear();
            }
            let _ = self.apply(id, &s.session_id, vec![Event::State { state: "offline".into(), turn_id: s.turn_id }]);
        }
    }

    pub fn has_session(&self, agent: &str, native_id: &str) -> bool {
        self.inner.lock().unwrap().records.values().any(|r| r.session.agent == agent && r.session.session_id == native_id && r.session.state != "exited")
    }

    pub fn has_tab(&self, tab: u32, generation: u32) -> bool {
        self.inner.lock().unwrap().records.values().any(|r| r.session.tab_id == tab && r.session.generation == generation)
    }

    pub fn is_alive(&self, id: &str) -> bool { self.inner.lock().unwrap().records.get(id).is_some_and(|record| record.session.state != "exited") }

    pub fn expire_connections(&self) {
        let ids: Vec<_> = self.inner.lock().unwrap().records.iter()
            .filter(|(_, r)| r.session.agent == "claude" && r.last_seen.elapsed() > Duration::from_secs(8) && !["offline","exited"].contains(&r.session.state.as_str()))
            .map(|(id, _)| id.clone()).collect();
        for id in ids { self.disconnect(&id); }
    }
}

pub type SharedHub = Arc<Hub>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_agent_exit_preserves_content_and_rejects_stale_controls() {
        let (hub,id,_)=ready();
        hub.share(&id,2,true,true).unwrap();hub.set_end_capability(&id);
        hub.apply(&id,"native",vec![Event::Items{items:vec![Item{id:"user".into(),kind:"user".into(),text:"keep history".into(),title:String::new(),status:"completed".into(),turn_id:String::new(),..Default::default()}]}]).unwrap();
        let cursor=hub.snapshot(&id,false).unwrap().seq;
        hub.finish(&id);
        let row=hub.snapshot(&id,true).unwrap();
        assert_eq!(row.state,"exited");assert_eq!(row.items[0].text,"keep history");assert!(!hub.is_alive(&id));
        assert!(hub.has_tab(1,2));assert!(hub.submit(command(&id),true).is_err());
        assert_eq!(hub.update(&id,Some(cursor)).unwrap()["snapshot"]["state"],"exited");
    }    fn ready() -> (Hub, String, String) {
        let hub = Hub::default(); let (id, token) = hub.register("claude", 1, 2, "D:/probe".into());
        hub.apply(&id, "native", vec![Event::Ready { session_id: "native".into(), model: "a".into(), models: vec!["a".into()], capabilities: Capabilities { send: true, stop: true, approvals: true, ..Default::default() } }]).unwrap();
        (hub, id, token)
    }
    fn command(id: &str) -> Command { Command { id: "operation-1".into(), runtime_id: id.into(), generation: 2, session_id: "native".into(), settings_revision: None, action: Action::SendText { text: "hello".into() } } }
    #[test]
    fn authorization_deduplication_and_revocation() {
        let (hub,id,token)=ready();
        assert!(!hub.authorize(&id,"wrong")); assert!(hub.authorize(&id,&token));
        assert!(hub.submit(command(&id),true).is_err());
        hub.share(&id,2,true,true).unwrap();
        hub.submit(command(&id),true).unwrap(); hub.submit(command(&id),true).unwrap();
        let fetched=hub.commands(&id);assert_eq!(fetched.len(),1);
        hub.claim(&id,&fetched[0]).unwrap();
        let mut conflict=command(&id); conflict.action=Action::SendText{text:"different".into()};
        assert!(hub.submit(conflict,true).is_err());
        let mut next=command(&id);next.id="next".into();hub.submit(next,true).unwrap();
        hub.share(&id,2,true,false).unwrap();
        assert!(hub.commands(&id).is_empty());
        assert_eq!(hub.receipt(&id,"next",true).unwrap().status,"rejected");
    }
    #[test]
    fn stale_turns_sessions_and_approvals_are_rejected() {
        let (hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        let mut c=command(&id);c.session_id="old".into();assert!(hub.submit(c,true).is_err());
        let mut c=command(&id);c.action=Action::StopTurn{turn_id:"old".into()};assert!(hub.submit(c,true).is_err());
        hub.apply(&id,"native",vec![Event::Approval{approval:Approval{id:"request-1".into(),title:"write".into(),detail:"a".into(),turn_id:"t".into()}}]).unwrap();
        hub.apply(&id,"native",vec![Event::Resolved{id:"request-1".into()}]).unwrap();
        let mut c=command(&id);c.action=Action::RespondApproval{request_id:"request-1".into(),allow:true};
        assert!(hub.submit(c,true).is_err());hub.retire(1,2);assert!(!hub.is_alive(&id));
    }
    #[test]
    fn fetched_commands_are_cancelled_before_claim_and_cannot_report_success() {
        let (hub,id,_)=ready(); hub.share(&id,2,true,true).unwrap();
        hub.submit(command(&id),true).unwrap(); let fetched=hub.commands(&id).pop().unwrap();
        hub.share(&id,2,true,false).unwrap();
        assert!(hub.claim(&id,&fetched).is_err());
        hub.apply(&id,"native",vec![Event::Result{id:fetched.id.clone(),ok:true,message:None}]).unwrap();
        assert_eq!(hub.receipt(&id,&fetched.id,true).unwrap().status,"rejected");
    }
    #[test]
    fn claims_bind_payload_and_reject_late_approvals() {
        let (hub,id,_)=ready(); hub.share(&id,2,true,true).unwrap();
        hub.submit(command(&id),true).unwrap(); let mut fetched=hub.commands(&id).pop().unwrap();
        fetched.action=Action::SendText{text:"changed".into()}; assert!(hub.claim(&id,&fetched).is_err());
        hub.apply(&id,"native",vec![Event::Approval{approval:Approval{id:"request".into(),title:"write".into(),detail:"a".into(),turn_id:"turn".into()}}]).unwrap();
        let mut a=command(&id);a.id="approval".into();a.action=Action::RespondApproval{request_id:"request".into(),allow:true};
        hub.submit(a.clone(),true).unwrap();hub.commands(&id);
        hub.apply(&id,"native",vec![Event::Resolved{id:"request".into()}]).unwrap();
        assert!(hub.claim(&id,&a).is_err()); assert_eq!(hub.receipt(&id,&a.id,true).unwrap().status,"rejected");
    }
    #[test]
    fn concurrent_approval_replies_dispatch_once() {
        let (hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        hub.apply(&id,"native",vec![Event::Approval{approval:Approval{id:"request".into(),title:"write".into(),detail:"a".into(),turn_id:"turn".into()}}]).unwrap();
        let mut a=command(&id);a.id="approval-a".into();a.action=Action::RespondApproval{request_id:"request".into(),allow:true};
        let mut b=a.clone();b.id="approval-b".into();
        hub.submit(a.clone(),true).unwrap();hub.submit(b.clone(),true).unwrap();hub.commands(&id);
        hub.claim(&id,&a).unwrap();assert!(hub.claim(&id,&b).is_err());assert!(hub.claim(&id,&a).is_err());
    }
    #[test]
    fn cursors_replay_deltas_and_recover_after_snapshots_and_limits() {
        let (hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        let cursor=hub.snapshot(&id,false).unwrap().seq;
        let entry=Item{id:"a".into(),kind:"assistant".into(),title:"".into(),text:"one".into(),status:"running".into(),turn_id:"t".into(),..Default::default()};
        hub.apply(&id,"native",vec![Event::Upsert{item:entry.clone()}]).unwrap();
        hub.apply(&id,"native",vec![Event::Delta{id:"a".into(),delta:" two".into()}]).unwrap();
        assert_eq!(hub.update(&id,Some(cursor)).unwrap()["changes"].as_array().unwrap().len(),2);
        let latest=hub.snapshot(&id,false).unwrap();assert_eq!(latest.items[0].text,"one two");
        assert_eq!(hub.update(&id,Some(latest.seq)).unwrap()["unchanged"],true);
        hub.apply(&id,"native",vec![Event::Items{items:vec![entry]}]).unwrap();
        assert!(hub.update(&id,Some(latest.seq)).unwrap().get("snapshot").is_some());
        hub.apply(&id,"native",vec![Event::Delta{id:"a".into(),delta:"中".repeat(MAX_TEXT)}]).unwrap();
        let session=hub.snapshot(&id,false).unwrap();assert!(session.items[0].text.len()<=MAX_TEXT);
        assert!(session.items[0].text.ends_with("内容已截断"));
        assert!(hub.update(&id,Some(latest.seq)).unwrap().get("snapshot").is_some());
    }
    #[test]
    fn settings_revision_and_connection_loss_cancel_stale_operations() {
        let (hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        hub.apply(&id,"native",vec![Event::Ready{session_id:"native".into(),model:"a".into(),models:vec!["a".into()],capabilities:Capabilities{models:true,..Default::default()}}]).unwrap();
        let mut c=command(&id);c.action=Action::SetModel{model:"a".into()};c.settings_revision=Some(hub.snapshot(&id,false).unwrap().settings_revision);
        hub.submit(c.clone(),true).unwrap();hub.commands(&id);
        hub.apply(&id,"native",vec![Event::Settings{model:Some("a".into()),policy:None}]).unwrap();
        assert!(hub.claim(&id,&c).is_err());
        hub.disconnect(&id);assert_eq!(hub.snapshot(&id,false).unwrap().state,"offline");
        let public=serde_json::to_string(&hub.list(true)).unwrap();assert!(!public.contains("token"));
    }
    #[test]
    fn typed_interactions_compete_once_and_reject_late_or_revoked_answers(){
        let (hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        let interaction=Interaction{id:"question".into(),kind:"question".into(),blocking:true,data:json!({"questions":[{"id":"q","multiSelect":false}]}),..Default::default()};
        hub.apply(&id,"native",vec![Event::Interaction{interaction}]).unwrap();
        let mut a=command(&id);a.id="answer-a".into();a.action=Action::RespondInteraction{request_id:"question".into(),response:json!({"answers":{"q":["A"]}})};
        let mut b=a.clone();b.id="answer-b".into();
        hub.submit(a.clone(),true).unwrap();hub.submit(b.clone(),true).unwrap();
        assert!(hub.bridge_commands(&id).is_empty());
        assert_eq!(hub.interaction_command(&id,"question").unwrap().unwrap().id,a.id);
        hub.claim(&id,&a).unwrap();assert!(hub.claim(&id,&b).is_err());
        hub.apply(&id,"native",vec![Event::Resolved{id:"question".into()}]).unwrap();
        let mut late=a.clone();late.id="late".into();assert!(hub.submit(late,true).is_err());
        hub.disconnect(&id);assert!(hub.snapshot(&id,false).unwrap().interactions.is_empty());
    }

    #[test]
    fn child_questions_and_items_survive_main_turn_completion(){
        let(hub,id,_)=ready();
        let child=Interaction{id:"child-question".into(),agent_id:"child".into(),kind:"question".into(),blocking:true,..Default::default()};
        hub.apply(&id,"native",vec![Event::Interaction{interaction:child},Event::Task{task:AgentTask{id:"child".into(),state:"waiting".into(),..Default::default()}},Event::Upsert{item:Item{id:"child:a".into(),agent_id:"child".into(),text:"child text".into(),..Default::default()}},Event::State{state:"idle".into(),turn_id:"".into()},Event::Items{items:vec![]}]).unwrap();
        let s=hub.snapshot(&id,false).unwrap();
        assert_eq!(s.state,"idle");assert_eq!(s.interactions[0].agent_id,"child");assert_eq!(s.tasks[0].state,"waiting");assert_eq!(s.items[0].text,"child text");
    }

    #[test]
    fn permission_catalog_and_queue_prevent_invented_or_stale_commands(){
        let(hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        hub.apply(&id,"native",vec![Event::Ready{session_id:"native".into(),model:"a".into(),models:vec![],capabilities:Capabilities{policy:true,queue:true,..Default::default()}},Event::Permissions{value:json!({"options":[{"profile":":danger-full-access","approvalPolicy":"never","reviewer":"user","allowed":true}]})},Event::Queue{items:vec![json!({"id":"one","text":"a"}),json!({"id":"two","text":"b"})]}]).unwrap();
        let mut c=command(&id);c.settings_revision=Some(hub.snapshot(&id,false).unwrap().settings_revision);
        c.action=Action::SetPermissions{profile:":danger-full-access".into(),approval_policy:"on-request".into(),reviewer:"user".into()};assert!(hub.submit(c.clone(),true).is_err());
        c.action=Action::SetPermissions{profile:":danger-full-access".into(),approval_policy:"never".into(),reviewer:"user".into()};assert!(hub.submit(c.clone(),true).is_ok());
        c.id="queue".into();c.action=Action::Queue{operation:"reorder".into(),id:"".into(),text:"".into(),ids:vec!["one".into(),"one".into()]};assert!(hub.submit(c.clone(),true).is_err());
        c.action=Action::Queue{operation:"start".into(),id:"deleted".into(),text:"".into(),ids:vec![]};assert!(hub.submit(c,true).is_err());
    }

    #[test]
    fn blocked_session_rejects_input_and_batch_rejection_is_atomic(){
        let(hub,id,_)=ready();hub.share(&id,2,true,true).unwrap();
        hub.apply(&id,"native",vec![Event::State{state:"blocked".into(),turn_id:"".into()}]).unwrap();
        assert!(hub.submit(command(&id),true).is_err());
        let before=hub.snapshot(&id,false).unwrap();
        assert!(hub.apply(&id,"native",vec![Event::State{state:"running".into(),turn_id:"bad".into()},Event::Interaction{interaction:Interaction{data:json!({"large":"x".repeat(MAX_TEXT+1)}),..Default::default()}}]).is_err());
        let after=hub.snapshot(&id,false).unwrap();assert_eq!(before.seq,after.seq);assert_eq!(after.state,"blocked");
    }

}
