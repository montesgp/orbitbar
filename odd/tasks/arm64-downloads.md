# Feature: arm64-downloads — ARM64 installers and architecture-aware download page

Status: **in progress** (opened 2026-11-12). Branch `feat/arm64-downloads` (from `dev`, `9dc7a42`).
Release builds gain Linux ARM64 and Windows ARM64; the download page offers them when it can tell the visitor runs ARM.

## Specs

- **S1 — Linux ARM64 release build.** The Release workflow builds Linux ARM64 installers on the native
  `ubuntu-22.04-arm` runner. Agreed proposal: "Linux ARM64 | `ubuntu-22.04-arm` (ARM nativo) |
  `_arm64.AppImage`, `_arm64.deb`, `.aarch64.rpm`" (exact file names are whatever the Tauri bundler produces;
  confirmed by the build-only run, S6).
- **S2 — Windows ARM64 release build.** The Release workflow builds a Windows ARM64 installer. Agreed proposal:
  "Windows ARM64 | `windows-11-arm` (o compilación cruzada con `--target aarch64-pc-windows-msvc`) |
  `_arm64-setup.exe`" and "para Windows ARM habría solo `.exe` (NSIS), sin `.msi`". Chosen: cross-compile on
  `windows-latest`, the path the Tauri docs document.
- **S3 — Architecture detection.** Agreed proposal: "Linux ARM | Casi siempre. El user agent dice `Linux aarch64`",
  "Windows ARM en Chrome o Edge | Sí, con `navigator.userAgentData.getHighEntropyValues(['architecture'])`",
  "Windows ARM en Firefox | **No.**".
- **S4 — Download choice.** Agreed proposal: "si se detecta ARM, el botón principal ofrece ARM64. Si no se sabe,
  queda x64 como hoy (en Windows ARM igual funciona por emulación) y se agrega un enlace visible "ARM64" junto a
  los formatos alternativos. Así nadie se queda sin descarga." When the latest release has no ARM64 installer
  (v0.2.0 today), the page behaves exactly as before.
- **S5 — Docs.** Agreed proposal: "También hay que actualizar la tabla de archivos en `README.md:42-44`."
  The release process docs and the page's first-run help name the ARM64 files too.
- **S6 — Build-only test run.** Agreed proposal: "Lo validaría primero con una corrida de prueba que solo
  compile, sin publicar nada." It must not create, change or publish any release.
- **S7 — Delivery.** Agreed proposal: "¿Lo implemento en una rama desde `dev`, con un PR que incluya workflow,
  página y README, más esa corrida de prueba de solo compilación?" — user: "Si implementemos eso".

## Tasks

| ID | Specs | Route | Status | Commit |
| --- | --- | --- | --- | --- |
| T1 | S1, S2, S5 | inline | done | `76115a0` (workflow), docs in T2 commit |
| T2 | S3, S4, S5 | inline | done | (this commit) |
| T3 | S6 | inline (CI on a throwaway branch) | done (branch delete pending) | run 43285440827 |
| T4 | S1-S6 | gentle-ai-verify (Risk item 5: release workflow) | pending | |
| T5 | S7 | inline: PR to `dev`, `ci-gate`, merge | pending | |

- T1: release matrix entries for Linux ARM64 and Windows ARM64; README file table; CONTRIBUTING release step.
- T2: `site/main.js` arch detection and ARM64 links; `site/index.html` links row and help text.
- T3: temporary workflow that builds only the two ARM64 entries and uploads the bundles as artifacts; read the
  real file names; delete the throwaway branch afterwards.
- T4: independent verification of S1-S6.
- T5: PR against `dev`, wait for `ci-gate` SUCCESS on the head commit, merge. Promotion to `main` and the
  release tag stay with the owner.

Delivery: single PR, forecast ~200 authored changed lines (under the ~400 budget).

## Log

- L1 (user, 2026-11-12): "La pagina de website asoaicada a este repo hace autodetect del cliente que consume para
  mostrar el download por default asociado? Es decir si consumen de mac mostramos esa descarga y asi con todos"
- L2 (user): "Y que deberiamos hacer para cubrir esos casos? Es muy costoso? De lo contrario lo hariamos"
- L3 (user): "Si implementemos eso"
- L4 (agent): Repo is public, so ARM runners are free. Windows ARM64 cross-compiles on `windows-latest`
  (Tauri docs: `--target aarch64-pc-windows-msvc`, NSIS installer stays x86 under emulation). Chrome's reduced
  user agent reports `Linux x86_64` on every Linux, so Linux ARM in Chromium also needs client hints.
- L5 (agent, T3): build-only run 43285440827 on throwaway branch `test/arm64-build` (temporary workflow mirroring the
  two ARM64 matrix entries, no release step): both jobs success in 8m36s. Bundles: `Orbitbar_0.2.0_arm64-setup.exe`,
  `Orbitbar_0.2.0_arm64_en-US.msi` (correction: WiX does build an ARM64 MSI, the proposal said it would not),
  `Orbitbar_0.2.0_aarch64.AppImage`, `Orbitbar_0.2.0_arm64.deb`, `Orbitbar-0.2.0-1.aarch64.rpm`. Header check without
  running anything: `orbitbar.exe` in the MSI and NSIS is PE ARM64 (NSIS stub x86); `orbitbar` in the deb and the
  AppImage runtime are ELF AArch64. No release was created or changed.
- L6 (agent, T2): no frontend test runner; scratch harness (Node vm, fake DOM/navigator/fetch, outside the repo) ran
  base `9dc7a42` vs new `site/main.js` over 14 browsers x {release without ARM64, with ARM64, offline}. RED on base:
  29 failures. GREEN on new: all checks; differential: without ARM64 assets or offline every element is identical.
