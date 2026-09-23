# 複数ウィンドウ・画面遷移のシナリオ自動テスト計画

2026-09-23 起案 (設計・検収 = ClaudeCode Opus 5.5 / 実装 = GPT-6 Sol / 独立レビュー = 別の GPT-6 Sol)。
状態: **T0 レビュー済み。T1 実装・対象検査済み、独立レビュー待ち（§8.2）**。

## 0. 背景

v4.0.0 の出荷前実機確認で、複数ウィンドウの不具合が 2 件見つかった (観測者: 利用者)。

| 件 | 症状 (利用者の報告) | 原因 (コード) | 既存テストが拾えなかった理由 |
| --- | --- | --- | --- |
| A (1ffce8118) | 複数ウィンドウモードで画像・ZIP の大半が開けない | 新しい detached session が描画前に Active 扱いになり、生存判定 `active_detached_transition_outstanding` が非同期 sidecar 復元の待ちを含まず `should_drop` した | 判定の各入力は個別にテストされていたが、**入力の組み合わせで判定する述語そのもの**と、「開いたウィンドウが描画に届く」という結果を検査するテストが無かった |
| B (fa862512e) | 片側寄せの単ページが、別ウィンドウを非アクティブにすると中央へ動き、アクティブに戻すとまた寄る | 非アクティブ窓の snapshot producer が 2 ページ見開きだけを再解決し、1 ページ単位を中央へ fallback した | 非アクティブ窓を描く経路 `render_detached_image_windows` をテストが一度も通していない。**アクティブ↔非アクティブの前後で描かれた位置を比べる**テストが無かった |

過去にも同じ形がある。v3.6.0 の出荷前レビューでは 5 件中 4 件が実機でだけ見つかり、自動テストは全部通っていた
([docs/review-v3.6.0/brief.md](review-v3.6.0/brief.md))。

共通点は、**個々の関数は正しくテストされているのに、「利用者が操作を並べたときに画面に何が出るか」を
検査していない**こと。今回の目的はこの層を作ることで、個別の不具合の再現テストを増やすことではない。

## 1. 既存のテスト層と穴

調査結果 (2026-09-23)。

- **状態テスト**: `*_for_test` builder で context を作り、App のメソッドを直接呼んで
  `DetachedWindowState` 等を検査する。detached テストの大半。
- **アクティブ窓の 1 フレーム**: `run_active_detached_frame_for_test` (`src/app/tests.rs`) は
  `update_active_viewer_context` だけを回す。`update_frame` 全体、非アクティブ窓の描画
  (`render_detached_image_windows`)、backstop は通らない。
- **ROOT の `eframe::App::update`**: `src/app/similar_navigation_tests.rs` の `run_root_app_update` だけ。
  ROOT viewport のみで、子 viewport は embed される。
- **子 viewport の実出力**: `render_backstop_with_captured_viewport` が
  `set_immediate_viewport_renderer` で子の `FullOutput` を取る唯一の例。viewport ごとの focus は注入していない。
- **実アプリ smoke**: `--test-script` (Rhai) と `scripts/ui-smoke.ps1`。複数窓 PDF の
  `MultiWindowPdf` がある。snapshot は page / binding / paint 出所を返すが**矩形・配置は返さない**。
  実アプリ操作は利用者の了承が要る運用 ([interactive-release-verification.md](interactive-release-verification.md))
  で、**リリース手順の必須項目になっていない**。対象も PDF のみ。

穴を一言でいうと: **「複数 viewport を持つ本物のフレーム」を、focus を切り替えながら、実ファイルで、
描かれた結果を見て検査する**テストが無い。

## 2. 目標 / 非目標

目標:

1. 不具合 A・B の**種類**を、個別の再現手順を知らなくても拾える検査を作る (不変条件で判定する)。
2. 内容の種類 × 表示設定 × 操作列の組み合わせを、機械的に広げられる形にする。
3. 非対話部分は `cargo test` で走り、`test-full.ps1` 経由で出荷前ゲートに自動で入る。
4. 実 HWND / OS focus が要る部分は、既存 `ui-smoke` を拡張して出荷前の検証枠に組み込む。

非目標:

