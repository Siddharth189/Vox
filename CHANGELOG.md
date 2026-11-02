# Changelog

All notable changes to Vox are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
