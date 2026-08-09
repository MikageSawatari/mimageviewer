# ブリーフ: タッチ対応 Phase 2 — 一覧の指スクロール (anchor + fraction)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md)。
**着手前に §5.4 全体、§5.10、§5.15 を読むこと。**
仮想スクロール / prefetch に触るので [docs/ui-responsiveness.md](ui-responsiveness.md) §4 も読む。

前提 (すべて完了・コミット済み・実機確認済み): Step 0 `bb9574b2` / Step 1 `66cb2910` /
Step 2 `6be3afd3` / Step 3 `42aa037a` / Step 3c `cae9ffd6` / ピンチ昇格 `21ef4643` /
1 フレーム 1 回 `04f71c12` / 相関診断 `b22e7d00` / pointer ミラー `961bb47f`。

---

## 0. これは何か

**一覧を指で直接スクロールできるようにする。** 利用者評:

> あとはグリッド表示時にスクロールができるようになれば、ある程度の操作はできそう。

現状 `process_scroll` ([src/app.rs:33489](../src/app.rs)) はホイール専用で、タッチでは一覧が動かない。
細いスクロールバーを指で掴むしかない。

## 1. 難しさ — 行スナップの不変条件を壊さないこと

mIV は `scroll_offset_y` が**常に `cell_size` の整数倍**であることを前提にしている
(CLAUDE.md「Virtual scrolling」/「Row snapping」)。仮想表示・サムネイル保持・eviction が
この前提に乗っており、**読み手が多い**。ドラッグ中だけ自由値にするのは避ける。

plan §5.4 の方式を使う:

```
正本    : scroll_offset_y   = 行境界にスナップされた anchor  (従来どおり不変条件を維持)
一時状態: fractional_drag_y = 0 .. cell_h 未満
描画位置: scroll_offset_y + fractional_drag_y
```

- ドラッグが 1 行を越えるたびに `scroll_offset_y` を 1 行進め、`fractional_drag_y` から 1 行分を引く
- これで **`scroll_offset_y` は常に行境界のまま**になり、既存の読み手を触らずに済む
- **指を離したら端数を最寄りの行へ確定する**

## 2. 付随して必要な改修 (plan §5.4 が名指ししている。省略しないこと)

### 2.1 端数表示中は可視範囲の末尾を 1 行多く保持する

でないと**下端が欠ける** (端数ぶんだけ次の行が覗くため)。

### 2.2 スクロール中であることを明示的に通知する

`detect_scroll_input_intent` ([src/app/runtime_ops.rs:16](../src/app/runtime_ops.rs)) は
ホイールとキーしか見ておらず、**タッチは `update_scroll_settle_state` の offset 変化 fallback
頼み**になっている (同ファイルのコメントにもそう書いてある)。

**端数だけ動いたフレームは `scroll_offset_y` が変わらないので、この fallback では検出できない。**

→ touch move ごとに次を更新し、**指を離すまで scroll settle を発火させない**こと:

- `last_prefetch_scroll_at`
- `last_scroll_event_at`
- idle-upgrade の時刻

これを怠ると、スクロール中に prefetch 抑制が効かず、PDF 等で「スクロール停止後に可視サムネが
数秒出ない」既知の症状 (CLAUDE.md「スクロール中の prefetch 抑制」) を踏む。

### 2.3 タッチ由来のドラッグでは native ファイル D&D を無効にする

セル自身が primary ドラッグを file D&D として奪う
([src/ui_main.rs](../src/ui_main.rs) の `response.drag_started_by(egui::PointerButton::Primary)`、12327 付近)。
これが生きているとスクロールにならない。

- **タッチ由来と確定したストリームでのみ D&D を抑止する**
- **マウスの D&D は一切変えない** (plan §5.15)
- plan §5.10 のとおり、しきい値の競争にしない。**タッチなら D&D 自体を無効にする**方が確実

> ペンもタッチ扱いなので D&D できなくなる。これは §5.16 で受け入れ済みの唯一の代償。

## 3. 認識器側 (`src/touch_input.rs`)

現在の `TouchOwner` に一覧スクロールの概念が無い。plan §5.10 の `GridScroll` を足す。

