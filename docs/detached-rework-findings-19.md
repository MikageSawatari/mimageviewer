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

### fix12 検収合格 + 実機 OK (Fable 2026-07-08)

**fix12 (60fc2f2e) = 合格**。露出条件を helper 連鎖
(`vst3_playback_ui_context_is_main_fullscreen` = 設定 OFF モード + presentation==Fullscreen →
`vst3_playback_controls_available` → native/music 変種) に**単一ソース化** (既存事実のみ、
新規 bool なし)。music chrome から DetachedWindow 撤去・parked chrome `show_vst:false`・
native availability / panel / owner-enter の全経路を同 helper に統一・stale `ToggleVst3Gui`
イベントは案内トースト「VST はメインウィンドウのフルスクリーンでのみ使用できます」へ。
stage-audio §3.5-4 の supersede 記載と §5-5 の書き換え、導出テスト両方向。実機 OK
(ユーザー確認)。チェーン効果の共有 (音は全再生に効く / UI のみ制限) も docs に明文化された。

## fix13 (実機 FB 2026-07-08、checklist P 中): 見開き + 自動トリムの passive snapshot が live の揃え描画を再現しない

**症状**: 画像窓が見開き + 自動トリム (view trim) 表示のとき、別窓 (動画) をアクティブ化して
passive になると、live では「左右ページの上下トリム位置を揃え、余白は白 (紙色)」だったものが、
**ページがずれて黒背景が露出**する (スクリーンショットあり: 右ページ上部に黒帯)。
P1-P11 自体は OK。

**見立て**: fix10b は単一画像の rect/UV を live 入力で焼き込んだが、見開き凍結ページ
(`DetachedImageWindowFrozenPage` = texture / rect_norm / rotation / content_bbox) の
passive 描画が live のトリム揃えを再現していない。候補:
(a) rect_norm がトリム揃えオフセット適用**前**の値で焼かれている、
(b) passive 描画が content_bbox の **UV crop を適用せず**フルテクスチャを描いている、
(c) live はトリム露出部を**白 (紙色) で塗る**が、passive は CentralPanel の黒 fill のまま。

### fix13 要件 — 短い Phase 1 → 実装

1. **Phase 1**: live の見開きトリム揃え描画 (rect / UV crop / 背景 fill) と、park 時の
   `detached_spread_frozen_pages_for_snapshot` + passive 描画を突き合わせ、(a)(b)(c) の
   どれが欠けているかを特定して報告 (複合可)。
2. **実装**: fix10b と同じ方針 = **park 時に live 描画と同一の per-page draw rect + UV +
   背景色を焼き込み、passive は同一入力で描く** (fit/トリム数式は既存の共有純関数群に
   揃える。snapshot 用の独自解釈を作らない)。live で白く見えていた領域は passive でも白。
3. 連続表示 (continuous) の凍結ページも同じ入力源であることを確認。
4. テスト: トリム量が左右で異なる見開きの rect + UV 一致 (純関数)、背景 fill の分岐。
5. コミット `(detached-rework findings-19 fix13)`。

### fix13 Phase 1 結果 + 実装メモ (Codex 2026-07-08)

Phase 1 の突き合わせ結果:

- (a) rect_norm: **欠落なし**。`detached_spread_frozen_pages_for_snapshot` は live の
  `draw_fs_spread` と同じ `layout_spread_page_rects` / `content_center_offset` 系で
  トリム後の配置を焼いている。見開きの内側 trim 分は rect 側に隠れたまま残る。
- (b) UV crop: **欠落なし**。`DetachedImageWindowFrozenPage.content_bbox` は
  `draw_detached_image_window_snapshot` → `draw_fs_spread_page` へ渡され、
  `normalized_sub_rect(img_rect, bbox)` と `uv=bbox` で描かれている。
- (c) 背景 fill: **欠落あり (根因)**。live は `transparent_bg_style(self.fs_transparent_bg_mode, ...)`
  を使うが、passive の frozen pages は常に `FsBgStyle::Default` で描いていたため、
  白/市松モードで live では白く見えていた trim 露出領域が passive では黒地に戻っていた。