- 画素比較。egui が出力する shape (texture id と矩形) を「描かれたもの」とみなす。
  GPU 合成・DWM・実 scanout は第 2 層と利用者確認に残す。
- PDF を第 1 層で扱うこと (PDF は `--pdf-worker` 子プロセスが要る。第 2 層で扱う)。
- 製品の挙動変更。本計画は検査の追加だけで、見つかった不具合は通常の修正手順へ回す。

## 3. 第 1 層: ヘッドレスの複数 viewport フレーム駆動 (cargo test)

### 3.1 フレーム駆動 (H1)

eframe が本番でしている複数 viewport ループを、テスト内で再現する。

- `eframe::Frame::_new_kittest()` と `<App as eframe::App>::update` を ROOT で回す
  (`run_root_app_update` を一般化)。
- `ctx.set_embed_viewports(false)` と `set_immediate_viewport_renderer` で、immediate 子 viewport を
  **その viewport 専用の RawInput** で実行し、`FullOutput` を viewport ごとに保存する。
- deferred viewport (非アクティブ窓の `show_viewport_deferred`) は、ROOT pass の
  `viewport_output[id]` に残る callback を取り出して実行する (順序は下記の因果順序)。
- viewport ごとの `focused` / 内側サイズ / 閉じる要求 を入力として持つ。`RawInput.focused` と
  `ViewportInfo.focused` は常に揃えて与える。
- **focus の値を変えるだけでは、非アクティブ窓の activation 経路を通らない** (T0 P1)。
  deferred callback が返す focus は `focused_last_frame` を更新するだけで、activation は別の
  watcher 要求と ROOT フレームでの commit を通る。「窓 B をクリックしてアクティブにする」は、
  本番が使う activation 要求を出し、その後の ROOT pass まで回したものとして表す。
- deferred 子 viewport には「ROOT の後に子」という固定順序は無い。ROOT が登録し、子は 0 回以上
  独立に再描画し、次の ROOT pass が子のイベントを drain する、という因果順序でモデル化する。
  `egui::Context` は 1 つに保ち、登録後の callback に差し替え、子には自分の viewport ID と
  viewport 情報全体を渡す。
- 非同期 worker は本物を使う。待ちは `wait_until(条件, timeout)` 型で、固定 sleep を使わない。

### 3.2 描かれたものの記録 (H2)

各フレーム・各 viewport の `FullOutput.shapes` を走査し、画像 mesh ごとに
`(viewport, texture id, 頂点列, uv, clip)` を取り出す。これを **PaintRecord** と呼ぶ。

矩形だけでは回転・切り抜きを失うので、頂点列・UV・clip をそのまま記録する (T0 P1)。

T1 の対象は通常の画像 mesh に限る。Lanczos も出力 texture の mesh を出す (`Shape::Callback` は
寿命保持用の 0 矩形) ので対象に入り、非アクティブ窓の静止画 snapshot も mesh を出す。
panorama と比較表示は callback だけで描くので T1 の対象外とし、後の段階でその描画の発生点に
型付きの証拠を置く。

texture id がどの context・どの項目・どのページのものかは、**描画時に選ばれた paint resource が
持つ出所**で引く。現在項目から逆算しない。`TestScriptContentProof.source_texture_id` は
入力側の id で、Lanczos の mesh は出力側の id を使う。holdover snapshot はこの証明を持たない。
したがって出所は capture / draw 時点で resource に載せて運ぶ。

`FullscreenPageLayout` や snapshot DTO は「描く予定」であって描いた結果ではないので、判定には使わない
(使うと不具合 B のように、DTO と実描画のずれを見逃す)。

### 3.3 不変条件の判定 (H3)

シナリオごとの期待値を書く代わりに、**毎フレーム共通の不変条件**で判定する。

