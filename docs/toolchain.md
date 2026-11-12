# Toolchain and target setup

Everything here was set up and checked on 2026-11-11 on Windows 11 with WSL2
Ubuntu 24.04. Any Linux x86_64 host with KVM should work the same way.

## Versions

| Piece | Version |
| --- | --- |
| QNX SDP | 8.0 (`com.qnx.qnx800` 8.0.0.00141), free QNX Everywhere non-commercial license |
| QNX OS on the target | `QNX qnx-ipc 8.0.0 2026/06/05 x86pc x86_64` |
| Rust compiler | QNX's host Rust package `com.qnx.qnx800.host.linux.x86_64.rust` 0.0.1.00014: `rustc 1.85.1-dev` |
| Rust target | `x86_64-pc-nto-qnx800` (std is prebuilt by QNX, also `aarch64-unknown-nto-qnx800`) |
| Cargo | rustup toolchain `1.85.1` (QNX's package ships `rustc` but no `cargo`) |
| C compiler | `qcc -Vgcc_ntox86_64` from the SDP |
| Emulator | QEMU 8.2.2 with KVM |

## One-time setup

1. Get the free license and install QNX Software Center, then SDP 8.0 and the Rust
   package into `~/qnx800`. Package ids: `com.qnx.qnx800` and
   `com.qnx.qnx800.host.linux.x86_64.rust`.
2. Install rustup's cargo of the same version as QNX's rustc:
   `rustup toolchain install 1.85.1 --profile minimal`.
3. Install QEMU: `sudo apt install qemu-system-x86 qemu-utils` and add yourself to
   the `kvm` group.
4. Install clang and libclang for bindgen: `sudo apt install clang libclang-dev`
   (Ubuntu 24.04 gives clang 18.1.3, `/usr/lib/llvm-18/lib/libclang.so`).
5. For `tools/lint.sh`: `rustup component add rustfmt clippy --toolchain 1.85.1`.

## Generated bindings

`crates/qnx-ipc-sys/build.rs` runs bindgen 0.72 over `crates/qnx-ipc-sys/wrapper.h`
and writes the result to cargo's `OUT_DIR` (never into the repo). It always reads the
QNX headers in `$QNX_TARGET/usr/include`, also when cargo builds for the Linux host,
and fails with a clear message if `QNX_TARGET` is unset. clang is called with
`--target=x86_64-pc-nto-qnx800` (`aarch64-unknown-nto-qnx800` when building for
aarch64), `--sysroot=$QNX_TARGET`, and the macros that `qcc` predefines but clang 18
does not: `__QNX__=800`, `__QNXNTO__`, `__unix__`, `__unix`, `__LITTLEENDIAN__` and,
on x86_64, `__X86_64__`. Without `__LITTLEENDIAN__` the headers stop with
`Endian not defined`.

## Daily use

```sh
source tools/env.sh      # SDP env, QNX rustc, target triple, runner, paths
tools/qemu-up.sh         # build the image once, boot it headless, wait for SSH
cargo run -p qnx-ipc --example hello --target $QNX_RUST_TARGET   # runs on QNX
tools/test-all.sh        # lint, docs, all tests on QNX, demo check -> docs/test-log.txt
tools/qemu-down.sh       # stop the target
```

`tools/env.sh` sets `RUSTC` to QNX's `rustc`, so cargo builds both host build
scripts and target code with it. It also sets
`CARGO_TARGET_X86_64_PC_NTO_QNX800_RUNNER` to `tools/qnx-run`, which copies each
binary to the target over SSH and runs it there, so `cargo run` and `cargo test`
for the QNX target execute on QNX and return the program's real exit code.

## The QEMU target

`tools/qemu-up.sh` builds a minimal image with `mkqnximage --type=qemu --arch=x86_64`
(1536 MB RAM, 2 CPUs, no graphics, root SSH login with a dedicated key at
`~/.ssh/qnx_target_ed25519`) and boots it with QEMU directly instead of through
`mkqnximage --run`:

- **Networking.** `mkqnximage --run` needs a host bridge (`virbr0` from libvirt or
  `br0`) and a setuid `qemu-bridge-helper`. We use QEMU user-mode networking with
  the guest's SSH port forwarded to `127.0.0.1:2222` instead, so no bridge, libvirt or
  sudo is needed and nothing changes on the host network.
- **SSH fix.** The SDP 8.0 OpenSSH `sshd` looks for its `sshd-auth` helper at
  `/usr/libexec/sshd-auth`, but the default `mkqnximage` image only includes
  `sshd-session`, so `sshd` exits at boot with
  `/usr/libexec/sshd-auth does not exist or is not executable`. `qemu-up.sh` adds the
  helper to the boot image at that absolute path through
  `local/snippets/ifs_files.custom`. (A relative path in that snippet lands under
  `/proc/boot`, which does not fix it.)

## Known limits

- QNX's Rust package has no `clippy` or `rustfmt`. `tools/lint.sh` runs both from the
  rustup toolchain with `RUSTC` unset, and runs clippy for the host target
  `x86_64-unknown-linux-gnu`. That works because the bindings come from the QNX
  headers whatever the target, so the crates type-check on Linux. Tests run only on
  the QNX target.
