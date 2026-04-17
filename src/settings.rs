use std::path::PathBuf;

const MAX_FAVORITES: usize = 20;

// -----------------------------------------------------------------------
// FavoriteEntry
// -----------------------------------------------------------------------

/// お気に入りフォルダの 1 エントリ。
///
/// `name` はユーザが任意に付けられる表示名 (ツールバーのボタンラベル等で使用)。
/// 既定ではフォルダ名 (`path.file_name()`) が入る。
///
/// 旧バージョンとの互換性のため、JSON 上では「文字列のみ (旧)」「オブジェクト (新)」
/// の両方を受け付ける。旧形式から読み込んだ場合、`name` はフォルダ名で自動補完される。
#[derive(Clone, Debug)]
pub struct FavoriteEntry {
    pub name: String,
    pub path: PathBuf,
}

impl<'de> serde::Deserialize<'de> for FavoriteEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // 旧: 文字列 or パス (例: "C:\\foo")
        // 新: オブジェクト (例: {"name": "my folder", "path": "C:\\foo"})
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Legacy(PathBuf),
            Full { name: String, path: PathBuf },
        }

        match Raw::deserialize(deserializer)? {
            Raw::Legacy(p) => {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                Ok(FavoriteEntry { name, path: p })
            }
            Raw::Full { name, path } => Ok(FavoriteEntry { name, path }),
        }
    }
}

impl serde::Serialize for FavoriteEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("FavoriteEntry", 2)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("path", &self.path)?;
        s.end()
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
            Self::Landscape16x9 =>  9.0 / 16.0,
            Self::Landscape3x2  =>  2.0 /  3.0,
            Self::Landscape4x3  =>  3.0 /  4.0,
            Self::Square        =>  1.0,
            Self::Portrait3x4   =>  4.0 /  3.0,
            Self::Portrait2x3   =>  3.0 /  2.0,
            Self::Portrait9x16  => 16.0 /  9.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Landscape16x9 => "16:9",
            Self::Landscape3x2  =>  "3:2",
            Self::Landscape4x3  =>  "4:3",
            Self::Square        =>  "1:1",
            Self::Portrait3x4   =>  "3:4",
            Self::Portrait2x3   =>  "2:3",
            Self::Portrait9x16  => "9:16",
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
    FileName,   // ファイル名順（辞書順）
    Numeric,    // 番号順（自然順: 1, 2, 9, 10, 11）
    DateAsc,    // 日付順（昇順）
    DateDesc,   // 日付順（降順）
}

impl SortOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::FileName => "ファイル名順",
            Self::Numeric  => "番号順",
            Self::DateAsc  => "日付順（古い順）",
            Self::DateDesc => "日付順（新しい順）",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::FileName => "名前",
            Self::Numeric  => "番号",
            Self::DateAsc  => "日付↑",
            Self::DateDesc => "日付↓",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::FileName, Self::Numeric, Self::DateAsc, Self::DateDesc]
    }

    /// 2 つのメディア項目をこのソート順で比較する。
    /// `name_a`/`name_b` はファイル名（拡張子付き）、`mtime_a`/`mtime_b` は更新日時。
    /// `natural_key` は番号順ソート用のキー生成関数。
    pub fn compare<K: Ord>(
        self,
        name_a: &str, mtime_a: i64,
        name_b: &str, mtime_b: i64,
        natural_key: impl Fn(&str) -> K,
    ) -> std::cmp::Ordering {
        match self {
            Self::FileName => {
                name_a.to_lowercase().cmp(&name_b.to_lowercase())
            }
            Self::Numeric => {
                natural_key(name_a).cmp(&natural_key(name_b))
            }
            Self::DateAsc  => mtime_a.cmp(&mtime_b),
            Self::DateDesc => mtime_b.cmp(&mtime_a),
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
            Self::Off    => "Off（生成しない）",
            Self::Auto   => "Auto（自動判定・推奨）",
            Self::Always => "Always（常に生成）",
        }
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
    fn default() -> Self { Self::Auto }
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
            Self::Single   => 0,
            Self::Ltr      => 1,
            Self::LtrCover => 2,
            Self::Rtl      => 3,
            Self::RtlCover => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single   => "1ページ表示",
            Self::Ltr      => "見開き 左→右",
            Self::LtrCover => "見開き 左→右（表紙あり）",
            Self::Rtl      => "見開き 右→左",
            Self::RtlCover => "見開き 右→左（表紙あり）",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Single, Self::Ltr, Self::LtrCover, Self::Rtl, Self::RtlCover]
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

    // ── ツールバー表示設定 ──────────────────────────────────
    /// ツールバーに「お気に入り」セクションを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_favorites: bool,
    /// アドレスバー (フォルダ入力行) を表示する
    #[serde(default = "default_true")]
    pub show_toolbar_folder: bool,
    /// ツールバーに「上のフォルダへ」ボタンを表示する
    #[serde(default = "default_true")]
    pub show_toolbar_parent_button: bool,
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
}

/// グリッド列数の最小値
pub const MIN_GRID_COLS: usize = 1;
/// グリッド列数の最大値
pub const MAX_GRID_COLS: usize = 10;

