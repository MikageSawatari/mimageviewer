# 起動時の索引スキャン軽減 (v4.3.0)

状態: 設計第 2 版 (実装前レビュー 1 回目の指摘と利用者決定を反映)。ブランチ `v430-startup-scan`。

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
- A の実行中の構成変更は、既存の「変わったものだけ作り直す」を、止め切ってから作り直す
  1 本の再構成経路に置き換える (§3)。経路を 1 本にして、止める途中・付け替え途中の組み合わせを
  増やさない。

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
  次の全部が同じ結果を使う: 各 supervisor の除外 root、起動時・再構成時の付け替え、
  Ctrl+G の絞り込み集合、お気に入りの編集の件数。
- 各 supervisor の除外 root = 共通除外 + 「自分の root より深い、索引対象の他お気に入りの root」。
  同じ root の非所有者は metadata の走査をしない (similar の watcher 共有だけは維持)。

### 検索と件数 (利用者から見て変えない)

- Ctrl+G でお気に入り F を選んだときの絞り込み集合を、「F の root と同じか、その内側に root を持つ
  索引対象のお気に入りの ID 全部」にする。所有者は最も深い root なので、F の配下のファイルは
  必ずこの集合のどれかが所有している。F の外のファイルを所有する ID は入らない。
- お気に入りの編集の件数も同じ集合の合計にする (今の「配下の件数」と同じ意味を保つ)。
  合計欄は ID ごとの和のままで二重に数えない。

### 付け替え (Tantivy に孤児を残さない)

- 所有者と異なる `favorite_id` を持つ行は、**消さずに** 所有者へ付け替える:
  `favorite_id` / `favorite_root` を所有者にし、`mtime` を一致し得ない値にする。
  これで所有者の walker がその行を「FS にある → 取り込み直し (Tantivy の favorite_id も更新)」
  または「FS に無い → 削除 (Tantivy も削除)」のどちらかで必ず処理する。行を先に消すと、
  既に FS から消えたファイルの Tantivy 文書を誰も消せなくなる (walker の削除候補は DB 行から作る、
  [search_walker.rs:199](../src/search_walker.rs))。
- 所有者がいない行 (お気に入りの削除・metadata OFF) は、既存の無効化時 purge の扱いのまま。
- 付け替えは 1 回の SQLite トランザクション。途中で落ちても行は残り、次回の付け替えか walker が拾う。
  Tantivy First (§4.2) の「SQLite が古い側へずれるのは安全」に収まる。

### 実行中の所有構成の変更 (1 つの再構成経路)

所有構成 = 正規化した (UUID, root) の集合 (索引対象のお気に入りだけ)。これが変わる操作は、
お気に入りの追加・削除・パス変更・metadata の ON/OFF。並べ替え・改名では変わらない。

- `IndexerManager` に再構成を 1 本だけ持つ。所有構成が変わったら、
  **影響を受ける supervisor** (除外 root か所有の有無が変わったもの + 追加・削除・OFF の当事者) を
  まとめて止め、それらのスレッド終了と writer への投入済みジョブの完了を待ち、
  付け替え (と OFF の当事者の purge) を行ってから、影響を受けたものを作り直す。
  影響の無い supervisor はそのまま動かし続ける (今の「変わったお気に入りだけ作り直す」挙動を保つ)。
- 待ちは UI スレッドでしない。再構成は manager の worker で行い、実行中に次の変更が来たら
  最新の構成 1 つにまとめる (途中の構成は捨てる)。
- 今の `sync_with_favorites` の「停止を待たずに新 supervisor を作る」「OFF の purge を停止前に行う」
  ([indexer_manager.rs:553](../src/indexer_manager.rs)、[app.rs:26823](../src/app.rs)) は、この経路に
  置き換えて解消する。
- 名前索引は主キーが `(favorite_root, path)` で奪い合いは無い。入れ子の二重走査は残る (§8)。

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
  訪問集合は root 自身を含み、DB と同じ正規化をした **列挙時のパス** で持つ
  (循環検出用の正規化パス集合は使わない、[fs_entry.rs:139](../src/fs_entry.rs))。
  共通除外の配下を消す既存の動き、入れ子の別 `favorite_root` の保護、空フォルダ、
  取消・不完全観測で消さないことは維持する。
- watcher 経由の部分走査 (`apply_single_change` / `run_subtree_scan`) は変えない
  (書き込み省略を入れず、既存の stamp prune のまま)。部分走査の root 自身の行は親側の更新が
  持つ契約で、親の訪問集合方式にすると誤って消すため ([name_index_supervisor.rs:517](../src/name_index_supervisor.rs))。

## 5. C: サイドカー判定のための stat を、候補があるときだけにする

- `external_metadata::sidecar_signature` は候補 4 つを `std::fs::metadata` で順に調べる
  ([external_metadata.rs:41](../src/external_metadata.rs))。サイドカーの無い画像では 4 回とも失敗する。
- walker は各フォルダの `read_dir` 結果から、`.json` / `.txt` の名前を小文字化した集合を作る。
  画像ごとに候補 4 つの名前を小文字化して引き、**1 つも当たらなければ `None`** (stat しない)。
  1 つでも当たれば既存の `sidecar_signature` (stat 版) をそのまま呼ぶ。値の作り方は一切変えない。
