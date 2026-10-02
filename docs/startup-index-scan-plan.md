# 起動時の索引スキャン軽減 (v4.3.0)

状態: 設計第 5 版 (実装前レビュー 4 回目で ACCEPT WITH CHANGES、その指摘を反映)。ブランチ `v430-startup-scan`。

## 1. 背景

起動のたびに、お気に入りごとに次の 3 系統が同じフォルダ木を全走査する
(現行の流れは [search-architecture.md](search-architecture.md) §4)。

| 系統 | 入口 | 走査 |
| --- | --- | --- |
| アイテム索引 (Ctrl+G) | `indexer_supervisor::run_initial_scan` → `search_walker::scan` | 全ファイル列挙 + fts_meta との 3-way diff |
| 名前索引 (Ctrl+S) | `name_bulk_indexer::run_bulk_name_index` | Pass 1 でフォルダ列挙、Pass 2 で全フォルダを再度 `read_dir`、全フォルダ行を DELETE→INSERT |
| 類似 (別バージョン) 索引 | `similar_index` Full (`FullReason::Initial`) | 全 root を列挙 + 全 inventory 読み込み |

利用者環境の `mimageviewer.log` (2026-10-02 10:05 起動と、その前の起動 `.prev`) より:

| 対象 | 前回起動 | 今回起動 |
| --- | --- | --- |
| 名前索引 d:\home\18 (12,219 フォルダ) | 9.5 秒 | 104 秒 |
| 類似索引 (432 万 dir entry) | 40.5 秒 | 300 秒時点で走査中 |
| アイテム索引 g:\home\comfyui (54.7 万ファイル、取り込み 0) | 36 秒 | 32 秒 |
| アイテム索引 c:\home\youtube\movie\youtube (591 件) | 取り込み 346 件 | 取り込み 245 件 |

利用者の使い方は「常駐したまま OS を再起動する」で、再起動のたびにこれを払う。

## 2. 方針と範囲

| 項目 | 内容 | 段 |
| --- | --- | --- |
| A | 入れ子のお気に入りでアイテム索引が毎回取り込み直す不具合 | 1 |
| B | 名前索引の二重 `read_dir` と、変化のない行の書き直しをやめる | 1 |
| C | サイドカー判定の per-image `stat` (最大 4 回) をやめる | 1 |
| E | 「起動時に、終了していた間の変更を確認しない」設定 (既定 OFF) と手動確認ボタン | 2 |
| D | 3 系統の走査の統合 | 3 (段 1 後の計測で決める) |

段 1 は意味を変えない最適化と不具合修正。段 2 は利用者が選ぶ挙動変更。段 3 は §8。

### 2.1 状態の組み合わせを減らす検討 (CLAUDE.md 設計の簡素化)

- E は「一部の取りこぼしを許容して起動時の負荷をなくす」機能 (2026-10-02 利用者決定)。
  整合性を厳密に保つ仕組み (終了の仕方の判定、仕事中の印の消去、耐久書き込み) は持たない。
  初版は「正常終了かどうか」を印で追う案だったが、mIV は WM_QUERYENDSESSION /
  WM_ENDSESSION を処理しておらず常駐中の OS 再起動では終了処理が走らないこと、
  利用者が厳密さを求めていないことから、**「一度でも完全に走査できた」印だけ** にした (§6)。
  終わり方・クラッシュ・処理途中のイベントの組み合わせを扱わなくてよくなる。
- E の「スキャンを省くか」は系統ごとに 1 つの印だけで決める。similar は root ごとに分けず
  全体で 1 つにする (§6.2)。
- 印には構成の指紋を含め、指紋が変われば印は無効 = 通常どおり走査、で済ませる
  (構成変更への専用の追従処理を作らない)。
- A の所有者の同点決着に並び順を使わず UUID を使う。並べ替えを所有構成の変更にしないため。
- A の所有者の移し替えに専用の工程を作らない。走査の突き合わせ先を「自分の所有範囲の行」にして、
  持ち主の違う行は通常の走査が取り込み直す (§3)。第 2 版は付け替え工程と、止め切ってから
  付け替える再構成経路を持っていたが、止め方・付け替えの途中・同時に走る旧 supervisor の
  組み合わせごとに取り残しが出た (設計レビュー 2 回目)。移し替えを走査の性質にすると、
  この組み合わせが無くなる。

## 3. A: 入れ子のお気に入りの取り込み直し

### 原因

`fts_meta.files` の主キーは `path` で、upsert のたびに `favorite_id` を上書きする
([fts_meta.rs:306](../src/fts_meta.rs))。walker の差分は自分の `favorite_id` の行だけを見る
([fts_meta.rs:398](../src/fts_meta.rs) `list_favorite_files`)。外側 (`c:\home\youtube\movie`) と
内側 (`...\movie\youtube`) の両方が `auto_index_metadata` だと、2 つの supervisor が互いの行を
「DB に無い」と判定して取り込み直し、`favorite_id` を奪い合う。ログの取り込み件数
245 + 346 = 591 = 内側の総数で、値は起動ごとに入れ替わる。watcher 経由の変更も両方が処理する。

