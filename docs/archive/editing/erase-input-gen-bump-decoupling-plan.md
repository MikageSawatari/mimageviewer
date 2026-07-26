# input_gen bump と post_filter / slider drag の分離 設計ブリーフ

**対象**: 消しゴム Apply 直後の MI-GAN 二度走り + Slider drag 中の MI-GAN thrash
**前提**: Phase 6/7/8 完了 (Ctrl+E)、Phase 1-5 code-review 修正完了 (`058762d5` / `4b93670b`)
**進め方**: Codex GUI で実装 → ClaudeCode で手作業レビュー
**Status**: 仕様詰めから着手するため、まずは本ブリーフをユーザーが読んで方針承認してから実装に移る

ClaudeCode の recall モード code-review (Phase 1-5) で **CONFIRMED** されたが、
小手先の修正では直せないため意図的に持ち越した 2 件の設計問題を、まとめて構造的に
解消する。最終結果 (= 表示される画像) は現状でも正しく、これは **perf / UX 改善**。
ただし MI-GAN は 1 回 300-500ms、Blur 4K で数秒かかるので体感はある。

---

## 0. 着手前に必ず読む

- [CLAUDE.md](../../../CLAUDE.md) §「コード修正時のドキュメント同時更新」
  「並行処理: try_lock + sleep は使わない」「UI スレッドでの同期 I/O は即 worker 化する」
- [docs/preset-and-adjustment.md](../../preset-and-adjustment.md) §4 (キャッシュ階層) と
  §5 (消しゴム機能)、§9 (キャッシュ無効化テーブル)
- [docs/display-pipeline.md](../../display-pipeline.md) §1〜§3 (表示パイプラインの優先順位)
- [docs/archive/editing/erase-cache-refactor-plan.md](erase-cache-refactor-plan.md) (= Step 2 ブリーフ、
  本ブリーフはその続編)

---

## 1. 症状 (実機 perf)

### 症状 A: Apply 直後の MI-GAN 二度走り (CONFIRMED)

消しゴムで mask を描いて Apply (E キー 2 連打) すると、**MI-GAN が 2 回走る**。
1 回目は途中でキャンセルされ、その結果は捨てられる。

### 症状 B: Slider drag 中の MI-GAN thrash (CONFIRMED)

保存済み消しゴム mask があるページで補正スライダーをドラッグすると、ドラッグ中
**毎 tick で MI-GAN が起動 → 即キャンセル**を繰り返す。完了するのはドラッグ終了後
の最後の 1 回だけ。

両者の根本原因は同じ:

---

## 2. 根本原因

### 2.1 `clear_adjustment_caches(idx)` のカスケード

[`src/app.rs`](../src/app.rs) の `clear_adjustment_caches(idx)` (line 18048):

```rust
pub(crate) fn clear_adjustment_caches(&mut self, idx: usize) {
    self.adjustment_cache.remove(&idx);
    self.invalidate_compare_prepared_for_idx(idx);
    self.thumb_adjust_tex.remove(&idx);
    self.bump_adjustment_generation(idx);     // ← ここが起点
    self.clear_conceal_caches(idx);
    self.erase_base_tex_cache.remove(&idx);
}
```

`bump_adjustment_generation` は `bump_input_generation` を呼び、その中で:

```rust
pub(crate) fn bump_input_generation(&mut self, idx: usize) {
    let slot = self.input_generation.entry(idx).or_insert(0);
    *slot = slot.wrapping_add(1);
    self.cancel_erase_commit_pending_for_idx(idx);   // ← 進行中の Commit をキャンセル
    self.clear_erase_result_caches_for_idx(idx);     // ← 結果も破棄
    self.clear_erase_preview(idx);
    self.clear_conceal_caches(idx);
}
```

これは `erase_result_cache` のキーが `(idx, input_gen, mask_gen)` で構成されている
ことに由来する設計 (Step 2 リファクタの根幹)。入力ピクセルが変わったら、cache の
key を bump して以前の結果を見えなくする — その仕組み自体は正しい。

問題は **「post_filter のオン/オフ切替」と「色調パラメータの一時変更 (drag 中)」が
input_gen の bump として扱われてしまう**点。

