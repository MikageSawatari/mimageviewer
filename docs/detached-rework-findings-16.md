# 検収所見 #16: メイングリッドのフォルダ移動が detached PDF 窓のページを「デコード失敗」化する

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
実機 (2026-07-07 深夜、smoke 中)。「動画を開いた後、画像 (PDF ページ) をスクロールすると
デコードに失敗エラー」。ログ解析済み (scratchpad/dec2_cur.log)。

## 実測 (確定)

```
36.257s === load_folder: c:\home\youtube\movie\youtube ===   ← メイングリッドのフォルダ移動
36.257s pdf-pool: pruned 19 stale jobs (current_epoch=8)
36.257s fs pdf render FAIL: context epoch advanced  Page 4..17 (×13)
        ← detached PDF 窓 (id=2, fs_idx=4, pending=13) の先読みページレンダが全滅
36.259s pdf-pool: pruned 6 stale jobs (current_epoch=9)
156.567s (同型の 2 回目バースト)
```

- 殺されたレンダは `FsLoadResult::Failed` として送信され、**fs_cache に Failed
  エントリが焼き付く** ([app.rs:34592 付近](../src/app.rs) の PDF arm)。
- ユーザーがその PDF 窓でスクロールすると Failed ページは「デコードに失敗」の終端表示
  (再試行なし)。
- 同時刻の grid サムネ `pdf-interrupted` は正常 (keep_range が再要求する)。問題は
  fullscreen 側の fs_load のみ。

## 根本原因

context epoch (2026-05、docs/pdf-pool-context-epoch-plan.md) は「UI ナビしたら旧 PDF の
レンダは無駄」という**単一コンテキスト前提**の設計。複数ウィンドウでは:

1. detached PDF 窓のコンテキストは生きている → その fullscreen ページレンダは stale
   ではないのに、メイングリッドの `load_folder` の epoch bump が殺す。
2. さらに epoch キャンセルが「デコード失敗」として記録される (キャンセル ≠ 失敗)。

今日の変更の退行ではなく、multi-window 化で露出した設計相互作用。リワーク退行では
ないが、基本フローでユーザー可視エラーになるため ship 前に修正する。

## 修正要件

### H1 (意味論、必須): epoch キャンセルを Failed として記録しない

- fs_load の PDF arm で、エラーが **cancel 系 (`context epoch advanced` /
  Interrupted 等)** の場合は `FsLoadResult::Failed` を送らない。
  fs_pending を畳むだけの **Canceled 通知** (または既存の cancel 経路への合流) にし、
  fs_cache に Failed を書かない → 次にそのページを表示するとき通常どおり再要求される。
- エラー種別の判定は pool が返すエラー enum / 種別で行う (文字列マッチにしない。
  必要なら pool のエラー型に cancel 判別を追加)。
- 真のデコード失敗 (壊れた PDF 等) は従来どおり Failed 表示。

### H2 (根本、必須): 生きている detached コンテキストの fs_load を epoch prune の対象外にする

- fullscreen ページの fs_load レンダ (現在ページ + 先読み) は「ユーザーが明示的に
  開いているコンテキスト」の仕事であり、メイングリッドのナビとは独立。
  **fs_load 経由の PDF render リクエストは `context_epoch=0` (prune 対象外 sentinel)
  にする**。ライフサイクルは既存の fs_load cancel トークン (コンテキスト close /
  ページ移動で発火) が既に管理しており、epoch に頼る必要がない。
- 対象は fs_load 経路のみ。grid サムネ (thumb_loader 経由) の epoch 運用は変更しない
  (メインのナビで旧フォルダのサムネを流すのは従来どおり正しい)。
- docs/pdf-pool-context-epoch-plan.md に「fs_load は epoch=0 (multi-window 対応、
  findings-16)」を追記する。

### テスト

1. H1: cancel 系エラーの fs_load 結果が fs_cache に Failed を残さない
   (再要求可能な状態に戻る)。
2. H2: fs_load の LoadRequest が epoch=0 で構築される。
3. シーケンス: detached PDF 窓の先読み中に main の epoch bump →
   ページが Failed にならず、後続の表示要求で再レンダされる。

## 完了条件

- [x] H1 + H2 + テスト。コミット `(detached-rework findings-16)`
- [ ] full test 緑 / fmt / glyphs / build-release

## 実装メモ (Codex 2026-07-07)

- H2: `App::fs_pdf_render_context_epoch()` を追加し、`start_fs_load` の PDF render は
  current / prefetch の priority に関係なく `context_epoch=0` に固定した。grid thumbnail
  / `thumb_loader` 経路の epoch 運用は変更しない。
- H1: `App::fs_pdf_render_error_is_cancel_like()` を追加し、`cancel` flag または
  `ErrorKind::Interrupted` の PDF render error は `FsLoadResult::Failed` を送らず、
  既存 cancel 経路と同じく `fs_pending` を畳むだけにした。真の PDF render error は
  従来どおり Failed 化する。
- 回帰テスト:
  - `fullscreen_pdf_loads_are_not_pruned_by_grid_epoch`
  - `fullscreen_pdf_interrupted_render_is_cancel_like`

## 実機確認

1. detached PDF 窓でページを開いたまま、メイングリッドでフォルダ移動 (動画フォルダ等) →
   PDF 窓に戻ってスクロール → 「デコードに失敗」が出ない (少し待てば全ページ表示)
2. 壊れたファイルの真のデコード失敗表示は従来どおり
