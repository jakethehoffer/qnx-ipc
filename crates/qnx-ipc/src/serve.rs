//! The serve loop: receive on a channel and pass each message or pulse to a
//! [`Handler`].

use std::ops::ControlFlow;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::pulse::Pulse;
use crate::request::Request;
use crate::{Channel, Error, Received, RecvBuf, Result};

/// What a server does with each message and pulse that [`Channel::serve`]
/// receives.
///
/// Returning `Ok(ControlFlow::Break(()))` stops the loop. An `Err` is ignored and
/// the loop continues: a request the handler did not answer has already been
/// answered `ENOSYS` when it was dropped.
pub trait Handler {
    /// Handles one message. Answer it with [`Request::reply`] or
    /// [`Request::reply_error`]; a request dropped unanswered replies `ENOSYS`,
    /// and one dropped by a panic replies `EIO`.
    fn on_message(&mut self, req: Request<'_>) -> Result<ControlFlow<()>>;

    /// Handles one pulse. The default ignores it and continues.
    fn on_pulse(&mut self, pulse: Pulse) -> Result<ControlFlow<()>> {
        let _ = pulse;
        Ok(ControlFlow::Continue(()))
    }
}

impl Channel {
    /// Receives messages and pulses into `buf` and passes each to `handler`
    /// until a handler call returns `Ok(ControlFlow::Break(()))`, then returns
    /// `Ok(())`.
    ///
    /// Each handler call runs inside `catch_unwind`. If `on_message` panics, its
    /// client gets `Err(Error::Os(EIO))` and the loop goes on; a panic in
    /// `on_pulse` is caught the same way. The panic message is still printed by
    /// the panic hook. Handler errors are ignored. The loop returns `Err` only
    /// when receiving fails (an interrupted receive, `EINTR`, is retried).
    pub fn serve<H: Handler>(&self, buf: &mut RecvBuf, handler: &mut H) -> Result<()> {
        loop {
            let received = match self.receive(buf) {
                Ok(received) => received,
                Err(Error::Os(errno)) if errno == qnx_ipc_sys::EINTR as i32 => continue,
                Err(e) => return Err(e),
            };
            // An unwinding `on_message` drops its `Request` during the unwind,
            // which answers the client `EIO` (see `Request`'s `Drop`), exactly once.
            let outcome = match received {
                Received::Message(req) => {
                    catch_unwind(AssertUnwindSafe(|| handler.on_message(req)))
                }
                Received::Pulse(pulse) => {
                    catch_unwind(AssertUnwindSafe(|| handler.on_pulse(pulse)))
                }
            };
            if let Ok(Ok(ControlFlow::Break(()))) = outcome {
                return Ok(());
            }
        }
    }
}
