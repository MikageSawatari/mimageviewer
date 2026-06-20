use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

const MAX_FAVORITES: usize = 20;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DetachedViewerWindowPlacement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl DetachedViewerWindowPlacement {
    pub fn is_sane(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.w.is_finite()
            && self.h.is_finite()
            && self.w >= 320.0
            && self.h >= 240.0
            && self.w <= 16_384.0
            && self.h <= 16_384.0
    }
}

// -----------------------------------------------------------------------
// FavoriteEntry
// -----------------------------------------------------------------------

/// お気に入りフォルダの 1 エントリ。
///
/// `name` はユーザが任意に付けられる表示名 (ツールバーのボタンラベル等で使用)。
/// 既定ではフォルダ名 (`path.file_name()`) が入る。
///
/// `id` は Tantivy / fts_meta.db の `favorite_id` として使われる安定 UUID。
/// お気に入りを表示名 rename しても保持される。root path を変更すると index は
/// 再スキャンされる (docs/search-expansion-design.md §5.5)。
///
/// `auto_index_*` はお気に入り単位の自動インデックス管理フラグ (v0.8.0 新設)。
/// 既存お気に入りは全て false 初期値で読み込まれ、後段の UI で個別 ON にする。
///
/// JSON 上の互換性:
/// - 旧 (v0.7 以前): 文字列 (パス) または `{"name", "path"}` の 2 フィールドオブジェクト
/// - 新 (v0.8): 上記 + `id`, `auto_index_*` (欠落時はデフォルト値)
#[derive(Clone, Debug)]
pub struct FavoriteEntry {
    /// 安定 UUID。Tantivy / fts_meta.db の favorite_id。
    /// 既存エントリ or 旧形式は読込時に `Uuid::new_v4()` で発行される。
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
    /// Ctrl+S (フォルダ/ZIP/PDF/動画名) の自動インデックス対象にするか。
    pub auto_index_structure: bool,
    /// Ctrl+F/G (全文メタデータ) の自動インデックス対象にするか。
    pub auto_index_metadata: bool,
    /// サムネイル事前キャッシュを自動生成するか。
    pub auto_index_thumbs: bool,
}

impl<'de> serde::Deserialize<'de> for FavoriteEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // 旧: 文字列 or パス (例: "C:\\foo")
        // 新 v0.7 系: {"name", "path"} の 2 フィールド
        // 新 v0.8 系: + id, auto_index_* (欠落時デフォルト値)
        //
        // id 欠落時は Uuid::nil() をプレースホルダとして deserialize し、
        // 後段 (Settings::load の sanitize) で nil を検出したら新規 UUID を発行する。
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Legacy(PathBuf),
            Full {
                #[serde(default)]
                id: Option<Uuid>,
                name: String,
                path: PathBuf,
                #[serde(default)]
                auto_index_structure: bool,
                #[serde(default)]
                auto_index_metadata: bool,
                #[serde(default)]
                auto_index_thumbs: bool,
            },
        }

        match Raw::deserialize(deserializer)? {
            Raw::Legacy(p) => {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                Ok(FavoriteEntry {
                    id: Uuid::nil(), // 後段の sanitize で UUID v4 を発行
                    name,
                    path: p,
                    auto_index_structure: false,
                    auto_index_metadata: false,
                    auto_index_thumbs: false,
                })
            }
            Raw::Full {
                id,
                name,
                path,
                auto_index_structure,
                auto_index_metadata,
                auto_index_thumbs,
            } => Ok(FavoriteEntry {
                id: id.unwrap_or_else(Uuid::nil),
                name,
                path,
                auto_index_structure,
                auto_index_metadata,
                auto_index_thumbs,
            }),
        }
    }
}

impl serde::Serialize for FavoriteEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("FavoriteEntry", 6)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("path", &self.path)?;
        s.serialize_field("auto_index_structure", &self.auto_index_structure)?;
        s.serialize_field("auto_index_metadata", &self.auto_index_metadata)?;
        s.serialize_field("auto_index_thumbs", &self.auto_index_thumbs)?;
        s.end()
    }
}

impl FavoriteEntry {
    /// 新しいお気に入りエントリを作る (UUID は自動発行、フラグは全 false)。
    pub fn new(name: String, path: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            path,
            auto_index_structure: false,
            auto_index_metadata: false,
            auto_index_thumbs: false,
        }
    }
}

// -----------------------------------------------------------------------
// TagDef (docs/tag-feature.md)
// -----------------------------------------------------------------------

/// ユーザ定義のタグ 1 エントリ。
///
/// `name` は `#` を除いた表示名 (例: "原神")。mIV タグの正本は tags.db で、
/// 画面表示時だけ `#` が付く。
///
/// `id` は順序変更・改名時の安定識別子。ツールバー/メニューのキー、
/// 将来の統計情報等で使用。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TagDef {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub tag_key: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub show_shortcut: bool,
}

impl TagDef {
    pub fn new(name: String) -> Self {
        let name = crate::tags_db::normalize_tag_display_name(&name);
        Self {
            id: Uuid::new_v4(),
            tag_key: crate::tags_db::normalize_tag_key(&name),
            name,
            show_shortcut: true,
        }
    }

    /// `#name` 形式 (検索・保存時の形式)。
    pub fn with_hash(&self) -> String {
        format!("#{}", self.name)
    }
}

// -----------------------------------------------------------------------
// サムネイルアスペクト比
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum ThumbAspect {
    Landscape16x9,
    Landscape3x2,
    Landscape4x3,
    #[default]
    Square,
    Portrait3x4,
    Portrait2x3,
    Portrait9x16,
}

impl ThumbAspect {
    /// セル幅に対するセル高さの比率
    pub fn height_ratio(self) -> f32 {
        match self {
            Self::Landscape16x9 => 9.0 / 16.0,
            Self::Landscape3x2 => 2.0 / 3.0,
            Self::Landscape4x3 => 3.0 / 4.0,
            Self::Square => 1.0,
            Self::Portrait3x4 => 4.0 / 3.0,
            Self::Portrait2x3 => 3.0 / 2.0,
            Self::Portrait9x16 => 16.0 / 9.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Landscape16x9 => "16:9",
            Self::Landscape3x2 => "3:2",
            Self::Landscape4x3 => "4:3",
            Self::Square => "1:1",
            Self::Portrait3x4 => "3:4",
            Self::Portrait2x3 => "2:3",
            Self::Portrait9x16 => "9:16",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Landscape16x9,
            Self::Landscape3x2,
            Self::Landscape4x3,
            Self::Square,
            Self::Portrait3x4,
            Self::Portrait2x3,
            Self::Portrait9x16,
        ]
    }
}

// -----------------------------------------------------------------------
// グリッド表示モード
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GridViewMode {
    #[default]
    Thumbnail,
    Details,
}

impl GridViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thumbnail => "サムネ",
            Self::Details => "詳細",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Thumbnail, Self::Details]
    }
}

// -----------------------------------------------------------------------
// 詳細表示ソート
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DetailsSortKey {
    #[default]
    Toolbar,
    Name,
    Rating,
    Tags,
    Kind,
    Size,
    Modified,
    Created,
    State,
    ImageDimensions,
    VideoDuration,
    VideoDimensions,
    VideoCodec,
}

impl DetailsSortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Toolbar => "ツールバー順",
            Self::Name => "名前",
            Self::Rating => "★",
            Self::Tags => "タグ",
            Self::Kind => "種類",
            Self::Size => "サイズ",
            Self::Modified => "更新日時",
            Self::Created => "作成日時",
            Self::State => "状態",
            Self::ImageDimensions => "画像解像度",
            Self::VideoDuration => "動画長さ",
            Self::VideoDimensions => "動画解像度",
            Self::VideoCodec => "動画コーデック",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DetailsSizeDisplayMode {
    #[default]
    Optimal,
    FixedBytes,
    FixedKb,
    FixedMb,
}

impl DetailsSizeDisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Optimal => "最適",
            Self::FixedBytes => "固定: バイト",
            Self::FixedKb => "固定: KB",
            Self::FixedMb => "固定: MB",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Optimal,
            Self::FixedBytes,
            Self::FixedKb,
            Self::FixedMb,
        ]
    }
}

#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum DetailsColumnId {
    Preview,
    Name,
    Rating,
    Tags,
    Kind,
    Size,
    Modified,
    Created,
    State,
    ImageDimensions,
    VideoDuration,
    VideoDimensions,
    VideoCodec,
}

impl DetailsColumnId {
    pub fn default_order() -> &'static [Self] {
        &[
            Self::Preview,
            Self::Name,
            Self::Rating,
            Self::Tags,
            Self::Kind,
            Self::Size,
            Self::Modified,
            Self::Created,
            Self::State,
            Self::ImageDimensions,
            Self::VideoDuration,
            Self::VideoDimensions,
            Self::VideoCodec,
        ]
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct DetailsColumnWidth {
    pub column: DetailsColumnId,
    pub width: f32,
}

// -----------------------------------------------------------------------
// スマートフィルタ (軽量 facet)
// -----------------------------------------------------------------------

#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum FacetItemKind {
    Folder,
    Image,
    Video,
    Zip,
    Pdf,
    Archive,
    ZipImage,
    PdfPage,
    Separator,
    SearchContainer,
}

impl FacetItemKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "フォルダ",
            Self::Image => "画像",
            Self::Video => "動画",
            Self::Zip => "ZIP",
            Self::Pdf => "PDF",
            Self::Archive => "変換アーカイブ",
            Self::ZipImage => "ZIP内画像",
            Self::PdfPage => "PDFページ",
            Self::Separator => "見出し",
            Self::SearchContainer => "検索コンテナ",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FacetTagMode {
    #[default]
    Any,
    All,
}

impl FacetTagMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "OR",
            Self::All => "AND",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetDatePreset {
    Last7Days,
    Last30Days,
    Last365Days,
}

impl FacetDatePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Last7Days => "7日以内",
            Self::Last30Days => "30日以内",
            Self::Last365Days => "1年以内",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Last7Days, Self::Last30Days, Self::Last365Days]
    }

    pub fn seconds(self) -> i64 {
        match self {
            Self::Last7Days => 7 * 24 * 60 * 60,
            Self::Last30Days => 30 * 24 * 60 * 60,
            Self::Last365Days => 365 * 24 * 60 * 60,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetSizePreset {
    Under1MiB,
    MiB1To10,
    MiB10To100,
    Over100MiB,
}

impl FacetSizePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Under1MiB => "1MB未満",
            Self::MiB1To10 => "1〜10MB",
            Self::MiB10To100 => "10〜100MB",
            Self::Over100MiB => "100MB以上",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Under1MiB,
            Self::MiB1To10,
            Self::MiB10To100,
            Self::Over100MiB,
        ]
    }

    pub fn range_bytes(self) -> (u64, Option<u64>) {
        const MIB: u64 = 1024 * 1024;
        match self {
            Self::Under1MiB => (0, Some(MIB)),
            Self::MiB1To10 => (MIB, Some(10 * MIB)),
            Self::MiB10To100 => (10 * MIB, Some(100 * MIB)),
            Self::Over100MiB => (100 * MIB, None),
        }
    }
}

#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum FacetEditFlag {
    Adjustment,
    /// 旧開発版の「AI補正あり」。読み込み互換のため残すが、UI には出さない。
    AiAdjustment,
    LocalAdjustment,
    Mask,
    Conceal,
    Annotation,
    Rotation,
    Tagged,
    Untagged,
    Rated,
    Unrated,
}

impl FacetEditFlag {
    pub fn label(self) -> &'static str {
        match self {
            Self::Adjustment => "補",
            Self::AiAdjustment => "AI",
            Self::LocalAdjustment => "レ",
            Self::Mask => "消",
            Self::Conceal => "隠",
            Self::Annotation => "文",
            Self::Rotation => "回",
            Self::Tagged => "タグあり",
            Self::Untagged => "タグなし",
            Self::Rated => "★あり",
            Self::Unrated => "★なし",
        }
    }

    pub fn menu_label(self) -> &'static str {
        match self {
            Self::Adjustment => "補（補正）",
            Self::AiAdjustment => "AI補正あり",
            Self::LocalAdjustment => "レ（補正レイヤー）",
            Self::Mask => "消（消しゴムマスク）",
            Self::Conceal => "隠（隠蔽加工）",
            Self::Annotation => "文（テキスト注釈）",
            Self::Rotation => "回（回転）",
            Self::Tagged => "タグあり",
            Self::Untagged => "タグなし",
            Self::Rated => "★あり",
            Self::Unrated => "★なし",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Adjustment,
            Self::LocalAdjustment,
            Self::Mask,
            Self::Conceal,
            Self::Annotation,
            Self::Rotation,
            Self::Tagged,
            Self::Untagged,
            Self::Rated,
            Self::Unrated,
        ]
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct FacetFilter {
    #[serde(default)]
    pub kinds: std::collections::BTreeSet<FacetItemKind>,
    #[serde(default)]
    pub exts: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub ai_models: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub ai_tools: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub tags: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub tag_mode: FacetTagMode,
    #[serde(default)]
    pub include_untagged: bool,
    #[serde(default)]
    pub date_preset: Option<FacetDatePreset>,
    #[serde(default)]
    pub size_preset: Option<FacetSizePreset>,
    #[serde(default)]
    pub edits: std::collections::BTreeSet<FacetEditFlag>,
    #[serde(default, alias = "ai_adjustment_include_descendants")]
    pub edit_include_descendants: bool,
}

impl FacetFilter {
    pub fn is_active(&self) -> bool {
        !self.kinds.is_empty()
            || !self.exts.is_empty()
            || !self.ai_models.is_empty()
            || !self.ai_tools.is_empty()
            || !self.tags.is_empty()
            || self.include_untagged
            || self.date_preset.is_some()
            || self.size_preset.is_some()
            || !self.edits.is_empty()
    }

    pub fn uses_tag_state(&self) -> bool {
        !self.tags.is_empty()
            || self.include_untagged
            || self.edits.contains(&FacetEditFlag::Tagged)
            || self.edits.contains(&FacetEditFlag::Untagged)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn has_rollup_edit_filter(&self) -> bool {
        self.edits.iter().any(|flag| flag.rolls_up_to_containers())
    }
}

impl FacetEditFlag {
    pub fn rolls_up_to_containers(self) -> bool {
        matches!(
            self,
            Self::Adjustment
                | Self::AiAdjustment
                | Self::LocalAdjustment
                | Self::Mask
                | Self::Conceal
                | Self::Annotation
                | Self::Rotation
        )
    }
}

// -----------------------------------------------------------------------
// ツールバーセクションの表示形式
// -----------------------------------------------------------------------

/// ツールバーの各セクションの表示形式。
///
/// `Buttons` (展開): 横並びの `selectable_label` 群。すべての選択肢が常時見える。
/// `Collapsible` (折りたたみ): 展開と同じ横並びだが ▶/▽ で畳める (お気に入り/タグ/本棚のみ)。
/// `Dropdown` (プルダウン): `ComboBox` + アクションボタン。選択肢を一覧してスペース節約。
/// `Unknown`: 将来バージョンの未知の値を旧バイナリが読んだ場合の安全弁 (= 展開扱い)。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum ToolbarSectionDisplay {
    #[default]
    Buttons,
    Collapsible,
    Dropdown,
    /// 未知の variant (ダウングレード耐性)。描画では展開 (Buttons) 扱いにする。
    #[serde(other)]
    Unknown,
}

impl ToolbarSectionDisplay {
    /// 列 / 比率 / ソート用 (展開 / プルダウンの 2 択。`Collapsible` は付けない)。
    pub fn all() -> &'static [Self] {
        &[Self::Buttons, Self::Dropdown]
    }
    /// お気に入り / タグ用 (展開 / 折りたたみ / プルダウンの 3 択)。
    pub fn all_with_collapsible() -> &'static [Self] {
        &[Self::Buttons, Self::Collapsible, Self::Dropdown]
    }
    /// 本棚用 (展開 / 折りたたみの 2 択)。本棚はコンボ(全本)が常時あり、ピンは常にボタンで
    /// 出すので、プルダウンは設けない (展開と区別が付かないため)。
    pub fn all_collapsible_only() -> &'static [Self] {
        &[Self::Buttons, Self::Collapsible]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Buttons | Self::Unknown => "展開",
            Self::Collapsible => "折りたたみ",
            Self::Dropdown => "プルダウン",
        }
    }
}

// -----------------------------------------------------------------------
// ToolbarSectionId (v2.0.0)
// -----------------------------------------------------------------------

/// ツールバーのセクション識別子。
///
/// セクション描画をハードコード順から `toolbar_section_order` によるデータ駆動ループへ
/// 移すための列挙 (v2.0.0 Phase 1)。並べ替え (ドラッグ) と表示/非表示カスタマイズの土台。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolbarSectionId {
    FolderTree,
    Bookshelf,
    Cols,
    Aspect,
    Sort,
    Rating,
    Favorites,
    Tags,
    /// 未知のセクション (将来バージョンが書いた変種を旧バイナリが読んだ場合)。
    /// `#[serde(other)]` でデシリアライズをエラーにせずここへ落とし、
    /// `ordered_with_fallback` が描画前に除外する (ダウングレード時の settings 全損を防ぐ)。
    #[serde(other)]
    Unknown,
}

impl ToolbarSectionId {
    /// 既定の並び順 (= v1.x までのハードコード順)。これを崩すと既存ユーザーの
    /// 見た目が変わるので、`toolbar_section_order` 未設定時は必ずこの順を使う。
    pub fn default_order() -> &'static [Self] {
        &[
            Self::FolderTree,
            Self::Bookshelf,
            Self::Cols,
            Self::Aspect,
            Self::Sort,
            Self::Rating,
            Self::Favorites,
            Self::Tags,
        ]
    }

    /// 保存済み順序に、未登録のセクションを既定順で末尾追加して返す。
    ///
    /// 詳細列の `details_ordered_columns` と同じ「保存順に無い新項目は末尾追加」方式。
    /// 空 Vec は既定順そのまま。重複は最初の 1 つだけ採用 (破損データ耐性)。
    /// 後方互換 / 将来のセクション追加の両方に耐える。
    pub fn ordered_with_fallback(saved: &[Self]) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::with_capacity(Self::default_order().len());
        for &id in saved {
            // 未知セクションは描画対象にしない (forward-compat: 将来変種を読み飛ばす)。
            if id == Self::Unknown {
                continue;
            }
            if !out.contains(&id) {
                out.push(id);
            }
        }
        for &id in Self::default_order() {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

// -----------------------------------------------------------------------
// SortOrder
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum SortOrder {
    #[default]
    FileName, // ファイル名順（辞書順）
    Numeric,  // 番号順（自然順: 1, 2, 9, 10, 11）
    DateAsc,  // 日付順（昇順）
    DateDesc, // 日付順（降順）
}

impl SortOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::FileName => "ファイル名順",
            Self::Numeric => "番号順",
            Self::DateAsc => "日付順（古い順）",
            Self::DateDesc => "日付順（新しい順）",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::FileName => "名前",
            Self::Numeric => "番号",
            Self::DateAsc => "日付↑",
            Self::DateDesc => "日付↓",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::FileName, Self::Numeric, Self::DateAsc, Self::DateDesc]
    }

    /// 2 つのメディア項目をこのソート順で比較する。
    /// `name_a`/`name_b` はファイル名（拡張子付き）、`mtime_a`/`mtime_b` は更新日時。
    /// `natural_key` は番号順ソート用のキー生成関数。
    ///
    /// 日付ソートで mtime が等しい場合、および番号順で natural key が等しい場合は
    /// ファイル名昇順で tiebreak する。
    /// - `mtime_secs` は秒精度なので、同一秒に作成・更新されたファイル群が
    ///   `read_dir` 順 (FS 依存で不安定) に並ぶのを防ぐ。
    /// - 番号順の natural key は記号・空白を除去するため `foo-bar1` / `foobar1` /
    ///   `foo bar1` のように記号差だけが違うファイルが同値になる。tiebreak が無いと
    ///   このグループ内で `read_dir` 列挙順がそのまま残り、表示が不安定になる。
    pub fn compare<K: Ord>(
        self,
        name_a: &str,
        mtime_a: i64,
        name_b: &str,
        mtime_b: i64,
        natural_key: impl Fn(&str) -> K,
    ) -> std::cmp::Ordering {
        match self {
            Self::FileName => name_a.to_lowercase().cmp(&name_b.to_lowercase()),
            Self::Numeric => natural_key(name_a)
                .cmp(&natural_key(name_b))
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase())),
            Self::DateAsc => mtime_a
                .cmp(&mtime_b)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase())),
            Self::DateDesc => mtime_b
                .cmp(&mtime_a)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase())),
        }
    }
}

// -----------------------------------------------------------------------
// CachePolicy
// -----------------------------------------------------------------------

/// サムネイルキャッシュの生成ポリシー（段階 C）。
///
/// - `Off`: 新規キャッシュを生成しない（既存キャッシュは引き続き読み込む）
/// - `Auto`: 実測時間としきい値/サイズによる自動判定（推奨デフォルト）
/// - `Always`: 現状互換の全件生成
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum CachePolicy {
    Off,
    #[default]
    Auto,
    Always,
}

impl CachePolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off（生成しない）",
            Self::Auto => "Auto（自動判定・推奨）",
            Self::Always => "Always（常に生成）",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Auto, Self::Always]
    }
}

// -----------------------------------------------------------------------
// ArchiveFileHandling
// -----------------------------------------------------------------------

/// RAR / 7z / LZH など、ZIP 変換キャッシュを作って閲覧するアーカイブの扱い。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFileHandling {
    /// 旧 `archive_convert_without_dialog` だけが存在する設定からの読み込み直後。
    /// UI や実行時判定には出さず、load-time migration で Ask / Convert に寄せる。
    Legacy,
    /// 開く前に確認ダイアログを表示する。
    Ask,
    /// 確認ダイアログを省略して変換を開始する。
    Convert,
    /// 一覧・フォルダ移動・ZIP 内の入れ子提案で変換対象アーカイブを扱わない。
    Ignore,
}

impl Default for ArchiveFileHandling {
    fn default() -> Self {
        Self::Legacy
    }
}

impl ArchiveFileHandling {
    pub fn from_legacy_without_dialog(without_dialog: bool) -> Self {
        if without_dialog {
            Self::Convert
        } else {
            Self::Ask
        }
    }

    pub fn resolved(self, legacy_without_dialog: bool) -> Self {
        match self {
            Self::Legacy => Self::from_legacy_without_dialog(legacy_without_dialog),
            other => other,
        }
    }

    pub fn all_user_visible() -> &'static [Self] {
        &[Self::Ask, Self::Convert, Self::Ignore]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy | Self::Ask => "確認してからキャッシュを作成する",
            Self::Convert => "確認せずキャッシュを作成する",
            Self::Ignore => "無視する",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Legacy | Self::Ask => {
                "未変換の RAR / 7z / LZH を開くときに確認ダイアログを表示します。"
            }
            Self::Convert => {
                "確認画面だけを省略して変換します。進捗、パスワード入力、エラーは表示します。"
            }
            Self::Ignore => {
                "RAR / 7z / LZH を一覧やフォルダ移動の対象にせず、変換キャッシュも開きません。"
            }
        }
    }
}

// -----------------------------------------------------------------------
// インデクサ速度プロファイル (v0.8.0, docs/search-expansion-design.md §7.5)
// -----------------------------------------------------------------------

/// バックグラウンドインデクサの速度プロファイル。
/// I/O 同時実行数 (GlobalIoSemaphore の permits) を決定する。
///
/// - `Low`: HDD / NAS / バッテリー向け。1 permit で UI 操作を最優先 (**デフォルト**, 2026-04 変更)
/// - `Medium`: HDD + SSD 混成。2 permits
/// - `High`: NVMe SSD。4 permits で初回インデックスを高速化
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IndexerSpeedProfile {
    #[default]
    Low,
    Medium,
    High,
}

impl IndexerSpeedProfile {
    /// `GlobalIoSemaphore` の permit 数。
    pub fn io_permits(self) -> usize {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low (1 permit, UI 優先, 既定)",
            Self::Medium => "Medium (2 permits)",
            Self::High => "High (4 permits, NVMe 向け)",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Low, Self::Medium, Self::High]
    }
}

// -----------------------------------------------------------------------
// Parallelism
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "mode", content = "value")]
pub enum Parallelism {
    Auto,
    Manual(usize),
}

impl Default for Parallelism {
    fn default() -> Self {
        Self::Auto
    }
}

impl Parallelism {
    /// 実際に使うスレッド数を返す
    pub fn thread_count(&self) -> usize {
        match self {
            Self::Auto => {
                let cores = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(2);
                (cores / 2).max(1)
            }
            Self::Manual(n) => (*n).max(1),
        }
    }
}

// -----------------------------------------------------------------------
// UiTheme (UI 背景色テーマ)
// -----------------------------------------------------------------------

/// UI 背景色テーマ (v0.7.0)。
///
/// - `System` (デフォルト): Windows の「アプリ用の色」に追従。レジストリから検出し、
///   起動時に Light または Dark を適用する。取得失敗時は Light にフォールバック。
/// - `Light`: メインウィンドウ・サムネイルは白基調、フルスクリーンは黒地
///   (フルスクリーン枠は `ui_fullscreen.rs` で `Color32::BLACK` にハードコード済み)
/// - `Dark`: メインウィンドウ・サムネイルとも暗色基調、フルスクリーンは黒地
///
/// `Standard` は v0.7.0 開発初期の互換のために残置されているが、視覚的には `Light` と同じ。
/// 新規 UI は `System` / `Light` / `Dark` の 3 択をユーザーに提示する。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum UiTheme {
    #[default]
    System,
    /// 互換目的。視覚的には `Light` と等価。
    Standard,
    Light,
    Dark,
}

impl UiTheme {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "システムに合わせる",
            Self::Standard => "標準",
            Self::Light => "ライト",
            Self::Dark => "ダーク",
        }
    }
}

// -----------------------------------------------------------------------
// AiFeatureMode (AI 利用範囲)
// -----------------------------------------------------------------------

/// アプリ全体の AI 機能利用範囲。
///
/// ページ個別 / お気に入り標準 / グローバルプリセットに保存された AI 設定は保持したまま、
/// 実行時に使うモデル範囲を制限する。低負荷モードへ切り替えてもユーザーのページ設定を
/// 破棄せず、あとで高画質へ戻したときに復元できるようにする。
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AiFeatureMode {
    /// AI アップスケール / AI ノイズ除去を実行しない。
    Disabled,
    /// 高速汎用 + 漫画トーン保持のみ。ノイズ除去は実行しない。
    #[default]
    Light,
    /// すべてのアップスケールモデルとノイズ除去を許可する。
    HighQuality,
}

impl AiFeatureMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "なし",
            Self::Light => "軽量",
            Self::HighQuality => "高画質",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Disabled => {
                "低スペック環境向け。AI アップスケールと AI ノイズ除去を実行しません。"
            }
            Self::Light => "軽め。高速汎用と漫画トーン保持モデルだけを使います。",
            Self::HighQuality => "GPU負荷高め。全アップスケールモデルとノイズ除去を使えます。",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Disabled, Self::Light, Self::HighQuality]
    }

    pub fn allows_upscale_model(self, kind: crate::ai::ModelKind) -> bool {
        match self {
            Self::Disabled => false,
            Self::Light => matches!(
                kind,
                crate::ai::ModelKind::UpscaleRealEsrGeneralV3
                    | crate::ai::ModelKind::UpscaleRealCugan4x
            ),
            Self::HighQuality => crate::ai::ModelKind::upscale_models().contains(&kind),
        }
    }

    pub fn allows_denoise(self) -> bool {
        matches!(self, Self::HighQuality)
    }

    pub fn auto_upscale_model(
        self,
        category: crate::ai::ImageCategory,
    ) -> Option<crate::ai::ModelKind> {
        match self {
            Self::Disabled => None,
            Self::Light => match category {
                crate::ai::ImageCategory::Comic => Some(crate::ai::ModelKind::UpscaleRealCugan4x),
                crate::ai::ImageCategory::Illustration
                | crate::ai::ImageCategory::ThreeD
                | crate::ai::ImageCategory::RealLife => {
                    Some(crate::ai::ModelKind::UpscaleRealEsrGeneralV3)
                }
            },
            Self::HighQuality => Some(category.preferred_upscale_model()),
        }
    }
}

/// - `RtlCover`: 見開き 右→左（表紙あり）— [0] [1,2] [3,4] ...
/// 動画 / ZIP・PDF 本を開く・移動したときに、前回位置 (続き) から始めるか先頭からか。
/// 「エントリ方法 (一覧から開く / Ctrl+↑↓ 移動) × メディア (動画 / 本)」の各セルに使う共通 enum。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResumeMode {
    /// 前回の位置 (動画=再生秒 / 本=最後に読んだページ) から。保存が無ければ先頭にフォールバック。
    #[default]
    Resume,
    /// 常に先頭 (動画=0 秒 / 本=1 ページ目) から。
    FromStart,
}

impl ResumeMode {
    /// 続きから復元するか (= Resume)。
    pub fn resumes(self) -> bool {
        matches!(self, Self::Resume)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Resume => "続きから",
            Self::FromStart => "最初から",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Resume, Self::FromStart]
    }
}

/// `book_nav_resume` の serde 既定 (= 従来の「フォルダ先頭着地」)。
fn default_resume_from_start() -> ResumeMode {
    ResumeMode::FromStart
}

fn default_reading_history_limit() -> usize {
    crate::reading_history_db::READING_HISTORY_LIMIT_DEFAULT
}

// -----------------------------------------------------------------------
// SpreadMode (フルスクリーンページ構成)
// -----------------------------------------------------------------------

/// フルスクリーンのページ構成。
///
/// - `Single`: 通常の1ページ表示
/// - `Ltr`: 見開き 左→右（表紙なし）— [0,1] [2,3] ...
/// - `LtrCover`: 見開き 左→右（表紙あり）— [0] [1,2] [3,4] ...
/// - `Rtl`: 見開き 右→左（表紙なし）— [0,1] [2,3] ...
/// - `RtlCover`: 見開き 右→左（表紙あり）— [0] [2,1] [4,3] ...
/// - `Vertical`: 旧DB互換用。新規 UI では `ReadingFlow::Vertical` を使う。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum SpreadMode {
    #[default]
    Single,
    Ltr,
    LtrCover,
    Rtl,
    RtlCover,
    Vertical,
}

impl SpreadMode {
    /// 見開き構成か
    pub fn is_spread(self) -> bool {
        matches!(
            self,
            Self::Ltr | Self::LtrCover | Self::Rtl | Self::RtlCover
        )
    }

    /// 旧DB互換の縦読みモードか。新規表示切替では `ReadingFlow::Vertical` を使う。
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Vertical)
    }

    /// 右→左（RTL）モードか
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl | Self::RtlCover)
    }

    /// 表紙（1ページ目単独表示）ありか
    pub fn has_cover(self) -> bool {
        matches!(self, Self::LtrCover | Self::RtlCover)
    }

    /// 整数値 (0-5) から生成
    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::Ltr,
            2 => Self::LtrCover,
            3 => Self::Rtl,
            4 => Self::RtlCover,
            5 => Self::Vertical,
            _ => Self::Single,
        }
    }

    /// 見開きモードの巡回トグルで次のモードを返す。キーボードのショートカット 1〜5 と
    /// 同じ並び (Single → Ltr → LtrCover → Rtl → RtlCover → Single …) を巡回する。
    /// 巡回外のモード (旧 `Vertical` 等) からは先頭 (`Single`) へ。
    /// ゲームパッド Select の見開き切替に使う。
    pub fn next_in_spread_cycle(self) -> Self {
        const CYCLE: [SpreadMode; 5] = [
            SpreadMode::Single,
            SpreadMode::Ltr,
            SpreadMode::LtrCover,
            SpreadMode::Rtl,
            SpreadMode::RtlCover,
        ];
        match CYCLE.iter().position(|&m| m == self) {
            Some(i) => CYCLE[(i + 1) % CYCLE.len()],
            None => CYCLE[0],
        }
    }

    /// 整数値を返す
    pub fn to_int(self) -> i32 {
        match self {
            Self::Single => 0,
            Self::Ltr => 1,
            Self::LtrCover => 2,
            Self::Rtl => 3,
            Self::RtlCover => 4,
            Self::Vertical => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "1ページ表示",
            Self::Ltr => "見開き 左→右",
            Self::LtrCover => "見開き 左→右（表紙あり）",
            Self::Rtl => "見開き 右→左",
            Self::RtlCover => "見開き 右→左（表紙あり）",
            Self::Vertical => "縦読み（旧）",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Single,
            Self::Ltr,
            Self::LtrCover,
            Self::Rtl,
            Self::RtlCover,
        ]
    }

    /// 見開きの表紙有無を保ったまま横方向だけを差し替える。
    ///
    /// Single / 旧 Vertical は見開き構成ではないため、そのまま返す。
    pub fn with_reading_direction(self, direction: ReadingDirection) -> Self {
        match (self, direction) {
            (Self::Ltr | Self::Rtl, ReadingDirection::Ltr) => Self::Ltr,
            (Self::Ltr | Self::Rtl, ReadingDirection::Rtl) => Self::Rtl,
            (Self::LtrCover | Self::RtlCover, ReadingDirection::Ltr) => Self::LtrCover,
            (Self::LtrCover | Self::RtlCover, ReadingDirection::Rtl) => Self::RtlCover,
            (mode, _) => mode,
        }
    }
}

// -----------------------------------------------------------------------
// ReadingFlow (フルスクリーン連結方式)
// -----------------------------------------------------------------------

/// フルスクリーンの連結方式。
///
/// `SpreadMode` が「1ページ / 見開き」を決め、こちらは表示ユニットを
/// 連結せずページ単位で見るか、縦・横に連続配置するかを決める。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReadingFlow {
    #[default]
    Paged,
    Vertical,
    Horizontal,
}

impl ReadingFlow {
    pub fn is_paged(self) -> bool {
        matches!(self, Self::Paged)
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Vertical)
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::Horizontal)
    }

    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::Vertical,
            2 => Self::Horizontal,
            _ => Self::Paged,
        }
    }

    pub fn to_int(self) -> i32 {
        match self {
            Self::Paged => 0,
            Self::Vertical => 1,
            Self::Horizontal => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Paged => "ページ単位",
            Self::Vertical => "縦連結",
            Self::Horizontal => "横連結",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Paged, Self::Vertical, Self::Horizontal]
    }

    pub fn next(self) -> Self {
        match self {
            Self::Paged => Self::Vertical,
            Self::Vertical => Self::Horizontal,
            Self::Horizontal => Self::Paged,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReadingDirection {
    #[default]
    Ltr,
    Rtl,
}

impl ReadingDirection {
    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::Rtl,
            _ => Self::Ltr,
        }
    }

    pub fn to_int(self) -> i32 {
        match self {
            Self::Ltr => 0,
            Self::Rtl => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ltr => "左→右",
            Self::Rtl => "右→左",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Ltr => Self::Rtl,
            Self::Rtl => Self::Ltr,
        }
    }
}

// -----------------------------------------------------------------------
// FullscreenFitMode (フルスクリーン倍率/フィット基準)
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FullscreenFitMode {
    #[default]
    Page,
    MarginFit,
    Width,
    Height,
    Original,
}

impl FullscreenFitMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Page => "ページ全体",
            Self::MarginFit => "ページ全体（余白カットフィット）",
            Self::Width => "横幅フィット",
            Self::Height => "縦幅フィット",
            Self::Original => "100%原寸",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Page, Self::Width, Self::Height, Self::Original]
    }

    pub fn default_for_flow(flow: ReadingFlow) -> Self {
        if flow.is_vertical() {
            Self::Width
        } else if flow.is_horizontal() {
            Self::Height
        } else {
            Self::Page
        }
    }

    pub fn effective_for_flow(self, _flow: ReadingFlow) -> Self {
        if matches!(self, Self::MarginFit) {
            Self::Page
        } else {
            self
        }
    }

    /// フロー上で選べるモード一覧 (ツールバーのメニュー・[0] 循環で共有)。
    /// 旧余白カットフィットは表示トリムへ移行したため、新規選択肢には含めない。
    pub fn selectable_for_flow(_flow: ReadingFlow) -> &'static [Self] {
        Self::all()
    }

    pub fn next_for_flow(self, flow: ReadingFlow) -> Self {
        let modes = Self::selectable_for_flow(flow);
        let current = self.effective_for_flow(flow);
        let pos = modes.iter().position(|&m| m == current).unwrap_or(0);
        modes[(pos + 1) % modes.len()]
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FullscreenLeftPanelTab {
    #[default]
    Adjustment,
    ViewTrim,
}

impl FullscreenLeftPanelTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Adjustment => "画像補正",
            Self::ViewTrim => "表示トリム",
        }
    }
}

