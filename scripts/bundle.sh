#!/usr/bin/env bash
# Builds "Shot Studio.app" and Shot-Studio-<version>.dmg from release binaries into dist/.
# The app carries the `shot` CLI next to its executable in Contents/MacOS, where the app looks
# for it; the cask links it onto PATH, a DMG install links it by hand (see README.md).
#
#   scripts/bundle.sh                build, bundle, then sign and notarize when configured
#   scripts/bundle.sh --skip-build   bundle the release binaries already built
#
# Developer ID signing and notarization run only when their variables are set, otherwise the
# app is signed ad hoc and the DMG stays unsigned:
#
#   SIGN_IDENTITY     a "Developer ID Application" identity in the keychain
#   NOTARY_KEY        path to an App Store Connect API key (.p8) for notarytool
#   NOTARY_KEY_ID     its key id
#   NOTARY_ISSUER     its issuer id
#
# The app runs with the hardened runtime and without the App Sandbox: it records the screen
# through ScreenCaptureKit, opens video folders and git repos anywhere on disk, and starts the
# user's `claude` and its own `shot` for the Claude bridge, which a sandbox would block. Screen
# recording has no entitlement or usage string outside the sandbox; macOS asks on first capture.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

APP_NAME="Shot Studio"
OUT=dist
PACKAGING=packaging/macos

fail() {
  echo "bundle: $*" >&2
  exit 1
}

build=true
for arg in "$@"; do
  case "$arg" in
    --skip-build) build=false ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) fail "unknown argument $arg. See scripts/bundle.sh --help." ;;
  esac
done

[ "$(uname -sm)" = "Darwin arm64" ] || fail "the app is built on Apple Silicon macOS only"

# A same-day release has the crate version YY.M.D-N and the app version YY.M.D.N.
crate_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
version=$(printf '%s' "$crate_version" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)-([0-9]+)$/\1.\2/')
target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)

if $build; then
  cargo build --release --locked -p shot-app -p shot-cli
fi
for bin in shot-app shot; do
  [ -x "$target_dir/release/$bin" ] || fail "no release build of $bin in $target_dir/release"
done

app="$OUT/$APP_NAME.app"
dmg="$OUT/Shot-Studio-$version.dmg"
rm -rf "$app" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target_dir/release/shot-app" "$target_dir/release/shot" "$app/Contents/MacOS/"
sed "s/@VERSION@/$version/g" "$PACKAGING/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp "$PACKAGING/AppIcon.icns" "$app/Contents/Resources/"
cp LICENSE THIRD-PARTY-LICENSES.md "$app/Contents/Resources/"

# Without a Developer ID the app is signed ad hoc, so the bundle is sealed under its bundle id
# and macOS can ask for camera and microphone access; Gatekeeper still treats it as unsigned.
signed=false
identity=-
timestamp=--timestamp=none
if [ -n "${SIGN_IDENTITY:-}" ]; then
  identity=$SIGN_IDENTITY
  timestamp=--timestamp
  signed=true
else
  echo "bundle: SIGN_IDENTITY is not set, the app is signed ad hoc, not with a Developer ID" >&2
fi
# Nested code first: codesign seals the bundle over the signed `shot`.
for code in "$app/Contents/MacOS/shot" "$app"; do
  codesign --force "$timestamp" --options runtime \
    --entitlements "$PACKAGING/entitlements.plist" --sign "$identity" "$code"
done
codesign --verify --strict --deep "$app"

notarize=false
if $signed && [ -n "${NOTARY_KEY:-}" ] && [ -n "${NOTARY_KEY_ID:-}" ] && [ -n "${NOTARY_ISSUER:-}" ]; then
  notarize=true
elif $signed; then
  echo "bundle: NOTARY_KEY, NOTARY_KEY_ID or NOTARY_ISSUER is not set, skipping notarization" >&2
fi

# Submits a file to the notary service and waits; the log names any rejected binary.
notarize() {
  local auth=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")
  local result
  result=$(xcrun notarytool submit "$1" "${auth[@]}" --wait --output-format json)
  xcrun notarytool log "$(jq -r .id <<<"$result")" "${auth[@]}" >&2
  [ "$(jq -r .status <<<"$result")" = Accepted ] || fail "notarization of $1 was not accepted: $result"
}

# The app is notarized and stapled on its own, so it opens offline after it is copied out of
# the DMG; the DMG gets its own ticket for the first open.
if $notarize; then
  zip="$OUT/notarize.zip"
  ditto -c -k --keepParent "$app" "$zip"
  notarize "$zip"
  rm "$zip"
  xcrun stapler staple "$app"
fi

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
ditto "$app" "$staging/$APP_NAME.app"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$staging" -fs HFS+ -format UDZO -ov "$dmg"

if $signed; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$dmg"
fi
if $notarize; then
  notarize "$dmg"
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose "$dmg"
fi

echo "Built $app ($(du -sh "$app" | cut -f1)) and $dmg ($(du -h "$dmg" | cut -f1))."
