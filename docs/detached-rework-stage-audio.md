# Stage AUDIO: 音声ファイルの detached 対応 (no-op 制限の解除)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md) — §2 (憲法) を先に読むこと。
前提: stage-settings が完了していること (同一ファイルを触るため並行しない)。

## 0. 背景とスコープ (ユーザー決定 2026-07-07)

- 音声ファイルは現在 detached 対応が**意図的 no-op** (toast 2 種、コミット ce6b757f、
  app.rs:39883 付近)。当時のブロッカー = font atlas 競合 (Y-32 系) は R1c の resync
  gate 統一で根治済みの見込み。
- 複数ウィンドウモードで「動画は別窓、音声はメイン窓」の非対称が発生しており、
  音声機能 (音楽統合) は**次リリースで初公開**のため、この非対称を出荷前に解消する
  (案 A)。**F12 での音声 detached も同時に解除**する。
- 音楽統合機能は未リリース → 音声側の挙動・設定はマイグレーション不要で自由に変更可。

## 1. 仕様 (ユーザー確定分)

1. **メディア窓は動画と共用で常に 1 本** (音声も動画もこの窓を使う)。
   VST チェーンが 1 本しかないため共用は必然、という判断。
   - 音声再生中に動画を開く / 動画再生中に音声を開く → 既存の「新メディア open で
     旧メディア窓を差し替え/close」規則をメディア種をまたいで適用する。
2. **音声も動画と同じ規則で live-park**: 別窓アクティブ化で閉じずに再生継続、
   非アクティブ中の操作はクリック復帰のみ、復帰後に HUD/操作が有効化。
3. **入口は動画と対称**:
   - 複数ウィンドウモード: グリッドから音声を開く → メディア窓 (detached) で開く
   - F12: フルスクリーン音声再生中の F12 で detached ⇔ main を切り替え (両モード)
   - no-op toast 2 種は撤去
4. **VST**: チェーンは共用 1 本 (現行どおり dsp_bridge 共有)。VST GUI (V キー /
   ボタン) が detached 音声窓で機能するか検証し、問題があれば挙動を報告
   (勝手なヒューリスティックで塞がない。必要なら Fable 判断)。
5. stage-settings で「動画は１つの動画ウィンドウで再生します」とした設定文言を
   「**動画/音声は１つのメディアウィンドウで再生します**」に更新する
   (ユーザー原案の文言に復帰)。

## 2. Phase S: スパイク (先に実施、挙動コミットなし)

本実装の前に以下を調査して**報告のみ**行う (Fable が仕様承認してから Phase I へ):

1. 音声 open の全入口を列挙: グリッド open (ON モード) / F12 (フルスクリーン音声) /
   その他 (SendTo / リング / キー操作等で音声が fullscreen 化する経路)。
   各入口で no-op gate がどこにあり、外すと何が起きるか。
2. no-op gate をローカルで外して起動し、**音楽ビューが detached viewport でどこまで
   描けるか**を確認: spectrum (108band) / DJ 波形 / 右パネル / 鍵盤 / シークバー。
   `fs_music_view_active` 系 (~98 述語) のうち「メイン viewport 前提」で壊れるものを
   件数と代表例で報告。
3. font atlas 症状 (文字消失 / Y-32 panic) の再発有無。
4. live-park の成立性: park 中も音が続くか / ParkedLive (immediate 維持) で
   spectrum 描画が止まらないか (repaint 駆動の有無)。
5. VST GUI: detached 音声窓で V キー / VST ボタンが機能するか、native shell owner が
   どの HWND になるか。
6. **判断基準**: 述語修正が局所 (代表的な分岐の付け替えで済む) なら Phase I 続行。
   広範囲な作り直しが必要なら**手を止めて報告** (リリース判断に影響するため)。

## 3. Phase I: 本実装 (Phase S 報告 → Fable 承認後)

- §1 の仕様どおり。入口の分岐は既存の動画 detached 経路 (prepare_viewer_presentation /
  keymap の F12 経路) への合流を基本にし、音声専用の新フラグ・時間窓を足さない (憲法 3/5)。
- ParkedLive の状態機械 / メディア 1 本規則 / live-park 復帰は**動画の実装をそのまま
  共用**する (音声だけの別実装を作らない)。
- テスト:
  - ON モード: 音声 open → メディア窓で開く / 動画→音声・音声→動画の差し替え規則
  - F12: 音声 fullscreen ⇔ detached の往復 (keymap 経由)
  - live-park: park で閉じない・クリック復帰 (既存の動画テストの音声版)
  - no-op toast の撤去確認 (該当コード grep 0 件)
- 既存の detached テスト・音楽 (Inc7) テストを弱体化しない。仕様変更で赤くなるものは
  列挙して報告。
- ドキュメント: docs/music-integration-plan.md (detached 制限の記述を更新) /
  docs/detached-viewer-implementation-plan.md (メディア窓の対象に音声を追加) /
  マニュアル (音楽関連ページがあれば detached の記述を追加、settings.html の文言更新) /
  [ship-checklist](detached-rework-ship-checklist.md) に音声ケースを追加
  (R6 の音声版: 音声 live-park 通し)。

## 3.5 Phase I 仕様承認 (Fable 2026-07-07、Phase S 報告に対する構造判断)

Phase S の最低条件 1/2/4 (supports_session / forced_presentation の media 一般化 /
DetachedSource 追加) は**そのまま承認**。争点だった 3 (music_* の bundle 化) と 5 (VST) は
以下のとおり決定する:

1. **music_* 状態は ViewerContextBundle に入れない (bundle 化しない別設計を採用)**。
   根拠: メディア窓は常に 1 本 (§1-1) なので、**音楽ビューの消費者はどの瞬間も 1 つ**
   しか存在できない — detached 音声窓が生きている間に main 側で音楽ビューが立つ経路は
   設計上存在しない (音声 open は常にメディア窓に行くため)。混線はこの規則が構造的に
   排除する。global のままでよい。
   - この不変条件をテストで固定する: 「音声 open は常にメディア窓へ」
     「メディア窓存在中に main viewport で `fs_music_view_active` が真にならない」。
2. **`DetachedSource::Audio` を追加** (`Media` 統合ではなく別値。ログ/状態の誤分類
   防止が目的なので、既存 `Video` と対で分かる方が診断に有利)。
3. **ParkedLive 音声の表示は 2 段構え**:
   - 第 1 候補: park 中もライブ描画を維持 (spectrum/波形は global 状態から描けるはず。
     bundle 依存の値 [トラック情報等] は park 時にスナップショットするか engine から取る)。
   - fs_cache / fullscreen_idx 依存がどうしても外せない場合: park 時点の見た目
     スナップショット + 音声再生継続にフォールバック (どちらを採ったか完了報告に明記)。
   - どちらでも**音の継続は必須**・**クリック復帰で即ライブ描画に戻る**は必須。
   - 時間窓・新規 bool 禁止は憲法どおり (状態は DetachedWindowState / runtime に載せる)。
4. **VST**: ボタン表示条件 (`viewer_presentation == Fullscreen` 前提) を detached にも
   拡張する。**「音にチェーンが効く」ことが Phase I の必須要件**。VST GUI (V キー) が
   native shell owner の都合で detached 窓と両立しない場合は、GUI 側のみ一旦非対応
   (実挙動を報告、仕様判断は Fable に戻す) でよい — 勝手なヒューリスティックで
   塞がないこと。
5. **startup direct open (SendTo 等) の Audio は今回スコープ外** (現状 Image/Video のみ
   のまま維持。広げない)。
6. 過去の wgpu Validation revert 歴があるため、実装後の実機 smoke では起動直後に
   「文字消失 / Y-32 / texture delta 喪失」系ログの確認を最初に行う (Codex 提案どおり)。

## 3.6 検収指摘 fix1 (Fable 2026-07-07): ParkedLive 音声窓が真っ黒になる

Phase I (b2063ef3) の検収で 1 点差し戻し。他は合格 (DetachedSource::Audio /
supports_session / F12 / メディア 1 本 / music_* 非 bundle 化 invariant / VST 条件 /
文言 / one-shot presentation テストすべて指示書どおり)。

### 指摘

`build_parked_live_media_window_snapshot` ([app.rs:27032](../src/app.rs)) は backdrop に
**1×1 の黒テクスチャ**を使う。動画は native presenter child が窓全面を覆うので問題に
ならないが、**音声には presenter がないため、ParkedLive 音声窓は「真っ黒 + バーのみ」の
窓になり、音だけ流れ続ける**。これは §3.5-3 の要求 (第 1 候補 = ライブ描画継続 /
フォールバック = park 時点の見た目スナップショット) の**どちらも満たしていない**。
また §3.5-3 の「どちらを採ったか完了報告に明記」も履行されていない。

### 修正要件 (fix1)

1. ParkedLive **音声**窓の表示を次のいずれかにする (動画の経路は変更しない):
   - (a) 推奨: **global 状態からの最小ライブ描画** — トラックタイトル (park 時に
     snapshot へ保持済み) + スペクトラム (global の spectrum 状態から bar 描画のみ)。
     フル音楽ビュー (タイムライン/右パネル/シークバー) は不要。bundle 依存の値を
     参照しないこと。
   - (b) 代替: park 時点の**見た目スナップショット** (音楽ビューの最終フレームを
     テクスチャ化)。実装コストが (a) より高ければ選ばなくてよい。
   - どちらを採ったか完了報告に明記する (今回は必須)。
2. 復帰規則の確認 (テスト or 報告): ParkedLive 音声窓は presenter がなく egui 経路で
   入力が来る。**クリックで復帰 / ホイール・キーでは復帰しない** (passive のクリック
   限定ルールと一貫) ことを確認し、ずれていれば揃える。
3. コミット `(detached-rework stage-audio fix1)`。

### fix1 実装結果 (Codex 2026-07-07)

- 採用方式: **(a) global 状態からの最小ライブ描画**。ParkedLive 音声窓は paused
  bundle から音声タイトルを識別し、黒 backdrop ではなく `MUSIC_VIEW_BG` 背景 +
  音楽アイコン + タイトル + クリック復帰ヒント + global `music_spectrum` の簡易
  スペクトラムを描く。
- spectrum の更新は ParkedLive bundle を mount して `poll_video` している既存区間で
  行う。`music_*` は bundle 化しないため、global の `music_pcm` / `music_spectrum`
  をそのまま更新し、描画側は最後に更新された spectrum を読む。動画 ParkedLive は
  従来どおり native presenter child が描くので変更しない。
- 復帰規則: presenter を持たない音声 ParkedLive は egui passive 経路を使い、
  press→release のクリックだけで復帰する。key / wheel は activation 入力に渡さない
  設計をテストで固定。

## 3.7 検収指摘 fix2 (実機 2026-07-07): F11 相互作用の 2 バグ

実機 (b2063ef3 ビルド) で新規 2 件。いずれも F11 との相互作用。

### 症状 A: 複数ウィンドウモードの音声窓が F11 往復でメインウィンドウ内に戻る

- ON モードで音声をダブルクリック → メディア窓 (detached) で再生開始 = 正常。
- その窓で **F11 → F11 (フルスクリーン往復) すると、音声がメインウィンドウ内の
  表示に変わってしまう**。同じ操作を動画で行うと detached のまま = 動画は正しい。
