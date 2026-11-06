#[path = "support/heavy_check.rs"]
mod support;

use develop_tools_core::heavy_check::state::*;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use support::{Source, directory, fixture};

#[test]
fn same_account_different_home_uses_one_root() {
    if let Some(account) = std::env::var_os("HEAVY_STATE_TEST_ACCOUNT") {
        let source = Source {
            account: account.into(),
            home: std::env::var_os("HOME").map(Into::into),
        };
        let state = prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
        println!("ROOT={}", state.path().display());
        println!("ACCOUNT={}", OsAccount.account().unwrap().home.display());
        assert_eq!(state.scope(), Scope::Host);
        return;
    }
    let temp = fixture();
    let account = directory(&temp.path().join("account"));
    let expected = account.join(STATE_RELATIVE);
    let os_home = OsAccount.account().unwrap().home;
    for name in ["home-a", "home-b"] {
        let home = directory(&temp.path().join(name));
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "same_account_different_home_uses_one_root",
                "--nocapture",
            ])
            .env("HEAVY_STATE_TEST_ACCOUNT", &account)
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", home.join("xdg"))
            .current_dir(&home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("ROOT={}", expected.display()))
        );
        assert!(!home.join(STATE_RELATIVE).exists());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("ACCOUNT={}", os_home.display()))
        );
    }
    for path in [
        &account,
        &account.join(".cache"),
        &account.join(".cache/develop-tools"),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let source = Source {
        account,
        home: None,
    };
    let first = prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
    fs::write(first.path().join("slot.lock"), b"stable").unwrap();
    let ino = first.slot().metadata().unwrap().ino();
    let second = prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
    assert_eq!(ino, second.slot().metadata().unwrap().ino());
    assert_eq!(
        fs::read(second.path().join("slot.lock")).unwrap(),
        b"stable"
    );
    for name in ["slot.lock", "queue.lock"] {
        assert_eq!(
            fs::metadata(first.path().join(name)).unwrap().mode() & 0o7777,
            0o600
        );
    }
    assert_eq!(
        fs::metadata(source.account.join(".cache/develop-tools"))
            .unwrap()
            .mode()
            & 0o7777,
        0o755
    );
    let injected = prepare(
        &source,
        &StateLayout::new("fixture/custom/state".into()).unwrap(),
        &NativeFilesystem,
    )
    .unwrap();
    assert_eq!(injected.path(), source.account.join("fixture/custom/state"));
    for fd in [first.slot(), first.guard(), first.queue()] {
        use std::os::fd::AsRawFd;
        assert_ne!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    for path in [first.path().to_owned(), first.path().join("queue")] {
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, 0o700);
    }
}

struct Hooks {
    denied: Option<i32>,
    wrong_owner: bool,
    replace_parent: bool,
}
impl FilesystemHooks for Hooks {
    fn before_prepare(&self, _: &Path) -> io::Result<()> {
        self.denied
            .map_or(Ok(()), |code| Err(io::Error::from_raw_os_error(code)))
    }
    fn owner(&self, path: &Path, actual: u32) -> u32 {
        if self.wrong_owner && path.ends_with("heavy-check") {
            actual.wrapping_add(1)
        } else {
            actual
        }
    }
    fn before_validate(&self, root: &Path) {
        if self.replace_parent {
            let parent = root.parent().unwrap();
            fs::rename(parent, parent.with_file_name("moved-parent")).unwrap();
            directory(parent);
            directory(root);
        }
    }
}

