# egui 0.37 更新 — 段2 検証台帳

## 段2a-1（2026-10-17）

対象HEAD `2b8f779a8916fdf8241e878a1a8056da30e19b4a`、branch `egui-036`、
worktree `G:\home\mimageviewer-egui036`、Rust 1.99.0。
設計第4.4版の段2aを、利用者指定の2a-1／2a-2／2a-3へ分けたうち最初の実装である。
以下は**実装担当の内部確認**（別contextでのソース確認・指摘修正を含む）。
正式な独立レビュー・設計担当の受入れ・実機確認を主張しない。

### 範囲とgraph

新しい実ディレクトリ`vendor-next/`を作った。junction／symlink／reparse pointは使わない。

| 構成 | 内容 |
| --- | --- |
| `vendor-next/Cargo.toml` | rootから独立したworkspace。memberはegui、egui-winit、egui-ime。egui／egui-winitを同ディレクトリのpathへpatch |
| `vendor-next/Cargo.lock` | offlineで解決した準備graph専用lock |
| `vendor-next/egui` | 0.37.0原本＋既存API・回帰の付け直し |
| `vendor-next/egui-winit` | 0.37.0原本＋キー配送・Disabled・trace・clipboardの付け直し |
| `vendor-next/egui-ime` | 共有policyの新版準備専用コピー。製品側の実装・manifestは変更しない |
| `vendor-next/upstream-provenance.json` | archive、checksum照合元、VCS、license、原本全ファイルhash |
| `vendor-next/import-upstream.py` | offlineで原本だけを再配置するscript。既存宛先は上書きしない |

rootのCargo.toml（依存版／patch／workspace）、Cargo.lock、toolchain、旧vendor、製品Rust／テストは
変更していない。root graphのegui-winitは0.33.3、eguiは旧`vendor/egui`、共有crateは
`crates/egui-ime`のままである。root metadataのworkspace memberにvendor-nextは含まれない。
準備metadataのeguiとegui-winitは各0.37.0のlocal一実体であり、旧0.33やregistry eguiとの型分裂はない。

### 原本の記録と最初の区切り

取得元は既存Cargo registryの次の実ファイルだけ。ネットワークは使っていない。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\{egui,egui-winit}-0.37.0
C:\Users\mikag\.cargo\registry\cache\index.crates.io-1949cf8c6b5b557f\{egui,egui-winit}-0.37.0.crate
```

| package | archive SHA-256 | 原本file数 |
| --- | --- | --- |
| egui 0.37.0 | `bec9144f49b7a28b5f395d3147e325b4fe3e77ba686eac11d1bdf0a7da3f375f` | 137 |
| egui-winit 0.37.0 | `e4e449f4c4a2a60c7ccb4ba7fa4078417bfe1f9ddc03b5e05ce75748e33a70de` | 11 |

両archive hashはローカルsparse indexの0.37.0エントリ`cksum`と一致した。
archive内全148ファイルと展開registry sourceの全bytesを照合してから原本を配置した。
上流は`https://github.com/emilk/egui`、VCS commitは両方とも
`7c0b40402271e8d40cda69f87cf3c2617c2e4382`、pathはそれぞれ`crates/egui`／`crates/egui-winit`。
原本の宣言`MIT OR Apache-2.0`を維持する。
archiveにlicense本文がないため、差分適用側で旧vendorの同名本文を無変更コピーした。
MIT SHA-256は`3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff`、
Apacheは`1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec`。
registry管理用`.cargo-ok`、target、archive外の管理ファイルは取り込まない。
`.cargo_vcs_info.json`はarchive自体に含まれる原本情報として保持した。

**原本配置直後を最初の区切りとし、全原本hashを記録してから付け直した。**
利用者指示によりgit add／commitは実行していない。
現在、原本から変更したファイルは次の5つだけで、他143原本はbyte一致する。

| crate | 原本から変更したfile | 差分側の追加file |
| --- | --- | --- |
| egui | Cargo.toml、src/context.rs、src/input_state/mod.rs | PATCHES.md、LICENSE-MIT、LICENSE-APACHE |
| egui-winit | Cargo.toml、src/lib.rs | PATCHES.md、LICENSE-MIT、LICENSE-APACHE、src/disabled_ime_tests.rs、src/keyboard_input_tests.rs、src/ime_key_trace.rs |

両README、両Cargo.toml.orig、両同梱Cargo.lockは**原本のまま**である。
旧Cargo.tomlを新版へ上書きしていない。新版manifest／feature／依存版を基準に必要行だけ追加した。
原本一覧・全hashの正本はupstream-provenance.json。
コミットを分ける設計担当向けに、上記5ファイルの原本／適用後の実コピーを
`target/egui037-stage2a1-handoff/{original,patched}/{egui,egui-winit}/...`へ残した。
原本commitでは一覧の148ファイルと原本台帳を対象にし、追加file・準備graph・共有crateコピーは
差分側へ分けられる。新しいcheckoutではimport-upstream.pyで同じ原本を再現できる。

### 付け直した差分と分類

分類(a)/(b)/(c)は設計書§4.8に従う。詳細は各新版PATCHES.mdにも記録した。

| 対象 | 分類 | 付け直し・変更理由 |
| --- | --- | --- |
| egui release/click-count射影 | (b) | 本家分類済みの全button releaseを元順で返す。非clickのNone、focus区間、discard後非再生を維持 |
| 旧クリック分類依存fixture | (c) | 旧遠隔位置列のcount固定を外し、近接1/2/3/3、遠隔全1、距離6 pointsと時間/triple境界を別検査。分類自体は本家(a)を使う |
| discard_pending_scroll | (b) | 新WheelStateのunprocessed_wheel_deltaだけを終端。現在delta・event・gesture状態・modifier/sourceは保持。scroll/zoomの残量、idle非再生と新wheel受理を回帰化 |
| repaint delay witness | (b) | miv_requested_repaint_delayとmiv-repaint-witness featureを保持。現在viewport schedulerの非変更読取りを検査 |
| Windows Process基本機構 | (a) | 本家0.37を土台にし、旧機構を二重に追加しない |
| Hidden/Forwarded | (b) | State所有の配送履歴、Forwardedの非降格、許可/logicalに依存しないrelease終端、focus喪失時の対象だけclearを保持 |
| Disabled producer配送 | (b) | 本家が捨てるDisabledを共有正規化へ届ける。Enabledの互換通知も保持し旧Linux例外を維持。Preeditの範囲変換は本家のまま |
| 旧has_sent_ime_enabled/合成通知 | (c) | 新TextEditとnormalizerはPreedit/Commitを直接扱う。旧enable latch・Commit後の合成Disabledを戻さず実OS通知を渡す |
| State→正規化→実TextEdit | (b・新規) | 段2a冒頭で最小回帰を配置し、最初に配送契約を確認 |
| trace・秘匿・worker I/O | (b) | 既定無効、共有時計、追記範囲観測、worker側I/Oを保持。新event形状に適応。新IntlBackslashも文字キーとして秘匿し、invalid rangeログも本文を出さない |
| 配送/State/網羅回帰 | (b) | 旧17件、全3,257,436列を保持。新modifier ownerへ適応 |
| egui-ime edge／共有clipboard | (b) | 新版準備コピーへのpath依存。共有条件、Ctrl+Shift、logical優先/physical fallback、release、CRLF、空paste、early returnを保持 |
| 新modifier／clipboard API | (a) | 本家のModifiersChanged順序、runtime OS判定、clipboard_shortcuts設定、画像paste fallbackを保持し実経路/配送回帰を追加 |
| 旧300ms補正 | (c) | 撤去済み。復活させない |
| license／standalone | (b) | 上記出典・独立workspaceで保持 |

本家0.37のクリック分類を検査したことは、製品操作差の受入れではない。
製品への採用は設計書§5の判断と段3以降の接続・検証を要する。

### Disabled互換と回帰

`State::on_ime`の実変換から準備用egui-ime pluginを通し、実Context::run_ui内のTextEditへ配送した。
実window/EventLoopは作らない。Window必須のgeometry更新だけを除外し、StateのRawInputをtakeする。
正規化は製品の共有policyを新版APIへ移した実装であり、テスト用の模倣normalizerではない。
FullOutputは検査後`drop_without_applying_deltas`でheadless明示破棄する。

