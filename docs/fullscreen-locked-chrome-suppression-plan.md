# F11 全画面中のロック済みクローム一時抑制 (§1.344)

作成: 2026-10-07 / ライン E / `next-viewer` / 調査基点 `d29bcfbec`。  
状態: **設計案のみ。利用者の未決事項・ClaudeCode の構造合意・独立レビューは未完了。**

## 1. 依頼と確定済みの境界

[次版バックログ](next-release-backlog.md) §1.344
の「次の版の決定 (利用者 2026-10-07)」と今回の作業指示を正本にする。
E は §1.344 → §1.342 → §1.341 → §1.340 の順で、できた分を出荷する。
今回は §1.344 の設計だけで、後続の表示方式・アニメーション・タイル一覧は扱わない。

- ウィンドウ表示のロックを保ち、**描画対象の窓が F11 全画面の間だけ**固定による常時表示を抑制する。
- 保存済み HUD / strip のロックを F11 入退場で書き換えない。右情報パネルの context-local なロックも維持する。
- F11 を戻すと現在のロック設定に従う。入場時の値を保存して復元する方式にはしない。
- 通常の自動表示と同じ召喚・操作を残す。画面端ホバー、touch、popup、操作中の drag を禁止する機能ではない。
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
native candidate はその候補の target placement から初期 snapshot を作り、旧 committed surface の値を
流用しない。候補が失敗したら旧 renderer は旧 snapshot を保持する。既存 transition owner の成功 / 失敗を使い、
本機能独自の rollback、世代、ack 待機を追加しない。

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
  Q4 を確定したら `on_display_target_changed` の transient 寿命も実効 lock で解く。F11 抑制中の明示 open が
  raw lock を理由に次ページでも保持されないようにし、raw lock 自体は保持する。
- メディア矩形から fit、見開き、連結、回転、zoom、pan、ルーペ、navigator、preview、PDF display target まで
  同じ geometry を渡す。画像デコード / 補正 / AI resource を F11 抑制のために reset しない。
- Q5 で音楽ビューの右 lock を含める場合は `draw_fs_music_view` の右 panel 表示 / 予約 / waveform hit、
  [ui_music_panels.rs](../src/ui_music_panels.rs) の `draw_fs_music_right_panel` / sink / 鍵表示も同じ結果へ接続する。
  動画→音声の hidden presenter の placement を音楽 viewport の F11 判定の代用にしない。
- 隠れた HUD / strip / panel の `Area`、背景 sink、wheel 捕捉、canvas blocker は登録しない。
  召喚用の端判定は残すが、不可視のパネル全体を hit rect として残さない。
  ClickToShow の callout と touch handle は実際に描いた矩形だけが操作を受ける。

### native 動画: `src/video/native_presenter/` を実装面に含める

raw lock を effective に置き換えて `NativeBarLockState` だけ送る案では、鍵の見た目と toggle 基準を失う。
バーと右パネルを別 command で抑制更新すると、間の frame で描画と映像予約が割れる。
**既存の bar / side-panel snapshot を一つの typed chrome snapshot にまとめる案**を推奨する。

- App の共通 factory（現 `native_bar_lock_state` / `sync_native_video_metadata`）で raw bar lock、
  context-owned info state / mode、対象方針、exact surface の F11 入力を収集する。
  生成 config、更新 command、placement candidate の初期化が同じ factory / resolver を使う。
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
  入力も snapshot に載せる。F11 の host 拡大 / 復元時は既存 transition 適用境界でこの snapshot を同期する。
  通常 main / parked owner の metadata poll が active detached の抑制値を送らないことを検証する。
- strip の一時非表示は既存の最終成功 present の `Hidden`、再表示は `Visible { window }` へ投影する。
  `CloseSeekStrip`、設定 OFF、worker cancel / decoder 再生成へ変換しない。
  resource session / mode / LRU を保ち、非表示中の新規 request / poll / periodic repaint は既存 Suspended 契約を使う。
  再表示は lifecycle と exact layout window を同じ final-present snapshot で返す。

## 6. 端ホバー・touch・popup・drag・明示操作

抑制は **lock が与える表示理由だけ**を外す。その他の入力 owner を先に解き、既存の mode / modal gate を保つ。

- 上端 / 下端 hover の閾値・維持帯、side panel に連動する上下表示は既存どおり。
  既に端へポインタがある場合、F11 入場で直ちに自動表示されても正しい。強制的にカーソルを動かさない。
- 右 `Hover` は端から panel 内までの既存 latch、`ClickToShow` は callout クリックを使う。
  F11 中だけ別の召喚方式へ切り替えない。ナビゲータを含める場合、隠れた navigator の exclusion は解除する。
- 中央 touch の session-only chrome latch、左右 touch handle、native HUD touch の widget passthrough を維持する。
  抑制中も時間で touch chrome を消さず、promoted mouse を二重に実行しない。
- spread / fit / rotation / overflow、seek-strip / speed / audio-track menu、tag picker は既存の表示維持 owner。
  popup を F11 のために閉じたり、不可視の backdrop だけ残したりしない。TextEdit / IME 中のキー優先も維持する。
- seek / strip / navigator の開始済み drag は release / cancel まで同じ owner が処理する。
  F11 と同 frame の release を失わず、release 後に新規 drag を合成しない。
  native `SetCapture` / capture-all region の既存寿命を維持し、非操作中だけ region を通常の可視形へ戻す。
- external D&D、pan / 360、crop / edit / capture、help modal は既存の表示・入力優先を維持する。
  lock の抑制で新たな edge-hover owner を作らない。
- F11 中の鍵クリックは Q6 の回答に従う。推奨は現在の raw lock を通常どおり変更・保存し、
  F11 退出時はその最新値を使う。抑制中も表示選択・strip mode の明示変更は従来の操作として扱う。