| ID | 不変条件 | 拾う種類 |
| --- | --- | --- |
| I1 | 利用者が開いた窓は、期限内に内容を描くか、明示のエラー表示に届く。利用者の閉じる操作・エラー以外で自分から消えない | A |
| I2 | activation の切り替えだけでは、どの窓の PaintRecord (項目・ページ・頂点・uv・clip) も変わらない。**アクティブ時と非アクティブ (parked) 時を比べる** | B、Z 拡大・ZipPla の倍率ずれ |
| I3 | 窓 X を対象にした操作は、他の窓の PaintRecord を変えない | 兄弟 context への漏れ |
| I4 | viewport V に描かれる texture は、V の context の現在項目のものに限る | 別窓の texture の混入 |
| I5 | 非アクティブ→アクティブの往復後、PaintRecord は往復前と同じ | 復路の再解決ずれ (B は I5 では落ちない。§8) |
| I6 | 開いた直後のフレームで窓が存在し、内容が未準備なら読込中表示が出る | 91e75ce42 の退行 |

**安定化**は経過時間ではなく状態で定義する: viewport サイズ・pixels-per-point・設定・items generation・
選択中の内容が同じで、必要な読込と受け渡しが完了し、比較する各役割で実際の描画が 1 回ある。
サイズや DPI が変わったら基準を取り直す。比較は内容の同一性と物理 px 許容差付きの形状で行い、
texture id 自体は正当に変わり得るので比較しない。

判定の失敗時は、シナリオ名・フレーム番号・前後の PaintRecord を出す (再現に必要な情報を 1 回で出す)。

### 3.4 シナリオの組み合わせ (H4)

シナリオ = 「fixture × 設定 × 操作列」。操作列は小さな語彙から作る:

`開く(一覧から / パスで)`、`検索・一覧ビューから開く`、`フルスクリーンへ`、`フルスクリーンを閉じる`、`別窓で開く`、
`focus(X)`、`次ページ(X)`、`前ページ(X)`、`拡大(X)`、`閉じる(X)`、`一覧へ戻る`。

設定の軸: 見開き (単/見開き)、端の単ページを片側へ寄せる (ON/OFF)、表紙の扱い、
読み方向 (左→右 / 右→左)、Z / ZipPla 拡大、回転、表示トリム。

内容の軸: 画像フォルダ、ZIP、変換アーカイブ、コレクション、**sidecar 復元あり / なし**。

全組み合わせは多すぎるので、軸のペアを 1 回以上ずつ覆う pairwise で選び、既知の不具合の組は明示で足す。
実行時間の予算は test-full 全体で数分以内に収める (予算は T1 の実測後に決める)。

### 3.5 非同期の完了順序 (H5)

不具合 A は「sidecar 復元が最初の描画より後に終わる」順序でだけ出る。本物の worker に任せると
順序が実行ごとに揺れ、検査が不安定になるか、揺れの片側しか通らない。

そこで、完了を App が受け取る時点をテストが決められるようにする。各シナリオを「完了が描画より前」
「後」の両方で回す。

関門は**受信の継ぎ目**に置く: テストが持つ receiver / relay が、特定の要求の完了を保留し、後で同じ結果を
そのまま渡す。通常の poll / apply は変えない。`poll_sidecar_restore` に test 用の早期 return を入れる
形は、完了の配送以外まで変えるので採らない。T1 は sidecar 復元だけ。フォルダ読込・デコードは、
そのシナリオを足すときに別の継ぎ目を作る。

## 4. 第 2 層: 実アプリ smoke の拡張 (出荷前の検証枠)

第 1 層で扱えない実 HWND・OS focus・PDF worker を扱う。

- test-script の窓 snapshot に、**ページごとの描画観測**を追加する: 窓の所有者、項目・ページの出所、
  選ばれた texture、最終的な頂点・UV・clip、viewport の大きさ、描画 revision。配置
  (`ResolvedDisplayPlacement`) は診断用に添える。記録は描画呼び出しの位置 (アクティブ窓の paint resource
  提出点と、非アクティブ窓の callback が frozen geometry を描画に変える点) で行う。
  `FullOutput` は App の外でしか得られないので、第 1 層の抽出処理そのものは共有できない。
  **記録の型と比較規則を共有する**。
- 現状、非アクティブ窓の callback は `frozen_continuous_pages` が空のときだけ内容の証明を出す。
  片側寄せの単ページは frozen geometry を使うので、この証明が無い (L2 P1)。観測はこの経路も覆う。
