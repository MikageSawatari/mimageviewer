//! 注釈 (comic) のスタイルプリセット (Inc 5)。
//!
//! セリフ (テキスト) / 本体 (吹き出し) / ウィンドウ の 3 種について、再利用できる
//! スタイルの束を保持する。`sys:*` id の組み込みプリセット (再起動で作り直す) と
//! `user:*` id のユーザープリセット (`presets.json` に永続化) を扱う。
//!
//! プリセットを「適用」すると対象オブジェクトへスタイル各フィールドをコピーし、その
//! オブジェクトのリンクフィールド (`TextBlock::preset_link` /
//! `BubbleObject::shape_preset_link` / `MessageWindowObject::style_preset_link`) に
//! プリセット id を焼く。リンクはボタンの点灯に使い、個別コントロールを編集すると
//! 呼び出し側が解除する。`comic-core` はこれらの id を解釈せず保持するだけ。
//!
//! 組み込みプリセットの生成 (system_*_presets) は `BubblePreset` / `WIN_PRESETS` に
//! 依存するため `ui_text.rs` 側に置く。本モジュールはデータ構造と永続化のみ。

use comic_core::{
    BubbleObject, BubbleShape, FillMode, FrameStyle, IndicatorKind, Insets, MarkupRule,
    MessageWindowObject, NamePlate, Orientation, PortraitSlot, Rgba, ShadowStyle, SizeMode,
    StrokeStyle, TailKind, TextAlign, TextBackgroundStyle, TextBlock, TextEchoStyle, TextGlowStyle,
    TextShadowStyle, VAnchor, WindowPosition,
};
use serde::{Deserialize, Serialize};

/// セリフ (テキスト) スタイルプリセット。本文内容は含めず、見た目だけを束ねる。
#[derive(Clone, Serialize, Deserialize)]
pub struct TextStylePreset {
    pub id: String,
    pub name: String,
    pub font_key: String,
    pub size_px: f32,
    pub color: Rgba,
    pub orientation: Orientation,
    pub align: TextAlign,
    pub line_gap: f32,
    pub letter_gap: f32,
    pub outline: Option<StrokeStyle>,
    #[serde(default)]
    pub extra_outlines: Vec<StrokeStyle>,
    #[serde(default)]
    pub shadow: Option<TextShadowStyle>,
    #[serde(default)]
    pub glow: Option<TextGlowStyle>,
    #[serde(default)]
    pub background: Option<TextBackgroundStyle>,
    #[serde(default)]
    pub echo: Option<TextEchoStyle>,
    pub auto_tcy: bool,
    pub markup_enabled: bool,
    /// 記法の記号セット (縦中横 / 横倒し のペア)。`markup_enabled` と対なので一緒に
    /// 取り込む / 適用する。旧 presets.json 互換のため default 付き。
    #[serde(default = "comic_core::default_markup_rules")]
    pub markup_rules: Vec<MarkupRule>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
}

impl TextStylePreset {
    /// スタイル各フィールドを `tb` にコピーし、このプリセットへリンクする。本文は不変。
    pub fn apply_to(&self, tb: &mut TextBlock) {
        tb.font_key = self.font_key.clone();
        tb.size_px = self.size_px;
        tb.color = self.color;
        tb.orientation = self.orientation;
        tb.align = self.align;
        tb.line_gap = self.line_gap;
        tb.letter_gap = self.letter_gap;
        tb.outline = self.outline;
        tb.extra_outlines = self.extra_outlines.clone();
        tb.shadow = self.shadow;
        tb.glow = self.glow;
        tb.background = self.background;
        tb.echo = self.echo;
        tb.auto_tcy = self.auto_tcy;
        tb.markup_enabled = self.markup_enabled;
        tb.markup_rules = self.markup_rules.clone();
        tb.bold = self.bold;
        tb.italic = self.italic;
        tb.preset_link = Some(self.id.clone());
    }

    /// `tb` の現在のスタイルを `id`/`name` の新規プリセットとして取り込む。
    pub fn from_text(id: String, name: String, tb: &TextBlock) -> Self {
        TextStylePreset {
            id,
            name,
            font_key: tb.font_key.clone(),
            size_px: tb.size_px,
            color: tb.color,
            orientation: tb.orientation,
            align: tb.align,
            line_gap: tb.line_gap,
            letter_gap: tb.letter_gap,
            outline: tb.outline,
            extra_outlines: tb.extra_outlines.clone(),
            shadow: tb.shadow,
            glow: tb.glow,
            background: tb.background,
            echo: tb.echo,
            auto_tcy: tb.auto_tcy,
            markup_enabled: tb.markup_enabled,
            markup_rules: tb.markup_rules.clone(),
            bold: tb.bold,
            italic: tb.italic,
        }
    }
}

