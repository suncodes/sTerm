//! Windows 路径的存储和比较规则，不访问磁盘。
pub fn normalize(path: &str) -> String {
    let path = path.replace('/', "\\");
    let trimmed = path.trim_end_matches('\\');
    if trimmed.is_empty() {
        return if path.is_empty() { String::new() } else { "\\".into() };
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && path.ends_with('\\') {
        return format!("{trimmed}\\");
    }
    trimmed.to_string()
}

pub fn key(path: &str) -> String {
    normalize(path).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_roots_and_matches_windows_separators() {
        assert_eq!(normalize(r"C:\"), r"C:\");
        assert_eq!(normalize("C:///"), r"C:\");
        assert_eq!(normalize(r"C:"), "C:");
        assert_eq!(normalize(r"\\server\share\"), r"\\server\share");
        assert_eq!(normalize("/"), r"\");
        assert_eq!(key("D:/项目/One/"), key(r"d:\项目\one"));
    }
}
