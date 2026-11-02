# ローカル AI 説明文 V0 手順書 (モデル比較と VRAM 計測)

Status: 手順の下書き (2026-11-02、設計担当 ClaudeCode)。未実行。
親: [local-vlm-caption-plan.md](local-vlm-caption-plan.md) §3.3・§11・§14。

## 0. 目的

V0 の結果で、次の 4 つを決める。

1. **4B 級に固定するか、4B 級と 9B 級の 2 段にするか** (計画書 §3.3)。品質の判断は利用者。
2. **使う系統** (Qwen3.5 か Gemma 4)。
3. **U1** (作成ジョブと AI 補正の関係、計画書 §11.2)。VLM・AI 補正・表示用テクスチャの VRAM を
   同じ物差しで実測し、§11.1 の推定を置き換える。
4. 画像の縮小サイズと、画像に使うトークン数の上限。

製品のバイナリは使わない。llama.cpp 公式の配布物と、開発用の `bench_ai` だけを使う。

## 1. 置き場所と扱い

| もの | 場所 | 扱い |
| --- | --- | --- |
| 比較用の画像 (利用者が用意、26 枚) | `C:\home\mimageviewer_local\books\LVM比較` | **読み取りのみ**。移動・改名・書き込みをしない |
| llama.cpp の配布物 | `G:\home\vlm-eval\bin\<版>-<vulkan\|cuda>\` | zip ごとに新しい空のフォルダへ展開する |
| モデル (GGUF) | `G:\home\vlm-eval\models\<配布元>\` | 配布元ごとに新しい空のフォルダ |
| 縮小した入力画像 | `G:\home\vlm-eval\inputs\<長辺px>\` | 比較用画像から作る一時ファイル。V0 の終了時に削除する |
| 実行結果 | `G:\home\vlm-eval\runs\<日付>-<条件>\` | 説明文を含むので**リポジトリに入れない・Artifact に公開しない** |
| 計測・比較用のスクリプト | `scripts/vlm-eval/` (リポジトリ内、`mcp-trial`) | ダウンロード物とは別の場所に置く。`.ps1` は ASCII のみ |

- 説明文は利用者の画像の内容を表すので、計画書へは**集計値と判断だけ**を書き、説明文そのものは書かない。
- 比較用画像の内訳 (ファイル名からの推定。違っていれば利用者が直す):
  写真 = 0001〜0003・0009〜0020 (15 枚)、AI 生成画像 = 0004〜0008 (5 枚)、本の表紙・ページ = 0021〜0026 (6 枚)。
  形式は JPG 15 枚・PNG 11 枚、合計 225MB。

## 2. ダウンロードするもの (実行前に利用者の承認を得る)

2026-11-02 に Hugging Face API・GitHub API で確認した名前とサイズ。実行時に固定する版・コミット・sha256 を
`runs\` の記録と計画書に残す。

| # | ファイル | 配布元 | サイズ |
| --- | --- | --- | --- |
| D1 | `llama-<版>-bin-win-vulkan-x64.zip` | [ggml-org/llama.cpp Releases](https://github.com/ggml-org/llama.cpp/releases) (2026-11-01 の b12032 で 36MiB) | 約 36MiB |
| D2 | `llama-<版>-bin-win-cuda-13.4-x64.zip` + `cudart-llama-bin-win-cuda-13.4-x64.zip` | 同上 | 158MiB + 404MiB |
| D3 | `Qwen3.5-4B-Q4_K_M.gguf` + `mmproj-F16.gguf` | [unsloth/Qwen3.5-4B-GGUF](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) | 2.55GiB + 0.63GiB |
| D4 | `Qwen3.5-4B-Q8_0.gguf` (量子化の影響を見る、任意) | 同上 | 4.17GiB |
| D5 | `Qwen3.5-9B-Q4_K_M.gguf` + `mmproj-F16.gguf` | [unsloth/Qwen3.5-9B-GGUF](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF) | 5.29GiB + 0.86GiB |
| D6 | `gemma-4-E4B-it-Q4_0.gguf` + `mmproj-gemma-4-E4B-it-Q8_0.gguf` | [ggml-org/gemma-4-E4B-it-GGUF](https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF) | 4.28GiB + 0.52GiB |

- 合計は約 19GiB (D4 を除くと約 15GiB)。G: の空きは 220GB (2026-11-02)。
- Qwen3.5 は提供元 (Qwen) の公式 GGUF が見当たらず (`Qwen/Qwen3.5-4B-GGUF` は存在しない)、量子化は第三者 (unsloth) のもの。
  V0 の比較には使うが、製品の配布元は別に決める (計画書 §16)。
- llama.cpp の Releases は数時間ごとに新しい版が出る。V0 の間は 1 つの版に固定する。
- GitHub の配布物は API の `digest` (sha256) と照合する。GGUF は Hugging Face の LFS の sha256 と照合する。

## 3. 段取り

### V0-1 準備 (ClaudeCode)

1. 利用者の承認後、§2 をダウンロードし、sha256 を照合して記録する。
2. 入力画像を作る: `scripts/vlm-eval/resize-inputs.ps1` (新規、ASCII) が System.Drawing で
   長辺 1024px と 768px の 2 種類を作る。EXIF の向き (Orientation) を適用してから縮小する
   (System.Drawing は自動では回さない)。PNG の透明部分は白で埋める。
   mIV 本体のデコーダとは結果が少し違うことを記録に残す (V1 では本体のデコーダを使う)。
3. 計測用スクリプトを用意する:
   - `scripts/vlm-eval/gpu-sample.ps1` (新規、ASCII): 指定した PID の
     `\GPU Process Memory(pid_<PID>_*)\Dedicated Usage` と、全体の `\GPU Adapter Memory(*)\Dedicated Usage` を
     0.5 秒ごとに CSV へ書く。GPU のメーカーを問わず同じ物差しで測れる。
   - `scripts/vlm-eval/run_eval.py` (新規): 入力画像を 1 枚ずつ llama-server へ送り、結果を JSONL に書く。
     Python は `-I` で起動する。Pillow は使わない (未導入のため)。
   - `scripts/vlm-eval/make_compare.py` (新規): 比較用の HTML を `runs\` の中に作る (§V0-5)。
4. 固定した版の `llama-server --help` で、次の指定方法を確かめて記録する:
   画像用の部品 (`--mmproj`)、GPU への全層の配置、文脈長、画像トークン数の上限
   (`--image-min-tokens` / `--image-max-tokens`)、Web UI の無効化、待ち受けを 127.0.0.1 に限ること、
   Qwen3.5 の「考える出力 (thinking)」を無効にする方法。

### V0-2 VLM の計測 (ClaudeCode)

条件ごとに llama-server を 1 つ起動し、26 枚を順に処理して止める。待ち受けは 127.0.0.1 だけにし、実行中だけ開く。

| 条件 | モデル | GPU | 入力 |
| --- | --- | --- | --- |
| C1 | Qwen3.5-4B Q4_K_M | Vulkan | 1024px |
| C2 | Qwen3.5-9B Q4_K_M | Vulkan | 1024px |
| C3 | Gemma 4 E4B Q4_0 | Vulkan | 1024px |
| C4 | Qwen3.5-4B Q8_0 (任意) | Vulkan | 1024px |
| C5 | C1〜C3 のうち品質のよい 4B 級 | Vulkan | 768px |
| C6 | C1 と C2 | CUDA | 1024px |

- 要求: OpenAI 互換の `/v1/chat/completions`。画像は base64 の `image_url`。出力は `response_format` の
  `json_schema` で `{"caption": string, "keywords": string[]}` に固定する (画像と json_schema を同時に使えるかは
  V0-1 で確かめる。使えなければ grammar で代える)。温度 0、出力上限 400 トークン、文脈 8192。
- プロンプト (版 v0-1、全条件で同じ文):

  ```text
  この画像に写っているものを日本語で説明してください。
  caption: 1〜3 文、200 文字以内。写っていないことは書かない。
  keywords: 画像を検索するときに使える日本語の語を 5〜15 個。
  ```

  内容の観点を限定する指示は入れない (計画書 §6.3、内容に関与しない方針)。
- 記録する (1 枚ごと): 所要時間 (応答までの実時間と、応答の `timings` 欄の画像読み込み・生成の内訳)、
  入力トークン数 (画像分を含む)、出力トークン数、JSON として正しいか、説明文の文字数。
  最初の 1 枚はモデルの準備を含むので、時間の集計から外す。
- 記録する (条件ごと): `gpu-sample.ps1` の CSV から、読み込み前の基準値・読み込み後・処理中のピーク。
  llama-server のログから、重み・文脈 (KV)・計算用・画像用部品の各 buffer のサイズ。
  これらの内訳は GPU の容量に依存しにくいので、8GB 機での必要量の見積もりに使う。
- 決定性の確認: C1 だけ 2 回走らせ、同じ出力になるかを見る。

### V0-3 AI 補正の計測 (利用者の承認後に ClaudeCode、または利用者)

- 開発用の `bench_ai` (`cargo run --release --features dev-tools --bin bench_ai -- ...`) を使い、
  `gpu-sample.ps1` でその PID を測る。
- **注意**: `bench_ai` は製品と同じ `%APPDATA%\mimageviewer\` にモデルを展開し、ログを書く (コードで確認:
  `model_manager::ensure_models_extracted`、ロガーの初期化)。TensorRT は同じ場所の pack を使う。
  設定 DB を開くかは実行前にコードで確かめる。実利用中のフォルダに触れるので、**実行には利用者の承認が要る**。
- 測るもの: `vendor/models/` の各モデル × DirectML / TensorRT のピーク。`--models a,b` で
  アップスケールとデノイズを 1 プロセスで続けて実行し、2 つ目の後に 1 つ目のモデルの分が残るかを見る。
  入力は比較用画像のうち大きめのもの (写真 1 枚と AI 生成画像 1 枚)。

### V0-4 表示用テクスチャの使用量 (一部は計測済み)

既存の perf ログ (`%APPDATA%\mimageviewer\logs\perf_events*.jsonl` の `gpu` / `vram_accounting` event、1 秒ごと) を
2026-11-02 に読み取りで集計した。観測者: この PC (RTX 4090、pool の上限 12,069MiB = 既定 50%) での利用者のセッションの perf ログ。
どの操作をしたセッションかは不明。

| ログ | 一覧 中央値 / p95 / 最大 | フルスクリーン 中央値 / p95 / 最大 |
| --- | --- | --- |
| perf_events.jsonl | 39 / 180 / 283MiB | 1,003 / 2,474 / 2,792MiB |
| perf_events.1.jsonl | 28 / 42 / 80MiB | 795 / 2,030 / 2,295MiB |
| perf_events.2.jsonl | 77 / 77 / 77MiB | 63 / 63 / 63MiB |
| perf_events.3.jsonl | 65 / 75 / 87MiB | 475 / 1,197 / 1,900MiB |
| perf_events.4.jsonl | 192 / 213 / 243MiB | 360 / 3,195 / 4,800MiB |

- 数えているのは egui のテクスチャだけで、wgpu の中間 buffer や swapchain は入らない。
- 8GB 機では pool の上限が 4GB になり、それを超える分は退去するので、上の最大値はそのままは当てはまらない。
- 追加で必要なら、利用者が普段の閲覧中にタスクマネージャーの「詳細」タブで `mimageviewer-core.exe` の
  「専用 GPU メモリ」を見る (テクスチャ以外も含む実際の値)。観測者は利用者。

### V0-5 品質の比較 (利用者)

- `make_compare.py` が `runs\<日付>-compare\compare.html` を作る。1 画像ごとに、縮小画像と各条件の説明文・検索語を
  横に並べる。**目隠し**の表示 (条件名を A / B / C にして画像ごとに並びを入れ替え、対応表は別ファイル) も選べる。
- 利用者は画像ごとに「4B 級で十分 / 9B 級が明らかに良い / どれも不可」と、気づいた点を付ける。
  HTML の「結果をコピー」で JSON をクリップボードへ写し、会話に貼る。
- このページはローカルで開くだけにし、Artifact などで公開しない (利用者の画像を含むため)。
- 日本語の自然さ、誤り (写っていないものを書く・文字の読み違い)、本の表紙の書名の読み取り、
  AI 生成画像の画風や人物の説明の具体さを見る。

### V0-6 併用の確認 (任意、利用者と ClaudeCode)

- ClaudeCode が C1 (または採用候補) で 26 枚を繰り返し処理している間に、利用者が mIV を普段どおり使い、
  AI アップスケールを含むフルスクリーン表示でカクつきが出るかを見る。`gpu-sample.ps1` が全体の使用量を記録する。
- 観測者は利用者 (操作感) と ClaudeCode (使用量の記録)。開発機は 24GB なので、8GB 機での併用の可否は
  ここでは確かめられない。V0-2 と V0-3 の内訳の合計から判断する。

## 4. 判断の仕方

- **4B 級に固定するか** (計画書 §3.3): V0-5 の結果を利用者が見て決める。目安は「9B 級が明らかに良い」が少数で、
  4B 級に「どれも不可」が目立たないこと。
- **系統**: 4B 級どうし (C1 と C3) の品質、JSON の正しさ、日本語、VRAM のピークで選ぶ。
- **U1**: 次の合計が、8GB 機で OS と画面表示の分 (1GB 前後と仮定) を除いた残りに収まるかを見る。
  採用候補の VLM のピーク (V0-2) + AI 補正のピーク (V0-3) + フルスクリーンのテクスチャの典型値 (V0-4)。
  収まれば、計画書 §11.2 (d) (同時に動かし、足りない機種では使えない) を推奨する。
  収まらなければ、8GB 機を対象外にするか、別の案を利用者と相談する。
- **縮小サイズとトークン数**: 768px で品質が落ちなければ 768px、落ちれば 1024px。

## 5. 記録

- 計画書 (local-vlm-caption-plan.md) に「V0 の結果」の節を足し、版・コミット・sha256、条件ごとの集計
  (時間・トークン・JSON の正しさ・VRAM の内訳とピーク)、利用者の判断、決めたことを書く。各数値に観測者を書く。
- 説明文そのもの・画像の名前と内容の対応は書かない。
- V0 の終了時に `G:\home\vlm-eval\inputs\` を削除する。モデルと結果を残すかは利用者に確かめる。

## 6. 出どころ

- ファイル名とサイズ: Hugging Face API (`/api/models/<repo>/tree/main`) と GitHub API (`/repos/ggml-org/llama.cpp/releases`)、2026-11-02 取得。
- llama.cpp の画像入力・トークン数の指定: [docs/multimodal.md](https://github.com/ggml-org/llama.cpp/blob/master/docs/multimodal.md)
  と検索結果の要約。固定した版の `--help` で確かめるまでは未確認として扱う。
- `bench_ai` の書き込み先: `src/bin/bench_ai.rs`、`src/ai/model_manager.rs`。
- テクスチャの使用量: `src/app/vram_accounting.rs` の `vram_accounting` event と、上記の perf ログ。
