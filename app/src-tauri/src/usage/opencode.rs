//! Opencode usage reader: SQLite `session` table at
//! `~/.local/share/opencode/opencode.db`.
//!
//! The `session` table is already consolidated per session (unlike the
//! claude/codex jsonl stores), so this is two indexed `SELECT`s: one totals
//! the window, one splits the same window per `project.worktree`. Money is
//! REAL - `session.cost` as recorded, not estimated - so opencode never goes
//! through the pricing table and a `0.0` is a genuine zero for a local model.
//!
//! Opened read-only (`rusqlite` with the `bundled` feature, no system SQLite
//! dependency) so the widget can never write to a database a live opencode
//! process might also have open.
//!
//! Sessions outside any project (a worktree under the OS temp dir, an
//! existing folder that is not inside a repository, no worktree, or no
//! project row; see `project_key`) count in the "Outside projects" row, so
//! the rows sum to the totals.

use crate::usage::{
    normalize_project_path, project_display_name, project_key, AgentUsageReport, CostBasis, ProjectAliases,
    ProjectRootCache, ProjectUsage, UsageTotals,
};
use chrono::{DateTime, Local};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

const AGENT: &str = "opencode";

/// Reads window usage from the opencode SQLite store at `db_path`.
/// Per-project totals are keyed by resolved project root (see
/// `resolve_project_root`), not the raw `project.worktree` path, so two
/// worktrees inside the same repository merge into one row, and each project
/// is reported under its alias (see `ProjectAliases::merge_rows`).
pub fn read_usage(
    db_path: &Path,
    start: DateTime<Local>,
    end: DateTime<Local>,
    root_cache: &mut ProjectRootCache,
    aliases: &ProjectAliases,
) -> AgentUsageReport {
    if !db_path.exists() {
        return AgentUsageReport::not_installed(AGENT, format!("database not found: {}", db_path.display()));
    }

    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => return AgentUsageReport::error(AGENT, format!("cannot open opencode.db: {e}")),
    };
    if let Err(e) = conn.busy_timeout(std::time::Duration::from_millis(1200)) {
        return AgentUsageReport::error(AGENT, format!("cannot set busy timeout: {e}"));
    }

    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();

    let totals = match read_totals(&conn, start_ms, end_ms) {
        Ok(t) => t,
        Err(e) => return AgentUsageReport::error(AGENT, format!("cannot read session totals: {e}")),
    };

    let projects = match read_projects(&conn, start_ms, end_ms, root_cache) {
        Ok(p) => aliases.merge_rows(p),
        Err(e) => return AgentUsageReport::error(AGENT, format!("cannot read per-project totals: {e}")),
    };

    let detail = format!("{} projects", projects.len());
    AgentUsageReport::ok(AGENT, totals, projects, detail)
}

fn read_totals(conn: &Connection, start_ms: i64, end_ms: i64) -> rusqlite::Result<UsageTotals> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(SUM(tokens_input), 0),
                COALESCE(SUM(tokens_output), 0),
                COALESCE(SUM(tokens_reasoning), 0),
                COALESCE(SUM(tokens_cache_read), 0),
                COALESCE(SUM(tokens_cache_write), 0),
                COALESCE(SUM(cost), 0.0),
                COUNT(*)
         FROM session WHERE time_updated >= ?1 AND time_updated < ?2",
    )?;
    stmt.query_row([start_ms, end_ms], |row| {
        let entries: i64 = row.get(6)?;
        let cost: f64 = row.get(5)?;
        Ok(UsageTotals {
            input_tokens: row.get::<_, i64>(0)? as u64,
            output_tokens: row.get::<_, i64>(1)? as u64,
            reasoning_tokens: row.get::<_, i64>(2)? as u64,
            cache_read_tokens: row.get::<_, i64>(3)? as u64,
            cache_write_tokens: row.get::<_, i64>(4)? as u64,
            // Real money, never estimated: a session with entries but a null
            // sum is 0.0, a real zero for a local model. Zero sessions means
            // no cost value can even be claimed.
            cost: if entries > 0 { Some(cost) } else { None },
            cost_basis: if entries > 0 { Some(CostBasis::Reported) } else { None },
            entries: entries as u64,
        })
    })
}

