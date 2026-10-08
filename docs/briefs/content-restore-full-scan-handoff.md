# 引き継ぎ: 「コピー・移動後も編集を引き継ぐ」の復元が件数×表サイズで遅い (2026-09-27)

> 起票: ClaudeCode Opus 5.5 (サイト作業セッション)。**次のリリースで直す** (利用者判断 2026-09-27)。
> 対象機能は v3.2.0 (2026-08-23) で出荷済み。**リリース済みの利用者に影響する**。
> 実装は Codex Sol (CLAUDE.md の担当分担)。本書は調査結果と修正の論点で、修正方針の最終決定は引き継ぎ先に任せる。

## 1. 観測 (誰が何を見たか)

- **利用者が観測**: 10 万ファイルのフォルダ `D:\home\image10000` から BS で親 `D:\home` へ移動したところ、
  「保存中の設定を確定中 / 保存中の変更が完了するまで待っています。」の表示で長時間止まった
  (スクリーンショットあり。表示は `src/app/sidecar_restore.rs:409-412` の `PhaseName::Quiescing`)。
- フォルダの中身: PNG 102,625 枚 + ZIP / MP4 / MP3 各 1。利用者が `G:\home\comfyui\...` の生成画像を
  コピーして作った計測用フォルダで、置き場所は SATA HDD (WD80EAZZ)。

### ログ (`%APPDATA%\mimageviewer\logs\mimageviewer.log` と `perf_events.jsonl`、プロセス pid 98132、起動 12:46)

| 時刻 (起動後) | 出来事 | 出典 |
|---|---|---|
| 7.06 s | `image10000` を開き、content identity の検出開始 (`targets=14177`) | perf `content_identity/detect_begin` |
| 150.08 s | 検出終了 `ms=142655`、`candidates=12403` (すべて `relation=copy`、元は `g:/home/comfyui/...`) | perf `detect_end`、log `content_identity: candidate ...` |
| 150.6 s → 339.4 s | 引き継ぎ確認ダイアログ (`content_restore`) を表示、利用者が引き継ぎを選択 | log `[input] ... content_restore` |
| 339.39 s | **復元開始 `restores=12403, declines=0`** | perf `content_identity/restore_enqueue` |
| 693.36 s | BS で `d:\home` へ。`sidecar restore started request=17507` → Quiescing で待機に入る | log |
| 〜857 s (ログ末尾) | `content_identity: restore completed ...` は**まだ出ていない**。UI は 1 秒ごとの heartbeat が続き、固まってはいない | log |

### プロセス統計 (ClaudeCode が 2026-09-27 13:00 頃に OS のカウンタで取得、アプリの UI は操作していない)

- 10 秒間で CPU 28.2 秒 (約 3 コア)、**読み込み 6,510 MB**、書き込み 17.5 MB、スレッド 178。
- 読み込みの大半はディスクではなく OS のファイルキャッシュからと推定 (未確認)。

**未確認**: 復元が最終的に何分で終わったか、途中で失敗したか。引き継ぎ時点で利用者に確認すること。

## 2. 原因 (コードの参照)

### 2.1 待たされる理由 (設計どおりの部分)

`sidecar_restore` の Quiescing は、競合する書き込みが無くなるまで次へ進まない。
`sidecar_restore_has_conflicting_work` (`src/app/sidecar_restore.rs:1044-1056`) が
`self.content_identity_restore_pending.is_some()` を競合として数えるので、
**content identity の復元が終わるまで、別フォルダへの移動がモーダルで止まる**。
復元側は完了時にしかログを出さず (`src/app/content_identity_restore.rs:333`)、進捗も中断手段も無い。

### 2.2 復元が遅い理由 (本件の不具合)

復元の入口は `restore_candidates_at` (`src/content_identity/restore.rs:147`)。DB を開く回数は
候補数に依存しない設計で、テストもある (`batch_restore_database_opens_are_constant_for_one_and_hundred_candidates`)。
しかし **開いた後のクエリが候補ごとに表を全件読みする**。

1. **`copy_prefix`** (`src/rename_key_migration.rs:1932`)
   - `restore_copy_mappings` (`src/content_identity/restore.rs:288`) は、候補 1 件ごとに `Exact` と
     `VirtualPrefix` の 2 つの mapping を必ず作る。`VirtualPrefix` は ZIP entry / PDF page 用 (`<path>::...`) だが、
     **普通の画像ファイルでも作られる** (今回の 12,403 件はすべて PNG なので、この検索は 1 件も当たらない)。
   - `copy_stores_at` (`:1143`) は unique な STORES の各表について、全 mapping を `copy_store_mapping` に通す。
   - `copy_prefix` は `SELECT DISTINCT {col} FROM {table} WHERE substr({col}, 1, ?1) = ?2` を実行する。
     **列に関数をかけているので索引が使えず、毎回全件スキャン**になる。
   - 結果として **候補数 × unique STORES の表の数 × 各表の行数** の読み込みになる。
2. **`query_family_rows`** (`src/content_identity/restore.rs:414-445`)
   - `load_restore_runtime_updates` (`:487-644`) から 7 回呼ばれ、family (≒候補) ごとに
     `WHERE {key} = ?1 OR substr({key}, 1, ?2) = ?3` を実行する。`OR substr(...)` があるので、
     これも索引が効かず **family ごとに全件スキャン**になる。`copy_stores_at` が終わった後に、同じ規模のもう 1 周が控えている。

### 2.3 規模の見積もり (上限)

