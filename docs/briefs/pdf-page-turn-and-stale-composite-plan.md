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

### 却下した仮説 (現時点)

- `final_composite_cache` の idx 一致だけの引き方が直接原因、という説。
  引き方も `resolve_fs_display_tex` の順序も v2.13.0 と同一で、さらに
  `start_loading_items_inner` (フォルダ読み込み経路) が
  `clear_all_final_pipeline_caches` を**無条件で呼ぶ**ため、移動時にキャッシュは空になる。
  ただし世代の刻印が無いこと自体は別の穴として残る (要修正かは Codex の回答で判断)

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

## 進め方

1. Codex (gpt-5.6-sol, read-only) に ① の原因判定を依頼中。**結果が出るまで実装しない**
   (一度誤った分析で実装しかけているため、検証を挟む)
2. ① の原因確定後、① と ② のブリーフを Codex へ出して実装
3. `cargo fmt` / `test-full.ps1` / `check_ui_glyphs.py` を通す
4. 検証ビルドを作って実機確認 → OK なら v3.0.0 の配布ビルドを作り直す