最小6回帰が、非空Preedit→Disabledのみ（同一/別pass）、focus喪失後の終端、
Commit後の確定文字保持、重複Disabled/空Commit、通常選択文字、兄弟State/viewport、
直後の通常Enter/Escapeを検査する。focusと通常キーも実State helperを通す。
追加回帰でDisabledが物理holdを消さないこと、Enabled通知、Unicode byte→char範囲と
invalid/None範囲、modifierイベント順、clipboard設定/画像pasteを検査した。

Disabledを本家どおり捨てる変異では最小6件が**2 passed / 4 failed、exit 101**。
復元後は全6件成功し、最終winit lib全件にも含まれる。
初回試験はtest fixture自身がTexturesDeltaを未処理dropして6件失敗したため、
headless出力の明示破棄へ訂正した。これを製品の取消失敗とは扱わない。

内部確認で新IntlBackslashの秘匿漏れを検出し、winit physical・egui logical/physicalの
全formatterを文字キー同等へ修正した。専用回帰は旧分類へ戻す変異で**0 passed / 1 failed、exit 101**、
修正後を含むwinit全件は43件成功。別contextのソース再確認でも未解決指摘なし。
正式な独立レビューのACCEPTではない。

### egui-imeの両版の扱い

製品の`crates/egui-ime`は0.33のまま無変更とし、新版は`vendor-next/egui-ime`へ実コピーした。
cfgや任意依存による単一crate内の型分岐は設けない。
normalizerの状態遷移・focus retry・Tab policy・clipboardの受付差は維持し、
Preedit構造体、Plugin input_hookのContext引数、root Ui、IDの新APIへ適応した。
新DeleteSurroundingはcompositionやfocus喪失後の終端責務を変えない。
通常依存はdefault-features=false、headlessテストのdev依存だけdefault_fontsを要求する。
通常依存だけのtreeにdefault_fonts／epaint_default_fontsがないことを確認した。
clipboard.rsは製品側とbyte一致（SHA-256 `058b6d7e56b236ee9662d475ac26939d8ab8cde2ac4235fbe9034ddb81dec73c`）。

段3では製品側にその後の変更がないか照合し、この移行差分を製品共有crateへ反映して、
root／backendを一つのcrateへ寄せる。producer回帰も正式graphへ接続し、両コピーを残さず
準備コピーと参照を撤去する。詳細手順は新版egui-ime/PATCHES.mdに記録した。

### 検証結果

すべてoffline、MSBUILDDISABLENODEREUSE=1、CARGO_BUILD_JOBS=1。
通常root fmtに準備Rustファイルのbyte変更がないこと、metadataのmember分離、
原本との差分一覧、root保護対象のHEAD内容一致（checkoutのCRLFだけ正規化）も確認した。
rootのfmt対象には準備graphが入らないため、両方を明示実行した。

| 検証 | 結果 |
| --- | --- |
| 準備egui lib（miv-repaint-witness有効） | 92 passed / 0 failed / 0 ignored、exit 0 |
| 準備egui-winit lib（最終trace修正含む） | 43 passed / 0 failed / 0 ignored、exit 0。全3,257,436列の網羅試験を含む |
| 準備egui-ime lib | 26 passed / 0 failed / 0 ignored、exit 0 |
| root cargo check -p mimageviewer --bin mimageviewer-core | exit 0、旧vendor使用 |
| 同 --features test-script | exit 0、旧vendor使用 |
| root cargo fmt／cargo fmt --check | 両方exit 0 |
| 準備cargo fmt --manifest-path vendor-next/Cargo.toml --all／同 --check | 両方exit 0 |
| 準備/root metadata、root winit tree、IME minimal tree | 各exit 0 |
| git diff --check | exit 0（git add禁止のため新fileは未追跡。原本の既存空白は改変しない） |
| archive/index/source、license、原本file別hash照合 | 成功。原本変更は上記5fileのみ |

正確な実行方法はvendor-next/README.md。最終の依存解決を伴うCargo commandには
--locked --offlineを付けた（初回の準備lock生成時だけ--lockedを付けない）。root Cargo.lockは変更していない。
集約command・exit・時間は`target/egui037-stage2a1-results.json`、ログは同prefixの
`{egui-final,winit-final-restored,ime-final,root-check-final,root-test-script-check-final}.log`。
winitの出力破棄統一・trace修正後だけは影響crateを再検証し、他の有効な成功結果を再利用した。
source/graph照合は`*-audit.json`、`*-graph-audit.json`、`*-source.json`、
変異は`*-disabled-mutation.log`／`*-trace-mutation.log`。
共有fixture移植の初回には旧API残り2件でcompile失敗（exit 101）があり、
新keyboard APIとWidgetAtomResponseからのResponse取得へ訂正して全26件成功した。
本家scene.rsの既存f32::EPSILON非推奨警告、rootの既存警告は今回の対象外として残る。

製品graphを変えないため全件lib、test-full.ps1、製品バイナリbuild、liveテストは利用者指定どおり実行しない。
ネットワーク、アプリ起動、git add／commit／branch操作／pushも行わない。

### 残課題・設計書との差

設計前提と実装の重大な食い違いはない。共有crateの両版接続は、許容された一時分岐として
準備コピー方式を選んだ。正式な独立レビュー・設計担当の受入れは未実施。
段2a-2のIME中断要求のviewport所有とdiscard再pass集約、段2a-3の非描画退役入口、
段2b/2c、段3の正式graph切替は未着手であり、段2a全体の完了とは扱わない。
新pending bool・時間窓・追加repaint・製品機能制限は加えていない。

## 段2a-2（2026-10-17）

開始HEAD `8023e4bdd`、branch `egui-036`、Rust 1.99.0。段2a-1は設計担当が
`b5365dc73`（原本）と`8023e4bdd`（差分の付け直し）に分けてコミット済み。
本節はその後の**実装担当の内部確認**であり、段2a-1の並行中の独立レビューや、
今回の正式な独立レビュー・設計受入れ・実機確認を代替しない。
今回もgit add／commit／branch操作は行わず、未コミットで返す。

### 合意・原因・修正境界

設計第4.4版§4.5「IME中断要求のviewport別所有」「discard再passのIME出力集約」、
段2a順序6〜10、detached計画§2と§11の構造合意を読み直した。
両修正は§11で設計上合意済みであり、新しい症状パッチの例外を設けていない。

観測した破れは二つである。

- 同一Contextの実入れ子run_uiで、親の要求が子begin_passのMemory共通bool resetにより消え、
  子の要求が親end_passのIME出力へ混入する。親のみ／子のみの回帰でそれぞれ検出した。
- 実request_discardの第1passだけが要求した場合、PlatformOutput::appendが後のSome全体で
  置換するため、最終出力の要求がfalseになる。新しい矩形・purpose自体は届いていた。

所有の不変条件は「要求を出したviewportの同じpassだけが生成し、そのviewportの未配送出力だけが保持する」。
既存のFocus ownerと既存append境界へ修正を収めた。

| 変更file | 変更内容 |
| --- | --- |
| `vendor-next/egui/src/memory/mod.rs` | Memory共通requested_interrupt_imeを既存viewport別Focusへ移管。Focus::begin_passは自分だけreset。interrupt_imeとgetterは現在Focusだけを扱う |
| `vendor-next/egui/src/data/output.rs` | 同一viewportの新IME metadataを採り、中断要求を旧・新のORで保持。新Noneなら既存Someを保持する。appendの同一viewport契約を明記 |
| `vendor-next/egui/src/context.rs` | 新回帰moduleの登録のみ。end_passの生成・既存viewport復帰のproduction処理は変更しない |
| `vendor-next/egui/src/context/ime_interrupt_tests.rs` | 実入れ子4件、実discard1件、Option/転送の補助1件。全名にime_interruptを含む |
| `vendor-next/egui/PATCHES.md`、本台帳 | 段2a-2の変更、証拠、残課題 |

request_focus、request_focus_next_frameは既存どおりinterrupt_imeへ合流する。
menuの次frame focus等の同型producerも同じ入口を通ることをsource確認した。
子endは対象Focusから生成し、既存viewport stack復帰後は親のentryを再選択する。
新map、親子の要求の手動退避・復元、App補助フラグ、追加repaint、新queue、retryは作らない。
Focus map自体が既存のserde(skip)対象のため、移管したpass状態は永続化されない。

### 回帰の内容

入れ子4件は`ime_interrupt_nested_{parent_only,child_only,both,neither}`。
headless rendererを実show_viewport_immediateへ登録し、そのcallbackで同じContextの子run_uiを実行する。
実window/EventLoopや製品は作らない。既存の親子登録・used判定・viewport stack復帰を通し、
Memoryのviewport IDをテストから手動で変更しない。

