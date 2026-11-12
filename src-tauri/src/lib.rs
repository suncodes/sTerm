mod agents;
mod console;
mod diagnostics;
mod events;
mod overlay;
mod pty;
mod paths;
mod storage;
mod win_cwd;
mod workspaces;

use serde::Serialize;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    ptys: pty::PtyManager,
    ws: workspaces::Store,
}

// ---------- shell ----------

#[tauri::command]
fn list_shells() -> Vec<pty::ShellInfo> {
    pty::detect_shells()
}

// ---------- pty ----------

#[tauri::command]
fn pty_spawn(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: u32,
    generation: u32,
    shell_id: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.ptys.spawn(app, id, generation, &shell_id, cwd, cols, rows)
}

#[tauri::command]
fn pty_write(
    state: tauri::State<AppState>,
    id: u32,
    generation: u32,
    data: String,
) -> Result<(), String> {
    state.ptys.write(id, generation, &data)
}

#[tauri::command]
fn pty_resize(
    state: tauri::State<AppState>,
    id: u32,
    generation: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.ptys.resize(id, generation, cols, rows)
}

#[tauri::command]
fn pty_kill(
    state: tauri::State<AppState>,
    id: u32,
    generation: u32,
) -> Result<(), String> {
    state.ptys.kill(id, generation)
}

/// 读取指定终端中 shell 进程的当前工作目录（用于工作区拦截）
#[tauri::command]
fn get_shell_cwd(state: tauri::State<AppState>, id: u32, generation: u32) -> Option<String> {
    state
        .ptys
        .shell_pid(id, generation)
        .and_then(|pid| win_cwd::get_cwd(pid))
}

const SHELL_INTEGRATION_BEGIN: &str = "# >>> sTerm cwd reporting >>>";
const SHELL_INTEGRATION_END: &str = "# <<< sTerm cwd reporting <<<";
const SHELL_INTEGRATION_BLOCK: &str = r#"
# >>> sTerm cwd reporting >>>
if (-not (Get-Variable -Name __sterm_orig_prompt -Scope Global -ErrorAction SilentlyContinue)) {
    $__sterm_orig_prompt = $function:prompt
    function global:prompt {
        $__sterm_loc = $ExecutionContext.SessionState.Path.CurrentLocation.ToString()
        $__sterm_uri = 'file:///' + ($__sterm_loc -replace '\\', '/')
        $e = [char]27
        Write-Host -NoNewline "$e]7;$__sterm_uri$e\"
        & $__sterm_orig_prompt
    }
}
# <<< sTerm cwd reporting <<<
"#;

#[derive(Serialize)]
struct ShellIntegrationStatus {
    installed: bool,
}

