# 第2回レビューの確認範囲と検証

対象は前回レビュー終点`512b49d4d`から、実装修正`8632af6a9`と文書更新`cf4a3ca50`まで。
開始時のHEADは`57fc2bc36`。追加のlease公開修正1件、マニュアル等2件も取り込んで照合した。
17 commits / 31 files / +2,310 -255。履歴は`commits.txt`、全変更ファイルは`changed-files.tsv`。
レビュー以外のアプリ修正、commit、push、署名、配布物作成、アプリ起動は行っていない。

## 方針・文書

前回読んだ基本方針、architecture、UI responsiveness、display、入力/keymap、出力、Remote、build/testの文書を前提に、今回は変更された`bake-stage-unification-plan.md`、`detached-rework-plan.md` §2/§11と各producerを照合。
「高性能環境向け」はUIで時間上限のないI/O/展開/外部COM待ちを許す理由にはしない。
detachedはrequestの所有contextを既存mount境界へ返す変更を評価し、無関係なcontextを失効させる修正は提案していない。

## 機能ごとの確認範囲

| 機能 | 主な変更ファイル・確認した境界 | 結果 |
|---|---|---|
| 関連付け起動 | `open_with.rs`, `external_tool.rs`: 各pathの成功/失敗、次段への再投入、部分失敗、temp transfer | F02解消 |
| 出力AI/編集 | `books.rs`, `materializer.rs`, `creative_lut.rs`, `external_tool.rs`, `ui_fullscreen.rs`: params→LUT、AIモデル選択、runtime、注釈/crop、cache identity、通常/stack、単枚/製本/一括/Merged | R05–R09/R14/R15 |
| 一括準備/worker | `ui_dialogs/export_batch.rs`, `export_batch.rs`: UI準備、フレーム境界、source固定、完了/cancel、lease | R04/R08。worker leaseは適切 |
| 一括編集/context | `edit_bundle_bulk.rs`, `app.rs`, `viewer_context_registry.rs`: confirmからowner固定、override snapshot、worker完了、回転だけ、終了drain、owner消滅、同一/別page sibling、Undo | 主なP1誤配送は解消、R11残存 |
| gamepad | `app/gamepad_input.rs`, `gamepad.rs`: enabled→OFF、物理切断、consumer保持、repaint、overlay、live rating、stop/restart | 保持入力は解消、R12 |
| 動画strip | `app/native_video.rs`: hidden/visible、last content/span、toggle/直接指定/cycle、保存値との接続 | F07解消 |
| 右情報パネル | `ui_fullscreen.rs`: 確保幅を受け渡す全caller、lock有無、狭幅、音楽/分析/VST分岐 | F11解消 |
| ピクセル配置 | `displayed_image_transform.rs`, `ui_fullscreen.rs`: half tie、整数平行移動、unit内/間の配置、描画帯、異寸法/trim/DPI/scale | R01を修正前後で数値再現 |
| 関連付け一覧 | `app.rs`, `ui_dialogs/context_menu.rs`: prewarm、in-flight、miss、空結果、cache lifetime、folder変更 | R10/R13 |
| 外部変更監視 | `app.rs`: notify消費/debounce、再要求、worker scan、owner照合、folder/世代、削除との競合、apply | R02/R03 |
| 設定移行 | `settings_db.rs`: bootstrapと通常saveの区別、移行失敗→別設定save→load、marker | F15解消 |
| 操作表示 | `context_menu_model.rs`, `ui_dialogs/context_menu.rs`: keymap snapshot、再割当/未割当、native/fallback | F16解消 |
| Remote/共通API | `app.rs`, `lib.rs`, `export_batch.rs`, `external_tool.rs`, `tests/export_integration.rs`: mandatory lease、barrier、closureのdrop、全producer、非Windows cfgの分岐を読解 | F09解消。実端末/GPUは未実施 |
| 公開文書 | `README.md`, `htdocs/mimageviewer/{index,privacy}.html`, `manual/{books,changelog,export,external-tools,settings}.html` | 焼き込み設定の説明をR07と照合。waveform保存の追記は実コード変更なし |

