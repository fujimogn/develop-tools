//! 複数 target の全取得と順次書込。取得失敗では一件も書かない。

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::Secret;
use crate::declaration::{Declaration, Target};
use crate::op::OpClient;
use crate::{dotenv, preflight, resolve, write};

pub enum Error {
    Fetch(String),
    Io(String),
}

pub fn run(
    declaration: &Declaration,
    client: &dyn OpClient,
    check: bool,
    concurrency: usize,
) -> Result<(), Error> {
    for target in &declaration.targets {
        write::ensure_writable_target(&target.output)
            .map_err(|error| Error::Io(error.to_string()))?;
    }
    let references: BTreeSet<&str> = declaration
        .targets
        .iter()
        .flat_map(|target| target.entries.values())
        .flat_map(|entry| entry.references.iter().map(String::as_str))
        .collect();
    let vaults: BTreeSet<&str> = references
        .iter()
        .filter_map(|reference| {
            reference
                .strip_prefix("op://")
                .and_then(|rest| rest.split('/').next())
        })
        .collect();
    preflight::check(true, client, &vaults.into_iter().collect::<Vec<_>>())
        .map_err(|error| Error::Fetch(error.to_string()))?;
    let resolved = resolve::resolve(
        client,
        &references.into_iter().collect::<Vec<_>>(),
        &resolve::Options {
            concurrency,
            ..resolve::Options::default()
        },
    );
    let mut failed = Vec::new();
    let mut optional = Vec::new();
    for target in &declaration.targets {
        for (key, entry) in &target.entries {
            if entry
                .references
                .iter()
                .any(|reference| resolved.failures.contains_key(reference))
            {
                if target.optional.contains(key) {
                    optional.push(format!("{}:{key}", target.name));
                } else {
                    failed.push(format!("{}:{key}", target.name));
                }
            }
        }
    }
    if !failed.is_empty() {
        return Err(Error::Fetch(format!(
            "必須 key を取得できませんでした: {}。どの target にも書いていません",
            failed.join(", ")
        )));
    }
    for key in optional {
        eprintln!("develop-secrets: optional key {key} を省き、既存値を残します");
    }
    if check {
        println!("develop-secrets: 全 target の必須 key を取得できます");
        return Ok(());
    }

    // 全 target の merge も書込前に済ませる。既存値を含むので Secret のまま保持する。
    let mut prepared = Vec::new();
    for target in &declaration.targets {
        let values = render(target, &resolved);
        let existing =
            write::read_existing(&target.output).map_err(|error| Error::Io(error.to_string()))?;
        let merged = dotenv::merge(existing.expose(), &values).map_err(|_| {
            Error::Fetch(format!(
                "{} の既存 dotenv が不正です。どの target にも書いていません",
                target.name
            ))
        })?;
        prepared.push(merged);
    }
    let mut written = Vec::new();
    for (index, (target, content)) in declaration.targets.iter().zip(prepared.iter()).enumerate() {
        if target.home_output {
            ensure_private_parent(
                &declaration.home_config,
                target.output.parent().expect("validated output"),
            )
            .map_err(|error| partial(&written, &declaration.targets[index..], error))?;
        }
        if let Err(error) = write::write_secret_file(&target.output, content) {
            let mut completed = written;
            let remaining = if matches!(error, write::WriteError::DurabilityUnconfirmed(..)) {
                completed.push(target.name.clone());
                &declaration.targets[index + 1..]
            } else {
                &declaration.targets[index..]
            };
            return Err(partial(&completed, remaining, error.to_string()));
        }
        written.push(target.name.clone());
        println!("develop-secrets: {} を書きました（0600）", target.name);
    }
    Ok(())
}

fn render(target: &Target, resolved: &resolve::Resolved) -> Vec<(String, Secret)> {
    target
        .entries
        .iter()
        .filter_map(|(key, entry)| {
            if entry
                .references
                .iter()
                .any(|reference| resolved.failures.contains_key(reference))
            {
                return None;
            }
            let mut value = String::new();
            let mut rest = entry.template.as_str();
            for reference in &entry.references {
                let start = rest.find("{{").expect("validated template reference");
                let after = &rest[start + 2..];
                let end = after.find("}}").expect("validated template reference");
                value.push_str(&rest[..start]);
                value.push_str(resolved.values[reference].expose());
                rest = &after[end + 2..];
            }
            value.push_str(rest);
            Some((key.clone(), Secret::new(value)))
        })
        .collect()
}

fn partial(written: &[String], remaining: &[Target], error: String) -> Error {
    Error::Io(format!(
        "書込済み: [{}]、未書込: [{}]。{}",
        written.join(", "),
        remaining
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        error
    ))
}

fn ensure_private_parent(root: &Path, parent: &Path) -> Result<(), String> {
    let config = root.parent().ok_or("config directory がありません")?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "config の出力先が不正です")?;
    match fs::metadata(config) {
        Ok(meta) if !meta.is_dir() => {
            return Err("共有 config が directory ではありません".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(config).map_err(|_| "共有 config を作れません".to_owned())?;
        }
        Err(_) => return Err("共有 config を読めません".to_owned()),
    }
    let mut dirs = vec![root.to_owned()];
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        dirs.push(current.clone());
    }
    for dir in dirs {
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err("config の親 directory が不正です".to_owned());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&dir)
                    .map_err(|_| "config の親 directory を作れません".to_owned())?;
            }
            Err(_) => return Err("config の親 directory を読めません".to_owned()),
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| "config の親 directory を 0700 にできません".to_owned())?;
    }
    Ok(())
}
