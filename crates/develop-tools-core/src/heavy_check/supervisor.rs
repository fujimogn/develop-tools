//! Family supervision runs in a dedicated process outside the worker group.
//! Workers never change groups. A slot is inherited by the payload family, so
//! killing this supervisor alone cannot release a live descendant's lock.
use super::Clock;
use super::queue::RETRY_INTERVAL;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

const SPAWN_ATTEMPTS: usize = 20;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);
/// Public core cannot depend on private tools/develop-core/src/process.rs; mirror its ETXTBSY-only pre-exec retry.
pub fn spawn(command: &mut Command) -> io::Result<Child> {
    // Only configure_worker may explicitly pass a control capability. Ordinary
    // worker children must not inherit a stale environment identity after exec.
    let control = std::ffi::OsStr::new(super::inherited::CONTROL_FD_ENV);
    if !command
        .get_envs()
        .any(|(key, value)| key == control && value.is_some())
    {
        command.env_remove(control);
    }
    for attempt in 0..SPAWN_ATTEMPTS {
        match command.spawn() {
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY) && attempt + 1 < SPAWN_ATTEMPTS =>
            {
                std::thread::sleep(SPAWN_RETRY_DELAY)
            }
            result => return result,
        }
    }
    unreachable!("bounded spawn loop always returns")
}

/// A private per-family socket pair transfers a duplicate of the acquired slot
/// to the group-external supervisor. Kernel-queued SCM_RIGHTS also holds the lock
/// before the supervisor receives it. A separate versioned start/ACK exchange
/// authenticates the external parent and its worker group before admission.
pub struct SlotReceiver(UnixStream);
pub struct SlotSender(UnixStream);
pub fn slot_channel() -> io::Result<(SlotReceiver, SlotSender)> {
    let (receiver, sender) = UnixStream::pair()?;
    receiver.set_nonblocking(true)?;
    Ok((SlotReceiver(receiver), SlotSender(sender)))
}
impl SlotSender {
    /// Configure only the outer supervisor's child: workers never create groups.
    pub fn configure_worker(&self, command: &mut Command) {
        let fd = self.0.as_raw_fd();
        command.process_group(0);
        command.env(super::inherited::CONTROL_FD_ENV, fd.to_string());
        // SAFETY: pre_exec only uses async-signal-safe fcntl and no allocations.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    /// Authenticate the group-external parent before starting admission. The
    /// kernel peer PID is captured when the supervisor creates the socket pair;
    /// a worker-created fake socket cannot impersonate that parent.
    pub fn wait_for_supervisor(&self) -> io::Result<()> {
        let parent = unsafe { libc::getppid() };
        let group = unsafe { libc::getpgrp() };
        if peer_pid(&self.0)? != parent
            || parent <= 1
            || unsafe { libc::getpgid(parent) } == group
            || unsafe { libc::getpid() } != group
        {
            return Err(handshake_error());
        }
        self.0.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        self.0.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let mut frame = [0u8; 16];
        (&self.0).read_exact(&mut frame)?;
        if frame != handshake_frame(parent, group) {
            return Err(handshake_error());
        }
        (&self.0).write_all(&frame)?;
        self.0.set_read_timeout(None)?;
        self.0.set_write_timeout(None)?;
        Ok(())
    }
    pub fn fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
    /// Decode the explicit external-supervisor capability. Absence leaves the
    /// caller on the standalone supervisor path; malformed identities fail.
    pub fn from_environment() -> io::Result<Option<Self>> {
        let Some(value) = std::env::var_os(super::inherited::CONTROL_FD_ENV) else {
            return Ok(None);
        };
        let fd = value
            .to_str()
            .and_then(|value| value.parse::<i32>().ok())
            .ok_or_else(handshake_error)?;
        Self::from_control_fd(fd).map(Some)
    }
    /// Borrow an untrusted FD number, validate its socket type, and own only a
    /// CLOEXEC duplicate. The incoming descriptor is made CLOEXEC too.
    pub fn from_control_fd(fd: i32) -> io::Result<Self> {
        if fd <= 2 {
            return Err(handshake_error());
        }
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
        let mut kind: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&kind) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                duplicate.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut libc::c_int).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if kind != libc::SOCK_STREAM || length as usize != std::mem::size_of_val(&kind) {
            return Err(handshake_error());
        }
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(unsafe {
            UnixStream::from_raw_fd(duplicate.into_raw_fd())
        }))
    }
    /// SAFETY: caller transfers sole ownership of its inherited socket FD.
    ///
    /// # Safety
    /// `fd` must be an open socket owned solely by this worker.
    pub unsafe fn from_inherited_fd(fd: i32) -> io::Result<Self> {
        let sender = Self(unsafe { UnixStream::from_raw_fd(fd) });
        if unsafe { libc::fcntl(sender.fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sender)
    }
    pub fn send(&self, slot: &std::fs::File) -> io::Result<()> {
        // usize storage supplies cmsghdr alignment on Linux and macOS.
        let mut control = [0usize; 8];
        let mut byte = [1u8];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as _) } as _;
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as _) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<i32>(), slot.as_raw_fd());
            if libc::sendmsg(self.fd(), &message, 0) == 1 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}
