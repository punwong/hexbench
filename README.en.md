# HexBench

A small tray-resident toolbox for Windows 11. Three tools share one set of settings: switch virtual desktops by pushing the cursor to the screen edge, open a file panel with one hotkey, and pin folders to the desktop. The panel also carries notes, built-in web windows, and mouse gestures.

- Platform: Windows 10 1809+ / Windows 11 (x64)
- Stack: Tauri 2 + React 19 + Rust
- License: MIT

[中文文档](README.md) · [Changelog](docs/CHANGELOG.md)

---

## Features

| Tool | What it does | How to open |
| --- | --- | --- |
| Edge desktop | Push the cursor to the outermost screen edge to switch to the previous / next virtual desktop | Edge trigger; `Ctrl+Alt+Shift+P` to pause |
| Quick launcher | Search, browse and move files, write notes, open web panels | `Alt+1` |
| Desktop fences | Keep folders and drop buckets on the desktop | Tray menu, or Settings → Desktop fences |

Also included: built-in web windows, right-drag gestures, a single hotkey registry, WebDAV config sync, silent background updates, run at login, and light/dark themes.

---

## Install

Download the latest installer from [Releases](https://github.com/punwong/hexbench/releases/latest).

- Windows 10 1809+ / Windows 11, x64
- WebView2 runtime (preinstalled on Windows 11)
- The installer is signed with minisign for update verification; the executable itself is not code-signed yet

HexBench lives in the notification area. The first launch opens the settings window with a short onboarding dialog.

`winget/manifests` holds manifests that have not been merged into winget-pkgs yet, so `winget install` is not available.

---

## Usage

### Edge desktop

Push the cursor into the trigger band at the **outermost** left or right edge of the desktop and rest for about 0.12 s to switch to the previous or next virtual desktop.

- The band sits at the bottom of each monitor by default and covers 10% of that monitor's height; both sides are active
- Only the outermost edges trigger — the seam between two monitors is ignored
- Each monitor uses its own height, so mixed landscape/portrait setups behave the same
- Hot-plugging a display, changing resolution, rotating or changing scaling recalculates the layout automatically
- After a switch the cursor is pushed 24 px back into the screen so a single push cannot fire twice

Settings → Edge desktop:

| Option | Default | Description |
| --- | --- | --- |
| Edge | Bottom | Use Top when the taskbar is set to auto-hide |
| Sides | Both | Left only or right only also work |
| Band height | 10% | 3%–50% of the monitor height |
| Dwell | 120 ms | How long the cursor must stay in the band |
| Cooldown | 500 ms | Minimum time between two switches |
| Poll interval | 50 ms | How often the cursor position is checked |
| Push cursor back | On / 24 px | Moves the cursor back after switching |
| Guards | On | Do not trigger while a mouse button is held, over the taskbar, or when locked |
| Process blacklist | Empty | Do not trigger while one of these processes is in front (case-insensitive) |

**Preview on screen** draws the trigger band on your displays for a few seconds so you can check its position or debug false triggers; it never switches desktops. The pause hotkey is `Ctrl+Alt+Shift+P` by default and can be changed under App → Hotkeys.

### Quick launcher

Press `Alt+1` to open the panel near the cursor. It covers 60% × 60% of the work area by default.

**Finding and opening**

- The search box covers files in the current category, pinned folders and notes
- Double-click a file or folder to open it; a single click selects
- `Esc` closes the panel; outside the search box, `Backspace` goes up one level

**Files**

- Drag on empty space (or drag with `Alt`) to marquee-select; `Ctrl+A` selects all
- `Alt+wheel` zooms the icons
- Drop files onto a category or pinned folder to move them there; cross-drive moves show a progress bar and ask before overwriting
- Drag files out to Explorer or another application
- `Ctrl+C / Ctrl+X / Ctrl+V` share the system clipboard with Explorer
- Delete moves to the recycle bin; `Ctrl+Z` restores, including from the bin

**Categories and folders**

- Categories come from a root folder: every subfolder becomes a category
- Drag categories to reorder them; click the eye icon to hide one
- Right-click a category to create a subfolder or rename it
- Pinned folders hold other frequently used directories; add, remove and reorder them
- Right-click empty space for the native Explorer menu (New, Paste, Git, VS Code, archive tools and other installed shell extensions)

**The panel**

- Drag an edge to resize, drag the header to move; size and position are remembered (can be turned off)
- The panel follows the light/dark theme from settings

Launcher settings are split into four tabs: **Categories** (root folder, panel size, position), **Pinned folders**, **Web window**, and **Notes**.

### Desktop fences

Fences are folder windows that sit on the desktop and stay out of the taskbar.

- Settings → Desktop fences → **Add folder fence** to pick a folder, or **New drop bucket** to create a collection folder
- Drop files onto a fence to move them into its folder; drag files out of a fence to move them elsewhere
- The **⋯** menu on each fence offers rename, change folder, open in Explorer, sort by name / modified time / size / type, lock position, hide, and delete
- Deleting a fence removes only the window — **the folder and its files are kept** — and asks for confirmation first
- With **Stay visible on Win+D** enabled, fences remain visible when you show the desktop
- **Snap while moving** shows alignment guides and snaps fences; hold `Alt` to suspend snapping. **Snap to an 8 px grid** aligns positions that are not near another fence
- A scrollbar appears when content overflows; an empty folder shows “This folder is empty”
- The tray menu has **Show all fences** to bring back hidden ones

### Notes

Notes are for short text you retype often: email addresses, shipping details, support replies, snippets.

- The **Notes** section on the left of the launcher; settings → Quick launcher → Notes also lists and clears them
- Click a card to edit: title plus a Markdown body, saved as you type, `Ctrl+Z` to undo
- Drop images on empty space to create a note, or on a card to append to that note
- **Send** closes the panel, returns focus to the previous window and types the body out; images are skipped. **Copy images** puts a note's images on the clipboard as files
- **Send Enter afterwards** suits chat boxes and search fields; leave it off for email bodies
- The pin button on a card puts the note on the desktop: always on top, no taskbar entry, Markdown and images rendered. Double-click the body to edit in place, double-click the title bar to roll it up into a strip, or click × to put it away

Notes are plain files: one `.md` per note with images in a sibling `images/` folder referenced by relative paths. The default folder is `Documents\HexBench 收藏` and can be changed to a synced drive; any editor or sync tool can work with it.

### Web windows

Settings → Quick launcher → Web window:

- Add sites; the site name and icon are detected automatically
- The web window docks to the left or right edge of the screen; width and height are set separately as percentages of the work area (default 24% / 85%)
- It does not follow the panel by default; enable **Follow the panel** when you want it to move with it
- After the panel closes the web window stays for a while (90 s by default)
- Position and size are remembered; the bar uses site icons to switch and refresh

### Gestures

Hold the right mouse button in a file area and drag horizontally:

- Drag left: back; drag right: forward (can be swapped in settings)
- The gesture fires after 80 px by default
- **Show trail** draws a line while dragging and shows the direction next to the cursor

### Hotkeys

Settings → App → Hotkeys:

| Action | Default |
| --- | --- |
| Open quick launcher | `Alt+1` |
| Pause / resume edge switching | `Ctrl+Alt+Shift+P` |

Click **Set hotkey** and press a new combination; **Clear** unbinds it. A combination already taken by another program is reported.

### Tray and startup

- Left-click the tray icon to open settings; right-click for the menu (show all fences, pause, quit)
- **General → General** has run at login (tray only, no window)
- **General → Displays** can refresh the layout after plugging or unplugging a monitor
- Thumbnail cache lives in `%LOCALAPPDATA%`, capped at 200 MB with automatic cleanup; **General → Storage** can inspect and clear it

---

## Configuration

There is a single source of truth: `%APPDATA%\io.github.hexbench\config.json`. Changes made in the UI are written immediately; the file can also be edited directly (restart to apply).

| Section | Contents |
| --- | --- |
| `enabled` | Master switch |
| `trigger` | Band position, ratio, dwell, cooldown, polling, cursor push |
| `guard` | Mouse buttons, taskbar, lock screen, process blacklist |
| `hotkeys` | Launcher, pause |
| `monitors` | Per-monitor enable switches (keyed by EDID, local only) |
| `launcher` | Root folder, pinned folders, panel size, web window and sites |
| `notes` | Notes switch, folder, send-Enter, pinned notes |
| `gestures` | Switch, direction, trigger distance, trail |
| `desktop` | Fences switch, snapping, visibility, per-fence geometry and view |
| `sync` | WebDAV URL and user name |
| `updates` | Update checking and dismissed versions |
| `ui` | Theme, language, onboarding state |

Writes go through `config.json.tmp`, replace the file and keep a `config.json.bak`; a failed read falls back to the backup. The WebDAV password is kept separately in `credentials.bin`, encrypted with Windows DPAPI.

### Config sync (WebDAV)

Settings → General → Config sync:

- Enter the WebDAV URL, user name and password, use **Test connection**, then save
- **Upload config** writes this machine's settings to the server; **Pull config** replaces them with the server's copy. Both ask for confirmation
- Optional auto-pull at startup; timestamps are compared first, so a newer local config is not overwritten silently
- The password is encrypted locally and never uploaded; monitor coordinates and per-monitor switches are not synced, so another machine cannot end up misconfigured
- Works with standard WebDAV servers such as Nextcloud, ownCloud, Synology, InfiniCLOUD and nutstore

---

## Updates

- Checked 3 s after launch, then every 10 minutes, and again when the settings window opens if the last check is more than 2 minutes old
- A new version downloads in the background; the sidebar only shows **vX.Y.Z** and an **Update** button once the package is ready
- **Update** installs silently and restarts the app; the × next to it dismisses that version
- **About → Check for updates** runs a check on demand

The endpoint is fixed to `https://github.com/punwong/hexbench/releases/latest/download/latest.json`, and the installer is verified with minisign. For a mirror, fork the project and change `updates.endpoint`.

---

## Troubleshooting

**The edge does not switch anything.** Check the status on the Edge desktop page: master switch, pause, whether the monitor is disabled, and whether the foreground process is blacklisted.

**A hotkey does nothing.** Another program may own the combination; set it again under App → Hotkeys. Registration failures are reported in the UI.

**Where is the config, and how do I back it up?** `%APPDATA%\io.github.hexbench\config.json`, together with `credentials.bin`. WebDAV sync is the other option.

**Where are notes stored?** `Documents\HexBench 收藏` by default, one `.md` per note.

---

## Build

Requires Rust 1.85+, Node 22+, pnpm 10+ and the MSVC toolchain.

```powershell
pnpm install
pnpm tauri dev      # hot-reloading dev build
pnpm tauri build    # release build; set TAURI_SIGNING_PRIVATE_KEY to sign updater artifacts
```

Tests and lints:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm exec tsc --noEmit
```

Layout:

| Path | Contents |
| --- | --- |
| `src-tauri/` | Tauri entry, windows, commands, tray, updater |
| `crates/hexbench-core/` | Config, geometry, notes, system bindings |
| `crates/hexbench-fence/` | Native desktop fence windows (D3D11 + Direct2D) |
| `src/` | React front end: settings, launcher, notes, overlay |
| `docs/` | Development and testing docs, changelog |

## Release

1. Bump the version in `Cargo.toml` and `src-tauri/tauri.conf.json`
2. Update `docs/CHANGELOG.md`
3. Push a `v*` tag; CI builds, signs, generates `latest.json` and publishes the release

## License

MIT — see [LICENSE](LICENSE). The project name and icon are not part of the license; forks should rename and re-icon.
