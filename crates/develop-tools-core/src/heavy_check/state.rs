//! Account-owned state. No environment path can change the host identity.
use std::ffi::{CStr, CString, OsString};
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub const STATE_RELATIVE: &str = ".cache/develop-tools/heavy-check";
pub const UNAVAILABLE_EXIT: i32 = 75;
const SLOT_LOCK: &str = "slot.lock";
const QUEUE_LOCK: &str = "queue.lock";
const QUEUE_DIRECTORY: &str = "queue";

#[derive(Debug, Clone)]
pub struct Account {
    pub home: PathBuf,
    pub uid: u32,
}

/// Injectable OS lookup. Production always uses getpwuid_r, never HOME.
pub trait AccountSource {
    fn account(&self) -> Result<Account, StateError>;
    fn environment_home(&self) -> Option<PathBuf>;
}

pub struct OsAccount;
impl AccountSource for OsAccount {
    fn account(&self) -> Result<Account, StateError> {
        // getpwuid_r owns no global passwd storage and does not consult HOME.
        let uid = unsafe { libc::geteuid() };
        let mut size = 16 * 1024;
        loop {
            let mut buffer = vec![0u8; size];
            let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut result = std::ptr::null_mut();
            let code = unsafe {
                libc::getpwuid_r(
                    uid,
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut result,
                )
            };
            if code == libc::ERANGE && size < 1024 * 1024 {
                size *= 2;
                continue;
            }
            if code != 0 || result.is_null() {
                return Err(StateError::AccountLookup);
            }
            let entry = unsafe { entry.assume_init() };
            if entry.pw_dir.is_null() || entry.pw_uid != uid {
                return Err(StateError::AccountLookup);
            }
            let home = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes().to_vec();
            let home = PathBuf::from(OsString::from_vec(home));
            absolute_components(&home)?;
            return Ok(Account { home, uid });
        }
    }
    fn environment_home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeReason {
    InvalidPath,
    Symlink,
    Kind,
    Owner,
    Mode,
    Replaced,
    HardLink,
}

/// I6 can classify lookup, unsafe state and OS denial without parsing messages.
#[derive(Debug)]
pub enum StateError {
    AccountLookup,
    Unsafe { path: PathBuf, reason: UnsafeReason },
    Io { path: PathBuf, error: io::Error },
}
impl StateError {
    pub fn exit_code(&self) -> i32 {
        UNAVAILABLE_EXIT
    }
    fn denied_errno(&self) -> Option<i32> {
        match self {
            Self::Io { error, .. } => error
                .raw_os_error()
                .filter(|code| [libc::EROFS, libc::EACCES, libc::EPERM].contains(code)),
            _ => None,
        }
    }
}
impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountLookup => write!(f, "account lookup failed; tests not run"),
            Self::Unsafe { reason, .. } => {
                write!(f, "unsafe heavy-check state ({reason:?}); tests not run")
            }
            Self::Io { error, .. } => write!(
                f,
                "heavy-check state unavailable (errno {:?}); tests not run",
                error.raw_os_error()
            ),
        }
    }
}
impl std::error::Error for StateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Host,
    HeadlessSandbox,
    Isolated { account_errno: i32 },
}

/// Core-only state-root injection. No production environment override exists.
#[derive(Debug)]
pub struct StateLayout {
    relative: PathBuf,
}
impl Default for StateLayout {
    fn default() -> Self {
        Self {
            relative: STATE_RELATIVE.into(),
        }
    }
}
impl StateLayout {
    pub fn new(relative: PathBuf) -> Result<Self, StateError> {
        if relative.is_absolute()
            || relative.components().count() < 3
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(unsafe_path(&relative, UnsafeReason::InvalidPath));
        }
        Ok(Self { relative })
    }
}

