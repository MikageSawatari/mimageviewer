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

## 修正の設計

### 位置づけと採用案（2026-10-30、再確認後の改訂第3版）

利用者判断により次版 **v4.11.0 予定**で修正する。上記の「v4.10.0 に取り込まない」は調査時の判断として保持する。v4.10.0 は公開済み。本節の確認基点は master 取り込み後の `bf6431c09feab234f3d41cee3fd45e9354ed41fe`、前回調査の記録は `01af22d76`。本節は source inspection による設計であり、新しい実行結果ではない。

**採用案: App の一覧スクロール状態を一つの owner にまとめ、vendored egui に現在の content 高さと位置更新の意味を同時に渡す、縦一覧限定の opt-in API を追加する。** physics の式・既定動作は変えない。App から egui の private state を直接編集する案は採らない。現在の geometry を渡すだけでは旧速度が残り、速度を消すだけでは旧 bounds が新たなばね速度を生成するため、この二つを同じ境界で扱う。

初版 `7cfc089ab` に対する独立 GPT-6.1 Sol / xhigh review は **REVISE（P2×4、P1/P3なし）**。現在 geometry・意味付き位置更新・context 所有をまとめる構造的修正の方針には同意を得た。第2版 `7790e2b4f` の再確認では、recognizer・RowAnchor・clamp の3件は解消、3段の分割・vendor範囲・5〜8人日の見積もりも妥当と判定された。残る **P2×1（同一frameのrelease→press）、P3×1（touchのframe番号）、P1なし**を本第3版で反映する。構造的修正への同意は再確認済みだが、本改訂の ACCEPT は未取得。以下の型・API 名は提案名で、まだ存在しない。今回も本書だけを変更し、ClaudeCode の確認と再レビュー後に実装する。

| 指摘 | 改訂内容・受入条件の所在 |
| --- | --- |
| P2-1: ID交代で切替 frame の新規 press が失われる | §3「切替 frame の press と capture」。表示面の interaction ID を安定させ、press ごとの所属を一つの capture owner が持つ。§9 R1で押し直し不要を検査 |
| P2-2: touch recognizer が旧接点からコマンドを出す | §4「MainGrid の recognizer」。transport correlation を保ったまま recognizer とコマンド受理を session に接続。§9 R2で実コマンド列を検査 |
| P2-3: row anchor は refresh / resume にも使う | §2を用途に依存しない `RowAnchor` に訂正し、既存 producer を移管。§9 R3で reload / 自動 rescan / resume を検査 |
| P2-4: App の先行 clamp が `Sync` に隠れる | §2・§3の frame-local 解決結果で `Set` の意味を保持。§9 R4で先行 clamp と非ゼロ速度 / target の結合を検査 |
| 第2版の残P2: 同一frameのrelease→pressにResponseの配送証拠が残らない | §3で順序付きイベントと `Memory::interaction()` の実保持先を併用。§9 R1で旧background dragからのrelease→新pressを具体化 |
| 第2版のP3: touch replayのframe番号が曖昧 | §3・§4で同じviewportの `ctx.cumulative_frame_nr()` に統一。§9 R2で実Appのdiscard経路を検査 |

実装は§10の3段を同じブランチ内で進め、最終段の受入まで次版へ出荷しない。入力寿命の片側だけを有効化する中間状態を作らない。

### 1. 不変条件と現在の違反箇所

| 不変条件 | 正本と違反箇所 |
| --- | --- |
| 保存値・選択は退出元へ保存し、宛先の位置は宛先の表示採用・復元 owner が決める | `src/app.rs::save_leaving_folder_grid_position`（現基点 :42307）、`resume_loading_items_after_sidecar`（:43111）。ここで保存値を誤ったものにする不具合は前回試験では確認されていない。保存先や hydration の順序を変更して直さない |
| 入力のない復元後の描画は、有効な復元位置を旧一覧の制約で動かさない | `src/ui_main.rs::render_details_list`（:17502）/ `render_grid`（:19464）からの注入に対し、`ScrollArea::begin`（`vendor/egui/src/containers/scroll_area.rs` :823、:919、:994）が旧 `max_offset` と旧 `vel` で計算する。新上限の保存は `Prepared::end` :1671 と遅い |
| physics の bounds、content の配置、可視行算出、scrollbar は同じ現在 geometry を使う | App が `snapped_scroll_extent` で算出した現在範囲と、egui の前回範囲が並存している。`scroll_physics.rs::AxisPhysics::step`（:124）は与えられた bounds どおり動くので、ばねの数式自体の不具合ではない |
| canonical offset は App 所有の行境界。描画端数・native 慣性は保存位置ではない | App の `scroll_offset_y`、サムネイルの `GridTouchScrollState`、egui の `State`。詳細 :17733 / サムネイル :20432 付近の読戻しが、旧 bounds による移動まで正しい入力として採用している |
| 一覧を退出した入力ジェスチャは、別一覧・再入場した一覧へ継続しない。同じ一覧の正当な操作は継続する | 現在は ScrollArea の保存 ID と background drag / bar の interaction ID が同じ寿命に結び付く。App の論理一覧の交代が伝わらず、速度・target・grab anchor を引き継ぎ得る |
| context の位置・入力寿命を兄弟に伝播させない | `src/app/viewer_context_registry.rs::ViewerContextBundle`、`swap_viewer_context_bundle`。現在も offset / ensure / edge は bundle 所有。変更後もまとめて move / swap し、main-global な reset を作らない |

行番号は本節の基点であり、実装時は関数名を正本として再確認する。`apply_scroll_to_selected` は現在 `src/app.rs:61168`。

### 2. App 側の単一 owner と明示更新

#### 設計担当補足: Edgeと未消費RowAnchor（2026-10-30、停止後の再開決定）

以下はClaudeCodeの補足指示であり、初版〜第3版の「row anchorも排他的なrequestに含める」
という記述を置き換える。Edge等の一回限りの要求と、未消費の行位置指定は寿命が異なり、互いに排他ではない。
`GridScrollState` は同一owner内に次の二枠を持つ。

1. その描画境界で一度だけ解決する要求枠（Position / RevealSelection / Edge）。
   この枠内の優先順位は従来どおり、同一境界のedgeがensureより優先し、同一覧の新しいedgeは最後を採る。
2. 未消費の `RowAnchor(rows_above)` 枠。選択が解決されてrevealが適用されるときだけ使う。
   選択対象が見つからない、選択がない、Edgeを適用した、という理由だけでは消去しない。
   `try_select_after_load` / `apply_scroll_to_selected` / Edgeのtakeの現在の消費規則を維持する。

既存の要求とrow hintを同じownerへ移すもので、新しい状態の追加ではない。
既存2試験 `grid_cursor_rating_grid_move_then_shift_click_uses_display_order_in_both_views` と
`paired_identity_remap_keeps_checks_and_scroll_then_shift_click_uses_remapped_anchor` のassert条件は変更しない。
「Edge適用時にrow anchorを取り消すべきか」は出荷済み挙動を変える別問題のため **今回は現状維持**。
起動前hintが通常loadのresetを跨ぐ経路も維持する。`StartupListIntent` へ運ぶ変更を行う場合は
挙動不変を先に検証し、変わるなら停止して報告する。

段Aの前提監査による停止理由はこの補足で解消され、段A→B→Cの受入条件を保って実装を再開する。
以下のrequest型の説明は、この補足に従いRowAnchorを独立した配置枠として読む。

`GridScrollState` を App 投影と `ViewerContextBundle` に一つずつ保持する。既存の `scroll_offset_y`、`scroll_to_selected`、`pending_grid_scroll`、`scroll_selected_to_rows_above` の責任をここへ移す。最後の field は名前にかかわらず **起動専用ではなく、refresh / resume の選択行位置にも使う**。互換用の旧 field を併存させず、位置の読み手は owner の canonical accessor を使う。履歴・session snapshot の保存形式は従来の数値・選択のままとし、gesture identity や egui State を保存しない。

- canonical offset、一回だけ解決する `GridScrollRequest`、未消費RowAnchorの配置枠を同じownerに置く。requestは `Keep` / `Position { placement }` / `RevealSelection` / `Edge(Top|Bottom)`、placementは `KeepPosition | RevealSelection` とし、RowAnchorはこのenumの外の配置枠に置く。復元位置を受け取る操作はcanonicalを更新して `Position { KeepPosition }` にする。復元は同値でも旧慣性を終える意味を持つ。
- `RevealSelection` は既存の `current_grid_order` と `apply_scroll_to_selected` の計算を使い、既に見えていれば位置を変えない。`RowAnchor(rows_above)` は「選択項目を上から何行目へ」の共通要求。`startup_list_restore.rs::current_list_refresh_intent` → cursor 捕捉（:147付近）、`finish_main_list_open` の `ResumeList` / `RefreshList`（:531付近）、`resume_loading_items_after_sidecar` の refresh cursor 採用（`app.rs:43598`付近）を含め、旧 field の全 producer を移す。通常 reload / 自動観測 rescan でも行位置を保持し、単なる ensure-visible へ格下げしない。
- fork へ process 起動時の未消費要求を複製しない規則は、起動 intent の fork / constructor policy として維持する。`RowAnchor` という要求種類自体を「main 起動だけ」に限定しない。context に所属する refresh / resume の要求はその bundle と一緒に move / swap し、fork の既存 copy policy と未採用要求の非継承を監査する。
- `Edge` は選択・チェック・最後の画像を変更しない。現行どおり、同じ描画境界で競合する ensure より edge を優先する。復元後の reveal は `Position` の placement を更新して合成し、見えているため reveal が無移動でも復元の `Set` 権限を失わない。未消費の edge がある間の ensure / 同一覧の位置更新は edge を取り消さず、同一覧で新しい edge が来れば最後の edge を採る。別一覧の採用では旧一覧の request を移さず、宛先の restore / initial request に置換する。要求登録・合成の順序を単体テストで固定する。
- wheel / gamepad 連続スクロールなど App が直接位置を動かす入口も owner の位置操作へ通す。egui の readback を行境界へ採用する操作は、新しい明示要求を発行しない。これを区別しないと毎フレーム native 慣性を切ってしまう。
- request は現在 geometry が決まった時点で一度 take する。描画後のリング等が積んだ要求は翌描画へ残す。旧一覧の readback は、背景操作で一覧を交換する前に完了させる現在の順序を保つ。
- geometry 解決は変更前の canonical と snap / clamp 後の canonical を同じ呼出し内で比較し、frame-local な `ResolvedGridScroll` として `Sync` / `Set` / `AppGesture` まで決める。**clamp で位置が変われば必ず `Set`** とし、その後に有効範囲内の値になったことを理由に `Sync` へ戻さない。行高変更による snap 補正も同様。永続 flag は追加しない。App touch の進行中は `AppGesture` が同じ終端権限を含み、clamp 後の anchor / remainder を渡す。egui readback の通常の行丸めは新要求を発行する操作と区別する。

これらは新たな復元 bool / pending / guard の追加ではなく、既存の分散した要求の置換である。新しい async owner・待ち・channel は作らない。保存復元値の取得、worker 完了の採否・世代検査は各既存 owner に残す。

#### 設計担当補足: 最終描画geometryとR3の受入段（2026-10-30）

段Aで確認した詳細一覧の行位置捕捉不整合は、今回の修正対象に含める。
最終描画の実列数・行高・viewportを一つのgeometry ownerが記録し、行位置の保存・復元・revealが
同じ記録を使うことは段Bのgeometry契約に属する。詳細では実列数1を記録する。
これは詳細表示で再読込・自動再走査後に選択行の画面内位置がずれる出荷済み不具合の修正でもある。
fixtureの列数や期待値で隠さず、R3の両rendererによるreload / observation / resumeを段Bの受入とする。
段Aの受入は「挙動不変（R3を除く）」とし、既知red4件とR3の失敗を記録したまま完了とする。
段B末尾で元の再現4件とR3をgreenにしてから段Cへ進む。通常の使用手順・選択維持の仕様は変えない。

段Bの独立実装レビューで、既存の比率変更が次の描画より先にcanonicalを変換する経路も確認した。
最終描画のgeometryと、その描画が現在context / items世代 / 列数に属する証明を同じownerへ集約する。
証明済みの比率変更ではcanonicalとgeometryを同時に変換し、保存・revealの単位を一致させる。
geometryは「最終描画、またはその証明に基づく明示的な比率変換後」の記録とする。
viewportは保持し、content / maxは実際の行数と新行高から再計算する。未描画・兄弟・別世代・列数違い・
詳細表示では既存の変換制約を保つ。これはsingle-owner契約の実装不足を補うもので、
独立reviewerも新たな設計前提相違ではないと確認した。予測値による無条件の上書きはしない。

#### 論理一覧と入力セッション

同じ owner が `GridScrollSessionId` を保持する。これは入力継続の identity であって、path、items generation、履歴キーの代用品ではない。context 内の値移譲は identity を維持し、新しい fork は fresh identity とする。

| 境界 | セッションの扱い |
| --- | --- |
| 別の可視場所の採用、root / 子への移動、検索 / Snapshot 等への入退場、成功した A/B 切替 | 新規。A→B→A が描画間に完了しても再利用しない。同じ path の A/B 切替も所有 workspace が変われば新規 |
| ZIP / PDF pending の空 shell を実際に可視採用 | 旧一覧の継続を終了する。その shell の列挙・hydration 完了は同じ入場に属し、もう一度セッションを更新しない |
| 未採用の scan / staged open、取消・失敗・stale result | 現在の一覧・セッションを変更しない。既存仕様で既に shell を可視採用した後の rollback は、元位置を戻す新しい入場として扱う |
| resize / DPI / バーの出入り / Auto 比率 / 列数の変更、同じ一覧の filter / sort / 再走査・streaming 補充 | セッションは維持。現在 geometry を更新し、既存の行スナップ・先頭復帰・選択維持の位置規則だけを適用する。`bump_items_generation` 自体では更新しない |
| 詳細↔サムネイルの切替 | canonical は既存の表示形式変更規則で変換する。旧描画形式の入力継続だけ終了する（native details と App touch の相互流入を防ぐ） |
| 一時的な bundle mount / deposit、兄弟の read-only open / close | その事実だけでは更新しない。表示入力を停止する実際の park / host の交代では当該 owner の入力セッションだけ終了し、復帰時は位置を保持して新規入力を受ける |

発行境界は一覧の可視採用側へ集約する。`install_new_items_inner` や `bump_items_generation` への無条件追加はしない。`TopLevelGridView` / navigation の既存 typed な可視場所・採用 cause を使い、path を描画時に比較して入場を推測する第二 map を作らない。セッション ID の新規発行は識別子の発行だけで、repaint・再走査・cache 失効を伴わない。

### 3. egui 側の最小契約

提案 API は `ScrollArea::external_vertical_scroll(ExternalVerticalScroll)`。縦のみ・`auto_shrink([false, false])` の、全体高さが事前に既知の仮想一覧用に限定する。現行の `vertical_scroll_offset` をこのモードでは併用しない。旧 builder の意味や他の ScrollArea の既定動作を変えない。

frame-local な値を次の一組として渡す:

```text
ExternalVerticalScroll {
    content_height,       // 実際に確保する全高。content margin 込み
    interaction_session,  // context / 描画surface / GridScrollSessionId から作る ID
    interaction_id,       // 実 viewport の Grid 縦一覧に安定した transport ID
    position: Sync(offset) | Set(offset) | AppGesture(display_offset)
}
```

`Sync` は通常の canonical 同期であり、同じ入力セッションの native 速度・drag を保存する。`Set` は復元・edge・実際に位置が動く reveal / App の明示位置操作に加え、**App が現在 geometry で位置を clamp / snap 補正した結果**にも使い、旧速度と旧 animation target を終了する。`AppGesture` は既存 touch recognizer / snap glide が位置を所有する描画で、端数込みの表示位置をそのまま渡し、native Y の速度・target を終了して drag / coast と二重適用しない。touch owner が所有する合成 pointer の bar 操作も重ねず、既存の touch / mouse 入力分類・readback 抑止を維持する。App の touch の判定を egui が推測しない。新しい永続 pending は不要である。

`ScrollArea::begin` の手順を、このモードに限って以下へ揃える:

