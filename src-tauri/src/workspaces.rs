use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

use crate::{paths, storage};

/// 工作区分组文件夹（支持多级嵌套，parent_id 为 None 表示根级）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub agents: Vec<String>,
    pub pinned: bool,
    pub created: u64,
    pub last_used: u64,
    #[serde(default)]
    pub folder_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreData {
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

pub struct Store {
    file: PathBuf,
    inner: Mutex<StoreData>,
}

impl Store {
    /// 从当前应用配置目录加载，兼容旧版纯数组。
    /// 只有文件不存在时使用空配置；读取或解析失败时保留原文件并报错。
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        Self::from_file(dir.join("workspaces.json"))
    }

    fn from_file(file: PathBuf) -> Result<Self, String> {
        let _lock = storage::lock(&file)?;
        let data = Self::read(&file)?;
        Ok(Self { file, inner: Mutex::new(data) })
    }

    fn read(file: &Path) -> Result<StoreData, String> {
        let Some(value) = storage::read_json::<serde_json::Value>(file)? else {
            return Ok(StoreData::default());
        };
        let result = if value.is_array() {
            serde_json::from_value(value).map(|workspaces| StoreData {
                folders: Vec::new(),
                workspaces,
            })
        } else {
            serde_json::from_value(value)
        };
        result.map_err(|e| format!("解析 {} 的工作区结构失败：{e}；原文件已保留", file.display()))
    }

    pub fn list(&self) -> StoreData {
        self.inner.lock().unwrap().clone()
    }

    /// 持跨进程锁重新读取最新配置；落盘成功后才替换缓存。
    fn update<T>(&self, change: impl FnOnce(&mut StoreData) -> Result<T, String>) -> Result<T, String> {
        let mut current = self.inner.lock().map_err(|e| e.to_string())?;
        let _lock = storage::lock(&self.file)?;
        let mut next = Self::read(&self.file)?;
        let previous = next.clone();
        let result = change(&mut next)?;
        if next != previous {
            let json = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
            storage::write(&self.file, &json, true)?;
        }
        *current = next;
        Ok(result)
    }

    /* ---------- workspace ---------- */

    /// 新增工作区；路径已存在时合并（补 agent 徽标、刷新 last_used）
    pub fn add(&self, path: &str, agent: &str) -> Result<Workspace, String> {
        let path = paths::normalize(path);
        self.update(|data| {
            let now = now_ms();
            if let Some(existing) = data.workspaces.iter_mut()
                .find(|w| paths::key(&w.path) == paths::key(&path))
            {
                existing.last_used = now;
                if !agent.is_empty() && !existing.agents.iter().any(|a| a == agent) {
                    existing.agents.push(agent.to_string());
                }
                return Ok(existing.clone());
            }
            let name = Path::new(&path).file_name()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| path.clone());
            let ws = Workspace {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                path,
                agents: if agent.is_empty() { vec![] } else { vec![agent.to_string()] },
                pinned: false,
                created: now,
                last_used: now,
                folder_id: None,
            };
            data.workspaces.push(ws.clone());
            Ok(ws)
        })
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() { return Err("名称不能为空".into()); }
        self.update(|data| {
            find_ws_mut(data, id)?.name = name.to_string();
            Ok(())
        })
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.update(|data| {
            data.workspaces.retain(|w| w.id != id);
            Ok(())
        })
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), String> {
        self.update(|data| {
            find_ws_mut(data, id)?.pinned = pinned;
            Ok(())
        })
    }

    /// 移动工作区到文件夹（folder_id 为 None 表示移回根级）
    pub fn move_ws(&self, id: &str, folder_id: Option<String>) -> Result<(), String> {
        self.update(|data| {
            if let Some(fid) = &folder_id {
                if !data.folders.iter().any(|f| &f.id == fid) {
                    return Err("文件夹不存在".into());
                }
            }
            find_ws_mut(data, id)?.folder_id = folder_id;
            Ok(())
        })
    }

    /// 批量导入历史工作区，跳过已存在路径，返回新增数量
    pub fn import(&self, items: &[crate::agents::ImportCandidate]) -> Result<usize, String> {
        self.update(|data| {
            let now = now_ms();
            let mut added = 0;
            for it in items {
                let path = paths::normalize(&it.path);
                if data.workspaces.iter().any(|w| paths::key(&w.path) == paths::key(&path)) {
                    continue;
                }
                let name = Path::new(&path).file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| path.clone());
                data.workspaces.push(Workspace {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    path,
                    agents: vec![it.agent.clone()],
                    pinned: false,
                    created: now,
                    last_used: it.last_used.min(now),
                    folder_id: None,
                });
                added += 1;
            }
            Ok(added)
        })
    }

    /* ---------- folder ---------- */

    pub fn folder_add(&self, name: &str, parent_id: Option<String>) -> Result<Folder, String> {
        let name = name.trim();
        if name.is_empty() { return Err("名称不能为空".into()); }
        self.update(|data| {
            if let Some(pid) = &parent_id {
                if !data.folders.iter().any(|f| &f.id == pid) {
                    return Err("父文件夹不存在".into());
                }
            }
            let folder = Folder {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                collapsed: false,
                parent_id,
            };
            data.folders.push(folder.clone());
            Ok(folder)
        })
    }

    /// 移动文件夹：改父级（嵌套）或排序。
    /// before_id 为 Some(目标 id) 时插到该文件夹前面，None 时追加到同父级末尾。
    pub fn folder_move(&self, id: &str, parent_id: Option<String>, before_id: Option<String>) -> Result<(), String> {
        self.update(|data| {
            if id == before_id.as_deref().unwrap_or("") {
                return Err("不能移到自身前面".into());
            }
            if let Some(pid) = &parent_id {
                if pid == id { return Err("不能移到自己内部".into()); }
                if !data.folders.iter().any(|f| &f.id == pid) {
                    return Err("目标文件夹不存在".into());
                }
                // 防环：新父级不能是自己的后代。
                let mut cur = parent_id.clone();
                while let Some(pid) = cur {
                    let f = data.folders.iter().find(|f| f.id == pid)
                        .ok_or_else(|| "目标文件夹不存在".to_string())?;
                    if f.id == id { return Err("不能移到自己的子文件夹内".into()); }
                    cur = f.parent_id.clone();
                }
            }
            if let Some(bid) = &before_id {
                if !data.folders.iter().any(|f| &f.id == bid) {
                    return Err("插入位置文件夹不存在".into());
                }
            }
            let pos = data.folders.iter().position(|f| f.id == id)
                .ok_or_else(|| "文件夹不存在".to_string())?;
            let mut folder = data.folders.remove(pos);
            folder.parent_id = parent_id;
            let insert_at = match &before_id {
                Some(bid) => data.folders.iter().position(|f| &f.id == bid)
                    .ok_or_else(|| "插入位置文件夹不存在".to_string())?,
                None => data.folders.len(),
            };
            data.folders.insert(insert_at, folder);
            Ok(())
        })
    }

    /// 合并 agent 徽标（不动 last_used，避免列表排序跳动）
    pub fn sync_agents(&self, path: &str, agents: &[String]) -> Result<(), String> {
        let norm = paths::normalize(path);
        self.update(|data| {
            let ws = data.workspaces.iter_mut()
                .find(|w| paths::key(&w.path) == paths::key(&norm))
                .ok_or_else(|| "工作区不存在".to_string())?;
            for a in agents {
                if !ws.agents.contains(a) { ws.agents.push(a.clone()); }
            }
            Ok(())
        })
    }

    pub fn folder_rename(&self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() { return Err("名称不能为空".into()); }
        self.update(|data| {
            let f = data.folders.iter_mut().find(|f| f.id == id)
                .ok_or_else(|| "文件夹不存在".to_string())?;
            f.name = name.to_string();
            Ok(())
        })
    }

    /// 删除文件夹：子文件夹提升到被删者的父级，工作区也移到上一级（不删除）
    pub fn folder_remove(&self, id: &str) -> Result<(), String> {
        self.update(|data| {
            let parent = data.folders.iter().find(|f| f.id == id)
                .and_then(|f| f.parent_id.clone());
            data.folders.retain(|f| f.id != id);
            for f in &mut data.folders {
                if f.parent_id.as_deref() == Some(id) { f.parent_id = parent.clone(); }
            }
            for ws in &mut data.workspaces {
                if ws.folder_id.as_deref() == Some(id) { ws.folder_id = parent.clone(); }
            }
            Ok(())
        })
    }

    pub fn folder_set_collapsed(&self, id: &str, collapsed: bool) -> Result<(), String> {
        self.update(|data| {
            let f = data.folders.iter_mut().find(|f| f.id == id)
                .ok_or_else(|| "文件夹不存在".to_string())?;
            f.collapsed = collapsed;
            Ok(())
        })
    }
}