- 同値性: 集合に無い名前は、大文字小文字を区別しないフォルダでも区別するフォルダでも stat が
  成功しない。区別するフォルダで綴りが違う場合は、集合に当たって stat 版へ回り、stat 版が既存どおり
  判定する。リンクも名前が一覧に出るので stat 版へ回る。find-data の古さは値に関与しない。
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
- similar: 有効 root 集合 (正規化・重複除去後)、DB schema / hash / page order の版、走査対象の
  拡張子集合。プロセス内でだけ数える値 (構成 epoch、PDF パスワードの revision) は使わない。
- 指紋を作る関数は系統ごとに 1 つにし、含めた入力の一覧をその関数のコメントに置く。
- 新しい版 → 旧版 → 新しい版と戻した場合、旧版も起動時に全走査して索引を最新にするので、
  残った印で省いても索引は旧版終了時点の状態になる。取りこぼしの範囲はこの設定の想定内。

### 6.4 系統ごとの省略

- **fts**: 起動時 reconciliation (search-architecture §4.3、Failed 行の掃除・版不一致の再構築) と
  §3 の付け替えは省かない。付け替えた行がある root は、印があっても走査する (付け替えの後始末は
  走査が担うため)。省くのは初回の `run_initial_scan` だけ。perf `initial_scan_done` に
  `skipped=true` を付ける。
- **name**: 初回の `run_full_scan` だけを省く。watcher 起動は従来どおり先。
- **similar**: 省略してよいのは、要求が純粋な `Initial` で、現在の構成の全 watch が `Ready`
  (Unavailable を含む terminal ではなく Ready)、未修復の gap が無いときだけ。
  Manual / Overflow / WatchRecovery / Reconfigure / SummaryRepair が合流したら通常の Full。
  省略時は Full の inventory 読み込みと列挙だけを行わず、
  build 残骸の掃除 (`cleanup_incomplete_if`)、構成に無い root の行の purge (purge 対象は
  「DB にある root のうち有効 root 集合に無いもの」を DB から列挙して渡す。現在の
  `purge_roots_except_if` は入力が空だと何もしない、[similar_db.rs:2966](../src/similar_db.rs))、
  メモリ読み込みを行い、**現在の store とメモリ snapshot の array ack を待ってから** Complete にする
  ([similar_index.rs:3032](../src/similar_index.rs))。
  watch 登録後に届いた dirty は **捨てずに Delta に残す** (実走査をしていないので
  `dirty.retain_after` で吸収しない)。`required_gap_epoch` を実走査なしで修復済みにしない。
- いずれの系統でも、起動後の overflow・watcher 回復・手動確認は従来どおり Full。

### 6.5 [今すぐ確認]

- manager レベルの API を 1 つ置く: `request_full_check_all()`。fts の各 supervisor へ
  `FullRescan`、name の各 supervisor へ `FullRescan` (現在 handle に要求 API が無いので追加、
  [name_index_supervisor.rs:63](../src/name_index_supervisor.rs))、similar へ `FullReason::Manual`
  (registration を持たない manager 側から全 root に対して出せる入口を追加)。
- 要求は非 blocking。進捗は既存の索引進捗表示に出る。IndexerManager が初期化中なら
  完了後に 1 回だけ実行する予約にし、利用不能 (初期化失敗) ならボタンを無効にして理由を出す。
  常駐中の一時停止 (pause) 中は要求を受け付け、再開後に走る (既存の pause の扱いに従う)。

### 6.6 永続データ

3 つの DB はいずれもリリース済み。変更はテーブルの追加だけ (`CREATE TABLE IF NOT EXISTS`)。
旧版で作られた DB には印が無い = 従来どおり走査。既存行の意味は変えない (§3 の付け替えは
既存の列の値の整理で、スキーマ変更ではない)。

## 7. テスト

- A: 入れ子 2 お気に入りで 2 回連続の初期 scan を回し、2 回目の取り込みが 0 であること。
  外側・内側それぞれで Ctrl+G 絞り込みをしたとき、配下のファイルが過不足なく 1 件ずつ出ること。
  付け替え: FS に残るファイルは取り込み直され Tantivy の favorite_id が所有者になる、
  FS から消えたファイルは Tantivy からも消える、付け替え後・走査前に落としても次回に処理される。
  同一 root の重複 (UUID 最小が所有)、並べ替えで所有者が変わらない、内側の追加・削除・OFF・
  パス変更を実行中に行ったとき影響する supervisor だけが止まり、止まり切ってから付け替えること、
  再構成中の連続変更が最新構成 1 つにまとまること。
- B: 変化の無い 2 回目のフル走査で `upsert_children` が 0 回。フォルダ削除・空になったフォルダ・
  深い subtree 削除で stale 行が消えること (既存 `tests/search_name_e2e.rs` を維持)。
  走査中に別の書き手が入れた行が消えないこと。祖先の大文字小文字だけ変えたとき display_path が
  更新されること。不完全観測で prune しないこと。
- C: 候補が一覧に無い画像で stat を呼ばないこと (関数分離か呼び出し計数)。候補がある場合の値が
  既存と同じこと (大文字の拡張子、stem が無い名前、`x.jpg.json` がディレクトリ、リンク、Unicode 名)。
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
- [async-architecture.md](async-architecture.md) に再構成経路と印の所有者、similar の reconcile
  文書に省略時の契約。
- [docs/README.md](README.md) に本書を登録。
- spec.md に設定項目、マニュアル (お気に入り・検索の索引のページ) に設定とボタン。
  通信・保存先は変わらないので privacy.html / 製品ページ安心セクションは対象外。
- CHANGELOG.md は公開準備で記入。
