use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs, io::{BufRead, BufReader, Cursor, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use crate::tunnel_process::{self, TunnelProcess};
use crate::secure::protect;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Provider { Cloudflare, Ngrok }

impl Provider {
    fn executable(&self) -> &'static str {
        match self { Self::Cloudflare => "cloudflared.exe", Self::Ngrok => "ngrok.exe" }
    }
    fn key(&self) -> &'static str {
        match self { Self::Cloudflare => "cloudflare", Self::Ngrok => "ngrok" }
    }
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub provider: Option<Provider>,
    pub running: bool,
    pub connected: bool,
    pub public_url: Option<String>,
    pub message: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedConfig {
    cloudflare_url: String,
}

#[derive(Default)]
struct Inner {
    child: Option<TunnelProcess>,
    generation: u64,
    failure: Option<String>,
    status: TunnelStatus,
}

#[derive(Default)]
pub struct TunnelState {
    inner: Mutex<Inner>,
    operation: tokio::sync::Mutex<()>,
}

impl Inner {
    fn reuse(&mut self, provider: &Provider, port: u16) -> Result<Option<TunnelStatus>, String> {
        let Some(process) = self.child.as_mut() else { return Ok(None); };
        if process.child.try_wait().map_err(|_| "无法查询现有公网组件状态，请先断开连接")?.is_some() {
            self.child.take();
            self.status = TunnelStatus::default();
            self.failure = None;
            return Ok(None);
        }
        if self.status.provider.as_ref() == Some(provider) && process.port == port {
            return Ok(Some(self.status.clone()));
        }
        Err("请先停止现有公网连接".into())
    }
}

impl TunnelState {
    pub fn stop(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(process) = inner.child.as_mut() { process.stop()?; }
        inner.generation += 1;
        inner.child.take();
        inner.status = TunnelStatus::default();
        inner.failure = None;
        Ok(())
    }
}

fn directory(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|error| error.to_string())?.join("remote-tools");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn public_origin(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "请输入有效的 HTTPS 地址")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() || url.path() != "/" || url.port().is_some() {
        return Err("公网地址必须是 HTTPS 主机地址，例如 https://sterm.example.com".into());
    }
    Ok(url.origin().ascii_serialization())
}

fn read_credential(dir: &Path, provider: &Provider) -> Result<String, String> {
    let bytes = fs::read(dir.join(format!("{}.credential", provider.key()))).map_err(|_| "请先保存此服务的连接凭证")?;
    String::from_utf8(protect(&bytes, true)?).map_err(|_| "凭证格式无效".into())
}

#[tauri::command]
pub fn tunnel_config(app: AppHandle) -> Result<Value, String> {
    let dir = directory(&app)?;
    let saved = crate::storage::read_json::<SavedConfig>(&dir.join("settings.json"))?.unwrap_or_default();
    Ok(json!({
        "cloudflareUrl": saved.cloudflare_url,
        "cloudflareReady": dir.join("cloudflare.credential").is_file(),
        "ngrokReady": dir.join("ngrok.credential").is_file(),
        "cloudflareInstalled": dir.join("cloudflared.exe").is_file(),
        "ngrokInstalled": dir.join("ngrok.exe").is_file(),
    }))
}

#[tauri::command]
pub fn tunnel_save(app: AppHandle, provider: Provider, credential: String, public_url: String) -> Result<(), String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.try_lock().map_err(|_| "公网配置操作进行中，请稍后重试")?;
    save_config(&app, provider, &credential, &public_url)
}

fn save_config(app: &AppHandle, provider: Provider, credential: &str, public_url: &str) -> Result<(), String> {
    let url = if provider == Provider::Cloudflare { Some(public_origin(public_url)?) } else { None };
    let dir = directory(&app)?;
    let token = credential.trim();
    if !token.is_empty() {
        if !(8..=16384).contains(&token.len()) || token.chars().any(char::is_whitespace) { return Err("连接凭证格式无效".into()); }
        crate::storage::write(&dir.join(format!("{}.credential", provider.key())), &protect(token.as_bytes(), false)?, false)?;
    } else { read_credential(&dir, &provider)?; }
    if let Some(url) = url {
        crate::storage::write(&dir.join("settings.json"), &serde_json::to_vec(&SavedConfig { cloudflare_url: url }).unwrap(), false)?;
    }
    Ok(())
}

fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().user_agent("sTerm-remote")
        .timeout(Duration::from_secs(180)).https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host = attempt.url().host_str().unwrap_or("");
            if attempt.previous().len() > 5 { return attempt.error("下载重定向过多"); }
            if attempt.url().scheme() == "https" && (host == "api.github.com" || host == "github.com" || host.ends_with(".githubusercontent.com")
                || host == "bin.ngrok.com" || host == "bin.equinox.io" || host.ends_with(".amazonaws.com")) { attempt.follow() }
            else { attempt.error("拒绝非官方来源的下载重定向") }
        })).build().map_err(|error| error.to_string())
}

async fn download(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = download_client()?.get(url).send().await.map_err(|_| "下载失败，请检查网络后重试")?
        .error_for_status().map_err(|_| "官方下载服务暂时不可用")?;
    let mut result = Vec::new();
    while let Some(bytes) = response.chunk().await.map_err(|_| "下载中断，请重试")? {
        if result.len() + bytes.len() > limit { return Err("下载文件超出大小限制".into()); }
        result.extend_from_slice(&bytes);
    }
    Ok(result)
}

fn digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn installed(dir: &Path, provider: &Provider) -> bool {
    let file = dir.join(provider.executable());
    match (fs::read(&file), fs::read_to_string(file.with_extension("sha256"))) {
        (Ok(bytes), Ok(expected)) => digest(&bytes) == expected.trim(),
        _ => false,
    }
}

#[tauri::command]
pub async fn tunnel_install(app: AppHandle, provider: Provider) -> Result<(), String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.lock().await;
    let dir = directory(&app)?;
    if installed(&dir, &provider) { return Ok(()); }
    if state.inner.lock().unwrap().status.running { return Err("请先停止公网连接再安装组件".into()); }
    let bytes = match provider {
        Provider::Cloudflare => {
            let release: Value = download_client()?.get("https://api.github.com/repos/cloudflare/cloudflared/releases/latest")
                .send().await.map_err(|_| "无法查询 cloudflared 官方版本")?.error_for_status().map_err(|_| "官方版本查询失败")?
                .json().await.map_err(|_| "官方版本信息格式错误")?;
            let asset = release["assets"].as_array().and_then(|assets| assets.iter().find(|asset| asset["name"] == "cloudflared-windows-amd64.exe")).ok_or("当前版本没有 Windows x64 组件")?;
            let expected = asset["digest"].as_str().and_then(|value| value.strip_prefix("sha256:")).ok_or("官方发布缺少校验信息，请稍后重试")?;
            let url = asset["browser_download_url"].as_str().filter(|url| url.starts_with("https://github.com/cloudflare/cloudflared/releases/download/")).ok_or("组件来源无效")?;
            let bytes = download(url, 100 * 1024 * 1024).await?;
            if digest(&bytes) != expected { return Err("cloudflared 官方 SHA-256 校验失败".into()); }
            bytes
        }
        Provider::Ngrok => {
            let archive = download("https://bin.ngrok.com/c/bNyj1mQVY4c/ngrok-v3-stable-windows-amd64.zip", 100 * 1024 * 1024).await?;
            let mut archive = zip::ZipArchive::new(Cursor::new(archive)).map_err(|_| "ngrok 下载文件无效")?;
            let mut executable = archive.by_name("ngrok.exe").map_err(|_| "下载包缺少 ngrok.exe")?;
            if executable.size() > 120 * 1024 * 1024 { return Err("ngrok 组件超出大小限制".into()); }
            let mut bytes = Vec::new();
            executable.read_to_end(&mut bytes).map_err(|_| "解压 ngrok 失败")?;
            bytes
        }
    };
    if bytes.get(..2) != Some(b"MZ") { return Err("下载的组件不是有效的 Windows 程序".into()); }
    let file = dir.join(provider.executable());
    crate::storage::write(&file, &bytes, false)?;
    crate::storage::write(&file.with_extension("sha256"), digest(&bytes).as_bytes(), false)?;
    Ok(())
}

fn ngrok_failure(line: &str) -> Option<String> {
    let (_, suffix) = line.split_once("ERR_NGROK_")?;
    let number: String = suffix.chars().take_while(char::is_ascii_digit).take(12).collect();
    if number.is_empty() { return None; }
    Some(if number == "108" {
        "ngrok 在线 agent 会话额度已满（ERR_NGROK_108）。请清理遗留连接；其他设备的会话可在 dashboard.ngrok.com/agents 管理。".into()
    } else {
        format!("ngrok 连接失败（ERR_NGROK_{number}），请检查凭证、账号额度及网络。")
    })
}

