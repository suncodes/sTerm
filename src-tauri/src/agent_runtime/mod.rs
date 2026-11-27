//! Agent 模式运行时入口；与远程网页的启停解耦。
pub mod protocol;
mod interactions;
pub(crate) mod hub;
mod bridge;
pub(crate) mod shell;
mod codex;
mod codex_mapping;
pub(crate) mod launcher;
pub mod web;
#[cfg(debug_assertions)]
pub mod dev;

use hub::SharedHub;

#[derive(Default)]
pub struct State {
    pub hub: SharedHub,
    pub shell: shell::ShellState,
    bridge: tokio::sync::Mutex<Option<bridge::Bridge>>,
    codex: tokio::sync::Mutex<Option<launcher::CodexServer>>,
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
        self.shell.retire(tab,generation);
        self.hub.retire(tab, generation);
    }

    pub fn stop(&self) {
        self.shell.stop();
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