fn default_grid_cols() -> usize { 4 }
fn default_prefetch_back() -> usize { 4 }
fn default_prefetch_forward() -> usize { 12 }
fn default_folder_skip_limit() -> usize { 5 }
fn default_thumb_px() -> u32 { 512 }
fn default_thumb_quality() -> u8 { 75 }
fn default_cache_threshold_ms() -> u32 { 25 }
fn default_cache_size_threshold_bytes() -> u64 { 2_000_000 }
fn default_true() -> bool { true }
fn default_thumb_prev_pages() -> u32 { 2 }
fn default_thumb_next_pages() -> u32 { 4 }
fn default_thumb_vram_cap_percent() -> u32 { 50 }
fn default_folder_thumb_sort() -> SortOrder { SortOrder::Numeric }
fn default_folder_thumb_depth() -> u32 { 3 }
fn default_ai_upscale_prefetch_back() -> usize { 1 }
fn default_ai_upscale_prefetch_forward() -> usize { 2 }
fn default_ai_upscale_skip_px() -> u32 { 2048 }
fn default_ai_denoise_skip_px() -> u32 { 2048 }
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
        "png", "bmp", "gif", "tiff", "tif",       // ロスレス
        "webp", "jxl", "avif", "heic", "heif",     // モダン (ロッシー/ロスレス混在)
        "jpg", "jpeg",                              // ロッシー
        "dng", "cr2", "cr3", "nef", "nrw", "arw",  // RAW (現像困難な場合が多い)
        "srf", "sr2", "raf", "orf", "rw2", "pef",
        "ptx", "rwl", "iiq",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_slideshow_interval() -> f32 { 3.0 }
fn default_toolbar_cols_items() -> Vec<usize> { (MIN_GRID_COLS..=MAX_GRID_COLS).collect() }
fn default_toolbar_aspect_items() -> Vec<ThumbAspect> { ThumbAspect::all().to_vec() }
fn default_toolbar_sort_items() -> Vec<SortOrder> { SortOrder::all().to_vec() }
fn default_rating_filter() -> [bool; 6] { [true; 6] }

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
            show_toolbar_favorites: true,
            show_toolbar_folder: true,
            show_toolbar_parent_button: true,
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
            global_preset: crate::adjustment::AdjustParams::default(),
            preset_slots: crate::adjustment::PresetSlots::default(),
            sidecar_backup_enabled: true,
        }
    }
}

impl Settings {
    fn settings_path() -> PathBuf {
        crate::data_dir::get().join("settings.json")
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
        settings.sanitize();
        settings
    }

    /// 読み込んだ設定値を安全範囲に補正する (JSON 手編集で範囲外の値が入った場合の防衛)。
    fn sanitize(&mut self) {
        // folder_skip_limit == 0 だと navigate_folder_with_skip が first を評価せず
        // フルスクリーン Ctrl+↑↓ が事実上機能しないため、最低 1 にクランプする。
        // 環境設定 UI 側のレンジ (1..=10) と整合させる。
        if self.folder_skip_limit == 0 {
            self.folder_skip_limit = 1;
        }
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
    /// 追加された場合 true を返す。
    pub fn add_favorite(&mut self, name: String, path: PathBuf) -> bool {
        if self.is_favorite(&path) {
            return false;
        }
        if self.favorites.len() >= MAX_FAVORITES {
            return false;
        }
        self.favorites.push(FavoriteEntry { name, path });
        true
    }

    /// 「アプリケーションで開く」で使用したアプリを履歴に記録する。
    /// 同じ exe_path が既にあれば先頭に移動。最大3件。
    pub fn record_recent_open_with(&mut self, display_name: String, exe_path: String) {
        const MAX_RECENT_OPEN_WITH: usize = 3;
        self.recent_open_with_apps
            .retain(|a| !a.exe_path.eq_ignore_ascii_case(&exe_path));
        self.recent_open_with_apps
            .insert(0, RecentApp { display_name, exe_path });
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
        assert!(loaded.favorites.is_empty());
    }

    /// JSON 手編集等で `folder_skip_limit: 0` が入っていたら、sanitize で 1 に
    /// 補正される。0 のままだと navigate_folder_with_skip が first を評価せず
    /// フルスクリーン Ctrl+↑↓ が事実上機能しなくなるための防衛。
    #[test]
    fn sanitize_clamps_folder_skip_limit_to_one() {
        let mut s = Settings::default();
        s.folder_skip_limit = 0;
        s.sanitize();
        assert_eq!(s.folder_skip_limit, 1);

        // >= 1 の値は据え置き
        let mut s = Settings::default();
        s.folder_skip_limit = 5;
        s.sanitize();
        assert_eq!(s.folder_skip_limit, 5);
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
        let entry = FavoriteEntry {
            name: "Test".to_string(),
            path: PathBuf::from(r"C:\test"),
        };
        let json = serde_json::to_string(&entry).unwrap();
        // オブジェクト形式で出力されることを確認
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"path\""));
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

        let manual: Parallelism =
            serde_json::from_str(r#"{"mode":"Manual","value":4}"#).unwrap();
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
