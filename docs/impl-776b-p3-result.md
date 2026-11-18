# §1.776 独立コードレビュー P3 対応（2026-11-18）

基点 `7ae2b4956`、作業ツリー `G:\home\mimageviewer-perf801`、ブランチ `perf-801-776b`。
正本の `review-776b-code-result.md` を確認した。開始時の作業ツリーは clean。コミットは行っていない。

## 変更

- `gpu_still_shaders::ResamplerShaders::uses_dxbc` を `#[cfg(test)]` の accessor として追加。feature から期待値を再計算せず、実際の `ShaderModules` enum を読む。
- NIS / Anime4K は shader オブジェクトを pipeline 作成後に破棄するため、その実オブジェクトから読んだ bool を `#[cfg(test)]` の field にだけ保存する。別の shader を検査用に作る方式にはせず、描画に使う pipeline set / resampler の選択結果を確認する。
- B-A の画素一致2件で、feature ありなら DXBC、なしなら WGSL を検査してから同じ pipeline で描画する。
- B-B の共通 draw helper でも、prebuilt / inline 両方について実際に使う NIS / Anime4K の DXBC 選択を検査する。同 helper を使う失敗 slot・異なる device の GPU 回帰も対象とする。
- display-pipeline §2.4.1 に、画素一致に加えて variant を確認する契約を追記した。

追加した読み取り・記録はすべて `#[cfg(test)]`。製品ビルドの分岐・構造・資源の寿命は変えない。

## 検証

対象は B-A の2件と B-B の3件、合計5件だけ。すべて既存の `gpu_tests` 配下なので nextest の test-group `gpu` に所属する。
`--no-capture` によりプロセス実行は直列。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "perf801-codex" -WorkDir G:\home\mimageviewer-perf801 -Command ".\target\section776p3\run-tests.ps1"
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "perf801-codex" -WorkDir G:\home\mimageviewer-perf801 -Command ".\target\section776p3\run-restored.ps1"
```

runner 内で実行する実コマンド（故障注入時と復元後に同じものを使用）:

```powershell
cargo nextest run -p mimageviewer --lib --offline --cargo-profile dev-runtime -E 'test(/^gpu_lanczos::gpu_tests::still_(nis|anime4k_vl)_dxbc_matches_wgsl_pixels$/) | test(/^gpu_lanczos::gpu_tests::prebuild::(prebuilt_contexts_share_arcs_and_match_inline_all_pixels|failed_store_uses_one_context_slot_for_prepare_encode_and_reuse|different_device_falls_back_without_replacing_original_store_arcs)$/)' --run-ignored default --retries 0 --no-tests fail --no-capture
```

実引数配列は `target/section776p3/command-args.json` に保存する。
`CMAKE_BUILD_PARALLEL_LEVEL=1` / `MSBUILDDISABLENODEREUSE=1` を設定し、FFmpeg DLL は dev-runtime/deps に配置した。

| 状態 | 実行件数・結果 | ログ |
| --- | --- | --- |
| 一時的に WGSL 固定 | **5件実行・0成功・5失敗**、nextest exit **100**、実行15.519秒。5件とも新しい variant assertion が検出 | `target/section776p3/mutant.log` |
| 通常選択へ復元後 | **5件実行・5成功・0失敗**、nextest / runner exit **0**、実行10.505秒 | `target/section776p3/restored.log` |

環境は RTX 4090 / driver32.0.16.2101 / DX12 + FXC、dev-runtime、VALIDATION あり。

## 故障注入

実選択箇所の DX12 条件へ一時的に `false &&` を加え、常に WGSL を選ぶ状態にした。
期待する検出は、feature なし側の正常な WGSL 描画を通った後、feature あり側の variant assertion が失敗すること。
B-B の3件も、共通 draw helper が使う実 pipeline の DXBC assertion で失敗した。

ログ抜粋:

```text
assertion `left == right` failed: shader variant: DX12 with passthrough must use DXBC, without it WGSL (anime=true)
  left: false
 right: true
shader variant: product DX12 NIS must use DXBC, both prebuilt and inline
Summary [ 15.519s] 5 tests run: 0 passed, 5 failed
```

検証用 script は最終 shader ファイルの bytes を保存し、`finally` でそのまま復元して SHA-256 を確認する。
復元後に同じ5件を実行して全件成功し、故障を最終差分に残していない。
最初の補助 script は nextest の通常出力と最終一覧の FAIL 行を重複して10件と数え、集計条件で停止した（runner exit 1）。
復元は finally で完了し、元の bytes と SHA-256 が一致した。Summary の実行5件・失敗5件と variant assertion 5件を
確認する集計に直し、既存の故障ログを使用して復元後の5件を別の runner 呼出しで実行した。故障注入は繰り返していない。

ログは `target/section776p3/mutant.log` / `restored.log`、結果と復元確認は `result.json` に記録する。
復元後 shader の SHA-256 は `E0B8BC4E39F01F04A024C5C72ADFB1D15B36B1A1DEEC5FF89EB7535B76959403`。
故障注入前に保存した bytes と一致し、`selector_restored=true` を確認した。
最終の `cargo fmt`（引数なし）は成功。整形後もテスト時点の5ソースの SHA-256 が不変で、`false &&` の故障注入がないことを確認した。
`cargo fmt --check` と `git diff --check` も exit 0。

テストとテスト専用の読み取りのみの変更なので、確認用製品バイナリの再ビルド・起動は行わない。
