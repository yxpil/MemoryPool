use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{Context, Result};

use crate::model::Memory;

/// In-memory index over a JSONL file with atomic persistence.
///
/// - `add` appends a single JSON line to the file (fast O(1) write).
/// - Mutations (`delete`, `set_tags`) rewrite the whole file through a
///   temporary file + `rename`, so readers never observe partial state.
pub struct Store {
    path: PathBuf,
    inner: RwLock<Vec<Memory>>,
}

impl Store {
    /// Resolve the data file: `$MEMORYPOOL_DATA_DIR/memories.jsonl` or
    /// `~/.memorypool/memories.jsonl` when the env var is unset.
    pub fn resolve_path() -> PathBuf {
        let dir = match std::env::var("MEMORYPOOL_DATA_DIR") {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => home_dir().join(".memorypool"),
        };
        dir.join("memories.jsonl")
    }

    /// Open the store: create the parent directory if needed and load all
    /// existing memories into memory.
    pub fn open() -> Result<Store> {
        Store::open_at(Self::resolve_path())
    }

    pub fn open_at(path: PathBuf) -> Result<Store> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data directory {}", parent.display()))?;
        }
        let memories = load_jsonl(&path)?;
        Ok(Store {
            path,
            inner: RwLock::new(memories),
        })
    }

    pub fn count(&self) -> usize {
        self.inner.read().expect("store lock poisoned").len()
    }

    /// Append a new memory and persist it as one JSON line.
    pub fn add(&self, memory: Memory) -> Result<Memory> {
        let line = serde_json::to_string(&memory).context("failed to serialize memory")?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open {}", self.path.display()))?;
        let mut writer = BufWriter::new(&file);
        writeln!(writer, "{line}").context("failed to append memory")?;
        writer.flush().context("failed to flush memory")?;
        file.sync_data().context("failed to sync memory file")?;
        self.inner
            .write()
            .expect("store lock poisoned")
            .push(memory.clone());
        Ok(memory)
    }

    pub fn get(&self, id: &str) -> Option<Memory> {
        self.inner
            .read()
            .expect("store lock poisoned")
            .iter()
            .find(|m| m.id == id)
            .cloned()
    }

    /// Delete by id; returns true when something was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut guard = self.inner.write().expect("store lock poisoned");
        let before = guard.len();
        guard.retain(|m| m.id != id);
        if guard.len() == before {
            return Ok(false);
        }
        rewrite_atomic(&self.path, &guard)?;
        Ok(true)
    }

    /// Add/remove tags on a memory; returns the updated memory.
    pub fn set_tags(
        &self,
        id: &str,
        add: Vec<String>,
        remove: Vec<String>,
    ) -> Result<Option<Memory>> {
        let mut guard = self.inner.write().expect("store lock poisoned");
        let Some(mem) = guard.iter_mut().find(|m| m.id == id) else {
            return Ok(None);
        };
        for t in add {
            if !t.is_empty() && !mem.tags.contains(&t) {
                mem.tags.push(t);
            }
        }
        mem.tags.retain(|t| !remove.contains(t));
        mem.updated_at = crate::model::now_rfc3339();
        let updated = mem.clone();
        rewrite_atomic(&self.path, &guard)?;
        Ok(Some(updated))
    }

    /// All memories sorted by `created_at` descending (newest first).
    pub fn list(&self, tag: Option<&str>) -> Vec<Memory> {
        let mut all: Vec<Memory> = self
            .inner
            .read()
            .expect("store lock poisoned")
            .iter()
            .filter(|m| tag.is_none_or(|t| m.tags.iter().any(|x| x == t)))
            .cloned()
            .collect();
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        all
    }

    /// Aggregate stats: count, tag distribution, oldest / newest timestamps.
    pub fn stats(&self) -> Stats {
        let guard = self.inner.read().expect("store lock poisoned");
        let mut tags: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let mut oldest: Option<String> = None;
        let mut newest: Option<String> = None;
        for m in guard.iter() {
            for t in &m.tags {
                *tags.entry(t.clone()).or_insert(0) += 1;
            }
            match (&oldest, &newest) {
                (Some(o), Some(n)) => {
                    if m.created_at < *o {
                        oldest = Some(m.created_at.clone());
                    }
                    if m.created_at > *n {
                        newest = Some(m.created_at.clone());
                    }
                }
                _ => {
                    oldest = Some(m.created_at.clone());
                    newest = Some(m.created_at.clone());
                }
            }
        }
        Stats {
            count: guard.len(),
            tags,
            oldest,
            newest,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Stats {
    pub count: usize,
    pub tags: std::collections::BTreeMap<String, usize>,
    pub oldest: Option<String>,
    pub newest: Option<String>,
}

/// Read all memories from a JSONL file; malformed lines are skipped with a
/// warning on stderr so one bad line cannot take the pool down.
fn load_jsonl(path: &Path) -> Result<Vec<Memory>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut content = String::new();
    File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?
        .read_to_string(&mut content)
        .context("failed to read memory file")?;
    let mut memories = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Memory>(line) {
            Ok(m) => memories.push(m),
            Err(e) => eprintln!(
                "warning: skipping malformed line {} in {}: {e}",
                i + 1,
                path.display()
            ),
        }
    }
    Ok(memories)
}

