# stage-r2e-2pre — 手書き mount 18 箇所を 2 本の helper へ寄せる

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §7 ②-pre。
**憲法 §2 の適用範囲に基づく合意は取得済み**で、根拠は同プラン **§11 の 2026-08-23 の項**に
記録してある。着手前にその項も読むこと。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2pre)` を含める。

**触ってよいファイルは `src/app.rs` と `src/app/tests.rs` の 2 本だけ。**

---

## 1. やること

「bundle を holder から取り出す → App へ swap → 何かする → swap で戻す → holder へ戻す」
という**所有権移動の定型が 18 箇所に手書きコピーされている**。正しい版は既にある:

```rust
// src/app.rs:16703 — catch_unwind + resume_unwind で panic 安全
pub(crate) fn with_active_detached_viewer_context<R>(
    &mut self, f: impl FnOnce(&mut Self) -> R,
) -> Option<R>;
```

parked 用の対を足し、18 箇所を 2 本へ寄せる。

```rust
/// window_id の parked bundle をマウントして f を実行し、元へ戻す。panic 時も戻す。
///
/// - その窓が存在しない / bundle を持っていない → `None` を返し `f` を呼ばない。
/// - **`f` の中でその窓自体が閉じられた場合は戻す先が無いので bundle を drop する**
///   (現行の vst3 経路 [app.rs:19855](../src/app.rs:19855) と同じ。id が残っていれば戻す)。
/// - **`detached_viewer_main_history_suppression_depth` は上げない**
///   (active 側 helper との非対称。parked 経路には現行もその抑止が無い)。
/// - **`native_video_parked_live_input_window_id` は触らない** (§3.4)。
#[cfg(windows)]
fn with_paused_detached_context<R>(
    &mut self, window_id: u64, f: impl FnOnce(&mut Self) -> R,
) -> Option<R>;
```

**この段では保管の形も型も変えない。** `active_detached_viewer_context` も
`paused_bundle` もそのまま。変えるのは「誰がその定型を書くか」だけ。

### 1.1 なぜ先にやるのか

②-d は保管フィールド 2 つを消して registry へ移す**1 コミットの一括切替**である。
その時点で手書き mount が 18 箇所残っていると、②-d は「保管を移す」と「18 箇所を書き直す」を
同時にやることになる。先に 2 本へ寄せておけば、②-d が書き直すのは **2 本だけ**になる。

さらに、「取り出せなかったので飛ばす」という分岐が helper 1 箇所に集まる。
②-d でそこが `residence()` の match になる。

## 2. 変換する 18 箇所

| # | 行 | 関数 / 文脈 | 種別 |
| --- | --- | --- | --- |
| 1 | [19808](../src/app.rs:19808) | `consume_deferred_vst3_media_open_in_all_contexts` | active |
| 2 | [27885](../src/app.rs:27885) | `metadata_import_refresh_index = None` の一括クリア | active |
| 3 | [27932](../src/app.rs:27932) | metadata import terminal index の前進 | active |
| 4 | [28151](../src/app.rs:28151) | metadata import 要求の組み立て | active |
| 5 | [28224](../src/app.rs:28224) | metadata writer の quiesce | active |
| 6 | [28374](../src/app.rs:28374) | metadata import 結果の適用 | active |
| 7 | [28994](../src/app.rs:28994) | rename 後の rehydrate | active |
| 8 | [30422](../src/app.rs:30422) | `rebuild_all_viewer_context_visible_indices` | active |
| 9 | [54322](../src/app.rs:54322) | `clear_all_edit_preview_materializations` | active |
| 10 | [19844](../src/app.rs:19844) | `consume_deferred_vst3_media_open_in_parked_contexts` | parked |
| 11 | [27892](../src/app.rs:27892) | #2 の parked 版 | parked |
| 12 | [27951](../src/app.rs:27951) | #3 の parked 版 | parked |
| 13 | [28170](../src/app.rs:28170) | #4 の parked 版 ⚠ §4.1 | parked |
| 14 | [28232](../src/app.rs:28232) | #5 の parked 版 | parked |
| 15 | [28398](../src/app.rs:28398) | #6 の parked 版 ⚠ §3.2 | parked |
| 16 | [29017](../src/app.rs:29017) | #7 の parked 版 ⚠ §4.1 | parked |
| 17 | [30430](../src/app.rs:30430) | #8 の parked 版 | parked |
| 18 | [54329](../src/app.rs:54329) | #9 の parked 版 | parked |

**変換しないもの** (mount-and-restore ではない。触らないこと):

- park: [38344](../src/app.rs:38344) / [42556](../src/app.rs:42556)
- 終端 close / teardown: [38583](../src/app.rs:38583) / [39457](../src/app.rs:39457)
- activation の恒久移動: [40155](../src/app.rs:40155) / [41048](../src/app.rs:41048)
- parked-live poll [39830](../src/app.rs:39830) — mount-and-restore だが close-after-poll と
  結合している。**②-d へ送る** (この段では触らない)

## 3. 等価性の制約 (レビューで確定済み。破らないこと)

### 3.1 挙動の差は 1 つだけ

mount 中の closure が panic したとき、押しのけられた bundle が **drop されずに戻る**。
panic 自体は `resume_unwind` でそのまま伝播するので**何も握り潰さない**。
これ以外の差を出さないこと。

### 3.2 ⚠ #15 (28398) は 3 分岐を保つ

現行は `window_index` で引き、**3 つの場合を区別**している
([app.rs:28387](../src/app.rs:28387) 付近):

| 現行 | 挙動 |
| --- | --- |
| その index に窓があり id が一致 | 適用する |
| その index に窓があるが id が違う | `stale_context = true` して skip |
| **その index に窓が無い (範囲外)** | **黙って skip** (`stale_context` は立てない) |

`with_paused_detached_context(window_id, ..)` へ素直に置き換えると、
**期待の窓が前の index へずれて元の index が範囲外になった場合**でも window_id で見つけてしまい、
**今は捨てられている結果が適用される**。これはステージ④の identity 是正の先取りであり、
この段でやってはならない。

したがって **helper を呼ぶ前に、必ずこの形で検証する**:

```rust
match self.detached_image_windows.get(window_index) {
    Some(window) if window.id == window_id => {}
    Some(_) => {
        stale_context = true;
        continue;
    }
    None => continue,          // ← この分岐を落とさない
}

