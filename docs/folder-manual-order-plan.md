# フォルダ手動順 (§1.911) 設計

- 状態: 段 1〜4 の実装と段 5 の文書整備済み (2026-11-27)。停止した操作面・検証範囲は各段の実装記録を参照。設計は独立設計レビュー R1〜R4 (gpt-6.1-sol / xhigh) の指摘と利用者の回答を反映した第 6 版 (2026-11-26) を起点とする。利用者確認 Q1〜Q6 は回答済み (§12、Q2・Q5 は推奨から変更)。
- ブランチ `folder-order-911` (worktree `G:\home\mimageviewer-folderorder911`)。
- 収録: v5.9.0 に間に合えば入れ、間に合わなければその後の版 (利用者決定 2026-11-26)。
- 起点: [next-release-backlog.md §1.911](next-release-backlog.md)。利用者決定 1〜7 はそちらが正本。
- 担当: 設計・検収 = ClaudeCode Opus、実装 = Codex gpt-6.1-sol/high、独立レビュー = 別の gpt-6.1-sol/xhigh。
- まれな失敗 (DB の書き込み失敗・異常・書き込み途中の強制終了) は既定の割り切り (ログ + 通知。読めない記録は原本を残して
  基準の並びで表示し、そのフォルダの手動順の書き込みを止める)。新しいストアは未リリースなので移行コードは不要。

## 0. 利用者決定の要約 (backlog §1.911 から)

1. 順番は mIV の DB に保存する。フォルダへファイルを書かない。
2. mIV 上の改名だけ追従する。外部で改名・追加されたファイルは未知として末尾。消えたファイルは飛ばす。
3. 手動順のとき、一覧でのドラッグは並べ替え。その間、一覧からのドラッグでのコピー・移動はできなくてよい。
   手動順以外では従来どおりのドラッグ。
4. 手動順に切り替えたときの初期の並びは直前の並び順。
5. 一度も並べ替えていない手動順は記録しない。最初の並べ替えが成功した時点で記録する。
6. 記録したフォルダで別の並び順へ切り替えてから手動順に戻すと記録した順に戻る。記録は消さない。
7. 記録を捨てて直前の並び順からやり直す操作 (リセット) が要る。

範囲外 (V1): ZIP / PDF / EPUB などの仮想フォルダ、本として開いたページ順、検索結果・スマートフォルダ・
コレクション・本棚・サブフォルダ展開・評価/タグ/しおりビュー、Remote からの並べ替え・手動の切り替え。

## 1. 今のコードの事実 (設計の前提)

- 並び順 `SortOrder` (src/settings.rs) は全体共通の 1 つ (`settings_kv` の `sort_order`)。フォルダごとの記憶は無い。
  お気に入りごとの表示状態 (`FavoriteViewState`、`remember_favorite_view_state`、既定 OFF) が有効なら、
  その値が `settings.sort_order` へ重ねられる (overlay)。並び順の操作面はメニュー・ドロップダウン (`draw_grid_sort_dropdown`)・
  ツールバーの並び順ボタン・ゲームパッドのリング (`apply_grid_picker_sort_order`) がある。
- フォルダ一覧の並びは `materialize_local_folder_listing_with_order_and_metadata` (src/app/book_materialization.rs) が合流点。
  PC 本体・Remote (`remote_ipc/container.rs`)・Agent IPC (`agent_ipc/folder.rs`) が同じ関数を使う。種別の投影
  (表示しない種類) と重複除外 (同名のフォルダ / 書庫、動画側の画像など) は `items` を作る前に行う。
  `GridDisplayOrder` は 4 種別 (実フォルダ / アーカイブ類 / 画像 / 動画・音声) を 4 つの「行」へ割り当てる設定で、
  **既定は「フォルダ + 書庫」「画像 + 動画・音声」の 2 行**。同じ行の中は混ぜて `SortOrder` で並べる。この「行」は並び順のまとまり
  (先に出す種別の順) で、**一覧の描画は改行しない** (セルの位置は通し番号 `position % cols` / `position / cols`、ui_main.rs の
  render_grid)。行の境目は見た目に区切られていない。
  本として扱うフォルダは本のページ順 (Numeric / FileName) が優先される。
- `ListingOrderRequest` (src/rating_sort.rs) は Standard / Rating の共有型で、物理フォルダ以外の一覧でも使う。
- 一覧を作る producer は複数ある: 通常の open (`start_folder_open_scan` の worker で scan、並べ替えは UI スレッドの
  `prepare_scanned_folder_listing_with_cache_keys`)、直接パスの分類 worker、DFS の事前走査、起動時の resolver
  (`FolderScanPolicy::scan`)、watcher の再走査 (`start_external_rescan_with_cause`、worker で並べ替えまで行い、
  結果が同じなら軽い観測更新・違えば一覧交換)、並び順の変更 (`apply_sort_change_reload`)、Remote、Agent IPC。
- 一覧は `App.items` と同じ添字の `image_metas`・`thumbnails`・選択・checked・page identity などが組で、
  一覧交換は items generation の更新と古い非同期結果の拒否を伴う。一覧は `ViewerContextBundle` と共に交換・fork される。
- 一覧の中のファイル送り・スライドショーは `App.items` + `current_reader_order()` に従う。本送りには凍結した
  `ListOrderSnapshot` (list_order_navigation.rs) がある。詳細表示では列ヘッダの並びが独立して一覧と読書の順を持つ。
- ファイル名スタック (`filename_stack.rs`) は自分で並べ直す。スタックの ON/OFF は transient (フォルダを出ると解除)。
- 一覧のドラッグは `dispatch_grid_cell_input` (src/ui_main.rs) で primary・press owner・touch 除外・
  `native_drag_just_finished`・右ドラッグ抑止を通って `drag_started` を判定し、`pending_native_drag` を立てて描画後に
  `SHDoDragDrop` (モーダル) へ入る。左ドラッグの範囲選択は無い。タッチの移動は一覧のスクロール所有者。
  Explorer からのドロップは別経路 (OLE target) で、現在フォルダか、ツリー / 一覧のフォルダへのコピー。
- 並べ替えのドラッグは「本のページの並べ替え」窓と「コレクションの並べ替え」窓にほぼ同じ実装がある
  (挿入位置 = セルの左右半分、縦棒の挿入線、端での自動スクロール、選択グループの移動)。共通部品は無い。
- 改名追従は `rename_key_migration.rs` の `STORES` (1 行 = 1 つの path キー列)。mIV 上の改名 (F2 / 右クリック) と
  管理本の構造操作が移行を呼ぶ。mIV 上の移動・コピー・貼り付け・D&D は移行しない (既存の方針)。通常の改名の衝突規則は
  `DestinationWins`。削除時は同じ表で purge し、孤児整理ダイアログも同じ表を走査する (「実体が無く親が在る」行だけ孤児)。
  content-identity の復元は unique store を候補探索 (`ledger_keys_with_restorable_rows_at`) と複写 (`copy_stores_impl`) に使う。
  移行の開始は `rename_migration_writers_busy` で書き込み worker が空になるまで待つ (旧キーの書き込みが移行後に復活しないため)。
- 本棚の本の順 ([book-order-plan.md](book-order-plan.md)) は棚の中の JSON ファイル。今回は決定 1 によりフォルダへ
  書かないので形式は流用せず、「未知は末尾・名前順」「読むだけでは書かない」の規則だけ揃える。

## 2. 「手動順を選んでいる」状態の持ち方 — 推奨: フォルダごと (利用者確認 Q1)

| | A: 全体の並び順の 1 つ (`SortOrder::Manual`) | B: フォルダごと (推奨) |
| --- | --- | --- |
| 手動順を選んだ後に別のフォルダを開く | そのフォルダも手動順。記録が無ければ直前の並び順で表示し、ドラッグは並べ替え (最初のドラッグで記録が始まる) | 普段の並び順で表示。ドラッグは従来どおり |
| ドラッグが並べ替えになる範囲 | 手動順を選んでいる間の**全フォルダ** | 手動順を選んだフォルダ / 記録が有効なフォルダ**だけ** |
| 実装の波及 | `SortOrder` の全 match、旧版が新しい値を読めない非互換 (評価順追加時と同じ) | 手動の有効状態を別に持ち、全操作面・お気に入り・Remote の表示へ配線する |

B を推奨する理由は、決定 3 のオプトイン (ドラッグがコピー・移動でなくなる) を利用者が手動順を選んだフォルダに閉じ込められること。
A では手動順を選んだまま別のフォルダでファイルを Explorer へドラッグしようとすると並べ替えになり、そのフォルダの記録も始まる。
B も既存の並び順から独立ではない (全操作面・お気に入り・Remote へ配線が要る) が、旧版との設定の非互換は生じない。

B の規則 (「普段の並び順」= お気に入りの表示状態が効けばその値、無ければ全体の並び順):

- フォルダの状態: **通常** / **手動・未記録** (切り替えただけ。メモリだけ) / **手動・記録あり** (DB に順と有効の印)。
  記録があって有効の印が外れているものは表示上 通常。
- 通常 → 手動: 記録があれば記録した順に戻し、**有効の印を保存する** (記録が既にあるので決定 5 の「記録しない」には当たらない)。
  記録が無ければ、今表示している並び (決定 4) で手動・未記録になり、DB に何も書かない (決定 5)。
- 手動・未記録のまま別のフォルダへ移ると、その選択は消える (セッション内でも覚えない)。
- 手動 → 別の並び順を選ぶ: 既存の並び順変更の handler をそのまま通し (全体またはお気に入りの値が変わる)、このフォルダを
  通常に戻す。記録があれば有効の印だけ外す (決定 6)。
