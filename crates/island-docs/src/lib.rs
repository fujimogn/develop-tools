//! 島共通の docs 検査（`develop-docs-check`）。
//!
//! 契約は `openspec/changes/island-rust-tool-release/`（design §5、spec `island-docs-check`）。
//! 土台は移設元の `check-docs`（link / 容量上限）で、規則は 7 実装の比較（調査記録
//! `docs/reviews/2026-09-30-island-common-tool-candidates.md` §1.4）から選んだ基準に揃えた。
//!
//! - [`constants`] — 島の定数（「repo × path → 上限」の表など）。上書き口は無い
//! - [`markdown`] — fence・code span・link 抽出・見出し slug
//! - [`links`] — 宛先の解決と存在確認（tracked 基準）
//! - [`git`] — index と commit tree からの読み取り
//! - [`check`] — `--tracked` / `--staged` / `--range` の組み立て
//! - [`rules`] — link と容量上限の外にある検査項目の結果の形。項目は [`docs`]（入口・配置・H1・
//!   索引）、[`adr`]、[`openspec`]、[`records`]（TODO / CHANGELOG）、[`emoji`]、[`retired`]（退役語）

pub mod adr;
pub mod check;
pub mod constants;
pub mod docs;
pub mod emoji;
pub mod git;
pub mod links;
pub mod markdown;
pub mod openspec;
pub mod records;
pub mod retired;
pub mod rules;
