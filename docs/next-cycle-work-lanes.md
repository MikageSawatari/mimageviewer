# 次サイクルの作業レーン (v3.2.0 の次)

決定: 2026-08-23 (利用者判断)。対象は v3.2.0 公開後に着手する 3 テーマ。
個々の設計・症状の正本は [next-release-backlog.md](next-release-backlog.md) と
各プラン文書で、本書は**どれを並行させ、どれを直列にするか**だけを決める。

## 0. 決定

**同時に走らせるのは 2 本** (レーン A = 複数ウィンドウ / レーン B = 動画ストリップ)。
**レーン C (360 度) は B の後**。C の前段のうち静止画側だけは並行して先行できる。

## 1. レーン A — 複数ウィンドウの根治 (master 本体)

| 段 | 内容 | 依存 |
| --- | --- | --- |
| A-0 | §1.115 (閉じるたびに 5 フレーム破棄 → ちらつき、**P1**) と §2.20 (非一様 scale、P2) | R2e に依存しない。§2.20 は**直す前に計装** |
| A-1 | R2e = 所有の型化。[briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) の**第 3 版を書くところから** | BLOCKER 4 件が第 2 版に残っている |

- A-0 の §1.115 は「surface がどのフレームで戻るか観測せず、固定 5 フレームの時間窓で
  race を吸収している」構造。観測して 1 回だけ発行する形に直す (憲法 5)。
- A-1 は `ViewerContextBundle` の生成・保管・移動を registry + build transaction に閉じ、
  `syn` ベースの CI 監査でモジュール外の生成を弾く。**`app.rs` を一括で切り替えるフェーズがある**。
- 凍結ルール ([detached-rework-plan.md](detached-rework-plan.md) §2) は継続。症状パッチを入れない。

## 2. レーン B — 動画ストリップ (別 worktree)

- §1.102 (サムネイル列) と §1.113 (音声波形) を、**最初から 1 つのストリップのモード切替**として
  設計する。片方だけ先に作らない (backlog の明示条件)。
- 前提は揃っている: 抽出コストの実測 ([brief-seek-thumbnail-measurement.md](brief-seek-thumbnail-measurement.md) §7) と
  「シーク時のズレ許容」設定は済み。`TimelineAnalysis` は動画でも既に生成される。
- 主戦場: `src/video/` / `native_presenter/` / `src/app/native_video.rs` / `ui_music_timeline.rs`。

## 3. レーン C — 360 度パノラマ拡張 (B の後)

- §1.112 (360 度動画) の前提だった §1.47 (動画スケーリングを mIV のシェーダで) は
  **v3.2.0 で出荷済み**。「投影を差し込む場所が無い」という阻害要因は消えた。
- **§1.59 (投影モードの一般化: 透視 / 立体射影 / 等距離 / 等立体角) は実装済み**
  (2026-08-23、branch `panorama-projection`、実機未確認)。先送り判断は利用者へ再確認して解除した。
  **動画へ移す数式はここで確定**し、正本は [panorama-360-view-plan.md §13](panorama-360-view-plan.md)。
  想定より広く、`panorama.rs` / `panorama_wgpu.rs` に加えて `ui_fullscreen.rs` (ナビゲータの視野枠と
  上バーのボタン)、`app.rs` (settle overlay の pose)、`settings.rs` / `keymap.rs` / 環境設定ページに
  触れた。**レーン A と `ui_fullscreen.rs` / `app.rs` で重なるので、マージ順に注意する**
  (どちらも追加のみで、detached の述語・viewport 経路には触れていない)。
- 本体 (presenter への投影ステージ追加と見回し入力) は B の完了後。

## 4. なぜ B と C を並行させないか

どちらも **native presenter のオーバーレイと「動画上のマウスドラッグ」を新規に定義する**
(シークバーから上へドラッグ vs 見回し + FOV)。同じファイル・同じ入力経路を同時に作ると、
マージだけでなく設計判断が競合する。

## 5. A と B を並行してよい根拠 (実測)

直近の実装コミットの diff で主戦場が分かれている。

| | 触るファイル |
| --- | --- |
| 複数ウィンドウ | `app.rs` / `app/gamepad_input.rs` / `ui_fullscreen.rs` / `detached_window_manager.rs` / `app/tests.rs` |
| 動画 | `video/mod.rs` / `native_presenter/*` / `app/native_video.rs` / `settings.rs` / `keymap.rs` |

- §1.47 は **11,681 行の変更で `app.rs` を 32 行しか触っていない** (`f053625d`)。
- §1.100 (detached) は `app.rs` 347 行 + `gamepad_input.rs` 410 行で presenter を触っていない。
- 重なるのは `app/native_video.rs` (detached は parked-live の入力 allow-list のみ)、
  `settings.rs` / `keymap.rs` (追記のみ)。
- **例外**: A-1 の一括切替フェーズ中だけ `app.rs` が大きく動く。その期間は B のマージを止めるか、
  B を先にリベースする。

## 6. 運用

- worktree は `vendor/` を **実体コピー** (junction 禁止)。撤収は必ず
  `scripts/safe-worktree-remove.ps1` 経由。
- 共有作業ツリーでのコミットは pathspec commit (`git commit -- <自分のパス>`)。
- 既存 worktree: `detached-rework` と `video-upscale-shader` は master へ取り込み済みで撤収候補。
  `seek-thumb-bench` は 2 コミット未マージ (シークサムネイルの計測計装) — **レーン B で
  再利用価値があるので、master へ入れるかを B の着手時に判断する**。

## 7. リリース区切り (暫定)

- 次版 = レーン A の到達点 + レーン B。
- その次 = レーン C。

動画アップスケール (§1.47) は v3.2.0 で出荷済みなので、次版の目玉はまだ空いている。
