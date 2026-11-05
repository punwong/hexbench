# HexBench

> **Switch Windows virtual desktops by pushing the mouse to the outermost edge of your screens.**

A small, tray-resident toolbox for Windows 11. Multi-monitor aware, zero configuration, and it runs on its own — no host app required.

**[中文文档](README.md) · MIT · Tauri 2 + React**

---

## What's inside

### Tool 1 — edge-triggered virtual desktops

Push the cursor into the corner zone at the **outermost** edge of the whole desktop, rest for ~100 ms, and HexBench switches to the previous/next virtual desktop.

- Works with any monitor arrangement: ultrawide + vertical, side by side, stacked
- Only the *outermost* edges trigger — the seam between two monitors is ignored
- Hot-plug a display, change resolution, rotate — it re-reads the layout automatically
- Test mode highlights the live zone without switching

### Tool 2 — quick launcher (快捷面板)

One hotkey brings up a keyboard-first launcher over whatever you are doing: browse folders by category, pin the ones you use every day, drag files in and out, and open a built-in web panel next to it.

The launcher also holds **收藏 (favorites)**: short Markdown notes for the personal details you keep retyping — an email address, a shipping address, a support reply. Write one in the panel, drop images into it, and hit **Send**: the panel hides, focus goes back to the window you came from, and the text is typed in. Images are stripped from the typed text; use **Copy images** to put a note's images on the clipboard as files.

Every note is a plain `.md` file with its images in an `images/` folder, so the collection can be opened in any editor or synced by any cloud drive. The folder defaults to `Documents\HexBench 收藏` and can be changed in settings.

The pin button on a note card turns it into a small always-on-top window on the desktop: Markdown is rendered, images are shown, double-click the body to edit, everything auto-saves (undo/redo only), and the window remembers where it was. Double-click the title bar (or the arrow button) to roll it up into a thin title strip, like a desktop fence. Unpin it or press Alt+F4 to put it away.

| Feature | |
| --- | --- |
| Finding files | categories from a root folder, pinned folders, fuzzy search, grid/list views |
| Moving files | drag into a folder tile or onto a sidebar folder; drag out to Explorer |
| Notes | Markdown body, multiple images dragged in, edit and delete, recycle-bin safe |
| Sending | Unicode `SendInput` into the previously focused window (no clipboard use) |
| Web panel | real WebView tabs beside the launcher for docs, AI chats, dashboards |

### Shared plumbing

- **Global hotkeys** — every shortcut is registered in one place (`全局 → 热键`), so tools never fight over a combo
- **WebDAV config sync** — upload/pull `config.json`; the password is stored with Windows DPAPI and never uploaded
- **One-click updates** — the sidebar shows when a new version exists: click to download (with progress), then click *Update & restart*. Silent NSIS install, automatic restart, no installer wizard.

## Install

Grab the latest installer from [Releases](https://github.com/punwong/hexbench/releases/latest).

- Windows 10 1809+ / Windows 11, x64
- WebView2 runtime (preinstalled on Windows 11)
- Package is signed for updates (minisign); the executable itself is not code-signed yet

winget manifests are prepared under `winget/` but not merged into [winget-pkgs](https://github.com/microsoft/winget-pkgs) yet.

## Build

```powershell
pnpm install
pnpm tauri dev      # hot-reloading dev build
pnpm tauri build    # release build; add TAURI_SIGNING_PRIVATE_KEY to sign updater artifacts
```

Requires Rust 1.85+, Node 22+, pnpm 10+, and the MSVC toolchain.

Tests and lints:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Configuration

Everything lives in one file: `%APPDATA%\io.github.hexbench\config.json`

| Section | What it holds |
| --- | --- |
| `trigger` | which edges/ratio trigger a desktop switch |
| `guard` | apps that suppress triggering while in front |
| `hotkeys` | pause and launcher shortcuts |
| `launcher` | categories root, pinned folders, web panel, gestures |
| `notes` | favorites: enabled, folder, send-Enter |
| `sync` | WebDAV URL, user, DPAPI-sealed password |
| `updates` | optional endpoint override (use a mirror instead of GitHub) |

## Updates

HexBench checks `https://github.com/punwong/hexbench/releases/latest/download/latest.json`, verifies the minisign signature of the installer, then runs it in NSIS passive mode and restarts into the new version.

You can point it at your own mirror in `关于 → 更新源` if GitHub is slow on your network.

To publish a release: bump the version in `Cargo.toml` and `src-tauri/tauri.conf.json`, then push a `v*` tag — CI builds, signs, generates `latest.json` and publishes the release.

## Roadmap

- [x] Edge-triggered virtual desktop switching
- [x] Quick launcher with Markdown favorites
- [x] Global hotkey registry
- [x] WebDAV config sync
- [x] Signed one-click updates + CI releases
- [x] winget manifests (PR [microsoft/winget-pkgs#460910](https://github.com/microsoft/winget-pkgs/pull/460910) pending review)
- [ ] SignPath code signing (pending approval)
- [ ] Scoop bucket
- [ ] Conflict handling / auto-pull for WebDAV sync

## License

MIT — see [LICENSE](LICENSE). The project name and icon are not part of the license; forks should rename and re-icon.