- 手動・記録ありのフォルダは、開き直しても・再起動しても手動順で開く。お気に入りの表示状態より手動が勝つ。
- 切り替えはすべての操作面から**共通の handler 1 つ** (手動の有効状態の所有者) を通す。同じ値の再選択を no-op にする既存経路
  (`sort_control_decision`、ゲームパッドの no-op 判定) も、手動のときは「別の並び順を選んだ」として扱う。

## 3. 保存先とキー

新しい DB ファイル `folder_order.db` (data_dir)。

```sql
CREATE TABLE folder_order_items (
  item_key TEXT PRIMARY KEY,   -- 子の完全 path、normalize_keep_drive (tags.db と同じ)
  position INTEGER NOT NULL
);
CREATE TABLE folder_order_folders (
  folder_key TEXT PRIMARY KEY, -- フォルダの完全 path、normalize_keep_drive
  active INTEGER NOT NULL,     -- 1 = 手動順が有効 (決定 6 の印)
  revision INTEGER NOT NULL
);
PRAGMA user_version = 1;
```

- items にフォルダの列を持たない (`STORES` は 1 行 1 列なので、2 つの path 列はフォルダ改名で片方が古く残る)。フォルダの子は
  `item_key` の範囲で引き、直下の子だけを Rust 側で残す。範囲の起点は「フォルダのキーに区切りを**一度だけ**付ける」共通 helper で作る
  (ドライブルート `c:/`・UNC 共有ルート・末尾区切り付きの入力で二重の `/` にならない)。読み込み・保存・リセットで同じ helper を使う。
- `STORES` に 2 行足す: `folder_order.db / folder_order_items / item_key` と `folder_order.db / folder_order_folders / folder_key`
  (KeepDrive、unique)。この表に乗るのは**削除の purge だけ**。
  - **改名は汎用の移行に乗せず** (`rename_generic = false`)、actor が自分の FIFO の中で行う (§4.1、R3 指摘 1)。通常の移行・journal・
    起動時の再試行は `rename_generic` で選ぶので、これで外れる。
  - **管理本の path 保守 (`migrate_book_paths_at`) からは明示的に外す**。この経路は `rename_generic` ではなく `book_policy` で対象を選び、
    false の descriptor を reading_history とみなす SQL もあるので (R4 指摘 2)、除外の policy を新しく足し、DB を開く前に除外する。
    本棚は範囲外で、本はページ順が固定のため eligible でない。
  - **孤児整理の対象から外す** (R4 指摘 1)。改名を actor に積んだ後・実行前に孤児整理が走ると、旧フォルダの行を孤児として消して
    記録を失う。孤児の行は表示も位置も変えず、そのフォルダの次の保存 (inventory に無い子を消す) と削除の purge で消えるので、
    孤児整理で消さなくてよい。
- **改名の規則は「移行元の状態を移し、移行先の古い記録は捨てる」** (R4 指摘 3)。外部で消えた `b.jpg` の行が残っているところへ
  `a.jpg` を `b.jpg` に改名したとき、a に位置があれば b はその位置、a に位置が無ければ b は未知 (古い b の位置を継がない)。
  フォルダの改名 (Tree) も同じで、移行先のフォルダとその配下の古い行を消してから、移行元のフォルダの行と配下の行を移す。
  actor の改名とメモリの記録の書き換えは同じ規則・同じ helper を使う。
- **複写の対象から外す**: content-identity の候補探索と複写の両方 (同じ「複写できる」宣言を共有する)、`copies_with_book_page` も false。
  位置は元のフォルダの中の値で、別のフォルダへ複写すると移動先の順に混ざる。移動したファイルは移動先で未知 = 末尾。
  `metadata_transfer` は明示形式、`image_folder_zip_metadata` は許可リストなので、足すだけでは複写されない (R1 で確認)。
- **保存する全順の集合** (R2 指摘 3):
  - 一覧に載っている子 (`items` 全部。ファセット等の絞り込みで見えていないものも含む) は、見えている子の新しい順を
    絞り込み前の全順へ合成した順 (`collection_visible_order::merge_visible_order` と同じ考え方)。初回の並べ替えも同じ。
  - 一覧に載っていないがディレクトリには在る子 (種別を表示しない・重複除外で `items` を作る前に外れたもの) は、
    **記録に位置があればその相対位置のまま残し、記録に無ければ位置を持たせない** (後で表示されたときは未知 = 末尾・名前順)。
  - ディレクトリに無い子の行は、その保存で消す。
  - 「ディレクトリに在るか」は、scan の worker が `read_dir` の直下の名前 (投影・除外の前) を集めた **inventory** で判断する。
    今の `ScannedDir` は投影後なので、inventory は scan の結果に別に載せ、一覧側の状態 (§4.2) が記録と組で持つ。
  - mIV 上の削除・改名が成功したら、一覧の `items` を直すのと同じ境界で、記録と inventory からもその子を外す / 書き換える。
    古い inventory や記録が、purge 済みの位置を次の保存で復活させないため。
- キーは小文字化されるので、大文字小文字だけの改名は移行が何もしないが、キーが同じなので位置は保たれる。

## 4. 記録の所有者と読み書きの順序

### 4.1 書き込みと読み込みを 1 つの直列 actor に集約する

- `folder_order.db` は本体プロセスでは専用の actor thread 1 本だけが開く。**保存 (全順)・有効の印の変更・リセット・読み込み**を
  すべて同じ FIFO で処理する。同じフォルダへの連続した保存は、まだ実行していないものを最新 1 件へまとめてよい
  (まとめるのは同じフォルダの同じ種類の要求の間だけ)。別のフォルダの要求は捨てない。
- 読み込みも同じ FIFO を通るので、「A の保存が終わる前に A を開き直すと古い DB を読む」ことが起きない。
  一覧の producer (worker thread) は actor へ読み込みを頼み、返事を待つ (UI スレッドは待たない)。
- 完了の通知は folder_key と revision を持ち、App は今の一覧の記録と照合して古い完了を無視する。
- **改名は actor の FIFO の要求にする** (R2 指摘 1・R3 指摘 1)。mIV 上の改名 (ファイル = Exact、フォルダ = Tree) の成功を App が
  採用する境界 (`poll_rename_pending`) で、(1) 今の一覧の記録を新しいキーへ書き換え、(2) 同じ対応表で actor へ「改名」を送る。
  この 2 つより後にしか新しいキーの保存・読み込みは作られないので、FIFO の順だけで「旧キーの保存 → 改名 → 新キーの読み書き」になる。
  汎用の移行 worker・その待ち (`rename_migration_writers_busy`)・journal とは独立にする (汎用の移行がほかの理由で待たされても、
  フォルダを開くときの読み込みは止まらない)。改名の失敗は既定の割り切り (ログ + 通知。記録の位置が末尾へ行くだけ)。
  2 段構えの一時停止 (保守 worker が actor を止める方式) は R3 で開始時点の競合が見つかったため採らない。
- 削除の purge (delete worker) は STORES の表から DB を直接書き、actor とは順序を揃えない。purge が消すのは実際に削除した path の行だけで、
  前後して起き得るのは「古い保存や改名が、削除済みの path の行を書き戻す」ことだけ。その行は resolver が飛ばし、次の保存で消える。
  有効な記録を消す組み合わせは無い (孤児整理は上のとおり対象外)。
- 本体プロセスで `folder_order.db` を書くのは actor と、削除の purge だけ。
- Remote の通常フォルダ一覧は本体プロセスの IPC worker (`ContainerEngine`、web-remote-plan.md §2.1) が作るので、Agent IPC と同じく actor を通して worker で読む (SharedReaders 経由)。DB を直接開く第 2 の読み手は作らない (段 2 の前提確認で訂正、2026-11-26)。
  Agent IPC は本体プロセス内なので actor を通す。

### 4.2 一覧側の状態

- 手動の状態は**一覧 (viewer context) が所有する型**にする: `{ folder identity, mode: 通常 | 手動・未記録 | 手動・記録あり,
  input: 対象外 | 読んだ, record, inventory, order_revision }`。表示中の一覧は必ず `読んだ` か `対象外` (読み込み中の仮一覧は作らない)。
  一覧と共に `ViewerContextBundle` で交換・fork される。別の一覧 (fork した読み手) は自分の写しを持ち、その一覧が読み直すまで
  元の順を保つ。並べ替えのドラッグは主ウィンドウの一覧でだけ行う (detached viewer に一覧のドラッグは無い)。
  detached の経路に新しい分岐は足さない ([detached-rework-plan.md](detached-rework-plan.md) の凍結ルール)。
- `order_revision` (R2 指摘 5) は、その一覧の**有効な並びを変える操作すべて**で 1 進める: 手動への切り替え・通常への切り替え・
  並べ替え・リセット・改名によるキーの書き換え・削除による除外・記録の読み込み完了。巻き戻さない (リセットでも進める)。
  context ごとの単調な番号で、別のフォルダを新しく開くときは移動先の番号を移動元と比べない。
- 一覧を作る要求 (再走査・並び順の refresh を含む) は既存の照合 (context・folder・open owner・items generation) に加えて
  `order_revision` を持ち、完了時に今の値と違えば採用しない (作り直す)。並び順の refresh の採用条件は今は `SortOrder` の照合だけ
  (`app.rs` の refresh 採用) なので、同じ `SortOrder` のまま手動の状態が変わる場合をこの番号で見分ける。未確定のドラッグも、
  開始時の番号と違えば取り消す。手動の状態は一覧の投影キー (`app.rs` の projection key) にも含める。
- mIV 上の改名の完了時に、今の一覧の記録のキーを移行元優先の規則で書き換える。削除の成功時は記録と inventory から外す (§3)。

