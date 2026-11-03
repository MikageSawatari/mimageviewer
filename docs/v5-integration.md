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

2026-11-01 設計担当の追確認: 上記4件はサンドボックス外では成功した。
設計担当が `mimageviewer-mcp` 65件、本体 `agent_ipc` 146件の成功を確認している。
これは設計担当による外部実行の報告であり、Codex がサンドボックス内で再現した結果とは区別する。

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
この節のファイル編集の引き渡し時点では index は未解決（`UU`）で、git add と merge commit は
設計担当へ引き渡した。その後の設計担当の merge commit は `e7d728213`（§2の基点）。
実pipe 4件は設計担当のサンドボックス外での追確認により完了した。
残る検証は、release入力を準備できる状態でのlauncher込みworkspace gate、別途計画する実機確認である。Git Bashを使う
補助スクリプト2件の通常権限での再検査も残る。衝突解消上の判断待ちとは区別する。

以下の確認用 build と起動案内は MCP merge 時点の履歴であり、未起動。
後続 merge が同じ `target/dev-runtime` を更新した後は、後続節の制約に従う。
特に §2 の検査専用 Sound 入力を含む build の通常 profile 起動案内には使わない。
MCP merge 時点の起動コマンドは次のとおり。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

この高速core buildは通常の `%APPDATA%\mimageviewer` profileを使い、実際の設定・データを
更新し得る。single-instance mutexを共有するため、先にインストール版 / tray常駐版を終了する。
通常設定と分離した確認が必要な場合は同じexeへ `--data-dir .\target\dev-runtime\data` を渡す。
確認対象は「AI 連携」窓の許可 / 登録 / 読み取り / タグ・★・コレクション変更と、
master 側の種類アイコン・短いクリック / ダブルクリック・コレクション再訪である。
このmergeでは実機・サブPCでの確認を実施していない。

## 2. claude/miv-sound-f1-a7147d の取り込み（2026-11-01〜02）

- 作業先は引き続き `G:\home\mimageviewer-badge-1663` / `v5-integration`。
- 取り込み前 HEAD: `e7d728213d0997efc544338d64760d8595d8b5fa`（前節の MCP merge）。
- MERGE_HEAD / 取り込む tip: `62ebdd232b8a674e168cec405282dc636a13bdd9`。
  件名: `Merge branch 'master' into claude/miv-sound-f1-a7147d`。
- 設計担当が開始した merge のファイル編集だけを担当。git add / commit / merge / push、
  製品起動、通常設定の変更、mIV Sound の配布承認は行わない。
  検証担当・以下のローカル観測者は Codex 実装担当。全ビルドで `MSBUILDDISABLENODEREUSE=1`。
- 派生版計画の F2a / F2b-1 の範囲を維持する。mIV の `effetune_*` 設定、状態ファイル名、
  通知IDと保存形式を変えず、移行の終端結果を local / Remote 共通の起動 gate へ接続する。
  未完了の F3 / F4 や旧版素材・実物 WebView2 の受け入れを、この merge だけで完了とはしない。

### 衝突の解き方

| ファイル | 解消内容 |
| --- | --- |
| `crates/launcher/Cargo.toml` | MCP を含む core → Remote → EPUB worker → MCP → launcher の5段階を保持し、音響調整の入力を `vendor/miv-sound` へ更新。 |
| `docs/native-dependencies.md` | 3箇所。MCP adapter の同梱・存在検査と5段階を保持し、旧 Mixwright 固定ファイル数から mIV Sound の config / 承認 manifest・候補作成・明示承認の契約へ更新。 |
| `docs/release-checklist.md` | 配布形態の表で MCP adapter と mIV Sound bundle を両方保持。取り込み側の承認・署名・公開前提も保持。 |
| `docs/ui-snapshot-policy.md` | master §1.672 の実親移動の記録と mIV Sound の設定3枚・credit追加2枚を別段落で併存。§1.677 等の先行記録も保持。 |
| `src/app/tests.rs` | `collection_thumb_1668` と Windows 限定 `audio_plugin_migration` を独立した module 宣言として両方保持。 |

### 自動 merge の整合性確認

- 引数・型・設定総数・起動 gate・Remote・runtime cleanup・build script の接続を確認した。
  mIV Sound は設定フィールドを追加しておらず、前節の設定総数485件・除外330件・転送155件を維持する。
- 初回の lib/test check は、未承認の `third_party/miv-sound/0.15.0-miv.1/` の通知が無く停止した。
  初回 workspace check も同じ承認物の manifest 不足で停止。旧 Mixwright の通知や manifest を
  新製品の承認物として代用せず、下記の検査専用入力で本体側の検証を進める。
- §1.668 / §1.677 / §1.679 / §1.680 の既存機能・テストは保持。
  衝突解消以外に新たな製品コード修正やスキーマ / migration 版番号の選択は行っていない。

### ネイティブ入力の準備と制約

- この worktree に VST3 SDK が無かったため、`G:\home\mimageviewer-everything-1664\vendor\vst3sdk`
  から実ファイルをコピー（reparse point 共有なし）。CMake の VS 18 2026 / x64 / Release で
  **現ソースを実コンパイルして host を再作成**し、再実行ログも保存した。古い host exe は流用しない。
  `vst3-host-identity.ps1 -ValidateRepo` は成功。source identity は
  `574446a13fa6faf50b3fc3e7d324867e6c75ba3de93f073fb16130c1e9f9ad89`。
  host exe は1,201,152 bytes、SHA-256は
  `91ecf527437ba1c59c3cc214dce6cd96f55640736e57d414c1e6840939ae799b`。
  `sound-host-artifact.json` にも保存。後続の歌詞 merge で host が変わったら再ビルドが必要。
- 指定の `build-miv-sound.ps1 -SourceRoot G:\home\miv-sound` は、ソース外 build tree への
  書き込みがサンドボックスで拒否された（`sound-original.log`、exit 1）。
- 同一ソース HEAD `f219c5604a95f0a209d39df1d7d431b73b5eb2e0` と submodule / SDK を
  `target/miv-sound-source` へ実体コピーし、clean を確認して同スクリプトを再実行した。
  CMake の compiler ABI detection / Ninja が10分以上無進捗となり、今回開始した process だけを
  停止した（688.316秒、exit 1、`sound-local.log` / `sound-local-interruption.txt`）。
  停止後の configure エラーを製品のテスト失敗には数えない。候補生成と ctest には未到達。
- Rust 統合の検証を続けるため、元ソースの既存 `build/windows-release/VST3/Release/mIV Sound.vst3`
  を **検査専用スナップショット**としてコピーした。ID:
  `dcdfa79447071cc35bfb0dc81928271650fff543e5257275fe0d8c2812fa6d34`。
  `target/v5-integration/prepare-sound-inspection.ps1` が artifact / VERSION の一覧と hash を記録し、
  現行 `Read-MivSoundCandidate` で検査した。`candidate.json` は `inspection_only=true` と
  原本の場所・コピー時の source HEAD・バイナリの由来未証明という注記を持つ。
  この hash / PE / 通知存在の確認は、ソースとバイナリの対応・実際の版・再ビルド・ctest 成功を証明しない。
  build contract / ctest 成功記録は作らず、承認処理はこの入力を明示拒否する。
  `-Approve` は実行せず、vendor / third_party の配布承認物も作らない。
- 下記の本体検証は `MIV_SOUND_CANDIDATE=<上記ID>` を用いる。
  Rust の統合・UI・配置の証拠に限定し、mIV Sound 本体の再ビルド / 実物受け入れ / 配布 gate の
  成功とは区別する。通常 profile の起動確認は行わない。

### 検証結果

ログ・各 command / exit / 経過秒の JSON は `target/v5-integration/sound-*`。
検査対象の変更ソース33本の SHA-256 は `sound-source-hashes.json`。

