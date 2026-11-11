# 一覧のコンテナを種類アイコンで表示する (§1.697)

作成: 2026-11-08。状態: **実装・自動検証済み (設計レビュー r3 / 独立実装レビュー ACCEPT)、サブ PC の実機確認で NG なし (2026-11-09)、master 取り込み済み (2026-11-09、v5.1.0)**。対象版: v5.1.0 候補。

**§1.770 (2026-11-11) で種類ごとの選択へ広げた。設定・判定・移行は §8 が正本。§1〜§7 の「ON」は §8 では「その項目の種類が選ばれている」と読み替える。**
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

実装: 2026-11-08〜09、`feat-1697-container-type-icons` worktree。設計の主要な前提を
コードで確認したうえで §4 の範囲を実装。コミット・merge・push・製品起動は行っていない。

変更ファイル:

- `src/grid_item.rs`: 対象種類を持つ唯一の判定と表示状態の純粋な射影。
- `src/settings.rs`、`src/settings_transfer.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`、`src/ui_dialogs/preferences/search_index.rs`:
  既定 OFF・DB/移送・checkbox/説明/検索・OK 時の現在一覧限定復帰。
- `src/app.rs`、`src/ui_main.rs`: 一覧要求・idle upgrade の抑止、全 production
  layout/draw と補正 texture の射影、自動比率の母数/標本/判定/終端条件、hover 抑止。
- `src/app/runtime_ops.rs`、`scripts/analyze_perf.py`、`scripts/test_analyze_perf.py`:
  作成時・進行中の画像待ち集合を絞り、空/既充足も同 seq の完了 endpoint を出す。
  analyzer は完了時の有効母数を表示する。
- `src/app/tests.rs`、`src/app/tests/container_type_icons.rs`、`src/app/collection_grid.rs`、
  `src/global_search_ui.rs`、`src/app/grid_type_icon_tests.rs`: handler・状態遷移・実 painter 回帰。
- `src/app/container_type_icon_fixture.rs`、`src/lib.rs`、`tests/ui_snapshot.rs`:
  App を使わない実 Loaded の表示射影 fixture と新規一覧 snapshot。
- `tests/snapshots/preferences_favorite_view_state_dark.png` を更新し、
  `tests/snapshots/container_type_icon_loaded_grid_dark.png` を追加した。
- 本書、`docs/display-pipeline.md` §1.3.2、`docs/README.md` の設計索引を更新。

追加回帰は `container_type_icon`（設定/移送/検索/OK・Cancel、対象種類表、keep/priority/drive、
要求中の切替と両終端順、idle、比率の keep 外終端・Loaded/seed・旧標本の判定/連勝/保存件数、
キャッシュ/固定保持、parked context、Collection/Everything の一覧保持、hover、painter/hit）と
`container_icon_scroll`（空/既読込・混在・進行中切替・covered 集合）の filter で実行できる。
一覧 snapshot は `container_type_icon_loaded_grid_dark`、環境設定は
`preferences_favorite_view_state_dark` を指定して生成・通常比較する。

実装担当の内部レビュー（GPT-6.1 Sol / xhigh）で、`contains_key` が実要求の終端後も
`AwaitingArchiveJudgement` の受付枠を含むことを指摘された。keep の表示状態保護は
実要求 ID の有無で判断し、受付枠は残すよう修正した。
`container_type_icons_finished_deferred_slot_is_no_art_without_removing_admission_owner` で
終端受信後の `NoArt` 化と受付枠保持を検査する。§4.2 の実要求の寿命契約に沿う修正であり、
仕様変更はない。索引の「一覧読み直し」説明だけは旧案が残っていたため併せて修正した。

実装担当の内部レビューは修正再確認後 **ACCEPT**、未解決指摘なし。最終ソースの focused gate:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/container_type_icon|container_icon_scroll|auto_aspect|thumbnail_request_lifecycle|settings_transfer::tests|thumbnail_type_icon/)'
```

**161 passed / 0 failed**（新規25件を含む、実行10.465秒）。ログ:
`target/1697-focused-complete.log`。反復途中の新規 fixture の失敗は、変換書庫の
準備済み受付状態と環境設定 draft の初期化を既存 helper に合わせて修正した。
同じ最終 gate に失敗・timeout はない。
`python scripts/test_analyze_perf.py` は担当が **106 passed** を確認し、その結果を再利用した。
`cargo fmt`（引数なし）、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`
（危険文字0件）、`git diff --check` は成功。

