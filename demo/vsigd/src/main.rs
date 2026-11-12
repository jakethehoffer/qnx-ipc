//! `vsigd`: the vehicle-signals server of the demo.
//!
//! It registers the name `"vehicle"` and answers `GET_SIGNAL` requests from the
//! clients, serves the same signals as text files at `/dev/vehicle/speed` and
//! `/dev/vehicle/brake`, and moves the simulated signals along on a 10 Hz pulse
//! timer.
//!
//! For each request it logs `served <client> at priority <p>`, where `<p>` is
//! the priority this server thread is running at while it handles the request,
//! read from the kernel with `SchedGet`. The channel does not have
//! `_NTO_CHF_FIXED_PRIORITY`, so QNX runs the receiving thread at the sending
//! client's priority: the log shows 40 for the safety client and 10 for the
//! infotainment client.

mod proto;

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use qnx_ipc::{
    Connection, NamedChannel, Pulse, PulseCode, PulseKind, PulseTimer, RecvBuf, Request,
};
use qnx_ipc_sys as sys;
use qnx_resmgr::{Device, ResourceManager};

use proto::{GetSignal, Hello, SignalValue, SIGNAL_BRAKE, SIGNAL_SPEED, VEHICLE_NAME};

const HELLO_KIND: u16 = <Hello as qnx_ipc::Message>::KIND;
const GET_SIGNAL_KIND: u16 = <GetSignal as qnx_ipc::Message>::KIND;

/// Pulse code of the simulation timer.
const TICK_CODE: u8 = 1;
/// The simulation step to start from: just past top speed, braking, so the
/// demo shows a non-zero brake pressure.
const START_TICK: u64 = 60;
/// How often the simulated signals move: 10 Hz.
const TICK_PERIOD: Duration = Duration::from_millis(100);

/// The current simulated signals, in thousandths of their unit.
#[derive(Debug, Default)]
struct Signals {
    speed_milli: AtomicI64,
    brake_milli: AtomicI64,
}

impl Signals {
    /// Moves the signals to simulation step `tick`: a smooth, repeating drive
    /// between 55 and 105 km/h, braking while the speed falls.
    fn update(&self, tick: u64) {
        let phase = tick as f64 * 0.03;
        let speed_kmh = 80.0 + 25.0 * phase.sin();
        let brake_kpa = (-phase.cos()).max(0.0) * 1200.0;
        self.speed_milli
            .store((speed_kmh * 1000.0).round() as i64, Ordering::Relaxed);
        self.brake_milli
            .store((brake_kpa * 1000.0).round() as i64, Ordering::Relaxed);
    }

    /// The value of signal `id`, or `None` for an unknown id.
    fn get(&self, id: u32) -> Option<i64> {
        match id {
            SIGNAL_SPEED => Some(self.speed_milli.load(Ordering::Relaxed)),
            SIGNAL_BRAKE => Some(self.brake_milli.load(Ordering::Relaxed)),
            _ => None,
        }
    }
}

/// `value_milli` as text with one decimal place, rounded half up
/// (`87_549` gives `"87.5"`, `87_550` gives `"87.6"`).
fn one_decimal(value_milli: i64) -> String {
    let tenths = (value_milli + value_milli.signum() * 50) / 100;
    let sign = if tenths < 0 { "-" } else { "" };
    format!("{sign}{}.{}", (tenths / 10).abs(), (tenths % 10).abs())
}

/// One signal served as a read-only text file, for example `speed_kmh=87.5\n`.
struct SignalFile {
    signals: Arc<Signals>,
    signal_id: u32,
    label: &'static str,
}

impl Device for SignalFile {
    fn read(&self) -> qnx_ipc::Result<Vec<u8>> {
        let value = self.signals.get(self.signal_id).unwrap_or(0);
        Ok(format!("{}={}\n", self.label, one_decimal(value)).into_bytes())
    }
}

/// The priority the calling thread is running at right now (`SchedGet`'s
/// `sched_curpriority`), which includes priority inherited from a client.
fn current_priority() -> qnx_ipc::Result<i32> {
    let mut param = sys::sched_param::default();
    // SAFETY: pid 0 and tid 0 name the calling thread; the kernel fills `param`,
    // a live local of the type SchedGet takes.
    let rc = unsafe { sys::SchedGet(0, 0, &mut param) };
    if rc == -1 {
        return Err(qnx_ipc::Error::last_os());
    }
    Ok(param.sched_curpriority)
}

