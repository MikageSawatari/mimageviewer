# v3.0.0 出荷前: PDF ページ送りの 2 件 (2026-08-14)

利用者の実機報告 2 件。**どちらも v3.0.0 で直してから出す**方針 (メジャー版なので目立つ
不具合を残さない、という判断)。

---

## ① Ctrl+↓ で前の PDF の絵が出続ける / 枠だけ次のページに変わる / ちらつく

### 症状 (実機、3 つとも同一原因)

フルスクリーンで PDF を開き Ctrl+↓ を連打すると:

1. 前の PDF のページ画像が、次以降の複数の PDF でも表示され続ける
2. 「次のページの縦横比の枠に合わせて画像が 1 度変換されてから、中身が差し替わる」
3. ちらつく

### 再現条件

- **v3.0.0 のみ**。利用者が v2.8.0 / v2.13.0 のポータブル版で試して **再現しない**ことを確認済み
- サムネイルキャッシュがある PDF が並ぶフォルダで Ctrl+↓ 連打 (約 100ms 間隔)

### 実測 (perf log)

テクスチャ `Managed(641)` は `20230103_009.pdf#0` の final composite として t=27.588 に生成され、
その後 **10 個の別 PDF** (items_generation 43〜52) で idx=0 として描画されている。
矩形は新しい PDF のページボックスへ正しく追随するのに、テクスチャだけが古いまま:

| 表示中 | 描画矩形 | 倍率 | テクスチャ寸法 |
| --- | --- | --- | --- |
| `_009` (本物) | 957.3 x 1396.0 | 0.606 | 1580 x 2304 |
| `_010` | 957.3 x 1396.0 | 0.606 | 同一 |
| `_013` | 920.0 x 1341.6 | 0.582 | 同一 |
| `_014` | 950.4 x 1386.0 | 0.602 | 同一 |
| `_018` | 957.3 → 953.6 (**同一世代で 2 回**) | | 同一 |
| `_019` | 943.5 x 1375.8 | 0.597 | 同一 |

`_018` の同一世代 2 回描画がちらつきの実体。

### 原因 (Codex 検証中)

`App::current_final_composite_texture` (src/app.rs 55823) が **idx だけ**で引いている:

```rust
self.final_composite_cache
    .iter()
    .find(|(key, _)| key.edit_key.idx == idx)
    .map(|(_, entry)| entry.texture.clone())
```

`EditResultKey` (src/app.rs 5550) に `items_generation` が無い。
`resolve_fs_display_tex` (src/ui_fullscreen.rs 4272) はこれを **fs_cache やサムネイルより先**に
引くので、仮想フォルダを移っても idx=0 が前の PDF のエントリに当たり、最優先で勝つ。

**未解決の疑問**: `current_final_composite_texture` は v2.13.0 と**バイト単位で同一**。
それでも v2.13.0 で再現しないので、変わったのは「移動時にキャッシュへ何が残っているか」の側。
候補は今サイクルの通過表示 (`2426961f` で v2.13.0 から外し、`96faeee6` 以降で入れ直し) と
PDF ページ形状の変更 (`22e47021` / `7dbb2f39` / `1b5fff92`)。Codex に判定を依頼中。

### 追加調査 (2026-08-14、Codex 待ちの間)

**却下していた holdover 説を撤回する。** 却下の根拠は「`resolve_fs_display_tex` は
`fs_holdover_tex` を読まない」だったが、**描画経路は resolve とは別に holdover を直接読む**
(`fs_nav_holdover_for_draw`)。ログの `texture_choice` が
`branch="fullscreen_overlay" source="nav_holdover"` と明記しており、
**描かれているのは holdover** で確定。

ナビ 1 回分の生ログ (t=27.84〜27.88、`_009` → `_010`):

```
27.849 ready          idx=0 key=pdf::..._010.pdf#0 from_cache=true w=290 h=421
27.850 texture_choice branch=resolve_processed  source=none_after_paint texture_id=null
27.852 texture_choice branch=fullscreen_overlay source=nav_holdover     texture_id=Managed(641)
27.853 paint          source=processed_other    texture_id=Managed(641)
27.853 page_turn_ready mode=materialized source=processed_other texture_id=Managed(641)
```

