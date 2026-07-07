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