### 2.2 症状 A の経路

```
[Apply 操作]
1. execute_erase_inpaint(ctx, idx)
2. └ apply_inpaint_only:
     - save_mask_with_sidecar (DB 書き込み + bump_erase_mask_generation)
     - run_inpaint_and_cache(..., is_preview=false)
       → erase_inpaint_pending に Commit ジョブ J1 を挿入
       → worker thread に MI-GAN を spawn (input_gen=A, mask_gen=B 時点)
3. └ reset_erase_mode:
     - post_filter_bypassed = false に戻す
     - clear_adjustment_caches(idx)
       → bump_adjustment_generation
       → bump_input_generation: input_gen A → A+1
         → cancel_erase_commit_pending_for_idx: **J1 をキャンセル**
         → clear_erase_result_caches_for_idx: cache 破棄
4. [次フレーム以降]
   - ensure_erase_result_texture(idx):
     - mask_pages.contains(idx) = true
     - current_erase_result_pixels(idx) = None (cache 破棄済み)
     - commit_pending_matches_current_erase_key = false (J1 はキャンセル済み)
     - resolve_erase_input_pixels(idx) を再取得 → adjustment_cache 再生成
       (今度は post_filter 適用込み)
     - run_inpaint_and_cache(..., is_preview=false) で **J2 を spawn**
5. J2 完了 → erase_result_cache に格納 → 画面に反映
```

ユーザー視点: Apply → ~500-1000ms の遅延 (J1 と J2 合計分) → 結果表示。
本来は J1 だけで済むはず。

### 2.3 症状 B の経路

[`src/ui_adjustment_panel.rs`](../src/ui_adjustment_panel.rs) の slider 変更ハンドラ
で、各 tick で `clear_adjustment_caches(fs_idx)` が呼ばれる (drag 中もキャッシュを
クリアして即時プレビューを出すため)。

```
[Slider tick N]
1. slider 値変更検知
2. clear_adjustment_caches(idx)
   → bump_input_generation: input_gen X → X+1
     → cancel J_in_flight (もしあれば)
     → clear erase_result_cache for idx
3. 表示 (drag 中): adjustment_cache が空 → maybe_apply_adjustment が同期合成
   → adjustment_cache に格納
4. ensure_erase_result_texture(idx):
   - mask_pages.contains(idx) = true (保存済み mask あり)
   - cache miss → 新 J_new を spawn (input_gen=X+1 で keyed)
5. [Slider tick N+1] (= 数 ms 〜 数十 ms 後、ユーザー drag 継続中)
   - clear_adjustment_caches → cancel J_new → cache 破棄
   - → 新 J_new2 を spawn
   - ...
```

ユーザー視点: drag 中は mask 領域に旧 cache 残骸 or 元画像が見える (J が完了しない
ので)。drag を止めると ~500-1000ms 後に最終 J が完了して mask が反映される。

GPU/CPU は drag 中ずっと無駄に MI-GAN を spawn → cancel を繰り返している。

---

## 3. なぜ「post_filter / drag 中だけ」は bump 不要なのか

`bump_input_generation` の意図は **「erase_result_cache に積まれている結果が
無効になった」** ことを表現すること。「無効」とは:

- MI-GAN が見るべき**入力ピクセル**が変わった (= adjustment_cache や ai_upscale_cache
  の中身が変わった)
- → 同じマスクで MI-GAN を再実行すると違う結果になる

しかし以下のケースでは **入力ピクセルは変わらない** (= MI-GAN を再実行する必要なし):

### Case 1: post_filter 切替 (= 症状 A)

`erase_result_cache` に積まれているのは **MI-GAN の出力**。MI-GAN の入力 (=
`resolve_erase_input_pixels` の結果) は `adjustment_cache > ai_upscale_cache >
fs_cache` の優先順位で決まる。

`post_filter_bypassed` フラグは erase モード中だけ true で、その間
`adjustment_cache` は post_filter を適用しないバージョンが入っている。Apply 時に
`post_filter_bypassed = false` に戻すと、`clear_adjustment_caches` で
`adjustment_cache.remove` され、次回 `maybe_apply_adjustment` で post_filter 込みの
バージョンが再生成される。

