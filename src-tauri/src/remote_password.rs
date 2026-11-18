use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr, num::NonZeroU32, path::Path, time::{Duration, Instant}};

const ITERATIONS: u32 = 600_000;
const IP_WINDOW: Duration = Duration::from_secs(5 * 60);
const GLOBAL_WINDOW: Duration = Duration::from_secs(10 * 60);

pub fn valid_password(password: &str) -> bool {
    password.len() == 6 && password.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PasswordVerifier {
    salt: [u8; 16],
    hash: [u8; 32],
}

impl PasswordVerifier {
    pub fn new(password: &str) -> Result<Self, String> {
        if !valid_password(password) { return Err("访问码必须为 6 位数字".into()); }
        let mut verifier = Self { salt: *uuid::Uuid::new_v4().as_bytes(), hash: [0; 32] };
        pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).unwrap(), &verifier.salt, password.as_bytes(), &mut verifier.hash);
        Ok(verifier)
    }

    pub fn verify(&self, password: &str) -> bool {
        valid_password(password) && pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).unwrap(), &self.salt, password.as_bytes(), &self.hash).is_ok()
    }
}

#[derive(Deserialize, Serialize)]
pub struct SavedPassword {
    version: u32,
    pub verifier: Option<PasswordVerifier>,
    #[serde(default)]
    password: Option<String>,
}

impl SavedPassword {
    pub fn can_reveal(&self) -> bool { self.verifier.is_some() && self.password.is_some() }

    pub fn reveal(&self) -> Result<&str, String> {
        self.password.as_deref().ok_or_else(|| if self.verifier.is_some() {
            "旧访问码需重新设置一次后才能查看".into()
        } else {
            "尚未设置长期访问码".into()
        })
    }
}

pub fn read(path: &Path) -> Result<SavedPassword, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(SavedPassword { version: 2, verifier: None, password: None }),
        Err(_) => return Err("无法读取访问码配置".into()),
    };
    let saved: SavedPassword = serde_json::from_slice(&crate::secure::protect(&bytes, true)?)
        .map_err(|_| "访问码配置无效，请在电脑端重新设置")?;
    match (saved.version, saved.verifier.is_some(), saved.password.as_deref()) {
        (1, _, None) | (2, false, None) => {},
        (2, true, Some(password)) if valid_password(password) => {},
        (1 | 2, _, _) => return Err("访问码配置无效，请在电脑端重新设置".into()),
        _ => return Err("访问码配置版本不支持，请在电脑端重新设置".into()),
    }
    Ok(saved)
}

pub fn write(path: &Path, password: Option<&str>) -> Result<SavedPassword, String> {
    let saved = SavedPassword { version: 2, verifier: password.map(PasswordVerifier::new).transpose()?, password: password.map(str::to_owned) };
    let _lock = crate::storage::lock(path)?;
    let bytes = serde_json::to_vec(&saved).map_err(|_| "无法保存访问码")?;
    crate::storage::write(path, &crate::secure::protect(&bytes, false)?, false)?;
    Ok(saved)
}

struct Window { started: Instant, count: u32 }

impl Window {
    fn new(now: Instant) -> Self { Self { started: now, count: 0 } }
    fn retry_after(&self, now: Instant, duration: Duration, limit: u32) -> Option<u64> {
        (self.count >= limit && now.duration_since(self.started) < duration)
            .then(|| duration.saturating_sub(now.duration_since(self.started)).as_secs() + 1)
    }
}

pub struct LoginLimiter {
    global: Window,
    ips: HashMap<IpAddr, Window>,
}

impl Default for LoginLimiter {
    fn default() -> Self { Self { global: Window::new(Instant::now()), ips: HashMap::new() } }
}