/// OS-stat and race seam for unprivileged fixtures; production uses native values.
/// Hooks must be trusted by the caller, like AccountSource and StateLayout.
pub trait FilesystemHooks {
    fn before_prepare(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn owner(&self, _: &Path, actual: u32) -> u32 {
        actual
    }
    fn before_validate(&self, _: &Path) {}
}
pub struct NativeFilesystem;
impl FilesystemHooks for NativeFilesystem {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(meta: &Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
}
#[derive(Debug, Clone, Copy)]
enum Policy {
    Ancestor,
    Home,
    Shared,
    Private,
    Lock,
}

#[derive(Debug)]
struct Entry {
    file: File,
    path: PathBuf,
    name: CString,
    parent: usize,
    identity: Identity,
    policy: Policy,
}

/// Holds every ancestor and lock FD. Call validate immediately before admission.
/// Never remove, rename or truncate slot.lock or queue.lock.
#[derive(Debug)]
pub struct State {
    path: PathBuf,
    scope: Scope,
    uid: u32,
    entries: Vec<Entry>,
    slot: usize,
    guard: usize,
    queue: usize,
}
impl State {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn scope(&self) -> Scope {
        self.scope
    }
    pub fn slot(&self) -> &File {
        &self.entries[self.slot].file
    }
    pub fn guard(&self) -> &File {
        &self.entries[self.guard].file
    }
    pub fn queue(&self) -> &File {
        &self.entries[self.queue].file
    }
    /// Queue entries use the same regular-file policy as I1's stable locks.
    /// The caller holds queue.lock and has validated the retained state FDs.
    pub(crate) fn validate_queue_file(&self, name: &CStr, file: &File) -> Result<(), StateError> {
        let path = self.entries[self.queue]
            .path
            .join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let meta = file.metadata().map_err(|e| io_error(&path, e))?;
        check(&path, &meta, Policy::Lock, self.uid, &NativeFilesystem)?;
        let stat = stat_at(self.queue(), name).map_err(|e| io_error(&path, e))?;
        check_stat(&path, &stat, Policy::Lock, self.uid, &NativeFilesystem)?;
        if stat_identity(&stat) != Identity::of(&meta) {
            return Err(unsafe_path(&path, UnsafeReason::Replaced));
        }
        Ok(())
    }
    pub fn validate(&self, hooks: &impl FilesystemHooks) -> Result<(), StateError> {
        for (index, entry) in self.entries.iter().enumerate() {
            let meta = entry
                .file
                .metadata()
                .map_err(|e| io_error(&entry.path, e))?;
            check(&entry.path, &meta, entry.policy, self.uid, hooks)?;
            if Identity::of(&meta) != entry.identity {
                return Err(unsafe_path(&entry.path, UnsafeReason::Replaced));
            }
            if index == 0 {
                continue;
            }
            let parent = &self.entries[entry.parent].file;
            let current = stat_at(parent, &entry.name)
                .map_err(|_| unsafe_path(&entry.path, UnsafeReason::Replaced))?;
            if stat_identity(&current) != entry.identity {
                return Err(unsafe_path(&entry.path, UnsafeReason::Replaced));
            }
        }
        Ok(())
    }
}

pub fn prepare(
    source: &impl AccountSource,
    layout: &StateLayout,
    hooks: &impl FilesystemHooks,
) -> Result<State, StateError> {
    let account = source.account()?;
    match prepare_at(&account.home, account.uid, layout, Scope::Host, hooks) {
        Ok(state) => Ok(state),
        Err(error) => {
            let Some(errno) = error.denied_errno() else {
                return Err(error);
            };
            inspect_existing(&account, layout, hooks)?;
            let Some(home) = source.environment_home() else {
                return Err(error);
            };
            // Compare normalized components and, when accessible, actual identity.
            let parts = absolute_components(&home)?;
            if parts == absolute_components(&account.home)? {
                return Err(error);
            }
            if let (Ok(left), Ok(right)) = (
                std::fs::symlink_metadata(&home),
                std::fs::symlink_metadata(&account.home),
            ) {
                if Identity::of(&left) == Identity::of(&right) {
                    return Err(error);
                }
            }
            prepare_at(
                &home,
                account.uid,
                layout,
                Scope::Isolated {
                    account_errno: errno,
                },
                hooks,
            )
        }
    }
}

/// Explicit executor context always uses isolated HOME, even on a writable host.
pub fn prepare_headless(
    source: &impl AccountSource,
    hooks: &impl FilesystemHooks,
) -> Result<State, StateError> {
    let home = source.environment_home().ok_or(StateError::AccountLookup)?;
    prepare_at(
        &home,
        unsafe { libc::geteuid() },
        &StateLayout::default(),
        Scope::HeadlessSandbox,
        hooks,
    )
}

fn open_home(
    home: &Path,
    uid: u32,
    layout: &StateLayout,
    scope: Scope,
    hooks: &impl FilesystemHooks,
) -> Result<State, StateError> {
    let components = absolute_components(home)?;
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .map_err(|e| io_error(Path::new("/"), e))?;
    let meta = root.metadata().map_err(|e| io_error(Path::new("/"), e))?;
    check(Path::new("/"), &meta, Policy::Ancestor, uid, hooks)?;
    let mut state = State {
        path: home.join(&layout.relative),
        scope,
        uid,
        entries: vec![Entry {
            file: root,
            path: "/".into(),
            name: CString::new("/").unwrap(),
            parent: 0,
            identity: Identity::of(&meta),
            policy: Policy::Ancestor,
        }],
        slot: 0,
        guard: 0,
        queue: 0,
    };
    for (index, part) in components.iter().enumerate() {
        let policy = if index + 1 == components.len() {
            Policy::Home
        } else {
            Policy::Ancestor
        };
        state.add(part, policy, false, hooks)?;
    }
    Ok(state)
}

/// A write denial must not hide unsafe siblings that preparation never reached.
/// This pass creates nothing and opens no lock for writing, so EROFS can still
/// fall back after all existing state has been checked. Unreadable state fails
/// closed: without inspecting it we cannot assert that it is safe.
fn inspect_existing(
    account: &Account,
    layout: &StateLayout,
    hooks: &impl FilesystemHooks,
) -> Result<(), StateError> {
    let mut state = open_home(&account.home, account.uid, layout, Scope::Host, hooks)?;
    for (index, part) in layout.relative.components().enumerate() {
        let Component::Normal(part) = part else {
            return Err(unsafe_path(&layout.relative, UnsafeReason::InvalidPath));
        };
        let parent = state.entries.len() - 1;
        let name = CString::new(part.as_encoded_bytes())
            .map_err(|_| unsafe_path(&layout.relative, UnsafeReason::InvalidPath))?;
        match stat_at(&state.entries[parent].file, &name) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return state.validate(hooks),
            Err(error) => {
                return Err(state.checked_io_error(
                    &state.entries[parent].path.join(part),
                    error,
                    hooks,
                ));
            }
        }
        state.add(
            part,
            if index < 2 {
                Policy::Shared
            } else {
                Policy::Private
            },
            false,
            hooks,
        )?;
    }
    let root = state.entries.len() - 1;
    let mut snapshots = Vec::new();
    for (name, policy) in [
        (SLOT_LOCK, Policy::Lock),
        (QUEUE_LOCK, Policy::Lock),
        (QUEUE_DIRECTORY, Policy::Private),
    ] {
        let path = state.path.join(name);
        let component = CString::new(name).unwrap();
        match stat_at(&state.entries[root].file, &component) {
            Ok(stat) => {
                check_stat(&path, &stat, policy, account.uid, hooks)?;
                snapshots.push((path, component, stat_identity(&stat), policy));
                if matches!(policy, Policy::Private) {
                    state.add_at(root, std::ffi::OsStr::new(name), policy, false, hooks)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(state.checked_io_error(&path, error, hooks)),
        }
    }
    state.validate(hooks)?;
    for (path, name, identity, policy) in snapshots {
        let current = stat_at(&state.entries[root].file, &name)
            .map_err(|_| unsafe_path(&path, UnsafeReason::Replaced))?;
        check_stat(&path, &current, policy, account.uid, hooks)?;
        if stat_identity(&current) != identity {
            return Err(unsafe_path(&path, UnsafeReason::Replaced));
        }
    }
    Ok(())
}

fn prepare_at(
    home: &Path,
    uid: u32,
    layout: &StateLayout,
    scope: Scope,
    hooks: &impl FilesystemHooks,
) -> Result<State, StateError> {
    let mut state = open_home(home, uid, layout, scope, hooks)?;
    // Safety is checked before a simulated or real write denial can permit fallback.
    hooks.before_prepare(home).map_err(|e| io_error(home, e))?;
    for (index, part) in layout.relative.components().enumerate() {
        let Component::Normal(part) = part else {
            return Err(unsafe_path(&layout.relative, UnsafeReason::InvalidPath));
        };
        state.add(
            part,
            if index < 2 {
                Policy::Shared
            } else {
                Policy::Private
            },
            true,
            hooks,
        )?;
    }
    let root_index = state.entries.len() - 1;
    state.slot = state.add_at(
        root_index,
        std::ffi::OsStr::new(SLOT_LOCK),
        Policy::Lock,
        true,
        hooks,
    )?;
    state.guard = state.add_at(
        root_index,
        std::ffi::OsStr::new(QUEUE_LOCK),
        Policy::Lock,
        true,
        hooks,
    )?;
    state.queue = state.add_at(
        root_index,
        std::ffi::OsStr::new(QUEUE_DIRECTORY),
        Policy::Private,
        true,
        hooks,
    )?;
    hooks.before_validate(&state.path);
    state.validate(hooks)?;
    Ok(state)
}

impl State {
    fn checked_io_error(
        &self,
        path: &Path,
        error: io::Error,
        hooks: &impl FilesystemHooks,
    ) -> StateError {
        match self.validate(hooks) {
            Err(unsafe_error) => unsafe_error,
            Ok(()) => io_error(path, error),
        }
    }
    fn add(
        &mut self,
        name: &std::ffi::OsStr,
        policy: Policy,
        create: bool,
        hooks: &impl FilesystemHooks,
    ) -> Result<usize, StateError> {
        self.add_at(self.entries.len() - 1, name, policy, create, hooks)
    }
    fn add_at(
        &mut self,
        parent: usize,
        name: &std::ffi::OsStr,
        policy: Policy,
        create: bool,
        hooks: &impl FilesystemHooks,
    ) -> Result<usize, StateError> {
        self.validate(hooks)?;
        let path = self.entries[parent].path.join(name);
        let name = CString::new(name.as_encoded_bytes())
            .map_err(|_| unsafe_path(&path, UnsafeReason::InvalidPath))?;
        let directory = !matches!(policy, Policy::Lock);
        let parent_file = &self.entries[parent].file;
        let before = match stat_at(parent_file, &name) {
            Ok(stat) => {
                check_stat(&path, &stat, policy, self.uid, hooks)?;
                Some(stat)
            }
            Err(e) if create && e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(self.checked_io_error(&path, e, hooks)),
        };
        // A denial after lstat must not hide a swapped or newly unsafe entry.
        let checked_error = |error| {
            if let Err(error) = self.validate(hooks) {
                return error;
            }
            match stat_at(parent_file, &name) {
                Ok(current) => {
                    if let Err(error) = check_stat(&path, &current, policy, self.uid, hooks) {
                        return error;
                    }
                    if before
                        .as_ref()
                        .is_some_and(|previous| stat_identity(previous) != stat_identity(&current))
                    {
                        return unsafe_path(&path, UnsafeReason::Replaced);
                    }
                }
                Err(_) if before.is_some() => return unsafe_path(&path, UnsafeReason::Replaced),
                Err(_) => {}
            }
            io_error(&path, error)
        };
        if before.is_none() && directory {
            let result = unsafe { libc::mkdirat(parent_file.as_raw_fd(), name.as_ptr(), 0o700) };
            if result != 0 {
                // Concurrent mkdir is fine, but its resulting inode/mode still must pass.
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(checked_error(error));
                }
            }
        }
        let flags = libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if directory {
                libc::O_RDONLY | libc::O_DIRECTORY
            } else {
                libc::O_RDWR
            };
        let mut fd = -1;
        if before.is_none() && !directory {
            fd = unsafe {
                libc::openat(
                    parent_file.as_raw_fd(),
                    name.as_ptr(),
                    flags | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )
            };
            if fd < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(checked_error(io::Error::last_os_error()));
            }
        }
        if fd < 0 {
            fd = unsafe { libc::openat(parent_file.as_raw_fd(), name.as_ptr(), flags) };
        }
        if fd < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            ) {
                return Err(unsafe_path(&path, UnsafeReason::Replaced));
            }
            return Err(checked_error(error));
        }
        // SAFETY: openat returned one owned FD, transferred exactly once to File.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|e| io_error(&path, e))?;
        check(&path, &meta, policy, self.uid, hooks)?;
        if let Some(stat) = before {
            if stat_identity(&stat) != Identity::of(&meta) {
                return Err(unsafe_path(&path, UnsafeReason::Replaced));
            }
        }
        let identity = Identity::of(&meta);
        self.entries.push(Entry {
            file,
            path,
            name,
            parent,
            identity,
            policy,
        });
        self.validate(hooks)?;
        Ok(self.entries.len() - 1)
    }
}