### 4.3 producer ごとの受け渡し

| producer | 記録の入手 |
| --- | --- |
| 通常の open / 直接パスの分類 / 起動時の resolver / DFS の事前走査 / 物理履歴の preflight (`collection_navigation.rs`) | その worker が actor から読む。結果に「記録あり / 記録なし」と inventory を載せる |
| 同期の `load_folder` 経路 (サブフォルダ展開を終えて戻る等。UI スレッドの scan fallback を含む) | 下の「記録のあるフォルダの索引」で分ける (2026-11-27 改訂)。索引が準備済みで移動先が索引に無ければ、**今の同期契約のまま** (`Loaded` を返し、その場で元の状態を破棄する) 開き、入力は「読んだ: 記録なし」とする。索引に有る、または索引が準備中なら、次の非同期採用へ回す。UI スレッドで actor を待たない。非同期採用へ回る経路だけ、既存の非同期 open (Direct / `MainListNavigation`) の成功採用へつなぎ、actor の読み込みが返ってから一覧を**一度だけ**採用する。元の状態の破棄 (サブフォルダ展開の終了なら snapshot の破棄と surface を Folder へ戻すこと、`subfolder_expansion.rs`) は**採用側へ移す** (R4 指摘 4。今は `load_folder` の前に破棄しているため、採用だけを遅らせると旧 items と状態が食い違う)。範囲外の一覧へ入る同期経路の契約は変えない。実装の段 2 の最初に同期の呼び出し元を全部挙げ、通常フォルダへ入るものを表にしてから変える |
| watcher の再走査・並び順の refresh (同じ一覧の作り直し) | DB を読まず、今の一覧の記録 (と revision) を渡す |
| Agent IPC | 自分の worker で actor から読む |
| Remote (本体プロセスの IPC worker) | actor から読む (SharedReaders 経由) |

#### 記録のあるフォルダの索引 (2026-11-27 改訂)

- 経緯: 段 2 の実装で、同期の `load_folder` 経路を一律に非同期採用へ回したところ、手動順を使っていないフォルダでも
  `load_folder_or_convert_archive` が `Loaded` でなく `Classifying` を返し、サブフォルダ展開を抜けるボタンがその場で抜けなくなった
  (既存テスト `subfolder_expansion_button_exits_immediately_when_view_is_on`・`staged_smart_history_root_commits_once_and_does_not_suppress_next_navigation`
  が失敗)。記録のあるフォルダはごく一部なので、記録の無いフォルダの同期経路は今の契約のまま残し、扱う組み合わせを
  「手動順のフォルダへ同期経路で入る」場合だけに減らす (CLAUDE.md「状態の組み合わせを減らす」)。
- 内容: `folder_order_folders` に行がある folder_key の集合 (有効・無効を問わない) と、準備済みの印。本体プロセスに 1 つ、
  actor の handle と共有する。UI スレッドは集合を見るだけで、DB を開かない。
- 起動中は**追記だけ**で、消さない (R7 指摘 1)。Reset・改名・削除の purge の後も key は残り、そのフォルダは再起動まで非同期採用へ回る。
  消すと、`Reset(F)` の完了が後に積まれた `Save(F)` の追加を消すなど、後続の要求との調停が要る。追記だけなら調停が無い。
- 追加は**要求の投入時** (handle が積むとき): Save は対象フォルダの key。改名は Exact・Tree とも、バッチ改名の対応規則 (段 1 の
  `rewrite_key_with_mappings`) で、集合にある移行元の key (Tree なら配下の key も) から生まれる移行先の key をすべて足す (R7 指摘 2)。
  移行元の key は消さない。
- 準備: actor は DB を開いた直後 (どの要求より先) に全 folder_key を読む。準備前に投入された改名は、集合とは別に投入順の記録へ積んでおく。
  actor は「読んだ key」に準備前の改名を投入順に当てはめ (移行先を足し、移行元を残す)、準備前に足された key と和集合にしてから、
  準備済みを公開する。準備前の同期経路は非同期採用へ回す。
- これで集合は常に「DB に行がある key と、投入済みで未実行の要求が作る key」の上位集合になる。まれな失敗 (Save の書き込み失敗など) は
  既定の割り切りで、集合が余分を持つだけ (取りこぼしにはならない)。
- 同期経路の入力「読んだ: 記録なし」は、集合に無い = DB に行が無いことから導く (worker の Read と同じ型・同じ意味)。
  集合にある (記録が無効なものを含む) フォルダは非同期採用で中身を読むので、手動への切り替えで記録を戻す規則 (§2) は変わらない。
- 集合を使うのは同期経路の分岐だけ。worker 側の producer (通常の open 等) は今までどおり actor から読む (もともと非同期で、
  読み込みを省いても利用者から見える差が無い。省くと集合の正しさに依存する経路が増える)。
- 同期契約を守る範囲 (R7 指摘 3): **索引が準備済みで、移動先が索引に無い**ときだけ、今の同期契約 (即時 `Loaded`・その場での状態破棄・
  履歴の確定) のまま開く。それ以外 (起動直後の準備中、この起動の間に記録があったフォルダ (リセット・削除したものを含む)、
  起動時に記録があったフォルダ) は、記録の無いフォルダでも非同期採用になり、記録を読み終えてから一覧が切り替わる (通常は一瞬)。§8 の 9。
- テスト (R7 指摘 4): 非同期採用を確かめるテストは移動先に実際の記録を作る。既存の即時性テストは索引の準備完了を明示してから操作する
  (待ち時間による偶然の準備完了に頼らない)。

- listing への入力は「対象外 / 読んだ: 記録なし / 読んだ: 記録あり」の型で渡す。materialize は DB を補読しない。
  eligible な物理フォルダの listing を、記録を読まずに作る経路があれば不具合として扱う (型で作れないようにし、テストでも検出する)。
- 手動の入力は `ListingOrderRequest` に混ぜず、物理フォルダの listing 専用の別引数にする (対象外の一覧へ波及させない)。

## 5. 並びの決め方 (resolver)

1. まず従来どおり普段の並び順で並べる (投影・重複除外・種別の行の割り当ては今のまま)。これを「基準の並び」と呼ぶ。
2. 手動・記録ありのときは、**一覧全体を 1 列として** (種別の行を区別せず、利用者決定 Q2)、記録にある子を記録の位置順に先に出し、
   記録に無い子 (未知) を後ろへ**基準の並びの順**で並べる。記録にあって一覧に無い子は飛ばす。種別の行のどれにも入らない子
   (今は末尾に置かれるもの) も同じ 1 列に含める。
   - 未知の並びを backlog の案 (名前順) から基準の並びの順に変えた: 種別を区別しない 1 列では、名前順にすると新しいフォルダが
     新しい画像の間に入る。基準の並びの順なら、決定 4 (手動順の初期の並び = 直前の並び順) と同じ規則 1 つで済む。
   - 行は見た目に区切られていないので、行をまたぐ並べ替えで表示の区切りは変わらない。並べ替えた後は、フォルダが画像の間に
     来るなど、種別のまとまりは崩れてよい (利用者が選んだ並び)。
   - `GridDisplayOrder` の設定を後で変えても、記録済みの子の順は変わらない (未知の子の並びだけが変わる)。
3. 手動・未記録のときは 1 の結果のまま (= 直前の並び順)。
4. 読むだけでは書かない。未知の子は、次にそのフォルダで並べ替えたときに記録へ入る。
5. **構造的な対象判定 (eligibility) を 1 つの述語**にする (R2 指摘 6): 「通常の物理フォルダの一覧で、本のページ順が固定されていない」
   (`physical_page_order_locked` の条件。自動全画面設定と内容でも成立する)。PC・Remote・Agent IPC が同じ述語を使う。
   ここから次を導く:
   - **選択できる** = eligible。スタック表示中でも選べ、選ぶとスタックを OFF にする遷移になる。
   - **基礎の並び (`items`) に適用する** = eligible かつ 手動 かつ スタック表示でない。詳細表示の列ヘッダの並びの有無によらない
     (R3 指摘 2)。列ヘッダの並びは今のまま `details_order` で詳細表示の最終表示を上書きするだけなので、詳細表示とサムネイル一覧を
     切り替えても再走査は要らず、サムネイル一覧へ戻れば手動順が見える。手動を選んでも列ヘッダの並び (全フォルダ共通の
     `details_sort_key`) は解除しない (R3 指摘 4)。
   - **ドラッグで並べ替えられる** = 基礎に適用する かつ サムネイル一覧。
   - eligible でなくなった一覧 (内容・設定で本のページ順になった等) では、記録と有効の印を残したまま適用しない。

## 6. 一覧での並べ替えのドラッグ

- 対象: 手動 (未記録・記録あり) のフォルダの**サムネイル一覧**。詳細表示は §7 の Q4。
- `dispatch_grid_cell_input` で既存の開始条件 (primary・press owner・touch 除外・`native_drag_just_finished`・右ドラッグ抑止) を
  そのまま通した後で分岐し、手動のときは `pending_native_drag` を立てず並べ替えのドラッグを始める。手動以外では今のコードを
  そのまま通る (決定 3 の「戻る」ことを handler-level のテストで確かめる)。
- 開始時に、動かす子の identity・選択集合・viewer context・items generation を固定する。ドラッグ中にフォルダ・表示形式・
  フィルタ・一覧の世代が変わったら、未確定のドラッグを取り消す (何も動かさない)。
- 掴んだ子が選択 (`checked`) に含まれれば、**選択全体**を動かす (利用者決定 Q5。種別の行をまたいだ選択も全部)。動かす子どうしの
  相対順は今の並びのまま保ち、挿入位置へ連続して置く。含まれなければ掴んだ 1 件を動かす。フィルタで見えていない選択済みの子は
  動かさない (見えている子だけが対象。既存の窓と同じ)。
