# 起動時の索引スキャン軽減 (v4.3.0)

状態: 設計第 7 版 (途中失敗は再起動で作り直す割り切りに簡素化、2026-10-02 利用者決定)。ブランチ `v430-startup-scan`。

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
  1. グループの各 supervisor を今と同じ停止 (cancel) で止める。停止を始める時点で similar へ
     `watch_unavailable` を知らせる (今は watcher を落とすだけで、次の `begin_watch` まで古い Ready が
     残る、[indexer_supervisor.rs:599](../src/indexer_supervisor.rs)、[similar_index.rs:2490](../src/similar_index.rs))。
  2. 全員の join を待つ。
  3. 掃除 (Tantivy First: Tantivy の commit / reload が成功してから SQLite):
     - 削除・OFF になった UUID、**root が変わった UUID**: Tantivy を `favorite_id` の term で全削除し、
       SQLite も `favorite_id` で削除する。root が変わった方は手順 4 で新 root を全走査する。
       ID の term で消すので、停止で SQLite に載らなかった Tantivy だけの文書も消える。
     - それ以外のグループの UUID: SQLite で「その UUID の行のうち新しい所有範囲の外」を列挙し、
       パスごとに Tantivy から消してから SQLite から消す (共通除外を広げた場合など)。
     - 共通除外の拡大: 新たに除外する prefix の raw STRING `path` を範囲 query で
       Tantivy から消して commit / reload し、同じ範囲を SQLite から消す。
       SQLite に未反映の文書も対象にする。起動時の最初の snapshot も現在の共通除外を適用する。
  4. 新しい構成でグループの supervisor を作る (初回走査は §6 の規則どおり)。
- 今の「停止を待たずに作る」「OFF の purge を停止前に行う」([indexer_manager.rs:553](../src/indexer_manager.rs)、
  [app.rs:26826](../src/app.rs)) はこの経路に置き換えて無くす。
- App はお気に入りと PDF パスワードを同じ構成 snapshot として manager に提出する。
  manager worker が固定した snapshot で similar の構成を反映し、その後に supervisor を作る。
  App が待ち行列の各変更を先に similar へ反映しない (OFF→ON の集約で、稼働中 watch の
  registration を失わないため)。manager が利用不能な場合だけ、既存の degraded bootstrap
  として similar を直接構成する。お気に入りの編集・パスワード変更・起動時もこの経路に揃える。
- 起動時は前回の構成が手元に無いので、起動時 reconciliation で「どの実効 metadata お気に入りの
  所有範囲にも入らない行」と「UUID の所有範囲の外にある行」を、手順 3 の後者と同じ方法で消す。
  この掃除で行を消したパスがあれば、そのパスの **今の所有者の root は印に関わらず全走査** する
  (消した文書を取り込み直すのはその走査だけなので。新しい版 → 旧版 → 新しい版の往復で、
  旧版が所有 ID を書き換えた場合もここで直る)。掃除は対象行数と時間を perf に出す。
  必要なら `(favorite_id, path)` の索引を追加する (既存 DB への索引追加は `CREATE INDEX IF NOT EXISTS`
  で、行の意味は変えない)。

#### 停止と再構成の契約

- アプリ終了時の 4 秒の期限 ([indexer_manager.rs:804](../src/indexer_manager.rs)) は、
  manager が持つ supervisor だけでなく **再構成 worker が止めている最中の handle と、
  再構成 worker 自身** にも適用する。終了の後は再作成をしない。期限を過ぎたものは今と同じく detach する。
- **再構成は進行中の構成 snapshot を固定する**。手順 1〜4 の途中で来た変更は待ち行列で
  最新の 1 つにまとめ、今の再構成が終わってから次の再構成として最初から行う
  (途中でグループを差し替えない)。今の `favorite_info` を即座に最新化する処理
  ([indexer_manager.rs:510](../src/indexer_manager.rs)) は、snapshot を確定する時点に移す。
- **停止を始めた時点で similar の watch registration を失効させる**。旧 generation の Ready・
  イベントは similar 側で拒否する (今は watch_unavailable の後も同じ generation の Ready を受理し、
  初回の watcher 起動成功は取消を確認せずに Ready を送る、
  [similar_index.rs:2569](../src/similar_index.rs)、[indexer_supervisor.rs:351](../src/indexer_supervisor.rs))。
  再登録時の既存の gap 修復は維持する。
- **途中で失敗したときは「次回起動で索引を作り直す」に割り切る** (2026-10-02 利用者決定:
  稀な例外のために実装を複雑にしない)。手順 3 の掃除 (Tantivy / SQLite の書き込み) が失敗したら、
  再試行や部分的な回復はせず、
  1. 「次回起動時にアイテム索引を作り直す」印を fts_meta に立てる (起動時は `INDEX_VERSION` 不一致と
     同じ経路で `files` を作り直し Tantivy を wipe する。今の rebuild pending の仕組みを流用する)。
  2. 利用者に「索引の更新に失敗しました。mIV を再起動すると索引を作り直します。」と知らせる
     (既存の通知の仕組みを使う)。
  3. グループの supervisor はそのまま作り直す (検索は動き続ける。索引の一部が古いのは再起動まで)。
  印を立てる書き込み自体も失敗したら、ログと通知だけにする。
  名前索引の clear が失敗したときはログだけにする (外したお気に入りの行は、Ctrl+S が有効な
  お気に入りの範囲でしか検索しないなら害が無い。実装時にこの前提を確認し、違えば同じく作り直しの印へ)。
- 停止は今の cancel のまま (書き込みを済ませてから止まる特別な停止は作らない)。停止の瞬間に
  処理中だったバッチは、Tantivy だけに反映されて SQLite に載らないことがある (今もある、
  search-architecture §4.1 終了応答性)。FS にあるファイルは範囲の持ち主の走査が取り込み直し、
  手順 3 の ID term 削除はこの文書も消す。ただし FS に残っていても所有範囲外へ移る
  Tantivy-only 文書は walker が観測せず、SQLite のパス列挙でも消せない。共通除外の拡大では
  上の path 範囲 query で削除する。所有範囲内の文書が SQLite に未反映のまま FS からも
  消えた場合の残存は従来どおりで、扱いを変えない。
