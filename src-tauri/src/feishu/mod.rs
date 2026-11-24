//! 将会话完成事件发布到飞书，并接收同一话题中的文本回复。
mod api;
mod routes;
mod socket;

use crate::{agent_turn::AgentEvent, AppState};
use api::{Api, Identity};
use routes::{Incoming, Route, Routes, SessionKey, Topic};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Config {
    enabled: bool,
    app_id: String,
    app_secret: String,
    chat_id: String,
    allowed_users: Vec<String>,
    allow_all_users: bool,
    send_title: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdate {
    enabled: bool,
    app_id: String,
    app_secret: Option<String>,
    chat_id: String,
    #[serde(default)]
    allowed_users: Vec<String>,
    #[serde(default)]
    allow_all_users: bool,
    send_title: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    enabled: bool,
    connected: bool,
    connection_error: Option<String>,
    delivery_error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    status: Status,
    app_id: String,
    has_secret: bool,
    chat_id: String,
    allowed_users: Vec<String>,
    allow_all_users: bool,
    send_title: bool,
}

struct Runtime {
    api: Arc<Api>,
    output: mpsc::Sender<Completion>,
    socket: tauri::async_runtime::JoinHandle<()>,
    listener: tauri::async_runtime::JoinHandle<()>,
    delivery: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    config: Config,
    status: Status,
    revision: u64,
    runtime: Option<Runtime>,
}

#[derive(Default)]
struct Sessions {
    opened: HashSet<SessionKey>,
    routes: Routes,
}

#[derive(Clone, Default)]
pub struct State {
    inner: Arc<Mutex<Inner>>,
    sessions: Arc<Mutex<Sessions>>,
    operation: Arc<tokio::sync::Mutex<()>>,
    pending: Arc<Mutex<HashMap<Uuid, FollowUp>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReport {
    id: u32,
    generation: u32,
    title: String,
    agent: String,
    event: AgentEvent,
}

struct FollowUp {
    key: SessionKey,
    revision: u64,
    input_revision: u64,
    text: String,
    result: oneshot::Sender<Result<(), String>>,
}

struct Completion {
    key: SessionKey,
    title: String,
    agent: String,
    at: SystemTime,
    uuid: String,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_local_data_dir().map_err(|_| "无法定位飞书设置目录")?
        .join("notifications").join("feishu.credential"))
}

fn read_config(path: &Path) -> Result<Config, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(_) => return Err("读取飞书配置失败".into()),
    };
    let decoded = crate::secure::protect(&bytes, true)?;
    serde_json::from_slice(&decoded).map_err(|_| "保存的飞书配置格式无效".into())
}

fn save_config(path: &Path, config: &Config) -> Result<(), String> {
    let bytes = serde_json::to_vec(config).map_err(|_| "飞书配置格式无效")?;
    let encrypted = crate::secure::protect(&bytes, false)?;
    let _lock = crate::storage::lock(path)?;
    crate::storage::write(path, &encrypted, false)
}

fn valid_id(id: &str, prefix: &str) -> bool {
    id.len() <= 128 && id.strip_prefix(prefix).is_some_and(|suffix| !suffix.is_empty()
        && suffix.bytes().all(|value| value.is_ascii_alphanumeric() || value == b'_' || value == b'-'))
}

impl Config {
    fn allows_sender(&self, open_id: Option<&str>) -> bool {
        if self.allowed_users.is_empty() {
            return self.allow_all_users;
        }
        self.allowed_users.iter().any(|user| Some(user.as_str()) == open_id)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.enabled && self.app_id.is_empty() && self.app_secret.is_empty() && self.chat_id.is_empty() && self.allowed_users.is_empty() {
            return Ok(());
        }
        if !valid_id(&self.app_id, "cli_") { return Err("请输入飞书应用的 App ID".into()); }
        if !(12..=256).contains(&self.app_secret.len()) || self.app_secret.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("请输入完整的飞书 App Secret".into());
        }
        if !valid_id(&self.chat_id, "oc_") { return Err("请输入目标群的 Chat ID（oc_…）".into()); }
        if self.allowed_users.len() > 64 || self.allowed_users.iter().any(|user| !valid_id(user, "ou_")) {
            return Err("授权用户请填写飞书 Open ID（ou_…），最多 64 个".into());
        }
        Ok(())
    }
}

