#!/bin/bash
# Archives the iOS app (Release) and uploads it to App Store Connect for
# TestFlight.
#
#   scripts/testflight.sh            archive and upload
#   scripts/testflight.sh --no-upload archive and export an .ipa to build/testflight/
#
# Needs:
# - ios/Config/Local.xcconfig with DEVELOPMENT_TEAM (see the template).
# - The app (bundle id se.eyevinn.tallycam, or your BUNDLE_ID_PREFIX) created
#   in App Store Connect.
# - To upload, either an App Store Connect API key:
#     ASC_KEY_ID, ASC_ISSUER_ID and ASC_KEY_PATH (the .p8 file)
#   or Xcode signed in (Settings > Accounts) with an account that may upload.
#
# The build number is the number of commits on HEAD, so it grows with every
# commit on main; uploads need a number higher than the last one. Set
# BUILD_NUMBER to override. The app version comes from
# ios/Config/Base.xcconfig (MARKETING_VERSION).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/build/testflight"
UPLOAD=1
for arg in "$@"; do
    case "$arg" in
        --no-upload) UPLOAD=0 ;;
        *) echo "usage: $0 [--no-upload]" >&2; exit 2 ;;
    esac
done

if [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Applications/Xcode.app/Contents/Developer ]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

TEAM="$(sed -n 's/^DEVELOPMENT_TEAM *= *\([A-Z0-9]*\).*/\1/p' "$ROOT/ios/Config/Local.xcconfig" 2>/dev/null || true)"
if [ -z "$TEAM" ]; then
    echo "error: set DEVELOPMENT_TEAM in ios/Config/Local.xcconfig (copy Local.template.xcconfig)" >&2
    exit 1
fi

if [ -n "$(git -C "$ROOT" status --porcelain -- ios core scripts)" ]; then
    echo "warning: uncommitted changes in ios/, core/ or scripts/ go into this build" >&2
fi
BUILD_NUMBER="${BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD)}"
VERSION="$(sed -n 's/^MARKETING_VERSION *= *//p' "$ROOT/ios/Config/Base.xcconfig")"
echo "TallyCam $VERSION ($BUILD_NUMBER), team $TEAM"

AUTH=()
if [ -n "${ASC_KEY_ID:-}" ]; then
    : "${ASC_ISSUER_ID:?set ASC_ISSUER_ID with ASC_KEY_ID}"
    : "${ASC_KEY_PATH:?set ASC_KEY_PATH (the .p8 file) with ASC_KEY_ID}"
    AUTH=(-authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID"
          -authenticationKeyPath "$ASC_KEY_PATH")
fi

rm -rf "$OUT"
mkdir -p "$OUT"
ARCHIVE="$OUT/TallyCam.xcarchive"

(cd "$ROOT/ios" && xcodegen generate -q)
xcodebuild archive \
    -project "$ROOT/ios/TallyCam.xcodeproj" \
    -scheme TallyCam \
    -configuration Release \
    -destination 'generic/platform=iOS' \
    -archivePath "$ARCHIVE" \
    -derivedDataPath "$OUT/dd" \
    -allowProvisioningUpdates ${AUTH[@]+"${AUTH[@]}"} \
    CURRENT_PROJECT_VERSION="$BUILD_NUMBER" \
    | grep -E "error:|warning: .*signing|ARCHIVE (SUCCEEDED|FAILED)" || true
[ -d "$ARCHIVE" ] || { echo "error: archive failed (run again without the grep to see why)" >&2; exit 1; }

DESTINATION=$([ "$UPLOAD" = 1 ] && echo upload || echo export)
cat > "$OUT/ExportOptions.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store-connect</string>
    <key>destination</key>
    <string>$DESTINATION</string>
    <key>teamID</key>
    <string>$TEAM</string>
    <key>signingStyle</key>
    <string>automatic</string>
    <key>manageAppVersionAndBuildNumber</key>
    <false/>
</dict>
</plist>
EOF

xcodebuild -exportArchive \
    -archivePath "$ARCHIVE" \
    -exportPath "$OUT/export" \
    -exportOptionsPlist "$OUT/ExportOptions.plist" \
    -allowProvisioningUpdates ${AUTH[@]+"${AUTH[@]}"}

if [ "$UPLOAD" = 1 ]; then
    echo "Uploaded TallyCam $VERSION ($BUILD_NUMBER). It shows in TestFlight once App Store Connect has processed it (about 15 minutes)."
else
    echo "Exported: $(ls "$OUT"/export/*.ipa)"
fi
