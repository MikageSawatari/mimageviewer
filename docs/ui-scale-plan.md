# UI スケール設定 実装計画 (VST プラグイン画面のみ非対応)

## 0. 背景と目的

高 DPI モニタで mIV の UI 文字が小さいという不満への恒久対策として、**アプリ内 UI スケール設定**
(70%〜200% 程度)を追加する。実体は egui の `zoom_factor` を永続化して適用するもので、OS の
表示スケール(= `native_pixels_per_point`)とは独立した「アプリ側の追加倍率」になる。

```
実効 pixels_per_point = zoom_factor(=UI スケール) × native_pixels_per_point(=OS DPI)
```

### スコープ判断 (確定済み)

- **メインウィンドウの egui UI**(メニュー / ダイアログ / タグ設定 / メタパネル / グリッド /
  静止画フルスクリーン) → **スケール反映する**。egui が points 単位で自己整合するため追加実装ほぼ不要。
- **動画 overlay の通常 panel**(HUD / チャプター・ブックマーク一覧 / 一括ブックマーク /
  ショートカットヘルプ / タグ picker 等、`overlay_draw.rs` が描くもの) → **スケール反映する**。
  これらは native presenter の**別 egui Context**が描画するので、presenter 側の ppp にスケールを
  注入する必要がある。
- **VST プラグイン GUI 画面**(`src/video/dsp/gui.rs` が作る bridge 所有の独立 HWND) →
  **スケール非対応**(据え置き)。プラグイン描画面は他社コードが所有し mIV から拡縮できず、また
  利用者が少なく、現状「本体ウィンドウ側の動画フルスクリーン」に限定されているため、この画面だけ
  OS 表示スケールに従う挙動は許容する。

### 調査で確定した前提 (実装の土台)

1. 現状 `zoom_factor` はコードで一切設定しておらず、egui 既定の `Options::zoom_with_keyboard=true`
   (Ctrl + Plus/Equals/Minus/Num0) だけが効いている。値は永続化されておらず再起動で 1.0 に戻る。
2. `ctx.input(i.pixels_per_point)` (= `App::last_pixels_per_point`, app.rs:21604) は **zoom 込みの
   実効値**。`compute_display_px` (thumb_loader.rs:777) 経由でサムネ/表示解像度に波及する。
3. native presenter は `egui::Context::default()` (mod.rs:3811) の**独立 Context**を持ち、
   `self.pixels_per_point` **1 個**を真実源として描画・当たり判定 region・ポインタ座標・panel
   レイアウトの全てを導出する:
   - 描画: `raw_input.viewports[ROOT].native_pixels_per_point = self.pixels_per_point` (mod.rs:5822) →
     `egui_ctx.run` (mod.rs:5828)。presenter Context の `zoom_factor` は 1.0 のまま。
   - region: `compute_hud_regions` が `self.pixels_per_point` (mod.rs:4971) を使う。
   - ポインタ: `x / self.pixels_per_point` (mod.rs:4317)。
   - panel レイアウト: `self.height / self.pixels_per_point` (mod.rs:5474 他)。
   → **`self.pixels_per_point` にスケールを掛ければ overlay 全体が自己整合してスケールする**
     (描画と region が同じ値なのでクリック位置もズレない)。これは DPI 変更と同じ経路
     (125/150/200% モニタで実績あり)。
4. `self.pixels_per_point` を設定する箇所は 2 つ:
   - 初期化: `pixels_per_point_for_hwnd(dcomp_hwnd)` = `GetDpiForWindow/96` (mod.rs:7858 / 3814)。
   - DPI 変更: `set_overlay_pixels_per_point(dpi/96)` (mod.rs:2624, 呼び出しは video/mod.rs:2860 の
     `WM_DPICHANGED` 経路)。