UI snapshot は `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot <filter>` で
指定の2件をそれぞれ生成し、各 **1 passed**。更新指定なしの同2件の nextest 比較も
**2 passed / 0 failed**（`target/1697-snapshot-compare.log`）。設定ページは新checkboxと
small説明の折り返し・既存の名前設定、一覧は中央icon＋左上markと非対象3種の画像保持を
両PNGを開いて目視確認した。既存設定ページは600×260・68,789 bytes、新規一覧は
744×488・67,177 bytes、合計135,966 bytes。Appや製品GUIの実行を伴わない純描画検査である。

確認 build は 2026-11-09 に `$env:CARGO_BUILD_JOBS = '1'; .\scripts\build-dev.ps1 -PreserveRuntime`
で **成功（exit 0）**。共有 PC の負荷を抑えるため1並列とし、通常 feature set（portable なし）で
core / Remote / EPUB PDF worker / MCP adapter / music-video editor の5本を
`target/dev-runtime/` に生成した。初回 core は依存構築込み48分34秒。
DLL 配置と PE 検査も成功（runtime=4 / pe=5）。検査記録:
`target/vcrt-pe-reports/dev-runtime.json`。製品バイナリは起動しておらず、実機動作は未確認。
利用者の通常設定へのアクセス・稼働中製品の停止・git の commit/merge/push は行っていない。

利用者が確認する場合は、インストール版／トレイ常駐版を終了してから次を実行する。
同じ single-instance mutex を共有し、引数なしでは通常の `%APPDATA%\mimageviewer` を使うため、
実際の設定・データを更新し得る。新設定の ON/OFF、コンテナのアイコン表示、固定代表の復帰、
単体画像・本の中のページの画像保持を確認する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

`test-lib-full.ps1` / `test-full.ps1` は利用者指示により未実施で、ClaudeCode の最終 gate へ引き継ぐ。

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
- 実装レビュー (設計レビューと同じ独立担当、実装担当とは別、2026-11-09、`bb4c509a1..0c9b4003d` + マニュアル):
  P3 1 件 (マニュアルの「代表画像の読み込みも行いません」「影響しません」が言い過ぎ) のみ。提案どおり
  文言を直した (470db253a)。実装上の P1/P2 なし。
- 最終確認 (ClaudeCode、2026-11-09): `test-lib-full.ps1` 16,791 件成功 (74 skipped)、
  `cargo test --test ui_snapshot` 247 件成功、`scripts/test_analyze_perf.py` 106 件成功、
  `check_ui_glyphs.py` 成功。製品の起動による確認はこの時点では未実施。
