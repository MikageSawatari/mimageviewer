# findings-19: ON モードのアクティブ切替が close+reopen 往復で窓を再構築しており、高速切替で窓が消える (2026-07-08)

報告: ship-checklist v2 の W1/W2 実施中 (ユーザー実機)。
ログ: scratchpad/w1-cur.log (MIV_DETACHED_WINDOW_DEBUG=1、3780〜3812s 付近)。

## 症状

- **A (P1)**: ON モードで窓を複数開き順にアクティブ化を繰り返すと、別の窓が**消える**ことがある。
- **B (P2)**: アクティブにした瞬間、その窓の**画像がすこしだけ移動**する (毎回)。

## A の機構 (ログで確定)

### 正常時でも切替のたびに close+reopen している

アクティブ切替 1 回ごとに、旧 active 窓が以下を辿る (このセッションで
`park_close_legacy_detached` が **74 回** = ほぼ全切替):

```
22: Active → Parked   reason=park_active_detached_image_window
22: Parked → Closing  reason=main_context_change          ← park 直後に close 判定
22: session_finish    reason=main_context_change
22: Closing → Removed reason=park_close_legacy_detached   ← runtime 削除 (OS 窓は孤児で生存)
  (0.7〜1.4 秒後)
22: Opening → Opening reason=deferred (hwnd=0x0)          ← 同じ window_id で再作成 (descriptor)
22: Opening → Parked  reason=hwnd_adopted_deferred hwnd=0x3c27e6  ← 孤児 HWND を養子縁組
                                                            (host_generation++)
```

つまり「旧 active → passive 降格」が **in-place の handoff ではなく、runtime を
close → descriptor から再作成 → 孤児 HWND 再採用**という往復で実装されている
(メディア窓の live-park は `handoff_active_detached_viewport_to_passive` で in-place
降格しており、静止画切替だけがこの churn 経路)。

- `main_context_change` による close は本来 OFF (linked) の規則のはずだが、
  **linked=false の独立窓**に対して毎回発火している (直後の descriptor 再作成が
  補償するため、ゆっくり操作すると見た目は保たれる)。

### 高速切替での破綻 (窓消失の直接原因)

再作成完了 (hwnd_adopted_deferred) 前に次の切替が起きると、**未請求の孤児 HWND が
2 つ以上並ぶ**:

```
3782.917 deferred_activate_watcher_dropped id=23 reason=repair_failed
         observed_hwnd=0x21817ae ... claimed_by=Some(21)
3811.587 hwnd_deferred_retry window_id=23 reason=ambiguous
         candidates=[2808d8, 21b17ae] claimed_count=0 host=hwnd=0   ← 候補 2 つ = 採用不能
3803.8xx deferred_registration_delayed id=21 / id=22
         reason=unconfirmed_hwnd_serialized  ← 毎フレーム交互に飢餓 (R2d の 1frame1窓
                                               直列化が、未確定窓 2 つで永久に回る)
```

- R1b の消去法採用は「未請求がちょうど 1 件」が条件のため、孤児 2 つで **ambiguous →
  hwnd=0 のまま stuck**。
- 未確定窓が 2 つになると R2d の直列化 (1 フレーム 1 窓) が交互に delay し続け、
  `show_viewport_deferred` されないパスが続く → **egui が未登録 viewport を破棄 = 窓が消える**
  (findings-10 と同じ最終形。今回の発生源は clear-on-park ではなく close+reopen の孤児併存)。

## B の観察 (未確定、調査指示あり)

`runtime_placement` は切替前後で **完全に不変** (x/y/w/h に 1px の揺れもなし) →
窓移動ではない。疑い = **passive の凍結スナップショット描画と active の live 描画で
画像コンテンツの配置矩形が数 px ずれる** (fit 計算・バー領域の扱い・ppp=1.5 の丸めの
いずれか)。アクティブ化の瞬間に snapshot → live に切り替わるときズレが見える。

## 修正指示 (fix10) — Phase 1 調査 → Fable 承認 → 実装

1. **Phase 1 (A)**: 静止画独立窓のアクティブ切替経路を特定して報告する:
   - 旧 active 窓が `main_context_change` close に落ちる call site (park_current_active_detached
     / close_legacy_detached / deferred_activate_commit 周辺) と、直後に同 id で再作成している
     機構 (reopen_descriptor)。
   - この close+reopen が**意図された設計か、OFF (linked) 規則の誤発火を descriptor が
     補償しているだけか**を判断する (git 履歴の確認は任意、コード構造からで可)。
