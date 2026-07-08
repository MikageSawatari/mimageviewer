# music_lab 実機検証チェックリスト

Status: Lab 検証用
Date: 2026-06-30

対象: `crates/music-core` + `tools/music_lab`。本体統合前に、ラボ単体で
「音楽 / 動画音声ビューとして致命的に体感が悪い箇所」と、mIV 本体へ移したときに
壊れやすい非同期境界を拾うための実機チェックリスト。

関連:

- [music-lab-plan.md](music-lab-plan.md)
- [async-architecture.md](async-architecture.md)
- [ui-responsiveness.md](ui-responsiveness.md)
- [video-architecture.md](video-architecture.md)
- [vst3-integration.md](vst3-integration.md)

## 0. 起動と前提

推奨は release build。debug build は解析と描画の体感が大きく悪化するため、性能評価には使わない。

```powershell
cargo run -p music_lab --release
cargo run -p music_lab --release -- "C:\path\to\sample.mp3"
```

確認対象は次を最低 1 本ずつ用意する。

- 3-5 分程度の通常楽曲
- 10 分超、または 30 分超の長尺音声
- 動画ファイル内の音声 (`mp4`, `mkv`, `webm` など)
- 無音に近いイントロ、急なサビ入り、低音が強い曲、男性 / 女性歌唱を含む曲

## 1. Open / D&D / 自動再生

### P0

- Open ダイアログで音声ファイルを開ける。
- ファイルをウィンドウへ D&D して開ける。
- 動画ファイルを D&D して音声だけ再生できる。
- Open / D&D 直後、全尺 timeline の完成を待たずに再生が始まる。
- 新しいファイルをロードしたとき、旧ファイルの loader / raster 結果が UI に戻らない。
- 壊れたファイルや未対応形式で panic せず、右パネルまたはステータスにエラーが出る。

### P1

- 連続で複数ファイルを D&D しても最後のファイルだけが残る。
- ロード中に Stop / Play / seek を押しても UI が固まらない。
- 日本語ファイル名、長いパス、OneDrive 配下のパスが文字化けしない。

## 2. 長尺ロードと音切れ

### P0

- 長尺音声でも UI が先に操作可能になる。
- ロード中に再生カーソルが動き、解析済み区間から timeline が埋まる。
- 右パネルの `Playback buffer` に `decoded`, `ahead`, `underruns` が表示される。
- 音切れが起きた場合、`underruns` が増える。
- ロード完了後、`buffer_ahead_secs` が十分に増え、再生が安定する。

### P1

- `%TEMP%\miv_music_lab_perf.log` に `decoded_secs` / `buffer_ahead_secs` /
  `underruns` が記録される。
- 長尺ロード中に Row 秒数を切り替えても、音切れ回数が極端に増えない。
- debug build で遅い場合も release build では実用速度になることを確認する。

## 3. Timeline 表示

### P0

- 5 / 10 / 15 / 30 / 60 / 120 / 300 / 600 秒 row を切り替えられる。
- Row 切替中、可視範囲と再生カーソル周辺が優先して再描画される。
- Row 10m などの大きな切替中でも、下段 spectrum analyzer が止まらない。
- 再生位置が画面外へ出ると自動スクロールする。
- 手動スクロール中は、自動追従で見ている位置を奪わない。
- 再生位置の行が上下に見切れた場合、行全体が見える位置へスクロールする。

### P1

- 波形、Loudness+Bass root、Key の 1 行単位が視認できる。
- 行の枠線 / 背景 / 文字色が黒テーマで読みやすい。
- Loudness+Bass root と Key が波形より明るく目立ちすぎない。
- ホバーでレーン名、値、時刻、推定音名が分かる。

## 4. Spectrum analyzer / piano

### P0

- 再生中に spectrum analyzer が滑らかに更新される。
- 低域の動きが過度に遅れず、一拍の上下が読める。
- 高域バーの上端だけが不自然に明るく残らない。
- 減衰背景が表示され、急にゼロへ落ちない。
- ホバーで Hz と近似音名が表示される。
- 鍵盤の実ピアノ範囲 A0-C8 が明るく、範囲外がグレーで表示される。
- スペクトルバーと鍵盤の水平位置が大きくずれない。

### P1

- 鍵盤ハイライトは全体が光りっぱなしにならず、相対的に強い音階が分かる。
- 打楽器的な広帯域トランジェントで鍵盤全体が過剰に反応しない。
- 20-60 Hz 付近の低域が視覚的に勝ちすぎない。

## 5. 解析メトリクス

### P0

- Loudness+Bass root は高さが音量、色が bass root として読める。
- Bass root は細かく点滅しすぎず、50-100ms 程度の表示用リズムグリッドで変化する。
- Key は短い瞬間ピークではなく、数秒窓の chroma と key profile で推定される。
- Key は低 confidence でも完全には消えず、淡い補助表示として残る。

### P1

- 転調に見える箇所で Key の色が大きく変わるかを確認する。
- ベースが抜ける、低音が入る、サビで密度が増えるなど、構成把握のヒントになるかを見る。
- 歌声 / Change / Drum 系の試作レーンは本体統合前提にはしない。

## 6. Perf / ログ

### P0

- Top bar に FPS / frame ms / UI ms / raster misses / spectrum 状態が表示される。
- Release build の通常再生で FPS が極端に落ちない。
- Row 切替や長尺ロード中に frame ms が大きく跳ねた場合、右パネルと perf log で原因候補を追える。

### P1

- `%TEMP%\miv_music_lab_perf.log` が肥大化しすぎない。
- 長時間放置後に resize / scroll しても極端に重くならない。
- 1 frame に大量の texture upload をしている兆候がない。

## 7. 本体統合前の判定ライン

本体統合へ進めてよい目安:

- release build で通常楽曲と長尺音声が再生可能。
- ロード中も UI が操作可能で、timeline が部分的に埋まる。
- Row 切替中も spectrum analyzer が固まらない。
- 音切れが発生しても `PlaybackSnapshot` で原因を追える。
- `TimelineAnalysis` の version / cache key / row texture の transient 境界が文書化されている。

本体統合時に移すもの:

- `music-core` の analysis 型と timeline analysis ロジック。
- `tools/music_lab` で固めた row texture / spectrum / hover 表示の UI 方針。
- `PlaybackSnapshot` 相当の buffer 計測。

本体統合時に置き換えるもの:

- lab の `cpal` 簡易プレイヤー。
- lab の streaming buffer。
- lab の Open / D&D の直接 decode 経路。
- 音量ノーマライズ、VST3、動画音声モードは mIV 既存 video audio engine 側へ接続する。
