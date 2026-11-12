//! Generates the raw QNX bindings from the QNX SDP 8.0 headers into `OUT_DIR`.
//!
//! The bindings always come from the QNX headers, also when cargo builds for the
//! host (that is how `tools/lint.sh` type-checks the workspace on Linux). The clang
//! target follows cargo's target architecture: aarch64 when building for aarch64,
//! x86_64 otherwise.

use std::env;
use std::path::PathBuf;

/// Functions bound from the QNX headers. Bindgen also pulls in every type they use.
const FUNCTIONS: &[&str] = &[
    "ChannelCreate",
    "ChannelDestroy",
    "ConnectAttach",
    "ConnectDetach",
    "MsgSendv",
    "MsgReceive",
    "MsgReply",
    "MsgReplyv",
    "MsgError",
    "MsgRead",
    "MsgWrite",
    "MsgSendPulse",
    "MsgInfo",
    "name_attach",
    "name_detach",
    "name_open",
    "name_close",
    "timer_create",
    "timer_settime",
    "timer_delete",
    "dispatch_create",
    "dispatch_destroy",
    "dispatch_context_alloc",
    "dispatch_context_free",
    "dispatch_block",
    "dispatch_handler",
    "resmgr_attach",
    "resmgr_detach",
    "iofunc_func_init",
    "iofunc_attr_init",
    "iofunc_read_verify",
    "iofunc_write_verify",
    "iofunc_devctl_default",
    "resmgr_msgreply",
    "TimerTimeout",
];

/// Types bound even when no listed function names them directly.
const TYPES: &[&str] = &[
    "_pulse",
    "_msg_info",
    "_name_attach",
    "sigevent",
    "itimerspec",
    "iov_t",
    "resmgr_context_t",
    "resmgr_connect_funcs_t",
    "resmgr_io_funcs_t",
    "resmgr_attr_t",
    "iofunc_attr_t",
    "io_read_t",
    "io_write_t",
    "io_devctl_t",
    "io_open_t",
    // C enums holding `_IO_BASE`, `_IO_MAX` and `_IO_CONNECT` (from `<sys/iomsg.h>`).
    "_msg_bases",
    "_io__Uint16types",
];

/// Constants (object-like macros and enum values) bound by name pattern.
const VARS: &[&str] = &[
    "_IO_CONNECT",
    "_IO_MAX",
    "_IO_BASE",
    "_PULSE_CODE_.*",
    "_NTO_CHF_.*",
    "_NTO_COF_.*",
    "_NTO_SIDE_CHANNEL",
    "SIGEV_PULSE",
    "_RESMGR_NOREPLY",
    // errno values (`EOK`, `ENOSYS`, `EINVAL`, ...) from `<errno.h>`.
    "E[A-Z0-9]+",
    // Thread states from `<sys/states.h>`: `TimerTimeout` flags are `1 << STATE_*`
    // (the `_NTO_TIMEOUT_*` macros are function-like, so bindgen skips them).
    "STATE_[A-Z_]+",
    "CLOCK_MONOTONIC",
    "CLOCK_REALTIME",
];

/// Macros that `qcc` predefines for QNX 8.0 and that the QNX headers test, but that
/// clang 18 does not define for an nto target (found by diffing
/// `qcc -Vgcc_ntox86_64 -dM -E - </dev/null` against clang's list). Both supported
/// architectures are little-endian.
const DEFINES: &[&str] = &[
    "-D__QNX__=800",
    "-D__QNXNTO__=1",
    "-D__unix__=1",
    "-D__unix=1",
    "-D__LITTLEENDIAN__=1",
];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=QNX_TARGET");

    let qnx_target = env::var("QNX_TARGET").unwrap_or_else(|_| {
        panic!(
            "QNX_TARGET is not set. qnx-ipc-sys generates its bindings from the QNX SDP 8.0 \
             headers: run `source tools/env.sh` (which sources qnxsdp-env.sh) before building."
        )
    });
    let include = PathBuf::from(&qnx_target).join("usr/include");
    if !include.is_dir() {
        panic!(
            "QNX_TARGET={qnx_target} has no usr/include directory. Point QNX_TARGET at the \
             SDP 8.0 target tree (for example ~/qnx800/target/qnx)."
        );
    }

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // The QNX headers test `__aarch64__` (which clang defines) on ARM, but `__X86_64__`
    // (which only qcc defines) on x86_64.
    let (clang_target, arch_defines): (&str, &[&str]) = if arch == "aarch64" {
        ("aarch64-unknown-nto-qnx800", &[])
    } else {
        ("x86_64-pc-nto-qnx800", &["-D__X86_64__=1"])
    };

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("--target={clang_target}"))
        .clang_arg(format!("--sysroot={qnx_target}"))
        .clang_arg(format!("-I{}", include.display()))
        .clang_args(arch_defines.iter().copied())
        .clang_args(DEFINES.iter().copied())
        .layout_tests(false)
        // Keep C enum constant names as they are (`_IO_CONNECT`, not `_io__Uint16types__IO_CONNECT`).
        .prepend_enum_name(false)
        .derive_default(true)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for f in FUNCTIONS {
        builder = builder.allowlist_function(f);
    }
    for t in TYPES {
        builder = builder.allowlist_type(t);
    }
    for v in VARS {
        builder = builder.allowlist_var(v);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate the QNX bindings from wrapper.h");
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("bindings.rs");
    bindings
        .write_to_file(&out)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
}
