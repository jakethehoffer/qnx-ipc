//! Typed messages and their wire format.

use core::marker::PhantomData;

/// The first kind open to user messages. Kinds below it are reserved for QNX
/// (`_IO_BASE`..`_IO_MAX`, 0x100..0x1FF, carries `name_open` connect messages).
pub(crate) const FIRST_USER_KIND: u16 = 0x200;

/// The header in front of every typed message on the wire.
///
/// A message is this header followed directly by the payload bytes of the
/// [`Message`] type, so a C client can send `struct { uint16_t kind; uint16_t
/// reserved; payload }`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Header {
    /// The message kind ([`Message::KIND`]).
    pub kind: u16,
    /// Always zero.
    pub reserved: u16,
}

/// A plain-data message type with its wire kind.
///
/// `KIND` must be at least 0x200, which keeps user kinds clear of QNX's
/// `_IO_BASE`..`_IO_MAX` range. Sending or decoding a type with a lower kind fails
/// to compile:
///
/// ```compile_fail
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
/// struct Bad { n: u32 }
/// impl qnx_ipc::Message for Bad { const KIND: u16 = 0x100; }
///
/// let conn = qnx_ipc::Connection::to_local(1).unwrap();
/// // error[E0080]: Message::KIND must be >= 0x200
/// let _: qnx_ipc::Result<Bad> = conn.send(&Bad { n: 1 });
/// ```
pub trait Message: bytemuck::Pod {
    /// The kind carried in [`Header::kind`]. Must be `>= 0x200`.
    const KIND: u16;
}

/// Compile-time check that `M::KIND` is a user kind. Evaluating `OK` for a type
/// with a kind below 0x200 is a compile error.
pub(crate) struct UserKind<M>(PhantomData<M>);

impl<M: Message> UserKind<M> {
    pub(crate) const OK: () = assert!(
        M::KIND >= FIRST_USER_KIND,
        "Message::KIND must be >= 0x200 (lower kinds are reserved for QNX)"
    );
}

/// Bytes on the wire for a message of type `M`: header plus payload.
pub(crate) const fn wire_len<M: Message>() -> usize {
    core::mem::size_of::<Header>() + core::mem::size_of::<M>()
}
