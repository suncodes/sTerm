use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::{agent_turn::{AgentEvent, AgentInput, AgentTurn}, diagnostics, events};

#[derive(Clone, Serialize)]
pub struct ShellInfo {
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OutputPayload {
    id: u32,
    generation: u32,
    data: String,
    input_revision: u64,
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
    input: Mutex<PtyInput>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    shell_pid: Option<u32>,
}

struct PtyInput {
    writer: Box<dyn Write + Send>,
    agent: AgentTurn,
    revision: Arc<AtomicU64>,
}

impl PtyInput {
    fn write(&mut self, data: &str, protocol_reply: bool) -> std::io::Result<()> {
        if !protocol_reply { self.revision.fetch_add(1, Ordering::Relaxed); }
        self.writer.write_all(data.as_bytes())
    }

    fn submit(&mut self, revision: u64, text: &str, paste: bool) -> Result<(), String> {
        if self.revision.load(Ordering::Relaxed) != revision {
            return Err("提交期间终端收到其他输入，请稍后重试".into());
        }
        let data = self.agent.take_input(text, paste)?;
        self.revision.fetch_add(1, Ordering::Relaxed);
        if write_agent_input(self.writer.as_mut(), data).is_err() {
            self.agent.reject_input();
            return Err("终端写入失败，请打开远程终端确认输入".into());
        }
        Ok(())
    }
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

    #[cfg(debug_assertions)]
    pub fn debug_size(&self, id: u32) -> Result<(u16,u16), String> {
        let pty = self.ptys.lock().unwrap().get(&id).cloned().ok_or("终端已退出")?;
        let size = pty.master.lock().unwrap().get_size().map_err(|e|e.to_string())?;
        Ok((size.cols,size.rows))
    }

