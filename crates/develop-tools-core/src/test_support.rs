//! cargo の JSON から成果物の path を引いて、exec できる写しを置く。
//!
//! feature `test-support` のときだけ公開する。directory 名から profile は決めない。
//! cargo 1.100 の build-dir は安定した interface ではない。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `cargo build` に渡す profile。
///
/// `cargo test` の既定は test profile で、最終成果物の directory は dev と同じなので、
/// debug assertion があるときは profile を付けない（dev）。`CARGO_PROFILE` は
/// release や独自 profile を明示するときだけ読む。`debug` / `test` / `dev` は dev に揃える。
pub fn profile_args() -> Vec<String> {
    let from_env = std::env::var("CARGO_PROFILE").ok();
    let profile = match from_env.as_deref() {
        Some("debug" | "test" | "dev") => None,
        Some(name) => Some(name.to_string()),
        None if cfg!(debug_assertions) => None,
        None => Some("release".to_string()),
    };
    profile
        .map(|name| vec!["--profile".to_string(), name])
        .unwrap_or_default()
}

/// `manifest_dir` で `cargo_args` を実行し、`accept` が通した executable を `dest_dir/<name>` へ置く。
///
/// target dir と target triple は起動した env（`CARGO_TARGET_DIR`・`CARGO_BUILD_TARGET`）を継ぐ。
/// profile は [`profile_args`] から付ける。executable の path は cargo の JSON だけを読む。
pub fn build_placed_executables(
    manifest_dir: &Path,
    dest_dir: &Path,
    cargo_args: &[&str],
    accept: impl Fn(&str) -> bool,
) -> BTreeMap<String, PathBuf> {
    fs::create_dir_all(dest_dir).expect("成果物の置き場");
    // 手放しは file を閉じたとき（この関数の終わりか process の終了）。
    let lock = File::create(dest_dir.join(".lock")).expect("成果物の lock");
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) },
        0,
        "成果物の lock を取れない: {}",
        std::io::Error::last_os_error()
    );
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(cargo_args)
        .args(profile_args())
        .arg("--message-format=json-render-diagnostics")
        .current_dir(manifest_dir)
        .stderr(Stdio::inherit());
    let output = command.output().expect("cargo を起動できない");
    assert!(
        output.status.success(),
        "cargo build が失敗した: {}",
        output.status
    );
    let mut placed = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let Some(name) = message["target"]["name"].as_str() else {
            continue;
        };
        if !accept(name) {
            continue;
        }
        let Some(executable) = message["executable"].as_str() else {
            continue;
        };
        placed.insert(
            name.to_string(),
            place(dest_dir, name, Path::new(executable)),
        );
    }
    placed
}

/// `executable` を `dir/name` へ置き換えて、その path を返す。exec 中の写しは rename で外れるだけで消えない。
pub fn place(dir: &Path, name: &str, executable: &Path) -> PathBuf {
    let staging = dir.join(format!(".{name}.{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    // 書き込み用に開いた file を exec すると Linux は ETXTBSY を返すので、cargo に合わせて macOS 以外は hard link。
    if cfg!(target_os = "macos") {
        fs::copy(executable, &staging).map(drop)
    } else {
        fs::hard_link(executable, &staging)
    }
    .unwrap_or_else(|error| panic!("{} を取り出せない: {error}", executable.display()));
    let placed = dir.join(name);
    fs::rename(&staging, &placed).expect("成果物を置き換えられない");
    // 同じ inode 同士の rename は何もしないので、fresh な Linux build では staging が残る。
    let _ = fs::remove_file(&staging);
    placed
}