- シナリオ: 画像フォルダと ZIP を**有効な** `mimageviewer.dat` 付きで複数窓に開き、復元が実際に
  起きたことも確かめる / 片側寄せ単ページを開いてアクティブ窓を切り替え、描画が同じかを確認する。
  先に root が ZIP の sidecar 値を中央 DB に取り込むため、detached ZIP は新規取り込み件数ではなく
  同じ非空 sidecar の読込完了を確認する。現行 T3 は 1 回の起動・1 本の Rhai で実行する。
  単窓設定での一覧→フルスクリーン→一覧は T2 に残す。
  開く操作は既存の一覧操作で表し、足りないときだけ `open_path` を足す。
- 既存の `--test-script` runner で**手操作なし**に実行し、`ui-smoke.ps1` の 1 シナリオとして
  1 コマンドにする。使い捨ての `target/portable-smoke` とそのデータだけを使う。起動前に
  session・thread desktop・input desktop の一致を確かめる preflight を足す。証跡保存は既存の仕組みを拡張する。
- **範囲の明示**: 窓の切り替えは test-script の activation 要求 + viewport の Focus 命令で行う。
  activation とその後の描画は覆うが、**OS 上のクリックで窓を切り替える経路は覆わない** (L2 P2)。
- sidecar 復元の完了順序は実 worker 任せで揺れる。不具合 A の回帰検出は第 1 層の受信継ぎ目が担い、
  第 2 層は実環境での統合を確かめる。

**運用 (2026-09-23 利用者決定):** 出荷前手順の項目は増やさない。第 2 層スイートは、公開担当が
既存の検証として実行する。**配布ビルドとは重ねない** (L2 P1): `build-dist` は起動中の viewer を
検出して止まり、内部の release / portable build は repository の viewer process を停止し得るうえ、
並行ビルドはメモリを圧迫する。配布ビルドの前にスイートを終えて app を閉じるか、配布ビルドの後に回す。このスイートに限り、実行中に PC の前面
ウィンドウ・入力を一時的に占有することへの**常設の了承**がある
([interactive-release-verification.md](interactive-release-verification.md) の「常設の了承」)。
実行前に所要時間を予告する。

## 5. 受入条件

- **変異確認**: T1 の検査は、`active_detached_transition_outstanding`を
  1ffce8118 前の pending OR のみに戻す（後続91e75ce42の`Preparing`判定も外す）と I1 が、
  fa862512e の修正を戻すと I2 が**失敗する**こと。戻したソースは一時的な作業ツリー変更に限り、
  確認後に正確に復元してmasterに入れない。
  (挙動不変のリファクタではなく、不具合を拾う検査なので、修正前で落ちることを確かめる)
- 現行 master で全シナリオが通る。10 回連続実行で結果が変わらない。
- 製品コードの変更は `cfg(test)` / test-script 用の観測と、3.5 の受信関門に限る。
  detached の述語・遷移は変えない。触れた箇所は [detached-rework-plan.md](detached-rework-plan.md) に記録する。

## 6. 段階

| 段階 | 内容 | 状態 |
| --- | --- | --- |
| T0 | 本計画の独立レビュー | 済 (§8) |
| T1 | H1 + H2 (画像 mesh) + I1/I2 + H5 (sidecar の受信継ぎ目) + 不具合 A・B の変異確認。指向シナリオ 2 本、pairwise 展開は T2 | 実装・対象検査済み。S-A folder/ZIPとS-Bは現行ソースで成功、A変異はI1、B変異はI2で失敗。独立レビュー待ち |
| T2 | 操作語彙と組み合わせの拡張、単窓設定での一覧→フルスクリーン→一覧、I3/I4/I6。安定化判定を指向fixture依存から共通化する（T1独立レビューP3）。Ctrl+G の double-click dispatch と検索 index の統合検証 | 未 |
| T3 | 第 2 層 (test-script 観測の追加、シナリオ、1 コマンドのスイート。出荷前手順の項目は増やさない) | 実装・非対話検査済み。独立レビューと公開担当のlive実行待ち。窓切替はtest-script activation要求を使い、OSクリック配送は対象外 (§4) |

## 7. レビューで確認したい点

1. egui 0.33 / eframe 0.33 で、deferred viewport の callback をテストから本番と同じ順に実行できるか。
   できない場合、非アクティブ窓の描画をどう通すか。
