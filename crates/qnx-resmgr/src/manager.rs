//! Attaching a [`Device`] at a path and running its dispatch loop.

use core::ffi::c_int;
use core::mem::size_of;
use core::ptr;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use qnx_ipc::{Error, Result};
use qnx_ipc_sys as sys;

use crate::trampoline::{self, DeviceAttr};
use crate::Device;

const EINTR: c_int = sys::EINTR as c_int;

/// A path to attach a [`Device`] at, with its permissions.
///
/// Build it with [`new`](ResourceManager::new), optionally set
/// [`mode`](ResourceManager::mode), then [`run`](ResourceManager::run) the
/// device on the calling thread or [`spawn`](ResourceManager::spawn) it on a
/// new one. The path appears as a character device.
///
/// The process that serves the path can open it as well. A [`Device`] method
/// must not do I/O on its own path, though: the dispatch thread would wait for
/// itself.
#[derive(Debug, Clone)]
pub struct ResourceManager {
    path: CString,
    mode: u32,
}

impl ResourceManager {
    /// A resource manager for the absolute path `path` (for example
    /// `/dev/hello`), with mode `0o666`.
    ///
    /// Fails with [`Error::InvalidName`] if `path` is not absolute or contains
    /// a NUL byte. Nothing is attached until [`run`](ResourceManager::run) or
    /// [`spawn`](ResourceManager::spawn).
    pub fn new(path: &str) -> Result<ResourceManager> {
        if !path.starts_with('/') {
            return Err(Error::InvalidName(path.into()));
        }
        let path = CString::new(path).map_err(|_| Error::InvalidName(path.into()))?;
        Ok(ResourceManager { path, mode: 0o666 })
    }

    /// Sets the permission bits of the path (only the low 12 bits, `0o7777`,
    /// are used). The default is `0o666`.
    pub fn mode(self, mode: u32) -> ResourceManager {
        ResourceManager {
            mode: mode & 0o7777,
            ..self
        }
    }

    /// Attaches `device` and serves it on the calling thread. Blocks; returns
    /// only if receiving fails, after detaching the path.
    ///
    /// Fails with the `errno` of the QNX call if attaching fails (for example
    /// `EPERM` when the process may not attach paths).
    pub fn run<D: Device>(self, device: D) -> Result<()> {
        Attached::new(&self, device)?.serve(None)
    }

    /// Attaches `device` and serves it on a new thread. The path exists when
    /// this returns; it is detached when the returned [`RunningManager`] drops.
    ///
    /// Fails with the `errno` of the QNX call if attaching fails.
    pub fn spawn<D: Device>(self, device: D) -> Result<RunningManager> {
        let attached = Attached::new(&self, device)?;
        // Our own connection to the dispatch channel for the stop pulse. It is
        // owned by the `RunningManager`, so the coid stays ours (and cannot be
        // reused for another channel) until after the thread is joined.
        // SAFETY: `attached.dpp` is a live dispatch handle.
        let coid =
            unsafe { sys::message_connect(attached.dpp, sys::MSG_FLAG_SIDE_CHANNEL as c_int) };
        if coid == -1 {
            return Err(Error::last_os());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let code = attached.stop_code;
        let thread_stop = Arc::clone(&stop);
        let spawned = thread::Builder::new()
            .name("qnx-resmgr".into())
            .spawn(move || attached.serve(Some(&thread_stop)));
        match spawned {
            Ok(thread) => Ok(RunningManager {
                stop,
                coid,
                code,
                thread: Some(thread),
            }),
            Err(e) => {
                // SAFETY: `coid` is our connection and is not used again.
                unsafe { sys::ConnectDetach(coid) };
                Err(Error::Os(e.raw_os_error().unwrap_or(sys::EAGAIN as i32)))
            }
        }
    }
}

/// A resource manager serving on its own thread (from
/// [`ResourceManager::spawn`]).
///
/// Dropping it stops the dispatch loop, detaches the path (files still open
/// on it are closed and fail with `EBADF` from then on), joins the thread and
/// drops the device.
#[derive(Debug)]
pub struct RunningManager {
    stop: Arc<AtomicBool>,
    coid: c_int,
    code: c_int,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Drop for RunningManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the loop out of `dispatch_block`; it then sees the flag. If the
        // loop already ended (a receive error), the send fails harmlessly.
        // SAFETY: `coid` is our live connection; a pulse carries no pointers.
        // Priority -1 means the calling thread's priority.
        let sent = unsafe { sys::MsgSendPulse(self.coid, -1, self.code, 0) };
        if let Some(thread) = self.thread.take() {
            if sent != -1 || thread.is_finished() {
                // The thread's result is the loop's result; a drop has nowhere
                // to report it.
                let _ = thread.join();
            }
            // Otherwise the loop cannot be woken: leave the thread detached
            // rather than block this drop forever.
        }
        // SAFETY: our connection, not used again.
        unsafe { sys::ConnectDetach(self.coid) };
    }
}

