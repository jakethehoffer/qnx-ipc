//! The vehicle-signals demo protocol: the Rust copy of `c/include/vehicle.h`.
//!
//! Both copies assert the struct sizes as fixed numbers, so a change that
//! alters a size fails to build until that side's assertion is changed too. A
//! change that keeps the sizes (reordered or retyped fields) is not caught. The
//! safety client includes this file with `#[path]`.

// Every program that includes this file uses only part of it.
#![allow(dead_code)]

use qnx_ipc::{Header, Message};

/// The name `vsigd` registers (`VEHICLE_NAME`).
pub const VEHICLE_NAME: &str = "vehicle";
/// The name the watchdog registers (`WATCHDOG_NAME`).
pub const WATCHDOG_NAME: &str = "watchdog";

/// Signal id of the vehicle speed, km/h (`VEHICLE_SIGNAL_SPEED`).
pub const SIGNAL_SPEED: u32 = 1;
/// Signal id of the brake pressure, kPa (`VEHICLE_SIGNAL_BRAKE`).
pub const SIGNAL_BRAKE: u32 = 2;

/// Heartbeat pulse code; the pulse value is the client id
/// (`WATCHDOG_HEARTBEAT_CODE`).
pub const HEARTBEAT_CODE: u8 = 10;

/// Client id of the safety client (`CLIENT_ID_SAFETY`).
pub const CLIENT_ID_SAFETY: i32 = 1;
/// Client id of the infotainment client (`CLIENT_ID_INFOTAINMENT`).
pub const CLIENT_ID_INFOTAINMENT: i32 = 2;

/// Length of the NUL-padded name in [`Hello`] (`VEHICLE_NAME_LEN`).
pub const NAME_LEN: usize = 32;

/// Sent once per connection to say who the client is (`struct vehicle_hello`).
/// Answered with an empty reply.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Hello {
    /// The client's name, NUL-padded (need not be NUL-terminated).
    pub name: [u8; NAME_LEN],
}

impl Message for Hello {
    const KIND: u16 = 0x211;
}

impl Hello {
    /// A hello carrying `name`, cut to [`NAME_LEN`] bytes.
    pub fn new(name: &str) -> Hello {
        let mut hello = Hello {
            name: [0; NAME_LEN],
        };
        let len = name.len().min(NAME_LEN);
        hello.name[..len].copy_from_slice(&name.as_bytes()[..len]);
        hello
    }

    /// The name up to the first NUL, with anything not printable replaced.
    pub fn name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        String::from_utf8_lossy(&self.name[..end])
            .chars()
            .map(|c| if c.is_ascii_graphic() { c } else { '?' })
            .collect()
    }
}

/// Asks for the current value of one signal (`struct vehicle_get_signal`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GetSignal {
    /// [`SIGNAL_SPEED`] or [`SIGNAL_BRAKE`].
    pub signal_id: u32,
}

impl Message for GetSignal {
    const KIND: u16 = 0x210;
}

/// The reply to [`GetSignal`]: the value in thousandths of its unit
/// (`struct vehicle_signal`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SignalValue {
    /// The value times 1000.
    pub value_milli: i64,
}

// The same sizes as the `_Static_assert`s in `c/include/vehicle.h`.
const _: () = assert!(core::mem::size_of::<Header>() == 4);
const _: () = assert!(core::mem::size_of::<Header>() + core::mem::size_of::<Hello>() == 36);
const _: () = assert!(core::mem::size_of::<Header>() + core::mem::size_of::<GetSignal>() == 8);
const _: () = assert!(core::mem::size_of::<SignalValue>() == 8);
