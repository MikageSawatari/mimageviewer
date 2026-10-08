# §1.221 右クリックメニュー構成

> 状態: 初回実装・区切り線追補・表示場面プレビュー追補を実装・自動検証・確認build済み
> （2026-09-13）。
> 正本は `docs/next-release-backlog.md` §1.221。

## 目的と境界

グリッドとフルスクリーンが共有する mImageViewer の右クリック項目について、環境設定から
静的項目の表示 ON/OFF、同一階層内の順序、各項目の直前の区切り線を変更できるようにする。既定の空設定では、従来の
`MenuNode` の内容、section、表示順を完全に維持する。対象の種類や選択状態による可否判定、
混在選択の拒否理由、`MenuCommand` の dispatch、キーや上部メニューの入口は変更しない。

## 永続 ID と固定枠

- `ContextMenuItemId` は表示名や実行時 payload に依存しない静的 leaf の stable ID だけを持つ。
  Root と Open With の親 ID を別に保存し、submenu を平坦化しない。
- 登録済み外部ツール群、Open With submenu、関連付けアプリ群は、元の capability-filtered tree
  にある位置と内部順を保つ固定枠である。設定 catalog、非表示一覧、共有 JSON へは出さない。
- Windows Shell 群も従来どおり native renderer が末尾に置く固定枠で、サブメニュー / 同階層
  の既存設定と Windows が返す内部順を維持する。

## 解決規則

`context_menu_model` は従来どおり可否判定済みの tree を先に構築する。その後、同一階層の元の
unit 列を static configurable slot と fixed slot に分類する。保存済み static ID は configurable
slot の間だけで並べ替え、fixed slot の位置と内容は動かさない。非表示も static item にだけ適用する。

区切り線は stable leaf ごとに `Inherit / Present / Absent` を保存する。並べ替え時は actual item ID と
`MenuNode` payload を一緒に移し、slot 側の section は既定境界としてその位置に残す。`Inherit` は
移動先 slot の既定境界、`Present / Absent` は item と一緒に移る明示指定になる。fixed slot の直前、
動的group内部、Windows Shellとの境界は設定対象にしない。最後の `normalize_menu` が空・先頭・末尾・
連続区切りを正規化する。

未知 ID、重複 ID、親階層違いは無視する。保存データに無い新規 ID は canonical 配列の位置を
空けたまま既知 ID だけを保存順で再配置するため、新項目が既定位置へ補完される。利用不能な
項目は元 tree に存在しないので復活しない。最後に `normalize_menu` を通し、leading / trailing /
連続 separator と空 submenu を除く。native Win32 と egui fallback はこの同じ解決済み tree を使う。

## 保存・共有・UI

`Settings.context_menu_layout` は `serde(default)` の独立設定で、旧 settings は空の標準設定として
読む。内部の区切り線は引き続き `Inherit / Present / Absent` の3状態で保存し、操作カスタマイズ共有
JSON にも同フィールドを含める。旧 JSON で欠落した場合は標準値となり、replace-only 取り込みは
右クリック設定も標準へ置き換える。

環境設定は「通常メニュー」と「右クリックメニュー」を別ページにする。右クリック側では Root /
Open With を別々に編集し、1項目だけの Open With 階層には意味のない上下ボタンを出さない。
確認用の表示場面を選ぶと、`context_menu_model` が固定fixtureをproductionの
`build_context_menu`へ通し、各stable leafがその場面で対象になるか、実際に直前の区切り線が
表示されるかを返す。UI側にitem-kind/capability表を複製しない。

設定画面の主表示は「表示 / 非表示」の2値とし、`Inherit` は実際の解決値へ「（既定）」を添える。
利用者が2値を選んだ時だけ `Present / Absent` を全場面共通の明示値として保存する。個別の既定復帰と、
表示・順序を保ったまま区切り線だけを全て既定へ戻す操作を用意する。選択場面で対象外、項目非表示、
同階層の先頭で最終normalizeにより線を置けない場合は理由を出して操作を無効にする。対象外・
非表示の項目には、項目表示をONにしたとき実際に現れる別の固定sceneを同じbuilder結果から案内する。
先頭や対象外でも保存済みの明示値がある場合は、個別の既定復帰だけは行える。設定ページを開く、
場面を切り替える、プレビューを読むだけでは保存対象の `Settings` を変更しない。

