//! The ping protocol, defined a second time here and in `c/include/qnx_ipc_ping.h`.
//! Both sides assert the struct sizes as fixed numbers, so a change that alters
//! a size fails to build until that side's assertion is changed too. A change
//! that keeps the sizes (reordered or retyped fields) is not caught.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{Header, Message};

/// The name the C programs attach and open (`PING_NAME`).
pub const PING_NAME: &str = "qnx-ipc-ping";

/// `struct ping_msg` minus its header (`PING_KIND` 0x201).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct Ping {
    pub n: u32,
}

impl Message for Ping {
    const KIND: u16 = 0x201;
}

/// `struct pong`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct Pong {
    pub n: u32,
}

// `sizeof(struct ping_msg) == 8` and `sizeof(struct pong) == 4` in the C header.
const _: () = assert!(size_of::<Header>() + size_of::<Ping>() == 8);
const _: () = assert!(size_of::<Pong>() == 4);
