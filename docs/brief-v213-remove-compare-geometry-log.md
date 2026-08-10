# ブリーフ: 比較ジオメトリの一時ログを撤去する

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない** (commit / add / stash / branch / reset いずれも)。master の作業ツリーに
  未コミットのまま残す。統合はこちらで行う。
- 作業ツリーは他セッションと共有している。**自分が触ったファイル以外に手を出さない**。

## 背景

v2.13.0 の比較機能 (X でピン留め / C で比較表示 / Shift+C ワイプ / Alt+C 差分) を直す間、
原因特定のために `[compare-geometry]` という一時ログを入れた。原因は確定し、修正は
`5d761678` までで入っている。このログは**出荷前に撤去する**。

`log_compare_geometry_if_changed` は「行が変わったら出す」抑制付きだが、ワイプをドラッグ
している間は fraction が毎フレーム変わるので**実質毎フレーム 1 行**出る。長い比較セッションで
`mimageviewer.log` が膨らむので、リリース版に残せない。

該当箇所には既に撤去予定のコメントがある:

```
src/ui_fullscreen.rs:5744
    // TEMPORARY instrumentation: 原因確定後は回帰テストへ置き換えてこのログを撤去する。
```

## やること

`src/ui_fullscreen.rs` から `[compare-geometry]` の計装を**すべて**消す。

消す対象:

- `struct CompareGeometryLogState` (`src/ui_fullscreen.rs:248` 付近)
- `fn compare_geometry_log_rect` / `compare_geometry_log_uv_window` /
  `compare_geometry_log_zoom_pan` / `compare_geometry_log_mode` /
  `compare_geometry_log_navigator` (`src/ui_fullscreen.rs:5607`-`5666` 付近)
- `fn log_compare_geometry_if_changed` (`src/ui_fullscreen.rs:5669`-`5748` 付近)
- 呼び出し 2 箇所:
  - ナビゲータ側 `src/ui_fullscreen.rs:17783`-`17797` の `if let (Some(pair), Some(draw_rect)) = ...` ブロック
  - メイン側 `src/ui_fullscreen.rs:23853`-`23867` の同型ブロック

**あわせて、ログ専用になっていたローカル束縛も消す**。両呼び出し元の直前にある

```rust
let draw_rect = shader_shape
    .as_ref()
    .map(|shape| shape.draw_rect)
    .or_else(|| { ... Self::compare_image_draw_rect(...) });
```

は、この 2 つのログ呼び出しでしか読まれていない (以降の描画は `shader_shape.draw_rect` を
直接使う)。ログを消したらこの `draw_rect` も消す。**消す前に、本当に他から読まれていないことを
確認すること**。読まれているなら残す。

## 残すもの (消さないこと)

- **`[compare-memory]` のログは残す**。こちらは prepare-start / cpu-ready / gpu-ready /
  gpu-clear / deactivate / pinned-hidden-retained / pinned-ready-reused など**離散的な
  ライフサイクル事象でしか出ない**ので、毎フレーム出るジオメトリログとは性質が違う。
  比較は今回 GPU テクスチャ上限超過でクラッシュした箇所なので、割り当てと解放の順序が
  ログに残る価値がある。`src/app.rs` / `src/compare_wgpu.rs` / `src/ui_fullscreen.rs` の
  `[compare-memory]` には一切触らない。
- `compare_shader_shape` / `compare_shader_visible_region` / `compare_wipe_line_visible` /
  `compare_wipe_screen_x` など、描画側の関数はそのまま。
- 既存のテストはそのまま通すこと。ジオメトリログのために足したテストがあれば、
  それは消してよい (あるかどうか確認すること)。

## 完了条件

- `git grep -n "compare-geometry"` が `src/` で 0 件 (docs/ のブリーフに残るのは可)。
- `cargo fmt` 済み、`cargo fmt --check` が通る。
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
- `cargo test -p mimageviewer --lib compare` が通る。
- 未使用の関数・型・変数の warning が新たに出ていないこと。

## 報告してほしいこと

- 消した項目の一覧。
- `draw_rect` のローカル束縛を消したか、残したか。残したならその理由 (どこから読まれているか)。
- ジオメトリログ用のテストがあったか。あったならどう扱ったか。
