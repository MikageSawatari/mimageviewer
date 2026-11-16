# §1.800 本を Enter で開き Esc で一覧へ戻るときのもたつき — 調査と設計

- 状態: 調査・設計 (2026-11-17)。実装前。設計の独立レビュー r1〜r3 = REVISE を反映済み (§8)。§3.5 の比較で案 S1 (一括で読まない) へ切り替えた。S1 の設計は §4S。r5 = REVISE (S1 の方向は支持、P2 4 件)、統括の判断待ち。
- ブランチ: `perf-800-book-close` (worktree `G:\home\mimageviewer-perf-800`)。
- 目標版: v5.3.0。

## 1. 報告と観測の出どころ

- 利用者の報告: 「Enter で本を開いて閉じるときの動作が、すこしワンテンポ待たされている感じがある」。
- 観測は**利用者のインストール版 v5.2.1 の perf ログ**だけ (写し: `G:\home\mimageviewer-perf-evidence\perf-20261117-book-close.jsonl`、
  起動後 20〜26 秒)。エージェントは製品を起動していない。
- 行数の分析は、利用者の `%APPDATA%\mimageviewer\cache` にある該当フォルダの catalog DB を**読み取り専用で写した**ものに対して行った
  (写しは scratchpad。製品の DB には書いていない)。

対象フォルダ: `G:\home\doujindl\downloads` (PDF 1,493 冊。`lf_scan` は folders=1493)。本は 26 ページの PDF。

## 2. perf ログの読み (区間と UI 停止の切り分け)

`analyze_perf.py nav` の `sli_end` は**開始から終了までの経過時間**で、途中に非同期の待ちを含む。UI が止まった時間は
フレーム間隔と `update_breakdown` で見る必要がある。閉じる 1 回 (seq 28、input `FsClose` = 23.087 s) の時系列:

| 時刻 (s) | 出来事 | UI 停止か |
| --- | --- | --- |
| 23.087 | `FsClose` | |
| 23.100〜23.118 | `load_folder` 30 ms (scan 5 / sort 2 / dup 1.6) + `sli` 前半 (items 代入 3.4、rating 4.3、tags 2.8) | 同期 (約 35 ms) |
| 23.158 | フレーム (間隔 74 ms) | |
| 23.218 | フレーム。`keep_fullscreen_viewport` 56 ms | 同期 (別件、§7) |
| 23.241 | サイドカー復元の終端を受け取り `resume_loading_items_after_sidecar` へ。`sli_sidecar_import`=122 ms と出るが、worker 自体は `probe_ms`=1.2 ms | **非同期の待ち** (フレーム数ぶんの経過) |
| 23.242→23.464 | **`sli_catalog_load_all` 221 ms (entries=3124)** | **同期。ここが停止の本体** |
| 23.474 | `sli_settings_save` 8.8 ms → `sli_end` 369 ms | 同期 |
| 23.496 | フレーム (間隔 **258 ms**) | |

ほかの閉じる回も同じ形 (seq 27: load_all 163 ms・フレーム間隔 186 ms、seq 29: 134 ms・167 ms)。
開く側 (26 ページ) は `sli_catalog_load_all` 0.8 ms・`sli_sidecar_import` 25〜42 ms (2〜3 フレームの待ち) で、
**開く側のもたつきは catalog ではない** (§7 に別件として記録)。

## 3. 原因

### 3.1 `sli_catalog_load_all` が UI スレッドで全行の画像本体を読む (主因)

- 経路: `App::resume_loading_items_after_sidecar` (src/app.rs、`── カタログを開く + cache_map ロード + 削除掃除 ──` の節)。
  合成ビュー (`prepared_aggregate`) は prepare worker で catalog を開き `load_all` 済みの `PreparedAggregateCatalog` を渡すが、
  **通常フォルダ・本 (仮想フォルダ) は UI スレッドで `catalog_arc.load_all()` を呼ぶ**。
- `CatalogDb::load_all` (src/catalog.rs) は `thumb_data` (WebP 本体) を含む全行を SELECT して `HashMap<String, CacheEntry>` を作る。
  このフォルダでは 3,124 行・**87 MB**。CLAUDE.md「UI スレッドで同期 I/O をしない」と docs/ui-responsiveness.md §4 に反する。
- 同じ節の `delete_missing` も UI スレッドで全行名の SELECT と行ごとの DELETE を行う (この利用者は §3.3 の理由で走っていない)。
- 写しの DB での計測 (Python sqlite3、同じ SQLite エンジン): 全列 172〜213 ms、`thumb_data` を除いた列 132〜139 ms、`filename` だけ 2 ms。
  **`thumb_data` を除いても速くならない**のは、表の列順が `thumb_data` の後ろに寸法・証明などの列を置いており、
  4 KiB を超える BLOB は overflow page に載るため、後ろの列を読むのに overflow の連鎖をたどるから。
  したがって「見出しだけ読む」案 (`load_source_dims` / `thumbnail_headers` と同じ形) は効かない。行を減らすか、UI から外すしかない。

### 3.2 `sli_sidecar_import` は UI の停止ではない

- `sli_sidecar_import` は `restore_started_at` (前半の終わり) から `resume_loading_items_after_sidecar` の入口までの**経過時間**
  (src/app.rs の perf::event)。サイドカー復元は `src/app/sidecar_restore.rs` の状態機械 (PreparingPageIndex → Quiescing → Checking →
  Running → CacheRefreshing → Resuming) で、各段の受け取りはフレームごとの poll。worker 本体は 1〜2 ms (`sli_sidecar_reuse_check`
  の `probe_ms`)。
- 閉じた直後のフレームは重い (§2 の 74 ms・80 ms) ため、数フレームぶんの待ちが 70〜122 ms に伸びる。UI は止まっていないが、
  一覧のサムネイルが出始めるまでの時間には効いている。本件では**この待ちと catalog の読み込みを重ねる** (§4 段階 A) ことで
  吸収し、サイドカー側の段数は変えない。

### 3.3 1,493 冊で 3,124 行になる理由 — 古い鍵の行が消えない

写しの DB の内訳 (基底名 1,494 件、うち 1 件は既に消えたファイル):

| 鍵の形 | 行数 | 本体 | 今の設定で読まれるか |
| --- | --- | --- | --- |
| `pdfthumb:X.pdf\|types:7d3f…\|autocrop:v3:rtl` | 1,139 | 47.9 MB | 読まれる (今の自動切り抜き) |
| `pdfthumb:X.pdf\|types:7d3f…` | 327 | 5.9 MB | 読まれる (切り抜き無しの名前空間。`representative_existing_keys` が残す) |
| `pdfthumb:X.pdf\|types:7d3f…\|autocrop:v1:ltr` | 1,386 | 25.8 MB | 読まれない (旧版 v1・逆の読み方向) |
| `pdfthumb:X.pdf` (`types` 無し) | 272 | 7.5 MB | 読まれない (種類の表示を絞る前の名前空間) |

同じ鍵の重複ではなく、**鍵の違う行**が積み上がっている。消えない理由:

- `resume_loading_items_after_sidecar` は `delete_missing` を
  `!is_synthetic_view_path(&source_path) && self.settings.file_type_visibility.is_all_visible()` のときだけ呼ぶ。
  利用者は種類の表示を絞っている (鍵に `|types:` が付く) ため、**片付けが一度も走らない**。
- この条件は v4.6.0 の 291b32e5b (2026-10-09「Apply app-wide file type visibility before list construction」) で入った。
  一覧が表示対象だけの射影になったので、隠した種類の行を消さないための意図的な判断 (コメント「Projection is not evidence of physical
  deletion. Preserve hidden-page and older representative cache rows」)。
- v5.2.0 の表紙の自動切り抜き (§1.764、bd3ba8b30 / 995f75f63) で PDF 1 冊に行がもう 1 つ増え、版 (v1→v3) や読み方向 (ltr/rtl) が
  変わるたびに 1 組ずつ増える。表示を絞っていない利用者は `delete_missing` が古い組を消すので増えない。
- **v5.3.0 で増える**: master の自動切り抜きは v4 (§1.773)。この利用者が v5.3.0 で同じフォルダを開くと、v4 の組 (~1,500 行・数十 MB) が
  さらに加わり、閉じるときの停止が今より長くなる見込み (推定。v4 の行の大きさは未計測)。

判定: 片付けが走らない条件自体は v4.6.0 から。**行数を倍にしたのは v5.2.0 の自動切り抜き**で、v5.3.0 の v4 でさらに増える。

## 3.5 単純な案との比較 (2026-11-17、r3 の後に統括の指示で実施)

設計レビューを重ねるたびに §4 の段階 A に仕組み (continuation の中の状態、復元の所有者、読み取り専用の別接続、区切った transaction) が
増えたので、状態の組み合わせを減らせる案と数字で比べた。

**計測** (利用者の catalog の写し、Python sqlite3 = 同じ SQLite、OS キャッシュが温まった状態の 5 回の中央値。製品の Rust では
HashMap の組み立ても加わり、ログでは 134〜221 ms):

| 読み方 | 時間 |
| --- | --- |
| 今の `load_all` (全行・全列、3,124 行) | 118 ms |
| 画像データの後ろの列 (寸法・証明・表紙など) まで、画像データ抜き | 61 ms |
| 画像データより前の列だけ (`filename, mtime, file_size, width, height`) | 9 ms |
| `filename` だけ | 1 ms |
| 鍵 1 件ずつ全列 (`load_one` 相当) | 0.03 ms / 件 (今の鍵 1,466 件で計 51 ms) |
| 古い行を消した写し (1,466 行) の `load_all` | 66 ms (VACUUM 後 57 ms) |