impl State {
    pub fn initialize(&self, app: &AppHandle) {
        match config_path(app).and_then(|path| read_config(&path)) {
            Ok(config) => self.apply(app, config),
            Err(error) => {
                self.inner.lock().unwrap().status.connection_error = Some(error);
                self.publish(app);
            }
        }
    }

    async fn save(&self, app: &AppHandle, config: Config) -> Result<(), String> {
        let path = config_path(app)?;
        let saved = config.clone();
        tauri::async_runtime::spawn_blocking(move || save_config(&path, &saved)).await.map_err(|_| "保存飞书配置失败")??;
        self.apply(app, config);
        Ok(())
    }

    fn apply(&self, app: &AppHandle, config: Config) {
        let (revision, previous) = {
            let mut inner = self.inner.lock().unwrap();
            inner.revision += 1;
            let previous = inner.runtime.take();
            if inner.config.app_id != config.app_id || inner.config.chat_id != config.chat_id {
                self.sessions.lock().unwrap().routes = Routes::default();
            }
            inner.status = Status { enabled: config.enabled, ..Status::default() };
            inner.config = config.clone();
            (inner.revision, previous)
        };
        if let Some(runtime) = previous {
            runtime.socket.abort();
            runtime.listener.abort();
            runtime.delivery.abort();
        }
        self.pending.lock().unwrap().clear();
        self.publish(app);
        if !config.enabled { return; }
        if let Err(error) = config.validate() {
            self.set_connection(app, revision, false, Some(format!("待配置：{error}")));
            return;
        }
        let api = match Api::new(config.clone()) {
            Ok(api) => Arc::new(api),
            Err(error) => { self.set_connection(app, revision, false, Some(error)); return; }
        };

        let (events, incoming) = mpsc::channel(64);
        let (output, queue) = mpsc::channel(128);
        let socket = tauri::async_runtime::spawn({
            let (state, app, api) = (self.clone(), app.clone(), api.clone());
            async move { socket::run(&api, &events, |connected, error| state.set_connection(&app, revision, connected, error)).await; }
        });
        let listener = tauri::async_runtime::spawn({
            let (state, app, api) = (self.clone(), app.clone(), api.clone());
            async move { state.listen(&app, revision, &config, &api, incoming).await; }
        });
        let delivery = tauri::async_runtime::spawn({
            let (state, app, api) = (self.clone(), app.clone(), api.clone());
            async move { state.deliver(&app, revision, &api, queue).await; }
        });
        self.inner.lock().unwrap().runtime = Some(Runtime { api, output, socket, listener, delivery });
    }

    pub fn stop(&self) {
        let previous = {
            let mut inner = self.inner.lock().unwrap();
            inner.revision += 1;
            inner.runtime.take()
        };
        if let Some(runtime) = previous {
            runtime.socket.abort();
            runtime.listener.abort();
            runtime.delivery.abort();
        }
        self.pending.lock().unwrap().clear();
    }

    fn publish(&self, app: &AppHandle) {
        let status = self.inner.lock().unwrap().status.clone();
        let _ = app.emit_to("main", "feishu-status", status);
    }

    fn set_connection(&self, app: &AppHandle, revision: u64, connected: bool, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.revision != revision { return; }
        inner.status.connected = connected;
        inner.status.connection_error = error;
        drop(inner);
        self.publish(app);
    }

    fn set_delivery(&self, app: &AppHandle, revision: u64, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.revision != revision { return; }
        inner.status.delivery_error = error;
        drop(inner);
        self.publish(app);
    }

    pub fn opened(&self, id: u32, generation: u32) {
        self.sessions.lock().unwrap().opened.insert(SessionKey { id, generation });
    }

    pub fn replaced(&self, id: u32, generation: u32) {
        let mut sessions = self.sessions.lock().unwrap();
        let previous: Vec<_> = sessions.opened.iter().copied().filter(|key| key.id == id && key.generation != generation).collect();
        for key in previous {
            sessions.opened.remove(&key);
            sessions.routes.retire(key);
        }
    }

    pub fn closed(&self, id: u32, generation: u32) {
        let key = SessionKey { id, generation };
        let mut sessions = self.sessions.lock().unwrap();
        sessions.opened.remove(&key);
        sessions.routes.retire(key);
    }

    fn push(&self, app: &AppHandle, completion: Completion) {
        let current = self.inner.lock().unwrap();
        if let Some(runtime) = current.runtime.as_ref() {
            if runtime.output.try_send(completion).is_err() {
                let revision = current.revision;
                drop(current);
                self.set_delivery(app, revision, Some("完成通知队列已满".into()));
            }
        }
    }

