#!/usr/bin/env bash
# Every check this repo has, in one command, against the running QEMU QNX 8.0
# target (start it with tools/qemu-up.sh):
#
#   1. tools/lint.sh: cargo fmt --check and clippy -D warnings (host target,
#      since QNX's Rust package has no clippy)
#   2. cargo doc --no-deps for the workspace, with rustdoc warnings as errors,
#      and the README's 30-second example compared with the file it shows
#   3. tools/build-c.sh, then every test of the workspace on the QNX target
#      (QNX_RUN_WITH_C=1, so the C interop programs are copied along)
#   4. tools/demo-check.sh: the vehicle-signals demo, checked from its log
#
#   tools/test-all.sh
#
# Writes docs/test-log.txt: the QNX and toolchain versions, then each check and
# each test with ok or FAILED. No timings. Also writes docs/demo-log.txt, the
# output of the checked demo run. Exits non-zero if anything failed;
# every step still runs, so the log shows all failures at once.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck disable=SC1091
source tools/env.sh

out="$root/docs/test-log.txt"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
# Results in run order, one "name ... ok|FAILED" line each.
: >"$work/results"

# Runs a check (name, then command), shows its output, records ok or FAILED.
check() {
  local name="$1"
  shift
  echo "== $name"
  if "$@" 2>&1 | tee "$work/step.log"; then
    echo "$name ... ok" >>"$work/results"
  else
    echo "$name ... FAILED" >>"$work/results"
    failed=1
  fi
}

check "tools/lint.sh (cargo fmt --check, clippy -D warnings)" bash tools/lint.sh

docs() {
  RUSTDOCFLAGS="-D warnings" env -u RUSTC cargo doc -j 6 --no-deps --workspace \
    --target x86_64-unknown-linux-gnu
}
check "cargo doc --no-deps (rustdoc -D warnings)" docs

# The README's 30-second example must be examples/hello_device.rs as it is (that
# file is compiled by clippy and by cargo test), so the README cannot rot.
readme_example() {
  awk '/^<!-- begin: crates\/qnx-resmgr\/examples\/hello_device.rs/ { f = 1; next }
       /^<!-- end -->/ { f = 0 }
       f' README.md | sed '1d;$d' | diff - crates/qnx-resmgr/examples/hello_device.rs
}
check "README example = crates/qnx-resmgr/examples/hello_device.rs" readme_example

check "tools/build-c.sh" bash tools/build-c.sh

# The target tests. Their per-test lines are kept; cargo's "finished in" timing
# lines and the rest are dropped.
tests() {
  QNX_RUN_WITH_C=1 cargo test -j 6 --no-fail-fast --workspace --target "$QNX_RUST_TARGET"
}
echo "== cargo test --workspace on the QNX target"
if tests 2>&1 | tee "$work/test.log"; then
  test_rc=0
else
  test_rc=1
  failed=1
fi

check "tools/demo-check.sh (vehicle-signals demo)" bash tools/demo-check.sh "$root/docs/demo-log.txt"

# Versions.
ssh_opts=(-i "$QNX_SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=no
          -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)
qnx_os="$(ssh "${ssh_opts[@]}" -p "$QNX_SSH_PORT" root@127.0.0.1 uname -a 2>/dev/null || echo unknown)"
rustc_v="$("$RUSTC" -V 2>/dev/null || echo unknown)"
cargo_v="$(cargo -V 2>/dev/null || echo unknown)"
gcc_v="$(ntox86_64-gcc --version 2>/dev/null | head -n 1 || echo unknown)"

{
  echo "# qnx-ipc test log, written by tools/test-all.sh"
  echo "#"
  echo "# QNX target:  $qnx_os (QEMU x86_64)"
  echo "# Rust:        $rustc_v (QNX host Rust package), target $QNX_RUST_TARGET"
  echo "# Cargo:       $cargo_v"
  echo "# C compiler:  qcc -Vgcc_ntox86_64 ($gcc_v)"
  echo
  echo "## Checks"
  cat "$work/results"
  echo
  echo "## Tests on the QNX target (cargo test --workspace, QNX_RUN_WITH_C=1)"
  # One section per test binary ("Running tests/x.rs (.../deps/x-0123abcd)"
  # becomes "### x (tests/x.rs)"), tests sorted by name so the log only
  # changes when a result does. Binaries without tests are left out.
  awk '
    function flush() {
      if (n > 0) {
        print ""; print head
        cmd = "sort"
        for (i = 1; i <= n; i++) print lines[i] | cmd
        close(cmd)
      }
      n = 0
    }
    /^ *Running / {
      flush()
      src = $0; sub(/^ *Running (unittests )?/, "", src); sub(/ \(.*$/, "", src)
      bin = $0; sub(/^.*\/deps\//, "", bin); sub(/-[0-9a-f]+\)$/, "", bin)
      head = "### " bin " (" src ")"
      next
    }
    /^test .* \.\.\. (ok|FAILED|ignored)$/ { lines[++n] = $0 }
    END { flush() }
  ' "$work/test.log"
  if [ "$test_rc" -ne 0 ]; then
    echo
    echo "cargo test ... FAILED"
  fi
} >"$out"

echo
cat "$out"
if [ "$failed" -ne 0 ]; then
  echo "test-all: FAILED" >&2
  exit 1
fi
echo "test-all: ok"