- `TouchOwner::GridScroll` を追加
- `TouchCommand::ScrollGrid { delta_y: f32 }` を追加 (**縦のみ**。横スクロールは対象外)
- 一覧 surface で単指ドラッグが tap しきい値を越えたら `GridScroll` に確定する
- **一度 `GridScroll` に確定したら、全接点が離れるまで別 action へ移さない**
  (`21ef4643` で明確化した規約。ただし接点追加時の昇格の扱いは §3.1 参照)
- **指を離したときに「端数を確定する」ことを呼び出し側へ伝えられること**
  (`ScrollGrid` の終了を表す手段。`PinchEnd` と同じ形でよい)

### 3.1 一覧での 2 本目の扱い

**一覧にピンチズームは無い** (セルサイズ変更は Ctrl+ホイール / 既存 UI の担当で、今回の対象外)。

→ `GridScroll` 中に 2 本目が来ても **`Pinch` へ昇格させないこと**。
`ViewerPointerPassthrough` → `Pinch` の昇格 (`21ef4643`) は**フルスクリーンのための規約**である。
surface ごとにピンチを受け付けるかが違うので、**認識器がそれを知る必要がある**。
`TapZoneGeometry` か `handle_sample` の引数に「この surface はピンチを受け付けるか」を
渡す形にすること (グローバル state にしない)。

## 4. 入れないもの

- **慣性は入れない** (plan §5.4 が明示)。「指に追従して動く + 離したら行スナップ」だけで足りる。
  無制限の物理スクロールは PDF 先読み・eviction・idle upgrade との調整コストに見合わない。
  実機評価後に必要なら、速度から最終到達行を決める限定形だけを別途検討する
- 横スクロール / 一覧のピンチによるセルサイズ変更
- **選択済みセルの再タップ open** (plan §5.8) — 同じ Phase 2 だが別ステップにする。
  スクロールと選択の相互作用を一度に入れると切り分けできない
- ツールバー / アドレスバー / ファセットバーのタッチ最適化

## 5. マウス無影響 (plan §5.15)

回帰テストで固定すること:

- ホイールスクロールが従来どおり (1 ノッチ 1 行、行スナップ維持)
- **マウスのファイル D&D が従来どおり動く**
- マウスのセル選択 / ダブルクリック open が従来どおり
- スクロールバーのドラッグが従来どおり
- `MIV_DISABLE_TOUCH_GESTURES=1` で完全に現行挙動へ戻る

## 6. テスト

**plan §5.12 のテスト方針どおり、計算部分は純関数 unit にすること**:

- **anchor / remainder 計算**: ドラッグ量から `scroll_offset_y` と `fractional_drag_y` を
  求める関数を純関数にし、行を跨ぐ / 跨がない / 複数行を一気に跨ぐ / 負方向 を固定する
- **`scroll_offset_y` が常に `cell_size` の整数倍である**ことを、上記すべてのケースで表明する
  (これが今回の最重要の不変条件)
- 端数表示時の可視範囲 / keep 範囲が 1 行多いこと
- 上端 / 下端でのクランプ (行き過ぎない、跳ねない)
- 指を離したときに最寄り行へ確定すること
- `GridScroll` 中に 2 本目が来ても `Pinch` にならないこと (§3.1)
- 認識器の状態遷移 (tap → GridScroll の確定条件)

## 7. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4887 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- 非 Windows を壊さないこと
- **`docs/spec.md` とマニュアル ([htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/)) を更新すること**。
  マニュアルは CLAUDE.md「マニュアル・製品ページの記述方針」に従う
  (バージョンタグを書かない / 内部用語を書かない)

## 8. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **`App` へのフィールド追加は最小限にする。** タッチ固有の一時状態は `ctx.data_temp` を優先する
  (plan §5.1-5)。`fractional_drag_y` のように**描画が毎フレーム読む値**は `App` でもよいが、
  その場合は**どちらにしたか理由を報告すること**
- **診断ログ (`b22e7d00`) は残すこと**
- detached-rework 凍結ルールは有効
- **範囲を広げないこと** (§4)

完了したら、変更内容・**行スナップ不変条件をどう保ったか**・**スクロール通知をどこに入れたか**・
**D&D 抑止をタッチ限定にした方法**・テスト結果を報告すること。
plan と食い違う判断をした箇所があれば理由も明記すること。
