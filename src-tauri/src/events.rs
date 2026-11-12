use std::{collections::HashMap, fs, io::{Read, Seek, SeekFrom}, path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Mutex, OnceLock}, time::Duration};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

static HUB: OnceLock<EventHub> = OnceLock::new();

struct EventHub {
    instance: String,
    dir: PathBuf,
    mailboxes: Mutex<HashMap<u32, Mailbox>>,
    stopped: AtomicBool,
}

struct Mailbox {
    tab_id: u32,
    generation: u32,
    file: PathBuf,
    offset: u64,
}

/// 保持旧版脚本格式兼容；STERM_TAB_ID 现在是实例/标签代次组成的相对路径。
#[cfg(test)]
const HOOK_SCRIPT: &str = "@echo off\r\nif \"%~1\"==\"\" exit /b 0\r\nif not defined STERM_TAB_ID exit /b 0\r\nif not exist \"%LOCALAPPDATA%\\sTerm\\events\" mkdir \"%LOCALAPPDATA%\\sTerm\\events\"\r\n>> \"%LOCALAPPDATA%\\sTerm\\events\\%STERM_TAB_ID%.event\" echo %~1\r\nexit /b 0\r\n";

pub fn init() -> Result<(), String> {
    let base = std::env::var("LOCALAPPDATA").map_err(|_| "缺少 LOCALAPPDATA".to_string())?;
    let root = PathBuf::from(base).join("sTerm");
    HUB.set(EventHub::new(root.join("events"))?)
        .map_err(|_| "事件信箱已初始化".to_string())
}

impl EventHub {
    fn new(root: PathBuf) -> Result<Self, String> {
        let instance = uuid::Uuid::new_v4().to_string();
        let dir = root.join(&instance);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(Self { instance, dir, mailboxes: Mutex::new(HashMap::new()), stopped: AtomicBool::new(false) })
    }

    fn prepare(&self, tab_id: u32, generation: u32) -> Result<String, String> {
        let name = format!("{tab_id}-{generation}");
        let file = self.dir.join(format!("{name}.event"));
        let mut mailboxes = self.mailboxes.lock().map_err(|e| e.to_string())?;
        if self.stopped.load(Ordering::Relaxed) { return Err("事件信箱已关闭".into()); }
        if mailboxes.contains_key(&tab_id) { return Err(format!("标签 {tab_id} 的事件信箱尚未回收")); }
        fs::OpenOptions::new().write(true).create_new(true).open(&file).map_err(|e| e.to_string())?;
        mailboxes.insert(tab_id, Mailbox { tab_id, generation, file, offset: 0 });
        Ok(format!("{}\\{name}", self.instance))
    }

    fn retire(&self, tab_id: u32, generation: u32) {
        let Ok(mut mailboxes) = self.mailboxes.lock() else { return };
        if mailboxes.get(&tab_id).is_some_and(|m| m.generation == generation) {
            if let Some(mailbox) = mailboxes.remove(&tab_id) {
                let _ = fs::remove_file(mailbox.file);
            }
        }
    }
}

/// shell 启动前创建独立信箱；原版脚本和 Pi 扩展也能使用该相对路径。
pub fn prepare(tab_id: u32, generation: u32) -> Result<String, String> {
    HUB.get().ok_or("事件信箱未初始化")?.prepare(tab_id, generation)
}

pub fn retire(tab_id: u32, generation: u32) {
    if let Some(hub) = HUB.get() { hub.retire(tab_id, generation); }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent {
    tab_id: u32,
    generation: u32,
    kind: String,
}

impl Mailbox {
    /// 只追加、按偏移消费完整行。读后不截断，追加中的半行留到下一轮。
    fn poll(&mut self, mut emit: impl FnMut(AgentEvent) -> Result<(), String>) -> Result<(), String> {
        let mut file = fs::File::open(&self.file).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(self.offset)).map_err(|e| e.to_string())?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| e.to_string())?;
        for line in content.split_inclusive('\n') {
            if !line.ends_with('\n') { break; }
            let kind = line.trim();
            if let Some((agent, event)) = kind.split_once('-') {
                if matches!(agent, "claude" | "codex" | "pi") && matches!(event, "start" | "prompt" | "stop" | "end") {
                    emit(AgentEvent { tab_id: self.tab_id, generation: self.generation, kind: kind.to_string() })?;
                }
            }
            // 成功分发后再推进；emit 失败时下次重试当前行。
            self.offset += line.len() as u64;
        }
        Ok(())
    }
}

