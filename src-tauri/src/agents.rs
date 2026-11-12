//! AI Agent CLI 的会话数据扫描。
//! 共享元数据索引与 cwd 文件头缓存；侧栏查询不读取标题，展开后仅为最终条目读取摘要。
//!
//! - Claude Code: ~/.claude/projects/<编码路径>/<uuid>.jsonl（目录名 = 路径中非字母数字替换为 '-'）
//! - Codex:      ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl（头部含 cwd）

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::paths;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportCandidate {
    pub agent: String,
    pub path: String,
    pub last_used: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct AgentSession {
    pub agent: String,
    pub session_id: String,
    pub last_used: u64,
    /// 会话标题（首条用户消息摘要；官方 resume 列表同款取法）
    pub title: Option<String>,
}

/// 清洗用户输入文本：跳过系统注入/控制命令，截断为标题
fn title_from_text(t: &str) -> Option<String> {
    let t = t.trim();
    if t.is_empty() || t.len() < 2 {
        return None;
    }
    let first = t.lines().next().unwrap_or("");
    if first.starts_with('#')
        || first.starts_with('<')
        || first.starts_with("Caveat:")
        || first.eq_ignore_ascii_case("exit")
        || first.eq_ignore_ascii_case("quit")
    {
        return None;
    }
    let cut: String = first.chars().take(60).collect();
    Some(cut)
}

/// claude：从会话 jsonl 头部提取首条用户消息
fn claude_jsonl_title(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let f = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(f.take(256 * 1024));
    for line in reader.lines().take(60) {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        if v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for part in content {
            if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if let Some(title) = title_from_text(text) {
                        return Some(title);
                    }
                }
            }
        }
    }
    None
}

fn home() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// 限制读取字节数和行数，使用 JSON 解码保留空格、Unicode 与路径转义。
fn head_cwd(file: &Path) -> Option<String> {
    let reader = BufReader::new(fs::File::open(file).ok()?.take(256 * 1024));
    for line in reader.lines().take(40) {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let Some(cwd) = value.get("cwd").or_else(|| value.pointer("/payload/cwd")).and_then(|v| v.as_str()) {
            if !cwd.is_empty() { return Some(cwd.to_string()); }
        }
    }
    None
}

/// Claude Code 项目目录名解码：'-' → '\'（"D--work-foo" → "D:\work\foo"）
fn decode_claude_dir_name(d: &Path) -> Option<String> {
    let name = d.file_name()?.to_string_lossy().to_string();
    if name.len() < 3 {
        return None;
    }
    let replaced: String = name
        .chars()
        .map(|c| if c == '-' { '\\' } else { c })
        .collect();
    let b = replaced.as_bytes();
    // 盘符样式 "D--..." → "D:\..."
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b'\\' && b[2] == b'\\' {
        return Some(format!("{}:{}", &replaced[..1], &replaced[2..]));
    }
    Some(replaced)
}

/// Claude Code 项目目录名编码（正向，用于会话查询）
fn encode_claude_dir(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// pi 会话目录编码：非字母数字转 '-'，首尾包 '--'
fn encode_pi_dir(path: &str) -> String {
    let inner: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("--{inner}--")
}

/// pi：从会话 jsonl 头部提取首条用户消息（type:message 且 role=user）
fn pi_jsonl_title(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let f = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(f.take(256 * 1024));
    for line in reader.lines().take(60) {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        if v.pointer("/message/role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let c = match v.pointer("/message/content") {
            Some(c) => c,
            None => continue,
        };
        if let Some(text) = c.as_str() {
            if let Some(t) = title_from_text(text) {
                return Some(t);
            }
        } else if let Some(arr) = c.as_array() {
            for part in arr {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if let Some(t) = title_from_text(text) {
                            return Some(t);
                        }
                    }
                }
            }
        }
    }
    None
}

fn collect_jsonl_recursive(dir: &Path, out: &mut Vec<(FileStamp, PathBuf)>, depth: usize) {
    if depth > 4 { return; }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_jsonl_recursive(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Some(stamp) = FileStamp::read(&path) { out.push((stamp, path)); }
        }
    }
}