ここで重要なのは: **MI-GAN が見るべき「色調」は post_filter とは独立** で、
post_filter は画面表示時の最終効果 (= CRT / 減色など)。MI-GAN の入力としては
post_filter 抜きの adjustment_cache 値で正しい。Apply 後に post_filter が掛かるか
否かは、**display パイプラインの一番上**で適用される話。

つまり Apply 時:
- J1 が見た入力 = adjustment_cache (post_filter 抜き) のピクセル
- J2 が見るべき入力 = 同じピクセル
- J1 の結果 = J2 の結果と同じ ✅

J1 は無駄に捨てられている。

### Case 2: Slider drag 中 (= 症状 B)

drag 中の slider 値は **暫定値** で、最終的にユーザーがリリースしたときの値だけが
重要。drag 中の中間値で MI-GAN を回しても、リリース時に最終値で再計算されるので
意味がない。

つまり drag 中:
- 中間 J_new は drag 終了時に再度キャンセルされて、J_final が走る
- 中間 J_new の結果は誰も使わない

### Case 3: 入力ピクセルが本当に変わるケース (bump が必要)

逆に、以下のケースでは bump が**必要**:
- AI upscale 完了 → ai_upscale_cache に新エントリ → MI-GAN 入力が変わる
- 補正 favorite / preset の **drag 中ではない確定操作** → MI-GAN 入力が変わる
- 別ページに移動 → cache miss なので bump 不要だが、erase_result_cache の key は
  そもそも idx ベースなので影響なし

---

## 4. 修正方針

### 4.1 設計方針: input_gen bump を「ピクセル変化の確定」に限定

`bump_input_generation` の呼び出し条件を、**erase_result_cache の中身を捨てたい
本当のケース** に絞る。具体的には:

| 経路 | 現状 bump? | 改修後 |
|---|---|---|
| AI upscale 完了 (`ai_upscale_cache.insert`) | yes | **yes** (維持) |
| 補正パラメータ確定 (slider drag 終了 / 数値入力) | yes | **yes** (維持) |
| 補正 favorite / preset 適用 | yes | **yes** (維持) |
| **post_filter_bypassed 切替** (erase mode 入退場) | yes | **no** (改修) |
| **Slider drag 中の中間 tick** | yes | **no** (改修) |
| Folder 切替 / `fs_cache.insert` | yes | **yes** (維持) |
| 回転 / ズーム | no (現状) | **no** (維持、影響なし) |

### 4.2 実装案 A: `clear_adjustment_caches_for_post_filter_toggle` を新設

post_filter 切替専用のヘルパーを作り、adjustment_cache を removeするが
**bump_input_generation を呼ばない**:

```rust
/// post_filter のみが変わるとき用 (= erase / conceal モード入退場)。
/// adjustment_cache は再生成する必要があるが、MI-GAN 入力としての pixel は
/// 同値なので erase_result_cache / commit pending を保持する。
pub(crate) fn clear_adjustment_caches_post_filter_only(&mut self, idx: usize) {
    self.adjustment_cache.remove(&idx);
    self.thumb_adjust_tex.remove(&idx);
    // post_filter は conceal 合成の上位レイヤなので、conceal_cache は触る必要なし
    // (= conceal も同じ adjustment_cache を入力にするが、post_filter 抜きの値で
    // 合成された結果は post_filter 込み display 時に上から適用される)。
    // erase_base_tex_cache は erase mode 中だけの texture なので、退場時に clear
    // するが、ここは bump 経由ではなく直接 remove する。
    self.erase_base_tex_cache.remove(&idx);
    // adjustment_generation の bump はせず、erase_result_cache / commit pending /
    // erase_preview_cache を温存する。
}
```

呼び出し元:

```rust
// src/ui_erase.rs::reset_erase_mode
if self.post_filter_bypassed && !self.analysis_mode {
    self.post_filter_bypassed = false;
    if let Some(idx) = restore_idx {
        // 旧: self.clear_adjustment_caches(idx);
        self.clear_adjustment_caches_post_filter_only(idx);
    }
}
```

同様に `enter_erase_mode` / `enter_conceal_mode` 側の `post_filter_bypassed = true`
切替も同じヘルパーへ。

