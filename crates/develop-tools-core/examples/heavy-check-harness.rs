//! Non-Release fixture entry point. Not registered in the Release manifest.
//! State probes and independent FIFO process fixtures; never a production CLI.
use develop_tools_core::heavy_check::queue::Ticket;
use develop_tools_core::heavy_check::state::{
    self, Account, AccountSource, NativeFilesystem, OsAccount, StateError, StateLayout,
};
use develop_tools_core::heavy_check::{Clock, MonotonicClock};
use std::cell::Cell;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

struct FixtureAccount {
    home: PathBuf,
}
impl AccountSource for FixtureAccount {
    fn account(&self) -> Result<Account, StateError> {
        Ok(Account {
            home: self.home.clone(),
            uid: unsafe { libc::geteuid() },
        })
    }
    fn environment_home(&self) -> Option<PathBuf> {
        OsAccount.environment_home()
    }
}
struct FixtureClock(Duration);
impl Clock for FixtureClock {
    fn now(&self) -> Duration {
        self.0
    }
    fn sleep(&self, _: Duration) {}
}
fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--cli-supervised")
    {
        use develop_tools_core::heavy_check::supervisor::{self, Family, Supervision};
        let _signals = Supervision::install().unwrap();
        let mut args = std::env::args_os().skip(2);
        let _home = args.next().unwrap();
        let mut command = std::process::Command::new(args.next().unwrap());
        command.args([
            "--executor-context=headless-sandbox",
            "/bin/sh",
            "-c",
            "printf external",
        ]);
        let (receiver, sender) = supervisor::slot_channel().unwrap();
        sender.configure_worker(&mut command);
        let child = supervisor::spawn(&mut command).unwrap();
        drop(sender);
        let mut family = Family::new(child, receiver).unwrap();
        family.start_worker().unwrap();
        std::process::exit(wait_preserving_family(
            &mut family,
            &MonotonicClock::default(),
        ));
    }

    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--close-slot-helper" || a == "--closed-fd-descendant")
    {
        let mut args = std::env::args_os().skip(1);
        let role = args.next().unwrap();
        let home = PathBuf::from(args.next().unwrap());
        if role == "--close-slot-helper" {
            use develop_tools_core::heavy_check::{inherited::SLOT_FD_ENV, supervisor};
            let fd: i32 = std::env::var(SLOT_FD_ENV).unwrap().parse().unwrap();
            assert_eq!(unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) }, 0);
            assert_eq!(unsafe { libc::close(fd) }, 0);
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .arg("--closed-fd-descendant")
                .arg(home)
                .env_remove(SLOT_FD_ENV);
            drop(supervisor::spawn(&mut child).unwrap());
        } else {
            std::fs::write(
                home.join("closed-fd-descendant"),
                std::process::id().to_string(),
            )
            .unwrap();
            let start = std::time::Instant::now();
            while !home.join("release-closed-fd").exists()
                && start.elapsed() < Duration::from_secs(5)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        return;
    }

    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--cli-fixture")
    {
        let mut args = std::env::args_os().skip(2);
        let home = PathBuf::from(args.next().unwrap());
        let mode = args.next().unwrap();
        let clock: Box<dyn Clock> = if mode == "step" {
            Box::new(CliClock(Cell::new(Duration::ZERO)))
        } else {
            Box::new(MonotonicClock::default())
        };
        std::process::exit(develop_tools_core::heavy_check::run(
            args.collect(),
            &FixtureAccount { home: home.clone() },
            &CliFilesystem {
                denied_home: (mode == "denied").then_some(home),
            },
            clock.as_ref(),
        ));
    }

    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a.to_string_lossy().starts_with("--io-") || a == "--pty-driver")
    {
        std::process::exit(io_main().unwrap_or_else(|e| {
            eprintln!("{e}");
            75
        }));
    }
    let mut args = std::env::args_os().skip(1);
    if std::env::args_os().nth(1).is_some_and(|arg| {
        arg == "--family" || arg == "--worker" || arg == "--primary" || arg == "--descendant"
    }) {
        std::process::exit(family_main());
    }
    if std::env::args_os().nth(1).is_some_and(|arg| {
        arg == "--inherited"
            || arg == "--inherited-supervisor"
            || arg == "--inherited-worker"
            || arg == "--inherited-body"
            || arg == "--fake-control"
            || arg == "--isolated-probe"
            || arg == "--control-worker"
    }) {
        std::process::exit(inherited_main().unwrap_or_else(|error| {
            eprintln!("{error}");
            75
        }));
    }
    let Some(home) = args.next() else {
        std::process::exit(75)
    };
    if home == "--fd-probe" {
        use std::os::fd::RawFd;
        let kept: RawFd = args.next().unwrap().to_str().unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::fcntl(kept, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_eq!(
            unsafe { libc::flock(kept, libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        for fd in args {
            let fd: RawFd = fd.to_str().unwrap().parse().unwrap();
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_GETFD) },
                -1,
                "private FD survived exec"
            );
        }
        return;
    }
    let relative = args.next().map(PathBuf::from);
    let clock: Box<dyn Clock> = match args.next() {
        Some(value) => match value.to_str().and_then(|s| s.parse().ok()) {
            Some(ms) => Box::new(FixtureClock(Duration::from_millis(ms))),
            None => std::process::exit(75),
        },
        None => Box::new(MonotonicClock::default()),
    };
    let queue_mode = match args.next() {
        Some(arg) if arg == "--queue" => Some(false),
        Some(arg) if arg == "--queue-stop" => Some(true),
        None => None,
        _ => std::process::exit(75),
    };
    if args.next().is_some() {
        std::process::exit(75);
    }
    let layout = match relative.map(StateLayout::new).transpose() {
        Ok(layout) => layout.unwrap_or_default(),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    };
    match state::prepare(
        &FixtureAccount { home: home.into() },
        &layout,
        &NativeFilesystem,
    ) {
        Ok(state) if queue_mode.is_some() => {
            let result = (|| {
                let ticket = Ticket::register(state)?;
                println!("queued:{}", ticket.number());
                io::stdout().flush().unwrap();
                let clock = AdvancingClock(Cell::new(clock.now()), Cell::new(queue_mode.unwrap()));
                let slot = ticket.wait(&clock)?;
                let inherited = slot.inheritable_file().unwrap();
                use std::os::fd::AsRawFd;
                assert_eq!(
                    unsafe { libc::fcntl(inherited.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                    0
                );
                println!("acquired");
                io::stdout().flush().unwrap();
                let mut release = String::new();
                io::stdin().read_line(&mut release).unwrap();
                drop(inherited);
                drop(slot);
                Ok::<_, StateError>(())
            })();
            if let Err(error) = result {
                eprintln!("{error}");
                std::process::exit(error.exit_code());
            }
        }
        Ok(state) => println!(
            "{} {:?} {}",
            state.path().display(),
            state.scope(),
            clock.now().as_millis()
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}

// Each retry advances logical time by a minute, while retaining a real scheduling interval.
struct AdvancingClock(Cell<Duration>, Cell<bool>);
impl Clock for AdvancingClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
    fn sleep(&self, interval: Duration) {
        self.0.set(self.now() + Duration::from_secs(60));
        println!("clock:{}", self.now().as_secs());
        io::stdout().flush().unwrap();
        // Stop outside queue.lock so the fixture tests ticket liveness, not guard ownership.
        if self.1.replace(false) {
            unsafe {
                libc::raise(libc::SIGSTOP);
            }
        }
        std::thread::sleep(interval);
    }
}

fn family_main() -> i32 {
    use develop_tools_core::heavy_check::supervisor::{self, Family, SlotSender, Supervision};
    use std::process::Command;
    let mut args = std::env::args_os().skip(1);
    let role = args.next().unwrap();
    let executable = std::env::current_exe().unwrap();
    if role == "--family" {
        let home = PathBuf::from(args.next().unwrap());
        let mode = args.next().unwrap();
        let _signals = Supervision::install().unwrap();
        let (receiver, sender) = supervisor::slot_channel().unwrap();
        let mut command = Command::new(executable);
        command
            .arg("--worker")
            .arg(&home)
            .arg(&mode)
            .arg(sender.fd().to_string());
        sender.configure_worker(&mut command);
        let worker = supervisor::spawn(&mut command).unwrap();
        drop(sender);
        let mut family = Family::new(worker, receiver).unwrap();
        println!("group:{}", family.group());
        io::stdout().flush().unwrap();
        family.start_worker().unwrap();
        let clock = MonotonicClock::default();
        if mode == "unconfirmed" {
            // A live group cannot be confirmed stopped. Its state stays intact.
            std::fs::write(home.join("retained-fixture"), b"keep until group stopped").unwrap();
            let result = family.cleanup_if_stopped(&clock, Duration::ZERO, || {
                std::fs::remove_file(home.join("retained-fixture")).unwrap();
            });
            if result.is_err() {
                println!("unconfirmed:retained");
                io::stdout().flush().unwrap();
            }
        }
        let code = if mode == "logical-term" {
            while !home.join("descendant-ready").exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            let clock = StepClock(Cell::new(Duration::ZERO));
            family.signal(libc::SIGTERM, &clock).unwrap();
            let code = wait_preserving_family(&mut family, &clock);
            println!("logical-stopped:{}", clock.now().as_secs());
            code
        } else {
            wait_preserving_family(&mut family, &clock)
        };
        println!("worker-exit:{}", family.worker_exit().unwrap_or(1));
        io::stdout().flush().unwrap();
        return code;
    }
    if role == "--worker" {
        let home = PathBuf::from(args.next().unwrap());
        let mode = args.next().unwrap();
        let fd = args.next().unwrap().to_str().unwrap().parse().unwrap();
        // This process was put in its group by the supervisor; never setpgid/setsid here.
        let sender = unsafe { SlotSender::from_inherited_fd(fd) }.unwrap();
        sender.wait_for_supervisor().unwrap();
        let state = state::prepare(
            &FixtureAccount { home: home.clone() },
            &StateLayout::default(),
            &NativeFilesystem,
        )
        .unwrap();
        let ticket = Ticket::register(state).unwrap();
        println!("queued:{}", ticket.number());
        io::stdout().flush().unwrap();
        let slot = ticket.wait(&MonotonicClock::default()).unwrap();
        sender.send(slot.file()).unwrap();
        drop(sender);
        println!("acquired");
        io::stdout().flush().unwrap();
        let inherited = slot.inheritable_file().unwrap();
        let mut command = Command::new(executable);
        use std::os::fd::AsRawFd;
        command
            .arg("--primary")
            .arg(if mode == "logical-term" {
                std::ffi::OsStr::new("ignore-term")
            } else {
                mode.as_os_str()
            })
            .arg(inherited.as_raw_fd().to_string())
            .arg(&home);
        if mode == "ignore-term" || mode == "logical-term" {
            ignore_signals();
        }
        let mut primary = supervisor::spawn(&mut command).unwrap();
        let status = primary.wait().unwrap();
        println!("primary-exited");
        io::stdout().flush().unwrap();
        drop(inherited);
        drop(slot);
        return status.code().unwrap_or(1);
    }
    let mode = args.next().unwrap();
    let inherited: i32 = args.next().unwrap().to_str().unwrap().parse().unwrap();
    let home = PathBuf::from(args.next().unwrap());
    if mode == "ignore-term" {
        ignore_signals();
    }
    if role == "--primary" {
        if mode == "short" {
            return 0;
        }
        let mut command = Command::new(executable);
        command
            .arg("--descendant")
            .arg(&mode)
            .arg(inherited.to_string())
            .arg(&home);
        let mut descendant = supervisor::spawn(&mut command).unwrap();
        if mode == "ignore-term" {
            return descendant.wait().unwrap().code().unwrap_or(1);
        }
        // Intentionally orphan a descendant and close its inherited stdout.
        // Its slot FD survives exec and its lifetime is not the pipe lifetime.
        drop(descendant);
        return if mode == "exit130" {
            130
        } else if mode == "exit137" {
            137
        } else {
            0
        };
    }
    if mode == "closed-slot" || mode == "exit130" || mode == "exit137" {
        unsafe {
            libc::close(inherited);
        }
    }
    println!("descendant:{}", std::process::id());
    io::stdout().flush().unwrap();
    std::fs::write(home.join("descendant-ready"), b"ready").unwrap();
    unsafe {
        libc::close(libc::STDOUT_FILENO);
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
fn ignore_signals() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            libc::signal(signal, libc::SIG_IGN);
        }
    }
}

struct StepClock(Cell<Duration>);
impl Clock for StepClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
    fn sleep(&self, _: Duration) {
        self.0.set(self.now() + Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_preserving_family(
    family: &mut develop_tools_core::heavy_check::supervisor::Family,
    clock: &dyn Clock,
) -> i32 {
    match family.wait(clock, Duration::from_secs(5)) {
        Ok(code) => code,
        Err(error) => {
            // An error must not panic/drop the only supervisor-held slot.
            eprintln!("{error}; retaining group {} and fixture", family.group());
            let recovery_clock = MonotonicClock::default();
            while family
                .confirm_stopped(&recovery_clock, Duration::from_secs(1))
                .is_err()
            {}
            1
        }
    }
}

fn inherited_main() -> Result<i32, Box<dyn std::error::Error>> {
    use develop_tools_core::heavy_check::inherited::{InheritedSlot, SLOT_FD_ENV};
    use develop_tools_core::heavy_check::supervisor::{self, Family, SlotSender, Supervision};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::process::Command;
    let mut args = std::env::args_os().skip(1);
    let role = args.next().unwrap();
    let home = PathBuf::from(args.next().unwrap());
    let executable = std::env::current_exe()?;
    if role == "--control-worker" {
        let sender = SlotSender::from_environment()?.ok_or("missing control identity")?;
        sender.wait_for_supervisor()?;
        std::fs::write(home.join("payload"), b"authenticated control")?;
        return Ok(0);
    }
    if role == "--inherited-body" {
        let expected = args.next().unwrap();
        // Validate again after exec: capability remains live down the family.
        let state = state::prepare(
            &FixtureAccount { home: home.clone() },
            &StateLayout::default(),
            &NativeFilesystem,
        )?;
        let _slot = InheritedSlot::from_environment(&state)?.ok_or("missing inherited slot")?;
        std::fs::write(
            home.join("payload"),
            format!("{} {}", expected.to_string_lossy(), unsafe {
                libc::getpgrp()
            }),
        )?;
        return Ok(0);
    }
    if role == "--fake-control" {
        let mode = args.next().unwrap();
        let (mut peer, socket) = UnixStream::pair()?;
        let regular = std::fs::File::create(home.join("control-file"))?;
        let fd = if mode == "regular" {
            regular.as_raw_fd()
        } else {
            socket.as_raw_fd()
        };
        if mode == "byte" {
            peer.write_all(&[1])?;
        }
        if mode == "forged-peer" {
            let mut frame = b"HCSUP001".to_vec();
            frame.extend_from_slice(&unsafe { libc::getppid() }.to_be_bytes());
            frame.extend_from_slice(&unsafe { libc::getpgrp() }.to_be_bytes());
            peer.write_all(&frame)?;
        }
        drop(peer);
        let sender = SlotSender::from_control_fd(fd)?;
        sender.wait_for_supervisor()?;
        std::fs::write(home.join("payload"), b"forged control accepted")?;
        return Ok(0);
    }
    if role == "--inherited-supervisor" {
        let _signals = Supervision::install()?;
        let (receiver, sender) = supervisor::slot_channel()?;
        let mut command = Command::new(&executable);
        command
            .arg("--inherited-worker")
            .arg(&home)
            .arg(sender.fd().to_string());
        sender.configure_worker(&mut command);
        let worker = supervisor::spawn(&mut command)?;
        drop(sender);
        let mut family = Family::new(worker, receiver)?;
        std::fs::write(home.join("worker-group"), family.group().to_string())?;
        // Retain the family on handshake error as on wait failure.
        if family.start_worker().is_err() {
            family.signal(libc::SIGKILL, &MonotonicClock::default())?;
        }
        return Ok(wait_preserving_family(
            &mut family,
            &MonotonicClock::default(),
        ));
    }
    let state = state::prepare(
        &FixtureAccount { home: home.clone() },
        &StateLayout::default(),
        &NativeFilesystem,
    )?;
    if role == "--isolated-probe" {
        if std::env::var_os(SLOT_FD_ENV).is_some() {
            return Err("fixture retained real identity".into());
        }
        let result =
            unsafe { libc::flock(state.slot().as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        return if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            Ok(0)
        } else {
            Err("outer real slot was unlocked".into())
        };
    }
    if role == "--inherited-worker" {
        let sender = SlotSender::from_environment()?.ok_or("missing control identity")?;
        sender.wait_for_supervisor()?;
        let slot = Ticket::register(state)?.wait(&MonotonicClock::default())?;
        sender.send(slot.file())?;
        drop(sender);
        let fresh = state::prepare(
            &FixtureAccount { home: home.clone() },
            &StateLayout::default(),
            &NativeFilesystem,
        )?;
        let inherited = InheritedSlot::validate(&fresh, slot.file().as_raw_fd())?;
        let mut command = Command::new(&executable);
        command.arg("--inherited").arg(&home);
        inherited.configure_payload(&mut command);
        return Ok(supervisor::spawn(&mut command)?.wait()?.code().unwrap_or(1));
    }
    let inherited = InheritedSlot::from_environment(&state)?.ok_or("missing inherited slot")?;
    let group = unsafe { libc::getpgrp() };
    let mut command = Command::new(&executable);
    command
        .arg("--inherited-body")
        .arg(&home)
        .arg(group.to_string());
    inherited.configure_payload(&mut command);
    Ok(supervisor::spawn(&mut command)?.wait()?.code().unwrap_or(1))
}

// I5 fixture roles keep protocol bytes off the payload's standard streams.
fn io_main() -> Result<i32, Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    let mut args = std::env::args_os().skip(1);
    let role = args.next().unwrap();
    let home = PathBuf::from(args.next().unwrap());
    if role == "--pty-driver" {
        return pty_driver(&home, args.next().unwrap().to_str().unwrap());
    }
    if role == "--io-supervisor" {
        use develop_tools_core::heavy_check::{
            supervisor::{self, Family, Supervision},
            terminal::Foreground,
        };
        use std::process::Command;
        let payload = args.next().unwrap();
        if payload == "--io-tty" {
            wait_file(&home.join("go"))?;
        }
        let _signals = Supervision::install()?;
        let (receiver, sender) = supervisor::slot_channel()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--io-worker")
            .arg(&home)
            .arg(payload)
            .args(args);
        sender.configure_worker(&mut command);
        let worker = supervisor::spawn(&mut command)?;
        drop(sender);
        let mut family = Family::new(worker, receiver)?;
        std::fs::write(home.join("worker-group"), family.group().to_string())?;
        let mut foreground = match Foreground::transfer(family.group()) {
            Ok(foreground) => foreground,
            Err(error) => {
                family.signal(libc::SIGKILL, &MonotonicClock::default())?;
                wait_preserving_family(&mut family, &MonotonicClock::default());
                return Err(error.into());
            }
        };
        if family.start_worker().is_err() {
            family.signal(libc::SIGKILL, &MonotonicClock::default())?;
        }
        let code = wait_preserving_family(&mut family, &MonotonicClock::default());
        foreground.restore()?;
        return Ok(code);
    }
    if role == "--io-worker" {
        use develop_tools_core::heavy_check::{
            inherited::InheritedSlot,
            supervisor::{self, SlotSender},
        };
        use std::os::fd::AsRawFd;
        use std::process::Command;
        let control_fd: i32 =
            std::env::var(develop_tools_core::heavy_check::inherited::CONTROL_FD_ENV)?.parse()?;
        let sender = SlotSender::from_environment()?.ok_or("missing control identity")?;
        sender.wait_for_supervisor()?;
        let state = state::prepare(
            &FixtureAccount { home: home.clone() },
            &StateLayout::default(),
            &NativeFilesystem,
        )?;
        let slot = Ticket::register(state)?.wait(&MonotonicClock::default())?;
        sender.send(slot.file())?;
        drop(sender);
        let payload = args.next().unwrap();
        let mut command = Command::new(std::env::current_exe()?);
        command.arg(&payload).arg(&home).args(args);
        // Probe the ordinary spawn path without configure_payload too.
        let fresh = state::prepare(
            &FixtureAccount { home: home.clone() },
            &StateLayout::default(),
            &NativeFilesystem,
        )?;
        let inherited = InheritedSlot::validate(&fresh, slot.file().as_raw_fd())?;
        if payload != "--io-control-probe" {
            inherited.configure_payload(&mut command);
        } else {
            command.arg(control_fd.to_string());
        }
        return Ok(supervisor::spawn(&mut command)?.wait()?.code().unwrap_or(1));
    }
    if role == "--io-echo" {
        let mut out = io::stdout().lock();
        for arg in args {
            let bytes = arg.as_bytes();
            out.write_all(&(bytes.len() as u32).to_be_bytes())?;
            out.write_all(bytes)?;
        }
        out.write_all(&u32::MAX.to_be_bytes())?;
        io::copy(&mut io::stdin().lock(), &mut out)?;
        eprintln!("payload-stderr");
        return Ok(0);
    }
    if role == "--io-control-probe" {
        assert!(
            std::env::var_os(develop_tools_core::heavy_check::inherited::CONTROL_FD_ENV).is_none(),
            "control identity leaked"
        );
        let fd: i32 = args.next().unwrap().to_str().unwrap().parse()?;
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) },
            -1,
            "control FD survived exec"
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
        return Ok(0);
    }
    if role == "--io-ignore-int" {
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let group = unsafe { libc::getpgrp() };
        std::fs::write(home.join("worker-group"), group.to_string())?;
        std::fs::write(
            home.join("payload-ready"),
            format!("{group} {}", unsafe { libc::tcgetpgrp(0) }),
        )?;
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if role == "--io-tty" {
        let group = unsafe { libc::getpgrp() };
        let foreground = unsafe { libc::tcgetpgrp(0) };
        std::fs::write(home.join("payload-ready"), format!("{group} {foreground}"))?;
        if args.next().unwrap() == "background" {
            return Ok(0);
        }
        let mut byte = [0];
        io::stdin().read_exact(&mut byte)?;
        std::fs::write(home.join("payload-read"), byte)?;
        return Ok(0);
    }
    Err("unknown IO role".into())
}

fn wait_file(path: &std::path::Path) -> io::Result<Vec<u8>> {
    let start = std::time::Instant::now();
    loop {
        match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
            _ if start.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                return Err(io::Error::other(format!(
                    "handshake missing: {}",
                    path.display()
                )));
            }
        }
    }
}

fn pty_driver(home: &std::path::Path, mode: &str) -> Result<i32, Box<dyn std::error::Error>> {
    use develop_tools_core::heavy_check::supervisor;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    let _signals = supervisor::Supervision::install()?;
    let mut master = -1;
    let mut slave = -1;
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    for descriptor in [&master, &slave] {
        assert_eq!(
            unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    assert!(unsafe { libc::setsid() } > 0);
    assert_eq!(
        unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSCTTY as _, 0) },
        0
    );
    unsafe {
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let mut command = if mode == "ctrl-c" {
        let mut command =
            Command::new(std::env::var_os("HEAVY_CHECK_CLI").ok_or("missing CLI fixture")?);
        command
            .env("HOME", home)
            .env("GITHUB_ACTIONS", "false")
            .arg("--executor-context=headless-sandbox")
            .arg(std::env::current_exe()?)
            .arg("--io-ignore-int")
            .arg(home);
        command
    } else {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--io-supervisor")
            .arg(home)
            .arg("--io-tty")
            .arg(mode);
        command
    };
    command.process_group(0);
    let fd = slave.as_raw_fd();
    let close_master = master.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::close(close_master);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = supervisor::spawn(&mut command)?;
    let child_group = child.id() as i32;
    let own = unsafe { libc::getpgrp() };
    let original = if mode == "background" {
        own
    } else {
        child_group
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<(), Box<dyn std::error::Error>> {
            assert_eq!(unsafe { libc::tcsetpgrp(fd, original) }, 0);
            std::fs::write(home.join("go"), b"go")?;
            let ready = String::from_utf8(wait_file(&home.join("payload-ready"))?)?;
            let groups: Vec<i32> = ready
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            assert_ne!(groups[0], child_group, "worker needs its own group");
            if mode == "background" {
                assert_eq!(groups[1], own, "background job stole terminal");
            } else {
                assert_eq!(groups[1], groups[0], "foreground was not transferred");
                if mode == "ctrl-c" {
                    let byte = [3u8];
                    assert_eq!(
                        unsafe { libc::write(master.as_raw_fd(), byte.as_ptr().cast(), 1) },
                        1
                    );
                } else if mode == "signal" {
                    assert_eq!(unsafe { libc::kill(child_group, libc::SIGTERM) }, 0);
                } else {
                    let bytes = b"x\n";
                    assert_eq!(
                        unsafe {
                            libc::write(master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len())
                        },
                        bytes.len() as isize
                    );
                    assert_eq!(wait_file(&home.join("payload-read"))?, b"x");
                }
            }
            let start = std::time::Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if start.elapsed() > Duration::from_secs(8) {
                    return Err("PTY supervisor did not exit".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            if mode == "ctrl-c" {
                assert!(start.elapsed() >= Duration::from_secs(5));
            }
            assert_eq!(
                status.code(),
                Some(if mode == "signal" {
                    143
                } else if mode == "ctrl-c" {
                    130
                } else {
                    0
                })
            );
            assert_eq!(
                unsafe { libc::tcgetpgrp(fd) },
                original,
                "foreground was not restored"
            );
            Ok(())
        },
    ));
    // Kill/reap the supervisor and its authenticated worker group on every path.
    let group = std::fs::read_to_string(home.join("worker-group"))
        .ok()
        .and_then(|v| v.parse::<i32>().ok());
    if let Some(group) = group {
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if let Some(group) = group {
        let started = std::time::Instant::now();
        loop {
            while unsafe { libc::waitpid(-group, std::ptr::null_mut(), libc::WNOHANG) } > 0 {}
            if unsafe { libc::kill(-group, 0) } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("fixture group stop unconfirmed; retain fixture".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    unsafe {
        libc::tcsetpgrp(fd, own);
    }
    std::fs::write(home.join("cleanup-confirmed"), b"confirmed")?;
    match outcome {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    }
    Ok(0)
}

// Accelerate only admission heartbeat time, retaining a real scheduling window.
struct CliClock(Cell<Duration>);
impl Clock for CliClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
    fn sleep(&self, _: Duration) {
        self.0.set(self.0.get() + Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct CliFilesystem {
    denied_home: Option<PathBuf>,
}
impl state::FilesystemHooks for CliFilesystem {
    fn before_prepare(&self, path: &std::path::Path) -> io::Result<()> {
        if self.denied_home.as_deref() == Some(path) {
            Err(io::Error::from_raw_os_error(libc::EROFS))
        } else {
            Ok(())
        }
    }
}