// -----------------------------------------------------------------------
// FullscreenJumpMode (Shift+左右の大きめページジャンプ量)
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FullscreenJumpMode {
    #[default]
    Percent,
    FixedPages,
}

impl FullscreenJumpMode {
    pub fn all() -> &'static [Self] {
        &[Self::Percent, Self::FixedPages]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Percent => "全体の割合",
            Self::FixedPages => "固定ページ数",
        }
    }
}

// -----------------------------------------------------------------------
// RecentApp (アプリケーションで開く 履歴)
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct RecentApp {
    pub display_name: String,
    pub exe_path: String,
}

// -----------------------------------------------------------------------
// StartupFolderMode (起動時に開く場所)
// -----------------------------------------------------------------------

#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartupFolderMode {
    /// 前回終了時に表示していた場所。既存挙動との互換のためデフォルト。
    #[default]
    Previous,
    /// Windows のデスクトップ。
    Desktop,
    /// ユーザー指定フォルダ。無効な場合は Desktop にフォールバック。
    Specific,
    /// 実フォルダではなく、接続済みドライブ一覧を表示する。
    Drives,
    /// 実フォルダではなく、最近フルスクリーンで読んだ本の一覧を表示する。
    ReadingHistory,
}

impl<'de> serde::Deserialize<'de> for StartupFolderMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "previous" => Self::Previous,
            "desktop" => Self::Desktop,
            "specific" => Self::Specific,
            "drives" => Self::Drives,
            "reading_history" => Self::ReadingHistory,
            _ => Self::Previous,
        })
    }
}

impl StartupFolderMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Previous => "前回終了した場所",
            Self::Desktop => "デスクトップ",
            Self::Specific => "指定フォルダ",
            Self::Drives => "ドライブ一覧",
            Self::ReadingHistory => "読書履歴",
        }
    }
}

// -----------------------------------------------------------------------
// Settings
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_grid_cols")]
    pub grid_cols: usize,
    #[serde(default)]
    pub grid_view_mode: GridViewMode,
    #[serde(default)]
    pub details_sort_key: DetailsSortKey,
    #[serde(default = "default_details_sort_ascending")]
    pub details_sort_ascending: bool,
    #[serde(default)]
    pub details_size_display_mode: DetailsSizeDisplayMode,
    #[serde(default)]
    pub details_timestamp_show_seconds: bool,
    #[serde(default)]
    pub details_column_order: Vec<DetailsColumnId>,
    #[serde(default)]
    pub details_column_widths: Vec<DetailsColumnWidth>,
    #[serde(default = "default_true")]
    pub details_show_preview: bool,
    #[serde(default = "default_true")]
    pub details_show_rating: bool,
    #[serde(default = "default_true")]
    pub details_show_tags: bool,
    #[serde(default = "default_true")]
    pub details_show_kind: bool,
    #[serde(default = "default_true")]
    pub details_show_size: bool,
    #[serde(default = "default_true")]
    pub details_show_modified: bool,
    #[serde(default)]
    pub details_show_created: bool,
    #[serde(default = "default_true")]
    pub details_show_state: bool,
    #[serde(default)]
    pub details_show_image_dimensions: bool,
    #[serde(default)]
    pub details_show_video_duration: bool,
    #[serde(default)]
    pub details_show_video_dimensions: bool,
    #[serde(default)]
    pub details_show_video_codec: bool,
    #[serde(default)]
    pub facet_filter: FacetFilter,
    /// ユーザーが手動で選んだ比率。Auto モードでも **書き換えない**
    /// (= Manual に戻したときに直前の手動値が復活するよう保持)。
    /// Auto 未確定時の effective 値ではない (= `App::effective_thumb_aspect` 参照)。
    #[serde(default)]
    pub thumb_aspect: ThumbAspect,
    /// 比率の自動選択モード。`true` でフォルダ内容に合わせて自動切替。
    /// デフォルト `false` (既存ユーザー保護)。詳細: [docs/auto-thumb-aspect-plan.md](../../docs/auto-thumb-aspect-plan.md)
    #[serde(default)]
    pub thumb_aspect_auto: bool,
    #[serde(default)]
    pub favorites: Vec<FavoriteEntry>,
    #[serde(default)]
    pub last_folder: Option<PathBuf>,
    #[serde(default)]
    pub startup_folder_mode: StartupFolderMode,
    #[serde(default)]
    pub startup_folder_path: Option<PathBuf>,
    /// 旧形式 (〜v1.5.0) のグローバルな最近開いたフォルダ一覧。v1.6.0 で A/B
    /// スロット別 (`quick_folder_recent_folders`) へ移行したが、初回移行のシード元 +
    /// 旧バージョンへのダウングレード互換のため残し、主スロット A の一覧を書き戻す。
    #[serde(default)]
    pub recent_folders: Vec<PathBuf>,
    /// A/B クイックフォルダごとの最近開いたフォルダ一覧 (v1.6.0+)。
    #[serde(default = "default_quick_folder_recent_folders")]
    pub quick_folder_recent_folders: [Vec<PathBuf>; 2],
    #[serde(default = "default_quick_folder_slots")]
    pub quick_folder_slots: [Option<PathBuf>; 2],
    /// ウィンドウ左上座標 (outer rect)
    #[serde(default)]
    pub window_pos: Option<[f32; 2]>,
    /// ウィンドウサイズ (outer rect)
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    #[serde(default)]
    pub parallelism: Parallelism,
    /// フルサイズ表示時の後方先読み枚数（現在位置より前）
    #[serde(default = "default_prefetch_back")]
    pub prefetch_back: usize,
    /// フルサイズ表示時の前方先読み枚数（現在位置より後）
    #[serde(default = "default_prefetch_forward")]
    pub prefetch_forward: usize,
    /// Ctrl+↑↓ フォルダ移動時に画像なしフォルダをスキップする最大回数（1〜10）
    #[serde(default = "default_folder_skip_limit")]
    pub folder_skip_limit: usize,
    /// サムネイルグリッドのソート順
    #[serde(default)]
    pub sort_order: SortOrder,
    /// サムネイルキャッシュの長辺ピクセル数
    #[serde(default = "default_thumb_px")]
    pub thumb_px: u32,
    /// テキスト注釈 (Ctrl+T) 編集中のプレビュー解像度の分母 (1=原寸 / 2 / 4 / 8)。下げると
    /// 編集中の合成 + GPU upload コストが 1/N² になりドラッグがスムーズになる (R2 perf)。
    /// 表示プレビューだけ縮小し、保存/コピー/比較/書き出しはフル解像度のまま。
    #[serde(default = "default_text_preview_scale")]
    pub text_preview_scale: u32,
    /// サムネイルキャッシュの WebP 品質 (1–100)
    #[serde(default = "default_thumb_quality")]
    pub thumb_quality: u8,
    /// サムネイルキャッシュ生成ポリシー（段階 C）
    #[serde(default)]
    pub cache_policy: CachePolicy,
    /// Auto モード: `decode + display` がこの値以上のファイルをキャッシュ対象にする（ms, 10-100）
    #[serde(default = "default_cache_threshold_ms")]
    pub cache_threshold_ms: u32,
    /// Auto モード: このサイズ以上のファイルは無条件でキャッシュ対象にする（bytes）
    #[serde(default = "default_cache_size_threshold_bytes")]
    pub cache_size_threshold_bytes: u64,
    /// Auto モード: 動画ファイルを無条件でキャッシュ対象にする
    #[serde(default = "default_true")]
    pub cache_videos_always: bool,
    /// Auto モード: 既存 .webp ファイルを無条件でキャッシュ対象にする（デコードが重いため）
    #[serde(default = "default_true")]
    pub cache_webp_always: bool,
    /// Auto モード: PDF ページを無条件でキャッシュ対象にする（PDFium レンダリングが重いため）
    #[serde(default = "default_true")]
    pub cache_pdf_always: bool,
    /// Auto モード: ZIP 内画像を無条件でキャッシュ対象にする（解凍+デコードの二重コスト）
    #[serde(default = "default_true")]
    pub cache_zip_always: bool,
    /// 変換済みアーカイブキャッシュ (RAR / 7z / LZH → ZIP) の容量上限。
    /// 0 は無制限 (= 既存挙動)。
    #[serde(default)]
    pub archive_cache_max_bytes: u64,
    /// RAR / 7z / LZH などの変換対象アーカイブをどう扱うか。
    #[serde(default)]
    pub archive_file_handling: ArchiveFileHandling,
    /// 旧設定互換: RAR / 7z / LZH を開くとき、確認ダイアログを省略するか。
    /// 新規 UI / 実行時判定は `archive_file_handling` を source of truth とし、
    /// この bool は古い設定の読み込み互換と旧版へ戻した場合の近似互換のために同期する。
    #[serde(default)]
    pub archive_convert_without_dialog: bool,
    /// 一括キャッシュ作成: ZIP 内の全画像をキャッシュ対象にする
    #[serde(default)]
    pub batch_cache_zip_contents: bool,
    /// お気に入り > インデックス作成ダイアログで選択されたお気に入りフォルダ。
    /// チェック状態をセッションをまたいで保存する (正規化せず元のパスで記録)。
    #[serde(default)]
    pub search_index_checks: Vec<PathBuf>,
    /// v0.8.0: 自動インデクサの速度プロファイル (docs/search-expansion-design.md §7.5)。
    /// I/O 同時実行数 (GlobalIoSemaphore permits) を決める。
    /// 変更は次回起動時に反映 (IndexerManager::new で読まれる)。
    #[serde(default)]
    pub indexer_speed_profile: IndexerSpeedProfile,
    /// 一括キャッシュ作成: PDF 内の全ページをキャッシュ対象にする
    #[serde(default)]
    pub batch_cache_pdf_contents: bool,
    /// 段階 B: サムネイル先読みの後方ページ数（現在位置より前に保持するページ数）
    #[serde(default = "default_thumb_prev_pages")]
    pub thumb_prev_pages: u32,
    /// 段階 B: サムネイル先読みの前方ページ数（現在位置より後に保持するページ数）
    #[serde(default = "default_thumb_next_pages")]
    pub thumb_next_pages: u32,
    /// 段階 D: サムネイル GPU 使用量の上限 (プライマリ GPU の総 VRAM に対する %)。
    /// 0 で無制限。
    ///
    /// ページ単位先読みで枚数は有界化されるが、巨大セル × 多ページ設定で
    /// 想定外に増えるケースへの安全ネット。超過時は keep_range を縮める。
    /// 実機の VRAM を DXGI で取得し、この % 倍を実上限とする。
    #[serde(default = "default_thumb_vram_cap_percent")]
    pub thumb_vram_cap_percent: u32,
    /// 段階 E: アイドル時にキャッシュから復元されたサムネイルを
    /// 元画像から再デコードして高画質化する。
    ///
    /// `Off`: 何もしない (キャッシュ画質のまま)
    /// `On` : スクロール停止 + 他の要求が全て完了した後、visible 範囲から順次再デコード
    #[serde(default = "default_true")]
    pub thumb_idle_upgrade: bool,
    /// サムネイル選択情報ツールチップにファイル名を表示する。
    #[serde(default = "default_true")]
    pub thumb_tooltip_show_filename: bool,
    /// サムネイル選択情報ツールチップに画像解像度を表示する。
    #[serde(default = "default_true")]
    pub thumb_tooltip_show_image_dimensions: bool,
    /// サムネイル選択情報ツールチップに動画長さを表示する。
    #[serde(default = "default_true")]
    pub thumb_tooltip_show_video_duration: bool,
    /// サムネイル選択情報ツールチップに種類を表示する。
    #[serde(default)]
    pub thumb_tooltip_show_kind: bool,
    /// サムネイル選択情報ツールチップにファイルサイズを表示する。
    #[serde(default)]
    pub thumb_tooltip_show_file_size: bool,
    /// サムネイル選択情報ツールチップに更新日時を表示する。
    #[serde(default)]
    pub thumb_tooltip_show_modified: bool,
    /// サムネイル選択情報ツールチップに作成日時を表示する。
    #[serde(default)]
    pub thumb_tooltip_show_created: bool,
    /// サムネイル選択情報ツールチップに動画解像度を表示する。
    #[serde(default)]
    pub thumb_tooltip_show_video_dimensions: bool,
    /// サムネイル選択情報ツールチップに動画コーデックを表示する。
    #[serde(default)]
    pub thumb_tooltip_show_video_codec: bool,
    /// サムネイル選択情報ツールチップに親フォルダ / コンテナ名を表示する。
    #[serde(default)]
    pub thumb_tooltip_show_location: bool,
    /// サムネイル選択情報ツールチップに読書履歴の最終閲覧日時を表示する。
    #[serde(default = "default_true")]
    pub thumb_tooltip_show_reading_history_last_read: bool,
    /// サムネイル選択情報ツールチップに読書履歴の既読位置を表示する。
    #[serde(default = "default_true")]
    pub thumb_tooltip_show_reading_history_progress: bool,

    // ── タグ機能 (docs/tag-feature.md) ──────────────────────────
    /// ユーザ定義のタグ一覧 (メニュー / ツールバー に表示される順)。
    #[serde(default)]
    pub tags: Vec<TagDef>,

    // ── ツールバー表示設定 ──────────────────────────────────
    /// ツールバーに「お気に入り」セクションを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_favorites: bool,
    /// ツールバーに「タグ」セクションを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_tags: bool,
    /// 左側の実フォルダツリーペインを表示する
    #[serde(default)]
    pub folder_tree_pane_visible: bool,
    /// フォルダツリー左右境界位置 (ウィンドウ幅に対する比率)。
    #[serde(default = "default_folder_tree_pane_width_ratio")]
    pub folder_tree_pane_width_ratio: f32,
    /// フォルダバー (フォルダ入力行) を表示する
    #[serde(default = "default_true")]
    pub show_toolbar_folder: bool,
    /// ツールバーに「ツリー」ボタンを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_folder_tree_button: bool,
    /// ツールバーに「本棚」セクションを表示する。
    #[serde(default = "default_true")]
    pub show_toolbar_bookshelf: bool,
    /// フォルダバーに「履歴を戻る/進む」ボタンを表示する。
    #[serde(default = "default_true")]
    pub show_address_bar_history_nav: bool,
    /// フォルダバーに A/B クイックフォルダボタンを表示する。
    #[serde(default = "default_true")]
    pub show_address_bar_quick_folders: bool,
    /// フォルダバーに「親フォルダへ」ボタンを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_parent_button: bool,
    /// フォルダバーに「ツリー順で前のフォルダへ」ボタンを表示する (Phase 5.8)。
    /// 既定 true、Ctrl+↑ と等価。
    #[serde(default = "default_true")]
    pub show_toolbar_prev_folder: bool,
    /// フォルダバーに「ツリー順で次のフォルダへ」ボタンを表示する (Phase 5.8)。
    /// 既定 true、Ctrl+↓ と等価。
    #[serde(default = "default_true")]
    pub show_toolbar_next_folder: bool,
    /// ツールバーに「VST3 プラグイン管理」ボタン (VST テキスト) を表示する (v0.9.0)。
    /// `vst3_enabled = true` のときだけ実際にツールバーに描画される (= 二重ガード)。
    /// 既定 true。
    #[serde(default = "default_true")]
    pub show_toolbar_vst3: bool,
    /// ツールバーに「レーティングフィルタ」セクション (☆|なし 1 2 3 4 5) を表示する
    #[serde(default = "default_true")]
    pub show_toolbar_rating: bool,
    /// ツールバー下のスマートフィルタバー (絞り込み) を表示する。
    #[serde(default = "default_true")]
    pub show_toolbar_facet_filter: bool,
    /// フォルダバーに「お気に入り追加 / 設定」(♡/♥) ボタンを表示する。
    #[serde(default = "default_true")]
    pub show_address_bar_favorite_button: bool,
    /// フォルダバーに「最近開いたフォルダ」履歴メニューを表示する。
    #[serde(default = "default_true")]
    pub show_address_bar_history_menu: bool,
    /// フォルダバーに「代表サムネ固定」(📌) ボタンを表示する。左クリックで
    /// 現在の選択アイテムをフォルダ / ZIP / PDF のサムネに固定 (= toggle)、
    /// 右クリックで固定解除。既定 true。
    #[serde(default = "default_true")]
    pub show_address_bar_folder_pin: bool,

    // ── エクスプローラ連携 ────────────────────────────────────
    /// 実ファイル / 実フォルダの右クリックに Windows Shell の標準メニューを使う。
    /// ZIP/PDF 内ページなど仮想アイテムは従来の mIV メニューにフォールバックする。
    #[serde(default = "default_true")]
    pub use_native_shell_context_menu: bool,

    /// マウス右フリック / ゲームパッド X リングの割り当て。
    /// 入力処理とは独立した設定本体。未知 id や context 不一致は load sanitize で無効化する。
    #[serde(default)]
    pub ring_shortcuts: crate::ring_shortcut::RingShortcutSettings,

    // ── レーティングフィルタ ───────────────────────────────────
    /// レーティングフィルタ (index 0 = 未評価, 1〜5 = ★の数)。
    /// 選択された星数のアイテムのみ表示。全て true = フィルタなし。
    #[serde(default = "default_rating_filter")]
    pub rating_filter: [bool; 6],

    // ── EXIF 表示フィルタ ──────────────────────────────────────
    /// 非表示にする EXIF タグ名のリスト
    #[serde(default = "default_exif_hidden_tags")]
    pub exif_hidden_tags: Vec<String>,

    // ── 同名ファイル処理 ──────────────────────────────────────────
    /// 同名の ZIP ファイルとフォルダがある場合、ZIP をスキップする
    #[serde(default = "default_true")]
    pub skip_zip_if_folder_exists: bool,
    /// 同名の動画と画像がある場合、画像をスキップする（動画サムネイルで代替）
    #[serde(default = "default_true")]
    pub skip_image_if_video_exists: bool,
    /// 同名の画像が複数拡張子で存在する場合、優先度の低いものをスキップする
    #[serde(default = "default_true")]
    pub skip_duplicate_images: bool,
    /// 画像拡張子の優先度リスト（先頭が最優先）
    #[serde(default = "default_image_ext_priority")]
    pub image_ext_priority: Vec<String>,

    // ── スライドショー ──────────────────────────────────────────
    /// スライドショーの切り替え間隔（秒）
    #[serde(default = "default_slideshow_interval")]
    pub slideshow_interval_secs: f32,
    /// スライドショーがフォルダ末尾に到達したときの動作。
    /// 新規フィールド (serde default = LoopFolder = 旧来挙動) なので移行不要。
    #[serde(default)]
    pub slideshow_end_action: SlideshowEndAction,

    // ── キャプチャ保存 ──────────────────────────────────────────
    /// Ctrl+S キャプチャ保存先。None のときは OS の Pictures/mimageviewer を使う。
    #[serde(default)]
    pub capture_output_dir: Option<PathBuf>,
    /// Ctrl+S キャプチャ保存形式。
    #[serde(default)]
    pub capture_format: crate::capture::CaptureFormat,
    /// 製本の本棚ルート。None のときは OS の Pictures/mimageviewer/books を使う。
    #[serde(default)]
    pub book_root: Option<PathBuf>,
    /// 製本でページ追加先にする本名。
    #[serde(default = "default_active_book_name")]
    pub active_book_name: String,
    /// ツールバーの本棚セクションにボタンとして固定表示する本名のリスト (表示順)。
    /// 本は本棚ルート直下のフォルダ名で識別する (UUID なし)。v2.0.0。
    #[serde(default)]
    pub pinned_books: Vec<String>,

    // ── 隠蔽加工 (Concealment) ─────────────────────────────────
    //
    // Phase 1 で導入。詳細仕様は docs/conceal-feature-plan.md §8.1。
    // 全フィールド `serde(default)` 付き ⇒ 既存の settings.db / settings.json を
    // 新コードで開いても安全 (= 欠落フィールドは型のデフォルト値で埋まる)。
    //
    /// 隠蔽加工の現在の処理タイプ。モード内 `T` キーで切替、終了後も維持。
    #[serde(default)]
    pub conceal_type: crate::conceal::ConcealType,
    /// モザイクタイルサイズの指定方式 (LongEdgeRatio / FixedPx)。
    #[serde(default)]
    pub conceal_mosaic_tile_mode: crate::conceal::TileSizeMode,
    /// モザイクタイルの境界処理 (Opaque / Translucent / MaskShape)。
    #[serde(default)]
    pub conceal_mosaic_boundary: crate::conceal::MosaicBoundary,
    /// 白塗り / 黒塗りの不透明度 (1..=100、1% 刻み)。
    #[serde(default = "crate::conceal::default_fill_opacity")]
    pub conceal_fill_opacity_percent: u8,
    /// 白塗り / 黒塗りの境界処理 (Sharp / Feathered)。
    #[serde(default)]
    pub conceal_fill_edge: crate::conceal::FillEdge,
    /// ぼかし半径 (px)。範囲 5..=100、1px 刻み。
    #[serde(default = "crate::conceal::default_blur_radius_px")]
    pub conceal_blur_radius_px: f32,
    /// ぼかしモード (AsMask / ExtendByRadius / InsideOnly)。
    #[serde(default)]
    pub conceal_blur_mode: crate::conceal::BlurMode,
    /// ぼかしの境界フェード ON/OFF (固定 8px 半径で内側へフェード)。
    #[serde(default)]
    pub conceal_blur_feather: bool,
    /// 隠蔽加工モードでのブラシ半径 (px)。初回エントリ時に画像長辺の 1/100 で初期化。
    #[serde(default)]
    pub conceal_brush_radius: f32,
    /// 隠蔽加工モードでの直線幅 (px)。初回エントリ時に画像長辺の 1/500 で初期化。
    #[serde(default)]
    pub conceal_line_width: f32,
    /// パラメータプリセット 4 スロット (`1`〜`4` キーで適用、`💾` ボタンで保存)。
    /// 各スロットは `Option<ConcealPreset>` で `None` = 空スロット。
    #[serde(default = "crate::conceal::default_conceal_presets")]
    pub conceal_presets: crate::conceal::ConcealPresetSlots,

    // ── エクスポート (Ctrl+E、Phase 6 で完成) ──────────────────
    //
    // Phase 1 ではフィールド定義 + Settings persistence までだけ用意する。
    // 実 UI と worker は Phase 6 で実装。
    //
    /// `Ctrl+E` でメタデータ (EXIF / XMP / tEXt / AI prompt) を保持して書き出すか。
    /// 既定 true。
    #[serde(default = "default_true")]
    pub export_embed_metadata: bool,
    /// ユーザーが「保存先」を元フォルダから別の場所に変更したときの記憶
    /// (= 「直前の上書き選択」の弱い記憶)。次回ダイアログの初期値で使う。
    #[serde(default)]
    pub export_last_directory: Option<PathBuf>,
    /// 元形式が書き出し非対応 (HEIC / AVIF / JXL / RAW / TIFF) のときに
    /// フォールバックする形式 (JPEG q=95 or PNG)。
    #[serde(default)]
    pub export_fallback_format: crate::conceal::ExportFallbackFormat,
    /// `Ctrl+E` ダイアログの前回出力サイズ。
    #[serde(default)]
    pub export_default_scale: crate::export_dialog::ExportScale,
    /// `Ctrl+E` ダイアログでチェックされていたバリエーション
    /// `[現在の設定, プリセット 1, 2, 3, 4]` の前回チェック状態。
    #[serde(default = "default_export_batch_selection")]
    pub export_batch_selection: [bool; 5],

    // ── フルスクリーン表示モード ────────────────────────────
    /// デフォルトのページ構成
    #[serde(default)]
    pub default_spread_mode: SpreadMode,
    /// デフォルトの連結方式
    #[serde(default)]
    pub default_reading_flow: ReadingFlow,
    /// デフォルトの横連結方向
    #[serde(default)]
    pub default_reading_direction: ReadingDirection,
    /// 見開き内の左右ページ間隔 (画面 px)。0 でページを隙間なく接続する。
    #[serde(default = "default_spread_page_gap_px")]
    pub spread_page_gap_px: u32,
    /// 縦/横連結読みで、次のページまたは次の見開きユニットまで空ける間隔 (画面 px)。
    #[serde(default = "default_continuous_reading_gap_px")]
    pub continuous_reading_gap_px: u32,
    /// フルスクリーンの倍率/フィット基準。
    #[serde(default)]
    pub fullscreen_fit_mode: FullscreenFitMode,
    /// 自動フィット時に 100% を超える拡大をしない。
    #[serde(default)]
    pub fullscreen_fit_no_upscale: bool,
    /// 自動フィット時に 100% 未満へ縮小しない。
    #[serde(default)]
    pub fullscreen_fit_no_downscale: bool,
    /// フルスクリーン左ホバーパネルで最後に開いていたタブ。
    #[serde(default)]
    pub fullscreen_left_panel_tab: FullscreenLeftPanelTab,
    /// 静止画フルスクリーン下部のページシークバーを常時表示し、画像領域から除外する。
    #[serde(default)]
    pub fullscreen_seek_bar_locked: bool,
    /// 静止画フルスクリーン右下に現在ページ / 総ページ数を常時表示する。
    #[serde(default = "default_true")]
    pub fullscreen_page_number_overlay: bool,
    /// 他アプリから mIV のメインウィンドウへ戻ったとき、フルスクリーン表示を
    /// 自動で閉じずにフルスクリーン側へフォーカスを戻す。
    #[serde(default)]
    pub fullscreen_keep_on_app_switch: bool,
    /// フルスクリーン表示中、マウス操作が止まってからカーソルを隠すまでの秒数。
    #[serde(default = "default_fullscreen_cursor_hide_delay_secs")]
    pub fullscreen_cursor_hide_delay_secs: f32,
    /// 画像フルスクリーンの大きめジャンプ (Shift+←/→) の量指定方式。
    #[serde(default)]
    pub fullscreen_jump_mode: FullscreenJumpMode,
    /// 画像フルスクリーンの割合ジャンプ (Shift+←/→) で移動するページ総数比率。
    #[serde(default = "default_fullscreen_jump_percent")]
    pub fullscreen_jump_percent: u32,
    /// 画像フルスクリーンの固定ページジャンプ (Shift+←/→) で移動する件数。
    /// `fullscreen_jump_mode == FixedPages` のときだけ使う。旧設定互換のため名前は維持する。
    #[serde(default = "default_fullscreen_fixed_jump_count")]
    pub fullscreen_fixed_jump_count: usize,
    /// 連結読みのホイール 1 ノッチあたりスクロール量 (画面サイズ比 %)。
    #[serde(default = "default_continuous_reading_wheel_scroll_percent")]
    pub continuous_reading_wheel_scroll_percent: u32,
    /// 連結読みの矢印キー / D-pad 1 回あたりスクロール量 (画面サイズ比 %)。
    #[serde(default = "default_continuous_reading_key_scroll_percent")]
    pub continuous_reading_key_scroll_percent: u32,
    /// 連結読みの左スティック最大入力時スクロール速度 (画面サイズ比 %/秒)。
    #[serde(default = "default_continuous_reading_gamepad_scroll_percent_per_sec")]
    pub continuous_reading_gamepad_scroll_percent_per_sec: u32,

    /// ZIP/PDF/対応アーカイブを一覧や起動引数/SendTo から明示的に開いたとき、
    /// ページ一覧を経由せずページを即フルスクリーンで開く。ON のときフルスクリーン中の
    /// Esc/Enter/短い右クリックは親フォルダ (一覧) へ戻り、Backspace でコンテナの
    /// ページ一覧を表示する。
    /// 既定 OFF (従来どおりページ一覧を表示)。
    #[serde(default)]
    pub auto_fullscreen_zip_pdf: bool,

    /// 旧設定互換用。読み込み時に `fullscreen_fit_mode == MarginFit` へ寄せ、
    /// 本を開いたタイミングで表示トリム Auto + ページ全体フィットへ移行する。
    #[serde(default)]
    pub margin_fit_enabled: bool,

    // ── UI テーマ (v0.7.0) ──────────────────────────────────────
    /// 背景色テーマ (System / Light / Dark)。デフォルト `System` で Windows のアプリ用色に追従。
    #[serde(default)]
    pub ui_theme: UiTheme,

    /// 初回セットアップダイアログを完了したか。
    #[serde(default)]
    pub first_setup_completed: bool,

    /// AI アップスケール / ノイズ除去の利用範囲。
    #[serde(default)]
    pub ai_feature_mode: AiFeatureMode,

    // ── ツールバー項目フィルタ（Vec が空 = セクション非表示）──
    /// ツールバーに表示する列数の選択肢
    #[serde(default = "default_toolbar_cols_items")]
    pub toolbar_cols_items: Vec<usize>,
    /// ツールバーの列セクションに「詳細」切替を表示するか
    #[serde(default = "default_true")]
    pub toolbar_cols_details_visible: bool,
    /// ツールバーに表示するアスペクト比の選択肢
    #[serde(default = "default_toolbar_aspect_items")]
    pub toolbar_aspect_items: Vec<ThumbAspect>,
    /// ツールバーに「自動」項目を表示するか (デフォルト: true)。
    /// `toolbar_aspect_items` は 7 種のバケットを管理するが、「自動」は別フラグで
    /// 制御する (UI 上は同じセクションにチェックボックスとして並ぶ)。
    #[serde(default = "default_toolbar_aspect_auto_visible")]
    pub toolbar_aspect_auto_visible: bool,
    /// ツールバー「列」セクションの表示形式 (展開 / プルダウン)。
    #[serde(default)]
    pub toolbar_cols_display: ToolbarSectionDisplay,
    /// ツールバー「比率」セクションの表示形式 (展開 / プルダウン)。
    #[serde(default)]
    pub toolbar_aspect_display: ToolbarSectionDisplay,
    /// ツールバー「ソート」セクションの表示形式 (展開 / プルダウン)。
    #[serde(default)]
    pub toolbar_sort_display: ToolbarSectionDisplay,
    /// お気に入り / タグ / 本棚セクションの表示形式 (展開 / 折りたたみ / プルダウン)。v2.0.0。
    #[serde(default)]
    pub toolbar_favorites_display: ToolbarSectionDisplay,
    #[serde(default)]
    pub toolbar_tags_display: ToolbarSectionDisplay,
    #[serde(default)]
    pub toolbar_bookshelf_display: ToolbarSectionDisplay,
    /// 折りたたみモード時の畳み状態 (true = 畳んで隠す)。v2.0.0。
    #[serde(default)]
    pub toolbar_favorites_collapsed: bool,
    #[serde(default)]
    pub toolbar_tags_collapsed: bool,
    #[serde(default)]
    pub toolbar_bookshelf_collapsed: bool,
    /// ツールバーに表示するソート順の選択肢
    #[serde(default = "default_toolbar_sort_items")]
    pub toolbar_sort_items: Vec<SortOrder>,
    /// ツールバーのセクション並び順 (v2.0.0)。空 = 既定順。
    /// `ToolbarSectionId::ordered_with_fallback` で未登録セクションを補完する。
    #[serde(default)]
    pub toolbar_section_order: Vec<ToolbarSectionId>,
    /// ツールバーに「列」セクションを表示する (v2.0.0)。
    /// 旧来は `toolbar_cols_items` が空 (かつ詳細も非表示) = 非表示だったが、統一
    /// カスタマイズで明示的な表示フラグを持たせ、空き領域右クリックの表示チェック
    /// リストから切替える。最終的な表示可否は「このフラグ AND 項目が 1 つ以上ある」。
    #[serde(default = "default_true")]
    pub show_toolbar_cols: bool,
    /// ツールバーに「比率」セクションを表示する (v2.0.0)。
    #[serde(default = "default_true")]
    pub show_toolbar_aspect: bool,
    /// ツールバーに「ソート」セクションを表示する (v2.0.0)。
    #[serde(default = "default_true")]
    pub show_toolbar_sort: bool,
    /// 「行頭に表示」= そのセクションの前で改行するセクション集合 (v2.0.0)。
    /// 自動折返し (horizontal_wrapped) に加え、ユーザーが行区切りを固定できる。
    /// 集合に入っているセクションは、その手前で必ず新しい行を始める (先頭セクションは無視)。
    #[serde(default)]
    pub toolbar_section_new_row: Vec<ToolbarSectionId>,

    // ── フォルダサムネイル ──────────────────────────────────────
    /// フォルダの代表画像を選ぶ際のソート順（デフォルト: 番号順）
    #[serde(default = "default_folder_thumb_sort")]
    pub folder_thumb_sort: SortOrder,

    /// フォルダの代表画像を探すときの最大探索階層数（デフォルト: 3）
    #[serde(default = "default_folder_thumb_depth")]
    pub folder_thumb_depth: u32,

    // ── アプリケーションで開く ──────────────────────────────────
    /// 最近使ったアプリケーション（最大3件、最新が先頭）
    #[serde(default)]
    pub recent_open_with_apps: Vec<RecentApp>,
    /// ユーザーが手動で追加したアプリケーション
    #[serde(default)]
    pub custom_open_with_apps: Vec<RecentApp>,

    // ── AI セッション設定 ────────────────────────────────────
    /// AI アップスケール: フルスクリーン表示時に有効にするか（デフォルト: false）
    #[serde(default)]
    pub ai_upscale_enabled: bool,

    /// AI アップスケール: モデルの手動オーバーライド (None = 自動判別)
    /// 値は ModelKind::as_str() の文字列（例: "realesrgan_x4plus"）
    #[serde(default)]
    pub ai_upscale_model_override: Option<String>,

    /// AI アップスケール: 先読み枚数（後方）
    #[serde(default = "default_ai_upscale_prefetch_back")]
    pub ai_upscale_prefetch_back: usize,

    /// AI アップスケール: 先読み枚数（前方）
    #[serde(default = "default_ai_upscale_prefetch_forward")]
    pub ai_upscale_prefetch_forward: usize,

    /// AI アップスケール / ノイズ除去: フルスクリーンを閉じた後も保持する結果の最大枚数。
    /// 0 または `retained_final_ai_cache_max_mib == 0` で保持を無効化する。
    #[serde(default = "default_retained_final_ai_cache_max_entries")]
    pub retained_final_ai_cache_max_entries: usize,

    /// AI アップスケール / ノイズ除去: フルスクリーンを閉じた後も保持する結果の上限 (MiB)。
    /// CPU 側 `ColorImage` の概算 RGBA8 バイト数で管理する。
    #[serde(default = "default_retained_final_ai_cache_max_mib")]
    pub retained_final_ai_cache_max_mib: u64,

    /// AI アップスケール: スキップしきい値（この値以上の画像はスキップ）
    ///
    /// 旧形式 (単一値)。読み書きは `ai_upscale_limit()` / `ai_upscale_size_limit` 経由に
    /// 移行済みで、このフィールドは `ai_upscale_size_limit` が無い旧設定の読み替え元と
    /// してのみ参照される (旧バージョンへの downgrade 互換のため当面残す)。
    #[serde(default = "default_ai_upscale_skip_px")]
    pub ai_upscale_skip_px: u32,

    /// AI ノイズ除去: スキップしきい値（この値以上の画像はスキップ）
    ///
    /// 旧形式 (単一値)。扱いは `ai_upscale_skip_px` と同じ。
    #[serde(default = "default_ai_denoise_skip_px")]
    pub ai_denoise_skip_px: u32,

    /// AI アップスケール: 処理対象サイズ上限 (長辺 x 短辺、どちらも未満なら処理)。
    /// `None` = 新フィールド未保存の旧設定。旧 `ai_upscale_skip_px` の値 `N` を
    /// `N x N` として読み替える (`ai_upscale_limit()` 参照)。
    #[serde(default)]
    pub ai_upscale_size_limit: Option<crate::ai::upscale::AiProcessSizeLimit>,

    /// AI ノイズ除去: 処理対象サイズ上限。`None` の扱いは `ai_upscale_size_limit` と同じ。
    #[serde(default)]
    pub ai_denoise_size_limit: Option<crate::ai::upscale::AiProcessSizeLimit>,

    /// AI バックエンド (Execution Provider グループ)
    /// None = DirectML (デフォルト)、"directml" / "tensorrt" / "cpu"
    /// 値は AiBackend::as_str() の文字列。バックエンド切り替えはアプリ再起動が必要。
    #[serde(default)]
    pub ai_backend: Option<String>,

    // 注: ai_tensorrt_fp16 フィールドは廃止。FP16 はランタイム側で常時 ON
    // (画質劣化は知覚不能、1.5-2x 高速化のメリットが大きい)。古い settings.json に
    // 残っているフィールドは serde の default で無視される。

    // ── グローバルプリセット ──────────────────────────────────────
    /// グローバルプリセット (0キー)。全フォルダ共通の補正設定。
    #[serde(default)]
    pub global_preset: crate::adjustment::AdjustParams,

    // ── 保存スロット ──────────────────────────────────────────
    /// 保存スロット (10個)。名前付きで保存した補正設定。
    #[serde(default)]
    pub preset_slots: crate::adjustment::PresetSlots,

    // ── フォルダ側サイドカー ───────────────────────────────────
    /// 補正・消しゴムマスク設定をフォルダごとのサイドカーファイル
    /// (`mimageviewer.dat`、隠し+システム属性) にバックアップする。
    /// OFF 時は読み書き両方スキップ (既存の `.dat` は削除しない)。
    #[serde(default = "default_true")]
    pub sidecar_backup_enabled: bool,

    /// タグをフォルダ側 `mimageviewer.dat` にバックアップする。
    /// 既定 OFF。中央 `tags.db` が正本で、サイドカーは opt-in の復元補助。
    #[serde(default)]
    pub tag_sidecar_backup_enabled: bool,

    // ── Susie プラグイン (v0.7.0) ──────────────────────────────
    /// Susie 画像プラグイン機能全体の ON/OFF (デフォルト: true、ワーカー exe が無い環境では自動的に無効化される)。
    #[serde(default = "default_true")]
    pub susie_enabled: bool,

    /// Susie プラグインを複数プロセスで並列実行する (デフォルト: true)。
    /// 古いプラグインで一時ファイル衝突・INI の race 書き込みが疑われる場合は false にして
    /// プール数を 1 に固定し、問題プラグインの切り分けを可能にする。
    #[serde(default = "default_true")]
    pub susie_allow_parallel: bool,

    // ── タスクトレイ常駐 (v0.9) ──────────────────────────────────
    /// 閉じるボタン [×] でウィンドウを閉じる代わりにタスクトレイに常駐する。
    /// notify-rs によるファイル監視を継続し、次回起動時の再スキャン負荷を回避する。
    /// OFF (既定) では従来どおり閉じるボタンでプロセス終了。
    #[serde(default)]
    pub minimize_to_tray_on_close: bool,

    /// タスクトレイに常駐している間 (= ウィンドウ非表示中) にバックグラウンドインデクサ
    /// (初回スキャン + notify-rs 経由の ingest) を一時停止する。ウィンドウを開き直すと
    /// 自動的に再開し、溜まっていた notify-rs イベントを順次処理する。
    /// OFF (既定) でも、非表示中は `GlobalIoSemaphore` の並列度を自動で 1 に絞ることで
    /// 他アプリへの I/O 負荷を抑える。
    #[serde(default)]
    pub pause_indexer_while_minimized: bool,

    /// レーティング (★) を XMP `xmp:Rating` としてファイル本体にも書き込むか。
    /// ON (opt-in) にすると F1〜F6 でファイル移動後もレーティングが保持され、Lightroom /
    /// Windows エクスプローラーの「評価」カラムとも互換性がある。代わりにファイル本体が
    /// 書き換わる (更新日時が変わる)。対応形式は JPEG / PNG / WebP のみ。
    /// デフォルト OFF — 「通常は非破壊」というアプリの基本方針に沿わせる。
    #[serde(default)]
    pub write_rating_to_xmp: bool,

    // ── バージョン更新通知 ────────────────────────────────────────
    /// 起動時 + 定期的に GitHub Releases API を叩いて新バージョンを確認するか。
    /// 既定 ON (オフライン環境では silent fail するので副作用なし)。
    #[serde(default = "default_true")]
    pub update_check_enabled: bool,

    /// ユーザーが「このバージョンの通知は表示しない」を選んだ tag (例: "v0.8.2")。
    /// チェック結果がここと一致するなら通知バッジを出さない。
    /// 新バージョンが更にリリースされて tag が変われば再度通知する。
    #[serde(default)]
    pub update_check_dismissed_version: Option<String>,

    // ── 開発者 / 診断 ─────────────────────────────────────────────
    /// 性能ログ (perf_events.jsonl、構造化イベントログ) を記録するか。
    /// 既定 OFF。フレーム単位のイベントを大量に吐くため常時 ON にはしない。
    /// 「動作が重い / カクつく」系の不具合をサポートに調べてもらうときだけ ON にする。
    /// 起動時に 1 度だけ読まれるので、変更は次回起動から有効。
    /// (`--perf-log` 引数は従来どおり起動直後から全イベントを記録する開発者向け経路。)
    #[serde(default)]
    pub perf_log_enabled: bool,

    // ── 動画インライン再生 ────────────────────────────────────────
    /// 動画再生時の既定音量 (線形ゲイン 0.0..+18dB 相当)。1.0 を超える値は
    /// 音声ポンプ側で pre-limiter boost として扱う。
    #[serde(default = "default_video_volume")]
    pub video_volume: f64,
    /// 動画再生速度。HUD の速度ボタンから変更され、動画切替 / アプリ再起動後も維持する。
    #[serde(default = "default_video_playback_speed")]
    pub video_playback_speed: f64,
    /// 旧自動再生設定 (bool)。現在の再生開始挙動では参照せず、設定ファイル互換のため保持する。
    #[serde(default)]
    pub video_autoplay: bool,
    /// 旧動画自動再生ポリシー。現在の UI/再生開始挙動では参照せず、読み込み時に互換正規化する。
    #[serde(default)]
    pub video_autoplay_mode: VideoAutoplayMode,
    /// 終端到達時に先頭から再生を繰り返すか (旧 v0.8.x 以前)。
    /// `video_loop_mode` がデフォルト値 (Off) のときだけ参照され、true なら Full に昇格する。
    /// 新ビルドでは `Settings::save()` の中で `mode != Off` から導出して書き戻すので、
    /// 個別 toggle 経路ではこのフィールドを意識する必要はない。
    #[serde(default)]
    pub video_loop: bool,
    /// 動画フルスクリーン時のループ再生モード (Off / 全体 / チャプター / ブックマーク)。
    /// Phase 0.10 で `video_loop: bool` から拡張。`Settings::sanitize()` の中で旧 bool から
    /// マイグレーションされる。
    #[serde(default)]
    pub video_loop_mode: VideoLoopMode,
    /// 動画フルスクリーン時の連続再生モード。既存ループ設定とは排他で、
    /// ON の間は実再生ループを無効化し、連続再生を優先する。
    #[serde(default)]
    pub video_continuous_mode: crate::video::VideoContinuousMode,
    /// 起動時にミュートで開始するか (オフィス環境などでの保険)。
    #[serde(default)]
    pub video_start_muted: bool,
    /// HUD のミュートボタン / M キーで最後に選んだ動画ミュート状態。
    /// `video_start_muted` は起動時だけ true 方向に効く安全スイッチで、こちらは
    /// 起動後の動画切替と次回起動へ引き継ぐ現在のミュート状態。
    #[serde(default)]
    pub video_muted: bool,
    /// 動画ファイルごとの最終再生位置 (絶対パス → 秒)。
    /// `VideoPlayer::open` 時に自動 resume、5 秒ごと + drop 時に保存。
    /// 動画末尾近く (残り 5 秒以内) は 0 にリセットして "次回最初から" の挙動。
    #[serde(default)]
    pub video_resume_positions: std::collections::HashMap<String, f64>,
    /// 一覧から明示的に動画を開いたとき、保存済み resume 位置を使わず先頭から開くか。
    /// (v0.9.0 リリース済みの bool。位置復元マトリクスの「動画 × 一覧から開く」セルの保存先を
    /// 兼ねる。互換のため enum 化せず bool のまま残す。アクセスは `Settings::video_open_resume`
    /// / `set_video_open_resume` 経由)。
    #[serde(default)]
    pub video_grid_open_starts_from_beginning: bool,
    /// 動画を Ctrl+↑↓ / ホイール / キーで移動したとき、続きから再生するか最初からか。
    /// (位置復元マトリクス「動画 × Ctrl+↑↓ 移動」。既定 = 続きから = 従来挙動)
    #[serde(default)]
    pub video_nav_resume: ResumeMode,
    /// ZIP/PDF を一覧や起動引数/SendTo から開いたとき、保存済み読書位置 (続き)
    /// から開くか先頭からか。
    /// (位置復元マトリクス「ZIP/PDF × 明示オープン」。既定 = 続きから = 従来挙動)
    #[serde(default)]
    pub book_open_resume: ResumeMode,
    /// ZIP/PDF を Ctrl+↑↓ フォルダナビで移動したとき、続きから開くか先頭からか。
    /// (位置復元マトリクス「ZIP/PDF × Ctrl+↑↓ 移動」。既定 = 先頭から = 従来「フォルダ先頭着地」)
    #[serde(default = "default_resume_from_start")]
    pub book_nav_resume: ResumeMode,
    /// フルスクリーンで開いた本 (フォルダ / ZIP / PDF) を読書履歴に記録するか。
    #[serde(default = "default_true")]
    pub reading_history_enabled: bool,
    /// 読書履歴の保持件数。既定 1000、上限 1000。
    #[serde(default = "default_reading_history_limit")]
    pub reading_history_limit: usize,
    /// ハードウェアデコードを利用するか (Windows D3D11VA)。D3D11VA 非対応 codec は
    /// SW で再生し、D3D11VA 対応 codec の HW 初期化 / open 失敗はエラーとして扱う。
    /// HEVC / 4K 動画の CPU 負荷を大きく下げるため既定 ON。GPU ドライバの不具合等で
    /// HW 経路だけ問題が出る場合は環境設定から OFF に切り替えて回避できる。
    #[serde(default = "default_true")]
    pub video_hw_decode: bool,
    /// インターレース動画のデインターレース処理。
    /// Auto は FFmpeg frame の interlaced flag または stream field_order が
    /// interlaced を示す場合に bwdif を適用する。
    #[serde(default)]
    pub video_deinterlace: VideoDeinterlaceMode,
    /// 動画グリッドサムネに、同名ファイル名の画像 (= sidecar、例 movie.mp4 の隣の
    /// movie.jpg) があれば優先採用するか。Phase 5.3 で導入。
    /// 既存ユーザー (= 過去の動作と整合) のため既定 true。OFF にすると Windows Shell
    /// 経由の動画自身のデフォルトサムネのみが使われる。
    /// ピン留めサムネ (= Phase 5.4.1 で実装予定) は本設定とは独立で常に最優先。
    #[serde(default = "default_true")]
    pub video_thumb_use_sidecar_image: bool,
    /// 動画タイルモードの列数 (Phase 6.D)。タイル中 Ctrl+Wheel で
    /// 4/6/10/16/20/26/30 のいずれかに切替可能。値が範囲外なら 10 にクランプ。
    #[serde(default = "default_video_tile_columns")]
    pub video_tile_columns: usize,
    /// 動画フルスクリーンを「メインウィンドウ内ウィンドウ再生」(in-window) で
    /// 行うか。false = 従来のモニタ全面フルスクリーン。動画 HUD のウィンドウ /
    /// 全画面トグルボタンで切り替え、ここに永続化する。
    #[serde(default)]
    pub video_in_window_mode: bool,
    /// 画像・動画ビューアをメイン一覧から分離した別ウィンドウで開くモード。
    /// Phase 1 では永続設定とキー操作だけを用意し、実際の detached host は後続で接続する。
    #[serde(default)]
    pub detached_viewer_enabled: bool,
    /// 別ウィンドウビューアの前回位置・サイズ。静止画 egui viewport と native 動画
    /// top-level window の両方で共有する。
    #[serde(default)]
    pub detached_viewer_window_placement: Option<DetachedViewerWindowPlacement>,

    // ── VST3 プラグイン処理 (v0.9.0+) ──
    //
    // 動画音声を VST3 プラグインで加工 (LUFS 測定 / EQ 等)。デフォルト OFF。
    // 詳細は docs/vst3-integration.md 参照。
    /// VST3 プラグイン処理を有効にするか。OFF (= デフォルト) なら bridge プロセスを
    /// 起動せず、音声経路もパススルー (オーバーヘッドゼロ)。
    #[serde(default)]
    pub vst3_enabled: bool,
    /// VST3 プラグインのチェーン (= 適用順序の配列)。配列の先頭から順番に音声を通す。
    /// 各エントリは個別に bypass トグル可能 (ロード状態は維持しつつスルー)。
    /// 起動時に自動ロードされる (= ユーザーが管理ウィンドウで都度設定し直す必要がない)。
    #[serde(default)]
    pub vst3_plugins: Vec<Vst3PluginEntry>,
    /// (deprecated) v0.9.0 開発初期版で使われていた単一プラグインパス。
    /// 読み込み時に `vst3_plugins` に migration するための互換フィールド。
    /// 一度 migrate されると settings.json への次回書き込みで消える。
    #[serde(default)]
    pub vst3_plugin_path: Option<String>,
    /// (deprecated) 同上。`Vst3PluginEntry::state` に migration する。
    #[serde(default)]
    pub vst3_plugin_state: Option<String>,
    /// プラグイン GUI の表示状態。V キー / 管理ウィンドウのトグル状態を永続化する。
    /// 全プラグイン共通の一斉トグル状態として扱う (個別表示の覚え書きはしない)。
    #[serde(default = "default_true")]
    pub vst3_gui_visible: bool,
    /// 動画フルスクリーン再生中、動画を右上 1/4 に縮小表示する (= プラグイン作業領域確保用)。
    /// false (= 既定): 動画はフルスクリーン全体を使う。
    /// true: 動画を右上 1/4 (幅・高さ各 1/2 = 面積 1/4) に縮小、左下 3/4 はプラグイン GUI 用に空く。
    #[serde(default)]
    pub vst3_video_compact: bool,
    /// 動画再生中 VST3 パネルの左上位置 (viewport/native overlay 内の logical points)。
    /// 解像度・DPI・モニター構成変更で画面外になる場合は、表示時に画面内へ clamp する。
    #[serde(default)]
    pub vst3_panel_pos: Option<[f32; 2]>,
    #[serde(default)]
    pub vst3_chain_slots: Vst3ChainPresetSlots,

    // ── 音量ノーマライズ (v0.10+) ──
    //
    // 動画ごとに -14 LUFS 相当に揃える音量自動調整。グローバル ON/OFF。
    // 測定値は別 DB (`audio_normalize.db`) にファイル単位でキャッシュ。
    /// グローバル ON/OFF。OFF (既定) なら gain は常に 1.0。
    #[serde(default)]
    pub audio_normalize_enabled: bool,
    /// ターゲット音量 (LUFS の千分の一単位、整数で持つ)。
    /// 既定 -14000 (= -14.000 LUFS、YouTube/Spotify 相当)。
    /// 直接編集される可能性を考慮し、使用時は `clamped_audio_normalize_target_lufs_milli()` で
    /// `-60_000..=0` にクランプする。
    #[serde(default = "default_audio_normalize_target_lufs_milli")]
    pub audio_normalize_target_lufs_milli: i32,

    // ── settings.json 内部メタ ──
    /// 直近にこの settings.json を書き込んだ mIV のバージョン。
    /// `Settings::load` でアプリの現バージョンと比較し、変わっていれば
    /// 旧版のスナップショットを `settings.json.preupgrade-v<old>` として
    /// 退避する (= バージョン跨ぎの安全網)。
    /// 新規 (= 過去に保存履歴なし) や旧コードで保存された JSON では None。
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

