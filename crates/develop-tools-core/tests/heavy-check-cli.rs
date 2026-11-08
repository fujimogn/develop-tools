#[path = "support/heavy_check.rs"]
#[allow(dead_code)]
mod support;

use develop_tools_core::heavy_check::supervisor;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn cli(ci: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_run-heavy-check"));
    command
        .process_group(0)
        .env("GITHUB_ACTIONS", if ci { "true" } else { "false" });
    for key in [
        "DEVELOP_HEAVY_CHECK_SLOT_FD",
        "DEVELOP_HEAVY_CHECK_CONTROL_FD",
        "DEVELOP_HEAVY_CHECK_EVENT_FD",
    ] {
        command.env_remove(key);
    }
    command.stdin(Stdio::null());
    command
}

#[test]
fn local_75_maps_to_one_but_ci_preserves_75() {
    let home = support::fixture();
    for (ci, expected) in [(false, 1), (true, 75)] {
        // Local admission must never acquire the real account's outer verify slot.
        // The fixture runs the same core; CI still exercises the released CLI entry point.
        let mut command = if ci {
            cli(true)
        } else {
            fixture_cli(home.path(), false)
        };
        let output = supervisor::spawn(command.args(["--", "/bin/sh", "-c", "exit 75"]))
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected));
    }
}

use develop_tools_core::heavy_check::state::{self, NativeFilesystem, StateLayout};
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn fixture_cli(home: &std::path::Path, step: bool) -> Command {
    let mut c = support::harness_command();
    c.arg("--cli-fixture")
        .arg(home)
        .arg(if step { "step" } else { "real" })
        .env("GITHUB_ACTIONS", "false")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}
