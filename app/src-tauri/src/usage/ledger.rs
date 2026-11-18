//! Orbitbar's own usage ledger: `usage-ledger.json` next to `config.json`,
//! the one file the usage code writes. It keeps the claude and pi daily
//! buckets (raw counters and model, never a cost, so `pricing.json` applies
//! at read time) for 365 days, so usage an agent later deletes - Claude Code
//! removes transcripts after `cleanupPeriodDays` - stays in the history.
//! Codex and OpenCode are not stored: their sessions move between days as
//! they grow, and their stores keep their own history.
//!
//! Buckets are kept per `(agent, date, project, model)`. Every
//! `collect_usage` call merges the buckets it read from the agents' files
//! with the stored ones per whole `(agent, date, project)` group: the side
//! whose group has more entries keeps all of its model buckets, the source on
//! a tie. The file is written only when that changed it (or it was an older
//! version), through a temp file in the same directory plus a rename, under
//! a process-wide lock. A file that cannot be read, does not parse, or has an
//! unknown version is never overwritten: that call uses the source data only
//! and says so in the agents' `detail`.
//!
//! Version 1 files hold one record per `(agent, date, project)`, named after
//! that day's last model; they load as is and are saved as version 2, which
//! has one record per model. The group merge is what keeps such a record and
//! the newer per-model buckets of the same day from both counting.

use crate::usage::daily::{DailyBuckets, DailyCounters};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub const FILE_NAME: &str = "usage-ledger.json";
const VERSION: u64 = 2;
/// The one-record-per-(agent, date, project) format, still read.
const VERSION_1: u64 = 1;
/// Days kept, today included: the longest window (`Last365Days`).
const RETENTION_DAYS: i64 = 365;

/// Serializes every load-merge-write of the ledger inside this process.
static LOCK: Mutex<()> = Mutex::new(());

/// The stored buckets, by agent, date, project key (`""`: no project) and model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    records: BTreeMap<(String, NaiveDate, String, String), DailyCounters>,
}

impl Ledger {
    /// The stored buckets of one agent.
    pub fn buckets(&self, agent: &str) -> DailyBuckets {
        let mut out = DailyBuckets::default();
        for ((_, date, project, _), counters) in self.records.iter().filter(|((a, _, _, _), _)| a == agent) {
            out.insert(*date, project, counters.clone());
        }
        out
    }

    /// Replaces every stored bucket of `agent` with `buckets`, so a model
    /// bucket the merge dropped from a group leaves the ledger too.
    pub fn store(&mut self, agent: &str, buckets: &DailyBuckets) {
        self.records.retain(|(a, _, _, _), _| a != agent);
        for (date, project, counters) in buckets.iter() {
            let key = (agent.to_string(), date, project.to_string(), counters.model.clone());
            self.records.insert(key, counters.clone());
        }
    }

    /// Drops the records dated before the 365 days that end with `today`.
    pub fn prune(&mut self, today: NaiveDate) {
        let oldest = today - Duration::days(RETENTION_DAYS - 1);
        self.records.retain(|(_, date, _, _), _| *date >= oldest);
    }
}

/// The file's shape: `{ "version": 2, "days": [Record, ...] }` (version 1
/// has the same shape, one record per `(agent, date, project)`).
#[derive(Serialize, Deserialize)]
struct LedgerFile {
    version: u64,
    days: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    agent: String,
    /// Local date, `YYYY-MM-DD`.
    date: String,
    project: String,
    model: String,
    input: u64,
    output: u64,
    cache_read: u64,
    #[serde(rename = "cacheWrite5m")]
    cache_write_5m: u64,
    #[serde(rename = "cacheWrite1h")]
    cache_write_1h: u64,
    reasoning: u64,
    entries: u64,
}

/// Merges the buckets read from an agent's files with the stored ones, per
/// whole `(date, project)` group: the side whose group has more entries (all
/// models summed) keeps every one of its model buckets for that group and
/// the other side's are dropped, `source` on a tie. A group only one side has
/// is kept as is (a stored day whose transcript was deleted still counts).
pub fn merge(source: &DailyBuckets, stored: &DailyBuckets) -> DailyBuckets {
    let mut merged = stored.clone();
    for (date, project) in source.groups() {
        if source.group_entries(date, project) >= stored.group_entries(date, project) {
            merged.replace_group(date, project, source.group(date, project));
        }
    }
    merged
}