1. 既存の outer size、bar 占有、min size から **現在の `inner_size`** を決める。
2. 渡された `content_height - inner_size.y` で現在 Y bounds と `content_is_too_large.y` を確定する。前回の `max_offset.y` を physics の入力に使わない。セッション変更時は、この State の Y 速度・Y target・bar grab anchor・momentum bounce の継続を終了する。background の interaction rect は現在の inner rect を使い、新セッションで旧 rect を消したため新規 press の Response までなくなる状態を作らない。offset や履歴を既定値へ戻す reset ではない。
3. `Sync` / `Set` / `AppGesture` の位置を現在 bounds で適用する。`Set` はセッションが同じでも Y 速度・Y target を 0 / None にする。App の先行 clamp があれば、有効値でも既に `Set` として届く。さらに API に範囲外値が来た場合も現在範囲へ clamp して旧 coast / target を終えるが、この検査だけを App の clamp 検出の代わりにしない。
4. **その確定した位置から**現在 bounds の `AxisPhysics` で通常の drag / step を行い、その結果を使って content UI の origin / viewport を作る。現行のように physics 前の位置で viewport を作ったまま、返却状態だけ変更する順序にはしない。`AppGesture` では前述の native Y 処理を重ねない。
5. `Prepared::end` の新しい wheel・scroll target・scrollbar の処理も同じ現在 bounds を使い、実測 content と現在の viewport で State を保存する。新 API の既知高さと実測高さの一致は回帰テストと debug assertion で検査する。二回描画・discard 強制・次 frame への補正で吸収しない。

`Set` の優先権は **過去から残った physics / animation に対する優先権**である。同じ一覧で現に掴んでいる scrollbar や現在 frame の新規 drag / wheel は既存順序どおり処理できる。`Set` のたびにその pointer capture を奪ったり、bar grab anchor を消したりしない。別一覧での旧 capture の拒否は、下記の press 所属によって行う。

State の保存 ID は、App 側で `(ViewerContextId, 描画形式, 縦一覧)` を明示し、同じ context のフォルダ移動・再走査では安定させる（path / items generation を含めない）。これにより共有 egui Context 上でも兄弟の geometry / velocity State を上書きしない。`State` の関連付けは `Native | ExternalVertical(session)` の型とし、新セッションと既存セッションを区別する（runtime identity は serde 保存しない）。**interaction ID をセッションで変える初版案は撤回する。** geometry State と、実表示面の pointer capture の所属は次のように寿命を分ける。

#### 切替 frame の press と capture（P2-1）

egui は `context.rs:533` 付近の begin-pass で前 pass の widget を hit-test し、`interaction.rs::interact` が capture を決める。App の描画後に ID を替えてもこの判定はやり直されない。そこで `interaction_id` は **実 viewport の同じ Grid 縦一覧**に安定させ、background / 縦 bar の子 ID はここから派生させる。Folder・A/B・viewer context・詳細↔サムネイルの交代で変えない。geometry State の ID からは派生させず、別 viewport / 独立した panel とは共有しない。

vendor の opt-in 経路内に、この transport ID に属する一つの typed な `ExternalScrollCapture` を置く。Context の temp data に安定 ID で保持し、`Idle | Sequence { press_stamp, session, role, button, phase }`（role は Background / VerticalBar、phase は Held / Released）とする。egui 本来の capture を複製して配送する仕組みではなく、実際のcaptureにどの論理一覧の操作権限を結び付けるかの唯一の所属記録である。geometry State 側に同じ capture 所属を二重保存しない。

1. egui の順序付き `PointerEvent` で押下区間を区切り、opt-in経路から **現在viewportの `ctx.memory(|m| m.interaction() …)` を読み取る**。`potential_drag_id` がbackground IDならbackground、縦bar IDが `potential_click_id` / `potential_drag_id` にあればbarの実保持先として扱う。これはcrate内の既存参照であり、setter・移送API・再hit-testは不要。`Response` は集約結果で、押下ごとの配送記録として使わない。実際に `context.rs:1908` 付近は途中のreleaseでdown / draggedを落とし、その後のpressでは戻さない。`interaction.rs:240` 付近も前後のdragged IDが同じならstarted / stoppedを出さないため、これらのResponse flagを新press受理の必須条件にしない。
2. イベント列とbutton-downの前後から、旧Sequenceを終了するreleaseと、**その後に別の連続押下を開始する新規press**を分ける。列の末尾でまだheldの新押下について、上記の実保持先が当該安定IDであると確認できた場合、その描画時点の `interaction_session` に新しいSequenceを結び付ける。末尾の保持先はこの最後の未終了区間の証拠であり、それ以前の終了済み区間の配送先の推定に流用しない。同じframeの短いpress→releaseは既存clickの証拠・規則を保ち、末尾に保持先があるかのように偽装しない。単なるrect内の座標、`drag_started`、別buttonの追加押下では旧heldを再所属させない。OS pollingや時間閾値を使わない。
3. `press_stamp` は **当該viewportの `ctx.cumulative_frame_nr()` と入力列のpress ordinal**とする。同じ入力をreplayするdiscard / 後続passでは同じstampに同じ所属を保つ。sessionが途中で替わっても同じpressを宛先へ再所属させない。releaseを受けたstampも当該replay区間では保持し、同じpress/releaseの再消費を防ぐ。
4. 後続move、barの絶対位置更新、release velocityは、Sequenceのsessionが描画中sessionと一致する場合だけ既存計算を行う。開始時点で既にheldだった旧所属、または所属記録のないheldは宛先で使わない。**release→新pressでは旧Sequenceのreleaseを先に終端し、旧releaseの速度を新Sequenceへ渡さない。** 新Sequenceは新press位置からdragの基準 / 現在geometryのbar grab anchorを初期化する。Responseのdown / dragged / `interact_pointer_pos`がこのframeで欠けても、順序付き新pressと実保持先に基づく受理を使い、正常な後続moveへ接続する。新pressより前のframe内deltaを新dragへ加えない。所属が一致する旧Sequenceの通常release / click処理は維持し、Context全体のcaptureはresetしない。
5. **宛先採用後の最初の描画と同じ frame に来た新規 press**は、前 pass と同じ transport ID に届くため、そこで宛先の Sequence を作れる。後続 frame の move / release も同じ所属で扱い、追加の押し直しは不要。旧一覧でdrag中でも、切替frameの「旧release→同じbackgroundへの新press」を別Sequenceとして受理する。反対に、その frame の入力を旧一覧で既に消費してから描画後に宛先を採用した場合は、同じイベントを宛先で再実行しない。

現在の rect / clip / enabled と egui の配送先の両方を尊重し、overlay・cell・他 bar に届いた press を raw 座標だけで横取りしない。別 geometry で初めて現れた widget の hit-test 規則を一般変更するものではない。受入 R1 は切替の前から存在する background / bar の実 ID で配送を確認し、両 renderer・context 交代でも検査する。

この方式なら `context.rs` / `interaction.rs` / `InputState` に capture 移送・再 hit-test・合成 press・新しい公開 API を追加せず、**`scroll_area.rs` の opt-in 適用境界とローカル所属記録だけ**で済む。同じ一覧の正当な drag は所属が一致し、通常の経路のまま継続する。単なる ID 交換、全入力の release 待ち、global pointer reset は採らない。

`AxisPhysics` は現在 bounds を受ける既存の純粋計算として維持し、`scroll_physics.rs` は変更しない。今回のvendor回帰は `scroll_area.rs` のopt-in契約を通して検査する。通常の rubber-band を全体で無効化したり、係数を変更したりしない。

#### App と同じ geometry を渡す方法

詳細は外側横 ScrollArea の内側でヘッダ・行間を引いた **body** を用い、横バー出現による高さ減少も反映してから高さを計算する。サムネイルは現在の列数・cell height と body を使う。両方で既存 `snapped_scroll_extent` の一回の結果を、content の確保高と新 API に渡す。古い `last_viewport_h` から算出しない。

縦のみでは縦バーの幅は Y extent に影響せず、`auto_shrink=false` なので body 高が安定する。ただし egui の既定 min height は 64 pt である。共通 helper で **同じ明示 min height（現行どおり 64 pt）**を builder と App の viewport 計算に使い、64 pt 未満の body でも計算を一致させる。content margin は現行の 0 を明示し、宣言高と実測高を一致させる。幅丸めで行数が変わるサムネイルでは、描画に使った cell / rows の結果をそのまま宣言高に使う。異なる場所で extent を再推定しない。

### 4. CLAUDE.md の行スナップ規則との関係

「オフセットは App が自前管理し、常に `cell_size` の整数倍」は維持する。詳細では row height 28 pt、サムネイルでは現在の `cell_h` がこの単位に当たる。

```text
M = natural_h <= viewport_h ? 0 : ceil((natural_h - viewport_h) / row_h) * row_h
確保高 = natural_h <= viewport_h ? natural_h : M + viewport_h
canonical = 行境界上の 0..M
touch の描画位置 = canonical + 既存の一時 remainder
```

位置要求は同 frame の行高で snap / clamp し、**その補正の意味も一緒に**渡す。例えば旧 canonical 840、縮小後 max 560 なら解決結果は `Set(560)` であって `Sync(560)` ではない。egui が受け取る数値が有効でも、旧 velocity / target を終えてから描画 origin / viewport を決める。`Set` はこの解決 frame の結果であり、以後の readback を毎 frame Set にするものではない。egui は履歴・canonical の所有者にはならず、native 入力の候補 offset を返す。App は既存の details 半行 / thumbnail `should_sync_grid_scrollbar` の読戻し閾値と丸めを保持し、丸め後も現在の snapped max に収める。touch 端数は canonical や履歴へ保存せず、現在の方向 settle・130 ms snap 補間・prefetch / keep 範囲の扱いを保つ。

既存 `GridTouchScrollState` の anchor / row / items generation 検査は、不適合な端数を再利用しない既存契約として維持する。一時状態のキーは `(viewport, ViewerContextId)`、その既存 state の適合検査に同じ scroll owner の session を含める。同一 path・同じ offset の所有者交代でも端数・glide を引き継がせず、session ごとの第二 map を作らない。ROOT の keep 範囲を読む既存 helper は main の context / session を指定し、一時投影した detached を ROOT の owner と扱わない。これは一時描画状態の scope の修正であり、別の pending や寿命 owner を追加するものではない。逆に touch の generation 検査から native ScrollArea のセッション更新を導出しない。詳細の native drag / coast が持つ一時的な連続値も、今と同じ読戻し規則を通す。仕様を「行境界が不要」に緩めたり、読戻しを禁止したりしない。

#### MainGrid の recognizer とコマンド受理（P2-2）

端数の失効だけでは不十分である。`touch_correlation.rs::state_id`（:597付近）は現在 `(viewport, TouchSurface)`、その `TouchCorrelationState.recognizer` が旧接点を保持し、`touch_input.rs::handle_move`（:408付近）はそのまま `ScrollGrid` を出す。**MainGrid の driver に、同じ `GridScrollSessionId` と ViewerContextId を明示して渡す。** StillFullscreen の操作仕様・recognizer はこの変更の対象にしない。

- `TouchCorrelationState` 内の MainGrid recognizer を `MainGrid { session, recognizer }` として既存 owner にまとめる。session 不一致なら、コマンドを作る前に旧 recognizer の contacts・pinch・scroll owner を終了し、現在 session の fresh recognizer に置換する。旧接点を新 recognizer の Start として再生しない。追加の「復元待ち」「全接点 release 待ち」flag や stale 接点集合は作らない。
- 一方、`pointer_touch` と `PendingSignature` は egui-winit の synthetic pointer の相関を追う **transport の状態**なので一括 reset しない。旧 pending のコマンド権限だけを終了し、所属 session を付けた `Retired` の相関として exact な後続 signature を追う。これにより旧 touch の End に対応する primary release が、無関係なマウス click に化けない。`CorrelatedPrimary` の分類は `Current(session) | Retired` とし、Retired は tap / click / native drag を許可しない。相関が成立しない本物のマウス入力まで抑止しない。
- 新 recognizer が Start を受け取っていない TouchId の Move / End / Cancel は、transport 相関だけを進め、recognizer へ渡さない。特に現 `handle_sample` の Cancel は全接点を消すため、**旧接点の Cancel を現在の新接点へ作用させない**。`TouchFrame.touch_cancelled` も現在 session の Cancel だけを意味し、旧 Cancel で現在の fraction / pinch を終了しない。旧 End から `ScrollGridEnd` / settle を発行しない。既存 contact 検索を受理境界へ使い、別の旧接点 map は持たない。
- 現在 session の新しい Start は、旧接点がまだ離れていなくてもその場で受理する。新しい接点だけで GridScroll / Pinch を認識し、旧接点との二本指にはしない。synthetic pointer の gate が旧接点で占有されている間も、有効な新 raw touch の `ScrollGrid` 等は既存の raw command 経路で受理する。タップ等の pointer 経路は既存の exact-correlation 規則に従い、新しい合成 pointer を発行しない。
- `TouchFrame` / pending command の所属にも session を持たせ、`ui_main.rs` の `into_commands` 適用境界で現在 owner と照合する。driver 後に一覧が替わっていれば旧 batch は宛先へ適用しない。これは別の寿命判定ではなく、同じ session で生成元と消費先を結ぶ契約である。
- `DrivenSurface` と replay result の既存フレーム印にも MainGrid の所属を含める。**通常drive・transport-only・replayは、すべて同じviewportの `ctx.cumulative_frame_nr()` を共有する。** 現 `ui_main.rs:19512` 付近の `self.frame_counter` を渡す経路はこのviewport番号へ接続する。`App::input_frame_diagnostics`（`update_phases.rs:1748`付近）はApp更新呼出しごとにcounterを増やすため、同一egui frameのdiscard / 複数passの再生判定には使わない。App全体のcounterとその既存用途は変更せず、touchの共有駆動印を使うStillFullscreen側も同じviewport番号で照合する。別viewportの値は混ぜない。
- 同じ `ctx.cumulative_frame_nr()` の後続passでsessionが替わっても、既に駆動したイベントを新規Startとして再駆動しない。コマンドは再配送せず、Retiredの相関・抑止情報を保つ。描画前に採用した宛先へ初めてdriveする場合は、そのframeの新Startを宛先で受理する。次のviewport frameでは新規入力を通常受理する。

**同frameの複数所属release混在についての設計補足（設計担当判断 2026-10-30）**:
旧touch由来のセルclickは、そのframeのclick-bearing primary releaseがすべてRetiredの場合だけ抑止する。
同frameにMouseのclick-bearing releaseが一つでもあれば抑止せず、v4.10.0のpointer fallbackを保つ。
曖昧Currentの除去で所属を証明できないreleaseも、Retiredとはみなさない。判定は既存のframe-localな
`PrimaryOwnership`とrelease ordinalで行い、press区間の推測、追加bool／pending／guardは使わない。
許容範囲はMouseの混在だけではない。抑止しないCurrent tapや所属不明のclick-bearing releaseが同frameに
混ざる場合も、v4.10.0と同じpointer fallbackになる。純touchでCurrentとRetiredが同frameに共存する場合を
含む、**複数所属のreleaseが混在するframe**を§1.487の範囲とし、そのcell clickの完全な隔離は保証しない。
非clickのmouse releaseはこの混在判定へ数えず、clickがないStill dragのterminal抑止と、
既存Current gestureが要求する抑止は維持する。

これにより、mouse click→旧第二指の孤立releaseでもmouse clickを受理する。一方、旧touchのcaptureを
releaseした後にwidgetのない場所でmouseをpress→releaseする同frame列では、旧cell clickも適用される。
この特殊な混在は§1.487に含め、今回の>>661で隔離完了とは扱わない。混在しない旧touch releaseは引き続き拒否する。
eguiの`interaction.rs::interact`から実click発生元の序数をsnapshotへ記録する案は、egui中核への変更と
上流更新時の保守負担が>>661の目的に対して大きいため不採用とした。vendorの許可範囲は広げない。

