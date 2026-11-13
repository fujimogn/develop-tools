#[path = "support/heavy_check.rs"]
mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

static PROCESS_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn harness() -> PathBuf {
    let path = support::harness_executable();
    assert!(
        path.is_file(),
        "heavy-check harness missing at {}; run cargo test --manifest-path packages/cli/Cargo.toml -p develop-tools-core to build the example",
        path.display()
    );
    path
}

struct Waiter {
    child: Child,
    lines: Receiver<String>,
    ticket: u64,
}
impl Waiter {
    fn start(home: &Path) -> Self {
        Self::start_mode(home, "--queue")
    }
    fn start_mode(home: &Path, mode: &str) -> Self {
        let mut child = Command::new(harness())
            .arg(home)
            .args([".cache/develop-tools/heavy-check", "0", mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut waiter = Self {
            child,
            lines,
            ticket: 0,
        };
        let registered = waiter
            .lines
            .recv_timeout(Duration::from_secs(5))
            .expect("registration handshake");
        waiter.ticket = registered
            .strip_prefix("queued:")
            .expect("queued handshake")
            .parse()
            .unwrap();
        waiter
    }
    fn acquired(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("admission handshake");
            if line == "acquired" {
                break;
            }
        }
    }
    fn waiting(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(25)) {
                assert_ne!(
                    line, "acquired",
                    "waiter overtook the head or overlapping payloads"
                );
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "waiter exited before admission"
            );
        }
    }
    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }
    fn exit(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "fixture child failed to stop");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn release(&mut self) {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        assert_eq!(self.exit(), 0);
    }
}
impl Drop for Waiter {
    fn drop(&mut self) {
        // Always reap before the fixture TempDir is dropped, including SIGSTOP/panic paths.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn ticket_path(home: &Path, number: u64) -> std::path::PathBuf {
    home.join(format!(
        ".cache/develop-tools/heavy-check/queue/{number:020}.ticket"
    ))
}

#[test]
fn registered_waiters_start_in_ticket_order() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let mut holder = Waiter::start(&home);
    holder.acquired();
    let mut first = Waiter::start(&home);
    let mut second = Waiter::start(&home);
    let mut third = Waiter::start(&home);
    assert!(
        holder.ticket < first.ticket
            && first.ticket < second.ticket
            && second.ticket < third.ticket
    );
    first.waiting();
    second.waiting();
    third.waiting();
    holder.release();
    first.acquired();
    second.waiting();
    third.waiting();
    first.release();
    second.acquired();
    third.waiting();
    second.release();
    third.acquired();
    third.release();
    assert!(!ticket_path(&home, third.ticket).exists());
}

#[test]
fn dead_waiter_is_reclaimed_only_by_flock() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let mut holder = Waiter::start(&home);
    holder.acquired();
    let mut dead = Waiter::start(&home);
    let dead_ticket = ticket_path(&home, dead.ticket);
    // Deliberately misleading PID metadata: an unlocked ticket is dead even if
    // its recorded PID is currently alive/reused.
    fs::write(&dead_ticket, holder.child.id().to_string()).unwrap();
    dead.signal(libc::SIGKILL);
    dead.exit();
    assert!(dead_ticket.exists(), "SIGKILL must leave a stale ticket");
    let mut next = Waiter::start(&home);
    next.waiting();
    assert!(
        !dead_ticket.exists(),
        "unlocked dead ticket must be reclaimed even while slot is busy"
    );
    holder.release();
    next.acquired();
    next.release();
}

#[test]
fn stopped_waiter_is_not_dead() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let mut holder = Waiter::start(&home);
    holder.acquired();
    let mut stopped = Waiter::start_mode(&home, "--queue-stop");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut status = 0;
        let pid = unsafe {
            libc::waitpid(
                stopped.child.id() as i32,
                &mut status,
                libc::WUNTRACED | libc::WNOHANG,
            )
        };
        if pid > 0 {
            assert!(libc::WIFSTOPPED(status));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "waiter did not stop outside guard"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut next = Waiter::start(&home);
    holder.release();
    next.waiting();
    assert!(ticket_path(&home, stopped.ticket).exists());
    stopped.signal(libc::SIGCONT);
    stopped.acquired();
    next.waiting();
    stopped.release();
    next.acquired();
    next.release();
}

#[test]
fn lost_own_ticket_does_not_start_payload() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let mut holder = Waiter::start(&home);
    holder.acquired();
    let mut lost = Waiter::start(&home);
    lost.waiting();
    fs::remove_file(ticket_path(&home, lost.ticket)).unwrap();
    assert_eq!(lost.exit(), 75);
    assert!(!lost.lines.try_iter().any(|line| line == "acquired"));
    holder.release();
}