impl SlotReceiver {
    fn resume_slot_polling(&self) -> io::Result<()> {
        // recvmsg is nonblocking from here on, so the handshake timeouts are
        // dormant. Clearing them can fail with EINVAL on Darwin if a fast
        // worker closed its peer after sending the ACK.
        self.0.set_nonblocking(true)
    }
    fn receive(&self) -> io::Result<Option<OwnedFd>> {
        let mut control = [0usize; 8];
        let mut byte = [0u8];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control) as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        let received = unsafe { libc::recvmsg(self.0.as_raw_fd(), &mut message, flags) };
        if received == 0 {
            return Ok(None);
        } // Peer died before acquiring; worker reap supplies the status.
        if received < 0 {
            let error = io::Error::last_os_error();
            return if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // Only the privately inherited sender can send one FD on this channel.
        let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
        if received != 1 || byte[0] != 1 || header.is_null() {
            return Err(io::Error::other("missing acquired slot descriptor"));
        }
        unsafe {
            if (*header).cmsg_level != libc::SOL_SOCKET
                || (*header).cmsg_type != libc::SCM_RIGHTS
                || (*header).cmsg_len as usize
                    != libc::CMSG_LEN(std::mem::size_of::<i32>() as _) as usize
            {
                return Err(io::Error::other("invalid slot transfer"));
            }
            let fd = OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                libc::CMSG_DATA(header).cast::<i32>(),
            ));
            if libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(fd))
        }
    }
}