pub fn spawn_poller(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(hub) = HUB.get() else { break };
        if hub.stopped.load(Ordering::Relaxed) { break; }
        let Ok(mut mailboxes) = hub.mailboxes.lock() else { break };
        for mailbox in mailboxes.values_mut() {
            if let Err(error) = mailbox.poll(|event| app.emit("agent-event", event).map_err(|e| e.to_string())) {
                crate::diagnostics::record(&format!("agent event read failed tab={} generation={}: {error}", mailbox.tab_id, mailbox.generation));
            }
        }
    });
}

pub fn shutdown() {
    let Some(hub) = HUB.get() else { return };
    hub.stopped.store(true, Ordering::Relaxed);
    if let Ok(mut mailboxes) = hub.mailboxes.lock() {
        for (_, mailbox) in mailboxes.drain() { let _ = fs::remove_file(mailbox.file); }
    }
    let _ = fs::remove_dir(&hub.dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;
    use std::io::Write;
    use std::os::windows::process::CommandExt;

    fn append(path: &std::path::Path, text: &str) {
        fs::OpenOptions::new().append(true).open(path).unwrap()
            .write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn poll_keeps_concurrent_appends_and_partial_lines() {
        let dir = TempDir::new();
        let path = dir.path().join("1.event");
        fs::write(&path, "codex-start\r\npi-pro").unwrap();
        let mut mailbox = Mailbox { tab_id: 1, generation: 7, file: path.clone(), offset: 0 };
        let mut received = Vec::new();
        mailbox.poll(|event| {
            assert_eq!(event.generation, 7);
            received.push(event.kind);
            append(&path, "mpt\ncodex-stop\n");
            Ok(())
        }).unwrap();
        assert_eq!(received, ["codex-start"]);
        mailbox.poll(|event| { received.push(event.kind); Ok(()) }).unwrap();
        assert_eq!(received, ["codex-start", "pi-prompt", "codex-stop"]);
        mailbox.poll(|_| panic!("重复投递")).unwrap();
        assert!(fs::read_to_string(&path).unwrap().starts_with("codex-start"));
    }

    #[test]
    fn failed_delivery_retries_only_unconsumed_lines() {
        let dir = TempDir::new();
        let path = dir.path().join("1.event");
        fs::write(&path, "codex-prompt\ncodex-stop\n").unwrap();
        let mut mailbox = Mailbox { tab_id: 1, generation: 1, file: path, offset: 0 };
        let mut received = Vec::new();
        assert!(mailbox.poll(|event| {
            if event.kind == "codex-stop" { return Err("分发失败".into()); }
            received.push(event.kind);
            Ok(())
        }).is_err());
        mailbox.poll(|event| { received.push(event.kind); Ok(()) }).unwrap();
        assert_eq!(received, ["codex-prompt", "codex-stop"]);
    }

    #[test]
    fn legacy_hook_routes_to_own_instance_and_generation() {
        let dir = TempDir::new();
        let root = dir.path().join("sTerm").join("events");
        let first = EventHub::new(root.clone()).unwrap();
        let second = EventHub::new(root.clone()).unwrap();
        let route = first.prepare(1, 1).unwrap();
        assert_ne!(route, second.prepare(1, 1).unwrap());
        let legacy = root.join("1.event");
        fs::write(&legacy, "pi-stop\n").unwrap();

        let script = dir.path().join("hook.cmd");
        fs::write(&script, HOOK_SCRIPT).unwrap();
        assert!(std::process::Command::new("cmd.exe")
            .args(["/D", "/C"]).arg(&script).arg("pi-prompt")
            .env("LOCALAPPDATA", dir.path()).env("STERM_TAB_ID", route)
            .creation_flags(0x0800_0000).status().unwrap().success());

        let mut received = Vec::new();
        first.mailboxes.lock().unwrap().get_mut(&1).unwrap()
            .poll(|e| { received.push(e.kind); Ok(()) }).unwrap();
        second.mailboxes.lock().unwrap().get_mut(&1).unwrap()
            .poll(|_| panic!("跨实例串台")).unwrap();
        assert_eq!(received, ["pi-prompt"]);
        assert_eq!(fs::read_to_string(legacy).unwrap(), "pi-stop\n");

        first.retire(1, 1);
        first.prepare(1, 2).unwrap();
        first.retire(1, 1);
        assert_eq!(first.mailboxes.lock().unwrap().get(&1).unwrap().generation, 2);
        assert!(second.mailboxes.lock().unwrap().get(&1).unwrap().file.exists());
    }
}
