# Changelog

All notable changes to Vox are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-11-04

### Added

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
[0.4.0]: https://github.com/Siddharth189/Vox/releases/tag/v0.4.0
[0.3.0]: https://github.com/Siddharth189/Vox/releases/tag/v0.3.0
