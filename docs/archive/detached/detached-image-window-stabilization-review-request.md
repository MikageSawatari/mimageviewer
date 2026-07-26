# detached image window stabilization review request

> Superseded note (2026-06): この文書は、`self.items` を共有したまま複数 detached
> window のちらつきを止めるための安定化レビュー記録である。現在の本対応は
> `docs/archive/detached/detached-viewer-context-separation-plan.md` を正とし、PDF / ZIP の detached book
> viewer では passive window に `ViewerContextDescriptor` を持たせ、focus-in で active context
> を再列挙して復帰できる。以下の「passive window は active viewer へ変換しない」は、
> context 分離前の暫定仕様として読むこと。

## 背景

画像を別ウィンドウで複数開く実装で、次のような不安定な挙動が出ていた。

- 複数の画像別ウィンドウを開いた後、別ウィンドウをアクティブにするとそのウィンドウが消える。
- ウィンドウを閉じた後に、同じようなウィンドウが複数回ちらついて表示される。
- 「毎回新しいウィンドウ」のはずが、active viewer の OS ウィンドウを再利用して中身を差し替える経路と、退避した passive window の経路が混ざっていた。

原因として、前回実装した「passive window をクリック / フォーカスしたら active viewer として再オープンする」処理が、現在の eframe viewport lifecycle と相性が悪かった。
passive viewport を消しつつ active fullscreen viewport を開き直すため、ユーザー視点では window close / recreate / focus が連鎖して見えていた。

## 今回の設計方針

いったん仕様を安定側へ戻し、active viewer と passive window を明確に分離する。

- active detached viewer は 1 つだけ存在し、ページ送り・スライドショー・編集・AI アップスケール・先読みなどの処理対象を持つ。
- passive `DetachedImageWindowSnapshot` は表示専用とする。
- passive window は texture / title / location / image size / rotation / placement / pinned だけを持つ。
- passive window はクリックやフォーカスで active viewer へ自動変換しない。
- active viewer への復帰を将来実装する場合は、同じ viewport を継続利用する設計を別途行う。passive viewport を閉じて別 viewport を開く実装には戻さない。

## 状態モデル

### active detached viewer

既存の `fullscreen_idx` と `viewer_presentation == ViewerPresentation::DetachedWindow` が active viewer の正本。

`detached_viewer_independent_active` は、active detached still session がメイン一覧と同期しないことを表す session 状態。
これは open 時の one-shot flag ではなく、ページ送り後も保持する。

### passive detached image window

`detached_image_windows: Vec<DetachedImageWindowSnapshot>` が passive window の一覧。
passive window は active session ではないため、次の処理を行わない。

- `open_fullscreen`
- `close_fullscreen`
- `sync_main_selection_from_viewer_idx`
- `sync_detached_viewer_to_selected`
- `fs_cache` / `ai_upscale_cache` / prefetch / slideshow / edit state の所有

### viewport cleanup

`detached_image_window_close_pending: Vec<u64>` を追加した。
`prepare_detached_image_windows_for_open` など `egui::Context` を持たない場所で passive window を取り除いた場合、次の `render_detached_image_windows` で該当 viewport に `ViewportCommand::Close` を送る。

これにより、state から削除した passive window の OS viewport が eframe 側に残り、後でちらついて再表示されることを避ける。

### active viewport recreate

`detached_viewer_recreate_on_next_render: bool` を追加した。

次の場合は、既存 active detached viewport の中身差し替えではなく、新しい active top-level viewport として作り直す。

- 「画像を開くとき、毎回新しいウィンドウで開く」で、現在の active viewer を passive snapshot に退避したあとに次画像を開く。
- 通常モードで未ピン留め passive window の placement を再利用して次画像を開く。

再生成時は `hide_current_fullscreen_viewport_for_recreate` で古い active viewport を隠し、`fs_viewport_generation` を進める。
その後、保存された placement を使って新しい active detached viewport を作る。

## 意図した挙動

### 毎回新しいウィンドウ

1. メイン一覧で画像 A を開く。
2. active detached viewer A が開く。
3. メイン一覧で画像 B を開く。
4. A は passive snapshot として残る。
5. active viewer は新しい OS viewport として B を開く。
6. A の passive window をアクティブにしても B の active session へ切り替わらない。
7. A を閉じると、state から消え、該当 viewport に明示 close が送られる。

### 通常モード + ピン留め

1. 通常 detached viewer を開く。
2. ピン留めすると `detached_viewer_independent_active = true` になり、メイン一覧との同期を止める。
3. メイン一覧で次画像を開くと、現在の active viewer は pinned passive snapshot として残る。
4. 新しい active detached viewer は通常モードの linked session として開き、メイン一覧との同期に戻る。
5. その active viewer 内のページ送りでも linked 状態を維持し、メイン一覧へ同期する。
6. linked / independent の切替は OS フォーカスでは行わず、ピン留め操作だけで行う。

### ZIP / PDF の自動 fullscreen

ZIP / PDF はページ列挙が非同期なので、`pending_auto_fs_open` を `DeferredFsReopen` に載せ替えてから enumerate 完了時に `open_fullscreen` する。
この deferred state は grid / CLI / SendTo の明示 open 由来かを保持し、detached viewer の focus と「毎回新しいウィンドウ」判定へ渡す。
Ctrl+↑↓ フォルダナビ由来の deferred reopen は明示 open ではないため、focus は奪わない。

PDF / ZIP の L2 ページ一覧でメイン側 Backspace から親一覧へ戻る場合は、次画像 open ではなく仮想フォルダ退出として扱う。
そのため active detached viewer は passive snapshot に退避せず閉じる。

### passive window

passive window は最後に見た画像を保持するだけ。
閉じる・移動・サイズ変更・ピン状態変更のみを受け付ける。

## 今回あえて見送ったこと

- passive window のクリック / フォーカスによる active viewer 化。
- ZIP/PDF を親一覧に留まったまま完全に独立した viewer context で開くこと。
- 複数 active session。

これらは現在の `fullscreen_idx` / `self.items` 共有モデルでは安全に実装しづらい。
特に passive window の active 化は、同じ OS viewport を引き継げない限り window close / recreate がユーザーに見えるため、今回の安定化対象から外した。

## レビューしてほしい点

- active detached viewer と passive detached image window を分離する方針で、今回のちらつき / 消える問題を構造的に避けられているか。
- passive window 削除時に `detached_image_window_close_pending` で明示 close する方針に漏れがないか。
- active viewer を新規ウィンドウ扱いにするため `detached_viewer_recreate_on_next_render` で viewport generation を進める方針が妥当か。
- `detached_viewer_independent_active` を session 状態として保持しつつ、pinned active viewer を passive へ退避した後の通常モード active viewer が linked に戻るか。
- 将来 passive window を active viewer に戻す場合、必要な追加設計は「同じ viewport の引き継ぎ」または「viewer context 分離」で足りるか。

## 関連コード

- `src/app.rs`
  - `DetachedImageWindowSnapshot`
  - `detached_viewer_independent_active`
  - `detached_viewer_recreate_on_next_render`
  - `detached_image_window_close_pending`
  - `prepare_detached_image_windows_for_open`
- `src/ui_fullscreen.rs`
  - `render_detached_image_windows`
  - `render_fullscreen_viewport`
- `src/app/tests.rs`
  - `still_window_mode_key_tests`