**新しい PDF の正しいサムネイルは用意できている** (`ready` が `from_cache=true` で出ている)
のに、holdover が解除されず前の絵が描かれている。`resolve_processed` が `null` を返しており、
holdover の解除条件が「processed/final が解決できること」に依存しているために解除されない、
という筋が通る。`resolve_fs_display_tex` は
`colorize_display_requires_final_effect(idx)` が真だと**サムネイルがあっても None を返す**
(src/ui_fullscreen.rs 4291)。カラー化や Creative LUT が効くページでは常にこの経路に入る。

`paint` の `source="processed_other"` は診断分類器 (src/ui_fullscreen.rs 4520-4555) が
erase / fs_cache / thumbnail しか見ないための **fallback ラベル**で、holdover を知らない。
分類器に holdover を足すべき (誤診を誘発した)。

**egui の texture id は再利用されない** (`epaint::TextureManager::alloc` が `next_id += 1`)
ので、`Managed(641)` が同一テクスチャであることは確定。

**v2.13.0 で再現しない説明 (仮説、Codex に確認中)**: `ff9d2eb1` が holdover の variant を
組み替え (`ColorizeDisplayUnit` → `FinalEffectSourceReload`)、
`capture_colorize_page_transition_holdover` を削除している。解除条件の依存先が
変わった可能性がある。

### 確定 (Codex gpt-5.6-sol の検証と合意、2026-08-14)

**供給元は holdover。`final_composite_cache` 説は否定された。**

1. 最初の Ctrl+↓ の直前、`capture_fs_nav_holdover` が `resolve_fs_display_tex` 経由で
   表示中の unit を退避する。この時点では final composite が `Managed(641)` を
   正当に供給する (ui_fullscreen.rs 5063 / 4657)
2. フォルダ移動は fullscreen を閉じ、`edit_result_cache` / `final_composite_cache` /
   `comic_cache` / `fs_cache` を**クリアする** (app.rs 49757 / 52246)。
   よって移動後にキャッシュ側へ古いエントリは残っていない
3. しかし **clone された `TextureHandle` は `FsHoldover::FolderNavigation` が所有し続ける**。
   連打時は同じ handle を保持したまま `fs_nav_locked_gen` だけ押し直す
   (ui_fullscreen.rs 21266 「Keep the existing holdover handle…」)。**これは意図的**
4. 新しい target が ready になるまで `fs_nav_holdover_decision` が同じ unit を返す (4993)
5. 通常ページを描いた上に holdover を重ねる (12724)。これが
   `source="nav_holdover" branch="fullscreen_overlay"`
6. 診断分類器は現在のキャッシュ群としか照合しないので `processed_other` に落ちる (4490)。
   paint は現在の `page.idx` / item key / `items_generation` を使うため (14127)、
   **新しい PDF のキーに古い texture id が並ぶ**

**「自己再退避」の却下は結果的に正しかった** (`resolve_fs_display_tex` は
`fs_holdover_tex` を読まない)。ただし**自己再退避は不要**で、最初の clone が
そのまま生き続けるだけだった。

### v2.13.0 との差 (Codex 判定)

**保持の仕組み自体は v2.13.0 以前から同じ** (`dd2e3c492` が導入、v2.13.0 の祖先)。
変わったのは **holdover の geometry**:

- `FsDisplayUnitHoldoverPage` は「items 入れ替え後に capture 時の `idx` から
  geometry を再導出してはならない」と明記している (app.rs 6506)
- ところが `draw_fs_display_unit_holdover` はその capture 時 `idx` を通常の描画経路へ
  渡している (ui_fullscreen.rs 25614)
- 現在の `draw_fs_image` は `self.items[0]` が**差し替わった新しい PDF ページ**であることを
  見て、その PDF の `fs_page_layout_source_size(0)` を引く (21955 / 14540)

原因コミット: `1b5fff92` (draw_fs_image に PDF の current-item layout 参照を導入) /
`22e47021` (寸法不一致時に page-layout 枠内へ contain するよう変更) /
`7dbb2f39` (PDF page-box レイアウトと raster 座標の分離)。

**これが「古い絵が次の PDF のページ枠に合わせて 1 度変換されてから中身が差し替わる」の実体。**
v2.13.0 では capture 時の `source_size` をそのまま使い、差し替え先の layout を参照しなかったので、
handle は残っても geometry が安定しており目立たなかった。

