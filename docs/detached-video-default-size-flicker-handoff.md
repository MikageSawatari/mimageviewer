# detached 動画: 既定サイズ小窓チラツキ 修正ハンドオフ

対象: F12 / ナビで detached 動画ビューアを再表示するたびに、保存配置 (例 1543x1377)
へ落ち着く前に **egui viewport が既定サイズ 822x656 の小窓をカスケード位置で一瞬出す**
チラツキ。これを消すのが目的。

このドキュメントは「別窓が残る/メインに一瞬出る」問題 (F12 二重トグル) を潰した後に
**残った別系統の問題**の引き継ぎ。長時間セッションを避けるため新セッションで集中対応する。

## すでに直っているもの (この修正の前提・触らない)

- **F12 二重トグル (main フラッシュ + 再分離)**: native presenter が placement 切替
  (Plan B) の再構築後に、既に離された F12 KeyDown を数百 ms 遅れて **stale 再配送**して
  いたのが原因。`repeat=false` でも stale。時間窓では正当な次押下 (~600ms) と分離不能。
  修正 = native F12 の toggle 分岐で `GetAsyncKeyState(F12)` の high bit を見て、KeyDown
  処理時点で物理的に押されていなければ stale として無視 (`native_video_key_physically_down`,
  `[native-video] ignore stale native F12 toggle: os_down=false`)。Codex と合意済み。
  実装: [src/app/native_video.rs](../src/app/native_video.rs) の
  `handle_native_video_key_event` の `ToggleDetachedViewerMode` 分岐。
- **ナビ時の黒点滅**: navigation_preview overlay がサムネ無し (`thumb=None`) でも全画面を
  黒で塗っていた → サムネがあるときだけ黒背景を塗るよう修正済み
  ([src/video/native_presenter/overlay_draw.rs](../src/video/native_presenter/overlay_draw.rs))。
- **ナビで別窓が残る**: F12 churn の連鎖 (main→detached→detached + host 作り直し) が
  親原因で、上記 os_down 修正で churn が止まったことに伴い解消。

これらに入れた一時診断ログ (`[native-video][diag] ...`) は撤去済み。

## 残っている問題 = 既定サイズ小窓チラツキ

### 症状

detached 動画で F12 / ナビをするたびに、正しい配置の窓とは別に **822x656 の小窓** が
一瞬見える。位置は Windows の新規窓カスケードで 114→228→342→456 と 114 ずつずれる。

### 実機ログの決定的証拠 (2026-07-01, `%APPDATA%/mimageviewer/logs/mimageviewer.log`)

```
5.200 [detached-viewer] show viewport: fs_idx=3 activate=Some(true) generation=0 host=hwnd=0
5.229 [detached-viewer] captured host 0x651666 host_generation=2
        new_state="rect=(314,614 1543x1377)"           ← 正しい保存配置の窓
5.231 [native-video] switch placement request=2 -> DetachedWindow   (presenter child rebuild)
5.494 [native-video] placement switched detached-viewer-child hwnd=0x162084 (presenter child)
5.535 [detached-window-debug] active_placement_update_rejected_default
        candidate={x:76 y:76 w:533 h:400}              ← 既定サイズ (533x400 論理 = 822x656 物理) を検出し保存は拒否
5.535 [detached-viewer] captured host 0x399154a host_generation=3
        old="0x651666 (1543x1377)" new="0x399154a rect=(114,114 822x656)"  ← 既定サイズ窓を host として捕捉
5.537 [native-video] synced fullscreen presenter hwnd=0x162084 foreground=0x399154a  ← presenter を既定サイズ窓へ同期
```

読み取れること:
- detached 動画ビューアの **egui viewport が、正しいサイズの窓 (0x651666) とは別に、
  既定サイズ 822x656 の窓 (0x399154a) を生成/報告している** (同一 window_id 内で HWND が
  0x651666 → 0x399154a に変わる)。
