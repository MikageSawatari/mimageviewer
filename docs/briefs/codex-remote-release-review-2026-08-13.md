# v3.0.0 リモート閲覧リリースレビュー引き継ぎ（2026-08-13）

作業ツリー: `C:\home\mimageviewer-web` (`web-remote`)

- レビュー基準 commit: `883d48ed` (`Time each stage of building a page, and count who else is in it`)
- `master` との merge-base: `d69ed731`
- 既知の「表示速度 / 先読みキュー管理」は別作業で進行中のため、本書では重複して指摘しない
- 製品コードは変更していない。本書だけを新規作成した
- 開始時から存在した `src/pdf_loader.rs` の変更と `docs/briefs/` の未追跡ファイル群は、利用者の作業として触れていない
- レビュー中に別作業の `docs/ui-responsiveness.md` / `docs/web-remote-plan.md` への追記が現れたため、これらも触れていない

## 0. 再々レビュー（2026-08-13、HEAD `c4af0192`）

### 判定

前回追加した RR-R1 / RR-R2 の修正 (`d557b966`, `cd234136`) と、その後の master merge
(`60f0236e`) を再確認した。**RR-R1 / RR-R2 は解消しており、今回確認した修正範囲に新しい
actionable finding はない。** 元の RR-01〜RR-10 も前回の解消判定を維持する。

既知の表示速度 / 先読みキュー管理、dependency advisory scan、実 Tailscale / SMB / Android の
動的確認はこの判定に含めない。それらを別の release gate として完了させる前提では、今回の
レビュー指摘を理由にリリースを保留する必要はない。

### RR-R1 の再確認

- logout 完了 receiver は `RemoteConnectionDialogState` ではなく、それより長寿命の
  `RemoteSessionUiState` が所有する (`src/remote_ipc/ui.rs:34-48`)。
- `show_remote_connection_dialog` はダイアログ有無の判定より前に毎フレーム receiver を poll し、
  `Running` 中はダイアログを閉じていても 50 ms 後の repaint を予約する
  (`src/remote_ipc/ui.rs:2552-2591`)。この関数自体も app update から常時呼ばれる
  (`src/app.rs:65284-65285`)。
- 成功遷移だけが `disconnect_local = true` を 1 回返し、成功後の dialog close は drain を
  再発行しない。確認中 / 実行中 / 成功 / 失敗の close 規則も typed transition に揃っている
  (`src/remote_ipc/ui.rs:113-160`, `:222-245`)。
- close 後成功、ダイアログ表示中成功、失敗、4 状態の close table に回帰テストがある
  (`src/remote_ipc/ui.rs:3896-4004`)。

したがって、署名鍵 rotate を開始した後で設定画面を閉じても、成功時の本体 session drain は
ダイアログの寿命に依存せず完了する。

### RR-R2 の再確認

- Cookie は `v2.{expires}.{nonce}.{mac}` となり、login ごとに OS CSPRNG から 16 byte の nonce を
 生成して、version / expires / nonce 全体を HMAC-SHA256 の対象にする
  (`crates/remote-web/src/auth.rs:239-293`)。
- reader は部品数、v2、expires、nonce の 32 hex 文字、MAC の 32 byte を検証してから MAC を
  照合し、v1 は明示的に拒否する (`crates/remote-web/src/auth.rs:295-340`)。
- RNG 失敗時は Cookie を発行せず HTTP 500 とし、nonce 無しへ fallback しない
  (`crates/remote-web/src/http.rs:2433-2452`)。
- 同じ秒の2発行、remember mode 違い、v1 / malformed v2、RNG failure、2 Cookie 間の
  client id / session id 分離をテストしている
  (`crates/remote-web/src/auth.rs:519-606`, `crates/remote-web/src/http.rs:5517-5588`)。

これにより、同じ秒に PIN 認証した別端末が同じ `AuthSessionIdentity` へ衝突する経路は閉じている。
リモート閲覧は未出荷のため v1 を移行対象にしない判断とも整合する。

### 再々レビューで実行した確認

- `node --test crates/remote-web/web/*.test.mjs`: **365 passed**
- `cargo test -p mimageviewer-remote`: **131 passed / 1 ignored**
  - ignored は管理 test sandbox が local named pipe を拒否する既知の test
- `cargo test -p mimageviewer-ipc`: **35 passed**
- `cargo test -p mimageviewer --lib remote_ipc`: **245 passed**
- `cargo fmt --all -- --check`: pass
- `cargo check -p mimageviewer --bin mimageviewer-core`: pass（既存 dead-code warning 8 件）