fn events(command: &mut Command) -> UnixStream {
    let (read, write) = UnixStream::pair().unwrap();
    let fd = write.as_raw_fd();
    // Move the owned writer into the pre-exec closure so it stays live through spawn.
    command.env("DEVELOP_HEAVY_CHECK_EVENT_FD", fd.to_string());
    unsafe {
        command.pre_exec(move || {
            assert_eq!(write.as_raw_fd(), fd);
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    read
}
fn read_events(mut read: UnixStream) -> Vec<serde_json::Value> {
    read.set_nonblocking(true).unwrap();
    let mut text = String::new();
    let _ = read.read_to_string(&mut text);
    text.lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

#[test]
fn heartbeat_is_stderr_only_and_stops_on_acquire() {
    let home = support::fixture();
    let source = support::Source {
        account: home.path().into(),
        home: None,
    };
    let state = state::prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
    assert_eq!(
        unsafe { libc::flock(state.slot().as_raw_fd(), libc::LOCK_EX) },
        0
    );
    let mut c = fixture_cli(home.path(), true);
    c.args(["--", "/bin/sh", "-c", "printf body; sleep 0.1"]);
    let event_read = events(&mut c);
    let mut child = supervisor::spawn(&mut c).unwrap();
    drop(c);
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if line.contains("still waiting") {
                let _ = tx.send(line.clone());
            }
            lines.push(line);
        }
        lines
    });
    let first = rx.recv_timeout(Duration::from_secs(5));
    let second = rx.recv_timeout(Duration::from_secs(5));
    // Release before any assertions so even a broken heartbeat cannot strand a waiter.
    unsafe {
        libc::flock(state.slot().as_raw_fd(), libc::LOCK_UN);
    }
    let output = child.wait_with_output().unwrap();
    let lines = reader.join().unwrap();
    assert!(output.status.success(), "{lines:?}");
    assert!(first.unwrap().contains("after 30s; head ticket=1"));
    assert!(second.unwrap().contains("after 60s; head ticket=1"));
    assert_eq!(output.stdout, b"body");
    let acquired = lines
        .iter()
        .position(|s| s.starts_with("acquired after"))
        .unwrap();
    assert!(
        lines[acquired + 1..]
            .iter()
            .all(|s| !s.contains("still waiting"))
    );
    let ev = read_events(event_read);
    assert_eq!(
        ev.iter()
            .map(|v| v["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["queued", "acquired", "completed"]
    );
}

#[test]
fn broken_event_channel_preserves_result() {
    for code in [0, 3, 75] {
        let home = support::fixture();
        let mut c = fixture_cli(home.path(), false);
        c.args(["/bin/sh", "-c", &format!("exit {code}")]);
        let peer = events(&mut c);
        drop(peer);
        let output = supervisor::spawn(&mut c)
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if code == 75 { 1 } else { code })
        );
    }
}

#[test]
fn headless_context_isolated_even_when_account_root_writable() {
    let root = support::fixture();
    let sandbox = support::directory(&root.path().join("sandbox"));
    let mut c = fixture_cli(root.path(), false);
    c.env("HOME", &sandbox).args([
        "--executor-context=headless-sandbox",
        "--",
        "/bin/sh",
        "-c",
        "printf body",
    ]);
    let read = events(&mut c);
    let output = supervisor::spawn(&mut c)
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let ev = read_events(read);
    assert_eq!(ev.last().unwrap()["context"], "headless-sandbox");
    assert_eq!(ev.last().unwrap()["isolated"], true);
    assert!(!root.path().join(".cache").exists());
    assert!(
        sandbox
            .join(".cache/develop-tools/heavy-check/slot.lock")
            .is_file()
    );
}

#[test]
fn raw_status_and_private_diagnostics() {
    for (script, result, raw, signal) in [
        ("exit 75", 1, Some(75), None),
        ("exit 130", 130, Some(130), None),
        ("kill -TERM $$", 143, None, Some(15)),
    ] {
        let home = support::fixture();
        let mut c = fixture_cli(home.path(), false);
        c.env("PRIVATE_CONNECTION", "secret-connection-value")
            .args(["/bin/sh", "-c", script, "secret-argv-value"]);
        let read = events(&mut c);
        let output = supervisor::spawn(&mut c)
            .unwrap()
            .wait_with_output()
            .unwrap();
        let ev = read_events(read);
        let end = ev.last().unwrap();
        assert_eq!(output.status.code(), Some(result));
        assert_eq!(end["rawExit"].as_i64(), raw);
        assert_eq!(end["rawSignal"].as_i64(), signal);
        assert_eq!(end["executed"], true);
        assert_eq!(end["result"], result);
        let logs = String::from_utf8(output.stderr).unwrap() + &format!("{ev:?}");
        assert!(!logs.contains("secret-"));
    }
}

#[test]
fn ci_never_prepares_state_and_unsafe_root_never_falls_back() {
    use std::os::unix::fs::symlink;
    let home = support::fixture();
    let sandbox = support::directory(&home.path().join("sandbox"));
    symlink(&sandbox, home.path().join(".cache")).unwrap();
    let mut local = fixture_cli(home.path(), false);
    local
        .env("HOME", &sandbox)
        .args(["/bin/sh", "-c", "printf SHOULD_NOT_RUN"]);
    let read = events(&mut local);
    let output = supervisor::spawn(&mut local)
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert_eq!(output.status.code(), Some(75));
    assert!(output.stdout.is_empty());
    let ev = read_events(read);
    assert_eq!(ev.last().unwrap()["event"], "unavailable");
    assert_eq!(ev.last().unwrap()["executed"], false);
    assert!(!sandbox.join(".cache").exists());
    let mut ci = fixture_cli(home.path(), false);
    ci.env("GITHUB_ACTIONS", "true")
        .args(["/bin/sh", "-c", "printf ci; exit 75"]);
    let read = events(&mut ci);
    let output = supervisor::spawn(&mut ci)
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert_eq!(output.status.code(), Some(75));
    assert_eq!(output.stdout, b"ci");
    let ev = read_events(read);
    assert!(ev.iter().all(|e| e["event"] != "queued"));
    assert_eq!(ev.last().unwrap()["context"], "ci");
}

#[test]
fn actual_wrapper_retains_slot_when_helper_closes_fd() {
    let home = support::fixture();
    let executable = support::harness_command().get_program().to_owned();
    let mut first = cli(false);
    first
        .env("HOME", home.path())
        .arg("--executor-context=headless-sandbox")
        .arg(&executable)
        .arg("--close-slot-helper")
        .arg(home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut first = supervisor::spawn(&mut first).unwrap();
    let limit = std::time::Instant::now();
    while !home.path().join("closed-fd-descendant").exists()
        && limit.elapsed() < Duration::from_secs(3)
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let ready = home.path().join("closed-fd-descendant").exists();
    let mut second = cli(false);
    second
        .env("HOME", home.path())
        .args([
            "--executor-context=headless-sandbox",
            "/bin/sh",
            "-c",
            "printf second",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut second = supervisor::spawn(&mut second).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let first_live = first.try_wait().unwrap().is_none();
    let second_live = second.try_wait().unwrap().is_none();
    std::fs::write(home.path().join("release-closed-fd"), b"release").unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(ready, "{first:?}");
    assert!(first_live, "wrapper released before descendant stopped");
    assert!(second_live, "next wrapper acquired while descendant lived");
    assert!(first.status.success(), "{first:?}");
    assert!(second.status.success(), "{second:?}");
    assert_eq!(second.stdout, b"second");
}

#[test]
fn public_argv_stdin_and_help_are_transparent() {
    use std::io::Write;
    let home = support::fixture();
    for arg in ["--help", "--version"] {
        let output = supervisor::spawn(
            cli(false)
                .env("HOME", home.path())
                .arg(arg)
                .stdout(Stdio::piped()),
        )
        .unwrap()
        .wait_with_output()
        .unwrap();
        assert!(output.status.success());
        assert!(!home.path().join(".cache").exists());
    }
    let executable = support::harness_command().get_program().to_owned();
    let mut c = cli(false);
    c.env("HOME", home.path())
        .args(["--executor-context=headless-sandbox", "--"])
        .arg(executable)
        .arg("--io-echo")
        .arg(home.path())
        .args(["", "--", "two words", "$HOME; secret"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = supervisor::spawn(&mut c).unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"refs heads\n\0\xff")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut expected = Vec::new();
    for arg in ["", "--", "two words", "$HOME; secret"] {
        expected.extend_from_slice(&(arg.len() as u32).to_be_bytes());
        expected.extend_from_slice(arg.as_bytes());
    }
    expected.extend_from_slice(&u32::MAX.to_be_bytes());
    expected.extend_from_slice(b"refs heads\n\0\xff");
    assert_eq!(output.stdout, expected);
}

#[test]
fn authenticated_external_supervisor_needs_no_private_cli_protocol() {
    let home = support::fixture();
    let mut c = support::harness_command();
    c.arg("--cli-supervised")
        .arg(home.path())
        .arg(env!("CARGO_BIN_EXE_run-heavy-check"))
        .env("HOME", home.path())
        .env("GITHUB_ACTIONS", "false")
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = supervisor::spawn(&mut c)
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"external");
}

#[test]
fn home_alone_does_not_bypass_and_write_denial_is_distinct() {
    for (mode, context, isolated) in [
        ("real", "host", false),
        ("denied", "write-denial-fallback", true),
    ] {
        let home = support::fixture();
        let sandbox = support::directory(&home.path().join("sandbox"));
        let mut c = support::harness_command();
        c.args(["--cli-fixture"])
            .arg(home.path())
            .arg(mode)
            .env("HOME", &sandbox)
            .env("GITHUB_ACTIONS", "false")
            .process_group(0)
            .args(["/bin/sh", "-c", "exit 0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let read = events(&mut c);
        let output = supervisor::spawn(&mut c)
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let ev = read_events(read);
        assert_eq!(ev.last().unwrap()["context"], context);
        if mode == "denied" {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("account state write denied (errno 30); using isolated HOME")
            );
        }
        assert_eq!(ev.last().unwrap()["isolated"], isolated);
        assert_eq!(sandbox.join(".cache").exists(), isolated);
    }
}

#[test]
fn terminal_ctrl_c_escalates_ignoring_payload_and_restores_foreground() {
    let home = support::fixture();
    let mut command = support::harness_command();
    command
        .arg("--pty-driver")
        .arg(home.path())
        .arg("ctrl-c")
        .env("HEAVY_CHECK_CLI", env!("CARGO_BIN_EXE_run-heavy-check"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = supervisor::spawn(&mut command)
        .unwrap()
        .wait_with_output()
        .unwrap();
    if !home.path().join("cleanup-confirmed").is_file() {
        panic!("cleanup unconfirmed; retained {}", home.keep().display());
    }
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn cancellation_before_admission_keeps_raw_signal_and_unexecuted_result() {
    let home = support::fixture();
    let source = support::Source {
        account: home.path().into(),
        home: None,
    };
    let state = state::prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
    assert_eq!(
        unsafe { libc::flock(state.slot().as_raw_fd(), libc::LOCK_EX) },
        0
    );
    let mut command = fixture_cli(home.path(), false);
    command.args(["/bin/sh", "-c", "printf must-not-run"]);
    let read = events(&mut command);
    let mut child = supervisor::spawn(&mut command).unwrap();
    let mut line = String::new();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    stderr.read_line(&mut line).unwrap();
    assert!(line.starts_with("queued ticket="), "{line}");
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(143));
    assert!(output.stdout.is_empty());
    let ev = read_events(read);
    let end = ev.last().unwrap();
    assert_eq!(end["event"], "unavailable");
    assert_eq!(end["executed"], false);
    assert_eq!(end["rawSignal"], 15);
    assert_eq!(end["result"], 75);
}

#[test]
fn unknown_executor_context_is_usage_error_without_admission() {
    let home = support::fixture();
    let mut command = fixture_cli(home.path(), false);
    command.args(["--executor-context=unknown", "/bin/true"]);
    let output = supervisor::spawn(&mut command)
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!home.path().join(".cache").exists());
}
