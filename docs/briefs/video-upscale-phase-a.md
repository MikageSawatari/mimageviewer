# 動画の拡大・縮小を mIV のシェーダで行う — Phase A

正本は [video-upscale-shader-plan.md](../video-upscale-shader-plan.md)、
backlog は [next-release-backlog.md](../next-release-backlog.md) **§1.47**。
**着手前に正本を通読すること。** 本書は正本の Phase A だけを実装単位に落としたもので、
判断の根拠は正本にある。食い違ったら正本が勝つ。

作業ブランチ: `video-upscale-shader` (worktree `C:\home\mimageviewer-video-upscale`)。
**master へは触らない。**

## 1. Phase A のスコープ

正本 §3 の Phase A のみ。**Anime4K (Phase B) には手を付けない。**

| 選択肢 | 中身 |
| --- | --- |
| OS に任せる | 従来動作 (DComp が拡大縮小)。GPU 固有の問題が出たときの退避先 |
| 標準 (既定) | 拡大・縮小とも Lanczos3 |
| シャープ | 拡大 NVIDIA Image Scaling、縮小 Lanczos3 |
| ニアレスト | 拡大 NEAREST、縮小 Lanczos3 |

**既定は現在の見え方を変えないこと。** 既定を「標準 (Lanczos3)」にするか「OS に任せる」に
するかは、実機で見比べてから決めて報告する。判断できなければ「OS に任せる」を既定にして
報告する (挙動不変が最も安全)。

**縮小が動画では新規の利点**になる (4K を 1080p ウィンドウで見るときのモアレ)。
「縮小のなめらかさ」は静止画とは**別設定**にする (正本 §3)。

物理等倍 (1.0 倍) は**リサンプルせず今の `CopySubresourceRegion` のまま**にする。

## 2. 変える構造 — 表示解像度サーフェス

正本 §2。swap chain のサイズを**映像の表示矩形の物理ピクセルサイズ**にし、
シェーダでソース解像度から表示解像度へ直接解決する。DComp の transform は位置合わせだけになる。

```
共有テクスチャ(1920x1080) → [grade pass] → [resample pass] → swap chain (表示矩形サイズ)
                                                            → DComp は M11=M22=1 + オフセット
```

触る場所:

- `create_video_swap_chain` ([render_core.rs:2396](../../src/video/native_presenter/render_core.rs:2396))
- `copy_frame_into_backbuffer` ([render_core.rs:2462](../../src/video/native_presenter/render_core.rs:2462))
  — **GPU 共有テクスチャ経路と CPU upload 経路の両方**を通る。片方だけ直さない
- `present_with_surface_swap` ([render_core.rs:2208](../../src/video/native_presenter/render_core.rs:2208)) /
  `present_reusing_surface` ([render_core.rs:2128](../../src/video/native_presenter/render_core.rs:2128))
- `render_once` ([render_core.rs:6099](../../src/video/native_presenter/render_core.rs:6099))

**`compute_video_visual_transform` ([render_core.rs:8585](../../src/video/native_presenter/render_core.rs:8585))
は無改造でよい。** サーフェスサイズを引数に取るので、表示解像度サーフェスでもリサイズ中の
古いサーフェスでも同じ式で正しい倍率が出る (正本 §2.2)。**改造が要ると思ったら、そこで止めて報告する。**

### 2.1 リサイズ中はサーフェスを差し替えない

正本 §2.2。リサイズ中は既存サーフェスのまま DComp に伸ばさせ (従来画質・追加コストゼロ)、
**落ち着いてから 1 回だけ**差し替える。**時間窓で「落ち着いた」を判定する必要が出た場合、
その根拠を報告に書くこと** (CLAUDE.md はタイマーで症状を吸収するのを禁じているが、リサイズ
静止の検出は入力イベントの終端であって症状の吸収ではない。既存の scroll settle / touch snap が
同種の先例)。

## 3. ⚠️ いちばん壊しやすいところ

### 3.1 grade pass の出力先が変わる

現在の grade pass は**動画解像度で backbuffer へ直接**書いている
([grade_pipeline.rs](../../src/video/native_presenter/grade_pipeline.rs))。
resample を最終パスにすると、grade は**ソース解像度の中間 RT へ出す**必要がある。

- grade はソース解像度のまま (コストを表示解像度へ持ち上げない)
- resample が最終パスとして backbuffer へ書く
- **色調・Creative LUT を使っていないときに中間 RT を作らない**こと。identity 経路が
  余計なコピーを 1 枚増やしてはならない

### 3.2 切替の瞬間にコンパイルも確保もしない

正本 §6。**フィルタを切り替える瞬間に、シェーダのコンパイルもテクスチャ確保も一切しない。**