事前passで親子の実TextEditへfocusを設定し、もう1passで両方のSome(IMEOutput)と要求falseを確認する。
測定passでは親のみ／子のみ／両方／なしについて、interrupt_ime／request_focus／
request_focus_next_frameの3入口をそれぞれ検査する。
親の要求は子を呼ぶ前に出す。親のNormalと子のPasswordのmetadataを別々に検査し、
両方Some、各1pass、親へviewportが復帰していることを必須とする。
測定後の新規runで、要求なしなら両方falseへ戻ることも検査する。
rendererの差し替えはthread-localで、失敗時も元callbackを復元する。

`ime_interrupt_discard_keeps_request_and_latest_text_edit_metadata`は実run_ui内の第1passだけで
要求とrequest_discardを行う。第2passは要求せず、TextEditの位置とpurposeを変える。
実2passを確認し、第1/第2passのrectとcursor_rectが違うこと、最終出力が第2passの
rect／cursor_rect／Passwordと要求trueを持つことを検査する。
その出力を一度移譲・明示破棄し、次の新規runで同じmetadataと要求falseになることを確認する。

補助の`ime_interrupt_platform_append_preserves_option_and_request_contract`は、
旧/新のSome/Noneと要求有無の16組、take後の要求消失を検査する。
この補助試験だけを実入れ子・実discardの代わりにはしていない。
各FullOutputはviewportごとにmetadataを採取して検査し、headlessで
drop_without_applying_deltasする。親子のPlatformOutputをappendしない。

### 修正前失敗・変異・復元

同じ6回帰のソースを保ったまま、修正前と修正後を比較した。
さらに修正済みsourceのbytesを保持し、所有／集約を一つずつ開始HEADの実装へ戻して検査した。
finallyで両sourceを修正済みbytesへ復元し、SHA-256一致と再成功を確認した。

| 条件 | 成功/失敗 | exit | 検出した失敗 |
| --- | --- | --- | --- |
| 修正前（両方旧実装） | 2 / 4 | 101 | 親のみ、子のみ、実discard、append補助 |
| 所有修正だけを戻す | 4 / 2 | 101 | 親のみ、子のみ。集約回帰は成功 |
| 集約修正だけを戻す | 4 / 2 | 101 | 実discard、append補助。入れ子4件は成功 |
| 両修正を復元 | 6 / 0 | 0 | 全件成功 |

変異の実行器は`target/verify-stage2a2-mutations.py`、command／exit／件数・復元hashは
`target/egui037-stage2a2-mutations.json`。ログは同prefixの
`before.log`、`ownership-reverted.log`、`aggregation-reverted.log`、`focused-restored.log`。
単独の変異から両不具合が独立して検出できることを確認した。

### 最終検証

MSBUILDDISABLENODEREUSE=1、CARGO_BUILD_JOBS=1、CARGO_NET_OFFLINE=true。
依存解決を伴う最終commandは--locked --offlineで実行する。

```powershell
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness ime_interrupt --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --features test-script --locked --offline
cargo fmt
cargo fmt --check
cargo fmt --manifest-path vendor-next/Cargo.toml --all
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
git diff --check
```

| 検証 | 結果 |
| --- | --- |
| 準備egui lib（witness有効） | 98 passed / 0 failed / 0 ignored、exit 0。追加6件を含む |
| 準備egui-winit lib | 43 passed / 0 failed / 0 ignored、exit 0 |
| 準備egui-ime lib | 26 passed / 0 failed / 0 ignored、exit 0 |
| root core check 通常／test-script | 両方exit 0、旧0.33.3 graph |
| root fmt／fmt --check | 両方exit 0 |
| 準備graph fmt／fmt --check | 両方exit 0 |
| git diff --check | exit 0。新回帰fileはgit addせず未追跡 |

最終command・exit・件数・時間は`target/egui037-stage2a2-results.json`、ログは同prefixの
`{egui-lib,winit-lib,ime-lib,root-check,root-test-script-check,root-fmt,root-fmt-check,prep-fmt,prep-fmt-check,diff-check}.log`。
関連source hashとroot／準備manifest・lock不変の照合は`target/egui037-stage2a2-source.json`。
準備graph以外のRust／テスト・root workspace／patch／製品codeは変更しない。
root/準備のCargo.lockも変更しない。既存scene.rsのf32::EPSILON非推奨警告とrootの既存警告は対象外。

別contextによる実装担当の内部確認でも、生成対象とviewport復帰、非永続性、集約と回帰・
変異証拠を照合し、未解決指摘はなかった。こちらはcargoを再実行せず、親担当の有効な証拠を再利用した。

### 残課題・本家報告

設計書との重大な食い違いはない。正式な独立レビュー・設計担当の受入れは未実施。
段2a-3の非描画退役、段2b/2c、段3の正式graph切替には手を付けていない。
rootの製品build、live、test-full.ps1、ネットワーク、アプリ起動、git add／commitは実行しない。

本家へ報告する価値がある。実入れ子run_ui／実discardの最小回帰と独立した修正前後の証拠を添えられる（今回は未送信）。

## 段2a-3（2026-10-17）

開始HEAD `9c54f4f1d`、branch `egui-036`、Rust 1.99.0。段2a-2は設計担当がコミット済み。
本節は**実装担当の内部確認**であり、並行中の正式な独立レビュー・設計受入れを代替しない。
git add／commit／branch操作は行わず、未コミットで返す。

### 合意・修正境界

設計第4.4版§4.2「非表示子のCloseとlifecycle要求」、§4.8のegui行、段2a順序11、
IME修正と非描画退役の別コミット指定、detached計画§2・§11の構造合意を読み直した。
非表示子Closeの受付・消費・既存終了ownerへの合流・非描画退役は合意済みの境界である。

従来の登録終了はend_pass内のGCにしかなく、非描画の承認済み子を終了する公開入口がなかった。
さらにGCはViewportStateをlock下でdropし、viewport別Memoryを残していた。
登録・親関係・viewport別状態の所有境界に共通処理を置き、明示退役と通常GCの両方を通した。

| 変更file | 変更内容 |
| --- | --- |
| `vendor-next/egui/src/context.rs` | 公開`Context::retire_viewport(ViewportId) -> Vec<ViewportId>`と通常GCの共通finalization。子孫を同時に終了し、実除去IDを返す。Stateのdropは両経路ともlock外 |
| `vendor-next/egui/src/memory/mod.rs` | 対象のareas／interactions／Focus／popupsを除去。現在Memory ownerが対象ならROOTへ戻す。兄弟と共有widget data／cacheを保持 |
| `vendor-next/egui/src/context/viewport_retirement_tests.rs` | 実Contextの8回帰。全名の共通filterはviewport_retirement |
| `vendor-next/egui-winit/src/disabled_ime_tests.rs` | 既存兄弟IME fixtureの生存登録を正規deferred APIで維持。producer・assertは不変（下記原因を参照） |
| `vendor-next/egui/PATCHES.md`、本台帳 | 契約・結果・残課題 |

入口はcompleteなrun_ui／run_pass／run_logicが返った後、plugin callback外、backend／Context
lock外で呼ぶ。ROOTとactive passはpanic、未知／重複IDはno-op。返すIDの順序は規定しない。
先行FullOutput／texture deltaの配送と対象IDの新規publish停止は呼出元backendの責務である。
新しい抑止ラッチ・退役済みIDリストはなく、後の明示openでは同じIDを新規登録できる。
子孫は親goneという既存GCの終了条件を同じpassで完了する。

対象登録・親関係・viewport別Memory・font galley cacheだけを終了する。共有texture manager・
未配送texture delta・generic Memory.data／cache・plugin所有状態はresetもdrainもしない。
egui-imeのcomposition map等は既存input_hookのRawInput.viewports live-set pruningに従う。
本入口だけでplugin側の対象IMEまで即時全消去するとは扱わない。段2c／段3のbackend接続では、
退役対象を新規RawInputのlive-setから外し、既存終了ownerと出力処理を接続する必要がある。

### 回帰と変異による検出確認

実window／EventLoopは作らず、Context.run_ui・show_viewport_deferred・run_logicを通す。
FullOutput検査後はdrop_without_applying_deltasでheadless明示破棄する。

- 承認済み対象とその子孫の登録・親関係・viewport別Memoryが終了する。
- 兄弟の保持キー・modifier・pointer・input、実TextEditの非空Preedit・cursor・undoer、Focus・
  Interaction・commands・pass数が不変。対象font cacheだけが消え、UI呼出しとrepaintは増えない。
