//! JSON-RPC 多连接客户端。只连接 sTerm 自有 app-server，不接管用户 daemon。
use super::{hub::SharedHub, protocol::*, codex_mapping as mapping};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

struct PendingApproval { native_id: Value, params: Value, interaction: Interaction, waiter: Option<oneshot::Sender<()>>, sent: bool }

pub struct Client {
    tx: mpsc::Sender<Value>,
    waiters: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    next: AtomicU64,
    bindings: Arc<Mutex<HashMap<String, (String, String)>>>,
    approvals: Arc<Mutex<HashMap<String, PendingApproval>>>,
    seen_async: Mutex<HashSet<String>>,
    hub: SharedHub,
}

pub fn item(value: &Value, turn: &str) -> Option<Item> {
    let kind = value["type"].as_str()?;
    let (kind, title, text) = match kind {
        "userMessage" => ("user", String::new(), value["content"].as_array()?.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")),
        "agentMessage" => ("assistant", String::new(), value["text"].as_str().unwrap_or_default().into()),
        "reasoning" => ("thinking", "思考摘要".into(), value["summary"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default()),
        "plan" => ("assistant", "计划".into(), value["text"].as_str().unwrap_or_default().into()),
        "commandExecution" => ("tool", "命令执行".into(), format!("{}\n\n{}", value["command"].as_str().unwrap_or_default(), value["aggregatedOutput"].as_str().unwrap_or_default())),
        "fileChange" => ("tool", "文件变更".into(), serde_json::to_string_pretty(&value["changes"]).unwrap_or_default()),
        "collabAgentToolCall" | "subAgentActivity" => ("tool", "协作任务".into(), serde_json::to_string_pretty(value).unwrap_or_default()),
        "mcpToolCall" | "dynamicToolCall" => ("tool", value["tool"].as_str().unwrap_or("工具").into(), serde_json::to_string_pretty(value).unwrap_or_default()),
        "functionCallOutput" => ("tool", value["name"].as_str().unwrap_or("工具").into(), serde_json::to_string_pretty(&value["output"]).unwrap_or_default()),
        _ => return None,
    };
    Some(Item { id: value["id"].as_str()?.into(), kind: kind.into(), title, text,
        status: value["status"].as_str().unwrap_or("completed").into(), turn_id: turn.into(), agent_id: String::new() })
}

fn settings(value: &Value) -> Event {
    let sandbox = value.get("sandboxPolicy").or_else(|| value.get("sandbox"));
    Event::Settings { model: value["model"].as_str().map(str::to_string),
        policy: Some(format!("{} · {}", sandbox.and_then(|s| s["type"].as_str()).unwrap_or("unknown"), value["approvalPolicy"].as_str().unwrap_or("managed"))) }
}
impl Client {
    pub async fn connect(endpoint: &str, token: &str, hub: SharedHub) -> Result<Arc<Self>, String> {
        let mut request = endpoint.into_client_request().map_err(|e| e.to_string())?;
        request.headers_mut().insert("authorization", HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?);
        let (socket, _) = tokio_tungstenite::connect_async(request).await.map_err(|e| e.to_string())?;
        let (mut writer, mut reader) = socket.split();
        let (tx, mut rx) = mpsc::channel::<Value>(128);
        let client = Arc::new(Self { tx, waiters: Default::default(), next: AtomicU64::new(1), bindings: Default::default(), approvals: Default::default(), seen_async: Default::default(), hub });
        let weak = Arc::downgrade(&client);
        let endpoint = endpoint.to_string(); let token = token.to_string();
        tokio::spawn(async move {
          loop {
            loop {
                tokio::select! {
                    out = rx.recv() => {
                        let Some(out) = out else { break };
                        #[cfg(debug_assertions)]
                        if out["_sterm_disconnect"] == true { let _ = writer.send(Message::Close(None)).await; break; }
                        if writer.send(Message::Text(out.to_string().into())).await.is_err() { break; }
                    }
                    incoming = reader.next() => {
                        let Some(Ok(message)) = incoming else { break };
                        match message {
                            Message::Text(text) => {
                                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                                let Some(client) = weak.upgrade() else { break };
                                if value.get("method").is_some() { client.notification(value); }
                                else if let Some(id) = value["id"].as_u64() {
                                    if let Some(waiter) = client.waiters.lock().unwrap().remove(&id) {
                                        let answer = if value.get("error").is_some() { Err(value["error"].to_string()) } else { Ok(value["result"].clone()) };
                                        let _ = waiter.send(answer);
                                    }
                                }
                            }
                            Message::Ping(data) => { if writer.send(Message::Pong(data)).await.is_err() { break; } }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            }
            let Some(client) = weak.upgrade() else { break };
            for (runtime, _) in client.bindings.lock().unwrap().values() { client.hub.disconnect(runtime); }
            client.seen_async.lock().unwrap().clear();
            client.waiters.lock().unwrap().clear();
            client.approvals.lock().unwrap().clear();
            while rx.try_recv().is_ok() {}
            drop(client);
            let mut delay = 250;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                if weak.strong_count() == 0 { return; }
                let Ok(mut request) = endpoint.clone().into_client_request() else { return };
                let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {token}")) else { return };
                request.headers_mut().insert("authorization", bearer);
                if let Ok((socket, _)) = tokio_tungstenite::connect_async(request).await {
                    (writer, reader) = socket.split();
                    if let Some(client) = weak.upgrade() {
                        tokio::spawn(async move {
                            if client.initialize().await.is_err() { return; }
                            let bindings = client.bindings.lock().unwrap().clone();
                            for (thread, (runtime, root)) in bindings {
                                if !client.hub.is_alive(&runtime) { continue; }
                                match client.call("thread/resume", json!({"threadId":thread,"excludeTurns":true})).await {
                                    Ok(response) => { if thread==root { let _ = client.hydrate(&runtime, &thread, &response).await; } else { client.refresh_child(&runtime,&root,&thread).await; } }
                                    Err(message) => { let _ = client.hub.apply(&runtime, &root, vec![Event::Error { message }]); }
                                }
                            }
                        });
                    }
                    break;
                }
                delay = (delay * 2).min(3000);
            }
          }
        });
        client.initialize().await?;
        Ok(client)
    }

    #[cfg(debug_assertions)]
    pub async fn debug_disconnect(&self) -> Result<(), String> { self.tx.send(json!({"_sterm_disconnect":true})).await.map_err(|e|e.to_string()) }

    async fn initialize(&self) -> Result<(), String> {
        self.call("initialize", json!({"clientInfo":{"name":"sterm","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        self.tx.send(json!({"method":"initialized"})).await.map_err(|e| e.to_string())
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(id, tx);
        if self.tx.send(json!({"id":id,"method":method,"params":params})).await.is_err() {
            self.waiters.lock().unwrap().remove(&id); return Err("Codex 后端已断开".into());
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), rx).await;
        self.waiters.lock().unwrap().remove(&id);
        match result { Ok(Ok(result)) => result, _ => Err("Codex 操作结果尚未确认，请核对会话后再操作".into()) }
    }

    pub async fn bind(self: &Arc<Self>, runtime: &str, thread: &str, response: &Value) -> Result<(), String> {
        self.bindings.lock().unwrap().insert(thread.into(), (runtime.into(), thread.into()));
        self.hydrate(runtime, thread, response).await?;
        let client = self.clone(); let runtime = runtime.to_string(); let thread = thread.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(150));
            while client.hub.is_alive(&runtime) {
                interval.tick().await;
                for command in client.hub.commands(&runtime) {
                    let adapter = client.clone(); let thread = thread.clone(); let runtime = runtime.clone();
                    let is_approval = matches!(command.action, Action::RespondApproval { .. } | Action::RespondInteraction { .. });
                    let dispatch = async move {
                        let result = adapter.execute(&thread, &command).await;
                        let message = match result.as_ref() {
                            Ok(()) if is_approval => Some("原生 Agent 已处理回答".into()),
                            Err(error) => Some(error.clone()),
                            _ => None,
                        };
                        let _ = adapter.hub.apply(&runtime, &thread, vec![Event::Result { id: command.id, ok: result.is_ok(), message }]);
                    };
                    // 审批确认不阻挡停止；其他操作按拉取顺序提交上游，避免设置与输入互相越过。
                    if is_approval { tokio::spawn(dispatch); } else { dispatch.await; }
                }
            }
            // TUI 退出或标签关闭后，释放共用后端中的对应回合与订阅。
            // 保留网页历史不代表 Agent 仍存活，不能让后台继续执行。
            if client.release_thread(&thread).await.is_err() {
                crate::diagnostics::record(&format!("agent_runtime codex_cleanup_failed runtime={runtime}"));
            }
            let children:Vec<_>=client.bindings.lock().unwrap().iter().filter(|(id,(_,root))|root==&thread&&*id!=&thread).map(|(id,_)|id.clone()).collect();
            for child in children {
                let _=client.release_thread(&child).await;
                client.bindings.lock().unwrap().remove(&child);
                client.seen_async.lock().unwrap().retain(|key|!key.starts_with(&format!("{child}:")));
                client.approvals.lock().unwrap().retain(|id,_|!id.starts_with(&format!("{child}:")));
            }
            client.bindings.lock().unwrap().remove(&thread);
            client.seen_async.lock().unwrap().retain(|key|!key.starts_with(&format!("{thread}:")));
            client.approvals.lock().unwrap().retain(|id, _| !id.starts_with(&format!("{thread}:")));
        });
        Ok(())
    }

    async fn release_thread(&self, thread: &str) -> Result<(), String> {
        let turns = self.call("thread/turns/list", json!({"threadId":thread,"limit":1,"sortDirection":"desc"})).await?;
        for turn in turns["data"].as_array().into_iter().flatten().filter(|turn| turn["status"] == "inProgress") {
            let id = turn["id"].as_str().ok_or("Codex 活跃回合没有标识")?;
            self.call("turn/interrupt", json!({"threadId":thread,"turnId":id})).await?;
        }
        self.call("thread/unsubscribe", json!({"threadId":thread})).await?;
        Ok(())
    }

    async fn hydrate(&self, runtime: &str, thread: &str, response: &Value) -> Result<(), String> {
        let mut models: Vec<String> = self.call("model/list", json!({})).await.ok().and_then(|v| v["data"].as_array().cloned()).unwrap_or_default()
            .iter().filter_map(|m| m["model"].as_str().map(str::to_string)).collect();
        if let Some(model) = response["model"].as_str() { if !models.iter().any(|item| item == model) { models.push(model.into()); } }
        self.hub.apply(runtime, thread, vec![Event::Ready { session_id: thread.into(),
            model: response["model"].as_str().unwrap_or_default().into(), models,
            capabilities: Capabilities { send: true, stop: true, models: true, plan: false, policy: true, approvals: true, end: false, queue: true } },
            Event::Settings { model: None, policy: Some(format!("{} · {}", response["sandbox"]["type"].as_str().unwrap_or_default(), response["approvalPolicy"].as_str().unwrap_or_default())) }])?;
        self.load_permissions(runtime,thread,response).await;
        self.refresh_queue(runtime,thread).await;
        self.refresh(runtime, thread).await;
        if let Ok(turns) = self.call("thread/turns/list", json!({"threadId":thread,"limit":1,"sortDirection":"desc"})).await {
            if let Some(turn) = turns["data"].as_array().and_then(|rows| rows.first()) {
                let (state,activity)=if turn["status"]=="inProgress"{("running",json!({}))}else{mapping::terminal_state(turn)};
                let mut events=vec![Event::Activity{value:activity},Event::State{state:state.into(),turn_id:if state=="running"{turn["id"].as_str().unwrap_or_default().into()}else{String::new()}}];
                if state!="running"{
                    for interaction in self.hub.snapshot(runtime,false)?.interactions.into_iter().filter(|i|i.agent_id.is_empty()&&i.kind=="asyncQuestion"){events.push(Event::Resolved{id:interaction.id});}
                }
                self.hub.apply(runtime,thread,events)?;
            }
        }
        Ok(())
    }
    async fn execute(&self, thread: &str, command: &Command) -> Result<(), String> {
        self.hub.claim(&command.runtime_id, command)?;
        match &command.action {
            Action::SendText { text } => { self.call("turn/start", json!({"threadId":thread,"input":[{"type":"text","text":text}]})).await?; }
            Action::StopTurn { turn_id } => { self.call("turn/interrupt", json!({"threadId":thread,"turnId":turn_id})).await?; }
            Action::SetModel { model } => {
                self.call("thread/settings/update", json!({"threadId":thread,"model":model})).await?;
                self.confirm_settings(&command.runtime_id, Some(model), None).await?;
            }
            Action::RespondApproval { request_id, allow } => {
                self.respond(&command.runtime_id,thread,request_id,&json!({"decision":if *allow {"accept"} else {"cancel"}})).await?;
            }
            Action::RespondInteraction { request_id, response } => {
                self.respond(&command.runtime_id,thread,request_id,response).await?;
            }
            Action::SetPermissions { profile, approval_policy, reviewer } => {
                self.call("thread/settings/update",json!({"threadId":thread,"permissions":profile,"approvalPolicy":approval_policy,"approvalsReviewer":reviewer})).await?;
                for index in 0..50 {
                    let s=self.hub.snapshot(&command.runtime_id,false)?;
                    if s.permissions["current"]["profile"]==*profile && s.permissions["current"]["approvalPolicy"]==*approval_policy && s.permissions["current"]["reviewer"]==*reviewer { return Ok(()); }
                    if index<49 { tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
                }
                return Err("原生 Agent 尚未确认权限变化，请核对终端".into());
            }
            Action::Queue { operation,id,text,ids } => {
                let mut params=json!({"threadId":thread});
                match operation.as_str() {
                    "add"|"update"=>{params["input"]=json!([{"type":"text","text":text,"text_elements":[]}]);}
                    "reorder"=>{params["queuedSubmissionIds"]=json!(ids);}
                    _=>{}
                }
                if !id.is_empty(){params["queuedSubmissionId"]=json!(id);}
                if operation=="add"{params["clientUserMessageId"]=json!(command.id);}
                self.call(&format!("thread/queue/{operation}"),params).await?;
                self.refresh_queue(&command.runtime_id,thread).await;
            }
            Action::SetPolicy { sandbox, approval_policy } => {
                let profile=if sandbox=="read-only"{":read-only"}else{":workspace"};
                let session=self.hub.snapshot(&command.runtime_id,false)?;
                super::interactions::validate_permissions(&session,profile,approval_policy,"user")?;
                self.call("thread/settings/update", json!({"threadId":thread,"permissions":profile,"approvalPolicy":approval_policy,"approvalsReviewer":"user"})).await?;
                self.confirm_settings(&command.runtime_id, None, Some(if sandbox == "read-only" {"readOnly"} else {"workspaceWrite"})).await?;
            }
            Action::EnterPlan => return Err("当前 Codex 适配器未启用 Plan 操作".into()),
        }
        Ok(())
    }

    async fn confirm_settings(&self, runtime: &str, model: Option<&str>, policy: Option<&str>) -> Result<(), String> {
        // RPC 返回接收结果后，还要核对原生设置通知，不能先覆盖本地展示。
        for _ in 0..50 {
            let session = self.hub.snapshot(runtime, false)?;
            if model.is_none_or(|value| session.model == value) && policy.is_none_or(|value| session.policy.starts_with(value)) { return Ok(()); }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Err("原生 Agent 尚未确认设置变化，请核对终端".into())
    }
    async fn refresh(&self, runtime: &str, thread: &str) {
        if let Ok(history) = self.call("thread/items/list", json!({"threadId":thread,"limit":300,"sortDirection":"desc"})).await {
            let mut rows: Vec<_> = history["data"].as_array().into_iter().flatten().filter_map(|entry| item(&entry["item"], entry["turnId"].as_str().unwrap_or_default())).collect();
            rows.reverse();
            let _ = self.hub.apply(runtime, thread, vec![Event::Items { items: rows }]);
            let mut resolved=vec![];
            for entry in history["data"].as_array().into_iter().flatten(){
                if entry["item"]["type"]=="userMessage"{self.resolve_async_answers(thread,&entry["item"],&mut resolved);}
            }
            if !resolved.is_empty(){let _=self.hub.apply(runtime,thread,resolved);}
        }
    }

    fn notification(self: &Arc<Self>, value: Value) {
        let method=value["method"].as_str().unwrap_or_default();
        let p=&value["params"];
        // 子线程仅挂到已接入父线程，不能因广播泄漏其他会话内容。
        if method=="thread/started" {
            let child=&p["thread"];
            if let (Some(id),Some(parent))=(child["id"].as_str(),child["parentThreadId"].as_str()) {
                let binding=self.bindings.lock().unwrap().get(parent).cloned();
                if let Some((runtime,root))=binding {
                    let task=AgentTask{id:id.into(),parent_id:if parent==root{String::new()}else{parent.into()},title:child["agentNickname"].as_str().unwrap_or("子 Agent").into(),model:child["model"].as_str().unwrap_or_default().into(),state:"running".into(),..Default::default()};
                    let _=self.hub.apply(&runtime,&root,vec![Event::Task{task}]);
                    self.attach_child(runtime,root,id.to_string());
                }
            }
            return;
        }
        let thread=p["threadId"].as_str().unwrap_or_default();
        let binding=self.bindings.lock().unwrap().get(thread).cloned();
        let Some((runtime,root))=binding else{return};
        let agent=if thread==root{""}else{thread};
        let turn=p["turnId"].as_str().unwrap_or_default();
        let mut events=vec![];
        if value.get("id").is_some(){
            let id=format!("{thread}:{}",value["id"]);
            if let Some(interaction)=mapping::request(method,&id,p,agent){
                self.approvals.lock().unwrap().entry(id).or_insert(PendingApproval{native_id:value["id"].clone(),params:p.clone(),interaction:interaction.clone(),waiter:None,sent:false});
                events.push(Event::Interaction{interaction});
            }
        } else {
            match method {
                "turn/started"=>{
                    let turn=p["turn"]["id"].as_str().unwrap_or_default();
                    if agent.is_empty(){events.push(Event::Activity{value:json!({})});events.push(Event::State{state:"running".into(),turn_id:turn.into()});}
                    else{events.push(Event::Task{task:self.task_state(&runtime,agent,"running",turn,"")});}
                }
                "turn/completed"=>{
                    let (state,activity)=mapping::terminal_state(&p["turn"]);
                    let pending:Vec<_>=self.approvals.lock().unwrap().iter().filter(|(id,r)|id.starts_with(&format!("{thread}:"))&&(r.interaction.turn_id.is_empty()||r.interaction.turn_id==p["turn"]["id"])).map(|(id,_)|id.clone()).collect();
                    for id in pending{self.approvals.lock().unwrap().remove(&id);events.push(Event::Resolved{id});}
                    if let Ok(session)=self.hub.snapshot(&runtime,false){
                        for i in session.interactions.into_iter().filter(|i|i.agent_id==agent&&i.turn_id==p["turn"]["id"]){
                            events.push(Event::Resolved{id:i.id});
                        }
                    }
                    if agent.is_empty(){events.push(Event::Activity{value:activity});events.push(Event::State{state:state.into(),turn_id:String::new()});}
                    else{events.push(Event::Task{task:self.task_state(&runtime,agent,if state=="idle"{"completed"}else{state},"",activity["message"].as_str().unwrap_or_default())});}
                }
                "item/started"|"item/completed"=>{
                    let native=&p["item"];
                    if let Some(mut item)=mapping::scoped_item(native,turn,agent){if method=="item/started"{item.status="running".into();}events.push(Event::Upsert{item});}
                    self.track_collaboration(&runtime,&root,thread,native,&mut events);
                    if native["type"]=="agentMessage"&&native["delivery"]=="async"{self.async_questions(thread,turn,agent,native,&mut events);}
                    if native["type"]=="userMessage"{self.resolve_async_answers(thread,native,&mut events);}
                }
                "item/agentMessage/delta"|"item/commandExecution/outputDelta"=>{
                    let id=p["itemId"].as_str().unwrap_or_default();
                    events.push(Event::Delta{id:if agent.is_empty(){id.into()}else{format!("{agent}:{id}")},delta:p["delta"].as_str().unwrap_or_default().into()});
                    if agent.is_empty() { if self.hub.snapshot(&runtime,false).is_ok_and(|s|s.state=="retrying"){events.push(Event::State{state:"running".into(),turn_id:turn.into()});events.push(Event::Activity{value:json!({})});} }
                }
                "thread/settings/updated" if agent.is_empty()=>{
                    let native=&p["threadSettings"];
                    events.push(settings(native));
                    if let Ok(s)=self.hub.snapshot(&runtime,false){
                        let mut catalog=s.permissions;catalog["current"]=mapping::permission_current(native);events.push(Event::Permissions{value:catalog});
                    }
                }
                "thread/queue/changed" if agent.is_empty()=>{
                    let client=self.clone();let runtime=runtime.clone();let root=root.clone();
                    tokio::spawn(async move{client.refresh_queue(&runtime,&root).await;});
                }
                "thread/status/changed"=>{
                    let status=&p["status"];
                    if !agent.is_empty(){
                        let state=match status["type"].as_str(){Some("active")=>if status["activeFlags"].as_array().is_some_and(|f|!f.is_empty()){"waiting"}else{"running"},Some("systemError")=>"failed",Some("idle")=>"idle",_=>"unknown"};
                        events.push(Event::Task{task:self.task_state(&runtime,agent,state,"","")});
                    }
                }
                "thread/closed"=>{
                    if agent.is_empty(){events.push(Event::State{state:"offline".into(),turn_id:String::new()});}
                    else{events.push(Event::Task{task:self.task_state(&runtime,agent,"completed","","子线程已关闭")});}
                }
                "serverRequest/resolved"=>{
                    let id=format!("{thread}:{}",p["requestId"]);
                    if let Some(pending)=self.approvals.lock().unwrap().remove(&id){if let Some(waiter)=pending.waiter{let _=waiter.send(());}}
                    events.push(Event::Resolved{id});
                }
                "error"=>{
                    let retry=p["willRetry"]==true;
                    let state=if retry{"retrying"}else{"failed"};
                    let activity=json!({"kind":state,"message":p["error"]["message"],"code":p["error"]["codexErrorInfo"]});
                    if agent.is_empty(){events.push(Event::Activity{value:activity});events.push(Event::State{state:state.into(),turn_id:turn.into()});}
                    else{events.push(Event::Task{task:self.task_state(&runtime,agent,state,turn,activity["message"].as_str().unwrap_or_default())});}
                }
                _=>{}
            }
        }
        if !events.is_empty(){let _=self.hub.apply(&runtime,&root,events);}
    }
}
impl Client {
    async fn load_permissions(&self,runtime:&str,root:&str,response:&Value){
        let mut profiles=vec![]; let mut cursor=Value::Null;
        for _ in 0..16{
            let Ok(page)=self.call("permissionProfile/list",json!({"cwd":response["cwd"],"cursor":cursor,"limit":100})).await else{break};
            profiles.extend(page["data"].as_array().into_iter().flatten().cloned());
            cursor=page["nextCursor"].clone();if cursor.is_null(){break;}
        }
        let requirements=self.call("configRequirements/read",Value::Null).await;
        let mut features=vec![];let mut cursor=Value::Null;
        for _ in 0..16{
            let Ok(page)=self.call("experimentalFeature/list",json!({"limit":100,"cursor":cursor})).await else{break};
            features.extend(page["data"].as_array().into_iter().flatten().cloned());
            cursor=page["nextCursor"].clone();if cursor.is_null(){break;}
        }
        let auto=features.iter().any(|f|(f["name"]=="auto_review"||f["name"]=="guardian_approval")&&f["enabled"]==true);
        let options=if let Ok(requirements)=requirements{mapping::permission_options(&profiles,&requirements["requirements"],auto)}else{vec![]};
        let _=self.hub.apply(runtime,root,vec![Event::Permissions{value:json!({"current":mapping::permission_current(response),"options":options})}]);
    }

    async fn refresh_queue(&self,runtime:&str,root:&str){
        let mut items=vec![];let mut cursor=Value::Null;
        for _ in 0..16{
            let Ok(page)=self.call("thread/queue/list",json!({"threadId":root,"cursor":cursor,"limit":100})).await else{return};
            items.extend(page["data"].as_array().into_iter().flatten().map(|entry|json!({"id":entry["id"],"text":entry["input"].as_array().into_iter().flatten().filter_map(|i|i["text"].as_str()).collect::<Vec<_>>().join("\n")})));
            cursor=page["nextCursor"].clone();if cursor.is_null(){break;}
        }
        let _=self.hub.apply(runtime,root,vec![Event::Queue{items}]);
    }

    async fn respond(&self,runtime:&str,root:&str,id:&str,response:&Value)->Result<(),String>{
        let interaction=self.hub.snapshot(runtime,false)?.interactions.into_iter().find(|i|i.id==id).ok_or("交互已处理或失效")?;
        if interaction.kind=="asyncQuestion"{
            let text=mapping::async_reply(&interaction,response)?;
            let thread=if interaction.agent_id.is_empty(){root}else{&interaction.agent_id};
            self.call("turn/start",json!({"threadId":thread,"input":[{"type":"text","text":text,"text_elements":[]}]})).await?;
            self.hub.apply(runtime,root,vec![Event::Resolved{id:id.into()}])?;
            return Ok(());
        }
        let (tx,rx)=oneshot::channel();
        let (native_id,result)={
            let mut pending=self.approvals.lock().unwrap();
            let request=pending.get_mut(id).ok_or("交互已处理或失效")?;
            if request.sent{return Err("已有回复正在确认".into())}
            let result=mapping::response(&request.interaction,&request.params,response)?;
            request.sent=true;request.waiter=Some(tx);
            (request.native_id.clone(),result)
        };
        self.tx.send(json!({"id":native_id,"result":result})).await.map_err(|_|"Codex 后端已断开")?;
        tokio::time::timeout(std::time::Duration::from_secs(8),rx).await.map_err(|_|"回答结果尚未确认，请核对原生终端")?.map_err(|_|"交互已失效或连接已断开".into())
    }

    fn task_state(&self,runtime:&str,agent:&str,state:&str,turn:&str,message:&str)->AgentTask{
        let mut task=self.hub.snapshot(runtime,false).ok().and_then(|s|s.tasks.into_iter().find(|t|t.id==agent)).unwrap_or_else(||AgentTask{id:agent.into(),title:"子 Agent".into(),..Default::default()});
        task.state=state.into();task.turn_id=turn.into();task.message=message.chars().take(2000).collect();task
    }

    fn attach_child(self:&Arc<Self>,runtime:String,root:String,child:String){
        if self.bindings.lock().unwrap().contains_key(&child){return}
        if self.bindings.lock().unwrap().values().filter(|(_,parent)|parent==&root).count()>=65{return}
        self.bindings.lock().unwrap().insert(child.clone(),(runtime.clone(),root.clone()));
        let client=self.clone();
        tokio::spawn(async move{
            let mut error=None;
            // 创建广播可早于子线程元数据落盘；只重试明确的短暂空记录，保持来源绑定接收期间事件。
            for attempt in 0..35{
                if !client.hub.is_alive(&runtime){return;}
                match client.call("thread/resume",json!({"threadId":child,"excludeTurns":true})).await{
                    Ok(response)=>{
                        error=None;
                        let state=match response["thread"]["status"]["type"].as_str(){Some("idle")=>"completed",Some("active")=>"running",Some("systemError")=>"failed",_=>"unknown"};
                        let task=client.task_state(&runtime,&child,state,"","");
                        let _=client.hub.apply(&runtime,&root,vec![Event::Task{task}]);break;
                    }
                    Err(message)=>{
                        let transient=message.contains("rollout")&&message.contains("is empty");
                        error=Some(message);
                        if !transient{break;}
                        tokio::time::sleep(std::time::Duration::from_millis(200*(attempt+1).min(5))).await;
                    }
                }
            }
            if let Some(error)=error{
                let task=client.task_state(&runtime,&child,"unknown","",&format!("子任务内容暂不可用：{error}"));
                let _=client.hub.apply(&runtime,&root,vec![Event::Task{task}]);
            }
            client.refresh_child(&runtime,&root,&child).await;
        });
    }

    async fn refresh_child(&self,runtime:&str,root:&str,child:&str){
        if let Ok(history)=self.call("thread/items/list",json!({"threadId":child,"limit":200,"sortDirection":"desc"})).await{
            let mut items:Vec<_>=history["data"].as_array().into_iter().flatten().filter_map(|r|mapping::scoped_item(&r["item"],r["turnId"].as_str().unwrap_or_default(),child)).collect();
            items.reverse();let _=self.hub.apply(runtime,root,vec![Event::TaskItems{agent_id:child.into(),items}]);
        }
    }

    fn track_collaboration(self:&Arc<Self>,runtime:&str,root:&str,sender:&str,item:&Value,events:&mut Vec<Event>){
        let mut children=vec![];
        if item["type"]=="subAgentActivity"{
            if let Some(id)=item["agentThreadId"].as_str(){
                let state=match item["kind"].as_str(){Some("completed")=>"completed",Some("interrupted")=>"interrupted",_=>"running"};
                let mut task=self.task_state(runtime,id,state,"","");
                task.parent_id=if sender==root{String::new()}else{sender.into()};
                task.title=item["agentPath"].as_str().unwrap_or("子 Agent").into();
                task.model=item["model"].as_str().unwrap_or_default().into();
                events.push(Event::Task{task});children.push(id.to_string());
            }
        }else if item["type"]=="collabAgentToolCall"{
            for id in item["receiverThreadIds"].as_array().into_iter().flatten().filter_map(Value::as_str){
                let native=&item["agentsStates"][id];
                let state=match native["status"].as_str(){Some("pendingInit")=>"pending",Some("completed")=>"completed",Some("errored")=>"failed",Some("shutdown")=>"killed",Some("notFound")=>"unknown",Some(s)=>s,None=>"pending"};
                let mut task=self.task_state(runtime,id,state,"",native["message"].as_str().unwrap_or_default());
                if task.title=="子 Agent"{task.title=item["prompt"].as_str().unwrap_or("子 Agent").chars().take(120).collect();}
                if task.model.is_empty(){task.model=item["model"].as_str().unwrap_or_default().into();}
                task.parent_id=if sender==root{String::new()}else{sender.into()};
                events.push(Event::Task{task});children.push(id.to_string());
            }
        }
        for child in children{if child!=root&&child!=sender{self.attach_child(runtime.into(),root.into(),child);}}
    }

    fn async_questions(&self,thread:&str,turn:&str,agent:&str,item:&Value,events:&mut Vec<Event>){
        let message=item["id"].as_str().unwrap_or_default();
        for (index,q) in item["questions"].as_array().into_iter().flatten().enumerate(){
            let qid=json!(["request_user_input_async",message,index]).to_string();
            let key=format!("{thread}:{qid}");
            {
                let mut seen=self.seen_async.lock().unwrap();
                if qid.len()>512||seen.len()>=8192||!seen.insert(key){continue;}
            }
            let mut questions=mapping::questions(&json!([q]));questions[0]["id"]=json!(qid);
            events.push(Event::Interaction{interaction:Interaction{id:format!("{thread}:async:{message}:{index}"),kind:"asyncQuestion".into(),title:"可随时回答".into(),detail:String::new(),agent_id:agent.into(),turn_id:turn.into(),blocking:false,data:json!({"questions":questions})}});
        }
    }

    fn resolve_async_answers(&self,thread:&str,item:&Value,events:&mut Vec<Event>){
        let Some((runtime,_))=self.bindings.lock().unwrap().get(thread).cloned() else{return};
        let Ok(session)=self.hub.snapshot(&runtime,false) else{return};
        for content in item["content"].as_array().into_iter().flatten(){
            let text=content["text"].as_str().unwrap_or_default();
            let Some(body)=text.strip_prefix("<send_user_message_question_reply>").and_then(|t|t.strip_suffix("</send_user_message_question_reply>")) else{continue};
            let Ok(replies)=serde_json::from_str::<Value>(body) else{continue};
            for reply in replies.as_array().into_iter().flatten(){
                for interaction in &session.interactions{
                    if interaction.kind=="asyncQuestion" && interaction.data["questions"].as_array().is_some_and(|qs|qs.iter().any(|q|q["id"]==reply["questionItemId"])){
                        events.push(Event::Resolved{id:interaction.id.clone()});
                    }
                }
            }
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->(Arc<Client>,mpsc::Receiver<Value>,String){
        let hub=Arc::new(super::super::hub::Hub::default());
        let(runtime,_)=hub.register("codex",1,2,"D:/isolated".into());
        hub.apply(&runtime,"root",vec![Event::Ready{session_id:"root".into(),model:"mock".into(),models:vec![],capabilities:Capabilities{send:true,approvals:true,..Default::default()}}]).unwrap();
        let(tx,rx)=mpsc::channel(32);
        let client=Arc::new(Client{tx,waiters:Default::default(),next:AtomicU64::new(1),bindings:Default::default(),approvals:Default::default(),seen_async:Default::default(),hub});
        client.bindings.lock().unwrap().insert("root".into(),(runtime.clone(),"root".into()));
        (client,rx,runtime)
    }
    #[tokio::test]
    async fn async_item_identity_deduplicates_and_another_clients_answer_closes_the_card(){
        let(client,_rx,runtime)=fixture();
        let item=json!({"type":"agentMessage","id":"async-item","delivery":"async","text":"","questions":[{"title":"Pick?","options":["A","B"]}]});
        client.notification(json!({"method":"item/started","params":{"threadId":"root","turnId":"turn","item":item}}));
        let s=client.hub.snapshot(&runtime,false).unwrap();assert_eq!(s.interactions.len(),1);assert!(!s.interactions[0].blocking);
        client.hub.disconnect(&runtime);
        assert_eq!(client.hub.snapshot(&runtime,false).unwrap().interactions.len(),1);
        client.hub.apply(&runtime,"root",vec![Event::State{state:"running".into(),turn_id:"turn".into()}]).unwrap();
        let qid=s.interactions[0].data["questions"][0]["id"].as_str().unwrap();
        let text=mapping::async_reply(&s.interactions[0],&json!({"answers":{qid:["B"]}})).unwrap();
        client.notification(json!({"method":"item/completed","params":{"threadId":"root","turnId":"turn","item":{"type":"userMessage","id":"answer","content":[{"type":"text","text":text}]}}}));
        client.notification(json!({"method":"item/completed","params":{"threadId":"root","turnId":"turn","item":item}}));
        assert!(client.hub.snapshot(&runtime,false).unwrap().interactions.is_empty());
    }
    #[tokio::test]
    async fn child_question_response_keeps_the_native_request_and_resolution_identity(){
        let(client,mut rx,runtime)=fixture();
        client.bindings.lock().unwrap().insert("child".into(),(runtime.clone(),"root".into()));
        client.notification(json!({"id":77,"method":"item/tool/requestUserInput","params":{"threadId":"child","turnId":"child-turn","questions":[{"id":"q","question":"Pick?","options":[{"label":"A"}]}]}}));
        let s=client.hub.snapshot(&runtime,false).unwrap();let request=s.interactions[0].clone();
        assert_eq!(request.agent_id,"child");assert_eq!(s.state,"idle");
        let child=client.clone();let id=request.id.clone();let runtime_copy=runtime.clone();
        let response=tokio::spawn(async move{child.respond(&runtime_copy,"root",&id,&json!({"answers":{"q":["A"]}})).await});
        let wire=rx.recv().await.unwrap();assert_eq!(wire["id"],77);assert_eq!(wire["result"],json!({"answers":{"q":{"answers":["A"]}}}));
        client.notification(json!({"method":"serverRequest/resolved","params":{"threadId":"child","requestId":77}}));
        assert!(response.await.unwrap().is_ok());assert!(client.hub.snapshot(&runtime,false).unwrap().interactions.is_empty());
    }
}
