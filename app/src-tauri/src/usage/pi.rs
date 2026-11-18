//! Pi usage reader: every `*.jsonl` under `~/.pi/agent/sessions`, de-duped
//! and aggregated into daily buckets like the claude reader.
//!
//! A session file starts with a `type: "session"` header carrying the cwd;
//! each assistant turn is a `type: "message"` line whose `message.usage`
//! holds `input` (without cache reads), `output`, `cacheRead`, `cacheWrite`
//! and `reasoning`, next to `message.provider` and `message.model`. Entries
//! are de-duplicated per file by their `id`.
//!
//! Pi's `claude-bridge` provider runs Claude Code, so those turns are also in
//! Claude Code's transcripts and share no id with them. A bridge turn counts
//! only for a `(date, project)` where Claude has no bucket; every other
//! provider always counts. Sessions outside any project or with no cwd (see
//! `project_key`) count in the "Outside projects" row.

use crate::usage::daily::{DailyBuckets, DailyCounters};
use crate::usage::{
    cache::FileCache, find_jsonl_files, normalize_project_path, pricing, project_key, AgentUsageReport, CostBasis, ProjectAliases,
    ProjectRootCache, UsageTotals, OUTSIDE_PROJECTS,
};
use chrono::{DateTime, Local, NaiveDate};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

pub const AGENT: &str = "pi";

/// The provider whose turns Claude Code's transcripts already hold.
const CLAUDE_BRIDGE: &str = "claude-bridge";

/// One session file's turns before project-root resolution and the
/// `claude-bridge` rule: summed per `(local date, normalized raw cwd, is a
/// claude-bridge turn, model)`, which is what `usage::cache` keeps per file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileUsage {
    turns: BTreeMap<(NaiveDate, String, bool, String), DailyCounters>,
}

/// Everything one scan of the store counted, before any window is applied.
#[derive(Debug, Clone, Default)]
pub struct PiDaily {
    /// Every counted turn, by local date and project key (see `project_key`).
    pub buckets: DailyBuckets,
    pub files_scanned: u64,
    /// `claude-bridge` turns left out because Claude has their `(date, project)`, per local date.
    pub bridge_skipped: BTreeMap<NaiveDate, u64>,
}

/// Reads window usage from every pi session file under `root`, pricing
/// against `overrides`: a [`read_daily`] scan folded over the local dates in
/// `[start, end)`. `claude` holds the `(date, project)` buckets Claude counts.
/// `collect_usage` calls the two steps itself to merge the ledger in between.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_usage(
    root: &Path,
    claude: &DailyBuckets,
    start: DateTime<Local>,
    end: DateTime<Local>,
    overrides: &pricing::PriceOverrides,
    root_cache: &mut ProjectRootCache,
) -> AgentUsageReport {
    match read_daily(root, claude, &FileCache::default(), root_cache) {
        Ok(daily) => report(&daily, start.date_naive(), end.date_naive(), overrides, &ProjectAliases::default()),
        Err(report) => *report,
    }
}

/// Scans every session file under `root` into daily buckets, leaving out the
/// `claude-bridge` turns whose `(date, project)` is in `claude`. Only files
/// that changed since the last call are read again (see `usage::cache`).
/// `Err` carries the finished report when there is nothing to fold: the
/// store is missing, or no session file could be read.
pub fn read_daily(
    root: &Path,
    claude: &DailyBuckets,
    cache: &FileCache<FileUsage>,
    root_cache: &mut ProjectRootCache,
) -> Result<PiDaily, Box<AgentUsageReport>> {
    if !root.exists() {
        return Err(Box::new(AgentUsageReport::not_installed(
            AGENT,
            format!("missing directory: {}", root.display()),
        )));
    }

    let files = find_jsonl_files(root);
    let parsed = cache.load(root, &files, parse_file);
    let mut daily = PiDaily { files_scanned: parsed.len() as u64, ..Default::default() };
    for file in &parsed {
        for ((date, cwd, bridge, _), counters) in &file.turns {
            let project = project_key(cwd, root_cache);
            if *bridge && claude.contains(*date, &project) {
                *daily.bridge_skipped.entry(*date).or_default() += counters.entries;
                continue;
            }
            daily.buckets.add(*date, &project, counters);
        }
    }

    if !files.is_empty() && daily.files_scanned == 0 {
        return Err(Box::new(AgentUsageReport::error(
            AGENT,
            format!("no session files readable under {}", root.display()),
        )));
    }
    Ok(daily)
}

