# Orbitbar

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

Orbitbar is a small bar that stays where you put it on your screen, always
visible above your other windows. It shows the token usage and cost of your local AI
coding agents (Claude Code, Codex, OpenCode and Pi): overall, per project, and
for the time window you pick. It runs on Windows, macOS and Linux.

| Dark | Light |
| --- | --- |
| ![Orbitbar in the Dark theme with the usage panel open on 7 days](docs/images/orbitbar-dark.png) | ![Orbitbar in the Light theme with the usage panel open on 7 days](docs/images/orbitbar-light.png) |

## What you get

- **A bar that is always there.** It starts at the right edge of your primary
  monitor, can be dragged anywhere on any monitor and stays where you drop it,
  collapses to a small tab, and opens a usage panel and a menu beside it, on
  whichever side has room. Only one Orbitbar runs at a time.
- **Usage per agent and per project.** Output tokens and cost for Today,
  the last 7 days, the last 30 days, the current month or the last 365 days.
- **Local data only.** Orbitbar reads the session history your agents
  already keep on disk. It needs no account and sends no telemetry. Its one
  network call is the update check against this repository's GitHub
  Releases, which you can turn off. See [Updating](#updating).
- **A year of history.** Orbitbar keeps its own daily totals for Claude Code
  and Pi, so usage stays visible after an agent deletes old sessions. See
  [History](#history).
- **Sourced prices.** A built-in price table with a per-user override file.
  A model without a price entry shows `no price data`.
- **Light and dark themes**, switched live from the bar's menu.
- **One JSON config file** for items, theme, autostart and placement.

## Download and install

The easiest way is the [download page](https://montesgp.github.io/orbitbar/):
it detects your system and offers the right installer from the latest
release in one click.

You can also get the installers from the
[Releases page](https://github.com/montesgp/orbitbar/releases/latest) and pick
the file for your OS:

| OS | File to pick |
| --- | --- |
| Windows | `Orbitbar_<version>_x64-setup.exe` (recommended) or `Orbitbar_<version>_x64_en-US.msi` |
| Windows on Arm | `Orbitbar_<version>_arm64-setup.exe` or `Orbitbar_<version>_arm64_en-US.msi` (the x64 installers also run, through emulation) |
| macOS | `Orbitbar_<version>_universal.dmg` (Apple silicon and Intel) |
| Linux | `Orbitbar_<version>_amd64.AppImage` (run it directly), `.deb` (Debian, Ubuntu) or `.rpm` (Fedora, openSUSE) |
| Linux on ARM64 | `Orbitbar_<version>_aarch64.AppImage`, `Orbitbar_<version>_arm64.deb` or `Orbitbar-<version>-1.aarch64.rpm` |

The download page picks the ARM64 build when the browser reports an Arm
processor (Chrome and Edge do; Firefox does on Linux). Otherwise it offers the
x64 build and links the ARM64 files below the button.

Agents are optional: Orbitbar reads whichever of Claude Code, Codex and
OpenCode are installed and reports the others as not installed. SQLite is
compiled into the binary, so there is nothing else to install.

### Updating

From 0.3.0 on, Orbitbar updates itself. About 30 seconds after it starts,
and then every 6 hours, it downloads `latest.json` from this repository's
[latest release](https://github.com/montesgp/orbitbar/releases/latest). When
that names a newer version, Orbitbar downloads it, checks its signature
against the project's updater key, installs it and restarts. An update that
is not signed with that key is never installed. Nothing else is sent or
received.

- Updates are on by default. Turn them off with **Automatic updates** in the
  bar's menu, or set `autoUpdate` to `false` in `config.json`. The change
  applies from the next check, without a restart.
- On Windows the update runs the installer in passive mode: a progress
  window shows briefly, with no questions.
- On Linux only the AppImage updates itself. A `.deb` or `.rpm` install is
  updated like any other package: install the new one from the latest
  release with your package manager.
- Versions before 0.3.0 do not update themselves: close the bar and run the
  installer from the latest release once. Your `config.json` is kept.

### First run: unsigned installers

The installers are not code-signed, so each OS asks for confirmation the
first time you open them:

- **Windows:** SmartScreen shows "Windows protected your PC". Choose
  **More info**, then **Run anyway**.
- **macOS:** Gatekeeper blocks the app on first launch. Right-click
  `Orbitbar.app` and choose **Open**, then confirm; or open **System Settings
  > Privacy & Security** and choose **Open Anyway**.
- **Linux:** make the AppImage executable and run it:
  `chmod +x Orbitbar_*.AppImage && ./Orbitbar_*.AppImage`. The `.deb` and
  `.rpm` packages install with your package manager
  (`sudo apt install ./Orbitbar_*.deb`, `sudo dnf install ./Orbitbar_*.rpm`).

### Per-OS details

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Runtime | WebView2, preinstalled on Windows 10 and 11 | WKWebView, part of macOS | WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`), installed as a dependency by the `.deb` and `.rpm`; the AppImage expects it on the system |
| Config | `%APPDATA%\com.orbitbar.app\config.json` | `~/Library/Application Support/com.orbitbar.app/config.json` | `~/.config/com.orbitbar.app/config.json` |
| Autostart | A per-user Registry Run entry | A LaunchAgent | An XDG autostart `.desktop` entry |

### Platform status

| OS | Status |
| --- | --- |
| Windows | Verified manually. Built by CI. |
| macOS | Built by CI. Not yet verified manually. |
| Linux | Built by CI. Not yet verified manually. |

## Build from source

For contributors and anyone who wants to build their own binary. Install
Node.js, [Rust](https://rustup.rs/) and the build prerequisites for your OS:

| OS | Build prerequisites |
| --- | --- |
| Windows | [Node.js](https://nodejs.org/), Rust with the MSVC toolchain (`rustup default stable-msvc`), and the Visual Studio C++ Build Tools ("Desktop development with C++") |
| macOS | [Node.js](https://nodejs.org/), Rust, and the Xcode Command Line Tools (`xcode-select --install`) |
| Linux | [Node.js](https://nodejs.org/), Rust, and the WebKitGTK 4.1 development packages (below) |

On Debian and Ubuntu, install the Linux build dependencies with:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

For Fedora, Arch and other distributions use the package list in the
[Tauri prerequisites guide](https://tauri.app/start/prerequisites/).

Then:

```sh
cd app
npm ci
npm run tauri build   # installer or app bundle for the current OS
```

Use `npm run tauri dev` instead to run with hot reload while you work on it.

The installer build also signs the update bundles, so without the project's
updater key (`TAURI_SIGNING_PRIVATE_KEY`) `npm run tauri build` fails at
the signing step. Skip the update bundles with
`npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'`,
or build only the binary with `npx tauri build --no-bundle`. A copy you
build yourself still checks this repository's releases for updates unless
you turn **Automatic updates** off.

`npm run tauri build` writes the installers under
`app/src-tauri/target/release/bundle/`, in one folder per format
(`nsis`, `msi`, `dmg`, `macos`, `deb`, `rpm`, `appimage`). Each OS produces
its own formats; build on the OS you are targeting. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the checks and the release process.

## Using Orbitbar

- **Click an agent cell** to open its usage panel. The panel always opens on
  Today; its selector switches to 7 days, 30 days, this month or 365 days while the
  panel is open. Click the same cell again, press Escape, or use the close
  button to close it.
- **Right-click the bar** (or the collapsed tab), or **left-click the
  settings cell**, for the menu: edit `config.json`, open `pricing.json`,
  collapse or expand, reload the config without a restart, switch between
  Light and Dark, turn **Start with system**, **Automatic updates** or
  **Show example action** on or off, or quit. Escape or a click outside closes the menu.
- **Drag the bar** to move it anywhere; it stays where you drop it, keeps
  inside the monitor's usable area (not under the taskbar) and comes back
  there after a restart. The panel and the menu open on the bar's left, or on
  its right when there is no room on the left, and never move the bar. Use the
  chevron at the top to collapse it to a tab; the tab keeps the bar's place.
- **Start it again** while it is running and the second launch exits and brings
  the running bar forward.

A block in the panel looks like this:

```text
Claude Code                                 Today ▾
------------------------------------------------------
output tokens   211,300      cost  ~$53.04 (API-equiv.)
------------------------------------------------------
project              output tokens        cost
my-app                     180,400       ~$45.10
api-service                 30,900        ~$7.94
```

- `~$X (API-equiv.)` marks an estimate: Claude Code and Codex report token
  counts, and Orbitbar prices them at the provider's public API rate.
- A plain `$X` is a cost OpenCode reports itself, shown as-is.
- `no price data` means the model has no entry in the built-in table or your
  override.

## Configuration

Orbitbar creates `config.json` with defaults on first launch (paths per OS in
[Per-OS details](#per-os-details)) and rewrites it when you change a setting from the app.

| Field | Meaning |
| --- | --- |
| `theme` | `dark` (default) or `light`. Any other value resolves to `dark`. |
| `fontSize` | Base font size in pixels. |
| `monitor` | Monitor for the first placement, and the one Orbitbar records when you move the bar; `primary` by default. |
| `margin` | Gap in pixels between the bar and the right screen edge when it is placed at the start (no saved `position`, or a saved one that is off every monitor). |
| `position` | Where the bar was dropped: `{ "x": 1836, "y": 340 }`, its top-left corner in physical screen pixels (negative on a monitor left of or above the primary). Written when you drag the bar or collapse/expand it; absent until then. Read at startup: a position that is partly off-screen is pulled back in, one that is off every monitor is ignored and the bar goes to the right edge of `monitor`. Delete the field to reset the placement. |
| `collapsed` | Whether the bar starts as the small tab. |
| `autoStart` | Start Orbitbar at login (on by default; release builds only). The menu's **Start with system** entry changes it for you: its check mark shows the real OS registration, and selecting it updates the OS entry and this field. |
| `autoUpdate` | Check this repository's GitHub Releases for a signed update and install it (on by default; release builds only, and on Linux only the AppImage). The menu's **Automatic updates** entry changes it for you. See [Updating](#updating). |
| `showExamples` | Show the example cells (items flagged `"example": true`). Off by default. The menu's **Show example action** entry changes it for you. |
| `items` | The cells on the bar, each with `id`, `glyph`, `label`, `tooltip` and `action`, and optionally `"example": true`. |

Item actions:

| Action | What it does |
| --- | --- |
| `agent-usage` | Opens the usage panel with every agent. |
| `agent-usage:claude`, `agent-usage:codex`, `agent-usage:opencode`, `agent-usage:pi` | Opens the panel for one agent. Pi has no cell of its own by default. |
| `toggle-autostart` | Turns autostart on or off and updates the OS entry. |
| `edit-config` | Opens the bar's menu at the click; "Edit config" there opens `config.json` in your default editor. |
| `open:<url>` | Opens an `http://` or `https://` URL in your default browser. Other schemes are rejected and the panel shows why. |
| `run:<program> [args]` | Starts a program with its arguments when you click the cell. See below. |
| `panel:<program> [args]` | Runs a program when you click the cell and shows what it prints in the panel. See below. |

`run:` splits the text after the colon on whitespace into a program and its
arguments; double quotes group an argument that contains spaces. Orbitbar
starts the program directly, without a shell, so `&&`, `|`, `>` and `%VAR%`
are passed to the program as plain text. The program runs detached: Orbitbar
does not wait for it, and it keeps running if you quit the bar. It starts only
when you click the cell. If it cannot be started, the panel shows the error.
A console program opens its own console window; a graphical program opens
only its own window. On Windows a bare program name is looked up on `PATH` with
each `PATHEXT` extension, so `run:code` finds VS Code's `code.cmd` and other
`.cmd`/`.bat` launchers, not only `.exe` files. The command comes from
`config.json`, which Orbitbar reads when you click the cell.

```json
{ "id": "editor", "glyph": "0x270E", "label": "editor", "tooltip": "Open my project", "action": "run:code \"C:/Projects/my app\"" }
```

| OS | Example action |
| --- | --- |
| Windows | `run:notepad.exe`, or `run:code C:\Projects\my-app` |
| macOS | `run:open -a Terminal` (`open` is a program on macOS) |
| Linux | `run:code /home/me/projects/my-app` (there is no shell, so `~` is not expanded: use the full path) |

`panel:` takes the same command line (same splitting, quoting, no shell, same
program lookup) but shows the program's output in the panel instead of
detaching it. The program runs once when you click the cell, with no console
window and no input; Orbitbar waits for it and shows what it printed to
standard output as plain preformatted text, terminal colors and cursor codes
removed. If it exits with an error, its standard error and exit code are shown;
if it has not finished after 10 seconds it is stopped and the panel says so;
output beyond 64 KiB is cut. Click the cell again, press Escape or use the
close button to close the panel; click the cell again after closing to run it
again. It has no refresh timer.

```json
{ "id": "disk", "glyph": "0x25A4", "label": "disk", "tooltip": "Disk usage", "action": "panel:df -h" }
```

The default config includes an example cell, `github`, whose action uses
`open:` to open the Orbitbar repository. It illustrates what a cell can do
(`open:` a page, `run:` a program) and is hidden by default: items flagged
`"example": true` appear only while `showExamples` is `true`. Turn it on with
the menu's **Show example action** entry, or set `"showExamples": true` in
`config.json`. Flag your own demonstration cells the same way. A `config.json`
you already have is not rewritten, so add the `github` item there by hand
(with `"example": true`) if you want it.

If `config.json` cannot be parsed, Orbitbar renames it to
`config.json.invalid` and starts on the defaults.

### Custom model prices

Built-in prices live in `app/src-tauri/src/usage/pricing.rs`, one row per
model with a source URL and an as-of date. To add or override a model, choose
"Open pricing file" in the bar's menu, which creates `pricing.json` next to
`config.json` from a template, or copy
[`app/pricing.example.json`](app/pricing.example.json) there yourself.
Matching is by exact model id, then by the longest id prefix, so a dated
model id still resolves against a shorter entry. The file is reloaded on every
panel refresh; a malformed `pricing.json` falls back to the built-in table
with a warning.

## Data sources

| Agent | Location read | Access |
| --- | --- | --- |
| Claude Code | `~/.claude/projects/**/*.jsonl` | Read-only |
| Codex | `~/.codex/sessions/**/*.jsonl` | Read-only |
| OpenCode | `~/.local/share/opencode/opencode.db` | Read-only SQLite |
| Pi | `~/.pi/agent/sessions/**/*.jsonl` | Read-only |

`~` is your home directory on every OS. See
[docs/architecture.md](docs/architecture.md) for how the readers work.

Pi turns run through its `claude-bridge` provider are Claude Code sessions,
so Claude Code's own files hold them too. Pi counts them only for a day and
project that Claude has no usage for, and its panel says how many it left
out; turns from every other Pi provider always count.

Usage is grouped by project: the repository a session ran in. Runs that are
not part of a project are not counted, neither in the project list nor in the
agent totals: sessions started in your operating system's temporary folder
(for example a tool that spawns short-lived agent runs there), and sessions
started in a folder that is not inside any Git repository (such as your home
folder). A project whose folder was renamed or deleted still appears, as one
row named after the folder that is gone. A repository reached through two
paths (for example through a Windows junction or a symbolic link) is one
project, named by the path the link points to.

Each agent lists its 5 busiest projects by output tokens; **Show all N
projects** under the list expands it to every project in the window.

### History

Claude Code deletes session files older than its `cleanupPeriodDays` setting
(30 days unless you change it in `~/.claude/settings.json`), so their usage
would drop out of the panel. Orbitbar keeps daily totals per project for
Claude Code and Pi in `usage-ledger.json`, next to `config.json`, and keeps
them for 365 days. It holds token counts and the model only; costs are
computed when the panel opens, so `pricing.json` still applies. Codex and
OpenCode keep their whole history in their own stores, so they are not in it.

`usage-ledger.json` is the only file Orbitbar writes for usage; the agents'
files above are only read. If the ledger cannot be read, Orbitbar leaves it
as it is, shows the agents' own data only and says so in the panel.

## Extensions

Orbitbar has optional extensions for the Herdr TUI and the
OmniRoute gateway. They are off by default and add no requirements to the
bar. See [extensions/README.md](extensions/README.md).

## Roadmap

Planned work is tracked as GitHub issues labeled
[`enhancement`](https://github.com/montesgp/orbitbar/issues?q=is%3Aissue+label%3Aenhancement).
Automatic updates shipped in 0.3.0 (see [Updating](#updating)).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Working with an AI coding agent? Point
it at [AGENTS.md](AGENTS.md).

## License

[MIT](LICENSE)
