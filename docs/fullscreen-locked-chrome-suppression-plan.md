# F11 全画面中のロック済みクローム一時抑制 (§1.344)

作成: 2026-10-07 / ライン E / `next-viewer` / 調査基点 `d29bcfbec`。  
改訂: 2026-10-07 R2 / コード照合基点 `ff538c48c`（初版の上に置く文書改訂）。
状態: **設計案のみ。独立 GPT-6.1 Sol / xhigh レビューは REVISE。構造方針への同意あり、
本改訂の再レビュー・利用者判断・ClaudeCode の構造合意は未完了。**

## 1. 依頼と確定済みの境界

[次版バックログ](next-release-backlog.md) §1.344
の「次の版の決定 (利用者 2026-10-07)」と今回の作業指示を正本にする。
E は §1.344 → §1.342 → §1.341 → §1.340 の順で、できた分を出荷する。
今回は §1.344 の設計だけで、後続の表示方式・アニメーション・タイル一覧は扱わない。

- ウィンドウ表示のロックを保ち、**描画対象の窓が F11 全画面の間だけ**固定による常時表示を抑制する。
- 保存済み HUD / strip のロックを F11 入退場で書き換えない。右情報パネルの context-local なロックも維持する。
- F11 を戻すと現在のロック設定に従う。入場時の値を保存して復元する方式にはしない。
- 通常の自動表示と同じ召喚・操作を残す。同一 core の画面端ホバー、touch、popup、開始済み drag は維持する。
  native core 再生成をまたぐ操作の維持は現コードの保証ではないため、§6 と Q7 で別に扱う。
- 静止画、本、native 動画、F12 detached の F11 を同じ契約で扱う。F12 ON だけで抑制しない。
- 「ページ全体」など fit 方式、グリッドの F11 最大化、全画面解除 / 一覧復帰とは別の問いにする。

以下の推奨仕様は利用者の回答前に確定扱いしない。製品バイナリは起動していない。
実行時の観測はなく、現状の記述は下記コードの source inspection に基づく。

## 2. 読んだ正本と主要前提のコード照合

`CLAUDE.md` の必読・バグ修正・設計の簡素化・UI/パネル・応答性・keymap・永続データ・文書同時更新、
[docs 索引](README.md)、[アーキテクチャ概観](architecture-overview.md)、
[表示パイプライン](display-pipeline.md) §2.2.1、[動画アーキテクチャ](video-architecture.md) の
placement / HUD / 固定バー、[右パネル設計](fullscreen-side-panel-mode-plan.md) §6.6、
[detached 憲法](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要) §2 を読んだ。
入力・UI の境界は [keymap 仕様](keymap-spec.md)、[keymap 実装計画](key-customization-impl-plan.md)、
[touch 設計](touch-support-plan.md) §5.5、[UI 応答性](ui-responsiveness.md) §4、
[環境設定レイアウト](preferences-layout-guidelines.md)、[snapshot 方針](ui-snapshot-policy.md) を参照する。
動画 strip は [表示寿命と worker 所有](video-seek-strip-lifetime-plan.md) が正本で、
[strip 設計](video-seek-strip-plan.md) の古い「HUD 非表示で close」という記述を採用しない。

