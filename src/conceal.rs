//! 隠蔽加工 (Concealment) 機能の型定義。
//!
//! 詳細仕様: [docs/conceal-feature-plan.md](../../docs/conceal-feature-plan.md)
//!
//! このモジュールは Phase 1 で導入される **型と既定値だけ** を提供する。
//! 合成アルゴリズム (Mosaic / Fill / Blur) は Phase 3、UI 操作は
//! Phase 2、永続化レイヤは [`crate::conceal_db`] を参照。
//!
//! # 設計の要点
//!
//! - 4 種類の処理タイプ (`ConcealType`): Mosaic / WhiteFill / BlackFill / Blur
//! - 9 種類のツール (`ConcealTool`): 消しゴムと完全統一
//! - パラメータは **グローバル設定** (ページ間共有)。複数の好みを保持したい
//!   ときは `ConcealPreset` 4 スロットを使う。マスク本体は `conceal_db` で
//!   ページごとに保存される
//! - UI ラベルは「処理内容を具体的に書く」ポリシー (CLAUDE.md
//!   「モザイク・成人向け画像処理の表記ポリシー」)。`Opaque` / `Translucent` /
//!   `MaskShape` 等の内部名はそのままユーザーに見せず、本モジュールの
//!   `process_description` ヘルパーで `"マスクを含むタイルを不透明で描画"` の
//!   ような文字列を取得して表示する

use serde::{Deserialize, Serialize};

// ── ConcealType ─────────────────────────────────────────────────────────

/// 隠蔽加工の処理タイプ。
///
/// モード内で `T` キー (修飾なし) を押すと
/// `Mosaic → WhiteFill → BlackFill → Blur → Mosaic …` の順に切り替わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConcealType {
    /// タイル化モザイク。`MosaicBoundary` で境界処理を 3 種類から選ぶ。
    #[default]
    Mosaic,
    /// 白塗り (RGB=#FFFFFF)。`FillEdge` でシャープ/フェード、不透明度 1-100%。
    WhiteFill,
    /// 黒塗り (RGB=#000000)。WhiteFill と同じ境界・不透明度オプション。
    BlackFill,
    /// Gaussian ぼかし。`BlurMode` 3 種 + 境界フェード ON/OFF。
    Blur,
}

impl ConcealType {
    /// `T` キーで次に切り替わるタイプを返す。
    pub fn next(self) -> Self {
        match self {
            ConcealType::Mosaic => ConcealType::WhiteFill,
            ConcealType::WhiteFill => ConcealType::BlackFill,
            ConcealType::BlackFill => ConcealType::Blur,
            ConcealType::Blur => ConcealType::Mosaic,
        }
    }

    /// UI ラジオボタンのラベル (パネル表示用、ユーザー向け文言)。
    pub fn label(self) -> &'static str {
        match self {
            ConcealType::Mosaic => "モザイク",
            ConcealType::WhiteFill => "白塗り",
            ConcealType::BlackFill => "黒塗り",
            ConcealType::Blur => "ぼかし",
        }
    }
}

// ── MosaicBoundary ─────────────────────────────────────────────────────

/// モザイクタイルの境界処理。マスクが部分的にタイルを覆うとき、どう塗るかを決める。
///
/// UI ラベルは [`MosaicBoundary::process_description`] で取得 (評価語ではなく
/// 処理内容を具体的に書くポリシー)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MosaicBoundary {
    /// マスクを含むタイル (= coverage > 0) を、タイル全体を平均色で **不透明** に塗る。
    /// 規約的に推奨されることが多い形式。
    #[default]
    Opaque,
    /// マスクを含むタイルを、マスクの割合 (coverage = 0.0..=1.0) に応じた
    /// 不透明度 (= coverage × 255 の alpha) で元画像と blend する。
    Translucent,
    /// マスクの形に沿って描画。マスク内の各画素を、その画素が属するタイルの
    /// 平均色で塗る (= 画素単位、不透明)。境界はマスクの形そのまま。
    MaskShape,
}

impl MosaicBoundary {
    pub fn process_description(self) -> &'static str {
        match self {
            MosaicBoundary::Opaque => "マスクを含むタイルを不透明で描画",
            MosaicBoundary::Translucent => "マスクを含むタイルをマスクの割合に応じた不透明度で描画",
            MosaicBoundary::MaskShape => {
                "マスクの形に沿って描画 (マスク内の各画素をその画素が属するタイルの平均色で塗る)"
            }
        }
    }
}

// ── FillEdge ────────────────────────────────────────────────────────────