副作用として、Ctrl+G のお気に入り絞り込み (選んだ 1 つの ID だけで絞る、
[global_search_ui.rs:2263](../src/global_search_ui.rs)) は、入れ子の範囲のファイルを
「最後に書いた側」にだけ出す。現状でも起動ごとに結果が変わる。

### 所有者の規則 (1 か所に置く)

- パス p の所有者は、`auto_index_metadata=true` のお気に入りのうち p を含む **最も深い** root。
  正規化後に同じ root が複数あれば **UUID の文字列順で最小** のもの。共通の `excluded_roots`
  (製本などアプリ管理の派生物) の配下は誰も所有しない。
- 純関数 `metadata_ownership(favorites, common_excluded) -> MetadataOwnership` に置き、
  次の全部が同じ結果を使う: 各 supervisor の除外 root と所有範囲、
  Ctrl+G の絞り込み集合、お気に入りの編集の件数、§6.3 の指紋。
- 各 supervisor の除外 root = 共通除外 + 「自分の root より深い、索引対象の他お気に入りの root」。
  同じ root の非所有者は metadata の走査をしない (similar の watcher 共有だけは維持)。

### 検索と件数 (利用者から見て変えない)

- Ctrl+G でお気に入り F を選んだときの絞り込み集合を、「F の root と同じか、その内側に root を持つ
  索引対象のお気に入りの ID 全部」にする。所有者は最も深い root なので、F の配下のファイルは
  必ずこの集合のどれかが所有している。F の外のファイルを所有する ID は入らない。
- お気に入りの編集の件数も同じ集合の合計にする (今の「配下の件数」と同じ意味を保つ)。
  合計欄は ID ごとの和のままで二重に数えない。

### 走査の突き合わせ先を「自分の ID の行」から「自分の所有範囲の行」に変える

- 所有範囲 = 自分の root の配下から、自分の除外 root (共通除外 + 入れ子) の配下を除いた範囲。
  walker の DB 側一覧 (`list_favorite_files`) を、`favorite_id` ではなく **パスの範囲** で引く
  (主キー `path` の範囲検索。除外 root を差し引いた複数の区間を SQL で引き、除外配下の行は
  取得しない。外側が内側の全行を読む重複を避けるため。取得行数と時間を perf に出す)。
- 差分の判定に「持ち主の不一致」を加える: 範囲内の行で `favorite_id` か `favorite_root` が
  自分と違うものは、mtime/size が同じでも取り込み直す (upsert が両方を自分に書き換え、
  Tantivy の favorite_id も更新される)。FS に無ければ削除する。
- これで所有者の移し替えは、新しい所有者の走査が自然に行う。別の付け替え工程は持たない。
  入れ子を足した・外した・root を変えたときは、除外 root が変わった側の走査
  (§6.3 の指紋も変わる) と新しい側の走査がそれぞれ自分の範囲を片付ける。
  行を先に消す処理が無いので、既に FS から消えたファイルの Tantivy 文書も、
  範囲の持ち主の走査が削除候補として拾う (walker の削除候補は DB 行から作る、
  [search_walker.rs:199](../src/search_walker.rs))。所有範囲の外に出た行 (root の変更、共通除外の
  拡大) は走査が見ないので、次節の作り直しの手順 3 で消す。
- **不完全観測では削除しない**: `read_dir` 失敗などで観測が不完全な走査は、
  削除候補を作らない (取り込み・更新は確認できた分だけ行う)。現在は `read_dir` 失敗を診断に
  記録するだけで、その配下の DB 行を全部「FS に無い」として削除する
  ([search_walker.rs:255](../src/search_walker.rs)、[search_walker.rs:199](../src/search_walker.rs))。
  NAS の一時切断などで索引が消える既存の欠陥でもあるので、ここで直す。walker は観測の完全性を
  typed な結果で返し、§6.2 の印もこれを使う。

### お気に入りを外したとき (OFF・削除) の purge

- Tantivy の削除を `favorite_id` の term で行い、SQLite も `favorite_id` で消す。
  今の purge は SQLite の行を列挙してパスごとに Tantivy から消す
  ([indexer_manager.rs:750](../src/indexer_manager.rs))。これだと、列挙と削除の間に別の持ち主が
  同じパスを取り込んだ文書を消してしまう。また、取消で SQLite に反映されなかった
  Tantivy だけの文書を消せない。ID の term で消せば、今その ID を持つ文書だけが消える。

### 実行中の構成変更: 重なるグループを止めて作り直す

設計の簡素化 (CLAUDE.md) の「閉じて作り直す」に従う。動いている supervisor を新しい所有範囲へ
追従させる処理は作らない。

- **実効 metadata 状態** = 「`auto_index_metadata` が ON で、かつ所有者の規則で走査する側」
  (同じ root の非所有者は OFF 扱い)。稼働対象の決定、構成の比較、spawn の 3 か所でこれを使う
  (今は保存フラグを比べる、[indexer_manager.rs:497](../src/indexer_manager.rs))。
