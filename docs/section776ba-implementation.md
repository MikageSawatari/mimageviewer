# §1.776 案 B-A 実装・検証記録

2026-11-18、`perf-801-776b`、基準 `cff3ff5c3` + 本作業の未コミット差分。
実装・ビルド・GPU 実測は Codex、独立静的レビューは別の Sol / xhigh。
本体 lib 全件とコミットは設計担当 ClaudeCode が担当する。製品バイナリは起動しない。

## 変更

- `build/still_image_shaders.rs` を build.rs から呼び、静止画 NIS と Anime4K VL を
  同一 WGSL → naga 30.0.1 → FXC SM5.1 で DXBC 化する。
  build-dependency `naga_wgpu` を `=30.0.1` の完全版固定で別名追加し、動画の naga 27 は変更しない。
- `gpu_still_shaders` で DX12 + device の PASSTHROUGH_SHADERS の場合だけ埋め込みを使う。
  VS と各 PS を別 module にし、WGSL 名から reflection の entry 名へ対応する。
  root device descriptor は adapter が公開する PASSTHROUGH_SHADERS だけを追加要求する。
  vendor/egui-wgpu は wrapper の descriptor をそのまま request_device へ渡しており、vendor 修正は不要。
- NIS の枝別 OnceCell、Anime4K の最初の prepare、context の所有、uniform、texture、
  pass topology、cache key、既存計装は維持する。作成中の状態、worker、retry は追加しない。
- Windows の常設 GPU 一致テスト 2 件（ignore なし）と、製品 DXBC 初回の ignored 計測 2 件を追加する。
  既存 `measure_776` helper も adapter の公開 feature を要求する。

## 生成条件の根拠

正本は registry の wgpu-hal 30.0.1 `src/dx12/device.rs::load_shader` / pipeline layout 生成と
`src/dx12/shader_compilation.rs::compile_fxc`。

| 項目 | 静止画用の条件 |
| --- | --- |
| naga | 実行時と同じ 30.0.1、動画用 27 は別系統で維持 |
| binding | NIS t0/b0、VL t0..t13/b0、特殊定数 b1、すべて space0 |
| HLSL Options | SM5.1、missing binding 不許可、restrict indexing、zero init、loop bounding、sampler heap、task limits 等を HAL に一致 |
| entry | 元 IR の fragment linkage → entry ごとの空 constants の process_overrides → HLSL Writer → reflection 名 |
| FXC release | ENABLE_STRICTNESS (`/Ges`)、既定 O1、Flags2=0 相当。動画の `/O3` を使わない |
| FXC DEBUG | release に DEBUG / SKIP_OPTIMIZATION (`/Zi /Od`) を追加 |
| variant 選択 | root egui-wgpu と同じ `InstanceFlags::from_build_config().with_env()`、WGPU_DEBUG を含む |

独立レビュー担当が、今回の生成 HLSL と保存済み PoC helper の HLSL を **76 stage 比較して全件バイト一致**を確認した。
共通 VS は各 FS に対して照合したため、製品の HLSL ファイルは 42 個、PoC は 38 個。
PoC helper と HAL 実生成 HLSL の一致は先行 PoC の計測で確認済みであり、今回は HAL の計装を再実行したわけではない。
release 用 DXBC も PoC の D3DCompile 出力と **38 stage 比較して全件バイト一致**だった。
CLI の FXC flags と実 API の生成結果の対応もこれで裏付ける。
WGSL 2 本も PoC と同一。path / SHA-256 / 比較境界は
`target/section776ba/independent-hlsl-contract-review.json` に保存する。
共有 VS の生成時検査・整形後の最終生成物でも、同じ 76 / 38 比較と生成 table の一致を再確認した。
最終 generator の hash と出力別の照合結果は `target/section776ba/final-generated-contract.json`。
bytecode は release 用 211,920 bytes、DEBUG 用 3,289,060 bytes、計 3,500,980 bytes（約 3.34 MiB）。

## 検証コマンドと結果

