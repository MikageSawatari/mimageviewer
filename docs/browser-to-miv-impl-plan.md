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
- **Q4 (画像を包むリンク先が画像ファイルのとき)**: HTTP(S) の `linkUrl` が既存の HTML 取り込みと同じ画像拡張子の判定に当たるときは、リンク先を先に取得する。**0.2.1 (2026-11-23)**: 利用者が Commons の説明ページで NG を報告したため、リンク先は `Content-Type: image/*` のときだけ使い、それ以外はページの画像 (`srcUrl`) を開いて `success_fallback` で知らせる。取得自体の失敗では取り直さない。リンク先・ページの画像・ページの origin をまとめて許可要求に含める。
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

**段階3の実装記録 (2026-11-22、ビルド・テスト・独立レビューは引き継ぎ待ち)**

- 用途は `DetachedWindowRuntime` の private field と生成専用 API に置いた。既存の暗黙生成は `Normal`。既存 runtime の park・resume・host 再登録・F11 は用途を変更しない。`BrowserView` の製品側 producer はまだ追加していない。
- `app/detached_close.rs` の `DetachedCloseOwner` が window lease・用途・`ViewerCloseCause` を保持し、同じ場所の純粋な表で保存・focus の方針を決める。Esc／OS close／context close／passive 一括終了／設定・Remote・終了の一括 close で、削除前に窓ごとの値を捕捉する。active renderer も入力・描画前の値を finalizer に渡す。finalizer は削除後の runtime から用途を推定しない。
- 位置保存は既存の runtime 除去の一か所だけで方針を適用する。focus は既存の idle 判定の直前で方針を適用する。設定変更・Remote・終了の一括 close にも既存の focus 経路があり、`BrowserView` でもそれを維持する。ブラウザ窓への置換では両用途とも focus を送らない。
- F12 は共通入口とメニューで同じ対象 lease の判定を使う。警告のキー名は実効 `ToggleDetachedViewerMode` から得る。native 動画の F12 にも型付き理由を運ぶため、範囲表に加えて `presentation_transition.rs` の既存 `CloseDetachedSession` effect と `native_video.rs` の消費側を更新した。effect の順序・待ち・host 操作は変更しない。
- `browser818_close_tests.rs` の 10 テストと、transition reducer の `browser818_close_policy_native_retirement_carries_typed_f12_reason` を追加した。GPU を生成しない CPU／egui／fake host のテスト。既存のテストは削除・弱化せず、finalizer の引数と close seam の source 検査を新しい所有値に対応させた。filter は `browser818_close_policy|browser818_toggle`。`Normal` の保存・focus・F11 往復・F12 往復も同じテストで検査する。
- App の detached 用 field・時間窓・placement の保存先・永続設定の変更はない。cargo は最後の `cargo fmt` 以外実行せず、コンパイル・テスト・UI glyph 検査は設計担当へ引き継ぐ。

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

**段階4の実装記録 (2026-11-22、ビルド・テスト・独立レビュー待ち)**

- `browser_presentation.rs` の `DetachedWindowSeed` が用途と任意の位置を一組で渡す。linked は window ID 確保直後、loading は flags／通常 seed 設定前、materialized は fork 先の新しい binding で生成値を適用する。通常の呼び口を残し、Normal と既存 seed を渡す。不適切な位置は既存の検査と設定済み位置／既定位置への扱いを使う。
- フル機能では受理済みの main-new open の境界で既存 linked を `BrowserReplacement` で終了し、新しい BrowserView を作る。独立した既存窓は通常の park／ParkedLive handoff を通す。複数ウィンドウの通常画像の activation は従来どおりメインの親フォルダ復元後に materialized fork を通す。loading は既存の入口を種付きの共通入口へ集約し、BrowserView の種も受け取れる形にした。loading の BrowserView はその共通入口へ注入して検査する。隠された拡張子の DirectMedia は既存の復元経路を維持する。
- F12 OFF でも BrowserView＋linked の実 runtime／binding に基づいて画像送りと一覧同期を保つ。保存設定は変更しない。新しい BrowserView は F11 を継承せず枠付きで生成する。独立窓の編集制限は既存のまま。
- Remote の両拒否・構成エディタの drain は、件数へ畳む前に拒否された BrowserOverlay の意図を保持する。★固定は解決 owner から同じ拒否時 main 前面化へ渡す。文書モーダル保留・SidecarRestore 破棄・旧要求の置換には前面化を足さない。
- `MainRestorePolicy` を既存の main 復帰と App の復帰 owner に渡す。通常は Activate の wrapper を通り、BrowserOverlay は PreserveForeground。root・全 retained viewport・全 context の native presenter の可視性を復帰し、旧メディアの focus／raise のみ省く。配置待ちの旧 show request も focus だけを退役させ、表示・最大化・placement の要求は残す。新しい窓だけが既存の配置完了後の show／focus を所有する。
- 非前面化での保存済み WINDOWPLACEMENT 復帰にも方針を渡すため、範囲表に加えて `tray.rs` の既存復帰 helper を更新した。通常の showCmd は変更しない。Win32 の実際の z-order・最小化／最大化からの非前面化復帰は実機未確認。
- `browser818_presentation_tests.rs` の CPU／egui テスト16件と、`depth_view_tests::gpu_tests` の3D回帰1件を追加した。後者は既存 nextest の gpu_tests グループ規則に該当する。既存テストの削除・期待値緩和はない。cargo は最後の fmt 以外実行せず、コンパイルと実行結果は設計担当へ引き継ぐ。
- App の detached 用 field、時間窓、位置の保存先、永続設定は追加していない。拡張・pipe・host の producer は段階5以後のまま。

段階4の選択引き継ぎ修正: 設計担当の182件実行で181件成功、linked置換の選択テスト1件が失敗した。実装が明示対象を選択してから旧窓を終了していたため、Manual close の `restore_grid_cursor_to_fullscreen_item` が旧画像へ戻していた。テストの期待は正しく、実装の順序違反である。BrowserOverlay の main-new に限り、既存の置換／終了を済ませてから明示選択を一度確定する順序へ修正した。通常要求の順序と利用者closeのカーソル復元は変更しない。元のassertを維持し、同期後の選択・表示・stamp・leaseを強化したほか、旧MainWindow／Fullscreen／BrowserViewの終了も検査する回帰を追加した。修正後のビルド・テストは引き継ぎ待ち。

**段階4の独立レビュー P1 修正 (2026-11-22、再検証・再レビュー待ち)**