2. **実装方針 (A、承認前提の推奨案)**: 切替時の close+reopen をやめ、**旧 active 窓を
   in-place で Parked (deferred) に降格**する — メディア窓の
   `handoff_active_detached_viewport_to_passive` と同じ「OS 窓・viewport id・HWND 登録を
   維持したまま状態だけ落とす」形に静止画も揃える。これで孤児 HWND が存在しなくなり、
   ambiguous / 直列化飢餓 / host_generation 増加が構造的に消える。
   - 憲法 §1/§2: 採用ヒューリスティックの強化 (候補 2 つの解決ロジック追加等) で
     対処**しない**こと。孤児を作らないのが根治。
3. **Phase 1 (B)**: アクティブ化前後で「画像コンテンツの描画矩形」を比較する
   (snapshot 描画の rect と live fit 計算の rect を同条件でログ or テストで突き合わせ)。
   数 px 差の出所 (バー領域 / fit / 丸め) を特定して報告 → 修正。
4. テスト: (A) 切替 2 連続 (再作成完了前に次の切替) のシーケンスで、旧 active 窓の
   HWND 登録が切替をまたいで不変 = 孤児が発生しないことを固定。(B) snapshot/live の
   rect 一致を純関数で固定。
5. コミット: `(detached-rework findings-19 fix10)` (A と B は別コミット可)。

## Phase 1 調査結果 (Codex 2026-07-08)

### A: active switch の close+reopen 経路

実コード上の active switch は、`activate_detached_image_window_snapshot()` が対象 snapshot を一度取り出したあと、現在の active を `park_and_close_current_active_detached_viewer(ctx)` で退避してから対象を復帰する構造になっている。

ON モードの静止画 legacy active では `active_detached_viewer_context` を持たず、`viewer_session_is_detached()` が真になるため、`park_and_close_current_active_detached_viewer()` の legacy 分岐に落ちる。この分岐は次の順で動く。

1. `preserve_active_detached_image_window_for_main_context_change()` を呼ぶ。
2. その中で `park_active_detached_image_window()` が snapshot を `detached_image_windows` へ追加し、`handoff_active_detached_viewport_to_passive("main_context_change")` で同じ ViewportId/HWND を Parked 側へ渡す。
3. 呼び出し元の legacy 分岐へ戻ったあと、直ちに `begin_active_detached_session_close("park_close_legacy_detached")`、`ViewportCommand::Close`、`finish_active_detached_session_close("park_close_legacy_detached")`、`close_fullscreen()`、`remove_detached_window_runtime(window_id, "park_close_legacy_detached")` が走る。

つまり、同じ関数内で「in-place park/handoff に成功した直後に、その viewport/session/runtime を close/remove する」矛盾した遷移になっている。これは `main_context_change` 用の legacy close 経路が、ON モードの independent active switch にも誤って適用されている状態であり、意図した仕様ではなく旧経路の misfire と判断する。

再表示は snapshot 側の復帰情報で補償されている。

- 通常静止画では `reopen_sync_stamp` を使う `resume_still_snapshot` 分岐が `adopt_active_detached_viewport_runtime_from_passive()` と `open_fullscreen(idx)` で同じ window id を復帰しようとする。
- PDF/ZIP/book 系では `reopen_descriptor` 分岐が passive viewport を close し、descriptor から active detached context を作り直す。

どちらも「close+reopen を正当化する設計」ではなく、先に壊した OS 窓/viewport lifetime を descriptor/stamp で復旧している補償経路である。正しい所有境界は、`pause_current_active_detached_viewer_context()` や `park_current_viewer_context_as_live_media_inner()` と同じく、Parked 化時に `handoff_active_detached_viewport_to_passive(...)` で OS 窓を保持し、close/remove を発生させない経路に寄せるべき。

### B: snapshot rect と live active rect の不一致

静止画 snapshot の描画経路は live active の描画経路と同じ入力を使っていない。