- 第 5〜6 版にあった「書き込みを済ませてから止める停止 (Drain) と、その typed な結果」
  「掃除失敗時の自動再試行」「ID 削除前の SQLite 無効化」は、上の割り切りで不要になったので削除した。


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
- **構成変更が旧印の範囲のデータを削除・書き換える前にも消す**。削除と印の失効は同じ
  DB transaction。3系統とも別指紋 Full は最初の行変更より前に失効を確定する。構成を元へ戻し、
  Full の途中で正常終了しても旧印を再利用しない。fts の起動補修の must-scan root も
  同じ削除 transaction で印を失効させる。通常 watcher の削除と、変更が無い起動整理は保持する。
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
  公開済み item / container の削除は同じ transaction で完走印を失効させる。
  build / prefill だけの整理では完走印を保持する。
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
  cancel 停止後の join と、放棄された旧バッチも追い越さない cleanup の完了後に再作成されること、
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

## 11. S2 実装・検証記録 (2026-10-02)

### 11.1 最初の部分実装 (既に受け入れ・コミット済み)

S2 全体は未完了。今回の独立した変更単位は、S1 レビュー残件 P3 と、§3 の
「不完全観測では削除しない」契約 (S2 brief の項目 3) のみ。
実行中の再構成は manager / supervisor / ingest / dispatcher / similar と名前索引の
停止所有をまとめて変更する規模のため、利用者が許可した coherent green subset の
区切りを使う。所有規則を一部の consumer だけへ配線する変更は入れていない。

- `search_walker::ScanResult` に `ObservationCompleteness::Complete / Incomplete` を追加。
  `ScanDiag` の read_dir、iterator entry、file_type、特殊エントリ分類、metadata、深さ制限の
  失敗から完全性を決め、Incomplete では `to_delete` を作らない。
  観測できた新規・変更候補の取り込みは従来どおり。取消は成功結果にせず、既存の Err を返す。
  root が存在せず read_dir が失敗した場合も、不在を確定せず既存行を保持する。
- `fs_entry::try_classify_dir_entry` で、Windows の特殊エントリ分類内の属性取得失敗を
  walker へ伝える。既存の `classify_dir_entry` は従来の fallback を保つ wrapper とした。
  UI 同期 I/O、常駐 worker、永続スキーマ・行の意味の変更はない。
- private な `WalkerIo` 境界へ実 FS と決定的な失敗注入をつなぎ、全観測失敗での削除禁止と
  新規・変更取り込みの維持、次の完全走査で通常削除へ戻ることを固定した。
- P3 は `external_metadata` の両経路が通る stat 境界を cfg(test) の thread-local counter で
  計数。実 `scan` で長い画像名 0 回・`IMG_0001.jpg` 1 回を検証し、同じ境界で旧関数の
  4 回も陽性対照として検証する。walker が旧関数の直呼びへ戻ると 0 / 1 の assertion が落ちる。
- `indexer_supervisor::run_initial_scan` のログへ完全性と追加の診断件数を反映。
  `initial_scan_done` の意味は変えていない。完全性の型は FS 観測だけの結果なので、
  S3 の印には書き込み・prune の typed 成功結果との組み合わせが必要。
- 新規回帰は walker 9 件と `search_metadata_e2e` 1 件の計 10 件。
  統合テストは使い捨て fixture の root を移動して read_dir 失敗を作り、SQLite / Tantivy の
  両方の保持と、復旧後の通常取り込み・削除まで確認する。追加テストは timing / sleep に依存しない。
- 独立レビュー (`gpt-6.1-sol` / `xhigh`) は設計と完成差分を確認し、必須修正の指摘なし。
  実装・自動検証は親の実装担当が所有し、reviewer は Cargo を重複実行していない。

未実装 (S2 brief の項目番号):

- 1: `metadata_ownership` と全 consumer の接続 (最深 root、UUID 同点決着、共通除外、実効 metadata)。
- 2: 除外を差し引く複数 SQL range の walker diff、所有 ID / root 不一致の再取り込み、perf。
- 4: OFF / 削除の favorite_id term purge と Tantivy First。
- 5: Ctrl+G の所有 filter set と、お気に入り件数への同じ集合の適用。
- 6: 推移的な重複グループ、固定 snapshot・最新要求合体の manager worker、単調な StopMode、
  typed DrainOutcome と失敗伝播、join 後の掃除・再作成、worker 所有 handle を含む 4 秒 shutdown、
  similar registration の revoke と旧 generation 拒否、App 呼出順統一。
- 7: 起動 reconciliation の所有範囲外掃除、強制 Full の root 集合、perf / 必要な索引の計測。
- 8: 名前索引の正規化 root 単位の supervisor と停止・clear・起動の直列化、共有 root の保護、
  起動・編集の統合、UUID 進捗の解決、S3 の marker 削除 hook。
- §6 E / S3 は未着手。Drain / Shutdown / coalescing / revoked registration の gate 付き回帰は、
  項目 6 と同じ変更単位で追加する。今回の結果を S2 全体の acceptance として扱わない。

自動検証 (すべて exit 0、失敗・ignore 0。以下の記録済み実行の合計 111 件):

| コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j 1 -p mimageviewer --lib search_walker` | 30 |
| `cargo test -j 1 -p mimageviewer --lib external_metadata` | 21 |
| `cargo test -j 1 -p mimageviewer --lib fs_entry` | 7 |
| `cargo test -j 1 -p mimageviewer --lib indexer_supervisor` | 11 |
| `cargo test -j 1 -p mimageviewer --lib name_bulk_indexer` | 14 |
| `cargo test -j 1 -p mimageviewer --test search_metadata_e2e` | 13 |
| `cargo test -j 1 -p mimageviewer --test search_name_e2e` | 15 |

`MSBUILDDISABLENODEREUSE=1` を設定し、既存の `target/debug/deps` の FFmpeg DLL を使用した。
最初の walker 実行は結果の捕捉が不足したため、コンパイル終了後に同じコマンドを再実行して
ログを保存した (表の 30 件は記録済みの再実行分)。
`cargo check -j 1 -p mimageviewer --bin mimageviewer-core` も exit 0。
依存の初回検査と native build を含め 11 分 38 秒かかったが、timeout / 中断はない。
全体 `cargo fmt` と `cargo fmt --check` は成功、`python scripts/check_ui_glyphs.py` は問題 0。
workspace 全体のテスト、アプリ起動、実データによる対話検証は行っていない。
ログは `target/startup-scan-s2/` に保存した。

確認用 build は `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定し、
`.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で exit 0。
通常 feature set の core・remote service・EPUB worker を生成し、CRT 検査は
`runtime=4 pe=3` で成功した。core の最適化コンパイルは 8 分 39 秒。
成果物は起動していない。通常 `%APPDATA%\mimageviewer` の設定・データには触れていない。

Git の書き込み・stash・コミットは利用者の指定どおり行っていない。
今回の部分実装専用のコミット文を `target/startup-scan-s2/commit-message.txt` に保存した。
末尾は指定の `Co-Authored-By: Codex GPT-6.1 Sol <noreply@openai.com>`。
ブランチは `v430-startup-scan`、HEAD は `9da28d637` のまま。

### 11.2 残りの S2: 第7版に合わせた実装

上の未実装一覧は 11.1 時点の記録。今回の対象は §3 A の残り全体であり、
§6 E / S3 の初回走査を省く印・設定・手動確認ボタンには着手していない。
第5〜6版の途中差分から Drain / DrainOutcome・自動 retry・SQLite 無効化を除去し、
cancel → join → cleanup → spawn の第7版へ統一した。

- `metadata_ownership` は最深 root・同一 root の UUID 文字列順・共通除外を純関数で
  決める。実効状態、所有範囲、除外、Ctrl+G の filter set、件数、再構成グループを共有する。
- walker は除外を差し引いた複数の path range を SQL で取得し、取得件数・時間を perf に
  出す。UUID / favorite_root 不一致は同じ mtime/size でも取り込み直す。失敗行も範囲で
  読んで再取り込み・不在削除へ回す。不完全観測の削除禁止は受け入れ済みの契約を維持する。
- `metadata_reconfiguration` が handle を唯一所有し、固定 snapshot の重複グループを
  直列に再構成する。後続要求は最新の1つへ集約する。Shutdown は停止中の control も
  取消し、worker 自身とその所有 handle を同じ4秒期限に含める。終了後の spawn はない。
- 削除・OFF・root 変更は favorite_id term purge、それ以外は新所有範囲外の path cleanup。
  旧バッチの応答を cancel で放棄しても追い越さないよう、cleanup は同じ Background FIFO
  に提出する。commit / reader reload 成功後だけ SQLite を削除する。失敗は既存 rebuild
  pending を立てて指定文言で通知し、再試行せず supervisor を再作成する。
- 起動時の所有範囲外 cleanup は全 supervisor の前に走り、強制 Full が必要な現在の owner
  root 集合を返す。`(favorite_id, path)` は EXPLAIN で covering index が選択されることを
  確認して `CREATE INDEX IF NOT EXISTS` で追加した。
- similar の停止による再登録待ちは `Revoked` として通常の `Unavailable` から分離した。
  旧 generation の Ready / event を拒否し、新登録は同じ gap を引き継ぐ。再登録前に
  degraded Full を走らせないため、交代に必要な repair Full は1回になる。通常の障害時の
  Unavailable Full と復旧後の gap repair は維持する。これは独立レビューとの構造合意済み。
- `NameIndexManager` は既存 DB と同じ `normalize_path` の root 単位で supervisor を所有し、
  停止・join・clear・起動を直列化する。同じ root の有効 UUID が残れば clear しない。
  起動と編集は同じ経路、UUID の進捗は root monitor へ解決する。clear transaction 内に
  S3 の印削除 hook を残す。Full 完了・観測結果は S3 が成功判定へ使える型で保持する。
- Ctrl+S の前提確認では、現行検索が OFF のお気に入り root も含み、空集合では全 root を
  検索することが分かった。第7版の指示に従い、name clear 失敗時も専用 rebuild pending
  を立て、次の writable open で名前行と印を同じ transaction で削除する。検索範囲の
  既存機能は変えていない。readonly open は印を消費しない。
- App の共通呼出 owner は similar.configure → metadata sync の順に統一し、name sync も
  同じ owner へ接続した。
  本棚保存先の環境設定変更も、設定採用後に新しい共通除外を一度提出する。実効保存先が
  同じなら再構成しない。UI で join・走査・DB 書き込みを追加していない。

追加回帰は 42 件 (lib 41、metadata 統合1)。snapshot/coalescing、worker 所有 handle の
Shutdown、旧バッチ FIFO、name stop/clear/start、revoked registration は gate / channel
で競合地点を固定した。独立レビューは設計と差分を確認し、未修正の必須指摘は0。
自動検証の最終コマンド・件数は以下へ記録する。

以下は記録済み成功分の合計235件 (失敗・ignore 0)。すべて
`MSBUILDDISABLENODEREUSE=1`、Cargo は `-j 1` を指定した。
lib のコマンドは `cargo test -j 1 -p mimageviewer --lib <filter>`。

| filter | 成功件数 |
| --- | ---: |
| `metadata_ownership` | 3 |
| `metadata_reconfiguration` | 9 |
| `search_walker` | 31 |
| `fts_meta` | 19 |
| `fts_writer_dispatcher` | 8 |
| `ingest_worker` | 14 |
| `indexer_manager` | 11 |
| `indexer_supervisor` | 14 |
| `name_index_manager` | 7 |
| `name_index_supervisor` | 7 |
| `name_bulk_indexer` | 15 |
| `search_index_db` | 38 |
| `ownership_count_tests` | 1 |
| `preferences_books_root` | 1 |
| `preferences_unchanged_effective_books_root` | 1 |
| `similar_index::tests::incremental_reconcile` | 25 |
| `similar_index::tests::revoked_watch` | 1 |
| `similar_index::tests::stopped_watch` | 1 |

| integration コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j 1 -p mimageviewer --test search_metadata_e2e` | 14 |
| `cargo test -j 1 -p mimageviewer --test search_name_e2e` | 15 |