/// The ledger at `path`; a missing file is an empty ledger. `Err` says why
/// the file cannot be used (unreadable, not JSON, unknown version), in which
/// case it must not be overwritten.
#[cfg_attr(not(test), allow(dead_code))] // `Session` reads through `read`
pub fn load(path: &Path) -> Result<Ledger, String> {
    read(path).map(|(ledger, _)| ledger)
}

/// [`load`], plus the file's version ([`VERSION`] for a missing file), so an
/// older file can be rewritten in the current one.
fn read(path: &Path) -> Result<(Ledger, u64), String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Ledger::default(), VERSION)),
        Err(e) => return Err(format!("cannot read {FILE_NAME}: {e}")),
    };
    let invalid = |e: &dyn std::fmt::Display| format!("{FILE_NAME} is not a valid ledger: {e}");
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| invalid(&e))?;
    let version = match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v @ (VERSION | VERSION_1)) => v,
        Some(other) => return Err(format!("{FILE_NAME} has unknown version {other}")),
        None => return Err(format!("{FILE_NAME} has unknown version (none given)")),
    };
    let file: LedgerFile = serde_json::from_value(value).map_err(|e| invalid(&e))?;

    let mut ledger = Ledger::default();
    for record in file.days {
        let date = NaiveDate::parse_from_str(&record.date, "%Y-%m-%d").map_err(|e| invalid(&format!("date {:?}: {e}", record.date)))?;
        let counters = DailyCounters {
            input: record.input,
            output: record.output,
            cache_read: record.cache_read,
            cache_write_5m: record.cache_write_5m,
            cache_write_1h: record.cache_write_1h,
            reasoning: record.reasoning,
            entries: record.entries,
            model: record.model,
        };
        // A key listed twice keeps the copy with more entries, the merge
        // rule. A version 1 key has no model: its whole group is one record.
        let (agent, project) = (record.agent, record.project);
        let first = (agent.clone(), date, project.clone(), String::new());
        let kept: Vec<_> = ledger
            .records
            .range(first..)
            .take_while(|((a, d, p, _), _)| (a, d, p) == (&agent, &date, &project))
            .filter(|((_, _, _, m), _)| version == VERSION_1 || *m == counters.model)
            .map(|(k, c)| (k.clone(), c.entries))
            .collect();
        if kept.iter().all(|(_, entries)| counters.entries > *entries) {
            for (k, _) in kept {
                ledger.records.remove(&k);
            }
            ledger.records.insert((agent, date, project, counters.model.clone()), counters);
        }
    }
    Ok((ledger, version))
}

