# v2.7.0 実装計画

更新: 2026-07-22

## 本ブックマークと全メディア横断一覧

v2.7.0 では、既存の動画・音声ブックマークに加え、製本、画像のみフォルダ、ZIP / CBZ / PDF、
対応アーカイブのページをブックマークできる。画像フルスクリーンの `B` は現在の本ページを登録し、
透過背景色切替の新規既定は `Shift+B` へ移す。

- 画像フォルダはページ index ではなく相対ファイルパス、アーカイブは完全な entry name、
  PDF は 0-origin ページ番号で保存する。変換アーカイブは cache ZIP ではなく元パスを保持する。
  アプリ内の本 / ページのリネーム、製本ページの並べ替え・別本への移動では、操作結果の path mapping を
  `book_bookmarks.db` にも transaction で適用する。ファイル変更前に元 path → 最終 path と、永続
  temp 名・copy/move の SHA-256 identity を含む filesystem step plan を同 DB の `Prepared` journal
  へ記録する。copy/move は最終 path へ直接書かず、journal 固有の sibling staging file を
  `create_new` で完成・同期してから atomic no-clobber で公開する。復旧時に最終 path の identity が
  異なる場合は外部ファイルとして一切削除せず、診断付きで journal を保持する。rename / create /
  delete 後は影響する親ディレクトリを同期し、その namespace barrier が成功した step だけ進捗へ
  記録する。`Applying` / `RollingBack` は step 進捗と診断を保持し、全 filesystem step の barrier 後
  だけ filesystem committed として bookmark mapping を適用する。`Prepared` のまま異常終了した操作は
  no-op 破棄し、適用中または rollback 中の操作は次回起動時に実 filesystem 状態から冪等に収束させる。
  全 rollback step の成功を証明できた場合だけ journal を消す。外部変更は missing 行として保持する。
- 左パネルを「画像補正 / 表示トリム / ブックマーク」の 3 タブにし、現在の本の登録ページを
  サムネイル付きで表示、移動、任意名称の編集、削除ができる。
  ネスト ZIP では完全な entry prefix の階層を materialize してからページを解決し、現在表示中とは
  別階層のブックマークにも移動できる。直下ページの親 prefix は空 (= ZIP root) として扱い、
  非空 prefix はツリー上の実在を確認してから現在階層を変更する。
- `場所▼ > ブックマーク` で動画・音声・本を横断表示し、メディア種別と本 subtype で絞り込む。
  任意名称はサムネイル中央へ表示し、詳細ビューでは元ファイル名と任意名称を併記する。
  動画・音声の行は再生初期化後に登録時刻へ移動する。
  サムネイル比率が自動の場合は一覧の前回確定値をキャッシュし、viewer から戻るたびに
  一度 `1:1` へ戻って再計算するちらつきを防ぐ。
  開いた対象から移動せず本・動画・音声を閉じると同じブックマーク一覧へ戻る。前後ファイル移動や
  `Ctrl+↑/↓` で別ファイル・別コンテナへ移動した後は、移動先の実フォルダ一覧へ戻る。
  「フル機能ウィンドウ + 動画・音声を別ウィンドウで再生」では、メインウィンドウにブックマーク
  一覧を保持したまま、実フォルダ一覧・プレイヤー・登録時刻への seek を detached viewer context が
  所有する。実フォルダ側のタグキャッシュ / rating hydration worker も detached context が所有し、
  読み込み開始時にメイン一覧のタグバッジを消さない。Esc / 右クリック / 別ウィンドウの close は
  同じ終了経路を通り、1 回の操作で再生を終了する。対象から移動していた場合だけ、detached context の
  最終フォルダと選択ファイルをメインへ反映する。終了時の DB 再照合で一覧内容が同一なら、
  メイン一覧を再構築せず現在の表示を維持する。
  同モードで detached 動画・音声の再生中に本ブックマークを開く場合は、先にメディア context を
  共通 handoff で `ParkedLive` へ退避してから、本を従来のフル機能 container open へ渡す。
  active media session を残したまま book session を開始せず、動画窓の再表示と本 open の競合を防ぐ。
  「画像を別ウィンドウで開く」複数ウィンドウモードでは、本のブックマークも通常の本を開く経路と
  同じ independent detached viewer context を使う。PDF / ZIP / 画像フォルダ / 製本 / 変換済み
  対応アーカイブのページ列挙と対象ページ解決は detached context が所有し、メインウィンドウは
  ブックマーク一覧・選択・スクロール・タグ表示を変更しない。本の中でページ移動しても main 一覧は
  独立しているため維持し、続けて別の本を開く操作は既存 viewer を main へ載せ替えず通常の
  always-new handoff で別ウィンドウを作る。
  rating の idx cache は各 context が所有する一方、path ごとの最終書込値と世代は App-global に置く。
  通常画像、path 指定、現在のフォルダ / ZIP / PDF の全ユーザー書込を同じ DB + 世代記録 boundary に
  通す。SQLite 書込が成功した場合だけ App-global 世代と各 context の表示 cache を更新し、失敗時は
  楽観表示・Undo・XMP 書込を公開せずエラーを通知する。XMP hydrate は投入時世代より新しい書込が
  あれば破棄し、detached での変更・0 クリアを main 一覧へ swap 境界で同期するため、一覧再構築を
  省略しても古い星や古い XMP 値を復活させない。
  一覧が同一なら開く直前のスクロール位置を保持し、ブックマークの増減等で一覧が変わった場合は
  開いた行が可視範囲へ入るようにスクロールする。
  行削除は DB 上のブックマークだけを対象とし、元ファイルは削除しない。missing 行も保持する。
- 一覧から開いた位置と戻り先は viewer context の状態として保持する。detached viewer / ParkedLive
  音声窓でも実プレイヤーを所有する context が初期化完了後の最終 seek を発行する。開く処理は待機段階、
  対象 path、player state、seek serial を `[bookmark-open]` として常時ログへ記録する。メディア DB の
  正規化 path key とフォルダ列挙・プレイヤーの実 path は、ドライブを保持した同一の正規化規則で照合する。
  開く要求は media / book の独立した `Option` を併存させず、単一の型付き pending とする。戻り先も
  origin grid を所有する `Opening`、DB 再構築後に位置を戻す `Restoring`、別 viewer context 側の
  `Detached` を明示し、snapshot の有無を ownership 判定に流用しない。マウス、Enter、ゲームパッドは
  すべて同じブックマーク open router を通す。
- スマートフィルタの `状態` に `ブックマークあり / ブックマークなし` を追加する。動画・音声は
  対象ファイル、本はコンテナまたは表示中ページの安定 identity で判定し、スマートフォルダの
  保存条件にも含める。
- 本 DB と横断一覧の SQLite / filesystem / archive I/O は worker へ分離する。同じ ZIP / PDF に
  複数のブックマークがある場合、missing 判定用の entry / ページ列挙は一覧構築1回につき
  コンテナごとに1回だけ行い、結果を共有する。

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
  入力元を開いてから保存先を `create_new` し、入力元 open またはコピー失敗で空・部分ファイルを残さない。
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