`draw_detached_image_window_snapshot()` の単一画像 snapshot は、passive 側で `draw_fs_image(ui, full_rect, ..., FullscreenFitMode::Page, FullscreenFitScaleLimits::default(), None)` を直接呼ぶ。一方、live active 側の `render_fullscreen_viewport()` は、`fullscreen_media_rect(full_rect, fs_idx, is_video)` で seek panel 領域などを除外した `image_rect` を作り、`effective_fullscreen_fit_mode()`、`fullscreen_fit_scale_limits()`、content bbox/trim、zoom/pan/free-rotation などを含む live state で描画する。

そのため、passive snapshot で見えていた画像が active 復帰時に live 描画へ切り替わると、同じ window placement でも fit 入力が数 px 単位で変わり得る。これが「active 切替のたびに画像が少し動く」直接候補である。

見開き/連続表示 snapshot は `detached_spread_frozen_pages_for_snapshot()` / `detached_continuous_frozen_pages_for_snapshot()` で `fullscreen_media_rect(...)` から page rect を正規化しており、単一画像 snapshot より live 経路に近い。ただし snapshot 作成時の placement size と復帰後の実 `full_rect` が違う場合、または seek panel / fit state が変化した場合は同様にズレる余地が残る。

Phase 2 では、A を先に in-place park/handoff へ直し、OS 窓の close+reopen を消したうえで、B は snapshot 側にも live と同じ rect/fit 入力を使わせる、または snapshot に live 描画で使った normalized image/page rect を保存して復帰前後で同一 rect を描く方向で修正するのがよい。回帰テストは、A は repeated active switch で window id/HWND runtime が close/remove されないこと、B は snapshot rect と live rect の純関数比較を固定する。

## 参考: ログ抽出

```powershell
Select-String -Path $log -Pattern 'park_close_legacy_detached|hwnd_adopted_deferred|ambiguous|unconfirmed_hwnd_serialized|repair_failed'
```

## Phase 2 承認 (Fable 2026-07-08)

Phase 1 の判断 (A = legacy close 経路の誤発火、B = snapshot/live の fit 入力不一致) を
**承認**する。A → B の順で実装する (別コミット可: `fix10a` / `fix10b`)。

### fix10a (A) 実装条件

1. ON モードの静止画 active switch では、`park_and_close_current_active_detached_viewer` の
   legacy 分岐が handoff 後に行う **close/remove 一式 (begin/finish close +
   ViewportCommand::Close + close_fullscreen + remove_detached_window_runtime) を発生させない**。
   `pause_current_active_detached_viewer_context` / live-park inner と同じ「handoff で
   OS 窓・ViewportId・HWND 登録を保持したまま Parked 化」で完結させる。
2. **legacy close 経路そのものは残す** — OFF (linked) の main_context_change close という
   本来の用途があるため。分岐は既存の事実 (independent/linked、モード) で判定し、
   新規 bool・ヒューリスティックを足さない (憲法 §3/§6)。
3. **補償機構 (resume_still_snapshot / reopen_descriptor) はこの fix では削除しない**
   (switch 経路から到達しなくなるだけ)。book (PDF/ZIP) 窓の park が paused_bundle /
   凍結ページを保持したまま in-place で成立すること (W8/W9 相当) を確認し、descriptor
   復帰に依存していた箇所があれば報告する。デッドコード化した範囲は実装メモに列挙
   (削除は別途判断)。
4. 回帰テスト: 高速切替 (再作成完了を待たず 2 連続以上) のシーケンスで、
   - 旧 active 窓の HWND 登録が切替をまたいで**不変** (clear/adopt が発生しない)
   - `Removed` 遷移が発生しない / host_generation が増えない
   - メディア窓の live-park (park_current_viewer_context_as_live_media) は不変
5. 実機確認: checklist v2 の W1/W2/W10 (高速切替 10 回+ → 全部閉じる)。

### fix10b (B) 実装条件

1. 方式はどちらでも可 (採った方を実装メモに明記):
   - (推奨) snapshot 作成時に **live 描画で実際に使った normalized image/page rect を保存**し、
     passive 描画は同一 rect で描く (seek panel・fit state・bbox/trim の再現に頑健)。
   - 代替: snapshot 描画に live と同じ入力 (`fullscreen_media_rect` /
     `effective_fullscreen_fit_mode` / `fullscreen_fit_scale_limits` / bbox) を与える。
2. 見開き/連続ページ snapshot も同じ入力源に揃っていることを確認 (既に近いが、
   placement size 変化時のズレ余地を含めて検証)。
