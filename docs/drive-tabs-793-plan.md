# §1.793 ツールバーのドライブ欄 (ドライブ別の前回の場所を常設ボタンから開く) — 設計

- 状態: **設計 R1 (2026-11-23)、独立設計レビュー前・利用者判断前・未実装**。v5.7.0 候補。
- ブランチ: `drive-tabs-793` (worktree `G:\home\mimageviewer-drivetabs793`)。
- 出どころ: 5ch [>>785](https://egg.5ch.io/test/read.cgi/software/1782419551/785)・[>>787](https://egg.5ch.io/test/read.cgi/software/1782419551/787)。
  >>786 で「C: や D: をツールバーに常設し、クリックでそのドライブの最後のフォルダへ戻る」と確認し、
  >>787 でそのイメージだと返答があった ([backlog-on-hold.md §1.793](backlog-on-hold.md))。場所▼やツリーのドライブ選択の話ではない。
  一覧カーソルを前回位置へ戻す件は別要望 (本体 §1.795) で、本書の範囲外。

## 1. 現状 (コード)

- 記録: A/B のクイックフォルダごとに `QuickFolderWorkspace::drive_current_dirs: BTreeMap<String, PathBuf>`
  (`src/app.rs`) を持ち、`Settings::quick_folder_drive_current_dirs: [BTreeMap; 2]` へ保存する (リリース済み)。
  キーは `"C:"` 形式。`update_active_quick_folder_target` (`src/app.rs`) が、主一覧の履歴記録のたびに、
  アクティブなスロットの、そのドライブのキーへ移動先を上書きする。合成ビューのパスは記録しない。
  移動先は履歴の `Path` なので、フォルダのほかに ZIP / PDF / 変換書庫などの**コンテナのパスも入る**。
  UNC (`\\server\share`) はドライブ文字が無いので記録されない (`drive_current_key_for_path`)。
- 既存のキー操作 (操作カスタマイズ、既定キーなし):
  - `GridSwitchDriveC..Z` 「ドライブ X: の最後の場所へ切り替える」→ `apply_switch_drive_letter`
    (`src/app/gamepad_input.rs`)。アクティブなスロットの記録を `resolve_openable_path` で解決し
    (消えていれば存在する最も近い親、同じドライブに限る)、無ければルート、ルートも無ければ
    「ドライブが見つかりません」。結果を `LocationJump(Physical)` の候補にする。
  - `GridOpenDriveC..Z` 「ドライブ X: のルートを開く」→ `apply_ring_drive_letter`、
    `GridOpenCurrentDriveRoot` → `apply_current_drive_root`。どちらもルートを同じく解決して `LocationJump`。
  - リング・ゲームパッドからも同じ関数に入る (`apply_location_navigation_key_action`)。
- **UI スレッドの I/O (既存)**: 上の 3 関数は、候補を作る時点で `resolve_openable_path` (`is_dir` / `is_file`
  を親へさかのぼって呼ぶ) を UI スレッドで実行する。さらに検索の外では `dispatch_location_jump` →
  `dispatch_plain_location_jump(.., DirectoryScanRoute::UiThread)` でフォルダ走査も UI スレッドで行う
  (§1.853 の「場所の移動」経路、共有メモ `ui-thread-scan-routes-20261122.md`)。切断されたネットワーク
  ドライブでは OS が諦めるまで UI が止まり得る。お気に入りは §1.805 で `DirectoryScanRoute::Worker`
  (`AddressBarNav::WorkerFavorite`) に寄せ済み。
- ツールバー: セクションは `ToolbarSectionId` (`src/settings.rs`)、表示は `show_toolbar_*`、並びは
  `toolbar_section_order` + `ordered_with_fallback` (保存順に無い新セクションは末尾)。描画は
  `src/ui_main.rs` のツールバー関数で、お気に入りの項目クリックは `toolbar_fav_nav` → Worker 走査。
  操作の約束事は [toolbar-customization-plan.md](toolbar-customization-plan.md) §1.1〜1.2 (項目の左クリック =
  安全な操作、セクション見出しの右クリック = そのセクションの設定、空き領域の右クリック = 表示するセクション)。
- 接続中のドライブの列挙は `known_folders::available_drives()` (`GetLogicalDrives` + `GetDriveTypeW`)。
  ドライブ一覧ビュー・フォルダペイン (1.5 秒間隔) ・場所▼ (開いたとき 1 回) が UI スレッドで呼ぶ既存例外が
  ある ([ui-responsiveness.md](ui-responsiveness.md) §4 の場所▼の節: 毎フレームの再解決は禁止)。

## 2. 決めたこと (推奨案)

### 2.1 出すドライブ: A/B の記録があるドライブ (案 D1、推奨・利用者判断 Q1)

- ドライブ欄に並べるのは、**A と B のどちらかに「最後の場所」の記録があるドライブ**の和集合。文字順に並べる。
  描画は `quick_folder_workspaces[*].drive_current_dirs` のキーだけを読み、**ファイルシステムにも
  ドライブ列挙にも触らない** (毎フレームでも I/O ゼロ、新しい非同期状態もゼロ)。
- 今いるドライブは移動のたびに記録されるので、自然に欄へ入る。初めて開いたドライブは、そのとき欄へ加わる。
- 記録の和集合にする理由: A/B を切り替えるたびに欄の並びが変わらないようにするため。押したときに使う記録は
  キー操作と同じく**アクティブなスロットの記録**で、そのスロットに記録が無ければルートを開く (2.3)。
- 表示は `C:` のようなドライブ文字だけ。ボリューム名は取得しない (I/O になるため。ドライブ一覧ビューと同じ)。
- 外した USB・切断したネットワークドライブも記録が残る限り欄に出る。存在は押したときに worker で確かめる (2.3)。
  不要になったドライブは右クリックの「一覧から外す」で記録ごと消す (2.5)。
- 比較した案 D2 (接続中のドライブを出す): エクスプローラーに近いが、UI スレッドで列挙しないためには
  列挙 worker と更新の契機 (起動時・窓が前面に戻ったとき・移動の完了時など) を持つ新しい状態が要る。
  空のカードリーダーや光学ドライブ、使わない回復用パーティションも並ぶので、隠す設定も欲しくなる。
  「最後の場所へ戻る」という要望には記録の無いドライブは不要で、状態を増やさない D1 を推す。
  周期的な起床は入れない方針 (idle の CPU) なので、D2 を採っても USB の抜き差しはすぐには反映されない。

### 2.2 置き場所と表示設定

- 新しいツールバーセクション `ToolbarSectionId::Drives`、見出し「ドライブ:」。表示は
  `Settings::show_toolbar_drives: bool`、**既定は非表示** (`#[serde(default)]` = false)。
  ツールバーの空き領域の右クリック (表示するセクションのチェック) から表示する。既定で出さない理由:
  多くの利用者には新しいボタンが増えるだけで、キー操作の代替が既にある。
- 既定の並びはお気に入りの直前 (場所へ移るセクションの並び)。`default_order()` に `Favorites` の直前で加える。
  既存の保存順 (`toolbar_section_order` は移行済みで全セクションを持つ) には `Drives` が無いので、
  `ordered_with_fallback` が `Drives` だけは末尾ではなく `Favorites` の直前へ差し込む
  (`Favorites` も無い壊れた保存順なら末尾)。保存順はドラッグ並べ替えで初めて書き直されるので、
  移行フラグは要らない。ほかのセクションの末尾追加の規則 (Collections の既存テスト) は変えない。
- 表示形式は「展開 (ボタンを並べる)」だけ。プルダウン・折りたたみは設けない (表示形式の設定を増やさない。
  ドライブは多くても数個)。見出しの右クリックには既存の共通項目 (行頭に表示、ドラッグ並べ替えなど
  `draw_section_settings_menu` が全セクションに出すもの) だけを出す。
- 本体のメインウィンドウのツールバーだけ。別ウィンドウ (detached)、フルスクリーン、mIV Remote には出さない。
  Remote の IPC は変わらない。

### 2.3 押したとき: キー操作と同じ関数を、UI スレッドの I/O なしで (案 B1、推奨・利用者判断 Q2)

- ボタンの左クリックは `GridSwitchDrive{X}` と**同じ関数**を呼ぶ。今の `apply_switch_drive_letter` を
  次の形に作り直し、キー・リング・ゲームパッド・ツールバーの全入口で共有する
  (ルートを開く `GridOpenDrive{X}` と `GridOpenCurrentDriveRoot` も同じ仕組みに載せる)。
  1. UI スレッドでは、アクティブなスロットの記録 (メモリの値) とドライブのキーだけを捕まえ、
     **解決要求** `DriveJumpRequest { drive_key, remembered: Option<PathBuf>, kind: LastLocation | Root }`
     を作る。スナップショット中の拒否 (`location_jump_snapshot_blocked`) は今と同じくここで行う。
  2. 短命の worker で、今の解決規則と同じ純関数 `resolve_drive_jump(request) -> DriveJumpResolution` を実行する:
     記録 → `resolve_openable_path` (同じドライブに限る) → だめならルート → ルートも無ければ `DriveMissing`。
     記録が消えて親やルートへ落ちた場合は、今と同じ警告文を結果に持たせる。
  3. 完了を UI が非 blocking で受け取ったフレームで、解決済みのパスを**既存の移動候補**として出す。
     ディレクトリは `AddressBarNav::WorkerFavorite` と同じ扱い (`DirectoryScanRoute::Worker`、検索中は
     `dispatch_location_jump` で検索を抜ける)、コンテナ (ZIP / PDF / 変換書庫など) は今の
     `LocationJump(Physical)` と同じ経路。以後の調停・履歴・検索の終了・全画面を閉じる処理は既存のまま。
- 解決中の要求は App が 1 件だけ持つ (`drive_jump_pending: Option<DriveJumpPending>`)。後から押した
  ドライブが前の要求を置き換える (連打は最後だけ)。届いた結果を使うのは、要求時に捕まえた
  主一覧の移動の印 (`capture_main_list_navigation` / `main_list_navigation_is_current` と同じ考え方) が
  まだ現在のものであり、ビューアのセッション・モーダル・スナップショットが始まっていないときだけ。
  それ以外は黙って捨てる (別の操作を選んだ利用者の意図を優先する)。終了時に worker を待たない。
- 待機表示: 解決は普通のディスクなら即座に終わる。遅いのは切断したネットワークドライブなどで、その間も
  既存の直接移動の待機表示 (500 ms 後) と同じ見せ方にする。走査が続けば走査の待機表示へ引き継がれる。
  専用の進捗表示は作らない。
- perf: `nav/drive_jump_resolve` (ms、kind、outcome = target / fallback / missing / stale) を worker の結果に置く。
- 比較した案:
  - **案 A (ツールバーから今の `apply_switch_drive_letter` をそのまま呼ぶ)**: 追加が最小で、キーと完全に同じ。
    ただし新しい入口が UI スレッドの stat と走査を引き継ぎ、[ui-responsiveness.md](ui-responsiveness.md) §4 に反する。
    ボタンは記録の残った外付け・ネットワークドライブを押されやすい (2.1)。採らない。
  - **案 C (ツールバーだけ worker にし、キーは今のまま)**: 同じ意味の操作が 2 つの実装に分かれる。採らない。
  - **案 B2 (解決を既存のフォルダ走査 worker (`folder_pane_open_pending`) の中で行う)**: 待ちが 1 つにまとまるが、
    走査は ディレクトリ専用で、記録がコンテナだった場合に走査の結果へ「開かずに返す」分岐を足すことになる。
    検索中の `JumpToPhysicalFolder` と検索外の `DirectNavigation` の両方の目的に解決を通す必要もある。
    解決を先に済ませ、種類が分かってから既存の経路へ流す B1 のほうが既存の調停を変えない。
- キー操作への影響 (リリース済みの挙動): 結果が同じフレームではなく数フレーム後に出る (普通のディスクでは
  見た目の差はほぼ無い)。検索の外では走査が worker になり、お気に入りと同じ待ち方になる。止まらなくなる
  方向の変更で、移動先の規則・警告文・履歴の付き方は変えない。

### 2.4 今いるドライブの見せ方

- 主一覧が実フォルダ (またはそのドライブ上の書庫・PDF など) を表示しているとき、そのドライブのボタンを
  選択状態 (`selectable_label(true, ..)`、お気に入りの「今いる場所」と同じ見せ方) で描く。
- 検索結果・★一覧・タグ・閲覧履歴・ドライブ一覧・スナップショットなどの合成一覧では、どのボタンも選択状態にしない。
- 今いるドライブのボタンを押すと、キー操作と同じく記録 (= 今の場所) を開き直す。特別扱いしない。
- ツールチップ: アクティブなスロットの記録のパス (例「D:\photos\2026」)。記録が無ければ
  「記録なし (ルートを開きます)」。A/B のどちらの記録かは書かない (常にアクティブ側を使うため)。

### 2.5 右クリック (利用者判断 Q3)

- ボタン (項目) の右クリックで小さなメニューを出す (★ と同じ例外扱い、推奨):
  - 「ルートを開く」: `GridOpenDrive{X}` と同じ (2.3 の `Root`)。
  - 「一覧から外す」: そのドライブの記録を **A と B の両方から**消して保存する
    (`sync_quick_folder_settings`)。キー操作もルートを開くようになる (記録が無いときの既存の動き)。
    今いるドライブでは押せない (消してもすぐ次の移動で記録されるため)。確認ダイアログは出さない
    (消えるのは最後の場所の記録だけで、そのドライブを開けば再び記録される)。
- [toolbar-customization-plan.md](toolbar-customization-plan.md) §1.1 の「項目の右クリック = 変更操作の直接実行、
  メニューを出さない」の例外にする。理由: 直接実行に向く変更操作が無く、「一覧から外す」を右クリック一発で
  実行すると誤操作で消えるため。代案は「見出しの右クリックに『一覧から外す ▶ D: / E:』を置き、項目の
  右クリックは何もしない」。
- 既存の「場所の記憶を消す」(`clear_quick_folder_slots`) は記録を全部消すので、ドライブ欄も今いるドライブ
  だけになる。追加の処理は要らない。

## 3. 状態の組み合わせを減らす検討 (CLAUDE.md「設計の簡素化」)

- **採用**: ドライブの集合を、既にある 1 つの所有者 (A/B の `drive_current_dirs`) から毎回導く。ドライブ欄専用の
  一覧・キャッシュ・存在確認の結果を持たない (D2 を採らない理由)。
- **採用**: 解決中の要求は 1 件だけ、置き換えのみ。届いた結果は「要求時の移動の印が現在のままか」だけで
  採否を決め、割り込みを元に戻す仕組みを作らない。解決の後は既存の経路にそのまま乗せ、確定の時点を
  既存の調停の 1 か所に揃える。
- **採用**: 表示形式はボタンだけ。プルダウン・折りたたみの組み合わせを作らない。
- **採らない**: 解決中をモーダルにする案。普通のディスクでは一瞬で終わり、モーダルの出入りの方が目障り。
  遅いのはネットワークの不調時だけで、その間の別操作は置き換えで足りる。
- **採らない**: 見えている間の自動更新・存在確認。押したときの 1 回だけにする。
- まれな失敗 (設定の保存失敗など) は割り切り (ログ + 既存の通知)。
- 記録がコンテナで、そのコンテナが移動・削除されていた場合は、worker の解決で親フォルダへ落ちる
  (今のキー操作と同じ)。通常の操作で起こり得るので割り切らずに今の規則を保つ。

## 4. 永続データ

- 追加: `Settings::show_toolbar_drives` (未リリース、`#[serde(default)]` false、移行不要)、
  `ToolbarSectionId::Drives` (未リリース。旧版は `#[serde(other)] Unknown` で読み飛ばす既存の仕組みがある)。
  設定の持ち運び (`settings_transfer.rs`) では、ほかの `show_toolbar_*` と同じ分類に入れる。
- 変更なし: `quick_folder_drive_current_dirs` の形式 (リリース済み)。「一覧から外す」はキーを消すだけ。

## 5. 触る範囲 (見込み)

- `src/settings.rs`: `ToolbarSectionId::Drives`、`default_order`、`ordered_with_fallback` の差し込み規則、
  `show_toolbar_drives`、`overwrite_non_preferences_from` 等の live 値の引き継ぎ。`src/settings_transfer.rs` の分類。
- `src/ui_main.rs`: ツールバーのセクション描画 (表示判定の match、ボタン・選択状態・ツールチップ・右クリック)、
  表示するセクションのチェックリスト。
- `src/app/gamepad_input.rs` (またはドライブ移動を新しい小さなモジュール `src/app/drive_jump.rs` へ分ける):
  `DriveJumpRequest` / `resolve_drive_jump` / `drive_jump_pending` と、3 つの既存関数の置き換え。
- `src/app/update_phases.rs`: 完了の受け取りと、既存の移動候補への合流 (優先順位はキー入力の候補と同じ扱い)。
- `src/app.rs`: 記録を消す関数 (A/B 両方)、今いるドライブの判定。
- detached 述語・viewport 経路には触れない (detached リワークの凍結対象外)。

## 6. 検証 (受入条件)

- 純関数: `resolve_drive_jump` を tempdir で (記録が存在 / 消えて親へ / 親も無くルートへ / コンテナが存在 /
  コンテナが消えて親へ / 記録なし / 別ドライブを指す記録は使わない)。ドライブ欄の集合の導出 (A∪B、文字順、
  空)。`ordered_with_fallback` の差し込み (Drives の無い保存順 → Favorites の直前、Favorites も無い → 末尾、
  既存の Collections の末尾追加は不変)。
- handler-level: キー (`GridSwitchDrive{X}`)・ツールバーのクリック・リングが**同じ要求**を作ること。
  要求の時点で UI スレッドからファイルシステムへ触らないこと (テスト用の解決 backend で数える)。
  完了フレームで、ディレクトリは Worker 走査の候補、コンテナは既存の LocationJump の候補になること。
  連打の置き換え、別の移動が勝った後・全画面を開いた後の結果の破棄。スナップショット中の拒否。
  検索中に押したとき検索を抜けて移ること (既存の favorite と同じ)。
- 状態遷移: 「一覧から外す」で A/B 両方の記録が消えて保存されること、今いるドライブでは押せないこと、
  外した後のキー操作がルートを開くこと。A/B 切替で欄の並びが変わらず、ツールチップとクリック先だけ変わること。
- 設定: 既定非表示、表示の切替が保存され再起動後も残ること、設定の持ち運びの分類テスト。
- UI スナップショット: ドライブ欄を表示したツールバー (選択状態あり・なし) を 1 枚追加。既存のスナップショットは
  既定非表示なので変わらない想定。
- 既存のドライブ切替のテスト (`drive_current_dirs_are_isolated_per_quick_folder_slot` ほか、
  `search_nav_entry_inventory_tests.rs` の入口一覧) が、非同期化に合わせた形で通ること。
- 実機確認 (利用者・サブ PC): 外した USB・切断したネットワークドライブのボタンを押しても一覧が止まらないこと、
  A/B での切替、検索中のクリック。

## 7. 文書の更新 (実装時)

- マニュアル (ツールバー・キー操作のページ) と製品ページの該当箇所、[spec.md](spec.md) (`show_toolbar_drives`、
  `quick_folder_drive_current_dirs` の説明にドライブ欄を追記)、[toolbar-customization-plan.md](toolbar-customization-plan.md)
  (セクション一覧と §1.1 の例外)、[keymap-spec.md](keymap-spec.md) (ドライブ移動が worker で解決すること)、
  [ui-responsiveness.md](ui-responsiveness.md) (新しい解決 worker)、[folder-history-location-plan.md](folder-history-location-plan.md)
  (ドライブ別の記録の利用者が増えたこと)、`docs/backlog-on-hold.md` §1.793 を本体バックログへ移す。

## 8. 利用者に決めてもらうこと (推奨を先頭)

- **Q1 出すドライブ**: (推奨) A/B どちらかに最後の場所の記録があるドライブ (I/O なし、外したドライブは右クリックで外す) /
  接続中のドライブ (列挙 worker と更新契機が要る。USB の抜き差しはすぐには反映されない)。
- **Q2 キー操作も同じ非同期の経路に寄せるか**: (推奨) 寄せる (ドライブ移動のキー・リング・パッドも UI が止まらなくなる。
  移動先の規則は同じ、表示が数フレーム後になる) / ツールバーだけ (同じ操作の実装が 2 つになる)。
- **Q3 右クリック**: (推奨) ボタンの右クリックで「ルートを開く」「一覧から外す」の小さなメニュー / 見出しの右クリックに
  「一覧から外す ▶」を置き、ボタンの右クリックは何もしない。
- 既定は非表示、置き場所はお気に入りの直前 (ドラッグ並べ替えで移動可)、表示形式はボタンだけ — は推奨どおりで進める
  (異論があれば指示してほしい)。

## 9. 経緯

- 2026-11-23 R1: 設計担当 (ClaudeCode Opus 5.5) が起案。独立設計レビュー前。