- 挿入位置はセルの左右半分で前/後ろ。挿入線は既存の窓と同じ縦棒。一覧のどのセルの前後にも入れられる (種別の行で制限しない)。
  一覧の外で離せば何もしない。
- 端での自動スクロールは一覧の外部スクロール所有者を通し、オフセットは行の高さ (`geometry.row_h`) の整数倍を守る。小さな pixel の
  移動量を毎フレーム丸めて 0 にしないよう、端数をドラッグ状態に貯めて 1 行分になったら進める。
- 離した位置が元と同じなら何もしない (記録も始めない)。動いたときは:
  1. 一覧の記録を新しい順にし、revision を上げる (最初の成功で手動・記録ありになる、決定 5)。
  2. 保存を actor へ送る (§3 の全順の集合)。
  3. 表示は**並び順を変えたときと同じ refresh 経路** (`apply_sort_change_reload` → `CurrentViewOrderRefresh` の再走査 → 既存の一覧交換)
     で作り直す (R2 指摘 2)。items をその場で並べ替えない。今の通常一覧をディスクなしで作り直す完成済みの経路は無く、
     一覧には同じ添字の状態が多数あるため (`ViewerContextBundle` の items 系の field)、新しい置換え経路を作るより既存の経路に揃える。
     再走査で inventory も新しくなる。選択・checked・今の項目は identity で復元し (並び順の変更で復元していないものがあれば
     ここで足す)、古い非同期結果は世代で拒否する。
     費用: 並べ替えるたびにフォルダの読み直しを待ち、サムネイルがキャッシュから描き直される (Q6 で利用者に示す)。
     実測で重ければ、ディスクなしの作り直しを別項目で検討する。
- 並べ替えのドラッグ中はカーソル形状を変えてよい (決定 3 のオプトイン)。Esc での取り消しは既存の窓に無いので足さない
  (一覧の外で離せば何もしない)。
- 共通化: 本・コレクションの窓の挿入位置・自動スクロール・グループ移動の重複関数を、今回の一覧と 3 か所で使う共通 helper へ
  まとめるかは実装の最初の段で判断する (まとめるなら挙動不変のリファクタとして別コミット)。

## 7. 切り替えとリセットの UI

- 「ソート順」メニュー・並び順のドロップダウン・ツールバーの並び順ボタン・ゲームパッドのリングに「手動順 (このフォルダ)」を足し、
  §2 の共通 handler を通す。手動のときはこれが選択状態、ほかの並び順は非選択に見せる。
- 「手動順をリセット…」をメニューとドロップダウンに足す。記録があるフォルダでだけ有効。**確認あり** (利用者が作った順が消えるため)。
  実行すると記録 (items と folders の行) を actor で消し、そのフォルダは手動・未記録として**今の普段の並び順から**やり直す (決定 7)。
- eligible でない場所 (§0 の範囲外、§5 の述語が偽) では「手動順」を無効表示にする。判定は `listing_sort_capabilities` に乗せる。
- ファイル名スタックの表示中は手動順を適用しない (利用者確認 Q3)。スタックを ON にしたら手動の表示を外し (記録・有効の印は残す)、
  スタック中に手動を選ぶとスタックを OFF にして手動を適用する。スタックの ON/OFF は transient なので DB には書かない。
- 詳細表示 (利用者確認 Q4、推奨): 列ヘッダの並びを使っていないときは手動順で表示し、列ヘッダの並びを選んでいる間は詳細表示では
  そちらが優先する (今の詳細表示の挙動のまま。列ヘッダの並びは全フォルダ共通の設定なので、手動を選んでも外さない)。
  詳細表示のドラッグは従来どおり。
- キー操作: `KeyAction` に「手動順に切り替え」を既定キー無しで足す (keymap の 6 点を揃える)。並べ替え自体のキー操作
  (選択を前へ/後ろへ) は V1 では足さない。
- Remote: 手動のフォルダは同じ resolver で同じ順に見える。並び順の状態は既存の `RemoteGridSortState` の形のまま
  `selected: "manual"`・それだけを含む `options`・`locked_reason` を返す (コレクションの手動順と同じ表現。既存の Web はこれを表示でき、
  書き込みもしない)。新しい IPC の field / enum は足さず、protocol の版 (今は 74) は上げない。書き込み用の `SortOrder` にも
  `manual` を足さない。Remote からの並び順変更は手動のフォルダでは理由付きで拒否する (`physical_page_order_locked` と同じ拒否の経路)。

## 8. 手動順のフォルダで今の使い方から変わること (利用者確認 Q6)

手動を選んだフォルダ (と、記録があって有効なフォルダ) でだけ、次が変わる。手動を選んでいないフォルダは今のまま。

サムネイル一覧のドラッグ:

1. Explorer・他のアプリへのファイルのドラッグ (コピー) ができない。
2. 一覧の中のフォルダのセルやフォルダツリーへドラッグして移動・コピーすることができない。
3. 並べ替えるたびに、並び順を変えたときと同じくフォルダを読み直す (大きいフォルダでは少し待ち、サムネイルが描き直される)。

1・2 が要るときは、詳細表示に切り替えるか (Q4 を推奨どおりにした場合)、別の並び順に戻せばよい (記録は残る)。
Explorer からのドロップ (コピー) は今のまま。

そのほか:

4. 手動・記録ありのフォルダは、お気に入りが覚えている並び順より手動順が優先し、再起動後も手動順で開く。
5. ファイル名スタック表示と同時には使えない (スタック中に手動を選ぶとスタックが外れる)。
6. Remote から、そのフォルダの並び順を変えられない (表示は「手動順」)。
7. 手動に切り替えただけで並べ替えずに別のフォルダへ移ると、手動の選択は残らない。
8. 記録した後に増えたファイル・フォルダは、種別によらず末尾に並ぶ (普段はフォルダが先頭に来るが、手動順では新しいフォルダも末尾)。
9. サブフォルダ展開を抜けて戻るときなど (同期の経路) で、記録のあるフォルダ、この起動の間に記録があったフォルダ (リセット・削除したものを含む)、
   または起動直後の一瞬に入ると、記録を読み終えてから一覧が切り替わる (通常は一瞬。§4.3 の索引)。それ以外のフォルダは今のまま (2026-11-27 改訂)。

## 9. 段の分け方 (実装の段)

1. ストアと actor: `folder_order.db` (schema・読み書き・区切り helper・全順の集合)、STORES 2 行 (移行元優先・複写の除外)、
   actor (FIFO・同じフォルダのまとめ・改名の要求)。UI 無しでテストできる範囲。
2. resolver と受け渡し: listing の手動入力 (別引数・型付き)、適用可否の述語、全 producer、一覧側の状態型 (context 所有・revision・
   投影キー)、改名完了時の書き換え、Remote の読み込み。
3. 切り替え・リセットの UI、全操作面の共通 handler、`listing_sort_capabilities`、スタックとの排他、詳細表示、KeyAction、
   Remote の表示と拒否。
4. 一覧の並べ替えドラッグ (共通 helper の判断を含む)、開始時の固定と取り消し、自動スクロール、一覧交換の境界での作り直し。
5. マニュアル・製品ページ・設計文書 (architecture-overview・display-pipeline・keymap-spec・filename-stack-plan・web-remote-plan) の更新。

## 10. テスト

- resolver: 記録順・未知は末尾で基準の並びの順・一覧に無い子を飛ばす・種別の行をまたいで 1 列 (既定の 2 行と 4 行の割り当ての両方で、
  記録済みの子の順が行の割り当てに左右されない)・行に入らない子も含む・未記録は普段の並び・
  大文字小文字違い・本のページ順が効く一覧では適用しない。
- 全順の集合: 種別を表示しない・重複除外・フィルタで隠れた子の位置が保存で失われない、ディレクトリから消えた子の行は保存で消える。
- 区切り helper: ドライブルート・UNC 共有ルート・末尾区切り付き。
- actor: 保存の後に読むと保存後の順、同じフォルダのまとめ、別フォルダの要求を捨てない、リセット後に古い保存が記録を復活させない、
  改名: 旧キーの保存 → 改名 → 新キーの保存の順で z の位置が残る、改名の後の読みは新しいキーの記録を返す、汎用の移行が待たされていても
  読み込みが止まらない、移行元が無い改名では移行先の古い行を消す (古い位置を継がない)。
- inventory: 削除・改名の後の保存で古い位置が復活しない。同期の `load_folder` 経路でも記録を読んでから一覧を一度だけ採用する。
- 詳細表示: 列ヘッダの並びの有無によらず基礎の並びは手動順、サムネイル一覧へ戻ると手動順が見える、手動を選んでも `details_sort_key` が変わらない。
- order_revision: 手動↔通常の切り替え・リセットで古い refresh 結果と未確定のドラッグが捨てられる。
- 状態遷移: 通常→手動 (記録無し / 有り・無効)→並べ替え→別の並び順→手動に戻す→記録した順 (有効の印が保存される)、リセット後は
  今の普段の並び順から、手動・未記録で別フォルダへ移ると消える、DB に書くのは最初の成功した並べ替えと記録ありの切り替えだけ。
  お気に入りの表示状態が有効なときの「別の並び順」とリセットの基準。
- 改名: mIV 上のファイル改名・フォルダ改名で位置が保たれる、移行先の古い行を継がない (移行元に位置が無ければ未知)、削除で purge、孤児整理と管理本の保守と汎用の移行に出ない、
  content-identity の候補探索・複写に出ない。改名直後の再走査で末尾へ行かない (一覧の記録と revision)。
