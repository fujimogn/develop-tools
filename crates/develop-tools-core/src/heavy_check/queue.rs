//! Guard-serialized FIFO admission. Liveness is an open-file-description lock,
//! never a PID, process name, or elapsed-time heuristic.
use super::Clock;
use super::state::{NativeFilesystem, State, StateError};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::Duration;

const SEQUENCE: &str = "next";
const SEQUENCE_TMP: &str = "next.tmp";
const TICKET_DIGITS: usize = 20;
const TICKET_SUFFIX: &str = ".ticket";

pub const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Owns both the ticket lock and validated state until admission/cancellation.
pub struct Ticket {
    state: State,
    file: File,
    name: String,
    number: u64,
    pending: bool,
}
/// Dropping this handle closes the execution lock. No explicit unlock is sent:
/// explicitly inherited duplicates must continue to hold the same description.
pub struct Slot(File);
impl Slot {
    pub fn file(&self) -> &File {
        &self.0
    }
    /// Only the execution slot may cross exec. Other state and ticket FDs stay CLOEXEC.
    pub fn inheritable_file(&self) -> io::Result<File> {
        let file = self.0.try_clone()?;
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) }
                < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }
}
impl Ticket {
    pub fn register(state: State) -> Result<Self, StateError> {
        let _guard = Guard::lock(state.guard()).map_err(|e| error(&state, e))?;
        state.validate(&NativeFilesystem)?;
        let previous_file = match open(&state, SEQUENCE, false) {
            Ok((file, _)) => Some(file),
            Err(StateError::Io { error, .. }) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let previous = if let Some(mut file) = previous_file.as_ref() {
            let mut text = String::new();
            (&mut file)
                .take(22)
                .read_to_string(&mut text)
                .map_err(|e| error(&state, e))?;
            let value = text
                .trim()
                .parse::<u64>()
                .map_err(|_| error(&state, io::Error::other("invalid queue sequence")))?;
            if text != format!("{value}\n") {
                return Err(error(&state, io::Error::other("invalid queue sequence")));
            }
            value
        } else {
            0
        };
        let number = previous
            .checked_add(1)
            .ok_or_else(|| error(&state, io::Error::other("queue sequence exhausted")))?;
        // A killed registrar may leave next.tmp; only an atomic rename publishes
        // the fully written sequence. A gap is fine; reusing a number is not.
        let (mut next, _) = open(&state, SEQUENCE_TMP, true)?;
        next.set_len(0).map_err(|e| error(&state, e))?;
        next.write_all(format!("{number}\n").as_bytes())
            .map_err(|e| error(&state, e))?;
        same_entry(&state, SEQUENCE_TMP, &next)?;
        if let Some(file) = previous_file {
            same_entry(&state, SEQUENCE, &file)?;
        }
        let temporary_name = name_cstring(&state, SEQUENCE_TMP)?;
        let sequence_name = name_cstring(&state, SEQUENCE)?;
        if unsafe {
            libc::renameat(
                state.queue().as_raw_fd(),
                temporary_name.as_ptr(),
                state.queue().as_raw_fd(),
                sequence_name.as_ptr(),
            )
        } != 0
        {
            return Err(error(&state, io::Error::last_os_error()));
        }
        let name = format!("{number:0width$}{TICKET_SUFFIX}", width = TICKET_DIGITS);
        let (file, created) = open(&state, &name, true)?;
        if !created {
            return Err(error(&state, io::Error::other("ticket already exists")));
        }
        lock(&file, libc::LOCK_EX).map_err(|e| error(&state, e))?;
        drop(_guard);
        Ok(Self {
            state,
            file,
            name,
            number,
            pending: true,
        })
    }
    /// Read only the earliest registered number under the queue guard.
    pub fn head_number(&self) -> Result<Option<u64>, StateError> {
        let _guard = Guard::lock(self.state.guard()).map_err(|e| error(&self.state, e))?;
        self.state.validate(&NativeFilesystem)?;
        tickets(&self.state)?
            .first()
            .map(|name| {
                name[..TICKET_DIGITS]
                    .parse()
                    .map_err(|e| error(&self.state, io::Error::other(e)))
            })
            .transpose()
    }
    pub fn number(&self) -> u64 {
        self.number
    }
    /// Ticket lock is observable for descriptor lifetime checks, not for unlocking.
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn try_acquire(&mut self) -> Result<Option<Slot>, StateError> {
        let _guard = Guard::lock(self.state.guard()).map_err(|e| error(&self.state, e))?;
        self.state.validate(&NativeFilesystem)?;
        same_entry(&self.state, &self.name, &self.file)?;
        for name in tickets(&self.state)? {
            if name != self.name {
                let (file, _) = open(&self.state, &name, false)?;
                if try_lock(&file).map_err(|e| error(&self.state, e))? {
                    remove(&self.state, &name, &file)?;
                    continue;
                }
                return Ok(None);
            }
            if !try_lock(self.state.slot()).map_err(|e| error(&self.state, e))? {
                return Ok(None);
            }
            let slot = self
                .state
                .slot()
                .try_clone()
                .map_err(|e| error(&self.state, e))?;
            remove(&self.state, &self.name, &self.file)?;
            self.pending = false;
            return Ok(Some(Slot(slot)));
        }
        Err(error(&self.state, io::Error::other("own ticket missing")))
    }
    /// Waiting is unbounded. The outer caller owns cancellation/deadline policy.
    pub fn wait(mut self, clock: &dyn Clock) -> Result<Slot, StateError> {
        loop {
            if let Some(slot) = self.try_acquire()? {
                return Ok(slot);
            }
            clock.sleep(RETRY_INTERVAL);
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.pending {
            if let Ok(_guard) = Guard::lock(self.state.guard()) {
                // A replaced path must never cause removal of another waiter's ticket.
                if self.state.validate(&NativeFilesystem).is_ok() {
                    let _ = remove(&self.state, &self.name, &self.file);
                }
            }
        }
    }
}
struct Guard<'a>(&'a File);
impl<'a> Guard<'a> {
    fn lock(file: &'a File) -> io::Result<Self> {
        lock(file, libc::LOCK_EX)?;
        Ok(Self(file))
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = lock(self.0, libc::LOCK_UN);
    }
}
fn lock(file: &File, flags: i32) -> io::Result<()> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
fn try_lock(file: &File) -> io::Result<bool> {
    match lock(file, libc::LOCK_EX | libc::LOCK_NB) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    }
}
fn error(state: &State, error: io::Error) -> StateError {
    StateError::Io {
        path: state.path().join("queue"),
        error,
    }
}
fn name_cstring(state: &State, name: &str) -> Result<CString, StateError> {
    CString::new(name).map_err(|e| error(state, io::Error::other(e)))
}
// All mutations are relative to I1's held directory, never a re-resolved path.
fn open(state: &State, name: &str, create: bool) -> Result<(File, bool), StateError> {
    let name = name_cstring(state, name)?;
    let flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    let mut created = false;
    let mut fd = -1;
    if create {
        fd = unsafe {
            libc::openat(
                state.queue().as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        created = fd >= 0;
        if fd < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
            return Err(error(state, io::Error::last_os_error()));
        }
    }
    if fd < 0 {
        fd = unsafe { libc::openat(state.queue().as_raw_fd(), name.as_ptr(), flags) };
    }
    if fd < 0 {
        return Err(error(state, io::Error::last_os_error()));
    }
    // SAFETY: openat yielded an owned descriptor, transferred exactly once.
    let file = unsafe { File::from_raw_fd(fd) };
    state.validate_queue_file(&name, &file)?;
    Ok((file, created))
}
fn same_entry(state: &State, name: &str, file: &File) -> Result<(), StateError> {
    let name = name_cstring(state, name)?;
    state.validate_queue_file(&name, file)
}
fn remove(state: &State, name: &str, file: &File) -> Result<(), StateError> {
    same_entry(state, name, file)?;
    let name = name_cstring(state, name)?;
    if unsafe { libc::unlinkat(state.queue().as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(error(state, io::Error::last_os_error()));
    }
    Ok(())
}
fn tickets(state: &State) -> Result<Vec<String>, StateError> {
    // A fresh description avoids sharing directory offsets with I1's retained FD.
    let fd = unsafe {
        libc::openat(
            state.queue().as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(error(state, io::Error::last_os_error()));
    }
    let dir = unsafe { libc::fdopendir(fd) };
    if dir.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(error(state, io::Error::last_os_error()));
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let dir = Directory(dir);
    let mut names = Vec::new();
    loop {
        // readdir uses NULL for both EOF and error; never treat an I/O error
        // as an empty queue (which could admit out of order).
        unsafe {
            *errno_pointer() = 0;
        }
        let entry = unsafe { libc::readdir(dir.0) };
        if entry.is_null() {
            let errno = unsafe { *errno_pointer() };
            if errno != 0 {
                return Err(error(state, io::Error::from_raw_os_error(errno)));
            }
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name.ends_with(TICKET_SUFFIX.as_bytes()) {
            if name.len() != TICKET_DIGITS + TICKET_SUFFIX.len()
                || !name[..TICKET_DIGITS].iter().all(u8::is_ascii_digit)
            {
                return Err(error(state, io::Error::other("invalid ticket name")));
            }
            names.push(String::from_utf8(name.to_vec()).expect("ASCII ticket"));
        }
    }
    names.sort_unstable();
    Ok(names)
}

#[cfg(target_os = "linux")]
fn errno_pointer() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}
#[cfg(target_os = "macos")]
fn errno_pointer() -> *mut libc::c_int {
    unsafe { libc::__error() }
}