#[test]
fn unsafe_component_never_falls_back() {
    for kind in [
        "symlink",
        "mode",
        "owner",
        "parent",
        "file-link",
        "file-mode",
        "hard-link",
        "no-access-file",
        "unsafe-cache",
        "fifo",
        "lookup",
    ] {
        let temp = fixture();
        let account = directory(&temp.path().join("account"));
        let home = directory(&temp.path().join("fallback"));
        let source = Source {
            account: account.clone(),
            home: Some(home.clone()),
        };
        let root = account.join(STATE_RELATIVE);
        let state = prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
        drop(state);
        match kind {
            "symlink" => {
                fs::rename(&root, root.with_file_name("old")).unwrap();
                symlink(root.with_file_name("old"), &root).unwrap();
            }
            "mode" => fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap(),
            "file-link" => {
                fs::remove_file(root.join("slot.lock")).unwrap();
                symlink(root.join("queue.lock"), root.join("slot.lock")).unwrap();
            }
            "file-mode" => {
                fs::set_permissions(root.join("slot.lock"), fs::Permissions::from_mode(0o644))
                    .unwrap()
            }
            "hard-link" => fs::hard_link(root.join("slot.lock"), root.join("alias")).unwrap(),
            "no-access-file" => {
                fs::set_permissions(root.join("slot.lock"), fs::Permissions::from_mode(0o0))
                    .unwrap()
            }
            "unsafe-cache" => {
                fs::set_permissions(account.join(".cache"), fs::Permissions::from_mode(0o777))
                    .unwrap()
            }
            "fifo" => {
                fs::remove_file(root.join("slot.lock")).unwrap();
                let path =
                    std::ffi::CString::new(root.join("slot.lock").as_os_str().as_encoded_bytes())
                        .unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            _ => {}
        }
        let hooks = Hooks {
            denied: None,
            wrong_owner: kind == "owner",
            replace_parent: kind == "parent",
        };
        let result = if kind == "lookup" {
            prepare(&FailedLookup, &StateLayout::default(), &hooks)
        } else {
            prepare(&source, &StateLayout::default(), &hooks)
        };
        let error = result.unwrap_err();
        assert_eq!(error.exit_code(), 75, "{kind}");
        assert!(!home.join(STATE_RELATIVE).exists(), "{kind} fell back");
        if kind == "parent" {
            assert!(matches!(
                error,
                StateError::Unsafe {
                    reason: UnsafeReason::Replaced,
                    ..
                }
            ));
        }
    }
}

struct FailedLookup;
impl AccountSource for FailedLookup {
    fn account(&self) -> Result<Account, StateError> {
        Err(StateError::AccountLookup)
    }
    fn environment_home(&self) -> Option<std::path::PathBuf> {
        panic!("lookup failure must not fallback")
    }
}

#[test]
fn denied_account_root_falls_back_only_to_distinct_safe_home() {
    let temp = fixture();
    let account = directory(&temp.path().join("account"));
    let home = directory(&temp.path().join("fallback"));
    // A safe existing queue must remain eligible for denial fallback, even
    // though the read-only inspection must not open its locks for writing.
    prepare(
        &Source {
            account: account.clone(),
            home: None,
        },
        &StateLayout::default(),
        &NativeFilesystem,
    )
    .unwrap();
    for errno in [
        libc::EROFS,
        libc::EACCES,
        libc::EPERM,
        libc::EIO,
        libc::ENOSPC,
    ] {
        let hooks = DenyAccount {
            account: account.clone(),
            errno,
        };
        let source = Source {
            account: account.clone(),
            home: Some(home.clone()),
        };
        let result = prepare(&source, &StateLayout::default(), &hooks);
        if [libc::EROFS, libc::EACCES, libc::EPERM].contains(&errno) {
            let state = result.unwrap();
            assert_eq!(
                state.scope(),
                Scope::Isolated {
                    account_errno: errno
                }
            );
            assert_eq!(state.path(), home.join(STATE_RELATIVE));
        } else {
            assert_eq!(result.unwrap_err().exit_code(), 75);
        }
        let same = Source {
            account: account.clone(),
            home: Some(account.join(".")),
        };
        assert_eq!(
            prepare(&same, &StateLayout::default(), &hooks)
                .unwrap_err()
                .exit_code(),
            75
        );
    }
    let link = temp.path().join("alias");
    symlink(&account, &link).unwrap();
    let source = Source {
        account: account.clone(),
        home: Some(link),
    };
    assert_eq!(
        prepare(
            &source,
            &StateLayout::default(),
            &DenyAccount {
                account: account.clone(),
                errno: libc::EROFS
            }
        )
        .unwrap_err()
        .exit_code(),
        75
    );
    fs::set_permissions(home.join(STATE_RELATIVE), fs::Permissions::from_mode(0o777)).unwrap();
    let source = Source {
        account: account.clone(),
        home: Some(home),
    };
    assert!(matches!(
        prepare(
            &source,
            &StateLayout::default(),
            &DenyAccount {
                account,
                errno: libc::EACCES
            }
        ),
        Err(StateError::Unsafe { .. })
    ));
}

struct DenyAccount {
    account: std::path::PathBuf,
    errno: i32,
}
impl FilesystemHooks for DenyAccount {
    fn before_prepare(&self, home: &Path) -> io::Result<()> {
        if home == self.account {
            Err(io::Error::from_raw_os_error(self.errno))
        } else {
            Ok(())
        }
    }
}

#[test]
fn denied_account_root_with_unsafe_queue_never_falls_back() {
    for name in ["queue.lock", "queue"] {
        for errno in [libc::EROFS, libc::EACCES, libc::EPERM] {
            let temp = fixture();
            let account = directory(&temp.path().join("account"));
            let home = directory(&temp.path().join("fallback"));
            let source = Source {
                account: account.clone(),
                home: Some(home.clone()),
            };
            let state = prepare(&source, &StateLayout::default(), &NativeFilesystem).unwrap();
            let root = state.path().to_owned();
            drop(state);
            let target = root.join(name);
            let original = target.with_extension("original");
            fs::rename(&target, &original).unwrap();
            symlink(&original, &target).unwrap();
            // Denial happens before queue.lock/queue could be opened, as with a
            // read-only account mount denying the earlier slot.lock O_RDWR.
            let hooks = DenyAccount {
                account: account.clone(),
                errno,
            };
            let error = prepare(&source, &StateLayout::default(), &hooks).unwrap_err();
            assert!(
                matches!(
                    error,
                    StateError::Unsafe {
                        reason: UnsafeReason::Symlink,
                        ..
                    }
                ),
                "{name}: {error:?}"
            );
            assert_eq!(error.exit_code(), 75);
            assert!(
                !home.join(STATE_RELATIVE).exists(),
                "{name} unexpectedly fell back"
            );
        }
    }
}
