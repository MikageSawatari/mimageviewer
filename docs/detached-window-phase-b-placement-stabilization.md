# Detached Window Phase B: placement lifetime stabilization

目的: Phase A2 で viewport/HWND runtime を `ViewerContextBundle` から外した後も残っていた
「active close 後に passive window が 533x400 へ縮む」問題を、placement の所有元を整理して抑止する。

## 1. 実機ログで残っていた症状

Phase A2 後のログでは、`host_lost_before_render` による recreate は消えた。
一方で active window close 後に次のような passive placement update が残った。

```text
active_close_finalize begin ...
passive_placement_update id=7 initial_apply=false
  from=DetachedViewerWindowPlacement { x: 502.0, y: 138.0, w: 1278.0, h: 840.6667, ... }
  to=DetachedViewerWindowPlacement { x: 152.0, y: 152.0, w: 533.3334, h: 400.0, ... }
```

`533.3334 x 400.0` は 1.5 ppp 環境の 800x600 physical に相当し、egui/winit の
新規 viewport 既定値を passive snapshot の正しい placement として採用している可能性が高い。

## 2. 今回の修正方針

### 2.1 active viewport の live placement を runtime として持つ

`settings.detached_viewer_window_placement` は次に開く window の seed と設定保存を兼ねる。
そのため active window を pause/snapshot 化するときに settings だけを見ると、
「今閉じる/退避する active 窓の実位置」ではなく、別 window の seed や default を拾うことがある。

Phase B では `active_detached_viewer_live_placement` を App-global runtime として追加した。
これは bundle に入れず、A2 の viewport runtime と同じく active detached viewport の実測値として扱う。

- 新規 active window runtime reset 時: 現在の設定 placement を初期 seed として入れる。
- passive resume 時: resume 対象 snapshot の placement を設定へ戻した後、その値を runtime seed として入れる。
- active detached render 中: `outer_rect` と `inner_rect` から実測した placement を settings と runtime の両方へ保存する。
- snapshot 作成時: runtime 値を優先し、無い場合のみ settings/default へ fallback する。

### 2.2 passive window の HWND を追跡し、再生成をログで観測する

`initial_placement_applied` は「過去に builder へ placement を渡した」ことしか表さない。
active close の直後に OS/egui 側で passive viewport が一時的に既定 geometry を報告しても、
`initial_placement_applied=true` のままでは保存済み placement を再 seed できない。

Phase B では passive snapshot に `passive_host_hwnd` を持たせ、passive render でも
active と同じ rect-based HWND capture を行う。

- 初回表示は従来どおり `initial_placement_applied=false` により placement seed を送る。
- その後は毎フレーム位置を強制しないため、drag 中の引き戻しは起きない。
- HWND が既知の状態から別 HWND へ変わった場合は viewport 再生成とみなし、`passive_hwnd_changed`
  を出す。
- HWND 変化そのものでは placement seed を再要求しない。実機ログで、HWND 変化のたびに seed すると
  seed / recreate / seed のループが起きることが分かったため、seed は default geometry を拒否した
  window だけに限定する。
- `passive_hwnd_changed` ログを出すため、兄弟 window close 時に残存 passive window が
  再生成されているかを実機ログで確認できる。

### 2.3 800x600 physical 相当への急落を placement update として採用しない

保険として、passive placement update が次の条件を満たす場合は保存しない。

- 初回 placement 適用フレームではない。
- candidate の logical size * ppp が 800x600 physical 近傍。
- 直前 placement から大きく縮んでいる。
- 位置も大きく跳んでいる。
- 直前 window は十分大きい。

該当した場合はログに `passive_placement_update_rejected_default` を出し、
該当 window の `initial_placement_applied=false` に戻して次フレームで再 seed する。

## 3. 変更した主なコード

- `src/app.rs`
  - `active_detached_viewer_live_placement` を追加。
  - `DetachedImageWindowSnapshot::passive_host_hwnd` を追加。
  - `build_active_detached_image_window_snapshot()` が live placement を優先。
  - `save_detached_viewer_placement_from_logical_rect()` /
    `save_detached_viewer_placement_from_native_geometry()` が settings と live runtime を同時更新。
  - active/passive 共通で使う `find_detached_viewer_host_hwnd_from_logical_rect()` を追加。
  - `detached_passive_placement_update_looks_like_default_viewport()` を追加。
- `src/ui_fullscreen.rs`
  - passive render でも HWND を capture し、`passive_hwnd_changed` をログ出力。
  - passive placement update に ppp を含めて評価。
  - default viewport らしい update を拒否して再 seed を要求。
  - active/passive close 後、passive window が残っている間は main font atlas resync と main focus
    reclaim を行わない。実機ログで、close 直後の font atlas resync と全 window re-seed が
    `passive_hwnd_changed` の連続発火と slow frame を増幅していたため。

## 4. 期待するログ

成功時:

- Phase A2 同様、`recreate viewport: reason=host_lost_before_render` は出ない。
- close 後に `passive_hwnd_changed` が連続発火しない。
- active close 後に 533x400 へ落ちる update が来た場合は
  `passive_placement_update_rejected_default` になり、snapshot placement は更新されない。
- default geometry を拒否した window だけ、次フレームで保存済み placement が builder に再 seed される。

もしまだ縮む場合は、`passive_placement_update_rejected_default` が出ているか、
それとも条件外の `passive_placement_update` として採用されているかを見る。

## 5. 残る検討

今回の修正は App-global active runtime + passive snapshot の範囲で placement を安定化する。
より厳密には、window id ごとの runtime map と viewport generation/epoch を持つ設計が最終形だが、
Phase B では既存構造に最小限追加して、実機ログで観測された default geometry 誤採用を止める。

増幅源の除去後の実機ログでは `passive_hwnd_changed` が 0 件になったため、passive window 存在中の
継続 `ctx.request_repaint()` は入れていない。もし今後 close 後の `passive_hwnd_changed` が再発する場合は、
`show_viewport_immediate` の lifetime 対策として継続 repaint または deferred viewport 化を再検討する。