| 確認した前提 | 現コードの根拠 / 設計への影響 |
| --- | --- |
| 静止画と動画の HUD ロックは別の永続設定 | [settings.rs](../src/settings.rs) の `fullscreen_top_bar_locked` / `fullscreen_seek_bar_locked` / `still_seek_strip_locked` と `video_top_bar_locked` / `video_seek_bar_locked` / `video_seek_strip_locked`。抑制方針を共通化しても元のロックは統合しない |
| 下 HUD と strip のロックには依存関係がある | `BottomBarLock::{None, BarOnly, BarAndStrip}`。「バー非固定 + strip 固定」は表現しない。strip 固定の有効化はバーも固定する。独立した抑制 checkbox の無条件追加ではこの不変条件を破る |
| strip の表示選択と lock は別 | 静止画は `still_seek_strip_visible`、動画は `video_seek_strip_state`。F11 抑制を strip OFF として保存すると利用者の表示選択と resource session が変わるため禁止 |
| 右情報パネル lock は保存済み設定ではない | [ui_helpers.rs](../src/ui_helpers.rs) の `FullscreenInfoPanelState { open, locked, hover_active }`、[registry](../src/app/viewer_context_registry.rs) の `fs_info_panel`。窓ごとの bundle が所有し、ファイル移動で保持、本当の viewer 終了で解除する |
| 現在も lock は描画と予約の両方で使う | [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `fullscreen_top_bar_locked_for_idx`、`StillSeekGeometry`、`still_info_panel_lock_effective_for_idx`、navigation gap の geometry / panel shell。通常 content だけ変更すると holdover / gap に旧予約が残る |
| パネルは lock 以外でも表示される | `FullscreenInfoPanelState::visible` は lock、明示 open、tag picker、hover の OR。[ui_metadata_panel.rs](../src/ui_metadata_panel.rs) の描画は lock を実効値へ投影したコピーを使う。lock だけ外しても既存 open があれば隠れない。§9 Q4 の判断が必要 |
| F11 は複数入力入口で同じ操作へ収束する | `KeyAction::FsToggleWindowMode` → `toggle_egui_viewer_window_mode_for_input` / `toggle_video_window_mode_for_input`。native VK、HUD ボタン、ring / mouse / gamepad の window mode も同系統。物理 F11 の検出に新機能を直結しない |
| F12 中の F11 は通常の fullscreen presenter 化ではない | [app.rs](../src/app.rs) の `toggle_detached_viewer_borderless_fullscreen` / `apply_detached_viewer_borderless_target` と viewport builder。detached host の装飾・サイズを変え、native 動画は `DetachedViewerChild` のまま |
| F11 状態は単に mounted bundle を読むだけでは足りない | `detached_viewer_borderless_fullscreen` / transition と `viewer_presentation` は App 側。registry に borderless field はない。main を mount した pass にも active detached の値が見えるので、**描画先 window binding との対応を明示**して投影する |
| passive 静止画は live HUD のコピーではない | `DetachedImageWindowSnapshot` は frozen image / placement 等で、borderless lock のコピーではない。`adopt_active_detached_viewport_runtime_from_passive` は borderless を false にする。park / resume の F11 記憶仕様をこの機能で拡張しない |
| native 動画 HUD は別の描画・入力面 | [render_core.rs](../src/video/native_presenter/render_core.rs) の `NativeEguiOverlay` / `VideoSeekGeometry` / `VideoVisualLayout` / `compute_hud_regions`、[overlay_draw.rs](../src/video/native_presenter/overlay_draw.rs)。`ui_fullscreen.rs` だけでは native 動画を変更できない |
| native 初期化と更新の共通入口は存在する | [native_video.rs](../src/app/native_video.rs) の `native_bar_lock_state` / `sync_native_video_metadata`、[video/mod.rs](../src/video/mod.rs) の config / `SetBarLockState` / `SetSidePanelState`。raw と effective を一つの経路で運ぶ方向が取れる |
| native 通常 command は切替先へ遅延適用される | `placement_transition_control` は control 以外を `pending_commands` に保持。candidate commit で `cur_placement` / `cur_generation` / `cur_owner_hwnd` を更新し、abort では旧値と旧 core を戻す。App で解決済みの旧 F11 bool を通常更新へ載せると、新 core を旧表示契約へ戻し得る。§4.1 で適用時に解く |
| native HUD 非表示は drag の終了原因になる | `hud_visible()` は seek drag owner を参照せず、`!bottom_hud_visible` の終端で `seek_row_gesture` / `seek_strip_drag_origin` を消す。capture-all region だけでは防げない。同一 core の可視判定に既存 owner を接続する必要がある |
| 通常 native F11 は overlay の入力状態を移譲しない | placement switch は `NativeRenderCore::new` で candidate を作り、`NativeEguiOverlay::new` は drag / popup を初期化する。decoder / strip resource の保持と overlay interaction の保持は別。§6 と Q7 で保証範囲を訂正する |
| 右 panel の × は現在 raw lock で禁止される | `ui_metadata_panel.rs` の `explicit && !locked_now`、`overlay_draw.rs` の `click_to_show && !info_panel_locked`、`ui_music_panels.rs` の `!locked_now && music_side_panel_close_visible`。鍵と × に同じ lock 入力を使わず、× は effective に接続する |
| ナビゲータは鍵ボタンではなく固定表示設定 | `fullscreen_navigator_visible` と `FsNavigatorToggle` / `FsNavigatorHold`、`fs_navigator_visibility_requested`。fixed、hold、interaction owner の OR で、Alt を離しても既存 drag は継続する。対象追加は利用者に質問する |

依頼の核（保存ロックを変更せず、F11 表示先から派生させる）は現コードで成立する。
ただし「F11 状態が bundle-local」「下 HUD と strip を独立 lock として扱える」「右 lock が永続設定」
という前提は置けない。上表の実構造を使う。実装でこの所有対応を満たせない経路が見つかったら、
新しい bool / pending や geometry 推定を足さず、設計担当へ返す。

## 3. 対象の棚卸しと設定案（利用者回答待ち）

| 要素 | 元の保持 / 対象案 | F11 中に抑制された場合 |
| --- | --- | --- |
| 上 HUD | 静止画用と動画用の保存 lock。対象候補 | 固定の force と上予約・固定 gap を外す。上端、touch、関連 popup、side panel による一時表示は従来のまま |
| 下 HUD と strip | `BottomBarLock`。**一組の対象として選ぶ案を推奨（Q2）** | 実効値を `None` にして下端自動表示へ。静止画ページ列と native 動画場面 / 波形の表示選択は維持 |
| 右情報パネル | context-local lock。対象候補 | 固定の force と右予約を外す。Hover / ClickToShow、touch handle、tag 編集による一時表示は残す |
| ナビゲータ | `fullscreen_navigator_visible`。**含めるか Q3** | 含めるなら fixed の force だけ外す。hold / interaction は維持。隠れている間の navigator rect と edge exclusion は登録しない |
| 左補正・ジャンプ・ブックマーク panel | 明示 open / hover で、上記と同じ lock はない | 対象に追加しない。右だけの召喚で左を開かない既存分離を維持 |
| ルーペ固定、crop / view trim、VST、分析・編集 panel | 操作モード / 別 window の owner | クローム lock と同一視しない。既存の編集・IME・focus / modal 優先を維持 |
| 音楽ビューの上下 UI | 常時描画で、静止画 / native 動画の HUD lock とは違う | 常時 UI の削除は別仕様。右 lock のみ適用する案を Q5 で質問 |
| ページ番号 overlay / グリッド・ツリー等 | lock 対象ではない | 本機能の対象外 |

設定 UI 案: 環境設定の表示関連ページに「全画面中は固定表示を一時的に自動表示にする」を置き、
上部バー / 下部バーとストリップ / 右情報パネルを選べる。一つの設定値（仮称 `FullscreenChromeSuppression`）に
対象集合をまとめ、空集合を OFF とする。ON bool と「全部 false」の二重 OFF 状態は作らない。
静止画・動画で対象方針は共有し、個々の raw lock は既存のままにする案を Q1 で確認する。
UI 文言・ページ配置は案であり未確定。新しいキー操作の追加は必要ない。

Q2 で個別選択を求められた場合は、単純に `BottomBarLock` の bool を落とすだけでは足りない。
現描画は下 HUD の表示と strip の表示を連動させるため、strip だけ `BarOnly` にしても strip は出得る。
`Keep / SuppressStrip / SuppressBarAndStrip` のような到達可能な下部方針と、その描画可否を同じ owner で
解く設計へ差し戻す。「バーだけ隠して strip を固定」の第4 lock 状態を勝手に増やさない。

## 4. 状態を増やさない実効ロック

描画 pass ごとの読み取り専用入力を、既存 owner から組み立てる。
仮称 `ViewerChromeSurface` は現在の描画先を表す値であり、新しい可変 runtime ではない。

```text
surface = MainEmbedded | Fullscreen | Detached { window_id, borderless_applied }
suppresses(element) = surface.is_f11_fullscreen() && configured_targets.contains(element)
effective_top_lock = raw_top_lock && !suppresses(Top)
effective_bottom_lock = if suppresses(BottomGroup) { None } else { raw_bottom_lock }
effective_info_lock = context.fs_info_panel.locked && !suppresses(Info)
effective_navigator_fixed = raw_navigator_fixed && !suppresses(Navigator)
```

この値に既存の mode eligibility を掛け、描画・geometry・hit の consumer に渡す。
raw / effective を両方持つ読み取り専用の結果（仮称 `ResolvedViewerChrome`）はよいが、
`suppressed` bool の App 保存、入場前ロックの backup、F11 専用の restore pending は作らない。
予約量は実効 lock **かつ描画可能**なときだけ発生する。一時表示は overlay として描き、
予約を一時的に復活させて画像を縮めない。

### 描画先の F11 判定

| 描画先 / 状態 | 判定入力 |
| --- | --- |
| main embedded viewer / main grid | false。main が最大化でも active detached が borderless でも false |
| 非 detached の専用 fullscreen viewport | その pass が選んだ `ViewerPresentation::Fullscreen` の描画先。保存設定だけでは判定しない |
| native `FullscreenBorderless` | 実際に描く committed presenter placement。切替要求中の旧 presenter は旧 placement のまま |
| F12 active detached（画像・本・動画・音楽） | 対象 `window_id` が現在の active host / registry binding と一致したときだけ、既存の applied borderless 値を読む |
| passive / ParkedLive / main の別 context pass | その窓の現 host 契約で解く。active 窓の App 値を無条件に渡さない。現行 passive は通常窓の経路を使い、live HUD のない frozen snapshot へ新しい UI を足さない |
| folder navigation gap / sidecar wait / holdover | 保持中の**表示先**で解く。`fullscreen_idx=None` や新 item 不在を F11 終了とみなさず、既存 navigation continuation の eligibility と組み合わせる |

`ctx.viewport().fullscreen` は borderless virtual fullscreen の正本ではない。
rect / monitor 一致、サイズ、foreground HWND、`settings.detached_viewer_enabled`、
`settings.video_in_window_mode`、field-presence sentinel から F11 を再推定しない。
通常の初回 fullscreen open も同じ表示先なら適用する案を Q1 に含める。

F11 の request 受付ではなく、既存の presentation / borderless 適用境界で表示契約を切り替える。
native の入力 snapshot は raw 設定・context 状態・対象方針を運び、非 detached の F11 bool / effective lock を
App で確定して運ばない。candidate / 遅延更新 / abort は次節の同じ適用規則を使う。
既存 transition owner の成功 / 失敗を使い、本機能独自の rollback、世代、ack 待機を追加しない。

### 4.1 native snapshot の適用先契約（R2・指摘1）

コード根拠: `src/video/mod.rs` の `placement_transition_control`（2193行付近）、
`NativeRenderCore::new`（6109行付近）、commit の `cur_placement` 更新（6322行付近）、
retire 待ち後の abort による旧 core / generation / owner 復元。
通常 command は待機中にも到着し、成功後は新 core、abort 後は旧 core に適用される。
**一 command 化だけでは、旧表示先で解決した実効値の遅延上書きを防げない。**

snapshot 内の二種類の値を同じ適用境界で区別する。

- **context / source に属する raw policy**: raw bar / info lock、info open / mode、抑制対象、寸法設定等。
  同じ player / source / context owner の最新値として、既存 command queue の順序・latest-slot 規則で適用する。
  F11 switch 前の presenter generation で生成したという理由だけで raw 設定更新を捨てない。
  source / context が変わったものは既存の source / binding 検査に従い、別 context の state は適用しない。
- **detached host に属する applied fact**: `DetachedSessionLease.window_id` と
  `DetachedHostClaim { incarnation, hwnd }`、対応する既存 native generation に結び付けた borderless 値。
  App の `current_detached_host_lease` / target の lease から作り、candidate / committed target の契約内で運ぶ。
  lease だけでは同じ host の placement 前後を識別できないので、既存 request / candidate epoch の対応も使う。
  独立した suppression generation、App-global host bool、geometry heuristic は追加しない。

| 適用場面 | surface と snapshot の解決規則 |
| --- | --- |
| 初回生成 / candidate prepare | native driver が `NativeRenderConfig` へ実際の placement / owner identity を渡す。candidate は prepare request の target placement / candidate epoch と exact target host fact で解く。非 detached では detached fact を参照しない |
| 通常更新（保留されていた分を含む） | raw policy を採用後、driver の **現在の `cur_placement` と target identity** から実効値を再計算する。App の旧 `viewer_presentation` / 解決済み F11 値は適用しない |
| detached host fact の通常更新 | 現 target の window lease・host incarnation / HWND・native generation が一致した fact だけ更新する。不一致や旧 main 由来の「detached ではない」という値で、candidate が既に持つ matched fact を消さない。raw policy 部分の更新は別に維持する |
| commit → retire | 新 core / `cur_placement` / generation / owner が同じ target 契約を持つ。App がまだ旧 presentation の間に発行した保留 command も、新 placement で解き直す。旧 generation の host fact は新 target を上書きしない。App の commit 回収後は既存 generation で exact host fact を同期する |
| prepare failure / commit 前 abort | 旧 core と旧 target 契約はそのまま。保留 raw policy は旧 placement で解く。candidate の host fact を旧 core へ移さない |
| commit 後・retire 前 abort | 現行 rollback が旧 core / placement / generation / owner を戻す際、旧 target の host fact もその core に属したまま戻る。その後の raw policy は戻った placement で解き直し、candidate epoch の host fact は適用しない |
| F12 窓の F11（同一 core） | `app.rs::apply_detached_viewer_borderless_target` の applied 境界から、同じ exact host / generation の fact を送る。既存 queue の発行順を保ち、後で作った metadata 更新も同じ現 applied 値を読む。resize や UI tick で旧値を再推定しない |

native 側が exact target lease を受け取れるよう、既存 config / prepare payload と chrome snapshot の型を延長する。
これは既存 target contract に事実を載せる変更で、placement reducer の phase / effect / commit / retire / abort 条件は変更しない。
host 未確定なら既存 prepare の host 待機境界に従い、抑制専用の待機を作らない。
host fact の absence / mismatch は raw policy の全破棄や別窓の fact による fallback の理由にしない。

## 5. 描画・予約・ポインタ領域を同じ結果へ接続する

### 静止画 / 本 / egui 面

- `fullscreen_top_bar_locked_for_idx` と上 HUD の `StillTopBarVisibilityInputs` に同じ実効 lock を渡す。
  `draw_fullscreen_top_bar`、通常 render、上 HUD 判定を複製した経路を棚卸しする。
- `still_seek_geometry_for_idx` と `draw_fullscreen_seek_overlay_with_geometry` の lock 入力を揃える。
  `still_seek_geometry_for_navigation_chrome` / `fs_navigation_gap_layout` / shell も同じ解決を使う。
  strip 高さ・通常シークバー併用設定・固定 gap の既存計算はそのまま利用する。
- 右は `still_info_panel_lock_effective_for_idx` / `locked_info_panel_reserved_width_for_eligibility` と
  `draw_metadata_panel_inner` / navigation shell のコピー投影を揃える。元の `fs_info_panel.locked` は変更しない。
  通常 panel と shell の `visible`、`side_panel_visible`、click sink、wheel / drag 抑止に別の raw-lock 判定を残さない。
  Q4 の推奨案では `open` / `hover_active` の入場 reset と `on_display_target_changed` の変更は行わない。
  明示 open は既存 lifecycle に従って残ることを利用者へ説明する。Q4 の別案が選ばれた場合だけ
  target-change を含む単一の typed open owner の設計へ戻し、raw lock は保持する。
- メディア矩形から fit、見開き、連結、回転、zoom、pan、ルーペ、navigator、preview、PDF display target まで
  同じ geometry を渡す。画像デコード / 補正 / AI resource を F11 抑制のために reset しない。
- Q5 で音楽ビューの右 lock を含める場合は `draw_fs_music_view` の右 panel 表示 / 予約 / waveform hit、
  [ui_music_panels.rs](../src/ui_music_panels.rs) の `draw_fs_music_right_panel` / sink / 鍵表示も同じ結果へ接続する。
  動画→音声の hidden presenter の placement を音楽 viewport の F11 判定の代用にしない。
- 隠れた HUD / strip / panel の `Area`、背景 sink、wheel 捕捉、canvas blocker は登録しない。
  召喚用の端判定は残すが、不可視のパネル全体を hit rect として残さない。
  ClickToShow の callout と touch handle は実際に描いた矩形だけが操作を受ける。

### 5.1 右パネルの鍵と × を分ける（R2・指摘3）

静止画の `explicit`、native 動画の `click_to_show`、音楽の `music_side_panel_close_visible` という
既存の mode / open 条件は維持する。これらに掛ける **lock による close 禁止だけを effective_info_lock** にする。
鍵アイコン・tooltip・toggle は raw lock、予約・配置・close 可否は effective lock を使う。
raw lock ON / effective OFF でも、明示的に一時表示した右 panel は従来の mode の × で閉じられる。

- 静止画: `begin_metadata_panel_frame` と navigation shell のタイトル部で `explicit && !effective_info_lock`。
- native: `draw_native_metadata_panel` に raw と effective を別の意味入力で渡し、
  `click_to_show && !effective_info_lock` で × とその hit / HUD region を生成する。
- 音楽（Q5 採用時）: `draw_fs_music_right_panel` の close predicate を effective へ接続し、
  raw の鍵描画 / toggle とは分ける。

close は context の `close_fullscreen_info_panel` が所有する `open=Closed` / `hover_active=false` と、
該当 presenter の tag picker / transient の close に限る。**raw lock を解除せず、Settings に保存しない。**
native の現 × は `ToggleClickInfoOpen` を返すため、close intent は明示 close として App の既存 close owner へ
収束させる（必要なら native command / event を close 専用にする）。遅延した × を再openの toggle にしない。
以後の描画・sink・wheel・HUD region は閉じた結果に従い、F11退出で raw lock による固定表示へ戻る。
通常 Hover の端だけによる表示へ新しい × を足す、という mode 変更はしない。

### native 動画: `src/video/native_presenter/` を実装面に含める

raw lock を effective に置き換えて `NativeBarLockState` だけ送る案では、鍵の見た目と toggle 基準を失う。
バーと右パネルを別 command で抑制更新すると、間の frame で描画と映像予約が割れる。
**既存の bar / side-panel snapshot を一つの typed chrome snapshot にまとめる案**を推奨する。

- App の共通 factory（現 `native_bar_lock_state` / `sync_native_video_metadata`）で raw bar lock、
  context-owned info state / mode、対象方針、identity付き detached host fact を収集する。
  生成 config、更新 command、placement candidate の初期化が同じ raw policy と §4.1 の適用時 resolver を使う。
  通常 native placement からの F11 判定は App factory の役割にしない。
  既存 `SetBarLockState` / `SetSidePanelState` の可変な二重更新は置換し、互換 owner を並置しない。
- renderer は一つの snapshot を適用してから実効値・eligibility・geometry を解く。
  `NativeRenderCore::set_overlay_bar_lock_state` / side-panel reservation の再計算をこの境界へまとめる。
  `video_top_bar_locked` / `video_bottom_lock` を raw と effective の別可変キャッシュとして増殖させない。
- `NativeEguiOverlay` の `hud_visible` / `top_bar_visible` / `right_panel_visibility_inputs`、
  `VideoSeekGeometry`、`video_content_rect_points`、`VideoVisualLayout` と DComp transform を揃える。
  VST compact は予約解放後の media rect から解く。presenter HWND 自体は縮めない。
- `overlay_draw.rs` の鍵アイコン / tooltip は raw lock を示し、配置と separator は実効 lock に従う。
  `ToggleBarLock` / strip lock / info lock event は raw owner へ戻す。event の source epoch / presenter generation
  検査は既存経路を維持し、F11 専用の遅延棄却条件を作らない。
- 描いた visibility snapshot と geometry から `compute_hud_regions` / `SetWindowRgn` を作る。
  fullscreen の独立 HUD HWND と、detached child の通常 overlay の双方を対象にする。
  隠れた領域は OS ポインタを奪わない。popup の実描画 rect、touch help、既存 capture-all の所有中例外は残す。
- `DetachedViewerChild` は placement enum だけでは F11 を区別できないので、exact host の applied borderless
  入力も §4.1 の identity に対応付けて snapshot に載せる。F11 の host 拡大 / 復元時は既存 transition 適用境界で同期する。
  通常 main / parked owner の metadata poll が active detached の抑制値を送らないことを検証する。
- strip の一時非表示は既存の最終成功 present の `Hidden`、再表示は `Visible { window }` へ投影する。
  `CloseSeekStrip`、設定 OFF、worker cancel / decoder 再生成へ変換しない。
  resource session / mode / LRU を保ち、非表示中の新規 request / poll / periodic repaint は既存 Suspended 契約を使う。
  再表示は lifecycle と exact layout window を同じ final-present snapshot で返す。

## 6. 端ホバー・touch・popup・drag・明示操作

Q4 の推奨案では、抑制は **lock が与える表示理由だけ**を外す。その他の入力 owner を先に解き、
既存の mode / modal gate を保つ。同一 core と core 再生成の境界を区別する。

- 上端 / 下端 hover の閾値・維持帯、side panel に連動する上下表示は既存どおり。
  既に端へポインタがある場合、F11 入場で直ちに自動表示されても正しい。強制的にカーソルを動かさない。
- 右 `Hover` は端から panel 内までの既存 latch、`ClickToShow` は callout クリックを使う。
  F11 中だけ別の召喚方式へ切り替えない。ナビゲータを含める場合、隠れた navigator の exclusion は解除する。
- 中央 touch の session-only chrome latch、左右 touch handle、native HUD touch の widget passthrough を維持する。
  抑制中も時間で touch chrome を消さず、promoted mouse を二重に実行しない。
- egui の spread / fit / rotation / overflow と navigator の既存表示維持を使う。
  native 同一 core は下記の owner 接続を追加する。TextEdit / IME 中のキー優先を維持する。
- external D&D、pan / 360、crop / edit / capture、help modal は既存の表示・入力優先を維持する。
  lock の抑制で新たな edge-hover owner を作らない。
- F11 中の鍵クリックは Q6 の回答に従う。推奨は現在の raw lock を通常どおり変更・保存し、
  F11 退出時はその最新値を使う。抑制中も表示選択・strip mode の明示変更は従来の操作として扱う。
- 右 lock と既存の明示 open の重なりは Q4。回答がないまま blanket reset を実装しない。

### 6.1 native 同一 core の操作継続（R2・指摘2）

現 `hud_visible()` は drag owner を見ず、描画終端の `!bottom_hud_visible` は seek gesture / strip drag を消す。
F12 host の F11拡大では core は同じでも、旧下端のポインタが新下端帯から外れる。
したがって「既存の drag 寿命で足りる」という初版の前提を撤回する。

既存 `seek_row_gesture` / `seek_strip_drag_origin` と各 popup の owner を、
**可視候補判定より前**の入力へ接続する。新しい `drag_active` / `keep_hud` bool を保存しない。
下 chrome は開始済み seek / strip 操作、seek-strip / audio-track / speed popup、
上 chrome は panorama projection 等のその面に属する popup によって一時表示を維持する。
mode / modal / source terminal の優先は変えず、単なる hover 消失・lock 抑制・同core resize を terminal にしない。

開始済み pointer / touch drag の move・release / cancel は viewport 外でも既存 owner に届くようにし、
同 pass の終端で一度だけ消費する。F11 / resize と同 frame の release は、見た目の新しい hit rect に
再hitして所有者を変えず、開始 owner のまま完了する。表示する geometry は新 viewport で解くが、
seek gesture / strip の既存 origin と確定規則を勝手に作り直さない。
popup が開いている間は popup rect と必要な HUD を draw / hit / region に含める。
release / cancel / popup close 後は通常の実効 lock・hoverで再計算して隠す。

`compute_hud_regions` の capture-all は配送の仕組みであり、これだけで可視維持を代用しない。
非表示時の既存 cleanup は source / mode / 明示 terminal 等で必要なため一律削除しない。
同coreの継続 owner を先に可視条件へ接続し、lock 抑制で誤って cleanup に入らないようにする。

### 6.2 native core 再生成時の境界（R2・Q7 新規）

通常動画の F11 / placement変更で `NativeRenderCore::new` を通る場合、現実装は popup / drag / egui focus を
candidate に移譲しない。chrome policy snapshot はこれらの入力 owner の所有者にならない。
decoder / source / strip resource session が継続することから、overlay interaction の継続を推論しない。

**Q7 の推奨案は現行の core 境界を維持する。** candidate の drag / popup は初期状態で始め、
成功して旧 core を retire したら旧 overlay の interaction はそこで終了する。新 core に旧押下からの
release / down level を新規 drag として注入せず、既存 window epoch / generation の配送規則に従う。
旧 core が実際に処理済みの seek / 設定 command は維持し、未処理の最終 release による seek 完了を
新たに保証しない。この制限は抑制機能の都合で追加する操作制限ではなく、現 F11 実装の境界である。

prepare失敗・commit前abort・commit後retire前abortでは、既存 protocol が維持 / 復元した旧 core が
入力状態も所有する。Prepare時に旧ownerを一律resetしない。旧epochのrelease / capture-cancel は
旧coreの配送規則で終端し、新epochへ移し替えない。abortを理由に新しいドラッグを再武装しない。

利用者が再生成をまたぐ継続を必要とする場合は、chrome snapshot だけの実装へ進めない。
exact old/new window epoch、source、gesture origin、popup focus / IME、capture と release/cancel の
一度だけの所有移譲を設計し直し、placement target の既存 transition owner 内で合意する。
F11を操作中だけ無効にする、popupを勝手に先に閉じる等の代案も、利用者承認なく採用しない。

## 7. 組み合わせを減らすために検討したこと

| 案 | 採否案と理由 |
| --- | --- |
| 入場で lock を OFF にして退場で復元 | 不採用。保存・実行値の二重 owner、途中の鍵変更、終了 / エラー復元が増え、保存ロック非変更の依頼にも反する |
| F11 専用 bool / restore pending / timer | 不採用。既存の applied presentation を読む派生値で足りる。時間窓や追加 repaint で geometry の不一致を吸収しない |
| F11 中は input を modal にして競合を消す | 不採用。長い保存処理ではなく閲覧表示であり、navigation・端召喚・touch・drag を止めると要求を満たさない |
| 抑制設定変更で viewer / panel を閉じて開き直す | 不採用案。decoder、情報編集、context-local lock、placement を閉じる費用が大きい。O(1) の既存 snapshot 更新で済み、専用の非同期 live rebuild は不要 |
| 下 HUD と strip の選択を一組にする | Q2 の推奨。既存の3値 lock と一体描画を維持し、第4状態と独立 visibility owner を増やさない |
| raw は既存 owner、effective は純 resolver、native 更新は一 snapshot | 採用案。遅延commandもnative driverの現placementで再計算し、host固有factだけ既存identityで照合する。native更新の一体化だけで競合解消とはしない |
| 右 panel の明示openまで抑制開始で閉じる | Q4 の別案。F11入場・設定変更・migrationと操作保護例外後を扱うownerが必要になる。推奨はlock forceだけ外し、既存openを保持して組み合わせを増やさない |
| 再生成するnative coreへdrag/popupも移譲 | Q7の別案。入力epoch・capture・IME/focus・abortまで所有移譲が広がる。推奨は現core境界を維持し、同coreの可視継続だけ補う |

UI thread の追加処理は固定個数の設定・owner 読み取りと geometry 計算に限定する。
同期 I/O、DB 読み込み、folder scan、decode、追加 GPU upload、blocking wait、`try_lock + sleep` を加えない。
抑制だけで worker・cache・texture を cancel / invalidate / drop しない。書き込み失敗の新しい回復機構も必要ない。

## 8. Detached §2 への適合と §11 記録案

**本案は症状パッチではなく構造的な表示責務の修正である、という実装担当の判断。**
利用者が提示した独立 GPT-6.1 Sol / xhigh レビューもこの構造方針へ同意している。
ただし判定は REVISE で、transport / interaction 等の補完後の再レビューとClaudeCodeの合意は未取得。
根拠は、固定表示の force と予約を raw 設定から各描画面で直接読む境界を、exact 表示先に従う共通の
実効値へ揃えることにある。描画だけを隠す guard ではなく、同じ owner の draw / layout / hit を一緒に変える。
detached F11 は exact window binding と既存 applied state を使い、main / sibling の context を変更しない。

- rect 一致捕捉、host_lost、HWND 再生成、focus / z-order、viewport identity、placement 保存先を変更しない。
- App に detached 用 bool / Option を追加しない。F11 の request / applied transition 自体も再設計しない。
- delay / grace / retry / extra repaint / blanket reset / fallback で競合を隠さない。
- context lock の create / mount / mutate / park / resume / target change / close は既存 bundle の所有に残す。
  read-only suppression は sibling の lock / cache / queue / generation を変えない。
- passive snapshot の機能追加、現行 park / resume の仕様変更、リワーク R4 は今回に含めない。

ClaudeCode と独立 Sol reviewer は、§4 の exact surface 解決、§5 の native snapshot、navigation gap、
§6 の transient / interaction 寿命を検討し、「症状パッチではなく構造的修正」と明示的に合意する。
owner を特定できない経路、behavior 削除が必要な経路が残れば実装を始めず設計担当へ返す。

合意後、実装と同じ coherent chunk で [detached 計画 §11](detached-rework-plan.md#11-リワーク外からの変更記録)
に次の内容を記録する。**今回 §11 へ合意済みの行は追記しない。**

> 日付: 合意日。対象: §1.344 F11 中の locked chrome 実効値。  
> 範囲: `app.rs::apply_detached_viewer_borderless_target` のapplied fact同期、既存のpresentation適用・
> preferences確定・F12 migration入口から共通policyを投影する接続。`ui_fullscreen.rs` のsurface / navigation
> continuation / geometry / hit、`app/native_video.rs` と `video/mod.rs` のinitial / deferred update /
> placement target snapshot、`app/presentation_transition.rs` の既存target leaseを運ぶ型境界、
> `video/native_presenter/{render_core,overlay_draw}.rs` のdraw / media reservation / HUD region /
> 同core interaction visibility / 右panel close、`ui_helpers.rs` / `ui_metadata_panel.rs` のinfo lock投影。
> Q5採用時は `ui_music_panels.rs` の右panel close / sinkと音楽render consumerも含む。
> Q4別案採用時だけ `app.rs::close_fs_side_panel_runtime` / `reset_fs_side_panel_runtime_for_file_change`
> （79095行付近）→ `FullscreenInfoPanelState::on_display_target_changed` のtransient寿命接続を追加。
> 実装した入口・consumerだけを最終記録へ列挙し、条件付き範囲を実施済みと書かない。
> 理由: raw lock を変えず、描画窓の既存 F11 applied state から一つの実効値を導出する。
> viewport / host / placement / focus のowner、transitionのphase / effect / commit / retire / abort条件は
> 変えず、既存target identityとapplied境界へchrome factを載せる。症状guardや復元状態は追加しない。
> main・sibling 不変と gap / native region / drag 回帰で所有境界を固定する。  
> 合意者・独立レビュー参照: 実際の合意後に記入。テスト件数・実機状況: 実行した証拠だけを記入。

## 9. 利用者への質問（回答待ち）

1. **Q1: 設定の既定値・共有範囲はどうするか。** 推奨: 初期は全対象 OFF（既存表示を維持）。
   対象選択は静止画と動画で共通にし、F11 相当の HUD / 割り当て変更済み `FsToggleWindowMode`、
   最初から fullscreen で開いた窓にも適用する。媒体別の対象設定が必要か。
2. **Q2: 下 HUD と strip は一組で選ぶことでよいか。** 推奨: 「下部バーとストリップ」の1項目。
   strip だけ隠してバーを残す必要があれば、その限定3値の表示方針へ設計を改める。
   バーだけ隠して strip を固定する指定は、現 lock 契約と一体描画の変更費用が大きい。
3. **Q3: ナビゲータの固定表示も対象候補に含めるか。** 推奨: 別の選択項目として用意し、初期 OFF。
   抑制中も `FsNavigatorHold`（既定 Alt）と開始済み操作は有効にする。
   端ホバーでは呼び出さず既存 hold を使うことも含めて判断してほしい。
4. **Q4【改訂】: 抑制が有効になったとき、右 panel の既存の明示 open も閉じるか。**
   **推奨は lock の表示理由だけを外し、明示 open は維持する案へ変更。** F11入場、fullscreen中の対象設定ON、
   F12 migration、初回openのすべてで同じ純resolverを使い、抑制開始用のtransient resetを作らない。
   tag picker / TextEdit / dragなどの保護操作が終了しても、それだけでは明示openを閉じない。
   raw lock ON では対象変更後も明示 open を保持するため、抑制中に開いたpanelもページ送りだけでは閉じない。
   ×などの明示closeで閉じる（raw lockは保持）。raw lock OFFなら、対象変更時の既存closeが働く。
   このため、lockと明示openの両方で開いていたpanelは、抑制開始だけでは消えない。×は§5.1で使える。
   別案は「既存明示openも閉じ、開始後の再openは許可」。その場合はF11だけに入口を限定せず、
   設定ON・migration・初回openと、保護例外の終了後まで一つのcontext-owned open ownerで扱う必要がある。
   「保護例外として開いたままにする / 終了後に自動で閉じる」もこの同じ質問の判断に含む。
   別案の採用時は、開始前後のopen provenanceと例外終了のtyped遷移を設計・再レビューしてから実装する。
   新しいpending boolやF11入場前openのbackupだけを足す実装には進まない。
5. **Q5: 音楽ビュー（音声・動画→音声）も右 lock の対象でよいか。** 推奨: 右 lock は同じ方針を適用。
   lock のない常時上下 UI は維持する。上下まで隠す要望は別仕様として相談する。
6. **Q6: 抑制中の鍵クリックで通常の lock を変えてよいか。** 推奨: 通常どおり変更し、
   F11 退出後は最新の値を使う。鍵は raw 状態を示し、抑制中は自動表示になる旨を tooltip で説明する。
   F11 中だけ別 lock を持つ案は採らない。
7. **Q7【新規】: native core再生成を伴うF11でも、drag / popupの継続が必要か。**
   推奨: 現行どおり新coreへの移譲は行わず、成功時に旧coreの操作を終了する。
   同じcoreのまま変わるF12窓のF11拡大・設定変更等は§6.1でdrag / popupを維持する。
   通常F11の再生成まで継続させるなら、chrome snapshotを超える入力・capture・focusの所有移譲設計が必要。
   操作中のF11を無効にする案を、継続の代わりに勝手に採らない。

Q1・Q2・Q3・Q5・Q6は既存の質問。R2でQ4の範囲・推奨を改訂し、Q7を新規追加した。
R3ではQ4の明示openの終了条件だけを説明修正。新規質問はない。
Q4とQ7は表示理由・操作保証の利用者仕様の質問であり、未回答のまま実装を開始しない。
上記の回答と structural agreement を coordinator がまとめてから、bounded 実装 handoff を作る。

## 10. 実装・文書・検証の handoff

### coherent chunk と担当境界

1. 利用者回答を記録し、実効 lock と transient の契約を確定。ClaudeCode と独立 reviewer が構造設計を合意する。
2. 一人の実装担当が共通 resolver、設定、egui / native の全 consumer を所有する。
   native transport の集約を先に行い、片側描画だけの途中状態を出荷しない。
3. 設定を追加するなら `Settings` の serde 欠落値は OFF とし、旧 settings.db の保存 lock / strip 選択 / unknown 値を
   維持する読み替えと roundtrip を検証する。既存 lock は出荷済みデータなので破壊変更しない。
   `settings_db.rs` の既存 `settings_kv` のkey/value保存で新しい選択値を追加できるため、本案のDDL変更・
   schema version更新は不要。欠落値の互換読込・保存・export/importの回帰は必要。
   `settings_transfer.rs` の field 分類、export / import、preferences draft → OK のコピーも漏らさない。
4. 同時更新: 本書の決定・実装記録、`docs/spec.md`、右 panel 計画 §6.6、表示 / 動画アーキテクチャ、
   keymap 仕様（既存 F11 操作の意味）、strip 寿命文書への関連付け、`docs/README.md`、バックログ、
   `htdocs/mimageviewer/manual/` と製品ページ。§11 は実際の合意・変更範囲を記録する。
   通信・Remote protocol は変えない。Remote / headless を PC の F11 snapshot の consumer にしない。

### 必要な自動回帰（実装時。今回は未実行）

| 層 | 観点 |
| --- | --- |
| 純 resolver | 対象 OFF / ON、raw lock OFF / ON、MainEmbedded / Fullscreen / detached normal / borderless、BottomBarLock 全3値。元の状態が不変であり、exit 後は最新 raw に戻る |
| App / context | main + detached A/B、main を mount したまま A を描く pass、A borderless でも main / B 不変、open / switch / close / cancel / error / park / resume、F12 migration と context promotion。lock、items generation、cache、worker identity が sibling へ漏れない |
| navigation | 通常 / embedded / separate / detached の content・holdover・gap shell、Ctrl+↑↓ / sibling / sidecar wait 中にも同じ effective geometry。loading の `None` で予約が復活しない |
| geometry / hit | 上・下・右予約と gap 解放、短い viewport、DPI / UI倍率、strip heights、show / hide / waveform、見開き / 連結 / 回転 / zoom。隠れた sink / wheel / edge exclusion なし、再表示の drawn rect と hit の一致 |
| input | 端 hover、Hover / ClickToShow、touch latch / handle、popup / tag picker / IME、navigator hold解除後のdrag完了、external D&D / modal / motion mode優先。Q4の選択案をF11・設定ON・F12 migration・初回open・保護例外終了で固定 |
| panel close | 三媒体でraw ON / effective OFF / 明示openの×が描画・hit / HUD region内にあり、一度のcloseでopen / hover / tag pickerを閉じる。raw lockは不変、非表示sinkなし、F11退出で固定表示へ戻る。raw ON / effective ONの従来×禁止、Hover端だけ表示の既存条件も維持 |
| native transport | initial / prepare candidate / delayed updateが適用先placementで再計算。Prepare→旧surface由来raw更新→Commit→Retireで抑制を旧値へ戻さない。prepare failure、commit前abort、commit後retire前abortでも復元されたplacementで解く。old generation / 別host incarnationのfactはtargetを上書きせず、raw設定更新は失わない |
| native interaction | 同coreのF12 F11拡大でポインタが新下端帯外でもseek gesture / strip dragが可視維持され、viewport外release / cancelを一度だけ処理。popupをpointer移動・設定ONで消さず、close後は通常の可視性へ。core再生成の成功はQ7の終端契約、abortは旧coreにownerが残り、新epochにreleaseを誤配送しない |
| native geometry | fullscreen HUD HWND / DetachedViewerChild、draw snapshotとHUD region、DComp media target、lock eventはrawを更新。capture-allだけを操作維持の証拠にしない |
| strip lifetime | F11 hide→Hidden / Suspended→edge reveal→Visible + exact window。session / decoder / worker を再生成しない。source変更 / explicit close だけが既存 terminal に到達 |
| UI / persistence | 新設定行の light / dark snapshot、狭幅とUI倍率、raw鍵表示と抑制説明、旧JSON欠落 / export-import / save roundtrip。既存設定の消失なし |

不具合修正を伴う箇所は、その違反の起点で failing test を先に実行し、正しい理由で赤になることを記録する。
新機能の未実装を赤と呼ぶだけで既存 bug の red 証拠を代用しない。
targeted → **`cargo test -p mimageviewer --lib`（pipe なし、実 exit code）** → `cargo fmt` →
normal / portable core check → glyph lint を実装担当が行う。
共有表示境界の最終検証は `scripts/test-full.ps1` も coordinator と検証担当を決めて行い、有効な結果を再利用する。

```powershell
cargo test -p mimageviewer --lib <targeted-filter>
cargo test -p mimageviewer --lib
cargo fmt
cargo fmt --check
cargo check -p mimageviewer --bin mimageviewer-core
cargo check -p mimageviewer --bin mimageviewer-core --features portable
python scripts/check_ui_glyphs.py
```

広い test は最低15分、check / build は最低10分の実行猶予を確保し、timeout 後の harness error を製品失敗としない。
実装完了後に automated gate と独立レビューを通し、`scripts/build-dev.ps1` で通常 profile の確認用 core を用意する。
この work line の指示に従い、**エージェントは製品バイナリを一切起動しない**。
native / 実機確認は利用者が行い、installed / tray 常駐を先に閉じること、通常 `%APPDATA%\mimageviewer` を使い
実データを更新し得ることを添えて `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を渡す。
確認シナリオは、各 lock を通常窓でON → F11で余白解放 → 端 / touchで召喚 → popup / drag → F11退出でlock表示、
静止画・ZIP/PDF・動画場面/波形・F12窓・別窓不変・navigation待ち、右panelの×、設定ONとF12 migration、
同coreのresize中drag / popupと通常F11の再生成境界、Q4 / Q7で決定した挙動を含める。

今回の証拠: source inspection と文書設計のみ。自動テスト0件、build未実行、製品起動なし。
初版は独立レビューREVISE、本改訂の独立再レビュー・実機確認は未実施。
本 worktree で commit / `.git` 書込は行わない。初版メッセージは `target/E-1344-design-msg.txt`、
HEAD上の追補コミットメッセージ案は `target/E-design-r2-msg.txt` に置く。

## 11. R2 独立設計レビューの指摘と処置

レビュー元は利用者が提示した別 GPT-6.1 Sol / xhigh セッションの結果。判定は REVISE。
5件とも現HEADのコードと照合し、指摘を受け入れた。反論・未修正扱いの指摘はない。
構造方針への同意は記録するが、本改訂の承認や実装独立レビュー済みとは書かない。

| 指摘 | コード照合 / 改訂先 |
| --- | --- |
| 1 P2: delayed snapshotの適用先 | `video/mod.rs` の2193 / 6109 / 6322行付近とretire待ち後abortを読んだ。§4.1でraw policyとexact host factを分け、適用時placement、candidate初期化、二つのabort、generation/leaseの照合、raw更新を失わない規則を追加 |
| 2 P2: drag / popup寿命の前提 | `render_core.rs` の11723 / 14410 / 9197行付近、`video/mod.rs` の6109行付近を読んだ。§6.1で同coreのdrag ownerを可視入力へ接続し、§6.2で再生成時の終了 / abort復元を分離。Q7新規と回帰を追加 |
| 3 P2: 右panelの× | `ui_metadata_panel.rs` の2159行付近、`overlay_draw.rs` の5943行付近、`ui_music_panels.rs` の758行付近とApp close handlerを読んだ。§5.1で鍵raw / close effective、mode条件維持、closeのraw不変、明示closeの配送と三媒体回帰を追加 |
| 4 P2: 抑制開始の別入口 | `ui_helpers.rs::visible`、`app.rs::apply_detached_viewer_borderless_target` / target-change、native snapshot同期を読んだ。Q4を全入口・保護例外終了を含む一問へ改訂し、reset不要のlock-forceのみ案を推奨。別案はtyped open ownerの再設計が前提と明記 |
| 5 P3: detached §11範囲 | `app.rs` の60410 / 79095行付近を読んだ。§8の記録案にapplied / migration / 設定入口、target lease payload、Q4別案のtarget-changeとQ5の音楽consumerを追加。既存transition owner / phase / effectを変更しない境界も明記 |