/// codex rollout 文件名 → session uuid（末尾 36 字符，不依赖时间戳格式）
/// rollout-2026-10-09T15-32-50-<uuid>.jsonl
fn codex_session_id(f: &Path) -> Option<String> {
    let stem = f.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("rollout-")?;
    if rest.len() < 36 {
        return None;
    }
    let id = rest.get(rest.len() - 36..)?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id.to_string())
}

/* ================= 共享会话索引 ================= */

const INDEX_TTL: Duration = Duration::from_secs(5);
static INDEX: Mutex<Option<SessionIndex>> = Mutex::new(None);

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self { modified: metadata.modified().ok(), len: metadata.len() })
    }

    fn last_used(&self) -> u64 {
        self.modified.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64).unwrap_or(0)
    }
}

#[derive(Clone)]
struct SessionFile {
    agent: &'static str,
    session_id: String,
    last_used: u64,
    file: PathBuf,
}

struct CachedCwd {
    stamp: FileStamp,
    cwd: Option<String>,
}

#[derive(Default)]
struct HistoryTitles {
    stamp: Option<FileStamp>,
    titles: HashMap<String, String>,
}

impl HistoryTitles {
    fn refresh(&mut self, path: &Path) {
        let stamp = FileStamp::read(path);
        if stamp == self.stamp { return; }
        let Some(stamp) = stamp else {
            self.stamp = None;
            self.titles.clear();
            return;
        };
        let Ok(file) = fs::File::open(path) else { return };
        #[derive(Deserialize)]
        struct HistoryLine { session_id: String, ts: u64, text: String }
        let mut best: HashMap<String, (u64, String)> = HashMap::new();
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { return };
            let Ok(item) = serde_json::from_str::<HistoryLine>(&line) else { continue };
            let Some(title) = title_from_text(&item.text) else { continue };
            let entry = best.entry(item.session_id).or_insert((item.ts, title.clone()));
            if item.ts < entry.0 { *entry = (item.ts, title); }
        }
        self.titles = best.into_iter().map(|(id, (_, title))| (id, title)).collect();
        self.stamp = Some(stamp);
    }
}

struct SessionIndex {
    home: PathBuf,
    refreshed: Option<Instant>,
    claude: HashMap<String, Vec<SessionFile>>,
    codex: HashMap<String, Vec<SessionFile>>,
    pi: HashMap<String, Vec<SessionFile>>,
    codex_cwds: HashMap<PathBuf, CachedCwd>,
    history: HistoryTitles,
}

impl SessionIndex {
    fn new(home: PathBuf) -> Self {
        Self {
            home, refreshed: None,
            claude: HashMap::new(), codex: HashMap::new(), pi: HashMap::new(),
            codex_cwds: HashMap::new(), history: HistoryTitles::default(),
        }
    }