5. presenter と**別に** OS DPI を直接読んで幾何を計算している App 側ヘルパー(= presenter の
   `self.pixels_per_point` を経由しない)があり、overlay をスケールするとこれらが desync する:
   - `tick_vst_window_overlap_adjustment` (native_video.rs:1351/1407): VST 窓の overlap 判定に
     `GetDpiForWindow(presenter_hwnd)/96` を直接使用。
   - `native_video_overlay_size_points` (native_video.rs:4510/4533): タイルレイアウト用サイズ算出に
     `GetDpiForWindow(win)/96` を直接使用 (`video_tile_layout_size` から呼ばれる)。
6. VST3 統合は egui スケール量に依存しない(別 HWND / OS 画面座標 / `GetDpiForSystem`・
   `GetDpiForWindow` 直接取得。`crates/vst3-host/`・`src/video/dsp/` に `pixels_per_point` /
   `zoom_factor` 参照ゼロ)。→ スケール設定で VST コードに手を入れる必要はない。
7. スクリーンショット/キャプチャ (capture.rs) は `ColorImage` の物理サイズで独立、zoom 非依存。
8. UI スナップショットテストは固定 ppp なので影響なし。

## 1. 設計方針: 「単一の真実源 = `ctx.zoom_factor()`」

UI スケールの実効値を **メイン egui Context の `zoom_factor` に一本化**する。

- **settings** はその永続化先。起動時に settings → `ctx.set_zoom_factor()` で適用。
- **presenter** は `ctx.zoom_factor()` の数値を**倍率としてミラー**し、自分の ppp に掛ける
  (`presenter_ppp = (dpi/96) × ui_scale`)。presenter Context 自体の zoom_factor は 1.0 のまま
  (= `self.pixels_per_point` に scale を織り込む)。
- **キーボードズーム** (Ctrl+±) は初回リリースでは無効化する (§2.3、両 Context で
  `zoom_with_keyboard=false`)。真実源は `settings.ui_scale_factor` に固定。

これにより「combo で変えても」「キーボードで変えても」「起動時に復元しても」全経路で
1 つの値 (`ctx.zoom_factor()`) に収束し、メイン UI・動画 overlay が同じ倍率で揃う。

## 2. Phase 1 — メインウィンドウのスケール (低リスク)

### 2.1 settings フィールド追加

- `src/settings.rs` + `src/settings_db.rs`: `ui_scale_factor: f32` (既定 1.0) を追加。
  - **範囲は 50%〜200% を 10% 刻みで提示する** (= 0.5, 0.6, … 2.0 の 16 段。**確定**)。50% は
    `set_pixels_per_point` の下限 (0.5) と一致するので presenter でもそのまま通る。
    - **案Y 採用 (100% 未満も動作させる)**: presenter の `max(1.0)` 経路 (§3.1) を生 ppp に直し、
      50% でも region と描画を一致させる。Codex P1 の「100% 未満で HUD 当たり判定がズレる」は
      **クリック位置ズレではなく、小要素のサブピクセル丸めによる軽微な見た目ズレ (±1〜2px) に
      とどまる**と判断し、ユーザー許容。50% 利用者は少なく、実害は視覚的な微差のみ。
    - この修正は **ppp ≥ 1.0 (= 現行の全 DPI ケース) では完全な no-op** なので、既存の 100% 以上
      挙動への回帰リスクはほぼゼロ。効くのは新しい 100% 未満時だけ。
  - **未リリース機能なので migration 不要**(前回リリース以降の新規追加。CLAUDE.md「永続データ・
    スキーマ変更時の判断」に従い、コミットメッセージにその旨を残す)。値が無い/壊れている旧
    settings は既定 1.0 にフォールバック。

### 2.2 起動時適用

- `src/main.rs` creator closure、テーマ適用(apply_theme, 1035 付近)の直後に:
  ```rust
  cc.egui_ctx.set_zoom_factor(saved.ui_scale_factor.clamp(0.2, 5.0));
  ```
  テーマと同様、初回フレーム前に適用して 1 フレーム目のちらつきを避ける。

