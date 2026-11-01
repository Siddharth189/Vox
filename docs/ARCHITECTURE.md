# Vox architecture

Vox is a local-first tray dictation app for macOS, Windows, and Linux. Press the
hotkey (default `Ctrl+.`) → speak → finish → Whisper transcribes → cleanup
(local rules, or Ollama when running) → dictionary normalization → paste at the
cursor.

## Pipeline stages

1. **Audio** (`audio/recorder.rs`) - cpal capture at the device's native rate,
   mono downmix, silence trim (95th-percentile RMS threshold), windowed-sinc
   resample to 16 kHz (anti-aliased; linear interpolation folded 8-24 kHz noise
   into the speech band), 5-minute hard cap per dictation. An absolute energy
   gate (`is_silent`) keeps room noise away from Whisper, which otherwise
   hallucinates "Thank you." on silence. `audio/cue.rs` synthesizes the
   start/stop chirps; `audio/wav.rs` reads WAV files for `vox transcribe`.
2. **Privacy** (`privacy.rs`) - `Privacy::Disabled` apps (password managers by
   default) are checked *before the mic opens*, and again before STT.
3. **STT** (`stt/local_whisper.rs`) - whisper-rs / whisper.cpp. Metal on macOS,
   CPU elsewhere, threads = physical cores. The decoder state is reused across
   dictations, clips under 1 s are padded (whisper.cpp drops them otherwise),
   `auto` really auto-detects (whisper.cpp's own default is English), and the
   model is warmed up when the pipeline is built.
4. **Context** - the foreground app, used as the per-app profile key:
   - macOS: NSWorkspace bundle ID (`context/mac_detector.rs`, main thread, fail closed)
   - Windows: foreground window's process image name, e.g. `Code`
     (`context/windows_detector.rs`)
   - Linux: WM_CLASS via `kdotool`/`xdotool` (`context/linux_detector.rs`)

   Windows and Linux fall back to the `default` profile rather than failing.
   Profile lookup is case-insensitive.
5. **Process** (`process/`) - `CleanupProcessor` uses Ollama when a cached,
   background-refreshed probe says it's up with the configured model
   (`cleanup.rs`), and the rule-based `LocalCleanup` otherwise (`local.rs`:
   filler removal, spacing, capitalization, shell/code-aware formatting,
   dictionary aliases). The probe never runs on the dictation's critical path:
   on Windows a connect to a closed localhost port takes ~2 s.
6. **Inject** (`inject/`) - clipboard + synthesized paste via `enigo`, after
   waiting for the hotkey's modifiers to be physically released (otherwise
   Ctrl+Alt+V / AltGr+V gets sent). Cmd+V on macOS (with an `osascript`
   fallback), Ctrl+V with the layout-independent `VK_V` on Windows, Ctrl+V via
   XTest/libei on Linux. The previous clipboard (text or image) is restored
   afterwards unless something else replaced it; on Windows dictations are
   excluded from Win+V clipboard history and cloud sync.

## Tray and hotkey

`tray.rs` runs the platform GUI toolkit on the main `tao` event loop. Hotkey
and menu events are forwarded into the loop through `EventLoopProxy` as they
happen. Work runs on worker threads: pipeline builds (model load and warm-up),
first-run model download, transcription, and injection, so paste delays never
block hotkey handling.

`hotkey.rs` holds the key parser (accepts `.`, `Period`, `a`, `KeyA`, `F5`...)
and the push-to-talk state machine. In the default `hybrid` mode a tap
(< 350 ms) latches hands-free recording until the next press, and a longer
hold stops on release. `hold` and `toggle` modes are also available. Esc is
registered only while recording, to cancel.

Windows ships two executables: `vox.exe` (console CLI) and `vox-tray.exe`
(GUI subsystem, so launching it from the Start menu or at login doesn't flash
a console). Logs go to `tray.log`.

## Config & data

| OS      | Data dir                                   |
|---------|--------------------------------------------|
| macOS   | `~/Library/Application Support/vox/`       |
| Windows | `%LOCALAPPDATA%\Vox\`                      |
| Linux   | `$XDG_DATA_HOME/vox` or `~/.local/share/vox` |

`VOX_DATA_DIR` overrides the data dir on any OS. It holds `settings.yaml`,
`history.json`, `models/`, `vox.lock`, and `tray.log`. Writes are atomic
(temp file + rename). On Unix they're `0600`/`0700`; on Windows,
%LOCALAPPDATA%'s inherited ACL is already per-user.

Launch at login (`autostart.rs`): HKCU `Run` key on Windows, a LaunchAgent on
macOS, XDG autostart on Linux.

## Settings UI

Axum binds `127.0.0.1:8722`. Middleware requires the `Host` to be the
loopback address (DNS-rebinding protection) and any `Origin` to be the page
itself. See `SETTINGS_UI.md`.
