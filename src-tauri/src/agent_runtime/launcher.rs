//! 仅准备 sTerm 自己启动的资源，不改写 CLI 用户配置和系统环境。
use super::{codex::Client, State};
use serde::Serialize;
use serde_json::json;
use std::{fs, os::windows::process::CommandExt, path::PathBuf, process::{Child, Command, Stdio}, sync::Arc, time::Duration};
use tauri::AppHandle;

const MOD: &str = include_str!("../../../integrations/claude-mod/hooks/register.js");

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch { pub runtime_id: String, pub program: String, pub args: Vec<String>, pub env: Vec<(String, String)> }

pub struct CodexServer { pub endpoint: String, pub token_env: String, pub token: String, pub client: Arc<Client>, pub process: Child, _job: crate::tunnel_process::Job }
impl Drop for CodexServer { fn drop(&mut self) { let _ = self.process.kill(); let _ = self.process.wait(); } }

fn data(app: &AppHandle) -> Result<PathBuf, String> {
    let path = crate::app_dirs::directory(app, "data")?.join("agent-runtime");
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

pub(super) fn executable(name: &str) -> Result<PathBuf, String> {
    // 测试只影响 debug 实例，不修改安装版的路径或真实配置。
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os(format!("STERM_DEV_{}_EXE", name.to_uppercase())) {
            let path = PathBuf::from(path);
            if path.is_file() { return Ok(path); }
            return Err(format!("{name} 测试程序不存在"));
        }
    }
    let dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    for dir in dirs {
        let direct = dir.join(format!("{name}.exe"));
        if direct.is_file() { return Ok(direct); }
        let module = if name == "claude" {
            dir.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe")
        } else {
            dir.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe")
        };
        if module.is_file() { return Ok(module); }
    }
    Err(format!("找不到 {name} 原生程序，请先安装支持的 CLI"))
}

async fn server(app: &AppHandle, state: &State, cwd: &str) -> Result<(String, String, String, Arc<Client>), String> {
    let mut current = state.codex.lock().await;
    if current.as_mut().is_some_and(|server| server.process.try_wait().ok().flatten().is_some()) { *current = None; }
    if current.is_none() {
        let directory = data(app)?.join("codex"); fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let socket = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let endpoint = format!("ws://127.0.0.1:{}", socket.local_addr().map_err(|e| e.to_string())?.port());
        drop(socket);
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        // 只把摘要传给进程，不写明文本地后端令牌。
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(token.as_bytes()));
        let job = crate::tunnel_process::Job::new()?;
        let mut process = Command::new(executable("codex")?)
            .args(["app-server", "--listen", &endpoint, "--ws-auth", "capability-token", "--ws-token-sha256", &digest])
            .current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .creation_flags(0x0800_0000).spawn().map_err(|e| e.to_string())?;
        if let Err(error) = job.assign(&process) { let _ = process.kill(); let _ = process.wait(); return Err(error); }
        let mut client = None;
        let mut last = String::new();
        for _ in 0..50 {
            if process.try_wait().map_err(|e| e.to_string())?.is_some() { break; }
            match Client::connect(&endpoint, &token, state.hub.clone()).await {
                Ok(connected) => { client = Some(connected); break; },
                Err(error) => last = error,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let Some(client) = client else { let _ = process.kill(); let _ = process.wait(); return Err(format!("Codex 本地后端未就绪：{last}")); };
        *current = Some(CodexServer { endpoint, token, token_env: format!("STERM_CODEX_{}", uuid::Uuid::new_v4().simple()), client, process, _job: job });
    }
    let server = current.as_ref().unwrap();
    Ok((server.endpoint.clone(), server.token_env.clone(), server.token.clone(), server.client.clone()))
}

pub async fn prepare_arguments(app: &AppHandle, state: &State, tab: u32, generation: u32, agent: &str, cwd: &str, resume: Option<String>, settings: serde_json::Value, native_args: Vec<String>) -> Result<Launch, String> {
    if resume.as_ref().is_some_and(|id| id.len() > 100 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) { return Err("无效的历史会话 ID".into()); }
    let program = executable(agent)?;
    let binary = program.clone();
    let version = tokio::task::spawn_blocking(move || Command::new(binary).arg("--version").creation_flags(0x0800_0000).output())
        .await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let version = String::from_utf8_lossy(&version.stdout);
    let supported = if agent == "claude" { "2.1.322" } else { "0.170.0" };
    if !version.split_whitespace().any(|part| part == supported) { return Err(format!("Agent 模式当前验证版本为 {agent} {supported}，检测到 {}", version.trim())); }
    if let Some(ref id) = resume {
        if state.hub.has_session(agent, id) { return Err("此会话已经在当前 sTerm 中运行，请切换到已有会话".into()); }
    }
    let (runtime, token) = state.hub.register(agent, tab, generation, cwd.into());
    let result: Result<Launch, String> = async {
        if agent == "claude" {
            let address = state.bridge().await?;
            let directory = data(app)?.join("claude").join(&runtime);
            fs::create_dir_all(directory.join(".claude-plugin")).map_err(|e| e.to_string())?;
            fs::create_dir_all(directory.join("hooks")).map_err(|e| e.to_string())?;
            fs::write(directory.join(".claude-plugin/plugin.json"), include_str!("../../../integrations/claude-mod/.claude-plugin/plugin.json")).map_err(|e| e.to_string())?;
            fs::write(directory.join("hooks/hooks.json"), include_str!("../../../integrations/claude-mod/hooks/hooks.json")).map_err(|e| e.to_string())?;
            let env_name = format!("STERM_CLAUDE_{}", uuid::Uuid::new_v4().simple());
            let config = json!({"url":format!("{address}/bridge/{runtime}"),"tokenEnv":env_name});
            fs::write(directory.join("hooks/register.js"), format!("const CONFIG = {config};\n{}", MOD.replace("__STERM_TOKEN_ENV__", &env_name))).map_err(|e| e.to_string())?;
            let mut args = vec!["--plugin-dir".into(), directory.to_string_lossy().into_owned()];
            if let Some(resume) = resume { args.extend(["--resume".into(), resume]); }
            args.extend(native_args);
            Ok(Launch { runtime_id: runtime.clone(), program: program.to_string_lossy().into_owned(), args, env: vec![(env_name, token)] })
        } else {
            let is_new = resume.is_none();
            let (endpoint, env_name, bearer, client) = server(app, state, cwd).await?;
            let mut request=settings;
            request["cwd"]=json!(cwd);
            let response = if let Some(resume) = resume {
                request["threadId"] = json!(resume); client.call("thread/resume", request).await?
            } else {
                client.call("thread/start", request).await?
            };
            let thread = response["thread"]["id"].as_str().ok_or("Codex 没有返回 threadId")?.to_string();
            // 新 thread 在持久化前不能由原生 TUI resume。设置名称使其落盘，避免空会话 bootstrap 失败。
            if is_new { client.call("thread/name/set", json!({"threadId":thread,"name":"sTerm Agent"})).await?; }
            client.bind(&runtime, &thread, &response).await?;
            Ok(Launch { runtime_id: runtime.clone(), program: program.to_string_lossy().into_owned(),
                args: {let mut args=vec!["resume".into(), thread, "--remote".into(), endpoint, "--remote-auth-token-env".into(), env_name.clone()];args.extend(native_args);args},
                env: vec![(env_name, bearer)] })
        }
    }.await;
    if result.is_err() { state.hub.remove(&runtime); }
    result
}
