# 動画インライン再生 v1 リリース計画

## ゴール

[現在の状態 (素のインライン再生 + 基本入力)] から、リリースに耐える動画ビューア機能 を作る。1 つずつ独立して実装し、各ステップでビルドテストして安定を確認しながら進める。

## v1 で含める機能

| # | 機能 | 概要 | 影響範囲 | 推定規模 |
|---|---|---|---|---|
| 1 | UI 改善 (音量スライダー / ミュートボタン / 再生速度) | HUD バーの右側に音量スライダー + 🔊ボタン、左側に Play/Pause + 再生速度ドロップダウン (0.5x-2x) | `ui_fullscreen.rs` の `draw_video_hud` 拡張 | S |
| 2 | シークバー操作の精度向上 | ドラッグ中のリアルタイムシーク、ホバー時の時刻ツールチップ | `draw_video_hud` 内 | S |
| 3 | HEVC ハードウェアデコード (D3D11VA) | iPhone .mov (HEVC) と 4K HEVC を CPU 負荷低く再生 | `decoder.rs` 大幅追加 + RTX VSR への土台 | M-L |
| 4 | シーク用プレビューサムネイル | バックグラウンドで N 枚抽出、シークバー hover でプレビュー表示 | 新 `video/preview.rs` + HUD | M |
| 5 | 📌 代表サムネイル (1 枚) | 動画再生中にキー or ボタンで現フレームをグリッドサムネとして採用 | `mimageviewer.dat` 拡張、`thumb_loader` 連携 | S-M |
| 6 | 🔖 ブックマーク (上限なし) | 現フレーム + 時刻 + サムネを記録、右パネルに一覧表示、シークバーにマーカー | `mimageviewer.dat` 拡張、新右パネル `ui_video_panel.rs` | M |
| 7 | 再生速度変更 | 1 と統合 (UI 部分)、デコーダー側は AV クロックの倍率 + swresample のレート変換 | `clock.rs` + `audio.rs` | S-M |

## v1 では含めない (将来候補)

| 機能 | 見送り理由 |
|---|---|
| 字幕表示 | 描画パイプライン追加コスト大、需要要観察 |
| HDR トーンマッピング | 色管理パイプラインの全面見直しが必要 |
| 4K 以上の動画 | 現状は GPU テクスチャ上限 (`MAX_TEXTURE_DIM`) で自動縮小、実用上問題なし |
| ループ再生 | 設定項目自体は残すが UI 露出は v1.1 |
| AI アップスケール (Real-ESRGAN リアルタイム) | 1080p 1 フレーム ~200ms かかり、リアルタイム不可 (30fps 動画 = 33ms 予算)。**バッチ書き出し**としては実装可能だが v1 スコープ外 |
| **NVIDIA RTX VSR (RTX 30/40 系限定リアルタイム)** | 技術的には可能、3-4 週間規模の実装。v1 で D3D11VA 土台ができれば v1.1 で着手検討 |

## 採用しない実装方針 (検討して却下)

- **AI Real-ESRGAN による動画リアルタイムアップスケール**: 30fps を維持できない。バッチ書き出しなら実用的だが v1 スコープ外。
- **ファイル単位サイドカー (`<videofile>.miv`)**: ファイル数が増えるので採用せず。フォルダ単位 `mimageviewer.dat` に動画用セクションを追加 (静止画の補正情報と同じ流儀)。

## 設計詳細

### A. UI 改善 (#1, #2, #7 の UI 部分)

**HUD レイアウト** (下部 60px、現在は時刻 + シークバー + 音量テキストのみ):

```
┌─────────────────────────────────────────────────────────────────────────┐
│ [▶/⏸] [00:42 / 12:34]  [█████████──────────────] [1.0x▼] [🔊 75% ━━━] │
└─────────────────────────────────────────────────────────────────────────┘
  ↑                       ↑                          ↑       ↑
  Play/Pause              シークバー (clickable)    速度    音量スライダ
  (Space と連動)          + bookmark マーカー       (1.0x)  (drag/click)
```

