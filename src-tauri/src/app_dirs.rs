//! 显式开发目录只对 debug 构建生效，安装版继续使用 Tauri 的应用目录。
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub fn directory(app: &AppHandle, kind: &str) -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        if let Some(root) = std::env::var_os("STERM_DEV_DATA_DIR") {
            let root = PathBuf::from(root);
            if !root.is_absolute() { return Err("开发数据目录必须使用绝对路径".into()); }
            return Ok(root.join(kind));
        }
    }
    match kind {
        "config" => app.path().app_config_dir(),
        "logs" => app.path().app_log_dir(),
        _ => app.path().app_local_data_dir(),
    }.map_err(|e| e.to_string())
}
