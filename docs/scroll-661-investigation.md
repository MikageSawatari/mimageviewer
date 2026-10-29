# 5ch >>661: ツリー移動で一覧の保存位置より上へ戻る調査

調査日: 2026-10-29〜30。調査専用 `scroll-661`。**製品コードの修正・コミット・v4.10.0 への取り込みは行わない。**

## 対象と実施条件

- 基準: `v4.9.0` = `7fea5839dfe3f3685080ab6956bf1b4a83b57db1`。
- master / 本作業ツリーの基点: `45366aff56bcb811c1c57f0105fdf662f5c03047`。
- 調査中に master は `42c2119ed203d38ef73c9287bfaf9a1b27c51e13` へ進んだ。差は backlog 文書だけで、製品 source は上記の試験基点と同一。以下の master の実行記録は試験基点のもの。
- 実行機: Windows 11 (`10.0.26300`)。Windows 10 の実画面検証は未実施。
- native window / 製品 exe / デスクトップ操作なし。既存 `eframe::HeadlessEpiIntegration` で本体の logic / render / frame 終端を実行し、実際の走査・詳細情報取得 worker、採用・復元、egui ScrollArea を通す。返る表示コマンドは適用しない。
- クライアント領域は既定の 1280×800 logical points、倍率 1、ツリーペイン表示。Full-frame backend は実経過時刻を使い、worker 待機のフレーム間には 16 ms を置く。描画・ログ出力の時間も加わるため固定 60 Hz ではない（egui 単体試験のみ時刻を固定）。
- テストごとに `phase_c_support::setup_app` の TempDir / data_dir / DB を使い、App と worker を先に破棄する。通常の設定・実データは変更しない。
- 既存の Full-frame headless fixture と同様に初回起動先の自動選択・更新確認・トレイ生成・名前索引 / LUT の初期処理を止める。`RawInput.focused` と ROOT viewport の focused を共に false にし、非アクティブの headless 描画でプログラム復元を調べる。test-only key-input lock は状態初期化・直列化のために保持する（この lock 自体が OS 入力を遮断するわけではない）。クリップボード取得・Remote は既定 OFF のまま。
- 32×48 の有効な JPEG を 1 枚含む ZIP を 200 個 / 100 個生成。JPG 対照では戻り元 200 個だけを同じ画像の JPG に置換し、中間の 100 個は ZIP のまま。ZIP の選択行のページ数が実 worker 経由で `Some(1)` になったことも検査する。
- 全 build / test に `MSBUILDDISABLENODEREUSE=1`。cold build の turbojpeg 並列失敗後だけ単独 `-j 1` で準備し、通常の反復は並列 build。