この利用者環境での、全件スキャン対象になる DB ファイルの大きさ (2026-09-27 時点):
`tags.db` 216.5 MB (STORES に 3 表)、`comic.db` 89.5 MB、`rating.db` 13.6 MB、`content_identity.db` 11.6 MB、
`adjustment.db` 9.6 MB (2 表)、`local_adjust.db` 3.8 MB、ほか 1 MB 前後が十数個。

1 候補あたり最大で約 350 MB のページ読みになり、12,403 候補だと約 4 TB。観測した 650 MB/s なら
約 1.9 時間。**ファイル全体が対象表ではないので実際は短い可能性があり、あくまで上限**。
行数に比例するので、タグや漫画の編集が多い利用者ほど遅くなる。

### 2.4 同じ形の箇所 (列挙のみ。どこまで直すかは引き継ぎ先が判断)

`substr(列, 1, n) = ?` / `OR substr(...)` を使っているクエリ (`grep -rn "substr(" src`。テストコード (`rename_key_migration.rs` の 2067 行目以降、`content_identity/restore.rs` の 675 行目以降) は除いた):

| 場所 | 用途 | 備考 |
|---|---|---|
| `src/rename_key_migration.rs:1945` `copy_prefix` | 復元・コピーの複写 | **本件** |
| `src/content_identity/restore.rs:428` `query_family_rows` | 復元後の再読み込み | **本件** |
| `src/rename_key_migration.rs:1707` `move_prefix` | 名前変更・移動の付け替え (`:1614`, `:1622` から) | 大量の名前変更・移動で同じ遅さになるはず (未計測) |
| `src/rename_key_migration.rs:1636-1641, 1976-1989` | path 列の付け替え・検索 | 呼ばれる件数を要確認 |
| `src/catalog.rs:585`, `src/search_index_db.rs:106,289,320`, `src/video/tile_thumb_cache.rs:586-594` | 別機能の前方一致 | 本件とは別。呼ばれ方が 1 回きりなら問題にならない |

**同じファイルに既に直した前例がある**: `purge_store` (`src/rename_key_migration.rs:1536-1556`) は
「全 STORES のキー列は既定 BINARY collation で、PK または path index を持つ」として、
`col >= ?1 AND col < ?2` の範囲検索 (`prefix_upper_bound`、`:1488`、テスト `:2383`) に置き換え、
上限が作れないときだけ `substr` に戻している。同じ手を `copy_prefix` / `move_prefix` / `query_family_rows` に使える見込み。

## 3. 修正の論点

1. **範囲検索化**: `purge_store` と同じく `col >= prefix AND col < prefix_upper_bound(prefix)` にする。
   `query_family_rows` の `key = ?1 OR substr(...)` は、`key = ?1` と範囲検索の 2 文に分けるか、`UNION ALL` にして索引を効かせる。
   前提 (キー列が BINARY collation で索引を持つ) を、対象の各表について確認すること。
2. **不要な prefix mapping を作らない**: 元がコンテナ (ZIP / PDF / 変換対象アーカイブ) でなければ
   `VirtualPrefix` は常に空振りする。`restore_copy_mappings` で `source.kind` を見て省けるかを検討する
   (1 だけでも計算量は直る。2 は定数倍の削減)。
3. **テスト**: 「DB を開く回数が一定」の既存テストに加えて、**全件スキャンをしないこと**を機械的に確かめる。
   候補: `rusqlite` の `Statement::get_status(StatementStatus::FullscanStep)` が 0 であること、
   または `EXPLAIN QUERY PLAN` が `SEARCH ... USING INDEX` になることを表ごとに検査する。
   行数を増やしても復元時間がほぼ変わらないことを示すベンチがあれば、なお良い。
4. **待ち画面 (別件として扱ってよい)**: 復元中に別フォルダへ移ると、何を待っているか分からないまま止まる。
   少なくとも「編集内容の引き継ぎを待っています (n / N 件)」のように待っている相手と進捗を出す。
   復元 worker は完了時しかログを書かないので、開始・進捗・完了の perf event も足す。
   Quiescing で待つ設計自体の是非は、sidecar と content identity の書き込み所有の話なので、変えるなら設計レビューを通す。
5. **検出にかかる時間 (参考)**: 14,177 件の検出が HDD 上で 142 秒。中身のハッシュを取るので I/O 律速と思われ、
   本件とは別。気になるなら別途計測する。

## 4. 利用者への案内 (いま止まっている場合)

- 待てば終わる見込み (上限 2 時間程度)。終われば Quiescing は自動で先へ進む。
- 途中で終了させた場合: `copy_store_transaction` は表ごとに 1 トランザクションで、複写は `INSERT OR IGNORE`。
  台帳の「引き継ぎ済み」記録 (`apply_batch_ledger_updates`) は複写の後にまとめて付く。
  そのため、**中身が壊れる形にはならず、次に同じフォルダを開くと確認がもう一度出る**見込み (コードからの判断、未確認)。

## 5. 再現手段

- 利用者環境: `D:\home\image10000` (利用者の生成画像のコピー 10 万件) を開き、引き継ぎを選ぶ。
  **利用者の実データで起きる現象なので、エージェントは実データのアプリを起動しない** (CLAUDE.md)。
- 単体で再現するなら: 一時 data dir に `tags.db` / `comic.db` などの表を数十万行で作り、
  `restore_candidates_at` に数千候補を渡して時間と FullscanStep を測る
  (`src/content_identity/restore.rs:795` の `measure_batch_database_opens` が土台に使える)。
