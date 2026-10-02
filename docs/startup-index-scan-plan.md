# 起動時の索引スキャン軽減 (v4.3.0)

状態: 設計 (実装前レビュー待ち)。ブランチ `v430-startup-scan`。

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
  初版の設計は「正常終了かどうか」を印で追う案だったが、mIV は WM_QUERYENDSESSION /
  WM_ENDSESSION を処理しておらず常駐中の OS 再起動では終了処理が走らないこと、
  利用者が厳密さを求めていないことから、**「一度でも完全に走査できた」印だけ** にした (§6)。
  終わり方・クラッシュ・処理途中のイベントの組み合わせを扱わなくてよくなる。
- E の「スキャンを省くか」は系統ごと・root ごとに 1 つの印だけで決める。省いた後の状態は
  「Full が完了した直後」と同じ状態に入る (専用の中間状態を作らない)。
- 設定変更時は既存どおり supervisor を作り直す経路に乗せる。印には構成の指紋を含め、
  指紋が変われば印は無効 = 通常どおり走査、で済ませる (専用の追従処理を作らない)。

## 3. A: 入れ子のお気に入りの取り込み直し

### 原因

`fts_meta.files` の主キーは `path` で、upsert のたびに `favorite_id` を上書きする
([fts_meta.rs:306](../src/fts_meta.rs))。walker の差分は自分の `favorite_id` の行だけを見る
([fts_meta.rs:398](../src/fts_meta.rs) `list_favorite_files`)。外側 (`c:\home\youtube\movie`) と
内側 (`...\movie\youtube`) の両方が `auto_index_metadata` だと、2 つの supervisor が互いの行を
「DB に無い」と判定して取り込み直し、`favorite_id` を奪い合う。ログの取り込み件数
245 + 346 = 591 = 内側の総数で、値は起動ごとに入れ替わる。watcher 経由の変更も両方が処理する。

### 修正

- **所有者の規則**: パス p の所有者は、`auto_index_metadata=true` のお気に入りのうち p を含む
  **最も深い** root。同じ root が複数あれば favorites の並びで先のもの。
  この規則を 1 つの純関数 (`metadata_owner_roots(favorites) -> per-favorite excluded roots`) に置き、
  walker・watcher 差分・起動時 reconciliation の 3 か所が同じ結果を使う。
- 各 supervisor の `excluded_roots` に「自分の root より深い、索引対象の他お気に入り root」を
  足す。walker (`search_walker::walk_dir_recursive`) と watcher 差分
  (`indexer_supervisor::apply_single_change`) は既に `excluded_roots` を尊重している。
- 同じ root の重複で所有者にならないお気に入りは、metadata 側の走査をしない
  (similar の watcher 共有だけは維持)。Ctrl+G は有効な全お気に入り ID で絞るので
  ([global_search.rs:226](../src/global_search.rs)) 検索結果は変わらない。
- **一度きりの付け替え**: 起動時 reconciliation (§4.3、supervisor spawn 前の同期処理) で、
  `favorite_id` が所有者と異なる行の **SQLite 行だけ** を消す (Tantivy は触らない)。所有者の
  walker が「FS にあり DB に無い」として取り込み、Tantivy は delete_term + add で上書きされる。
  外側が内側の行を `to_delete` で消す経路 (Tantivy の path 単位 delete) を作らないこと。
- お気に入りの編集画面の件数は所有者に付く (外側の件数から内側の分が減る)。表示上の変化として
  マニュアルに書くほどではないが、レビューで確認する。
- 名前索引は主キーが `(favorite_root, path)` で奪い合いは無い。入れ子の二重走査は残る (§8)。

## 4. B: 名前索引の単一走査と書き込み省略

- Pass 1 / Pass 2 を 1 回の深さ優先走査にまとめ、各フォルダの `read_dir` 1 回で
  「子フォルダ」と「索引対象の子 (Folder / ZipFile / PdfFile)」を同時に得る。
  進捗の分母は前回スキャンのフォルダ数 (DB から数える) を目安に使い、無ければ件数だけ出す。
- 各フォルダで、DB の直下行 (`path, kind, mtime, display_name`) と今回の子集合が同じなら
  `upsert_children` を呼ばない (トランザクション自体を開かない)。
