# Vox architecture

Vox is a local-first tray dictation app for macOS, Windows, and Linux. Press the
hotkey (default `Ctrl+.`) → speak → finish → Whisper transcribes → voice commands and snippets → cleanup
(local rules, or a local LLM server when running) → dictionary normalization →
paste at the cursor.

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

   **Streaming** (`stt/streaming.rs`, on by default, setting
   `streaming_transcription`). Whole-clip transcription makes the wait after
   stop grow with the dictation (3.2 s for 14 s of speech, 9.3 s for 40 s on a
   4-core laptop with `base-q5_1`). Instead, when recording starts the tray
   opens a `StreamingSession` on the recorder's `LiveTap` (a shared view of the
   growing capture buffer; `Recorder::start` allocates a fresh one per
   dictation, so a draining worker never sees the next dictation's audio). A
   worker thread polls it every 150 ms, and a pure `Segmenter` cuts it in the
   middle of the first sentence-length pause (700 ms below 10% of the
   segment's 95th-percentile level; comma pauses measure 460-660 ms) at least
   6 s in, takes the longest pause of 200 ms or more after 12 s, and cuts at the
   quietest window by 20 s regardless. Each segment goes through the usual
   trim/resample/silence gate and is transcribed with a `SegmentHint`: the last
   200 characters so far as Whisper's prompt (after the dictionary), and the
   language detected in the first segment when the input language is `auto`.
   A pause isn't always a sentence end: decoded alone, the fragment "And so my
   fellow Americans, ask not" (JFK's pause for emphasis) came back as "asked
   not". So a segment is kept only if Whisper ends it with sentence
   punctuation (`ends_sentence`, any script). Otherwise the cut is deferred
   and the audio is decoded again, longer, at the next pause; from 12 s on it
   is kept regardless. The redo costs CPU while the user is still talking, not
   wait time. At stop, the tail is decoded in one piece.
   On stop, `Pipeline::prepare_streamed` re-checks privacy, then finishes the
   session: only the tail since the last pause is left (about 2 s of speech on
   real recordings). Whisper sometimes starts a segment by repeating the end of its prompt;
   those leading words (3 or more, matching the previous text's end) are
   dropped. `scripts/bench-stream.ps1` (Windows CI) checks a synthesized 40 s
   paragraph streamed against one pass for repeats, split sentences and drift.
   If any segment fails, the whole recording is transcribed in one pass, the
   same as without streaming. Cancel (Esc) drops the session. History records
   `streamed_segments`, and `transcribe_ms` is then only the wait after stop;
   `stt_work_ms` is Whisper's total time across segments, which is what
   `speed.rs` (slow-dictation warnings, `vox doctor`) measures against
   `audio_ms`, so streaming can't hide a slow build.
   `vox transcribe FILE.wav --stream` plays a file through this path in real
   time and reports that wait.
4. **Context** - the foreground app, used as the per-app profile key:
   - macOS: NSWorkspace bundle ID (`context/mac_detector.rs`, main thread, fail closed)
   - Windows: foreground window's process image name, e.g. `Code`
     (`context/windows_detector.rs`)
   - Linux: WM_CLASS via `kdotool`/`xdotool` (`context/linux_detector.rs`)

   Windows and Linux fall back to the `default` profile rather than failing.
   Profile lookup is case-insensitive.
5. **Commands** (`process/commands.rs`) - splits the transcript into speech
   and literals. "new line" / "new paragraph" become line breaks and
   "scratch that" drops the previous sentence, but only when spoken as their
   own clause (punctuation on both sides), so ordinary sentences are never
   rewritten; line breaks are never inserted in `shell` profiles. Snippet
   triggers match anywhere and expand verbatim. Only speech segments go
   through cleanup, so the LLM can't rewrite a snippet. With no match, the
   transcript passes through unchanged.
6. **Process** (`process/`) - `CleanupProcessor` uses the LLM server when a cached,
   background-refreshed probe says it's up with the configured model
   (`cleanup.rs`), and the rule-based `LocalCleanup` otherwise (`local.rs`:
   filler removal, spoken repeats (`repeats.rs`: stutters and repeated
   phrases, restarts across a full stop only where unambiguous, never in
   shell), spacing, capitalization, shell/code-aware formatting, dictionary
   aliases). The probe never runs on the dictation's critical path:
   on Windows a connect to a closed localhost port takes ~2 s.
7. **Inject** (`inject/`) - clipboard + synthesized paste via `enigo`, after
   waiting for the hotkey's modifiers to be physically released (otherwise
   Ctrl+Alt+V / AltGr+V gets sent). Cmd+V on macOS (with an `osascript`
   fallback), Ctrl+V with the layout-independent `VK_V` on Windows, Ctrl+V via
   XTest/libei on Linux. The previous clipboard (text or image) is restored
   afterwards unless something else replaced it; on Windows dictations are
   excluded from Win+V clipboard history and cloud sync.

   The LLM server is Ollama (`/api/chat`) or any OpenAI-compatible server
   (`/v1/chat/completions`: LM Studio, llama-server, Jan, vLLM), set by
   `llm_provider` and `llm_base_url` (`process/ollama.rs`). Non-loopback URLs
   are refused unless `VOX_ALLOW_REMOTE_LLM=1`, both in the probe and in the
   request itself.

## Usage stats

`stats.rs` keeps lifetime totals in `stats.json`: dictations, words, speech
time, and words per UTC day (last 400 days) for streaks. Counts only, never
text. The tray adds to it after each paste; `/api/stats` and `vox stats`
report words, minutes saved against a 40 wpm typist, speaking pace, and streak.

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
`history.json`, `stats.json`, `models/`, `vox.lock`, and `tray.log`. Writes are atomic
(temp file + rename). On Unix they're `0600`/`0700`; on Windows,
%LOCALAPPDATA%'s inherited ACL is already per-user.

Launch at login (`autostart.rs`): HKCU `Run` key on Windows, a LaunchAgent on
macOS, XDG autostart on Linux.

## Settings UI

Axum binds `127.0.0.1:8722`. Middleware requires the `Host` to be the
loopback address (DNS-rebinding protection) and any `Origin` to be the page
itself. See `SETTINGS_UI.md`.
