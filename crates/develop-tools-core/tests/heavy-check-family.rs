#[path = "support/heavy_check.rs"]
#[allow(dead_code)]
mod support;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use std::cell::RefCell;

static CLEANUP_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct Fixture(Option<tempfile::TempDir>);
impl Fixture {
    fn new() -> Self {
        CLEANUP_FAILED.store(false, std::sync::atomic::Ordering::Relaxed);
        Self(Some(support::fixture()))
    }
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if CLEANUP_FAILED.load(std::sync::atomic::Ordering::Relaxed) {
            let path = self.0.take().unwrap().keep();
            eprintln!("unconfirmed fixture retained: {}", path.display());
            if !std::thread::panicking() {
                panic!("fixture cleanup refused");
            }
        }
    }
}
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Runner {
    child: Child,
    lines: Receiver<String>,
    group: Option<i32>,
    seen: RefCell<Vec<String>>,
    home: std::path::PathBuf,
    confirmed_stopped: bool,
}
impl Runner {
    fn start(home: &Path, mode: &str) -> Self {
        // Adopt fixture orphans on Linux so panic cleanup also reaps grandchildren.
        #[cfg(target_os = "linux")]
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        let mut command = Command::new(
            std::env::var_os("HEAVY_CHECK_TEST_HARNESS")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_exe()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("examples/heavy-check-harness")
                }),
        );
        command
            .args([
                std::ffi::OsStr::new("--family"),
                home.as_os_str(),
                std::ffi::OsStr::new(mode),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = develop_tools_core::heavy_check::supervisor::spawn(&mut command).unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut runner = Self {
            child,
            lines,
            group: None,
            seen: RefCell::new(Vec::new()),
            home: home.to_owned(),
            confirmed_stopped: false,
        };
        let line = runner.line();
        runner.group = Some(
            line.strip_prefix("group:")
                .expect("family group handshake")
                .parse()
                .unwrap(),
        );
        runner
    }
    fn line(&self) -> String {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(5))
            .expect("family handshake");
        self.seen.borrow_mut().push(line.clone());
        line
    }
    fn until(&self, prefix: &str) -> String {
        if let Some(line) = self
            .seen
            .borrow()
            .iter()
            .find(|line| line.starts_with(prefix))
        {
            return line.clone();
        }
        loop {
            let line = self.line();
            if line.starts_with(prefix) {
                return line;
            }
        }
    }
    fn alive(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "supervisor released family early"
        );
    }
    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }
    fn exit(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // A successful supervisor return certifies complete group reap.
                self.confirmed_stopped = status.code().is_some_and(|code| code != 101);
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "supervisor failed to stop");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        self.seen.borrow_mut().extend(self.lines.try_iter());
        // Record escaped groups as well, so the group-separation mutation cannot
        // leak a descendant when the assertion deliberately fails.
        let mut groups: Vec<i32> = if self.confirmed_stopped {
            Vec::new()
        } else {
            self.group.into_iter().collect()
        };
        if std::thread::panicking() && !self.confirmed_stopped {
            for line in self.seen.borrow().iter() {
                if let Some(pid) = line
                    .strip_prefix("descendant:")
                    .and_then(|s| s.parse::<i32>().ok())
                {
                    let group = unsafe { libc::getpgid(pid) };
                    if group > 1 && group != unsafe { libc::getpgrp() } && !groups.contains(&group)
                    {
                        groups.push(group);
                    }
                }
            }
        }
        for group in &groups {
            unsafe {
                libc::kill(-*group, libc::SIGKILL);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        for group in groups {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let mut status = 0;
                while unsafe { libc::waitpid(-group, &mut status, libc::WNOHANG) } > 0 {}
                if unsafe { libc::kill(-group, 0) } != 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                if Instant::now() >= deadline {
                    CLEANUP_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(number) = self.seen.borrow().iter().find_map(|line| {
            line.strip_prefix("queued:")
                .and_then(|n| n.parse::<u64>().ok())
        }) {
            let ticket = self.home.join(format!(
                ".cache/develop-tools/heavy-check/queue/{number:020}.ticket"
            ));
            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(ticket)
            {
                use std::os::fd::AsRawFd;
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    CLEANUP_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn adopted_other_group_is_reaped_while_family_remains_live() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut runner = Runner::start(&home, "adopted-orphan");
    runner.until("primary-exited");
    // Both the group member keeping the slot and the escaped orphan report PID.
    while runner
        .seen
        .borrow()
        .iter()
        .filter(|line| line.starts_with("descendant:"))
        .count()
        < 2
    {
        runner.line();
    }
    let mut escaped = None;
    for line in runner.seen.borrow().iter() {
        if let Some(pid) = line.strip_prefix("descendant:") {
            let pid = pid.parse::<i32>().unwrap();
            if unsafe { libc::getpgid(pid) } == pid {
                escaped = Some(pid);
            }
        }
    }
    let escaped = escaped.expect("separate process group descendant");
    let proc = std::path::PathBuf::from(format!("/proc/{escaped}/status"));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let status = std::fs::read_to_string(&proc).unwrap();
        if status.lines().any(|line| {
            line.strip_prefix("PPid:")
                .is_some_and(|parent| parent.trim() == runner.child.id().to_string())
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "orphan was not adopted");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(home.join("exit-orphan"), b"exit").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while proc.exists() && Instant::now() < deadline {
        runner.alive();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !proc.exists(),
        "adopted other-group child remained a zombie"
    );
    runner.alive();
    assert_eq!(
        unsafe { libc::kill(-runner.group.unwrap(), libc::SIGKILL) },
        0
    );
    assert_eq!(runner.exit(), 0, "orphan exit replaced worker exit");
}

#[test]
fn primary_exit_keeps_descendant_slot() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut first = Runner::start(&home, "closed-slot");
    first.until("descendant:");
    first.until("primary-exited");
    let mut next = Runner::start(&home, "short");
    next.until("queued:");
    std::thread::sleep(Duration::from_millis(350));
    first.alive();
    next.alive();
    assert!(
        !next.lines.try_iter().any(|line| line == "acquired"),
        "descendant lost slot after primary exit"
    );
    assert_eq!(
        unsafe { libc::kill(-first.group.unwrap(), libc::SIGKILL) },
        0
    );
    assert_eq!(first.exit(), 0);
    next.until("acquired");
    assert_eq!(next.exit(), 0);
}

#[test]
fn runner_sigkill_keeps_descendant_slot() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut first = Runner::start(&home, "descendant");
    first.until("descendant:");
    first.until("primary-exited");
    first.signal(libc::SIGKILL);
    assert_eq!(first.exit(), -1);
    let mut next = Runner::start(&home, "short");
    next.until("queued:");
    std::thread::sleep(Duration::from_millis(350));
    next.alive();
    assert!(
        !next.lines.try_iter().any(|line| line == "acquired"),
        "runner SIGKILL released descendant slot"
    );
    unsafe {
        libc::kill(-first.group.unwrap(), libc::SIGKILL);
    }
    next.until("acquired");
    assert_eq!(next.exit(), 0);
}

#[test]
fn term_escalates_after_five_seconds() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut runner = Runner::start(&home, "ignore-term");
    runner.until("descendant:");
    let start = Instant::now();
    runner.signal(libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(300));
    runner.alive();
    assert_eq!(runner.exit(), 128 + libc::SIGTERM);
    assert!(
        start.elapsed() >= Duration::from_secs(5),
        "TERM grace was shortened"
    );
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "TERM did not escalate"
    );
    assert_eq!(
        unsafe { libc::kill(-runner.group.unwrap(), 0) },
        -1,
        "group not reaped"
    );
}

#[test]
fn unconfirmed_stop_prevents_cleanup() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut runner = Runner::start(&home, "unconfirmed");
    runner.until("descendant:");
    runner.until("unconfirmed:retained");
    runner.alive();
    let mut next = Runner::start(&home, "short");
    next.until("queued:");
    std::thread::sleep(Duration::from_millis(350));
    next.alive();
    assert!(
        !next.lines.try_iter().any(|line| line == "acquired"),
        "unconfirmed stop released slot"
    );
    assert!(
        home.join("retained-fixture").exists(),
        "cleanup ran before stop confirmation"
    );
    unsafe {
        libc::kill(-runner.group.unwrap(), libc::SIGKILL);
    }
    assert_eq!(runner.exit(), 0);
    next.until("acquired");
    assert_eq!(next.exit(), 0);
}

#[test]
fn injected_clock_escalates_at_five_seconds() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let mut runner = Runner::start(&home, "logical-term");
    runner.until("descendant:");
    let stopped = runner.until("logical-stopped:");
    let elapsed: u64 = stopped
        .strip_prefix("logical-stopped:")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(elapsed, 5, "logical grace was {elapsed}");
    assert_eq!(runner.exit(), 143);
}