- 構成 = 各お気に入りの (UUID、正規化 root、実効 metadata、実効 similar) と共通除外 root。
  変化したお気に入りがあれば、その旧 root と新 root の **どちらかと重なる** (同じ・祖先・子孫)
  お気に入りを推移的に集めたものを「グループ」とする。共通除外が変わったときは全お気に入り。
  重ならないお気に入りは今と同じく触らない。並べ替え・改名は構成を変えない。
- グループの作り直しは manager の worker で 1 本ずつ行い、UI スレッドは待たない。
  待っている間に来た変更は最新の構成 1 つにまとめる。手順:
  1. グループの各 supervisor に **書き込みを済ませて止まる** 停止 (`StopMode::Drain`) を送る。
     走査は次の確認点で止めるが、writer へ投入済みのバッチは返信を待ち、SQLite まで反映してから
     抜ける。アプリ終了時の停止 (`StopMode::Shutdown`、今の取消) とは型で分ける
     (今は取消で返信待ちをやめ SQLite 反映を省く、[fts_writer_dispatcher.rs:190](../src/fts_writer_dispatcher.rs)、
     [ingest_worker.rs:185](../src/ingest_worker.rs))。停止を始める時点で similar へ
     `watch_unavailable` を知らせる (今は watcher を落とすだけで、次の `begin_watch` まで古い Ready が
     残る、[indexer_supervisor.rs:599](../src/indexer_supervisor.rs)、[similar_index.rs:2490](../src/similar_index.rs))。
  2. 全員の join を待つ。
  3. 掃除 (Tantivy First: Tantivy の commit / reload が成功してから SQLite):
     - 削除・OFF になった UUID、**root が変わった UUID**: Tantivy を `favorite_id` の term で全削除、
       SQLite も `favorite_id` で削除。root が変わった方は手順 4 で新 root を全走査する。
       ID の term で消すので、取消で SQLite に載らなかった Tantivy だけの文書も消える。
     - それ以外のグループの UUID: SQLite で「その UUID の行のうち新しい所有範囲の外」を列挙し、
       パスごとに Tantivy から消してから SQLite から消す (共通除外を広げた場合など)。
  4. 新しい構成でグループの supervisor を作る (初回走査は §6 の規則どおり)。
- 今の「停止を待たずに作る」「OFF の purge を停止前に行う」([indexer_manager.rs:553](../src/indexer_manager.rs)、
  [app.rs:26826](../src/app.rs)) はこの経路に置き換えて無くす。
- App 側の呼び出し順は `similar.configure` → `sync_with_favorites` に揃える
  ([app.rs:25709](../src/app.rs) の順。お気に入りの編集画面の呼び出し順
  [favorites_editor.rs:1050](../src/ui_dialogs/favorites_editor.rs) も揃える)。
- 起動時は前回の構成が手元に無いので、起動時 reconciliation で「どの実効 metadata お気に入りの
  所有範囲にも入らない行」と「UUID の所有範囲の外にある行」を、手順 3 の後者と同じ方法で消す。
  この掃除で行を消したパスがあれば、そのパスの **今の所有者の root は印に関わらず全走査** する
  (消した文書を取り込み直すのはその走査だけなので。新しい版 → 旧版 → 新しい版の往復で、
  旧版が所有 ID を書き換えた場合もここで直る)。掃除は対象行数と時間を perf に出す。
  必要なら `(favorite_id, path)` の索引を追加する (既存 DB への索引追加は `CREATE INDEX IF NOT EXISTS`
  で、行の意味は変えない)。

#### 停止と再構成の契約 (設計レビュー 4 回目)

- **停止状態は 1 つの共有値で、`Running → Drain → Shutdown` の向きにだけ進む**。通知は
  非 blocking (今の Stop 送信は blocking、[indexer_supervisor.rs:166](../src/indexer_supervisor.rs))。
  Drain 中の返信待ちは Shutdown を定期的に確認し、Shutdown になったら今の取消と同じく抜ける。
- アプリ終了時の 4 秒の期限 ([indexer_manager.rs:804](../src/indexer_manager.rs)) は、
  manager が持つ supervisor だけでなく **再構成 worker が止めている最中の handle と、
  再構成 worker 自身** にも適用する。Shutdown の後は再作成をしない。期限を過ぎたものは今と同じく
  detach する。同期 `read_dir` には上限が無いので、Drain が終わる時間は保証しない
  (Shutdown が来れば期限で切る)。
- **Drain の結果は typed に返す**: `Drained` (投入済みバッチがすべて Tantivy と SQLite に反映済み) /
  `Failed` (SQLite 更新失敗・reader reload 失敗などを含む。今はログだけで成功扱い、
  [ingest_worker.rs:199](../src/ingest_worker.rs)、[fts_writer_dispatcher.rs:383](../src/fts_writer_dispatcher.rs)) /
  `Shutdown`。`Failed` の UUID は、手順 3 の ID term による削除だけを行い (失敗の影響を受けない)、
  範囲外の行の掃除は行わず、印を消して手順 4 で全走査させる。`Shutdown` なら再構成を中止する。
- **再構成は進行中の構成 snapshot を固定する**。手順 1〜4 の途中で来た変更は待ち行列で
  最新の 1 つにまとめ、今の再構成が終わってから次の再構成として最初から行う
  (途中でグループを差し替えない)。今の `favorite_info` を即座に最新化する処理
  ([indexer_manager.rs:510](../src/indexer_manager.rs)) は、snapshot を確定する時点に移す。
