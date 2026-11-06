//! Best-effort JSON lines on a dedicated, caller-owned nonblocking channel.
//! No command, environment value, or connection detail enters this protocol.
use serde_json::Value;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub const EVENT_FD_ENV: &str = "DEVELOP_HEAVY_CHECK_EVENT_FD";

pub struct Events(Option<File>);
impl Events {
    pub fn from_environment() -> Self {
        let file = std::env::var_os(EVENT_FD_ENV)
            .and_then(|v| v.to_str()?.parse::<i32>().ok())
            .filter(|fd| *fd > 2)
            .and_then(|fd| {
                let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
                if duplicate < 0 {
                    return None;
                }
                let file = unsafe { File::from_raw_fd(duplicate) };
                let flags = unsafe { libc::fcntl(duplicate, libc::F_GETFL) };
                if flags < 0
                    || unsafe { libc::fcntl(duplicate, libc::F_SETFL, flags | libc::O_NONBLOCK) }
                        < 0
                {
                    return None;
                }
                // The caller dedicates this open description to telemetry. Keep
                // the supplied descriptor private to this wrapper on later exec.
                unsafe {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
                Some(file)
            });
        Self(file)
    }
    pub fn emit(&self, value: Value) {
        if let Some(file) = &self.0 {
            let mut bytes = value.to_string().into_bytes();
            bytes.push(b'\n');
            // One bounded write: full pipe, closed peer, and malformed FD are
            // diagnostic loss only. No retry can stall a check or its deadline.
            unsafe {
                libc::write(file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len());
            }
        }
    }
    pub(crate) fn configure_worker(&self, command: &mut Command) {
        command.env_remove(EVENT_FD_ENV);
        if let Some(file) = &self.0 {
            let fd = file.as_raw_fd();
            command.env(EVENT_FD_ENV, fd.to_string());
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }
}
