//! 絵文字スタンプ (画像ステッカー) の同梱カタログとデコード (Inc 4c)。
//!
//! 絵文字は **画像**として扱う (フォントのカラー字形ではない、`docs/stamp-feature-design.md`)。
//! Twemoji の SVG を `build.rs` が `vendor/twemoji/svg/*.svg` から `include_bytes!` で exe に
//! 同梱し (生成コードは `$OUT_DIR/emoji_svgs.rs`)、表示時に resvg (純 Rust) で 512px へ
//! ラスタライズして `comic_core::RgbaOverlay` (straight-alpha) を得る。ベイカ
//! (`comic_core::bake_overlay_with_stamps`) 自体はデコード非依存のまま。
//!
//! ライセンス: Twemoji グラフィックスは CC-BY 4.0 (Twitter, Inc. and other contributors)。
//! 帰属表示はソフトウェア情報 / installer/readme.txt に記載する。
//!
//! ラボ版 (`tools/comic_lab/src/stamp.rs`) との違いは、アセットをディスクから読むのではなく
//! exe へ同梱した SVG バイト列から解決する点と、最近使用の保存先を `data_dir` に置く点のみ。

use std::path::{Path, PathBuf};

use base64::Engine as _;
use comic_core::{RgbaOverlay, StampSource};

// build.rs が生成する `pub static EMOJI_SVGS: &[(&str, &[u8])]` (key → SVG バイト列)。
// アセット未配置なら空配列になる。
include!(concat!(env!("OUT_DIR"), "/emoji_svgs.rs"));

/// 絵文字 SVG をラスタライズするネイティブ解像度。ベイカが canvas サイズへ
/// バイリニア縮小するので、ここでは鮮明さ / メモリの上限を決めるだけ。
pub const EMOJI_RENDER_PX: u32 = 512;

/// ユーザー画像スタンプをキャッシュに保持するネイティブ解像度の長辺上限 (px)。
/// 巨大写真 (8000px 級) を素のままキャッシュすると 1 枚で数百 MB になり、複数枚で
/// セッションメモリを圧迫する (Codex P3)。長辺がこれを超える場合は面積平均縮小して
/// から保持する (canvas へはバイリニア拡縮するので実用上の画質劣化は軽微)。
pub const FILE_STAMP_MAX_PX: usize = 2048;

/// ユーザー画像スタンプを注釈データへ **埋め込む** ときの長辺上限 (px)。フォルダ移動 /
/// 別 PC / 元ファイル削除でも欠落しないよう、選択時にこのサイズへ面積平均縮小して PNG +
/// base64 で `StampSource::Embedded` に格納する。大きすぎると comic.db が肥大化するので
/// 画質とサイズのバランスでこの値にする (canvas へはバイリニア拡縮)。
pub const FILE_STAMP_EMBED_PX: usize = 1024;

/// ピッカーのカテゴリ。`all()` の順 = タブ順。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmojiCategory {
    Smileys,
    Gestures,
    Hearts,
    Animals,
    Food,
    Activities,
    Symbols,
}

impl EmojiCategory {
    pub fn all() -> &'static [EmojiCategory] {
        use EmojiCategory::*;
        &[
            Smileys, Gestures, Hearts, Animals, Food, Activities, Symbols,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            EmojiCategory::Smileys => "顔",
            EmojiCategory::Gestures => "手",
            EmojiCategory::Hearts => "ハート",
            EmojiCategory::Animals => "動物",
            EmojiCategory::Food => "食べ物",
            EmojiCategory::Activities => "活動",
            EmojiCategory::Symbols => "記号",
        }
    }
}

/// カタログ 1 件: Twemoji ファイル名 (小文字 16 進コードポイントを `-` 連結)、
/// 検索用の名前 (英語 + 少しの日本語)、カテゴリ。
pub struct EmojiEntry {
    pub key: &'static str,
    pub name: &'static str,
    pub category: EmojiCategory,
}

