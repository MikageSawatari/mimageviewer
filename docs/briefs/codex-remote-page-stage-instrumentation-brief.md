# ページ供給の内訳を割る (計装だけ。挙動は変えない)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

- **1 件 = 1 コミット** (2 コミット)。
- `docs/briefs/HANDOFF.md` と他の brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。
- **今回は計装のみ。速くする変更・順序を変える変更・キャッシュを止める変更を入れない。**
  遅い場所が見つかっても直さない (打ち手は測ってから決める)。

---

## 何が分かっていて、何が分かっていないか

リモートのページ 1 枚 (46.5 MP、高画質 8192) は中央値 1784ms かかる。同一ログでの内訳:

| 部分 | ms | 出所 |
|---|---|---|
| PDFium の描画 | 507 | `pdf/pool_recv` の `worker_render_ms` |
| **`source` 段のうち、pool が返した後** | **458** | **計装が無い。差分でしか見えていない** |
| **`compose` 段** | **284** | **`remote_page/stage` の 1 本しか無く、中身が見えない** |
| JPEG エンコード | 155 | `remote_page/stage` の `jpeg` |
| ワーカーの RGBA 組立 | 100 | `pdf/pool_recv` の `worker_serialize_ms` |
| パイプ転送 | 138 | `worker_write_ms` + `parent_read_ms` |
| 縮小 | 64 | `remote_page/stage` の `resize` |

**太字の 2 つに名前を付けるのがこの作業**。合わせて 742ms / 42% を占めるのに、
どちらも「1 本の区間」としてしか観測できていない。

### 分かっている構造 (調査済み。前提にしてよい)

`ContainerService::load_page_pixels...` ([container.rs](../../src/remote_ipc/container.rs) の
`RemotePageStage::Source` を enter している箇所、4264 行あたり) は:

```rust
crate::thumb_loader::process_load_request(...);   // ← 同期呼び出し
drop(tx);
let (color_image, _) = rx.into_iter().find_map(...)?;   // ← 戻ってきた後で drain
```

`tx` は非同期の `mpsc::channel` なので、`process_load_request` は表示用 ColorImage を
送った**後も戻ってこない**。つまり `load_one_cached` の後半 —
**サムネイル WebP のエンコード (46 MP からの縮小を含む) と catalog への SQLite 書き込み** —
も `source` 段の中で起きている ([thumb_loader.rs](../../src/thumb_loader.rs) の
`should_save` ブロック)。`skip_cache: true` は cache **読み**を飛ばすだけで、
`should_save` は見ていないため、フルページ要求でも保存側は走る。

**これは 458ms の有力候補だが、決めつけない。**名前を付けた区間の合計と実測の差を
`unaccounted_ms` として必ず残すこと。予想が外れたときに外れたと分かる形にする。

---

## コミット 1: `remote_page/stage` に内訳を持たせ、`source` と `compose` を割る

すべて [src/remote_ipc/container.rs](../../src/remote_ipc/container.rs) 内。

### 1-a. 段の中に「区間」を持てるようにする

`RemotePageStageGuard` に区間を足す:

```rust
fn add_phase(&mut self, name: &'static str, ms: f64);
fn phase_from(&mut self, name: &'static str, started: Instant);  // 便利版
```

`Drop` で既存の `extras` に 2 つ足す:

- `phases`: `{"名前": ms, ...}` の object (空なら出さない)
- `unaccounted_ms`: 段の実測 ms − 区間の合計。**負にしない** (0 で下げ止める)

区間名の合成と `unaccounted_ms` の計算は**純関数へ切り出してテストする**:

```rust
fn remote_page_phase_summary(stage_ms: f64, phases: &[(&'static str, f64)])
    -> (Option<serde_json::Value>, f64);
```

- 空の入力 → `(None, stage_ms)`
- 同じ名前が 2 回来たら**足し合わせる** (呼び出し側でループする区間があるため)
- 合計が段の実測を超えたら `unaccounted_ms` は 0 (計測誤差で負値を出さない)

### 1-b. `source` を割る

`Source` の guard へ:

- `load_request_ms` — `process_load_request` の呼び出しそのもの
- `drain_ms` — `rx.into_iter().find_map(...)` の所要

上の構造どおりなら `drain_ms` はほぼ 0 になるはずで、そうなれば
「`source` の中身 = `process_load_request` の中身」が**確かめられた事実**になる。
そうならなければ前提が違うので、それも分かる。

### 1-c. `compose` を割る

`Compose` の guard へ、以下をそれぞれ計る。**早期 return の経路でも記録が残るように**
(`cached_composite_pixels` を使う分岐、`prepared_composite` が無い分岐):