#[test]
fn normal_128_plus_exit_codes_do_not_start_stop_budget() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (mode, code) in [("exit130", 130), ("exit137", 137)] {
        let root = Fixture::new();
        let home = support::directory(&root.path().join("account"));
        let mut first = Runner::start(&home, mode);
        first.until("descendant:");
        first.until("primary-exited");
        let mut next = Runner::start(&home, "short");
        next.until("queued:");
        std::thread::sleep(Duration::from_millis(5500));
        first.alive();
        next.alive();
        assert!(
            !next.lines.try_iter().any(|line| line == "acquired"),
            "normal exit {code} released live descendant slot"
        );
        unsafe {
            libc::kill(-first.group.unwrap(), libc::SIGKILL);
        }
        assert_eq!(first.exit(), code);
        next.until("acquired");
        assert_eq!(next.exit(), 0);
    }
}

#[test]
fn inherited_worker_channel_restores_cloexec() {
    use develop_tools_core::heavy_check::supervisor::{SlotSender, slot_channel};
    let (_receiver, sender) = slot_channel().unwrap();
    let inherited = unsafe { libc::dup(sender.fd()) };
    assert!(inherited >= 0);
    let worker_sender = unsafe { SlotSender::from_inherited_fd(inherited) }.unwrap();
    assert_ne!(
        unsafe { libc::fcntl(worker_sender.fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
}

#[test]
fn worker_stops_when_supervisor_dies_before_start_handshake() {
    use develop_tools_core::heavy_check::supervisor::{slot_channel, spawn};
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = Fixture::new();
    let home = support::directory(&root.path().join("account"));
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/heavy-check-harness");
    let (receiver, sender) = slot_channel().unwrap();
    let mut command = Command::new(executable);
    command
        .arg("--worker")
        .arg(&home)
        .arg("short")
        .arg(sender.fd().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sender.configure_worker(&mut command);
    struct Probe(Child);
    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut worker = Probe(spawn(&mut command).unwrap());
    drop(sender);
    drop(receiver);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if worker.0.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker survived supervisor startup death"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !home.join(".cache").exists(),
        "worker touched state before supervisor readiness"
    );
}
