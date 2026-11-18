#!/usr/bin/env bash
# Run outside the GUI configuration hierarchy; never alter its Cargo patches.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
export SQLX_OFFLINE=true
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
if [[ -d /usr/include/openssl-3 && -d /usr/lib/openssl-3 ]]; then
    export OPENSSL_INCLUDE_DIR="${OPENSSL_INCLUDE_DIR:-/usr/include/openssl-3}"
    export OPENSSL_LIB_DIR="${OPENSSL_LIB_DIR:-/usr/lib/openssl-3}"
fi
scratch=$(mktemp -d /tmp/opencode/core-check.XXXXXX)
trap 'rmdir -- "$scratch"' EXIT
cd -- "$scratch"
command=${1:-check}
if [[ $# -gt 0 ]]; then shift; fi
case "$command" in
    fmt) cargo fmt --manifest-path "$root/Cargo.toml" "$@" ;;
    check|test|build) cargo "$command" --manifest-path "$root/Cargo.toml" --locked --offline "$@" ;;
    *) echo 'Usage: bash docs/check.sh {fmt|check|test|build} [cargo arguments]' >&2; exit 2 ;;
esac