| 検査 | 結果 / ログ |
| --- | --- |
| `cargo fmt` / `cargo fmt --check` | exit 0 / 0、45.832秒 / 45.204秒。 |
| `cargo check -p mimageviewer --lib --tests --features test-script` | 検査専用IDを指定して exit 0、393.161秒。`sound-check-lib-final.log`。初回の通知不足は `sound-check-lib.log`。 |
| 関連 nextest（test-script） | 1,149件すべて成功、実行80.832秒（compile / lock待ち込み814.417秒）、exit 0。run `21f9994e-62e8-47be-97b3-ced647009534`、`sound-nextest.log` / `.json` / `sound-nextest-junit.xml`。 |
| credit追加 nextest（test-script） | `test(miv_sound)` の1件成功（上の群とは重複なし）、実行2.511秒、compile / lock待ち込み754.763秒。run `aa2a5d22-a4d1-4a0f-b4f6-de26e7d764fc`、`sound-nextest-credits.log` / `.json` / `sound-nextest-credits-junit.xml`。本体共通helperのcredit画像2枚も比較。 |
| `cargo test --test ui_snapshot` | 228件すべて成功、実行43.17秒、compile / lock待ち込み1016.698秒、exit 0。`sound-ui-snapshot.log` / `.json`。§1.175の例外停止は発生せず、補完不要。基準PNGの再生成・更新なし。 |
| `cargo check --workspace --all-targets` | exit 101、978.807秒（lock待ち含む）。launcher が承認済み `third_party/miv-sound/0.15.0-miv.1/manifest.sha256` 不足で停止。検査専用IDはlauncherの承認を代用しない。`sound-workspace-final.log`。 |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher` | exit 0、186.972秒。`sound-workspace-no-launcher.log`。通常featureの本体・workspace全targetの型検査に新たな食い違いなし。 |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | exit 101、957.391秒（lock待ち含む）。最初のworkspace nextestのcompile準備で同じ承認manifest不足。テスト実行・doctest・vendor段には進まず、full gateの成功とはしない。`sound-full-gate.log`。 |
| VST3 CMake build / identity | exit 0 / 0。`sound-host-build.log` / `sound-host-identity.log`。 |
| host のパス境界 ctest | 1件成功、exit 0。`sound-host-path.log`。inert DLL だけを使用し製品 host を起動しない。 |
| 候補・配布・依存更新・dev build保護のスクリプト回帰 | すべて exit 0。`sound-candidate-tests` / `sound-distribution-tests` / `sound-dependency-tests` / `sound-dev-safety`。合成 fixture だけを使用。 |
| マニュアル生成器 | 30件成功、exit 0。`sound-doc-tests.log`。 |
| 移行検証ツールのオフライン検査 | 211項目 / 99項目成功、exit 0 / 0。`sound-probe-offline.log` / `sound-migration-probe-offline.log`。WebView2・製品は未起動。 |
| UI glyph | 0件、exit 0。`sound-glyphs.log`。 |
| dev build | `build-dev.ps1 -MivSoundCandidate dcdfa79447071cc35bfb0dc81928271650fff543e5257275fe0d8c2812fa6d34 -PreserveRuntime -WaitForOtherBuildsMinutes 0`、exit 0、286.453秒。通常featureのcore / Remote / EPUB worker / MCPを生成し、検査専用Sound入力を配置。PE検査は runtime=4 / pe=4 成功。`sound-build-dev.log` / `.json`、4本のhashは `sound-build-dev-artifacts.json`。未起動。 |

nextest の filter と共通引数は `sound-nextest.json` に全文保存した。JUnit の実行件数を照合し、
重複ありの領域別件数を `sound-test-coverage.json` に保存した。音響調整160件、移行App入口5件、
DSP32件、Remote IPC536件、runtime cleanup22件、環境設定231件、設定転送35件を含む。
master 側は catalog provider14件（§1.668）、種類icon16件（§1.677）、§1.679の22件、
§1.680の35件が成功した。§1.668 の既存 ignored 性能ハーネスは実行していない。
追加 credit 群を含む関連 nextest は計1,150件成功。新しいcredit基準2枚は今回も画像を開き、
明暗の文字、狭幅での折り返しと全文の収まりを目視確認した。

製品起動禁止に従い、`effetune_host_handler_tests` の3件は filter で除外し、未検証とする。
テストの削除・ignore追加・期待値の緩和は行っていない。host の実プラグインロード、
WebView2、実音声、通常profileの移行・実機・サブPC確認は今回実施していない。

33本の変更ソースのhashは各検査・build後も一致し、検証中の製品コード変更はない。
最終 `git diff --check` は exit 0、変更ファイルの衝突markerは0件。
独立レビューは別contextの GPT-6.1 Sol / xhigh が読み取り専用で担当し、
5衝突・自動merge近接経路・起動gate・build契約を確認。製品コードの未解決指摘なし。
検証結果は実装担当の証拠を再利用した。文書の「実pipe4件が未完了」という古い記述を修正し、
§1の起動案内を現在のSound buildへ適用しないことも追記した。

### 判断待ち・引き渡し

同じ設定・UI・永続形式・migration版番号について推測で決めた点や、未解決の判断待ちはない。
ファイル内容は解消済みだが、index の5ファイルは `UU` のまま。
設計担当が内容を確認して git add と merge commit を行う。Codex は git の書き込みを行っていない。

残る検証は以下のとおり。衝突解消の判断待ちとは区別する。

- サンドボックス外で正式な mIV Sound 候補をソースからビルドし、同じ候補の ctest を実行する。
  今回の検査専用IDを承認へ進めず、正式候補で確認用buildを作り直す。
- 配布承認・release埋め込み入力を用意できる段階で launcher 込みの workspace / full gate を通す。
  今回は承認物を作らず、fullのdoctest・vendor段も未実施。
- 製品起動を含むhost handler3件、実音声・WebView2と旧版素材の移行・本体/サブPCの確認は、
  設計担当が別途計画する実機検証で扱う。後続の歌詞mergeではhostの再build / identity照合もやり直す。

現在の `target/dev-runtime` は検査専用Sound入力を含み、通常profileでの起動を受け入れた候補ではない。
この途中版は通常データでは起動しない（[派生版計画 §5.7](effetune-fork-plan.md#57-f2b-1-の実装と検証の入口)）。
起動の承認・隔離データの規則は将来の確認にも必要であり、今回のbuild成功から起動許可は導かない。
通常featureのcoreは引数なしでは `%APPDATA%\mimageviewer` の実設定・データを更新し得て、
インストール版 / tray常駐版とsingle-instance mutexも共有する。
正式候補を準備した後にユーザーが通常profileで実行する場合のコマンドは、リポジトリルートから次のとおり。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

通常profileで確認する場合は先にインストール版 / tray常駐版を終了する。
隔離確認は同じexeへ `--data-dir .\target\dev-runtime\data` を渡す。
確認対象は、音響調整の名称・credit、移行前後のlocal / Remoteの起動待ちとユーザーVST、
MCP経由で開く操作との共存、masterのコレクションサムネイル・種類icon・クリック操作である。
今回これらの製品起動は行っていない。

## 3. musicvideo の取り込み（2026-11-02）

- 作業先: `G:\home\mimageviewer-badge-1663` / `v5-integration`。
- 取り込み前 HEAD: `c23d75661`（前節の mIV Sound merge）。
- MERGE_HEAD / 取り込む tip: `6aa58ae7da047c3c93e46aa90921bbff6f91799e`。
  件名: `docs(musicvideo): Record P5a independent review 3 ACCEPT`。
- 設計担当が開始した merge のファイル編集のみを担当。git add / commit / merge / push、
  製品起動、通常設定の変更、Sound の配布承認は行わない。未追跡 `out.mp4` は対象外。
  検証時は `MSBUILDDISABLENODEREUSE=1`。以下のローカル結果の観測者は Codex 実装担当。
- P5a は取り込み元で独立レビュー3回目 ACCEPT、利用者の GUI 確認待ちという状態を保持。
  この merge の自動検査だけで GUI 確認や候補版の受け入れを完了とはしない。

### 衝突23ファイルの解き方

| ファイル | 解消内容 |
| --- | --- |
| `CLAUDE.md` | master の規則・v4/v5並行開発を保持。MCP と editor の前提・出力固定を統合し、release 6段／内包入力5本へ更新。 |
| `crates/launcher/Cargo.toml` | MCP・editor・mIV Sound の全同梱を保持。core → Remote → EPUB worker → MCP → editor → launcher の順序を記載。 |
| `crates/launcher/build.rs` | 全5 exe の存在・hash・rerun を保持。musicvideo P4e のレビュー済み OUT_DIR 導出を採り、MCP も同じ release tree から読む。Sound の config／承認 manifest 検査は維持。 |
| `crates/launcher/src/main.rs` | MCP 専用起動入口・MCP_ASSET と editor の ASSETS を併存。両 exe の毎回の内容 hash 検査を維持。 |
| `docs/development-build-and-test-native.md` | release 6段と全成果物を併記。通常データの保護・製品起動禁止を維持。 |
| `docs/development-build-and-test.md` | dev 5本、release 6段、launcher 前提5本、準備コマンドを統合。出力先 guard と editor 自動停止禁止を維持。 |
| `docs/keymap-spec.md` | 既存の連携窓と editor 固定入力の説明を両方保持。 |
| `docs/native-dependencies.md` | MCP＋editor 同梱、OUT_DIR 契約と Sound の承認 manifest／ctest／provenance 契約を統合。 |
| `docs/portable-build-plan.md` | MCP＋editor の loose 配置・5本の build と editor 保護を統合。Sound／host の portable 非同梱という現行制約を維持。 |
| `docs/release-checklist.md` | MCP と editor を build／clean／署名／PE 検査対象へ併記。Sound 承認・公開前提は保持。 |
| `docs/release-operations.md` | 6段 build、両 package の clean・署名・PE 検査を統合。MCP の検証入口と editor 保護を維持。 |
| `docs/spec.md` | master の Collection 永続キャッシュ仕様と musicvideo の fps 仕様を両方保持。 |
| `htdocs/mimageviewer/index.html` | Everything／AI と editor の通信・保存説明を保持。カードの字数上限を守り、詳細は privacy と既存 trust-note へ配置。 |
| `htdocs/mimageviewer/privacy.html` | 更新日は新しい11月1日を保持。MCP／既存診断と musicvideo の保存・通信説明を保持。 |
| `scripts/build-dev.ps1` | MCP と editor を同じ dev-runtime へ build。5本の artifact／PE 検査、Sound candidate、既存 process 保護を併存。 |
| `scripts/build-dist.ps1` | 両 package の clean と全5 exe の署名／内包前 PE 検査を保持。Sound 承認 staging は従来契約のまま。 |
| `scripts/build-portable.ps1` | 全5 exe の build／copy／署名／PE／manifest を統合。MCP の既存 process 保護と editor 非停止を保持。 |
| `scripts/build-release.ps1` | 6段 build、5入力の artifact 照合・署名、固定出力 guard、MCP の exact-path 停止、editor 非停止、Sound 承認検査を統合。 |
| `scripts/build-release.sh` | 同じ6段順序・artifact 照合を保持し、両 exe の前提検査と editor 保護を統合。 |
| `scripts/test-build-dev-safety.ps1` | MCP＋editor の出力・mock build 件数・process 保護を検査する一覧へ統合。 |
| `scripts/test-release-build-safety.ps1` | 6段の mock build・artifact／署名順序を検査する一覧へ統合。 |
| `scripts/ui-smoke/release-checks/Support.ps1` | manifest 照合へ MCP と editor の両パスを渡す。実アプリ試験自体は未実施。 |
| `src/runtime_cleanup.rs` | editor を残す process snapshot 回帰に MCP も加え、Sound bundle 名は config の BUNDLE_NAME を使う。既存の版／世代 lease と保護を保持。 |

### 自動 merge 後に直した食い違い

- launcher の非衝突テストに残った editor 追加時の資産一覧・OUT_DIR 入力一覧へ MCP を補った。
  MCP と editor を含む全資産が抽出され、埋め込み入力が同じ release tree に属する不変条件を検査する。
- `scripts/test-musicvideo-build-safety.ps1` の manifest fixture に MCP を補い、両 exe の hash を検査する。
- `scripts/test-build-process-policy.ps1` の先頭3文固定の検査が musicvideo の metadata guard と
  衝突していたため、metadata guard → allow-list guard の両方が停止・build・署名前にあることを検査する。
  mock artifact／manifest の対象一覧にも両 exe を保持した。
- 文書の自動 merge 箇所にも残った MCP 欠落・4本／5段表記を現行5入力／6段へ補正。
  `docs/project-reference.md` の旧 Mixwright 表記も承認契約付き mIV Sound へ更新した。
  `music-video-export-plan.md` の過去の検査件数・当時の4本／5段は履歴として変更していない。
- 設定の総数485・除外330・転送155は不変。schema／migration／保存設定の意味を選び直す変更はない。
  music-core の既存APIは既定寸法を維持し、editor の寸法指定APIと併存する。
- Sound の F3（公開文書）／F4（portable 有効化）は本 merge の対象外。
  launcher／release の Sound 承認同梱契約は保持し、portable の未実装機能を新たに有効化しない。

### 検証条件と結果

ログと command／exit／秒の JSON は `target/v5-integration/musicvideo-*`。
本体の検査には §2 と同じ検査専用 Sound 入力
`dcdfa79447071cc35bfb0dc81928271650fff543e5257275fe0d8c2812fa6d34`
を `MIV_SOUND_CANDIDATE` に指定した。承認済み bundle・ctest 成功の代用にはせず、
製品・通常 profile・実 editor は起動しない。詳細な入力の限界は §2 を参照。

| 検査 | 結果／証跡 |
| --- | --- |
| `cargo fmt` | exit 0、22.130秒。`musicvideo-fmt.log` / `.json`。 |
| `cargo fmt --check` | exit 0、21.050秒。`musicvideo-fmt-check.log` / `.json`。 |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit 0、192.045秒。`musicvideo-check-lib.log` / `.json`。型・引数・設定分類で追加修正を要するエラーなし。 |
| `cargo check --workspace --all-targets` | exit 101、99.096秒（Cargo lock待ちを含む）。launcher の承認済み Sound manifest 読み取りで停止。`musicvideo-workspace.log` / `.json`。 |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher` | exit 0、671.179秒（lock待ちを含む）。通常 feature の workspace 全targetに追加の型・呼出し不整合なし。`musicvideo-workspace-no-launcher.log` / `.json`。 |
| 機能・関連 nextest（`test-script`） | exit 0、1,234件すべて成功／15,453 skip、実行129.006秒、compile／lock待ち込み530.216秒。run `966a942f-3fe3-4e9e-8f51-de1786e370df`。`musicvideo-nextest.log` / `.json` / `musicvideo-nextest-junit.xml`。 |
| 追加の経路 nextest | `comic-core` の `legacy_text_pixels` と `musicvideo-core` の `analysis_routes`、5件すべて成功。exit 0、実行0.148秒、compile／lock待ち込み505.049秒。run `c7cc9c0a-4c0e-4604-9009-8c90044569b7`。`musicvideo-nextest-routes.log` / `.json` / `musicvideo-nextest-routes-junit.xml`。前のlib群と重複なし。 |
| `cargo test --test ui_snapshot` | exit 0、228件すべて成功、実行38.15秒、compile／lock待ち込み578.603秒。`musicvideo-ui-snapshot.log` / `.json`。§1.175の例外停止なし、補完不要、基準PNGの再生成・更新なし。 |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | exit 101、607.771秒（lock待ちを含む）。workspace nextest のcompile準備で同じ承認manifest不足。テスト実行／doctest／vendor段へ未到達。`musicvideo-full-gate.log` / `.json`。 |
| OUT_DIR 導出 helper の単独テスト | 実 module を `rustc --test` でコンパイルし5件すべて成功。製品の起動ではない。`musicvideo-launcher-path-{compile,tests}.log` / `.json`。launcher 全体の埋め込み・展開テストは配布入力不足で未実施。 |
| UI glyph／製品ページカード | 両方 exit 0、glyph違反0／51 cardsの字数違反0。`musicvideo-glyphs` / `musicvideo-site-cards` の `.log` / `.json`。 |
| build safety（実装担当の別 context） | 下記5系統成功。実行履歴を再利用し、同じ検査の再実行はしない。ログファイルはなくツール実行履歴のみ。 |
| 独立レビュー | GPT-6.1 Sol / xhigh、実装と別 context・read-only。23衝突と非衝突の近接経路を両親と照合、最終 ACCEPT・P1〜P3指摘なし。既存変更snapshot2枚も目視。自動検証は担当者の証拠を使用し、§3とJSON／ログ／JUnit、114本のsource hashも照合。 |

