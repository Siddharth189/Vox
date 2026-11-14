<div align="center">

# Vox

**Private voice dictation for macOS, Windows, and Linux. Press a key, speak, and polished text lands at your cursor. Nothing leaves your machine.**

[![CI](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml/badge.svg)](https://github.com/Siddharth189/Vox/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/Siddharth189/Vox)](https://github.com/Siddharth189/Vox/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational)
![Rust](https://img.shields.io/badge/rust-1.89%2B-orange)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

[Install](#install) · [Features](#features) · [Usage](#usage) · [Local LLMs](#optional-polish-with-a-local-llm) · [FAQ](#faq) · [Contributing](#contributing)

</div>

---

Vox is an open-source, offline alternative to cloud dictation apps. It lives in your tray. Press
`Ctrl+.` in any text field (email, Slack, your editor, a terminal), talk, and press it again.
[Whisper](https://github.com/ggerganov/whisper.cpp) transcribes on your device, Vox cleans the
text up for the app you're in, and pastes it at your cursor.

You speak at 150 wpm; most people type at 40. On a 4-core laptop with no GPU, a short sentence
reaches the cursor **under a second** after you stop talking, and a long paragraph isn't much
slower: Vox transcribes while you speak.

- **Private by design.** On-device speech recognition. No account, no API key, no telemetry.
- **Small.** Two ~6 MB executables and a 57 MB speech model, downloaded on first run.
- **Works without an LLM.** Built-in cleanup is instant. A local LLM is an optional upgrade.
- **Cross-platform.** One Rust codebase for macOS (Metal-accelerated), Windows, and Linux.

## Why Vox

| | Vox | Typical cloud dictation |
|---|---|---|
| Where your voice goes | Stays on your computer | Uploaded to a server |
| Works offline | Yes | No |
| Price | Free, MIT-licensed | Subscription |
| Account required | No | Yes |
| Platforms | macOS, Windows, Linux | Often macOS/Windows only |
| Bring your own LLM | Ollama, LM Studio, llama.cpp, any OpenAI-compatible local server | Vendor's model |
| Inspect and change it | All source here | Closed |

## Install

### Download

Grab the file for your system from the
**[latest release](https://github.com/Siddharth189/Vox/releases/latest)**. No Rust or build tools
needed. Vox downloads its speech model the first time it starts.

| System | File | Then |
|---|---|---|
| Windows 10/11 | `Vox-<version>-windows-x86_64.zip` | Unzip, then run `powershell -ExecutionPolicy Bypass -File install-windows.ps1 -BinDir .` in that folder |
| macOS 13+, Apple Silicon | `Vox-<version>-macos-arm64.zip` | Unzip, move `Vox.app` to Applications, and open it (see below) |
| Linux x86_64 | `Vox-<version>-linux-x86_64.tar.gz` | Extract, copy `vox` to `~/.local/bin`, and run `vox tray` |

- **macOS:** the app isn't notarized yet, so the first launch is blocked. Open it once, then
  choose **Open Anyway** in System Settings → Privacy & Security, or run
  `xattr -dr com.apple.quarantine /Applications/Vox.app`. Intel Macs: install from source below.
- **Linux:** built on Ubuntu 24.04. It needs GTK 3, libayatana-appindicator, libxdo, and ALSA at
  runtime, which most desktops already have. On older distros, or if it doesn't start, use
  `install-linux.sh` below.
- Each release lists SHA-256 checksums in `SHA256SUMS.txt`.

Or install from source with the script for your system:

### Windows 10/11

```powershell
git clone https://github.com/Siddharth189/Vox.git
cd Vox
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
```

This builds Vox, installs it for your user (no admin needed), adds a **Vox** Start-menu entry,
downloads the speech model, turns on launch at login, and starts the tray.

### macOS 13+

```bash
git clone https://github.com/Siddharth189/Vox.git
cd Vox
./scripts/install.sh
```

Installs `Vox.app` to `~/Applications` and the `vox` CLI to `~/.local/bin`, downloads the model,
and starts the tray. Grant **Accessibility** access when asked: that's what lets Vox paste for you.

### Linux (Fedora, Debian/Ubuntu, Arch)

```bash
git clone https://github.com/Siddharth189/Vox.git
cd Vox
./scripts/install-linux.sh
```

Installs the build dependencies for your distro, builds Vox, and downloads the model. Desktop
support varies by compositor; read [Linux notes](docs/PLATFORMS.md#linux) before filing a bug.

### After installing

```bash
vox doctor
```

`vox doctor` checks the microphone, hotkey, speech model, transcription speed, optional LLM
server, and permissions, and prints the exact fix for anything that's missing.

<details>
<summary>Installer options, building from source, and uninstalling</summary>

| | Windows (`install-windows.ps1`) | macOS / Linux (`install.sh`, `install-linux.sh`) |
|---|---|---|
| Bigger speech model | `-Model small-q5_1` | `VOX_WHISPER_MODEL=small-q5_1` |
| Also install Ollama + model | - | `VOX_WITH_LLM=1` |
| Preview without changing anything | `-DryRun` | `VOX_INSTALL_DRY_RUN=1` |
| Skip launch at login | `-NoAutostart` | (Linux: off by default) |
| Uninstall | `scripts\uninstall-windows.ps1 [-RemoveData]` | delete the app/binary and the data dir below |

Re-running an installer upgrades in place and keeps your settings, dictionary, and history.

**Build from source.** You need [Rust](https://rustup.rs) 1.89+, CMake, and a C/C++ compiler:

- **Windows:** Visual Studio 2022 Build Tools (C++ workload), plus `winget install Kitware.CMake LLVM.LLVM`.
- **macOS:** Xcode Command Line Tools and `brew install cmake`.
- **Linux:** `cmake`, a C compiler, and the dev headers for ALSA, libxdo, GTK 3, and
  libayatana-appindicator (`install-linux.sh` installs them).

```bash
cargo build --release --locked
./target/release/vox download-model
./target/release/vox tray
```

</details>

## Features

### Dictation that fits where you're typing

- **One hotkey, two styles.** *Tap* `Ctrl+.` to start hands-free and tap again to finish, or
  *hold* it while you talk. `Esc` cancels. Rebind it in settings by pressing the new shortcut.
- **Per-app profiles.** Clean prose by default, casual in Slack, code-safe in your editor, no
  capital letter or trailing period in terminals. Vox recognizes the focused app (bundle ID on
  macOS, executable name on Windows, WM_CLASS on Linux) and ships sensible defaults.
- **Voice commands.** Say *"new line"*, *"new paragraph"*, or *"scratch that"* as its own
  phrase. Vox doesn't act on them when they're part of a sentence ("add a new line to the
  file"), and never inserts line breaks into a terminal.
- **Snippets.** Say *"my calendar link"* or *"my signature"* and Vox types the stored text exactly
  as written, line breaks and all.
- **Edit by voice.** Select text, press `Ctrl+Shift+.`, and say how to change it: *"make it
  shorter"*, *"more polite"*, *"isko English mein kar do"*. Your local LLM rewrites the selection
  in place. With nothing selected, say what to write.
- **See your words as you speak.** A small bubble above the taskbar shows a level meter and the
  text as it comes, so you know Vox is hearing you (Windows; the tray title on macOS and Linux).
- **Long dictations finish fast.** Vox transcribes while you talk, cutting the audio at natural
  pauses. When you stop, only the last few seconds are left, so a 40-second paragraph doesn't
  make you wait much longer than a quick reply. Turn it off with **Transcribe while you speak**
  in settings.
- **Multilingual.** Auto-detects the language, or pin input and output languages. Includes
  handling for Hinglish and other code-switched speech, and translation when a local LLM is on.

### Accurate on your words

- **Custom dictionary.** Teach Vox product names, people, and jargon. Misheard spellings map
  back to your canonical form, and multi-word terms get alias variants automatically.
- **Learns from corrections.** Fix a past dictation in the history view. Vox remembers what
  it misheard ("cube cuddle" becomes "kubectl") and suggests the new names you typed for your
  dictionary, which Whisper is then primed with, so it hears them right next time.
- **Smart cleanup without an LLM.** Removes "um" and "uh" without touching real words, fixes
  spacing and capitalization, and respects the app's format. Adds no noticeable delay.
- **Optional LLM polish.** With a local model running, Vox fixes grammar, expands spoken forms
  ("pr six eight four" becomes "PR #684"), and rewrites what you said rather than answering it.

### Respectful of your machine

- **Your clipboard survives.** Vox pastes, then restores what you had copied (text or image). On
  Windows, dictations stay out of Win+V history and cloud clipboard.
- **Privacy switch per app.** Vox won't even open the microphone while a password manager
  (1Password, Bitwarden, KeePass, KeePassXC) is focused. Add any app you like.
- **Never loses a dictation.** If pasting is blocked, the text stays on your clipboard.
- **Locked-down storage.** Settings and history are private to your user. The settings page only
  answers itself (Host and Origin checks block DNS-rebinding attacks).

### See what it's doing

- **Usage stats.** Words dictated, typing time saved, speaking pace, and your daily streak, in
  the settings page and via `vox stats`. Only counts are stored, never text.
- **History with timings.** Each recent dictation shows the raw transcript, LLM output, final
  text, and how long each stage took.
- **Clear status.** The tray icon turns red while listening and amber while transcribing, with
  optional start and stop sounds.

## Usage

Click into any text field, press **`Ctrl+.`**, speak, and press **`Ctrl+.`** again (or hold it
while you speak and let go). Press `Esc` to cancel.

Open **Settings** from the tray menu, or go to <http://127.0.0.1:8722>.

### Edit by voice

Select some text, press **`Ctrl+Shift+.`**, say what to do with it, and press it again (or hold
it while you speak). The selection is replaced with the result. `Ctrl+Z` in the app undoes it.

| Selected | Say | You get |
|---|---|---|
| "send me the report now" | "make it more polite" | "Could you please send me the report when you get a chance?" |
| "kal ki meeting 5 baje hai" | "isko English mein kar do" | "Tomorrow's meeting is at 5." |
| "He don't know where the files is." | "fix the grammar" | "He doesn't know where the files are." |
| *(nothing)* | "write a short thank-you note to Priya" | a note, typed at the cursor |

It uses your [local LLM](#optional-polish-with-a-local-llm), so it needs LLM cleanup and
auto-paste on, and a model running. It's off in terminals, where Ctrl+C would stop the program.
In code editors that copy the whole line when nothing is selected (VS Code, JetBrains), select
what you want changed first.

### Voice commands

| Say | You get |
|---|---|
| "…thanks. **New paragraph.** See you…" | a blank line between paragraphs |
| "…item one. **New line.** Item two…" | a line break |
| "Let's meet Tuesday. **Scratch that.** Let's meet Wednesday." | "Let's meet Wednesday." |
| a snippet trigger, e.g. "**my email**" | the snippet's text, exactly as saved |

Pause briefly before a command so Whisper punctuates it as its own phrase. Turn voice commands
off under **Settings > Behavior**. Add snippets under **Settings > Snippets** or in
`settings.yaml`.

### Command line

```text
vox tray                      run the tray app (Windows: vox-tray.exe or Start menu "Vox")
vox setup                     download the speech model and run the readiness check
vox doctor [--json]           readiness report: mic, hotkey, model, LLM server, permissions
vox stats [--json|--reset]    words dictated, time saved, speaking pace, streak
vox download-model [NAME]     fetch a Whisper model (--list for choices) and select it
vox devices                   list microphones (* = selected)
vox listen [--secs 5]         record once from the mic and print the cleaned result
vox transcribe FILE.wav       transcribe a recording (benchmarks, bug reports)
                [--stream]    ...played in real time through streaming; prints the wait after stop
                [--live]      ...and the live text as it changes
vox demo [--dry-run] "text"   run text through cleanup, voice commands, and snippets
vox inject-test "text"        test auto-paste on its own
vox autostart on|off|status   launch the tray at login
vox check                     send one test request to the configured LLM server
```

### Speech models

| Model | Size | Best for |
|---|---|---|
| `base-q5_1` (default) | 57 MB | any CPU; multilingual; fast |
| `base.en-q5_1` | 57 MB | English only, slightly more accurate |
| `small-q5_1` | 181 MB | better accuracy, about 3x slower |
| `large-v3-turbo-q5_0` | 547 MB | best accuracy; use a GPU (macOS) |

`vox download-model small-q5_1` downloads a model and switches to it. The tray picks it up
without a restart.

## Optional: polish with a local LLM

Vox works fully without an LLM. When one is running on your machine, Vox uses it for deeper
rewriting and translation, and falls back to built-in cleanup the moment it's unavailable. No
dictation waits on a server that isn't there.

| Server | Setup | Vox settings |
|---|---|---|
| [Ollama](https://ollama.com) (default) | `ollama pull llama3.2` | nothing to change |
| [LM Studio](https://lmstudio.ai) | load a model, then start the server (Developer tab) | provider `openai_compatible` |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) | `llama-server -m model.gguf --port 8080` | provider `openai_compatible`, URL `http://127.0.0.1:8080` |
| Jan, vLLM, LocalAI, … | any OpenAI-compatible `/v1/chat/completions` | provider `openai_compatible` + its URL |

Choose the server under **Settings > Models**, or in `settings.yaml`:

```yaml
llm_cleanup: true
llm_provider: openai_compatible       # or: ollama
llm_base_url: http://127.0.0.1:1234   # optional; defaults to the provider's usual local port
llm_model: llama-3.2-3b-instruct
```

Keeping your dictations on your machine is the point of Vox, so it only talks to servers on
`localhost`. To use a server elsewhere on your network, set `VOX_ALLOW_REMOTE_LLM=1`. If the
server needs a key, put it in `VOX_LLM_API_KEY` (it's never written to disk).

On a machine with less than 12 GB of RAM, use a 1B-class model (`llama3.2:1b`). `vox doctor`
warns when your model is too big for your memory.

## Configuration

Everything in the settings page is stored in one human-readable file:

| OS | Data directory |
|---|---|
| macOS | `~/Library/Application Support/vox/` |
| Windows | `%LOCALAPPDATA%\Vox\` |
| Linux | `$XDG_DATA_HOME/vox` (usually `~/.local/share/vox`) |

It holds `settings.yaml`, `history.json` (last 100 dictations), `stats.json` (counts only),
`models/`, and `tray.log`. The tray reloads `settings.yaml` when it changes.

```yaml
hotkey: { enabled: true, modifiers: [control], key: Period, mode: hybrid }  # hybrid | hold | toggle
edit_hotkey: { enabled: true, modifiers: [control, shift], key: Period, mode: hybrid }
live_text: true                 # show words as you speak
input_language: auto
output_language: auto
streaming_transcription: true   # transcribe while you speak
voice_commands: true
snippets:
  - trigger: my email
    text: sam@example.com
  - trigger: my signature
    text: |-
      Thanks,
      Sam
custom_dictionary: [Kubernetes, PostgreSQL]
custom_aliases:
  Kubernetes: [coopernetes]
profiles:
  default: { format: clean_prose, privacy: local_only }
  Code: { format: code_context, privacy: local_only }
  1Password: { format: clean_prose, privacy: disabled }
```

Formats: `clean_prose`, `casual`, `professional_email`, `code_context`, `shell`, `markdown`.

<details>
<summary>Environment variables</summary>

| Variable | Effect |
|---|---|
| `VOX_DATA_DIR` | use a different data directory (portable installs, testing) |
| `VOX_ALLOW_REMOTE_LLM=1` | allow an LLM server that isn't on this machine |
| `VOX_LLM_API_KEY` | bearer token for an OpenAI-compatible server |
| `VOX_MODEL` | override the LLM model for one run |
| `VOX_WHISPER_THREADS` | Whisper CPU threads (default: physical cores) |
| `VOX_WHISPER_TIMINGS=1` | print Whisper system info and timings to stderr |
| `VOX_FULL_AUDIO_CTX=1` | disable the short-clip encoder speedup |
| `RUST_LOG=vox=debug` | verbose logging |

</details>

## Platform support

| | macOS | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Speech-to-text | ✅ Metal GPU | ✅ CPU | ✅ CPU | ✅ CPU |
| Global hotkey | ✅ | ✅ | ✅ | ⚠️ only while an XWayland window is focused |
| Per-app profiles | ✅ | ✅ | ✅ `xdotool` | ✅ KDE via `kdotool` |
| Auto-paste | ✅ Accessibility permission | ✅ not into apps running as admin | ✅ | ⚠️ compositor-dependent; text always on clipboard |
| Launch at login | ✅ | ✅ | ✅ | ✅ |

Measured performance, known limits, and the reasons behind each ⚠️ are in
[docs/PLATFORMS.md](docs/PLATFORMS.md).

## How it works

```text
 hotkey ─► audio capture ─► privacy check ─► Whisper (on-device) ─► voice commands & snippets
                                                                              │
     paste at cursor ◄─ restore clipboard ◄─ dictionary ◄─ cleanup (local rules or local LLM)
```

Every stage is a Rust trait with a real and a fake implementation, so the whole pipeline is
tested end to end without a microphone, a model, or a desktop. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## FAQ

**Does any audio or text leave my computer?**
No. Speech recognition runs on your machine. The optional LLM must be on `localhost` unless you
explicitly set `VOX_ALLOW_REMOTE_LLM=1`. There is no telemetry or update check. The only network
request Vox makes is to download a speech model when you ask for one.

**Do I need a GPU?**
No. The default model transcribes a short sentence in well under a second on a laptop CPU. On
Apple Silicon, Whisper uses the GPU automatically.

**Do I need Ollama or another LLM?**
No. Built-in cleanup handles filler words, repeated words ("so so", "I, I think"), spacing,
capitalization, your dictionary, voice commands, and snippets. An LLM adds grammar fixes, rewriting, and translation.

**`Ctrl+.` conflicts with a shortcut I use.**
Pick a new one under **Settings > Behavior > Dictation hotkey**: click it, then press your new
combination.

**Nothing was pasted.**
Run `vox doctor`. The text is always on your clipboard, so `Ctrl+V` (or `Cmd+V`) works either
way. Common causes: on macOS, the Accessibility permission is missing; on Windows, the target app
is running as administrator; on Wayland, see [Linux notes](docs/PLATFORMS.md#linux).

**It typed "Thank you." when I said nothing.**
That's a known Whisper hallucination on silence. Vox filters silence before Whisper; if it still
happens, please [open an issue](https://github.com/Siddharth189/Vox/issues/new/choose) with
your `vox doctor --json` output.

## Roadmap

Planned work, in rough priority order. Each item is open for contributors: open an issue (or
comment on the existing one) to agree on the approach before you start.

- [ ] Wayland global shortcuts through the `org.freedesktop.portal.GlobalShortcuts` portal
- [ ] Signed installers: `.msi` for Windows, notarized `.dmg` for macOS, AppImage/Flatpak for Linux
- [ ] Live text window on macOS and Linux (today: the tray title)
- [ ] More speech backends (NVIDIA Parakeet, Moonshine) behind the existing `Transcriber` trait
- [ ] Per-app snippets and per-app custom vocabulary
- [ ] Settings import/export
- [ ] Localized settings UI

See [CHANGELOG.md](CHANGELOG.md) for what has shipped.

## Contributing

Contributions of all sizes are welcome: bug reports with `vox doctor --json` output, testing on
a desktop environment we haven't covered, docs fixes, and code.

```bash
git clone https://github.com/Siddharth189/Vox.git && cd Vox
cargo test                                   # unit and pipeline tests; no mic or model needed
cargo run -- demo --dry-run "hi sam, new paragraph. um, ship it friday"
```

Start with [CONTRIBUTING.md](CONTRIBUTING.md): it covers setup on each OS, where things live in
the code, and how to add a platform backend, a cleanup rule, or a settings field. Issues labeled
[`good first issue`](https://github.com/Siddharth189/Vox/labels/good%20first%20issue) are a good
place to begin.

Please follow our [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately as
described in [SECURITY.md](SECURITY.md).

## Acknowledgements

Vox is built on [whisper.cpp](https://github.com/ggerganov/whisper.cpp) and
[whisper-rs](https://github.com/tazz4843/whisper-rs), [cpal](https://github.com/RustAudio/cpal),
[enigo](https://github.com/enigo-rs/enigo), [tray-icon](https://github.com/tauri-apps/tray-icon),
[global-hotkey](https://github.com/tauri-apps/global-hotkey), and [axum](https://github.com/tokio-rs/axum).

## License

[MIT](LICENSE). Use it, fork it, ship it.
