# ブリーフ: 左右パネルへの導線 (タッチハンドル + エッジスワイプ)

対象: v2.13.0 Step 3b。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.5 / §5.7 / §6-4。

前提 (完了・コミット済み・実機確認済み): Phase 1 一式 (`bb9574b2` 〜 `9f64f5d3`)、
Phase 2 の一覧スクロール・ピンチ列数変更。

**範囲は静止画フルスクリーン (egui 側) のみ**。動画 native は Phase 3、
初回オーバーレイヘルプは次ステップに分ける。

---

## 1. 直す症状

実機報告 (2026-08-06、Phase 1 の最初の確認時):

> ｉボタンでクリックで開く状態にしても、**左右端をうまくおさないとパネルを開けない**。

plan §6-4 が推測していた原因が実機で裏付けられた形になる:

呼び出し callout は **端にホバー位置がある間だけ描かれる**
([ui_fullscreen.rs:5628-5639](../src/ui_fullscreen.rs))。

```rust
let pointer = ctx.input(|i| if self.cursor_hidden { None } else { i.pointer.hover_pos() });
...
if !crate::ui_helpers::callout_hit(edge_rect, pointer) { continue; }
```

タッチでは **Touch End と同じ batch で `PointerGone` が来る**ため、押した瞬間に見えていた
callout が release フレームで消え、**click completion に到達しない**。
さらに表示幅は 20pt、hit 幅も `PANEL_CALLOUT_HIT_PX` しかなく、§5.7 のタッチ基準に届かない。

---

## 2. 対応 (1) — タッチクローム中の専用ハンドル

§5.5 のとおり、中央タップで出るクロームには**左右のパネル呼び出しハンドル**が含まれる。

> 中央タップ → クローム表示 (上下バー + 左右のパネルハンドル) → ハンドルをタップ → パネルが開く

### 2.1 既存 callout に手を入れず、別要素として描く

**マウス用の hover callout は一切変更しないこと。** タッチハンドルは
**タッチクロームがラッチされている間だけ描く別の要素**にする。ラッチはタッチ操作中しか
立たないので、この分離だけで「マウス無影響」(§5.15) が構造的に保証される。

- 既存 latch は `still_touch_chrome_latched` / `StillTouchChromeLatch`
  ([ui_fullscreen.rs:1118-1160](../src/ui_fullscreen.rs))
- **ホバー位置に依存させないこと**。これが症状の根本原因なので、
  `hover_pos()` を条件に入れない
- **`FsSidePanelMode` で出し分けないこと**。§5.5 の表のとおり `Hover` / `ClickToShow`
  どちらでもタッチクローム中は明示ハンドルを出す。既存 callout は
  `ClickToShow` のときだけ描かれる ([ui_fullscreen.rs:5620-5623](../src/ui_fullscreen.rs)) が、
  **タッチハンドルはこの早期 return より前に処理する**

### 2.2 サイズ

§5.7 の実測表より、タッチターゲットは **最低 40pt** を狙う。

- 幅は **44pt 以上** (現行の表示 20pt / hit `PANEL_CALLOUT_HIT_PX` より明確に大きく)
- 高さは既存 `panel_callout_bar_rect` と同じ考え方 (画面高の比率 + クランプ) でよい
- **矢印の意味と向きは既存 callout と揃える** (`draw_panel_callout_arrow`、開いていれば逆向き)
- サイズは **1 か所の定数**にして、根拠を doc comment に書く (実機で調整しうる)

### 2.3 動作

- 左ハンドル → 左パネル (`adjustment_mode`)、右ハンドル → 右パネル
  (`toggle_fullscreen_click_info_open`)。**既存 callout と同じ action へ合流**する。
  新しい開閉経路を作らないこと
- 左パネルを閉じるときの `persist_pending_view_trim_state()` も既存どおり呼ぶ
- **ハンドル矩形を、その frame の `TapZoneGeometry.excluded` に入れる**こと。
  入れないとハンドルへのタップが `PageSide` / `Center` と判定され、
  ページ送りやクロームトグルに化ける
- ハンドル上で始まったタッチは `WidgetPassthrough` になるので、
  既存の primary emulation でそのまま `clicked()` が成立するはず。
  **成立しない場合は原因を報告すること** (§6-4 の残りがここに出る)

### 2.4 ⚠ 上下バーとの重なり

ハンドルは画面中央高さに出るので上下バーとは通常重ならないが、
**縦が短いウィンドウでは重なりうる**。重なる場合は上下バーを優先し、
ハンドルを縮めるか位置をずらすこと。**両方に当たり判定を持たせないこと**。

---

## 3. 対応 (2) — エッジスワイプの配線

認識器は既に **`OpenSidePanel { left }` を出している**
([touch_input.rs:466-473](../src/touch_input.rs))。消費側が捨てているだけ:

- [ui_fullscreen.rs:16224](../src/ui_fullscreen.rs) と
  [ui_fullscreen.rs:16267](../src/ui_fullscreen.rs) が `=> {}`
- [ui_main.rs:15112](../src/ui_main.rs) も `=> {}` (**一覧では今後も無視でよい**)

### 3.1 静止画フルスクリーンで配線する

`OpenSidePanel { left }` を §2.3 と**同じ action** へ流す。
`left` は**物理的な画面左右**であって読み方向ではない (パネルは画面固定)。
読み方向で反転させないこと。

