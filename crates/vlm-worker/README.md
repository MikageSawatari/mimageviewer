# ローカル AI 説明文 V1 worker

Windows x64 の worker と Rust client。GPU 判定・モデル導入の設定画面へ接続済み。
画像から説明文を作成して DB へ保存するジョブは後続段階で接続する。
製品バイナリ、通常の設定 DB、HTTP server を起動しない。
`vendor/llama.cpp` は変更しない。画像は元の場所から読み、JSONL はリポジトリ外の runs に保存する。
画像・説明文の内容をレビュー文書やリポジトリへ転載しない。

## 前提とビルド

- Visual Studio 18 2026 BuildTools、MSVC C++、Windows SDK、CMake 3.24 以上。
- Vulkan SDK `C:\VulkanSDK\1.4.363.0`。未設定なら CMake がこの場所を使う。
- llama.cpp **b12032**, commit `dde879bc4f954b809892fd0d660515f42536394a` を `vendor/llama.cpp/` に配置済みであること。
  提供されたアーカイブの SHA-256 は `2fbadc9cc8552352efbdf3cf5bc7b8c9cce84a12c876cca03c858cb062567e2a`。
  ビルドに取得処理はない。別バージョンへの互換性は保証しない。

リポジトリルートの PowerShell から実行する。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CMAKE_BUILD_PARALLEL_LEVEL = '1'
cmake -S crates/vlm-worker -B crates/vlm-worker/build -G 'Visual Studio 18 2026' -A x64 -DVLM_REFERENCE_TOOLS=OFF
cmake --build crates/vlm-worker/build --config Release --target mimageviewer-vlm vlm-contract-tests vlm-greedy-tests vlm-image-attempt-tests vlm-output-tests vlm-image-input-tests vlm-protocol-tests -- /m:1 /nr:false
ctest --test-dir crates/vlm-worker/build -C Release --output-on-failure
```

出力は `vendor/vlm-worker/mimageviewer-vlm.exe`。MSBuild の並列ノード起動が sandbox で不安定だったため `/m:1` を使う。
`GGML_VULKAN=ON`, `GGML_NATIVE=OFF`, `GGML_RPC=OFF`, `BUILD_SHARED_LIBS=OFF`, `GGML_BACKEND_DL=OFF`。
`LLAMA_OPENSSL`, `LLAMA_BUILD_SERVER/TOOLS/EXAMPLES/TESTS/APP/COMMON`, `LLAMA_SUBPROCESS`, `MTMD_VIDEO` は OFF。
b12032 の `LLAMA_BUILD_MTMD=ON` が `tools/mtmd` のライブラリだけを追加するので、tools 一式を有効にする必要はない。
worker の推論ライブラリは `ggml`, `llama`, `mtmd`。別途 Windows の `dxgi` と Vulkan loader を使う。
`nlohmann/json.hpp` は同じ vendor のヘッダだけを使う。

llama/ggml/mtmd と MSVC CRT は静的リンクする。OpenMP も無効化して runtime DLL 依存を避ける。
CPU backend は SSE4.2/BMI2/AVX/AVX2/FMA/F16C などの追加命令を無効にし、x64 の基準に揃える。
Vulkan loader と GPU driver は OS/driver 側の前提で、worker の配布物にはコピーしない。
開発用 reference exe は worker の配布ディレクトリへ出さない。

## 確認用モード

```powershell
$modelDir = 'G:\home\vlm-eval\models\unsloth__Qwen3.5-4B-GGUF'
$run = 'G:\home\vlm-eval\runs\20261102-V1-0-review-fixes'
New-Item -ItemType Directory -Force $run | Out-Null
.\vendor\vlm-worker\mimageviewer-vlm.exe --list-devices
.\vendor\vlm-worker\mimageviewer-vlm.exe --selftest --model "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" --mmproj "$modelDir\mmproj-F16.gguf" --images 'G:\home\vlm-eval\inputs\1024' --out "$run\forward-32.jsonl" --order forward
.\vendor\vlm-worker\mimageviewer-vlm.exe --selftest --model "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" --mmproj "$modelDir\mmproj-F16.gguf" --images 'G:\home\vlm-eval\inputs\1024' --out "$run\reverse-32.jsonl" --order reverse
```

`--out` はモデル読込前に `CreateFileW(CREATE_NEW)` で排他新規作成し、既存ファイルは上書きしない。
読み取り共有だけを許可する。起動後にモデル読込等で失敗した場合も出力は残すので、再実行には別の出力名を使う。
引数は Unicode の Windows command line を受ける。
PNG のファイル名でソートし、reverse はその逆順。画像をコピーしない。入力は縮小済みの PNG（各軸1〜1024px）だけを受ける。
IHDR の寸法をデコード前に検査し、検査した同一の byte buffer を mtmd helper/STB でデコードする。
入力ファイル全体（付加チャンク・末尾データを含む）は **16 MiB**まで。
1024×1024・RGBA・16bit の非圧縮画素は約8 MiBなので、その約2倍を固定上限とし、通常のPNGと一定量の付加情報を収める。
この確認用モードで大きすぎる付加情報まで読み込まないための上限で、超過はその画像だけ `ImageFailure` にする。
64 KiB単位で読み、上限判定用の1 byteを含めても最大16 MiB＋1 byteしか読まない。ファイルサイズの事前確認やseekには依存しない。
保持する入力は最大16 MiBであり、`kMaxPngBytes <= INT_MAX` をstatic_assertし、STBの `int` 長へ必ず収まる。

モデルと mmproj はプロセスで一度だけ読み込む。各画像で llama context（memory・sequence）と grammar sampler を新規作成し、
位置 0、sequence 0 から開始して、その画像の最後に破棄する。mtmd の画像チャンク・bitmap も各件で破棄する。
処理は直列なので supersession、rollback、並行ジョブの状態は作らない。
`--selftest` は従来と同じ mtmd helper/STB で PNG を RGB へデコードしてから、`--serve` と共通の `caption_rgb` で推論する。

固定条件は greedy、context 8192、生成上限 800、batch 2048、ubatch 512、flash attention AUTO、CPU threads 24（C9 と同じ）。
greedy 最大候補を先に文法で検査し、不許可のときだけ全語彙を mask する。常に全語彙を mask する場合と選択結果は同じ。
`prompt.h` のプロンプトは `run_eval.py` の v0-3、会話は Qwen3.5 の user 1件・tools/system なし・`enable_thinking=false` 専用。
任意モデルの chat template には対応しない。

JSONL は `image`, `caption`, `text`, `keywords`, `content`, `finish_reason`, `usage`, `generated_tokens`, `wall_s` と
`model_load_s`, `validation`, `status`, `prompt_chunks`, `device` を含む。stdout は番号・時間・成否だけで、説明文を表示しない。
`wall_s` は画像デコードから context/sampler の解放まで。モデル読み込み時間はモデル＋mmproj＋vision warmup を含み、context 作成は各画像に含む。
成功には EOG、完全な JSON、3欄の型、caption/text 各400 Unicode文字、検索語20個・各32文字、800 tokens以内をすべて要求する。
失敗行を含む完走は exit 2、全体を止める失敗は exit 1、全件成功は exit 0。
parse の前に buffer 全体の生の NUL と不正な UTF-8 を拒否する。JSON の `\u0000` エスケープは受理する。

| 失敗 | 処理 |
| --- | --- |
| PNG読取・ヘッダ・寸法・デコードの拒否、画像前処理/tokenizeの失敗、文脈予算超過 | その画像を `status=failed`, `finish_reason=error` の JSONL に記録し、次へ進む |
| 画像prompt/生成評価の非致命的エラー（戻り値 -1 または正値） | 同じくその画像だけ失敗。context/samplerを破棄して次へ進む |
| 完成した出力のJSON・型・長さ・終了条件の不合格 | `validation` と失敗を記録して次へ進む |
| モデル/mmproj読込、context/sampler/batch作成・設定、割当失敗/VRAM不足、device喪失、評価戻り値 `< -1` | 全体を止める。自動再試行はしない |
| 出力作成・書込の失敗、予期しない例外 | 全体を止める |

llama の評価は公開された戻り値（`< -1` は致命的）を優先して判定し、そのログは分類に使わない。
b12032 の mtmd は `bad_alloc` 等も catch して画像の前処理/encode失敗コードへまとめるため、その範囲だけログを補完に使う。
登録先をmtmd/clipのcallbackに限定し、`ERROR` の固定診断だけを照合する（数値が大きい `CONT` はERROR扱いしない）。
具体的には `clip_encode` 等の割当失敗全文、clip graph computeの致命的status全文、
`mtmd_tokenize` / `mtmd_encode_chunk` の例外報告形式における `bad allocation` / `std::bad_alloc` の完全一致、
Vulkan-Hppの `vk::<型名>::<API名>`＋定義済みのメモリ不足/device喪失コード、
およびggml-vulkanが直接投げる2種類の固定メモリ不足診断を認識する。
型名とAPI名はASCIIのC++識別子として検査し、型名の列挙はしない。空の要素・数字始まり・余分な修飾・パス文字は拒否する。
パス・メタデータの部分文字列は検索しない。診断はstderrへそのまま転送する。
この補完は固定版の発生元・形式に依存するので、llama.cpp 更新時は分類テストも確認する。

検索語は schema・埋め込み GBNF・検証器のすべてで **32文字**に揃えた。
元の `run_eval.py` の v0-3l（40文字）は過去比較用として変更せず、worker の `prompt.h` の `kSchema` を32文字にする。
JSONL の `prompt_version=v0-3l` は元の評価条件名を引き継ぐが、現在の worker の schema 上限は32文字である。

## 常駐用の通信 (`--serve`、protocol version 2)

stdin/stdout の匿名パイプを CRT の `_O_BINARY` にし、stdout は枠の出力だけに使う。
llama/ggml/mtmd の callback は stderr へ向ける。ネットワーク、共有メモリ、worker 側の画像デコードは使わない。
親が worker の Job Object と期限を管理する。worker は直列で読み、1要求ずつ答える。

枠は `u32 LE body_length` + `u32 LE json_length` + UTF-8 JSON + 残りのバイト列。
`body_length` は内側の長さ4バイトも含み、4〜16 MiB。JSON は object で、生NUL、不正UTF-8、末尾の余分な内容を拒否する。
`caption` の残りだけが RGB8 で、各行を詰めた `width * height * 3` バイト。他の種類には残りのバイトを許可しない。
長さ、JSON、種類、必須欄、順序、寸法、RGB量の不正は stderr に出し exit 1。やりとりを続けない。
外側の長さを読む前の EOF は正常終了、途中の EOF は不正な枠として終了する。

| 要求のJSON | 応答のJSON |
| --- | --- |
| `{"type":"hello"}` | `type="hello"`, `protocol_version=2`, `prompt_version=1`, `source_identity` (64桁のSHA-256) |
| `{"type":"load","model_path":"...","mmproj_path":"...","device_luid":"HIGH:LOW","cpu_threads":24}` | `type="load"`, `status="ok"`, `model_load_s`, `device`。読込失敗は `status="fatal"`, `error` を返して終了 |
| `{"type":"caption","request_id":1,"width":1024,"height":768}` + RGB | `type="caption"`, 同じ `request_id`, `status="ok"` / `"image_failure"` / `"fatal"`, `wall_s`。成功時は `caption`, `text`, `keywords`, `content`, `finish_reason`, `validation`, `generated_tokens`, `usage`, `timings`, `prompt_chunks` |
| `{"type":"shutdown"}` | 応答せず終了 |

`hello` は最初の1回、`load` はその後1回だけ。以降 `caption` を繰り返す。`shutdown` と EOF はロード前も使える。
`request_id` は0〜u64最大、寸法は各軸1〜1024、CPU threadsは1〜1024の整数。パスは空文字とNULを拒否。
LUID は各8桁16進数を `:` で結び、空なら既定GPUを選ぶ。load後にdevice/モデルを変更しない。
`device` は従来の selftest と同じDXGI/Vulkan/ggml対応情報を含む。

`validation` のbool欄は `encoding_valid`, `ended_on_eog`, `json_complete`, `field_types`, `limits`, `token_limit`, `ok`。
画像単位の例外では `error`, `finish_reason="error"`, すべて不合格の `validation`, `json_ok=false` を返し、次へ進む。
完成した出力が検証で不合格でも `image_failure`。致命的例外では `error` を返して終了する。
推論・検証・失敗の境界は selftest と共通で、自動再試行や自動再起動はしない。
`prompt_chunks` の画像idは selftest ではPNGのSHA-256、serveでは空。推論結果には使わない。

source identity は `CMakeLists.txt`, `src/*.{cpp,h}`, `tests/*.cpp`, `tools/source-identity.ps1` が対象。
CRLFだけをバイト単位でLFにし、各SHA-256をordinal順のslash相対名 `name:hash\n` に連結してさらにSHA-256化する。
ビルド時に同じPowerShellスクリプトで再計算する。vendorコードや生成物は対象外。
`MIV_VLM_SOURCE_SHA256:<hash>` を PE に 1 つ焼き込む。配布時の source identity gate は
`scripts/vlm-worker-identity.ps1` が worker を実行せずに現ソースと照合する。
release Cargo build・通常版の署名と内包前・portable のコピー前に検査し、再利用でも省略しない。
製品の `prompt_version` は `ai_caption.db` と同じ整数 (現在 1)。selftest JSONL の
`v0-3l` は従来の比較条件名として残し、製品の版とは区別する。

CTestの `vlm-protocol` はバイナリ画素（NUL/CR/LF/0x1Aを含む）、u64最大、16MiB境界、途中EOF、
内外長さ、JSON/UTF-8、必須欄、RGB寸法/量、要求順序を非モデルテストで確認する。

### Rust client と確認

`src/ai/vlm_worker.rs` は AI worker の transport として TRT client の隣に置く。
`job_process` は AI に依存しない Windows 起動 helper で、EPUB と共有する。
PDFium の外側の枠と同じ u32 LE を使うが、512MiB 上限の PDF 私有関数は変更せず、
VLM の16MiB上限と JSON/RGB の内側の検査は client 内で所有する。

V1-3 の持ち主は worker スレッドで `worker_executable()` → `VlmWorkerClient::start_at()` を呼び、
`kill_handle()` を App に渡してから `hello()` → `load()` → `caption()` を直列で呼ぶ。
起動から hello までまとめた `spawn_at()` もテスト等に使える。
通常の中止は今の1件の完了後に結果を捨てて `shutdown()`、アプリ終了は App の kill handle から直ちに `kill()`。
終了は子を先に止め、I/O thread の join は待たない。client は再起動・再試行しない。
Rust 側でも終了理由・完全JSON・3欄の型/内容一致・Unicode文字数の上限を検査する。
worker の場所は debug build だけ `MIV_VLM_WORKER` を優先し、その後 portable は exe の隣、
通常版は core 内包の worker を `<data_dir>/vlm/workers/<内容のSHA256>/` に展開する。
展開は worker スレッド上で VST3 と同じ regular-file 検査・全バイト照合・一時ファイルからの置換を使う。
reparse point を拒否し、旧版は消さない。実行時は protocol と prompt_version を照合し、source identity は比較しない。
GPU の利用条件は選択した同一 device が Vulkan 対応かつ専用 VRAM 総量 7.5GiB 以上。
通常版・portable の build は `scripts/build-vlm-worker.ps1` で native worker を先に再ビルドする。
署名は通常版が内包前、portable がコピー後に行う。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/ai::vlm_worker::tests/) | test(/job_process::tests/) | test(/epub_convert::tests/)'
```

偽 worker は `tests/support/vlm_fake_worker.rs` の内容のハッシュを名前に含めた
`target/vlm-fake-worker/<hash>/` に rustc で build し、既存のものを使い回す。
同じ cache root 内の一時フォルダーで build してから、exe・PDB を含むフォルダーを rename で公開する。
同時 build で公開に負けた側は既存のものを使い、一時フォルダーを回収する。生成物は `%TEMP%` に残さない。
製品の exe、画像、モデル、設定は使わない。実モデル照合は次の3つの環境変数がすべてある場合だけ実行する。
PNG の decode → RGB8 はこのテスト内だけにあり、製品の画像準備・縮小は V1-3 で接続する。
画像をコピーせず、説明文の内容・画像名をテスト出力に出さず、3欄の完全一致を26枚すべてで検査する。

```powershell
$env:MIV_VLM_TEST_MODEL = 'G:\home\vlm-eval\models\unsloth__Qwen3.5-4B-GGUF\Qwen3.5-4B-Q4_K_M.gguf'
$env:MIV_VLM_TEST_MMPROJ = 'G:\home\vlm-eval\models\unsloth__Qwen3.5-4B-GGUF\mmproj-F16.gguf'
$env:MIV_VLM_TEST_IMAGES = 'G:\home\vlm-eval\inputs\1024'
cargo nextest run -p mimageviewer --lib -E 'test(/real_model_serve_matches_v1_0/)'
```

基準は `G:\home\vlm-eval\runs\20261102-V1-0\forward.jsonl`。
任意の `MIV_VLM_TEST_BASELINE` / `MIV_VLM_TEST_LUID` で基準ファイルとGPUを明示できる。
CPU threads は V1-0 と同じ24。合成テストと本体 lib 全件を走らせる際は実モデル用の3変数を外す。

### V1-1 の検証記録 (2026-11-02)

実装担当による検証。独立レビューは別途行う。
CTest は **6/6 成功**。Rust の VLM client・共通起動 helper・既存 EPUB テストは **76/76 成功**。
上記3変数を指定した実モデルテストは **26/26 枚で3欄が V1-0 と完全一致**し、全件が Rust 側の検証も通過した。
モデルは Qwen3.5-4B Q4_K_M + mmproj-F16、CPU threads 24、画像は指定済みの1024px版をその場で読み取った。
画像をコピーせず、説明文・画像名を報告へ転載していない。実モデルテスト全体は92.192秒。
通信系 import は引き続き無く、exe は ADVAPI32 / KERNEL32 / DXGI / Vulkan の4 DLLだけを import する。

Rust の JUnit と CTest ログは `target/vlm-v1-1/`、import監査は
`G:\home\vlm-eval\runs\20261102-V1-1-worker-serve\imports`。
本体 lib 全件は既存の `target/test-lib-full/` 台帳を使う。
この sandbox の初回依存ビルドでは MSVC の compiler PDB エラー C1090 と MSBuild 並列実行失敗を観測したため、
検証コマンド内だけで `_CL_=/Z7` と `CARGO_BUILD_JOBS=1` を指定した。
`MSBUILDDISABLENODEREUSE=1` を指定し、製品のソースや共通ビルド設定は変更していない。

## GPU の選択

`--device 00000000:00000000` のように `--list-devices` が示す `HIGH:LOW`（各8桁の16進 LUID）を指定できる。
省略時は DXGI/Vulkan を LUID で結べるアダプタのうち専用メモリ総量が最大のものを選ぶ。
空き VRAM は選択に使わない。DXGI の専用メモリ総量を結果に残す。
`--list-devices` は `vulkan_available` と `vulkan_error` も返す。Vulkan runtimeの初期化・列挙が失敗した場合もDXGI一覧を残す。
ただし静的importの `vulkan-1.dll` が無い場合はworker自体を起動できず、親側の起動失敗になる。

b12032 の Vulkan backend は公開 API に LUID/UUID を出さず、`get_proc_address` も NULL。
そこで `VkPhysicalDeviceIDProperties.deviceLUID` → Vulkan physical index を求め、ggml の初期化前に
`GGML_VK_VISIBLE_DEVICES` をその index 1つへ固定する。この環境変数は **ggml の番号ではなく raw physical index** を取る。
公開される唯一の `Vulkan0` の handle を llama の `devices` と mtmd の `device` 両方へ渡す。

別 VkInstance 間の index だけを同定の根拠にしない。`VK_EXT_pci_bus_info` があれば ggml の `device_id` と PCI ID を照合する。
なければ全 Vulkan physical device の中で名前が一意であることと、実 ggml device の description 一致を必須にする。
同名 GPU が複数あり PCI ID を得られない構成は拒否する。完全な同名複数 GPU 対応には ggml 側の UUID/LUID 公開 API が必要。
一覧には DXGI、Vulkan、ggml と対応の方式・曖昧さを表示する。継承した visibility filter は一覧時に解除する。

## 開発時の文法生成と入力照合（オフライン）

`tools/reference.cpp` は開発専用で、同じ b12032 の `llama-common` を使う。worker へリンクしない。
HTTP/取得 API は一切呼ばず、モデルメタデータの template と schema だけをローカルで処理する。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CMAKE_BUILD_PARALLEL_LEVEL = '1'
cmake -S crates/vlm-worker -B crates/vlm-worker/build -G 'Visual Studio 18 2026' -A x64 -DVLM_REFERENCE_TOOLS=ON
cmake --build crates/vlm-worker/build --config Release --target vlm-reference -- /m:1 /nr:false
# Regenerate after changing the fixed schema; the generated constant is tracked.
.\crates\vlm-worker\build\Release\vlm-reference.exe --grammar-header crates/vlm-worker/src/caption-grammar.h
.\crates\vlm-worker\build\Release\vlm-reference.exe "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" "$modelDir\mmproj-F16.gguf" 'G:\home\vlm-eval\inputs\1024' "$run\reference.jsonl"
```

`prompt.h` の `kSchema.properties.keywords.items.maxLength=32` を入力に、上の `--grammar-header` を実行する。
同じ b12032 の `json_schema_to_grammar(common_json::parse(kSchema))` の出力を `caption-grammar.h` に定数として埋め込む。
今回もこの手順で再生成し、`keywords-item` の `char{0,32}` を確認した。
実行時の schema 変換・Jinja は不要。reference は server と同じ `common_chat_templates_apply` に画像 marker と text の2 part、
schema、thinking=false を渡す。rendered prompt と worker の固定文字列を byte 比較する。
さらに server と同じ `mtmd_tokenize(add_special=true, parse_special=true)` で26枚を token 化して保存する。
worker の全 text token ID、画像 ID、画像 token数・位置数を照合する。画像 embedding そのものの比較はしない。
reference は実際の chat path の文法も `server-path.gbnf` に保存するので、schema 直変換の定数と照合できる。
実測では JSON 部分の生成規則は rule 名を揃えると完全一致するが、文法全体は一致しない。
server は assistant/thinking prefix の規則も持ち、`common` が generation prompt を grammar sampler に prefill する。
prefill 後の whitespace 受理状態と worker の JSON root 開始は異なるので、入力トークン一致や schema 部分一致だけから
文法状態の厳密同値・C9 の出力一致を保証するものではない。
確認後は冒頭の `VLM_REFERENCE_TOOLS=OFF` で構成し直し、worker をビルドする。

## a〜e の集計と import 検査

```powershell
python -I crates/vlm-worker/tools/summarize.py --baseline 'G:\home\vlm-eval\runs\20261102-C9\results.jsonl' --forward "$run\forward-32.jsonl" --reverse "$run\reverse-32.jsonl" --reference "$run\reference.jsonl" --out "$run\summary.json"
python -I crates/vlm-worker/tools/check_fixtures.py --server-grammar "$run\server-path.gbnf" --out "$run\grammar-comparison.json"
$dumpbin = 'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.50.35717\bin\Hostx64\x64\dumpbin.exe'
python -I crates/vlm-worker/tools/audit_imports.py --bin vendor/vlm-worker --dumpbin $dumpbin --out $run
```

集計は画像集合・重複を検査し、3欄の完全一致、各欄、finish_reason、usage、prompt token/chunk、全成功条件、
初回を除いた中央値・初回・読み込み時間を出す。説明文の内容や画像名は stdout に出さない。
import 検査は配布ディレクトリ以下の全 exe/DLL に `dumpbin /imports` を実行し、DLL名・サイズ・SHA-256 と全文を保存する。
`ws2_32`, `wsock32`, `winhttp`, `wininet`, `urlmon`, `dnsapi`, `mswsock`, `iphlpapi`, `netapi32`, `wldap32`, `rasapi32` の直接 import を拒否する。
これは worker/同梱 DLL の直接 import とリンク境界の検査であり、OS/Vulkan driver 内部や全ての動的ロード先の監査ではない。

## 初回の実測結果（40文字版、履歴）

2026-11-02、Windows / RTX 4090 / Intel Core Ultra 9 285、Release、上記設定。
個別 JSONL とログは `G:\home\vlm-eval\runs\20261102-V1-0\` にのみ置く。
実行・ビルド・CTest・fixture比較・import監査はいずれも exit 0。

| 確認 | 結果 |
| --- | --- |
| a. C9 と3欄すべて完全一致 | **21/26**。caption 22/26、text 25/26、keywords 21/26 |
| a. finish_reason / usage | finish_reason は26/26一致（すべて stop）、usage 全体は21/26一致。prompt_tokens は26/26一致、cached_tokens は全件0 |
| b. forward / reverse | **26/26**で3欄・finish_reason・usage が一致 |
| c. §6.5 | **両順序とも26/26成功**。EOG・完全JSON・欄の型・各上限・800 tokens以内を個別に記録 |
| d. worker imports | **ADVAPI32.dll, KERNEL32.dll, dxgi.dll, vulkan-1.dll** の4本だけ。通信系 import なし。同梱が必要な DLL は0本 |
| e. forward | モデル読み込み **2.479秒**、最初の1枚 **24.861秒**、以降25枚中央値 **1.500秒** |
| e. reverse | モデル読み込み **2.368秒**、最初の1枚 **21.758秒**、以降25枚中央値 **1.481秒** |
| e. サイズ | exe **48,076,288 bytes（45.85 MiB）**。追加配布 DLL なし |
| 入力照合 | common/server と同じ展開・token化経路の参照と、**26/26で全 text token ID・image ID・画像token数・位置数が一致** |
| GPU 照合 | RTX 4090: LUID `00000000:00016F0D` → physical 0 → PCI `0000:02:00.0` → ggml `Vulkan0`。DXGI専用総量 **25,310,527,488 bytes** |
| 一覧・異常入力 | 継承 `GGML_VK_VISIBLE_DEVICES=1` があっても Vulkan/ggml各2台を表示。未知LUIDは exit 1、既存outは exit 1で内容保持 |
| ソース保護 | llama.cpp 全 **4,015ファイルの SHA-256 が作業前後で完全一致** |
| 自動検証 | CTest **2/2成功**。Unicode長・型・不完全JSON・EOG/出力上限等の検証器、greedy の全面maskとの同値12ケース（tie・accept回数を含む） |

exe SHA-256: `13745cec9f29378f1d657de995af1f65346973bb797f2ea149e097d8eec0f6de`。
link command でも `llama-common` / HTTP / RPC のライブラリがないことを確認した。
入力トークン列の比較は server のコード経路をオフラインで再現したもの。過去の C9 実行には全 token ID の記録がなく、C9 そのものとは token 数を比較した。

差が出た5枚には caption の値の差4件、text の値の差1件、検索語の差5件がある（欄は重複）。
caption/text の差は空白・句読点だけの差ではない。検索語の差も並べ替えだけではなく、うち1件は個数も違う。
いずれも JSON は正常で、説明文・文字起こし・検索語の内容はここへ転載しない。

**原因は未確定。** 固定prompt、元PNGとhelper経路、token/chunk情報、schemaのJSON生成規則は照合済み。
全件の出力が JSON の `{` から始まり、差は欄の値に出ている。
server は `mtmd_batch_encode` と server slot の評価経路、worker は `mtmd_helper_eval_chunks` と毎画像の新規contextを使う。
配布版 C9 と今回の build 設定・数値演算、画像embeddingやdecoder評価の浮動小数点差を候補として残す。
文法全体とprefill状態の厳密な同値、画像embedding・logitsの数値比較は未実施なので、どれが原因かは断定しない。
切り分けを続ける場合は、同じ build のオフライン参照に grammar prefill と評価の設定を揃え、最初に分岐する logits を比較する。

初回は forward の prompt 評価 **23.432秒**が大半（context初期化0.202秒、生成1.179秒）。
以降の中央値は context初期化0.157秒、prompt評価0.261秒、生成1.013秒。
Vulkan の初回pipeline/graph準備が遅延の候補だが、その内部の時間配分は未計測。
文法を毎回全語彙へ適用した初版は約7秒/枚だったため、greedy候補を先に検査する形へ変更した。
初版と最終版で **26/26のcontent・3欄・finish_reasonが完全一致**し、この最適化はC9との差を生んでいない。

残る範囲は、C9との差の厳密な数値切り分け、初回の遅延、PCI ID がない同名複数GPU、別GPUでの性能、
本体IPC・ジョブ・保存・UI・配布への組み込み。検索語上限の32文字への統一は下記の追補で完了した。
今回 Rust本体・Cargo/workspace・llama.cppソースを変更せず、mIV製品バイナリは起動していない。コミットもしていない。

## 指摘対応と再確認（32文字版、2026-11-02）

設計担当による `db7434bf0` の検収後の修正。上の失敗分類・排他作成・入力制限はこの追補で導入した。
schema と GBNF を32文字に統一し、reference の全引数は `wmain` から UTF-8 に変換、filesystem は `u8path` で扱う。

- CTest **4/4成功**。生のNUL＋末尾のゴミ、不正UTF-8（継続byte単独・overlong・途中終了・surrogate・Unicode範囲外）、
  正当な `\u0000`、JSON前後の空白、空白だけを検証。
- 画像処理の同じ実行境界に前処理・文脈超過・prompt/生成評価エラーを注入し、失敗行を作って次の画像へ進むことを確認。
  致命的な戻り値、割当失敗の診断、`bad_alloc` は外へ伝播して全体を止めることを確認。実VRAM不足は発生させていない。
- 131072×1 のPNGヘッダを読み、寸法で拒否する回帰テスト。出力は同期して開始する8 threadで新規作成を競わせ、
  1つだけが書けること・既存内容が保持されることを検証。いずれも使い捨ての合成データだけを使う。
- reference を日本語の入力フォルダ・PNG名・出力名・文法出力名で実行して成功。
  同じ32文字 schema からの再生成headerがbyte一致し、serverのJSON部分とも生成規則が一致することを確認。

26枚の forward は **1回だけ再実行**し、`G:\home\vlm-eval\runs\20261102-V1-0-review-fixes\forward-32.jsonl` に新規保存した。
**26/26で全検証成功**。前回 `20261102-V1-0\forward.jsonl` と caption・text・keywords・content・finish_reason・usage・
prompt_chunks がそれぞれ **26/26完全一致**。検索語は最長17文字だった。
時刻・所要時間と、新設した `validation.encoding_valid` は比較対象に含めない。

```powershell
python -I crates/vlm-worker/tools/compare_forward.py --previous 'G:\home\vlm-eval\runs\20261102-V1-0\forward.jsonl' --current "$run\forward-32.jsonl" --out "$run\forward-comparison.json"
```

読み込み3.561秒、初回35.106秒、初回以外の中央値1.635秒。
exe は48,080,384 bytes、SHA-256 `1b493a52b3a658e17d24a427ad659b5a3dafe88ec739da5c54324021d05963d4`。
再度の `dumpbin /imports` は従来と同じ4 DLLのみ、通信系 import なし。
llama.cpp は4,015ファイルすべてのSHA-256が初回作業前と一致した。Rust本体は未変更、製品起動・通信・コミットは行っていない。

## ログ分類と入力byte上限の追補（2026-11-02）

`2f0c54be8` 後の2件への対応。ログの全文部分一致を廃止し、上記のとおり戻り値を優先する。
mtmd側だけは失敗の種類が失われるため、ERRORレベル・発生元・固定診断形式を照合する補完を残した。
モデルパスやメタデータのINFO/DEBUGログ、ERRORに含まれる判定語、CONTや診断に続く余分な内容では致命扱いにしない。

PNG入力の固定上限は16 MiB。読取中にも制限するので、大きな付加チャンク・末尾データ、seek不可の入力、
検査後に増えるファイルでも入力全量を保持しない。上限超過は失敗行にして次の画像へ進む。
STBに渡すbufferはこの読取関数からのものだけで、長さは必ず `int` に収まる。

CTest **5/5成功**。判定語を含む正常なパスの各logレベル、ERRORの非診断メッセージ、固定形式の資源失敗を回帰検証した。
入力は上限直前・上限ちょうどの受理、1 byte超過・2 GiB超の拒否、最大読取量16 MiB＋1 byte、
拒否後の次画像への続行を非seekの合成streamで検証した。2 GiBの実ファイルや私的な画像のコピーは作成していない。

forwardは新しい出力 `G:\home\vlm-eval\runs\20261102-V1-0-review-fixes-2\forward-bounded.jsonl` へ1回実行した。
**26/26で全検証成功**。直前の `20261102-V1-0-review-fixes\forward-32.jsonl` と、caption・text・keywords・content・
finish_reason・usage・prompt_chunks はすべて **26/26完全一致**。検索語は最長17文字。

```powershell
$run = 'G:\home\vlm-eval\runs\20261102-V1-0-review-fixes-2'
python -I crates/vlm-worker/tools/compare_forward.py --previous 'G:\home\vlm-eval\runs\20261102-V1-0-review-fixes\forward-32.jsonl' --current "$run\forward-bounded.jsonl" --out "$run\forward-comparison.json"
```

読み込み2.393秒、初回21.757秒、初回以外の中央値1.392秒。
exeは48,081,920 bytes、SHA-256 `708b8e2214251752f674737958de431b1115546cd555115dcc27cafbac6e671b`。
importは引き続き4本のOS DLLのみ。llama.cppの4,015ファイルも初回作業前から不変。
Rust本体・Cargo/workspaceは未変更、製品起動・通信・コミットは行っていない。

## Vulkan API識別子の追補（2026-11-02）

`93522fa01` 後のP2対応。Vulkan例外の発生元を `vk::<型>::<関数>` の形式で検査し、型の列挙を廃止した。
ERRORレベル・mtmdの例外報告形式・末尾の資源エラーコードの照合は維持する。
`CommandBuffer::begin` / `end` のメモリ不足とdevice喪失、encode失敗が画像単位の失敗へ落ちず全体へ伝播することを回帰検証した。
不正な識別子・余分な修飾・パス文字・未知のエラーコード・ERROR以外のログを受理しないケースも追加した。
CTestは **5/5成功**。

forwardを `G:\home\vlm-eval\runs\20261102-V1-0-review-fixes-3\forward-vulkan-identifiers.jsonl` へ1回再実行した。
**26/26で全検証成功**。前回 `20261102-V1-0-review-fixes-2\forward-bounded.jsonl` と、
caption・text・keywords・content・finish_reason・usage・prompt_chunks はそれぞれ **26/26完全一致**。
モデル読み込み2.401秒、初回22.214秒、初回以外の中央値1.412秒。
集計・CTestログ・import監査・ソース保護確認は同じrunフォルダに記録した。
Rust本体・llama.cppは未変更、製品起動・通信・コミットは行っていない。
