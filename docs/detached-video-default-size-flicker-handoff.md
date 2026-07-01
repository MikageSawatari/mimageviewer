# detached 動画: 既定サイズ小窓チラツキ 修正ハンドオフ

対象: F12 / ナビで detached 動画ビューアを再表示するたびに、保存配置 (例 1543x1377)
へ落ち着く前に **egui viewport が既定サイズ 822x656 の小窓をカスケード位置で一瞬出す**
チラツキ。これを消すのが目的。

## 解決 (2026-07-01)

修正済み (実機レビュー 2 回転を反映した確定版)。

### 決定的原因

detached **動画**では、native presenter child (WS_CHILD) を host に付け替える過程で、
**egui が同一 ViewportId の OS 窓を既定サイズ 822x656 で作り直す**。しかも旧 host は
**生存したまま** (即座に破棄されず、数フレーム後に GC される) 新既定サイズ窓が生えるため、
「host が生きているなら再生成しない」という前提が崩れる。この egui 挙動自体は既知で、
[src/app/native_video.rs](../src/app/native_video.rs) の death-gate コメント
(「egui は detached viewport を作り直す過程で『別の既定位置 host 窓』を一瞬見せる…
拡大された古いフレーム」) が記述している。ユーザーが見た「拡大されたサムネが一瞬」= この窓。

実機ログ (2026-07-01) の該当シーケンス:
```
6.270 captured host 0x441ccc rect=(314,614 1543x1377)   ← 正しいサイズを capture
6.531 placement switched detached-viewer-child            ← presenter child 再構築
6.583 active_placement_update_rejected_default 533x400    ← egui が既定サイズ窓を報告
6.712 host_lost (0x441ccc alive=false)                    ← 旧 host が遅れて GC
6.745 captured host 0x3c02d8 rect=(314,614 1543x1377)     ← 再生成窓が保存配置へ落ち着いてから capture
```

### 入れた修正

1. **detached 動画中は builder に placement を常時 seed** ([src/ui_fullscreen.rs](../src/ui_fullscreen.rs))
   — active builder の `apply_placement` を
   `need_show || detached_viewer_should_seed_placement() || detached_video_presentation_active_or_targeted()`
   に。detached 動画 (active / detach 切替中) の間は毎フレーム live placement を seed するので、
   egui が窓を作り直しても最初から保存配置サイズで生成され、既定サイズ窓が一切見えない。旧 host
   生存中も seed する点が肝 (host 生存判定だけの初版では取りこぼした = 実機レビュー P1)。
   Win32 タイトルバー drag は modal loop 中に egui が builder を再 diff しないので、常時 seed でも
   ドラッグは引き戻さない。静止画の enumerate holdover は live placement を更新しないので従来
   どおり host 未生存時のみ seed (Codex P2)。active / inactive holdover / keep-alive backstop の
   3 経路は共有述語 `App::detached_viewer_should_seed_placement()` (= host 未生存) に統一
   (backstop の `== 0` は stale nonzero HWND 取りこぼし = P2)。
2. **capture の「最後の防波堤」** ([src/app.rs](../src/app.rs)) — `capture_detached_viewer_host_hwnd_from_logical_rect`
   が既定サイズ (outer ≤ 900x720 physical) かつ十分大きい保存配置から hard shrink した候補窓を
   host に採用しない (`detached_capture_rect_looks_like_default_viewport`)。判定に使う previous は
   **save より前に**スナップショットして渡す (同一フレームで save が live/settings を default rect
   へ false-negative 上書きすると防波堤が汚染される = 実機レビュー P2)。これで presenter child を
   既定サイズ窓へ再親付けする挙動 (動画が一瞬小窓へ寄る) が止まる。
   併せて active render 経路は `restore_placement.is_some()` の過渡フレームでは capture 自体を skip。