metadata 統合テストは入れ子の不変2回目の走査に加え、同じ stamp の UUID / root 不一致を
SQLite / Tantivy 双方へ作り、実 walker → IngestSession で両ストアの所有者と検索 filter を
修復すること、FS に無い別所有者の文書を両方から消すことも確認した。

`cargo check -j 1 -p mimageviewer --bin mimageviewer-core` は exit 0。
最後のコード変更後に全 workspace `cargo fmt` と `cargo fmt --check` が成功し、
`python scripts/check_ui_glyphs.py` は問題0。`git diff --check` も成功。
途中の check / lib コンパイルでは旧 stop module の残り配線、削除した型・API参照、
test import の不足を修正した。manager の最初の実行は10成功・1失敗で、watch交代時に
Fullが余分に走ることを検出した。上の Revoked 境界の修正後は期待値を緩めず11件成功。
ログは `target/startup-scan-s2/*-v7.log` と `*-final.log` に保存した。
全 workspace suite、アプリ起動、実データの性能計測は行っていない。

設計からの逸脱はない。name rebuild 印・Background FIFO・Revoked の型分離は、
第7版の指示・既存機能保持・停止所有の境界を具体化した上記の実装判断である。
S2 の残項目はない。S3 の起動省略の印と§6の機能は未着手。

確認用 build は `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定して
`.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` が exit 0。
通常 feature set の core・remote service・EPUB worker を生成し、CRT 検査は
`runtime=4 pe=3` で成功した。core の最適化コンパイルは10分46秒。
成果物は起動していない。実 `%APPDATA%\mimageviewer` のデータには触れていない。
ログは `target/startup-scan-s2/build-dev-v7.log`。

Git の書き込み・stash・コミットは行っていない。残り S2 専用のコミット文を
`target/startup-scan-s2/commit-message.txt` に上書きし、指定の Co-Authored-By を末尾にした。

### 11.3 S2 独立レビューの5件修正

対象は受け入れコミット `3bd690d55` への ACCEPT WITH CHANGES の5件。
範囲と稀な失敗時の方針は第7版のまま。

- similar の構成 owner を metadata 再構成 worker へ移した。App はお気に入り・共通除外・
  immutable な PDF password store clone を1つの snapshot として提出する。worker が
  その固定 snapshot を similar へ反映し、新しい supervisor が watch を登録する。
  待機中の OFF→ON が集約されても、触らないお気に入りの Ready を失わない。
  起動中の変更は manager の採用時に最新 snapshot を再提出する。metadata manager が
  利用不能なときだけ、既存の degraded bootstrap を維持する直接 configure を残した。
- 独立レビューで同型の root 表記変更も確認した。metadata の所有範囲の root は末尾区切りを
  除くが、similar の既存保存キーは区切りを保持するため、similar が有効な構成比較には
  そのキーも含めた。末尾区切りだけの変更でも watch を再登録し、永続キーは変更しない。
- Tantivy 0.26.1 の `IndexWriter::delete_query` と raw STRING path の `RangeQuery` が
  inverted index の全一致 term を対象にできることをローカル source と回帰で確認した。
  新しい共通除外 prefix の root 自身・子孫を半開区間で削除し、commit / reload の成功後に
  同じ SQLite 範囲を transaction で削除する。旧 submitted batch を追い越さない
  Background FIFO を使い、SQLite に未反映の文書も消す。通常の本棚設定変更では rebuild
  を強制しない。範囲 cleanup の書き込み失敗は既存の marker・log・通知へ渡し、retry はない。
- Ctrl+G は選択 UUID の存在と保存 metadata ON を先に検証し、OFF・削除なら選択解除する。
  子だけが ON でも外側の OFF を維持しない。同じ root の非所有者は保存 ON なら選択可能。
- metadata の編集画面進捗も、件数と同じ ownership filter set で集約する。いずれかの
  owner が Full 中なら作成中、全 owner の初期走査が済めば監視中とする。未登録 owner は
  起動待ちとして扱い、別 root の進捗は混ぜない。
- `list_path_owners` は path / favorite_id の2列だけにし、実際の SELECT の EXPLAIN が
  `COVERING INDEX idx_files_fav_path` になることをテストで固定した。
  `metadata_out_of_range_cleanup` perf event に `owner_rows`・`query_ms`・
  `owner_rows_bytes` を追加。bytes は Vec の capacity × tuple サイズと全 String の
  capacity の合計という取得バッファの推定確保量で、プロセス全体や SQLite cache の量ではない。

追加回帰は10件 (runtime2、writer2、fts_meta2、Ctrl+G2、編集進捗2)。
runtime の OFF→ON 集約・root alias 変更、submitted-but-not-applied batch と prefix cleanup
の競合地点は gate / channel で固定した。独立 source review の追加 alias 指摘も修正済みで、
未修正の必須指摘は0。以下へ最終検証結果を記録する。

自動検証は合計144件成功、失敗・ignore 0。`MSBUILDDISABLENODEREUSE=1`、
Cargo は `-j 1` を指定。lib コマンドは
`cargo test -j 1 -p mimageviewer --lib <filter>`。

| filter | 成功件数 |
| --- | ---: |
| `metadata_reconfiguration` | 11 |
| `fts_meta` | 21 |
| `fts_writer_dispatcher` | 10 |
| `favorite_filter_` | 2 |
| `ownership_count_tests` | 3 |
| `indexer_manager` | 11 |
| `indexer_supervisor` | 14 |
| `ingest_worker` | 14 |
| `similar_index::tests::incremental_reconcile` | 25 |
| `similar_index::tests::revoked_watch` | 1 |
| `similar_index::tests::stopped_watch` | 1 |
| `preferences_books_root` | 1 |
| `preferences_unchanged_effective_books_root` | 1 |

| integration コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j 1 -p mimageviewer --test search_metadata_e2e` | 14 |
| `cargo test -j 1 -p mimageviewer --test search_name_e2e` | 15 |

修正後の `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` は exit 0。
全 workspace の `cargo fmt` / `cargo fmt --check`、`git diff --check` は成功し、
`python scripts/check_ui_glyphs.py` は問題0。
ログは `target/startup-scan-s2/review-fixes-*.log` に保存。
全 workspace suite・アプリ起動・実データの性能計測は行っていない。

