# §1.776 案 B-B 実装記録（2026-11-18）

対象は `perf-801-776b`、基点 `9a008b5ca` と本件の未コミット差分。コミットと製品実行は行わない。
正本は notes 作業ツリーの `design-776b-prebuild.md` 第3版と、設計レビュー第1回 REVISE・第2回 ACCEPT。
設計の変更は行っていない。

## 変更と設計の対応

| 設計 | 実装 |
| --- | --- |
| §2.1 device 単位の単一所有者 | `src/scaler_pipeline_store.rs` の `ScalerPipelineStore` を App が1つ所有。process static と context park / swap への追加はない |
| 種類ごとの作成状態 | `PipelineCell<T>` の `OnceLock<Option<Arc<T>>>`。起動の重複抑止用 Once と起動時判定用 Once は別の責務。builder は store に再入しない |
| context の単一 slot | `GpuLanczosCache::anime_resampler` / `nis` が Option<Arc>。初回だけ get_or_insert_with し、共有できなければ直接作成。prepare と encode は同じ Arc |
| 不変資源の分離 | `NisPipelineSet` に bgl・pipeline layout・pipeline を集約。NIS 専用 prepare helper はその bgl を使用。その他の Lanczos3 枝は従来どおり context 所有 |
| §2.2 開始契機 | root の `normal_presented()` 後の1か所。global preset・全 slot・adjustment_favorite_params のメモリ値を一度だけ判定。対象なしでも消費し、後の設定変更・ページ DB・timer から request しない |
| §2.3 終端 | 完成は Arc、作成中は残りを待つ、未着手は直接作成、None は context で直接作成、worker panic は未初期化。設定変更では保持し、device 不一致はログと直接作成。背景 retry / cancel / 終了 join はない |
| §2.4 worker | 名前付き低優先度 thread、device clone。renderer / App lock を取得しない。Validation・Internal・OutOfMemory を push、逆順の全 pop 後に集約、失敗は None とログ1行 |
| §2.5 計装 | gpu/scaler_prebuild の begin/end・trigger・ms・result と、既存計測の pipeline_source / pipeline_wait_ms。待ちは作成時間の内数 |
| §2.6 回帰 | fake builder の状態遷移、起動契機の純粋関数・一度だけの消費、scope の Internal 注入と全 pop、実 GPU の共有・drop・失敗 slot・device 不一致・全画素一致、ignored の queue 競合計測 |
| §3 detached の構造合意 | 実装前に `docs/detached-rework-plan.md` §11 へ設計担当と独立レビューの合意・変更範囲を記録。context 固有の出力資源は移さない |

既存の perf JSONL では `kind` が event 名の予約欄であるため、scaler の種類は `scaler_kind` に格納する。
作成元が prebuilt / waited のときは過去の creation breakdown を転載しない。
Anime4K の取得は resampler_create_ms、NIS は pipeline_create_ms に含め、wait_ms を二重加算しない。
案 B-A の build.rs・DXBC / WGSL 分岐・device feature と §1.801 の計装は維持する。
生成条件と wgpu-hal の照合根拠は [B-A 実装記録](section776ba-implementation.md) を引き継ぐ。

呼出し側は `src/lib.rs` の render-state 初期化、`src/app.rs` の App 所有と起動契機、
`src/ui_fullscreen.rs` の通常 resolve、`src/app/final_prepare_budget.rs` の既存 prewarm を更新した。
store は引数で渡し、`resolve → resolve_key → generate` と prewarm から同じ builder へ合流する。
既存 prewarm / canonical-demand / page-turn ROI のテストは store 不在を明示する引数へ追従した。
GPU テストは `src/gpu_lanczos/prebuild_gpu_tests.rs`、状態遷移は store 内、計装回帰は
`src/gpu_lanczos/measurement_tests.rs` に置いた。

文書は display-pipeline §2.4.1、async-architecture、ui-responsiveness、next-release-backlog §1.776 と本記録を更新する。

## 独立レビュー

Sol / xhigh の独立ソースレビューは ACCEPT。初回に見つかったテスト引数重複2か所と、
既存 measure_776 NIS probe の作成内訳欠落を修正した。追加 GPU テスト・競合計測の再レビューも ACCEPT。
追加した CPU 計装テストでは合計時間の自己参照を指摘され、合計14msを独立固定して待ち7msの
二重加算を検出する形へ修正し、再確認 ACCEPT。P1 / P2 / P3 の残件はない。
実行検証の担当は親の実装担当で、レビュー担当による重複実行はしない。