2. shape からの PaintRecord 抽出で、Lanczos 等の `Shape::Callback` 描画 (texture が mesh に出ない経路) を
   取りこぼさないか。取りこぼすなら、何を「描いた証拠」にするか。
3. 3.5 の受信関門を、製品の分岐を増やさずに入れられる場所はどこか。
4. I2 の「安定化後」をどう定義すれば、正当な再描画 (DPI 変化・リサイズ) と区別できるか。
5. この設計で拾えない、過去に実機だけで見つかった種類は何か。

## 8. T0 レビュー結果 (GPT-6 Sol xhigh, 2026-09-23)

設計担当 (Opus) が指摘をコードで確認し、全件を採用した。

- [P1] focus 入力だけでは parked 窓の activation 経路を通らない → §3.1 に反映。
- [P1] 画像 mesh だけでは一般の描画証拠にならない (panorama / 比較は callback のみ)。
  矩形だけでは回転・切り抜きを失う → §3.2 に反映。T1 は画像 mesh に限定。
- [P2] 不具合 B は「寄せ → parked で中央 → 再アクティブで寄せ」で、往復後は元に戻るので I5 では落ちない。
  I2 はアクティブ時と parked 時を比べる → §3.3・§5 に反映。
- §7 の回答: deferred callback は因果順序でヘッドレス実行できる / Lanczos は出力 texture の mesh を出す /
  関門は受信の継ぎ目に置ける / 安定化は状態で定義する / 拾えない種類 = HWND 再作成・z-order のクリック先・
  DWM scanout のちらつき・混在 DPI・非 Windows のコンパイル (後者は CI の番人)。
- 変異予測 (ソース読みによる、未実行): A は sidecar 復元を保留し、他の pending が無い状態で最初の ROOT pass を
  回すと I1 で落ちる。B は縦長の端単ページ・片側寄せ ON のアクティブ窓を parked にし、deferred snapshot を
  描かせると I2 で落ちる。
- T1 の最小規模: `src/app/multiwindow_scenario_tests.rs` を追加し `app.rs` に登録、sidecar の受信継ぎ目のみ
  `sidecar_restore.rs` に追加。既存の `run_root_app_update` と captured-immediate helper を再利用。
  テスト 400〜700 行程度 + 小さな継ぎ目。

### 8.1 第 2 層の第二意見 (同じ Sol セッション, 2026-09-23)

- [P1] 配布ビルドと並行させない → §4 の運用に反映。
- [P1] 非アクティブ窓の片側寄せ単ページは、現行の内容証明が出ない経路で描かれる → §4 に反映。
- [P2] 窓の切り替えは test-script の activation 要求で、OS クリック経路は覆わない → §4 に範囲を明記。
- 1 コマンド化は multi-script 対応なしで可能 (1 本の Rhai で順に回す)。`-InteractiveApproved` は残し、
  常設の了承の対象スイートのコマンドだけが付ける。

### 8.2 T1 の変異確認 (2026-09-23)

T1の3テストは、実画像フォルダ・実ZIPと有効な`mimageviewer.dat`をTempDirで作り、
sidecar checking結果を最初のROOT pass前に受信側で保留したS-A 2媒体、縦長端単ページの
active→parked描画を比べるS-Bを通した。対象3本を10回連続実行し全回成功、
テスト本体は各回0.97〜1.12秒。I1は描画のほか実際のshapeにある明示的エラー文も観測する。
S-Bは`fa862512e`の単ページfrozen producerを
旧Double限定相当に戻すと、I2でactiveの左寄せ頂点`[0,478]`がparkedで中央`[240,720]`へ
変わり失敗する。受信値はrelayがそのまま保持し、release時に同じstate machineへ返す。

初回は`1ffce8118`のOpening/Resuming判定と早期Active昇格抑止だけを戻したため、
後続`91e75ce42`の`DetachedSessionContentPhase::Preparing`が窓を保持し、
S-Aのfolder/ZIPは両方成功した。設計担当が「初回描画前に窓が消える」種類の検出を
受入意図と確認し、`active_detached_transition_outstanding`からruntime stateの
Opening/Resuming/Closing判定と`Preparing`判定を共に外し、旧来のpending ORだけ残す
変異へ改めた。この変異ではfolder/ZIPとも最初のROOT passで窓が消え、I1が
`frame=0 -> 1 window=101 vanished without an explicit error before=[] after=[]`で失敗した。
session開始時のActive昇格や`detached_window_state_for_show_label`は変更不要だった。
変異を正確に復元した後の対象3テストは全件成功した。

