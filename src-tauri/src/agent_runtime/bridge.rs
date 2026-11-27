//! Claude Mod 的私有本机 HTTP 通道，与浏览器的 Origin/登录校验分离。
use super::{hub::SharedHub, protocol::{Batch, Event}};
use axum::{extract::{ConnectInfo, DefaultBodyLimit, Path, State}, http::{HeaderMap, StatusCode}, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, sync::Arc};
use tokio::sync::Notify;

pub struct Bridge {
    pub address: String,
    shutdown: Arc<Notify>,
}

fn allowed(hub: &SharedHub, id: &str, headers: &HeaderMap, peer: SocketAddr) -> bool {
    peer.ip().is_loopback() && headers.get("origin").is_none() &&
        headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| hub.authorize(id, token))
}

async fn poll(State(hub): State<SharedHub>, Path(id): Path<String>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !allowed(&hub, &id, &headers, peer) { return Err(StatusCode::UNAUTHORIZED); }
    Ok(Json(json!({ "commands": hub.commands(&id) })))
}

async fn claim(State(hub): State<SharedHub>, Path(id): Path<String>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(command): Json<super::protocol::Command>) -> Result<Json<Value>, StatusCode> {
    if !allowed(&hub, &id, &headers, peer) { return Err(StatusCode::UNAUTHORIZED); }
    hub.claim(&id, &command).map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(json!({"ok":true})))
}

async fn event(State(hub): State<SharedHub>, Path(id): Path<String>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(batch): Json<Batch>) -> Result<Json<Value>, StatusCode> {
    if !allowed(&hub, &id, &headers, peer) { return Err(StatusCode::UNAUTHORIZED); }
    if batch.events.len() > 256 { return Err(StatusCode::PAYLOAD_TOO_LARGE); }
    let events: Result<Vec<Event>, _> = batch.events.into_iter().map(serde_json::from_value).collect();
    hub.apply(&id, &batch.session_id, events.map_err(|_| StatusCode::BAD_REQUEST)?).map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(json!({ "ok": true })))
}

impl Bridge {
    pub async fn start(hub: SharedHub) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await.map_err(|e| e.to_string())?;
        let address = format!("http://127.0.0.1:{}", listener.local_addr().map_err(|e| e.to_string())?.port());
        let shutdown = Arc::new(Notify::new());
        let done = shutdown.clone();
        let router = Router::new().route("/bridge/{id}/poll", get(poll)).route("/bridge/{id}/event", post(event)).route("/bridge/{id}/claim", post(claim))
            .layer(DefaultBodyLimit::max(1024 * 1024)).with_state(hub);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async move { done.notified().await }).await;
        });
        Ok(Self { address, shutdown })
    }
}
impl Drop for Bridge { fn drop(&mut self) { self.shutdown.notify_one(); } }