- サブ PC 実機確認 (依頼 24、2026-11-09、観測者はサブ PC 担当 Claude の画面操作、隔離 data-dir、
  ビルド 9652331be = code 0c9b4003d の dev-runtime): A〜D に NG なし。通常一覧の ON/OFF で選択・スクロールが
  変わらない、OFF 中・ON 中に固定した表紙が OFF で出る、書庫内フォルダはアイコン・ページは画像、
  詳細表示の hover プレビューは本で出ない、閲覧履歴・ドライブ一覧・Everything (OFF で同じ 5 件のまま表紙が戻る)、
  自動比率 (本だけのフォルダは 1:1 のまま待ちなし、混在フォルダは画像に合わせて 3:2)。ログに ERROR / WARN / panic なし。
  未確認: 本棚・コレクション・複数ウィンドウ・Remote (自動テストのみ)。結果は `C:\miv-subpc\results-24\`。

## 8. 種類ごとの選択 (§1.770)

作成: 2026-11-11。状態: **設計レビュー r2 ACCEPT、実装 (9e7c3a97b) の独立実装レビュー ACCEPT (2026-11-12)。実機未確認**。要望: 5ch >>764 (実フォルダだけをアイコンにし、書庫・PDF の表紙はそのまま見たい)。
バックログ: [next-release-backlog.md §1.770](next-release-backlog.md)。

### 8.1 要件

- §1.697 の一括の切り替えを、**実フォルダ・書庫・PDF/EPUB の 3 種類**ごとに選べる形へ広げる。
  拡張子ごとの設定、単体の画像・動画・音声への拡張はしない。
- 新規の利用者と、旧設定 OFF の利用者は従来どおり代表画像 (3 種類とも未選択)。
- 旧設定 ON の利用者は、更新後も旧設定の対象すべてがアイコンのまま (3 種類とも選択)。
- 実フォルダだけを選んだとき、書庫・PDF/EPUB と書庫の中のフォルダ・書庫 (`ZipDir`) を巻き込まない。
- 選択を外した種類だけ通常の取得に戻し、代表の固定・キャッシュ・一覧の場所・選択・スクロールを保つ。
- §1〜§7 の構造 (1 つの判定、`NoArt` への射影、要求を作らない、画像待ちの集計から除く、
  idle 高画質化から除く、hover を開かない、切り替えで一覧を読み直さない) はそのまま使う。

### 8.2 種類の対応表

判定は項目の種類 (`GridItem` の variant) だけで決める。開いている一覧の種類 (通常フォルダ・本棚・
コレクション・検索結果など) やパスの中身は見ない。表は `grid_item::container_icon_kind` だけが持つ。

| 項目 | 種類 | 理由 |
| --- | --- | --- |
| `Folder` (通常一覧・本棚・コレクションのフォルダ行・スマートフォルダ・閲覧履歴など) | 実フォルダ | ディスク上のフォルダ。画像フォルダを本として扱っている場合も同じ (下記) |
| `Folder` (ドライブ一覧のドライブ) | 実フォルダ | ドライブはディスク上のフォルダの根。別の種類を設けるほどの差がない |
| `SearchContainer { kind: Folder }` (検索結果の集約行) | 実フォルダ | ヒットを含む親フォルダのまとまり |
| `ZipFile` | 書庫 | ZIP / CBZ |
| `ConvertibleArchive` | 書庫 | RAR / 7z / LZH、入れ子書庫を含む ZIP |
| `ZipDir` (`is_archive` が真 = 書庫の中の書庫、偽 = 書庫の中のフォルダ) | 書庫 | 書庫の一部。実フォルダだけを選んだときに巻き込まない (§8.1) |
| `SearchContainer { kind: Zip }` | 書庫 | ヒットを含む ZIP のまとまり |
| `PdfFile` (EPUB を含む) | PDF/EPUB | |

対象外は §2.1 のまま (`Image` / `Video` / `Audio` / `ZipImage` / `PdfPage` / `Stack` / `CollectionPlaceholder`)。

決めたこと:

- **画像フォルダを本として扱う設定のフォルダも「実フォルダ」**: 項目は `Folder` のままで、本として扱うかは
  開くときの扱いの違いにとどまる。種類の判定に本扱いの判定 (フォルダの中身の走査や本の設定の参照) を
  持ち込むと、判定が項目の種類だけで決まらなくなり、描画・要求・集計の各所で同じ答えを出す保証が崩れる。
  画像フォルダの表紙を見たい利用者は「実フォルダ」を選ばなければよい。
- **書庫の中のフォルダは「書庫」**: 書庫の中を見ている間のフォルダ行は書庫の一部で、ディスク上の
  フォルダではない。要望の「実フォルダだけ」をそのまま満たす。
- **ドライブは「実フォルダ」**: 一覧に出るのはフォルダと同じ `Folder` 項目で、要求経路も §4.2 のとおり
  共通の判定を通る。ドライブ専用の選択肢は作らない。

### 8.3 設定と移行

設定はリリース済み (v5.1.0 の `thumb_container_type_icon_only: bool`) なので移行が必要。

- 新しい値: `Settings::thumb_container_icon_kinds: ContainerIconKinds`、`#[serde(default)]`。
  `ContainerIconKinds { folder: bool, archive: bool, pdf_epub: bool }` (Copy、既定は全部 false)。
  `settings_kv` には 1 つのキー `thumb_container_icon_kinds` (JSON の object) で保存する。
  3 種類を 1 つの値にまとめるのは、旧キーからの変換を「1 つのキーを作る」1 回で済ませ、
  部分的な読み込み (設定ファイルの一部だけの取り込み) で 3 つがばらばらに混ざらないようにするため。
