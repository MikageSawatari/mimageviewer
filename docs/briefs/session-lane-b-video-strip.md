# セッション指示書: レーン B — 動画シークバー近傍のストリップ (サムネイル列 ⇄ 音声波形)

体制: **別 worktree**で進める。レーン構成の正本は [next-cycle-work-lanes.md](../next-cycle-work-lanes.md)。

## 0. 先に読む

1. [next-release-backlog.md](../next-release-backlog.md) **§1.102** (YouTube 型のサムネイル列) と
   **§1.113** (音声波形への切替) — **この 2 件は 1 つの UI として設計する。片方だけ先に作らない。**
2. [video-architecture.md](../video-architecture.md) の `ThumbnailWorker` 節 (現行実装の正本)
3. [brief-seek-thumbnail-measurement.md](../brief-seek-thumbnail-measurement.md) **§7** —
   抽出コストの実測結果。§2〜6 は計測時点の baseline として読む
4. CLAUDE.md の「UI スレッドでの同期 I/O は即 worker 化する」と
   [ui-responsiveness.md](../ui-responsiveness.md) §4

## 1. 目的

動画のシークバーから上へドラッグしたときだけ開く**ストリップ**を作る。
中身は**サムネイル列**と**音声波形**をモード切替で出す。常時表示のサムネイル付きシークバーではない。

## 2. 前提は揃っている

- 抽出コストは実測済み: 4K AV1 60fps / GOP 約 7 秒で hover 待ちは p50 177ms / p90 575ms、
  支配項は **GOP 内 decode** (解像度や storage 速度は一次要因ではない)。
- 「シーク時のズレ許容 (秒)」設定は実装済み (既定 1.0、0.0 で従来の精密動作)。
- 波形側は `TimelineAnalysis` が**動画ファイルでも `Z` 波形モードで既に生成されている**。
  描画も `draw_music_timeline` のラスタキャッシュがある。

## 3. 設計で決めること

| # | 論点 | メモ |
| --- | --- | --- |
| 1 | 開く導線と閉じる条件 | シークバーから上へ一定量ドラッグ。固定シークバー時も通常は隠す |
| 2 | モード切替 UI | サムネイル列 ⇄ 波形。切替の記憶 (セッション / 永続) を決める |
| 3 | 抽出方式 | §1.102 の (A) 固定間隔を精密 / (B) キーフレームのみ / (C) 先に (B) を出して (A) へ差し替え / (D) 範囲先頭へ 1 回 seek して順方向にまとめて復号。**既存の「ズレ許容」設定と重複しないか**を先に整理する |
| 4 | 生成範囲 | 見える枚数 + 前後少量のみ。**全尺の先行生成をしない** |
| 5 | 波形の進行表示 | 全尺デコードが要る。progressive の途中経過をどう見せるか (既存挙動を流用できるか) |
| 6 | 解析の起動条件 | 現状 `ensure_music_analysis` は音楽ビュー時のみ。**ストリップを開いたときだけ**へ広げる。常時起動にしない |
| 7 | 永続キャッシュ | 音楽側は「直近 N 曲だけメモリ」で永続 DB を持たない。動画も同じでよいかを判断する |
| 8 | 既存機能との境界 | `S` のタイル表示、`B` の動画ブックマーク、シークバー hover の 1 枚プレビュー。**役割が被らないこと** |
| 9 | 別ウィンドウでの扱い | 独立窓 / `ParkedLive` でストリップを出すか。**parked-live の入力 allow-list を広げる場合は §4 を読む** |
| 10 | リモート | 出すか出さないか。出す場合は IPC に波及する ([video-upscale-shader-plan.md](../video-upscale-shader-plan.md) §10.7 が先例) |

## 4. 触ってよい範囲と、注意する境界

主戦場: `src/video/` (`thumbnail.rs` / `mod.rs`)、`src/video/native_presenter/`
(`overlay_draw.rs` / `render_core.rs`)、`src/app/native_video.rs`、`src/ui_music_timeline.rs`、
`crates/music-core/`、`settings.rs` / `keymap.rs` (追記)。

⚠ **`src/app/native_video.rs` の parked-live 入力 allow-list は detached リワークの領域**
(`native_video.rs:3496` 付近)。ここを広げる必要が出たら、
[detached-rework-plan.md](../detached-rework-plan.md) §2 の適用範囲に従い、
**「症状パッチではなく構造的修正である」ことを ClaudeCode と Codex の双方で合意**してから触り、
同書 §11 に記録する。

⚠ **レーン A-1 が `app.rs` を一括で切り替えるフェーズに入ったら、マージを止めるか先にリベースする**
(§5 の運用)。

## 5. worktree の準備

```powershell
git worktree add C:\home\mimageviewer-video-strip -b video-strip

# vendor は junction ではなく実体コピー (合計 ~325MB)。
# eframe / egui-wgpu は **git 追跡下**なので worktree の checkout に既にある。
# ここへ入れて上書きすると、改行だけ違う 29 ファイルが M になるのでコピーしない
# (入れてしまった場合は `git checkout -- vendor/eframe vendor/egui-wgpu` で戻す)。
$src = "C:\home\mimageviewer\vendor"; $dst = "C:\home\mimageviewer-video-strip\vendor"
New-Item -ItemType Directory -Force $dst | Out-Null
foreach ($n in @("ffmpeg","pdfium","ort","susie-worker","vst3-host","models","twemoji")) {
  robocopy "$src\$n" "$dst\$n" /E /XD target /NFL /NDL /NJH /NJS | Out-Null
}
```

撤収は必ず `.\scripts\safe-worktree-remove.ps1` 経由 (junction 事故防止)。

## 6. 先に判断すること

既存 worktree `C:\home\mimageviewer-seek-bench` (ブランチ `seek-thumb-bench`) に、
**未マージのシークサムネイル計測計装が 2 コミット**ある
(`video/thumbnail.rs` / `native_presenter/render_core.rs` / `scripts/analyze_seek_thumb.py`)。
ストリップの抽出方式を測るのに再利用できる。**master へ入れるか、この worktree へ取り込むか、
捨てるかを着手時に決める** ([brief-seek-thumbnail-measurement.md](../brief-seek-thumbnail-measurement.md) §6)。

## 7. スコープ外

- 360 度動画 (レーン C)。**同じ presenter とマウス入力を触るので同時に作らない。**
- presenter のスケーリング構造 (§1.47 は v3.2.0 で出荷済み。触らない)。

## 8. 出口

設計が固まったら `docs/video-seek-strip-plan.md` を正本として起こし、
backlog §1.102 / §1.113 からそこへリンクする。実装後は
[video-architecture.md](../video-architecture.md) と
`htdocs/mimageviewer/manual/video.html` を同時更新する (内部用語を出さない)。
