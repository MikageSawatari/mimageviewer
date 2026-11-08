# 一覧のコンテナを種類アイコンで表示する (§1.697)

作成: 2026-11-08。状態: **設計確定 (独立設計レビュー r3 ACCEPT、2026-11-08)・実装中**。対象版: v5.1.0 候補。
要望: 5ch >>728・730。バックログ: [next-release-backlog.md §1.697](next-release-backlog.md)。

## 1. 要件

- 環境設定 → サムネイルに切り替えを 1 つ足す。既定は OFF で、OFF の挙動は今と同じ。
- ON のとき、一覧のフォルダ・書庫・PDF などの**コンテナ**は、代表画像 (自動選定・手動固定とも) を
  描かず、今「代表画像が無いとき」に描いている既存の種類アイコンで描く。
- 単体の画像・動画・音声のサムネイル、本を開いた後のページ (ZIP 内画像・PDF ページ) は変えない。
- 既存の「サムネイルに種類アイコンを表示する」(`thumb_show_type_icon`、左上の固定マーク) とは独立。
  両方 ON なら、中央の種類アイコンと左上のマークが両方出る (左上は今も代表画像なしのセルに出ている)。
- 切り替えで手動固定 (`folder_thumb_pins.db`、`video_pins.db`) とサムネイルキャッシュを消さない。
  OFF に戻せば、残っているキャッシュ・固定をそのまま使う。

## 2. 対象

### 2.1 対象の項目 (`GridItem`)

| 種類 | ON での表示 | 理由 |
| --- | --- | --- |
| `Folder` (通常一覧。製本・本扱いの画像フォルダ・コレクションのフォルダ行を含む) | 📁 | フォルダ |
| `Folder` (ドライブ一覧) | ドライブの図形 | ドライブ一覧の代表画像も固定された代表なので同じ扱い |
| `ZipFile` / `PdfFile` (EPUB は PdfFile) | 📦 / 📄 | 書庫・PDF |
| `ConvertibleArchive` | 🗜 | RAR / 7z 等 |
| `ZipDir` | 📦 (内側書庫) / 📁 (書庫内フォルダ) | 書庫の中のフォルダ・書庫もコンテナ |
| `SearchContainer` | 📁 / 📦 + 階層・件数 | 検索結果のまとまり。今の「代表なし」の描画と同じ |

対象外:

- `Image` / `Video` / `Audio` / `ZipImage` / `PdfPage`: 単体の画像・ページ (要件)。
  ブックマーク一覧の本ページ行もこれらなので変わらない。
- `Stack` (ファイル名スタック): 同じ接頭辞の**画像**を畳んだもので、代表は単体画像のサムネイルを
  そのまま使っている。既存の大きな種類アイコンも無い (代表が無いと「読込失敗」表示になる)。
  利用者の「単体画像のサムネイルはそのまま」に合わせて対象外とする。
- `CollectionPlaceholder`: 今も常に専用のアイコン表示。

判定は 1 つの純粋関数 `grid_item::container_icon_only(item, enabled) -> bool` だけが持つ
(種類の列挙をこの関数以外に書かない)。以下「対象の項目」はこの関数が真の項目を指す。

### 2.2 対象の一覧

判定は項目の種類だけで決まるので、メインウィンドウのサムネイル一覧すべてに同じように効く:
通常フォルダ、書庫・PDF の中 (`ZipDir`)、本棚、コレクション (ルートと中身)、レーティング一覧、
Everything 検索結果、グローバル検索の集約、スマートフォルダ、お気に入り横断検索、閲覧履歴、ドライブ一覧。
いずれも共通の要求経路 (`update_keep_range_and_requests` → `make_load_request_with_visibility`) と
共通の描画 (`grid_paint::paint_cell`) を通る (棚卸し: 2026-11-08 Codex 調査、設計レビュー r1 で再確認)。

対象外の画面:

- detached のページ概要: 要求対象が Image / ZipImage / PdfPage に限られ、コンテナを含まない。
- 詳細表示の Preview 列: もともと代表画像を描かず型のベクターアイコンを描く。変更なし。
  詳細表示の hover 拡大プレビューは §4.4。
