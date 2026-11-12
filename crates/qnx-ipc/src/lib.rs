//! Safe Rust for QNX 8.0 native message passing, pulses and named channels.

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
