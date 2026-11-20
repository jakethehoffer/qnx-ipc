//! Kernel timeouts for one blocking call (`TimerTimeout`).

use core::time::Duration;

use qnx_ipc_sys as sys;

use crate::{Error, Result};

/// The blocked states a timed send covers: waiting to be received
/// (`_NTO_TIMEOUT_SEND`) and waiting for the reply (`_NTO_TIMEOUT_REPLY`).
pub(crate) const SEND: u32 = (1 << sys::STATE_SEND) | (1 << sys::STATE_REPLY);

/// The blocked state a timed receive covers (`_NTO_TIMEOUT_RECEIVE`).
pub(crate) const RECEIVE: u32 = 1 << sys::STATE_RECEIVE;

/// Arms a kernel timeout of `timeout` (relative, `CLOCK_MONOTONIC`) on the
/// calling thread for the next kernel call that blocks in one of `states`. The
/// kernel clears it when that next kernel call returns, so the caller must make
/// the blocking call right after this, with no other kernel call in between.
///
/// A timeout too long for a `u64` of nanoseconds (over 584 years) is cut to
/// that. A zero timeout makes the call fail with `ETIMEDOUT` at once if it
/// would block.
pub(crate) fn arm(states: u32, timeout: Duration) -> Result<()> {
    let ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
    // SAFETY: a null event means "unblock the thread" (SIGEV_UNBLOCK). `ns` is a
    // live local the kernel only reads, and the old-time pointer is null (not
    // wanted).
    let rc = unsafe {
        sys::TimerTimeout(
            sys::CLOCK_MONOTONIC as sys::clockid_t,
            states,
            core::ptr::null(),
            &ns,
            core::ptr::null_mut(),
        )
    };
    if rc == -1 {
        return Err(Error::last_os());
    }
    Ok(())
}