- `b92b6e243` は設計担当から lib 全19,659件成功の報告があったが、独立レビューは REVISE、実装への構造合意は保留。隠した拡張子の要求で、新しい DirectMedia reader を構築した後に旧 linked 窓を閉じると、正しい close の元一覧復元が新 reader を破棄していた。テストの不足に加えて実装の順序違反であり、close の復元に guard を加えて避けない。
- DirectMedia の構築入口2か所（sidecar continuation／復元済み一覧の即時 action）を `install_direct_media_reader_for_open` に集約した。BrowserOverlay の新規要求だけは、対象パス・準備済みメタデータ・要求を値で保持して旧 reader の終了／park を済ませ、その後の一覧で対象を解決・構築する。MainWindow／Fullscreen／BrowserView からの置換も同じ境界を通す。Preferences の構築順序は変更しない。
- `ViewerOpenOrigin::MainPrepared` は旧 reader の引き継ぎを終えた新規要求を表す。要求 ID・意図・選択所有を維持して hydration／RequiredFullscreen の待ちを通し、表示先では新規 routing を保ちながら旧 reader を再終了しない。App のフラグ・待機時間窓・位置の保存先は増やさない。通常の DirectMedia 終了による元一覧復元は変更しない。
- `browser818_presentation_activation_hidden_image_replaces_each_previous_reader` は実際の activation 受付・resolver・フォルダ復元から表示・同期・close までを通す。旧 F12 linked／MainWindow／Fullscreen／BrowserView を検査し、隠した画像への再要求も検査する。`browser818_presentation_hidden_image_immediate_action_finishes_old_reader_before_install` は即時構築入口、`browser818_presentation_direct_media_handoff_retains_request_and_preferences_order` は要求相関・引き継ぎ済み状態・Preferences の順序を検査する。既存テストの削除・弱化はない。
- 今回の変更に対するビルド・テストは未実行（cargo は最後の fmt のみ）。上記の既存成功結果は今回の差分の検証には代用しない。実装の構造合意は再レビューで確認する。

P1 修正の再検証では設計担当から190件中189件成功・上記 activation 回帰1件失敗の報告があった。失敗した末尾の「全項目が PNG」は、viewer 終了直後に非同期の帰り先一覧復元まで完了したとみなしていたテスト側の誤り。`restore_direct_media_reader_origin` は snapshot を退役して `restore_view_return_origin` へ帰り先を渡し、通常の物理フォルダ履歴 owner が一覧を非同期復元する。`close_fullscreen_to_completion` の完了条件は viewer teardown であり、一覧復元の完了ではない。既存の `finish_history_restore_preparation_for_test` でその owner と hydration の完了を待ってから、元の PNG assert を維持し、帰り先フォルダ・元の2画像との完全一致・可視 index・fullscreen 終了も検査するように修正した。製品コードの変更はなく、このテスト修正の再実行は設計担当へ引き継ぐ。

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

**段階5の実装記録 (2026-11-22、ビルド・テスト・独立レビュー待ち)**

- `mimageviewer-browser-view-ipc` を workspace に追加した。Settings／egui／本体には依存せず、serde／serde_json と Windows の transport だけを持つ。wire は `u32` little-endian の本文長 + UTF-8 JSON、本文上限128 KiB。要求は `protocol_version=1`・workspace の `application_version`・絶対パス・物理配置を持つ。応答も両方の版を持ち、`Queued`／`VersionMismatch`／`InvalidRequest`／`ChannelClosed` を返す。Queued は activation channel への配送だけを意味し、App の受付・表示結果ではない。版の不一致は channel に入れず、client も応答の両方の版を確認する。
- CLI は共有 crate で `--browser-view left,top,right,bottom,maximized -- path` として読む。最大化は `0`／`1`、座標は i32、幅・高さは1〜65,536。画面との重なりは段階4の既存検査に任せる。Unicode の絶対パスを運び、NUL・空・UTF-16で32,767単位超を拒否する。core／launcher の既存 option-value 表にも switch を加え、位置指定を通常の path と誤解しない。`--` 以降の文字列は path として扱う。
- launcher は Browser 専用の mutex probe と terminal な配送分岐を持つ。起動中なら新 pipe に一度だけ接続し、server PID → `AllowSetForegroundWindow(pid)` → 要求送信 → 応答確認の順で処理する。pipe 不在・版不一致・他の配送失敗はログと「mIV を再起動してください」で終わり、旧 pipe・activation event・別 core 起動へ移らない。未起動は元の引数をそのまま core に渡し、spawn 後に child PID にも明示的に許可を渡す。許可失敗はログに残して配送を続ける。host→launcher の spawn と許可の呼び出し元は段階6であり、共有 crate に同じ子プロセス許可 helper を用意した。
- core が mutex 取得の競合に負けた場合も Browser は同じ共有配送処理で終端し、旧配送と activation event へ進まない。この分岐の診断は本体ログを truncate しない `browser-view.log` へ記録する。初回起動は CLI から既存 `ExplicitOpenPath` に同じ意図を渡し、既存 `InitialStartup` owner へ入る。通常要求の起動・2重起動の分岐は維持した。`--data-dir` 付き launcher は従来どおり core に名前空間の解決を任せる。
- `single_instance/browser_view.rs` の listener は、有効な wire 要求に対して直ちに `ActivationOpenPath::received` を呼び、BrowserOverlay の要求として既存 channel に送る。Remote／Agent と同じ現在ユーザー専用 DACL と remote-client 拒否を使う。browser pipe のみ portable と data-dir suffix を既存の mutex と揃え、既存4種類の名前・wire は変更しない。listener の owner は `ActivationListener` に置き、App の field は増やさない。
- connect／read／write は OVERLAPPED と永続 stop event を同じ待ちに含める。停止時は CancelIoEx 後の完了を待ってから buffer／OVERLAPPED を解放する。停止を `Interrupted` にせず `ConnectionAborted` にし、read_exact／write_all の自動再試行で停止を失わない。client の接続再試行はなく、read／write の I/O timeout は5秒（競合を隠す時間窓ではない）。Win32 の PID・許可・送受信は Connection trait、mutex 判定は注入可能な closure を通し、テストから desktop は操作しない。
- Cargo.lock は既存依存だけの新 package／参照を手で追加した。cargo は最後の fmt のみ。コンパイル・実行・実機結果は未取得であり、設計担当へ引き継ぐ。公開文書は段階9、host producer は段階6のまま。

追加テスト (Windows で計19件、いずれも GPU／desktop 操作なし):

- 新 crate (12件): `browser818_cli_unicode_delimiter_and_position_round_trip`、`browser818_cli_missing_arguments_and_invalid_rectangles_are_rejected`、`browser818_ipc_unicode_frame_queues_exactly_once_and_ack_is_channel_only`、`browser818_ipc_unknown_protocol_or_application_version_never_queues`、`browser818_ipc_invalid_request_never_queues`、`browser818_ipc_frame_limits_reject_before_reading_body_and_writing_header`、`browser818_ipc_server_pid_and_grant_precede_request_write`、`browser818_ipc_foreground_failure_logs_and_keeps_single_delivery`、`browser818_ipc_old_core_reply_is_terminal_without_retry`、`browser818_ipc_probe_then_core_mutex_race_routes_without_second_spawn`、`browser818_ipc_stop_interrupts_connect_and_partial_frame_read`、`browser818_ipc_stop_before_next_operation_cannot_be_missed`。
- launcher (2件): `browser818_ipc_launcher_routes_once_without_legacy_or_spawn_fallback`、`browser818_cli_launcher_keeps_literal_delimiter_and_skips_rectangle_value`。
- 本体 lib (5件): `browser818_cli_core_keeps_browser_intent_and_never_opens_rectangle_as_path`、`browser818_ipc_pipe_namespace_tracks_flavor_and_data_dir_without_changing_legacy_names`、`browser818_ipc_receive_and_initial_startup_keep_same_intent_and_distinct_request_ids`、`browser818_ipc_core_mutex_race_uses_terminal_browser_route`、`browser818_ipc_activation_captures_remote_ownership_before_channel_delivery`。

