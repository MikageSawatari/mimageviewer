use std::path::PathBuf;
use uuid::Uuid;

const MAX_FAVORITES: usize = 20;

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
    /// Ctrl+S (フォルダ/ZIP/PDF 名) の自動インデックス対象にするか。
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
/// `name` は `#` を除いた表示名 (例: "原神")。付与時に `#` が自動で付加されて
/// XMP dc:subject に "#原神" として書き込まれる。
///
/// `id` は順序変更・改名時の安定識別子。ツールバー/メニューのキー、
/// 将来の統計情報等で使用。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TagDef {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
}

impl TagDef {
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
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
    /// 日付ソートで mtime が等しい場合はファイル名昇順で tiebreak する。`mtime_secs`
    /// は秒精度なので、同一秒に作成・更新されたファイル群が `read_dir` 順 (FS 依存で
    /// 不安定) に並ぶのを防ぐ。
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
            Self::Numeric => natural_key(name_a).cmp(&natural_key(name_b)),
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
// SpreadMode (見開き表示)
// -----------------------------------------------------------------------

/// 見開き表示モード。
///
/// - `Single`: 通常の1ページ表示
/// - `Ltr`: 見開き 左→右（表紙なし）— [0,1] [2,3] ...
/// - `LtrCover`: 見開き 左→右（表紙あり）— [0] [1,2] [3,4] ...
/// - `Rtl`: 見開き 右→左（表紙なし）— [0,1] [2,3] ...
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

/// - `RtlCover`: 見開き 右→左（表紙あり）— [0] [1,2] [3,4] ...
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum SpreadMode {
    #[default]
    Single,
    Ltr,
    LtrCover,
    Rtl,
    RtlCover,
}

impl SpreadMode {
    /// 見開きモードか
    pub fn is_spread(self) -> bool {
        !matches!(self, Self::Single)
    }

