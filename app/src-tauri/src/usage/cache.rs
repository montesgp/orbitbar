//! A per-file cache of what the Claude, Pi and Codex readers parsed, so a
//! collect reads again only the session files that changed since the last
//! one. An agent's store holds hundreds of files (gigabytes in total) of
//! which only a few change on a given day.
//!
//! An entry is keyed by the file's path and valid while the file keeps the
//! same length and modified time. It holds that file's parsed result before
//! anything that depends on the machine or the config is applied: raw cwd,
//! local date, model, counters. Project-root resolution, aliases, the Pi
//! `claude-bridge` rule and the window fold run on every collect over it, so a
//! warm cache gives the same snapshot as a cold one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

/// One agent's cached files.
pub struct FileCache<T> {
    files: Mutex<HashMap<PathBuf, Cached<T>>>,
    /// Files parsed (opened and read) through this cache, for tests.
    parsed: AtomicU64,
}

struct Cached<T> {
    len: u64,
    modified: SystemTime,
    data: Arc<T>,
}

impl<T> Default for FileCache<T> {
    fn default() -> Self {
        Self { files: Mutex::new(HashMap::new()), parsed: AtomicU64::new(0) }
    }
}

impl<T> FileCache<T> {
    /// The parsed result of every file in `files` that could be read, in
    /// order. A file whose length and modified time match its entry is not
    /// opened; any other file is parsed again with `parse` (`None`: it could
    /// not be opened, so it is skipped and not cached). Entries under `root`
    /// for files no longer in `files` are dropped.
    ///
    /// The lock is held only to look an entry up and to store the results,
    /// never while a file is read, so concurrent collects only wait for each
    /// other's map updates. Two of them may parse the same changed file; the
    /// results are equal.
    pub fn load(&self, root: &Path, files: &[PathBuf], parse: impl Fn(&Path) -> Option<T>) -> Vec<Arc<T>> {
        let mut out = Vec::with_capacity(files.len());
        let mut fresh: HashMap<PathBuf, Cached<T>> = HashMap::with_capacity(files.len());
        for path in files {
            // A file whose modified time cannot be read is never trusted to be
            // unchanged; one that cannot be stat'ed at all is gone.
            let Ok(meta) = std::fs::metadata(path) else { continue };
            let (len, modified) = (meta.len(), meta.modified().ok());
            let hit = modified.and_then(|modified| {
                let files = self.lock();
                let cached = files.get(path)?;
                (cached.len == len && cached.modified == modified).then(|| cached.data.clone())
            });
            let data = match hit {
                Some(data) => data,
                None => {
                    // Read with the length and time taken before it: a file
                    // that grows meanwhile mismatches next call and is read again.
                    let Some(data) = parse(path) else { continue };
                    self.parsed.fetch_add(1, Ordering::Relaxed);
                    Arc::new(data)
                }
            };
            if let Some(modified) = modified {
                fresh.insert(path.clone(), Cached { len, modified, data: data.clone() });
            }
            out.push(data);
        }

        let mut files = self.lock();
        files.retain(|path, _| !path.starts_with(root) || fresh.contains_key(path));
        files.extend(fresh);
        out
    }

    /// A panic elsewhere while the lock was held leaves the map consistent
    /// (every update is one insert or one retain), so the lock stays usable.
    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Cached<T>>> {
        self.files.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Files parsed through this cache so far (tests only).
    #[cfg(test)]
    pub fn parsed(&self) -> u64 {
        self.parsed.load(Ordering::Relaxed)
    }

    /// Files cached right now (tests only).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

/// The file caches of every reader that has one. `collect_usage` takes it
/// as a parameter: the app passes [`UsageCache::global`], tests a fresh one
/// each, so they never share state.
#[derive(Default)]
pub struct UsageCache {
    pub claude: FileCache<crate::usage::claude::FileUsage>,
    pub pi: FileCache<crate::usage::pi::FileUsage>,
    pub codex: FileCache<crate::usage::codex::FileUsage>,
}

static GLOBAL: LazyLock<UsageCache> = LazyLock::new(UsageCache::default);

impl UsageCache {
    /// The one cache of the running app, shared by `get_usage` and the
    /// start-up warm-up.
    pub fn global() -> &'static UsageCache {
        &GLOBAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn read(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    /// A file that cannot be read is skipped and not cached; a missing one too.
    #[test]
    fn an_unreadable_file_is_skipped_and_not_cached() {
        let dir = tempdir().unwrap();
        let (good, gone) = (dir.path().join("good"), dir.path().join("gone"));
        std::fs::write(&good, "g").unwrap();
        let cache = FileCache::default();
        let got = cache.load(dir.path(), &[good.clone(), gone], read);
        assert_eq!(got.iter().map(|s| s.as_str()).collect::<Vec<_>>(), vec!["g"]);
        assert_eq!(cache.len(), 1);
        let skip = cache.load(dir.path(), std::slice::from_ref(&good), |_| None::<String>);
        assert_eq!(skip.len(), 1, "an unchanged file is not parsed, so it is still there");
    }

    /// Entries are dropped only under the root being listed, so two stores
    /// sharing a cache never evict each other.
    #[test]
    fn only_entries_under_the_listed_root_are_dropped() {
        let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
        let (fa, fb) = (a.path().join("f"), b.path().join("f"));
        std::fs::write(&fa, "a").unwrap();
        std::fs::write(&fb, "b").unwrap();
        let cache = FileCache::default();
        cache.load(a.path(), std::slice::from_ref(&fa), read);
        cache.load(b.path(), std::slice::from_ref(&fb), read);
        assert_eq!(cache.len(), 2);
        cache.load(a.path(), &[], read);
        assert_eq!(cache.len(), 1, "a's file is gone from its listing");
        cache.load(b.path(), std::slice::from_ref(&fb), read);
        assert_eq!(cache.parsed(), 2, "b's entry survived");
    }

    /// A parse that panics never holds the lock, so the cache keeps working.
    #[test]
    fn a_panicking_parse_leaves_the_cache_usable() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        let cache = FileCache::<String>::default();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.load(dir.path(), std::slice::from_ref(&file), |_| panic!("boom"));
        }));
        assert!(panicked.is_err());
        let got = cache.load(dir.path(), std::slice::from_ref(&file), read);
        assert_eq!(got[0].as_str(), "x");
        assert_eq!(cache.len(), 1);
    }
}
