#!/usr/bin/env bash
set -euo pipefail

app_path="${1:?usage: scripts/audit-desktop-artifact.sh /path/to/Orbcode Desktop.app}"
resources="${app_path}/Contents/Resources"
helper="${resources}/bin/orbcode"

if [[ ! -d "${app_path}" || ! -x "${helper}" || ! -f "${app_path}/Contents/Info.plist" ]]; then
    echo "desktop app or bundled helper is missing" >&2
    exit 1
fi

if find "${resources}" -type f -perm -002 | read -r _; then
    echo "desktop bundle contains world-writable resources" >&2
    exit 1
fi

if find "${app_path}" -type f \( -name '*.pem' -o -name '*.p12' -o -name '*.key' \) | read -r _; then
    echo "desktop bundle contains signing or private-key material" >&2
    exit 1
fi

ui_root="${resources}"
if rg -a -n "https?://(localhost|127\\.0\\.0\\.1):[0-9]+|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|APPLE_(ID|PASSWORD|TEAM_ID)" "${ui_root}"; then
    echo "desktop bundle contains a development URL or credential marker" >&2
    exit 1
fi

app_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${app_path}/Contents/Info.plist")"
helper_version="$("${helper}" --version | awk 'NR == 1 { print $2 }')"
if [[ -z "${app_version}" || "${helper_version}" != "${app_version}" ]]; then
    echo "desktop app/helper version mismatch: app=${app_version:-missing} helper=${helper_version:-missing}" >&2
    exit 1
fi
"${helper}" --version
if [[ "${ORBCODE_REQUIRE_SIGNED:-0}" == "1" ]]; then
    codesign --verify --deep --strict --verbose=2 "${app_path}"
    codesign --verify --strict --verbose=2 "${helper}"
    signature="$(codesign -dv --verbose=4 "${app_path}" 2>&1)"
    if ! rg -q '^Authority=Developer ID Application:' <<<"${signature}"; then
        echo "desktop app is not signed by a Developer ID Application identity" >&2
        exit 1
    fi
    if ! rg -q 'flags=.*runtime' <<<"${signature}"; then
        echo "desktop app signature is missing Hardened Runtime" >&2
        exit 1
    fi
    helper_signature="$(codesign -dv --verbose=4 "${helper}" 2>&1)"
    if ! rg -q '^Authority=Developer ID Application:' <<<"${helper_signature}"; then
        echo "desktop helper is not signed by a Developer ID Application identity" >&2
        exit 1
    fi
    if ! rg -q 'flags=.*runtime' <<<"${helper_signature}"; then
        echo "desktop helper signature is missing Hardened Runtime" >&2
        exit 1
    fi
    for signed_target in "${app_path}" "${helper}"; do
        entitlements_dump="$(mktemp "${TMPDIR:-/tmp}/orbcode-entitlements.XXXXXX")"
        codesign -d --entitlements - "${signed_target}" >"${entitlements_dump}" 2>/dev/null
        if [[ -s "${entitlements_dump}" ]]; then
            entitlements_json="$(plutil -convert json -o - "${entitlements_dump}")"
            if [[ "${entitlements_json}" != "{}" ]]; then
                echo "desktop signature has unexpected entitlements: ${signed_target}" >&2
                rm -f "${entitlements_dump}"
                exit 1
            fi
        fi
        rm -f "${entitlements_dump}"
    done
    xcrun stapler validate "${app_path}"
    spctl --assess --type execute --verbose=2 "${app_path}"
else
    codesign --verify --deep --strict "${app_path}" 2>/dev/null || true
fi
echo "desktop artifact audit passed"