- **再生 / 一時停止ボタン**: クリックで `player.toggle_play()`
- **時刻**: `MM:SS / MM:SS` (1 時間超は `HH:MM:SS`)
- **シークバー**:
  - クリック: その位置にシーク
  - ドラッグ: ドラッグ中は preview 位置を表示、ドロップで実シーク
  - hover: 時刻ツールチップ + 該当位置のプレビューサムネ (#4)
  - 🔖位置にマーカー縦線 (#6)
- **再生速度ドロップダウン**: 0.5x / 0.75x / 1.0x / 1.25x / 1.5x / 2.0x
- **音量スライダー**: ドラッグで 0-100%、🔊ボタンクリックでミュート切替
- **マウス無操作 2 秒でフェードアウト** (画像のメタデータパネル相当のロジック)

実装: `ui_fullscreen.rs` の `draw_video_hud` を分割 (`hud_layout.rs` に分離検討)。egui の widget で済む。

### B. HEVC ハードウェアデコード (#3)

**目的**: iPhone .mov (HEVC) と 4K HEVC を CPU 負荷を抑えて再生する。NVIDIA RTX VSR (v1.1 候補) の土台でもある。

**実装方針**:

1. `decoder.rs` の `Context::from_parameters` 後、コーデックが H.264 / HEVC / VP9 / AV1 なら hwaccel 設定を試みる
2. `AVCodecContext.hw_device_ctx` に D3D11VA デバイスをセット (`av_hwdevice_ctx_create(AV_HWDEVICE_TYPE_D3D11VA)`)
3. デコード結果は `AVFrame` の `format = AV_PIX_FMT_D3D11`。`av_hwframe_transfer_data` で CPU 側 NV12 に降ろす
4. swscale で NV12 → RGBA 変換 (現状の経路と統合)
5. デコーダー初期化失敗時は CPU デコード (現行) にフォールバック

ffmpeg-the-third から D3D11VA を扱う API は薄い (`hwcontext` モジュール) ので、`ffmpeg-sys-the-third` の生 FFI を一部使う必要があり。

**検証**:
- 1080p H.264 / HEVC / VP9 でデコード時間が 50% 以上短縮されること
- 4K HEVC で再生がスムーズになること
- HW 非対応コーデック (VC-1 等) で CPU フォールバックが効くこと

**制限**:
- v1 では HW デコード後に CPU readback (transfer_data) して swscale なので、純粋な「GPU メモリだけで完結」ではない。RTX VSR を入れる v1.1 で、D3D11 テクスチャを直接 wgpu に渡す経路に拡張する。

### C. シーク用プレビューサムネイル (#4)

**目的**: ユーザーがシークバーをホバーしたとき、その時刻のフレームを先回り表示する (動画プレイヤー一般機能)。

**実装方針**:

1. `VideoPlayer::open` で動画情報取得後、バックグラウンドスレッド (`video-preview`) を起動
2. duration を等分して N 個 (例: 60 個) のターゲット時刻を計算
3. 各時刻に `avformat_seek_file` → 1 フレームデコード → 160x90 (16:9) に縮小 → WebP エンコード → in-memory `Vec<(secs, ColorImage)>` に保存
4. 全部終わったら sidecar (`mimageviewer.dat`) に書き出し (次回起動で即時利用)
5. シークバー hover 時、最寄りのプレビューを HUD bar の上に小さく表示

**専用デコーダーで用意**: メイン再生用デコーダーをシークすると再生が止まるので、プレビュー用に **別の AVFormatContext** を開く (同じファイルを 2 回 open input)。

**サイズ目安**: 60 枚 × 160x90 WebP ≒ 60KB / 動画 1 本。サイドカーに格納 OK。

### D. 📌 代表サムネイル (#5)

**仕様**:
- フルスクリーン中に **`P` キー または HUD のピンボタン** で「現フレームをこの動画のグリッドサムネとして採用」
- 動画 1 本につき最大 1 枚 (上書き)
- グリッドの `video_thumb` (Windows Shell API) より優先される
- 解除は別途リセット UI (右クリックメニューに「サムネイルをリセット」)

**保存先**: `mimageviewer.dat` の動画セクション (`pinned_thumbnail: Option<WebP bytes>` per file path)

**実装**:
1. キー P 検出 → `player.texture()` から現在のフレームを取得 → WebP エンコード (q=85, 長辺 512px)
2. `mimageviewer.dat` の該当動画エントリに保存
3. グリッドの `thumb_loader::get_video_thumbnail` を改修: まずサイドカーに pinned があれば優先返却、無ければ Shell API へフォールバック
4. catalog (SQLite) のサムネキャッシュも `<path>::pinned` キーで持つ (再起動時の即表示用)

### E. 🔖 ブックマーク (#6)

**仕様**:
- フルスクリーン中に **`B` キー または HUD のブックマークボタン** で「現時刻にブックマーク追加」
- 1 動画につき **無制限**、UI スクロールで対応
- 各ブックマークに自動でその時刻のフレームのサムネ (160x90) を保存
- 右パネル (動画専用、画像の metadata パネルと別) に一覧表示
  - サムネ + 時刻 + 削除ボタン (✕)
  - クリックでその時刻にシーク
- シークバー上にブックマーク位置を縦線マーカーで表示 (色: 黄色など)

**保存先**: `mimageviewer.dat` の動画セクション (`bookmarks: Vec<{secs: f64, thumbnail: WebP bytes}>` per file path)

**右パネル設計**:
- 既存の adjustment / metadata 抑止パスの代わりに `draw_video_panel(ui, ctx, full_rect, fs_idx)` を呼ぶ
- 右側 320px 固定幅、上から: 動画情報 (codec / duration / dimensions) → ブックマーク一覧 (スクロール可能)
- 上部にバー: 「+ ブックマーク追加」ボタン

**実装**:
1. 新規ファイル `src/ui_video_panel.rs`
2. `mimageviewer.dat` フォーマット拡張 (既存の `sidecar.rs` を参考)
3. `VideoPlayer` に `bookmarks: Vec<Bookmark>` を保持、追加 / 削除 / シーク API
4. シークバー描画 (`draw_video_hud`) でブックマーク位置をマーカー表示

### F. 再生速度変更 (#7)

**仕様**:
- HUD で速度を 0.5x / 0.75x / 1.0x / 1.25x / 1.5x / 2.0x から選択
- 既定 1.0x、選択は永続化しない (動画ごとにリセット)

**実装方針** (音声側の検討が肝):

オプション 1: **音声を破棄して映像だけ早送り**
- 簡単。AvClock の進行レートを倍に → 早送り表示
- 音声は素直に出すと早すぎ + ピッチアップ

オプション 2: **swresample で音声をそのままレート変換 (ピッチも変わる)**
- swresample は時間ベースのリサンプリング。`set_compensation` で速度比を指定すると音声長が変わるがピッチも変わる
- 古いカセットレコーダーの早送り音と同等

オプション 3: **WSOLA / SOLA でピッチ保存タイムストレッチ**
- libsoundtouch などを使えばピッチを保ったまま速度変更
- C++ ライブラリのため Rust ラッパー (`soundtouch-sys`) が必要、実装コスト中

→ **v1 はオプション 2 (リサンプル変換、ピッチも変わる)** を採用。実装が軽く、よくある「早回し効果」として受け入れられる。ピッチ保存は v1.1 / v2 で検討。

実装:
1. AvClock に `playback_rate: AtomicU64` (f64 bits) フィールド追加
2. `now_secs()` の elapsed 計算で `elapsed * playback_rate` する
3. swresample の `set_compensation(input_count, output_count)` で速度比に応じた sample 数調整
4. 動画フレームの PTS 比較 (`pts_secs <= now + slack`) は速度変更後の now と比較するので変更不要 (now が早く進むから自動的に多くのフレームが drop される)

## 実装順序 (推奨)

各ステップでビルド + 動作確認しながら進める。

1. **#1 UI 改善 (音量スライダー / ミュートボタン / Play-Pause ボタン)** — 即見える変化、低リスク
2. **#7 再生速度ドロップダウン** — UI 連携、AvClock 改修。低-中リスク
3. **#2 シークバー操作精度** — ドラッグ中リアルタイムシーク、hover ツールチップ。低-中リスク
4. **#3 HEVC HW デコード (D3D11VA)** — 中-高リスク。CPU フォールバックを必ず保つ
5. **#4 プレビューサムネイル** — 中リスク。専用デコーダー、サイドカーキャッシュ
6. **#5 📌 代表サムネイル** — 低-中リスク
7. **#6 🔖 ブックマーク + 右パネル** — 中リスク。最後に来る理由は他の機能 (シークバー / サイドカー / 専用パネル) と統合度高いため

サイドカー (`mimageviewer.dat`) フォーマット拡張は #5 のタイミングで先行実装し、#6 で再利用する。

## 検証 (v1 リリース前の最終確認)

各ステップ完了後 + リリース前に:

- 各種コーデック動画で再生確認: H.264 / HEVC / AV1 / VP9 / MPEG-2 / WMV
- iPhone .mov (HEVC) で HW デコードが効くこと
- 4K HEVC で CPU 使用率が許容範囲 (HW デコード前後で比較)
- ←/→ シークが正確に 5 秒 (Shift で 30 秒) — 既存バグの修正再検証
- Space 再生/停止、↑↓ 音量、M ミュート、L ループ
- Shift+Enter で外部プレイヤー起動
- 📌 で代表サムネ更新 → グリッドに反映
- 🔖 を 5-10 個追加 → 右パネル一覧表示 + シークバーマーカー
- 速度 0.5x / 1.5x / 2.0x で再生 (ピッチ変化を許容)
- フォルダ移動でサイドカーが追従 (mimageviewer.dat ごと移動)
- ファイル個別移動 (フォルダ間) では失われる (現状の制限、v2 で再考)
- 依存 DLL 回帰チェック (`dumpbin /dependents` で `VCRUNTIME140.dll` が出ないこと)

## ドキュメント同時更新

各機能の完了時に以下も更新:

- `docs/architecture-overview.md` — 新モジュール (preview, video_panel) の追加
- `docs/async-architecture.md` — 新ワーカー (preview generator) の追加
- `docs/spec.md` — 動画機能の操作・対応コーデック・既知の制限
- `htdocs/mimageviewer/manual/fullscreen.html` (or 専用ページ) — 動画再生操作の追加
- `htdocs/mimageviewer/index.html` — 機能一覧に追加
- `README.md` — 機能リスト + リリース時の更新履歴
- CLAUDE.md の Tech Stack に HW decode の記述追加 (#3 完了時)

## 既知のバグ (v1 着手前に解消済み)

- ✅ 動画ダブルクリックが「play button mode」で `start_fs_load` を呼んでいなかった
- ✅ シーク量が 1/78 しか進まない (`input.seek` の単位が time_base ではなく AV_TIME_BASE)
- ✅ EOF でシークバーが右端まで進まずに止まる
- ✅ シーク後すぐにシークバーが戻る (post-seek serial で fill_output の clock 更新を抑止)
- ✅ スロー再生 + 低ピッチ (デバイスのサンプルレートとデコーダー出力レートが一致していなかった)
- ✅ Shift+Enter が効かない (修飾子マッチを緩めた)