3. テスト: snapshot rect と live rect の一致を純関数で固定 (単一画像 + 見開きの 2 ケース)。
4. 実機確認: W2 (アクティブ化の瞬間に画像が動かない)。

## fix10a / fix10b 検収合格 (Fable 2026-07-08)

- **fix10a (2759c4e3) = 合格**。`park_current_active_detached_viewer` の legacy 分岐冒頭で
  `preserve_active_detached_image_window_for_main_context_change()` 成功時に即 return =
  handoff 後の close/remove 一式が発生しない。preserve のゲート
  (`should_preserve_...` = `detached_viewer_open_images_in_window` 設定 + still 対応 idx +
  fs_nav 非ロック) により **OFF (linked) は従来どおり close_legacy_detached に落ちる** =
  条件 2 充足 (既存事実で分岐、新規 bool なし)。handoff 後に
  `transition_detached_window_state(Parked, "main_context_change_handoff")` で状態機械も整合。
  テスト = 切替後に旧窓が同 id で Parked・HWND 生存 (0x2222)・host_generation 不変。
- **fix10b (adf65c62) = 合格**。**推奨案 (park 時に live の normalized rect を保存) を採用**:
  `detached_single_image_snapshot_layout` が live 入力 (`fullscreen_media_rect` /
  `effective_fullscreen_fit_mode` / `fullscreen_fit_scale_limits` / content bbox /
  rotation / zoom_pan / free_rotation) から draw rect を計算して `image_rect_norm` +
  `image_content_bbox` を snapshot に焼き込み、passive は
  `draw_detached_frozen_image_at_rect` で同一 rect 描画。fit 数式は純関数
  (`fs_image_draw_rect_for_size` 等) に切り出され live/passive の単一ソース。
  deferred view から `zoom_pan` を撤去 (rect に焼き込み済み)。テスト = 単一 + 見開きの
  rect 一致 (`detached_single_snapshot_rect_matches_live_fit_input` /
  `detached_spread_snapshot_rects_match_live_fit_input`)。
- 軽微指摘 (作業不要): 実装メモの本 doc 追記 (fix10a のデッドコード化範囲列挙) が未実施。
  補償機構 (resume_still_snapshot / reopen_descriptor) の到達性整理はリリース後の
  クリーンアップ候補としてここに記録する。
- 実機確認 (ユーザー): W1/W2/W10 (高速切替・全閉じ) + アクティブ化瞬間の画像静止。

## fix11 (実機 FB 2026-07-08): 非アクティブ窓で OS の × ボタンが効かない

**症状**: passive (非アクティブ) 静止画窓のタイトルバー × をクリックしても閉じない。
1 回クリックでアクティブ化した後なら閉じられる。ユーザー判断 = 直感に反するので修正する。

**機構 (Fable 見立て、Phase 1 で確認)**: OS の × は非アクティブ窓にも WM_CLOSE を送り、
コードは deferred/immediate 両 passive 経路とも `viewport_close_requested` を配線済み
(ui_fullscreen.rs 4494/4935)。効かない原因の有力候補:
(a) **deferred viewport のイベント配送欠落** (findings-8 A1 と同じ配送路) が
close_requested にも及ぶ、(b) × の物理 down を watcher がタイトルバー click と解釈して
activation に変換 (G1 層別) し、close にならない。

### fix11 要件 — 短い Phase 1 → 実装

1. **Phase 1 (計装 or コード確認、1 ラウンド)**: 非アクティブ窓の × クリック時に
   deferred 側の `viewport_close_requested` が true で観測されるかを確認 (probe 可。
   `MIV_DETACHED_WINDOW_DEBUG` ゲート + イベント時のみ出力)。観測されるなら race/処理順、
   されないなら配送欠落と確定。
2. **実装 (どちらかを採用、採った方と理由を実装メモに記載)**:
   - **案 A**: watcher の down/up に `WM_NCHITTEST == HTCLOSE` 判定を追加し、× 上の
     down→up は activation でなく既存 close intent (G2 経路) に変換する。判定は OS への
     問い合わせのみ (ジオメトリ計算・DPI 補正を自前でしない)。cross-thread
     `SendMessageW` のブロッキングが気になる場合は `SendMessageTimeoutW` 小 timeout 可
     (時間窓ではなくデッドロック回避の下限)。
   - **案 B**: 登録済み detached HWND に軽量 subclass を張り、WM_CLOSE (または
     WM_SYSCOMMAND/SC_CLOSE) を既存 close intent チャネルへ変換する
     (`install_main_window_subclass` と同パターン)。install/uninstall は HWND registry の
     登録/クリアに同期。