- staleなRawInput metadataが残っても、後続root／兄弟の実出力に対象は再登場しない。
  同じIDを後で明示openすると、新しい登録が成立する。
- hide／minimizeだけでは終了せず、通常GCも同じ子孫・Memory終了を行う。
- 最後のMemory ownerが退役対象のときだけROOTへ戻り、兄弟なら維持する。
- 明示入口と通常GCのcallback ownerはlock外でdropし、通常Context readが成功する。
- callback所有TextureHandleの解放は、共有managerの未配送upload／freeへ通常どおり合流し、
  兄弟textureを解放しない。ROOT／pass中の無効な呼出しは状態変更前に拒否する。

| 条件 | 成功 / 失敗 | exit |
| --- | --- | --- |
| Memory終了呼出しだけを外す | 4 / 4 | 101 |
| 通常GCから共通finalizationへの呼出しだけを外す | 6 / 2 | 101 |
| 修正済みbytesへ復元 | 8 / 0 | 0 |

変異の実行器は`target/verify-stage2a3-mutations.py`。finallyで元bytesへ復元しhashを記録した。
件数・command・exit・復元hashは`target/egui037-stage2a3-mutations.json`、ログは同prefixの
`memory-cleanup-reverted.log`／`normal-gc-sharing-reverted.log`／`focused-restored.log`。

### 既存兄弟IME fixtureの適応

最初の全lib検証はeguiが106成功、egui-winitが42成功／1失敗（exit 101）となった。
失敗はdisabled_producer_isolates_sibling_state_and_viewportで、兄弟Preeditが実TextEditへ入らなかった。
fixtureはRawInput.viewportsへIDを載せるだけで、親から子を存続登録していなかった。
旧GCでも次のroot passで子ViewportStateが消えており、残留Memory.Focusへ依存していた。

Focus除去だけを一時的に外すと旧fixtureの当該1件は成功（exit 0）した。
finallyで修正済みMemory bytesへ復元した。証拠は`target/egui037-stage2a3-fixture-stale-focus.log`。
fixtureを非embedにし、root passで既存の兄弟IDをshow_viewport_deferredへ登録し続ける形へ直した。
State変換→共有plugin→実TextEditの経路と全assertを保ち、producerや製品codeは変更しない。
修正後は当該1件とegui-winit全43件が成功した。これはegui以外への必要なtest-only適応である。

### 最終検証

MSBUILDDISABLENODEREUSE=1、CARGO_BUILD_JOBS=1、CARGO_NET_OFFLINE=true。
依存解決を伴うcommandには--locked --offlineを付けた。

```powershell
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness viewport_retirement --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --features test-script --locked --offline
cargo fmt
cargo fmt --check
cargo fmt --manifest-path vendor-next/Cargo.toml --all
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
git diff --check
```

| 検証 | 結果 |
| --- | --- |
| 準備egui lib（witness有効） | 106 passed / 0 failed / 0 ignored、exit 0。追加8件を含む |
| 準備egui-winit lib | 43 passed / 0 failed / 0 ignored、exit 0 |
| 準備egui-ime lib | 26 passed / 0 failed / 0 ignored、exit 0 |
| root core check 通常／test-script | 両方exit 0、旧0.33.3 graph |
| root fmt／fmt --check | 両方exit 0。準備graphはrootの対象外で別途実行 |
| 準備fmt／fmt --check | 両方exit 0 |
| git diff --check | exit 0。新回帰fileは未追跡のまま、別途末尾空白も検査 |

最終command・exit・件数・時間は`target/egui037-stage2a3-final-results.json`。
root fmt等の先行成功と初回winit失敗は`target/egui037-stage2a3-results.json`に保持した。
ログは同prefixの`{egui-lib-final,winit-lib-final,ime-lib-final,root-check,root-test-script-check,
root-fmt,root-fmt-check,prep-fmt-final,prep-fmt-check-final,diff-check}.log`。
source hashとroot／準備manifest・lock不変の照合は`target/egui037-stage2a3-source.json`。
root workspace／patch／製品code・旧vendor・両Cargo.lockは変更しない。
既存scene.rs非推奨警告とroot既存警告は残る。アプリbuild／起動、live、test-full.ps1、networkは実行しない。

別contextの実装担当の内部確認でも、共通GC・子孫所有・lock外drop・共有資源保持・fixture因果を
sourceと有効な検証証拠で確認し、未解決指摘はなかった。正式な独立ACCEPTを意味しない。

### 残課題・設計書との差

重大な食い違いはない。追加の範囲は上記egui-winitのfixture-only適応で、既存検証の生存前提を修正した。
段2cのClose受付・消費、backend window／surface／State退役、段2b、段3の正式接続は未実装。
本家へは通常GCのviewport別Memory残留とlock下callback破棄を最小回帰と共に報告する価値がある（未送信）。

## 段2b（2026-10-17）

開始HEAD `357b25c9d`、branch `egui-036`、Rust 1.99.0。段2a-1／2a-2は独立レビューACCEPT、
段2a-3は設計担当がコミット済みで独立レビュー中、という引継ぎで着手した。
本節は**実装担当の内部確認**。正式な独立レビュー・設計受入れを代替しない。
git add／commit／branch操作／pushは行わず、未コミットで返す。

### 原本・取り込み境界・準備graph

設計第4.4版§4.8のegui-wgpu行・取り込み手順、§4.9、段2表2b・受入条件を読み直した。
旧vendor READMEに加え、registryの0.33.3原本と現vendorの実差分全体を照合した。
srcは9ファイルの差分（追加moduleを含む）で、shader空行以外の機能差分を論理項目ごとに付け直した。
旧差分の全file台帳は`target/egui037-stage2b-old-inventory.json`、diffは同prefixのold-diff以下。

| 原本記録 | 値 |
| --- | --- |
| 配置 | 実directory `vendor-next/egui-wgpu`、version 0.37.0 |
| 元 | `C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\egui-wgpu-0.37.0` |
| archive | 同registryのcacheにあるegui-wgpu-0.37.0.crate |
| archive SHA-256 | `0309cbe1febed48af5b974e4bcc424b35e9664863a4d402697147b57a488c864` |
| checksum照合 | registry index cksum、archive、展開sourceが一致 |
| VCS | `7c0b40402271e8d40cda69f87cf3c2617c2e4382`、crates/egui-wgpu |
| license | MIT OR Apache-2.0。本文は旧vendorから追加、hashはupstream-provenance.jsonのlocal_license_files |

import-upstream.pyへpackage指定と既存provenance保持を追加し、
`python vendor-next/import-upstream.py egui-wgpu`で原本12ファイルをbyte照合して配置した。
原本を置いてから移植し、旧manifestを上書きコピーしていない。`.cargo-ok`、target、
不要なregistry管理ファイル・他DLLは取り込まず、junction／symlinkも作らない。

| handoff | 内容 |
| --- | --- |
| `target/egui037-stage2b-handoff/original/` | 原本package12ファイル＋その時点のimporter／provenance。mIV追加source・回帰・PATCHES・license本文をpackageに含めない |
| 同`patched/` | 最終packageと変更した準備graph・文書・file hash。CHECKPOINT.jsonで原本不変／変更／追加を区別 |

原本のままなのは`.cargo_vcs_info.json`、同梱Cargo.lock、Cargo.toml.orig、README.md、
src/egui.wgsl、src/setup.rs、src/texture_copy.wgslの7ファイル。
原本から変更したのはCargo.toml、src/lib.rs、src/renderer.rs、src/winit.rs、src/capture.rsの5ファイル。
追加はatlas_diag.rs、mipmap.rs／wgsl、startup_diagnostics.rs、renderer_tests.rs、winit_tests.rs、
PATCHES.md、LICENSE-MIT／APACHEの9ファイルで、全てpatched側にだけ含める。

準備workspaceへmemberとegui-wgpu path patchを足した。eguiは既存vendor-next/eguiと同じ実体。
準備lockはwgpu 30.0.1接続とpollster dev依存のため更新した。既存package版の除去・更新はなく、
新package版を44件追加した（lock-audit.json）。rootのworkspace・patch・製品code・Cargo.lockは不変。
同梱crate lockは原本資料として残し、実行はvendor-next/Cargo.lockへ統一する。

### 付け直した差分と分類

詳細は`vendor-next/egui-wgpu/PATCHES.md`と実source差分を正本とする。