- 入力 routing (handler-level): 手動のときだけ drag が並べ替えになり `pending_native_drag` が立たない。手動以外・詳細表示・タッチでは今のまま。
  選択全体の移動 (種別の行をまたぐ選択・相対順の保持・見えていない選択は動かない)、行をまたぐ離しも有効、同じ位置は無変更、ドラッグ中の一覧の世代変更で取り消し。
- 一覧交換: 並べ替え後に選択・checked・今の項目が identity で戻り、古いサムネイル結果が拒否される。
- Remote と PC が同じ順を返す。Remote の並び順変更が手動のフォルダで拒否され、表示が「手動順」になる。
- UI スレッドで `folder_order.db` を開かない (各 producer と並べ替え・リセットの経路)。

## 11. 簡素化の検討 (CLAUDE.md「状態の組み合わせを減らす」)

- 採用: 手動をフォルダごとにして、ドラッグが変わる範囲を利用者が選んだフォルダに閉じ込めた (§2)。全体の並び順の値を増やさないので
  旧版との設定の非互換も作らない。ただし全操作面・お気に入り・Remote の表示への配線は要る (共通 handler 1 つに集める)。
- 採用: 未記録の手動はメモリだけ・フォルダを出たら消す。「切り替えたが未記録」を DB と同期する状態を作らない。
- 採用: DB の読み書きを 1 つの直列 actor に集め、保存・有効の印・リセット・読み込みの順序を FIFO だけで決める。
- 採用: 一覧の記録は一覧 (viewer context) が所有し、再走査は DB を読み直さず revision で古い結果を捨てる。改名との競合を構造で消す。
- 採用: このストアの改名は汎用の移行に乗せず、actor の FIFO の要求にする (移行と保存の前後を FIFO だけで決める)。
- 採用: 削除の purge とは順序を揃えない (前後しても残るのは削除済みの path の行だけ)。孤児整理と管理本の保守からは外す。
- 採用: 読み込み中の仮一覧を作らない (同期経路も記録を読んでから一度だけ採用)。
- 採用: 並べ替え後の表示は並び順の変更と同じ refresh 経路で作り直す (items のその場の並べ替えや新しい置換え経路を作らない)。
  費用は §8 の 5 として利用者に示す。
- 採用: ドラッグ中に一覧の条件が変わったら取り消す (途中の状態を新しい一覧へ移し替えない)。
- 採用: スタック表示と排他にした (スタックの中の並びと手動順の組み合わせを扱わない)。
- 採用: Remote は読むだけ。Remote からの並べ替え・切り替えは扱わない。
- 不採用: 本棚順と同じフォルダ内 JSON (決定 1 でフォルダへ書かない)。
- 不採用: `SortOrder::Manual` (§2 の A)。

## 12. 利用者への確認 (Q1〜Q6、2026-11-26 統括経由で依頼)

- Q1 手動順の持ち方: フォルダごと (推奨、§2 の B) か、全体の並び順の 1 つ (A) か。B では手動に切り替えただけで並べ替えずに
  別のフォルダへ移ると、手動の選択は残らない。
- Q2 表示行: 並べ替えは表示行の中だけ (推奨)。行をまたいで 1 列に並べることはしない。
  → **回答: 行をまたいで並べ替えられるようにする** (推奨と違う)。利用者の言葉: 「Q2ですが、ユーザはその種類を強く認識していないと
  思うので、並べ替えようとして並べ替えられない、というストレスに繋がりそうです。ここは規定の分類を気にせず並べ替えられるように
  したいです。」 §5・§6 を変更。
- Q3 ファイル名スタック: スタック表示とは同時に使わない (推奨)。
- Q4 詳細表示: 手動順で表示し、列ヘッダの並びを選んでいる間は詳細表示ではそちらが優先 (列ヘッダの並びは全フォルダ共通の設定なので、
  手動を選んでも外さない)。詳細表示のドラッグは今のまま (推奨)。
- Q5 複数行にまたがる選択のドラッグ: 掴んだ行の選択だけ動かす (推奨)。
  → **回答: 選んだ全体を動かす** (推奨と違う)。利用者の言葉: 「Q5 は、選んだ全体を移動したいです。追加された複数ファイルを、
  一覧の途中に移動したい場合などはよくあるとおもうため。」 §6 を変更。
- Q6 §8 の 1〜9 の変化でよいか。特に 5 (並べ替えるたびにフォルダを読み直す)。
- Q1・Q3・Q4・Q6: 推奨どおり (Q1 = フォルダごと、Q6 = 受け入れる)。Q6 の一覧は Q2・Q5 の回答で 3・4 (行をまたげない・掴んだ行だけ)
  が無くなり、§8 は 7 項目になった。
- 回答日 2026-11-26、統括 (AskUserQuestion) 経由。

### 実装記録 (段 1)

- §10 の actor テストに残る「移行元が無い改名で移行先の行を消さない」は、§3 と段 1 の明示指示に反する旧記述。
  実装・回帰テストは §3 を正本とし、移行元に行が無い場合も移行先の古い行を消す。Tree は移行先配下も同じ扱いにする。
- data_dir の DB 列挙は `architecture-overview.md` の永続化表と `portable-build-plan.md` の保存先一覧にあり、
  `folder_order.db`（段 1、一覧・UI 未接続）を追記した。`data_dir.rs` には全 DB の登録表は無い。
  `settings_restore.rs` のバックアップ／完全リセットは settings.db の家族と本の構成を対象とする専用処理で、
  任意の data_dir DB を列挙・削除する仕組みではないため、そこへフォルダ手動順は追加しない。
- `STORES` の共通宣言に `copies_with_content_identity` と `orphan_cleanup` を追加した。
  前者を候補探索・複写・進捗件数で共有し、後者を孤児 scan と delete の両方で DB open 前に確認する。
  管理本は `BookStorePolicy::Excluded` で open 前に除外し、閲覧履歴専用 SQL は `rename_generic` の真偽から切り離した。
  全 STORES の本番 schema を検査する既存テストにも新 DB の初期化を追加した。
- Save の要求は一覧全順と inventory を持ち、DB の既存記録との合成・active=1・DB revision の増加を同一 transaction に置く。
  要求の `revision` は後続段の一覧側が渡す完了照合用の番号で、DB record の `revision` とは別に扱う。
  リセットで DB row を消しても、一覧側の `order_revision` を巻き戻す意味にはしない。
- Save の集約はキュー末尾にある同じフォルダの連続 Save で、一覧の集合と inventory も同じ場合に限定する。
  集合が変わると、先の Save で初めて記録され、後の一覧では隠れた子の位置を先の Save を落とすことで失い得るため。
  Read・SetActive・Reset・Rename と別フォルダの要求もすべて境界とし、それらをまたいで古い Save を置換・移動しない。
- 読取 record の `items`、Save の `listing_order` / `inventory` は子の完全 path の KeepDrive キーで統一した。
  `None` は未記録を表す。複数改名は一要求・一 transaction とし、元のキーの snapshot を共通 pure helper で写して
  相互改名でも移行元の値を失わない。temp DB の actor テストは専用 harness が worker を join してから一時領域を破棄する。
- 段 1 の実装担当では利用者の PC 負荷指定に従ってコンパイル・自動テスト・検証用ビルドを実行しない。
  追加テストの実行とコンパイル確認は設計担当の共有 runner へ引き継ぐ。
- `cargo fmt`（引数なし）は実行済み。独立実装レビュー (設計担当が依頼する別の gpt-6.1-sol / xhigh) は全段の後に受ける
  (実装担当の報告にあった「別 context の静的レビュー」は設計担当が依頼・確認したものではないため、独立レビューとして扱わない)。

### 実装記録 (段 2)

段 1 の共有 runner で失敗した `folder_order_save_and_reset_touch_direct_children_only_with_literal_wildcards` は、
fixture のフォルダキー `c:/fXX` も `c:/fxx` に統一する。production の正規化済みキーの検査は維持する。

同期 `load_folder` の production 呼出し元を着手時に列挙した。入口列の行番号は段 2 着手前のもの。
再確認列は関連 815 件中 6 件失敗の報告と、§4.3 の索引導入後のソース照合（2026-11-27）。対象かどうかは移動元でなく移動先の一覧で決まる。
以下の「索引分岐」は、準備済みかつ移動先が索引に無ければ同期、それ以外は非同期 Read 後に採用することを指す。
テスト、コメント内の参照、`load_folder_with_scan` 等の内部実装呼出しはこの表と分けて確認する。

