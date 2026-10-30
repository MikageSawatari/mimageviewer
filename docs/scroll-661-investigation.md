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

### 位置づけと採用案（2026-10-30、設計レビュー前）

利用者判断により次版 **v4.11.0 予定**で修正する。上記の「v4.10.0 に取り込まない」は調査時の判断として保持する。v4.10.0 は公開済み。本節の確認基点は master 取り込み後の `bf6431c09feab234f3d41cee3fd45e9354ed41fe`、前回調査の記録は `01af22d76`。本節は source inspection による設計であり、新しい実行結果ではない。

**採用案: App の一覧スクロール状態を一つの owner にまとめ、vendored egui に現在の content 高さと位置更新の意味を同時に渡す、縦一覧限定の opt-in API を追加する。** physics の式・既定動作は変えない。App から egui の private state を直接編集する案は採らない。現在の geometry を渡すだけでは旧速度が残り、速度を消すだけでは旧 bounds が新たなばね速度を生成するため、この二つを同じ境界で扱う。

今回は本書だけを変更する。ClaudeCode の確認と、別コンテキストの GPT-6.1 Sol / xhigh による独立設計レビュー後に実装する。以下の型・API 名は提案名であり、存在する API と混同しない。独立レビューは今回まだ実施していない。

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

`GridScrollState` を App 投影と `ViewerContextBundle` に一つずつ保持する。既存の `scroll_offset_y`、`scroll_to_selected`、`pending_grid_scroll`、起動時の `scroll_selected_to_rows_above` の責任をここへ移す。互換用の旧 field を併存させず、位置の読み手は owner の canonical accessor を使う。履歴・session snapshot の保存形式は従来の数値・選択のままとし、gesture identity や egui State を保存しない。

- canonical offset と、一回だけ解決する `GridScrollRequest` を同じ owner に置く。request は `Keep` / `Position { placement }` / `RevealSelection` / `StartupRowAnchor` / `Edge(Top|Bottom)` の排他的な型とする。`placement` は `KeepPosition | RevealSelection | StartupRowAnchor(rows_above)`。復元位置を受け取る操作は canonical を更新して `Position { KeepPosition }` にする。復元は同値でも旧慣性を終える意味を持つ。
- `RevealSelection` は既存の `current_grid_order` と `apply_scroll_to_selected` の計算を使い、既に見えていれば位置を変えない。`StartupRowAnchor` は現在の「選択項目を上から何行目へ」の規則を保つ。起動 owner だけがその要求を発行し、fork へ未消費の起動要求を複製しない。
- `Edge` は選択・チェック・最後の画像を変更しない。現行どおり、同じ描画境界で競合する ensure より edge を優先する。復元後の reveal は `Position` の placement を更新して合成し、見えているため reveal が無移動でも復元の `Set` 権限を失わない。未消費の edge がある間の ensure / 同一覧の位置更新は edge を取り消さず、同一覧で新しい edge が来れば最後の edge を採る。別一覧の採用では旧一覧の request を移さず、宛先の restore / initial request に置換する。要求登録・合成の順序を単体テストで固定する。
- wheel / gamepad 連続スクロールなど App が直接位置を動かす入口も owner の位置操作へ通す。egui の readback を行境界へ採用する操作は、新しい明示要求を発行しない。これを区別しないと毎フレーム native 慣性を切ってしまう。
- request は現在 geometry が決まった時点で一度 take する。描画後のリング等が積んだ要求は翌描画へ残す。旧一覧の readback は、背景操作で一覧を交換する前に完了させる現在の順序を保つ。