### 3.2 ⚠ ズーム中はパンを優先する (§5.5)

> ズーム中はキャンバスの pan を優先し、パネルは中央タップ → ハンドルから開く。

拡大表示中は端からのドラッグでパンしたいので、**エッジスワイプを成立させない**。

- **消費側で `OpenSidePanel` を捨てる形にしないこと。** それだと認識器が既に
  `EdgeSwipe` に確定してパンを失っている
- **認識器へ伝える**: `TouchSurfaceBehavior::Viewer` に
  `accepts_edge_swipe: bool` 相当を足し、ズーム中は false にする。
  `accepts_pinch` と同じ形にすること
- 判定の入力は「現在キャンバスがズームされているか」。等倍/フィット時は true

### 3.3 ⚠ `EdgeSwipe` からピンチへ昇格させないこと (§5.10 の宿題)

現在 [touch_input.rs:305-312](../src/touch_input.rs) は、確定済みの `EdgeSwipe` でも
2 本目の接点でピンチへ昇格する:

```rust
TouchOwner::EdgeSwipe { .. } => {
    // Step 3 does not dispatch OpenSidePanel yet, so an added
    // contact may still claim pinch. Revisit this ownership
    // choice when Step 3b wires the edge-swipe action.
    self.begin_pinch();
}
```

**今回でこの宿題を片付ける。** `OpenSidePanel` は**確定と同時に発火する**ので、
その後ピンチへ移すと「確定済み owner を同じ接点集合のまま別 action へ移す」ことになり、
§5.10 の規約に反する (パネルが開いた直後にズームが始まる)。

→ **`EdgeSwipe` からは昇格しない** (`WidgetPassthrough` / `ViewerTapZone` / `Cancelled`
と同じ扱い) に変更し、コメントを「Step 3b で決着」に更新すること。

### 3.4 下端・上端のエッジスワイプは入れない

利用者の実機確認 (2026-08-07):

- **下から上**: Windows のスタートメニューが開く。**OS 予約なので使えない**
- **上から下**: 何も起きなかった

**左右のエッジスワイプだけ**を対象にする。認識器の `edge_side` は既に左右のみ。

---

## 4. 入れないもの

- **初回オーバーレイヘルプ** (§5.5) — 次ステップ
- **動画 native / 音楽ビューのハンドル** — Phase 3
- パネル内コントロールの hit resolver (★ 行 5 等分・タグボタン等、§5.7-3) — 別ステップ
- 上下端のエッジスワイプ
- 右上の常時メニューアイコン (§5.5 で不採用)

---

## 5. マウス無影響 (§5.15)

- **既存の hover callout が従来どおり**であること (表示条件・サイズ・位置・矢印・tooltip)
- `Hover` / `ClickToShow` の既存挙動が変わらないこと
- 端へのマウス移動でパネルが出る / 出ないの条件が変わらないこと
- `MIV_DISABLE_TOUCH_GESTURES=1` で現行挙動へ戻ること

---

## 6. テスト

**純関数 unit** で:

- タッチハンドルの矩形計算 (幅 ≥ 44pt / 画面高に対する高さ / 左右対称 /
  極端に低いウィンドウで上下バーと重ならないこと)
- ハンドル矩形が `excluded` に入ること (= ハンドル上のタップが `Center` / `PageSide`
  と判定されないこと)
- **エッジスワイプがズーム中は成立しないこと** (`accepts_edge_swipe: false` で
  `OpenSidePanel` が出ず、`ViewerPointerPassthrough` になる)
- **`EdgeSwipe` 確定後に 2 本目が来てもピンチへ昇格しないこと**
- `OpenSidePanel { left: true / false }` が左右それぞれのパネル action に写像されること
  (読み方向で反転しないこと)

**kittest snapshot** で:

- タッチクロームラッチ ON のときにハンドルが描かれること (左右とも)
- ラッチ OFF では描かれないこと
- 既存 callout のスナップショットが変化しないこと

スナップショット更新時は `UPDATE_SNAPSHOTS=1` で再生成し、
**PNG を目視確認してからコミットすること** ([docs/ui-snapshot-policy.md](ui-snapshot-policy.md))。

---

## 7. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4934 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- 非 Windows を壊さないこと
- **[docs/touch-support-plan.md](touch-support-plan.md) を更新**すること:
  - §5.5 / §5.7 にハンドルの実装記録 (Step 3c の記録と同じ粒度で)
  - §5.10 の `EdgeSwipe` 昇格の宿題が決着したこと
  - §6-4 (callout がタッチで押せるか) の結論
- **マニュアル** ([htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/)) は、
  タッチ操作の説明をまだ入れていないなら**今回も入れない** (Phase 3 完了後にまとめる)。
  既に入れている場合は整合させること。判断した結果を報告すること

---

## 8. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **診断ログは残すこと**
- detached-rework 凍結ルールは有効
- **範囲を広げないこと**

---

完了したら次を報告すること:

1. **ハンドルの矩形定数と、上下バーと重なるときの扱い**
2. **`excluded` へどう入れたか**
3. **ズーム中の抑止を認識器へどう伝えたか**
4. **`EdgeSwipe` 昇格をどう決着させたか**
5. §6-4 の結論 (ハンドルのタップが click completion に到達したか。
   到達しない場合は原因)
6. テスト結果 (スナップショットを更新した場合はその理由)
7. **実機で確認してほしいこと**の一覧