/// VST3 プラグインチェーンの 1 エントリ。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Vst3PluginEntry {
    /// .vst3 ファイル / バンドルディレクトリへの絶対パス。
    pub path: String,
    /// true ならこのスロットを音声処理でスキップ (= ロード済みのままパススルー)。
    /// false なら通常通り `IAudioProcessor::process` を呼ぶ。
    #[serde(default)]
    pub bypass: bool,
    /// プラグイン側の現在状態 (= IComponent::getState chunk) を Base64 エンコードしたもの。
    /// 終了時に bridge から取得し、次回起動時に復元する (= EQ カーブ等が保持される)。
    /// v0.9.0 では bridge 側 query_state プロトコル未実装のため未使用 (将来拡張)。
    #[serde(default)]
    pub state: Option<String>,
    /// ユーザーが個別に GUI × で閉じた状態を永続化する (= 2026-04 ユーザー要望)。
    /// true: 起動後の VST 一括表示 (= VST ボタン / `set_all_guis_visible(true)`) で
    /// このスロットの GUI は表示されない。`show_slot_gui` の明示呼び出し
    /// (= パネルの「GUI」ボタン) で false に戻る。
    /// false (= 既定): 通常通り表示候補に含まれる。
    #[serde(default)]
    pub user_hidden: bool,
    /// プラグイン GUI ウィンドウの **デスクトップ位置** (画面座標、左上点)。
    /// 終了時 / VST3 OFF / chain rebuild の直前に `GetWindowRect` で取得して保存。
    /// 次回起動時に CreateWindowExW のデフォルト位置の代わりに使う (= 2026-05 ユーザー要望
    /// 「ウィンドウ位置を復元してほしい」)。
    /// None: 過去保存なし (= 初回 / 復元情報を破棄したい時)、デフォルト中央配置を使う。
    #[serde(default)]
    pub gui_pos: Option<(i32, i32)>,
    /// プラグイン GUI ウィンドウの **外枠サイズ** (= title bar 込みの outer rect)。
    /// resizable プラグインがユーザーリサイズで広げた状態を覚える用。非 resizable
    /// プラグインの場合はプラグイン要求値が優先されるので保存しても基本使われない
    /// (記録は残しておくが復元時の参照は resizable プラグインのみに限定)。
    #[serde(default)]
    pub gui_size: Option<(u32, u32)>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Vst3ChainPresetSlot {
    pub name: String,
    #[serde(default)]
    pub plugins: Vec<Vst3PluginEntry>,
    #[serde(default = "default_true")]
    pub gui_visible: bool,
    #[serde(default)]
    pub video_compact: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Vst3ChainPresetSlots {
    pub slots: [Option<Vst3ChainPresetSlot>; 10],
}

/// 動画音量の既定値。0dB (= boost なし)。
pub const VIDEO_VOLUME_DEFAULT: f64 = 1.0;
/// 動画音量フェーダーのミュート端。UI では -∞dB と表示し、内部ゲインは 0.0 にする。
pub const VIDEO_VOLUME_MUTE_DB: f64 = -80.0;
/// 動画音量の上限。+18dB は約 794% の手動 boost。
pub const VIDEO_VOLUME_MAX_DB: f64 = 18.0;
/// 動画音量の上限を線形ゲインで保持する。既存 settings.json 互換のため保存値は線形。
pub const VIDEO_VOLUME_MAX: f64 = 7.943_282_347_242_816;
/// HUD / 設定 UI の dB フェーダー目盛り。隣接目盛り間を線形補間する。
pub const VIDEO_VOLUME_FADER_DB_MARKS: [f64; 10] = [
    -80.0, -60.0, -40.0, -20.0, -10.0, -5.0, 0.0, 6.0, 12.0, 18.0,
];
/// キーボード音量変更は表示目盛り間をさらにこの数で分割する。
pub const VIDEO_VOLUME_KEY_STEPS_PER_FADER_MARK: usize = 4;

pub fn clamp_video_volume(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, VIDEO_VOLUME_MAX)
    } else {
        VIDEO_VOLUME_DEFAULT
    }
}

pub fn video_volume_db_to_linear(db: f64) -> f64 {
    if !db.is_finite() {
        return VIDEO_VOLUME_DEFAULT;
    }
    let db = db.clamp(VIDEO_VOLUME_MUTE_DB, VIDEO_VOLUME_MAX_DB);
    if db <= VIDEO_VOLUME_MUTE_DB {
        0.0
    } else {
        clamp_video_volume(10.0_f64.powf(db / 20.0))
    }
}

pub fn video_volume_linear_to_db(value: f64) -> f64 {
    let value = clamp_video_volume(value);
    if value <= 0.0 {
        VIDEO_VOLUME_MUTE_DB
    } else {
        (20.0 * value.log10()).clamp(VIDEO_VOLUME_MUTE_DB, VIDEO_VOLUME_MAX_DB)
    }
}

pub fn video_volume_db_to_fader_pos(db: f64) -> f64 {
    let db = if db.is_finite() {
        db.clamp(VIDEO_VOLUME_MUTE_DB, VIDEO_VOLUME_MAX_DB)
    } else {
        0.0
    };
    let marks = &VIDEO_VOLUME_FADER_DB_MARKS;
    if db <= marks[0] {
        return 0.0;
    }
    let last = marks.len() - 1;
    if db >= marks[last] {
        return 1.0;
    }
    for i in 0..last {
        let lo = marks[i];
        let hi = marks[i + 1];
        if db >= lo && db <= hi {
            let local = (db - lo) / (hi - lo);
            return (i as f64 + local) / last as f64;
        }
    }
    video_volume_db_to_fader_pos(0.0)
}

pub fn video_volume_fader_pos_to_db(pos: f64) -> f64 {
    let pos = if pos.is_finite() {
        pos.clamp(0.0, 1.0)
    } else {
        video_volume_db_to_fader_pos(0.0)
    };
    let marks = &VIDEO_VOLUME_FADER_DB_MARKS;
    let last = marks.len() - 1;
    if pos <= 0.0 {
        return marks[0];
    }
    if pos >= 1.0 {
        return marks[last];
    }
    let scaled = pos * last as f64;
    let i = scaled.floor() as usize;
    let local = scaled - i as f64;
    marks[i] + (marks[i + 1] - marks[i]) * local
}

pub fn video_volume_linear_to_fader_pos(value: f64) -> f64 {
    video_volume_db_to_fader_pos(video_volume_linear_to_db(value))
}

pub fn video_volume_fader_pos_to_linear(pos: f64) -> f64 {
    video_volume_db_to_linear(video_volume_fader_pos_to_db(pos))
}

pub fn step_video_volume_by_fader_key_step(value: f64, direction: i32) -> f64 {
    if direction == 0 {
        return clamp_video_volume(value);
    }
    let total_steps =
        (VIDEO_VOLUME_FADER_DB_MARKS.len() - 1) * VIDEO_VOLUME_KEY_STEPS_PER_FADER_MARK;
    let scaled = video_volume_linear_to_fader_pos(value) * total_steps as f64;
    let step = if direction > 0 {
        (scaled + 1.0e-9).floor() as i32 + 1
    } else {
        (scaled - 1.0e-9).ceil() as i32 - 1
    };
    let step = step.clamp(0, total_steps as i32) as usize;
    video_volume_fader_pos_to_linear(step as f64 / total_steps as f64)
}

pub fn format_video_volume_db(value: f64) -> String {
    let db = video_volume_linear_to_db(value);
    if db <= VIDEO_VOLUME_MUTE_DB + 0.05 {
        "-∞ dB".to_string()
    } else if db.abs() < 0.05 {
        "0 dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

pub fn clamp_fullscreen_cursor_hide_delay_secs(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(
            FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS,
            FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS,
        )
    } else {
        FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS
    }
}

/// 音量ノーマライズの target_lufs_milli 既定値 (= -14.000 LUFS、YouTube/Spotify 相当)。
fn default_audio_normalize_target_lufs_milli() -> i32 {
    -14_000
}

/// 音量ノーマライズの target_lufs_milli 範囲 (= -60 LUFS 〜 0 LUFS)。
/// 設定ファイル直接編集で異常値が入っても DB キーが無限に増える事故を防ぐためクランプする。
pub const AUDIO_NORMALIZE_TARGET_LUFS_MILLI_MIN: i32 = -60_000;
pub const AUDIO_NORMALIZE_TARGET_LUFS_MILLI_MAX: i32 = 0;

/// 動画タイルモード列数の候補 (Phase 6.D)。
/// 4 は縦長ディスプレイ向け (6 列でもタイルが小さいため)。
pub const VIDEO_TILE_COLUMN_CANDIDATES: &[usize] = &[4, 6, 10, 16, 20, 26, 30];

/// タイルサムネイルを新規抽出するときの固定幅 (px)。高さは動画のアスペクト比から導出。
/// 列数・モニター解像度・どのモニターで再生するかに依らず常にこの幅で抽出・保存するため、
/// キャッシュは「動画 × 絶対 PTS」で 1 行に集約され、列数を切り替えても解像感が混ざらない。
/// 640px の根拠: 6 列@4K (= 従来の常用抽出値) と一致するので既存キャッシュを無駄にせず、
/// 縦長モニターでの 4 列 (表示 ~540px) も縮小表示でシャープ。横長 4K での 4 列
/// (表示 ~960px) だけ ~1.5x の拡大描画になるが、スクラブ一覧用途として許容する。
pub const VIDEO_TILE_EXTRACT_WIDTH: u32 = 640;
/// ホイール動画ナビゲーション中に表示する resume プレビューの抽出幅。
/// タイル一覧より大きめにしつつ、4K/8K 原寸 RGBA を overlay にアップロードして
/// GPU/VRAM 圧迫を再発させない上限にする。
pub const VIDEO_RESUME_PREVIEW_EXTRACT_WIDTH: u32 = 1280;

fn default_video_tile_columns() -> usize {
    10
}

/// 旧動画自動再生ポリシー。設定ファイル互換用に残す。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoAutoplayMode {
    /// 一覧から明示的に開いたときだけ再生する。フルスクリーン中の移動では一時停止。
    #[default]
    Off,
    /// 旧設定互換。読み込み時に Off に正規化する。
    OnlyFromGrid,
    /// 常に自動再生 (= 旧 video_autoplay=true 相当)。
    Always,
}

impl VideoAutoplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "一覧から開いたときだけ再生する",
            Self::OnlyFromGrid => "一覧から開いたときだけ再生する",
            Self::Always => "常に自動再生する",
        }
    }
    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Always]
    }
}

/// スライドショーがフォルダ末尾に到達したときの動作。
///
/// `LoopFolder` (既定) はフォルダ内で先頭の静止画系へ折り返す (旧来挙動)。
/// `NextFolder` は手動 Ctrl+↓ と同じ skip-walk で次フォルダへ進む (ただし判定述語は
/// 静止画ありに限定し、動画のみ・画像なしフォルダは飛ばす。skip_limit 内に静止画
/// フォルダが無ければ停止)。`Stop` は末尾で停止する。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SlideshowEndAction {
    #[default]
    LoopFolder,
    NextFolder,
    Stop,
}

impl SlideshowEndAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::LoopFolder => "フォルダ内でループ",
            Self::NextFolder => "次のフォルダへ進む",
            Self::Stop => "最後で停止",
        }
    }
}

/// 動画フルスクリーン時のループ再生モード。
///
/// Off → Full → Chapter → Bookmark → Off の 4 段階サイクル。
/// チャプター / ブックマークが空の動画では当該段階は cycle でスキップされ、
/// 既に当該モードのまま当該データ無しの動画に移動した場合は Full と等価に振る舞う
/// (= ボタンの見た目はユーザー意図のモードを維持しつつ、実効的な loop は全体ループ)。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoLoopMode {
    #[default]
    Off,
    Full,
    Chapter,
    Bookmark,
}

impl VideoLoopMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "ループしない",
            Self::Full => "全体ループ",
            Self::Chapter => "チャプターループ",
            Self::Bookmark => "ブックマークループ",
        }
    }
    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Full, Self::Chapter, Self::Bookmark]
    }
}

/// `current` の **次** の loop モードを返す。`has_ch` / `has_bm` が false の段階は
/// 飛ばす。Off → Full → Chapter → Bookmark → Off の循環順。
///
/// 「現在モードが無効」のケース (動画移動でモードを保持しているが新動画では当該
/// データが無い) は、循環順の **次** の有効モードを返す:
/// - `(Chapter, has_ch=false, has_bm=true)` → Bookmark
/// - `(Chapter, has_ch=false, has_bm=false)` → Off (Bookmark もスキップ)
/// - `(Bookmark, has_bm=false, ...)` → Off (Bookmark の次は Off で常に有効)
pub fn cycle_loop_mode(current: VideoLoopMode, has_ch: bool, has_bm: bool) -> VideoLoopMode {
    let order = [
        VideoLoopMode::Off,
        VideoLoopMode::Full,
        VideoLoopMode::Chapter,
        VideoLoopMode::Bookmark,
    ];
    let mut idx = order.iter().position(|m| *m == current).unwrap_or(0);
    for _ in 0..order.len() {
        idx = (idx + 1) % order.len();
        match order[idx] {
            VideoLoopMode::Chapter if !has_ch => continue,
            VideoLoopMode::Bookmark if !has_bm => continue,
            m => return m,
        }
    }
    VideoLoopMode::Off
}

/// 「ユーザーが選んでいる mode」と「現動画で実際に効く mode」を分離する。
/// チャプター/ブックマーク無しの動画では Chapter/Bookmark は Full に降格される。
/// HUD 表示には設定値 (= `mode`) を使い、再生挙動には effective を使う。
pub fn effective_loop_mode(mode: VideoLoopMode, has_ch: bool, has_bm: bool) -> VideoLoopMode {
    match mode {
        VideoLoopMode::Chapter if !has_ch => VideoLoopMode::Full,
        VideoLoopMode::Bookmark if !has_bm => VideoLoopMode::Full,
        other => other,
    }
}

/// `starts` (finite + nonneg + sort + dedup 前提) の中で、`t` 以下の最大値を返す。
/// ループ境界の「現在区間の開始秒」を求めるのに使う。
pub fn start_at(starts: &[f64], t: f64) -> Option<f64> {
    starts.iter().rev().copied().find(|s| *s <= t)
}

/// `starts` (finite + nonneg + sort + dedup 前提) の中で、`v` より大きい最小値を返す。
/// ループ境界の「次の境界 = 現在区間の end」を求めるのに使う。
pub fn first_boundary_after(starts: &[f64], v: f64) -> Option<f64> {
    starts.iter().copied().find(|s| *s > v)
}

/// 境界 tick の判定結果。`tick_native_video_loop_boundary` から呼ばれる純関数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoundaryDecision {
    /// serial 変化 / 巻き戻り → seek を起こさず baseline (last_loop_pos と
    /// loop_target_secs) を更新するだけ。
    BaselineUpdate,
    /// 境界跨ぎ → `seek_to` (= prev_pos が属する区間の開始秒) へ seek。
    Loop { seek_to: f64 },
    /// 何もしない (= まだ境界手前)。
    Continue,
}

/// 境界 tick の判定純関数。`prev_pos` 側の区間で `prev_start` / `next_boundary`
/// を計算する前提 (= 跨いだ瞬間 cur が次区間に入っていても見逃さない)。
///
/// `tol` は境界手前の小マージン (フレーム間隔吸収用、20ms 程度)。`prev_pos < boundary`
/// は厳密判定 (左辺は tol 引かない) — `prev_pos=9.99`, `boundary=10.00` で `prev_pos < boundary - tol`
/// を採ると false になり境界跨ぎを見逃すため。
pub fn decide_boundary_action(
    prev_pos: f64,
    prev_serial: u64,
    cur: f64,
    serial: u64,
    prev_start: f64,
    next_boundary: Option<f64>,
    tol: f64,
) -> BoundaryDecision {
    if serial != prev_serial || cur < prev_pos {
        return BoundaryDecision::BaselineUpdate;
    }
    // 「前進していること」だけを微小 epsilon で確認する (Codex P1 第10ラウンド):
    // 旧 `cur >= prev_pos + tol * 0.5` (= tol/2 = 10ms) は厳しすぎて、低速再生 (0.5x)
    // や高頻度 tick で 1 tick 分の進行が 10ms 未満になると境界を見逃した。
    // FORWARD_PROGRESS_EPSILON 超の前進があれば通常再生・低速再生・stutter 後の進行の
    // いずれでも検出でき、pause/scrub の `cur == prev_pos` だけが除外される
    // (= 誤発火防止としては十分)。strict `>` 比較なので、ちょうど 1us の進行は
    // 不発になるが実用上問題ない (clock 解像度より十分粗い)。
    const FORWARD_PROGRESS_EPSILON: f64 = 1.0e-6;
    let boundary = next_boundary.unwrap_or(f64::INFINITY);
    if prev_pos < boundary && cur >= boundary - tol && cur > prev_pos + FORWARD_PROGRESS_EPSILON {
        return BoundaryDecision::Loop {
            seek_to: prev_start,
        };
    }
    BoundaryDecision::Continue
}

/// 動画再生時のデインターレース設定。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoDeinterlaceMode {
    /// デコードフレームまたはストリームが interlaced と示しているときだけ bwdif を適用する。
    #[default]
    Auto,
    /// 常に bwdif を適用する。メタデータが壊れている素材向け。
    On,
    /// デインターレースしない。
    Off,
}

impl VideoDeinterlaceMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自動",
            Self::On => "常に有効",
            Self::Off => "無効",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Auto, Self::On, Self::Off]
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn force_all_frames(self) -> bool {
        matches!(self, Self::On)
    }
}

fn default_video_volume() -> f64 {
    VIDEO_VOLUME_DEFAULT
}

fn default_video_playback_speed() -> f64 {
    1.0
}

/// グリッド列数の最小値
pub const MIN_GRID_COLS: usize = 1;
/// グリッド列数の最大値
pub const MAX_GRID_COLS: usize = 10;
pub const FULLSCREEN_JUMP_PERCENT_MIN: u32 = 1;
pub const FULLSCREEN_JUMP_PERCENT_MAX: u32 = 100;
pub const FULLSCREEN_JUMP_PERCENT_DEFAULT: u32 = 10;
pub const FULLSCREEN_FIXED_JUMP_MIN: usize = 1;
pub const FULLSCREEN_FIXED_JUMP_MAX: usize = 100;
pub const FULLSCREEN_FIXED_JUMP_DEFAULT: usize = 10;
pub const FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS: f32 = 0.1;
pub const FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS: f32 = 5.0;
pub const FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS: f32 = 1.0;
pub const RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MIN: usize = 0;
pub const RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MAX: usize = 20;
pub const RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_DEFAULT: usize = 10;
pub const RETAINED_FINAL_AI_CACHE_MAX_MIB_MIN: u64 = 0;
pub const RETAINED_FINAL_AI_CACHE_MAX_MIB_MAX: u64 = 8192;
pub const RETAINED_FINAL_AI_CACHE_MAX_MIB_DEFAULT: u64 = 512;

fn default_grid_cols() -> usize {
    4
}
fn default_details_sort_ascending() -> bool {
    true
}
fn default_prefetch_back() -> usize {
    4
}
fn default_prefetch_forward() -> usize {
    12
}
fn default_folder_skip_limit() -> usize {
    5
}
fn default_thumb_px() -> u32 {
    512
}