| 項目 | 分類・処置 |
| --- | --- |
| managed mipmap allocation／再生成／sampler | (b)。opt-in allocation、whole／partial後の再生成、Noneへのview制限を保持。0.37のflags bindingとpredictable filteringを維持 |
| 公開MipmapGenerator／exact texel count | (b)。wgpu30のBGL Option・immediate_size・multiview_maskへ適応。奇数端・非正方形を保持 |
| atlas diagnostics／identity／texture_size | (b)。RGBAとID別delta列に対応、各deltaのbefore／after・freeと順序を記録 |
| 既存atlas overflow skip-write | (b・既存範囲のみ)。根治策として広げず、通常whole→partialを実画素で検査 |
| 無描画delta／対象surface限定退役 | (b)。同じTextureDeliveryでdrain／submit／free、safe／unsafe入口共通で対象だけの資源を除去 |
| startup GPU stage／root present observer | (b)。async列挙・Instance保持・Queue::presentへ適応。setと最終submit/freeのTextureDelivery spanを保持 |
| atlas-probe／capture outcome／readback診断 | (b)。文字列summaryだけを保持し、viewport相関と中断・callback結果を記録。診断cloneに適用権を持たせない |
| upstream submission guard | (a)+(b)。早期終了submit保証を利用し、mutable deltaとmipmap submit・free完結を接続 |
| callback screenshot／hidden offscreen capture | (a)。本家のone-shot callback・共有画像・acquire失敗時capture継続を利用 |
| SurfaceConfig／stencil／lib test有効化 | (a)。新版構造・buffer条件・既定test有効を利用。stencilだけを理由にbufferを追加しない |
| shader空行・旧channel／Event配送・旧surface error分類・旧manifest/lockコピー | (c)。空行は不要、他は新版API／準備workspaceへ置換。追加repaintや旧版動作を再注入しない |
| license／test依存・回帰接続 | (b)。本文のbyte保持、local license includeとpollsterを新manifestへ追加 |

AutoVsync／latency 1とMIV_WGPU_FRAME_LATENCY等の製品設定はroot旧graphでそのまま保持する。
新WgpuConfiguration.surfaceへの正式接続は段3。vendorに製品環境変数parserを複製しない。
D3D11 native presenter／Naga27の別経路も変更しない。

### TexturesDeltaとsubmissionの完結owner

Painter consumerは`&mut TexturesDelta`を受け、rendererがあればsurface lookupより前に
set mapをdrainする。各ID内のwhole→partial列順を保持し、全IDの適用後にsubmitを完結してから
free setをdrainする。renderer未準備なら両方未消費のまま呼出元の既存pending ownerへ残す。

TextureDeliveryは本家guardを拡張した呼出し内の単一完結ownerで、通常submit後の空submitを抑止する。
内部update結果Unchanged／Pending／Submittedにより、最後のmipmap内部submitを追う。
最後がmipmapなら不要な空submitをせず、後から通常writeがあればsubmitする。
update_buffers後・acquire不成立・capture zero-size・surface再作成失敗も同じ完結境界に合流する。
通常paint／capture-onlyは通常submit後にfreeし、RootPresentedは実presentの返却時だけ発行する。
単なるsurfaceなしではcaptureのgeometryがないので、upload/freeだけを完結する。

共有rendererと未配送deltaをviewport終了で一括clearしない。productionのGPU待機・retry・
新queue／pending owner・App補助フラグ・追加repaint・二重適用は加えていない。
本家のsurface回復・repaint動作は維持する。診断用ownerはcall-local enum／Stringだけで、適用権はない。
headlessで配送先不在と確定したfixtureだけはmetadata採取後clear／FullOutput明示破棄で終端する。

### 回帰・追加確認で見つかった検証不備

全16件。mip count2件、shader/layout2件、atlas列順1件、startup final-pass／present observer2件、
managed／公開mipmap GPU2件、capture GPU1件、Painter経路6件で構成する。

- no-surfaceでatlas whole1x1→2x2→重なるpartial2件を配送し、最終4画素の順序を検査。
  同batch creation+free、free-only、消費済みdelta空、実submit回数、free後のtexture不在を確認する。
- renderer不在はset／freeを保持し、通常／fallback submit回数が0のまま。
- acquire不成立相当は実update_buffers後に同じ完結ownerをdropし、通常submit経路とも比較する。
- mip→通常、通常→mip、全mipでfallback回数を1／0／0、通常submitでnormal1／fallback0を検査する。
- managed mipmapは実bind group／samplerで描画してreadback。partial後の鎖再生成、None view制限、
  wholeでのサイズ更新、公開generatorの3x3奇数端の寄与を検査する。
- captureはsurface無しでRGBA／BGRAのresize・row padding・clone callback one-shot・複数要求を検査。
  runtime probe off/on両方を通し元設定へ戻す。新callbackに追加repaintを付けない。
- safe／unsafe set_window(None)の実公開入口へactual GPU depth／MSAA viewを登録し、対象だけの除去と
  root／兄弟attachment・共有renderer identity／texture保持を検査。surface mapは同じ共通helperへ
  drop sentinelを渡して対象だけのdropを検査する。実Surfaceはwindow無しでは作らない。
- startup callbackからset側とfinalization側のBegin/End、実submit返却・free後の観測順を検査する。

初回test compileは新API差4件で失敗した（tool exit 1）：InstanceDescriptorにDefaultがない2箇所と、
device.pop_error_scopeがErrorScopeGuardへ変わった2箇所。新版factoryとguard.popへ訂正した。

追加の変異確認では、fallback submitを外しても最初のGPU buffer readback回帰が成功した。
この結果をsubmitの証拠として採用せず、driver側の原因も断定しない。
実normal／fallback queue.submitの返却直後だけ加算するcfg(test) thread-local counterを追加し、
consumer前後の回数差で欠落と重複を検査する形へ補強した。productionには計測field／flagを残さない。
GPU readbackは画素・buffer内容の証拠として維持する。
内部確認で見つけた旧atlas/capture outcomeログ・最終free spanの移植漏れも、全検証前に復元した。

| 最終変異 | 成功 / 失敗 | exit |
| --- | --- | --- |
| fallback submit条件を無効化 | 0 / 1 | 101 |
| 対象限定cleanupを全clearへ戻す | 0 / 1 | 101 |
| 修正済みbytesへ復元・通常lib全件 | 16 / 0 | 0 |

変異はfinallyでsource bytesを復元しhashを記録した。記録は
`target/egui037-stage2b-mutations-final.json`、各ログはsubmission-guard-reverted／
target-cleanup-reverted／wgpu-restored。検出できなかった初回試験も同prefixの
submission-reverted.log／mutations.jsonとして保持し、成功証拠と混同しない。

### 最終検証・残課題

MSBUILDDISABLENODEREUSE=1、CARGO_BUILD_JOBS=1、CARGO_NET_OFFLINE=true。
全Cargo commandはoffline、解決を伴う最終commandはlockedも指定した。registry不足はなかった。

```powershell
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit,egui/miv-repaint-witness --lib --locked --offline
cargo check --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit --locked --offline
cargo check --manifest-path vendor-next/Cargo.toml -p egui-wgpu --no-default-features --features wgpu/dx12 --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --features test-script --locked --offline
cargo fmt
cargo fmt --check
cargo fmt --manifest-path vendor-next/Cargo.toml --all
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
git diff --check
```

| 検証 | 結果 |
| --- | --- |
| 準備egui-wgpu lib 通常／診断 | 各16 passed / 0 failed / 0 ignored、exit 0。nocaptureログにもGPU skip無し |
| 同通常check／egui無し最小check | 両方exit 0 |
| 準備egui／egui-winit／egui-ime lib | 106／43／26 passed、失敗・ignore無し、各exit 0 |
| root core check 通常／test-script | 両方exit 0、旧graph使用 |
| root fmt／fmt --check | 両方exit 0 |
| 準備fmt／fmt --check | 両方exit 0（root fmt対象外のため別途実施） |
| diff／原本hash／新file空白／root不変／準備graph実体照合 | 成功 |

初回全件の結果は`target/egui037-stage2b-results.json`。最終補強で変えたwgpuだけ再検証し、
不変の関連crate／rootの有効な成功結果を再利用した。最終結果は同prefixのfinal-results.json、
ログはwgpu-lib-final／wgpu-lib-diagnostic-final／wgpu-normal-check-final／wgpu-minimal-check-final等。
sourceと原本・root不変照合は同prefixのaudit.json、準備metadataはmetadata.jsonに保存する。
wgpu固有の旧診断featureは存在しないため、新featureは作らず既存egui witnessを診断graphに併用し、
wgpuのruntime capture probe off/onを回帰内で検査する、と受入条件を具体化した。

