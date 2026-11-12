use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::{diagnostics, events};

#[derive(Clone, Serialize)]
pub struct ShellInfo {
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Serialize, Clone)]
struct OutputPayload {
    id: u32,
    generation: u32,
    data: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExitPayload {
    id: u32,
    generation: u32,
    exit_code: Option<u32>,
}

struct Pty {
    generation: u32,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    shell_pid: Option<u32>,
}

pub struct PtyManager {
    ptys: Arc<Mutex<HashMap<u32, Arc<Pty>>>>,
}

/// Win32 COORD 使用有符号 16 位整数，转换前统一校验。
fn checked_size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(format!("无效的终端尺寸：{cols}x{rows}"));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn spawn(
        &self,
        app: AppHandle,
        id: u32,
        generation: u32,
        shell_id: &str,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let size = checked_size(cols, rows)?;
        // 同一标签重启时清理旧实例，事件通过 generation 隔离。
        self.kill_matching(id, None);

        let shell = resolve_shell(shell_id);
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| e.to_string())?;
        // 在启动 shell 前准备好管道，避免中途失败留下无人管理的子进程。
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let event_route = events::prepare(id, generation)?;
        let mut cmd = CommandBuilder::new(&shell.program);
        cmd.args(&shell.args);
        cmd.env("TERM", "xterm-256color");
        if shell.id == "cmd" { cmd.env("STERM_PATH_PERCENT", "%"); }
        cmd.env("STERM_TAB_ID", event_route);
        cmd.cwd(cwd.unwrap_or_else(|| {
            std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
        }));

        let mut child = pair.slave.spawn_command(cmd).map_err(|e| {
            events::retire(id, generation);
            e.to_string()
        })?;
        let shell_pid = child.process_id();
        drop(pair.slave);

        let pty = Arc::new(Pty {
            generation,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            shell_pid,
        });
        self.ptys.lock().unwrap().insert(id, pty);
        diagnostics::record(&format!(
            "spawn tab={id} generation={generation} shell={shell_id} pid={shell_pid:?} size={cols}x{rows}"
        ));

        // 独立读线程持续排空输出，ConPTY 关闭时也不能停止读取。
        let app_reader = app.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Err(error) => {
                        diagnostics::record(&format!(
                            "read ended tab={id} generation={generation}: {error}"
                        ));
                        break;
                    }
                    Ok(n) => {
                        let data = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        // 前端已关闭时仍排空管道，避免 ClosePseudoConsole 等待输出而阻塞。
                        let _ = app_reader.emit(
                            "pty-output",
                            OutputPayload {
                                id,
                                generation,
                                data,
                            },
                        );
                    }
                }
            }
        });

        let ptys = self.ptys.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            let exit_code = status.as_ref().ok().map(|s| s.exit_code());
            diagnostics::record(&format!(
                "exit tab={id} generation={generation} code={exit_code:?} status={status:?}"
            ));
            // 自然退出也回收宿主；旧实例只能移除自己，不能移除已重启的实例。
            let old = {
                let mut ptys = ptys.lock().unwrap();
                if ptys.get(&id).is_some_and(|p| p.generation == generation) {
                    ptys.remove(&id)
                } else {
                    None
                }
            };
            events::retire(id, generation);
            drop(old);
            let _ = app.emit(
                "pty-exit",
                ExitPayload {
                    id,
                    generation,
                    exit_code,
                },
            );
        });

        Ok(())
    }

    pub fn write(&self, id: u32, generation: u32, data: &str) -> Result<(), String> {
        let pty = self.get(id, generation)?;
        let mut writer = pty.writer.lock().unwrap();
        writer.write_all(data.as_bytes()).map_err(|error| {
            diagnostics::record(&format!(
                "write failed tab={id} generation={generation}: {error}"
            ));
            error.to_string()
        })
    }

    pub fn resize(&self, id: u32, generation: u32, cols: u16, rows: u16) -> Result<(), String> {
        let size = checked_size(cols, rows)?;
        let pty = self.get(id, generation)?;
        let master = pty.master.lock().unwrap();
        if master.get_size().map_err(|e| e.to_string())? == size {
            return Ok(());
        }
        master.resize(size).map_err(|error| {
            diagnostics::record(&format!(
                "resize failed tab={id} generation={generation} size={cols}x{rows}: {error}"
            ));
            error.to_string()
        })?;
        diagnostics::record(&format!(
            "resize tab={id} generation={generation} size={cols}x{rows}"
        ));
        Ok(())
    }

    pub fn kill(&self, id: u32, generation: u32) -> Result<(), String> {
        self.kill_matching(id, Some(generation));
        Ok(())
    }

    fn kill_matching(&self, id: u32, generation: Option<u32>) {
        let pty = {
            let mut ptys = self.ptys.lock().unwrap();
            if ptys
                .get(&id)
                .is_some_and(|p| generation.is_none_or(|g| p.generation == g))
            {
                ptys.remove(&id)
            } else {
                None
            }
        };
        // 不持有全局会话锁执行终止或释放宿主。
        if let Some(pty) = pty {
            events::retire(id, pty.generation);
            // portable-pty 0.8 的 Windows killer 会将成功误报为 Err，实际退出由 wait 确认。
            let _ = pty.killer.lock().unwrap().kill();
        }
    }

    pub fn kill_all(&self) {
        let all: Vec<(u32, Arc<Pty>)> = self.ptys.lock().unwrap().drain().collect();
        for (id, pty) in all {
            events::retire(id, pty.generation);
            let _ = pty.killer.lock().unwrap().kill();
        }
    }

    pub fn shell_pid(&self, id: u32, generation: u32) -> Option<u32> {
        self.ptys.lock().unwrap().get(&id)
            .filter(|p| p.generation == generation).and_then(|p| p.shell_pid)
    }

    fn get(&self, id: u32, generation: u32) -> Result<Arc<Pty>, String> {
        self.ptys
            .lock()
            .unwrap()
            .get(&id)
            .filter(|p| p.generation == generation)
            .cloned()
            .ok_or_else(|| format!("终端会话已结束或已重启：{id}/{generation}"))
    }
}

// ---------- shell 探测 ----------

pub fn detect_shells() -> Vec<ShellInfo> {
    let mut v = Vec::new();
    if let Some(p) = find_pwsh() {
        v.push(ShellInfo {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            program: p,
            args: vec!["-NoLogo".into()],
        });
    }
    v.push(ShellInfo {
        id: "powershell".into(),
        name: "Windows PowerShell".into(),
        program: "powershell.exe".into(),
        args: vec!["-NoLogo".into()],
    });
    v.push(ShellInfo {
        id: "cmd".into(),
        name: "命令提示符".into(),
        program: "cmd.exe".into(),
        args: vec![],
    });
    v
}

pub fn resolve_shell(shell_id: &str) -> ShellInfo {
    detect_shells()
        .into_iter()
        .find(|s| s.id == shell_id)
        .unwrap_or_else(|| ShellInfo {
            id: "powershell".into(),
            name: "Windows PowerShell".into(),
            program: "powershell.exe".into(),
            args: vec!["-NoLogo".into()],
        })
}

fn find_pwsh() -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("pwsh.exe");
            if p.is_file() {
                return p.to_str().map(String::from);
            }
        }
    }
    for p in [
        r"C:\Program Files\PowerShell\7\pwsh.exe",
        r"C:\Program Files\PowerShell\7-preview\pwsh.exe",
        r"C:\Program Files\PowerShell\6\pwsh.exe",
    ] {
        if Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}