**案 S2: 古い行の片付けだけ** (統括の案 2)。行数は 3,124 → 1,466 で、読み込みは 118 → 57〜66 ms (約半分)。製品のログに当てはめると
70〜120 ms の UI 停止が残る。全種類表示の利用者は既に片付いているので効果が無く、同じ 1,500 冊のフォルダで同じ停止が出る。
v5.3.0 の自動切り抜き v4 で鍵が変わると、片付けるまでの間はまた増える。**単独では足りない** (§4 段階 C の後段として残す)。

**案 S1: 一覧を開いたときに一括で読まない** (統括の案 1 を一段進めた形)。
- 「UI では小さな列だけ読む」は、画像データの後ろにある列 (寸法・表紙・自動切り抜きの素材・証明) が overflow をたどるため 61 ms
  かかり、数 ms には収まらない。小さく読めるのは画像データより前の列 (鍵・mtime・大きさ・サムネイルの幅と高さ) だけ (9 ms)。
- そこで、`cache_map` を「DB の全行の写し」から「この一覧で読んだ・書いた行の控え」に改め、一覧を開くときは空で始める。
  サムネイル worker は控えに無ければ `load_one` で DB の行を鍵で引く (1 件 0.03 ms、worker の中なので UI は止まらない)。
  鍵で引く仕組みは、親フォルダ・コレクションの要求 (`RequestCatalog::Parent` などの `lazy_scope`) に既にあり、通常の
  `RequestCatalog::Fixed` は控えの未ヒットで DB を引かない (`thumb_loader.rs` の `uses_lazy_catalog`)。ここを Fixed でも引くようにする。
- これで UI の `load_all` (134〜221 ms) が無くなり、§4 段階 A の非同期の合流・取り消し・表紙の失効の窓・別接続・区切りが**全部
  要らない**。worker が DB を引くのは要求の時点なので、表紙の失効で消えた行は引かない (窓が無い)。
- 代わりに直す所: `cache_map` に行が無いことを「DB に行が無い」とみなしている読み手。コードで見つけたもの:
  - 動画ピンの seed (`seed_folder_video_pin_thumbs`): `purge_stale` が控えに無いと古いピンの行を消さない。`already_seeded` の判定。
    控えが空だと古い行が消えず、worker が DB から引いて古いピンのサムネイルが戻る (Codex Phase C P3 で塞いだ不具合の再発)。
  - 仮想フォルダの seed (`setup_virtual_folder_seed_and_writeback`): `already_in_virtual`。空だと毎回 seed し直す (正しいが余計な書き込み)。
  - ドライブ一覧の seed (`seed_drive_list_pin_thumbs_from_catalog`): `already_cached`。
  - 自動縦横比の seed (`reset_and_seed_auto_aspect_with_seed`): 控えの `source_dims` から数件の見本を取る。空だと catalog からの
    見本が無く、フォルダごとの縦横比の記録 (`auto_aspect_cache`) とサムネイルの到着に頼る。
  - 色フィルタ: 通常の画像は控え → デコードの経路 (共有の helper)。Fixed で DB を引くようにすれば同じく効く。
  r5 の指摘で、これらを UI で DB を引く形に直すのはやめ、照合が要る仕事をその鍵を持つ worker の要求・一覧の job へ移した (§4S)。
- 影響 (見込み): サムネイルが出始めるまでが「サイドカーの待ち + 0」になり、段階 A (並列) より速い。メモリも一覧の分だけ (控え)。
  起動・フォルダ移動など、本以外の一覧の読み込みもすべて同じく軽くなる。

**判断**: S1 を採る方向で設計し直す。§4 の段階 A (並列の先読み) より状態が少なく、効果も大きい。ただし `cache_map` の意味を
変えるので、上の「行が無い = DB に無い」とみなす読み手を漏れなく列挙するのが前提で、独立レビューでその列挙を確かめる。
S1 が成り立たないと分かった場合だけ §4 の段階 A に戻る。

## 4S. 案 S1 の設計 (採用。r9 の指摘を受けて r10 で改訂、2026-11-17)

r4 (r3 までの案へのレビュー、§8) は「片付けを区切っても UI の writeback の待ちは上限にならず、writeback 自体を worker へ移す必要がある」と
して REVISE だった。r3 までの案は直すたびに仕組みが増えるので、§3.5 の案 S1 に切り替えた。r5 は S1 の方向を支持し、P2 4 件で REVISE。
利用者決定 (2026-11-17、統括経由): **S1 を v5.3.0 に入れる。**

方針: **一覧を開くときに catalog の全行を読まない。サムネイル worker は控えに無い鍵を DB から引く。** §1.773b の表紙の失効とは、
DB から引いた行を控えへ登録しないことで、新しい競合を作らない (4S.4)。

**seed の置き場所の経緯 (r6〜r9) と r10 の判断**: r5 の「UI の DB I/O を増やさない」に従い、動画ピン・仮想フォルダの seed (今は
一覧の開始時に UI で同期に行う) を worker へ移そうとした。r6 (一覧の job) → r7 (pool の最初の 1 手と門) → r8 (項目の要求に写しを載せる)
→ r9 (worker が実行時に正本を読む) と 4 回改訂し、そのたびに、移したことで生じる順序 (ピン更新・pool の作り直しとの前後、
古い写しの書き込み)、正本の DB の WAL と reader の排他、照合要求の寿命の指摘が出た。いずれも「今は UI が一覧の開始時に、pool の
起動前に、同期で行っている」ことで最初から生じていない問題で、CLAUDE.md の「状態の組み合わせを減らす」に照らし、**r10 では seed を
今の UI の位置・順序のまま残す**。S1 で変えるのは、seed の中の「控えにあるか」の判定 3 か所を、その鍵 1 件の DB の読みに置き換える
ことだけにする (4S.1-5)。UI の I/O の増減は 4S.1-5 の表のとおりで、一覧を開くときの UI の I/O は全ての場合に今より減る。seed の
ループそのもの (今も UI で `video_pins.db` の WebP を読み、catalog に書く) を worker へ移すのは、S1 の前からある UI の I/O の扱いで、
本件とは分けて §7 に記録する。

自動縦横比の catalog からの見本は、collection と同じく控えからだけ取る (4S.1-7、通常の使い方への影響があるので統括経由で利用者に
確認する)。

### 4S.1 変えること

1. **一覧の読み込みで `load_all` を呼ばない。** `resume_loading_items_after_sidecar` の catalog 節で、通常の一覧 (合成ビュー以外。
   通常フォルダ、PDF/EPUB の仮想フォルダ、ZIP・変換書庫の物理一覧) の `cache_map` を空で作る。catalog の open
   (`get_or_open_catalog`)・`delete_missing` は今のまま UI で行う (行名だけを読む 1〜2 ms、今と同じ)。合成ビューは今の
   `prepared_aggregate` のまま (worker が作った全行の写し)。**ドライブ一覧** (独自の `load_all` と
   `seed_drive_list_pin_thumbs_from_catalog`) は catalog が数行なので変えない (範囲外)。
2. **`cache_map` の意味を「控え」にする。** 「この一覧で worker が保存・seed した行」の控えで、無いことは
   「DB に無い」を意味しない。合成ビューの写しも控えとして扱う。型や mode の印は足さない。
3. **サムネイル worker: `RequestCatalog::Fixed` でも、控えに無ければ DB を鍵で引く。** `uses_lazy_catalog` の意味は変えず、
   「未ヒットで DB を引く」条件だけを `lazy || (Fixed かつ catalog が retired でない)` に広げる (例: `looks_up_catalog_on_miss`)。
   Fixed の PDF メタ補完が使う `initial_catalog`、Parent の観測と stamp 変換、retired な CacheOnly の読み取り専用の継続、Legacy の
   置換、SourceOnly の bypass、EPUB の世代解決後の鍵、ピン・cascade・自動切り抜きの鍵は今の分岐のまま。
   - **DB から引いた行は控えに登録しない** (今の collection の lazy 読みと同じ。Parent だけが今どおり stamp を変えて登録する)。
   - 引いた行の WebP が壊れていれば、lazy と同じくログと `record_cache_error` を出し、通常の生成へ進む。
4. **ピンだけの要求の前方一致の探索** (`send_pinned_only_cached` の preview と本命の 2 か所): 控えの走査をやめ、今の `resumed`
   経路と同じ `load_latest_uncropped_with_prefix_matching` で DB を引く。