関連 nextest の内訳は本体574、comic-core 122、music-core 49、musicvideo-core 165、
musicvideo-editor 189、musicvideo-ui 135。filter 全文と共通引数は JSON に保存した。
本体 mvexport／musicvideo launch／共有 spectrum、runtime cleanup／locks、settings_transfer、
preferences、context menu、type_icon、catalog_provider、§1.679／§1.680を含む。
設定分類テストと文字準備予算の致命エラーテストも成功。
§1.668 の opt-in 大量画像計測2件は既存 ignored のままで実行せず、catalog_provider の通常回帰14件を実行した。
skip 数は filter 対象外と既存 ignored の合計で、0件一致を成功とはしていない。
nextest は2群合計1,239件成功。これを full gate 成功とは扱わない。
検査中に製品コードや基準PNGの追加変更はなく、統合された変更ソース114本の SHA-256 は
`musicvideo-source-hashes.json` に保存した。最終の差分／marker 検査も成功し、
`musicvideo-final-checks.json` に source変更0／marker残存0／diff-check exit 0／未stageのUU 23件を記録した。

build safety の command はすべて
`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/<name>`。
`test-build-dev-safety.ps1`（exit 0、0.768秒）、
`test-musicvideo-build-safety.ps1`（exit 0、1.101秒）、
`test-build-output-safety.ps1`（exit 0、7.826秒、実Cargo metadata 88ケース）、
`test-release-build-safety.ps1 -PowerShellOnly`（内部の process-policy 検査も含む、exit 0、総秒未計測）、
`test-release-check-safety.ps1`（exit 0、総秒未計測）が成功。
通常の `test-release-build-safety.ps1` は exit 1、1.584秒で、内部の Git Bash が
`CreateFileMapping` / Win32 error 5（内部exit 256）により起動できなかった。
Bash の syntax／mock 実行は環境不成立として残し、PowerShell の成功と混同しない。
修正前の release safety 一回は新しい fixture の `PSScriptRoot` が空で停止したが、
fixture の修正後に PowerShell 全範囲が成功した。製品の不具合とは区別する。