実装:

- `DetachedImageWindowFrozenPage` に `DetachedImageWindowFrozenBackground` を追加し、
  park 時に現在の透過背景モード (`Default` / `Solid(WHITE)` / `Checker(texture)`) を
  per-page DTO へ焼き込む。
- passive 描画は snapshot の `background` を `FsBgStyle` へ戻して `draw_fs_spread_page` に渡す。
  現在のメイン context の背景設定を再解釈しない。
- continuous と spread は同じ `DetachedImageWindowFrozenPage` を使うため、どちらも同じ背景入力を保持する。

追加テスト:

- `paused_continuous_detached_window_preserves_transparent_background`
- `detached_spread_snapshot_preserves_trim_uv_and_background`

### fix13 検収合格 (Fable 2026-07-08)

**fix13 (d68f2d49) = 合格**。Phase 1 で (a) rect / (b) UV は欠落なし (live と共有の
layout_spread_page_rects / content_bbox UV) を確認し、根因 = (c) **passive の frozen pages が
常に FsBgStyle::Default で描かれ、live の transparent_bg_style (白/市松) が失われていた**と
特定。修正 = `DetachedImageWindowFrozenBackground` (Default/Solid/Checker) を park 時に
per-page DTO へ焼き込み、passive は snapshot の背景を FsBgStyle に戻して描画 (現在の
メイン状態を再解釈しない = fix10b/fix6d-2 と同じ「park 時焼き込み」原則)。continuous /
spread は同一 DTO で共有。テストあり・実装メモ完備・コミットタグあり。
実機確認 (ユーザー): 見開き + 自動トリム + 白背景で park → 白のまま揃って見えること。

## fix13-2 (実機 NG 2026-07-08): 真因は背景でなく「passive がページを crop + 再フィットで描き直す」非対称

fix13 (背景焼き込み) 後も実機 NG (同スクリーンショット)。コード照合で機構を確定
(計装不要):

- **live** = `layout_spread_page_rects` ([ui_fullscreen.rs:1331](../src/ui_fullscreen.rs)) が
  **フルページ矩形** (余白込み `scaled_w × scaled_h`、start_y 共有) を返し、bbox は
  「コンテンツ端の突き合わせ配置」と hit rect にのみ使用。**白く見えるのはページ自身の
  白余白ピクセル** (背景 fill ではない)。
- **passive** = `draw_fs_spread_page` ([ui_fullscreen.rs:16431-16443](../src/ui_fullscreen.rs))
  が保存 rect 内で `fit_display_size_in_rect` により**再フィット**し、さらに
  `normalized_sub_rect(img_rect, bbox)` + `uv=bbox` で**コンテンツのみ crop 描画**。
  余白が描かれず黒露出 + 再フィットによる配置差。
- fix13 の背景焼き込みは実在する差分だが副次的 (背景モード Default では不変)。
  Phase 1 (b) の「UV crop は欠落なし」は crop の存在確認であって、live が crop
  **しない**非対称の見落とし。

### fix13-2 要件

1. **再導出の廃止 = 最終描画値の焼き込み** (fix10b の単一画像と同じ原則を見開きにも
   徹底する): park 時に live が実際に描いた **per-page の最終 paint rect (正規化) と
   uv rect** を `DetachedImageWindowFrozenPage` に焼き込み、passive は
   `painter.image(tex, rect, uv)` を直接呼ぶ。`draw_fs_spread_page` の
   再フィット (`fit_display_size_in_rect`) と bbox crop を frozen 経路から**通さない**。
   - 見開きトリム揃えでは live = フルページ rect + uv 全面。焼き込みが live の
     layout 出力 (`layout_spread_page_rects` の rect) をそのまま使えば余白ピクセルも
     再現される。
2. 背景 fill (fix13 の `DetachedImageWindowFrozenBackground`) は温存 (フルページ描画では
   ほぼ見えないが、live で背景が見えるケースの正しさのため)。
