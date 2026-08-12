# Vox

[![CI](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml/badge.svg)](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml)

Local-first macOS menu-bar voice dictation. Hold a global hotkey, speak, release —
Whisper transcribes on-device, Ollama cleans the text for the focused app, then
Vox pastes at the cursor. No cloud calls on the default path.

## Features

- **Hold-to-talk global hotkey** — hold, speak, release; text lands at your cursor a moment
  later. Default `Control+Alt+Space`, fully rebindable from the settings UI.
- **100% local pipeline** — speech-to-text (Whisper via `whisper-rs`, Metal-accelerated) and
  text cleanup (a local Ollama model) both run on-device. Nothing is ever sent to the cloud
  on the default path.
- **Per-app behavior profiles** — set a format (clean prose, casual, professional email,
  code-editor-safe, shell command, Markdown, …) and privacy level per application, keyed by
  bundle ID. Ships with sane defaults for Slack, VS Code, Terminal, Alacritty, and 1Password.
- **Per-app privacy switch** — mark any app "disabled" (1Password by default) and Vox will
  never transcribe, process, or inject text while that app is focused.
- **LLM cleanup tuned for dictation, not chat** — fixes grammar/punctuation/filler words,
  expands spoken forms ("pr six eight four" → "PR #684"), and rewrites *requests* instead of
  fulfilling them, so saying "write two lines summarizing the outage" produces two lines of
  prose, not an assistant's reply.
- **Multilingual & code-switched speech** — auto-detects the spoken language by default, or
  pin an input/output language explicitly. Purpose-built handling for Hinglish and other
  Hindi/English code-switched speech, including a few-shot-tuned translation mode.
  Recognized language codes: English, Hindi, Tamil, Spanish, Japanese, French, German,
  Portuguese, Chinese, Arabic, Korean, Italian, Russian.
- **Custom dictionary with auto-generated aliases** — teach Vox proper nouns, product names,
  and jargon. Multi-word/hyphenated/slash terms (e.g. `CI/CD`) automatically get sensible
  alias variants so the LLM's spelling gets normalized back to your canonical form.
- **Learns from your corrections** — edit a past dictation in the history view and Vox
  aligns the change against your dictionary to learn a new spelling alias automatically —
  no manual dictionary editing required for the mistakes you actually make.
- **Auto-paste with layered fallbacks** — tries a trusted synthetic paste, falls back to
  `osascript`, and worst case always leaves the cleaned text on your clipboard so you never
  lose a dictation to a missing permission.
- **Custom system prompt override** — for full control, replace the generated system prompt
  entirely with your own, previewed live before you save.
- **Local settings web UI** (`http://127.0.0.1:8722`) — language, models, hotkey, per-app
  profiles, custom dictionary/aliases, and a dictation history/diagnostics view with
  per-stage latency (privacy check → transcribe → LLM cleanup → inject) for every dictation.
- **`vox doctor`** — one command to check the whole stack: Whisper model present and valid,
  Ollama reachable with your configured model pulled, Accessibility permission granted,
  build toolchain present.
- **Menu-bar only, no Dock icon** — a lightweight `LSUIElement` app; a template tray icon
  goes red while recording.
- **Every settings/history file is written `0600`/`0700`** — no dictation transcript or
  config is ever left world-readable on disk.

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

You need a local checkout first — `install.sh` builds from whatever source tree it's run
from (it resolves its own repo root from its own path), it does not fetch anything on its
own unless you explicitly ask it to. So: clone once, then run the script from inside it.

```bash
git clone git@github.com:Siddharth189/Vox.git
cd Vox
./scripts/install.sh
```

`install.sh` is idempotent and safe to re-run (e.g. to update after a `git pull`) — it
checks for required tools before installing them, and only writes a fresh `settings.yaml`
if one doesn't already exist, so your saved settings/dictionary survive a reinstall. It
installs the CLI to `~/.local/bin/vox`, packages `Vox.app` into `~/Applications/` (not
`/Applications`), downloads the Whisper model, pulls the configured Ollama model, and starts
the tray. Preview every step without touching your system with:

```bash
VOX_INSTALL_DRY_RUN=1 ./scripts/install.sh
```

If you'd rather have the installer fetch the source itself (no local clone), set
`VOX_REPO_URL` and it will clone into `~/.cache/vox-src` for you:

```bash
VOX_REPO_URL=git@github.com:Siddharth189/Vox.git ./scripts/install.sh
```

Manual install (no script):

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
