//! 島共通の secret 注入の核（`develop-secrets`）。
//!
//! 契約は `openspec/changes/island-rust-tool-release/`（design §6、spec
//! `island-secret-injection-core`）。旧実装 8 本の食い違いは調査記録
//! `docs/reviews/2026-09-30-island-common-tool-candidates.md` §3。平らな refs の旧 mode と、
//! repo ごとの宣言から template・複数 target・optional を処理する mode を持つ。
//!
//! - [`secret`] — 値を表示しない型（`infra` と共有）
//! - [`op`] — `op` の起動と失敗の固定カテゴリ（未認証は exit code で判定）
//! - [`resolve`] — 直列 1 件目・並列・読み直し・single-flight
//! - [`preflight`] — service account の token と vault の事前確認
//! - [`refs`] — 入力（key → op 参照）
//! - [`declaration`] — repo ごとの JSONC 宣言
//! - [`multi_write`] — 全取得後の複数 target 書込
//! - [`dotenv`] — `.env` の quote・parser・merge（Bun の dotenv と往復する）
//! - [`write`] — 0600 の atomic writer

pub mod declaration;
pub mod dotenv;
pub mod multi_write;
pub mod op;
pub mod preflight;
pub mod refs;
pub mod resolve;
pub mod secret;
pub mod write;

pub use secret::Secret;
