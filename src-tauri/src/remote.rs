use axum::{
    body::Body,
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, ConnectInfo, DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{broadcast, Notify};
use crate::remote_password::{self, LoginLimiter, PasswordVerifier};

const PAIR_LIFETIME: Duration = Duration::from_secs(300);
const CLIENT_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const PASSWORD_CLIENT_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_CLIENTS: usize = 8;
const MAX_PAIR_TICKETS: usize = 128;
const MAX_SNAPSHOT: usize = 2 * 1024 * 1024;

pub trait RemoteHost: Send + Sync {
    fn snapshot(&self, id: u32, generation: u32, request: &str) -> Result<(), String>;
    fn write(&self, id: u32, generation: u32, data: &str) -> Result<(), String>;
    fn control_changed(&self, session: &SharedSession);
    fn resize(&self, id: u32, generation: u32, cols: u16, rows: u16) -> Result<(), String>;
    fn asset(&self, path: &str) -> Option<(Vec<u8>, String)>;
    fn agent_hub(&self) -> Option<crate::agent_runtime::hub::SharedHub> { None }
    fn end_agent(&self,_runtime:&str)->Result<(),String>{Err("当前主机不支持结束 Agent".into())}
}

struct DesktopHost(AppHandle);

impl RemoteHost for DesktopHost {
    fn end_agent(&self,runtime:&str)->Result<(),String>{self.0.state::<crate::agent_runtime::State>().shell.end(runtime)}
    fn agent_hub(&self) -> Option<crate::agent_runtime::hub::SharedHub> { Some(crate::agent_runtime::web::hub(&self.0)) }
    fn snapshot(&self, id: u32, generation: u32, request: &str) -> Result<(), String> {
        self.0.emit("remote-snapshot-request", json!({"id": id, "generation": generation, "request": request}))
            .map_err(|error| error.to_string())
    }

    fn write(&self, id: u32, generation: u32, data: &str) -> Result<(), String> {
        self.0.state::<crate::AppState>().ptys.write(id, generation, data)
    }

    fn control_changed(&self, session: &SharedSession) {
        let _ = self.0.emit("remote-control", session);
    }

    fn resize(&self, id: u32, generation: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.0.emit("remote-resize", json!({"id": id, "generation": generation, "cols": cols, "rows": rows}))
            .map_err(|error| error.to_string())
    }

    fn asset(&self, path: &str) -> Option<(Vec<u8>, String)> {
        self.0.asset_resolver().get(path.to_owned()).map(|asset| (asset.bytes, asset.mime_type))
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SharedSession {
    pub id: u32,
    pub generation: u32,
    #[serde(default)]
    pub link_key: Option<String>,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub allow_control: bool,
    #[serde(default)]
    pub controller: Option<String>,
    #[serde(default)]
    pub epoch: Option<String>,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
}

struct PairTicket {
    origin: String,
    expires: Instant,
    session: Option<String>,
}

struct Client {
    id: String,
    expires: Instant,
}

struct Inner {
    port: u16,
    origins: Vec<String>,
    pairs: HashMap<String, PairTicket>,
    clients: HashMap<String, Client>,
    connected: HashSet<String>,
    sessions: HashMap<u32, SharedSession>,
    attempts: HashMap<IpAddr, (Instant, u32)>,
    password: Option<PasswordVerifier>,
    auth_generation: u64,
    login_limiter: LoginLimiter,
}

impl Inner {
    fn issue_pair(&mut self, origin: &str, session: Option<String>) -> Result<String, String> {
        if !self.origins.iter().any(|allowed| allowed == origin) { return Err("请选择已连接的访问地址".into()); }
        self.pairs.retain(|_, ticket| ticket.expires > Instant::now());
        if self.pairs.len() >= MAX_PAIR_TICKETS { return Err("临时配对链接数量已达上限，请稍后重试".into()); }
        let token = secret();
        self.pairs.insert(token.clone(), PairTicket { origin: origin.into(), expires: Instant::now() + PAIR_LIFETIME, session });
        Ok(format!("{origin}/#pair={token}"))
    }
}

pub struct RemoteHub {
    inner: Mutex<Inner>,
    input: Mutex<()>,
    events: broadcast::Sender<Value>,
    host: Arc<dyn RemoteHost>,
    stopped: AtomicBool,
    shutdown: Notify,
    password_checks: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub port: u16,
    pub origins: Vec<String>,
    pub sessions: Vec<SharedSession>,
    pub clients: usize,
    pub password_enabled: bool,
}

impl RemoteHub {
    fn new(port: u16, origins: Vec<String>, host: Arc<dyn RemoteHost>) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Mutex::new(Inner {
                port, origins, pairs: HashMap::new(), clients: HashMap::new(),
                connected: HashSet::new(), sessions: HashMap::new(), attempts: HashMap::new(),
                password: None, auth_generation: 0, login_limiter: LoginLimiter::default(),
            }),
            input: Mutex::new(()), events, host, stopped: AtomicBool::new(false), shutdown: Notify::new(),
            password_checks: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    fn status(&self) -> RemoteStatus {
        let inner = self.inner.lock().unwrap();
        let mut sessions: Vec<_> = inner.sessions.values().cloned().collect();
        sessions.sort_by_key(|session| session.id);
        RemoteStatus {
            enabled: !self.stopped.load(Ordering::Relaxed), port: inner.port,
            origins: inner.origins.clone(), sessions, clients: inner.connected.len(), password_enabled: inner.password.is_some(),
        }
    }

    pub(crate) fn agent_authenticated(&self, headers: &HeaderMap) -> bool {
        !self.stopped.load(Ordering::Relaxed) && self.authenticate(headers).is_some()
    }

    pub(crate) fn submit_agent(&self, headers: &HeaderMap, command: crate::agent_runtime::protocol::Command) -> Result<crate::agent_runtime::protocol::Receipt, String> {
        let _input = self.input.lock().unwrap();
        if !self.agent_authenticated(headers) { return Err("浏览器授权已撤销".into()); }
        self.agent_hub().ok_or("Agent 服务不可用")?.submit(command, true)
    }

    pub(crate) fn end_agent(&self,headers:&HeaderMap,runtime:&str,generation:u32,session_id:&str)->Result<(),String>{
        let _input=self.input.lock().unwrap();
        if !self.agent_authenticated(headers){return Err("浏览器授权已撤销".into())}
        let agent=self.agent_hub().ok_or("Agent 服务不可用")?;
        let row=agent.snapshot(runtime,true)?;
        if row.generation!=generation||row.session_id!=session_id||!row.allow_control||!row.capabilities.end||row.state=="exited"{return Err("会话不可结束或已替换".into())}
        self.host.end_agent(runtime)
    }
    pub(crate) fn agent_hub(&self) -> Option<crate::agent_runtime::hub::SharedHub> {
        self.host.agent_hub()
    }

    fn roster(&self) {
        let _ = self.events.send(json!({"type": "sessions", "sessions": self.status().sessions}));
    }

    pub fn share(&self, mut session: SharedSession) -> Result<(), String> {
        if self.host.agent_hub().is_some_and(|hub| hub.has_tab(session.id, session.generation)) {
            return Err("此终端已接入 Agent 模式，请在远程访问设置中分享".into());
        }
        if session.cols < 2 || session.rows == 0 || session.title.len() > 512 {
            return Err("无效的会话信息".into());
        }
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        session.controller = None;
        session.epoch = None;
        session.link_key = Some(uuid::Uuid::new_v4().to_string());
        if let Some(previous) = inner.sessions.get(&session.id).filter(|old| old.generation == session.generation) {
            session.seq = previous.seq;
            session.link_key.clone_from(&previous.link_key);
            if session.allow_control {
                session.controller = previous.controller.clone();
                session.epoch = previous.epoch.clone();
            }
        }
        self.host.control_changed(&session);
        inner.sessions.insert(session.id, session);
        drop(inner);
        self.roster();
        Ok(())
    }

    pub fn unshare(&self, id: u32, generation: u32) {
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        if inner.sessions.get(&id).is_some_and(|session| session.generation == generation) {
            if let Some(mut session) = inner.sessions.remove(&id) {
                session.controller = None;
                session.epoch = None;
                self.host.control_changed(&session);
            }
        }
        drop(inner);
        self.roster();
    }

    pub fn publish(&self, id: u32, generation: u32, seq: u64, data: String, cols: u16, rows: u16) {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.get_mut(&id).filter(|session| session.generation == generation) else { return };
        if seq <= session.seq || data.len() > 128 * 1024 { return; }
        session.seq = seq;
        session.cols = cols;
        session.rows = rows;
        let _ = self.events.send(json!({"type": "output", "id": id, "generation": generation, "seq": seq, "data": data, "cols": cols, "rows": rows}));
    }

    pub fn snapshot(&self, request: String, id: u32, generation: u32, seq: u64, data: String, cols: u16, rows: u16) -> Result<(), String> {
        if data.len() > MAX_SNAPSHOT { return Err("终端快照过大，请缩小回滚范围".into()); }
        let inner = self.inner.lock().unwrap();
        let session = inner.sessions.get(&id).filter(|session| session.generation == generation).ok_or("会话已取消分享")?;
        if seq != session.seq { return Err("快照序号与输出不一致，请重新连接".into()); }
        let _ = self.events.send(json!({"type": "snapshot", "request": request, "id": id, "generation": generation, "seq": seq, "data": data, "cols": cols, "rows": rows}));
        Ok(())
    }

    fn issue_pair(&self, origin: &str) -> Result<Value, String> {
        let mut inner = self.inner.lock().unwrap();
        Ok(json!({"url": inner.issue_pair(origin, None)?, "expiresIn": PAIR_LIFETIME.as_secs()}))
    }

    fn pair(&self, token: &str, origin: &str, ip: IpAddr) -> Result<String, StatusCode> {
        let mut inner = self.inner.lock().unwrap();
        inner.attempts.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(60));
        if inner.attempts.len() >= 1024 && !inner.attempts.contains_key(&ip) { return Err(StatusCode::TOO_MANY_REQUESTS); }
        let (_, count) = inner.attempts.entry(ip).or_insert((Instant::now(), 0));
        *count += 1;
        if *count > 20 { return Err(StatusCode::TOO_MANY_REQUESTS); }
        inner.clients.retain(|_, client| client.expires > Instant::now());
        if inner.clients.len() >= MAX_CLIENTS { return Err(StatusCode::TOO_MANY_REQUESTS); }
        let ticket = inner.pairs.get(token).ok_or(StatusCode::UNAUTHORIZED)?;
        if ticket.expires <= Instant::now() || ticket.origin != origin { return Err(StatusCode::UNAUTHORIZED); }
        if let Some(key) = ticket.session.as_ref() {
            if inner.password.is_some() || !inner.sessions.values().any(|session| session.link_key.as_ref() == Some(key)) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        inner.pairs.remove(token);
        let cookie = secret();
        inner.clients.insert(cookie.clone(), Client { id: uuid::Uuid::new_v4().to_string(), expires: Instant::now() + CLIENT_LIFETIME });
        Ok(cookie)
    }

    async fn login_password(&self, password: String, origin: &str, ip: IpAddr) -> Result<String, LoginError> {
        if !origin.starts_with("https://") {
            return Err(LoginError::new(StatusCode::FORBIDDEN, "访问码登录仅支持 HTTPS 地址，请使用公网地址或扫码配对"));
        }
        let permit = self.password_checks.clone().try_acquire_owned().map_err(|_| LoginError::limited(2))?;
        let (verifier, generation) = {
            let mut inner = self.inner.lock().unwrap();
            if self.stopped.load(Ordering::Relaxed) || !inner.origins.iter().any(|allowed| allowed == origin) {
                return Err(LoginError::new(StatusCode::UNAUTHORIZED, "无法登录，请重试或在电脑端重新设置访问码"));
            }
            inner.login_limiter.reserve(ip, Instant::now()).map_err(LoginError::limited)?;
            (inner.password.clone(), inner.auth_generation)
        };
        let Some(verifier) = verifier else {
            return Err(LoginError::new(StatusCode::UNAUTHORIZED, "无法登录，请重试或在电脑端重新设置访问码"));
        };
        let matches = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            verifier.verify(&password)
        }).await.map_err(|_| LoginError::new(StatusCode::SERVICE_UNAVAILABLE, "登录暂时不可用，请稍后重试"))?;
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        if !matches || self.stopped.load(Ordering::Relaxed) || inner.auth_generation != generation
            || !inner.origins.iter().any(|allowed| allowed == origin) {
            return Err(LoginError::new(StatusCode::UNAUTHORIZED, "无法登录，请重试或在电脑端重新设置访问码"));
        }
        inner.clients.retain(|_, client| client.expires > Instant::now());
        if inner.clients.len() >= MAX_CLIENTS {
            return Err(LoginError::new(StatusCode::TOO_MANY_REQUESTS, "已授权设备达到上限，请在电脑端撤销设备后重试"));
        }
        let cookie = secret();
        inner.clients.insert(cookie.clone(), Client { id: uuid::Uuid::new_v4().to_string(), expires: Instant::now() + PASSWORD_CLIENT_LIFETIME });
        Ok(cookie)
    }

    fn authenticate(&self, headers: &HeaderMap) -> Option<String> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?.split(';')
            .filter_map(|part| part.trim().split_once('='))
            .find_map(|(name, value)| (name == "sterm_remote").then_some(value))?;
        self.inner.lock().unwrap().clients.get(cookie)
            .filter(|client| client.expires > Instant::now()).map(|client| client.id.clone())
    }

    fn claim(&self, client: &str, id: u32, generation: u32) -> Result<(), String> {
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        if !inner.clients.values().any(|entry| entry.id == client && entry.expires > Instant::now()) {
            return Err("设备授权已失效，请重新登录或扫码配对".into());
        }
        let session = inner.sessions.get_mut(&id).filter(|session| session.generation == generation).ok_or("会话已结束或取消分享")?;
        if !session.allow_control { return Err("电脑端尚未允许此会话接受远程输入".into()); }
        if session.controller.as_deref().is_some_and(|owner| owner != client) { return Err("其他设备正在控制此会话".into()); }
        session.controller = Some(client.into());
        session.epoch = Some(secret());
        self.host.control_changed(session);
        drop(inner);
        self.roster();
        Ok(())
    }

    fn remote_write(&self, client: &str, id: u32, generation: u32, epoch: &str, data: &str) -> Result<(), String> {
        if data.len() > 8192 { return Err("每次输入不能超过 8 KB".into()); }
        let _input = self.input.lock().unwrap();
        let inner = self.inner.lock().unwrap();
        if !inner.clients.values().any(|entry| entry.id == client && entry.expires > Instant::now()) {
            return Err("设备授权已失效，请重新登录或扫码配对".into());
        }
        let session = inner.sessions.get(&id).filter(|session| session.generation == generation).ok_or("会话已结束")?;
        if !session.allow_control || session.controller.as_deref() != Some(client) || session.epoch.as_deref() != Some(epoch) {
            return Err("控制权已失效，请重新接管".into());
        }
        drop(inner);
        self.host.write(id, generation, data)
    }

    pub(crate) fn with_unclaimed_input(&self, id: u32, generation: u32, write: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let _input = self.input.lock().unwrap();
        if !self.local_write_allowed(id, generation) {
            return Err("手机正在控制此会话，请先在远程面板收回控制权".into());
        }
        write()
    }

    pub(crate) fn shared_link(&self, id: u32, generation: u32) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        if self.stopped.load(Ordering::Relaxed) { return None; }
        let key = inner.sessions.get(&id).filter(|session| session.generation == generation)?.link_key.clone()?;
        let origin = inner.origins.iter().find(|origin| origin.starts_with("https://"))?.clone();
        if inner.password.is_some() { return Some(format!("{origin}/#session={key}")); }
        let url = inner.issue_pair(&origin, Some(key.clone())).ok()?;
        Some(format!("{url}&session={key}"))
    }

    pub fn local_write_allowed(&self, id: u32, generation: u32) -> bool {
        !self.inner.lock().unwrap().sessions.get(&id)
            .is_some_and(|session| session.generation == generation && session.controller.is_some())
    }

    pub fn reclaim(&self, id: u32, generation: Option<u32>) {
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        if let Some(session) = inner.sessions.get_mut(&id).filter(|session| generation.is_none_or(|value| session.generation == value)) {
            session.controller = None;
            session.epoch = None;
            self.host.control_changed(session);
        }
        drop(inner);
        self.roster();
    }

    fn disconnect(&self, client: &str) {
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        inner.connected.remove(client);
        for session in inner.sessions.values_mut().filter(|session| session.controller.as_deref() == Some(client)) {
            session.controller = None;
            session.epoch = None;
            self.host.control_changed(session);
        }
        drop(inner);
        self.roster();
    }

    fn revoke_clients(&self) {
        self.revoke_with_password(None);
    }

    fn set_password(&self, password: Option<PasswordVerifier>) {
        self.revoke_with_password(Some(password));
    }

    fn revoke_with_password(&self, password: Option<Option<PasswordVerifier>>) {
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        inner.auth_generation += 1;
        if let Some(password) = password {
            inner.password = password;
            inner.login_limiter = LoginLimiter::default();
        }
        if let Some(agent) = self.agent_hub() { agent.revoke_operations(); }
        inner.clients.clear();
        inner.pairs.clear();
        inner.connected.clear();
        for session in inner.sessions.values_mut() {
            session.controller = None;
            session.epoch = None;
            self.host.control_changed(session);
        }
        drop(inner);
        let _ = self.events.send(json!({"type": "revoked"}));
        self.roster();
    }

    pub fn set_public_origin(&self, origin: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.origins.iter().find(|origin| origin.starts_with("https://")) == origin.as_ref() { return; }
        inner.origins.retain(|origin| origin.starts_with("http://"));
        if let Some(origin) = origin { inner.origins.push(origin); }
        inner.pairs.retain(|_, ticket| ticket.origin.starts_with("http://"));
    }

    fn stop(&self) {
        {
            let _input = self.input.lock().unwrap();
            self.stopped.store(true, Ordering::Relaxed);
            if let Some(agent) = self.agent_hub() { agent.revoke_operations(); }
        }
        for session in self.status().sessions { self.reclaim(session.id, None); }
        self.inner.lock().unwrap().clients.clear();
        let _ = self.events.send(json!({"type": "stopped"}));
        self.shutdown.notify_one();
    }
}

