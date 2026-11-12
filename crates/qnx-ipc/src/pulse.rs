//! Pulses: small, non-blocking notifications delivered on a channel.

/// A pulse received on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    /// The pulse code. User pulses use `0..=127`; negative codes come from the
    /// kernel (for example `_PULSE_CODE_DISCONNECT`).
    pub code: i8,
    /// The pulse value (`sigval.sival_int`).
    pub value: i32,
}