fn observe_line(app: &AppHandle, generation: u64, line: &str) {
    let state = app.state::<TunnelState>();
    let mut inner = state.inner.lock().unwrap();
    if inner.generation != generation { return; }
    let is_ngrok = inner.status.provider == Some(Provider::Ngrok);
    if let Some(message) = is_ngrok.then(|| ngrok_failure(line)).flatten() {
        inner.status.connected = false;
        inner.status.public_url = None;
        inner.status.message = message.clone();
        inner.failure = Some(message);
        if let Some(hub) = app.state::<crate::remote::RemoteState>().current() { hub.set_public_origin(None); }
        return;
    }
    if let Ok(value) = serde_json::from_str::<Value>(line) {
        if is_ngrok {
            if let Some(url) = value["url"].as_str().and_then(|value| public_origin(value).ok()) {
                inner.status.public_url = Some(url);
                inner.status.connected = true;
                inner.status.message = "公网已连接".into();
                inner.failure = None;
            }
        } else if value["message"].as_str().is_some_and(|message| message.contains("Registered tunnel connection")) {
            inner.status.connected = true;
            inner.status.message = "公网已连接；请确认域名路由已配置".into();
        }
    }
    if !is_ngrok && line.contains("Registered tunnel connection") { inner.status.connected = true; }
    if inner.status.connected {
        if let Some(hub) = app.state::<crate::remote::RemoteState>().current() {
            hub.set_public_origin(inner.status.public_url.clone());
        }
    }
}

#[tauri::command]
pub fn tunnel_start(app: AppHandle, provider: Provider) -> Result<TunnelStatus, String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.try_lock().map_err(|_| "公网组件操作进行中，请稍后重试")?;
    let hub = app.state::<crate::remote::RemoteState>().current().ok_or("请先开启远程服务")?;
    let port = crate::remote::remote_port(&hub);
    let mut inner = state.inner.lock().unwrap();
    if let Some(status) = inner.reuse(&provider, port)? { return Ok(status); }
    hub.set_public_origin(None);
    let dir = directory(&app)?;
    let lease = tunnel_process::lease(&dir)?;
    tunnel_process::cleanup_orphans(&dir)?;
    if !installed(&dir, &provider) { return Err("请先下载或重新校验公网组件".into()); }
    let credential = read_credential(&dir, &provider)?;
    let mut command = Command::new(dir.join(provider.executable()));
    command.creation_flags(0x0800_0000).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let public_url = if provider == Provider::Cloudflare {
        let saved = crate::storage::read_json::<SavedConfig>(&dir.join("settings.json"))?.ok_or("请先保存 Cloudflare 域名")?;
        let url = public_origin(&saved.cloudflare_url)?;
        command.args(["tunnel", "--no-autoupdate", "--loglevel", "info", "--output", "json", "run"])
            .env("TUNNEL_TOKEN", &credential);
        Some(url)
    } else {
        let config = dir.join("ngrok.yml");
        crate::storage::write(&config, b"version: 3\nagent:\n  web_addr: false\n  remote_management: true\n  update_check: false\n", false)?;
        command.args(["http", &format!("http://127.0.0.1:{port}"), "--log", "stdout", "--log-format", "json", "--inspect=false", "--config"])
            .arg(config).env("NGROK_AUTHTOKEN", &credential);
        None
    };
    let mut process = TunnelProcess::spawn(&mut command, lease, port)?;
    inner.generation += 1;
    let generation = inner.generation;
    for stream in [process.child.stdout.take().map(|stream| Box::new(stream) as Box<dyn Read + Send>), process.child.stderr.take().map(|stream| Box::new(stream) as Box<dyn Read + Send>)].into_iter().flatten() {
        let reader_app = app.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if line.len() < 64 * 1024 { observe_line(&reader_app, generation, &line); }
            }
        });
    }
    inner.child = Some(process);
    inner.failure = None;
    inner.status = TunnelStatus { provider: Some(provider), running: true, connected: false, public_url, message: "正在连接公网…".into() };
    let status = inner.status.clone();
    drop(inner);
    let monitor_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        let mut connected_once = false;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let state = monitor_app.state::<TunnelState>();
            let mut inner = state.inner.lock().unwrap();
            if inner.generation != generation { break; }
            let ended = inner.child.as_mut().is_some_and(|process| process.child.try_wait().is_ok_and(|status| status.is_some()));
            connected_once |= inner.status.connected;
            let timed_out = !connected_once && started.elapsed() >= Duration::from_secs(45);
            let rejected = inner.failure.is_some();
            if !ended && !timed_out && !rejected { continue; }
            if !ended {
                if let Some(process) = inner.child.as_mut() {
                    if let Err(error) = process.stop() { inner.status.message = error; continue; }
                }
            }
            inner.child.take();
            inner.status.running = false;
            inner.status.connected = false;
            inner.status.public_url = None;
            if let Some(message) = inner.failure.take() {
                inner.status.message = message;
            } else {
                inner.status.message = if timed_out {
                    "公网连接超时，已停止组件。请检查网络及账号额度后重试。".into()
                } else { "公网连接已退出，请检查凭证、账号额度及网络后重试".into() };
            }
            if let Some(hub) = monitor_app.state::<crate::remote::RemoteState>().current() { hub.set_public_origin(None); }
            break;
        }
    });
    Ok(status)
}