5. **動画ピン・仮想フォルダの seed は、今の UI の位置・順序のまま。「控えにあるか」の判定だけを DB の 1 行読みにする** (r10)。
   `resume_loading_items_after_sidecar` で、今と同じく catalog の open の後、pool の起動の前に、仮想フォルダの seed
   (`setup_virtual_folder_seed_and_writeback` の seed の部分) → 動画ピンの seed (`seed_folder_video_pin_thumbs`) の順に UI で行う。
   控えは空なので、控えを見ていた次の 3 か所を、その鍵の行を保存先の catalog から読む形 (`load_one`、または中身が要らなければ
   存在だけを読む) にする。書いた行・消した行は今どおり控えにも反映する。
   - `already_in_virtual` (仮想 catalog に先頭ページの行があるか): 鍵 1 件の存在の読み。
   - 動画ピンの `already_seeded` (stamp・WebP・cover が同じ seed があるか): pinned_key 1 件の読み。
   - 動画ピンの `purge_stale` (古い seed の行があるなら消す): 控えではなく DB に行があるときに消す。
   | 一覧を開くときの UI の catalog の I/O | 今 | S1 (r10) |
   | --- | --- | --- |
   | 全行の読み込み (`load_all`) | 全行・全列 (1,493 冊の一覧で 3,124 行・87 MB、134〜221 ms) | 無し |
   | 仮想フォルダの seed | 先頭ページが控えに無ければ親の行 1 件の読み + 仮想 catalog への書き込み | 先頭ページの存在 1 件の読みを足す。読み・書きは今と同じ条件 |
   | 動画ピンの seed | 動画ピン 1 件ごとに `video_pins.db` の WebP の読み、seed が控えに無いか違えば catalog への書き込み | 動画ピン 1 件ごとに catalog の鍵 1 件の読みを足す (温まった状態で 0.03 ms。今の WebP の読みより小さい) |
   | `delete_missing` | 行名の読み (1〜2 ms) + 消える行の削除 | 今と同じ |
   - 足す読みは、今の seed が同じ項目について既に行っている UI の I/O (WebP の読み・書き込み) より小さく、動画ピンが無い一覧
     (ほとんどの一覧) では仮想フォルダの 1 件だけ。順序 (seed → pool の起動 → 要求)・内容・対象 (画面外の項目も含む) は今と同じなので、
     r6〜r9 の順序・寿命・WAL の問題は生じない。pool がまだ動いていないので、新しい pool の worker と接続を取り合うことも無い。
   - `pin_materialization` (4S.2) は今どおりピン更新時に worker で seed・削除を行う。
6. **仮想フォルダの writeback を、先頭ページの要求に載せる** (`fire_virtual_folder_writeback_if_ready` の UI の保存をやめる)。
   - UI は今と同じ条件 (PDF・ZIP、EPUB は対象外) で、対象 (仮想フォルダの source、先頭ページの idx と鍵、items の世代、親 catalog、
     親の鍵、**本を開いた時点の親ファイルの mtime・大きさ**) を「親との対応」として `VirtualFolderWriteback` の代わりに持つ。
     親ファイルの stat は今どおり UI (本を開くときの既存の 1 回)。
   - 要求に載せるのは、この一覧の pool へ出す要求を組み立てる共通の付加処理 1 か所で、source・鍵・世代・idx がすべて一致するとき
     だけ。可視・prefetch・navigation・still seek・overview・hover・idle upgrade はこの付加処理を通る。色フィルタ・独立プレビュー・
     Remote には載せない (idx が一致するだけでは載せない。仮想フォルダの seed は 4S.1-5 で pool の起動前に済んでいるので、色フィルタも
     今どおり seed した行を使える)。
   - worker: 先頭ページを元から生成した要求が終わる直前 (Finalized を送る前) に、親ファイルを stat し、**載せてきた開いた時点の stamp と
     一致するときだけ**、仮想 catalog にある要求の stamp の先頭ページの行 (この要求が保存したもの、保存を省いたなら既にある行) を、
     開いた時点の stamp で親 catalog の親の鍵へ写す。変わっていれば写さない。仮想 catalog に要求の stamp の行が無ければ写さない。
   - **今との意図した差分** (r7 P3-5): (a) 今は items の世代ごとに 1 回だけ写すが、r8 は先頭ページを元から生成するたびに写す
     (idle upgrade や再要求の後も写し、親の行が新しい画質の WebP で上書きされ得る)。(b) cache hit では今も写さない (Finalized が
     出ない) ので同じ。(c) 保存を省いて既存行がある場合は今と同じく写す。
   - 「親との対応」は、一覧の置き換え・退役では捨て、一時的な context の swap では今の `virtual_folder_writeback` と同じく所有 context
     へ移して保持する (detached の述語や状態遷移は変えない)。一回限りの発火の状態は要らなくなる。
7. **自動縦横比: catalog からの見本は控えからだけ取る** (collection と同じ。統括経由で利用者に確認する)。
   - `reset_and_seed_auto_aspect_with_seed` を「世代のリセットと記録 (`auto_aspect_cache`) の復元」と「控えから見本を探す処理」に
     分ける。通常の一覧の S1 経路は前者だけを呼ぶ (控えは空なので、後者は全項目の要求を組み立てても見本が見つからず、ピンの cascade・
     stat・動画 WebP の読みを UI で全件に行ってしまう。r7 P2-1)。合成ビューと collection は今どおり両方を呼ぶ (準備済みの控えに行が
     あれば今と同じく見本を取る)。空の控えを判定の印にはせず、呼び出し側の目的で分ける。
   - 見本はサムネイルの到着 (DB から引いた行を含む) で今の集計 (`poll_thumbnails`) に入り、今の判定のゲートを通って決まる。新しい
     job・payload は足さない。見える違いは 4S.6。

### 4S.2 metadata のピン更新 (`pin_materialization`、r5 P2-3)

`pin_materialization` の worker は共有の控えを写して、その中の鍵から DB の削除対象を決めている。控えが疎になると、DB にだけある
古いピンの行を消し損なう。既存の worker の中で次のように直す (新しい pending は足さない)。r6 で「単体では十分」と確認済み。

- 古いピンの variant の削除候補は、DB の鍵から求める (`prefix` で始まる行名を catalog から読む。行名だけなので軽い)。
  残す条件は今の `inventory_retains_key` のまま。
- 動画ピンの seed の一致判定は、控え → DB (`load_one`) の順で行を引いて比べる。WebP が無い・壊れているときの削除は、控えの有無ではなく
  DB の存在で決める。
- 置き換えの控え (`replacement`) は今どおり、写した控えから削除を除き seed を足したもの (疎のままでよい)。

### 4S.3 「控えに無い = DB に無い」とみなしていた読み手の一覧 (r5 の照合結果を含む)

| 読み手 | 場所 | S1 での扱い |
| --- | --- | --- |
| 通常 worker のキャッシュ判定 | `thumb_loader.rs` | 4S.1-3 (Fixed も未ヒットで DB を引く、登録しない) |
| ピンだけの要求の前方一致 | `send_pinned_only_cached` | 4S.1-4 |
| 動画ピンの seed・古い行の削除 | `seed_folder_video_pin_thumbs` | 4S.1-5 (今の UI の位置のまま、控えの判定を DB の 1 行読みに) |
| 仮想フォルダの seed | `setup_virtual_folder_seed_and_writeback` | 4S.1-5 (今の UI の位置のまま、`already_in_virtual` を DB の存在の読みに) |
| 仮想フォルダの writeback | `fire_virtual_folder_writeback_if_ready` | 4S.1-6 (先頭ページの要求に載せる) |
| 自動縦横比 | `reset_and_seed_auto_aspect_with_seed` | 4S.1-7 (通常の一覧はリセットと記録の復元だけ。合成ビュー・collection は控えから) |
| metadata のピン更新 | `pin_materialization.rs` | 4S.2 |
| 色フィルタ | `color_filter.rs` | 通常の画像は 4S.1-3 の共通の helper で効く。代表画像用の私有の一行の控えと鮮度判定は今のまま。writeback は載せない (seed は pool の起動前に済む) |
| ドライブ一覧の seed | `seed_drive_list_pin_thumbs_from_catalog` | ドライブ一覧は変えない (4S.1-1) |
| 表紙回転の失効 | `cover_rotation_cache.rs` | 失効 worker は DB の見出しと控えを合わせて対象を決めるので、疎な控えで足りる。競合は 4S.4 |
| Everything の seed | `everything/metadata.rs` | 削除する鍵を明示して DB と控えへ当てるので影響なし |
| collection・合成ビュー・退避と復帰 | `collection_grid.rs`、`smart_folder.rs`、`viewer_context_registry.rs` | 疎でも密でも控えとして持てる。context の所有と swap は今のまま |
| リモート | `remote_ipc/` | 独立した DB 読みと私有の控え。影響なし |
| K2・一括キャッシュ処理・詳細情報 | `catalog.rs`、`cache_ops.rs`、`metadata_ops.rs` | DB を直接読む、または独立した全行の写し。影響なし |
| idle upgrade・HarvestOnCancel・進捗 | `thumb_loader.rs`、`app.rs` | 控えの総数に依存しない。SourceOnly の bypass、保存後の登録、要求単位の終端の計数は今のまま |
| test-script | `test_script/release_checks.rs` | 控えに無いことと DB に無いことを別々に確かめており、そのまま使える。空の控えで元画像からの読みを期待する fixture は直す |
| 類似画像の worker | `ui_metadata_panel.rs` | 私有の空の控え・catalog 無しで共通処理を呼ぶので、DB を引かない |

### 4S.4 表紙の失効 (§1.773b) との関係 (r5 P2-2、r6 で解消を確認)

§1.773b (`docs/cover-crop-plan.md` §29) の契約: 失効 worker が DB と控えから対象の行を消し、UI は完了を受けて
`clear_deleted_cover_memory` で今の控えからも消し、対象の idx (表示中・要求中) を退避して読み直す。表示した素材にはセッション番号を
付け、復元のときに比べる。**最後の旧 worker の DB・控えへの書き戻しは割り切る** (§29.7)。

- **DB から引いた行を控えに登録しない** (4S.1-3)。r5 の順序の最後の手 (旧行を控えへ登録) が無くなり、後の要求は毎回 DB を引くので、
  削除済みの行は引かない。r6 で「この窓は解消する」と確認された。
