#!/usr/bin/env bash
# Build and package a release `orbcode` binary with a stable, predictable name.
#
# Produces an archive named `orbcode-<version>-<target>.<ext>` (`.tar.gz` on
# Unix, `.zip` on Windows) plus a matching `.sha256` checksum under the output
# directory. Naming is single-sourced here so CI and local runs agree.
#
# Usage:
#   scripts/package-release.sh [options]
#
# Options:
#   --target <triple>   Cargo target triple (default: host triple from rustc).
#   --out-dir <dir>     Output directory for archives (default: dist/).
#   --no-build          Reuse an existing release binary; do not run cargo.
#   --print-name        Print the artifact base name + archive file, then exit.
#   -h, --help          Show this help.
#
# The version is read from the workspace `[workspace.package] version`, which is
# the same value baked into the binary via CARGO_PKG_VERSION.
#
# Exit codes: 0 = archive + checksum produced; non-zero = first failing step.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

target=""
out_dir="${repo_root}/dist"
do_build=1
print_name_only=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            target="${2-}"
            shift 2
            ;;
        --out-dir)
            out_dir="${2-}"
            shift 2
            ;;
        --no-build)
            do_build=0
            shift
            ;;
        --print-name)
            print_name_only=1
            shift
            ;;
        -h|--help)
            sed -nE 's/^# ?//p' "${BASH_SOURCE[0]}" | sed -n '1,30p'
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

host_target() {
    rustc -vV | sed -n 's/^host: //p'
}

if [[ -z "${target}" ]]; then
    target="$(host_target)"
fi
if [[ -z "${target}" ]]; then
    echo "ERROR: could not determine target triple" >&2
    exit 1
fi

# Version is the single source of truth in the workspace manifest. The
# `^version = "..."` anchor only matches the [workspace.package] line; cargo
# dependency lines never start with a bare `version = `.
version="$(sed -nE 's/^version = "([^"]+)".*/\1/p' "${repo_root}/Cargo.toml" | head -n1)"
if [[ -z "${version}" ]]; then
    echo "ERROR: could not read workspace version from Cargo.toml" >&2
    exit 1
fi

base_name="orbcode-${version}-${target}"

bin_name="orbcode"
archive_ext="tar.gz"
case "${target}" in
    *windows*)
        bin_name="orbcode.exe"
        archive_ext="zip"
        ;;
esac
archive_file="${base_name}.${archive_ext}"

if [[ "${print_name_only}" -eq 1 ]]; then
    echo "base_name=${base_name}"
    echo "archive_file=${archive_file}"
    echo "checksum_file=${archive_file}.sha256"
    echo "bin_name=${bin_name}"
    echo "target=${target}"
    echo "version=${version}"
    exit 0
fi

echo "==> package-release.sh"
echo "    repo:    ${repo_root}"
echo "    version: ${version}"
echo "    target:  ${target}"
echo "    archive: ${archive_file}"

if [[ "${do_build}" -eq 1 ]]; then
    echo "==> cargo build --release -p orbcode --target ${target}"
    cargo build --release -p orbcode --target "${target}"
fi

binary="${repo_root}/target/${target}/release/${bin_name}"
# A plain `cargo build --release` (no --target) lands the host binary directly
# under target/release. When packaging the host target with --no-build, accept
# that location so we can reuse an already-built binary.
if [[ ! -f "${binary}" && "${target}" == "$(host_target)" ]]; then
    fallback="${repo_root}/target/release/${bin_name}"
    if [[ -f "${fallback}" ]]; then
        binary="${fallback}"
    fi
fi
if [[ ! -f "${binary}" ]]; then
    echo "ERROR: built binary not found at ${binary}" >&2
    echo "       (run without --no-build, or build the target first)" >&2
    exit 1
fi

mkdir -p "${out_dir}"
stage_dir="${out_dir}/${base_name}"
rm -rf "${stage_dir}"
mkdir -p "${stage_dir}"
cp "${binary}" "${stage_dir}/${bin_name}"

# A small provenance file travels with the binary so an extracted archive is
# self-describing without running the binary.
{
    echo "name: orbcode"
    echo "version: ${version}"
    echo "target: ${target}"
    echo "packaged: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "${stage_dir}/BUILDINFO.txt"

archive_path="${out_dir}/${archive_file}"
rm -f "${archive_path}" "${archive_path}.sha256"

echo "==> creating ${archive_ext} archive"
case "${archive_ext}" in
    tar.gz)
        tar -czf "${archive_path}" -C "${out_dir}" "${base_name}"
        ;;
    zip)
        if command -v zip >/dev/null 2>&1; then
            ( cd "${out_dir}" && zip -r -q "${archive_file}" "${base_name}" )
        elif command -v 7z >/dev/null 2>&1; then
            ( cd "${out_dir}" && 7z a -tzip "${archive_file}" "${base_name}" >/dev/null )
        else
            # PowerShell is always available on Windows runners.
            powershell -NoProfile -Command \
                "Compress-Archive -Force -Path '${stage_dir}' -DestinationPath '${archive_path}'"
        fi
        ;;
esac

if [[ ! -f "${archive_path}" ]]; then
    echo "ERROR: archive was not created at ${archive_path}" >&2
    exit 1
fi

echo "==> writing sha256 checksum"
checksum_path="${archive_path}.sha256"
if command -v sha256sum >/dev/null 2>&1; then
    ( cd "${out_dir}" && sha256sum "${archive_file}" > "${archive_file}.sha256" )
elif command -v shasum >/dev/null 2>&1; then
    ( cd "${out_dir}" && shasum -a 256 "${archive_file}" > "${archive_file}.sha256" )
else
    echo "ERROR: no sha256 tool (sha256sum/shasum) found" >&2
    exit 1
fi

echo
echo "OK: packaged release artifact"
echo "    archive:  ${archive_path}"
echo "    checksum: ${checksum_path}"
cat "${checksum_path}"