fn secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn origin_authority(origin: &str) -> Option<String> {
    let url = reqwest::Url::parse(origin).ok()?;
    let host = url.host_str()?;
    Some(match url.port() { Some(port) => format!("{host}:{port}"), None => host.into() })
}

fn request_origin(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::ORIGIN)?.to_str().ok()
}

async fn guard(State(hub): State<Arc<RemoteHub>>, request: Request<Body>, next: Next) -> Response {
    if hub.stopped.load(Ordering::Relaxed) { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let api = request.uri().path().starts_with("/api/");
    let (valid_host, valid_origin) = {
        let inner = hub.inner.lock().unwrap();
        (inner.origins.iter().any(|origin| origin_authority(origin).as_deref() == Some(host)),
        request_origin(request.headers()).is_some_and(|origin| {
            inner.origins.iter().any(|allowed| allowed == origin) && origin_authority(origin).as_deref() == Some(host)
        }))
    };
    let session_check = request.method() == axum::http::Method::GET && request.uri().path() == "/api/auth";
    let valid_session_check = session_check && (request_origin(request.headers()).is_none() || valid_origin);
    if !valid_host || api && !valid_origin && !valid_session_check { return StatusCode::FORBIDDEN.into_response(); }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

#[derive(Deserialize)]
struct PairRequest { token: String }

async fn pair_handler(State(hub): State<Arc<RemoteHub>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(request): Json<PairRequest>) -> Response {
    let origin = request_origin(&headers).unwrap_or("");
    match hub.pair(&request.token, origin, peer.ip()) {
        Ok(cookie) => authenticated_response(&cookie, origin, CLIENT_LIFETIME),
        Err(status) => status.into_response(),
    }
}

fn authenticated_response(cookie: &str, origin: &str, lifetime: Duration) -> Response {
    let secure = if origin.starts_with("https://") { "; Secure" } else { "" };
    let mut response = Json(json!({"ok": true})).into_response();
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!(
        "sterm_remote={cookie}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}", lifetime.as_secs()
    )).unwrap());
    response
}