固定sceneは一覧の画像、通常フォルダの余白、ZIP本体（閲覧履歴）、検索結果の画像、PDF内ページ、
Stack、複数実ファイル選択、フルスクリーン画像・動画を持つ。さらに外部ツール1件・関連付けアプリ
1件を含む画像sceneを別に置く。通常sceneには両動的groupを含めず、各sceneのラベルと説明にfixture
条件を明記する。これは実行中のフォルダやOS関連付けの列挙ではなく、場面差を確認する固定例である。
全stable leafが少なくとも1sceneで対象になることを回帰で固定する。

## 受入条件

- 空設定で Grid / Fullscreen / checked の代表 tree と section が従来どおり。
- unavailable item は復活せず、unknown / duplicate / wrong-parent を無視し、新 ID は既定位置へ補完。
- static reorder / hide / separator override 後も外部ツール、Open With、関連付けアプリ、Windows Shell の固定位置と内部順を維持。
- native / egui が同じ resolved preorder を使い、正規化後に不正 separator や空 submenu がない。
- 環境設定のスクロール、上下移動、非表示、区切りの実2値、個別・一括の既定復帰、すべて表示が
  実widgetから反映される。内部3状態と旧保存値は維持する。
- settings と操作カスタマイズ共有の round-trip、区切りfield欠落を含む旧 payload の標準置換、unknown / duplicate sanitize を固定する。
- 固定sceneが全stable leafを覆い、NewFolderは画像ファイル場面で対象外、通常フォルダ余白で対象になる。
- previewの区切り線はproduction builderの最終treeと一致し、関連付けアプリ有無、先頭、hidden、
  unavailableを区別する。scene閲覧・切替後も保存対象`Settings`全体がexact不変。
- 「通常メニュー」と「右クリックメニュー」が別の環境設定ページ・検索項目になり、右クリックページで
  2値変更、個別既定復帰、区切り線だけ全既定復帰を操作できる。

## 検証記録（2026-09-13）

- 最終combined freezeでcontext-menu focused 100/100、操作カスタマイズ共有 8/8が成功。model、
  settings、native / egui menu、Grid / Fullscreen共通配線、実設定UIを含む。
- 実 widget の長い設定一覧でスクロール、上下移動、非表示、すべて表示、既定へ戻す、
  区切り3値、Open With 階層の展開を確認。Open Withでは矢印buttonを生成せず、全catalog labelを
  現在fontで測った共通列幅を両階層へ適用し、区切りcomboの左端が1pt以内で一致することを
  actual widgetで固定した。snapshot は
  `tests/snapshots/preferences_context_menu_layout{,_open_with}.png` に固定した。
- `RUST_TEST_THREADS=1` の `scripts/test-full.ps1 -SuppressCrashDialogs` は main 8349 件、
  UI snapshot 48 件だった初回実装gateに加え、§1.220追補と同じ最終freezeでmain 8378 passed /
  0 failed / 45 ignored、UI snapshot 50/50、workspace / integration / doc / vendor全targetが成功し、
  process error modeも元へ復帰した。最終完全ログは
  `target/section220-221-followup-20260913/test-full.stdout.log`、
  `test-full.stderr.log`、`test-full.exit.txt`。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
  UI glyph、viewer context audit、diff check は成功。
- 同じ source freeze の `scripts/build-dev.ps1 -PreserveRuntime` は exit 0。core は
  SHA-256 `6910EAE6ACB511D9C1BF4C77ABA673C087B9624F27CBEC99E9F978375E7A5002`
  （2026-09-13 18:36:01 JST）、remote は既存 runtime を保持した。agent によるアプリ起動・停止と
  手動 UI 確認は行っていない。

### source freeze SHA-256

| path | SHA-256 |
|---|---|
| `src/context_menu_model.rs` | `FB3926B93DD5DBD35F65A03A0988FFBCDA67AF977783E84B37E9A409B37CB45A` |
| `src/native_context_menu.rs` | `9299C82A665D7DB7261D942E6D2213D6AAF0E2983C12A567A51F6FB5F5FA7C9F` |
| `src/operation_customize_share.rs` | `4C3A3CB89BDD420FCA8A03055CA8EF45F9192879F46DC7B3FC8B2CE6E40D1554` |
| `src/settings.rs` | `0F2FB25A9C4B6147EE574B6EFCC95EA718753AFD46626D2316FF1989782D3068` |
| `src/ui_dialogs/context_menu.rs` | `F14977D2421CDC5A64138071D84AF67FCFAAC372E8236F84D6D007786E106DF1` |
| `src/ui_dialogs/preferences.rs` | `BD8039882349AD334BA94C63B679EBA612541EBAC570205B894C8A90BC3A1DC2` |
| `src/ui_dialogs/preferences/pages.rs` | `372F3CEEAD1AA6E11A62C02FA2F4C797F53D553563BB092EF63E7ACBD4453954` |
| `docs/architecture-overview.md` | `6A50101CD55702CB5EEA1409B79D41ABADDAE13720987C2D3FB5A1A10B6FBE86` |
| `docs/context-menu-unification-plan.md` | `044AAD4AEDDB483AE3A3634A6104278EABDF5D38290BF0FF1560FC525B384686` |
| `docs/operation-customize-share-plan.md` | `3650532460985D7E5228EB2755EC4B7E31F8A89EB5DFD0F7762D98BE512A672B` |
| `docs/spec.md` | `06F6BBDF68061B8C970A8067F5AFC7B9B5C9C84B4196ACC5EE15469B7F95E2D1` |
| `htdocs/mimageviewer/manual/settings.html` | `F9B4BA65BEA13FF7B60263DC0FB6C121237AE7696FC6826F4DA08B393E5641E7` |
| `tests/snapshots/preferences_context_menu_layout.png` | `C38C878C67F60528DA7B329EBDF1886DFDD06E7C141719EB38A58F595A177CC1` |
| `tests/snapshots/preferences_context_menu_layout_open_with.png` | `5C565AC617F0C2BE84C3F11C7490AB760DD702B7AC56A79654008364D029F0C2` |