    /// 右→左（RTL）モードか
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl | Self::RtlCover)
    }

    /// 表紙（1ページ目単独表示）ありか
    pub fn has_cover(self) -> bool {
        matches!(self, Self::LtrCover | Self::RtlCover)
    }

    /// 整数値 (0-4) から生成
    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::Ltr,
            2 => Self::LtrCover,
            3 => Self::Rtl,
            4 => Self::RtlCover,
            _ => Self::Single,
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
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "1ページ表示",
            Self::Ltr => "見開き 左→右",
            Self::LtrCover => "見開き 左→右（表紙あり）",
            Self::Rtl => "見開き 右→左",
            Self::RtlCover => "見開き 右→左（表紙あり）",
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
// Settings
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_grid_cols")]
    pub grid_cols: usize,
    #[serde(default)]
    pub thumb_aspect: ThumbAspect,
    #[serde(default)]
    pub favorites: Vec<FavoriteEntry>,
    #[serde(default)]
    pub last_folder: Option<PathBuf>,
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
    /// アドレスバー (フォルダ入力行) を表示する
    #[serde(default = "default_true")]
    pub show_toolbar_folder: bool,
    /// ツールバーに「上のフォルダへ」ボタンを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_parent_button: bool,
    /// ツールバーに「前のフォルダへ」ボタンを表示する (Phase 5.8)。
    /// 既定 true、Ctrl+↑ と等価。
    #[serde(default = "default_true")]
    pub show_toolbar_prev_folder: bool,
    /// ツールバーに「次のフォルダへ」ボタンを表示する (Phase 5.8)。
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

    // ── 見開き表示 ──────────────────────────────────────────
    /// デフォルトの見開き表示モード
    #[serde(default)]
    pub default_spread_mode: SpreadMode,

    // ── UI テーマ (v0.7.0) ──────────────────────────────────────
    /// 背景色テーマ (System / Light / Dark)。デフォルト `System` で Windows のアプリ用色に追従。
    #[serde(default)]
    pub ui_theme: UiTheme,

    // ── ツールバー項目フィルタ（Vec が空 = セクション非表示）──
    /// ツールバーに表示する列数の選択肢
    #[serde(default = "default_toolbar_cols_items")]
    pub toolbar_cols_items: Vec<usize>,
    /// ツールバーに表示するアスペクト比の選択肢
    #[serde(default = "default_toolbar_aspect_items")]
    pub toolbar_aspect_items: Vec<ThumbAspect>,
    /// ツールバーに表示するソート順の選択肢
    #[serde(default = "default_toolbar_sort_items")]
    pub toolbar_sort_items: Vec<SortOrder>,

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

    /// AI アップスケール: スキップしきい値（この値以上の画像はスキップ）
    #[serde(default = "default_ai_upscale_skip_px")]
    pub ai_upscale_skip_px: u32,

    /// AI ノイズ除去: スキップしきい値（この値以上の画像はスキップ）
    #[serde(default = "default_ai_denoise_skip_px")]
    pub ai_denoise_skip_px: u32,

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

    // ── 動画インライン再生 ────────────────────────────────────────
    /// 動画再生時の既定音量 (0.0-1.5)。1.0 を超える値は音声ポンプ側で
    /// pre-limiter boost として扱う。
    #[serde(default = "default_video_volume")]
    pub video_volume: f64,
    /// フルスクリーン化時に自動再生を開始するか (旧: bool)。
    /// Phase 7.J で `VideoAutoplayMode` に拡張。現在の UI は
    /// Off / Always の 2 択で、OnlyFromGrid は旧設定互換として Off に正規化する。
    /// 新フィールド `video_autoplay_mode` を見るのが推奨。本フィールドは migration 用に残す:
    /// `video_autoplay_mode` がデフォルト値 (= 未保存) のときだけ参照される。
    #[serde(default)]
    pub video_autoplay: bool,
    /// 動画フルスクリーン時の自動再生ポリシー (Phase 7.J)。
    #[serde(default)]
    pub video_autoplay_mode: VideoAutoplayMode,
    /// 終端到達時に先頭から再生を繰り返すか。
    #[serde(default)]
    pub video_loop: bool,
    /// 起動時にミュートで開始するか (オフィス環境などでの保険)。
    #[serde(default)]
    pub video_start_muted: bool,
    /// 動画ファイルごとの最終再生位置 (絶対パス → 秒)。
    /// `VideoPlayer::open` 時に自動 resume、5 秒ごと + drop 時に保存。
    /// 動画末尾近く (残り 5 秒以内) は 0 にリセットして "次回最初から" の挙動。
    #[serde(default)]
    pub video_resume_positions: std::collections::HashMap<String, f64>,
    /// ハードウェアデコードを利用するか (Windows D3D11VA)。失敗時は自動的に SW にフォールバック。
    /// 初期は OFF (安定優先)。HEVC / 4K 動画の CPU 負荷を大きく下げるため、動作確認後に
    /// 有効化することを想定。
    #[serde(default)]
    pub video_hw_decode: bool,
    /// インターレース動画のデインターレース処理。
    /// Auto は FFmpeg frame の interlaced flag が立ったフレームのみ bwdif を適用する。
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
    /// 6/10/16/20/26/30 のいずれかに切替可能。値が範囲外なら 10 にクランプ。
    #[serde(default = "default_video_tile_columns")]
    pub video_tile_columns: usize,

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
    #[serde(default)]
    pub vst3_chain_slots: Vst3ChainPresetSlots,
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

/// 動画音量の既定値。100% (= boost なし)。
pub const VIDEO_VOLUME_DEFAULT: f64 = 1.0;
/// 動画音量の上限。150% は +3.5dB 程度の手動 boost。
pub const VIDEO_VOLUME_MAX: f64 = 1.5;

pub fn clamp_video_volume(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, VIDEO_VOLUME_MAX)
    } else {
        VIDEO_VOLUME_DEFAULT
    }
}