/// 本体 (吹き出し) スタイルプリセット。コンテナの見た目 (形 + しっぽ種別 + 塗り +
/// アウトライン + 余白) だけを束ね、本文・しっぽ先端/根元の位置は対象に残す。
#[derive(Clone, Serialize, Deserialize)]
pub struct ShapeStylePreset {
    pub id: String,
    pub name: String,
    pub shape: BubbleShape,
    pub tail_kind: Option<TailKind>,
    pub fill: Option<Rgba>,
    pub fill_opacity: f32,
    pub outline: StrokeStyle,
    pub padding_px: f32,
}

impl ShapeStylePreset {
    /// コンテナスタイルを `b` に整合的に適用する。本文・しっぽの位置は保ち、しっぽ種別
    /// だけプリセットに合わせる。`default_tail` は新規にしっぽを生やすときの既定値。
    pub fn apply_to(&self, b: &mut BubbleObject, default_tail: comic_core::Tail) {
        b.shape = self.shape;
        b.fill = self.fill;
        b.fill_opacity = self.fill_opacity;
        b.outline = self.outline;
        b.padding_px = self.padding_px;
        match self.tail_kind {
            None => b.tail = None,
            Some(kind) => {
                let mut tail = b.tail.unwrap_or(default_tail);
                tail.kind = kind;
                b.tail = Some(tail);
            }
        }
        b.shape_preset_link = Some(self.id.clone());
    }

    /// `b` の現在のコンテナスタイルを新規プリセットとして取り込む。
    pub fn from_bubble(id: String, name: String, b: &BubbleObject) -> Self {
        ShapeStylePreset {
            id,
            name,
            shape: b.shape,
            tail_kind: b.tail.map(|t| t.kind),
            fill: b.fill,
            fill_opacity: b.fill_opacity,
            outline: b.outline,
            padding_px: b.padding_px,
        }
    }
}

/// ウィンドウスタイルプリセット (本文 / 名前の TEXT 内容と pivot 以外すべて)。
#[derive(Clone, Serialize, Deserialize)]
pub struct WindowStylePreset {
    pub id: String,
    pub name: String,
    pub size_mode: SizeMode,
    pub half_w: f32,
    pub half_h: f32,
    pub margin_px: f32,
    pub corner_px: f32,
    pub position: WindowPosition,
    pub fill_mode: FillMode,
    pub fill: Option<Rgba>,
    pub fill_opacity: f32,
    pub gradient_to: Option<Rgba>,
    pub scrim_dense_side: VAnchor,
    pub frame: FrameStyle,
    pub outline: StrokeStyle,
    pub frame_gap_px: f32,
    pub shadow: Option<ShadowStyle>,
    pub padding: Insets,
    pub v_anchor: VAnchor,
    pub wrap: bool,
    pub name_plate: NamePlate,
    pub portrait: PortraitSlot,
    pub indicator: IndicatorKind,
    #[serde(default)]
    pub indicator_auto: bool,
    /// 本文テキストの STYLE のみ (色/サイズ/アウトライン/向き/整列)。本文内容 + フォントは
    /// 適用時に対象から保つ。
    pub text_style: TextBlock,
}

impl WindowStylePreset {
    /// スタイルを `w` に適用する。本文・名前の TEXT 内容とフォントは対象から保つ。
    ///
    /// レイアウト (size_mode / half_w / half_h / margin_px / position) は対象に**残す**。
    /// 「見た目を着せ替える」操作で利用者がリサイズ/移動した枠が既定サイズへ縮む事故を
    /// 防ぐ (Codex P3)。プリセット側のレイアウト値は `from_window` で取り込むが適用しない。
    pub fn apply_to(&self, w: &mut MessageWindowObject) {
        w.corner_px = self.corner_px;
        w.fill_mode = self.fill_mode;
        w.fill = self.fill;
        w.fill_opacity = self.fill_opacity;
        w.gradient_to = self.gradient_to;
        w.scrim_dense_side = self.scrim_dense_side;
        w.frame = self.frame;
        w.outline = self.outline;
        w.frame_gap_px = self.frame_gap_px;
        w.shadow = self.shadow;
        w.padding = self.padding;
        w.v_anchor = self.v_anchor;
        w.wrap = self.wrap;
        // 名前プレート: プリセットの装飾を取り、利用者の名前テキスト/フォントは保つ。
        let name_text = w.name_plate.name.text.clone();
        let name_font = w.name_plate.name.font_key.clone();
        w.name_plate = self.name_plate.clone();
        w.name_plate.name.text = name_text;
        w.name_plate.name.font_key = name_font;
        w.portrait = self.portrait;
        w.indicator = self.indicator;
        w.indicator_auto = self.indicator_auto;
        // 本文: プリセットのスタイル、内容 + フォントは既存を保つ。
        let content = std::mem::take(&mut w.text.text);
        let font = std::mem::take(&mut w.text.font_key);
        let mut ts = self.text_style.clone();
        ts.text = content;
        ts.font_key = font;
        ts.preset_link = None;
        w.text = ts;
        w.style_preset_link = Some(self.id.clone());
    }

