#!/usr/bin/env bash
# package_release.sh — zip/dmg release artifacts from a built Vox.app.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${VOX_APP_OUT:-${ROOT}/dist}"
APP="${OUT_DIR}/Vox.app"
VERSION="${VOX_VERSION:-0.1.0}"

if [[ ! -d "${APP}" ]]; then
  "${ROOT}/scripts/package_app.sh"
fi

mkdir -p "${OUT_DIR}"
ZIP="${OUT_DIR}/Vox-${VERSION}-macos.zip"
DMG="${OUT_DIR}/Vox-${VERSION}-macos.dmg"

rm -f "${ZIP}" "${DMG}"
(
  cd "${OUT_DIR}"
  zip -r "Vox-${VERSION}-macos.zip" Vox.app
)

if command -v hdiutil >/dev/null 2>&1; then
  hdiutil create -volname "Vox" -srcfolder "${APP}" -ov -format UDZO "${DMG}"
  echo "wrote ${DMG}"
else
  echo "hdiutil not available; skipped dmg"
fi

echo "wrote ${ZIP}"
