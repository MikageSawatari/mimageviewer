# 見開きのページ戻り長押しで操作不能になる — upload 抑止の循環を断つ

正本は [next-release-backlog.md](../next-release-backlog.md) **§1.109**。
**再現・原因確認済み**なので、まず同項を読むこと。本ブリーフはコード上の位置と、
判断が要る点だけを補う。

**着手前に必読**: [CLAUDE.md](../../CLAUDE.md)「バグ修正の一般原則」、
[detached-rework-plan.md](../detached-rework-plan.md) §2 (凍結ルール)。

## 1. 循環の正体 (コード上の位置)

| | 位置 | 何をしているか |
| --- | --- | --- |
| ① | [ui_fullscreen.rs:5954](../../src/ui_fullscreen.rs:5954) | `FsNavigationTargetPhase::Awaiting { accept_rendition: true }` のとき **(active=true, ready=false)** を返す |
| ② | [ui_fullscreen.rs:8843](../../src/ui_fullscreen.rs:8843) | `defer_ui_uploads: rendition_sequence_active` — **ready を見ずに** active だけで抑止する |
| ③ | [app.rs:63202](../../src/app.rs:63202) | `poll_prefetch` が `defer_ui_uploads()` で**早期 return し、upload パス全体を飛ばす** |

見開きの片側だけ thumbnail が無いと ① の状態に入り、そのまま ②③ で
**target を materialize するための upload 自体が止まる**。target が完成しないので sequence は
退役できず、①のまま循環する。実ログでは 131072 frames 継続した。

**待っている当人が、待ちを終わらせる唯一の作業を止めている。**

## 2. 直し方 — 2 案。**§1.109 は (B) を決めている**

### (A) 抑止条件を `pass_through` に変える (1 行)

```rust
defer_ui_uploads: pass_through,   // = rendition_sequence_active && passthrough_rendition_ready
```

**利点**: 循環は消える。抑止が「代わりに描くものがあるとき」だけになり意味が通る。
**欠点**: rendition が無い連打では、通り過ぎるページの full-size upload を毎回払う。
連打を軽くするという ② の当初意図を失う。

### (B) 「現在の target に属する upload」だけ抑止から外す ← **こちらを採る**

§1.109 の方針そのもの:

> pass-through rendition が未準備でも、対象見開きの完成済み full-size result を
> `fs_upload_backlog` から反映して materialized target を settle できる状態遷移にする

**現在の navigation target のページに対する upload は抑止しない。それ以外の先読み upload は
従来どおり抑止してよい。** これなら連打の軽さを保ったまま循環が消える。

材料は既にある: `fs_navigation_rendition_target_pages(fs_idx) -> Option<Vec<usize>>`
([ui_fullscreen.rs:5971](../../src/ui_fullscreen.rs:5971))。

`poll_prefetch` の pacing は「現在 idx + もう 1 枚」を選ぶ ([app.rs:63207](../../src/app.rs:63207) 以降)
ので、見開きの相方は「もう 1 枚」に入る。**片側だけ反映されて止まらないこと**を確認すること
(§1.109 の回帰項目)。

**(B) が構造的に無理だと判断したら、実装せずに理由を報告すること。** (A) へ黙って落とさない。

## 3. 誤った意図を固定しているテスト

[ui_fullscreen.rs:36470](../../src/ui_fullscreen.rs:36470) の
`page_turn_decision_keeps_paint_and_ui_work_as_independent_axes` が
`(active=true, ready=false) → defer=true` を assert している。**名前自体が
「描画と UI 作業は独立した軸である」と主張しているが、この循環はその主張が
偽であることの証明**である。target のページに対する upload は描画と独立ではない。

**このテストは期待値ごと書き換える。名前も実態に合わせる。**
[36388](../../src/ui_fullscreen.rs:36388) の
`unresolved_page_turn_keeps_materialized_paint_behind_the_atomic_holdover` も同じ組み合わせを
assert しているので併せて見直す。

⚠️ CLAUDE.md の警告どおり、**実装を追認するテストが誤仕様を固定していた**例である。
書き換えるとき「今の実装がそうだから」ではなく「どの不変条件を守るか」で期待値を決めること。

## 4. 不変条件 (テストで固定する)

- 長押し中に受理したページ移動が順に処理され、**キーを離した後は最後に表示したページで
  必ず settled になる**。
- **前方向 / 後方向、素材形式、先読み完了順にかかわらず、次の入力を塞ぐ pending /
  transition 状態が残らない。**
- 実ログの順序を固定する: 「target の片側に thumbnail が無い」「もう片側だけ full-size ready」
  「残り片側の full-size result は upload backlog」→ **target が materialized で settle し、
  backlog が drain される**。
- 同一 target の `still_awaiting` が**無期限に継続しない**。

## 5. 兄弟経路の棚卸し (§1.109 の要求)

再現した経路だけで終わらせない。`navigation sequence` / `fs_pending` / `fs_upload_backlog` /
viewer context の **mount / park / activate** を同じ context owner と generation で棚卸しし、
**前後移動・open / switch / close / cancel / error** の兄弟経路に同じ循環が無いか確認する。

「抑止フラグが、その抑止を解除する唯一の作業を止める」という**同じ形**が他にないかを見ること。

## 6. 制約

- **timeout / 強制 reset / repaint 追加では直さない。**
- 複数ウィンドウの入力・表示状態所有に触れるため、[detached-rework-plan.md](../detached-rework-plan.md) §2 に従う。
  **detached predicate / viewport 経路へ変更が及ぶ場合は、実装せずに報告すること**
  (ClaudeCode と Codex 双方で「症状パッチではなく構造的修正である」ことに合意し、
  §11 へ記録する手順が要る)。
- 複数ウィンドウは順序を再現しやすくする条件であって原因ではない。**共有経路の問題として直す。**

## 7. 再現データ

`C:	mp\miv-spread-webp-portrait-600-20260820` (1200x1800、静止 WebP 600 枚、連番入り)。
複数の静止画ウィンドウを開いた状態で見開きを長押し往復すると再現する。
**エージェントはアプリを起動しない。**修正後の実機確認は利用者に依頼する。

## 8. 回帰確認 (利用者へ渡す手順に含める)

- JPEG / PNG / 静止 WebP / Animated WebP を同一枚数・同一寸法で比較
- 左綴じ / 右綴じ、左右キー、前進 / 後退、先読み枚数、サムネイルキャッシュ有無
- 複数ウィンドウモードを主対象に、フル機能ウィンドウと F12 detached を対照
