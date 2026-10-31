# egui-winit 0.37.0 のローカル変更（段2a-1）

原本の版・archive checksum・VCS・全ファイルhashは
[upstream-provenance.json](../upstream-provenance.json) に記録する。
0.37.0原本へ必要な旧差分を付け直し、旧manifestを上書きコピーしていない。
README、Cargo.toml.orig、同梱Cargo.lockと、元のclipboard等の他ソースは原本を保持する。
準備graphは親の [Cargo.toml](../Cargo.toml)、製品graphは旧vendorのままである。

## 差分と分類

| 項目 | 分類 | 実装・根拠 |
| --- | --- | --- |
| Windows Process基本機構 | (a) 本家を土台 | 0.37にもProcess遮断があるため旧基本機構の二重追加はしない。HashSetの配送履歴不足だけを次行で置換する |
| Hidden / Forwarded | (b) 保持 | StateごとのHashMapが物理holdの配送履歴を所有。Process repeatでForwardedをHiddenへ戻さない |
| releaseとfocus終端 | (b) 保持 | releaseはIME許可・logicalに依存せずremoveしてHiddenだけ遮断。focus喪失は対象Stateだけclear。IME許可変更・Disabledでは履歴を消さない |
| Disabled producer通知 | (b) 付け直し | `State::on_ime`で非推奨通知を限定的に配送。WindowsのDisabled-only取消を共有normalizerへ届ける。Enabledも保持（旧Linux例外を維持）。理由付き非推奨許容は変換armに限定する |
| State→正規化→TextEdit回帰 | (b・新規) | `disabled_ime_tests.rs`。本物のState変換・準備用egui-ime plugin・実run_ui TextEditを通す。合成normalizerやwindowは使わない |
| trace・文字秘匿 | (b) 保持 | `ime_key_trace.rs`の既定無効・共通時計・worker I/O・append範囲観測を維持。構造体PreeditとDeleteSurroundingへ適応。新版の印字キーIntlBackslashも他の文字キーと同じく秘匿。通常ログのlogical key全文とinvalid rangeのpreedit全文も出さず、入力文字と範囲を秘匿する |
| 配送・State・網羅回帰 | (b) 保持 | `keyboard_input_tests.rs`へ既存17件を全移植。6入力×IME許可2通りの長さ1〜6全3,257,436列を省略しない |
| egui-ime依存・clipboard共有 | (b) 保持 | 通常path依存は準備用`../egui-ime`。Key/Text/clipboardより前にProcess判定。共有winit条件でCtrl+Shiftを保持し、専用キー・Insert/Delete・論理優先/物理fallback・CRLF・空Paste・early returnも保持 |
| 新modifierとclipboard API | (a) 本家を土台 | Stateのmodifier owner・イベント順・runtime OS判定を維持。0.37のclipboard_shortcuts設定とimage paste fallbackも維持し追加回帰で確認。modifier変換をprivate helperへ挙動不変で抽出 |
| 旧has_sent_ime_enabled・合成通知 | (c) 不要 | 新TextEditと正規化はPreedit/Commitを直接処理する。旧enable latchやCommit後の合成Disabledを戻さず、実OS通知だけを互換配送する。新Preeditのactive_range_chars・Noneを維持 |
| 旧300ms補正 | (c) 撤去済み | 時間窓・pending boolを復活させない |
| Windows枠なし窓の影 (§1.426) | (b・新規) mIV固有契約の復元 | 本家 #8169 の常時影要求と #8449 のwinit fullscreen限定除外は、独自borderless全画面を使うmIVではv4.8.0からclient原点を1pxずらす回帰になる。生成時の `with_undecorated_shadow(false)` とDecorations時の `set_undecorated_shadow(false)` で全枠なしviewportをv4.7.1の影なしへ戻す。影を必要とする窓がないためper-viewport APIは追加しない。本家へは報告しない。非表示実HWNDで生成・true→false更新のclient/window原点・寸法一致を固定し、修正前は両方の原点不一致で失敗。タスクバーとの因果・native動画の解消は実機未確認 |

新manifestの追加は共有crateの依存edgeと、headlessテスト専用egui default_fontsだけ。
通常の共有crate依存から既定フォントは入れない。新版依存版・featureは原本を基準にする。

## 回帰の境界

Disabledは同一/別pass、focus喪失後、Commit後、重複通知/空Commit、通常選択、兄弟State/viewport、
直後の通常Enter/Escapeを検査する。Disabledでphysical holdが消えないことも別に検査する。
Unicode byte→character範囲変換、invalid/None範囲、Enabled配送、modifierイベント順、
Ctrl+Shift+C、clipboard無効設定、画像paste、traceの新variant秘匿も検査する。
実windowを必要とする`take_egui_input`のgeometry更新だけを除外し、Stateが所有するRawInputを
`take`して実Contextへ渡す。headless FullOutputは検査後に明示破棄する。

本家同様Disabledを捨てる変異で取消回帰が失敗し、復元後成功する証拠は
[段2検証台帳](../../docs/egui-0.37-stage2-verification.md) に記録する。
この文書は実装担当の内部確認であり、正式な独立レビュー・製品操作差の受入れではない。

段2a-2のIME中断要求・discard集約、段2a-3の非描画退役、eframe/egui-wgpuには触れない。
vendor削除は、Hidden/Forwardedの配送保証とDisabled取消が本家同等になり、traceも不要と
判断できた時点とする。Enabled/Disabledを警告削減だけで削除しない。

## ライセンス

宣言`MIT OR Apache-2.0`を維持。原本archiveに本文がないため、旧vendorの同名本文を
byte変更なしで同梱する。

| コピー元 | SHA-256 |
| --- | --- |
| `../../vendor/egui-winit/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../../vendor/egui-winit/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |


## Stage 3: terminal command ownership (design revision 4.5, 2026-10-17)

RetireApproved belongs to the eframe pass-outside consumer. Reaching ordinary
window-command processing is a contract violation, rather than a Close event or
a silently dropped command. Existing Close/CancelClose behavior is preserved.

## Stage 3: formal vendor graph

Earlier descriptions of vendor-next/preparation-only graphs are historical.
Stage 3 uses the formal vendor/ crates from the root Cargo.toml and Cargo.lock.
Each normalized vendor manifest has its own [workspace] and [patch.crates-io]
pointing at the same formal egui, egui-winit, egui-wgpu, and eframe crates.
Cargo.toml.orig and the upstream README are retained. Original upstream lockfile
hashes remain in the provenance inventory. Active standalone Cargo.lock files were
initially copied from the root lock during WIP setup and are resolved/recorded by
ordinary offline Cargo; they are not original upstream lockfiles.
The shared IME implementation lives in crates/egui-ime; egui-winit and
eframe use ../../crates/egui-ime. Archive/VCS/checksum and original file hashes
were moved to vendor/upstream-provenance.json. Graph migration is not evidence
that runtime checks or release gates have passed.