- Fable の初期調査: detached 中の F11 は本来
  `toggle_detached_viewer_borderless_fullscreen` (detached 窓自体のボーダレス化、
  presentation 不変) に行くべき。音声 (音楽ビュー) のキー配線がこの dispatch に
  乗らず、main fullscreen 系の経路 (presentation 再解決) に流れている疑い。
  音楽ビューのキー処理は歴史的に「main viewport 前提」だった (Phase S 指摘の
  fs_music_view_active 系) ので、F11 の分岐がその残りである可能性が高い。
- 修正方針: **音声 detached 中の F11 を動画と同じ dispatch に合流させる**
  (presentation を再解決しない)。音声専用分岐・新フラグを足さない (憲法 3)。

### 症状 B: 動画の音声モード (♪) × F11 で音声モードが壊れる

- 動画再生中に ♪ (音声モード、Inc7) → 音楽ビュー表示 = 正常。
- そこで **F11 を押すと動画表示に戻り、以後 ♪ を押しても音声モードに切り替え
  できなくなる** (トグル不能の固着)。
- 候補: F11 の遷移が `video_audio_mode` (hidden presenter 方式) の enter/exit
  状態機械を経由せずに presenter を un-hide し、内部状態 (video_audio_mode /
  hidden presenter / fs_music_view_active 前提) が desync している。
  Inc7 の enter gate が「既に音声モード扱い」等で弾いている可能性。
- 修正方針: F11 は音声モードを**維持したまま** borderless/fullscreen を切り替えるか、
  exit するなら正規の exit 経路 (Z/Esc と同じ teardown) を通す。どちらの仕様に
  するかは実装前に一言提案してよい (Fable 承認は不要、報告のみ。ただし
  「固着して再入不能」は必ず解消)。
- 注意: これは音楽統合 (Inc7) の状態機械に触れる。docs/music-integration-plan.md の
  該当節 (7-hidden / 7e-vst) を読み、hidden presenter の enter/exit 対称性を壊さない。

### fix2 要件

1. 症状 A: 音声 detached 中の F11 往復で presentation が DetachedWindow のまま。
   回帰テスト (音声 + F11 往復で presentation 不変。動画の既存挙動も不変)。
2. 症状 B: ♪ → F11 → ♪ の往復が何度でも成立 (固着なし)。回帰テスト
   (video_audio_mode の enter/exit が F11 を挟んでも対称)。
3. コミット `(detached-rework stage-audio fix2)` (fix1 と別コミットでよい)。

### fix2 実装メモ (Codex 2026-07-07)

- F11 / 音楽ビュー上バーの window ボタンを `toggle_egui_viewer_window_mode_for_input` に集約。
  detached session 中は presentation を再解決せず `toggle_detached_viewer_borderless_fullscreen`、
  非 detached では従来どおり `toggle_still_window_mode`。
- 動画→音声モード (`video_audio_mode`) 中の F11 は hidden presenter の placement switch へ流さず、
  egui 音楽ビューの window-mode 切替として処理する。`video_audio_mode` は保持し、映像復帰は
  `exit_video_audio_mode` だけが行う。
- 音楽 VST shell / 動画→音声 VST shell の window-toggle も同 helper に寄せ、detached 音声の
  VST 中 F11 でも MainWindow へ戻さない。

## 3.8 検収指摘 fix2b (実機 2026-07-07): 症状 B が残存 — F11 の presentation 遷移が音声モードを落とす

fix2 (27704d25) の実機確認: **症状 A は解消**。症状 B は残存 —
「♪ (音声モード) 中に F11 → フルスクリーンになると同時に動画表示へ戻る」。

### Fable の分析

- fix2 の dispatch 集約自体は正しく、audio-mode F11 は
  `toggle_egui_viewer_window_mode_for_input` に到達している
  (`[video-audio] window toggle while in audio mode` ログで確認可能)。
- 問題は非 detached 分岐の先: `toggle_still_window_mode()` が `viewer_presentation` を
  MainWindow⇄Fullscreen に切り替える際、**動画 item の presentation 遷移機構
  (prepare/open 系・native presenter の placement switch・exit_video_audio_mode の
  いずれか) が `video_audio_mode` をリセットしている**。fix2 のテストは dispatch
  直後の状態しか検証しておらず、遷移完了後のリセットを検出できていない。

### fix2b 要件

1. まず計装/ログで「F11 後に `video_audio_mode` を落としている呼び出し元」を特定して
   報告する (exit 経路の関数名まで)。
2. 音声モード中の F11 (非 detached) は「音楽ビューの表示先が embedded main ⇄
   fullscreen viewport に変わるだけ」とし、**`video_audio_mode` と hidden presenter の
   状態を維持**する (presenter を un-hide しない)。正規 exit (Z/Esc/♪) の経路は
   変更しない。
3. 回帰テスト: dispatch 直後ではなく **presentation 遷移の完了後**に
   `video_audio_mode` が維持されていることを検証するシーケンステスト
   (F11 → Fullscreen 化後も Some(fs_idx) → 再 F11 → MainWindow 化後も Some(fs_idx) →
   ♪/Z で正規 exit 可能)。
4. コミット `(detached-rework stage-audio fix2b)`。

### fix2b 実装メモ (Codex 2026-07-07)

- 実際に `video_audio_mode = None` を直接実行していたのではなく、F11 後の detached
  borderless settle / host resync が `sync_detached_video_child_presenter_rect()` 経由で
  hidden presenter に `SwitchPlacement` を投げ、presenter 側が再表示されることで
  「音声モードが落ちたように見える」状態になっていた。
- `video_audio_mode_hides_native_presenter_for(fs_idx)` を追加し、動画→音声モード中
  (VST ホスト表示中を除く) は native presenter の表示責務を持たないことを明示。
  `try_resync_detached_video_host()` と `sync_detached_video_child_presenter_rect()` はこの状態の
  host resync を **resolved no-op** として扱い、pending retry も `SwitchPlacement` も発行しない。
  正規の映像復帰は従来どおり `exit_video_audio_mode()` / `poll_video_audio_exit_pending()` のみ。
- 回帰テストは、poll 経由の resync no-op、direct settle 経由の
  `sync_detached_video_child_presenter_rect()` no-op、detached F11 の borderless settle 後も
  `video_audio_mode` と `native_video_mode_switch=None` が維持されることを固定。

## 3.9 検収指摘 fix3 (実機 2026-07-07): 動画の音声モード中に park すると窓が真っ黒

### 症状

動画を再生 → ♪ (音声モード) → **PDF ファイルを開く** (ON モードの book open =
メディア窓が live-park される) → **音声モードのメディア窓が真っ黒**になる (音は継続)。

### Fable の分析 (fix1 と同クラスの取り漏らし)

- fix1 の ParkedLive 最小ライブ描画は `viewer_context_bundle_audio_title`
  ([app.rs:25794](../src/app.rs)) が **`GridItem::Audio` のみ**を音声と判定する。
- 「動画を音声モードにした状態」は item が **`GridItem::Video` のまま**なので動画経路
  (黒 backdrop 1×1 + native presenter が覆う前提) に落ちる。しかし音声モード中は
  fix2b の所有境界どおり **presenter が hidden** → 覆うものがない → 真っ黒。

### fix3 要件

1. ParkedLive 判定を「presenter が窓を覆うか」で行う: parked bundle の内容が
   **音声モード中の動画 (= hidden presenter)** の場合も、fix1 の最小ライブ描画
   (タイトル + spectrum) を使う。タイトルは動画ファイル名でよい。
   判定は fix2b の `video_audio_mode_hides_native_presenter_for` と同じ意味論を
   parked bundle 側に適用する (新規 bool を足さない)。
2. park をまたいで `video_audio_mode` が維持されること (park で勝手に exit しない /
   presenter を un-hide しない)。クリック復帰で音楽ビューに戻り、♪/Z の正規 exit が
   引き続き機能する。
3. 通常の動画 ParkedLive (音声モードでない) は従来どおり presenter 表示 = 変更しない。
4. 回帰テスト: 音声モード中の park → 最小ライブ描画に分類される (黒 backdrop 経路に
   落ちない) / 復帰後 video_audio_mode 維持、のシーケンステスト。
5. コミット `(detached-rework stage-audio fix3)`。

### fix3 実装メモ (Codex 2026-07-07)

- `video_audio_mode` / `video_audio_vst` / audio-mode exit pending を
  `ViewerContextBundle` の swap 対象に追加した。`music_*` 解析状態は §3.5 どおり bundle 化せず
  global のまま、動画→音声モードという **メディア窓の文脈 state** だけを parked bundle が所有する。
- `ParkedLiveMusicWindowInfo` DTO を追加し、`GridItem::Audio` だけでなく
  `video_audio_mode == Some(fullscreen_idx)` かつ VST ホスト表示中でない `GridItem::Video` も
  ParkedLive 音楽表示対象として扱う。通常動画 ParkedLive は従来どおり presenter 表示を前提にする。
- 回帰テストで、動画→音声モードを park すると main 側の `video_audio_mode` は None に戻り、
  parked bundle 側に Some(fs_idx) が保持され、再アクティブ化後の active context でも維持される
  ことを固定。

## 3.10 UX 改善 fix4 (ユーザー FB 2026-07-07): park 中の音声表示を通常音楽ビューのレイアウトに近づける

### 背景

fix1 の最小ライブ描画 (アイコン + タイトル + 簡易スペクトラム) は仕様どおりだが、
実機でユーザーから「park のたびに見た目・サイズ感が大きく変わって違和感がある」と
FB。音楽機能は次リリースの目玉なので、park 中の表示を通常の音楽ビューに近づける。

### fix4 要件

1. ParkedLive 音声窓 (fix3 の音声モード動画を含む) の表示を、**通常の音楽ビューと
   同じレイアウトの主要ビジュアル**に拡張する。描画ソースは引き続き global 状態のみ:
   - タイムライン/DJ 波形 (global TimelineTextureCache)
   - 鍵盤スペクトラム (global MusicSpectrumState)
   - 下部 PCM グラフ (global music_pcm 系)
   - トラックタイトル (fix1 と同じ取得方法)
   - 再生位置 (playhead): parked bundle 内の player から **read-only** で取得できるなら
     使う (mount を毎フレーム化しない)。取得できない場合は playhead なしで波形のみ。
2. **操作系は描かない or 不活性**: シークバー・右パネル・ボタン類は非表示とし、
   クリック復帰ヒントは維持 (park の「クリック復帰のみ」規則と一貫)。
3. 実装は「draw_fs_music_view の bundle 依存部分を引き剥がす大規模リファクタ」を
   **しない**こと。global 状態から描けるコンポーネント描画の再利用・切り出しに留め、
   引き剥がしが必要になったコンポーネントは省略して報告する (park 中は多少簡略でも
   よい。目的はレイアウト・サイズ感の一貫性)。
4. 回帰: fix1 のテスト (音声識別・クリックのみ復帰) を維持。
5. コミット `(detached-rework stage-audio fix4)`。fix3 と同時実装でよい。

### fix4 実装メモ (Codex 2026-07-07)

- ParkedLive 音楽表示を、黒背景 + アイコン中心の最小ビューから、通常音楽ビューと同じ
  `MUSIC_VIEW_BG` / 上情報バー / 中央波形 / 下段鍵盤スペクトラム構成へ変更した。
