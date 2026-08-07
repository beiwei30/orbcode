#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
app_path="${1:-${repo_root}/clients/desktop/src-tauri/target/release/bundle/macos/Orbcode Desktop.app}"
identity="${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY must name a Developer ID Application identity}"
notary_profile="${APPLE_NOTARY_KEYCHAIN_PROFILE:?APPLE_NOTARY_KEYCHAIN_PROFILE must name a notarytool keychain profile}"
entitlements="${repo_root}/clients/desktop/src-tauri/Entitlements.plist"
helper="${app_path}/Contents/Resources/bin/orbcode"
output_dir="${ORBCODE_DESKTOP_DIST_DIR:-${repo_root}/dist/desktop}"
archive="${output_dir}/Orbcode-Desktop.zip"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "desktop signing and notarization require macOS" >&2
    exit 1
fi
if [[ ! -x "${helper}" ]]; then
    echo "bundled helper missing: ${helper}" >&2
    exit 1
fi

codesign --force --timestamp --options runtime --sign "${identity}" "${helper}"
codesign --force --timestamp --options runtime --entitlements "${entitlements}" --sign "${identity}" "${app_path}"
codesign --verify --deep --strict --verbose=2 "${app_path}"

mkdir -p "${output_dir}"
ditto -c -k --keepParent "${app_path}" "${archive}"
xcrun notarytool submit "${archive}" --keychain-profile "${notary_profile}" --wait
xcrun stapler staple "${app_path}"
xcrun stapler validate "${app_path}"
spctl --assess --type execute --verbose=2 "${app_path}"
ditto -c -k --keepParent "${app_path}" "${archive}"

echo "signed and notarized desktop app: ${app_path}"
echo "notarization submission archive: ${archive}"
