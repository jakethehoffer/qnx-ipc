//! Which clients of a channel still have requests held, so that a departed
//! client's server connection is released only after the last of them.

use std::sync::{Mutex, MutexGuard};

use qnx_ipc_sys as sys;

/// The server connections (scoids) of one channel that have requests the
/// server has received and not yet let go of.
///
/// QNX keeps a departed client's scoid allocated until the server detaches it,
/// and gives the number to the next client after that. A receive id contains
/// the scoid, so detaching while a [`Request`](crate::Request) of the departed
/// client is still held would let that request's receive id come to name a
/// request of the next client. This type delays the detach until the last
/// such request is gone.
///
/// Only a channel that gets disconnect pulses (`_NTO_CHF_DISCONNECT`) keeps
/// count: without them nothing is ever detached, and receiving and answering
/// take no lock.
#[derive(Debug)]
pub(crate) struct Clients {
    track: bool,
    held: Mutex<Vec<Held>>,
}

/// One scoid with requests held.
#[derive(Debug)]
struct Held {
    scoid: i32,
    /// Requests received from it and not dropped yet. Never 0 in the list.
    requests: u32,
    /// Its disconnect pulse has arrived: detach when `requests` reaches 0.
    departed: bool,
}

impl Clients {
    /// `track` says whether the channel has `_NTO_CHF_DISCONNECT`.
    pub(crate) fn new(track: bool) -> Clients {
        Clients {
            track,
            held: Mutex::new(Vec::new()),
        }
    }

    /// The list. A panic cannot leave it half-updated (no user code runs under
    /// the lock), so a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Vec<Held>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A request from `scoid` was received.
    pub(crate) fn hold(&self, scoid: i32) {
        if !self.track {
            return;
        }
        let mut held = self.lock();
        match held.iter_mut().find(|h| h.scoid == scoid) {
            Some(h) => h.requests += 1,
            None => held.push(Held {
                scoid,
                requests: 1,
                departed: false,
            }),
        }
    }

    /// A request from `scoid` was dropped, answered or not. Detaches the scoid
    /// if its client has departed and this was its last held request.
    pub(crate) fn release(&self, scoid: i32) {
        if !self.track {
            return;
        }
        let mut held = self.lock();
        let Some(i) = held.iter().position(|h| h.scoid == scoid) else {
            return;
        };
        held[i].requests -= 1;
        if held[i].requests == 0 {
            if held[i].departed {
                detach(scoid);
            }
            held.swap_remove(i);
        }
    }

    /// The disconnect pulse of `scoid` arrived. Detaches it now if no request
    /// from it is held, else when the last one is dropped.
    pub(crate) fn departed(&self, scoid: i32) {
        let mut held = self.lock();
        match held.iter_mut().find(|h| h.scoid == scoid) {
            Some(h) => h.departed = true,
            None => detach(scoid),
        }
    }
}

/// Releases the kernel's server connection `scoid` to a departed client.
fn detach(scoid: i32) {
    // SAFETY: `scoid` came from a disconnect pulse, so it is the kernel's server
    // connection to a departed client, which stays allocated until the server
    // detaches it, and this is its one detach: `departed` runs once per
    // disconnect pulse and either detaches or leaves it to the one `release`
    // that finds the count at 0. ConnectDetach takes only an integer; a failure
    // leaves nothing to clean up and is ignored.
    unsafe {
        sys::ConnectDetach(scoid);
    }
}
