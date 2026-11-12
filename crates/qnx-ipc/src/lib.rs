//! Safe Rust for QNX 8.0 native message passing, pulses and named channels.
//!
//! QNX processes talk by synchronous message passing: a client sends a message
//! and stays blocked until the server replies. This crate wraps that kernel API
//! (`ChannelCreate`, `ConnectAttach`, `MsgSendv`, `MsgReceive`, `MsgReply`,
//! `MsgError`, `MsgRead`, `MsgSendPulse`, `name_attach`, `name_open`, and timers
//! that deliver pulses) in owned types:
//!
//! - [`Channel`] and [`NamedChannel`] receive; [`Connection`] sends.
//! - A [`Message`] is a plain-data (`bytemuck::Pod`) type with a `KIND`; it goes
//!   on the wire as a 4-byte [`Header`] plus its bytes, so C code can speak the
//!   same protocol.
//! - A received [`Request`] is answered at most once, by
//!   [`reply`](Request::reply) or [`reply_error`](Request::reply_error). If it
//!   is dropped unanswered it replies `ENOSYS` (`EIO` during a panic): a
//!   request that is dropped, or whose handler panics, still answers its
//!   client (the limits, such as a request leaked with `mem::forget`, are
//!   listed under "Known limits" in `docs/architecture.md`). A request borrows
//!   the channel it came from, so it cannot outlive it.
//! - [`Channel::serve`] runs a [`Handler`] over every message and pulse, with
//!   each call inside `catch_unwind`.
//! - [`Pulse`]s are small non-blocking notifications; a [`PulseTimer`] sends one
//!   periodically and borrows its connection, so it cannot outlive it.
//!
//! Every kernel object is released when its owner drops. The safety argument
//! (each `unsafe` block and why the API around it is sound) is in
//! `docs/architecture.md` in the repository.
//!
//! The server thread runs at the priority of the client it is serving (QNX
//! priority inheritance) unless the channel has
//! [`ChannelFlags::FIXED_PRIORITY`].
//!
//! # Example
//!
//! A server thread and a client in one process (`examples/ping_pong.rs`):
//!
#![doc = concat!("```no_run\n", include_str!("../examples/ping_pong.rs"), "```")]
#![deny(missing_docs)]

mod channel;
mod connection;
mod error;
mod message;
mod name;
mod pulse;
mod request;
mod serve;
mod timer;

pub use channel::{Channel, ChannelFlags, Received, RecvBuf};
pub use connection::Connection;
pub use error::{Error, Result};
pub use message::{Header, Message};
pub use name::NamedChannel;
pub use pulse::{Pulse, PulseCode, PulseKind};
pub use request::{Request, SenderInfo};
pub use serve::Handler;
pub use timer::PulseTimer;