#[test]
fn logical_wait_beyond_three_hundred_seconds_has_no_internal_deadline() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let mut holder = Waiter::start(&home);
    holder.acquired();
    let mut waiter = Waiter::start(&home);
    loop {
        let line = waiter.lines.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(line, "acquired");
        if line
            .strip_prefix("clock:")
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|s| s > 300)
        {
            break;
        }
    }
    waiter.waiting();
    holder.release();
    waiter.acquired();
    waiter.release();
}

#[test]
fn only_explicit_slot_duplicate_survives_exec_and_holds_the_lock() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    use develop_tools_core::heavy_check::{MonotonicClock, queue::Ticket, state};
    use std::os::fd::AsRawFd;
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let prepare = || {
        state::prepare(
            &support::Source {
                account: home.clone(),
                home: None,
            },
            &state::StateLayout::default(),
            &state::NativeFilesystem,
        )
        .unwrap()
    };
    let state = prepare();
    let guard_fd = state.guard().as_raw_fd();
    let ticket = Ticket::register(state).unwrap();
    let ticket_fd = ticket.file().as_raw_fd();
    for fd in [guard_fd, ticket_fd] {
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    let slot = ticket.wait(&MonotonicClock::default()).unwrap();
    assert_ne!(
        unsafe { libc::fcntl(slot.file().as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    for fd in [guard_fd, ticket_fd] {
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) },
            -1,
            "private FD kept after admission"
        );
    }
    let inherited = slot.inheritable_file().unwrap();
    let output = Command::new(harness())
        .args([
            "--fd-probe",
            &inherited.as_raw_fd().to_string(),
            &slot.file().as_raw_fd().to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(slot);
    let mut next = Ticket::register(prepare()).unwrap();
    assert!(
        next.try_acquire().unwrap().is_none(),
        "duplicate must hold slot after original closes"
    );
    drop(inherited);
    assert!(next.try_acquire().unwrap().is_some());
}

#[test]
fn cancelling_a_ticket_removes_only_its_own_file() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    use develop_tools_core::heavy_check::{queue::Ticket, state};
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let prepare = || {
        state::prepare(
            &support::Source {
                account: home.clone(),
                home: None,
            },
            &state::StateLayout::default(),
            &state::NativeFilesystem,
        )
        .unwrap()
    };
    let first = Ticket::register(prepare()).unwrap();
    let second = Ticket::register(prepare()).unwrap();
    let first_path = ticket_path(&home, first.number());
    let second_path = ticket_path(&home, second.number());
    drop(first);
    assert!(!first_path.exists());
    assert!(second_path.exists());
    drop(second);
    assert!(!second_path.exists());
}

#[test]
fn sequence_publication_recovers_stale_tmp_and_refuses_overflow() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    use develop_tools_core::heavy_check::{queue::Ticket, state};
    use std::os::unix::fs::PermissionsExt;
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let prepare = || {
        state::prepare(
            &support::Source {
                account: home.clone(),
                home: None,
            },
            &state::StateLayout::default(),
            &state::NativeFilesystem,
        )
        .unwrap()
    };
    let first = Ticket::register(prepare()).unwrap();
    let first_number = first.number();
    drop(first);
    let queue = home.join(".cache/develop-tools/heavy-check/queue");
    fs::write(queue.join("next.tmp"), b"partial from dead registrar").unwrap();
    fs::set_permissions(queue.join("next.tmp"), fs::Permissions::from_mode(0o600)).unwrap();
    let second = Ticket::register(prepare()).unwrap();
    assert_eq!(second.number(), first_number + 1);
    drop(second);
    assert!(!queue.join("next.tmp").exists());
    fs::write(queue.join("next"), format!("{}\n", u64::MAX)).unwrap();
    assert_eq!(Ticket::register(prepare()).err().unwrap().exit_code(), 75);
    fs::write(queue.join("next"), b"corrupt\n").unwrap();
    assert_eq!(Ticket::register(prepare()).err().unwrap().exit_code(), 75);
}

#[test]
fn unsafe_queue_file_never_admits_or_unlinks_the_target() {
    let _serial = PROCESS_TEST.lock().unwrap_or_else(|e| e.into_inner());
    use develop_tools_core::heavy_check::{queue::Ticket, state};
    use std::os::unix::fs::symlink;
    let root = support::fixture();
    let home = support::directory(&root.path().join("account"));
    let state = state::prepare(
        &support::Source {
            account: home.clone(),
            home: None,
        },
        &state::StateLayout::default(),
        &state::NativeFilesystem,
    )
    .unwrap();
    let target = root.path().join("untouched");
    fs::write(&target, b"preserve").unwrap();
    let next = home.join(".cache/develop-tools/heavy-check/queue/next");
    symlink(&target, &next).unwrap();
    assert_eq!(Ticket::register(state).err().unwrap().exit_code(), 75);
    assert!(next.is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
}