今回、通常 profile / portable smoke のアプリは起動していない。HANDOFF には修正後の新 Cookie login と
dialog close 後の操作権返却を実機確認済みと記録されているが、本節の判定ではその実機操作を再実施して
いない。

## 0.1 前回の再レビュー（2026-08-13、HEAD `c3316390`）

### 判定

基準 commit `883d48ed` 以降の修正を再確認した。**元の RR-01〜RR-10 は、利用者が決定した
RR-03 / RR-04 の仕様を含め、意図どおり対応されている。** 一方、RR-05 の全端末ログアウトに
非同期完了の所有権漏れと Cookie identity の衝突を見つけたため、現 HEAD を無条件の
リリース可とはしない。

| ID | 優先度 | 分類 | 再レビュー結果 |
|---|---:|---|---|
| RR-R1 | P2 | 機能 / セキュリティ運用 | 全端末ログアウトの実行中に接続ダイアログを閉じると、本体側のリモート操作権解放が実行されない |
| RR-R2 | P2 | セキュリティ / session 所有権 | 同じ秒に PIN 認証した端末へ同一 Cookie を発行し、別端末を同じ session identity として扱う |

RR-R1 / RR-R2 の修正後、下記の未実施実機確認を通せば、今回レビューした範囲ではリリースを止める
既知事項はない。既知の表示速度 / 先読みキュー管理は、引き続き本レビューの対象外とする。

### RR-R1 [P2] 全端末ログアウトの完了処理がダイアログの寿命に従属している

#### 観測した事実

- 全端末ログアウトの確認後、`RemoteSessionLogoutState::Running` が完了 receiver を
  `RemoteConnectionDialogState` 内に保持する (`src/remote_ipc/ui.rs:2994-3000`)。
- receiver が成功を返したときだけ、ダイアログ描画処理が
  `handle.local_disconnect()` を呼び、本体側の現在のリモート操作権を drain する
  (`src/remote_ipc/ui.rs:2508-2526`)。
- しかし「閉じる」、Esc、または window close は、logout が `Running` かにかかわらず
  `connection_dialog = None` とし、receiver を破棄する (`src/remote_ipc/ui.rs:3016-3018`)。
- service owner に送信済みの `RotateSessionSecret` は receiver が破棄されても続行するため、
  Cookie 署名鍵の更新と remote-web の再起動は行われる (`src/remote_ipc/service.rs:258-264`)。
- remote-web の IPC 切断処理は、その接続の page job と接続情報だけを破棄し、active session を
  drain しない (`src/remote_ipc/session.rs:1656-1672`)。本体側の操作権は最終 ping から
  通常 `LIVENESS_TIMEOUT = 60 秒` まで残り得る。非終端の AI / archive job がある場合は
  liveness timeout では解放せず、`IDLE_TIMEOUT = 10 分` まで残し得る
  (`src/remote_ipc/session.rs:21-22`, `:593-608`)。

#### 影響

「すべての端末をログアウト」を確定した直後に接続ダイアログを閉じると、ブラウザ Cookie の
全失効は進む一方、本体 UI は通常最大約 60 秒、非終端 job があれば最大約 10 分、古いリモート
session の drain 待ちになる可能性がある。
認証情報の失効自体は破られないが、セキュリティ操作の完了と本体操作権の解放が原子的でなく、
利用者には「ログアウトしたのに本体操作へ戻らない」と見える。

#### 修正方針

完了 receiver と `local_disconnect()` を接続ダイアログの一時状態から外し、ダイアログを閉じても
生存する所有者へ移す。単に「実行中は閉じるボタンを無効化する」だけでは window close や将来の
別経路に同じ不変条件を残すため、署名鍵 rotate の成功と本体 session drain を 1 つの永続的な
operation owner が完了させる。

回帰テストは少なくとも次を含める。

- rotate 開始 → ダイアログ破棄 → 成功完了でも `local_disconnect()` 相当が必ず 1 回起きる。
- rotate 失敗時は成功表示せず、既存仕様どおり PIN と session secret の状態を区別する。
- ダイアログを開いたままの成功経路でも二重 drain しない。

### RR-R2 [P2] 同じ秒の PIN 認証が別端末で同一 Cookie identity になる

#### 観測した事実

- session Cookie の署名対象は `v1.{expires}` だけで、端末 / login ごとの nonce を含まない。
  `expires` は秒精度の現在時刻 + 90 日であるため、同じ秒に発行した Cookie は完全に同一になる
  (`crates/remote-web/src/auth.rs:252-274`, `:331-335`)。`remember` の違いも Cookie 本文には入らない。