3. **switching 状態も default-rejection に含める** ([src/app.rs](../src/app.rs)) — F12 動画 detach の
   遷移中は `viewer_presentation` がまだ `Fullscreen` で `viewer_session_is_detached()` が false に
   なる窓があり、その間 save 側の default-rejection が発火しなかった (Codex P1)。save 側 guard を
   render 側と同じ `viewer_session_is_detached_or_switching()` に揃えた。

### 続き: 小窓は消えたが「窓が再表示される」動き (2 次症状) も解決

小窓フラッシュを消した後、実機で「本来サイズの窓が出た後、一度消えてアニメーションしながら
再表示 + 拡大された古いフレームの二枚目 presenter 窓」が残った。原因は **switching 中の
ViewportId が不安定**だったこと (Codex 実機 P1):

- MainWindow→DetachedWindow の切替中は render 側が `viewer_session_is_detached_or_switching()`
  で detached host を出すが、`viewer_presentation` はまだ MainWindow・`active_detached_session`
  も未設定なので、`fullscreen_viewport_id()` が **fallback `("fullscreen_viewer", generation)`
  ID** で host を作っていた。
- `PlacementSwitched` で `begin_active_detached_session` が走った瞬間に **detached ID へ変わり**、
  egui が OS 窓を破棄→再生成 (= 再表示アニメ)。最初の presenter child は旧 host 道連れ死し、
  death-gate resync が二枚目の presenter 窓を出していた。

修正:
1. **切替開始で window_id を確定** ([src/app/native_video.rs](../src/app/native_video.rs)) —
   `switch_native_video_viewer_presentation` で target=DetachedWindow のとき、host を最初に出す
   **前に** `ensure_detached_viewer_window_id()` を呼ぶ。`apply_video_presentation_switched` も
   同じ ensure を呼ぶので id は一致する。
2. **切替中から安定 detached ID を返す** ([src/ui_fullscreen.rs](../src/ui_fullscreen.rs)) —
   `fullscreen_viewport_id()` に「`viewer_session_is_detached_or_switching()` かつ window_id あり」
   の分岐を追加 (既存の presentation==DetachedWindow 分岐は folder-nav reopen で fullscreen_idx が
   一時 None の間も維持されるので残す)。これで switching 開始から commit まで同じ detached ID を
   使い、OS 窓の作り直しが起きない。
3. **失敗/timeout cleanup も正しい窓を隠す** ([src/ui_fullscreen.rs](../src/ui_fullscreen.rs)) —
   switch が失敗して presentation が非 detached へ戻っても、detached host を表示済み
   (`fs_viewport_presentation==DetachedWindow`) の間は detached ID を返す分岐を追加。cleanup の
   `Visible(false)` が実際に出した detached host 窓を隠せる (fallback ID だと pre-commit host が
   宙に残る = Codex P2)。

回帰テスト (全体): `detached_viewer_should_seed_placement_tracks_host_liveness` /
`detached_inactive_builder_seeds_placement_unless_host_is_alive` /
`detached_capture_rect_rejects_default_viewport_last_line_of_defense` (save 汚染後も pre-save
snapshot で弾く) / `active_detached_default_viewport_geometry_is_rejected_during_detach_switch` /
`fullscreen_viewport_id_is_stable_across_detach_switch` /
`fullscreen_viewport_id_stays_detached_for_failed_switch_cleanup`。
Codex 実機レビュー計 6 ラウンドで P1/P2 対応済み・残指摘なしを確認。**実機目視での最終確認は要**
(下記「再現・検証」の目安ログ + 小窓が見えないこと + 窓の再表示アニメが出ないこと)。

付随して、pre-existing のテスト赤 (`native_video_f12_toggles_detached_viewer_mode`、commit
18728865 の `GetAsyncKeyState` 物理キー判定が headless で常に stale 扱いになる) も解消:
`native_video_key_physically_down` を `#[cfg(test)]` で true 返しにして実機専用判定を迂回。

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