### 判断待ち・引き渡し

同じ設定項目・UI・永続データ形式の意味に関する判断待ちはない。
index は設計担当の git add 前なので23ファイルが `UU` のまま。作業ファイルの衝突マーカーは除去済み。
git add と merge commit は設計担当が行う。

launcher の配布入力は release exe 5本すべて、
`third_party/miv-sound/0.15.0-miv.1/manifest.sha256` と `vendor/miv-sound/VERSION` が未配置。
承認を作ったり検査専用入力で置き換えたりせず、設計担当が正規の入力を揃えてから
launcher 込み workspace 検査・release build／展開試験を行う。
今回の変更は launcher／埋め込み構成に依存するため、dev build だけを確認用成果物として渡さない。
新しい user-runnable release 確認バイナリはこの前提不足により作成していない。
§2 の検査専用 `target/dev-runtime` も通常 profile の起動確認には使わない。

## 4. effetune-lyrics-trial の取り込み（2026-11-02）

- 作業先は同じ `v5-integration` worktree。取り込み前 HEAD は `8915df78ee47888129d8069dc9b8371d16a1c21f`。
- MERGE_HEAD / tip は `5b983ad06be798f7400e18c93efa2333af049a26`。
  件名は `Merge master (v4.8.0) into effetune-lyrics-trial`。
- 設計担当が開始した4本目のmergeについて、ファイル編集と非対話検証だけを担当。
  git add / commit / merge / push、製品起動、通常設定の変更、Sound候補の承認は行わない。
  index の `scripts/build-dev.ps1` は設計担当のaddまで `UU` のままとなる。

### 試作から取り込む内容と範囲

`docs/effetune-lyrics-trial-plan.md` の方式Aを取り込む。埋め込み歌詞をdecoder workerで取得し、
既存の画像補完workerで同名 `.lrc` を独立に取得する。Now Playingは曲の世代・歌詞本文・長さを持ち、
画像がない曲でも歌詞の補完を公開する。Localのclock/diagnosticsを同じ所有状態に保持し、
host-control workerから世代付きの再生位置・停止/再生・速度・QPC時刻を通知する。
Remoteは歌詞/位置なしとし、clear・所有交代・再登録を既存配送境界で扱う。

Rust/C++/testerをprotocol v8に揃え、C++はNow PlayingとPlaybackPositionを別の有界mailboxで配送する。
音声callbackには時刻差の採取を追加し、待ち・ログ出力は追加しない。
分析・素材生成・旧試作の実機手順も保持する。歌詞解析・描画はプラグイン側の責任であり、
このmergeが持つのはmIV/host側の送信実装。**歌詞を受信・表示するmIV Soundの完成を意味しない**。
旧Mixwright試作の設計・検証結果は履歴として残し、planとsmoke冒頭に現行v5との差を追記した。

### 衝突と自動merge後の食い違い

| 対象 | 解消内容 |
| --- | --- |
| `scripts/build-dev.ps1`（唯一の衝突） | MCP・Remote・EPUB worker・musicvideoを含む5本のbuild/PE検査、process保護、Soundの既定承認入力とcandidate経路を保持。歌詞側のdev限定raw bundle差し替え引数 `-EffetuneBundle` を、現行configの `mIV Sound.vst3` 配置へ統合した。旧Mixwright名とcandidateとの同時指定は拒否し、配布の承認/manifest検査は緩めない。 |
| `src/video/dsp/extract.rs`（非衝突） | 旧試作の `-lyrics-trial.exe` への改名を共通hostへ持ち込むと、ユーザーVSTのCHOC WebView保存先まで変わる。標準basenameを保持し、音響調整の分離は既存の `BridgePurpose` / `MIV_SOUND_DATA_DIR` を使う。対応する既存抽出テストも標準名を検査する。 |
| `scripts/test-build-dev-lyrics-trial.ps1` | 旧vendor/bundle固定fixtureを現行Sound configへ更新。既定承認gateとraw manifest照合、実candidate reader/stager、試作差し替え、stale pointerを含む全container置換、入力原本・再配置不変、相互包含・旧名・同時指定の拒否を検査する。fixtureはtarget内の合成データで、承認候補としては使わない。 |
| `src/app/list_order_ratingnav649_tests.rs` / `src/ui_main.rs`（全件検証で発見、test-only） | §1.680のactivation後の入力消費に対して旧positive controlが残っていた。現在のmasterの修正 `e56021968b60d366db57d9b10928688a89f8ad22` のtest/helper hunkだけを再利用。1クリック＋secondaryではmenuあり・open受付0、通常double-clickではnavあり・受付1・menuなし、Smartの既存assertを維持する。歌詞mergeによる製品退行ではなく、`611fdfb95` 後のテスト前提の不整合。製品の入力処理・MCP等は変更しない。 |

`-EffetuneBundle` は既定の承認noticeが揃った開発環境で、移植済みのmIV Sound試作bundleを指定するための入口。
raw入力のPE/名前確認は実装対応・全資産の完全性・ctest成功・承認を証明しない。
既定の `Assert-MivEffetuneSource` / raw manifest検査、candidateの内容hash検査、releaseの承認gateは維持する。
旧Mixwright試作をrenameして流用する互換経路は作らない。

hostのmIV Sound窓タイトル・initialize失敗理由の保持・ProcessContextの既存契約は両親と照合して残存を確認。
歌詞通知追加でこれらを置き換えていない。設定項目・永続schema・migration版番号に新たな選択はない。
独立レビュー（GPT-6.1 Sol / xhigh、実装と別context、read-only）は上記basenameのP2を検出し、
修正後の製品コード・build-dev・配置回帰をACCEPT。受信側未移植とnotice前提は残作業として扱う。
検証追補のtest/helperも同じreviewerが元commitとの一致・既存2クリックの入力列保持・test-only範囲を確認しACCEPT。

### 検証条件と結果

観測者はCodex実装担当。`MSBUILDDISABLENODEREUSE=1`、`CARGO_NET_OFFLINE=true`。
§2の検査専用Sound入力 `dcdfa79447071cc35bfb0dc81928271650fff543e5257275fe0d8c2812fa6d34`
をRust検査の `MIV_SOUND_CANDIDATE` に指定。配布入力・承認済みbundle・歌詞受信側の代用とはしない。
各command/exit/秒は `target/v5-integration/lyrics-*.json`、出力は同名 `.log` に保存する。
Windowsのerror modeで検証子processのcrash dialogを抑止し、非ゼロ終了は失敗のまま記録する。

