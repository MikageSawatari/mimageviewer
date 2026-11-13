# ローカル文字起こし T1 区切り1 実装・検証記録

## 検収後の修正（2026-11-14、現在の結果）

検収済み `280f0301c` に対するP2/P3と、設計担当が決定したUTF-8修復の変更を実装した。
今回の変更はコミットしていない。作業ツリーは `G:\home\mimageviewer-asr-t1` だけを編集し、
本体製品exe、通常プロファイル、バックアップには触れていない。指定のworkerだけを単体実行した。
別の `gpt-6.1-sol / xhigh` による完了レビューで、新規P1/P2/P3の指摘はない。
以下の現行結果が、後半に残した初回実装の履歴を更新する。

### 修正内容と契約

- GPU選択からloadまで選択したLUIDを保持する。`LoadRequest.device_luid` を送信し、
  workerは最新の一覧でLUIDから解決する。番号の `device_index` は受け付けない。
  Rustは応答のLUIDを要求、およびDXGIのLUIDと照合する。物理番号が変わった同じGPUは受理し、
  番号が同じ別GPUは拒否する。既存VLMの選択部分を純粋な共通関数へ移し、VLMの条件は維持した。
- 端の空白はPython `str.isspace()` と同じ29文字に固定した。U+3000を含み、U+200B/U+FEFFは含めない。
  有効なUTF-8へ復元した後に `str.strip()` 相当を適用する。内部の空白は保つ。
- 採用するtoken本文でだけ復元不能なUTF-8断片を捨てる。不一致まで一致したprefixを捨て、
  不一致を起こしたbyteは先頭として再評価する。`e3 83 e3 81 86` は `e3 83` だけを捨て「う」を保つ。
  正常な区間またぎ、ASCII、4byte文字は維持する。窓末尾の未完prefixは従来どおり捨てる。
  共有framingは変更せず、Rustの不正wire拒否も維持した。
- `Window` / `End` の必須u64フィールドは `utf8_discarded_bytes` / `utf8_discarded_fragments`。
  不一致で捨てるprefixを1断片、不正な先頭・単独の継続byteを各1byte・1断片と数える。
  overlong・surrogate・範囲外の2byte目で失敗した場合も先頭だけを捨て、残りを再評価する。
  窓末尾の未完prefixは全体で1断片。採用する区間だけをtrim前に数え、捨てるlookaheadと端の空白は数えない。
  区間が空になって除去されても破棄数は残す。Endは公開したWindowの合計、beginで0へ戻す。
  clientは値の整合、加算overflow、終端合計も検査する。定義はコードとworker READMEにも記載した。
- `Draining`、`InvalidUtf8` 専用の `FileFailure`、Rustの `AsrFileFailure` / `AsrEvent::FileFailure` を削除した。
  抽出途中でendを送り、呼出し元が暫定窓を捨て、次のbeginへ進む使い方をC++の実StreamとRustの偽workerで検査した。
  モデルの再load、再試行、回復処理は追加していない。

窓の切り方・継ぎ目・CLI paramsは検収済み実装から変えていない。設計担当が更新した
`docs/local-asr-transcript-plan.md` は今回の開始時から変更済みであり、実装側からは編集していない。
計画との意図した相違はない。破棄数のperf・ログへの接続、抽出失敗を扱う作成ジョブは依頼どおり次の区切り。

### 最終workerでの実モデル回帰

最終worker（下記SHA）でモデルを1回loadし、m1→m2→m3→m4→m5→m6→Gを同じprocessで実行した。
GPUはRTX 4090、要求と応答のLUIDはともに `00000000:00016A26`。
モデルは評価正本の `ggml-large-v3-turbo-q5_0.bin`。m1〜m5は
`work/runs/turbo-q5_0-vulkan-mc0/<素材>/audio16k.wav`、m6は指定の
`work/runs/t0/m6/audio16k.wav`、Gは `work/user-videos/audio/G.wav` を読んだ。

