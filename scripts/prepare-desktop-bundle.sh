#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
desktop_root="${repo_root}/clients/desktop"
helper_destination="${desktop_root}/src-tauri/resources/bin/orbcode"

workspace_version="$({
    sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' "${repo_root}/Cargo.toml"
} | head -n 1)"
desktop_version="$(node -p "require('${desktop_root}/package.json').version")"
tauri_version="$({
    sed -n 's/^version = "\([^"]*\)"/\1/p' "${desktop_root}/src-tauri/Cargo.toml"
} | head -n 1)"
tauri_config_version="$(node -p "require('${desktop_root}/src-tauri/tauri.conf.json').version")"
compatibility_version="$({
    sed -n 's/^export const DESKTOP_VERSION = "\([^"]*\)";/\1/p' "${desktop_root}/src/compatibility.ts"
} | head -n 1)"

if [[ -z "${workspace_version}" || "${desktop_version}" != "${workspace_version}" || "${tauri_version}" != "${workspace_version}" || "${tauri_config_version}" != "${workspace_version}" || "${compatibility_version}" != "${workspace_version}" ]]; then
    echo "desktop/helper version mismatch: workspace=${workspace_version:-missing} renderer=${desktop_version} host=${tauri_version} tauri-config=${tauri_config_version} compatibility=${compatibility_version:-missing}" >&2
    exit 1
fi

if [[ "${ORBCODE_DESKTOP_SKIP_HELPER_BUILD:-0}" != "1" ]]; then
    cargo build --release -p orbcode --manifest-path "${repo_root}/Cargo.toml"
fi

helper_source="${repo_root}/target/release/orbcode"
if [[ ! -x "${helper_source}" ]]; then
    echo "release helper missing or not executable: ${helper_source}" >&2
    exit 1
fi

mkdir -p "$(dirname "${helper_destination}")"
install -m 755 "${helper_source}" "${helper_destination}"

reported_version="$("${helper_destination}" --version | awk 'NR == 1 { print $2 }')"
if [[ "${reported_version}" != "${workspace_version}" ]]; then
    echo "staged helper reports ${reported_version}; expected ${workspace_version}" >&2
    exit 1
fi

echo "staged Orbcode ${reported_version} at ${helper_destination}"
