# Contributing

## How the project is run

- The owner, [@montesgp](https://github.com/montesgp), reviews and merges every
  contribution. Nobody else has write access.
- Everyone else contributes through **issues** and **pull requests from a
  fork**, following this guide.
- Workflows from forks run after the owner approves them, so your first CI run
  may wait for a moment.

## Development setup

Install the build prerequisites for your OS (see
[Build from source](README.md#build-from-source)), then:

```sh
cd app
npm ci
npm run tauri dev
```

`app/README.md` describes the project layout.

## Checks

CI runs these on Windows, macOS and Linux for every pull request. Run them
before opening one (`npm run build` first, because the Rust build embeds
`app/dist`):

```sh
cd app
npm ci
npx tsc --noEmit                                        # frontend type check
npm run build                                           # type check and Vite build
cd src-tauri
cargo test                                              # Rust unit tests
cargo clippy --all-targets -- -D warnings
```

There is no frontend test runner yet; `app/src/usage-view.ts` is kept free of
DOM code so it can be unit tested once one is added.

## Branches

```
feature branch ──► dev ──► main (GitHub Pages, tagged vX.Y.Z)
hotfix/... ──────────────► main ──► back into dev
```

- `dev` collects the work in progress. Create a feature branch from `dev`
  (`feat/...`, `fix/...`, `chore/...`) and open a pull request against `dev`.
- `main` holds the final, promoted version: GitHub Pages and releases come only
  from `main`. When the work on `dev` is complete and tested, the owner promotes
  it with a pull request from `dev` to `main`, then fast-forwards `dev` to
  `main` (`git push origin main:dev` once `ci-gate` is green on `main`) so both
  branches point at the same commit.
- `main` and `dev` are protected: they cannot be force-pushed or deleted, and
  every change lands through a pull request with a green `ci-gate` check. Only
  the owner merges, by hand or through the owner's coding agents. The single
  exception is the owner's fast-forward of `dev` to `main` right after a
  promotion, which adds no new commits.

### Hotfixes

Installed copies update themselves from the latest release, so a release
reaches every user within hours; most fixes still simply go through `dev`
like any other change. Use a hotfix only when a released version
is broken and `dev` holds work that is not ready to ship:

1. Create `hotfix/<short-name>` from `main`.
2. Fix the problem (with a test where the logic allows it) and bump the patch
   version (`X.Y.Z+1`) in the four places listed in
   [Release process](#release-process).
3. Open a pull request against `main`. The owner merges it once `ci-gate` is
   green and tags the patch release.
4. Bring the fix back into `dev` with a pull request from `main` to `dev`.

## Commit messages

This repo uses [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, ...). Keep the summary line
short and describe the "why" in the body when it is not obvious.

## Reporting an issue

Open a GitHub issue with the **Bug report** or **Feature request** form. A bug
report asks for:

- OS and version (Windows, macOS or Linux, with the build number if relevant).
- Orbitbar version, or the commit you built from.
- The agent or agents involved (Claude Code, Codex, OpenCode) if the report is
  about the usage panel.
- Steps to reproduce and what you expected instead.

Redact real token history, API keys and any file paths that include your
username before pasting.

## Release process

1. On a branch from `dev`, bump the version in all four places, keeping them
   equal:
   - `app/package.json` (and `app/package-lock.json`, via `npm install`)
   - `app/src-tauri/tauri.conf.json`
   - `app/src-tauri/Cargo.toml`
   - `app/src-tauri/Cargo.lock` (updated by running `cargo check` in
     `app/src-tauri`)
2. Merge the bump into `dev`, then merge `dev` into `main` through a pull
   request once CI is green.
3. Tag the release on `main` and push the tag:

   ```sh
   git checkout main && git pull
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

4. The **Release** workflow (`.github/workflows/release.yml`) builds the
   installers for Windows (x64, and ARM64 cross-compiled), macOS (universal)
   and Linux (x64, and ARM64 on the `ubuntu-22.04-arm` runner) with
   `tauri-apps/tauri-action` and publishes them as the GitHub Release
   "Orbitbar vX.Y.Z". The installers are not code-signed, but the update
   bundles are signed with the updater key (see
   [Updater signing key](#updater-signing-key)). Check the release page
   afterwards for the expected assets: the installers, each update bundle
   with its `.sig` file, and `latest.json`, which installed copies read to
   find the update. A `latest.json` that lacks a platform means that
   platform's installs do not see the release.
5. The [download page](https://montesgp.github.io/orbitbar/) follows the new
   release by itself: it reads the latest release from the GitHub API and
   picks each installer by its name pattern (`_x64-setup.exe`,
   `_arm64-setup.exe`, `_universal.dmg`, `_amd64.AppImage`,
   `_aarch64.AppImage`, ...). If the installer names change,
   update the patterns in `site/main.js`.

### Updater signing key

The Release workflow signs the update bundles with two repository secrets,
`TAURI_SIGNING_PRIVATE_KEY` (the private key) and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (its password). The matching public key
is `plugins.updater.pubkey` in `app/src-tauri/tauri.conf.json`, and every
installed copy accepts only updates signed with it.

- The key files live outside the repository, in the owner's `~/.tauri/`
  (`orbitbar-updater.key`, `.key.password`, `.key.pub`). Never commit or
  print them. Back them up somewhere safe: GitHub does not let anyone read a
  secret back.
- Losing the private key means existing installs can no longer be updated.
  A new key pair needs a new public key in `tauri.conf.json`, and copies
  with the old key accept nothing signed with the new one, so their users
  have to install the next release by hand.
- Building installers locally runs the same signing step: without
  `TAURI_SIGNING_PRIVATE_KEY` set, `npm run tauri build` fails. Build the
  binary with `npx tauri build --no-bundle`, or installers without update
  bundles with
  `npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Download page

The page in `site/` is plain HTML, CSS and JavaScript with no build step.
The **Pages** workflow (`.github/workflows/pages.yml`) publishes it to GitHub
Pages when a change to `site/` reaches `main`. To preview it, serve the folder
locally, for example with `python -m http.server --directory site`, and open
the printed address.

`site/og-image.png` (1200×630) is the preview that chats and social networks
show when the link is shared: the icon and the name, kept inside the centered
630×630 square that WhatsApp crops for its thumbnail. It is referenced by the
`og:image` tags in `index.html`; when you regenerate it, bump the `?v=` in
`og:image` so apps that cached the old one fetch it again.
