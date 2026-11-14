# Settings UI

Local web UI served by the tray process at [http://127.0.0.1:8722](http://127.0.0.1:8722).

## Pages

- **Your dictation** - lifetime words, time saved, speaking pace, and streak
  (`GET /api/stats`).
- **Language** - Spoken language and Output language dropdowns (`languages.js`)
  writing the same `input_language` / `output_language` strings as before. A
  saved value that isn't in the list (Hinglish, "English, Hindi") is shown as
  its own entry and kept on save.
- **General** - Whisper model, LLM server (Ollama or
  OpenAI-compatible, plus URL) and model, voice commands toggle, auto-paste,
  optional system-message override with live prompt preview, hotkey editor.
- **Snippets** - trigger phrase → text inserted verbatim.
- **Dictionary** - custom dictionary terms + `Canonical = alias1, alias2` aliases.
- **Profiles** - per-bundle-id format/privacy table (`default` required).
- **History** - recent dictations with latency breakdown, speed relative to the
  length of speech, and inline correction learning (`POST /api/learn-correction`:
  learns aliases for dictionary terms and returns `suggested_terms`; send them back
  in `add_terms` to add them to the dictionary).

## Design notes

Minimal single-accent card layout, light/dark via `prefers-color-scheme`. No
external assets; HTML/JS are `include_str!`'d into the binary.