- 旧キー `thumb_container_type_icon_only` は `Settings` から外す。読み込み時に map の段階で新キーへ変換する。
  既存の前例 `same_name_folder_book::canonicalize` (旧 bool キー `skip_zip_if_folder_exists` → 新キー) と
  同じ形にする:
  - 新キーがあれば新キーを使う (旧キーは無視する)。
  - 新キーが無く旧キーが bool なら、`true` → 3 種類とも選択、`false` → 3 種類とも未選択の新キーを作る。
  - 新キーがあれば、新キーの値が壊れていても旧キーへは倒さない (壊れた新キーは各入口の通常の扱い)。
  - 旧キーが bool でない (壊れた値) なら、旧キーを取り除いて「旧値が不正」を返す (新キーは作らない)。
    扱いは入口ごとに決める (下記)。同名フォルダの前例は DB 読み込みで `Incompatible` を返すが、それは
    「将来の版の値を古い版が上書きしない」ための保護で、この旧キーは今後の版で書かれることがない。
  - 旧キーは map から常に取り除く。
- 変換関数は 1 つ (`canonicalize_container_icon_kinds(map) -> Result<(), ...>`)。通す入口は 3 つ:
  1. `impl Deserialize for Settings` (settings.rs、`same_name_folder_book::canonicalize` を呼んでいる所)。
     `settings_kv` からの読み込みと旧 JSON からの移行はここを通る。旧値が不正なら既定 (未選択) に倒し、
     ログに残す (起動を止めるほどの値ではない)。`save_full` は `settings_kv` を全部書き直すので、
     次の保存で旧キーの行が消える。
  2. 設定の取り込み (`settings_transfer::parse_items`)。v5.1.0 で書き出した設定ファイルに
     `thumb_container_type_icon_only` があれば同じ規則で新キーとして受け付ける (件数は 1 件、
     不明な項目には数えない)。旧値が不正なら、既存の契約どおり指摘 1 件 (`TransferIssue`) にして
     取り込み先の値を変えない。書き出しは新キーだけを出す。
  3. `--settings-override` (`settings_override.rs`)。overlay は現在の設定へマージしてから Deserialize するが、
     マージは Settings に無いキーを `UnknownSetting` で拒否するので、旧キーは Deserialize まで届かない
     (設計レビュー r1 P2)。**マージの前に overlay の map を同じ関数で変換する**。overlay 内で新キーが勝ち、
     旧キーだけなら現在の選択を ALL / NONE で上書きする。旧値が不正なら override を拒否する
     (試験の構成を黙って変えない、という override の既存方針どおり)。
- Remote の一覧設定 (`REMOTE_LISTING_SETTINGS_SQL`) はこの設定を読まない (§2.2 のとおり Remote は対象外)。
  変更なし。Remote の設定転送の方針 (環境設定にある項目だけを転送する) にも、新しい項目は追加しない。
- 旧版へ戻した場合: 新しい版で保存した設定 DB は、v5.1.0 の既存の version gate が新しい版の DB として扱い、
  設定全体を既定値で起動して保存を抑止する (`settings_db.rs` の version gate)。本件で互換のための保存は足さない。

### 8.4 判定の形

- `grid_item::ContainerIconKind { Folder, Archive, PdfEpub }` と
  `grid_item::container_icon_kind(item) -> Option<ContainerIconKind>` が §8.2 の表を持つ唯一の場所。
- `ContainerIconKinds::contains(kind)`、`ContainerIconKinds::NONE` / `ALL`。
- `container_icon_only(item, kinds: ContainerIconKinds) -> bool` と
  `effective_thumbnail_state(item, thumb, kinds)` は、今の `enabled: bool` を `kinds` に置き換えるだけ。
  呼び出し元 (要求ループ、ドライブ一覧の要求、優先要求、idle 高画質化、描画・レイアウトの全箇所、
  補正済みテクスチャ、自動比率、スクロール計測、hover) は、`self.settings.thumb_container_type_icon_only` を
  `self.settings.thumb_container_icon_kinds` に置き換える。新しい分岐は足さない。
- テスト用の `true` は `ContainerIconKinds::ALL` に置き換える。

### 8.5 切り替え (環境設定の OK)