3. continuous (連続表示) の凍結ページも同じ「最終 rect + uv」焼き込みに揃える。
4. テスト: **左右で上下トリム量が異なる見開き**で、park 焼き込み rect/uv == live layout
   出力の一致 (純関数)。既存 fix13 テストは維持。
5. 実機再確認 (ユーザー): 同じ再現手順 (見開き + 自動トリム + 動画側アクティブ化) で
   白余白のまま揃って見えること。
6. コミット `(detached-rework findings-19 fix13-2)`。

### fix13-2 実装メモ (Codex 2026-07-08)

- `DetachedImageWindowFrozenPage` に `paint_rect_norm` と `uv_rect` を追加し、park 時に
  frozen page の最終描画入力を焼き込むようにした。見開き / continuous とも、
  live layout が決めた page rect を `paint_rect_norm` として保存し、UV は全面
  (`0..1`) を保存する。
- passive 描画は `draw_fs_spread_page` を通さず、snapshot の
  `paint_rect_norm` / `uv_rect` / `background` から `painter.image(...)` を直接呼ぶ。
  これにより `fit_display_size_in_rect` の再フィットと `content_bbox` の再 crop が
  frozen 経路から消え、ページ自身の白余白ピクセルを live と同じ矩形で描ける。
- fix13 の `DetachedImageWindowFrozenBackground` は温存。透過画像や背景モードが
  関係するケースでも、現在のメイン context を再解釈せず park 時の背景を使う。
- `location_display` は frozen page direct paint では読込中 fallback を描かないため
  DTO から削除した。
- テストは既存 fix13 の 2 本を強化し、見開き/continuous の `paint_rect_norm` が
  park 時の live layout を保持することと、`uv_rect == full_uv` を固定した。

## fix13-3 (実機 NG 2026-07-09): 黒露出は解消、代わりに白被り (ページが live と別スケール/位置)

fix13-2 (06eb05d8 = paint_rect_norm + uv_rect 焼き込み、指示どおりの実装) で黒トリム露出は
解消。しかし parked 化すると**画像の一部が白く塗られたように見える** — スクリーンショット
比較では、parked 側はページが live と異なるスケール/位置で描かれ、live ではウィンドウ外に
クリップされていた白余白 (ページ自身のピクセル) が窓内に現れている形。

**方針転換: 推測をやめ、数値突き合わせで 1 回確定させる (fix13-3 Phase 1)。**

### Phase 1 — 計装 (MIV_DETACHED_WINDOW_DEBUG ゲート、イベント時のみ出力)

1. **park 時** に 1 行/ページ: live 最終 paint_rect (絶対値) と uv_rect、正規化に使った
   基準 rect (full_rect or media_rect、その由来 = placement か実 client か、値)、
   焼き込んだ paint_rect_norm。
2. **passive 初回描画時** に 1 行/ページ: 復元後 paint_rect (絶対値)、使った full_rect
   (実 client)、ppp。
3. ユーザー実機 1 回 (同再現) → ログの数値差からズレの原因を確定して報告 → 修正。

### 原因候補 (計装で判別する。先に直しに行かない)

- (i) **正規化基準の不一致**: park 側の基準 (placement 由来 w/h、または media_rect =
  seek panel 除外後) と passive 側の基準 (CentralPanel の実 full_rect) が別物 →
  スケール/オフセット差。live 画面にはページ表示 (107,108/120) があり seek panel 分の
  差が疑わしい。
- (ii) rect と uv の組み合わせ取り違え (uv がフルページ化したのに rect 側の意味が旧の
  content 配置のまま、等)。
- (iii) 背景 fill (fix13) の塗り順/塗り範囲が paint_rect 全面に出て content に重なる。

### 実装条件

- 数値差の原因確定後、park と passive の**基準 rect を同一定義に統一**して修正
  (fix10b 単一画像と同じ「同じ座標系で焼いて同じ座標系で戻す」)。
- 回帰テスト: 基準 rect (placement/media/client) の変換を含めた rect/uv round-trip 一致。
- 計装は debug ゲート内で残置可。コミット `(detached-rework findings-19 fix13-3)`。

