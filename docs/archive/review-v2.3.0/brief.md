# v2.3.0 出荷前 品質レビュー ブリーフ (共通参照用)

作成: 2026-07-09。レビュー対象は **コミット範囲 `7eff5a9e` (= v2.2.0) .. `01910684` (= HEAD)**。
362 コミット / 207 ファイル / +77k 行。基準コミットはこの 2 つの SHA に固定する
(タグや HEAD の移動に依存しない)。

## 背景

v2.2.0 → v2.3.0 の間に「複数ウィンドウ (detached viewer 構造リワーク)」と
「音声再生 (音楽ビュー + 動画→音声モード)」という 2 大機能を追加した。
開発中にバグが多発し、機能を当初計画よりシンプル化して収束させた経緯があり、
出荷品質に不安がある。実機検証は限られた操作パターンしかカバーできていないため、
**ロジック上起きうる問題を静的レビューで洗い出す**のが目的。

## レビュー対象のサブシステム別ファイルマップ (差分行数順)

### A. 音楽/音声再生統合 (music integration Inc0〜7)
- `src/ui_music_timeline.rs` (+2260, 新規) — DJ 波形タイムライン描画
- `src/ui_music_panels.rs` (+1667, 新規) — 音楽ビュー右/左パネル
- `src/ui_music_spectrum.rs` (+1501, 新規) — 108band スペクトラム描画
- `crates/music-core/` (analysis.rs +1477, beat.rs, effects.rs, timeline.rs) — 解析ロジック
- `src/audio_decode.rs` (+658, 新規) — 音声デコード
- `src/video/mod.rs` (+647) / `src/video/decoder.rs` / `src/video/audio.rs` — audio-only 対応
- `src/video/engine/` (actor.rs / state.rs) — EngineActor state machine 拡張
- `src/app.rs` の音楽ビュー・動画→音声モード (`video_audio_mode` / `video_audio_vst` /
  hidden presenter 方式 / keep_audio_mode source-swap / EOF 継続 Option A)

### B. detached viewer 構造リワーク (R0〜R2d + stage-audio + findings-19 fix1〜15)
- `src/app.rs` (+28k/-18k、大半がこのリワークと音楽統合) — DetachedWindowRuntime、
  reducer、placement 一本化、live-park、active/passive/parked ライフサイクル
- `src/app/native_video.rs` (+1895) — F12 host migration、動画 presenter 移送
- `src/video/native_presenter/` (overlay_draw.rs +722, mod.rs, native_window.rs)
- `src/dwm_transitions.rs` (+215, 新規)
- 正本: `docs/detached-rework-plan.md` (§2 憲法 = BA-1〜BA-7 の分類あり)。
  進捗・検収記録: `docs/archive/detached/detached-rework-findings-19.md` ほか findings シリーズ、
  `docs/detached-rework-stage-*.md`

### C. 入力系
- `src/keymap.rs` (+400) / `src/ring_shortcut.rs` (+308) / `src/app/gamepad_input.rs` (+487)

### D. その他
- `src/fs_animation.rs` (+293) / `src/logger.rs` (+166) / `src/settings.rs` (+232) /
  `src/ui_dialogs/preferences/pages.rs` / `src/ui_metadata_panel.rs` (+187)

## 主要モード述語 (D3 レビューの中心)

| 述語 | 場所 | 意味 |
| --- | --- | --- |
| `viewer_session_is_detached_or_switching` | src/app.rs:24775 | detached または切替中 (統一述語) |
| `detached_active_window_alive_wanted` | src/app.rs:25432 | active detached 窓の生存要求 |
| `detached_video_presentation_active_or_targeted` | src/app/native_video.rs:626 | 動画 presenter が detached 対象 (in-flight switch 含む) |
| `fs_music_view_active` | src/ui_fullscreen.rs:21347 | 音楽ビュー表示中 (~98 呼び出しの中央述語) |
| `fullscreen_uses_video_ring_context` | src/app/gamepad_input.rs:5527 | FS が動画リング文脈 (動画 or 音楽ビュー) |
| `is_detached_viewer_child` | src/video/mod.rs:281 | presenter が detached 子窓 |
| `video_audio_mode` (state) | src/app.rs | 動画→音声モード中の fs_idx |
| `video_audio_vst` (state) | src/app.rs | 音声モード VST GUI (Opening/Active) |

モード軸: {フル機能 1 ウィンドウ / マルチウィンドウモード / F12 detached (active・passive・parked)} ×
{グリッド / FS 画像 / native 動画 / 音楽ビュー(音声ファイル) / 動画音声モード / 動画音声+VST GUI} ×
{定常 / 遷移中 (F12 host migration・fast-swap・park/reopen)}

## 既知の残課題 (重複指摘は不要。ただし「悪化」「新たな波及」は指摘対象)

1. DetachedWindow (F12) × 音声モードの組合せは Inc7e の残作業で**未対応** —
   ただし「未対応の組合せに入れてしまえる/入ったら壊れる」ならそれは指摘対象
2. music-integration の docs 反映 (Inc8) と inert 残置削除 (step9) は未実施
3. v2.2.0 レビューの残: IO セマフォ (P2) / 改名 staleness (P3)
4. ParkedLive は BA-5 (既定サイズ placement) に非免疫 (ゲート C / R4 で再評価予定)

## 制約 (必読)

- **detached viewer はリワーク凍結中** (CLAUDE.md / detached-rework-plan.md §2)。
  detached 関連の指摘は修正パッチではなく、**BA 番号 (壊れた前提の分類) への
  マッピング付きで報告**する。応急処置提案は stopgap 形式 (撤去予定ステージ明記) のみ。
- このレビューでは**コードを修正しない**。指摘レポートのみ。

## 指摘の出力形式

各指摘は以下を含める:

```
[P1|P2|P3] <一言サマリ>
- 場所: file:line
- シナリオ: 具体的な入力・状態 → 誤動作 (ユーザー視点の症状まで書く)
- 根拠: コード引用 or 呼び出し経路
- (detached 関連なら) BA-x マッピング
- 確度: 高/中/低 (推測を含む場合は明記)
```

P1 = クラッシュ・データ破壊・機能不能・UI 長時間ブロック / P2 = 誤動作・リーク・
競合で稀に壊れる / P3 = 品質・保守性・軽微。

**false positive を恐れず、確度を正直に書くこと**。検収側で全件コード照合する。