- 操作系は置かない。中央波形は `music_analysis` から L/R peak を静的描画し、
  `draw_music_timeline()` は seek/drag を持つため呼ばない。下段スペクトラムは global
  `music_spectrum` を描くだけで、クリック復帰規則を壊さない。
- 再生位置は parked bundle 内の player から read-only で取得できる場合に playhead と
  spectrum marker へ反映する。取得できない場合は解析中表示へフォールバックする。

## 3.11 検収指摘 fix5 (実機 2026-07-08 未明): park 中の上部グラフが通常表示と全く違う

- ユーザー報告: 音声モード表示中に別窓を開く (park) と、**上側のグラフが普段と
  全く違うものになる**。
- fix4 の指示 (§3.10-1) は「タイムライン/DJ 波形は **global TimelineTextureCache**
  を再利用」だったが、実装メモは「静的波形」= 独自描画になっており、通常ビューの
  タイムラインと見た目が一致していない可能性が高い。
- fix5 要件:
  1. park 中の上部グラフは、**通常の音楽ビューが表示しているのと同じタイムライン
     テクスチャ (global cache)** を描く。テクスチャが未生成のトラックのみ現行の
     静的波形にフォールバック。
  2. 描画スケール/配色も通常ビューと揃える (見た目の連続性が目的)。
  3. もし既にテクスチャを再利用しているのに違って見える場合は、何を描いているか
     (データソース・スケール) を調査して報告してから直す。
  4. コミット `(detached-rework stage-audio fix5)`。

### fix5 実装メモ (Codex 2026-07-08)

- ParkedLive 音楽窓の上部グラフを、独自静的波形から通常音楽ビューと同じ
  `draw_music_timeline()` + global `TimelineTextureCache` 経路へ変更した。
- `MusicTimelineOutcome.displayed_texture_rows` を追加し、timeline texture がまだ 1 行も
  表示できない初回 frame だけ従来の静的波形 fallback を重ねる。
- ParkedLive 中の操作は引き続き有効化しない。timeline の seek outcome は無視し、
  クリック復帰のみの規則を維持する。

## 3.12 検収指摘 fix6 (実機 2026-07-08): 非アクティブ窓の合図がなく、音声 parked のタイムラインがちらつく

実機 (ゲート C smoke) で 3 症状:

- **①** 動画を音声モードにして別窓で PDF を開き、PDF 窓でホイールすると、音声モード
  parked 窓の上側グラフが一瞬「縦に太い波形」に化ける。原因 =
  `draw_parked_live_music_window` ([ui_fullscreen.rs:3875-3887](../src/ui_fullscreen.rs)) の
  `draw_parked_live_music_timeline() || draw_parked_live_music_waveform()` で、timeline が
  `displayed_texture_rows == 0` の frame に静的波形フォールバックへ落ちる (fix5 の副作用)。
- **②** 音声モード窓に戻る (アクティブ化) と中身のレイアウトが変わって気になる。上部 HUD の
  内容が変わるのは可だが、ヘッダのサイズは固定にしたい。
- **④** 複数窓で映像窓をタスクバー等で前面化して ♪ を押しても効かない。実機機構 = parked
  映像窓は presenter イベントが `native_video_event_blocked_by_parked_live_filter`
  ([native_video.rs:2487](../src/app/native_video.rs)) で左クリック (= アクティブ化) 以外
  破棄され、既に「1 クリックでアクティブ化してから操作」モデルだが**視覚的な合図がない**
  だけ (= ユーザーは「効かない」と誤認する)。

**ユーザー決定 (2026-07-08)**: 非アクティブ窓は HUD を減光して「1 クリックで有効化」を
視覚化する。**全体グレーではなく HUD 部分 / ボタンの明度を下げる**。音声 parked 窓は
上側タイムラインを隠すが、**下側スペクトラムは動かし続ける** (再生中が分かる)。
アクティブ化ロジック (watcher / parked-live filter) は変えない = **視覚のみの修正**。

### fix6 要件

**"非アクティブ" の位置づけ (述語不要)**: passive/parked 窓の描画経路
`draw_parked_live_music_window` / `draw_detached_image_window_bar` は本質的に**非アクティブ
窓の描画** (アクティブ窓は active viewport = `draw_fs_music_view` / native presenter で描く)。
よって egui 側は「アクティブか」の述語や新規 bool を足さず (憲法 §3)、これらの描画関数は
常時 dim + タイムライン非表示でよい。

1. **音声モード parked 窓 (`draw_parked_live_music_window`, egui)**:
   - 上側タイムライン (`draw_parked_live_music_timeline` と静的波形フォールバック
     `draw_parked_live_music_waveform`) を **parked 中は描画しない** → **① のちらつきが
     構造的に消える** (フォールバック経路そのものが無くなる)。中央領域は暗色 +
     「クリックで操作に戻る」等のヒントのみ。
   - 下側スペクトラム (`music_spectrum.draw`) は**継続描画**。poll 側の解析更新
     (`update_parked_live_audio_music_view_state`) は不変で spectrum が動き続けること
     (アクティブ化した瞬間タイムラインも即出せるよう、解析は止めない)。
   - 上部情報バー (ヘッダ) は減光 (テキスト/背景の明度を下げる)。**ヘッダ高さ (54px) は
     固定** (parked/active でヘッダ配置が動かない)。クリック復帰ヒントは維持。
2. **画像/本 passive 窓 (`draw_detached_image_window_bar`, egui)**:
   - passive バー / ボタンの明度を下げる (一貫性のため)。バーの高さ・配置は不変。
3. **映像再生中 parked 窓 (native presenter HUD)** — **Phase 0 調査 → Fable 承認 → 実装**:
   - presenter の HUD は presenter 内の HUD ウィンドウで egui 描画される。以下を調査して
     報告する: (a) active/parked を presenter へどう伝えるか (既存 `set_native_*` パターンに
     dim flag を足せるか)、(b) parked 中に HUD が実際に描かれるか、(c) dim を HUD の egui
     描画側で色/alpha に反映する最小の差し込み点。
   - 承認後、HUD を減光する dim 経路を追加 (**全体でなく HUD 部分 / ボタン明度**)。ボタンは
     既に inert (parked-live filter) なので**挙動は変えない = 視覚のみ**。
   - **承認前に `src/video/native_presenter/` を触らない** (native は Phase 0 制、憲法 §7)。

### 禁止 / 注意

- 減光は「アクティブか否か」の状態ベース。**時間窓 / フェードで競合を吸収しない** (憲法 §5)。
  見た目のフェードは競合吸収目的でないなら任意だが、まずは即切替で実装する (混乱回避)。
- `draw_fs_music_view` の bundle 引き剥がしリファクタは禁止 (fix4 と同方針)。
- スコープは dimming + parked タイムライン非表示のみ。activation ロジックは変えない (§7)。

### テスト

- 音声 parked 窓の描画分岐: parked 中は timeline 描画を呼ばず spectrum のみ、が分かる形で
  検証する (「timeline を描くか」の導出を純関数に切り出してテスト、または描画呼び出しの
  有無を検証)。
- 既存の fix1/fix3/fix4 テスト (音声識別・クリックのみ復帰・video_audio_mode 維持) を維持。
- (native dim は Phase 0 承認後、可能なら dim flag 導出をユニットテスト)

### 触ってよいファイル

- `src/ui_fullscreen.rs` (`draw_parked_live_music_window`, `draw_detached_image_window_bar`)
- `src/app.rs` (描画分岐 helper が必要な場合)
- `src/app/tests.rs`
- (Phase 0 承認後) `src/video/native_presenter/`, `src/app/native_video.rs`
- `docs/detached-rework-stage-audio.md` (実装メモ)

### コミット

`(detached-rework stage-audio fix6)`。native presenter dim は承認後に別コミット `fix6b` へ
分けてよい (egui 分を先に出す)。

### fix6 実装結果 (Codex 2026-07-08) + 検収 (Fable 2026-07-08、机上合格 → 実機 NG は §3.14 fix6c へ)

コミット = `a5c5fa50` (src/ui_fullscreen.rs のみ)。

- `draw_parked_live_music_timeline` / `draw_parked_live_music_waveform` を**関数ごと削除**
  (残参照 0 件確認) → ① のフォールバック経路が構造的に消滅。
- レイアウト導出を純関数 `parked_live_music_window_layout` に切り出し
  (`draw_timeline` / `draw_waveform_fallback` は常に false を返す構造で不変条件を明文化、
  描画側 debug_assert + テスト 2 本で固定)。
- ヘッダは `PARKED_LIVE_MUSIC_TOP_H = 54.0` 定数で固定 (テストで assert) + 減光
  (タイトル gray 220→160、ラベル/ヒント減光、背景 alpha 90→132)。
- 中央領域 = 音符アイコン + 「クリックで操作に戻る」+ 再生位置 (`format_duration_mm_ss`)。
- 下側 spectrum は継続描画 (`show_spectrum` 時、band の 34% / max 180px)。poll 側
  (`update_parked_live_audio_music_view_state`) は不変 = 解析継続。
- passive バー (`draw_detached_image_window_bar`) = 背景 alpha 200→150・区切り線 60→34・
  × ボタン (150,42,42,205 / 48,48,48,160)・タイトル文字 235→168 に減光。高さ・配置不変。
- 検収確認: 憲法 §3 (App 新規 bool なし・layout 構造体はローカル値のみ) / §5 (時間窓・
  フェードなし、即切替) / §7 (スコープ内 1 ファイルのみ) / §8 (fix1/fix3/fix4 テスト無傷、
  src/app/tests.rs の parked_live 系 7 本不変)。tests/ui_snapshot.rs に detached/parked 系
  スナップショットは無く配色変更の snapshot 回帰なし。テスト 3224 pass (--bin)。

### fix6b Phase 0 承認 (Fable 2026-07-08): native presenter HUD 減光

Codex の Phase 0 報告 (parked 状態は App 側 `native_video_parked_live_input_window_id` から
導出し、既存 `set_native_*` / overlay 系で dim flag を presenter へ渡す。parked 中も
presenter/HUD は生きており、ボタンは filter で既に inert = 視覚のみで実現可能) を**承認**。
実装条件:

1. **App に新規フィールドを作らない** (憲法 §3)。dim は既存 parked 状態からの導出のみ。
   flag の保存先は presenter/overlay 側 (`set_native_loop_enabled` 等と同じパターン)。
2. **set/unset の対称性に注意**: `native_video_parked_live_input_window_id` は parked poll
   区間だけ Some になる一時フィールド。parked poll 区間で dim=true、アクティブ側の通常
   poll / 復帰遷移で dim=false を冪等に送る (毎 poll set で可)。**復帰後に dim が残る /
   アクティブ窓が dim になる取りこぼしがないこと**をテストまたはログで確認する。
3. 減光は HUD 部分/ボタンの明度のみ (映像フレームは減光しない)。hit-test・イベント挙動は
   不変 (視覚のみ)。
4. 時間窓/フェードで切替を吸収しない (憲法 §5)。即切替。
5. 別コミット `(detached-rework stage-audio fix6b)`。

### fix6b 実装メモ (Codex 2026-07-08)