| 呼出し元 | 入口 (着手前 file:line) | 対象と採用境界の確認 | 失敗報告後の再確認 |
| --- | --- | --- | --- |
| 再生の自動検証で親フォルダを開く | `app.rs:34375` (`poll_play_test`) | 物理一覧。採用待ちと採用後の動画選択を既存 open owner に揃える | 索引分岐。移動中は return し、移動先の採用後に動画を選ぶため両方に対応する |
| 一時ビューの戻り先 (mounted context) | `app.rs:35338` (`restore_view_return_origin`) | 通常物理フォルダは共通の採用へ。context の既存復元所有権を維持する | 索引分岐。展開一覧の復元は別 dispatcher。load 直後は return し、後続の同期 items 操作なし |
| 検索から開いた本の前後送り | `app.rs:35874` (`favsearch_navigate_sibling`) | `Search(Favorite)` の所有する対象外一覧。nav_stack / address と load 後 items の同期契約を維持する | 検索の所有を維持する surface 判定で手動 Read 対象外。同期契約のまま |
| 検索内の親へ戻る | `app.rs:35896` (`favsearch_back`) | 同じく検索所有で対象外。空スタックでは既存の検索結果一覧へ戻る | 同じ検索 surface。空スタックは execute_favsearch へ渡す |
| 本棚を開く | `app.rs:56021` (`open_books_root`) | 本棚は手動順の対象外。同期契約を維持する | books root は索引分岐より前の除外条件に含む |
| 本の管理から開く | `app/location_jump.rs:30` (`open_book_manager_target`) | 本棚／管理本は対象外。それ以外の物理一覧は共通 open へ | 本棚／直接の管理本は除外。それ以外は索引分岐で、load 後の同期 items 操作なし |
| 展開走査の中止から元の場所へ戻る | `app/subfolder_expansion.rs:1801` (`toggle_subfolder_expansion_view`) | 展開一覧が表示中なら、その snapshot / surface の破棄は成功採用側 | 索引分岐。即時の場合も worker Read 後の場合も同じ installer が破棄する。入口に事前破棄を追加しない |
| サブフォルダ展開を終了 | `app/subfolder_expansion.rs:1986` (`exit_subfolder_expansion_view`) | 同上。read 前に元の表示状態を破棄しない | 索引分岐。失敗 5 の未記録フォルダは準備済みなら即時終了。非同期テストは復帰先に実記録を保存する |
| スマートフォルダ中止の戻り先が再利用不能 | `app/smart_folder.rs:8299` (`restore_cancelled_smart_folder_prior`) | 物理戻り先の旧表示所有権の破棄は成功採用側 | 索引分岐。戻り先なしのドライブ一覧は即時のまま |
| 表示中スマートフォルダの定義削除 | `app/smart_folder.rs:8373` (`forget_smart_folder_definition`) | 同上。戻り先なしのドライブ一覧は既存の即時採用を維持する | 索引分岐。非同期テストは復帰先に実記録を保存し、旧 owner の保持を検証する |
| 固定 snapshot の変換書庫を一覧で開く | `app/snapshot_ops.rs:1926` | 固定範囲内の操作で対象外。`snapshot_internal_nav` 中の同期契約を維持する | internal nav と固定範囲を維持し、通常物理 Read / 索引分岐へ入らない |
| 固定 snapshot の読書開始 | `app/snapshot_ops.rs:2135` (`snapshot_load_and_open`) | load 直後の items と fullscreen 選択に依存する対象外操作。同期契約を維持する | 同じ internal nav 除外。固定内にいる間の契約は維持する |
| 固定 snapshot の別コンテナを一覧で開く | `app/snapshot_ops.rs:2870` | 同じく対象外。同期契約を維持する | 同じ除外。固定解除後の通常物理先への再読取は worker producer なので索引で省略しない |

上表とは別に、失敗 6 の `load_folder_or_convert_archive`（`app.rs:30254`）も照合した。
Smart root の履歴採用後、定義の範囲外にある新しい通常物理フォルダへ移るテストであり、Smart 所有の子への移動ではない。
`duration_metadata.rs:73` の移動先 surface は Folder、空の一覧は `folder_order/listing.rs:356` の predicate で eligible になる。
改訂後は同じ索引分岐で、準備済み・索引に無い場合はその場で Loaded と履歴確定まで進み、
索引に有るか準備中なら Classifying を返して worker Read 後に採用する。移動元で対象外へ読み替えない。

- 段 2 の前提確認で、§4.1 の「Remote は別プロセスなので DB を直接読む」と実際の一覧生成の所有が一致しないことを発見。
  `web-remote-plan.md:86` は通常フォルダ一覧を IPC → 本体 worker と規定し、`remote_ipc/pipe.rs:42` が
  本体側の `ContainerEngine` を使用する。この worker で `read_remote_at` を呼ぶと本体 actor 単一 opener の原則と両立しない。
  設計担当の回答により actor 経由へ修正すると決定した（2026-11-27、段 2 続行）。§4.1 / §4.3 も同じ境界へ訂正済み。
- 手動入力は `FolderOrderListingInput::NotApplicable | Read { result, stack_active }` とした。
  `Read(Err)` に相当する入力は読取失敗の事実を保持し、原本を残して基準順を表示する。`NotApplicable` や未記録への読み替えはしない。
  stack は同じ Read の適用抑止情報で、active や記録を消さない。`ListingOrderRequest` は変更しない。
- `ScannedDir` は `read_dir` の entry を得た直後、内部ファイル・隠し属性・種別投影・同名除外より前に完全 path キーを inventory へ入れる。
  materializer は型付き入力の別引数だけを使い、DB を補読しない。基準配置後の permutation を items / metadata / sort metadata と
  PreparedRating の添字へ同じように適用し、その後に row record / video 添字を作る。未知は基準配置の順を保つ。
- App は process 所有の actor を一つ持ち、一覧側の `FolderOrderState` だけを `ViewerContextBundle` へ載せる。
  同じフォルダの fork は folder key と context-local revision が一致し得るため、書込要求・完了へ明示 owner token も加えた。
  完了は owner context / folder key / revision の三つで照合し、兄弟の状態へ適用しない。Save の集約も同じ owner の要求に限定する。
  DB schema は変更しない。通常 UI の Drop は join せず、最終終了とテスト終了だけ明示 shutdown で受理済み要求を排出する。
- watcher と sort refresh は一覧の記録 snapshot と `order_revision` を使い、actor を読み直さない。
  表示行が同じ watcher 応答も inventory は新しい走査結果へ更新し、表示外で消えた子を後の保存で復活させない。
  revision が変わった結果は採用せず、既存の refresh 所有経路で現在の条件を作り直す。
- サブフォルダ展開／スマートフォルダから物理一覧へ戻るとき、旧 snapshot / surface の clear は入口から除き、
  既存 `start_loading_items` の成功採用境界で items とともに行う。対象外の固定 snapshot と検索所有の一覧は同期契約を維持する。
- ★固定の `saved_items` を直接復元する経路にも、同じ所有境界の `saved_folder_order` を持たせる。
  固定一覧自体は対象外とし、記録済みの元一覧は記録・inventory と組で戻す。元の未記録選択は保存対象にせず、DB には書かない。
  復元時も context revision は進め、direct-media の fork で元一覧を復元する経路にも同じ受け渡しを行う。
  固定内の child で解除し、その child が通常の対象フォルダになる場合は、既存の `MainListNavigation` Restore に渡して
  worker の Read 後に一度だけ採用する。待機中の旧 rows は Snapshot surface に所属させ、未読の通常一覧を公開しない。
- Stack の適用可否は集約セルを元の静止画として判定する。本の固定順の条件を共通 helper へ切り出し、
  既存ソートの wrapper は従来の Image のみの判定を維持する。手動順の predicate だけ Image / Stack を静止画とみなす。
  本棚 root / 直接の管理本 / 仮想経路 / 固定 snapshot / 検索所有の一覧は対象外のままにした。
- `cargo fmt`（引数なし）と UI glyph 検査を実行した。コンパイル・cargo check・build・test・nextest・実機起動は
  利用者の PC 負荷指定により未実行。独立実装レビューは今回受領しておらず、設計担当が全段の後に依頼する。
  実装担当はコミットを行っていない。段 2 の初回差分は設計担当が `9ccc99e91`（WIP）として記録し、続行分を同じ作業ツリーへ残す。

段 2 続行（2026-11-27）:

- `ContainerEngine` は production 起動時に `Arc<SharedReaders>` を受け取り、各読取時に登録済みの actor handle を取得する。
  engine 作成後の App による handle 登録も反映し、通常フォルダ一覧に第 2 の DB reader を作らない。
  不要になった `read_remote_at` と直接読取 helper は削除する。読取失敗は共通の `Read(Err)` と基準順表示に揃える。
- 共有 runner で報告された lib test target の7件の名前解決エラーは、`phase_c_support` の
  `PathBuf` / `ScannedDir` / `OpenRequestOwner` / `StartupListIntent` の明示 import を補った。
  前回変更した他の test module も import・型・追加 field・共有 API の引数をソースで照合し、
  shutdown 回帰に残った削除 API の参照を test-private な DB 確認へ置き換えた。
  この確認はコンパイル・テスト実行・独立レビューの代わりではない。
- Remote 回帰は実 actor を登録する形へ更新した。Save の直後の読取（完了待ちなし）、
  engine 作成後の handle 登録、終了した actor と保存済み DB の組で直接 DB を開き直さないこと、
  actor 未登録、未記録、破損 DB の原本保持を対象にした。protocol は 74 のまま変更しない。
- 続行分も `cargo fmt`（引数なし）、UI glyph 検査、`git diff --check` を実行して正常終了した。
  コンパイル・テスト・検証用ビルド・コミットは実施せず、共有 runner での再確認へ引き継ぐ。

段 2 関連テストの失敗対応（2026-11-27）:

- 設計担当の共有 runner による修正前の結果は 815 件中 809 件成功・6 件失敗。以下の修正後のテストは未実行。
- 新規の stale rescan / hidden inventory テストは、分類の完了を一覧の identity 準備完了と取り違えていた。
  `poll_external_rescan` は準備中に応答の採用を保留する既存契約なので、fixture と refresh helper を既存の
  `finish_page_identity_preparation_for_test` へ揃えた。応答の消費、古い revision の拒否、refresh 要求の保持、
  Equal 応答の新 inventory の採用という assertions は維持・補強する。production の保留条件は変えない。
- materializer の行をまたぐ順のテストは、表示用 metadata のサイズと整列用 metadata のサイズを同一視していた。
  実フォルダの整列用サイズは既存契約で None。基準一覧の表示用と整列用を別々に控え、手動順・詳細ヘッダありの双方で
  items と全 metadata の対応・列長を照合するテストへ直した。基準の投影は変えない。
