# §1.88 — 見開き 1 ページずらし後にページ送りが停止する

対象: [next-release-backlog.md](../next-release-backlog.md) §1.88。**P1、出荷済みの退行**
(v3.0.0 / v3.1.0 で再現、v2.9.1 では出ない)。利用者報告 (専用スレ >>239, 2026-08-16)。

関連: [display-pipeline.md](../display-pipeline.md) §2.5.4 の atomic display-unit 契約。

## 0. 症状と再現

1. フルスクリーンで右綴じの見開き (テンキー 4 / 5) にし、
   `見開き表示を右方向へ1ページずらす` (既定 Ctrl+Right) を実行する
   (または左綴じ (2 / 3) で左方向へずらす)。
2. その後、カーソルキーによるページ移動を受け付けなくなる。
   連打 / hold で起きやすいが 1 回でも起きる。左右どちらの Ctrl でも起きる。
3. Backspace でフルスクリーンを閉じると復旧する
   (= sequence がフルスクリーン終了で破棄されるため)。

## 1. 根本原因 — 描画元を texture id の包含から**推測**している

見開き 1 ページずらしは、旧 unit `[2, 3]` から新 unit `[3, 4]` のように、
**移動前後の表示単位で 1 ページを意図的に共有する**。ここで壊れる。

### 1.1 誤判定の実体 (2 箇所が同じ推測を共有している)

`observe_fs_navigation_sequence_presented` ([ui_fullscreen.rs](../../src/ui_fullscreen.rs)):

```rust
let captured_page_visible = trace_pages.iter().any(|page| {
    self.fs_holdover_tex
        .as_ref()
        .and_then(|holdover| holdover.page_for_texture_id(page.texture_id))
        .is_some()
});
...
if !captured_page_visible && presented_pages == target.pages {
    self.fs_nav_locked_gen = None;
    self.fs_holdover_tex = None;   // ← ここへ到達しない
}
```

`fs_texture_source_for_trace` ([ui_fullscreen.rs](../../src/ui_fullscreen.rs)) も**先頭で同じ判定**をする:

```rust
if self.fs_holdover_tex.as_ref()
    .is_some_and(|holdover| holdover.contains_texture_id(texture_id))
{
    return "holdover";      // ← live の thumbnail / composite / edit 判定より前
}
```

共有ページ (上の例の 3) は **target の live source として正しく描かれても**、
同じ texture id が previous holdover unit に含まれているため:

- `source` ラベルが `"holdover"` になる (誤り)
- `captured_page_visible = true` になる (誤り)
- sequence が `Presenting` のまま残る
- `blocks_new_target()` が後続ナビを拒否し続ける

**`FsDisplayUnitTracePage` に `source: &'static str` は既にあるが、その値自身が同じ推測から
導かれているので、これを使っても直らない。** 描画元の事実を別に通す必要がある。

### 1.2 当初疑ったが原因ではないもの

右 Ctrl 既定の `押している間だけ元画像を表示する` との競合は根本原因ではない。
右 Ctrl は texture 選択のタイミングへ影響し得るが、**左 Ctrl でも同じ ownership 誤判定が
成立する**。ここへ手を出さないこと。

## 2. 直し方 — provenance を選択元から運ぶ

**texture id の包含から描画所有元を推測しない。** 事実は選んだ場所にある。

- `FsNavigatorTextureSources` は現在 `pages: Vec<(usize, FullscreenPaintResource)>` で、
  **どこから取った resource かを捨てている**。構築側 (holdover unit から取ったのか、
  live に解決したのか) はその時点で知っている。
- そこで typed な provenance を per-page で持たせ、`FsDisplayUnitTracePage` まで通す。
- `observe_fs_navigation_sequence_presented` は
  **「完全な target page set が live source として描かれた frame」**で解放する。
- `fs_texture_source_for_trace` の `"holdover"` ラベルも同じ事実から決める
  (包含テストを先頭に置かない)。

`&'static str` を増やすのではなく **typed enum** にすること
(`Live` / `Holdover` のような)。`source: &'static str` は perf ログ用の表示名なので、
判定に使う provenance とは別の型にし、表示名はそこから導く。

### 2.1 弱めてはいけない契約 ⚠️

