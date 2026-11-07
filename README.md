# Vox

[![CI](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml/badge.svg)](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A local-first tray dictation app for **macOS, Windows, and Linux**. Press `Ctrl+.`, speak,
press it again (or just hold it while you talk). Whisper transcribes on-device, the text is
cleaned up for whatever app you're in, and the result lands at your cursor. Speaking runs at
~150-200 wpm; typing at 40-90. Nothing leaves your machine.

The install is small: two ~6 MB executables plus a 57 MB speech model, downloaded on first
run. A local LLM (Ollama) is an optional upgrade, not a requirement.

Vox started as a macOS app. Linux support's rough edges are platform-dependent (which desktop
environment and display server you run) rather than code-dependent. See
[Linux support](#linux-support) and [Windows notes](#windows-notes) below.

## Features

- **One hotkey, two ways to use it.** Default `Ctrl+.` on every OS. *Tap* it to start
  listening hands-free and tap again to finish, or *hold* it while you talk and release to
  finish. `Esc` cancels. Strict hold-to-talk and press-to-toggle modes are also available.
  Rebind it by pressing a new shortcut in the settings UI.
- **Fully local pipeline.** Speech-to-text runs on-device via Whisper (Metal-accelerated on
  macOS, CPU on Windows and Linux). No cloud calls, no API keys, no telemetry.
- **Long dictations finish as fast as short ones.** Vox transcribes while you talk, cutting
  the audio at natural pauses and transcribing each piece in the background. When you stop,
  only the last few seconds are left, so a 40-second paragraph doesn't make you wait any
  longer than a quick reply. Turn it off in settings ("Transcribe while you speak") to
  transcribe in one pass after you stop.
- **Instant cleanup, LLM optional.** Built-in cleanup (filler words, spacing, capitalization,
  terminal/code-aware formatting, your dictionary) adds no latency and needs nothing extra.
  When Ollama is running with the configured model, Vox uses it for deeper rewriting and
  translation, and falls back to the built-in cleanup the moment Ollama isn't there.
- **Knows when it's listening.** A short chirp on start and stop (toggle in settings), and the
  tray icon turns red while listening and amber while transcribing.
- **First run just works.** The tray downloads the speech model with progress in its menu.
  Launch at login is one click in the tray menu.
- **Per-app profiles.** Choose a writing style (clean prose, casual, professional email,
  code-editor-safe, shell command, Markdown) and a privacy level for each application, keyed
  by bundle ID on macOS, executable name on Windows (`Code`, `slack`, `WindowsTerminal`), or
  WM_CLASS on Linux, matched case-insensitively. Ships with defaults for Slack, VS Code,
  terminals (Terminal, Windows Terminal, PowerShell, cmd, Alacritty, Konsole), and password
  managers. On Linux, this needs a way to ask the desktop which window is focused; see
  [Linux support](#linux-support).
- **Per-app privacy switch.** Mark any app as disabled (1Password, Bitwarden, KeePass, and
  KeePassXC are disabled by default) and Vox won't even open the microphone while that app
  is focused.
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
- **Auto-paste that respects your clipboard.** Synthesizes Cmd+V (macOS) or Ctrl+V
  (Windows, Linux) once the hotkey's modifier keys are released, then puts your previous
  clipboard (text or image) back. On Windows, dictations are kept out of Win+V clipboard
  history. macOS falls back to `osascript`, and in the worst case the cleaned text stays on
  your clipboard so you never lose a dictation to a missing permission.
- **Custom system prompt override.** Replace the generated system prompt entirely with your
  own, previewed live before you save.
- **Local settings web UI** at `http://127.0.0.1:8722`: language, models, hotkey, per-app
  profiles, custom dictionary and aliases, and a dictation history view with a per-stage
  latency breakdown (privacy check, transcribe, LLM cleanup, inject) for every dictation.
- **`vox doctor`** checks the whole stack in one command: microphone, hotkey, Whisper model
  present and valid, Ollama (optional), and Accessibility permission on macOS.
- **Tray only.** No Dock icon or taskbar window.
- **Locked-down local storage.** Settings and history are written atomically, `0600`/`0700` on
  macOS/Linux and under the per-user `%LOCALAPPDATA%` on Windows. The settings server only
  answers its own page: Host and Origin are checked, which blocks DNS-rebinding reads of your
  history.

## Requirements

**Windows**: Windows 10 or 11 (x64). Nothing else to run a release. To build from source:
[Rust](https://rustup.rs), Visual Studio 2022 Build Tools (C++ workload), CMake, and LLVM
(`winget install Kitware.CMake LLVM.LLVM`).

**macOS**: macOS 13 or later (Apple Silicon tested), Xcode Command Line Tools, `cmake`, a
stable Rust toolchain.

**Linux**: a Rust toolchain, `cmake`, a C compiler, and the native dev headers for audio
(`alsa-lib-devel` / `libasound2-dev`), keystroke synthesis (`libxdo-devel` / `libxdo-dev`),
and the tray icon (`gtk3-devel` + `libayatana-appindicator-gtk3-devel`, or the `apt`
equivalents). `scripts/install-linux.sh` installs all of these for you on Fedora, Debian/
Ubuntu, or Arch.

**Optional, any OS**: [Ollama](https://ollama.com) with a model pulled (`ollama pull
llama3.2`) for LLM rewriting and translation. The Whisper model is downloaded by Vox itself
(`vox download-model`, or automatically by the tray on first run).

## Installation

### Windows

From a checkout (builds, then installs for the current user, no admin needed):

```powershell
git clone https://github.com/Siddharth189/Vox.git
cd Vox
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
```

From a release zip (`Vox-windows-x86_64.zip`, built by CI): unzip, then run
`install-windows.ps1 -BinDir .` from that folder.

The installer copies `vox.exe` (CLI) and `vox-tray.exe` (the windowless tray) to
`%LOCALAPPDATA%\Programs\Vox`, adds it to your PATH, creates a **Vox** Start-menu shortcut,
downloads the speech model, turns on launch at login, and starts the tray. Re-run it to
upgrade; settings and history are kept. `-Model small-q5_1`, `-NoAutostart`, and `-DryRun`
are available. `scripts\uninstall-windows.ps1 [-RemoveData]` removes it.

### macOS

```bash
git clone git@github.com:Siddharth189/Vox.git
cd Vox
./scripts/install.sh
```

`install.sh` builds Vox, installs the CLI to `~/.local/bin/vox`, packages `Vox.app` into
`~/Applications` (not `/Applications`), downloads the Whisper model, and starts the tray. Set
`VOX_WITH_LLM=1` to also install Ollama and pull the configured model, and
`VOX_WHISPER_MODEL=small-q5_1` for a larger speech model. It's idempotent and safe to re-run
after a `git pull`: it only writes a fresh `settings.yaml` if one doesn't already exist, so
your saved settings and dictionary survive a reinstall.

To preview every step without touching your system, set `VOX_INSTALL_DRY_RUN=1`. To let the
installer fetch the source itself instead of cloning manually, set `VOX_REPO_URL`.

Manual installation:

```bash
cargo build --release --locked
./target/release/vox download-model
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
and writes a `~/.config/systemd/user/vox.service` unit (not enabled by default - see
[Linux support](#linux-support) for why; the tray's "Launch at login" uses XDG autostart
instead). `VOX_WITH_LLM=1` adds Ollama. Data lives under `~/.local/share/vox`, following the
XDG Base Directory spec. `VOX_INSTALL_DRY_RUN=1` and `VOX_REPO_URL` work the same as on macOS.

Manual installation:

```bash
cargo build --release --locked
./target/release/vox download-model
./target/release/vox tray
```

## Usage

Click into any text field, press `Ctrl+.`, speak, and press `Ctrl+.` again (or hold it while
speaking and let go). The cleaned-up text is pasted at your cursor. `Esc` cancels.

```
vox tray                  run the tray app (Windows: vox-tray.exe / Start menu "Vox")
vox setup                 download the speech model and run the readiness check
vox doctor [--json]       readiness report: mic, hotkey, model, Ollama, permissions
vox download-model [NAME] fetch a Whisper model (--list for choices) and select it
vox transcribe FILE.wav   transcribe a recording (benchmarking / support)
vox listen [--secs 5]     record from the mic once and clean up the result
vox demo [--dry-run] "text"   run text through the cleanup pipeline
vox inject-test "text"    exercise the auto-paste path directly
vox autostart on|off      launch the tray at login
vox check                 quick Ollama reachability check
```

Open the settings UI from the tray menu (or `http://127.0.0.1:8722`) to configure languages,
models, the hotkey, per-app profiles, and your custom dictionary.

### Speech models

| Model | Size | Notes |
|---|---|---|
| `base-q5_1` (default) | 57 MB | multilingual, fast on any CPU |
| `base.en-q5_1` | 57 MB | English only, slightly more accurate for English |
| `small-q5_1` | 181 MB | multilingual, more accurate, ~3x slower |
| `large-v3-turbo-q5_0` | 547 MB | most accurate; best with a GPU (macOS) |

`vox download-model small-q5_1` downloads and switches; the tray picks it up without a
restart.

## Windows notes

Tested on Windows 11 (i7-1185G7, 4 cores, no GPU) with the default `base-q5_1` model,
dictating into a real text box with the real hotkey and microphone:

| What | Result |
|---|---|
| Stop -> text at cursor, 3-5 s utterance | 0.7-1.1 s |
| Stop -> text at cursor, 18 s paragraph | ~2.9 s |
| Accuracy on those clips | word-perfect |
| Idle CPU / memory | ~0.02% / ~260 MB |

- **Per-app profiles** key off the executable name (`Code`, `slack`, `WindowsTerminal`,
  `powershell`). Terminals get the shell profile: no forced capital, no trailing period.
- **Apps running as administrator** can't receive synthesized keystrokes from a normal
  process (Windows UIPI). The text is still on the clipboard; press Ctrl+V. Run Vox elevated
  only if you dictate into elevated apps a lot.
- **`Ctrl+.` is global** while Vox runs, so it takes over VS Code's "Quick Fix" binding.
  Pick another shortcut in Settings if you rely on that.
- Dictations are excluded from Win+V clipboard history and cloud clipboard, and your
  previous clipboard is restored after each paste.
- Logs: `%LOCALAPPDATA%\Vox\tray.log`.

## Linux support

The pipeline itself, audio capture, Whisper transcription, Ollama cleanup, the settings web
UI, and history, is identical on Linux, Windows, and macOS. What differs is the handful of things that
touch the desktop directly, and how well those work depends entirely on your session. This
section reflects real testing on Fedora 42 with KDE Plasma on Wayland, not assumptions.

- **Speech-to-text** runs on CPU via `whisper-rs` (no Metal-equivalent GPU backend is wired
  up) and works well: the `small` model (487MB resident) transcribed synthesized speech
  accurately in testing. On lower-RAM machines, `./scripts/download_model.sh tiny` or `base`
  trade accuracy for a smaller footprint.
- **The event loop had a real bug that made everything look broken.** `tao`'s
  `ControlFlow::WaitUntil` does not reliably re-arm on at least one real setup (KDE Plasma
  Wayland): it fired a handful of times at startup and then silently stopped, which meant
  the hotkey grab succeeded and events genuinely arrived in the channel, but nothing was
  left polling for them - holding the hotkey did nothing, and the "Settings..." menu item
  (which separately used the macOS-only `open` command with no Linux fallback) did nothing
  either. Fixed by parking the loop fully idle (`ControlFlow::Wait`, ~0% CPU) and waking it
  on a fixed cadence from a dedicated thread via `EventLoopProxy::send_event` - the same
  mechanism worker threads already use to deliver pipeline results back to the UI, so it
  doesn't depend on the platform re-arming a timer correctly. An earlier `Poll`-based
  workaround "fixed" the symptom but cost 30-50% constant CPU, which starved the actual
  transcription and LLM steps of CPU and made a real dictation take 113 seconds end to end.
- **Model size matters more than you'd expect on RAM-constrained machines, for both models.**
  This was the single biggest performance issue found in testing: on an 8GB machine, the
  default `llama3.2:latest` (3B, ~2GB) pushed the system into swap once the desktop
  session's own memory use was added in, and a single dictation's cleanup step took 4+
  minutes without completing, well past Vox's 60-second request timeout. Switching to
  `llama3.2:1b` (1.3GB) brought that down to single-digit seconds. Whisper's `small` model
  (487MB) was the other half of the same problem - about 50 seconds to transcribe a 7-second
  clip, competing with the LLM and the tray itself for the same limited cores; `base`
  (141MB) cut that to a few seconds with no noticeable accuracy loss on clear speech. The
  1B LLM tradeoff is real: it occasionally drops the subject of a sentence, invents small
  details, or (fixed in testing) prefaces its answer with unstripped meta-commentary like
  "I'll provide the corrected text:" that the 3B model didn't produce in the same tests.
  `vox doctor` checks your total RAM against the configured LLM's size and warns if they're
  a bad match. If you have 12GB+ RAM, the default 3B model plus the `small` Whisper model
  are worth keeping for the better quality; below that, `install-linux.sh` picks the faster
  pairing (`llama3.2:1b` + `base`) automatically.
- **Per-app profile detection is confirmed working** via `kdotool` on KDE Plasma, X11 or
  Wayland alike - verified against real windows in testing, including correctly identifying
  a Konsole window by its `org.kde.konsole` WM_CLASS. This is notable because the naive
  approach (`xdotool`/`xprop` against `_NET_ACTIVE_WINDOW`) does not work on Plasma Wayland:
  it returns a KWin-internal focus-proxy window, not your actual app, silently. `kdotool`
  goes through KWin's own scripting interface instead and doesn't have this problem. On a
  plain X11 session Vox falls back to `xdotool` directly. Neither present means every
  dictation uses the `default` profile rather than failing - `vox doctor` tells you which
  path you're on.
- **Auto-paste is the rough edge, and here's exactly why.** Plain XTest-based synthesis
  (`enigo`'s default backend, used through XWayland) reports success from Vox's side but did
  not reliably deliver keystrokes into a real Wayland-native window in testing - not
  Vox-specific: raw `xdotool type` and even `ydotool`'s kernel-level `uinput` injection
  (indistinguishable from real hardware input at the driver level) had the same result once
  verified against an actual window. So Vox now also tries `enigo`'s `libei_tokio` backend,
  which goes through the correct, sanctioned mechanism for this - an authorized
  `xdg-desktop-portal` RemoteDesktop session, the same one-time-consent model macOS's
  Accessibility permission uses. On this test machine that path hits what looks like a KDE
  portal bug (`xdp-kde-remotedesktop` fails its own internal permission-store lookup before
  ever showing the consent dialog, confirmed via `journalctl`), so it doesn't fully close the
  gap here - but the fix is real, is the officially-supported mechanism, and may well work on
  other Plasma versions or a fresh permission store. Either way, this path is now
  timeout-bounded (3s) specifically because a broken or absent portal must never turn into an
  indefinite hang for the whole pipeline - it always falls through to the clipboard. What
  works every time, confirmed in testing: **the cleaned-up text always lands on your
  clipboard**, the same safety net macOS falls back to when Accessibility isn't granted.
  `vox doctor` can't detect this specific failure mode since the synthesis call doesn't error
  from Vox's point of view, so treat "Vox: inserted at cursor" as optimistic on Wayland until
  you've confirmed it yourself once.
- **Global hotkey and tray icon** use the `global-hotkey` and `tray-icon` crates. Registration
  succeeds without error on this setup, and per-app detection working confirms the tray
  process itself runs correctly; the actual hold-to-talk key press wasn't verified end-to-end
  by automated testing (it requires physical keyboard input, same class of limitation as
  auto-paste above) - run `vox tray` from a terminal and try it once to confirm.
  `global-hotkey` grabs keys through X11, so on a **native Wayland session the hotkey only
  fires while an XWayland window is focused**. The proper fix is the
  `org.freedesktop.portal.GlobalShortcuts` portal (KDE 6, GNOME 48+), which isn't wired up
  yet. Until then, bind a desktop shortcut to `vox` or use an X11 session.

Run `vox doctor` after installing; it reports all of the above for your specific session,
with an install command or fix suggestion for whatever's missing.

## How it works

Each dictation flows through six stages: audio capture, privacy check, speech-to-text,
app context detection, LLM cleanup, and injection. Every stage is a trait with a real and a
fake implementation, so the pipeline can be tested end to end without a microphone, an LLM,
or macOS itself. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full breakdown.

## Development

CI builds, tests, and lints on `macos-14`, `ubuntu-latest`, and `windows-latest` on every
push; the Windows job also uploads an installable `Vox-windows-x86_64.zip`. Check the badge
above or the Actions tab for current status.

```bash
cargo test
cargo build --release
cargo clippy --all-targets --all-features
./target/release/vox doctor
./target/release/vox demo --dry-run "hey john review pr six eight four"
./target/release/vox transcribe some-recording.wav --repeat 3   # STT latency/accuracy
./target/release/vox transcribe some-recording.wav --stream     # wait after stop, streamed
```

**Windows**: build with the MSVC toolchain (Visual Studio 2022 Build Tools with the C++
workload, plus CMake and LLVM for whisper-rs's bindgen). The `x86_64-pc-windows-gnu`
target doesn't work out of the box: whisper-rs-sys 0.13 passes MSVC-only flags and ggml
names its archives without the `lib` prefix.

**Release builds you distribute** must not be tuned to the build machine: whisper.cpp
defaults to `-march=native`, which crashes with "illegal instruction" on CPUs without the
build machine's extensions (e.g. AVX-512). Set
`CMAKE_PROJECT_INCLUDE_BEFORE=<repo>/cmake/portable-cpu.cmake` to build for the portable
AVX2 baseline instead (CI does this). Builds for your own machine are fastest without it.

Useful environment variables: `VOX_DATA_DIR` (data dir override), `VOX_WHISPER_THREADS`,
`VOX_WHISPER_TIMINGS=1` (whisper system info and timings to stderr), `VOX_FULL_AUDIO_CTX=1`
(disable the short-clip encoder speedup), `RUST_LOG=vox=debug`.

Root `Cargo.toml` gates Apple-only crates (`core-graphics`, `core-foundation`, `objc2*`) and
Metal-accelerated Whisper behind `cfg(target_os = "macos")`, and `windows-sys` behind
`cfg(windows)`; Linux and Windows get `whisper-rs` on its CPU backend. The handful of things
with no cross-platform crate (app-context detection, modifier-key state, the exact paste
keystroke, launch at login) are the only places with a per-OS `cfg` split in application
code - see `src/context/` (`mac_detector.rs`, `windows_detector.rs`, `linux_detector.rs`)
for the pattern to follow if you're adding another platform-specific piece.

## Contributing

Issues and pull requests are welcome. A few things to know before opening one:

- Keep changes scoped. New behavior should land as a new trait implementation where possible,
  not a rewrite of `pipeline.rs`.
- Run `cargo test` and `cargo clippy` before submitting; CI runs both on macOS, Linux, and
  Windows.
- If you're changing prompt text, few-shot examples, or dictionary normalization rules, add or
  update a test in the same module. Those behaviors are easy to regress silently.
- If you're improving Linux desktop support (a compositor-specific hotkey backend, another
  window-manager's active-window query, ...), keep the macOS path untouched and branch with
  `#[cfg(target_os = "macos")]` the way `src/context/linux_detector.rs` does.

## License

[MIT](LICENSE)