/// 動画タイルモード列数の候補 (Phase 6.D)。
pub const VIDEO_TILE_COLUMN_CANDIDATES: &[usize] = &[6, 10, 16, 20, 26, 30];

fn default_video_tile_columns() -> usize {
    10
}

/// 動画フルスクリーン時の自動再生ポリシー (Phase 7.J)。
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

/// 動画再生時のデインターレース設定。
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoDeinterlaceMode {
    /// デコードフレームが interlaced と示しているときだけ bwdif を適用する。
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

/// グリッド列数の最小値
pub const MIN_GRID_COLS: usize = 1;
/// グリッド列数の最大値
pub const MAX_GRID_COLS: usize = 10;

fn default_grid_cols() -> usize {
    4
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
fn default_toolbar_cols_items() -> Vec<usize> {
    (MIN_GRID_COLS..=MAX_GRID_COLS).collect()
}
fn default_toolbar_aspect_items() -> Vec<ThumbAspect> {
    ThumbAspect::all().to_vec()
}
fn default_toolbar_sort_items() -> Vec<SortOrder> {
    SortOrder::all().to_vec()
}
pub fn default_rating_filter() -> [bool; 6] {
    [true; 6]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            grid_cols: default_grid_cols(),
            thumb_aspect: ThumbAspect::default(),
            favorites: Vec::new(),
            last_folder: None,
            window_pos: None,
            window_size: None,
            parallelism: Parallelism::default(),
            prefetch_back: default_prefetch_back(),
            prefetch_forward: default_prefetch_forward(),
            folder_skip_limit: default_folder_skip_limit(),
            sort_order: SortOrder::default(),
            thumb_px: default_thumb_px(),
            thumb_quality: default_thumb_quality(),
            cache_policy: CachePolicy::default(),
            cache_threshold_ms: default_cache_threshold_ms(),
            cache_size_threshold_bytes: default_cache_size_threshold_bytes(),
            cache_videos_always: true,
            cache_webp_always: true,
            cache_pdf_always: true,
            cache_zip_always: true,
            batch_cache_zip_contents: false,
            batch_cache_pdf_contents: false,
            search_index_checks: Vec::new(),
            indexer_speed_profile: IndexerSpeedProfile::default(),
            thumb_prev_pages: default_thumb_prev_pages(),
            thumb_next_pages: default_thumb_next_pages(),
            thumb_vram_cap_percent: default_thumb_vram_cap_percent(),
            thumb_idle_upgrade: true,
            exif_hidden_tags: default_exif_hidden_tags(),
            skip_zip_if_folder_exists: true,
            skip_image_if_video_exists: true,
            skip_duplicate_images: true,
            image_ext_priority: default_image_ext_priority(),
            slideshow_interval_secs: default_slideshow_interval(),
            default_spread_mode: SpreadMode::default(),
            ui_theme: UiTheme::default(),
            tags: Vec::new(),
            show_toolbar_favorites: true,
            show_toolbar_tags: true,
            show_toolbar_folder: true,
            show_toolbar_parent_button: true,
            show_toolbar_prev_folder: true,
            show_toolbar_next_folder: true,
            show_toolbar_vst3: true,
            show_toolbar_rating: true,
            rating_filter: default_rating_filter(),
            toolbar_cols_items: default_toolbar_cols_items(),
            toolbar_aspect_items: default_toolbar_aspect_items(),
            toolbar_sort_items: default_toolbar_sort_items(),
            folder_thumb_sort: default_folder_thumb_sort(),
            folder_thumb_depth: default_folder_thumb_depth(),
            recent_open_with_apps: Vec::new(),
            custom_open_with_apps: Vec::new(),
            ai_upscale_enabled: false,
            ai_upscale_model_override: None,
            ai_upscale_prefetch_back: default_ai_upscale_prefetch_back(),
            ai_upscale_prefetch_forward: default_ai_upscale_prefetch_forward(),
            ai_upscale_skip_px: default_ai_upscale_skip_px(),
            ai_denoise_skip_px: default_ai_denoise_skip_px(),
            ai_backend: None,
            global_preset: crate::adjustment::AdjustParams::default(),
            preset_slots: crate::adjustment::PresetSlots::default(),
            sidecar_backup_enabled: true,
            susie_enabled: true,
            susie_allow_parallel: true,
            minimize_to_tray_on_close: false,
            pause_indexer_while_minimized: false,
            write_rating_to_xmp: false,
            update_check_enabled: true,
            update_check_dismissed_version: None,
            video_volume: default_video_volume(),
            video_autoplay: false,
            video_autoplay_mode: VideoAutoplayMode::default(),
            video_loop: false,
            video_start_muted: false,
            video_resume_positions: std::collections::HashMap::new(),
            video_hw_decode: false,
            video_deinterlace: VideoDeinterlaceMode::default(),
            video_thumb_use_sidecar_image: true,
            video_tile_columns: default_video_tile_columns(),
            vst3_enabled: false,
            vst3_plugins: Vec::new(),
            vst3_plugin_path: None,
            vst3_plugin_state: None,
            vst3_gui_visible: true,
            vst3_video_compact: false,
            vst3_chain_slots: Vst3ChainPresetSlots::default(),
        }
    }
}