別contextの実装担当の内部確認でも、旧実差分・所有境界・新API・最終変異／検証証拠を照合し、
最終的な未解決指摘はなかった。正式な独立ACCEPTを主張しない。
既存egui scene.rsのf32::EPSILON警告とrootの既存警告は残る。

設計との重大な食い違いはない。実Surfaceの作成・acquire／presentそのものはwindow必須なので、
今回のoffscreenでは同一owner／cleanup helperと実GPU資源で検証し、実機確認は後続へ残す。
段2cのeframe接続、段3の製品切替・native overlay・VSync環境変数接続、段4の実機確認は未着手。
live・製品build／起動・test-full.ps1・ネットワーク・git add／commitは実行していない。

## 段2c-1 着手・依存不足による中断（2026-10-17）

開始HEAD `fb07cec57`。段2bは設計担当が`603e2aa5c`（原本）と`fb07cec57`（差分）へ
分けてコミット済み、独立レビュー中。段2aは全体が独立レビューACCEPTという引継ぎで着手した。
以下は**実装担当の内部確認**であり、段2c-1の実装完了・正式な独立レビューを意味しない。

設計第4.4版§4.2全体、§4.8のeframe行・取り込み手順、§4.9、段2c受入条件と案Bの1〜15を
読み直した。今回を既存パッチ移植までとし、入力所有・案B・描画資格・Closeは後続2c-2〜4へ残す。

### 完了した原本取り込み

- `vendor-next/import-upstream.py`へeframeを追加し、index pathをpackage名から組み立てる形へ適応した。
- `python vendor-next/import-upstream.py eframe`でローカルregistryの0.37.0原本36ファイルを配置した。
  archive／index checksum／展開sourceをbyte照合。既存3packageのprovenance記録は維持する。
- archive SHA-256は`9c5642501a65ca657d05f00c643a5c14ff2346cb3818032bfe23d58e1f8e5f84`。
  VCS・MIT OR Apache-2.0宣言・全file hashはupstream-provenance.jsonのeframe行へ追記した。
- `target/egui037-stage2c1-handoff/original/`へ原本packageとその時点のimporter／provenanceを保存した。
  mIV追加ファイル・license本文の追加は原本境界に含めていない。現packageも原本36ファイルのまま。
- 旧vendor対0.33.3原本の機械差分台帳を`target/egui037-stage2c1-old-inventory.json`へ、
  file別diffを同prefixのold-diff以下へ保存した。項目別の新版再適用は未着手。

### offline準備graphの解決失敗

eframeを準備workspaceのmember／path patchへ追加して、次を実行した。

```powershell
cargo generate-lockfile --manifest-path vendor-next/Cargo.toml --offline
```

`no matching package named egui_inspection found`で失敗した（tool exit 1）。
新版eframeのinspection featureはoptionalだが、無効な構成でもCargoの依存解決対象となる。
新manifestのoptional依存・featureを削って回避することはしていない。
指定registryのindex `.cache/eg/ui/egui_inspection`、srcの`egui_inspection-*`、
cacheの`egui_inspection-*.crate`はいずれも存在しないことを読み取りで確認した。
この不成立のnative Cargo exit codeは採取していないため、101等を推測して記録しない。

ネットワークを使わない条件に従い、差分移植・新版のテストへ進まず中断した。
追加予定の準備manifestは`target/egui037-stage2c1-handoff/patched/vendor-next/Cargo.toml`へ保存し、
作業ツリーの準備manifestは開始HEADと同じ4member／3patchへ戻した。
準備Cargo.lockも不変。これにより並行レビュー中の段2b graphを依存不足で壊さない。
patched handoffは**依存解決で中断した部分状態**であり、パッチ適用済みの完成状態ではない。

### 検証・未実施・再開条件

原本36ファイルのhash一致と、root workspace／patch／Cargo.lock、既存準備manifest／lockが
開始HEADと不変であることを確認した。実行結果は同prefixのblocked-results.jsonへ記録した。

| 検証 | 結果 |
| --- | --- |
| root core check 通常／test-script | 両方exit 0 |
| root fmt／fmt --check | 両方exit 0 |
| 既存準備graph fmt／fmt --check | 両方exit 0 |
| 既存4memberのmetadata（locked／offline／no-deps） | exit 0 |
| git diff --check、原本36file hash、不変file・index照合 | 成功、indexは空 |

新eframeのlibテスト・scheduler等の移植・その分類台帳・live試験の新版compileは未実施。
新eframeが未接続なので、既存4crateの成功を段2c-1新版graphの検証とは扱わない。
live・製品build／起動・test-full.ps1・ネットワーク・git add／commit／branch操作は行わない。

再開には少なくともegui_inspection 0.37系のindex情報を含むローカル依存cacheの補充が必要。
補充後にoffline解決を再開し、他の不足があれば同じ条件で報告する。
設計との重大な矛盾は観測していないが、差分移植と受入条件の検証はこれからである。

## 段2c-1 再開・既存eframeパッチの付け直し（2026-10-17）

設計担当がegui_inspection 0.37.0とeframe全featureの依存cacheを補充し、原本36ファイルと
原本記録を`83836dd16`へコミットしたとの引継ぎで再開した。上の中断記録は当時の事実として残す。
今回の作業中は取得・ネットワーク通信を行わず、すべて`--offline`で解決・検証できた。
以下は**実装担当の内部確認**。正式な独立レビュー・ACCEPTではない。変更は未ステージ・未コミット。

### 原本・差分・handoffの境目

原本は前節と同じeframe 0.37.0、archive SHA-256
`9c5642501a65ca657d05f00c643a5c14ff2346cb3818032bfe23d58e1f8e5f84`、
VCS `3c6eb8261c3293ad9c7ea1ed58487e038860ebef`の`crates/eframe`、MIT OR Apache-2.0。
registry原本・原本handoff・provenanceの36file hashを再照合した。原本コミットを変更していない。

`target/egui037-stage2c1-handoff/original/`は原本36ファイルと取り込み時点の記録を維持する。
`patched/`は中断時の部分状態から最終の適用後状態へ更新し、CHECKPOINT.jsonへ境目を記録する。
原本36ファイル中、**28ファイルは原本とbyte一致、8ファイルを変更、5ファイルを追加**した。

- 変更8件: `Cargo.toml`、`src/epi.rs`、`src/lib.rs`、`src/native/epi_integration.rs`、
  `src/native/glow_integration.rs`、`src/native/run.rs`、`src/native/wgpu_integration.rs`、
  `src/native/winit_integration.rs`。
- 追加5件: `LICENSE-APACHE`、`LICENSE-MIT`、`PATCHES.md`、
  `src/miv_test_script_window_witness.rs`、`src/startup_window_observer.rs`。原本境目には含めない。
- 上記以外の原本ファイルはすべて不変。特にREADME.md、Cargo.toml.orig、同梱Cargo.lockは原本のまま。
  ライセンス本文は旧vendorからbyte保持で同梱し、取得元・hashをprovenanceへ追記した。

準備workspaceはegui／egui-winit／egui-ime／egui-wgpu／eframeの5member。
新版の依存・featureを基準にeframeのmemberとpath patchを追加し、旧manifestはコピーしていない。
metadataでeframeから使うegui・egui-winit・egui-wgpuが各1実体で、すべて準備directoryであること、
wgpuが30.0.1であることを確認する。egui_glow・egui_inspection等は補充済みregistryを使用する。
rootのworkspace／patch／依存版／Cargo.lock／製品コード・旧vendorは不変。

準備用`vendor-next/Cargo.lock`だけはoffline再生成でeframe等を追加した（計452package）。
再解決により既存のzbus_names 4.3.4→4.3.5、zvariant／zvariant_derive 5.15.0→5.15.1も更新された。
追加31エントリ・削除3エントリの内訳は同prefixのlock-audit.jsonに残す。
これは準備graphのresolver更新であり、root lockや製品の依存更新ではない。

### 実差分の分類と付け直し

旧vendor対eframe 0.33.3原本の実差分を項目別に照合した。
詳細は[eframe/PATCHES.md](../vendor-next/eframe/PATCHES.md)。READMEだけを移植台帳としていない。