| 区間名 | 何を計るか |
|---|---|
| `edit_space` | `analyze_page_content_type` (PDF の正準ラスタ寸法の解決) |
| `auto_trim` | `crate::margin_fit::detect_content_bbox` |
| `edits` | `execute_remote_edits` (いま `elapsed_ms` をテキストログに出しているもの) |
| `lut` | `resolve_remote_lut_timed` |
| `composite` | `execute_remote_composite` |
| `comic` | `comic_composite` |
| `crop` | `crop_with_stored_edit_space` |
| `cache_insert` | `page_composite_cache` への `insert` |
| `to_rgba` | `loaded_image_from_color_image` (= `color_image_to_rgba` の全画素コピー) |

`to_rgba` は複数の return 地点から呼ばれる。**呼び出しごとに記録する** (呼び出し側で
`Instant` を取って `phase_from` する形でよい。ヘルパを 1 つ作って 3 箇所から呼ぶのが素直)。

注記: `wait_ms` (mutex 待ち) は既に別フィールドで出ているので、区間と二重に数えないこと。
`lock_with_remote_page_wait` に渡している guard の待ち時間はそのまま `wait_ms` に残す。

### 検証

- `remote_page_phase_summary` の純関数テスト (上の 3 性質)
- 既存の `remote_page/stage` のテストがあれば壊れていないこと

---

## コミット 2: `process_load_request` の中身を割る

[src/thumb_loader.rs](../../src/thumb_loader.rs) の `load_one_cached` の末尾で、
perf イベントを 1 本出す。

```
cat="thumb", kind="load_phases"
```

### key は既存の空間に揃える (これが肝)

- `pdf_page` が Some → `crate::grid_item::pdf_page_perf_key(path, page_num)`
- それ以外 → ZIP エントリなら `path::entry_name`、通常ファイルならパス

**PDF の場合、これは `pdf/pool_recv` の key とも `remote_page/stage` の key とも同一になる**
(`remote_page_perf_key` が同じ helper を使っている)。3 つのイベントを key で join できる形に
することが目的なので、ここを勝手に変えない。

### 区間

既に局所変数として計っているもの (`decode_ms` / `display_ms` / `encode_ms`) を再利用し、
足りないところを足す。**合計と実測の差を `unaccounted_ms` として出す**のは段と同じ。

| extras | 何を計るか |
|---|---|
| `total_ms` | `load_one_cached` の全体 |
| `decode_ms` | 既存 (PDF では `render_page` の往復を含む) |
| `render_ms` | **新規**: `pdf_loader::render_page` の呼び出しそのもの (PDF 以外は出さない) |
| `orientation_ms` | **新規**: `apply_orientation` |
| `display_ms` | 既存 (`resize_to_display_color_image` + pinned adjustment) |
| `send_display_ms` | **新規**: 表示用 `tx.send` |
| `cache_encode_ms` | 既存の `encode_ms` (46 MP からの縮小 + WebP) |
| `cache_save_ms` | **新規**: `save_with_layout_dims` (SQLite 書き込み) |
| `cache_map_ms` | **新規**: `cache_map` への write lock + insert |
| `unaccounted_ms` | `total_ms` − 上の合計 (負にしない) |
| `should_save` | bool。保存側が走ったのかどうか |
| `skip_cache` | bool。呼び出し元の要求 (フルページ要求では true) |
| `pdf_page` / `idx` / `input_seq` | 突き合わせ用 |

`should_save` が false の経路では保存側の区間は 0 になる (出さなくてもよいが、
出すなら 0 で揃える)。

### 制約

- **この関数は本体のサムネイル生成でも走る。**イベント発火は `crate::perf::is_enabled()` で
  必ずガードする。`Instant::now()` の追加は既存のスタイル (`t` / `t_display` / `t_enc` が
  無条件) に合わせてよい。
- **1 回の呼び出しにつきイベントは 1 本**。早期 return (decode 失敗 / cancel / STALE) では
  出さなくてよい (出すなら `outcome` を付けて区別できるようにする)。
- 既存のテキストログ行 (`decode=... display=... encode=...`) は**消さない**。
  実機で `mimageviewer.log` を見る運用がある。

### 検証

- 区間の合計と `unaccounted_ms` を作る部分を純関数へ切り出してテストする
  (コミット 1 と同じ性質で構わない。共有できるなら共有してよい)。

---

## 実行するテスト

```
cargo test -p mimageviewer --lib remote_ipc
cargo test -p mimageviewer --lib thumb_loader
cargo fmt --all -- --check
```

## 報告してほしいこと

- 付けた区間の一覧と、それぞれが**何から何まで**を含むか (境界を言葉で)。
- `source` の中で名前を付けられなかった部分が残るか。残るなら、どのコードがそこに居るか。
- `compose` の区間のうち、**補正も注釈も無いページで実際に走るのはどれか** (コードから見て)。
- ブリーフと意図的に違えた点があれば、その理由。