- 右 lock と既存の明示 open の重なりは Q4。回答がないまま blanket reset を実装しない。

## 7. 組み合わせを減らすために検討したこと

| 案 | 採否案と理由 |
| --- | --- |
| 入場で lock を OFF にして退場で復元 | 不採用。保存・実行値の二重 owner、途中の鍵変更、終了 / エラー復元が増え、保存ロック非変更の依頼にも反する |
| F11 専用 bool / restore pending / timer | 不採用。既存の applied presentation を読む派生値で足りる。時間窓や追加 repaint で geometry の不一致を吸収しない |
| F11 中は input を modal にして競合を消す | 不採用。長い保存処理ではなく閲覧表示であり、navigation・端召喚・touch・drag を止めると要求を満たさない |
| 抑制設定変更で viewer / panel を閉じて開き直す | 不採用案。decoder、情報編集、context-local lock、placement を閉じる費用が大きい。O(1) の既存 snapshot 更新で済み、専用の非同期 live rebuild は不要 |
| 下 HUD と strip の選択を一組にする | Q2 の推奨。既存の3値 lock と一体描画を維持し、第4状態と独立 visibility owner を増やさない |
| raw は既存 owner、effective は純 resolver、native 更新は一 snapshot | 採用案。create / mutate / draw / hit / reserve で別々の force を作らない。F11 を抜けると再計算するだけ |
| 右 panel の既存 transient を入場時に区切る | Q4 の推奨。必要なら既存 open owner の明示 lifecycle として扱い、入場前 open の backup / restore を新設しない |

UI thread の追加処理は固定個数の設定・owner 読み取りと geometry 計算に限定する。
同期 I/O、DB 読み込み、folder scan、decode、追加 GPU upload、blocking wait、`try_lock + sleep` を加えない。
抑制だけで worker・cache・texture を cancel / invalidate / drop しない。書き込み失敗の新しい回復機構も必要ない。

## 8. Detached §2 への適合と §11 記録案

**本案は症状パッチではなく構造的な表示責務の修正である、という実装担当の判断。合意はまだ未取得。**
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
> 範囲: `ui_fullscreen.rs` の surface / navigation continuation / geometry / hit、
> `app/native_video.rs` と `video/mod.rs` の initial / update / placement chrome snapshot、
> `video/native_presenter/{render_core,overlay_draw}.rs` の draw / media reservation / HUD region、
> `ui_helpers.rs` / `ui_metadata_panel.rs` の context-local info lock 投影。  
> 理由: raw lock を変えず、描画窓の既存 F11 applied state から一つの実効値を導出する。
> viewport / host / placement / focus の owner は変えず、症状 guard や復元状態は追加しない。
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
4. **Q4: F11 入場前から右 panel が明示 open だった場合も隠すか。** 推奨: 対象指定かつ raw lock ON の
   右 panel は、F11 の適用時に既存 transient open / hover の表示理由を区切り、lock は保持する。
   開いている tag picker / TextEdit / touch・pointer drag は既存の操作終了・明示 close まで維持する。
   その例外では入場時の強制非表示を求めず、新しい遅延 close pending は作らない。
   その後の端・callout・touch での open は許可する。単に lock force だけ外す案では既存 open が残り、
   F11 入場で panel が消えない場合がある。利用者判断と owner ごとの実装設計が必要。
5. **Q5: 音楽ビュー（音声・動画→音声）も右 lock の対象でよいか。** 推奨: 右 lock は同じ方針を適用。
   lock のない常時上下 UI は維持する。上下まで隠す要望は別仕様として相談する。
6. **Q6: 抑制中の鍵クリックで通常の lock を変えてよいか。** 推奨: 通常どおり変更し、
   F11 退出後は最新の値を使う。鍵は raw 状態を示し、抑制中は自動表示になる旨を tooltip で説明する。
   F11 中だけ別 lock を持つ案は採らない。

Q4 は表示理由の残り方を含む利用者仕様の質問であり、未回答のまま実装を開始しない。
上記の回答と structural agreement を coordinator がまとめてから、bounded 実装 handoff を作る。

## 10. 実装・文書・検証の handoff

### coherent chunk と担当境界

1. 利用者回答を記録し、実効 lock と transient の契約を確定。ClaudeCode と独立 reviewer が構造設計を合意する。
2. 一人の実装担当が共通 resolver、設定、egui / native の全 consumer を所有する。
   native transport の集約を先に行い、片側描画だけの途中状態を出荷しない。
3. 設定を追加するなら `Settings` の serde 欠落値は OFF とし、旧 settings.db の保存 lock / strip 選択 / unknown 値を
   維持する読み替えと roundtrip を検証する。既存 lock は出荷済みデータなので破壊変更しない。
   既存 DB の汎用設定保存で新しい選択値を追加できるなら schema version は不要だが、DDL 変更が必要なら移行必須。
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
| input | 端 hover、Hover / ClickToShow、touch latch / handle、popup / tag picker / IME、F11 と release 同 frame、navigator hold解除後のdrag完了、external D&D / modal / motion mode 優先。Q4 の入場前 open と入場後 open を区別 |
| native | initial config / update / candidate placement の共通値、fullscreen HUD HWND / DetachedViewerChild、draw snapshot と HUD region、DComp media target、failed switch は旧契約、lock event は raw を更新 |
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
静止画・ZIP/PDF・動画場面/波形・F12窓・別窓不変・navigation待ち・Q4の右openを含める。

今回の証拠: source inspection と文書設計のみ。自動テスト0件、build未実行、独立レビュー未実施、実機未確認。
本 worktree で commit / `.git` 書込は行わない。コミットメッセージ案は `target/E-1344-design-msg.txt` に置く。