/// 白塗り / 黒塗りの境界処理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FillEdge {
    /// マスクの形をそのままシャープに描画。
    #[default]
    Sharp,
    /// マスク境界の内側にフェードを掛ける (= 境界画素 alpha=0、内側 N px で 255)。
    /// 不透明度との組合せで段階的な減衰となる。
    Feathered,
}

impl FillEdge {
    pub fn process_description(self) -> &'static str {
        match self {
            FillEdge::Sharp => "マスクの形をシャープに描画",
            FillEdge::Feathered => "マスクの形に境界フェードを掛けて描画",
        }
    }
}

// ── BlurMode ────────────────────────────────────────────────────────────

/// ぼかし処理のモード。マスクと Gaussian カーネルの関係を 3 種類から選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlurMode {
    /// マスク通り: 鋭い境界、Gaussian カーネルは元画像 (マスク外含む) から sampling。
    /// 標準的なぼかし (Photoshop 同等)。隣接する別オブジェクトの色も少し混ざる。
    #[default]
    AsMask,
    /// マスク拡張: マスクをぼかし半径ぶん膨張させてから描画。
    /// 元のマスクの外側にもぼかし結果が広がる。オブジェクト輪郭そのものを
    /// 曖昧にしたいときに使う。
    ExtendByRadius,
    /// マスク内のみ: Gaussian カーネルがマスク外画素を参照しない (= 鏡像 / 0 で外挿)。
    /// 隣接する別要素 (例: 顔ぼかし隣の名前タグ) の色が漏れ込まない。
    InsideOnly,
}

impl BlurMode {
    pub fn process_description(self) -> &'static str {
        match self {
            BlurMode::AsMask => "マスク通り (外画素を参照してぼかす)",
            BlurMode::ExtendByRadius => "マスク拡張 (半径ぶん広げて描画)",
            BlurMode::InsideOnly => "マスク内のみ (外画素を参照しない)",
        }
    }
}

// ── TileSizeMode ────────────────────────────────────────────────────────

/// モザイクタイルサイズの指定方式。
///
/// JSON 表現はタグ付き (`{"mode":"long_edge_ratio","value":1.0}` 等)。
/// 既定値は `LongEdgeRatio(1.0)` (= 画像長辺の 1/100、最小 4px)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
pub enum TileSizeMode {
    /// 画像長辺に対する比率。`compute_tile_size` で
    /// `tile = round(long_edge / 100 * multiplier).max(4)` として展開される。
    /// 範囲 0.25..=5.0、UI は 0.25 刻みのスライダー。
    LongEdgeRatio(f32),
    /// 画像サイズによらず固定 px。範囲 4..=200、UI は 1px 刻み。
    FixedPx(u32),
}

impl Default for TileSizeMode {
    fn default() -> Self {
        TileSizeMode::LongEdgeRatio(1.0)
    }
}

/// LongEdgeRatio モードの multiplier の範囲 (UI スライダー両端含む)。
pub const TILE_RATIO_MIN: f32 = 0.25;
pub const TILE_RATIO_MAX: f32 = 5.0;
pub const TILE_RATIO_STEP: f32 = 0.25;

/// FixedPx モードの px 値の範囲。
pub const TILE_FIXED_MIN: u32 = 4;
pub const TILE_FIXED_MAX: u32 = 200;

/// 画像長辺と `TileSizeMode` から実際のタイル px サイズを算出する。
/// どちらのモードでも最小 4px に補正される。
pub fn compute_tile_size(image_long_edge: u32, mode: TileSizeMode) -> u32 {
    match mode {
        TileSizeMode::LongEdgeRatio(multiplier) => {
            let base = ((image_long_edge as f32 / 100.0).round().max(4.0)) as u32;
            ((base as f32 * multiplier).round() as u32).max(TILE_FIXED_MIN)
        }
        TileSizeMode::FixedPx(px) => px.max(TILE_FIXED_MIN),
    }
}

// ── ConcealTool ─────────────────────────────────────────────────────────

/// 隠蔽加工モードのツール種別 (9 種、消しゴムと統一)。
///
/// Phase 2 で実装。Phase 1 では型定義のみ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConcealTool {
    /// 選択 (S): ベクタオブジェクトを選択 → ハンドル編集 (Phase 2 で `vector_edit.rs` 経由)
    Select,
    /// 筆 (B): 円形ブラシで自由塗り、ビットマップに反映
    #[default]
    Brush,
    /// 囲み (L): 多角形を描き内側を塗りつぶす、ビットマップに反映
    Lasso,
    /// 多角形 (P): クリックで頂点を置き内側を塗りつぶす、ビットマップに反映
    Polygon,
    /// 直線 (I): `Shape::Line { kind: Diagonal, .. }` をベクタで作成
    Line,
    /// 縦線 (V): `Shape::Line { kind: Vertical, .. }`
    VertLine,
    /// 横線 (H): `Shape::Line { kind: Horizontal, .. }`
    HorizLine,
    /// 矩形 (R): `Shape::Rect { .. }` をドラッグで作成
    Rect,
    /// 楕円 (O): `Shape::Ellipse { .. }` を内接 bbox ドラッグで作成
    Ellipse,
}

