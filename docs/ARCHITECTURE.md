# Vox architecture

Vox is a local-first menu-bar dictation app for macOS and Linux. Hold a global hotkey → speak
→ release → Whisper transcribes → Ollama cleans/formats → dictionary normalization →
auto-paste at the cursor.

## Pipeline stages

1. **Audio** (`audio/recorder.rs`) - cpal capture at the device native rate,
   mono downmix, silence trim (95th-percentile RMS threshold), linear resample
   to 16 kHz. Cross-platform as-is.
2. **Privacy** (`privacy.rs`) - `Privacy::Disabled` apps abort before STT.
3. **STT** (`stt/local_whisper.rs`) - whisper-rs / whisper.cpp, Metal on
   macOS, CPU on Linux. Cross-platform as-is.
4. **Context** - frontmost app via NSWorkspace on macOS
   (`context/mac_detector.rs`, main thread only, fail closed), or via
   `kdotool`/`xdotool` on Linux (`context/linux_detector.rs`, falls back to
   the `default` profile rather than failing when neither tool applies to
   the session).
5. **Process** (`process/`) - Ollama chat + prompt/few-shot + deterministic
   dictionary aliases. Cross-platform as-is.
6. **Inject** (`inject/`) - clipboard + synthesized Cmd+V (macOS) or Ctrl+V
   (Linux) via `enigo`, with an `osascript` fallback and Accessibility
   status checks on macOS only.

The tray runs the platform GUI toolkit (AppKit/cpal on macOS, GTK via
`tray-icon` on Linux) on the main `tao` event loop, preparation on a worker
thread, and injection on a second worker so paste delays never block hotkey
handling.

## Config & data

macOS: `~/Library/Application Support/vox/`. Linux: `$XDG_DATA_HOME/vox`,
falling back to `~/.local/share/vox`. Both hold `settings.yaml`,
`history.json`, `models/`, `vox.lock`, and `tray.log`, written `0600`/`0700`.

## Settings UI

Axum binds `127.0.0.1:8722` with same-origin middleware. See `SETTINGS_UI.md`.
