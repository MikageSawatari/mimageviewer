# stage-r2e-2pre — 手書き mount 18 箇所を 2 本の helper へ寄せる

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §7 ②-pre。
**憲法 §2 の適用範囲に基づく合意は取得済み**で、根拠は同プラン **§11 の 2026-08-23 の項**に
記録してある。着手前にその項も読むこと。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2pre)` を含める。

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
/// window_id の parked bundle をマウントして f を実行し、必ず元へ戻す (panic 時も)。
/// bundle を持っていない窓 / 存在しない窓なら None を返し f を呼ばない。
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

さらに、「取り出せなかったので飛ばす」という分岐 (現在 6 箇所の `else { continue }`) が
helper 1 箇所に集まる。②-d でそこが `residence()` の match になる。

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
| 11 | [27892](../src/app.rs:27892) | 上記 2 の parked 版 | parked |
| 12 | [27951](../src/app.rs:27951) | 上記 3 の parked 版 | parked |
| 13 | [28170](../src/app.rs:28170) | 上記 4 の parked 版 | parked |
| 14 | [28232](../src/app.rs:28232) | 上記 5 の parked 版 | parked |
| 15 | [28398](../src/app.rs:28398) | 上記 6 の parked 版 ⚠ §3.2 | parked |
| 16 | [29017](../src/app.rs:29017) | 上記 7 の parked 版 | parked |
| 17 | [30430](../src/app.rs:30430) | 上記 8 の parked 版 | parked |
| 18 | [54329](../src/app.rs:54329) | 上記 9 の parked 版 | parked |

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

### 3.2 ⚠ #15 (28398) の identity 検証を落とさない

現行は `window_index` で引き、**その窓の `id` が期待値と一致するか**を確かめてから適用する
([app.rs:28380](../src/app.rs:28380) 付近)。`with_paused_detached_context(window_id, ..)` へ
素直に置き換えると、**index がずれた context を window_id で見つけてしまい、
今は捨てられている結果が適用される**。

→ **既存の `window_index` + `window_id` の検証を残したまま**、マウント部分だけを helper へ移す。
identity の是正 (`ContextSlot` を `ViewerContextId` にする) は**ステージ④の担当**であり、
この段で先取りしない。

### 3.3 ⚠ #10 (19844) の parked-live marker は closure の内側

`native_video_parked_live_input_window_id` を `Some(id)` にして戻す処理は、
**マウント後・アンマウント前**、つまり closure の**内側**で行うこと。

### 3.4 helper に `native_video_parked_live_input_window_id` を入れない

あれは parked-live の入力 / メディア方針であって汎用の residence ではない。
helper へ入れると metadata import や cache 保守の mount 中に入力フィルタ・HUD・activation・
メディア所有権が変わる。汎用化は②-d で `residence()` として行う (§11 の記録参照)。

### 3.5 active helper の suppression depth

`with_active_detached_viewer_context` は `detached_viewer_main_history_suppression_depth` を
一時的に上げる。変換対象の active 9 本の本体から、その読み手
(`detached_viewer_suppresses_main_history_persistence`) へ到達しないことは確認済みだが、
**変換のたびに各本体で再確認し、到達する経路が見つかったら変換せずに報告すること**。

## 4. 書き方

現行 (例: #8 / #17):

```rust
if let Some(mut active) = self.active_detached_viewer_context.take() {
    self.swap_viewer_context_bundle(&mut active.bundle);
    self.rebuild_visible_indices_preserving_facet_scope();
    self.swap_viewer_context_bundle(&mut active.bundle);
    self.active_detached_viewer_context = Some(active);
}
for window_index in 0..self.detached_image_windows.len() {
    let Some(mut bundle) = self.detached_image_windows[window_index].paused_bundle.take()
    else { continue };
    self.swap_viewer_context_bundle(&mut bundle);
    self.rebuild_visible_indices_preserving_facet_scope();
    self.swap_viewer_context_bundle(&mut bundle);
    self.detached_image_windows[window_index].paused_bundle = Some(bundle);
}
```

変換後:

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

- **index ではなく id で回す。** closure の中で `detached_image_windows` が動く可能性があるため、
  先に id を集めてから回す。ただし **#15 は §3.2 のとおり index 検証を残す**。
- 戻り値が要る本体 (`busy |= ...` / `complete = ...` / `stale_context |= ...`) は
  helper の戻り値 `Option<R>` を使う。**`unwrap_or(既定値)` の既定値は、現行で
  「取り出せなかったとき」に起きることと一致させること** (例: `complete` は現行だと
  `continue` で値が変わらないので、`unwrap_or(complete)` ではなく変数を書き換えない形にする)。
  ここは 1 箇所ずつ現行の制御フローを読んで決める。**推測で既定値を置かない。**

## 5. スコープ外

- 保管フィールド (`active_detached_viewer_context` / `paused_bundle`) の形を変えること
- `ViewerContextBundle` / `swap_viewer_context_bundle` に触ること
- `viewer_context_registry` を production から呼ぶこと (②-d の仕事)
- §2 の「変換しないもの」に手を入れること
- `ContextSlot` の identity 是正 (ステージ④)

## 6. 触ってはいけないもの

- **憲法 1**: rect 一致捕捉に条件を足さない
- **憲法 2**: geometry 由来の host_lost を recreate トリガにしない
- **憲法 3**: App に新しい `bool` / `Option` を足さない (**この段は App にフィールドを 1 つも足さない**)
- **憲法 4**: placement の保存先・同期経路を作らない
- **憲法 5**: 時間窓で競合を吸収しない
- **憲法 7**: §2 に列挙していないファイル・機構を「ついでに」直さない
- **憲法 8**: 既存テスト (detached 関連 約 207 本) を削除・弱体化しない

## 7. テスト

1. **panic 安全の回帰テスト** (この段で唯一挙動が変わる点):
   - active: `with_active_detached_viewer_context` の中で panic させ、
     `resume_unwind` で伝播すること、**かつ押しのけられた bundle が holder に戻っていること**。
   - parked: 同じことを `with_paused_detached_context` で。
   - `catch_unwind` + `AssertUnwindSafe` で受ける。
2. **bundle を持っていない窓 / 存在しない窓**で `with_paused_detached_context` が
   `None` を返し、closure を呼ばないこと。
3. **closure の中で `detached_image_windows` が変化しても、id で回すループが壊れないこと**
   (窓が閉じられたら以降の id は `None` を返して飛ぶ)。
4. #15 (28398) の identity 検証が生きていること:
   **window_index の窓の id が期待値と違うとき、結果が適用されない**ことを固定する
   (この段で最も壊しやすい)。
5. 既存テストは**削除・弱体化しない**。18 箇所の変換で赤くなるテストが出たら、
   書き換えずに報告する。

## 8. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
2. `cargo test -p mimageviewer --lib` が緑。**既存テストの本数が減っていない。**
3. 新規テスト (§7 の 1〜4) が緑。
4. `cargo fmt` 済み。
5. **手書き mount が 0 件**になっていること:
   ```bash
   # 変換対象の 18 箇所に take → swap → swap → 戻す の形が残っていないこと
   grep -n "active_detached_viewer_context.take()" src/app.rs
   # -> 16703 (helper 本体) と、§2 の「変換しない」4 箇所だけ
   grep -n "paused_bundle.take()" src/app.rs
   # -> §2 の「変換しない」箇所だけ (39830 / 39457 / 40155 / 41048)
   ```
6. 完了報告に:
   - 18 箇所それぞれの変換後の姿 (helper 呼び出し 1 行で済んだか、戻り値の扱いをどうしたか)
   - **戻り値の既定値を決めた 4 箇所** (`busy` / `complete` / `stale_context` など) について、
     現行の制御フローと一致することの根拠
   - §3.5 の suppression depth 到達確認の結果
   - 完了条件 5 の grep の実際の出力

## 9. 実機 smoke (利用者が実施)

挙動の差は panic 経路だけなので、**通常操作で差は出ない前提**。ただし変換範囲が広いので:

- 複数ウィンドウで画像 / 動画 / ZIP / PDF を開き、**メタ情報の一括 import** を実行して、
  各ウィンドウのレーティング・タグ表示が正しく更新されること
- ファイル名変更後に、別ウィンドウの一覧が壊れないこと
- 動画ウィンドウを parked-live にしたまま、メイン側でフォルダ移動・スタック切替を行うこと