入力のない A→B→A や同一 path の A/B でも session 自体が変わるため、この境界を通る。一時 mount / deposit で session は変えない。実 park では既存 lifecycle が MainGrid recognizer の実行権限を終了し、復帰で old contacts を再利用しない。MainGrid を描画しない区間に raw tail が配送される場合も、当該 viewport の frame 終端に、MainGrid の通常 drive がなかった場合だけ同じ transport tracker を観測継続する（コマンド生成はしない）。既存 frame 印を `Driven(owner) | TransportOnly` の一つの処理結果へ拡張し、**この終端観測も同じviewportの `ctx.cumulative_frame_nr()` で記録する**。観測・通常 drive・replay で同じイベントを二度相関させる別 queue / bool を作らない。transport-only の後に discard で表示先が変わっても、その frame の入力を新たな Start として再実行しない。StillFullscreen の command 駆動印を transport-only 観測で上書きせず、同 tracker の処理済み情報として記録する。StillFullscreen や兄弟 viewport の入力を消費・reset しないことも R2 の対照に含める。

### 5. 比較した別案と簡素化

| 案 | 利点 | 採否・欠点 |
| --- | --- | --- |
| **現在 geometry + 意味付き位置更新を ScrollArea の適用境界へ渡す（採用）** | private physics state を egui が所有したまま、初回描画から正しい bounds と優先権を使える。式・通常 UI・default API の変更を避けられる | 小さい opt-in API、安定 transport ID に対する capture 所属、App 2 renderer の接続が必要。既知高さと実測高さの一致を caller が保証する |
| App が `State::load/store` で offset / max / vel をまとめて変更 | builder の変更を少なく見せられ、現在の注入処理に置ける | 現 public API には max / vel / target / grab の setter がない。追加すると App が egui の内部 field と widget ID の作り方を知る。min height / bar / margin 計算も二重化する。velocity だけ消しても旧上限のばねが再生成し、max だけ直しても旧 velocity / capture が残る。全部を正しく encapsulate した API は採用案と同じ責務になり、App 直接編集の利点がない |
| すべての ScrollArea の physics を content 測定後へ移す | 未知の content 高にも一般的に対応できる | `show_viewport` の可視行・入力 hit-test を決めるため、offset は測定前に必要。二度の layout や全 UI の physics 順序変更となり、最小 vendor 差分を超える。今回の全高既知の一覧には不要 |
| `vertical_scroll_offset(Some)` は常に速度を消す / step しない | vendor の差分が短い | mIV は毎フレーム注入するため同じ一覧の native coast を消す。bar / drag と明示更新を識別できず、出荷済み機能を制限するため不採用 |
| path で ScrollArea ID を分ける / generation ごとに State を初期化 | フォルダ往復の一部で再現しなくなる | 同 path 再入場に旧慣性が残る。resize / filter の旧 geometry は解決せず、再走査で正当な drag も切る。保存位置と入力寿命を混同するため不採用 |
| interaction ID を session で交換 / 新 press を別 ID へ移送 | 一見簡単に旧 capture を分離できる | 初版の交換案は事前 hit-test 済みの新 press も失うため撤回。移送案は potential click / drag・snapshot・複数 pass の整合まで変更する必要がある。安定 ID と局所的な所属により、egui 全体の capture 再配送を不要にする |
| hydration 完了を最初の描画後へ遅延 / 復元を毎 frame 再注入 | 観測順序を合わせやすい | 完了順序の組み合わせを温存し、新 pending や入力との競合を増やす。禁止された症状隠しなので不採用 |
| フォルダ移動・詳細取得をモーダル化 / 一覧を閉じて開き直す | 非同期の組み合わせを減らせる | 普通の閲覧を待たせ、選択・scrollbar 操作を中断する。現在 geometry と位置更新を一つの同期境界へ揃えれば順序差そのものが問題にならず、閲覧の制限は不要 |
| egui interactionへ実click発生元の序数を追加 | 同frame混在時もnativeの実captureとreleaseを厳密に対応できる | 不採用。中核interactionへのvendor差分と保守負担が今回の目的に対して大きい。同frameのtouch／mouse混在を§1.487へ集約し、Mouse click-bearing releaseを含む場合は既存pointer fallbackを維持することで状態の組み合わせを減らす |

簡素化として採るのは、**geometry と位置の確定点を一つにし、request の owner を一つにすること**である。「復元済み×旧 bounds×ばね適用済み」を後から修復する状態を作らない。rare I/O failure の回復・retry 設計は対象外で、既定の割り切り方針に従う。

### 6. 同型経路と実装時の監査範囲

| 入口・一覧 | 適用方針 |
| --- | --- |
| ツリー / address / favorite / D&D / DFS からの通常 Folder 採用 | `start_loading_items`、物理 listing install、ready 採用の共通境界で session / request を確定。source ごとの reset を追加しない |
| Back / Forward / BS、ZIP 内階層、PDF / EPUB・変換書庫の採用 | 保存値の owner と論理 source key は変更せず、可視採用・復元から同じ scroll owner を使う。未採用・Cancel・late result は新 session / 位置を発行しない |
| 起動復元 / refresh / resume / フルスクリーンから Grid 復帰 / 選択行の可視化 | `RowAnchor(rows_above)` / `RevealSelection` で同じ適用境界へ合流。捕捉済み相対行位置と「最小限見せる」を用途ごとに保つ。起動要求の fork policy は別に扱う |
| 検索（Ctrl+S/G/T、Everything、Ctrl+F の再適用）、タグ・お気に入り、Rating、Collection / Smart の root と子、閲覧履歴、サブ展開、ブックマーク、Snapshot | 共通 `items` を表示する限り両 renderer と同じ契約。各 session / return snapshot の canonical 保存先は増やさない。root と子を同じ synthetic path だけで区別しない。検索・filter の既存先頭復帰仕様も変更しない |
| filter / sort / F5 / 自動観測の再走査、削除、同一覧 streaming、遅延列完了による下部バー消失 | 現在の extent を毎描画更新。同一論理一覧の既存位置規則を保つ。再生成の generation を理由に native 入力を一律終了しない |
| Home / End（選択移動）、リング / mouse gesture / gamepad の `GridScrollTop/Bottom`、gamepad 連続 scroll | 前者は cursor owner → reveal、後者は選択を変えない edge。既存 KeyAction と入力 router を保ち、scroll 要求だけ一元化 |
| A/B、main / active detached の bundle 投影・fork / park / promote | offset・request・session を同じ bundle で move / swap。fork の位置継承 policy は既存どおり、入力 session は新規。実 park の当該入力だけ終端。兄弟の canonical / generation / velocity / jobs は変更しない |
| MainGrid の raw touch / synthetic primary / recognizer / command batch / fraction | 同じ session の生成・受理を一貫させる。recognizer の旧接点終了と transport 相関の保持を分ける。unknown Move / End / Cancel、replay、未描画区間の tail まで§4の境界を適用 |
| 詳細の外側横 ScrollArea、F12 の独立した本ページ一覧・pan / 連結読み、音楽 timeline、side panel / popup | canonical 縦一覧とは別 owner。新 API を勝手に適用せず既存契約を保持。opt-in なしの動作を vendor 対照テストで確認する |
| 製本 / Collection 並べ替え（`ui_main.rs:9774`、`ui_dialogs/collections.rs:4854`）、整理・設定・metadata の明示 offset | source 検索で別の `vertical_scroll_offset` caller がある。各ダイアログ固有位置であり main の folder restore へ接続しない。今回 default semantics を変えないため動作維持を確認し、独自の動的 extent / 明示移動を同じ controlled 契約へ移す作業は別 chunk とする。本修正で全 egui caller の未知高問題を解決したとは報告しない |

実装時の一覧採用監査は `install_loading_items_inner`、`install_new_items_inner` とその caller、`install_prepared_*`、`TopLevelGridView::replace_surface`、context の fork / swap / park を起点に行う。各 caller を「新入場 / 同一覧更新 / 所有移譲」に分類し、全体 install を全部新入場と扱わない。cancel / failure で旧 owner を保つか、既に可視 shell に移ったかも区別する。UI thread に filesystem 判定や新しい非同期準備は追加しない。

### 7. F12・複数ウィンドウと detached 憲法

**bundle のスクロール状態の移譲には触れる。HWND / viewport の生成・閉鎖・配置や detached predicate は変更しない計画。** `ViewerContextBundle` に旧三 field のまま追加情報を散らさず、単一 scroll owner を登録し、既存の swap / fork / park の所有境界で扱う。

これは `detached-rework-plan.md` §2 の BA-7（分散状態）と同じ所有原則に従う構造修正である。新しい detached bool、geometry による host 判定、recreate、時間窓、repaint、兄弟への reset は不要。park による入力停止は既存 lifecycle owner に集約し、描画中の `is_detached` 分岐で補修しない。単なる一時 mount / read-only open を park と読み替えない。

ClaudeCode の設計コミットと独立 review / 再確認により、**症状パッチではなく構造的修正である方向への同意は確認済み**。初版の「未取得」という記述は更新する。ただし残P2・P3を反映した第3版全体の再 ACCEPT とは区別する。実装着手前に、確認済みの判断と実際に触る bundle / park / touch 入力寿命の範囲を同 plan §11 に記録する。今回は依頼どおり本節だけを改訂し、§11 は変更しない。viewport / predicate や、MainGrid 以外の gesture の仕様変更が必要になれば設計担当へ戻す。

### 8. 出荷済み挙動への影響

| 変わる点 | 利用者への意味 |
| --- | --- |
| 長い一覧の復元を、直前の短い一覧の bounds で押し戻さなくなる | 選択・保存した位置を初回描画から保持する。バックアップ ON/OFF・素材・完了順序に依存しない |
| 別一覧への移動・再入場に旧 drag / bar grab / coast / raw touch gesture が流入しなくなる | 元一覧で始めた操作は元一覧で終了。宛先の初回描画 frame の新 press は安定 ID で受理し、後続 move / release も押し直しなし。新 raw touch Start は旧接点の release を待たず受理する |
| refresh / resume の行 anchor を共通要求へ移す | 起動以外でも選択行の画面内位置を維持する。出荷済みの行 anchor 機能を削らない |
| 明示移動と layout clamp が過去の native velocity / animation に勝つ | Home/End・edge・復元・App wheel の結果が残った速度で流れ続けない。同じ一覧で保持中の正当な pointer 操作は維持する |
| viewport / content 確定と可視行の算出が一致する | resize や下部バー出入りでも旧高に対する不要なばねを作らない。64 pt min・margin・横バー・行末余白の見た目は現行寸法を維持する |

新しい待機・手順・機能削除は予定しない。通常閲覧の速度、wheel 行数、bar 操作、details drag / coast、touch の方向 settle / snap glide、選択・チェック・保存位置・履歴・A/B の意味は維持する。**通常の使い方を不便にする変更は本案では予定していない。** 同一一覧の慣性廃止、drag の release まで他操作を拒否、復元機能や入力経路の制限が必要だと判明した場合は実装を止め、出荷済み挙動と費用を示して ClaudeCode から利用者へ相談する。

DB / sidecar / archive / 設定の schema・保存値の意味は変更せず migration は不要。main の位置修正を理由に thumbnail cache、PDF epoch、worker / context generation を更新しない。layout の追加処理は定数時間の算術・identity 照合、入力所属は既存イベント列に対する処理で、再走査・decode・GPU upload を増やさない。

### 9. テスト計画と受入条件

実装前後を同じ入力・fixture・assert で比較する。すべて `MSBUILDDISABLENODEREUSE=1`、一時 data_dir / 有効画像入り ZIP・JPG、非対話 headless を使う。実キー polling の focus 隔離、Full-frame driver / 実 worker、output の明示破棄という前回条件を維持する。今回は設計のみなので test / build は実行しない。

1. **既存 `scroll_661::` 9 件をすべて成功へ**。自然順 200→100→200 の両表示、ON / 逆向き / JPG、fast hydration、fresh surface / spring-off 対照を残す。egui 最小再現には採用 API で現在高と明示更新を渡すよう caller を適応するが、旧上限・新上限・要求位置・期待値・ばね有効条件は維持する。API 導入前の red 証跡を保存し、旧 API を無断で一般修正した結果とは扱わない。
2. fast hydration をサムネイルにも追加し、最初の宛先描画の前 / 後に実復元が終わる両順序を制御する。件数200/100の比較だけでなく、短い / 空 / 大きい同一 source の範囲変化、最終項目以外の保存位置も検査する。自然順の偶然の成功だけでは受入不可。
3. egui の固定時刻・実 `Context` 試験: 既知高の増減、viewport の増減、初回描画、非ゼロ旧 velocity / 旧 target を伴う Set、新セッションへ held background / bar pointer、A→B→A、同じ session の drag / bar の継続と release coast、同じ frame の新規入力。位置だけでなく描画 viewport / content origin・State bounds・velocity・bar の到達点を検査する。`Sync` が毎 frame coast を消す変異、bounds 更新だけ / 速度終端だけを戻す変異で回帰を検出できることを確認する。
4. App の request / geometry 単体試験: ensure と edge の優先順位、用途共通の row anchor、復元と reveal の合成、描画後に積んだ要求の寿命、readback が明示更新を再発行しないこと。0件・1行・端数 viewport・64 pt 未満・倍率1/1.5/2・詳細横バーと進捗バーの出入り・サムネイル列数 / Auto 比率変更で行倍数・max・宣言高と実測高を検査する。
5. Full-frame 入力回帰: wheel / popup 消費、scrollbar の press→move→release、details drag / coast、Home/End の選択移動と ring / gamepad edge の選択不変。touch の fractional drag / 方向 settle / 130 ms 補間・pinch、keep 範囲 / prefetch の既存試験も再利用する。filter / sort / F5 / 詳細完了時の現在 extent と既存の位置方針を検査する。
6. 履歴 / BS / 検索復帰 / Rating・Collection / 閲覧履歴・サブ展開 / Snapshot / A/B の既存 position suite を通す。cancel・失敗・stale completion は可視 owner と位置を変えず、可視 shell 経由は新規入力寿命になることを検査する。keyboard / ring / gamepad は既存 handler まで通す。
7. 複数 context の非対話回帰: 片方に非ゼロ速度・位置・touch 端数を持たせ、もう片方の長短一覧移動・open / close・fork / park / resume / promote 後も兄弟の canonical・session・egui State・items generation / jobs が不変であることを確認する。同じ path の A/B、描画を挟まない往復、一時 mount / deposit も含める。実 HWND / GPU 提示の検証を headless 成功で代用しない。
8. vendor は新 API の opt-in 有無を分けて試験し、既存 native ScrollArea（nested / both axes / stick-to-end / scroll-to / wheel chaining）を退行させない。実 discard / multipass でも request が二重発火せず、最終描画と正本が一致することを確認する。App の UI snapshot は位置・最終行・バー周辺の既存 coverage を更新 / 補完するが、snapshot だけで physics を検証しない。

#### 指摘に対応する必須回帰（第2版の残P2・P3を含む）

**R1（P2-1と第2版の残P2、段Cの受入）**: 実 `Context` を前 frame から走らせ、background / bar の transport widget を登録する。宛先を採用する frame に実 `PointerButton::Pressed` を与え、その frame の egui capture が安定 ID に配送されたことを確認する。**切替＋press → 次 frame の move → release**を通し、宛先で drag / bar が動き、押し直しなしで完了することを両 renderer で検査する。さらに必須ケースとして、**旧background dragを成立させる → 宛先切替frameに旧release→同じbackground IDへの新press → 次frameのmove→release**を通す。切替frameのResponseでdown / dragged / started / stopped / clickの証拠が欠ける条件を確認しながら、実 `potential_drag_id` は当該background IDを保持し、新しいpress stampと宛先所属が記録されること、旧releaseから速度を引き継がず、新press以後の操作が押し直しなしで動くことを検査する。barの同様の再押下と、別widgetへ新pressが届く非横取り対照も追加する。旧一覧で既にpress → 切替＋move → releaseのまま新pressがない場合は、宛先の位置・release velocityが変わらないことを確認する。同contextのFolder、同path A/B、同viewportのcontext交代、詳細↔サムネイルで実施する。同一一覧のdrag継続、追加buttonで旧所属を再取得しないこと、discardで同じpressを宛先へ再所属させないことも含める。ID交換だけの変異、新pressの証明をResponseだけに戻す変異、所属検査を外す変異をそれぞれ検出する。

