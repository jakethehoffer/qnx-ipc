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
    /// The pulse value (`sigval.sival_int`).
    pub value: i32,
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
}

/// The meaning of a pulse code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PulseKind {
    /// A user pulse with this code (`0..=127`).
    User(u8),
    /// All connections from a client process went away, for example because it
    /// exited (`_PULSE_CODE_DISCONNECT`, needs [`ChannelFlags::DISCONNECT`]).
    /// [`Channel::receive`] has already released the kernel's server connection.
    ///
    /// [`ChannelFlags::DISCONNECT`]: crate::ChannelFlags::DISCONNECT
    /// [`Channel::receive`]: crate::Channel::receive
    Disconnect,
    /// A client blocked on this channel was signalled or timed out
    /// (`_PULSE_CODE_UNBLOCK`, needs [`ChannelFlags::UNBLOCK`]). The value is the
    /// client's receive id.
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