- 固定解除の child テストも、固定の identity 準備中に child load を要求して拒否され、origin 復元の分岐を通っていた。
  固定の準備を待ち、解除前の child 採用を明示的に確認する。同時に `take_return_origin` が surface を Folder へ変えていた
  production の境界を修正し、通常物理 child の Read 待機中は旧 Snapshot surface・items・手動状態・戻り先の所有を保持する。
  成功採用だけが Folder へ切り替え、items generation を 1 回進める。origin 復元と対象外 child の即時契約は維持する。
- この時点の改訂前設計では、既存の失敗 5（展開ボタンの即時終了）と 6（Smart 履歴後の別通常フォルダへの即時 Loaded）は未修正。
  どちらも移動先は eligible な物理一覧で、対象外の Smart / 展開一覧へ入る例ではない。§4.3 の Read 後 1 回採用と、
  既存の即時終了・即時履歴確定を同時に満たせない。移動元で対象外にする分岐、先に surface だけを戻す処理、
  未採用なのに Loaded と返す処理、既存テストの期待値変更は入れず、この部分は契約の判断待ちとして停止する。
  同型の復帰経路の確認は上の同期呼出し元表へ記録した。段 2 の検収完了を意味しない。
- 今回も `cargo fmt`（引数なし）と `git diff --check` は正常終了。コンパイル・自動テスト・検証用ビルド・コミットは実施しない。

段 2 記録索引による同期契約の復元（2026-11-27）:

- §4.3 改訂に従い、`Queue` が `RecordIndex { keys, ready, startup_renames }` を持つ。
  handle と actor の既存の共有 Mutex で、要求の投入と索引の追加を同じ境界に置いた。UI の照会はメモリだけで、
  actor は DB の全 folder_key を読んでから同じ Mutex 下で履歴の反映・和集合・準備済みの公開を行う。
  DB の読取中は Mutex を保持しない。active の値で索引の対象を絞らない。
- Save は投入時に対象を追加する。Rename は `rewrite_key_with_mappings` を既存キーの snapshot 全体へ適用して追加し、
  Tree の配下・バッチの同時改名・準備前の連続改名も同じ規則で扱う。Reset・改名・purge の完了で key を削除しない。
  準備前の追加と初期 DB キーの取りこぼしを避けるため、準備前 Rename は投入順に保存し、初期 DB キーへ順番に適用してから和集合にする。
- DB open または初期索引読取に失敗した場合は準備済みにしない。既存の open 失敗と同じくログを残し、
  以後の Read / write に同じ typed error を返して既存の一覧読取・書込完了側で通知する。同期入口は非同期 Read へ回り続ける。
  記録なしとは見なさず、再試行・DB 再作成・データ削除は追加しない。
- 同期入口の事前分類分岐と、共通の物理一覧 Read 分岐に索引の照会を接続した。
  準備済み・索引に無い対象は scan に明示の `Read(Ok(None))` を渡し、既存 installer の末尾まで同期実行する。
  記録あり・準備中は従来の worker Read と成功採用を維持する。展開終了などの入口へ事前破棄や独自分岐は足していない。
  通常 open・startup・DFS・履歴 preflight・Remote・Agent の worker producer は索引で読取を省略しない。
- テストの即時性は、共通 `setup_app_with_options` の最後で実 actor の Read の完了を待ち、索引の準備済みを assert して明示する。
  既存の多数の同期 fixture のタイミング依存を残さず、既存 2 件の body / 期待値を変えないため共通初期化を選んだ。
  これはテスト thread のみ。準備中のテストは DB open 前の channel gate を持つ専用 actor へ置き換え、時間待ちには依存しない。
- 非同期復帰を検証する展開終了・Smart 定義削除のテストは、復帰先の実記録を保存する形へ更新した。
  新規テストでは即時 Loaded・Read 要求ゼロ、記録先の非同期採用、準備中の未記録先の非同期採用、
  Reset 後の key 保持、Reset→Save、Exact / Tree の移行先追加、準備前 A→B→C と保存の和集合を検証する。
  前回停止した失敗 5・6 の実装上の衝突はこの改訂で解消し、修正後の実行確認は共有 runner へ引き継ぐ。
- `cargo fmt`（引数なし）と `git diff --check` は正常終了。変更したテストの import・型・引数をソースで照合した。
  コンパイル・テスト・検証用ビルド・コミットは利用者の指定により実施していない。

段 2 分類状態を観測する既存テストの失敗対応（2026-11-27）:

- 設計担当の報告では `ee9ee88b8` までの関連 1141 件中、Remote 取得前の分類状態を確認する既存テスト 1 件が失敗した。
  原因は production の slot 変更ではなく、段 2 で追加した `AppTestEnv::load_folder_with_scan_owned` が
  呼出直後に `settle_open_path_classification_for_test` で分類完了まで進め、最初の assert 前に状態を消費したこと。
- 同型の test wrapper である `load_folder` / `load_folder_with_scan` も照合し、事前 scan の無い EPUB・書庫名のパスでは
  必須の分類 owner を呼出元に残す形へ揃えた。事前 scan で directory と確定している場合と、通常名の手動記録 Read による
  新しい非同期採用だけは、従来どおり fixture の便利メソッドが待つ。既存の失敗テストの body / 期待値は変えない。
- production の差分も master と比較した。段 2 の `start_open_path_classification_owned_at_stage` への呼出変更と、
  読取用の `start_open_path_classification_owned_with_navigation` の追加は、いずれも既存の共通実装へ到達し、
  `top_level_grid_view.set_open_path_classification` に同じ owner を置く。Remote の取得は同じ PathClassification の退役で
  owner を取り出して cancel し、その後の poll は読込結果を採用しない。本番の分類・Remote 退役処理の変更は不要だった。
- EPUB・7z 名のディレクトリ × 3 種の fixture load 入口で、準備済みの未記録索引でも分類状態が残り、
  Remote 取得で cancel され、後から結果を poll しても移動先を開かないことを検証するテストを追加した。
- `cargo fmt`（引数なし）と `git diff --check` を実施し、コンパイル・テスト・コミットは利用者指定により実施しない。

### 実装記録 (段 3)

- 段 2 は `4657a489f` までの関連 1143 件が成功したとの設計担当の報告を引き継いだ。
  段 3 のコンパイル・テスト実行結果を表すものではない。
- UI の選択は `FolderSortChoice::Standard(SortOrder) | Manual` で共通の `select_folder_sort` へ渡す。
  Standard は手動の解除と既存設定値の変更までを担当し、保存と再表示は各入口の既存の末尾で一度だけ行う。
  Manual は段 2 の遷移と再表示を自身で担当する。再選択は何もせず、スタック中の選択だけは OFF にして手動へ戻す。
  `SortOrder` の保存形式には手動を追加しない。同値の Standard 選択も、手動がスタックで隠れている場合を含め通常へ戻す。
- メニューと toolbar / facet の両 dropdown へ「手動順 (このフォルダ)」と「手動順をリセット…」を追加した。
  手動が適用されている間は通常順の選択表示を外す。`listing_sort_capabilities` の手動用の無効理由は、
  段 2 の物理一覧の対象判定から導き、詳細ヘッダの表示上の並べ替えとは独立させた。手動を選んでもヘッダ設定は変えない。
- **ツールバーの手動個別ボタンは未追加**。`settings.rs:6295` の `toolbar_sort_items: Vec<SortOrder>` と
  ツールバーの「出すソート」の編集が、個別ボタンの集合を所有している。手動をその集合で設定できるようにするには
  新しい設定表現が必要なため、利用者の段 3 指示に従ってこの部分を停止した。無条件の追加や新設定は行わない。
  既存の設定済み Standard ボタンは共通 handler へ接続し、手動から同値を選んだ場合も解除する。
- リセット確認は要求元の context ID と folder key を保持し、その対象の記録だけを破棄する。
  `common_modal_dialog_open` へ表示 helper を登録した。Window は `default_pos`、通常の OS テーマ、意味色の警告、
  IME 対応の Enter / Escape helper と非 repeat Enter を使う。Cancel・Escape・× は記録を変えない。
  純描画 helper の小画面テストは 1093×614 相当の 100% / 200%、Light / Dark で操作行の矩形と実クリックを検証する。
  同じフォルダを開いた兄弟 context に確認操作を渡しても記録を破棄しない回帰を追加した。
- スタックの既存 toggle の「次の再表示に向けた状態と選択ヒントの変更」を helper として共有した。
  手動への切り替え・リセットも同じ helper を通し、再表示自体は sort refresh 一回に揃える。
  スタック ON/OFF は記録と active を保持し、DB へ書かない。既存のスタックの入力受付・通知は維持する。
- ゲームパッドの picker は一つの `grid_sort_choice` を持ち、通常順と手動を同じ選択列で循環する。
  collection の所有先と既存のロックは維持する。preview は共通 handler を使い、確定時は既に適用した手動を再実行しない。
  preview を閉じても既に適用した変更を保持する既存契約も維持する。
- `GridEnterFolderManualOrder` を Grid / Press / 既定なしで追加し、KeyAction の列挙・INI 名・説明・context・trigger・
  default・ALL_ACTIONS と実際の入力 helper を接続した。既定 INI と keymap 仕様も同期した。操作は enter-only。
  スタック等の入力保留中は既存の受付条件に従う。
- Remote の手動 shape は段 2 の `FolderOrderState::applies_to_physical_listing` が決めており、PC の適用表示も同じ helper を使う。
  `selected: manual`・単一 option・locked reason と SetSortOrder の拒否は変更しない。protocol は 74 のまま。