| 分類 | 対象と新版での扱い |
| --- | --- |
| (a) 上流採用 | #7905のhidden due基本経路・sleep、PassMode／250ms期限、callback screenshot、Ui／SurfaceConfig等の新版基本APIを保持。旧backportを重ねない |
| (b) scheduler | dirty／painting、window別claim・surface generation／size、outer epochを移植。hidden再予約のmaxによる100ms下限と遅い要求を早めない保証を保持。本家のdeadline mapを同じschedulerへ統合し、期限超過はouter LogicOnly、同対象のFull damageを優先。idle heartbeatは追加しない |
| (b) resize | Bootstrap／AccessKit／InteractiveResizeのprovenance。元Resizedが非ゼロの場合だけinline資格を付け、描画時の最新実寸利用は本家のまま |
| (b) show・観測 | root show返却後の初回visible receiptとFrame getter、startup window observer。子paintがrootのreceiptを消費しない。新版のbefore／after_first_showも保持 |
| (b) renderer限界 | root／childのegui-winit Stateへ同じ実renderer由来max_texture_sideを設定 |
| (b) delta配送 | lookup／GC前に共有pending ownerへappendし、非描画・消滅viewportも段2bのmutable Painter consumerへ接続。未初期化renderer時は同ownerへ保持 |
| (b) screenshot | 既存保留actionにあるimmediate子のcallbackをその子へ配送。wgpuの子paint、glowの子surface readbackで完了し、one-shot・順序・他action保持を検査 |
| (b) 診断 | test-script witness／capture・focus診断、通常版immediateの段階別時間・delta件数／RGBA bytes・遅延ログを移植。wgpu_no_default_featuresとcallback APIへ適応 |
| (b) 配布・検証 | miv feature、Win32 Gdi／Performance、Windows test用wgpu30 DX12、workspace patch、license同梱。live試験のignoreと子再実行--ignoredを保持し、App::uiへ適応 |
| (c) 削除・非移植 | 旧Event::Screenshot／capture channelはcallbackへ置換し、画像配送だけのrepaintを復活させない。旧manifest・lockはコピーせず、#7905だけを理由とするvendor全体の撤去条件も採用しない |

### delta完結ownerと後続段との境界

本家0.37でRunning／Viewportに分かれていたpending deltaを、旧パッチにある共有renderer単位の
SharedState ownerへ集約した。UI出力は対象viewport lookupやGCより前に一度だけappendする。
通常描画も無描画も同じmutable deltaをPainterへ渡し、段2bのset drain→upload／submit→free drainが
完結ownerになる。renderer未準備時は共有ownerへ残す。viewport Dropで共有deltaをclearしない。
共有renderer／Contextそのものの終了時だけ明示破棄し、診断目的の適用権を持つcloneを作らない。

eframeの窓なし回帰は実GC helperを通す子消滅時のdelta保持・root generation不変、callbackの対象・
順序・one-shotと他action保持、whole／partialのRGBA統計を検査する。
glow側は偽画像でcallback完了を検査する。schedulerは既存14件と新版deadline統合4件、
Frame receiptは既存2件、witnessは既存4件を新版APIで検査する。

窓なしでeframeのPainterへ実RenderStateを注入する既存APIがないため、新setterや初期化時点変更は
追加していない。実GPUでのdrain／submit／free・readbackは同じgraphのegui-wgpu回帰で検証し、
eframeの接続はsourceを照合した。eframe実入口からGPU完結まで一体実行したという証拠ではない。

2c-2の保留RawInput所有／child skip、2c-3の案B・最小化UI継続／UIと描画の分離／immediate描画資格・
当該UI出力のaction完結、2c-4のClose受付・消費・非描画退役には未着手。本家の現挙動を保持する。
既存保留Screenshotの移植と、当該UI出力Screenshotの前倒し消費を混同しない。
構造合意に対する今回の実装範囲をdetached-rework-plan.md §11へ追記した。

### 検証結果

Rust 1.99.0（b940084d7 2026-09-28）。すべて非対話・offline。
最終commandと所要時間は`target/egui037-stage2c1-final-results.json`、ログは同prefixの`*-final.log`。
root fmtの成功は先行した同prefix results.jsonの記録を再利用した（rootソース不変）。

```powershell
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence --lib --locked --offline -- --nocapture
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence,miv-test-script-window-witness --lib --locked --offline -- --nocapture
cargo check --manifest-path vendor-next/Cargo.toml -p eframe --no-default-features --features glow,default_fonts --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit --lib --locked --offline -- --nocapture
cargo test --manifest-path vendor-next/Cargo.toml -p egui --features miv-repaint-witness --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --features test-script --locked --offline
cargo fmt
cargo fmt --check
cargo fmt --manifest-path vendor-next/Cargo.toml --all
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
git diff --check
```

| 検証 | 結果 |
| --- | --- |
| eframe lib（defaultにglow／persistence追加） | 28 passed / 0 failed / 1 ignored、exit 0 |
| eframe lib（上記＋witness） | 32 passed / 0 failed / 1 ignored、exit 0 |
| eframe glowのみcheck | exit 0 |
| egui-wgpu lib | 16 passed / 0 failed / 0 ignored、exit 0。GPU skip無し |
| egui／egui-winit／egui-ime lib | 106／43／26 passed、失敗・ignore無し、各exit 0 |
| root core check 通常／test-script | 両方exit 0、旧graph使用 |
| root fmt／fmt --check、準備fmt／fmt --check | すべてexit 0。root fmtの対象外なので準備側も明示実行 |
| diff／原本hash／root不変／graph単一実体／index照合 | 成功。indexは空 |

live試験`synchronous_damage_returns_before_outer_paint_gate`は両構成ともignoreされ、実行していない。
その内部の子再実行用`--ignored`は保存しているが、検証commandには付けていない。
初回の診断構成はViewportClassのDebug実装が無くcompile exit 101。
診断formatterを4variantの明示ラベルへ修正後、上記通常／診断を再実行して成功した。
初回失敗ログも残す。receipt回帰の意図的panicはcatch済みで、失敗件数には含まれない。

最終tested-source.jsonのRust／manifest／lock hashをhandoff時に照合し、有効なテスト結果と一致させる。
audit.jsonに原本28／変更8／追加5の全一覧、root不変、graph実体、handoff file hashを保存する。
別contextによる実装担当の内部確認でも実差分・所有境界・後続段との区切り・テスト証拠を照合した。
正式な独立レビューは設計担当から依頼される別工程である。

既存egui scene.rsのf32::EPSILON、witnessのfetch_update非推奨、glowのみ構成の未使用AccessKit variant、
rootの既存警告は残る。設計との重大な食い違いはない。実windowのresize／show／surfaceは未実行で、
後続2c-2〜4・製品切替・実機確認を残す。新App flag・queue・retry・時間窓・追加repaintは追加しない。
live・製品build／起動・test-full.ps1・ネットワーク・git add／commit／branch操作は行っていない。

## 段2c-2 保留RawInputのviewport所有・child skip保持（2026-10-17）

開始HEAD `d9a28e0f3`（段2c-1は設計担当がコミット済み、独立レビュー中）。
第4.4版§4.2のbackend保留・root LogicOnlyとvisible child・条件表・hook契約、§4.8 eframe行、
段2c受入条件の最初8項目、およびdetached §2／§11の構造合意を再確認した。
以下は**実装担当の内部確認**であり、正式な独立レビュー・ACCEPTではない。未ステージ・未コミット。

### 原因と所有境界の変更

本家由来のEpiIntegration共通pending_raw_inputは、root LogicOnly(A)の未消費入力を
deferred child Full(B)のprepare_raw_inputが取り出せた。RawInput.appendはeventsを連結しつつ
viewport_idを新しい側へ変えるため、Aがchildへ混入してroot Full(C)で欠落する。
両backendのchild skipもState take後であり、UIを実行しない子の入力をそのまま捨てていた。

共通保留を削除し、wgpu／glowそれぞれの既存Viewportへ移した（b・新規）。
共通take helperは対象ownerの保留→新規State入力だけをappendし、最新metadataを採る。
hookは連結後・従来位置で呼ぶ。root LogicOnlyはpost-hookの未消費RawInputを返し、
backendはユーザーコード中のborrow／lockを解放してから同じrootへ戻す。
deferred／immediateも同じ取得入口を使い、immediateだけの追加App hookは作らない。
child skipは共通selectorでState take前に判断し、State・既存保留の双方を触らない。

