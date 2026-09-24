# EPUB → PDF スパイク結果 (2026-09-25)

計画の正本は `docs/backlog-on-hold.md` §1.247。本書はその「技術検証 (スパイク)」の結果記録。
実装は `crates/epub-pdf-worker` (単体 CLI `mimageviewer-epub-pdf`、mIV 本体へは未組み込み)。

## 方式

- EPUB パッケージ (container / OPF / spine / rendition の本全体・ページ単位の上書き /
  page-progression-direction / ページ寸法 / DRM) を Rust で解析し、作業フォルダへ展開する。
- spine を「固定レイアウトの連続区間」と「リフローの各項目」に分け、区間ごとに WebView2 で印刷し、
  `lopdf` で spine 順に結合する。右開きは `/ViewerPreferences /Direction /R2L`。
- 固定区間は 1 枚の印刷用 HTML に、ページごとに W×H CSS px の箱と名前付き `@page` を並べる。
  ラスタ画像のページは `<img>`、それ以外 (XHTML・**SVG**) は `<iframe>`。
- 印刷は DevTools `Page.printToPDF` (`preferCSSPageSize: true`)。
  **`ICoreWebView2_7::PrintToPdf` はページごとの CSS 寸法を無視する** (寸法混在の本で確認) ので使わない。

## 結果 (テスト素材 27 冊、デバッグビルド)

素材はリポジトリ外 `C:\home\mimageviewer_testdata_epub\` (IDPF epub3-samples 16 冊、
手元のリフロー絵本 4 冊、`gen_synthetic.py` の合成 7 冊)。

| 観点 | 結果 | 根拠 |
| --- | --- | --- |
| ページ順・右開き | 固定レイアウト全冊で spine 順どおり | 合成本のページ番号を画像化して目視、ハルコさんの彼氏 13p を目視 |
| ページ寸法 | 1 ページずつ保持 (見開き横長・横向きの混在も) | PDF の MediaBox と画像化 |
| JPEG の画質 | **再圧縮・縮小なし**。PDF 内の DCTDecode ストリームが元 JPEG と SHA-256 一致 (漫画系 全ページ) | レポートの `image_fidelity` |
| PNG | FlateDecode (可逆) で同寸法 | 同上 |
| DRM | `rights.xml` 付きを終了コード 2 で拒否。フォント難読化のみの本は変換 | 合成 DRM 相当本、wasteland-otf-obf |
| 変換時間 | 300 ページ (1600×2400 JPEG、117MB) で 23.5 秒。40 ページ 5.1 秒、ハルコ 13 ページ 0.8 秒 | デバッグビルドの実測 |
| リフロー (横書き) | 変換できる。既定 1200×1700 px / 16px 文字で**文字が小さい** | moby-dick 等を画像化 |
| 縦書き | 絵本 (ごん狐) はページ全高で正しく組める。**草枕は 1 行がページ高の約 4 割で上半分が空白** | 画像化して目視 |
| WebView2 Runtime | 153.0.4234.48 で動作。無い環境の失敗は終了コード 4 を用意 (実環境では未試験) | |
| ユーザーデータフォルダ | 作業フォルダ配下に明示指定。コントローラ終了後 12ms で削除できた回と、30 秒後も共有違反で消せなかった回がある | Codex の試行 |

## 見つけて直した不具合

- **SVG ページが白紙** (svg-in-spine の 3 ページ): SVG を `<img>` で貼ると SVG 内の外部画像
  (`<image href="flyer.jpg">`) が読み込まれない。SVG ページは iframe で描くよう修正。

## 未解決・組み込み前に決めること

1. **WebView2 の束縛**: 実装担当のサンドボックスに crates.io 接続が無かったため、COM vtable を
   手書きで束縛している (`webview.rs`)。組み込むなら `webview2-com` (0.39 系、`windows` 0.62) へ
   置き換えるか判断する。上流 README によれば既定で Loader を静的リンクでき、
   **`WebView2Loader.dll` の同梱が不要**になる (未検証)。現状は DLL を exe 隣に置く必要がある。
2. **Codex のサンドボックス内では WebView2 が起動しない** (`CreateCoreWebView2Controller` が
   `0x8000FFFF`)。サンドボックス外では同じバイナリが動く。変換の実行確認は検収側で行う。
3. リフロー時の組版パラメータ (ページ寸法・文字サイズ・余白)。決め打ち + 再変換で足りるか。
4. 縦書きの行長 (草枕)。本の CSS と注入 CSS の相互作用と見ているが未調査。漫画用途の成立条件ではない。
5. ユーザーデータフォルダが消せない場合の後始末 (次回起動時の掃除など)。
6. 朗読音声・動画などメディアは PDF に入らない (ごん狐の mp3)。無言で落ちる点を利用者へどう示すか。