fn default_text_preview_scale() -> u32 {
    1
}
fn default_thumb_quality() -> u8 {
    75
}
fn default_cache_threshold_ms() -> u32 {
    25
}
fn default_cache_size_threshold_bytes() -> u64 {
    2_000_000
}
fn default_true() -> bool {
    true
}
fn default_active_book_name() -> String {
    crate::books::DEFAULT_BOOK_NAME.to_string()
}
fn default_quick_folder_slots() -> [Option<PathBuf>; 2] {
    [None, None]
}
fn default_quick_folder_recent_folders() -> [Vec<PathBuf>; 2] {
    [Vec::new(), Vec::new()]
}
fn default_fullscreen_cursor_hide_delay_secs() -> f32 {
    FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS
}
pub(crate) fn default_folder_tree_pane_width_ratio() -> f32 {
    0.22
}
fn default_thumb_prev_pages() -> u32 {
    2
}
fn default_thumb_next_pages() -> u32 {
    4
}
fn default_thumb_vram_cap_percent() -> u32 {
    50
}
fn default_folder_thumb_sort() -> SortOrder {
    SortOrder::Numeric
}
fn default_folder_thumb_depth() -> u32 {
    3
}
fn default_ai_upscale_prefetch_back() -> usize {
    1
}
fn default_ai_upscale_prefetch_forward() -> usize {
    2
}
fn default_retained_final_ai_cache_max_entries() -> usize {
    RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_DEFAULT
}
fn default_retained_final_ai_cache_max_mib() -> u64 {
    RETAINED_FINAL_AI_CACHE_MAX_MIB_DEFAULT
}
fn default_ai_upscale_skip_px() -> u32 {
    2048
}
fn default_ai_denoise_skip_px() -> u32 {
    2048
}
pub fn default_exif_hidden_tags() -> Vec<String> {
    [
        // バイナリ / 巨大データ
        "MakerNote",
        "UserComment",
        "PrintImageMatching",
        // 空になりがちなフィールド
        "ImageDescription",
        "Artist",
        "Copyright",
        // 内部フォーマット情報
        "ComponentsConfiguration",
        "FlashpixVersion",
        "ExifVersion",
        "InteroperabilityIndex",
        "InteroperabilityVersion",
        "FileSource",
        "SceneType",
        // サムネイル IFD 全体
        "Compression",
        "JPEGInterchangeFormat",
        "JPEGInterchangeFormatLength",
        // 解像度 (通常は関心なし)
        "XResolution",
        "YResolution",
        "ResolutionUnit",
        // その他の低価値タグ
        "YCbCrPositioning",
        "SensitivityType",
        "OffsetTime",
        "OffsetTimeOriginal",
        "OffsetTimeDigitized",
        "GPSVersionID",
        "CustomRendered",
        "DigitalZoomRatio",
        "GainControl",
        "Contrast",
        "Saturation",
        "Sharpness",
        "Temperature",
        "Pressure",
        "WaterDepth",
        "Acceleration",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
pub fn default_image_ext_priority() -> Vec<String> {
    // ロスレス系 > ロッシー系 > RAW 系
    [
        "png", "bmp", "gif", "tiff", "tif", // ロスレス
        "webp", "jxl", "avif", "heic", "heif", // モダン (ロッシー/ロスレス混在)
        "jpg", "jpeg", // ロッシー
        "dng", "cr2", "cr3", "nef", "nrw", "arw", // RAW (現像困難な場合が多い)
        "srf", "sr2", "raf", "orf", "rw2", "pef", "ptx", "rwl", "iiq",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_slideshow_interval() -> f32 {
    3.0
}
fn default_spread_page_gap_px() -> u32 {
    4
}
fn default_continuous_reading_gap_px() -> u32 {
    20
}
fn default_continuous_reading_wheel_scroll_percent() -> u32 {
    20
}
fn default_continuous_reading_key_scroll_percent() -> u32 {
    16
}
fn default_continuous_reading_gamepad_scroll_percent_per_sec() -> u32 {
    130
}
fn default_fullscreen_jump_percent() -> u32 {
    FULLSCREEN_JUMP_PERCENT_DEFAULT
}
fn default_fullscreen_fixed_jump_count() -> usize {
    FULLSCREEN_FIXED_JUMP_DEFAULT
}
pub(crate) fn default_toolbar_cols_items() -> Vec<usize> {
    (MIN_GRID_COLS..=MAX_GRID_COLS).collect()
}
pub(crate) fn default_toolbar_aspect_items() -> Vec<ThumbAspect> {
    ThumbAspect::all().to_vec()
}
pub(crate) fn default_toolbar_aspect_auto_visible() -> bool {
    true
}
pub(crate) fn default_toolbar_sort_items() -> Vec<SortOrder> {
    SortOrder::all().to_vec()
}
pub fn default_rating_filter() -> [bool; 6] {
    [true; 6]
}

/// `Ctrl+E` ダイアログのバリエーションチェック初期値。
/// `[現在の設定, プリセット 1, 2, 3, 4]` → 現在の設定だけ ON。
pub fn default_export_batch_selection() -> [bool; 5] {
    [true, false, false, false, false]
}

fn sanitize_details_column_order(order: &mut Vec<DetailsColumnId>) {
    if order.is_empty() {
        return;
    }

    let mut cleaned = Vec::with_capacity(DetailsColumnId::default_order().len());
    for &column in order.iter() {
        if DetailsColumnId::default_order().contains(&column) && !cleaned.contains(&column) {
            cleaned.push(column);
        }
    }
    for &column in DetailsColumnId::default_order() {
        if !cleaned.contains(&column) {
            if column == DetailsColumnId::Preview {
                cleaned.insert(0, column);
            } else {
                cleaned.push(column);
            }
        }
    }
    *order = cleaned;
}

fn sanitize_details_column_widths(widths: &mut Vec<DetailsColumnWidth>) {
    let mut by_column = std::collections::BTreeMap::new();
    for width in widths.drain(..) {
        if width.column == DetailsColumnId::Name || !width.width.is_finite() {
            continue;
        }
        by_column.insert(width.column, width.width.clamp(40.0, 800.0));
    }
    widths.extend(
        by_column
            .into_iter()
            .map(|(column, width)| DetailsColumnWidth { column, width }),
    );
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            grid_cols: default_grid_cols(),
            grid_view_mode: GridViewMode::default(),
            details_sort_key: DetailsSortKey::default(),
            details_sort_ascending: default_details_sort_ascending(),
            details_size_display_mode: DetailsSizeDisplayMode::default(),
            details_timestamp_show_seconds: false,
            details_column_order: Vec::new(),
            details_column_widths: Vec::new(),
            details_show_preview: true,
            details_show_rating: true,
            details_show_tags: true,
            details_show_kind: true,
            details_show_size: true,
            details_show_modified: true,
            details_show_created: false,
            details_show_state: true,
            details_show_image_dimensions: false,
            details_show_video_duration: false,
            details_show_video_dimensions: false,
            details_show_video_codec: false,
            facet_filter: FacetFilter::default(),
            thumb_aspect: ThumbAspect::default(),
            thumb_aspect_auto: false,
            favorites: Vec::new(),
            last_folder: None,
            startup_folder_mode: StartupFolderMode::default(),
            startup_folder_path: None,
            recent_folders: Vec::new(),
            quick_folder_recent_folders: default_quick_folder_recent_folders(),
            quick_folder_slots: default_quick_folder_slots(),
            window_pos: None,
            window_size: None,
            parallelism: Parallelism::default(),
            prefetch_back: default_prefetch_back(),
            prefetch_forward: default_prefetch_forward(),
            folder_skip_limit: default_folder_skip_limit(),
            sort_order: SortOrder::default(),
            thumb_px: default_thumb_px(),
            text_preview_scale: default_text_preview_scale(),
            thumb_quality: default_thumb_quality(),
            cache_policy: CachePolicy::default(),
            cache_threshold_ms: default_cache_threshold_ms(),
            cache_size_threshold_bytes: default_cache_size_threshold_bytes(),
            cache_videos_always: true,
            cache_webp_always: true,
            cache_pdf_always: true,
            cache_zip_always: true,
            archive_cache_max_bytes: 0,
            archive_file_handling: ArchiveFileHandling::Ask,
            archive_convert_without_dialog: false,
            batch_cache_zip_contents: false,
            batch_cache_pdf_contents: false,
            search_index_checks: Vec::new(),
            indexer_speed_profile: IndexerSpeedProfile::default(),
            thumb_prev_pages: default_thumb_prev_pages(),
            thumb_next_pages: default_thumb_next_pages(),
            thumb_vram_cap_percent: default_thumb_vram_cap_percent(),
            thumb_idle_upgrade: true,
            thumb_tooltip_show_filename: true,
            thumb_tooltip_show_image_dimensions: true,
            thumb_tooltip_show_video_duration: true,
            thumb_tooltip_show_kind: false,
            thumb_tooltip_show_file_size: false,
            thumb_tooltip_show_modified: false,
            thumb_tooltip_show_created: false,
            thumb_tooltip_show_video_dimensions: false,
            thumb_tooltip_show_video_codec: false,
            thumb_tooltip_show_location: false,
            thumb_tooltip_show_reading_history_last_read: true,
            thumb_tooltip_show_reading_history_progress: true,
            exif_hidden_tags: default_exif_hidden_tags(),
            skip_zip_if_folder_exists: true,
            skip_image_if_video_exists: true,
            skip_duplicate_images: true,
            image_ext_priority: default_image_ext_priority(),
            slideshow_interval_secs: default_slideshow_interval(),
            slideshow_end_action: SlideshowEndAction::default(),
            capture_output_dir: None,
            capture_format: crate::capture::CaptureFormat::default(),
            book_root: None,
            active_book_name: default_active_book_name(),
            pinned_books: Vec::new(),
            default_spread_mode: SpreadMode::default(),
            default_reading_flow: ReadingFlow::default(),
            default_reading_direction: ReadingDirection::default(),
            spread_page_gap_px: default_spread_page_gap_px(),
            continuous_reading_gap_px: default_continuous_reading_gap_px(),
            fullscreen_fit_mode: FullscreenFitMode::default(),
            fullscreen_fit_no_upscale: false,
            fullscreen_fit_no_downscale: false,
            fullscreen_left_panel_tab: FullscreenLeftPanelTab::default(),
            fullscreen_seek_bar_locked: false,
            fullscreen_page_number_overlay: true,
            fullscreen_keep_on_app_switch: false,
            fullscreen_cursor_hide_delay_secs: FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS,
            fullscreen_jump_mode: FullscreenJumpMode::Percent,
            fullscreen_jump_percent: FULLSCREEN_JUMP_PERCENT_DEFAULT,
            fullscreen_fixed_jump_count: FULLSCREEN_FIXED_JUMP_DEFAULT,
            continuous_reading_wheel_scroll_percent:
                default_continuous_reading_wheel_scroll_percent(),
            continuous_reading_key_scroll_percent: default_continuous_reading_key_scroll_percent(),
            continuous_reading_gamepad_scroll_percent_per_sec:
                default_continuous_reading_gamepad_scroll_percent_per_sec(),
            auto_fullscreen_zip_pdf: false,
            margin_fit_enabled: false,
            ui_theme: UiTheme::default(),
            first_setup_completed: false,
            ai_feature_mode: AiFeatureMode::default(),
            tags: Vec::new(),
            show_toolbar_favorites: true,
            show_toolbar_tags: true,
            folder_tree_pane_visible: false,
            folder_tree_pane_width_ratio: default_folder_tree_pane_width_ratio(),
            show_toolbar_folder: true,
            show_toolbar_folder_tree_button: true,
            show_toolbar_bookshelf: true,
            show_address_bar_history_nav: true,
            show_address_bar_quick_folders: true,
            show_toolbar_parent_button: true,
            show_toolbar_prev_folder: true,
            show_toolbar_next_folder: true,
            show_toolbar_vst3: true,
            show_toolbar_rating: true,
            show_toolbar_facet_filter: true,
            show_address_bar_favorite_button: true,
            show_address_bar_history_menu: true,
            show_address_bar_folder_pin: true,
            use_native_shell_context_menu: true,
            ring_shortcuts: crate::ring_shortcut::RingShortcutSettings::default(),
            rating_filter: default_rating_filter(),
            toolbar_cols_items: default_toolbar_cols_items(),
            toolbar_cols_details_visible: true,
            toolbar_aspect_items: default_toolbar_aspect_items(),
            toolbar_aspect_auto_visible: default_toolbar_aspect_auto_visible(),
            toolbar_cols_display: ToolbarSectionDisplay::default(),
            toolbar_aspect_display: ToolbarSectionDisplay::default(),
            toolbar_sort_display: ToolbarSectionDisplay::default(),
            toolbar_favorites_display: ToolbarSectionDisplay::default(),
            toolbar_tags_display: ToolbarSectionDisplay::default(),
            toolbar_bookshelf_display: ToolbarSectionDisplay::default(),
            toolbar_favorites_collapsed: false,
            toolbar_tags_collapsed: false,
            toolbar_bookshelf_collapsed: false,
            toolbar_sort_items: default_toolbar_sort_items(),
            toolbar_section_order: Vec::new(),
            show_toolbar_cols: true,
            show_toolbar_aspect: true,
            show_toolbar_sort: true,
            toolbar_section_new_row: Vec::new(),
            folder_thumb_sort: default_folder_thumb_sort(),
            folder_thumb_depth: default_folder_thumb_depth(),
            recent_open_with_apps: Vec::new(),
            custom_open_with_apps: Vec::new(),
            ai_upscale_enabled: false,
            ai_upscale_model_override: None,
            ai_upscale_prefetch_back: default_ai_upscale_prefetch_back(),
            ai_upscale_prefetch_forward: default_ai_upscale_prefetch_forward(),
            retained_final_ai_cache_max_entries: default_retained_final_ai_cache_max_entries(),
            retained_final_ai_cache_max_mib: default_retained_final_ai_cache_max_mib(),
            ai_upscale_skip_px: default_ai_upscale_skip_px(),
            ai_denoise_skip_px: default_ai_denoise_skip_px(),
            ai_upscale_size_limit: None,
            ai_denoise_size_limit: None,
            ai_backend: None,
            global_preset: crate::adjustment::AdjustParams::default(),
            preset_slots: crate::adjustment::PresetSlots::default(),
            sidecar_backup_enabled: true,
            tag_sidecar_backup_enabled: false,
            susie_enabled: true,
            susie_allow_parallel: true,
            minimize_to_tray_on_close: false,
            pause_indexer_while_minimized: false,
            write_rating_to_xmp: false,
            update_check_enabled: true,
            update_check_dismissed_version: None,
            perf_log_enabled: false,
            video_volume: default_video_volume(),
            video_playback_speed: default_video_playback_speed(),
            video_autoplay: false,
            video_autoplay_mode: VideoAutoplayMode::default(),
            video_loop: false,
            video_loop_mode: VideoLoopMode::default(),
            video_continuous_mode: crate::video::VideoContinuousMode::default(),
            video_start_muted: false,
            video_muted: false,
            video_resume_positions: std::collections::HashMap::new(),
            video_grid_open_starts_from_beginning: false,
            video_nav_resume: ResumeMode::Resume,
            book_open_resume: ResumeMode::Resume,
            book_nav_resume: ResumeMode::FromStart,
            reading_history_enabled: true,
            reading_history_limit: default_reading_history_limit(),
            video_hw_decode: true,
            video_deinterlace: VideoDeinterlaceMode::default(),
            video_thumb_use_sidecar_image: true,
            video_tile_columns: default_video_tile_columns(),
            video_in_window_mode: false,
            detached_viewer_enabled: false,
            detached_viewer_window_placement: None,
            vst3_enabled: false,
            vst3_plugins: Vec::new(),
            vst3_plugin_path: None,
            vst3_plugin_state: None,
            vst3_gui_visible: true,
            vst3_video_compact: false,
            vst3_panel_pos: None,
            vst3_chain_slots: Vst3ChainPresetSlots::default(),
            audio_normalize_enabled: false,
            audio_normalize_target_lufs_milli: default_audio_normalize_target_lufs_milli(),
            last_seen_version: None,
            // ── 隠蔽加工 (Phase 1) ────────────────────────────
            conceal_type: crate::conceal::ConcealType::default(),
            conceal_mosaic_tile_mode: crate::conceal::TileSizeMode::default(),
            conceal_mosaic_boundary: crate::conceal::MosaicBoundary::default(),
            conceal_fill_opacity_percent: crate::conceal::default_fill_opacity(),
            conceal_fill_edge: crate::conceal::FillEdge::default(),
            conceal_blur_radius_px: crate::conceal::default_blur_radius_px(),
            conceal_blur_mode: crate::conceal::BlurMode::default(),
            conceal_blur_feather: false,
            conceal_brush_radius: 0.0, // enter_conceal_mode で初期化
            conceal_line_width: 0.0,   // enter_conceal_mode で初期化
            conceal_presets: crate::conceal::default_conceal_presets(),
            // ── エクスポート (Phase 1 部分、Phase 6 で UI 完成) ──
            export_embed_metadata: true,
            export_last_directory: None,
            export_fallback_format: crate::conceal::ExportFallbackFormat::default(),
            export_default_scale: crate::export_dialog::ExportScale::default(),
            export_batch_selection: default_export_batch_selection(),
        }
    }
}

// -----------------------------------------------------------------------
// settings.json バックアップ / アトミック保存
// -----------------------------------------------------------------------
//
// 過去に新バイナリ初回起動時に settings.json が default で上書きされ、お気に入り
// やタグが消えるユーザー報告 (2026-05-09) があった。再発しても自動復旧できるよう、
// 以下の安全網を 1 セットで導入する:
//
//   #1 atomic save                  — 半端ファイル根絶
//   #2 世代バックアップ (10 世代)   — `settings.json.bak1..bak10`
//   #3 logger 出力                  — 失敗を `mimageviewer.log` に残す
//   #4 アップグレード前バックアップ — `settings.json.preupgrade-v<old>`
//   #5 quarantine                   — 壊れた main を `settings.json.broken-<TS>` に退避
//
// 詳細フロー:
//   load(): try main → fail なら quarantine → bak1..bak10 を新→古で順試行 →
//            復旧成功なら main に copy 戻し / 全滅なら Default。
//            その後バージョン跨ぎを検出したら preupgrade snapshot を作成。
//   save(): プロセス内最初の保存だけ rotate_backups で世代を 1 段ずらしてから
//            atomic write で main を書き換える。

// ----------------------------------------------------------------------------
// 旧 JSON ベース永続化のヘルパ群 (Phase 3 で SQLite に切替済み)。
//
// 以下の関数群は **Phase 3 では runtime 経路から呼ばれない** が、次の理由で
// 残置し `#[allow(dead_code)]` を付ける:
// 1. Phase 2 migration から `try_parse_settings_file` / `try_load_with_recovery`
//    に等価な経路 (`read_settings_json_for_migration`) を提供しており、設計参考用に
//    本物のロジックを近くに置いておきたい。
// 2. レガシー write_atomic / rotate_backups / quarantine_path は他クレートに公開
//    されない private 関数なので、`#[deprecated]` の警告ターゲットにできない。
// 3. 数バージョン後 (= Phase 6 / 7) で deletion を検討する (spec §9 Phase 6)。
//
// `Settings::load` / `Settings::save` の新実装は `settings_db::boot_settings_db`
// と `settings_db::with_db_result` を経由する。
// ----------------------------------------------------------------------------

const BACKUP_COUNT: usize = 10;

/// 現プロセス内で `Settings::save()` の世代ローテーションが既に実施されたかを記録する。
/// 起動 1 回につき最初の save() でのみ rotation を走らせ、以降の保存は
/// settings.json を上書きするだけ (= bak1 = "今セッションを開いた時点の状態" を維持)。
static BACKUP_DONE_THIS_SESSION: AtomicBool = AtomicBool::new(false);

/// 起動時に main がパースエラーではなく **I/O エラー** で読めなかった場合に立てる。
///
/// このセッションでは backup から in-memory 復旧して動作するが、`Settings::save()`
/// は **完全にスキップ** する。理由: save() は `rotate_backups` で `settings.json -> bak1`
/// に rename し、その後 `write_atomic` で新規 main を作る。main が
/// (権限拒否・ロック・ディレクトリと衝突等で) 一時的にアクセス不能なだけのとき、
/// この rename が成功してしまうと **真に保護したかった main** が bak1 に置き換わり
/// 失われる (Codex P2 2026-05-09 指摘)。
///
/// 抑止結果として、このセッションでユーザーが触った設定は永続化されない。
/// 次回起動で I/O 障害が解消していれば main をそのまま読めるし、解消していなくても
/// backup から再度 in-memory 復旧する。**意図したトレードオフ**: 「セッション中の
/// 入力消失」 vs 「本物の main 喪失」。後者の方が遥かに深刻なので前者を選ぶ。
static MAIN_UNREADABLE_THIS_SESSION: AtomicBool = AtomicBool::new(false);

fn backup_path(main: &Path, n: usize) -> PathBuf {
    let mut name = main
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
    name.push(format!(".bak{}", n));
    main.with_file_name(name)
}

#[allow(dead_code)] // Phase 3: 旧 JSON 経路の残置 (settings.rs 冒頭の解説参照)
fn quarantine_path(main: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = main
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
    name.push(format!(".broken-{}", stamp));
    main.with_file_name(name)
}

#[allow(dead_code)] // Phase 3: 旧 JSON 経路の残置 (settings.rs 冒頭の解説参照)
fn preupgrade_path(main: &Path, prev_version: &str) -> PathBuf {
    let label = safe_version_label(prev_version);
    let mut name = main
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
    name.push(format!(".preupgrade-v{}", label));
    main.with_file_name(name)
}

/// バージョン文字列をファイル名に埋めても安全な形にする。
/// `[A-Za-z0-9._-]` 以外は `_` に置換、空文字は "unknown"。
fn safe_version_label(v: &str) -> String {
    let cleaned: String = v
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// `try_parse_settings_file` の結果。
///
/// I/O エラー (`PermissionDenied`, ファイルロック中, デバイスエラー等) と
/// 内容のエラー (UTF-8 デコード失敗 / JSON パース失敗) を区別する。
/// 前者は **一時的かもしれない** ため main を quarantine してはならず、
/// 後者は本当に壊れているので退避して bak から復旧する (Codex P2 2026-05-09)。
enum LoadFileResult {
    Ok(Settings),
    NotFound,
    /// 読み取り I/O 失敗。ファイルは存在するが内容を取得できなかった
    /// (権限・ロック・デバイス障害等)。再試行で直る可能性があるので退避しない。
    IoError,
    /// 内容を bytes として取得できたが Settings として解釈できなかった
    /// (UTF-8 でない / JSON でない / スキーマ違反)。退避対象。
    ParseError,
}

#[cfg(test)]
impl std::fmt::Debug for LoadFileResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadFileResult::Ok(_) => write!(f, "Ok(_)"),
            LoadFileResult::NotFound => write!(f, "NotFound"),
            LoadFileResult::IoError => write!(f, "IoError"),
            LoadFileResult::ParseError => write!(f, "ParseError"),
        }
    }
}

/// 指定パスの JSON を `Settings` にパースする。詳細は `LoadFileResult` を参照。
///
/// `read_to_string` ではなく `std::fs::read` で bytes を読んでから UTF-8 変換することで、
/// I/O 段階のエラー (`InvalidData` 以外の OS エラー) と内容段階のエラー (UTF-8 / JSON) を
/// 切り分ける。前者は IoError、後者はどちらも ParseError。
/// `NotFound` のみは正常な「初回起動」相当なのでログを抑制する。
fn try_parse_settings_file(path: &Path) -> LoadFileResult {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadFileResult::NotFound,
        Err(e) => {
            settings_diag_log(&format!("settings: read failed {}: {}", path.display(), e));
            return LoadFileResult::IoError;
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(e) => {
            settings_diag_log(&format!(
                "settings: UTF-8 decode failed {}: {}",
                path.display(),
                e
            ));
            return LoadFileResult::ParseError;
        }
    };
    match serde_json::from_str::<Settings>(text) {
        Ok(s) => LoadFileResult::Ok(s),
        Err(e) => {
            settings_diag_log(&format!(
                "settings: JSON parse failed {}: {}",
                path.display(),
                e
            ));
            LoadFileResult::ParseError
        }
    }
}

/// `try_load_with_recovery` の結果。
///
/// `main_unreadable` は **main が I/O エラーで読めなかった** ことを示す。
/// パース失敗 (`ParseError`) は main を quarantine してしまうので、その後はもう
/// "main が手付かずで残っているケース" ではない → false に集約する。
/// 一方 `IoError` は main をそのまま残しているので、後段の save() が rotate で
/// 触らないよう呼び出し元 (`Settings::load`) でフラグ伝搬する。
#[allow(dead_code)] // Phase 3: 旧 JSON load 経路用、残置
struct LoadOutcome {
    settings: Option<Settings>,
    main_unreadable: bool,
}

/// Phase 2 migration の `read_settings_json_for_migration` 戻り値
/// (Codex P1 v11 2026-05-14)。`Option<Settings>` ではなく明示的な enum で返すことで、
/// caller が「main が transient I/O で読めないだけ」と「main が parse fail で
/// bak から復旧した」を区別できるようにする。前者は **bak 採用せず即 abort** が
/// 正解 (= main の本物が次回読めるまで待つ)。
pub(crate) enum MigrationReadResult {
    /// main または bak からの読み込みに成功。
    Loaded(Settings),
    /// main が IoError で読めなかった。bak には絶対倒れず、上層は migration 自体を
    /// 諦めて save 抑止に倒す (= 次回ブートで main がまた読めればそこから migration する)。
    MainUnreadable,
    /// main も bak も全部 NotFound / ParseError。migration ソースが完全に無い。
    AllFailed,
}

/// Phase 2 (SQLite migration) から呼ぶ migration エントリ。
///
/// `main` (`settings.json`) と `main.bak1..bak10` を順に試行する。`try_load_with_recovery`
/// のラッパだが:
/// - 副作用 (broken-ts rename / main への copy 書き戻し) は **抑制する**。migration 経路は
///   読みっぱなしで OK。
/// - main の I/O エラーは bak フォールバック対象とせず、`MainUnreadable` で返す
///   (Codex P1 v11 2026-05-14)。main の transient I/O で誤って古い bak から migrate して
///   main を上書き quarantine する事故を防ぐ。
/// - main の `NotFound` も **ambiguous NotFound** (= read_dir で親 dir 列挙すると main が
///   見える) なら `MainUnreadable` で abort。同じ AV / cloud-sync 起因の transient で
///   bak に倒して main を消す事故を防ぐ (Codex P1 v12 2026-05-14)。
///   - read_dir で main が本当に見えない場合だけ「真の不在」と見なし、bak フォールバックを
///     許可する (= 旧マイグレーション残骸 / 手動削除等)。
/// - main の ParseError は bak フォールバックする (= 内容破損として正当)。
pub(crate) fn read_settings_json_for_migration(main: &Path) -> MigrationReadResult {
    match try_parse_settings_file(main) {
        LoadFileResult::Ok(s) => return MigrationReadResult::Loaded(s),
        LoadFileResult::IoError => {
            settings_diag_log(&format!(
                "settings: migration: main {} unreadable (I/O error); aborting migration",
                main.display()
            ));
            return MigrationReadResult::MainUnreadable;
        }
        LoadFileResult::NotFound => {
            // Codex P1 v12 (2026-05-14): read_dir で本当に存在しないか robust 確認。
            // 一度の `read` が `NotFound` を返してきても、read_dir で main が見えれば
            // transient と判定して `MainUnreadable` 扱いにする。
            if !path_really_absent_via_readdir(main) {
                settings_diag_log(&format!(
                    "settings: migration: main {} reports NotFound but read_dir sees it; \
                     treating as transient and aborting migration",
                    main.display()
                ));
                return MigrationReadResult::MainUnreadable;
            }
            settings_diag_log(&format!(
                "settings: migration: main {} confirmed absent via read_dir; bak fallback ok",
                main.display()
            ));
        }
        LoadFileResult::ParseError => {
            // 内容破損は bak フォールバックの正当な理由なのでそのまま進む。
        }
    }
    for n in 1..=BACKUP_COUNT {
        let bak = backup_path(main, n);
        if let LoadFileResult::Ok(s) = try_parse_settings_file(&bak) {
            settings_diag_log(&format!(
                "settings: migration: loaded {} for SQLite migration",
                bak.display()
            ));
            return MigrationReadResult::Loaded(s);
        }
    }
    MigrationReadResult::AllFailed
}

/// 指定パスの親 dir を `read_dir` で列挙して、対象 file が **本当に存在しない** ことを
/// 確認する (Codex P1 v12 2026-05-14)。`std::fs::metadata` / `std::fs::read` が
/// `NotFound` を返しても、read_dir で見えるなら transient NotFound と判定する。
///
/// 戻り値:
/// - true: 親 dir は読めて main が見当たらない → 真の不在
/// - false: read_dir が main を列挙する → transient (= path 自体は disk 上に存在する)
/// - false: read_dir 自体が失敗 → 判別不能 → 安全側に倒して transient 扱いとする
fn path_really_absent_via_readdir(path: &Path) -> bool {
    let parent = match path.parent() {
        Some(p) => p,
        None => return true, // 親 path 不明 (= root 等)。元の NotFound を信じる。
    };
    let file_name = match path.file_name() {
        Some(n) => n,
        None => return true,
    };
    match std::fs::read_dir(parent) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_name() == file_name {
                    return false; // 列挙できた → transient
                }
            }
            true // 列挙したが見えない → 本当に不在
        }
        Err(_) => false, // 親 dir 列挙も落ちる → 判別不能 → 安全側 (transient 扱い)
    }
}

/// migration 完了後にリネームすべき旧 JSON ファイル一覧を返す
/// (`main` + `main.bak1..bak10` のうち実在するもの)。
pub(crate) fn legacy_json_files_for_migration(main: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if main.exists() {
        out.push(main.to_path_buf());
    }
    for n in 1..=BACKUP_COUNT {
        let bak = backup_path(main, n);
        if bak.exists() {
            out.push(bak);
        }
    }
    out
}

/// `legacy_json_family_presence` を `Ambiguous` が出るうちは最大 N 回リトライする
/// (T06 v0.9.0 Codex P2 反映)。AV / cloud sync の一瞬の blip 対応。
pub(crate) fn legacy_json_family_presence_with_retry(
    data_dir: &Path,
) -> crate::settings_db::FamilyPresence {
    use crate::settings_db::FamilyPresence;
    const ATTEMPTS: u32 = 3;
    const BACKOFF_MS: u64 = 80;
    for attempt in 0..ATTEMPTS {
        let presence = legacy_json_family_presence(data_dir);
        if presence != FamilyPresence::Ambiguous {
            if attempt > 0 {
                crate::settings_db::log_diag(&format!(
                    "settings: legacy_json family presence stabilized to {presence:?} on attempt {}",
                    attempt + 1
                ));
            }
            return presence;
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(BACKOFF_MS));
        }
    }
    FamilyPresence::Ambiguous
}

/// 旧 `settings.json` 家族の存在を tri-state で判定する (T06 v0.9.0)。
/// `settings_db::FamilyPresence` と同じセマンティクス: per-file metadata の NotFound 以外
/// エラー or `read_dir` の non-NotFound 失敗を `Ambiguous` として上層に返し、decision tree
/// が「JSON 無し → clean install」に誤って倒れて旧データを破棄するのを防ぐ。
pub(crate) fn legacy_json_family_presence(data_dir: &Path) -> crate::settings_db::FamilyPresence {
    use crate::settings_db::FamilyPresence;
    let mut ambiguous = false;
    let main = data_dir.join("settings.json");
    let candidates =
        std::iter::once(main.clone()).chain((1..=BACKUP_COUNT).map(|n| backup_path(&main, n)));
    for p in candidates {
        match std::fs::metadata(&p) {
            Ok(_) => return FamilyPresence::Present,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => ambiguous = true,
        }
    }
    // 経路 2: read_dir 列挙 (flatten() を使わず entry error を Ambiguous に拾う)。
    match std::fs::read_dir(data_dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(e) => {
                        if let Some(name) = e.file_name().to_str() {
                            if name == "settings.json" {
                                return FamilyPresence::Present;
                            }
                            if let Some(rest) = name.strip_prefix("settings.json.bak")
                                && rest.parse::<u32>().is_ok()
                                && rest == rest.trim_start_matches('0')
                                && let Ok(n) = rest.parse::<u32>()
                                && (1..=BACKUP_COUNT as u32).contains(&n)
                            {
                                return FamilyPresence::Present;
                            }
                        }
                    }
                    Err(_) => ambiguous = true,
                }
            }
            if ambiguous {
                FamilyPresence::Ambiguous
            } else {
                FamilyPresence::ConfirmedAbsent
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if ambiguous {
                FamilyPresence::Ambiguous
            } else {
                FamilyPresence::ConfirmedAbsent
            }
        }
        Err(_) => FamilyPresence::Ambiguous,
    }
}

// 旧 `legacy_settings_json_path()` は Codex P2 v8b-2 (2026-05-14) で削除。Phase 2 の
// migration / decision tree は data_dir 引数を唯一の真として `data_dir.join("settings.json")`
// を使うので、`data_dir::get()` 経由のパス計算は不要になった。

/// `Settings::load()` 経路で適用される **load-time migrations** を、外部呼び出し用に
/// 公開した版 (Codex P2 v8b-1 2026-05-14)。Phase 2 の JSON migration はこの関数を
/// 介して読み込んだ Settings を正規化してから SQLite に書き込む必要がある。
/// 正規化せずに DB へ書くと:
/// - favorites の id = nil (= 旧形式) が複数あると PRIMARY KEY 衝突で save_full が失敗
/// - vst3_plugin_path/state 旧形式が Vec に流れない
/// - video_loop=true の旧 bool が video_loop_mode に伝搬しない
///
/// `Settings::load()` の中身と同じ migrations を呼ぶ:
/// 1. `migrate_vst3_legacy`
/// 2. `migrate_legacy_video_loop`
/// 3. `sanitize` (favorites の nil UUID 発行、video_volume クランプ等)
pub(crate) fn apply_load_time_migrations(settings: &mut Settings) {
    settings.migrate_vst3_legacy();
    settings.migrate_legacy_video_loop();
    settings.migrate_legacy_archive_file_handling();
    settings.sanitize();
}

/// メイン → bak1 → bak2 → … の順で復旧を試みる。
///
/// メインが **パース失敗** (= 内容が壊れている) のときだけ `.broken-<TS>` に rename 退避し、
/// I/O エラー (権限拒否・ロック・ディレクトリと衝突等) のときは退避しない
/// (Codex P2 2026-05-09)。I/O エラーは一時的な可能性があり、ここで rename して
/// しまうと正常なファイルを失う恐れがある。
/// 復旧できた場合は **退避が成功した時のみ** bak の内容を main に copy で書き戻し、
/// 次回 load から同じ復旧を繰り返さないようにする。全滅は `settings = None`。
#[allow(dead_code)] // Phase 3: 旧 JSON load 経路、残置
fn try_load_with_recovery(main: &Path) -> LoadOutcome {
    let main_result = try_parse_settings_file(main);
    if let LoadFileResult::Ok(s) = main_result {
        return LoadOutcome {
            settings: Some(s),
            main_unreadable: false,
        };
    }

    let main_was_io_error = matches!(main_result, LoadFileResult::IoError);
    let main_quarantined = matches!(main_result, LoadFileResult::ParseError);
    if main_quarantined {
        let q = quarantine_path(main);
        match std::fs::rename(main, &q) {
            Ok(_) => settings_diag_log(&format!(
                "settings: quarantined corrupt {} -> {}",
                main.display(),
                q.display()
            )),
            Err(e) => settings_diag_log(&format!(
                "settings: quarantine failed {} -> {}: {}",
                main.display(),
                q.display(),
                e
            )),
        }
    }

    for n in 1..=BACKUP_COUNT {
        let bak = backup_path(main, n);
        match try_parse_settings_file(&bak) {
            LoadFileResult::Ok(s) => {
                settings_diag_log(&format!("settings: recovered from {}", bak.display()));
                // 退避が走ったときのみ main に書き戻す。I/O エラーで main を残した
                // ケースは触らない (= ロックが解けたら元の main をまた読める)。
                if main_quarantined {
                    if let Err(e) = std::fs::copy(&bak, main) {
                        settings_diag_log(&format!(
                            "settings: failed to write recovered content back to {}: {}",
                            main.display(),
                            e
                        ));
                    }
                }
                return LoadOutcome {
                    settings: Some(s),
                    main_unreadable: main_was_io_error,
                };
            }
            // NotFound / IoError / ParseError はどれも次の bak を試すだけ。
            // bak ファイル自体は退避しない (= 壊れた bak はそのまま放置し、
            // ローテーションで自然に押し出されるのを待つ)。
            _ => continue,
        }
    }

    LoadOutcome {
        settings: None,
        main_unreadable: main_was_io_error,
    }
}

/// `Settings::load` 入口で main + bak1..bak10 の disk 上の状態を `settings.log` に
/// 1 ブロック append する。各ファイルの size / mtime / `read()` の即時試行結果を
/// 1 行ずつ書く。クラッシュ → 再起動で「全 NotFound 落ち」が再発したとき、
/// 「load 試行時に何が disk 上にあったか」を後から検証するための診断ログ。
///
/// 出力例:
/// ```text
/// [ts] settings: load disk snapshot:
/// [ts]   main settings.json: size=4166542 mtime=... read=Ok
/// [ts]   bak1: size=4166431 mtime=... read=Ok
/// [ts]   bak2: missing
/// ...
/// ```
#[allow(dead_code)] // Phase 3: 旧 JSON load 経路、残置
fn log_disk_snapshot(main: &Path) {
    settings_diag_log("settings: load disk snapshot:");
    log_one_file_snapshot("main settings.json", main);
    for n in 1..=BACKUP_COUNT {
        log_one_file_snapshot(&format!("bak{n}"), &backup_path(main, n));
    }
}

