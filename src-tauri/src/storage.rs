//! 配置文件读写：跨进程锁、同目录原子替换和上一版备份。
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// 文件句柄关闭时自动释放锁；调用方在整个读改写期间持有它。
pub fn lock(path: &Path) -> Result<File, String> {
    let parent = path.parent().ok_or("配置路径缺少父目录")?;
    fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(sibling(path, ".lock"))
        .map_err(|e| format!("打开 {} 的配置锁失败：{e}", path.display()))?;
    file.lock().map_err(|e| format!("锁定 {} 失败：{e}", path.display()))?;
    Ok(file)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(raw.trim_start_matches('\u{feff}'))
            .map(Some)
            .map_err(|e| format!("解析 {} 失败：{e}；原文件已保留，可检查同目录的 .bak 备份", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败：{e}；原文件未修改", path.display())),
    }
}

/// 不先删除目标文件。Windows 的 rename 使用替换语义，失败时保留原目标。
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = sibling(path, &format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 调用方持有配置锁并已校验旧内容，备份始终来自上一次完整配置。
pub fn write(path: &Path, bytes: &[u8], backup: bool) -> Result<(), String> {
    let result = (|| {
        if backup {
            match fs::read(path) {
                Ok(previous) => replace(&sibling(path, ".bak"), &previous)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(e),
            }
        }
        replace(path, bytes)
    })();
    result.map_err(|e| format!("保存 {} 失败：{e}；未清空原文件", path.display()))
}

pub fn write_if_changed(path: &Path, text: &str, backup: bool) -> Result<(), String> {
    let _lock = lock(path)?;
    match fs::read_to_string(path) {
        Ok(current) if current == text => return Ok(()),
        Ok(_) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    }
    write(path, text.as_bytes(), backup)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sterm-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path { &self.0 }
    }

    impl Drop for TempDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn replaces_config_and_keeps_previous_backup() {
        let dir = TempDir::new();
        let path = dir.path().join("配置.json");
        let _lock = lock(&path).unwrap();
        write(&path, br#"{"version":1}"#, true).unwrap();
        write(&path, br#"{"version":2}"#, true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), br#"{"version":2}"#);
        assert_eq!(fs::read(sibling(&path, ".bak")).unwrap(), br#"{"version":1}"#);
        assert_eq!(read_json::<serde_json::Value>(&path).unwrap().unwrap()["version"], 2);
    }
}
