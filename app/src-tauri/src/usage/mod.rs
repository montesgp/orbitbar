//! Token-spend usage readers: claude, codex, opencode and pi, each read from the
//! agent's own local files, no external service or program involved. The
//! per-agent modules below own the deduplication, cumulative-vs-summed
//! counter handling and windowing each store needs.
//!
//! One function, [`collect_usage`], returns a per-agent breakdown for a time
//! window plus a per-project split sorted by output tokens. A broken or
//! missing store on one agent never hides the others: each carries its own
//! [`AgentStatus`].

pub mod claude;
pub mod codex;
pub mod daily;
pub mod ledger;
pub mod opencode;
pub mod pi;
pub mod pricing;

use chrono::{DateTime, Datelike, Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The time window a snapshot is computed over. `Today` is the default: the
/// usage panel always opens on it (the selector is never persisted) because
/// it is the fastest view to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TimeWindow {
    #[default]
    Today,
    Last7Days,
    Last30Days,
    ThisMonth,
    Last365Days,
}

/// Resolves a window to a local `[start, end)` instant pair. All windows are
/// full-day boundaries at local midnight, so "last 7 days" always means 7
/// whole days including today, never a rolling 168-hour clock, and "365 days"
/// is today plus the previous 364 days.
pub fn window_range(window: TimeWindow, now: DateTime<Local>) -> (DateTime<Local>, DateTime<Local>) {
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always a valid time");
    let today_start = Local.from_local_datetime(&today_start).single().unwrap_or(now);
    let tomorrow_start = today_start + Duration::days(1);

    match window {
        TimeWindow::Today => (today_start, tomorrow_start),
        TimeWindow::Last7Days => (today_start - Duration::days(6), tomorrow_start),
        TimeWindow::Last30Days => (today_start - Duration::days(29), tomorrow_start),
        TimeWindow::ThisMonth => {
            let month_start_naive = today_start
                .date_naive()
                .with_day(1)
                .expect("day 1 always exists");
            let month_start = Local
                .from_local_datetime(&month_start_naive.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .unwrap_or(today_start);
            let next_month_start_naive = if month_start_naive.month() == 12 {
                month_start_naive
                    .with_year(month_start_naive.year() + 1)
                    .and_then(|d| d.with_month(1))
            } else {
                month_start_naive.with_month(month_start_naive.month() + 1)
            }
            .expect("adjacent month always exists");
            let next_month_start = Local
                .from_local_datetime(&next_month_start_naive.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .unwrap_or(month_start);
            (month_start, next_month_start)
        }
        TimeWindow::Last365Days => {
            // Counted in calendar days, not 364 x 24 hours, so a daylight-
            // saving change inside the year never moves the start off midnight.
            let start_naive = today_start.date_naive() - Duration::days(364);
            let start = Local
                .from_local_datetime(&start_naive.and_hms_opt(0, 0, 0).unwrap())
                .earliest()
                .unwrap_or(today_start - Duration::days(364));
            (start, tomorrow_start)
        }
    }
}

/// Strips the `\\?\` verbatim prefix `std::fs::canonicalize` adds on Windows
/// (`\\?\UNC\server\share` becomes `\\server\share`), leaving any other path as is.
pub fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
}

/// `path` as the file system resolves it (junctions, symlinks, 8.3 short
/// names, letter case), or `path` itself when it cannot be resolved.
fn resolved_dir(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(resolved) => PathBuf::from(strip_verbatim_prefix(&resolved.to_string_lossy())),
        Err(_) => path.to_path_buf(),
    }
}

/// Per-agent status: `Ok` even when the window is empty of activity, `NotInstalled`
/// when the store's root directory/file is missing (the agent was never
/// used, or is not installed on this machine), `Error` for anything else
/// (locked file, unreadable database, ...). A broken store never reports
/// zeroes as if that were the truth.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
pub enum AgentStatus {
    Ok,
    NotInstalled,
    Error(String),
}

/// Whether `UsageTotals.cost` is money the provider actually reported
/// (opencode's own `session.cost` column) or an estimate computed from
/// official list prices times the real token counts (claude, codex). Codex
/// usage read through a ChatGPT subscription is not billed per token at all;
/// `ApiEquivalent` says "this is what the same tokens would cost on the pay-
/// per-token API", never implies a bill the user actually received.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CostBasis {
    ApiEquivalent,
    Reported,
}

/// Aggregated counters for one agent (or one project inside an agent), over
/// the window. `cost` is `None` when the store carries no cost field and no
/// price row exists for the model(s) involved - never a guessed number.
/// `cost_basis` is `None` exactly when `cost` is `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    pub entries: u64,
}

/// One project's totals inside an agent's window, keyed by the normalized
/// session `cwd`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUsage {
    pub path: String,
    pub name: String,
    pub model: String,
    pub totals: UsageTotals,
}

/// One agent's full report: overall totals plus the per-project breakdown,
/// sorted by output tokens descending (busiest project first).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageReport {
    pub agent: String,
    pub status: AgentStatus,
    pub totals: UsageTotals,
    pub projects: Vec<ProjectUsage>,
    pub detail: String,
}

impl AgentUsageReport {
    pub fn not_installed(agent: &str, detail: impl Into<String>) -> Self {
        Self {
            agent: agent.to_string(),
            status: AgentStatus::NotInstalled,
            totals: UsageTotals::default(),
            projects: Vec::new(),
            detail: detail.into(),
        }
    }

    pub fn error(agent: &str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            agent: agent.to_string(),
            status: AgentStatus::Error(detail.clone()),
            totals: UsageTotals::default(),
            projects: Vec::new(),
            detail,
        }
    }

    pub fn ok(agent: &str, totals: UsageTotals, mut projects: Vec<ProjectUsage>, detail: impl Into<String>) -> Self {
        projects.sort_by_key(|p| std::cmp::Reverse(p.totals.output_tokens));
        Self {
            agent: agent.to_string(),
            status: AgentStatus::Ok,
            totals,
            projects,
            detail: detail.into(),
        }
    }
}

/// The agents' store roots, resolved from a home directory. Readers take their
/// root as a parameter (never resolve it themselves) so tests can point them
/// at fixtures.
#[derive(Debug, Clone)]
pub struct UsagePaths {
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub opencode_db: PathBuf,
    pub pi_dir: PathBuf,
}

