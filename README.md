# Vox

Local-first macOS menu-bar voice dictation. Hold a global hotkey, speak, release —
Whisper transcribes on-device, Ollama cleans the text for the focused app, then
Vox pastes at the cursor. No cloud calls on the default path.

## Status

Vox is macOS-only in practice (menu bar, Metal Whisper, AppKit, Accessibility auto-paste).
Correctness is verified by CI on a `macos-14` GitHub Actions runner on every push — see
the badge / Actions tab for current build & test status.

| Check | Where it runs |
|---|---|
| `cargo build --release`, `cargo test`, `cargo clippy` | macOS CI (`.github/workflows/ci.yml`) |
| `vox doctor` | macOS only — checks Whisper model, Ollama reachability, permissions |
| Full tray / Metal Whisper / auto-paste | **Requires macOS**, manual verification |

## Requirements

- **macOS 13+ (Apple Silicon tested)** for the full product
- Xcode Command Line Tools, `cmake`, Rust stable
- [Ollama](https://ollama.com) + `ollama pull llama3.2`
- Whisper ggml model via `scripts/download_model.sh`

Root `Cargo.toml` gates Apple-only crates (`whisper-rs`, `core-graphics`, `core-foundation`,
`objc2*`) behind `cfg(target_os = "macos")`, and several modules (STT, permissions,
Accessibility injection) ship a non-macOS stub alongside the real implementation. This lets
most of the crate's logic (config, prompt/dictionary rules, pipeline orchestration, settings
web API) type-check and unit-test on Linux too — useful for contributors without a Mac —
though `cpal` (audio capture) still needs your platform's native audio dev headers
(e.g. `alsa-lib-devel` / `libasound2-dev` on Linux) to build.

## Install (macOS)

```bash
./scripts/install.sh
```

Manual:

```bash
cargo build --release --locked
./scripts/download_model.sh small
./scripts/package_app.sh
cp -R dist/Vox.app ~/Applications/
open ~/Applications/Vox.app
```

## CLI

```
vox check
vox doctor [--json]
vox demo [--dry-run] "raw dictated text"
vox listen [--secs 5]
vox tray
vox inject-test "text"
```

Settings UI (tray running): http://127.0.0.1:8722  
Default hotkey: **Control+Alt+Space** (hold to talk).

## Verify on macOS

```bash
cargo test
cargo build --release
./target/release/vox doctor
./target/release/vox demo --dry-run "hey john review pr six eight four"
```

## Layout

See `docs/ARCHITECTURE.md`. Stages live under `src/` (audio → STT → privacy → process → inject).
