# ブリーフ: 単指パン中に 2 本目が来たらピンチへ昇格させる

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.2 / §5.6 / §5.10。

前提 (完了・コミット済み): Step 0 `bb9574b2` / Step 1 `66cb2910` / Step 2 `6be3afd3` /
Step 3 `42aa037a` / Step 3c `cae9ffd6`。

---

## 1. 症状

実機報告 (2026-08-07、開発機に接続したタッチパネルディスプレイ):

> ピンチ操作がなかなか反応しないことがある。**ピンチして拡大できることもあれば、
> できないこともある**感じ。

## 2. 原因 (特定済み)

`TouchRecognizer::handle_start` ([src/touch_input.rs](../src/touch_input.rs)) は、
2 本目の接点が来たときに **`owner == Undecided` のときだけ**ピンチへ入る:

```rust
if self.contacts.len() >= PINCH_CONTACT_COUNT {
    match self.owner {
        TouchOwner::Undecided => { self.owner = TouchOwner::Pinch; ... }
        TouchOwner::Pinch => self.rebase_pinch(),
        _ => {}          // ← ここに落ちるとピンチにならない
    }
}
```

一方 `single_motion_command` は、1 本目が `TAP_MAX_DISTANCE_PT` (12pt) を超えて動いた時点で
`owner = TouchOwner::ViewerPointerPassthrough` に確定させる。

→ **1 本目が 12pt 動く前に 2 本目が着地したときしかピンチが成立しない。**

2 本の指は同時には着かない。先に触れた指が滑る / 着地しながら広げ始めると 12pt をすぐ超える。
UI 倍率 150% なら 12 論理ポイントは実測 18px 程度で、さらに超えやすい。
**「できたりできなかったり」はこの窓に入れたかどうかの差。**

## 3. 直すこと

**`ViewerPointerPassthrough` から `Pinch` への昇格を許可する。**

- 昇格時は `rebase_pinch()` して基準を現在位置から取り直す
  (**ズームが飛ばないようにするため。必須**)
- 昇格時に `suppress_primary = true` にする (`Undecided` からの遷移と同じ扱い)
- 昇格を**許さない** owner は次のとおり:
  - `WidgetPassthrough` — UI ボタン / パネルの上で始まったジェスチャは egui へ委譲したまま
  - `Cancelled` — 破棄済み
  - `ViewerTapZone` — タップとして確定済み (release 済みなので実際には接点が残らない)
- `EdgeSwipe` は現状 Step 3 では未配線 (`OpenSidePanel` は無視されている) なので、
  **昇格させてよい**。ただし Step 3b で配線したときに再検討が要る旨をコメントに残すこと

## 4. plan の規約との関係 (必ず読むこと)

plan §5.2 「所有権のライフタイム」に次の記述がある:

> 一度 pan / pinch / scroll に確定した stream は、**全接点が離れるまで別 action へ移さない**

字面どおりだと今回の昇格は違反に見える。しかしこの規約の意図は
**同じ接点の解釈が行ったり来たりするのを防ぐこと**であり、
**新しい接点が増えたときの昇格は別の話**である。厳密に読むと §2 のバグがそのまま仕様になる。

**したがって規約側を明確化する。** [docs/touch-support-plan.md](touch-support-plan.md) の
§5.2 と §5.10 に、次の趣旨を追記すること:

- 確定済みの解釈を**同じ接点集合のまま**別の action へ移さない、というのが規約の意味
- **接点が増えたときに限り、単指パンからピンチへ昇格してよい**。昇格時は基準を取り直す
- 逆方向 (ピンチ → 単指パン) の降格は**しない**。
  2 本目が離れても、全接点が離れるまで `Pinch` を保持する (既存の挙動を維持)

## 5. テスト

`src/touch_input.rs` の `mod tests` に追加:

- **1 本目が 12pt を大きく超えて動いた後に 2 本目が着地 → `Pinch` になり、Zoom が出る**
  (これが今回の回帰テスト本体)
- 昇格直後の 1 サンプル目で**巨大な zoom factor が出ないこと** (`rebase_pinch` の確認)
- `WidgetPassthrough` から始まった場合は 2 本目が来ても昇格しないこと
- 昇格後に 2 本目が離れても `Pinch` のまま (降格しないこと)
- 昇格後の全接点解放後、次の Start で状態がリセットされること
- 既存のピンチ関連テストが**すべてそのまま通ること**

## 6. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4880 件)
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- plan の §5.2 / §5.10 を §4 のとおり更新すること

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **範囲を広げないこと。** しきい値 (12pt) の変更や、ピンチ以外のジェスチャ追加はしない。
  今回は昇格経路だけを直す
- detached-rework 凍結ルールは有効

完了したら、変更内容・**昇格を許す / 許さない owner の判断**・テスト結果・
plan の更新箇所を報告すること。