/// Resolves the store roots under a home directory. `home_override`
/// lets tests and the parity harness point at a fixture home instead of the
/// real one; the ordinary caller passes `None` and gets `dirs::home_dir()`.
///
/// The layout is home-relative and identical across Windows, macOS and
/// Linux: opencode is a cross-platform CLI that writes to
/// `<home>/.local/share/opencode/opencode.db` on every OS, not to each
/// platform's "special" data directory, so this mirrors that literal path
/// rather than asking `dirs::data_dir()` for a platform-specific one.
pub fn resolve_paths(home_override: Option<&Path>) -> Option<UsagePaths> {
    let home = match home_override {
        Some(p) => p.to_path_buf(),
        None => dirs::home_dir()?,
    };
    Some(UsagePaths {
        claude_dir: home.join(".claude").join("projects"),
        codex_dir: home.join(".codex").join("sessions"),
        opencode_db: home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db"),
        pi_dir: home.join(".pi").join("agent").join("sessions"),
    })
}

/// The canonical form of a project path: trimmed, written with the host OS
/// separator, trailing separator removed, so the same cwd string always
/// buckets to the same project key and stays a path the OS can walk.
///
/// On Windows, where agents mix `/` and `\`, every `/` becomes `\`. On macOS
/// and Linux the path is kept as written: `/` is the only separator there and
/// `\` is a legal file-name character, so rewriting `/` into `\` would yield
/// a path `Path::parent` cannot walk up (project-root resolution then silently
/// fell back to the raw cwd on those systems).
pub fn normalize_project_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized = if cfg!(windows) {
        trimmed.replace('/', "\\")
    } else {
        trimmed.to_string()
    };
    let sep = std::path::MAIN_SEPARATOR;
    let without_trailing = normalized.trim_end_matches([sep, ' ']);
    if without_trailing.is_empty() {
        // The filesystem root itself ("/"): keep it rather than erase it.
        sep.to_string()
    } else {
        without_trailing.to_string()
    }
}

/// The display name for a project key: its last path segment, falling back
/// to the whole key when there is no separator (e.g. a bare drive root).
pub fn project_display_name(key: &str) -> String {
    key.rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(key)
        .to_string()
}

/// Memoizes [`resolve_project_root`] lookups across one snapshot. Claude,
/// codex and opencode all report many sessions that share the same cwd
/// (subagents, resumed sessions, several projects under one parent), so a
/// shared cache means the filesystem is walked at most once per distinct
/// cwd across all the readers, not once per session.
pub struct ProjectRootCache {
    policy: RootPolicy,
    resolved: HashMap<String, Option<String>>,
}

/// The two filesystem boundaries the resolver depends on. Injected (instead of
/// read from the environment inside the walk) so tests are deterministic.
#[derive(Debug, Clone, Default)]
pub struct RootPolicy {
    /// Directories whose contents are scratch work, not projects.
    pub temp_dirs: Vec<PathBuf>,
    /// The walk for a repository never climbs into or past this directory.
    pub home: Option<PathBuf>,
}

impl RootPolicy {
    /// The real machine: the OS temp dir and the user's home. The temp dir is
    /// listed as the OS reports it and, when that differs, in its resolved
    /// form too (Windows may report an 8.3 short name such as `PATRI~1` where
    /// an agent recorded the long one; macOS reports `/var` for `/private/var`).
    pub fn system() -> Self {
        let mut temp_dirs = vec![std::env::temp_dir()];
        let resolved = resolved_dir(&temp_dirs[0]);
        if resolved != temp_dirs[0] {
            temp_dirs.push(resolved);
        }
        Self { temp_dirs, home: dirs::home_dir() }
    }
}

impl ProjectRootCache {
    pub fn new() -> Self {
        Self::with_policy(RootPolicy::system())
    }

    pub fn with_policy(policy: RootPolicy) -> Self {
        Self { policy, resolved: HashMap::new() }
    }

    /// Number of distinct cwds resolved so far (tests only).
    #[cfg(test)]
    fn cached_lookups(&self) -> usize {
        self.resolved.len()
    }
}

impl Default for ProjectRootCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves the project root for a session `cwd`, or `None` when the session
/// does not belong to any project and must not be counted at all (neither in
/// a project row nor in the agent totals). In order:
///
/// 1. A cwd under the OS temp dir is scratch work (for example a review
///    subprocess), excluded whether or not it still exists.
/// 2. Otherwise the root is the nearest ancestor directory (including `cwd`
///    itself, or its nearest surviving ancestor when `cwd` was deleted) that
///    contains a `.git` entry; the walk never climbs into the home directory.
///    The walk runs on the path as the file system resolves it (junctions,
///    symlinks, 8.3 short names), so `C:\repositories\x` reached through a
///    junction to `R:\repositories\x` gives the same key as the target.
/// 3. With no repository found: an existing `cwd` (the home directory, a plain
///    folder) is excluded; a missing `cwd` belongs to a repository that was
///    renamed or deleted, and is keyed by the topmost missing directory, the
///    child of the deepest surviving ancestor, so every subfolder of the
///    vanished repo shares one row. If no ancestor survives at all, the raw
///    cwd is the key.
///
/// The repository walk is what turns `app`, `app/src-tauri` and `.claude/worktrees/agent-foo` -
/// three different session cwds inside the same repository - into one
/// per-project row instead of three.
///
/// When `.git` is a FILE rather than a directory - a git worktree's
/// `gitdir: <main>/.git/worktrees/<name>` pointer - this resolves to the
/// MAIN repository root, not the worktree's own checkout, so a session run
/// from a worktree rolls up into the same project as the primary checkout.
///
/// A session recorded on a different machine than the one reading it now has
/// a missing cwd and falls under rule 3.
pub fn resolve_project_root(cwd: &str, cache: &mut ProjectRootCache) -> Option<String> {
    if let Some(cached) = cache.resolved.get(cwd) {
        return cached.clone();
    }
    let resolved = resolve_project_root_uncached(cwd, &cache.policy);
    cache.resolved.insert(cwd.to_string(), resolved.clone());
    resolved
}

/// True when `path` is `base` or lies below it, compared on normalized paths
/// by whole components (`Temp-projects` is not under `Temp`) and without regard
/// to letter case on Windows.
fn is_within(path: &str, base: &Path) -> bool {
    let base = normalize_project_path(&base.to_string_lossy());
    if base.is_empty() {
        return false;
    }
    let (path, base) = if cfg!(windows) {
        (path.to_lowercase(), base.to_lowercase())
    } else {
        (path.to_string(), base)
    };
    path == base
        || path
            .strip_prefix(&base)
            .is_some_and(|rest| rest.starts_with(std::path::MAIN_SEPARATOR))
}