利点: 最小変更。既存の `clear_adjustment_caches` は wire 通り「真にピクセルが変わる」
ケースで使い続ける。

欠点: 「post_filter 経路だけ別ヘルパー」は将来的に経路追加で漏れやすい。例えば
新しい post_filter モードを追加するときに、誤って `clear_adjustment_caches` を
呼ぶと regression する。

### 4.3 実装案 B: `bump_input_generation` に `reason` enum を導入

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) enum InputGenBumpReason {
    /// MI-GAN 入力に使うピクセルが本当に変わる (AI 完了 / 補正確定 / 回転 / fs_cache 入替)
    PixelChanged,
    /// adjustment_cache の post-filter / drag transient のみで、MI-GAN 入力は変わらない
    DisplayOnly,
}

pub(crate) fn bump_input_generation(&mut self, idx: usize, reason: InputGenBumpReason) {
    let slot = self.input_generation.entry(idx).or_insert(0);
    *slot = slot.wrapping_add(1);
    match reason {
        InputGenBumpReason::PixelChanged => {
            self.cancel_erase_commit_pending_for_idx(idx);
            self.clear_erase_result_caches_for_idx(idx);
            self.clear_erase_preview(idx);
            self.clear_conceal_caches(idx);
        }
        InputGenBumpReason::DisplayOnly => {
            // erase_result_cache / commit pending は触らない
            // (= MI-GAN 入力は変わらないので結果は引き続き有効)。
            self.clear_conceal_caches(idx);
        }
    }
}
```

→ 全ての呼び出し元で `reason` を明示する必要があり、grep しやすい。

利点: タイプセーフ。新しい呼び出し元を追加するときに reason を選ぶよう強制される。

欠点: 大きい diff (呼び出し元 7 箇所 +テスト)。

### 4.4 推奨

**案 A (`clear_adjustment_caches_post_filter_only` 新設) を採用**。
理由:

- 影響範囲が局所的 (post_filter 経路の 2-3 箇所のみ修正)
- 既存テストへの影響なし
- 将来 reason ベースが必要になったら案 B へ段階移行可能

ただし**ガード**を入れる:
- 新ヘルパー内に `debug_assert!` で「post_filter_bypassed が変わるときだけ呼ばれる」
  ことを保証

### 4.5 Slider drag 中の thrash (症状 B) への対処

症状 A の修正だけでは drag 中の thrash は治らない (= drag 中は input_gen が
PixelChanged 経路で bump され続ける)。

#### 案 B-1: `adjustment_dragging` 中は `bump_input_generation` を遅延

[`src/ui_adjustment_panel.rs`](../src/ui_adjustment_panel.rs) には既に
`adjustment_dragging` フラグがある。

```rust
pub(crate) fn bump_input_generation(&mut self, idx: usize) {
    let slot = self.input_generation.entry(idx).or_insert(0);
    *slot = slot.wrapping_add(1);
    // adjustment_dragging 中は cancel/clear をスキップして、drag 終了時にまとめて
    // 1 回だけ反映する。display はその間 adjustment_cache の中間値を見るので
    // mask 領域は古い erase_result_cache (= 1 つ前の状態) で塗られたまま。
    // ユーザー体感としては「drag 終了 → 一拍置いて mask 反映」になる。
    if self.adjustment_dragging {
        return;
    }
    self.cancel_erase_commit_pending_for_idx(idx);
    self.clear_erase_result_caches_for_idx(idx);
    self.clear_erase_preview(idx);
    self.clear_conceal_caches(idx);
}
```

ただし drag 終了時に必ず 1 回 bump を強制する必要がある:

```rust
// src/ui_adjustment_panel.rs の drag 終了ハンドラ
fn on_adjustment_drag_end(&mut self, fs_idx: usize) {
    self.adjustment_dragging = false;
    // drag 中スキップした分の bump をここでまとめて 1 回実行
    self.bump_input_generation(fs_idx);
}
```

利点: 最小変更。

欠点: `bump_input_generation` 自体に挙動分岐が入り、呼び出し元の意図が読みにくい。

#### 案 B-2: drag 中は専用の「diff キャッシュ」を使う

drag 中だけ別の `adjustment_drag_cache` を用意し、`adjustment_cache` 本体は触らない。
drag 終了時に `adjustment_drag_cache` の最終値を `adjustment_cache` にコピーして
bump。

利点: 構造的に綺麗。drag 中は明示的に "transient" な cache を見る。

欠点: 大きい改修 (cache 層 1 つ追加)。

#### 推奨

**案 B-1 を採用**。理由は案 A と同じ (= 影響局所、後で B-2 化可能)。

ただし `adjustment_dragging` 専用判定だと「drag 以外の連続変更」(= 1〜4 キー連打で
プリセット切替、など) には効かない。これはユーザー UX 上は drag より低頻度なので
許容する。

---

## 5. 実装ステップ

### Step 1: 案 A を実装

1. [`src/app.rs`](../src/app.rs) に `clear_adjustment_caches_post_filter_only` を新設
2. [`src/ui_erase.rs`](../src/ui_erase.rs) の `enter_erase_mode` / `reset_erase_mode` で
   post_filter 切替の clear を新ヘルパーに置換
3. [`src/ui_conceal.rs`](../src/ui_conceal.rs) も同様に置換 (= 同じ post_filter_bypassed
   切替を使っているはず — 要確認)
4. テスト追加: `tests/erase_apply_no_double_mig`an_run.rs` のような新規ファイルで、
   Apply 後に MI-GAN 呼び出し回数が 1 (not 2) になることを確認
   - 実装難易度高: MI-GAN は実 GPU 推論なので mock が要る。代わりに
     `erase_inpaint_pending.len()` や cancel count をカウントして検証する形でも可

