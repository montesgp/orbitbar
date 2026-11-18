//! Claude usage reader: every `*.jsonl` under `~/.claude/projects`, de-duped
//! and aggregated into daily buckets.
//!
//! Subagent files are billed too, so every file counts, not just the newest
//! one. The store keeps more than one copy of many assistant messages
//! (original plus a snapshot copy), so entries are de-duplicated per file by
//! `requestId`, falling back to `message.id` and then `uuid` - summing raw
//! doubles every counter.
//!
//! Every entry carries its own timestamp, so [`read_daily`] sorts it into a
//! `(local date, project key)` bucket (see `usage::daily`) and [`report`]
//! folds the window's dates. The totals and the per-project breakdown come
//! from the same buckets, so the per-project split always matches what the
//! totals claim for the same window. Sessions outside any project (a cwd under the OS temp dir, or an existing
//! folder that is not inside a repository; see `resolve_project_root`) are
//! left out of the totals and the project rows alike, so that invariant holds.

use crate::usage::daily::{DailyBuckets, DailyCounters};
use crate::usage::{
    find_jsonl_files, normalize_project_path, pricing, resolve_project_root, AgentUsageReport, CostBasis,
    ProjectRootCache, UsageTotals,
};
use chrono::{DateTime, Local, NaiveDate};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

pub const AGENT: &str = "claude";

/// Everything one scan of the store found, before any window is applied.
#[derive(Debug, Clone, Default)]
pub struct ClaudeDaily {
    /// Every entry, by local date and resolved project root (`""`: no cwd).
    pub buckets: DailyBuckets,
    pub files_scanned: u64,
    /// Entries whose cwd is excluded (see `resolve_project_root`), per local
    /// date. They are in no bucket, so neither the totals nor the rows count them.
    pub excluded: BTreeMap<NaiveDate, u64>,
}

/// Reads month/window usage from every claude session file under `root`,
/// pricing against `overrides` layered on top of the built-in table: a
/// [`read_daily`] scan folded over the local dates in `[start, end)`.
/// `collect_usage` calls the two steps itself to hand Claude's buckets to pi.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_usage(
    root: &Path,
    start: DateTime<Local>,
    end: DateTime<Local>,
    overrides: &pricing::PriceOverrides,
    root_cache: &mut ProjectRootCache,
) -> AgentUsageReport {
    match read_daily(root, root_cache) {
        Ok(daily) => report(&daily, start.date_naive(), end.date_naive(), overrides),
        Err(report) => *report,
    }
}

/// Scans every session file under `root` into daily buckets. Per-project
/// buckets are keyed by resolved project root (see `resolve_project_root`),
/// not the raw session cwd, so two cwds inside the same repository share a
/// bucket. `Err` carries the finished report when there is nothing to fold:
/// the store is missing, or no session file could be read.
pub fn read_daily(root: &Path, root_cache: &mut ProjectRootCache) -> Result<ClaudeDaily, Box<AgentUsageReport>> {
    if !root.exists() {
        return Err(Box::new(AgentUsageReport::not_installed(
            AGENT,
            format!("missing directory: {}", root.display()),
        )));
    }

    let files = find_jsonl_files(root);
    let mut daily = ClaudeDaily::default();

    for file in &files {
        let Ok(handle) = std::fs::File::open(file) else { continue };
        daily.files_scanned += 1;
        let reader = BufReader::new(handle);

        // Dedupe is per file: the same requestId can appear in more than one
        // session file (a resumed session, a subagent transcript), and those
        // are separate billable events.
        let mut seen: HashSet<String> = HashSet::new();

        for line in reader.lines() {
            let Ok(line) = line else { continue };
            if !line.contains("\"usage\"") {
                continue;
            }
            // A malformed line is skipped, never fatal to the file.
            let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };

            let usage = entry
                .get("message")
                .and_then(|m| m.get("usage"))
                .or_else(|| entry.get("usage"));
            let Some(usage) = usage else { continue };

            let key = entry
                .get("requestId")
                .and_then(Value::as_str)
                .or_else(|| entry.get("message").and_then(|m| m.get("id")).and_then(Value::as_str))
                .or_else(|| entry.get("uuid").and_then(Value::as_str));
            if let Some(key) = key {
                if !seen.insert(key.to_string()) {
                    continue;
                }
            }

            // An entry without a readable timestamp belongs to no day, so no
            // window can count it.
            let Some(date) = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Local).date_naive())
            else {
                continue;
            };

            let cwd = entry.get("cwd").and_then(Value::as_str).unwrap_or("");
            let cwd = normalize_project_path(cwd);
            // An entry with no cwd stays in the totals without a project row.
            let project_key = if cwd.is_empty() {
                cwd
            } else {
                match resolve_project_root(&cwd, root_cache) {
                    Some(root) => root,
                    None => {
                        *daily.excluded.entry(date).or_default() += 1;
                        continue;
                    }
                }
            };

            let model = entry
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|m| !m.is_empty() && !m.starts_with('<'))
                .unwrap_or("");

            let (cache_write_5m, cache_write_1h) = extract_cache_write(usage);
            let counters = DailyCounters {
                input: get_u64(usage, "input_tokens"),
                output: get_u64(usage, "output_tokens"),
                cache_read: get_u64(usage, "cache_read_input_tokens"),
                cache_write_5m,
                cache_write_1h,
                reasoning: usage
                    .get("output_tokens_details")
                    .map(|d| get_u64(d, "thinking_tokens"))
                    .unwrap_or(0),
                entries: 1,
                model: model.to_string(),
            };
            daily.buckets.add(date, &project_key, &counters);
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

