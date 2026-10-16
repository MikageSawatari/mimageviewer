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