/// Writes `ledger` to `path` through a temp file in the same directory and a
/// rename, so a reader never sees a half-written file.
pub fn save(path: &Path, ledger: &Ledger) -> Result<(), String> {
    let file = LedgerFile {
        version: VERSION,
        days: ledger
            .records
            .iter()
            .map(|((agent, date, project, _), c)| Record {
                agent: agent.clone(),
                date: date.format("%Y-%m-%d").to_string(),
                project: project.clone(),
                model: c.model.clone(),
                input: c.input,
                output: c.output,
                cache_read: c.cache_read,
                cache_write_5m: c.cache_write_5m,
                cache_write_1h: c.cache_write_1h,
                reasoning: c.reasoning,
                entries: c.entries,
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("cannot serialize {FILE_NAME}: {e}"))?;

    let dir = path.parent().ok_or_else(|| format!("{} has no parent folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let temp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&temp)
        .and_then(|mut f| {
            std::io::Write::write_all(&mut f, json.as_bytes())?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    written.map_err(|e| {
        // The temp file is Orbitbar's own; never leave a half-written one.
        let _ = std::fs::remove_file(&temp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// One collect call's use of the ledger: holds the process-wide lock from
/// the load until [`Session::finish`] writes.
pub struct Session<'a> {
    _guard: MutexGuard<'static, ()>,
    path: &'a Path,
    /// The file as loaded, to tell whether this call changed it.
    loaded: Ledger,
    /// The file is an older version, so it is rewritten even when unchanged.
    outdated: bool,
    /// `None` when the file cannot be used; the reason is in `note`.
    current: Option<Ledger>,
    note: Option<String>,
}

impl<'a> Session<'a> {
    pub fn open(path: &'a Path) -> Self {
        // A panic in another call while it held the lock leaves nothing
        // half-done on disk (the write is a rename), so the lock is still good.
        let guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match read(path) {
            Ok((loaded, version)) => Self {
                _guard: guard,
                path,
                current: Some(loaded.clone()),
                loaded,
                outdated: version != VERSION,
                note: None,
            },
            Err(reason) => Self {
                _guard: guard,
                path,
                loaded: Ledger::default(),
                outdated: false,
                current: None,
                note: Some(format!("history ledger not used: {reason}")),
            },
        }
    }

    /// Merges `source` (the buckets read from `agent`'s files) into the
    /// ledger and returns the merged buckets; just `source` when the file
    /// cannot be used.
    pub fn merge(&mut self, agent: &str, source: &DailyBuckets) -> DailyBuckets {
        let Some(current) = self.current.as_mut() else { return source.clone() };
        let merged = merge(source, &current.buckets(agent));
        current.store(agent, &merged);
        merged
    }

    /// The stored buckets of `agent`, for an agent whose files could not be read.
    pub fn stored(&self, agent: &str) -> DailyBuckets {
        self.current.as_ref().map(|c| c.buckets(agent)).unwrap_or_default()
    }

    /// Prunes, writes the file when this call changed it or it was an older
    /// version, and releases the lock. Returns the note for the agents'
    /// `detail` when the ledger was not used or could not be saved.
    pub fn finish(self, today: NaiveDate) -> Option<String> {
        let Some(mut current) = self.current else { return self.note };
        current.prune(today);
        if current == self.loaded && !self.outdated {
            return None;
        }
        save(self.path, &current).err().map(|e| format!("history ledger not saved: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    fn counters(entries: u64, output: u64) -> DailyCounters {
        DailyCounters { output, entries, model: "m".into(), ..Default::default() }
    }

    fn buckets(items: &[(u32, &str, DailyCounters)]) -> DailyBuckets {
        let mut out = DailyBuckets::default();
        for (d, project, c) in items {
            out.insert(day(*d), project, c.clone());
        }
        out
    }

    #[test]
    fn merge_keeps_the_bucket_with_more_entries_and_the_source_on_a_tie() {
        let source = buckets(&[
            (1, "grew", counters(5, 50)),
            (1, "shrank", counters(2, 20)),
            (1, "tie", counters(3, 31)),
            (2, "new", counters(1, 10)),
        ]);
        let stored = buckets(&[
            (1, "grew", counters(4, 40)),
            (1, "shrank", counters(6, 60)),
            (1, "tie", counters(3, 30)),
            (3, "deleted", counters(7, 70)),
        ]);

        let merged = merge(&source, &stored);
        assert_eq!(merged.get(day(1), "grew", "m"), Some(&counters(5, 50)), "the source has more entries");
        assert_eq!(merged.get(day(1), "shrank", "m"), Some(&counters(6, 60)), "the stored day has more entries");
        assert_eq!(merged.get(day(1), "tie", "m"), Some(&counters(3, 31)), "a tie keeps the source");
        assert_eq!(merged.get(day(2), "new", "m"), Some(&counters(1, 10)));
        assert_eq!(merged.get(day(3), "deleted", "m"), Some(&counters(7, 70)), "a stored-only day still counts");
        assert_eq!(merged.iter().count(), 5);
    }

    fn with_model(model: &str, entries: u64, output: u64) -> DailyCounters {
        DailyCounters { output, entries, model: model.into(), ..Default::default() }
    }

    /// The `(model, entries, output)` buckets of one `(date, project)` group.
    fn group(b: &DailyBuckets, d: u32, project: &str) -> Vec<(String, u64, u64)> {
        b.iter()
            .filter(|(date, p, _)| *date == day(d) && *p == project)
            .map(|(_, _, c)| (c.model.clone(), c.entries, c.output))
            .collect()
    }

    /// A v1 record holds a whole day under one model name, so the merge
    /// compares whole `(date, project)` groups: the side with more entries
    /// keeps all of its model buckets, the source on a tie, and the other
    /// side's buckets for that group are dropped, never added on top.
    #[test]
    fn merge_compares_whole_date_project_groups() {
        let source = buckets(&[
            (1, "v1", with_model("opus", 6, 60)),
            (1, "v1", with_model("sonnet", 3, 30)),
            (2, "tie", with_model("opus", 6, 60)),
            (2, "tie", with_model("sonnet", 3, 30)),
            (3, "more", with_model("opus", 6, 60)),
            (3, "more", with_model("sonnet", 3, 30)),
            (5, "source-only", with_model("opus", 1, 10)),
        ]);
        let stored = buckets(&[
            (1, "v1", with_model("haiku", 10, 100)),
            (2, "tie", with_model("haiku", 9, 90)),
            (3, "more", with_model("haiku", 5, 50)),
            (4, "ledger-only", with_model("haiku", 7, 70)),
        ]);

        let merged = merge(&source, &stored);
        let both = vec![("opus".to_string(), 6, 60), ("sonnet".to_string(), 3, 30)];
        assert_eq!(group(&merged, 1, "v1"), vec![("haiku".to_string(), 10, 100)], "the ledger group has more entries");
        assert_eq!(group(&merged, 2, "tie"), both, "a tie keeps the source group");
        assert_eq!(group(&merged, 3, "more"), both, "the source group has more entries");
        assert_eq!(group(&merged, 4, "ledger-only"), vec![("haiku".to_string(), 7, 70)]);
        assert_eq!(group(&merged, 5, "source-only"), vec![("opus".to_string(), 1, 10)]);
        assert_eq!(merged.iter().count(), 7);
    }

    /// Storing an agent's merged buckets replaces all of that agent's records,
    /// so a model bucket the merge dropped leaves the ledger too.
    #[test]
    fn store_replaces_every_record_of_the_agent() {
        let mut ledger = Ledger::default();
        ledger.store("claude", &buckets(&[(1, "p", with_model("haiku", 9, 90))]));
        ledger.store("pi", &buckets(&[(1, "p", with_model("gpt", 1, 1))]));
        ledger.store("claude", &buckets(&[(1, "p", with_model("opus", 6, 60)), (1, "p", with_model("sonnet", 4, 40))]));

        let claude = ledger.buckets("claude");
        assert_eq!(group(&claude, 1, "p"), vec![("opus".to_string(), 6, 60), ("sonnet".to_string(), 4, 40)]);
        assert_eq!(group(&ledger.buckets("pi"), 1, "p"), vec![("gpt".to_string(), 1, 1)], "other agents untouched");
    }

    /// A version 1 file loads as is and is saved as version 2 with the same records.
    #[test]
    fn a_version_1_file_loads_and_saves_as_version_2() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let record = |model: &str, entries: u64| {
            serde_json::json!({
                "agent": "claude", "date": "2026-09-17", "project": "p", "model": model, "input": 1, "output": 2,
                "cacheRead": 3, "cacheWrite5m": 4, "cacheWrite1h": 5, "reasoning": 6, "entries": entries
            })
        };
        // A v1 key listed twice keeps the copy with more entries, whatever its model.
        let v1 = serde_json::json!({ "version": 1, "days": [record("a", 3), record("b", 5)] });
        std::fs::write(&path, v1.to_string()).unwrap();

        let ledger = load(&path).expect("a v1 file loads");
        let date = NaiveDate::from_ymd_opt(2026, 9, 17).unwrap();
        let claude = ledger.buckets("claude");
        let kept: Vec<(NaiveDate, &str, &DailyCounters)> = claude.iter().collect();
        let expected = DailyCounters {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write_5m: 4,
            cache_write_1h: 5,
            reasoning: 6,
            entries: 5,
            model: "b".into(),
        };
        assert_eq!(kept, vec![(date, "p", &expected)]);

        save(&path, &ledger).unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({ "version": 2, "days": [record("b", 5)] }));
        assert_eq!(load(&path), Ok(ledger));
    }

    #[test]
    fn prune_keeps_the_365_days_that_end_today() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let mut ledger = Ledger::default();
        let mut stored = DailyBuckets::default();
        stored.insert(today, "p", counters(1, 1));
        stored.insert(today - Duration::days(364), "p", counters(1, 2));
        stored.insert(today - Duration::days(365), "p", counters(1, 3));
        ledger.store("claude", &stored);
        ledger.store("pi", &stored);

        ledger.prune(today);
        for agent in ["claude", "pi"] {
            let kept: Vec<NaiveDate> = ledger.buckets(agent).iter().map(|(d, _, _)| d).collect();
            assert_eq!(kept, vec![today - Duration::days(364), today], "{agent}");
        }
    }

    #[test]
    fn agents_are_stored_apart() {
        let mut ledger = Ledger::default();
        ledger.store("claude", &buckets(&[(1, "p", counters(1, 10))]));
        ledger.store("pi", &buckets(&[(1, "p", counters(2, 20))]));
        assert_eq!(ledger.buckets("claude").get(day(1), "p", "m"), Some(&counters(1, 10)));
        assert_eq!(ledger.buckets("pi").get(day(1), "p", "m"), Some(&counters(2, 20)));
        assert!(ledger.buckets("codex").iter().next().is_none());
    }

    #[test]
    fn a_missing_file_is_an_empty_ledger() {
        let dir = tempdir().unwrap();
        assert_eq!(load(&dir.path().join(FILE_NAME)), Ok(Ledger::default()));
    }

    /// Every counter and the model survive a save and a load, and the file
    /// has the documented shape.
    #[test]
    fn save_and_load_round_trip_every_counter() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config").join(FILE_NAME);
        let full = DailyCounters {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write_5m: 4,
            cache_write_1h: 5,
            reasoning: 6,
            entries: 7,
            model: "claude-opus-5-5".into(),
        };
        let mut ledger = Ledger::default();
        ledger.store("claude", &buckets(&[(17, "C:\\repos\\orbitbar", full.clone())]));
        ledger.store("pi", &buckets(&[(18, "", counters(1, 9))]));

        save(&path, &ledger).unwrap();
        assert_eq!(load(&path), Ok(ledger));

        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["version"], 2);
        assert_eq!(
            json["days"][0],
            serde_json::json!({
                "agent": "claude",
                "date": "2026-09-17",
                "project": "C:\\repos\\orbitbar",
                "model": "claude-opus-5-5",
                "input": 1,
                "output": 2,
                "cacheRead": 3,
                "cacheWrite5m": 4,
                "cacheWrite1h": 5,
                "reasoning": 6,
                "entries": 7
            })
        );
        assert_eq!(json["days"].as_array().unwrap().len(), 2);
        let names: Vec<String> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE_NAME.to_string()], "no temp file is left behind");
    }

    #[test]
    fn a_file_that_is_not_a_version_1_or_2_ledger_cannot_be_loaded() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        for (content, reason) in [
            ("{ not json", "not a valid ledger"),
            (r#"{"version":3,"days":[]}"#, "unknown version 3"),
            (r#"{"days":[]}"#, "unknown version"),
            (r#"{"version":1,"days":[{"agent":"claude","date":"17/09/2026","project":"","model":"","input":0,"output":0,"cacheRead":0,"cacheWrite5m":0,"cacheWrite1h":0,"reasoning":0,"entries":1}]}"#, "not a valid ledger"),
        ] {
            std::fs::write(&path, content).unwrap();
            let err = load(&path).expect_err(content);
            assert!(err.contains(reason), "{content}: {err}");
            assert!(err.contains(FILE_NAME), "{err}");
        }
    }
}