/// Reads one session file; `None` when it cannot be opened.
fn parse_file(file: &Path) -> Option<FileUsage> {
    let handle = std::fs::File::open(file).ok()?;
    let reader = BufReader::new(handle);
    let mut out = FileUsage::default();
    let mut seen: HashSet<String> = HashSet::new();
    // The session header comes first; until it is read the turns have no
    // cwd, so they are outside projects.
    let mut cwd = OUTSIDE_PROJECTS.to_string();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        // A malformed line is skipped, never fatal to the file.
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };

        match entry.get("type").and_then(Value::as_str) {
            Some("session") => {
                cwd = normalize_project_path(entry.get("cwd").and_then(Value::as_str).unwrap_or(""));
                continue;
            }
            Some("message") => {}
            _ => continue,
        }

        let Some(message) = entry.get("message") else { continue };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.get("usage").filter(|u| u.is_object()) else { continue };

        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }

        // A turn without a readable timestamp belongs to no day.
        let Some(date) = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Local).date_naive())
        else {
            continue;
        };

        let bridge = message.get("provider").and_then(Value::as_str) == Some(CLAUDE_BRIDGE);

        let model = message.get("model").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let counters = DailyCounters {
            input: get_u64(usage, "input"),
            output: get_u64(usage, "output"),
            cache_read: get_u64(usage, "cacheRead"),
            cache_write_5m: get_u64(usage, "cacheWrite"),
            cache_write_1h: 0,
            reasoning: get_u64(usage, "reasoning"),
            entries: 1,
            model: model.to_string(),
        };
        out.turns
            .entry((date, cwd.clone(), bridge, counters.model.clone()))
            .or_insert_with(|| DailyCounters { model: counters.model.clone(), ..Default::default() })
            .add(&counters);
    }
    Some(out)
}

/// The window report for the local dates in `[start, end)`. Each bucket is
/// priced with its own model; a cache write is priced as a 5-minute write,
/// since Pi does not split it by TTL. Each project is reported under its alias.
pub fn report(
    daily: &PiDaily,
    start: NaiveDate,
    end: NaiveDate,
    overrides: &pricing::PriceOverrides,
    aliases: &ProjectAliases,
) -> AgentUsageReport {
    // With no session file left, the ledger may still hold days to fold.
    if daily.files_scanned == 0 && daily.buckets.iter().next().is_none() {
        return AgentUsageReport::ok(AGENT, UsageTotals::default(), Vec::new(), "no session files yet");
    }

    let price = |c: &DailyCounters| {
        pricing::estimate_cost(&c.model, c.input, c.output, c.cache_read, c.cache_write_5m, c.cache_write_1h, overrides)
            .amount
    };
    let fold = daily.buckets.fold(start, end, price, CostBasis::ApiEquivalent, aliases);
    let in_window = |per_day: &BTreeMap<NaiveDate, u64>| -> u64 { per_day.range(start..end.max(start)).map(|(_, n)| n).sum() };
    let skipped = in_window(&daily.bridge_skipped);

    let mut detail = format!("{} projects - {} files", fold.projects.len(), daily.files_scanned);
    if skipped > 0 {
        detail.push_str(&format!(" - {skipped} claude-bridge turns already counted by Claude"));
    }
    AgentUsageReport::ok(AGENT, fold.totals, fold.projects, detail)
}

