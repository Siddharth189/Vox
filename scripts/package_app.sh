#!/usr/bin/env bash
# package_app.sh — build Vox.app bundle + codesign (ad-hoc by default).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

BUNDLE_ID="${VOX_BUNDLE_ID:-com.siddharth.vox}"
APP_NAME="Vox"
OUT_DIR="${VOX_APP_OUT:-${ROOT}/dist}"
APP="${OUT_DIR}/${APP_NAME}.app"

if [[ "${VOX_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --locked
fi

BIN="${ROOT}/target/release/vox"
if [[ ! -x "${BIN}" ]]; then
  echo "missing ${BIN}" >&2
  exit 1
fi

rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"

cp "${BIN}" "${APP}/Contents/MacOS/Vox"
chmod 755 "${APP}/Contents/MacOS/Vox"

cat > "${APP}/Contents/PkgInfo" <<'EOF'
APPL????
EOF

cat > "${APP}/Contents/Resources/README.txt" <<'EOF'
Vox — local-first menu-bar voice dictation.
Hold the global hotkey, speak, release. Text is transcribed and pasted locally.
EOF

cat > "${APP}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>Vox</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleName</key>
  <string>Vox</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Vox needs the microphone to dictate text locally.</string>
</dict>
</plist>
EOF

ENTITLEMENTS="${ROOT}/scripts/Vox.entitlements.plist"
IDENTITY="${VOX_CODESIGN_IDENTITY:--}"

codesign --force --deep --options runtime \
  --entitlements "${ENTITLEMENTS}" \
  --sign "${IDENTITY}" \
  "${APP}"

# Stable local designated requirement for ad-hoc builds
if [[ "${IDENTITY}" == "-" ]]; then
  codesign --force --deep --options runtime \
    --entitlements "${ENTITLEMENTS}" \
    --sign "-" \
    -r "=designated => identifier \"${BUNDLE_ID}\"" \
    "${APP}" || true
fi

echo "built ${APP}"
