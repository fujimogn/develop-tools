#[path = "support/heavy_check.rs"]
#[allow(dead_code)]
mod support;

use develop_tools_core::heavy_check::supervisor;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::process::Stdio;

#[test]
fn argv_and_binary_stdin_round_trip() {
    let home = support::fixture();
    let args = [
        OsString::from(""),
        OsString::from("two words"),
        OsString::from("'quoted' \"double\""),
        OsString::from("$HOME; echo * | & > < $(x)"),
        OsString::from_vec(vec![b'x', 0xff]),
    ];
    let input = b"refs/heads/develop abc refs/heads/develop def\n\0\xff\r\n";
    let mut command = support::harness_command();
    command
        .arg("--io-supervisor")
        .arg(home.path())
        .arg("--io-echo")
        .process_group(0)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = supervisor::spawn(&mut command).unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut expected = Vec::new();
    for arg in args {
        let bytes = arg.into_vec();
        expected.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        expected.extend(bytes);
    }
    expected.extend_from_slice(&u32::MAX.to_be_bytes());
    expected.extend_from_slice(input);
    assert_eq!(output.stdout, expected);
    assert_eq!(output.stderr, b"payload-stderr\n");
}

fn pty(mode: &str) {
    let home = support::fixture();
    let mut command = support::harness_command();
    command
        .arg("--pty-driver")
        .arg(home.path())
        .arg(mode)
        .stdin(Stdio::null());
    let status = supervisor::spawn(&mut command).unwrap().wait().unwrap();
    if !home.path().join("cleanup-confirmed").is_file() {
        let retained = home.keep();
        panic!("PTY cleanup unconfirmed; retained {}", retained.display());
    }
    assert!(status.success(), "PTY {mode}: {status}");
}
#[test]
fn foreground_pty_is_restored_after_exit_and_signal() {
    pty("normal");
    pty("signal");
}
#[test]
fn background_job_does_not_steal_terminal() {
    pty("background");
}

#[test]
fn ordinary_child_does_not_inherit_control_identity() {
    let home = support::fixture();
    // Exercise a real worker environment, rather than configure_payload's removal.
    let mut worker = support::harness_command();
    worker
        .arg("--io-supervisor")
        .arg(home.path())
        .arg("--io-control-probe")
        .process_group(0);
    assert!(
        supervisor::spawn(&mut worker)
            .unwrap()
            .wait()
            .unwrap()
            .success()
    );
}
