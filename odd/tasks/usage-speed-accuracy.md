# Feature: usage-speed-accuracy — fast reloads, per-model prices, project aliases, usage outside projects

Status: **in progress** (opened 2026-11-18). Branch `feat/usage-speed-accuracy` (from `dev`, `0e216cc`).
The usage panel stops re-reading every agent file on each load, prices each model at its own rate, lets the user
merge project rows, and shows usage that ran outside any repository instead of dropping it.

## Specs

- **S1 — Fast loads.** User: "podemos ser mas rapidos por ejemplo cuando calculamos el gasto de Claude en 1 solo
  dia? Porque tarda tanto la carga? Recalcula o que hace?" Diagnosis (L2): every load re-reads ~2.4 GB (Claude
  1.4 GB in 728 files, Pi 594 MB, Codex 374 MB) although 3 files changed that day. Chosen: "Carga rápida —
  Caché en memoria por archivo, solo se reprocesa lo que cambió; precarga en segundo plano al iniciar."
- **S2 — Project aliases.** Chosen: "Unir proyectos (alias) — En config.json, por ejemplo: sortea-wt → sortea,
  para unir filas de worktrees borrados."
- **S3 — Price per model.** Chosen: "Precio por modelo — Agrupar también por modelo para que cada uno se cobre a
  su precio (corrige ~1 %)."
- **S4 — Usage outside projects.** Chosen: "Mostrar lo no contado — Una fila 'Fuera de proyectos' con las
  sesiones fuera de repositorios, en lugar de descartarlas."

## Design (decided by the orchestrator)

- **D1 — Per-model buckets (S3).** Daily buckets are stored per `(date, project, model)`. The window fold prices
  each bucket at its own model. A project row's `model` is the model with the most output tokens in the window.
  Ledger: file version 2 (records already carry `model`; v1 files load as-is and are rewritten as v2). Merge stays
  safe across the granularity change by comparing whole `(date, project)` groups: the side (source or ledger)
  whose group has more entries keeps all of its model buckets for that group, the source on a tie; a group only
  one side has is kept. This prevents a v1 record (all models under one model name) and new per-model source
  buckets from both counting.
- **D2 — Outside projects (S4).** Entries now excluded by `resolve_project_root` (temp dir, existing folder outside
  any repository) and entries with no cwd go to one project key `""`, counted in the agent totals and shown as a
  row named "Outside projects" (path empty) when it has entries; the `detail` text no longer says "not counted".
  Applies to all four agents (claude, codex, opencode, pi). Stored in the ledger like any project. README's
  "not counted" paragraph is rewritten.
- **D3 — Aliases (S2).** `config.json` gains `projectAliases`: an object mapping a path to a project path, e.g.
  `{ "R:\\repositories\\personal\\sortea-wt": "R:\\repositories\\personal\\sortea" }`. Applied at report time to
  the resolved project key: a key equal to an alias path or under it (whole path components, compared like
  `normalize_project_path` output, case-insensitive on Windows) becomes the alias target; the longest matching
  alias wins. Ledger and dedupe keys stay un-aliased, so changing an alias applies to all history at once.
  `get_usage` passes the aliases from the loaded config.
- **D4 — File cache (S1).** A process-wide cache keyed by file path holding `(length, modified time)` and that
  file's parsed, not-yet-resolved result (raw cwd, date, model, provider, counters), for the Claude, Pi and Codex
  readers. A file whose length and modified time are unchanged is not read again; a file that disappeared is
  dropped from the cache. Project-root resolution, the Pi claude-bridge rule, aliases and the window fold run on
  every call over cached data (cheap). A background warm-up runs one collect shortly after launch so the first
  panel open is fast too. The cache never changes results: a collect with a warm cache must equal a cold one.

## Tasks

| ID | Specs | Route | Status | Commit |
| --- | --- | --- | --- | --- |
| T1 | S3 | gentle-ai-worker: D1 (per-model buckets, ledger v2, group merge) | done | `ed92746` |
| T2 | S4 | gentle-ai-worker: D2 | done | `d9a1e16` |
| T3 | S2 | gentle-ai-worker: D3 | done | `99da098` |
| T4 | S1 | gentle-ai-worker: D4 | done | `68c4db6` |
| T5 | S1-S4 | gentle-ai-verify (high risk: ledger format, concurrency) | pending | |
| T6 | all | PR to `dev`, `ci-gate`, merge | pending | |

## Log

- **L1** (2026-11-18, user): "Genial ahi lo instale y mejoro un monton ahora tengo mas info. No se si estoy tan de
  acuerdo con el total de gastos para algunos proyectos pero bueno, deberia investigarlo. Puede ser que esten
  faltando detalle de gastos en algunso casos? Ademas, podemos ser mas rapidos por ejemplo cuando calculamos el
  gasto de Claude en 1 solo dia? Porque tarda tanto la carga? Recalcula o que hace?"
- **L2** (2026-11-18, investigation, read-only): duplicate lines per request are identical (18,686 requests, 0
  differences: no tokens lost by dedupe); 42 of 96 November (day, cwd) groups mix priced models, last-model
  pricing moves the month by about $16 of ~$3,850 (max ~$11 per project); deleted worktrees
  `sortea-wt\t1`..`t6` show as separate projects; 933 Claude entries this month were outside any project and not
  counted; Pi `gpt-6-sol` has no price row.
- **L3** (2026-11-18, user decision): selected all four: "Carga rápida (Recomendado)", "Unir proyectos (alias)",
  "Precio por modelo", "Mostrar lo no contado".
- **L4** (2026-11-18, T3 worker): separate bug found, not fixed here: a missing folder directly under a Windows
  drive root gets a key like `R:name` (the separator is dropped). Follow-up.