impl ConcealTool {
    pub fn label(self) -> &'static str {
        match self {
            ConcealTool::Select => "選 [S]",
            ConcealTool::Brush => "筆 [B]",
            ConcealTool::Lasso => "囲 [L]",
            ConcealTool::Polygon => "多 [P]",
            ConcealTool::Line => "直 [I]",
            ConcealTool::VertLine => "縦 [V]",
            ConcealTool::HorizLine => "横 [H]",
            ConcealTool::Rect => "矩 [R]",
            ConcealTool::Ellipse => "楕 [O]",
        }
    }
}

// ── ConcealPreset ───────────────────────────────────────────────────────

/// 隠蔽加工パラメータのプリセット (4 スロット、`settings_db` 経由で永続化)。
///
/// マスクスロット (= 形状の保存) とは別軸で、「処理タイプ + 各パラメータ一式」を
/// 保存する。VST3 のスロット設計を踏襲。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConcealPreset {
    /// ユーザー編集可能な表示名 (空なら `"プリセット N"` のデフォルト表示)。
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub conceal_type: ConcealType,
    #[serde(default)]
    pub mosaic_tile_mode: TileSizeMode,
    #[serde(default)]
    pub mosaic_boundary: MosaicBoundary,
    #[serde(default = "default_fill_opacity")]
    pub fill_opacity_percent: u8,
    #[serde(default)]
    pub fill_edge: FillEdge,
    #[serde(default = "default_blur_radius_px")]
    pub blur_radius_px: f32,
    #[serde(default)]
    pub blur_mode: BlurMode,
    #[serde(default)]
    pub blur_feather: bool,
}

impl Default for ConcealPreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            conceal_type: ConcealType::default(),
            mosaic_tile_mode: TileSizeMode::default(),
            mosaic_boundary: MosaicBoundary::default(),
            fill_opacity_percent: default_fill_opacity(),
            fill_edge: FillEdge::default(),
            blur_radius_px: default_blur_radius_px(),
            blur_mode: BlurMode::default(),
            blur_feather: false,
        }
    }
}

impl ConcealPreset {
    pub(crate) fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self {
            name: "現在の設定".to_string(),
            conceal_type: settings.conceal_type,
            mosaic_tile_mode: settings.conceal_mosaic_tile_mode,
            mosaic_boundary: settings.conceal_mosaic_boundary,
            fill_opacity_percent: settings.conceal_fill_opacity_percent,
            fill_edge: settings.conceal_fill_edge,
            blur_radius_px: settings.conceal_blur_radius_px,
            blur_mode: settings.conceal_blur_mode,
            blur_feather: settings.conceal_blur_feather,
        }
    }
}

/// Settings のデフォルト値 (1% 刻みスライダーの 100%)。
pub fn default_fill_opacity() -> u8 {
    100
}

/// Settings のデフォルト値 (ぼかし半径 20px)。
pub fn default_blur_radius_px() -> f32 {
    20.0
}

/// ConcealPreset 4 スロットの永続化型 (`settings.conceal_presets`)。
///
/// `[Option<ConcealPreset>; 4]` は serde で扱えるが、JSON では配列固定長として
/// 表現される。`None` (= 空スロット) はそのまま `null` で永続化される。
pub type ConcealPresetSlots = [Option<ConcealPreset>; 4];

pub fn default_conceal_presets() -> ConcealPresetSlots {
    [None, None, None, None]
}

// ── ExportFallbackFormat ────────────────────────────────────────────────

/// `Ctrl+E` で HEIC / AVIF / JXL / RAW / TIFF など書き出し非対応形式が選ばれたとき、
/// どの形式にフォールバックするか (Phase 6 で使用、Phase 1 では型定義のみ)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportFallbackFormat {
    /// JPEG q=95 にフォールバック (写真向け、メタデータ保持可)。
    #[default]
    Jpeg95,
    /// PNG にフォールバック (ロスレス、AI 画像向け、tEXt でメタデータ保持可)。
    Png,
}

