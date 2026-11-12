//! The C callbacks the QNX resource-manager framework calls, and the extended
//! attribute and OCB structures they work on.
//!
//! # Soundness
//!
//! Pointer provenance:
//!
//! - The manager allocates one [`Resources`](crate::manager) box per attached
//!   path and turns it into a raw pointer with `Box::into_raw`. The handle it
//!   passes to `resmgr_attach` is a pointer to the box's [`DeviceAttr`] field
//!   (derived from that raw pointer, so it may access the whole `DeviceAttr`),
//!   cast to `*mut iofunc_attr_t`. `DeviceAttr` is `#[repr(C)]` with the
//!   `iofunc_attr_t` first, so the cast back from the framework's
//!   `ocb->attr` to `*mut DeviceAttr<D>` gets the original pointer back.
//! - `DeviceAttr::device` points at the box's `device` field (same allocation,
//!   set once before attaching, never changed). Callbacks only create shared
//!   `&D` references from it; everything else in the allocation that C writes
//!   (the attribute's counts, flags and lock) is a disjoint field.
//! - Every OCB comes from [`ocb_calloc`], which returns `Box::into_raw` of a
//!   [`DeviceOcb`] (`#[repr(C)]`, `iofunc_ocb_t` first). The framework passes
//!   that same pointer to the I/O callbacks and finally to [`ocb_free`], which
//!   turns it back into the box. An OCB belongs to one open file, and the
//!   manager runs a single dispatch thread, so a callback's `&mut DeviceOcb` is
//!   the only reference to it while the callback runs.
//!
//! Lifetime: callbacks run only inside `dispatch_handler` (and `resmgr_detach`,
//! which closes the open files) on the manager's dispatch thread. The manager
//! frees the `Resources` box, and with it the device, only after that thread
//! has left its loop, detached the path and destroyed the dispatch handle, so
//! no callback can run on freed memory.
//!
//! Panics: every callback runs its body inside `catch_unwind`. A panic
//! (normally from a [`Device`] method) answers the client `EIO` and the
//! framework goes on serving; no unwind crosses the FFI boundary. The body
//! never panics after it has replied, so a client is answered exactly once.

use core::ffi::{c_int, c_long, c_uint, c_void};
use core::mem::size_of;
use core::ptr;
use std::panic::{catch_unwind, AssertUnwindSafe};

use qnx_ipc::Error;
use qnx_ipc_sys as sys;

use crate::Device;

const EOK: c_int = sys::EOK as c_int;
const EIO: c_int = sys::EIO as c_int;
const ENOSYS: c_int = sys::ENOSYS as c_int;

/// The attribute structure of an attached path: the `iofunc` attribute plus a
/// pointer to the device that serves it.
#[repr(C)]
pub(crate) struct DeviceAttr<D> {
    /// The framework's attribute; must stay the first field.
    pub(crate) attr: sys::iofunc_attr_t,
    /// The device, owned by the same `Resources` box.
    pub(crate) device: *const D,
}

/// The open control block of one open file: the `iofunc` OCB plus that open's
/// snapshot of the device content.
#[repr(C)]
pub(crate) struct DeviceOcb {
    /// The framework's OCB; must stay the first field.
    ocb: sys::iofunc_ocb_t,
    /// The content returned by `Device::read`, taken on the first read.
    snapshot: Option<Vec<u8>>,
}

/// Runs a callback body, turning a panic into `EIO` for the client.
fn guard(body: impl FnOnce() -> c_int) -> c_int {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(EIO)
}

/// The `errno` a client gets for a device error: the error's own `errno` if it
/// is a positive OS error, else `EIO`.
fn errno_of(e: &Error) -> c_int {
    match e.errno() {
        Some(code) if code > 0 => code,
        _ => EIO,
    }
}

/// The calling thread's `errno`, or `EIO` if it is not set.
fn last_errno() -> c_int {
    errno_of(&Error::last_os())
}

