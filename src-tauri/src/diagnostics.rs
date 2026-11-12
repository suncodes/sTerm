//! 仅记录终端生命周期、尺寸与窗口诊断，不记录命令输入或终端输出。
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_FILE: OnceLock<Mutex<PathBuf>> = OnceLock::new();
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn init(dir: &Path) {
    if fs::create_dir_all(dir).is_ok() {
        let _ = LOG_FILE.set(Mutex::new(dir.join("terminal.log")));
    }
}

/// 与其他实例协调轮转，最多保留当前日志和一个历史文件。
fn append(path: &Path, line: &[u8], limit: u64) -> Result<(), String> {
    let _lock = crate::storage::lock(path)?;
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.to_string()),
    };
    if size > 0 && size.saturating_add(line.len() as u64) > limit {
        fs::rename(path, path.with_extension("log.1")).map_err(|e| e.to_string())?;
    }
    OpenOptions::new().create(true).append(true).open(path)
        .and_then(|mut file| file.write_all(line)).map_err(|e| e.to_string())
}

pub fn record(message: &str) {
    let Some(path) = LOG_FILE.get().and_then(|path| path.lock().ok()) else { return };
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let line = format!("[{millis}] {message}\n");
    let _ = append(&path, line.as_bytes(), MAX_LOG_BYTES);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    #[test]
    fn rotation_keeps_only_current_and_previous_log() {
        let dir = TempDir::new();
        let path = dir.path().join("terminal.log");
        for line in ["first\n", "second\n", "third\n"] {
            append(&path, line.as_bytes(), 8).unwrap();
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        assert_eq!(fs::read_to_string(dir.path().join("terminal.log.1")).unwrap(), "second\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }
}