fn get_u64(node: &Value, name: &str) -> u64 {
    node.get(name).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{AgentStatus, RootPolicy};
    use chrono::TimeZone;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};

    /// Noon local time on `day` of September 2026, as the UTC Pi writes.
    fn noon(day: u32) -> String {
        Local.with_ymd_and_hms(2026, 9, day, 12, 0, 0).unwrap().to_utc().to_rfc3339()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    fn window() -> (DateTime<Local>, DateTime<Local>) {
        (
            Local.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            Local.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        )
    }

    fn header(cwd: &str) -> String {
        format!(r#"{{"type":"session","version":3,"id":"session-1","timestamp":"{}","cwd":"{cwd}"}}"#, noon(1))
    }

    #[allow(clippy::too_many_arguments)]
    fn turn(id: &str, ts: &str, provider: &str, model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
        let total = input + output + cache_read + cache_write;
        format!(
            r#"{{"type":"message","id":"{id}","parentId":null,"timestamp":"{ts}","message":{{"role":"assistant","provider":"{provider}","model":"{model}","timestamp":1,"usage":{{"input":{input},"output":{output},"cacheRead":{cache_read},"cacheWrite":{cache_write},"totalTokens":{total},"reasoning":3,"cost":{{"total":0.5}}}}}}}}"#
        )
    }

    fn codex_turn(id: &str, ts: &str, output: u64) -> String {
        turn(id, ts, "openai-codex", "gpt-5.5", 100, output, 0, 0)
    }

    fn bridge_turn(id: &str, ts: &str, output: u64) -> String {
        turn(id, ts, CLAUDE_BRIDGE, "claude-opus-5-5", 2, output, 0, 0)
    }

    /// A fixture home with a repository; the cache stops its walk at the home
    /// and has no temp dir, since fixtures live under the real one.
    struct Fixture {
        dir: TempDir,
        sessions: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let sessions = dir.path().join("sessions");
            fs::create_dir_all(&sessions).unwrap();
            Self { dir, sessions }
        }

        /// A repository folder under the fixture home, as a cwd string.
        fn repo(&self, name: &str) -> String {
            let repo = self.dir.path().join(name);
            fs::create_dir_all(repo.join(".git")).unwrap();
            repo.to_string_lossy().replace('\\', "/")
        }

        /// The project key the resolver gives the fixture repository `name`.
        fn key(&self, name: &str) -> String {
            let mut cache = self.cache();
            crate::usage::resolve_project_root(&self.repo(name), &mut cache).expect("a repository is a project")
        }

        fn session(&self, name: &str, cwd: &str, lines: &[String]) {
            let dir = self.sessions.join("--project--");
            fs::create_dir_all(&dir).unwrap();
            let mut content = header(cwd) + "\n";
            for line in lines {
                content.push_str(line);
                content.push('\n');
            }
            fs::write(dir.join(name), content).unwrap();
        }

        fn cache(&self) -> ProjectRootCache {
            ProjectRootCache::with_policy(RootPolicy { temp_dirs: Vec::new(), home: Some(self.dir.path().to_path_buf()) })
        }

        fn read(&self, claude: &DailyBuckets) -> AgentUsageReport {
            let (start, end) = window();
            read_usage(&self.sessions, claude, start, end, &pricing::PriceOverrides::default(), &mut self.cache())
        }
    }

    #[test]
    fn missing_root_is_not_installed() {
        let dir = tempdir().unwrap();
        let (start, end) = window();
        let report = read_usage(
            &dir.path().join("nope"),
            &DailyBuckets::default(),
            start,
            end,
            &pricing::PriceOverrides::default(),
            &mut ProjectRootCache::new(),
        );
        assert_eq!(report.agent, "pi");
        assert_eq!(report.status, AgentStatus::NotInstalled);
        assert_eq!(report.totals, UsageTotals::default());
    }

    #[test]
    fn an_empty_root_is_ok_with_empty_totals() {
        let fx = Fixture::new();
        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.status, AgentStatus::Ok);
        assert_eq!(report.totals, UsageTotals::default());
        assert!(report.projects.is_empty());
        assert_eq!(report.detail, "no session files yet");
    }

    /// Dedupe is per file: a repeated id inside one file counts once, the
    /// same id in another session file is a separate turn.
    #[test]
    fn a_repeated_entry_id_counts_once_per_file() {
        let fx = Fixture::new();
        let cwd = fx.repo("repo");
        fx.session("a.jsonl", &cwd, &[codex_turn("e1", &noon(10), 50), codex_turn("e1", &noon(10), 50)]);
        fx.session("b.jsonl", &cwd, &[codex_turn("e1", &noon(10), 50)]);

        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.totals.entries, 2);
        assert_eq!(report.totals.output_tokens, 100);
    }

    /// Only assistant turns that carry a usage object count: user and tool
    /// messages, other line types and malformed lines are ignored.
    #[test]
    fn only_assistant_turns_with_usage_count() {
        let fx = Fixture::new();
        let cwd = fx.repo("repo");
        let ts = noon(10);
        fx.session(
            "a.jsonl",
            &cwd,
            &[
                codex_turn("kept", &ts, 40),
                format!(r#"{{"type":"message","id":"u1","timestamp":"{ts}","message":{{"role":"user","usage":{{"input":999,"output":999}}}}}}"#),
                format!(r#"{{"type":"message","id":"t1","timestamp":"{ts}","message":{{"role":"toolResult","usage":{{"input":999,"output":999}}}}}}"#),
                format!(r#"{{"type":"message","id":"a2","timestamp":"{ts}","message":{{"role":"assistant","provider":"openai-codex","model":"gpt-5.5"}}}}"#),
                format!(r#"{{"type":"model_change","id":"m1","timestamp":"{ts}","usage":{{"input":999,"output":999}}}}"#),
                "not json but has \"usage\" in it".to_string(),
            ],
        );

        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.status, AgentStatus::Ok);
        assert_eq!(report.totals.entries, 1);
        assert_eq!(report.totals.output_tokens, 40);
        assert_eq!(report.totals.input_tokens, 100);
    }

    /// A `claude-bridge` turn counts only where Claude has no bucket for the
    /// same date and project; another provider counts there anyway.
    #[test]
    fn a_claude_bridge_turn_counts_only_where_claude_has_no_bucket() {
        let fx = Fixture::new();
        let shared = fx.repo("shared");
        let other = fx.repo("other");
        fx.session(
            "shared.jsonl",
            &shared,
            &[
                bridge_turn("b-same", &noon(10), 1000),
                bridge_turn("b-other-day", &noon(11), 20),
                codex_turn("c-same", &noon(10), 300),
            ],
        );
        fx.session("other.jsonl", &other, &[bridge_turn("b-other-project", &noon(10), 4)]);

        let mut claude = DailyBuckets::default();
        claude.add(date(10), &fx.key("shared"), &DailyCounters { output: 1000, entries: 1, ..Default::default() });

        let report = fx.read(&claude);
        assert_eq!(report.totals.entries, 3, "the bridge turn Claude already counts is left out");
        assert_eq!(report.totals.output_tokens, 324);
        let shared_row = report.projects.iter().find(|p| p.name == "shared").expect("shared row");
        assert_eq!(shared_row.totals.output_tokens, 320);
        let other_row = report.projects.iter().find(|p| p.name == "other").expect("other row");
        assert_eq!(other_row.totals.output_tokens, 4);
        assert!(report.detail.contains("1 claude-bridge turns already counted by Claude"), "{}", report.detail);

        let without_claude = fx.read(&DailyBuckets::default());
        assert_eq!(without_claude.totals.entries, 4, "with no Claude data every bridge turn counts");
        assert!(!without_claude.detail.contains("claude-bridge"), "{}", without_claude.detail);
    }

    /// The project comes from the session header's cwd, resolved like every
    /// other agent's: two cwds in one repository are one row, and a session
    /// outside any repository or with no cwd is in the "Outside projects" row.
    #[test]
    fn the_project_comes_from_the_session_header_cwd() {
        let fx = Fixture::new();
        let repo = fx.repo("repo");
        let sub = format!("{repo}/app/src-tauri");
        fs::create_dir_all(&sub).unwrap();
        let plain = fx.dir.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        fx.session("root.jsonl", &repo, &[codex_turn("a", &noon(10), 10)]);
        fx.session("sub.jsonl", &sub, &[codex_turn("b", &noon(10), 20)]);
        fx.session("plain.jsonl", &plain.to_string_lossy().replace('\\', "/"), &[codex_turn("c", &noon(10), 700)]);
        fx.session("nocwd.jsonl", "", &[codex_turn("d", &noon(10), 5)]);

        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.projects.len(), 2);
        let outside = &report.projects[0];
        assert_eq!((outside.name.as_str(), outside.path.as_str()), ("Outside projects", ""), "sorted by output");
        assert_eq!(outside.totals.output_tokens, 705, "the plain folder and the no-cwd turn");
        assert_eq!(report.projects[1].name, "repo");
        assert_eq!(report.projects[1].path, fx.key("repo"));
        assert_eq!(report.projects[1].totals.output_tokens, 30);
        assert_eq!(report.totals.output_tokens, 735, "every turn counts");
        assert_eq!(report.totals.entries, 4);
        assert_eq!(report.projects.iter().map(|p| p.totals.entries).sum::<u64>(), report.totals.entries);
        assert!(!report.detail.contains("not counted"), "{}", report.detail);
    }

    /// A session under the temp dir is outside projects too, and a
    /// claude-bridge turn there is skipped when Claude has the same day
    /// outside projects, like any other project key.
    #[test]
    fn a_temp_session_is_outside_projects_and_the_bridge_rule_applies_to_it() {
        let fx = Fixture::new();
        let temp = fx.dir.path().join("Temp");
        let run = temp.join("run-1");
        fs::create_dir_all(&run).unwrap();
        let run = run.to_string_lossy().replace('\\', "/");
        fx.session("t.jsonl", &run, &[codex_turn("c", &noon(10), 7), bridge_turn("b", &noon(10), 50)]);
        let mut cache =
            ProjectRootCache::with_policy(RootPolicy { temp_dirs: vec![temp], home: Some(fx.dir.path().to_path_buf()) });
        let (start, end) = window();
        let read = |claude: &DailyBuckets, cache: &mut ProjectRootCache| {
            read_usage(&fx.sessions, claude, start, end, &pricing::PriceOverrides::default(), cache)
        };

        let alone = read(&DailyBuckets::default(), &mut cache);
        assert_eq!(alone.totals.output_tokens, 57);
        assert_eq!(alone.projects.len(), 1);
        assert_eq!((alone.projects[0].name.as_str(), alone.projects[0].path.as_str()), ("Outside projects", ""));
        assert_eq!(alone.projects[0].totals, alone.totals);

        let mut claude = DailyBuckets::default();
        claude.add(date(10), "", &DailyCounters { output: 50, entries: 1, ..Default::default() });
        let with_claude = read(&claude, &mut cache);
        assert_eq!(with_claude.totals.output_tokens, 7, "Claude already counts that day outside projects");
        assert!(with_claude.detail.contains("1 claude-bridge turns already counted by Claude"), "{}", with_claude.detail);
    }

    #[test]
    fn turns_outside_the_window_are_not_counted() {
        let fx = Fixture::new();
        let cwd = fx.repo("repo");
        let (start, end) = window();
        fx.session(
            "a.jsonl",
            &cwd,
            &[
                codex_turn("in", &start.to_utc().to_rfc3339(), 50),
                codex_turn("before", &(start - chrono::Duration::seconds(1)).to_utc().to_rfc3339(), 999),
                codex_turn("after", &end.to_utc().to_rfc3339(), 999),
                codex_turn("no-ts", "not a time", 999),
            ],
        );

        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.totals.entries, 1);
        assert_eq!(report.totals.output_tokens, 50);
    }

    /// Cost is the API-equivalent price of the turn's model, with the cache
    /// write priced as a 5-minute write; an unpriced model adds no cost.
    #[test]
    fn cost_is_the_api_equivalent_price_by_model() {
        let fx = Fixture::new();
        let priced = fx.repo("priced");
        let unpriced = fx.repo("unpriced");
        fx.session("p.jsonl", &priced, &[turn("p", &noon(10), CLAUDE_BRIDGE, "claude-opus-5-5", 1_000_000, 2_000_000, 3_000_000, 4_000_000)]);
        fx.session("u.jsonl", &unpriced, &[turn("u", &noon(10), "openai-codex", "some-unknown-model", 10, 20, 0, 0)]);

        let overrides = pricing::PriceOverrides::default();
        let expected = pricing::estimate_cost("claude-opus-5-5", 1_000_000, 2_000_000, 3_000_000, 4_000_000, 0, &overrides)
            .amount
            .expect("claude-opus-5-5 is priced");
        let report = fx.read(&DailyBuckets::default());
        assert_eq!(report.totals.cost, Some(expected), "Pi's own cost field is not used");
        assert_eq!(report.totals.cost_basis, Some(CostBasis::ApiEquivalent));
        assert_eq!(report.totals.cache_read_tokens, 3_000_000);
        assert_eq!(report.totals.cache_write_tokens, 4_000_000);
        assert_eq!(report.totals.reasoning_tokens, 6);

        let priced_row = report.projects.iter().find(|p| p.name == "priced").expect("priced row");
        assert_eq!(priced_row.model, "claude-opus-5-5");
        let unpriced_row = report.projects.iter().find(|p| p.name == "unpriced").expect("unpriced row");
        assert_eq!(unpriced_row.totals.cost, None);
        assert_eq!(unpriced_row.totals.cost_basis, None);
    }
}
