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
