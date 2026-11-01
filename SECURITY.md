# Security policy

Vox handles everything you dictate, so we take security reports seriously.

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Report them privately through
[GitHub's private vulnerability reporting](https://github.com/Siddharth189/Vox/security/advisories/new).

Include what an attacker could do, the steps to reproduce, and the Vox version (`vox --version`)
and OS. We aim to acknowledge reports within a week and will credit you in the release notes
unless you'd rather we didn't.

## Scope

Especially relevant:

- Anything that sends dictated audio or text off the machine without the user opting in.
- The local settings server (`127.0.0.1:8722`): reaching it from a web page, DNS rebinding,
  reading history or changing settings cross-origin.
- File permissions on settings, history, and stats in the data directory.
- Text being pasted into an app marked `privacy: disabled`, or the microphone opening while
  such an app is focused.
- Clipboard contents leaking between dictations or into clipboard history.

Out of scope: attacks that already require running code as the same user, and problems in a
third-party LLM server you configured.

## Supported versions

Security fixes go into the latest release and `main`.
