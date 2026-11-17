# Feature: usage-history-macos — full project list, one-year history, Pi sessions and macOS rendering

Status: **in progress** (opened 2026-11-17). Branch `feat/usage-history-macos` (from `dev`, `16299ea`).
Projects past the top 5 become reachable, usage history reaches one year (Pi sessions plus Orbitbar's own
ledger), and the bar renders correctly on macOS Retina screens.

## Specs

- **S1 — Projects past the top 5.** User: "Este mes trabaje por ejemplo en black-roasters y no lo veo computador
  en las metricas de la hotbar". Diagnosis: the Claude reader counts it (This month: 125 entries, 15th of 18
  projects) but the panel shows only `MAX_PROJECTS_SHOWN = 5`. Decision: "Top 5 + 'ver todos'" — the panel keeps
  the top 5 and offers a control that expands to every project.
- **S2 — One-year history window.** User: "me gustaria tener un historico total, porque hay proyectos que no toco
  hace mucho mas de 1 o 2 meses y quiero saber las metricas y las perdi." Decision on retention: "Menos tiempo 365
  dias y el historico lo planteamos a ese value" — the panel gains a window covering the last 365 days.
- **S3 — Read Pi sessions.** Decision: "Ventana 'Todo' + leer Pi + registro propio". Pi keeps its sessions in
  `~/.pi/agent/sessions/*/*.jsonl` (since 2026-09-20 on this machine); Orbitbar reads them as a fourth agent,
  counting only usage that is not already counted elsewhere: Pi's `claude-bridge` turns also appear in Claude
  Code's transcripts and share no id with them, so they count only for a (day, project) where Claude has no data.
- **S4 — Orbitbar's own ledger.** Same decision: "registro propio". Orbitbar keeps daily per-agent, per-project
  aggregates in its own config directory, so usage an agent later deletes (Claude Code deletes transcripts after
  `cleanupPeriodDays`) stays in the history. The ledger keeps 365 days (S2). Orbitbar still never writes to the
  agents' files.
- **S5 — Same repository under two paths.** `C:\repositories` is a junction to `R:\repositories`; a project
  reached through both paths is one project row.
- **S6 — macOS rendering.** User: "en la MAC de una amiga se ve malisimamemente la interface visual" (macOS Sonoma
  14.8.9 via OCLP 2.5.1, MacBookPro11,5 Retina). Two causes: the window is not transparent on macOS without
  Tauri's private API (white background), and window sizes are physical pixels while the page lays out in CSS
  pixels (at scale 2 the window is half the size it needs: bar clipped, panel cut). Decision: "Solo arreglar y
  publicar en el próximo release" — no Mac test before release.
- **S7 — Claude retention (outside the repo).** User: "Menos tiempo 365 dias": `cleanupPeriodDays: 365` in
  `~/.claude/settings.json`. Done 2026-11-17, not part of the repository.

## Design (T3-T5, decided by the orchestrator)

- **D1 — Daily buckets (claude, pi).** Claude and Pi entries carry their own timestamps, so a past day never
  changes. Their readers aggregate into `(local date, project key)` buckets holding raw counters (input, output,
  cache read, cache write 5m/1h, reasoning, entries) plus the bucket's last model; `""` is the no-project key.
  The window report is a fold over the dates in `[start.date, end.date)` (every window is whole local days).
  Cost is priced per bucket and summed; a total's cost is `None` only when no bucket in it could be priced.
  `read_usage(root, start, end, ...)` keeps its signature for codex/opencode and the existing tests.
- **D2 — 365-day window.** `TimeWindow::Last365Days` (`last365Days`, label "365 days"): today plus the previous
  364 days. Codex and OpenCode keep their current readers (their stores never delete history) and just get the
  new window.
