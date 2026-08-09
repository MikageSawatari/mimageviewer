# ブリーフ: Phase 3 Step 3g — 前面奪還中もタッチのジェスチャを通す (ドラッグの取りこぼし)

対象: v2.13.0 タッチ対応 Phase 3。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.14-11 / §5.15。
前回の関連ブリーフ: [docs/brief-touch-foreground-reclaim-taps.md](brief-touch-foreground-reclaim-taps.md)。

前提 (コミット済み、`50730055` まで): Phase 3 Step 3f まで。

⚠ **作業ツリーに ClaudeCode の一時診断ログが未コミットで載っている**
(`[TOUCH-DEBUG] continuous-drag` / `continuous-drag-apply`、`src/ui_fullscreen.rs`)。
**この Step で撤去すること** (§6)。

---

## 1. 症状と、確定した原因

実機報告 (2026-08-09):

> 起動直後は 1 本指でドラッグできません。そしてタップするたびにアニメーションが起きます。
> 一度別ウィンドウにした後は起きません。その状態でもタップ操作ではパンできません。
> **マウスで再度 mIV をクリックしてアクティブにすると、今度はパン操作できるようになりました。**

`MIV_TOUCH_DEBUG=1` のログで原因は確定済み。**推測は残っていない。**

### 1.1 ログの証拠

```
[fs-focus] foreground=0x1521bca fullscreen=0x11218
           current_foreign=true suppress=true native_claim=true set_foreground=true
```

- mIV は**前面ウィンドウではない** (`foreground` が別プロセスの窓)。
  19 件の `fs-focus` のうち **16 件が `current_foreign=true`**
- タッチのたびに前面奪還を試み、そのたびに抑止が armed される

```
continuous-drag frame=2482 owner=ViewerPointerPassthrough suppress_response=false
  response_dragged=true total_drag_delta=Some([-611.6 102.4]) pointer_primary_down=true
  cursor_in_panel=false touch_input_enabled=true suppress_this_frame=true
```

- 認識器の owner、応答のドラッグ、移動量、いずれも**正常**
- `suppress_response=false` = タッチ相関側の抑止では**ない**
- **`suppress_this_frame=true`** — これだけで左ボタン系の分岐が丸ごと飛ぶ

マウスでアクティブ化した後は `suppress_this_frame=false` になり、
`fs_vertical_scroll 0.0->6.2->14.3->…->296.8` と 1:1 で正常にスクロールしている。
**Step 3f の実装自体は正しい。**

### 1.2 これは前回の修正の直し残し

`suppress_this_frame` の出どころは
`FullscreenPrimarySuppression::PointerStream`。フォーカス復帰の猶予中に primary が
押されると `arm_pointer_stream()` で armed される
([ui_fullscreen.rs:16703 付近](../src/ui_fullscreen.rs))。

2026-08-08 の「前面復帰中にタッチが全部捨てられる」修正で直したのは
**コマンド経路** (`touch_input_enabled` の観測 / 実行の分離) だけだった。当時のブリーフは
widget への合成 primary press を「通さない」と明記しており、**応答駆動の経路は
抑止されたまま残っていた**。

結果として生まれている非対称:

| 経路 | ゲート | 前面奪還中 |
| --- | --- | --- |
| タップ (`PageSide` / `ToggleChrome`) | `touch_input_enabled` のみ | **通る** (前回修正済み) |
| 応答駆動のドラッグ (連結スクロール / ズームパン) | `suppress_this_frame` | **死ぬ** ← 今回 |

**キャンバスのドラッグはコントロール押下ではなくジェスチャ**であり、
前回「通す」と決めた中央タップと同じ分類である。

---

## 2. 直し方

**前面奪還の抑止を、相関済みタッチ stream のキャンバスジェスチャに適用しないこと。**

plan §5.14-11 の判断を 1 層深く適用する:

| 前面奪還中の入力 | 扱い |
| --- | --- |
| **マウスの復帰クリック** | **従来どおり食べる**。マウスには「1 回目で復帰、2 回目から操作」がある |
| **タッチのタップ** | **通す** (前回修正済み。維持する) |
| **タッチのキャンバスドラッグ** (連結スクロール / ズームパン) | **通す** ← 今回追加 |
| **タッチから overlay の control へ届く合成 primary press** | **通さない** (維持) |

### 2.1 実装の方向

- **`FullscreenPrimarySuppression` の owner モデルを壊さないこと。** Idle / PointerStream /
  TouchStream の 3 状態と、各 owner が自分で Idle へ戻れる性質を保つ