/// The window report for the local dates in `[start, end)`. Each bucket is
/// priced with its own model and the amounts summed (see `DailyBuckets::fold`).
pub fn report(
    daily: &ClaudeDaily,
    start: NaiveDate,
    end: NaiveDate,
    overrides: &pricing::PriceOverrides,
) -> AgentUsageReport {
    // With no session file left, the ledger may still hold days to fold.
    if daily.files_scanned == 0 && daily.buckets.iter().next().is_none() {
        return AgentUsageReport::ok(AGENT, UsageTotals::default(), Vec::new(), "no session files yet");
    }

    let price = |c: &DailyCounters| {
        pricing::estimate_cost(&c.model, c.input, c.output, c.cache_read, c.cache_write_5m, c.cache_write_1h, overrides)
            .amount
    };
    let fold = daily.buckets.fold(start, end, price, CostBasis::ApiEquivalent);
    let excluded: u64 = daily.excluded.range(start..end.max(start)).map(|(_, n)| n).sum();

    let mut detail = format!("{} projects - {} files", fold.projects.len(), daily.files_scanned);
    if excluded > 0 {
        detail.push_str(&format!(" - {excluded} entries outside any project not counted"));
    }
    AgentUsageReport::ok(AGENT, fold.totals, fold.projects, detail)
}

fn get_u64(node: &Value, name: &str) -> u64 {
    node.get(name).and_then(Value::as_u64).unwrap_or(0)
}