// ── テスト ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conceal_type_cycle() {
        let mut t = ConcealType::Mosaic;
        for _ in 0..4 {
            t = t.next();
        }
        assert_eq!(t, ConcealType::Mosaic, "T キー 4 回で 1 周する");
    }

    #[test]
    fn compute_tile_size_long_edge_ratio() {
        // 1400px 長辺、倍率 1.0 → base=14、結果 14
        assert_eq!(
            compute_tile_size(1400, TileSizeMode::LongEdgeRatio(1.0)),
            14
        );
        // 4000px 長辺、倍率 2.0 → base=40、結果 80
        assert_eq!(
            compute_tile_size(4000, TileSizeMode::LongEdgeRatio(2.0)),
            80
        );
        // 400px 長辺、倍率 1.0 → base=max(4, round(4.0))=4、結果 4
        assert_eq!(compute_tile_size(400, TileSizeMode::LongEdgeRatio(1.0)), 4);
        // 100px 長辺、倍率 0.25 → base=max(4, round(1.0))=4、結果 max(round(1.0), 4)=4
        assert_eq!(compute_tile_size(100, TileSizeMode::LongEdgeRatio(0.25)), 4);
    }

    #[test]
    fn compute_tile_size_fixed_px() {
        assert_eq!(compute_tile_size(1400, TileSizeMode::FixedPx(16)), 16);
        assert_eq!(compute_tile_size(4000, TileSizeMode::FixedPx(8)), 8);
        // 最小 4 にクランプ
        assert_eq!(compute_tile_size(1400, TileSizeMode::FixedPx(1)), 4);
        assert_eq!(compute_tile_size(1400, TileSizeMode::FixedPx(0)), 4);
    }

    #[test]
    fn tile_size_mode_serde_roundtrip() {
        let m = TileSizeMode::LongEdgeRatio(2.5);
        let j = serde_json::to_string(&m).unwrap();
        // tag=mode, content=value 形式
        assert!(j.contains("\"mode\":\"long_edge_ratio\""));
        assert!(j.contains("\"value\":2.5"));
        let back: TileSizeMode = serde_json::from_str(&j).unwrap();
        assert_eq!(back, m);

        let fixed = TileSizeMode::FixedPx(16);
        let j2 = serde_json::to_string(&fixed).unwrap();
        let back2: TileSizeMode = serde_json::from_str(&j2).unwrap();
        assert_eq!(back2, fixed);
    }

    #[test]
    fn conceal_preset_default_values() {
        let p = ConcealPreset::default();
        assert_eq!(p.conceal_type, ConcealType::Mosaic);
        assert_eq!(p.mosaic_tile_mode, TileSizeMode::LongEdgeRatio(1.0));
        assert_eq!(p.mosaic_boundary, MosaicBoundary::Opaque);
        assert_eq!(p.fill_opacity_percent, 100);
        assert_eq!(p.fill_edge, FillEdge::Sharp);
        assert!((p.blur_radius_px - 20.0).abs() < 1e-6);
        assert_eq!(p.blur_mode, BlurMode::AsMask);
        assert!(!p.blur_feather);
    }

    #[test]
    fn conceal_preset_serde_default_missing_fields() {
        // 旧バージョンが書いた preset (例: blur_mode が欠落) を新コードで読んでも
        // serde(default) で埋まる
        let json = r#"{"name":"test","conceal_type":"mosaic"}"#;
        let p: ConcealPreset = serde_json::from_str(json).unwrap();
        assert_eq!(p.name, "test");
        assert_eq!(p.conceal_type, ConcealType::Mosaic);
        assert_eq!(p.fill_opacity_percent, 100); // default
        assert!((p.blur_radius_px - 20.0).abs() < 1e-6); // default
    }

    #[test]
    fn conceal_preset_slots_default_all_none() {
        let slots = default_conceal_presets();
        for slot in slots.iter() {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn process_descriptions_are_non_empty_strings() {
        // ポリシー: ラベルは「処理内容を具体的に書く」。空文字や「強い」等の
        // 評価語ではないことだけ確認する (具体的な文言は UI 改修で変わりうる)。
        for b in [
            MosaicBoundary::Opaque,
            MosaicBoundary::Translucent,
            MosaicBoundary::MaskShape,
        ] {
            assert!(!b.process_description().is_empty());
        }
        for e in [FillEdge::Sharp, FillEdge::Feathered] {
            assert!(!e.process_description().is_empty());
        }
        for m in [
            BlurMode::AsMask,
            BlurMode::ExtendByRadius,
            BlurMode::InsideOnly,
        ] {
            assert!(!m.process_description().is_empty());
        }
    }
}
