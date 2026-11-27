//! 浏览器 Agent 通道复用远程访问鉴权，不暴露本机桥接凭据。
use super::{protocol::Command, State as AgentState};
use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;
use crate::remote::RemoteHub;

fn check(hub: &RemoteHub, headers: &HeaderMap) -> Result<(), StatusCode> {
    hub.agent_authenticated(headers).then_some(()).ok_or(StatusCode::UNAUTHORIZED)
}

async fn list(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    check(&hub, &headers)?;
    let agent = hub.agent_hub().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(agent.list(true)))
}

#[derive(Deserialize)]
struct Cursor { seq: Option<u64> }
async fn snapshot(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap, Path(id): Path<String>, Query(cursor): Query<Cursor>) -> Result<Json<Value>, StatusCode> {
    check(&hub, &headers)?;
    let agent = hub.agent_hub().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(agent.update(&id, cursor.seq).map_err(|_| StatusCode::NOT_FOUND)?))
}

async fn command(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap, Json(command): Json<Command>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check(&hub, &headers).map_err(|status| (status, Json(json!({"message":"请重新登录"}))))?;
    let receipt = hub.submit_agent(&headers, command).map_err(|message| (StatusCode::CONFLICT, Json(json!({"message":message}))))?;
    Ok(Json(json!(receipt)))
}

async fn receipt(State(hub): State<Arc<RemoteHub>>, headers: HeaderMap, Path((runtime, command)): Path<(String, String)>) -> Result<Json<Value>, StatusCode> {
    check(&hub, &headers)?;
    let agent = hub.agent_hub().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(json!(agent.receipt(&runtime, &command, true).map_err(|_| StatusCode::NOT_FOUND)?)))
}

pub fn routes() -> Router<Arc<RemoteHub>> {
    Router::new().route("/api/agents", get(list).post(list)).route("/api/agents/command", post(command))
        .route("/api/agents/{id}", get(snapshot).post(snapshot)).route("/api/agents/{runtime}/commands/{command}", get(receipt).post(receipt))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

pub fn hub(app: &tauri::AppHandle) -> super::hub::SharedHub { app.state::<AgentState>().hub.clone() }
