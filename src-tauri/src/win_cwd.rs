//! 通过读取目标进程 PEB 中的 ProcessParameters 获取其工作目录 / 命令行。
//! 我们自己 spawn 了 shell，持有足够权限；全程只读，无副作用。
//! 仅支持 x64 目标进程（pwsh/powershell/cmd/node 均为 x64）。

#![allow(non_snake_case)]

use core::ffi::c_void;

#[repr(transparent)]
#[derive(Clone, Copy)]
struct HANDLE(*mut c_void);

const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const PROCESS_VM_READ: u32 = 0x0010;

#[repr(C)]
#[derive(Default)]
struct ProcessBasicInformation {
    reserved1: usize,
    peb: usize,
    reserved2: [usize; 2],
    pid: usize,
    reserved3: usize,
}

extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> HANDLE;
    fn CloseHandle(h: HANDLE) -> i32;
    fn ReadProcessMemory(
        h: HANDLE,
        base: *const c_void,
        buf: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn NtQueryInformationProcess(
        h: HANDLE,
        cls: u32,
        info: *mut c_void,
        len: u32,
        ret_len: *mut usize,
    ) -> i32;
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// x64 偏移量
const PEB_PROCESS_PARAMETERS: usize = 0x20;
const RUPP_CURRENT_DIR: usize = 0x38; // UNICODE_STRING（len:u16, max:u16, pad, buf）

struct ProcContext {
    _guard: HandleGuard,
    params: usize,
}

/// 打开进程并定位 ProcessParameters 地址
fn process_params(pid: u32) -> Option<ProcContext> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if h.0.is_null() {
            return None;
        }
        let guard = HandleGuard(h);

        let mut pbi = ProcessBasicInformation::default();
        let mut ret = 0usize;
        if NtQueryInformationProcess(
            h,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut ret,
        ) != 0
            || pbi.peb == 0
        {
            return None;
        }

        let mut params = 0usize;
        if ReadProcessMemory(
            h,
            (pbi.peb + PEB_PROCESS_PARAMETERS) as *const c_void,
            &mut params as *mut _ as *mut c_void,
            8,
            std::ptr::null_mut(),
        ) == 0
            || params == 0
        {
            return None;
        }
        Some(ProcContext { _guard: guard, params })
    }
}

/// 读取 UNICODE_STRING（结构体地址）指向的宽字符串
fn read_us_string(ctx: &ProcContext, us_addr: usize) -> Option<String> {
    unsafe {
        let h = ctx._guard.0;
        let mut us = [0u8; 16];
        if ReadProcessMemory(
            h,
            us_addr as *const c_void,
            us.as_mut_ptr() as *mut c_void,
            16,
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        let len = u16::from_ne_bytes([us[0], us[1]]) as usize;
        let buf = usize::from_ne_bytes(us[8..16].try_into().ok()?);
        if len == 0 || len > 32768 || len % 2 != 0 || buf == 0 {
            return None;
        }
        let mut out = vec![0u16; len / 2];
        if ReadProcessMemory(
            h,
            buf as *const c_void,
            out.as_mut_ptr() as *mut c_void,
            len,
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        Some(String::from_utf16_lossy(&out))
    }
}

/// 读取进程当前工作目录，失败返回 None（调用方自行降级）
pub fn get_cwd(pid: u32) -> Option<String> {
    let ctx = process_params(pid)?;
    read_us_string(&ctx, ctx.params + RUPP_CURRENT_DIR)
}