- server-side の `AuthSessionIdentity` は Cookie 全文の SHA-256 で作る
  (`crates/remote-web/src/auth.rs:170-176`)。したがって、同じ秒にログインした別端末を区別できない。
- `RemoteClientIdentities::resolve` は identity が既存なら、新しい request の
  `X-mIV-Remote-Client` を無視し、先に保存された client id を返す
  (`crates/remote-web/src/http.rs:104-125`)。
- 同じ map entry に remote session id も保存するため、後から acquire した端末が同じ entry を
  上書きし、session header を付けられない stream request なども別端末の現在 owner へ結び付く
  (`crates/remote-web/src/http.rs:141-179`)。

#### 影響

2 台の端末、または別 browser profile が同じ 1 秒内に PIN 認証すると、別々の
`X-mIV-Remote-Client` を送っても server は同一 client として扱う。session acquire / supersede の
所有者表示と routing が交差し、常時 1 台だけを active owner とする不変条件、および Cookie ごとの
logout/session bookkeeping が崩れる。PIN を知らない第三者への権限昇格ではないが、認証済み別端末を
分離するための security identity が発行時刻の偶然で衝突する。

#### 修正方針

Cookie の署名対象へ login ごとの CSPRNG nonce を追加し、同時発行でも token と
`AuthSessionIdentity` が必ず異なる形式へ上げる。たとえば
`v2.{expires}.{nonce}.{HMAC(version || expires || nonce)}` とし、nonce の長さ・hex 構文を
検証してから MAC を定数時間検証する。v1 Cookie を移行期間だけ受けるか、v3.0.0 の初回起動で
session secret を rotate して再ログインさせるかは、リリース前の互換性判断として明記する。

回帰テストは少なくとも次を含める。

- 同一 `now_unix` で連続発行した Cookie が異なり、異なる `AuthSessionIdentity` になる。
- `remember=true/false` でも token identity は共有されない。
- 同時 login の 2 Cookie を `RemoteClientIdentities` へ渡し、client id と session id が交差しない。
- nonce / expires / MAC の改変、期限切れ、形式不正は従来どおり拒否する。

### 元の指摘の再確認

| ID | 状態 | 確認内容 |
|---|---|---|
| RR-01 | 解消 | PIN 欄は password manager 用属性を保ちつつ数字キーボードを要求しない。DOM test あり |
| RR-02 | 解消 | 所有 proxy の `/` だけを Configured とし、非 root path は非対応として UI へ運ぶ |
| RR-03 | 解消（仕様決定込み） | raw UNC / device namespace を filesystem I/O 前に拒否。割り当て済みドライブは維持し、両マニュアルに差分を記載 |
| RR-04 | 解消（仕様決定込み） | 上限を 100,000 件へ緩和し、40 MiB の直列化予算と画面上の truncation 通知を追加。ページングは明示的に延期 |
| RR-05 | 機構は解消、RR-R1 / RR-R2 あり | この端末の logout、全 Cookie の署名鍵 rotate、PIN 維持、マニュアル説明を確認。完了 lifecycle と token identity に追加指摘あり |
| RR-06 | 解消 | route の early return を含む全応答が共通 finalizer を通り、CSP `frame-ancestors 'none'` と XFO `DENY` を付与 |
| RR-07 | 解消 | Tailscale 側で許可した他者端末にも届き得ることと、mIV が端末所有者を識別しないことを明記 |
| RR-08 | 解消 | 名前検索が「お気に入りの中」であることを両マニュアルに明記 |
| RR-09 | 解消 | 変更可能な補正 subset と、保存済みの他補正は反映だけされることを両マニュアルに明記 |
| RR-10 | 解消 | iOS / iPadOS Safari と PC browser の確認済み範囲、Android Chrome 未実機確認を両マニュアルに明記 |

RR-03 は利用者判断どおり、`\\server\share` を拒否し、Windows の割り当て済みネットワークドライブは
利用可能なままにしている。このため「認証済み client が任意の raw UNC を発行する」境界は閉じたが、
ホスト利用者が明示的に割り当てたドライブの接続は許容する。RR-04 も利用者判断どおり、極端な一覧は
100,000 件または 40 MiB で打ち切って画面で通知し、ページング自体は後続課題としている。

### 再レビューで実行した確認

- `node --test crates/remote-web/web/*.test.mjs`: **365 passed**
- `cargo test -p mimageviewer-remote`: **125 passed / 1 ignored**
  - ignored は管理 test sandbox が local named pipe を拒否する既知の test
