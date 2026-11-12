//! `safety-client`: the high-priority client of the vehicle-signals demo.
//!
//! It switches itself to `SCHED_FIFO` priority 40, says hello to `vsigd`, then
//! asks for the brake pressure every 100 ms and sends the watchdog a heartbeat
//! pulse each time. It runs until it is killed or `vsigd` goes away.

#[path = "../../vsigd/src/proto.rs"]
mod proto;

use std::thread::sleep;
use std::time::Duration;

use qnx_ipc::{Connection, Error, PulseCode};
use qnx_ipc_sys as sys;

use proto::{
    GetSignal, Hello, SignalValue, CLIENT_ID_SAFETY, HEARTBEAT_CODE, SIGNAL_BRAKE, VEHICLE_NAME,
    WATCHDOG_NAME,
};

/// The name this client gives `vsigd` and logs under.
const NAME: &str = "safety-client";
/// The `SCHED_FIFO` priority this client runs at.
const PRIORITY: i32 = 40;
/// How often it asks for the brake pressure.
const PERIOD: Duration = Duration::from_millis(100);

/// Switches the calling thread to `SCHED_FIFO` at `priority` (`SchedSet`).
fn set_fifo_priority(priority: i32) -> qnx_ipc::Result<()> {
    let param = sys::sched_param {
        sched_priority: priority,
        ..Default::default()
    };
    // SAFETY: pid 0 and tid 0 name the calling thread; `param` is a live,
    // initialized sched_param the kernel only reads.
    let rc = unsafe { sys::SchedSet(0, 0, sys::SCHED_FIFO as i32, &param) };
    if rc == -1 {
        return Err(Error::last_os());
    }
    Ok(())
}

/// Opens `name`, retrying for up to 5 s while its server starts.
fn open(name: &str) -> qnx_ipc::Result<Connection> {
    let mut attempt = 0;
    loop {
        match Connection::open(name) {
            Err(Error::Os(errno)) if errno == sys::ENOENT as i32 && attempt < 100 => {
                attempt += 1;
                sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{NAME}: {e}");
        std::process::exit(1);
    }
}

fn run() -> qnx_ipc::Result<()> {
    set_fifo_priority(PRIORITY)?;

    let vehicle = open(VEHICLE_NAME)?;
    vehicle.send::<Hello, ()>(&Hello::new(NAME))?;
    let watchdog = open(WATCHDOG_NAME)?;
    let heartbeat = PulseCode::user(HEARTBEAT_CODE)?;
    println!("{NAME}: running at priority {PRIORITY} (SCHED_FIFO), reading brake pressure");

    loop {
        let _brake: SignalValue = vehicle.send(&GetSignal {
            signal_id: SIGNAL_BRAKE,
        })?;
        watchdog.send_pulse(heartbeat, CLIENT_ID_SAFETY)?;
        sleep(PERIOD);
    }
}