**R2（P2-2、段Cの受入）**: `TouchRecognizer` だけへの偽 command 注入ではなく、実 `Touch` と egui-winit と同じ synthetic pointer 列 → `drive_egui_touch_input` → MainGrid command 消費 → 実 layout を通す。旧 Start / Move で GridScroll（別ケースで Pinch）を成立させ、一覧切替後の旧 Move / End / Cancel が `ScrollGrid` / `ScrollGridEnd` / `Zoom` / tap を宛先へ出さず、canonical・端数・列数を変えないことを確認する。旧接点がまだ down の間に別 TouchId の新 Start / Move を入れ、新 session の scroll が実際に動くこと、旧 Cancel がこの新接点を消さないことも検査する。同一 path の A/B、描画を挟まない A→B→A、driver 後に採用される旧 batch、切替 frame の新 Start、後続 pass の replay を含める。未描画 / park 区間で旧 tail を観測してから復帰し、新 Start が相関できること、旧 synthetic release が mouse click / native coast に化けないことを確認する。StillFullscreen・別 viewport・実 mouse の既存試験を対照とし、fraction の session 検査だけを残して recognizer 境界を戻す変異を検出する。

R2のnative入力隔離には§1.487の混在例外を適用する（2026-10-30設計補足）。同frameのclick-bearing
primary releaseがすべてRetiredなら旧cell応答を拒否し、Mouseを含む場合は抑止しない。
両rendererのmouse click→旧第二指の孤立releaseの回帰は期待値を維持して通す。旧cell releaseのみの四件と
異座標Endの二件も必須とする。旧touch release→capture先のないmouse clickという反例は、
抑止しない許容挙動を実Contextで検査する。既存の旧第二指Move混入も§1.487の別枠として保持する。

R2には第2版のP3に対する **実Appのdiscard回帰**も必須とする。固定frame番号を渡す局所試験だけで済ませず、Full-frame headlessの実App update / renderから `ctx.request_discard` を通す。同一viewportの複数passでAppの `frame_counter` が進んでも `ctx.cumulative_frame_nr()` は同じであること、通常drive / transport-only / replayが同じ番号を読み、同じraw入力からのcommand生成・適用が一度だけであることを検査する。pass間にsession / 表示先を変えたケースでも旧入力を新規Startとして再認識せず、相関・Retired抑止を維持する。次のviewport frameの新しい入力は受理する。別viewportの独立したframe番号・処理を混同しない対照も含める。touchの番号だけを `self.frame_counter` に戻す変異で、実Appの二重駆動または誤った再受理を検出する。

**R3（P2-3、段Bで受入し段Cでも再利用）**: 起動復元に加え、通常 reload / F5、自動観測 rescan、`ResumeList` / `RefreshList` の採用と hydration を実際に通す。最下行以外を選択して `rows_above > 0` を捕捉し、再採用後に同じ安定キーの選択項目が同じ画面内行位置へ来ることを両 renderer で検査する。選択項目より前に行が増減するケースも含め、旧 index / 絶対 offset だけの比較にしない。snapshot / context の move・fork、および操作由来の選択が復元に優先する既存規則も対照にする。refresh の `RowAnchor` を通常 reveal に置換する変異を検出できることを確認する。

**R4（P2-4、段Bの受入）**: App の実 geometry resolver に canonical 840 と縮小後 max 560 を与え、**先に App が clamp して `Set(560)` を作る**ところから、非ゼロ native velocity と旧 target を持った実 ScrollArea に接続する。速度だけ / target だけ / 両方の状態、縮小した content と拡大した viewport の両方で試す。新規入力なしで canonical / 返却 offset が560、velocity が0、旧 target が終了し、**最初の描画の content origin / viewport も560に一致**すること、次 frame に旧 target へ戻らないことを検査する。有効な `Sync(560)` に置き換える変異で失敗することを確認する。位置が変わらない `Sync` の正当な coast / target、および Set 後の新入力・同一一覧で保持中の bar は別対照として残す。

狭い試験から始め、関連 history / touch / context suite、fmt / core check、最後に共有 vendor を含む `scripts/test-full.ps1` の gate へ進む。vendor 差分は `vendor/egui/PATCHES.md` の末尾へ本件・opt-in 契約・変更ファイル・既定動作保持・回帰・証跡の所在を追記し、`Cargo.toml.orig` / provenance を原本として保持する。未実行・内部確認・独立受入を区別する。

実装完了後は自動 gate と通常の検証 build を用意するが、製品を agent が起動しない。実画面は本書の使い捨て portable / 専用 data の手順に、実 scrollbar drag・details drag・touch / DPI・F12 の context 往復を加え、ClaudeCode が具体的な範囲と時間を提示して別途承認を得る。Windows 10 / 11 の双方を候補とし、未確認側は未確認と記録する。

### 10. 実装の段、規模、引き継ぎ

#### レビュー単位と受入ゲート

同じブランチ上で以下の順に、コンパイル可能な一貫した塊として実装・レビューする。各段の完了は次段への技術的な checkpoint で、独立した出荷候補ではない。legacy field と新 owner を併存させる二重正本、session の片側だけ有効な gesture、機能を一時無効化する切替 flag は作らない。

| 段 | 実装範囲 | 受入条件・>>661 の状態 |
| --- | --- | --- |
| **A: 挙動不変の所有者集約** | canonical / reveal / edge / 汎用 RowAnchor を `GridScrollState` へ移し、全 producer と bundle の move / swap / fork policy を移管。renderer は既存の offset 注入・読戻しのまま。新 physics / capture / recognizer の振舞いはまだ有効にしない | 既存 request の優先順位、history / A/B / context・touch の既存挙動（R3を除く）が保持される。core check・focused suite・fmt成功。**>>661 の決定的再現は従来どおり red**、対照は維持。自然順の偶然の pass を修正済みと扱わず、期待する既知 red は別記録。テストを ignore / 弱体化して gate を偽らない |
| **B: egui 契約と2 rendererの接続（>>661 本体）** | opt-in API の現在 geometry と `Sync/Set/AppGesture` 適用境界を実装。2 renderer が同じ geometry 解決結果を注入し、clamp の Set を含めて readback まで接続。この段は既存 interaction ID / capture / raw touch の受理挙動を維持し、session による新しい入力終端はまだ導入しない。最終 API の session / capture 部分は段Cで一括接続する | **既存9件を caller 適応だけで全 green**、追加 fast hydration両順序、R4、現在geometry・同一覧drag / coast / bar / touch・R3・関連history試験成功。vendor既定APIの対照も成功。ここで >>661 の旧boundsによるばねを解消した証拠が揃うが、入力寿命の全要件を満たした最終版とは扱わない |
| **C: 入力寿命・contextの統合確認** | session発行を可視採用 / 実parkへ接続し、安定transport IDとcapture所属、MainGrid recognizer / command受理・fraction、contextの保存IDを同じ塊で導入する。touchのtransport観測 / replayも接続し、片側だけの失効を残さない | **R1/R2を含む全受入成功、R3/R4と>>661の全greenを保持**。同path A/B、無描画往復、fork / park / resume / promote・兄弟不変・入力router / snapshot / vendor対照を確認。最後に共有vendorを含む `test-full.ps1`、独立completion review、文書・証跡、検証buildを揃える |

段Aは既存挙動と既知不具合を保つ refactor、段Bはその不具合だけを直す coherent chunk、段Cは契約全体を完成させる chunk とする。**次版への統合・出荷は段Cの完了と設計担当の検収後だけ**。段Bを先に公開したり、段Cの難点を理由に「旧操作を無効化した中間版」を出したりしない。段の途中の依存変更を単独 cherry-pick しない。実装範囲がこの区切りで一貫しなくなったら設計担当へ戻し、段を再分割する。

#### 最小 vendor 差分と再見積もり

P2-1の方式は **安定 interaction ID＋局所capture所属**に確定する。vendorの製品変更は `vendor/egui/src/containers/scroll_area.rs` の opt-in builder / State関連付け / begin-end / private capture owner に限定し、同ファイル内の回帰と `PATCHES.md` の記録を伴う。第3版の実保持先参照も同opt-in経路から既存 `Memory::interaction()` のIDを読むだけである。**`context.rs` / `interaction.rs` は変更しない。** `InputState`・memory・egui-winit / eframeにもsetter・capture移送API・再hit-testを増設しない。`scroll_physics.rs` は変更せず、数式・係数も維持する。新capture ownerは固定数のtransport単位であり、イベントqueue・sessionごとの履歴mapを保持しない。

App側で増える範囲は `touch_correlation.rs` のMainGrid sessionと既存transport / replayの分離、`touch_input.rs` のcontact所属を確認する小さな入口、`ui_main.rs` の生成・消費境界と既存viewport入力観測への接続である。一般のtouch操作やnative pointer生成の変更は含めない。既存 `app.rs` / 共通scroll owner / `viewer_context_registry.rs` の採用・移譲と合わせて監査する。

**中規模、約5〜8人日相当へ再見積もりする**（初版4〜7日を置換）。長い build / 全体gateの待ちと承認後の実機待ちは別。安定ID方式でvendorの基盤変更は抑えられる一方、press所属・transport相関を含む実入力列の回帰と、refresh / resume producerの移管検証を追加した分を見込む。

第3版でも **5〜8人日の暫定見積もりを維持**する。追加は既存内部保持先を使う局所受理判定とR1の具体化、touch専用のframe番号接続とR2の実App discard確認であり、既に見込んだ段Cの実イベント回帰・レビュー対応の範囲内。新しいvendor基盤変更やApp全体のcounter改修は生じない。

| 工程 | 目安 |
| --- | --- |
| 段Aと既存producer / R3の検証 | 1〜1.5日 |
| 段B、既存redのgreen、R4 / geometry回帰 | 1〜2日 |
| 段C、capture / recognizer / context統合とR1/R2 | 2〜3日 |
| 各段の独立review指摘対応・最終証跡 / 文書 | 1〜1.5日 |

実装の主要な不確定要素は、受理方式そのものではなく既存callerの網羅と実イベント回帰fixtureの整備である。局所方式では足りずContextのcapture再配送や通常入力の制限が必要と判明した場合は、この見積もりのまま拡張せず設計担当へ戻す。

実装時の文書更新先は本書の実装・検証台帳、`coding-conventions.md`（App 正本と egui 入力候補の契約）、`details-view-and-filter-plan.md`、`touch-support-plan.md`、`folder-history-location-plan.md` / `top-level-grid-view.md`、前述の detached §11 と vendor 台帳。公開向けには内部型名を出さず、復元位置の不具合修正を通常の変更履歴へ記す。

今回の成果物はこの設計節の改訂のみ。製品コード・既存の失敗テスト・vendor は未変更、commit / build / test / 製品起動 / デスクトップ操作なし。第2版の残P2・P3とsourceの照合・文書の整合確認を行った。通常の使い方を不便にする仕様変更は追加していない。本改訂の再レビューと実装着手可の判定は、ClaudeCode と次の reviewer へ引き継ぐ。

## 実装・検証記録（2026-10-30）

### 着手条件と段Aの前提監査での停止

設計第3版 `446a435f1` は独立設計レビューで **ACCEPT（追加指摘なし）**。
構造的修正であることへの独立reviewer・ClaudeCode・Codexの同意を受けて実装調査を開始した。
上の設計本文の「ACCEPT未取得」は設計提出時の記録であり、この時点では解消している。
ただし依頼の「設計の前提がコードと食い違ったら、設計を黙って変えずに止める」に該当する
下記の相違を、段Aの製品編集前に確認したため停止した。設計本文は変更していない。

**相違: 既存の未消費RowAnchorとEdgeの同時保持を、設計の排他的requestが表せない。**

- `src/app/startup_list_restore.rs:544` / `:565` は `ResumeList` / `RefreshList` / 起動復元で
  `scroll_selected_to_rows_above` を設定してから `try_select_after_load` を呼ぶ。
- `src/app.rs::try_select_after_load`（:74406）は対象名が見つからないとfalseを返し、
  row hintを消さない。`apply_scroll_to_selected`（:61168）も選択なしならhintをtakeする前にreturnする。
- `render_grid`（`src/ui_main.rs:19467`）はrevealを一度取り出してfalseにするが、
  row hintは別fieldに残る。`request_grid_scroll`（`src/app/gamepad_input.rs:8694`）はEdgeだけを設定し、
  layout境界（`src/ui_main.rs:19745`付近）もEdgeだけをtakeする。
  したがって旧stateには、Edge適用後もrow hintを保持し、後のrevealで消費できる組み合わせがある。
- 既存 `grid_cursor_rating_grid_move_then_shift_click_uses_display_order_in_both_views`
  （`src/ui_main.rs:28197`）と
  `paired_identity_remap_keeps_checks_and_scroll_then_shift_click_uses_remapped_anchor`
  （:28377）は、Edgeとrow hintの同時保持を明示的にassertしている。
  前者はrevealも同時に要求し、後者はreveal=falseのままhintを保持する。

これは「実機で利用者に新しい症状を再現した」という記録ではない。
確認したのはproductionのproducer / consumerが許す状態と、その保持を明示する既存試験である。
設計の `Edge(Top|Bottom)` は補助payloadを持たず、ownerに並行hintも置かないため、
この組み合わせを失わず移管する仕様が不足している。
hintを捨てる、既存assertを削る、Edgeに設計外payloadを加える、のいずれも行っていない。

設計担当へ返す論点は、**単一ownerの中で、Edgeの即時適用と未消費の行位置指定をどう合成・保持するか**。
保持する型と消費規則を設計へ補い、段Aの挙動不変条件に照合してから再開する。
もし不要なhintとして破棄する仕様を選ぶなら、対象の出荷済み経路への影響と既存試験の変更根拠を
先に設計担当が判断する。実装側で機能制限やtest期待値の緩和を選ばない。

別途、起動前のhint設定（`startup_ops.rs:1990`付近）が通常loadのoffset / reveal / Edge resetを跨ぐ
既存経路も確認した。集約時は未採用cursorを既存 `StartupListIntent` で運ぶ候補があるが、
この案も今回まだ実装していない。

| 段 | 作業ツリーと検証結果 |
| --- | --- |
| A | 編集前のsource監査で停止。製品コード・テスト変更なし、受入テスト未実行 |
| B | 未着手。既知redをgreenにしたとは扱わない |
| C | 未着手。入力・contextの統合確認、全体gate、auditは未実施 |

この試行で更新したのは本記録と `detached-rework-plan.md` §11の構造合意・停止記録のみ。
Cargo / fmt / test / audit / 確認build / 製品起動 / desktop操作 / commitは実行していない。
未実装のためbacklogへ「実装済み」は登録せず、他の機能文書にも完成仕様として転記しない。
文書差分の `git diff --check` と、製品source / vendor / 既存testの差分なしを確認する。

### 設計担当補足後の再開

上記の停止後、ClaudeCodeが「一回限りの要求」と「未消費RowAnchor」を同一ownerの別枠とする
補足を確定した（設計§2に追記）。既存2試験のassert条件、Edgeだけではanchorを消さない規則、
起動前hintが通常loadのresetを跨ぐ規則を維持して段Aから再開する。
同じ描画で実revealとEdgeが両方ある場合は、旧来のreveal適用→Edge優先という順序も保持する。
実装のrequestはEdge variantのplacement payloadでこの合成を表し、別のreveal boolを追加しない。
ここから下は再開後の検証台帳とし、上の停止時点の未実施記録を成功結果へ読み替えない。

### 再開後の段A: 所有者集約と、新たなgeometry前提の相違

`src/app/grid_scroll.rs` にcanonical offset、型付き一回要求、独立RowAnchorを集約した。
App / ViewerContextBundle / SmartRootの退避・復帰と、producer / reader / readbackを移管した。
保存snapshotの形式は従来の数値のまま。起動hintはownerの独立RowAnchor枠としてload resetを跨ぎ、
`StartupListIntent` を変更する必要はなかった。既存2試験のassert条件を維持した。
egui・physics・capture・recognizer・一覧sessionの変更はまだ行っていない。

独立Sol / xhighによる段Aのコード・テスト設計レビューはACCEPT。
途中で見つかった「詳細ヘッダのsortがearly reveal claim後に積む要求の消失」は
`take_layout_request` と実sort handler回帰で修正した。
これは実行受入の完了とは別である。下記R3が失敗したため段Aの受入は未完了、B/Cへ進んでいない。

全Cargo実行は指定の **`MSBUILDDISABLENODEREUSE=1`** を設定。
通常feature、test / dev profile、一時data_dir、有効JPEG入りZIP / JPG、headlessの条件を維持した。

