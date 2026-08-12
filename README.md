# Vox

[![CI](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml/badge.svg)](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A local-first, menu-bar voice dictation app for macOS. Hold a global hotkey, speak, release.
Whisper transcribes on-device, a local Ollama model cleans up the text for whatever app you're
in, and the result lands at your cursor. Nothing leaves your machine on the default path.

## Features

- **Hold-to-talk global hotkey.** Hold, speak, release. Default is `Control+Alt+Space`, fully
  rebindable from the settings UI.
- **Fully local pipeline.** Speech-to-text runs on-device via Whisper (Metal-accelerated), and
  text cleanup runs against a local Ollama model. No cloud calls, no API keys, no telemetry.
- **Per-app profiles.** Choose a writing style (clean prose, casual, professional email,
  code-editor-safe, shell command, Markdown) and a privacy level for each application,
  keyed by bundle ID. Ships with sensible defaults for Slack, VS Code, Terminal, Alacritty,
  and 1Password.
- **Per-app privacy switch.** Mark any app as disabled (1Password is disabled by default) and
  Vox will never transcribe, process, or inject text while that app is focused.
- **Dictation-aware LLM cleanup.** Fixes grammar, punctuation, and filler words, expands
  spoken forms ("pr six eight four" becomes "PR #684"), and rewrites requests instead of
  fulfilling them, so saying "write two lines summarizing the outage" produces two lines of
  prose, not an assistant's reply.
- **Multilingual and code-switched speech.** Auto-detects the spoken language by default, or
  pin an input/output language explicitly. Includes purpose-built handling for Hinglish and
  other Hindi/English code-switched speech. Recognized languages: English, Hindi, Tamil,
  Spanish, Japanese, French, German, Portuguese, Chinese, Arabic, Korean, Italian, Russian.
- **Custom dictionary with automatic aliases.** Teach Vox proper nouns, product names, and
  jargon. Multi-word or hyphenated terms (for example `CI/CD`) get sensible alias variants
  automatically, so misheard spellings get normalized back to your canonical form.
- **Learns from your corrections.** Edit a past dictation in the history view, and Vox aligns
  the edit against your dictionary to learn a new spelling alias automatically.
- **Auto-paste with layered fallbacks.** Tries a trusted synthetic paste, falls back to
  `osascript`, and in the worst case always leaves the cleaned text on your clipboard so you
  never lose a dictation to a missing permission.
- **Custom system prompt override.** Replace the generated system prompt entirely with your
  own, previewed live before you save.
- **Local settings web UI** at `http://127.0.0.1:8722`: language, models, hotkey, per-app
  profiles, custom dictionary and aliases, and a dictation history view with a per-stage
  latency breakdown (privacy check, transcribe, LLM cleanup, inject) for every dictation.
- **`vox doctor`** checks the whole stack in one command: Whisper model present and valid,
  Ollama reachable with your configured model pulled, Accessibility permission granted, and
  the build toolchain present.
- **Menu bar only.** No Dock icon. The tray icon turns red while recording.
- **Locked-down local storage.** Every settings and history file is written `0600`/`0700`, so
  no dictation transcript or config is ever left world-readable on disk.

## Requirements

- macOS 13 or later (Apple Silicon tested)
- Xcode Command Line Tools, `cmake`, and a stable Rust toolchain
- [Ollama](https://ollama.com), with a model pulled (`ollama pull llama3.2`)
- A Whisper ggml model, fetched via `scripts/download_model.sh`

## Installation

Clone the repository, then run the installer from inside it:

```bash
git clone git@github.com:Siddharth189/Vox.git
cd Vox
./scripts/install.sh
```

`install.sh` builds Vox, installs the CLI to `~/.local/bin/vox`, packages `Vox.app` into
`~/Applications` (not `/Applications`), downloads the Whisper model, pulls the configured
Ollama model, and starts the tray. It's idempotent and safe to re-run after a `git pull`: it
only writes a fresh `settings.yaml` if one doesn't already exist, so your saved settings and
dictionary survive a reinstall.

To preview every step without touching your system:

```bash
VOX_INSTALL_DRY_RUN=1 ./scripts/install.sh
```

To let the installer fetch the source itself instead of cloning manually, set `VOX_REPO_URL`:

```bash
VOX_REPO_URL=git@github.com:Siddharth189/Vox.git ./scripts/install.sh
```

### Manual installation

```bash
cargo build --release --locked
./scripts/download_model.sh small
./scripts/package_app.sh
cp -R dist/Vox.app ~/Applications/
open ~/Applications/Vox.app
```

## Usage

Hold the hotkey, speak, release. The cleaned-up text is pasted at your cursor.

```
vox check                 quick Ollama reachability check
vox doctor [--json]       full readiness report
vox demo [--dry-run] "text"   run text through the real LLM cleanup pipeline
vox listen [--secs 5]     record from the mic once and clean up the result
vox tray                  launch the menu-bar app
vox inject-test "text"    exercise the auto-paste path directly
```

Open the settings UI at `http://127.0.0.1:8722` while the tray is running to configure
languages, models, the hotkey, per-app profiles, and your custom dictionary.

## How it works

Each dictation flows through six stages: audio capture, privacy check, speech-to-text,
app context detection, LLM cleanup, and injection. Every stage is a trait with a real and a
fake implementation, so the pipeline can be tested end to end without a microphone, an LLM,
or macOS itself. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full breakdown.

## Development

Vox is macOS-only in practice (menu bar, Metal Whisper, AppKit, Accessibility auto-paste), so
correctness is verified by CI on a `macos-14` GitHub Actions runner on every push. Check the
badge above or the Actions tab for current status.

```bash
cargo test
cargo build --release
cargo clippy --all-targets --all-features
./target/release/vox doctor
./target/release/vox demo --dry-run "hey john review pr six eight four"
```

Root `Cargo.toml` gates Apple-only crates (`whisper-rs`, `core-graphics`, `core-foundation`,
`objc2*`) behind `cfg(target_os = "macos")`, and a few modules (speech-to-text, permissions,
Accessibility injection) ship a non-macOS stub alongside the real implementation. This lets
most of the crate, config, prompt and dictionary rules, pipeline orchestration, and the
settings web API, type-check and unit-test on Linux too, which is useful if you're
contributing without access to a Mac. `cpal` (audio capture) still needs your platform's
native audio headers to build (for example `libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel`
on Fedora).

## Contributing

Issues and pull requests are welcome. A few things to know before opening one:

- Keep changes scoped. New behavior should land as a new trait implementation where possible,
  not a rewrite of `pipeline.rs`.
- Run `cargo test` and `cargo clippy` before submitting; CI runs both on `macos-14`.
- If you're changing prompt text, few-shot examples, or dictionary normalization rules, add or
  update a test in the same module. Those behaviors are easy to regress silently.
- For anything platform-specific, check whether the change needs a non-macOS stub to keep the
  crate buildable on Linux (see `src/stt/local_whisper_stub.rs` for the existing pattern).

## License

[MIT](LICENSE)