3. どちらの案でも: 既存の close 優先処理 (close と activation が同時に来たら close 勝ち) と
   dedup を再利用。アクティブ窓・メイン窓の × は不変。ParkedLive (メディア) 窓の × も
   同じ経路で閉じることを確認。
4. テスト: × 由来の close intent が activation より優先されること (純関数 or シーケンス)。
5. コミット `(detached-rework findings-19 fix11)`。

## fix11 Phase 1 / 実装メモ (Codex 2026-07-08)

コード確認で次を確定した。

- deferred/immediate passive 経路は `viewport_close_requested` を読む配線自体はある
  (`ui_fullscreen.rs` の deferred event / parked-live render)。ただし findings-8 A1 と同じく、
  deferred viewport では OS 側イベント配送が信頼できない前提が既にある。
- 既存の `DetachedActivationWatcher` は close intent を持っていたが、判定対象は
  mIV が描く独自バー (`detached_image_window_bar_close_button_rect`) / music chrome の
  close slot であり、Windows の非クライアント caption button そのものではなかった。
  そのため OS のタイトルバー × を押した場合、`viewport_close_requested` が届かなければ
  watcher は activate と close を因果的に識別できない。

採用案は **案 A**。理由:

- 非アクティブ passive 窓の物理クリックを App の intent へ変換する所有境界は既に
  `DetachedActivationWatcher` に集約されている。ここへ OS の
  `WM_NCHITTEST == HTCLOSE` を足すのが最小変更で、subclass の install/uninstall
  ライフタイムを増やさずに済む。
- `SendMessageTimeoutW(..., WM_NCHITTEST, ..., 50ms)` を使う。これは時間窓で競合を
  吸収するものではなく、別スレッド window への問い合わせが hung したときの
  デッドロック回避である。

実装:

- watcher の OS sample に `native_close_hit_hwnd` を追加し、カーソル下 root HWND に
  `WM_NCHITTEST` を投げて `HTCLOSE` ならその HWND を記録。
- down/up が同じ native close hit 上で完結したときだけ `DetachedCloseRequest` を発行する。
  release が × から外れた場合は既存の `up_close_outside` としてキャンセル。
- close と activation が同時候補になる場合は既存どおり close 勝ち。
- OS × は activation readiness と独立した window manager 操作なので、
  `activation_ready_frame` 前の passive 窓でも close intent として扱う。

回帰テスト:

- `activation_watcher_native_caption_close_sends_close_not_activation`
- `activation_watcher_native_caption_close_release_outside_is_ignored`
- `activation_watcher_native_caption_close_works_before_activation_ready`

## fix11 検収合格 + 実機 OK (Fable 2026-07-08)

**fix11 (285ad4f7) = 合格**。案 A (watcher に `WM_NCHITTEST == HTCLOSE` 判定) を採用理由付きで
実装。`SendMessageTimeoutW(SMTO_ABORTIFHUNG, 50ms)` はデッドロック回避 (時間窓ではない) と
明記あり ✓。LPARAM の座標 packing は負座標 (マルチモニタの y<0) でも Win32 規約どおり ✓。
down/up とも × 上で完結したときのみ close intent、外して release はキャンセル、
close > activation 優先、`activation_ready_frame` 前でも close 可 (window manager 操作として
妥当)。テスト 3 本。**実機 = 非アクティブ窓の × で即 close 確認済み (ユーザー)**。

P4 (任意、次回ついで可): watcher の OS sample が「カーソル下の任意の root HWND」へ毎回
WM_NCHITTEST を送る。登録済み detached HWND のときだけ問い合わせるようゲートすると、
無関係ウィンドウ (他プロセス含む) へのメッセージ送信が消えて僅かに安全・軽量になる。

## fix12 (実機 FB 2026-07-08、checklist V1): 音楽ビューの VST ボタンが detached で表示される

**症状**: 複数ウィンドウ (ON) モードのメディア窓で、動画→音声モード / 音声ファイル再生の
上部バーに VST ボタンが出る。クリックすると不適切なエラー。native 動画 HUD の VST ボタンは
出ない (正しい)。