| 素材 | 窓数 | 区間数（worker / 評価） | 全区間の本文・開始・終了 |
| --- | --- | --- | --- |
| m1 | 1 | 22 / 22 | 完全一致 |
| m2 | 1 | 28 / 28 | 完全一致 |
| m3 | 1 | 31 / 31 | 完全一致 |
| m4 | 1 | 28 / 28 | 完全一致 |
| m5 | 1 | 10 / 10 | 完全一致 |
| m6 | 3 | 477 / 477 | 完全一致 |
| G | 9 | 3488 / 3488 | 旧replace本文との差は2区間。生byteから独立修復した参照とは全件一致 |

m1〜m6の破棄数はすべて0。m6の継ぎ目は1805020 / 3608180msでT0と一致した。
Gは507,010,868bytesのPCMをすべて送信し、全9窓と `end status=ok, duration_ms=15844089` を受信した。
Gの開始・読み終わり・継ぎ目・区間数・破棄数も全窓でT0参照と一致した。
T0は最終長を秒の小数点以下2桁へ丸めて15844090msとするため、その1msだけは実WAVのsample数から
求めたfloor値15844089msを期待値とした。差は比較JSONの `t0_rounding_delta_ms:1` に残した。
区間時刻を補正する変更はない。

Gの差は以下の2か所だけ。旧評価は不正byteを `errors="replace"` で読んでおり、そこにあった
置換文字が今回なくなる。後続の正常な文字は維持され、区間数と全区間時刻は変わらない。
本文は記録へ転載せず、元CLI区間番号・byte位置はともに0始まりで示す。

| 窓 | 元CLI区間 | 区間の絶対開始 / 終了 (ms) | 区間本文のbyte位置 | 捨てたhex | 次区間の先頭（維持） |
| --- | --- | --- | --- | --- | --- |
| 1 | 162 | 760740 / 790740 | 511〜512 | `e3 83` | `e3 81 86` |
| 3 | 88 | 3935720 / 3935720 | 279〜280 | `e3 81` | `e3 81 b2` |

どちらも前区間の末尾の未完prefixを持ち越した後、次区間の新しい先頭で不一致となったもの。
合計4byte・2断片。参照解析は元CLI JSONを `surrogateescape` で読んでbyteを復元し、
workerとは別のPython実装で採用区間の修復・stripを行った。
証跡は `target/asr-t1-fixes/{asr-final.json,asr-final.stderr.log,asr-command.json,asr-comparison.json}`。

VLMも同じ最終workerでQwen3.5-4B Q4_K_M + mmproj-F16を再確認した。
統合前exe（SHA `d342f3564c87362b160d296967ed70b823e1d82dfb3254f379ed53a19ba298b9`）との比較で、
READMEの26枚はvalidation 26/26、caption/text/keywords/content/status/finish_reason/generated_tokens/
usage/prompt_chunks/validationの10項目とGPUがすべて26/26完全一致。
`--serve` protocol 2の実モデル1枚も10項目・GPUが一致し、shutdown終了0。
記録は `target/asr-t1-fixes/vlm-comparison/{summary.json,commands.txt}`。

### G通しのメモリと所要時間（§13）

100msを指定して現在の作業セットとcommit（GetProcessMemoryInfoのPagefileUsage）を採取した。
窓応答を読み終えたreader threadで窓末尾も採取し、窓内ピークをリセットする。
表のpeakは100ms採取値の窓ごとの最大、末尾は窓応答受信時の値であり、OSの累積peakと区別する。
Gの範囲は5,006採取（周期4,997 + begin以降の窓snapshot等）。周期の間隔の中央値100.36ms、最大147.74ms。
全7入力の生timelineは `asr-final.json.memory_samples` に6,071件保存した。
MiBは1,048,576bytes。Gの各窓は次のとおり。