### 2.3 UI: メニュー「設定 > スケーリング」(確定)

- **エントリポイントはメニューバーの「設定」→「スケーリング」サブメニュー**。50%〜200% を
  10% 刻みで並べ、現在値にチェックマークを付ける (ラジオ的な選択)。
- 選択時に `ctx.set_zoom_factor(v)` を即時適用 + `settings.ui_scale_factor = v` を保存。
  active な fullscreen / detached / native video viewer がある場合は、ビューワモード設定変更と
  同じ close 経路で閉じる。再 open 時に新倍率で presenter / viewport content を構築する。
- 現在値は `settings.ui_scale_factor` を正とする (キーボードズームは §下記で無効化するため、
  値が外部から動くことはない)。
- (任意) 環境設定ダイアログの表示ページにも同じ選択肢を置くかは実装時判断。まずはメニュー優先。

**キーボードズーム (Ctrl+±) の方針 → 初回は無効化に決定 (Codex P2 反映)**:
- **`zoom_with_keyboard = false` を「メイン Context と presenter overlay Context の両方」に設定する。**
  presenter は別 `egui::Context::default()` (mod.rs:3811) を持ち、これも既定で `zoom_with_keyboard=true`
  なので、キー入力が overlay Context に届くと **overlay だけが独自にズームして desync** しうる
  (Codex P2)。両 Context で無効化し、UI スケールは settings combo を唯一の変更経路にする。
- 将来キーボードズームを復活させるなら、メイン Context の zoom 変化検出 → settings 保存 →
  presenter への同期 → keymap 衝突整理、が別途必要。初回リリースでは見送る。

### 2.4 サムネ/表示解像度の扱い

- `compute_display_px` は `last_pixels_per_point`(zoom 込み)で解像度を決めるため、スケールを
  上げるとサムネ/画像デコードが高解像度化する(クランプ 256〜2048px, thumb_loader.rs:768-770)。
- **方針: 現状維持**(スケールに応じて鮮明化するのは正しい挙動、上限クランプで頭打ち)。
  低スペック機での負荷増はユーザーが設定で選ぶ決定的挙動として許容 (CLAUDE.md の
  deterministic-over-adaptive 方針に整合)。ドキュメントに「高スケールでメモリ使用が増える」旨を注記。

## 3. Phase 2 — 動画 overlay のスケール (presenter, 中リスク)

### 3.1 presenter に UI スケールを保持させる

- `NativeEguiOverlay` (mod.rs) に `ui_scale: f32` (既定 1.0) を追加。
- presenter が `self.pixels_per_point` を算出する箇所を **必ず `(dpi/96) × ui_scale`** に統一:
  - 初期化: `pixels_per_point_for_hwnd(dcomp_hwnd)` (mod.rs:7858 / 3814) の結果に `ui_scale` を掛ける。
  - DPI 変更: `set_overlay_pixels_per_point` (mod.rs:2624)。**呼び出し側 (video/mod.rs:2859) が
    `dpi/96.0` を直接渡している**ので、そこを `os_ppp × ui_scale` にする (Codex P2 で明示指摘)。
    → presenter が現在の `ui_scale` を保持し、`WM_DPICHANGED` 時に自分で掛ける形が堅い
      (呼び出し側が掛け忘れても一貫する)。
- `set_ui_scale(scale)` setter を追加: `ui_scale` 更新 + 現在の dpi から `self.pixels_per_point` を
  再計算 (= 既存 `set_pixels_per_point` 経路を通し、再レイアウト + region 再計算をトリガ)。
