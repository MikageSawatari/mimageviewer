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

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Default)]
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Default)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Default)]
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

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
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
// Settings
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
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
}

fn default_grid_cols() -> usize { 4 }
fn default_prefetch_back() -> usize { 4 }
fn default_prefetch_forward() -> usize { 12 }
fn default_folder_skip_limit() -> usize { 3 }
fn default_thumb_px() -> u32 { 512 }
fn default_thumb_quality() -> u8 { 75 }
fn default_cache_threshold_ms() -> u32 { 25 }
fn default_cache_size_threshold_bytes() -> u64 { 2_000_000 }
fn default_true() -> bool { true }
fn default_thumb_prev_pages() -> u32 { 2 }
fn default_thumb_next_pages() -> u32 { 4 }
fn default_thumb_vram_cap_percent() -> u32 { 50 }
fn default_toolbar_cols_items() -> Vec<usize> { (2..=10).collect() }
fn default_toolbar_aspect_items() -> Vec<ThumbAspect> { ThumbAspect::all().to_vec() }
fn default_toolbar_sort_items() -> Vec<SortOrder> { SortOrder::all().to_vec() }

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
            thumb_prev_pages: default_thumb_prev_pages(),
            thumb_next_pages: default_thumb_next_pages(),
            thumb_vram_cap_percent: default_thumb_vram_cap_percent(),
            thumb_idle_upgrade: true,
            show_toolbar_favorites: true,
            show_toolbar_folder: true,
            toolbar_cols_items: default_toolbar_cols_items(),
            toolbar_aspect_items: default_toolbar_aspect_items(),
            toolbar_sort_items: default_toolbar_sort_items(),
        }
    }
}

impl Settings {
    fn settings_path() -> PathBuf {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("mimageviewer").join("settings.json")
    }

    pub fn load() -> Self {
        let path = Self::settings_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            std::fs::write(&path, json).ok();
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
}
