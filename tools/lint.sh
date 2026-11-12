#!/usr/bin/env bash
# Format check and clippy for the whole workspace.
#
#   tools/lint.sh
#
# QNX's Rust package has no clippy or rustfmt, so both run from the rustup toolchain
# with RUSTC unset. Clippy checks the host target (x86_64-unknown-linux-gnu): the
# crates type-check there because qnx-ipc-sys always generates its bindings from the
# QNX headers, which needs QNX_TARGET from tools/env.sh.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck disable=SC1091
source tools/env.sh

env -u RUSTC cargo fmt --all --check
env -u RUSTC cargo clippy -j 6 --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
echo "lint: ok"