/// The device behind an OCB and the OCB's attribute.
///
/// # Safety
///
/// `ocb` must be a live OCB of a path attached with a `DeviceAttr<D>` handle
/// (see the module docs); the returned reference must not outlive the callback.
/// Call this before creating a `&mut DeviceOcb`, so the raw read of `ocb->attr`
/// does not alias it.
unsafe fn parts_of<'a, D: Device>(
    ocb: *const sys::iofunc_ocb_t,
) -> (&'a D, *mut sys::iofunc_attr_t) {
    // SAFETY: per the contract, `ocb->attr` is the `DeviceAttr<D>` handle and its
    // `device` points at the live device in the same `Resources` box.
    unsafe {
        let attr = (*ocb).attr;
        (&*(*attr.cast::<DeviceAttr<D>>()).device, attr)
    }
}

/// Sets `flags` (`IOFUNC_ATTR_*`) on an attribute.
///
/// # Safety
///
/// `attr` must point at a live attribute. Only the dispatch thread touches it.
unsafe fn mark_attr(attr: *mut sys::iofunc_attr_t, flags: u32) {
    // SAFETY: per the contract. `flags` is a field of the `iofunc_attr_t`, which
    // is disjoint from the device that a live `&D` may point at.
    unsafe { (*attr).flags |= flags };
}

/// `iofunc_funcs_t::ocb_calloc`: allocates a [`DeviceOcb`] for a new open file
/// (called by `iofunc_open_default`). The framework fills in the OCB fields.
pub(crate) unsafe extern "C" fn ocb_calloc(
    _ctp: *mut sys::resmgr_context_t,
    _attr: *mut sys::iofunc_attr_t,
) -> *mut sys::iofunc_ocb_t {
    catch_unwind(|| {
        let ocb = Box::new(DeviceOcb {
            ocb: sys::iofunc_ocb_t::default(),
            snapshot: None,
        });
        Box::into_raw(ocb).cast::<sys::iofunc_ocb_t>()
    })
    // A null OCB makes the open fail (ENOMEM); nothing panics here in practice.
    .unwrap_or(ptr::null_mut())
}

/// `iofunc_funcs_t::ocb_free`: frees an OCB from [`ocb_calloc`] (called by
/// `iofunc_close_ocb_default` and by `resmgr_detach` for files still open).
pub(crate) unsafe extern "C" fn ocb_free(ocb: *mut sys::iofunc_ocb_t) {
    if ocb.is_null() {
        return;
    }
    // SAFETY: every OCB of our paths comes from `ocb_calloc` (`Box::into_raw` of
    // a `DeviceOcb`), and the framework frees each exactly once.
    let ocb = unsafe { Box::from_raw(ocb.cast::<DeviceOcb>()) };
    // Dropping a `Vec<u8>` does not panic; the guard is for the rule.
    let _ = catch_unwind(AssertUnwindSafe(move || drop(ocb)));
}

/// `pulse_attach` handler for the stop pulse: does nothing. The pulse only
/// wakes the dispatch loop, which then sees its stop flag.
pub(crate) unsafe extern "C" fn stop_pulse(
    _ctp: *mut sys::message_context_t,
    _code: c_int,
    _flags: c_uint,
    _handle: *mut c_void,
) -> c_int {
    0
}

/// `resmgr_io_funcs_t::read`: serves `_IO_READ` from the open file's snapshot.
pub(crate) unsafe extern "C" fn io_read<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_read_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: the framework passes a live context, the received message and an
    // OCB of our path (from `ocb_calloc`), valid for this call.
    guard(|| unsafe { read::<D>(ctp, msg, ocb) })
}