- 配置の **保存** は `detached_passive_placement_update_looks_like_default_viewport` /
  `active_placement_update_rejected_default` ガードで既定サイズを弾いて守られている。
- しかし **表示上の小窓** と **presenter host の再捕捉** は防げていない。5.537 で presenter
  child を既定サイズ窓 (0x399154a) に同期し直しているので、映像も一瞬そちらへ寄る可能性。
- `show viewport ... generation=0 host=hwnd=0` が再表示ごとに出る = detached viewport が
  毎回 teardown → 再生成されている (host が 0 にクリアされてから再捕捉)。

### 想定原因 (要検証)

egui の detached viewport が再生成される際、**新規 OS 窓が既定サイズ (822x656) で一瞬作られ、
その後保存配置へリサイズ**される。ViewportBuilder が新規窓生成時に必ず placement を seed
できていない経路がある。

- 参考: [src/ui_fullscreen.rs](../src/ui_fullscreen.rs)
  - `build_detached_viewer_viewport_builder(fs_idx, active, apply_placement)` (~6926):
    `apply_placement=true` のときだけ `with_inner_size/with_position/with_maximized` を seed。
  - `build_inactive_fullscreen_viewport_builder` (~6896): DetachedWindow 分岐で
    `apply_placement = self.detached_viewer_host_hwnd == 0`。**host_hwnd が旧 (死んだ) HWND を
    まだ指している (!=0) 間に新規窓が作られると `apply_placement=false` → 既定サイズ**、という
    取りこぼしが疑わしい。
  - `capture_detached_viewer_host_hwnd_from_logical_rect` ([src/app.rs](../src/app.rs) ~24500):
    viewport の outer_rect に一致する窓を host として捕捉する。outer_rect が一瞬 822x656 に
    なると既定サイズ窓を掴む。

### 修正の方向 (案、着手時に再評価)

1. **新規 OS 窓生成時は必ず placement を seed**: active/inactive どちらのビルダーでも、
   「新しい OS 窓が生まれる可能性がある」フレームでは `apply_placement=true` にして
   既定サイズで出さない。`host_hwnd==0` 判定だけでは stale HWND 参照中に取りこぼす。
2. **既定サイズの host は capture しない**: `capture_detached_viewer_host_hwnd_from_logical_rect`
   で、候補窓が既定 geometry (533x400 論理相当) のときは掴まない
   (`detached_passive_placement_update_looks_like_default_viewport` と同じ判定を capture 側にも
   適用)。presenter を既定サイズ窓へ同期し直す 5.537 の挙動を止める。
3. **viewport を毎回作り直さない**: `show viewport generation=0 host=hwnd=0` が再表示ごとに
   出る = teardown/再生成している。ナビ/F12 をまたいで detached viewport を存続させれば
   新規窓生成自体が減り、チラツキの根が消える。ただしこれは lifecycle の踏み込んだ変更。

再設計の全体像は [docs/detached-viewer-lifecycle-redesign-proposal.md](detached-viewer-lifecycle-redesign-proposal.md) と
[docs/detached-window-current-behavior-investigation.md](detached-window-current-behavior-investigation.md) を参照。

### 再現・検証

- detached 動画再生中に F12 (main⇔detached) / ホイールで動画送り を数回。
- `--perf-log` 不要。通常の `mimageviewer.log` に `captured host ... 822x656` /
  `active_placement_update_rejected_default` が出なくなればチラツキ解消の目安。
- 実機目視で小窓の一瞬が消えたかを確認 (このプロジェクトは UI を実機で精密検証する方針)。

### 注意

- egui 0.33 の viewport は immediate/独立イベントキュー。ViewportId は
  `detached_image_window_viewport_id(window_id)` で window_id ごとに安定だが、OS 窓自体は
  teardown/再生成され得る。ID 安定 = OS 窓不変ではない。
- native presenter child は WS_CHILD で detached host の子。host を既定サイズ窓に付け替えると
  映像もそこへ寄る。host 捕捉の正しさが映像位置の正しさに直結する。
