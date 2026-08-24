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
- **§1.59 (投影モードの一般化: 透視 / 立体射影 / 等距離) は `panorama_wgpu.rs` だけで完結**し、
  A とも B とも衝突しない。**B と並行して先行してよい**。動画へ移す数式をここで確定させる。
  ただし先送り判断 (2026-08-13) の再評価が要る。
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

- worktree は `vendor/` を **実体コピー** (junction 禁止)。`vendor/eframe` と
  `vendor/egui-wgpu` は **git 追跡下**なのでコピーしない (改行だけ違う 29 ファイルが M になる)。
  撤収は必ず `scripts/safe-worktree-remove.ps1` 経由。
- **新しい worktree で `cargo test` を走らせる前に、FFmpeg DLL を `target\debug` へ置く。**
  無いとテスト実行体が `STATUS_DLL_NOT_FOUND` (exit `-1073741515`) で即死し、テストが 1 件も
  走らないまま落ちる。`Copy-Item vendor\ffmpeg\bin\*.dll target\debug\ -Force`。
  `build-dev.ps1` が置くのは `target\dev-runtime` の方なので、これとは別に要る。
- **レーンをまたいで native ビルドを同時に走らせない。** `turbojpeg-sys` の cmake は
  共有 cargo registry の libjpeg-turbo から MSBuild を回すため、2 つの worktree が同時に
  ビルドすると `.tlog` ディレクトリの作成で `MSB3191` (アクセス拒否) で落ちる。
  target dir が別でも起きる。`CMAKE_BUILD_PARALLEL_LEVEL` を下げても効かない
  (cmake クレートが自前の `--parallel` を渡すため)。
  **`build-dev.ps1` は他の `MSBuild` が居たら空くまで待つ** (既定 30 分、
  `-WaitForOtherBuildsMinutes 0` で無効化)。それ以外の経路で `cargo build` を打つときは
  `Get-Process MSBuild` を先に見る。
- テスト実行体が `0xC0000005` (アクセス違反) で落ちることがある。**間欠**で、同じコードが
  次の実行で通る。2 回続けて同じ位置で落ちるなら別の話として扱う。
- 共有作業ツリーでのコミットは pathspec commit (`git commit -- <自分のパス>`)。
- 既存 worktree: `detached-rework` と `video-upscale-shader` は master へ取り込み済みで撤収候補。
  `seek-thumb-bench` は 2 コミット未マージ (シークサムネイルの計測計装) — **レーン B で
  再利用価値があるので、master へ入れるかを B の着手時に判断する**。

## 6.1 レーン D (master 本体) の進捗 — 2026-08-25

A〜C が別 worktree にいるので、master 作業ツリーは小さめのタスクに使っている。

**完了 (すべて実機確認済み)**

| 項目 | 内容 |
| --- | --- |
| §1.121 | RAR を代表サムネにしても親フォルダに出ない。pin-root の発見条件からタイルの状態を外した |
| §1.120 | Susie ワーカーがクラッシュから戻らない。dispatcher の退役 / respawn / 連続失敗カウント / キュー排出 / **落とした対象の記憶** |
| — | [susie-crash-plugin.md](susie-crash-plugin.md): 落ちる Susie プラグインを自作。**この経路は検証手段が無く、一度も確かめられていなかった** |
| — | `build-dev.ps1` が他 worktree の native ビルドを待つ (§6 参照) |
| — | worktree 3 本撤収 (`rar-pin` / `detached` / `video-upscale`)、数十 GB 解放 |

**次に着手する予定**: §1.116 (メインウィンドウの起動状態を選べるようにする、0.5〜1.5 日)。
`lib.rs` / `settings.rs` を触るので A〜C と重ならない。⚠ 設計で 1 点: detached 側の
placement が「今のサイズ」と「戻すサイズ」を 1 個に同居させたのが §1.115 の原因なので、
メイン側は**最初から restore rect と maximized flag を別に持つ**こと。

**このセッション中に見つけて記録した別件** (どちらも未着手)

- §2.21 フルスクリーンが Susie プラグインの拡張子を画像として扱わない (サムネイルは通る)
- §1.95 追加項目: 取り消した編集が復元元として残り続ける (`has_restorable_content` が
  0 → 1 の単調遷移で下がらない)

**§1.120 の残り**: 再起動上限に達したときの利用者通知と、診断画面への表示。

## 7. リリース区切り (暫定)

- 次版 = レーン A の到達点 + レーン B。
- その次 = レーン C。

動画アップスケール (§1.47) は v3.2.0 で出荷済みなので、次版の目玉はまだ空いている。
