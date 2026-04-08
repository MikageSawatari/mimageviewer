use std::path::PathBuf;

const MAX_FAVORITES: usize = 20;

// -----------------------------------------------------------------------
// サムネイルアスペクト比
// -----------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Default)]
pub enum ThumbAspect {
    Landscape16x9,
    Landscape4x3,
    #[default]
    Square,
    Portrait3x4,
    Portrait9x16,
}

impl ThumbAspect {
    /// セル幅に対するセル高さの比率
    pub fn height_ratio(self) -> f32 {
        match self {
            Self::Landscape16x9 =>  9.0 / 16.0,
            Self::Landscape4x3  =>  3.0 /  4.0,
            Self::Square        =>  1.0,
            Self::Portrait3x4   =>  4.0 /  3.0,
            Self::Portrait9x16  => 16.0 /  9.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Landscape16x9 => "16:9",
            Self::Landscape4x3  =>  "4:3",
            Self::Square        =>  "1:1",
            Self::Portrait3x4   =>  "3:4",
            Self::Portrait9x16  => "9:16",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Landscape16x9,
            Self::Landscape4x3,
            Self::Square,
            Self::Portrait3x4,
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
    pub favorites: Vec<PathBuf>,
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
}

fn default_grid_cols() -> usize { 4 }
fn default_prefetch_back() -> usize { 4 }
fn default_prefetch_forward() -> usize { 12 }
fn default_folder_skip_limit() -> usize { 3 }
fn default_thumb_px() -> u32 { 512 }

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

    /// 現在のフォルダをお気に入りに追加する（重複・上限チェック付き）。
    /// 追加された場合 true を返す。
    pub fn add_favorite(&mut self, path: PathBuf) -> bool {
        if self.favorites.contains(&path) {
            return false;
        }
        if self.favorites.len() >= MAX_FAVORITES {
            return false;
        }
        self.favorites.push(path);
        true
    }
}