impl Settings {
    fn settings_path() -> PathBuf {
        crate::data_dir::get().join("settings.json")
    }

    /// UUID でお気に入りを引く。UI ドロップダウン等で `Option<Uuid>` を表示するときに使う。
    pub fn favorite_by_id(&self, id: uuid::Uuid) -> Option<&FavoriteEntry> {
        self.favorites.iter().find(|f| f.id == id)
    }

    pub fn load() -> Self {
        let path = Self::settings_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("settings load failed: {} ({})", path.display(), e);
                return Self::default();
            }
        };
        let mut settings: Self = serde_json::from_str(&data).unwrap_or_else(|e| {
            eprintln!("settings JSON parse failed: {} ({})", path.display(), e);
            Self::default()
        });
        // migration: UUID nil チェック → 割り当てが発生したかを検出
        let had_nil_uuids = settings.favorites.iter().any(|f| f.id.is_nil());
        let autoplay_mode_migrated =
            settings.video_autoplay_mode == VideoAutoplayMode::OnlyFromGrid;
        let video_volume_before_sanitize = settings.video_volume;
        // migration: VST3 旧形式 (vst3_plugin_path / vst3_plugin_state) を Vec に移送
        let vst3_migrated = settings.migrate_vst3_legacy();
        settings.sanitize();
        let video_volume_sanitized =
            (settings.video_volume - video_volume_before_sanitize).abs() > 1.0e-9;
        // 新規 UUID を発行したので settings.json に書き戻して永続化する。
        // これで次回起動以降は sanitize でのマイグレーションが不要になる。
        if had_nil_uuids || vst3_migrated || autoplay_mode_migrated || video_volume_sanitized {
            settings.save();
        }
        settings
    }

    /// v0.9.0 開発初期版の単一 VST3 プラグイン形式 (`vst3_plugin_path` + `vst3_plugin_state`)
    /// から Vec 形式 (`vst3_plugins`) への migration。
    /// 一度実行されたら旧フィールドは None にクリアし、次回 save で settings.json から消える。
    /// 戻り値: migration が発生したか (= save が必要か)。
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
        self.video_volume = clamp_video_volume(self.video_volume);

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
        self.thumb_aspect = src.thumb_aspect;
        self.sort_order = src.sort_order;
        self.rating_filter = src.rating_filter;
        // ── サムネイル画質 (A/B 比較ダイアログで編集) ──
        self.thumb_px = src.thumb_px;
        self.thumb_quality = src.thumb_quality;
        // ── キャッシュ系 (環境設定に出ていない項目) ──
        self.cache_videos_always = src.cache_videos_always;
        self.batch_cache_zip_contents = src.batch_cache_zip_contents;
        self.batch_cache_pdf_contents = src.batch_cache_pdf_contents;
        // ── ウィンドウ / ナビゲーション状態 ──
        self.last_folder = src.last_folder.take();
        self.window_pos = src.window_pos;
        self.window_size = src.window_size;
        // ── お気に入り / タグ (専用ダイアログで編集) ──
        self.favorites = std::mem::take(&mut src.favorites);
        self.tags = std::mem::take(&mut src.tags);
        // ── 検索インデックス関連 ──
        self.search_index_checks = std::mem::take(&mut src.search_index_checks);
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
        // `vst3_video_compact` も同様 (= プレイバックパネルで切替)。
        self.vst3_gui_visible = src.vst3_gui_visible;
        self.vst3_video_compact = src.vst3_video_compact;
        self.vst3_chain_slots = std::mem::take(&mut src.vst3_chain_slots);
    }

    pub fn save(&self) {
        let path = Self::settings_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("settings dir create failed: {} ({})", parent.display(), e);
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("settings save failed: {} ({})", path.display(), e);
                }
            }
            Err(e) => {
                eprintln!("settings serialize failed: {e}");
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

    // -- Settings defaults --

    #[test]
    fn settings_default_values() {
        let s = Settings::default();
        assert_eq!(s.grid_cols, 4);
        assert_eq!(s.thumb_aspect, ThumbAspect::Square);
        assert!(s.favorites.is_empty());
        assert!(s.last_folder.is_none());
        assert!(s.window_pos.is_none());
        assert!(s.window_size.is_none());
        assert_eq!(s.prefetch_back, 4);
        assert_eq!(s.prefetch_forward, 12);
        assert_eq!(s.folder_skip_limit, 5);
        assert_eq!(s.sort_order, SortOrder::FileName);
        assert_eq!(s.thumb_px, 512);
        assert_eq!(s.thumb_quality, 75);
        assert_eq!(s.cache_policy, CachePolicy::Auto);
        assert_eq!(s.cache_threshold_ms, 25);
        assert_eq!(s.cache_size_threshold_bytes, 2_000_000);
        assert!(s.cache_videos_always);
        assert!(s.cache_webp_always);
        assert_eq!(s.thumb_prev_pages, 2);
        assert_eq!(s.thumb_next_pages, 4);
        assert_eq!(s.thumb_vram_cap_percent, 50);
        assert!(s.thumb_idle_upgrade);
        assert!(s.show_toolbar_favorites);
        assert!(s.show_toolbar_folder);
    }

    // -- Settings JSON roundtrip --

    #[test]
    fn settings_roundtrip_json() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.grid_cols, original.grid_cols);
        assert_eq!(loaded.thumb_px, original.thumb_px);
        assert_eq!(loaded.thumb_quality, original.thumb_quality);
        assert_eq!(loaded.cache_threshold_ms, original.cache_threshold_ms);
        assert_eq!(loaded.prefetch_back, original.prefetch_back);
    }

    #[test]
    fn settings_missing_fields_use_defaults() {
        let loaded: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.grid_cols, 4);
        assert_eq!(loaded.thumb_px, 512);
        assert_eq!(loaded.thumb_quality, 75);
        assert_eq!(loaded.video_volume, VIDEO_VOLUME_DEFAULT);
        assert_eq!(loaded.video_deinterlace, VideoDeinterlaceMode::Auto);
        assert!(loaded.favorites.is_empty());
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
    fn sanitize_clamps_video_volume_to_manual_boost_range() {
        let mut s = Settings::default();
        s.video_volume = 2.0;
        s.sanitize();
        assert_eq!(s.video_volume, VIDEO_VOLUME_MAX);

        s.video_volume = -0.5;
        s.sanitize();
        assert_eq!(s.video_volume, 0.0);
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
    }

    #[test]
    fn vst3_chain_slots_roundtrip() {
        let mut settings = Settings::default();
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
}