fn shell_profile_path(shell_id: &str) -> Result<PathBuf, String> {
    if !matches!(shell_id, "powershell" | "pwsh") {
        return Err("当前默认 Shell 不支持 PowerShell 目录上报集成".into());
    }
    let shell = pty::resolve_shell(shell_id);
    let out = std::process::Command::new(&shell.program)
        .args(["-NoLogo", "-NoProfile", "-Command", "$PROFILE"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("无法启动 {}: {e}", shell.program))?;
    if !out.status.success() {
        return Err(format!("无法读取 {} 的 PowerShell profile 路径", shell.name));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() || !path.to_ascii_lowercase().ends_with(".ps1") {
        return Err("无法获取 PowerShell profile 路径".into());
    }
    Ok(PathBuf::from(path))
}

fn strip_marked_shell_blocks(mut profile: String) -> Result<String, String> {
    for (begin, end) in [(SHELL_INTEGRATION_BEGIN, SHELL_INTEGRATION_END)] {
        while let Some(start) = profile.find(begin) {
            let content_start = start + begin.len();
            let end_offset = profile[content_start..]
                .find(end)
                .ok_or("检测到不完整的 sTerm PowerShell 集成标记；未修改 profile")?;
            let mut content_end = content_start + end_offset + end.len();
            if profile[content_end..].starts_with("\r\n") {
                content_end += 2;
            } else if profile[content_end..].starts_with('\n') {
                content_end += 1;
            }
            profile.replace_range(start..content_end, "");
        }
    }
    Ok(profile)
}

fn update_shell_profile(
    path: &Path,
    create_if_missing: bool,
    change: impl FnOnce(String) -> Result<String, String>,
) -> Result<(), String> {
    if !create_if_missing && !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let _lock = storage::lock(path)?;
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => String::new(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取 {} 失败：{error}", path.display())),
    };
    let updated = change(existing.clone())?;
    if updated != existing {
        storage::write(path, updated.as_bytes(), true)?;
    }
    Ok(())
}

fn shell_integration_status_impl(shell_id: &str) -> Result<ShellIntegrationStatus, String> {
    let profile = shell_profile_path(shell_id)?;
    let content = match fs::read_to_string(profile) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    Ok(ShellIntegrationStatus {
        installed: content.contains(SHELL_INTEGRATION_BEGIN),
    })
}

/// 仅在用户从设置页主动选择后写入 PowerShell profile。
#[tauri::command]
fn shell_integration_status(shell_id: String) -> Result<ShellIntegrationStatus, String> {
    shell_integration_status_impl(&shell_id)
}

/// 安装时只更新 sTerm 标记包围的代码，不触碰 profile 的其他内容。
#[tauri::command]
fn install_shell_integration(shell_id: String) -> Result<ShellIntegrationStatus, String> {
    let profile = shell_profile_path(&shell_id)?;
    update_shell_profile(&profile, true, |content| {
        let mut updated = strip_marked_shell_blocks(content)?;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        if !updated.is_empty() {
            updated.push('\n');
        }
        updated.push_str(SHELL_INTEGRATION_BLOCK);
        Ok(updated)
    })?;
    shell_integration_status_impl(&shell_id)
}

#[tauri::command]
fn remove_shell_integration(shell_id: String) -> Result<ShellIntegrationStatus, String> {
    let profile = shell_profile_path(&shell_id)?;
    update_shell_profile(&profile, false, strip_marked_shell_blocks)?;
    shell_integration_status_impl(&shell_id)
}

// ---------- workspaces ----------

#[tauri::command]
fn store_list(state: tauri::State<AppState>) -> workspaces::StoreData {
    state.ws.list()
}

fn emit_ws(app: &AppHandle, state: &AppState) {
    let _ = app.emit("workspaces-changed", state.ws.list());
}
#[tauri::command]
fn ws_add(
    app: AppHandle,
    state: tauri::State<AppState>,
    path: String,
    agent: Option<String>,
) -> Result<workspaces::Workspace, String> {
    let ws = state.ws.add(&path, &agent.unwrap_or_default())?;
    emit_ws(&app, state.inner());
    Ok(ws)
}

#[tauri::command]
fn ws_rename(app: AppHandle, state: tauri::State<AppState>, id: String, name: String) -> Result<(), String> {
    state.ws.rename(&id, &name)?;
    emit_ws(&app, state.inner());
    Ok(())
}

#[tauri::command]
fn ws_remove(app: AppHandle, state: tauri::State<AppState>, id: String) -> Result<(), String> {
    state.ws.remove(&id)?;
    emit_ws(&app, state.inner());
    Ok(())
}

#[tauri::command]
fn ws_pin(app: AppHandle, state: tauri::State<AppState>, id: String, pinned: bool) -> Result<(), String> {
    state.ws.set_pinned(&id, pinned)?;
    emit_ws(&app, state.inner());
    Ok(())
}

/// 移动工作区到文件夹（folderId 为 null 时移回根级）
#[tauri::command]
fn ws_move(app: AppHandle, state: tauri::State<AppState>, id: String, folder_id: Option<String>) -> Result<(), String> {
    state.ws.move_ws(&id, folder_id)?;
    emit_ws(&app, state.inner());
    Ok(())
}

/// 原生 Agent 不再安装状态 Hook，只清理旧版 sTerm 条目；Pi 继续使用扩展。
/// 此函数仅由用户在设置页主动触发。
fn install_agent_integration_impl(cli: &str) -> Result<(), String> {
    let profile = std::env::var("USERPROFILE").map_err(|_| "no USERPROFILE".to_string())?;
    let home = PathBuf::from(profile);
    let (env_name, default_dir, filename) = match cli {
        "claude" => ("CLAUDE_CONFIG_DIR", home.join(".claude"), "settings.json"),
        "codex" => ("CODEX_HOME", home.join(".codex"), "hooks.json"),
        "pi" => {
            let dir = std::env::var_os("PI_CODING_AGENT_DIR").map(PathBuf::from)
                .unwrap_or_else(|| home.join(".pi").join("agent"));
            return if dir.is_dir() { ensure_pi_extension(&dir) } else { Ok(()) };
        }
        _ => return Err("unknown cli".into()),
    };
    let dir = std::env::var_os(env_name).map(PathBuf::from).unwrap_or(default_dir);
    cleanup_agent_hooks(&dir.join(filename))
}

fn cleanup_agent_hooks(path: &Path) -> Result<(), String> {
    // 原生协议无需配置文件；尚未创建配置时不产生目录、锁或空文件。
    if !path.try_exists().map_err(|e| e.to_string())? { return Ok(()); }
    let _lock = storage::lock(path)?;
    let Some(mut root) = storage::read_json(path)? else { return Ok(()); };
    if !remove_agent_hooks(&mut root)? { return Ok(()); }
    let out = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    storage::write(path, &out, true)
}

/// 兼容 Windows / POSIX 的旧状态桥，仅移除明确引用 sTerm 脚本的命令。
fn remove_agent_hooks(root: &mut serde_json::Value) -> Result<bool, String> {
    let before = root.clone();
    let obj = root.as_object_mut().ok_or("config root is not an object")?;
    let events = ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"];
    let bare = events.iter().any(|event| obj.contains_key(*event));
    let target = if bare {
        obj
    } else {
        let Some(hooks) = obj.get_mut("hooks") else { return Ok(false); };
        hooks.as_object_mut().ok_or("hooks section is not an object")?
    };
    for event in events {
        let Some(value) = target.get_mut(event) else { continue; };
        let list = value.as_array_mut().ok_or_else(|| format!("{event} is not an array"))?;
        list.retain_mut(|entry| {
            if is_sterm_hook(entry) { return false; }
            let Some(hooks) = entry.get_mut("hooks").and_then(|v| v.as_array_mut()) else {
                return true;
            };
            let previous_len = hooks.len();
            hooks.retain(|hook| !is_sterm_hook(hook));
            hooks.len() == previous_len || !hooks.is_empty()
        });
    }
    Ok(*root != before)
}

fn is_sterm_hook(hook: &serde_json::Value) -> bool {
    hook.get("command").and_then(|v| v.as_str())
        .is_some_and(|command| command.contains("sTerm-hook.cmd") || command.contains("sterm-agent-events.py"))
}

/// pi 扩展接入：写 ~/.pi/agent/extensions/sterm-events.ts
/// 扩展在 pi 进程内订阅生命周期事件并写入事件信箱（与 shell hook 同格式），全局目录免审批自动加载
fn ensure_pi_extension(agent_dir: &Path) -> Result<(), String> {
    const EXT: &str = r#"// sTerm agent 状态事件桥：pi 生命周期事件 → sTerm 事件信箱
// 守卫：仅当由 sTerm 启动（STERM_TAB_ID 存在）时写入，其余环境零副作用
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const tab = process.env.STERM_TAB_ID;
const base = join(process.env.LOCALAPPDATA ?? "", "sTerm", "events");

function write(kind: string) {
  if (!tab) return;
  try {
    mkdirSync(base, { recursive: true });
    appendFileSync(join(base, `${tab}.event`), `pi-${kind}\n`);
  } catch {
    // 写失败不影响 pi 本身
  }
}

export default function (pi: { on: (ev: string, cb: () => void) => void }) {
  pi.on("session_start", () => write("start"));
  pi.on("agent_start", () => write("prompt"));
  pi.on("agent_end", () => write("stop"));
  pi.on("session_shutdown", () => write("end"));
}
"#;
    let path = agent_dir.join("extensions").join("sterm-events.ts");
    storage::write_if_changed(&path, EXT, true)
}

#[tauri::command]
fn install_agent_integration(cli: String) -> Result<(), String> {
    install_agent_integration_impl(&cli)
}

#[tauri::command]
fn ws_sync_agents(
    app: AppHandle,
    state: tauri::State<AppState>,
    path: String,
    agents: Vec<String>,
) -> Result<(), String> {
    state.ws.sync_agents(&path, &agents)?;
    emit_ws(&app, state.inner());
    Ok(())
}

// ---------- folders ----------

#[tauri::command]
fn folder_add(
    app: AppHandle,
    state: tauri::State<AppState>,
    name: String,
    parent_id: Option<String>,
) -> Result<workspaces::Folder, String> {
    let f = state.ws.folder_add(&name, parent_id)?;
    emit_ws(&app, state.inner());
    Ok(f)
}

/// 移动文件夹：改父级（嵌套）或排序（beforeId 为 null 时追加到同父级末尾）
#[tauri::command]
fn folder_move(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: String,
    parent_id: Option<String>,
    before_id: Option<String>,
) -> Result<(), String> {
    state.ws.folder_move(&id, parent_id, before_id)?;
    emit_ws(&app, state.inner());
    Ok(())
}

#[tauri::command]
fn folder_rename(app: AppHandle, state: tauri::State<AppState>, id: String, name: String) -> Result<(), String> {
    state.ws.folder_rename(&id, &name)?;
    emit_ws(&app, state.inner());
    Ok(())
}

#[tauri::command]
fn folder_remove(app: AppHandle, state: tauri::State<AppState>, id: String) -> Result<(), String> {
    state.ws.folder_remove(&id)?;
    emit_ws(&app, state.inner());
    Ok(())
}

#[tauri::command]
fn folder_toggle(app: AppHandle, state: tauri::State<AppState>, id: String, collapsed: bool) -> Result<(), String> {
    state.ws.folder_set_collapsed(&id, collapsed)?;
    emit_ws(&app, state.inner());
    Ok(())
}

// ---------- agent 扫描 / 导入 ----------

/// 扫描 ~/.claude/projects 与 ~/.codex/sessions，返回候选历史工作区
#[tauri::command]
async fn agent_import_scan() -> Result<Vec<agents::ImportCandidate>, String> {
    tauri::async_runtime::spawn_blocking(agents::scan_import).await.map_err(|e| e.to_string())
}

/// 批量导入工作区，返回新增数量
#[tauri::command]
fn ws_import(
    app: AppHandle,
    state: tauri::State<AppState>,
    candidates: Vec<agents::ImportCandidate>,
) -> Result<usize, String> {
    let n = state.ws.import(&candidates)?;
    emit_ws(&app, state.inner());
    Ok(n)
}

/// 列出指定工作区下最近的会话（点击会话可 resume）
#[tauri::command]
async fn agent_sessions(path: String, limit: usize) -> Result<Vec<agents::AgentSession>, String> {
    tauri::async_runtime::spawn_blocking(move || agents::agent_sessions(&path, limit))
        .await.map_err(|e| e.to_string())
}

/// 批量判断各工作区是否拥有历史会话（侧栏"有会话才显示展开箭头"）
#[tauri::command]
async fn session_counts(paths: Vec<String>) -> Result<Vec<bool>, String> {
    tauri::async_runtime::spawn_blocking(move || agents::session_counts(&paths))
        .await.map_err(|e| e.to_string())
}

// ---------- 入口 ----------

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            if let Ok(dir) = app.path().app_log_dir() {
                diagnostics::init(&dir);
            }
            console::init(&app.path().resource_dir()?)?;
            let store = workspaces::Store::load(app.handle())?;
            app.manage(overlay::OverlayState::load(app.handle())?);
            app.manage(AppState {
                ptys: pty::PtyManager::new(),
                ws: store,
            });
            // Pi 扩展事件信箱：初始化目录并轮询状态。
            events::init()?;
            events::spawn_poller(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 仅主窗口销毁时结束终端与应用；关闭悬浮窗不会触发会话清理。
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    state.ptys.kill_all();
                }
                events::shutdown();
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            overlay::overlay_init,
            overlay::overlay_snapshot,
            overlay::overlay_set_enabled,
            overlay::overlay_set_opacity,
            overlay::overlay_publish,
            overlay::overlay_ready,
            overlay::overlay_save_position,
            overlay::overlay_activate,
            list_shells,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            get_shell_cwd,
            shell_integration_status,
            install_shell_integration,
            remove_shell_integration,
            store_list,
            ws_add,
            ws_rename,
            ws_remove,
            ws_pin,
            ws_move,
            ws_sync_agents,
            install_agent_integration,
            folder_add,
            folder_move,
            folder_rename,
            folder_remove,
            folder_toggle,
            agent_import_scan,
            ws_import,
            agent_sessions,
            session_counts
        ])
        .run(tauri::generate_context!())
        .expect("error while running sTerm");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    #[test]
    fn shell_integration_removal_is_scoped_to_its_marked_block() {
        let profile = format!("before\n{SHELL_INTEGRATION_BLOCK}\nafter\n");
        let cleaned = strip_marked_shell_blocks(profile).unwrap();
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
        assert!(!cleaned.contains(SHELL_INTEGRATION_BEGIN));
        assert!(!cleaned.contains("__sterm_orig_prompt"));

        let incomplete = format!("before\n{SHELL_INTEGRATION_BEGIN}\n");
        assert!(strip_marked_shell_blocks(incomplete).is_err());
    }

    #[test]
    fn hook_cleanup_preserves_user_commands_and_is_idempotent() {
        let custom = serde_json::json!({"type":"command", "command":"echo user-hook"});
        let mut config = serde_json::json!({
            "permissions":{"allow":["Read"]},
            "hooks":{"Stop":[
                {"matcher":"*", "timeout":99, "hooks":[
                    {"type":"command", "command":"old/sTerm-hook.cmd claude-stop"},
                    custom.clone()
                ]},
                {"hooks":[{"type":"command", "command":"echo $STERM_TAB_ID"}]}
            ]}
        });
        assert!(remove_agent_hooks(&mut config).unwrap());
        assert_eq!(config["hooks"]["Stop"][0]["hooks"], serde_json::json!([custom]));
        assert_eq!(config["hooks"]["Stop"][0]["matcher"], "*");
        assert_eq!(config["hooks"]["Stop"][0]["timeout"], 99);
        assert_eq!(config["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(config["permissions"]["allow"][0], "Read");
        assert!(config["hooks"].get("SessionStart").is_none());
        assert!(!remove_agent_hooks(&mut config).unwrap());
    }

    #[test]
    fn hook_cleanup_handles_windows_posix_and_bare_configs() {
        for marker in ["sTerm-hook.cmd", "sterm-agent-events.py"] {
            for bare in [false, true] {
                let hooks = serde_json::json!({
                    "SessionStart":[{"type":"command", "command":format!("old/{marker} start")}],
                    "Stop":[{"hooks":[{"type":"command", "command":format!("old/{marker} stop")}]}]
                });
                let mut config = if bare { hooks } else { serde_json::json!({"hooks":hooks, "other":true}) };
                assert!(remove_agent_hooks(&mut config).unwrap());
                let target = if bare { &config } else { &config["hooks"] };
                assert_eq!(target["SessionStart"], serde_json::json!([]));
                assert_eq!(target["Stop"], serde_json::json!([]));
                assert!(!remove_agent_hooks(&mut config).unwrap());
                if !bare { assert_eq!(config["other"], true); }
            }
        }
    }

    #[test]
    fn invalid_hook_configs_are_never_overwritten() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        for original in ["{broken", r#"{"hooks":{"Stop":"wrong-type"}}"#, "[]"] {
            std::fs::write(&path, original).unwrap();
            assert!(cleanup_agent_hooks(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert!(!dir.path().join("settings.json.bak").exists());
        }
    }

    #[test]
    fn hook_cleanup_skips_missing_and_unmanaged_configs() {
        let dir = TempDir::new();
        let path = dir.path().join("missing").join("settings.json");
        cleanup_agent_hooks(&path).unwrap();
        assert!(!path.parent().unwrap().exists());
        let path = dir.path().join("settings.json");
        let original = "{ \"permissions\": { \"allow\": [\"Read\"] } }";
        std::fs::write(&path, original).unwrap();
        cleanup_agent_hooks(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!dir.path().join("settings.json.bak").exists());
    }

    #[test]
    fn hook_cleanup_backs_up_the_original_config() {
        let dir = TempDir::new();
        let path = dir.path().join("hooks.json");
        let original = r#"{"hooks":{"Stop":[{"hooks":[{"command":"old/sTerm-hook.cmd codex-stop"}]}]}}"#;
        std::fs::write(&path, original).unwrap();
        cleanup_agent_hooks(&path).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("hooks.json.bak")).unwrap(), original);
        cleanup_agent_hooks(&path).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("hooks.json.bak")).unwrap(), original);
    }
}