/// The per-project rows. Every session in the window is in exactly one row,
/// so they sum to `read_totals`: a session with no project row or an empty
/// worktree is outside projects.
fn read_projects(
    conn: &Connection,
    start_ms: i64,
    end_ms: i64,
    root_cache: &mut ProjectRootCache,
) -> rusqlite::Result<Vec<ProjectUsage>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(p.worktree, ''),
                COALESCE(SUM(s.tokens_input), 0),
                COALESCE(SUM(s.tokens_output), 0),
                COALESCE(SUM(s.tokens_reasoning), 0),
                COALESCE(SUM(s.tokens_cache_read), 0),
                COALESCE(SUM(s.tokens_cache_write), 0),
                COALESCE(SUM(s.cost), 0.0),
                COUNT(*)
         FROM session s LEFT JOIN project p ON s.project_id = p.id
         WHERE s.time_updated >= ?1 AND s.time_updated < ?2
         GROUP BY p.worktree",
    )?;
    let rows = stmt.query_map([start_ms, end_ms], |row| {
        let worktree: String = row.get(0)?;
        let entries: i64 = row.get(7)?;
        let cost: f64 = row.get(6)?;
        Ok((
            worktree,
            UsageTotals {
                input_tokens: row.get::<_, i64>(1)? as u64,
                output_tokens: row.get::<_, i64>(2)? as u64,
                reasoning_tokens: row.get::<_, i64>(3)? as u64,
                cache_read_tokens: row.get::<_, i64>(4)? as u64,
                cache_write_tokens: row.get::<_, i64>(5)? as u64,
                cost: if entries > 0 { Some(cost) } else { None },
                cost_basis: if entries > 0 { Some(CostBasis::Reported) } else { None },
                entries: entries as u64,
            },
        ))
    })?;

    // Several worktrees can resolve to the same project root (a subfolder,
    // another checkout of the same repo), so this aggregates by resolved
    // root rather than pushing one row per SQL group.
    let mut aggregated: HashMap<String, UsageTotals> = HashMap::new();
    for row in rows {
        let (worktree, totals) = row?;
        let acc = aggregated.entry(project_key(&normalize_project_path(&worktree), root_cache)).or_default();
        acc.input_tokens += totals.input_tokens;
        acc.output_tokens += totals.output_tokens;
        acc.reasoning_tokens += totals.reasoning_tokens;
        acc.cache_read_tokens += totals.cache_read_tokens;
        acc.cache_write_tokens += totals.cache_write_tokens;
        acc.entries += totals.entries;
        // Every row here came from a matched session, so `cost`/`cost_basis`
        // are always `Some` (see the query above) - summing the amounts and
        // re-asserting `Reported` keeps that same guarantee on the merged row.
        acc.cost = Some(acc.cost.unwrap_or(0.0) + totals.cost.unwrap_or(0.0));
        acc.cost_basis = Some(CostBasis::Reported);
    }

    let mut out = Vec::with_capacity(aggregated.len());
    for (root, totals) in aggregated {
        out.push(ProjectUsage {
            name: project_display_name(&root),
            path: root,
            model: String::new(),
            totals,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::AgentStatus;
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn window() -> (DateTime<Local>, DateTime<Local>) {
        (
            Local.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            Local.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        )
    }

    fn make_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 time_updated INTEGER,
                 cost REAL,
                 tokens_input INTEGER,
                 tokens_output INTEGER,
                 tokens_reasoning INTEGER,
                 tokens_cache_read INTEGER,
                 tokens_cache_write INTEGER
             );",
        )
        .unwrap();
        conn
    }

    fn ms(y: i32, m: u32, d: u32) -> i64 {
        Local.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap().timestamp_millis()
    }

    #[test]
    fn missing_database_is_not_installed() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("opencode.db");
        let (start, end) = window();
        let report = read_usage(&missing, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.status, AgentStatus::NotInstalled);
    }

    #[test]
    fn reads_the_real_cost_column_not_an_estimate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', 'C:/repos/orbitbar')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 1.2345, 1000, 500, 10, 20, 30)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.status, AgentStatus::Ok);
        assert_eq!(report.totals.cost, Some(1.2345));
        assert_eq!(report.totals.output_tokens, 500);
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].totals.cost, Some(1.2345));
        assert_eq!(
            report.totals.cost_basis,
            Some(CostBasis::Reported),
            "opencode's own cost column is a reported bill, never an estimate"
        );
    }

    /// Two different worktrees inside the same repository (the repo root and
    /// a subfolder) must aggregate into one project row instead of two, once
    /// the resolved project root is used as the bucket key.
    #[test]
    fn two_worktrees_in_the_same_repo_merge_into_one_project_row() {
        let dir = tempdir().unwrap();
        let repo_root = dir.path().join("repo");
        std::fs::create_dir_all(repo_root.join(".git")).unwrap();
        let subfolder = repo_root.join("app").join("src-tauri");
        std::fs::create_dir_all(&subfolder).unwrap();
        let root_worktree = repo_root.to_string_lossy().replace('\\', "/");
        let sub_worktree = subfolder.to_string_lossy().replace('\\', "/");

        let db_path = dir.path().join("opencode.db");
        let conn = make_db(&db_path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', ?1), ('p2', ?2)",
            rusqlite::params![root_worktree, sub_worktree],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 1.0, 100, 10, 0, 0, 0),
                    ('s2', 'p2', ?1, 2.0, 100, 20, 0, 0, 0)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let (start, end) = window();
        let mut cache = ProjectRootCache::with_policy(crate::usage::RootPolicy {
            temp_dirs: Vec::new(),
            home: Some(dir.path().to_path_buf()),
        });
        let report = read_usage(&db_path, start, end, &mut cache, &ProjectAliases::default());
        assert_eq!(report.projects.len(), 1, "both worktrees share the same repo root");
        assert_eq!(report.projects[0].totals.output_tokens, 30, "tokens from both worktrees must sum");
        assert_eq!(report.projects[0].totals.cost, Some(3.0), "cost from both worktrees must sum");
        assert_eq!(report.projects[0].name, "repo");
    }

    #[test]
    fn a_real_zero_cost_stays_zero_not_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', 'C:/repos/local-model')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 0.0, 100, 50, 0, 0, 0)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.totals.cost, Some(0.0), "a real zero must be reported, not treated as missing");
    }

    #[test]
    fn window_filtering_excludes_sessions_outside_the_range() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', 'C:/repos/orbitbar')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('in', 'p1', ?1, 1.0, 100, 50, 0, 0, 0),
                    ('out', 'p1', ?2, 999.0, 999, 999, 0, 0, 0)",
            rusqlite::params![ms(2026, 9, 15), ms(2026, 8, 31)],
        )
        .unwrap();
        drop(conn);

        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.totals.entries, 1);
        assert_eq!(report.totals.output_tokens, 50);
    }

    #[test]
    fn empty_database_is_ok_with_empty_totals() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        make_db(&path);

        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.status, AgentStatus::Ok);
        assert_eq!(report.totals.entries, 0);
        assert_eq!(report.totals.cost, None);
        assert!(report.projects.is_empty());
    }

    /// Sessions whose worktree is scratch (under the temp dir) or a folder
    /// outside any repository stay in the totals and form one "Outside
    /// projects" row with an empty path, so the rows still sum to the totals.
    #[test]
    fn sessions_outside_any_project_count_in_the_outside_projects_row() {
        use crate::usage::RootPolicy;
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let kept = dir.path().join("kept-app").join("app");

        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', ?1), ('p2', ?2), ('p3', ?3)",
            rusqlite::params![
                kept.to_string_lossy(),
                temp.join("run-1").to_string_lossy(),
                plain.to_string_lossy()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 1.0, 100, 10, 0, 0, 0),
                    ('s2', 'p2', ?1, 20.0, 100, 500, 0, 0, 0),
                    ('s3', 'p3', ?1, 30.0, 100, 700, 0, 0, 0)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let mut cache = ProjectRootCache::with_policy(RootPolicy {
            temp_dirs: vec![temp],
            home: Some(dir.path().to_path_buf()),
        });
        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut cache, &ProjectAliases::default());

        assert_eq!(report.totals.entries, 3);
        assert_eq!(report.totals.output_tokens, 1210);
        assert_eq!(report.totals.cost, Some(51.0));
        assert_eq!(report.projects.len(), 2);
        let outside = &report.projects[0];
        assert_eq!((outside.name.as_str(), outside.path.as_str()), ("Outside projects", ""), "sorted by output");
        assert_eq!(outside.totals.output_tokens, 1200);
        assert_eq!(outside.totals.cost, Some(50.0));
        assert_eq!(outside.totals.cost_basis, Some(CostBasis::Reported));
        assert_eq!(report.projects[1].name, "kept-app");
        assert_eq!(report.projects.iter().map(|p| p.totals.entries).sum::<u64>(), report.totals.entries);
        assert!(!report.detail.contains("not counted"), "{}", report.detail);
    }

    /// A session with an empty worktree, or whose project row is missing,
    /// is outside projects too: the rows always sum to the totals.
    #[test]
    fn sessions_with_no_worktree_are_outside_projects() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute("INSERT INTO project (id, worktree) VALUES ('p1', '')", []).unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 1.0, 100, 10, 0, 0, 0),
                    ('s2', 'gone', ?1, 2.0, 100, 20, 0, 0, 0)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut ProjectRootCache::new(), &ProjectAliases::default());
        assert_eq!(report.totals.entries, 2);
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].path, "");
        assert_eq!(report.projects[0].name, "Outside projects");
        assert_eq!(report.projects[0].totals, report.totals);
    }

    /// When every session is outside projects they are all in the totals and
    /// the one row, with their reported cost.
    #[test]
    fn when_every_session_is_outside_projects_they_are_all_in_one_row() {
        use crate::usage::RootPolicy;
        let dir = tempdir().unwrap();
        let temp = dir.path().join("Temp");
        let path = dir.path().join("opencode.db");
        let conn = make_db(&path);
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('p1', ?1)",
            [temp.join("run-1").to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, time_updated, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
             VALUES ('s1', 'p1', ?1, 2.5, 100, 50, 0, 0, 0)",
            [ms(2026, 9, 15)],
        )
        .unwrap();
        drop(conn);

        let mut cache = ProjectRootCache::with_policy(RootPolicy {
            temp_dirs: vec![temp],
            home: Some(dir.path().to_path_buf()),
        });
        let (start, end) = window();
        let report = read_usage(&path, start, end, &mut cache, &ProjectAliases::default());

        assert_eq!(report.totals.entries, 1);
        assert_eq!(report.totals.cost, Some(2.5));
        assert_eq!(report.totals.cost_basis, Some(CostBasis::Reported));
        assert_eq!(report.projects.len(), 1);
        assert_eq!(report.projects[0].name, "Outside projects");
        assert_eq!(report.projects[0].totals, report.totals);
    }
}