/// Atomically rewrite the whole file: write a sibling temp file, sync it,
/// then `rename` over the target (atomic on the same filesystem).
pub fn rewrite_atomic(path: &Path, memories: &[Memory]) -> Result<()> {
    let tmp = temp_sibling(path);
    {
        let file = File::create(&tmp)
            .with_context(|| format!("failed to create temp file {}", tmp.display()))?;
        let mut writer = BufWriter::new(file);
        for m in memories {
            let line = serde_json::to_string(m).context("failed to serialize memory")?;
            writeln!(writer, "{line}").context("failed to write temp file")?;
        }
        writer.flush().context("failed to flush temp file")?;
        writer
            .get_ref()
            .sync_all()
            .context("failed to sync temp file")?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} -> {}", tmp.display(), path.display()))?;
    sync_parent_dir(path);
    Ok(())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "memories.jsonl".to_string());
    path.with_file_name(format!(".{name}.tmp"))
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

fn home_dir() -> PathBuf {
    if let Some(home) = std::env::home_dir() {
        return home;
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_rfc3339;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "memorypool-test-{}-{name}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mem(text: &str, tags: Vec<String>) -> Memory {
        crate::model::new_memory(text.to_string(), tags, 0.5, None)
    }

    #[test]
    fn append_and_reload_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join("memories.jsonl");
        {
            let store = Store::open_at(path.clone()).unwrap();
            store.add(mem("alpha", vec!["a".into()])).unwrap();
            store.add(mem("beta", vec![])).unwrap();
        }
        let store = Store::open_at(path.clone()).unwrap();
        assert_eq!(store.count(), 2);
        let all = store.list(None);
        assert_eq!(all[0].text, "beta"); // newest first
        assert_eq!(all[1].text, "alpha");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_rewrites_atomically_without_temp_leftovers() {
        let dir = tmp_dir("delete");
        let path = dir.join("memories.jsonl");
        let store = Store::open_at(path.clone()).unwrap();
        let m1 = store.add(mem("keep", vec![])).unwrap();
        let m2 = store.add(mem("drop", vec![])).unwrap();
        assert!(store.delete(&m2.id).unwrap());
        assert!(!store.delete(&m2.id).unwrap());
        // Only the data file remains: no .tmp leftovers after atomic rewrite.
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let reloaded = Store::open_at(path).unwrap();
        assert_eq!(reloaded.count(), 1);
        assert!(reloaded.get(&m1.id).is_some());
        assert!(reloaded.get(&m2.id).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_tags_persists_and_bumps_updated_at() {
        let dir = tmp_dir("tags");
        let path = dir.join("memories.jsonl");
        let store = Store::open_at(path.clone()).unwrap();
        let m = store.add(mem("tagged", vec!["old".into()])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let updated = store
            .set_tags(&m.id, vec!["new".into()], vec!["old".into()])
            .unwrap()
            .unwrap();
        assert_eq!(updated.tags, vec!["new"]);
        assert!(updated.updated_at >= updated.created_at);
        let reloaded = Store::open_at(path).unwrap();
        assert_eq!(reloaded.get(&m.id).unwrap().tags, vec!["new"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_lines_are_skipped_on_load() {
        let dir = tmp_dir("malformed");
        let path = dir.join("memories.jsonl");
        let store = Store::open_at(path.clone()).unwrap();
        let m = store.add(mem("good", vec![])).unwrap();
        drop(store);
        // Corrupt the middle of the file.
        let mut content = fs::read_to_string(&path).unwrap();
        content.push_str("{not valid json}\n");
        fs::write(&path, content).unwrap();
        let store = Store::open_at(path).unwrap();
        assert_eq!(store.count(), 1);
        assert!(store.get(&m.id).is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_starts_empty() {
        let dir = tmp_dir("empty");
        let store = Store::open_at(dir.join("memories.jsonl")).unwrap();
        assert_eq!(store.count(), 0);
        assert_eq!(store.stats().count, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stats_reports_oldest_and_newest() {
        let dir = tmp_dir("stats");
        let store = Store::open_at(dir.join("memories.jsonl")).unwrap();
        let a = store.add(mem("first", vec!["x".into()])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = store
            .add(mem("second", vec!["x".into(), "y".into()]))
            .unwrap();
        let stats = store.stats();
        assert_eq!(stats.count, 2);
        assert_eq!(stats.tags.get("x"), Some(&2));
        assert_eq!(stats.tags.get("y"), Some(&1));
        assert_eq!(stats.oldest.as_deref(), Some(a.created_at.as_str()));
        assert_eq!(stats.newest.as_deref(), Some(b.created_at.as_str()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_filters_by_tag_and_sorts_desc() {
        let dir = tmp_dir("listfilter");
        let store = Store::open_at(dir.join("memories.jsonl")).unwrap();
        store.add(mem("a", vec!["t1".into()])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.add(mem("b", vec!["t2".into()])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.add(mem("c", vec!["t1".into()])).unwrap();
        let t1: Vec<String> = store
            .list(Some("t1"))
            .iter()
            .map(|m| m.text.clone())
            .collect();
        assert_eq!(t1, vec!["c", "a"]);
        let all = store.list(None);
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|w| w[0].created_at >= w[1].created_at));
        assert!(parse_rfc3339(&all[0].created_at).is_some());
        fs::remove_dir_all(&dir).unwrap();
    }
}