#[derive(Debug)]
struct LoginError {
    status: StatusCode,
    message: &'static str,
    retry_after: Option<u64>,
}

impl LoginError {
    fn new(status: StatusCode, message: &'static str) -> Self { Self { status, message, retry_after: None } }
    fn limited(wait: u64) -> Self {
        Self { status: StatusCode::TOO_MANY_REQUESTS, message: "尝试次数较多，请稍后重试", retry_after: Some(wait) }
    }
}

impl IntoResponse for LoginError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({"message": self.message, "retryAfter": self.retry_after}))).into_response();
        if let Some(wait) = self.retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_str(&wait.to_string()).unwrap());
        }
        response
    }
}

async fn auth_handler(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap) -> Json<Value> {
    let authenticated = hub.authenticate(&headers).is_some();
    let inner = hub.inner.lock().unwrap();
    let host = headers.get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let password_allowed = inner.origins.iter().any(|origin| origin.starts_with("https://") && origin_authority(origin).as_deref() == Some(host));
    Json(json!({"authenticated": authenticated, "passwordEnabled": inner.password.is_some(), "passwordAllowed": password_allowed}))
}

#[derive(Deserialize)]
struct PasswordRequest { password: String }

async fn password_handler(State(hub): State<Arc<RemoteHub>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(request): Json<PasswordRequest>) -> Response {
    let origin = request_origin(&headers).unwrap_or("");
    match hub.login_password(request.password, origin, peer.ip()).await {
        Ok(cookie) => authenticated_response(&cookie, origin, PASSWORD_CLIENT_LIFETIME),
        Err(error) => error.into_response(),
    }
}