    pub fn spawn_program(
        &self, app: AppHandle, id: u32, generation: u32, shell_id: &str,
        cwd: Option<String>, cols: u16, rows: u16,
        bootstrap: Option<crate::agent_runtime::shell::Bootstrap>,
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
        if let Some(bootstrap)=bootstrap{
            cmd.env("STERM_SHELL_BRIDGE",bootstrap.address);
            cmd.env("STERM_SHELL_TOKEN",bootstrap.token);
            if shell.id=="cmd"{
                let directory=bootstrap.script.parent().unwrap().join("cmd");
                std::fs::create_dir_all(&directory).map_err(|e|e.to_string())?;
                let pwsh=resolve_shell("pwsh");
                for agent in ["claude","codex"]{
                    let command=format!("@echo off\r\n\"{}\" -NoLogo -NoProfile -File \"{}\" {} %*\r\n",pwsh.program,bootstrap.script.display(),agent);
                    std::fs::write(directory.join(format!("{agent}.cmd")),command).map_err(|e|e.to_string())?;
                }
                let path=std::env::var("PATH").unwrap_or_default();
                cmd.env("PATH",format!("{};{path}",directory.display()));
            }else{
                let script=bootstrap.script.to_string_lossy().replace('\'',"''");
                cmd.args(["-NoExit","-Command",&format!(". '{script}'")]);
            }
        }
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

        let input_revision = Arc::new(AtomicU64::new(0));
        let pty = Arc::new(Pty {
            generation,
            input: Mutex::new(PtyInput { writer, agent: AgentTurn::default(), revision: input_revision.clone() }),
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            shell_pid,
        });
        self.ptys.lock().unwrap().insert(id, pty);
        app.state::<crate::feishu::State>().opened(id, generation);
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
                                input_revision: input_revision.load(Ordering::Relaxed),
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
            app.state::<crate::feishu::State>().closed(id, generation);
            app.state::<crate::agent_runtime::State>().retire(id, generation);
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

    pub fn has_generation(&self, id: u32, generation: u32) -> bool {
        self.ptys.lock().unwrap().get(&id).is_some_and(|p| p.generation == generation)
    }

    pub fn write(&self, id: u32, generation: u32, data: &str) -> Result<(), String> {
        self.write_input(id, generation, data, false)
    }

    pub fn write_input(&self, id: u32, generation: u32, data: &str, protocol_reply: bool) -> Result<(), String> {
        let pty = self.get(id, generation)?;
        let mut input = pty.input.lock().unwrap();
        input.write(data, protocol_reply).map_err(|error| {
            diagnostics::record(&format!(
                "write failed tab={id} generation={generation}: {error}"
            ));
            error.to_string()
        })
    }

    pub fn report_agent_event(&self, id: u32, generation: u32, agent: &str, event: AgentEvent) -> bool {
        let Ok(pty) = self.get(id, generation) else { return false };
        let mut input = pty.input.lock().unwrap();
        input.agent.report(agent, event)
    }

    pub fn input_revision(&self, id: u32, generation: u32) -> Result<u64, String> {
        let pty = self.get(id, generation)?;
        let revision = pty.input.lock().unwrap().revision.load(Ordering::Relaxed);
        Ok(revision)
    }

    pub fn submit_agent_text(&self, id: u32, generation: u32, revision: u64, text: &str, paste: bool) -> Result<(), String> {
        let pty = self.get(id, generation).map_err(|_| "原会话已失效")?;
        let mut input = pty.input.lock().unwrap();
        input.submit(revision, text, paste)
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

// 调用期间持有输入锁，正文与最后的提交回车之间不会混入其他输入。
fn write_agent_input(writer: &mut dyn Write, mut input: AgentInput) -> std::io::Result<()> {
    if input.submit_delay.is_zero() {
        input.data.push('\r');
        return writer.write_all(input.data.as_bytes());
    }
    writer.write_all(input.data.as_bytes())?;
    writer.flush()?;
    std::thread::sleep(input.submit_delay);
    writer.write_all(b"\r")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn ready_input(agent: &str) -> (PtyInput, SharedWriter) {
        let writer = SharedWriter::default();
        let mut state = AgentTurn::default();
        state.report(agent, AgentEvent::Prompt);
        assert!(state.report(agent, AgentEvent::Complete));
        (PtyInput { writer: Box::new(writer.clone()), agent: state, revision: Arc::new(AtomicU64::new(0)) }, writer)
    }

    #[test]
    fn local_editing_never_invalidates_a_ready_agent() {
        for agent in ["claude", "pi", "codex"] {
            for edits in [vec!["draft", "\x7f\x7f\x7f\x7f\x7f"], vec!["\x1b[D", "\x1b[C", "\x1b[H", "\x1b[F"], vec!["existing draft"]] {
                let (mut input, writer) = ready_input(agent);
                for edit in &edits { input.write(edit, false).unwrap(); }
                let prefix = writer.0.lock().unwrap().clone();
                let revision = input.revision.load(Ordering::Relaxed);
                input.submit(revision, "飞书续问", true).unwrap();
                let actual = writer.0.lock().unwrap().clone();
                assert_eq!(&actual[..prefix.len()], prefix);
                assert_eq!(&actual[prefix.len()..], "\x1b[200~飞书续问\x1b[201~\r".as_bytes(), "{agent}");
                assert!(input.submit(input.revision.load(Ordering::Relaxed), "重复", true).unwrap_err().contains("提交"));
            }
        }
    }

    #[test]
    fn concurrent_input_rejects_only_the_current_request() {
        let (mut input, writer) = ready_input("claude");
        let previous = input.revision.load(Ordering::Relaxed);
        input.write("local", false).unwrap();
        assert!(input.submit(previous, "飞书续问", true).unwrap_err().contains("提交期间"));
        assert_eq!(*writer.0.lock().unwrap(), b"local");
        let current = input.revision.load(Ordering::Relaxed);
        input.write("\x1b[I", true).unwrap();
        input.write("\x1b[1;1R", true).unwrap();
        assert_eq!(input.revision.load(Ordering::Relaxed), current);
        assert!(input.submit(current, "重新发送", true).is_ok());
    }

    #[test]
    fn submission_rechecks_the_current_agent_state() {
        let (mut input, writer) = ready_input("pi");
        input.agent.report("pi", AgentEvent::Prompt);
        assert!(input.submit(0, "继续", true).unwrap_err().contains("运行"));
        input.agent.report("pi", AgentEvent::Blocked);
        assert!(input.submit(0, "继续", true).unwrap_err().contains("等待"));
        assert!(writer.0.lock().unwrap().is_empty());
        input.agent.report("pi", AgentEvent::Ready);
        assert!(input.submit(0, "继续", true).is_ok());
    }

    #[test]
    fn concurrent_bot_submissions_write_exactly_one_prompt() {
        let (input, writer) = ready_input("claude");
        let input = Arc::new(Mutex::new(input));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2).map(|_| {
            let input = input.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                input.lock().unwrap().submit(0, "继续", true)
            })
        }).collect();
        barrier.wait();
        let accepted = handles.into_iter().map(|handle| handle.join().unwrap()).filter(Result::is_ok).count();
        assert_eq!(accepted, 1);
        assert_eq!(*writer.0.lock().unwrap(), "\x1b[200~继续\x1b[201~\r".as_bytes());
    }