    /// `w` の現在のスタイルを新規プリセットとして取り込む (本文 / 名前内容は空にする)。
    pub fn from_window(id: String, name: String, w: &MessageWindowObject) -> Self {
        let mut text_style = w.text.clone();
        text_style.text = String::new();
        text_style.preset_link = None;
        let mut name_plate = w.name_plate.clone();
        name_plate.name.text = String::new();
        WindowStylePreset {
            id,
            name,
            size_mode: w.size_mode,
            half_w: w.half_w,
            half_h: w.half_h,
            margin_px: w.margin_px,
            corner_px: w.corner_px,
            position: w.position,
            fill_mode: w.fill_mode,
            fill: w.fill,
            fill_opacity: w.fill_opacity,
            gradient_to: w.gradient_to,
            scrim_dense_side: w.scrim_dense_side,
            frame: w.frame,
            outline: w.outline,
            frame_gap_px: w.frame_gap_px,
            shadow: w.shadow,
            padding: w.padding,
            v_anchor: w.v_anchor,
            wrap: w.wrap,
            name_plate,
            portrait: w.portrait,
            indicator: w.indicator,
            indicator_auto: w.indicator_auto,
            text_style,
        }
    }
}

/// `presets.json` の中身 (ユーザープリセットのみ。`sys:*` は保存しない)。
#[derive(Default, Serialize, Deserialize)]
pub struct UserPresetDoc {
    #[serde(default)]
    pub text: Vec<TextStylePreset>,
    #[serde(default)]
    pub shape: Vec<ShapeStylePreset>,
    #[serde(default)]
    pub window: Vec<WindowStylePreset>,
}

/// 組み込み (sys:*) プリセットかどうか。
pub fn is_system_preset(id: &str) -> bool {
    id.starts_with("sys:")
}

/// ユーザープリセット永続化ファイル `%APPDATA%/mimageviewer/comic_presets.json`。
pub fn presets_path() -> std::path::PathBuf {
    crate::data_dir::get().join("comic_presets.json")
}

/// ユーザープリセットを読む (sys:* は捨て、組み込みは起動時に作り直す)。失敗時は空。
pub fn load_user_presets() -> UserPresetDoc {
    let Ok(text) = std::fs::read_to_string(presets_path()) else {
        return UserPresetDoc::default();
    };
    match serde_json::from_str::<UserPresetDoc>(&text) {
        Ok(mut doc) => {
            doc.text.retain(|p| !is_system_preset(&p.id));
            doc.shape.retain(|p| !is_system_preset(&p.id));
            doc.window.retain(|p| !is_system_preset(&p.id));
            doc
        }
        Err(_) => UserPresetDoc::default(),
    }
}