§4.5 を種類ごとにする。旧値 `old` と新値 `new` から「外した種類」`turned_off = old - new` を求める。

- 種類を足したとき: 何もしない (次のフレームから描画の置き換えと要求の停止が効く)。
- 種類を外したとき: 現在の一覧で `container_icon_only(item, turned_off)` が真かつ `NoArt` の idx だけを
  `Pending` に戻し、再描画を要求する。残っている種類 (例: 書庫は選んだまま実フォルダだけ外した) の
  `NoArt` は戻さない。戻した項目は通常の要求経路で、キャッシュ・固定から読み込まれる。
- 一覧の読み直し・現在地・選択・スクロール・要求 owner・キャッシュ・固定・退避中の viewer context の扱いは
  §4.5 と同じ。

### 8.6 環境設定の画面

`thumbnail/container-type-icon` の anchor を残し、チェックボックス 1 つを「見出し + 横並びの 3 つ」にする。

- 見出し: 「代表画像の代わりに種類のアイコンで表示する:」
- 横並び (`horizontal_wrapped`): 「実フォルダ」「書庫 (ZIP・RAR など)」「PDF・EPUB」
- 説明 (small): 「選んだ種類は代表画像を使わずアイコンで表示します。書庫の中のフォルダは「書庫」に従います。
  画像・動画・音声と、本を開いた後のページはそのままです。固定した代表画像やキャッシュは残り、
  外すと再び表示されます。」
- 高さは今の項目から 1 行増える程度。環境設定はページ内が ScrollArea なので 1093×614 でも操作できる。
  既存の環境設定のスナップショット (`preferences_favorite_view_state_dark`、620×260 の本文 fixture) を更新する。
  これはダイアログ全体の操作性までは見ないので、1093×614 の環境設定ダイアログ全体を描く既存の headless
  検査の形で、サムネイルページを開いて 3 つのチェックボックスへ到達でき (スクロール後に見える)、OK / キャンセルが
  見えることと、狭い幅で横並びが折り返すことを確かめるテストを足す (設計レビュー r1 P3)。
- 設定検索の索引の見出しを「代表画像の代わりに種類のアイコンで表示する」にし、キーワードに
  「実フォルダ」「フォルダ」「書庫」「PDF」「EPUB」を足す。
- 設定の書き出しのラベルは「種類のアイコンで表示する項目」。

### 8.7 テスト

- 判定の表: 全 `GridItem` の種類 × 8 通りの `ContainerIconKinds`。`ZipDir` (`is_archive` 真偽とも) と
  `SearchContainer { Zip }` は書庫だけで真、`SearchContainer { Folder }` とドライブの `Folder` は実フォルダだけで真。
- 要求経路: 実フォルダだけを選んだ混在一覧で、`Folder` / `SearchContainer{Folder}` は要求なし・`NoArt`、
  `ZipFile` / `PdfFile` / `ConvertibleArchive` / `ZipDir` / `SearchContainer{Zip}` / 画像は従来どおり要求される。
  書庫だけ・PDF/EPUB だけも同様に 1 本ずつ。
- 書庫の中: 実フォルダだけを選んで書庫を開くと、書庫内フォルダ (`ZipDir`) とページは代表・画像のまま。
- 切り替え: 全部選択 → 実フォルダだけ外す OK で、`Folder` の `NoArt` だけが `Pending` に戻り、書庫・PDF の
  `NoArt` は残る。キャッシュ行・固定行が残り、戻した `Folder` が `Loaded` になる。Cancel は何も変えない。
- 自動比率・スクロール計測: 実フォルダだけを選んだ一覧で、書庫の代表は母数・標本・完了条件に入る。
- 移行 (設定 DB): 旧キーだけの DB で `true` → `ALL`、`false` → `NONE`、どちらも無い → `NONE`、
  旧キーが bool でない → `NONE`、新旧両方 → 新キーが勝つ。保存後に旧キーの行が無いこと、
  他の設定 (列数・お気に入り) が残ること。ポータブルへコピーした DB でも同じ。
- 移行 (設定の取り込み): v5.1.0 形式の `{"thumb_container_type_icon_only": true}` で `ALL` (受付 1 件・
  不明 0 件・指摘なし)、`false` で `NONE`、新旧両方で新キーが勝つ、どちらも無ければ取り込み先を変えない。
  書き出しに旧キーが出ない。新キーの壊れた値は他の項目と同じ「正しくありません」の指摘になる。
