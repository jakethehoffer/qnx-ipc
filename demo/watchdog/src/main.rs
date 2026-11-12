//! `watchdog`: starts the demo's clients and restarts any client whose
//! heartbeats stop.
//!
//! ```text
//! watchdog <id> <name> <program> [<id> <name> <program> ...]
//! ```
//!
//! It registers the name `"watchdog"`, starts each `<program>` (no arguments),
//! and receives heartbeat pulses (code 10, value = client id) from them. A
//! 500 ms pulse timer drives the checks: a client that sent no heartbeat since
//! the previous check misses that check, and after 3 missed checks in a row the
//! watchdog logs `restarting <name>`, kills the old process (a stopped one
//! too) and starts `<program>` again.
//!
//! Heartbeats are matched by their pulse value because a pulse does not say
//! which connection sent it.

#[path = "../../vsigd/src/proto.rs"]
mod proto;

use std::ops::ControlFlow;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use qnx_ipc::{
    Connection, NamedChannel, Pulse, PulseCode, PulseKind, PulseTimer, RecvBuf, Request,
};

use proto::{HEARTBEAT_CODE, WATCHDOG_NAME};

/// Pulse code of the check timer.
const CHECK_CODE: u8 = 1;
/// How often heartbeats are checked.
const CHECK_PERIOD: Duration = Duration::from_millis(500);
/// Missed checks in a row after which a client is restarted.
const MAX_MISSED: u32 = 3;

/// One supervised client.
struct Client {
    id: i32,
    name: String,
    program: String,
    child: Child,
    /// A heartbeat arrived since the last check.
    alive: bool,
    /// Checks missed in a row.
    missed: u32,
}

impl Client {
    fn start(id: i32, name: String, program: String) -> std::io::Result<Client> {
        let child = spawn(&program)?;
        println!("watchdog: started {name}");
        Ok(Client {
            id,
            name,
            program,
            child,
            alive: false,
            missed: 0,
        })
    }

    /// Kills the current process (`SIGKILL` also ends a stopped one), reaps it
    /// and starts the program again.
    fn restart(&mut self) -> std::io::Result<()> {
        println!("watchdog: restarting {}", self.name);
        // It may have exited already; either way it is gone after `wait`.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child = spawn(&self.program)?;
        self.alive = false;
        self.missed = 0;
        Ok(())
    }
}

/// Starts `program` with no input; its output goes where the watchdog's goes.
fn spawn(program: &str) -> std::io::Result<Child> {
    Command::new(program).stdin(Stdio::null()).spawn()
}

/// Handles heartbeat and check pulses.
struct Watchdog {
    clients: Vec<Client>,
}

impl Watchdog {
    fn check(&mut self) {
        for client in &mut self.clients {
            // Reap a client that exited on its own; it is restarted below once
            // it has missed enough checks.
            let _ = client.child.try_wait();
            if client.alive {
                client.alive = false;
                client.missed = 0;
                continue;
            }
            client.missed += 1;
            if client.missed >= MAX_MISSED {
                println!(
                    "watchdog: {} missed {MAX_MISSED} heartbeat checks",
                    client.name
                );
                if let Err(e) = client.restart() {
                    eprintln!("watchdog: cannot restart {}: {e}", client.name);
                }
            }
        }
    }
}

impl qnx_ipc::Handler for Watchdog {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        // The watchdog takes no messages; dropping the request answers ENOSYS.
        drop(req);
        Ok(ControlFlow::Continue(()))
    }

    fn on_pulse(&mut self, pulse: Pulse) -> qnx_ipc::Result<ControlFlow<()>> {
        match pulse.kind() {
            PulseKind::User(HEARTBEAT_CODE) => {
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == pulse.value) {
                    client.alive = true;
                }
            }
            PulseKind::User(CHECK_CODE) => self.check(),
            _ => {}
        }
        Ok(ControlFlow::Continue(()))
    }
}

/// Parses `<id> <name> <program>` triples.
fn parse(args: &[String]) -> Result<Vec<(i32, String, String)>, String> {
    if args.is_empty() || args.len() % 3 != 0 {
        return Err("usage: watchdog <id> <name> <program> [<id> <name> <program> ...]".into());
    }
    args.chunks(3)
        .map(|t| {
            let id = t[0]
                .parse()
                .map_err(|_| format!("not a client id: {}", t[0]))?;
            Ok((id, t[1].clone(), t[2].clone()))
        })
        .collect()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("watchdog: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let specs = parse(&args)?;

    // Register the name before starting the clients, so their heartbeats have
    // somewhere to go.
    let channel = NamedChannel::attach(WATCHDOG_NAME)?;
    let self_conn = Connection::to_local(channel.id())?;

    let mut watchdog = Watchdog {
        clients: Vec::new(),
    };
    for (id, name, program) in specs {
        watchdog.clients.push(Client::start(id, name, program)?);
    }

    let _timer = PulseTimer::periodic(&self_conn, PulseCode::user(CHECK_CODE)?, CHECK_PERIOD)?;
    channel.serve(&mut RecvBuf::with_capacity(64), &mut watchdog)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_triples() {
        let specs = parse(&strings(&["1", "a", "./a", "2", "b", "./b"])).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[1], (2, "b".to_string(), "./b".to_string()));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&[]).is_err());
        assert!(parse(&strings(&["1", "a"])).is_err());
        assert!(parse(&strings(&["x", "a", "./a"])).is_err());
    }
}