| 窓 | 開始 / 読み終わり / 継ぎ目 (ms) | 区間数 | 破棄 byte / 断片 | WS peak / 末尾 (MiB) | commit peak / 末尾 (MiB) |
| --- | --- | --- | --- | --- | --- |
| 1 | 0 / 1829800 / 1803260 | 412 | 2 / 1 | 623.11 / 401.99 | 2071.63 / 1349.83 |
| 2 | 1803260 / 3634680 / 3604960 | 447 | 0 / 0 | 628.98 / 409.11 | 2078.16 / 1379.62 |
| 3 | 3604960 / 5428790 / 5399220 | 377 | 2 / 1 | 635.29 / 420.70 | 2107.04 / 1393.82 |
| 4 | 5399220 / 7219220 / 7193860 | 382 | 0 / 0 | 646.66 / 423.64 | 2122.36 / 1398.38 |
| 5 | 7193860 / 9027210 / 8999200 | 304 | 0 / 0 | 652.11 / 439.51 | 2128.45 / 1382.18 |
| 6 | 8999200 / 10829460 / 10814500 | 313 | 0 / 0 | 666.34 / 435.50 | 2111.36 / 1410.41 |
| 7 | 10814500 / 12640050 / 12610200 | 420 | 0 / 0 | 663.27 / 442.36 | 2138.12 / 1380.95 |
| 8 | 12610200 / 14440670 / 14410700 | 462 | 0 / 0 | 670.12 / 457.09 | 2112.20 / 1387.21 |
| 9 | 14410700 / 15844089 / 15844089 | 371 | 0 / 0 | 642.47 / 449.52 | 2111.07 / 1384.10 |

窓末commitは1349.83〜1410.41MiBに収まり、後半も上下している。窓末WSは401.99から449.52MiBへ
47.53MiBの緩い増加があり、最大457.09MiBから最終窓では低下した。「増加が全くない」とは扱わない。
9窓の観測では窓ごとのPCM・復号用一時メモリが累積する規模の増加は見られないが、
これより長い入力での挙動まではこの1回の測定では断定しない。
全processのOS累積peakはWS735.53MiB / commit2138.12MiBで、表の周期採取peakとは一致しない。

同じ最終workerのm6も、窓末WS/commitは365.86/1336.87 → 373.30/1350.03 → 370.20/1341.24MiB、
窓内peakは588.10/2020.76 → 595.03/2065.30 → 441.02/1981.34MiBだった。
Gの処理は501.870秒 / 音声15844.089秒、実時間比0.03168（約31.57倍速）。m6は73.414秒。
これはbeginからEnd受信までの参考値で、モデルloadを含まない。他セッションの負荷は排除できていない。

### 検証コマンドと最終成果物

以下のビルド・テスト・実モデル計測はすべて指定の入口経由で実行した。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command '<下表のコマンド>'
```

ロック領域へ書けずrunnerが警告してBelowNormalで続行した。Rustでは `CARGO_BUILD_JOBS=1`、
`MSBUILDDISABLENODEREUSE=1`、`CMAKE_BUILD_PARALLEL_LEVEL=1` を設定した。
Rustテスト時のPATHに `vendor/ffmpeg/bin` / `vendor/vcrt` を加えた。

| コマンド（wrapper内） | 結果 |
| --- | --- |
| `cmake -S crates/vlm-worker -B crates/vlm-worker/build -G 'Visual Studio 18 2026' -A x64 -DVLM_REFERENCE_TOOLS=OFF` | 成功 |
| `cmake --build crates/vlm-worker/build --config Release --target mimageviewer-vlm vlm-contract-tests vlm-greedy-tests vlm-image-attempt-tests vlm-output-tests vlm-image-input-tests vlm-protocol-tests asr-contract-tests -- /m:1 /nr:false` | 成功 |
| `ctest --test-dir crates/vlm-worker/build -C Release --output-on-failure` | 最終8/8成功。UTF-8持越し・破棄/計数・4byte/overlong/surrogate/範囲外・Unicode空白・列挙順の入替・途中end→次beginを含む |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(ai::asr_worker::) \| test(ai::vlm_worker::)' --no-fail-fast` | 45/45成功、7.974秒（compile 6分56秒）。strictな不正wire拒否・kill解除も維持 |
| `python -B -I crates/vlm-worker/tools/test_asr_transcribe.py` | 5/5成功 |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | 成功、54.50秒 |
| `powershell -NoProfile -File scripts/vlm-worker-identity.ps1 -ValidateRepo .` | 成功、最終exeと現sourceのidentity一致 |
| `python -I crates/vlm-worker/tools/audit_imports.py --bin vendor/vlm-worker --dumpbin <VS dumpbin.exe> --out target/asr-t1-evidence/imports-repair-luid` | ADVAPI32/KERNEL32/dxgi/vulkan-1のみ。ws2_32/winhttp/wininetなし |
| `python -B -I scripts/asr-model/regression_vlm.py run ...` / `compare ...` | 最終workerの26枚＋serve1枚すべて一致。実引数は上記 `commands.txt` |
| `python -B -I crates/vlm-worker/tools/asr_transcribe.py --worker vendor/vlm-worker/mimageviewer-vlm.exe --model <評価model> --device-luid 00000000:00016A26 --out target/asr-t1-fixes/asr-final.json <m1..m6,GのWAV>` | 7入力すべてok、shutdown終了0。省略しない実引数は `asr-command.json` |
| `python -B -I scripts/asr-model/analyze_repaired_asr.py --run target/asr-t1-fixes/asr-final.json --evaluation-work G:\home\mimageviewer-asr-eval\scripts\asr-eval\work --out target/asr-t1-fixes/asr-comparison.json` | 全件比較成功、exit0 |
| `python -B -I target/asr-t1-fixes/final-source-checks.py`（内部で `cargo fmt`、`cargo fmt --check`、`git diff --check`、identity再検査） | すべて成功。独立参照decoderの5つの境界ケースと持越しbyteの出所、Python3ファイルの構文、最終exeのSHAも再検査 |