全31ファイルの差分を確認。大きいapp/testファイルは変更hunkと関係producer/consumerを追跡した。
操作機器の実機検証や全v3.4.0以降の差分を再度最初から読んだという意味ではない。第1回の機能台帳と今回の修正差分をつないで再確認している。

## 実行した検証

| 検証 | 結果 | 証拠 |
|---|---|---|
| 一括編集の狭いlib test | 19 passed | `test-bulk.log` |
| 全体Rust gate | PASS / 8,159 passed / 0 failed / 36 ignored | `test-full-current.log` |
| 上記のmimageviewer lib | 7,235 passed / 30 ignored | 同ログ |
| `cargo fmt --check` | exit0 | `test-fmt.log` |
| UI危険文字検査 | 0件 / exit0 | `test-glyphs.log` |
| 現ソースのgeometry probe | 実行成功、gap不一致を再現 | `geometry-probe.log` |
| 基点ソースのgeometry比較 | 等寸法の旧問題と、異寸法の新退行を分離 | `geometry-probe-baseline.log` |
| production compositorの注釈probe | 実行成功、元寸法なしで位置・寸法ずれを再現 | `annotation_probe.log` |

初回`test-full.log`は他作業の編集中にintegration testが先に更新され、公開leaseがまだ見えずE0433になった記録。安定した`8632af6a9`で再実行して通過したので、現在の製品エラーには数えていない。runner timeoutでもない。
全体gateはworkspace、pack-build-tools、およびworkspace外のegui-wgpu/eframeテストを含む。
Remote WebのJSは今回変更なし。前回の382 passedを参照し、今回は再実行していない。非Windows shadow checkと実装者のmutation testは独立には再実行していない。

## 再現用probe

リポジトリ直下で実行する。事前に本体と依存のdebugライブラリがビルド済みであること。

```powershell
python docs/review-v3.5.0/round2/geometry_probe.py
python docs/review-v3.5.0/round2/geometry_probe.py 512b49d4d
python docs/review-v3.5.0/round2/run_rust_probe.py docs/review-v3.5.0/round2/annotation_probe.rs mimageviewer egui comic_core image
```

geometryは関数をsourceから抽出し、既存egui rlibでコンパイルする。旧版比較もリポジトリの実コードを変更せず`git show`から抽出する。
注釈probeは本体ライブラリの公開`write_composited_page`を直接呼ぶ。Cargo fingerprintで対応する依存ライブラリを選び、違うfeatureで作った型を混ぜない。16×16のfixtureと出力は`target/review-v350-round2/annotation`。data_dirも同配下へ明示override。AI runnerだけを決定的な2倍画像生成にし、モデル初期化/GPU推論は行わない。
probe作成途中の依存ライブラリ不一致/Windows import library探索はprobeのリンク設定を直して解消した。アプリのビルドエラーとしては扱わない。

## 実機で残る確認

第1回`../verification.md`のマトリクスを引き継ぐ。今回の修正後は特に次を確認する。

- 外部Shellの部分起動失敗、起動済みアプリ、別アプリ登録の追加/削除、COM遅延。
- 連続上書き中のfolder scan、scan中の移動/同folder再読込/削除、export準備中の一覧変更。
- 同じページを別窓で開いた一括貼付/解除とUndo、gamepad切断直前の★preview。
- 1000/1001px混在・trim・見開き・横連結・回転・DPI変更時の接合と入力座標。
- Remote acquire中のAI出力/cancel、初期化待ち、切断/再接続とGPU資源の停止。

確認済みコードの修正なしなので、今回のレビューでは検証用アプリのbuild/起動は行っていない。通常profileも配布物も変更していない。