本書は上表の source / golden 14 path を freeze とする。検証記録自身の hash は本文追記後に
別途照合する。

## 表示場面プレビュー追補の検証（2026-09-13）

- production builder由来の場面preview 3件、設定UI 7件、設定検索索引 10件、ページ閲覧・
  scene切替後も保存対象`Settings`全体が不変であるactual-widget回帰 1件、計21件が成功した。
  全27 stable leafの場面被覆、画像fileと通常folder余白、検索結果、ZIP本体（閲覧履歴）、
  PDF内ページ、動的groupなし / 外部tool1件・関連付けapp1件の固定条件、先頭 / hidden /
  unavailable、個別・全separator resetを直接確認した。
- `tests/snapshots/preferences_context_menu_layout{,_open_with}.png`を更新した。二値の実表示、
  既定線の小線、対象外 / 先頭、Root / Open Withの列揃えを目視確認済み。tooltipだけで案内する
  表示可能sceneは同じbuilder結果を使うため、goldenのpixel差分には含まれない。
- `RUST_TEST_THREADS=1`で`./scripts/test-full.ps1 -SuppressCrashDialogs`を実行し、main
  8382 passed / 0 failed / 45 ignored、UI snapshot 50/50、workspace / integration / doc / vendor
  全target（egui 25 / egui-wgpu 9 / eframe 15を含む）が成功した。process error modeも復帰した。
  完全ログは`target/section221-scene-preview-20260913/test-full.stdout.log`（SHA-256
  `FEA167105F766A22647C77A767EE5E2C17FA4E4E93C8B2BC5004CD923EABF775`）、
  `test-full.stderr.log`（`1F8A377C7828DC5168696D43B92EC69B3E9B63239A7D10FBDD183CF0EA606EA2`）、
  `test-full.exit.txt`（`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`、内容0）。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、UI glyph、
  viewer context audit、diff checkはすべてexit 0。ログは同directoryの`static.{stdout,stderr}.log`
  と`static.exit.txt`に保存した。
- source freezeとresident不在を再確認して`./scripts/build-dev.ps1 -PreserveRuntime`を実行し、exit 0。
  coreはSHA-256 `8F867A41B3F20E9AB57E2F80DD90FA82B4809720D35808AC52237DC48397BF4E`
  （2026-09-13 20:18:11 JST）。remote serviceはPreserveRuntimeにより既存成果物
  `A03CE402A7137613E063867C9F8338BEDC5C338FE8897325007BB35110935C64`
  （2026-09-13 03:16:07 JST）を保持した。build後もresidentは0件で、agentはアプリを起動・停止していない。
  buildログは同directoryの`build-dev.stdout.log`（`7C741DECEFAFEB8E583858E9A6CA13EEDE311C270F399F8BD5AC51B6280DCC85`）、
  `build-dev.stderr.log`（`862992F17444A35A3AE82E3CF2D837A619C31AE3C180A224F38E3F84F2E3C689`）、
  `build-dev.exit.txt`（`13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`、内容0）。
- この追補のexact source / golden hashは同directoryの`source-freeze.sha256.txt`を正本とする。

### 利用者確認（2026-09-13、表示場面追補）

確認build後、利用者から「動作は大丈夫」との確認を受けた。表示場面の多さによる設定の複雑さについて相談があり、設計担当は場面別に設定を分割せず、共通設定と表示場面プレビューを維持することを推奨した。追加のUI変更は行っていない。
