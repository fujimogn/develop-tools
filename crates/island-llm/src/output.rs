//! 出力形の検査。構文だけを見る（JSON の中身の意味は呼び出し側が検査する）。

use serde_json::Value;

use crate::Output;
use crate::purpose::Shape;

pub fn parse(shape: Shape, text: &str) -> Option<Output> {
    match shape {
        Shape::Line => (!text.contains(['\n', '\r'])).then(|| Output::Line(text.to_owned())),
        // `from_str` は値の後ろに空白以外があれば失敗するので、2 個目の値も弾ける。
        Shape::Json => serde_json::from_str::<Value>(text).ok().map(Output::Json),
    }
}