⚠ Codex の留保: v2.13.0 が「絶対に古い絵を出さない」ことは source からは証明できない
(保持機構は同じ)。利用者の実機で再現しなかった差は、target thumbnail の readiness /
キャッシュ状態 / final-effect の有効設定 / 入力タイミングなど runtime 条件の可能性がある。

なお **Ctrl+↓ は通過表示 (pass-through) の再投入とは無関係**。物理 chord 照合が
修飾キー完全一致を要求するため (keymap.rs 5993)、Ctrl+Down は無修飾 Down の
ページ送り経路を起動しない。

### 却下した仮説

- **`final_composite_cache` の idx 一致だけの引き方が直接原因** — 否定。
  holdover が握る clone は、キャッシュを空にしても解放されない。
  `keep_set_evict` (app.rs 56719) も修理境界ではない。
  ただし**世代の刻印が無いこと自体は latent な穴として残る** (下の「同型の穴」)
- **自己再退避ループ** — 不要だった (最初の clone が生き続けるだけ)

### 同型の穴 (Codex 指摘、今回は latent)

いずれも `close_fullscreen` のクリアに救われているだけで、軽量な items 差し替え経路が
そのクリアを外すと ABA になる:

- `current_edit_result_texture` が `idx` だけで走査し他の `EditResultKey` 世代を無視 (app.rs 53781)
- `current_comic_composite_texture` が `idx` 直引き (コメントに「世代チェックなし」と明記、app.rs 56470)
- `conceal_cache` は `idx` + global conceal 世代のみで `items_generation` を見ない (ui_fullscreen.rs 4297)
- `EditResultKey` / `FinalCompositeKey` に item-context 世代が無い (app.rs 5550 / 6032)

**意図された正しい形は `ContinuousPageTransition`** で、これは `items_generation` を
自分で持っている (app.rs 6434)。

### 修正方針 (Codex の回答で確定させる)

`final_composite_cache` のエントリに `items_generation` を持たせ、
`current_final_composite_texture` / `final_composite_texture_is_complete` /
`current_final_composite_is_complete` の 3 か所で照合する。
**同型 (idx だけで引いて世代を見ない) の箇所を他にも洗い出す**こと。

### bisect について

**実施しない**。知りたかったのは「リリース済み版にも patch が要るか」で、v2.8.0 / v2.13.0 で
再現しない = **影響は v3.0.0 のみ**と確定済み。修正内容も由来に依存しない。

---

## ② キャッシュの無い PDF で、黒背景にパスだけの画面が一瞬出る

### 症状

サムネイルキャッシュの無い PDF を開いてページ送りキーを押しっぱなしにすると、
真っ黒な背景に PDF のファイルパスだけが出る画面が一瞬混ざる。
再現ファイル: `h:\home\mimageviewer_old\testimage\pdftest\20230103_019.pdf`

### 実測

`page_turn_decision` の `reason` 内訳 (同一セッション):

| 件数 | reason |
| --- | --- |
| 5504 | `no_page_transition` |
| 4139 | `pending_zero` |
| 3951 | `pass_through` |
| **352** | **`passthrough_rendition_unavailable`** |

### 原因

`location_display_for_loading` (src/ui_fullscreen.rs 13731) は
**`fs_cache[idx]` にも `thumbnails[idx]` にもテクスチャが無いとき**にパスを返し、
それが読込中プレースホルダ (黒背景 + 中央にパス) になる。

通過表示は「その場でサムネイルを作れば成立する」前提だが、これは**画像での実測** (1 枚 33ms)。
PDF ページは PDFium レンダで 1 枚 300ms 前後かかり、キーリピート (34ms) には追いつかない。
§1.58 の記述 (`docs/next-release-backlog.md`) がこの前提を画像基準で書いている。

### 仕様 (利用者判断 2026-08-14、確定)

**PDF でも通過するページのサムネイル画質の絵を実際に作って表示する。
1 ページあたり 300ms かかるのは許容する。**

- 画像側の「通過するページをすべて表示する」と同じ原則にそろえる
- 素材が間に合わないあいだに届いた**キーリピートは溜めずに捨てる**
  (溜めるとキーを離した後もページが進み続ける)