    #[derive(Default)]
    struct RecordingWriter {
        writes: Vec<(Instant, Vec<u8>)>,
        fail_write: bool,
        fail_flush: bool,
        flushes: usize,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if self.fail_write { return Err(std::io::Error::other("write failed")); }
            self.writes.push((Instant::now(), data.to_vec()));
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            if self.fail_flush { Err(std::io::Error::other("flush failed")) } else { Ok(()) }
        }
    }

    fn completed_input(agent: &str) -> AgentInput {
        let mut turn = AgentTurn::default();
        turn.report(agent, AgentEvent::Prompt);
        assert!(turn.report(agent, AgentEvent::Complete));
        turn.take_input("第一段\n\n第三段", true).unwrap()
    }

    #[test]
    fn codex_writes_all_paragraphs_before_one_delayed_submit() {
        let mut writer = RecordingWriter::default();
        let input = completed_input("codex");
        let delay = input.submit_delay;
        write_agent_input(&mut writer, input).unwrap();
        assert_eq!(writer.writes.len(), 2);
        assert_eq!(writer.writes[0].1, "\x1b[200~第一段\x1b[201~\x1b\r\x1b[200~\x1b[201~\x1b\r\x1b[200~第三段\x1b[201~".as_bytes());
        assert_eq!(writer.writes[1].1, b"\r");
        assert!(writer.writes[1].0.duration_since(writer.writes[0].0) >= delay);
        assert_eq!(writer.flushes, 1);

        for agent in ["claude", "pi"] {
            let mut writer = RecordingWriter::default();
            write_agent_input(&mut writer, completed_input(agent)).unwrap();
            assert_eq!(writer.writes.len(), 1);
            assert_eq!(writer.writes[0].1, "\x1b[200~第一段\r\r第三段\x1b[201~\r".as_bytes());
            assert_eq!(writer.flushes, 0);
        }
    }

    #[test]
    fn failed_paste_does_not_send_the_submit_key() {
        let mut writer = RecordingWriter { fail_write: true, ..RecordingWriter::default() };
        assert!(write_agent_input(&mut writer, completed_input("codex")).is_err());
        assert!(writer.writes.is_empty());
        assert_eq!(writer.flushes, 0);

        let mut writer = RecordingWriter { fail_flush: true, ..RecordingWriter::default() };
        assert!(write_agent_input(&mut writer, completed_input("codex")).is_err());
        assert_eq!(writer.writes.len(), 1);
        assert_eq!(writer.flushes, 1);
        assert!(!writer.writes[0].1.ends_with(b"\r"));
    }
}