確認用 build は `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定し、
`.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` が exit 0。
core・remote service・EPUB worker と CRT 検査 (`runtime=4 pe=3`) が成功した。
core の最適化コンパイルは10分29秒。ログは
`target/startup-scan-s2/review-fixes-build-dev.log`。成果物は起動していない。
レビュー修正専用の `target/startup-scan-s2/commit-message.txt` を指定署名付きで上書きし、
Git の書き込み・stash・コミットは行っていない。

### 11.4 S3: 終了中の変更確認の省略と手動確認 (2026-10-02)

第7版 §6 E を3系統とも実装した。既定 OFF の `skip_offline_change_scan` を既存 Settings
保存経路へ追加し、欠落キーは false として読む。お気に入り編集と環境設定の常駐ページが
同じ値を編集する。検索語と anchor、終了後の再走査を断定していた説明も更新した。
共通の純描画 helper に設定・補足・[今すぐ確認] を置き、Light/Dark の snapshot を追加した。

- リリース済みの3 DB に `CREATE TABLE IF NOT EXISTS` で `scanned_once` だけを追加する。
  fts/name は root 単位、similar は全有効 root の集合で1印。既存の行の意味・保存先は変えず、
  各旧DBを開いて既存行が保たれ、印が無く通常の走査へ進む互換テストを追加した。
- 指紋は各系統に1関数と入力一覧コメントを置いた。fts/name は安定 JSON tuple、similar は
  JSON tuple の SHA-256。similar の拡張子は画像・Susie・ZIP/CBZ/PDF に加え、本の分類に
  関与する既存の動画・音声の集合も含める。プロセス epoch/password revision は含めない。
- 印は typed Complete (取消・不完全観測・書き込み失敗・prune失敗なし) だけで更新する。
  通常イベント・途中終了・通常Full/Deltaの失敗では以前の印を保持する。S2 の Failed 行の
  起動 cleanup と再構築要求は消し、同じ起動の判断へ反映する。Failed root は重複除去して
  1 root 1回だけ消す。retry loop や新しい段階的な復旧は追加しない。
- fts の新規作成/schema再作成は、古い管理行の stamp が stable と判定されると文書を再投入
  できない。store open 境界で管理行と印を同一 transaction で消し、現在の起動で復元する。
  失敗は既存 rebuild pending と通知へ渡す。独立レビューとこの根本修正に合意し、実文書が
  復元されるまでの回帰を追加した。
- 起動設定は構成の集約対象から分離して worker 開始時に固定する。name も最初の構成だけ
  省略を許し、実行中の除外変更・OFF/ON等の再構成では通常Fullへ戻す。watcher は省略前に
  起動する。再構成・手動・overflow・回復を省かない。
- similar は既存 worker 内で `ScannedFull / ReusedInitial` を分ける。pure Initial・全Ready・
  gapなしだけを再利用する。未完build掃除、共通除外を差し引くroot和集合外のpurge、ページ順
  修復とメモリ読み込みを維持し、現在storeのarray ack後に同じComplete経路へ進む。
  dirty吸収とgap修復はScannedFullだけに適用し、再利用中のdirtyはDeltaへ残す。
- [今すぐ確認] は App の1入口。名前は独立managerのmailboxへ即時提出、metadata/similarは
  App初期化中の予約を1回へ集約し、共有managerの構成採用後にmetadata-only Fullとsimilar
  Manual 1回を発行する。初期構成の採用前にも要求を失わない。pause中は受付を保ち再開後に
  実行し、共有manager利用不能時は理由を通知して名前の要求を維持する。
- 設計簡素化: 手動確認をモーダルにすると索引更新中の通常閲覧を妨げるため採用しない。
  新しいworker/復旧stateを作らず、既存構成ownerのmailboxと既存完了経路へ集約した。
- 名前bulkのComplete前提を確認し、深さ上限、特殊DirEntry分類失敗、root metadataの
  NotFound以外の失敗を不完全観測へ伝えた。NotFoundは既存仕様どおり完全な空走査とする。
  watcherによるroot消滅は名前行だけを消し、完走印は保持する。

独立レビュー (`gpt-6.1-sol` / `xhigh`) は設計と完成差分を確認した。store再作成のinventory、
起動設定の集約、初期構成前のManual、nameの再構成時省略、通常失敗の印保持の指摘を修正し、
決定的な回帰を追加した。未解決のP1/P2は0。Cargo検証は親の実装担当が所有する。
similarのUnavailable/Manual合流・array ack・dirty、構成採用と手動予約はchannel gateで
競合地点を固定し、処理時間の推測で順序を決めない。

自動検証の成功分は373件 (lib 285、integration 29、snapshot 59)。
`MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定し、Cargo は `-j 1`。
lib のコマンドは `cargo test -j 1 -p mimageviewer --lib <filter>`。

| filter | 成功件数 |
| --- | ---: |
| `metadata_reconfiguration` | 14 |
| `fts_meta` | 24 |
| `fts_index` | 27 |
| `indexer_supervisor` | 17 |
| `indexer_manager` | 13 |
| `name_index_manager` | 10 |
| `name_index_supervisor` | 13 |
| `name_bulk_indexer` | 16 |
| `search_index_db` | 42 |
| `similar_db` | 60 |
| `similar_index::tests::startup_` | 9 |
| `similar_index::tests::incremental_reconcile` | 25 |
| `index_full_check_tests` | 3 |
| `skip_offline_change_scan` | 1 |
| `preferences::search_index` | 11 |

similar_db の既存手動用3件 (`measure_delta_scoped_reconcile_candidate_scaling`、
`measure_full_reconcile_inventory_reference_cardinality`、`migrate_a_real_v1_store_copy`)
は ignore のまま。新規に ignore を追加していない。

