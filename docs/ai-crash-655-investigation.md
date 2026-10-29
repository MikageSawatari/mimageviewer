# 5ch >>655 AI 処理中の連続ページ送り調査

調査日: 2026-10-29。製品コードの修正・製品 exe の起動・デスクトップ操作・コミットは行っていない。
この文書は調査記録であり、原因が確認される前の候補を製品の確定不具合として扱わない。

**結果: 公開版 v4.9.0 と依存更新後の HEAD の各10試行、計20試行で終了症状は未再現。**
AI 未完了のまま10回進める条件、および取消要求後も推論が続く条件は再現した。
原因は未特定であり、製品修正・依存更新による解決済みとの判断はしない。実画面の描画・VRAM・window lifecycle は未検証。

## 対象と報告の区別

- 報告者の観測: 約30枚の ZIP、高速汎用、AI 完了前のホイール連続送りで約10枚先に進むと砂時計になり終了。
- >>641 の初期の複数回終了→後に安定、>>651 の読み込み中の移動・ダブルクリックも前提に含める。
- 「先読みバッファをオーバー」は報告者の仮説。版、画像寸法、GPU、先読み設定、表示モードは未確定。
- 原文 URL は閲覧ツールで取得できなかったため、依頼文に記載された経緯を根拠とする。未提供の返信内容は補わない。

| 区分 | 対象 |
| --- | --- |
| 公開版の基準 | `v4.9.0` = `7fea5839dfe3f3685080ab6956bf1b4a83b57db1` |
| 調査ブランチ | `ai-crash-655`、HEAD `fec29871bc85c19f9fbe64cc2dee3d494742bf21` + 調査用テストのみ |
| 確認時の master（23:27） | `45366aff56bcb811c1c57f0105fdf662f5c03047`。v4.10.0版情報・test-script専用キー修正等は進んだが、調査 HEAD との `src/app.rs` / `src/ai/` 差分なし |
| 現行 native AI | ORT DirectML 1.24.4 / ort rc.15、API 24 |
| 公開版側 native AI | ORT DirectML 1.24.2 / タグ内の patched ort rc.12 |

公開版ソースは Git archive を `target/ai-crash-655/v490/` へ展開した。Git LFS filter を無効化して取得したソースを使い、モデルは用意済みの実ファイルをコピーした。
公開版側の ORT は開発機の既存 DLL を読み取り専用でコピーし、`OrtGetApiBase()->GetVersionString()` が `1.24.2` であることを確認した。
LibRaw はタグの setup script と一致する SHA-256 のローカル保管アーカイブから 0.22.2 を展開、PDFium は保管されていた 8086 を使用する。
FFmpeg / モデル / その他のビルド入力は提供済み vendor から実ファイルをコピーする。ZIP 内 PNG の試験で FFmpeg・LibRaw・PDFium のデコードは通さない。

入力 SHA-256:

- 高速汎用モデル: `09B757ACCD747D7E423C1D352B3E8F23E77CC5742D04BAE958D4EB8082B76FA4`。タグの Git LFS pointer の oid・サイズ4871181と一致
- ORT 1.24.2 DLL: `A2323BC49544645B911743052F1EDCE594E17DF1E3423B71468C7386BC902F80`
- ORT 1.24.4 DLL: `E7EEDEC6A6F26DC39DC948276A75EF6D2BEE3FFF944D874CEED0BBD3B97BFF40`

## 自動試験

試験本体は `src/app/tests/ai_crash_655.rs` の `ai_crash_655_headless_zip_navigation`。
`src/app/tests.rs` の test-only module 宣言以外、既存ソースを変更していない。GPU 試験は明示実行の ignored test とし、通常の gate からは起動しない。

1回ごとに新しいテストプロセスを使う。既存の `phase_c_support::setup_app()` の TempDir、data_dir override と共有直列化ロックを保持する。
DLL・モデル・DB・ZIP・ログはその隔離先に置く。既存 `tests/test_ai_upscale.rs` は実行しない。
`MSBUILDDISABLENODEREUSE=1`。cold build の turbojpeg-sys 並列失敗後、`-j 1` で再実行した。
公開版側は古い lockfile の依存物も再ビルドした。TurboJPEG 成功後、依存コンパイル途中で自分が起動した build process tree だけを停止して `-j 4` へ変更し、完成済み依存を再利用した。
その中断（exit 15）はテスト実行前で、製品／テストの異常終了とは区別する。最終の両 `--no-run` build は exit 0。

30枚の RGB PNG を Stored ZIP にし、実際の ZIP 一覧採用・source worker・ページ移動の共通着地点 `land_still_page_navigation_target`・通常の final composite selector・先読み・eviction・結果回収を呼ぶ。
0→10 の11ページを訪問し、直前ページの AI 完了前に何回進めたかを記録する。初回のモデルロード・推論と、AI 結果保持済みの閲覧を区別する。
設定は隔離環境の新規既定値を用い、AI mode を Light・モデルを高速汎用にする。source先読みは後4／前12、AI先読みは後2／前3、処理上限は2048×2048未満。
ここでの cold は新しい process の ORT / Session / AI 結果 cache の状態を指す。OS のファイルキャッシュや driver の shader cache は消さない。

| ケース | 動作 |
| --- | --- |
| `rapid` | AI 完了を待たずに進む |
| `wait` | 各ページの AI 完了を待つ |
| `off` | AI 無効の対照 |
| `cached` | 0〜10を処理済みにしてから session close 用の live cache 破棄関数を呼び、保持 AI 結果を利用して進む。実ウィンドウの close/reopen は行わない。11枚を収容するため、この対照だけ保持枚数を20にする（既定10枚・512MiB） |

記録: job ID、対象 idx、items generation、取消フラグ、待機キューの画像バイト数、local activity、live/retained 件数、retained bytes、PrivateUsage / working set / peak working set。
`popped_or_result_waiting` は「キューから取り出したが pending に残る」要求であり、モデルロード中・推論中・結果未回収をそれだけでは区別できない。
`local_ai_activity` は待機を含む job lifetime の件数であり、同時推論数ではない。native 実行は perf の `upscale_begin` / tile / cancel / end と合わせて読む。
既存 logger / perf / native exception recorder は隔離先へ初期化し、通常終了・Rust unwind 時にログを artifacts へ保存する。native 強制終了時は標準エラーに表示した TempDir のログも確認する。