- **mIV Remote (Web UI)**: 独自の HTML/CSS 描画で、本体の表示設定を運ぶ IPC 項目も無い
  (名前の表示設定 `thumb_name_prioritize_width` と同じ扱い)。Remote は従来どおり代表画像を出す。
- お気に入りの「独自の表示状態」(`FavoriteViewState`): 含めない。既存の `thumb_show_type_icon` と
  同じく全体設定とする。

## 3. 設計の方針 (状態の組み合わせを減らす)

CLAUDE.md の方針に従い、状態の種類・回復の仕組みを増やさない。

- **描画**: 対象の項目は、描画とレイアウトに渡すサムネイル状態を `ThumbnailState::NoArt` に
  置き換える (`effective_thumbnail_state`)。今ある「代表なし」の描画分岐がそのまま種類アイコンを描く。
  新しい描画分岐・新しい状態の種類は作らない。
- **要求**: 対象の項目は、一覧の要求経路で要求を作らず、要求の出ていない `Pending` / `Evicted` を
  `NoArt` (画像なしの正常終端) にする。終端なので、再要求・hover の再描画・release check の終端判定から
  自然に外れる。`NoArt` は表示状態であり、要求の owner (`ThumbnailRequests`) の終端通知の代わりには
  使わない。要求が既に出ている idx は書き換えず、その要求は通常の終端で閉じる (§4.2)。
- **画像待ちの集計**: 「表示する画像の到着を待つ」集計 (自動比率、スクロール計測) は、対象の項目を
  母数・標本・完了条件から除く (§4.3)。
- **切り替え**: 一覧の読み直しはしない (読み直しはコレクションのルートで効かず、Everything では
  別の検索に置き換わるため。設計レビュー r1 P2)。ON は次のフレームから描画の置き換えと要求の停止で効く。
  OFF は、現在の一覧で対象の項目の `NoArt` を `Pending` に戻すだけで、通常の要求経路が
  残っているキャッシュ・固定から読み込む (§4.5)。現在地・検索条件・選択・スクロールはそのまま。

検討して採らなかった案:

- 描画だけを置き換え、要求は出し続ける: 実装は最小だが、表示しない代表画像の探索
  (フォルダの再帰走査、ZIP の中央ディレクトリ読み、PDF 描画) を続けることになり、要望の背景
  (一覧を見分けやすく・軽く) に合わない。
- 要求を出さずに `Pending` のまま置く: `Pending` は起動処理・検索・ゲームパッド・snapshot など
  多くの箇所で「まだ届いていない」の意味で読まれており、それぞれに例外を足すことになる。
  終端の `NoArt` にすれば、終端を扱う既存の経路にそのまま乗る。
- `NoArt` ではなく新しい状態 `IconOnly` を足す: `ThumbnailState` の全 match に分岐が増える。
  コンテナは今 `NoArt` を受けない (現状 `NoArt` を作るのは Audio だけ) ので、既存の
  「画像なしの正常終端」の意味で足りる。
- 切り替え時に一覧を読み直す (`reload_current_folder_preserving_override`): r1 案。コレクションのルートは
  物理一覧として扱われて戻らず (`current_folder = None`)、Everything は `refresh_global_search` が
  メタ検索で再検索して結果を置き換える。一覧の種類ごとに再入場の経路を設計するより、状態の戻しの方が狭い。
- OFF のとき keep ループが「コンテナの `NoArt`」を見て再要求する: コンテナが `NoArt` になる理由が
  この設定だけ、という暗黙の前提に依存する。切り替えの確定時に一度だけ戻す。
- 判定を `ThumbnailOverlayLayout` やセル描画の中に持たせる: 描画関数が設定を知らない今の構造を崩す。
  呼び出し側で状態を置き換える 1 か所の helper にする。

まれな失敗 (設定 DB の書き込み失敗など) は既定の割り切り方針に従い、追加の回復処理を設けない。

## 4. 詳細

### 4.1 設定

- `Settings::thumb_container_type_icon_only: bool`、`#[serde(default)]` (= false)。
  `settings_kv` の新しいキーなので、リリース済みの設定 DB に対しても**移行は不要** (欠けていれば OFF)。
