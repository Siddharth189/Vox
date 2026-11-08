# Vox iOS spike

A throwaway test for building the iPhone keyboard without a Mac or a paid
developer account. A free Apple ID through Sideloadly grants no App Group, so
this checks whether the keyboard can reach its app over a loopback connection
while a microphone session keeps the app alive in the background.

## Install

1. Download `VoxSpike-ipa` from the latest **iOS spike** run in GitHub Actions and unzip it.
2. Connect the iPhone by USB and install `VoxSpike.ipa` with Sideloadly.
3. If this is the first install, turn on Developer Mode, trust the profile in VPN & Device Management, add the Vox Spike keyboard, and turn on **Allow Full Access**.

## Test

1. Open Vox Spike, tap **Start mic session**, and allow the microphone. `Link` should say `Listening`.
2. Tap the text field, switch to the Vox Spike keyboard, and tap **Run test**. `Keyboard reached app` should pass.
3. Open Notes without closing Vox Spike, switch to the keyboard, and tap **Run test**. The reply should say `app=background mic=on`.
4. Wait two minutes in Notes and tap **Run test** again. `buffers` should be higher than before.