- `NativeEguiOverlay` に HUD 専用の `hud_dimmed` 状態を追加し、既存の
  `set_native_*` と同じ command 経路 (`SetHudDimmed`) で App から presenter thread へ
  伝搬する。App 側には新規フィールドを追加せず、既存の
  `native_video_parked_live_input_window_id.is_some()` から毎 poll 導出する。
- parked poll 区間では `dimmed=true`、通常 active poll / 復帰後は `dimmed=false` を
  冪等送信する。SwitchPlacement で presenter が再構築された場合も presenter thread 側で
  現行値を保持し、新 presenter に再適用する。
- 減光は top bar / bottom HUD の描画後に HUD 領域だけ半透明黒を重ねる方式。video frame
  には重ねず、hit-test / command 発行 / parked-live filter は不変。
- 回帰テスト: `native_video_hud_dimmed_only_during_parked_live_poll` で dim が parked poll
  の一時状態だけから導出され、通常 poll へ戻ると false になることを固定。

## 3.13 検収指摘 fix7 (実機 2026-07-08): parked 中のメディアが EOF で次へ進まない

実機: 画像 (本) をアクティブにしたまま別窓で音声/動画を parked 再生していると、末尾に
達しても次のメディアへ進まない。ユーザー要望 = 「動画/音楽を聞きながら本を読む」用途に
対応したい。

**機構**: `poll_parked_live_detached_windows` ([app.rs:26220-26227](../src/app.rs)) が parked
poll の間 `video_continuous_mode = Off` を強制 → `poll_video` 内の連続再生 EOF ハンドラが
発火しない。これは R2b の「ParkedLive は EOF 自動進行なし」仕様。

**ユーザー決定 (2026-07-08)**: **連続再生設定が ON のときだけ** parked 窓でも次メディアへ
進める (アクティブ窓と同じ挙動)。設定 OFF なら従来どおり停止。

### fix7 要件 — **Phase 0 調査 → Fable 承認 → 実装** (挙動変更・リスクあり)

1. **Phase 0 調査 (先に報告)**: parked bundle を swap-in した状態
   (`swap_viewer_context_bundle`) で連続再生 EOF ハンドラ
   (`handle_video_audio_mode_continuous_eof` / `handle_music_continuous_eof` /
   `handle_video_continuous_eof`) を走らせたときに:
   - (a) 次メディアへの source-swap が **parked 窓内で完結**するか (音声モードの hidden
     presenter source-swap が parked 状態でも成立するか)。
   - (b) アクティブ (本) 文脈 (`items` / `current_folder` / `auto_aspect` / `fullscreen_idx`)
     を**汚さない**か。
   - (c) parked 状態 (`ParkedLive`) と音声モード (`video_audio_mode`) が**維持**されるか。
   - (d) `video_continuous_mode = Off` 強制を外す/条件化したときの影響範囲。
   を調査し、安全に実装できる形を報告する。危険なら代替 (parked 専用の軽量 EOF 進行 helper)
   を提案する。

2. **実装 (承認後)**: parked poll で `video_continuous_mode` をユーザー設定尊重にし
   (無条件 Off をやめる)、parked 窓の EOF で次の表示順メディア (動画/音声) を parked
   bundle の player へ source-swap する。窓は `ParkedLive` のまま、音声モードなら音声モード
   維持。**連続再生 ON のときのみ**進行。アクティブ窓の EOF (Inc7-eof) は不変。

### 禁止 / 注意

- 進行の要否は `video_continuous_mode` 設定 + EOF 状態から導出 (新規 bool 禁止、憲法 §3)。
- アクティブ (本) 文脈を奪わない (findings-6/12/17 と同じ不変条件)。
- スコープは parked EOF 進行のみ (§7)。

### テスト

- シーケンス: parked 音声窓 + 連続再生 ON → EOF → 次メディアへ source-swap
  (`ParkedLive` 維持・音声モード維持・アクティブ本文脈の `items`/`current_folder`/
  `auto_aspect` 不変)。
- 連続再生 OFF → EOF → 停止 (進まない)。
- アクティブ側文脈不変の回帰 (findings-12/17 と同型)。

### 触ってよいファイル

- `src/app.rs` (`poll_parked_live_detached_windows`、parked EOF 進行 helper)
- `src/app/native_video.rs` (parked source-swap が必要な場合)
- `src/app/tests.rs`
- `docs/detached-rework-stage-audio.md` (実装メモ)

### コミット

`(detached-rework stage-audio fix7)`。fix6 とは**別コミット必須** (挙動変更)。

## 3.14 検収指摘 fix6c (実機 2026-07-08): fix6 の実機 NG 3 件 — 多重ヘッダ / スペクトラム位置ずれ / HUD クリック不発

fix6 (a5c5fa50) の実機 smoke で 3 症状。①②は fix6 が顕在化させた構造問題、③は fix6 指示書の
前提誤り (Fable の診断漏れ)。いずれも機構確定済み:

- **①-1 音声 parked の上部文字が多重表示 (減光も知覚されない)**: ParkedLive 窓の描画
  ([ui_fullscreen.rs:4514-4530](../src/ui_fullscreen.rs)) は `draw_parked_live_music_window`
  (54px ヘッダ = メタデータタイトル + 再生中 + 右端ヒント) の**上に**
  `draw_detached_image_window_bar` (44px バー = ファイル名 + ×) を**無条件で重ね描き**する。
  ヘッダ 2 枚 + 別文字列 2 本。fix4 当時からの潜在問題だが、旧バー背景 alpha 200 が下の
  文字を隠していた。fix6 の減光 (alpha 150) で透けて顕在化した退行。
- **①-2 スペクトラム位置ずれ + 下 HUD 消失**: アクティブ音楽ビュー
  ([ui_fullscreen.rs:20583-20634](../src/ui_fullscreen.rs)) は spectrum を
  「窓下端 − `MUSIC_HUD_HEIGHT`(62) − 4」に置くが、parked layout は「窓下端 − 24」+
  高さ式も別 (band 34% vs 「timeline 120px を残して伸縮」)。park の瞬間に spectrum が
  約 42px 下へジャンプ。fix4 の目的 (park で見た目を変えない) に反する。
- **② 映像 parked 窓の HUD クリック (♪ 等) でアクティブ化しない**: activation capture は
  生の左クリック `Window(MouseButton(Left))` ([native_video.rs:2459](../src/app/native_video.rs))
  のみ。HUD ボタン上のクリックは HUD 側 egui が消費し**セマンティックイベント**
  (ToggleAudioMode 等) として届くため、filter ([native_video.rs:2478](../src/app/native_video.rs))
  が黙って破棄するだけで activation に変換されない。fix6 指示書の「1 クリックで
  アクティブ化モデルは既に動いている」は HUD 領域については誤りだった。
- (参考) 「映像窓の HUD 減光が効かない」は fix6b 未実装のため期待どおり。fix6c の対象外。

**ユーザー決定 (2026-07-08)**: ①-2 は**下 HUD を減光して表示** (帯確保だけでなく描画する。
fix4 の「操作系は非表示」をここで改訂)。② は **HUD ボタン上のクリックも 1 クリックで
アクティブ化に変換** (ボタンの機能は実行しない。キー/ホイールは従来どおり無視 =
クリックのみ復帰の規則は維持)。

### fix6c 要件

1. **多重ヘッダ解消 (単一ヘッダ化)**:
   - ParkedLive 音楽窓 (call site [ui_fullscreen.rs:4518-4529](../src/ui_fullscreen.rs)) では
     `draw_detached_image_window_bar` の**バー背景 + ファイル名テキストを描かず、× ボタン
     のみ**描画する (引数 or 別 helper 切り出し。App の新規フィールドではないので憲法 §3 OK)。
   - × ボタンの rect は引き続き `detached_image_window_bar_close_button_rect()` から導出
     (watcher の × hit 判定との単一ソース、findings-15 G2 のテストを壊さない)。
   - 音楽ヘッダ右端のヒントテキストは × ボタンと重なるため**削除** (中央の
     「クリックで操作に戻る」ヒントは fix6 で入っており維持)。
   - 画像/PDF passive 窓 (音楽表示なし) のバーは fix6 のまま不変。
2. **parked レイアウトをアクティブ音楽ビューと一致させる**:
   - `parked_live_music_window_layout` を active 側 (20583-20634) と同じ式に:
     `band_top = top + 54` (fix6 の +8 を撤去)、`band_bottom = bottom − MUSIC_HUD_HEIGHT − 4`、
     `spectrum_h = 180.min(band_h − 8 − 120)` (TIMELINE_MIN_H 予約を含め**完全同一** =
     同一窓サイズで spectrum rect が active と画素一致)。gutter 式は既に同一。
   - 可能なら式を小さな共有 helper / 共有 const に切り出して両者から使う (draw_fs_music_view
     の bundle 引き剥がし大規模リファクタは引き続き禁止。式の共有だけにする)。
3. **下 HUD を減光して描画** (①-2 のユーザー決定):
   - parked 音楽窓の下端 `MUSIC_HUD_HEIGHT` 帯に HUD を減光描画する。シークバー位置は
     info (position/duration) から。操作は不能のまま (クリックは復帰動作、変更しない)。
   - 既存の音楽 HUD 描画が info + 設定だけで描けるなら流用 + dim。bundle/player への依存が
     強く引き剥がしが必要なら、**見た目相当の簡易 HUD** (シークバー + 主要ボタンの減光
     描画) で可 (fix4 の簡易描画と同じ路線)。どちらを選んだか実装メモに記載する。
4. **HUD クリック → アクティブ化** (② のユーザー決定、`src/app/native_video.rs` = App 側のみ):
   - `NativeVideoOutputEvent` の variant を棚卸しし、(a) ライフサイクル/ステータス系
     (PlacementSwitched 系は既に allowed。FirstFramePresented / エラー通知等があれば
     **絶対に activation にしない**) と (b) **ユーザーのクリック起点で発生する HUD 操作
     イベント**に分類する。分類は純関数にしてテストで固定。
   - parked-live filter で (b) を破棄する際に activation 要求へ変換する (既存の
     `native_video_parked_live_activation_requests` push + dedup を流用。機能自体は実行しない)。
   - wheel / key / hover 由来のイベントは従来どおり破棄のみ (クリックのみ復帰の規則維持)。
   - presenter (`src/video/native_presenter/`) は触らない (fix6b の範囲)。

### 禁止 / 注意

- App に新規 bool / Option フィールドを足さない (憲法 §3)。描画分岐は関数引数 / ローカルで。
- 時間窓禁止 (憲法 §5)。
- スコープは上記 4 点のみ (憲法 §7)。fix6b (native HUD dim) は別コミットのまま独立。
- 既存テスト (fix1/fix3/fix4/fix6/findings-15) を弱体化しない (憲法 §8)。

### テスト

- レイアウト一致: 同一 rect 入力で parked layout の spectrum rect が active 式の値と一致
  することを固定 (共有 helper 化した場合はその helper のテスト)。
- 単一ヘッダ: ParkedLive 音楽窓ではバー背景/テキストを描かず × のみ、の分岐を引数/純関数
  レベルで固定。`detached_image_window_bar_close_button_rect` 由来の × rect が不変であること。
- HUD イベント分類: クリック由来 → activation 変換 / ステータス系 → 変換しない /
  wheel・key → 変換しない、を variant 網羅でテスト。
