use std::{
    fs::{self, File, OpenOptions},
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    process::{Child, Command},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject, JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE},
        SystemInformation::GetSystemTimeAsFileTime,
        Threading::{GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE},
    },
};

fn own_handle(raw: HANDLE) -> Option<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE { None }
    else { Some(unsafe { OwnedHandle::from_raw_handle(raw) }) }
}

fn process_error(message: &str) -> String { format!("{message}：{}", io::Error::last_os_error()) }

pub fn lease(dir: &Path) -> Result<File, String> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(dir.join("agent.lock")).map_err(|_| "无法打开公网组件运行锁")?;
    file.try_lock().map_err(|_| "另一个 sTerm 实例正在使用此版本的公网组件，请先在该实例断开连接")?;
    Ok(file)
}

struct Job(OwnedHandle);

impl Job {
    fn new() -> Result<Self, String> {
        let handle = own_handle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
            .ok_or_else(|| process_error("无法创建公网进程托管"))?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe { SetInformationJobObject(handle.as_raw_handle(), JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) } == 0 {
            return Err(process_error("无法设置公网进程退出清理"));
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(process_error("无法托管公网组件，已取消启动"));
        }
        Ok(())
    }
}

pub struct TunnelProcess {
    pub child: Child,
    pub port: u16,
    job: Job,
    _lease: File,
}

impl TunnelProcess {
    pub fn spawn(command: &mut Command, lease: File, port: u16) -> Result<Self, String> {
        let job = Job::new()?;
        let mut child = command.spawn().map_err(|_| "启动隧道组件失败，请检查组件是否被系统拦截")?;
        if let Err(error) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Self { child, port, job, _lease: lease })
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.child.try_wait().map_err(|_| "无法查询公网组件退出状态")?.is_some() { return Ok(()); }
        if unsafe { TerminateJobObject(self.job.0.as_raw_handle(), 0) } == 0 {
            return Err(process_error("停止公网组件失败，请重试"));
        }
        if unsafe { WaitForSingleObject(self.child.as_raw_handle(), 5000) } != WAIT_OBJECT_0 {
            return Err("公网组件尚未退出，请稍后再次断开连接".into());
        }
        self.child.wait().map_err(|_| "无法确认公网组件已退出")?;
        Ok(())
    }
}

impl Drop for TunnelProcess {
    fn drop(&mut self) { let _ = self.stop(); }
}

fn timestamp(time: FILETIME) -> u64 { (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime) }

fn created_at(handle: &OwnedHandle) -> Option<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle.as_raw_handle(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 { None }
    else { Some(timestamp(created)) }
}

fn image_path(handle: &OwnedHandle) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.as_raw_handle(), 0, buffer.as_mut_ptr(), &mut length) } == 0 { return None; }
    use std::os::windows::ffi::OsStringExt;
    Some(std::ffi::OsString::from_wide(&buffer[..length as usize]).into())
}

fn orphaned(parent_pid: u32, child_created: u64) -> bool {
    let Some(parent) = own_handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, parent_pid) }) else {
        return io::Error::last_os_error().raw_os_error() == Some(87);
    };
    (unsafe { WaitForSingleObject(parent.as_raw_handle(), 0) == WAIT_OBJECT_0 })
        || created_at(&parent).is_some_and(|created| created > child_created)
}

