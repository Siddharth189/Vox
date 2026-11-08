# Vox iOS spike

Hold-to-talk dictation on iPhone, built without a Mac or a paid developer
account. A free Apple ID through Sideloadly grants no App Group, so the keyboard
talks to its app over a loopback connection. A microphone session keeps the app
alive in the background. Speech goes to Groq, which returns English text.

## Install

1. Download `VoxSpike-ipa` from the latest **iOS spike** run in GitHub Actions and unzip it.
2. Connect the iPhone by USB and install `VoxSpike.ipa` with Sideloadly. Free signing lasts 7 days.
3. On a first install, turn on Developer Mode, trust the profile in VPN & Device Management, add the Vox Spike keyboard, and turn on **Allow Full Access**.

## Use

1. Open Vox Spike and save a Groq API key once. It stays in the iPhone's Keychain.
2. Opening the app starts a session. `Mic` should say `Running` and `Link` should say `Listening`.
3. Go to any app, switch to the Vox Spike keyboard, hold **Hold to talk**, speak, and release.

A session ends after 5 idle minutes. Open the app again to start a new one.