- 既存の parked_live 系テスト 7 本 + fix6 layout テスト 2 本を維持 (fix6 テストは
  レイアウト式変更に合わせた期待値更新のみ可)。

### 触ってよいファイル

- `src/ui_fullscreen.rs` (`draw_parked_live_music_window` / `parked_live_music_window_layout` /
  `draw_detached_image_window_bar` とその call site / 共有レイアウト helper)
- `src/app/native_video.rs` (イベント分類 + activation 変換)
- `src/app.rs` (helper が必要な場合)
- `src/app/tests.rs`
- `docs/detached-rework-stage-audio.md` (実装メモ)

### コミット

`(detached-rework stage-audio fix6c)`。fix6b (native presenter HUD dim) とは別コミット。

### fix6c 実装メモ (Codex 2026-07-08)

- ParkedLive 音楽窓は `draw_parked_live_music_window` の 54px ヘッダを正本とし、
  passive 共通バーは `CloseOnly` 分岐で × ボタンだけ描く。× rect は引き続き
  `detached_image_window_bar_close_button_rect()` 由来で、画像/PDF passive の full bar は不変。
- 右端ヒントは × と重なるため削除し、中央ヒントのみ維持。動画→音声モードの parked 窓では
  中央ヒントを「動画の音声操作に戻る」に変える。
- parked layout は active 音楽ビューの帯計算に合わせ、`band_top = top + 54`、
  `band_bottom = bottom - MUSIC_HUD_HEIGHT - 4`、`spectrum_h = 180.min(band_h - 8 - 120)`。
  下端の `MUSIC_HUD_HEIGHT` 帯には、操作不可の簡易 HUD (seek bar + 主要ボタン風表示 +
  時刻) を減光表示する。`draw_music_bottom_hud` は入力・player 依存が強いため流用せず、
  見た目相当の inert HUD にした。
- native ParkedLive の HUD semantic event は App 側で分類する。`Window` / placement /
  hover thumbnail 系は activation にせず、それ以外の HUD command は機能を実行せず
  `native_video_parked_live_activation_requests` に dedup して積む。
- 回帰テスト: parked layout の spectrum rect、CloseOnly bar 分岐、HUD event 分類と
  activation 変換 (ToggleAudioMode が音声モードを実行しないこと) を固定。

### fix6b/fix6c 検収 (Fable 2026-07-08): fix6b 合格 / fix6c は 1 点差し戻し → fix6c-2

**fix6b (317ac6f2) = 合格**。承認条件 5 点すべて充足を確認: App 新規フィールドなし
(`native_video_hud_dimmed_for_current_poll()` = 既存 `native_video_parked_live_input_window_id`
からの導出のみ)・毎 poll 冪等送信で set/unset 対称 (parked poll 区間=true / 通常 poll=false)・
SwitchPlacement 再構築時の再適用あり・dim は top bar 54px + bottom HUD 帯のみで
`interactable(false)` = hit-test/挙動不変・時間窓なし・別コミット。
(P3 polish、非ブロッキング: `set_hud_dimmed` は `set_checked` の `last_*` AtomicBool dedup
パターンを持たず毎 poll channel 送信する。overlay 側 early-return で実害なし。fix6c-2 の
ついでに直してよいが必須ではない。)

**fix6c (3d87e253) = レイアウト一致 / 単一ヘッダ / CloseOnly / 簡易 HUD はすべて指示どおり。
1 点差し戻し**:

- **指摘: catch-all 分類がホイールをアクティブ化に変換してしまう**。presenter overlay は
  **無修飾ホイールを `NavigateItem` に、Ctrl+ホイールを `TileColumnsDelta` に変換する**
  ([native_presenter/mod.rs:4257-4267](../src/video/native_presenter/mod.rs))。このとき
  `consumed_wheel` ([mod.rs:987-993](../src/video/native_presenter/mod.rs)) が raw
  `Window(MouseWheel)` の二重転送を抑止するため、**実機のホイールは semantic イベント
  だけが App に届く**。fix6c の分類 (`_ => true`) はこれを HUD クリック扱いにするので、
  parked 動画窓上のホイールがアクティブ化に化ける = fix6c 要件 4「wheel/key は破棄のみ」
  違反 (R2b F5 以来の「クリックのみ復帰」規則の退行)。fix6c 以前は `NavigateItem` が
  blocked 側に落ちて no-op だった (= 従来の parked ホイール no-op の実体)。
  追加したテストは `Window(MouseWheel)` しか検証しておらず、この実経路を検出できない。

### fix6c-2 要件

1. **推奨 (案 B): `NavigateItem` に起源を持たせて事実で判定する** (憲法 §5 の「事実で判定」)。
   - `NativeOverlayCommand::NavigateItem` / `NativeVideoOutputEvent::NavigateItem` に
     `via_wheel: bool` (または origin enum) を追加。発火点は 3 箇所のみ:
     ホイール変換 ([native_presenter/mod.rs:4264](../src/video/native_presenter/mod.rs)) =
     wheel、HUD 前/次項目ボタン ([mod.rs:6633 / 6659](../src/video/native_presenter/mod.rs)) =
     button。マッピング ([video/mod.rs:1559](../src/video/mod.rs) /
     [video/mod.rs:3249-3257](../src/video/mod.rs)) は素通しで伝搬。
   - 既存 consumer ([native_video.rs:944 / 2695](../src/app/native_video.rs)) は origin を
     無視して従来どおり動作 (アクティブ窓の挙動不変)。
   - 分類関数: `NavigateItem { via_wheel: false, .. }` → activation / `via_wheel: true` →
     破棄 (no-op)。`TileColumnsDelta` は **ホイール由来のみ** (Ctrl+ホイール) なので無条件で
     activation 対象外 (false 側) に移す。
   - 配管が予想外に大きくなる場合は代替 (案 A): `NavigateItem` / `TileColumnsDelta` を
     両方 false 側に移す (= parked 中は前/次ボタンのクリックでアクティブ化しない制限を
     実装メモに明記)。案 A を選ぶ場合は先に一言報告する。
2. **テスト**: 実経路で書く —
   - `NavigateItem { via_wheel: true }` (parked 中) → activation **されない** + 機能も実行
     されない (no-op)。
   - `NavigateItem { via_wheel: false }` (parked 中) → activation 変換 (案 B の場合)。
   - `TileColumnsDelta` (parked 中) → activation されない。
   - 既存の fix6c テスト (`parked_live_native_hud_commands_request_activation_only`) は維持
     しつつ、上記を追加。
3. (任意) fix6b の `set_hud_dimmed` に `last_*` dedup を追加 (上記 P3)。
4. **既知の限界 (修正不要、実装メモに記載のみ)**: ブックマーク名編集モーダルの Enter 確定
   (`SetBookmarkTitle`) はキー由来だがクリック起源 (OK ボタン) と共用のため activation 側の
   まま。parked 窓でモーダルが開いたまま、という経路自体が事実上ない。

### fix6c-2 の触ってよいファイル / コミット

- `src/video/native_presenter/mod.rs` (NavigateItem 発火点への origin 付与のみ)、
  `src/video/mod.rs` (enum + マッピング)、`src/app/native_video.rs` (分類)、
  `src/app/tests.rs`、本 doc。
- コミット `(detached-rework stage-audio fix6c-2)`。

### fix6c-2 実装メモ (Codex 2026-07-08)

- `NativeOverlayCommand::NavigateItem` / `NativeVideoOutputEvent::NavigateItem` に
  `via_wheel` を追加し、native presenter のホイール変換は `true`、HUD の前/次項目
  ボタンは `false` として伝搬する。
- active 窓の通常処理は `via_wheel` を無視して既存どおり動作する。ParkedLive filter
  だけが origin を読み、`via_wheel=true` は no-op、`via_wheel=false` は HUD クリック
  として復帰要求に変換する。
- `TileColumnsDelta` は Ctrl+ホイール由来のため ParkedLive では activation 対象外にした。
- 回帰テストでは `NavigateItem { via_wheel: true }` / `TileColumnsDelta` が復帰しないこと、
  `NavigateItem { via_wheel: false }` が復帰要求になることを固定した。

## 3.15 実機 FB (2026-07-08): fix6d = 音楽 chrome の parked/active パリティ + fix6b-2 = native 下 HUD 減光が効かない

fix6b/fix6c ビルドの実機 smoke でユーザー FB 2 件 (スクリーンショット 4 枚):

- **①** 音声 (音楽ビュー) 窓の parked と active で **上下 HUD の項目が違う**。
  上部: active は [−][Row 30s][+][VST][表示切替][×] 等のボタン列 + タイトル (「タイトル /
  ファイル名 (拡張子なし)」形式)、parked は タイトル (拡張子付きファイル名) + 「再生中」+ ×
  のみ。下部: active はフル HUD (シークバー + 頭出し/再生/ループ/連続/前後マーカー/前後
  ファイル + 時間 + 速度 + 音量 + Norm + dB)、parked は fix6c の簡易 HUD (R/||/L/C プレース
  ホルダ)。**ユーザー意図 = 同じ項目構成を減光して表示** (fix6c-1 の「減光して表示」決定の
  趣旨。簡易 HUD では不足)。
- **②** 動画 parked 窓: **上バーは減光されるが下 HUD が減光されない** (fix6b の
  `draw_native_hud_dim_overlay` は top/bottom 両方に dim 帯を重ねる実装で、コード上は
  bottom も同座標・同フラグで描くはずのため、机上では原因未確定 → Phase 1 調査)。

### fix6d 要件 (①、egui 側)

1. **音楽ビュー chrome の単一ソース化**: active 音楽ビューの上部バー / 下部 HUD の描画を
   「表示状態 struct (+ inert/dimmed flag)」を受け取る描画関数に切り出し、active
   (`draw_fs_music_view`) と parked (`draw_parked_live_music_window`) の**両方から同じ関数を
   呼ぶ** (今後項目が増えても構造的に乖離しない)。
   - fix4 以来の「`draw_fs_music_view` の bundle 引き剥がしリファクタ禁止」は、**この
     chrome 2 段の切り出しに限り解除**する (関数は bundle 非依存の入力 struct のみ受け取る
     形に限る。timeline/パネル/入力処理の切り出しはしない)。
2. parked 側の入力値: position/duration/playing は既存 info、再生速度など player 由来の
   表示値が必要なら `ParkedLiveMusicWindowInfo` に read-only で追加 (position と同じ
   bundle 読みパターン)。ループ/連続/音量/Norm は global (`video_loop_mode` /
   `video_continuous_mode` / settings) から。
3. parked 描画は **同項目・減光・inert** (クリックは復帰のみ、既存 activation 経路不変)。
   タイトル表記も active と同形式に揃える。
4. **× の重複回避**: parked の × は既存 CloseOnly ボタン
   (`detached_image_window_bar_close_button_rect` 由来、watcher と単一ソース) を維持し、
   パリティ chrome 側の × は parked では描かない (`show_close: false` 等の引数)。
5. 中央領域は現状のまま (タイムライン非表示 + アイコン + クリック復帰ヒント、fix6 決定)。
6. テスト: chrome 描画関数が active/parked で同じ入力 struct を使うことを型で保証した上で、
   parked 側の状態値マッピング (global 設定 → struct) を純関数テストで固定。