新規 filter は3 package の `browser818_ipc|browser818_cli`。通常の回帰は launcher の `tests::gpu_option`／`tests::gpu_options`／`mcp_launch::tests`、本体の `tests::gpu_startup_values_never_become_core_open_paths`／`tests::source_close_probe_values_never_become_startup_open_paths`／`single_instance::tests`／`browser818_open_owner` を用いる。既存テストは削除・弱化していない。

**段階5の独立レビュー P1 修正 (2026-11-22、再検証・再レビュー待ち)**

- `6c07dc523` の独立レビューは構造に合意、通信終端の P1 で REVISE。`serve` の応答 write の完了は client の読了を意味せず、直後の `DisconnectNamedPipe` が未読応答を破棄し、配送済みでも launcher が失敗と判断する競合があった。
- `Stream::serve_until_closed` で、応答書込後は `wait_for_client_close` の overlapped read で client の EOF／ERROR_BROKEN_PIPE／ERROR_PIPE_NOT_CONNECTED を待ってから切断する。start／completion の Win32 error code を保って分類する。終了待ちは INFINITE と永続 stop event を使い、期限・追加 ack・FlushFileBuffers は設けない。request read／reply write の既存 I/O timeout は変更しない。
- 旧 listener は1 instance・1 loop だったため、そこで EOF を待つだけでは居座る peer が次の受付を塞ぐ。transport の `Server` が接続済み instance を引き渡す前に次の待受 instance を確保し、接続ごとの worker に reply と EOF の寿命を持たせた。FIRST_PIPE_INSTANCE は最初だけ、後続も同じ DACL／PIPE_UNLIMITED_INSTANCES を使い、待受名の所有を切らさない。完了 worker は順次回収し、停止と accept／spawn エラーでは共通 stop を signal して全 worker を join する。App の field は増やさない。
- `single_instance/browser_view.rs` の listener はこの `Server::run` に既存の activation callback とログを渡す。wire・CLI・launcher／core の client・既存の open-path／activation event は変更しない。
- 実 pipe の回帰を3件追加した。`browser818_ipc_delayed_reply_read_keeps_complete_response_and_accepts_other_peers` は PeekNamedPipe で応答が書かれたことだけを確認し、未読応答を保持したまま別 peer の要求を完了してから元の応答を検査する。Queued と VersionMismatch を検査し、client close 後の次の接続も通す。`browser818_ipc_client_close_releases_reply_wait_without_stop` は stop 無しの client close が終端を解放すること、`browser818_ipc_stop_joins_reply_waiters_with_clients_still_open` は client を開いたまま stop で全 peer と accept が終了することを検査する。desktop／前面化の操作はしない。
- 共有 crate の新旧テスト filter は `browser818_ipc|browser818_cli`。本体は `single_instance::browser_view::tests|single_instance::tests`、launcher は既存の `browser818_ipc|browser818_cli` を引き継ぐ。今回のビルド・テストは未実行、cargo は最後の fmt のみ。再実行と P1 の再レビューは設計担当へ引き継ぐ。

**段階5の独立レビュー P2 修正 (2026-11-22、再検証・再レビュー待ち)**

- `fb20fbbe5` の独立レビューで応答終端の P1 は解消・構造に合意。同時に保持する peer の資源量が無制限だったため P2 で REVISE。設計担当の指定どおり、EOF 待ちを含む保持接続を固定4件に制限する。設定にはせず、満杯ではその接続に応答せず切断・ログを残し、listener は次の接続を受け付け続ける。client 側は既存の失敗・再起動案内に合流し、再試行や待ち行列は加えない。
- transport の `PeerWorkers` が JoinHandle の集合を容量と寿命の正本として持つ。接続の受付後に終了済み worker を join・除去してから容量を判定するため、accept 待ち中に閉じた peer の枠も回収できる。別の permit／カウンターは足さない。Drop は共通 stop を signal して全 worker を join し、accept／spawn のエラーでも同じ所有者が後始末する。保持 worker は最大4で、満杯時は待受 instance を切断・再利用し、新しい instance・worker・reader・activation 要求を作らない。受理時だけ次の待受を確保して接続を引き渡すので、server が保持する pipe instance も4 peer と待受1本の範囲に収まる。
- `browser818_ipc_peer_limit_rejects_without_reply_and_keeps_listener_alive` を追加し、実 pipe で4接続を保持したまま追加接続を繰り返し拒否し、応答が1 byte も無いことと listener の存続を検査する。`browser818_ipc_peer_limit_recovers_slot_after_completed_close` は production と同じ admission owner に実 pipe を渡す。満杯では同じ待受 handle を再利用して worker 数も配送数も増えないこと、拒否された client の handle を保持しても受付を続けられることを検査する。閉じた worker の完了を実 handle で確認した後、枠を回収して次の要求を受けることも2回検査する。拒否要求の再試行や時間窓でテストを通さない。
- 既存の `browser818_ipc_stop_joins_reply_waiters_with_clients_still_open` は保持数を2から4へ強化し、上限まで保持した EOF 待ちと次の accept が停止で終了・全 join されることを検査する。P1 の遅延読込テストは維持する。wire・CLI・launcher／core client・既存 open-path／activation event は変更しない。今回のビルド・テストは未実行、cargo は最後の fmt のみ。共有 crate の `browser818_ipc|browser818_cli` を設計担当へ引き継ぐ。

P2 修正の再検証では設計担当から共有 crate の17件中16件成功・枠回収テスト1件失敗、本体 lib の127件成功の報告があった。失敗はテスト側の接続順序の誤り。手動で admission owner を進めるテストが、同じスレッドで切断済み instance の `ConnectNamedPipe` を開始する前に次の client open を行い、ERROR_PIPE_BUSY になっていた。製品の listener は次の loop で接続待ちを開始する。テストは client を並行実行し、テスト専用の1回の WaitNamedPipeW と connect 完了／client thread の join で待受・接続の事実を確認してから、従来どおり要求を事前に書いて admission を通す形に修正した。同じ待受再利用を通る service fixture と上限拒否テストも、OS の待受状態を確認してから1回だけ open する。worker 終了は既存の JoinHandle::is_finished で待ち、sleep・接続の再試行・assert の弱化はない。製品コードは変更せず、今回のテスト修正の再実行は設計担当へ引き継ぐ。

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

**段階6の実装引き継ぎ（2026-11-22、自動検証待ち）**