/// Answers the clients' messages and the simulation timer's pulses.
struct Server {
    signals: Arc<Signals>,
    /// Client names from `HELLO`, by sender process id.
    names: HashMap<i32, String>,
    tick: u64,
}

impl Server {
    fn client_name(&self, pid: i32) -> &str {
        self.names
            .get(&pid)
            .map_or("unnamed client", String::as_str)
    }
}

impl qnx_ipc::Handler for Server {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        let pid = req.sender().pid;
        match req.kind() {
            HELLO_KIND => {
                let hello: Hello = req.decode()?;
                let name = hello.name();
                println!("vsigd: hello from {name}");
                self.names.insert(pid, name);
                req.reply(&())?;
            }
            GET_SIGNAL_KIND => {
                let get: GetSignal = req.decode()?;
                let Some(value_milli) = self.signals.get(get.signal_id) else {
                    req.reply_error(sys::EINVAL as i32)?;
                    return Ok(ControlFlow::Continue(()));
                };
                let priority = current_priority()?;
                println!(
                    "vsigd: served {} at priority {priority}",
                    self.client_name(pid)
                );
                req.reply(&SignalValue { value_milli })?;
            }
            // Dropping the request answers ENOSYS.
            _ => {}
        }
        Ok(ControlFlow::Continue(()))
    }

    fn on_pulse(&mut self, pulse: Pulse) -> qnx_ipc::Result<ControlFlow<()>> {
        if pulse.kind() == PulseKind::User(TICK_CODE) {
            self.tick += 1;
            self.signals.update(self.tick);
        }
        Ok(ControlFlow::Continue(()))
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("vsigd: {e}");
        std::process::exit(1);
    }
}

fn run() -> qnx_ipc::Result<()> {
    let signals = Arc::new(Signals::default());
    signals.update(START_TICK);

    let channel = NamedChannel::attach(VEHICLE_NAME)?;
    let _speed = ResourceManager::new("/dev/vehicle/speed")?
        .mode(0o444)
        .spawn(SignalFile {
            signals: Arc::clone(&signals),
            signal_id: SIGNAL_SPEED,
            label: "speed_kmh",
        })?;
    let _brake = ResourceManager::new("/dev/vehicle/brake")?
        .mode(0o444)
        .spawn(SignalFile {
            signals: Arc::clone(&signals),
            signal_id: SIGNAL_BRAKE,
            label: "brake_kpa",
        })?;

    let self_conn = Connection::to_local(channel.id())?;
    let _timer = PulseTimer::periodic(&self_conn, PulseCode::user(TICK_CODE)?, TICK_PERIOD)?;

    println!(
        "vsigd: serving channel \"{VEHICLE_NAME}\", /dev/vehicle/speed and /dev/vehicle/brake"
    );
    let mut server = Server {
        signals,
        names: HashMap::new(),
        tick: START_TICK,
    };
    channel.serve(&mut RecvBuf::with_capacity(64), &mut server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_decimal_rounds_half_up() {
        assert_eq!(one_decimal(87_549), "87.5");
        assert_eq!(one_decimal(87_550), "87.6");
        assert_eq!(one_decimal(0), "0.0");
        assert_eq!(one_decimal(5), "0.0");
        assert_eq!(one_decimal(-1_250), "-1.3");
    }

    #[test]
    fn signals_stay_in_range() {
        let signals = Signals::default();
        for tick in 0..1000 {
            signals.update(tick);
            let speed = signals.get(SIGNAL_SPEED).unwrap();
            let brake = signals.get(SIGNAL_BRAKE).unwrap();
            assert!((55_000..=105_000).contains(&speed), "speed {speed}");
            assert!((0..=1_200_000).contains(&brake), "brake {brake}");
        }
        assert_eq!(signals.get(3), None);
    }

    #[test]
    fn hello_name_round_trip() {
        assert_eq!(Hello::new("safety-client").name(), "safety-client");
        assert_eq!(
            Hello::new("a-name-that-is-longer-than-32-bytes").name(),
            "a-name-that-is-longer-than-32-by"
        );
    }
}