/// 仅清理当前组件目录中父进程已退出的进程；句柄和创建时间用于防止 PID 复用误判。
pub fn cleanup_orphans(dir: &Path) -> Result<usize, String> {
    let executables: Vec<PathBuf> = ["ngrok.exe", "cloudflared.exe"].into_iter()
        .filter_map(|name| fs::canonicalize(dir.join(name)).ok()).collect();
    if executables.is_empty() { return Ok(0); }
    let mut observed_at = FILETIME::default();
    unsafe { GetSystemTimeAsFileTime(&mut observed_at); }
    let snapshot = own_handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        .ok_or_else(|| process_error("无法检查遗留公网进程"))?;
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    let mut count = 0;
    while available != 0 {
        let length = entry.szExeFile.iter().position(|value| *value == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if name.eq_ignore_ascii_case("ngrok.exe") || name.eq_ignore_ascii_case("cloudflared.exe") {
            if let Some(process) = own_handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, entry.th32ProcessID) }) {
                let created = created_at(&process);
                let path = image_path(&process).and_then(|path| fs::canonicalize(path).ok());
                if path.as_ref().is_some_and(|path| executables.contains(path))
                    && created.is_some_and(|created| created <= timestamp(observed_at) && orphaned(entry.th32ParentProcessID, created))
                    && unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
                    if unsafe { TerminateProcess(process.as_raw_handle(), 0) } == 0 {
                        return Err(process_error("清理遗留公网进程失败，请检查当前用户权限"));
                    }
                    if unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) } != WAIT_OBJECT_0 {
                        return Err("遗留公网进程尚未退出，请稍后重试清理".into());
                    }
                    count += 1;
                }
            }
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    Ok(count)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::tests::TempDir;
    use std::{os::windows::process::CommandExt, process::Stdio, time::{Duration, Instant}};

    pub(crate) fn sleeper(executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        command.args(["-n", "120", "127.0.0.1"]).creation_flags(0x0800_0000)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command
    }

    pub(crate) fn ping_executable() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/ping.exe")
    }

    #[test]
    fn only_one_agent_can_lease_a_directory() {
        let dir = TempDir::new();
        let first = lease(dir.path()).unwrap();
        assert!(lease(dir.path()).is_err());
        drop(first);
        assert!(lease(dir.path()).is_ok());
    }

    #[test]
    fn stop_waits_for_exit_and_drop_releases_the_lease() {
        let dir = TempDir::new();
        let mut process = TunnelProcess::spawn(&mut sleeper(ping_executable()), lease(dir.path()).unwrap(), 8765).unwrap();
        assert!(process.child.try_wait().unwrap().is_none());
        process.stop().unwrap();
        assert!(process.child.try_wait().unwrap().is_some());
        process.stop().unwrap();
        drop(process);
        assert!(lease(dir.path()).is_ok());
    }

    #[test]
    #[ignore = "由进程生命周期测试作为独立父进程调用"]
    fn child_process_helper() {
        let dir = PathBuf::from(std::env::var_os("STERM_TUNNEL_TEST_DIR").unwrap());
        let mut command = sleeper(dir.join("ngrok.exe"));
        if std::env::var("STERM_TUNNEL_TEST_MODE").unwrap() == "managed" {
            let process = TunnelProcess::spawn(&mut command, lease(&dir).unwrap(), 8765).unwrap();
            fs::write(dir.join("child.pid"), process.child.id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(30));
        } else {
            let mut child = command.spawn().unwrap();
            fs::write(dir.join("child.pid"), child.id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(30));
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    struct Probe {
        parent: Child,
        child: Option<OwnedHandle>,
    }

    impl Probe {
        fn spawn(dir: &Path, mode: &str) -> Self {
            fs::copy(ping_executable(), dir.join("ngrok.exe")).unwrap();
            let parent = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tunnel_process::tests::child_process_helper", "--ignored", "--nocapture"])
                .env("STERM_TUNNEL_TEST_DIR", dir).env("STERM_TUNNEL_TEST_MODE", mode)
                .creation_flags(0x0800_0000).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
                .spawn().unwrap();
            let mut probe = Self { parent, child: None };
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(pid) = fs::read_to_string(dir.join("child.pid")) {
                    probe.child = own_handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid.parse().unwrap()) });
                    assert!(probe.child.is_some());
                    return probe;
                }
                assert!(Instant::now() < deadline, "子进程未在期限内启动");
                assert!(probe.parent.try_wait().unwrap().is_none(), "测试父进程提前退出");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn terminate_parent(&mut self) {
            self.parent.kill().unwrap();
            self.parent.wait().unwrap();
        }

        fn wait_child(&self, timeout: u32) -> u32 {
            unsafe { WaitForSingleObject(self.child.as_ref().unwrap().as_raw_handle(), timeout) }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.parent.kill();
            let _ = self.parent.wait();
            if let Some(child) = &self.child {
                unsafe {
                    TerminateProcess(child.as_raw_handle(), 0);
                    WaitForSingleObject(child.as_raw_handle(), 5000);
                }
            }
        }
    }

    #[test]
    fn killing_the_parent_also_terminates_the_managed_agent() {
        let dir = TempDir::new();
        let mut probe = Probe::spawn(dir.path(), "managed");
        assert_ne!(probe.wait_child(0), WAIT_OBJECT_0);
        probe.terminate_parent();
        assert_eq!(probe.wait_child(5000), WAIT_OBJECT_0);
        assert!(lease(dir.path()).is_ok());
    }

    #[test]
    fn cleanup_terminates_only_orphans_in_the_requested_directory() {
        let dir = TempDir::new();
        let foreign_dir = TempDir::new();
        let mut probe = Probe::spawn(dir.path(), "orphan");
        assert_eq!(cleanup_orphans(dir.path()).unwrap(), 0);
        assert_ne!(probe.wait_child(0), WAIT_OBJECT_0);
        let mut foreign = Probe::spawn(foreign_dir.path(), "orphan");
        probe.terminate_parent();
        foreign.terminate_parent();
        assert_eq!(cleanup_orphans(dir.path()).unwrap(), 1);
        assert_eq!(probe.wait_child(0), WAIT_OBJECT_0);
        assert_ne!(foreign.wait_child(0), WAIT_OBJECT_0);
        assert_eq!(cleanup_orphans(dir.path()).unwrap(), 0);
        assert_eq!(cleanup_orphans(foreign_dir.path()).unwrap(), 1);
    }
}