pub const SIGNAL_GRACE: Duration = Duration::from_secs(5);
static SIGNAL: AtomicI32 = AtomicI32::new(0);
extern "C" fn record_signal(signal: libc::c_int) {
    // Keep the first cancellation's deadline; repeated signals cannot extend it.
    let _ = SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

/// Install only in a dedicated, single-threaded outer supervisor before spawn.
/// The returned guard restores signal dispositions and Linux adoption policy.
pub struct Supervision {
    previous: Vec<(i32, libc::sigaction)>,
    #[cfg(target_os = "linux")]
    previous_subreaper: i32,
}
impl Supervision {
    pub fn install() -> io::Result<Self> {
        let mut this = Self {
            previous: Vec::new(),
            #[cfg(target_os = "linux")]
            previous_subreaper: 0,
        };
        #[cfg(target_os = "linux")]
        unsafe {
            if libc::prctl(
                libc::PR_GET_CHILD_SUBREAPER,
                &mut this.previous_subreaper,
                0,
                0,
                0,
            ) != 0
                || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        SIGNAL.store(0, Ordering::Relaxed);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = record_signal as *const () as usize;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            let mut previous = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                return Err(io::Error::last_os_error());
            }
            this.previous.push((signal, previous));
        }
        Ok(this)
    }
}
impl Drop for Supervision {
    fn drop(&mut self) {
        for (signal, previous) in &self.previous {
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        #[cfg(target_os = "linux")]
        unsafe {
            libc::prctl(
                libc::PR_SET_CHILD_SUBREAPER,
                self.previous_subreaper,
                0,
                0,
                0,
            );
        }
    }
}

/// An unconfirmed group must retain its fixture/state. This value grants no
/// permission to clean up or to treat the primary child's exit as completion.
#[derive(Debug)]
pub struct UnconfirmedStop {
    pub group: i32,
}

pub struct Family {
    worker: Child,
    group: i32,
    worker_exit: Option<i32>,
    worker_signal: Option<i32>,
    stopping: Option<(i32, Duration)>,
    killed: bool,
    receiver: SlotReceiver,
    slot: Option<OwnedFd>,
}
impl Family {
    /// The outer supervisor must create the worker with process_group(0).
    /// Group membership is checked before taking supervision ownership.
    pub fn new(mut worker: Child, receiver: SlotReceiver) -> io::Result<Self> {
        let group = worker.id() as i32;
        if unsafe { libc::getpgid(group) } != group {
            let _ = worker.kill();
            let _ = worker.wait();
            return Err(io::Error::other(
                "worker must lead the supervisor-created group",
            ));
        }
        Ok(Self {
            worker,
            group,
            worker_exit: None,
            worker_signal: None,
            stopping: None,
            killed: false,
            receiver,
            slot: None,
        })
    }
    /// The receiver remains nonblocking for later SCM_RIGHTS polling. Only the
    /// fixed-size start/ACK exchange temporarily uses bounded blocking I/O.
    pub fn start_worker(&self) -> io::Result<()> {
        self.receiver.0.set_nonblocking(false)?;
        let result = (|| {
            self.receiver.0.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
            self.receiver.0.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
            let frame = handshake_frame(unsafe { libc::getpid() }, self.group);
            (&self.receiver.0).write_all(&frame)?;
            let mut ack = [0u8; 16];
            (&self.receiver.0).read_exact(&mut ack)?;
            if ack != frame {
                return Err(handshake_error());
            }
            Ok(())
        })();
        self.receiver.resume_slot_polling()?;
        result
    }
    pub fn group(&self) -> i32 {
        self.group
    }
    pub fn worker_exit(&self) -> Option<i32> {
        self.worker_exit
    }
    pub fn signal(&mut self, signal: i32, clock: &dyn Clock) -> io::Result<()> {
        if self.stopping.is_none() {
            self.stopping = Some((signal, clock.now()));
            send(self.group, signal)?;
            self.killed = signal == libc::SIGKILL;
        }
        Ok(())
    }
    /// Reap only this family. Linux subreaper adoption includes orphaned
    /// grandchildren; on macOS init reaps orphans and group disappearance is
    /// still required. A zombie/EPERM is never classified as a stopped group.
    fn stopped(&mut self) -> io::Result<bool> {
        if self.slot.is_none() {
            self.slot = self.receiver.receive()?;
        }
        loop {
            let mut status = 0;
            let pid = unsafe { libc::waitpid(-self.group, &mut status, libc::WNOHANG) };
            if pid > 0 {
                if pid == self.worker.id() as i32 {
                    self.worker_signal = libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status));
                    self.worker_exit = Some(if libc::WIFSIGNALED(status) {
                        128 + libc::WTERMSIG(status)
                    } else {
                        libc::WEXITSTATUS(status)
                    });
                }
                continue;
            }
            if pid < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.raw_os_error() != Some(libc::ECHILD) {
                    return Err(error);
                }
            }
            break;
        }
        if group_stopped(self.group)? {
            self.slot.take();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    /// An absolute confirmation budget never resets on progress/heartbeat.
    /// Failure preserves this handle and the caller's state for later recovery.
    pub fn confirm_stopped(
        &mut self,
        clock: &dyn Clock,
        budget: Duration,
    ) -> Result<(), UnconfirmedStop> {
        let start = clock.now();
        loop {
            if self.stopped().unwrap_or(false) {
                return Ok(());
            }
            if clock.now().saturating_sub(start) >= budget {
                return Err(UnconfirmedStop { group: self.group });
            }
            clock.sleep(RETRY_INTERVAL);
        }
    }
    /// Cleanup is permitted only after every member has stopped and been reaped.
    pub fn cleanup_if_stopped(
        &mut self,
        clock: &dyn Clock,
        budget: Duration,
        cleanup: impl FnOnce(),
    ) -> Result<(), UnconfirmedStop> {
        self.confirm_stopped(clock, budget)?;
        cleanup();
        Ok(())
    }
    /// No execution deadline is introduced here: the outer wall timer owns it.
    /// After cancellation, confirmation is bounded separately from execution.
    /// On error the caller must retain this handle (and state), never run cleanup.
    pub fn wait(&mut self, clock: &dyn Clock, stop_budget: Duration) -> io::Result<i32> {
        let mut stop_started = None;
        loop {
            let signal = SIGNAL.swap(0, Ordering::Relaxed);
            if signal != 0 {
                self.signal(signal, clock)?;
            }
            if let Some((_, start)) = self.stopping {
                if !self.killed && clock.now().saturating_sub(start) >= SIGNAL_GRACE {
                    send(self.group, libc::SIGKILL)?;
                    self.killed = true;
                }
            }
            if self.stopped()? {
                return Ok(self
                    .stopping
                    .map(|(signal, _)| 128 + signal)
                    .or(self.worker_exit)
                    .unwrap_or(1));
            }
            // Keyboard signals target the foreground worker group, not this
            // group-external supervisor. A reaped worker's signal is therefore
            // a cancellation request too; ordinary exit 128+N is not.
            if self.stopping.is_none() {
                if let Some(signal) = self
                    .worker_signal
                    .filter(|signal| [libc::SIGINT, libc::SIGTERM, libc::SIGHUP].contains(signal))
                {
                    self.signal(signal, clock)?;
                }
            }
            if self.killed || (self.worker_signal.is_some() && self.stopping.is_none()) {
                // Stop confirmation measures OS progress in real monotonic time;
                // a test's accelerated grace clock must not shorten this budget.
                let started = *stop_started.get_or_insert_with(Instant::now);
                if started.elapsed() >= stop_budget {
                    return Err(io::Error::other(format!(
                        "group {} stop unconfirmed; retain state and slot",
                        self.group
                    )));
                }
            }
            clock.sleep(RETRY_INTERVAL);
        }
    }
}
fn group_stopped(group: i32) -> io::Result<bool> {
    if unsafe { libc::kill(-group, 0) } == 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(true),
        // Darwin killpg1 skips SZOMB members and can return EPERM while
        // init is still reaping the group. This is pending, never stopped.
        #[cfg(target_os = "macos")]
        Some(libc::EPERM) => Ok(false),
        _ => Err(error),
    }
}
fn send(group: i32, signal: i32) -> io::Result<()> {
    if unsafe { libc::kill(-group, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        // A zombie-only Darwin group is not signalable. Keep the original
        // cancellation and its deadline; only group_stopped may release it.
        #[cfg(target_os = "macos")]
        Some(libc::EPERM) => Ok(()),
        _ => Err(error),
    }
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_MAGIC: &[u8; 8] = b"HCSUP001";
fn handshake_frame(supervisor: i32, group: i32) -> [u8; 16] {
    let mut frame = [0u8; 16];
    frame[..8].copy_from_slice(HANDSHAKE_MAGIC);
    frame[8..12].copy_from_slice(&supervisor.to_be_bytes());
    frame[12..].copy_from_slice(&group.to_be_bytes());
    frame
}
fn handshake_error() -> io::Error {
    io::Error::other("supervisor handshake failed")
}

#[cfg(target_os = "linux")]
fn peer_pid(socket: &UnixStream) -> io::Result<i32> {
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of_val(&peer) || peer.uid != unsafe { libc::geteuid() } {
        return Err(handshake_error());
    }
    Ok(peer.pid)
}
#[cfg(target_os = "macos")]
fn peer_pid(socket: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of_val(&pid) {
        return Err(handshake_error());
    }
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_handshake_accepts_closed_peer() {
        let (receiver, sender) = slot_channel().unwrap();
        receiver.0.set_nonblocking(false).unwrap();
        receiver
            .0
            .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
            .unwrap();
        receiver
            .0
            .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
            .unwrap();
        // The ACK may already be consumed when a short-lived worker exits.
        drop(sender);
        receiver.resume_slot_polling().unwrap();
        assert!(receiver.receive().unwrap().is_none());
    }

    #[test]
    fn zombie_group_remains_unconfirmed_until_reaped() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]).process_group(0);
        let mut child = spawn(&mut command).unwrap();
        let group = child.id() as i32;
        // Observe exit without reaping: Darwin reports EPERM for this real
        // zombie-only group. The supervisor must keep waiting, not fail or free.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let observed = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        let pending = group_stopped(group);
        let term = send(group, libc::SIGTERM);
        let kill = send(group, libc::SIGKILL);
        // Reap before asserting so a RED run cannot leave a fixture zombie.
        child.wait().unwrap();
        assert_eq!(observed, 0);
        assert!(!pending.unwrap(), "unreaped group released early");
        assert!(term.is_ok(), "zombie TERM must remain pending: {term:?}");
        assert!(kill.is_ok(), "zombie KILL must remain pending: {kill:?}");
        assert!(group_stopped(group).unwrap());
    }
}