/// 厳選した汎用絵文字。数百件に絞ることで、3,500 件の無分類ファイルではなく
/// 「カテゴリ + 名前 + 小さな同梱サイズ」のピッカーになる。`setup-twemoji.sh` は
/// このキー集合をちょうど取得する。
#[rustfmt::skip]
pub const EMOJI_CATALOG: &[EmojiEntry] = &[
    // ---- Smileys / faces ----
    EmojiEntry { key: "1f600", name: "grinning face にっこり",        category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f603", name: "smiling face 笑顔",            category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f604", name: "smile big 笑",                 category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f606", name: "laughing 大笑い",              category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f609", name: "wink ウインク",                category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f60a", name: "blush 照れ",                   category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f60d", name: "heart eyes ハート目",          category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f618", name: "blow kiss 投げキッス",         category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f61c", name: "tongue wink てへぺろ",         category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f60e", name: "cool sunglasses サングラス",   category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f914", name: "thinking 考え中",              category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f644", name: "eye roll 呆れ",                category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f62d", name: "loudly crying 号泣",           category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f622", name: "crying 涙",                    category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f621", name: "pouting angry 怒り",           category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f620", name: "angry むっ",                   category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f633", name: "flushed 赤面",                 category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f631", name: "scream 悲鳴",                  category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f971", name: "yawn あくび",                  category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f634", name: "sleeping 睡眠",                category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f60c", name: "relieved ほっ",               category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f970", name: "smiling hearts 大好き",        category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f973", name: "party face お祝い",            category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f92f", name: "mind blown 衝撃",              category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f92b", name: "shush 内緒",                   category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f637", name: "mask マスク",                  category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f602", name: "tears of joy 爆笑",            category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f923", name: "rofl 転げ笑い",               category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f615", name: "confused 困惑",                category: EmojiCategory::Smileys },
    EmojiEntry { key: "1f97a", name: "pleading うるうる",            category: EmojiCategory::Smileys },
    // ---- Gestures / hands / people ----
    EmojiEntry { key: "1f44d", name: "thumbs up いいね",             category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f44e", name: "thumbs down だめ",             category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f44f", name: "clap 拍手",                    category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f64f", name: "pray お願い 感謝",             category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f44b", name: "wave 手を振る",                category: EmojiCategory::Gestures },
    EmojiEntry { key: "270c", name: "victory peace ピース",          category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f44c", name: "ok ok手",                      category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f91d", name: "handshake 握手",               category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f4aa", name: "muscle 力こぶ",                category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f64c", name: "raising hands 万歳",           category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f91f", name: "love you gesture",             category: EmojiCategory::Gestures },
    EmojiEntry { key: "270b", name: "raised hand 手のひら",          category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f449", name: "point right 右指差し",         category: EmojiCategory::Gestures },
    EmojiEntry { key: "1f448", name: "point left 左指差し",          category: EmojiCategory::Gestures },
    // ---- Hearts / love ----
    EmojiEntry { key: "2764", name: "red heart 赤ハート",            category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f9e1", name: "orange heart オレンジ",        category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f49b", name: "yellow heart 黄ハート",        category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f49a", name: "green heart 緑ハート",         category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f499", name: "blue heart 青ハート",          category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f49c", name: "purple heart 紫ハート",        category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f5a4", name: "black heart 黒ハート",         category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f90d", name: "white heart 白ハート",         category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f495", name: "two hearts 二つのハート",      category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f496", name: "sparkling heart きらハート",   category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f493", name: "beating heart 鼓動",           category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f494", name: "broken heart 失恋",            category: EmojiCategory::Hearts },
    EmojiEntry { key: "1f48b", name: "kiss mark キスマーク",         category: EmojiCategory::Hearts },
    // ---- Animals ----
    EmojiEntry { key: "1f436", name: "dog 犬",                       category: EmojiCategory::Animals },
    EmojiEntry { key: "1f431", name: "cat 猫",                       category: EmojiCategory::Animals },
    EmojiEntry { key: "1f42d", name: "mouse ねずみ",                 category: EmojiCategory::Animals },
    EmojiEntry { key: "1f439", name: "hamster ハムスター",           category: EmojiCategory::Animals },
    EmojiEntry { key: "1f430", name: "rabbit うさぎ",                category: EmojiCategory::Animals },
    EmojiEntry { key: "1f98a", name: "fox きつね",                   category: EmojiCategory::Animals },
    EmojiEntry { key: "1f43b", name: "bear くま",                    category: EmojiCategory::Animals },
    EmojiEntry { key: "1f43c", name: "panda パンダ",                 category: EmojiCategory::Animals },
    EmojiEntry { key: "1f981", name: "lion ライオン",                category: EmojiCategory::Animals },
    EmojiEntry { key: "1f437", name: "pig ぶた",                     category: EmojiCategory::Animals },
    EmojiEntry { key: "1f438", name: "frog かえる",                  category: EmojiCategory::Animals },
    EmojiEntry { key: "1f427", name: "penguin ペンギン",             category: EmojiCategory::Animals },
    EmojiEntry { key: "1f424", name: "chick ひよこ",                 category: EmojiCategory::Animals },
    EmojiEntry { key: "1f989", name: "owl ふくろう",                 category: EmojiCategory::Animals },
    EmojiEntry { key: "1f422", name: "turtle かめ",                  category: EmojiCategory::Animals },
    EmojiEntry { key: "1f419", name: "octopus たこ",                 category: EmojiCategory::Animals },
    EmojiEntry { key: "1f41f", name: "fish さかな",                  category: EmojiCategory::Animals },
    EmojiEntry { key: "1f98b", name: "butterfly ちょう",             category: EmojiCategory::Animals },
    // ---- Food ----
    EmojiEntry { key: "1f354", name: "hamburger ハンバーガー",       category: EmojiCategory::Food },
    EmojiEntry { key: "1f355", name: "pizza ピザ",                   category: EmojiCategory::Food },
    EmojiEntry { key: "1f363", name: "sushi 寿司",                   category: EmojiCategory::Food },
    EmojiEntry { key: "1f371", name: "bento 弁当",                   category: EmojiCategory::Food },
    EmojiEntry { key: "1f35c", name: "ramen ラーメン",               category: EmojiCategory::Food },
    EmojiEntry { key: "1f366", name: "soft cream ソフトクリーム",    category: EmojiCategory::Food },
    EmojiEntry { key: "1f370", name: "cake ケーキ",                  category: EmojiCategory::Food },
    EmojiEntry { key: "1f369", name: "donut ドーナツ",               category: EmojiCategory::Food },
    EmojiEntry { key: "1f36a", name: "cookie クッキー",              category: EmojiCategory::Food },
    EmojiEntry { key: "1f34e", name: "apple りんご",                 category: EmojiCategory::Food },
    EmojiEntry { key: "1f353", name: "strawberry いちご",            category: EmojiCategory::Food },
    EmojiEntry { key: "1f349", name: "watermelon すいか",            category: EmojiCategory::Food },
    EmojiEntry { key: "1f375", name: "tea お茶",                     category: EmojiCategory::Food },
    EmojiEntry { key: "2615", name: "coffee コーヒー",               category: EmojiCategory::Food },
    EmojiEntry { key: "1f37a", name: "beer ビール",                  category: EmojiCategory::Food },
    EmojiEntry { key: "1f376", name: "sake 日本酒",                  category: EmojiCategory::Food },
    // ---- Activities / objects ----
    EmojiEntry { key: "2728", name: "sparkles キラキラ",             category: EmojiCategory::Activities },
    EmojiEntry { key: "1f389", name: "party popper クラッカー",      category: EmojiCategory::Activities },
    EmojiEntry { key: "1f38a", name: "confetti 紙吹雪",              category: EmojiCategory::Activities },
    EmojiEntry { key: "1f380", name: "ribbon リボン",                category: EmojiCategory::Activities },
    EmojiEntry { key: "1f381", name: "gift プレゼント",              category: EmojiCategory::Activities },
    EmojiEntry { key: "1f3b5", name: "music note 音符",              category: EmojiCategory::Activities },
    EmojiEntry { key: "1f3b6", name: "musical notes 音符たち",       category: EmojiCategory::Activities },
    EmojiEntry { key: "1f525", name: "fire 炎",                      category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4a1", name: "light bulb 電球",              category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4a3", name: "bomb 爆弾",                    category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4a4", name: "zzz 眠い",                     category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4a6", name: "sweat drops 汗",               category: EmojiCategory::Activities },
    EmojiEntry { key: "1f3c6", name: "trophy トロフィー",            category: EmojiCategory::Activities },
    EmojiEntry { key: "1f947", name: "gold medal 金メダル",          category: EmojiCategory::Activities },
    EmojiEntry { key: "26bd", name: "soccer サッカー",               category: EmojiCategory::Activities },
    EmojiEntry { key: "1f3ae", name: "game controller ゲーム",       category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4f7", name: "camera カメラ",                category: EmojiCategory::Activities },
    EmojiEntry { key: "1f4b0", name: "money bag お金",               category: EmojiCategory::Activities },
    // ---- Symbols ----
    EmojiEntry { key: "2b50", name: "star 星",                       category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f31f", name: "glowing star 輝く星",          category: EmojiCategory::Symbols },
    EmojiEntry { key: "2757", name: "exclamation びっくり",          category: EmojiCategory::Symbols },
    EmojiEntry { key: "2753", name: "question はてな",               category: EmojiCategory::Symbols },
    EmojiEntry { key: "203c", name: "double exclamation",            category: EmojiCategory::Symbols },
    EmojiEntry { key: "2049", name: "interrobang !?",                category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f4af", name: "hundred points 百点",          category: EmojiCategory::Symbols },
    EmojiEntry { key: "2705", name: "check mark チェック",           category: EmojiCategory::Symbols },
    EmojiEntry { key: "274c", name: "cross mark バツ",               category: EmojiCategory::Symbols },
    EmojiEntry { key: "2b55", name: "circle まる",                   category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f6ab", name: "no entry 禁止",                category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f4a2", name: "anger mark 怒りマーク",        category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f4ac", name: "speech balloon 吹き出し",      category: EmojiCategory::Symbols },
    EmojiEntry { key: "1f4a5", name: "collision どーん",             category: EmojiCategory::Symbols },
    EmojiEntry { key: "27a1", name: "arrow right 右矢印",            category: EmojiCategory::Symbols },
    EmojiEntry { key: "2b06", name: "arrow up 上矢印",               category: EmojiCategory::Symbols },
];

/// 同梱絵文字 SVG のバイト列を返す (未同梱なら `None`)。
pub fn emoji_svg_bytes(key: &str) -> Option<&'static [u8]> {
    EMOJI_SVGS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, bytes)| *bytes)
}

/// スタンプソースの安定キャッシュキー (デコードキャッシュ / サムネイルキャッシュ用)。
pub fn stamp_source_key(source: &StampSource) -> String {
    match source {
        StampSource::Emoji(key) => format!("e:{key}"),
        StampSource::File(path) => format!("f:{}", path.display()),
        StampSource::Embedded { data, .. } => format!("b:{}", embedded_data_key(data)),
    }
}

/// 埋め込みデータ (base64 PNG) の安定キャッシュキー。data 全体は長いので決定的ハッシュを使う
/// (DefaultHasher::new() は固定キーなので実行間で安定)。長さも混ぜて衝突を減らす。
fn embedded_data_key(data: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    format!("{:016x}-{}", h.finish(), data.len())
}

/// スタンプソースの短い表示名 (オブジェクト一覧 / プロパティ)。
pub fn stamp_label(source: &StampSource) -> String {
    match source {
        StampSource::Emoji(key) => EMOJI_CATALOG
            .iter()
            .find(|e| e.key == key && !e.name.is_empty())
            .map(|e| {
                e.name
                    .split_whitespace()
                    .last()
                    .unwrap_or(e.name)
                    .to_string()
            })
            .unwrap_or_else(|| format!("emoji {key}")),
        StampSource::File(path) => path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .to_string(),
        StampSource::Embedded { name, .. } => {
            if name.is_empty() {
                "画像".to_string()
            } else {
                name.clone()
            }
        }
    }
}

/// 同梱絵文字アセットが利用可能か (= build.rs が SVG を 1 つ以上埋め込んだか)。
pub fn emoji_assets_available() -> bool {
    !EMOJI_SVGS.is_empty()
}

/// スタンプソースを straight-alpha RGBA オーバーレイへデコードする。アセット / ファイルが
/// 無い・読めないときは `None` (ベイカがプレースホルダを描く)。
pub fn load_stamp_image(source: &StampSource) -> Option<RgbaOverlay> {
    match source {
        StampSource::File(path) => {
            let bytes = std::fs::read(path).ok()?;
            let img = decode_raster(&bytes)?;
            // 巨大画像はネイティブ長辺を上限で抑えてキャッシュメモリを bound する。
            if img.w.max(img.h) > FILE_STAMP_MAX_PX {
                Some(downscale_overlay(&img, FILE_STAMP_MAX_PX))
            } else {
                Some(img)
            }
        }
        StampSource::Embedded { data, .. } => {
            // 注釈に埋め込んだ base64 PNG をデコードする (fs アクセスなし = 持ち運び可)。
            let png = base64::engine::general_purpose::STANDARD
                .decode(data)
                .ok()?;
            decode_raster(&png)
        }
        StampSource::Emoji(key) => {
            let bytes = emoji_svg_bytes(key)?;
            render_svg(bytes, EMOJI_RENDER_PX)
        }
    }
}

/// ユーザー画像ファイルを **埋め込みスタンプ** へ変換する。読み込み→デコード→長辺
/// `FILE_STAMP_EMBED_PX` へ面積平均縮小→PNG エンコード→base64 で `StampSource::Embedded`
/// を返す。これで注釈データが自己完結し、フォルダ移動 / 別 PC / 元ファイル削除でも
/// スタンプが欠落しない (Codex 監査 P1)。読めない / デコード不可なら `None`。
pub fn embed_file_stamp(path: &Path) -> Option<StampSource> {
    let bytes = std::fs::read(path).ok()?;
    let img = decode_raster(&bytes)?;
    let scaled = if img.w.max(img.h) > FILE_STAMP_EMBED_PX {
        downscale_overlay(&img, FILE_STAMP_EMBED_PX)
    } else {
        img
    };
    let png = encode_overlay_png(&scaled)?;
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("画像")
        .to_string();
    Some(StampSource::Embedded { name, data })
}

/// straight-alpha オーバーレイを PNG バイト列へエンコードする (埋め込み用)。
fn encode_overlay_png(img: &RgbaOverlay) -> Option<Vec<u8>> {
    let rgba = image::RgbaImage::from_raw(img.w as u32, img.h as u32, img.pixels.clone())?;
    let mut out = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// ラスタ画像 (PNG/JPG/WebP/GIF/BMP) を straight-alpha オーバーレイへデコードする。
fn decode_raster(bytes: &[u8]) -> Option<RgbaOverlay> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    Some(RgbaOverlay {
        w,
        h,
        pixels: img.into_raw(),
    })
}

/// SVG を straight-alpha オーバーレイへラスタライズする (長辺 `target_px`、アスペクト保持)。
fn render_svg(bytes: &[u8], target_px: u32) -> Option<RgbaOverlay> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    if sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    let scale = target_px as f32 / sw.max(sh);
    let w = (sw * scale).round().max(1.0) as u32;
    let h = (sh * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia のピクセルは premultiplied なので straight alpha へ戻す。
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for (i, px) in pixmap.pixels().iter().enumerate() {
        let c = px.demultiply();
        let o = i * 4;
        pixels[o] = c.red();
        pixels[o + 1] = c.green();
        pixels[o + 2] = c.blue();
        pixels[o + 3] = c.alpha();
    }
    Some(RgbaOverlay {
        w: w as usize,
        h: h as usize,
        pixels,
    })
}

/// straight-alpha オーバーレイの面積平均ダウンスケール (ピッカーサムネイル用、長辺
/// `target_long` px)。premultiplied 平均で透明縁の暗いフリンジを避ける。拡大はしない。
pub fn downscale_overlay(src: &RgbaOverlay, target_long: usize) -> RgbaOverlay {
    if src.w == 0 || src.h == 0 {
        return RgbaOverlay::new(1, 1);
    }
    let long = src.w.max(src.h);
    if long <= target_long {
        return src.clone();
    }
    let scale = target_long as f32 / long as f32;
    let tw = ((src.w as f32 * scale).round() as usize).max(1);
    let th = ((src.h as f32 * scale).round() as usize).max(1);
    let mut pixels = vec![0u8; tw * th * 4];
    for ty in 0..th {
        let sy0 = ty * src.h / th;
        let sy1 = (((ty + 1) * src.h / th).max(sy0 + 1)).min(src.h);
        for tx in 0..tw {
            let sx0 = tx * src.w / tw;
            let sx1 = (((tx + 1) * src.w / tw).max(sx0 + 1)).min(src.w);
            let (mut pr, mut pg, mut pb, mut sa, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = (sy * src.w + sx) * 4;
                    let a = src.pixels[i + 3] as f32 / 255.0;
                    pr += src.pixels[i] as f32 * a;
                    pg += src.pixels[i + 1] as f32 * a;
                    pb += src.pixels[i + 2] as f32 * a;
                    sa += a;
                    n += 1.0;
                }
            }
            let o = (ty * tw + tx) * 4;
            if sa > 0.0 {
                pixels[o] = (pr / sa).round().clamp(0.0, 255.0) as u8;
                pixels[o + 1] = (pg / sa).round().clamp(0.0, 255.0) as u8;
                pixels[o + 2] = (pb / sa).round().clamp(0.0, 255.0) as u8;
            }
            pixels[o + 3] = if n > 0.0 {
                (sa / n * 255.0).round().clamp(0.0, 255.0) as u8
            } else {
                0
            };
        }
    }
    RgbaOverlay {
        w: tw,
        h: th,
        pixels,
    }
}

// ---- 最近使ったスタンプ (MRU、永続化) -------------------------------------

const RECENT_STAMP_CAP: usize = 24;

fn recent_stamps_path() -> PathBuf {
    crate::data_dir::get().join("comic_recent_stamps.json")
}

pub fn load_recent_stamps() -> Vec<StampSource> {
    let path = recent_stamps_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_recent_stamps(recent: &[StampSource]) {
    let path = recent_stamps_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(recent) {
        let _ = std::fs::write(&path, text);
    }
}

/// `source` を MRU リストの先頭へ (重複除去)、長さを上限で切り詰める。
/// 埋め込みスタンプ (base64 PNG) は 1 件で数百 KB〜MB になり MRU json を肥大化させるので
/// 積まない (持ち運びは注釈データ側の埋め込みで担保済み。MRU は絵文字主体に保つ)。
pub fn push_recent_stamp(recent: &mut Vec<StampSource>, source: &StampSource) {
    if matches!(source, StampSource::Embedded { .. }) {
        return;
    }
    recent.retain(|s| s != source);
    recent.insert(0, source.clone());
    recent.truncate(RECENT_STAMP_CAP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_unique_and_named() {
        let mut seen = std::collections::HashSet::new();
        for e in EMOJI_CATALOG {
            assert!(!e.name.is_empty(), "catalog entry {} has no name", e.key);
            assert!(seen.insert(e.key), "duplicate catalog key {}", e.key);
        }
    }

    #[test]
    fn emoji_svg_decodes_when_assets_present() {
        // 同梱 SVG があれば resvg でデコードできること (アセット未配置なら gracefully skip)。
        if !emoji_assets_available() {
            eprintln!("skip: emoji assets not bundled (run scripts/setup-twemoji.sh)");
            return;
        }
        let key = EMOJI_CATALOG[0].key;
        let Some(bytes) = emoji_svg_bytes(key) else {
            eprintln!("skip: catalog[0] key {key} not bundled");
            return;
        };
        let img = render_svg(bytes, EMOJI_RENDER_PX).expect("emoji should decode");
        assert!(img.w > 0 && img.h > 0, "decoded image has size");
        assert!(
            img.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "decoded emoji has opaque pixels"
        );
    }

    #[test]
    fn downscale_shrinks_to_target() {
        let src = RgbaOverlay {
            w: 100,
            h: 50,
            pixels: vec![255u8; 100 * 50 * 4],
        };
        let t = downscale_overlay(&src, 20);
        assert_eq!(t.w.max(t.h), 20, "long edge should be the target");
        assert_eq!(t.h, 10, "aspect preserved");
    }

    #[test]
    fn stamp_keys_and_labels() {
        let e = StampSource::Emoji("1f600".into());
        assert_eq!(stamp_source_key(&e), "e:1f600");
        assert_eq!(stamp_label(&e), "にっこり");
        let f = StampSource::File(PathBuf::from("/x/y/cat.png"));
        assert_eq!(stamp_source_key(&f), "f:/x/y/cat.png");
        assert_eq!(stamp_label(&f), "cat.png");
        let b = StampSource::Embedded {
            name: "dog.png".into(),
            data: "AAAA".into(),
        };
        assert_eq!(stamp_label(&b), "dog.png");
        assert!(stamp_source_key(&b).starts_with("b:"));
    }

    #[test]
    fn embedded_stamp_png_roundtrip() {
        // 32x16 のオーバーレイを PNG+base64 へ畳んで Embedded にし、load で復元できること。
        let mut pixels = vec![0u8; 32 * 16 * 4];
        for (i, p) in pixels.chunks_exact_mut(4).enumerate() {
            p[0] = (i % 256) as u8;
            p[1] = 10;
            p[2] = 20;
            p[3] = 255;
        }
        let src = RgbaOverlay {
            w: 32,
            h: 16,
            pixels,
        };
        let png = encode_overlay_png(&src).expect("encode png");
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let embedded = StampSource::Embedded {
            name: "t.png".into(),
            data,
        };
        let loaded = load_stamp_image(&embedded).expect("load embedded");
        assert_eq!((loaded.w, loaded.h), (32, 16), "size preserved");
        // PNG はロスレスなので画素も一致する。
        assert_eq!(loaded.pixels, src.pixels, "pixels preserved (lossless PNG)");
    }

    #[test]
    fn embedded_not_pushed_to_recent() {
        let mut recent = Vec::new();
        push_recent_stamp(
            &mut recent,
            &StampSource::Embedded {
                name: "x".into(),
                data: "AAAA".into(),
            },
        );
        assert!(recent.is_empty(), "embedded stamps stay out of the MRU");
        push_recent_stamp(&mut recent, &StampSource::Emoji("1f600".into()));
        assert_eq!(recent.len(), 1, "emoji still goes to MRU");
    }
}