ログは `target/asr-t1-fixes/{rust-focused,rust-check,python-tool-tests,vlm-run,vlm-compare}.log`、
最終検査は `target/asr-t1-fixes/final-source-checks.log`、
nativeは `target/asr-t1-evidence/worker-repair-luid-{build-ctest,identity,import-audit}.log`。
初回のnamed pipe2件は検収側がsandbox外で再実行して成功済み。この環境の制約として扱う。
全件libとworkspace gateは今回の修正で再実行しておらず、初回の有効な検証結果を再利用し、影響範囲を再検査した。
前回のworkspace gateが要求したrelease内包用5exeは今回も追加buildしていない。
UI未接続のAPI/worker修正なので本体の `build-dev.ps1` は実行せず、検証用成果物は下記Release workerとした。

`vendor/vlm-worker/mimageviewer-vlm.exe` は48,491,520bytes。

- SHA-256: `d7e38aee6e90cc96ce8125b684d9e875c37a06d6f17582043d8762cc7979811c`
- source identity: `572c3ed51f59f4318f94b2ece56f9c6a0537cb8d9648b638ca73b19af17eb31e`

### 今回の変更ファイル

| ファイル（brace内は同じディレクトリ） | 要点 |
| --- | --- |
| `crates/vlm-worker/src/{devices.cpp,devices.h,device_selection.h}` | LUIDでの最新一覧の解決と、既存VLM選択部分の共有。新規 `device_selection.h` |
| `crates/vlm-worker/src/{asr.cpp,asr_protocol.h,asr_window.h}` | LUID load、Unicode trim、断片修復・破棄数、状態の削減 |
| `crates/vlm-worker/src/asr_errors.h` | `InvalidUtf8` 専用型を削除（ファイル削除） |
| `crates/vlm-worker/tests/asr.cpp` | GPU identity・空白・UTF-8修復と数え方・early endの回帰 |
| `src/ai/{asr_worker.rs,asr_worker_tests.rs}` | 要求/応答LUID、Window/End破棄数と検査、FileFailure削除と回帰 |
| `tests/support/vlm_fake_worker.rs` | ASR load入替・破棄数・不正応答のfixture |
| `crates/vlm-worker/tools/{asr_transcribe.py,test_asr_transcribe.py}` | LUID送信、破棄数検査、100ms生timeline、窓区間数と受信位置 |
| `scripts/asr-model/analyze_repaired_asr.py` | 生byte由来の独立参照・破棄位置・全素材比較（新規） |
| `crates/vlm-worker/README.md`, `docs/{async-architecture.md,local-asr-t1-chunk1-report.md}` | 現行契約と検証記録 |

## 初回実装の履歴（検収済み280f0301c、以下は修正前）

以下は初回検収時の記録で、`device_index` / `FileFailure` / Gの失敗と未測定の記述は上の修正で置き換わった。
設計・検収はClaudeCode Opus、実装・テストはCodex、独立レビューは別のgpt-6.1-sol / xhigh。