- **`max(1.0)` 経路を生 ppp に直す (案Y・Codex P1 対応、50% 対応の中核)**: 描画側は生
  `self.pixels_per_point` を使うのに、以下は `max(1.0)` で床上げしていて 100% 未満で desync する。
  **これらを生 ppp に揃える** (ppp ≥ 1.0 では no-op = 既存挙動不変):
  - `compute_hud_regions` の `ppp` (mod.rs:4971) — ここが `to_px`/`rect_to_px`/`width_points`/
    `height_points` に流れるので、この 1 行で HUD 内の全 region が一斉に整合する。
  - cursor polling 活性領域 (mod.rs:2834)
  - IME cursor area (mod.rs:7734)
  - ring guide 換算 (overlay_draw.rs:1962) — ここは `native_ring_guide_overlay_rect` に生 ppp を
    渡す経路 (mod.rs:5183) と二重に絡むので特に注意。
  - **`(self.width/ppp).max(1.0)` 等、"結果" 側の `max(1.0)`(最小 1pt ガード)は残す**(ppp の床とは別物)。

### 3.2 App → presenter の倍率適用と実行中変更

- presenter は別スレッド。生成時に現在の `ctx.zoom_factor()` を渡す(コンストラクタ引数)。
- 実行中の live propagation は行わない。倍率変更時は active viewer を既存 close 経路で閉じ、
  再 open 時に正しい倍率を渡す。mounted active detached bundle / source-swap pending など複数所有先への
  伝搬漏れを避ける低リスク方針とし、短い再生中断を仕様として許容する。
- **detached viewer (F12) は専用の別経路は不要 (Codex P2 反映)**。F12 は placement を
  `DetachedViewerChild` に切替 (app.rs:24294) し、`SwitchPlacement` が presenter を作り直す
  (video/mod.rs:2540)。既存状態は `cur_*` として新 presenter に再適用される (video/mod.rs:2567)。
  → session 内の placement 切替では生成時の `ui_scale` を `cur_ui_scale` として保持し、presenter を
    再生成するたびに再適用する。settings から倍率を変えた session 自体は上記 close 経路で終了する。

### 3.3 直接 `GetDpiForWindow` を読む App 側ヘルパーの辻褄合わせ

overlay を scale する以上、presenter の ppp を経由せず OS DPI を直読みしている以下を
**`(dpi/96) × ui_scale`** に統一する(でないと overlay の点空間とズレる):

- `native_video_overlay_size_points` (native_video.rs:4533): `w_px / ((dpi/96) * ui_scale)` に変更。
  → `video_tile_layout_size` 経由でタイル敷き詰めが overlay の点空間と一致する。
- `tick_vst_window_overlap_adjustment` (native_video.rs:1407): HUD 帯の物理範囲は **scale 込み ppp**
  で計算し、VST 窓の矩形は **OS 物理 px のまま** (VST 窓は非スケール) で比較する。
  → 「スケールされた HUD」対「非スケールの VST 窓」の overlap を正しく判定する。ここが本実装で
    最も注意を要する箇所(過去に VST 座標系で難航した領域)。ハードコードの `62pt` band は
    scale 込み ppp で physical 化する。

### 3.4 スケールされないもの (設計どおり・確認事項)

- **動画映像そのもの**: DComp サーフェス全面に描画され overlay ppp と独立。scale しても映像は
  全画面のまま、その上の HUD/文字だけ拡大 = 望ましい挙動。実装で映像経路に触れないことを確認。
- **VST プラグイン GUI** (src/video/dsp/gui.rs): 別 HWND なので自動的に非スケール。**何もしない**。
  ただし §3.3 の overlap 調整で「VST 窓 = 非スケール」を前提に計算することを明示。

## 4. 影響なしを確認済み (触らない)

- VST3 音声処理 (normalize / LUFS / bridge IPC): 座標無関係。
- スクリーンショット/キャプチャ: `ColorImage` 物理サイズ、zoom 非依存。
- UI スナップショットテスト: 固定 ppp。
- viewport の物理 geometry は UI 表示倍率から独立させる。保存 placement / monitor rect は OS DPI only
  の論理 geometry を正本とし、`ViewportBuilder` / `ViewportCommand` へ渡す直前に `ui_scale` で割る。
  egui から placement を保存するときは逆に `ui_scale` を掛けて正本へ戻す。native DPI は eframe / winit
  の logical→physical 変換に残すため、高 DPI / 複数 monitor を壊さない。

