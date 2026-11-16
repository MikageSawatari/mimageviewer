# §1.800 本を Enter で開き Esc で一覧へ戻るときのもたつき — 調査と設計

- 状態: 調査・設計 (2026-11-17)。実装前。設計の独立レビュー (GPT-6.1 Sol / xhigh) を受ける。
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

## 4. 直し方 (段階 A・B を v5.3.0、段階 C は判断待ち)

### 段階 A: 通常フォルダの catalog の読み込み・片付けを worker へ移す (必須)

合成ビューで既にある形 (`PreparedAggregateCatalog` を worker が作り、continuation が受け取る) を通常フォルダにも使う。

- 前半 (`start_loading_items` の continuation を作る箇所) で、通常フォルダなら catalog の読み込みを 1 回限りの worker へ出す。
  渡すもの: 開いてあれば `Arc<CatalogDb>` (UI で cold open しない。キャッシュに無ければ worker が開く)、`catalog_existing_keys`、
  片付けを行うか (今の条件と同じ)。返すもの: `PreparedFolderCatalog { db: Option<Arc<CatalogDb>>, entries }`。
- worker の中の順序: (片付けの条件を満たすなら) `delete_missing` → 読み込み。今は「読み込み → 片付け」だが、片付けた行は
  読み込んでも誰も引かない (§4 段階 B の論拠と同じ) ので、順序を入れ替えても結果は同じで、読む量が減る。
- **合流点は 1 か所**: `resume_loading_items_after_sidecar` の入口で、サイドカーの終端と catalog の結果の**両方が揃うまで
  continuation を進めない**。所有者はサイドカー復元の continuation (`SidecarLoadContinuation`) 1 つに置き、catalog 用の受信口を
  そこへ持たせる。別の pending や bool を App に足さない。
- 取り消し: continuation の `cancel` を worker にも渡す。取り消された結果は worker 側で捨てる (大きな map の drop を UI でしない)。
- 失敗 (開けない・読めない): ログを出し `db: None` で続ける (サムネイルは catalog 無しで作られる。合成ビューと同じ扱い)。
  まれな失敗は割り切り (ログ + 次回の読み込みで再試行)。UI での再試行はしない。
- worker の完了で `ctx.request_repaint()` を呼び、次のフレームで合流させる。
- 対象: 通常フォルダと、本 (PDF/ZIP の仮想フォルダ)。合成ビューは今の経路のまま。
- **状態の組み合わせ**: 新しい状態は「continuation の中の catalog 受信口」1 つで、終端はサイドカーと同じ所有者が見る。
  閉じて開き直す・別フォルダへ移る・取り消しは、今のサイドカー continuation の取り消し経路にそのまま乗る。
  `sidecar_restore` の段 (Checking など) の中に catalog を混ぜる案は、サイドカーが無い・再利用の経路などで段を通らない場合が
  あり分岐が増えるので採らない。

効果の見込み (推定): 閉じるときの 186〜258 ms のフレームが無くなる。サムネイルが出始めるまでは
「サイドカーの待ち + 読み込み」(230〜370 ms) から「どちらか長いほう」へ縮む。

### 段階 B: 今の一覧で引く鍵の行だけ読む (推奨)

- worker の読み込みを `CatalogDb::load_keys(&existing_keys)` (新設) にする。`filename` だけの SELECT (2 ms) で該当行を絞り、
  該当行だけ本体を読む。このフォルダでは 3,124 行・87 MB → 1,466 行・54 MB (今の設定で読まれる行だけ)。v5.3.0 の v4 の組が
  増えても読む量は増えない。
- 論拠: `cache_map` の未ヒットは、通常フォルダでは DB を引き直さずに元から作り直す (`thumb_loader.rs` の `uses_lazy_catalog` が
  偽の経路)。`cache_map` を引く鍵は `make_load_request` の鍵で、`catalog_existing_keys` は「`delete_missing` の存続キー集合と
  `make_load_request` の読み書きキーが乖離しない」ことを前提に同じ helper で作られている。表示を絞っていない利用者では
  集合外の行は既に `delete_missing` で消えているので、集合外の行に頼る読み手があれば既に壊れているはず。