- **停止を始めた時点で similar の watch registration を失効させる**。旧 generation の Ready・
  イベントは similar 側で拒否する (今は watch_unavailable の後も同じ generation の Ready を受理し、
  初回の watcher 起動成功は取消を確認せずに Ready を送る、
  [similar_index.rs:2569](../src/similar_index.rs)、[indexer_supervisor.rs:351](../src/indexer_supervisor.rs))。
  再登録時の既存の gap 修復は維持する。
- アプリ終了時の取消で Tantivy だけに反映された文書は、今もある問題
  (search-architecture §4.1 終了応答性) で、扱いを変えない。範囲の持ち主の走査は FS にあるファイルを
  取り込み直すので、残るのは「終了と同じ時期にファイルも消えた」場合だけ。

### 名前索引

- 主キーが `(favorite_root, path)` で奪い合いは無い。入れ子の二重走査は残る (§8)。
- ただし今の ON/OFF の扱いに 2 つ欠陥がある: OFF の clear は旧 supervisor の join 後に走るが、
  ON はすぐ新 supervisor を作るので、OFF→ON を続けると新しい走査の後に古い clear が走り得る
  ([app.rs:5279](../src/app.rs)、[app.rs:25685](../src/app.rs))。また clear は root 単位
  ([search_index_db.rs:258](../src/search_index_db.rs)) なので、同じ root の別 UUID を OFF にすると
  有効な方の行まで消す。§6 の印が残ると、次回起動で空の索引を使い続ける。
- 修正: 名前索引の supervisor を **正規化 root 単位** で 1 つにする (同じ root のお気に入りが
  複数あっても 1 つ)。停止・clear・起動は root ごとに 1 本の順番待ちで行う。clear するのは、
  その root を使う `auto_index_structure` のお気に入りが 1 つも残らないときだけで、
  clear と印の削除は同じトランザクションにする。

## 4. B: 名前索引の単一走査と書き込み省略

- 起動時・overflow・手動の **フル走査** で、Pass 1 / Pass 2 を 1 回の深さ優先走査にまとめ、
  各フォルダの `read_dir` 1 回で「子フォルダ」と「索引対象の子 (Folder / ZipFile / PdfFile)」を
  同時に得る。進捗の分母は前回のフォルダ数 (DB から数える) を目安にし、無ければ件数だけ出す。
- 各フォルダで、DB の直下行 (`path, display_path, display_name, kind, mtime`) と今回の子集合が
  同じなら `upsert_children` を呼ばない (トランザクションも開かない)。name の mtime は現在
  常に 0 で ([name_bulk_indexer.rs:326](../src/name_bulk_indexer.rs))、意味は変えない。
- stale 掃除を「`favorite_root` の範囲で、**親フォルダが今回の訪問集合に無い** かつ
  `updated_at < scan_start_stamp`」に変える。後半の条件は、走査中に別の書き手 (同じ root の別 UUID、
  watcher) が入れた新しい行を守る既存の役割を残すため。比較と削除は DB の同じ lock 内で行う。
  訪問集合は「除外されず、子の一覧を実際に観測したフォルダ」だけ。通常は root 自身を含むが、
  root が共通除外の配下なら含めない (今も root が除外配下なら列挙しない、
  [folder_tree.rs:967](../src/folder_tree.rs))。DB と同じ正規化をした **列挙時のパス** で持つ
  (循環検出用の正規化パス集合は使わない、[fs_entry.rs:139](../src/fs_entry.rs))。
  共通除外の配下を消す既存の動き、入れ子の別 `favorite_root` の保護、空フォルダ、
  取消・不完全観測で消さないことは維持する。
- watcher 経由の部分走査 (`apply_single_change` / `run_subtree_scan`) は変えない
  (書き込み省略を入れず、既存の stamp prune のまま)。部分走査の root 自身の行は親側の更新が
  持つ契約で、親の訪問集合方式にすると誤って消すため ([name_index_supervisor.rs:517](../src/name_index_supervisor.rs))。

## 5. C: サイドカー判定のための stat を、候補があるときだけにする

- `external_metadata::sidecar_signature` は候補 4 つを `std::fs::metadata` で順に調べる
  ([external_metadata.rs:41](../src/external_metadata.rs))。サイドカーの無い画像では 4 回とも失敗する。
- 判定は **フォルダ単位** にする。walker は各フォルダの `read_dir` 結果を見て、
  「サイドカーになり得るエントリ」が 1 つも無いフォルダでは、そのフォルダの画像すべてで
  `None` を返す (stat しない)。1 つでもあれば、そのフォルダの画像は既存の `sidecar_signature`
  (stat 版) をそのまま呼ぶ。値の作り方は一切変えない。
