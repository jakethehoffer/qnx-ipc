//! Timers that deliver pulses.

use core::ffi::{c_long, c_short};
use core::marker::PhantomData;
use core::time::Duration;

use qnx_ipc_sys as sys;

use crate::connection::thread_priority;
use crate::pulse::PulseCode;
use crate::{Connection, Error, Result};

/// A kernel timer that sends a pulse on a connection at a fixed period. Dropping
/// it deletes the timer.
///
/// The kernel timer holds only the connection's coid number. Connection ids are
/// reused: after a [`Connection`] is detached, a later `ConnectAttach` in this
/// process can get the same coid for a different channel, and a timer still
/// running would then send its pulses there. So the timer borrows its
/// connection (`'c`) and cannot outlive it:
///
/// ```compile_fail
/// use std::time::Duration;
/// use qnx_ipc::{Connection, PulseCode, PulseTimer};
///
/// let conn = Connection::to_local(1).unwrap();
/// let timer =
///     PulseTimer::periodic(&conn, PulseCode::user(9).unwrap(), Duration::from_millis(100))
///         .unwrap();
/// drop(conn); // error[E0505]: cannot move out of `conn` because it is borrowed
/// drop(timer);
/// ```
#[derive(Debug)]
pub struct PulseTimer<'c> {
    id: sys::timer_t,
    /// Ties the timer to the connection whose coid it sends on.
    _conn: PhantomData<&'c Connection>,
}

impl<'c> PulseTimer<'c> {
    /// Starts a `CLOCK_MONOTONIC` timer that sends a pulse with `code` (value 0)
    /// on `conn` every `period`, the first one `period` from now. The pulses carry
    /// the calling thread's priority.
    ///
    /// A zero `period` fails with `Error::Os(EINVAL)`.
    pub fn periodic(
        conn: &'c Connection,
        code: PulseCode,
        period: Duration,
    ) -> Result<PulseTimer<'c>> {
        if period.is_zero() {
            return Err(Error::Os(sys::EINVAL as i32));
        }
        let priority =
            c_short::try_from(thread_priority()?).map_err(|_| Error::Os(sys::EINVAL as i32))?;
        let mut event = sys::sigevent::default();
        sys::SIGEV_PULSE_INIT(
            &mut event,
            conn.coid(),
            priority,
            c_short::from(code.get()),
            core::ptr::null_mut(),
        );

        let mut id: sys::timer_t = 0;
        // SAFETY: `event` is a fully initialized sigevent and `id` a live local the
        // kernel writes the new timer id into; both outlive the call.
        let rc = unsafe {
            sys::timer_create(sys::CLOCK_MONOTONIC as sys::clockid_t, &mut event, &mut id)
        };
        if rc == -1 {
            return Err(Error::last_os());
        }
        // From here on, dropping `timer` deletes the kernel timer.
        let timer = PulseTimer {
            id,
            _conn: PhantomData,
        };

        let spec = timespec(period)?;
        let value = sys::itimerspec {
            it_value: spec,
            it_interval: spec,
        };
        // SAFETY: `value` is a live, initialized itimerspec the kernel only reads;
        // the old-value pointer is null (not wanted). `timer.id` is a live timer.
        let rc = unsafe { sys::timer_settime(timer.id, 0, &value, core::ptr::null_mut()) };
        if rc == -1 {
            return Err(Error::last_os());
        }
        Ok(timer)
    }
}

impl Drop for PulseTimer<'_> {
    fn drop(&mut self) {
        // SAFETY: `id` came from a successful timer_create and is deleted only
        // here, once. A failure cannot be acted on in drop, so it is ignored.
        unsafe {
            sys::timer_delete(self.id);
        }
    }
}

/// `d` as a `timespec`, or `EINVAL` if the seconds do not fit.
fn timespec(d: Duration) -> Result<sys::timespec> {
    Ok(sys::timespec {
        tv_sec: sys::_Timet::try_from(d.as_secs()).map_err(|_| Error::Os(sys::EINVAL as i32))?,
        tv_nsec: c_long::from(d.subsec_nanos()),
    })
}
