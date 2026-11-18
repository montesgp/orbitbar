# Feature: auto-update — signed automatic updates from GitHub Releases, then release 0.3.0

Status: **in progress** (opened 2026-11-17). Branch `feat/auto-update` (from `dev`, `a28b148`).
Orbitbar updates itself from the repository's GitHub Releases (on by default, a menu toggle turns it off); then
everything on `dev` ships as 0.3.0.

## Specs

- **S1 — Updates on by default, can be turned off.** User: "Si metamos ese cambio como posibilidad de no
  utilizarlo, pero la funcionalidad que venga por defecto asi quien la utiliza, puede recibir los cambios cada vez
  que yo actualice."
- **S2 — Toggle in the bar's options.** User: "Que este en las opciones de la hotbar." — an entry in the bar's
  context menu, checked while automatic updates are on.
- **S3 — One release with everything.** User: "Publiquemos todo junto asi si tenemos un error en lo otro que
  fixeamos, corregimos y Mara no tiene que reinstalar, simplemente hace update de la app. Lo mismo que las demas
  personas que la descarguen de ahora en adelante." — the updater and the `usage-history-macos` work ship together
  as 0.3.0; later releases reach 0.3.0+ installs without a reinstall.
- **S4 — Signed updates.** Agreed proposal: "la app consulta un archivo `latest.json` publicado en los GitHub
  Releases de tu repositorio ... la descarga, verifica su firma y la instala." Only updates signed with the
  project's updater key install. The private key is never committed or printed.

## Design (decided by the orchestrator)

- **D1 — Plugin.** `tauri-plugin-updater` (Rust API; no JS permission needed). `tauri.conf.json`:
  `bundle.createUpdaterArtifacts: true`; `plugins.updater.pubkey` = the public key below;
  `plugins.updater.endpoints` = `["https://github.com/montesgp/orbitbar/releases/latest/download/latest.json"]`;
  `plugins.updater.windows.installMode = "passive"`.
  Public key: `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRFRTQwM0YxRkVDN0REODcKUldTSDNjZis4UVBrVG1DcHZ4ZzVDK202VlVjSHQ1WXY5NnExdUlsdm9iOThuczJxRnEvTWI4NWgK`
- **D2 — Config.** `AppConfig.auto_update: bool`, JSON `autoUpdate`, default `true` (an existing config without
  the field gets `true`, like `autoStart`).
- **D3 — When it runs.** A background task started in `setup`: first check about 30 s after launch, then every
  6 hours. Each tick re-reads `config.json`; when `autoUpdate` is false it does nothing (so the toggle works without
  a restart). On an available update: download, verify (the plugin does), install, then `app.restart()`. Failures
  are logged and never fatal. A debug build never checks (like autostart). On Linux it checks only when running as
  an AppImage (`APPIMAGE` env set): `.deb`/`.rpm` installs are updated by the package manager.
- **D4 — Menu.** "Automatic updates" checkbox entry after "Start with system"; toggling saves `autoUpdate`. The
  menu height constants in `main.ts` grow by one entry.
- **D5 — Release workflow.** `tauri-action` gets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from repository secrets (created 2026-11-17) and uploads `latest.json`.
- **D6 — Docs.** README's "no network calls" promise, AGENTS.md's rule and docs/architecture.md describe the one
  network call (the update check to GitHub Releases, on by default, menu toggle). CONTRIBUTING's release process
  names the two secrets and that losing the private key means installs can no longer be updated.

## Tasks

| ID | Specs | Route | Status | Commit |
| --- | --- | --- | --- | --- |
| T1 | S4 | inline: key pair in `~/.tauri/orbitbar-updater.key{,.password,.pub}`, two repo secrets | done | (outside the repo) |
| T2 | S1, S2, S4 | gentle-ai-worker: D1-D6 | done | `706a25c`, `868eb76`, `018536d` |
| T3 | S1-S4 | gentle-ai-verify (high risk: release, security); findings fixed inline | done | see L4 |
| T4 | S3 | inline: bump 0.3.0, PR to `dev`, promotion PR to `main`, tag `v0.3.0` | pending | |
| T5 | S3, S4 | inline: check the release has `latest.json` and signatures; local end-to-end update test | pending | |

## Log

- **L1** (2026-11-17, user): "Para ver impactado los cambios que deberia hacer tanto yo como Mara? Reinstalar la
  app? Existe la posibilidad de que la app tenga un upgrade con mi github? Seria que la app le pegue a una api de
  tercero para hablar con Github pero es un proveedor completamente confiable y reconocido y seguro"
- **L2** (2026-11-17, user): "Si metamos ese cambio como posibilidad de no utilizarlo, pero la funcionalidad que
  venga por defecto asi quien la utiliza, puede recibir los cambios cada vez que yo actualice. Que este en las
  opciones de la hotbar. Publiquemos todo junto asi si tenemos un error en lo otro que fixeamos, corregimos y Mara
  no tiene que reinstalar, simplemente hace update de la app. Lo mismo que las demas personas que la descarguen de
  ahora en adelante."
- **L3** (2026-11-17, T1): key pair generated with a random password; the key and password files are readable
  only by the user; secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` created (no
  secret existed before).
- **L4** (2026-11-18, verify): no blockers; S1, S2, S4, D1-D4, D6 PASS; D5 wiring PASS. Platform keys confirmed in
  the tauri-action v1 source (`upload-version-json.ts`: a universal macOS build writes `darwin-aarch64` and
  `darwin-x86_64`). Fixed: the update tick read the config through `config::load`, which renames and resets an
  invalid file, so it now uses the read-only `config::read_auto_update` (RED: function missing; GREEN 215 passed);
  the workflow check parses an ArrayBuffer, a string or JSON; clearer re-run message; stale doc references; a test
  pins the pubkey, endpoint, passive install mode and `createUpdaterArtifacts`.
