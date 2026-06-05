# 音量ノーマライズ スキャン速度ベンチ

動画音声ノーマライズの初回スキャン待ち時間を、実フォルダの動画で測るための開発用メモ。

## 目的

HDD 上の長尺動画では、未測定動画を再生する前に全尺スキャンを待つ設計が体感待ち時間に
なりやすい。並列スキャンは SSD では効く可能性がある一方、HDD ではランダム I/O 化して
再生側 demux と競合する懸念がある。

`normalize_scan_bench` は既存の `scan_audio_loudness` をそのまま使い、ランダム抽出した
動画を逐次 / 並列で測る。アプリの再生 UI は起動せず、スキャン単体の実効速度を見る。

## 使い方

別 worktree で clean build する場合、`vendor/` は junction / symlink で共有しない。必要な
依存ファイルは setup script で作るか、実体ファイルをコピーする。

```powershell
# 例: FFmpeg だけを worktree 側に実体コピーする場合
New-Item -ItemType Directory -Force vendor | Out-Null
Copy-Item -Recurse -Force C:\home\mimageviewer\vendor\ffmpeg vendor\ffmpeg
```

既存の build artifacts を再利用してベンチだけ動かす場合は、`CARGO_TARGET_DIR` を本体側
`target` に向けると再ビルドを避けやすい。この場合も junction は作らない。

```powershell
$ffmpegBin = if (Test-Path vendor\ffmpeg\bin) {
  (Resolve-Path vendor\ffmpeg\bin).Path
} else {
  'C:\home\mimageviewer\vendor\ffmpeg\bin'
}
$env:PATH = $ffmpegBin + ';' + $env:PATH
$env:CARGO_TARGET_DIR = 'C:\home\mimageviewer\target' # 別 worktree で既存 build artifacts を再利用する場合
cargo run --release --bin normalize_scan_bench -- D:\home\18\dms2 --sample 12 --jobs 1 --csv bench-j1.csv
cargo run --release --bin normalize_scan_bench -- D:\home\18\dms2 --sample 12 --jobs 2 --seed 1 --csv bench-j2.csv
cargo run --release --bin normalize_scan_bench -- D:\home\18\dms2 --sample 12 --jobs 4 --seed 1 --csv bench-j4.csv
```

主なオプション:

- `--sample N`: 対象フォルダから reservoir sampling で N 本選ぶ。既定 8。
- `--jobs N`: 同時スキャン数。HDD では 1 / 2 / 4 を比較する。
- `--seed U64`: ランダム抽出 seed。同じファイル集合で jobs だけ変えると比較しやすい。
- `--exts mp4,mkv,mov`: 対象拡張子の上書き。
- `--csv out.csv`: 詳細結果を書き出す。

## 読み方

出力の `speed` は `動画の音声時間 / スキャン実時間`。150x なら 150 分ぶんを 1 分で測れる。
`batch wall` は選ばれた動画群を実際に掃き切るまでの壁時計時間、`batch speed` は動画群全体の
実効速度。

HDD 向け判断:

- `--jobs 2` で p50 / p90 が 1 job より大きく落ち、合計 wall 時間も縮まらないなら並列化は
  HDD で逆効果の可能性が高い。
- `--jobs 4` で各 worker の速度が大きく落ちるなら、ランダム I/O 競合が出ていると見る。
- 再生中バックグラウンドスキャンを入れるなら、jobs は 1 を基本にし、再生バッファが薄い間は
  scanner を一時停止する設計を優先する。

## 初回実測メモ (2026-06-05)

`D:\home\18\dms2` では 311 本の動画が見つかった。seed 固定で 6 本抽出した結果:

| jobs | batch wall | batch speed | per-file speed |
| --- | ---: | ---: | --- |
| 1 | 82.24s | 321.5x | 305.8x〜330.5x |
| 4 | 31.31s | 844.7x | 261.5x〜304.4x |

このサンプルでは 4 並列で 1 本あたりの速度は 6〜20% 程度落ちたが、全体の掃き切り時間は
短くなった。少なくともスキャン単体では「HDD だから即座に崩壊する」挙動ではない。
ただしこれは再生を同時に走らせた測定ではないため、再生中スキャンでは動画再生バッファの余裕を
見て throttle / pause できる設計にする。

## 設計メモ

「10 分ほどスキャンしたら仮 gain で再生開始し、スキャン継続後に確定 gain へ ramp する」
方向は、HDD では並列化より安全に体感待ちを下げやすい。

今回の実装では以下をセットにした:

- scanner から `Provisional` と `Done` の 2 段階メッセージを返す。
- 仮結果は DB に保存しない。確定結果だけ `audio_normalize.db` に保存する。
- `AvClock::normalize_gain` は目標値の atomic とし、audio pump 側で 4 秒の dB ramp を持つ。
- 仮 gain 適用後は `ProvisionalApplied` UI 状態にし、モーダル progress を閉じて再生を開始する。

残る検討:

- 継続スキャンは低優先度にし、再生バッファ不足 / seek / source switch / fullscreen close で
  一時停止またはキャンセルする adaptive throttle。
- idle 時や再生していない動画の事前測定では jobs 2〜4 も候補。ただし再生中は jobs 1 から始め、
  audio/video queue が薄い場合はスキャンを止める adaptive throttle を優先する。
