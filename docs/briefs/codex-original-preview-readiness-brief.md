# §1.89 — 元画像プレビュー中のページ送りが 4〜5 秒止まる

対象: 新規 (backlog §1.89 として追記する)。**利用者報告 + 実測済み**。
§1.88 (`4606c318`) の修正で永久固着が消えた結果、露出した**別の既存問題**。

## 0. 実測 (推測ではない)

右 Ctrl (既定 = `押している間だけ元画像を表示する`) を押しながら Ctrl+Right で
見開き 1 ページずらしを連続実行すると、**同じページ対で 4〜5 秒止まる**。
2026-08-17 の実 perf log (`perf_events.jsonl`) から:

| 停止 | 長さ | ページ対 | その間の Ctrl+Right |
| --- | --- | --- | --- |
| 1 | **5.22s** | (4, 5) | **54 回 consume されて target は 1 度も進まず** |
| 2 | 4.30s | (4, 5) | 同様 |

停止中の内訳は 2 回とも同一:

```
PDF decode                 約 150ms
ai/upscale_begin→end       1.3s × 3 回 (逐次)   ← 大半
fs/final_composite_build   73ms  (カラー化)
→ ここで target が進む
```

**支配的なコストは AI アップスケール。** カラー化 (final composite) は AI の完了後にしか
走れない 73ms の尾であって原因ではない。

対照: 左 Ctrl では target が約 0.14 秒で落ち着く (382 trigger → 149 target)。
**§1.88 の退行ではない**。§1.88 の修正は効いており、永久固着は消えて Backspace 不要になっている。

## 1. 根本原因 — readiness が「実際に描く source」ではなく「加工済み source」を待っている

`FsNavigationSequence` の target readiness ([ui_fullscreen.rs](../../src/ui_fullscreen.rs) の
`materialized_ready` 計算):

```rust
let materialized_ready = target.pages.iter().all(|idx| {
    self.resolve_fs_display_tex(*idx, true).is_some()      // ← 加工済みを要求
    || (...thumbnail fallback...)
});
```

ところが**描画側は元画像へ迂回している**:

```rust
fn fs_display_bypasses_final_pipeline(&self, original_preview_active: bool) -> bool {
    original_preview_active || self.analysis_mode
}
// → true なら resolve_original_preview_tex(idx) を使う
```

つまり **そのフレームが実際に描くのは元画像なのに、readiness は AI + カラー化の完了を待つ**。
これが不整合の本体である。

さらに通過表示 (低解像度の代理) も塞がれている。
`fs_page_turn_ordinary_context_blocker` が `original_preview_active` で
`Some("original_preview")` を返すため `accept_rendition = false` になり、
`rendition_ready` の経路も使えない (実 log に `passthrough_rendition_unavailable` が 1608 件、
`navigation_target_phase` の `accept_rend=False`)。

結果:

1. 元画像プレビュー中 → 通過表示なし + readiness は加工済み待ち
2. phase が `Awaiting` のまま
3. `blocks_new_target()` は **`RenditionFailed` 以外すべてで true** を返す
   ([app.rs](../../src/app.rs)) → 追加の Ctrl+Right は consume されても再 target できない
4. AI + composite が終わるまで 4〜5 秒停止

## 2. 直し方 (利用者判断 2026-08-17 で確定)

**不変条件: readiness は「そのフレームが実際に描く source」に対して評価する。**

`fs_display_bypasses_final_pipeline(original_preview_active)` が true のとき、
`materialized_ready` は **`resolve_original_preview_tex(idx)` が解決するか**で判定する。
元画像は AI もカラー化も通らないので即座に ready になり、停止が消える。

これは機能ごとの carve-out ではなく、**描画 source と readiness source を一致させる**修正である。
現状は「描くもの」と「待つもの」が別で、そのずれが症状として出ている。

### 2.1 気を付けること

- `original_preview_active` は **OS キー状態を読む**。既に
  「呼び出し側で一度だけ評価する」規約がある ([ui_fullscreen.rs](../../src/ui_fullscreen.rs) の
  該当 doc comment)。readiness 計算へ**フレームごとに 1 回だけ**渡すこと。
  readiness の内側で再度 OS を読まない (同一フレーム内で判定が割れる)。
