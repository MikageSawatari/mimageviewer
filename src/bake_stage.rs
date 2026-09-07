//! 焼き込みをどこまで進めるか。
//!
//! 製本 / <kbd>Ctrl+E</kbd> (1 枚・一括) / 外部ツールが**同じ語彙**でこれを言う。
//! 正本は [docs/bake-stage-unification-plan.md](../docs/bake-stage-unification-plan.md)。
//!
//! 段は表示パイプラインの適用順の上に置く。**順序は変えない** — 段は「どこまで」だけを指す。
//!
//! ```text
//! 編集 ────────────→ AI 処理 ──────→ 表示用補正
//! 消しゴム              アップスケール    スマートシャープ
//! ローカル調整          デノイズ          カラー化
//! 隠蔽                                    Creative LUT
//! 回転 / 注釈 / 切り取り                  ポストフィルタ
//! 色調補正
//! ```
//!
//! **個別 ON / OFF にはしない。** `AdjustParams::effective_smart_sharpen` が
//! 「AI 拡大した出力にはシャープを掛けない」という条件を持っており、任意の組み合わせを
//! 許すと、この手の相互作用を組み合わせの数だけ定義することになる。3 値なら 3 通り。

/// どの段まで焼き込むか。
///
/// 値の順序が段の深さそのものなので、`PartialOrd` で「〜以上か」を問える。
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum BakeStage {
    /// 編集まで。消しゴム / ローカル調整 / 隠蔽 / 回転 / 注釈 / 切り取り / 色調補正。
    ///
    /// 製本と外部ツールの既定。**後から加工する前提の出力**で、表示専用の効果を載せない。
    #[default]
    Edits,
    /// AI 処理まで。上に加えて AI アップスケール / デノイズ。
    ///
    /// 「AI 拡大は焼けた状態で手直ししたいが、カラー化は入れたくない」向け。
    Ai,
    /// 表示用補正まで。上に加えてスマートシャープ / カラー化 / Creative LUT / ポストフィルタ。
    ///
    /// <kbd>Ctrl+E</kbd> の既定で、単枚書き出しもこの選択値を読む。
    /// **「画面と同一」ではない** — 段を全部適用したもので、GPU テクスチャ上限の
    /// 影響を受けない分だけ画面より良いことがある。
    DisplayAdjust,
}

impl BakeStage {
    /// AI アップスケール / デノイズを焼くか。
    pub fn includes_ai(self) -> bool {
        self >= Self::Ai
    }

    /// スマートシャープ / カラー化 / Creative LUT / ポストフィルタを焼くか。
    pub fn includes_display_adjust(self) -> bool {
        self >= Self::DisplayAdjust
    }

    /// 設定 UI 用のラベル。
    ///
    /// 「画像補正」ではなく「編集」なのは、この段が回転・消しゴム・注釈を含み補正だけでは
    /// ないため。「AI アップスケール」ではなく「AI 処理」なのは、デノイズも含むため。
    pub fn label(self) -> &'static str {
        match self {
            Self::Edits => "編集",
            Self::Ai => "AI 処理",
            Self::DisplayAdjust => "表示用補正",
        }
    }

    pub const ALL: [Self; 3] = [Self::Edits, Self::Ai, Self::DisplayAdjust];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 段は積み上げ。深い段は浅い段を必ず含む。
    #[test]
    fn a_deeper_stage_includes_everything_a_shallower_one_does() {
        assert!(!BakeStage::Edits.includes_ai());
        assert!(!BakeStage::Edits.includes_display_adjust());

        assert!(BakeStage::Ai.includes_ai());
        assert!(!BakeStage::Ai.includes_display_adjust());

        assert!(BakeStage::DisplayAdjust.includes_ai());
        assert!(BakeStage::DisplayAdjust.includes_display_adjust());
    }

    /// 既定は「編集まで」。製本と外部ツールの現行挙動がこれで、既定を変えると
    /// 出荷済みの見た目が黙って変わる。
    #[test]
    fn the_default_is_the_shallow_stage_that_shipped() {
        assert_eq!(BakeStage::default(), BakeStage::Edits);
    }

    #[test]
    fn single_export_default_remains_display_adjust() {
        assert_eq!(
            crate::settings::Settings::default().bake_stage_export,
            BakeStage::DisplayAdjust
        );
    }

    /// 設定へ保存する値なので、綴りが変わると読めなくなる。意図として固定する。
    #[test]
    fn the_stored_spelling_is_fixed() {
        for (stage, text) in [
            (BakeStage::Edits, "\"Edits\""),
            (BakeStage::Ai, "\"Ai\""),
            (BakeStage::DisplayAdjust, "\"DisplayAdjust\""),
        ] {
            assert_eq!(serde_json::to_string(&stage).unwrap(), text);
            assert_eq!(
                serde_json::from_str::<BakeStage>(text).unwrap(),
                stage,
                "{text} が読めなくなっている"
            );
        }
    }
}
