//! Pulses: small, non-blocking notifications delivered on a channel.

use qnx_ipc_sys as sys;

use crate::{Error, Result};

/// A pulse code open to user pulses: `0..=127`. Negative codes are reserved for
/// the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PulseCode(u8);

impl PulseCode {
    /// A user pulse code. Fails with [`Error::InvalidPulseCode`] unless `code` is
    /// in `0..=127` (`_PULSE_CODE_MINAVAIL..=_PULSE_CODE_MAXAVAIL`).
    pub fn user(code: u8) -> Result<PulseCode> {
        if u32::from(code) > sys::_PULSE_CODE_MAXAVAIL {
            return Err(Error::InvalidPulseCode(i32::from(code)));
        }
        Ok(PulseCode(code))
    }

    /// The code as a number in `0..=127`.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// A pulse received on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    /// The pulse code. User pulses use `0..=127`; negative codes come from the
    /// kernel (for example `_PULSE_CODE_DISCONNECT`).
    pub code: i8,
    /// The pulse value: the `sival_int` member of the pulse's `sigval`, that
    /// is the low 32 bits of [`value_long`](Pulse::value_long).
    pub value: i32,
    /// The whole pulse value: the `sival_long` member of the pulse's `sigval`
    /// (64 bits on QNX 8.0). An [`Unblock`](PulseKind::Unblock) pulse carries
    /// a receive id here; read it with [`rcvid`](Pulse::rcvid).
    pub value_long: i64,
    /// The server connection id of the pulse's sender, as in
    /// [`SenderInfo::scoid`](crate::SenderInfo::scoid). On a
    /// [`Disconnect`](PulseKind::Disconnect) pulse it is the departed
    /// client's.
    pub scoid: i32,
}

impl Pulse {
    /// What the pulse's code means.
    pub fn kind(&self) -> PulseKind {
        if self.code >= 0 {
            return PulseKind::User(self.code as u8);
        }
        match i32::from(self.code) {
            sys::_PULSE_CODE_DISCONNECT => PulseKind::Disconnect,
            sys::_PULSE_CODE_UNBLOCK => PulseKind::Unblock,
            sys::_PULSE_CODE_THREADDEATH => PulseKind::ThreadDeath,
            sys::_PULSE_CODE_COIDDEATH => PulseKind::CoidDeath,
            _ => PulseKind::Other(self.code),
        }
    }

    /// On an [`Unblock`](PulseKind::Unblock) pulse, the receive id of the
    /// request whose client wants to stop waiting: compare it with
    /// [`Request::rcvid`](crate::Request::rcvid) of the requests the server
    /// holds and answer the one that matches. `None` on any other pulse.
    pub fn rcvid(&self) -> Option<ReceiveId> {
        (self.kind() == PulseKind::Unblock).then_some(ReceiveId(self.value_long))
    }
}

/// The kernel's id for one received message (`rcvid_t`), for telling the
/// requests a server holds apart: [`Request::rcvid`](crate::Request::rcvid)
/// and [`Pulse::rcvid`]. QNX reuses the number once the message is answered or
/// its client is gone, so compare it only with requests that are still held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReceiveId(pub(crate) i64);

impl ReceiveId {
    /// The id as the kernel's number.
    pub fn raw(self) -> i64 {
        self.0
    }
}

/// The meaning of a pulse code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PulseKind {
    /// A user pulse with this code (`0..=127`).
    User(u8),
    /// All connections from a client process went away, for example because it
    /// exited (`_PULSE_CODE_DISCONNECT`, needs [`ChannelFlags::DISCONNECT`]).
    /// [`Pulse::scoid`] says which client. [`Channel::receive`] releases the
    /// kernel's server connection: at once, or after the last request still
    /// held from that client.
    ///
    /// [`ChannelFlags::DISCONNECT`]: crate::ChannelFlags::DISCONNECT
    /// [`Channel::receive`]: crate::Channel::receive
    Disconnect,
    /// A client blocked on this channel was signalled or timed out
    /// (`_PULSE_CODE_UNBLOCK`, needs [`ChannelFlags::UNBLOCK`]). The kernel sends
    /// the receive id of the client's request as the value; [`Pulse::rcvid`]
    /// returns it.
    ///
    /// [`ChannelFlags::UNBLOCK`]: crate::ChannelFlags::UNBLOCK
    Unblock,
    /// A thread of this process died (`_PULSE_CODE_THREADDEATH`). The value is
    /// its thread id.
    ThreadDeath,
    /// A server this process was connected to went away
    /// (`_PULSE_CODE_COIDDEATH`). The value is the connection id.
    CoidDeath,
    /// Any other kernel pulse code (negative).
    Other(i8),
}