**限界**: テスト用 profile であり公開 release exe の試験ではない。OS ホイール入力の受付、実 HWND、swapchain、実描画、egui/wgpu の GPU texture upload、Lanczos 描画は実行しない。
egui texture delta は消費するだけなので、画像表示と DirectML が VRAM を共有する実画面の圧力を、この試験だけで否定できない。
新規テストの診断はすべて test-only で、製品には追加しない。

### 実行結果

公開版 `v4.9.0` / `7fea5839` + 調査テスト、ORT 1.24.2 の10試行はすべて1件実行・exit 0。
実際に隔離先へ展開された DLL の hash も、AI を使った全7試行で上記1.24.2の値と一致した。
非対話の `test` profile（unoptimized + debuginfo）、追加featureなし。公開 release exe の実行結果ではない。

| 公開版の入力 | rapid: 完了前に進めた回数 / Private MiB | wait: 同回数 / Private MiB | off: Private MiB |
| --- | ---: | ---: | ---: |
| 256×341 | 10 / 847.20 | 0 / 855.20 | 202.80 |
| 768×1024 | 10 / 1106.34 | 0 / 1614.75 | 198.64 |
| 1440×1920 | 10 / 2028.52 | 0 / 2763.25 | 270.30 |

公開版 cached は256×341で完了前の移動0回・Private 881.25MiB。Display の0〜10は warm-up 各1回だけで、再訪時に再推論しない。
公開版を含む全20試行で native exception log は0 bytes。AI失敗・panic・CPU fallback のログも検出しなかった。

現行 HEAD `fec29871` の10試行はすべて1件実行・exit 0。通常の `test` profile、feature追加なし。
表のメモリはナビゲーション時点でサンプルした最大 PrivateUsage（GPU dedicated bytes やプロセス全期間の最大値ではない）。

| 入力 | rapid: 完了前に進めた回数 / Private MiB | wait: 同回数 / Private MiB | off: Private MiB |
| --- | ---: | ---: | ---: |
| 256×341 | 10 / 789.86 | 0 / 850.71 | 186.60 |
| 768×1024 | 10 / 1107.64 | 0 / 1617.16 | 187.05 |
| 1440×1920 | 10 / 1837.79 | 0 / 2668.71 | 269.07 |

cached は256×341で実施し、完了前の移動0回・Private 898.77MiB。0〜10の Display 要求は各1回（warm-up）だけで、再訪時に再投入されず保持結果を利用した。2回目の閲覧中も idx11 の通常先読みは開始している。
全試行で native exception log は0 bytes、DirectML EP 登録失敗／CPU fallback のログなし。システムの物理RAMは127.27GiB（GlobalMemoryStatusEx）。低RAM・低VRAM機を試した結果ではない。
1440 rapid の最終ページ完了時は公開版でlive結果2件、現行側では1件だった。実行順と保持量が異なるため、版間のメモリ差をORT更新の改善／修正の証拠として扱わない。

公開版の代表証拠:

- `target/ai-crash-655/v490-analysis.json` と `v490-results.json`: 全10試行の集計、exit、実際の展開DLL hash。
- `target/ai-crash-655/v490-source-check.json`: タグと、展開先のapp / AI関連4ファイル・manifest・lockの一致確認。両版で使った追加テストファイルも同一hash。
- `target/ai-crash-655/v490-7fea5839-rapid-1440.log` と `v490-artifacts/rapid-1440/{mimageviewer.log,perf_events.jsonl}`:
  idx0/job1を22.768sで要求、23.454sから12タイルの処理開始。24.537s付近のidx6入場でidx0のcancelが立ち、24.649sで `upscale_cancel(after_tile=3)`。
  25.688sのidx10入場時には、job10（idx9）が既に処理中、最新job11（idx10）はキュー先頭に待機。世代は1のまま。
  idx9 の処理後、31.993sからidx10を処理し、37.394sに upscale end。取消フラグと実処理の終了が異なること、最新ページより前の近傍ページの実行が残り得ることを記録した。
- 同時点の取消済み queued idx1〜4 は各11,059,200 bytesのsourceを保持。合計約42.19MiBだが、他のsource cacheとのArc共有を考慮せず「追加RAM42.19MiB」とは数えない。

現行側と共通チェックの証拠:

- `target/ai-crash-655/head-analysis.json`: 10試行の exit・メモリ・取消タイル数・Display 要求 idx。
- `target/ai-crash-655/master-rapid-256.log`: 最初の cold 試行。モデルロードが終わる前に0→10へ進み、11 pending / 10 queued。idx0 は取消済みでも処理 owner に残り、最新 idx10 が次に処理された。
- `target/ai-crash-655/head-fec29871-rapid-1440.log` と `artifacts/rapid-1440/{mimageviewer.log,perf_events.jsonl}`:
  idx0/job1 を26.092sで開始要求、26.778sから12タイルの処理開始、27.977s付近の idx6 入場で idx0 の cancel が立つ。
  4タイル目の native 推論が返った後の28.355sで `upscale_cancel(after_tile=4)`。
  idx10/job11は29.248sで要求、29.625sに処理開始、36.739sに upscale end。初回ジョブが取消要求後も実行・回収待ちに残る場合を実際に通した。
- 1440 rapid の10枚先では、取消済み queued idx1〜4 の入力だけで約42.19MiBを保持（1枚10.55MiB）。11 activity は11並列推論を意味しない。
- `target/ai-crash-655/focused-results.json`: 既存の結果回収、PDF orphan、取消済み Ready の context/job 照合、先読み gate、古い retained epoch の拒否の5テストすべてexit 0。
- `target/ai-crash-655/fmt-check.log`: `cargo fmt --check` exit 0。
- `head-temp-cleanup.json` / `v490-temp-cleanup.json`: 実行終了後、今回のstderrに記録された使い捨てdata_dir各10件だけを検証・清掃。DLL／DBの開いたhandleによるTempDirの削除残りを、親側でprocess終了後に片付けた。

完了待ちの対照のほうがメモリ使用量が多くても正常に終わった。したがって、この開発機の結果から「連続送りだけでメモリ量が増えて終了した」とは結論できない。
低RAM機のメモリ不足、実描画中のVRAM圧力、driver固有障害を否定する試験でもない。画像形式は合成RGB PNG、ZIP方式はStoredに限定した。
失敗のまま残した回帰テストはない。全ライブラリ約1.5万件の実行や `test-full.ps1` は行っていない。