/// Splits `usage`'s cache-write tokens into (5-minute TTL, 1-hour TTL)
/// buckets. Recent claude records carry `cache_creation.ephemeral_5m_input_tokens`
/// / `ephemeral_1h_input_tokens`; older records only have the flat
/// `cache_creation_input_tokens`, which this treats as a 5-minute write since
/// that was the only TTL that existed before the split field shipped.
fn extract_cache_write(usage: &Value) -> (u64, u64) {
    if let Some(cache_creation) = usage.get("cache_creation") {
        let five_m = get_u64(cache_creation, "ephemeral_5m_input_tokens");
        let one_h = get_u64(cache_creation, "ephemeral_1h_input_tokens");
        return (five_m, one_h);
    }
    (get_u64(usage, "cache_creation_input_tokens"), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs;
    use tempfile::tempdir;

    fn line(request_id: &str, ts: &str, cwd: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"requestId":"{request_id}","timestamp":"{ts}","cwd":"{cwd}","message":{{"id":"msg_{request_id}","model":"claude-opus-5-5","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":10,"cache_creation_input_tokens":5,"output_tokens_details":{{"thinking_tokens":2}}}}}}}}"#
        )
    }

    fn window() -> (DateTime<Local>, DateTime<Local>) {
        (
            Local.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            Local.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn missing_directory_is_not_installed() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        let (start, end) = window();
        let report = read_usage(&missing, start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.status, crate::usage::AgentStatus::NotInstalled);
    }

    #[test]
    fn duplicate_entries_are_deduplicated_by_request_id() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = format!(
            "{}\n{}\n",
            line("req1", "2026-09-15T10:00:00Z", "C:/repos/orbitbar", 100, 50),
            line("req1", "2026-09-15T10:00:00Z", "C:/repos/orbitbar", 100, 50),
        );
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.status, crate::usage::AgentStatus::Ok);
        assert_eq!(report.totals.entries, 1, "the duplicate copy must not be counted twice");
        assert_eq!(report.totals.output_tokens, 50);
    }

    #[test]
    fn window_filtering_excludes_entries_outside_the_range() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let (start, end) = window();
        // Timestamps are derived from the window's own local boundaries
        // (converted to the UTC the store actually writes) rather than
        // hardcoded UTC strings, so this test is correct under any local
        // timezone offset, not just UTC.
        let in_window_ts = start.to_utc().to_rfc3339();
        let before_ts = (start - chrono::Duration::seconds(1)).to_utc().to_rfc3339();
        let after_ts = end.to_utc().to_rfc3339();
        let content = format!(
            "{}\n{}\n{}\n",
            line("in-window", &in_window_ts, "C:/repos/orbitbar", 100, 50),
            line("before", &before_ts, "C:/repos/orbitbar", 999, 999),
            line("after", &after_ts, "C:/repos/orbitbar", 999, 999),
        );
        fs::write(project.join("session.jsonl"), content).unwrap();

        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.totals.entries, 1);
        assert_eq!(report.totals.output_tokens, 50);
    }

    #[test]
    fn a_malformed_line_is_skipped_not_fatal() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = format!(
            "not json at all but has \"usage\" in it\n{}\n",
            line("ok1", "2026-09-15T10:00:00Z", "C:/repos/orbitbar", 100, 50),
        );
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.status, crate::usage::AgentStatus::Ok);
        assert_eq!(report.totals.entries, 1);
    }

    /// Fixtures live under the real temp dir, which the system policy would
    /// exclude; this cache stops its walk at `home` and has no temp dir.
    fn fixture_cache(home: &std::path::Path) -> ProjectRootCache {
        ProjectRootCache::with_policy(crate::usage::RootPolicy {
            temp_dirs: Vec::new(),
            home: Some(home.to_path_buf()),
        })
    }

    #[test]
    fn project_breakdown_sums_per_cwd_and_sorts_by_output_desc() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = format!(
            "{}\n{}\n",
            line("req-a", "2026-09-15T10:00:00Z", &dir.path().join("small").to_string_lossy().replace('\\', "/"), 100, 10),
            line("req-b", "2026-09-15T10:01:00Z", &dir.path().join("big").to_string_lossy().replace('\\', "/"), 100, 500),
        );
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut fixture_cache(dir.path()));
        assert_eq!(report.projects.len(), 2);
        assert_eq!(report.projects[0].name, "big");
        assert_eq!(report.projects[1].name, "small");
    }

    /// Two different session cwds inside the same repository (the repo root
    /// and a subfolder) must aggregate into one project row instead of two,
    /// once the resolved project root is used as the bucket key.
    #[test]
    fn two_cwds_in_the_same_repo_merge_into_one_project_row() {
        let dir = tempdir().unwrap();
        let repo_root = dir.path().join("repo");
        fs::create_dir_all(repo_root.join(".git")).unwrap();
        let subfolder = repo_root.join("app").join("src-tauri");
        fs::create_dir_all(&subfolder).unwrap();
        // JSON string literals can't carry raw Windows backslashes; forward
        // slashes round-trip through normalize_project_path either way.
        let root_cwd = repo_root.to_string_lossy().replace('\\', "/");
        let sub_cwd = subfolder.to_string_lossy().replace('\\', "/");

        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let content = format!(
            "{}\n{}\n",
            line("req-a", "2026-09-15T10:00:00Z", &root_cwd, 100, 10),
            line("req-b", "2026-09-15T10:01:00Z", &sub_cwd, 100, 20),
        );
        fs::write(sessions.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(
            &sessions,
            start,
            end,
            &pricing::PriceOverrides::default(),
            &mut fixture_cache(dir.path()),
        );
        assert_eq!(report.projects.len(), 1, "both cwds share the same repo root");
        assert_eq!(report.projects[0].totals.output_tokens, 30, "tokens from both cwds must sum");
        assert_eq!(report.projects[0].name, "repo");
    }

    #[test]
    fn unpriced_model_leaves_cost_none() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = r#"{"requestId":"req1","timestamp":"2026-09-15T10:00:00Z","cwd":"C:/repos/orbitbar","message":{"id":"msg_req1","model":"some-unknown-model","usage":{"input_tokens":100,"output_tokens":50}}}"#.to_string() + "\n";
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.totals.cost, None, "a model without a price row must never invent a cost");
    }

    #[test]
    fn empty_directory_with_no_session_files_is_ok_with_empty_totals() {
        let dir = tempdir().unwrap();
        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert_eq!(report.status, crate::usage::AgentStatus::Ok);
        assert_eq!(report.totals.entries, 0);
        assert!(report.projects.is_empty());
    }

    #[test]
    fn a_priced_model_carries_the_api_equivalent_cost_basis() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = line("req1", "2026-09-15T10:00:00Z", "C:/repos/orbitbar", 100, 50) + "\n";
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        assert!(report.totals.cost.is_some());
        assert_eq!(report.totals.cost_basis, Some(crate::usage::CostBasis::ApiEquivalent));
    }

    /// A record carrying the TTL-split `cache_creation` object must price its
    /// 1-hour tokens at the 1-hour rate, not fold them into the flat
    /// `cache_creation_input_tokens` 5-minute fallback.
    #[test]
    fn ephemeral_1h_cache_write_is_priced_separately_from_5m() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = r#"{"requestId":"req1","timestamp":"2026-09-15T10:00:00Z","cwd":"C:/repos/orbitbar","message":{"id":"msg_req1","model":"claude-opus-5","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000000}}}}"#.to_string() + "\n";
        fs::write(project.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut ProjectRootCache::new());
        // claude-opus-5 cache_write_1h = 10.0 per 1M, not 6.25 (the 5m rate).
        assert_eq!(report.totals.cost, Some(10.0));
    }

    fn priced_line(request_id: &str, ts: &str, cwd: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"requestId":"{request_id}","timestamp":"{ts}","cwd":"{cwd}","message":{{"id":"msg_{request_id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":1000000,"cache_creation":{{"ephemeral_5m_input_tokens":1000000,"ephemeral_1h_input_tokens":1000000}}}}}}}}"#
        )
    }

    /// Noon local time on `day` of September 2026, as the UTC the store writes.
    fn noon(day: u32) -> String {
        Local.with_ymd_and_hms(2026, 9, day, 12, 0, 0).unwrap().to_utc().to_rfc3339()
    }

    /// A window spanning two days with one model gives the totals the single
    /// window-wide sum gave: the counters add up, and the per-day costs sum to
    /// the price of the summed counters.
    #[test]
    fn a_window_over_two_days_gives_the_same_totals_as_one_sum() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = repo.to_string_lossy().replace('\\', "/");
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let content = format!(
            "{}\n{}\n",
            priced_line("day-1", &noon(14), &cwd, "claude-opus-5", 1_000_000, 2_000_000),
            priced_line("day-2", &noon(15), &cwd, "claude-opus-5", 3_000_000, 1_000_000),
        );
        fs::write(sessions.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let overrides = pricing::PriceOverrides::default();
        let report = read_usage(&sessions, start, end, &overrides, &mut fixture_cache(dir.path()));

        let expected = pricing::estimate_cost(
            "claude-opus-5",
            4_000_000,
            3_000_000,
            2_000_000,
            2_000_000,
            2_000_000,
            &overrides,
        )
        .amount
        .expect("claude-opus-5 is priced");
        assert_eq!(report.totals.entries, 2);
        assert_eq!(report.totals.input_tokens, 4_000_000);
        assert_eq!(report.totals.output_tokens, 3_000_000);
        assert_eq!(report.totals.cache_read_tokens, 2_000_000);
        assert_eq!(report.totals.cache_write_tokens, 4_000_000);
        assert_eq!(report.totals.cost, Some(expected));
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].totals, report.totals);
        assert_eq!(report.projects[0].model, "claude-opus-5");
    }

    /// Cost is priced per (day, project) bucket: a later bucket with an
    /// unpriced model no longer erases the cost of the earlier, priced one,
    /// in the agent totals or in a project whose model changed between days.
    #[test]
    fn an_unpriced_bucket_does_not_hide_the_cost_of_the_priced_ones() {
        let dir = tempdir().unwrap();
        let priced_repo = dir.path().join("priced");
        let unpriced_repo = dir.path().join("unpriced");
        fs::create_dir_all(priced_repo.join(".git")).unwrap();
        fs::create_dir_all(unpriced_repo.join(".git")).unwrap();
        let cwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let content = format!(
            "{}\n{}\n{}\n",
            priced_line("a", &noon(14), &cwd(&priced_repo), "claude-opus-5", 0, 1_000_000),
            priced_line("b", &noon(15), &cwd(&priced_repo), "some-unknown-model", 0, 1_000_000),
            priced_line("c", &noon(15), &cwd(&unpriced_repo), "some-unknown-model", 0, 5),
        );
        fs::write(sessions.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let overrides = pricing::PriceOverrides::default();
        let report = read_usage(&sessions, start, end, &overrides, &mut fixture_cache(dir.path()));

        let day_one = pricing::estimate_cost("claude-opus-5", 0, 1_000_000, 1_000_000, 1_000_000, 1_000_000, &overrides)
            .amount
            .expect("claude-opus-5 is priced");
        assert_eq!(report.totals.output_tokens, 2_000_005);
        assert_eq!(report.totals.cost, Some(day_one));
        assert_eq!(report.totals.cost_basis, Some(crate::usage::CostBasis::ApiEquivalent));

        let priced = report.projects.iter().find(|p| p.name == "priced").expect("priced row");
        assert_eq!(priced.totals.cost, Some(day_one));
        assert_eq!(priced.model, "some-unknown-model");
        let unpriced = report.projects.iter().find(|p| p.name == "unpriced").expect("unpriced row");
        assert_eq!(unpriced.totals.cost, None);
        assert_eq!(unpriced.totals.cost_basis, None);
    }

    /// Two models in one day and project are priced each at its own rate, and
    /// the row names the model with the most output tokens.
    #[test]
    fn a_day_mixing_two_models_prices_each_at_its_own_rate() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = repo.to_string_lossy().replace('\\', "/");
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let content = format!(
            "{}\n{}\n",
            priced_line("opus", &noon(14), &cwd, "claude-opus-5", 0, 2_000_000),
            priced_line("sonnet", &noon(14), &cwd, "claude-sonnet-5", 0, 1_000_000),
        );
        fs::write(sessions.join("session.jsonl"), content).unwrap();

        let (start, end) = window();
        let overrides = pricing::PriceOverrides::default();
        let report = read_usage(&sessions, start, end, &overrides, &mut fixture_cache(dir.path()));

        let price = |model: &str, output: u64| {
            pricing::estimate_cost(model, 0, output, 1_000_000, 1_000_000, 1_000_000, &overrides).amount.expect("priced")
        };
        let expected = ((price("claude-opus-5", 2_000_000) + price("claude-sonnet-5", 1_000_000)) * 10_000.0).round() / 10_000.0;
        assert_eq!(report.totals.entries, 2);
        assert_eq!(report.totals.output_tokens, 3_000_000);
        assert_eq!(report.totals.cost, Some(expected), "each model at its own rate");
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].totals, report.totals);
        assert_eq!(report.projects[0].model, "claude-opus-5", "the model with the most output");
    }

    /// Entries whose cwd is scratch (under the temp dir) or a folder outside
    /// any repository are not counted anywhere: not in the project rows and
    /// not in the totals, so the split still sums to the totals. An entry with
    /// no cwd cannot be judged and stays in the totals.
    #[test]
    fn entries_outside_any_project_are_left_out_of_totals_and_rows() {
        use crate::usage::RootPolicy;
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let plain = dir.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        let cwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let content = format!(
            "{}\n{}\n{}\n{}\n",
            line("req-a", "2026-09-15T10:00:00Z", &cwd(&dir.path().join("kept-app").join("app")), 100, 10),
            line("req-b", "2026-09-15T10:01:00Z", &cwd(&temp.join("gentle-ai-codex-reviewer-1")), 100, 500),
            line("req-c", "2026-09-15T10:02:00Z", &cwd(&plain), 100, 700),
            line("req-d", "2026-09-15T10:03:00Z", "", 100, 5),
        );
        fs::write(project.join("session.jsonl"), content).unwrap();

        let mut cache = ProjectRootCache::with_policy(RootPolicy {
            temp_dirs: vec![temp],
            home: Some(dir.path().to_path_buf()),
        });
        let (start, end) = window();
        let report = read_usage(dir.path(), start, end, &pricing::PriceOverrides::default(), &mut cache);

        assert_eq!(report.totals.entries, 2, "the kept entry and the one with no cwd");
        assert_eq!(report.totals.output_tokens, 15);
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].name, "kept-app");
        assert_eq!(report.projects[0].totals.output_tokens, 10);
        assert!(report.detail.contains("2 entries outside any project not counted"), "{}", report.detail);
    }
}