| コマンド・条件 | 結果 | 証跡 |
| --- | --- | --- |
| `cargo fmt`（引数なし） | 実行、整形済み | 後続diff check成功 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 初回coldのturbojpeg-sys並列native build失敗 | `target/scroll661-stage-a-check.log` |
| `cargo check -p turbojpeg-sys -j 1` | PASS、2m57s | `target/scroll661-native-j1.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | PASS、5m50s（native準備・lock待ちを含む） | `target/scroll661-stage-a-check-j1.log` |
| `cargo run --locked -p viewer_context_audit --target-dir target/scroll661-audit` | exit 0、違反0 | `target/scroll661-stage-a-audit.log` |
| `cargo test -p mimageviewer --lib app::grid_scroll::tests:: -j 1 -- --test-threads=1` | 初回lib test compileに移管上の3型エラー。保存用数値参照2箇所とfixtureの借用1箇所を修正後、**5 PASS**（compile 7m37s、実行0.00s） | `target/scroll661-stage-a-owner-tests.log` / `target/scroll661-stage-a-owner-tests-2.log` |
| `cargo test -p mimageviewer --lib scroll_661:: -- --test-threads=1` | **5 PASS / 7 FAIL**、13.50s。内訳は既知red4件＋新R3の3件 | `target/scroll661-stage-a-reproduction-and-r3.log` |
| 下記nextest filter | **458 PASS / 0 FAIL**、33.072s | `target/scroll661-stage-a-regression.log` |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/details_header_sort_reveal|grid_cursor_rating_grid_move|paired_identity_remap|startup_restore::|section1328_session|phase_c_folder_nav_history_tests::|snapshot_ops::|viewer_context_registry::|smart_folder::|touch_correlation::|touch_input::|grid_scroll_ring/) - test(scroll_661::)' --no-fail-fast --retries 0 --no-tests fail
```

458件には実mount / deposit、live-media / materialized-stillの両fork、SmartRoot退避・復帰の
新しいRowAnchor所有回帰3件と、既存2試験、late reveal回帰を含む。
既知red4件は `egui_explicit_offset_after_content_growth`、ZIPのOFF詳細 / OFFサムネイル、
fast hydration詳細で、段Aでは修正していないためredを保持した。

#### R3で観測した既存の詳細表示の行位置捕捉不整合（停止理由）

新しい `row_anchor_{reload,observation,resume}_keeps_middle_selected_row_in_both_renderers` は
3件とも **詳細表示の初期配置直後、refresh / resumeに入る前**のassertで失敗した。
選択index100、行高28、表示offset2744なので実際の行差は `100 - 2744/28 = 2`。
ところが `selected_rows_below_view_top` は0を返した。
各試験は詳細→サムネイルの順なので、失敗後のサムネイル側・再採用ループには到達していない。

source確認（行番号は段A差分を含む時点）:

- `src/app.rs:82776` の `selected_rows_below_view_top` は列数を常に `last_grid_cols` から取る。
- 初期値は `last_grid_cols=4`（`src/app.rs:20758`付近）。
  更新はサムネイル描画の `src/ui_main.rs:19730` だけで、詳細分岐はその前にreturnする。
- 詳細は `render_details_list` で行高28と `GridAspectLayout::Details` を記録し、revealはcols=1で適用するが、
  列数の最終描画記録を1へ更新しない。
- このため上記例の保存用計算は `floor(100/4) - 98` を飽和減算して0になる。
  `startup_list_restore.rs::current_main_list_cursor`（:348〜359付近）もこの値を使う。
  単なるtest oracleだけの問題ではない。

HEAD `446a435f1` のhelper / renderer / 初期値を照合し、同じ不整合が存在することを確認した。
段Aの当該helper変更はoffset accessorへの移管だけで、列数計算は変更していない。
独立reviewerもsourceと失敗値を再確認し、段Aの「挙動不変」とR3両renderer成功は
この既存不具合のまま両立しないと判断した。
修正前HEADの新R3テスト実行はしていないため、その実行結果を得たとは扱わない。

依頼の「別の相違が見つかったら停止」に従い、**製品のgeometry計算は変更せず停止**した。
fixtureに `last_grid_cols=1` を仕込む、期待値2を0へ変える、詳細側を省く、といった対応はしていない。
設計担当へ返す判断は、詳細の実列数1を既存の最終描画geometryへ記録する修正を対象に含め、
段Aに先行する小修正とするか、R3の受入段を見直すかである。
設定上の現在モードだけを読んで補うguardではなく、保存・復元が参照する実際の描画geometryを揃える方向を推奨する。

| 段 | 停止時の状態 |
| --- | --- |
| A | 所有者集約を実装し、core check・audit・owner5件・関連458件成功。新R3の3件が上記既存不整合で失敗し、受入未完了 |
| B | 未着手。vendor無変更、>>661の既知red4件を保持 |
| C | 未着手。session / capture / recognizer統合、新API、R1/R2/R4、最終feature check・full/focused gateは未実施 |

作業差分は未コミットで保持する。製品binary / desktop操作 / 通常profileへのアクセスは行っていない。
利用者指示により確認用buildも不要。未完成のためbacklogへ「実装済み」は追加しない。
テスト・ビルドログはPowerShellのUTF-16 BOM付き出力で、機械読取時はBOMを尊重する。

### 詳細geometryの修正承認と段B再開（2026-10-30）

ClaudeCodeが段Aを `a930891dd`（`wip(grid): 5ch 661 stage A`）としてコミットした。
直前の停止記録は当時の判断として保持する。設計担当は上記の出荷済み不整合を段Bの対象に含め、
R3の受入を段Bへ移すと決定した。段Aは既存458件とowner5件・core check・auditの成功をもって
挙動不変（R3を除く）のcheckpointを満たす。段B開始時の作業ツリーはclean。
最終描画geometryの共有、旧boundsの排除、clampのSet伝達を実装し、元の4再現とR3を検査する。
製品の起動・デスクトップ操作・確認用buildは依頼により行わない。

### 段B: 現在geometryと位置更新契約の接続（受入済み）

`GridScrollState` に最終geometryと描画proofを集約し、両rendererが同じresolverから
content高・max・`Sync / Set / AppGesture`を取得する。詳細では列数1、実body高を記録する。
Appでの先行clamp / snapもSetへ解決し、readbackは現在行高で丸めてmaxへ収め、再要求を出さない。
eguiの変更は `scroll_area.rs` のopt-inと同ファイル内のテストに限定し、現在boundsをphysics前、
その結果によるcontent origin / viewportをphysics後へ移した。既定APIとphysics式は維持する。

独立コードレビューのP2二件（比率変換直後の保存geometryの不整合、revealなしframeのO(N)選択探索）を
修正し、限定再確認でACCEPT・未解決指摘なしとなった。これは自動実行の受入とは区別する。
初回lib-test compileのheader fixture借用エラーはlocal変数へ取り出して解消した。

vendor単体はfocused 11件、全lib nextest **150件PASS**（run `d5f03cea-ba7d-47fa-83de-03c21601c043`）。
Appは元の4再現・R3に、fast hydrationサムネイル、中間位置と完了順序、
実resolverのclampから実native coast / targetと最初の描画位置へ接続するR4を追加して検証中。
R4はclampの意味をSyncへ置き換える対照も持つ。詳細は実行完了後に本台帳へ追記する。

段Bの途中証跡（最終受入とは区別）:

- `cargo fmt`（引数なし）実行済み。core checkはPASS（2m13s）、auditはexit 0・違反0。
- lib-testの初回二回はfixtureの借用とUIからのprivate registry accessor参照でcompile失敗。
  前者はlocal化、後者はApp内のgeometry記録入口へ集約して公開範囲を広げず修正した。
- `cargo test -p mimageviewer --lib scroll_661:: -- --test-threads=1` の第三回はcompile 8m11s、
  **11 PASS / 4 FAIL**（実行42.06s）。元の再現4件とR4はgreen。R3三件と追加中間位置fixtureに失敗が残った。
  ログは `target/scroll661-stage-b-reproduction{-2,-3}.log`（初回はsuffixなし）。
- 既にコンパイルした同じtest binaryでowner・aspect・late reveal・既存保持二件をfilterして
  **14 PASS**（3.22s）。`target/scroll661-stage-b-state-regression.log`。これらの関数はfixture修正で変更しない。
- R3は従来失敗していた初期詳細配置を通過した。その後のfixtureは、hydrationまで完了させるhelperの直後に
  activeを期待していたこと、App側completionをdrainしないrescan待機、物理slotと表示順の混同を確認した。
  画面内位置2行・安定選択・実worker／hydrationの条件を維持してfixtureの駆動境界を修正する。
- 追加中間位置の既定サムネイルはviewportが2.5行しかなく、上から2行のanchorでは選択行下端が見えず、
  復帰時の通常revealが合法に1行動かした。追加caseだけ10列／Auto OFFで三行以上を確保する。
  元の報告手順の既定設定と既存assertは変更していない。

fixture修正の独立レビューもACCEPT（期待値弱体化・製品不具合の隠蔽なし）。第四回の同コマンドは
compile 6m27s、実行62.82sで **15 PASS / 0 FAIL**。
元の再現4件、R3三件（両rendererの通常reload・観測rescan・ResumeList、前方追加／削除）、R4、
fast hydrationサムネイルと両順序／中間位置の対照がすべて通った。
ログは `target/scroll661-stage-b-reproduction-4.log`。関連nextest回帰を確認してから段Cへ進む。

関連nextestはrun `bd6db056-2bc8-43e8-8f17-86a4d2f02100`、**520 PASS / 0 FAIL**（50.870s）。
owner / 比率 / startup / history / Snapshot / Smart / viewer context / touchを含み、既に成功した
`scroll_661::` 15件は再実行せず除外した。ログは `target/scroll661-stage-b-regression.log`。

| 段 | checkpointの作業ツリー・受入 |
| --- | --- |
| A | `a930891dd`にコミット済み。挙動不変（R3を除く）として受入 |
| B | HEADはAのまま、Bの製品・vendor・test・文書差分は未コミットで保持。元4再現＋R3＋R4を含む15件、関連520件、owner等14件、vendor150件、fmt・core check・auditが成功。コード／fixture独立レビューACCEPT。ここで>>661本体と詳細の行位置捕捉不具合の自動修正証拠が揃った |
| C | B受入後に着手。入力寿命とcontextの統合が完了するまで出荷・単独cherry-pickしない |

全Cargo実行は `MSBUILDDISABLENODEREUSE=1`。製品binary・desktop・実profileを使用せず、
確認用buildとcommitも行っていない。段Cの最終gateは別途記録する。

### 段C: 入力寿命とcontextの統合（検証中）

一覧sessionをtypedな可視採用・成功したA/B交代・実parkへ接続し、forkはfresh、
move／swap／一時mountは維持する。ZIP／PDFのshellとその完了は一つの入場とし、
同一覧refresh／resume／resize／filter／sortのたびに寿命を更新しない。
検索は初回可視rowsとroot／drill採用、Snapshotは実rows差替え、Smartはowner復帰後に更新する。
ROOTの端数参照はread-only Main ContextRefへ集約した。

MainGridのrecognizerとcommand batchをscopeへ接続し、transportのexact correlationは保持した。
旧接点のMove／End／CancelはRetiredとして扱い、新接点を取消さない。
同座標の後続mouseを逆順のedge照合で区別し、MainGridの混在入力中断でもbackendのpointer gateを保持する。
このtouch境界は独立レビューでACCEPT。新規press、旧held、release→新pressを実両rendererへ通すR1と、
実App discard・接点切替・pinchのR2を追加している。

vendorの独立レビューで、Appが半行未満のreadbackを採らない場合のSync再passが
最初のnative移動を消すP2を確認・修正した。既存処理済frame印にsession所属を含め、
同ownerのSync再passではnative結果を保持する。Set／AppGesture／新sessionは位置更新を優先する。
同boundsのrubber-bandも既存physicsの判定をendへ伝え、描画と返却Stateを一致させた。
subrow drag／coast／target・明示位置・session交代・rubber／extent縮小の実Context回帰を追加し、
独立再確認はACCEPT。Sync再注入を戻す変異は期待どおり280と290の不一致で失敗し、sourceを完全復元した。
復元後のvendor全lib nextestは **161 PASS / 0 FAIL / 0 skipped**（1.735s）。
ログは `target/scroll661-stage-c-vendor-{replay-4,replay-mutation,final}.log`。

App側レビューでPinch中のnative Y二重所有も修正した。active PinchとZoom／PinchEnd発行frameを
AppGestureへ接続し、接点終了後の通常マウスを保持する。実背景／bar captureと実Appの切替回帰を追加した。
初期統合checkの失敗は、scopeを持つ一時状態をDefault不要のremoveへ変更し、test側のGuiRounding importを補った。
続いてtap閾値未満のUndecided入力でもcanonical＋fractionと異なるnative描画が生じることを、
実背景／barの3pt移動回帰で確認した（canonical 2808、viewport 2809.65）。現在scopeのtap候補を
StartからterminalまでApp所有Yへ含め、widgetのtapは維持した。終了後のidle／Retiredだけのframeは
所有せず、通常mouseを止めない。Pinchとtap候補の実renderer試験は修正後成功した。

### 段Cの設計前提の相違と停止（2026-10-30）

**段Cは未完了。追加の製品変更を停止し、失敗回帰をignoreせず残す。**
これは>>661の旧bounds修正とは別の、同じ入力寿命契約に含まれるnative pointer消費境界の不足である。
容量上限による中断から再開後、Sol / highのApp実装とSol / xhighの独立レビューを分離して進め、
独立reviewerも以下がbackendで表現可能な混在入力で、設計担当へ返す必要があると確認した。
Windows実機でのイベント配送・製品画面の観測を得たとは扱わない。

#### 具体的な経路と観測値

回帰 `retired_secondary_touch_move_cannot_drive_a_new_mouse_capture`
（`src/app/tests/scroll_661.rs:1469`付近）は一時data_dirと素材を使い、実フォルダ採用・hydration・
実renderer・egui Contextを通す。旧touchなしの対照を先に実行し、新mouseの継続移動量とも比較する。

1. 旧一覧で二指をStart。主接点1がsynthetic pointer、第二接点2はraw touchだけになる。
2. 別フォルダを実採用し、旧接点をRetiredへ移す。旧主接点1をEndし、backendの現在pointer位置で
   releaseとGoneを渡す。第二接点2はまだheldで、backendのpointer gateは空になる。
3. 新一覧の実背景で本物のmouseをpressし、安定background IDのcaptureが成立することを検査する。
4. 旧第二接点2のMoveと、backendが生成する対応PointerMovedを渡す。
   recognizerは宛先コマンドを生成せず、App touchのY所有もfalseだが、native背景captureがこのMoveを採る。

viewportは **756 → 800**、content originは **-580 → -624**。現在maxは前後とも756、
一覧scope・mouse captureは同じで、変更前のviewportとcanonicalも一致する。
現在extentをphysics前に確定する段Bの契約は成立しており、旧maxへの復元という元の原因ではない。
新mouseの押し直し不要・旧touchの宛先操作禁止の両方を、現在のAPIだけでは同時に満たせていない。

sourceのproducer／consumer（行番号はこの未コミット差分時点）:

- `vendor/egui-winit/src/lib.rs::on_touch:891`、gate条件`:915`、Endでの解放`:932`。
  主接点が終わりgateが空くと、まだheldの第二接点のMoveもsynthetic pointerへ出る。
  Endのrelease座標はraw touch座標ではなく現在pointer位置なので、fixtureもその順序と座標に合わせた。
- `src/touch_correlation.rs::TouchFrame::grid_owns_native_y:94` はRetiredだけのframeをApp所有にしない。
  新mouseの正当なdrag／coastを終わらせないため、この判断自体を一律反転できない。
- `vendor/egui/src/containers/scroll_area.rs::external_capture_input:891` の
  `PointerEvent::Moved`（`:1016`以降）は現在captureのsessionとbuttonを照合するが、
  eventごとの由来がないため旧touch由来Moveも新mouseのdeltaとprevious位置へ加える。
  `ExternalVerticalScroll:403` のgeometry／position／session／transport IDは、この由来を渡せない。

