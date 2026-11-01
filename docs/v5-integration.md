# v5 系の統合記録

2026-11-01 の利用者決定により、master は v4 系のリリース用とし、v5 系は
`v5-integration` で統合する。順序は `mcp-trial` → `claude/miv-sound-f1-a7147d` →
`musicvideo` → `effetune-lyrics-trial`。候補版の確認先は手元とサブ PC。
この記録は公開・配布や master への統合を示さない。

## 1. mcp-trial の取り込み（2026-11-01）

- 作業先: `G:\home\mimageviewer-badge-1663` / `v5-integration`。
- 取り込み前の HEAD: `946f60f43`（master から作成）。
- MERGE_HEAD / 取り込む tip: `f313f05e60b401ac518e829e103e6e5d855bcece`。
  件名: `docs(mcp): List AI integration use cases and what the current tools can do`。
- 設計担当が開始した `--no-ff --no-commit` merge のファイル編集のみを担当。
  git add / commit / merge / push は行わず、製品も起動しない。検証時は
  `MSBUILDDISABLENODEREUSE=1`。検証担当・実行結果の観測者は Codex 実装担当。

### 衝突の解き方

| ファイル | 解消内容 |
| --- | --- |
| `docs/detached-rework-plan.md` | master の Everything close / F12 と補正入力 owner の追補、MCP の既存 activation 受付への接続記録をすべて保持。detached の述語や viewport 経路の新規変更はない。 |
| `docs/mcp-integration-plan.md` | 段階 0〜2-4 の実装・検証・検収、利用者回答、公開文書下書き・利用例を保持。§15.1 / §15.5〜15.8 は後続レビューで確定した専用入口・純粋関数共有・cache 非使用・配布契約へ更新し、旧案の所有分離の意図と変更理由も §15.5 の履歴に残した。 |
| `htdocs/mimageviewer/privacy.html` | 前回終了・AI 活動の固定項目診断、MCP 要求・変更の記録、AI アプリの登録先と backup の説明を別々の info-box として保持。通常ログとメモリ内の変更一覧の保存範囲を混同しない。 |
| `src/app.rs` | master の `diagnostic_stage` / `DiagnosticEffectWorker` / `DiagnosticFinalAiProgress` と MCP の `agent_change_ui_tests` を独立した項目として保持。それぞれの終端 brace を補い、診断と MCP readiness / toast の双方を残した。 |
| `src/settings_transfer.rs` | 両側の設定分類を保持。master の転送対象155件に MCP の許可・縮小画像・GPS の除外3件を加え、合計485件・除外330件へテスト期待値を更新。MCP 権限を移行先で維持する方針は不変。 |

### 自動 merge 後に直した食い違い

- `all_settings_fields_are_classified`: 除外件数だけでなく、衝突しなかった総数482も485へ修正。
  マクロの除外定義を実際に数えて330件と照合した。転送対象155件は維持する。
- `Cargo.lock`: 新規 workspace crate `mimageviewer-agent-ipc` / `mimageviewer-mcp` の版が
  取り込み元の4.10.0のままだった。Cargo が workspace の4.10.2へ更新した差分を保持する。
  リリース番号の新たな決定や永続スキーマの版変更ではない。
- `docs/development-build-and-test-native.md`: 衝突せず残っていた旧4段階の手順に MCP を加え、
  実スクリプトと同じ5段階・成果物一覧へ修正。MCP 計画の冒頭も段階2-4までの検収と
  v5 側への統合中という現在の状態へ揃えた。`docs/README.md` に本記録への入口を追加。
- 衝突解消処理の初回には marker 末尾のブランチ名が残り、最初の lib/test check が失敗した。
  全5ファイルから除去して fmt を再実行した。これは今回の編集上の不備で、製品の既存不具合と区別する。
- 独立レビューが追加履歴文の文字化けを検出し、UTF-8 の本文へ修正して再読確認した。

### 検証

ログと補助 runner は `target/v5-integration/`（git 管理外）。実アプリ操作・通常 profile での
起動・利用者の AI アプリ設定の読み書きは行っていない。

以下はこの merge 解消後の未コミット差分に対する結果。Rust ソースと lockfile は
成功した型検査・nextest・snapshot・build の間で変更していない。

