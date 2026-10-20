//! secret file を 0600 で原子的に置き換える。
//!
//! 1. 出力先が symlink なら拒否する（追従すると、link の先の任意の file を書き換える）
//! 2. 同じ directory に一時 file を `O_EXCL | O_NOFOLLOW`、最初から 0600 で作る
//! 3. 書いて fsync する
//! 4. **rename の直前にもう一度** 出力先が symlink でないかを見る（1 と 4 の間に置かれうる）
//! 5. rename し、directory を fsync する（電源断で rename が消えないように）
//!
//! rename 前の失敗なら一時 file を消し、出力先は元のまま残す。rename 後の directory fsync
//! 失敗では出力先は新しい内容のまま旧 file へ戻さず、永続化は未確認として専用エラーを返す。
//! 一部の旧実装には 3 の fsync と 4 の再検査が無かった（調査記録 §3.3-7）。

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::secret::Secret;

pub const SECRET_FILE_MODE: u32 = 0o600;
/// 一時 file の接尾辞。子 repo の `.gitignore` は `.env.*.secret-tmp` / `.*.secret-tmp` で外している。
pub const TEMP_SUFFIX: &str = ".secret-tmp";

#[derive(Debug)]
pub enum WriteError {
    Symlink(PathBuf),
    NotRegularFile(PathBuf),
    Io(PathBuf, io::Error),
    DurabilityUnconfirmed(PathBuf, io::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Symlink(path) => write!(
                f,
                "{} は symlink です。secret file は link の先へ書きません",
                path.display()
            ),
            Self::NotRegularFile(path) => {
                write!(f, "{} は通常の file ではありません", path.display())
            }
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::DurabilityUnconfirmed(path, error) => {
                write!(
                    f,
                    "{}: directory fsync に失敗しました: {error}",
                    path.display()
                )
            }
        }
    }
}

/// 出力先が symlink でも通常 file 以外でもないこと（無いのは可）。**読み始める前にも呼ぶ。**
pub fn ensure_writable_target(target: &Path) -> Result<(), WriteError> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => Err(WriteError::Symlink(target.to_owned())),
        Ok(meta) if !meta.is_file() => Err(WriteError::NotRegularFile(target.to_owned())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WriteError::Io(target.to_owned(), error)),
    }
}

/// 既存の内容を読む（無ければ空）。symlink は辿らない。**内容は secret を含みうる。**
pub fn read_existing(target: &Path) -> Result<Secret, WriteError> {
    ensure_writable_target(target)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(target);
    let mut file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Secret::new("")),
        Err(error) => return Err(WriteError::Io(target.to_owned(), error)),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| WriteError::Io(target.to_owned(), error))?;
    Ok(Secret::new(text))
}

/// `content` で `target` を置き換える。
pub fn write_secret_file(target: &Path, content: &Secret) -> Result<(), WriteError> {
    write_with(target, content, || {}, sync_parent)
}

/// `before_rename` は rename の直前に呼ぶ（テストがここで symlink を差し込む）。
/// `sync_directory` は rename 後だけ呼ぶ（テストが directory fsync 失敗を差し込む）。
fn write_with(
    target: &Path,
    content: &Secret,
    before_rename: impl FnOnce(),
    sync_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), WriteError> {
    ensure_writable_target(target)?;
    let temp = temp_path(target).map_err(|error| WriteError::Io(target.to_owned(), error))?;
    let result = stage(&temp, content).and_then(|()| {
        before_rename();
        ensure_writable_target(target)?;
        fs::rename(&temp, target).map_err(|error| WriteError::Io(target.to_owned(), error))?;
        sync_directory(target)
            .map_err(|error| WriteError::DurabilityUnconfirmed(target.to_owned(), error))
    });
    if result.is_err() {
        // rename 前なら一時 file を消す。rename 後なら一時 file はもう無く、新しい出力先には触れない。
        let _ = fs::remove_file(&temp);
    }
    result
}

fn stage(temp: &Path, content: &Secret) -> Result<(), WriteError> {
    let io_error = |error| WriteError::Io(temp.to_owned(), error);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(SECRET_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(temp)
        .map_err(io_error)?;
    // 作成時の mode は umask で狭まることはあっても広がらないが、明示して揃える。
    file.set_permissions(fs::Permissions::from_mode(SECRET_FILE_MODE))
        .map_err(io_error)?;
    file.write_all(content.expose().as_bytes())
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn sync_parent(target: &Path) -> io::Result<()> {
    let parent = parent_dir(target);
    File::open(parent).and_then(|dir| dir.sync_all())
}

fn parent_dir(target: &Path) -> &Path {
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// `<dir>/.<name から先頭の . を除いたもの>.<乱数>.secret-tmp`。`.env` なら `.env.<乱数>.secret-tmp`。
///
/// 旧実装は `..env.<uuid>.secret-tmp` と
/// `.env.<uuid>.secret-tmp` に割れていて、前者は
/// `.env.*.secret-tmp` だけを外す repo の `.gitignore` に掛からない。後者に揃える。
pub fn temp_path(target: &Path) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "出力先に file 名がありません"))?
        .to_string_lossy();
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let random: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let stem = name.strip_prefix('.').unwrap_or(&name);
    Ok(parent_dir(target).join(format!(".{stem}.{random}{TEMP_SUFFIX}")))
}

#[cfg(test)]
mod tests;
