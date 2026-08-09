# ブリーフ: 前面復帰中にタッチが全部捨てられる

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.14-11 / §5.15。

前提 (完了・コミット済み・実機確認済み): Phase 1 一式、Phase 2、Step 3b、Step 3d
(`6840786a` まで)。

---

## 1. 症状と実機ログ

実機報告 (2026-08-08):

> ヘルプが出て、**タップしても消えない**状態になりました。
> (別ウィンドウで文章を打った後) 再度タップしたら消えました。

同じ利用者から前日にも「マウス操作を合わせてテストしたせいか、タップで HUD が
出なくなった。その後いろいろやっていたら出るようになった。条件が分からない」
という報告が出ている。**両方とも同じ現象**である。

`MIV_TOUCH_DEBUG=1` のログ (`%APPDATA%\mimageviewer\logs\mimageviewer.log`) より、
効かなかったタップ 11 回すべてが次の形をしている:

```
[fs-focus] foreground=0x761406 fullscreen=0x80326
           current_foreign=true suppress=true native_claim=true set_foreground=true
[TOUCH-DEBUG] egui ... events=[Touch(id=1551 phase=Start pos=(595.2,384.0)) -> PointerMoved -> PointerButton(pressed=true) -> ...]
[TOUCH-DEBUG] correlation ... owner=Undecided->Undecided commands=0[] contacts=0->0
                                                                      ^^^^^^^^^^^^
```

**egui のイベント列に Start が入っているのに、認識器の接点が 0 のまま。**
20.3〜23.6 秒の 11 回が全滅し、57.6 秒の 1 回だけ `contacts=0->1` で通っている。
これは利用者が別ウィンドウで入力していた時間と正確に一致する。

## 2. 原因

[ui_fullscreen.rs:16455 付近](../src/ui_fullscreen.rs)。他アプリが前面のとき:

```rust
self.fs_suppress_primary_until_release = true;
let _ = crate::touch_correlation::drive_egui_touch_input(
    ctx, TouchSurface::StillFullscreen, /* geometry */ ..., self.frame_counter,
    false,                       // ← touch_input_enabled = false
);
return (FsPageNav::None, false); // ← 入力処理を丸ごと飛ばす
```

「別アプリから戻るときのクリックを操作に使わない」というマウス向けの既存処理で、
タッチ対応より前から存在する。**Step 3d の退行ではない。**

### なぜタッチだと致命的か

- マウスは「1 回クリックして前面化 → 2 回目から操作」で自然に抜けられる。
  タッチには**その最初のクリックに相当する別操作が無い**
- **最初の 1 回だけでなく、前面に戻るまでずっと**捨て続ける。実機では 3.3 秒間に
  11 回叩いて全部無視された
- 初回オーバーレイヘルプが出ている状態でこれに入ると、**ヘルプを消す手段が無くなる**

---

## 3. 直し方

動画 native 側で決めた **plan §5.14-11 (activation tap)** と同じ考え方を、静止画の
egui 面にも適用する。ただし静止画には**左右タップのページ送りという副作用がある**ので、
そこだけ動画と条件が違う。

| 前面復帰中のタッチ | 扱い |
| --- | --- |
| **中央タップ (`ToggleChrome`)** | **通す**。副作用が無く、これが唯一の脱出口 |
| 左右タップ (`PageSide`) | **通さない**。前面に戻すだけの操作でページが飛ぶのは驚く |
| ピンチ / パン / その他コマンド | **通さない** |
| widget への合成 primary press | **通さない** (既存の `fs_suppress_primary_until_release` のまま) |

### 3.1 実装の方向

**早期 return の構造は壊さないこと。** あの branch が他の入力処理を飛ばすのは意図的で、
そこへ通常の入力経路を流し込むのは範囲外。変えるのは**その branch の中**だけ:

- `drive_egui_touch_input` を **`touch_input_enabled = true` で呼ぶ**
  (= 認識器に接点を渡す)
- 返ってきたコマンドのうち **`ToggleChrome` だけ**を既存の
  `toggle_still_touch_chrome_latch` へ流す。他は捨てる
- **`fs_suppress_primary_until_release = true` はそのまま維持**する
- **新しい状態・delay・retry・repaint ループを足さないこと**

### 3.2 ⚠ 無効化中も相関ミラーを同期させること

現在のログでは、`enabled=false` のフレームで Start が来ているのに
`pointer_touch=absent->absent` のままになっている。
= **egui-winit の pointer gate ミラーが更新されていない**。

これは Phase 1 で「ピンチが交互にしか効かない」を起こしたのと同じ種類の穴で
(plan §5.2 / `961bb47f`)、抑止の出入りをまたいだときに状態が壊れる。

- **`touch_input_enabled` の値にかかわらず、相関ミラー (`pointer_touch`) と
  接点の追跡は常に進めること。** `enabled` が制御するのは
  「コマンドを実行してよいか」であって「入力を観測するか」ではない
- 他にも `enabled=false` で呼んでいる箇所がある
  (範囲選択キャプチャ等、[ui_fullscreen.rs:16010 付近](../src/ui_fullscreen.rs))。
  **同じ契約に揃えること**
- この契約を doc comment に明記し、テストで固定する

### 3.3 ヘルプ表示中との関係

初回オーバーレイヘルプ表示中はどのタップも中央タップ扱いなので、上の規則で
**確実に消せる**ようになる。ここが今回の実害なので、テストで固定すること。

---

## 4. マウス無影響 (§5.15)

- **マウスのみの入力列では挙動が一切変わらないこと。**
  前面復帰クリックが操作に使われない既存挙動 (`fs_suppress_primary_until_release`)
  を維持する
- キーボード操作も不変
- `MIV_DISABLE_TOUCH_GESTURES=1` で現行挙動へ戻ること

---

## 5. テスト

- 前面復帰中 (foreign foreground) のタッチで:
  - 中央タップ → **クロームが出る / 初回ヘルプが消える**
  - 左右タップ → **ページが動かない**
  - 合成 primary press が widget に届かない
- 抑止に入る前 / 中 / 出た後をまたいでも、**接点と相関ミラーが破綻しないこと**
  (Start が抑止中、End が抑止解除後、のような並びを含める)
- `enabled=false` の他の呼び出し箇所でもミラーが同期すること
- マウスのみの列で従来どおりであること

---

## 6. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4952 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **[docs/touch-support-plan.md](touch-support-plan.md) を更新**すること:
  - §5.14-11 に「静止画 egui 面では `PageSide` を除いて適用」を追記
  - §5.2 に「`touch_input_enabled` は観測ではなく実行の可否である」という契約を追記
  - 実機ログで確認した原因と、Step 3d の退行ではないことを記録

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- detached-rework 凍結ルールは有効。focus 機構の既存構造には手を入れず、
  **必要になったら症状パッチを入れずに報告すること**
- **範囲を広げないこと。** foreground 奪還ロジック自体は直さない

---

完了したら次を報告すること:

1. `enabled` の契約をどう整理したか (観測と実行の分離)
2. 他の `enabled=false` 呼び出し箇所への影響
3. 抑止の出入りをまたぐ状態の一貫性をどうテストしたか
4. テスト結果
5. **実機で確認してほしいこと**
