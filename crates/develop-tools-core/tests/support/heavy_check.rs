use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// cargo は package 自身の integration test に非 default の feature を付けない。
// default にすると依存先の通常 build へ公開される。source は `src/test_support.rs` の 1 つ。
#[path = "../../src/test_support.rs"]
mod test_support;

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

/// `heavy-check-harness` の path。`HEAVY_CHECK_TEST_HARNESS` があればそれ、無ければ cargo の
/// JSON が報告した executable の写し。test 実行ファイルの 2 つ上を profile directory と推さない。
#[allow(dead_code)]
pub fn harness_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("HEAVY_CHECK_TEST_HARNESS") {
        return PathBuf::from(path);
    }
    static PLACED: OnceLock<PathBuf> = OnceLock::new();
    PLACED.get_or_init(build_harness_executable).clone()
}

fn build_harness_executable() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("heavy-check-harness");
    test_support::build_placed_executables(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &dir,
        &[
            "build",
            "--locked",
            "-p",
            "develop-tools-core",
            "--example",
            "heavy-check-harness",
        ],
        |name| name == "heavy-check-harness",
    )
    .remove("heavy-check-harness")
    .expect("cargo が heavy-check-harness の executable を報告しなかった")
}

/// Tests use the built example even when no runner environment is provided.
#[allow(dead_code)]
pub fn harness_command() -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let path = harness_executable();
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