[報告 >>661](https://egg.5ch.io/test/read.cgi/software/1782419551/661) は Web ツールで取得不能だった。初報・比較結果の根拠は依頼文に提示された内容であり、スレッドの後続投稿まで読めたとは扱わない。返信文は作成していない。

## 再現結果

**v4.9.0 と master の双方で再現した。カーソルは最後の ZIP のままで、保存・復元した位置が描画後に上へ変わる。** 最終 source・入力隔離条件を揃えた実行結果は以下。数値は App の canonical offset（logical points）であり、描画側の実 offset は次の表と生ログで分けて示す。

| 条件 | v4.9.0 初回 | master 初回 | 同じ exe の UTF-8 再採取 |
|---|---:|---:|---|
| ZIP 200→100→200、詳細、バックアップ OFF | **5040→2940** | **5040→2268** | 両版 5040 を保持した回もある |
| ZIP 200→100→200、サムネイル、OFF | **11760→7595** | **11760→8330** | v4.9.0 は保持、master は **11760→2450** |
| ZIP 200→100→200、詳細、ON | 5040 を保持 | 5040 を保持 | 両版保持 |
| ZIP 100→200→100、詳細、OFF | 2240 を保持 | 2240 を保持 | 両版保持 |
| JPG 200→ZIP 100→JPG 200、詳細、OFF | 5040 を保持 | 5040 を保持 | 両版保持 |

初回は報告の比較結果まで一致した。一方、自然な worker 完了順では毎回の再現を保証しない。PowerShell 5.1 による日本語 phase 名の文字化けを避けて同じ exe の標準出力を直接採取し直した回では、ZIP 詳細の自然順ケースも保持された。これは隠さず **完了順序に依存する結果**として扱う。再現するまで無制限に反復したものではない。

実 worker の復元を戻り先の最初の描画前に完了させる回帰試験は、両版・両実行で失敗した（初回 v4.9.0 **5040→2576**、master **5040→1960**、再採取はそれぞれ **392 / 840**）。復元処理を維持して旧 egui 状態を引き継がない fresh context 対照はすべて **5040** を保持した。移動量はフレームの実経過時間に依存し、固定の行数ではない。

### 保存・復元・描画の記録例

v4.9.0 初回の ZIP 詳細ケース。すべて選択は `0199.zip`（index 199）。戻った後の App 側は 200 行・natural height 5600・items generation 3 のまま。

| 時点 | 保存値 | ensure-visible 要求 | App offset | egui の実 offset / 上限 | generation |
|---|---:|---|---:|---:|---:|
| 元の 200 件一覧を退出 | 退出時に 5040 を保存 | 消費済み | 5040 | 5040 / 5040 | 1 |
| 200 件へ採用・復元完了、最初の描画前（frame 21 終端） | 5040 | true | 5040 | **0 / 2240（直前の 100 件一覧の状態）** | 3 |
| 戻り先の最初の描画後（frame 22） | 5040 | false | **4984** | **4991.5 / 5040、速度 -4562.4** | 3 |
| 詳細取得完了を確認した観測点（frame 64） | 5040 | false | **2940** | **2928.0 / 5040、速度 -432.2** | 3 |

`pending_grid_scroll` は復帰時の全観測で None。復元は history の offset と `scroll_to_selected` が担う。中間の 100 件一覧は 100 行・natural height 2800・generation 2・上限 2240。viewport は通常 587、詳細取得の下部バー表示中は 563。行境界へ拡張した egui content height は通常 200 件で 5627、100 件で 2827。

上表の最終点は「詳細 Ready かつ canonical offset が 4 フレーム変化しない」観測であり、native velocity が完全に 0 になるまで待った意味ではない。mIV の半行未満の読戻し許容と行スナップにより canonical と egui offset は少し異なる。**大きな位置ずれはその両方に生じている。** 保存値はまだ 5040 のままで、選択や items 再構築が原因ではない。

Windows 10 の報告者環境の runtime ログは取得していない。上記は Windows 11 上の v4.9.0 / master source に対する headless の確認であり、報告者の実機で同じ原因と確定した診断とは区別する。

## 確認できた原因とコード位置

守るべき条件は、フォルダの保存位置を現在の一覧・レイアウトへ復元した後、入力のない描画が旧一覧の制約でその位置を変更しないこと。

1. ツリーの要求は `post_render_main_navigation` → pane scan worker → ready の通常採用に入る。`save_leaving_folder_grid_position` は移動前のフォルダへ正しい位置と選択を保存する。
2. `resume_loading_items_after_sidecar` は移動先の保存値を `scroll_offset_y` と選択へ戻し、`scroll_to_selected` を立てる。ここまで保存先・復元値は正しい。
3. `render_details_list` / `render_grid` は現在の件数・viewport で extent を算出し、`apply_scroll_to_selected` と clamp を行い、正しい offset を `ScrollArea::vertical_scroll_offset` へ注入する。
4. egui は同じ ScrollArea ID の **前フレームの `max_offset`** を保持している。注入後の `AxisPhysics::step` が、新しい長い一覧に対して有効な位置を、短い旧一覧に対する overscroll と判定する。rubber-band のばねが上向きの速度を生成する。
5. 現在フレームの content / viewport から egui の上限が更新されるのはその後。既に変わった offset と速度は残る。mIV の読み戻しがその値を canonical offset に採用する。`scroll_to_selected` は描画開始時に一度消費されているため、選択行は移動せず画面外になる。

位置対応（行番号は製品コード未変更の各版）:

| 処理 | v4.9.0 | master |
|---|---:|---:|
| 移動元の保存 `save_leaving_folder_grid_position` | `src/app.rs:41967` | `src/app.rs:42305` |
| 保存位置の復元を含む `resume_loading_items_after_sidecar` | `src/app.rs:42771` | `src/app.rs:43109` |
| 選択行の可視化 `apply_scroll_to_selected` | `src/app.rs:60803` | `src/app.rs:61166` |
| 詳細描画 `render_details_list` | `src/ui_main.rs:17235` | `src/ui_main.rs:17502` |
| サムネイル描画 `render_grid` | `src/ui_main.rs:19165` | `src/ui_main.rs:19464` |
| 注入された offset の受取 | `vendor/egui/src/containers/scroll_area.rs:823` | 同じ |
| 旧上限による physics 構築 / step | 同ファイル `:919` / `:994` | 同じ |
| 新上限への更新 | 同ファイル `:1671` | 同じ |
| overscroll のばね | `vendor/egui/src/containers/scroll_physics.rs:124` | 同じ |

詳細表示の注入は master `src/ui_main.rs:17608`、読み戻しは `:17733`。サムネイル側も注入 `:20052`、読み戻し `:20432` 付近で同じ境界を持つ。

**単体での原因切り分け**: 同じ egui に、短い content 2800 → 長い content 5600 と明示 offset 5116 を与えた。入力なしで実 offset が **5081.174**、上向き速度が **-3950.2** になった。テスト内だけでばねを外した対照は offset **5116**、速度 **0**。これは製品の修正案ではない。App 側の対照では、復元・実 worker・描画を維持したまま旧 ScrollArea 状態を引き継がない新しい egui context を使う。

確認範囲で、短い一覧の clamp 値を誤ったフォルダへ保存した形跡、ZIP 詳細取得が選択を別行へ動かした形跡、詳細取得完了による items 再生成はない。詳細取得の完了で下部の 25 pt 予約領域が消え、viewport / extent が変わる経路も実際の試験に含めているが、大きな上方向移動はそれより前に始まる。

バックアップ ON は同じ hydration owner に sidecar 確認・静止待ちを加え、OFF でもページ identity 準備 worker は動く。Full-frame では採用が grid 描画より後、hydration poll は frame 終端にある（master `src/app.rs:99261`）。ZIP の早い準備が同じ frame の終端に間に合うと、新しい一覧の最初の描画前に復元される。JPG 対照と ON の成功ログでは、新しい rows を offset 0 で描いた後に復元され、egui の上限はすでに 200 件用だった。これが本機で比較差を生む完了順序として確認できた。ZIP / OFF 自体を必要条件とはせず、報告者の実機での因果確認とも混同しない。

## v4.9.0 と master の差

`save_leaving_folder_grid_position`、`resume_loading_items_after_sidecar`、`apply_scroll_to_selected`、`poll_details_meta_load`、`begin_sidecar_restore` は関数本文が一致。egui の `scroll_area.rs` と `scroll_physics.rs` はファイル全文が一致する。

`render_details_list` の差は `items_are_tag_view` 引数 2 箇所、`render_grid` の差はサムネイル名設定の引数追加 2 箇所。今回の offset 注入・extent・読み戻し処理の差はない。比較証跡は `target/scroll-661-version-comparison.txt`。

## 修正方針案・影響・規模

**App の canonical offset と、egui 内部の幾何・慣性という二つの所有者の契約を直す。** 新しい復元待ち bool / pending、遅延、毎フレームの再復元、履歴復元の無効化は追加しない。

- 現在フレームの content / viewport extent と明示位置更新の優先権を同じ描画境界で確定し、旧一覧の bounds / velocity が新しい一覧の復元を変更しない設計にする。既存 `GridScrollIntent`・offset・ensure-visible の責任を先に整理する。
- egui に現在レイアウトの extent を渡して physics より先に確定できる API、または測定後に現在 bounds で physics を評価する構造を検討する。復元済みの正しい値へ old-bounds のばねを適用してから補正する形を残さない。
- 一覧の logical lifetime と入力 gesture の lifetime を明確にする。フォルダを変えた後に旧一覧の慣性を引き継がず、同じ一覧の正当な drag / scrollbar 操作は保つ。単純な path ID 変更や generation ごとの無条件初期化だけを完成案にしない。リサイズ・filter・再走査でも content extent は変わるためである。
- おおむね詳細 / サムネイルの 2 描画経路、共通 scroll 契約、vendored egui の該当境界とそのテスト・文書が対象の中規模修正。DB schema、ZIP 形式、sidecar 形式の変更は不要と考える。
- 既存の wheel、scrollbar drag、サムネイルの App 所有 touch / row snapping、詳細の drag、Home/End・リング / gamepad の Top/Bottom、選択行の可視化、履歴 / BS / 検索復帰、A/B / viewer context ごとの位置を保つ。所有境界を変える本実装では独立レビューと関連回帰が必要。

簡素化の検討: 読み取り専用のフォルダ移動や詳細取得をモーダル化・遅延させる必要はない。すでに計算できている現在レイアウトと、既存の復元意図を一つの適用境界へ揃える方を優先する。まれな I/O 失敗の回復処理はこの問題の範囲に入れない。

## 回帰テストと検証証跡

追加は `src/app/tests/scroll_661.rs` と、その test-only module 登録 `src/app/tests.rs`。正しい復元位置を要求するため、製品未修正のまま失敗する試験を残す。実ファイル走査、通常のツリー nav 裁定、hydration 完了、ZIP 詳細取得、実際の egui layout / readback を通す。カーソル・保存値・復元フラグ・canonical offset・egui State（実 offset / max / velocity / viewport）・件数 / 高さ・items generation をログへ出す。

通常 feature set（`portable` なし）、test profile、`--locked --offline`、`--test-threads=1 --nocapture`。全 lib の実行ではなく、下記 module の 9 件だけを選択した。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo test --manifest-path target/scroll-661-v490/Cargo.toml --target-dir target -p mimageviewer --lib scroll_661:: --locked --offline -- --test-threads=1 --nocapture
cargo test -p mimageviewer --lib scroll_661:: --locked --offline -- --test-threads=1 --nocapture
```

- v4.9.0: **5 PASS / 4 FAIL**、exit **101**、11.22 秒（compile 1m04s）。`target/scroll-661-v490-final-isolated.log`。
- master: **5 PASS / 4 FAIL**、exit **101**、12.07 秒（compile 2m07s）。`target/scroll-661-master-final-isolated.log`。
- 上記で失敗した 4 件は `egui_explicit_offset_after_content_growth`、`zip_200_to_100_backup_off_details`、`zip_200_to_100_backup_off_thumbnail`、`zip_200_to_100_fast_hydration_details`。正しい復元位置を期待する回帰検出であり、compile / harness failure ではない。
- 日本語 phase 名を native UTF-8 のまま保存するため、**同じ exe・同じ最終 test source** を一度ずつ再実行。v4.9.0 **7 PASS / 2 FAIL**（24.68 秒）、master **6 PASS / 3 FAIL**（25.15 秒）、両方 exit **101**。共通の失敗は egui 単体と fast hydration。master は自然順のサムネイルも失敗。自然順の ZIP 詳細は両版で保持。`target/scroll-661-{v490,master}-trace.log`。
- 再採取のコマンド・対象 commit・binary / test-source SHA-256・exit / 件数は `target/scroll-661-final-evidence.json`。捕捉用スクリプトは `target/scroll-661-capture-native.py`。Python recorder の exit 0 は記録完了であり、Rust test の成功ではない。
- `cargo fmt --all -- --check`: **PASS**（`target/scroll-661-fmt-final.log`）。`git diff --check`: **PASS**。
- 関連既存試験 `app::sidecar_restore::tests::section1339_adoption_precedes_real_sidecar_hydration_and_commits_once` は master の同一製品コードで **1 PASS**（`target/scroll-661-sidecar-control.log`）。最終結果に先行 harness の ON timeout は含めない。
- 全体 gate、配布版 / portable feature の build、Windows の実画面試験は未実施。製品未修正・test-only の調査のため dev-runtime 確認 binary は作成しない。

独立 Sol / xhigh reviewer はコード・harness・原因説明を読み取りレビューした。focus 隔離の指摘を反映済みで、他の重大な指摘なし。reviewer は Cargo を再実行しておらず、最終実行ログの検収まで済んだとは扱わない。最終の検収・コミットは依頼どおり ClaudeCode へ引き継ぐ。

先行 harness の限定: 最初の版には詳細モードで thumbnail の列数を使うログ計算、必要な完了通知 consumer の不足、局所 solid style に上書きされる App 対照設定、復元 poll を本体と違って描画前に置く問題があった。製品の失敗とは分け、最終版は本体 Full-frame driver と fresh context 対照を使う。先行ログだけの timeout、比較の合否、対照不成立は製品の検証結果に数えない。

v4.9.0 は `target/scroll-661-v490/` に tag の実ファイルを展開して検査する（worktree / junction は作らない）。未追跡 vendor は実ファイルをコピーし、Tantivy は cache の upstream 0.26.2 に tag の tracked patch を重ねた。LibRaw は既存 cache の 0.22.2 tarball を SHA-256 `627928088300ecde6ca91ffd202e189203f04ad61ad12f0fe9dc57b9a7a0fb3c` で検証して復元。元の vendor は変更しない。FFmpeg 等の配置済み native asset を使うが、本試験で動画・AI・PDF・VST は動かさない。

## 未検証範囲・実画面確認を行う場合

Windows 10 / 11 の実窓、DPI、入力デバイス、portable launcher、実際の初回設定画面は未検証。headless は OS の window manager / GPU 提示 / 実マウスを使わない。件数境界も測定していない。

ClaudeCode が別途承認を得た場合の確認枠:

- 素材: `target/scroll-661-materials/` の `test200`（JPEG 入り ZIP 200 個）、`test100`（同 100 個）、`test200-jpg`（同じ JPEG 200 個）。生成器は `target/scroll-661-materials.py`。使い捨て素材のみ。
- 承認後の既定手順に沿った disposable portable copy / 専用 data を使う。通常 profile / installed / tray 版で試さない。v4.9.0 と将来の修正候補の条件を分ける。製品 binary の起動・実窓検証用の準備 build はこの調査では実施していない。
- 既定 1280×800、詳細表示、バックアップ OFF。ツリーで test200 → 末尾選択 → 詳細完了待ち → test100 → test200。選択名と viewport の移動を分けて記録。
- 同じ条件で ON、100→200→100、test200-jpg→test100→test200-jpg を比較。サムネイルでも基準ケースを確認。
- 1 版あたり約 5〜10 分＋必要な build / 初回展開。対象の窓・マウス / キーボードを使用するため、その時間は利用者のデスクトップ入力を専有する。

現段階で報告者への追加質問は不要。必要な実窓確認は、開発側の承認された検証枠で行う。
