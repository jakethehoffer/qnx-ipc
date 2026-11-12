//! A server and a client in one process, talking over a QNX channel with typed
//! messages: the client sends `Ping { n: 41 }`, the server replies
//! `Pong { n: 42 }` and stops.
//!
//! `cargo run -p qnx-ipc --example ping_pong --target x86_64-pc-nto-qnx800`

use std::ops::ControlFlow;
use std::thread;

use qnx_ipc::{Channel, ChannelFlags, Connection, Handler, Message, RecvBuf, Request};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Ping {
    n: u32,
}

impl Message for Ping {
    const KIND: u16 = 0x201; // user kinds start at 0x200
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Pong {
    n: u32,
}

struct Server;

impl Handler for Server {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        // A short or foreign message fails here; the dropped request then
        // answers the client ENOSYS, so it is not left blocked.
        let ping: Ping = req.decode()?;
        req.reply(&Pong { n: ping.n + 1 })?;
        Ok(ControlFlow::Break(())) // one request, then stop
    }
}

fn main() -> qnx_ipc::Result<()> {
    let channel = Channel::new(ChannelFlags::empty())?;
    let conn = Connection::to_local(channel.id())?;
    // The server thread owns the channel. If the send fails, `?` returns from
    // `main` and the process exits, server thread included; joining a server
    // that never got its message would wait forever.
    let server = thread::spawn(move || channel.serve(&mut RecvBuf::with_capacity(64), &mut Server));
    let pong: Pong = conn.send(&Ping { n: 41 })?;
    println!("sent ping 41, got pong {}", pong.n);
    server.join().expect("server thread panicked")
}