async fn websocket_handler(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let Some(client) = hub.authenticate(&headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    {
        let mut inner = hub.inner.lock().unwrap();
        if !inner.connected.insert(client.clone()) { return StatusCode::CONFLICT.into_response(); }
    }
    ws.max_message_size(16 * 1024).max_frame_size(16 * 1024)
        .on_upgrade(move |socket| run_socket(socket, hub, client))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMessage {
    Attach { id: u32, generation: u32 },
    Claim { id: u32, generation: u32 },
    Release { id: u32, generation: u32 },
    Input { id: u32, generation: u32, epoch: String, data: String },
    Resize { id: u32, generation: u32, epoch: String, cols: u16, rows: u16 },
    Ack { seq: u64 },
    Ping,
}

async fn send(socket: &mut WebSocket, message: &Value) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Text(message.to_string().into()))).await, Ok(Ok(())))
}

async fn run_socket(mut socket: WebSocket, hub: Arc<RemoteHub>, client: String) {
    let mut receiver = hub.events.subscribe();
    let mut attached: Option<(u32, u32)> = None;
    let mut snapshot_request: Option<String> = None;
    let mut ready = false;
    let mut last_seen = Instant::now();
    let mut snapshot_started = Instant::now();
    let mut outstanding: VecDeque<u64> = VecDeque::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    if !send(&mut socket, &json!({"type": "hello", "clientId": client, "sessions": hub.status().sessions})).await {
        hub.disconnect(&client);
        return;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(incoming)) = incoming else { break };
                last_seen = Instant::now();
                let Message::Text(text) = incoming else { if matches!(incoming, Message::Close(_)) { break; } continue };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else { break };
                let result = match message {
                    ClientMessage::Attach { id, generation } => {
                        if let Some((previous, previous_generation)) = attached {
                            let owns = hub.inner.lock().unwrap().sessions.get(&previous).is_some_and(|session| session.controller.as_deref() == Some(&client));
                            if owns { hub.reclaim(previous, Some(previous_generation)); }
                        }
                        let exists = hub.inner.lock().unwrap().sessions.get(&id).is_some_and(|session| session.generation == generation);
                        if exists {
                            attached = Some((id, generation));
                            ready = false;
                            outstanding.clear();
                            let request = secret();
                            snapshot_request = Some(request.clone());
                            snapshot_started = Instant::now();
                            hub.host.snapshot(id, generation, &request)
                        } else { Err("此会话未分享或已结束".into()) }
                    }
                    ClientMessage::Claim { id, generation } => {
                        if ready && attached == Some((id, generation)) { hub.claim(&client, id, generation) }
                        else { Err("请等待画面同步完成".into()) }
                    }
                    ClientMessage::Release { id, generation } => {
                        let owns = hub.inner.lock().unwrap().sessions.get(&id).is_some_and(|session| session.generation == generation && session.controller.as_deref() == Some(&client));
                        if owns { hub.reclaim(id, Some(generation)); }
                        Ok(())
                    }
                    ClientMessage::Input { id, generation, epoch, data } => {
                        if ready && attached == Some((id, generation)) {
                            let input_hub = hub.clone();
                            let input_client = client.clone();
                            tokio::task::spawn_blocking(move || input_hub.remote_write(&input_client, id, generation, &epoch, &data)).await.unwrap_or_else(|_| Err("输入任务失败".into()))
                        } else { Err("请重新连接会话".into()) }
                    }
                    ClientMessage::Resize { id, generation, epoch, cols, rows } => {
                        let allowed = hub.inner.lock().unwrap().sessions.get(&id).is_some_and(|session| session.generation == generation && session.controller.as_deref() == Some(&client) && session.epoch.as_deref() == Some(&epoch));
                        if allowed && (20..=400).contains(&cols) && (5..=200).contains(&rows) { hub.host.resize(id, generation, cols, rows) }
                        else { Err("无效的尺寸或控制权已失效".into()) }
                    }
                    ClientMessage::Ack { seq } => {
                        while outstanding.front().is_some_and(|sent| *sent <= seq) { outstanding.pop_front(); }
                        Ok(())
                    }
                    ClientMessage::Ping => Ok(()),
                };
                if let Err(error) = result { if !send(&mut socket, &json!({"type": "error", "message": error})).await { break; } }
            }
            event = receiver.recv() => {
                let Ok(event) = event else { break };
                let kind = event["type"].as_str().unwrap_or("");
                if kind == "stopped" || kind == "revoked" { let _ = send(&mut socket, &event).await; break; }
                if kind == "sessions" {
                    if !send(&mut socket, &event).await { break; }
                    continue;
                }
                if attached != event["id"].as_u64().zip(event["generation"].as_u64()).map(|(id, generation)| (id as u32, generation as u32)) { continue; }
                if kind == "snapshot" {
                    if snapshot_request.as_deref() != event["request"].as_str() { continue; }
                    ready = true;
                    snapshot_request = None;
                } else if !ready { continue; }
                if let Some(seq) = event["seq"].as_u64() { outstanding.push_back(seq); }
                if outstanding.len() > 128 || !send(&mut socket, &event).await { break; }
            }
            _ = heartbeat.tick() => {
                if hub.stopped.load(Ordering::Relaxed) || last_seen.elapsed() > Duration::from_secs(45)
                    || !ready && snapshot_request.is_some() && snapshot_started.elapsed() > Duration::from_secs(15) { break; }
                let authorized = hub.inner.lock().unwrap().clients.values().any(|entry| entry.id == client && entry.expires > Instant::now());
                if !authorized {
                    let _ = send(&mut socket, &json!({"type": "authExpired"})).await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
    let _ = socket.send(Message::Close(None)).await;
    hub.disconnect(&client);
}

async fn asset_handler(State(hub): State<Arc<RemoteHub>>, request: Request<Body>) -> Response {
    let path = match request.uri().path() { "/" => "remote.html", path => path.trim_start_matches('/') };
    let permitted = path == "remote.html" || path == "agent.html" || path.starts_with("assets/") && !path.contains("..") && !path.contains('%');
    if !permitted { return StatusCode::NOT_FOUND.into_response(); }
    match hub.host.asset(path) {
        Some((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn router(hub: Arc<RemoteHub>) -> Router {
    Router::new().route("/api/pair", post(pair_handler)).route("/api/auth", get(auth_handler))
        .route("/api/password/login", post(password_handler)).route("/api/ws", get(websocket_handler))
        .merge(crate::agent_runtime::web::routes())
        .fallback(get(asset_handler)).layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn_with_state(hub.clone(), guard)).with_state(hub)
}

#[derive(Default)]
pub struct RemoteState {
    hub: Mutex<Option<Arc<RemoteHub>>>,
    operation: tokio::sync::Mutex<()>,
}

pub fn remote_port(hub: &RemoteHub) -> u16 { hub.inner.lock().unwrap().port }

impl RemoteState {
    pub fn current(&self) -> Option<Arc<RemoteHub>> { self.hub.lock().unwrap().clone() }

    pub fn stop(&self) {
        if let Some(hub) = self.hub.lock().unwrap().take() { hub.stop(); }
    }
}

fn password_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = crate::app_dirs::directory(app, "data")?.join("remote-auth");
    Ok(dir.join("password.credential"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordConfig { enabled: bool, can_reveal: bool }

impl From<&remote_password::SavedPassword> for PasswordConfig {
    fn from(saved: &remote_password::SavedPassword) -> Self {
        Self { enabled: saved.verifier.is_some(), can_reveal: saved.can_reveal() }
    }
}

#[tauri::command]
pub fn remote_password_config(app: AppHandle) -> Result<PasswordConfig, String> {
    Ok(PasswordConfig::from(&remote_password::read(&password_path(&app)?)?))
}

/// 仅供电脑端设置窗口主动查看，不通过远程 HTTP 或状态轮询返回访问码。
#[tauri::command]
pub fn remote_password_reveal(app: AppHandle, window: tauri::WebviewWindow) -> Result<String, String> {
    if window.label() != "main" { return Err("仅可在电脑端设置中查看长期访问码".into()); }
    Ok(remote_password::read(&password_path(&app)?)?.reveal()?.to_owned())
}

#[tauri::command]
pub async fn remote_password_save(app: AppHandle, password: Option<String>) -> Result<PasswordConfig, String> {
    let state = app.state::<RemoteState>();
    let _operation = state.operation.lock().await;
    let path = password_path(&app)?;
    let saved = tauri::async_runtime::spawn_blocking(move || remote_password::write(&path, password.as_deref()))
        .await.map_err(|_| "无法更新访问码")??;
    let config = PasswordConfig::from(&saved);
    if let Some(hub) = state.current() { hub.set_password(saved.verifier); }
    Ok(config)
}

#[tauri::command]
pub async fn remote_start(app: AppHandle, port: u16, lan: bool) -> Result<RemoteStatus, String> {
    let state = app.state::<RemoteState>();
    let _operation = state.operation.lock().await;
    let password = remote_password::read(&password_path(&app)?)?.verifier;
    if app.state::<RemoteState>().current().is_some() { return Err("远程服务已启动".into()); }
    let listener = tokio::net::TcpListener::bind((if lan { "0.0.0.0" } else { "127.0.0.1" }, port)).await.map_err(|error| format!("启动远程服务失败：{error}"))?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let mut origins = vec![format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    if lan {
        for interface in if_addrs::get_if_addrs().map_err(|error| error.to_string())? {
            if let IpAddr::V4(ip) = interface.ip() {
                if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() { origins.push(format!("http://{ip}:{port}")); }
            }
        }
    }
    origins.sort();
    origins.dedup();
    let hub = Arc::new(RemoteHub::new(port, origins, Arc::new(DesktopHost(app.clone()))));
    hub.inner.lock().unwrap().password = password;
    {
        let state = app.state::<RemoteState>();
        let mut current = state.hub.lock().unwrap();
        if current.is_some() { return Err("远程服务已启动".into()); }
        *current = Some(hub.clone());
    }
    let task_hub = hub.clone();
    tokio::spawn(async move {
        let shutdown_hub = task_hub.clone();
        let _ = axum::serve(listener, router(task_hub).into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async move { shutdown_hub.shutdown.notified().await }).await;
    });
    Ok(hub.status())
}

#[tauri::command]
pub fn remote_status(state: tauri::State<RemoteState>) -> Option<RemoteStatus> { state.current().map(|hub| hub.status()) }

#[tauri::command]
pub fn remote_pair(state: tauri::State<RemoteState>, origin: String) -> Result<Value, String> {
    state.current().ok_or("远程服务尚未启动")?.issue_pair(&origin)
}

#[tauri::command]
pub async fn remote_stop(app: AppHandle) -> Result<(), String> {
    let state = app.state::<RemoteState>();
    let _operation = state.operation.lock().await;
    app.state::<crate::tunnel::TunnelState>().stop()?;
    app.state::<RemoteState>().stop();
    Ok(())
}

#[tauri::command]
pub fn remote_share(state: tauri::State<RemoteState>, session: SharedSession, enabled: bool) -> Result<(), String> {
    let hub = state.current().ok_or("远程服务尚未启动")?;
    if enabled { hub.share(session) } else { hub.unshare(session.id, session.generation); Ok(()) }
}

#[tauri::command]
pub fn remote_publish(state: tauri::State<RemoteState>, id: u32, generation: u32, seq: u64, data: String, cols: u16, rows: u16) {
    if let Some(hub) = state.current() { hub.publish(id, generation, seq, data, cols, rows); }
}

#[tauri::command]
pub fn remote_snapshot(state: tauri::State<RemoteState>, request: String, id: u32, generation: u32, seq: u64, data: String, cols: u16, rows: u16) -> Result<(), String> {
    state.current().ok_or("远程服务已关闭")?.snapshot(request, id, generation, seq, data, cols, rows)
}

#[tauri::command]
pub fn remote_reclaim(state: tauri::State<RemoteState>, id: u32) {
    if let Some(hub) = state.current() { hub.reclaim(id, None); }
}

#[tauri::command]
pub fn remote_revoke_clients(state: tauri::State<RemoteState>) {
    if let Some(hub) = state.current() { hub.revoke_clients(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHost {
        input: Mutex<Vec<String>>,
        snapshots: Mutex<Vec<String>>,
    }
    impl RemoteHost for TestHost {
        fn snapshot(&self, _: u32, _: u32, request: &str) -> Result<(), String> { self.snapshots.lock().unwrap().push(request.into()); Ok(()) }
        fn write(&self, _: u32, _: u32, data: &str) -> Result<(), String> { self.input.lock().unwrap().push(data.into()); Ok(()) }
        fn control_changed(&self, _: &SharedSession) {}
        fn resize(&self, _: u32, _: u32, _: u16, _: u16) -> Result<(), String> { Ok(()) }
        fn asset(&self, _: &str) -> Option<(Vec<u8>, String)> { Some((b"remote-only".to_vec(), "text/html".into())) }
    }

    fn fixture() -> (Arc<RemoteHub>, Arc<TestHost>) {
        let host = Arc::new(TestHost { input: Mutex::new(Vec::new()), snapshots: Mutex::new(Vec::new()) });
        let hub = Arc::new(RemoteHub::new(8123, vec!["http://127.0.0.1:8123".into()], host.clone()));
        for id in ["phone", "next-device"] {
            hub.inner.lock().unwrap().clients.insert(format!("test-{id}"), Client { id: id.into(), expires: Instant::now() + CLIENT_LIFETIME });
        }
        (hub, host)
    }

    #[tokio::test]
    async fn password_login_coexists_with_qr_and_preserves_control_permissions() {
        let (hub, _) = fixture();
        let verifier = PasswordVerifier::new("012345").unwrap();
        hub.set_password(Some(verifier));
        hub.set_public_origin(Some("https://terminal.example".into()));
        let ip = "127.0.0.1".parse().unwrap();
        assert_eq!(hub.login_password("012345".into(), "http://127.0.0.1:8123", ip).await.unwrap_err().status, StatusCode::FORBIDDEN);
        assert_eq!(hub.login_password("wrong".into(), "https://terminal.example", ip).await.unwrap_err().status, StatusCode::UNAUTHORIZED);
        let password_cookie = hub.login_password("012345".into(), "https://terminal.example", ip).await.unwrap();
        let pair = hub.issue_pair("https://terminal.example").unwrap();
        let token = pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let qr_cookie = hub.pair(token, "https://terminal.example", ip).unwrap();
        let (client, password_remaining, qr_remaining) = {
            let inner = hub.inner.lock().unwrap();
            let password = &inner.clients[&password_cookie];
            let qr = &inner.clients[&qr_cookie];
            (password.id.clone(), password.expires.duration_since(Instant::now()), qr.expires.duration_since(Instant::now()))
        };
        assert!(password_remaining > Duration::from_secs(29 * 24 * 60 * 60));
        assert!(qr_remaining <= CLIENT_LIFETIME);
        assert!(qr_remaining > CLIENT_LIFETIME - Duration::from_secs(10));
        hub.share(SharedSession { id: 1, generation: 1, link_key: None, title: "readonly".into(), cols: 80, rows: 24,
            allow_control: false, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None }).unwrap();
        assert!(hub.claim(&client, 1, 1).is_err());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={password_cookie}")).unwrap());
        assert!(hub.authenticate(&headers).is_some());
        hub.revoke_clients();
        assert!(hub.authenticate(&headers).is_none());
        assert!(hub.status().password_enabled);
        let replacement_cookie = hub.login_password("012345".into(), "https://terminal.example", ip).await.unwrap();
        headers.insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={replacement_cookie}")).unwrap());
        hub.set_password(None);
        assert!(hub.authenticate(&headers).is_none());
        assert!(!hub.status().password_enabled);
        assert!(hub.login_password("012345".into(), "https://terminal.example", ip).await.is_err());
    }

    #[test]
    fn saved_code_reads_preserve_devices_and_config_contains_only_metadata() {
        let dir = crate::storage::tests::TempDir::new();
        let path = dir.path().join("password.credential");
        let saved = remote_password::write(&path, Some("012345")).unwrap();
        let (hub, _) = fixture();
        hub.set_password(saved.verifier);
        let origin = "http://127.0.0.1:8123";
        let pair = hub.issue_pair(origin).unwrap();
        let token = pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let ip = "127.0.0.1".parse().unwrap();
        let cookie = hub.pair(token, origin, ip).unwrap();
        let unused_pair = hub.issue_pair(origin).unwrap();
        let unused_token = unused_pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let saved = remote_password::read(&path).unwrap();
        assert_eq!(saved.reveal().unwrap(), "012345");
        assert_eq!(serde_json::to_value(PasswordConfig::from(&saved)).unwrap(), json!({ "enabled": true, "canReveal": true }));
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={cookie}")).unwrap());
        assert!(hub.authenticate(&headers).is_some());
        assert!(hub.pair(unused_token, origin, ip).is_ok());
    }

    #[tokio::test]
    async fn revocation_rejects_an_inflight_password_check() {
        let (hub, _) = fixture();
        hub.set_password(Some(PasswordVerifier::new("123456").unwrap()));
        hub.set_public_origin(Some("https://terminal.example".into()));
        let request_hub = hub.clone();
        let request = tokio::spawn(async move {
            request_hub.login_password("123456".into(), "https://terminal.example", "127.0.0.1".parse().unwrap()).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while hub.password_checks.available_permits() == 2 { tokio::task::yield_now().await; }
        }).await.unwrap();
        hub.revoke_clients();
        assert_eq!(request.await.unwrap().unwrap_err().status, StatusCode::UNAUTHORIZED);
        assert!(hub.inner.lock().unwrap().clients.is_empty());
    }

    #[tokio::test]
    async fn password_http_routes_enforce_origins_limits_and_secure_cookie() {
        let (hub, _) = fixture();
        hub.set_password(Some(PasswordVerifier::new("123456").unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let root = format!("http://{address}");
        let origin = "https://terminal.example";
        hub.inner.lock().unwrap().origins = vec![root.clone(), origin.into()];
        let app = router(hub.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        let client = reqwest::Client::new();
        let status = client.get(format!("{root}/api/auth")).send().await.unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        assert_eq!(status.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(status.json::<Value>().await.unwrap(), json!({"authenticated": false, "passwordEnabled": true, "passwordAllowed": false}));
        assert_eq!(client.get(format!("{root}/api/auth")).header("origin", "https://attacker.example").send().await.unwrap().status(), StatusCode::FORBIDDEN);
        assert_eq!(client.post(format!("{root}/api/password/login")).header("origin", &root).json(&json!({"password":"123456"})).send().await.unwrap().status(), StatusCode::FORBIDDEN);
        assert_eq!(client.post(format!("{root}/api/password/login")).header("origin", origin).json(&json!({"password":"123456"})).send().await.unwrap().status(), StatusCode::FORBIDDEN);
        let response = client.post(format!("{root}/api/password/login")).header("host", "terminal.example").header("origin", origin)
            .json(&json!({"password":"123456"})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("Max-Age=2592000"));
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert!(cookie.ends_with("; Secure"));
        assert!(!cookie.contains("123456"));
        let response = client.get(format!("{root}/api/auth")).header("host", "terminal.example")
            .header("cookie", cookie.split(';').next().unwrap()).send().await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(response["authenticated"], true);
        assert_eq!(response["passwordAllowed"], true);
        for path in ["/api/password/reveal", "/api/remote_password_reveal", "/api/password/config"] {
            let response = client.get(format!("{root}{path}")).header("host", "terminal.example").header("origin", origin)
                .header("cookie", cookie.split(';').next().unwrap()).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        for suffix in 1..5 {
            let response = client.post(format!("{root}/api/password/login")).header("host", "terminal.example").header("origin", origin)
                .header("x-forwarded-for", format!("203.0.113.{suffix}")).json(&json!({"password":"bad"})).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = client.post(format!("{root}/api/password/login")).header("host", "terminal.example").header("origin", origin)
            .header("x-forwarded-for", "203.0.113.99").json(&json!({"password":"123456"})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(header::RETRY_AFTER));
        server.abort();
    }

    #[test]
    fn pairing_is_single_use_origin_bound_and_revocable() {
        let (hub, _) = fixture();
        let pair = hub.issue_pair("http://127.0.0.1:8123").unwrap();
        let token = pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let ip = "127.0.0.1".parse().unwrap();
        assert!(hub.pair(token, "https://attacker.example", ip).is_err());
        let cookie = hub.pair(token, "http://127.0.0.1:8123", ip).unwrap();
        assert!(hub.pair(token, "http://127.0.0.1:8123", ip).is_err());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={cookie}")).unwrap());
        assert!(hub.authenticate(&headers).is_some());
        hub.stop();
        assert!(hub.authenticate(&headers).is_none());
    }

    #[test]
    fn leases_reject_viewers_stale_packets_and_old_generations() {
        let (hub, host) = fixture();
        let mut session = SharedSession { id: 1, generation: 2, link_key: None, title: "test".into(), cols: 80, rows: 24, allow_control: false, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None };
        hub.share(session.clone()).unwrap();
        assert!(hub.claim("phone", 1, 2).is_err());
        session.allow_control = true;
        hub.share(session).unwrap();
        hub.claim("phone", 1, 2).unwrap();
        let epoch = hub.status().sessions[0].epoch.clone().unwrap();
        assert!(hub.remote_write("other", 1, 2, &epoch, "bad").is_err());
        assert!(hub.remote_write("phone", 1, 1, &epoch, "bad").is_err());
        hub.remote_write("phone", 1, 2, &epoch, "ok").unwrap();
        assert!(!hub.local_write_allowed(1, 2));
        assert!(hub.with_unclaimed_input(1, 2, || panic!("已被手机接管")).is_err());
        hub.reclaim(1, None);
        assert!(hub.local_write_allowed(1, 2));
        assert!(hub.remote_write("phone", 1, 2, &epoch, "bad").is_err());
        assert_eq!(*host.input.lock().unwrap(), vec!["ok"]);
    }

    #[test]
    fn notification_links_pair_without_a_password_and_follow_the_shared_generation() {
        let (hub, _) = fixture();
        let mut session = SharedSession {
            id: 7, generation: 1, link_key: None, title: "session".into(), cols: 80, rows: 24,
            allow_control: false, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None,
        };
        hub.share(session.clone()).unwrap();
        assert!(hub.shared_link(7, 1).is_none());
        hub.set_public_origin(Some("https://terminal.example".into()));
        let url = hub.shared_link(7, 1).unwrap();
        assert!(url.starts_with("https://terminal.example/#pair="));
        let key = hub.status().sessions[0].link_key.clone().unwrap();
        assert!(url.ends_with(&format!("&session={key}")));
        assert_eq!(hub.inner.lock().unwrap().clients.len(), 2);
        let token = url.split("#pair=").nth(1).unwrap().split('&').next().unwrap();
        let ip = "127.0.0.1".parse().unwrap();
        assert!(hub.pair(token, "https://terminal.example", ip).is_ok());
        assert!(hub.pair(token, "https://terminal.example", ip).is_err());
        session.title = "renamed".into();
        hub.share(session.clone()).unwrap();
        assert!(hub.shared_link(7, 1).unwrap().ends_with(&format!("&session={key}")));
        let pending = hub.shared_link(7, 1).unwrap();
        session.generation = 2;
        hub.share(session).unwrap();
        assert!(hub.shared_link(7, 1).is_none());
        let token = pending.split("#pair=").nth(1).unwrap().split('&').next().unwrap();
        assert!(hub.pair(token, "https://terminal.example", ip).is_err());
        assert_ne!(hub.shared_link(7, 2), Some(url));
        hub.unshare(7, 2);
        assert!(hub.shared_link(7, 2).is_none());
    }

    #[test]
    fn notification_pairing_can_be_refreshed_and_password_mode_uses_login_links() {
        let (hub, _) = fixture();
        let session = SharedSession {
            id: 7, generation: 1, link_key: None, title: "session".into(), cols: 80, rows: 24,
            allow_control: false, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None,
        };
        hub.share(session.clone()).unwrap();
        let origin = "https://terminal.example";
        let ip = "127.0.0.1".parse().unwrap();
        hub.set_public_origin(Some(origin.into()));
        let expired = hub.shared_link(7, 1).unwrap();
        let token = expired.split("#pair=").nth(1).unwrap().split('&').next().unwrap();
        hub.inner.lock().unwrap().pairs.get_mut(token).unwrap().expires = Instant::now() - Duration::from_secs(1);
        assert!(hub.pair(token, origin, ip).is_err());
        let fresh = hub.shared_link(7, 1).unwrap();
        let token = fresh.split("#pair=").nth(1).unwrap().split('&').next().unwrap();
        assert!(hub.pair(token, origin, ip).is_ok());
        let old = hub.shared_link(7, 1).unwrap();
        hub.unshare(7, 1);
        hub.share(session).unwrap();
        let token = old.split("#pair=").nth(1).unwrap().split('&').next().unwrap();
        assert!(hub.pair(token, origin, ip).is_err());
        hub.set_password(Some(PasswordVerifier::new("123456").unwrap()));
        let url = hub.shared_link(7, 1).unwrap();
        assert!(url.starts_with("https://terminal.example/#session="));
        assert_eq!(hub.shared_link(7, 1), Some(url));
        assert!(hub.inner.lock().unwrap().pairs.is_empty());
    }

    #[test]
    fn public_origin_changes_preserve_lan_pair_tickets() {
        let (hub, _) = fixture();
        let origin = "http://127.0.0.1:8123";
        hub.set_public_origin(Some("https://first.example".into()));
        let lan_pair = hub.issue_pair(origin).unwrap();
        let public_pair = hub.issue_pair("https://first.example").unwrap();
        hub.set_public_origin(Some("https://second.example".into()));
        let public_token = public_pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        assert!(hub.pair(public_token, "https://first.example", "127.0.0.1".parse().unwrap()).is_err());
        let lan_token = lan_pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        assert!(hub.pair(lan_token, origin, "127.0.0.1".parse().unwrap()).is_ok());
        assert!(hub.issue_pair("https://second.example").is_ok());
    }

    #[test]
    fn agent_runtime_cannot_be_shared_as_a_raw_terminal() {
        struct AgentHost(crate::agent_runtime::hub::SharedHub);
        impl RemoteHost for AgentHost {
            fn agent_hub(&self) -> Option<crate::agent_runtime::hub::SharedHub> { Some(self.0.clone()) }
            fn snapshot(&self, _:u32, _:u32, _: &str) -> Result<(),String> { Ok(()) }
            fn write(&self, _:u32, _:u32, _: &str) -> Result<(),String> { Ok(()) }
            fn control_changed(&self, _: &SharedSession) {}
            fn resize(&self, _:u32, _:u32, _:u16, _:u16) -> Result<(),String> { Ok(()) }
            fn asset(&self, _: &str) -> Option<(Vec<u8>, String)> { None }
        }
        let agents = Arc::new(crate::agent_runtime::hub::Hub::default());
        agents.register("claude",1,2,"D:/probe".into());
        let hub = RemoteHub::new(1,vec![],Arc::new(AgentHost(agents)));
        let session = SharedSession { id:1,generation:2,title:"agent".into(),cols:100,rows:30,allow_control:true,
            controller:None,epoch:None,seq:0,cwd:None,agent:None,agent_status:None,link_key:None };
        assert!(hub.share(session).is_err());
        assert!(hub.status().sessions.is_empty());
        assert!(hub.local_write_allowed(1,2));
    }
    #[test]
    fn downgrading_or_unsharing_immediately_invalidates_control() {
        let (hub, host) = fixture();
        let mut session = SharedSession { id: 1, generation: 2, link_key: None, title: "test".into(), cols: 80, rows: 24,
            allow_control: true, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None };
        hub.share(session.clone()).unwrap();
        hub.claim("phone", 1, 2).unwrap();
        let previous = hub.status().sessions[0].epoch.clone().unwrap();
        session.allow_control = false;
        hub.share(session.clone()).unwrap();
        assert!(hub.local_write_allowed(1, 2));
        assert!(hub.status().sessions[0].controller.is_none());
        assert!(hub.remote_write("phone", 1, 2, &previous, "stale").is_err());
        assert!(hub.claim("phone", 1, 2).is_err());
        session.allow_control = true;
        hub.share(session).unwrap();
        hub.claim("phone", 1, 2).unwrap();
        let epoch = hub.status().sessions[0].epoch.clone().unwrap();
        assert_ne!(previous, epoch);
        hub.unshare(1, 2);
        assert!(hub.local_write_allowed(1, 2));
        assert!(hub.remote_write("phone", 1, 2, &epoch, "stale").is_err());
        assert!(host.input.lock().unwrap().is_empty());
    }

    #[test]
    fn revoking_devices_rejects_auth_and_queued_claims_but_keeps_sessions() {
        let (hub, host) = fixture();
        hub.share(SharedSession { id: 1, generation: 2, link_key: None, title: "existing shell".into(), cols: 80, rows: 24,
            allow_control: true, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None }).unwrap();
        let origin = "http://127.0.0.1:8123";
        let pair = hub.issue_pair(origin).unwrap();
        let token = pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let cookie = hub.pair(token, origin, "127.0.0.1".parse().unwrap()).unwrap();
        let unused = hub.issue_pair(origin).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={cookie}")).unwrap());
        let client = hub.authenticate(&headers).unwrap();
        hub.claim(&client, 1, 2).unwrap();
        let epoch = hub.status().sessions[0].epoch.clone().unwrap();
        hub.revoke_clients();
        assert!(hub.authenticate(&headers).is_none());
        assert!(hub.claim(&client, 1, 2).is_err());
        assert!(hub.remote_write(&client, 1, 2, &epoch, "stale").is_err());
        assert!(hub.local_write_allowed(1, 2));
        assert_eq!(hub.status().sessions.len(), 1);
        assert!(!hub.stopped.load(Ordering::Relaxed));
        let unused_token = unused["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        assert!(hub.pair(unused_token, origin, "127.0.0.1".parse().unwrap()).is_err());
        assert!(host.input.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn gateway_rejects_foreign_origins_and_unauthenticated_sockets() {
        let (hub, _) = fixture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        hub.inner.lock().unwrap().origins = vec![format!("http://{address}")];
        let app = router(hub);
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        let client = reqwest::Client::new();
        let root = format!("http://{address}");
        assert_eq!(client.get(&root).send().await.unwrap().status(), StatusCode::OK);
        assert_eq!(client.get(format!("{root}/index.html")).send().await.unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(client.post(format!("{root}/api/pair")).header("origin", "https://attacker.example").json(&json!({"token": "bad"})).send().await.unwrap().status(), StatusCode::FORBIDDEN);
        assert_eq!(client.get(format!("{root}/api/ws")).header("origin", &root).header("connection", "upgrade").header("upgrade", "websocket").header("sec-websocket-version", "13").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        server.abort();
    }

    #[tokio::test]
    async fn websocket_attaches_existing_session_and_disconnect_releases_control() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message as WsMessage}};
        let (hub, host) = fixture();
        hub.share(SharedSession { id: 7, generation: 3, link_key: None, title: "existing shell".into(), cols: 80, rows: 24,
            allow_control: true, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None }).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        hub.inner.lock().unwrap().origins = vec![origin.clone()];
        let app = router(hub.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        let pair = hub.issue_pair(&origin).unwrap();
        let token = pair["url"].as_str().unwrap().split("#pair=").nth(1).unwrap();
        let cookie = hub.pair(token, &origin, address.ip()).unwrap();
        let mut request = format!("ws://{address}/api/ws").into_client_request().unwrap();
        request.headers_mut().insert(header::ORIGIN, HeaderValue::from_str(&origin).unwrap());
        request.headers_mut().insert(header::COOKIE, HeaderValue::from_str(&format!("sterm_remote={cookie}")).unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        let hello: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(hello["sessions"][0]["title"], "existing shell");
        socket.send(WsMessage::Text(json!({"type": "attach", "id": 7, "generation": 3}).to_string().into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while host.snapshots.lock().unwrap().is_empty() { tokio::task::yield_now().await; }
        }).await.unwrap();
        let snapshot_request = host.snapshots.lock().unwrap()[0].clone();
        hub.snapshot(snapshot_request, 7, 3, 0, "existing screen".into(), 80, 24).unwrap();
        async fn receive(socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, kind: &str) -> Value {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let message = socket.next().await.unwrap().unwrap();
                    if let WsMessage::Text(text) = message {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["type"] == kind { return value; }
                    }
                }
            }).await.unwrap()
        }
        assert_eq!(receive(&mut socket, "snapshot").await["data"], "existing screen");
        hub.publish(7, 3, 1, "incremental".into(), 80, 24);
        assert_eq!(receive(&mut socket, "output").await["seq"], 1);
        socket.send(WsMessage::Text(json!({"type": "claim", "id": 7, "generation": 3}).to_string().into())).await.unwrap();
        let roster = receive(&mut socket, "sessions").await;
        let epoch = roster["sessions"][0]["epoch"].as_str().unwrap();
        socket.send(WsMessage::Text(json!({"type": "input", "id": 7, "generation": 3, "epoch": epoch, "data": "test\r"}).to_string().into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while host.input.lock().unwrap().is_empty() { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert_eq!(*host.input.lock().unwrap(), vec!["test\r"]);
        socket.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !hub.local_write_allowed(7, 3) { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert_eq!(hub.status().sessions.len(), 1);
        assert!(hub.claim("next-device", 7, 3).is_ok());
        server.abort();
    }
}