impl LoginLimiter {
    /// 同时限制成功、失败和正在校验的请求，不信任客户端传入的代理 IP。
    pub fn reserve(&mut self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        if now.duration_since(self.global.started) >= GLOBAL_WINDOW { self.global = Window::new(now); }
        self.ips.retain(|_, window| now.duration_since(window.started) < IP_WINDOW);
        if let Some(wait) = self.global.retry_after(now, GLOBAL_WINDOW, 30) { return Err(wait); }
        if let Some(wait) = self.ips.get(&ip).and_then(|window| window.retry_after(now, IP_WINDOW, 5)) { return Err(wait); }
        self.global.count += 1;
        self.ips.entry(ip).or_insert_with(|| Window::new(now)).count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    #[test]
    fn passwords_are_salted_verified_and_encrypted_on_disk() {
        for invalid in ["12345", "1234567", "１２３４５６", "12 456", "abcdef"] {
            assert!(PasswordVerifier::new(invalid).is_err());
        }
        let first = PasswordVerifier::new("012345").unwrap();
        let second = PasswordVerifier::new("012345").unwrap();
        assert_ne!(first.salt, second.salt);
        assert_ne!(first.hash, second.hash);
        assert!(first.verify("012345"));
        assert!(!first.verify("012346"));
        let dir = TempDir::new();
        let path = dir.path().join("password.credential");
        assert!(read(&path).unwrap().verifier.is_none());
        write(&path, Some("012345")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(6).any(|window| window == b"012345"));
        assert!(read(&path).unwrap().verifier.unwrap().verify("012345"));
        write(&path, None).unwrap();
        assert!(read(&path).unwrap().verifier.is_none());
    }

    #[test]
    fn saved_code_can_be_revealed_after_reload_and_cleared_on_disable() {
        let dir = TempDir::new();
        let path = dir.path().join("password.credential");
        assert!(!read(&path).unwrap().can_reveal());
        write(&path, Some("012345")).unwrap();
        let saved = read(&path).unwrap();
        assert!(saved.can_reveal());
        assert_eq!(saved.reveal().unwrap(), "012345");
        assert!(saved.verifier.unwrap().verify("012345"));
        let before = std::fs::read(&path).unwrap();
        assert!(write(&path, Some("12345")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        write(&path, Some("654321")).unwrap();
        let saved = read(&path).unwrap();
        assert_eq!(saved.reveal().unwrap(), "654321");
        assert!(!saved.verifier.unwrap().verify("012345"));
        write(&path, None).unwrap();
        let saved = read(&path).unwrap();
        assert!(saved.verifier.is_none());
        assert!(!saved.can_reveal());
        assert!(saved.reveal().is_err());
        let decrypted = crate::secure::protect(&std::fs::read(&path).unwrap(), true).unwrap();
        assert!(!String::from_utf8(decrypted).unwrap().contains("654321"));
    }

    #[test]
    fn legacy_hash_only_code_still_authenticates_until_reset() {
        let dir = TempDir::new();
        let path = dir.path().join("password.credential");
        let legacy = serde_json::json!({ "version": 1, "verifier": PasswordVerifier::new("012345").unwrap() });
        crate::storage::write(&path, &crate::secure::protect(&serde_json::to_vec(&legacy).unwrap(), false).unwrap(), false).unwrap();
        let saved = read(&path).unwrap();
        assert!(!saved.can_reveal());
        assert_eq!(saved.reveal().unwrap_err(), "旧访问码需重新设置一次后才能查看");
        assert!(saved.verifier.unwrap().verify("012345"));
        write(&path, Some("012345")).unwrap();
        assert_eq!(read(&path).unwrap().reveal().unwrap(), "012345");
    }

    #[test]
    fn limits_apply_per_peer_and_globally_then_expire() {
        let now = Instant::now();
        let mut limiter = LoginLimiter::default();
        limiter.global.started = now;
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..5 { assert!(limiter.reserve(ip, now).is_ok()); }
        assert!(limiter.reserve(ip, now).unwrap_err() <= 301);
        for last in 2..=6 {
            let other = IpAddr::from([127, 0, 0, last]);
            for _ in 0..5 { limiter.reserve(other, now).unwrap(); }
        }
        assert!(limiter.reserve(IpAddr::from([127, 0, 0, 7]), now).is_err());
        assert!(limiter.reserve(ip, now + GLOBAL_WINDOW).is_ok());
    }
}