| 検査 | 結果 / ログ |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | exit 0 / 0。最終 check は30.446s、`fmt-check.log`。 |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit 0、7m02s。`check-lib-tests-final.log`。初回失敗は `check-lib-tests.log`。 |
| `cargo check --workspace --all-targets` | exit 101、245.853s（前の検査のlock待ち含む）。launcher の埋め込み入力である release core / Remote / EPUB worker / MCP の4本が未作成。`check-workspace.log`。 |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher` | exit 0、1259.832s（他のCargo検査のlock待ち含む）。`check-workspace-without-launcher.log`。 |
| 機能本体・関連 nextest（test-script） | exit 100。760件中756成功 / 4失敗、15,268 filter対象外。実行103.146s、compile/lock待ち込み1088.656s。run `1a75a5e1-1c55-4fa5-81ff-564709dfe88d`、`nextest-focused.log` / `.json`。下記4件以外は成功。 |
| 共有経路の追加 nextest（test-script、上の群との重複除外） | exit 0。1,094件すべて成功、14,854 filter対象外。実行56.726s。run `af6dd5c2-ee88-49e0-b9a4-4863bef8ebef`。Remote IPC / tags_db / conceal_db / PDF / ZIP / ui_main / page_edit_release。`nextest-shared.log` / `.json`。 |
| `cargo test --test ui_snapshot` | exit 0。228件すべて成功、実行55.14s、compile/lock待ち込み1564.590s。`ui-snapshot.log` / `.json`。§1.175の例外停止は発生せず、補完実行は不要。PNGの再生成・基準の変更なし。 |
| UI glyph / 製品ページカード | exit 0。`glyphs.log` / `site-cards.log`。 |
| Python MCP stdio 検査・Cargo artifact 検査のテスト | exit 0、15件 / 3件。`python-mcp-tools.log` / `python-cargo-output.log`。 |
| build process policy の mock 検査 | exit 1。PowerShell の mock 部分は通過、Git Bash が `CreateFileMapping` / Win32 error 5 で起動できず停止。`build-policy-tests.log`。 |
| dev / release build の安全性テスト | dev は exit 0（`build-dev-safety.log`）。release は exit 1、Git Bash の `CreateFileMapping` / Win32 error 5（`release-build-safety.log`）。 |
| release 入力の準備 | `build-release.ps1 -PreserveRuntime -SkipVst3Bridge` は exit 1。既存の core / Remote / Susie / VST3 process を保護する検査で停止（`release-prerequisites.log`）。停止処理や APPDATA cache の変更は行わない。 |
| dev 確認用 build | `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は exit 0、584.737s。core / Remote / EPUB worker / MCP を同じ通常featureの `dev-runtime` profileで生成。PE検査も runtime=4 / pe=4 成功（`build-dev.log`）。起動はしていない。 |
| 独立レビュー | GPT-6.1 Sol / xhigh、別 context・読み取り専用。5衝突・自動 merge の近接変更・起動受付・終了順・設定・UI・テストを照合。文字化け修正後の未解決指摘なし。実行検証は実装担当の結果を使用。 |

最初の nextest の失敗4件は、いずれも実 named pipe の作成 / open が OS error 5
（アクセス拒否）で失敗した。取り込み元の MCP 計画 §26.3 / §29 の sandbox 内検証と
同じテスト・失敗箇所であり、DACL の緩和、skip / ignore の追加、期待値変更は行わない。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`
- MCP crate `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- MCP crate `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`

機能本体の filter と共通 nextest 引数は `nextest-focused.json` に全文保存した。
MCP / agent IPC の全lib、agent連携、settings_transfer、preferences、tag_ops、tag_write_worker、
rating_db、collection_store、ui_request_dispatch、§1.679 / §1.680、type_icon、catalog_provider、
process_images を選択した。`all_settings_fields_are_classified` と MCP 権限の移行除外、
master §1.668 / §1.677 / §1.679 / §1.680 に該当するこの群の回帰に失敗はなかった。

関連 nextest は2群合計1,854件中1,850成功 / 4失敗。共有群は最初の群と同じ
nextest共通引数を使い、上記追加領域から最初の群のfilterに一致する名前を除外した。
全文は `nextest-shared.json` に保存。0件一致・中断を成功として数えていない。
全lib / `test-full.ps1` は今回実行していない。launcher込みのworkspace gateは上記の
release入力不足と準備時の稼働process保護で未成立であり、全体成功とは扱わない。

最終の `git diff --check` は exit 0。5衝突ファイルのmarker・末尾ブランチ名残骸は0件、
privacyのdiv対応も確認した。検証対象の `src/app.rs` / `src/settings_transfer.rs` /
`Cargo.lock` のSHA-256は `final-source-hashes.json`、dev成果物4本のSHA-256は
`build-dev-artifact-hashes.json` に保存した。

### 判断待ち・引き渡し

同じ設定の意味、UI、永続スキーマ / migration に関する判断待ちはない。
ファイル内容を解消しても index は未解決のまま（`UU`）であり、設計担当の git add と
merge commit が必要。残る検証は、通常権限での実pipe 4件、release入力を準備できる
状態でのlauncher込みworkspace gate、別途計画する実機確認である。Git Bashを使う
補助スクリプト2件の通常権限での再検査も残る。衝突解消上の判断待ちとは区別する。

確認用 build は未起動。利用者が実行する場合のコマンドは次のとおり。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

この高速core buildは通常の `%APPDATA%\mimageviewer` profileを使い、実際の設定・データを
更新し得る。single-instance mutexを共有するため、先にインストール版 / tray常駐版を終了する。
通常設定と分離した確認が必要な場合は同じexeへ `--data-dir .\target\dev-runtime\data` を渡す。
確認対象は「AI 連携」窓の許可 / 登録 / 読み取り / タグ・★・コレクション変更と、
master 側の種類アイコン・短いクリック / ダブルクリック・コレクション再訪である。
このmergeでは実機・サブPCでの確認を実施していない。
