# 引き継ぎ: detached を閉じたときのメイン画面ちらつき

**宛先**: detached リワーク担当セッション (worktree `C:\home\mimageviewer-r2e` /
branch `video-latency-and-context-ownership`)
**調査**: video-strip セッション (2026-08-27)
**状態**: 原因まで特定済み・**未修正**。凍結ルール (CLAUDE.md「Detached viewer リワーク中のルール」)
に触れる範囲なので、こちらでは直していない。

---

## 1. 症状

**F12 の別ウィンドウ (detached) を閉じると、画面全体が数フレーム白/黒/灰に明滅する。**

利用者が画面キャプチャを提供済み: `C:\Users\mikag\Videos\2026-08-27 08-40-25.mkv`
(1280x720 / 30fps / 3.53 秒)。

キャプチャを 1 フレームずつ輝度 (YAVG) で測った結果:

| フレーム | 時刻 | 平均輝度 | 見え方 |
| ---: | ---: | ---: | --- |
| 53 | 1.767 | 167.9 | 動画 (正常) |
| 54-55 | 1.800 | **233.8** | 白 |
| 56-57 | 1.867 | **26.4** | 黒 |
| 58 | 1.933 | **119.4** | 灰 |
| 59 | 1.967 | **25.7** | 黒 |
| 60 | 2.000 | **211.8** | 明 |
| 61 | 2.033 | 168.2 | |
| 62 | 2.067 | **133.8** | 暗 |
| 63 | 2.100 | 210.0 | デスクトップ |

**9 フレーム / 約 300 ms のあいだに全画面の明るさが 6 回変わる。**
フレームを個別に見ると、mIV のウィンドウだけでなく**デスクトップ全体が塗り直されて**おり、
他アプリ (Chrome / ドキュメントウィンドウ) が 1 枚ずつ描き直されていく様子が写っている。

再現測定は次で行える:

```
ffmpeg -i <capture> -vf "signalstats,metadata=print:key=lavfi.signalstats.YAVG" -f null -
```

---

## 2. 測れた事実 (ログ)

### 2.1 フレームの隙間

`[ui-frame-gap]` を `App::update` の入口に追加済み (video-strip ブランチ、
`src/app.rs`)。update と update の**間隔**が 50ms を越えたら出る。

```
14.453  [native-video-key] ... presentation=detached outcome=action:close_detached_session
14.458  [viewport] cleanup_visible_false: presentation=Some(DetachedWindow) recreate=true
14.486  [native-video] fullscreen presenter stopped
14.621  [ui-fonts] schedule main font atlas resync: detached_viewer_cleanup
14.645  [ui-fonts] discard pass for font atlas resync (same-frame repass) generation=1
14.685  [ui-frame-gap] 232.6ms without a frame: presentation=Fullscreen detached=false
14.727  [ui-fonts] discard pass ... generation=2
14.768  [ui-frame-gap] 52.4ms
14.796  [ui-fonts] discard pass ... generation=3
14.841  [ui-frame-gap] 62.4ms
14.856  [ui-fonts] discard pass ... generation=4
14.897  [ui-frame-gap] 50.0ms
14.915  [ui-fonts] discard pass ... generation=5
15.103  [ui-frame-gap] 100.6ms
```

**パス破棄 1 回につき 50-60ms の隙間が 1 つ。それが 5 回続く。**
14.45 から 15.10 の約 650ms のうち、およそ 500ms ぶんフレームが進んでいない。

### 2.2 別セッションでも同じ形

前日の別セッション (同じ操作) でも generation 41-45 で 5 回連続。
そのときは 1 パスが **351.8ms (うち input=348.3ms)** かかっており、ログ全体でこの 1 件だけ突出。

```
[37.364s] [eframe] immediate_viewport total=351.8ms input=348.3 run=0.6 paint=1.1
```

**`input` が支配的**な理由は未調査。egui の入力収集中に何かを待っている。

### 2.3 副次的な観測

`presentation=DetachedWindow` の間、`[ui-frame-gap] 64.x ms` が定常的に並ぶ
(= メインウィンドウが約 15fps)。意図的な間引きかどうかは未確認。**別件の可能性**。

---

## 3. 機構

`ctx.request_discard()` は「このパスを捨てて同じフレーム内で描き直す」。
捨てられたパスは画面に出ないので、**そのフレームは進まない**。

判定はここ (`src/app.rs`):

```rust
fn should_defer_main_paint_for_font_atlas_resync(_reason: &str) -> bool {
    true
}
```

**引数を受け取って無視し、常に true を返す。** すぐ下の分岐

