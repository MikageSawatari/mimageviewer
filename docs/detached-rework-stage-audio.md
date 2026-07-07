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
6. OFF モード: 音声 F12 の 1 枚 detached が動作