- 新 host の設定は `browser-host/<世代>/config.json` の `{ "v": 1, "launcher": "絶対パス", "output_dir": "保存先基底の絶対パス" }`。設定の作成・登録・解除と portable での登録無効の UI は段階7で扱う。未リリースの新形式で、既存設定・画像の形式は変えていない。
- native 応答は `{ "ok": true, "path": "保存した絶対パス" }`、失敗は `{ "ok": false, "error": "snake_case のコード", "path": "保存済みならそのパス" }`。`registration_missing` は設定の不在・不正、`launcher_missing` は設定された launcher の不在。拡張がそれぞれ再登録／再インストール・解除の案内に対応させる。成功は launcher の終了コードによる受け渡しまでで、App の表示成功ではない。
- frame は本文確保前に 64 MiB、応答は 1 MiB を検査。元バイト列を共有名で非上書き公開し、MOTW は共有部品へ page／final URL を渡す。保存後の MOTW／起動失敗では画像を残す。右クリック記録は寸法・位置・DPR の一致で使用し、時刻期限を設けない。
- host が渡す seed の `maximized` は false。ブラウザが最大化されていても、mIV の外側を表示領域の矩形へ合わせる契約を保つ（mIV 自体の最大化でモニターの作業領域へ広げない）。既存の位置検査・既定位置への処理は変更しない。
- 内包対象は6本。release の host は editor の後、内包対象の署名は launcher の前。出力照合・PE・clean・portable hash manifest とその利用側を揃えた。runtime cleanup の変更はテストだけで、登録世代は対象へ加えない。
- 新規 Rust テストは host 12件、launcher と runtime cleanup の bundle 回帰各1件。filter は `browser818_host|browser818_positions|browser818_bundle`。スクリプト回帰は `test-build-dev-safety.ps1`、`test-release-build-safety.ps1`（process policy を含む）、`test-musicvideo-build-safety.ps1`、`test-build-portable-source-fingerprint.ps1`。既存 companion の検査は残したまま host を追加した。
- ビルド・Rust テスト・PowerShell スクリプト・アプリの実行は行わず、設計担当へ引き継ぐ。実ビルド時間・配布物・署名の検証結果は未取得。

**段階6の独立レビュー修正（2026-11-22、検証待ち）**

- 保存先の基底に加え、作成後の月フォルダも、ファイル作成前に共有の `is_browser_host_destination` で検査する。既存の月フォルダが junction で登録世代を指す場合は `InvalidDestination`。保存・MOTW・launcher 起動へ進まない。
- 画像リンクの拡張子と signature の正本は `crates/capture-file/src/image_formats.rs` の公開表 `CAPTURE_IMAGE_FORMATS`。TIFF、HEIC/HEIF、JPEG XL を含む既存12拡張子・9形式。host と取り込みの両方が参照する。段階8の JS 側の拡張子集合は、この表の `extensions` 列との一致をテストする。
- 取り込みの部分ヘッダ判定と bounded decoder の対応、AVIF の native 優先は維持する。host の全バイト列判定では BMFF の box 長も検査する。保存名・MOTW・既存画像／設定の形式は変更しない。追加9テストの検証は設計担当へ引き継ぐ。

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

**段階7の実装引き継ぎ（自動検証待ち）**

- `src/browser_integration/` に登録 backend と root の単一 owner を配置。準備・登録／解除・保存先操作・既定先解決・registry・copy/hash・掃除は worker。利用不可時は登録へ触れない。起動時の有効な登録は同じ世代／設定／host hash なら再利用する。
- UI は「起動と連携」の取り込みの次に置き、操作ごとに反映する。設定を閉じても process-global poll が完了を回収する。場所▼も取り込みの直後、通常の位置と末尾 fallback の両方へ追加し、Remote は変更しない。
- 受付済みの登録／保存先の値は処理中の要求が持ち、環境設定の未保存の編集には重ねて残さない。処理中の表示はその要求を参照する。失敗時、まだ同じ画面が開いていて別の編集がなければ、要求の値を編集欄へ戻して通知する（登録の rollback・自動再試行はしない）。
- 前提確認で `Settings::save_checked` が同期の全体保存であること、`SAVE_GENERATION` が全項目の保存証明であることを確認した。UI から新たな同期保存を呼ばず、worker の2キー保存を追加。root 採用後に保存し、全体保存の世代は進めない。初回バックアップの claim と保存は全体保存と共通の gate で直列化する。新規2キーは未リリースで DB スキーマの変更・移行はない。
- 設定ファイルの import で新規2項目が変わる場合も同じ worker へ合流する。処理中／利用不可ならその2項目は採用せず通知し、既存の他項目の適用は維持する。
- installer は usPostUninstall で自己 manifest を指す HKCU だけを解除し、管理フォルダを reparse point をたどらず削除する。画像の保存先と拡張は対象外。実行アカウント以外の登録は残る旨を UI に表示する。
- UI の新規文言あり。`check_ui_glyphs.py` と Rust の各 filter、4件の snapshot の生成・目視・通常比較、Inno のビルドは設計担当へ引き継ぐ。cargo は最後の fmt のみとし、コンパイル・テスト・実登録・製品起動は行わない。

**段階7の独立レビュー修正（P1・P2、2026-11-22、検証・再レビュー待ち）**

- P1 の不変条件は、通常終了が「登録処理の完了 → root が結果を live Settings へ採用 → 2キー保存の終端」より先へ進まないこと。従来は画面寿命から独立した poll だけで、終了受付との接続がなく、worker 完了済み・root 未回収から古い enabled を終了保存し得た。
- `root_lifecycle::accept_root_lifecycle_request` に他の後片付け owner と同じ参加口を足した。`BrowserIntegration` が `Option<RootCloseIntent>` の一つの消費済み継続を持ち、準備／登録／解除／保存先更新／保存中は CancelClose。追加の登録操作は受け付けない。既存の worker 完了 repaint と非blocking poll で進み、保存まで終わってから継続を一度だけ取り出し、`resume_root_lifecycle_close` へ戻す。Quit は普通の Close で格下げしない。App の bool、時間窓、再試行は足さない。
- フォルダ picker も同じ worker の Action なので、通常終了で receiver を同期受信する案は採らなかった。未完了の picker を含め UI を止めず、既存の終了継続に乗せる。トレイ退避はプロセス終了ではないため、その場で従来どおり退避し、登録は継続する。LogicOnly の service にも同じ poll を接続する。
- `on_exit_inner` の最初に最終 fence を置く。通常の終了受付を迂回してここへ来た場合だけ、既に発行した worker の receiver を受け取り、結果を live に採用し、既に進んでいる部分保存の終端を確かめる。その後の既存の終了保存が新しい値を含む。ここで registry／ファイル操作のやり直しや、新たな部分保存は行わない。描画中の同期I/Oではなく、終了時の Settings-family／sidecar 等と同じ最終所有境界での結果回収であり、通常の描画・入力には待ちを持ち込まない。
- GPU group の新しい handler-level 回帰で、worker 完了・root 未回収と worker 未完了の各状態から、実際の root Close／Quit、採用、部分保存、再開された Close、終了保存まで通す。登録・解除・保存先更新・picker を注入し、終了後の実DBと注入した登録結果の一致を検査する。保存中の追加 Close、トレイ退避、受付を迂回した on_exit、準備中／error／channel切断も検査する。実レジストリ・desktop は触れない。
- P2 のUI注意を「★固定中は、ブラウザ閲覧の保存先が固定の範囲の外にあると、ブラウザからの画像を開けません。」へ修正。段階9のマニュアルは既に範囲外の条件を明記しているため変更しない。UI文言の glyph 検査と4件のsnapshot再生成・目視は設計担当へ引き渡す。cargo は最後の fmt のみ、今回のビルド・テストは未実行。

