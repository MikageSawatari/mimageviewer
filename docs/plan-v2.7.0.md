# v2.7.0 実装計画

更新: 2026-07-21

## 本ブックマークと全メディア横断一覧

v2.7.0 では、既存の動画・音声ブックマークに加え、製本、画像のみフォルダ、ZIP / CBZ / PDF、
対応アーカイブのページをブックマークできる。画像フルスクリーンの `B` は現在の本ページを登録し、
透過背景色切替の新規既定は `Shift+B` へ移す。

- 画像フォルダはページ index ではなく相対ファイルパス、アーカイブは完全な entry name、
  PDF は 0-origin ページ番号で保存する。変換アーカイブは cache ZIP ではなく元パスを保持する。
- 左パネルを「画像補正 / 表示トリム / ブックマーク」の 3 タブにし、現在の本の登録ページを
  サムネイル付きで表示、移動、任意名称の編集、削除ができる。
- `場所▼ > ブックマーク` で動画・音声・本を横断表示し、メディア種別と本 subtype で絞り込む。
  任意名称はサムネイル中央へ表示する。動画・音声の行は再生初期化後に登録時刻へ移動する。
  行削除は DB 上のブックマークだけを対象とし、元ファイルは削除しない。missing 行も保持する。
- スマートフィルタの `状態` に `ブックマークあり / ブックマークなし` を追加する。動画・音声は
  対象ファイル、本はコンテナまたは表示中ページの安定 identity で判定し、スマートフォルダの
  保存条件にも含める。
- 本 DB と横断一覧の SQLite / filesystem / archive I/O は worker へ分離する。

詳細な確定仕様は [spec.md](spec.md)、identity は [virtual-folders.md](virtual-folders.md)、
キー優先順位は [keymap-spec.md](keymap-spec.md)、worker 境界は
[async-architecture.md](async-architecture.md) を参照。

## UI フォント設定

v2.7.0 では、Windows にインストール済みの日本語フォント、またはユーザーが選んだ
TrueType / OpenType フォント (`.ttf` / `.otf` / `.ttc` / `.otc`) を mImageViewer の
UI フォントとして指定できるようにする。フォントサイズの独立設定は追加せず、従来の
「設定 → スケーリング」を文字を含む UI 全体の倍率指定として使う。

### UI と保存形式

- 環境設定「表示 → フォント」の専用ページで、既定フォントとシステムフォントを選択する。
- 日本語 (`今`, `あ`) を持たない face と Italic / Oblique は一覧・ファイル追加の対象外にする。
- TTC / OTC はファイルパスと face index を保存し、コレクション内の書体を正しく復元する。
- 同一ファミリーの face は OS/2 weight (`Thin`〜`Black`) を正確に表示し、可変フォントは
  `Variable` と明記する。ラベル変更前の保存値は path + face index で同一性を判定する。
- ファイルから追加したフォントは `%APPDATA%/mimageviewer/user_fonts` へ重複上書きせずコピーする。
- 選択前に日本語・英数を含むプレビューを表示する。記号・絵文字など選択 face にない
  glyph は既定 fallback で補う。
- 自動補正後の縦位置へ `-4.0..=4.0 pt` の微調整を追加できる。既定は `0.0 pt`。

### 縦位置補正

固定フォント専用の値だけで位置を決めず、`ttf-parser` で選択 face の ascent / descent /
line gap と代表 glyph (`今`, `あ`, `A`, `a`, `0`) の outline bbox を測る。既定の Yu Gothic
との視覚中心差を `FontTweak.y_offset_factor` へ変換し、従来のツールバー補正値へ加える。
bitmap glyph など outline bbox を取れない場合だけ raster image bounds を使う。

記号、数学英字、絵文字、簡体字、繁体字、韓国語の fallback も選択フォントを基準に
相対補正する。ユーザー微調整は logical point なので DPI / UI スケーリングに追従する。

### 応答性と適用範囲

- システムフォント列挙、font file 読み込み、coverage 判定、プレビューのラスタライズ、
  font definitions の準備はワーカースレッドで行う。
- UI スレッドは小さいプレビューテクスチャの登録と `Context::set_fonts` だけを行う。
- font definitions のキャッシュは大きな CJK font data を保持するため直近 1 設定に制限する。
- 適用時は既存の font atlas full-resync 経路を使う。active な別ウィンドウ / fullscreen /
  native presenter は表示倍率変更と同じ正規 teardown 経路で閉じ、再表示時に新設定を使う。
- native 動画 HUD の独立 `egui::Context` にも `UiFontSettings` を渡す。ただし動画・音声 HUD の
  固定サイズ `Norm` / 再生速度 / 時刻 / 音量ラベルは、選択フォントで位置・幅が変わらない専用の
  既定 HUD font family を使う。VST 等の固定 glyph は従来どおりベクター描画する。

### 検証

- 設定の既定値、serde round-trip、sanitize。
- TTC face index、欠落ファイル fallback、手動縦位置、font atlas resync。
- システム catalog の重複排除・coverage、プレビューのラスタライズ。
- 日本語なし / Italic / Oblique face の候補除外と、保存値から渡された場合の既定 fallback。
- Meiryo Bold の手動補正 snapshot、および日本語 UI の代表候補である
  BIZ UDPGothic 9pt / Meiryo 10pt / Meiryo UI 10pt の自動補正 snapshot。
- 上記 3 face の実メトリクスによる視覚中心差と、記号・絵文字・CJK fallback の
  相対補正を unit test で固定する。
- `cargo fmt --check`、関連 unit test、`cargo test --test ui_snapshot`、
  `python scripts/check_ui_glyphs.py`、Windows release build。

実機では、既定 / BIZ UDPGothic / Meiryo / Meiryo UI / 追加した TTC の切り替え、
メイン一覧、設定画面、F11/F12、動画 HUD、動画の音声モード、音声ファイル HUD の表示と、
再起動後の復元を確認する。
