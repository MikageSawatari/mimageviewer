# ページ送りの速度差を切り分ける計装 (右 Ctrl / 元画像ホールド)

対象: 利用者報告 (2026-08-17)「左 Ctrl の左右ページ送りはどちらも速いが、右 Ctrl では
戻り方向だけ遅い」。**計装のみ。原因修正はしない。**

## 0. 今わかっていること (実測)

昨夜のログ (`perf_events.jsonl`、往復テストを含む t=144〜406 秒) を全件集計した結果:

| 指標 | 値 |
| --- | --- |
| `fs/original_preview_blocker_summary` 件数 | 91 |
| `checks` 累計 (ページ送り判定でこの地点を通った回数) | **55,817** |
| うちナビシーケンス進行中 | 1,055 |
| `original_preview_returns` | **0** |

つまり `fs_page_turn_ordinary_context_blocker`
([ui_fullscreen.rs:15183](../../src/ui_fullscreen.rs:15183)) で
`original_preview_active` が true になったことが**一度もない**。

→ 「元画像ホールドがページ送りの pass-through を止めている」も
「§1.91 の除外が効いていない」も、このログでは**支持されない**。

残る可能性は 2 つで、今の計装では区別できない:

1. この記録の間、右 Ctrl が実際には押されていなかった
2. 右 Ctrl は押されていたが、この地点で hold が検出されていない

`[fs-key]` の行は全て `Modifiers::NONE` だが、**FS ビューポートの egui modifiers は stale**
なので判定に使えない (`*_held_via_os` が存在する理由そのもの)。

### 0.1 今の probe が答えられない理由

`emit_fs_original_preview_blocker_probe` の `last_*` フィールドは
`summary.last_return` 由来で、**blocker が発火したときだけ**記録する
([ui_fullscreen.rs:15047](../../src/ui_fullscreen.rs:15047))。発火が 0 なので全部 null。

**調査対象の信号に記録条件を依存させてはならない** (keymap-spec.md に同じ原則がある。
この案件で 3 回目)。

## 1. やること — 既存イベントに 4 つの属性を足すだけ

新しいイベントは作らない。`fs/page_turn_decision`
([ui_fullscreen.rs](../../src/ui_fullscreen.rs) の `fs_page_turn_decision_for_frame` から
毎フレーム無条件で出ている) に以下を追加する。rate limit も候補判定も付けない。

| 属性 | 内容 |
| --- | --- |
| `original_preview_active` | 15368 で取っている memo 値をそのまま |
| `context_blocker` | `fs_page_turn_ordinary_context_blocker` の戻り値 (`None` なら null)。`reason` は burst 側の理由で別物なので混ぜない |
| `right_ctrl_held` | **OS 直読み・非消費**で右 Ctrl の押下 |
| `left_ctrl_held` | 同じく左 Ctrl (`ModKind::Ctrl`) |

- 左右を分けるのが要点。`ModKind::Ctrl` と `ModKind::RightCtrl` の 2 つを読む。
  `RightCtrl` の egui projection は `Ctrl` なので、**egui modifiers では区別できない**。
  `modifier_held_via_os` ([keymap.rs:6140](../../src/keymap.rs:6140) が使っている経路) を
  crate 内へ出すか、`Keymap` に非消費の薄い helper を足す。
- permit が取れないフレームは 2 つの `*_ctrl_held` を **null** にする (false と区別する)。
- `page_turn_decision` は既に毎フレーム出ているので、イベント数は増えない。

## 2. やらないこと

- 挙動を変えない。純粋な計装。`original_preview_active` の評価順 (memo を
  シーケンス生成前に焼く) も**今回は直さない** — 上の実測で、直しても表に出る変化が
  無いことがわかっている。原因が確定してから触る。
- 新しい rate limit / 候補判定を入れない。既存の `original_preview_blocker_summary` は
  そのまま残す (別の質問に答えている)。
- 時間窓で何かを判定しない。

## 3. テスト

- 属性 4 つが**常に存在する**こと (permit なしのフレームでは `*_ctrl_held` が null、
  それでも key が欠けない)。
- `context_blocker` が `original_preview` のときと null のときの両方を含むこと。
- mutation: 各属性の生成を削ると対応テストが落ちることを確認して報告する。

## 4. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