| 検査 | 結果／証跡（`lyrics-` prefix） |
| --- | --- |
| `cargo fmt` / 最終 `cargo fmt --check` | exit 0 / 0。`fmt`、test-only追補後の `fmt-final`、`fmt-check` のlog/JSON。整形による無関係な変更なし。 |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit 0、168.815秒。`check-lib`。後の追加変更は既存masterのtest-only差分で、下記112件のtest compile/runで確認。 |
| `cargo check --workspace --all-targets` | exit 101、28.835秒。`check-workspace`。launcherの承認済みSound manifest不足で停止。 |
| 同上 `--exclude mimageviewer-launcher` | exit 0、488.710秒（Cargo lock待ちを含む）。`check-workspace-no-launcher`。通常featureの全targetに型・引数の追加不整合なし。 |
| CMake configure / Release build | exit 0 / 0、1.392秒 / 20.806秒。`host-configure` / `host-build`。既存exeの流用ではなく、歌詞とSound変更を持つ現ソースを実コンパイル。 |
| `scripts/vst3-host-identity.ps1 -ValidateRepo .` | exit 0、0.753秒。`host-identity`。source identity `194ddc8da9ed51416f828db49508429a3b156b55f797011cfe0cbf27d3f888fc`。 |
| host CTest | exit 0、5/5成功、40.946秒。`host-ctest`。通知・headless mailbox・protocol v8・GUI境界・pathのfixtureを確認。製品UIや実プラグインの起動ではない。 |
| 関連nextest（`test-script`） | exit 0、628/628成功、15,408 skip、実行64.533秒（compile込み398.728秒）。`focused`、`focused-junit.xml`。歌詞・音響調整・音声/DSP・decoder・settings_transfer・preferencesを含む。 |
| `scripts/test-lib-full.ps1 -Features test-script`（指定の1回） | exit 100、15,965実行／15,962成功／3失敗／71 skip。nextest763.145秒、wrapper1034.428秒。`lib-full`。run `fe26844e-dd56-4983-a56d-ce7188e20e66`、完全なJUnit/logは `target/test-lib-full/runs/20261101T154623294Z-ef8030c2/`。成功台帳は作られず、full PASSとはしない。 |
| 上記で発見した入力テスト修正の関連nextest | 112/112成功、15,924 skip、106.672秒。`ordered-input.log` / `ordered-input-junit.xml` / `ordered-input-observed.json`、run `effb24aa-6503-42f9-bd00-cf353acc9c32`。ratingnav649、§1.680、共有helperを使う履歴親経路、§1.679を検査。テスト成功後のXMLコピーがdisk fullで失敗し記録helperはexit 1。元の完全なJUnitを回収・照合した。test成功と記録helperの失敗を混同しない。 |
| 同じhelperの表示読込consumer（§1.674） | exit 0、1/1成功、実行12.857秒（compile込み15.700秒）。`ordered-input-display` / 同JUnit。PDF/通常画像・縦/横連結・animation有無の同じhandlerを確認。 |
| `cargo test --test ui_snapshot` 再実行 | exit 0、228/228成功、実行36.21秒（compile込み79.851秒）。`ui-snapshot-r2`。§1.175の例外停止なし、未完了テストの補完不要、基準PNGの変更なし。 |
| 各crateのnextest（本体/launcher除外） | exit 100、1,550実行／1,548成功／2失敗／5 skip、38 binaries。実行122.869秒（compile込み345.235秒）。`crates-r2` / 同JUnit、run `323d14b4-6d44-486c-9612-f9f69a17dc8b`。VST3 tester・MCP/IPC・musicvideo等を含む。失敗2件は下記named pipeの既知制約で、その他は全成功。 |
| 各crateのdoctest（同じ2packageを除外） | exit 0、176.940秒。`crates-doc-r2`。実行0件で、実行テストの成功数には加えない。 |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | exit 101、190.154秒。`full-gate`。launcherの同じ承認manifest不足によりcompile準備で停止。テスト実行／vendor／doctestへ未到達。 |
| Python分析回帰 / 配置回帰 / dev安全性 / UI glyph | 全てexit 0。Python4/4、Sound配置3経路のfixture成功、process操作をmockしたdev安全性成功、危険glyph 0。`python-analyzer` / `staging` / `dev-safety` / `glyphs`。 |

再作成したhostは1,228,800 bytes、SHA-256
`c348d193cfdbde1005db84e045472ef002a58560fde13318c345d848a777242f`。
`host-artifact.json` に保存した。最終JUnitの件数・失敗名は `final-results.json` に集約し、
取り込み差分37ファイルの最終SHA-256は `source-hashes-final.json` に保存。
marker残存0、`git diff --check` exit 0、HEAD/MERGE_HEADは開始時のままと確認した。
本体全件の後のRust変更は上記master由来のtest-only 2ファイルだけで、112件＋共有consumer1件の追加検証を適用する。

本体全件の失敗は次の3件に限定された。
`agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry` と
`agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers` はnamed pipe作成時のWin32 error 5
（`src/agent_ipc/pipe.rs:334` / `tests.rs:471`）。サンドボックス外では通るという設計担当の既知報告と
一致するが、本作業では外側で再実行していない。残るratingnav649の旧positive controlは上記のtest-only差分で修正し、
関連112件を再検証した。本体全件の再実行は行わず、元の失敗結果と修正後の結果を分けて保存する。
§1.668 / §1.677 / §1.679 / §1.680、設定分類/転送、MCP/AI連携の本体回帰も全件選択に含む。
既存ignoredは実行せず、skipを成功数へ加えていない。