## 範囲と API

- `mimageviewer-vlm.exe --asr-serve` / ASR protocol 1。既存 `--serve` / VLM protocol 2 は維持。
- `AsrWorkerClient::start[_at]` → kill handle をジョブへ公開 → worker thread で `hello` / `load` → `split`。
  `AsrInput` を抽出 thread、`AsrOutput` を結果 thread が所有する。両 endpoint を保持したまま
  begin/PCM/end を繰り返せる。抽出 thread は end を送っただけで Input を捨てず、結果 thread の
  End 受信まで保持する。どちらの Drop もプロセスを停止するので、相手の pipe 待ちが解ける。
- `AsrEvent::{Begin, Progress, Window, End, FileFailure}`。Window は絶対 ms の区間、読み始め・読み終わり・継ぎ目・最終窓の印を持つ。
  begin 応答も送信済み要求番号と照合し、不正 UTF-8、時刻の逆転・非単調、窓の不連続を拒否する。
  終端のdurationは、送信側が実際に送ったPCMの全sample数とも照合する。
  `FileFailure` は1本だけの失敗で、途中に返した窓も保存しない。同じモデルで次のbeginへ進める。
- 共通exeの展開は共有mutexで検査から公開までを直列化する。ASRとVLMの初回同時展開が
  起動済みexeを置換しようとする競合を防ぎ、毎回の照合・破損修復は維持する。結果キャッシュや再試行は追加しない。
- `asr_worker::detect_gpu` は共通の DXGI/Vulkan 列挙 snapshot を使い、software 以外の最大専用メモリを選ぶ。
  同量なら列挙順、専用メモリ0の内蔵GPUも残す。VLMの候補除外・7.5GiB条件は維持。
  load に渡す番号は Vulkan の physical_index。worker が LUID と突き合わせて単独 Vulkan0 に固定する。
- `AsrAddon` は `ModelAddon` の取得・中止・Range再開・状態・scan・削除を使う。
  `installed_model_path` はキャッシュ、`model_path()` は worker 専用の検証付き I/O。
  保存先は `addons/asr/packs/<model_id>/`、固定タグ `asr-model-v1`、index 名 `asr-model-index.json`。
  `MIV_ASR_MODEL_BASE_URL` は debug のみ。schema1 / model・LICENSE・NOTICE / MIT / 分割なし / 2GiB未満を検査する。

DB、作成ジョブ、FFmpeg抽出、UI、MCP、検索は今回追加していない。

## 実装判断

窓の規則は T0 採用版と同じ。16kHz の s16le を float にし、10ms ごとの float64 平均二乗を
50枠で積算する。30分の前後10秒に中央が入る最小0.5秒を選び、同値なら早い方にする。
切れ目の30秒先まで読み、その窓の区間の頭から継ぎ目を選ぶ。読み足しが EOF に届いたら
全区間を使って終了する。読み足しのちょうど末尾では次の sample または end を待つため、
1800.2秒やちょうど1820秒の入力で余計な空の窓を作らない。

保持上限は静音探索の後ろ10秒も含む1840秒のfloat PCMと、最大1MiBの受信枠。
CLIへ露出しない内部 Cut 注入で、T0の発話途中の強制切れ目も同じ Stream で検査する。
通常の切れ目は固定の quiet_cut。T0 の規則との意図した相違はない。

CLI `-l ja -t 8 -bs 5 -bo 5 -mc 0` に対応する decoding params を明示した。
スレッド数はコア数と8の小さい方。temperature=0、増分0.2、entropy=2.4、logprob=-1、
no_speech=0.6、suppress_nst=false、VAD/DTWなし、flash attentionあり。
CLIが上書きしない値は v1.9.5 の default params を使う（no_context=trueを含む）。
重みを読み込んだ context は保持し、復号の whisper_state だけを窓ごとに作って破棄する。
CLIの1回実行と同じ初期状態を使い、復号用の一時メモリを次の窓へ持ち越さない。