- 現在の stale 掃除は `updated_at < scan_start_stamp` の mark-and-sweep
  ([search_index_db.rs:241](../src/search_index_db.rs))。書き込みを省くと未更新行が stale 扱いで
  消えるので、掃除を **訪問済みフォルダ集合** に基づく形へ変える:
  完全な走査 (`cancelled=false && had_error=false`) の後、favorite の行のうち
  「親フォルダが今回の訪問集合に無い行」を削除する (`favorite_root` 直下の行は親 = root)。
  直下の子は訪問済みフォルダごとに authoritative に比較済みなので、これで同値になる。
  不完全観測時に prune しない規則は維持。
- `apply_single_change` / `run_subtree_scan` (watcher 経由) の挙動は変えない。subtree 版の
  prune (`prune_stale_under_subtree`) も同じ理由で書き込み省略と両立しないため、subtree 走査でも
  訪問集合方式に揃えるか、subtree 走査では書き込み省略をしない。どちらにするかは実装者が
  コードで確かめて決め、設計書へ記録する。

## 5. C: サイドカー判定をフォルダ一覧から行う

- `external_metadata::sidecar_signature` は候補 4 つを `std::fs::metadata` で順に調べる
  ([external_metadata.rs:41](../src/external_metadata.rs))。サイドカーの無い画像では 4 回とも
  失敗する syscall になる。
- walker は各フォルダの `read_dir` 結果から `.json` / `.txt` の (小文字化した名前 → size, mtime,
  is_file) の表を作り、同じ優先順位・同じ大文字小文字無視で引く版
  (`sidecar_signature_from_listing`) を使う。
- 署名値は既存と **完全に同じ** にする: ハッシュに入れる名前は候補の構成名
  (`<full>.json` 等。ディスク上の綴りではない)、mtime は秒、fingerprint の式も同じ。
  既存版と新版が同じ値を返すことをテストで固定する (大文字の拡張子・stem が無い名前・
  ディレクトリ名が `x.jpg.json` のもの を含む)。
- 画像本体の mtime/size も同じ `read_dir` 由来 (`DirEntry::metadata`) なので、出どころが揃う。
  watcher 差分の単体経路 (`indexer_supervisor.rs:947`) は 1 件なので既存の stat 版のままでよい。
- ingest 時のテキスト読み取り (`read_search_text_counted`) は変えない (取り込む画像だけ)。

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
- **[今すぐ確認]** ボタン: 全お気に入りの 3 系統へ Full を要求する
  (`IndexerManager::request_full_rescan` / `NameIndexCommand::FullRescan` /
  similar の `FullReason::Manual`)。現在は手動の全確認 UI が無く、「アプリを再起動すれば
  確認される」が回避策だった ([favorites_editor.rs:14](../src/ui_dialogs/favorites_editor.rs))。
  設定 ON ではこの回避策が効かなくなるので、ボタンは同時に必須。
- 設定を変えても、走っている supervisor には影響しない (次回起動の判断にだけ使う)。

### 6.2 「一度でも完全に走査できた」印

系統 k (fts / name / similar) と root r ごとに、ディスク上に 1 つの印を持つ:

```
scanned_once(k, r) = { config_fingerprint }    // 行が無い = 印なし
```

- **立てる**: その root の Full が完全に終わったとき (cancel なし・不完全観測なし・prune 済み)。
  設定 ON / OFF に関わらず立てる (OFF で使っていた人が ON にした直後の起動から省けるように)。
- **消さない**。イベント受信・クラッシュ・強制終了・電源断・処理途中の終了では何もしない。
  終了中や処理途中に起きた変更の取りこぼしは、この設定の利用者が受け入れる範囲
  (2026-10-02 利用者決定)。終了時に捨てる watcher の待ち行列 (§4.1 終了応答性) や、
  Tantivy First の SQLite 側更新漏れ (§4.2) が次回起動で直らないことも同じ扱い。
- 起動時、設定 ON かつ印があり指紋が一致する root は、その系統の初回 Full を省き、
  「Full が完了し watcher が Ready」の状態から始める。印が無い・指紋不一致・設定 OFF なら
  従来どおり Full。**全体の走査になるのは、索引がまだ一度も完成していない root と、
  構成・索引の版が変わった root だけ**。