/// The body of [`io_read`], following the guide's read handler: verify, reject
/// extended types, reply the data with `MsgReply` and return `_RESMGR_NOREPLY`.
///
/// # Safety
///
/// As for [`io_read`].
unsafe fn read<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_read_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: arguments as the framework passed them (see `io_read`).
    let status = unsafe { sys::iofunc_read_verify(ctp, msg, ocb, ptr::null_mut()) };
    if status != EOK {
        return status;
    }
    // SAFETY: `msg` is the received `io_read_t`; `i` and `i64_` overlay the same
    // plain-integer header, so reading either is defined.
    let (xtype, requested) = unsafe {
        let m = &*msg;
        let mut nbytes = u64::from(m.i.nbytes);
        // `_IO_READ_GET_NBYTES` from `<sys/iofunc.h>`.
        if u32::from(m.i.type_) == sys::_IO_READ64 {
            nbytes |= u64::from(m.i64_.nbytes_hi) << 32;
        }
        (m.i.xtype, nbytes)
    };
    if xtype & sys::_IO_XTYPE_MASK != sys::_IO_XTYPE_NONE {
        return ENOSYS;
    }

    // SAFETY: a live OCB of a `DeviceAttr<D>` path.
    let (device, attr) = unsafe { parts_of::<D>(ocb) };
    // SAFETY: the OCB comes from `ocb_calloc` and only this callback uses it now
    // (see the module docs).
    let docb = unsafe { &mut *ocb.cast::<DeviceOcb>() };
    if docb.ocb.offset == 0 || docb.snapshot.is_none() {
        match device.read() {
            Ok(content) => docb.snapshot = Some(content),
            Err(e) => return errno_of(&e),
        }
    }
    let snapshot = docb.snapshot.as_deref().unwrap_or(&[]);
    // A negative offset cannot come from `iofunc_lseek_default`; treat it, like
    // any offset past the end, as end of file.
    let start = usize::try_from(docb.ocb.offset)
        .unwrap_or(usize::MAX)
        .min(snapshot.len());
    let count = (snapshot.len() - start).min(usize::try_from(requested).unwrap_or(usize::MAX));
    let chunk = &snapshot[start..start + count];
    // `count` is at most the snapshot length, which fits in an `off_t`.
    docb.ocb.offset += count as sys::off_t;
    if count > 0 {
        // SAFETY: the OCB's live attribute.
        unsafe { mark_attr(attr, sys::IOFUNC_ATTR_ATIME) };
    }
    // SAFETY: `ctp` is the live context of this message.
    unsafe { sys::_IO_SET_READ_NBYTES(&mut *ctp, count as c_long) };
    // SAFETY: `chunk` is `count` readable bytes that live across the call. A
    // failed reply (the client is gone) needs no handling.
    unsafe {
        sys::MsgReply(
            (*ctp).rcvid,
            count as c_long,
            chunk.as_ptr().cast(),
            count as sys::_Sizet,
        );
    }
    sys::_RESMGR_NOREPLY
}

/// `resmgr_io_funcs_t::write`: passes the full write payload to `Device::write`.
pub(crate) unsafe extern "C" fn io_write<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_write_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: as for `io_read`.
    guard(|| unsafe { write::<D>(ctp, msg, ocb) })
}

/// Bytes of the client's message after this message's `header` bytes: what
/// `resmgr_msgread` at offset `header` can still read.
///
/// # Safety
///
/// `ctp` must be the live context of the current message.
unsafe fn payload_available(ctp: *const sys::resmgr_context_t, header: usize) -> usize {
    // SAFETY: per the contract.
    let ctx = unsafe { &*ctp };
    usize::try_from(ctx.info.srcmsglen)
        .unwrap_or(0)
        .saturating_sub(ctx.offset)
        .saturating_sub(header)
}

/// The body of [`io_write`], following the guide's write handler: verify,
/// reject extended types, read the data after the header with
/// `resmgr_msgread`, set `_IO_SET_WRITE_NBYTES` and reply with no parts.
///
/// # Safety
///
/// As for [`io_read`].
unsafe fn write<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_write_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: arguments as the framework passed them.
    let status = unsafe { sys::iofunc_write_verify(ctp, msg, ocb, ptr::null_mut()) };
    if status != EOK {
        return status;
    }
    // SAFETY: `msg` is the received `io_write_t` (overlaid integer headers).
    let (xtype, claimed) = unsafe {
        let m = &*msg;
        let mut nbytes = u64::from(m.i.nbytes);
        // `_IO_WRITE_GET_NBYTES` from `<sys/iofunc.h>`.
        if u32::from(m.i.type_) == sys::_IO_WRITE64 {
            nbytes |= u64::from(m.i64_.nbytes_hi) << 32;
        }
        (m.i.xtype, nbytes)
    };
    if xtype & sys::_IO_XTYPE_MASK != sys::_IO_XTYPE_NONE {
        return ENOSYS;
    }

    let header = size_of::<sys::io_write_t>();
    // Never trust `nbytes` beyond what the client actually sent.
    // SAFETY: `ctp` is the live context.
    let len = usize::try_from(claimed)
        .unwrap_or(usize::MAX)
        .min(unsafe { payload_available(ctp, header) });
    let mut data = vec![0u8; len];
    if len > 0 {
        // SAFETY: `data` has `len` writable bytes; `resmgr_msgread` adds
        // `ctp->offset` itself, so `header` is relative to this message.
        let got = unsafe { sys::resmgr_msgread(ctp, data.as_mut_ptr().cast(), len, header) };
        let Ok(got) = usize::try_from(got) else {
            return last_errno();
        };
        data.truncate(got);
    }

    // SAFETY: a live OCB of a `DeviceAttr<D>` path.
    let (device, attr) = unsafe { parts_of::<D>(ocb) };
    let taken = match device.write(&data) {
        Ok(n) => n.min(data.len()),
        Err(e) => return errno_of(&e),
    };
    if taken > 0 {
        // SAFETY: the OCB's live attribute.
        unsafe { mark_attr(attr, sys::IOFUNC_ATTR_MTIME | sys::IOFUNC_ATTR_CTIME) };
    }
    // SAFETY: `ctp` is the live context of this message.
    unsafe { sys::_IO_SET_WRITE_NBYTES(&mut *ctp, taken as c_long) };
    sys::_RESMGR_NPARTS(0)
}