- 引いた要求そのものの結果は、今の要求中の扱い (退避で request ID を失う、または採った素材を退避して読み直す) で塞がっている。
- 控えに入るのは、この一覧の worker が保存した行と、UI が一覧の開始時に seed した行だけ。仮想フォルダの seed (親の行を仮想 catalog へ写す) は
  今の UI の seed と同じ写しで、失効と重なったときの扱いも今と同じ割り切り。動画ピンの行は NotAutomatic で失効の対象外。
- 自動縦横比の見本は 4S.1-7 で新しい採用経路を作らないので、r6 P2-4 (失効をまたいだ見本の採用) は生じない。到着した素材の見本は
  今どおり失効の処理 (`poll_cover_rotation_cache`) で除かれる。
- 合成ビューの準備済みの全行の写しが、失効の完了後に採られる窓は S1 の前からある (§7-4)。
- 回帰テスト: 「worker が DB から行を引いた後に、失効が DB と控えから消し、UI が完了を処理し、そのあとで同じ鍵の新しい要求が来る」
  順序を固定し、新しい要求が旧行を使わず (控えにも無い)、作り直すことを確かめる。

### 4S.5 変えないこと・状態

- 一覧ごとに足す状態は無い。seed は今の UI の位置のまま。writeback は先頭ページの要求に載せる値。仮想フォルダの「親との対応」は今の
  `VirtualFolderWriteback` の置き換え。App に足すものは Box にする。
- 片付け (`delete_missing`) は今のまま UI で (全種類表示のときだけ)。表示を絞っている利用者の古い行は段階 C (後段)。
- 永続データの形は変えない (catalog の schema・鍵・行の中身は同じ)。
- まれな失敗 (DB の読み書きの失敗など) は既定の割り切り (ログ + 既存の通知、次の要求でまた行う)。

### 4S.6 通常の使い方への影響 (見込み)

- 閉じて一覧へ戻るとき、UI の 134〜221 ms の停止が無くなる。seed は鍵 1 件の読みが動画ピンの数 + 1 件増える (4S.1-5 の表)。writeback の UI の catalog の保存は無くなる。
- worker は要求ごとに鍵 1 件の SELECT が増える (OS のキャッシュが温まった状態で 0.03 ms)。デコード (数 ms) に比べて小さい。
  同じ項目を表示し直すと毎回 DB を引くが、今の Parent・collection の lazy 読みと同じ。
- メモリは、控えが一覧で保存・seed した行の分だけになる。
- **自動縦横比 (利用者に確認する)**: 今は一覧を開いたとき、catalog にある行から見本を取り、最初の描画の前に比を決め直せる (入力 idle・
  連勝のゲートを飛ばす即決)。S1 では catalog からの見本が無くなり、見本はサムネイルの到着で入って、今のゲートを通って決まる。
  - フォルダの縦横比の記録 (`auto_aspect_cache`) がある一覧 (一度自動で決まった一覧。本を閉じて戻る一覧はこれ) では、最初の描画は
    記録した比を復元して今と同じ。その後の判定は、表示して届いた見本に依存する (下の 2 点)。
  - 記録が無く catalog には行がある一覧 (以前に自動をオフにしていた、記録を消した等) では、最初の描画は既定の比で、サムネイルが
    届いてゲートを通った後に切り替わる。初めて開くフォルダと同じ動き。
  - **一覧の順から、表示して届いた項目へ**: 今は一覧の先頭からの順で catalog にある行を見本にする。S1 では実際に表示・先読みして
    届いた項目が見本になる。内容が同じでも、復元したスクロール位置や見た範囲によって、決まる比 (と記録される比) が変わり得る。
  - **待つだけでは決まらないことがある**: 必要な見本の数 (項目数の 1/4 で 8〜24 件。項目が 8 件未満の一覧では全項目) が表示・先読みの範囲で届く数より多いと、
    スクロールなどで別の項目を表示するまで、既定の比・記録した比が続く。記録した見本の数によるゲート (`cached_sample_gate`) も
    今どおり残る。
  - collection も多くはこの動き (準備済みの控えに行があるときだけ、最初の見本を取れる)。
  - 今と同じ即決を残す案 (catalog からの見本を集める job を足す) は r6 で、seed との順序・失効との有効性・job の占有・UI の要求の
    組み立ての I/O の 4 点の指摘を受けた。残すなら、これらを塞ぐ仕組みが要る。

### 4S.7 検証と計装

- worker: 控えが空でも、DB にある行 (stamp 一致) を使い、作り直さない。stamp 不一致は使わない。引いた行を控えに登録しない。
  EPUB の世代付きの鍵・ピンの鍵・自動切り抜きの鍵でも同じ。retired な catalog は引かない。色フィルタの worker も同じ。
- seed: 控えが空で DB に同じ動画ピンの seed がある → 書き直さない (今の `already_seeded` と同じ結果)。WebP が変わった → 書き直す。
  WebP が無く DB に古い行がある → 消す (Codex Phase C P3 の再発防止)、行が無い → 書かない。仮想 catalog に先頭ページの行がある →
  seed しない、無い → 親から写す (今と同じ条件、`cache_book_cover_only` ON では写さない)。画面外の項目も一覧を開いたときに揃う。
- writeback: 生成して保存した → 開いた時点の stamp で親へ写す。親ファイルが変わった → 写さない。保存を省いて既存行がある → 写す、
  無い → 写さない。cache hit → 写さない。SourceOnly の保存 → 写す。繰り返しの要求 → そのたびに写す (4S.1-6 の差分)。EPUB は対象外。
  付加処理が source・鍵・世代・idx の一致でだけ載せる。色フィルタ・独立プレビューには載らない。context の swap で対応が保たれる。
- pin_materialization: 控えが空で DB に古いピンの行がある → 消す。
- 表紙の失効: 4S.4 の順序の回帰テスト。
- 自動縦横比: 通常の一覧で見本を探す処理を呼ばない (UI で全件の要求を組み立てない)。到着した見本で今のゲートどおりに決まる。記録の
  復元は最初の描画の前。合成ビュー・collection は今どおり控えから見本を取る。
- perf:
  - `thumb_catalog_lookup` を Fixed に対応させ (`catalog_kind` に `fixed`)、接続の待ちを含む時間・hit を出す。
  - 一覧の読み込みの区間は `sli_catalog_load_all` を「控えの用意」の区間に改め (`entries` は控えの件数)、`analyze_perf.py` の
    区間名も合わせる。ログの `entries in DB` は控えの件数の表記へ改める。
  - seed の区間 (今の `sli_catalog_load_all` に含まれていた部分) の時間と、足した DB の読みの件数を出す。writeback の回数と時間を出す。
- ベンチ (`bench_scroll`): 「控えあり (今の全行)」「DB あり・空の控え (S1)」「SourceOnly」を分ける。今の「キャッシュなし」は
  空の控えと既存の DB を渡しているので、S1 では DB hit になり意味が変わる。合成した 1 万件の catalog で、初期表示とスクロール、
  新しい接続 (SQLite のページキャッシュが冷えた状態) と温まった状態、空の DB・stamp 不一致の多い DB・部分的にある DB、worker の
  少ない設定を比べる。OS のファイルキャッシュが冷えた状態は利用者の実機の perf ログで見る。
- 受け入れの条件: (1) 1 万件で UI の閉じる・開くの区間が今より短い、(2) 初期表示の可視範囲のサムネイルが出るまでの時間が、今の
  全行の控えより遅くならない (同程度)。満たさなければ実装を止めて設計へ戻す。
- 利用者の実機確認 (`build-dev.ps1`): 1,000 冊級のフォルダで本を開いて閉じ、閉じた直後の大きなフレーム間隔が消えたことと、
  サムネイルが作り直されず (PDF の描き直しが走らず) に出ることを perf ログで見る。

## 4. (r3 までの案) 直し方 (段階 A を v5.3.0 で必須、段階 B は条件付き、段階 C は後段)

> 2026-11-17: §3.5 の比較で S1 に切り替える方向。以下は r3 までの案として残す。段階 C は S1 でも同じく後段。

r1 レビュー (§8) を受けて改訂した。段階 A は**今の意味を変えずに場所だけ worker へ移す**ことに絞り、行を減らす工夫は段階 B に分けた。

### 段階 A: 通常の一覧の catalog の開く・読み込み・片付けを worker へ移す (必須)

合成ビューで既にある形 (prepare worker が `PreparedAggregateCatalog` を作り、continuation が受け取る) を、通常の一覧にも使う。

**対象**: `start_loading_items` の continuation を作る唯一の製品コードの箇所 (src/app.rs の `SidecarLoadContinuation { .. }`) を通る一覧
すべて。通常フォルダ、PDF/EPUB の仮想フォルダ、ZIP・変換書庫の物理一覧。合成ビュー (`prepared_aggregate`) は今の経路のまま。
ZIP 内の階層移動 (`IndexOnly`、`install_new_items` の軽量経路) は continuation を作らないので対象外。

**型付きの状態 1 つ**: continuation に `catalog: FolderCatalogPreparation` を持たせる。