- 結果として、押しっぱなしでは **約 300ms ごとに 1 ページ**、確実に絵が出る
- 黒背景 + パスのプレースホルダは、本当に何も無いとき (初回オープン) だけに限定する

**この仕様を選ぶ理由** (利用者の言葉):

- 高速に先のページを見たいときは**シークバー**がある。キー押しっぱなしは
  「なんとなく内容を見ながら進む」操作として扱う
- サムネイルは一度作れば次回から速いので、大抵の場合は問題にならない

### 却下

「素材が無いときは直前のページの絵を保持する」案。利用者判断で不採用。
通過したページを見せないまま進むことになり、画像側の原則と食い違うため。

---

## 修正方針 (Codex と合意、2026-08-14)

①と②は**同じ 1 つの仕組みへ収束する**。「解決していない移動先がある間、何を見せるか」を
型で持ち、通過した対象を必ず提示する。

### Part A — holdover を capture 時の geometry だけで描く (①の見た目の直接原因)

- `FsDisplayUnitHoldoverPage` に **canonical layout geometry を capture 時に持たせる**
  (rotation / source size / content bbox と並べる)
- 単ページ・見開きとも、holdover の描画は**この capture 済み geometry だけ**から行う
- holdover 描画中に **現在の `self.items` / thumbnails / page-layout メタを一切参照しない**
- 画面矩形ではなく **canonical layout size** を持つこと (holdover 表示中に viewport が
  正当にリサイズされ得るため)
- 診断分類器 (ui_fullscreen.rs 4490) に **holdover を追加**する。
  現状 erase / fs_cache / thumbnail しか見ず、holdover を `processed_other` と誤表示して
  調査を誤らせた

Part A だけで「枠に合わせて変形してから中身が入れ替わる」「ちらつく」は止まる。
**ただし中間の PDF が表示されないことは直らない。**

#### Part A 実装記録 (2026-08-14)

- `FsDisplayUnitHoldoverPage` に capture 時の `layout_size` を追加。PDF はそのページの page box、
  通常画像は source size（いずれも無い旧 fallback だけ texture size）を保存する。
- 単ページは `draw_fs_image`、見開きは spread geometry と `draw_fs_spread_page` へ
  `FsPageLayoutSource::Captured { .. }` を渡す。holdover 分岐では replacement items の
  `items` / `thumbnails` / page-layout metadata を idx から参照しない。共有 resampler 経路も
  capture 済み post-filter と診断 identity / load sequence を受け取り、replacement item の
  `effective_params` / perf key / `fs_cache` を再解決しない。
- `fs_texture_source_for_trace` は holdover が所有する texture id を cache 分類より先に判定し、
  `processed_other` ではなく `holdover` と記録する。
- 単ページ / 見開きそれぞれで、同 idx の replacement PDF が異なる page box を持っても
  capture 時の縦横比で描く回帰テストと、classifier の回帰テストを追加した。
- retention policy、受理 target の逐次提示、key repeat の drop は変更していない。これらは
  引き続き Part B の範囲とする。`final_composite_cache` / key の世代 hardening も未変更。

### Part B — 受理した移動先を必ず提示し、捌けないキーリピートは捨てる (①の残りと②)

利用者判断 (2026-08-14): **通過するページ / 移動先は、サムネイル画質でよいので必ず実際に
表示する。1 ページ 300ms かかるならそれは許容する。**

- **型付きの未解決 page-turn / nav unit (sequence)** を持つ:
  - 移動先の rendition が ready になるまで、直前の unit を**原子的に**保持する
  - **受理した対象は必ず 1 度提示してから**次を受理する
  - 提示待ちの間に届いたキーリピートは**溜めずに捨てる** (溜めるとキーを離した後も進み続ける)
  - キーアップで landing の rendition / final image へ settle する
- 黒背景 + パスのプレースホルダは、本当に何も無いとき (初回オープン) だけに限定する

#### Part B 実装記録 (2026-08-14、未コミット)

- `App::fs_holdover_tex` の typed variant として `FsHoldover::NavigationSequence` を追加した。
  `FsNavigationSequence` は captured previous display unit と
  `FsNavigationSequenceTarget::{FolderItems, Display}` を一体で所有し、target phase は
  `Awaiting { accept_rendition }` → `Ready(Rendition|Materialized|Failure)` →
  `Presenting(...)` と遷移する。thumbnail/PDFium が terminal failure の場合は
  `RenditionFailed` として次 repeat を許可し、key-up 後は previous unit を保持したまま full
  materialization の成功/失敗へ settle する。