fn find_ws_mut<'a>(data: &'a mut StoreData, id: &str) -> Result<&'a mut Workspace, String> {
    data.workspaces.iter_mut().find(|w| w.id == id)
        .ok_or_else(|| "工作区不存在".to_string())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::Barrier;

    #[test]
    fn workspace_paths_keep_roots_and_merge_separator_variants() {
        let dir = TempDir::new();
        let store = Store::from_file(dir.path().join("workspaces.json")).unwrap();
        let first = store.add("C:/", "pi").unwrap();
        let second = store.add(r"c:\", "codex").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.path, r"C:\");
        assert_eq!(second.agents, ["pi", "codex"]);
        store.add("D:/项目/one/", "pi").unwrap();
        store.add(r"d:\项目\one", "claude").unwrap();
        assert_eq!(store.list().workspaces.len(), 2);
    }

    #[test]
    fn stale_instances_merge_the_latest_file() {
        let dir = TempDir::new();
        let path = dir.path().join("workspaces.json");
        let first = Store::from_file(path.clone()).unwrap();
        let second = Store::from_file(path.clone()).unwrap();
        let barrier = Barrier::new(2);
        let (one, two) = std::thread::scope(|scope| {
            let one = scope.spawn(|| { barrier.wait(); first.add(r"D:\one", "codex").unwrap() });
            let two = scope.spawn(|| { barrier.wait(); second.add(r"D:\two", "pi").unwrap() });
            (one.join().unwrap(), two.join().unwrap())
        });
        first.rename(&one.id, "renamed").unwrap();
        second.set_pinned(&two.id, true).unwrap();
        let saved = Store::from_file(path).unwrap().list();
        assert_eq!(saved.workspaces.len(), 2);
        assert_eq!(saved.workspaces.iter().find(|w| w.id == one.id).unwrap().name, "renamed");
        assert!(saved.workspaces.iter().find(|w| w.id == two.id).unwrap().pinned);
    }

    #[test]
    fn failed_save_keeps_disk_and_memory_unchanged() {
        let dir = TempDir::new();
        let path = dir.path().join("workspaces.json");
        let store = Store::from_file(path.clone()).unwrap();
        let workspace = store.add(r"D:\project", "codex").unwrap();
        let before = store.list();
        let bytes = fs::read(&path).unwrap();
        // 模拟目标文件被外部程序占用，不允许替换或删除。
        let held = fs::OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        assert!(store.rename(&workspace.id, "unsaved").is_err());
        assert_eq!(store.list(), before);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!fs::read_dir(dir.path()).unwrap().any(|e|
            e.unwrap().path().extension().is_some_and(|e| e == "tmp")));
        drop(held);
        store.rename(&workspace.id, "saved").unwrap();
        assert_eq!(Store::from_file(path).unwrap().list().workspaces[0].name, "saved");
    }

    #[test]
    fn legacy_migration_and_invalid_json_preserve_originals() {
        let dir = TempDir::new();
        let path = dir.path().join("workspaces.json");
        let legacy = br#"[{"id":"old","name":"old","path":"D:/old","agents":[],"pinned":false,"created":1,"last_used":1}]"#;
        fs::write(&path, legacy).unwrap();
        let store = Store::from_file(path.clone()).unwrap();
        store.set_pinned("old", true).unwrap();
        assert_eq!(fs::read(dir.path().join("workspaces.json.bak")).unwrap(), legacy);
        let before = store.list();
        fs::write(&path, "{invalid").unwrap();
        assert!(Store::from_file(path.clone()).is_err());
        assert!(store.rename("old", "lost").is_err());
        assert_eq!(store.list(), before);
        assert_eq!(fs::read_to_string(path).unwrap(), "{invalid");
        assert_eq!(fs::read(dir.path().join("workspaces.json.bak")).unwrap(), legacy);
    }
}
