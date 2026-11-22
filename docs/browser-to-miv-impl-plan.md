# §1.818 段 2「mIV で見る」製品実装の計画

ステータス: **実装前の計画 (2026-11-22)。独立レビュー第 13 回で ACCEPT、凍結の決まりの構造的変更に合意 (§6)**。設計書 [browser-to-miv-plan.md](browser-to-miv-plan.md) (独立レビュー第 11 回 ACCEPT) を正本とし、この文書はコードを追って確定した実装の段階・差し込み口・テストをまとめる。
作成: 実装担当 GPT-6.1 Sol / high (read-only の調査、2026-11-22) の計画案を、設計担当 ClaudeCode Opus 5.5 / high が §0 の判断を足して採用した。独立レビュー (GPT-6.1 Sol / xhigh) の記録は §6。
作業ブランチ: `browser-818-impl` (worktree `G:\home\mimageviewer-browser818-impl`)。

## 0. 設計担当の判断 (計画案 §4 の未決事項)

- **アンインストール** (設計書 §9.7 の保留): アンインストールの最後 (`usPostUninstall`) に、設定の削除の確認とは関係なく、次の 2 つだけを消す。
  - `HKCU\Software\Google\Chrome\NativeMessagingHosts\<host 名>` と `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\<host 名>`。既定値が mIV の manifest (`{userappdata}\mimageviewer\browser-host\<host 名>.json`) を指すときだけ消す。
  - `{userappdata}\mimageviewer\browser-host\` のフォルダ。
  - 「ブラウザ閲覧」フォルダの画像と、ブラウザに入れた拡張は残す。保存先を `{userappdata}\mimageviewer` の下にした利用者が設定の削除を選んだ場合に画像が消えるのは、クリップボード取り込みの保存先と同じ扱いとし、特別な保護は足さない。
  - **保存先と管理の場所を重ねない** (独立レビュー第 12 回 P1): 「ブラウザ閲覧」の保存先の基底は、`browser-host\` のフォルダ (とその下) にできない。環境設定で保存先を選ぶとき、登録・更新で host の設定を作るとき、中継 exe が保存するときの 3 か所で同じ判定 (共有部品の 1 つの関数) を使い、当たれば理由を示して断る。これで `browser-host\` を無条件に消しても画像は巻き込まない。回帰テストを足す (段階 1 の共有部品・段階 6・段階 7)。
  - **自動で消せる範囲は、アンインストーラを実行したアカウントのものだけ** (独立レビュー第 12 回 P2): インストーラは管理者権限で動く (`PrivilegesRequired=admin`)。`HKCU` と `{userappdata}` は、どちらも実行したアカウントを指す (https://jrsoftware.org/inno-setup/documentation/topic_consts.htm#userappdata)。標準ユーザーが別の管理者の資格情報で昇格した場合、mIV を使っていたアカウントのキー・manifest・中継 exe・その設定は残る。残った中継 exe は、launcher が無いので「mIV が見つからない」を返し、拡張がそれを知らせる (§9.7 の既存の動き、ファイルは消えない)。この経路は通常の操作で起こり得るので、マニュアルとアンインストールの案内に「アンインストールの前に、mIV の環境設定でブラウザ連携を解除する」と書く。拡張の知らせの文言も「mIV が見つかりません。mIV を入れ直すか、連携を解除してください」とし、何を直せばよいかが分かる形にする。既存の設定の削除 (`DelTree`) も同じく実行したアカウントにだけ効く既存の動きで、変えない。
- **Q4 (画像を包むリンク先が画像ファイルのとき)**: 計画案どおり、HTTP(S) の `linkUrl` が既存の HTML 取り込みと同じ画像拡張子の判定に当たるときは `linkUrl` を取得し、その origin も許可の要求に含める。失敗しても `srcUrl` で取り直さない (1 回の取得)。試作では確かめていないので、実装後の実機確認の項目にする。
- **native host 名**: `to.mikage.mimageviewer`。コード・manifest・レジストリ・説明書で同じ名前を使う。
- **公開鍵**: Chrome と Edge の公開鍵は、設計担当が段階 8 の指示で渡す (共有メモの `store-prep-818/` にある)。
- **拡張の受付の順序**: 計画案 §4 のとおり、同期の受付 latch → `permissions.request` (最初の await より前) → `storage.session` への記録、の順を段階 8 の状態遷移として書き、テストで固定する。


読み取りのみ実施しました。ファイル変更・cargo 実行・アプリ起動は行っていません。作業ツリーは clean です。

調査対象は `browser-818-impl` の `90dd6d45d`。ローカル master は `0e1b617b0` で、両者の差は設計文書だけです。以下の既存コードの行番号は、現在の master と一致します。新規ファイル名・型名・テスト名は提案です。

**共通の open 要求の所有を先に直し、窓の終了方針、表示先、IPC、配布、登録、拡張の順に接続する**構成を提案します。試作の App フラグ、要求ファイル、時刻による有効期限、常時連動窓にする分岐は移植しません。

## 1. コードを追って確定した事項

### 通常の二重起動にも、選択が戻る経路がある

次の経路はブラウザ試作専用ではありません。

1. `apply_startup_open_path_resolve_result` が親フォルダを読み、指定ファイルを開く  
   [startup_ops.rs:1566](../src/app/startup_ops.rs:1566)、同 `1854`。
2. `open_loaded_file_fullscreen_with_adoption` が指定ファイルへカーソルを移す  
   [startup_ops.rs:2511](../src/app/startup_ops.rs:2511)、同 `2528`。
3. sidecar 待機なら、全画面 open を `DeferredFullscreen` に預ける  
   `app.rs:73154`、`app/sidecar_restore.rs:917`。
4. 復元完了時、`resume_loading_items_after_sidecar` がフォルダ履歴の選択を戻す  
   [app.rs:45670](../src/app.rs:45670)。
5. その後の deferred open は指定画像を表示するが、一覧の選択は合わせ直さない  
   `app/sidecar_restore.rs:3114` 以降。
6. F12 の連動窓は一覧の選択に追従する  
   [app.rs:72078](../src/app.rs:72078)。

したがって、**履歴があり、非同期の復元待ちを通り、通常の F12 連動が有効な場合には、通常の二重起動でも同じ不変条件違反が起こり得ます**。これはコードからの判断で、今回の実機観測ではありません。

修正対象は「ブラウザ窓を作る直前の選択」ではなく、**有効な明示 open 要求が、復元待ちの間も選択を所有する境界**です。

### 連動窓の位置の差し込み口

`commit_viewer_session` 自体は、表示先と独立性の確定だけを行います。窓や placement を作る関数ではありません。

- 表示先確定：`app/depth_view.rs:1022`
- 連動窓の準備：`app.rs:72540`
- window ID の確保：`app.rs:65776`
- session 開始による通常 seed の確保：`app.rs:65375`

要求の seed は、`prepare_viewer_presentation_open` で表示先を確定した後、**window ID を確保し、`begin_active_detached_session` が通常 seed を確保する前**に、その window の runtime へ渡します。`detached_viewer_window_placement()` 全体を要求依存に変える方式は採りません。

位置が不適切な場合は、既存の

- `detached_window_runtime_placement_is_usable`：`app.rs:2663`
- `set_detached_window_runtime_placement`：`app.rs:2679`
- `detached_viewer_window_placement`：`app/detached_placement.rs:73`

と同じ検査・既定位置への扱いを使います。

### 複数ウィンドウには、loading と materialized の両経路がある

`start_active_detached_loading_context` は、既に seed を受け取ります。

- `app.rs:68166`：引数
- `app.rs:68195`：seed を runtime へ適用

ただし、親フォルダを読み終えた通常画像は、こちらだけを通るとは限りません。

- `route_materialized_physical_still_open_to_active_context`：`app.rs:70574`
- `fork_materialized_still_context`：`app/viewer_context_registry.rs:6246`

この既存経路も要求の seed・用途を受け取れるようにします。ブラウザ要求だけ一覧を再走査する、新しい独立 context の作り方を設ける、といった変更は不要です。

### 終了時には、用途を後から問い合わせられない

- runtime の削除・位置保存：`app.rs:3548`
- Esc 等の終了入口：`app.rs:83366`
- OS close の入口：`app/viewport_retirement.rs:32`
- active viewport の最終処理：`app.rs:67941`
- passive 窓の終了：`ui_fullscreen.rs:24825`
- メインへの focus：`app.rs:67906`

Esc の経路では最終処理より早く runtime が消えます。したがって、**用途と理由を終了開始時に値として捕捉し、終了の呼び出し系列へ渡す必要があります**。runtime の有無や「最後に閉じた窓」の App フラグでは判定できません。

### launcher だけでなく、core の二重起動経路も対象

launcher が未起動と判断して core を起動しても、その後 core が mutex を取るまでに別インスタンスが起動し得ます。

現在の `src/lib.rs:1448` 以降は、その場合に旧 open-path pipe と activation event を使います。ここも `BrowserOverlay` なら新しい型付き pipe へ転送し、**activation event を立てない**必要があります。

---

## 2. 段階別の実装

各段階を一つの coherent なコミット・独立レビュー単位にします。段階0の合意後に製品コードへ着手します。内部の設計記録は各段階で更新し、公開文書は最後に製品全体としてそろえます。

### 段階0：構造合意と実装範囲の記録

**範囲**

- `docs/browser-to-miv-plan.md`：§9.2、§9.3、§9.5 の追跡結果。
- `docs/detached-rework-plan.md`：§11 に今回の所有境界と変更理由。
- 段階別 brief：担当ファイル、既存回帰 filter、完了条件。

**確定する内容**

- 明示 open 要求が選択・表示意図を所有すること。
- 用途は window runtime、終了方針は用途と理由から決めること。
- 既存連動窓の交換は、受理済みの明示 open による終了・新規生成であること。
- R4 と同じファイルを並行変更しないこと。
- 後述するアンインストール方針など、該当段階を止める未決事項。

**テスト・完了条件**

コード変更なし。ClaudeCode と独立レビュー担当が、下の憲法照合を含めて構造的変更と合意し、§11 に記録すること。

### 段階1：保存名と Mark of the Web の共有部品

実装記録 (2026-11-22): `mimageviewer-capture-file` へ既存の名前・日時・URL・MOTW の規則を抽出。
clipboard の保存・公開処理と既存テストは本体に残し、HTML 取り込みも同じ crate を使う。
`CaptureOrigin::from_browser_urls` はページ URL と最終 URL を受け取り、既存の MOTW 境界で無害化する。
§0 の共通判定は `is_browser_host_destination(destination, browser_host_dir)`。
worker 上で既存の祖先 (junction・短縮名を含む) を解決し、未作成の末尾・区切り・Windows の大文字小文字を扱う。
独立レビュー P1 対応: `absolute` 後に `.` / `..` の component が残る入力は、祖先を解決する前にエラーにする。
verbatim パスの未作成部分を文字列操作で戻して junction を解決し損ねる経路をなくし、保存時の `join` と食い違う入力を受け付けない。
回帰は `join("..")` で入力を組み立てず、生の verbatim 文字列と実際の junction を使う。
判定不能はエラーで返し、許可とみなさない。ディレクトリは作らず、比較後の別プロセスによる祖先の置換を防ぐ保証は持たない。
この段階では呼出元を接続しない。新規テストは `browser818_capture_file` filter。
ビルド・テストは実装担当では未実行 (設計担当の共有実行枠へ引き渡す)。既存の永続形式は変えず、移行は不要。

**範囲**

既存：

- `src/clipboard_capture/data.rs:179`：`CaptureOrigin`
- 同 `230`：`sanitize_url`
- 同 `368`：`CaptureTimestamp`
- 同 `424`：`capture_filename`
- 同 `459`：`motw_contents`
- 同 `482`：`save_image`
- 同 `535`：`write_motw`
- `src/clipboard_capture/fetch.rs:1095`、同 `1130`：HTML 取り込みからの利用
- `Cargo.toml`、`Cargo.lock`

新規：

- 軽量 crate `crates/capture-file/`
- 仮称 `mimageviewer-capture-file`
- URL の無害化、出どころ、日時・月、名前、MOTW 作成・書き込みを配置。

**実装方針**

本体 crate に中継 exe を依存させません。画像デコード・clipboard API・egui も共有 crate に持ち込みません。

既存の名前を維持します。とくに、日時のミリ秒、連番桁数、衝突時の接尾辞、ドメインの選び方を試作の規則に変えません。PNG 保存と HTML 取り込みも同じ共有部品を呼びます。

MOTW は `ZoneId=3`、`ReferrerUrl=page_url`、`HostUrl=final_url`。ブラウザ側では三つの URL を検査し、userinfo・fragment を除き、改行等による ADS 内容の注入を拒否します。

**テスト**

新規 filter：`browser818_capture_file`

- 既存の名前・MOTW 出力との一致。
- 同時刻の衝突で既存画像を上書きしない。
- Unicode・IPv6・userinfo・fragment・制御文字。
- 月フォルダがまだない場合。
- clipboard の単枚・複数枚双方の規則。

既存 filter：`clipboard_capture::data::tests`、`clipboard_capture::fetch_tests`。

**完了条件**

clipboard の既存動作と保存形式が変わらず、名前・MOTW の規則が一か所になること。既存データの移行は不要です。

### 段階2：open 要求の型と、復元中の選択所有

**範囲**

- `src/single_instance.rs:629`：`ActivationOpenPath`
- 同 `637`：`received`
- `src/app.rs:4644`：`StartupOpenPathOwner`
- 同 `4707`：`StartupOpenPathResolvePending`
- 同 `1552`：`MainListNavigation`
- 同 `1144`：`MainListAfterHydration`
- `src/app/startup_ops.rs:284`：初回要求の保持
- 同 `303`：activation の選別
- 同 `397` 以降：resolver の起動・所有
- 同 `843`、`1566`：完了の採用
- 同 `2482`：指定ファイルの open
- `src/app/sidecar_restore.rs:294`、`374`、`917`、`3114`
- `src/app/book_composition_permission.rs:30`、`1340`
- `src/app.rs:45149`、`45615`、`45838`、`73069`
- `src/lib.rs:2181`

新規：

- `src/app/external_open.rs`
- `ViewerOpenIntent::{Preferences, BrowserOverlay { placement }}`
- 要求相関 ID を含む、明示ファイル open の値。
- 表示先・明示選択を既存 continuation へ渡す共通の要求型。

**要求の流れ**

```text
ActivationOpenPath::received
  → StartupOpenPathOwner::Activation { 要求 }
  → StartupOpenPathResolvePending
  → 採用した一覧の continuation
  → sidecar / composition の既存の待機 owner
  → 全画面 open
  → 表示先の確定