fn resolve_project_root_uncached(cwd: &str, policy: &RootPolicy) -> Option<String> {
    let normalized_cwd = normalize_project_path(cwd);
    if normalized_cwd.is_empty() {
        return Some(normalized_cwd);
    }

    if policy.temp_dirs.iter().any(|temp| is_within(&normalized_cwd, temp)) {
        return None;
    }

    // A deleted cwd (a removed worktree, a deleted subfolder) starts the walk
    // from its nearest surviving ancestor, so it still rolls up into the repo
    // that contained it. `top_missing` remembers the highest directory that
    // no longer exists, the key when no repo is found above it.
    let mut start = Path::new(&normalized_cwd);
    let mut top_missing: Option<&Path> = None;
    while !start.exists() {
        top_missing = Some(start);
        match start.parent() {
            Some(parent) => start = parent,
            None => return Some(normalized_cwd),
        }
    }

    // From here the walk runs on resolved paths: the surviving directory
    // through junctions and symlinks, and the temp and home boundaries in
    // their resolved form too, so a link never slips past either.
    let resolved_start = PathBuf::from(normalize_project_path(&resolved_dir(start).to_string_lossy()));
    // The missing part is rebuilt on top of the resolved ancestor, so a
    // deleted repo reached through a link keeps the key it had while it
    // existed (the ledger stores days under that key).
    let top_missing = top_missing.map(|missing| match missing.strip_prefix(start) {
        Ok(suffix) => resolved_start.join(suffix),
        Err(_) => missing.to_path_buf(),
    });
    let start = resolved_start;
    if top_missing.is_none() {
        let resolved_cwd = start.to_string_lossy();
        if policy.temp_dirs.iter().any(|temp| is_within(&resolved_cwd, &resolved_dir(temp))) {
            return None;
        }
    }

    // Never walk past the user's home directory. Dotfiles managed as a bare
    // git repo directly in $HOME are common, and without this boundary every
    // session cwd that has no repo of its own would silently merge into one
    // giant project named after the home folder instead of falling back to
    // its own path.
    let home_boundary: Vec<PathBuf> = policy
        .home
        .iter()
        .flat_map(|home| [home.clone(), resolved_dir(home)])
        .collect();

    let mut current = start.as_path();
    loop {
        if home_boundary.iter().any(|home| home == current) {
            break;
        }
        let git_path = current.join(".git");
        if git_path.is_file() {
            if let Some(main_root) = main_root_from_gitdir_file(&git_path) {
                return Some(normalize_project_path(&resolved_dir(&main_root).to_string_lossy()));
            }
            // Unreadable or malformed gitdir pointer: this directory is
            // still the most honest root we have, rather than failing the
            // whole lookup.
            return Some(normalize_project_path(&current.to_string_lossy()));
        }
        if git_path.is_dir() {
            return Some(normalize_project_path(&current.to_string_lossy()));
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    top_missing.map(|missing| normalize_project_path(&missing.to_string_lossy()))
}

/// Reads a git worktree's `.git` FILE (`gitdir: <main>/.git/worktrees/<name>`)
/// and walks back up to `<main>`, the main repository root.
fn main_root_from_gitdir_file(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.trim().strip_prefix("gitdir:")?.trim();
    let gitdir_path = if Path::new(gitdir).is_absolute() {
        PathBuf::from(gitdir)
    } else {
        git_file.parent()?.join(gitdir)
    };
    // gitdir points at <main>/.git/worktrees/<name>; its main root is three
    // path segments up.
    let worktrees_dir = gitdir_path.parent()?; // <main>/.git/worktrees
    let dot_git_dir = worktrees_dir.parent()?; // <main>/.git
    let main_root = dot_git_dir.parent()?; // <main>
    Some(main_root.to_path_buf())
}

/// Every `*.jsonl` file under `root`, recursively. A directory that cannot be
/// read (permissions, race with a delete) is skipped rather than failing the
/// whole scan - a store is otherwise honest even if one subfolder is
/// temporarily unreadable.
pub(crate) fn find_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl")) {
                out.push(path);
            }
        }
    }
    out
}

/// One snapshot of every agent for a time window - the shape the
/// `get_usage` Tauri command returns.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub window: TimeWindow,
    pub window_start: String,
    pub window_end: String,
    pub generated_at: String,
    pub agents: Vec<AgentUsageReport>,
    /// Set when `pricing.json` exists but failed to parse: built-in prices
    /// were used anyway (never a crash), and the panel should say so instead
    /// of silently ignoring the user's overrides. `None` when there is no
    /// override file, or it parsed fine.
    pub pricing_warning: Option<String>,
}

