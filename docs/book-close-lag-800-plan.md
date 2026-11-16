# §1.800 本を Enter で開き Esc で一覧へ戻るときのもたつき — 調査と設計

- 状態: 調査・設計 (2026-11-17)。実装前。設計の独立レビュー r1・r2 = REVISE を反映済み (§8)、r3 待ち。
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

## 4. 直し方 (段階 A を v5.3.0 で必須、段階 B は条件付き、段階 C は後段)

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

DirectMedia になり得るかは、親フォルダの読み込みを頼む時点 (`startup_ops.rs` の起動時 open) で分かっているので、その印を
読み込みの要求に載せて `start_loading_items` まで運ぶ。continuation の `navigation_after_hydration` (後から付く) では判断しない。

**worker の中身 (今の UI の処理と同じ順序・同じ条件)**: 開く → 読み込み → (今と同じ条件なら) `delete_missing`。
- 開く: 受付時に開いてある `Arc<CatalogDb>` があればそれを使う。無ければ受付時に捕捉した `CatalogWork` (admission) で worker が開く。
  worker の中で新しい admission を取り直さない (catalog の保守との順序を受付時点で固定するため。src/catalog.rs の admission)。
- **長い読み込みは別の読み取り専用の接続で行う (r2 指摘 2)**。`CatalogDb` は 1 本の接続を Mutex で守り、`load_all` は読み込みの間
  ずっと握る (`CatalogDb::phase`)。一方、仮想フォルダの writeback (`app.rs` のサムネイル完了の poll から親 catalog へ同期保存) は
  UI から同じ接続を取る。共有の接続で読むと、例えば別ウィンドウの PDF の writeback が、メインの先読みの SELECT を UI で待つ。
  そこで読み込みは `CatalogDb::open_existing_read_only_admitted` と同じ形の、受付時の admission で開く別の読み取り専用の接続で行う
  (WAL なので書き手を止めない)。共有の接続 (`db`) は、サムネイル worker に渡すためと、短い `delete_missing` だけに使う。
  `delete_missing` が長くなるのは版が上がった直後の初回 (古い組をまとめて消す) だけで、その間の UI の writeback の待ちは割り切る。
- DB の file が無い (初めてのフォルダ) ときは、共有の接続を開いて作り (今の `get_or_open_catalog` と同じ)、読み込みは空。
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
- 空の一覧の直接 resume (`start_sidecar_restore` の `self.items.is_empty() && !restore_sidecar` の枝) は待つ所有者が無い。
  catalog 節に `NotRequested` または `Waiting` のまま来たら待たず、`Waiting` は受信口ごと worker 側で破棄し、「開いてある catalog が
  あればその Arc、無ければ None」・空の map で続ける。読み込みも片付けもしない。
  - 今は空の一覧でも UI で開いて全行を読み、全種類表示なら空集合で `delete_missing` (= `audioart:` 以外の全行削除) を呼ぶ。
    r2 の確認で、ZIP・PDF の列挙の失敗・worker の切断、PDF のパスワード待ちの一部の枝で実際に到達する (ZIP の純粋な読み込み中の殻は
    到達しない)。失敗や待機は物理的な消失の証拠ではないので、空の一覧で片付けないのは正しい変更とした (§8 Q1)。
  - 割り切る点: サイドカーが無効で、DirectMedia の対象のほかに表示できる項目が無いフォルダを起動時に開いた場合、DirectMedia の
    サムネイルは catalog の行を使わず 1 枚作り直す (表示はされる。データは失われない)。

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
- 割り切る点: サイドカーが無効で、DirectMedia の対象のほかに表示できる項目が無いフォルダを起動時に開いた場合だけ、そのサムネイルを
  1 枚作り直す (§4 段階 A)。
- メモリは段階 B で減る (このフォルダで約 33 MB)。
- 永続データ: 形を変えない (読む場所と範囲だけ)。段階 C を行う場合も、消すのは作り直せるサムネイルのキャッシュだけ。

## 6. 検証

- 状態遷移 (段階 A):
  - 「サイドカー先 → catalog 後」「catalog 先 → サイドカー後」で、合流・worker 起動・選択の復元・first-display action が各 1 回。
  - 合流待ちの間に取り消し / 別フォルダへ移動 / 閉じて開き直す: 古い結果を採らない。送信済み・未受信の取り消しで map が UI で
    drop されない。
  - 「読み込み完了 → 表紙の失効の完了を poll → 合流」で、失効した行が `cache_map` に無い (決定的な回帰テスト)。
  - DirectMedia: 「表示できる兄弟あり / なし」×「サイドカー有効 / 無効」の 4 通りで、範囲が開始前に `All` に決まり、`Ready` を持ち越して
    2 回目の終端で待たず、明示対象の既存の行を使う (兄弟なし・サイドカー無効は §4 の割り切りどおり 1 枚作り直す)。catalog の準備と
    サムネイル worker の起動が各 1 回。
  - 空の一覧 (DirectMedia でない): catalog を読まず、片付けも呼ばない。直接 resume の枝に `Waiting` で来たときは待たずに破棄する。
  - 読み込みが別の読み取り専用の接続で行われ、先読み中に UI から共有の接続 (writeback) を取っても長く待たない。
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