### fix6b-2 要件 (②、native 側 — Phase 1 調査 → 報告 → 修正)

1. **Phase 1**: `hud_dimmed=true` で top 帯は効き bottom 帯が効かない機構を特定する。
   候補: (a) `native_video_seek_hud` Area と dim Area の z-order が Area order memory で
   期待と逆 (b) bottom 帯の rect / 座標系ズレ (c) `bottom_hud_visible` が render 時点で
   false になる timing (d) HUD child window (hud_window.rs) 側の present 経路の関与
   (e) そもそも parked presenter に dim=true が届いていない (top の「減光して見える」が
   別要因)。必要なら dim 描画箇所に一時 debug ログ (hud_dimmed / top_visible /
   bottom_visible / rect) を入れて実機 1 回で確定してよい。
2. 特定後に修正。**dim 帯の重ね描きに固執しない** — HUD 描画色に dim 係数を直接適用する
   方式への変更も可 (fix6d の音楽側と見た目の一貫性が出る)。挙動 (hit-test / command /
   filter) は不変のまま。
3. コミットは fix6d と分ける: `(detached-rework stage-audio fix6b-2)`。

### fix6d/fix6b-2 の触ってよいファイル

- fix6d: `src/ui_fullscreen.rs` (chrome 切り出し + parked 呼び出し)、`src/ui_music_panels.rs`
  (HUD 描画の切り出し先がこちらの場合)、`src/app.rs` (`ParkedLiveMusicWindowInfo` 拡張)、
  `src/app/tests.rs`、本 doc。
- fix6b-2: `src/video/native_presenter/mod.rs` / `overlay_draw.rs`、(必要なら)
  `src/app/native_video.rs` の診断ログ、`src/app/tests.rs`、本 doc。
- fix6c-2 (NavigateItem origin) が未完なら**先に fix6c-2 を完了**してから着手する
  (native_video.rs / mod.rs の同時編集を避ける)。

### fix6d 実装メモ (Codex 2026-07-08)

- `MusicChromeViewState` を追加し、active/parked の音楽 chrome が同じ表示値
  (タイトル、再生位置、速度、音量、loop/continuous、Row 秒、Norm 状態など) から
  描けるようにした。parked の値は `ParkedLiveMusicWindowInfo` へ read-only で拡張。
- ParkedLive 音楽窓の上部 chrome は active と同じボタン構成
  (Row、VST、動画へ戻る、window toggle) を dim/inert で描く。× は既存の CloseOnly
  ボタンを watcher と共有するため chrome 側では描かない。
- ParkedLive 音楽窓の下 HUD は簡易プレースホルダをやめ、active と同じ項目構成
  (seek、頭出し、再生、loop/continuous、前後項目、前後マーカー、時間、速度、
  音量、Norm、dB) を dim/inert で描く。入力は従来どおり復帰経路に任せ、HUD 機能は
  実行しない。
- 回帰テストでは parked layout と close-only に加え、`MusicChromeViewState` が active
  chrome 相当の項目を保持しつつ close だけ外部ボタンへ委譲することを固定した。

### fix6b-2 調査・実装メモ (Codex 2026-07-08)

- top dim が効き bottom dim が見えない原因は、bottom HUD が `native_video_seek_hud`
  `Area(Order::Foreground)` 内で描かれ、別 `Area` の dim overlay が egui の Area order
  memory に左右される余地があることだった。top は standalone bar なので従来 overlay で
  十分だったが、bottom は同じ Foreground Area 同士の順序に依存する構図になっていた。
- bottom HUD は `native_video_seek_hud` Area の全コントロール描画後に、同じ painter で
  `hud_rect` へ dim rect を直接塗る方式に変更した。hit-test / command / filter は
  触らず、見た目だけを同一 pass 内で完結させる。
- `draw_native_hud_dim_overlay` は top のみを担当し、bottom には使わない。これで上バーと
  下 HUD の両方が dimmed=true に追従しつつ、Area z-order 依存を排除する。

### fix6c-2 / fix6b-2 / fix6d 検収 (Fable 2026-07-08)

- **fix6c-2 (62588a67) = 合格**。案 B どおり `via_wheel` を 3 発火点 (wheel 変換 4264 =
  true / HUD 前後ボタン 6633・6661 = false) に付与、マッピング 2 箇所素通し、分類は
  `NavigateItem { via_wheel } → !via_wheel` + `TileColumnsDelta → false`、既存 consumer は
  `..` で不変。テストあり。
- **fix6b-2 (a6d8f3e6) = 合格**。bottom を同一 Area 内の in-place dim に変更 (Area z-order
  依存の排除)、top は従来 overlay。挙動不変。実機でも上下とも減光確認済み。
- **fix6d (859207bc) = 差し戻し → fix6d-2 (§3.16)**。共有したのは state struct
  (`MusicChromeViewState`) とアイコン関数のみで、**レイアウト描画は parked 専用の再実装**
  (`draw_parked_live_music_top_chrome` / `draw_parked_live_music_bottom_hud`) のまま =
  要件 1「同じ関数を両方から呼ぶ」未達。実機で乖離が残存 (§3.16 ③④)。

## 3.16 実機 FB 第 2 ラウンド (2026-07-08): fix6d-2 (真の単一ソース化) / fix6e (parked raw click passthrough) / fix7 開始

実機 FB 4 件と機構:

- **① HUD の無ボタン領域・時間表示のクリックでアクティブ化しない**。機構確定 (native):
  raw `MouseButton` の App 転送は `!wants_pointer_input` ゲート
  ([native_presenter/mod.rs:1393-1395](../src/video/native_presenter/mod.rs)) を通るため、
  HUD chrome 帯の上では egui overlay がポインタを要求して raw click が**転送されない**。
  widget に当たれば semantic → fix6c 変換で activation ✓、**dead zone (時間表示・余白) では
  何も App に届かない** = 報告どおり。→ fix6e。
- **② 連続再生ボタンの active 青背景が parked で消える (Norm は正常)**。機構確定 (native):
  parked poll の `video_continuous_mode = Off` 強制 (fix7 未実装の現行仕様) が、poll_video の
  `player.set_native_continuous_mode(continuous_mode)` ([app.rs:45182](../src/app.rs)) 経由で
  **HUD 表示にまで漏れる** (Norm は settings 直読みなので無事)。→ **根治 = fix7** (連続
  再生 ON の尊重で強制自体が消える)。egui パリティ側は render 時の復元値を読むため理論上
  正しい — 音声窓でも消える場合は fix7 Phase 0 で info 生成経路を確認して報告。
- **③ 音声モード動画/通常音声とも parked の HUD 項目が active と違う** / **④ シークバーの
  マーカー縦線が parked で消える** (動画 native は残る)。原因 = fix6d の再実装 (検収記載)。
  速度表記 (`x1` vs `1.00x`)・Norm/音量の並び・再生ボタンの active 背景解釈・マーカー
  非描画などが乖離。→ fix6d-2。

### fix6d-2 要件 (差し戻し、egui 側)

1. **レイアウトコードそのものを共有する**: active 音楽ビューの上部バー + 下部 HUD の
   描画本体を `MusicChromeViewState` + `interactive: bool` を取る関数に**移動**し、active は
   interactive=true (既存のクリック/ドラッグ/ツールチップ処理をそのまま内包 or 応答 rect を
   返して呼び出し側処理)、parked は interactive=false + dim で**同じ関数**を呼ぶ。
   アイコン関数単位の再利用は不可 (fix6d の轍)。
2. **シークバーのマーカー縦線を共有描画に含める**: marker データ (music bookmarks 等、
   active 側が描いているもの) を struct に載せる。parked 時に global `music_bookmarks` が
   parked path のまま有効かを確認し、ズレる場合は bundle から read-only 取得 (取得元を
   実装メモに明記)。
3. ボタンの状態背景 (再生/ループ/連続/Norm/ミュート) の導出は active 側の既存解釈を共有
   (parity 独自の「playing で青」等を持ち込まない — 共有化で自動的に一致する)。
4. dim は共有関数内の色係数 (interactive=false 時) で行い、fix6b-2 と同じ in-place 方式。
5. 完了条件: **同一 state を与えた active/parked の描画が dim 以外で画素一致することが
   「同一関数」により構造的に保証**されていること。
6. テスト: interactive=false が入力 (ui.interact の click sense) を発生させないこと +
   既存 chrome state テスト維持。
7. コミット `(detached-rework stage-audio fix6d-2)`。

### fix6e 要件 (①、native 側 — 短い Phase 0 報告 → 実装)

1. **parked (hud_dimmed) 中は raw `MouseButton` を App へ必ず転送する**:
   [mod.rs:1393-1395](../src/video/native_presenter/mod.rs) のゲートに hud_dimmed
   バイパスを追加 (`hud_dimmed || (!wants_pointer_input && !modal_dialog_active)` 相当)。
   HUD は parked 中 inert (App filter) なので、egui が消費したクリックでも raw を流して
   よい — 既存の左クリック down/up → activation 経路に乗る。active (dimmed=false) の
   routing は不変。Phase 0 として影響範囲 (modal gate の扱い / 右クリック / wheel は
   変えない) を一言報告してから実装。
   - 注: hud_dimmed は fix6b で「視覚のみ」としたが、parked の inert 状態と同一の事実
     由来なので、この入力 passthrough への利用は許容する (Fable 判断)。フラグ名/コメントで
     「parked chrome 状態 (視覚 dim + raw passthrough)」であることを明示する。
2. ボタンクリックが semantic 変換 (fix6c) と raw click の**二重経路**になるが、既存の
   activation dedup で 1 回に収束することをテストで固定。
3. **音声モード parked 窓の確認**: hidden presenter の HUD 入力窓 (hud_window) が生きて
   egui 音楽ビューへのクリックを奪っていないか確認する。奪っている場合は presenter hide と
   同時に HUD 入力窓も不活性化 (または同じ passthrough)。結果を実装メモに記載。
4. コミット `(detached-rework stage-audio fix6e)`。

### fix6d-2 実装メモ (2026-07-08)

- 上バーは `draw_music_top_chrome(MusicChromeViewState, interactive)` に移動し、active
  は response を適用、ParkedLive は `interactive=false` で同じ関数を呼ぶ。ParkedLive の
  閉じる × は watcher と共有する外側ボタンを維持するため `show_close=false`。
- 下 HUD は active 既存の `draw_music_bottom_hud` を `MusicChromeViewState` 入力に変更し、
  ParkedLive も `interactive=false` で同じ関数を呼ぶ。旧 `draw_parked_live_music_top_chrome`
  / `draw_parked_live_music_bottom_hud` は削除。
- シークバー marker は `MusicChromeViewState.bookmark_secs` に載せる。ParkedLive では
  global `music_bookmarks` ではなく、parked bundle 内の `music_bookmarks` /
  `music_bookmarks_loaded_for` から read-only 取得するため、メイン側で別ファイルを開いても
  parked 窓の marker がズレない。
- `interactive=false` の chrome button sense は `hover` に落とし、click sense を作らない。
  下 HUD は描画後に dim overlay を重ね、操作 intent は App/player に適用しない。

### fix6e Phase 0 / 実装メモ (2026-07-08)