特殊tokenを除いたtoken byte列を区間ごとにつなぎ、末尾の未完UTF-8を次区間へ送る。
窓末尾の断片は指定どおり破棄する（次窓が継ぎ目から独立に書き起こすため）。
本体は置換せず strict JSON/UTF-8 として読む。モデルが復元不能なUTF-8を出した場合は、
§6.5に従いその1本だけ失敗にする。workerはRunning→Drainingの型付き状態で残りPCMを
既存endまで読み捨て、`end status=item_failure, code=invalid_utf8` を1回返してLoadedへ戻る。
end中に不正が見つかった場合は直ちに失敗終端を返す。失敗した枠の残り・読み捨てた枠も
時間に数える。中間の失敗通知・取消命令・モデルの読み直し・再試行は追加しない。
モデル・GPU・プロセスの失敗や壊れたwireは引き続き致命的な型付き失敗とする。
EOFを見た読取threadが停止権を行使しても、先に届いたfatal応答を失わないようにした。

ソースなしの既存worker再利用を維持するため、whisperの関係38ファイルの正規化SHAを
追跡manifestへ固定し、そのmanifestをidentityへ含めた。ソースがあれば全件照合し、
再ビルド時はソース必須。改変・不足・追加・reparse pointを拒否する。

## 検証コマンド

コンパイル・テスト・実モデル推論は次の共通入口を使った。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1-codex -WorkDir G:\home\mimageviewer-asr-t1 -Command '<下記コマンド>'
```

このsandboxではロック領域に書けず、runnerは警告後に BelowNormal で続行した。
実モデルのVLMとASRはこのセッション内で順番に流し、同時にGPU推論していない。
MSBuildはノード再利用なし、CMake buildは `/m:1 /nr:false`。

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` | 最終コードで成功（56.96秒）。最初の並列依存buildはturbojpegのMSBuildで失敗、単一build jobで解消 |
| `cargo nextest run -j 1 -p mimageviewer --lib -E '(test(/asr\|vlm_worker\|addon::\|worker_transport/) and not test(asr_addon::tests::published_v1_index_validates))' --no-fail-fast` | 最終112/112成功（22.314秒）。`CARGO_BUILD_JOBS=1` と `PATH=vendor/ffmpeg/bin;vendor/vcrt;...` を設定。初回はDLL探索でlist失敗、PATH設定後に成功 |
| `cargo nextest run -p mimageviewer --lib -E 'test(asr_addon::tests::published_v1_index_validates)'` | 実際に配置された公開用indexで1/1成功。ignore・欠落時skipなし |
| `cmake -S crates/vlm-worker -B crates/vlm-worker/build -G 'Visual Studio 18 2026' -A x64 -DVLM_REFERENCE_TOOLS=OFF` | 成功 |
| `cmake --build crates/vlm-worker/build --config Release --target mimageviewer-vlm vlm-contract-tests vlm-greedy-tests vlm-image-attempt-tests vlm-output-tests vlm-image-input-tests vlm-protocol-tests asr-contract-tests -- /m:1 /nr:false` | 成功 |
| `ctest --test-dir crates/vlm-worker/build -C Release --output-on-failure` | 8/8成功。既存6本、ASR窓・framing・UTF-8、固定ソースidentity |
| `powershell -NoProfile -File scripts/test-vlm-worker-identity.ps1` | 成功。whisperの固定hash・展開攻撃・既存保持も合成fixtureで検査 |
| `powershell -NoProfile -File scripts/vlm-worker-identity.ps1 -ValidateRepo .` | 成功 |
| `python -I crates/vlm-worker/tools/audit_imports.py --bin vendor/vlm-worker --dumpbin <VS dumpbin.exe> --out target/asr-t1-evidence/imports-final` | ADVAPI32/KERNEL32/dxgi/vulkan-1のみ。ws2_32/winhttp/wininetなし |
| `python scripts/check_ui_glyphs.py` | 危険なグリフ0 |
| `python -I crates/vlm-worker/tools/test_asr_transcribe.py` | 4/4成功。送信中の窓受信、fatal優先、ローカルWAVエラー、入力単位失敗の終端 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功 |

