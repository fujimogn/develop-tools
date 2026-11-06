//! Shared heavy-check admission core. No production path/skip environment overrides.
pub mod state;

use std::time::{Duration, Instant};

/// Monotonic clock seam shared by queue waiting and family supervision.
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

pub struct MonotonicClock {
    origin: Instant,
}
impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
pub mod queue;

pub mod supervisor;

pub mod inherited;

pub mod terminal;

pub mod events;

use events::Events;
use inherited::InheritedSlot;
use queue::Ticket;
use serde_json::{Value, json};
use state::{AccountSource, FilesystemHooks, Scope, StateLayout};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use supervisor::{Family, SlotSender, Supervision};

const HEARTBEAT: Duration = Duration::from_secs(30);
const RESULT_FD_ENV: &str = "DEVELOP_HEAVY_CHECK_RESULT_FD";

/// Shared production/fixture entry. Only account/filesystem/clock are injectable;
/// production has no test path override or environment-only admission bypass.
pub fn run(
    args: Vec<OsString>,
    source: &impl AccountSource,
    hooks: &impl FilesystemHooks,
    clock: &dyn Clock,
) -> i32 {
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        print!(
            "{}",
            crate::cli_help::Help::new(
                "run-heavy-check [--executor-context=headless-sandbox] [--] <command...>",
                "Run one heavy-check family with account-wide FIFO admission.",
                "host admission; GITHUB_ACTIONS=true preserves raw result without admission",
                "account cache ticket and lock files; diagnostics on stderr only",
            )
            .exit("payload code (local 75 maps to 1); signal 128+signal; unavailable 75")
            .render()
        );
        return 0;
    }
    if args.first().is_some_and(|a| a == "--version") {
        println!("run-heavy-check {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let mut command_args = args;
    if command_args.first().is_some_and(|arg| {
        arg.as_encoded_bytes().starts_with(b"--executor-context=")
            && arg != "--executor-context=headless-sandbox"
    }) {
        eprintln!("run-heavy-check: unknown executor context");
        return 2;
    }
    let headless = command_args
        .first()
        .is_some_and(|a| a == "--executor-context=headless-sandbox");
    if headless {
        command_args.remove(0);
    }
    if command_args.first().is_some_and(|a| a == "--") {
        command_args.remove(0);
    }
    if command_args.is_empty() {
        eprintln!("run-heavy-check: command required");
        return 2;
    }
    let ci = std::env::var_os("GITHUB_ACTIONS").is_some_and(|v| v == "true");
    let events = Events::from_environment();
    let result = (|| -> io::Result<i32> {
        if let Some(sender) = SlotSender::from_environment()? {
            sender.wait_for_supervisor()?;
            let mut report = match std::env::var_os(RESULT_FD_ENV) {
                None => None,
                Some(value) => {
                    let fd = value
                        .to_str()
                        .and_then(|v| v.parse::<i32>().ok())
                        .filter(|fd| *fd > 2)
                        .ok_or_else(|| io::Error::other("invalid result channel"))?;
                    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
                    if duplicate < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    unsafe {
                        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    }
                    Some(unsafe { UnixStream::from_raw_fd(duplicate) })
                }
            };
            return worker(
                &command_args,
                source,
                hooks,
                clock,
                &events,
                Some(sender),
                report.as_mut(),
                ci,
                headless,
            );
        }
        // A valid nested capability remains in its existing supervised group.
        if std::env::var_os(inherited::SLOT_FD_ENV).is_some() && !ci {
            return worker(
                &command_args,
                source,
                hooks,
                clock,
                &events,
                None,
                None,
                ci,
                headless,
            );
        }
        let _signals = Supervision::install()?;
        let (receiver, sender) = supervisor::slot_channel()?;
        let (result_read, result_write) = UnixStream::pair()?;
        let result_fd = result_write.as_raw_fd();
        let mut command = Command::new(std::env::current_exe()?);
        // Reexec exactly the original invocation, including fixture entry args.
        command
            .args(std::env::args_os().skip(1))
            .env(RESULT_FD_ENV, result_fd.to_string());
        sender.configure_worker(&mut command);
        events.configure_worker(&mut command);
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(result_fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = supervisor::spawn(&mut command)?;
        drop(sender);
        drop(result_write);
        let mut family = Family::new(child, receiver)?;
        let mut foreground = match terminal::Foreground::transfer(family.group()) {
            Ok(value) => value,
            Err(error) => {
                family.signal(libc::SIGKILL, clock)?;
                retain_until_stopped(&mut family, clock);
                return Err(error);
            }
        };
        if family.start_worker().is_err() {
            family.signal(libc::SIGKILL, clock)?;
        }
        let code = retain_until_stopped(&mut family, clock);
        if foreground.restore().is_err() {
            eprintln!("heavy-check terminal foreground restore failed; preserving check result");
        }
        result_read.set_nonblocking(true)?;
        let mut reports = String::new();
        // The private protocol has at most two short frames. Do not wait for EOF
        // if a helper retained an unrelated descriptor outside the contract.
        let _ = result_read.take(4096).read_to_string(&mut reports);
        let values: Vec<Value> = reports
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect();
        if let Some(start) = values.first() {
            let raw = values.get(1).unwrap_or(start);
            let check_ms =
                monotonic_ms().saturating_sub(start["acquiredMs"].as_u64().unwrap_or(0) as u128);
            eprintln!("completed after {}s; exit={code}", check_ms / 1000);
            events.emit(json!({"event":"completed", "lockWaitMs":start["lockWaitMs"], "checkMs":check_ms,
                "rawExit":raw["rawExit"], "rawSignal":raw["rawSignal"].as_i64().or_else(|| (values.len() < 2 && code >= 128).then_some((code-128) as i64)),
                "result":code, "executed":true, "isolated":start["isolated"], "context":start["context"]}));
        } else {
            unavailable(
                &events,
                clock,
                headless,
                ci,
                (code >= 128).then_some(code - 128),
            );
        }
        Ok(code)
    })();
    result.unwrap_or_else(|_| {
        if std::env::var_os(inherited::CONTROL_FD_ENV).is_none() {
            unavailable(&events, clock, headless, ci, None);
        }
        75
    })
}

fn retain_until_stopped(family: &mut Family, clock: &dyn Clock) -> i32 {
    match family.wait(clock, Duration::from_secs(5)) {
        Ok(code) => code,
        Err(_) => {
            eprintln!("heavy-check group stop unconfirmed; retaining slot");
            let real = MonotonicClock::default();
            while family
                .confirm_stopped(&real, Duration::from_secs(1))
                .is_err()
            {}
            1
        }
    }
}
fn unavailable(
    events: &Events,
    clock: &dyn Clock,
    headless: bool,
    ci: bool,
    raw_signal: Option<i32>,
) {
    eprintln!(
        "lock unavailable after {}s; tests not run",
        clock.now().as_secs()
    );
    events.emit(
        json!({"event":"unavailable", "lockWaitMs":clock.now().as_millis(), "checkMs":0,
        "rawExit":null, "rawSignal":raw_signal, "result":75, "executed":false, "isolated":headless, "context": if ci {"ci"} else if headless {"headless-sandbox"} else {"host"}}),
    );
}

#[allow(clippy::too_many_arguments)]
fn worker(
    args: &[OsString],
    source: &impl AccountSource,
    hooks: &impl FilesystemHooks,
    clock: &dyn Clock,
    events: &Events,
    sender: Option<SlotSender>,
    mut report: Option<&mut UnixStream>,
    ci: bool,
    headless: bool,
) -> io::Result<i32> {
    let start = clock.now();
    let mut slot = None;
    let mut inherited = None;
    let mut isolated = headless;
    let mut context = if ci {
        "ci"
    } else if headless {
        "headless-sandbox"
    } else {
        "host"
    };
    if !ci {
        let state = if headless {
            state::prepare_headless(source, hooks)
        } else {
            state::prepare(source, &StateLayout::default(), hooks)
        }
        .map_err(io::Error::other)?;
        if let Scope::Isolated { account_errno } = state.scope() {
            eprintln!("account state write denied (errno {account_errno}); using isolated HOME");
            isolated = true;
            context = "write-denial-fallback";
        }
        inherited = InheritedSlot::from_environment(&state).map_err(io::Error::other)?;
        if inherited.is_none() {
            let mut ticket = Ticket::register(state).map_err(io::Error::other)?;
            eprintln!("queued ticket={}", ticket.number());
            events.emit(json!({"event":"queued", "ticket":ticket.number(), "executed":false, "isolated":isolated, "context":context, "lockWaitMs":0, "checkMs":0}));
            let mut heartbeat = clock.now() + HEARTBEAT;
            loop {
                if let Some(acquired) = ticket.try_acquire().map_err(io::Error::other)? {
                    slot = Some(acquired);
                    break;
                }
                if clock.now() >= heartbeat {
                    let head = ticket
                        .head_number()
                        .map_err(io::Error::other)?
                        .unwrap_or(ticket.number());
                    eprintln!(
                        "still waiting after {}s; head ticket={head}",
                        clock.now().saturating_sub(start).as_secs()
                    );
                    heartbeat = clock.now() + HEARTBEAT;
                }
                clock.sleep(queue::RETRY_INTERVAL);
            }
        }
        if let (Some(sender), Some(slot)) = (&sender, &slot) {
            sender.send(slot.file())?;
        }
    }
    drop(sender);
    let acquired = clock.now();
    let acquired_tick = monotonic_ms();
    let wait = acquired.saturating_sub(start).as_millis();
    eprintln!("acquired after {}s", wait / 1000);
    events.emit(json!({"event":"acquired", "lockWaitMs":wait, "checkMs":0, "executed":false, "isolated":isolated, "context":context}));
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .env_remove(RESULT_FD_ENV)
        .env_remove(events::EVENT_FD_ENV);
    if let Some(slot) = &slot {
        // Validate and propagate the exact acquired open description.
        let state = if headless {
            state::prepare_headless(source, hooks)
        } else {
            state::prepare(source, &StateLayout::default(), hooks)
        }
        .map_err(io::Error::other)?;
        inherited = Some(
            InheritedSlot::validate(&state, slot.file().as_raw_fd()).map_err(io::Error::other)?,
        );
    }
    if let Some(inherited) = &inherited {
        inherited.configure_payload(&mut command);
    }
    let child = supervisor::spawn(&mut command)?;
    let start_report =
        json!({"lockWaitMs":wait,"acquiredMs":acquired_tick,"isolated":isolated,"context":context});
    if let Some(report) = report.as_mut() {
        writeln!(report, "{start_report}")?;
    }
    let status = child.wait_with_output()?.status;
    let raw = status.code();
    let signal = status.signal();
    let code = raw.unwrap_or_else(|| 128 + signal.unwrap_or(1));
    let result = if !ci && code == 75 { 1 } else { code };
    let end = json!({"rawExit":raw,"rawSignal":signal});
    if let Some(report) = report.as_mut() {
        let _ = writeln!(report, "{end}");
    } else {
        eprintln!(
            "completed after {}s; exit={result}",
            clock.now().saturating_sub(acquired).as_secs()
        );
        events.emit(json!({"event":"completed", "lockWaitMs":wait,"checkMs":clock.now().saturating_sub(acquired).as_millis(),
            "rawExit":raw,"rawSignal":signal,"result":result,"executed":true,"isolated":isolated,"context":context}));
    }
    Ok(result)
}

fn monotonic_ms() -> u128 {
    let mut value: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) },
        0
    );
    value.tv_sec as u128 * 1000 + value.tv_nsec as u128 / 1_000_000
}
