# Vox

[![CI](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml/badge.svg)](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A local-first, menu-bar voice dictation app for macOS and Linux. Hold a global hotkey, speak,
release. Whisper transcribes on-device, a local Ollama model cleans up the text for whatever
app you're in, and the result lands at your cursor. Nothing leaves your machine on the
default path.

Vox started as a macOS app. Linux support is newer, and its rough edges are platform-dependent
(which desktop environment and display server you run) rather than code-dependent. See
[Linux support](#linux-support) below for exactly what to expect on your setup.

## Features

- **Hold-to-talk global hotkey.** Hold, speak, release. Default is `Control+Alt+Space`, fully
  rebindable from the settings UI.
- **Fully local pipeline.** Speech-to-text runs on-device via Whisper (Metal-accelerated on
  macOS, CPU on Linux), and text cleanup runs against a local Ollama model. No cloud calls,
  no API keys, no telemetry.
- **Per-app profiles.** Choose a writing style (clean prose, casual, professional email,
  code-editor-safe, shell command, Markdown) and a privacy level for each application, keyed
  by bundle ID on macOS or WM_CLASS on Linux. Ships with sensible defaults for Slack, VS
  Code, Terminal, Alacritty, Konsole, and 1Password. On Linux, this needs a way to ask the
  desktop which window is focused; see [Linux support](#linux-support).
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
- **Auto-paste with layered fallbacks.** Tries a synthetic Cmd+V (macOS) or Ctrl+V (Linux)
  paste, falls back further on macOS to `osascript`, and in the worst case always leaves the
  cleaned text on your clipboard so you never lose a dictation to a missing permission.
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

**macOS**: macOS 13 or later (Apple Silicon tested), Xcode Command Line Tools, `cmake`, a
stable Rust toolchain.

**Linux**: a Rust toolchain, `cmake`, a C compiler, and the native dev headers for audio
(`alsa-lib-devel` / `libasound2-dev`), keystroke synthesis (`libxdo-devel` / `libxdo-dev`),
and the tray icon (`gtk3-devel` + `libayatana-appindicator-gtk3-devel`, or the `apt`
equivalents). `scripts/install-linux.sh` installs all of these for you on Fedora, Debian/
Ubuntu, or Arch.

**Both**: [Ollama](https://ollama.com), with a model pulled (`ollama pull llama3.2`), and a
Whisper ggml model, fetched via `scripts/download_model.sh`.

## Installation

### macOS

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

To preview every step without touching your system, set `VOX_INSTALL_DRY_RUN=1`. To let the
installer fetch the source itself instead of cloning manually, set `VOX_REPO_URL`.

Manual installation:

```bash
cargo build --release --locked
./scripts/download_model.sh small
./scripts/package_app.sh
cp -R dist/Vox.app ~/Applications/
open ~/Applications/Vox.app
```

### Linux

```bash
git clone git@github.com:Siddharth189/Vox.git
cd Vox
./scripts/install-linux.sh
```

`install-linux.sh` installs the native build dependencies for your package manager (dnf/apt/
pacman), builds Vox, installs the CLI to `~/.local/bin/vox`, downloads the Whisper model,
pulls the configured Ollama model, and writes a `~/.config/systemd/user/vox.service` unit
(not enabled by default - see [Linux support](#linux-support) for why). Data lives under
`~/.local/share/vox`, following the XDG Base Directory spec. `VOX_INSTALL_DRY_RUN=1` and
`VOX_REPO_URL` work the same as on macOS.

Manual installation:

```bash
cargo build --release --locked
./scripts/download_model.sh small
./target/release/vox tray
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

## Linux support

The pipeline itself, audio capture, Whisper transcription, Ollama cleanup, the settings web
UI, and history, is identical on Linux and macOS. What differs is the handful of things that
touch the desktop directly, and how well those work depends entirely on your session. This
section reflects real testing on Fedora 42 with KDE Plasma on Wayland, not assumptions.

- **Speech-to-text** runs on CPU via `whisper-rs` (no Metal-equivalent GPU backend is wired
  up) and works well: the `small` model (487MB resident) transcribed synthesized speech
  accurately in testing. On lower-RAM machines, `./scripts/download_model.sh tiny` or `base`
  trade accuracy for a smaller footprint.
- **LLM model size matters more than you'd expect on RAM-constrained machines.** This was the
  single biggest issue found in testing: on an 8GB machine, the default `llama3.2:latest`
  (3B, ~2GB) pushed the system into swap once the desktop session's own memory use was added
  in, and a single dictation's cleanup step took 4+ minutes without completing, well past
  Vox's 60-second request timeout. Switching to `llama3.2:1b` (1.3GB) brought that down to
  single-digit seconds reliably. The tradeoff is real: the 1B model occasionally drops words,
  loses the subject of a sentence, or invents small details (a hallucinated time, for
  example) that the 3B model handled correctly in the same tests. `vox doctor` now checks
  your total RAM against the configured model's size and warns if they're a bad match, with
  a fix suggestion. If you have 12GB+ RAM, the default 3B model is worth keeping for the
  better output quality; below that, start with `llama3.2:1b` and raise it only if your
  machine keeps up.
- **Per-app profile detection is confirmed working** via `kdotool` on KDE Plasma, X11 or
  Wayland alike - verified against real windows in testing, including correctly identifying
  a Konsole window by its `org.kde.konsole` WM_CLASS. This is notable because the naive
  approach (`xdotool`/`xprop` against `_NET_ACTIVE_WINDOW`) does not work on Plasma Wayland:
  it returns a KWin-internal focus-proxy window, not your actual app, silently. `kdotool`
  goes through KWin's own scripting interface instead and doesn't have this problem. On a
  plain X11 session Vox falls back to `xdotool` directly. Neither present means every
  dictation uses the `default` profile rather than failing - `vox doctor` tells you which
  path you're on.
- **Auto-paste is the rough edge.** Synthesizing Ctrl+V (via `enigo`, using XTest through
  XWayland) reports success from Vox's side but did not reliably deliver keystrokes into a
  real Wayland-native window in testing on this setup - not a Vox-specific problem: plain
  `xdotool type` and even `ydotool`'s kernel-level `uinput` injection (which is supposed to
  be indistinguishable from real hardware input) had the same result once verified against
  an actual application window, most likely because this compositor gates synthetic input
  through a portal-authorized path that neither method satisfies. What does work every time,
  confirmed in testing: **the cleaned-up text always lands on your clipboard**, the same
  safety net macOS falls back to when Accessibility isn't granted. Practically, expect to
  press Ctrl+V yourself after each dictation until proper `xdg-desktop-portal` RemoteDesktop
  support is added (tracked as a real gap, not a "should be fine" assumption) - on some other
  compositor or an X11 session, automatic paste may well work fully; `vox doctor` can't
  detect this particular failure mode itself since the synthesis call genuinely doesn't
  error, so treat "Vox: inserted at cursor" as optimistic on Wayland until you've confirmed
  it yourself once.
- **Global hotkey and tray icon** use the `global-hotkey` and `tray-icon` crates. Registration
  succeeds without error on this setup, and per-app detection working confirms the tray
  process itself runs correctly; the actual hold-to-talk key press wasn't verified end-to-end
  by automated testing (it requires physical keyboard input, same class of limitation as
  auto-paste above) - run `vox tray` from a terminal and try it once to confirm.

Run `vox doctor` after installing; it reports all of the above for your specific session,
with an install command or fix suggestion for whatever's missing.

## How it works

Each dictation flows through six stages: audio capture, privacy check, speech-to-text,
app context detection, LLM cleanup, and injection. Every stage is a trait with a real and a
fake implementation, so the pipeline can be tested end to end without a microphone, an LLM,
or macOS itself. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full breakdown.

## Development

CI builds, tests, and lints on both `macos-14` and `ubuntu-latest` on every push. Check the
badge above or the Actions tab for current status.

```bash
cargo test
cargo build --release
cargo clippy --all-targets --all-features
./target/release/vox doctor
./target/release/vox demo --dry-run "hey john review pr six eight four"
```

Root `Cargo.toml` gates Apple-only crates (`core-graphics`, `core-foundation`, `objc2*`) and
Metal-accelerated Whisper behind `cfg(target_os = "macos")`; Linux gets the same `whisper-rs`
on its default CPU backend instead. The handful of things with no cross-platform crate at all
(app-context detection, and the exact keystroke `enigo` synthesizes for paste) are the only
places with a real `#[cfg(target_os = "macos")]` / `#[cfg(not(target_os = "macos"))]` split in
application code - see `src/context/mac_detector.rs` next to `src/context/linux_detector.rs`
for the pattern to follow if you're adding another platform-specific piece.

## Contributing

Issues and pull requests are welcome. A few things to know before opening one:

- Keep changes scoped. New behavior should land as a new trait implementation where possible,
  not a rewrite of `pipeline.rs`.
- Run `cargo test` and `cargo clippy` before submitting; CI runs both on macOS and Linux.
- If you're changing prompt text, few-shot examples, or dictionary normalization rules, add or
  update a test in the same module. Those behaviors are easy to regress silently.
- If you're improving Linux desktop support (a compositor-specific hotkey backend, another
  window-manager's active-window query, ...), keep the macOS path untouched and branch with
  `#[cfg(target_os = "macos")]` the way `src/context/linux_detector.rs` does.

## License

[MIT](LICENSE)