```

未起動時は、既存の `startup_open_path: Option<PathBuf>` を要求全体の保持に置き換え、`InitialStartup` に同じ意図を持たせます。別の `pending_browser_*` は追加しません。

`StartupOpenPathSource` は出どころの分類に残し、意図を source の判定から再構成しません。Agent の既存入口は通常意図を与え、受付規則は変えません。

**選択の修正**

有効な明示 open がある場合、復元 owner はその対象を選択の正本として扱います。

- 履歴・読書位置・一般の名前ヒントが選択を上書きしない。
- 復元を終えて表示を公開する前に、対象と一覧選択が一致する。
- deferred open の context・世代・item identity を検証してから選択権を認める。
- 置換・取消・対象消失では、その要求だけを退役させる。
- 後から `sync_detached_viewer_to_selected` を止めて隠す方式にはしない。
- 復元処理自体や既存の warm 表示を遅らせない。

明示要求は、既存の `DeferredFullscreen` や `MainListAfterHydration` に移譲します。同じ表示を二つの予約から確定させない構造にします。

**テスト**

新規 filter：`browser818_open_owner`、`browser818_selection`

- 通常の初回起動・二重起動・Agent と BrowserOverlay。
- 保存履歴が別画像を指すフォルダで、復元を意図的に待たせる。
- F12 が開いている通常二重起動で、指定画像・一覧選択・同期 stamp が一致する。
- 同じパスの新旧要求を区別し、古い完了が新要求を消さない。
- 置換、取消、close、error、対象消失、別フォルダへの移動。
- sibling の items generation、cancel、channel、cache、window binding が不変。

既存 filter：

- `app::startup_ops::tests`
- `app::startup_ops::agent_open_tests`
- `app::sidecar_restore::tests`
- `section1285`、`section1419`、`realmachine50`

**完了条件**

ブラウザの producer がまだなくても、通常の二重起動に対する選択回帰が直り、意図が全待機経路を通ること。

**段階2の実装記録 (2026-11-22、ビルド・テスト・独立レビューは引き継ぎ待ち)**

- `app/external_open.rs` に `ViewerOpenIntent`、相関 ID を持つ `ViewerOpenRequest`、パスと要求を持つ `ExplicitOpenPath` を追加。既存の `ViewerOpenAdoption` をここへ移し、要求・routing origin・明示選択の方針を運ぶ variant を追加した。`MainNew` と `ContextContinuation` の既存の入口は維持する。
- 通常 activation と Agent は受信時に Preferences の要求を作る。初回起動も要求全体を保持し、Remote 取得時の再保留、文書モーダル／EPUB 削除中の再キューで同じ ID と意図を保持する。`StartupOpenPathSource` の受付規則は変えない。
- 一覧の navigation → sidecar／composition → fullscreen → `prepare_viewer_presentation_open` まで、既存の adoption 値で運ぶ。materialized physical context への handoff は routing origin だけを continuation に変え、要求を保持する。BrowserOverlay の製品 producer、表示先の変更、配置の適用はまだ追加しない。
- sidecar 完了で context・items generation・item key を検証した明示対象を、同じターンの hydration に渡し、履歴・名前ヒント・読書位置より先に選択する。RequiredFullscreen はその continuation の exact target、composition 待ちは既存 owner の context・generation・candidate identity から選択権を確認する。最終 open でも待機の検証後・表示公開前に選択を一致させる。
- 明示要求が既存の fullscreen hydration action を置き換える場合は、古い action を退役させる。DirectMedia も close の取消対象に含める。モーダル中の受付、warm 表示、復元 worker、連動 sync の実行条件や順序を遅らせる仕組みは追加しない。App の新しい pending／detached フラグ、時刻による有効期限、回復処理、永続データ変更はない。
- 新規テストは `browser818_open_owner`／`browser818_selection`。既存テストの owner 構築・期待値は相関 ID を保持する形へ追従し、シナリオ・アサーションの意味を維持する。cargo 実行は指示に従い最後の `cargo fmt` のみ。設計担当が新規 filter と上記既存 filter を共有実行枠で検証する。

**段階2の退行修正 (2026-11-22、設計担当の指示に基づく追加実装・検証待ち)**

- `af98859b9` の新規20件・既存158件の成功は設計担当から報告済み。追加の読み取り監査で、明示 open が履歴スクロールの適用と操作結果の自動チェックまで省く差を発見した。設計担当は、どちらも選択所有の修正に必須ではない副作用として除去すると決定した。
- 明示対象は引き続きカーソルを所有するが、履歴のスクロールは従来の復元設定に従い適用する。そこから既存の reveal／行アンカーだけで要求画像を見せる。新しい配置規則・時間窓・待機は作らない。
- 操作結果の判定・チェック付与・適用記録を `apply_post_operation_checks` に分ける。明示 open があればチェックだけを適用し、なければ従来の `apply_post_operation_selection` が操作結果へカーソルも移す。単枚ではチェックせず、複数枚はチェックする既存規則を維持する。
- `target/browser818-repro/repro_tests.rs` の修正前 characterization と同じ activation → 実際に作られた hydration owner の待機 → F12 連動窓 → sync → close を、修正後の期待値で一本通しにしたテストを追加。通常全画面、履歴位置で可視／範囲外、行アンカー、操作結果との重なり、明示 open がない操作、単枚の各テストも追加する。filter は `browser818_selection`。今回も cargo は最後の `cargo fmt` のみで、ビルド・テストは設計担当へ引き継ぐ。

### 段階3：窓の用途、終了方針、F12 の共通拒否

**範囲**

- `src/app/detached_window_manager.rs:225`：`DetachedWindowRuntime`
- 同 `256`：生成
- `src/app/close_return.rs:5`：`ViewerCloseCause`
- `src/app.rs:3548`：runtime 除去・位置保存
- 同 `65387`、`65557`：session close
- 同 `66035`：context close
- 同 `66114`：全 viewer 終了
- 同 `67906`、`67941`：focus・finalize
- 同 `83366`：Esc 等の共通終了
- 同 `94459`：`toggle_detached_viewer_mode`
- `src/app/viewport_retirement.rs:32`
- `src/ui_fullscreen.rs:24786`、`24825`
- `src/app/viewer_context_registry.rs:6262`：context 退役
- `src/ui_dialogs/preferences.rs:3162`
- `src/remote_ipc/ui.rs:1211`

新規型：

- `DetachedWindowPurpose::{Normal, BrowserView}`
- `DetachedCloseOwner`：window identity、用途、型付き終了理由。
- 終了後に適用する位置保存・focus の値。

**終了の持たせ方**

受理した真の close の入口で、runtime が存在する間に `DetachedCloseOwner` を作ります。以後、teardown と最終処理へ値で渡します。

active renderer 内で早期 teardown が起きる経路も、`render_fullscreen_viewport` より前の所有値を終了処理へ渡せる形にします。`finalize_closed_active_detached_viewport` が削除後の runtime を探して方針を決める作りはやめます。passive の一括終了も窓ごとに捕捉します。

`ViewerCloseCause` の `RemoteAcquire`・`AppExit` 等は維持し、ブラウザ置換と F12 の理由を区別します。診断文字列の比較で動作を決めません。

| 用途・理由 | 位置保存 | メインへ focus |
|---|---|---|
| Normal・利用者／F12 | 既存どおり | 既存どおり |
| Normal・ブラウザ窓への置換 | 保存 | 送らない |
| BrowserView・利用者／ブラウザ置換 | 保存しない | 送らない |
| BrowserView・設定変更／Remote／終了等 | 保存しない | 既存の理由の方針 |

用途は生成時だけ設定し、park・resume・F11 で変えません。

**F12**

`toggle_detached_viewer_mode` の入口で、**切替対象の window lease から用途を解決**し、BrowserView なら設定を反転する前に拒否します。

知らせは既存の `show_feedback_warning`。設計書の文面を基本とし、再割り当て時の案内は実効 `ToggleDetachedViewerMode` のキー表示と整合させます。メニューの有効状態・説明も同じ判定を使います。独自の raw F12 判定は追加しません。

**テスト**

新規 filter：`browser818_close_policy`、`browser818_toggle`

- 上表の全行。
- Esc・×・passive close・設定変更・Remote・アプリ終了。
- runtime が早期に消える場合も用途を保持する。
- BrowserView の F11 往復で通常の保存位置を汚さない。
- main、viewer、再割り当て、ゲームパッドの同じ操作が共通入口で拒否される。
- sibling の終了・focus・placement を変更しない。

既存の `f12_round_trip_recreates_detached_host_with_borderless_mode_and_restore_placement`、`always_new_media_f12_round_trip_preserves_borderless_presentation_intent`、detached manager／placement テストを維持します。

**完了条件**

新しい BrowserView がなくても、Normal の終了・F11・F12 が従来どおりで、方針が用途と終了理由だけから決まること。

### 段階4：BrowserOverlay の表示先・seed・前面化

**範囲**

- `src/app.rs:71382`：要求する表示先
- 同 `72467`：表示先の解決
- 同 `72540`：presentation open
- 同 `65776`：window ID 確保
- 同 `68166`：loading context
- 同 `70574`：materialized context
- 同 `72078`：一覧との連動
- 同 `72303`：既存窓の再利用・park
- `src/app/detached_placement.rs:73`、`123`
- `src/app/startup_ops.rs:303`、`357`、`375`、`925`
- `src/window_activation.rs:23`
- `src/tray_integration.rs:383`
- `src/app/depth_view.rs:1022`

**表示先**

- フル機能：既存の連動窓だけを、型付きの「ブラウザ置換」で閉じる。要求位置の新しい linked／BrowserView を作る。
- 複数ウィンドウ：通常の park／fork／loading の規則で新しい独立窓を作り、前の窓を残す。
- 保存設定 `detached_viewer_enabled` を一時的に変更しない。
- 独立窓の編集制限を解除しない。
- 新しい seed の引数は placement と用途を一組にし、通常入口は Normal と既存 seed を渡す。

F12 設定 OFF でも BrowserView の連動窓である間は、実 session・runtime の linked 所有に基づいて画像送りと一覧同期を維持します。初回要求の意図を消費した後に、設定だけを見て main 表示へ戻ることを防ぎます。

**受付結果の前面化**

| 結果 | 接続する場所・動作 |
|---|---|
| 受理 | 新しい窓の既存 visibility／focus 要求。配置完了後にその窓を表示する |
| 文書モーダル保留 | `poll_activation_open_paths` で channel に残す。前面化しない |
| 現在の Remote 所有による拒否 | `reject_activation_open_paths_for_remote` で BrowserOverlay の有無を保持し、main を前面化 |
| 受信時の Remote 所有・世代による拒否 | request を絞る時点で同じ方針を適用 |
| 構成エディタによる拒否 | drain 時に意図を数え、既存通知と main 前面化 |
| 解決後の★固定による拒否 | `StartupOpenPathOwner` の意図から main 前面化 |
| SidecarRestore による破棄 | ログのみ |
| 最新要求への置換 | 古い要求からは何もしない |

現在の拒否処理は件数だけへ畳んでいる箇所があるため、その前に BrowserOverlay の有無を失わない形に直します。

**トレイ復帰**

`sync_after_restore` は保持中のメディア面の復帰も行います。そのまま呼ぶと BrowserOverlay 以外の窓を前に出す余地があります。

既存の復帰 owner に「通常の前面復帰／BrowserOverlay の復帰」という型付き方針を渡します。**可視性の復帰は方針によらず既存どおり全部行います** (root、保持中の viewport の復帰 [tray_integration.rs:389](../src/tray_integration.rs:389)、全 context の native presenter の可視性 [app.rs:67130](../src/app.rs:67130))。方針で変えるのは focus / raise だけで、BrowserOverlay では保持中の viewport を既存の `focus=false` の経路 ([tray_integration.rs:98](../src/tray_integration.rs:98)) で戻し、最終の focus を新しい窓に限定します。`window_visible` を直接反転するだけの処理や、別の復帰フラグは作りません (独立レビュー第 12 回 P2 で、root だけを戻す当初案から改めた)。

**テスト**

新規 filter：`browser818_presentation`、`browser818_admission`

- F12 ON／OFF、複数ウィンドウ、既存 linked／独立／ParkedLive。
- linked、loading、materialized の全生成経路で seed と用途が一致。
- 無効・画面外 seed は既存と同じ扱い。
- 上の受付結果ごとの focus 出力。
- tray／最小化からの受理で root・旧メディアを focus しない。
- 置換で旧3Dを終了し、新3D・Uは自動開始しない。
- main と sibling の資源・要求世代が意図せず失効しない。

**完了条件**

注入した型付き要求で、モード別の窓、位置、終了、前面化が一通り成立すること。実機の z-order は別途未確認として残します。

### 段階5：新しい pipe と launcher／core の引数

**範囲**

- 新規軽量 crate `crates/browser-view-ipc/`
- 新規 `crates/launcher/src/browser_view.rs`
- `crates/launcher/src/main.rs:192`、`644`、`690`
- `crates/launcher/Cargo.toml`
- `src/single_instance.rs:199`、`488`、`629`、`656`、`683`
- `src/lib.rs:1344`、`1448`、`2181`、`2456`
- `src/single_instance.rs` の process names／listener 終了処理

**通信**

`\\.\pipe\mImageViewerBrowserView_v1` を使用します。共有 crate は wire 型・版・長さ制限・CLI の位置指定形式を持ち、本体の Settings や egui に依存させません。

- `--browser-view <位置指定> -- <path>` を launcher と core で同じ規則で読む。
- 位置指定が通常の開くパスとして解釈されないよう、既存 option-value の解析も更新。
- 新しい pipe は版照合と配送結果を返せる双方向にする。
- 応答は「activation channel へ渡した」まで。App の受理・表示結果ではない。
- listener が有効な要求を受けた直後に `ActivationOpenPath::received` を呼び、Remote snapshot と受信順を固定。
- 既存 activation channel の最新一件・保留を共有する。

launcher は接続した pipe の server PID を `GetNamedPipeServerProcessId` で取得し、配送前に `AllowSetForegroundWindow(pid)` を呼びます。MCP の `crates/mcp-server/src/pipe.rs:240` と同じ権限移譲です。host→launcher→core の子プロセス経路でも、許可の引き継ぎを明示的に扱い、失敗はログに残します。

既存 mutex があるのに pipe がない／版が違う場合は、再起動を促して終了します。旧 pipe・activation event・別 core 起動への切替はしません。core の同時起動競合も同じ処理へ合流させます。

**テスト**

新規 filter：`browser818_ipc`、`browser818_cli`

- Unicode path、`--`、欠損引数、不正矩形、過長 frame、未知版。
- 起動済み／未起動／mutex 判定後の競合。
- 古い core・pipe 不在で旧配送・二重起動へ進まない。
- server PID を取得した後、配送前に権限を渡す。
- activation event を立てない。
- listener の停止で読み待ちが残らない。

Win32 操作は差し替え可能にし、テストから desktop を変更しません。

**完了条件**

launcher、core の双方で同じ要求を配送でき、版不一致を launcher が検出できること。

### 段階6：中継 exe と同梱経路

**範囲**

新規 crate `crates/browser-host/`、package／exe は `mimageviewer-browser-host`。

- `src/lib.rs`：検査・保存・起動の処理と差し替え可能な I/O 境界。
- `src/main.rs`：native messaging の一メッセージ処理。
- `protocol.rs`、`positions.rs`、`image_bytes.rs`。

既存：

- `Cargo.toml`、`Cargo.lock`
- `crates/launcher/build.rs:48`、`164`
- `crates/launcher/src/main.rs:43`、`66`、`346`、`513`
- `src/runtime_cleanup.rs:309`、`665`
- `scripts/build-release.ps1:140`、`406`、`430`
- `scripts/build-release.sh:144`
- `scripts/build-dev.ps1:289`
- `scripts/build-dist.ps1:115`、`192`、`249`
- `scripts/build-portable.ps1:314`、`425`、`487`、`588`
- `scripts/test-full.ps1:69`
- `scripts/test-release-build-safety.ps1`
- `scripts/test-build-portable-source-fingerprint.ps1`
- `installer/mimageviewer.iss:52`

**host**

- origin は指定の Chrome／Edge ID の二つだけ。
- `v=1`、`kind=open_bytes` のみ。試作の download・診断メッセージは持たない。
- frame 上限は本文確保前に検査。
- base64・画像 signature・三つの HTTP(S) URL・有限な座標・正の寸法を検査。
- MIME から拡張子を決めない。再エンコードしない。
- 共有部品で月フォルダ・名前・MOTW を作る。
- 保存後の MOTW／launcher 起動失敗では画像を消さない。
- launcher は `Command::arg` で起動し、シェルを通さない。
- stdout は native response のみ。ログに base64 本文を出さない。
- 応答は「受け渡しまで成功」と保存パス。App の表示成功とは表現しない。

位置計算は右クリック差分と8 DIP の推定だけにします。試作の DWM／窓全体／比較用別解は製品経路に残しません。

**同梱**

release は、既存の core→remote→EPUB→MCP→editor の後、browser-host を作り、**内包対象の署名後に launcher を作る**順にします。出力先照合・PE 検査・clean 対象・署名対象も追加します。

- installer は launcher 内包によって同梱。インストール時登録はしない。
- portable ZIP にも companion として同梱するが、連携登録は無効。
- `test-full.ps1` の launcher build 前提は、現在の5本から browser-host を含む6本になる。
- smoke の source fingerprint／成果物 manifest にも host の hash を入れる。
- `runtime_cleanup.rs` は process image のディレクトリ所属で保護しており、exe 名追加のためだけに製品ロジックを変える必要はない。host を加えた回帰で確認する。
- 登録先の `browser-host/<世代>` は runtime cleanup の対象へ混ぜない。

**テスト**

新規 filter：`browser818_host`、`browser818_positions`、`browser818_bundle`

- 入力検査、短い読み書き、サイズ境界、元バイト列の保存。
- `final_url` が MOTW に使われる。
- 保存後失敗でもファイルを保持。
- 通常／負座標／zoom／DPR／記録なし・変化あり。
- launcher の内包 hash と展開結果。
- release・portable・署名・clean のスクリプト回帰。

**完了条件**

単体 exe、installer、portable、テスト前提がそろい、host が本体の巨大な依存を引き込まないこと。

### 段階7：連携登録、保存先、設定画面、場所▼

**範囲**

新規：

- `src/browser_integration/`：登録 backend、保存先解決、単一 worker owner。
- `src/ui_dialogs/preferences/browser_integration.rs`
- `BrowserIntegrationState`：利用不可／準備中／利用可能／処理中の型付き状態。

既存：

- `src/settings.rs:5586`、`8377`
- `src/ui_dialogs/preferences.rs:390`、`854`、`1252`、`3372`、`4474`
- `src/ui_dialogs/preferences/search_index.rs:531`
- `src/app/update_phases.rs:1931` 周辺の root maintenance
- `src/known_folders.rs:57`
- `src/ui_main.rs:13437`、`16660`、`16791`
- `src/agent_registration.rs:47`、`src/ui_dialogs/agent_integration.rs:114` を worker 構成の参考にする。

**画面**

「起動と連携」に、クリップボード取り込みと並ぶ「ブラウザ連携」を追加します。

- 「ブラウザ連携を使う」
- 登録状態・解除
- 保存先の変更／既定に戻す／フォルダを開く
- 既定 profile 限定と★固定の注意
- launcher が不明な場合の無効理由

Settings は新規の `browser_integration_enabled=false` と `browser_view_output_dir=None`。保存先の正本は Settings、host 隣の設定は登録時に作る投影です。

**worker**

UI は設定・環境のスナップショットを渡して非blocking poll するだけにします。既知フォルダ照会、exe の確認・コピー、hash、manifest、registry、フォルダ操作は worker です。

登録・解除・保存先更新は一件ずつ行い、処理中の同じ設定操作を受け付けません。設定画面を閉じても root の owner が完了を回収します。再試行・rollback・複数更新の待ち行列は設けません。

**登録の配置**

```text
%APPDATA%\mimageviewer\browser-host\
  <host名>.json                 ← registry が指す固定 manifest
  <世代>\
    mimageviewer-browser-host.exe
    config.json                ← launcher絶対パス・保存先基底・版