- 「サイドカーになり得るエントリ」(保守的に広く取る):
  - 拡張子が ASCII の大文字小文字を無視して `json` のもの、または `txt` で **始まる** もの
    (`x.txtold` のような長い名前は 8.3 の短い名前で `.TXT` になり、候補 `<stem>.txt` が
    短い名前として解決され得るため。`.json` は 4 文字なので短い名前の拡張子にならない)。
  - 拡張子に ASCII 以外の文字を含むもの (Windows の名前比較は大文字小文字の変換表を使い、
    Rust の小文字化と同じとは限らないので、判定できないものは「あり」に倒す)。
  - 種類 (ファイル・フォルダ・リンク) は問わない (stat 版が判定する)。
- 短い名前 (8.3) の別名: 自動生成の短い名前も、`SetFileShortNameW` で明示した短い名前も、
  一覧 (`DirEntry::file_name` は長い名前) には出ない。そこで「サイドカーになり得るエントリが無い」
  フォルダでも、**8.3 の形になり得る候補** (stem が UTF-16 で 8 文字以下・拡張子 3 文字以下・
  ドットが 1 つ以下。文字種は見ない) だけは stat する。`.json` は拡張子 4 文字なので該当せず、
  該当し得るのは `<stem>.txt` だけ。`IMG_0001.jpg` なら `IMG_0001.txt` の 1 回、
  `ComfyUI_00001_.png` のような長い名前なら 0 回になる。stat した結果の値は既存と同じ
  (`sidecar_signature` の候補順で、短い名前の候補より前の候補は一覧から無いと分かっている)。
- 画像ごとに名前を突き合わせる細かい判定はしない。画像名の比較の同値性 (短い名前の別名、
  Unicode の大文字小文字) を保証しにくく、サイドカーのあるフォルダでは今と同じ費用で足りる。
  効くのは「サイドカーが 1 つも無いフォルダ」で、生成画像の出力フォルダの多くがこれに当たる。
- 他の経路 (watcher 差分の 1 件、ingest 時のテキスト読み取り) は変えない。

## 6. E: 終了していた間の変更を確認しない設定

### 6.1 利用者から見た挙動

- 設定 `skip_offline_change_scan` (既定 false)。表示文言の案:
  「起動時に、mIV を終了していた間の変更を確認しない」。補足:
  「常駐して使っている場合に起動を速くします。終了中に追加・削除・移動したファイルは、
  検索結果に反映されないことがあります。その場合は [今すぐ確認] を押してください。」
- 置き場所: お気に入りの編集の索引の節 (トレイ常駐の案内の隣、
  [favorites_editor.rs:440](../src/ui_dialogs/favorites_editor.rs)) と、環境設定の
  タスクトレイ常駐ページ ([pages.rs:7400](../src/ui_dialogs/preferences/pages.rs))。
  値の所有者は `Settings` 1 つ。両方の既存文言「終了すると次回起動時に再スキャン」は、
  設定 ON のときに事実と異なるので書き換える。
- **[今すぐ確認]** ボタン: §6.5。設定 ON では「再起動すれば確認される」という今の回避策
  ([favorites_editor.rs:14](../src/ui_dialogs/favorites_editor.rs)) が効かないので、同時に必須。
- 設定は起動時の判断にだけ使う。実行中の supervisor には影響しない。

### 6.2 「一度でも完全に走査できた」印

```
scanned_once(k, scope) = { fingerprint }    // 行が無い = 印なし
```

- k = fts / name / similar。scope は fts / name では root、**similar では有効 root 集合全体で 1 つ**
  (similar の Full は DB 全体の inventory を読み、未訪問の行を全体で消すので、root ごとに
  部分省略すると省いた root の行を消してしまう。[similar_db.rs:2154](../src/similar_db.rs)、
  [similar_db.rs:2541](../src/similar_db.rs))。
- **立てる**: その scope の Full が **完全に** 終わったときだけ。完全 = 取消なし・不完全観測なし
  (walker の未記録 entry error も含む、[search_walker.rs:261](../src/search_walker.rs))・
  全書き込み成功・prune 成功。既存の `initial_scan_done` は失敗でも立つので
  ([indexer_supervisor.rs:388](../src/indexer_supervisor.rs)) 使わず、系統ごとに typed な完了結果を
  返させ、それだけで判断する。設定 ON / OFF に関わらず立てる
  (OFF で使っていた人が ON にした直後の起動から省けるように)。
- **消す**: 索引ストアが作り直された・交換された・再構築されたとき (fts の rebuild pending、
  Tantivy の新規作成・schema 再構築 ([fts_index.rs:361](../src/fts_index.rs))、similar の作り直し)。
  消したことをその起動の判断に渡す (同じ起動で「印あり」と読まない)。
- **イベント・クラッシュ・強制終了・電源断・処理途中の終了では消さない**。終了中や処理途中の
  変更の取りこぼし、終了時に捨てる watcher の待ち行列 (search-architecture §4.1 終了応答性)、
  Tantivy First の SQLite 側更新漏れ (§4.2) が次回起動で直らないことは、この設定の利用者が
  受け入れる範囲 (2026-10-02 利用者決定)。
- 書き込みは既存 DB の通常の書き込みでよい (失われても次回走査するだけ)。

### 6.3 指紋

指紋が変わったら印は無効 (= 従来どおり走査)。「変わると既存の索引では足りない」入力だけを含める。