```text
enum FolderCatalogPreparation {
    NotRequested,                 // 起動しない (下の「起動の判断」)
    Waiting { rx, cancel, invalidations: Vec<完了した表紙の失効> },
    Ready   { prepared: PreparedFolderCatalog, invalidations: Vec<…> },
}
struct PreparedFolderCatalog { db: Option<Arc<CatalogDb>>, entries: HashMap<String, CacheEntry> }
struct FolderCatalogRequest { scope: CatalogLoadScope, delete_missing: bool, work: CatalogWork, warm_db: Option<Arc<CatalogDb>> }
```

App に別の pending / bool は足さない。所有者はサイドカー復元の continuation で、取り消し・別フォルダ・閉じて開き直す・context の
退避は、今の continuation の経路にそのまま乗る。

**起動の判断 (continuation を作る時点で 1 回だけ決める。r2 指摘 1)**: 読む範囲・片付けの可否・起動するかは、worker を起動する前に
呼び出し元から渡す。後から worker をやり直さない。

| 一覧 | 起動 | 範囲 (段階 B) | 片付け |
| --- | --- | --- | --- |
| 項目のある通常フォルダ、PDF/EPUB の仮想フォルダ、ZIP・変換書庫の物理一覧 (ZIP は全階層の inventory) | する | `Inventory` | 今と同じ条件 |
| DirectMedia になり得る読み込み (起動時の open で、指定ファイルが種類の表示で隠れる場合。`startup_ops.rs` の `result.direct_media`) | する (一覧が空でも) | `All` | 今と同じ条件 (一覧が空なら しない) |
| 上以外の空の一覧 (列挙の失敗・パスワード待ちなど) | しない (`NotRequested`) | — | しない |
| 合成ビュー・テストの fixture | 今の経路のまま | — | — |

DirectMedia になり得るかは、親フォルダの読み込みを頼む時点で分かっているので、その印を読み込みの要求に載せて
`start_loading_items` まで運ぶ。continuation の `navigation_after_hydration` (後から付く) では判断しない。DirectMedia を作る製品コードは
`startup_ops.rs` の起動時 open の適用経路 1 か所 (r3 で確認)。印は `InitialStartup` だけでなく、activation・Agent も通る共通の
適用経路から渡す。Bookmark は `result.direct_media` を作らないので対象外。

**worker の中身 (今の UI の処理と同じ順序・同じ条件)**: 書き手の準備 → 読み込み → (今と同じ条件なら) `delete_missing`。
初めてのフォルダ・移行前の DB も分岐させず、次の一本道にする (r3 指摘 3):
1. 書き手の準備: 受付時に開いてある `Arc<CatalogDb>` があればそれを使う。無ければ受付時に捕捉した `CatalogWork::open` で開く
   (作成・WAL・スキーマの初期化・PDF 寸法の移行まで終える。今の `get_or_open_catalog` と同じ)。
2. 読み込み: 同じ受付時の admission で `CatalogWork::open_existing` (= `open_existing_read_only_admitted`) の別の読み取り専用の接続を
   開き、読み込みを終えて statement と読み取りの transaction を閉じる。本当に空なら SELECT が空を返す (「file が無かった」の観測で
   読み込みを省かない)。移行は 1 で済んでいるので、移行前の行を map に取り込まない。
3. 片付け: 2 の後、共有の接続 (書き手) で `delete_missing` (区切って行う。下記)。
- worker の中で新しい admission を取り直さない (catalog の保守との順序を受付時点で固定するため。src/catalog.rs の admission)。
  lease は各段の SQL の間だけ握り、合流待ちまで持ち越さない。保守が途中で受け付けられたら、古い admission の SQL は拒否される。
  その場合は失敗として扱い (下の「失敗」)、片付けまで完走させる保証はしない。
- **長い読み込みは別の読み取り専用の接続で行う (r2 指摘 2)**。`CatalogDb` は 1 本の接続を Mutex で守り、`load_all` は読み込みの間
  ずっと握る (`CatalogDb::phase`)。一方、仮想フォルダの writeback (`app.rs` のサムネイル完了の poll から親 catalog へ同期保存) は
  UI から同じ接続を取る。共有の接続で読むと、例えば別ウィンドウの PDF の writeback が、メインの先読みの SELECT を UI で待つ。
  そこで読み込みは上の 2 のとおり別の読み取り専用の接続で行う (WAL なので読み手は書き手を止めない)。共有の接続 (`db`) は、
  サムネイル worker に渡すためと、片付けだけに使う。
- **片付けは区切って行う (r3 指摘 2)**。今の `delete_missing` は両表の全行名の取得・判定・削除・commit を 1 回の Mutex の保持と
  1 つの transaction で行うので、worker に移すと、その間 UI の writeback が待つ (WAL でも書き手は 1 つ)。全種類表示では毎回走り、
  大量に消す回もあるので割り切れない。そこで `delete_missing` を「行名の取得 (2 ms 程度) → 消す行の判定 → 一定数 (例: 64 行) ごとに
  `phase()` を取り直して DELETE + commit」に分け、区切りの間で Mutex と SQLite の書き込みの権利を手放す。UI の writeback の待ちは
  1 区切り分 (数 ms) が上限になり、サムネイル worker の保存と UI の writeback が今すでに同じ接続を順に使っているのと同じ種類の待ちに
  収まる。区切りの間に同じ名前の行が保存し直されても、消す判定は今と同じ「存続キー集合に無い」なので結果は変わらない。
  - 代わりに UI の writeback を worker へ移す案は、writeback の書き手が UI 以外にも増え、仮想フォルダの seed と writeback の順序の
    所有が変わるので採らない。
- 読み込みと片付けの順序は今のまま「読み込み → 片付け」。片付けで消した行も map には残る (今と同じ)。DirectMedia の明示対象は
  `catalog_existing_keys` に入らないが、今は map に残るので使える。順序を変えるとこれが失われる (r1 指摘 3)。
- 片付けの条件は今と同じ (`!is_synthetic_view_path && is_all_visible`)。表示を絞っている利用者の古い行は段階 C で扱う。
- 失敗 (開けない・読めない): ログを出し `db: None`・空の map で続ける。まれな失敗は割り切り (ログ + 次回の読み込みで再試行)。
  UI で開き直さない (合成ビューと同じ)。
- 完了したら結果を送り `ctx.request_repaint()`。

**合流点**: `sidecar_restore.rs` の終端境界で、既存の `keep` の判定 (state を戻して次のフレームで再 poll するかの判断、
`poll_sidecar_restore_projected` の終端の手前) に catalog の状態を加える。位置は、終端の共通処理 (`spread_shift.resolve`・警告の
表示・効果の適用・`favorite_failures`・`take_live()`) より**前**。各 phase の中で既に 1 回だけ行われる処理 (Checking の採用など) と、
共通の終端処理を分けて扱う (r2 指摘 3)。
- `Waiting` で未受信なら `keep` として state を戻し、次のフレームで再 poll する。共通の終端処理は合流できた 1 回だけ行う。
- 受信したら `Ready` にして続ける。context・generation・source の照合は今の終端の照合をそのまま使う。
- `Ready` は continuation の catalog 節 (`resume_loading_items_after_sidecar` の「カタログを開く + cache_map ロード + 削除掃除」) で
  採用するまで continuation に残す。DirectMedia は同じ continuation を再 hydration するので、`Ready` を持ち越して 2 回目の終端では
  待たない。
- 空の一覧の直接 resume (`start_sidecar_restore` の `self.items.is_empty() && !restore_sidecar` の枝) は、条件を「空の一覧かつ
  サイドカー不要**かつ catalog が `NotRequested`**」に狭める (r3 指摘 1)。catalog が `Waiting` (DirectMedia になり得る空の一覧) なら
  直接 resume せず、既存の復元の所有者を作り、空のページ索引を準備済みとして共通の合流へ進める。別の pending や worker の
  やり直しは足さない。`NotRequested` の空の一覧は、catalog 節で「開いてある catalog があればその Arc、無ければ None」・空の map で
  続け、読み込みも片付けもしない。
  - 今は空の一覧でも UI で開いて全行を読み、全種類表示なら空集合で `delete_missing` (= `audioart:` 以外の全行削除) を呼ぶ。
    r2 の確認で、ZIP・PDF の列挙の失敗・worker の切断、PDF のパスワード待ちの一部の枝で実際に到達する (ZIP の純粋な読み込み中の殻は
    到達しない)。失敗や待機は物理的な消失の証拠ではないので、空の一覧で片付けないのは正しい変更とした (§8 Q1)。

**表紙の失効との競合 (r1 指摘 1)**: 表紙の回転などで catalog の行を消す処理 (`cover_rotation_cache`) の完了は、
`poll_cover_rotation_cache` が `current_color_cache_map` から該当行を消して知らせる。合流前は `current_color_cache_map` が `None`
なので、worker が読んだ後・採用前に完了した失効が map に反映されない。本を閉じる直前に回転した場合が該当し、まれではない。
- `poll_cover_rotation_cache` (App 全体の poll) で完了を受け取ったとき、`Waiting` / `Ready` の continuation があれば、投影中の
  context に関係なく (AtRest の対象でも) その `invalidations` にも積む。
- 採用時、`entries` に `clear_deleted_cover_memory` と同じ処理を `invalidations` で当ててから `cache_map` にする。範囲を限る
  `ThumbnailPoolCatalog` は、continuation の `source_path` から作る (読み込み中は現在の pool が `None` に戻っているので、投影中の
  窓の pool は使わない。r2 指摘 3)。取り除いた `CacheEntry` は worker 側の破棄経路へ渡す。