    fn refresh(&mut self) {
        if self.refreshed.is_some_and(|at| at.elapsed() < INDEX_TTL) { return; }
        self.claude = collect_projects(&self.home.join(".claude/projects"), "claude");
        self.pi = collect_projects(&self.home.join(".pi/agent/sessions"), "pi");

        let mut files = Vec::new();
        collect_jsonl_recursive(&self.home.join(".codex/sessions"), &mut files, 0);
        let mut previous = std::mem::take(&mut self.codex_cwds);
        self.codex.clear();
        for (stamp, file) in files {
            let Some(id) = codex_session_id(&file) else { continue };
            let last_used = stamp.last_used();
            let cached = previous.remove(&file).filter(|cached| cached.stamp == stamp)
                .unwrap_or_else(|| CachedCwd { stamp, cwd: head_cwd(&file) });
            if let Some(cwd) = &cached.cwd {
                self.codex.entry(paths::key(cwd)).or_default().push(SessionFile {
                    agent: "codex", session_id: id, last_used, file: file.clone(),
                });
            }
            self.codex_cwds.insert(file, cached);
        }
        for sessions in self.claude.values_mut().chain(self.codex.values_mut()).chain(self.pi.values_mut()) {
            sessions.sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.file.cmp(&b.file)));
        }
        self.refreshed = Some(Instant::now());
    }

    fn groups(&self, path: &str) -> [Option<&Vec<SessionFile>>; 3] {
        let normalized = paths::normalize(path);
        [
            self.claude.get(&encode_claude_dir(&normalized).to_lowercase()),
            self.codex.get(&paths::key(&normalized)),
            self.pi.get(&encode_pi_dir(&normalized).to_lowercase()),
        ]
    }

    fn flags(&self, paths: &[String]) -> Vec<bool> {
        paths.iter().map(|path|
            self.groups(path).iter().flatten().any(|sessions| !sessions.is_empty())
        ).collect()
    }

    fn sessions(&mut self, path: &str, limit: usize) -> Vec<AgentSession> {
        if limit == 0 { return Vec::new(); }
        let mut selected: Vec<&SessionFile> = self.groups(path).into_iter().flatten().flatten().collect();
        selected.sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.file.cmp(&b.file)));
        // 元数据先排序截取；只为最终展示的会话读取标题。
        let selected: Vec<SessionFile> = selected.into_iter().take(limit).cloned().collect();
        if selected.iter().any(|session| session.agent == "codex") {
            self.history.refresh(&self.home.join(".codex/history.jsonl"));
        }
        selected.into_iter().map(|session| {
            let title = match session.agent {
                "claude" => claude_jsonl_title(&session.file),
                "pi" => pi_jsonl_title(&session.file),
                _ => self.history.titles.get(&session.session_id).cloned(),
            };
            AgentSession {
                agent: session.agent.into(), session_id: session.session_id,
                last_used: session.last_used, title,
            }
        }).collect()
    }

    fn imports(&self) -> Vec<ImportCandidate> {
        let mut out: HashMap<String, ImportCandidate> = HashMap::new();
        for sessions in self.claude.values().chain(self.pi.values()).chain(self.codex.values()) {
            let Some(session) = sessions.first() else { continue };
            let cwd = if session.agent == "codex" {
                self.codex_cwds.get(&session.file).and_then(|cached| cached.cwd.clone())
            } else {
                head_cwd(&session.file).or_else(|| {
                    if session.agent == "claude" {
                        session.file.parent().and_then(decode_claude_dir_name)
                    } else { None }
                })
            };
            let Some(path) = cwd.filter(|path| Path::new(path).is_dir()) else { continue };
            let key = paths::key(&path);
            let candidate = ImportCandidate {
                agent: session.agent.into(), path: paths::normalize(&path), last_used: session.last_used,
            };
            let entry = out.entry(key).or_insert_with(|| candidate.clone());
            if candidate.last_used > entry.last_used { *entry = candidate; }
        }
        let mut result: Vec<_> = out.into_values().collect();
        result.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        result
    }
}

/// Claude / Pi 按编码目录建立元数据索引；不打开 jsonl。
fn collect_projects(root: &Path, agent: &'static str) -> HashMap<String, Vec<SessionFile>> {
    let mut projects = HashMap::new();
    let Ok(entries) = fs::read_dir(root) else { return projects };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) { continue; }
        let Ok(files) = fs::read_dir(entry.path()) else { continue };
        let mut sessions = Vec::new();
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let id = if agent == "pi" {
                let Some((_, id)) = stem.split_once('_') else { continue };
                if id.is_empty() { continue; }
                id
            } else { stem };
            let Some(stamp) = FileStamp::read(&path) else { continue };
            sessions.push(SessionFile { agent, session_id: id.to_string(), last_used: stamp.last_used(), file: path });
        }
        if !sessions.is_empty() {
            projects.insert(entry.file_name().to_string_lossy().to_lowercase(), sessions);
        }
    }
    projects
}

fn with_index<T>(query: impl FnOnce(&mut SessionIndex) -> T) -> Option<T> {
    let home = home()?;
    let mut guard = INDEX.lock().ok()?;
    if guard.as_ref().is_none_or(|index| index.home != home) {
        *guard = Some(SessionIndex::new(home));
    }
    let index = guard.as_mut()?;
    index.refresh();
    Some(query(index))
}

