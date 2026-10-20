//! `bin/_adr-standard.test.ts` のうち**検査側 18 本**を移設したもの。
//!
//! 残る 7 本は template の plan / apply で、消費者が `sync-adr-template` だけなので
//! `develop-sync` の担当。1 ファイルのテストを消費者で割る形は、この移設で初めて出た。

use super::*;
use support::*;

mod archive;
mod confirmation;
mod index;
mod numbering;
mod staged;
mod structure;
mod support;

fn body_read_fixture() -> tempfile::TempDir {
    let root = tempfile::Builder::new()
        .prefix("adr-body-")
        .tempdir()
        .expect("temp");
    let adr_dir = root.path().join("docs/adr");
    fs::create_dir_all(&adr_dir).expect("adr dir");
    fs::write(adr_dir.join("adr-template.md"), TEMPLATE).expect("template");
    fs::write(
        adr_dir.join("README.md"),
        "0001-unreadable.md\n0002-valid.md\n",
    )
    .expect("readme");
    fs::write(adr_dir.join("0002-valid.md"), filled_adr("0002", "valid")).expect("valid adr");
    root
}

fn inspect_body_fixture(root: &std::path::Path) -> Vec<Issue> {
    inspect(
        &Options {
            project_root: root,
            canonical_template: TEMPLATE,
            migrated_through: None,
            reserved_numbers: &[],
            sanitized_numbers: &[],
        },
        &validators(),
    )
}

#[test]
fn directory_adr_body_reports_read_error_with_path_and_cause() {
    let root = body_read_fixture();
    fs::create_dir(root.path().join("docs/adr/0001-unreadable.md"))
        .expect("directory named as ADR");

    let issues = inspect_body_fixture(root.path());
    println!("directory fixture: {issues:?}");
    assert_eq!(codes(&issues), vec!["adr-body-read-error"]);
    assert!(issues[0].message.contains("0001-unreadable.md"));
    assert!(issues[0].message.contains("directory"));
}

#[test]
fn non_utf8_adr_body_reports_read_error_with_path_and_cause() {
    let root = body_read_fixture();
    fs::write(root.path().join("docs/adr/0001-unreadable.md"), [0xff]).expect("non UTF-8 ADR");

    let issues = inspect_body_fixture(root.path());
    println!("non UTF-8 fixture: {issues:?}");
    assert_eq!(codes(&issues), vec!["adr-body-read-error"]);
    assert!(issues[0].message.contains("0001-unreadable.md"));
    assert!(issues[0].message.contains("UTF-8"));
}

#[cfg(unix)]
#[test]
fn unreadable_adr_directory_reports_enumeration_issue() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(&root, "docs/adr/README.md", "# ADR\n");
    write(&root, "docs/adr/0001-bad.md", "# incomplete\n");
    let adr_dir = staged_opts(&root).project_root.join("docs/adr");
    fs::set_permissions(&adr_dir, fs::Permissions::from_mode(0o111)).expect("chmod");
    let enumeration_error = fs::read_dir(&adr_dir).expect_err("fixture must deny enumeration");
    let issues = inspect_root(&root, None, &[], &[]);
    fs::set_permissions(&adr_dir, fs::Permissions::from_mode(0o755)).expect("restore chmod");

    assert_eq!(
        enumeration_error.kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(
        codes(&issues).contains(&"adr-directory-read-error"),
        "{issues:?}"
    );
}

#[test]
fn missing_adr_directory_keeps_existing_template_result() {
    let root = temp_root();
    fs::remove_dir(staged_opts(&root).project_root.join("docs/adr"))
        .expect("remove empty fixture directory");

    let issues = inspect_root(&root, None, &[], &[]);
    assert_eq!(codes(&issues), vec!["missing-adr-template"]);
}