#[tauri::command]
pub fn tunnel_status(state: tauri::State<TunnelState>) -> TunnelStatus { state.inner.lock().unwrap().status.clone() }

#[tauri::command]
pub fn tunnel_stop(app: AppHandle) -> Result<(), String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.try_lock().map_err(|_| "公网组件操作进行中，请稍后重试")?;
    state.stop()?;
    if let Some(hub) = app.state::<crate::remote::RemoteState>().current() { hub.set_public_origin(None); }
    let dir = directory(&app)?;
    let _lease = tunnel_process::lease(&dir)?;
    tunnel_process::cleanup_orphans(&dir)?;
    Ok(())
}

#[tauri::command]
pub fn tunnel_cleanup(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.try_lock().map_err(|_| "公网组件操作进行中，请稍后重试")?;
    let dir = directory(&app)?;
    let _lease = tunnel_process::lease(&dir)?;
    tunnel_process::cleanup_orphans(&dir)
}

#[tauri::command]
pub async fn tunnel_cloudflare_setup(app: AppHandle, api_token: String, hostname: String) -> Result<String, String> {
    let state = app.state::<TunnelState>();
    let _operation = state.operation.try_lock().map_err(|_| "公网配置操作进行中，请稍后重试")?;
    if state.inner.lock().unwrap().status.running { return Err("请先停止公网连接再创建隧道".into()); }
    let origin = public_origin(&format!("https://{}", hostname.trim()))?;
    let hostname = reqwest::Url::parse(&origin).unwrap().host_str().unwrap().to_owned();
    let hub = app.state::<crate::remote::RemoteState>().current().ok_or("请先开启远程服务")?;
    let port = crate::remote::remote_port(&hub);
    let client = reqwest::Client::builder().https_only(true).timeout(Duration::from_secs(20)).build().map_err(|_| "创建管理连接失败")?;
    async fn api(client: &reqwest::Client, token: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut request = client.request(method, format!("https://api.cloudflare.com/client/v4/{path}")).bearer_auth(token);
        if let Some(body) = body { request = request.json(&body); }
        let response = request.send().await.map_err(|_| "Cloudflare API 连接失败")?;
        let response: Value = response.json().await.map_err(|_| "Cloudflare 返回格式错误")?;
        if response["success"] != true { return Err("Cloudflare 操作失败，请检查 API Token 权限、域名状态和账号限制".into()); }
        Ok(response["result"].clone())
    }
    let zones = api(&client, &api_token, reqwest::Method::GET, "zones?status=active&per_page=50", None).await?;
    let zone = zones.as_array().and_then(|zones| zones.iter().filter(|zone| zone["name"].as_str().is_some_and(|name| hostname.ends_with(&format!(".{name}"))))
        .max_by_key(|zone| zone["name"].as_str().unwrap_or("").len())).ok_or("未找到此主机名对应的已激活域名；请先完成 Cloudflare DNS 接入")?;
    let account = zone["account"]["id"].as_str().ok_or("域名缺少账号信息")?;
    let zone_id = zone["id"].as_str().ok_or("域名缺少标识")?;
    let existing = api(&client, &api_token, reqwest::Method::GET, &format!("zones/{zone_id}/dns_records?name={hostname}"), None).await?;
    if existing.as_array().is_none_or(|records| !records.is_empty()) { return Err("该主机名已有 DNS 记录，请使用新的专用子域名；未覆盖原记录".into()); }
    let tunnel = api(&client, &api_token, reqwest::Method::POST, &format!("accounts/{account}/cfd_tunnel"), Some(json!({"name": format!("sterm-{}", uuid::Uuid::new_v4().simple()), "config_src": "cloudflare"}))).await?;
    let id = tunnel["id"].as_str().ok_or("隧道创建结果缺少标识")?;
    let token = if let Some(token) = tunnel["token"].as_str() { token.to_owned() }
        else { api(&client, &api_token, reqwest::Method::GET, &format!("accounts/{account}/cfd_tunnel/{id}/token"), None).await?.as_str().ok_or("无法获取运行凭证")?.to_owned() };
    save_config(&app, Provider::Cloudflare, &token, &origin)?;
    api(&client, &api_token, reqwest::Method::PUT, &format!("accounts/{account}/cfd_tunnel/{id}/configurations"), Some(json!({"config": {"ingress": [
        {"hostname": hostname, "service": format!("http://127.0.0.1:{port}")}, {"service": "http_status:404"}
    ]}}))).await.map_err(|error| format!("{error}；隧道已创建并保存，可在 Cloudflare 控制台补完路由"))?;
    api(&client, &api_token, reqwest::Method::POST, &format!("zones/{zone_id}/dns_records"), Some(json!({"type": "CNAME", "name": hostname, "content": format!("{id}.cfargotunnel.com"), "proxied": true}))).await
        .map_err(|error| format!("{error}；隧道已保存，请在控制台补完 DNS"))?;
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_public_origins_without_credentials_or_paths() {
        assert_eq!(public_origin("https://sterm.example.com/").unwrap(), "https://sterm.example.com");
        for invalid in ["http://example.com", "https://a:b@example.com", "https://example.com/path", "https://example.com/#token", "https://example.com:123"] {
            assert!(public_origin(invalid).is_err());
        }
    }

    #[test]
    fn repeated_starts_reuse_the_same_live_agent_and_stop_releases_it() {
        use crate::{storage::tests::TempDir, tunnel_process::tests::{ping_executable, sleeper}};
        let dir = TempDir::new();
        let process = TunnelProcess::spawn(&mut sleeper(ping_executable()), tunnel_process::lease(dir.path()).unwrap(), 8765).unwrap();
        let pid = process.child.id();
        let state = TunnelState::default();
        {
            let mut inner = state.inner.lock().unwrap();
            inner.child = Some(process);
            inner.status = TunnelStatus { provider: Some(Provider::Ngrok), running: true, ..TunnelStatus::default() };
            for _ in 0..10 {
                assert!(inner.reuse(&Provider::Ngrok, 8765).unwrap().unwrap().running);
                assert_eq!(inner.child.as_ref().unwrap().child.id(), pid);
            }
            assert!(inner.reuse(&Provider::Cloudflare, 8765).is_err());
            assert!(inner.reuse(&Provider::Ngrok, 8766).is_err());
        }
        state.stop().unwrap();
        state.stop().unwrap();
        assert!(state.inner.lock().unwrap().child.is_none());
        assert!(tunnel_process::lease(dir.path()).is_ok());
    }

    #[test]
    fn ngrok_errors_report_limits_without_echoing_credentials() {
        let failure = ngrok_failure(r#"{"err":"authentication failed: ERR_NGROK_108 secret-not-for-display"}"#).unwrap();
        assert!(failure.contains("会话额度已满"));
        assert!(!failure.contains("secret-not-for-display"));
        assert!(ngrok_failure("ERR_NGROK_3200").unwrap().contains("ERR_NGROK_3200"));
        assert!(ngrok_failure(r#"{"msg":"started tunnel","url":"https://example.ngrok-free.app"}"#).is_none());
    }

    #[test]
    fn credentials_are_protected_for_current_windows_user() {
        let secret = b"test-only-ngrok-credential";
        let encrypted = protect(secret, false).unwrap();
        assert!(!encrypted.windows(secret.len()).any(|window| window == secret));
        assert_eq!(protect(&encrypted, true).unwrap(), secret);
    }
}