- `analysis_mode` も同じ `fs_display_bypasses_final_pipeline` を通る。
  **分析モードでも同じ不整合が成立する**ので、`original_preview` 専用の条件を書かず
  `fs_display_bypasses_final_pipeline` を判定に使うこと。
- キーを離した瞬間に加工済みへ戻る。そこで AI が走るのは**正しい挙動**
  (利用者判断)。離した後の初回表示が加工済みを待つのは従来どおり。
- 「カラー化の白黒→カラー切り替わりを見せない」要件には反しない。
  **利用者が元画像を明示的に要求している間だけ**の話であり、要求していない切り替えを
  見せるわけではない。要件の適用範囲をこの理由付きでコメントに残すこと。

### 2.2 やらないこと

- `blocks_new_target()` の呼び出し側に「元画像中は例外的に通す」条件を足さない。
  直すのは readiness の評価対象であって、ブロックの迂回ではない。
- `original_preview` を `fs_page_turn_ordinary_context_blocker` から**外さない**。
  通過表示 (加工済みの低解像度代理) を出さないこと自体は正しい
  (元画像要求と矛盾する)。塞ぐべきは代理表示であって readiness ではない。
- AI アップスケール側に手を出さない (キャンセル / 待たない化は別件。§3 参照)。
- 時間窓 / タイムアウトで `Awaiting` を強制解除しない (憲法 5)。
- §1.31 / §4.2 / §1.88 の作業に手を出さない。

## 3. 範囲外として記録すること

停止の**コストそのもの** (AI アップスケールが 1.3s × 3 回 = 約 4 秒、逐次) は本件では直さない。
元画像プレビュー以外の経路でも同じ待ちは起き得るので、
**backlog に別項として残す** (「送り中の AI アップスケールを待たない / 打ち切る」)。
本件は「元画像を描くフレームが加工済みを待つ」不整合だけを直す。

## 4. 触ってよいファイル

- `src/ui_fullscreen.rs`
- `src/app.rs` (必要最小限)
- `src/app/tests.rs` または `src/ui_fullscreen.rs` の test module
- `docs/next-release-backlog.md` (§1.89 追記 + §3 の別項追記)
- `docs/display-pipeline.md` (readiness の不変条件を書き足すとき)
- `docs/detached-rework-plan.md` (§11 記録)

## 5. テスト

1. `fs_display_bypasses_final_pipeline` が true のとき、加工済みテクスチャが**無い**状態でも
   元画像が解決すれば `materialized_ready` が true になる。
2. false のとき (通常表示) は従来どおり加工済みを要求する。**既存の待ち挙動を変えない。**
3. 元画像も無い場合は `Awaiting` のまま (即 ready にしない)。
4. `analysis_mode` でも 1 と同じになる。
5. 見開きは**両ページが揃ってから** ready (片側だけで ready にしない = atomic 契約維持)。
6. §1.88 の回帰テスト 4 本と既存の atomic 契約テストが**無修正で**通る。

**6 が赤くなったら報告して止まること。**

## 6. 凍結ルール

[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に読むこと。
憲法 5 (時間窓で競合を吸収しない) が特に効く。完了時に §11 へ追記。

## 7. 完了条件

1. readiness が `fs_display_bypasses_final_pipeline` を見て評価対象を切り替えている。
2. `original_preview` 専用の条件分岐になっていない (§2.1)。
3. `blocks_new_target()` の呼び出し側に例外が足されていない (§2.2)。
4. §5 のテスト 1〜5 が入り、通る。
5. §5 の 6 (既存テスト) が無修正で通る。
6. `cargo fmt --check` が通り、`.\scripts\test-full.ps1` が exit 0。
7. §11 記録と backlog 更新 (§1.89 完了 + §3 の別項追記)。
8. 実機確認の手順を報告に書く。

## 8. 実機確認 (利用者が後で行う)

- 右 Ctrl を押しながら Ctrl+Right / Ctrl+Left を連続 → **4〜5 秒の停止が無い**
- 右 Ctrl を離すと加工済み (カラー化 + AI) 表示に戻る
- 右 Ctrl を押していない通常のページ送りが従来どおり (通過表示が出る、ちらつかない)
- 分析モードでも送りが止まらない
