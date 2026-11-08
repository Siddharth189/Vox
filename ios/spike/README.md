# Vox iOS spike

A throwaway test. It answers one question before the real iPhone keyboard is
built: can a custom keyboard and its app share data when installed with a free
Apple ID from Windows, with no Mac and no paid developer account?

## Install

1. Download `VoxSpike-ipa` from the latest **iOS spike** run in GitHub Actions and unzip it.
2. On Windows, install iTunes and iCloud from Apple's website (not the Microsoft Store), then [Sideloadly](https://sideloadly.io).
3. Connect the iPhone by USB, open `VoxSpike.ipa` in Sideloadly, and sign in with a spare Apple ID.
4. On the iPhone, trust the profile in Settings > General > VPN & Device Management, and turn on Settings > Privacy & Security > Developer Mode.

## Test

1. Go to Settings > General > Keyboard > Keyboards > Add New Keyboard and add Vox Spike.
2. Tap Vox Spike in that list and turn on **Allow Full Access**.
3. Open Vox Spike and tap **Send token to keyboard**.
4. Tap the text field, switch to the Vox Spike keyboard with the globe key, and tap **Run test**.
5. Green **PASS** means the shared data works. Text should also appear in the field.
6. Open Notes, switch to the same keyboard, and tap **Run test** to confirm it types in other apps.
