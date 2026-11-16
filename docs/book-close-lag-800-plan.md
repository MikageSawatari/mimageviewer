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
  これらは対象の鍵が少ない (ピンの項目・見本の数件) ので、DB を鍵で引く形 (`load_one`) に直せる。ただし UI の同期 I/O が鍵の数だけ
  残るので、件数の上限と置き場所 (UI か worker か) は設計で決める。
- 影響 (見込み): サムネイルが出始めるまでが「サイドカーの待ち + 0」になり、段階 A (並列) より速い。メモリも一覧の分だけ (控え)。
  起動・フォルダ移動など、本以外の一覧の読み込みもすべて同じく軽くなる。

**判断**: S1 を採る方向で設計し直す。§4 の段階 A (並列の先読み) より状態が少なく、効果も大きい。ただし `cache_map` の意味を
変えるので、上の「行が無い = DB に無い」とみなす読み手を漏れなく列挙するのが前提で、独立レビューでその列挙を確かめる。
S1 が成り立たないと分かった場合だけ §4 の段階 A に戻る。

## 4S. 案 S1 の設計 (採用方向、r5 でレビュー)

r4 (r3 までの案へのレビュー、§8) は「片付けを区切っても UI の writeback の待ちは上限にならず、writeback 自体を worker へ移す必要がある」と
して REVISE だった。r3 までの案は直すたびに仕組みが増えるので、§3.5 の案 S1 に切り替える。

### 4S.1 変えること

1. **一覧の読み込みで `load_all` を呼ばない。** `resume_loading_items_after_sidecar` の catalog 節で、通常の一覧 (合成ビュー以外。
   通常フォルダ、PDF/EPUB の仮想フォルダ、ZIP・変換書庫の物理一覧) の `cache_map` を空で作る。catalog の open
   (`get_or_open_catalog`)・`delete_missing`・各 seed の呼び出し位置は今のまま。合成ビューは今の `prepared_aggregate` のまま
   (worker が作った全行の写し)。
2. **`cache_map` の意味を「控え」にする。** 「この一覧で worker が読んだ・書いた行」の控えで、無いことは「DB に無い」を意味しない。
   合成ビューの写しも、控えとして扱って困らない (控えに無い鍵を DB で引き直すだけ)。型や mode の印は足さない。
3. **サムネイル worker: 控えに無ければ DB を鍵で引く。** `thumb_loader.rs` の要求処理で、`RequestCatalog::Fixed` も、今の
   `lazy_scope` の要求と同じく、控えの未ヒットで解決済みの catalog に `load_one(lookup_key)` し、stamp (mtime・大きさ) が合えば
   控えに入れて使う。保存した行を控えに入れる今の処理はそのまま。retired な catalog・CacheOnly・旧表紙 (`refresh_legacy_cover`) の
   扱いは今の分岐のまま。色フィルタの worker は同じ helper を通るので同じく効く。
4. **「控えに無い = DB に無い」とみなしている読み手を、DB を引く形に直す。** コードで見つけたもの (r5 で漏れを確認してもらう):

   | 読み手 | 場所 | 今の前提 | 直し方 |
   | --- | --- | --- | --- |
   | 動画ピンの seed の `purge_stale`・`already_seeded` | `seed_folder_video_pin_thumbs` (app.rs、UI) | 控えに無ければ古いピンの行は無い | catalog を鍵で引いて判定する (対象はピン留めした項目だけ) |
   | ドライブ一覧の seed の `already_cached` | `seed_drive_list_pin_thumbs_from_catalog` (app.rs、UI) | 同上 | 同上 |
   | 仮想フォルダの seed の `already_in_virtual` | `setup_virtual_folder_seed_and_writeback` (app.rs、UI) | 控えにあれば seed 済み | 仮想 catalog を鍵で引く (1 件) |
   | ピンだけの要求の前方一致の探索 | `send_pinned_only_cached` (thumb_loader.rs、worker) | 控えを前方一致で走査 | 今の `resumed` の経路と同じ `load_latest_uncropped_with_prefix_matching` で DB を引く |
   | 自動縦横比の seed | `reset_and_seed_auto_aspect_with_seed` (app.rs、UI) | 控えの `source_dims` から見本 | 見本の鍵 (最大 `min_samples_for` の 2 倍 = 48 件) だけ DB を鍵で引く。件数の上限があるので UI に残す (perf で時間を出す) |
   | 仮想フォルダの writeback | `virtual_writeback` (app.rs、UI) | worker が控えに入れた WebP を取り出す | worker が DB から引いた行も控えに入れるので今のまま (要確認) |

### 4S.2 変えないこと・状態

- 新しい非同期の状態・合流・取り消しは無い。catalog を引く時点は worker の要求の時点だけになる。
- 表紙の失効: worker が DB を引くのは要求の時点なので、失効で消えた行は引かない。控えに入った行は今の
  `poll_cover_rotation_cache` が消す。r1 の P2-1 の窓は生じない。
- 片付け (`delete_missing`) は今のまま UI で (全種類表示のときだけ)。行名の取得は 1〜2 ms。古い行が大量にある回の削除は今と同じ
  (退行ではない)。表示を絞っている利用者の古い行は段階 C。
- 永続データの形は変えない。

### 4S.3 通常の使い方への影響 (見込み)

- 閉じて一覧へ戻るとき、UI の 134〜221 ms の停止が無くなる。サムネイルはサイドカーの待ちの後すぐ worker が引き始める。
- worker の 1 件あたりの読み込みは鍵 1 件の SELECT (温まった状態で 0.03 ms) が増える。デコード (数 ms) に比べて小さい。
  worker は共有の接続を 1 件ずつ短く使う (今の保存と同じ種類の短い待ち)。
- 起動時・フォルダ移動など、本以外の一覧の読み込みも同じく軽くなり、メモリは一覧で使った行の分だけになる。
- 自動縦横比: 見本を DB から引くので、今と同じ見本が得られる。

### 4S.4 検証

- worker: 控えが空でも、DB にある行 (stamp 一致) を使い、作り直さない。stamp 不一致は使わない。EPUB の世代付きの鍵・ピンの鍵・
  自動切り抜きの鍵でも同じ。色フィルタの worker も同じ。
- 4S.1 の表の各読み手: 控えが空で DB に古いピンの行があると消す (Codex Phase C P3 の再発防止)、seed 済みなら書き直さない。
- 表紙の失効: 失効の完了後に worker が引かない・控えから消える。
- perf: `sli_catalog_load_all` は「開く + 控えの用意 + seed」の区間として残し (entries は 0 になる)、worker の DB 引きを
  `thumb.load_phases` に区間 (例: `catalog_lookup_ms`) として出す。自動縦横比の見本の読み込み時間を出す。
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
- 写しの完全さに頼る読み手の照合表 (r5 の全文は scratchpad の review-800-r5.md): 4S.1 の表のほか、ドライブ一覧の独立した
  `load_all` の入口 (app.rs `seed_drive_list_pin_thumbs_from_catalog` の呼び出し元) の範囲を明記すること。