## 実行検証

全重処理は次の runner、Owner=perf801-codex、WorkDir=本作業ツリーを通す。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "perf801-codex" -WorkDir G:\home\mimageviewer-perf801 -Command "<下記コマンド>"
```

| コマンド / 条件 | 件数・結果 | ログ |
| --- | --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core --offline` | 最初は resolve_key の store 引数漏れ E0425 を検出。修正後 exit 0 | `target/section776bb-check.log`、`target/section776bb-check-r2.log` |
| 下記の焦点 nextest、dev-runtime、ignore 除外 | **88件実行、88成功、0失敗**。実行28.470秒 | `target/section776bb/Tests-1.log` |
| `measure_776b_anime4k_ui_queue_overlap`、同 dev-runtime、ignore only、3回別プロセス | **1件 × 3回すべて成功**、各 exit 0 | `target/section776bb/Measure-{1,2,3}.log`、`overlap-results.json` |
| `cargo fmt`（引数なし）、`cargo fmt --check`、`git diff --check` | 最終実行成功。テスト時点の source hash 12ファイルも不変 | 端末出力、`target/section776bb/source-hashes.json` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | **exit 0**。通常 feature の core + companion 4本作成。PE 検査 runtime=4 / pe=5 成功。起動なし | `target/section776bb/build-dev.log`、`target/vcrt-pe-reports/dev-runtime.json` |

焦点テストは次のコマンド。GPU 計測は同じ dev-runtime バイナリで、filter を
`test(/measure_776b_anime4k_ui_queue_overlap/)`、`--run-ignored only --test-threads 1` に替え、3回別プロセスで実行する。

```powershell
cargo nextest run -p mimageviewer --lib --offline --cargo-profile dev-runtime -E 'test(/^gpu_lanczos::/) | test(/^gpu_anime4k::tests::/) | test(/^scaler_pipeline_store::tests::/) | test(=app::tests::history_transition_storage_keeps_app_stack_footprint_bounded)' --run-ignored default --retries 0 --no-tests fail --no-capture
```

実行は `target/run-776bb-check-tests-measure.ps1` / `target/verify-776bb.ps1` から直列化し、
`CMAKE_BUILD_PARALLEL_LEVEL=1` / `MSBUILDDISABLENODEREUSE=1` を設定した。
FFmpeg DLL を vendor/ffmpeg/bin から dev-runtime/deps へコピーして test harness の依存を満たした。
新規 GPU テストは gpu_tests 配下のため nextest の gpu group（最大2）に所属する。
`--no-capture` により今回の焦点テストと計測のプロセス実行自体は直列である。
本体 lib 全件は設計担当の担当範囲で、ここでは実行しない。

新規の常設 GPU 3件はすべて成功。背景作成と直接作成の比較は NIS / Anime4K の各3条件、
**6比較・53,002画素対の RGBA8 が完全一致**した。加えて既存 B-A の WGSL / DXBC 一致2件も成功。
CPU の store 状態遷移10件と、追加した待ち時間二重計上の計装テストも含む。
通常 GPU テストは VALIDATION あり、下記計測は DEBUG / VALIDATION なしである。

## GPU 競合計測

観測者: **実装担当の GPU テスト**。
`measure_776b_anime4k_ui_queue_overlap` は DX12 + FXC + PASSTHROUGH_SHADERS、
dev-runtime、DEBUG / VALIDATION なしで、背景 Anime4K 作成中に UI 相当の thread で
write_buffer・write_texture・submit・poll(Wait) を実行する。

開始の rendezvous は計時区間外。背景 thread の実際の store.get_anime4k の begin/end と
各 API の begin/end を照合し、各 API が作成区間内に収まる呼出しを少なくとも1件要求する。
背景作成の終了をまたぐ API 呼出しも、その呼出し全体の時間を中央値・最大の対象とする。
計時区間は error scope を含む store.get_anime4k 全体であり、個々の PSO API との重なりは直接計測しない。
各値は API 呼出し全体の壁時計時間で、純粋な lock 待ちだけを分離した値ではない。
surface present、実アプリの frame time、全 GPU / driver の待ちを保証する試験ではない。