/// `resmgr_io_funcs_t::devctl`: standard commands go to
/// `iofunc_devctl_default`, the rest to `Device::devctl`.
pub(crate) unsafe extern "C" fn io_devctl<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_devctl_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: as for `io_read`.
    guard(|| unsafe { devctl::<D>(ctp, msg, ocb) })
}

/// The body of [`io_devctl`], following the guide's devctl handler: try
/// `iofunc_devctl_default` first, then hand the data (`_DEVCTL_DATA`) to the
/// device and reply the `_io_devctl_reply` header plus the output bytes.
///
/// # Safety
///
/// As for [`io_read`].
unsafe fn devctl<D: Device>(
    ctp: *mut sys::resmgr_context_t,
    msg: *mut sys::io_devctl_t,
    ocb: *mut sys::iofunc_ocb_t,
) -> c_int {
    // SAFETY: arguments as the framework passed them.
    let status = unsafe { sys::iofunc_devctl_default(ctp, msg, ocb) };
    if status != sys::_RESMGR_DEFAULT {
        return status;
    }
    // SAFETY: `msg` is the received `io_devctl_t`; copy its input header before
    // anything can reuse the receive buffer.
    let input = unsafe { (*msg).i };

    let header = size_of::<sys::io_devctl_t>();
    let reply_header = size_of::<sys::_io_devctl_reply>();
    // SAFETY: `ctp` is the live context.
    let (in_avail, out_room) = unsafe {
        let dst = usize::try_from((*ctp).info.dstmsglen).unwrap_or(0);
        (
            payload_available(ctp, header),
            dst.saturating_sub(reply_header),
        )
    };
    // The buffer holds the client's `nbytes`, but never more than the client
    // actually sent or can receive (so a bogus `nbytes` cannot exhaust memory).
    let len = (input.nbytes as usize).min(in_avail.max(out_room));
    let mut data = vec![0u8; len];
    let to_read = len.min(in_avail);
    if to_read > 0 {
        // SAFETY: `data` has at least `to_read` writable bytes.
        let got = unsafe { sys::resmgr_msgread(ctp, data.as_mut_ptr().cast(), to_read, header) };
        if got < 0 {
            return last_errno();
        }
    }

    // SAFETY: a live OCB of a `DeviceAttr<D>` path.
    let (device, _) = unsafe { parts_of::<D>(ocb) };
    let out = match device.devctl(input.dcmd as u32, &mut data) {
        Ok(n) => n.min(data.len()),
        Err(e) => return errno_of(&e),
    };

    let reply = sys::_io_devctl_reply {
        zero: 0,
        ret_val: 0,
        nbytes: out as u32,
        zero2: 0,
    };
    let mut iov = [sys::iov_t::default(), sys::iov_t::default()];
    sys::SETIOV(&mut iov[0], ptr::addr_of!(reply).cast(), reply_header);
    sys::SETIOV(&mut iov[1], data.as_ptr().cast(), out);
    // SAFETY: both parts point at live buffers of the given lengths for the
    // whole call. A failed reply (the client is gone) needs no handling.
    unsafe { sys::MsgReplyv((*ctp).rcvid, EOK as c_long, iov.as_ptr(), 2) };
    sys::_RESMGR_NOREPLY
}