設計担当へ返す論点は、**順序付きeventのprovenanceをどの所有境界でnative pointerの受理へ接続するか**。
新mouseのcapture・速度・target・previous位置を保ったまま旧touch由来eventだけを隔離する必要がある。
AppGestureの一律適用、session／captureの一律reset、全接点release待ちは通常mouse操作を変えるため採らない。
App側のegui入力前routingを含めて境界を決める必要があり、特定vendorファイルの変更が必須とまでは
結論していない。承認済みAPIを黙って拡張せず、`context.rs`／`interaction.rs`／`scroll_physics.rs`／
egui-winitは無変更。まれな失敗の回復処理でもない。

#### 停止時の検証記録

全Cargo実行は `MSBUILDDISABLENODEREUSE=1`。以下のcheck／audit成功は段C全体の受入を意味しない。

| コマンド・対象 | 結果 | ログ |
| --- | --- | --- |
| `cargo fmt`（引数なし） | 実行済み | sourceへ適用 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | PASS、2m26s | `target/scroll661-stage-c-core-check.log` |
| `cargo check -p mimageviewer --lib --tests --features test-script` | 第四回PASS、2m29s。先行の一時状態remove／import／旧style APIのcompileエラーは修正済み | `target/scroll661-stage-c-feature-check-4.log` |
| `cargo run --locked -p viewer_context_audit` | exit 0、違反0 | `target/scroll661-stage-c-audit-final.log` |
| vendor eguiの全lib nextest | 161 PASS、1.735s。source復元後の最終実行 | `target/scroll661-stage-c-vendor-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(scroll_661::)' --no-fail-fast --retries 0 --no-tests fail` | 21 PASS / 1 FAIL、8.554s。唯一の失敗は上記混在入力回帰 | `target/scroll661-stage-c-checkpoint-4.log` |
| 下記の停止時関連nextest | 509 PASS / 1 FAIL、38.283s、exit 100。唯一の失敗は同じ混在入力回帰 | `target/scroll661-stage-c-stop-checkpoint.log` |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/details_header_sort_reveal|grid_cursor_rating_grid_move|paired_identity_remap|startup_restore::|section1328_session|phase_c_folder_nav_history_tests::|snapshot_ops::|viewer_context_registry::|smart_folder::|touch_correlation::|touch_input::|grid_scroll_ring|grid_scroll::|scroll_661::|grid_pinch_actual_renderer|grid_tap_candidate_actual_renderer/)' --no-fail-fast --retries 0 --no-tests fail
```

この21件に元4再現、R3、R4、R1の切替＋新press／旧background dragのrelease→新press、
実App discardと非描画往復のR2を含む。R1専用fixtureは遅延metadataの25pt status barで
press対象が移動しないよう任意詳細列をOFFにした。既定列・ZIP詳細取得を通す元報告fixture、
期待値、capture成立・viewport／originのassertは弱めていない。
先行checkpointのfixture失敗や未完了metadataをpollだけで待つ試行は成功証拠へ数えない。

core／feature check以降の変更はtest fixtureと文書に限る。
最終の `test-focused.ps1`（exit 2時の `test-lib-full.ps1`）、共有変更のfull gate、必要時のUI snapshotは
段Cの設計不足で完成条件を満たせないため未実施。製品起動・desktop操作・実profile使用・
確認用build・commitは行わず、未コミットのB/C差分を保持する。
`git diff --check` は成功し、vendorの変更ファイルは許可された `scroll_area.rs` と `PATCHES.md` だけである。
backlogへの「実装済み・実機未確認」追記も未完了のため行っていない。
再開時は設計補足を確認して当該redを通し、段C全体と最終gateを完了してから出荷可能とする。

<a id="mixed-input-v410-baseline"></a>

### 混在入力回帰のv4.10.0基点比較と範囲の確定（2026-10-30）

上記停止後、ClaudeCodeがB/Cの途中差分を `910391359`（WIP）としてコミットし、
Collection帰還の調査・20試験を `da7bd5aaa` で取り込んだ。再開時の作業ツリーはclean。
設計担当は「まず同じ回帰を基点で実行し、新規退行なら段Cで扱う、既存なら別backlogへ分離する」
と指示した。基点比較が終わるまで、追加の製品変更と他経路のテストは進めなかった。

基点 **v4.10.0 `b10dc74060b9109220ffeae6d2266ba6721b7b0d`** を
`target/scroll661-v410-baseline/` へ `git archive` で実ファイル展開した。
worktree／stash／junctionは使わず、未追跡vendorを実体コピーして基点のtracked内容を重ねた。
LFS filterのsandbox内エラーはarchiveコマンド限定のfilter無効化で回避し、永続Git設定は変更しなかった。
使用素材は同じ一時生成JPGで、LFS画像やモデルを動作条件にしない。
展開時のtracked4,248件をarchiveとbyte比較し、試験追加後も全tracked／vendorを再監査した。
基点製品ロジックは変更せず、変更は `tests.rs` のmodule登録、`ui_main.rs` の `cfg(test)` witness、
追加のtest moduleだけ。eguiの基点 `scroll_area.rs`／physicsはSHA-256一致を確認した。

同じ回帰を旧fieldへ適合し、実フォルダ移動・worker／hydration・Context／renderer・
正当なbackground captureを通した。存在しない新session APIはfolder／items世代／行数／選択／列数の
不変条件に置き換えた。イベント順序と旧Moveなしの対照、viewport／origin／返却Stateの検査は維持した。
基点はviewport／content originをphysics前に確定し、その位置で描画するので、同frameのStateと描画を別々に記録し、
直後の本物mouse Moveとその描画が出る次frameも採取した。assertは全記録後へ移して失敗を残した。

| 観測 | 旧Moveなしの対照 | 旧第二指Moveあり |
| --- | ---: | ---: |
| 直前のcanonical／native／viewport | 756／756／756 | 756／756／756 |
| 当該frameの返却native offset | 756 | **800** |
| 同frameのviewport／content origin | 756／-580 | 756／-580 |
| 当該frameのcanonical／touch端数 | 756／0 | 756／0 |
| 次の本物mouse +100後のnative offset | 656 | **576** |
| 行丸め後canonicalと次frameのviewport | 630／630 | 630／630 |

両分岐でfolder・items／世代・選択・列数・background captureは同じ。
**旧synthetic Moveを新mouseの移動としてnativeが採用する不具合は、基点から存在する。**
その80ptは次の本物mouse deltaにも混ざる。一方、基点のこのfixtureは行丸めで描画差を隠す。
>>661の新契約はphysics後の位置でviewport／content originを確定するため、現行版では同frameのviewportも756→800となる。
**同じ描画症状まで基点で出た／今回の変更の影響がない、とは扱わない。**

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo test --manifest-path target/scroll661-v410-baseline/Cargo.toml --target-dir target -p mimageviewer --lib retired_secondary_touch_move_cannot_drive_a_new_mouse_capture --locked --offline -- --test-threads=1 --nocapture
```

最終の凍結source実行は **0 PASS / 1 FAIL、exit101**、実行1.95s・compile込み107.13s。
旧Moveなしの対照は不変、旧MoveありのState不変assert相当だけが失敗した。
source開始／終了SHA一致とbinary／logのSHAを記録した。compile中にfixtureを補正した
初回試行は最終証拠に数えず、補正完了後に再実行した結果だけを採用している。

- ログと実行証拠: `target/scroll661-baseline-final.log`、`target/scroll661-baseline-final-evidence.json`。
- 差分・source監査: `target/scroll661-baseline-fixture.diff`、`target/scroll661-baseline-source-audit.json`。
- 生成・採取: `target/scroll661-prepare-baseline.py`、`target/scroll661-adapt-baseline.py`、
  `target/scroll661-run-baseline-final.py`。基点test SHA-256は
  `15d4c15ee3ea26b339b06353fa9f010dc81686d42d184f537592d28ac453bb27`。

Sol / xhighの独立reviewerもsource・入力列・最終証拠を読み取り確認し、上記留保を含めて
設計担当の条件付き除外指示を適用してよいと判断した。再実行・製品の実機配送はしていない。
利用者判断に従い混在操作を **§1.487** へ起票し、元の追加回帰は
`#[ignore = "known pre-existing: §1.487 (v4.10.0 native touch/mouse pointer mixing)"]` で保持する。
これは既存の混在入力不具合の切出しで、通常一覧切替のbounds／速度／capture修正を取り消さない。
承認済みAPIや製品ロジックを追加変更せず、段Cの残りのgateへ進む。

<a id="stage-c-final-verification"></a>

### 段C再開後の統合検証（2026-10-30）

Collectionの20試験は、旧offset／edge／reveal fieldと最後のgeometryの読み出しを
同じscroll ownerのaccessor／要求入口へ適合した。fixture、キー・worker・帰還経路、20件の期待値は変更しない。
§1.487の理由付きignore以外に試験を除外していない。最終gateと20件の結果は本節の台帳に記録する。

R2の最終棚卸しで、別viewportの実frame番号を直接比べる対照を追加した。
`grid_viewports_keep_independent_frames_and_commands_across_discard` と
`grid_transport_only_and_replay_use_the_actual_viewport_frame` は、同じegui ContextへROOT／childの
実RawInputを交互に渡す。同じscopeとTouchId、同数frameの独立性、片側だけを先行する両方向、
実discard／replayのcommand一回性、休止側のmarker／replay状態、実end-passでの旧End観測、
新Startの再開まで検査する。viewport出力は適用せず実窓を作らない。
追加は `touch_correlation.rs` のtest module内だけで、独立sourceレビューACCEPT。
後述のfocused45件と最終本体fullで、追加2件も成功した。

本再開の製品コードは `910391359` のまま。最初の現行focused compileは基点試験と共有した
Cargo targetのegui成果物を再利用し、新APIがない19エラーで終了した（exit101）。
基点eguiのAPIが現行には存在することをsourceで照合し、`cargo clean -p egui`（exit0）で
当該packageのbuild cacheを除いた後に再検証する。製品修正やfixtureの期待値変更で解消していない。
途中ログは `target/scroll661-stage-c-final-focused.log`、cache処理は `target/scroll661-main-egui-cache-clean.log`。
`test-focused.ps1 -Base b10dc7406` は共有App／vendor／文書を含むためexit2。
`test-lib-full.ps1` へ進む選択結果は `target/scroll661-stage-c-test-focused.log` に保存した。

最終focusedはrun `f13a9edf-e189-423d-98c8-e8412f9f2449`、**45 PASS / 0 FAIL**（27.192s）。
新viewport2件、R1〜R4・>>661、実pinch／tapと、取り込んだCollection **20件すべて**を含む。
JUnitの実行case名と件数を機械照合した抽出は `target/scroll661-stage-c-focused-results.json`、
ログは `target/scroll661-stage-c-final-focused-2.log`。Collectionの期待値を緩める変更はない。

本体全件の初回はrun `4c4fe977-b5fa-47b7-a432-99acdd060c10`、
**14,980 PASS / 13 FAIL / 66 skipped**（710.526s、exit100）。
`target/scroll661-stage-c-test-lib-full.log` と
`target/test-lib-full/runs/20261030T052514413Z-b2acac9d/` に記録した。
失敗は `startup_restore_tests::section1335_*` の13件だけで、保存行位置Some(3)対期待Some(2)の同型だった。

6箇所のhandler-only fixtureは旧cacheへcols3／行高40を直接設定してoffset80を作っていたが、
位置保存の正本となったownerのgeometryは未設定で既定cols4／行高200のままだった。
index12の計算が `12/4 - round(80/200) = 3` となる原因をsourceと全失敗から確認し、
独立reviewerも製品経路の不整合ではなくfixture API適合漏れと判断した。
両製品rendererは既に実geometryをownerへ記録する。full実行中にはsourceを変更せず完走後に、
6箇所を `set_departure_grid_geometry` へ移した。
このtest-only helperはcols3／行高40／viewport120（3可視行）と実itemsのnatural高を記録し、
13項目でもoffset80が有効なmaxになることを検査する。旧cacheも同じ値へ揃える。
RowAnchorや描画proofを作らず、期待Some(2)、入力、後段assertを一切変えない。
当該module全件と本体fullを再検証する。製品コードは引き続き無変更。

helperと6置換の独立レビューもACCEPT。旧試験の入力・期待値・後段assertが維持されていることを
機械照合した。修正後の `cargo nextest run -p mimageviewer --lib -E 'test(/startup_restore_tests::/)' --no-fail-fast --retries 0 --no-tests fail` は
run `d672fc97-61a0-4b11-a026-beebbff15287`、**73 PASS / 0 FAIL**（4.918s）。
ログは `target/scroll661-stage-c-startup-fixture-regression.log`。初回13失敗の後段まで成功した。

再度の本体fullはrun `24ae7d2c-998c-4f87-96c0-b402e9ecc393`、
**14,993 PASS / 0 FAIL / 66 skipped**（536.504s、exit0）。
当該sourceの成功receiptは `target/test-lib-full/76be7ef127a5122c711130ab56f53040e2d696ba72a662024ef0df583318f78e.json`、
source IDは `e1ea5a9740eb328971ffc0f9335d119e6d5b1dc161d8f84562e24f20a6ed8241`。
ログは `target/scroll661-stage-c-test-lib-full-2.log`、詳細／JUnitは
`target/test-lib-full/runs/20261030T054149332Z-164a4b51/`。sourceは実行中固定し、
その後の変更は本検証台帳等の文書だけで、製品・test source・条件は維持している。

同じ最終sourceのcore checkはexit0（42.84s）、test-script付きlib／tests checkもexit0（1m53s）。
`cargo test --test ui_snapshot` は **214 PASS / 0 FAIL**（32.00s、exit0）。
snapshotの更新指定はせず、PNG差分はない。ログはそれぞれ
`target/scroll661-stage-c-handoff-{core,feature}-check.log` と `target/scroll661-stage-c-ui-snapshot.log`。
本体全件にはCollection20件を再び含み、後から取り込んだ全期待値が最終sourceでも成功した。

最終workspace scriptの初回は、PowerShell内の出力リダイレクトにより
`Compiling quote v1.0.47` という通常のstderrがNativeCommandErrorへ昇格してexit1となった。
これは製品テスト失敗ではなく、`target/scroll661-stage-c-workspace-full.log` が途中試行の記録である。
Pythonから別PowerShell processを起動してOSのstdout／stderrをfileへ捕捉し、
script内部のnative stderrをPowerShellのerror streamへ合流させず再実行した。
同じwrapper内のaudit終了コードが初回中断で保存できなかったため、auditも記録を取り直した。
recorderは `target/scroll661-run-handoff-gates.py`、最終command・exitは
`target/scroll661-stage-c-handoff-gates-native.json` にrunごとに保存する。

取り直したauditは **exit0・違反0**（80.65s）、workspace fullは **exit101**（6.67s）。
workspace fullは `mimageviewer-launcher` のbuild scriptが必要とする
`target/release/mimageviewer-core.exe`／`mimageviewer-remote.exe`／`mimageviewer-epub-pdf.exe` の
3ファイルが未配置のため、テスト開始前に停止した。fakeの入力やdebug exeへの置換はしない。
確認用build不要の依頼に従い、これを埋めるためのrelease buildは行っていない。
成功した本体全件とは区別し、**共通workspace／doc／後続vendor graphのfull gateは未完了**とする。
本機能のvendor eguiは、製品sourceと同ファイル内testsが前回から無変更のため、
変異復元後の最終 **161 PASS**（1.735s）を再利用する。`PATCHES.md` の参照もそのfinal logへ揃えた。

| 最終コマンド・検証 | 結果 | 証跡 |
| --- | --- | --- |
| `cargo fmt`（引数なし）／`git diff --check` | 成功 | 最終sourceへ適用／差分確認 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit0、42.84s | `target/scroll661-stage-c-handoff-core-check.log` |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit0、1m53s | `target/scroll661-stage-c-handoff-feature-check.log` |
| `cargo nextest run --offline --manifest-path vendor/egui/Cargo.toml --lib` | 161 PASS、1.735s、同sourceの成功を再利用 | `target/scroll661-stage-c-vendor-final.log` |
| `scripts/test-focused.ps1 -Base b10dc7406` | exit2、共有変更のためfullを選択 | `target/scroll661-stage-c-test-focused.log` |
| `scripts/test-lib-full.ps1` | 最終14,993 PASS、66 skipped、exit0 | `target/scroll661-stage-c-test-lib-full-2.log`／上記receipt |
| `cargo test --test ui_snapshot` | 214 PASS、exit0、PNG変更なし | `target/scroll661-stage-c-ui-snapshot.log` |
| `cargo run --locked -p viewer_context_audit` | exit0、違反0 | `target/scroll661-stage-c-handoff-audit-native.log` |
| `scripts/test-full.ps1` | exit101、release内包exe3点不足でcompile停止。テスト未実行 | `target/scroll661-stage-c-workspace-full-native.log` |

