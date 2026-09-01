use super::PreferencesPage;

pub(super) struct PrefSearchEntry {
    /// ページ内で一意な anchor id。
    pub anchor: &'static str,
    pub page: PreferencesPage,
    /// 画面に表示されるラベルと同じ文字列。
    pub title: &'static str,
    /// 画面に出ない表記揺れ・同義語。
    pub keywords: &'static [&'static str],
}

macro_rules! entry {
    ($anchor:literal, $page:ident, $title:literal, [$($keyword:literal),* $(,)?]) => {
        PrefSearchEntry {
            anchor: $anchor,
            page: PreferencesPage::$page,
            title: $title,
            keywords: &[$($keyword),*],
        }
    };
}

pub(super) const PREF_SEARCH_INDEX: &[PrefSearchEntry] = &[
    entry!(
        "general/theme",
        General,
        "テーマ",
        ["theme", "外観", "ライト", "ダーク"]
    ),
    entry!(
        "general/text-contrast",
        General,
        "文字のコントラスト",
        ["文字色", "見やすさ", "contrast"]
    ),
    entry!(
        "general/ai-processing",
        General,
        "表示時の AI 処理 (アップスケール / ノイズ除去)",
        [
            "人工知能",
            "高画質",
            "軽量",
            "upscale",
            "denoise",
            "デノイズ"
        ]
    ),
    entry!(
        "general/viewer-mode",
        General,
        "ビューワモード",
        [
            "viewer",
            "ビューア",
            "別ウィンドウ",
            "複数ウィンドウ",
            "F12"
        ]
    ),
    entry!(
        "general/book-display",
        General,
        "本の表示モード",
        ["ZIP", "PDF", "ページ一覧", "本"]
    ),
    entry!(
        "general/image-folder-book",
        General,
        "画像のみのフォルダは、PDF/ZIP のように本として扱う",
        ["画像フォルダ", "本扱い"]
    ),
    entry!(
        "general/media-window",
        General,
        "動画・音声は別ウィンドウで再生",
        [
            "メディアウィンドウ",
            "video",
            "audio",
            "ビューワ",
            "ビューア"
        ]
    ),
    entry!(
        "font/ui-font",
        Font,
        "UI フォント",
        ["font", "書体", "文字", "追加フォント"]
    ),
    entry!(
        "font/vertical-adjust",
        Font,
        "縦位置の微調整:",
        ["上下", "ずれ", "位置", "pt"]
    ),
    entry!(
        "startup/mode",
        Startup,
        "起動時に開く場所",
        [
            "起動フォルダ",
            "startup",
            "前回",
            "デスクトップ",
            "閲覧履歴"
        ]
    ),
    entry!(
        "startup/restore-cursor",
        Startup,
        "前回のカーソル位置を復元する",
        ["カーソル", "選択", "位置", "cursor", "復元"]
    ),
    entry!(
        "startup/specific-folder",
        Startup,
        "指定フォルダ:",
        ["パス", "folder", "directory"]
    ),
    entry!(
        "startup/window-state",
        Startup,
        "起動時のウィンドウ状態",
        [
            "最大化",
            "ウィンドウサイズ",
            "サイズ復元",
            "位置",
            "maximize",
            "window"
        ]
    ),
    entry!(
        "explorer/context-menu",
        ExplorerIntegration,
        "実ファイル/実フォルダでは Windows 標準の右クリックメニューを使う",
        ["コンテキストメニュー", "shell", "右クリック"]
    ),
    entry!(
        "explorer/sendto",
        ExplorerIntegration,
        "SendTo",
        ["送る", "エクスプローラー", "Explorer", "登録"]
    ),
    entry!(
        "thumbnail/category-order",
        Thumbnail,
        "グリッドのカテゴリ表示順",
        ["表示順", "フォルダ", "画像", "動画", "アーカイブ"]
    ),
    entry!(
        "thumbnail/video-indicator",
        Thumbnail,
        "動画サムネイルの目印",
        ["再生アイコン", "左下バッジ", "非表示"]
    ),
    entry!(
        "thumbnail/idle-upgrade",
        Thumbnail,
        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
        ["thumbnail", "画質", "高品質", "再デコード"]
    ),
    entry!(
        "thumbnail/click-selection",
        Thumbnail,
        "選択方式:",
        [
            "クリック",
            "チェック",
            "エクスプローラー",
            "複数選択",
            "選択済み項目",
            "もう一度",
            "開く"
        ]
    ),
    entry!(
        "thumbnail/cursor-wrap",
        Thumbnail,
        "カーソル移動をループする",
        ["矢印", "先頭", "末尾", "循環"]
    ),
    entry!(
        "thumbnail/selection-info",
        Thumbnail,
        "表示方法:",
        ["選択情報", "ツールチップ", "下部情報バー"]
    ),
    entry!(
        "thumbnail/details-selection-bar",
        Thumbnail,
        "詳細表示時の下部情報バー:",
        ["詳細表示", "列設定", "専用設定"]
    ),
    entry!(
        "thumbnail/tooltip-items",
        Thumbnail,
        "ツールチップに表示する項目:",
        ["ファイル名", "解像度", "コーデック", "場所", "日時"]
    ),
    entry!(
        "slideshow/interval",
        Slideshow,
        "ページ送り間隔:",
        ["スライドショー", "秒", "interval"]
    ),
    entry!(
        "slideshow/continuous",
        Slideshow,
        "連結読みスライドショー:",
        ["待機時間", "スクロール時間", "スクロール量"]
    ),
    entry!(
        "slideshow/end-action",
        Slideshow,
        "フォルダの最後まで進んだら:",
        ["ループ", "次のフォルダ", "停止"]
    ),
    entry!(
        "capture/format",
        Capture,
        "保存形式",
        ["PNG", "JPEG", "JPG", "キャプチャ"]
    ),
    entry!(
        "capture/folder",
        Capture,
        "保存先フォルダ",
        ["保存場所", "出力先", "directory", "capture"]
    ),
    entry!(
        "lut/manage",
        CreativeLut,
        ".cube LUTを追加…",
        ["3D LUT", "ルック", "色", "カラー", "creative"]
    ),
    entry!(
        "lut/display-name",
        CreativeLut,
        "表示名:",
        ["LUT名", "登録解除", "プリセット"]
    ),
    entry!(
        "menu/layout",
        MenuLayout,
        "メニューバーの上位メニューと固定項目の表示順を変更します。登録済みお気に入り、タグ一覧、更新確認など状態で変わる項目は固定位置に残ります。",
        ["メニュー構成", "表示", "非表示", "並べ替え", "順番"]
    ),
    entry!(
        "parallelism/mode",
        Parallelism,
        "手動",
        ["自動", "スレッド", "CPU", "並列", "parallel"]
    ),
    entry!(
        "parallelism/pdf",
        Parallelism,
        "PDF の同時処理数",
        ["PDF", "同時", "プロセス", "メモリ", "次回起動"]
    ),
    entry!(
        "prefetch/full-image",
        Prefetch,
        "フルサイズ画像の先読み",
        ["前の画像", "次の画像", "prefetch"]
    ),
    entry!(
        "prefetch/thumbnail",
        Prefetch,
        "サムネイルの先読み",
        ["前のページ", "次のページ", "GPU"]
    ),
    entry!(
        "prefetch/ai",
        Prefetch,
        "AI・カラー化の先読み",
        [
            "アップスケール",
            "ノイズ除去",
            "デノイズ",
            "denoise",
            "colorize"
        ]
    ),
    entry!(
        "prefetch/retained-ai-count",
        Prefetch,
        "最大枚数:",
        ["AI 結果", "CPU メモリ", "保持", "RAM"]
    ),
    entry!(
        "prefetch/retained-ai-memory",
        Prefetch,
        "最大 CPU メモリ (RAM):",
        ["AI キャッシュ", "容量", "MB"]
    ),
    entry!(
        "prefetch/ai-size-limit",
        Prefetch,
        "AI 処理のサイズ上限",
        [
            "長辺",
            "短辺",
            "アップスケール",
            "ノイズ除去",
            "デノイズ",
            "denoise",
            "pixel"
        ]
    ),
    entry!(
        "gpu-memory/limit",
        GpuMemory,
        "上限:",
        ["GPU メモリ", "VRAM", "容量", "無制限"]
    ),
    entry!(
        "ai-backend/backend",
        AiBackend,
        "バックエンド:",
        [
            "DirectML",
            "TensorRT",
            "NVIDIA",
            "GPU",
            "AI",
            "アップスケール",
            "ノイズ除去",
            "デノイズ",
            "denoise"
        ]
    ),
    entry!(
        "ai-backend/tensorrt-pack",
        AiBackend,
        "TensorRT",
        ["パック", "ダウンロード", "削除", "engine", "CUDA"]
    ),
    entry!(
        "editing-addon/manage",
        EditingAddon,
        "編集用追加ファイル",
        [
            "追加パック",
            "フォント",
            "被写体分離",
            "ダウンロード",
            "オノマトペ"
        ]
    ),
    entry!(
        "cache/mode",
        Cache,
        "モード",
        ["Off", "Auto", "Always", "サムネイルキャッシュ"]
    ),
    entry!(
        "cache/auto-time",
        Cache,
        "時間しきい値 (decode + display の合計がこれ以上ならキャッシュ):",
        ["ミリ秒", "ms", "速度", "decode"]
    ),
    entry!(
        "cache/auto-size",
        Cache,
        "サイズしきい値 (このサイズ以上は無条件キャッシュ):",
        ["MB", "容量", "画像サイズ"]
    ),
    entry!(
        "cache/auto-kinds",
        Cache,
        "既存 .webp は常にキャッシュ (処理が重いため推奨)",
        ["WebP", "PDF", "ZIP", "常にキャッシュ"]
    ),
    entry!(
        "cache/edit-preview",
        Cache,
        "編集結果をサムネイル一覧に保持する",
        ["プレビューキャッシュ", "派生 WebP", "編集", "容量上限"]
    ),
    entry!(
        "cache/archive-handling",
        Cache,
        "RAR / 7z / LZH の処理",
        ["対応アーカイブ", "変換", "無視"]
    ),
    entry!(
        "cache/archive-limit",
        Cache,
        "容量上限を有効にする",
        ["変換済みアーカイブ", "キャッシュ", "無制限", "MB"]
    ),
    entry!(
        "folder/hidden-files",
        Folder,
        "隠しファイル・フォルダを表示する",
        ["hidden", "不可視", "システムファイル"]
    ),
    entry!(
        "folder/thumbnail-sort",
        Folder,
        "代表画像の選択基準",
        ["フォルダサムネイル", "並び順", "ソート"]
    ),
    entry!(
        "folder/thumbnail-depth",
        Folder,
        "サブフォルダ探索階層:",
        ["深さ", "代表画像", "再帰"]
    ),
    entry!(
        "folder/skip-empty",
        Folder,
        "空フォルダのスキップ上限:",
        ["Ctrl", "フォルダ移動", "画像なし"]
    ),
    entry!(
        "folder/edit-restore",
        Folder,
        "コピー・移動したファイルの編集内容を復元するか確認する",
        ["編集内容", "復元", "コピー", "移動", "照合"]
    ),
    entry!(
        "folder/delete-confirmation",
        Folder,
        "ごみ箱へ移すときは削除前の確認を省略する",
        ["削除", "確認", "削除確認", "ゴミ箱", "完全削除", "delete"]
    ),
    entry!(
        "folder/backup",
        Folder,
        "フォルダに補正・マスク設定のバックアップを保存する",
        [
            "mimageviewer.dat",
            "sidecar",
            "サイドカー",
            "設定バックアップ"
        ]
    ),
    entry!(
        "folder/tag-backup",
        Folder,
        "フォルダにタグのバックアップを保存する",
        ["tag", "タグ", "sidecar", "mimageviewer.dat"]
    ),
    entry!(
        "folder/stack-script",
        Folder,
        "分類ルールをスクリプト (カスタム) で行う",
        ["スタック", "Rhai", "正規表現", "stack_rules"]
    ),
    entry!(
        "book/root",
        Book,
        "本棚の保存先",
        ["製本", "保存先フォルダ", "books", "本"]
    ),
    entry!(
        "duplicate/archive-folder",
        DuplicateFiles,
        "同名の ZIP/PDF/RAR/7z/LZH ファイルとフォルダがある場合、アーカイブ側をスキップ",
        ["重複", "同名", "archive"]
    ),
    entry!(
        "duplicate/zip-archive",
        DuplicateFiles,
        "同名の ZIP/CBZ と RAR/7z/LZH がある場合、ZIP/CBZ だけ表示",
        ["重複", "優先", "archive"]
    ),
    entry!(
        "duplicate/video-image",
        DuplicateFiles,
        "同名の動画と画像がある場合、画像をスキップ",
        ["重複", "sidecar", "サイドカー"]
    ),
    entry!(
        "duplicate/image-priority",
        DuplicateFiles,
        "同名の画像が複数拡張子で存在する場合、優先度で選択",
        ["拡張子", "優先順位", "JPEG", "PNG", "WebP"]
    ),
    entry!(
        "exif/hidden-tags",
        ExifDisplay,
        "メタデータパネルで非表示にする EXIF タグを選択します。",
        ["Image Info", "メタデータ", "タグ", "非表示"]
    ),
    entry!(
        "exif/custom-tag",
        ExifDisplay,
        "カスタム追加:",
        ["MakerNote", "内部名", "EXIF タグ"]
    ),
    entry!(
        "spread/side-panels",
        SpreadMode,
        "左右パネルの表示",
        ["サイドパネル", "ホバー", "クリック"]
    ),
    entry!(
        "spread/boundary-notice",
        SpreadMode,
        "先頭 / 末尾の案内を表示",
        ["境界", "最初", "最後", "通知"]
    ),
    entry!(
        "spread/processing-status",
        SpreadMode,
        "処理状況を表示",
        ["読込中", "AI 処理", "消去補完", "ステータス"]
    ),
    entry!(
        "spread/prefetch-status",
        SpreadMode,
        "先読み状況を表示",
        ["AI 先読み", "進捗", "前後ページ", "ステータス"]
    ),
    entry!(
        "spread/page-layout",
        SpreadMode,
        "デフォルトのページ構成",
        ["単ページ", "見開き", "比較", "構成"]
    ),
    entry!(
        "spread/reading-flow",
        SpreadMode,
        "デフォルトの連結方式",
        ["連結読み", "縦", "横", "flow"]
    ),
    entry!(
        "spread/direction",
        SpreadMode,
        "横連結の方向",
        ["右から左", "左から右", "RTL", "LTR", "読み方向"]
    ),
    entry!(
        "spread/fit",
        SpreadMode,
        "ズーム/フィット",
        ["拡大", "縮小", "倍率", "zoom"]
    ),
    entry!(
        "spread/no-upscale",
        SpreadMode,
        "拡大しない",
        ["自動フィット", "倍率制限"]
    ),
    entry!(
        "spread/no-downscale",
        SpreadMode,
        "縮小しない",
        ["自動フィット", "倍率制限"]
    ),
    entry!(
        "spread/anime-limit",
        SpreadMode,
        "元画像範囲の長辺",
        ["アニメ塗り拡大", "サイズ上限", "upscale"]
    ),
    entry!(
        "spread/seek-bar",
        SpreadMode,
        "下部ページシークバーを固定表示",
        ["ページバー", "下部バー", "固定"]
    ),
    entry!(
        "spread/seek-direction",
        SpreadMode,
        "ページシークバーの方向",
        ["読み方向", "先頭", "末尾"]
    ),
    entry!(
        "spread/cursor-direction",
        SpreadMode,
        "カーソルキー左右の方向",
        ["矢印キー", "左右キー", "ページ移動"]
    ),
    entry!(
        "spread/top-bar",
        SpreadMode,
        "上部情報バーを固定表示",
        ["上部バー", "固定", "情報"]
    ),
    entry!(
        "spread/bar-gap",
        SpreadMode,
        "固定バーと表示内容の間隔",
        ["画像", "映像", "余白", "gap", "px"]
    ),
    entry!(
        "spread/page-number",
        SpreadMode,
        "ページ番号を常時表示",
        ["ページ数", "右下", "overlay"]
    ),
    entry!(
        "spread/keep-fullscreen",
        SpreadMode,
        "メインに戻ったらフルスクリーンへ復帰",
        ["Alt+Tab", "フォーカス", "fullscreen"]
    ),
    entry!(
        "spread/cursor-hide",
        SpreadMode,
        "マウスカーソルを隠すまで",
        ["ポインター", "非表示", "秒"]
    ),
    entry!(
        "spread/panorama-projection",
        SpreadMode,
        "投影方式",
        [
            "360",
            "パノラマ",
            "魚眼",
            "立体射影",
            "等距離",
            "等立体角",
            "リトルプラネット",
            "projection",
            "panorama",
            "fisheye"
        ]
    ),
    entry!(
        "spread/page-jump",
        SpreadMode,
        "ページジャンプ量",
        ["Shift", "割合", "固定ページ数"]
    ),
    entry!(
        "spread/spread-gap",
        SpreadMode,
        "見開きのページ間隔",
        ["ページ間", "余白", "gap"]
    ),
    entry!(
        "spread/continuous-gap",
        SpreadMode,
        "連結読みのページ間隔",
        ["ページ間", "余白", "gap"]
    ),
    entry!(
        "spread/continuous-scroll",
        SpreadMode,
        "連結読みのスクロール量 (画面サイズ基準)",
        [
            "ホイール",
            "マウスホイール",
            "矢印キー",
            "D-pad",
            "スティック"
        ]
    ),
    entry!(
        "resume/modes",
        PlaybackResume,
        "一覧から開く",
        [
            "続きから",
            "先頭から",
            "最初から",
            "動画",
            "音声",
            "ZIP",
            "PDF"
        ]
    ),
    entry!(
        "resume/video-audio",
        PlaybackResume,
        "動画・音声の再生位置",
        ["すべてクリア", "レジューム", "記憶", "履歴", "resume"]
    ),
    entry!(
        "resume/book",
        PlaybackResume,
        "本 (フォルダ / ZIP / PDF) の読書位置",
        ["すべてクリア", "続き", "記憶"]
    ),
    entry!(
        "resume/history-enabled",
        PlaybackResume,
        "手動で開いた本・動画・音声を閲覧履歴に記録する",
        ["history", "最近見た", "閲覧履歴"]
    ),
    entry!(
        "resume/history-limit",
        PlaybackResume,
        "保持件数:",
        ["履歴件数", "最大", "記憶"]
    ),
    entry!(
        "susie/enabled",
        SusiePlugins,
        "Susie 画像プラグインを有効にする",
        ["SPI", "plugin", "プラグイン"]
    ),
    entry!(
        "susie/parallel",
        SusiePlugins,
        "プラグインを並列実行する (推奨: ON)",
        ["ワーカー", "同時", "parallel"]
    ),
    entry!(
        "susie/folder-scan",
        SusiePlugins,
        "プラグインフォルダ:",
        ["再読み込み", "reload", "フォルダー", "ロード済み"]
    ),
    entry!(
        "indexer/speed",
        IndexerSpeed,
        "速度プロファイル",
        ["インデクサ", "index", "I/O", "High", "Low"]
    ),
    entry!(
        "tray/residency",
        TrayResidency,
        "アプリを閉じる代わりに、タスクトレイに常駐する",
        ["最小化", "閉じる", "常駐", "tray"]
    ),
    entry!(
        "tray/pause-indexer",
        TrayResidency,
        "常駐中はインデックス更新を一時停止する",
        ["pause", "スキャン", "ファイル監視"]
    ),
    entry!(
        "rating/xmp",
        Rating,
        "レーティングを XMP にも書き込む",
        ["評価", "星", "★", "xmp:Rating", "F1"]
    ),
    entry!(
        "update/automatic",
        UpdateCheck,
        "新バージョンを自動的に確認する",
        ["アップデート", "更新チェック", "GitHub", "version"]
    ),
    entry!(
        "update/releases",
        UpdateCheck,
        "リリース履歴を開く",
        ["変更履歴", "release", "バージョン"]
    ),
    entry!(
        "video/hardware-decode",
        Video,
        "ハードウェアデコードを有効にする",
        ["GPU", "D3D11VA", "HEVC", "4K", "hardware"]
    ),
    entry!(
        "video/deinterlace",
        Video,
        "デインターレース",
        ["インターレース", "横縞", "bwdif"]
    ),
    entry!(
        "video/bar-visibility",
        Video,
        "再生画面のバー",
        [
            "上部情報バー",
            "下部シークバー",
            "シークストリップ",
            "固定表示",
            "鍵",
            "領域を確保",
            "余白"
        ]
    ),
    entry!("video/loop", Video, "ループ再生:", ["繰り返し", "loop"]),
    entry!(
        "video/start-muted",
        Video,
        "起動直後はミュートで開始",
        ["無音", "mute", "音量"]
    ),
    entry!(
        "video/default-volume",
        Video,
        "既定音量",
        ["ボリューム", "volume", "dB"]
    ),
    entry!(
        "video/seek-thumbnail-tolerance",
        Video,
        "シーク時のズレ許容 (秒)",
        ["プレビュー", "サムネイル", "速度", "位置", "seek"]
    ),
    entry!(
        "video/seek-strip-interval",
        Video,
        "シークストリップの画像間隔",
        ["動画", "サムネイル", "見渡す", "大まか", "seek"]
    ),
    entry!(
        "video/seek-strip-waveform-span",
        Video,
        "音声波形で見渡す範囲",
        ["動画", "波形", "音声", "見渡す", "長時間", "seek"]
    ),
    entry!(
        "video/remote-streaming",
        Video,
        "リモート端末への動画配信を有効にする",
        ["リモート", "remote", "配信", "ストリーミング"]
    ),
    entry!(
        "video/normalize-cache",
        Video,
        "音量ノーマライズ測定値",
        ["LUFS", "音量均一", "測定", "クリア"]
    ),
    entry!(
        "video/sidecar-thumbnail",
        Video,
        "同名ファイル名の画像があれば動画サムネに優先採用",
        ["サイドカー", "sidecar", "動画サムネイル"]
    ),
    entry!(
        "vst3/enabled",
        Vst3,
        "VST3 プラグイン処理を有効にする",
        ["音声", "エフェクト", "EQ", "LUFS", "チェーン", "スキャン"]
    ),
    entry!(
        "developer/diagnostics",
        Developer,
        "ログを zip にする",
        ["診断情報", "サポート", "エラーログ", "export"]
    ),
    entry!(
        "developer/performance-log",
        Developer,
        "性能ログを記録する (次回起動から有効)",
        ["perf", "重い", "カクつく", "パフォーマンス"]
    ),
];