- 影響範囲は native presenter の raw `MouseButton` 転送だけ。active (`hud_dimmed=false`) の
  routing は従来どおり `wants_pointer_input` / modal gate を使い、wheel は `consumed_wheel`
  と `wants_pointer_input` の既存規則を維持する。
- ParkedLive (`hud_dimmed=true`) では HUD chrome が inert で App 側 filter により機能実行
  されないため、raw `MouseButton` は modal/wants pointer に関係なく App へ転送する。
  semantic HUD command と raw click の二重経路は既存 activation request dedup で 1 回に
  収束することをテストで固定した。
- 右クリックも raw button として転送対象だが、App 側の ParkedLive activation は左 click
  down/up のみを見るため、右クリック機能は実行されない。wheel / key は従来どおり inert。
- 音声モード parked の egui 音楽ビューは native presenter の HUD HWND ではなく通常 egui
  側で描く。`set_hud_window_visible` は定義のみで現行経路からは呼ばれておらず、今回の
  native 映像 parked HUD dead zone とは独立。実機で奪いが確認された場合は別途 HUD HWND
  visibility/region の責務で扱う。

### fix7 開始 (②の根治を含む)

fix6d-2/fix6e の後、§3.13 のとおり **Phase 0 調査 → Fable 承認 → 実装**。追加要件:

- parked 中の native HUD 連続再生表示が**ユーザー設定と一致**すること (loop の
  「HUD は user intent」コメント [app.rs:45180](../src/app.rs) と同じ扱い)。設定 OFF なら
  従来どおり停止 + 表示 Off。
- Phase 0 で、egui パリティ chrome の連続再生状態が parked で正しいか (render 時復元値) も
  確認し、実機で消えていた場合は info 生成経路を報告に含める。

### fix7 Phase 0 調査結果 (Codex 2026-07-08)

#### 現状の機構

- `poll_parked_live_detached_windows` は parked bundle を `swap_viewer_context_bundle` で
  App に mount してから `poll_video(ctx)` を呼ぶが、その直前に
  `video_continuous_mode = Off` を一時適用している。したがって `poll_video` 内の
  `continuous_enabled` が false になり、`handle_music_continuous_eof` /
  `handle_video_audio_mode_continuous_eof` / `handle_video_continuous_eof` は発火しない。
- parked 音楽 chrome の表示値は `viewer_context_bundle_music_window_info` が作る
  `ParkedLiveMusicWindowInfo.continuous_mode` から描画される。この値は render 時の
  `self.video_continuous_mode` (ユーザー設定) で、parked poll 中の一時 Off からは直接影響を
  受けない。egui パリティ chrome は現在の実装では**表示だけならユーザー設定と一致する**。
- native presenter HUD は `poll_video` が毎 tick `player.set_native_continuous_mode(continuous_mode)`
  を送るため、parked poll 中は一時 Off が presenter 側へ渡る。従って **native parked HUD の
  連続再生表示は現状 Off に倒れる**。

#### (a) source-swap が parked 窓内で完結するか

**そのまま Off 強制を外すだけでは安全ではない。**

- 音声ファイル (`GridItem::Audio`) の EOF は `handle_music_continuous_eof` →
  `open_fullscreen_from_fs_navigation` → `open_fullscreen` へ進む。これは mount 中の bundle
  (`items` / `fullscreen_idx` / `fs_cache`) に対して同期的に完結するため、parked 窓内で
  完結できる見込み。
- 音声モード動画 / 通常動画の EOF は `try_start_native_video_fast_swap` 経由で
  `native_video_source_swap_pending` を積む。`native_video_source_swap_pending` は
  `ViewerContextBundle` の swap 対象ではなく App-global である。
- 現在の update 順序は main 側 `poll_video` が先、`poll_parked_live_detached_windows` が後。
  parked poll で積んだ source-swap pending は次 root frame の main 側 `poll_video` に先に
  見える。このとき main 側の `fullscreen_idx` / `items` は parked bundle ではないため、
  `poll_native_video_source_swap_pending` の
  `fullscreen_idx != Some(target_idx)` / `items[target_idx] != target_path` 判定で pending が
  abort され得る。

したがって、fix7 実装では次のどちらかが必要:

1. **推奨**: parked live media が存在する間は main 側 `poll_video` をスキップし、parked
   bundle を mount した `poll_video` だけが media pending を処理する。メディア窓は 1 本規則
   なので、main 側に同時に別 media が存在しない前提と整合する。
2. 代替: `native_video_source_swap_pending` / fast-swap pending を `ViewerContextBundle` 側へ
   移す。ただし影響範囲が大きく、fix7 の最小実装としては過剰。

#### (b) アクティブ本文脈を汚さないか

- `open_fullscreen` / `open_fullscreen_from_fs_navigation` 自体は mount 中なら bundle の
  `items` / `current_folder` / `fullscreen_idx` / `fs_cache` を更新するため、基本文脈は
  汚さない。
- ただし上記の通り native source-swap pending が App-global であるため、main 側 poll が
  先に触ると pending の abort / toast / close などが main 側状態に漏れる可能性がある。
  これを防ぐには **parked media 中の main poll 抑止**を同時に入れる必要がある。
- `fs_media_open_forced_presentation` は動画 EOF で一時的に立つが、
  `open_native_video_fullscreen_from_navigation_with_options` が fast-swap 開始時に clear する。
  現経路では one-shot リークの主因にはならない見込み。

#### (c) ParkedLive / 音声モードが維持されるか

- `poll_parked_live_detached_windows` は runtime state を変更せず bundle を poll して戻すだけなので、
  `ParkedLive` 自体は維持される。
- 音声モード動画は `handle_video_audio_mode_continuous_eof` が
  `source_swap_keep_audio_mode=true` を焼き込み、pending completion で
  `enter_video_audio_mode(ctx, target_idx)` を呼ぶ設計。pending を parked bundle mount 中に
  完了させられれば、音声モード維持は成立する見込み。
- pending が main 側で abort されると音声モード維持以前に次送りが成立しない。よって (a) の
  main poll 抑止が前提。

#### (d) Off 強制を外す/条件化したときの影響範囲

- Off 強制を完全に外すと、連続再生 ON の parked media は EOF で次へ進む。ユーザー決定と一致。
- 連続再生 OFF なら `video_continuous_mode.is_enabled()` が false のため従来どおり停止する。
- native HUD の連続再生表示もユーザー設定と一致するようになる。
- ただし実装時は main 側 `poll_video` 抑止をセットで入れること。Off 強制だけを外す単独修正は
  source-swap pending abort の競合を作るため不可。

#### 実装案 (承認待ち)

1. `should_poll_main_video_context()` を拡張し、active detached context に加えて
   **ParkedLive media window が存在する間も main 側 poll を止める**。判定は既存 runtime /
   snapshot から導出し、新規 App bool は追加しない。
2. `poll_parked_live_detached_windows` の `video_continuous_mode = Off` 強制を撤去し、ユーザー設定を
   そのまま `poll_video` に渡す。
3. 回帰テスト:
   - ParkedLive 音声 + 連続再生 ON: EOF で次 audio へ進み、active 本文脈 (`items` /
     `current_folder` / `fullscreen_idx` / `auto_aspect`) は不変。
   - ParkedLive 音声 + 連続再生 OFF: EOF で進まない。
   - ParkedLive 音声モード動画 + 連続再生 ON: source-swap pending が main 側 poll で abort
     されず、parked bundle mount 中に処理されること (少なくとも main poll 抑止述語を固定)。
   - native HUD / egui chrome の continuous 表示がユーザー設定と一致すること。

### fix6d-2 / fix6e 検収 + fix7 Phase 0 承認 (Fable 2026-07-08)

- **fix6d-2 (16061659) = 合格**。`draw_music_top_chrome` / `draw_music_bottom_hud` を active
  (21263/21291) と parked (4083/4148) の両方が呼ぶ真の単一ソース化 (正味 −258 行)。
  `MusicChromeViewState` に `bookmark_secs` / `bookmarks_loaded` を含めマーカー縦線も共有、
  dim は `music_chrome_dim_color`、入力は `music_chrome_click_sense` (interactive=false は
  `Sense::hover`) でテスト固定。ただし parked の `show_close: false` が右端スロットを詰めて
  しまい、CloseOnly × と window toggle の重なり + ボタン位置ずれを生んだ (実機 FB、
  §3.17 fix6f-a で修正)。
- **fix6e (fde0571e) = 合格**。routing gate に `hud_dimmed` バイパス (MouseButton のみ、
  wheel/move は不変) + routing テスト + dedup テスト。
- **fix7 Phase 0 = 承認 (実装案 1)**。`main 側 poll 抑止 + Off 強制撤去` を条件付きで承認
  (§3.17 fix7 実装条件)。

## 3.17 実機 FB 第 3 ラウンド (2026-07-08): fix6f (× スロット / タイルモード) + fix7 実装

実機 FB と機構:

- **① 動画 native HUD の連続再生 active 表示が減光時に消える (継続)**: 既知 (§3.16 ②)。
  parked poll の Off 強制が `set_native_continuous_mode` で HUD 表示に漏れる。
  **fix7 実装で解消する** (Off 強制撤去)。fix6f では触らない。
- **② parked の × が window toggle (F11) ボタンと重なり、ボタン位置が active とずれる**
  (音声モード動画 / 音声ファイル両方): fix6d-2 の `show_close: false` が close スロットを
  詰めるため、chrome の右端に window toggle が来て、44px バー座標の CloseOnly × と重なる。
- **③ タイルモード (サムネイルタイル表示) 中に park**: (b1) 上 HUD が減光されない。
  (b2) タイル一覧が非アクティブでもホバー反応する。
  **ユーザー決定 (Fable 提案 2026-07-08): タイルは非表示にせず「静止表示」にする** —
  parked (hud_dimmed) 中は overlay egui へのポインタ配送を止め、ホバー反応を殺す。
  クリックは fix6e の raw passthrough で復帰。音楽タイムラインの非表示はフォールバック
  ちらつきの構造対策であってタイルには当てはまらない。見た目の文脈が保たれ実装も最小。

### fix6f 要件

1. **(②) × スロットの単一ソース化**: `draw_music_top_chrome` は parked でも close スロットを
   **予約**し (レイアウトが active と画素一致)、× ボタンをそのスロットに置く。実装は
   どちらでも可 (推奨 = chrome が × を描き、parked では × だけ interactive
   [唯一の例外、クリックは既存の close 経路へ]。代替 = スロット rect を返す helper +
   CloseOnly ボタンをその rect に配置)。いずれも:
   - スロット rect は共有 helper (`music_chrome_close_slot_rect` 等) から導出し、
     **watcher の × hit 判定も音楽窓ではこの rect を使う** (画像/PDF passive は従来の
     `detached_image_window_bar_close_button_rect` のまま)。rect 不一致だと「window toggle を
     クリックすると watcher が close する」誤爆になるため、テストで rect 一致を固定する。
   - × クリックは close (activation ではない)。既存の close 優先処理は不変。
2. **(③-b2) parked 中の overlay ポインタ配送停止**: `hud_dimmed` 中は overlay egui に
   pointer move / hover を流さない (PointerGone 等)。ボタン hover・タイル hover・seek hover
   サムネイルが全て静止する。クリックは raw passthrough (fix6e) で復帰、wheel は従来どおり
   inert。タイル一覧・HUD の**表示自体は維持** (非表示化しない)。
   - 実装位置は overlay への event 配送 (`push_native_event` / pending_events) か egui input
     構築のどちらでもよいが、hud_dimmed=false への復帰で即座に通常配送へ戻ること。