追加の寿命照合で、skip後の入力を持つStateがnative builder再生成時に破棄される経路も確認した。
native window再生成はlogical Viewportの終了ではないため、置換前に旧Stateの未消費入力を
同じ既存保留ownerへ移譲する。保留→旧State→再生成後の新規入力の順序を維持する。
初回take前のState raw viewport_idはROOTの既定値なので、Viewport.ids.thisで正規化する。
次の通常取得で最新の全viewport metadataを付けてhookへ渡し、誤配送後に捨てるguardは作らない。
通常GCでlogical Viewportを除去した場合だけそのownerの保留とStateが終了し、兄弟は不変。
同IDの新規logical ownerへ旧保留を移さない。hide／minimizeだけでは除去しない。

### IME policyの通常GC寿命

準備egui-imeの入力hookだけによるlive-set pruningには、Context GCが入力hookの後で走り、
次のUI passで不在metadataを観測する前に同IDが再openされる穴があった。
run_logicは入力hookを呼ばないので、それだけでは旧compositionを終端できない。

既存のper-pass output_hookへ、実Context GC後の全viewport_output集合による対象限定pruneを接続した。
FullOutput集約で古いIDをunionする前の集合を使用する。minimized／hiddenなlive siblingは残す。
終了対象のcompositionと既存focus contract、その欄の既存retryだけを削除する。
同ID再openでpass番号がresetすると古いretry番号が再び有効になり得るため、この寿命も揃えた。
別欄へfocus contractを置換／破棄するときには旧欄のretryも終端し、既存contractで所有対象を
特定できる不変条件を保つ。新registry／queue／Plugin APIや共有widget dataの一括resetは追加しない。

egui-ime変更は準備copyだけ。eframeのnative dev-dependencyに同じ準備egui-imeを追加し、
準備Cargo.lockのeframe依存列へ1行追加しただけで版・checksum更新はない。
root製品のcrates/egui-ime、Cargo.toml／Cargo.lock、workspace／patch／旧vendor・Appは不変。
段3で単一policyへ統合する際、この寿命差分と回帰も移す（同crate PATCHESへ記録）。

### 回帰と部分復元の証拠

窓なしで実EpiIntegration.update／update_logic_onlyとfake App・入力取得fixtureを接続する。
native run_ui_and_paint全体やHWNDをfakeで置換した試験ではなく、実integration＋共通取得helperの
実行と、両backendの実接続のsource照合を区別する。実State fixtureはUnavailable display handleを使う。
実window／EventLoopを作らず、FullOutput検査後はdrop_without_applying_deltasで明示破棄する。

| 回帰 | 検査する契約 |
| --- | --- |
| viewport_input_logic_only_child_full_root_full_preserves_owner_and_hook_order | 1回／3回のroot LogicOnlyとchild Fullの交互更新。childはBのみ、rootはA＋C、hook mutationもrootへ保持。次のFullは新規Dだけ |
| viewport_input_skips_do_not_drain_and_resume_appends_new_input_after_terminal_events | 反復skipでtake 0回、兄弟Full不変、release／focus／ModifiersChangedと新規入力の順序、最新rect、一度だけ消費 |
| viewport_input_child_skip_preserves_keys_focus_ime_terminal_and_real_text_edit | 実TextEditのPreedit→skip×2→他viewport Full→Commit／Disabled＋新規入力。keys_down空・modifier解除・composition終端・TextEdit focusと文字列・再配送なし |
| viewport_input_state_replacement_retains_queued_terminal_events_and_owner | 実Stateの初回take前／取得済み双方で、保留＋release／focus／ModifiersChanged／Disabledを移譲。child ID・metadata・一度のdrain・新Stateの入力順序 |
| wgpu／glowのviewport lifetime回帰各1件 | 実initialize_or_update_viewportのnative再生成／deferred→immediate変更で保留と実State入力を保持。logical remove／同ID再openは空、兄弟不変。wgpuは実通常GC helperも通す |
| normal_viewport_gc_ends_input_policy_before_logic_and_same_id_reopen | 実Context GC、古いRawInput live metadata、run_logic、同ID再open・pass resetを通し、旧composition／focus contract／retryを終端。非表示・最小化siblingは不変 |
| changing_text_focus_ends_the_previous_fields_hidden_retry | 旧欄を描かず別欄へfocusを移すと、旧retryがそのcontract寿命を越えて残らない |

変更部分を一時的に戻す変異試験を行い、各回で元のbyte列へfinally復元した。

| 部分復元 | 修正を外した結果 | 復元後 |
| --- | --- | --- |
| skip判定前にStateをdrain | 5件中2件失敗、exit 101（take回数・終端列欠落） | 最終suite成功 |
| EpiIntegration共通pendingを復活 | 対象1件失敗、exit 101。childでa＋B0を観測 | 最終suite成功 |
| egui-imeのGC output_hook無効 | 対象1件失敗、exit 101。GC後も対象compositionが残る | 最終suite成功 |
| native State置換前移譲helper無効 | 6件中3件失敗、exit 101（core／wgpu／glowの終端列欠落） | 最終suite成功 |

ログは`target/egui037-stage2c2-mutation-*.log`、変異結果も
`target/egui037-stage2c2-results.json`へ保存した。
最初のfocused compileではtest fixtureのcallback引数数とViewportClass Debug制約の誤りがあり、
それぞれ新版の2引数closure／直接比較へ修正した。新しいproduction契約の変更で回避していない。

### 検証・未実施と後続境界

最終command・件数・exit codeは`target/egui037-stage2c2-results.json`、実行scriptは
`target/verify-stage2c2.py`。Rust 1.99.0、locked／offline、MSBuild node reuse無効・jobs 1。
root fmtと準備fmtは対象が異なるため、それぞれfmt／fmt --checkを明示実行する。

```powershell
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence --lib --locked --offline -- --nocapture
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence,miv-test-script-window-witness --lib --locked --offline -- --nocapture
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --no-default-features --features glow,default_fonts --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit --lib --locked --offline -- --nocapture
cargo test --manifest-path vendor-next/Cargo.toml -p egui --features miv-repaint-witness --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --locked --offline
cargo check -p mimageviewer --bin mimageviewer-core --features test-script --locked --offline
cargo fmt
cargo fmt --check
cargo fmt --manifest-path vendor-next/Cargo.toml --all
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
git diff --check
```

| 最終検証 | 結果 |
| --- | --- |
| eframe lib（default＋glow／persistence） | 34 passed / 0 failed / 1 ignored、exit 0 |
| eframe lib（上記＋witness） | 38 passed / 0 failed / 1 ignored、exit 0 |
| eframe lib（glow／default_fontsのみ） | 29 passed / 0 failed / 0 ignored、exit 0。wgpu用live fixtureはこの構成ではcompile対象外 |
| egui-wgpu lib | 16 passed / 0 failed / 0 ignored、exit 0、GPU skip無し |
| egui／egui-winit／egui-ime lib | 106／43／28 passed、失敗・ignore無し、各exit 0 |
| root core check 通常／test-script | 両方exit 0、旧graph使用 |
| root fmt／fmt --check、準備fmt／fmt --check | すべてexit 0 |
| git diff --check、root不変・index空・準備graph実体・検証後source hash照合 | 成功 |

liveのsynchronous_damage_returns_before_outer_paint_gateはignoreと子再実行用--ignoredを維持し、
今回の実行commandへ--ignoredを付けていない。既存scheduler／deadline回帰も全libで維持された。
検証済みRust／manifest／lock hashを同prefixのtested-source.jsonに、root不変・対象file一覧・
graph実体・index照合をaudit.jsonへ保存する。別contextの実装担当の内部確認で見つけた
State置換時の入力移譲漏れは修正し、実State fixtureとsource接続を再確認して未解決重大指摘なし。
既存scene.rsのEPSILON、witnessのfetch_update、feature別未使用variant、rootの警告は残る。

案B・Windows最小化中のUI継続、immediateの描画資格・非描画action完結は2c-3へ残す。
Closeの受付・拒否／承認・保留snapshotからの要求消費・backend／Contextのpass外退役は2c-4へ残す。
現在backendからContext::retire_viewportを呼ぶ経路はない。通常GCとは別に、明示Close退役時にも
同じ対象のpolicyを終端する接続が2c-4で必要であり、今回のoutput_hookだけで完了とはしない。
rootのlogic-only Close確定追加回帰も後続である。

設計との重大な食い違いはない。実State再生成時の入力移譲は同じlogical ownerの寿命保持として
今回に含め、Close契約は先取りしなかった。実装担当の内部確認でも所有・寿命と後続範囲を照合した。
live・製品build／起動・test-full.ps1・ネットワーク・git add／commit／branch操作は行っていない。