重い処理はすべて次の wrapper 経由。`target/verify-776ba.ps1` はこの作業の検証だけに使用し、製品には組み込まない。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "perf801-codex" -WorkDir G:\home\mimageviewer-perf801 -Command "<以下のコマンド>"
```

| コマンド / 条件 | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core --offline` | 成功、通常 dev、1分59秒 |
| `target/verify-776ba.ps1 -Phase DebugTests`（test） | DEBUG=true、一致2件成功、6比較 / 53,002画素対すべて差0、10.232秒（compile 9分59秒） |
| `target/verify-776ba.ps1 -Phase RuntimeTests`（dev-runtime、初回） | 74件すべて成功、19.149秒（compile 17分42秒）。DEBUG=false の6比較も全画素一致 |
| `target/verify-776ba.ps1 -Phase Measure`（dev-runtime、最終ソース） | 各familyを3回、6件すべて成功。3.362 / 3.317 / 3.208秒、最初の再build 5分45秒 |
| `target/verify-776ba.ps1 -Phase RuntimeTests`（dev-runtime、最終ソース） | 74件すべて成功、24.223秒。DEBUGなしの6比較 / 53,002画素対すべて差0 |
| `target/verify-776ba.ps1 -Phase DebugVariant`（dev-runtime、WGPU_DEBUG=1、最終ソース） | GPU一致2件と版一致1件、3件すべて成功、11.402秒。DEBUGありの6比較 / 53,002画素対すべて差0 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功 |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 成功（通常features、portableなし）。core 6分00秒、companion4本も成功、vcrt-pe は runtime=4 / pe=5 |

最終 RuntimeTests は `3645a952-60e4-40fb-a67b-da3bf507a535`、DebugVariant は
`d067308f-0b6a-40a6-8ee4-e529d99283f9`。ログは `target/section776ba/RuntimeTests-1.log` と
`DebugVariant-1.log`。PowerShell 5.1 の native command ログは UTF-16LE のため、集計では BOM を検出して読む。
最終ソースでの通常・DEBUG 切替・計測は計83実行成功、失敗0。先行確認の2件・74件は別行に残す。
GPUテストは既存 `gpu_tests` prefix によって gpu group へ入り、生成前の所属検査も通過した。
本体lib全件、workspace全件、製品起動は実施していない。

通常 test の run は `ca9859de-456a-493b-aba1-709d8cdd0a7a`、ログは
`target/section776ba/DebugTests-1.log`。この時点では版一致 CPU テストは追加前。
その後の追加は共有 VS の生成時 assertion と版一致テストで、既存の生成条件と GPU 比較入力は変えていない。
初回 dev-runtime の run は `8976607a-9394-46ae-9cf4-5aa034ca24d9`、保存ログは
`target/section776ba/RuntimeTests-initial.log`。

各 phase の実コマンドは次のとおり。いずれも `--offline --retries 0 --no-tests fail --no-capture` を指定し、
通常テストは `--run-ignored default`、計測だけ `--run-ignored only --test-threads 1` を使う。
完全な argv は `target/section776ba/*-command.txt` に保存する。

```powershell
# DebugTests (通常test)、DebugVariant は同じfilterをdev-runtime + WGPU_DEBUG=1で実行
cargo nextest run -p mimageviewer --lib --cargo-profile test -E 'test(/^gpu_lanczos::gpu_tests::still_/) | test(/^gpu_still_shaders::tests::/)'
# RuntimeTests
cargo nextest run -p mimageviewer --lib --cargo-profile dev-runtime -E 'test(/^gpu_lanczos::/) | test(/^gpu_anime4k::tests::/) | test(/^gpu_still_shaders::tests::/)'
# Measure: 3回、毎回2つの別nextestプロセス / 新device
cargo nextest run -p mimageviewer --lib --cargo-profile dev-runtime -E 'test(/^gpu_lanczos::gpu_tests::measure_776_.*_product_dxbc_first$/)' --run-ignored only --test-threads 1
```

