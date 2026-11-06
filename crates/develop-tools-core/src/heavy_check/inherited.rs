//! Reentry is a capability carried by an open, locked slot description, not
//! a boolean environment bypass. This is cooperative admission, not a boundary
//! against malicious processes running as the same account.
use super::state::{NativeFilesystem, State, StateError};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub const SLOT_FD_ENV: &str = "DEVELOP_HEAVY_CHECK_SLOT_FD";
pub const CONTROL_FD_ENV: &str = "DEVELOP_HEAVY_CHECK_CONTROL_FD";

/// Closing this duplicate never explicitly unlocks the shared description.
#[derive(Debug)]
pub struct InheritedSlot(File);
impl InheritedSlot {
    /// Validate against retained, safe state; the probe opens its own description.
    /// A missing environment identity means normal admission; malformed identity
    /// is unavailable and must never fall back to unlocked execution.
    pub fn from_environment(state: &State) -> Result<Option<Self>, StateError> {
        let Some(value) = std::env::var_os(SLOT_FD_ENV) else {
            return Ok(None);
        };
        let fd = value
            .to_str()
            .and_then(|value| value.parse::<RawFd>().ok())
            .filter(|fd| *fd > 2)
            .ok_or_else(|| invalid(state))?;
        Self::validate(state, fd).map(Some)
    }

    /// Duplicate before inspecting: callers retain ownership of the supplied FD.
    /// A temporary independent canonical description must be busy *before* testing the
    /// candidate. Otherwise a new open of an unlocked inode could forge reentry.
    pub fn validate(state: &State, fd: RawFd) -> Result<Self, StateError> {
        state.validate(&NativeFilesystem)?;
        if fd <= 2 {
            return Err(invalid(state));
        }
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate < 0 {
            return Err(invalid(state));
        }
        let file = unsafe { File::from_raw_fd(duplicate) };
        let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if descriptor_flags < 0
            || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } < 0
        {
            return Err(invalid(state));
        }
        let expected = state.slot().metadata().map_err(|_| invalid(state))?;
        let actual = file.metadata().map_err(|_| invalid(state))?;
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if !actual.is_file()
            || actual.dev() != expected.dev()
            || actual.ino() != expected.ino()
            || actual.uid() != expected.uid()
            || actual.mode() != expected.mode()
            || actual.nlink() != 1
            || flags < 0
            || flags & libc::O_ACCMODE != libc::O_RDWR
        {
            return Err(invalid(state));
        }
        // A fresh description ensures both that reentry is already locked and
        // that rejecting an idle slot cannot leave State holding a new lock.
        // Resolve the sibling relative to a retained directory; compare identity
        // and revalidate ancestors before trusting that open.
        let probe = unsafe {
            libc::openat(
                state.queue().as_raw_fd(),
                c"../slot.lock".as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if probe < 0 {
            return Err(invalid(state));
        }
        let probe = unsafe { File::from_raw_fd(probe) };
        let identity = probe.metadata().map_err(|_| invalid(state))?;
        if identity.dev() != expected.dev() || identity.ino() != expected.ino() {
            return Err(invalid(state));
        }
        state.validate(&NativeFilesystem)?;
        match flock(probe.as_raw_fd()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Ok(()) => return Err(invalid(state)), // Only this independent FD acquired it.
            Err(_) => return Err(invalid(state)),
        }
        flock(file.as_raw_fd()).map_err(|_| invalid(state))?;
        state.validate(&NativeFilesystem)?;
        Ok(Self(file))
    }

    /// Propagate just this capability. No process_group/setpgid/setsid here:
    /// the outer supervisor owns the family group throughout nested reentry.
    /// Keep this handle alive until spawn has completed.
    pub fn configure_payload(&self, command: &mut Command) {
        let fd = self.0.as_raw_fd();
        command
            .env(SLOT_FD_ENV, fd.to_string())
            .env_remove(CONTROL_FD_ENV);
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    pub fn file(&self) -> &File {
        &self.0
    }
}
fn flock(fd: RawFd) -> io::Result<()> {
    loop {
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
fn invalid(state: &State) -> StateError {
    StateError::Io {
        path: state.path().join("slot.lock"),
        error: io::Error::other("invalid inherited heavy-check capability"),
    }
}