これらは新たな復元 bool / pending / guard の追加ではなく、既存の分散した要求の置換である。新しい async owner・待ち・channel は作らない。保存復元値の取得、worker 完了の採否・世代検査は各既存 owner に残す。

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
    position: Sync(offset) | Set(offset) | AppGesture(display_offset)
}
```

`Sync` は通常の canonical 同期であり、同じ入力セッションの native 速度・drag を保存する。`Set` は復元・edge・実際に位置が動く reveal / App の明示位置操作で、旧速度と旧 animation target を終了する。`AppGesture` は既存 touch recognizer / snap glide が位置を所有する描画で、端数込みの表示位置をそのまま渡し、native Y の速度・target を終了して drag / coast と二重適用しない。touch owner が所有する合成 pointer の bar 操作も重ねず、既存の touch / mouse 入力分類・readback 抑止を維持する。App の touch の判定を egui が推測しない。新しい永続 pending は不要である。

`ScrollArea::begin` の手順を、このモードに限って以下へ揃える:

1. 既存の outer size、bar 占有、min size から **現在の `inner_size`** を決める。
2. 渡された `content_height - inner_size.y` で現在 Y bounds と `content_is_too_large.y` を確定する。前回の `max_offset.y` を physics の入力に使わない。セッション変更時は、この State の Y 速度・Y target・bar grab anchor・momentum bounce・旧 interact rect の継続を終了する。offset や履歴を既定値へ戻す reset ではない。
3. `Sync` / `Set` / `AppGesture` の位置を現在 bounds で適用する。`Set` はセッションが同じでも Y 速度・Y target を 0 / None にする。レイアウト縮小で有効範囲外になった注入値は、ばねを起動せず現在範囲へ clamp し、その不正範囲へ向かう旧 coast を持ち越さない。
4. **その確定した位置から**現在 bounds の `AxisPhysics` で通常の drag / step を行い、その結果を使って content UI の origin / viewport を作る。現行のように physics 前の位置で viewport を作ったまま、返却状態だけ変更する順序にはしない。`AppGesture` では前述の native Y 処理を重ねない。
5. `Prepared::end` の新しい wheel・scroll target・scrollbar の処理も同じ現在 bounds を使い、実測 content と現在の viewport で State を保存する。新 API の既知高さと実測高さの一致は回帰テストと debug assertion で検査する。二回描画・discard 強制・次 frame への補正で吸収しない。

`Set` の優先権は **過去から残った physics / animation に対する優先権**である。同じ一覧で現に掴んでいる scrollbar や現在 frame の新規 drag / wheel は既存順序どおり処理できる。`Set` のたびにその pointer capture を奪ったり、bar grab anchor を消したりしない。別一覧へ入った場合は下記の別 interaction ID により旧 capture が新一覧へ作用しない。これにより、毎フレーム位置を同期しても正当な scrollbar / details drag は維持する。

State の保存 ID は、App 側で `(ViewerContextId, 描画surface, 縦一覧)` を明示し、同じ context のフォルダ移動・再走査では安定させる（path / items generation を含めない）。これにより共有 egui Context 上でも兄弟の State を上書きしない。その ID と別に、background drag と縦 scrollbar の **interaction ID だけ**をセッションで分ける。`State` 内の関連付けは `Native | ExternalVertical(session)` の型で表し、新セッションと既存セッションを区別する（runtime identity は serde 保存しない）。これは egui の入力継続の所有情報であり、復元待ち状態ではない。旧 widget capture があるままの held pointer は、新 ID に自動的に引き継がせない。Context 全体の pointer / wheel / widget data を消さない。兄弟 ScrollArea と横方向の状態も消さない。

`AxisPhysics` は現在 bounds を受ける既存の純粋計算として維持する。`scroll_physics.rs` は原則変更不要で、必要なら純粋テストだけを追加する。通常の rubber-band を全体で無効化したり、係数を変更したりしない。

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

位置要求は同 frame の行高で snap / clamp してから渡す。egui は履歴・canonical の所有者にはならず、native 入力の候補 offset を返す。App は既存の details 半行 / thumbnail `should_sync_grid_scrollbar` の読戻し閾値と丸めを保持し、丸め後も現在の snapped max に収める。touch 端数は canonical や履歴へ保存せず、現在の方向 settle・130 ms snap 補間・prefetch / keep 範囲の扱いを保つ。

既存 `GridTouchScrollState` の anchor / row / items generation 検査は、不適合な端数を再利用しない既存契約として維持する。一時状態のキーは `(viewport, ViewerContextId)`、その既存 state の適合検査に同じ scroll owner の session を含める。同一 path・同じ offset の所有者交代でも端数・glide を引き継がせず、session ごとの第二 map を作らない。ROOT の keep 範囲を読む既存 helper は main の context / session を指定し、一時投影した detached を ROOT の owner と扱わない。これは一時描画状態の scope の修正であり、別の pending や寿命 owner を追加するものではない。逆に touch の generation 検査から native ScrollArea のセッション更新を導出しない。詳細の native drag / coast が持つ一時的な連続値も、今と同じ読戻し規則を通す。仕様を「行境界が不要」に緩めたり、読戻しを禁止したりしない。

### 5. 比較した別案と簡素化

| 案 | 利点 | 採否・欠点 |
| --- | --- | --- |
| **現在 geometry + 意味付き位置更新を ScrollArea の適用境界へ渡す（採用）** | private physics state を egui が所有したまま、初回描画から正しい bounds と優先権を使える。式・通常 UI・default API の変更を避けられる | 小さい opt-in API、セッション付き interaction ID、App 2 renderer の接続が必要。既知高さと実測高さの一致を caller が保証する |
| App が `State::load/store` で offset / max / vel をまとめて変更 | builder の変更を少なく見せられ、現在の注入処理に置ける | 現 public API には max / vel / target / grab の setter がない。追加すると App が egui の内部 field と widget ID の作り方を知る。min height / bar / margin 計算も二重化する。velocity だけ消しても旧上限のばねが再生成し、max だけ直しても旧 velocity / capture が残る。全部を正しく encapsulate した API は採用案と同じ責務になり、App 直接編集の利点がない |
| すべての ScrollArea の physics を content 測定後へ移す | 未知の content 高にも一般的に対応できる | `show_viewport` の可視行・入力 hit-test を決めるため、offset は測定前に必要。二度の layout や全 UI の physics 順序変更となり、最小 vendor 差分を超える。今回の全高既知の一覧には不要 |
| `vertical_scroll_offset(Some)` は常に速度を消す / step しない | vendor の差分が短い | mIV は毎フレーム注入するため同じ一覧の native coast を消す。bar / drag と明示更新を識別できず、出荷済み機能を制限するため不採用 |
| path で ScrollArea ID を分ける / generation ごとに State を初期化 | フォルダ往復の一部で再現しなくなる | 同 path 再入場に旧慣性が残る。resize / filter の旧 geometry は解決せず、再走査で正当な drag も切る。保存位置と入力寿命を混同するため不採用 |
| hydration 完了を最初の描画後へ遅延 / 復元を毎 frame 再注入 | 観測順序を合わせやすい | 完了順序の組み合わせを温存し、新 pending や入力との競合を増やす。禁止された症状隠しなので不採用 |
| フォルダ移動・詳細取得をモーダル化 / 一覧を閉じて開き直す | 非同期の組み合わせを減らせる | 普通の閲覧を待たせ、選択・scrollbar 操作を中断する。現在 geometry と位置更新を一つの同期境界へ揃えれば順序差そのものが問題にならず、閲覧の制限は不要 |

簡素化として採るのは、**geometry と位置の確定点を一つにし、request の owner を一つにすること**である。「復元済み×旧 bounds×ばね適用済み」を後から修復する状態を作らない。rare I/O failure の回復・retry 設計は対象外で、既定の割り切り方針に従う。

### 6. 同型経路と実装時の監査範囲

| 入口・一覧 | 適用方針 |
| --- | --- |
| ツリー / address / favorite / D&D / DFS からの通常 Folder 採用 | `start_loading_items`、物理 listing install、ready 採用の共通境界で session / request を確定。source ごとの reset を追加しない |
| Back / Forward / BS、ZIP 内階層、PDF / EPUB・変換書庫の採用 | 保存値の owner と論理 source key は変更せず、可視採用・復元から同じ scroll owner を使う。未採用・Cancel・late result は新 session / 位置を発行しない |
| 起動復元 / フルスクリーンから Grid 復帰 / 選択行の可視化 | `StartupRowAnchor` / `RevealSelection` で同じ適用境界へ合流。startup の相対行位置、現在の「最小限見せる」を保つ |
| 検索（Ctrl+S/G/T、Everything、Ctrl+F の再適用）、タグ・お気に入り、Rating、Collection / Smart の root と子、閲覧履歴、サブ展開、ブックマーク、Snapshot | 共通 `items` を表示する限り両 renderer と同じ契約。各 session / return snapshot の canonical 保存先は増やさない。root と子を同じ synthetic path だけで区別しない。検索・filter の既存先頭復帰仕様も変更しない |
| filter / sort / F5 / 自動観測の再走査、削除、同一覧 streaming、遅延列完了による下部バー消失 | 現在の extent を毎描画更新。同一論理一覧の既存位置規則を保つ。再生成の generation を理由に native 入力を一律終了しない |
| Home / End（選択移動）、リング / mouse gesture / gamepad の `GridScrollTop/Bottom`、gamepad 連続 scroll | 前者は cursor owner → reveal、後者は選択を変えない edge。既存 KeyAction と入力 router を保ち、scroll 要求だけ一元化 |
| A/B、main / active detached の bundle 投影・fork / park / promote | offset・request・session を同じ bundle で move / swap。fork の位置継承 policy は既存どおり、入力 session は新規。実 park の当該入力だけ終端。兄弟の canonical / generation / velocity / jobs は変更しない |
| 詳細の外側横 ScrollArea、F12 の独立した本ページ一覧・pan / 連結読み、音楽 timeline、side panel / popup | canonical 縦一覧とは別 owner。新 API を勝手に適用せず既存契約を保持。opt-in なしの動作を vendor 対照テストで確認する |
| 製本 / Collection 並べ替え（`ui_main.rs:9774`、`ui_dialogs/collections.rs:4854`）、整理・設定・metadata の明示 offset | source 検索で別の `vertical_scroll_offset` caller がある。各ダイアログ固有位置であり main の folder restore へ接続しない。今回 default semantics を変えないため動作維持を確認し、独自の動的 extent / 明示移動を同じ controlled 契約へ移す作業は別 chunk とする。本修正で全 egui caller の未知高問題を解決したとは報告しない |

実装時の一覧採用監査は `install_loading_items_inner`、`install_new_items_inner` とその caller、`install_prepared_*`、`TopLevelGridView::replace_surface`、context の fork / swap / park を起点に行う。各 caller を「新入場 / 同一覧更新 / 所有移譲」に分類し、全体 install を全部新入場と扱わない。cancel / failure で旧 owner を保つか、既に可視 shell に移ったかも区別する。UI thread に filesystem 判定や新しい非同期準備は追加しない。

### 7. F12・複数ウィンドウと detached 憲法

**bundle のスクロール状態の移譲には触れる。HWND / viewport の生成・閉鎖・配置や detached predicate は変更しない計画。** `ViewerContextBundle` に旧三 field のまま追加情報を散らさず、単一 scroll owner を登録し、既存の swap / fork / park の所有境界で扱う。

これは `detached-rework-plan.md` §2 の BA-7（分散状態）と同じ所有原則に従う構造修正である。新しい detached bool、geometry による host 判定、recreate、時間窓、repaint、兄弟への reset は不要。park による入力停止は既存 lifecycle owner に集約し、描画中の `is_detached` 分岐で補修しない。単なる一時 mount / read-only open を park と読み替えない。

この構造判断への ClaudeCode / Codex 双方の合意は **未取得**。設計確認後、実装で触る bundle / park 範囲と上記理由を同 plan §11 に記録することを着手条件とする。今回は依頼どおり本節のみを追記し、§11 に合意済みとは記録しない。実装中に viewport / predicate の追加変更が必要になれば、その範囲を設計担当へ戻す。

### 8. 出荷済み挙動への影響

| 変わる点 | 利用者への意味 |
| --- | --- |
| 長い一覧の復元を、直前の短い一覧の bounds で押し戻さなくなる | 選択・保存した位置を初回描画から保持する。バックアップ ON/OFF・素材・完了順序に依存しない |
| 別一覧への移動・再入場に旧 drag / bar grab / coast が流入しなくなる | 元一覧で始めた操作は元一覧で終了。移動先での新規入力は即受理し、release 待ちの時間窓を作らない |
| 明示移動と layout clamp が過去の native velocity / animation に勝つ | Home/End・edge・復元・App wheel の結果が残った速度で流れ続けない。同じ一覧で保持中の正当な pointer 操作は維持する |
| viewport / content 確定と可視行の算出が一致する | resize や下部バー出入りでも旧高に対する不要なばねを作らない。64 pt min・margin・横バー・行末余白の見た目は現行寸法を維持する |

新しい待機・手順・機能削除は予定しない。通常閲覧の速度、wheel 行数、bar 操作、details drag / coast、touch の方向 settle / snap glide、選択・チェック・保存位置・履歴・A/B の意味は維持する。**通常の使い方を不便にする変更は本案では予定していない。** 同一一覧の慣性廃止、drag の release まで他操作を拒否、復元機能や入力経路の制限が必要だと判明した場合は実装を止め、出荷済み挙動と費用を示して ClaudeCode から利用者へ相談する。

DB / sidecar / archive / 設定の schema・保存値の意味は変更せず migration は不要。main の位置修正を理由に thumbnail cache、PDF epoch、worker / context generation を更新しない。追加処理は既存 layout に対する定数時間の算術・identity 照合で、再走査・decode・GPU upload を増やさない。

### 9. テスト計画と受入条件

実装前後を同じ入力・fixture・assert で比較する。すべて `MSBUILDDISABLENODEREUSE=1`、一時 data_dir / 有効画像入り ZIP・JPG、非対話 headless を使う。実キー polling の focus 隔離、Full-frame driver / 実 worker、output の明示破棄という前回条件を維持する。今回は設計のみなので test / build は実行しない。

1. **既存 `scroll_661::` 9 件をすべて成功へ**。自然順 200→100→200 の両表示、ON / 逆向き / JPG、fast hydration、fresh surface / spring-off 対照を残す。egui 最小再現には採用 API で現在高と明示更新を渡すよう caller を適応するが、旧上限・新上限・要求位置・期待値・ばね有効条件は維持する。API 導入前の red 証跡を保存し、旧 API を無断で一般修正した結果とは扱わない。
2. fast hydration をサムネイルにも追加し、最初の宛先描画の前 / 後に実復元が終わる両順序を制御する。件数200/100の比較だけでなく、短い / 空 / 大きい同一 source の範囲変化、最終項目以外の保存位置も検査する。自然順の偶然の成功だけでは受入不可。
3. egui の固定時刻・実 `Context` 試験: 既知高の増減、viewport の増減、初回描画、非ゼロ旧 velocity / 旧 target を伴う Set、新セッションへ held background / bar pointer、A→B→A、同じ session の drag / bar の継続と release coast、同じ frame の新規入力。位置だけでなく描画 viewport / content origin・State bounds・velocity・bar の到達点を検査する。`Sync` が毎 frame coast を消す変異、bounds 更新だけ / 速度終端だけを戻す変異で回帰を検出できることを確認する。
4. App の request / geometry 単体試験: ensure と edge の優先順位、startup row anchor、復元と reveal の合成、描画後に積んだ要求の寿命、readback が明示更新を再発行しないこと。0件・1行・端数 viewport・64 pt 未満・倍率1/1.5/2・詳細横バーと進捗バーの出入り・サムネイル列数 / Auto 比率変更で行倍数・max・宣言高と実測高を検査する。
5. Full-frame 入力回帰: wheel / popup 消費、scrollbar の press→move→release、details drag / coast、Home/End の選択移動と ring / gamepad edge の選択不変。touch の fractional drag / 方向 settle / 130 ms 補間・pinch、keep 範囲 / prefetch の既存試験も再利用する。filter / sort / F5 / 詳細完了時の現在 extent と既存の位置方針を検査する。
6. 履歴 / BS / 検索復帰 / Rating・Collection / 閲覧履歴・サブ展開 / Snapshot / A/B の既存 position suite を通す。cancel・失敗・stale completion は可視 owner と位置を変えず、可視 shell 経由は新規入力寿命になることを検査する。keyboard / ring / gamepad は既存 handler まで通す。
7. 複数 context の非対話回帰: 片方に非ゼロ速度・位置・touch 端数を持たせ、もう片方の長短一覧移動・open / close・fork / park / resume / promote 後も兄弟の canonical・session・egui State・items generation / jobs が不変であることを確認する。同じ path の A/B、描画を挟まない往復、一時 mount / deposit も含める。実 HWND / GPU 提示の検証を headless 成功で代用しない。
8. vendor は新 API の opt-in 有無を分けて試験し、既存 native ScrollArea（nested / both axes / stick-to-end / scroll-to / wheel chaining）を退行させない。実 discard / multipass でも request が二重発火せず、最終描画と正本が一致することを確認する。App の UI snapshot は位置・最終行・バー周辺の既存 coverage を更新 / 補完するが、snapshot だけで physics を検証しない。

狭い試験から始め、関連 history / touch / context suite、fmt / core check、最後に共有 vendor を含む `scripts/test-full.ps1` の gate へ進む。vendor 差分は `vendor/egui/PATCHES.md` の末尾へ本件・opt-in 契約・変更ファイル・既定動作保持・回帰・証跡の所在を追記し、`Cargo.toml.orig` / provenance を原本として保持する。未実行・内部確認・独立受入を区別する。

実装完了後は自動 gate と通常の検証 build を用意するが、製品を agent が起動しない。実画面は本書の使い捨て portable / 専用 data の手順に、実 scrollbar drag・details drag・touch / DPI・F12 の context 往復を加え、ClaudeCode が具体的な範囲と時間を提示して別途承認を得る。Windows 10 / 11 の双方を候補とし、未確認側は未確認と記録する。

### 10. 規模と引き継ぎ

**中規模。実装・非対話回帰・レビュー修正を合わせて約4〜7人日相当**を目安とする（長い build / 全体 gate / 承認後の実機待ちは別）。API 文字数より、既存の直接 field 更新と各一覧採用の分類監査が費用の中心になる。独立レビューで境界が変われば再見積もりする。

| 塊 | 主な変更対象と成果物 |
| --- | --- |
| egui 契約（約1〜2日） | `scroll_area.rs` の opt-in API・begin / end の適用境界・interaction ID とテスト、`PATCHES.md`。`scroll_physics.rs` の数式変更なし |
| App 接続（約1〜2日） | 共通 scroll owner（小モジュール化を想定）、`app.rs` の既存 producer、`ui_main.rs` の2 renderer、`viewer_context_registry.rs` の移譲・fork / park。無関係な UI 全体の書換えを含めない |
| 回帰・検収用証跡（約2〜3日） | 前回 red の green、geometry / gesture / history / context 回帰、vendor / App gate、独立レビュー指摘対応、関連文書の更新 |

実装時の文書更新先は本書の実装・検証台帳、`coding-conventions.md`（App 正本と egui 入力候補の契約）、`details-view-and-filter-plan.md`、`touch-support-plan.md`、`folder-history-location-plan.md` / `top-level-grid-view.md`、前述の detached §11 と vendor 台帳。公開向けには内部型名を出さず、復元位置の不具合修正を通常の変更履歴へ記す。

今回の成果物はこの設計節のみ。製品コード・既存の失敗テスト・vendor は未変更、commit / build / 製品起動 / デスクトップ操作なし。実装着手可という判定や独立レビュー済みという記録は、ClaudeCode と次の reviewer へ引き継ぐ。