**段階7・9の独立レビュー2回目の修正（2026-11-22、検証・再レビュー待ち）**

- 起動時準備の影響を `PreparationEffect::{ReadOnly, MayUpdateRegistration}` で区別し、Running は worker 発行時の設定から確定した影響と receiver を一緒に持つ。OFF の準備は root の終了継続を持たず、最終 fence でも受信せず破棄する。未発行時は受付時の設定で判定し、ON の起動時更新は従来どおり待つ。後で live の設定が変わっても実行中 worker の分類は変えない。
- `root_lifecycle` の `sidecar_rejected` 判定を登録 owner へ継続を渡す前に適用する。根拠は `sidecar_restore` の「通常 Close は復元中の入力として拒否し、後で再生しない／明示 Quit は終了経路」の区別。settings-family も終了の適格性と処理中 phase を確認してから継続を保持する（`settings_family_deferred_root_close`・`SettingsFamilyOperationState::defer_root_close`）。登録側も不適格な要求を保持しない。他の参加者の動作は変更しない。
- 保留した準備 worker の OFF / ON から実際の Close を入れる回帰と、OFF の最終 fence が応答なしで戻る回帰を追加。復元が先に終わり登録が後に完了する場面では、拒否された Close は再発行されず、明示 Quit は待って再開することを別々の回帰で検査する。GPU を使う回帰は既存 `gpu_tests` 内。睡眠・時間窓・再試行は追加しない。
- 最終 fence は ChooseFolder の同期 picker も受け取る。受付を迂回した終了と picker の組み合わせは、まれな終了の割り切りとして扱い、追加の取消や回復の仕組みは設けない。worker が短い registry／FS 操作だけという前提にはしない。
- 段階9の範囲と公開チェックは、日本語マニュアル、および privacy.html の日本語・英語の両方の節へ統一。英語マニュアルの新設・存在は要求しない。ビルド・Rustテストは設計担当へ引き継ぎ、cargo は最後の fmt のみ実行する。

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
- リンク先 (画像拡張子の HTTP(S) URL のときだけ) とページの画像を順序付き候補とし、同じ URL は一つにする。リンク先・ページの画像・ページの origin を重複なく、クリック内の最初の await より前にまとめて許可要求する。画像候補の origin のどれかが未許可なら `permission_denied` で終え、別候補へ逃げない。ページだけ未許可なら従来どおり位置を推定する。
- **0.2.1 の独立レビュー P2 対応 (2026-11-23)**: `srcUrl` が `data:`・`blob:`・不正 URL でも、有効な画像リンクがあればリンク先だけを候補とする。候補は URL と `kind: link/source` を一つに保持し、HTTP(S) 以外の origin は許可に含めない。リンク先だけでも応答の型を調べ、非画像なら本文を cancel し `link_not_image` と `!` バッジ・日英の title で知らせる。リンク先が無いときの不正な `srcUrl` は従来どおり `invalid_url`、同一 URL の候補はページの画像として一つにする。
- 取得は `credentials: include`、`cache: no-store`。**0.2.1 (2026-11-23、利用者が Commons の説明ページで NG を報告)**: リンク先は `Content-Type: image/*` (大小無視、パラメータ付き可) のときだけ使い、それ以外は本文を読まず cancel してページの画像を開く。成功時は `success_fallback` と成功バッジ・日英の title で切り替えを知らせる。ページの画像に型の検査は足さず、従来どおり host が形式を検査する。
- fetch・HTTP・サイズ超過など取得自体の失敗では切り替えない。候補は最大二つの逐次取得とし、既存の同期 latch を維持することで、候補別の許可回避・並行取得・再試行の組み合わせを作らない。まれな失敗は既定どおり通知して終了し、回復処理を増やさない。
- DNR は取得する URL ごとの完全一致・`TAB_ID_NONE`・自拡張起点限定。各取得の前に足し、終わったら外す。二回目の取得の前に付け直し、後片付け失敗は従来どおり `rule_failed`。
- `image_url` と `final_url` は実際に使った候補の値を送る。切り替え後の失敗は、その失敗の状態で知らせる。
- `response.url` を `final_url` として必ず送る。
- Content-Length、読み取り中、最終 JSON の UTF-8 バイト長の三段階で上限を検査。
- worker 再起動では自分の残存規則を除去し、中断を一回通知。再取得・再送しない。
- 処理中、許可拒否、未登録、版不一致、保存・起動失敗を ja/en の badge／title で知らせる。

`context.js` は上位フレームの座標と寸法だけを記録します。試作の `target_src` や要素の内容は読みません。登録・既存タブ注入は `onInstalled`、`onStartup`、`permissions.onAdded` を通し、二重 listener を防ぎます。

**key と ZIP**

- ソース manifest の key：ClaudeCode 提供の Chrome 公開鍵。
- Chrome ZIP：Chrome key。
- Edge 確認用 ZIP (`mimageviewer-browser-extension-edge-v<VER>.zip`)：生成物の manifest だけ Edge key に差し替え。開発者モードでの通しの確認にはこちらを展開する。
- Edge 提出用 ZIP (`mimageviewer-browser-extension-edge-v<VER>-store.zip`)：確認用の manifest の key 行だけを除去し、ほかの書式・バイトは保つ。JSON の妥当性と key 以外の全フィールドの一致をスクリプト内で検査する。manifest 以外の収録物・順序・日時・圧縮設定は確認用と同じ。
- Chrome と Edge の公開鍵から計算した ID と指定 ID の一致を検査。
- 2026-11-23、利用者の提出時に Edge Partner Center が manifest の key を拒否したため、Edge へは提出用を出す。Chrome は key 入りで受け付けられており変更しない。
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
- Chrome・Edge 確認用 ZIP の key・ID・権限・locales・収録内容。Edge 提出用の key 不在、確認用との残りの全フィールド・key 行以外の manifest バイト・ほかの収録物のバイト一致。3 種とも 2 回の生成で ZIP がバイト一致すること。
- リンク先の HTML・型なし・汎用型の本文を読まずに破棄し、ページの画像へ切り替えること。画像型ならリンク先だけ使い、取得失敗では切り替えないこと。候補ごとの許可・DNR の順序、実際の候補の URL、`success_fallback` の日英通知と成功バッジ。

モックテストを、Chrome／Edge の実動作や座標精度の証拠にはしません。

**完了条件**

製品用 manifest と三つの ZIP (Chrome、Edge 確認用、Edge 提出用) を再現可能に生成でき、試作専用コード・権限・診断本文を含まないこと。

**段階8の実装引き継ぎ（2026-11-22）**

