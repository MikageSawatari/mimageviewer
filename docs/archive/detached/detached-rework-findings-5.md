# 検収所見 #5: CUT 後の退行 2 件 (OFF モード F12 で本文フォント消失 + host 再取得の無限ループ)

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md) /
対象コミット: d3b56c15 / 64cddcab (Stage CUT)

**検収 NG・差し戻し**。実機 (2026-07-06 夜、複数ウィンドウ OFF) で「F12 切替後に
メインウィンドウの文字が全て消える」(スクリーンショットあり: UI が灰色矩形のみ、
タイトルバーの OS 描画テキストは正常 = main viewport の font atlas 破損の典型)。
ログは `%APPDATA%\mimageviewer\logs\bug-20260706-2135-cut-f12-text.log` に退避済み。

## CUT-1: OFF モードの F12 切替後、メインの文字が消える (font atlas 破損)

タイムライン (退避ログ):

```
19.5〜27.0s  native_video_backdrop_hide 理由の resync が複数ラウンド
             (generation 21〜35、直前まで動画を使っていた)
27.198s      toggle_detached_viewer_mode_begin enabled=false (main→detached、静止画 idx=15)
27.235s      session_begin window_id=2 (既存 window_id 再利用)
(以後、本セッション終了まで ui-fonts の schedule は 83.029s の
 detached_viewer_cleanup 1 回のみ)
```

- 直前の resync ラウンド (gen 31〜35、27.0s 完了) と 27.2s の F12 viewport churn が
  近接しており、この間に main atlas が desync した疑い。
- 破損後に修復用 resync がスケジュールされない (F12 の detached open 経路は
  resync を要求しない)。83.029s の cleanup resync (窓 close 時) で直ったかは未確認
  — **修正後の実機確認項目に「破損が起きないこと」と併せて含める**。
- 調査観点: CUT で F12 toggle (OFF モード) の close/open 経路が変わったことで、
  (a) atlas を壊すフレーム (テクスチャ upload 破棄 / discard pass との競合) が
  新たに生じていないか、(b) 壊れた場合に修復 resync を要求する経路が消えていないか。
  v1.8.0 の black-thumb 回帰 (FS close 直後の no-surface フレーム) と同族の可能性。

## CUT-2: F12 再 detach 後、host 登録が永久ループ (Opening→Opening ×859 回)

```
27.236s  keepalive_backstop が hwnd=0x26d099a を登録 (label=keepalive_backstop)
27.336s  show viewport: generation=1 host=0x26d099a alive=true
27.391s  host_lost_diag reason=host_lost_before_render frames_since_render=29
         → clear host window_id=2
27.391s〜 state_transition Opening→Opening reason=active_render +
         runtime_placement active_placement_update が毎フレーム (~10ms 間隔) で
         **859 回**繰り返し (回復せず)
```

- 登録直後 (alive=true) の hwnd が 100ms 後に host_lost 判定 → clear → 以後
  before/after 差分は毎フレーム `no_new_window` 相当で、**未請求窓の消去法採用が
  発火しない** (R1b の採用は「登録済み hwnd が show 中に死んだ」経路にのみ配線
  されており、hwnd=0 からの平常リトライ経路に無い) — 恒久未登録のまま。
- 毎フレームの `runtime_placement active_placement_update` 書き込みもこのループの
  一部で、placement 汚染のリスクがある。
- 調査観点:
  (a) なぜ登録直後の OS 窓が死んだのか (egui が窓を作り直した? CUT で toggle 時の
  window_id 再利用 / keepalive_backstop と active_render の順序が変わった?)。
  (b) 回復性: hwnd=0 の平常リトライが N フレーム続いた場合にも消去法採用を
  試みるべきか (geometry 推定には戻らない)。ループ検出時の警告ログも必要。

## 修正要件

1. CUT-1 / CUT-2 それぞれ根因を特定し、**ログの該当行を引用して**完了報告に書く
   (推測で複数箇所を同時に変えない)。
2. headless テスト: OFF モードの F12 往復 (main→detached→main→detached) で
   (a) host 登録が有限フレームで完了、(b) Opening→Opening が連続 N 回を超えない、
   (c) 遷移ストームの回帰 (F9) が再発しない。
3. font atlas 側は headless で固定しにくければ、「F12 の detached open/close 経路で
   resync 要求が期待どおり出る/出ない」のイベント列をテストで固定し、実機確認を
   ゲートに残す。
4. 既存テスト + full `cargo test` 緑。コミットに `(detached-rework CUT fix1)` を含める。

## 実機再確認 (修正後)

1. 複数ウィンドウ OFF で F12 を 4 往復 (静止画・PDF・動画各 1 回以上) →
   **メインの文字が消えない**・detached 窓が正常表示・ログに Opening ループなし
2. 万一文字が消えた場合: その場でログ退避 + 何秒時点かをメモ
