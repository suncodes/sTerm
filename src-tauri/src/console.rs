//! 从应用资源目录加载配套的 ConPTY 组件，使控制台版本随应用固定。
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

pub fn init(resource_dir: &Path) -> Result<(), String> {
    for name in ["conpty.dll", "OpenConsole.exe"] {
        if !resource_dir.join(name).is_file() {
            return Err(format!(
                "缺少控制台组件：{}",
                resource_dir.join(name).display()
            ));
        }
    }
    let path = resource_dir.join("conpty.dll");
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // 使用绝对路径，仅在组件所在目录及系统默认目录解析依赖。
    let module = unsafe {
        LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), 0x0000_0100 | 0x0000_1000)
    };
    if module.is_null() {
        return Err(format!(
            "加载 {} 失败：{}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    for name in [
        b"CreatePseudoConsole\0".as_slice(),
        b"ResizePseudoConsole\0",
        b"ClosePseudoConsole\0",
    ] {
        if unsafe { GetProcAddress(module, name.as_ptr()) }.is_null() {
            return Err(format!("控制台组件接口不完整：{}", path.display()));
        }
    }
    // 保留模块引用直到应用退出；portable-pty 随后的同名加载会复用该模块。
    crate::diagnostics::record(&format!("console loaded from {}", path.display()));
    Ok(())
}