- 製品拡張の版は `0.2.0`。ソースの manifest は Chrome の公開鍵を持ち、Edge 用は ZIP 内の key だけを変える。提供されたアイコンと `store-keys.json` は変更しない。鍵・テスト・この文書は ZIP に入れない。
- 要求は同期の latch で一件に制限し、`permissions.request` を最初の await より前に呼んでから session storage に記録する。画像 origin の許可があれば取得でき、ページ origin が無いときは右クリック記録を使わない。画像リンクの拡張子集合は Rust の共有表と Node テストで照合する。
- session rule は固定 ID `818` を専用に所有し、正規表現でエスケープした最初の URL の完全一致、GET、`xmlhttprequest`、`TAB_ID_NONE` に限定。取得の成否にかかわらず消し、削除が失敗した場合も送信せず終了する。Cookie を含む一回の fetch、既定の redirect、`response.url`、宣言サイズ／stream／JSON UTF-8 長の検査を持つ。
- 起き直した worker は専用の残存規則と要求を消して中断を一回通知する。再取得・再送・時刻期限は持たない。native の13種類の失敗は、それぞれ別の ja/en の説明に対応する。API の失敗や未知の応答も badge/title で知らせる。画像の内容や診断本文はログに出さない。
- `context.js` は ISOLATED の上位フレームで直前一件の座標と geometry だけを保持する。記録の有効性と位置換算は段階6の host が所有し、JS へ重複して実装しない。登録は単一 ID、登録・更新イベントは直列に処理する。インストール／起動／許可追加時の既存タブ注入と、同一 document への二重 listener 防止を持つ。
- API 契約は Chrome の一次資料（[permissions](https://developer.chrome.com/docs/extensions/reference/api/permissions)、[DNR](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest)、[scripting](https://developer.chrome.com/docs/extensions/reference/api/scripting)）でも確認した。Node のモックと ZIP 検査を、実 Chrome／Edge の許可 UI・Cookie・redirect・座標精度の証拠とはしない。ストアへの提出・公開と実機検証は別工程。
- Node v22.16.0 の64テストは全件成功（`target/browser-extension/worker-tests.tap` に全テスト名と結果）。通常の `node --test` はこの実行環境で子プロセスの spawn が EPERM となったため、`--experimental-test-isolation=none` を付けて同じテストを実行した。PowerShell の package テストも成功（`target/browser-extension/package-test-results.txt`）。最終ソースの二つの ZIP は `target/browser-extension/` に生成済みで、対応するテスト生成物とバイト一致。Chrome ID は `fpppoodelfkddekjahhmalmmfdhbpdma`、Edge ID は `dlcbdkekoppppebnhhncghhglkkccdhk`。Rust・cargo・実登録・ブラウザ・mIV は実行していない。

**拡張 0.2.1 の検証記録（2026-11-23）**

- `node --test --experimental-test-isolation=none browser-extension/tests/worker.test.cjs`: 87 件成功、失敗 0 件 (既存 66 件と追加 21 件)。通常の `node --test` は環境の子プロセス起動制限 (`spawn EPERM`) でテスト開始前に停止したため、既存の段階8と同じ分離無効の実行に切り替えた。
- `scripts/test-package-browser-extension.ps1`: Chrome・Edge とも key/ID・権限・翻訳・収録ファイル・ソース一致・再現性が PASS。出力範囲とソース不変の検査も PASS。検査スクリプトの変更は不要だった。
- `git diff --check` と `python scripts/check_ui_glyphs.py` は成功。Rust・native host は変更せず、cargo・製品バイナリ・実ブラウザは実行していない。Commons での修正後の実機結果は未確認。
- マニュアルに残っていた「1 回取得」を候補切り替えの説明へ修正し、両画像サイトへの許可が必要なことも追記した。
- 独立レビュー 1 回目 (GPT-6.1 Sol / xhigh、read-only、設計担当が依頼、対象 `e82bb8699`): **REVISE**。P2 1 件 (`srcUrl` を無条件に HTTP(S) として検査するため、`data:` / `blob:` のサムネイルを包む有効な画像リンクが取得前に `invalid_url` になる退行)。それ以外 (規則の後片付け、候補ごとの URL・型・サイズ予算、許可の範囲、切り替えの通知) は適合。
  - 訂正 (2026-11-23、設計担当): この項には当初「独立レビューは ACCEPT、未解決の指摘なし」と書かれていた。これは実装担当が自分の作業の中で書いた記録で、設計担当が依頼した独立レビューの結果ではない。上の 1 回目の結果に置き換えた。

**同日の独立レビュー P2 対応の追補**

- 1 回目の独立レビューの P2 (上) に対応した。候補の型とリンク先だけの取得・`link_not_image` の通知を段階8の仕様へ反映した。途中でメイン PC がブルースクリーンで止まり、実装担当のセッションを resume して残りを仕上げた。
- Node は通常の `node --test` が `spawn EPERM` で開始前に停止したため、`--experimental-test-isolation=none` で実行し、既存 89 件と追加 11 件の計 100 件成功、失敗 0 件。追加分は `data:` / `blob:` / 不正 URL の候補・許可・通知、切り替え後の宣言サイズ／stream 超過時の規則削除、中断した worker の session からの復旧を検査する。中断テストは API の継続を止めて session storage と規則だけを新 worker へ渡すモックであり、実 Chrome の worker 終了の観測ではない。
- パッケージ検査 (Chrome・Edge と出力範囲・ソース不変)、`git diff --check`、グリフ検査は成功。Rust・native host は未変更で、cargo・コミット・製品バイナリ・実ブラウザは実行していない。
- 独立レビュー 2 回目 (同じレビュー担当、read-only、対象 `ad9f0d8cf`): **ACCEPT**。P2 は解消し、コードの新たな P1／P2 は無し。P3 1 件 (この項の記録の訂正。実装担当が書いた「再レビューは ACCEPT」は、レビューの前に書かれたもので証拠にならない) を受けて、この項を書き直した。レビュー担当は Node テストを `--experimental-test-isolation=none` で自分で流し、100 件成功・失敗 0 件、`git diff --check` も成功。

**段階8の独立レビュー P2 修正（2026-11-22、再レビュー待ち）**

- `TAB_ID_NONE` だけでは Web の service worker の要求も含む。`exactRule` へ実行中の `chrome.runtime.id` を渡し、`initiatorDomains: [extensionId]` を追加した。URL完全一致・GET・resource type・タブ条件は維持し、Chrome／Edge の ID を製品コードへ固定しない。
- 根拠となる Chrome の公式資料は、[拡張のネットワーク取得と独立した origin](https://developer.chrome.com/docs/extensions/develop/concepts/network-requests#extension-origin)、[拡張 origin の `chrome-extension://<ID>` 形式](https://developer.chrome.com/docs/extensions/how-to/web-platform/origin-trials#register-your-extension)、[initiator は要求の起点 origin](https://developer.chrome.com/docs/extensions/reference/api/webRequest#event-onBeforeRequest)、[DNR の initiatorDomains は要求先ではなく起点のドメインを照合する](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest#property-RuleCondition-initiatorDomains)。これらの仕様の照合に基づき、拡張 service worker の fetch の起点の host が拡張 ID と一致すると判断した。DNR の[service worker との関係](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest#interactions-with-service-workers)にも fetch が対象となることが明記されている。実ブラウザでの再現・適用確認とは区別する。
- 条件オブジェクトの小さな matcher をテストに置き、同じ URL・GET・tabId=-1 でも Web 起点を除外すること、従来の条件だけなら一致すること、別拡張・起点なしも除外することを検査した。実際の click→fetch の規則も、提供された Chrome／Edge の両 ID で検査する。
- Node は通常コマンドが環境の child spawn EPERM で開始できず、`--experimental-test-isolation=none` で66件すべて成功。ZIP検査も成功（両 ID・権限・locales・収録内容・再現性・ソース不変）。生成物はこの worktree の `target/browser-extension/package-test-c466e32f6d5a41a2a24b407083e67df6` 内だけ。Rust・cargo・コミット・実ブラウザ起動は行っていない。

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
- 日本語マニュアルの新規ブラウザ連携ページ、目次・設定案内（英語マニュアルは作らない）
  - ページは `htdocs/mimageviewer/manual/browser-integration.html`、拡張の入れ方の節の id は `install-extension`。環境設定の「拡張の入れ方 (マニュアル)」のリンクがこのページの節を指す (`src/ui_dialogs/preferences/browser_integration.rs` の定数、2026-11-22 統括の決定)。ストアの URL は本体に書かず、この節だけに書く。ストアでの公開前は、この節に「公開準備中」と書く (本体の画面には書かない。公開後に古い文言が残るため)
- `htdocs/mimageviewer/privacy.html` の日本語・英語の両方の節
- 既存の製品ページ（日本語。紹介カードは拡張公開時まで追加しない）
- 配布 readme

**記載内容**

**公開の順序（2026-11-22 統括・利用者の決定）**：本体を先に公開し、拡張のストア公開は後にする。本体公開時のマニュアルは「公開準備中」としてストア URL を載せず、privacy は本体が作る登録・中継ファイル・画像と削除範囲を先に説明する。以下の拡張の通信の開示は、拡張の公開前に満たす最終要件として保持する。製品ページの新しい紹介カードは拡張公開時まで追加しない。

設計書 §11 に従い、一回の画像取得、Cookie、常に送るページ origin、redirect、座標記録、PC 内への受け渡し、MOTW、保存後失敗時の残存、登録・解除を privacy の日本語・英語の両方の節で説明します。マニュアルは日本語だけを対象とします。

窓のモード差、メイン一覧の移動、F12 制限、F11、位置の限界、★固定、ページ個別補正と U の書込先の違いも記載します。閉じた後のブラウザへの復帰は保証しません。

外部ダウンローダ名、特定サイトを宣伝に使う表現、公開文書の内部実装用語は入れません。ストア提出・公開は ClaudeCode／利用者の公開工程へ引き渡します。

**完了条件**

対応する自動検証・独立レビュー・必要な利用者実機確認を区別して記録し、未確認事項を成功扱いせず引き渡すこと。

**段階9の引き渡し（2026-11-22、文書のみ・独立レビュー待ち）**

- 本体先行公開のマニュアル (日本語。サイトに英語のマニュアルは無い) を作成し、`install-extension` の節を公開準備中とした。右クリック・サイト許可・badge の説明は、その節に公開後の操作として記載。ストアの URL は載せない。privacy の日英両方には、現時点の本体が作る HKCU の2キー、中継ファイルと設定、保存画像、解除・アンインストールの削除範囲を先に記載した。拡張の通信の開示は公開前の追記作業として残す。
- **英語版の配置**：サイトには英語のマニュアルが無い (英語は privacy.html の英語の節だけ)。実装担当が新設した `en/manual/browser-integration.html` は、孤立したページになるので設計担当が外した (2026-11-22)。privacy.html の英語の節は、他の英語の節と同じく日本語のマニュアルへリンクする。アプリ内のリンクは日本語のページ。
- 根拠は `src/browser_integration/backend.rs`（資格、保存先、世代完成と登録、解除・掃除）、`crates/browser-host/src/lib.rs::handle`（URL は出どころの記録に使用し、受け取った bytes を保存して launcher へ渡す）、`src/single_instance/browser_view.rs::activation`（core へは保存済み path と placement）、`installer/mimageviewer.iss::RemoveBrowserRegistration` / `usPostUninstall`（実行アカウントの自己 manifest を指すキーと管理フォルダだけを追加削除）。本体・host にこの画像を外部へ取得する経路は加わっていない。保存先を通常データフォルダ配下に変えてデータ削除を選んだときの画像削除、および使用中の中継ファイルが残る場合は別記した。
- 製品ページの既存カードは、通信を利用者が選ぶ機能と更新確認、保存を PC 内としており、今回の追加を除外する断定はない。新規カードは追加せず、製品ページを変更しない。`check_site_cards.py` の対象も変更なし。配布 readme には解除と portable の制限を追記し、通常版 readme の通信の列挙を限定列挙に見せる文は改めた。
- 設計文書は段階6〜7の既存記録を重ねず、仕様・表示要求／IPC／host の責務・登録 worker の応答性を補完した。detached §11 の段階2〜5の実装合意・ACCEPT と修正経緯は既存記録で充足。新しい KeyAction はなく、F12 の共通拒否と F11 維持も既記録のため、キー文書・detached 文書は変更しない。

| 検証の種類 | 今回の状態と引き渡し |
|---|---|
| 文書の自動・静的確認 | 初回編集時にHTML 5件・新規2URLを確認した記録は英語ページ除外前のもの。現在の公開確認は日本語マニュアルと privacy の日本語・英語の両方の節が対象で、英語マニュアルの存在は要求しない。英語ページ除外後のサイトマップは `--check` 成功（219URL）。今回の修正で公開HTML・製品ページは変更しない |
| 既存privacyの自動検査 | `python scripts/test_public_privacy_scope.py` は9件中7件成功・2件失敗。`test_all_pages_scope_assurances_including_metadata` の privacy の既存「確認では」と、`test_sfz_storage_and_learning_match_hardware_facts` の既存SFZパスの件数不一致。HEAD の privacy をメモリ内で同じ無変更のsuiteへ渡して同じ2失敗となることを確認。今回の変更による新規失敗ではなく、範囲外としてテスト・既存SFZの説明は変更しない |
| 製品コードの自動検証 | cargo・Rustテスト・スナップショット・検証バイナリ作成は今回実行しない。段階1〜6の受け入れや段階8のNode/ZIP検査は、それぞれのソースと担当の記録を再利用し、この文書変更の新規成功として数えない |
| 独立レビュー | 段階7はレビュー中、段階9の文書は未レビュー。今回の実装担当のコード照合は独立レビューの代用ではない |
| 利用者の実機確認 | 今回はブラウザ・mIV・登録・アンインストールを実行していない。試作の確認と、製品の許可 UI・Cookie・redirect・Q4画像リンク・zoom/DPI・窓の位置／focus・登録解除を区別し、製品経路は未確認のまま引き渡す |
| 拡張公開前の残作業 | ストアURLと正式なインストール手順、privacyの日英への一回取得・Cookie・常に送るページorigin・redirect・座標記録・PC内の受け渡しの開示、掲載情報と紹介カード、統合後の必要な検証・レビューを別工程で完了する |

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
  試作 worker は `srcUrl` を使っており、Q4 の実機結果は §9.11 にありません。0.2.0 は画像拡張子判定で HTTP(S) の `linkUrl` を優先し、一回だけ取得する形でした。2026-11-23 の利用者による Commons の NG 報告を受け、0.2.1 では §0・段階8のとおり応答の型で画像かを確かめ、画像でないときだけ `srcUrl` へ切り替えて知らせます。取得自体の失敗では切り替えません。

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

### 段階 3 の実装 (2026-11-22、`5f631f116`) — ACCEPT

指摘なし。凍結の決まりの構造的変更 (実装) に合意。関連テスト 156 件と UI の記号の検査が成功。範囲外の既存テストの変更 (`detached_activation.rs`) は設計担当が戻した。

### 段階 4 の実装 (2026-11-22、`b92b6e243`・`d6623e62c`) — ACCEPT

最初のビルドで新しいテスト 1 件が落ち、要求の画像を選んだ後に前の窓を閉じる順序の誤りと分かって直した。独立レビュー 1 回目は REVISE (P1: 前の linked 窓の置換による終了が新しい DirectMedia reader を壊す)、凍結の決まりは不合意。前の reader の終了を先に完了させる形に直し、2 回目で ACCEPT・合意。関連テスト 190 件成功、本体 lib の全件 19,659 件成功 (`b92b6e243`)。

### 段階 5 の実装 (2026-11-22、`6c07dc523`・`fb20fbbe5`・`201f8fdb6`) — ACCEPT

1 回目 REVISE (P1: 応答の直後の `DisconnectNamedPipe` が未読の応答を捨てる)、2 回目 REVISE (P2: 接続を保持するクライアントに worker と資源の上限が無い)、3 回目 ACCEPT。凍結の決まりの構造的変更 (実装) は毎回合意。IPC 17 件 (3 回連続)・本体 lib 127 件成功。launcher のテストは段階 6 の release の後。レビューの補足: 製品の launcher は `ERROR_PIPE_BUSY` で待たずに再起動の案内で終わる。ふだんの 1 件ずつの送信では、受理の前に次の instance を作るので当たらない。

### 段階 6 の実装 (2026-11-22、`25e5e464d`・`469145a42`) — ACCEPT

1 回目 REVISE (P1: 月のフォルダの junction を介して管理領域へ保存できる、P2: 画像リンクの対象の形式と host の対応形式が食い違う、P3: 空行)。月のフォルダにも共有の判定をかけ、形式の表を `mimageviewer-capture-file` に 1 つにして解消、2 回目 ACCEPT。`build-release.ps1 -PreserveRuntime` (署名なし) 成功、host 15 件・共有部品 15 件・launcher 24 件 (段階 5 の分を含む)・本体 `runtime_cleanup` 23 件・`clipboard_capture` 123 件成功。`test-build-portable-source-fingerprint.ps1` は PowerShell 7 が無く未実行。

### 段階 7 の実装 (2026-11-22〜23、`956a03ddf`・`51bf78a0d`・`f24782fa0`) — ACCEPT

- 設計担当の修正 (レビュー前、`956a03ddf` に含む): 管理フォルダのパスの区切りが `/` 混じりで、アンインストーラの完全一致の比較に合わなかった (区切りを `\` だけにし、テストで固定)。junction を `symlink_metadata().is_dir()` で判定してファイルとして消そうとしていた (DIRECTORY 属性で判定)。settings_transfer の分類 (2 項目は移行しない)、検索索引の照合元、理由の文言の実装用語、環境設定の「拡張の入れ方 (マニュアル)」のリンク (統括の決定)。
- 独立レビュー (GPT-6.1 Sol / xhigh、read-only) 第 1 回 REVISE: P1 = 登録・解除の結果を採用する前に通常終了でき、DB に古い `enabled` が残る (次回起動で登録し直され得る)。P2 = ★固定の注意文が実際の受付条件より広い。→ `51bf78a0d` で、登録の持ち主を root の終了受付に接続 (処理中は CancelClose し、採用・保存の後に 1 回だけ再開。トレイ退避は待たせない。受付を迂回した終了は `finish_for_exit` で結果を受け取ってから終了保存)、注意文を「保存先が固定の範囲の外にあると」に限定。
- 第 2 回 REVISE (段階 9 と合わせて): P2 = 既定 OFF でも起動直後の終了を準備の完了まで待たせる。P2 = 復元モーダルが拒否した Close を登録の持ち主が後で再開し得る (`sidecar_restore.rs` の「拒否した Close は再生しない」に反する)。P2 (段階 9) = 公開チェックが存在しない英語マニュアルを要求。→ `f24782fa0` で、`PreparationEffect::{ReadOnly, MayUpdateRegistration}` により OFF の準備は終了を待たせず最終 fence でも受け取らない。拒否した Close は登録の持ち主に渡さない。チェックは日本語マニュアルと privacy の日英両節に統一。
- 第 3 回: 段階 7・段階 9 とも ACCEPT (指摘なし)。既定 OFF の起動経路に新しい同期処理が無いことも確認された。
- 設計担当の実行: 関連の filter 1088 passed (`f24782fa0`)、`ui_snapshot` 270 passed (新しい 4 件は目視)、glyph ok、`test-test-tools.ps1`・`test-installer-launcher-swap.ps1` PASS、lib 全件は `51bf78a0d` で 19694 passed (`f24782fa0` は実行中)。
- 未確認: インストーラのアンインストール処理の実行、実機での登録・解除・起動経路 (サブ PC / ノート PC の確認で行う)。

### 段階 9 の文書 (2026-11-22〜23、`c6ef521d9`・`f24782fa0`) — ACCEPT

- 実装担当が新設した英語のマニュアル `en/manual/browser-integration.html` は、サイトに英語のマニュアルが無く孤立するため設計担当が外した。privacy.html の英語の節は日本語のマニュアルへリンクする。
- `python scripts/test_public_privacy_scope.py` は 9 件中 2 件失敗だが、master でも同じ 2 件が失敗する既存の失敗 (今回の変更によるものではない)。`python scripts/check_site_cards.py` 53 cards ok。
- 独立レビューは段階 7 の第 2 回・第 3 回と合わせて実施し、第 3 回で ACCEPT。

### 段階 8 の実装 (2026-11-22、`0fde63262`・`94dba1c31`) — ACCEPT

- 独立レビュー (GPT-6.1 Sol / xhigh、read-only、本体の段階のレビューとは別の担当): 第 1 回 REVISE、P2 1 件 = DNR の一時規則が `TAB_ID_NONE` だけで、許可済みの origin の Web の service worker が同じ URL へ GET すると Referer が書き換わり得る (Chrome の文書と Chromium のコードからの指摘、実ブラウザでの再現ではない)。`initiatorDomains: [runtime.id]` で自拡張の起点に限定し、Web 起点が一致しないことをテストで固定 (`94dba1c31`)。第 2 回 ACCEPT。
- 設計担当の再実行: `node --test browser-extension/tests/worker.test.cjs` 66 passed、`scripts/test-package-browser-extension.ps1` PASS (Chrome `fpppoodelfkddekjahhmalmmfdhbpdma`・Edge `dlcbdkekoppppebnhhncghhglkkccdhk` と一致)。
- 未確認: 実際の Chrome / Edge での動作 (許可の画面・Cookie・redirect・DNR の適用・位置)。v5.6.0 に入れるかの判定はこの段階を待たない (統括 2026-11-22)。
