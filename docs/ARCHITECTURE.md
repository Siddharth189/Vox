# Vox architecture

Vox is a local-first macOS menu-bar dictation app. Hold a global hotkey → speak →
release → Whisper transcribes → Ollama cleans/formats → dictionary normalization →
auto-paste at the cursor.

## Pipeline stages

1. **Audio** (`audio/recorder.rs`) — cpal capture at the device native rate,
   mono downmix, silence trim (95th-percentile RMS threshold), linear resample
   to 16 kHz.
2. **Privacy** (`privacy.rs`) — `Privacy::Disabled` apps abort before STT.
3. **STT** (`stt/local_whisper.rs`) — whisper-rs / whisper.cpp with Metal.
4. **Context** (`context/mac_detector.rs`) — frontmost app via NSWorkspace
   (main thread only; fail closed).
5. **Process** (`process/`) — Ollama chat + prompt/few-shot + deterministic
   dictionary aliases.
6. **Inject** (`inject/`) — clipboard + synthesized Cmd+V with Accessibility
   fallbacks.

The tray runs AppKit / cpal on the main `tao` event loop, preparation on a
worker thread, and injection on a second worker so paste delays never block
hotkey handling.

## Config & data

All state lives under `~/Library/Application Support/vox/` (`settings.yaml`,
`history.json`, `models/`, `vox.lock`, `tray.log`), written `0600`/`0700`.

## Settings UI

Axum binds `127.0.0.1:8722` with same-origin middleware. See `SETTINGS_UI.md`.