- 問題は「マウス由来として arm した `PointerStream` が、実際にはタッチ由来だった」こと。
  arm 地点 ([16703 付近](../src/ui_fullscreen.rs)) は `touch_frame` より前なので、
  その時点では入力源が分からない。**相関が確定した時点で正しい owner へ訂正する**のが素直
- **新しい bool を足さないこと。** 足したくなったら plan §5.10 を読み直す
- **`fs_focus_regained_at` と前面奪還そのものには触れないこと** (§4)

### 2.2 ⚠ 「復帰タップがコントロールを押す」は防いだままにする

前回の判断を後退させないこと。**overlay の control へ届く合成 primary press は
引き続き抑止する。**通すのは**キャンバス上のジェスチャだけ**である。
この 2 つを分けているのが既存の相関層 (`should_suppress_response`) なので、
そちらの意味は変えない (今回のログでも `suppress_response=false` = 正しく働いている)。

---

## 3. マウス無影響 (§5.15)

- **マウスの復帰クリックが操作に使われない既存挙動を維持すること。**
  別アプリから戻る 1 回目のクリックでページが飛んだり、パンが始まったりしてはならない
- 連結読み中のマウスクリックページ送り抑制も維持 (Step 3f のまま)
- `MIV_DISABLE_TOUCH_GESTURES=1` で現行挙動へ戻ること

## 4. ⚠ 触らないこと — 前面奪還そのもの

ログは、**mIV が前面でない間、タッチのたびに `SetForegroundWindow` を試みて失敗している**
ことも示している (`native_claim=true set_foreground=true` かつ `current_foreign=true` が継続、
利用者には別ウィンドウのちらつきとして見える)。

- これは focus 機構の話で **detached-rework の凍結ルールの範囲**である
- **今回は直さないこと。** 抑止のスコープだけを直す
- 観測した事実を [docs/detached-rework-plan.md](detached-rework-plan.md) に
  **報告として記録**すること (症状パッチを入れない)

---

## 5. ⚠ 先に読むこと

- [docs/brief-touch-foreground-reclaim-taps.md](brief-touch-foreground-reclaim-taps.md) —
  今回はその続きである
- plan §5.10「実機で 3 回踏んだ同型バグ」— 抑止状態を触るので必読
- plan §5.5「パネル外タップ後の primary 抑制 ownership 修正」—
  `FullscreenPrimarySuppression` を導入した経緯

## 6. 一時診断の撤去

ClaudeCode が原因特定のために入れた次の 2 つを**削除すること**:

- `[TOUCH-DEBUG] continuous-drag …` (分岐の入力値を 1 行に出すブロック)
- `[TOUCH-DEBUG] continuous-drag-apply …` (分岐本体の着地ログ。
  `if touch_debug_enabled()` で `scroll_vertical_reading_by` を 2 回書いている形も戻す)

**代わりに §7 の回帰テストを入れる。** 診断は原因が分かるまでの足場であり、
恒久的な計装として残す価値は無い (テストのほうが強い)。

## 7. テスト

- **前面奪還の抑止が armed の状態で、相関済みタッチのドラッグが**
  - 連結読みをスクロールすること
  - ズーム中の画像をパンすること
- 同じ状態で**タッチのタップが従来どおり通る**こと (前回修正の回帰)
- 同じ状態で **overlay の control へ合成 primary press が届かない**こと (維持)
- **マウスの復帰クリックが操作に使われない**こと
- 抑止の終端 (release / touch completion / cancel) で owner が Idle へ戻ること。
  ドラッグを跨いでも状態が残らないこと

## 8. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4980 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること (更新不要のはず)
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **ドキュメント更新**:
  - [docs/touch-support-plan.md](touch-support-plan.md) に、前回修正が
    **コマンド経路しか直していなかった**こと、応答駆動経路も同じ規則で扱うこと、
    control への合成 press は抑止したままであることを記録する
  - [docs/detached-rework-plan.md](detached-rework-plan.md) に §4 の観測を記録する

## 9. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで作業する
- **範囲を広げないこと**。前面奪還ロジック、相関層の `should_suppress_response` の意味、
  連結読みの移動量計算 (Step 3f) には手を出さない

---

完了したら次を報告すること:

1. 入力源の訂正をどこで行ったか (owner モデルを壊していないこと)
2. control への合成 press が引き続き抑止されることの根拠
3. マウスの復帰クリックが不変であることの根拠
4. 一時診断を撤去したこと
5. テスト結果
6. **実機で確認してほしいこと**
