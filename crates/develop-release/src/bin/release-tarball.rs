//! `release-tarball <bin_dir> <archive>` — Release asset の tarball を作る。
//!
//! 入れる binary の一覧は公開側と非公開側の設定から合成する。
//! release job が一覧を YAML へ複写しないための入口。

use develop_release::{create_release_tarball, load_release_binaries};
use develop_tools_core::cli_help::Help;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        let help = Help::new(
            "release-tarball <bin_dir> <archive>",
            "release-tarball — 配布対象 binary だけを tar.gz にまとめます。",
            "公開 CLI と非公開の binary を archive へ書きます。1 つでも欠けると失敗します。--list で同じ一覧を出力します。",
            "archive を作成します。bin_dir は読み取りだけです。",
        )
        .exit("0 = 作成、1 = 引数不正・binary 欠落・tar 失敗")
        .examples(&["release-tarball tools/target/release develop-tools-linux-x86_64.tar.gz"]);
        print!("{}", help.render());
        return;
    }
    if args.as_slice() == ["--list"] {
        match load_release_binaries(Path::new(".")) {
            Ok(binaries) => {
                for binary in binaries {
                    println!("{binary}");
                }
            }
            Err(err) => {
                eprintln!("release-tarball: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    let [bin_dir, archive] = args.as_slice() else {
        eprintln!("使い方: release-tarball <bin_dir> <archive>");
        std::process::exit(1);
    };
    if let Err(err) = create_release_tarball(Path::new("."), Path::new(bin_dir), Path::new(archive))
    {
        eprintln!("release-tarball: {err}");
        std::process::exit(1);
    }
}