fn absolute_components(path: &Path) -> Result<Vec<&std::ffi::OsStr>, StateError> {
    if !path.is_absolute() {
        return Err(unsafe_path(path, UnsafeReason::InvalidPath));
    }
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(value) => parts.push(value),
            _ => return Err(unsafe_path(path, UnsafeReason::InvalidPath)),
        }
    }
    if parts.is_empty() {
        return Err(unsafe_path(path, UnsafeReason::InvalidPath));
    }
    Ok(parts)
}
fn stat_at(parent: &File, name: &CStr) -> io::Result<libc::stat> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { stat.assume_init() })
    }
}
#[allow(clippy::unnecessary_cast)] // macOS dev_t is signed; Linux dev_t is u64.
fn stat_identity(stat: &libc::stat) -> Identity {
    Identity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }
}
#[allow(clippy::unnecessary_cast)] // stat field widths differ on macOS.
fn check_stat(
    path: &Path,
    stat: &libc::stat,
    policy: Policy,
    uid: u32,
    hooks: &impl FilesystemHooks,
) -> Result<(), StateError> {
    let kind = stat.st_mode & libc::S_IFMT;
    if kind == libc::S_IFLNK {
        return Err(unsafe_path(path, UnsafeReason::Symlink));
    }
    if kind
        != if matches!(policy, Policy::Lock) {
            libc::S_IFREG
        } else {
            libc::S_IFDIR
        }
    {
        return Err(unsafe_path(path, UnsafeReason::Kind));
    }
    check_attributes(
        path,
        stat.st_uid,
        stat.st_mode as u32 & 0o7777,
        stat.st_nlink as u64,
        policy,
        uid,
        hooks,
    )
}
fn check(
    path: &Path,
    meta: &Metadata,
    policy: Policy,
    uid: u32,
    hooks: &impl FilesystemHooks,
) -> Result<(), StateError> {
    if matches!(policy, Policy::Lock) && !meta.is_file()
        || !matches!(policy, Policy::Lock) && !meta.is_dir()
    {
        return Err(unsafe_path(path, UnsafeReason::Kind));
    }
    check_attributes(
        path,
        meta.uid(),
        meta.mode() & 0o7777,
        meta.nlink(),
        policy,
        uid,
        hooks,
    )
}
fn check_attributes(
    path: &Path,
    owner: u32,
    mode: u32,
    links: u64,
    policy: Policy,
    uid: u32,
    hooks: &impl FilesystemHooks,
) -> Result<(), StateError> {
    let owner = hooks.owner(path, owner);
    if owner != uid && !(matches!(policy, Policy::Ancestor) && owner == 0) {
        return Err(unsafe_path(path, UnsafeReason::Owner));
    }
    let valid = match policy {
        Policy::Private => mode == 0o700,
        Policy::Lock => mode == 0o600,
        _ => mode & 0o7022 == 0,
    };
    if !valid {
        return Err(unsafe_path(path, UnsafeReason::Mode));
    }
    if matches!(policy, Policy::Lock) && links != 1 {
        return Err(unsafe_path(path, UnsafeReason::HardLink));
    }
    Ok(())
}
fn unsafe_path(path: &Path, reason: UnsafeReason) -> StateError {
    StateError::Unsafe {
        path: path.to_owned(),
        reason,
    }
}
fn io_error(path: &Path, error: io::Error) -> StateError {
    StateError::Io {
        path: path.to_owned(),
        error,
    }
}