| integration コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j 1 -p mimageviewer --test search_metadata_e2e -- --test-threads=1` | 14 |
| `cargo test -j 1 -p mimageviewer --test search_name_e2e -- --test-threads=1` | 15 |
| `cargo test -j 1 -p mimageviewer --test ui_snapshot` | 59 |

metadata 結合テストの最初の並列実行は6成功・8件が既存10秒の初回走査待ちで timeout。
コードや待ち時間を変更せず、テストを直列実行して14件成功した。並列実行時の資源競合は
疑われるが原因確定とは扱わない。手動確認のfixture初期化と検索設定タイトル検査の失敗は
実装を修正し、それぞれ3件・11件の再実行が成功した。

新しい Light/Dark 2画像だけを `UPDATE_SNAPSHOTS=1` の対象フィルタ
`offline_change_scan_setting` で生成し、文字・折り返し・checkbox・ボタンを目視確認した。
既存snapshot画像の更新はない。その後、更新指定なしの59件が成功した。

全 workspace の `cargo fmt --all` / `cargo fmt --all --check`、`git diff --check` は成功。
`python scripts/check_ui_glyphs.py` は問題0、
`cargo check -j 1 -p mimageviewer --bin mimageviewer-core` は exit 0。
ログは `target/startup-scan-s3/` (再実行は `*-final.log` / `*-serial.log`) に保存した。
全 workspace suite・アプリ起動・実データの性能計測は行っていない。

確認用 build は `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`
が exit 0。core は8分43秒、remote service・EPUB worker も成功し、CRT 検査は
`runtime=4 pe=3`。ログは `target/startup-scan-s3/build-dev.log`。
通常 feature set の成果物は起動しておらず、実 `%APPDATA%\mimageviewer` に触れていない。
実機での省略と[今すぐ確認]の確認は利用者へ引き継ぐ。S3 の実装残件はない。

Git の書き込み・stash・コミットは行っていない。指定署名を末尾に含むコミット文を
`target/startup-scan-s3/commit-message.txt` に作成した。

### 11.5 S3 独立レビュー ACCEPT WITH CHANGES の修正

`4483e6f4d` に対する P2 3件と P3 文言1件を修正する。旧構成の完走印を残したまま構成変更が
索引を削除すると、元の構成へ戻して Full 完了前に正常終了した次の起動で、不完全な索引を
旧指紋により再利用できたことが根因。途中終了そのものではなく、構成変更のデータ変更境界で
証拠を失効させる。通常イベントと同一指紋 Full の取消で旧印を保持する許容範囲は変えない。

- fts: OFF・root変更の favorite purge、共通除外拡大の range purge、所有範囲変更と起動補修の
  path削除に専用の印失効を接続。旧行の正規化 favorite_root と現在の補修先 root を、行の削除と
  同じ SQLite transaction で消す。favorite purge は SQLite 0件でも既知の旧rootの印を消す
  (Tantivy-only文書も削除し得る)。range purge は実行がある区間と交差する root だけ消し、
  無関係root・行が無い区間・変更が無い起動整理は保持する。通常 `delete_paths` は保持する。
  must-scan 集合を返す起動補修にも同じ durable な失効を適用し、次回起動までの穴を閉じた。
  Failed cleanup も事前の現在rootだけの失効から同じ削除transactionへ統合した。
  UUIDと現在の範囲が一致して保存favorite_rootだけ旧rootである行も、旧/現在rootを消す。
- name: Full開始時、保存指紋と異なる root の印だけを transaction で消し、commit成功後に
  bulkの最初の行置換へ進む。失効の書き込み失敗なら行を変更せず Failed で終える。
- similar: 設定purgeで実際に削除する transaction に印削除を統合。別指紋 Full は既存の
  未完build掃除 transaction で旧印を消し、列挙・書き換え前に確定する。同一指紋 Full と
  no-op purge は保持する。通常 watcher は変更しない。
- OFF時 purge の ActivityGate迂回は、既存の「OFFにした範囲を検索から即座に除く」設定反映を
  維持するため許容。workerで実行されUIを待たせず、keep-root和集合と構成epochを確認する。
  pauseは索引作成を止めるもので設定による整理を保留しない。paused gateでpurgeが確定する
  回帰を追加した。新しいretry・復旧state・modalは不要で、既存transactionに失効を統合した。
- fav_add の「変更監視と起動時スキャンで自動更新」を、変更監視と起動確認を設定で選べる
  説明へ更新。設定や操作自体は変わらず、マニュアルの既存説明とも一致する。

追加回帰は13件。ftsのOFF/root/除外往復と起動補修、nameの除外往復、similarのOFF/除外往復を
実DB・既存ActivityGateまたはchannel gateで固定し、通常stop/join・DB再open・元構成の次回
起動で省略しないことを検証する。transaction失敗時rollbackと無関係/no-op印保持も検証する。
検証結果と確認buildは以下へ記録する。

独立レビュー (`gpt-6.1-sol` / `xhigh`) は上記の所有境界に合意し、完成差分を承認した。
途中レビューで見つかった無関係rangeの印失効とFailed行の旧root印残存も修正済み。
similar再起動回帰は旧DB接続を使い回さず新connectionへ変更した。未解決P1/P2は0。
Cargoと確認buildは親の実装担当が所有し、reviewerとimplementerは重複実行していない。

自動検証は `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定。
libの成功分は267件、コマンドは `cargo test -j1 -p mimageviewer --lib <filter>`。
`indexer_manager` は再実行時に `-- --test-threads=1` を追加した。

| filter | 成功件数 |
| --- | ---: |
| `fts_meta` | 27 |
| `metadata_reconfiguration` | 16 |
| `name_index_supervisor` | 16 |
| `search_index_db` | 43 |
| `similar_db` | 63 |
| `similar_index::tests::startup_` | 10 |
| `name_index_manager` | 10 |
| `indexer_supervisor` | 17 |
| `indexer_manager` | 13 |
| `similar_index::tests::incremental_reconcile` | 25 |
| `index_full_check_tests` | 3 |
| `fts_writer_dispatcher` | 10 |
| `ingest_worker` | 14 |

similar_db の既存手動用3件は ignore のまま (11.4と同じ)。追加のignoreはない。
最初のlib test compileはテスト用DB gateの直接initializer不足で停止し、補完後に成功。
最初の `indexer_manager` は12成功・1失敗で、既存の共有watch回帰がarray公開の
`0x80070005` (アクセス拒否) を報告した。コード・待ち時間を変えず直列再実行で13件成功。
原因は確定とは扱わず、最初の失敗ログも保存した。製品へretryは追加していない。

