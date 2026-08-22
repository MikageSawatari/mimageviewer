# ブリーフ: シークサムネイル抽出の時間内訳を測る

対象: v3.1.1 時点の master。作業は**別 worktree**で行う。計測コードは一時的なもので、
残すかどうかは結果を見てから判断する。

## 1. 何のために測るか

利用者 (pattier) から「シークバーのサムネイルが出るまで少し待つ」「mpv のようにキビキビ
切り替わってほしい」という要望があり、**高速シーク (キーフレーム着地) モードを追加する価値が
あるか**を判断したい。実装する前に、いまの時間がどこで使われているかを確定させる。

**利用者の実測 (HDD / NVMe 差なし、「シーク中」表示が出ている時間)**:

| 素材 | 時間 |
| --- | --- |
| SD 640x480 h264 30fps 1.3GB 95分 | 0.5 秒 |
| HD 1280x720 h264 24fps 2.5GB 120分 | 1 秒 |
| FHD 1920x1080 h264 60fps 6.4GB 40分 | 1 秒 |
| 5760x2880 hevc 30fps 1.38GB 4分 | 1 秒 |

**解像度にほぼ比例していない**のが重要な手がかり。復号が支配的ならこの並びにはならないので、
高速シークにしても効かない可能性がある。だから先に測る。

## 2. 計測前の実装 (historical baseline)

すべて [src/video/thumbnail.rs](../src/video/thumbnail.rs)。

- ワーカーは**専用スレッド 1 本の最新勝ち**。`request()` が `pending_target_bits` を上書きして
  wake するだけ (152 行)。
- **ファイルは 1 回だけ開いて使い回す** (`if decoder.is_none()`、482 行付近)。オープンのコストは
  初回だけで、毎回の要求には乗らない。
- 抽出は**精密**。`av_seek_frame + AVSEEK_FLAG_BACKWARD` で手前のキーフレームへ飛び、
  **target まで復号し続ける。復号数に固定上限は置かない** (305 行、320 行のコメント)。
- 中断は 2 つ: `cancel`、および**別バケットの新要求**による supersede (同バケットの再要求は
  supersede にしない)。
- `nearest()` は **±1〜2 バケット**内の既存サムネを返す (158 行)。利用者の「キャッシュが利くのか
  パッと出ることもある」はこれ。
- 呼び出し側は `pump_native_hover_thumbnail` ([src/video/mod.rs:8097](../src/video/mod.rs)) で、
  `nearest()` が無ければ `request_seek_thumbnail` を出す。デバウンスやスロットルは無い。

## 3. 測る値

`decode_t0` (493 行付近) の周辺に計装を足す。既存の `crate::logger::log` でもよいが、
**`crate::perf::event("video_thumb", ...)` にすると `scripts/analyze_perf.py` と同じ土俵で
集計できる**ので、そちらを推奨。

1 要求あたり次を記録する。

- `seek_ms` — `av_seek_frame` 単体の時間
- `frames_to_target` — 着地キーフレームから target に到達するまでに**復号したフレーム数**
- `decode_ms` — 復号ループ全体の時間
- `scale_ms` — RGBA 変換 / リサイズの時間 (別枠にする)
- `total_ms` — 要求受理から結果格納まで
- `hw` — HW 復号が効いていたか (`hw_decode_active`)
- `outcome` — `ready` / `superseded` / `no_frame`
- `target_secs` / `bucket` / `cache_hit`
- 可能なら `keyframe_secs` (着地したキーフレームの pts) — `target_secs - keyframe_secs` が
  実効 GOP 長になり、高速シークにしたときの**時刻ずれの見積もり**にもなる

## 4. 判断基準 (測る前に決めておく)

- `frames_to_target` が大きく `decode_ms` が `total_ms` の大半 → **高速シークが効く**。
  設定として入れる価値がある。ずれの大きさは `target_secs - keyframe_secs` で提示できる。
- `frames_to_target` が小さいのに `total_ms` が大きい → 原因は I/O か 1 要求あたりの固定費。
  **高速シークを入れても体感は変わらない**。別の対処 (先読み、バケット粒度、要求の間引き) を考える。
- `superseded` が多い → スクラブ中に捨てている仕事が多い。バケット粒度や要求頻度の問題。

## 5. 手順

```powershell
# 1. worktree を作る (junction は禁止。vendor は実体コピー)
git worktree add C:\home\mimageviewer-seek-bench -b seek-thumb-bench

# 2. ビルドに要る vendor だけ実体コピー (各 target/ は除く。合計 ~325MB)
#    eframe / egui-wgpu は Cargo.toml の [patch.crates-io] が相対パスで参照するので必須
$src = "C:\home\mimageviewer\vendor"; $dst = "C:\home\mimageviewer-seek-bench\vendor"
New-Item -ItemType Directory -Force $dst | Out-Null
foreach ($n in @("eframe","egui-wgpu","ffmpeg","pdfium","ort","susie-worker","vst3-host","models","twemoji")) {
  Copy-Item -Recurse -Force "$src\$n" "$dst\$n" -Exclude "target"
}

# 3. 計装を入れてビルド (初回はフルビルドになる)
.\scripts\build-dev.ps1

# 4. 利用者が実機で 4 素材を測る
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList "--perf-log"
```

⚠ `Copy-Item -Exclude "target"` は再帰コピーでは効かないことがあるので、`eframe` /
`egui-wgpu` は `robocopy /XD target` を使うか、コピー後に `target` を消す。

## 6. 出口

- 結果を [next-release-backlog.md](next-release-backlog.md) の該当項目へ書き戻す。
- 高速シークを入れないと決めた場合も、**測った数字を残す** (次に同じ相談が来たときの根拠になる)。
- 計測コードを master へ入れるかは、`perf::event` として恒久的に有用かで判断する。

## 7. 計測結果と実装結果 (2026-08-20〜22)

4K AV1 60fps・約108分・GOP約7秒で42回測定し、hover 待ちは p50 177ms / p90 575ms /
最大1007msだった。67件の ready 要求では `av_seek_frame` が p50 0.2ms、queue が1〜2ms、
decoder open は初回62msだけだった一方、keyframe から target までの decode は p50 53ms /
p90 374ms / 最大923ms、21 / 212 / 343 frames だった。支配項は GOP 内 decode であり、解像度や
storage 速度は一次要因ではない。

実装は「シーク時のズレ許容 (秒)」1設定に決定した。既定1.0、範囲0.0〜30.0で、0.0は従来の
精密動作。実測素材では1.0秒は元から速かった64%だけを短縮し、遅い尾を一件も短縮しないため、
既定値は報告症状を解消しない。従来の表示が最大1秒ずれ得た挙動へ寄せるための意図的な既定で、
速さを求める利用者は値をGOP長まで上げる。7秒以上なら全要求で着地keyframeを採用でき、試算は
p50 41ms / p90 73ms / 最大83ms、最大ずれ5.69秒だった。

現行実装の正本は [video-architecture.md](video-architecture.md) の `ThumbnailWorker` 節。
実PTS `BTreeMap`、要求ごとの許容、desktop / marker / Remote の分離、backward seek着地後の
採用判定、1要求1枚の固定をそこで定める。この文書の§2〜6は計測時点のbaselineとして残す。
