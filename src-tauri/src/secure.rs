use std::ffi::c_void;

#[repr(C)]
struct Blob {
    len: u32,
    bytes: *mut u8,
}

/// 仅在当前 Windows 用户上下文中保护本地凭证。
pub(crate) fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(input: *const Blob, description: *const u16, entropy: *const Blob, reserved: *mut c_void, prompt: *mut c_void, flags: u32, output: *mut Blob) -> i32;
        fn CryptUnprotectData(input: *const Blob, description: *mut *mut u16, entropy: *const Blob, reserved: *mut c_void, prompt: *mut c_void, flags: u32, output: *mut Blob) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    let input = Blob {
        len: u32::try_from(bytes.len()).map_err(|_| "凭证过长")?,
        bytes: bytes.as_ptr() as *mut u8,
    };
    let mut output = Blob { len: 0, bytes: std::ptr::null_mut() };
    let result = unsafe {
        if decrypt {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output)
        } else {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output)
        }
    };
    if result == 0 {
        return Err("Windows 凭证保护失败".into());
    }
    let result = unsafe { std::slice::from_raw_parts(output.bytes, output.len as usize).to_vec() };
    unsafe { LocalFree(output.bytes.cast()); }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_and_restores_bytes_for_current_user() {
        let secret = b"test-only-secret";
        let encrypted = protect(secret, false).unwrap();
        assert!(!encrypted.windows(secret.len()).any(|window| window == secret));
        assert_eq!(protect(&encrypted, true).unwrap(), secret);
    }
}