開発者向けの再実行例（新しいテストプロセスを1回だけ起動する）:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:MIV_655_CASE = 'rapid'
$env:MIV_655_SIZE = '1440'
cargo test -p mimageviewer --lib app::tests::ai_crash_655::ai_crash_655_headless_zip_navigation -j 1 -- --exact --ignored --nocapture
```

公開版側は `target/ai-crash-655/v490/` を作業ディレクトリにし、`CARGO_TARGET_DIR` をこの作業ツリーの絶対 `target` パスへ向けて同じテストを実行した。
実行した test exe は現行 `target/debug/deps/mimageviewer-528eefd81284cb36.exe`、公開版 `target/debug/deps/mimageviewer-877362fec51d7707.exe`。
ビルドログは現行 `target/ai-crash-655-build-final.log`、公開版 `target/ai-crash-655/v490-build-j4.log`。

## ソースで確認したこと

### 公開版と現行版の差

関数単位の抽出比較で、`prefetch_final_ai` / `maybe_start_final_ai` / `run_ai_worker` / `run_final_ai_job` / `has_uncancelled_final_ai_pending` / `cancel_or_orphan_final_ai_for_session` / `mark_cancel_or_orphan_final_ai_for_evict` / `cancel_final_ai_for_idx` は同一。
`src/ai/upscale.rs` と `src/ai/final_pipeline.rs` もファイル全体が同一（改行コードを正規化して比較）。
`poll_final_ai` の差は AI 完了後に取り消す後段 final-effect の reason を渡す部分。
eviction / session close にも同じ後段 final-effect の所有処理の差がある。開始・推論の仕組みそのものを依存更新後の挙動だけから公開版へ推定しない。
ORT 初期化は rc.15 の明示 Environment 所有と API 差への対応が入っている。

### 開始・取消・所有

- 現役は `maybe_start_final_ai` → `AiJobQueue` → 常駐 `final-ai-worker` → `run_final_ai_job` → `ai/final_pipeline` → `upscale_to_max_dim_with_progress`。
  main の `render_prepare_fullscreen_materials` と active viewer 更新から先読み・eviction を呼び、通常 selector から表示要求を作る。
- 旧 `prefetch_ai_upscale` は dead code。`maybe_start_ai_upscale` の製品側呼び出しはその旧先読み内のみ。旧 poll が残っていても、今回の主たる要求 producer と混同しない。
- final worker は1本。`AiRuntime::with_session` の Mutex は他の AI 利用者を含む各 `session.run()` を直列化する。ページを10枚送ったことと、GPU 推論が10本並列で動くことは異なる。
- Display は LIFO、Prefetch は FIFO。先読みは未取消 pending があれば新規要求を止める。表示要求は別ページの pending を全取消せず enqueue できる。
- ZIP は PDF retained orphan の例外対象ではない。keep-set 外では cancel を立てるが、live pending は結果回収まで残す。session close / page invalidation は pending を外して cancel を立てる。
- **取消要求は実終了ではない**。キューから出す前なら worker は `Cancelled` を返して推論を省略する。開始済みではモデルロード、倍率検出用の初回推論、実 `session.run()` はその途中で止まらない。upscale の取消確認は各タイルの前。
- job は source / runtime / manager の Arc と local activity lease を終端まで持つ。ページ・context の解放だけで推論入力や Session が先に解放される経路は見つかっていない。
- worker の Drop は join しないが、キューの共有 owner は worker 側にも残る。UI の取消を GPU の完了・資源返却と誤認してはいけない。

### 結果・世代・context

- FinalAiKey は item identity / items generation / source・編集世代 / 色・AI 設定 / 背景を含み、結果は key + job_id が一致する pending または追跡 orphan へだけ配送する。
- pending / live cache は viewer context とともに移譲され、queue / shared receiver / job sequence は App 側で持つ。退避 context 宛ては既存 backlog へ保留する。
- root の logic service に `service_cancelled_ai_terminals` があり、取消済み Ready を context/job に照合して回収する。表示 poll だけを見て「取り消した結果が常に復活する」と診断してはいけない。
- PDF orphan は最大1件で retained 層だけへ保存し、epoch が変われば破棄する。ZIP の終了原因の説明に PDF の完走例外を流用しない。
- open/switch/close/invalidate、Remote の drain と context Drop も確認対象とした。activity lease は pending の撤去後も実 worker の終端まで残る。

### メモリと上限

- キューは `VecDeque<AiJob>` で、件数・バイトのハード上限がない。取消済みの未着手 job も、worker が取り出すまで source Arc を保持する。LIFO の新要求が先行する間、古い取消済み入力を保持し続け得る。
  これは確認できる資源所有上の懸念であり、「30枚の ZIP で10枚先に行くと終了する原因」の確定とは異なる。
- source decode は既存 process-wide scheduler の6枠（Normal 最大4）を使用する。final AI の実行数1件とは別の上限。
- final AI は長辺8192以内へ直接累積する。出力 RGBA だけでなく、RGB + 重みの4本の f32 累積配列を確保する。モデル倍率検出は高速汎用でも512²の推論を1回行う。
- blender は1本、タイル channel は `sync_channel(2)`。ページ単位 job queue / shared result channel の上限とは別である。
- retained AI は既定10件・512MiB（PDF retained と合算）。live result と retained は同じ Arc を共有するため、その合計を単純加算すると二重計上する。
- live AI CPU cache は keep-set 管理。表示用の GPU texture 会計があっても、DirectML の Session・native GPU allocation や CPU の累積配列まで同じ予算へ収まることは意味しない。
- Session は ModelKind ごとに保持・再利用する。ページ移動ごとに他モデルの Session を排出する処理はない。この試験は高速汎用1種のみで、複数モデル／個別プリセットの利用履歴による累積は未確認。

理論量（4x、clamp 前に長辺8192以下の場合。モデルの native メモリ、タイル出力、入力コピー、表示 texture は別）:

| 入力 | source RGBA | f32累積4本 | 結果RGBA | 累積＋結果が同時にある量 |
| --- | ---: | ---: | ---: | ---: |
| 256×341 | 0.33MiB | 21.31MiB | 5.33MiB | 26.64MiB |
| 768×1024 | 3MiB | 192MiB | 48MiB | 240MiB |
| 1440×1920 | 10.55MiB | 675MiB | 168.75MiB | 843.75MiB |

### 終了の種類

通常の ORT error は `AiError` → `FinalAiResult::Failed` へ返る。通常の worker panic は channel 切断として UI 側に届く経路がある。
一方、native access violation / stack overflow / fast-fail / allocator abort は Rust の Result や通常の panic hook で捕捉できるとは限らない。
「エラーメッセージがなかった」という報告だけではどちらとも決められない。native exception log は first-chance 記録であり、存在自体は fatal の証明にならない。

DirectML の同一 Session の Run 直列化要件等は [Microsoft / ONNX Runtime の公式説明](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html#configuration-options) も参照した。
この参照から、今回の機器固有の device loss や OOM の発生を推定したものではない。

## 開発機の既存ログ（報告者の証拠ではない）

- 読取り時の `native-exception.log`: 0 bytes。
- `panic.log`: 最終更新 2026-10-19。末尾には viewer context assertion や native video pump stall の記録があり、今回の報告との同一性を示す証拠はない。
- Application / WER: 過去14日・ID1000/1001は94件を読取り、mIV関連は Susie の意図的 crash plugin の AV、v4.8.1 / v4.7.0 の RADAR_PRE_LEAK 等。今回の core + AI 終了の証明にはならない。
- 既存起動ログの wgpu adapter は RTX 4090、driver 32.0.16.1088。これはその既存起動の記録であり、新しいテストの DirectML EP が選んだ adapter の直接照会とは区別する。CIM によるハード情報照会はアクセス拒否だった。
- 利用者の通常 settings / DB / キャッシュは変更しない。インストール済みアプリも停止しない。

## 原因・次の判断

終了の原因は特定できていない。守るべき不変条件は、取消やページ移動後も native 実行中の入力・Session は実終了まで所有し、終端結果は要求元の key / job / 世代へだけ配送すること。
今回その寿命違反や別ページへの結果誤採用は確認していない。取消済みの未着手入力をキューが保持する点はコード上の改善候補だが、今回の終了原因と認定する証拠はない。
製品コードの修正はせず、実画面での process liveness / fault module / CPU・GPU メモリを次の切り分けにする。依存更新で直った、先読みが原因、という結論も出さない。

根本修正を検討する場合も、まず「未着手要求の入力所有」と「native 実行中の所有・会計」を分ける。
不要になった未着手要求が source を保持する境界を直す案は、AI・先読み機能を維持したまま検討できる。
ただし、この所有上の懸念を直せば報告されたプロセス終了も直るとは、fatal の証拠なしには結論しない。
実行中の資源を取消フラグだけで返却したり、症状を消すキャッシュ一括リセットを入れたりしない。

## 実画面を確認する場合の引継ぎ

ClaudeCode が利用者の明示承認を取得してから行う。今回の調査では実行しない。

- 素材: artifacts に保存した30枚 ZIP。まず 768×1024、必要なら1440×1920を使用。
- 環境: 公開版 v4.9.0 の使い捨て portable を準備し、data_dir がその portable の `data` であることを確認。通常設定をコピーしない。
- 条件: 単ページで高速汎用、既定先読み。cold session で AI 完了前にホイールで約10枚進め、現在 idx と実 job の対応を perf/通常ログから確認。
- 対照: 同じ ZIP で AI 完了待ち、AI なし。キャッシュ利用の往復は初回と区別する。表示モードの総当たりはしない。
- 記録: ウィンドウが消えた後のprocess生存（非表示／tray移行と終了を区別）、終了コード、native exception、WER の fault module / exception code、プロセス private bytes、GPU dedicated/shared usage。可能なら fatal dump を開発側で採取する。
- 所要: ビルド・準備を除いて約5〜10分。試験中はテスト用ウィンドウの前面化とホイール入力でデスクトップを使用する。
- 公開版で起きたときだけ同じ限定条件を現行版へ比較する。再現のために実利用データ・設定を変更しない。

報告者への最初の追加照会案は「使用版」と「該当画像1枚の縦×横」。前者は追う実装の版、後者は既定のAI処理対象か・どの寸法で試験するかを変える。
既存ログがあれば先にそこから読み取る。GPU・設定・終了ログが必要になった場合も診断資料1件へまとめ、追加操作の総当たりや「念のため再試験」を頼まない。

## 追加調査: GPU / VRAM と終了理由の表示（2026-10-29）

ClaudeCode からの追加依頼に基づくコード調査・設計案。**実装は未着手、v4.10.0には含めない。**
この追記では製品・デスクトップ・GPU負荷試験を起動していない。前節の20件の headless 成功を再利用するが、それらは描画 device / texture upload / present を通らないため、以下の描画側の故障を否定する試験ではない。

### 結論と確度

VRAM不足・GPU起因で、理由を画面に出さず終了する経路は**ある**。特に描画用 texture の確保失敗が wgpu の未捕捉 `OutOfMemory` になると、既定 handler の panic へ進む経路はコードで確認できる。
一方、通常の ORT error はジョブ失敗として返り、それだけで process を終了させる実装ではない。
**>>655 がこの経路だったという証拠はない。** VRAM不足、TDR、native driver障害は候補であり、現状は原因未特定のまま。

| 条件 | コードで確認した挙動 | >>655への確度 |
| --- | --- | --- |
| ORTが通常の失敗Statusを返す | Rustの `Result::Err` → `AiError` → `FinalAiResult::Failed`。該当keyを失敗扱いにして通常ログへ記録。現役final経路では利用者向け通知なし | この戻り値だけではprocess終了を説明しない |
| 描画texture等の未捕捉OOM / validation / internal error | wgpu既定handlerでpanic。UI/main経路で未回収ならprocess終了。panic hookはログのみ | 無表示終了が可能であることは確認。今回の発生は未確認 |
| `DXGI_ERROR_DEVICE_REMOVED` / RESET | HALは `DeviceError::Lost`。wgpuはdeviceを無効化し、device-lost callbackへ通知する別経路 | **Lostそのものが必ず既定panicへ行くわけではない**。後続surface再設定のpanic等はあり得るが、実際の終端は未確認 |
| native AV / stack overflow / heap corruption / fast-fail / allocator abort | Rust Resultとは別。panic hookを通る保証なし。VEH / WERも記録できない場合あり | 無表示終了は可能。報告者の証拠なし |

### 調査した版と差分

- 公開版基準: `v4.9.0` = `7fea5839dfe3f3685080ab6956bf1b4a83b57db1`。
- 今回の読取り対象: `ai-crash-655` の `4a729bc091b7809f5cbc58fb627ca5a9543e5562`（前節の調査差分をコミット済み）。読取り時の master は `45366aff56bcb811c1c57f0105fdf662f5c03047`。
- `src/gpu_info.rs`、`native_exception_log.rs`、`run_native_failure.rs`、`gpu_startup_diagnostics.rs`、`feedback.rs`、`crates/startup-diagnostics/`、patched `vendor/eframe/` と `vendor/egui-wgpu/` は v4.9.0 と調査HEADで差分なし。`src/lib.rs` の差分は別機能の公開fixture/moduleだけで、panic hook / GPU設定 / 終了処理に差分なし。これらの対象は調査HEADと上記masterでも同じ。
- 両版とも `eframe / egui-wgpu 0.37.0`（vendor patch）、`wgpu / wgpu-core / wgpu-hal 30.0.1`、`winit 0.30.13`。registry内の該当版sourceまで追跡した。
- AI製品側の `upscale.rs` / `final_pipeline.rs` は前節どおり同じ。依存は ORT 1.24.2 + ort rc.12（patch）から ORT 1.24.4 + ort rc.15へ変更。今回追加で確認した上流ORT実装は **v1.24.2 source**。1.24.4のnative内部実装が完全に同じだとは主張しない。現行Rust wrapperの `session/run.rs` も失敗Statusを `?` で返す。

### 推論側: 通常エラーとnative終了を分ける

上流ORT v1.24.2の `ExecutionProvider.cpp` は HRESULT をthrow/checkし、COM入口で `ORT_CATCH_RETURN` を使用する。`InferenceSession::Run` は通常のC++例外を失敗Statusに変換し、C APIの `OrtApis::Run` は `ToOrtStatus(status)` を返す。
したがって、割当失敗がこの通常のエラー伝播を通る場合は、mIVが受け取れるエラーになる。あらゆるOOM、非同期GPU故障、driver内SEH、abortが必ずStatusへ変換される保証ではない。
根拠: [ORT DML provider v1.24.2](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/providers/dml/DmlExecutionProvider/src/ExecutionProvider.cpp)、[InferenceSession::Run](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/session/inference_session.cc#L2836)、[OrtApis::Run](https://github.com/microsoft/onnxruntime/blob/v1.24.2/onnxruntime/core/session/onnxruntime_c_api.cc#L785)。

mIV側の経路は `src/ai/upscale.rs::run_tile_inference` / `detect_scale_factor` の `session.run(...).map_err(...)` → `final_pipeline::execute_selected_final_ai` → `src/app.rs::run_final_ai_job` → `poll_final_ai` の Failed分岐。
Failed分岐はlive pendingなら失敗key登録とログ・repaint、orphanならログのみ。ここに終了要求、panic、MessageBox、feedback通知はない。EP登録失敗時の既存CPU fallbackは `runtime.rs::register_directml_ep` の別段階で、推論途中のOOMを一般に救うものではない。

DirectML公式仕様は、OOMもdevice-removalの要因に挙げる。DML deviceだけの喪失がD3D12 device喪失を伴うとは限らず、その逆向きは伝播する。このため「AIのDMLエラーが出たら必ず描画deviceも死ぬ」とは扱わない。
[Microsoft: DirectMLのエラーとdevice-removal](https://learn.microsoft.com/en-us/windows/ai/directml/dml-errors)。
同じGPUでも、mIVのORT default DirectML EPとeframeのwgpu deviceは別の所有者である。両者が同じadapterかは、GPU名の推定ではなくLUID等の一致を採らない限り未確認とする。

### 描画側: OOM panicとdevice lossは別経路

1. `vendor/egui-wgpu/src/renderer.rs::update_texture` は新規画像textureを `device.create_texture` で確保する。製品描画経路にはこの割当を囲むOOM error scopeがない。
2. `wgpu-core 30.0.1/src/device/resource.rs::create_texture_inner` のHAL割当失敗は `handle_hal_error_with_nonfatal_oom` で戻る。ここでの「nonfatal」はcore内のdevice無効化を直ちには行わない意味で、mIVが通知して継続するという意味ではない。
3. `wgpu 30.0.1/src/backend/wgpu_core.rs::ErrorSinkRaw` はscopeもcustom handlerもなければ `default_error_handler` を呼び、`panic!("wgpu error: ...")`。OOM以外のValidation / Internalも対象。
4. mIV / vendorの通常描画deviceについて `on_uncaptured_error` の差替えは見つからない。error scopeの検索結果はvendorテストのみ。動画native overlay用の別deviceにだけ `set_device_lost_callback` があり、generation付きlatchを立てるがreason/messageは保持していない (`src/video/native_presenter/overlay_gpu.rs`)。
5. DX12 HALの `auxil/dxgi/result.rs` は `E_OUTOFMEMORY` → OutOfMemory、DEVICE_REMOVED / RESET → Lost。`cargo tree -e features -i wgpu-hal --offline` で `device_lost_panic` / `internal_error_panic` は有効でないことも確認した（compile・製品起動なし）。
6. **LostはOOM panicと別**。wgpu backendの `handle_error_inner` は `ErrorType::DeviceLost` をuncaptured handlerへ渡さずreturnする。coreの `Device::lose` はvalid=falseにして登録済みclosureを呼ぶが、初期値はNone。mIVのroot描画deviceにはclosureがない。この入口だけから「TDR→必ずpanic」とは結論できない。
7. surface acquireのOutdatedは再設定、Lostはsurface再作成要求、Occluded等はskip。これは**surface**の扱いで、device再作成ではない。patched `winit.rs::configure_surface` / runtime installには `expect("runtime surface configuration is fatal")` が残る。device喪失後に再設定失敗・別エラーへ進めばpanic等になり得るが、単に描画が止まる経路も含め、今回の機器での結果は未観測。
8. 初回rootの非対応surfaceや `run_native` が返した `Err` は、既存 `run_native_failure::finish_run_native` によりログ保存・終了処理後にnative MessageBoxを出す（script時は抑止）。これはpanic / native例外を包む処理ではない。

wgpuには予算比率によるresource OOM / device lossの閾値指定もあるが、mIVのinstance設定は `MemoryBudgetThresholds::default()`（両方None）で、独自の固定閾値で喪失させてはいない。OS/native側からの割当失敗・喪失までなくなるわけではない。

Cargo profiles / `.cargo/config.toml` / release scriptsには `panic=abort` 指定がなく、通常Rust panicはunwind。
winit Windowsはcallbackのpanicをcatchして保留し、event loop境界で `resume_unwind` する。mainの `mimageviewer::run()` / `eframe::run_native` を包む最外のcatchはないため、UI側のpanicはmainまで抜けて終了し得る。**常にabort設定だから落ちる、ではない。** 二重panicやunwind不可境界、native異常は別にabortし得る。
AI worker等の背景threadだけの通常panicなら、hookが動いても直ちにprocess全体が死ぬとは限らない。従ってpanic.logの有無も終了の確定印にはできない。
なおCPU側も `upscale.rs` の累積配列4本は `vec![0.0f32; npixels]` で確保し、`try_reserve` 等による失敗Resultへの変換はない。Rust標準allocatorの割当失敗によるabortは通常panicと別で、panic.logが残らない可能性がある。VRAMとprocess private bytesの両方を記録する理由になる。

`src/lib.rs::install_panic_log_hook` はpayload・位置・強制backtraceを構築し、`panic.log` へ直接appendしてから通常loggerへ渡す。panic.logは4MiBで1世代rotation。hook内の画面通知はない。windows subsystemでstderrも通常見えない。
この既存hookには割当・filesystem処理があるため、極端なOOM時やstack破損時まで保存できる保証はない。新しいfatal証拠はこの重い処理**より前**に短く残す設計とする。

### 「砂時計→消失」とTDRの位置付け

**推測として症状に合う経路**は、同じadapterを使うAI負荷やメモリpagingにより、UI threadのsurface acquire / queue submit / present / texture割当などのnative callが待たされ、続いてOOM panicやdevice loss後の別panic、またはnative障害で終了する経路。
`egui-wgpu::Painter::paint_and_update_textures` はUI描画中にacquire / submit / presentを行い、DX12 HALにもswapchain待機、fence待機、`Present` があるため、AIがworker上でもUIがGPU待ちになる可能性はある。OSの「応答なし」表示に相当するか、実際にどのcallで待ったかは未確認。
CPUメモリ圧迫や別の同期停止も似た見え方になるので、砂時計だけをTDRの証拠にはしない。

TDR既定2秒は、schedulerがGPU taskを完了またはpreemptできるまでのtimeoutである。**`Session::run` の経過時間やタイル全体のCPU wall timeと同じではない。**
弱いGPU・driverのpreemption特性・長いdispatchによってAI由来のTDRが起こる可能性はあるが、タイルが2秒以上かかった事実だけでは判定できない。
[Microsoft: WDDM TDR](https://learn.microsoft.com/windows-hardware/drivers/display/timeout-detection-and-recovery)。
高速汎用は `model_tile_size` で512²、初回の倍率検出も512²の実推論。通常タイルはoverlap32、画像端も固定入力へpaddingする。取消はタイル境界であり実行中のRunを中断しない。
従って「取消要求済みなのにGPU処理継続」は正常にあり得る。画像寸法が大きいと主にタイル数・CPU累積配列が増えるので、画像寸法をタイルdispatchの巨大化と同一視しない。
TDRが起こった、OOMが起こった、同じGPUだった、の3点はいずれも報告者について未確認。TdrDelayの変更や機能削減は提案しない。

### 既存診断の記録条件と不足

| 仕組み | 現在取れるもの・時点 | 不足 / 注意 |
| --- | --- | --- |
| `gpu_info::emit_vram_trace[_with_options]` | **perf有効時だけ**、呼出点でDXGI local/nonlocal Usage / Budget / Reservation等。任意でPDH Dedicated / Sharedも取得、失敗は一度ログ | 現在の製品callsiteはvideoのdecode/drop/render経路。静止画final AIの連続ページ移動に常時sampleはない。PDHは比較的重い。DXGIも列挙・COMを含みfatal内では不可 |
| `query_dxgi_video_memory_snapshot` | 最初のsoftware以外・専用VRAM>0のadapter、node0を照会 | wgpu/ORTの選択adapterと照合しない。専用VRAM0のUMAを除外し得る。全GPU合計でもAI専用使用量でもない |
| `native-exception.log` | data_dir初期化・single-instance受付後に事前open、VEHでAV/stack overflow/breakpoint/illegal instruction等7種類。時刻、thread、code、address、AVアクセス種別、version、exe/ntdll base。1行2048bytes、1session256件、起動時1MiB超でrotation | first-chanceで `CONTINUE_SEARCH`、捕捉済み例外も含む。HRESULTのOOM / DEVICE_REMOVEDはnative例外とは限らず通常ここに出ない。fast-failはVEHを通らない場合あり。job/VRAM/device reasonなし |
| `panic.log` | hookが呼ばれたRust panicの要約・位置・backtraceを常時直接append | UI通知なし。threadの帰結、run idとの強い相関、GPU予算/対象jobなし。背景thread panicや捕捉されたpanicも記録 |
| 通常log / perf | 通常logは常時、AIの要求・実行・cancel境界やFailedの文字列。perfはopt-inで詳細時系列、writerは非同期・有界 | crash直前のqueue未排出分は失われ得る。通常logにはパスを含み得る。perfの全件提出は利用者負担が大きい |
| startup diagnostics / GPU startup capture | 常時の起動timeline、run/role/pid、adapter選択、初回root描画段階、process.terminal。GPU依存Errorの限定捕捉はInstance〜最初のRootPresentedまで | 起動完了後のGPU失敗は対象外。`ProductDiagnostics` は通常runtimeのwgpu HAL/core Errorを保存しない。device removalの詳細HRESULTを一般に保持しない |
| UI heartbeat watchdog | 5秒以上App updateがない疑いを通常ログへ記録 | 待っているGPU callを確定しない。killもしない。logger停止やprocess即死では残らない場合あり |

`CurrentUsage > Budget` は圧迫・pagingの手掛かりで、即OOMの証明ではない。予算は変動し、sampling間のpeakも見逃す。
[Microsoft: DXGI_QUERY_VIDEO_MEMORY_INFO](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info)。
逆に直前sampleが予算内でもnative一時割当失敗を否定しない。VRAM数値だけで「VRAM不足で終了しました」と表示しない。

### 設計案の比較と推奨範囲

**推奨は(a)+(c)、および(b)のうち既にResultとして返るORT失敗の通知。**
目的は理由を取り出せるようにする診断であり、device再作成、再推論、retry、キャッシュ一括破棄、AI無効化の回復処理は含めない。

| 案 | 利点 | 制約・規模 |
| --- | --- | --- |
| (a) 次回起動の前回終了通知＋コピー | 落ちたUIに依存せず、利用者は短文を貼るだけ。常時有効なら事前の再現設定が不要 | 中規模。終了境界、前回資料の読取り、privacy-safe要約、表示とコピー、単体/子process試験 |
| (b) 受け取れた失敗を即時通知 | ORT Failedは既存feedbackを使い小規模で説明可能 | wgpu OOM / device lossを一般に握りつぶして描画継続する案は不採用。失敗textureは利用不可で、device無効の場合もある。描画復旧は別の大きな設計 |
| (c) 軽い直前状態＋fatal時保存 | cancel済みRun継続、現在ページと実job、VRAM圧迫を区別できる | 中規模。(a)と共通基盤。fatalだけのdumpではfast-fail/kill/電断を捕捉できないため、通常時の非同期保存も必要 |

#### (a) 終了印と表示の責務

- launcher / worker / help / 二重起動activationを除く、**GUI coreの実行単位**で1つの診断ownerを持つ。既存startupのrun ID（launcher共有相関がある場合はcore role/pid/開始時刻も含む）へ結び付ける。正常設定DBへboolを追加しない。
- single-instanceのfirst admissionとdata_dir確定後、起動workerが前回の小資料を読み、上限・schema・runを検査してメモリへ渡す。UIはpollして表示するだけ。通常ログのrotationや今回の記録で前回資料を上書きする前に前回1件を確保する。同じprocessが生存するleaseは異常終了に数えず、前回版に資料がない初回は表示しない。
- 状態は例として `Running / ReturnedOk / ReturnedError` を1つの型で保持し、run id・version・開始時刻・最終更新時刻を付ける。`ReturnedError` は制御されたエラー終了で、即時通知済みなら次回同じ警告を重ねない。
- cleanの確定はmainの明示した最終終了境界でのみ行う。ウィンドウ非表示、tray移行、App Drop、unwind中のRAII Dropではcleanにしない。既存 `finish_run_native` / `main` / `exit_startup` の実際の早期return・終了コードを棚卸しする。正常な終了が実行中AIをdetachして終える既存仕様も異常と扱わず、AI完了待ちを新設しない。
- `--play-test` は `src/app.rs` から直接 `process::exit(0)` しmain境界を通らない。これを含む試験modeは診断対象外と明示するか、意図した試験終了として同じownerへ通知する。GUI roleを持つという理由だけで対象に含めない。
- `miv_startup::terminal` はfirst-outcomeを守るが、`shutdown_until` の既定Cancelledや別roleのterminalもあり、単なるterminal存在を正常終了印に流用しない。通常終了印の書込み失敗もあり得る。最終保存は既存の終了時deadline内でworkerへ依頼し、UI frame内やfatal内でbarrier待ちをしない。記録失敗を回復する待機・再試行は作らない。
- 前回Runningのままなら「**前回の正常終了を確認できませんでした。原因は記録されていません**」が基準。taskkill、OS終了、電源断、診断保存失敗とcrashを区別できない。対応するpanic / device loss / first-chance例外があれば「終了前に観測された情報」として添える。確実な終端証拠なしにfirst-chance例外やworker panicを終了原因へ昇格しない。
- UI表示は初回の通常root描画が可能になってから、消えない小さな通知・詳細欄に「報告用の文をコピー」「閉じる」を置く。起動中の操作overlayを塞がない。コピーは明示操作時のみ、送信は自動化しない。直前資料は前回1件だけを保持し、表示済み/閉じるの扱いも同じownerへ集約する。短時間のtoastだけでコピー機会を失わせない。

表示・コピーの例（数値・IDは架空、全項目必須ではない）:

```text
前回の正常終了を確認できませんでした。
終了前の記録: 描画用GPUメモリの確保に失敗 / Rust panic
mIV x.y.z (build ...), Windows ..., ORT ..., GPU ... / API DX12
直前GPU使用量 1700 MiB / 予算 1900 MiB（採取0.8秒前・描画adapter）
表示11/30, 実行job42: 6/30, 高速汎用, 入力1440x1920, tile512x512
job42: 取消要求済み・Run終了未確認。記録ID ...
※使用量の値だけではメモリ不足を確定できません。
```

既存 [startup diagnostics](startup-diagnostics-plan.md) のrun・時刻・role・非blocking publisher / writer・保存先解決・保持方針を再利用する。通常版の起動診断保存先はLOCALAPPDATA、portable/明示data-dirはそのdata/logs/startup配下なので、診断のためにUIへAPPDATAの同期I/Oを増やさない。
既存の [run_native Err通知](render-init-failure-report-plan.md) と二重通知を避け、次回起動も描画できない場合は既存nativeエラー表示までが限界。crash中のMessageBoxを保証する設計にはしない。

#### (c) 記録内容とfatal経路

- 通常時は固定容量の直近イベント（例32件、文字列を含め全体16KiB程度を上限）と最新snapshotを保持。現在表示のcontext/page/items世代と、queue要求・worker pick・実Run開始/終了・取消要求・実terminalを分ける。要求job ID、model/backend、source/output/tile寸法、tile番号、in-flight件数、queue件数・所有入力bytes、CPU private bytesも対象を絞って採る。未計測値は0で偽装せずunknown。
- UIからは既にある整数・enum等をメモリへpublishするだけ。画像を再decodeせず、queue/session lockを計測のために待たず、文字列のパス整形やCOM照会をしない。worker側のRun直前/直後で状態を刻むため、UI上のpending撤去をGPU完了と誤認しない。初回model load / scale detect / tile Runも区別する。
- GPU memory samplerは背景workerで、AI活動中など必要な期間に約1秒間隔から開始。PDHの毎回open/enumerationは常時経路に転用せず、まず選択adapter/nodeに結び付いたDXGI Usage/Budget（local/nonlocal）、採取時刻、照会成否を取る。LUID取得不能・DMLのadapter未確認・UMAでも欠落を明記し、別GPUの数値をAIのVRAMと呼ばない。sampleの年齢をコピー文に含める。待機はworker通知付きとし、新しいUI repaintやUI sleepを作らない。
- 小資料の保存もworker所有。dirty時の周期checkpointと主要境界をcoalesceして上限付きstateを置換し、前回1件＋今回1件だけ保持する。Disk停止でもproducerを止めず、記録失敗はdiagnostic-unavailableとする。通常ログqueueに入っただけのイベントを「保存済み」と表示しない。詳しいperfは従来の任意診断として残す。
- root device生成直後にdevice-lost callbackを登録し、reason/messageを有界・path非含有の情報へ変換してpublishする。明示destroyを異常に数えない。callbackはwgpu再入・resource解放・再作成・UI呼出しをしない。uncaptured handlerを診断用に置く場合も、既定のfatal契約を無条件の継続へ変えず、記録して同じpanicに進む範囲から始める。
- wgpu callbackの `DeviceLostReason::Unknown` と一般messageだけでは、nativeな `DXGI_ERROR_DEVICE_HUNG` 等は取れない場合がある。詳細が必要なら生きている各device所有境界で `GetDeviceRemovedReason` を採る設計を別途限定する。wgpuはHAL公開範囲/vendor境界の確認が必要、ORT内部DML deviceは現在mIVが直接所有しない。**別deviceへの照会を代用したり、常に詳細理由が取れると約束しない。** [D3D12のGetDeviceRemovedReason](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device-getdeviceremovedreason)。
- panic / native handlerは上記の完成済みsnapshotと読取り可能なring要素を、小さな事前open済み別sinkへbest effortで保存する。既存 `native_exception_log` の固定buffer、再入時skip、直接 `WriteFile`、件数上限を再利用する。一般loggerを呼ばず、alloc / Rust lock / TLS / COM / DXGI / backtrace / thread待機 / flush待機を足さない。既存のfirst-chance方針と `CONTINUE_SEARCH` を維持する。
- producer停止中でもfatal readerが待たずに読めるpublication契約が必要。非atomic payloadをseqlockの検査だけで並行readする方式はRustのdata raceを避けられないので不採用。固定primitive atomicと世代確認、読めないslotはskip等の上限付き方式を独立レビューする。ringの完全取得を優先してhandlerを待たせない。
- job ID・寸法・段階は同じpublication単位で照合し、異なるthreadの更新を継ぎ合わせない。単純な二重buffer切替も読取り中bufferの再利用問題があるため、それだけでは採用しない。一度の整合性検査が失敗した項目はunknownとし、fatal handlerで読めるまでspinしない。
- fatal追記と通常state保存は別sink/runで相関し、遅れたcheckpointがfatal証拠を消さないようにする。panic記録は「観測」であり自動的なcrash印ではない。fast-fail / TerminateProcess / 電源断ではhandlerが走らないので、前回checkpointまでしかない場合を正常に扱う。これを埋める監視process・dump強制採取・WER検索の自動実行は初期案に含めない。

#### (b) 捕まえられた失敗の通知範囲

ORT Failedの詳細を安全な短文へ変換し、現役jobの所有contextへ既存 `show_feedback_result_on(..., FeedbackKind::Error)` 等で通知する案は小規模。背景prefetchや古い取消jobの失敗を現在ページの失敗として出さず、同一失敗を連打しない既存key寿命へ結び付ける。再試行やAI設定変更は追加しない。短い通知から同じ診断詳細・コピーパネルを開ける形にすると資料を再要求せず済む。

wgpuにcustom handlerを付けるだけでpanicを消す案は採らない。scopeでOOMを受け取れても、eguiのtexture登録・delta適用・bind group・後続描画に不完全な資源を渡さない所有境界が必要になる。
device loss後に同じdeviceで通知UIを描ける保証もない。安全な制御終了へ変えるなら、eframe/vendorの描画中断と終了境界を先に設計し、次回通知へ渡す別作業とする。device再構築・自動API切替・CPU再推論を診断追加へ混ぜない。

#### 個人情報・規模・検証

コピー文と常時の小資料は、**ファイル名、ZIP内名、絶対パス、検索語を保存しない**。ページ番号・総数・寸法・model・短命context/job/run IDで相関する。ORT error / panic payload / driver messageはパス等を含み得るため、生文字列を無条件にコピーせず、分類・HRESULT・安全な要約・長さ上限を適用する。panic source位置もbasename程度に留め、識別できない場合は一般文とする。既存詳細ログはローカルに残るが、自動添付・送信しない。GPU型番・driver/version・メモリ値を含むコピー内容は押す前に見えるようにする。

規模の見積りは、(a)+(c)で**中規模・数日程度の実装と独立レビューを要する別作業**。startup診断の拡張、実行owner/終了印、AI publish、GPU sampler、主device callback/vendor接点、fatal-safe snapshot、UI詳細・コピーとテストが対象。native詳細HRESULTまで確実に取る部分は別段階で見積もる。(b)のORT通知だけは小規模だが、単独ではprocessが消える理由を取れない。

将来の自動検証はすべて一時data_dirで行い、正常return / 制御されたErr / second-instance / worker / tray非終了 / worker panic / main panic / 強制終了 / stale・破損資料 / fatalとcheckpointの競合 / adapter不一致 / sample欠落・古さ / privacyを重点化する。panic/nativeは同じtest exeの限定子processと注入したformatter/sinkで検証し、本物のGPU OOM・TDRを自動で起こさない。UI文・コピーはsnapshot/純粋テストで確認する。
今回の追記は文書のみなのでbuild-devは作成せず、既存headless試験も再実行しない。実装承認や実画面再現の承認をこの設計調査から推定しない。報告者へは現段階で設定変更や追加の総当たり試験を頼まず、診断を導入できた版で自然に再発した際に「画面の報告文をコピーして貼る」1操作で資料を得られる構成を目指す。

限定独立レビュー（`gpt-6.1-sol` / `xhigh`）でLostとpanicの区別、終了印のGUI process所有、fatal-safe記録順序を再確認した。指摘された `--play-test` の直接終了とsnapshotの複数thread整合性を上記へ反映。製品変更はなく、文書の `git diff --check` が成功。