/// 批量查询只访问共享元数据索引，不读取标题或全局输入历史。
pub fn session_counts(paths: &[String]) -> Vec<bool> {
    if paths.is_empty() { return Vec::new(); }
    with_index(|index| index.flags(paths)).unwrap_or_else(|| vec![false; paths.len()])
}

pub fn agent_sessions(path: &str, limit: usize) -> Vec<AgentSession> {
    if limit == 0 { return Vec::new(); }
    with_index(|index| index.sessions(path, limit)).unwrap_or_default()
}

pub fn scan_import() -> Vec<ImportCandidate> {
    with_index(|index| index.imports()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    fn codex_file(home: &Path, cwd: &str, timestamp: u64) -> (PathBuf, String) {
        let directory = home.join(".codex/sessions/2026/01/01");
        fs::create_dir_all(&directory).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = directory.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl"));
        let header = serde_json::json!({"type":"session_meta", "payload":{"cwd":cwd}});
        fs::write(&path, serde_json::to_string_pretty(&header).unwrap().replace('\n', " ")).unwrap();
        fs::File::options().write(true).open(&path).unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(timestamp)).unwrap();
        (path, id)
    }

    #[test]
    fn index_finds_old_workspaces_and_flags_do_not_load_titles() {
        let dir = TempDir::new();
        let (_, old_id) = codex_file(dir.path(), r"D:\旧项目\one", 1);
        for _ in 0..305 { codex_file(dir.path(), r"D:\recent", 10); }
        let history = dir.path().join(".codex/history.jsonl");
        let lines = [
            serde_json::json!({"session_id":old_id,"ts":2,"text":"later input"}).to_string(),
            serde_json::json!({"session_id":old_id,"ts":1,"text":"old workspace input"}).to_string(),
        ].join("\n");
        fs::write(history, lines).unwrap();

        let claude = dir.path().join(".claude/projects").join(encode_claude_dir(r"C:\"));
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join("claude-id.jsonl"), r#"{"type":"user","message":{"content":[{"type":"text","text":"Claude title"}]}}"#).unwrap();
        let pi = dir.path().join(".pi/agent/sessions").join(encode_pi_dir(r"D:\pi"));
        fs::create_dir_all(&pi).unwrap();
        fs::write(pi.join("2026_pi-id.jsonl"), r#"{"type":"message","message":{"role":"user","content":"Pi title"}}"#).unwrap();

        let mut index = SessionIndex::new(dir.path().to_path_buf());
        index.refresh();
        assert_eq!(index.flags(&["d:/旧项目/ONE/".into(), "C:/".into(), "d:/pi/".into(), "D:/absent".into()]),
            [true, true, true, false]);
        assert!(index.history.stamp.is_none());
        assert!(index.history.titles.is_empty());
        let sessions = index.sessions("D:/旧项目/one", 5);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, old_id);
        assert_eq!(sessions[0].title.as_deref(), Some("old workspace input"));
        assert_eq!(index.sessions("D:/recent", 3).len(), 3);
        assert_eq!(index.sessions("C:/", 1)[0].title.as_deref(), Some("Claude title"));
        assert_eq!(index.sessions("D:/pi", 1)[0].title.as_deref(), Some("Pi title"));
    }

    #[test]
    fn refresh_replaces_changed_headers_and_prunes_deleted_files() {
        let dir = TempDir::new();
        let (file, _) = codex_file(dir.path(), r"D:\before", 1);
        let mut index = SessionIndex::new(dir.path().to_path_buf());
        index.refresh();
        fs::write(&file, r#"{"payload": {"cwd": "E:\\after\\中文"}}"#).unwrap();
        index.refresh();
        assert_eq!(index.flags(&["D:/before".into()]), [true]);
        index.refreshed = None;
        index.refresh();
        assert_eq!(index.flags(&["D:/before".into(), "e:/after/中文/".into()]), [false, true]);
        fs::remove_file(file).unwrap();
        index.refreshed = None;
        index.refresh();
        assert_eq!(index.flags(&["E:/after/中文".into()]), [false]);
        assert!(index.codex_cwds.is_empty());
    }
}