- `cargo test -p mimageviewer-ipc`: **35 passed**
- `cargo test -p mimageviewer --lib remote_ipc`: **234 passed**
- `cargo fmt --all -- --check`: pass
- `cargo check -p mimageviewer --bin mimageviewer-core`: pass（既存 dead-code warning 8 件）

実 Tailscale Serve、実 SMB、Android Chrome の動的確認は今回も実施していない。HANDOFF に記録された
iPad 実機修正についても、今回の再レビューでは実機再試験していない。

## 1. 初回レビュー時の結論（履歴）

基準 commit `883d48ed` の時点では、v3.0.0 のリモート閲覧をそのままリリース可とは判断しない。
少なくとも **P1 の 3 件は、修正するか、セキュリティ境界を明示的に再決定してからリリース判定する**。

| ID | 優先度 | 分類 | 要約 |
|---|---:|---|---|
| RR-01 | P1 | 機能 | 本体で英字・記号 PIN を設定できるのに、スマホ側が数字キーボードを要求する |
| RR-02 | P1 | 機能 | Tailscale Serve のサブパスを「設定済み」と判定するが、SPA は origin root 固定で動かない |
| RR-03 | P1 | セキュリティ / 仕様判断 | 認証済みクライアントが任意 UNC を解決させ、ホストから SMB 認証を発生させ得る |
| RR-04 | P2 | 機能 / 仕様差 | 一覧・検索・コンテナを 1000 件で打ち切り、ページングがない |
| RR-05 | P2 | セキュリティ | 既定 90 日 Cookie にログアウト / 失効操作がない |
| RR-06 | P2 | セキュリティ | anti-framing がなく、リモート UI を別ページから iframe 化できる |
| RR-07 | P2 | セキュリティ文書 | 「自分の端末以外からは届かない」という説明を実装は強制していない |
| RR-08 | P3 | マニュアル差分 | 名前検索が「お気に入り内のみ」であることをマニュアルが明記していない |
| RR-09 | P3 | マニュアル差分 | リモートで編集できる画像補正が本体の一部であることを明記していない |
| RR-10 | P3 | マニュアル差分 | 動画の動作確認済み / 未確認環境を、計画の要求どおり記載していない |

優先度の意味:

- P1: リリース前に修正または明示的な仕様・リスク受容が必要
- P2: リリース前の解消を推奨。延期するなら制約と運用を明記する
- P3: 実装よりも利用者向け仕様の正確さに関する問題

## 2. 指摘事項

### RR-01 [P1] 英字・記号 PIN をスマホで入力できない組み合わせがある

#### 観測した事実

- 本体の正本 validator は `!` から `~` までの印字可能 ASCII を許可する
  (`crates/remote-ipc/src/auth.rs:31-44`)。
- 本体 UI とマニュアルも「半角英数字・記号」と案内する
  (`src/remote_ipc/ui.rs:2510`, `htdocs/mimageviewer/manual/remote.html:131-135`)。
- Web の PIN 欄は `inputMode = "numeric"` で、placeholder も「6桁以上」になっている
  (`crates/remote-web/web/app.js:2185-2193`)。

#### 影響

iOS / Android の数字キーボードでは英字・記号へ到達できない、または切替が著しく困難な場合がある。
本体で `Abc!23` のような有効 PIN を設定した利用者が、主対象であるスマートフォンからログインできない。
validator の問題ではなく、**設定側と入力側の契約不一致**である。

#### 推奨修正

- PIN 欄の `inputMode = "numeric"` を削除するか `text` にする。
- placeholder を「6文字以上」に揃える。
- モバイル入力の意図しない変換を避けるため、`autocapitalize="none"`、`spellcheck=false` も固定する。
- PIN の許容文字を数字だけへ狭める修正は、既存仕様の機能低下になるため行わない。

#### 回帰条件

- 英字・数字・記号を混ぜた有効 PIN を iOS / Android 相当の入力経路で入力できる。
- DOM 契約テストで PIN 欄が numeric keyboard を要求しないことと、「6文字」の表示を固定する。
- 本体 validator の全 printable ASCII テストは維持する。

### RR-02 [P1] Serve サブパスを対応済みと誤判定する

#### 観測した事実

- `inspect_tailscale_serve` は、所有 proxy が `/miv` のようなサブパスにあっても
  `Configured` とし、`https://host/miv/` を接続 URL にする
  (`crates/remote-web/src/connection_url.rs:168-205`, `:231-235`)。
- `configured_handler_path_is_part_of_the_connection_url` はこの挙動を成功として固定している
  (`crates/remote-web/src/connection_url.rs:539-547`)。