- 読み込み前に完了した失効は DB に反映済み、採用後の完了は今の経路 (`current_color_cache_map`) が受け持つ。窓の両端が閉じる。
- 同じく採用前の map に作用する他の失効・書き込みが無いかを実装で列挙する (`current_color_cache_map` の書き手: collection_grid、
  smart_folder の退避・復帰、viewer_context_registry の swap、cover_rotation_cache)。

**取り消し (r1 指摘 5)**: `ContinuationOwner::discard()` で `Live` を `Discarded` へ置き換えるとき、catalog の状態
(`Waiting` の受信口、`Ready` の結果) を既存の worker 側の破棄経路 (`retire_smart_folder_payloads` 等) へ移し、大きな map の drop を
UI でしない。worker は cancel を見て、取り消し後は送らない。送信済み・未受信のまま取り消された場合も受信口ごと worker 側で破棄する。

**簡素化の比較 (r1 の求め)**:
- 直列 (サイドカーの終端の後に catalog を読む): 合流が要らず状態は減るが、表示開始までの時間は今と同じ「サイドカー + 読み込み」で、
  UI が固まらなくなるだけ。
- 並列 (採用案): 閉じるときのサイドカーの待ち 70〜122 ms (§2) が読み込みと重なる分だけ早い。増える状態は continuation 内の
  `FolderCatalogPreparation` 1 つで、終端・取り消し・失効の 3 か所で扱う。利用者の不満は表示までの間 (ワンテンポ) なので並列を採る。
  実装後に perf で `sli_catalog_join_wait` (合流で待った時間) を見て、重なりの効果を確かめる。重なりがほとんど無いと分かったら
  直列へ戻す。
- 親フォルダの `cache_map` を保持して戻るときに使い回す案: 設定変更・外部の更新・writeback・表紙の失効への追従が要り、保持する状態が
  増えるので採らない。
- サイドカーの Checking worker の中で読む案: サイドカーが無効なときなど Checking を通らない経路があり、分岐が増える。
  `PreparingPageIndex` (空でない一覧は必ず通る) に入れる案は、ページ索引の準備と直列になり並列の利点が無い。

効果の見込み (推定): 閉じた直後の 186〜258 ms のフレームが無くなる。サムネイルが出始めるまでは「サイドカーの待ち + 読み込み」
(230〜370 ms) から「長いほう」(この利用者で 130〜220 ms) へ縮む。

### 段階 B: 今の一覧が読む行だけを読む (条件付き)

- worker の読み込みを、`filename` だけの SELECT (2 ms) で行を絞ってから本体を読む形にする。このフォルダでは 3,124 行・87 MB →
  約 1,466 行・54 MB。v5.3.0 の自動切り抜き v4 の組が増えても読む量は増えない。段階 A で読み込みが worker に移るので、B は
  表示開始の時間とメモリのための追加で、UI の停止とは関係しない。
- 絞り方は完全一致ではなく、`delete_missing` と同じ `thumbnail_cache_key::inventory_retains_key(existing, name)` で残る行
  (r1 指摘 2: EPUB ピンの `|generation:mtime:size` 付きの行は pending の鍵から派生する)。表示時の鍵・stamp の照合は今のまま。
- 適用範囲は型で持つ: `CatalogLoadScope::{All, Inventory(Arc<HashSet<String>>)}`。どちらにするかは段階 A の「起動の判断」の表で、
  worker の起動前に決める。`Inventory` は「今の一覧の鍵を全部入れた」と分かっているときだけ。DirectMedia になり得る読み込みは
  `All` (明示対象が集合に入らないため。r1 指摘 3・r2 指摘 1)。空集合は目印にしない。
- 読み手の確認 (r1 の表): 色フィルタ・自動縦横比・動画ピンの seed・仮想フォルダの seed は要求の鍵か集合内の鍵を使う。リモートと
  K2 は DB を直接読むので map の縮小と独立。表紙回転のキャッシュは map と DB の見出しを併せて読み、map に全行は要らない。
- 採る条件: 上の読み手の確認をテストで固め (EPUB の直接ピン・cascade ピン、DirectMedia、色フィルタ)、段階 A と同じ版に入れられる
  場合だけ。間に合わなければ A を先に出す。

### 段階 C: 表示を絞っている利用者の古い行の片付け (後段、v5.3.0 には入れない)

- 段階 B で時間への影響は消えるが、ディスク上の古い行は残る (このフォルダで 33 MB)。
- 案: 表示中の項目について、今の鍵と同じ項目・同じ表示方針で、自動切り抜きの印だけが違う行 (旧版・逆の読み方向) を消す。
  隠した種類のファイルの行と、別の表示方針の行は今の意図どおり残す。
- K2 (フォルダの自動代表が子の本の v1 の行を流用する、docs/cover-crop-plan.md、`thumb_loader.rs` の `legacy_auto_child_keys`) と
  衝突するので、v5.3.0 の締めには間に合わせない。§1.800 の後段に残す。

## 5. 通常の使い方への影響

- 退行なし (見込み)。表示されるサムネイルは同じ。閉じた直後に固まる 1 フレームが無くなり、空のセルが見える時間は今のサイドカーの
  待ちと同程度。
- 変わる点: 空の一覧で catalog を開かず、片付け (全行削除) もしない。後で項目が入った一覧を読み込むときに今どおり片付く。
  列挙の失敗・パスワード待ちの一覧で、今は消えていたサムネイルのキャッシュが残るようになる (良い方向)。
- メモリは段階 B で減る (このフォルダで約 33 MB)。
- 永続データ: 形を変えない (読む場所と範囲だけ)。段階 C を行う場合も、消すのは作り直せるサムネイルのキャッシュだけ。

## 6. 検証

- 状態遷移 (段階 A):
  - 「サイドカー先 → catalog 後」「catalog 先 → サイドカー後」で、合流・worker 起動・選択の復元・first-display action が各 1 回。
  - 合流待ちの間に取り消し / 別フォルダへ移動 / 閉じて開き直す: 古い結果を採らない。送信済み・未受信の取り消しで map が UI で
    drop されない。
  - 「読み込み完了 → 表紙の失効の完了を poll → 合流」で、失効した行が `cache_map` に無い (決定的な回帰テスト)。
  - DirectMedia: 「表示できる兄弟あり / なし」×「サイドカー有効 / 無効」の 4 通りで、範囲が開始前に `All` に決まり、`Ready` を持ち越して
    2 回目の終端で待たず、明示対象の既存の行を使う (4 通りとも作り直さない)。catalog の準備と
    サムネイル worker の起動が各 1 回。
  - 空の一覧 (DirectMedia でない): catalog を読まず、片付けも呼ばない。直接 resume の枝に `Waiting` で来たときは待たずに破棄する。
  - 読み込みが別の読み取り専用の接続で行われ、先読み中に UI から共有の接続 (writeback) を取っても長く待たない。片付けの最中も、
    UI の writeback の待ちが 1 区切り分に収まる (区切りの間で Mutex を手放すことをテストで確かめる)。
  - catalog の DB: 初めてのフォルダ (file が無い)、移行前の古い DB、保守 (admission) の受付が読み込みの前・最中・後の各場合で、
    移行前の行を map に入れない・新しい admission を取り直さない・保守の後の SQL が拒否されて失敗扱いになる。
  - detached viewer の context (Mounted / AtRest / Retired) で、対象の context だけが変わり、兄弟の context が変わらない。
- 回帰: 全種類表示の読み込みで、今と同じ行が DB に残る。合成ビューの経路が変わらない。
- 段階 B: `inventory_retains_key` と同じ行を返す (EPUB の直接ピン・cascade ピンの世代付き行を含む)。`audioart:` を除く今の扱い。
  DirectMedia は `All`。
- perf (r1 指摘 6): `sli_sidecar_import` はサイドカー単独の終端までに改め、次を分けて同じ要求番号で出す:
  `sli_catalog_preload` (worker: open / load / delete の各 ms・entries・scope)、`sli_catalog_join_wait` (サイドカーの終端から
  合流までの待ち)、`sli_catalog_adopt` (UI: 失効の適用・seed・採用)。`scripts/analyze_perf.py` の `nav` の固定イベント一覧と
  「SLI 全体 (UI)」の表記を合わせて直す (今の `sli` は非同期の待ちを含む経過時間)。
- 利用者の実機確認 (開発版 `build-dev.ps1`): 1,000 冊級のフォルダで本を開いて閉じ、perf ログで閉じた直後の大きなフレーム間隔が
  消えたことと `sli_catalog_join_wait` を見る。最終の速さの比較は release 構成 (次の版のインストーラ) で行う。

## 7. 本件の範囲外で見つかったもの (別項目の候補、いずれもログからの読み・未確認)

1. 閉じた直後のフレームで `keep_fullscreen_viewport` が 56 ms (23.218 s)。閉じた全画面の後始末か描画が UI で重い。
2. 開く側: 最初の全画面フレームが 133 ms。`fs.final_composite_build` が保持していた AI 拡大済みのページ (4800×6916、idx 25)
   の合成に 86 ms (`post_ms` 45・`upload_ms` 40)。開いたページ (idx 0) とは別のページの合成が、最初の描画の前に UI で走っている。
3. `sli_settings_save` が開く・閉じるたびに 6〜9 ms (起動直後は 212 ms)。`last_folder` の保存が UI で同期。
4. (r5 の指摘からのコードの読み、S1 の前からある) 合成ビューの準備済みの全行の写し (`prepared_aggregate.catalog`) は worker が
   DB から作るので、表紙の失効 (§1.773b) の完了より後に UI が採ると、消えた行が写しに残る。S1 は合成ビューの経路を変えないので
   本件では扱わない。