## 5. リスクと検証計画

| 領域 | リスク | 検証 |
|---|---|---|
| メイン窓スケール (Phase 1) | 低 (egui 自己整合) | 数段階 (100/150/200%) でダイアログ・タグ設定・グリッドが崩れず、小窓+高スケールで固定高パネル(消しゴム等)が見切れないか目視 |
| overlay スケール (Phase 2) | 低〜中 (単一 ppp 源だが presenter は別スレッド) | 動画 fullscreen で HUD/チャプター/ブックマーク/タグ picker が拡大し、**クリック位置が合う**か。tile grid が正しく敷き詰まるか |
| VST overlap (§3.3) | 中 (scale HUD × 非scale VST 窓) | **実プラグイン 1 本**で、スケール変更時に VST 窓が HUD に不正に重なる/飛ぶことがないか目視 (time-box 可) |
| detached viewer presenter | 中 (再生成時の初期値) | 倍率変更で active detached が閉じ、F12 で開き直した動画 overlay が新倍率になるか |
| サムネ解像度 | 低 (クランプ済) | 高スケールでメモリ増が過大でないか (2048 上限確認) |

- **多モニタ DPI 全網羅の再検証は不要**(overlay スケールは DPI 変更パス流用)。VST overlap のみ
  実プラグイン確認が要る。VST は利用者少・quilk 許容の前提なので time-box する。
- 追加の perf 計装は不要(スケールは静的設定で hot path に影響しない)。

### 5.1 50% (100% 未満) 動画 HUD の実機チェックリスト (案Y の検証)

`max(1.0)` を生 ppp に直した結果、50% で動画 HUD の region が描画と一致するかを確認する。
共通 `rect_to_px` 経由の要素は 1 箇所直せば揃うので、まず「軽い確認」で機構を検証し、
その後「取りこぼしやすい個別要素」を各 1 回:

- **軽い確認 (機構検証・7 割方これで潰れる)**: 50% で動画再生 → タグ設定 → ブックマーク登録/一覧。
  シークバー・上バー・タグ picker・チャプター/ブックマーク一覧・ブックマーク名編集のクリックが合うか。
- **取りこぼしやすい個別要素 (各 1 回)**:
  1. リング picker / guide を出してクリック (ppp が二重に絡む唯一の要素)
  2. 複数動画でタイルグリッド表示 → タイル選択
  3. 小アイコンボタン (フレームステップ / カメラ / 速度 / ?) — サブピクセル丸めズレの確認
  4. 長いタグ名 / 長いブックマークタイトルでダイアログ下部ボタンが切れないか
  5. ブックマーク名編集で日本語 IME 変換 (候補窓位置)
  6. (VST 使用時のみ) プラグイン読込 → VST3 パネルと overlap
- 想定される最悪ケースは **小要素の ±1〜2px の見た目ズレのみ** (クリック位置ズレではない)。
  それを超える破綻 (要素が大きくずれる / クリックが外れる) が出たら、その要素固有の ppp 経路を追う。

## 6. ドキュメント更新 (CLAUDE.md doc-sync ポリシー)

- `htdocs/mimageviewer/manual/` (表示/環境設定ページ): UI スケール設定の説明。
  「**動画再生中に表示される VST プラグインの画面は、Windows の表示スケールに従います**
  (アプリ内スケールは反映されません)」と注記。バージョンタグは書かない。
- `htdocs/mimageviewer/index.html`: 機能一覧に「UI 表示倍率の設定」。
- `docs/video-architecture.md` / `docs/display-pipeline.md`: presenter の ppp に UI スケールを
  乗せる旨、生成時の `ui_scale` 適用と変更時 close 経路を追記。
- `docs/spec.md`: 設定項目 `ui_scale_factor` を追加。
- キーボードズームを無効化する場合 (案 ii) は、その旨と理由を `docs/keymap-spec.md` に
  「固定扱い/対象外」として残す。