- main viewer の手動 Ctrl+↑↓ / Ctrl+PageUp/Down、snapshot、Ctrl+G drill、smart/favsearch、
  nested ZIP、およびページ送りを sequence の受理 gate に通した。detached と slideshow の
  legacy `FolderNavigation` owner は変更していない。未解決中の repeat と同一 frame の追加 edge は
  accumulator へ入れず drop する。グリッド側の folder-nav burst は従来どおり queue する。
- target page set を既存 thumbnail `keep_set` の一時 anchor に加え、priority request を発行する。
  PDF page は `pdf_loader::promote_to_high_normal` へ明示的に渡すため、catalog thumbnail / pixels が
  無い場合も PDFium source render が実際に開始される。item-context generation は追加していない。
- `page_turn_decision_for_inputs` は rendition 未着時に `PassThrough` を選ばなくなった。
  `defer_ui_uploads=true` のまま `Materialized` を選び、その下を previous atomic unit が覆う。
  rendition ready frame だけ `PassThrough` にし、描画末尾で target generation / complete page set /
  non-holdover texture identity を確認して初めて sequence を解放する。
- key-up は burst 画質 owner を `Idle` に戻すが unresolved sequence は消さない。landing rendition を
  1 frame 提示した後の次 frame で final work を再開する。repeat queue が無いため、key-up 後に
  navigation が追加で進む producer は存在しない。
- 回帰テストを 4 件追加し、既存の page-turn decision / same-frame Ctrl+Down の期待値を新仕様へ
  更新した。`mimageviewer` lib は 5,671 tests。`scripts/test-full.ps1` PASS、
  `cargo fmt --check` clean、`scripts/check_ui_glyphs.py` 0、`scripts/build-dev.ps1` 成功。

**なぜ「直前の絵を保持するだけ」では駄目か** (Codex 指摘): 移動だけ先へ進みながら古い絵を
出し続けると、飛ばしたページを見せないことになり **R1 に反する**
(docs/display-pipeline.md 1524)。保持自体は spec が要求する正しい fallback だが
(同 1613 / 1828)、「提示せずに進む」ことが違反。

### ②の経路の訂正 (Codex 指摘)

- 単ページ表示のパス文字列は `prepare_fullscreen_state` 内の **`location_display_for`**
  から来る (ui_fullscreen.rs 14383)。`location_display_for_loading` は見開きと
  連結読みの経路 (25731 ほか)
- `page_turn_decision_for_inputs` は burst 中、`passthrough_rendition_ready == false`
  でも **PassThrough を選ぶ** (7427)。**この挙動を要求する unit test が 32279 にある**
  ので、変更時はテストごと直す
- `prepare_fullscreen_state` が full texture を `None` に落とし (14317)、
  `ensure_passthrough_rendition` は catalog thumbnail と `thumb_pixels[idx]` の
  **両方**が無いと `None` を返す (app.rs 60740)。両方欠けると 22063 の
  「読込中...」+ パスへ到達する
- 352 件の `passthrough_rendition_unavailable` は「rendition が長く不在だった」ことは
  示すが、**352 回のレンダ失敗を意味しない** (frame / display-unit page ごとに出る)。
  PDFium の遅さは有力だが、queueing / cancel / `thumb_pixels` 欠落も候補

### やらないこと

- `final_composite_cache` への `items_generation` 追加は**この不具合の修正にはならない**。
  hardening としては有効だが、やるなら key / read / insert のライフサイクル全体に通す必要があり
  (app.rs 55292 / 55640 にも exact-key ヒットがある)、**リリース直前にやる範囲ではない**。
  「同型の穴」としてバックログへ送る
- bisect は実施しない (影響は v3.0.0 のみと利用者の実機確認で確定済み)

## 進め方

1. ~~Codex に原因判定を依頼~~ **完了。上記で合意**
2. Part A → Part B の順で実装 (A は範囲が小さく単独で価値がある)
3. `cargo fmt` / `test-full.ps1` / `check_ui_glyphs.py` を通す
4. 検証ビルドを作って実機確認 → OK なら v3.0.0 の配布ビルドを作り直す