[display-pipeline.md](../display-pipeline.md) §2.5.4 の atomic display-unit 契約は維持する。
**条件緩和で直さないこと**:

- previous overlay を**本当に描いている**間は解放しない
- 見開きの**片側だけ** target が揃った間は解放しない
- target page set が**不完全**な間は解放しない
- 「1 枚でも live なら解放」のような緩和は不可 (ちらつきが戻る)

解放の条件は「**target の全ページが live source として描かれた**」であって、
「holdover が見えていない」の否定形で近似しない。

### 2.2 併せて確認すること

`spread_shift_anchor_idx` の更新と previous unit capture の**順序**を確認し、
遷移前 unit と遷移後 unit の所有者を同じ mutable pairing から取り違えていないことを固定する。
ここに問題があれば報告すること (見つからなければ「確認した」と報告に書く)。

## 3. 回帰テスト (backlog 指定。全部入れる)

1. previous `[2, 3]` / target `[3, 4]` で **3 の texture id が同一でも**、
   3 と 4 が target の live source として完全に描かれたら sequence が解放される。
2. 同じ page set でも、**共有ページを previous holdover から実際に描いた**場合は解放しない。
3. 左綴じ / 右綴じ、左右へのずらし、単発 / repeat を確認する
   (左 Ctrl / 右 Ctrl の別は入力層の話なので、ここでは keymap 経路のテストで足りる)。
4. 既存の disjoint な通常ページ送り / 片側だけ ready / 不完全 target /
   旧 texture の index 再利用を検出するテストを**維持する**。

**4 が緑のままであることが、契約を緩めていないことの担保**になる。赤くなったら
条件を緩めた疑いがあるので、直す前に報告すること。

## 4. 触ってよいファイル

- `src/ui_fullscreen.rs` (trace / provenance / observe / sources)
- `src/app.rs` (`FsHoldover` / `FsNavigationSequence` 周辺。**必要最小限**)
- `src/app/tests.rs` または `src/ui_fullscreen.rs` の test module (回帰テスト)
- `docs/next-release-backlog.md` / `docs/display-pipeline.md` (契約の記述を変えたときのみ)

`vendor/` には触れない。§1.31 / §4.2 の作業に手を出さない。

## 5. やらないこと

- **条件緩和で直さない** (§2.1)。
- 右 Ctrl の `押している間だけ元画像を表示する` に手を出さない (§1.2)。
- 症状パッチ (guard / delay / retry / 追加 repaint / 一括 reset / silent fallback) を
  根本原因の代わりに入れない。**特に「一定時間で sequence を強制解放する」は不可**
  (時間窓で競合を吸収しない)。
- `blocks_new_target()` の呼び出し側に「例外的に通す」条件を足さない。
  直すのは sequence が解放されないことであって、拒否を迂回することではない。

## 6. 凍結ルール

[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象
(フルスクリーン表示経路 = viewport / paint に触れる)。着手前に §2 を読むこと。
憲法 5 (時間窓で競合を吸収しない) が特に効く。
完了時に §11 (リワーク外からの変更記録) へ追記する。

## 7. 完了条件

1. 描画元 provenance が選択元から `FsDisplayUnitTracePage` まで typed に通っている。
   `contains_texture_id` / `page_for_texture_id` による**推測が判定から消えている**。
2. `observe_fs_navigation_sequence_presented` の解放条件が
   「target 全ページが live source として描かれた」になっている。
3. §3 の回帰テスト 1〜3 が入り、通る。
4. §3 の 4 (既存テスト群) が**無修正で**通る。赤くなったら報告 (§2.1)。
5. `cargo fmt --check` が通り、`.\scripts\test-full.ps1` が exit 0。
6. `docs/detached-rework-plan.md` §11 に記録。
7. `docs/next-release-backlog.md` §1.88 を完了に更新。
8. §2.2 の `spread_shift_anchor_idx` 順序確認の結果を報告に書く。

## 8. 実機確認 (利用者が後で行う。手順を報告に書くこと)

- 右綴じ見開き (テンキー 4 / 5) → Ctrl+Right でずらす → カーソルキーでページ送りが続く
- 左綴じ見開き (2 / 3) → Ctrl+Left でずらす → 同上
- 連打 / hold でも詰まらない
- 通常のページ送り、フォルダ跨ぎ、連結読みでちらつきが増えていない