```rust
if !defer_main_paint {
    // detached cleanup 等、保守的 defer が不要な経路。フォント再アップロードを
    // 予約しただけで、この pass はそのままメイン UI を描く。
```

は**到達しない**。コメントだけが元の意図を残している。

---

## 4. 経緯 (ここが判断の核心)

もともとは detached cleanup を例外にしていた。**まさに今回のちらつきを避けるため**に入った。

```
0277e7ba  Avoid main flicker when closing detached viewer
          fn should_defer_main_paint_for_font_atlas_resync(reason: &str) -> bool {
              reason != FONT_ATLAS_RESYNC_REASON_DETACHED_VIEWER_CLEANUP
          }
```

その後、意図的に外されている。

```
d48982e5  2026-06-18  Use conservative font atlas resync for detached cleanup
```

`docs/display-pipeline.md` の同時変更にその理由が書かれている:

> detached cleanup も、メイン UI を stale font atlas のまま描くとフォント崩れが
> 残ることがあるため、同じ保守経路へ乗せる。

**つまりバグではなく、ちらつきとフォント崩れのトレードオフで後者を避けた結果。**
単純に `0277e7ba` へ戻すと、`d48982e5` が避けたフォント崩れが再発しうる。

---

## 5. 未確認のこと (推測しないで残した部分)

- **なぜ 5 フレーム続くのか。** `d48982e5` は「1 パス破棄して描き直す」つもりで書かれており、
  5 回連続は想定外に見える。generation が 1 -> 5 と増えるので、**再同期が毎フレーム自分を
  再予約している**可能性があるが、**確認していない**。
- **`input=348.3ms` が何を待っているか。** 1 件しか観測しておらず、再現条件も不明。
- **`presentation=DetachedWindow` 中の 64ms 間隔**が意図的かどうか。
- **利用者はウィンドウモード (複数ウィンドウ / フル機能) を切り替えながら試している**ので、
  経路が毎回同じとは限らない。`close_fullscreen` に入れた計器は 1 行も出なかった
  (= その経路は通っていない)。

---

## 6. 提案する方向 (video-strip セッションの見立て)

**5 回連続する方を減らすのが本筋だと思う。** 1 回で済むなら失われるフレームは 1 つで、
フォント崩れ対策 (`d48982e5`) を保ったままちらつきが実用上消える。
例外を戻す (`0277e7ba` へ回帰) のはトレードオフを逆へ倒すだけで、同じ議論を繰り返す。

ただしこれは**測っていない見立て**なので、まず「なぜ 5 回か」を観測してから決めてほしい。

---

## 7. 再現手順

1. 動画をフルスクリーンで開く
2. **F12** で別ウィンドウ (detached) へ
3. **Escape または Enter** で閉じる
4. `%APPDATA%\mimageviewer\logs\mimageviewer.log` の
   `[ui-frame-gap]` と `discard pass for font atlas resync` を見る

`[ui-frame-gap]` は video-strip ブランチにしか無い (`b29ffde3`)。
必要なら cherry-pick するか、同等のものを入れてほしい。**閾値 50ms、`App::update` の入口で
前フレームとの間隔を測るだけ**の 18 行。

> 注意: アイドル時の就寝も隙間として出る (実測 14.8 秒の行がある)。
> 操作中の 50-250ms の行だけを見ること。

---

## 8. ついでに見つけた別件: flaky なテスト

```
app::tests::still_window_mode_key_tests::detached_builder_placement_latch_does_not_follow_live_drag_updates
```

`5f6cd105 Latch detached video builder placement` (2026-07-07、findings-14) で入ったテスト。

**壊れているのではなく揺れている。** 同じコードのまま:

| 実行 | 結果 |
| --- | --- |
| 単独実行 (20:10 頃) | FAILED |
| 単独実行 x3 (20:30 頃) | 3 回とも ok |
| モジュール全体 450 件 | ok |
| フル実行 (マージ直後) | 6550 件全通過 |
| フル実行 (その後) | 1 件失敗 |

失敗時の値:

```
left:  DetachedViewerWindowPlacement { x: 80.0, y: 80.0, w: 960.0, h: 720.0 }   (既定値に見える)
right: DetachedViewerWindowPlacement { x: 1564.0, y: 240.66667, w: 1167.0, h: 765.0 }
```

期待値はテスト内の定数なので、外部から読んでいるわけではない。`setup_app()` は temp dir と
`test_override_lock()` で隔離されている。**何が揺らしているかは未特定。**
`active_detached_builder_placement_latch` がときどき既定値へ落ちる経路がある、という形。
