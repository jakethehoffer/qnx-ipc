//! Safe Rust for QNX 8.0 native message passing, pulses and named channels.

mod channel;
mod connection;
mod error;
mod message;
mod pulse;
mod request;

pub use channel::{Channel, ChannelFlags, Received, RecvBuf};
pub use connection::Connection;
pub use error::{Error, Result};
pub use message::{Header, Message};
pub use pulse::Pulse;
pub use request::{Request, SenderInfo};