- 部分選択の切り替え: `{実フォルダ, PDF/EPUB}` → `{書庫, PDF/EPUB}` を 1 回の OK で行うと、`Folder` だけが
  `Pending` に戻り、書庫は新たに抑止 (要求なし)、PDF は `NoArt` のまま。要求 owner と退避中の viewer context は
  変わらない (設計レビュー r1 P2)。
- 部分選択の保存: `{実フォルダ, PDF/EPUB}` などを DB へ保存・再読み込み、書き出し・取り込みで往復して同じ値。
- 新キーが壊れていて旧キーが正しい場合に、旧値へ倒さないこと (DB 読み込み・取り込み・override)。
- 旧 JSON (`settings.json` からの移行経路 `read_settings_json_for_migration` → `apply_load_time_migrations`) に
  旧キーだけがある場合の移行。
- `--settings-override`: 旧キーだけの overlay で現在の選択が ALL / NONE になる、新旧両方なら新キーが勝つ、
  旧値が不正なら拒否される。
- 既存の §1.697 のテストは `true` → `ALL` の置き換えで全部通ること。

### 8.8 §1.764 との関係

§1.764 (表紙範囲の切り出し、別ブランチ) は代表サムネイルの要求経路 (固定の解決・source_id・worker の
切り出し) を触る。本件は要求経路の中身を変えず、既存の判定呼び出しの引数を置き換えるだけにする。
後から入る側の merge で、`self.settings.thumb_container_type_icon_only` の行が近くにある場合の
単純な衝突が起こり得る。

### 8.9 検討して採らなかった案

- 3 つの bool を別々のキーにする: 移行で 3 キーを作る必要があり、部分的な取り込みで混在し得る。
- 旧キーを残して「旧キーが ON なら 3 種類とも ON」と毎回読む: 種類を外しても旧キーの ON が残り、
  次の読み込みで戻ってしまう (旧キーを書き換える処理が別に要る)。読み込み時に 1 回変換して旧キーを消す。
- 種類ごとの判定に「本として扱うフォルダか」を含める: §8.2 のとおり、判定が項目の種類だけで決まらなくなる。
- ドライブ・検索結果の集約行に専用の選択肢を作る: 要望の範囲外で、選択肢が増えるだけ。
- 種類を外したとき一覧を読み直す: §3 の理由で採らない (コレクションのルート・Everything)。

まれな失敗 (設定 DB の書き込み失敗など) は既定の割り切り方針に従い、追加の回復処理を設けない。

### 8.10 実装記録

- 2026-11-12: `ContainerIconKind` / `ContainerIconKinds` と唯一の対応表 `container_icon_kind` を
  `grid_item.rs` に追加。要求・描画・自動比率・スクロール・hover の既存判定は引数だけ置換し、
  要求の作成・owner・終端処理は変更しない。
- 旧 bool は共通の `canonicalize_container_icon_kinds` で map の段階から除去し、DB / 旧 JSON、
  設定取り込み、override のマージ前に変換。壊れた旧値は DB / JSON では既定＋ログ、
  取り込みでは指摘1件で宛先維持、override では拒否。新値があれば壊れていても旧値へ倒さない。
- OK で外した種類の `NoArt` だけを `Pending` に戻す。既存の一覧位置・要求 owner・
  キャッシュ・固定と退避 context は保持。3つの checkbox、検索索引、書き出しラベルと
  マニュアル・表示パイプライン文書を更新した。
- 回帰: 全 variant と8通りの選択、3種類別の混在一覧要求、書庫内の folder / page、
  部分選択時の自動比率・スクロール待ち、種類の解除／入替と sibling owner 保持、
  固定・cache からの復帰、出荷済み DB とコピーした portable DB、旧 JSON、
  取り込み／書き出し、override、1093×614 の実ダイアログと狭幅の折り返しを検査する。
- 検証結果は以下のとおり。全lib・build-dev は設計担当へ引き継ぐ（実装担当には実行しない指示）。
  製品バイナリ起動とコミットは行わない。