    async fn deliver(&self, app: &AppHandle, revision: u64, api: &Api, mut queue: mpsc::Receiver<Completion>) {
        while let Some(completion) = queue.recv().await {
            let (enabled, send_title) = {
                let inner = self.inner.lock().unwrap();
                (inner.revision == revision && inner.config.enabled, inner.config.send_title)
            };
            if !enabled { return; }
            let (open, topic) = {
                let sessions = self.sessions.lock().unwrap();
                (sessions.opened.contains(&completion.key), sessions.routes.topic(completion.key))
            };
            if !open { continue; }
            let link = app.state::<crate::remote::RemoteState>().current().and_then(|hub| {
                hub.shared_link(completion.key.id, completion.key.generation)
            });
            let title = if send_title && !completion.title.is_empty() { &completion.title } else { "终端会话" };
            let card = completed_card(title, &completion.agent, completion.key.id, completion.at, link);
            let mut attempt = 0;
            let sent = loop {
                attempt += 1;
                let result = api.send(topic.as_ref().map(|topic| topic.root.as_str()), "interactive", &card, &completion.uuid).await;
                match result {
                    Err(error) if error.retryable() && attempt < 3 => tokio::time::sleep(Duration::from_secs(attempt)).await,
                    result => break result,
                }
            };
            match sent {
                Ok(sent) => {
                    let inner = self.inner.lock().unwrap();
                    if inner.revision != revision { return; }
                    let mut sessions = self.sessions.lock().unwrap();
                    let mut topic = topic.unwrap_or(Topic { root: sent.message_id, thread: None });
                    if sent.thread_id.as_deref().is_some_and(|thread| valid_id(thread, "omt_")) { topic.thread = sent.thread_id; }
                    sessions.routes.bind(completion.key, topic);
                    if !sessions.opened.contains(&completion.key) { sessions.routes.retire(completion.key); }
                    drop(sessions);
                    drop(inner);
                    self.set_delivery(app, revision, None);
                }
                Err(error) => self.set_delivery(app, revision, Some(error.to_string())),
            }
        }
    }

    async fn listen(&self, app: &AppHandle, revision: u64, config: &Config, api: &Api, mut events: mpsc::Receiver<Vec<u8>>) {
        let mut identity: Option<Identity> = None;
        while let Some(payload) = events.recv().await {
            if !config.allow_all_users && config.allowed_users.is_empty() { continue; }
            if identity.is_none() {
                match api.identity().await {
                    Ok(bot) => identity = Some(bot),
                    Err(error) => { self.set_delivery(app, revision, Some(error.to_string())); continue; }
                }
            }
            let Some(input) = Incoming::parse(&payload, config, &identity.as_ref().unwrap().open_id) else { continue };
            let route = {
                let mut sessions = self.sessions.lock().unwrap();
                if !sessions.routes.accept(&input.id) { continue; }
                sessions.routes.target(&input)
            };
            let result = match (route, input.text) {
                (Route::Expired, _) => Err("原会话已失效，请使用新会话的通知".into()),
                (Route::Unbound, _) => Err("请打开当前 sTerm 发出的完成通知，从卡片进入回复话题后 @ 机器人".into()),
                (Route::Session(_), None) => Err("请发送普通文本。斜杠命令和其他操作可在远程终端使用".into()),
                (Route::Session(key), Some(text)) if text == "打开会话" => {
                    app.state::<crate::remote::RemoteState>().current()
                        .and_then(|hub| hub.shared_link(key.id, key.generation))
                        .map(Some).ok_or_else(|| "请在电脑端开启原会话分享并连接公网 HTTPS，再获取链接".to_owned())
                }
                (Route::Session(key), Some(text)) => self.submit(app, revision, key, text).await.map(|_| None),
            };
            let (kind, content) = match result {
                Ok(Some(url)) => ("interactive", json!({
                    "header":{"title":{"tag":"plain_text","content":"sTerm · 远程会话"}},
                    "elements":remote_link_elements(url),
                })),
                Ok(None) => ("text", json!({"text":"已提交到原会话"})),
                Err(error) => ("text", json!({"text":error})),
            };
            match api.send(Some(&input.id), kind, &content, &Uuid::new_v4().to_string()).await {
                Err(error) => self.set_delivery(app, revision, Some(error.to_string())),
                Ok(_) => self.set_delivery(app, revision, None),
            }
        }
    }

