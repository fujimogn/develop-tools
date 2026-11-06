use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use develop_tools_core::heavy_check::state::{Account, AccountSource, StateError};

pub struct Source {
    pub account: PathBuf,
    pub home: Option<PathBuf>,
}

impl AccountSource for Source {
    fn account(&self) -> Result<Account, StateError> {
        Ok(Account {
            home: self.account.clone(),
            uid: unsafe { libc::geteuid() },
        })
    }
    fn environment_home(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

pub fn directory(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_owned()
}

pub fn fixture() -> tempfile::TempDir {
    // Keep fixtures below the worktree: /tmp is a deliberately unsafe ancestor.
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/heavy-check-fixtures");
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let dir = tempfile::Builder::new()
        .prefix("develop-meta-heavy-state-")
        .tempdir_in(base)
        .unwrap();
    eprintln!("session fixture created: {}", dir.path().display());
    dir
}

/// Tests use the built example even when no runner environment is provided.
#[allow(dead_code)]
pub fn harness_command() -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let path = std::env::var_os("HEAVY_CHECK_TEST_HARNESS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .and_then(Path::parent)
                .expect("test executable in target/<profile>/deps")
                .join("examples/heavy-check-harness")
        });
    assert!(
        path.is_file(),
        "build heavy-check-harness example first: {}",
        path.display()
    );
    let mut command = std::process::Command::new(path);
    for name in [
        "DEVELOP_HEAVY_CHECK_SLOT_FD",
        "DEVELOP_HEAVY_CHECK_CONTROL_FD",
    ] {
        if let Some(fd) = std::env::var_os(name)
            .and_then(|v| v.to_str()?.parse::<i32>().ok())
            .filter(|fd| *fd > 2)
        {
            // close affects only the fixture child; explicit flock unlock would
            // release the outer wrapper's lock too.
            unsafe {
                command.pre_exec(move || {
                    libc::close(fd);
                    Ok(())
                });
            }
        }
        command.env_remove(name);
    }
    command
}