- 既存の `facet_sort_dark` / `facet_sort_narrow_dark` / `facet_sort_edges_dark` / `facet_sort_edges_narrow_dark` は
  閉じた combo の純描画 fixture で、`tag_sort_choices_dark` も専用のタグ選択のみ。今回の追加行を開く既存 PNG は無いため、
  更新対象の既存 baseline は無い。PNG の生成・更新は行わず、実入力と geometry の回帰を追加した。
- 追加テストは 19 件。既存の INI / settings 往復テストは手動順の操作も含め、
  `unassigned_grid_actions_roundtrip_ini_and_settings` へ改名した。
  `cargo fmt`（引数なし）、`python scripts/check_ui_glyphs.py`、`git diff --check` は正常終了。
  コンパイル・テスト・検証用ビルド・コミットは利用者指定により実施せず、設計担当の共有 runner へ引き継ぐ。

#### 関連回帰の修正 (2026-11-27)

- 設計担当から、段 3 と build_ui の修正後の関連 2437 件中、master で成功する既存 3 件が
  失敗したとの報告を受けた。本修正後のテスト結果はまだ受け取っていない。
- 段 2 で追加した投影キーの拡張が Normal / NotApplicable / スタック中にも folder identity と
  order_revision を混ぜていた。通常順の削除でも番号が進むため、保持済みの観測記録とキーが一致せず、
  次の走査で残存行の比較とディレクトリの表示用 mtime の保持が働かなかった。
  検索内の物理の子でも、準備した従来キーと現在キーが異なっていた。
  拡張は対象の物理一覧で手動順を適用する場合だけとし、それ以外は従来キーをそのまま返す。
  通常の order_revision の進行と、走査・refresh の独立した番号照合は維持する。
- 準備段階では要求時の従来キーを保持し、採用時の番号確定後に手動分を一度だけ加える。
  採用処理がスタック状態を片付ける前に要求の ON/OFF を取り、現在キー・走査要求も同じ要求側の判定を使う。
- 同じ物理フォルダの通常スタックのグループ化完了は、新しいフォルダへの移動ではない。
  この派生表示の install では記録・inventory・mode・番号の所有を維持する。
  抽出結果・サブフォルダ展開などへの install は従来どおり対象外へ退役する。
- 手動適用中の削除では、削除前の現在キーに一致した観測記録だけを、成功した行削除と新番号へ結び直す。
  同じ source の行記録が削除後の世代へ正しく引き継がれた場合に限り、非表示の子の削除も扱う。
  未反映のソート変更がある記録、途中のスタックで退役した記録、最後の非画像を削除してページ順固定へ
  変わった記録は新方針の証拠にしない。削除前の走査結果は引き続き番号照合で拒否する。
- 通常キーの不変・手動遷移・スタック抑制、グループ化の所有保持、表示中と非表示の子の削除、
  旧ソート方針とページ順固定への変更を覆う回帰 7 件を追加した。既存 3 件の期待値は変更しない。
  設計からの逸脱や既存機能の変更はない。コンパイル・テスト実行・コミットは利用者指定により行わない。
  `cargo fmt`（引数なし）と `git diff --check` を実施した。

#### App の格納サイズ (2026-11-27)

- 設計担当から `d669a3ef3` の本体 lib 全 20934 件中、App の既存サイズ検査だけが失敗し、
  実測が 110408 bytes だったとの報告を受けた。上限 110000 bytes は変更しない。
- App の `folder_order`、process 所有の service、reset confirmation を Box へ置き、
  `ViewerContextBundle` でも同じ Box 型を swap / fork する。fork の clone は独立した状態の写しを作る。
  App に inline で載る SnapshotState の saved_folder_order も Box に揃え、退避・復元で移す。
  worker の materialize 結果は従来の値型を保ち、採用時に App が持つ Box の内容へ書き込む。
- 既存の cold-owner 検査へ 3 field のポインタサイズ検査を追加し、history-transition のサイズ検査へ
  storage / payload の診断出力を追加した。コンパイル・テスト・コミットは利用者指定により実施しない。

### 実装記録 (段 4)

- 段 3 は `51cce6c5a` までの本体 lib 全 20934 件が成功したとの設計担当の報告を引き継いだ。
  以下は段 4 の実装事実で、コンパイル・実行・検収の結果ではない。
- `dispatch_grid_cell_input` の既存の開始条件は維持し、その成立後だけ分岐する。
  `folder_manual_order_drag_enabled` のときは並べ替えを始め、それ以外は従来の native payload を作る。
  詳細表示・通常順・スタック・対象外でのコピー／移動、右ドラッグ・touch の抑止を変更しない。
- `app/folder_order_drag.rs` の一時状態は egui Context の temporary data に置く。固定部分は Arc で共有し、
  毎フレーム一覧のキーを clone しない。App や ViewerContextBundle に inline field を増やさない。
  開始時の子の identity・見えている選択集合・context・viewport・scroll session・folder・items generation・
  order_revision・表示 geometry・フィルタの方針と可視集合を照合し、変化したら保存せず捨てる。
  fork／swap で途中の操作を別 context へ引き継がない。
- grabbed が checked にあれば見えている checked 全体を、無ければ grabbed だけを移す。
  固定した順で連続挿入し、開始後の checked の変更で移動対象を入れ替えない。
  左半分で前、右半分で後、挿入線は窓と同じ選択色の縦棒と両端の短線。
  viewport 外・空のセル・位置情報のない release は保存しない。
- 本／コレクションの窓と同じく、cell Response を処理する前に古い操作の terminal を取り出す。
  同じ input batch の release と新しい press が互いの移動集合を奪わない。
  旧操作が保存した場合は revision と既存の refresh 中の入力停止が次の操作を退役させる。
- 自動スクロールは canonical `GridScrollState` の `set_position` から既存の external scroll へ渡す。
  小さい pixel 移動量はドラッグの端数へ貯め、row_h の整数倍だけ offset を変える。
  同じ frame の再 pass では二重に貯めず、端にいてまだ進める間だけ次の描画を要求する。
  touch の描画端数と scroll transport／readback の所有は変更しない。
- 成功時は既存の `commit_folder_manual_reorder` を呼ぶ。items はその場で置換せず、
  メモリの記録・revision・actor Save・sort-change refresh の経路で作り直す。
  同位置の drop は未記録のままで、既に記録している場合も保存／refresh を発行しない。
  非表示項目と inventory-only の記録済み項目は §3 の合成で保持する。
- refresh の採用前に selected／checked を identity で取り、採用後に復元する処理は段 2 で実装済みだった。
  その本体処理を増やさず、初回の filtered reorder と待機中の選択変更を覆う回帰を追加した。
  サムネイル一覧の「今の項目」は selected のカーソルであり、fullscreen の close／別窓保持の契約は変更しない。
- **共通化は今回は行わない**。本は page key／編集 metadata、コレクションは entry ID／表示投影を所有し、
  どちらも窓内の配列を更新する。一覧は物理キーの固定列を保存して非同期 refresh する。
  挿入線の gap も本は列端を考慮し、コレクションは固定幅、一覧は連続セルで異なる。
  窓の pixel scroll と一覧の row snap も別の契約のため、3 者の状態まで汎用化する変更は持ち込まない。
  既存の窓を変更するリファクタは無く、「別コミットに分けるべき範囲」は無い。
- 既存の窓で共有するカーソル変更の設定は見つからなかったため、カーソル形状を変えない。
  Esc の操作・設定項目・既定キーは増やさない。設計からの逸脱は無い。
- 回帰は 25 件追加（Windows の context 検査 1 件を含む）。既存 snapshot の非ドラッグ時の描画は変更せず、
  PNG を生成・更新していない。コンパイル・テスト・検証用ビルド・コミットは利用者指定により実施しない。
  `cargo fmt`（引数なし）、`python scripts/check_ui_glyphs.py`、`git diff --check` は正常終了した。

### 実装記録 (段 5)

- 段 4 は `d8e8d7e52` までの関連 952 件が成功したとの設計担当の報告を引き継いだ。
  take_drag helper と reset dialog の警告のコンパイル修正 2 件は設計担当が実施した。
- 公開文書の表記ポリシーを確認し、grid の手動順の節を正本として、settings・shortcuts・
  一覧のチュートリアルから案内した。選択方法、選択全体のドラッグ、最初の成功からの保存、改名／追加、
  リセット、通常のドラッグへ戻す方法、詳細ヘッダ・スタックとの関係、既定キーなしの操作を記載した。
  公開する操作説明には actor・revision・inventory 等の実装語や版固有の案内を加えていない。
- 製品ページは専用カード 1 枚とマニュアルへのリンクを追加した。privacy の保存データを日英とも更新し、
  folder_order.db が対象パスと順を端末内に保存することを記載した。
  「安心して使えます」と通信の記述は照合し、新しい通信・外部保存が無いため本文を変更しない。
- architecture の module map・既存 DB 行、display pipeline の resolver と採用境界、stack の手動抑制、
  Remote の共有 actor／manual shape／sort-lock 拒否／protocol 74、README の設計索引を更新した。
- CHANGELOG は版番号・日付を決めず「未リリース」欄へ 1 項目を追加した。公開済みの版の見出しと
  manual/changelog.html は変更しない。画像や PNG baseline の作成・差替えも行わない。
  未実装のツールバーの手動個別ボタンは案内せず、実装済みの選択欄・メニュー・キー操作を説明する。
- 本段は文書だけを変更し、コンパイル・Rust テスト・検証用ビルド・コミットは実施しない。
  `python scripts/check_site_cards.py` は 54 cards / 違反 0、`python scripts/check_ui_glyphs.py` と
  `git diff --check` も正常終了した。