#[allow(dead_code)] // Phase 3: 旧 JSON load 経路、残置
fn log_one_file_snapshot(label: &str, path: &Path) {
    let meta = std::fs::metadata(path);
    let read_kind = match std::fs::File::open(path) {
        Ok(_) => "Ok".to_string(),
        Err(e) => format!("Err({:?})", e.kind()),
    };
    match meta {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(-1);
            settings_diag_log(&format!(
                "settings:   {label}: size={} mtime_unix={} read={read_kind}",
                m.len(),
                mtime
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            settings_diag_log(&format!("settings:   {label}: missing"));
        }
        Err(e) => {
            settings_diag_log(&format!(
                "settings:   {label}: metadata Err({:?}) read={read_kind}",
                e.kind()
            ));
        }
    }
}

/// `main` (settings.json) または `bak1..bak{BACKUP_COUNT}` のうち、いずれか 1 つでも
/// ディスク上に **ファイルとして実在** するかを返す。
///
/// `Settings::load` で「全 load 失敗 → built-in default」に落ちたとき、これが `true`
/// なら「真の初回起動ではない」と判断し、save 抑止フラグを立てて世代ローテで bak が
/// 空 default に押し出されるのを防ぐ。
///
/// `try_parse_settings_file` が `NotFound` を返したパスでも、ここで `metadata()` を呼べば
/// 別 API なので Windows のロック / share violation 由来の偽 NotFound と区別できる
/// (= `std::fs::read` が NotFound と言っても `std::fs::metadata` は別経路を辿る)。
#[allow(dead_code)] // Phase 3: 旧 JSON load 経路、残置
fn any_settings_file_exists(main: &Path) -> bool {
    if std::fs::metadata(main).is_ok() {
        return true;
    }
    for n in 1..=BACKUP_COUNT {
        if std::fs::metadata(backup_path(main, n)).is_ok() {
            return true;
        }
    }
    false
}

/// 世代バックアップを 1 段ずらす。`bak{N}` を捨て、`bakN-1 -> bakN` …
/// 最後に `settings.json -> bak1` で現状を退避する。本関数は **メインを書き込む前**
/// に呼ぶ前提 (= 実行後 main は存在しなくなるが、続く `write_atomic` が新ファイルを作る)。
#[allow(dead_code)] // Phase 3: 旧 JSON save 経路、残置 (SQLite 版は SettingsDb::rotate_backups)
fn rotate_backups(main: &Path) {
    // 一番古い世代を捨てる。
    let oldest = backup_path(main, BACKUP_COUNT);
    let _ = std::fs::remove_file(&oldest);

    // bak{n} -> bak{n+1} (高い番号から処理しないと衝突する)。
    for n in (1..BACKUP_COUNT).rev() {
        let from = backup_path(main, n);
        let to = backup_path(main, n + 1);
        if from.exists() {
            if let Err(e) = std::fs::rename(&from, &to) {
                settings_diag_log(&format!(
                    "settings: rotate {} -> {} failed: {}",
                    from.display(),
                    to.display(),
                    e
                ));
            }
        }
    }

    // 最後に main -> bak1。
    let bak1 = backup_path(main, 1);
    if main.exists() {
        if let Err(e) = std::fs::rename(main, &bak1) {
            settings_diag_log(&format!(
                "settings: rotate {} -> {} failed: {}",
                main.display(),
                bak1.display(),
                e
            ));
        }
    }
}

/// アトミック書き込み: `<path>.tmp` に書き込んでから rename で置き換える。
///
/// `std::fs::rename` は **Windows でも `MoveFileExW(MOVEFILE_REPLACE_EXISTING |
/// MOVEFILE_WRITE_THROUGH)` 経由で atomic な置換を行う** (Rust 1.70+ の仕様、
/// `library/std/src/sys/pal/windows/fs.rs` 参照)。POSIX の rename(2) も同様に
/// atomic。よって既存 dest を事前削除する必要はなく、削除すると逆に
/// rename 失敗時に main が消えてセッション中の変更が飛ぶ
/// (Codex P2 2026-05-09 指摘)。
///
/// rename 失敗時は新内容が `.tmp` に残っていても main の旧内容は無傷なので、
/// アプリは古い設定で動き続ける。`.tmp` は best-effort で掃除する。
#[allow(dead_code)] // Phase 3: 旧 JSON save 経路、残置 (SQLite 版は SettingsDb::save_full)
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = {
        let mut name = path
            .file_name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
        name.push(".tmp");
        path.with_file_name(name)
    };
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// settings 復旧経路向けの永続診断ログ。
///
/// `crate::logger::log` はデフォルトでは未初期化 (`logger::init()` が
/// `cfg!(debug_assertions) || --log` でしか呼ばれない、`src/main.rs` 参照) なので、
/// **release ビルドの通常起動では復旧ログが残らない** (Codex P2 2026-05-09 指摘)。
/// 設定リセット系のユーザー報告は再現が難しく、後追い解析するためにはイベントが
/// 確実にディスクに残っている必要がある。
///
/// そこで本関数は:
///   1. `crate::logger::log` にも投げる (initialized なら mimageviewer.log に残る)
///   2. **常に** `<data_dir>/logs/settings.log` に append する
///      (= release ビルドでも `--log` 不要で診断履歴が残る)
///
/// `panic.log` と同じ「常時 ON の診断ログ」枠の扱い。
fn settings_diag_log(msg: &str) {
    use std::io::Write;

    // dev / `--log` 起動時はメインの logger にも残しておく。
    crate::logger::log(msg);

    // <data_dir>/logs/settings.log は常に append する (init 不要の独立 sink)。
    let path = crate::data_dir::logs_dir().join("settings.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{timestamp}] {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

#[cfg(test)]
fn reset_backup_state_for_test() {
    BACKUP_DONE_THIS_SESSION.store(false, Ordering::Relaxed);
    MAIN_UNREADABLE_THIS_SESSION.store(false, Ordering::Relaxed);
    // Phase 3: settings_db 側の global state もリセットする (= 直列化された他テストの
    // 設定が漏れ込まない)。
    crate::settings_db::reset_global_for_test();
    crate::settings_db::set_save_suppressed(false);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLoadMeta {
    pub boot_source: crate::settings_db::BootSource,
    pub previous_last_seen_version: Option<String>,
    pub db_loaded: bool,
}

impl Default for SettingsLoadMeta {
    fn default() -> Self {
        Self {
            boot_source: crate::settings_db::BootSource::CleanInstall,
            previous_last_seen_version: None,
            db_loaded: false,
        }
    }
}

#[derive(Clone)]
pub struct SettingsLoadResult {
    pub settings: Settings,
    pub meta: SettingsLoadMeta,
}

impl Settings {
    pub fn books_root_path(&self) -> PathBuf {
        crate::books::settings_books_root(self)
    }

    pub fn active_book_name_or_default(&self) -> String {
        let name = crate::books::normalize_book_name(&self.active_book_name);
        if name.is_empty() {
            default_active_book_name()
        } else {
            name
        }
    }

    pub fn archive_file_handling_resolved(&self) -> ArchiveFileHandling {
        self.archive_file_handling
            .resolved(self.archive_convert_without_dialog)
    }

    pub fn archive_convert_suppresses_confirm(&self) -> bool {
        self.archive_file_handling_resolved() == ArchiveFileHandling::Convert
    }

    pub fn archive_file_handling_ignores_convertible(&self) -> bool {
        self.archive_file_handling_resolved() == ArchiveFileHandling::Ignore
    }

    pub fn set_archive_file_handling(&mut self, handling: ArchiveFileHandling) {
        let handling = handling.resolved(self.archive_convert_without_dialog);
        self.archive_file_handling = handling;
        self.archive_convert_without_dialog = handling == ArchiveFileHandling::Convert;
    }

    /// 位置復元マトリクス「動画 × 一覧から開く」セル。保存先は互換維持のため既存 bool
    /// `video_grid_open_starts_from_beginning`。FromStart = 先頭から開く。
    pub fn video_open_resume(&self) -> ResumeMode {
        if self.video_grid_open_starts_from_beginning {
            ResumeMode::FromStart
        } else {
            ResumeMode::Resume
        }
    }

    /// 上記セルの設定。既存 bool に書き戻す (旧バージョンへ downgrade しても解釈できる)。
    pub fn set_video_open_resume(&mut self, mode: ResumeMode) {
        self.video_grid_open_starts_from_beginning = matches!(mode, ResumeMode::FromStart);
    }

    #[allow(dead_code)] // Phase 3: 旧 JSON 経路で使われていた settings.json パス
    fn settings_path() -> PathBuf {
        crate::data_dir::get().join("settings.json")
    }

    /// UUID でお気に入りを引く。UI ドロップダウン等で `Option<Uuid>` を表示するときに使う。
    pub fn favorite_by_id(&self, id: uuid::Uuid) -> Option<&FavoriteEntry> {
        self.favorites.iter().find(|f| f.id == id)
    }

    /// AI アップスケールの実効サイズ上限。新フィールドが無い旧設定では
    /// 旧単一しきい値 `N` を `N x N` として読み替える (挙動互換)。
    pub fn ai_upscale_limit(&self) -> crate::ai::upscale::AiProcessSizeLimit {
        self.ai_upscale_size_limit.unwrap_or_else(|| {
            crate::ai::upscale::AiProcessSizeLimit::square(self.ai_upscale_skip_px)
        })
    }

    /// AI ノイズ除去の実効サイズ上限。読み替え規則は `ai_upscale_limit()` と同じ。
    pub fn ai_denoise_limit(&self) -> crate::ai::upscale::AiProcessSizeLimit {
        self.ai_denoise_size_limit.unwrap_or_else(|| {
            crate::ai::upscale::AiProcessSizeLimit::square(self.ai_denoise_skip_px)
        })
    }

    /// 音量ノーマライズの target_lufs_milli を、設定ファイル直接編集による
    /// 異常値から守るため `[-60_000, 0]` の範囲にクランプして返す。
    pub fn clamped_audio_normalize_target_lufs_milli(&self) -> i32 {
        self.audio_normalize_target_lufs_milli.clamp(
            AUDIO_NORMALIZE_TARGET_LUFS_MILLI_MIN,
            AUDIO_NORMALIZE_TARGET_LUFS_MILLI_MAX,
        )
    }

    /// 設定をロードする (Phase 3: SQLite ベース)。
    ///
    /// spec §5 の決定木 (`boot_settings_db`) を経由する。旧 JSON 経路 (`settings.json` +
    /// `*.bak1..bak10`) は `boot_settings_db` の内部で migration として読まれるだけで、
    /// 通常ロードでは触らない (= migration 完了後は `.migrated-<ts>` にリネーム済み)。
    pub fn load() -> Self {
        Self::load_with_meta().settings
    }

    /// `Settings::load()` と同じロードを行い、起動経路など load-time の判定材料も返す。
    ///
    /// 通常は `load()` を使う。main thread の起動処理だけが、リリース済み入力挙動の
    /// 変更確認など「clean install と upgrade を区別したい」用途でこのメタ情報を使う。
    pub fn load_with_meta() -> SettingsLoadResult {
        let data_dir = crate::data_dir::get();
        let outcome = crate::settings_db::boot_settings_db(&data_dir);
        let source = outcome.source;
        let db_loaded = outcome.db.is_some();
        if !db_loaded {
            // SettingsDb が使えない (全復旧経路 fail)。本セッションの save() は完全に
            // 抑止する (= 旧 MAIN_UNREADABLE_THIS_SESSION セマンティクスを継承)。
            // settings_db 側でも `SAVE_SUPPRESSED` が立っているので二重防御。
            MAIN_UNREADABLE_THIS_SESSION.store(true, Ordering::Relaxed);
            settings_diag_log(
                "settings: boot returned no DB handle (FailedFallbackDefault); \
                 save() suppressed for this session",
            );
        }
        let mut settings = outcome.settings;
        let previous_last_seen_version = settings.last_seen_version.clone();

        // SQLite 化で「文字列のままディスク上にいる外部編集 settings.json」のパスは
        // 消えているが、scheme migration が将来追加される可能性は残るので、`Settings::load`
        // と同じ load-time migrations を **再度** 適用しておく (idempotent)。
        // - MigratedFromJson: Phase 2 で既に適用済み → no-op
        // - LoadedExistingDb / RestoredFromDbBackup: DB に既に正規化済みのデータがいるはず
        //   だが念のため
        // - CleanInstall: Default 値なので no-op
        let autoplay_mode_migrated =
            settings.video_autoplay_mode == VideoAutoplayMode::OnlyFromGrid;
        let video_volume_before_sanitize = settings.video_volume;
        let video_playback_speed_before_sanitize = settings.video_playback_speed;
        let vst3_migrated = settings.migrate_vst3_legacy();
        let video_loop_migrated = settings.migrate_legacy_video_loop();
        let archive_file_handling_migrated = settings.migrate_legacy_archive_file_handling();
        settings.sanitize();
        let mouse_nav_upgrade_prompt_pending = matches!(
            source,
            crate::settings_db::BootSource::LoadedExistingDb
                | crate::settings_db::BootSource::MigratedFromJson
                | crate::settings_db::BootSource::RestoredFromDbBackup
                | crate::settings_db::BootSource::FailedFallbackDefault
        ) && !settings.ring_shortcuts.mouse_nav_prompt_done
            && matches!(
                settings.ring_shortcuts.mouse_back_forward_action,
                crate::ring_shortcut::MouseBackForwardActionId::None
            );
        if mouse_nav_upgrade_prompt_pending {
            settings.ring_shortcuts.set_mouse_buttons_from_legacy_pair(
                crate::ring_shortcut::MouseBackForwardActionId::TreeFolderPrevNext,
            );
        }
        let mouse_nav_clean_install_defaulted = source
            == crate::settings_db::BootSource::CleanInstall
            && !settings.ring_shortcuts.mouse_nav_prompt_done
            && matches!(
                settings.ring_shortcuts.mouse_back_forward_action,
                crate::ring_shortcut::MouseBackForwardActionId::None
            );
        if mouse_nav_clean_install_defaulted {
            settings.ring_shortcuts.set_mouse_buttons_from_legacy_pair(
                crate::ring_shortcut::MouseBackForwardActionId::FolderHistoryPrevNext,
            );
            settings.ring_shortcuts.mouse_back_forward_action =
                crate::ring_shortcut::MouseBackForwardActionId::None;
            settings.ring_shortcuts.mouse_nav_prompt_done = true;
        }
        let video_volume_sanitized =
            (settings.video_volume - video_volume_before_sanitize).abs() > 1.0e-9;
        let video_playback_speed_sanitized =
            (settings.video_playback_speed - video_playback_speed_before_sanitize).abs() > 1.0e-9;

        // バージョン跨ぎの安全網 (#4) を SQLite 版に置換:
        // - 旧版は `settings.json` を `settings.json.preupgrade-v<old>` に std::fs::copy
        // - 新版は `settings.db` を `settings.db.preupgrade-v<old>` に `VACUUM INTO` snapshot
        let current_version = env!("CARGO_PKG_VERSION");
        let prev_version = settings.last_seen_version.clone();
        let version_changed = prev_version.as_deref() != Some(current_version);
        if version_changed && db_loaded {
            let prev_label = prev_version.as_deref().unwrap_or("unknown");
            let pre_path = data_dir.join(format!(
                "settings.db.preupgrade-v{}",
                safe_version_label(prev_label)
            ));
            if !pre_path.exists() {
                // 既存ファイルがなければ snapshot を取る (= 同バージョンを複数回起動しても 1 回限り)。
                let result = crate::settings_db::with_db_result(|db| db.backup_to(&pre_path));
                match result {
                    Ok(()) => settings_diag_log(&format!(
                        "settings: pre-upgrade snapshot saved {} (prev v{})",
                        pre_path.display(),
                        prev_label
                    )),
                    Err(e) => settings_diag_log(&format!(
                        "settings: pre-upgrade snapshot failed {}: {}",
                        pre_path.display(),
                        e
                    )),
                }
            }
            settings.last_seen_version = Some(current_version.to_string());
        }

        settings_diag_log(&format!(
            "settings: boot source = {source:?}, favorites={}, save_enabled={}",
            settings.favorites.len(),
            db_loaded && !MAIN_UNREADABLE_THIS_SESSION.load(Ordering::Relaxed)
        ));

        // 何かしら値が変わったなら書き戻して永続化する。db_loaded == false なら
        // save_internal が即 return するので無害。
        //
        // Codex P2 v13 (2026-05-14): この writeback は **bootstrap save** なので
        // `save_internal_no_rotation` を使う。spec §6.1 で rotation は「**user** save の
        // 最初の 1 回」と定義されており、`load()` 内の migration/version 書き戻しで
        // rotation を消費すると次の真の user save が in-place 書込みになってしまう。
        if vst3_migrated
            || autoplay_mode_migrated
            || video_loop_migrated
            || archive_file_handling_migrated
            || video_volume_sanitized
            || video_playback_speed_sanitized
            || mouse_nav_clean_install_defaulted
            || version_changed
        {
            settings.save_internal_no_rotation();
        }
        SettingsLoadResult {
            settings,
            meta: SettingsLoadMeta {
                boot_source: source,
                previous_last_seen_version,
                db_loaded,
            },
        }
    }

    /// 旧 `archive_convert_without_dialog` から新しい 3 択設定へ移行する。
    /// `ArchiveFileHandling::Legacy` は serde default 専用で、load 後は残さない。
    fn migrate_legacy_archive_file_handling(&mut self) -> bool {
        if self.archive_file_handling != ArchiveFileHandling::Legacy {
            let old_without_dialog = self.archive_convert_without_dialog;
            self.archive_convert_without_dialog =
                self.archive_file_handling == ArchiveFileHandling::Convert;
            return old_without_dialog != self.archive_convert_without_dialog;
        }
        let migrated =
            ArchiveFileHandling::from_legacy_without_dialog(self.archive_convert_without_dialog);
        self.archive_file_handling = migrated;
        self.archive_convert_without_dialog = migrated == ArchiveFileHandling::Convert;
        true
    }

    /// v0.9.0 開発初期版の単一 VST3 プラグイン形式 (`vst3_plugin_path` + `vst3_plugin_state`)
    /// から Vec 形式 (`vst3_plugins`) への migration。
    /// 一度実行されたら旧フィールドは None にクリアし、次回 save で settings.json から消える。
    /// 戻り値: migration が発生したか (= save が必要か)。
    /// 旧 v0.8.x 以前: `video_loop: bool` だけだった。新 enum `video_loop_mode` が
    /// Default (Off) のまま旧 bool が true なら Full に昇格する片方向 migration。
    /// **load() からだけ呼ぶ** (sanitize は冪等にするためこのロジックは sanitize に置かない)。
    fn migrate_legacy_video_loop(&mut self) -> bool {
        if self.video_loop_mode == VideoLoopMode::Off && self.video_loop {
            self.video_loop_mode = VideoLoopMode::Full;
            return true;
        }
        false
    }

    fn migrate_vst3_legacy(&mut self) -> bool {
        if self.vst3_plugin_path.is_none() {
            return false;
        }
        // 既に新形式に値があるなら旧形式のクリアだけ行う (= 新形式が source of truth)。
        if self.vst3_plugins.is_empty() {
            if let Some(path) = self.vst3_plugin_path.as_ref() {
                self.vst3_plugins.push(Vst3PluginEntry {
                    path: path.clone(),
                    bypass: false,
                    state: self.vst3_plugin_state.clone(),
                    user_hidden: false,
                    gui_pos: None,
                    gui_size: None,
                });
            }
        }
        self.vst3_plugin_path = None;
        self.vst3_plugin_state = None;
        true
    }

    /// 読み込んだ設定値を安全範囲に補正する (JSON 手編集で範囲外の値が入った場合の防衛)。
    /// お気に入りの UUID マイグレーションもここで行う。
    fn sanitize(&mut self) {
        // 環境設定 UI 側のレンジ (1..=30) と整合させる。
        // 下限 0 は navigate_folder_with_skip が first を評価せず Ctrl+↑↓ が
        // 事実上機能しなくなる。上限を超える値は ZIP 中身検査込みの DFS が
        // 長時間走り UI 非応答を招くので、両側クランプする。
        self.folder_skip_limit = self.folder_skip_limit.clamp(1, 30);
        if self.video_autoplay_mode == VideoAutoplayMode::OnlyFromGrid {
            self.video_autoplay = false;
            self.video_autoplay_mode = VideoAutoplayMode::Off;
        }
        // 旧 bool `video_loop` ↔ 新 enum `video_loop_mode` の同期。
        // **bool → mode の片方向 migration は `migrate_legacy_video_loop` で load 時 1 回だけ**
        // 行う (sanitize は冪等にする必要があるため — Off にしたとき毎回 Full に戻されると
        // 「ユーザーが意図的に Off にした」と「旧 bool=true のまま新 enum=Off に書き戻し」が
        // 区別できない)。ここでは mode を source of truth として bool を導出する片方向のみ。
        self.video_loop = !matches!(self.video_loop_mode, VideoLoopMode::Off);
        self.video_volume = clamp_video_volume(self.video_volume);
        self.video_playback_speed =
            crate::video::clock::clamp_playback_speed(self.video_playback_speed);
        self.ring_shortcuts.sanitize();
        self.spread_page_gap_px = self.spread_page_gap_px.min(200);
        self.continuous_reading_gap_px = self.continuous_reading_gap_px.min(200);
        self.continuous_reading_wheel_scroll_percent =
            self.continuous_reading_wheel_scroll_percent.clamp(1, 100);
        self.continuous_reading_key_scroll_percent =
            self.continuous_reading_key_scroll_percent.clamp(1, 100);
        self.continuous_reading_gamepad_scroll_percent_per_sec = self
            .continuous_reading_gamepad_scroll_percent_per_sec
            .clamp(10, 300);
        self.fullscreen_jump_percent = self
            .fullscreen_jump_percent
            .clamp(FULLSCREEN_JUMP_PERCENT_MIN, FULLSCREEN_JUMP_PERCENT_MAX);
        self.fullscreen_fixed_jump_count = self
            .fullscreen_fixed_jump_count
            .clamp(FULLSCREEN_FIXED_JUMP_MIN, FULLSCREEN_FIXED_JUMP_MAX);
        self.reading_history_limit = self
            .reading_history_limit
            .clamp(1, crate::reading_history_db::READING_HISTORY_LIMIT_MAX);
        self.fullscreen_cursor_hide_delay_secs =
            clamp_fullscreen_cursor_hide_delay_secs(self.fullscreen_cursor_hide_delay_secs);
        self.retained_final_ai_cache_max_entries = self.retained_final_ai_cache_max_entries.clamp(
            RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MIN,
            RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MAX,
        );
        self.retained_final_ai_cache_max_mib = self.retained_final_ai_cache_max_mib.clamp(
            RETAINED_FINAL_AI_CACHE_MAX_MIB_MIN,
            RETAINED_FINAL_AI_CACHE_MAX_MIB_MAX,
        );
        if self.margin_fit_enabled && matches!(self.fullscreen_fit_mode, FullscreenFitMode::Page) {
            self.fullscreen_fit_mode = FullscreenFitMode::MarginFit;
        }
        self.margin_fit_enabled = matches!(self.fullscreen_fit_mode, FullscreenFitMode::MarginFit);

        // v0.8 マイグレーション: お気に入りの UUID が nil なら発行する。
        // 旧形式 / id フィールド欠落時は deserialize で Uuid::nil() が入っているので、
        // ここで検出して新規 UUID を割り当てる。設定は次回 save で JSON に書き戻される。
        //
        // 安全性の観点: nil UUID 同士が複数あっても個別に別の UUID が割り振られる
        // (タイミングが同時でも Uuid::new_v4 は衝突しない)。
        for fav in self.favorites.iter_mut() {
            if fav.id.is_nil() {
                fav.id = Uuid::new_v4();
            }
        }
        sanitize_details_column_order(&mut self.details_column_order);
        sanitize_details_column_widths(&mut self.details_column_widths);
        self.normalize_tag_settings();
        self.normalize_facet_tag_filter();
        if self.facet_filter.edits.remove(&FacetEditFlag::AiAdjustment) {
            self.facet_filter.edits.insert(FacetEditFlag::Adjustment);
        }
    }

    fn normalize_tag_settings(&mut self) {
        let mut cleaned: Vec<TagDef> = Vec::new();
        let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for mut tag in self.tags.drain(..) {
            if tag.id.is_nil() {
                tag.id = Uuid::new_v4();
            }
            tag.name = crate::tags_db::normalize_tag_display_name(&tag.name);
            if tag.tag_key.trim().is_empty() {
                tag.tag_key = crate::tags_db::normalize_tag_key(&tag.name);
            } else {
                tag.tag_key = crate::tags_db::normalize_tag_key(&tag.tag_key);
            }
            if tag.name.is_empty() {
                tag.name = tag.tag_key.clone();
            }
            if tag.tag_key.is_empty()
                || tag.name.chars().count() > 64
                || crate::tags_db::tag_display_name_has_whitespace(&tag.name)
            {
                continue;
            }
            if let Some(&idx) = by_key.get(&tag.tag_key) {
                cleaned[idx].show_shortcut |= tag.show_shortcut;
            } else {
                by_key.insert(tag.tag_key.clone(), cleaned.len());
                cleaned.push(tag);
            }
        }
        self.tags = cleaned;
    }

    fn normalize_facet_tag_filter(&mut self) {
        if self.facet_filter.tags.is_empty() {
            return;
        }
        self.facet_filter.tags = self
            .facet_filter
            .tags
            .iter()
            .map(|tag| crate::tags_db::normalize_tag_key(tag))
            .filter(|tag| !tag.is_empty())
            .collect();
    }

    /// 環境設定ダイアログが**編集しない**フィールドを `src` から取り込む (move)。
    ///
    /// 環境設定ダイアログの「OK」押下は内部的に `self.settings = state.settings;` で
    /// 全体差し替えするが、state.settings は開いた時点のスナップショットなので、
    /// 開いている間に他ダイアログ (お気に入り編集 / タグ編集 / 補正プリセット等) や
    /// runtime (ツールバー選択 / ウィンドウ移動) で変化した値は消えてしまう。
    ///
    /// このメソッドは「環境設定 UI が触らないフィールド」を列挙し、差し替え直前に
    /// 最新値を state へ移すために使う。**新規に「環境設定 UI から触らないフィールド」を
    /// Settings に追加した場合は、ここにも追記が必要**。逆に「環境設定 UI から触る
    /// フィールド」が増えても、このメソッドには触らなくて良い。
    ///
    /// `src` 側は Vec / String など大きいフィールドは `std::mem::take` で奪うので、
    /// 呼出後は空の既定値になる (呼出元はすぐ `*self = state.settings` で捨てる想定)。
    pub fn overwrite_non_preferences_from(&mut self, src: &mut Settings) {
        // ── グリッド / ツールバー runtime 状態 ──
        self.grid_cols = src.grid_cols;
        self.grid_view_mode = src.grid_view_mode;
        self.details_sort_key = src.details_sort_key;
        self.details_sort_ascending = src.details_sort_ascending;
        self.details_size_display_mode = src.details_size_display_mode;
        self.details_timestamp_show_seconds = src.details_timestamp_show_seconds;
        self.details_column_order = src.details_column_order.clone();
        self.details_column_widths = src.details_column_widths.clone();
        self.details_show_preview = src.details_show_preview;
        self.details_show_rating = src.details_show_rating;
        self.details_show_tags = src.details_show_tags;
        self.details_show_kind = src.details_show_kind;
        self.details_show_size = src.details_show_size;
        self.details_show_modified = src.details_show_modified;
        self.details_show_created = src.details_show_created;
        self.details_show_state = src.details_show_state;
        self.details_show_image_dimensions = src.details_show_image_dimensions;
        self.details_show_video_duration = src.details_show_video_duration;
        self.details_show_video_dimensions = src.details_show_video_dimensions;
        self.details_show_video_codec = src.details_show_video_codec;
        self.facet_filter = src.facet_filter.clone();
        self.thumb_aspect = src.thumb_aspect;
        self.sort_order = src.sort_order;
        self.rating_filter = src.rating_filter;
        self.folder_tree_pane_visible = src.folder_tree_pane_visible;
        self.folder_tree_pane_width_ratio = src.folder_tree_pane_width_ratio;
        // ── ツールバー カスタマイズ (v2.0.0: 環境設定ではなくツールバー右クリックで編集) ──
        // 表示/非表示・並び順・行頭・表示形式・出す項目は、環境設定ダイアログを開いている
        // 間にも右クリックメニューから変更できる。OK 押下時に旧 snapshot で巻き戻らないよう、
        // 全て live (= App 側) の値を引き継ぐ。
        self.show_toolbar_cols = src.show_toolbar_cols;
        self.show_toolbar_aspect = src.show_toolbar_aspect;
        self.show_toolbar_sort = src.show_toolbar_sort;
        self.show_toolbar_favorites = src.show_toolbar_favorites;
        self.show_toolbar_tags = src.show_toolbar_tags;
        self.show_toolbar_folder_tree_button = src.show_toolbar_folder_tree_button;
        self.show_toolbar_bookshelf = src.show_toolbar_bookshelf;
        self.show_toolbar_rating = src.show_toolbar_rating;
        self.show_toolbar_facet_filter = src.show_toolbar_facet_filter;
        self.toolbar_cols_display = src.toolbar_cols_display;
        self.toolbar_aspect_display = src.toolbar_aspect_display;
        self.toolbar_sort_display = src.toolbar_sort_display;
        self.toolbar_favorites_display = src.toolbar_favorites_display;
        self.toolbar_tags_display = src.toolbar_tags_display;
        self.toolbar_bookshelf_display = src.toolbar_bookshelf_display;
        self.toolbar_favorites_collapsed = src.toolbar_favorites_collapsed;
        self.toolbar_tags_collapsed = src.toolbar_tags_collapsed;
        self.toolbar_bookshelf_collapsed = src.toolbar_bookshelf_collapsed;
        self.toolbar_cols_items = std::mem::take(&mut src.toolbar_cols_items);
        self.toolbar_cols_details_visible = src.toolbar_cols_details_visible;
        self.toolbar_aspect_items = std::mem::take(&mut src.toolbar_aspect_items);
        self.toolbar_aspect_auto_visible = src.toolbar_aspect_auto_visible;
        self.toolbar_sort_items = std::mem::take(&mut src.toolbar_sort_items);
        self.toolbar_section_order = std::mem::take(&mut src.toolbar_section_order);
        self.toolbar_section_new_row = std::mem::take(&mut src.toolbar_section_new_row);
        // フォルダバー (アドレス行) の表示設定も v2.0.0 で右クリックメニューへ移したため、
        // 環境設定を開いている間の変更が OK で巻き戻らないよう live 値を引き継ぐ (Codex P2)。
        self.show_toolbar_folder = src.show_toolbar_folder;
        self.show_address_bar_history_nav = src.show_address_bar_history_nav;
        self.show_address_bar_quick_folders = src.show_address_bar_quick_folders;
        self.show_toolbar_parent_button = src.show_toolbar_parent_button;
        self.show_toolbar_prev_folder = src.show_toolbar_prev_folder;
        self.show_toolbar_next_folder = src.show_toolbar_next_folder;
        self.show_address_bar_favorite_button = src.show_address_bar_favorite_button;
        self.show_address_bar_history_menu = src.show_address_bar_history_menu;
        self.show_address_bar_folder_pin = src.show_address_bar_folder_pin;
        // ── サムネイル画質 (A/B 比較ダイアログで編集) ──
        self.thumb_px = src.thumb_px;
        // テキスト編集中プレビュー解像度 (環境設定外の Ctrl+T 左パネルで編集)。環境設定 OK の
        // 全体差し替えで巻き戻らないよう live 値を引き継ぐ (Codex P3)。
        self.text_preview_scale = src.text_preview_scale;
        self.thumb_quality = src.thumb_quality;
        // ── キャッシュ系 (環境設定に出ていない項目) ──
        self.cache_videos_always = src.cache_videos_always;
        self.batch_cache_zip_contents = src.batch_cache_zip_contents;
        self.batch_cache_pdf_contents = src.batch_cache_pdf_contents;
        // ── ウィンドウ / ナビゲーション状態 ──
        self.last_folder = src.last_folder.take();
        self.recent_folders = std::mem::take(&mut src.recent_folders);
        self.quick_folder_recent_folders = std::mem::take(&mut src.quick_folder_recent_folders);
        self.quick_folder_slots = std::mem::take(&mut src.quick_folder_slots);
        self.window_pos = src.window_pos;
        self.window_size = src.window_size;
        self.detached_viewer_enabled = src.detached_viewer_enabled;
        self.detached_viewer_window_placement = src.detached_viewer_window_placement;
        // ── お気に入り / タグ (専用ダイアログで編集) ──
        self.favorites = std::mem::take(&mut src.favorites);
        self.tags = std::mem::take(&mut src.tags);
        // ── 検索インデックス関連 ──
        self.search_index_checks = std::mem::take(&mut src.search_index_checks);
        // ── 製本 runtime 選択 ──
        self.active_book_name = std::mem::take(&mut src.active_book_name);
        self.pinned_books = std::mem::take(&mut src.pinned_books);
        // ── 「アプリケーションで開く」履歴 ──
        self.recent_open_with_apps = std::mem::take(&mut src.recent_open_with_apps);
        self.custom_open_with_apps = std::mem::take(&mut src.custom_open_with_apps);
        // ── AI アップスケール runtime 選択 ──
        self.ai_upscale_enabled = src.ai_upscale_enabled;
        self.ai_upscale_model_override = src.ai_upscale_model_override.take();
        // ── 補正プリセット (フルスクリーン `P` / スロットダイアログで編集) ──
        self.global_preset = std::mem::take(&mut src.global_preset);
        self.preset_slots = std::mem::take(&mut src.preset_slots);
        // ── VST3 プラグイン (環境設定→VST3 プラグインページで編集) ──
        // ⚠️ `vst3_plugins` の **構造 (= path / 順序)** は preferences が source of truth
        // (= 旧設計では管理ウィンドウで編集していたが、現設計では preferences で編集する)。
        // 全置換すると preferences で追加したプラグインが OK 押下時に消えるバグが
        // あった (= 2026-04 報告)。
        //
        // ただし **runtime 変動フィールド** (= `bypass` / `user_hidden`) は再生中パネル
        // (vst3_manager.rs) で変わるため、preferences を開いている間にも値が更新される。
        // これらは self (= App) 側が最新値を持っているので、path 一致で entry を引いて
        // self → state へ移送する。これで preferences OK で巻き戻る不具合を回避する
        // (Codex P3 2026-05-01)。
        // legacy migration field (deprecated path/state) のみ App 側を残す。
        for entry in self.vst3_plugins.iter_mut() {
            if let Some(latest) = src.vst3_plugins.iter().find(|e| e.path == entry.path) {
                entry.bypass = latest.bypass;
                entry.user_hidden = latest.user_hidden;
                // gui_pos / gui_size / state は runtime 側で更新される field なので、
                // preferences ダイアログを開いている間に変わった最新値を採用する。
                if latest.gui_pos.is_some() {
                    entry.gui_pos = latest.gui_pos;
                }
                if latest.gui_size.is_some() {
                    entry.gui_size = latest.gui_size;
                }
                if latest.state.is_some() {
                    entry.state = latest.state.clone();
                }
            }
        }
        self.vst3_plugin_path = src.vst3_plugin_path.take();
        self.vst3_plugin_state = src.vst3_plugin_state.take();
        // `vst3_gui_visible` は VST ボタン runtime トグルで変わるので App 側を保つ。
        // `vst3_video_compact` / `vst3_panel_pos` も同様 (= プレイバックパネルで切替)。
        self.vst3_gui_visible = src.vst3_gui_visible;
        self.vst3_video_compact = src.vst3_video_compact;
        self.vst3_panel_pos = src.vst3_panel_pos;
        self.vst3_chain_slots = std::mem::take(&mut src.vst3_chain_slots);
    }

    /// 設定を永続化する (Phase 3: SQLite ベース、user save 用)。
    ///
    /// プロセス内最初の user save で 1 回だけ `settings.db.bak1..bak10` を世代ローテし
    /// (`BACKUP_DONE_THIS_SESSION`)、それ以降は in-place で `save_full` のみ実行する。
    /// `MAIN_UNREADABLE_THIS_SESSION` または `settings_db::save_suppressed()` が立って
    /// いれば一切書き込まない。
    ///
    /// Phase 6 (2026-05-14) で `MIV_SETTINGS_SAVE_TRACE` の Phase 0 計装は削除済み。
    /// Phase 0 の実測結果から hot-path upsert API (e.g. `upsert_video_resume_position`) が
    /// 必要かどうかは future Phase 7 で検討する。
    pub fn save(&self) {
        self.save_internal(/* allow_rotation = */ true);
    }

    /// `Settings::load` 内部の writeback (migration / version_changed) 専用の保存経路
    /// (Codex P2 v13 2026-05-14)。**世代 rotation を発火させない / `BACKUP_DONE_THIS_SESSION`
    /// flag も立てない** ことで、spec §6.1 の「プロセス最初の **user save** で 1 回 rotate」
    /// 規約を維持する。
    fn save_internal_no_rotation(&self) {
        self.save_internal(false);
    }

    fn save_internal(&self, allow_rotation: bool) {
        // session-wide 抑止フラグ。
        // - `MAIN_UNREADABLE_THIS_SESSION`: settings.rs 上の抑止 (旧来から維持)
        // - `settings_db::save_suppressed()`: settings_db 側の抑止 (Phase 2 で追加)
        // どちらか一つでも立っていたら書込まない。
        if MAIN_UNREADABLE_THIS_SESSION.load(Ordering::Relaxed)
            || crate::settings_db::save_suppressed()
        {
            settings_diag_log("settings: save suppressed (session-wide flag set)");
            return;
        }
        // 保存直前に旧フィールドを新フィールドから導出する。
        // self は &self なので clone してから書き換える。
        let snapshot = {
            let mut s = self.clone();
            s.video_loop = !matches!(s.video_loop_mode, VideoLoopMode::Off);
            let archive_handling = s.archive_file_handling_resolved();
            s.archive_file_handling = archive_handling;
            s.archive_convert_without_dialog = archive_handling == ArchiveFileHandling::Convert;
            s
        };

        let data_dir = crate::data_dir::get();
        // Codex P2 v13 (2026-05-14): allow_rotation == false のときは
        // `BACKUP_DONE_THIS_SESSION` を **触らずに** rotation を skip する。
        // load() 内の migration / version_changed 用 bootstrap save が次回の user save の
        // rotation を消費してしまう事故 (= spec §6.1 違反) を防ぐ。
        let did_rotate = if allow_rotation {
            !BACKUP_DONE_THIS_SESSION.swap(true, Ordering::Relaxed)
        } else {
            false
        };

        // 全体を with_db_result でラップする。global handle が無ければ即 Err。
        let result = crate::settings_db::with_db_result(|db| {
            if did_rotate {
                // プロセス内最初の user save: 世代 rotation を一度だけ走らせる。
                // 失敗してもアプリの動作は継続するため log のみで吸収し、後続の
                // save_full は実行する (= "bak ロテで失敗しても本体は保存" のセマンティクス)。
                if let Err(e) = db.rotate_backups(&data_dir) {
                    settings_diag_log(&format!(
                        "settings: rotate_backups failed (continuing with save_full): {e}"
                    ));
                }
            }
            db.save_full(&snapshot)
        });

        match result {
            Ok(()) => {
                settings_diag_log(&format!(
                    "settings: save ok: favorites={} rotated={did_rotate}",
                    snapshot.favorites.len(),
                ));
            }
            Err(e) => {
                // SaveSuppressed のときは設計通り (= 上の suppress チェックを擦り抜けて
                // boot 経路の Failed 状態に当たったケース)。verbose 化しない。
                if !matches!(e, crate::settings_db::SettingsDbError::SaveSuppressed) {
                    eprintln!("settings save failed: {e}");
                    settings_diag_log(&format!(
                        "settings: save failed: {e} (rotated={did_rotate})"
                    ));
                }
            }
        }
    }

    /// 指定パスが既にお気に入り (重複) に登録されているかを返す。
    pub fn is_favorite(&self, path: &std::path::Path) -> bool {
        self.favorites.iter().any(|f| f.path == path)
    }

    /// 任意の表示名でお気に入りに追加する（重複・上限チェック付き）。
    /// 追加された場合 true を返す。UUID は自動発行、index フラグは全 false。
    pub fn add_favorite(&mut self, name: String, path: PathBuf) -> bool {
        if self.is_favorite(&path) {
            return false;
        }
        if self.favorites.len() >= MAX_FAVORITES {
            return false;
        }
        self.favorites.push(FavoriteEntry::new(name, path));
        true
    }

    /// 「アプリケーションで開く」で使用したアプリを履歴に記録する。
    /// 同じ exe_path が既にあれば先頭に移動。最大3件。
    pub fn record_recent_open_with(&mut self, display_name: String, exe_path: String) {
        const MAX_RECENT_OPEN_WITH: usize = 3;
        self.recent_open_with_apps
            .retain(|a| !a.exe_path.eq_ignore_ascii_case(&exe_path));
        self.recent_open_with_apps.insert(
            0,
            RecentApp {
                display_name,
                exe_path,
            },
        );
        self.recent_open_with_apps.truncate(MAX_RECENT_OPEN_WITH);
    }
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Toolbar section order (v2.0.0 Phase 1) --

    #[test]
    fn toolbar_section_order_empty_is_default() {
        let got = ToolbarSectionId::ordered_with_fallback(&[]);
        assert_eq!(got, ToolbarSectionId::default_order().to_vec());
    }

    #[test]
    fn toolbar_section_order_appends_missing_in_default_order() {
        // 保存順が一部だけ (Tags を先頭へ) でも、残りは既定順で末尾補完される。
        let saved = vec![ToolbarSectionId::Tags, ToolbarSectionId::Cols];
        let got = ToolbarSectionId::ordered_with_fallback(&saved);
        assert_eq!(got[0], ToolbarSectionId::Tags);
        assert_eq!(got[1], ToolbarSectionId::Cols);
        // 全 8 セクションが過不足なく 1 回ずつ含まれる。
        assert_eq!(got.len(), ToolbarSectionId::default_order().len());
        for &id in ToolbarSectionId::default_order() {
            assert_eq!(
                got.iter().filter(|&&x| x == id).count(),
                1,
                "{id:?} は 1 回だけ含まれるべき"
            );
        }
    }

    #[test]
    fn toolbar_section_order_dedups_corrupt_duplicates() {
        let saved = vec![
            ToolbarSectionId::Cols,
            ToolbarSectionId::Cols,
            ToolbarSectionId::Cols,
        ];
        let got = ToolbarSectionId::ordered_with_fallback(&saved);
        assert_eq!(got.len(), ToolbarSectionId::default_order().len());
        assert_eq!(got[0], ToolbarSectionId::Cols);
        assert_eq!(
            got.iter().filter(|&&x| x == ToolbarSectionId::Cols).count(),
            1
        );
    }

    #[test]
    fn toolbar_section_order_drops_unknown_future_variants() {
        // 将来バージョンの未知セクションは描画順から除外される (forward-compat)。
        let saved = vec![
            ToolbarSectionId::Tags,
            ToolbarSectionId::Unknown,
            ToolbarSectionId::Cols,
        ];
        let got = ToolbarSectionId::ordered_with_fallback(&saved);
        assert!(
            !got.contains(&ToolbarSectionId::Unknown),
            "Unknown は除外されるべき"
        );
        assert_eq!(got.len(), ToolbarSectionId::default_order().len());
        assert_eq!(got[0], ToolbarSectionId::Tags);
        assert_eq!(got[1], ToolbarSectionId::Cols);
    }

    #[test]
    fn toolbar_section_id_deserializes_unknown_tolerantly() {
        // 旧バイナリ × 新データ: 未知の variant 名はエラーにならず Unknown になる。
        let v: Vec<ToolbarSectionId> =
            serde_json::from_str(r#"["Cols","FutureSection","Tags"]"#).unwrap();
        assert_eq!(
            v,
            vec![
                ToolbarSectionId::Cols,
                ToolbarSectionId::Unknown,
                ToolbarSectionId::Tags,
            ]
        );
    }

    #[test]
    fn show_toolbar_cols_aspect_sort_default_true_when_missing() {
        // v2.0.0 で追加した表示フラグは、旧 settings JSON (フィールド無し) を読んだとき
        // `default_true` で true になる。false にすると既存ユーザーで列/比率/ソートが
        // 消える退行になるため、欠落 = 表示 (true) を担保する。
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.show_toolbar_cols);
        assert!(s.show_toolbar_aspect);
        assert!(s.show_toolbar_sort);
        assert!(
            s.toolbar_section_new_row.is_empty(),
            "新規 new_row 集合は欠落時 空"
        );
    }

    #[test]
    fn toolbar_section_display_unknown_is_tolerant() {
        // 旧バイナリ × 新データ: 未知の表示形式はエラーにならず Unknown (= 展開扱い)。
        let v: ToolbarSectionDisplay = serde_json::from_str("\"FutureMode\"").unwrap();
        assert_eq!(v, ToolbarSectionDisplay::Unknown);
        assert_eq!(v.label(), "展開");
    }

    #[test]
    fn toolbar_section_display_known_variants_roundtrip() {
        for m in ToolbarSectionDisplay::all_with_collapsible() {
            let s = serde_json::to_string(m).unwrap();
            let back: ToolbarSectionDisplay = serde_json::from_str(&s).unwrap();
            assert_eq!(*m, back);
        }
    }

    // -- Settings defaults --

    #[test]
    fn settings_default_values() {
        let s = Settings::default();
        assert_eq!(s.grid_cols, 4);
        assert_eq!(s.thumb_aspect, ThumbAspect::Square);
        assert!(s.favorites.is_empty());
        assert!(s.last_folder.is_none());
        assert_eq!(s.startup_folder_mode, StartupFolderMode::Previous);
        assert!(s.startup_folder_path.is_none());
        assert!(s.reading_history_enabled);
        assert_eq!(
            s.reading_history_limit,
            crate::reading_history_db::READING_HISTORY_LIMIT_DEFAULT
        );
        assert!(s.recent_folders.is_empty());
        assert_eq!(s.quick_folder_slots, [None, None]);
        assert!(s.window_pos.is_none());
        assert!(s.window_size.is_none());
        assert_eq!(s.prefetch_back, 4);
        assert_eq!(s.prefetch_forward, 12);
        assert_eq!(
            s.retained_final_ai_cache_max_entries,
            RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_DEFAULT
        );
        assert_eq!(
            s.retained_final_ai_cache_max_mib,
            RETAINED_FINAL_AI_CACHE_MAX_MIB_DEFAULT
        );
        assert_eq!(s.folder_skip_limit, 5);
        assert_eq!(s.sort_order, SortOrder::FileName);
        assert_eq!(s.thumb_px, 512);
        assert_eq!(s.thumb_quality, 75);
        assert_eq!(s.cache_policy, CachePolicy::Auto);
        assert_eq!(s.cache_threshold_ms, 25);
        assert_eq!(s.cache_size_threshold_bytes, 2_000_000);
        assert!(s.cache_videos_always);
        assert!(s.cache_webp_always);
        assert_eq!(s.archive_cache_max_bytes, 0);
        assert_eq!(s.archive_file_handling, ArchiveFileHandling::Ask);
        assert!(!s.archive_convert_without_dialog);
        assert_eq!(s.thumb_prev_pages, 2);
        assert_eq!(s.thumb_next_pages, 4);
        assert_eq!(s.thumb_vram_cap_percent, 50);
        assert!(s.thumb_idle_upgrade);
        assert_eq!(s.spread_page_gap_px, 4);
        assert_eq!(s.continuous_reading_gap_px, 20);
        assert_eq!(s.fullscreen_fit_mode, FullscreenFitMode::Page);
        assert!(!s.fullscreen_seek_bar_locked);
        assert!(s.fullscreen_page_number_overlay);
        assert!(!s.fullscreen_keep_on_app_switch);
        assert_eq!(
            s.fullscreen_cursor_hide_delay_secs,
            FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS
        );
        assert_eq!(s.fullscreen_jump_mode, FullscreenJumpMode::Percent);
        assert_eq!(s.fullscreen_jump_percent, FULLSCREEN_JUMP_PERCENT_DEFAULT);
        assert_eq!(s.fullscreen_fixed_jump_count, 10);
        assert_eq!(s.continuous_reading_wheel_scroll_percent, 20);
        assert_eq!(s.continuous_reading_key_scroll_percent, 16);
        assert_eq!(s.continuous_reading_gamepad_scroll_percent_per_sec, 130);
        assert!(s.show_toolbar_favorites);
        assert!(s.show_toolbar_tags);
        assert!(!s.folder_tree_pane_visible);
        assert_eq!(
            s.folder_tree_pane_width_ratio,
            default_folder_tree_pane_width_ratio()
        );
        assert!(s.show_toolbar_folder);
        assert!(s.show_toolbar_folder_tree_button);
        assert!(s.show_toolbar_bookshelf);
        assert!(s.show_address_bar_history_nav);
        assert!(s.show_address_bar_quick_folders);
        assert!(s.show_toolbar_parent_button);
        assert!(s.show_toolbar_prev_folder);
        assert!(s.show_toolbar_next_folder);
        assert!(s.show_toolbar_rating);
        assert!(s.show_toolbar_facet_filter);
        assert!(s.show_address_bar_favorite_button);
        assert!(s.show_address_bar_history_menu);
        assert!(s.show_address_bar_folder_pin);
        assert!(s.use_native_shell_context_menu);
        assert!(!s.first_setup_completed);
        assert_eq!(s.ai_feature_mode, AiFeatureMode::Light);
    }

    /// 旧設定 (`ai_upscale_skip_px` のみ、新フィールドなし) は `N x N` として
    /// 読み替えられ、新フィールドがあればそちらが優先される
    /// (docs/ai-processing-size-threshold-plan.md)。
    #[test]
    fn ai_size_limit_falls_back_to_legacy_skip_px() {
        use crate::ai::upscale::AiProcessSizeLimit;
        let mut s = Settings::default();
        s.ai_upscale_skip_px = 1024;
        s.ai_denoise_skip_px = 512;
        assert_eq!(s.ai_upscale_limit(), AiProcessSizeLimit::square(1024));
        assert_eq!(s.ai_denoise_limit(), AiProcessSizeLimit::square(512));

        let limit = AiProcessSizeLimit {
            long_edge_px: 4096,
            short_edge_px: 2048,
        };
        s.ai_upscale_size_limit = Some(limit);
        assert_eq!(s.ai_upscale_limit(), limit);
        // denoise 側は引き続き旧値から読み替え
        assert_eq!(s.ai_denoise_limit(), AiProcessSizeLimit::square(512));
    }

    /// 新フィールドが無い JSON (旧バージョンが保存した設定) を deserialize すると
    /// `None` になり、旧しきい値 `2048` 相当の挙動が維持される (serde 互換ガード)。
    #[test]
    fn ai_size_limit_deserializes_as_none_from_legacy_json() {
        let mut v = serde_json::to_value(Settings::default()).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("ai_upscale_size_limit");
        obj.remove("ai_denoise_size_limit");
        obj.insert("ai_upscale_skip_px".into(), 1024.into());
        let s: Settings = serde_json::from_value(v).unwrap();
        assert!(s.ai_upscale_size_limit.is_none());
        assert_eq!(
            s.ai_upscale_limit(),
            crate::ai::upscale::AiProcessSizeLimit::square(1024)
        );
        assert_eq!(
            s.ai_denoise_limit(),
            crate::ai::upscale::AiProcessSizeLimit::square(2048)
        );
    }

    #[test]
    fn spread_cycle_loops_through_keys_1_to_5() {
        // ゲームパッド Select / 見開きトグルはキーボード 1〜5 と同じ順で巡回する。
        assert_eq!(SpreadMode::Single.next_in_spread_cycle(), SpreadMode::Ltr);
        assert_eq!(SpreadMode::Ltr.next_in_spread_cycle(), SpreadMode::LtrCover);
        assert_eq!(SpreadMode::LtrCover.next_in_spread_cycle(), SpreadMode::Rtl);
        assert_eq!(SpreadMode::Rtl.next_in_spread_cycle(), SpreadMode::RtlCover);
        // 末尾 RtlCover からは先頭 Single へループ。
        assert_eq!(
            SpreadMode::RtlCover.next_in_spread_cycle(),
            SpreadMode::Single
        );
        // 巡回外モード (Vertical) からは先頭 Single へ。
        assert_eq!(
            SpreadMode::Vertical.next_in_spread_cycle(),
            SpreadMode::Single
        );
    }

    #[test]
    fn spread_mode_with_reading_direction_preserves_cover_phase() {
        assert_eq!(
            SpreadMode::Ltr.with_reading_direction(ReadingDirection::Rtl),
            SpreadMode::Rtl
        );
        assert_eq!(
            SpreadMode::Rtl.with_reading_direction(ReadingDirection::Ltr),
            SpreadMode::Ltr
        );
        assert_eq!(
            SpreadMode::LtrCover.with_reading_direction(ReadingDirection::Rtl),
            SpreadMode::RtlCover
        );
        assert_eq!(
            SpreadMode::RtlCover.with_reading_direction(ReadingDirection::Ltr),
            SpreadMode::LtrCover
        );
        assert_eq!(
            SpreadMode::Single.with_reading_direction(ReadingDirection::Rtl),
            SpreadMode::Single
        );
        assert_eq!(ReadingDirection::Ltr.next(), ReadingDirection::Rtl);
        assert_eq!(ReadingDirection::Rtl.next(), ReadingDirection::Ltr);
    }

    #[test]
    fn fullscreen_fit_mode_cycles_exclude_legacy_margin_fit() {
        assert_eq!(
            FullscreenFitMode::default_for_flow(ReadingFlow::Paged),
            FullscreenFitMode::Page
        );
        assert_eq!(
            FullscreenFitMode::default_for_flow(ReadingFlow::Vertical),
            FullscreenFitMode::Width
        );
        assert_eq!(
            FullscreenFitMode::default_for_flow(ReadingFlow::Horizontal),
            FullscreenFitMode::Height
        );
        assert_eq!(
            FullscreenFitMode::Page.next_for_flow(ReadingFlow::Paged),
            FullscreenFitMode::Width
        );
        assert_eq!(
            FullscreenFitMode::Page.next_for_flow(ReadingFlow::Vertical),
            FullscreenFitMode::Width
        );
        assert_eq!(
            FullscreenFitMode::MarginFit.effective_for_flow(ReadingFlow::Horizontal),
            FullscreenFitMode::Page
        );
    }

    #[test]
    fn fit_mode_menu_list_matches_flow() {
        // 余白カットフィットは表示トリムへ移行したため、ページ表示でも候補から外す。
        assert_eq!(
            FullscreenFitMode::selectable_for_flow(ReadingFlow::Paged),
            FullscreenFitMode::all()
        );
        for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
            let modes = FullscreenFitMode::selectable_for_flow(flow);
            assert_eq!(modes.len(), 4);
            assert!(!modes.contains(&FullscreenFitMode::MarginFit));
            // メニュー一覧は [0] 循環の対象集合と一致する。
            for &m in modes {
                assert!(modes.contains(&m.next_for_flow(flow)));
            }
        }
    }

    #[test]
    fn ai_feature_mode_limits_models_without_destroying_saved_choices() {
        use crate::ai::{ImageCategory, ModelKind};

        assert!(!AiFeatureMode::Disabled.allows_upscale_model(ModelKind::UpscaleRealCugan4x));
        assert!(!AiFeatureMode::Disabled.allows_denoise());
        assert_eq!(
            AiFeatureMode::Disabled.auto_upscale_model(ImageCategory::Comic),
            None
        );

        assert!(AiFeatureMode::Light.allows_upscale_model(ModelKind::UpscaleRealEsrGeneralV3));
        assert!(AiFeatureMode::Light.allows_upscale_model(ModelKind::UpscaleRealCugan4x));
        assert!(!AiFeatureMode::Light.allows_upscale_model(ModelKind::UpscaleRealEsrganX4Plus));
        assert!(!AiFeatureMode::Light.allows_denoise());
        assert_eq!(
            AiFeatureMode::Light.auto_upscale_model(ImageCategory::Comic),
            Some(ModelKind::UpscaleRealCugan4x)
        );
        assert_eq!(
            AiFeatureMode::Light.auto_upscale_model(ImageCategory::RealLife),
            Some(ModelKind::UpscaleRealEsrGeneralV3)
        );

        assert!(
            AiFeatureMode::HighQuality.allows_upscale_model(ModelKind::UpscaleRealEsrganX4Plus)
        );
        assert!(AiFeatureMode::HighQuality.allows_denoise());
        assert_eq!(
            AiFeatureMode::HighQuality.auto_upscale_model(ImageCategory::Illustration),
            Some(ImageCategory::Illustration.preferred_upscale_model())
        );
    }

    // -- Settings JSON roundtrip --

    #[test]
    fn settings_roundtrip_json() {
        let mut original = Settings::default();
        original.startup_folder_mode = StartupFolderMode::Specific;
        original.startup_folder_path = Some(PathBuf::from(r"D:\Images"));
        original.recent_folders = vec![
            PathBuf::from(r"D:\Images"),
            PathBuf::from(r"C:\Users\test\Pictures"),
        ];
        original.quick_folder_slots = [
            Some(PathBuf::from(r"D:\Images\Source")),
            Some(PathBuf::from(r"E:\Archive\Dest")),
        ];
        original.ring_shortcuts.mouse_flick_enabled = true;
        original.ring_shortcuts.gamepad_ring_enabled = false;
        original.ring_shortcuts.shift_wheel_pair =
            crate::ring_shortcut::WheelPairActionId::ZoomInOut;
        original.ring_shortcuts.alt_wheel_pair =
            crate::ring_shortcut::WheelPairActionId::FolderHistoryPrevNext;
        original.ring_shortcuts.mouse_buttons_grid.back =
            crate::ring_shortcut::RingActionId::GridParentFolder;
        original.ring_shortcuts.mouse_buttons_image.forward =
            crate::ring_shortcut::RingActionId::ImageSlideshow;
        original.ring_shortcuts.mouse_buttons_video.back =
            crate::ring_shortcut::RingActionId::VideoMute;
        original.ring_shortcuts.mouse_nav_prompt_done = true;
        original.ring_shortcuts.grid.slots[0] = crate::ring_shortcut::RingActionId::GridHistoryBack;
        let json = serde_json::to_string(&original).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.grid_cols, original.grid_cols);
        assert_eq!(loaded.thumb_px, original.thumb_px);
        assert_eq!(loaded.thumb_quality, original.thumb_quality);
        assert_eq!(loaded.cache_threshold_ms, original.cache_threshold_ms);
        assert_eq!(loaded.prefetch_back, original.prefetch_back);
        assert_eq!(
            loaded.retained_final_ai_cache_max_entries,
            original.retained_final_ai_cache_max_entries
        );
        assert_eq!(
            loaded.retained_final_ai_cache_max_mib,
            original.retained_final_ai_cache_max_mib
        );
        assert_eq!(loaded.startup_folder_mode, original.startup_folder_mode);
        assert_eq!(loaded.startup_folder_path, original.startup_folder_path);
        assert_eq!(loaded.recent_folders, original.recent_folders);
        assert_eq!(loaded.quick_folder_slots, original.quick_folder_slots);
        assert_eq!(loaded.ring_shortcuts, original.ring_shortcuts);
    }

    #[test]
    fn numeric_sort_tiebreaks_on_lowercase_filename() {
        // 記号差のみのファイル名は natural key が一致するので、tiebreak が
        // 無いと FS の `read_dir` 列挙順依存になる。`SortOrder::Numeric` は
        // ファイル名 lowercase の昇順で安定化させる。
        use crate::ui_helpers::natural_sort_key;
        let mut names = vec![
            "foobar1.jpg",
            "foo-bar1.jpg",
            "foo bar1.jpg",
            "foo#bar1.jpg",
        ];
        names.sort_by(|a, b| SortOrder::Numeric.compare(a, 0, b, 0, natural_sort_key));
        // ASCII: ' ' (0x20) < '#' (0x23) < '-' (0x2D) < 'b' (0x62)
        assert_eq!(
            names,
            vec![
                "foo bar1.jpg",
                "foo#bar1.jpg",
                "foo-bar1.jpg",
                "foobar1.jpg",
            ]
        );
    }

    #[test]
    fn numeric_sort_groups_hash_and_plain_numbers_together() {
        // `#1.jpg` と `1.jpg` の natural key を一致させ、tiebreak で並ぶ。
        // ASCII: '#' (0x23) < '1' (0x31) なので `#1.jpg` が先。
        use crate::ui_helpers::natural_sort_key;
        let mut names = vec!["1.jpg", "#1.jpg", "2.jpg"];
        names.sort_by(|a, b| SortOrder::Numeric.compare(a, 0, b, 0, natural_sort_key));
        assert_eq!(names, vec!["#1.jpg", "1.jpg", "2.jpg"]);
    }

    #[test]
    fn settings_missing_fields_use_defaults() {
        let loaded: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.grid_cols, 4);
        assert_eq!(loaded.thumb_px, 512);
        assert_eq!(loaded.thumb_quality, 75);
        assert_eq!(loaded.video_volume, VIDEO_VOLUME_DEFAULT);
        assert_eq!(loaded.video_playback_speed, 1.0);
        assert!(!loaded.ring_shortcuts.mouse_flick_enabled);
        assert!(loaded.ring_shortcuts.gamepad_ring_enabled);
        assert_eq!(
            loaded.ring_shortcuts.shift_wheel_pair,
            crate::ring_shortcut::WheelPairActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.alt_wheel_pair,
            crate::ring_shortcut::WheelPairActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_back_forward_action,
            crate::ring_shortcut::MouseBackForwardActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_buttons_grid.back,
            crate::ring_shortcut::RingActionId::GridHistoryBack
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_buttons_grid.forward,
            crate::ring_shortcut::RingActionId::GridHistoryForward
        );
        assert!(!loaded.ring_shortcuts.mouse_nav_prompt_done);
        assert_eq!(
            loaded.ring_shortcuts.grid.slots[crate::ring_shortcut::RingDirection::Up.slot_index()],
            crate::ring_shortcut::RingActionId::GridParentFolder
        );
        assert_eq!(
            loaded.ring_shortcuts.grid.slots
                [crate::ring_shortcut::RingDirection::DownLeft.slot_index()],
            crate::ring_shortcut::RingActionId::AddToBook
        );
        assert_eq!(
            loaded.ring_shortcuts.grid.slots
                [crate::ring_shortcut::RingDirection::DownRight.slot_index()],
            crate::ring_shortcut::RingActionId::GridToggleCheck
        );
        assert_eq!(
            loaded.ring_shortcuts.image.slots[crate::ring_shortcut::RingDirection::Up.slot_index()],
            crate::ring_shortcut::RingActionId::ImageSlideshow
        );
        assert_eq!(
            loaded.ring_shortcuts.image.slots
                [crate::ring_shortcut::RingDirection::DownLeft.slot_index()],
            crate::ring_shortcut::RingActionId::AddToBook
        );
        assert_eq!(
            loaded.ring_shortcuts.image.slots
                [crate::ring_shortcut::RingDirection::UpLeft.slot_index()],
            crate::ring_shortcut::RingActionId::ImageCapture
        );
        assert_eq!(
            loaded.ring_shortcuts.video.slots[crate::ring_shortcut::RingDirection::Up.slot_index()],
            crate::ring_shortcut::RingActionId::VideoLoop
        );
        assert_eq!(
            loaded.ring_shortcuts.video.slots
                [crate::ring_shortcut::RingDirection::DownLeft.slot_index()],
            crate::ring_shortcut::RingActionId::AddToBook
        );
        assert_eq!(
            loaded.ring_shortcuts.video.slots
                [crate::ring_shortcut::RingDirection::UpLeft.slot_index()],
            crate::ring_shortcut::RingActionId::VideoCapture
        );
        assert_eq!(loaded.fullscreen_fit_mode, FullscreenFitMode::Page);
        assert_eq!(loaded.fullscreen_jump_mode, FullscreenJumpMode::Percent);
        assert!(!loaded.fullscreen_keep_on_app_switch);
        assert_eq!(
            loaded.fullscreen_jump_percent,
            FULLSCREEN_JUMP_PERCENT_DEFAULT
        );
        assert_eq!(loaded.fullscreen_fixed_jump_count, 10);
        assert_eq!(loaded.continuous_reading_wheel_scroll_percent, 20);
        assert_eq!(loaded.continuous_reading_key_scroll_percent, 16);
        assert_eq!(
            loaded.continuous_reading_gamepad_scroll_percent_per_sec,
            130
        );
        assert_eq!(
            loaded.video_continuous_mode,
            crate::video::VideoContinuousMode::Off
        );
        assert_eq!(
            loaded.retained_final_ai_cache_max_entries,
            RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_DEFAULT
        );
        assert_eq!(
            loaded.retained_final_ai_cache_max_mib,
            RETAINED_FINAL_AI_CACHE_MAX_MIB_DEFAULT
        );
        assert!(!loaded.video_muted);
        assert_eq!(loaded.video_deinterlace, VideoDeinterlaceMode::Auto);
        assert!(!loaded.video_grid_open_starts_from_beginning);
        assert!(loaded.favorites.is_empty());
    }

    #[test]
    fn ring_shortcuts_sanitize_unknown_and_context_mismatch() {
        let json = r#"{
            "ring_shortcuts": {
                "mouse_flick_enabled": true,
                "gamepad_ring_enabled": false,
                "shift_wheel_pair": "future_wheel",
                "alt_wheel_pair": "zoom_in_out",
                "mouse_back_forward_action": "future_mouse_nav",
                "mouse_buttons_grid": { "back": "future_mouse_button", "forward": "grid_history_forward" },
                "mouse_buttons_image": { "back": "grid_history_back", "forward": "video_capture" },
                "mouse_buttons_video": { "back": "image_capture", "forward": "grid_history_forward" },
                "grid": { "slots": ["future_action", "video_capture"] },
                "image": { "slots": ["video_capture"] },
                "video": { "slots": ["image_capture"] }
            }
        }"#;
        let mut loaded: Settings = serde_json::from_str(json).unwrap();
        loaded.sanitize();

        assert!(loaded.ring_shortcuts.mouse_flick_enabled);
        assert!(loaded.ring_shortcuts.gamepad_ring_enabled);
        assert_eq!(
            loaded.ring_shortcuts.shift_wheel_pair,
            crate::ring_shortcut::WheelPairActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.alt_wheel_pair,
            crate::ring_shortcut::WheelPairActionId::ZoomInOut
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_back_forward_action,
            crate::ring_shortcut::MouseBackForwardActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_buttons_grid.back,
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_buttons_image.forward,
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.mouse_buttons_video.back,
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.grid.slots.len(),
            crate::ring_shortcut::RING_SHORTCUT_SLOT_COUNT
        );
        assert_eq!(
            loaded.ring_shortcuts.grid.slots[0],
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.grid.slots[1],
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.image.slots[0],
            crate::ring_shortcut::RingActionId::None
        );
        assert_eq!(
            loaded.ring_shortcuts.video.slots[0],
            crate::ring_shortcut::RingActionId::None
        );
    }

    /// JSON 手編集等で `folder_skip_limit` が UI レンジ (1..=30) 外に
    /// なっていれば sanitize でクランプされる。下限 0 は Ctrl+↑↓ が
    /// 機能しなくなり、上限超過は ZIP 中身検査込みの DFS が長時間走って
    /// UI 非応答を招くため両側で防衛する。
    #[test]
    fn sanitize_clamps_folder_skip_limit() {
        let mut s = Settings::default();
        s.folder_skip_limit = 0;
        s.sanitize();
        assert_eq!(s.folder_skip_limit, 1);

        let mut s = Settings::default();
        s.folder_skip_limit = 5;
        s.sanitize();
        assert_eq!(s.folder_skip_limit, 5);

        let mut s = Settings::default();
        s.folder_skip_limit = 999;
        s.sanitize();
        assert_eq!(s.folder_skip_limit, 30);
    }

    #[test]
    fn sanitize_clamps_reading_history_limit() {
        let mut s = Settings::default();
        s.reading_history_limit = 0;
        s.sanitize();
        assert_eq!(s.reading_history_limit, 1);

        s.reading_history_limit = crate::reading_history_db::READING_HISTORY_LIMIT_MAX + 1;
        s.sanitize();
        assert_eq!(
            s.reading_history_limit,
            crate::reading_history_db::READING_HISTORY_LIMIT_MAX
        );
    }

    #[test]
    fn startup_folder_mode_unknown_falls_back_to_previous() {
        let loaded: Settings = serde_json::from_str(
            r#"{
                "startup_folder_mode": "future_mode"
            }"#,
        )
        .unwrap();
        assert_eq!(loaded.startup_folder_mode, StartupFolderMode::Previous);
    }

    #[test]
    fn sanitize_clamps_fullscreen_gap_settings() {
        let mut s = Settings::default();
        s.spread_page_gap_px = 999;
        s.continuous_reading_gap_px = 999;
        s.continuous_reading_wheel_scroll_percent = 0;
        s.continuous_reading_key_scroll_percent = 999;
        s.continuous_reading_gamepad_scroll_percent_per_sec = 999;
        s.fullscreen_jump_percent = 999;
        s.fullscreen_fixed_jump_count = 999;
        s.fullscreen_cursor_hide_delay_secs = 99.0;
        s.retained_final_ai_cache_max_entries = 999;
        s.retained_final_ai_cache_max_mib = 999_999;
        s.sanitize();
        assert_eq!(s.spread_page_gap_px, 200);
        assert_eq!(s.continuous_reading_gap_px, 200);
        assert_eq!(s.continuous_reading_wheel_scroll_percent, 1);
        assert_eq!(s.continuous_reading_key_scroll_percent, 100);
        assert_eq!(s.continuous_reading_gamepad_scroll_percent_per_sec, 300);
        assert_eq!(s.fullscreen_jump_percent, FULLSCREEN_JUMP_PERCENT_MAX);
        assert_eq!(s.fullscreen_fixed_jump_count, FULLSCREEN_FIXED_JUMP_MAX);
        assert_eq!(
            s.fullscreen_cursor_hide_delay_secs,
            FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS
        );
        assert_eq!(
            s.retained_final_ai_cache_max_entries,
            RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MAX
        );
        assert_eq!(
            s.retained_final_ai_cache_max_mib,
            RETAINED_FINAL_AI_CACHE_MAX_MIB_MAX
        );

        s.fullscreen_jump_percent = 0;
        s.fullscreen_fixed_jump_count = 0;
        s.fullscreen_cursor_hide_delay_secs = 0.0;
        s.retained_final_ai_cache_max_entries = 0;
        s.retained_final_ai_cache_max_mib = 0;
        s.sanitize();
        assert_eq!(s.fullscreen_jump_percent, FULLSCREEN_JUMP_PERCENT_MIN);
        assert_eq!(s.fullscreen_fixed_jump_count, FULLSCREEN_FIXED_JUMP_MIN);
        assert_eq!(
            s.fullscreen_cursor_hide_delay_secs,
            FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS
        );
        assert_eq!(s.retained_final_ai_cache_max_entries, 0);
        assert_eq!(s.retained_final_ai_cache_max_mib, 0);

        s.fullscreen_cursor_hide_delay_secs = f32::NAN;
        s.sanitize();
        assert_eq!(
            s.fullscreen_cursor_hide_delay_secs,
            FULLSCREEN_CURSOR_HIDE_DELAY_DEFAULT_SECS
        );
    }

    #[test]
    fn sanitize_migrates_legacy_margin_fit_bool_to_fit_mode() {
        let mut s = Settings::default();
        s.margin_fit_enabled = true;
        s.fullscreen_fit_mode = FullscreenFitMode::Page;
        s.sanitize();
        assert_eq!(s.fullscreen_fit_mode, FullscreenFitMode::MarginFit);
        assert!(s.margin_fit_enabled);

        let mut s = Settings::default();
        s.margin_fit_enabled = true;
        s.fullscreen_fit_mode = FullscreenFitMode::Width;
        s.sanitize();
        assert_eq!(s.fullscreen_fit_mode, FullscreenFitMode::Width);
        assert!(!s.margin_fit_enabled);
    }

    #[test]
    fn sanitize_clamps_video_playback_speed() {
        let mut s = Settings::default();
        s.video_playback_speed = 999.0;
        s.sanitize();
        assert_eq!(
            s.video_playback_speed,
            crate::video::clock::MAX_PLAYBACK_SPEED
        );

        let mut s = Settings::default();
        s.video_playback_speed = f64::NAN;
        s.sanitize();
        assert_eq!(s.video_playback_speed, 1.0);
    }

    #[test]
    fn sanitize_migrates_legacy_only_from_grid_autoplay_to_off() {
        let mut s = Settings::default();
        s.video_autoplay = true;
        s.video_autoplay_mode = VideoAutoplayMode::OnlyFromGrid;
        s.sanitize();
        assert_eq!(s.video_autoplay_mode, VideoAutoplayMode::Off);
        assert!(
            !s.video_autoplay,
            "legacy OnlyFromGrid should not be bridged back to Always by video_autoplay"
        );
    }

    #[test]
    fn migrate_legacy_video_loop_promotes_bool_true_to_full() {
        // 旧 bool=true + 新 enum=Off (= 旧バージョンの settings.json を読み込んだ直後)
        // → Full に昇格する。
        let mut s = Settings::default();
        s.video_loop = true;
        s.video_loop_mode = VideoLoopMode::Off;
        let did_migrate = s.migrate_legacy_video_loop();
        assert!(did_migrate);
        assert_eq!(s.video_loop_mode, VideoLoopMode::Full);
    }

    #[test]
    fn migrate_legacy_video_loop_does_not_overwrite_explicit_mode() {
        // 新 enum=Chapter (新ビルドが書いた値) + 旧 bool=false → Chapter のまま、migration なし。
        let mut s = Settings::default();
        s.video_loop = false;
        s.video_loop_mode = VideoLoopMode::Chapter;
        assert!(!s.migrate_legacy_video_loop());
        assert_eq!(s.video_loop_mode, VideoLoopMode::Chapter);
    }

    #[test]
    fn migrate_legacy_video_loop_is_noop_when_bool_false_and_mode_off() {
        let mut s = Settings::default();
        assert!(!s.migrate_legacy_video_loop());
        assert_eq!(s.video_loop_mode, VideoLoopMode::Off);
    }

    #[test]
    fn migrate_legacy_archive_handling_uses_old_without_dialog_bool() {
        let mut s = Settings::default();
        s.archive_file_handling = ArchiveFileHandling::Legacy;
        s.archive_convert_without_dialog = true;

        assert!(s.migrate_legacy_archive_file_handling());
        assert_eq!(s.archive_file_handling, ArchiveFileHandling::Convert);
        assert!(s.archive_convert_without_dialog);

        let mut s = Settings::default();
        s.archive_file_handling = ArchiveFileHandling::Legacy;
        s.archive_convert_without_dialog = false;

        assert!(s.migrate_legacy_archive_file_handling());
        assert_eq!(s.archive_file_handling, ArchiveFileHandling::Ask);
        assert!(!s.archive_convert_without_dialog);
    }

    #[test]
    fn migrate_legacy_archive_handling_keeps_explicit_ignore() {
        let mut s = Settings::default();
        s.archive_file_handling = ArchiveFileHandling::Ignore;
        s.archive_convert_without_dialog = true;

        assert!(s.migrate_legacy_archive_file_handling());
        assert_eq!(s.archive_file_handling, ArchiveFileHandling::Ignore);
        assert!(!s.archive_convert_without_dialog);
    }

    #[test]
    fn sanitize_syncs_legacy_bool_from_mode_idempotent() {
        // mode を source of truth として bool を導出。sanitize は idempotent。
        let mut s = Settings::default();
        s.video_loop_mode = VideoLoopMode::Bookmark;
        s.video_loop = false;
        s.sanitize();
        assert!(s.video_loop);

        s.video_loop_mode = VideoLoopMode::Off;
        s.sanitize();
        assert!(!s.video_loop);
        // 2 回目の sanitize でも変化なし
        s.sanitize();
        assert!(!s.video_loop);
        assert_eq!(s.video_loop_mode, VideoLoopMode::Off);
    }

    #[test]
    fn cycle_loop_mode_normal_progression_when_all_available() {
        let f = |m| cycle_loop_mode(m, true, true);
        assert_eq!(f(VideoLoopMode::Off), VideoLoopMode::Full);
        assert_eq!(f(VideoLoopMode::Full), VideoLoopMode::Chapter);
        assert_eq!(f(VideoLoopMode::Chapter), VideoLoopMode::Bookmark);
        assert_eq!(f(VideoLoopMode::Bookmark), VideoLoopMode::Off);
    }

    #[test]
    fn cycle_loop_mode_skips_chapter_when_no_chapters() {
        let f = |m| cycle_loop_mode(m, false, true);
        assert_eq!(f(VideoLoopMode::Off), VideoLoopMode::Full);
        assert_eq!(f(VideoLoopMode::Full), VideoLoopMode::Bookmark);
        assert_eq!(f(VideoLoopMode::Bookmark), VideoLoopMode::Off);
    }

    #[test]
    fn cycle_loop_mode_skips_bookmark_when_no_bookmarks() {
        let f = |m| cycle_loop_mode(m, true, false);
        assert_eq!(f(VideoLoopMode::Off), VideoLoopMode::Full);
        assert_eq!(f(VideoLoopMode::Full), VideoLoopMode::Chapter);
        assert_eq!(f(VideoLoopMode::Chapter), VideoLoopMode::Off);
    }

    #[test]
    fn cycle_loop_mode_skips_both_when_neither_available() {
        let f = |m| cycle_loop_mode(m, false, false);
        assert_eq!(f(VideoLoopMode::Off), VideoLoopMode::Full);
        assert_eq!(f(VideoLoopMode::Full), VideoLoopMode::Off);
    }

    #[test]
    fn cycle_loop_mode_handles_invalid_current_chapter() {
        // 動画 A (CH 有り) で Chapter モードのまま動画 B (CH 無し / BM 有り) に移動した状態。
        // 「無効な現在モードから次に押した時」の挙動を固定する。
        assert_eq!(
            cycle_loop_mode(VideoLoopMode::Chapter, false, true),
            VideoLoopMode::Bookmark
        );
        assert_eq!(
            cycle_loop_mode(VideoLoopMode::Chapter, false, false),
            VideoLoopMode::Off
        );
    }

    #[test]
    fn cycle_loop_mode_handles_invalid_current_bookmark() {
        assert_eq!(
            cycle_loop_mode(VideoLoopMode::Bookmark, true, false),
            VideoLoopMode::Off
        );
        assert_eq!(
            cycle_loop_mode(VideoLoopMode::Bookmark, false, false),
            VideoLoopMode::Off
        );
        assert_eq!(
            cycle_loop_mode(VideoLoopMode::Bookmark, true, true),
            VideoLoopMode::Off
        );
    }

    #[test]
    fn effective_loop_mode_degrades_to_full_when_data_missing() {
        assert_eq!(
            effective_loop_mode(VideoLoopMode::Chapter, false, true),
            VideoLoopMode::Full
        );
        assert_eq!(
            effective_loop_mode(VideoLoopMode::Bookmark, true, false),
            VideoLoopMode::Full
        );
        // 当該データありなら降格しない
        assert_eq!(
            effective_loop_mode(VideoLoopMode::Chapter, true, false),
            VideoLoopMode::Chapter
        );
        // Off / Full は has_* に依存しない
        assert_eq!(
            effective_loop_mode(VideoLoopMode::Off, true, true),
            VideoLoopMode::Off
        );
        assert_eq!(
            effective_loop_mode(VideoLoopMode::Full, false, false),
            VideoLoopMode::Full
        );
    }

    #[test]
    fn start_at_returns_largest_le() {
        let starts = vec![0.0, 5.0, 10.0, 20.0];
        assert_eq!(start_at(&starts, -1.0), None);
        assert_eq!(start_at(&starts, 0.0), Some(0.0));
        assert_eq!(start_at(&starts, 4.99), Some(0.0));
        assert_eq!(start_at(&starts, 5.0), Some(5.0));
        assert_eq!(start_at(&starts, 9.99), Some(5.0));
        assert_eq!(start_at(&starts, 100.0), Some(20.0));
        assert_eq!(start_at(&[], 5.0), None);
    }

    #[test]
    fn first_boundary_after_returns_smallest_gt() {
        let starts = vec![0.0, 5.0, 10.0, 20.0];
        assert_eq!(first_boundary_after(&starts, -1.0), Some(0.0));
        assert_eq!(first_boundary_after(&starts, 0.0), Some(5.0));
        assert_eq!(first_boundary_after(&starts, 4.99), Some(5.0));
        assert_eq!(first_boundary_after(&starts, 5.0), Some(10.0));
        assert_eq!(first_boundary_after(&starts, 19.99), Some(20.0));
        assert_eq!(first_boundary_after(&starts, 20.0), None);
        assert_eq!(first_boundary_after(&starts, 100.0), None);
    }

    #[test]
    fn decide_boundary_action_loops_on_crossing() {
        // prev=9.99, cur=10.01 で boundary 10.00 を跨いだ → Loop へ
        let dec = decide_boundary_action(9.99, 7, 10.01, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Loop { seek_to: 0.0 });
    }

    #[test]
    fn decide_boundary_action_no_loop_when_not_crossed() {
        // 9.0 → 9.5 はまだ跨いでいない (tol=0.020 マージン外)
        let dec = decide_boundary_action(9.0, 7, 9.5, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Continue);
    }

    #[test]
    fn decide_boundary_action_baseline_update_on_seek_serial_change() {
        // 境界跨ぎ相当の delta でも serial 変化があれば手動 seek と判断
        let dec = decide_boundary_action(9.99, 7, 10.01, 8, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::BaselineUpdate);
    }

    #[test]
    fn decide_boundary_action_baseline_update_on_backward() {
        let dec = decide_boundary_action(10.0, 7, 9.0, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::BaselineUpdate);
    }

    #[test]
    fn decide_boundary_action_continue_when_no_next_boundary() {
        // 最後の区間 (= duration まで境界なし) では Loop しない (EOF は VideoPlayer 側経路)
        let dec = decide_boundary_action(10.0, 7, 10.5, 7, 5.0, None, 0.020);
        assert_eq!(dec, BoundaryDecision::Continue);
    }

    #[test]
    fn decide_boundary_action_loops_within_tolerance_margin() {
        // tol=0.020, prev=9.99, cur=9.99 + 0.010 → cur >= boundary - tol で発火
        let dec = decide_boundary_action(9.99, 7, 9.99 + 0.010, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Loop { seek_to: 0.0 });
    }

    #[test]
    fn decide_boundary_action_loops_when_prev_is_just_below_boundary() {
        // prev=9.99, boundary=10.00, cur=10.00 (= boundary に到達)
        // 左辺 prev_pos < boundary は厳密判定 (tol 引かない) なので 9.99 < 10.00 で true
        let dec = decide_boundary_action(9.99, 7, 10.00, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Loop { seek_to: 0.0 });
    }

    #[test]
    fn decide_boundary_action_no_loop_when_strictly_no_progress() {
        // playing seek/scrub が tol 内に着地して cur == prev_pos のまま再開した場合、
        // 即ループしない (Codex P2 第8ラウンド)。前進ゼロは Continue。
        let dec = decide_boundary_action(9.99, 7, 9.99, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Continue);
    }

    #[test]
    fn decide_boundary_action_loops_at_low_speed_with_small_delta() {
        // 0.5x 再生 + 60Hz tick 相当 (delta ≈ 8ms)。低速再生でも境界を見逃さない
        // (Codex P1 第10ラウンド)。
        let dec = decide_boundary_action(9.974, 7, 9.982, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Loop { seek_to: 0.0 });
    }

    #[test]
    fn decide_boundary_action_loops_at_micro_progress() {
        // 1us 単位の進行でも前進している限り境界手前 tol 内なら Loop 発火する。
        let dec = decide_boundary_action(9.9999, 7, 9.99991, 7, 0.0, Some(10.0), 0.020);
        assert_eq!(dec, BoundaryDecision::Loop { seek_to: 0.0 });
    }

    #[test]
    fn sanitize_clamps_video_volume_to_manual_boost_range() {
        let mut s = Settings::default();
        s.video_volume = 10.0;
        s.sanitize();
        assert_eq!(s.video_volume, VIDEO_VOLUME_MAX);

        s.video_volume = -0.5;
        s.sanitize();
        assert_eq!(s.video_volume, 0.0);
    }

    #[test]
    fn video_volume_db_helpers_map_fader_marks() {
        assert_eq!(video_volume_db_to_linear(VIDEO_VOLUME_MUTE_DB), 0.0);
        assert!((video_volume_db_to_linear(0.0) - 1.0).abs() < 1.0e-12);
        assert!((video_volume_db_to_linear(6.0) - 1.9952623149688795).abs() < 1.0e-12);
        assert!((video_volume_db_to_linear(12.0) - 3.981_071_705_534_972_2).abs() < 1.0e-12);
        assert!((video_volume_db_to_linear(18.0) - VIDEO_VOLUME_MAX).abs() < 1.0e-12);

        assert!((video_volume_linear_to_db(1.0) - 0.0).abs() < 1.0e-12);
        assert!((video_volume_linear_to_db(VIDEO_VOLUME_MAX) - 18.0).abs() < 1.0e-12);
        assert_eq!(video_volume_linear_to_db(0.0), VIDEO_VOLUME_MUTE_DB);

        for &mark in &VIDEO_VOLUME_FADER_DB_MARKS {
            let pos = video_volume_db_to_fader_pos(mark);
            assert!((video_volume_fader_pos_to_db(pos) - mark).abs() < 1.0e-9);
            let linear = video_volume_db_to_linear(mark);
            let roundtrip =
                video_volume_fader_pos_to_linear(video_volume_linear_to_fader_pos(linear));
            assert!((roundtrip - linear).abs() < 1.0e-9);
        }
    }

    #[test]
    fn video_volume_step_uses_quarter_fader_mark_steps() {
        let mut up = 1.0;
        for _ in 0..4 {
            up = step_video_volume_by_fader_key_step(up, 1);
        }
        assert!(
            (up - video_volume_db_to_linear(6.0)).abs() < 1.0e-12,
            "four key steps above 0dB should reach the next visible mark"
        );

        let mut down = 1.0;
        for _ in 0..4 {
            down = step_video_volume_by_fader_key_step(down, -1);
        }
        assert!(
            (down - video_volume_db_to_linear(-5.0)).abs() < 1.0e-12,
            "four key steps below 0dB should reach the next visible mark"
        );
        assert!(
            (step_video_volume_by_fader_key_step(1.0, 1) - video_volume_db_to_linear(1.5)).abs()
                < 1.0e-12
        );
        assert_eq!(step_video_volume_by_fader_key_step(0.0, -1), 0.0);

        let mut high = video_volume_db_to_linear(12.0);
        for _ in 0..4 {
            high = step_video_volume_by_fader_key_step(high, 1);
        }
        assert_eq!(
            high, VIDEO_VOLUME_MAX,
            "four key steps above +12dB should reach +18dB"
        );
        assert_eq!(
            step_video_volume_by_fader_key_step(VIDEO_VOLUME_MAX, 1),
            VIDEO_VOLUME_MAX
        );
    }

    #[test]
    fn video_autoplay_mode_choices_hide_legacy_only_from_grid() {
        assert_eq!(
            VideoAutoplayMode::all(),
            &[VideoAutoplayMode::Off, VideoAutoplayMode::Always]
        );
    }

    // -- FavoriteEntry serde --

    #[test]
    fn favorite_deserialize_legacy_string() {
        let json = r#""C:\\foo\\bar""#;
        let entry: FavoriteEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.name, "bar");
        assert_eq!(entry.path, PathBuf::from(r"C:\foo\bar"));
    }

    #[test]
    fn favorite_deserialize_new_format() {
        let json = r#"{"name":"My Folder","path":"C:\\foo"}"#;
        let entry: FavoriteEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.name, "My Folder");
        assert_eq!(entry.path, PathBuf::from(r"C:\foo"));
    }

    #[test]
    fn favorite_serialize_always_object() {
        let entry = FavoriteEntry::new("Test".to_string(), PathBuf::from(r"C:\test"));
        let json = serde_json::to_string(&entry).unwrap();
        // オブジェクト形式で出力されることを確認
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"path\""));
        assert!(json.contains("\"id\""));
        assert!(json.contains("\"auto_index_structure\""));
        assert!(json.contains("\"auto_index_metadata\""));
        assert!(json.contains("\"auto_index_thumbs\""));
    }

    #[test]
    fn favorite_legacy_string_migrates_to_new_uuid() {
        // 旧形式 (文字列のみ) → UUID は nil で deserialize 後、sanitize で発行される
        let json = r#""C:\\foo\\bar""#;
        let entry: FavoriteEntry = serde_json::from_str(json).unwrap();
        assert!(entry.id.is_nil(), "deserialize 時点では nil");
        assert_eq!(entry.name, "bar");
        assert!(!entry.auto_index_structure);
        assert!(!entry.auto_index_metadata);
        assert!(!entry.auto_index_thumbs);
    }

    #[test]
    fn favorite_new_format_defaults_flags_false() {
        let json = r#"{"name":"a","path":"C:\\x"}"#;
        let entry: FavoriteEntry = serde_json::from_str(json).unwrap();
        assert!(entry.id.is_nil(), "id 欠落時は nil (sanitize で発行)");
        assert_eq!(entry.name, "a");
        assert!(!entry.auto_index_structure);
        assert!(!entry.auto_index_metadata);
        assert!(!entry.auto_index_thumbs);
    }

    #[test]
    fn favorite_full_v08_roundtrip() {
        let mut e = FavoriteEntry::new("x".to_string(), PathBuf::from(r"C:\x"));
        e.auto_index_structure = true;
        e.auto_index_metadata = true;
        e.auto_index_thumbs = false;
        let json = serde_json::to_string(&e).unwrap();
        let back: FavoriteEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, e.id);
        assert_eq!(back.name, "x");
        assert!(back.auto_index_structure);
        assert!(back.auto_index_metadata);
        assert!(!back.auto_index_thumbs);
    }

    #[test]
    fn sanitize_assigns_uuid_to_nil_favorites() {
        let mut s = Settings::default();
        let mut legacy_fav = FavoriteEntry::new("a".to_string(), PathBuf::from(r"C:\a"));
        legacy_fav.id = Uuid::nil();
        s.favorites.push(legacy_fav);
        s.sanitize();
        assert!(
            !s.favorites[0].id.is_nil(),
            "sanitize で UUID が発行されるはず"
        );
    }

    // -- ThumbAspect --

    #[test]
    fn thumb_aspect_height_ratio() {
        let eps = 1e-6;
        assert!((ThumbAspect::Square.height_ratio() - 1.0).abs() < eps);
        assert!((ThumbAspect::Landscape16x9.height_ratio() - 9.0 / 16.0).abs() < eps);
        assert!((ThumbAspect::Landscape3x2.height_ratio() - 2.0 / 3.0).abs() < eps);
        assert!((ThumbAspect::Landscape4x3.height_ratio() - 3.0 / 4.0).abs() < eps);
        assert!((ThumbAspect::Portrait3x4.height_ratio() - 4.0 / 3.0).abs() < eps);
        assert!((ThumbAspect::Portrait2x3.height_ratio() - 3.0 / 2.0).abs() < eps);
        assert!((ThumbAspect::Portrait9x16.height_ratio() - 16.0 / 9.0).abs() < eps);
    }

    #[test]
    fn thumb_aspect_all_has_all_variants() {
        assert_eq!(ThumbAspect::all().len(), 7);
    }

    // -- IndexerSpeedProfile (v0.8.0) --

    #[test]
    fn indexer_speed_profile_io_permits() {
        assert_eq!(IndexerSpeedProfile::Low.io_permits(), 1);
        assert_eq!(IndexerSpeedProfile::Medium.io_permits(), 2);
        assert_eq!(IndexerSpeedProfile::High.io_permits(), 4);
    }

    #[test]
    fn indexer_speed_profile_default_is_low() {
        // HDD 環境で UI 応答性を優先するため、既定は 1 permit (Low)。
        // SSD/NVMe 向けに Medium/High を選べる。
        assert_eq!(IndexerSpeedProfile::default(), IndexerSpeedProfile::Low);
    }

    #[test]
    fn indexer_speed_profile_all_has_three_variants() {
        assert_eq!(IndexerSpeedProfile::all().len(), 3);
    }

    #[test]
    fn indexer_speed_profile_roundtrip_serde() {
        for p in IndexerSpeedProfile::all() {
            let json = serde_json::to_string(p).unwrap();
            let back: IndexerSpeedProfile = serde_json::from_str(&json).unwrap();
            assert_eq!(back, *p);
        }
    }

    #[test]
    fn vst3_chain_slots_default_when_missing() {
        let loaded: Settings = serde_json::from_str("{}").unwrap();
        assert!(loaded.vst3_chain_slots.slots.iter().all(Option::is_none));
        assert_eq!(loaded.vst3_panel_pos, None);
    }

    #[test]
    fn vst3_chain_slots_roundtrip() {
        let mut settings = Settings::default();
        settings.vst3_panel_pos = Some([123.0, 456.0]);
        settings.vst3_chain_slots.slots[0] = Some(Vst3ChainPresetSlot {
            name: "Mix".to_string(),
            plugins: vec![Vst3PluginEntry {
                path: r"C:\VST3\Test.vst3".to_string(),
                bypass: true,
                state: Some("state".to_string()),
                user_hidden: true,
                gui_pos: Some((12, 34)),
                gui_size: Some((640, 480)),
            }],
            gui_visible: false,
            video_compact: true,
        });

        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.vst3_panel_pos, Some([123.0, 456.0]));
        let slot = loaded.vst3_chain_slots.slots[0].as_ref().unwrap();
        assert_eq!(slot.name, "Mix");
        assert!(!slot.gui_visible);
        assert!(slot.video_compact);
        assert_eq!(slot.plugins.len(), 1);
        assert_eq!(slot.plugins[0].path, r"C:\VST3\Test.vst3");
        assert!(slot.plugins[0].bypass);
        assert_eq!(slot.plugins[0].state.as_deref(), Some("state"));
        assert_eq!(slot.plugins[0].gui_pos, Some((12, 34)));
        assert_eq!(slot.plugins[0].gui_size, Some((640, 480)));
    }

    // -- SortOrder --

    #[test]
    fn sort_order_compare_filename() {
        let ord = SortOrder::FileName;
        let result = ord.compare("Bbb.jpg", 0, "aaa.jpg", 0, |s: &str| s.to_string());
        assert_eq!(result, std::cmp::Ordering::Greater); // "bbb" > "aaa"
    }

    #[test]
    fn sort_order_compare_date() {
        assert_eq!(
            SortOrder::DateAsc.compare("a", 100, "b", 200, |s: &str| s.to_string()),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            SortOrder::DateDesc.compare("a", 100, "b", 200, |s: &str| s.to_string()),
            std::cmp::Ordering::Greater
        );
    }

    /// 日付ソートで mtime が同じ場合はファイル名昇順で安定化する
    /// (mtime_secs は秒精度なので同一秒の衝突は実際に起きる)。
    #[test]
    fn sort_order_compare_date_tiebreak_by_name() {
        let key = |s: &str| s.to_string();
        assert_eq!(
            SortOrder::DateAsc.compare("Bbb", 100, "aaa", 100, key),
            std::cmp::Ordering::Greater,
            "DateAsc 同 mtime: 名前昇順で並ぶべき (Bbb > aaa)"
        );
        assert_eq!(
            SortOrder::DateDesc.compare("Bbb", 100, "aaa", 100, key),
            std::cmp::Ordering::Greater,
            "DateDesc 同 mtime でも名前昇順 (= 新しいもの優先で揃え、同 mtime は名前順)"
        );
    }

    // -- CachePolicy --

    #[test]
    fn cache_policy_labels() {
        // 全バリアントにラベルがあることを確認（空でない）
        assert!(!CachePolicy::Off.label().is_empty());
        assert!(!CachePolicy::Auto.label().is_empty());
        assert!(!CachePolicy::Always.label().is_empty());
    }

    // -- Parallelism --

    #[test]
    fn parallelism_manual_min_one() {
        assert_eq!(Parallelism::Manual(0).thread_count(), 1);
        assert_eq!(Parallelism::Manual(1).thread_count(), 1);
        assert_eq!(Parallelism::Manual(4).thread_count(), 4);
    }

    #[test]
    fn parallelism_serde_tagged() {
        let auto: Parallelism = serde_json::from_str(r#"{"mode":"Auto"}"#).unwrap();
        assert_eq!(auto, Parallelism::Auto);

        let manual: Parallelism = serde_json::from_str(r#"{"mode":"Manual","value":4}"#).unwrap();
        assert_eq!(manual, Parallelism::Manual(4));
    }

    // -- add_favorite --

    #[test]
    fn add_favorite_success() {
        let mut s = Settings::default();
        assert!(s.add_favorite("Test".to_string(), PathBuf::from(r"C:\test")));
        assert_eq!(s.favorites.len(), 1);
    }

    #[test]
    fn add_favorite_duplicate() {
        let mut s = Settings::default();
        s.add_favorite("Test".to_string(), PathBuf::from(r"C:\test"));
        assert!(!s.add_favorite("Test2".to_string(), PathBuf::from(r"C:\test")));
        assert_eq!(s.favorites.len(), 1);
    }

    #[test]
    fn add_favorite_max_limit() {
        let mut s = Settings::default();
        for i in 0..MAX_FAVORITES {
            assert!(s.add_favorite(format!("F{i}"), PathBuf::from(format!(r"C:\dir{i}"))));
        }
        assert_eq!(s.favorites.len(), MAX_FAVORITES);
        // 21個目は追加できない
        assert!(!s.add_favorite("Overflow".to_string(), PathBuf::from(r"C:\overflow")));
        assert_eq!(s.favorites.len(), MAX_FAVORITES);
    }

    // -----------------------------------------------------------------
    // Backup / atomic save tests (#1, #2, #3, #4, #5)
    // -----------------------------------------------------------------

    // 旧 BACKUP_TEST_LOCK (= settings.rs ローカル) は Codex P2 v9b 2026-05-14 で
    // 削除。data_dir::set_test_override は process-global なので、settings_db.rs /
    // app/tests.rs と共有の `crate::data_dir::test_override_lock()` に統一する。

    struct BackupTestEnv {
        _tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for BackupTestEnv {
        fn drop(&mut self) {
            crate::data_dir::set_test_override(None);
            // 後続テストが state を持ち込まないようリセット。
            reset_backup_state_for_test();
        }
    }

    fn setup_backup_env() -> BackupTestEnv {
        let lock = crate::data_dir::test_override_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        crate::data_dir::set_test_override(Some(tmp.path().to_path_buf()));
        reset_backup_state_for_test();
        BackupTestEnv {
            _tmp: tmp,
            _lock: lock,
        }
    }

    fn settings_with_favorite(name: &str) -> Settings {
        let mut s = Settings::default();
        s.add_favorite(name.to_string(), PathBuf::from(format!(r"C:\{name}")));
        s
    }

    /// #1 atomic save / #4 preupgrade: 普通のラウンドトリップで .tmp が残らず、
    ///    保存後の last_seen_version が現バージョンに更新されること。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::save_load_roundtrip`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn save_load_roundtrip_clean() {
        let _env = setup_backup_env();
        let s = settings_with_favorite("alpha");
        s.save();

        let main_path = Settings::settings_path();
        let tmp_path = main_path.with_file_name("settings.json.tmp");
        assert!(main_path.exists(), "main settings.json should exist");
        assert!(!tmp_path.exists(), "tmp file should be cleaned up");

        let loaded = Settings::load();
        assert_eq!(loaded.favorites.len(), 1);
        assert_eq!(loaded.favorites[0].name, "alpha");
        assert_eq!(
            loaded.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    /// #2 世代バックアップ: 起動 (= save 1 回) ごとに 1 段ずつ rotate される。
    /// 同プロセス内では 2 回目以降の save() で rotate しない (= bak1 が
    /// "セッション開始時の状態" のまま維持される) ことを確認する。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::rotation_*`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn save_rotates_only_once_per_session() {
        let _env = setup_backup_env();

        let s1 = settings_with_favorite("first");
        s1.save();

        let main_path = Settings::settings_path();
        let bak1 = backup_path(&main_path, 1);
        // 初回 save: 旧 main は無いので bak1 はまだ作られていない。
        assert!(!bak1.exists(), "no rotation source on initial save");

        // 同プロセス内で 2 回保存しても rotation は走らないので bak1 は依然空。
        let s2 = settings_with_favorite("second");
        s2.save();
        assert!(
            !bak1.exists(),
            "second save in same session must not rotate"
        );

        // 別セッションを模す (= rotation flag をリセット) と、次の save で
        // 直前 main が bak1 へ退避される。
        reset_backup_state_for_test();
        let s3 = settings_with_favorite("third");
        s3.save();
        assert!(
            bak1.exists(),
            "next session should rotate prior main into bak1"
        );

        let prior: Settings =
            serde_json::from_str(&std::fs::read_to_string(&bak1).unwrap()).unwrap();
        assert_eq!(prior.favorites[0].name, "second");
    }

    /// 10 セッション分 rotate すると bak10 まで埋まり、それ以降の世代は捨てられる。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::rotation_*`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn rotation_keeps_at_most_10_generations() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();

        // セッション 1..=12 を模す: 各セッションで save 1 回 + rotation flag リセット。
        for i in 1..=12 {
            reset_backup_state_for_test();
            let s = settings_with_favorite(&format!("gen{i}"));
            s.save();
        }

        // bak1..bak10 まで存在し、bak11+ は存在しないこと。
        for n in 1..=BACKUP_COUNT {
            assert!(
                backup_path(&main_path, n).exists(),
                "bak{n} should exist after 12 sessions"
            );
        }
        assert!(
            !backup_path(&main_path, BACKUP_COUNT + 1).exists(),
            "bak{} must not exist (we only keep {} generations)",
            BACKUP_COUNT + 1,
            BACKUP_COUNT
        );

        // bak1 はセッション 11 の状態 (= "gen11") のはず
        // (セッション 12 の save 直前の main は gen11)。
        let bak1: Settings =
            serde_json::from_str(&std::fs::read_to_string(backup_path(&main_path, 1)).unwrap())
                .unwrap();
        assert_eq!(bak1.favorites[0].name, "gen11");
    }

    /// #5 quarantine + #2 auto recovery: 壊れた main は .broken-<TS> へ退避され、
    /// 直近の bak1 から復旧される。
    ///
    /// Phase 3: 旧 JSON 経路のテスト (`.broken-<TS>` リネームは JSON path)。
    /// SQLite 版は `phase3_sqlite::corrupt_recovery_*`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn corrupt_main_recovers_from_bak1() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();

        // bak1 に良い JSON を仕込む。
        let good = settings_with_favorite("recovered");
        let good_json = serde_json::to_string_pretty(&good).unwrap();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();
        std::fs::write(backup_path(&main_path, 1), &good_json).unwrap();

        // main を壊す。
        std::fs::write(&main_path, "{ this is not json").unwrap();

        let loaded = Settings::load();
        assert_eq!(loaded.favorites.len(), 1);
        assert_eq!(loaded.favorites[0].name, "recovered");

        // 壊れた main は .broken-<TS> に rename されている。
        let broken_dir = main_path.parent().unwrap();
        let broken_files: Vec<_> = std::fs::read_dir(broken_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.broken-")
            })
            .collect();
        assert!(
            !broken_files.is_empty(),
            "corrupt main should be quarantined as .broken-<TS>"
        );
    }

    /// bak1 も壊れていれば bak2 へフォールバックする (= 新→古に順試行)。
    ///
    /// Phase 3 注: SQLite 化後はこのテストは「壊れた JSON が `boot_settings_db` の
    /// migration 経路で読まれ、bak2 から復旧されるか」を実質的にテストすることになる。
    /// `settings_db::tests::migrate_from_settings_json_*` で同等カバレッジあり。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in settings_db tests"]
    fn corrupt_main_and_bak1_falls_through_to_bak2() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();

        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();
        let good = settings_with_favorite("from_bak2");
        std::fs::write(
            backup_path(&main_path, 2),
            serde_json::to_string_pretty(&good).unwrap(),
        )
        .unwrap();
        // bak1 と main は壊しておく。
        std::fs::write(backup_path(&main_path, 1), "{ corrupt").unwrap();
        std::fs::write(&main_path, "{ corrupt").unwrap();

        let loaded = Settings::load();
        assert_eq!(loaded.favorites[0].name, "from_bak2");
    }

    /// 全滅 (main + bak1..bak10 すべて壊れている) なら Default。
    ///
    /// Phase 3: 旧 JSON 経路。SQLite 版 `settings_db::tests::boot_failed_returns_default_with_suppress`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in settings_db tests"]
    fn all_broken_falls_back_to_default() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        std::fs::write(&main_path, "{ broken").unwrap();
        for n in 1..=BACKUP_COUNT {
            std::fs::write(backup_path(&main_path, n), "{ broken").unwrap();
        }

        let loaded = Settings::load();
        assert!(loaded.favorites.is_empty());
        assert_eq!(loaded.grid_cols, default_grid_cols());
    }

    /// #4 preupgrade: 過去に保存された JSON のバージョンと現バイナリのバージョンが
    /// 違うとき、現状の settings.json を `settings.json.preupgrade-v<old>` に複製する。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::version_preupgrade_snapshot`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn version_change_creates_preupgrade_snapshot() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // 旧バージョンで保存された状態を仕込む。
        let mut old = settings_with_favorite("preupgrade_test");
        old.last_seen_version = Some("0.0.0-test-prev".to_string());
        std::fs::write(&main_path, serde_json::to_string_pretty(&old).unwrap()).unwrap();

        let loaded = Settings::load();
        // 現バイナリのバージョンに更新されている。
        assert_eq!(
            loaded.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );

        // preupgrade snapshot が存在し、中身は旧 last_seen_version を持っている。
        let pre = preupgrade_path(&main_path, "0.0.0-test-prev");
        assert!(pre.exists(), "preupgrade snapshot should be created");
        let pre_settings: Settings =
            serde_json::from_str(&std::fs::read_to_string(&pre).unwrap()).unwrap();
        assert_eq!(
            pre_settings.last_seen_version.as_deref(),
            Some("0.0.0-test-prev")
        );
        assert_eq!(pre_settings.favorites[0].name, "preupgrade_test");
    }

    /// 同じ「前バージョン」名の preupgrade snapshot が既に存在するなら上書きしない
    /// (= 同バージョンの起動を繰り返しても、直近 1 回分の素材だけが保存される)。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::version_preupgrade_snapshot_not_overwritten`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn preupgrade_snapshot_is_not_overwritten() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        let pre = preupgrade_path(&main_path, "0.0.0-test-prev");
        std::fs::write(&pre, "EXISTING_SNAPSHOT_CONTENT").unwrap();

        let mut old = settings_with_favorite("snapshot_collision");
        old.last_seen_version = Some("0.0.0-test-prev".to_string());
        std::fs::write(&main_path, serde_json::to_string_pretty(&old).unwrap()).unwrap();

        let _ = Settings::load();

        // 既存ファイルは温存されている (= load 中に上書きしていない)。
        assert_eq!(
            std::fs::read_to_string(&pre).unwrap(),
            "EXISTING_SNAPSHOT_CONTENT"
        );
    }

    /// Codex P2 (#2): try_parse_settings_file は **read I/O 失敗** と
    /// **内容のエラー (UTF-8 / JSON)** を別の variant で返さねばならない。
    #[test]
    fn try_parse_distinguishes_io_from_parse_error() {
        let _env = setup_backup_env();
        let dir = Settings::settings_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();

        // (a) 存在しない -> NotFound
        assert!(matches!(
            try_parse_settings_file(&dir.join("nonexistent.json")),
            LoadFileResult::NotFound
        ));

        // (b) 不正な JSON テキスト -> ParseError
        let bad_json = dir.join("bad.json");
        std::fs::write(&bad_json, b"{ invalid").unwrap();
        assert!(matches!(
            try_parse_settings_file(&bad_json),
            LoadFileResult::ParseError
        ));

        // (c) 不正な UTF-8 バイト列 -> ParseError (Codex 指摘のとおり、
        //     read 段階で読めても内容が不正なら ParseError 扱い)
        let utf8_bad = dir.join("utf8_bad.json");
        std::fs::write(&utf8_bad, &[0xFFu8, 0xFE, 0xFD][..]).unwrap();
        assert!(matches!(
            try_parse_settings_file(&utf8_bad),
            LoadFileResult::ParseError
        ));

        // (d) 正常 -> Ok
        let good = dir.join("good.json");
        std::fs::write(&good, serde_json::to_string(&Settings::default()).unwrap()).unwrap();
        assert!(matches!(
            try_parse_settings_file(&good),
            LoadFileResult::Ok(_)
        ));

        // (e) ディレクトリ -> IoError (read が NotFound 以外の OS エラーで失敗)
        let dir_path = dir.join("isadir.json");
        std::fs::create_dir(&dir_path).unwrap();
        let result = try_parse_settings_file(&dir_path);
        assert!(
            matches!(result, LoadFileResult::IoError),
            "expected IoError for directory path, got {:?}",
            result
        );
    }

    /// Codex P2 (#2): main 読み取りが I/O エラー (一時的かもしれない) のときは
    /// quarantine してはいけない (= ロックが解けたら正常な main を再読みしたい)。
    #[test]
    fn io_error_on_main_does_not_quarantine() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // bak1 に良い JSON を仕込む。
        let good = settings_with_favorite("from_bak1");
        std::fs::write(
            backup_path(&main_path, 1),
            serde_json::to_string_pretty(&good).unwrap(),
        )
        .unwrap();

        // main を read 不能にする (= ディレクトリにする)。
        std::fs::create_dir(&main_path).unwrap();

        // recovery は走るが quarantine は起きない。main_unreadable フラグも立つ。
        let outcome = try_load_with_recovery(&main_path);
        let recovered = outcome.settings.expect("should recover from bak1");
        assert_eq!(recovered.favorites[0].name, "from_bak1");
        assert!(
            outcome.main_unreadable,
            "outcome must flag main as unreadable when read failed with non-NotFound I/O error"
        );

        // main path は依然ディレクトリのまま (= rename されていない)。
        assert!(
            main_path.is_dir(),
            "main path must not be quarantined on I/O error"
        );

        // .broken-* も作られていないこと。
        let parent = main_path.parent().unwrap();
        let broken_files: Vec<_> = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.broken-")
            })
            .collect();
        assert!(
            broken_files.is_empty(),
            "no quarantine file should be created on I/O error"
        );
    }

    /// Codex P2 (#4 2026-05-09): main が I/O エラーで読めなかったセッションで
    /// `Settings::load()` 全体を通したとき、後段の自動 save (= migration / version
    /// 変更トリガ) が `rotate_backups` で main を bak1 に rename して壊さないこと。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は
    /// `phase3_sqlite::io_error_save_suppression`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn io_error_on_main_during_load_does_not_clobber_via_save() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // bak1 に良い JSON を仕込む (last_seen_version は意図的に旧値にして
        // version_changed = true で save が走る条件を作る)。
        let mut good = settings_with_favorite("from_bak1");
        good.last_seen_version = Some("0.0.0-prev".to_string());
        std::fs::write(
            backup_path(&main_path, 1),
            serde_json::to_string_pretty(&good).unwrap(),
        )
        .unwrap();

        // main を read 不能にする (= ディレクトリにする)。
        std::fs::create_dir(&main_path).unwrap();

        // フル load() を走らせる。in-memory には bak1 の内容、副作用は最小限。
        let loaded = Settings::load();
        assert_eq!(loaded.favorites[0].name, "from_bak1");

        // main path は依然ディレクトリのまま (= save() の rotate / write_atomic で
        // 触られていない)。
        assert!(
            main_path.is_dir(),
            "main path must remain (unrenamed) after Settings::load() with I/O error"
        );

        // bak1 もそのまま (= rotate されていない)。同じ内容で再パースできる。
        let bak1_path = backup_path(&main_path, 1);
        assert!(bak1_path.is_file(), "bak1 should remain a regular file");
        let bak1_loaded: Settings =
            serde_json::from_str(&std::fs::read_to_string(&bak1_path).unwrap()).unwrap();
        assert_eq!(bak1_loaded.favorites[0].name, "from_bak1");

        // 万一サブシステム経由で `settings.save()` が呼ばれてもスキップされる。
        let mut later = loaded.clone();
        later.add_favorite("after_load".to_string(), PathBuf::from(r"C:\after"));
        later.save();
        assert!(
            main_path.is_dir(),
            "explicit save() in unreadable session must remain a no-op"
        );
        let bak1_after: Settings =
            serde_json::from_str(&std::fs::read_to_string(&bak1_path).unwrap()).unwrap();
        assert_eq!(
            bak1_after.favorites[0].name, "from_bak1",
            "bak1 must not be rotated by suppressed save()"
        );
    }

    /// 2026-05-12 復元事故回帰: 「main + bak1..bak10 が全部 load 失敗 (NotFound や
    /// ParseError 等) → built-in default に落ちる」エッジケースで、**bak ファイルが
    /// ディスク上に実在しているなら save を抑止する** ことを固定する。
    ///
    /// Phase 3 で SQLite 経路に切替後は `boot_settings_db` が壊れた JSON 群を
    /// migration 経路で読もうとして `AllFailed` (= 全 ParseError) を返し、
    /// `FailedFallbackDefault` で SAVE_SUPPRESSED が立つ、と等価。
    /// `settings_db::tests::boot_failed_returns_default_with_suppress` でも同様に確認している。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in settings_db tests"]
    fn all_load_failed_with_existing_baks_suppresses_save() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // main は不在 (= NotFound)、bak1..bak3 は壊れた JSON (= ParseError)。
        // ※ ParseError でも `try_load_with_recovery` は recovery 失敗で skip するだけで、
        //   bak 自体は disk に残る (Codex P2 2026-05-09: 壊れた bak は放置でローテ任せ)。
        let original_bak_contents = b"{ this is not json";
        for n in 1..=3 {
            std::fs::write(backup_path(&main_path, n), original_bak_contents).unwrap();
        }

        // load → 全 ParseError 経由で settings = None → default フォールバック
        let _loaded = Settings::load();

        // 直後の明示的 save() が抑止されることを確認 (= MAIN_UNREADABLE_THIS_SESSION が
        // 立っている)。
        let mut later = Settings::default();
        later.add_favorite("post_load".to_string(), PathBuf::from(r"C:\post"));
        later.save();
        assert!(
            !main_path.exists(),
            "save() must be suppressed when all loads failed but bak files exist on disk"
        );

        // bak ファイルもそのまま (= rotate されていない)。
        for n in 1..=3 {
            let bak = backup_path(&main_path, n);
            assert!(bak.is_file(), "bak{n} should remain on disk");
            let raw = std::fs::read(&bak).unwrap();
            assert_eq!(
                raw, original_bak_contents,
                "bak{n} content must not be touched by suppressed save()"
            );
        }
    }

    /// 「真の初回起動」(= main も bak1..bak10 も実在しない) では、save 抑止は **立たない** こと。
    /// アプリ初回インストール時に save が抑止されると初期 settings.json が作れず壊れる。
    ///
    /// Phase 3: 旧 JSON 経路のテスト。SQLite 版は `phase3_sqlite::pristine_first_launch`。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON path; SQLite equivalent in phase3_sqlite module"]
    fn pristine_first_launch_does_not_suppress_save() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // main も bak1..bak10 も無い状態で load
        let loaded = Settings::load();
        assert_eq!(loaded.favorites.len(), 0);

        // save が正常に走り、settings.json が作られる
        let mut s = loaded;
        s.add_favorite("first_install".to_string(), PathBuf::from(r"C:\first"));
        s.save();
        assert!(
            main_path.exists(),
            "save() must work on pristine first launch (no bak files exist)"
        );
        let reloaded = Settings::load();
        assert_eq!(reloaded.favorites[0].name, "first_install");
    }

    /// Codex P2 (#3): release ビルド (= main logger 未初期化) でも、復旧経路の
    /// 診断は `<data_dir>/logs/settings.log` に常時記録される。
    ///
    /// Phase 3: 旧 JSON 経路の文字列を検証していたため ignore。SQLite 経路の log は
    /// `phase3_sqlite::diag_log_records_boot_path` で別途検証する。
    #[test]
    #[ignore = "Phase 3: tests legacy JSON diag strings; SQLite equivalent in phase3_sqlite module"]
    fn settings_diag_log_writes_to_persistent_file() {
        let _env = setup_backup_env();
        let main_path = Settings::settings_path();
        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();

        // 壊れた main + 良い bak1 で recovery + diag log を発生させる。
        std::fs::write(&main_path, "{ corrupt").unwrap();
        let good = settings_with_favorite("diag_test");
        std::fs::write(
            backup_path(&main_path, 1),
            serde_json::to_string_pretty(&good).unwrap(),
        )
        .unwrap();

        let _ = Settings::load();

        let diag_path = crate::data_dir::logs_dir().join("settings.log");
        assert!(diag_path.exists(), "settings.log should be created");
        let content = std::fs::read_to_string(&diag_path).unwrap();
        assert!(
            content.contains("JSON parse failed") || content.contains("UTF-8 decode failed"),
            "diag log should record the parse failure, got: {content}"
        );
        assert!(
            content.contains("recovered from"),
            "diag log should record the recovery, got: {content}"
        );
    }

    /// `safe_version_label`: ファイル名に不適な文字を `_` 化する。
    #[test]
    fn safe_version_label_sanitizes() {
        assert_eq!(safe_version_label("0.9.0"), "0.9.0");
        assert_eq!(safe_version_label("1.0.0-rc1"), "1.0.0-rc1");
        assert_eq!(safe_version_label("evil/path"), "evil_path");
        assert_eq!(safe_version_label("..\\foo"), ".._foo");
        assert_eq!(safe_version_label(""), "unknown");
    }

    // =======================================================================
    // Phase 3 SQLite path tests
    //
    // 旧 JSON 経路の `#[ignore]` テストと等価なシナリオを SQLite 経路で検証する。
    // すべて `setup_backup_env` (= data_dir 共有 lock + state リセット) を使う。
    // =======================================================================
    mod phase3_sqlite {
        use super::*;

        fn data_db_path(env: &BackupTestEnv) -> PathBuf {
            // env は tempdir を保持しているが path 取得は data_dir::get() でできる。
            let _ = env;
            crate::data_dir::get().join("settings.db")
        }
        fn db_bak_path(env: &BackupTestEnv, n: usize) -> PathBuf {
            let _ = env;
            crate::data_dir::get().join(format!("settings.db.bak{n}"))
        }

        /// 普通の save→load ラウンドトリップで settings.db が作成され、内容が一致する。
        ///
        /// アプリ起動順序を模す: 必ず最初に `Settings::load()` で boot → 続いて `save()`。
        #[test]
        fn save_load_roundtrip() {
            let env = setup_backup_env();
            // boot で CleanInstall → settings.db を作る。
            let _initial = Settings::load();
            assert!(
                data_db_path(&env).exists(),
                "settings.db should be created on first load"
            );
            // ユーザー操作後の save。
            let s = settings_with_favorite("alpha");
            s.save();
            // legacy settings.json は生まれない。
            assert!(!crate::data_dir::get().join("settings.json").exists());
            // 別セッション相当で reload して内容を確認。
            reset_backup_state_for_test();
            let loaded = Settings::load();
            assert_eq!(loaded.favorites.len(), 1);
            assert_eq!(loaded.favorites[0].name, "alpha");
            assert_eq!(
                loaded.last_seen_version.as_deref(),
                Some(env!("CARGO_PKG_VERSION"))
            );
        }

        /// `thumb_aspect_auto` の save→load ラウンドトリップ。
        /// schema migration なしで `#[serde(default)]` のみで永続化されることを確認。
        #[test]
        fn thumb_aspect_auto_roundtrip() {
            let env = setup_backup_env();
            let _initial = Settings::load();
            assert!(data_db_path(&env).exists());

            let mut s = Settings::default();
            s.grid_view_mode = GridViewMode::Details;
            s.details_sort_key = DetailsSortKey::Size;
            s.details_sort_ascending = false;
            s.details_size_display_mode = DetailsSizeDisplayMode::FixedKb;
            s.details_timestamp_show_seconds = true;
            s.details_column_order = vec![
                DetailsColumnId::Size,
                DetailsColumnId::Name,
                DetailsColumnId::Modified,
            ];
            s.details_column_widths = vec![
                DetailsColumnWidth {
                    column: DetailsColumnId::Size,
                    width: 128.0,
                },
                DetailsColumnWidth {
                    column: DetailsColumnId::Modified,
                    width: 188.0,
                },
            ];
            s.details_show_preview = false;
            s.details_show_rating = false;
            s.details_show_tags = false;
            s.details_show_kind = false;
            s.details_show_size = false;
            s.details_show_modified = false;
            s.details_show_created = true;
            s.details_show_state = false;
            s.details_show_image_dimensions = true;
            s.details_show_video_duration = true;
            s.details_show_video_dimensions = true;
            s.details_show_video_codec = true;
            s.facet_filter.kinds.insert(FacetItemKind::Image);
            s.facet_filter.exts.insert("png".to_string());
            s.facet_filter
                .ai_models
                .insert("sd_xl_base_1.0".to_string());
            s.facet_filter.ai_tools.insert("ComfyUI".to_string());
            s.facet_filter.tags.insert("#原神".to_string());
            s.facet_filter.include_untagged = true;
            s.facet_filter.tag_mode = FacetTagMode::All;
            s.facet_filter.date_preset = Some(FacetDatePreset::Last30Days);
            s.facet_filter.size_preset = Some(FacetSizePreset::MiB10To100);
            s.facet_filter.edits.insert(FacetEditFlag::Tagged);
            s.facet_filter.edit_include_descendants = true;
            s.thumb_aspect_auto = true;
            s.thumb_aspect = ThumbAspect::Portrait2x3;
            s.thumb_tooltip_show_filename = false;
            s.thumb_tooltip_show_image_dimensions = false;
            s.thumb_tooltip_show_video_duration = false;
            s.thumb_tooltip_show_kind = true;
            s.thumb_tooltip_show_file_size = true;
            s.thumb_tooltip_show_modified = true;
            s.thumb_tooltip_show_created = true;
            s.thumb_tooltip_show_video_dimensions = true;
            s.thumb_tooltip_show_video_codec = true;
            s.thumb_tooltip_show_location = true;
            s.thumb_tooltip_show_reading_history_last_read = false;
            s.thumb_tooltip_show_reading_history_progress = false;
            s.fullscreen_left_panel_tab = FullscreenLeftPanelTab::ViewTrim;
            s.toolbar_cols_details_visible = false;
            s.toolbar_aspect_auto_visible = false;
            s.toolbar_cols_display = ToolbarSectionDisplay::Dropdown;
            s.toolbar_aspect_display = ToolbarSectionDisplay::Dropdown;
            s.toolbar_sort_display = ToolbarSectionDisplay::Dropdown;
            s.toolbar_favorites_display = ToolbarSectionDisplay::Collapsible;
            s.toolbar_tags_display = ToolbarSectionDisplay::Dropdown;
            s.toolbar_bookshelf_display = ToolbarSectionDisplay::Collapsible;
            s.toolbar_favorites_collapsed = true;
            s.toolbar_tags_collapsed = true;
            s.toolbar_bookshelf_collapsed = true;
            s.pinned_books = vec!["テスト本".to_string()];
            s.show_toolbar_bookshelf = false;
            s.show_toolbar_facet_filter = false;
            s.show_toolbar_cols = false;
            s.show_toolbar_aspect = false;
            s.show_toolbar_sort = false;
            s.toolbar_section_new_row = vec![ToolbarSectionId::Tags, ToolbarSectionId::Favorites];
            s.ring_shortcuts.mouse_flick_enabled = true;
            s.ring_shortcuts.gamepad_ring_enabled = false;
            s.ring_shortcuts.shift_wheel_pair =
                crate::ring_shortcut::WheelPairActionId::VideoVolumeUpDown;
            s.ring_shortcuts.alt_wheel_pair =
                crate::ring_shortcut::WheelPairActionId::FolderHistoryPrevNext;
            s.ring_shortcuts.mouse_buttons_grid.back =
                crate::ring_shortcut::RingActionId::GridParentFolder;
            s.ring_shortcuts.mouse_buttons_image.forward =
                crate::ring_shortcut::RingActionId::ImageSlideshow;
            s.ring_shortcuts.mouse_buttons_video.back =
                crate::ring_shortcut::RingActionId::VideoMute;
            s.ring_shortcuts.mouse_nav_prompt_done = true;
            s.ring_shortcuts.x_picker_hint_shown = true;
            s.ring_shortcuts.image.slots[crate::ring_shortcut::RingDirection::Right.slot_index()] =
                crate::ring_shortcut::RingActionId::ImageCapture;
            s.save();

            reset_backup_state_for_test();
            let loaded = Settings::load();
            assert!(
                loaded.thumb_aspect_auto,
                "thumb_aspect_auto should survive roundtrip"
            );
            assert_eq!(
                loaded.grid_view_mode,
                GridViewMode::Details,
                "grid_view_mode should survive roundtrip"
            );
            assert_eq!(
                loaded.details_sort_key,
                DetailsSortKey::Size,
                "details_sort_key should survive roundtrip"
            );
            assert!(
                !loaded.details_sort_ascending,
                "details_sort_ascending should survive roundtrip"
            );
            assert_eq!(
                loaded.details_size_display_mode,
                DetailsSizeDisplayMode::FixedKb,
                "details_size_display_mode should survive roundtrip"
            );
            assert!(
                loaded.details_timestamp_show_seconds,
                "details_timestamp_show_seconds should survive roundtrip"
            );
            assert_eq!(
                loaded.details_column_order.first().copied(),
                Some(DetailsColumnId::Preview),
                "new preview column should be inserted at the default leading position"
            );
            assert_eq!(
                loaded.details_column_order.get(1).copied(),
                Some(DetailsColumnId::Size),
                "details_column_order should survive roundtrip"
            );
            assert!(
                loaded
                    .details_column_widths
                    .iter()
                    .any(|entry| entry.column == DetailsColumnId::Size
                        && (entry.width - 128.0).abs() < 0.1),
                "details_column_widths should survive roundtrip"
            );
            assert!(
                !loaded.details_show_preview,
                "details_show_preview should survive roundtrip"
            );
            assert!(
                !loaded.details_show_rating,
                "details_show_rating should survive roundtrip"
            );
            assert!(
                !loaded.details_show_tags,
                "details_show_tags should survive roundtrip"
            );
            assert!(
                !loaded.details_show_kind,
                "details_show_kind should survive roundtrip"
            );
            assert!(
                !loaded.details_show_size,
                "details_show_size should survive roundtrip"
            );
            assert!(
                !loaded.details_show_modified,
                "details_show_modified should survive roundtrip"
            );
            assert!(
                loaded.details_show_created,
                "details_show_created should survive roundtrip"
            );
            assert!(
                !loaded.details_show_state,
                "details_show_state should survive roundtrip"
            );
            assert!(
                loaded.details_show_image_dimensions,
                "details_show_image_dimensions should survive roundtrip"
            );
            assert!(
                loaded.details_show_video_duration,
                "details_show_video_duration should survive roundtrip"
            );
            assert!(
                loaded.details_show_video_dimensions,
                "details_show_video_dimensions should survive roundtrip"
            );
            assert!(
                loaded.details_show_video_codec,
                "details_show_video_codec should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.kinds.contains(&FacetItemKind::Image),
                "facet_filter kinds should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.exts.contains("png"),
                "facet_filter extensions should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.ai_models.contains("sd_xl_base_1.0"),
                "facet_filter AI model names should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.ai_tools.contains("ComfyUI"),
                "facet_filter AI tool names should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.tags.contains("原神"),
                "facet_filter tags should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.include_untagged,
                "facet_filter include_untagged should survive roundtrip"
            );
            assert_eq!(
                loaded.facet_filter.tag_mode,
                FacetTagMode::All,
                "facet_filter tag mode should survive roundtrip"
            );
            assert_eq!(
                loaded.facet_filter.date_preset,
                Some(FacetDatePreset::Last30Days),
                "facet_filter date preset should survive roundtrip"
            );
            assert_eq!(
                loaded.facet_filter.size_preset,
                Some(FacetSizePreset::MiB10To100),
                "facet_filter size preset should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.edits.contains(&FacetEditFlag::Tagged),
                "facet_filter edit flags should survive roundtrip"
            );
            assert!(
                loaded.facet_filter.edit_include_descendants,
                "facet_filter edit descendant option should survive roundtrip"
            );
            assert!(
                !loaded.toolbar_cols_details_visible,
                "toolbar_cols_details_visible (false override) should survive roundtrip"
            );
            assert!(
                !loaded.toolbar_aspect_auto_visible,
                "toolbar_aspect_auto_visible (false override) should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_cols_display,
                ToolbarSectionDisplay::Dropdown,
                "toolbar_cols_display should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_aspect_display,
                ToolbarSectionDisplay::Dropdown,
                "toolbar_aspect_display should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_sort_display,
                ToolbarSectionDisplay::Dropdown,
                "toolbar_sort_display should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_favorites_display,
                ToolbarSectionDisplay::Collapsible,
                "toolbar_favorites_display should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_tags_display,
                ToolbarSectionDisplay::Dropdown,
                "toolbar_tags_display should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_bookshelf_display,
                ToolbarSectionDisplay::Collapsible,
                "toolbar_bookshelf_display should survive roundtrip"
            );
            assert!(
                loaded.toolbar_favorites_collapsed
                    && loaded.toolbar_tags_collapsed
                    && loaded.toolbar_bookshelf_collapsed,
                "toolbar *_collapsed flags should survive roundtrip"
            );
            assert_eq!(
                loaded.pinned_books,
                vec!["テスト本".to_string()],
                "pinned_books should survive roundtrip"
            );
            assert!(
                !loaded.show_toolbar_bookshelf,
                "show_toolbar_bookshelf (false override) should survive roundtrip"
            );
            assert!(
                !loaded.show_toolbar_facet_filter,
                "show_toolbar_facet_filter (false override) should survive roundtrip"
            );
            assert!(
                !loaded.show_toolbar_cols
                    && !loaded.show_toolbar_aspect
                    && !loaded.show_toolbar_sort,
                "show_toolbar_cols/aspect/sort (false override) should survive roundtrip"
            );
            assert_eq!(
                loaded.toolbar_section_new_row,
                vec![ToolbarSectionId::Tags, ToolbarSectionId::Favorites],
                "toolbar_section_new_row should survive roundtrip"
            );
            assert!(
                loaded.ring_shortcuts.mouse_flick_enabled,
                "ring shortcut mouse toggle should survive roundtrip"
            );
            assert!(
                loaded.ring_shortcuts.gamepad_ring_enabled,
                "legacy false gamepad X ring toggle should be normalized"
            );
            assert_eq!(
                loaded.ring_shortcuts.shift_wheel_pair,
                crate::ring_shortcut::WheelPairActionId::VideoVolumeUpDown,
                "ring shortcut Shift wheel pair should survive roundtrip"
            );
            assert_eq!(
                loaded.ring_shortcuts.alt_wheel_pair,
                crate::ring_shortcut::WheelPairActionId::FolderHistoryPrevNext,
                "ring shortcut Alt wheel pair should survive roundtrip"
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_back_forward_action,
                crate::ring_shortcut::MouseBackForwardActionId::None,
                "legacy mouse back/forward action should remain migrated"
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_buttons_grid.back,
                crate::ring_shortcut::RingActionId::GridParentFolder,
                "grid mouse back button action should survive roundtrip"
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_buttons_image.forward,
                crate::ring_shortcut::RingActionId::ImageSlideshow,
                "image mouse forward button action should survive roundtrip"
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_buttons_video.back,
                crate::ring_shortcut::RingActionId::VideoMute,
                "video mouse back button action should survive roundtrip"
            );
            assert!(
                loaded.ring_shortcuts.mouse_nav_prompt_done,
                "mouse nav prompt flag should survive roundtrip"
            );
            assert!(
                loaded.ring_shortcuts.x_picker_hint_shown,
                "ring shortcut X picker hint flag should survive roundtrip"
            );
            assert_eq!(
                loaded.ring_shortcuts.image.slots
                    [crate::ring_shortcut::RingDirection::Right.slot_index()],
                crate::ring_shortcut::RingActionId::ImageCapture,
                "ring shortcut slots should survive roundtrip"
            );
            assert_eq!(
                loaded.thumb_aspect,
                ThumbAspect::Portrait2x3,
                "manual thumb_aspect should also be preserved"
            );
            assert!(
                !loaded.thumb_tooltip_show_filename,
                "thumb tooltip filename flag should survive roundtrip"
            );
            assert!(
                !loaded.thumb_tooltip_show_image_dimensions,
                "thumb tooltip image dimensions flag should survive roundtrip"
            );
            assert!(
                !loaded.thumb_tooltip_show_video_duration,
                "thumb tooltip video duration flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_kind,
                "thumb tooltip kind flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_file_size,
                "thumb tooltip file size flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_modified,
                "thumb tooltip modified flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_created,
                "thumb tooltip created flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_video_dimensions,
                "thumb tooltip video dimensions flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_video_codec,
                "thumb tooltip video codec flag should survive roundtrip"
            );
            assert!(
                loaded.thumb_tooltip_show_location,
                "thumb tooltip location flag should survive roundtrip"
            );
            assert!(
                !loaded.thumb_tooltip_show_reading_history_last_read,
                "thumb tooltip reading-history last-read flag should survive roundtrip"
            );
            assert!(
                !loaded.thumb_tooltip_show_reading_history_progress,
                "thumb tooltip reading-history progress flag should survive roundtrip"
            );
            assert_eq!(
                loaded.fullscreen_left_panel_tab,
                FullscreenLeftPanelTab::ViewTrim,
                "fullscreen left panel tab should survive roundtrip"
            );
        }

        /// プロセス内最初の save だけ rotate_db_backups が走り、2 回目以降は走らない。
        /// reset_backup_state_for_test() で flag を戻すと次の save で再び rotate される。
        #[test]
        fn rotation_runs_once_per_session() {
            let env = setup_backup_env();
            let _initial = Settings::load();
            // 初回 user save: rotate_backups が走り、現在の DB を bak1 に snapshot する。
            let s1 = settings_with_favorite("first");
            s1.save();
            assert!(
                db_bak_path(&env, 1).exists(),
                "bak1 should be created by initial rotate (VACUUM INTO snapshot)"
            );
            let bak1_mtime_after_first = std::fs::metadata(db_bak_path(&env, 1))
                .unwrap()
                .modified()
                .unwrap();

            // 同セッション内の 2 回目 save は rotate を走らせない (mtime 不変)。
            // small sleep to ensure mtime granularity allows detection (no-op assert)
            let mut s2 = settings_with_favorite("second");
            s2.add_favorite("xxx".into(), PathBuf::from(r"C:\xxx"));
            s2.save();
            let bak1_mtime_after_second = std::fs::metadata(db_bak_path(&env, 1))
                .unwrap()
                .modified()
                .unwrap();
            assert_eq!(
                bak1_mtime_after_first, bak1_mtime_after_second,
                "second save in same session must not re-rotate bak1"
            );

            // 別セッションを模す: flag リセット。次の save で bak1 → bak2 → ...
            reset_backup_state_for_test();
            let s3 = settings_with_favorite("third");
            s3.save();
            assert!(
                db_bak_path(&env, 2).exists(),
                "bak2 should appear after second session rotation"
            );
        }

        /// 12 回 rotate しても bak10 までしか残らず、bak11 は作られない。
        #[test]
        fn rotation_caps_at_10_generations() {
            let env = setup_backup_env();
            // 初回 boot で settings.db を作る。
            let _ = Settings::load();
            // セッション 1..=12 を模す。
            for i in 1..=12 {
                reset_backup_state_for_test();
                let s = settings_with_favorite(&format!("gen{i}"));
                s.save();
            }
            for n in 1..=10 {
                assert!(
                    db_bak_path(&env, n).exists(),
                    "bak{n} should exist after 12 rotations"
                );
            }
            assert!(
                !db_bak_path(&env, 11).exists(),
                "bak11 must not exist (10 generations only)"
            );
        }

        /// バージョン変化時に `.preupgrade-v<old>` snapshot が VACUUM INTO で作られる。
        #[test]
        fn version_preupgrade_snapshot() {
            let env = setup_backup_env();
            // 初回 boot で settings.db 作成。
            let _ = Settings::load();
            // 旧バージョンを設定済みの状態を作る。Settings::load() が走った後で last_seen_version
            // は現バージョンに更新済みなので、テスト用に旧値を上書き保存する。
            let mut older = settings_with_favorite("preupgrade_target");
            older.last_seen_version = Some("0.0.0-prev-test".to_string());
            older.save();
            // 新セッション: last_seen_version は旧値のまま読み込まれ、version_changed=true
            // で preupgrade snapshot が作られる。
            reset_backup_state_for_test();
            let loaded = Settings::load();
            assert_eq!(
                loaded.last_seen_version.as_deref(),
                Some(env!("CARGO_PKG_VERSION"))
            );
            let pre = crate::data_dir::get().join(format!(
                "settings.db.preupgrade-v{}",
                safe_version_label("0.0.0-prev-test")
            ));
            assert!(
                pre.exists(),
                "preupgrade snapshot should be created at {}",
                pre.display()
            );
            // ファイル単体で開けるか確認 (= SettingsDb として valid な snapshot)。
            let other = tempfile::TempDir::new().unwrap();
            std::fs::copy(&pre, other.path().join("settings.db")).unwrap();
            let restored = crate::settings_db::SettingsDb::open(other.path()).unwrap();
            let restored_settings = restored.load_into_settings().unwrap();
            assert_eq!(
                restored_settings.last_seen_version.as_deref(),
                Some("0.0.0-prev-test")
            );
            let _ = env;
        }

        /// 同じ「前バージョン」名の preupgrade snapshot が既に存在するなら上書きしない。
        #[test]
        fn version_preupgrade_snapshot_not_overwritten() {
            let env = setup_backup_env();
            // 初回 boot で settings.db 作成。
            let _ = Settings::load();
            // 旧バージョンを設定済み状態にする。
            let mut older = settings_with_favorite("collision_target");
            older.last_seen_version = Some("0.0.0-prev-test".to_string());
            older.save();
            // 同じ "前バージョン" 名の snapshot を手作業で配置 (= 既存ファイル相当)。
            let pre = crate::data_dir::get().join(format!(
                "settings.db.preupgrade-v{}",
                safe_version_label("0.0.0-prev-test")
            ));
            std::fs::write(&pre, b"EXISTING_SENTINEL").unwrap();
            reset_backup_state_for_test();
            let _ = Settings::load();
            // 既存ファイルは温存されている (= 上書きしていない)。
            assert_eq!(std::fs::read(&pre).unwrap(), b"EXISTING_SENTINEL");
            let _ = env;
        }

        /// 真の初回起動 (= 何もない dir) では SQLite 経路でも save 抑止は立たず、
        /// CleanInstall として settings.db が作られる。
        #[test]
        fn pristine_first_launch() {
            let env = setup_backup_env();
            let loaded = Settings::load();
            assert_eq!(loaded.favorites.len(), 0);
            assert_eq!(
                loaded.ring_shortcuts.mouse_back_forward_action,
                crate::ring_shortcut::MouseBackForwardActionId::None
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_buttons_grid.back,
                crate::ring_shortcut::RingActionId::GridHistoryBack
            );
            assert_eq!(
                loaded.ring_shortcuts.mouse_buttons_grid.forward,
                crate::ring_shortcut::RingActionId::GridHistoryForward
            );
            assert!(
                loaded.ring_shortcuts.mouse_nav_prompt_done,
                "clean install should silently accept the new mouse back/forward default"
            );
            // load() 内で migration/version トリガで save() が走るか、または明示的に save。
            let mut s = loaded;
            s.add_favorite("first_install".into(), PathBuf::from(r"C:\first"));
            s.save();
            assert!(
                data_db_path(&env).exists(),
                "settings.db must exist after first save"
            );
            let reloaded = Settings::load();
            assert_eq!(reloaded.favorites[0].name, "first_install");
        }

        /// settings.db を壊して bak1 から復旧されるシナリオ (= spec §5 decision tree)。
        #[test]
        fn corrupt_main_recovers_from_bak() {
            let env = setup_backup_env();
            // 初回 boot + 1 回 save で bak1 を作る (= rotate_backups で VACUUM INTO)。
            let _ = Settings::load();
            let s = settings_with_favorite("good_state");
            s.save();
            assert!(db_bak_path(&env, 1).exists());
            // main DB を壊す。WAL / SHM も削除して状態を綺麗に。
            std::fs::write(data_db_path(&env), b"NOT A SQLITE DB").unwrap();
            let _ = std::fs::remove_file(crate::data_dir::get().join("settings.db-wal"));
            let _ = std::fs::remove_file(crate::data_dir::get().join("settings.db-shm"));
            reset_backup_state_for_test();
            let loaded = Settings::load();
            assert_eq!(
                loaded.favorites[0].name, "good_state",
                "should recover from bak1"
            );
        }

        /// 全壊 (= main 壊、bak も無く JSON も無い) で開けない状況なら save 抑止。
        /// SQLite 経路では Corrupted 検出時に main DB を quarantine するので、
        /// 元ファイルは `.corrupted-<ts>-<seq>` にリネームされる。
        #[test]
        fn failed_fallback_sets_save_suppressed() {
            let env = setup_backup_env();
            // main を壊し、bak を一切置かない、JSON も無い状態を作る。
            std::fs::write(data_db_path(&env), b"NOT A SQLITE DB").unwrap();
            let loaded = Settings::load();
            // boot は FailedFallbackDefault に倒れ、save 抑止フラグが立つ。
            assert!(MAIN_UNREADABLE_THIS_SESSION.load(Ordering::Relaxed));
            assert!(crate::settings_db::save_suppressed());
            // 続く save() は no-op (= 新しい main DB は作られない、quarantine もうこれ以上発生しない)。
            let mut s = loaded.clone();
            s.add_favorite("ignored".into(), PathBuf::from(r"C:\ignored"));
            s.save();
            // 壊れた main は quarantine されて `.corrupted-*` にリネーム済み。
            // 新しい settings.db は作られない (= save 抑止)。
            assert!(
                !data_db_path(&env).exists(),
                "after FailedFallbackDefault, suppressed save must not create a new settings.db"
            );
            let mut found_corrupted = false;
            for entry in std::fs::read_dir(crate::data_dir::get()).unwrap().flatten() {
                if entry.file_name().to_string_lossy().contains(".corrupted-") {
                    found_corrupted = true;
                    break;
                }
            }
            assert!(
                found_corrupted,
                "corrupt main should be quarantined as .corrupted-*"
            );
            let _ = env;
        }

        /// Codex P2 v13 (2026-05-14): `Settings::load()` 内の migration/version
        /// writeback が rotation を消費しないこと。次の真の user save が初めて
        /// rotation を発火させて bak1 を作る。
        #[test]
        fn load_writeback_does_not_consume_rotation() {
            let env = setup_backup_env();
            // 初回 boot で clean install → settings.db 作成。load() 内部で
            // version_changed=true なので save_internal_no_rotation が走る。
            // ここで rotation が走ってしまっていないか確認する。
            let _ = Settings::load();
            // bak1 はまだ無いはず (= load() 内 writeback が rotation を発火させていない)。
            assert!(
                !db_bak_path(&env, 1).exists(),
                "load()-internal writeback must NOT trigger rotation (bak1 should not exist yet)"
            );
            // BACKUP_DONE_THIS_SESSION も立っていないはず。
            assert!(
                !BACKUP_DONE_THIS_SESSION.load(Ordering::Relaxed),
                "BACKUP_DONE_THIS_SESSION must not be set by load()-internal save"
            );
            // 次の **user** save で初めて rotation が走り bak1 が作られる。
            let s = settings_with_favorite("real_user_save");
            s.save();
            assert!(
                db_bak_path(&env, 1).exists(),
                "first user save() should now create bak1 (rotation finally consumed)"
            );
        }

        /// diag log がブート経路を 1 行記録する (Codex P2 #3 の SQLite 等価)。
        #[test]
        fn diag_log_records_boot_path() {
            let env = setup_backup_env();
            let _ = Settings::load();
            let diag = crate::data_dir::logs_dir().join("settings.log");
            assert!(diag.exists(), "settings.log should be created");
            let content = std::fs::read_to_string(&diag).unwrap();
            // boot path or migration kind が記録されている。
            assert!(
                content.contains("settings_db: boot") || content.contains("settings: boot source"),
                "diag log should mention boot path, got:\n{content}"
            );
            let _ = env;
        }
    }
}