| integration コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j1 -p mimageviewer --test search_metadata_e2e -- --test-threads=1` | 14 |
| `cargo test -j1 -p mimageviewer --test search_name_e2e -- --test-threads=1` | 15 |
| `cargo test -j1 -p mimageviewer --test ui_snapshot -- --test-threads=1` | 59 |

成功分の合計は355件。今回の文言変更で既存snapshot画像の差分はなく、更新指定なしで59件成功。
永続schema・既存行の意味・保存先は変更していない。検証ログは
`target/startup-scan-s3/review-fixes/` に保存した。

最後のコード変更後に全 workspace の `cargo fmt --all` / `cargo fmt --all --check` が成功。
`python scripts/check_ui_glyphs.py` は問題0、`git diff --check` と
`cargo check -j1 -p mimageviewer --bin mimageviewer-core` は exit 0。
コミット文は指定署名付きで `target/startup-scan-s3/commit-message.txt` を上書きし、
UTF-8 BOMなし (先頭3byteを検査) とした。今回もGitの書き込み・コミットは行っていない。
全 workspace suite・アプリ起動・実データの性能測定は行っていない。

確認用 build は `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`
が exit 0。coreの最適化コンパイルは7分11秒、remote service・EPUB workerも成功し、
CRT検査は `runtime=4 pe=3`。ログは `target/startup-scan-s3/review-fixes/build-dev.log`。
成果物は起動しておらず、通常 `%APPDATA%\mimageviewer` に触れていない。

#### 追加の設計確認事項: fts の拡張子集合変更 Full

指定された4指摘の修正後、限定棚卸しで新しいP2経路を確認した。Susie非対応になった
拡張子はwalkerの候補から外れ、完全観測なら既存行をto_deleteへ回す。IngestSessionが
削除バッチを確定し通常 `delete_paths` で行を消した後に正常停止すると、FullはStoppedで
新印を立てず、旧印が残る。次回Susieの集合を戻すと旧指紋に一致し、削除済み索引を省略できる。
起動補修は既に消えた行を拾えない。通常の同一指紋Fullの取消とは区別が必要。

独立reviewerもこの経路をソースで確認した。利用者はftsを「構成変更と起動補修の削除経路だけ」
と明示しており、Full側まで広げるかは設計側へ確認した。追加案はname/similarと同じく、
別指紋Fullの最初の行変更前に旧root印を失効する小さい変更単位。retryや復旧stateは不要。
この時点では指定4件の修正と355件成功のgreen差分を保持し、追加範囲は未実装・回答待ちとした。
利用者の追加承認を受けた実装は次の §11.6 に記録する。

### 11.6 fts の別指紋 Full の統一 (2026-10-02)

利用者がnameと同じ一般則をftsにも適用すると決定した。Susie拡張子集合を含む全指紋入力を
対象に、保存印と指紋が異なるFullは最初の行変更より前に旧印を失効する。同一指紋Fullの
取消では以前の印を保持する。HEAD `84b5c141c` を基点に追加修正し、新しい復旧やretryは作らない。

- `FtsMetaDb::prepare_full_scan` が root と指紋不一致の条件付きDELETEをtransactionで確定。
  他rootと同一指紋の印は保持し、印なしでは新しい印を作らない。失効失敗なら共通Full入口で
  Failedとして終了し、walker・Tantivyバッチ・管理行の変更へ進まない。
- 共通入口 `run_initial_scan` の指紋算出直後へ接続。実際には初回に加え、FullRescan /
  metadata-only手動確認・overflow・監視回復・再構成後のFullもこの入口を通る。
- nameの稼働中Fullは `run_full_scan` の準備transactionへ集約済み。similarは実削除を伴う
  設定purgeで同transaction失効し、通常Fullは `run_index_job` の未完build掃除transactionで
  指紋不一致を失効してからinventoryと走査の書き込みへ進む。ReusedInitialは一致時だけ。
  初回・手動・Overflow・WatchRecovery・Reconfigure・SummaryRepairのproducer/consumerを
  棚卸しし、稼働中の別指紋Fullに失効を迂回する追加経路は見つからなかった。
- 指紋の比較はFull開始時に算出した集合が対象。Susie実poolの実行途中のlive再読み込みは
  開始指紋不一致とは異なる入力変化であり、この規則では新しい状態管理を加えない。
  similarのpage-order修復も調べた。現在の版は1で、修復を要する旧データは完走印導入前。
  現行版で別指紋の完走印を残して修復する経路はなく、将来の版変更時は失効境界も確認する。
- 追加回帰は4件。旧Susie申告を模す拡張子集合の印と管理行をseedし、現在の実walkerが
  非対応行を削除してバッチが確定した地点をchannelで固定する。Fullの完了判定前に通常取消、
  DB再open、元の申告集合へ戻して起動判断と実supervisorの `initial_scan_skipped=false` を確認。
  実pluginや共有poolは変更せず、元集合の注入はtest-only thread-localで他テストと分離する。
  同一指紋での削除後取消・再openは印保持/初回省略を陽性対照として確認する。
  失効DELETEの失敗で管理行と印を保持しTantivyバッチを提出しない回帰、root限定の準備も追加。

独立レビュー (`gpt-6.1-sol` / `xhigh`) が稼働中のFull全経路と完成差分を承認した。
失効失敗テストのwriter gateは、誤提出が起きても待ち続けないようreleaseを事前に切断し、
提出通知のassertionで退行を検出する。指摘は反映済み、未解決のP1/P2/P3は0。

自動検証は `MSBUILDDISABLENODEREUSE=1`・`CARGO_BUILD_JOBS=1` を設定し、
libコマンドは `cargo test -j1 -p mimageviewer --lib <filter> -- --test-threads=1`。
成功分は271件、similar_dbの既存手動用3件はignoreのまま。

| filter | 成功件数 |
| --- | ---: |
| `indexer_supervisor` | 20 |
| `fts_meta` | 28 |
| `metadata_reconfiguration` | 16 |
| `indexer_manager` | 13 |
| `fts_writer_dispatcher` | 10 |
| `ingest_worker` | 14 |
| `name_index_supervisor` | 16 |
| `search_index_db` | 43 |
| `name_index_manager` | 10 |
| `similar_db` | 63 |
| `similar_index::tests::startup_` | 10 |
| `similar_index::tests::incremental_reconcile` | 25 |
| `index_full_check_tests` | 3 |

| integration コマンド | 成功件数 |
| --- | ---: |
| `cargo test -j1 -p mimageviewer --test search_metadata_e2e -- --test-threads=1` | 14 |
| `cargo test -j1 -p mimageviewer --test search_name_e2e -- --test-threads=1` | 15 |

成功分の合計300件 (重複の再実行は数えない)。ログは
`target/startup-scan-s3/fingerprint-full/`。UI変更がないためsnapshotは実行・更新せず、
前段 §11.5 の59件成功を再利用する。全workspace suite・アプリ起動は行わない。

ハング防止修正後の `indexer_supervisor` も20件成功 (`indexer_supervisor-final.log`)。
全workspaceの `cargo fmt --all` / `cargo fmt --all --check`、`git diff --check` は成功し、
`python scripts/check_ui_glyphs.py` は問題0。
`cargo check -j1 -p mimageviewer --bin mimageviewer-core` は exit 0。
コミット文は指定署名付きで `target/startup-scan-s3/commit-message.txt` を上書きし、
先頭byte検査でUTF-8 BOMなしを確認した。今回もGitの書き込み・コミットは行っていない。

確認用 build は `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`
が exit 0。coreは6分11秒、remote service・EPUB workerも成功し、CRT検査は
`runtime=4 pe=3`。ログは `target/startup-scan-s3/fingerprint-full/build-dev.log`。
成果物は起動しておらず、通常 `%APPDATA%\mimageviewer` に触れていない。
今回承認された別指紋Fullの修正に未実装項目はない。

### 11.7 similar の補助データ整理と完走印 (2026-10-02)

HEAD `1c54be82f` の再レビューで、同一指紋の ReusedInitial が共通除外配下の prefill だけを
整理しても印を消し、その次の起動で不要な Full を実行することが判明した。
`purge_keys_if` は item / container の削除数を build / prefill の削除前に保存し、その数だけで
同じ transaction 内の印の失効を判定する。呼び出し元へ返す全テーブルの削除件数、変更履歴、
watermark、集計更新、取消時の rollback は既存契約を維持する。

- 共通除外整理と OFF 時 purge は同じ修正済み入口を通る。公開 item / container を削除する
  purge は引き続き印を消し、補助データだけの purge は印を保持する。
- 回帰は3件追加。DB テストで prefill のみ / build のみ / 両方の整理を両 purge API で確認し、
  公開索引と watermark と印を保持することを固定する。公開 container のみを削除する場合は
  印が失効することも確認する。
- worker 回帰は Full を実際に完了させ、共通除外配下の閲覧を模した prefill を保存して正常終了。
  同一構成の DB を開き直す2回の起動がどちらも ReusedInitial となり、Full inventory を読まず、
  array ack 後に Complete となることを確認する。array 要求前の停止と解除は channel gate を使う。
- 任意の prefill 保存時の共通除外チェックは見送った。現在の保存条件は検索処理と共有する
  `enabled_roots` を正本とし、共通除外は別の scheduler 設定状態にある。既存の OFF / 保存 / purge
  の順序保証を保って両者を読むには所有境界の変更が必要なため、単純な追加条件では済まない。
  今回は保存経路の責務を広げず、補助データが残っても公開索引の完走印を失わない規則を採用した。
- schema・UI・マニュアルの操作説明には変更なし。retry・新しい復旧状態・再利用時の印の再作成は
  追加しない。

初回検証では追加 fixture の前提を2点修正した。build 開始 API が作る公開 table の Building
placeholder を除かないと補助データだけにならず、空 container の公開には page_count=0 が必要。
製品コードの変更は公開 table の削除数で印の失効を判定する修正だけを維持した。
また worker 回帰は gate 停止中の DB / 進捗を取得し、解除・join 後に assert する。
旧不具合で印が消えてもテストが gate 待ちでハングせず、assert で失敗する。

最終の対象テストはすべて成功。`MSBUILDDISABLENODEREUSE=1` を設定し、lib は下記を
`cargo test -j1 -p mimageviewer --lib <filter> -- --test-threads=1` で実行した。

| filter | 成功件数 | ignored |
| --- | ---: | ---: |
| `similar_db` | 65 | 3 |
| `similar_index::tests::startup_` | 11 | 0 |
| `similar_index::tests::incremental_reconcile` | 25 | 0 |
| `prefill` | 6 | 0 |

成功分は重複を除いて106件 (`prefill` に新しい startup 回帰1件が重複)。ログは
`target/startup-scan-s3/auxiliary-tidy/*-final.log`。初回の
`cargo test -j1 -p mimageviewer --lib startup_auxiliary_only_purge -- --test-threads=1` は
fixture の前提不一致で0成功・1失敗 (`auxiliary-purge.log`)。
続く `similar_db` は64成功・1失敗・3 ignored (`similar-db.log`) で、空 container の fixture を
修正した。上表はその修正後の最終実行であり、失敗は残っていない。

全workspaceの `cargo fmt --all` と `cargo fmt --all --check`、`git diff --check` が成功。
`python scripts/check_ui_glyphs.py` は問題0。
`cargo check -j1 -p mimageviewer --bin mimageviewer-core` は exit 0 (`cargo-check.log`)。
UIに変更がないためsnapshotの実行・更新はせず、§11.5 の59件成功を再利用する。
fts / name の integration は変更範囲外で、§11.6 の成功結果を再利用する。
全workspace suite・アプリ起動・コミットは行っていない。

確認用 build は `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` が exit 0。
core は4分49秒、remote service・EPUB worker も成功し、CRT 検査は `runtime=4 pe=3`。
ログは `target/startup-scan-s3/auxiliary-tidy/build-dev.log`。成果物は起動していない。
`target/startup-scan-s3/commit-message.txt` を指定署名付きで上書きし、先頭byte検査で
UTF-8 BOMなし、末尾が指定の Co-Authored-By 行であることを確認した。
要求された失効判定と回帰に未実装項目はなく、任意の保存時除外チェックの見送り理由は上記。
