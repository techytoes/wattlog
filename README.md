# Wattlog

Zero-root macOS Apple Silicon menu-bar utility for live **Power (Watts), CPU/GPU, Memory Pressure**, plain-English **Sleep & Battery diagnostics**, safe-by-default **Keep-Awake (Caffeine)**, and **grouped app monitoring**—built with Tauri v2 (Rust + React/TypeScript).

- **Zero `sudo`, zero root daemon** — drag-to-Trash uninstall
- **Menu-bar-first (`<20 MB` idle RAM)** — tray-anchored floating `NSPanel` over fullscreen Spaces, auto-destroys WebView after 60s hidden, parks `macmon` when closed
- **Same binary for GUI & CLI** — run `wattlog --plain` or `wattlog --json` with zero GUI boot

---

## Install (Apple Silicon macOS 13+)

```bash
curl -fsSL https://raw.githubusercontent.com/techytoes/wattlog/main/install.sh | sh
```

Then launch the menu-bar app or run a quick terminal check:

```bash
open -a Wattlog
wattlog --plain
```

### Uninstall

```bash
rm -rf /Applications/Wattlog.app ~/Applications/Wattlog.app \
  /opt/homebrew/bin/wattlog /usr/local/bin/wattlog ~/.local/bin/wattlog
```

---

## Features

- **Live Power, CPU/GPU & Memory Strip:** Interactive 3-metric header (`p` / `u` / `m`) wired to a load-reactive OLED waveform and hardware breakdown rail (`AppleSMC PSTR` + `AppleSmartBattery` + sudoless `macmon` CPU/GPU/ANE/DRAM split).
- **Live Sleep Blockers & Grouped Apps:** Native `IOPMCopyAssertionsByProcess` blockers pinned inline when active; Chrome/Electron/IDE helper rollup via `libproc` + `/usr/share/pmenergy/` Energy Impact + true macOS Memory Pressure (`Green`/`Yellow`/`Red`); safe `Quit` and PID-revalidated 2-click `Force Kill` with a hardcoded system-process denylist.
- **Plain-English Sleep & Battery Health:** Streams `/usr/bin/pmset -g log` into period-scoped overnight verdicts, 24h battery & silent-wake chart, 20+ decoded wake reasons (`wifibt`, `NUB.SPMI`, `EC.USBC`, `lid`, `pwrbtn`, etc.), and read-only `pmset -g custom` fixes with 1-click copy.
- **Safe-by-Default Keep-Awake:** Native `IOPMAssertionCreateWithName` with automatic kernel release on exit, `<20%` battery safety floor (ON by default), `30m`/`1h`/`2h`/`∞` timers, AC unplug auto-off, and while-app-running watch mode.

## Keyboard Shortcuts

- `1` / `2` — Switch between `Live` and `Sleep & Battery`
- `p` / `u` / `m` — Focus **Power**, **CPU & GPU**, or **Memory** (and auto-sort apps)
- `c` — Toggle Keep-Awake
- `↑` `↓` / `→` — Navigate apps and expand individual helper PIDs
- `q` — Graceful Quit selected app
- `/` — Filter apps
- `y` — Copy top `pmset` fix command (in `Sleep & Battery`)
- `Esc` — Hide popover

## Local Development

```bash
pnpm install
pnpm tauri dev

# CLI without booting GUI
cd src-tauri
cargo run -- --plain
cargo run -- --demo --plain
cargo run -- --json
```

## Cutting a Release

Tag and push a semver release to trigger `.github/workflows/release.yml`:

```bash
git tag v0.1.0
git push origin v0.1.0
```