    async fn submit(&self, app: &AppHandle, revision: u64, key: SessionKey, text: String) -> Result<(), String> {
        let request = Uuid::new_v4();
        let input_revision = app.state::<AppState>().ptys.input_revision(key.id, key.generation)
            .map_err(|_| "原会话已失效")?;
        let (result, response) = oneshot::channel();
        {
            let inner = self.inner.lock().unwrap();
            if inner.revision != revision || !inner.config.enabled { return Err("飞书续问已关闭".into()); }
            self.pending.lock().unwrap().insert(request, FollowUp { key, revision, input_revision, text, result });
        }
        if app.emit_to("main", "feishu-input", json!({
            "request": request.to_string(), "id": key.id, "generation": key.generation
        })).is_err() {
            self.pending.lock().unwrap().remove(&request);
            return Err("无法联系终端窗口，请打开远程终端".into());
        }
        match tokio::time::timeout(Duration::from_secs(10), response).await {
            Ok(Ok(result)) => result,
            _ => {
                self.pending.lock().unwrap().remove(&request);
                Err("终端未确认写入，请先打开原会话查看".into())
            }
        }
    }
}

fn completed_card(title: &str, agent: &str, id: u32, at: SystemTime, link: Option<String>) -> Value {
    let agent = match agent { "claude" => "Claude Code", "codex" => "Codex", "pi" => "Pi", _ => "Agent" };
    let note = format!("{} 已完成 · 会话 #{}\n{}", agent, id, completion_time(at));
    let mut elements = vec![json!({"tag":"div", "text":{"tag":"plain_text", "content":note}})];
    if let Some(url) = link {
        elements.extend(remote_link_elements(url));
    }
    json!({"config":{"wide_screen_mode":true},"header":{"template":"green","title":{"tag":"plain_text","content":title}},
        "elements":elements})
}

fn remote_link_elements(url: String) -> [Value; 2] {
    let hint = if url.contains("#pair=") {
        "临时配对链接 5 分钟内有效且只能使用一次。失效后请在本话题中 @ 机器人发送「打开会话」获取新链接。"
    } else {
        "使用当前浏览器授权或 6 位访问码登录。需要新链接时，请在本话题中 @ 机器人发送「打开会话」。"
    };
    [
        json!({"tag":"action","actions":[{"tag":"button","type":"primary",
            "text":{"tag":"plain_text","content":"打开原会话"},"url":url}]}),
        json!({"tag":"note","elements":[{"tag":"plain_text","content":hint}]}),
    ]
}

fn completion_time(at: SystemTime) -> String {
    let offset = time::UtcOffset::from_hms(8, 0, 0).expect("有效的东八区偏移");
    let date_time = time::OffsetDateTime::from(at).to_offset(offset);
    format!("{} {:02}:{:02} UTC+08:00", date_time.date(), date_time.hour(), date_time.minute())
}

fn desktop(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { Ok(()) } else { Err("请在 sTerm 主窗口操作".into()) }
}

#[tauri::command]
pub fn feishu_settings(window: WebviewWindow, state: tauri::State<State>) -> Result<Settings, String> {
    desktop(&window)?;
    let inner = state.inner.lock().unwrap();
    Ok(Settings {
        status: inner.status.clone(), app_id: inner.config.app_id.clone(), has_secret: !inner.config.app_secret.is_empty(),
        chat_id: inner.config.chat_id.clone(), allowed_users: inner.config.allowed_users.clone(),
        allow_all_users: inner.config.allow_all_users, send_title: inner.config.send_title,
    })
}

#[tauri::command]
pub async fn feishu_save(app: AppHandle, window: WebviewWindow, update: ConfigUpdate) -> Result<(), String> {
    desktop(&window)?;
    let state = app.state::<State>();
    let _operation = state.operation.lock().await;
    let mut config = {
        let inner = state.inner.lock().unwrap();
        Config {
            enabled: update.enabled, app_id: update.app_id.trim().to_owned(),
            app_secret: update.app_secret.unwrap_or_else(|| inner.config.app_secret.clone()).trim().to_owned(),
            chat_id: update.chat_id.trim().to_owned(),
            allow_all_users: update.allow_all_users && update.allowed_users.is_empty(),
            allowed_users: update.allowed_users, send_title: update.send_title,
        }
    };
    config.allowed_users.sort();
    config.allowed_users.dedup();
    config.validate()?;
    state.save(&app, config).await
}