3. **(③-b1) タイルモードの上 HUD 減光**: タイル表示中の top bar が dim を通らない経路を
   特定して減光を適用 (`draw_native_hud_dim_overlay` の gate / タイルモード専用 chrome の
   どちらかにあるはず)。
4. テスト: × スロット rect の一致 (chrome / close ボタン / watcher) + hud_dimmed 中は
   overlay egui にポインタが渡らないこと (routing/配送の純関数レベルで可)。
5. コミット `(detached-rework stage-audio fix6f)`。

### fix7 実装条件 (Phase 0 承認、Fable 2026-07-08)

実装案 1 (main 側 poll 抑止 + Off 強制撤去) を承認する。条件:

1. 抑止述語は既存 runtime / snapshot からの導出のみ (新規 App bool 禁止、Codex 案どおり)。
2. **安全条件 = メディア窓 1 本規則の構造保証をテストで固定**: 「ParkedLive media 窓が
   存在する間に main 文脈で動画/音声の再生を開始すると、parked 窓が close/差し替えられる
   (= main 再生と parked 窓が共存しない)」。既存テストがあれば参照を実装メモに記載、
   なければ追加する。この不変条件が main poll 抑止の前提 (main に生きた player がいる状態で
   poll を止めると main が固まるため)。
3. main poll 抑止中も presenter イベント drain が滞らないこと (parked poll の `poll_video` が
   毎フレーム走ることを確認)。
4. EOF の次メディア選択は parked bundle の items (メディア窓自身のフォルダの表示順)。
5. Phase 0 の回帰テスト 4 本 (連続 ON 進行 + 本文脈不変 / OFF 停止 / pending が parked
   mount 中に処理 / HUD・chrome の連続再生表示一致) を実装。
6. コミット `(detached-rework stage-audio fix7)`。fix6f と別コミット。

## 3.18 UX 改善 fix8 (ユーザー要望 2026-07-08): タイムラインズーム時に再生カーソルを画面内へ追従スクロール

**症状**: アクティブ音楽ビューで Row 30s の − / + ボタン (タイムラインズーム =
`music_timeline_row_secs` 変更) を押すと、再生位置カーソル (playhead) が表示範囲外に
行ってしまうことがある。

**ユーザー要望**: ズーム操作をしたら、再生カーソルが画面内に存在するようにタイムラインを
スクロールさせる。

### fix8 要件

1. `music_timeline_row_secs` が変わる**すべての入口** (− / + ボタン。keymap 等に他の入口が
   あれば列挙して同様に) で、変更後に「現在の再生位置を含む row」が表示範囲内に入るよう
   タイムラインの ScrollArea オフセットを再設定する。表示位置は playhead 行が
   ビューポート内に見えることが必須 (中央寄せにするかは任意、実装しやすい方でよいが
   どちらにしたか実装メモに記載)。
2. ズーム以外の挙動 (再生中の自動追従・手動スクロール) は変えない。ズーム操作の
   フレームだけの one-shot 補正にする (毎フレーム強制スクロールにしない)。
3. 対象はアクティブ音楽ビュー (main / detached / 音声モード動画すべて同経路)。parked は
   タイムライン非表示なので対象外。
4. テスト: 「position / row_secs / ビューポート高 / row 高 → 目標スクロールオフセット」の
   導出を純関数に切り出して固定 (ズーム前に画面外となる組で、ズーム後オフセットが
   playhead 行を含むこと)。
5. コミット `(detached-rework stage-audio fix8)`。fix6f / fix7 と別コミット、着手順は
   fix6f → fix7 → fix8 (同一ファイルの並行編集を避ける)。

### fix6f / fix7 / fix8 検収 (Fable 2026-07-08)

- **fix6f (f4586a18) = 合格 (留保 2 点)**。× は chrome が両モードで描き
  `music_chrome_close_slot_rect` 単一ソース + parked では close のみ interactive・CloseOnly
  撤去・rect 相違テストあり。ポインタ配送停止は `push_native_event` 冒頭の suppress +
  `set_hud_dimmed(true)` 時の PointerGone + 分類テスト。タイルモード top bar dim も追加。
  留保: (1) **watcher の × hit 判定が legacy rect のまま** (app.rs:1237、指示 1 の未消化。
  chrome × との不一致は数 px で実害は未発生 → fix6g-c で回収) (2) 実装メモが本 doc に
  未記載 (fix6g でまとめて可)。
- **fix7 (872f66b2) = 合格**。`parked_live_media_window_exists()` 導出述語 (新規 bool なし) +
  `should_poll_main_video_context` 拡張 + Off 強制撤去。承認条件どおり。
- **fix8 (e9946b01) = 合格**。Row ± ボタン (21308) と Ctrl+ホイール (9454) の両入口が
  `set_music_timeline_row_secs_from_input` に合流し one-shot flag → 次 timeline 描画で消費、
  ファイル変更/teardown でクリア、純関数 2 テスト。

## 3.19 実機 FB 第 4 ラウンド (2026-07-08): fix6g = parked ホバーで HUD が出ない / タイトルと Row 30s の重なり

実機 (fix6f/7/8 ビルド): タイル静止表示 OK・× スロット OK・その他改善確認。残 2 件:

- **① 動画 parked 窓でホバーしても (減光された) HUD が出ない**。ユーザー期待 = 非アクティブ
  でもホバーで dim HUD が出る。機構確定: `hud_visible()` は
  `pointer_pos.is_some_and(|pos| pos.y >= h - 220)` ([native_presenter/mod.rs:5575-5586](../src/video/native_presenter/mod.rs))
  で判定するが、fix6f の suppress が dimmed 中 `pointer_pos` を常時 None にする
  (`clear_overlay_pointer_for_dimmed_hud`) ため **hud_visible が恒久 false**。さらに
  suppress された MouseMove は `dirty` を立てず再描画も走らない。
- **② 上部タイトルが Row 30s ステッパー/右ボタン群と重なる** (長いタイトルで発生、
  active/parked 共通)。機構: `draw_music_top_chrome` の title 描画がクリップなしの
  `painter.text` で、右側要素 (Row ステッパー・VST 等) の上に伸びる。

### fix6g 要件

1. **(①) HUD 可視性はホバー追従を維持、egui pointer だけ遮断する**: dimmed 中も
   「可視性判定専用の raw カーソル位置」を更新し (`pointer_pos` とは別フィールド、または
   suppress 前に可視性入力だけ更新する構造)、`hud_visible()` はそれを参照する。
   suppress された MouseMove でも HUD の出入りが変わるときは `dirty` を立てて再描画する。
   MouseLeave で raw 位置をクリア (カーソルが出たら HUD が消える、通常と同じ)。
   egui への配送停止 (hover 反応なし・タイル静止) は fix6f のまま維持。
   - 期待挙動: parked 窓にカーソルを乗せると**減光された HUD がフェードイン**し、ボタンは
     無反応 (クリックは復帰)。カーソルを外すと通常どおり消える。
2. **(②) タイトルのクリップ**: `draw_music_top_chrome` で title (と「再生中」ラベル) の
   描画を右側要素の左端 (Row ステッパー群 or 右ボタン群の最左) − マージンでクリップする
   (`painter.with_clip_rect` / truncate)。共有 chrome なので active/parked 両方で直る。
3. **(c) watcher × rect の統一 (fix6f 留保の回収)**: parked 音楽窓の watch target に
   close rect 情報を持たせ、`detached_activation_close_button_contains` (app.rs:1237) が
   音楽窓では `music_chrome_close_slot_rect` を使うようにする (画像/PDF は従来どおり)。
   rect 選択のテストを追加。
4. テスト: ① の可視性判定 (dimmed 中の raw 位置で hud_visible が変わる / egui pointer は
   None のまま) を純関数レベルで固定。② はクリップ rect 導出の検証。
5. fix6f/6g の実装メモを本 doc に追記する (fix6f 分の後追い記載も含む)。
6. コミット `(detached-rework stage-audio fix6g)`。

## 4. 完了条件

- [ ] Phase S 報告 (§2 の 1〜6)。コミット不要 (調査ログ・診断追加のみ可)
- [x] Phase I: 実装 + テスト + ドキュメント。コミット `(detached-rework stage-audio)`
- [x] full test 実行 (既知フレークを単独再実行で確認) / `cargo fmt --check` / `python scripts/check_ui_glyphs.py` /
      `.\scripts\build-release.ps1`

補足: full test は `global_search::tests::cancel_stops_early` が Windows の access-denied
フレークで 1 回失敗したが、同テスト単独再実行は pass。detached/audio 関連の
`still_window_mode_key_tests` は 183 件 pass。

## 5. 実機確認 (ユーザー、チェックリストと合流)

1. ON モード: グリッドから音声を開く → メディア窓で再生 (spectrum / 波形 / 操作 OK)
2. 音声窓で F12 → main に戻る / 再度 F12 → detached (音切れなし)
3. 音声再生中に別窓クリック (park) → 再生継続 → クリック復帰 → 操作有効
4. 音声窓がある状態で動画を開く → メディア窓が動画に差し替わる (逆も)
5. VST ON で音声 detached → 音にチェーンが効いている / V キーの挙動が破綻しない
6. (fix6/fix6c/fix6d) 音声モード parked 窓: 別窓で PDF ホイール → 上グラフがちらつかない /
   下 spectrum は動く / ヘッダ文字が二重にならない (タイトル 1 本 + × のみ) / spectrum の
   位置が park⇄active で動かない / **上下 HUD の項目構成が active と同じで減光表示**
   (fix6d) / クリックでアクティブ化して操作復帰
7. (fix6b/fix6b-2/fix6c) 映像 parked 窓: **上バーと下 HUD の両方**が減光している / HUD
   ボタン (♪ 等) の上をクリックしてもアクティブ化する (fix6c、機能は実行されない) /
   HUD 外クリックでもアクティブ化する / **parked 窓上のホイールではアクティブ化しない**
   (fix6c-2)
8. (fix7) 本をアクティブにしたまま parked 音声/動画: 連続再生 ON で末尾 → 次へ自動進行 /
   連続再生 OFF → 停止 / どちらでもメイン (本) の一覧・フォルダが無傷 / **parked 中の
   連続再生ボタン表示がユーザー設定と一致** (青背景が消えない)
9. (fix6f) parked 音楽窓の × が右端スロットに収まり window toggle と重ならない / ボタン
   位置が active と一致 / × クリックで close (アクティブ化しない) / window toggle 位置の
   クリックで close 誤爆しない
10. (fix6f) 動画タイルモード中に park: 上 HUD が減光する / タイルはホバーに反応しない
    (静止表示) / クリックで復帰
11. (fix8) 音楽ビューで Row − / + を押す → 再生カーソルが常に画面内に見える
12. (fix6g) 動画 parked 窓にカーソルを乗せる → 減光された HUD がフェードイン (ボタンは
    無反応、クリックで復帰) / カーソルを外すと消える
13. (fix6g) 長いタイトルでも Row 30s / 右上ボタン群と重ならない (active / parked 両方)
6. OFF モード: 音声 F12 の 1 枚 detached が動作
