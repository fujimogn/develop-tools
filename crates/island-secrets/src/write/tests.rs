//! 保存境界の契約（ADR-0014 v4 の `write-*` scenario）を実 filesystem で確かめる。

use std::os::unix::fs::symlink;

use super::*;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(TEMP_SUFFIX))
        .collect()
}

/// `write-owner-only`: 新規は 0600。
#[test]
fn a_new_file_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".env");

    write_secret_file(&target, &Secret::new("A='1'\n")).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "A='1'\n");
    assert_eq!(mode(&target), 0o600);
    assert!(leftovers(dir.path()).is_empty());
}

/// `write-repairs-loose-mode`: 既存の 0644 は置き換え後に 0600。
#[test]
fn a_loose_existing_file_becomes_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".env");
    fs::write(&target, "old\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();

    write_secret_file(&target, &Secret::new("new\n")).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "new\n");
    assert_eq!(mode(&target), 0o600);
}

/// `write-rejects-symlink`: 出力先が symlink なら書かず、link の先も変えない。
#[test]
fn a_symlink_target_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim");
    fs::write(&victim, "untouched\n").unwrap();
    let target = dir.path().join(".env");
    symlink(&victim, &target).unwrap();

    let error = write_secret_file(&target, &Secret::new("x\n")).unwrap_err();

    assert!(matches!(error, WriteError::Symlink(_)), "{error}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched\n");
    assert!(leftovers(dir.path()).is_empty());
}

/// rename の直前に差し込まれた symlink も拒否し、既存の出力先（ここでは link）を置き換えない。
#[test]
fn a_symlink_placed_just_before_rename_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim");
    fs::write(&victim, "untouched\n").unwrap();
    let target = dir.path().join(".env");

    let error = write_with(
        &target,
        &Secret::new("x\n"),
        || symlink(&victim, &target).unwrap(),
        sync_parent,
    )
    .unwrap_err();

    assert!(matches!(error, WriteError::Symlink(_)), "{error}");
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched\n");
    assert!(leftovers(dir.path()).is_empty());
}

/// `write-atomic-preserves-existing`: 途中で失敗したら既存の内容を残す。
#[test]
fn a_failure_before_rename_keeps_the_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".env");
    fs::write(&target, "keep\n").unwrap();

    // rename の直前に出力先を directory へすり替える → 置き換えを拒否する。
    let error = write_with(
        &target,
        &Secret::new("new\n"),
        || {
            fs::rename(&target, dir.path().join("moved")).unwrap();
            fs::create_dir(&target).unwrap();
        },
        sync_parent,
    )
    .unwrap_err();

    assert!(matches!(error, WriteError::NotRegularFile(_)), "{error}");
    assert_eq!(
        fs::read_to_string(dir.path().join("moved")).unwrap(),
        "keep\n"
    );
    assert!(leftovers(dir.path()).is_empty());
}

/// rename 後の directory fsync 失敗は新しい内容を残し、専用エラーを返す。
#[test]
fn a_parent_sync_failure_reports_replaced_but_unconfirmed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(".env");
    fs::write(&target, "old\n").unwrap();

    let error = write_with(
        &target,
        &Secret::new("new\n"),
        || {},
        |_| Err(io::Error::other("injected directory fsync failure")),
    )
    .unwrap_err();

    assert!(
        matches!(error, WriteError::DurabilityUnconfirmed(_, _)),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "new\n");
    assert!(leftovers(dir.path()).is_empty());
}

/// `write-ignores-temp`: 一時 file 名は `.env.*.secret-tmp` の形（先頭の `.` を重ねない）。
#[test]
fn the_temp_name_matches_the_ignore_patterns() {
    let temp = temp_path(Path::new("/repo/.env")).unwrap();
    let name = temp.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with(".env.") && !name.starts_with("..env"),
        "{name}"
    );
    assert!(name.ends_with(TEMP_SUFFIX), "{name}");
    assert_eq!(temp.parent(), Some(Path::new("/repo")));

    let dev_vars = temp_path(Path::new("apps/api/.dev.vars")).unwrap();
    let name = dev_vars.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(".dev.vars."), "{name}");

    assert_ne!(
        temp_path(Path::new(".env")).unwrap(),
        temp_path(Path::new(".env")).unwrap()
    );
}

#[test]
fn reading_a_missing_file_is_empty_and_a_symlink_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_existing(&dir.path().join(".env")).unwrap().is_empty());

    let link = dir.path().join("link");
    symlink(dir.path().join("elsewhere"), &link).unwrap();
    assert!(matches!(read_existing(&link), Err(WriteError::Symlink(_))));
}