GPU 一致テストは同じ adapter から feature なしの WGSL device と feature ありの DXBC device を作る。
斜線・1px 細部・透明境界を持つ 63×47 の premultiplied RGBA8 入力を使い、
126×94 の整数倍、107×80 の非整数倍、91×67 / `[3.25, 2.5, 48.75, 35.5]` の小数 ROI を比較する。
製品の prepare / uniform / pass topology / encode を通し、textureLoad による lossless blit の後に readback する。
Anime4K は全 17 中間 pass を通した最終 resolve を比較する。中間 RGBA16Float 値そのものの検査ではない。
adapter / feature / device 欠落は失敗し、成功扱いの skip はしない。

## 製品 DXBC 経路の初回作成時間

観測者は今回の実装担当 Codex の headless GPU テスト。NVIDIA GeForce RTX 4090、driver `32.0.16.2101`、
DX12 + FXC、dev-runtime（opt-level 2）。flags は製品既定の `VALIDATION_INDIRECT_CALL` だけで、
DEBUG / VALIDATION なしを測定前に検査する。一致テストの validation あり設定とは分ける。
各 family・各回を別 nextest プロセス / 新 device で実行し、NIS は layout owner と NIS 枝、
Anime4K VL は constructor の layout / module / 全 pipeline の CPU 作成時間を測る。
描画フレーム時間、GPU 実行時間、PSO API 単体時間の計測ではない。

| 対象 | 1回目 ms | 2回目 ms | 3回目 ms | 中央値 ms | module作成の中央値 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| NIS | 115.586 | 132.088 | 127.709 | **127.709** | 0.065 |
| Anime4K VL | 696.500 | 627.921 | 663.719 | **663.719** | 0.200 |

元ログは `target/section776ba/Measure-{1,2,3}.log`、集計と各作成区間は `measurement-summary.json`。
run は順に `10f19b3d-bef3-46b6-ab0a-982a6c6a9c87`、`446c1c5a-2634-4ce5-8d6e-8ffabc2cbdf9`、
`b5587b09-04fd-44ea-9d16-05d1fba04a1a`。
driver cache は消しておらず、PC 全体の idle も保証しないため、完全 cold と呼ばない。
PoC の 95.475 / 577.751ms とは別測定として保持する。今回の値が大きい寄与は切り分けていない。
残る pipeline 作成時間が大きく、B-A だけで初回停止をなくす結果ではない。

## 確認用ビルドと引き継ぎ

確認用ビルドは共有 runner 経由の
`scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功した。
core は `target/dev-runtime/mimageviewer-core.exe`（2026-11-18 03:24:21 JST、398,398,464 bytes）。
SHA-256・対象ソース hash・command・exit 0 は `target/section776ba/verification-build.json`。
core と companion は起動していない。本体 lib 全件・コミット・実アプリ確認は設計担当へ引き継ぐ。

ユーザー確認の起動コマンド（この作業ツリーをカレントディレクトリとする）:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

通常の `%APPDATA%\mimageviewer` を使い、実際の設定・データを更新し得る。
single-instance mutex を共有するので、インストール版・tray 常駐版を先に終了する。
小さい静止画を全画面で拡大し、「くっきり拡大」「アニメ拡大」の初回・2回目の表示と画像を確認する。

## 危険と更新時の確認

- `dxil` 欄での DXBC は現在の HAL の bytes 受け渡し実装に依存し、公開 API の形式保証とは区別する。
- passthrough は通常の binding / shader interface 検証を省く。固定 layout・WGSL・生成器の対応を同時に維持する。
- wgpu 更新では naga の版、register / 特殊定数、Options、entry linkage、FXC flags、passthrough の扱いを再確認する。
  DEBUG あり/なしの両 GPU 一致テストと生成 HLSL の HAL 照合を行う。
- DEBUG variant 選択は root の instance flags policy と対になっている。root の flags policy を変更するときも同時に確認する。
- DXBC 化してもドライバの PSO 作成は残る。今回の CPU 作成計測を、製品 release の表示フレーム時間とは扱わない。
- 他 GPU / driver の互換性と実アプリの体感確認は別途必要。非同期先行作成の採否は今回の実装から切り離す。