`scripts/test-full.ps1 -SuppressCrashDialogs` はworkspaceのテスト開始前に、launcher内包用の
`target/release/{mimageviewer-core,mimageviewer-remote,mimageviewer-epub-pdf,mimageviewer-mcp,mimageviewer-musicvideo}.exe`
5本の未配置で停止した（exit101）。`scripts/test-lib-full.ps1` の本体lib全件は、
17,618件実行・17,616成功（21 slow、1 leaky）・2失敗・76 skip、1057.211秒。
失敗は変更していない次のnamed pipeテストで、いずれもOS5 `PermissionDenied / AccessDenied`。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`（`src/agent_ipc/pipe.rs:334`）
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`（`src/agent_ipc/tests.rs:762`）

ASR・VLM・addonの失敗はない。この全件結果は展開mutex追加前のコードで、展開経路だけは
追加後のfocused112件で再確認した。関連しない全件の再実行はしていない。
したがって全gate成功とは扱わず、workspace gateのrelease内包入力とnamed pipe環境は検収側へ引き継ぐ。
ログは `target/asr-t1-evidence/rust-{focused,published-index,full-gate,lib-full}.log`。
全件JUnitは `target/test-lib-full/runs/20261113T153938722Z-a1509a32/junit.xml`。

検証用成果物は指定のRelease worker exe。`build-dev.ps1` による本体coreは、UI未接続の今回の
区切りでは作成していない（全gateの前提ファイル・実行環境も上記のとおり未充足）。
製品バイナリの起動と通常設定の操作は行っていない。実配布元からのモデル取得は未実施で、
fixtureサーバーでRange・中止・再開・hash・公開境界を検証し、公開用indexの契約を別途検査した。

## 実モデル回帰

VLM: Qwen3.5-4B Q4_K_M + mmproj-F16、READMEの26枚。
旧exeと統合後exeの双方で26/26が検証成功し、caption/text/keywords/content/status/
finish_reason/generated_tokens の7項目がすべて26/26完全一致、GPUも26/26一致。
同じRGBを送った `--serve` protocol2の実モデル1枚も7項目が一致し、shutdown終了0。
`target/asr-t1-evidence/vlm-comparison/summary.json` と `commands.txt` に記録した。
READMEの比較項目であるusage・prompt_chunksとvalidationも、before/afterの同じモード同士で
selftest26/26・serve1/1の完全一致を別途確認した（`vlm-comparison-expanded/summary.json`）。
prompt_chunksの診断表現はselftest/serve間で違うため、モードをまたぐ比較は内容の7項目で行う。
このVLM比較の統合後SHAは `c3b4b952da588211d2e34a9882b0b33f0763d423358507102b924dbd062aebb3`。
以後のC++差分はASRの内部Cut注入・UTF-8失敗分類・ログcallback・テストで、
VLM経路を変えていないため、独立レビューでも再利用可と確認した。

最終workerは48,496,640 bytes、SHA-256
`7ded5fbace18af476c3829a78bf775200c575569f8eaa63e023a6225906a065f`、identity
`1f966ecc03b1362e4f72000acb8723d2ac56f0ad0958e454bc13293f79292313`。
ASRは、評価と同じ16kHz抽出音声で次の結果だった（正常系を変えていない失敗分類修正前の
`62cbdb9d…` の証拠を再利用）。

| 素材 | 区間数（worker / 評価） | 文字・開始・終了の全件一致 | 窓 |
| --- | --- | --- | --- |
| m1 | 22 / 22 | 一致 | 1 |
| m2 | 28 / 28 | 一致 | 1 |
| m3 | 31 / 31 | 一致 | 1 |
| m4 | 28 / 28 | 一致 | 1 |
| m5 | 10 / 10 | 一致 | 1 |
| m6 | 477 / 477 | 一致 | 3 |

m6の継ぎ目も1805.02秒・3608.18秒でT0と一致。
最初は `materials/m6_long/master.wav` を使って483区間になったが、T0が使ったのは
AACから抽出した `runs/t0/m6/audio16k.wav` だった。両者は長さが同じでもsampleが違い、
最初の静音切れ目も1802.34秒と1802.35秒で違う。正しい抽出音声での再確認では全件一致した。
初回の出力を消さず、`asr-first6-comparison.json` と `asr-m6-correct-comparison.json` に区別して記録した。