| 段 | 最終作業ツリー・受入状態 |
| --- | --- |
| A | `a930891dd`で挙動不変（R3を除く）のcheckpoint済み |
| B | 元4再現とR3／R4を含む15件・関連520件成功。`910391359`がBおよびC途中実装のWIPを保持 |
| C | 入力／contextの実装と本体・focused・UI／auditの統合確認は完了。基点既存の混在入力は設計担当判断で§1.487へ分離。追加製品変更はなく、viewport試験2件、既存fixtureのAPI適合、理由付きignore、文書の差分を未コミットで保持。共通workspace full gateは上記前提不足のため未完了で、出荷判定は保留 |

R1の実App folder切替と両renderer／bg・bar、R2の実App discard・無描画往復と追加viewport対照、
R3の実reload／rescan／resume、R4の実geometry clampからnative motionと描画への接続を検査した。
同path A/B・fork／park／move／swap・兄弟不変はApp／registry、異geometry／保存IDのcaptureはvendorの
対照へ分担しており、全組合せを一つのApp end-to-endで実行したとは扱わない。
独立コードレビューの追加指摘は解消済み。最終台帳もSol / xhighの読み取り照合でACCEPTとなった。
reviewerはJUnitの本体14,993件・Collection20件・startup73件・追加viewport2件と理由付き除外、
各check／snapshot／audit、workspace前提不足の記録を照合し、テストを重複実行していない。

F12／複数窓の変更範囲と構造上の理由はdetached計画§11へ追記した。
製品起動・desktop操作・実profile使用・確認用build・commitは行っていない。
§1.488を「実装済み・実機未確認」として起票し、詳細の旧列数を使う行位置保存不具合も併記した。
実機確認は利用者の事前承認を得る別枠で、今後は専用dataと200／100の有効ZIPによるツリー往復、
Collectionの本からの各帰還、両renderer・通常wheel／drag・A/B／F12を確認する。予定は1枠約10〜15分＋準備。
今回、実画面で確認済み／報告者のWindows10で改善済みとは報告しない。

<a id="implementation-review-revise"></a>

### 6c4369e3e の実装独立レビュー後の再修正（2026-10-30）

ClaudeCodeが前節までを `6c4369e3e` へコミットし、独立実装レビューでP2四件のREVISEとなった。
構造的修正の方向は合意済みだが、前節のR1／R2等の完了記録だけでは以下の未達を検出できていなかった。
その記録を当時の結果として保持し、四件を回帰へ追加して再確認した。開始時の作業ツリーはclean。

| 指摘 | 確認した違反と修正した所有境界 |
| --- | --- |
| 1: barのpress→release→press Held | 完了clickをendで処理すると、古いReleasedが新Heldとgrabを上書きした。`finish_external_bar_press` は既存press stampを後退させず、frame-localなbar位置にも既存 `ExternalPress` のstampを渡す。完了click／bar位置の最大2区間をstamp順で適用し、新Heldのgrabを維持する |
| 2: non-rubber位置と描画の不一致 | `AxisPhysics::drag/step` は非rubberのclampをcallerへ委ねるため、opt-in配置前に現在boundsへ確定し、実際に越境をclampしたときはY速度も終端する。正当なrubber-bandを維持し、viewport／origin／返却Stateの位置を一致させる。既定APIの配置順・physics式は変更しない |
| 3: 旧touch releaseによる宛先cellの操作 | 両rendererが実 `TouchFrame` を共通 `handle_cell_interaction` へ渡し、primary click／double-clickの適用だけをrelease所属で拒否する。press、right-drag抑止、native D&Dの既存受理は分けて維持し、旧release後の新Heldを奪わない |
| 4: raw End(q)とbackend release(p)の誤照合 | backendのgate／cursor／既存pendingを `PointerTransport` へ集約。実PointerMoved／Goneでcursorを観測し、Endは最後のcursor位置でreleaseを照合する。cursorなしならGoneのみを待つ。scope交代でbackendの状態を消さず、旧Endの正常なtailで現在recognizerを取り消さない |

`TouchFrame::should_suppress_response` は、後続pressで変わるaggregateなresponse座標に依存せず、
既存API `released_buttons_with_click_counts()` の最後のclick-bearing primary releaseへ対応させる。
派生情報のprimary-release ordinalを相関結果に保持し、曖昧なCurrentイベントがfinishで除去されても
Retiredの序数を詰めない。clickがない場合は最後のterminal releaseへ対応し、Stillのdrag終端抑止も保つ。
このcounterはframe-localであり、新しい永続bool／pending枠／queueは加えていない。
EndGoneは既存pending enum内で未確認のterminal commandを運び、Goneの確認前に実行せず、scope退役で破棄する。

#### 先行REDと修正後の回帰

- vendorの元3回帰は **0 PASS / 3 FAIL**（exit100）を先行確認した。
  `external_capture_completed_bar_click_then_held_press_keeps_latest_sequence` は
  press→release→press Held→次frame Move→releaseを実Contextへ渡す。
  `external_vertical_edge_crossing_draws_final_drag_and_coast_position` と
  `external_vertical_extent_shrink_to_fitting_content_draws_clamped_coast` は、rubber有効／無効、drag／coast、
  scroll不要へ縮小したSyncを通し、viewport／origin／返却Stateを検査する。
  ログとsource前後SHAは `target/scroll661-revise-vendor-{red,green}.{log,json}`。
- vendor途中の独立レビューで、逆向きの「前frame Held→旧release→新short click/release」も必要と判明した。
  旧releaseのbar位置を新clickより後に適用する逆転を、期待620／実127.99998のREDで確認した。
  `external_capture_old_bar_release_then_short_click_keeps_input_order` を追加し、上記stamp順へ修正した。
  正当なbar releaseの浮動小数差だけは0.001pt許容とし、入力順の位置期待は維持した。
  最終 **165 PASS / 0 FAIL**（1.088s、compile込み5.36s、exit0）。
  run `0eeb7c0e-f5d9-427d-8554-bdf8e188047e`、
  `target/scroll661-revise-vendor-order-{red,green}.{log,json}`。
  vendor source SHA-256は前後一致の `465e6febc5054cc2ee2bdc2a7618d88058761106f9d96575acd30c58fb9b40e8`。
- 指摘4の2回帰は、旧Move(p)→scope交代→新接点2 scroll→旧End(q)/release(p)で、
  曖昧扱い・現在接点の取消になるREDを確認した。
  ログは `target/scroll661-revise-app-red.log`。修正後は同pass／分割tailの双方で接点2が継続する。
  cursorなしEnd、EndGoneの分割／退役、後続の同座標mouse、除去されたCurrentと後続nonclick release、
  Stillのnonclick drag終端を含む計6回帰を `touch_correlation.rs` へ追加した。
  既存3fixtureはrelease座標／Goneのみの列をbackendの実生成規則へ訂正し、現接点保持のassertは維持した。
- 指摘3の最初のFolder-only fixtureは、旧Endでegui clickが出る前提を満たさず、原因再現とは数えなかった。
  実cell Responseのtest-only witnessでcaptureとtap候補を確認し、非モーダルな同path A/B採用を追加した。
  実 `activate_quick_folder_slot` によるsession交代、同じcell ID、実egui clickを確認したうえで、
  両rendererの旧releaseが選択None→Some(1)へ変える **4件の有効RED**を得た。
  `target/scroll661-revise-app-red-3.log` はtouch全43件PASS／cell4件FAIL。
  通常Folder採用の対照も残し、両renderer×Explorer／Check、旧End直後の同frame新mouse Held→次framerelease、
  新しいtouch tapを正の対照にした。宛先のchecked={4}も旧releaseで消去・変更されないことを検査する。
  新しい正対照で誤認した「Checkのplain clickでチェックも付く」という期待は、製品sourceを照合して
  「選択更新・既存チェック保持」へ訂正した。旧capture／egui click、旧release後の選択・集合不変assertは維持した。
  診断用SnapshotのDebug指定とmouse専用helperの新引数追従漏れによるcompile試行はテスト結果に数えない。

最終focusedは **90 PASS / 0 FAIL**（13.950s、exit0）、run `46047350-dbff-4de8-8ef2-8939216efd71`。
`target/scroll661-revise-app-green-3.log` に、追加回帰、元>>661、R1〜R4、Collection20件、
実pinch／tapとtouch全件の成功を記録した。独立機能再レビューはACCEPT・追加指摘なし。

#### 再修正の最終検証

全Cargoは `MSBUILDDISABLENODEREUSE=1`。製品／test sourceを固定して検証し、結果確定後は文書だけを更新した。

| コマンド・検証 | 結果 | 証跡 |
| --- | --- | --- |
| `cargo fmt`（引数なし）／`git diff --check` | 成功 | 最終sourceへ適用／差分確認 |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit0、89.20s（compile込み） | `target/scroll661-revise-final-feature_check.log` |
| `cargo nextest run --offline --manifest-path vendor/egui/Cargo.toml --target-dir vendor/egui/target --lib --no-fail-fast --retries 0 --ignore-default-filter` | 165 PASS、exit0 | `target/scroll661-revise-vendor-order-green.log`／同stem JSON |
| `scripts/test-focused.ps1` | exit2、vendorを含む共有変更のためfullを選択 | `target/scroll661-revise-test-focused.log` |
| `scripts/test-lib-full.ps1` | **15,003 PASS / 0 FAIL / 66 skipped**、551.869s、exit0 | `target/scroll661-revise-test-lib-full.log` |
| `cargo test --test ui_snapshot` | **214 PASS**、35.96s（compile込み189.62s）、exit0、PNG差分なし | `target/scroll661-revise-final-ui_snapshot.log` |
| `cargo run --locked -p viewer_context_audit` | **exit0・違反0**、80.88s | `target/scroll661-revise-final-viewer_context_audit.log` |

full runは `ddcdd013-a87f-426d-8041-1fca13068bda`、
source IDは `4de51cc21fa0a101185cb63544d63b2ec187fcd8f33ed19542b354a222c45514`。
receiptは `target/test-lib-full/a4ad199e1391d6f35716f219b6c182a3d8d21369df92446ac1aeb9aaeb9f4097.json`、
JUnit／詳細ログは `target/test-lib-full/runs/20261030T070743911Z-f9054bb1/`。
元の再現期待値・Collection20件・startup73件は維持し、§1.487の既存ignoreも除外範囲を広げず保持した。
最終各gateの実command／exitは `target/scroll661-revise-final-gates.json`、recorderは
`target/scroll661-revise-run-final-gates.py`。recorderのexit0は採取完了であり、各テストのexitと区別する。

vendor変更は `scroll_area.rs` のopt-in／同file testsと `PATCHES.md` に限定した。
新しい公開fnはなく、登録表の追加は不要。`context.rs`／`interaction.rs`／`scroll_physics.rs`／egui-winitは無変更。
default ScrollArea、physics式、F12／contextのmove／swap／fork／parkの規則は変更していない。
共通workspace fullは前節のrelease内包exe3点不足が未解消で、引き続き未完了。
製品起動・desktop操作・実profile使用・確認用build・commitは行っていない。
最終の文書・証跡照合もSol / xhigh独立レビューでACCEPTとなった。
reviewerは各結果と、変更した四つのRust sourceのSHAがfull検証時のものと一致することを確認し、
Cargoや製品実行を重複していない。追加の未解決指摘はない。

<a id="implementation-review-orphan-release"></a>

### 20be36843 の再レビュー後の確認と設計前提の相違（2026-10-30）

開始時は `20be36843`、作業ツリーはclean。前回四件の解消は再レビューで確認されたが、
「本物mouseのclickの後の孤立したRetired releaseを、セルclickの発生元と誤認する」P2が追加された。
今回は以下の実Context回帰を追加した。**修正完了ではなく、所有境界の設計補足が必要なため停止している。**

#### 依頼された入力列のRED

`src/app/tests/scroll_661.rs` の次の二件は、実App・両rendererの実cell captureを通す。

- `thumbnail_mouse_click_before_retired_orphan_release_is_applied`
- `details_mouse_click_before_retired_orphan_release_is_applied`

二指touchのStart後、同pathのA/B採用でsessionを交代し、旧主指のEnd／release／Goneを処理する。
その旧captureのreleaseが選択を変えないことを確認したうえで、第二指だけが残る状態から、
本物mouseのMoved→press→release、旧第二指のEnd(q)→release(p)→Goneを同frameへ渡す。
nativeのcell ID／`clicked_by(Primary)`／`InteractionSnapshot.clicked`、二つのreleaseの`Some(click)`を確認し、
mouse clickによる選択とExplorerのチェック解除を期待する。HEADでは両方とも選択がNoneのままで失敗した。
先行REDは **0 PASS / 2 FAIL**（exit100）、`target/scroll661-orphan-release-red.log`。

#### press区間だけでは実capture区間を復元できない

frame-localにpress／releaseを順に読み、各releaseは「前frameからの区間、または新pressが開いた区間」を
一度だけ閉じるものとする候補を試した。後続のpressなしreleaseはclick-bearingでも発生元候補にしない。
any buttonのreleaseでeguiの共有captureが解放される点も反映した。
この候補では依頼二件・既存の旧cell release二件・ordinal回帰一件の **5 PASS**を確認した
（exit0、`target/scroll661-orphan-release-green.log`、run `8ab8a4c8-e4c3-4d36-bc7f-560484d22f4d`）。
途中の参照型の推論ミスによるcompile失敗は、テスト結果には数えていない。

ただし、新pressが**実際にwidgetをcaptureしたか**は、この区間列には含まれない。
同じ適用境界の次の列で、旧Retired clickを本物mouseへ誤って付け替える。

1. 前frameに旧touchでcellをpressし、eguiの実captureを成立させる。
2. session交代後のframeに、旧End→release→Goneを渡す。これは旧cellのnative clickを生成する。
3. 同frameの後半に、widgetのない場所で本物mouseをMoved→press→releaseする。
   後者にも`Some(click)`が付くが、captureがないのでnativeのclicked IDは更新されない。

`touch_correlation::tests::retired_capture_click_is_not_reassigned_to_later_uncaptured_mouse_interval`
は実Contextと実`TouchCorrelationState`でこの列を検査する。最初のcapture、最後のnative clicked ID、
二つのclick-bearing release、序数0のRetired／序数1のMouseを確認し、旧cell clickの抑止を期待する。
press区間候補でも **0 PASS / 1 FAIL**（exit100、`target/scroll661-orphan-release-capture-interval-red.log`）となった。
空間のcontains判定で推定するとeguiの前pass hit-test／重なり／
hit radius等をAppで再実装することになるため、そのguardは足さない。

`vendor/egui/src/interaction.rs::interact` は、release時に`potential_click_id`が実widgetを指す場合だけ
`clicked`を更新し、すべてのreleaseでcaptureを解放する。孤立releaseだけでなく、click対象のない新pressの
後のreleaseも、既存の`clicked`を上書きしない。一方、公開の`InteractionSnapshot`はclicked IDだけを返し、
それを更新したreleaseの序数を返さない。`Memory::interaction()`もcrate-privateであり、描画時点では
release済みcaptureの履歴が残っていない。順序付き入力だけから実widgetのclick発生元を決められる、という
前提が不足しているため、候補の製品コード変更を撤回し、回帰だけを残す。

#### 設計担当へ渡す最小の補足案（未承認・未実装）

- eguiの`interaction.rs::interact`が**実captureからclickedを更新した瞬間**に、そのpointer releaseの
  button／序数をsnapshotのframe-localな由来情報として対応付ける。clickを更新しなかったreleaseは
  この情報を変更しない。既存のclick／drag／captureの配送判断は変えない。
- Appは既存の公開`Context::interaction_snapshot`で、そのclicked IDに対応する由来を読み、
  `CorrelatedPrimary`の既存release ordinalと照合する。クリックのcaptureをAppで複製しない。
  clickのないStill dragのterminal抑止は、click由来とは分けて維持する。
- 新しい永続bool／pending／guard、待機、capture移送、再hit-test、mouse混在の中断規則変更は不要。
  新Heldを変更しない。long-touch等のpointer release以外のclickは別の由来として区別する。
