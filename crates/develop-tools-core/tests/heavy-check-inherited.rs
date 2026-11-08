#[path = "support/heavy_check.rs"]
#[allow(dead_code)]
mod support;

use develop_tools_core::heavy_check::MonotonicClock;
use develop_tools_core::heavy_check::queue::Ticket;
use develop_tools_core::heavy_check::state::{self, NativeFilesystem, StateLayout};
use develop_tools_core::heavy_check::supervisor;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SLOT_ENV: &str = "DEVELOP_HEAVY_CHECK_SLOT_FD";

// A concurrent fork can retain validate's temporary locked probe until exec,
// even with CLOEXEC and distinct fixture roots. Keep probes and spawns apart.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn state(home: &Path) -> state::State {
    state::prepare(
        &support::Source {
            account: home.into(),
            home: None,
        },
        &StateLayout::default(),
        &NativeFilesystem,
    )
    .unwrap()
}
fn command(home: &Path) -> Command {
    let mut command = support::harness_command();
    command.arg("--inherited").arg(home);
    command
}
fn pass_fd(command: &mut Command, fd: i32) {
    command.env(SLOT_ENV, fd.to_string());
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
struct Fixture(Option<tempfile::TempDir>);
impl Fixture {
    fn new() -> Self {
        Self(Some(support::fixture()))
    }
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.path().join("account/cleanup-unconfirmed").exists() {
            eprintln!(
                "retaining unconfirmed fixture: {}",
                self.0.take().unwrap().keep().display()
            );
        }
    }
}
struct Process {
    child: std::process::Child,
    home: std::path::PathBuf,
    reaped: bool,
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        let mut groups = vec![self.child.id() as i32];
        if let Ok(group) = fs::read_to_string(self.home.join("worker-group")) {
            groups.push(group.parse().unwrap());
        }
        for group in &groups {
            if *group > 1 && *group != unsafe { libc::getpgrp() } {
                unsafe {
                    libc::kill(-*group, libc::SIGKILL);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        for group in groups {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let mut status = 0;
                while unsafe { libc::waitpid(-group, &mut status, libc::WNOHANG) } > 0 {}
                if unsafe { libc::kill(-group, 0) } < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                if Instant::now() >= deadline {
                    fs::write(
                        self.home.join("cleanup-unconfirmed"),
                        b"retain group/slot/state",
                    )
                    .unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // State can only be removed once no live waiter retains a ticket lock.
        let queue = self.home.join(".cache/develop-tools/heavy-check/queue");
        if let Ok(entries) = fs::read_dir(queue) {
            for entry in entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|e| e == "ticket"))
            {
                let file = File::open(entry.path()).unwrap();
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
                    fs::write(self.home.join("cleanup-unconfirmed"), b"retain live ticket")
                        .unwrap();
                }
            }
        }
    }
}
fn run(command: Command) -> (i32, String) {
    run_with(command, |_| {})
}
fn run_with(mut command: Command, after_spawn: impl FnOnce(u32)) -> (i32, String) {
    #[cfg(target_os = "linux")]
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let home = command.get_args().nth(1).unwrap().into();
    command.process_group(0);
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut process = Process {
        child: supervisor::spawn(&mut command).unwrap(),
        home,
        reaped: false,
    };
    after_spawn(process.child.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    let code = loop {
        if let Some(status) = process.child.try_wait().unwrap() {
            break status.code().unwrap_or(-1);
        }
        if Instant::now() >= deadline {
            panic!("inherited fixture timed out (possible self-wait)");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut output = String::new();
    process.reaped = true;
    process
        .child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    (code, output)
}
fn assert_rejected(command: Command, home: &Path) {
    let (code, _) = run(command);
    assert_eq!(code, 75, "invalid inherited identity must be unavailable");
    assert!(
        !home.join("payload").exists(),
        "rejection must precede payload spawn"
    );
}

#[test]
fn valid_inherited_slot_does_not_enqueue() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let home = support::directory(&fixture.path().join("account"));
    let slot = Ticket::register(state(&home))
        .unwrap()
        .wait(&MonotonicClock::default())
        .unwrap();
    let inherited = slot.inheritable_file().unwrap();
    let sequence = home.join(".cache/develop-tools/heavy-check/queue/next");
    let before = fs::read(&sequence).unwrap();
    let mut child = command(&home);
    pass_fd(&mut child, inherited.as_raw_fd());
    assert_eq!(run(child).0, 0);
    assert_eq!(
        fs::read(sequence).unwrap(),
        before,
        "reentry registered a second ticket"
    );
    assert!(home.join("payload").exists());
}

#[test]
fn env_only_or_other_open_description_is_rejected() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for case in [
        "env-only",
        "closed",
        "same-inode",
        "other-file",
        "pipe",
        "unlocked",
    ] {
        let fixture = Fixture::new();
        let home = support::directory(&fixture.path().join("account"));
        let prepared = state(&home);
        let slot_path = prepared.path().join("slot.lock");
        drop(prepared);
        let slot = (case != "unlocked").then(|| {
            Ticket::register(state(&home))
                .unwrap()
                .wait(&MonotonicClock::default())
                .unwrap()
        });
        let file = if case == "other-file" {
            File::create(home.join("other")).unwrap()
        } else {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&slot_path)
                .unwrap()
        };
        let mut child = command(&home);
        pass_fd(&mut child, file.as_raw_fd());
        if case == "env-only" {
            child.env(SLOT_ENV, "true");
        }
        if case == "closed" {
            let fd = file.as_raw_fd();
            unsafe {
                child.pre_exec(move || {
                    libc::close(fd);
                    Ok(())
                });
            }
        }
        let (pipe, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        if case == "pipe" {
            pass_fd(&mut child, pipe.as_raw_fd());
        }
        assert_rejected(child, &home);
        drop(slot);
    }
}

#[test]
fn supervised_worker_does_not_create_second_group() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let home = support::directory(&fixture.path().join("account"));
    let mut child = support::harness_command();
    child.arg("--inherited-supervisor").arg(&home);
    let (code, _) = run(child);
    assert_eq!(code, 0);
    let groups = fs::read_to_string(home.join("payload")).unwrap();
    let groups: Vec<i32> = groups
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(
        groups[0], groups[1],
        "worker/payload changed the externally owned group"
    );
    let outer_group: i32 = fs::read_to_string(home.join("worker-group"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        groups[0], outer_group,
        "nested reentry left the externally owned group"
    );
    assert_ne!(groups[0], unsafe { libc::getpgrp() });
}

#[test]
fn fake_control_channel_is_rejected_before_payload() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["byte", "forged-peer", "eof", "regular"] {
        let fixture = Fixture::new();
        let home = support::directory(&fixture.path().join("account"));
        let mut child = support::harness_command();
        child.arg("--fake-control").arg(&home).arg(mode);
        assert_rejected(child, &home);
    }
}

#[test]
fn fixture_child_closes_identity_without_unlocking_parent() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let home = support::directory(&fixture.path().join("account"));
    let slot = Ticket::register(state(&home))
        .unwrap()
        .wait(&MonotonicClock::default())
        .unwrap();
    let inherited = slot.inheritable_file().unwrap();
    let fd = inherited.as_raw_fd();
    let mut child = support::harness_command();
    child.arg("--isolated-probe").arg(&home);
    // Model an outer verification wrapper: close only the child's duplicate,
    // never LOCK_UN on the shared open file description.
    child.env_remove(SLOT_ENV);
    unsafe {
        child.pre_exec(move || {
            libc::close(fd);
            Ok(())
        });
    }
    assert_eq!(
        run(child).0,
        0,
        "child close released the parent's real slot"
    );
    assert_eq!(
        unsafe { libc::flock(slot.file().as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
}

#[test]
fn rejected_unlocked_description_does_not_take_the_slot() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    use develop_tools_core::heavy_check::inherited::InheritedSlot;
    let fixture = Fixture::new();
    let home = support::directory(&fixture.path().join("account"));
    let prepared = state(&home);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(prepared.path().join("slot.lock"))
        .unwrap();
    assert!(InheritedSlot::validate(&prepared, file.as_raw_fd()).is_err());
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "rejected candidate left an admission lock behind"
    );
}

#[test]
fn control_handshake_validates_version_group_and_short_frames() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::net::UnixStream;
    for mode in ["valid", "version", "group", "short"] {
        let fixture = Fixture::new();
        let home = support::directory(&fixture.path().join("account"));
        let (mut peer, socket) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut child = support::harness_command();
        child.arg("--control-worker").arg(&home);
        let fd = socket.as_raw_fd();
        child.env("DEVELOP_HEAVY_CHECK_CONTROL_FD", fd.to_string());
        unsafe {
            child.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let (code, _) = run_with(child, |pid| {
            let mut frame = b"HCSUP001".to_vec();
            frame.extend_from_slice(&(std::process::id() as i32).to_be_bytes());
            frame.extend_from_slice(&(pid as i32).to_be_bytes());
            if mode == "version" {
                frame[7] = b'2';
            }
            if mode == "group" {
                frame[15] ^= 1;
            }
            if mode == "short" {
                frame.truncate(9);
            }
            peer.write_all(&frame).unwrap();
            if mode == "valid" {
                let mut ack = [0; 16];
                peer.read_exact(&mut ack).unwrap();
                assert_eq!(ack.as_slice(), frame);
            }
            drop(peer);
        });
        assert_eq!(code, if mode == "valid" { 0 } else { 75 }, "{mode}");
        assert_eq!(home.join("payload").exists(), mode == "valid");
    }
}
