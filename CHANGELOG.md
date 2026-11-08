# Changelog

All notable changes to Vox are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-11-08

### Added

- **Bilingual accuracy.** Set the input language to the languages you speak ("English, Hindi",
  "en+hi") and Vox detects only among those, so a short accented clip no longer comes out as
  Arabic or Indonesian. Short, unsure clips fall back to the language you were last speaking
  clearly. Text in a script none of your languages use is never pasted ("didn't catch that - try
  a full sentence"). Names from the focused window's title help Whisper spell them right.
- **Tells you when the LLM isn't running.** With LLM cleanup on but no server answering, the tray
  and `vox doctor` say so, including when that means dictations aren't being translated.
  Whisper's confidence is saved with each dictation in history.
- **Built-in cleanup tidies spoken repeats.** Without an LLM, "so so what I was thinking",
  "I, I think" and "we need to, we need to ship" now come out as one copy. Repeats that are
  usually meant stay ("very very good", "no no", "I know that that works", phone numbers).
  Across a full stop, only unambiguous restarts are merged ("go to the. The store"); "I think
  so. So we ship" is left as is.

- **Transcribe while you speak.** Vox cuts the recording at natural pauses and transcribes each
  piece in the background, so stopping only waits for the last few seconds of speech instead of
  the whole dictation (previously 3.2 s for 14 s of speech and 9.3 s for 40 s on a 4-core
  laptop). Each piece continues from the text before it, and if one fails the recording is
  transcribed in one pass as before. On by default; **Transcribe while you speak** in settings
  turns it off. `vox transcribe FILE.wav --stream` measures it.

- **Slow-dictation warning.** When a dictation takes longer than you spoke, and much longer
  than usual on your machine, the tray says so and why when it can tell ("on battery: plug in
  for full speed").
- **Speed in history and `vox doctor`.** Each dictation shows how fast it ran relative to your
  speech, and `vox doctor` reports the median and how to fix a slow machine.
- **Smarter correction learning.** Corrections are compared word by word, so a split or merged
  word ("voxand" to "Vox and") no longer throws off the rest. Vox suggests new names and jargon
  from your fix for the dictionary, and learns multi-word mishearings ("visual studio coat").
- **Speed benchmark in CI and releases** (`scripts/bench.sh`): Windows and Linux builds fail if
  Whisper runs slower than real time.

### Fixed

- **Whisper loops over several sentences are caught.** A short clip could come back as the
  same three sentences repeated three times plus a cut-off fourth; Vox now keeps one copy, as it
  already did for a single repeated sentence.

- **Mid-dictation language switches with several languages set.** With "English, Hindi", each
  longer piece of a streamed dictation is checked again, and Vox switches when it's clearly the
  other language (so Indian-accented English isn't decoded as Hindi).
- **No more silently missing sentences.** A piece discarded as the wrong script is retried with
  the next phrase; if part is still lost, the tray says "inserted (part not caught)".
- **Hinglish keeps Devanagari.** The wrong-script guard no longer throws away Hindi script for
  the Hinglish setting.
- **The window title can't be pasted as dictation.** When Whisper only repeats its prompt (window
  title, dictionary) on a mumble or an unsure clip, nothing is inserted. Clearly saying a name
  from the title still works.
- **Window-title hint keeps the site name** after a page title too long to fit.

## [0.3.0] - 2026-11-02

### Added

- **Prebuilt downloads.** Every release now ships ready-to-run packages for Windows, macOS
  (Apple Silicon), and Linux, with SHA-256 checksums. No Rust toolchain needed.
- **Voice commands.** Say "new line", "new paragraph", or "scratch that" as its own phrase.
  Vox ignores them when they're part of a sentence and never inserts line breaks into a
  terminal. Toggle with `voice_commands`.
- **Snippets.** Say a trigger phrase to insert stored text verbatim (signatures, addresses,
  links). Manage them in the new Snippets section of the settings page or under `snippets` in
  `settings.yaml`.
- **Usage stats.** Words dictated, typing time saved, speaking pace, and daily streak, shown at
  the top of the settings page and with `vox stats`. Stored in `stats.json` as counts only.
- **OpenAI-compatible LLM servers.** LM Studio, llama.cpp's `llama-server`, Jan, vLLM, and
  similar servers work alongside Ollama (`llm_provider`, `llm_base_url`). Servers that aren't
  on this machine are ignored unless `VOX_ALLOW_REMOTE_LLM=1`. Optional `VOX_LLM_API_KEY`.
- `vox doctor` checks the configured LLM server, whichever provider it is.
- `vox demo` applies voice commands and snippets, so you can test them without a microphone.
- Windows support: installer, windowless `vox-tray.exe`, per-app profiles by executable name,
  clipboard-history exclusion, launch at login.
- Hybrid hotkey (tap for hands-free, hold to talk), `Ctrl+.` default on every OS, start and stop
  sounds, microphone selection, built-in cleanup that needs no LLM.

### Changed

- The tray rebuilds its pipeline when the LLM server, voice command, or snippet settings change.
- README reorganized; platform details moved to `docs/PLATFORMS.md`.

## [0.1.0]

- First release: macOS tray dictation with local Whisper, Ollama cleanup, per-app profiles,
  custom dictionary, and the local settings page.

[Unreleased]: https://github.com/Siddharth189/Vox/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Siddharth189/Vox/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Siddharth189/Vox/releases/tag/v0.3.0