- 独立コードレビュー（GPT-6.1 Sol / xhigh、2026-11-12）: ACCEPT、P1/P2/P3 なし。
  種類表・3入口の移行・新値優先・解除範囲・owner / 退避 context 保持・回帰テスト範囲を確認。
  下記のテスト修正も限定再確認 ACCEPT。検査の弱体化なし。
- 初回 focused は574件中570成功・4失敗。追加した自動比率テストが既存750ms streakを
  待たない期待になっていたため、実 poll の候補と標本数を検証してから時計だけ進めた。
  環境設定の3件は同名「実フォルダ」の Label と Checkbox が曖昧だったため、
  role を Checkbox に限定。製品コードの修正・期待値の緩和はない。
- `cargo fmt`（引数なし）、`cargo fmt --check`、`git diff --check` 成功。
  `python scripts/check_ui_glyphs.py` は危険グリフ0件、exit 0。
  記録: `target/icon-kinds-{fmt-check-final,diff-check-final,glyphs}.log`。
- `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot preferences_favorite_view_state_dark`:
  1 passed、exit 0。更新 PNG は620×260、77,567 bytes。
  実装担当と独立 reviewer が実 PNG を開き、見出し・3 checkbox・説明文に欠け／重なりがないことを確認。
  記録: `target/icon-kinds-snapshot-update.log`。製品アプリの起動は含まない。
- 修正後 focused（既定 feature / test profile、未コミット差分）:
  `cargo nextest run -p mimageviewer --lib -E 'test(/container_type_icon|container_icon|container_icon_scroll|auto_aspect|thumbnail_request_lifecycle|settings_transfer::tests|settings_db|same_name_folder_book|thumbnail_type_icon|preferences/)'`
  は574 passed / 0 failed（1 slow）、exit 0。記録: `target/icon-kinds-focused-final.log`。
- 更新指定なしの `cargo test --test ui_snapshot`: 249 passed / 0 failed / 0 ignored、exit 0。
  PNG 差分は `tests/snapshots/preferences_favorite_view_state_dark.png` だけ。
  記録: `target/icon-kinds-snapshot-full.log`。
- 設計からの挙動変更はない。実装記録を指定の §8.10 とするため、既存の設計レビュー履歴は §8.11 へ移した。

- 全件 (設計担当、2026-11-12、9e7c3a97b、`test-lib-full.ps1` run `20261111T154724649Z-a032f613`):
  17,218 件中 17,215 成功・3 失敗・75 skipped (995 秒)。失敗は
  `close_return_remote_acquire_and_return_never_resurrects_main_terminal_request`、
  `section1375_large_physical_scan_10k_100k` (全件中 368 秒)、
  `page_turn_mainwindow_submits_visible_vacant_leaf_both_directions` で、本件の変更箇所と関係がなく、
  3 件とも単独の再実行で成功した (全件実行の負荷による失敗、[§1.765](next-release-backlog.md) と同じ型)。

### 8.11 設計レビューの記録

- r1 (GPT-6.1 Sol / xhigh、2026-11-11): REVISE、P1 なし。P2 `--settings-override` はマージ前に未知キーを拒否するので
  旧キーが変換に届かない → マージ前に overlay を変換 (§8.3)。P2 取り込みで不正な旧値を黙って捨てると既存の
  「指摘 1 件・取り込み先を維持」の契約に反する → 入口ごとの扱いを決めた (§8.3)。P2 部分選択の受け入れテスト不足 →
  §8.7 に追記。P3 旧版へ戻した場合の説明が version gate と違う → 訂正 (§8.3)。P3 1093×614 のダイアログ全体の
  検査 → §8.6 に追記。対応表・引数置換で判定の一致が保たれること・`old - new` の差集合・Remote に変更が
  要らないことは妥当と確認された。

- r2 (同、resume): ACCEPT。r1 の指摘の反映を確認、追加指摘なし。
- 実装レビュー (同じ独立担当、実装担当とは別、2026-11-12、`54a701ef0..9e7c3a97b`): ACCEPT、P1/P2/P3 なし。
  置換漏れ・`ALL` 固定なし、差集合・3 入口の変換・serde 形と DB/取り込みの整合、旧テストの OK/Cancel/保存/再表示の
  検査の保持、UI・マニュアル・PNG の変化が意図どおりであることを確認。
