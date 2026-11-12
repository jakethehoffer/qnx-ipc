#!/usr/bin/env bash
# Checks that every `compile_fail` example in the crates' docs fails to compile
# for the reason its comment states.
#
#   tools/compile-fail.sh
#
# rustdoc cannot run doctests for the QNX target, so the docs' compile_fail
# examples would otherwise never be compiled. This script takes each
# ```compile_fail block from crates/*/src/*.rs, builds it as the body of `main`
# in a scratch crate that depends on qnx-ipc, for the Linux host (RUSTC unset,
# as for tools/lint.sh; nothing is run), and requires the build to fail with
# the error the block names in a comment, for example
#
#   drop(conn); // error[E0505]: cannot move out of `conn` because it is borrowed
#
# The comment is removed before compiling (rustc echoes source lines). Then the
# compiler must report an error with that code (an `error[E....]` line) and the
# text after it must appear in its output, so an example that fails for an
# unrelated reason fails this check. A block without such a comment fails too.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck disable=SC1091
source tools/env.sh

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Extract the blocks: $work/blocks/<n>.rs, and "<n> <file>:<line>" in $work/index.
mkdir -p "$work/blocks"
for f in crates/*/src/*.rs; do
  awk -v dir="$work/blocks" -v file="$f" -v start="$(find "$work/blocks" -name '*.rs' | wc -l)" '
    BEGIN { n = start }
    /^ *\/\/\/ ```compile_fail/ { n++; out = dir "/" n ".rs"; print n " " file ":" FNR > (dir "/../index." n); inb = 1; next }
    inb && /^ *\/\/\/ ```/ { inb = 0; close(out); next }
    inb { line = $0; sub(/^ *\/\/\/ ?/, "", line); sub(/^# /, "", line); print line > out }
  ' "$f"
done
cat "$work"/index.* 2>/dev/null | sort -n >"$work/index"
if [ ! -s "$work/index" ]; then
  echo "compile-fail: no compile_fail examples found" >&2
  exit 1
fi

# A scratch crate outside the workspace, with the workspace's lock file so the
# dependency versions are the ones already built.
crate="$work/crate"
mkdir -p "$crate/src"
cat >"$crate/Cargo.toml" <<EOF
[package]
name = "compile-fail-check"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
qnx-ipc = { path = "$root/crates/qnx-ipc" }
qnx-resmgr = { path = "$root/crates/qnx-resmgr" }
bytemuck = { version = "1", features = ["derive"] }

[workspace]
EOF
cp Cargo.lock "$crate/Cargo.lock"

failed=0
while read -r n where; do
  block="$work/blocks/$n.rs"
  comment="$(grep -o 'error\[E[0-9]\{4\}\]: .*$' "$block" | head -n 1)"
  if [ -z "$comment" ]; then
    echo "$where: FAILED (no '// error[E....]: ...' comment names the expected error)"
    failed=1
    continue
  fi
  code="${comment%%:*}"
  text="${comment#*: }"
  {
    echo "fn main() {"
    sed 's|// *error\[E[0-9]\{4\}\]:.*$||' "$block"
    echo "}"
  } >"$crate/src/main.rs"
  # `build`, not `check`: a const evaluation error in a generic function only
  # shows when the function is instantiated for code generation. The examples
  # are meant to fail before anything is linked.
  if env -u RUSTC CARGO_TARGET_DIR="$CARGO_TARGET_DIR/compile-fail" \
       cargo build -q -j 6 --offline --manifest-path "$crate/Cargo.toml" \
       --target x86_64-unknown-linux-gnu >"$work/out" 2>&1; then
    echo "$where: FAILED (compiled, but should fail with $code)"
    failed=1
  elif ! grep -qF -- "$text" "$work/out" ||
       ! awk -v c="$code" 'index($0, c) == 1 { f = 1 } END { exit !f }' "$work/out"; then
    echo "$where: FAILED (did not fail with: $comment)"
    cat "$work/out"
    failed=1
  else
    echo "$where: fails as documented ($code)"
  fi
done <"$work/index"

if [ "$failed" -ne 0 ]; then
  echo "compile-fail: FAILED" >&2
  exit 1
fi
echo "compile-fail: ok"