crate側の2失敗は `mimageviewer-mcp` の
`pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked` と
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`。
どちらも `crates/mcp-server/src/pipe.rs:499` で `raw_os_error=Some(5)`、named pipe clientのopenが拒否された。
本体側の同じ制約2件と分けて数え、テストのignore化や期待変更は行っていない。

検証途中の容量不足も区別する。最初の `cargo test --test ui_snapshot` はlink時の
`LLVM ERROR: IO failure on output stream: no space on device` でexit 101、136.820秒、テスト未実行。
各crateの初回nextestはcompile中にrustcの `STATUS_ACCESS_VIOLATION`（local-adjust-core）で停止し、
記録helperのXMLコピーもdisk fullで失敗。doctest初回もos error 112 / no spaceでexit 101、7.741秒、未実行だった。
これらはテスト本体の失敗や§1.175の実行時例外には数えない。

未使用の `target/dev-runtime/incremental` と、古い `target/debug/incremental/mimageviewer-18cy3t3jl6zg2` を
reparse point 0と確認してNTFS圧縮（ファイル内容保持、削除0）した。
元の論理サイズは12,497,901,995 / 9,305,070,547 bytes、圧縮後の記憶量は8,528,831,127 / 5,521,649,664 bytes。
空きは観測最少735,211,520 bytesから7,392,731,136 bytesへ回復。
最初に試みたcache削除は実行ポリシーで拒否され、実行していない。
`cache-*-preflight.json`、`cache-dev-compress-summary.txt`、`cache-debug-compress.log`（UTF-16）、`cache-space.json`に記録。
追加のcompileの容量も確保するため、同じprofile内のcache `mimageviewer-0jv6ymp3zjw02` と、
検証済みlib-testのcache `mimageviewer-3fzmlkzbrfl24` にも同じ検査後の圧縮を行った。
論理サイズ5,799,159,703 / 9,651,075,440 bytesは保持し、記憶量3,774,144,512 / 5,719,871,488 bytesへ縮小。
`cache-debug-{extra,test}-preflight.json` / 同 `compress.log`（UTF-16）に記録する。exe/DLL・検証記録・data directoryは圧縮/削除対象外。
不足を解消した再実行は `CARGO_BUILD_JOBS=2` とし、global設定・期限・テスト期待・golden画像は変更しない。

### 候補版を作るために残っていること

1. **歌詞受信側の移植**: `G:\home\miv-sound` のpluginソースには、今回送る `lyricsSidecar` /
   `EffeTune.PlaybackPosition` の受信がまだない。旧 `effetune-mixwright-trial` の変更を、
   mIV Soundの名称・ID・専用保存先・現行ProcessContextと両立する形で別リポジトリへ移植・レビュー・検証する。
   本worktreeのmergeだけで歌詞を表示できる候補とはしない。
2. **正式なSound候補**: §2のビルド環境制約を解消し、同じ候補に結び付いた完全なctest証拠を作る。
   利用者の明示承認後に `vendor/miv-sound/VERSION`、bundle、notice、承認manifest/provenanceを揃える。
   検査専用snapshotを承認扱いにしたり、配布入力へコピーしたりしない。
3. **release入力と残りのgate**: 正式入力を使ってcore→Remote→EPUB→MCP→editor→launcherの順で
   release buildを行い、launcher込みworkspaceの型検査・全体gate・埋め込み/展開検査を完走する。
   v5候補版の版番号・更新履歴・同梱文書も設計担当/公開担当が確定する（本mergeでは4.10.2を変更しない）。
   今回は変更したVST3 bridgeを含むため、dev buildだけを確認成果物として渡さない。
   正式入力が未配置のため、本作業では新しいuser-runnable release確認バイナリを作成していない。
   検証後のG:空き容量は約0.97 GBまで減っているため、release build前に未使用build cache等を整理して容量を確保する。
   本作業ではcache削除が実行ポリシーで拒否され、上記の内容保持の圧縮まで行った。
4. **候補版の受入**: SoundのF3/F4等の未完了範囲、musicvideoのGUI受入、MCPとの共存を設計担当が整理する。
   手元/サブPC用の使い捨てデータで、歌詞の停止・seek・速度・曲切替・Remote抑制、
   ユーザーVSTの既存WebViewデータ、Sound窓タイトル・initializeエラー、masterの関連UIを確認する。
   実アプリ試験は別途具体的なsuiteの明示承認後とし、今回の自動検証では未実施。
5. **引き渡し**: 設計担当がdiffを確認してgit add / merge commitを行う。masterへは統合しない。
   再ビルドしたhost exe＋CRTのツリー外backupも、書き込み範囲外のため設計担当へ引き継ぐ。

同じ設定/UI/永続形式の衝突に関する判断待ちはない。旧歌詞受信実装をどのmIV Sound候補へ
取り込むか、正式承認と実機受入はこのmergeのファイル解消とは別の未完了事項である。

## 5. master (v4.11.0) と mcp-trial の最新の取り込み（2026-11-02）

- 設計担当 ClaudeCode が実施。master `3a193e50d` (v4.11.0 公開後。§1.677〜§1.685 の記録、RAW 縮小画像の再要求ループ修正、`deploy-site.ps1`) を `9d57d16c4` で、mcp-trial `bf70f3797` (サブ PC での利用例確認 依頼 4・5、ローカル VLM 説明文計画の下書き、VLM 評価スクリプト) を `87385aea0` で取り込んだ。どちらも衝突なし。版は master の 4.11.0 になった (v5 の版番号は候補版で決める)。
- 検証: `MIV_SOUND_CANDIDATE` を §2 の検査専用入力 (`dcdfa794…6d34`) にして `test-lib-full.ps1`。15,834 件中 15,833 件成功、71 skip。失敗 1 件 `app::tests::history_parent_1473::section1473_smart_and_collection_physical_entrances_keep_native_root_and_origin` は全件時 27.6 秒で失敗し、単独では 3 回とも成功 (4.2〜4.7 秒)。負荷時の期限切れと判断し、製品の不具合とはしない。`MIV_SOUND_CANDIDATE` を付けないと build.rs が mIV Sound 通知の欠落で止まる (正式候補の未配置。§4 の残作業 2)。
- この取り込みで §4「候補版を作るために残っていること」は変わらない。
- 2026-11-02〜03 追記: §1.684 (`343d3afd8`)、§1.686 (`a59734ce3`、取り込み `a3449d245`)、§1.683 (`498a7e901`、取り込み `bbf71d612`) を master 経由で取り込んだ。§1.683 は `src/remote_ipc/collections.rs` の同じ位置に両側がテストを足して衝突したため両方を残し、v5 にだけある `src/app/agent_metadata_write_tests.rs` の `BookOpPending` に新しい `shelf_scope: None` を足した。 v5 での検証: 製本・本棚・Remote 本棚・MCP 書き込み・フォルダツリーの焦点 1,397 件と `ui_snapshot` 236 件が成功。lib 全件は候補版の前にまとめて流す。

## 6. v5 に入れる範囲（2026-11-02 利用者決定）

- 統合済みの 4 本: MCP 対応 (`mcp-trial`)、mIV Sound (`claude/miv-sound-f1-a7147d`)、音楽動画の書き出し (`musicvideo`)、歌詞の受け渡し (`effetune-lyrics-trial`)。§4 の残作業 (歌詞受信側の移植、正式 Sound 候補と承認、launcher 込みの release gate、候補版の受入) を済ませる。
- 追加: §1.686 左ツリーの外部変更への追従、§1.684 フォルダバーで引用符付きパス、§1.683 製本の並び順、ローカル AI 説明文 (`docs/local-vlm-caption-plan.md`)。
- 進め方: §1.686・§1.684・§1.683 は v4 のコードだけで完結するので、master から作業ブランチを切って master へ入れ、master → v5-integration の取り込みで v5 に入れる (v5 の build は正式 Sound 候補が揃うまで通常データで起動できず、実機確認を master の build-dev で頼むため)。master から v4 系の追加リリースは予定しない (2026-11-02 利用者方針)。ローカル AI 説明文は MCP 担当のセッションが mcp-trial で進め、v5-integration へ取り込む。
- 追加 (2026-11-02 利用者要望): mIV Sound の Visualizer に「mIV Original」プリセットを足し、初めて開いたときの表示を Visualizer のこのプリセットにする。内容はタイトル・サムネイル (title / artist / album / artwork)、歌詞があれば歌詞 (lyrics)、スペクトラム (spectrum)、Harmony Lab プリセットの左側の鍵盤と音階が縦に流れる表示 (notes)。既存の部品の組み合わせで、デザインは利用者と相談しながら別セッションで作る。G:\home\miv-sound のソースが変わるので、正式候補は作り直し、承認はプリセット完成後に 1 回にまとめる (今の候補 195326d4… はサブ PC の依頼 4 で確認だけ行う)。既存利用者の保存済みの表示選択は上書きしない。

## 7. mIV Sound 候補 195326d4… のサブ PC 確認 (依頼 4、2026-11-02)

観測者はサブ PC 担当 (記録 `C:\miv-subpc\results-4\`)。ビルドは v5-integration `18f936216` の dev-runtime に候補を `build-dev.ps1 -MivSoundCandidate` で入れたもの、隔離 data-dir。音は聞いておらず、ビープとの比較は HUD の時刻 (1 秒単位)・シークバー・メーターで代用。

- 期待どおり: 名称・ロゴ・クレジット (「mIV Sound version 0.15.0-miv.1 … Based on EffeTune / EffeTune Mixwright by Frieve」)、エフェクトの効き (Mute と Level Meter で確認)、歌詞の同期 (1.0 倍・一時停止と再開・シークバーと ← → ・0.5 倍 / 2 倍・曲の切り替えで前の曲の歌詞が出ない)、文字コード (Shift_JIS)・埋め込み歌詞 (mp3 / m4a)・時刻なし歌詞・歌詞なし、歌詞行のクリックで位置が飛ばない、Lyric Stage (Counter なし)。WebView2 のダイアログ・panic なし。
- C7 (動画の秒数が約 2 秒大きい) は素材の問題: `beep-video.mp4` の pts 0 の画面に元から「2.000000」と描かれている (設計担当が ffmpeg で確認)。mIV の不具合ではない。
- **C6 (不具合ではない)**: 再生画面から Backspace で一覧へ戻ると Visualizer の曲名・歌詞が消える件は、歌詞担当の調査 (コードとサブ PC のログ) で、一覧への退出が `close_fullscreen_now` でプレイヤーを drop して再生自体を終える作りで、`reconcile_local_now_playing` がそれに合わせて Local を clear する一貫した動きと判明。ログでは退出直後から host への音声ブロックが途絶えている (「音は続いていた」はスペクトラムの動きからの推測だった)。v4.6.0 (§1.359 part B) からの作りで v4.11.0 も同じ (コード判断、実機未確認)。一覧へ戻っても再生を続けたい場合は、別の新機能として検討する。
- **A1 (要調査)**: mIV を最大化した状態で mIV Sound の窓が後ろに回ると、音響調整のボタンを 3 回押しても前に出ない (窓は可視のまま)。初回は 2 回目で前に出た。設計 (effetune-integration-plan §17.12「表示・前面化」) と異なる。v4.11.0 でも起きるかは未確認。
- 承認は「mIV Original」プリセットを入れた候補で 1 回にまとめる方針 (§6) なので、この候補は承認しない。

## 8. mIV Sound 候補 4b7a3eba… の承認 (2026-11-03)

- 候補 `4b7a3eba00d3946c591d103eb6adecfba42b6cae3ea3eb3f010958093bf8c0bc` (miv-sound `miv` ブランチ、「mIV Original」プリセットと初回表示を含む)。ctest 117/117、独立レビュー ACCEPT。
- 利用者が v5 の dev build で、初回表示・字幕の読みやすさ・マイクのダイアログが出ないこと・アナライザーの 4 点を確認し (観測者: 利用者)、承認を指示した。
- `.\scripts\build-miv-sound.ps1 -Approve -Candidate 4b7a3eba…` を実行し、`third_party/miv-sound/0.15.0-miv.1/` を `636b2936b` でコミット。バックアップは `C:\home\mimageviewer_vendor_backup\miv-sound-0.15.0-miv.1\`。以後の v5 の検査に `MIV_SOUND_CANDIDATE` は要らない。
- 承認後の `cargo check --workspace --all-targets` は、承認物の検査を通過し、launcher が release の exe 5 本を要求するところで止まった (未ビルドのため。公開前ゲートで `build-release.ps1` から作る)。

## 9. mIV Sound 0.15.0-miv.2 (候補 4d2a0fc3…) の承認 (2026-11-03)

- 内容: 「起動時の表示」に「前回の表示 (デフォルト)」を足し、新しい利用者の既定にした (記録が無いときは Visualizer の mIV Original)。Effects の選択肢から「(デフォルト)」を外した。保存状態の無い初回に作る既定のパイプラインの Volume を -6 dB から 0 dB にした。既存の利用者が選んだ起動時の表示と保存済みのパイプラインは変えない。
- 経緯: v5 全般チェック (サブ PC 依頼 7、観測者: サブ PC 担当) で、再起動すると毎回 Visualizer で開くことが分かり、利用者が「前回の表示を残す」を選んだ。候補 302a5bef… を利用者が確かめた際に (観測者: 利用者)、「(デフォルト)」の表記と、初めて開いたときに音量が 6 dB 下がることの指摘があり、0 dB にすることを利用者が決めた。
- miv-sound `miv` ブランチ: `4570657` (前回の表示)、`1df923f`・`f6efd03` (テスト)、`295db5a`・`026df1c` (表記・0 dB・ビルドの安全策)、`a97108e` (版 0.15.0-miv.2)。0.15.0-miv.1 は承認済みで同じ版を上書きできないため、版を上げた。mIV 側は `config/miv-sound.rs` の `VERSION` を合わせた。
- 独立レビュー (gpt-6.1-sol / xhigh): 前回の表示は ACCEPT (P3 1 件はテスト追加で対応)。表記と 0 dB は 1 回目 REJECT (P2 locale 置換の欠落でビルドが止まらない、P3 文書の書き方) → 対応後 ACCEPT。
- ctest: 候補 `4d2a0fc3be02aa77c59ceedbb9a454f628adbe83ecc778217b0c8f9391b5d541` で 117/117。途中の候補 `302a5bef…` (`1df923f` で ctest 2 件失敗、テスト側の誤り)、`2c00671d…` (中断したビルドが残した記録) は `target/miv-sound-candidates-rejected/` へよけた。
- 利用者の確認 (観測者: 利用者、v5 dev build に候補 `302a5bef…`、続いて `2c00671d…` を入れたもの。`4d2a0fc3…` との違いは版番号の表示だけ): 初回の Visualizer、再起動後の Effects / Visualizer の復元、明示値の優先、初回の音量が下がらないこと、起動時の表示の表記。
- `.\scripts\build-miv-sound.ps1 -Approve -Candidate 4d2a0fc3…` で `third_party/miv-sound/0.15.0-miv.2/` を作成。バックアップは `C:\home\mimageviewer_vendor_backup\miv-sound-0.15.0-miv.2\`。旧 `third_party/miv-sound/0.15.0-miv.1/` は残している (削除は管理担当の判断)。

## 10. 配布構成の試走 (2026-11-03、利用者の指示)

v5-integration `4cecdcd57` (mIV Sound 0.15.0-miv.2 承認後、§1.693・§1.694・§1.695・§1.696 を含む)。公開用の `build-dist.ps1` ではなく、署名・インストーラ・ポータブル版を作らない試走。ローカル AI 説明文 (vlm-caption) は未取り込みで対象外。

| 段 | 結果 |
| --- | --- |
| `build-release.ps1 -PreserveRuntime` | 成功。core・remote・EPUB・MCP・editor・launcher (単体 exe 541,201,408 bytes)。VC runtime gate は runtime=4 / pe=6 と embedded pe=12 の両方 passed。VST3 bridge は C++ 変更なしで既存 exe (11-02 00:34) のまま |
| `test-full.ps1` 1 回目 | テスト実行前に停止。テスト実行ファイルの並列ビルドで commit 上限に達し `os error 1455` (ページング ファイルが小さすぎる)。metadata の読み込み失敗が `E0786`/`E0460` として大量に出るが、ソースの不具合ではない |
| `test-full.ps1` 2 回目 (`CARGO_BUILD_JOBS=6`) | 18,012 件中 18,011 成功、77 skip、1,035 秒。失敗 1 件 `archive_conversion_failure_diagnostics::locked_zip_publish_reports_plain_message_and_native_error_without_overwrite` (ログのフォルダが見つからない NotFound) は単独で成功。同時実行の影響と見ているが、何がフォルダを消したかは未調査 |
| `test-lib-full.ps1 -Features test-script` | 16,125 件中 16,123 成功、71 skip。失敗 2 件 (`page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`、`phase_c_folder_nav_history_tests::section1328_pdf_history_back_control_preserves_parent_position`) は単独の再実行 1 回目で exit 100、2 回目で 2 件とも成功。不安定なテストとして扱う |

- 公開前の全体テストでは、メイン PC でほかのアプリが動いている場合に備えて `CARGO_BUILD_JOBS` を絞る (6 で通った)。`build-dist.ps1` も同じ並列ビルドを行う。
- 残り: ローカル AI 説明文 (V2 検索・V3 MCP) の実装と取り込み、版 5.0.0 と更新履歴、公開文書、`build-dist.ps1` での本番ビルド・署名、配布物での受入、Phase 2 の依存物確認・perf smoke (`ui-smoke.ps1 -Scenario ReleaseChecks`)・panic log の棚卸し。

## 11. mcp-trial (ローカル AI 説明文 V1〜V3) の取り込み（2026-11-03）

- 作業先: `G:\home\mimageviewer-badge-1663` / `v5-integration`。
- 取り込み前 HEAD: `deb712d04dedcf65bf595a083fc4e4de4a3dfe8a`。
  MERGE_HEAD: `d982c85c34cc8681defcf03e29966344b232546e`（`mcp-trial`、62コミット）。
- 設計担当が開始した `git merge --no-ff --no-commit mcp-trial` のファイル編集のみ。
  git add / merge commit / その他のgit書き込み、製品起動は行わない。
  本節の検証の観測者はCodex実装担当。mIV Sound 0.15.0-miv.2の承認物を使用し、
  検査専用候補への差し替えは行っていない。
- §6の範囲を保持する。VLMの取り込みはモデル公開・配布物の作成・受入の完了を意味しない。
  本体lib全件、build-release、build-devは依頼どおり設計担当の検証範囲に残す。

### 衝突の解き方

| ファイル | 解消内容 |
| --- | --- |
| `docs/detached-rework-plan.md` | §1.670の独立reader終了境界の追補とV1-4a/V1-4bの表紙解決・検索分岐の記録を全文保持。今回新しいdetached述語・viewport処理は追加しない。 |
| `docs/keymap-spec.md` | 音楽ビデオのメニュー専用操作・editorの固定キーと、AI説明文作成のメニュー専用操作を併存。既定キー割当を増やさない。 |
| `docs/spec.md` | v5側の本棚順序・RAW foreground仕様と、VLMの保存・表示・検索・MCP仕様を保持。V2時点のMCP検索除外の説明は履歴とし、現在のV3の許可による制御へ参照する。 |
| `docs/ui-responsiveness.md` / `docs/ui-snapshot-policy.md` | 両側の独立した追補を保持。mIV Soundの設定・creditとVLMの取得・情報パネル等の検証履歴を併存。 |
| `htdocs/mimageviewer/index.html` / `privacy.html` | musicvideo・Everything・AI連携・AI説明文を保持。AI説明文の許可と本文の直接取得の違い、ローカル保存・モデル取得・外部AIアプリへ渡す境界を維持。mIV Soundの名称・分離保存先・歌詞等の受渡しを現コードへ照合して訂正。 |
| `htdocs/sitemap.xml` | `python scripts/gen-sitemap-xml.py` で再生成。両側の音楽ビデオページとAIチュートリアル2ページを含む216 URL。 |
| `scripts/build-release.ps1` / `scripts/build-portable.ps1` | v5のcore→Remote→EPUB→MCP→editor→launcherのRust順、mIV Sound承認照合、`-PreserveRuntime`、editor非停止とVC runtime gateを保持。core前のVLM build/identity/署名、通常core内包、portableのloose配置/署名/PE検査を統合。 |
| `src/lib.rs` | AI失敗feedback fixtureとWindowsのjob_process、musicvideo_launchとvlm_addon/downloadをそれぞれ併存。 |
| `src/app.rs` | RAW foreground用thumbnail_is_foregroundを保持し、既存の変換書庫resolverを表紙解決から参照可能にする。本の追記はworker内でai_caption.dbの存在を調べ、v5の本棚順序処理を含むappend_pages_at_with_raw_and_shelfへ渡す。 |
| `src/settings_transfer.rs` | 新しい説明文許可を移行対象から除外。統合後は総数486・転送155・除外331。転送形式は1のまま。 |
| `src/thumb_loader.rs` | VLM側で切り出した共有read resolverを通常サムネイルにも使用。v5側のcatalog provider、RAW foreground、要求identity、source policyを保持。memory pin seedの読出し拒否は既存uses_memory_pin_seedへ統一し、source解決を二重に残さない。 |
| `tests/snapshots/preferences_context_menu_layout{,_open_with}.png` | 片側の画像を採用せず、解消後のlib fixtureだけをUPDATE_SNAPSHOTS=1で再生成する。結果と目視内容は下記。 |

### 自動 merge 後に直した食い違い・確認した境界

- `src/app/caption_display_tests.rs` のBookAppendSummary fixtureにv5側の `shelf: None` を補完。
  初回の関連nextestはこのE0063でcompile中に停止し、テストは未実行だった。製品仕様や期待値の緩和ではない。
- 設定分類の片親の件数485/483をそのまま採らず、新欄1件をv5側の分類へ合流。
  メタ情報の形式はHEADの11からVLM側の12（AI説明文）へ、アイテム索引は10から11へ更新。
  両側が別用途で同じ番号へ上げた衝突は無く、追加の版上げ・保存形式の再定義は不要だった。
  旧形式11以下ではai_captions欄を無効にする取り込み契約も保持。
- 自動mergeされたcontext_menu_modelのCreateMusicVideo / CreateAiCaptions、環境設定のSound項目と
  AiCaptionページ・即時許可の写し、AboutのSound credit / VLMライセンスを照合。
  `video/dsp/extract.rs` は既存のregular-file/ディレクトリ検査をpub(crate)で共有する変更だけで、
  Soundの内容アドレス展開とVLMの展開を別々のdata_dir配下に保つ。
- root `build.rs` は両build flavorでVLMを必須入力にし、release identityを検査する。
  launcherはworkerを内包したcoreを包むため、VLMを別のlauncher assetへ追加する必要はない。
  build-devも通常coreの同じ内包経路を使う。PE検査器は渡されたPEを列挙するため、呼出側の
  VLM追加で対象になる。`build-dist.ps1` にVLM identityのfixture回帰と最終照合を補い、
  `docs/release-checklist.md` の配布形態・署名・PE検査も合わせた。
- 公開2ページは「作成処理で画像を外部へ送らない」と「許可時に説明文をAIアプリへ渡す」を区別。
  mIV Soundは `<data_dir>/effetune/plugin` / `effetune/webview` を使い、旧共有先はコピー元として残る。
  旧 `%APPDATA%/mimageviewer-vst3-host.exe` は利用者追加VSTが現在も使い得るため、自動削除を案内しない。
  これは今回の公開主張の整合化であり、F3全体の改稿・マニュアル再編の完了とはしない。

### 検証

ログとcommand/exit/秒のJSONは `target/v5-vlm-merge/`（git管理外）。
Rust検証は通常feature、`CARGO_BUILD_JOBS=6`、`CARGO_NET_OFFLINE=true`、
`MSBUILDDISABLENODEREUSE=1`。製品・実モデル・WebView2の実試験は行わない。

最初のcore checkはvendor workerの不足で停止した。文書の復旧入口
`scripts/vlm-worker-identity.ps1 -RestoreBackupRepo .` でバックアップを実体コピーし、
source identity `79c6482ae05f4aee923ff0b207af1386462658546a431f6e2bc1685331641fd9` を照合。
workerは48,105,472 bytes、SHA-256
`15921e46f0e5a1f8b06a059852b437fa44730962a01303df098454ba23078798`。
再core checkはexit 0、81.076秒。workerの起動やバックアップへの書き込みは行っていない。

| 検査 | 結果・証跡 |
| --- | --- |
| `cargo fmt`（引数なし）/ `cargo fmt --check` | exit 0 / 0。fixture補完後の最終fmtは21.581秒、checkは23.744秒。`fmt-final` / `fmt-check` のlog/JSON。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 上記復旧後にexit 0、81.076秒。`check-core-final.log` / `.json`。 |
| `cargo check --workspace --all-targets` | exit 0、198.268秒。`check-workspace.log` / `.json`。§10の既存release入力を使った型検査であり、VLM取り込み後のreleaseビルドや内包・展開の実行検証とは区別する。 |
| 関連lib nextest | 891件実行、889成功・2失敗、exit 100。実行60.571秒、compile込み64.131秒。run `6d43d3b6-ddaa-4ec5-aa8b-4ceabd8bc585`。`tests-related-final.log` / `.json` / `.junit.xml`、集計は `coverage.json`。失敗は下記の既知pipe制約のみ。 |
| `cargo test -p mimageviewer-mcp` | lib 67件中65成功・2失敗、exit 101、36.318秒（実行30.02秒）。`test-mcp.log` / `.json`。同じ既知pipe制約で、bin/doctest段には進まない。 |
| `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib preferences_context_menu_layout_snapshot -- --test-threads=1` | 1件成功、既存PNG2枚を再生成。exit 0、387.931秒（compile6分25秒、実行2.15秒）。`snapshot-update.log` / `.json`。更新指定なしの同テスト比較は上の891件に含まれ成功（1.785秒）。 |
| `cargo test --test ui_snapshot preferences_context_menu` | exit 0、162.913秒、236件がfilter対象外で実行0件。成功件数には数えない。`snapshot-integration.log` / `.json`。実際の生成・比較は上のlib fixtureで完了した。 |
| `scripts/test-focused.ps1 -Path <衝突16ファイル>` | exit 2、0.780秒、テスト実行なし。app/lib・scripts/docs・snapshot等の広域/未対応入力を仕様どおり全列挙して選択を拒否。`focused.ps1` / `.log` / `.json`。本体lib全件へ自動拡大せず、上の明示filterで検証した。 |
| VLM実行体のPE静的検査 | exit 0、runtime=4 / pe=1、1.387秒。`vlm-pe.log` / `.json` / `vlm-pe-report.json`。workerを起動しない。 |
| build scriptのfixture検査 | `test-vlm-worker-identity.ps1`、`test-release-build-safety.ps1 -PowerShellOnly`、`test-musicvideo-build-safety.ps1`、`check-vcrt-pe-dependencies.ps1 -SelfTest` はすべてexit 0。実装の別contextが担当し、ツール実行履歴の結果を再利用（専用logファイルなし）。実build・署名・process停止は行わない。 |
| UI glyph / 公開文書 | glyph 0、exit 0、`glyphs.log` / `.json`。文書担当のカード検査は51件・超過0、HTML2枚の構造/ID/ローカル参照59件・sitemapのXML解析も成功。sitemap `--check` は216 URLで成功。 |
| 独立レビュー | GPT-6.1 Sol / xhigh、実装と別context、読み取り専用。両親との差分、Rust衝突、自動merge近接経路、版・設定・Job・配布・公開文書を照合しACCEPT、未解決P1〜P3なし。PNG2枚と891件のJUnit/集計・hashも別途照合。検証コマンドの再実行なし。 |

関連nextestは依頼の4 filterをORでまとめ、共有read resolverの回帰としてthumb_loader群と
RAW foregroundの関連名も加えた。群ごとの件数は重複ありで、VLM/caption234成功、
settings_transfer/metadata_transfer152成功、context_menu166成功、agent/mcp237成功・2失敗、
thumb_loader120成功。これらを足して総実行数としない。
分類486/155/331、説明文許可の移行除外、メタ情報の旧版互換、captionの保存・索引・表示・MCP許可、
サムネイルの共有解決とCollection providerの回帰に失敗はなかった。

今回の失敗4件は、いずれもfixtureのnamed pipe作成/openで `OS 5 / PermissionDenied`。
本体側2件は `src/agent_ipc/pipe.rs:334` / `src/agent_ipc/tests.rs:757`、
MCP側2件は `crates/mcp-server/src/pipe.rs:499`。本体stdio fixtureはadapterの起動前に失敗した。
取り込み元のV3と本記録§1のsandbox内制約と同型だが、今回の統合後ソースをsandbox外で
実行した証拠はまだ無い。skip化・期待値変更・DACL緩和・反復再試行は行わない。

- `agent_ipc::pipe::tests::shutdown_interrupts_an_idle_connected_reader_without_retry`
- `agent_ipc::tests::stdio_exe_uses_real_core_fixture_handlers`
- MCP `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked`
- MCP `pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`

### 再生成PNGの目視

2枚とも620×700。`preferences_context_menu_layout.png` は168,289 bytesで、
「AI 説明文を作る」がファイル整理先の直後にあり、既存のチェック・上下移動・区切り線の操作列が保たれる。
`preferences_context_menu_layout_open_with.png` は159,378 bytesで、末尾の「音楽ビデオを作成」と
「アプリケーションで開く…」内の「外部ツールの設定…」がある。
確認場面が画像なので音楽ビデオ行は「対象外」。新旧の両操作が設定表へ共存し、文字欠けや操作列の重なりがない。
両方を実装担当と独立レビュー担当が開いて確認した。
`snapshot-final-hashes.json` に両親と現在のhashを保存し、2枚ともどちらの親とも異なることを照合した。
純粋なheadless fixtureの検査であり、製品GUIの起動や実機受入とは区別する。

### 判断待ち・引き渡し

設定・メニューの位置・保存形式・版番号の意味に関する判断待ちはない。
衝突16ファイルの内容は解消済み。indexは設計担当のgit add前なので16件ともUUのまま。
本体lib全件、最終full gate、build-release / build-dev、統合後のpipe4件のsandbox外確認と
配布物/実機での受入は設計担当へ引き継ぐ。今回のチェック成功をこれらの完了とはしない。
依頼に従いユーザー起動用バイナリは作らず、製品・実モデル・WebView2・通常profileを起動していない。
git add / commit / merge等のgit書き込みも行わない。

最終の差分空白検査・全追跡ファイルの衝突marker検索・HEAD/MERGE_HEADの照合は
`final-checks.json` に記録した（差分検査exit 0、marker 0、両tip不変、indexのUU 16件）。
統合後の全変更172ファイルのSHA-256は `final-source-hashes.json`。
型検査・関連テスト以降の追加変更は本節とsnapshot方針の文書だけである。