描画clipはactiveがviewport全体、parkedがmesh領域を指定する場合がある。
PaintRecordには生のclipを保存し、I2ではmesh頂点領域との交差で得る可視clipを比較する。
これは等価な描画を不一致にしないためで、実際に切られる領域が変わればI2は失敗する。

### 8.3 T1 独立レビューの修正 (2026-09-23)

- 非Windowsの`cfg(test)`でも描画フックを解決できるよう、提出証明の記録と抽出を
  `app::paint_record_test_support`へ分離した。Windows依存のROOT/childシナリオだけを
  `cfg(all(test, windows))`に保つ。
- 画像meshの直前に提出ごとのテスト専用markerを置き、shape順に一対一で対応させる。
  同一viewport・同一texture IDの2提出でも別々の出所を保持する単体テストを追加した。
- I1の明示的エラーは対象窓のviewportに限る。ROOTと兄弟窓のエラーだけでは対象窓の
  消失を許さない単体テストを追加した。
- 安定化判定の指向fixture依存（P3）はT2で共通化する。

### 8.4 Ctrl+G 検索結果からの画像 open (2026-09-23)

複数ウィンドウ設定で、`/` 区切りの Ctrl+G 画像結果を synthetic な `GridItem::Image` として
検索一覧に置き、ダブルクリック時と同じ production detached router
`open_grid_container_in_detached_book_context` を直接呼ぶ。親フォルダには別の先行画像と有効な
`mimageviewer.dat` を置き、detached context の物理 scan、sidecar checking 保留と再開、
ROOT / child 描画を通す。I1 に加え、対象画像の選択と main 検索一覧の維持を検査する。
この layer-1 シナリオの範囲は router・scan・sidecar・lifecycle であり、
double-click dispatch と検索 index の統合検証は T2 に残す。
修正前は `poll_detached_physical_folder_open` の raw `path_eq` が検索結果の `/` と
`read_dir` の `\` を別パスと扱い、対象不在として新規窓を最初の ROOT frame で退役させた。
テストは `frame=0 -> 1 window=1 vanished without an explicit error` で失敗し、
drive を保持した区切り文字正規化の照合へ変更後に成功した。

## 9. 実行記録

### 2026-09-23 第 2 層の初回実行 (公開担当 = ClaudeCode Opus、常設の了承に基づく)

対象は master `9c8886d08` を元にした使い捨ての `target/portable-smoke` (`portable,test-script`)。

| run | scenario | 結果 | 備考 |
| --- | --- | --- | --- |
| 20260923T042648874Z | MultiWindowStills | exit 1 | 2 窓の描画待ちが timeout。どの条件で落ちたかをスクリプトが出していなかったため、待ちごとに全窓の snapshot を出すよう変更 (`7932882a4`) |
| 20260923T051245654Z | MultiWindowStills | exit 1 | 2 窓とも描画済み・片側寄せ (`SingletonSpread { side: Right }`, x 482-960) は正しかった。ZIP 窓の `sidecar_loaded` を要求していたが、同じフォルダの sidecar を一覧が先に取り込んでいるため ZIP 窓には読むものが無い。**スクリプト側の条件が厳しすぎた** (製品の不具合ではない)。detached の ZIP 復元は第 1 層 S-A ZIP が担う |
| 20260923T051403475Z | MultiWindowStills | **exit 0** | 全段 (フォルダ窓・ZIP 窓の描画、片側寄せ、parked 比較、往復、兄弟窓不変、閉じる) を通過 |
| 20260923T051429982Z | MultiWindowPdf | **exit 0** | 既存シナリオ |

証跡は各 run の `target/ui-smoke-runs/<run>/`。窓の切り替えは test-script の activation 要求で、OS のクリック経路は含まない。
`MultiWindowPdf` の出力には入力デスクトップ preflight の行が出ていない。preflight が新シナリオだけに入っている可能性があり、T2 で確認する。