- `context.rs`／`scroll_physics.rs`／egui-winitの変更は不要と考えるが、少なくとも
  **現在変更禁止の`interaction.rs`への変更が必要になる案**であり、設計担当による範囲補足前には実装しない。
  snapshotの型／API・既定経路への影響・vendor台帳は、その補足とレビューで確定する。

#### 停止時の検証範囲

全build／testは `MSBUILDDISABLENODEREUSE=1`。製品候補の修正は残さず、追加回帰はignoreせずに保持する。
候補撤回後の最終差分は本節とテスト三件だけ。`cargo fmt`（引数なし）／`git diff --check`は成功した。
最終状態で次のコマンドを実行し、**0 PASS / 3 FAIL**（exit100、0.988s）を確認した。
ログは `target/scroll661-orphan-release-final-red.log`。native capture／clickの前提assertをすべて通過し、
二件は本物mouseの選択適用、一件は旧Retired clickの抑止という本来の期待値で失敗している。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/mouse_click_before_retired_orphan_release_is_applied/) | test(/retired_capture_click_is_not_reassigned_to_later_uncaptured_mouse_interval/)' --no-fail-fast --retries 0 --no-tests fail
```

今回の未解決相違を直した実装としてのfeature check／focused→full／snapshot／auditは未完了であり、
前節の成功を今回の修正完了の根拠にはしない。vendor・製品起動・desktop操作・実profile・commitは変更／実行していない。
独立レビューはClaudeCodeが別に依頼する。本節に独立レビューの合格判断は含めない。

<a id="implementation-review-mixed-release"></a>

### 同frameのtouch／mouse混在を§1.487へ集約した再修正（2026-10-30）

設計担当から、前節の`interaction.rs`案は採らず、同frameの混在入力を§1.487へ含める判断を受けた。
中核interactionのvendor差分と上流更新時の保守負担が>>661の目的に対して大きいため、
実clickのrelease発生元をAppで推測しない規則へ簡素化する。開始時HEADは `258cd57a1`、作業ツリーはclean。
前節の停止記録とREDは当時の証跡として保持し、本節が現在の仕様・実装を示す。

#### 所有境界と許容する挙動

`TouchFrame::should_suppress_response` は既存release ordinalでframeの全click-bearing primary releaseを
既存`PrimaryOwnership`へ対応させる。旧touch由来のセル応答は、すべてRetiredの場合だけ抑止する。
Mouseが一つでもあれば抑止しない。曖昧Currentが除去されて所属を証明できないreleaseも抑止根拠にせず、
ordinary pointer fallbackを維持する。クリックのない場合は、従来どおり最後のterminal releaseを用いる。
これはMouseだけの例外ではなく、抑止しないCurrent tapや所属不明のclick-bearing releaseが同frameに
混ざる場合もv4.10.0と同じfallbackになる。純touchのCurrent／Retired共存を含む複数所属のrelease混在も
§1.487の許容範囲であり、隔離保証の対象外とする。
共通helperを使うStillFullscreen等の既存Current gestureの`should_suppress`要求も保持する。
状態field／bool／pending／guard／queueは増やさず、press区間の候補は使わない。
新Held、一般のmouse混在によるrecognizer中断、transport照合、eguiのcapture配送を変更しない。

- mouse click→旧第二指の孤立releaseでは本物mouseのcell clickを適用する。
- 混在しない旧touch releaseは引き続きcellへ適用しない。
- 旧touchのcell capture→session交代→同frameに旧End／release／Gone→widgetのない場所で
  mouseのMoved／press／releaseでは、旧cellのnative clickも抑止しない。これは設計担当判断により
  v4.10.0と同じ許容挙動として§1.487へ残す。基点で同列を再実行したという新しい検証記録ではない。
  既知混在操作の隔離を完了したとは扱わない。

#### 回帰と文書

- 両rendererの `thumbnail_mouse_click_before_retired_orphan_release_is_applied`／
  `details_mouse_click_before_retired_orphan_release_is_applied` は入力・assertを変えずに保持する。
- 前節の反例は `mixed_retired_capture_and_uncaptured_mouse_click_keep_v410_pointer_fallback` と改名し、
  native capture／clicked ID／release序数・所属のassertを保持したまま、承認された「抑止しない」を期待する。
  §1.487の許容挙動であることをコメントに明記し、ignoreしない。
- 既存の「除去Current→Retired→任意のnonclick mouse tail」のordinal試験も、全click-bearingの所属が
  Retiredとは証明できないため抑止しない期待へ合わせた。序数の非圧縮、各clickの有無、replay同値のassertは
  維持し、`ambiguous_click_release_ordinals_preserve_pointer_fallback_with_nonclick_tail` と改名した。
- 旧cellの四件、raw End異座標の二件を含むtouch全件、>>661／R1〜R4、Collection20件を対象に確認する。
  §1.487の既存Move回帰のignore・他の期待値は変更しない。
- 設計§4／§5、touch-support-plan、backlog §1.487を更新した。vendor／F12／context lifecycleの変更はない。

#### 検証台帳

全Cargoは `MSBUILDDISABLENODEREUSE=1`。製品・test sourceを固定して検証し、結果確定後は文書だけを更新した。

| コマンド | 結果 | 証跡 |
| --- | --- | --- |
| `cargo fmt`（引数なし）／`git diff --check` | 成功 | 最終差分を確認 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/touch_correlation::/) \| test(/scroll_661::/) \| test(/collection_return::/)' --no-fail-fast --retries 0 --no-tests fail` | **91 PASS / 0 FAIL**、15.124s、exit0 | `target/scroll661-mixed-release-focused.log` |
| `cargo check -p mimageviewer --lib --tests --features test-script` | exit0、42.514s | `target/scroll661-mixed-release-gate-feature_check.log` |
| `.\scripts\test-focused.ps1` | 文書を含む差分のためexit2、fullへ移行 | `target/scroll661-mixed-release-gate-test_focused.log` |
| `.\scripts\test-lib-full.ps1` | **15,006 PASS / 0 FAIL / 66 skipped**、実行549.851s、全体555.266s、exit0 | `target/scroll661-mixed-release-gate-test_lib_full.log` |
| `cargo test --test ui_snapshot` | **214 PASS**、実行34.59s、compile込み93.783s、exit0。PNG差分なし | `target/scroll661-mixed-release-gate-ui_snapshot.log` |
| `cargo run --locked -p viewer_context_audit` | **違反0・exit0**、83.828s | `target/scroll661-mixed-release-gate-viewer_context_audit.log` |

focus runは `b8c1ed08-ed7c-4502-8ee4-0753d902d197`、full runは `ca19989d-750f-4db5-8b63-86e74846047c`。
元>>661の4再現、R1〜R4、Collection20件、旧cell四件・異座標End二件は期待値を変えずに成功した。
§1.487の旧Move回帰と他のignoreは変更せず、全件の除外は従来どおり66件。
今回の混在反例はignoreせず、上記の承認された許容挙動で成功している。

full source IDは `48743471e4f281c951923ca8b68c97afa8b79329a1f9a2355b3a95c73306b145`、
receiptは `target/test-lib-full/aed9d5e20c280e0e5f182bbae34eae5056395079c23a91031922876c0a729102.json`、
JUnit／詳細ログは `target/test-lib-full/runs/20261030T081609453Z-1f458d45/`。
変更した唯一のRust source `src/touch_correlation.rs` のSHA-256は検証中・終了後の照合で一致する
`b0f9120fd2362468b86e19bc31039b11d95638cacfa550daa3560091965f6eeb`。
各gateのcommand／exit／時間は `target/scroll661-mixed-release-gates.json`、recorderは
`target/scroll661-mixed-release-run-gates.ps1`。auditは違反診断の出力がなくexit0であり、
ログ内のPowerShell `NativeCommandError` 表示はCargoの進捗stderrを記録した書式である。

vendorは無変更で再テストしていない。公開fnの追加はなく、登録表の追加も不要。
F12／contextのmove・swap・fork・park、rendererのgeometry、一般のmouse混在中断には変更がない。
共通workspace fullのrelease内包exe不足による前節の未完了は継続し、今回の本体lib全件成功とは区別する。
実機未確認。独立レビューはClaudeCodeが別に依頼する。
製品起動・desktop操作・実profile・確認用build・commitは行っていない。

### be45531ec の再レビューP3補足（2026-10-30）

設計担当から独立再レビューのACCEPT（P1・P2なし）とP3一件の説明補足依頼を受けた。
開始時HEADは `be45531ec`、作業ツリーはclean。今回は製品ロジックを変更していない。

- 設計補足と§1.487に、fallbackがMouse混在だけの例外ではないことを明記した。
  抑止しないCurrent tapや所属不明のclick-bearing releaseが同frameに混ざる場合も、
  v4.10.0と同じfallbackになる。純touchでCurrentとRetiredが共存する場合を含む
  複数所属releaseの混在は、cell clickを完全に隔離した保証の範囲外である。
- `ambiguous_click_release_ordinals_preserve_pointer_fallback_with_nonclick_tail` では、
  該当press位置にraw Touch Startがないこと、最初の曖昧判定原因がそのpressの`UnmatchedPrimary`であること、
  frameでMouseに分類されることを追加assertした。先行Currentの相関結果が除去される既存assertと、
  release ordinal／click有無／Retired所属／fallback／replayの期待はすべて維持した。
- 製品コード・vendor・状態field・ignoreは変更しない。再レビューの判断は設計担当からの受領記録であり、
  この補足差分について独立レビューを実行したという意味ではない。

検証は `MSBUILDDISABLENODEREUSE=1` で実施した。

| コマンド | 結果 |
| --- | --- |
| `cargo fmt`（引数なし） | 成功 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/touch_correlation::/) \| test(/scroll_661::/)' --no-fail-fast --retries 0 --no-tests fail` | **71 PASS / 0 FAIL**、実行10.404s、compile 1m57s、exit0 |
| `git diff --check` | 成功 |

runは `1fa82995-b30a-4290-96eb-37fb2c7ddabe`、ログは `target/scroll661-p3-release-scope-tests.log`。
追加assertはnonclick tailの有無の両方で成功した。既存の入力列・期待値・ignoreは変更していない。
HEADと現在の `touch_correlation.rs` をLF正規化し、`#[cfg(test)] mod tests` より前の内容が同一であることも確認した
（SHA-256 `265b9a874c3986a22fcf55703d1c3dc281feb6ac6ee838428f9244e6afd72f26`）。
今回の差分は文書と試験内の観測・assertだけなので、指示範囲の二モジュールを実行し、前節の全件／snapshot／
auditは再実行していない。製品起動・desktop操作・確認用build・commitも行っていない。

### 全件負荷でのR3 observation試験の不安定さ（2026-10-30）

`flaky-661`（基点 `4b3c9da43`）で、
`row_anchor_observation_keeps_middle_selected_row_in_both_renderers` の失敗を調査した。
元の二作業ツリーの失敗ログは残っておらず、そのassertとの同一性は確認できない。
以下は今回新たに採取した再現証拠である。

原因はテストの待機条件の不足だった。`Driver::settled` はpane scan・hydration・詳細metadata・
offsetの静止を確認するが、`external_rescan_pending`を待っていなかった。
初期監視登録や通知後の走査が残ったままfixtureを追加／削除すると、次の
`start_external_rescan_with_cause`は同sourceの既存workerへ合流し、`restart_requested`を立てる。
observation試験はその古いworkerのreceiverを保持し、最初の返信を変更完了後の一覧と仮定していた。
製品は古い返信の処理後に再走査するが、試験はその前に失敗していた。

- 未変更の試験を16プロセス並列で80回実行し、通常CPU条件では **80成功／0失敗**。
- 同じ16プロセスをaffinity `0xf`（4論理CPU）へ集めると **62成功／18失敗**。
  `must adopt changed rows`、または描画前の表示順assertで失敗した。
  例: 削除完了後の期待位置100に対し114。描画後の「上から2行」は失敗箇所ではなかった。
- 待機・期待値を変えず診断printlnだけ加え、同じ負荷で40回実行すると **36成功／4失敗**。
  失敗4件とも変更開始前からrescanが残っていた。
  `target/flaky661-diagnostic/0006.log` は削除開始時 `rescan=Some((2, false))`、
  返信時 `equal=true restart=true` の後に `must adopt changed rows`。
  `0005.log` は追加開始時にも走査が残り、`equal=false restart=true`、表示順105／期待120だった。

修正は`Driver::settled`に`external_rescan_pending.is_none()`を加えるだけとした。
再走査はpoll内で同期的に登録されるので、その連鎖も完了してから次のfixture変更へ進む。
fixtureのファイル変更中はApp frameを進めず、OS callbackは通知をenqueueするだけである。
したがって変更後に明示開始し保持するreceiverは、変更完了後の走査に属する。
別途debounceを待つ必要はない。既存の実worker・hydration・両renderer・追加／削除・行位置2・
選択とoffsetのassert、期限、sleepは維持し、診断printlnは撤去した。製品コード・vendorは無変更。
Sol / xhighの独立したsource／再現ログレビューも同じ原因と修正方針を確認した。

反復用driverは`target/flaky661-repeat.py`、各回の全出力と件数は
`target/flaky661-{before,before-affinity,diagnostic}/`の`*.log`／`summary.json`。
実行は全て`MSBUILDDISABLENODEREUSE=1`、`test-script`付きのlib test binaryを使い、
1プロセスにつき当該1試験だけを`--exact --nocapture --test-threads=1`で実行した。
製品起動・実profile・desktop操作・commitは行わない。テストだけの変更なので確認用製品buildは不要。

修正後は同じaffinity `0xf`／16プロセス並列で **160成功／0失敗**。
`cargo check -p mimageviewer --lib --tests --features test-script`も並行し、exit0（1m00s）。
全160ログで1試験ずつ実行したことを確認した。結果は`target/flaky661-after-affinity/summary.json`、
checkは`target/flaky661-check{.log,-exit.txt}`。`cargo fmt`と`cargo fmt --check`は成功した。
`scripts/test-focused.ps1 -Features test-script`は調査文書が選択不能なためexit2となり、
指定どおり`test-lib-full.ps1 -Features test-script`へ移行した。

全件は **15,201成功／assert失敗8／timeout1／skipped66、exit100**（1145.078s）。
`scroll_661::`は **27成功／0失敗／既存ignore 1**。対象のobservation試験も成功（12.457s）。
ignoreは既存の`retired_secondary_touch_move_cannot_drive_a_new_mouse_capture`で、今回追加・変更していない。
全件gateの成功とは扱わない。
run `985a08fc-40e7-4116-8b63-be4dc65c9c03`、JUnitと全ログは
`target/test-lib-full/runs/20261030T130757553Z-8256c58e/`、
コマンドexit記録は`target/flaky661-lib-full-exit.txt`。
実行中、別のnextestプロセスも並行稼働していることを観測した（起動元は未確認）。

変更対象外の失敗は次のとおり。

- `video::tests::real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch`:
  `tail10ms.mkv rate=48000 serial=2`、burst回数2／期待1。
- `app::rar_archive_cache_tests`の4件: fixture初回表示がsettleせず、`rar_archive_cache_tests.rs:118`。
- `app::tests::still_window_mode_key_tests`のRemote取得2件: `AcquiringRemote`／期待`RemoteActive`。
- `ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`:
  `old9 must remain visible outside the growing black leaf`。
- `app::folder_scan::section1375_scan_perf_tests::section1375_large_physical_scan_10k_100k`:
  nextestの600秒上限（600.493s）でtimeout。製品のassert失敗とは区別し、再実行していない。

assert失敗8件は、全件完走後、同じbinary・featureで各1回だけ直列実行し **8成功／0失敗**。
全件の失敗を取り消す証拠とは扱わず、対象外コード・期待値・期限は変更しなかった。
正確なテスト名・各回のexit・時間・全出力は`target/flaky661-isolated/summary.json`と`01.log`〜`08.log`。
直列確認driverは`target/flaky661-isolate-failures.py`。

最終Rust差分は待機条件1行と理由コメント3行だけで、独立した限定再確認もACCEPT・未解決所見なし。
反復・check・全件後の対象source SHA-256はすべて
`ad8fd70967a81ba634c5d9c62070659df07a6ef51a5c045e4ac6d6ea69108561`で一致する。
最後の文書更新は検証結果の追記のみ。コミットと製品起動はしていない。