- 指紋には「変わったら既存の索引では足りない」ものだけを含める: root の正規化パス、
  `excluded_roots` (A の入れ子除外を含む。入れ子のお気に入りを外すと、外側が一度も
  走査していない範囲が生まれるため)、系統の索引版 (fts は `INDEX_VERSION`、similar は
  hash_version に相当する値、name はスキーマ版)。サイドカーや対象拡張子の判定が変わる
  版上げは、それぞれの索引版を上げる既存の運用に乗せる。
  指紋を作る関数は系統ごとに 1 つにし、含めた入力の一覧をその関数のコメントに置く。
- 書き込みは既存 DB の通常の書き込み (WAL + NORMAL) でよい。印を立てた直後に失われても
  次回走査するだけで、安全側。

### 6.3 系統ごとの省略

- **fts**: 起動時 reconciliation (§4.3、Failed 行の掃除・版不一致の再構築) は省かない。
  rebuild pending / 版不一致のときは印に関わらず Full。省くのは `run_initial_scan` だけで、
  `initial_scan_done=true` とし、perf `initial_scan_done` には `skipped=true` を付ける。
- **name**: `run_full_scan` の初回だけを省く。watcher 起動は従来どおり先に行う。
- **similar**: 初回 Full の代わりに、Full が担っていた次の処理だけを走らせる:
  `cleanup_incomplete_if` (build 残骸の掃除)、構成に無い root の行の purge
  (`purge_roots_except_if` 相当)、メモリ読み込み、進捗を Complete に進めること。
  inventory 読み込みと列挙はしない。印は similar の root 単位 (similar 側の root 重複除去後) で持つ。
  gap epoch / repaired_gap_epoch は「Full が完了した」場合と同じ値に進める。
- いずれの系統でも、起動後の overflow・watcher 回復・手動確認は従来どおり Full を走らせる。

### 6.4 永続データ

3 つの DB はいずれもリリース済み。変更はテーブルの追加だけ (`CREATE TABLE IF NOT EXISTS`)。
旧版で作られた DB には印が無い = 従来どおり走査、で安全側に倒れる。旧版へ戻した場合も
旧版は新テーブルを読まないだけで影響しない。既存行の意味は変えない (A の付け替えは
データの整理で、スキーマ変更ではない)。

## 7. テスト

- A: 入れ子 2 お気に入りで 2 回連続の初期 scan を回し、2 回目の取り込みが 0 であること。
  付け替え後に外側・内側どちらの Ctrl+G 絞り込みでも同じファイルが 1 件で出ること。
  同一 root 重複、内側が metadata OFF、内側を後から ON/OFF した場合。
- B: 変化の無い 2 回目 scan で `upsert_children` が 0 回。フォルダ削除・空になったフォルダ・
  深い subtree 削除で stale 行が消えること (既存 `tests/search_name_e2e.rs` を維持)。
  不完全観測で prune しないこと。
- C: `sidecar_signature` と listing 版の値一致 (§5 の境界を含む)。walker が画像ごとに
  `std::fs::metadata` を呼ばないこと (呼び出し計数または関数分離で確認)。
- E: 印あり + 指紋一致で初回 Full が走らないこと / 指紋不一致・印なし・設定 OFF で走ること /
  Full の完了 (不完全観測なし) でだけ印が立ち、cancel・不完全観測では立たないこと /
  省いた起動の後も watcher 差分・overflow の Full が従来どおり動くこと。similar は purge・build 掃除・進捗 Complete が
  省略時も起きること。手動確認で 3 系統に Full が要求されること。
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

段 1 の後、利用者に OS 再起動直後の `--perf-log` (または通常ログの `initial scan done` 行) を
取ってもらい、系統ごとの所要時間と走査の重なりを見てから、D1 / D2 / 不要を決める。
現時点では、C で comfyui の走査 (サイドカーなし画像 × 4 回の stat) がどれだけ縮むかが
未計測で、D の効果を見積もれない。

## 9. 文書

- [search-architecture.md](search-architecture.md) §4.1 / §4.3 / §4.4 に A・B・E を反映。
- spec.md に設定項目、マニュアル (お気に入り・検索の索引のページ) に設定とボタン。
  通信・保存先は変わらないので privacy.html / 製品ページ安心セクションは対象外。
- CHANGELOG.md は公開準備で記入。
