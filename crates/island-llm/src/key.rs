//! 鍵の解決。service account の token を島共通の規則で決め、`op read` で鍵を読む。
//! 素の `op`（利用者の画面に承認 dialog を出す）には落ちない。

use std::process::Command;

use island_secrets::Secret;
use island_secrets::preflight::{self, Token};

use crate::{KeyRef, Runtime};

pub fn resolve(key_ref: &KeyRef, runtime: &Runtime<'_>) -> Option<Secret> {
    let env_token = runtime.op_service_account_token.as_deref();
    let token = match preflight::locate_token(env_token, Some(&runtime.home)).ok()? {
        // `Runtime` が注入点なので、env の token も継承に任せず明示して渡す。
        Token::FromEnv => Secret::new(env_token?),
        Token::FromFile(secret) => secret,
    };
    let mut command = Command::new(&runtime.op_program);
    command.args(["read", key_ref.as_str()]);
    preflight::pass_to_child(&mut command, Some(&token));
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let key = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!key.is_empty()).then(|| Secret::new(key))
}