```

新しい世代を完成させ、manifest を同一ディレクトリの一時ファイルから置換します。既存 registry は固定 manifest を指し続けます。

登録キーは次の二つの既定値です。

- `HKCU\Software\Google\Chrome\NativeMessagingHosts\<host名>`
- `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\<host名>`

古い世代の整理では、実行中 host の設定を先に削除しないようにします。自分の exe の削除を先に試み、使用中ならその世代全体を残す簡単な方式を提案します。待機・kill・再試行はしません。削除対象は登録 owner の世代フォルダだけとし、reparse point をたどりません。

portable・明示 `--data-dir`・launcher 不明の起動は、既存登録も変更しません。core 直接起動で MCP の隣接 exe 探索を使う作りは、ブラウザ連携へ流用しません。

**保存先と場所▼**

既定は、worker で解決した `ピクチャ\mimageviewer\browser`。月フォルダは最初の保存時に host が作ります。

場所▼は、クリップボード取り込みの直後に「ブラウザ閲覧」を出します。本棚より後、整理先・通常の場所より前。末尾へ出す既存 fallback も含めて両箇所を更新します。メイン専用とし、Remote の共有候補へ追加しません。描画中の存在確認は行いません。

**テスト**

新規 filter：`browser818_registration`、`browser818_preferences`、`browser818_location`

- 新規設定の旧設定読込・round trip。
- default／portable／明示 data-dir／launcher 不明。
- 世代完成前に manifest が公開されない。
- 保存先だけ変わる更新、起動時更新、解除。
- 使用中世代を整理するとき設定を消さない。
- registry・環境・ファイル操作は注入し、実登録を触らない。
- UI を閉じた後の完了回収、連続操作の拒否。
- 場所▼の表示条件・順序・Remote 非露出。
- 明暗・長いパス・利用不可・処理中の snapshot。
- パス入力を IME helper 経由にし、typing inventory に設定欄として登録。

**完了条件**

登録が UI を止めず、既定 profile だけが登録の持ち主になり、保存先変更と host 設定が同じ操作として確定すること。

### 段階8：製品の拡張とストア別 ZIP

**範囲**

新規 `browser-extension/`：

- `manifest.json`
- `worker.js`
- `context.js`
- `_locales/ja/messages.json`
- `_locales/en/messages.json`
- icons
- `tests/worker.test.cjs`

新規スクリプト：

- `scripts/package-browser-extension.ps1`
- ZIP・manifest を検査するテストスクリプト。

**製品仕様**

MV3、ソースは一つ。権限は設計書 §9.6 の宣言する 6 つ (`contextMenus`・`nativeMessaging`・`storage`・`activeTab`・`scripting`・`declarativeNetRequestWithHostAccess`) と、任意の HTTP(S) host 権限だけにします。`downloads` と `declarativeNetRequestFeedback` は入れません。

- 同じ worker 内の連続クリックを同期の受付 latch で防ぎ、要求を `storage.session` に保持。
- `permissions.request` は user gesture 内、最初の await より前。
- 画像とページの origin をまとめて求め、取得継続の条件は設計書どおり。
- 取得は一回、`credentials: include`、`cache: no-store`。
- DNR は最初の URL 完全一致・`TAB_ID_NONE`・一回限り。
- `response.url` を `final_url` として必ず送る。
- Content-Length、読み取り中、最終 JSON の UTF-8 バイト長の三段階で上限を検査。
- worker 再起動では自分の残存規則を除去し、中断を一回通知。再取得・再送しない。
- 処理中、許可拒否、未登録、版不一致、保存・起動失敗を ja/en の badge／title で知らせる。

`context.js` は上位フレームの座標と寸法だけを記録します。試作の `target_src` や要素の内容は読みません。登録・既存タブ注入は `onInstalled`、`onStartup`、`permissions.onAdded` を通し、二重 listener を防ぎます。

**key と ZIP**

- ソース manifest の key：ClaudeCode 提供の Chrome 公開鍵。
- Chrome ZIP：Chrome key。
- Edge ZIP：生成物の manifest だけ Edge key に差し替え。
- 両方とも公開鍵から計算した ID と指定 ID の一致を検査。
- ソースを書き換えず、試作・テスト・共有メモを ZIP に含めない。
- スクリプトは ASCII の PowerShell。

**テスト**

`node --test browser-extension/tests/worker.test.cjs`

- 連打、worker 再起動、各終端での busy 解放。
- 一括許可拒否／画像だけ許可済み／同一 origin。
- DNR の追加・削除、HTTP エラー、redirect、`final_url`。
- JSON 全体のサイズ境界。
- 記録あり／なし／窓移動・サイズ・DPR 変更。
- 既存タブへの再注入、二重登録。
- native response と `runtime.lastError`。
- 両 ZIP の key・ID・権限・locales・収録内容。

モックテストを、Chrome／Edge の実動作や座標精度の証拠にはしません。

**完了条件**

製品用 manifest と二つの ZIP を再現可能に生成でき、試作専用コード・権限・診断本文を含まないこと。

### 段階9：公開文書と最終検証の引き渡し

**範囲**

- `docs/spec.md`
- `docs/architecture-overview.md`
- `docs/async-architecture.md`
- `docs/ui-responsiveness.md`
- `docs/keymap-spec.md`
- `docs/key-customization-impl-plan.md`
- `docs/browser-to-miv-plan.md`
- `docs/detached-rework-plan.md` §11
- 開発ビルド・テスト文書、`CLAUDE.md` の同梱本数・ビルド順
- `docs/release-checklist.md`
- 日本語・英語マニュアルの新規ブラウザ連携ページ、目次・設定案内
- `htdocs/mimageviewer/privacy.html`
- 製品ページ日本語・英語
- 配布 readme

**記載内容**

設計書 §11 に従い、一回の画像取得、Cookie、常に送るページ origin、redirect、座標記録、PC 内への受け渡し、MOTW、保存後失敗時の残存、登録・解除を両言語で説明します。

窓のモード差、メイン一覧の移動、F12 制限、F11、位置の限界、★固定、ページ個別補正と U の書込先の違いも記載します。閉じた後のブラウザへの復帰は保証しません。

外部ダウンローダ名、特定サイトを宣伝に使う表現、公開文書の内部実装用語は入れません。ストア提出・公開は ClaudeCode／利用者の公開工程へ引き渡します。

**完了条件**

対応する自動検証・独立レビュー・必要な利用者実機確認を区別して記録し、未確認事項を成功扱いせず引き渡すこと。

## 3. detached 憲法 §2 との照合

以下は全段階共通で、該当する変更を段階0の合意記録へ対応付けます。

| 項 | 今回の置き場所・制約 |
|---|---|
| **2-1：rect 捕捉へ条件追加しない** | HWND は既存の backend allocation／window lease を使う。拡張の矩形は placement の入力だけ。窓の同定には使わない |
| **2-2：geometry から再生成しない** | 再生成は受理済み BrowserOverlay による明示的な置換だけ。host_lost・位置不一致を契機にしない |
| **2-3：App に detached bool／Option を追加しない** | 意図は要求・continuation、用途は `DetachedWindowRuntime`、終了方針は `DetachedCloseOwner`。登録 worker は process-global の連携サービスで、viewer の状態にはしない |
| **2-4：placement の保存・同期経路を増やさない** | 生成時の seed と既存 runtime placement を使う。通常位置の保存先は既存 Settings のまま。BrowserView は保存対象から外す |
| **2-5：時間窓で競合を吸収しない** | 要求 ID、context、世代、item identity、window lease で判断。試作の時刻期限を撤去。右クリック記録の利用も時刻ではなく寸法・位置・DPR の一致で決める |
| **2-6：実機症状へヒューリスティックを足さない** | 所有の問題は BA-7、配置は BA-6、同定なら BA-1 として報告。zoom／DPI の未確認を補正定数で埋めない |
| **2-7：範囲外を直さない** | 各 brief で上記ファイルを固定。R4、メディア表示、編集機能、設定DB全体の整理は行わない |
| **2-8：既存テストを弱めない** | 新規回帰を追加する。既存 F11／F12／park／multiwindow テストの期待値緩和・削除はしない |

選択の修正は、復元側が持たない明示選択の権限を要求 owner へ戻すものです。窓交換は、配置を既存 Normal の保存値へ混ぜないための明示 lifecycle です。どちらも guard・delay・追加 repaint による症状抑止ではありません。

## 4. 設計書との差・未決事項

### 実装へ反映すべきコード上の補足

1. **複数ウィンドウは loading seed だけでは不足する**  
   materialized fork の経路にも seed・用途を渡します。既存の高速経路を保つ修正で、仕様変更は不要です。

2. **BrowserView の初回確定後にも、F12 OFF を扱う必要がある**  
   現在の `sync_detached_viewer_to_selected` は設定 OFF で戻ります。初回要求の分岐だけでは連動を維持できません。runtime／session の linked 所有を使う範囲を段階4に含めます。

3. **トレイ復帰は可視性だけの操作ではない**  
   `sync_after_restore` に既存メディア面の復帰があります。BrowserOverlay の focus 方針を、復帰 owner に渡す必要があります。可視性の復帰はそのまま全部行い、focus / raise だけを方針で分けます (段階 4)。

4. **版照合には transport 応答が必要**  
   新 pipe を一方向の書き捨てにすると、launcher は版不一致を判断できません。版照合と channel 配送までの応答を設けます。App の受付結果は返しません。

### 設計担当と独立レビューで決める事項

- **アンインストール時の扱い**  
  §9.7 の保留は残っています。現在の installer は `installer/mimageviewer.iss:100` 以降で、利用者が選ぶと APPDATA 全体を削除しますが、native host の registry は消しません。  
  推奨案は、アンインストールで当該 manifest を指す自分のキーと host 配置だけを解除し、拡張と保存画像は残す形です。ただし昇格時の利用者 HKCU、および保存先を APPDATA 配下に指定した場合の画像保護を決める必要があります。これは段階7／9の完了前に確定し、既存の再帰削除へ安易に追加しません。

- **Q4 の画像リンク優先**  
  試作 worker は `srcUrl` を使っており、Q4 の実機結果は §9.11 にありません。製品案は、既存 HTML 取り込みと同じ画像拡張子判定で HTTP(S) の `linkUrl` を優先し、選んだ取得先とページの origin を許可要求へ含める形です。失敗時に `srcUrl` を取り直すと「一回取得」を変えるため、fallback は追加しません。対象形式の集合とあわせ、段階8前にレビューで固定します。

- **公開鍵と native host 名**  
  Chrome／Edge 公開鍵は ClaudeCode から受領します。ID は指定の二つに固定。native host 名は試作名を使わず、一つの製品名をコード・manifest・registry・説明書で共有します。

- **拡張の受付と user gesture の順序**  
  `storage.session` の非同期読込より先に `permissions.request` が必要です。同期 latch と、起動時の残存要求処理、session への記録を組み合わせる順序を段階8の状態遷移として明示します。単に storage の read→write だけで排他し、連打を通してしまう実装にはしません。

### 実装後も実機確認が必要な事項

最大化、zoom 100% 以外、混在 DPI、負座標、右サイドパネル、Edge、redirect、製品の launcher／pipe 経由の前面化、Esc・×後の前面窓です。試作での利用者の観測を、これらの検証済み証拠へ広げません。

## 5. 永続データと検証の担当

ブラウザ連携の設定・保存先・host 設定は未リリースです。既存 Settings へ既定値付き項目を追加し、新機能用の旧形式移行は作りません。

一方、clipboard 取り込みは `CHANGELOG.md:331` の v4.4.0 に記載された既存機能です。名前・MOTW の抽出は挙動維持を条件とし、既存画像、設定DB、補正DB、通常の detached 保存位置の形式は変えません。

検証実行は ClaudeCode が `run-heavy.ps1` 経由で担当し、上記の package／target／filter を brief に指定します。0件一致を成功扱いせず、同じソース・条件の成功台帳は再利用します。

段階2〜4は共有の所有・非同期・複数ウィンドウを変えるため、焦点テストだけで最終受入れにせず、本体 lib 全件の担当と実行時点を事前指定します。最終統合では `test-full.ps1`、`test-script` 付き lib 全件、fmt、UI glyph、サイトカード、拡張テストを通します。

今回は計画作成なので、検証用バイナリも作っていません。実装後の全体確認は launcher・内包 host を含むため、`build-release.ps1` の成果物を使います。利用者への起動コマンドは次です。

```powershell
Start-Process -FilePath .\target\release\mimageviewer.exe
```

この起動は通常の `%APPDATA%\mimageviewer` を使い、実設定・データを更新し得ます。利用者にはインストール版・トレイ常駐版を先に終了してもらいます。エージェント自身は通常 profile の製品を起動せず、ブラウザ登録を伴う実機確認は、具体的なシナリオとデータ範囲を定めた別の検証工程へ引き渡します。
## 6. 独立レビューの記録

### 第 12 回 (2026-11-22、GPT-6.1 Sol / xhigh、read-only、設計書のレビューと同じ担当を継続、対象 `a1dc9b237`) — REVISE

計画のコードの追跡 (通常の 2 重起動でも選択が戻る経路、連動窓の種の差し込み口と複数ウィンドウの 2 経路、Esc で runtime が先に消えること、core の起動競合の経路) は妥当とされた。段階の順序・コミットの単位も成立。§4 の補足 (materialized の経路、F12 OFF での連動の維持、transport の応答) は設計書の範囲内の具体化。§0 の Q4・host 名・受付の順序も妥当。

凍結の決まりの構造的変更の合意: 段階 2 = 合意、段階 3 = 合意、段階 4 = 不合意 (トレイ復帰で root だけを戻す部分が、前面化を避けるために既存の可視性の復帰まで抑えている)、段階 5 = 合意。

| # | 重大度 | 指摘 | 対応 |
| --- | --- | --- | --- |
| 1 | P1 | `browser-host` を無条件に消すと、そこを保存先にした画像を巻き込む | §0: 保存先の基底を `browser-host\` の下にできない不変条件を足し、3 か所で同じ判定を使う。回帰テストを段階 1・6・7 に |
| 2 | P2 | トレイ復帰を root だけにすると、保持中の窓の可視性の復帰まで失う | 段階 4: 可視性の復帰は全部既存どおり行い、focus / raise だけを方針で分ける (保持中の viewport は既存の `focus=false` の経路) |
| 3 | P2 | 別の管理者で昇格したときに残るものの説明が不正確 (HKCU も `{userappdata}` も実行アカウントを指す) | §0: 自動で消せるのは実行したアカウントのものだけと定義し、残るもの・拡張の知らせ・「アンインストールの前に解除する」案内を書いた |
| 4 | P3 | 拡張の権限の数 (設計書は宣言 6 つ + 任意の host 権限) | 段階 8 を訂正 |

### 第 13 回 (2026-11-22、同じ担当を継続、対象 `b5387eb55`) — ACCEPT

第 12 回の 4 件はすべて解消し、新しい指摘は無し。段階 4 も構造的変更として合意 (段階 2〜5 すべて合意)。合意は [detached-rework-plan.md](detached-rework-plan.md) §11 に記録した。

### 段階 1 の実装 (2026-11-22、`0d6ade306`・`5982ed756`) — ACCEPT

1 回目は REVISE (P1: まだ無いフォルダの `..` の先がジャンクションのとき、管理フォルダの外と誤判定する)。`absolute` の後に `.` / `..` が残るパスはエラーにして解消。

### 段階 2 の実装 (2026-11-22、`af98859b9`・`8b9e480fe`) — ACCEPT

統括の指示で、直す前のコード `5982ed756` に再現テストだけを入れた別の worktree で流し、F12 の連動窓での誤り (履歴の復元が選択を前回の画像へ戻し、同期で開き直す) を再現した。通常の全画面では閉じると選択は要求の画像へ戻る。ふだんの起動 (ダブルクリック・関連付け・送る・2 重起動・Agent) の変化を洗い出し、退行 2 つ (履歴のスクロール、保留中のファイル操作の自動チェック) を `8b9e480fe` で戻した。関連テスト 200 件成功。独立レビューで指摘なし、凍結の決まりの構造的変更 (実装) に合意。本体 lib の全件は段階 4 の後。