現在の `set_video_grade` ([render_core.rs:3082](../../src/video/native_presenter/render_core.rs:3082))
はレンダースレッドで**同期的に `D3DCompile`** している。Phase A は本数が少ないので、
**pipeline 生成時に全変種をまとめてコンパイルして保持し、切替時は shader を差し替えるだけ**に
する。ビルド時 `fxc` プリコンパイル (正本 §4.3) は Phase B で本数が増えたときの要求であり、
Phase A では要らない。**この判断を変えるなら理由を報告に書く。**

一時停止中の切り替えも即反映すること。既存の `FramePresentationState` の
「Visible なら 1 回だけ再提示」をそのまま使う。

### 3.3 フォールバックを黙ってやらない

VRAM 不足・表示解像度が上限を超える等で表示解像度サーフェスを作れない場合、
**型付きの理由を持って** OS 任せへ落ちる。CLAUDE.md の禁止事項どおり、silent fallback にしない。

- 理由を perf event に出す
- 設定 UI のその場に理由を出す (正本 §7.2 が `processing_size_outside_note` の書式を指定)

## 4. 設定と UI

- 置き場所: **動画フルスクリーンの左パネル →「画像補正」→「フィルタ」タブ**
  (`NativeVideoAdjustmentTab::Filter`、[overlay_draw.rs:436](../../src/video/native_presenter/overlay_draw.rs:436))。
  現在 Creative LUT を置いている場所。**右パネル (メタ情報) には置かない** (正本 §7.1)
- **命名は静止画の `PostFilter` に揃える** ([adjustment.rs:34](../../src/adjustment.rs:34))。
  静止画は `標準（補間あり）` / `ニアレスト（補間なし）` / `シャープ拡大` / `アニメ塗り拡大`。
  **揃えない選択をするなら理由を報告に書く**
- **旧 egui 動画 UI へだけ設定を足さない** (native presenter が正)
- UI 文言に実装語を出さない (`シェーダ` / `swap chain` / `サーフェス` / `DComp` を出さない)
- `KeyAction` に「動画の拡大方法の切り替え」を足す (正本 §7.3)。
  `ini_name()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` / 呼び出し側 helper /
  [keymap.ini.default](../keymap.ini.default) を揃える

## 5. 触ってはいけないもの

- **detached 述語 / viewport 経路**。触る必要が出たら**着手前に止めて報告する**
  (CLAUDE.md「Detached viewer リワーク中のルール」)
- **下部 HUD / シークバーの表示判定** (`hud_visible` / `native_hud_bottom_visible_from_hover` /
  `compute_hud_regions`)。master 側が §1.101 (HUD 固定) で同じ場所を触っている。
  **このブランチからは触らない**
- `docs/next-release-backlog.md`。master 側が高頻度で書き換えているので競合する。
  記録は本書と正本へ書く

## 6. テスト

正本 §9.1。

- **目標サーフェスサイズの決定を純関数に切り出す**。フィルタ / 表示倍率 / 上限 / リサイズ中の
  各場合をテストする。GPU 不要であること
- `compute_video_visual_transform` の既存テスト群に、表示解像度サーフェス時 (M11=M22=1) を追加
- HLSL がシェーダモデル 5 でコンパイルできること
  (`grade_hlsl_compiles_for_shader_model_5` [grade_pipeline.rs:454](../../src/video/native_presenter/grade_pipeline.rs:454) と同じ形)
- 既定値で現在と同じ経路を通ること (**回帰**)
- 物理等倍で `CopySubresourceRegion` 経路のままであること

## 7. 計装

正本 §9.3。

- perf event: 選択されたフィルタ、サーフェス差し替え回数、フォールバック理由と件数、
  resample パスの GPU 時間
- `emit_vram_trace` に中間 RT の確保を含める

## 8. 完了条件

- `cargo fmt` 済み
- `cargo test -p mimageviewer --lib` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- ドキュメント更新: [video-upscale-shader-plan.md](../video-upscale-shader-plan.md) の
  ステータス、[video-architecture.md](../video-architecture.md)、
  [display-pipeline.md](../display-pipeline.md)、[spec.md](../spec.md)、
  `htdocs/mimageviewer/manual/`
- **報告に書くこと**: 既定をどれにしたか / 静止画と命名を揃えたか / grade pass の出力先を
  どう変えたか / リサイズ静止の判定方法 / フォールバックの型と表示

> **実機確認が要る項目** (正本 §9.2): 再生中の切替で映像が途切れないこと、リサイズ中に
> 差し替わらず静止後に 1 回だけ差し替わること、ウィンドウ⇔全画面、シーク、解像度の変わる
> ストリーム、マルチモニタ / DPI 違い、4K→1080p の縮小品質。
> **利用者不在のため、ビルドまで用意して確認手順を残すこと。**