/// AND 部分一致で検索し、設計上の優先順位とツリー順で返す。
pub(super) fn search_preferences(
    query: &str,
    tree_position: impl Fn(PreferencesPage) -> (&'static str, usize, usize),
) -> Vec<&'static PrefSearchEntry> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let normalized_query = query.trim().to_ascii_lowercase();
    let mut results = Vec::new();
    for (index, entry) in PREF_SEARCH_INDEX.iter().enumerate() {
        let title = entry.title.to_ascii_lowercase();
        let keywords: Vec<String> = entry
            .keywords
            .iter()
            .map(|keyword| keyword.to_ascii_lowercase())
            .collect();
        let (category, category_index, page_index) = tree_position(entry.page);
        let page = entry.page.label().to_ascii_lowercase();
        let category = category.to_ascii_lowercase();
        let all_match = terms.iter().all(|term| {
            title.contains(term)
                || keywords.iter().any(|keyword| keyword.contains(term))
                || page.contains(term)
                || category.contains(term)
        });
        if !all_match {
            continue;
        }
        let rank = if title.starts_with(&normalized_query) {
            0
        } else if terms.iter().all(|term| title.contains(term)) {
            1
        } else if terms.iter().all(|term| {
            title.contains(term) || keywords.iter().any(|keyword| keyword.contains(term))
        }) {
            2
        } else {
            3
        };
        results.push((rank, category_index, page_index, index, entry));
    }
    results.sort_by_key(|item: &(u8, usize, usize, usize, &PrefSearchEntry)| {
        (item.0, item.1, item.2, item.3)
    });
    results
        .into_iter()
        .map(|(_, _, _, _, entry)| entry)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const PAGES_SOURCE: &str = include_str!("pages.rs");
    const PREFERENCES_SOURCE: &str = include_str!("../preferences.rs");

    fn anchors_in_pages_source(source: &str) -> Vec<&str> {
        source
            .split("anchored(")
            .skip(1)
            .filter_map(|rest| {
                let start = rest.find('"')? + 1;
                let end = rest[start..].find('"')? + start;
                Some(&rest[start..end])
            })
            .collect()
    }

    fn missing_pages(
        pages: &[PreferencesPage],
        entries: &[PrefSearchEntry],
    ) -> Vec<PreferencesPage> {
        let indexed: HashSet<_> = entries.iter().map(|entry| entry.page).collect();
        pages
            .iter()
            .copied()
            .filter(|page| !indexed.contains(page))
            .collect()
    }

    fn preference_page_variants_in_source(source: &str) -> Vec<&str> {
        let marker = "pub(crate) enum PreferencesPage {";
        let body = source
            .split_once(marker)
            .expect("PreferencesPage enum が preferences.rs にありません")
            .1;
        let body = body
            .split_once("\n}")
            .expect("PreferencesPage enum の終端がありません")
            .0;
        body.lines()
            .filter_map(|line| line.trim().strip_suffix(','))
            .filter(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
            .collect()
    }

    #[test]
    fn index_entries_are_unique_valid_and_placed_in_pages() {
        let mut anchors = HashSet::new();
        let placed: HashSet<_> = anchors_in_pages_source(PAGES_SOURCE).into_iter().collect();
        for entry in PREF_SEARCH_INDEX {
            assert!(
                anchors.insert(entry.anchor),
                "検索索引で anchor が重複しています: {}",
                entry.anchor
            );
            assert!(!entry.title.is_empty(), "title が空です: {}", entry.anchor);
            assert!(
                PAGES_SOURCE.contains(entry.title),
                "title が pages.rs の表示文字列と一致しません: {} / {}",
                entry.anchor,
                entry.title
            );
            let mut keywords = HashSet::new();
            for keyword in entry.keywords {
                assert!(
                    keywords.insert(keyword.to_ascii_lowercase()),
                    "keywords が重複しています: {} / {}",
                    entry.anchor,
                    keyword
                );
            }
            assert!(
                placed.contains(entry.anchor),
                "検索索引にある anchor が pages.rs の anchored 呼び出しにありません: {}",
                entry.anchor
            );
        }
    }

    #[test]
    fn all_page_list_covers_every_preferences_page_variant() {
        let source_variants: HashSet<String> =
            preference_page_variants_in_source(PREFERENCES_SOURCE)
                .into_iter()
                .map(str::to_owned)
                .collect();
        let listed_variants: HashSet<String> = PreferencesPage::ALL
            .iter()
            .map(|page| format!("{page:?}"))
            .collect();
        let missing: Vec<_> = source_variants.difference(&listed_variants).collect();
        let stale: Vec<_> = listed_variants.difference(&source_variants).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "PreferencesPage::ALL と列挙子が一致しません: ALL に不足={missing:?}, 存在しない値={stale:?}"
        );
    }

    #[test]
    fn every_page_anchor_is_indexed_and_every_page_has_an_entry() {
        let indexed: HashSet<_> = PREF_SEARCH_INDEX.iter().map(|entry| entry.anchor).collect();
        for anchor in anchors_in_pages_source(PAGES_SOURCE) {
            assert!(
                indexed.contains(anchor),
                "pages.rs に置いた anchor が検索索引にありません: {anchor}"
            );
        }

        let missing = missing_pages(PreferencesPage::ALL, PREF_SEARCH_INDEX);
        assert!(
            missing.is_empty(),
            "検索 entry が 1 件もない PreferencesPage: {missing:?}"
        );
    }

    #[test]
    fn missing_page_check_reports_a_new_unindexed_page() {
        let indexed = [PrefSearchEntry {
            anchor: "test/general",
            page: PreferencesPage::General,
            title: "test",
            keywords: &[],
        }];
        let pages = [PreferencesPage::General, PreferencesPage::Font];
        assert_eq!(missing_pages(&pages, &indexed), vec![PreferencesPage::Font]);
    }

    fn test_tree_position(page: PreferencesPage) -> (&'static str, usize, usize) {
        let index = PreferencesPage::ALL
            .iter()
            .position(|candidate| *candidate == page)
            .unwrap();
        ("カテゴリ", 0, index)
    }

    #[test]
    fn title_prefix_precedes_title_substring() {
        let results = search_preferences("表示", test_tree_position);
        let prefix = results
            .iter()
            .position(|entry| entry.title.starts_with("表示"))
            .unwrap();
        let substring = results
            .iter()
            .position(|entry| !entry.title.starts_with("表示") && entry.title.contains("表示"))
            .unwrap();
        assert!(prefix < substring);
    }

    #[test]
    fn search_uses_and_ascii_case_insensitive_and_keywords() {
        let ai = search_preferences("upSCALE 高画質", test_tree_position);
        assert_eq!(
            ai.first().map(|entry| entry.anchor),
            Some("general/ai-processing")
        );
        assert!(search_preferences("upscale 存在しない語", test_tree_position).is_empty());
        assert!(
            search_preferences("マウスホイール", test_tree_position)
                .iter()
                .any(|entry| entry.anchor == "spread/continuous-scroll")
        );
    }

    #[test]
    fn recycle_bin_confirmation_search_opens_the_folder_file_page() {
        let result = search_preferences("ごみ箱 削除確認", test_tree_position)
            .into_iter()
            .find(|entry| entry.anchor == "folder/delete-confirmation")
            .expect("the moved recycle-bin setting must remain searchable");
        assert_eq!(result.page, PreferencesPage::Folder);
        assert_eq!(result.page.label(), "フォルダ・ファイル");
    }
}