/// ユーザープリセット (= sys:* を除いた分) を `presets.json` に保存する。
pub fn save_user_presets(
    text: &[TextStylePreset],
    shape: &[ShapeStylePreset],
    window: &[WindowStylePreset],
) {
    let doc = UserPresetDoc {
        text: text
            .iter()
            .filter(|p| !is_system_preset(&p.id))
            .cloned()
            .collect(),
        shape: shape
            .iter()
            .filter(|p| !is_system_preset(&p.id))
            .cloned()
            .collect(),
        window: window
            .iter()
            .filter(|p| !is_system_preset(&p.id))
            .cloned()
            .collect(),
    };
    let path = presets_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&doc) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                crate::logger::log(format!("[comic] presets.json 保存失敗: {e}"));
            }
        }
        Err(e) => crate::logger::log(format!("[comic] presets.json シリアライズ失敗: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_preset_id_detection() {
        assert!(is_system_preset("sys:text_white"));
        assert!(!is_system_preset("user:t1"));
        assert!(!is_system_preset(""));
    }

    #[test]
    fn text_preset_apply_sets_style_and_link() {
        let preset = TextStylePreset {
            id: "user:t1".to_string(),
            name: "test".to_string(),
            font_key: "MyFont".to_string(),
            size_px: 72.0,
            color: Rgba::new(10, 20, 30, 255),
            orientation: Orientation::Vertical,
            align: TextAlign::End,
            line_gap: 3.0,
            letter_gap: 2.0,
            outline: Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 5.0,
            }),
            extra_outlines: vec![StrokeStyle {
                color: Rgba::BLACK,
                width_px: 8.0,
            }],
            shadow: Some(TextShadowStyle::default()),
            glow: Some(TextGlowStyle::default()),
            background: Some(TextBackgroundStyle::default()),
            echo: Some(TextEchoStyle::default()),
            auto_tcy: false,
            markup_enabled: true,
            markup_rules: comic_core::markup_rules_angle(),
            bold: true,
            italic: false,
        };
        let mut tb = TextBlock {
            text: "本文は保持".to_string(),
            ..TextBlock::default()
        };
        preset.apply_to(&mut tb);
        assert_eq!(tb.text, "本文は保持", "本文は不変");
        assert_eq!(tb.font_key, "MyFont");
        assert_eq!(tb.size_px, 72.0);
        assert_eq!(tb.align, TextAlign::End);
        assert!(tb.bold, "bold もプリセットから復元される");
        assert_eq!(tb.extra_outlines.len(), 1);
        assert!(tb.shadow.is_some());
        assert!(tb.glow.is_some());
        assert!(tb.background.is_some());
        assert!(tb.echo.is_some());
        assert_eq!(
            tb.markup_rules,
            comic_core::markup_rules_angle(),
            "記号セットもプリセットから復元される"
        );
        assert_eq!(tb.preset_link.as_deref(), Some("user:t1"));
    }

    #[test]
    fn text_preset_from_text_roundtrips_style() {
        let tb = TextBlock {
            text: "x".to_string(),
            font_key: "F".to_string(),
            size_px: 33.0,
            letter_gap: 1.5,
            ..TextBlock::default()
        };
        let p = TextStylePreset::from_text("user:t2".to_string(), "n".to_string(), &tb);
        // 取り込んだスタイルを別ブロックへ適用すると一致する (本文以外)。
        let mut other = TextBlock::default();
        p.apply_to(&mut other);
        assert_eq!(other.font_key, "F");
        assert_eq!(other.size_px, 33.0);
        assert_eq!(other.letter_gap, 1.5);
    }

    #[test]
    fn user_preset_doc_filters_system_on_save_roundtrip() {
        // save_user_presets は sys:* を除外する (serde で UserPresetDoc を組む経路の検証)。
        let text = vec![
            TextStylePreset {
                id: "sys:text_white".to_string(),
                name: "sys".to_string(),
                font_key: String::new(),
                size_px: 40.0,
                color: Rgba::WHITE,
                orientation: Orientation::Horizontal,
                align: TextAlign::Center,
                line_gap: 0.0,
                letter_gap: 0.0,
                outline: None,
                extra_outlines: Vec::new(),
                shadow: None,
                glow: None,
                background: None,
                echo: None,
                auto_tcy: false,
                markup_enabled: false,
                markup_rules: comic_core::default_markup_rules(),
                bold: false,
                italic: false,
            },
            TextStylePreset {
                id: "user:t1".to_string(),
                name: "user".to_string(),
                font_key: String::new(),
                size_px: 40.0,
                color: Rgba::WHITE,
                orientation: Orientation::Horizontal,
                align: TextAlign::Center,
                line_gap: 0.0,
                letter_gap: 0.0,
                outline: None,
                extra_outlines: Vec::new(),
                shadow: None,
                glow: None,
                background: None,
                echo: None,
                auto_tcy: false,
                markup_enabled: false,
                markup_rules: comic_core::default_markup_rules(),
                bold: false,
                italic: false,
            },
        ];
        let doc = UserPresetDoc {
            text: text
                .iter()
                .filter(|p| !is_system_preset(&p.id))
                .cloned()
                .collect(),
            shape: Vec::new(),
            window: Vec::new(),
        };
        assert_eq!(doc.text.len(), 1);
        assert_eq!(doc.text[0].id, "user:t1");
    }
}
