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
        match &command.action {
            Action::SendText { text } if !s.capabilities.send || text.trim().is_empty() || text.len() > 32 * 1024 => return Err("文字为空、过长或不支持发送".into()),
            Action::StopTurn { turn_id } if !s.capabilities.stop || turn_id.is_empty() || *turn_id != s.turn_id => return Err("此回合已结束".into()),
            Action::SetModel { model } if !s.capabilities.models || !s.models.contains(model) || s.state != "idle" => return Err("请在空闲时选择可用模型".into()),
            Action::SetPolicy { sandbox, approval_policy } if !s.capabilities.policy || !["read-only", "workspace-write"].contains(&sandbox.as_str()) || approval_policy != "on-request" || s.state != "idle" => return Err("请选择空闲会话支持的权限策略".into()),
            Action::EnterPlan if !s.capabilities.plan || s.state != "idle" => return Err("当前不支持切换 Plan".into()),
            Action::RespondApproval { request_id, .. } if !s.capabilities.approvals || !s.approvals.iter().any(|a| a.id == *request_id) => return Err("审批已处理或失效".into()),
            _ => {}
        }
        if matches!(command.action, Action::SetModel { .. } | Action::SetPolicy { .. } | Action::EnterPlan) && command.settings_revision != Some(s.settings_revision) { return Err("设置已变化，请刷新后重试".into()); }
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
        Ok(receipt)
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
        if changed_session { Self::cancel_pending(record, "原生会话已切换"); record.changes.clear(); }
        let changes = events.clone();
        for event in events {
            let s = &mut record.session;
            match event {
                Event::Ready { session_id, model, models, capabilities } => {
                    if s.session_id != session_id { s.items.clear(); s.approvals.clear(); s.turn_id.clear(); }
                    let can_end=s.capabilities.end;
                    s.session_id = session_id; s.model = model; s.models = models; s.capabilities = capabilities;
                    s.capabilities.end=can_end;
                    s.state = "idle".into(); s.error = None; s.settings_revision += 1;
                }
                Event::State { state, turn_id } => {
                    if !["idle","running","waiting","offline","exited"].contains(&state.as_str()) { continue; }
                    if state == "running" { s.error = None; }
                    s.state = state;
                    s.turn_id = turn_id;
                    if ["idle", "offline", "exited"].contains(&s.state.as_str()) { s.approvals.clear(); }
                }
                Event::Items { items } => { s.items = items; }
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
                Event::Approval { mut approval } => {
                    approval.detail = clipped(&approval.detail, MAX_TEXT);
                    // 待审批输入和错误会截断，统一回退快照避免客户端状态分歧。
                    if s.approvals.len() < 32 && !s.approvals.iter().any(|a| a.id == approval.id) { s.approvals.push(approval); }
                    s.state = "waiting".into();
                }
                Event::Resolved { id } => { s.approvals.retain(|a| a.id != id); if s.approvals.is_empty() && s.state == "waiting" { s.state = "running".into(); } }
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
        if clipped || changes.iter().any(|event| matches!(event, Event::Items { .. } | Event::Ready { .. } | Event::Approval { .. } | Event::Error { .. })) {
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
        let reason = if original.session_id != session.session_id || original.generation != session.generation {
            Some("会话已替换")
        } else if *remote && (!session.shared || !session.allow_control) {
            Some("网页操作权限已撤销")
        } else if ["offline", "exited", "connecting"].contains(&session.state.as_str()) {
            Some("Agent 尚未连接")
        } else {
            match &original.action {
                Action::StopTurn { turn_id } if session.turn_id != *turn_id => Some("回合已结束"),
                Action::SetModel { .. } | Action::SetPolicy { .. } | Action::EnterPlan if session.state != "idle" => Some("Agent 已开始运行，请稍后重试"),
                Action::SetModel { .. } | Action::SetPolicy { .. } | Action::EnterPlan if original.settings_revision != Some(session.settings_revision) => Some("设置已变化，请刷新后重试"),
                Action::RespondApproval { request_id, .. } if !session.approvals.iter().any(|a| a.id == *request_id) => Some("审批已处理或失效"),
                Action::RespondApproval { request_id, .. } if record.commands.values().any(|(other, receipt, _)| {
                    receipt.status == "dispatching" && matches!(&other.action, Action::RespondApproval { request_id: id, .. } if id == request_id)
                }) => Some("已有审批回复正在确认"),
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
            record.session.state="exited".into();record.session.turn_id.clear();record.session.approvals.clear();
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
                record.session.approvals.clear(); record.changes.clear();
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
        hub.apply(&id,"native",vec![Event::Items{items:vec![Item{id:"user".into(),kind:"user".into(),text:"keep history".into(),title:String::new(),status:"completed".into(),turn_id:String::new()}]}]).unwrap();
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
        let entry=Item{id:"a".into(),kind:"assistant".into(),title:"".into(),text:"one".into(),status:"running".into(),turn_id:"t".into()};
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
}