### Step 2: 案 B-1 を実装

1. `bump_input_generation` に `adjustment_dragging` ガードを追加
2. `set_page_params` / slider drag 終了ハンドラで、drag 終了時に bump を 1 回実行
   - 現状 slider 終了の検知箇所を探す: `adjustment_dragging` を false に戻す箇所が
     1 つでもあればそこに `bump_input_generation(fs_idx)` を追記
3. テスト追加: `adjustment_dragging = true` で複数回 clear_adjustment_caches を呼んだ
   とき、`erase_result_cache` が削除されないことを確認

### Step 3: 文書更新

1. [`docs/preset-and-adjustment.md`](../../preset-and-adjustment.md) §9 の「キャッシュ無効化」
   表を更新:
   - `post_filter 切替` 行を追加 → `adjustment_cache クリア / erase_result 維持`
   - `スライダー drag 中` 行を追加 → `bump 抑止 / drag 終了で 1 回 bump`
2. [`docs/display-pipeline.md`](../../display-pipeline.md) §3 の「erase_result_cache の
   無効化ポリシー」節 (= 新設) に上記の判断基準を残す
3. [CLAUDE.md](../../../CLAUDE.md) §「並行処理」近辺に、`bump_input_generation` の
   「呼び出し条件: 真にピクセルが変わるときだけ」を 2-3 行で明記

### Step 4: 実機検証

- 補正プリセットが効いた状態で消しゴム Apply → MI-GAN が 1 回だけ走る (perf log で
  確認可能、`erase: inpaint start` ログの回数を count)
- 保存済み mask があるページで明るさ slider を drag → drag 中は MI-GAN spawn なし、
  drag 終了で 1 回だけ走る
- 通常の AI 完了 / 補正確定操作で MI-GAN が再走することを確認 (= regression なし)

---

## 6. 既知の落とし穴 / レビュー観点

### 6.1 adjustment_dragging の対称ペアリング

`adjustment_dragging = true` を立てる箇所と false に戻す箇所が **必ず対** に
なっていないと、true のまま固まって以降 bump が一切走らない致命的バグになる。
レビューで grep して確認:

```bash
grep -n 'adjustment_dragging = ' src/ -r
```

drag 開始 / 終了の両方で `clear_adjustment_caches` が呼ばれる経路があるか確認。
特に **panic / 例外 / early return** で drag 終了処理が飛ばされる経路がないこと。

### 6.2 多重 idx が走る場合

見開きモードで両ページに保存済み mask があり、片方の Apply のみが走る:
- 左ページの Apply → reset_erase_mode → 左 idx の bump 抑止 + J1 だけが走る
- 右ページは触られない (J もキャンセルされない)

