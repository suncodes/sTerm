//! Agent 模式运行时入口；与远程网页的启停解耦。
pub mod protocol;
pub(crate) mod hub;
mod bridge;
mod codex;
pub(crate) mod launcher;
pub mod web;
#[cfg(debug_assertions)]
pub mod dev;

use std::{collections::HashMap, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager};
use hub::SharedHub;

#[derive(Default)]
pub struct State {
    pub hub: SharedHub,
    bridge: tokio::sync::Mutex<Option<bridge::Bridge>>,
    codex: tokio::sync::Mutex<Option<launcher::CodexServer>>,
    launches: Mutex<HashMap<String, launcher::Launch>>,
}

impl State {
    async fn bridge(&self) -> Result<String, String> {
        let mut current = self.bridge.lock().await;
        if current.is_none() {
            *current = Some(bridge::Bridge::start(self.hub.clone()).await?);
            let weak = std::sync::Arc::downgrade(&self.hub);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let Some(hub) = weak.upgrade() else { break };
                    hub.expire_connections();
                }
            });
        }
        Ok(current.as_ref().unwrap().address.clone())
    }

    pub fn retire(&self, tab: u32, generation: u32) {
        self.hub.retire(tab, generation);
        self.launches.lock().unwrap().retain(|id, _| self.hub.is_alive(id));
    }

    pub fn take_launch(&self, runtime: &str, tab: u32, generation: u32) -> Result<launcher::Launch, String> {
        let session = self.hub.snapshot(runtime, false)?;
        if session.tab_id != tab || session.generation != generation { return Err("启动上下文已失效".into()); }
        self.launches.lock().unwrap().remove(runtime).ok_or("启动信息已使用或已失效".into())
    }

    pub fn cancel_launch(&self, runtime: &str) { self.launches.lock().unwrap().remove(runtime); self.hub.remove(runtime); }

    pub fn stop(&self) {
        if let Ok(mut server) = self.codex.try_lock() { *server = None; }
    }
}

#[tauri::command]
pub fn agent_runtime_list(window: tauri::WebviewWindow, state: tauri::State<State>) -> Result<serde_json::Value, String> {
    if window.label() != "main" { return Err("仅主窗口可管理 Agent".into()); }
    Ok(state.hub.list(false))
}

#[tauri::command]
pub fn agent_runtime_share(window: tauri::WebviewWindow, state: tauri::State<State>, runtime_id: String, generation: u32, shared: bool, allow_control: bool) -> Result<(), String> {
    if window.label() != "main" { return Err("仅主窗口可修改分享".into()); }
    state.hub.share(&runtime_id, generation, shared, allow_control)
}

#[tauri::command]
pub async fn agent_runtime_prepare(app: AppHandle, window: tauri::WebviewWindow, state: tauri::State<'_, State>, tab_id: u32, generation: u32, agent: String, cwd: String, resume: Option<String>) -> Result<launcher::Launch, String> {
    if window.label() != "main" { return Err("仅主窗口可启动 Agent".into()); }
    if !["claude", "codex"].contains(&agent.as_str()) { return Err("首版支持 Claude 与 Codex".into()); }
    if !PathBuf::from(&cwd).is_dir() { return Err("工作目录不存在".into()); }
    if generation > 0 && state.hub.has_tab(tab_id, generation - 1) { return Err("此终端正在运行共享 Agent，请新建终端".into()); }
    if generation == 0 || !app.state::<crate::AppState>().ptys.has_generation(tab_id, generation - 1) { return Err("终端已替换".into()); }
    let launch = launcher::prepare(&app, &state, tab_id, generation, &agent, &cwd, resume).await?;
    let id = launch.runtime_id.clone();
    if !app.state::<crate::AppState>().ptys.has_generation(tab_id, generation - 1) { state.cancel_launch(&id); return Err("终端已关闭或替换".into()); }
    state.launches.lock().unwrap().insert(id.clone(), launch);
    Ok(launcher::Launch { runtime_id: id, program: String::new(), args: Vec::new(), env: Vec::new() })
}






#[tauri::command]
pub fn agent_runtime_cancel(window: tauri::WebviewWindow, state: tauri::State<State>, runtime_id: String) -> Result<(), String> {
    if window.label() != "main" { return Err("仅主窗口可管理 Agent".into()); }
    state.cancel_launch(&runtime_id);
    Ok(())
}