5. (r6〜r9 の検討から、S1 の前からある) 一覧の開始時の動画ピン・仮想フォルダの seed は UI で同期に `video_pins.db` の WebP を読み、
   catalog に書く (動画ピンの数に比例)。worker へ移すには、ピン更新との順序、`video_pins.db` の WAL と reader の排他、pool の作り直しを
   またぐ寿命を設計する必要があり (§8 r6〜r9)、本件とは分けて扱う。

## 8. 設計レビューの記録

### r1 (GPT-6.1 Sol / xhigh、2026-11-17、0854e6fbb に対して) — REVISE、P1 なし

調査の (a)〜(d) はコードと一致 ((d) は通常のサムネイル経路について。retired な CacheOnly と色フィルタの代表画像は map 未ヒットで
`load_one` する例外がある)。指摘と対応:

| 指摘 | 対応 |
| --- | --- |
| P2-1 先読みした map が、採用前に完了した表紙の失効を取りこぼす | §4 段階 A「表紙の失効との競合」: 完了を continuation に積み、採用時に当てる |
| P2-2 段階 B の完全一致では EPUB ピンの世代付きの行を読み落とす | §4 段階 B: `inventory_retains_key` と同じ範囲にした |
| P2-3 DirectMedia の明示対象は存続キーに入らない。片付けを先にすると今は使える行が消える | §4 段階 A: 順序は今のまま「読み込み → 片付け」。段階 B は DirectMedia で `All` |
| P2-4 関数の入口で待つと、待つ間の所有者が無い。空の一覧の直接 resume と DirectMedia の再 hydration も要る | §4 段階 A「合流点」: `take_live()` の前で `FolderCatalogPreparation` を見て state を戻す。空の一覧は `NotRequested`、DirectMedia は `Ready` を持ち越す。admission は受付時に捕捉 |
| P2-5 送信済み・未受信の取り消しで大きな map が UI で drop される | §4 段階 A「取り消し」: 受信口・結果を worker 側の破棄経路へ |
| P3-6 perf の区間がまた混ざる | §6 の perf: 区間を分け `analyze_perf.py` も直す。最終比較は release 構成 |
| 簡素化の比較が足りない | §4 段階 A「簡素化の比較」に直列・並列・保持・Checking/PreparingPageIndex を並べた |

r2 で確かめてほしかった点 (r2 で回答あり):

- Q1: 空の一覧で `delete_missing(空集合)` が全行を消す今の挙動は、列挙の失敗・パスワード待ち・読み込み中の殻でも起きているか。
  起きているなら、空の一覧で片付けをしない (`NotRequested`) のは正しい変更か。
- Q2: 表紙の失効を continuation に積む形で窓が閉じるか。ほかに採用前の map に作用すべき書き手・失効が無いか。

### r2 (同じセッションの resume、2026-11-17、c83b7df2b に対して) — REVISE、P1 なし

r1 の P2-2・P2-5・P3-6 は解消、P2-1・P2-3・P2-4 は方式は妥当で明文化が必要との判断。Q1 は「空の一覧で片付けない」に賛成
(列挙の失敗・worker の切断・パスワード待ちの一部の枝で今は全行削除に到達する。ZIP の純粋な読み込み中の殻は到達しない)。
Q2 は再適用の範囲を固定すれば窓が閉じる。ほかの書き手 (pin materialization・context の swap・Smart の退避復帰・通常の cache fill)
は積む必要なし。

| 指摘 | 対応 |
| --- | --- |
| P2-1 DirectMedia の `All` が worker の起動に間に合わない (`navigation_after_hydration` は後から付く)。空の一覧でも要る | §4 段階 A「起動の判断」の表: 起動時 open の時点で印を運び、開始前に `All`・空でも起動。兄弟なし・サイドカー無効だけ割り切り |
| P2-2 共有の `Arc<CatalogDb>` で読むと、UI の writeback が Mutex で長い SELECT を待つ | §4 段階 A: 読み込みは受付時の admission で開く別の読み取り専用の接続。共有の接続は worker への受け渡しと短い片付けだけ |
| P3-3 失効の再適用の範囲と合流の位置 | §4 段階 A: `source_path` から作る `ThumbnailPoolCatalog`、App 全体の poll から積む。合流は既存の `keep` 判定に入れ、共通の終端処理の前 |
| P3-4 「再利用では Checking を通らない」はコードと違う | §4 の簡素化の比較を直した |

r3 で確かめてほしい点:

- Q3: 別の読み取り専用の接続で読む形で、admission (catalog の保守・退避) と矛盾しないか。初めてのフォルダ (file が無い) の順序。
- Q4: DirectMedia の印を起動時 open から `start_loading_items` まで運ぶ経路が 1 本で足りるか (ほかに DirectMedia を付ける入口が無いか)。

### r3 (同じセッションの resume、2026-11-17、088efe603 に対して) — REVISE、P1 なし

r2 の P3-3 (合流の位置・失効の範囲) と P3-4 は解消。Q3 (別の読み取り専用の接続) は原理的に矛盾なし、順序の固定が必要。
Q4 は DirectMedia を作る入口が 1 か所で足りる (activation・Agent も通る共通の適用経路から印を渡す。Bookmark は対象外)。

| 指摘 | 対応 |
| --- | --- |
| P2-1 空の一覧の DirectMedia で、起動した catalog の結果を捨てて作り直す (通常の操作で毎回起こる。割り切れない) | §4 段階 A: 直接 resume を `NotRequested` の空の一覧だけに狭め、`Waiting` なら復元の所有者を作って共通の合流へ。割り切りを撤回 |
| P2-2 共有の接続での `delete_missing` の間、UI の writeback が待つ (毎回走る通常の保守。割り切れない) | §4 段階 A: 片付けを一定数ごとの transaction に区切り、区切りの間で Mutex を手放す。待ちの上限は 1 区切り |
| P3-3 初めての DB で「読み込みは空」を判断する時点が曖昧 | §4 段階 A: 書き手の準備 → 読み取り専用で読む → 片付け の一本道。初回も分岐しない |

§6 に、初めての DB・移行前の DB・保守の受付の前後、DirectMedia 4 通りの行の再利用、片付け中の writeback の待ちの検証を足した。

### r4 (同じセッションの resume、2026-11-17、71aac1a34 に対して) — REVISE、P1 なし

r3 の P2-1 (空の一覧の DirectMedia) と P3-3 (一本道) は解消。P2-1: 片付けを区切っても `phase()` の Mutex は UI へ順番を渡さず、
UI の待ちの上限にならない。UI の writeback と seed の親 `load_one` を worker へ移す設計が要る。P3-2: 区切った片付けの契約
(取り消し・保守で止める境界、部分的な完了の許容)。P3-3: §6 に空の一覧の旧条件が残っている。

対応: r3 までの案は直すたびに仕組みが増える (今回は writeback の worker 化) ので、§3.5 の比較に基づき案 S1 (§4S) へ切り替えた。
r4 の指摘は §4 の案に対するもので、S1 では片付けを worker へ移さないので該当しない。

### r5 (同じセッションの resume、2026-11-17、dd365d954 = 案 S1 に対して) — REVISE、P1 なし。S1 の方向は支持

- §3.5 の判断 (S2 単独を退け、不要な全行の読み込みをやめる) に賛成。合成ビューの全行の写しを控えとして扱うことも可、mode は不要。
  worker の分岐は `uses_lazy_catalog` の意味を変えず、Fixed の未ヒット時の lookup を局所的に許す形なら成立する (Parent の観測・
  EPUB の世代・ピン・cascade・自動切り抜きの鍵・retired CacheOnly・Legacy の置換・SourceOnly を保てる。Fixed の PDF メタの補完は
  `initial_catalog` のまま)。
- P2-1: UI の DB I/O を増やさない。4S.1 の表の seed の照合・見本の取得は既存の一覧準備の worker にまとめ、writeback も UI は対象・
  stamp・payload の確定までにして保存を worker へ。件数の上限を UI の I/O を許す条件にしない。worker の 1 件ずつの lookup は
  共有の接続で UI の writeback と競合する機会を増やす。
- P2-2: 表紙の失効の窓は S1 でも残る (worker が旧行を読む → 失効が DB と控えから消す → UI が完了を処理 → worker が旧行を控えへ登録)。
  lookup・控えへの登録・画像の採用を、表紙の失効の所有者と同じ有効性の契約にまとめる必要がある。
  (設計担当の注: 保存した行を控えへ入れる今の経路 `thumb_loader.rs` の保存後の `map.insert` にも同じ形がある。S1 は読む回数が多いぶん
  当たりやすくなる。)
- P2-3: 表に `pin_materialization` (app/pin_materialization.rs) が漏れている。控えの鍵から DB の削除対象を作るので、疎な控えでは
  旧ピンの行が残る。
- P2-4: 自動縦横比の「最大 48 件」は誤り。今は有効な見本が集まるまで項目を探し、48 は旧行のヘッダを調べる回数の予算。worker 側で
  今と同じ順序・条件で準備する。
- P3-5: 計装 (`thumb_catalog_lookup` の分類、cache hit は `load_phases` まで進まない)、`bench_scroll` の「キャッシュなし」の意味、
  Rust の実経路での warm / cold・1 万件・writeback 競合の受け入れ条件。