- fts: root の正規化パス、自分の UUID、§3 の除外 root (入れ子除外を含む)、`INDEX_VERSION`、
  走査対象の拡張子集合 (Susie が申告する拡張子を含む、[folder_tree.rs:137](../src/folder_tree.rs))。
- name: root の正規化パス、除外 root、名前索引の版 (現在定数が無いので新設し、走査・分類の
  意味を変えたら上げる運用をコメントに書く)。
- similar: 有効 root 集合 (正規化・重複除去後)、共通除外 root (正規化・整列・重複除去後。
  similar は除外の変化を構成変更として扱う、[similar_index.rs:2384](../src/similar_index.rs))、
  DB schema / hash / page order の版、走査対象の拡張子集合。プロセス内でだけ数える値 (構成 epoch、PDF パスワードの revision) は使わない。
- 指紋を作る関数は系統ごとに 1 つにし、含めた入力の一覧をその関数のコメントに置く。
- 新しい版 → 旧版 → 新しい版と戻した場合、旧版も起動時に全走査して索引を最新にするので、
  残った印で省いても索引は旧版終了時点の状態になる。取りこぼしの範囲はこの設定の想定内。

### 6.4 系統ごとの省略

- **fts**: 起動時 reconciliation (search-architecture §4.3、Failed 行の掃除・版不一致の再構築) は
  省かない。省くのは初回の `run_initial_scan` だけ。所有範囲が変わった root は指紋 (除外 root・UUID)
  が変わるので走査され、§3 の持ち主の移し替えもその走査が行う。perf `initial_scan_done` に
  `skipped=true` を付ける。
- **name**: 初回の `run_full_scan` だけを省く。watcher 起動は従来どおり先。
- **similar**: 省略してよいのは、要求が純粋な `Initial` で、現在の構成の全 watch が `Ready`
  (Unavailable を含む terminal ではなく Ready)、未修復の gap が無いときだけ。
  Manual / Overflow / WatchRecovery / Reconfigure / SummaryRepair が合流したら通常の Full。
  省略時は Full の inventory 読み込みと列挙だけを行わず、
  build 残骸の掃除 (`cleanup_incomplete_if`) と、有効 root の外の行の purge を行う。
  similar DB は行ごとに「どのお気に入り root のものか」を持たない
  ([similar_db.rs:4801](../src/similar_db.rs)、[similar_db.rs:4827](../src/similar_db.rs)) ので、
  purge は「公開済みの item / container、build、prefill のキーのうち、現在の有効 root の和集合
  (共通除外を除く) の外にあるものを消す」API を新設する。包含関係 (入れ子の root) を保護し、
  変更履歴と watermark は既存の purge と同じく更新する。
  そのうえでメモリ読み込みを行い、**現在の store とメモリ snapshot の array ack を待ってから**
  Complete にする ([similar_index.rs:3032](../src/similar_index.rs))。
  実装は既存の worker loop の中で行い、job の結果を typed に `ScannedFull` / `ReusedInitial` と
  区別する。Full 完了処理の dirty の吸収 (`dirty.retain_after`) と gap の修復
  ([similar_index.rs:1872](../src/similar_index.rs)) は `ScannedFull` だけに適用する。
  watch 登録後に届いた dirty は捨てずに残し、既存の `MoreWork` で Delta に回す。
  page order の修復 ([similar_index.rs:2834](../src/similar_index.rs)) は維持する。
  独立した Complete の発行経路は作らない。
- いずれの系統でも、起動後の overflow・watcher 回復・手動確認は従来どおり Full。

### 6.5 [今すぐ確認]

- 入口は App に 1 つ置く (`App::request_index_full_check`)。名前索引の handle は App が持つので、
  ここで集約すれば名前索引を IndexerManager の初期化に依存させずに済む。
  - fts: 各 supervisor へ **metadata だけの** 全走査要求 (今の `FullRescan` は similar にも Manual を
    送る、[indexer_supervisor.rs:494](../src/indexer_supervisor.rs)。全体確認ではこれを分け、
    similar への要求を 1 回にする)。
  - name: 各 supervisor へ `FullRescan` (現在 handle に要求 API が無いので追加、
    [name_index_supervisor.rs:63](../src/name_index_supervisor.rs))。
  - similar: 全 root に対して 1 回だけ `FullReason::Manual` (registration を持たない入口を追加)。
- 要求は非 blocking。進捗は既存の索引進捗表示に出る。IndexerManager が初期化中なら
  fts / similar の分は完了後に 1 回だけ実行する予約にし、利用不能 (初期化失敗) なら
  その分は行わず理由を出す (名前索引の分は実行する)。
  常駐中の一時停止 (pause) 中は要求を受け付け、再開後に走る (既存の pause の扱いに従う)。

### 6.6 永続データ

3 つの DB はいずれもリリース済み。変更はテーブルの追加だけ (`CREATE TABLE IF NOT EXISTS`)。
旧版で作られた DB には印が無い = 従来どおり走査。既存行の意味は変えない (§3 の持ち主の
移し替えは既存の列を通常の upsert で書き換えるだけで、スキーマ変更ではない)。
リリース済みの DB を開いて印のテーブルが追加され、既存行が保たれ、印が無いので走査すること、を
テストで固定する。

