//! staged された TODO.md / CHANGELOG.md / ADR の pre-commit 検査。
//!
//! 判定は `docs_standard`・`adr_standard`・`staged_adr` に置き、`check-staged-docs`・
//! `check-staged-adr`・`archive-changelog` が入口になる。develop-health の guard と
//! `check-project-integrity` もここから判定を引く（逆向きの依存は持たない）。

pub mod adr_standard;
pub mod docs_standard;
pub mod staged_adr;