**機構**: native 側は `presentation == Fullscreen` 限定 (app.rs:45403-45408) だが、音楽
chrome の `show_vst` (ui_fullscreen.rs:3956-3961) は stage-audio Phase I 承認 #4 で
`Fullscreen | DetachedWindow` に拡張された。実機でチェーン/GUI が detached で機能しない
(クリックでエラー) ため、**Phase I の拡張前提は不成立 → 撤回する** (将来 detached 対応する
なら独立ステージで再導入)。

**確定仕様 (ユーザー決定 2026-07-08)**: VST ボタン = **フル機能 (OFF) モード + メイン
ウィンドウ表示 (非 detached) + フルスクリーン**のみ。ON モードと F12 detached では出さない。
VST 機能自体 (メイン fullscreen での チェーン / V キー GUI) は不変。

### fix12 要件

1. 音楽 chrome の `show_vst` 条件から `DetachedWindow` を外し、native 側と同じ
   `presentation == Fullscreen` 基準に揃える (単一ソース化できるなら共通 helper 可)。
2. parked chrome (`parked_live_music_chrome_view_state`) の `show_vst: true` 固定を
   false に変更 (parked は常に detached のため表示しない。パリティ維持 = active 側も
   detached では非表示なので整合)。
3. **V キー (keymap 経由の VST GUI toggle) の detached 中ガード**: 現状の「不適切な
   エラー」の正体を確認して実装メモに記載し、detached 中は適切な no-op または簡潔な
   トースト (「VST はメインウィンドウのフルスクリーンでのみ使用できます」相当) に変える。
4. ship-checklist v2 の V8 を新仕様に更新: VST 確認は「OFF モード + メインウィンドウ +
   フルスクリーン」で実施、detached ではボタンが出ないことを確認項目に追加。
   stage-audio §5 項目 5 (音声 detached で VST) は本仕様変更で読み替え。
5. テスト: `show_vst` 導出 (Fullscreen=true / DetachedWindow=false / parked=false)。
6. コミット `(detached-rework findings-19 fix12)`。

### fix12 実装メモ (Codex 2026-07-08)

- VST GUI / panel の露出条件を `vst3_playback_ui_context_is_main_fullscreen()` /
  `vst3_playback_controls_available()` / `native_video_vst3_controls_available()` に集約。
  条件は **VST 有効 + 複数ウィンドウ mode OFF + `ViewerPresentation::Fullscreen`**
  (native はさらに placement switch なし)。音声チェーン処理は変更せず、GUI/owner/HUD
  だけを main fullscreen に限定する。
- 音楽 chrome の `show_vst` は `music_chrome_should_show_vst(fs_idx)` から導出し、
  `DetachedWindow` / always-new ON / ParkedLive では false。ParkedLive の `show_vst: true`
  固定も削除。
- native 動画 HUD の availability / VST panel も同じ helper へ寄せ、button だけ消えて
  panel event が残る半端な状態を避けた。
- 旧 V キーの keymap VST GUI toggle は既に撤去済みだったため、新規 key route の変更はなし。
  誤って音楽 chrome 入口が呼ばれた場合は
  `VST はメインウィンドウのフルスクリーンでのみ使用できます` を出す。
- ship-checklist v2 の V8 は既に fix12 仕様 (main fullscreen で確認、detached はボタン absent)
  へ更新済み。stage-audio §3.5 / §5 の古い detached VST 前提も撤回済み。

### fix12 訂正 (Fable 2026-07-08、ユーザー指摘): V キーショートカットは存在しない

fix12 要件 3 の「V キー (keymap 経由の VST GUI toggle)」は**誤り** (stage-audio 時代の古い
記述由来)。keymap に VST 系 KeyAction は無く、native の 0x56 参照はクリップボード Ctrl+V
のみ。要件 3 は次のように読み替える: **キーではなく、ボタン以外に残る VST GUI 到達経路
(native `ToggleVst3Gui` イベント等) を列挙し、detached 中に到達し得るものがあれば適切な
no-op / トーストにする。現状ユーザーが見た「不適切なエラー」の正体の確認・記載は必須のまま**。
なお「フル機能側で設定したチェーンの効果が ON モードの再生にも乗る」ことはユーザー実機で
確認済み = 仕様どおり (チェーン共用 1 本、UI のみメイン fullscreen 限定)。
ship-checklist V8 も同時訂正済み。