## 7. テスト

- A: 入れ子 2 お気に入りで 2 回連続の初期 scan を回し、2 回目の取り込みが 0 であること。
  外側・内側それぞれで Ctrl+G 絞り込みをしたとき、配下のファイルが過不足なく 1 件ずつ出ること。
  持ち主の移し替え: 他の ID の行は取り込み直され Tantivy の favorite_id が所有者になる、
  FS から消えたファイルは Tantivy からも消える、同じ UUID で root を変えたとき favorite_root が
  更新される。不完全観測 (read_dir 失敗) の走査で削除が起きないこと。
  同一 root の重複 (UUID 最小が所有)、並べ替えで所有者が変わらない、内側の追加・削除・OFF・
  パス変更を実行中に行ったとき重なるグループだけが作り直され、重ならないお気に入りは動き続けること、
  Drain 停止で投入済みバッチが SQLite まで反映されてから掃除と再作成が行われること、
  root を変えた UUID の旧 root の文書 (Tantivy だけのものを含む) が消えること、
  共通除外を広げたとき範囲外の行が消えること、同じ root に UUID の小さいお気に入りを足したとき
  旧勝者が止まり新しい所有者だけが走ること、待っている間の連続変更が最新構成 1 つにまとまること、
  起動時に所有範囲外の行が消えること。
  OFF の purge が ID の term で行われ、同じパスを取り込み直した別の持ち主の文書を消さないこと。
  停止開始時に similar へ watch_unavailable が届くこと。
  名前索引: OFF→ON の連続で古い clear が新しい走査の後に走らないこと、同じ root の別 UUID を
  OFF にしても有効な方の行が残ること。
- B: 変化の無い 2 回目のフル走査で `upsert_children` が 0 回。root が共通除外の配下・root が
  存在しない場合に既存どおり古い行が消えること。フォルダ削除・空になったフォルダ・
  深い subtree 削除で stale 行が消えること (既存 `tests/search_name_e2e.rs` を維持)。
  走査中に別の書き手が入れた行が消えないこと。祖先の大文字小文字だけ変えたとき display_path が
  更新されること。不完全観測で prune しないこと。
- C: サイドカーになり得るエントリが無いフォルダで stat を呼ばないこと (関数分離か呼び出し計数)。
  あるフォルダでは既存と同じ値になること。「なり得る」の判定: 大文字の拡張子、`.txtold`、
  拡張子に ASCII 以外を含む名前、`x.jpg.json` がフォルダの場合。8.3 の形の候補だけ stat すること
  (`IMG_0001.jpg` は 1 回、長い名前は 0 回)、短い名前を明示設定したファイルが従来どおり検出されること
  (短い名前を作れない環境ではテストを skip せず、8.3 判定の純関数テストで代える)。
- E: 印あり + 指紋一致で初回 Full が走らないこと / 指紋不一致・印なし・設定 OFF で走ること /
  完全な Full でだけ印が立ち、取消・不完全観測・書き込み失敗では立たないこと /
  ストアの作り直しで印が消え、同じ起動で省かないこと / 付け替え行のある root は走査すること /
  similar: watch が Unavailable・Manual 合流のときは省かない、省略時も purge・build 掃除・
  array ack 後の Complete が起きる、watch 登録後の dirty が Delta で処理される /
  省いた起動の後も watcher 差分・overflow の Full が従来どおり動くこと。
- [今すぐ確認]: 3 系統に要求が届くこと、初期化中の予約、利用不能時の無効化。
- 文言変更があるので UI スナップショットの該当があれば更新。

## 8. 段 3: 走査の統合 (D) は計測後に決める

段 1 の後も、設定 OFF (既定) の利用者は起動ごとに 3 系統の走査を払う。候補:

- D1: アイテム索引と名前索引の走査を、お気に入りごとに 1 回の列挙へまとめる
  (両者は同じ「お気に入り単位の常駐スレッド」なので形が近い。名前索引は今
  `IndexerManager` より先に独立起動しており、まとめると Ctrl+S の準備が IndexerManager の
  初期化に引きずられる点が論点)。watcher もお気に入りあたり 2 本 → 1 本になる。
- D2: similar を含む 3 系統の起動時走査を、ボリュームごとに 1 本ずつ順番に走らせる
  (同じ HDD で同時に走らせない)。後から走る系統は OS のキャッシュに当たる。
- similar の走査を他と共有する案は、similar の並列走査・gap barrier と所有境界が大きく違い、
  費用に見合うか不明なので候補から外す。

段 1 の後、利用者に OS 再起動直後のログ (通常ログの `initial scan done` 行、similar の
`reconcile terminal` 行) を取ってもらい、系統ごとの所要時間と走査の重なりを見てから、
D1 / D2 / 不要を決める。計測には cold / warm、ボリュームごとの重なり、UI 応答性、
ピークメモリを含める。現時点では、C で comfyui の走査 (サイドカーなし画像 × 4 回の stat) が
どれだけ縮むかが未計測で、D の効果を見積もれない。

## 9. 文書