- 設定の移送 (`settings_transfer.rs`) に bool / plain で登録する。
- 環境設定 → サムネイル、`thumbnail/type-icon` の直後に anchor `thumbnail/container-type-icon`:
  - チェックボックス: 「フォルダ・書庫・PDF を種類のアイコンで表示する」
  - 説明 (small): 「代表画像の代わりにアイコンを表示します。画像・動画・音声と、本を開いた後のページは
    そのままです。固定した代表画像やキャッシュは残り、オフに戻すと再び表示されます。」
  - 設定検索の索引に載せる (既存 anchor と同じ登録方法)。

### 4.2 要求経路 (表示しない代表の探索・生成を省く範囲)

対象の項目について、**メインウィンドウの一覧が出す要求だけ**を省く:

| 経路 | ON での扱い |
| --- | --- |
| `update_keep_range_and_requests` の keep 内ループ (可視・先読み) | 要求を作らない。owner に要求の無い `Pending` / `Evicted` を `NoArt` にする |
| ドライブ一覧の pin-only 要求 (`make_drive_list_pin_load_request_with_visibility`) | 同上 |
| `enqueue_priority_thumbnail` (詳細表示 hover・優先要求) | 要求しない |
| `enqueue_idle_upgrades` | 共通判定で除く。ON にする前の `Loaded` や、切り替え前の要求の遅着で `Loaded` になった項目も高画質化しない (r1 P2) |

既に owner に要求がある idx (切り替え直前に出た要求) は、表示状態を書き換えず、owner の通常の
終端 (`Pixels` 採用 + `Finalized`、`Failed`、`Canceled` 等) で閉じる。採用されて `Loaded` になっても
描画は §4.3 で `NoArt` に置き換わり、idle 高画質化もされない。次に `Evicted` になったとき keep ループが `NoArt` にする。

省かないもの (一覧の表示とは別の利用者で、表示設定に従わせない):

- 一括サムネイル作成、mIV Remote のサムネイル要求、AI 説明文の表紙解決 (`resolve_book_cover`)、
  MCP の `item_info`、色フィルター、開いた本の先頭ページからの親表紙書き戻し、
  `seed_folder_video_pin_thumbs`、PDF の隣接本先読み。
- これらは今も一覧とは別に代表を作る・読む。一覧の `thumbnails` / texture へは書き込まない
  (設計レビュー r1 で確認) ので、ON の間に代表画像が一覧へ混入することもない。

ON の間に一覧が代表行を作らないことの影響 (受け入れる):

- 親フォルダの自動代表は、子コンテナの保存済み代表行を再利用する。ON の間に新しい子の行は
  一覧からは作られないので、OFF に戻した直後の親の代表が、行がある場合と違う候補になり得る。
  OFF で子を表示すれば行が作られ、親の proof (catalog revision) が変わって次の読み込みで選び直される。
- ドライブ直下を表示したときの `force_cache` 保存も一覧の要求に乗っているので、ON の間は
  ドライブ一覧用の行が新しくは作られない。OFF でドライブ直下を表示すれば作られる。
- AI 説明文・MCP の表紙解決が子の既存行 (`ChildCached`) を使う場合、ON の間に新しい行が増えないぶん
  選ばれる表紙が OFF のときと違い得る。行は消さないので、ON の前に作られた行による結果は変わらない。
- 既存の行・固定は消さないので、ON の前に作られたものは OFF に戻せばそのまま使われる。

### 4.3 描画と画像待ちの集計

描画:

- 一覧のセル描画・レイアウトの全呼び出し元に、`effective_thumbnail_state(item, thumb, enabled)` で
  求めた状態を渡す。判定が真なら `&ThumbnailState::NoArt`、偽なら元の状態。
  r1 で確認した呼び出し元: 通常 grid の layout / draw (`ui_main.rs` 21223 付近・21395 付近)、
  背景判定用 `draw_cell_with_hit_areas` (21261 付近)、check / cut 変更後の再 layout (21340 付近)、
  入力後のセル再描画の layout / draw (17517 付近・17553 付近)。テストと snapshot fixture は設定を持たないので対象外。