### fix13-3 Phase 1 計装メモ (Codex 2026-07-09)

- `MIV_DETACHED_WINDOW_DEBUG=1` 配下で、park 時に `frozen_page_bake` を 1 ページ 1 行出す。
  `phase=continuous|spread`、`window_id`、`idx`、`page_ord`、live 最終 `paint`、`uv`、
  正規化基準 `basis` (`basis_kind=full_rect`, `basis_source=runtime_placement`)、
  `media`、焼き込み後 `norm`、`content_bbox`、`ppp`、`placement` を記録する。
- deferred passive の初回 callback で、`frozen_page_restore source=deferred_first_draw` を
  1 ページ 1 行出す。実 client 由来の `basis` (`basis_source=passive_client`)、復元後
  `paint`、焼き込み `norm` / `uv`、`ppp`、`placement` を記録する。
- 次の実機ログでは同一 `window_id` + `page_ord` の `frozen_page_bake` と
  `frozen_page_restore` を突き合わせ、(i) 正規化基準差、(ii) rect/uv 組み合わせ差、
  (iii) 背景塗り範囲差のどれかを数値で確定する。

## fix13-4 (計装ログで確定 2026-07-09): 焼き込み不足は「clip」— フルページ矩形は重なる設計で、live は可視スパン clip で切っている

**計装結果 (scratchpad/fix133-cur.log、bake/restore 3 往復 ×2 ページ)**:

- **round-trip は完全一致**: basis (1243.33 vs 1243.34) / paint / norm / uv / ppp すべて
  0.01px 級で bake == restore。fix13-2/13-3 の焼き込み・復元機構自体は正しい。
- **決定的証拠**: page_ord=0 paint = x 73.66〜647.79、page_ord=1 paint = x 596.76〜1171.72
  → **2 ページの矩形が x 596.76〜647.79 (~51px) で重なっている**。フルページ矩形は
  `layout_spread_page_rects` がコンテンツ端を突き合わせる設計のため、余白ぶん必ず重なる。
- live はこの重なり・外側余白を**ページごとの clip (可視スパン = content bbox の x 範囲)**
  で切って描くため見えない。passive は uv 全面 + clip なしで painter.image するため:
  page 1 の左余白 (白) が page 0 のコンテンツへ**上書き** (中央の白被り) + 各ページの
  外側余白も露出 (左右の白柱)。スクリーンショットの症状と完全一致。

### fix13-4 要件

1. **live が実際に適用している per-page clip を特定し、同じものを焼き込む**:
   live の見開き描画でページ paint に効いている clip rect (可視スパン。おそらく
   x = content bbox 範囲、y = 全高。media_rect/エッジの扱い含め live の実装から取る) を
   確認し、`DetachedImageWindowFrozenPage` に **clip rect (正規化)** を追加して park 時に
   焼き込む。passive は `painter.with_clip_rect(復元 clip)` で同じ paint rect + uv を描く。
   - 代替実装 (等価): clip の代わりに「clip 済み sub-rect + 対応する uv crop
     (x を bbox 範囲に、y は全域)」を焼き込む。DTO を増やさないならこちらでも可。
     どちらを採るかは live の clip 実装 (clip が矩形 1 個で表せるか) を見て判断し、
     採った方を実装メモに記載。
2. 計装 (`frozen_page_bake` / `frozen_page_restore`) に **clip も追記**して残置
   (今回の教訓: paint/uv だけでは live との等価性を証明できなかった)。
3. テスト: 「重なるフルページ矩形 + clip」で、隣接ページのコンテンツ領域に他ページの
   余白が描画されないこと (clip 適用の純関数 or 描画矩形の交差判定)。
4. 実機再確認: 同再現で (i) 中央の白被りなし (ii) 左右の白柱なし (iii) live で見えていた
   上端の白余白 (ページ自身のピクセル) は見える、の 3 点。
5. コミット `(detached-rework findings-19 fix13-4)`。