実行環境: NVIDIA GeForce RTX 4090、driver **32.0.16.2101**、DX12 + FXC + 埋め込み DXBC、
PASSTHROUGH_SHADERS=true。flags は `InstanceFlags(VALIDATION_INDIRECT_CALL)` で DEBUG / VALIDATION bit はない。
run-heavy によりプロセス優先度は BelowNormal、worker はさらに既存 helper で thread 優先度を下げた。
PC 全体の idle と driver cache 消去は保証していない。

| 回 | 背景作成 ms | write_buffer 中央 / 最大 ms | write_texture 中央 / 最大 ms | submit 中央 / 最大 ms | poll 中央 / 最大 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 581.971 | 0.0195 / 0.2925 | 0.0104 / 0.2948 | 0.0352 / 0.4649 | 0.0751 / 3.0168 |
| 2 | 774.246 | 0.0196 / 0.2461 | 0.0109 / 0.0831 | 0.0350 / 0.3002 | 0.0732 / 0.9186 |
| 3 | 747.264 | 0.0390 / 0.3986 | 0.0160 / 0.1194 | 0.0530 / 0.3218 | 0.0875 / 0.9316 |

背景作成時間の3回中央値は **747.264ms**。API 中央値の3回中央値は write_buffer 0.0196ms、
write_texture 0.0109ms、submit 0.0352ms、poll 0.0751ms。
全3回での最大は同順に **0.3986 / 0.2948 / 0.4649 / 3.0168ms**。
この条件では背景作成の数百 ms が queue API の長い停止として現れることは観測しなかった。

重なりの証拠（時刻は各回の origin からの ms）:

| 回 | worker 区間 | 交差した API 呼出しの時刻範囲 | 完全に作成区間内の呼出し数（write_buffer / write_texture / submit / poll） | 交差する呼出し数（同順） |
| --- | --- | --- | --- | --- |
| 1 | 0.3580–582.3286 | 0.3598–582.3286（581.9688ms） | 2798 / 2798 / 2798 / 2798 | 2799 / 2798 / 2798 / 2798 |
| 2 | 0.5800–774.8257 | 0.5832–774.8257（774.2425ms） | 3721 / 3721 / 3720 / 3720 | 3721 / 3721 / 3721 / 3720 |
| 3 | 0.3999–747.6639 | 0.3999–747.6639（747.2640ms） | 2943 / 2944 / 2943 / 2943 | 2944 / 2944 / 2944 / 2943 |

各 API が実際の作成区間内に入ることを assert した。区間端をまたぐ呼出しも除外していない。
時刻範囲は最初と最後の交差時刻の幅であり、API が連続して実行された時間の総和ではない。

## 危険と更新時の確認

- 作成中の全画面は、利用者が決めたとおり残りを同期的に待つ。起動後に初めて選んだ種類・ページだけの設定では先行作成の効果がない。
- pipeline は設定変更と context の close をまたいで store に残る。出力 texture の cache / VRAM 計上とは分離される。
- 低優先度 worker でも driver 内の同期をなくす保証はない。今回の headless queue 実測を実アプリの滑らかさの証明としない。
- wgpu 更新時は B-A の naga / HAL / binding / FXC / passthrough 契約に加え、error scope のスレッド所属・全 pop、共有 layout と prepare / encode の Arc 一致を検査し、常設 GPU 回帰と競合計測を再実行する。
- device lost 後の復旧は既存どおり保証しない。異なる device に元の pipeline を渡さず、ログ後にその context で作る。

## 実アプリ確認の引き継ぎ

build-dev は **2026-11-18 05:31:49 JST に正常終了**した。製品バイナリは起動していない。
core は `target/dev-runtime/mimageviewer-core.exe`、398,460,928 bytes、更新日時05:31:17 JST。
SHA-256 は `249AB69FBB1C0144054870860EC891908DFC566B4B465139D073E4E107F5DF5A`。
最終ソース12ファイルの hash はテスト時点から不変。成果物の対応は `target/section776bb/verification-result.json` にも保存した。

利用者がインストール版・トレイ常駐版を終了してから実行する。
単一インスタンス mutex は共通で、通常の `%APPDATA%\mimageviewer` の実設定・データを更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

起動時に NIS / Anime4K を選んでいる preset で、通常一覧の表示後に最初の全画面を開き、
画質と初回の待ちを確認する。同じ拡大方法を別 context で使う場合も完成した pipeline を共有する。
本体 lib 全件と実アプリの最終受け入れは設計担当へ引き継ぐ。