G（15844.089秒）は最初の窓の出力で `Invalid ASR token UTF-8 continuation`。
T0の保存済み `uv-G/follow/w0/whisper.json` も、区間162の末尾に3バイト文字の先頭2バイト
`e3 83` が残り、次の区間は別の文字の先頭から始まる。区間のbyteをすべて連結しても不正であり、
単なる区間境界での分割と異なる。現状は指定のstrict処理を維持して、その1本だけを型付き失敗とする。
復元不能な断片も破棄して続ける扱いは別途設計判断が要るため、今回追加していない。

### workerのメモリ（§13）

`62cbdb9d…` で同じprocessのm6→Gを流し、100ms間隔の現在量と窓受信時の量を採取した。
作業セット・commitはMiB（1048576bytes）。窓末尾のsnapshotと、その窓の区間内ピークを分ける。

| 入力・窓 | 窓内ピーク 作業セット / commit | 窓末尾 作業セット / commit |
| --- | --- | --- |
| m6 窓1 | 596.1 / 2002.4 | 386.7 / 1354.3 |
| m6 窓2 | 619.2 / 2084.3 | 380.2 / 1345.4 |
| m6 窓3 | 504.1 / 1976.8 | 379.2 / 1347.6 |
| G 最初の窓・失敗まで | 608.7 / 2077.2 | — |

m6は3窓で末尾の保持量が増え続けていない。Gは認識結果の不正byteで止まったため、
9窓を重ねた長時間のメモリ確認は未完了。全process累積ピークは647.7MiB / 2084.4MiB。
記録は `asr-g-diagnostic.json` のwindow.memoryとworker_failure.memory。
他セッションの実行を排除できない条件のため、処理時間の厳密な比較には使わない。

最終workerではG→m1を同じprocessで再確認した。GのPCM507,010,868bytesをすべて受信し、
duration15844089msで `item_failure / invalid_utf8` を返した後、モデルを読み直さずm1へ進み、
22区間すべてが評価と一致。workerのshutdown終了は0。
Gの区間内ピークは600.8MiB / 1913.8MiB、読み捨て後は268.1MiB / 1121.3MiB。
この再確認でもGの文字起こしは未完了であり、9窓の推論が通ったとは扱わない。
証拠は `asr-g-then-good.json`。

## 変更ファイル

| 区分 | ファイル |
| --- | --- |
| 固定依存・identity | `.gitignore`, `scripts/setup-whisper-cpp.ps1`, `scripts/bootstrap-vendor.sh`, `scripts/vlm-worker-identity.ps1`, `scripts/test-vlm-worker-identity.ps1`, `crates/vlm-worker/whisper-source-manifest.json`, `crates/vlm-worker/tools/source-identity.ps1` |
| C++ worker | `crates/vlm-worker/CMakeLists.txt`, `src/{main.cpp,devices.cpp,devices.h,protocol.h,framing.h,asr.cpp,asr.h,asr_errors.h,asr_protocol.h,asr_window.h}`, `tests/{asr.cpp,source-identity.ps1}`（後二列の相対基点は `crates/vlm-worker/`） |
| Rust client・GPU | `src/ai/{mod.rs,vlm_worker.rs,vlm_worker_extract.rs,vlm_worker_tests.rs,worker_transport.rs,worker_gpu.rs,asr_worker.rs,asr_worker_tests.rs}`, `tests/support/vlm_fake_worker.rs` |
| モデル取得 | `src/{addon.rs,asr_addon.rs,vlm_addon_download.rs,lib.rs}` |
| 検証道具 | `crates/vlm-worker/tools/{asr_transcribe.py,test_asr_transcribe.py}`, `scripts/asr-model/{regression_vlm.py,compare_asr.py}` |
| 文書・通知 | `VLM-WORKER-LICENSE.txt`, `crates/vlm-worker/README.md`, `docs/{README.md,native-dependencies.md,architecture-overview.md,async-architecture.md,local-asr-t1-chunk1-report.md}` |

並行作業で追加された `scripts/asr-model/{asr-model-index.v1.json,model-NOTICE.txt,prepare_model_release.py}` は
上書きしていない。公開用indexは実ファイルを読むテストの対象にする。
