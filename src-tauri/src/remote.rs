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

const PAIR_LIFETIME: Duration = Duration::from_secs(300);
const CLIENT_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_CLIENTS: usize = 8;
const MAX_SNAPSHOT: usize = 2 * 1024 * 1024;

pub trait RemoteHost: Send + Sync {
    fn snapshot(&self, id: u32, generation: u32, request: &str) -> Result<(), String>;
    fn write(&self, id: u32, generation: u32, data: &str) -> Result<(), String>;
    fn control_changed(&self, session: &SharedSession);
    fn resize(&self, id: u32, generation: u32, cols: u16, rows: u16) -> Result<(), String>;
    fn asset(&self, path: &str) -> Option<(Vec<u8>, String)>;
}

struct DesktopHost(AppHandle);

impl RemoteHost for DesktopHost {
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
}

pub struct RemoteHub {
    inner: Mutex<Inner>,
    input: Mutex<()>,
    events: broadcast::Sender<Value>,
    host: Arc<dyn RemoteHost>,
    stopped: AtomicBool,
    shutdown: Notify,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub port: u16,
    pub origins: Vec<String>,
    pub sessions: Vec<SharedSession>,
    pub clients: usize,
}

impl RemoteHub {
    fn new(port: u16, origins: Vec<String>, host: Arc<dyn RemoteHost>) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Mutex::new(Inner {
                port, origins, pairs: HashMap::new(), clients: HashMap::new(),
                connected: HashSet::new(), sessions: HashMap::new(), attempts: HashMap::new(),
            }),
            input: Mutex::new(()), events, host, stopped: AtomicBool::new(false), shutdown: Notify::new(),
        }
    }

    fn status(&self) -> RemoteStatus {
        let inner = self.inner.lock().unwrap();
        let mut sessions: Vec<_> = inner.sessions.values().cloned().collect();
        sessions.sort_by_key(|session| session.id);
        RemoteStatus {
            enabled: !self.stopped.load(Ordering::Relaxed), port: inner.port,
            origins: inner.origins.clone(), sessions, clients: inner.connected.len(),
        }
    }

    fn roster(&self) {
        let _ = self.events.send(json!({"type": "sessions", "sessions": self.status().sessions}));
    }

    pub fn share(&self, mut session: SharedSession) -> Result<(), String> {
        if session.cols < 2 || session.rows == 0 || session.title.len() > 512 {
            return Err("无效的会话信息".into());
        }
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        session.controller = None;
        session.epoch = None;
        if let Some(previous) = inner.sessions.get(&session.id).filter(|old| old.generation == session.generation) {
            session.seq = previous.seq;
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
        if !inner.origins.iter().any(|allowed| allowed == origin) { return Err("请选择已连接的访问地址".into()); }
        inner.pairs.retain(|_, ticket| ticket.expires > Instant::now());
        if inner.pairs.len() >= 16 { inner.pairs.clear(); }
        let token = secret();
        inner.pairs.insert(token.clone(), PairTicket { origin: origin.into(), expires: Instant::now() + PAIR_LIFETIME });
        Ok(json!({"url": format!("{origin}/#pair={token}"), "expiresIn": PAIR_LIFETIME.as_secs()}))
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
        inner.pairs.remove(token);
        let cookie = secret();
        inner.clients.insert(cookie.clone(), Client { id: uuid::Uuid::new_v4().to_string(), expires: Instant::now() + CLIENT_LIFETIME });
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
            return Err("设备授权已失效，请重新扫码配对".into());
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
            return Err("设备授权已失效，请重新扫码配对".into());
        }
        let session = inner.sessions.get(&id).filter(|session| session.generation == generation).ok_or("会话已结束")?;
        if !session.allow_control || session.controller.as_deref() != Some(client) || session.epoch.as_deref() != Some(epoch) {
            return Err("控制权已失效，请重新接管".into());
        }
        drop(inner);
        self.host.write(id, generation, data)
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
        let _input = self.input.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
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
        self.stopped.store(true, Ordering::Relaxed);
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
    if !valid_host || api && !valid_origin { return StatusCode::FORBIDDEN.into_response(); }
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
        Ok(cookie) => {
            let secure = if origin.starts_with("https://") { "; Secure" } else { "" };
            let mut response = Json(json!({"ok": true})).into_response();
            response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&format!("sterm_remote={cookie}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{secure}")).unwrap());
            response
        }
        Err(status) => status.into_response(),
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
                if !authorized || socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
    let _ = socket.send(Message::Close(None)).await;
    hub.disconnect(&client);
}

async fn asset_handler(State(hub): State<Arc<RemoteHub>>, request: Request<Body>) -> Response {
    let path = match request.uri().path() { "/" => "remote.html", path => path.trim_start_matches('/') };
    let permitted = path == "remote.html" || path.starts_with("assets/") && !path.contains("..") && !path.contains('%');
    if !permitted { return StatusCode::NOT_FOUND.into_response(); }
    match hub.host.asset(path) {
        Some((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn router(hub: Arc<RemoteHub>) -> Router {
    Router::new().route("/api/pair", post(pair_handler)).route("/api/ws", get(websocket_handler))
        .fallback(get(asset_handler)).layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn_with_state(hub.clone(), guard)).with_state(hub)
}

#[derive(Default)]
pub struct RemoteState {
    hub: Mutex<Option<Arc<RemoteHub>>>,
}

pub fn remote_port(hub: &RemoteHub) -> u16 { hub.inner.lock().unwrap().port }

impl RemoteState {
    pub fn current(&self) -> Option<Arc<RemoteHub>> { self.hub.lock().unwrap().clone() }

    pub fn stop(&self) {
        if let Some(hub) = self.hub.lock().unwrap().take() { hub.stop(); }
    }
}

#[tauri::command]
pub async fn remote_start(app: AppHandle, port: u16, lan: bool) -> Result<RemoteStatus, String> {
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
pub fn remote_stop(app: AppHandle) -> Result<(), String> {
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
        let mut session = SharedSession { id: 1, generation: 2, title: "test".into(), cols: 80, rows: 24, allow_control: false, controller: None, epoch: None, seq: 0, cwd: None, agent: None, agent_status: None };
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
        hub.reclaim(1, None);
        assert!(hub.local_write_allowed(1, 2));
        assert!(hub.remote_write("phone", 1, 2, &epoch, "bad").is_err());
        assert_eq!(*host.input.lock().unwrap(), vec!["ok"]);
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
    fn downgrading_or_unsharing_immediately_invalidates_control() {
        let (hub, host) = fixture();
        let mut session = SharedSession { id: 1, generation: 2, title: "test".into(), cols: 80, rows: 24,
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
        hub.share(SharedSession { id: 1, generation: 2, title: "existing shell".into(), cols: 80, rows: 24,
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
        hub.share(SharedSession { id: 7, generation: 3, title: "existing shell".into(), cols: 80, rows: 24,
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