- 補正済みテクスチャ (`adjusted_tex`) も、判定が真なら渡さない。
- レイアウト (名前プレート・フォルダ名バッジ・SearchContainer の caption 位置・背景 hit 判定) と描画が
  同じ状態を見る。painter 内部は渡された状態だけを読む (r1 で確認)。

画像待ちの集計 (同じ判定 `container_icon_only` を使う):

- **自動比率** (`thumb_aspect_auto`): 母数・Loaded からの再構築・poll 後の標本追加・cache seed の標本集め・
  コレクションの準備済み母数・「全サムネイルが終端」の完了条件のすべてで、対象の項目を除く
  (keep 外で `Pending` のままの対象の項目も完了を妨げない)。
  加えて、判定時 (`maybe_apply_auto_aspect`) に標本 map を読むときも idx と現在の判定で対象を除き、
  必要標本数・連勝判定・保存する標本件数を同じ有効標本集合で数える。OFF の間に集めたコンテナの標本が
  map に残っていても、ON の間は判定に使わない (map から消さないので、OFF に戻せば再び使われる)。
  全項目が対象の一覧は、母数 0 の既存の扱い (現在の比率で確定) になる。
  保存済みの自動比率 (フォルダごとの記憶) は、標本の集め方が変わっても今までどおり先に復元して使う。
  切り替えたときに現在の一覧の比率を計算し直す処理は作らず、次にその一覧を読み込んだときの
  通常の流れに任せる (保存済みがあればそれを使う)。
- **スクロール計測** (`runtime_ops.rs` の `visible_thumb_all_ready`): 可視の画像待ち target から
  対象の項目を除く。`thumb.ready` を偽装しない。target の作成時だけでなく、進行中の計測
  (`ScrollSettleState.visible_target` と covered の集計) にも現在の判定を適用し、OFF の間に始まった計測が
  ON の後にコンテナを待ち続けないようにする。
  target が空、または作成時点で既に充足している場合も、同じ settle seq の完了 endpoint
  (`visible_thumb_all_ready`) を出す (今は `already_loaded >= target_count` で黙って戻り、analyzer が
  各 settle に要求する endpoint が欠ける。r2 P3)。

### 4.4 詳細表示の hover 拡大プレビュー

今は `NoArt` / `Failed` で「表示できません」を出す。対象の項目は hover プレビューを開かない
(ドライブ一覧と同じ扱い)。代表画像を出さない設定なのに「表示できません」と見せない。

### 4.5 切り替え時

`apply_preferences_dialog` で旧値と新値を比べる。一覧の読み直しの `else if` 連鎖には入れない
(読み直しは他の設定の理由で起きればそれに任せる。読み直された一覧は新しい値で要求が決まる)。

- OFF → ON: 状態は変えない。次のフレームから描画の置き換えと要求の停止が効き、keep ループが
  要求の無い `Pending` / `Evicted` を `NoArt` にする。`Loaded` のテクスチャは描かれないまま、
  通常の退去で解放される。
- ON → OFF: 現在の一覧 (App が今持つ `items` / `thumbnails`) で、`container_icon_only(item, true)` が真かつ
  `NoArt` の idx を `Pending` に戻し、再描画を要求する。keep ループが通常どおり要求し、
  キャッシュ・固定から読み込む。`Loaded` のまま残っていた項目はそのまま再び描かれる。
  これは表示状態だけの遷移で、items generation・要求 owner・キャッシュ・固定には触れない。
- 退避中の別 viewer context (`viewer_context_registry` に退避されている一覧) は書き換えない
  (他 context を変更しない。[detached-rework-plan §2](detached-rework-plan.md))。ON は描画の置き換えで
  復帰時から効く。OFF の場合、退避中に `NoArt` になった項目は、その一覧を次に読み込むまでアイコンのまま
  (既存のフォルダ代表の「表示済みテクスチャは次の読み込みまで残る」と同じ割り切り)。

### 4.6 手動固定の操作

ON の間も「代表サムネに固定」の操作は従来どおり使える (固定は保存され、OFF に戻すと表示される)。
固定の変更後に既存処理が該当セルを読み直す・`Evicted` にする場合も、keep ループが要求を作らず `NoArt` にする。