- 適用条件は型で持つ: `CatalogLoadScope::{All, ExactKeys(Arc<HashSet<String>>)}`。`catalog_existing_keys` が空集合のまま
  continuation を作る経路 (テスト・一部の continuation) を「全部捨てる」と取り違えないよう、空集合を目印にしない。
- **レビューで確かめてほしい点**: `cache_map` (と `current_color_cache_map`) を、一覧の項目の鍵以外で引く読み手が無いか
  (色フィルタ、リモートのコンテナ一覧、仮想フォルダの seed / writeback、動画ピンの seed、自動縦横比の seed、K2 の流用)。

### 段階 C: 表示を絞っている利用者の古い行の片付け (判断待ち、v5.3.0 には入れない案)

- 段階 B で時間への影響は消えるが、ディスク上の古い行は残る (このフォルダで 33 MB)。
- 案: 表示中の項目について、今の設定の鍵 (`representative_existing_keys` の 2 つ) と**同じ項目・同じ種類の表示方針**で、
  自動切り抜きの印だけが違う行 (旧版・逆の読み方向) を消す。隠した種類のファイルの行と、別の表示方針の行 (`types` 無し) は
  今の意図どおり残す。
- K2 (フォルダの自動代表が子の本の v1 の行を流用する、docs/cover-crop-plan.md) との関係を整理する必要があり、v5.3.0 の締めに
  間に合わせる規模ではない。§1.800 の後段として残す。

## 5. 通常の使い方への影響

- 退行なし (見込み)。表示されるサムネイルは同じ。閉じた直後に固まる 1 フレームが無くなり、空のセルの時間は今のサイドカー待ちと
  同程度。メモリは段階 B で減る (このフォルダで約 33 MB)。
- 永続データ: 形を変えない (読み方だけ)。段階 C を行う場合も消すのは再生成できるサムネイルのキャッシュだけ。

## 6. 検証

- 単体: `CatalogDb::load_keys` (集合内だけ返す、空集合で何も返さない、`audioart:` を除く今の扱いを保つ)。
- 状態遷移: continuation が「サイドカー先・catalog 先・どちらかの途中で取り消し・別フォルダへ移動」のどれでも、合流は 1 回だけ、
  取り消し後に古い結果を採らないこと (request の相関で確かめる)。
- 回帰: 片付けの条件 (`is_all_visible`) を満たす読み込みで、今と同じ行が残ること。合成ビューは経路が変わらないこと。
- perf: `sli_catalog_load_all` を worker 側の区間 (`sli_catalog_preload`、ms・entries・scope) と、合流までの待ち
  (`sli_catalog_join_wait`) に分ける。
- 利用者の実機確認 (開発版 `build-dev.ps1`): 1,000 冊級のフォルダで本を開いて閉じ、perf ログで閉じた直後の大きなフレーム間隔が
  消えたことを確かめてもらう。

## 7. 本件の範囲外で見つかったもの (別項目の候補、いずれもログからの読み・未確認)

1. 閉じた直後のフレームで `keep_fullscreen_viewport` が 56 ms (23.218 s)。閉じた全画面の後始末か描画が UI で重い。
2. 開く側: 最初の全画面フレームが 133 ms。`fs.final_composite_build` が保持していた AI 拡大済みのページ (4800×6916、idx 25)
   の合成に 86 ms (`post_ms` 45・`upload_ms` 40)。開いたページ (idx 0) とは別のページの合成が、最初の描画の前に UI で走っている。
3. `sli_settings_save` が開く・閉じるたびに 6〜9 ms (起動直後は 212 ms)。`last_folder` の保存が UI で同期。
