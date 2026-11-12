# Source this file (do not run it): sets up QNX SDP 8.0, QNX's host Rust toolchain
# and the build paths used by every other script in tools/.
#
#   source tools/env.sh
#
QNX_IPC_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export QNX_IPC_ROOT

# QNX SDP 8.0 (sets QNX_HOST, QNX_TARGET and puts qcc on PATH).
# shellcheck disable=SC1091
source "${QNX_SDP:-$HOME/qnx800}/qnxsdp-env.sh" >/dev/null

# QNX ships rustc (with std prebuilt for QNX 8.0) but no cargo, so we use rustup's
# cargo of the same version and point it at QNX's rustc.
export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=1.85.1
export RUSTC="$QNX_HOST/usr/bin/rustc"
export QNX_RUST_TARGET=x86_64-pc-nto-qnx800

# Keep build output on the Linux filesystem (much faster than /mnt/c).
export CARGO_TARGET_DIR="$HOME/build/qnx-ipc"

# Tests and examples for the QNX target run on the QEMU target through this runner.
export CARGO_TARGET_X86_64_PC_NTO_QNX800_RUNNER="$QNX_IPC_ROOT/tools/qnx-run"

# QEMU target location and SSH port forwarded from the guest.
export QNX_IMAGE_DIR="${QNX_IMAGE_DIR:-$HOME/qnx-images/ipc}"
export QNX_SSH_PORT="${QNX_SSH_PORT:-2222}"
export QNX_SSH_KEY="${QNX_SSH_KEY:-$HOME/.ssh/qnx_target_ed25519}"
