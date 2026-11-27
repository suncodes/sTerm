//! 仅 debug 构建使用的本地文件驱动，用于隔离原生集成验证。
use super::State;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub fn start(app: AppHandle) {
    let Some(root) = std::env::var_os("STERM_DEV_DATA_DIR").map(PathBuf::from) else { return };
    if !root.is_absolute() || std::env::var("STERM_DEV_PROBE").as_deref() != Ok("1") { return; }
    let inbox = root.join("probe-command.json");
    let outbox = root.join("probe-result.json");
    tauri::async_runtime::spawn(async move {
        let mut previous = std::fs::read_to_string(&inbox).unwrap_or_default();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let Ok(text) = std::fs::read_to_string(&inbox) else { continue };
            if text == previous { continue; }
            previous = text.clone();
            let Ok(command) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else { continue };
            let result = run(&app, &command).await;
            let payload = match result { Ok(result) => json!({"id":command["id"],"ok":true,"result":result}), Err(message) => json!({"id":command["id"],"ok":false,"error":message}) };
            let _ = std::fs::write(&outbox, payload.to_string());
        }
    });
}

async fn run(app: &AppHandle, command: &Value) -> Result<Value, String> {
    let state = app.state::<State>();
    match command["action"].as_str().unwrap_or_default() {
        "spawn" => {
            let tab = command["tab"].as_u64().ok_or("tab")? as u32;
            let cwd = command["cwd"].as_str().ok_or("cwd")?;
            let agent = command["agent"].as_str().ok_or("agent")?;
            // 与桌面入口使用同一 prepare → 一次性上下文 → pty_spawn 链。
            app.state::<crate::AppState>().ptys.spawn(app.clone(),tab,1,"pwsh",Some(cwd.into()),130,38)?;
            let launch = super::agent_runtime_prepare(app.clone(), app.get_webview_window("main").ok_or("main")?, app.state::<State>(), tab, 2, agent.into(), cwd.into(), command["resume"].as_str().map(str::to_string)).await?;
            let id = launch.runtime_id.clone();
            crate::pty_spawn(app.clone(),app.state::<crate::AppState>(),tab,2,"pwsh".into(),Some(cwd.into()),130,38,Some(id.clone()))?;
            Ok(json!({"runtimeId":id}))
        }
        "write" => {
            app.state::<crate::AppState>().ptys.write(command["tab"].as_u64().ok_or("tab")? as u32,2,command["data"].as_str().ok_or("data")?)?;
            Ok(json!({}))
        }
        "list" => Ok(state.hub.list(false)),
        "snapshot" => Ok(json!(state.hub.snapshot(command["runtimeId"].as_str().ok_or("runtime")?,false)?)),
        "share" => {
            state.hub.share(command["runtimeId"].as_str().ok_or("runtime")?,2,command["shared"].as_bool().unwrap_or(true),command["control"].as_bool().unwrap_or(true))?;
            Ok(json!({}))
        }
        "disconnect_codex" => {
            let server = state.codex.lock().await;
            server.as_ref().ok_or("Codex backend")?.client.debug_disconnect().await?;
            Ok(json!({}))
        }
        "size" => Ok(json!(app.state::<crate::AppState>().ptys.debug_size(command["tab"].as_u64().ok_or("tab")? as u32)?)),
        "close" => {
            crate::pty_kill(app.clone(),app.state::<crate::AppState>(), command["tab"].as_u64().ok_or("tab")? as u32, 2)?;
            Ok(json!({}))
        }
        "remote_stop" => { crate::remote::remote_stop(app.clone()).await?; Ok(json!({})) }
        "revoke" => { crate::remote::remote_revoke_clients(app.state::<crate::remote::RemoteState>()); Ok(json!({})) }
        "remote" => {
            let status=crate::remote::remote_start(app.clone(),0,false).await?;
            Ok(json!(status))
        }
        "pair" => Ok(crate::remote::remote_pair(app.state::<crate::remote::RemoteState>(),command["origin"].as_str().ok_or("origin")?.into())?),
        "stop" => {
            app.state::<crate::AppState>().ptys.kill_all();
            app.state::<State>().stop();
            app.state::<crate::remote::RemoteState>().stop();
            app.exit(0);Ok(json!({}))
        }
        _ => Err("未知调试操作".into()),
    }
}
