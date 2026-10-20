#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

const CHILD_MARKER: &str = "STRIP_GIT_ENV_TEST_CHILD";

#[test]
fn non_utf8_values_do_not_interrupt_git_environment_stripping() {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "check_child_environment"])
        .env(CHILD_MARKER, "1")
        .env("GIT_NON_UTF8_TEST", OsString::from_vec(vec![0xff]))
        .env(OsString::from_vec(b"GIT_\xff".to_vec()), "value")
        .env("KEEP_NON_UTF8_TEST", OsString::from_vec(vec![0xfe]))
        .output()
        .expect("run child test");

    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn check_child_environment() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        return;
    }

    let mut command = Command::new("env");
    develop_tools_core::strip_git_env(&mut command);
    let output = command.output().expect("run env");
    assert!(output.status.success());

    let entries: Vec<&[u8]> = output.stdout.split(|byte| *byte == b'\n').collect();
    assert!(entries.iter().all(|entry| !entry.starts_with(b"GIT_")));
    assert!(
        entries
            .iter()
            .any(|entry| *entry == b"KEEP_NON_UTF8_TEST=\xfe")
    );
}
