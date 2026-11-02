# ローカル AI 説明文 V1-0 worker

Rust 本体に接続しない Windows x64 の単体試作。製品バイナリ、通常の設定 DB、HTTP server を起動しない。
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
cmake --build crates/vlm-worker/build --config Release --target mimageviewer-vlm vlm-contract-tests vlm-greedy-tests -- /m:1 /nr:false
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
$run = 'G:\home\vlm-eval\runs\20261102-V1-0'
New-Item -ItemType Directory -Force $run | Out-Null
.\vendor\vlm-worker\mimageviewer-vlm.exe --list-devices
.\vendor\vlm-worker\mimageviewer-vlm.exe --selftest --model "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" --mmproj "$modelDir\mmproj-F16.gguf" --images 'G:\home\vlm-eval\inputs\1024' --out "$run\forward.jsonl" --order forward
.\vendor\vlm-worker\mimageviewer-vlm.exe --selftest --model "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" --mmproj "$modelDir\mmproj-F16.gguf" --images 'G:\home\vlm-eval\inputs\1024' --out "$run\reverse.jsonl" --order reverse
```

既存の `--out` は上書きしない。再実行には別の出力名を使う。引数は Unicode の Windows command line を受ける。
PNG のファイル名でソートし、reverse はその逆順。画像をコピーしない。入力デコードはこの段階だけ mtmd helper/STB を使う。

モデルと mmproj はプロセスで一度だけ読み込む。各画像で llama context（memory・sequence）と grammar sampler を新規作成し、
位置 0、sequence 0 から開始して、その画像の最後に破棄する。mtmd の画像チャンク・bitmap も各件で破棄する。
処理は直列なので supersession、rollback、並行ジョブの状態は作らない。製品の匿名パイプ・共有メモリ・Job Object は未実装。

固定条件は greedy、context 8192、生成上限 800、batch 2048、ubatch 512、flash attention AUTO、CPU threads 24（C9 と同じ）。
greedy 最大候補を先に文法で検査し、不許可のときだけ全語彙を mask する。常に全語彙を mask する場合と選択結果は同じ。
`prompt.h` のプロンプトは `run_eval.py` の v0-3、会話は Qwen3.5 の user 1件・tools/system なし・`enable_thinking=false` 専用。
任意モデルの chat template には対応しない。

JSONL は `image`, `caption`, `text`, `keywords`, `content`, `finish_reason`, `usage`, `generated_tokens`, `wall_s` と
`model_load_s`, `validation`, `status`, `prompt_chunks`, `device` を含む。stdout は番号・時間・成否だけで、説明文を表示しない。
`wall_s` は画像デコードから context/sampler の解放まで。モデル読み込み時間はモデル＋mmproj＋vision warmup を含み、context 作成は各画像に含む。
成功には EOG、完全な JSON、3欄の型、caption/text 各400 Unicode文字、検索語20個・各32文字、800 tokens以内をすべて要求する。
失敗行を含む完走は exit 2、起動・モデル・推論・出力 I/O のエラーは exit 1、全件成功は exit 0。
画像デコードの失敗は行を残して次へ進む。

**仕様差:** v0-3l の schema は検索語 **40文字**、計画書 §6.4 の成功条件は **32文字**。
C9 との比較用 GBNF は40文字を維持し、検証器は32文字を適用する。C9 の26件は最長17文字で、この差による不合格はない。

## GPU の選択

`--device 00000000:00000000` のように `--list-devices` が示す `HIGH:LOW`（各8桁の16進 LUID）を指定できる。
省略時は DXGI/Vulkan を LUID で結べるアダプタのうち専用メモリ総量が最大のものを選ぶ。
空き VRAM は選択に使わない。DXGI の専用メモリ総量を結果に残す。

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
# Initial development generation only; the generated constant is tracked.
.\crates\vlm-worker\build\Release\vlm-reference.exe --grammar-header crates/vlm-worker/src/caption-grammar.h
.\crates\vlm-worker\build\Release\vlm-reference.exe "$modelDir\Qwen3.5-4B-Q4_K_M.gguf" "$modelDir\mmproj-F16.gguf" 'G:\home\vlm-eval\inputs\1024' "$run\reference.jsonl"
```

`json_schema_to_grammar(common_json::parse(kSchema))` で作った GBNF を `caption-grammar.h` の定数へ一度だけ埋め込む。
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
python -I crates/vlm-worker/tools/summarize.py --baseline 'G:\home\vlm-eval\runs\20261102-C9\results.jsonl' --forward "$run\forward.jsonl" --reverse "$run\reverse.jsonl" --reference "$run\reference.jsonl" --out "$run\summary.json"
python -I crates/vlm-worker/tools/check_fixtures.py --server-grammar "$run\server-path.gbnf" --out "$run\grammar-comparison.json"
$dumpbin = 'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.50.35717\bin\Hostx64\x64\dumpbin.exe'
python -I crates/vlm-worker/tools/audit_imports.py --bin vendor/vlm-worker --dumpbin $dumpbin --out $run
```

集計は画像集合・重複を検査し、3欄の完全一致、各欄、finish_reason、usage、prompt token/chunk、全成功条件、
初回を除いた中央値・初回・読み込み時間を出す。説明文の内容や画像名は stdout に出さない。
import 検査は配布ディレクトリ以下の全 exe/DLL に `dumpbin /imports` を実行し、DLL名・サイズ・SHA-256 と全文を保存する。
`ws2_32`, `wsock32`, `winhttp`, `wininet`, `urlmon`, `dnsapi`, `mswsock`, `iphlpapi`, `netapi32`, `wldap32`, `rasapi32` の直接 import を拒否する。
これは worker/同梱 DLL の直接 import とリンク境界の検査であり、OS/Vulkan driver 内部や全ての動的ロード先の監査ではない。

## 実測結果

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
40文字の比較用schemaと32文字の製品上限の統一。本体IPC・ジョブ・保存・UI・配布への組み込みは次段階で設計する。
今回 Rust本体・Cargo/workspace・llama.cppソースを変更せず、mIV製品バイナリは起動していない。コミットもしていない。
