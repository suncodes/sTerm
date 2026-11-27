//! 浏览器 Agent 通道复用远程访问鉴权，不暴露本机桥接凭据。
use super::{protocol::Command, State as AgentState};
use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;
use crate::remote::RemoteHub;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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

#[derive(Deserialize)]
struct Media { item:String }

fn image_mime(bytes:&[u8])->Option<&'static str>{
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n"){Some("image/png")}
    else if bytes.starts_with(b"\xff\xd8\xff"){Some("image/jpeg")}
    else if bytes.starts_with(b"GIF87a")||bytes.starts_with(b"GIF89a"){Some("image/gif")}
    else if bytes.starts_with(b"RIFF")&&bytes.get(8..12)==Some(b"WEBP"){Some("image/webp")}
    else {None}
}

async fn media(State(hub):State<Arc<RemoteHub>>,headers:HeaderMap,Path(id):Path<String>,Query(query):Query<Media>)->Result<Response,StatusCode>{
    check(&hub,&headers)?;
    let agent=hub.agent_hub().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let session=agent.snapshot(&id,true).map_err(|_|StatusCode::NOT_FOUND)?;
    let item=session.items.iter().find(|item|item.id==query.item&&item.kind=="image").ok_or(StatusCode::NOT_FOUND)?;
    let path=item.data["savedPath"].as_str().or(item.data["path"].as_str()).ok_or(StatusCode::NOT_FOUND)?;
    if path.starts_with("\\\\")||path.starts_with("//") { return Err(StatusCode::NOT_FOUND); }
    let path=std::path::Path::new(path);
    let path=if path.is_absolute(){path.to_path_buf()}else{std::path::Path::new(&session.cwd).join(path)};
    let file=tokio::fs::File::open(path).await.map_err(|_|StatusCode::NOT_FOUND)?;
    if !file.metadata().await.map_err(|_|StatusCode::NOT_FOUND)?.is_file(){return Err(StatusCode::NOT_FOUND)}
    let mut bytes=Vec::new();
    file.take(8*1024*1024+1).read_to_end(&mut bytes).await.map_err(|_|StatusCode::NOT_FOUND)?;
    if bytes.len()>8*1024*1024{return Err(StatusCode::PAYLOAD_TOO_LARGE)}
    let mime=image_mime(&bytes).ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    Ok(([ ("content-type",mime),("cache-control","no-store"),("x-content-type-options","nosniff"),("content-security-policy","default-src 'none'") ],bytes).into_response())
}

async fn task_output(State(hub):State<Arc<RemoteHub>>,headers:HeaderMap,Path((id,task_id)):Path<(String,String)>)->Result<Json<Value>,StatusCode>{
    check(&hub,&headers)?;
    let agent=hub.agent_hub().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let session=agent.snapshot(&id,true).map_err(|_|StatusCode::NOT_FOUND)?;
    let task=session.tasks.iter().find(|task|task.id==task_id&&task.kind=="shell").ok_or(StatusCode::NOT_FOUND)?;
    let path=task.data["outputFile"].as_str().ok_or(StatusCode::NOT_FOUND)?;
    if !std::path::Path::new(path).is_absolute()||path.starts_with("\\\\")||path.starts_with("//"){return Err(StatusCode::NOT_FOUND)}
    let mut file=tokio::fs::File::open(path).await.map_err(|_|StatusCode::NOT_FOUND)?;
    let metadata=file.metadata().await.map_err(|_|StatusCode::NOT_FOUND)?;
    if !metadata.is_file(){return Err(StatusCode::NOT_FOUND)}
    let start=metadata.len().saturating_sub(64*1024);
    file.seek(std::io::SeekFrom::Start(start)).await.map_err(|_|StatusCode::NOT_FOUND)?;
    let mut bytes=Vec::new();file.take(64*1024).read_to_end(&mut bytes).await.map_err(|_|StatusCode::NOT_FOUND)?;
    Ok(Json(json!({"text":String::from_utf8_lossy(&bytes),"truncated":start>0})))
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

#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct EndRequest { generation:u32, session_id:String }
async fn end(State(hub):State<Arc<RemoteHub>>,headers:HeaderMap,Path(id):Path<String>,Json(request):Json<EndRequest>)->Result<Json<Value>,(StatusCode,Json<Value>)>{
    check(&hub,&headers).map_err(|status|(status,Json(json!({"message":"请重新登录"}))))?;
    hub.end_agent(&headers,&id,request.generation,&request.session_id)
        .map_err(|message|(StatusCode::CONFLICT,Json(json!({"message":message}))))?;
    Ok(Json(json!({"ok":true})))
}
pub fn routes() -> Router<Arc<RemoteHub>> {
    Router::new().route("/api/agents/{id}/end",post(end)).route("/api/agents", get(list).post(list)).route("/api/agents/command", post(command))
        .route("/api/agents/{id}/media",get(media))
        .route("/api/agents/{id}/tasks/{task}/output",get(task_output).post(task_output))
        .route("/api/agents/{id}", get(snapshot).post(snapshot)).route("/api/agents/{runtime}/commands/{command}", get(receipt).post(receipt))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

pub fn hub(app: &tauri::AppHandle) -> super::hub::SharedHub { app.state::<AgentState>().hub.clone() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_preview_accepts_only_raster_signatures() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n"),Some("image/png"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBP"),Some("image/webp"));
        assert!(image_mime(b"<svg onload='alert(1)'/>").is_none());
        assert!(image_mime(b"<html><script>alert(1)</script></html>").is_none());
    }
}