両ページの結果が混在する状態 (= 片方が新、片方が旧) で問題ないか実機確認。

### 6.3 `clear_conceal_caches` は両経路で呼ぶ

post_filter 切替でも conceal_cache は **clear すべき** (post_filter のオン/オフ
で conceal の見え方が変わる可能性があるため)。

詳しくは [docs/conceal-feature-plan.md](../../conceal-feature-plan.md) §9。要確認。

### 6.4 metadata_cache_key vs idx

`bump_adjustment_generation` は `metadata_cache_key(idx)` 経由でファイル単位の
generation を bump する。idx 単位の `bump_input_generation` とは独立。両方を理解した
上で改修すること。

### 6.5 既存テストへの影響

`tests/` 配下で `clear_adjustment_caches` / `bump_*_generation` を直接呼んでいる
テストはあまり多くないはず。ただし `cargo test --lib` で 1390 通る現状を回帰
させないこと。

```bash
cargo fmt --check
cargo check
cargo test --lib
cargo test --tests
cargo test --test ui_snapshot
cargo build --release --bin mimageviewer-core
```

---

## 7. 完了条件

- [ ] Apply 操作で MI-GAN が 1 回だけ走る (実機 perf log で確認)
- [ ] Slider drag 中に MI-GAN が spawn されない、drag 終了で 1 回だけ走る
- [ ] AI 完了 / 補正確定 / フォルダ切替で従来通り `erase_result_cache` が無効化される
- [ ] [docs/preset-and-adjustment.md](../../preset-and-adjustment.md) §9 のキャッシュ表が
      新しい振る舞いを反映
- [ ] 全テスト pass (`cargo test --lib` 1390+、`cargo test --tests` 1671+)
- [ ] perf 改善を測定値で報告 (例: 「補正プリセット有効化状態の Apply で MI-GAN
      実行回数 2→1、レスポンス時間 ~50% 短縮」)

---

## 8. 工数感

| ステップ | 想定 |
|---|---|
| Step 1 (post_filter 専用ヘルパー) | 0.5 日 |
| Step 2 (drag 中の bump 抑止 + 終了時 1 回 bump) | 0.5-1 日 |
| Step 3 (docs 更新) | 0.5 日 |
| Step 4 (実機検証 + perf 計測) | 0.5 日 |
| **合計** | **2-2.5 日** |

---

## 9. リスク評価

### High
- `bump_input_generation` の挙動を変えるので、消しゴム / 隠蔽 / 補正の cache 整合性
  全般に波及する。テストカバレッジが薄い領域 (= 見開きモードでの片側 Apply、
  spread mode 跨ぎ等) は手動 QA が必要

### Medium
- `adjustment_dragging` フラグが他の経路で再利用されている場合、bump 抑止が想定
  外の効果を持つ可能性。grep で全使用箇所を確認

### Low
- post_filter 経路は限定的なので案 A は局所的修正

---

## 10. 代替案: 「何もしない」

両症状とも **最終結果は正しい** (= ユーザーが見る画像は意図通り)。perf / UX
改善のための作業なので、優先度を下げて見送る判断もアリ。

その場合は CLAUDE.md か docs/preset-and-adjustment.md に「既知の perf 課題」として
1 段落残し、将来のリリースで取り組む。

ユーザーが実機で「Apply 後に 1 秒以上待たされる」を不快に感じる場合は優先度を上げ、
そうでなければ後回しでよい。

---

## 補足: 修正の影響を測る指標

実装後に測定すべき定量指標:

- `erase: inpaint start` ログの回数 (= MI-GAN spawn 数)
  - 補正なし Apply: 1 回 (変わらず)
  - 補正あり Apply: 2 回 → **1 回** (改善)
  - Slider drag (N tick): N+1 回 → **1 回** (大幅改善)
- `erase: inpaint cancelled` ログの回数
  - 補正あり Apply: 1 回 → **0 回**
- Apply 〜 erase 結果表示までのフレーム数
  - 補正あり: ~30-60 frames → **~15-30 frames** (半分)

`scripts/analyze_perf.py` で抽出可能 (perf event 名は要確認)。