- [search-architecture.md](search-architecture.md) §4.1 / §4.3 / §4.4 に A・B・E を反映。
- [async-architecture.md](async-architecture.md) に名前索引の root 単位の順番待ちと印の所有者、
  similar の reconcile 文書に省略時の契約 (`ReusedInitial`)。
- search-architecture §4.2 の「次回起動の walker が補修する」説明に、設定 ON では補修されない旨。
- [sidecar-metadata-ingest.md](sidecar-metadata-ingest.md) の検出コストの説明 (C)。
- [docs/README.md](README.md) に本書を登録。
- spec.md に設定項目、マニュアル (お気に入り・検索の索引のページ) に設定とボタン。
  通信・保存先は変わらないので privacy.html / 製品ページ安心セクションは対象外。
- CHANGELOG.md は公開準備で記入。

## 10. S1 実装・検証記録 (2026-10-02)

対象は §4 B と §5 C のみ。§3 A・§6 E は未実装。

- B: フル走査を単一 DFS に変更し、直下の五列が同じフォルダは transaction を開かず置換を省く。
  訪問集合は列挙パスの DB key とし、循環検出の canonical key とは分離した。
  prune は未訪問の親かつ開始 stamp より古い行だけを、比較中から同じ DB lock を保持して削除する。
  削除がある場合は 1 transaction にまとめる。watcher の部分走査は変更していない。
  旧 DFS の `._*` ディレクトリ内への走査範囲も、再帰先と索引対象を別に集めて維持した。
- C: 拡張子の保守的判定と 8.3 の形の判定を純関数に分離した。候補があるフォルダは既存の
  `sidecar_signature` を使い、無いフォルダは該当する `<stem>.txt` だけを確認する。
  署名の生成処理を共有し、mtime と fingerprint の意味・優先順位は変えていない。
  列挙途中にエラーがあれば不在を確認できないため、既存の stat 経路を使う。
- 独立実装レビューで C の不完全一覧からの不在判定を指摘され、上記の経路と回帰テストを追加。
  再レビューで解消確認済み。設計からの逸脱はない。既存スキーマ・行の意味は変更していない。
- 追加回帰は 19 件 (bulk 8、DB 5、walker 6)。skip を含まない 8.3 純関数テストを維持した。
  この環境では使い捨て fixture に `fsutil file setshortname` で `.TXT` の別名も設定できたため、
  追加の非対話 probe で「一覧に候補拡張子が無い状態の検出・不変走査・削除後の差分」を確認した。

自動検証 (すべて exit 0、失敗・ignore 0):

| コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j 1 -p mimageviewer --lib name_bulk_indexer` | 14 |
| `cargo test -j 1 -p mimageviewer --lib search_index_db` | 35 |
| `cargo test -j 1 -p mimageviewer --lib search_walker` | 21 |
| `cargo test -j 1 -p mimageviewer --lib external_metadata` | 21 |
| `cargo test -j 1 -p mimageviewer --test search_name_e2e` | 15 |
| `cargo test -j 1 -p mimageviewer --test search_metadata_e2e` | 12 |
| `rustc --test tests/startup_scan_shortname_probe.rs ...` → `target/debug/deps/startup_scan_shortname_probe.exe --nocapture` (一時 probe) | 1 |

全体 `cargo fmt` 実行後、`cargo fmt --check` 成功。`python scripts/check_ui_glyphs.py` は問題 0。
Cargo には `MSBUILDDISABLENODEREUSE=1` を設定し、lib テスト前に指定の FFmpeg DLL 6 本を
`target/debug/deps` に配置した。初回の native dependency build は並列実行で停止したため、
生成済みの turbojpeg CMake build を `--parallel 1` で完了し、Cargo を `-j 1` で再実行した。
テスト実行前の build failure であり、製品テストの失敗ではない。
テストログは `target/startup-scan-s1/` に保存 (最初の bulk テストはツール出力で確認)。
一時 probe は既に検証済みの debug rlib を使用し、FFmpeg と Windows import library の検索パス、
`-C target-feature=+crt-static -C linker=rust-lld.exe` を指定した。最初の直接リンクは Windows
import library の検索パス不足で停止し、検索パスを追加して成功した。
probe のソースは `target/startup-scan-s1/startup_scan_shortname_probe.rs` に保存し、通常のテスト
対象からは除いた (短い別名を作れない環境では、恒久的な純関数テストで検証する)。

確認用ビルドは `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定し、
`.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功した。
core・remote service・EPUB worker を生成し、CRT 検査も `runtime=4 pe=3` で成功した。
不足していた承認済み EffeTune Mixwright v0.11.1 は、主作業ツリーの既存 bundle から
実ファイルをコピーした。双方に reparse point がないことを確認し、依存版は変更していない。
アプリ起動・実データでの性能計測は行っていない。

コミットは未作成。共通 Git 管理領域
`C:/home/mimageviewer/.git/worktrees/mimageviewer-startscan/index.lock` への書き込みが
環境の許可範囲外で拒否され、`git add` が失敗した。指定の Co-Authored-By を末尾に含む
コミット文は `target/startup-scan-s1/commit-message.txt` に準備済み。
