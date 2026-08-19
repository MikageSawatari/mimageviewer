# backlog §1.96 — 動画を通過した後にキーが効かなくなる (観測を先に足す)

対象: [next-release-backlog.md](../next-release-backlog.md) §1.96。利用者メール (pattier) +
開発側で再現済み。**利用者は次リリース (v3.1.2) に入れたい意向。**

**この brief は観測だけ。原因修正は書かない。** 推測でガードを足さないこと
(CLAUDE.md「バグ修正の一般原則」)。この案件は同型 4 件目なので、当てずっぽうの修正は
特に危ない。

## 0. わかっていること

- 症状: 動画と画像が混在するフォルダをフルスクリーンで <kbd>↑</kbd> / <kbd>↓</kbd> 送りし、
  **画像 → 動画 → 画像**と進んだ後にキーが効かなくなる。<kbd>Esc</kbd> では一覧へ戻れる。
- 再現フォルダを用意した: **`C:\tmp\miv-mixed-video-nav`** (15 件、画像と動画を交互に配置。
  単独動画を 2 か所、連続動画を 1 か所置いてある)。
- **`Esc` が効くことは仮説と矛盾しない (2026-08-19 確認)**。フルスクリーンのキー処理は
  `input_permits.discrete` が無いと [ui_fullscreen.rs:17515](../../src/ui_fullscreen.rs:17515) で
  その frame のキーを捨てて return するが、**`Esc` の handler は同関数の外側にも複数ある**
  ([ui_fullscreen.rs:12781](../../src/ui_fullscreen.rs:12781),
  [16618](../../src/ui_fullscreen.rs:16618),
  [33346](../../src/ui_fullscreen.rs:33346))。矢印キーの handler はこの gate の**内側だけ**。
  つまり「Esc だけ生き残る」は fail-closed が発火している場合の予想どおりの見え方。
- `discrete` は `viewport_focused || routed_key_down` で決まる
  ([keyboard_input.rs:159](../../src/keyboard_input.rs:159))。
  `viewport_focused` = `ctx.input(|i| i.viewport().focused)`、
  `routed_key_down` = `crate::key_input::frame_had_key_down(viewport)`。
- **この early return は完全に無言**で、発火しても記録が残らない。これが今回の障害。

## 1. やること — 無言の early return に理由を残す

`input_permits.discrete.is_none()` で return する地点で、**なぜ permit が無いのか**を
記録する。型付きにし、自由文にしない。

記録する内容:

| 項目 | 意図 |
| --- | --- |
| `viewport_focused` | permit の 2 条件のうち片方 |
| `routed_key_down` | もう片方 |
| viewport id | どのビューポートの pass か |
| 現在アイテムの種別 (Image / Video / Audio 等) | 「動画を抜けた直後」を特定する |
| native presenter の有無と HWND の生存 | presenter が残っているか |
| foreground HWND / main HWND / presenter HWND | **誰がキーボードを持っているか** |
| 捨てたキーの内容 (key + modifiers、数個まで) | 「全キーが死んでいる」ことの直接証拠 |

### 1.1 出す条件 (重要)

- **この frame に実際に捨てたキー入力があるときだけ出す。** gate 自体は入力の有無に関係なく
  毎 frame 走るので、無条件に出すと洪水になる。
- **抑制条件を、調査対象の信号に依存させてはならない。** 「permit が無い理由」が調査対象なので、
  `viewport_focused` や `routed_key_down` の値で出す/出さないを変えない。**捨てた入力があるか**
  だけで決める (これは調査対象ではなく、事象が起きたかどうかの判定)。
  この原則で過去 2 回失敗している ([next-release-backlog.md](../next-release-backlog.md) §1.91、
  §5.4-A)。
- 連打で線形に増えるのは可。frame ごとに出るものにしない。
- 性能ログ ON のときだけ perf event を出す。**加えて、通常ログ (`mimageviewer.log`) にも
  1 行残す**こと。利用者が perf log 無しで再現したときに何も残らないのを避ける。
  通常ログ側は同じ状態が続く間の連投を抑えること (状態が変わったときだけ、など)。

### 1.2 判別できるようにすること

このログを見たときに、次が**一意に決まる**こと:

- **A: 入力の届き先の問題** — permit が無いまま矢印が捨てられている
- **B: ナビゲーション側の問題** — permit はあるのに移動しない (この場合、上の event は
  出ない。出ないこと自体が B の証拠になる)

したがって **B のときに「permit はあった」ことも分かる**必要がある。gate を通過した後の
ナビゲーション判断にも、既存の計装で足りないなら最小限の記録を足す
(新しい高頻度イベントは作らない)。

## 2. やらないこと

- **原因を直さない。** guard / delay / retry / focus の強制奪還を追加しない。
- presenter の生成・破棄・focus 処理に手を入れない。
- `decide_keyboard_input_permits` の判定を変えない。**読むだけ。**
- 時間窓で何かを判定しない。

## 3. 凍結ルール

detached / native video の発火面に触れるので
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に §2 を読み、
完了時に §11 へ「観測追加のみ」として追記する。

## 4. テスト

1. permit があるときは event が出ない。
2. permit が無く、かつ捨てたキーがあるときだけ出る。
3. permit が無くても捨てたキーが無ければ出ない。
4. `viewport_focused` / `routed_key_down` の 4 組み合わせが、値としてそのまま出る
   (どちらが欠けたのかログから判る)。
5. 既存のフルスクリーン入力テストが無修正で通る。**赤くなったら報告して止まる。**

## 5. 利用者へ渡す手順 (報告に書くこと)

`C:\tmp\miv-mixed-video-nav` を使って、①性能ログ ON →②再起動 →③フォルダを開いて
フルスクリーン →④<kbd>↓</kbd> で画像 → 動画 → 画像と進む →⑤矢印が効かなくなったら
**そのまま**他のキーも試す (Enter / R / M / Space など。**全キーが死んでいるか、矢印だけかを
確かめる**) →⑥再起動せず「ログを zip にする」。

## 6. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、イベント名、追加テスト、§5 の手順を含める。