#[tauri::command]
pub async fn feishu_set_enabled(app: AppHandle, window: WebviewWindow, enabled: bool) -> Result<(), String> {
    desktop(&window)?;
    let state = app.state::<State>();
    let _operation = state.operation.lock().await;
    let mut config = state.inner.lock().unwrap().config.clone();
    config.enabled = enabled;
    state.save(&app, config).await
}

#[tauri::command]
pub async fn feishu_test(window: WebviewWindow, state: tauri::State<'_, State>) -> Result<(), String> {
    desktop(&window)?;
    let api = {
        let inner = state.inner.lock().unwrap();
        if !inner.config.enabled { return Err("请先启用飞书通知".into()); }
        inner.config.validate()?;
        inner.runtime.as_ref().map(|runtime| runtime.api.clone()).ok_or("飞书连接尚未就绪")?
    };
    api.send(None, "text", &json!({"text":"sTerm 飞书通知测试"}), &Uuid::new_v4().to_string())
        .await.map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn feishu_agent_event(app: AppHandle, window: WebviewWindow, report: AgentReport) -> Result<(), String> {
    desktop(&window)?;
    let app_state = app.state::<AppState>();
    let ptys = &app_state.ptys;
    let completed = ptys.report_agent_event(report.id, report.generation, &report.agent, report.event);
    if completed {
        app.state::<State>().push(&app, Completion {
            key: SessionKey { id: report.id, generation: report.generation },
            title: report.title.chars().filter(|c| !c.is_control()).take(100).collect(),
            agent: report.agent, at: SystemTime::now(),
            uuid: Uuid::new_v4().to_string(),
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn feishu_input_result(app: AppHandle, window: WebviewWindow, request: String, ready: bool, paste: bool) -> Result<(), String> {
    desktop(&window)?;
    let request = Uuid::parse_str(&request).map_err(|_| "无效的飞书输入请求")?;
    let state = app.state::<State>();
    let Some(FollowUp { key, revision, input_revision, text, result: response }) = state.pending.lock().unwrap().remove(&request) else { return Ok(()) };
    let result = if ready {
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<State>();
            {
                let current = state.inner.lock().unwrap();
                if current.revision != revision || !current.config.enabled || response.is_closed() {
                    return;
                }
            }
            let app_state = app.state::<AppState>();
            let write = || app_state.ptys.submit_agent_text(key.id, key.generation, input_revision, &text, paste);
            let result = match app.state::<crate::remote::RemoteState>().current() {
                Some(hub) => hub.with_unclaimed_input(key.id, key.generation, write),
                None => write(),
            };
            let _ = response.send(result);
        }).await.map_err(|_| "飞书续问处理失败")
    } else {
        let _ = response.send(Err("原会话已失效，请打开终端确认".into()));
        Ok(())
    };
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_credentials_encrypted_and_does_not_export_the_secret() {
        let dir = crate::storage::tests::TempDir::new();
        let path = dir.path().join("feishu.credential");
        let config = Config {
            app_id: "cli_example".into(), app_secret: "secret-for-test".into(), chat_id: "oc_example".into(),
            allow_all_users: true, ..Config::default()
        };
        config.validate().unwrap();
        save_config(&path, &config).unwrap();
        assert!(!std::fs::read(&path).unwrap().windows(config.app_secret.len()).any(|part| part == config.app_secret.as_bytes()));
        let saved = read_config(&path).unwrap();
        assert_eq!(saved.app_secret, config.app_secret);
        assert!(saved.allow_all_users);
        assert!(!valid_id("oc_/outside", "oc_"));
    }

    #[test]
    fn card_contains_only_session_metadata_and_the_optional_target() {
        let card = completed_card("后端", "codex", 2, SystemTime::UNIX_EPOCH, Some("https://example.com/#session=target".into()));
        assert_eq!(card["header"]["title"]["content"], "后端");
        assert_eq!(card["elements"][0]["text"]["content"], "Codex 已完成 · 会话 #2\n1970-01-01 08:00 UTC+08:00");
        assert_eq!(card["elements"][1]["actions"][0]["url"], "https://example.com/#session=target");
        let next_day = completed_card("前端", "pi", 1, SystemTime::UNIX_EPOCH + Duration::from_secs(20 * 60 * 60), None);
        assert_eq!(next_day["header"]["title"]["content"], "前端");
        assert_eq!(next_day["elements"][0]["text"]["content"], "Pi 已完成 · 会话 #1\n1970-01-02 04:00 UTC+08:00");
        assert_eq!(next_day["elements"].as_array().unwrap().len(), 1);
    }
}