- 写しの完全さに頼る読み手の照合表 (r5 の全文は統括の共有メモのフォルダの review-800-r5.md): 4S.1 の表のほか、ドライブ一覧の独立した
  `load_all` の入口 (app.rs `seed_drive_list_pin_thumbs_from_catalog` の呼び出し元) の範囲を明記すること。

r5 への対応 (r6 の改訂、§4S): 統括の指示どおり、仕組みを足して塞ぐ前に「一覧の catalog を読む・書くのはその一覧の worker だけ」に
寄せられるかを先に検討した。

| 指摘 | 対応 |
| --- | --- |
| P2-1 | 動画ピンの seed は pinned_key の要求に WebP を載せて worker で照合・保存 (4S.1-5)。仮想フォルダの seed と writeback は先頭ページの要求に載せて worker で行い、`VirtualFolderWriteback` の UI の保存をやめる (4S.1-8)。UI に新しい catalog の I/O を足さない。件数の上限を条件にしない |
| P2-2 | DB から引いた行を控えに登録しない (今の collection の lazy 読みと同じ)。§1.773b の契約 (失効後の退避・読み直し、最後の書き戻しは §29.7 の割り切り) に S1 が新しい窓を足さない (4S.4)。合成ビューの写しの窓は S1 の前からあり §7-4 へ |
| P2-3 | pin_materialization の削除候補を DB の行名から求め、動画ピンの一致判定を DB で行う (4S.2) |
| P2-4 | 候補は今と同じ順序・除外で UI が全 eligible について作り (DB を読まない)、照合 (stamp・`source_dims`・ヘッダ予算・見本数) は一覧の job で今のループと同じに行う (4S.1-6/7)。見える違い (見本が数 ms 遅れて入る) を 4S.6 に書いた |
| P3-5 | `thumb_catalog_lookup` の Fixed 対応、区間名と `analyze_perf.py`、`bench_scroll` の 3 つの mode、1 万件の受け入れ条件 (4S.7) |

### r6 (同じセッションの resume、2026-11-17、153bf9f4b に対して) — REVISE、P1 なし

- r5 P2-2 の元の窓は、Fixed の DB hit を控えへ登録しないことで解消する。4S.2 (pin_materialization) は単体で十分。計装・ベンチの三分割は妥当。
- P2-1: 自動縦横比の候補を全項目について UI で作ると、ピンの cascade・stat・動画 WebP の読みが UI に増える。
- P2-2: 見本の job は seed と順序の依存がある (仮想 seed・動画ピンの seed より先に走ると同じ見本にならない)。
- P2-3: 古い動画ピン行の削除 job は、起動時の snapshot で、後のピン更新が書いた正しい行を消し得る (要求に載せた古い WebP の保存も同じ)。
- P2-4: 見本の結果を世代だけで採ると、表紙の失効をまたいだ旧見本を再登録する。
- P2-5: writeback の時点の stat を、旧版から作った WebP の stamp として保存してはいけない。
- P3-6: 要求への付加の入口と、job の結果の配送の境界 (poll_thumbnails の request 相関の前で分ける) の具体化。
- P3-7: 全件 miss の job の占有、記録がある一覧でも見本が後から比を変え得る点。

r6 への対応 (r7、§4S): 指摘の多くが「見本の job」と「順序に依存しないとした削除 job」から出ていたので、job をやめた。

| 指摘 | 対応 |
| --- | --- |
| P2-1・P2-2・P2-4・P3-7 | 見本の job をやめ、catalog からの見本は控えからだけ取る (collection と同じ、4S.1-7)。見える違いは 4S.6 に書き、統括経由で利用者に確認する |
| P2-3 | 動画ピン・仮想フォルダの seed と古い行の削除は、今の UI の順序ごと pool の最初の 1 手 (一覧の準備) へ移し、要求はその後に取り出す (4S.1-5)。queue で待つ job ではないので、起動時の snapshot が後で効くことは無い。一覧の開始から数 ms の間にピン更新が重なる場合は既定の割り切り |
| P2-5 | 開いた時点の親の stamp を要求に載せ、worker の stat と一致するときだけ、その stamp で写す (4S.1-6) |
| P3-6 | 付加は pool へ出す要求の共通の付加処理 1 か所で、source・鍵・世代・idx の一致でだけ載せる。色フィルタ・独立プレビュー・Remote・自動縦横比には載せない (4S.1-6)。新しい payload は無くなった |

### r7 (同じセッションの resume、2026-11-17、91830780c に対して) — REVISE、P1 なし

- r6 で確認済みの部分 (控えへ登録しない、4S.2、writeback の stamp と付加の入口、payload の撤去、計装) に新たな問題は無い。
- P2-1: `reset_and_seed_auto_aspect_with_seed` をそのまま呼ぶと、空の控えでは見本が見つからず全項目の要求を UI で組み立てる。
- P2-2: pool の門は、pool の作り直しとピン更新の書き込みを順序づけない (準備が未完了のまま後継の pool が動く、旧準備が新しい行を消す)。
  「数 ms」「通常操作では重ならない」は保証できない。
- P2-3: 自動縦横比の影響の説明が狭い (見本が一覧順から表示した項目へ変わる、待つだけでは決まらないことがある)。
- P3-4: context の swap では親との対応を保持する。P3-5: writeback の今との差分を明記する。

r7 への対応 (r8、§4S):

| 指摘 | 対応 |
| --- | --- |
| P2-1 | 関数を「リセットと記録の復元」と「控えから見本を探す」に分け、通常の一覧は前者だけを呼ぶ (4S.1-7) |
| P2-2 | 一覧の準備と門をやめ、seed・削除を項目の要求に載せる。各要求が自分の事実で行を確かめて書き直すので、順序づけが要らない。古い書き込みが後から入った分は次の要求で直り、表示は要求自身の事実から作る。直接行を読む別の読み手が間だけ古い行を見るのは §29.7 と同じ割り切り (4S.1-5) |
| P2-3 | 4S.6 に 2 点を追加し、統括経由の利用者への確認も補う |
| P3-4・P3-5 | 4S.1-6 に swap での保持と、今との意図した差分を書いた |

### r8 (同じセッションの resume、2026-11-17、10618dcff に対して) — REVISE、P1 なし

- r7 の関数分割、writeback の swap での保持・stamp の運搬・意図した差分は対応できている。状態の組み合わせは減っている。
- P2-1: ピン更新で控えが複製・置換されるので、新しい控えは正しく DB だけ古い順序がある。控えの一致を DB の一致とみなすと直らない。
- P2-2: 要求されない項目の seed・削除が無くなる。cascade・ドライブ一覧は `video_pins.db` を見ずに子の行を読むので、画面外の項目の
  古い行が競合なしでも長く残る。仮想 seed も、色フィルタが先に走ると親の行を使えない。
- P2-3: 仮想 seed の `cache_book_cover_only` の条件が抜けている。P3-4: 自動縦横比の説明に修正前の限定が残る。
- dead pin の base_req 方式は、削除を通常の cache hit より前に行い、visibility 付きの最終の鍵を使えば Phase C P3 の再発を防げる。

r8 への対応 (r9、§4S):

| 指摘 | 対応 |
| --- | --- |
| P2-1 | 照合の相手を保存先の DB の行にし、控えの一致を DB の一致とみなさない。書く内容は worker がその時点で読む正本 (`video_pins.db`・親の行)。表示は照合済みの正本の画素 (4S.1-5) |
| P2-2 | 一覧の開始時に、動画ピンの項目と仮想フォルダの先頭ページの照合だけの要求を pool に入れ、今と同じく画面外の項目も一覧を開いたときに揃える。正本をその時点で読むので、queue で待っても古い写しを書かない。色フィルタにも親から読む許可を載せる (4S.1-5) |
| P2-3 | `CacheDecision.cache_book_cover_only` で写すかを決める。SourceOnly は親を使わず writeback だけ (4S.1-5) |
| P3-4 | 4S.6 を「記録があれば最初に復元し、その後は表示して届いた見本に依存する」に統一し、小さい一覧の見本数を補った |


### r9 (同じセッションの resume、2026-11-17、58f2dab31 に対して) — REVISE、P1 なし

- 保存先の DB を照合する変更、dead pin の base_req、writeback・`cache_book_cover_only`・SourceOnly・関数分割・自動縦横比の説明は改善。
- P2-1: 正本を読んでから保存先のロックを待つ間に正本と保存先が更新され、旧値を最後に書く順序が残る (「数 µs」に限定できない)。
- P2-2: `video_pins.db` は WAL が保証されておらず、worker の reader が UI の保存を待たせ得る。
- P2-3: 同じ一覧で pool を作り直すと、未実行の照合要求が失われる。P3-4: 読み取り専用接続の入力と読取失敗の扱い。P3-5: 色フィルタの記述の不統一。

r9 への対応 (r10、§4S): r6〜r9 の指摘は、seed を UI から worker へ移したことで生じたものだった (今は UI が一覧の開始時、pool の起動前に
同期で行うので、順序・寿命・WAL の問題が最初から無い)。状態の組み合わせを減らすため、seed は今の UI の位置・順序のまま残し、
控えを見ていた 3 か所だけを DB の 1 行読みにする (4S.1-5)。一覧を開くときの UI の I/O は全ての場合に今より減る (4S.1-5 の表)。
seed のループを worker へ移すことは S1 の前からある課題として §7-5 に分けた。writeback (先頭ページの要求に載せる) と 4S.2 は r9 までの形のまま。
