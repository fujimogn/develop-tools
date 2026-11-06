//! Foreground ownership belongs only to the dedicated outer supervisor.
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

/// Transfer only when our group already owns the controlling terminal.
/// Pipe input and the payload's standard streams are never replaced.
pub struct Foreground {
    terminal: Option<File>,
    original: i32,
}
impl Foreground {
    /// Call only in the single-threaded outer supervisor, after creating the
    /// worker group and before releasing its start handshake. Inner workers
    /// must inherit standard streams and never call this operation.
    pub fn transfer(worker_group: i32) -> io::Result<Self> {
        let terminal = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY)
            .open("/dev/tty")
        {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ENXIO | libc::ENODEV | libc::ENOTTY)
                ) =>
            {
                return Ok(Self {
                    terminal: None,
                    original: 0,
                });
            }
            Err(error) => return Err(error),
        };
        let original = unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) };
        if original < 0 {
            return Err(io::Error::last_os_error());
        }
        if original != unsafe { libc::getpgrp() } {
            return Ok(Self {
                terminal: None,
                original,
            });
        }
        set_group(&terminal, worker_group)?;
        Ok(Self {
            terminal: Some(terminal),
            original,
        })
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(terminal) = &self.terminal {
            set_group(terminal, self.original)?;
        }
        self.terminal.take();
        Ok(())
    }
}
impl Drop for Foreground {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn set_group(terminal: &File, group: i32) -> io::Result<()> {
    // Restoring runs while we are in the background. Block SIGTTOU for just
    // this thread and restore its exact prior mask, including on syscall error.
    let mut blocked = unsafe { std::mem::zeroed() };
    let mut previous = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
    }
    let error = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    let result = loop {
        if unsafe { libc::tcsetpgrp(terminal.as_raw_fd(), group) } == 0 {
            break Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            break Err(error);
        }
    };
    let error =
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
    result?;
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    Ok(())
}