## 7. 実装順序 (提案)

1. Phase 1 (settings `ui_scale_factor` + 起動時 set_zoom_factor + メニュー「設定>スケーリング」
   50-200% 10%刻み + 両 Context の keyboard zoom 無効化) — 単体で価値があり低リスク。
2. Phase 2 §3.1〜3.2 (presenter ppp 注入 + `max(1.0)` 4 箇所を生 ppp 化 + 変更時 viewer close +
   session 内 placement 再生成時の `cur_ui_scale` 再適用)。
3. Phase 2 §3.3 (VST overlap / tile size 辻褄) — 最注意。
4. ドキュメント更新。
5. 検証 (§5 + §5.1 の 50% チェックリスト) → Codex レビュー → リリース手順。

## 8. オープン論点 → Codex レビューで解決済み

1. **キーボードズーム**: **初回は無効化** (§2.3)。メイン + presenter overlay の**両 Context**で
   `zoom_with_keyboard=false`。真実源を settings に固定。復活は後追い。
2. **OS DPI 直読みの見落とし**: §3.3 の 2 箇所 (native_video.rs:1407 / 4533) に加え、**WM_DPICHANGED
   の presenter 更新 (video/mod.rs:2859) と初期 DPI (mod.rs:7858)** も注入点 (§3.1 に反映済み)。
   App 側リストはこれで概ね完全との評価。
3. **detached viewer**: 別の live propagation は作らない。倍率変更時は既存 viewer close 経路を使い、
   session 内の placement 再生成だけ `cur_ui_scale` を再適用する (§3.2 に反映済み)。
4. **VST overlap**: HUD 領域は `os_ppp × ui_scale` で px 化、VST 窓矩形は screen px のまま比較。
   VST タイトルバー/Windows chrome は OS DPI 寸法として扱う。**HUD ppp に `max(1.0)` を入れない**
   (§3.3 の方針どおり)。
5. **サムネ解像度**: v1 は連動のままで可 (2048px cap + VRAM cap で許容)。不満が出たら
   「thumbnail render scale」を独立設定にするのが次段階 (§2.4 の現状維持で確定)。

### 9. Codex レビュー総括 + 最終決定

- **P1**: `self.pixels_per_point.max(1.0)` 経路があるため 100% 未満で HUD 描画と region が desync。
  → **当初は「100%以上に限定して回避」だったが、ユーザー判断で 50%〜200% を採用 (案Y)**。
  `max(1.0)` 4 箇所 (§3.1) を生 ppp に直して 50% でも整合させる。**残リスクは小要素の ±1〜2px
  見た目ズレのみ**でクリック位置は合う、という前提で許容。ppp≥1.0 では no-op のため既存挙動は不変。
- **P2 (反映済み)**: キーボードズームは両 Context で無効化 / WM_DPICHANGED (video/mod.rs:2859) も
  注入点 / 倍率変更時は viewer close / session 内 detached 再生成は `cur_ui_scale` 再適用。
- **P3**: VST3 は egui scale から独立との主張は妥当 (別 HWND・OS API、`vst3_available` は fullscreen
  borderless 限定 app.rs:44574)。VST 除外方針で問題なし。

### 10. 最終確定仕様 (2026-07-02)

- **範囲/刻み**: 50%〜200% を 10% 刻み (16 段)。
- **UI**: メニュー「設定 > スケーリング」から選択 (現在値にチェック)。
- **VST プラグイン GUI 画面のみ非スケール** (据置)。それ以外 (メイン窓 + 動画 overlay の通常
  panel = タグ設定/チャプター/ブックマーク等) は全て反映。
- **50% 対応**: 案Y (`max(1.0)` 4 箇所を生 ppp 化) を実装。許容リスク = 軽微な見た目ズレのみ。
- **キーボードズーム無効化** / **サムネ解像度は連動のまま** / **未リリースにつき migration 不要**。