## 5. テスト

- 判定関数: 全 `GridItem` の種類 × ON/OFF の表 (Stack・Image 等は常に偽)。
- 要求経路 (App テスト): ON で Folder / ZipFile / PdfFile / ConvertibleArchive / ZipDir / SearchContainer /
  ドライブ一覧を表示すると、要求台帳に要求が無く keep 内の状態が `NoArt`、Image / Video / Stack は
  従来どおり要求される。OFF で同じ一覧は従来どおり要求される。
- 要求中の切り替え: OFF で要求が出た直後に ON → その要求は通常終端で閉じ、owner に残らない。
  採用後も描画は `NoArt` 相当、idle 高画質化の要求が出ない。
- idle 高画質化: ON で `Loaded` (UpgradeableCache) のコンテナがあっても upgrade 要求が出ない。
- 切り替え (環境設定の OK): ON → 要求が止まる。OFF → `NoArt` が `Pending` に戻り、既存キャッシュ行
  (手動固定の `#pin:` 行を含む) から `Loaded` になる。固定 DB の行とキャッシュ行が残っていること。
  Cancel は何も変えない。コレクションのルートと Everything 結果で、OFF に戻すと代表が戻り、
  現在地・検索結果 (Everything の結果集合)・選択・スクロールが変わらないこと。
- 描画: 代表画像を持つ `Loaded` の Folder / ZipFile / SearchContainer / ドライブ一覧の Folder を ON で描くと、
  代表なしと同じアイコン・ラベル配置・hit 領域になる (既存の painter テストの形)。
- 本の中のページ一覧 (ZipImage / PdfPage) と Image は ON でも `Loaded` のまま描かれる。
- 自動比率: OFF の間にコンテナの標本を集めた後 ON にし、新着画像なしで通常の poll を回しても比率が変わらない。ON で (a) 全項目が対象で keep 範囲を超える一覧が確定する、(b) 少数の画像との混在で画像だけを
  標本にして確定する、(c) コレクションの一覧、(d) ON にする前の `Loaded` があっても標本にしない。
- スクロール計測: 全項目が対象の画面・全画像が既読込の画面・混在画面、および未完了の settle 中に ON へ切り替えた場合に、同じ seq の `visible_thumb_all_ready` が出る (analyzer の検査も通る)。
- hover プレビュー: ON でコンテナに hover しても開かない。
- 設定: 既定 false、DB 往復、欠けたキーは false、設定移送の往復。
- UI スナップショット: 環境設定のサムネイルページ (`preferences_favorite_view_state_dark` 等、
  当該ページを写す fixture) を更新。ON の一覧セルの snapshot を 1 枚足す。

## 6. 実装記録

(実装後に記入: 触ったファイル、テスト名、レビュー結果)

## 7. 設計レビューの記録

- r1 (GPT-6.1 Sol / xhigh、2026-11-08): REVISE。P2 読み直し関数がコレクションのルート・Everything で不適 →
  読み直しをやめ、OFF 時に現在の一覧の `NoArt` を `Pending` に戻す方式へ (§3・§4.5)。P2 自動比率の母数・
  標本・完了条件 → 全入口に判定を適用 (§4.3)。P2 idle 高画質化が旧 `Loaded` を拾う → 明示的に除く (§4.2)。
  P3 スクロール計測が `Loaded` だけを完了と数える → target から除く (§4.3)。
  `NoArt` の再利用、対象種類、Stack・Remote・FavoriteViewState の除外、hover を開かない方針、
  別経路からの画像混入なし、detached §2 に抵触しないことは r1 で妥当と確認された。
- r2 (同、resume): REVISE。読み直しをやめた方式・要求 owner の扱い・現在の App を所有先とすること・
  別理由のコンテナ `NoArt` が無いこと・keep 外 `Pending` の完了条件は妥当と確認。P2 切り替え前の
  自動比率の標本が判定に混入 → 判定時にも除外 (§4.3)。P3 スクロール計測の空 target で endpoint が欠ける・
  進行中の target が残る → endpoint を出す契約と進行中への適用を追記 (§4.3)。
- r3 (同、resume): ACCEPT。追加指摘なし。