/// Everything the framework points into for one attached path. Lives in a box
/// whose address stays fixed from before `resmgr_attach` until after
/// `dispatch_destroy` (see the soundness notes in `trampoline.rs`).
#[repr(C)]
struct Resources<D> {
    attr: DeviceAttr<D>,
    mount: sys::iofunc_mount_t,
    ocb_funcs: sys::iofunc_funcs_t,
    connect: sys::resmgr_connect_funcs_t,
    io: sys::resmgr_io_funcs_t,
    device: D,
}

/// An attached path and its dispatch handle. Dropping it detaches the path and
/// frees everything, in the order the framework needs.
struct Attached<D: Device> {
    dpp: *mut sys::dispatch_t,
    ctp: *mut sys::dispatch_context_t,
    res: *mut Resources<D>,
    /// `resmgr_attach` id, or -1.
    id: c_int,
    /// The pulse code `pulse_attach` allocated for the stop pulse, or -1.
    stop_code: c_int,
}

// SAFETY: the raw pointers are owned by this value and used by one thread at
// a time (the one that owns it); the device inside `res` is `Send + Sync`.
unsafe impl<D: Device> Send for Attached<D> {}

impl<D: Device> Attached<D> {
    /// Creates the dispatch handle, attaches `config.path` with `device` and
    /// prepares the dispatch context. Any failure undoes the earlier steps
    /// (through `Drop`) and returns the failing call's `errno`.
    fn new(config: &ResourceManager, device: D) -> Result<Attached<D>> {
        // SAFETY: no preconditions.
        let dpp = unsafe { sys::dispatch_create() };
        if dpp.is_null() {
            return Err(Error::last_os());
        }
        let res = Box::into_raw(Box::new(Resources {
            attr: DeviceAttr {
                attr: sys::iofunc_attr_t::default(),
                device: ptr::null(),
            },
            mount: sys::iofunc_mount_t::default(),
            ocb_funcs: sys::iofunc_funcs_t::default(),
            connect: sys::resmgr_connect_funcs_t::default(),
            io: sys::resmgr_io_funcs_t::default(),
            device,
        }));
        let mut attached = Attached {
            dpp,
            ctp: ptr::null_mut(),
            res,
            id: -1,
            stop_code: -1,
        };

        // SAFETY: `res` is the live box from `Box::into_raw`; every pointer the
        // framework keeps is derived from it and points into it (see
        // `trampoline.rs`). Only raw place expressions are used, so no Rust
        // reference to the box outlives a statement.
        unsafe {
            sys::iofunc_func_init(
                sys::_RESMGR_CONNECT_NFUNCS,
                ptr::addr_of_mut!((*res).connect),
                sys::_RESMGR_IO_NFUNCS,
                ptr::addr_of_mut!((*res).io),
            );
            (*res).io.read = Some(trampoline::io_read::<D>);
            (*res).io.write = Some(trampoline::io_write::<D>);
            (*res).io.devctl = Some(trampoline::io_devctl::<D>);

            // Our OCB allocator, reached through the mount structure ("Extending
            // the OCB and attribute structures" in the resource manager guide).
            (*res).ocb_funcs.nfuncs = sys::_IOFUNC_NFUNCS;
            (*res).ocb_funcs.ocb_calloc = Some(trampoline::ocb_calloc);
            (*res).ocb_funcs.ocb_free = Some(trampoline::ocb_free);
            if sys::iofunc_mount_init(
                ptr::addr_of_mut!((*res).mount),
                size_of::<sys::iofunc_mount_t>(),
            ) == -1
            {
                return Err(Error::last_os());
            }
            (*res).mount.funcs = ptr::addr_of_mut!((*res).ocb_funcs);

            sys::iofunc_attr_init(
                ptr::addr_of_mut!((*res).attr.attr),
                sys::S_IFCHR | config.mode,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            (*res).attr.attr.mount = ptr::addr_of_mut!((*res).mount);
            (*res).attr.device = ptr::addr_of!((*res).device);

            attached.id = sys::resmgr_attach(
                dpp,
                ptr::null_mut(),
                config.path.as_ptr(),
                sys::_FTYPE_ANY,
                // Let this process open the path too (without it, its own opens
                // fail with ENOENT).
                sys::_RESMGR_FLAG_SELF,
                ptr::addr_of!((*res).connect),
                ptr::addr_of!((*res).io),
                ptr::addr_of_mut!((*res).attr).cast::<sys::iofunc_attr_t>(),
            );
        }
        if attached.id == -1 {
            return Err(Error::last_os());
        }

        // SAFETY: `dpp` is live; the handler ignores its arguments.
        attached.stop_code = unsafe {
            sys::pulse_attach(
                dpp,
                sys::MSG_FLAG_ALLOC_PULSE as c_int,
                0,
                Some(trampoline::stop_pulse),
                ptr::null_mut(),
            )
        };
        if attached.stop_code == -1 {
            return Err(Error::last_os());
        }

        // SAFETY: `dpp` is live and has its attachments.
        attached.ctp = unsafe { sys::dispatch_context_alloc(dpp) };
        if attached.ctp.is_null() {
            return Err(Error::last_os());
        }
        Ok(attached)
    }

    /// The dispatch loop: block for a message or pulse, handle it, repeat until
    /// `stop` is set. Dropping `self` afterwards detaches the path.
    fn serve(mut self, stop: Option<&AtomicBool>) -> Result<()> {
        let stopped = || stop.is_some_and(|s| s.load(Ordering::SeqCst));
        while !stopped() {
            // SAFETY: `ctp` is our live context; `dispatch_block` may return a
            // different (reallocated) context, which replaces it.
            let next = unsafe { sys::dispatch_block(self.ctp) };
            if next.is_null() {
                let e = Error::last_os();
                if e.errno() == Some(EINTR) {
                    continue;
                }
                return Err(e);
            }
            self.ctp = next;
            // Runs the callbacks in `trampoline.rs` (or the stop pulse handler).
            // Its return value is only an internal status.
            // SAFETY: `ctp` holds the message `dispatch_block` just received.
            unsafe { sys::dispatch_handler(self.ctp) };
        }
        Ok(())
    }
}

impl<D: Device> Drop for Attached<D> {
    fn drop(&mut self) {
        // SAFETY: each resource is released once, in reverse order of creation,
        // on the thread that ran the loop, so no callback is running. Detaching
        // with `_RESMGR_DETACH_ALL` closes the files still open (freeing their
        // OCBs through `ocb_free`) while the attribute and device still exist.
        unsafe {
            if self.id != -1 {
                sys::resmgr_detach(self.dpp, self.id, sys::_RESMGR_DETACH_ALL);
            }
            if self.stop_code != -1 {
                sys::pulse_detach(self.dpp, self.stop_code, 0);
            }
            if !self.ctp.is_null() {
                sys::dispatch_context_free(self.ctp);
            }
            sys::dispatch_destroy(self.dpp);
            // Nothing points into the box any more: free it, dropping the device.
            drop(Box::from_raw(self.res));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_and_nul_paths() {
        assert_eq!(
            ResourceManager::new("dev/x").unwrap_err(),
            Error::InvalidName("dev/x".into())
        );
        assert_eq!(
            ResourceManager::new("/dev/a\0b").unwrap_err(),
            Error::InvalidName("/dev/a\0b".into())
        );
        assert_eq!(ResourceManager::new("/dev/x").unwrap().mode, 0o666);
    }
}
