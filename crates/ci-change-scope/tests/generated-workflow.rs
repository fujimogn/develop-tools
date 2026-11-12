use std::{fs, path::Path, process::Command};

const PUBLISHED: &str = include_str!("golden/published-record-only-skip.yml");

fn git(root: &Path, args: &[&str]) {
    let output = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn check_workflow(workflow: &str) -> std::process::Output {
    let repo = tempfile::Builder::new()
        .prefix("develop-meta-ci-scope-")
        .tempdir()
        .unwrap();
    let root = repo.path();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::create_dir_all(root.join(".config")).unwrap();
    fs::write(root.join(".github/workflows/ci-develop.yml"), workflow).unwrap();
    fs::write(
        root.join(".config/develop-tools.json"),
        r#"{"ci":{"recordOnlySkip":true,"rust":{}}}"#,
    )
    .unwrap();
    fs::write(root.join("coverage.json"), "[]").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);

    Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .current_dir(root)
        .args(["--check-dependencies", "coverage.json"])
        .output()
        .unwrap()
}

#[test]
fn published_workflow_reaches_the_next_unproven_rust_step() {
    let output = check_workflow(PUBLISHED);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("unresolved step setting: .github/workflows/ci-develop.yml:rust:2"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_rust_checkout_rejects_changed_inputs_and_action() {
    let (before_rust, rust_job) = PUBLISHED.split_once("  rust:\n").unwrap();
    let checkout = "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist-credentials: false";
    assert!(rust_job.contains(checkout));
    for changed in [
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          fetch-depth: 0\n          persist-credentials: false",
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist-credentials: false\n        run: echo bypass",
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist-credentials: true",
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist_credentials: false",
        "      - uses: actions/checkout@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa # v7.0.1\n        with:\n          persist-credentials: false",
        "      - uses: oven-sh/setup-bun@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist-credentials: false",
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1",
    ] {
        let workflow = format!(
            "{before_rust}  rust:\n{}",
            rust_job.replacen(checkout, changed, 1)
        );
        let output = check_workflow(&workflow);
        assert_eq!(output.status.code(), Some(1), "{changed}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(".github/workflows/ci-develop.yml:rust:0"),
            "{changed}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
