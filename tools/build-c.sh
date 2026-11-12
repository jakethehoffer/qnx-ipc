#!/usr/bin/env bash
# Build C programs for the QNX target with qcc.
#
#   tools/build-c.sh                 build every c/*.c into $CARGO_TARGET_DIR/c/
#   tools/build-c.sh <src.c> <out>   build one program
set -euo pipefail
: "${QNX_HOST:?run 'source tools/env.sh' first}"
root="$(cd "$(dirname "$0")/.." && pwd)"

build() {
  qcc -Vgcc_ntox86_64 -Wall -Wextra -Werror -O2 -I "$root/c/include" -o "$2" "$1"
}

if [ $# -eq 2 ]; then
  build "$1" "$2"
  exit 0
fi

mkdir -p "$CARGO_TARGET_DIR/c"
for src in "$root"/c/*.c; do
  build "$src" "$CARGO_TARGET_DIR/c/$(basename "${src%.c}")"
done
echo "built $(ls "$root"/c/*.c | wc -l) C program(s) into $CARGO_TARGET_DIR/c"