- 一方、HTML の manifest / icon / CSS / module は `/...` の origin-root 固定
  (`crates/remote-web/web/index.html:18-20`, `:103`)。
- API、stream、service worker も `/api/...`、`/stream/...`、scope `/` の固定値である
  (`crates/remote-web/web/app.js:1424-1427` ほか、`crates/remote-web/web/video-stream.mjs`)。

Tailscale 自体は `--set-path` によるサブパスマウントを正式に提供しているため、存在し得る設定である。
参考: [Tailscale Serve CLI](https://tailscale.com/docs/reference/tailscale-cli/serve)

#### 影響

接続ダイアログは「設定済み」と表示し、QR / URL も `/miv/` を案内するが、読み込まれる asset と API は
root へ飛ぶ。root が未設定なら 404、別サービスがあればそのサービスへ誤送信する。
現在の自動設定が root を使うことは、既存設定検出の誤判定を解消しない。

#### 推奨修正

今回は **所有 proxy の handler path が `/` のときだけ `Configured` とする**のが小さく安全である。
非 root は「未設定」ではなく「サブパス構成は未対応」と区別できるとよい。

サブパス対応を選ぶ場合は、接続 URL だけを変えるのでは不十分である。asset、全 API、stream、manifest、
service worker の URL / scope、hash navigation、offline fallback を 1 つの base path owner へ集約する。

#### 回帰条件

- `/` -> 所有 proxy は `Configured`。
- `/miv` -> 所有 proxy は未対応として明示され、動作する URL と誤認させない。
- 本当にサブパス対応する場合は `/miv/` だけから shell、API、動画、service worker が完結する E2E を置く。

### RR-03 [P1] 任意 UNC 解決がリモート閲覧の権限を越え得る

#### 観測した事実

- 共有 `validate_absolute_path` は `\\server\share\...` を明示的に許可し、テストでも成功を固定している
  (`crates/remote-ipc/src/lib.rs:120-135`, `:3397-3404`)。
- remote-web はブラウザが送った絶対 path を `std::fs::canonicalize` する
  (`crates/remote-web/src/path_guard.rs:11-16`)。
- Core 側も同じ path を再検証して filesystem I/O を行う。
- HTTP API は UI から発行された path かどうかを示す capability を持たず、認証済みクライアントは
  query を直接組み立てられる。

Windows の SMB client は共有への接続で認証を行い、NTLM を許す環境では challenge-response を送る。
Microsoft も、悪意ある server へ NTLM request を送らせる攻撃への対策として outbound NTLM blocking を
説明している。

- [Microsoft SMB protocol authentication](https://learn.microsoft.com/en-us/windows/win32/fileio/microsoft-smb-protocol-authentication)
- [SMB security hardening / outbound NTLM blocking](https://learn.microsoft.com/en-us/windows-server/storage/file-server/smb-security-hardening)

#### 影響

tailnet 到達性と有効な PIN / Cookie を持つ攻撃者は、画面に存在しない UNC を直接 API へ渡し、
mIV ホストを任意 SMB server へ接続させ得る。環境によっては NTLM challenge-response の露出、relay / cracking
の足掛かり、または到達不能 UNC による worker 長時間占有になる。

これは未認証攻撃ではない。しかし、マニュアルが説明する「表示できるファイルの閲覧」から、
**ホストに任意のネットワーク I/O と認証試行を行わせる権限**へ境界が広がる。

#### 仕様判断が必要な点

単純な favorite allowlist は、計画書で既に「保証できない境界」として却下されているため復活させない。
次のいずれかを明示的に選ぶ。

1. remote では UNC / device namespace を拒否する（本体との差分になるため利用者承認とマニュアル更新が必要）。
2. Core が列挙・発行した address だけを session-bound capability として受理する。ローカル drive / UNC の
   どちらも扱えるが、ブラウザが任意 path を発明できない構造にする。
3. 認証済み remote client を「ホストの任意 filesystem / network path を開ける完全信頼主体」と定義し、
   UNC と outbound authentication をマニュアルで明記してリスク受容する。

2 が構造的には最も安全だが、全 address lifecycle に及ぶため独立設計が必要である。
実機確認のために開発 PC から未知の SMB server へ接続してはならない。resolver を注入したテストで、
拒否が filesystem I/O より前で起きることを固定する。

### RR-04 [P2] 1000 件超の項目へ UI から到達できない

#### 観測した事実

- folder / ZIP / PDF container は `CONTAINER_ENTRY_LIMIT = 1000` で打ち切る
  (`src/remote_ipc/container.rs:24`, `:3337-3339`)。
- 検索・タグ・スマートフォルダ等も `MAX_REMOTE_COLLECTION_ENTRIES = 1000` で truncate する
  (`src/remote_ipc/collections.rs:17`, `:825-828`)。
- Web は「先頭 1000 件」と表示するが、次ページを取得する手段はない
  (`crates/remote-web/web/app.js:5117-5126`)。
- 開発計画には「ページングは実装しない」と書かれているが
  (`docs/web-remote-plan.md:844-849`)、利用者マニュアルには上限がない。

#### 影響

1001 件目以降は、並び順によってはリモート UI から開けない。大きな画像フォルダ、1000ページ超 PDF / ZIP、
検索・タグ・スマートフォルダで本体との差が出る。「全ドライブの表示可能ファイルを閲覧できる」という
セキュリティ警告 / 機能説明とも厳密には一致しない。

#### 推奨修正

- cursor / offset と安定 sort key を持つページングを container / collection の共通契約として追加する。
- すぐ実装しない場合は、マニュアルの「できないこと」に 1000 件上限と、超過分へ到達できないことを明記する。
- 「画面に警告が出る」だけで仕様差分の説明を完了扱いにしない。

#### 回帰条件

- 1001 件 fixture の最後の項目へ UI 操作だけで到達して開ける、または文書化された制限どおり明示停止する。
- folder、ZIP、PDF、検索、タグ、スマートフォルダを個別に確認する。

### RR-05 [P2] 90 日セッションにログアウト / 失効導線がない

#### 観測した事実

- Cookie の有効期限は 90 日である (`crates/remote-web/src/auth.rs:12-16`, `:237-263`)。
- PIN 画面の「この端末を記憶しない」は既定 unchecked なので、通常操作は 90 日保存になる
  (`crates/remote-web/web/app.js:2195-2202`, `:2240-2245`)。
- auth route は status と PIN login だけで、logout がない
  (`crates/remote-web/src/http.rs:576-580`)。
- マニュアルは Cookie の期限、端末側 logout、端末紛失時の失効方法を説明していない。

#### 影響

共有 / 借用端末で誤って保存した場合や端末紛失時に、利用者が当該端末をログアウトできない。
ブラウザデータ削除か PIN 変更による全端末強制再認証しか実質的な手段がない。

Cookie は署名済み stateless token なので、UI で Cookie を消すだけではコピー済み token を失効できない。
「ログアウト」と「盗難端末 / 全端末からの失効」は別の要件として扱う。

#### 推奨修正

- `/api/auth/logout` と UI を追加し、同じ Cookie 属性で `Max-Age=0` / 過去 `Expires` を返す。
- 本体の接続ダイアログに「すべての端末をログアウト」を設け、PIN を変えずに session secret を rotate する。
- 個別端末失効まで必要なら stateful session registry が必要。field の追加だけで stateless token に
  revocation を装った実装はしない。
- 90 日、session-only checkbox、logout / 全失効の差をマニュアルへ書く。

#### 回帰条件

- logout 後に同じブラウザの API が 401 になる。
- 全端末失効後は発行済み Cookie がすべて無効になる。
- `Secure` 条件を含め、削除 Cookie の属性が発行 Cookie と一致する。

### RR-06 [P2] リモート UI を iframe から保護していない

#### 観測した事実

共通 response header は `X-Content-Type-Options: nosniff` と `Referrer-Policy: no-referrer` のみで、
`Content-Security-Policy: frame-ancestors ...` と `X-Frame-Options` がない
(`crates/remote-web/src/http.rs:826-834`)。

さらに query parse error と「session 未取得」の早期 return は、この共通 header 追加自体を通らない
(`crates/remote-web/src/http.rs:522-562`)。

#### 影響

リモート画面は埋め込み用途を持たないのに、別の tailnet Web ページ等から frame 化できる。
透明 overlay による操作誘導や、frame 内で PIN を入力させてからの clickjacking を防ぐ層がない。
`SameSite=Lax` は frame 禁止の代用ではない。

#### 推奨修正

- 全 response の最終化を `handle` 側の 1 箇所に集約する。
- 少なくとも `Content-Security-Policy: frame-ancestors 'none'` と `X-Frame-Options: DENY` を全応答へ付ける。
- 完全な CSP (`default-src` 等) は inline bootstrap の nonce / hash を含む別設計でよいが、
  `frame-ancestors` は独立して先に入れられる。

#### 回帰条件

- index、asset、401、400、session 未取得、stream error の全系統で anti-framing header が付く。
- 別 origin の iframe から表示できないことをブラウザ E2E で確認する。

### RR-07 [P2] Tailscale の到達性説明が実装より強い

#### 観測した事実

- チュートリアルは「サインインした自分の端末以外からは、そもそも届きません」と断定する
  (`htdocs/mimageviewer/manual/tut-remote.html:62-71`)。
- 別の警告は、正しく「Tailscale address へ接続でき、PIN を知っている人から見える」と書く
  (`htdocs/mimageviewer/manual/remote.html:83-89`)。
- remote-web は `Tailscale-User-*` の値を認可には使わず、PIN / Cookie だけを application auth とする。
- Tailscale の公式説明では、Serve traffic には **device share を受諾した外部利用者**も含まれ得る。
  参考: [Tailscale Serve — Identity headers](https://tailscale.com/docs/features/tailscale-serve#identity-headers)

#### 影響

node sharing、ACL / grants、tailnet 運用によっては「同じ自分のアカウントの端末だけ」が実装上の保証ではない。
現状の本当の境界は「Serve URL へ到達でき、PIN / 有効 Cookie を持つ主体」である。

#### 推奨修正

- `remote.html:86-88` の到達性 + PIN の説明を正本にする。
- 「同じアカウントでサインイン」は簡単なセットアップ手順として残してよいが、セキュリティ保証として
  「自分以外は届かない」と断定しない。
- device share / ACL / grants でこの PC へ到達を許した相手も対象になること、共有用途は想定外であることを
  近接して書く。
- 本当に self-only を強制するなら、PIN を省略する話とは別に Tailscale identity を認可境界として
  設計する必要がある。tagged device や loopback header spoofing を含むため、文字列比較だけで済ませない。

### RR-08 [P3] 名前検索の範囲がマニュアルでは広く読める

- 実装は全 favorite path を `SearchIndexDb::search` の scope に渡す
  (`src/remote_ipc/collections.rs:199-227`)。
- Web UI は「お気に入りの中から、フォルダ・ZIP・PDF」と正確に表示する
  (`crates/remote-web/web/app.js:3344-3350`)。
- マニュアルは「フォルダ・ZIP・PDF を名前で検索」とだけ書き、お気に入り内という範囲を省略する
  (`htdocs/mimageviewer/manual/remote.html:183-185`,
  `htdocs/mimageviewer/manual/tut-remote.html:209-211`)。

両マニュアルを「お気に入りの中にあるフォルダ・ZIP・PDFを名前で検索」に揃える。
本体の他の検索機能と同等の全域 / メタデータ検索だと誤解させない。

### RR-09 [P3] 「画像補正」が本体と同じ全機能に見える

- wire 型はリモートで編集できる値を明示的な subset とし、post-filter 等を含めない
  (`crates/remote-ipc/src/lib.rs:277-297`)。
- Web の補正タブは `色調 / AI / カラー化` の 3 つだけ
  (`crates/remote-web/web/local-settings.mjs:9-13`)。
- 計画書も本体の `フィルタ` は未公開と記録する
  (`docs/web-remote-plan.md:1471-1487`)。
- 本体で保存済みの補正・編集結果は remote 描画へ反映される一方、リモートから編集できない項目がある。
- マニュアルは単に「画像補正、表示トリム」と書く
  (`htdocs/mimageviewer/manual/remote.html:189-191`,
  `htdocs/mimageviewer/manual/tut-remote.html:215-217`)。

マニュアルでは、少なくとも次を分けて書く。

- リモートから変更できる: 色調、自動補正、AI upscale / denoise、カラー化、表示トリム。
- 本体で保存済みなら表示へ反映されるがリモートから変更できない: smart sharpen、Creative LUT、
  post-filter 等。
- リモート編集の対象外: 消しゴム、補正レイヤー、隠蔽加工、切り取り、テキスト注釈、export。

実装の追加状況に合わせて一覧を正本化し、「描画へ反映」と「リモートから編集」を混ぜない。

### RR-10 [P3] 動画の検証環境がマニュアルにない

動画計画は、利用者向けマニュアルへ次を明記するよう要求している
(`docs/web-remote-video-streaming-plan.md:1091-1111`)。

- iOS / iPadOS Safari: 実機確認済み
- Android Chrome: 対応対象だが実機未確認
- PC Chrome / Edge / Firefox: hls.js 経路の開発・確認に使用

現在の `remote.html` / `tut-remote.html` はブラウザ名と確認状況を記載していない。
動画を「できます」とだけ案内すると、未確認環境で同等保証に読める。対応対象と動作確認済みを分けて書く。

## 3. セキュリティ面で確認できた良い境界

今回の確認で、次は意図どおり実装・文書化されていた。

- managed remote server は loopback 待受を前提とし、外部公開を Tailscale Serve に分離している。
- PIN は Argon2id hash、session は HMAC 署名、Cookie は `HttpOnly` / `SameSite=Lax`、
  HTTPS proxy 判定時は `Secure` になる。
- named pipe は current-user の DACL と `PIPE_REJECT_REMOTE_CLIENTS` を持つ
  (`src/remote_ipc/pipe.rs:3592-3685`)。
- archive subresource は `..`、絶対 path、drive-qualified path、Windows alias を拒否する。
- remote-web と Core の両方で path / 実在 / 種別を再検証し、IPC 境界で片側の検証を信頼していない。
- write は remote ownership を Core UI 適用直前にも確認し、別 context へ遅延 write を適用しないテストがある。
- 診断ログは URL query の path、PIN、Bearer、`Tailscale-User-*` の値を残さない。
- frontend source で `innerHTML` / `insertAdjacentHTML` / `eval` を使わず、表示文字列を DOM API で構築している。
- service worker はユーザー media を cache せず、offline shell だけを扱う。
- 「全ドライブの mIV 表示可能ファイルが対象」「操作端末は同時に 1 台」「file 操作と tag 更新は不可」
  はマニュアルに明記されている。

したがって、favorite/root allowlist のような保証できない境界を追加する、二重に DB を開く、
UI thread で I/O する、といった方向へ戻す必要はない。

## 4. 開発セッションへの推奨分割（初回レビュー時）

互いに独立してレビューしやすい順序は次のとおり。

1. **入力 / 接続の即時 blocker**: RR-01 と RR-02。小さい修正で利用不能経路を閉じる。
2. **セキュリティ設計判断**: RR-03。UNC を維持するか capability 化するかを先に決める。
3. **認証 hardening**: RR-05 と RR-06。logout / 全失効と response finalization を別 commit にする。
4. **一覧契約**: RR-04。ページングを実装するか、v3.0.0 の明示制限にするかを決める。
5. **マニュアル整合**: RR-07〜RR-10。上記の仕様決定後に 2 つのマニュアルを同時更新する。

RR-03 は他より大きい。短期 workaround として UNC を黙って拒否すると既存機能を落とすため、
利用者判断なしに実装しない。

## 5. 初回レビューで実行した確認

```
node --test
# 359 passed, 0 failed

cargo test -p mimageviewer-remote
# 107 passed, 0 failed, 1 ignored
# ignored: managed test sandbox で local named-pipe connection が拒否される既知のテスト

cargo test -p mimageviewer-ipc
# 33 passed, 0 failed

cargo test -p mimageviewer --lib remote_ipc
# 218 passed, 0 failed, 5291 filtered out
```

`cargo audit` / `cargo deny` はこの環境に入っておらず、依存 crate の advisory scan は未実施。
ツールを勝手に install して利用者環境を変更していない。

## 6. 初回レビューの未実施 / 限界

- 実 Tailscale Serve、iPhone / iPad / Android、SMB server を使った動的侵入テストは行っていない。
- 通常 profile / portable smoke の mImageViewer は起動していない。
- release packaging、署名、実配布物の asset 展開、FFmpeg の実動画再生は今回の静的レビュー範囲外。
- 依存関係 advisory scan は前節の理由で未実施。
- 既知の先読み / queue 性能問題と、開始時に存在した PDF timing instrumentation は評価対象から外した。

## 7. リリース判定チェックリスト

- [x] RR-01: 英字・記号 PIN でスマホ login できる
- [x] RR-02: non-root Serve handler を対応済みと誤表示しない
- [x] RR-03: UNC / device namespace の信頼境界を決定し、コード・テスト・マニュアルを一致させる
- [x] RR-04: 1000 件超をページングする、または v3.0.0 の明示制限として承認する
- [x] RR-05: local logout と全 session 失効の運用がある（基本機構）
- [x] RR-06: 全 HTTP response が frame embedding を拒否する
- [x] RR-07: Tailscale 到達性の説明が node sharing / ACL を含めて正確である
- [x] RR-08〜10: 検索範囲、補正 subset、ブラウザ検証状況を両マニュアルへ反映する
- [x] RR-R1: 全端末 logout の完了を dialog close で失わず、本体 session を必ず drain する
- [x] RR-R2: 同時発行 Cookie に nonce を持たせ、端末/session identity を分離する
- [ ] dependency advisory scan を release gate で実行する
- [ ] disposable portable smoke と、必要な実端末 smoke を別途完了する