/// Reads every agent for `window`, as of `now`, in the order claude, codex,
/// opencode, pi, pricing claude/codex/pi against `overrides` layered on top
/// of the built-in table (see `pricing::find_price_entry`). Pi reads after
/// claude because its claude-bridge turns count only where Claude's daily
/// buckets have no `(date, project)`.
///
/// With a `ledger_path`, the claude and pi daily buckets are merged with
/// Orbitbar's own ledger (see `usage::ledger`) before the window is folded,
/// so a day an agent has since deleted still counts, and the ledger is
/// written back when that changed it. `None` reads the agents' files only and
/// writes nothing (tests, the parity harness). Sequential: bounded, independent reads
/// are cheaper than the coordination a parallel version would need, and a
/// broken one never blocks the others because each is wrapped in its own
/// `AgentUsageReport`.
pub fn collect_usage(
    window: TimeWindow,
    paths: &UsagePaths,
    now: DateTime<Local>,
    overrides: &pricing::PriceOverrides,
    ledger_path: Option<&Path>,
) -> UsageSnapshot {
    let (start, end) = window_range(window, now);
    let (first_day, end_day) = (start.date_naive(), end.date_naive());
    let mut root_cache = ProjectRootCache::new();

    let claude_daily = claude::read_daily(&paths.claude_dir, &mut root_cache);
    let codex = codex::read_usage(&paths.codex_dir, start, end, overrides, &mut root_cache);
    let opencode = opencode::read_usage(&paths.opencode_db, start, end, &mut root_cache);

    // The ledger is locked from here until it is written, so the claude and
    // pi merges of concurrent calls never interleave.
    let mut ledger = ledger_path.map(ledger::Session::open);

    // Claude's merged buckets stay around for pi, whose claude-bridge turns
    // count only where Claude (its files or the ledger) has no bucket.
    let (mut claude, claude_buckets) = match claude_daily {
        Ok(mut daily) => {
            if let Some(ledger) = ledger.as_mut() {
                daily.buckets = ledger.merge(claude::AGENT, &daily.buckets);
            }
            (claude::report(&daily, first_day, end_day, overrides), daily.buckets)
        }
        Err(report) => (*report, ledger.as_ref().map(|l| l.stored(claude::AGENT)).unwrap_or_default()),
    };
    let mut pi = match pi::read_daily(&paths.pi_dir, &claude_buckets, &mut root_cache) {
        Ok(mut daily) => {
            if let Some(ledger) = ledger.as_mut() {
                daily.buckets = ledger.merge(pi::AGENT, &daily.buckets);
            }
            pi::report(&daily, first_day, end_day, overrides)
        }
        Err(report) => *report,
    };

    if let Some(note) = ledger.and_then(|l| l.finish(now.date_naive())) {
        for report in [&mut claude, &mut pi] {
            report.detail.push_str(" - ");
            report.detail.push_str(&note);
        }
    }

    UsageSnapshot {
        window,
        window_start: start.to_rfc3339(),
        window_end: end.to_rfc3339(),
        generated_at: now.to_rfc3339(),
        agents: vec![claude, codex, opencode, pi],
        pricing_warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};
    use tempfile::tempdir;

    fn local(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn today_window_is_local_midnight_to_midnight() {
        let now = local(2026, 9, 28, 15, 30);
        let (start, end) = window_range(TimeWindow::Today, now);
        assert_eq!(start, local(2026, 9, 28, 0, 0));
        assert_eq!(end, local(2026, 9, 29, 0, 0));
    }

    #[test]
    fn last_7_days_includes_today_and_the_previous_6_days() {
        let now = local(2026, 9, 28, 8, 0);
        let (start, end) = window_range(TimeWindow::Last7Days, now);
        assert_eq!(start, local(2026, 9, 22, 0, 0));
        assert_eq!(end, local(2026, 9, 29, 0, 0));
        assert_eq!((end - start).num_days(), 7);
    }

    #[test]
    fn last_30_days_spans_exactly_30_days() {
        let now = local(2026, 9, 28, 8, 0);
        let (start, end) = window_range(TimeWindow::Last30Days, now);
        assert_eq!((end - start).num_days(), 30);
        assert_eq!(end, local(2026, 9, 29, 0, 0));
    }

    #[test]
    fn this_month_spans_the_calendar_month() {
        let now = local(2026, 9, 15, 12, 0);
        let (start, end) = window_range(TimeWindow::ThisMonth, now);
        assert_eq!(start, local(2026, 9, 1, 0, 0));
        assert_eq!(end, local(2026, 10, 1, 0, 0));
    }

    #[test]
    fn this_month_rolls_over_at_a_december_boundary() {
        let now = local(2026, 12, 31, 23, 0);
        let (start, end) = window_range(TimeWindow::ThisMonth, now);
        assert_eq!(start, local(2026, 12, 1, 0, 0));
        assert_eq!(end, local(2027, 1, 1, 0, 0));
    }

    #[test]
    fn last_365_days_includes_today_and_the_previous_364_days() {
        let now = local(2026, 9, 28, 8, 0);
        let (start, end) = window_range(TimeWindow::Last365Days, now);
        assert_eq!(start, local(2025, 9, 29, 0, 0));
        assert_eq!(end, local(2026, 9, 29, 0, 0));
        assert_eq!((end.date_naive() - start.date_naive()).num_days(), 365);
    }

    #[test]
    fn last_365_days_serializes_as_last365days() {
        assert_eq!(serde_json::to_string(&TimeWindow::Last365Days).unwrap(), "\"last365Days\"");
        let parsed: TimeWindow = serde_json::from_str("\"last365Days\"").unwrap();
        assert_eq!(parsed, TimeWindow::Last365Days);
    }

    #[test]
    fn strip_verbatim_prefix_removes_only_the_verbatim_prefix() {
        assert_eq!(strip_verbatim_prefix(r"\\?\R:\repositories\orbitbar"), r"R:\repositories\orbitbar");
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\server\share\repo"), r"\\server\share\repo");
        assert_eq!(strip_verbatim_prefix(r"C:\repositories\orbitbar"), r"C:\repositories\orbitbar");
        assert_eq!(strip_verbatim_prefix(r"\\server\share"), r"\\server\share");
        assert_eq!(strip_verbatim_prefix("/home/me/orbitbar"), "/home/me/orbitbar");
    }

    #[test]
    fn default_window_is_today() {
        assert_eq!(TimeWindow::default(), TimeWindow::Today);
    }

    #[test]
    #[cfg(windows)]
    fn normalize_project_path_unifies_separators_and_trims_trailing_slash() {
        assert_eq!(normalize_project_path("C:/repos/orbitbar/"), "C:\\repos\\orbitbar");
        assert_eq!(normalize_project_path("  C:\\repos\\orbitbar\\  "), "C:\\repos\\orbitbar");
        assert_eq!(normalize_project_path(""), "");
    }

    /// On macOS and Linux the path must stay walkable with `Path::parent`:
    /// `/` is kept, only a trailing separator is trimmed, and the root
    /// survives.
    #[test]
    #[cfg(not(windows))]
    fn normalize_project_path_keeps_unix_separators_and_trims_trailing_slash() {
        assert_eq!(normalize_project_path("/home/me/orbitbar/"), "/home/me/orbitbar");
        assert_eq!(normalize_project_path("  /home/me/orbitbar  "), "/home/me/orbitbar");
        assert_eq!(normalize_project_path("/"), "/");
        assert_eq!(normalize_project_path(""), "");
    }

    #[test]
    fn project_display_name_is_the_last_segment() {
        assert_eq!(project_display_name("C:\\repos\\orbitbar"), "orbitbar");
        assert_eq!(project_display_name("/home/user/orbitbar"), "orbitbar");
        assert_eq!(project_display_name("orbitbar"), "orbitbar");
    }

    #[test]
    fn resolve_paths_uses_the_override_home_when_given() {
        let home = Path::new("Z:\\fixture-home");
        let paths = resolve_paths(Some(home)).expect("override must resolve");
        assert_eq!(paths.claude_dir, home.join(".claude").join("projects"));
        assert_eq!(paths.codex_dir, home.join(".codex").join("sessions"));
        assert_eq!(
            paths.opencode_db,
            home.join(".local").join("share").join("opencode").join("opencode.db")
        );
        assert_eq!(paths.pi_dir, home.join(".pi").join("agent").join("sessions"));
    }

    /// Every agent is reported, in a fixed order with pi last, even when
    /// none of the stores exist.
    #[test]
    fn collect_usage_reports_claude_codex_opencode_then_pi() {
        let dir = tempdir().unwrap();
        let paths = resolve_paths(Some(dir.path())).unwrap();
        let snapshot = collect_usage(TimeWindow::Today, &paths, Local::now(), &pricing::PriceOverrides::default(), None);

        let agents: Vec<&str> = snapshot.agents.iter().map(|a| a.agent.as_str()).collect();
        assert_eq!(agents, vec!["claude", "codex", "opencode", "pi"]);
        assert!(snapshot.agents.iter().all(|a| a.status == AgentStatus::NotInstalled));
    }

    /// Pi's `claude-bridge` turns are checked against the Claude buckets of
    /// the same snapshot: a day Claude has is not counted twice.
    #[test]
    fn collect_usage_leaves_out_bridge_turns_claude_already_counts() {
        let dir = tempdir().unwrap();
        let paths = resolve_paths(Some(dir.path())).unwrap();
        let now = local(2026, 9, 15, 18, 0);
        let ts = |h: u32| local(2026, 9, 15, h, 0).to_utc().to_rfc3339();
        // No cwd on either side: both land in the no-project bucket, so the
        // test never depends on the machine's real temp or home directory.
        std::fs::create_dir_all(paths.claude_dir.join("p")).unwrap();
        std::fs::write(
            paths.claude_dir.join("p").join("s.jsonl"),
            format!(
                "{}\n",
                format_args!(r#"{{"requestId":"r1","timestamp":"{}","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":10}}}}}}"#, ts(9))
            ),
        )
        .unwrap();
        std::fs::create_dir_all(paths.pi_dir.join("p")).unwrap();
        let pi_turn = |id: &str, provider: &str, output: u64| {
            format!(
                r#"{{"type":"message","id":"{id}","timestamp":"{}","message":{{"role":"assistant","provider":"{provider}","model":"gpt-5.5","usage":{{"input":1,"output":{output},"cacheRead":0,"cacheWrite":0,"totalTokens":0}}}}}}"#,
                ts(10)
            )
        };
        std::fs::write(
            paths.pi_dir.join("p").join("s.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                r#"{"type":"session","id":"s","cwd":""}"#,
                pi_turn("b", "claude-bridge", 10),
                pi_turn("c", "openai-codex", 7),
            ),
        )
        .unwrap();

        let snapshot = collect_usage(TimeWindow::Today, &paths, now, &pricing::PriceOverrides::default(), None);
        let claude = &snapshot.agents[0];
        let pi = &snapshot.agents[3];
        assert_eq!(claude.totals.output_tokens, 10);
        assert_eq!(pi.agent, "pi");
        assert_eq!(pi.totals.entries, 1);
        assert_eq!(pi.totals.output_tokens, 7);
        assert!(pi.detail.contains("1 claude-bridge turns already counted by Claude"), "{}", pi.detail);
    }

    /// A home for the ledger tests: claude and pi files with no cwd (the
    /// no-project bucket, so nothing depends on the real temp or home
    /// directory), and a ledger in a config folder of its own.
    struct LedgerHome {
        dir: tempfile::TempDir,
        paths: UsagePaths,
    }

    impl LedgerHome {
        fn new() -> Self {
            let dir = tempdir().unwrap();
            let paths = resolve_paths(Some(dir.path())).unwrap();
            std::fs::create_dir_all(paths.claude_dir.join("p")).unwrap();
            std::fs::create_dir_all(paths.pi_dir.join("p")).unwrap();
            Self { dir, paths }
        }

        fn ledger(&self) -> PathBuf {
            self.dir.path().join("config").join(ledger::FILE_NAME)
        }

        /// Writes a claude transcript of `(id, day, output)` entries.
        fn claude(&self, name: &str, entries: &[(&str, u32, u64)]) -> PathBuf {
            let path = self.paths.claude_dir.join("p").join(name);
            let mut content = String::new();
            for (id, day, output) in entries {
                content.push_str(&format!(
                    r#"{{"requestId":"{id}","timestamp":"{}","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":{output},"cache_read_input_tokens":2,"cache_creation":{{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":4}}}}}}}}"#,
                    ts(*day)
                ));
                content.push('\n');
            }
            std::fs::write(&path, content).unwrap();
            path
        }

        /// Writes a pi session of `(id, day, provider, output)` turns.
        fn pi(&self, name: &str, turns: &[(&str, u32, &str, u64)]) {
            let mut content = r#"{"type":"session","id":"s","cwd":""}"#.to_string() + "\n";
            for (id, day, provider, output) in turns {
                content.push_str(&format!(
                    r#"{{"type":"message","id":"{id}","timestamp":"{}","message":{{"role":"assistant","provider":"{provider}","model":"gpt-5.5","usage":{{"input":1,"output":{output},"cacheRead":0,"cacheWrite":0,"totalTokens":0}}}}}}"#,
                    ts(*day)
                ));
                content.push('\n');
            }
            std::fs::write(self.paths.pi_dir.join("p").join(name), content).unwrap();
        }

        fn collect(&self, ledger: Option<&Path>) -> UsageSnapshot {
            collect_usage(TimeWindow::ThisMonth, &self.paths, ledger_now(), &pricing::PriceOverrides::default(), ledger)
        }
    }

    fn ledger_now() -> DateTime<Local> {
        local(2026, 9, 15, 18, 0)
    }

    /// Noon local time on `day` of September 2026, as the UTC the stores write.
    fn ts(day: u32) -> String {
        local(2026, 9, day, 12, 0).to_utc().to_rfc3339()
    }

    /// A ledger file holding one no-project claude record per `(date, entries, output)`.
    fn ledger_file(records: &[(&str, u64, u64)]) -> String {
        let days: Vec<serde_json::Value> = records
            .iter()
            .map(|(date, entries, output)| {
                serde_json::json!({
                    "agent": "claude", "date": date, "project": "", "model": "claude-opus-5-5",
                    "input": 0, "output": output, "cacheRead": 0, "cacheWrite5m": 0, "cacheWrite1h": 0,
                    "reasoning": 0, "entries": entries
                })
            })
            .collect();
        serde_json::json!({ "version": 1, "days": days }).to_string()
    }

    /// Claude Code deletes old transcripts; a day the ledger saw still counts.
    #[test]
    fn a_deleted_claude_transcript_still_counts_through_the_ledger() {
        let home = LedgerHome::new();
        let ledger = home.ledger();
        let transcript = home.claude("s.jsonl", &[("r1", 14, 10), ("r2", 15, 20)]);

        let first = home.collect(Some(&ledger));
        assert!(ledger.exists(), "the first collect creates the ledger");
        std::fs::remove_file(&transcript).unwrap();
        let second = home.collect(Some(&ledger));

        let (before, after) = (&first.agents[0], &second.agents[0]);
        assert_eq!(after.status, AgentStatus::Ok);
        assert_eq!(after.totals.entries, 2);
        assert_eq!(after.totals.output_tokens, 30);
        assert_eq!(after.totals, before.totals, "every counter and the cost come back from the ledger");
        assert!(after.totals.cost.is_some());

        let without_ledger = home.collect(None);
        assert_eq!(without_ledger.agents[0].totals.entries, 0, "the agent's own files no longer have it");
    }

    /// The ledger is written only when a collect changed it: an equal
    /// ledger, even formatted differently, keeps its bytes.
    #[test]
    fn an_unchanged_ledger_is_not_rewritten() {
        let home = LedgerHome::new();
        let ledger = home.ledger();
        home.claude("s.jsonl", &[("r1", 14, 10)]);
        home.collect(Some(&ledger));

        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&ledger).unwrap()).unwrap();
        let compact = serde_json::to_string(&value).unwrap();
        std::fs::write(&ledger, &compact).unwrap();
        home.collect(Some(&ledger));
        assert_eq!(std::fs::read_to_string(&ledger).unwrap(), compact, "nothing changed, nothing written");

        home.claude("s.jsonl", &[("r1", 14, 10), ("r2", 15, 20)]);
        home.collect(Some(&ledger));
        let reread = ledger::load(&ledger).unwrap();
        assert_eq!(reread.buckets("claude").iter().count(), 2, "a new day is written");
    }

    /// A version 1 ledger is read as is and rewritten as version 2 by the
    /// next collect, with the same records and the same totals.
    #[test]
    fn a_version_1_ledger_is_rewritten_as_version_2_with_the_same_totals() {
        let home = LedgerHome::new();
        let ledger = home.ledger();
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        let v1 = ledger_file(&[("2026-09-01", 9, 900), ("2026-09-14", 2, 20)]);
        std::fs::write(&ledger, &v1).unwrap();

        let first = home.collect(Some(&ledger));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&ledger).unwrap()).unwrap();
        let mut expected: serde_json::Value = serde_json::from_str(&v1).unwrap();
        expected["version"] = 2.into();
        assert_eq!(json, expected, "same records, version 2");

        let second = home.collect(Some(&ledger));
        let claude = &first.agents[0];
        assert_eq!(claude.totals.entries, 11);
        assert_eq!(claude.totals.output_tokens, 920);
        assert!(claude.totals.cost.is_some());
        assert_eq!(second.agents[0].totals, claude.totals, "the v2 file gives the same totals");
    }

    /// A ledger Orbitbar cannot use is never overwritten: the call reads the
    /// agents' files only and the claude and pi details say why.
    #[test]
    fn a_corrupt_or_unknown_version_ledger_is_left_untouched() {
        for (content, reason) in [
            ("{ not json".to_string(), "not a valid ledger"),
            (ledger_file(&[("2026-09-01", 9, 900)]).replace(r#""version":1"#, r#""version":3"#), "unknown version 3"),
        ] {
            let home = LedgerHome::new();
            let ledger = home.ledger();
            std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            std::fs::write(&ledger, &content).unwrap();
            home.claude("s.jsonl", &[("r1", 14, 10)]);
            home.pi("s.jsonl", &[("c1", 14, "openai-codex", 7)]);

            let snapshot = home.collect(Some(&ledger));
            assert_eq!(std::fs::read_to_string(&ledger).unwrap(), content, "{reason}: left byte for byte");
            let (claude, pi) = (&snapshot.agents[0], &snapshot.agents[3]);
            assert_eq!(claude.totals.output_tokens, 10, "{reason}: source data only");
            assert_eq!(pi.totals.output_tokens, 7);
            for detail in [&claude.detail, &pi.detail] {
                assert!(detail.contains("history ledger not used"), "{detail}");
                assert!(detail.contains(reason), "{detail}");
            }
            assert!(!snapshot.agents[1].detail.contains("ledger"), "codex never uses the ledger");
        }
    }

    /// A claude-bridge turn on a day only the ledger still has for Claude is
    /// already counted there, whether the Claude folder is empty or gone.
    #[test]
    fn a_bridge_turn_on_a_day_only_the_ledger_has_for_claude_is_skipped() {
        for claude_dir_exists in [true, false] {
            let home = LedgerHome::new();
            if !claude_dir_exists {
                std::fs::remove_dir_all(&home.paths.claude_dir).unwrap();
            }
            let ledger = home.ledger();
            std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            std::fs::write(&ledger, ledger_file(&[("2026-09-15", 1, 10)])).unwrap();
            home.pi("s.jsonl", &[("b1", 15, "claude-bridge", 10), ("c1", 15, "openai-codex", 7)]);

            let snapshot = home.collect(Some(&ledger));
            let pi = &snapshot.agents[3];
            assert_eq!(pi.totals.entries, 1, "claude folder exists: {claude_dir_exists}");
            assert_eq!(pi.totals.output_tokens, 7);
            assert!(pi.detail.contains("1 claude-bridge turns already counted by Claude"), "{}", pi.detail);
            if claude_dir_exists {
                assert_eq!(snapshot.agents[0].totals.output_tokens, 10, "the ledger day is in Claude's totals");
            }
        }
    }

    /// Two collects at once, each reading a different home into the same
    /// ledger, leave a valid file holding both.
    #[test]
    fn two_collects_at_once_leave_the_union_in_the_ledger() {
        for _ in 0..5 {
            let config = tempdir().unwrap();
            let ledger = config.path().join(ledger::FILE_NAME);
            let (a, b) = (LedgerHome::new(), LedgerHome::new());
            a.claude("s.jsonl", &[("r1", 14, 10)]);
            b.claude("s.jsonl", &[("r2", 15, 20)]);

            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                for home in [&a, &b] {
                    let (barrier, ledger) = (&barrier, &ledger);
                    scope.spawn(move || {
                        barrier.wait();
                        home.collect(Some(ledger));
                    });
                }
            });

            let stored = ledger::load(&ledger).expect("a valid ledger");
            let days: Vec<NaiveDate> = stored.buckets("claude").iter().map(|(d, _, _)| d).collect();
            assert_eq!(days, vec![local(2026, 9, 14, 0, 0).date_naive(), local(2026, 9, 15, 0, 0).date_naive()]);
            let empty = LedgerHome::new();
            assert_eq!(empty.collect(Some(&ledger)).agents[0].totals.output_tokens, 30);
        }
    }

    /// A ledger that cannot be written never fails the collect. A directory
    /// sits where the temp file goes, which blocks the write the same way on
    /// every OS while the (missing) ledger itself still loads.
    #[test]
    fn a_ledger_that_cannot_be_saved_is_reported_in_the_detail() {
        let home = LedgerHome::new();
        let ledger = home.ledger();
        let config = ledger.parent().unwrap();
        let blocker = config.join(format!("{}.{}.tmp", ledger::FILE_NAME, std::process::id()));
        std::fs::create_dir_all(&blocker).unwrap();
        home.claude("s.jsonl", &[("r1", 14, 10)]);
        home.pi("s.jsonl", &[("c1", 14, "openai-codex", 7)]);

        let snapshot = home.collect(Some(&ledger));
        let (claude, pi) = (&snapshot.agents[0], &snapshot.agents[3]);
        assert_eq!(claude.totals.output_tokens, 10);
        assert!(claude.detail.contains("history ledger not saved"), "{}", claude.detail);
        assert!(pi.detail.contains("history ledger not saved"), "{}", pi.detail);
        assert!(!ledger.exists(), "no ledger written");
        assert!(blocker.is_dir(), "the blocking directory is left alone");
    }

    /// Records older than the 365 days that end today leave the file.
    #[test]
    fn a_collect_prunes_records_older_than_365_days() {
        let home = LedgerHome::new();
        let ledger = home.ledger();
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        let today = ledger_now().date_naive();
        let kept = (today - Duration::days(364)).to_string();
        let old = (today - Duration::days(365)).to_string();
        std::fs::write(&ledger, ledger_file(&[(&kept, 1, 1), (&old, 1, 2)])).unwrap();

        home.collect(Some(&ledger));
        let days: Vec<String> =
            ledger::load(&ledger).unwrap().buckets("claude").iter().map(|(d, _, _)| d.to_string()).collect();
        assert_eq!(days, vec![kept]);
    }

    /// Without a ledger path the collect only reads: the parity harness
    /// relies on it.
    #[test]
    fn a_collect_without_a_ledger_path_writes_nothing() {
        let home = LedgerHome::new();
        home.claude("s.jsonl", &[("r1", 14, 10)]);
        let before = list_files(home.dir.path());
        home.collect(None);
        assert_eq!(list_files(home.dir.path()), before);
    }

    fn list_files(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                }
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }

    #[test]
    fn agent_report_ok_sorts_projects_by_output_desc() {
        let mk = |name: &str, output: u64| ProjectUsage {
            path: name.into(),
            name: name.into(),
            model: "".into(),
            totals: UsageTotals { output_tokens: output, ..Default::default() },
        };
        let report = AgentUsageReport::ok(
            "claude",
            UsageTotals::default(),
            vec![mk("a", 10), mk("b", 100), mk("c", 50)],
            "",
        );
        let names: Vec<&str> = report.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn resolve_project_root_walks_up_to_the_nearest_git_ancestor() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("app").join("src-tauri");
        std::fs::create_dir_all(&nested).unwrap();

        let mut cache = ProjectRootCache::with_policy(RootPolicy::default());
        let root = resolve_project_root(&nested.to_string_lossy(), &mut cache);
        assert_eq!(root, canonical_key(dir.path()));
    }

    #[test]
    fn resolve_project_root_follows_a_worktree_gitdir_file_to_the_main_root() {
        let dir = tempdir().unwrap();
        let main = dir.path().join("main");
        let worktrees_dir = main.join(".git").join("worktrees").join("agent-foo");
        std::fs::create_dir_all(&worktrees_dir).unwrap();

        let worktree = dir.path().join("agent-foo-checkout");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktrees_dir.display()),
        )
        .unwrap();

        let mut cache = ProjectRootCache::with_policy(RootPolicy::default());
        let root = resolve_project_root(&worktree.to_string_lossy(), &mut cache);
        assert_eq!(root, canonical_key(&main));
    }

    /// A resolver whose walk stops at `home` and that treats `temp` as scratch,
    /// so no test depends on the machine's real home or temp directory.
    fn policy(home: &Path, temp: &[&Path]) -> RootPolicy {
        RootPolicy {
            temp_dirs: temp.iter().map(|t| t.to_path_buf()).collect(),
            home: Some(home.to_path_buf()),
        }
    }

    fn resolve(cwd: &Path, policy: RootPolicy) -> Option<String> {
        resolve_project_root(&cwd.to_string_lossy(), &mut ProjectRootCache::with_policy(policy))
    }

    /// The key of an existing folder: its path as the file system resolves it
    /// (macOS reports the temp dir as `/var` for `/private/var`; Windows may
    /// report an 8.3 short name).
    /// The key of a deleted folder `missing` directly under the surviving
    /// `parent`: the parent in resolved form (the OS temp dir is a short
    /// `RUNNER~1` name on Windows CI and `/var` for `/private/var` on macOS)
    /// plus the missing name.
    fn resolved_key(parent: &Path, missing: &str) -> Option<String> {
        let resolved = std::fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(strip_verbatim_prefix(&resolved.to_string_lossy()));
        Some(normalize_project_path(&parent.join(missing).to_string_lossy()))
    }

    fn canonical_key(path: &Path) -> Option<String> {
        let resolved = std::fs::canonicalize(path).unwrap();
        Some(normalize_project_path(&strip_verbatim_prefix(&resolved.to_string_lossy())))
    }

    /// Links `link` to the existing directory `target`: a junction on Windows,
    /// a symlink elsewhere. `false` when the OS refused to create it.
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|out| out.status.success())
                && link.exists()
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    /// `C:\repositories` is a junction to `R:\repositories`: a repository
    /// reached through both paths is one project key, the resolved one.
    #[test]
    fn two_paths_to_the_same_repository_through_a_junction_share_one_key() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        let repo = real.join("orbitbar");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("app")).unwrap();
        let link = dir.path().join("link");
        if !link_dir(&real, &link) {
            eprintln!("skipped: could not create a directory link in {}", dir.path().display());
            return;
        }
        let p = || policy(dir.path(), &[]);

        let direct = resolve(&repo.join("app"), p());
        let through_link = resolve(&link.join("orbitbar").join("app"), p());
        let link_root = resolve(&link.join("orbitbar"), p());
        assert_eq!(direct, canonical_key(&repo));
        assert_eq!(through_link, direct);
        assert_eq!(link_root, direct);
    }

    /// A repository reached through a link keeps its key after it is deleted,
    /// so the ledger's stored days and the transcripts' days stay one project
    /// instead of two rows counting the same usage.
    #[test]
    fn a_deleted_repository_reached_through_a_link_keeps_its_resolved_key() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        let repo = real.join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("app")).unwrap();
        let link = dir.path().join("link");
        if !link_dir(&real, &link) {
            eprintln!("skipped: could not create a directory link in {}", dir.path().display());
            return;
        }
        let cwd = link.join("proj").join("app");
        let before = resolve(&cwd, policy(dir.path(), &[]));
        assert_eq!(before, canonical_key(&repo));

        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!(resolve(&cwd, policy(dir.path(), &[])), before);
    }

    /// A folder outside any repository stays excluded when it is reached
    /// through a link: the home boundary is compared in resolved form too.
    #[test]
    fn a_linked_folder_outside_any_repository_is_still_excluded() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let real = dir.path().join("plain");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("plain-link");
        if !link_dir(&real, &link) {
            eprintln!("skipped: could not create a directory link in {}", dir.path().display());
            return;
        }

        assert_eq!(resolve(&link, policy(dir.path(), &[])), None);
    }

    /// An existing folder with no repository above it (up to the home
    /// boundary) is not a project: it is excluded, not keyed by its own path.
    #[test]
    fn an_existing_folder_outside_any_repository_is_excluded() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("no-repo-here");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(resolve(&nested, policy(dir.path(), &[])), None);
    }

    /// The home directory itself (and anything that only reaches it) is not a
    /// project, even when the home holds a bare dotfiles repo.
    #[test]
    fn the_home_directory_itself_is_excluded() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        assert_eq!(resolve(dir.path(), policy(dir.path(), &[])), None);
    }

    #[test]
    fn resolve_project_root_falls_back_to_cwd_when_the_path_is_missing() {
        let missing = "Z:\\this\\path\\does\\not\\exist\\on\\this\\machine";
        let mut cache = ProjectRootCache::with_policy(RootPolicy::default());
        let root = resolve_project_root(missing, &mut cache);
        assert_eq!(root, Some(normalize_project_path(missing)));
    }

    /// A removed worktree or deleted subfolder of a repo that still exists
    /// must roll up into that repo, not linger as its own row.
    #[test]
    fn resolve_project_root_rolls_a_deleted_subfolder_into_its_surviving_repo() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let deleted = dir.path().join(".claude").join("worktrees").join("agent-gone");

        assert_eq!(resolve(&deleted, RootPolicy::default()), canonical_key(dir.path()));
    }

    /// A deleted folder with no repo above it is keyed by its own path.
    #[test]
    fn resolve_project_root_keeps_a_deleted_folder_outside_any_repo() {
        let dir = tempdir().unwrap();
        let deleted = dir.path().join("gone-project");

        assert_eq!(resolve(&deleted, policy(dir.path(), &[])), resolved_key(dir.path(), "gone-project"));
    }

    /// A repo that was renamed or deleted leaves session cwds in its
    /// subfolders. They all belong to the repo root, the topmost directory
    /// that no longer exists (the child of the deepest surviving ancestor).
    #[test]
    fn deleted_subfolders_of_a_vanished_repo_share_its_topmost_missing_root() {
        let dir = tempdir().unwrap();
        let gone = dir.path().join("herdr-omniroute");
        let p = || policy(dir.path(), &[]);

        for sub in [
            gone.clone(),
            gone.join("app"),
            gone.join("app").join("src-tauri").join("target"),
            gone.join(".claude").join("worktrees").join("agent-x"),
            gone.join("hotbar-tauri").join("src-tauri"),
        ] {
            assert_eq!(resolve(&sub, p()), resolved_key(dir.path(), "herdr-omniroute"), "{}", sub.display());
        }
    }

    /// A cwd under the OS temp dir is scratch work (for example a review
    /// subprocess run): excluded whether or not it still exists, and even when
    /// it holds a repository.
    #[test]
    fn a_cwd_under_the_temp_dir_is_excluded_whether_or_not_it_exists() {
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let alive = temp.join("run-1");
        std::fs::create_dir_all(alive.join(".git")).unwrap();
        let p = || policy(dir.path(), &[&temp]);

        assert_eq!(resolve(&alive, p()), None, "existing, with a repo");
        assert_eq!(resolve(&temp.join("run-2"), p()), None, "deleted");
        assert_eq!(resolve(&temp.join("run-2").join("deep"), p()), None, "deleted, nested");
        assert_eq!(resolve(&temp, p()), None, "the temp dir itself");
    }

    /// The temp check is by whole path components, never a string prefix.
    #[test]
    fn a_sibling_that_only_shares_the_temp_prefix_is_not_excluded() {
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let sibling = dir.path().join("Temp-projects").join("app");

        assert_eq!(resolve(&sibling, policy(dir.path(), &[&temp])), resolved_key(dir.path(), "Temp-projects"));
    }

    /// Windows paths are case-insensitive: the agent may record the temp dir
    /// with different letter case (or the long name where the OS reports 8.3).
    #[cfg(windows)]
    #[test]
    fn the_temp_check_ignores_case_on_windows() {
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let cwd = dir.path().join("TEMP").join("gentle-ai-codex-reviewer-123");

        assert_eq!(resolve(&cwd, policy(dir.path(), &[&temp])), None);
    }

    #[test]
    fn resolve_project_root_caches_repeated_lookups() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        let mut cache = ProjectRootCache::with_policy(RootPolicy::default());
        let cwd = dir.path().to_string_lossy().to_string();
        let first = resolve_project_root(&cwd, &mut cache);
        // Remove the .git marker: if the second call still hit the
        // filesystem it would now resolve differently instead of returning
        // the cached root, so this proves the cache - not a repeated walk - is
        // what the second call actually used.
        std::fs::remove_dir_all(dir.path().join(".git")).unwrap();
        let second = resolve_project_root(&cwd, &mut cache);
        assert_eq!(first, second);
        assert_eq!(cache.cached_lookups(), 1);
    }

    /// An exclusion is cached too: it costs a filesystem walk like any other.
    #[test]
    fn an_exclusion_is_cached_like_a_root() {
        let dir = tempdir().unwrap();
        let mut cache = ProjectRootCache::with_policy(policy(dir.path(), &[dir.path()]));
        let cwd = dir.path().join("x").to_string_lossy().to_string();
        assert_eq!(resolve_project_root(&cwd, &mut cache), None);
        assert_eq!(resolve_project_root(&cwd, &mut cache), None);
        assert_eq!(cache.cached_lookups(), 1);
    }

    /// Not a unit test: prints the real home directory's ThisMonth snapshot
    /// so it can be sanity-checked by hand against what the agents' own UIs
    /// report. It passes no ledger path, so it never writes a file.
    /// `--ignored` because it depends on this machine's real
    /// `.claude` / `.codex` / opencode / `.pi` stores, which do not exist in CI or on
    /// a fresh checkout.
    ///
    /// Run with: `cargo test -- --ignored parity --nocapture`
    #[test]
    #[ignore]
    fn parity_this_month_against_the_real_home() {
        let paths = resolve_paths(None).expect("real home directory must resolve on this machine");
        let overrides = pricing::PriceOverrides::default();
        let snapshot = collect_usage(TimeWindow::ThisMonth, &paths, Local::now(), &overrides, None);

        println!("\n=== orbitbar Rust reader parity (ThisMonth) ===");
        for agent in &snapshot.agents {
            println!(
                "{:9} status={:?} entries={:<6} output_tokens={:<10} cost={:?} projects={}",
                agent.agent,
                agent.status,
                agent.totals.entries,
                agent.totals.output_tokens,
                agent.totals.cost,
                agent.projects.len()
            );
            println!("          detail: {}", agent.detail);
        }
    }
}