if let Some(applied) = self.with_paused_detached_context(window_id, |app| {
    app.apply_current_metadata_import_terminal_result(context, changed)
}) {
    stale_context |= !applied;
}
```

検証の直後に同じ UI スレッドで helper を呼ぶので、間に順序の問題は入らない。

### 3.3 ⚠ #10 (19844) の parked-live marker は closure の内側

`native_video_parked_live_input_window_id` を `Some(id)` にして戻す処理は、
**マウント後・アンマウント前**、つまり closure の**内側**で行うこと。

### 3.4 helper に `native_video_parked_live_input_window_id` を入れない

あれは parked-live の入力 / メディア方針であって汎用の residence ではない。
helper へ入れると metadata import や cache 保守の mount 中に入力フィルタ・HUD・activation・
メディア所有権が変わる。汎用化は②-d で `residence()` として行う (§11 の記録参照)。

### 3.5 ⚠ active helper の suppression depth を各本体で確認する

`with_active_detached_viewer_context` は
`detached_viewer_main_history_suppression_depth` を一時的に上げる。手書き 9 箇所は上げていない。
**変換対象の active 9 本の本体から、この深さを読む処理へ到達しないことを 1 本ずつ確認すること。**

読み手は **4 箇所**ある (抽象経由 2 + 直読み 2):

| 読み手 | 場所 |
| --- | --- |
| `detached_viewer_suppresses_main_history_persistence` ([app.rs:16675](../src/app.rs:16675)) | history 遷移 [app.rs:17691](../src/app.rs:17691) / startup・quick folder の永続化 [app.rs:24823](../src/app.rs:24823) |
| `detached_viewer_context_is_mounted` (深さを直読み) [app.rs:32024](../src/app.rs:32024) | `bump_full_context_for_load` が消費 |
| `route_materialized_physical_still_open_to_active_context` (深さを直読み) [app.rs:42489](../src/app.rs:42489) | — |

作業手順: まず
`rg 'detached_viewer_main_history_suppression_depth' src/` で読み手の全数を取り直し
(上の表が古くなっていないか確認)、次に 9 本の closure 本体から**呼び出しを辿って**
到達しないことを確認する。**production の VST3 コールバック (`consume`) も辿ること。**
到達する経路が 1 つでも見つかったら、**その箇所は変換せずに報告する**。

## 4. 書き方

### 4.1 parked 側のループの回し方は 1 通りではない

「id で回す」を全 9 箇所へ一律に適用してはならない。site ごとに必要なものが違う。

| # | 回し方 | 保つもの |
| --- | --- | --- |
| #10 (19844) | id。既にそうなっている | **pending を持つ窓だけ**という前置きフィルタを mount の外に残す |
| #11 (27892) | 全 id を 1 回スナップショット | 本体は vector を変えない |
| #12 (27951) | 全 id を 1 回スナップショット | deadline / budget の判定と `complete` の扱い (§4.2) |
| #13 (28170) | ⚠ **`Vec<(window_index, window_id)>` を 1 回スナップショット**。bundle を持たない窓も含める | **`ContextSlot::PausedDetached { index }` に入れるのは元の vector index**。id だけにすると slot の index がずれる |
| #14 (28232) | 全 id を 1 回スナップショット | `busy` の扱い (§4.2) |
| #15 (28398) | ⚠ **窓のループではない**。結果 (`ContextResult`) 側から `(window_index, window_id)` が来る | §3.2 の 3 分岐 |
| #16 (29017) | **rename 述語に一致する窓の id だけ**をスナップショット | 一致しない context を mount しないこと |
| #17 (30430) | 全 id を 1 回スナップショット | — |
| #18 (54329) | 全 id を 1 回スナップショット | — |

「index ではなく id で回す」ことにした理由は、closure が `detached_image_windows` を
動かし得るためだが、**#11 / #12 / #14 / #17 / #18 の本体は vector を変えない**。
それでも id を先にスナップショットすれば、現行の「長さを 1 回だけ読む」挙動
(途中で足された窓は回らない / 順序は初期の vector 順) がそのまま保たれる。

### 4.2 外へ値を出す本体の扱い (推測しない。この 4 通りだけ)

**`busy |= ...` (#5 active / #14 parked)** — mount できなければ寄与しない:

```rust
if let Some(context_busy) = /* helper */ {
    busy |= context_busy;
}
```

**`complete = ...` (#3 active)** — 既存の 3 つの gate をすべて残す。
helper が `None` を返したら **`complete` に代入しない**。

**`complete = ...` (#12 parked)** — deadline / budget の判定は **mount の前**に行う。
`Some(value)` なら代入し、`false` なら break。`None` なら `complete` を変えずに続行。

**`stale_context |= ...` (#6 active / #15 parked)** — closure が適用結果を返し、

```rust
if let Some(applied) = /* helper */ {
    stale_context |= !applied;
}
```

mount に失敗した場合、現行も staleness に寄与しない。

**VST3 (#1 active / #10 parked)** — ⚠ **Rust の値を外へ出さない**。
`take_mounted_deferred_vst3_media_open` と **`consume(app, idx)` の両方を closure の内側**で呼ぶ。
`idx` を返して unmount 後に consume すると **別の context に対して実行してしまう**。
active 側の pending gate と parked 側の「pending を持つ id だけ」フィルタは残す。
helper の戻り値 `Option<()>` は捨ててよい。

### 4.3 変換の例 (#8 / #17)

```rust
self.with_active_detached_viewer_context(|app| {
    app.rebuild_visible_indices_preserving_facet_scope();
});
let window_ids: Vec<u64> = self.detached_image_windows.iter().map(|w| w.id).collect();
for window_id in window_ids {
    self.with_paused_detached_context(window_id, |app| {
        app.rebuild_visible_indices_preserving_facet_scope();
    });
}
```

## 5. スコープ外

- 保管フィールド (`active_detached_viewer_context` / `paused_bundle`) の形を変えること
- `ViewerContextBundle` / `swap_viewer_context_bundle` に触ること
- `viewer_context_registry` を production から呼ぶこと (②-d)
- **汎用の全 context 巡回 helper (`for_each_viewer_context` 等) を作ること (ステージ③)**
- `ContextSlot` の identity 是正 (ステージ④)
- §2 の「変換しないもの」に手を入れること

## 6. 触ってはいけないもの

- **憲法 1**: rect 一致捕捉に条件を足さない
- **憲法 2**: geometry 由来の host_lost を recreate トリガにしない
- **憲法 3**: App に新しい `bool` / `Option` を足さない (**この段は App にフィールドを 1 つも足さない**)
- **憲法 4**: placement の保存先・同期経路を作らない
- **憲法 5**: 時間窓で競合を吸収しない
- **憲法 7**: §2 に列挙していないファイル・機構を「ついでに」直さない
- **憲法 8**: 既存テスト (detached 関連 約 207 本) を削除・弱体化しない

## 7. テスト

### 7.1 helper 自体

1. **panic 安全 (この段で唯一挙動が変わる点)**:
   - active: closure の中で panic → `resume_unwind` で伝播し、**押しのけられた main 投影と
     bundle が戻っている**こと、**かつ suppression depth が元に戻っている**こと。
   - parked: closure の中で panic → parked bundle と main 投影の**両方**が戻っていること。
   - `catch_unwind` + `AssertUnwindSafe` で受ける。
2. bundle を持たない窓 / 存在しない窓で `with_paused_detached_context` が
   `None` を返し closure を呼ばないこと。
3. **closure の中で対象の窓が閉じられた場合**の規約 (§1 の doc comment) が守られること。

### 7.2 変換の等価性

4. **#15 の 3 分岐**:
   - その index に窓があるが **id が違う** → 適用されず `stale_context` が立つ。
   - **期待の窓が前へずれて元の index が範囲外** → **適用されず、`stale_context` も立たない**。
     (この段で最も壊しやすい。id で引く helper に素通しすると適用されてしまう)
5. **#13 の index 保持**: bundle を持たない窓を先頭に置いた状態で、
   発行された `ContextSlot::PausedDetached { index }` が**元の vector index** であること。
6. **mount に失敗しても `busy` / `complete` / `stale_context` が変わらない**こと。
7. **parked context が既にマウントされている最中**に、変換後の「全 context 操作」を呼んでも
   その context が処理されること (設計 §8 のテスト計画で②-pre に求めているもの)。
8. **vector を変更するテストは、合成ループではなく変換後の production 巡回を通すこと。**
   VST3 の `consume` は差し替え可能なので、その中で**まだ回っていない先の id の窓**を閉じ、
   **後続の生存 id が処理され続ける**ことを確認するのに使える。
   (テスト 3 は「今マウント中の窓自体が閉じられた場合」で、こちらとは別のケース)

### 7.3 既存テストの活用

次は変換の等価性を直接踏むので、**赤くなったら書き換えずに報告する**:

- VST3 の mounted marker / context [tests.rs:40076](../src/app/tests.rs:40076)
- context を跨ぐ metadata 適用 [tests.rs:601](../src/app/tests.rs:601)
- 編集プレビューの clear [tests.rs:719](../src/app/tests.rs:719)
- visible index の再構築 [tests.rs:924](../src/app/tests.rs:924)
- writer の quiesce [tests.rs:1001](../src/app/tests.rs:1001)
- 絞り込み付き parked rename rehydrate [tests.rs:41121](../src/app/tests.rs:41121)

### 7.4 書き方

新規テストは、可能な範囲で **holder フィールドを直接覗かず helper 越しに**検証する。
`active_detached_viewer_context` と `paused_bundle` は**②-d で両方消える**ので、
直接覗くテストはそこで書き直しになる。

## 8. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
2. `cargo test -p mimageviewer --lib` が緑。**既存テストの本数が減っていない。**
3. §7 の新規テストが緑。
4. `cargo fmt` 済み。
5. **手書き mount が 0 件**:
   ```bash
   grep -n "active_detached_viewer_context.take()" src/app.rs
   # -> with_active_detached_viewer_context (helper 本体)
   #    + 変換しない 3 箇所 (park 38344 / 終端 close 38583 / media handoff の park 42556)
   #    = 4 件
   grep -n "paused_bundle.take()" src/app.rs
   # -> with_paused_detached_context (**新しい helper 本体も take する**)
   #    + 変換しない 4 箇所 (teardown 39457 / parked-live poll 39830 /
   #      activation 40155 / activation 41048)
   #    = 5 件
   ```
   (行番号は変換で動くので、**件数と関数名**で確認すること)
6. **実機確認用バイナリを作る**: `.\scripts\build-dev.ps1`。
   本ステージは production の挙動を変える (§3.1) ので、CLAUDE.md
   「修正完了時の実機確認用バイナリ」の方針どおり、利用者が §9 の smoke を回せる状態にする。
   **エージェントは起動しない。**
7. 完了報告に:
   - 18 箇所それぞれの変換後の姿
   - §4.2 の 4 通りをどう適用したか (site ごと)
   - **§3.5 の suppression depth 到達確認**: `rg` の出力と、9 本それぞれの追跡結果
   - 完了条件 5 の grep の実際の出力
   - `build-dev.ps1` の結果

## 9. 実機 smoke (利用者が実施)

挙動の差は panic 経路だけなので、**通常操作で差は出ない前提**。ただし変換範囲が広いので:

- 複数ウィンドウで画像 / 動画 / ZIP / PDF を開き、**メタ情報の一括 import** を実行して、
  各ウィンドウのレーティング・タグ表示が正しく更新されること
- ファイル名変更後に、別ウィンドウの一覧が壊れないこと
- 動画ウィンドウを parked-live にしたまま、メイン側でフォルダ移動・スタック切替を行うこと
- VST3 を有効にした状態で起動し、deferred media open が正しい窓で再開すること