- **D3 — Junction-aware roots (S5).** `resolve_project_root` resolves an existing cwd through the file system
  (`std::fs::canonicalize`, `\\?\` prefix stripped, then `normalize_project_path`) before the Git walk, so
  `C:\repositories\x` and `R:\repositories\x` produce the same key. A missing path keeps today's rules.
- **D4 — Pi reader (S3).** Root `~/.pi/agent/sessions`; every `*.jsonl` below it. The first line
  (`type: "session"`) carries `cwd`; usage lines are `type: "message"` with `message.role == "assistant"` and
  `message.usage` = `{input, output, cacheRead, cacheWrite, totalTokens, cost{total}, reasoning?}`, plus
  `message.provider` and `message.model`; the entry `timestamp` is RFC 3339. Dedupe per file by entry `id`.
  Agent id `pi`, display name "Pi". Cost: `ApiEquivalent` from the price table by model (cache write priced as
  5-minute). A `claude-bridge` turn is also in Claude Code's transcripts and shares no id with them, so it counts
  only when Claude (source or ledger) has no bucket for the same `(date, project)`; every other provider always
  counts. No default bar cell: Pi shows in the all-agents panel and through `agent-usage:pi`.
- **D5 — Ledger (S4).** `usage-ledger.json` next to `config.json`, written only by Orbitbar:
  `{ "version": 1, "days": [ { agent, date: "YYYY-MM-DD", project, model, input, output, cacheRead,
  cacheWrite5m, cacheWrite1h, reasoning, entries } ] }`. Holds claude and pi buckets only (codex and opencode
  sessions move between days as they grow, and their stores keep history). Merge per `(agent, date, project)`:
  the record with more entries wins, the source on a tie. Records older than 365 days are pruned. Written only
  when it changed, through a temp file plus rename, under a process-wide lock. An unreadable or unknown-version
  file is never overwritten: the ledger is skipped for that call and the agent's `detail` says so.

## Tasks

| ID | Specs | Route | Status | Commit |
| --- | --- | --- | --- | --- |
| T1 | S6 | inline | done | `ba84d11` |
| T2 | S1 | inline | done | `1424012` |
| T3 | S2, S5 | gentle-ai-worker (context backstop): D1-D3 | done | `1dedbda` |
| T4 | S3 | gentle-ai-worker: D4 | done | `cba2792` |
| T5 | S4 | gentle-ai-worker (stalled before reporting; parent checked and mutation-tested) | done | `471e77d` |
| T6 | S1-S6 | gentle-ai-verify; blocker B1 fixed inline | done | see L6 |
| T7 | all | PR to `dev`, `ci-gate`, merge | pending | |

## Log

- **L1** (2026-11-17, user): "Algunos problemas que debemos revisar e intentar resolver. Este mes trabaje por
  ejemplo en black-roasters y no lo veo computador en las metricas de la hotbar, ademas me gustaria tener un
  historico total, porque hay proyectos que no toco hace mucho mas de 1 o 2 meses y quiero saber las metricas y las
  perdi. Esto se pierde por cuestiones de lo que registran los agentes o lo podriamos recuperar? Y algo mas
  importante aun, en la MAC de una amiga se ve malisimamemente la interface visual [screenshot] macOS Sonoma
  14.8.9, instalado con OpenCore Legacy Patcher (OCLP) 2.5.1 en tu MacBook Pro (modelo MacBookPro11,5)."
- **L2** (2026-11-17, evidence): parity run (`cargo test --lib -- --ignored parity`) on the real home, This month:
  claude 18 projects, `black-roasters-store` 15th with 125 entries; codex 0 entries; Pi `openai-codex` 1081
  messages this month not counted anywhere. Oldest Claude transcript 2026-10-19 (default 30-day cleanup).
- **L3** (2026-11-17, user decisions): projects "Top 5 + 'ver todos'"; history "Ventana 'Todo' + leer Pi + registro
  propio (Recomendado)"; retention "Menos tiempo 365 dias y el historico lo planteamos a ese value"; Mac "Solo
  arreglar y publicar en el próximo release". Interpretation recorded: `cleanupPeriodDays = 365`, the history
  window is the last 365 days and the ledger keeps 365 days.
- **L4** (2026-11-17, evidence T4): parity This month: pi 1081 entries (all `openai-codex`), 7650 `claude-bridge`
  turns skipped as already counted by Claude. Pi's model `gpt-6-sol` (4881 turns) and
  `claude-sonnet-4-5-20250929` have no price row, so Pi's cost shows `no price data`; no price rows were added.
- **L5** (2026-11-17, evidence T5): the worker stalled after writing the ledger and its tests, without a report or
  RED evidence. Parent ran tsc, `cargo test --lib` (206 passed) and clippy (clean), and a mutation check in place
  of RED: `merge` made to drop stored buckets fails 4 tests (merge rule, deleted transcript, ledger-only bridge day,
  concurrent collects).
- **L6** (2026-11-17, verify): gentle-ai-verify on `471e77d`: S1, S2, S3, S6, S7, D1, D2, D4 PASS; S4/S5 FAIL on
  blocker B1 — a repository reached through a junction keyed `real\proj` while it existed and `link\proj` after
  deletion, so the ledger counted its days twice. Fixed by rebuilding the missing part of a deleted cwd on top of
  its resolved surviving ancestor (RED: `left: ...\link\proj`, `right: ...\real\proj`; GREEN: 207 passed, clippy
  clean). Advisories left as follow-ups: A2 (a Pi bridge turn stored before Claude's file existed for that day
  stays counted), A3 (time-zone change can shift days), A4 (no Claude store hides its ledger days), A5 (more
  entries wins undercounts a day partly deleted and partly new), A6 (unknown ledger fields dropped on rewrite).
- **L7** (2026-11-17, evidence): read-only parity over 365 days: claude 51335 entries / 19 projects, codex 363 /
  20, opencode 69 / 9, pi 13354 entries (9.96M output tokens) recovered from Pi sessions, 12538 claude-bridge
  turns skipped as already counted by Claude.
