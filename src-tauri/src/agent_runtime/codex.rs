//! JSON-RPC 多连接客户端。只连接 sTerm 自有 app-server，不接管用户 daemon。
use super::{hub::SharedHub, protocol::*};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

struct PendingApproval { native_id: Value, waiter: Option<oneshot::Sender<()>>, sent: bool }

pub struct Client {
    tx: mpsc::Sender<Value>,
    waiters: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    next: AtomicU64,
    bindings: Arc<Mutex<HashMap<String, (String, String)>>>,
    approvals: Arc<Mutex<HashMap<String, PendingApproval>>>,
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
        "mcpToolCall" | "dynamicToolCall" => ("tool", value["tool"].as_str().unwrap_or("工具").into(), serde_json::to_string_pretty(value).unwrap_or_default()),
        "functionCallOutput" => ("tool", value["name"].as_str().unwrap_or("工具").into(), serde_json::to_string_pretty(&value["output"]).unwrap_or_default()),
        _ => return None,
    };
    Some(Item { id: value["id"].as_str()?.into(), kind: kind.into(), title, text,
        status: value["status"].as_str().unwrap_or("completed").into(), turn_id: turn.into() })
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
        let client = Arc::new(Self { tx, waiters: Default::default(), next: AtomicU64::new(1), bindings: Default::default(), approvals: Default::default(), hub });
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
                            for (thread, (runtime, _)) in bindings {
                                if !client.hub.is_alive(&runtime) { continue; }
                                match client.call("thread/resume", json!({"threadId":thread,"excludeTurns":true})).await {
                                    Ok(response) => { let _ = client.hydrate(&runtime, &thread, &response).await; }
                                    Err(message) => { let _ = client.hub.apply(&runtime, &thread, vec![Event::Error { message }]); }
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
                    let is_approval = matches!(command.action, Action::RespondApproval { .. });
                    let dispatch = async move {
                        let result = adapter.execute(&thread, &command).await;
                        let message = match result.as_ref() {
                            Ok(()) if is_approval => Some("审批请求已由原生 Agent 结束".into()),
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
            client.bindings.lock().unwrap().remove(&thread);
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
            capabilities: Capabilities { send: true, stop: true, models: true, plan: false, policy: true, approvals: true, end: false } },
            Event::Settings { model: None, policy: Some(format!("{} · {}", response["sandbox"]["type"].as_str().unwrap_or_default(), response["approvalPolicy"].as_str().unwrap_or_default())) }])?;
        self.refresh(runtime, thread).await;
        if let Ok(turns) = self.call("thread/turns/list", json!({"threadId":thread,"limit":1,"sortDirection":"desc"})).await {
            if let Some(turn) = turns["data"].as_array().and_then(|rows| rows.first()).filter(|turn| turn["status"] == "inProgress") {
                self.hub.apply(runtime, thread, vec![Event::State { state: "running".into(), turn_id: turn["id"].as_str().unwrap_or_default().into() }])?;
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
                let (tx, rx) = oneshot::channel();
                let native_id = {
                    let mut pending = self.approvals.lock().unwrap();
                    let approval = pending.get_mut(request_id).ok_or("审批已处理或失效")?;
                    if approval.sent { return Err("已有回复正在确认".into()); }
                    approval.sent = true; approval.waiter = Some(tx); approval.native_id.clone()
                };
                self.tx.send(json!({"id":native_id,"result":{"decision":if *allow {"accept"} else {"cancel"}}})).await.map_err(|e| e.to_string())?;
                tokio::time::timeout(std::time::Duration::from_secs(8), rx).await
                    .map_err(|_| "审批结果尚未确认，请核对原生终端".to_string())?
                    .map_err(|_| "审批已失效或连接已断开".to_string())?;
            }
            Action::SetPolicy { sandbox, approval_policy } => {
                let sandbox_policy = if sandbox == "read-only" { json!({"type":"readOnly","networkAccess":false}) }
                    else { json!({"type":"workspaceWrite","writableRoots":[],"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true}) };
                self.call("thread/settings/update", json!({"threadId":thread,"sandboxPolicy":sandbox_policy,"approvalPolicy":approval_policy})).await?;
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
        }
    }

    fn notification(self: &Arc<Self>, value: Value) {
        let method = value["method"].as_str().unwrap_or_default();
        let p = &value["params"];
        let thread = p["threadId"].as_str().unwrap_or_default();
        let Some((runtime, session)) = self.bindings.lock().unwrap().get(thread).cloned() else { return };
        let turn = p["turnId"].as_str().unwrap_or_default();
        let event = match method {
            "turn/started" => Some(Event::State { state: "running".into(), turn_id: p["turn"]["id"].as_str().unwrap_or_default().into() }),
            "turn/completed" => {
                self.approvals.lock().unwrap().retain(|id, _| !id.starts_with(&format!("{thread}:")));
                Some(Event::State { state: "idle".into(), turn_id: String::new() })
            },
            "item/started" | "item/completed" => item(&p["item"], turn).map(|mut item| { if method == "item/started" { item.status = "running".into(); } Event::Upsert { item } }),
            "thread/closed" => Some(Event::State { state: "offline".into(), turn_id: String::new() }),
            "item/agentMessage/delta" | "item/commandExecution/outputDelta" => Some(Event::Delta { id: p["itemId"].as_str().unwrap_or_default().into(), delta: p["delta"].as_str().unwrap_or_default().into() }),
            "thread/settings/updated" => Some(settings(&p["threadSettings"])),
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let id = format!("{thread}:{}", value["id"]);
                self.approvals.lock().unwrap().entry(id.clone()).or_insert(PendingApproval { native_id: value["id"].clone(), waiter: None, sent: false });
                Some(Event::Approval { approval: Approval { id, title: if method.contains("commandExecution") {"执行命令"} else {"修改文件"}.into(),
                    detail: p["command"].as_str().or(p["reason"].as_str()).unwrap_or("请在原生终端核对详细变更").into(), turn_id: turn.into() } })
            }
            "serverRequest/resolved" => {
                let id = format!("{thread}:{}", p["requestId"]);
                if let Some(pending) = self.approvals.lock().unwrap().remove(&id) { if let Some(waiter) = pending.waiter { let _ = waiter.send(()); } }
                Some(Event::Resolved { id })
            }
            "error" => Some(Event::Error { message: p["error"]["message"].as_str().unwrap_or("Codex 请求失败").into() }),
            _ => None,
        };
        if let Some(event) = event { let _ = self.hub.apply(&runtime, &session, vec![event]); }
    }
}
