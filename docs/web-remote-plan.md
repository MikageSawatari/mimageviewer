# リモート閲覧 (Web) 機能 計画書

v3.0.0 の目玉として、外出先のスマートフォン / タブレット / PC のブラウザから
自宅 PC の mIV ライブラリを閲覧できるようにする。本書がこの機能の正本。

- ブランチ: `web-remote` (worktree: `C:\home\mimageviewer-web`)
- 実装: Codex Sol (xhigh) / レビュー・統合: ClaudeCode / 実機検証: ユーザー
- 現在のフェーズ: **動画ストリーミング増分 7/7 (第 1 段の実装完了、実機検証待ち)**

---

## 1. 位置づけと前提

- **想定利用者は「自分」**。外出中に自分の端末から自分のライブラリを見る用途に限定する。
  複数ユーザーでの共有・公開は機能としてもドキュメントとしても扱わない。
- **主戦場はスマートフォン**。UI はタッチ操作優先で設計する (スワイプ・タップゾーン・
  ピンチズーム)。マウス操作はその上位互換として自然に成立する範囲で対応する。
- **外部接続の経路はアプリの責務外**。Tailscale / Cloudflare Tunnel / ポート開放は
  ユーザーが選ぶ。mIV は HTTP サーバを提供し、将来的に接続診断で選択を支援する。
- 既定は無効。明示的にオプトインしたときだけサーバが起動する。

## 2. 全体アーキテクチャ (確定事項)

```
[ブラウザ]  ──HTTP──▶  [mimageviewer-remote.exe]  ──IPC──▶  [mimageviewer-core.exe]
                         (crates/remote-web)                   (本体・単一 writer)
                              │
                              └─ read-only で直接読む: 実メディア / settings DB
```

### 2.1 読み書きの分離 (重要な不変条件)

| 種別 | 担当 | 理由 |
|---|---|---|
| 読み取り (画像・動画バイト) | remote-web が **直接** read-only で読む | HTTP の range / 画像配信を本体 UI から分離する |
| 通常フォルダ一覧 | **IPC → 本体** | 表示対象・順序・sidecar 吸収・重複除去を本体の production materializer 1 箇所に固定する (§12.15) |
| サムネイル参照・生成 | **IPC → 本体** | catalog の実態が当初想定と異なったため。本体の既存生成経路とキャッシュ方針を一元利用する (§9) |
| 書き込み (読書履歴・ブックマーク・見開き・トリム・タグ・レーティング) | **必ず IPC → 本体** | 全永続ストアの writer を本体 1 つに固定する |
| 重い生成 (PDF レンダ・AI アップスケール・カラー化・補正合成) | **IPC → 本体** | PDFium プール・ONNX セッション・GPU をステートフルに保持しているのは本体 |

SQLite は DB ごとに journal 設定が異なる。特に `spread.db` は WAL / `busy_timeout` を設定せず、
`App` が起動時に開いた接続を保持するため、IPC worker の第2接続からは書かない。
「本体が唯一の writer / remote-web は reader」を維持し、書き込みは App 所有ハンドルへ渡す。
**この境界を崩す変更を入れないこと。**

remote-web 専用サムネイルキャッシュは §9 の縦串増分で撤去した。settings / catalog の writer は
引き続き本体だけであり、remote-web はこれらへ書き込まない。

### 2.2 セッションと排他

本体に単一の remote session owner を置く。ブラウザは認証後に
`POST /api/session/acquire` で確認なしに操作権を取得し、本体は「リモート接続中」ダイアログを
出して main / fullscreen / detached / native presenter の通常入力をロックする。操作者は常に
1 人であり、2 台目やローカルとの競合は**後から操作した側が勝つ**。表示フォルダ等の状態同期はしない。

- ダイアログには `tailscale status --json` の `Peer.TailscaleIPs` と接続元 IP の照合結果から
  direct / relay と対向端末名を表示する。CLI 失敗・未照合時は「取得できません」と明示する
- 接続時刻、経過時間、IPC lifecycle が持つ現在処理、要求/完了/失敗/実行中/待機件数を表示する
- ローカルの「切断する」は常に即時解放する。以後の IPC/API は「ローカルで使用中」を返すが、
  ブラウザで次のコマンドが発火すると acquire を自動実行して確認なしに再取得する
- ブラウザごとのランダム client ID を各 API から IPC まで運び、別端末の要求を同じ
  remote-web process の owner として混同しない。保持できる owner は常に 1 client だけで、
  2 台目の操作は確認なしに owner を置き換える。旧 owner は「別の端末で使用中です」と表示し、
  次の操作で acquire を送り直して奪い返す。client ID は認証 credential には使わない。通常の
  `fetch` が送る `X-mIV-Remote-Client` を、認証済み `miv_remote_session` cookie ごとの
  owner identity として remote-web 内で固定する。native HLS の `<video>` や header を付けない
  stop は同じ cookie から owner を復元する。cookie 認証に失敗した要求は従来どおり
  fail-closed guard で 401 とし、owner 解決へ進めない。
  認証後の全画面では左上の小さな badge に「操作中」または
  「別の端末が操作中 (操作すると取得します)」を常時表示し、確認や入力 blocking は行わない
- session owner が変わった時、本体は既存の media pause、slideshow stop、owner-scoped native
  pending cancel を通して動画・音声・音楽ビュー・スライドショー・GIF/APNG・連続送りを停止する。
  player、停止位置、main/fullscreen/detached の window 構成は保持し、操作権返却時も自動再開しない
- ブラウザは 30 秒ごとに `POST /api/session/ping` を送り、直近の利用者入力と video/audio 再生中を
  通知する。通常の IPC 要求と remote-web が直接処理する一覧/画像 API も活動として数える
- 生存タイムアウトは ping/API が 60 秒無い場合、放置タイムアウトは「利用者操作なし、かつ
  再生なし」が 10 分続いた場合。再生中は放置タイムアウトだけを抑止し、生存確認は継続する
- active 中は watchdog thread が `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` を保持し、解放時に
  `ES_CONTINUOUS` へ戻す
- ローカルへ操作権が戻った瞬間、読書履歴・レーティング・ブックマーク・スマートフォルダ・
  通常フォルダの既存「再読み込み」入口を 1 回呼ぶ。fullscreen 中は item identity を保持し、
  一覧再構築後に同じ item を既存 `open_fullscreen` 経路で開き直す。再 open する media / animated
  image には paused 状態を引き継ぎ、利用者が再生操作を行うまで動かさない
- **その鏡像**として、リモートが操作権を取得した瞬間、端末は cache を破棄し、home 画面の
  データ (お気に入り / 場所 / スマートフォルダ定義) を取り直す。**この排他があるからこそ、
  セッション取得が「本体側で何か変わったかもしれない」の完全な信号になる** — 本体は
  リモートが操作権を持つ間は変更できないので、本体側の変更は必ず切断を挟むため。
  本体側の変更を `remote_state_generation` で拾おうとしないこと (§12.16 冒頭の表)

### 2.3 既存資産の再利用状況

| 対象 | 状況 |
|---|---|
| サムネイル | remote-web は `絶対 path + subresource + optional source_address + target_px` を本体へ IPC 中継する。本体が catalog 参照、画像の既存生成経路、動画の pin / sidecar / Shell 優先順位を担当する (§9) |
| 閲覧起点 | お気に入り・スマートフォルダ・場所は従来どおり入口として表示するが、アクセス境界にはしない。リモートから読める範囲は mIV 本体と同じ |
| 補正・回転・トリム・モザイク・消しゴム・ローカル調整・コミック注釈 | `books::BookPageSource::Composited` + `BakedEditSnapshot` に**ヘッドレス合成が既にある**。入力も File / ZipEntry / PdfPage をカバー済み |
| AI アップスケール・カラー化 | `page_requires_full_composite` から display-only として**意図的に除外**されている。ヘッドレス経路への追加は**新規作業** |
| ZIP / PDF 列挙 | `zip_loader` / `pdf_loader` をそのまま利用 |
| 検索 | Tantivy / `fts_meta.db`。`favorite_id` 単位の絞り込みが既にある |

## 3. 設計制約 (全フェーズ共通)

### 3.1 パス表現とリモートから読める範囲

`RemoteAddress` は **絶対 path + subresource** とする。HTTP では絶対 path を必ずクエリ引数
`?path=...` で渡し、URL path や診断ログの details へ入れない。request log は記録前にクエリ全体を
落とす。ブラウザの hash route には利用者自身の端末内での履歴・復帰に必要な path を含める。

- リモートから読める範囲は、ネットワーク共有の共有名表記と device namespace を除き、
  **mIV 本体が開ける範囲と同じ**である。ネットワークドライブはドライブ文字の住所を保ったまま
  公開し、canonicalize が共有名を返す環境でも次の要求を拒否しない。お気に入り、スマートフォルダ、
  タグ、レーティング、ブックマーク、閲覧履歴、本棚をアクセス境界にはしない
- 保証できない範囲制限を保護として提示すると、利用者に検証されていない保証を信じさせるため、
  favorite / registered-root allowlist は置かない。過去の制限は攻撃を防がず正当な一覧を欠落させた
- 到達には tailnet 内の端末であることと PIN の両方が必要である。待受は 127.0.0.1 のままとし、
  外部からは `tailscale serve` 経由だけにする
- この判断は、有効化のたび接続ダイアログで「mIV が開ける画像・動画・PDF すべて」が対象になると
  明示し、利用者が有効化前に認識できることを前提とする。無効化時には警告を出さない
- path は NUL を含まないドライブ文字の絶対パスで、実在することを remote-web と本体の境界で検証する。
  I/O は canonicalize した値を使い、公開住所は通常表記へ戻す。ネットワークドライブの canonical が
  共有名になった場合だけ、字句正規化した呼び出し元のドライブ文字表記を公開住所に使う。
  ZIP entry / prefix の slash、drive、backslash、NUL、
  `..` component 検証と PDF page 範囲検証は維持する

### 3.2 認証

- ブラウザ認証にはユーザー設定の **6〜1024 文字の PIN / パスフレーズ**を使い、文字種は
  **U+0021〜U+007E の印字可能 ASCII だけ**に限定する。U+0020 の空白も許可しない。
  本体の「リモート接続」ダイアログで設定・更新し、Argon2id の salt 付き hash とランダムな
  セッション署名鍵だけを認証ファイルへ永続化する。平文 PIN は保持しない。
  この制限は、伏字の入力欄では IME 変換が意図どおり確定したか確認できないことと、Windows 本体と
  iPad 等のブラウザで Unicode 正規化形が異なると同じ見た目でも hash が一致せず、伏字のため
  誤入力と区別できない認証失敗になることを避けるためである。
  `mimageviewer-remote` の `--set-pin` は撤去し、書き手を本体 1 つに限定する
- ブラウザ側の PIN 入力欄は数字キーボードを要求せず、英字・記号へ切り替えられる既定の
  text キーボードを使う。自動大文字化とスペル補正は無効にし、唯一の設定入口である本体の共有
  validator で上記制限を強制する
- 認証ファイルは本体のデータディレクトリ直下の `remote-web-auth.json`
  (portable は `<exe_dir>/data/remote-web-auth.json`)。本体が temp file + rename で書き、
  remote-web は `--auth-file` で渡されたファイルを起動時に読むだけとする。
  PIN 未設定または認証ファイル破損時は本体 UI で未設定として扱い、有効化できない。
  remote-web も PIN 未設定時は従来どおり fail-closed で起動を拒否する
- PIN 検証は Argon2id の定数時間検証を使う。失敗回数はプロキシ経由で送信元を識別できない場合も
  効くようサーバ全体で数え、5 回失敗で 30 秒、以後の失敗は解除後に 60 秒、120 秒……と
  指数バックオフする。失敗時刻・接続元・累積失敗回数を診断ログへ記録する
- 成功時は HMAC-SHA256 署名付き HttpOnly / SameSite=Lax Cookie を発行する。通常は Max-Age 90 日、
  「この端末を記憶しない」選択時は Max-Age のないセッション Cookie とする。`Secure` は direct TLS
  または `X-Forwarded-Proto: https` を検出したリクエストでだけ付ける。`POST /api/auth/logout` は
  現在の remote session を typed drain へ移してから、発行時と同じ Path / HttpOnly / SameSite /
  Secure 属性と Max-Age=0 でその端末の Cookie を削除する
- curl 等の診断用には起動時生成の 256bit `Authorization: Bearer <token>` も残す。Bearer は
  定数時間比較し、PIN・hash・セッション署名値・Bearer をログへ出さない。認証失敗本文に内部情報を出さない
- 接続用 QR コードには URL だけを含め、PIN や Bearer は含めない。URL は `--url`、Tailscale の
  `--json` 状態、bind 先の順で決める。remote-web は確定 URL、`tailscale serve` 状態、`/` を占める
  既存 proxy 先、自分の proxy がある未対応サブパス、tailnet の HTTPS 証明書状態、接続キーの有効期限を
  protocol v47 の接続情報として
  本体へ通知する。判定は `Web` の `.ts.net` キーだけでなく、handler の `Proxy` が本体から渡された
  bind / port と一致し、かつ handler path が `/` であるかまで見る。HTTPS 証明書と期限も remote-web が
  `tailscale status --json` を解釈し、本体は JSON を独自解釈しない。
  PIN 設定状態は認証ファイルの所有者である本体が判定し、
  設定メニューの「リモート接続…」に QR、URL、即時反映する opt-in、受付状態、active session の
  有無に基づく二値の利用状況を表示する。排他的 owner なので端末台数は表示しない。
  remote-web はブラウザ要求と独立した常駐 worker で IPC を維持し、250 ms から 5 秒上限の指数
  backoff で再接続する。接続 / 再接続の handshake 完了時に接続情報を必ず再通知する。本体 UI は
  handshake 済み接続を URL 受信前から追跡し、準備中、版不一致、起動失敗、接続情報の受信中を区別する

### 3.2.1 core 所有の service lifecycle

core は local-only / current-user-only の named pipe を常設する。設定ダイアログの有効化 / 無効化
ボタンを押したフレームで opt-in を保存し、専用 owner worker へ希望状態を送る。worker は有効時に同じディレクトリの
`mimageviewer-remote.exe` を起動し、初期化済みの `data_dir` を `--data-dir` で渡す。無効化時の
kill / wait も worker 側で行い、UI thread を止めない。core が spawn した `Child` だけを終了対象とし、
プロセス名や port から外部起動分の所有を推定しない。
core は認証ファイルを `<data_dir>/remote-web-auth.json` に固定する。診断ログディレクトリは
解決済み `<data_dir>` の末尾名へ `-remote` を足した兄弟とする
(例: `%APPDATA%\mimageviewer-remote`、`<exe_dir>\data-remote`、
隔離実行の `...\dev-runtime\data-remote`)。これにより remote-web の書き込みを data directory
の外に保ちながら、`--data-dir` ごとの隔離と同時起動時のログ名前空間を維持する。
兄弟名を導出できない filesystem root は別の既定領域へ逃がさず、明示エラーで service を開始しない。
既定ポートは共有 crate の定数を正本とし、本体が所有して `--port` で子プロセスへ明示する。
子プロセスには解決済みの認証ファイルとログファイルを `--auth-file` / `--log` で渡し、
remote-web 側で保存場所を推測しない。PIN の hash 化とファイル書き込みも owner worker 上で行い、
有効中の変更では所有 child を再起動する。新しい session 署名鍵になるため既存端末は再認証する。
接続ダイアログの「すべての端末をログアウト」も同じ owner worker 上で、PIN hash を保ったまま
session 署名鍵だけを atomic に更新し、所有 child を再起動する。UI thread は認証ファイルを読まない。
stdout は起動時 Bearer token を含むため収集せず、stderr の安全な診断だけを本体 UI へ渡す。
管理用 marker を受けた remote は IPC を累積 15 秒復旧できなければ自ら終了し、core の強制終了でも
画像を配信する孤児を残さない。手動起動には marker が無く、従来どおり無期限に再接続を待つ。

### 3.3 バインドアドレス

- 既定は `127.0.0.1`。**既定で LAN に晒さない**
- 外部公開は `--bind` の明示指定を必要とする。起動ログに実際の bind 先を出す

### 3.4 依存の方針

- **HTTP サーバは `tiny_http`** を使う。本ワークスペースには async ランタイムが存在せず
  (`ureq` のみ)、tokio を持ち込む判断は現時点でしない。スレッドプールで捌く
- **フロントエンドにビルドステップを導入しない**。素の HTML + CSS + ES modules で書く。
  バンドラ / TypeScript の採用可否は PoC 完了後に判断する
- 新規コードは新規ファイルに置く。既存ファイルへの変更は最小のフック点に留める
  (master が 1 日 5,000 行ペースで動くため、衝突面積を構造的に減らす)
- read-only 不変条件は **remote-web が本体データディレクトリへ書き込まない**ことを指す。
  本体が所有する認証ファイルの読み取りは許可する。remote-web が書く診断ログは `--log` で指定された
  本体データディレクトリ外のパスだけを許可し、settings / catalog 等も従来どおり変更しない

## 4. PoC のスコープ (現在のフェーズ)

**目的は「実回線で実用になるか」を最短で確かめること。** 機能の網羅は目的ではない。

### 4.1 成果物

`crates/remote-web` (bin 名 `mimageviewer-remote`) を新規追加する。本体 lib
(`mimageviewer`) に依存してよい。

#### API (すべて認証必須)

| エンドポイント | 内容 |
|---|---|
| `GET /api/favorites` | お気に入り一覧 (id, 表示名, 絶対 path) を JSON で返す |
| `GET /api/list?path=<absolute>` | 本体の通常フォルダ一覧を IPC で取得する。種別・表示名・絶対 path・サイズ・mtime に加え、セルの `address` と吸収済み sidecar を含む `thumbnail_address` を返す |
| `GET /api/thumb?path=<absolute>&w=<px>[&thumbnail_source_path=<absolute>]` | 画像・フォルダ・動画のサムネイルを本体へ IPC 中継し WebP で返す。動画の sidecar 選択時も `path` は元動画を保ち、任意の source path を併送する。本体未接続時は 503 と利用者向け理由を返す |
| `GET /api/image-info?path=<absolute>` | EXIF 回転反映後の元画像寸法を返す。クライアントの実描画幅計算に使う |
| `GET /api/image?path=<absolute>&w=<px>` | 画像を `w` に合わせて縮小し WebP で返す。リサイズ不要・EXIF identity・ブラウザ対応形式なら元バイトを素通しする |
| `GET /` および静的ファイル | フロントエンドを配信 |

- サムネイルの catalog 参照・キー生成・生成・保存判定は本体の既存経路に集約する。remote-web は
  catalog の内部構造を知らず、専用サムネイル DB も持たない。通常画像・フォルダ代表・sidecar は
  既存 catalog 経路を使う。動画自身は pin → sidecar → Windows Shell の順で選び、pin / Shell 用の
  catalog key は新設しない
- 本体側でも絶対 path の構文・実在・canonicalize と実ファイル種別を再検証し、remote-web の検証結果を信頼しない
- 一覧の走査と materialize は本体 `scan_directory_with_settings` →
  `materialize_local_folder_listing` だけを使い、remote-web に別の列挙を持たない

#### フロントエンド (`crates/remote-web/web/`)

タッチ操作優先。以下だけでよい。

1. **お気に入り一覧** — タップでフォルダへ
2. **フォルダビュー** — サブフォルダとサムネイルのグリッド。**仮想スクロール必須**
   (数千件で破綻しないこと)。パンくずで上位へ戻れる
3. **画像ビュー** — タップで全画面。**左右スワイプで前後の画像**。ピンチズーム。
   表示モードは全体フィット (既定) / 幅フィット / 原寸 (100%)。端末の向き変更に追従
4. 元寸法・表示モード・viewport から求めた実描画幅に `devicePixelRatio` を掛けて `w` に渡す。
   縦長画像の全体フィットで viewport 全幅を要求しない。次の画像を 1 枚先読みする

### 4.2 PoC の非スコープ

以下は**入れない**。PoC の判断を遅らせるだけになる。

- 本体との IPC / セッションロック / スリープ抑止 (PoC は read-only 単体で動く)
- 書き込み系すべて (履歴・ブックマーク・タグ・レーティング・見開き・トリム)
- ZIP / PDF / 動画 / 音声
- 検索 / ファセット
- AI アップスケール / カラー化 / 補正の反映
- exe への埋め込み / 配布 / 本体 UI からの起動 / 接続診断
- 認証以外のセキュリティ強化 (レート制限・HTTPS 終端など)

オンデマンド生成は当初この非スコープに置いたが、実機ログと catalog DB の直接調査により
前提が成立しないことが分かったため PoC スコープへ昇格した (§8.2)。これは既存 mIV catalog への
書き込み解禁ではなく、外部パスの remote-web 専用キャッシュを追加する変更である。

### 4.3 受け入れ条件

**自動テスト**

- パス解決の単体テスト: `..` / 絶対パス / ドライブ指定 / 正規化後の root 脱出 /
  リンクによる脱出 をすべて拒否すること。正常系が root 配下で解決すること
- トークン検証の単体テスト: 未指定 / 不一致 / 長さ違い で 401 になること
- 一覧の分類テスト: 拡張子から種別が正しく決まること
- `cargo test -p mimageviewer-remote` が緑

**実機 (ユーザーが実施)**

- スマートフォンのブラウザで お気に入り → フォルダ → サムネイル一覧 → 画像全画面 →
  スワイプ前後 が一通り動く
- 数千件のフォルダでサムネイル一覧がスクロールできる
- 画像 1 枚の表示が実回線で体感 1 秒以内

### 4.4 PoC で測る値 (本来の目的)

実装完了後、ユーザーが実回線で以下を記録する。**この数値が次フェーズの判断材料**になる。

- `tailscale status` が `direct` か `relay` か
- 自宅回線の上り実測値
- サムネイル一覧の初回表示にかかる時間
- 画像 1 枚の表示にかかる時間 (5G / 公衆 Wi-Fi それぞれ)

## 5. 以降のフェーズ (PoC 完了後に着手)

1. **縦串** — IPC・セッションロック・認証の本実装、ZIP / PDF、本読み (見開き)、
   補正反映 (既存ヘッドレス合成の利用)、ブックマーク・読書履歴の読み書き
2. **仕上げ** — 動画・音声のストリーミング配信、検索 (Ctrl+S / F / G 相当)、
   タグ・レーティング、AI アップスケール・カラー化のヘッドレス化、
   接続診断ウィザード、exe 埋め込みと配布

**動画は PC 側で H.264 + AAC へ再エンコードし、HLS で配信する (2026-08-01 に方針変更)。**
正本は [web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md)。

当初は「トランスコードは実装しない。コンテナ非対応 (MKV 等) は remux で救い、コーデック
非対応 (HEVC / AV1 / WMV) は音声フォールバックに逃がす」としていたが、この方針では
**コーデック非対応**と**回線帯域不足**という 2 つのリスクが構造的に残るため撤回した。
remux と音声フォールバックはトランスコードの下位互換なので、どちらも実装しない。
音量ノーマライズもサーバ側で処理済みの PCM を送るため、Web 側の WebAudio GainNode は不要になる。

## 6. 運用ルール

- **リリースタグごとに master を本ブランチへ取り込む** (2〜4 日おき)。溜めない
- master と独立に価値がある変更 (例: AI アップスケール・カラー化のヘッドレス化) は、
  Web 固有部分と分けて先に master へ落とす
- worktree の `vendor/` は master から実体コピーしている。**junction を張らない**。
  撤収は必ず `scripts/safe-worktree-remove.ps1` 経由
- 本体 (`src/`) に触る変更を入れるときは、master 側の並行作業との衝突を避けるため
  変更点を最小に保ち、本書に触れた範囲を記録する

## 6.5 確定した方針 (2026-07-29)

### 6.5.1 接続経路と HTTPS

- **mIV は TLS を持たない。`tailscale serve` に委譲する。** tailscaled が自マシン上で TLS を
  終端し、`https://<マシン名>.<tailnet>.ts.net/` として正規証明書付きで公開される。
  通信は従来どおり WireGuard の直接経路を通り、Tailscale のサーバはデータ経路に入らない
  (`tailscale funnel` とは別物。Funnel は使わない)
- mIV 自身は `127.0.0.1` に bind する。LAN にも tailnet にも直接出さない
- **HTTPS が必要な理由は盗聴対策ではなく PWA と secure context**。Service Worker は
  HTTPS 必須であり、ホーム画面登録を成立させるために要る
- `tailscale serve --bg <port>` は **非管理者権限で実行できることを実機確認済み** (2026-07-29)。
  したがって mIV から通常権限の子プロセスとして設定できる。UAC 昇格は不要。
  接続ダイアログは実行するコマンドと、tailnet 内だけへ HTTPS で公開し TLS は Tailscale が処理する意味を
  先に表示し、利用者が「tailscale serve を設定する」を押したときだけ本体の owner worker が代行する
- `tailscale serve status --json` / `tailscale status --json` も **非管理者で読める**。
  `Self.DNSName` と serve の `Web` handler / proxy 先から接続 URL を自動組み立てる。
  外部ツールは、参照系 (`status --json` / `serve status --json`) は無条件で実行してよい。
  ローカル CLI で代行できる変更系は、実行内容と意味を説明したうえで利用者の明示ボタン操作を必要とする。
  一方、tailnet 全体の HTTPS 証明書とデバイス単位の接続キー有効期限は管理コンソールまたは
  管理者トークン付き API の設定であり、mIV は変更を代行せず、参照結果と管理コンソールへの案内だけを出す。
  これを外部ツール方針の第 3 区分とし、そもそもローカルで代行できない設定を変更操作に見せない
- 自前 TLS (`tailscale cert` で証明書ファイルを取得して mIV が終端する) は、プロキシのホップを
  減らせるが証明書更新の管理を抱える。**現時点では採用しない**。製品化フェーズの比較対象として残す

### 6.5.2 認証

- **PIN はユーザーが設定して永続化する。** 起動ごとのランダム生成にしない (外出先から
  接続するとき自宅のコンソールを読めないため)。Argon2id でハッシュ化、平文は保存しない
- 6 文字以上。未設定ならサーバは起動しない (fail-closed)
- 失敗回数による指数バックオフとロックアウト。**ロックアウトは全体で 1 カウンタ**
  (下記のとおり送信元アドレスが常にループバックのため、送信元ごとに分けても意味がない)
- 成功後は長寿命の署名付き HttpOnly Cookie。「この端末を記憶しない」を選ぶとセッション Cookie
- `Secure` 属性はリクエストごとに判定する (直接 TLS または `X-Forwarded-Proto=https` のときだけ)。
  常時付与すると `http://127.0.0.1` でのローカルテストが壊れる
- 起動時に接続 URL の QR をコンソール表示する。**QR に PIN や Bearer を含めない**

### 6.5.3 `tailscale serve` 越しのリクエスト実測 (2026-07-29)

実機アクセスのログから確認した事実。

| 項目 | 実測値 |
|---|---|
| `remote_addr` | 常に `127.0.0.1:<port>` (proxy 経由のため送信元の識別に使えない) |
| `X-Forwarded-Proto` | `https` (Cookie の `Secure` 判定に使える) |
| `X-Forwarded-For` | **付かない** |
| `Tailscale-User-Login` | **付く** (tailnet 利用者のメールアドレス) |
| `Tailscale-User-Name` / `-Profile-Pic` | 付く (Name は RFC 2047 エンコード) |

**将来の検討事項**: `Tailscale-User-Login` が使えるということは、`tailscale serve` 経由で
所有者本人と一致する場合に **PIN 入力自体を省略できる**可能性がある。ただしこのヘッダは
ループバックから届くため、同一マシン上のプロセスからは詐称し得る。採用するなら
「`127.0.0.1` にのみ bind している」ことを前提条件として明示する必要がある。
現時点では PIN を主防御として維持する。

### 6.5.4 サポート範囲

- **対象は「自分が管理する端末」から自分のライブラリを見る用途に限定する**
- 借用 PC / 他人の PC は**スコープ外**。tailnet に参加させると mIV のポートだけでなく
  tailnet 内の全マシンにネットワーク層で到達できてしまうため、リスクが機能の範囲を超える
- 複数ユーザーでの共有・公開は機能としてもドキュメントとしても扱わない

### 6.5.5 UI の方針

- **タッチで成立する形を土台にし、キーボード / マウスを上に乗せる。** 逆順 (マウス優先で
  作ってからタッチを足す) は、ホバー依存・小さいヒット領域・密なメニューを生み、
  後から作り直しになる
- 入力とコマンドの間に**コマンド層**を挟み、タッチ / マウス / キーボードを同じコマンドに
  落とす (mIV 本体の `KeyAction` + helper と同じ思想)
- 直接のジェスチャに割り当てるのは 3〜4 個までに絞り、**それ以外はメニューに集約**する
- レイアウトは同じ DOM・同じコマンドのまま **CSS のブレークポイントで切り替える**。
  スマートフォン用とノート PC 用の別実装を作らない
- **ホバーでしか到達できない機能を作らない**
- キー割り当ては mIV 本体の既定に寄せる。操作カスタマイズは初期実装では入れない

### 6.5.6 Web PoC のコマンドと固定キー割り当て

フロントエンドは `PointerEvent`、`keydown`、ホイール、ボタンを直接状態変更へ接続せず、
すべて `next_page` / `prev_page` / `zoom_in` / `zoom_out` / `zoom_reset` /
`toggle_menu` / `toggle_viewer_bars` / `back` / `parent_folder` / `open` 等の共通コマンドへ変換する。コマンド実行時の
telemetry には `input_source` (`touch` / `mouse` / `keyboard`) と入力の詳細を記録する。

キーは `docs/keymap.ini.default` の次の既定値へ合わせる。Web に同じ概念がない操作は追加せず、
ズームの `+` / `-` だけはブラウザ向けの固定補助キーとする (`FsZoomMode=Z` は本体の
長押しズームモードであり、Web の離散ズームとは意味が異なるため割り当てない)。PIN 等の
テキスト入力中はショートカットを一切発火させない。ボタンとメニューでは通常のキー操作を
優先し、`Esc` / `?` によるメニュー開閉だけを受け付ける。

| Web の文脈 | キー | コマンド | mIV 本体の対応 |
|---|---|---|---|
| 画像 | `→` / `↓` / `PageDown` | `next_page` | fullscreen の固定矢印送り / `FsFixedJumpNextNoRtl` |
| 画像 | `←` / `↑` / `PageUp` | `prev_page` | fullscreen の固定矢印送り / `FsFixedJumpPrevNoRtl` |
| 画像 | `Home` / `End` | `first_page` / `last_page` | `FsJumpFirst` / `FsJumpLast` |
| 画像 | `Backspace` / `Enter` / `Esc` | `back` (一覧へ) | `FsBackToList` / `FsClose` / 固定 Esc |
| 画像 | `+` / `-` | 拡大 / 縮小 | Web 固有の固定補助キー |
| 画像 | `0` / `Numpad0` | `fit_cycle` (全体 → 幅 → 原寸) | `FsFitModeCycle.1=0` / `.2=Numpad0` |
| 一覧 | `←` / `↑` / `→` / `↓` | グリッド選択移動 | グリッドの固定矢印移動 |
| 一覧 | `Enter` | `open_selected` | `GridOpenSelected` |
| 一覧 | `Backspace` / `Alt+↑` | `parent_folder` | `GridParentFolder` |
| 一覧 | `Alt+←` / `Alt+→` | `back` / `forward` | `GridHistoryBack` / `GridHistoryForward` |
| 一覧 | `PageUp` / `PageDown` | 1 画面分移動 | `GridPagePrev` / `GridPageNext` |
| 一覧 | `Home` / `End` | 先頭 / 末尾を選択 | `GridMoveFirst` / `GridMoveLast` |
| 共通 | `F11` | `toggle_fullscreen` | `FsToggleWindowMode` / `GridToggleMaximize` |
| 共通 | `?` | `toggle_menu` | `HelpShowContextShortcuts` |

入力経路とコマンドの対応は次のとおり。メニュー項目はスマートフォンでは下端シート、
ノート PC では右サイドパネルになるが、DOM とコマンドは共通で CSS だけを切り替える。

| 入力経路 | 直接操作 | 共通コマンド |
|---|---|---|
| touch / pen | 左右 34% の即時タップ、中央の即時タップ、上下左右スワイプ、ピンチ、拡大中パン | 前 / 次、上下バー切替、一覧 / メニュー / 前 / 次、ズーム、パン |
| mouse | 左右クリックゾーン、中央クリック、右クリック、通常ホイール、Ctrl/Cmd+ホイール | 前 / 次、上下バー切替、メニュー、前 / 次、ズーム |
| keyboard | 上表の固定キー | 対応する同一コマンド |
| 共通メニュー / ボタン | 各項目のクリックまたはタップ | 対応する同一コマンド |

画像領域は `touch-action: none`、`overscroll-behavior: contain`、
`-webkit-touch-callout: none`、`user-select: none` とする。他の操作ボタンは
`touch-action: manipulation` とする。iOS Safari の左端スワイプ自体は抑止できないため、
左端 32px から始まる自前スワイプ判定を無効にする。一覧からビューアを開くときと画像を送る
たびに `history.pushState` し、ブラウザの「戻る」は画像を 1 枚ずつ遡る。閉じるボタン、メニュー、
`Backspace` / `Enter` / `Esc` は state に保持した viewer depth を `history.go(-n)` でまとめて戻し、
何十枚読んだ後でも 1 アクションで一覧へ戻る。

## 7. PoC レビュー結果 (2026-07-29 / commit `4a68a730`)

### 7.1 検証済み

- **パス境界**: root と候補の双方を canonicalize し、Windows では大文字小文字を無視した
  **コンポーネント単位**の前方一致で判定している (文字列前方一致による `C:\foo` /
  `C:\foobar` の取り違えが起きない)。`..` / 絶対パス / ドライブ相対 / UNC を拒否し、
  junction による脱出のテストもある
- **認証**: `subtle` による定数時間比較 (長さ比較も定数時間)。**ルーティングより前**に
  認証しており fail-closed。早期終了を含む全応答の送信直前に `X-Content-Type-Options` /
  `Referrer-Policy` / `Content-Security-Policy: frame-ancestors 'none'` /
  `X-Frame-Options: DENY` を 1 回だけ付け、別ページからの frame 埋め込みを拒否する
- **静的配信**: URL → ファイル名は完全一致テーブルで、クライアントが与えた文字列を
  ファイル名に使っていない。静的配信側からの traversal は成立しない
- **read-only**: `SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only=ON`。書き込みが拒否される
  ことをテストで確認している
- **catalog identity**: canonicalize 後の `\\?\` 表記ではなく**論理パス**でキャッシュ DB を
  引いている。ここを取り違えると全サムネイルが 404 になる箇所で、正しく処理されている
- **キー規約の一致**: `catalog_db_path` は `catalog::db_path_for` + `path_key::normalize` /
  `normalize_keep_drive` と、`catalog_image_key` の drive-root 分岐は
  `use_full_path_cache_keys_for_folder` と一致することを確認済み
- **鮮度判定**: DB 行の mtime / file_size を実ファイルと突き合わせてから返す
- `src/` は未変更 (差分は `Cargo.toml` / `Cargo.lock` と新規クレートのみ)。テスト 24 件緑

### 7.2 対応が必要な事項

- **F1 (解消済み、§9)**: remote-web の catalog 直読みとキー規約複製を撤去し、サムネイルは
  catalog 参照を含めて本体へ IPC 中継する。プロトコル型は共有クレートへ集約した
- **F2 (解消済み)**: PIN 認証への変更時に、通常のセッション Cookie を Max-Age 90 日とした。
  端末に残したくない場合は画面からセッション Cookie を選べる
- **F3 (解消済み、§9 の副次効果)**: catalog blob の解釈も本体の既存 decoder へ戻したため、
  旧 JPEG 行を remote-web が WebP と誤認して 404 にする分岐自体を撤去した
- **F4 (解消済み)**: PIN 入力画面へ移行し、`?t=` 認証とリダイレクト自体を廃止した
- **F5 (計画どおりの省略)**: リサイズが必要な画像のサーバ側キャッシュは無く、リクエストごとに
  フルデコードする。F8 の素通し条件外でどこまで実用になるかは引き続き実機測定の対象

### 7.3 実機測定時の注意

**必ず release ビルドで測ること。** debug ビルドは画像デコードとリサイズが桁違いに遅く、
性能の測定値が意味を持たない。

## 8. 実機ログから見えた性能の事実 (2026-07-29)

`/api/image` の実測内訳 (896x1152 の画像、要求幅 1320)。

| 区間 | 実測 |
|---|---|
| `decode_ms` | 11〜20ms |
| `resize_ms` | 0.8ms (要求幅が元画像より大きいため実質リサイズなし) |
| **`webp_encode_ms`** | **54ms (支配的)** |

- **ボトルネックはデコードではなく WebP エンコード**だった。F5 (リサイズ済み画像の
  サーバ側キャッシュ) を検討する際は、キャッシュすべきは「エンコード済みバイト列」である
- **F8 (PoC で対応済み)**: 要求幅が元画像以上で、EXIF 回転が identity、かつ JPEG / PNG /
  WebP / GIF / BMP / AVIF の場合は Content-Type を元形式に合わせてファイルバイトを素通しする。
  HEIC / RAW 等と回転が必要な画像は従来どおりデコード → リサイズ → WebP 変換する
- より大きな画像 (20MP 級) での内訳は未計測。F8 の対象外となる縮小時はデコード側の比率が
  上がるはずなので、F5 の優先度はその実測を見てから最終判断する

### 8.2 catalog サムネイルの実態と PoC スコープ変更

実機ログでは catalog hit 10 件に対して miss 574 件で、miss のほぼすべてが `row_missing` だった。
直近 40 個の catalog DB を直接調べると 31 個が 0 行で、行がある DB も
`folderthumb:auto-v2:numeric:d3:<name>` が中心だった。個々の画像サムネイルは主にメモリ上で
生成・破棄され、永続 catalog に常在するという当初前提は成立しない。

このため一時的に §4.2 からオンデマンド生成を外し、参照順を catalog → remote-web 専用 SQLite
→ 生成へ変更した。その暫定実装と `--thumb-cache` は §9 の縦串増分で撤去済みである。現在は
catalog 参照を含む全サムネイル処理を IPC 経由で本体へ委譲する。

### 8.3 スマートフォンのグリッド寸法と描画 (2026-07-30)

実機では従来の「幅 420px 未満は最小セル幅 128pxを floor で割り、行高は常に 168px」という
計算により、390px 幅で 2 列・実セル幅約 179px・高さ168pxとなり、mIV のサムネイル比率も
無視されていた。ちらつき対策として入れた `content-visibility: auto` と固定の
`contain-intrinsic-size: 156px 168px` も実セル幅とは一致していなかった。ただし行トラックは
`grid-auto-rows` で明示されていたため、この intrinsic size は横長化の主因ではない。
WebKit 固有の描画省略経路を残す必要もないため撤去した。上から下へ枠が同化する直接原因は、
グリッド面が透明で `body` の上端だけ明るい radial gradient を透過していたことだった。

remote-web は settings.db の `thumb_aspect` / `thumb_aspect_auto` を read-only で読む。手動時は
本体 `ThumbAspect::height_ratio()` と同じ高さ / 幅比を使う。Auto 時はフォルダごとに
`auto_aspect_cache.db` を **read-only** で引き、行があればその確定値を使い、行または DB が
無ければ `App::effective_thumb_aspect` の「Auto 未確定は Square」と同じ 1:1 を使う。

`folder_key` は `src/auto_aspect_cache.rs` と同じ `path_key::normalize_keep_drive` 規約、すなわち
論理フォルダパスのドライブ文字を保持したまま小文字化し、バックスラッシュを `/` に統一する。
canonicalize 後の `\\?\` パスはキーに使わない。`aspect` INTEGER は本体の明示変換と同じく
`0=16:9, 1=3:2, 2=4:3, 3=1:1, 4=3:4, 5=2:3, 6=9:16` とする。設定 DB、Auto 比率 DB、
catalog のいずれにも remote-web から書き込まない。

列数は利用可能幅 `A`、gap `G`、目標セル幅 `T` から
`ceil((A + G) / (T + G))` とする。左右 inset を除き、スマートフォンは `T=132px, G=8px`、
中幅は `T=180px, G=12px`、広幅は `T=210px, G=12px`。Square の例では 390px = 3 列、
768px = 4 列、1280px = 6 列、1920px = 9 列となる。

列数 `N` の確定後、実セル幅 `C=(A-G×(N-1))/N`、プレビュー高
`P=round(C×実効高さ比)`、固定ラベル高 `L=38px`、タイル高 `H=P+L`、row pitch `R=H+G`
の順に一度だけ寸法を確定する。CSS grid の row 高も `H`、プレビューの flex-basis も `P`、
ラベル高も `L` に固定し、タイル内に未所有の余白を作らない。画面幅・向き変更時は先頭表示 item
をアンカーに全寸法を再計算する。

仮想スクロールは自然高 `行数×R` から、最大 offset を
`ceil((自然高-viewport高)/R)×R` にそろえて全体高さを先に確保し、可視行と前後 3 行だけを DOM に
置く。ホイールは 1 イベント 1 行、タッチ慣性・スクロールバードラッグは `scrollend` または
140ms の idle 後に最寄りの行へ snap する。

`content-visibility` / `contain-intrinsic-size` は撤去した。ちらつき対策は可視範囲 + overscan のみを
DOM に置く仮想化、セル DOM の再利用、テーマ色のプレースホルダ、transition 無効、
`decoding="async"`、`cache: "force-cache"`、`contain: layout paint style` を維持し、グリッド面は
単色背景にした。プレースホルダのグリフはサムネイル decode 成功時に親へ `thumb-loaded` を付けて
隠し、HTTP / decode 失敗時だけ表示する。

### 6.5.6 ターゲット端末の確定 (2026-07-29)

**Web クライアントはスマートフォン / タブレットを最優先とする。ノート PC 向けの
mIV ネイティブ・リモートクライアントは実装しない。**

判断の根拠:

- ノート PC には既に十分な代替がある。**リモートデスクトップ** (mIV のフル機能を
  そのまま操作できる)、**ネットワークドライブ** (Tailscale 越しの SMB / Taildrive を
  ドライブにマップすれば mIV は改修ゼロでローカルフォルダとして扱える)、
  **USB ドライブでのライブラリ持ち出し**
- 一方 **スマートフォンにはこれらの代替が無い**。Web クライアントの独自価値はここに集中している
- ネイティブ・リモートクライアント (ローカルキャッシュ実体化方式) は Rust 2,700〜5,400 行と
  見積もられ、上記の代替手段に対して見合わない

Windows Home では RDP のサーバ側を使えないが、この層も次の手段で埋まる。

- **ネットワークドライブ方式は edition 非依存**で、しかも mIV のフル UI が使える。
  ライブラリ閲覧という目的にはこちらの方が RDP より適している
- 画面転送が必要なら RustDesk / Parsec / Chrome リモートデスクトップ等の代替がある

したがって Home 利用者が取り残されることはなく、ネイティブクライアントを作る理由にはならない。

**この確定に伴う扱い:**

- ノート PC のブラウザは **「動作するが最適化対象外」**。凝ったレイアウトや多ペイン化はしない
- **キーボード / マウス対応 (§6.5.5 のコマンド層) は維持する。** 開発とテストの主戦場が
  PC ブラウザであること、キーボード付きタブレットが存在することが理由
- **重要度が上がる項目** (スマートフォンが唯一のターゲットになったため):
  PWA / ホーム画面登録、通信断からの復帰、iOS のバックグラウンド復帰時の状態保持、
  片手での到達性、従量課金回線への配慮 (画質を落とすトグル / 通信量表示)
- 動画は PC 側トランスコード + HLS 配信に一本化する (2026-08-01)。当初維持するとしていた
  fMP4 remux は、コンテナ非対応しか救えず帯域も元ファイルのままなので採らない。
  正本は [web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md)
- ユーザー向けマニュアルには「ノート PC からはリモートデスクトップやネットワークドライブ
  という選択肢がある」と正直に書く。信頼とサポート負荷削減の双方に効く

## 9. サムネイル生成の所在 — PoC の前提誤りと是正 (2026-07-29)

### 9.1 何が起きたか

PoC は「サムネイルは catalog に WebP で入っているので、読んで返すだけでよい」を前提に
設計した。実機テストでサムネイルがほとんど表示されず、診断ログと実 DB を調査した結果、
**この前提が誤りだったことが判明した**。

- 診断ログ: hit 10 件に対し miss 574 件。miss の内訳はほぼ全て `row_missing`
- 実 catalog DB: 直近 40 個のうち **31 個が 0 行**。行がある DB も
  `folderthumb:auto-v2:numeric:d3:<name>` (フォルダ代表) ばかりで、**個々の画像の行は
  ほとんど存在しない**

mIV は既定の `CachePolicy::Auto` ([thumb_loader.rs](../src/thumb_loader.rs) の `should_cache`)
で「生成が速いものは永続化しない」判断をするため、通常の画像サムネイルはメモリ上で
生成・破棄されている。catalog に常時揃っているという想定が成り立たない。

### 9.2 是正: 生成は本体が持つ

**サムネイル生成は remote-web に実装しない。IPC で本体に依頼する。**

remote-web 側で生成すると、本体が持つ以下をすべて複製することになる。F1 (catalog キー
規約の複製) の何十倍もの複製面積であり、silent drift のリスクが機能全体に及ぶ。

- TurboJPEG の DCT スケール縮小デコード / WIC (HEIC / AVIF / JXL / RAW) / Susie プラグイン
- ZIP / PDF ページのレンダリング
- フォルダ代表サムネイルの選定 (ソート順・探索深さ・手動ピン)
- 利用者が設定したサイズ・画質・キャッシュ方針
- 回転 DB の適用

これは計画書 §2.1 の「重い生成は IPC → 本体」に元から該当していた。catalog に揃っていると
誤認したために読み取り側へ分類してしまったのが原因である。

**採用する形**: サムネイルは **catalog 参照も含めて丸ごと本体に任せ、remote-web は中継だけ**
にする。これにより **F1 が解消する** (remote-web が catalog の内部構造を知らなくなる)。
副次的に、リモート閲覧が本体のキャッシュを温めるため、次にローカルで開いたときも速くなる。

### 9.3 暫定措置 (撤去済み)

commit `865d9c2a` で導入した remote-web 側のオンデマンド生成と専用 SQLite は、縦串増分で
撤去した。`thumb_cache.rs` 全体、`--thumb-cache`、`store.rs` のサムネイル生成・catalog 直読み、
`image_support.rs` のサムネイル用 helper は残していない。

IPC の型と版数は GUI / native runtime 非依存の `crates/remote-ipc` に集約する。接続時に
`PROTOCOL_VERSION` を照合し、不一致なら接続を拒否して client / server の双方の版数をログへ出す。
本体は起動中常に `PIPE_REJECT_REMOTE_CLIENTS` と current-user-only DACL を付けた local named pipe を
開き、受信と生成を UI thread 外の上限制御された worker で処理する。同名 pipe が既に存在する場合は
二重起動せずエラーを記録する。ネットワーク待受は opt-in で開始する remote exe だけが所有する。

実機初回測定では 1 要求 1 接続だったため、571 要求中 274 件が接続成立後の
`ipc_protocol_error` になった。直接原因は、旧 server の `PipeStream::flush` が空実装のまま、
応答 `WriteFile` の直後に `DisconnectNamedPipe` していたことだった。サーバ側 write が pipe
buffer への書き込みに成功しても、client が読む前に切断すると応答が欠損し、client 側だけが
response read / decode error になる。これは mIV ログに write failure が無いこと、生成所要時間後に
502 になること、reload ごとに成功分だけ catalog が温まることと一致する。
`ERROR_PIPE_BUSY` を含む connect 失敗は 503、worker queue 満杯も明示 503 なので、この 2 つは
502 の直接原因ではなかった。pipe の `Write::flush` は `FlushFileBuffers` を呼ぶ実装へ直した。
是正後の protocol v5 は 1 本の長寿命 duplex 接続上で
`Thumbnail` / `Home` / `Collection` / `Container` / `Page` の各 request / response を共通の
request id で多重化する。
remote-web は pending 要求を id で解決し、接続断では全 pending を失敗させて
自動再接続する。本体は 4 pipe instance を accept 待ちとして先に用意し、1 本を受け付けた直後に
補充する。worker queue 満杯時は接続を切らず、request id に対応する `Busy` を即時応答して
同じ接続上の後続要求 (特に軽量な Home) の読み取りを止めない。診断ログには
`ipc_stage` / `ipc_error_kind` / `ipc_os_error` /
`ipc_retry_count` / `ipc_retry_statuses` / `ipc_connection_id` を残し、次回測定では
復旧した一時エラーを含めて失敗段階を直接集計できる。

`ThumbnailRequest { address, source_address, target_px }` の `address` は §12 の共通アドレス型であり、本体側で
再度、絶対 path の構文・実在・canonicalize と実ファイル種別を検証する。通常画像とフォルダ代表は `thumb_loader::process_load_request`
(`load_one_cached`) へ渡し、catalog 参照、DCT / WIC / Susie、回転 DB、利用者のサイズ・画質、
`CacheDecision::from_settings` による保存判断を既存経路へ揃える。同一要求の同時到着は flight を
共有して重複生成しない。remote-web の `/api/thumb` はこの要求・応答を中継するだけで、IPC 往復
時間を `ipc_ms` として診断ログへ記録する。

本体未起動・pipe 未接続時も HTTP サーバ自体は起動する。`/api/thumb` は 503 と機械可読な
`miv_not_running` を返し、フロントは「mIV 本体が起動していません」と画面内に表示する。
仮想グリッドの tile は binding 世代と絶対 path + subresource の address identity を応答時に照合し、破棄時は fetch を abort する。
これにより scroll で detach / 再表示された tile に古い in-flight 応答を適用しない。
ブラウザからのサムネイル HTTP 要求は同時 4 件に制限し、ネットワーク失敗・502・一時的な 503
だけを 200 / 400 / 800 ms の指数 backoff で最大 3 回再試行する。404 / 422 と protocol 版不一致は
再試行しない。上限到達時は tile に「再試行上限」を表示する。

protocol v40 では動画要求の identity と sidecar source を分離した。動画 pin は `video_pins.db`
の WebP を最優先し、sidecar は通常画像と同じ catalog 経路、sidecar が無い場合だけ Windows
Shell を 1 回呼ぶ。Shell の GetImage が抽出待ちになり得る失敗は `NotReady` →
HTTP 503 `thumbnail_not_ready` として Heavy 枠を解放し、上記の bounded retry に渡す。
`WTS_E_FAILEDEXTRACTION` と GetImage 以外の段階は `GenerationFailed` → 422 として再試行しない。
pin / Shell は catalog へ保存せず、既存の `video_pins.db` / Windows thumbcache / HTTP 60 秒 cache
だけを使う。

旧 remote-web 生成経路が記録した `representative_missing` 50 件は、IPC 移管前の
2026-07-31 00:24–00:25 (JST) の記録であり、移管後ログには 0 件だった。移管後のフォルダ代表は
Web 独自探索を持たず `thumb_loader::process_load_request` 内の
`resolve_folder_thumb_image` (本体 UI と同じ sort / depth / pin 条件) を使う。その resolver が
代表を返さない場合は `NotFound` (HTTP 404) として明示し、通常画像の生成失敗 (422) と区別する。

### 9.4 この発見の位置づけ

PoC の目的は「実回線で実用になるか」の確認だったが、**設計前提の誤りを実装が深くなる前に
発見できた**点が最大の収穫となった。縦串フェーズを IPC から始める根拠でもある。

### 9.5 HTTP worker 枯渇と負荷分離 (2026-07-31)

実機で Home が「お気に入りを読み込んでいます」から進まなくなった。再起動直後の
`/api/favorites` は 27ms であり、停止の原因は DB ではなかった。HTTP worker 8 本すべてが
サムネイル IPC 待ちに入り、IPC を使わない要求まで受信後に処理できない worker 枯渇だった。
本体側では RAW (`.ORF`) 1.7 秒、JPEG 0.8 秒の decode が観測され、remote-web 側の IPC 応答期限も
120 秒だったため、重い依存先が HTTP server 全体を長時間止め得る構造になっていた。

remote-web の HTTP worker は 12 本とし、非待機型 admission gate で IPC 全体を 6、うち
サムネイル・container・page・集約一覧の重い要求を 4 に制限する。通常フォルダの `/api/list` は
全体 6 枠だけを使い heavy 4 枠を消費しないため、heavy が上限でも一覧 / Home 用に 2 枠が残る。
IPC がすべて待機中でも少なくとも 6 worker は `/api/favorites` / 認証等の IPC-free endpoint に
使える。上限超過は queue 待ちせず HTTP 503 + `Retry-After: 1` と
`ipc_status=admission_busy` を返す。ブラウザの thumbnail 再試行がこの応答を処理する。

IPC 応答期限は 10 秒とする。実測の最も遅い単発 RAW decode 1.7 秒に約 6 倍の余裕があり、
本体側の remote heavy worker 2 本に Web 側の重い要求 4 件が並んでも通常は期限内に収まる一方、
異常要求が HTTP worker を分単位で保持しない値である。timeout は 503 +
`Retry-After: 1`、`ipc_status=response_read_timeout` として記録し、同じ HTTP worker 内では
再試行しない。多重化接続では期限切れ request id を tombstone として保持し、遅着応答だけを
読み捨てるため、1 件の timeout が他の進行中要求や接続全体を巻き添えにしない。

本体 IPC は Home と通常フォルダ一覧を専用 queue + 1 worker に分離した。サムネイルと集約評価は
別の heavy queue で処理し、利用者設定 worker 数の半分かつ最大 2 worker
(`clamp(configured/2, 1, 2)`) に制限する。
remote IPC の読み取り・生成は UI thread を使わず、ローカル表示用 worker とも別である。
§12.8 の短い App-owned 永続書き込みだけは UI thread で行う。CPU / disk の物理競合は
残るが、remote 由来の decode を最大 2 本に抑え、1 クライアントが利用者設定上限の全 worker を
追加消費しない。heavy queue 満杯時は pipe connection を切断も block もせず、プロトコル上の
`Busy` を返す。

### 9.6 持続接続の同期 I/O デッドロックとライフサイクル観測 (2026-07-31)

実機では handshake 成功後に Home / thumbnail の全要求が timeout し、本体は CPU idle、かつ要求
受信ログも無かった。原因は queue 分離ではなく、持続接続化した named pipe の同じ同期 handle を
reader thread と writer thread から使っていたことだった。handshake 後に reader の同期 `ReadFile`
が応答待ちへ入ると、その handle の `WriteFile` が直列化され、最初の要求自体が本体へ届かなかった。

サーバの `CreateNamedPipeW` と remote-web の `CreateFileW` はともに
`FILE_FLAG_OVERLAPPED` で開き、接続・read・write を要求ごとの `OVERLAPPED` + event で完了待ちする。
持続接続上では `WriteFile` の完了を送信完了とし、pending read と直列化し得る
`FlushFileBuffers` は使わない。Home 専用 1 worker、heavy 最大 2 worker、bounded queue と満杯時の
`Busy` 応答は維持する。

本体ログには connection id 付きの受理 / handshake / 切断、request id と種別、投入 queue、
worker の開始 / 完了 / outcome / 所要時間、queue wait、queued / active 件数、read / write の失敗段階
を記録する。実 pipe を使わない回帰テストでも、本番の dispatcher helper から Home queue、専用
worker、request id 付き応答までを往復させ、両端の pipe 作成 flag が overlapped のままであることを
固定する。

## 10. ホームと読み取り専用の集約ビュー (2026-07-31)

ホームは「お気に入り」「スマートフォルダ」「場所」の 3 タブとする。「場所」を初期表示にし、
場所タブは本体のフォルダバー「場所▼」と同じ列挙結果を使う。順序はドライブ一覧、閲覧履歴、
ブックマーク、レーティング ★1〜5、本棚フォルダ、区切り、デスクトップ／ピクチャ／ダウンロード、
区切り、各ドライブとする。各項目は本体と同じ `show_location_*` 設定に従い、ブックマークだけは
本体側にも専用の非表示設定がないため常に表示する。既知フォルダは取得できた実在フォルダだけを
載せ、`folder_tree::path_eq` で重複を除く。

列挙条件・順序・区切りは本体の共通 read model が所有する。remote-web は protocol v38 以降の Home
payload を順に描画するだけで、既知フォルダやドライブを独自に列挙しない。ドライブ一覧は本体 IPC
が `available_drives()` を `RemoteEntry::Folder` へ写像する collection として返し、既知フォルダと
各ドライブは Home payload の絶対 path を持つ folder entry から既存フォルダ route を開く。

スマートフォルダの定義一覧と、ドライブ一覧・読書履歴・レーティング・本棚・ブックマーク・
スマートフォルダの評価結果は本体 IPC から取得する。remote-web は DB の集約条件や並び順を
再実装しない。
本体側は次の既存 read model / evaluator を利用する。

- 読書履歴: `ReadingHistoryDb::list_recent`
- レーティング: `RatingDb::list_by_stars` → `rating_view::rating_row_to_view_row` →
  `rating_view::sort_rows(RatedAtDesc)`
- ブックマーク: `bookmark_browser::build_rows_readonly` で既存の行構築と
  `BookmarkViewSort::CreatedAtDesc` を共有する。内部 DB だけ read-only open に切り替える
- スマートフォルダ: `app::smart_folder` の候補走査、metadata filter、表示順計算をそのまま使う
- 本棚: `Settings::books_root_path` に対して `books::list_books` を使う

`/api/home` は保存済みスマートフォルダの ID / 名前と、共通 read model が確定した「場所」の項目だけを
返し、ファイルシステム走査や各集約ビューの内容取得は行わない。`scan_smart_folder` は利用者が
該当スマートフォルダを開いて `/api/collection` を要求した時だけ実行する。読書履歴は本体設定由来の
上限 (最大 1000) を既に持つ。リモートのコレクション項目は最大 100,000 件とし、さらに直列化後の
`entries` と `page_groups` の反復部分を 40 MiB 以内へ制限する。レーティング・本棚・ブックマーク・スマートフォルダも同じ
上限と byte 予算を使い、`truncated` / `entry_limit` を返して Web 画面に打ち切りを表示する。byte
予算で打ち切った場合の `entry_limit` は実際に返した件数とする。現段階ではページングは実装しない。

IPC の `RemoteEntry` は canonicalize 済みの絶対 `path`、表示名、種別、進捗・レーティング等の
表示用メタデータを持つ。候補は favorite との包含関係では落とさず、欠落して canonicalize できない
場合だけ元の絶対 path を保つ。開く時点で remote-web と本体が実在・種別を再検証する。

HTTP は認証必須の `GET /api/home` と `GET /api/collection` を追加する。集約一覧の通常画像・
フォルダは既存 `/api/thumb?path=<absolute>&w=<px>` を使う。ZIP / PDF は §12 の増分で
コンテナ内ページまで閲覧可能になった。動画・音声・変換アーカイブの再生、および
読書履歴・レーティング・ブックマーク等への書き込みは後続のセッションロック設計と一緒に行う。

### 10.1 集約コレクションを保った画像閲覧 (protocol v49, 2026-08-13)

ドライブ一覧・閲覧履歴・ブックマーク・本棚・レーティング・スマートフォルダの画像を開くときは、
実親フォルダの container へ移動せず、取得済み collection の `entries` / `images` を viewer 文脈として
保つ。フォルダ、ZIP / PDF、動画、音声の open route は従来どおりである。

`CollectionRequest` は session 中の `spread_mode` / `reading_direction` と縦持ち用
`force_single_page` を受け取る。collection には安定した container key がないため `spread.db` へは
保存せず、指定がない初回は `SpreadRestoreDefaults::NON_BOOK` を使う。本体は collection 全体から
画像だけを選び、`ui_fullscreen::build_remote_spread_page_groups` で address-based `page_groups` を
生成する。横長判定は画像を実親フォルダごとにまとめ、各親の catalog を 1 回だけ read-only open
して `load_source_dims` を一括取得する。さらに全画像の保存済み回転を既存ページキーで一括取得し、
通常コンテナ / fullscreen と共通の回転後横長判定へ渡す。旧行の寸法が `NULL` の画像だけ
`load_one` の thumbnail 寸法へ fallback し、未取得は縦長扱いとする。親の処理ごとに catalog と
寸法 map を破棄する。

Web は応答をコンテナと共通の `setContainerPageGroups` へ渡し、見開き変更・読み方向変更・縦持ち
Single の変更時には `/api/collection` を再取得する。collection の見開き指定はその閲覧 session にだけ
残る。画像 URL は `#collection/<kind>[/<id>]/image/<encoded-path>` とし、再読込では collection を
読み直して同じ address の画像を直接開く。画像が一覧から消えていた場合はエラー画面にせず、その
collection 一覧へ戻す。

## 11. 通常運用と並行する検証用インスタンス (2026-07-31)

単一インスタンスの意味は「1 build flavor につき1プロセス」ではなく、**1 data directory につき
1プロセス**とする。解決・正規化した data directory が通常版または portable 版の既定値と一致する
場合、mutex、activate event、open-path pipe、installer shutdown event は従来のリテラル名を
そのまま使う。異なる場合だけ、正規化 path の SHA-256 先頭64bitを `_data_<16 hex>` として4名
すべてへ付加する。これにより `--data-dir` だけで通常運用中のmIVと検証用mIVを同時起動でき、
同じ隔離data directoryを指定した2プロセスは従来どおり既存側をactivateして後発側が終了する。

検証用 core は次のように起動する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe `
  -ArgumentList '--data-dir', '.\target\dev-runtime\data'
```

remote-webにも同じdata directoryを渡し、同じお気に入り・設定を読む。

```powershell
Start-Process -FilePath .\target\remote-home-release\release\mimageviewer-remote.exe `
  -ArgumentList '--data-dir', '.\target\dev-runtime\data'
```

remote thumbnail IPC の `\\.\pipe\mimageviewer-remote-thumbnail` はremote-web側の接続先との互換性を
保つため固定のままとする。named pipe は常設なので、異なる data directory の本体を 2 つ起動した
場合も先発側だけが remote-web の接続先になる。後発側は `FILE_FLAG_FIRST_PIPE_INSTANCE` による
作成失敗を stderr と各data directoryの `logs/mimageviewer.log` に記録し、GUI本体自体は起動を
継続する。隔離側の remote を検証するときは、通常運用側を先に終了して pipe owner を一意にする。

## 12. ZIP / PDF のリモート閲覧 (2026-07-31)

### 12.1 共通アドレスと境界検証

`crates/remote-ipc` の `RemoteAddress` を本体と remote-web の唯一のアドレス表現とする。
実ファイルは canonicalize 済みの絶対 `path` で表し、
`RemoteSubresource` に `File`、`ZipDirectory { prefix }`、
`ZipEntry { entry_name }`、`PdfPage { page_number }` を持つ。ZIP entry / prefix は
`/` 区切りの相対表現だけを許し、先頭 slash、drive 指定、backslash、NUL、
`..` component を両プロセスの共通検証で拒否する。PDF page は 0-origin とし、本体が実際に
列挙した page count 未満であることを確認してからレンダリングする。

remote-web は IPC 前に NUL のない絶対 path、実在、canonicalize、内部アドレス構文を検証する。
本体も同じ構文検証の後に `remote_ipc::path_guard::resolve_existing` を通す。したがって
remote-web の検証を迂回しても、相対・不存在 path、悪性 ZIP entry、範囲外 PDF page は本体境界で
再度拒否される。絶対 path は IPC 応答とブラウザ hash route に含め、HTTP では query にだけ入れる。
request log は query を保存しない。

動画ストリーミングの実ファイルも同じ `RemoteAddress::File` と二重検証を通す。remote-web と
本体はそれぞれ絶対 path の実在・canonicalize・実ファイル種別を検証し、本体 IPC
境界では remote-web の判定を信頼しない。

通常フォルダ一覧も要求 address を両側で検証する。本体が列挙・canonicalize した各セルの
`address` と `thumbnail_address` を remote-web は組み替えずに返す。

### 12.2 本体既存経路の再利用

コンテナ列挙とページ生成は新規 `src/remote_ipc/container.rs` から次へ接続する。

- ZIP: `zip_loader::enumerate_image_entries_detailed` →
  `ZipTree::build` / `collapse_redundant` / `materialize_level(BOOK_READING_PAGE_ORDER)`
- nested ZIP のページ読み出し: `zip_loader::read_entry_bytes` に到達する
  `thumb_loader::process_load_request`
- PDF: 本体 PDF 一覧と同じ catalog `pdf_meta` (mtime / file size / password flag 一致時)
  → miss 時だけ `pdf_loader::enumerate_pages`、描画は `process_load_request` 内の
  `pdf_loader::render_page` (`container_page_meta` は ZIP / folder / converted archive 用)
- ZIP / PDF page thumbnail: `thumb_loader::process_load_request` と本体 catalog

remote-web は ZIP/PDF の列挙、ソート、decoder、catalog key を実装しない。PDF の remote 要求は
`LoadRequest.priority=false`、`context_epoch=0` として PDFium pool の `Normal` lane へ入り、
`Critical` 予約 worker を消費しない。保存済み PDF password が本体にあれば read-only で利用する。
保存値が無い、または復号値で開けない保護 PDF は `PasswordRequired` (HTTP 423) とし、
Web からの password 入力・保存は行わない。

### 12.3 HTTP / UI と件数・時間上限

認証必須の `GET /api/container` がコンテナの 1 階層を返し、`GET /api/page` がページを
turbojpeg q85 の JPEG で返す。`GET /api/thumb` はサムネイル用 WebP のまま、同じ address query（`entry` / `prefix` / `page` のいずれか）
へ拡張する。コンテナは最大 100,000 項目を返す。応答 IPC フレームの 64 MiB 上限に対して、
`entries` と同じ address を再掲する `page_groups` も含めた直列化後の概算を 40 MiB 以内へ制限し、
残り 24 MiB を応答 envelope・固定フィールド・将来拡張の余裕とする。件数または byte 予算の超過時は
`truncated=true` とし、byte 予算で打ち切った場合の `entry_limit` は実際に返した件数を画面に表示する。
ページングはこの増分では行わない。

フロントは ZIP/PDF を通常フォルダと同じ仮想グリッドで表示し、ページを既存の swipe、
pinch zoom、表示モード、keyboard / mouse command layer へ渡す。hash route は
`RemoteAddress` の JSON を percent encode した絶対 path + subresource を保持する。パンくずは favorite、
実フォルダ、コンテナ、ZIP 内 prefix を 1 DOM 上で組み立て、親の実フォルダへ戻れる。

HTTP heavy admission は従来どおり最大 4、IPC 応答期限は 10 秒とする。本体の既存実測は
PDF cold render 1.441 秒、通常の page render 0.7〜3 秒であり、remote heavy worker 2 本へ
同時要求 4 件を制限した条件では 10 秒以内に収まる見積もりである。remote は Critical 予約枠を
使わないため、ローカル UI の現在ページを優先できる。診断 JSONL には `container` / `page`
別に `ipc_ms`、`ipc_status`、retry 回数、entry 数、target / output 寸法、応答 byte 数を残す。

### 12.4 明示的な非スコープ

RAR / 7z / LZH の変換、リモートからの PDF password 入力、コンテナの 100,000 件超のページング、
読書履歴・rating・bookmark 等の書き込みは含めない。nested ZIP は本体の列挙文字列と
`read_entry_bytes` をそのまま使うため対応するが、nested RAR / 7z / LZH は変換増分まで扱わない。

アニメーション GIF / APNG / WebP は当面非対応とする。remote の `/api/page` は常に JPEG へ
変換するため静止画として表示される。対応には元バイトを返す別経路が必要で、先読み予算、cache key、
見開き合成のすべてへ影響するため、独立した増分として扱う。

### 12.5 実機で判明した PDF worker と初回表示の修正 (2026-07-31)

隔離 `--data-dir` の本体では PDFium pool の子が存在せず、PDF 要求が 1〜12 ms で
`ERROR_NO_DATA` (os error 232) になった。原因は pool が子を `--pdf-worker` だけで起動し、親の
`--data-dir` を継承していなかったことと、worker mode が通常の `data_dir::init` より前に分岐して
いたことだった。確認時の既定 APPDATA の DLL は 7,231,064 bytes、隔離 directory / 現 build の
DLL は 7,220,736 bytes で異なっていた。子は通常版の 5 worker が load 中の既定 DLL を現 build の
埋め込み DLL へ atomic replace しようとして失敗し、`run_worker_process` から即 return していた。
旧 pool は process spawn の成功だけで ready と数え、この終了を検知していなかった。

子へ `--data-dir <親の解決済み directory>` を明示し、worker 分岐でも最初に `data_dir::init` を
行う。さらに PDFium bind 完了後に worker が readiness protocol と実 data directory を stdout へ
返し、親は 5 秒以内に一致を確認できた子だけを pool へ登録する。`pdf-pool: init begin`、DLL の
成否、各 worker の stderr / ready / startup failure、最終 ready 数を本体 log に記録するため、
子の spawn、DLL bind、data directory 不一致を process 一覧なしで判別できる。

同時期の `remote-web-log.jsonl` を集計すると、`/api/list` 200 は 139 件で total p50 0.826 ms、
p95 7.611 ms、scan p50 0.100 ms、p95 0.231 msだった。73 項目の実例も total 1.200 ms / scan
0.090 msであり、folder 列挙は ZIP を開いていなかった。一方、visible thumbnail grid 82 件は
p50 69 ms、p95 687 ms、最大 34.011 秒、成功した thumbnail IPC 606 件は p50 28.959 ms、
p95 1.453 秒、最大 15.772 秒で、利用者が感じた待ちは後続の代表 thumbnail 生成だった。
folder grid は label DOM を先に作り、その次の animation frame から thumbnail fetch を始める。
`/api/list` log には ZIP / PDF 件数、`/api/thumb` log には path を出さない `source_kind`、client
telemetry には `folder_list` の fetch / first paint 時間を追加し、次回は container 別に測定する。

ZIP page の切替時は、元寸法未取得の address に viewport 寸法を仮定し、その暫定 layout をまだ
表示中の旧 `<img>` へ適用していたため、旧 page が一瞬拡大されていた。新 page は非表示の別
`<img>` で fetch / decode し、natural size から最終 layout を決定した後だけ旧 page と原子的に
差し替える。取得した寸法は address 単位で cache し、以後の要求幅計算にも使う。

PDFium の `PasswordError` は subprocess protocol を通しても失われない専用 marker に変換し、
本体 IPC の `PasswordRequired`、HTTP 423、画面の「パスワード保護」表示まで区別を維持する。

### 12.6 ページ先読みと表示待ちの短縮 (2026-07-31)

実機 `/api/page` 56 件は p50 938 ms、p95 1,740 ms、最大 1,973 ms、応答 p50 605 KiB で、
待ちの大部分は IPC 内の render + WebP encode だった。既存の container 先読みは現在ページの
取得開始直後に参照を保持しない `new Image()` へ次の1枚を設定するだけで、方向、abort、保持上限、
foreground との優先度を持っていなかった。

2026-08-04 の画質 mode 導入後は、表示と先読みを同じ長辺上限（8192 / 4096 / 2048 /
1024、既定 4096）で取得する。標準画質の実測約 1.2 MiB/ページに対し、進行方向 3 ページで
従来の約 3.6 MiB という帯域根拠を維持するため、前方 3 ページ、逆方向 1 ページへ変更した。
現在の上限は §14.6 を正とする。表示・先読み共通の要求長辺と先読み通信の同時最大 2 件は維持し、
窓は端末設定の既定 前方 8 / 逆方向 4、保持上限は表示中 + 有効な窓の枚数で決める。固定 byte の
開始ゲートは使わない。viewer を離れた時、別 target の foreground を要求した時、または計画から外れた時は
不要な fetch を中断する。

現行 protocol の `PageRequest.priority` は `Foreground` / `Prefetch` を共有型で表す。remote-web は
prefetch admission を2件に制限し、all / heavy の最終1枠を使用させない。本体も全接続合計の
prefetch queued + active を worker 数に応じて最大2件に制限し、常に remote heavy worker 1本を
foreground 用に残す。worker が1本なら prefetch は `Busy`、2本なら最大1件、3本なら最大2件とする。
既存 heavy 処理や queue があれば先読みは待たせず `Busy` にする。PDF pool
では foreground を `HighNormal`、prefetch を `Normal` に送り、ローカル UI の `Critical` 予約枠は
リモート要求に使用しない。

ページ本体と AI result はサムネイルの WebP encoder を共用せず、turbojpeg q85 の JPEG で返す。
画質設定は ☰ の「端末の設定」で選び、`local-settings.mjs` の version 1 aggregate に省略可能
field として保存する。旧保存値や不正値は標準 4096 へ正規化する。ピンチ拡大は取得済み画像の
CSS 変形だけを行い、追加取得しない。
画質変更時は `target_px` を含む page cache / AI result identity が変わるため、現在ページを同じ
画質で再取得し、進行中の AI job は既存 supersede 規則へ入る。

foreground が225 msを超えた時だけ、表示中の旧ページ中央へ高さ3 pxの半透明 indeterminate barを
重ねる。完了、失敗、abort で必ず消し、速い cache hit では一瞬も表示しない。閾値判定と先読み順、
要求長辺は `command-core.mjs` の純粋関数としてテストする。また `web/package.json` の ESM 指定と
Node 標準 `node:test` だけを使う最小 fake DOM テストを追加し、viewer の fetch → blob → decode →
layout → atomic replace を実行する。bundler、TypeScript、追加 package、build step は導入しない。

### 12.7 見開き表示 (2026-08-01)

見開きのページ組みは remote-web に複製しない。protocol v11 の `ContainerRequest` は
縦持ち表示用の `force_single_page` を本体へ渡す。`spread_mode` / `reading_direction` の
一時上書き欄は互換用に残すが、Web の明示操作は §12.8 の書き込み要求を使う。
本体は `spread.db` を read-only で参照し、コンテナ合成 key と root fallback を
`App::spread_container_key` / `App::apply_spread_for_key_with_fallback` と同じ規則で解決する。
初期値はコンテナ行、行が無ければ `Settings::default_spread_mode` とする。

本体のグループ列生成は `ui_fullscreen::build_image_reading_indices`、
`build_spread_display_units_with_predicates`、`is_spread_pairable_item`、
`SpreadDisplayUnit::spread_pair` を通す。したがって表紙の `has_cover` / `prefix_end` 位相、
画像として組めない item、横長ページの単独境界、RTL の左右配置を fullscreen と共有する。
横長判定は本体 catalog の既存 `source_dims`（無い場合は既存 thumbnail の寸法）と保存済み回転を
read-only で一括参照し、fullscreen / collection と共通の回転後横長判定を使う。寸法未確定時は
fullscreen と同じく非横長として扱う。応答の `page_groups` はグループ列を
読み進める順に並べ、各 `pages` は画面上の左→右順、`anchor` はそのグループの読み順先頭とする。
remote-web は受信した各 address を組み直さず、独自 sort もしない。

モードと固定キーは本体既定に合わせる。

| キー | モード |
|---|---|
| `1` | Single |
| `2` | Ltr |
| `3` | LtrCover |
| `4` | Rtl |
| `5` | RtlCover |

メニューの巡回順も `SpreadMode::next_in_spread_cycle` と同じ
Single → Ltr → LtrCover → Rtl → RtlCover とする。`spread_page_gap_px` は本体 settings から応答し、
CSS page gap へ反映する。viewport は `height > width` を縦持ちとし（正方形は横持ち側）、本体へ
端末ローカル設定が ON の時だけ `force_single_page=1` を再要求する。応答は保存由来の
`configured_spread_mode` と描画に使う
`effective_spread_mode=Single` を分けるため、向き変更で保存設定は変わらない。resize / orientation
change は 180 ms debounce 後にこの判定を再実行する。

`spread.db::get_direction` も本体側で同じ key / fallback から読み、`reading_direction` として応答する。
LTR / RTL の見開きモードは本体と同じく方向をそのモードへ揃え、Single は保存済み方向を維持する。
Web で RTL から Single へ切り替えた場合も、そのセッションの RTL を request に引き継ぐ。
RTL の横方向入力は画面上の方向を反転し、左 swipe / 左 tap zone / `ArrowLeft` を次グループ、
右 swipe / 右 tap zone / `ArrowRight` を前グループとする。上下矢印と PageUp / PageDown は
前後という意味を維持する。見開きの2画像は両方を別 DOM で fetch / decode し、最終寸法から
共通高さの layout を確定してから page layer を1回で差し替えるため、片側だけの表示や暫定倍率を
見せない。

先読みには現在グループの左右両 index を `visibleIndexes` として渡す。既存の進行方向8ページ、
逆方向1ページ、同時1件、LRU 12件 / 32 MiB、foreground 優先と abort 条件は維持する。
次の対象が2ページ組なら同じグループ寸法で各ページの要求幅を計算するため、先読み cache key と
foreground 要求が一致する。

実機で見開き全体が viewport へ収まらない事象を確認した。縮尺の純粋関数は2ページ合計を使って
いたが、DOM の `.viewer-pages` は計算済みの塊寸法を受け取らず `width: max-content` に依存して
いた。WebKit の flex intrinsic sizing では decode 後画像の intrinsic size が塊幅へ使われ得るため、
画像ごとの inline width と実際の flex container 幅が一致しなかった。塊の CSS width / height を
layout 結果に含め、page layer と各 flex item の width / height を明示する。

左右の高さが異なる場合は本体と同様、`H=max(left.height,right.height)` を共通の基準高とし、各幅を
`Wi=page.width*H/page.height` へ正規化する。全体 fit は
`scale=min((viewport.width-gap)/(Wleft+Wright), viewport.height/H)`、幅 fit は第1項だけ、原寸は
`scale=1` とする。塊幅は `Wleft*scale + gap + Wright*scale`、塊高は `H*scale`。各 `/api/page`
要求幅もこの最終 page width × DPR から決める。単ページ・横長単独・縦持ち Single は従来の
`viewerImageLayout` を通し、gap を0へ戻す。

前後移動が範囲外になる操作は無反応にせず、画面下部の半透明 status overlay を2.4秒表示する。
LTR は「先頭ページです」「最終ページです」。RTL はそれぞれ
「先頭ページです（右→左綴じ：次は左をタップ）」、
「最終ページです（右→左綴じ：前は右をタップ）」とし、overlay は pointer input を遮らない。

### 12.8 App 所有ハンドル経由の見開き書き込みと端末設定 (2026-08-01)

protocol v11 は書き込み種別を共有 `RemoteWriteRequest` enum へ集約し、最初の variant を
`SetSpread { address, spread_mode, reading_direction }` とする。種別ごとの bool / `Option` /
pending field は作らない。認証済み `POST /api/write` は remote-web の専用 bounded FIFO worker へ
渡る。要求内の各絶対 path は反復 `?path=...` query にだけ置き、JSON body は各 address の
`path_query_index` と subresource を持つ。remote-web は query から typed request を復元し、
body に path を直接置く旧形式や余分・重複 index を拒否する。本体側でも絶対 path の実在・
canonicalize、コンテナ種別を再検証する。worker は
DB を開かず、`SessionHandle` の bounded queue へ要求と one-shot 応答 channel を投入する。
既存 repaint context を起こし、UI thread の `App::poll_remote_session` が drain して
`App.spread_db` から mode / direction を1 transactionで保存し、成功または型付き失敗理由を
同じ経路で HTTP client まで返す。

UI が2秒以内に要求を claim しなければ pending→cancelled の原子的遷移を行い、以後 drain されても
適用せず `UiTimeout` (HTTP 504) を返す。期限内に UI が claim 済みなら曖昧な timeout 応答にはせず、
短い DB transaction の確定結果を待つ。session owner は IPC worker の投入前と UI drain 時の
両方で照合し、未取得 client は `NotAcquired`、別 client は `Superseded` として拒否する。
書き込みは畳み込まず単一 worker の FIFO で全要求へ個別応答する。Web は重複適用を避けるため
write の transport retry も行わない。連打時は全書き込みを順番に適用し、表示再取得だけを最後の
要求へまとめる。

キーは `spread_db::container_key_with_fallback` と `get_state_with_fallback` に集約し、App と remote
の両方が exact key (`zip_path + effective prefix`) と旧 root fallback を共有する。remote 書き込みも
同じ exact keyへ保存する。exact 行がまだ無い場合の reading flow は fallback 行から継承し、
fallback がある本で既定 mode を明示した場合も継承を上書きする exact 行を残す。
縦長画面の `force_single_page` は `effective_spread_mode` だけを Single にし、書き込み API を呼ばず
`configured_spread_mode` と DB 行を変更しない。操作権返却時は既存
`reload_after_remote_session_release` が現在 view を再読込するため、remote 保存値が本体表示へ反映される。

端末ごとの設定は server へ送らず、localStorage の単一 key
`miv-remote-local-settings` に JSON
`{"version":1,...}` aggregate として保存する。`portraitSinglePage` / `gestureHelpDismissed` / 画像画質 /
grid 列数に加え、診断の詳細段を端末ごとに明示 opt-in する `telemetryDebugDetails` を持つ。
既定は `portraitSinglePage=true`。☰ の「端末の設定」で OFF にすると縦持ちでも保存済み見開きを
維持する。parse / normalize / serialize は純粋関数とし、未知 version、壊れた JSON、型不正は
安全な既定値へ戻す。localStorage の取得・保存例外は wrapper で捕捉し、そのタブ内のメモリ値で
動作を継続する。項目追加時は version 方針を決め、`defaultLocalSettings`、
`normalizeLocalSettings`、設定画面、round-trip / 不正値テストを同じ aggregate objectへ追加する。
`telemetryDebugDetails` は version 1 への後方互換な加法で既定 OFF とし、ON 中は右下 HUD が
「詳細記録 ON」を常時表示して tap で設定へ戻る。

### 12.9 閲覧中の操作 UI (2026-08-01)

上下バーの表示状態はファイルや localStorage ではなく、フロントのセッション状態
`viewerBarsVisible` が所有する。初期値は表示で、中央 32% のタップとメニューの
「上下バーを表示 / 隠す」が同じ `toggle_viewer_bars` コマンドを通る。ページ移動や一覧からの
別画像オープンではリセットせず、タブを閉じると既定へ戻る。自動非表示 timer は持たない。

下バーの range は通常フォルダと ZIP / PDF のどちらも `pageGroups` を入力にする。Single では
1目盛り1ページ、見開きでは1目盛り1グループ（1見開き）とし、ラベルはグループ数でなく
`state.images` 上の実ページ番号を `12-13 / 240` の形式で出す。range の値は綴じ方向にかかわらず
読み順 group index と一致させ、LTR は native range の `dir=ltr`、RTL は `dir=rtl` を使う。これにより
RTL では最小値が物理右端、最大値が物理左端となり、thumb と塗りの向きを native control 内で揃える。
range は keyboard / ARIA の owner として残す。pointer は pointerup で tap / drag を確定し、tap なら
押下したトラック位置の絶対値へ移動、drag なら押下時の値からの相対移動を使う。この規則は動画・音声の
シークバー、静止画のページバー、音量、画像補正・表示トリムの各スライダーで共通とする。判定と絶対値・
相対値の計算は `command-core.mjs` の共通関数を使う。静止画のページバーではトラック全幅の移動を全
group 範囲へ正規化して step 1 で丸め、LTR は右移動、RTL は `dir` と同じ direction を使った左移動で
index を増やす。静止画のページバーでは pointer capture と既存の `touch-action: none` により、thumb 外から
始めた touch も追跡する。
range の `input` または pointer move 中は thumb とラベルだけを更新し、native `change` / pointerup で
session acquire を確認してから確定した1回だけ `changeImageTo` を呼ぶため、ドラッグ途中の画像 fetch /
decode は発生しない。実ページが1枚だけなら range だけを隠し、`1 / 1` の位置表示は維持する。位置、
相対移動量、実ページラベル、LTR / RTL の direction は `command-core.mjs` の純粋関数でテストする。

音量スライダーは端末判定にかかわらず常に表示する。iOS / iPadOS では Web の `volume` 代入値を
読み戻せても実際の音量には反映されないため、代入・読み戻しによる機能検出は行わない。iOS の明示的な
識別子、または Mac 系識別子と複数 touch point の組み合わせで iPadOS と判定した場合だけ、音量欄に
「iOS / iPadOS では、音量はデバイスのボタンで操作します。ここでの変更は反映されません。」と表示する。
この端末判定は注記だけを制御し、スライダーの表示や既存の音量コマンド経路は変えない。

上下左右 swipe は同じ純粋判定を使う。開始点から主軸が **52 CSS px を超え**、かつ直交軸の
**1.25倍を超えた**場合だけ成立する。左端32pxの browser edge gesture guard も従来どおり適用する。
上 swipe はメニュー、下 swipe は一覧、左右 swipe は綴じ方向に従う前後ページへ送る。
`scale > 1.01` では1本指移動を常に pan とし、swipe を発火しない。幅フィットの縦 drag は、
`scrollHeight > clientHeight` かつ指の移動方向へ `scrollTop` を実際に変えられる場合だけ scroll を
優先する。内容が収まる場合、上端から下へ引く場合、下端から上へ引く場合は pan 扱いにせず、
縦 swipe の一覧 / メニュー操作へ渡す。drag 中に一度でも実スクロールした場合は同じ gesture の
残りを pan とする。一方、幅フィット中の明確な横 swipe は従来のページ送りを維持する。

静止画は左右 34% の各 tap をページ送り、中央 32% の各 tap を上下バー切替として、gesture 判定が
`TAP` を返した同じ `pointerup` 処理内で即時実行する。中央だけを待たせる timer や tap pair 状態は
持たないため、中央を素早く2回叩いた場合もバー切替が2回起き、fit mode は変わらない。
`Original` (100%) は操作メニュー → 表示の「原寸 (100%)」から選べる。拡大・縮小は独自 pinch、
`scale > 1.01` の移動は1本指 pan を維持する。動画 viewer は従来どおり再生 / ±10秒 tap zone と
native zoom 抑止だけを持つ。

初回ヘルプは最初の画像表示完了後に modal で出し、左右 tap、中央 tap、上下 swipe と拡大中 pan を
示す。閉じた時点で aggregate local setting の `gestureHelpDismissed=true` を保存する。この field は
既存 version 1 への後方互換な加法とし、旧 JSON では既定 `false` を補う。localStorage が使えない
場合もタブ内のメモリ値は更新する。ビューアの ☰ →「操作方法を見る」から保存値に関係なく再表示できる。

メニューの操作ボタンは全画面サイズで2列にし、現在の階層と項目構成は §12.13 に従う。
キー表記は `shouldShowKeyboardShortcuts` で決める。`pointer: fine` は既定表示、
`pointer: coarse` は既定非表示とし、そのセッションで実際の `keydown` を1回観測した後は action 内の
キー hint と「有効なキー」一覧をともに表示する。

### 12.10 読書位置・レーティング・ブックマーク書き込み (protocol v12, 2026-08-01)

protocol v12 は §12.8 の単一 `RemoteWriteRequest` に
`RecordReadingProgress`、`SetRating`、`SetBookmark`、`GetItemState` を加える。別 IPC route、別 UI
queue、種別別 pending field は作らず、全要求が同じ bounded FIFO worker、session owner の投入前
確認、UI drain 時の再確認を通る。`GetItemState` はメニュー表示の正本値を App 所有ハンドルから
読むための request だが、順序と ownership を変えないため同じ enum / queue に置く。

`RecordReadingProgress` の page index / page number / page count と、履歴・resume の適用可否は
browser の申告を信用せず、本体 worker がローカル一覧と同じ走査、sort、カテゴリ配置、重複除外で
再計算する。通常フォルダの resume は `App::record_book_resume` と同じ全 items 上の 0-origin index、
ZIP / PDF も materialize 後の 0-origin index を `book_resume_writer` へ渡す。ネスト ZIP 内はローカルと
同じく resume を書かない。読書履歴は `App::record_reading_history` と同じく、表示順の全要素が
page data の場合だけ 1-origin position / count を持ち、ネスト ZIP では本自体の履歴だけを progress
無しで残す。履歴 key は `path_key::normalize_keep_drive`、resume key は既存 writer / DB 内の
`path_key::normalize` 規則をそのまま使い、`last_reading_history_touch` の同一本30秒間引きも共有する。
App の `reading_history_db` / `reading_history_writer` / `book_resume_db` / `book_resume_writer` 以外の
接続は開かない。

Web は画像の decode・表示完了を観測点にし、先頭位置を即時送信した後は30秒窓で最新位置だけを
保持する。窓満了時に最新1件を送り、ページ送りごとの request は作らない。送信は直列化し、一覧へ
戻る前には pure reducer の `flush` で未送信の最終位置を必ず1回生成し、その request の完了を待って
から route を変える。browser history で同じ本のページ間を移動するだけなら flush しない。

ページレーティング key / metadata はローカル `page_path_key` / `rating_meta_for_idx` と同じで、通常
画像は `adjustment_db::normalize_path`、ZIP entry と PDF page は
`adjustment_db::zip_entry_key` (`page_N`) を使う。0は解除、1〜5は設定で、App の `rating_db` を
`write_user_rating_shared` 経由で更新する。本ブックマークはローカル
`current_book_bookmark_draft` と同じ container key (`normalize_keep_drive`) と
`RelativePath` / `ArchiveEntry` / `PdfPage` identity を作り、App の既存
`book_bookmark_service` request/event へ渡す。UI thread は service 完了を待ってブロックせず、既存の
request id で元の FIFO 応答を完了する。

viewer の ☰ を開くたびに `GetItemState` で `rating_db` と `book_bookmark_service` から現在値を読む。
レーティング / ブックマーク変更後も成功・失敗を問わず同じ取得を再実行し、取得できなければ現在値を
不明として操作を無効化するため、要求値を正本のように残さない。session 解放後は既存
`reload_after_remote_session_release` が reading history / rating / bookmarks / smart folder / 通常一覧を
再読込するので、ローカルが操作を取り戻した時点で remote の変更が反映される。

### 12.11 通常フォルダの見開き表示 (protocol v13, 2026-08-01)

protocol v13 は `ContainerKind::Folder` を追加し、通常フォルダも ZIP / PDF と同じ
`GET /api/container` / `ContainerPayload` / `page_groups` 経路へ載せる。本体
`ContainerEngine` は要求された実在する絶対 path のディレクトリを走査した後、ローカル
`App::load_folder` と同じ `materialize_local_folder_listing` を呼ぶ。sort、カテゴリ配置、動画
sidecar、画像拡張子、実フォルダ対仮想コンテナの重複除外を別実装しない。
protocol v16 以降の `/api/list` もこの結果を `FolderListPayload` として受け取り、一覧描画は
その応答だけで確定する。`/api/list` と `/api/container` は同じ `AbortController` の所有下で
並行開始し、`loadFolder` が待つのは前者だけである。両方とも IPC だが、通常一覧は heavy queue
ではなく Home と同じ専用 queue を使い、container / thumbnail 待ちから分離する。

folder container の promise は現在フォルダの文脈に保持する。応答済みなら画像タップ時に即座に、
未完ならその既存 promise だけを待って `entries` / `page_groups` / spread mode / reading direction を
viewer state へ install する。したがって viewer のページ順の正本は引き続き `/api/container` であり、
ZIP / PDF と同じ `setContainerPageGroups`、`ImageViewer.loadGroup`、seek、端メッセージ、先読み cache
を通る。一覧を container 応答で再描画して順序を途中で入れ替えることはしない。

別フォルダ / ホーム / collection への遷移は既存 `requestController` を abort し、container install
直前にも controller、folder identity、現在の load owner がすべて一致することを確認する。古い
container 応答は fetch 実装が abort を無視して完了しても現在フォルダへ適用できない。画像を開く
直前に端末の向きが変わり `force_single_page` が背景要求と異なる場合だけ、古い要求を破棄して現在の
向きで `/api/container` を再要求する。

通常画像の `RemoteSubresource::File` は `/api/page` でも受け付け、本体
`thumb_loader::process_load_request` へ渡す。ページ address から親フォルダの container address を
復元できるため、hash history でページを進めても同じフォルダの `page_groups` を再取得できる。
フォルダの見開きキーも `spread_db::container_key_with_fallback(folder, [])` と
`get_state_with_fallback` を使い、Web の `SetSpread` 書き込みと読み取りを同じ規則にした。
端末ローカルの縦持ち設定は ZIP / PDF と同じ `force_single_page` だけを変え、保存済み
`configured_spread_mode` は変更しない。

グループ列は引き続き本体
`build_image_reading_indices` → `build_spread_display_units_with_predicates` →
`SpreadDisplayUnit::spread_pair` を通す。通常 `GridItem::Image` でも表紙位相、catalog
`source_dims` と保存済み回転による横長単独、RTL の画面左右順が ZIP / PDF と同じになる。純粋関数テストは
LtrCover、横長境界、RTL、縦持ち Single を通常画像 item で固定する。

読書位置と一覧の drift は、`materialize_local_folder_listing` を唯一の production
materializer とし、`ContainerEngine::recompute_folder_listing` をその薄い scan wrapper にすることで
構造的に防ぐ。regression test は同じ materialized result と実際の `FolderListPayload` の全 item の
種別 / path / 順序 / mtime / size / thumbnail source を比較する。素材は画像のみ、画像 + 動画 +
同名 sidecar、同 stem の複数画像拡張子、同名 ZIP + 実フォルダを含むため、リモート投影が item を
欠落・追加した場合や sidecar 出所を失った場合も失敗する。各画像の raw item index、page position、
page count の検証も維持する。

一覧へ戻る前の読書位置 flush は上限を短縮しない。現行は 30 秒 batch の最終値を生成して直列 write
tail の完了を待つため、遷移後も書き込みを継続する方式へ変えると session ownership の返却や
ページ context の切替と競合し、最終位置を失う可能性がある。UI claim 前の異常系には既存 2 秒
`UiTimeout` があり、通常は App 所有 writer への短い enqueue で完了するので、最大待ちより位置保存を
優先する判断を維持する。

### 12.12 PWA / standalone 起動 (2026-08-01)

`manifest.webmanifest` は配信位置からの相対 `start_url: "./"` / `scope: "./"`、
`display: "standalone"`、暗色 shell と同じ `#111318` の background / theme color を持つ。
通常アイコン 192 / 512 と maskable 192 / 512 を manifest へ登録し、iOS は 180 の
`apple-touch-icon`、`apple-mobile-web-app-capable=yes`、暗色に合わせた
`black-translucent` status bar を使う。認証後の hash router は従来どおり同一 origin の現在 URL 上で
動き、空 hash での起動は既存 boot が `#home/places` へ置き換える。

manifest とアイコンは index / JS / CSS と同じ認証外の static shell とする。ホーム画面への追加と
PIN 入力画面の構築に必要で、内容は固定の名称・色・画像だけでライブラリ情報や credential を含まない。
`/api/*` は従来どおり認証内である。全 static PWA route は既存 `Cache-Control: no-cache` を使い、
未認証 / Cookie 認証の両方で同じ bytes / MIME を返す route test を持つ。

`viewport-fit=cover` に加えて、PIN 画面、通常 top bar、home content、grid 左右余白 / notice、loading /
error、session status、viewer 上下 bar / seek / edge message、下 sheet と desktop side menu に
`env(safe-area-inset-top/right/bottom/left)` を適用する。縦持ちの status bar / home indicator だけでなく、
横持ちの左右ノッチ側にも操作要素や grid tile が入らない。

standalone ではブラウザ chrome が無いため、☰ の「再読み込み」は remote session acquire を必要としない
local command とし、現在 hash を保った `location.reload()` を行う。session acquire / ping が 401 を
受けた場合も PIN 画面へ戻すため、期限切れ後に standalone 内だけで再認証できる。現時点の shell は
外部リンクを提供していないため外部サイトへ出る導線の閉じ込めは存在しない。追加時は同一 scope の
内部 route と区別して OS browser へ渡す。

Service Worker は接続不能時の案内だけを担当する。保存するのは独立した
`offline.html` 1 件で、通常の page navigation は必ず network-first とし、通信失敗または
5xx のときだけ案内へ切り替える。案内は PC で mIV が起動していることとリモート接続が
有効であることの確認を促し、standalone でも空の error response をそのまま表示しない。

`app.js` / CSS / manifest / icon は Cache API へ保存せず、通常どおり network から読む。
fetch handler は navigation 以外へ介入しないため、認証応答、`/api/page`、
`/api/ai/jobs/*/result`、thumbnail を含む全 API / 利用者画像は端末の offline cache に
入らない。Service Worker script は `updateViaCache: "none"` で確認する。asset token の照合は
session acquire 直後だけ一度の自動再読込を許し、稼働中の定期・foreground 復帰時は
「新しい版があります」通知を更新案内の正本とする (§12.17)。

初回の成功した読み込みより前は Service Worker 自体が端末に存在しないため案内不能である。
特に iOS のホーム画面版は、一度 online で起動して登録と activate を完了した後の
scope 内 navigation から案内対象になる。この初回制約は「受付停止中も HTTP listener を
残す」形では回避せず、受付停止時に network listener を閉じる不変条件を優先する。

### 12.13 iPhone 実機指摘の反映 (protocol v14, 2026-08-01)

protocol v14 は `ContainerPayload` に `resume_page`、`open_mode`、
`thumb_aspect_height_ratio` を追加する。読書履歴からだけ特別に再開する規則は作らない。
ローカルと同じく、コンテナを開いたときに自動で viewer へ入るかは ZIP / PDF の
`effective_auto_fullscreen_zip_pdf`、画像だけの通常フォルダの
`auto_fullscreen_image_folders_enabled` で決め、自動で入る場合の「続きから / 最初から」は
`book_open_resume` に従う。自動で入らない場合も保存位置を一覧カーソルへ復元する。
`resume_page` は App 所有の `book_resume_db` を UI request queue 経由で読み、container worker が
別 DB connection を開かない。直前の remote 書き込みと読み出しは同じ bounded FIFO を通す。

保存された raw item index が現在も読み出せるページを指す場合だけ address に解決する。
ファイル削除などで index が現在の items 範囲外、またはページでない item を指す場合は
`resume_page=None` とし、ローカルの `resume_page_for_container` と同じく先頭の読み出せるページへ
フォールバックする。UI request が Busy / Timeout / Stopped で読み出せなかった場合も、理由をログへ
残して `resume_page=None` とする。再開位置は任意情報なので、この失敗を folder / ZIP / PDF の
container 取得エラーへ伝播させない。Web の viewer は page address から `page_groups` 上の所属 group を引くため、
見開き時は保存ページを含む見開きを開く。純粋関数テストで有効位置、ページ数ちょうどの上限、
大幅な超過、Web 側の不一致 fallback を固定する。

コンテナのサムネイル比率は collection 一覧と同じ
`aggregate_thumb_aspect_height_ratio(&settings)` から取得する。これはローカル設定に対応する
aggregate 比率の正本を共有し、コンテナ専用の既定値を持たない。通常のフォルダ / collection tile は
代表画像として従来の `object-fit: cover` を維持する一方、ページ一覧 tile だけはページ全形状の判別を
優先して `object-fit: contain` とする。

safe area の漏れは、通常フォルダ、ZIP / PDF のページ一覧、読書履歴・ブックマーク・本棚・
レーティング・スマートフォルダの collection 一覧が共有する mobile topbar / virtual grid と、
home content 上端にあった。mobile topbar を通常 flow 内へ戻して top / left / right inset の内側に置き、
親フォルダ、ホーム、現在位置のパンくず、操作メニューを常時表示する。virtual grid は JavaScript が
inline `left/right` で safe-area 指定を潰していたため、可変余白を CSS custom property へ渡し、
左右 inset との加算は CSS に一元化する。選択カーソルは tile 外側の outline ではなく内側の
box-shadow とし、最上段でも欠けない。home content は top inset も持つ。CSS / source test は
4 辺の指定、inline 上書きの不在、mobile topbar 非表示規則の不在を検査する。

viewer のメイン操作メニューは使用頻度順に、ブックマーク、レーティング、表示サイズ、見開き設定
(container のみ)、上下バー、全画面、ページ位置、一覧へ戻る、端末設定、操作方法、再読み込みとする。
レーティングは次画面で解除 / ★1〜5、表示サイズはズームを戻す / 全体 / 幅 / 原寸、見開きは
1ページ / LTR / LTR表紙 / RTL / RTL表紙、ページ位置は先頭 / 最後を選ぶ。前 / 次、拡大 / 縮小は
メニューからだけ外し、画面端 tap / pinch と既存 keyboard command (`矢印` / `PageUp` /
`PageDown` / `+` / `-`) は維持する。coarse pointer 用の高さと余白を詰め、全 menu page の action 数を
最大 11 件に固定するテストを持つ。

### 12.14 実機指摘: page 入口・サムネイル admission・一覧タイル (2026-08-01)

`GET /api/page` の入口は `ZipEntry` / `PdfPage` に加えて、通常フォルダ内画像の
`RemoteSubresource::File` を受け付ける。File は実ファイルであり、かつ `/api/list` と同じ画像拡張子
分類に一致する場合だけ許可し、動画・音声・テキスト、ZIP/PDF 本体、ディレクトリは page として 400 にする。3 種の許可対象も
含め、全 address は `Library::validate_remote_address` を通して絶対 path の構文・実在・
canonicalize と subresource を検証する。入口の回帰テストは通常画像 / ZIP entry / PDF page の許可と、
動画 / 音声 / テキスト、相対・不存在 path、subresource traversal の拒否を同じ guard に対して固定する。

remote-web の `MAX_CONCURRENT_HEAVY_IPC=4` と `MAX_CONCURRENT_IPC=6` は変更しない。ブラウザの
サムネイル fetch は pure policy から最大 3 件とし、共有 heavy 枠を page / container の foreground
処理用に 1 件残す。VirtualGrid の表示用 overscan は維持するが、ネットワーク要求の所有範囲は実際に
viewport と交差する行だけとする。未完了セルがその範囲を外れた時点で、そのセルの queued / active
request を `AbortController` で取り下げる。取得済み object URL は保持し、再表示時の不要な再取得は
行わない。

`503` かつ body の `error="ipc_busy"` は通信失敗ではなく admission の順番待ち通知として扱う。
`Retry-After` を待って同じ concurrency queue の末尾へ戻し、通常の network / 502 / service 503 用の
3 回 retry budget は消費しない。画面外へ出た場合はこの待機中も abort できる。telemetry は通常 retry
count と `admission_wait_count` を分離する。単一端末で他の heavy 処理がない定常一覧では、実機で
観測された admission 503 比率 76% を 0% 近傍へ落とす設計であり、別端末や foreground 要求との瞬間的
競合による 503 は Retry-After 待機へ戻して表示上の「再試行上限」にしない。

ファイル一覧とページ一覧は同じ `button.grid-tile > .tile-preview + .tile-label` 構造を使う。タイルを
CSS grid の preview 行 + label 行として明示し、ラベル背景を独立させるため、`Page 1` 等が画像へ
重ならない。選択 / focus 枠は child より前面の `::after` inset shadow でタイル全体に描く。通常画像と
コンテナページはいずれも `image-tile` として `object-fit: contain` を使い、設定されたタイル比率の箱を
維持しつつ縦長 / 横長の全形状を letterbox 込みで見せる。フォルダ・ZIP・PDF の代表画像は従来どおり
`cover` とする。

topbar の親フォルダ / ホームだけに `navigation-icon` を付け、40〜42px の tap area は変えず字形を
1.75rem に拡大する。iOS の慣例 route `/apple-touch-icon.png` と
`/apple-touch-icon-precomposed.png` は既存 `icons/icon-180.png` を返し、manifest と同じく認証より
前の static shell route に置く。

### 12.15 通常フォルダ一覧の本体統一 (protocol v16, 2026-08-02)

GET /api/list の remote-web ローカル走査を廃止し、FolderListRequest /
FolderListPayload を本体 IPC へ追加する。正本は
app::folder_scan::materialize_local_folder_listing である。App::load_folder が直接使い、
remote container / folder list は ContainerEngine::recompute_folder_listing という scan wrapper
から同じ関数を使う。remote-web は一覧の分類・sort・sidecar / 重複規則を持たない。

FolderListEntry は address、thumbnail_address、name、RemoteEntryKind、size、
mtime を運ぶ。動画と同名画像が video_thumb_use_sidecar_image により吸収された場合、動画の
address は .mp4 のまま、thumbnail_address は吸収した画像になる。Web グリッドは
`thumbnailAddressForEntry` で後者を選ぶが、/api/thumb には元 address と source path を併送する。
sidecar 画像は独立 tile として表示しない。

集約系は結果中の動画を親フォルダ単位にまとめ、一覧順で先に現れる異なる親を最大64件だけ
1回ずつ走査して既存の同stem規則を適用する。上限外の親は `thumbnail_address` を付けず
Shellへフォールバックし、打ち切った親数をログへ残す。走査結果の専用cacheは持たない。

タイル比率は手動設定なら本体 Settings、自動設定なら本体 auto_aspect_cache.db の該当フォルダを
core 側で read-only 参照して運ぶ。DB が無い場合は作成せず Square へ戻るため、旧 /api/list の
フォルダ別比率と既存の見た目を維持しつつ、remote-web から設定 / cache 読み取りを除去する。

HTTP admission は既存の全体 6 枠だけを使い、heavy 4 枠と stream 専用 4 枠を消費しない。本体側も
Home 専用 queue / worker を共有する。したがって thumbnail / container / page が heavy 上限でも
通常一覧用に 2 枠が残る。saturated_ipc_does_not_block_an_ipc_free_endpoint は変更せず、
stream・heavy・通常一覧の lane 分離を別テストで固定する。

debug test harness の warm 30 回計測では、旧 remote-web ローカル走査は 318 件で p50 0.336 ms /
p95 0.431 ms、522 件で p50 0.500 ms / p95 0.595 ms だった。新経路の core scan +
production materialize + DTO 投影 + IPC frame encode/decode は 318 件で p50 22.704 ms /
p95 27.485 ms、522 件で p50 37.741 ms / p95 42.854 ms。remote-web の独立 containment 再検証は
それぞれ p50 16.256 ms / p95 19.887 ms、p50 27.039 ms / p95 29.188 ms であり、合算の p50 は
約 39.0 ms / 64.8 ms である。稼働中の固定 pipe を奪わない測定のため named-pipe syscall と HTTP
送出は含めず、frame codec までを測った値である。一覧は heavy queue 待ちから分離されるため、
522 件でも操作入口を長時間塞がない。

open command telemetry は全試行を記録し、nested payload.kind、mediaKind、
open_route、handled を含める。成功 route も folder_container_image / media_image /
media_video 等で記録するため、media_open_route_rejected /
video_viewer_entry_rejected が 0 件でも、その手前のどの route が実際に使われたかを判断できる。

### 12.16 本体状態の共有 generation とリモート cache 整合性 (2026-08-05、2026-08-09 改訂)

> **先に読むこと — 何を何で検知するか。**
> 変化の出どころによって検知の仕組みが違う。混ぜると必ず取りこぼす。
>
> | 変化の出どころ | 検知 | 根拠 |
> | --- | --- | --- |
> | **本体側の変更** (設定・お気に入り・スマートフォルダ定義・「場所▼に出す項目」) | **セッション取得**。取得時に端末 cache を破棄し、home 画面のデータを取り直す | 排他により、本体はリモートが操作権を持つ間は変更できない (§2.2)。つまり本体側の変更は必ず切断を挟み、再取得を通る。**セッション取得は完全な信号であり、近似ではない** |
> | **セッション中に起きる変化** (リモート自身の書き込み = トリム・レーティング・見開き等) | **`remote_state_generation`** | 同一セッション内で起きるため、セッション取得では拾えない |
>
> **本体側の変更を generation で拾おうとしないこと。** generation は
> favorites / view-trim の内容差分で進むので、`show_location_*` やスマートフォルダ定義の
> 変更では進まない。ここへ項目を足していく方向で直すと、次に増えた設定でまた漏れる。
> セッション取得を入口にすれば、本体側で何が変わっても一律に読み直せる。
>
> この分担は [briefs/codex-remote-session-epoch-addendum.md](briefs/codex-remote-session-epoch-addendum.md)
> 「セッションを版の正本にする / 切断されたら明示的に再接続するまで止まる」で確定した。
> 実装は `applyRemoteSessionId` (セッション ID が変われば cache epoch 更新 + 各 cache 破棄)。

以下は generation 側 (セッション中の変化) の仕様である。

本体が正本である favorites と view-trim の更新は、remote-web が持つ 1 個の
`remote_state_generation` で端末へ公開する。値は remote-web 起動ごとの乱数 prefix と単調 counter
から成り、`settings.db` と `view_trim.db` の専用 read-only connection で
`PRAGMA data_version` を観測する。settings の版が変わった場合だけ favorites を全件再読込し、内容が
同一なら generation を進めない。view-trim の版変更と favorites の追加・削除・改名・並べ替えは同じ
generation を進める。

端末は session acquire / ping の応答で同じ generation を受け取る。ブラウザの全 command は既存の
「後から操作した owner が勝つ」規則により active 中も acquire を先に送るため、ページ送りはこの既存
ownership RTT で generation も観測し、ページ group ごとの `GET /api/remote-state` pre-flight は送らない。
シーク確定と viewer resize も cache-only の迂回路にせず acquire 後に描画する。generation が変わった
場合だけ `PageResourceCache`、画像寸法 hint、表示中 group を破棄・再読込する。`GET /api/remote-state`
自体は no-store の状態確認 API として残すが、通常のページ表示経路からは使用しない。

`/api/page` の URL と `PageResourceCache` key には group 開始時に snapshot した generation を入れる。
server はその期待値を IPC の前後で確認し、途中で版が変われば
`409 remote_state_generation_mismatch`（no-store）を返す。見開きは 2 request を同じ snapshot で
並行取得し、既存の両 page decode 完了後の atomic replacement だけを表示経路にするため、旧版と新版の
混在は DOM に到達しない。generation 変更時は group cache を破棄して両 page を取り直す。
HTTP の `private, max-age=60` は維持するが、版変更時は URL 自体が変わるため自然失効待ちには依存しない。
page response の `X-mIV-Remote-State-Generation` は request の group snapshot と一致することを端末でも
検証し、欠落・不一致なら画像を表示しない。この header は画像を生成した版の証明であり、HTTP cache
hit 時点の最新性そのものは証明しない。現在版は先行する ownership acquire / 定期 ping / visibility
復帰時 acquire から学び、request が server に届く場合は前後検証と 409 でも自己修復する。端末が既存
session と既存表示を保ったまま、次の acquire / ping / page request が一度も発生しない間は表示中画像を
自発更新しない、という採用済みの鮮度境界は残る。

**ここで「本体 DB だけが変わる」場合を数えないこと。** 本体はリモートが操作権を持つ間は変更できない
(§2.2)。本体側の変更は必ず切断を挟むので、この鮮度境界の対象は**セッション中にリモート自身が
起こした変化**に限られる。本体側の変更は冒頭の表のとおりセッション取得で拾う。

favorites の `data_version` 観測はホームの「お気に入り」一覧と remote state generation の更新にだけ
使う。お気に入りの追加・削除・名称変更は入口表示へ即時反映するが、path のアクセス可否は変えない。
500 favorites の warm 計測では `data_version` が p50 2.10 µs / p95 2.40 µs、全件読込が
p50 239.20 µs / p95 315.30 µs（p50 約 114 倍）だったため、変更が無い request での全件読込は行わない。

Auto trim の見開き slot に相手 address が無い場合は、上下 harmonize だけを諦めて
`AutoSingle` としてページ自体は描画する。相手が指定されている場合の address・slot 検証は維持する。

### 12.17 端末版の記録と接続時の更新 (2026-08-07)

端末が走らせている版は、後から `/api/app-version` を読んだ値ではない。navigation で返す
`index.html` の固定 placeholder へ、その時点の `web_asset_token` を埋めた
`miv-remote-asset-token` meta を正本とする。これにより、古い script が走ったまま配信資産だけ
新しくなった場合も、端末版を新しい版と誤記録しない。session acquire の成功応答には同時点の
配信 asset token を載せ、追加 RTT なしで照合する。通常段の `app_version` telemetry は
`running_asset_token` / `served_asset_token` / `versions_match` / `update_outcome` を持つ。
これらは秘密ではなく、生の client ID / session ID、PIN、Bearer token は含めない。稼働中の照合で
新しい組合せを観測した場合も一度記録し、配信差し替え後の調査に残す。

取得直後に両 token が違う場合は自動再読込する。ただしタブ単位の `sessionStorage` に
再読込前の端末 token と対象の配信 token を保存し、書込み後の読戻しまで成功した場合だけ実行する。
現行版との一致を確認するまで自動再読込は一回に限定し、再読込後も古い場合、対象版がさらに変わった
場合、または storage を利用できない場合は二度目を踏まず既存バナーへ落とす。稼働中の5分巡回と
`visibilitychange` 復帰時は、開発中に本体と session を生かしたまま資産を差し替える経路があるため、
従来どおりバナーだけを出す。

「asset token が変わった = 本体が再起動した」は厳密には成り立たない。外部起動の remote service は
本体の再起動を越えて待機でき、開発時の web root は本体を生かしたまま変更できる。一方、remote session
と video stream の所有者は本体プロセス内の `SessionHandle` なので、本体終了を越えて旧状態が生きる
経路はない。通常の managed service も本体終了時に子 process が終了する。この境界から、更新の自動化は
新しい session の取得成功直後だけに限定する。

### 12.18 配布物への remote service 統合 (2026-08-07)

core の探索規則は従来どおり「自分と同じ directory の `mimageviewer-remote.exe`」だけとする。
単体 exe / installer の launcher は release core、remote service、FFmpeg 6 DLL を
`include_bytes!` で同じ `%APPDATA%\mimageviewer\runtime\<version>\` へ展開する。launcher の
`build.rs` は core と remote の両方が先に存在しなければ停止し、それぞれの hash を埋め込む。
版別 runtime と workspace 共通 version、同じ source tree の `mimageviewer-ipc` により、利用者が
core と protocol 不一致の remote を組み合わせる経路を作らない。

remote は従来、release でも `CARGO_MANIFEST_DIR/web` という開発 source tree の絶対 path から
ブラウザ資産を読んでいた。配布用の `embedded-web-assets` feature では HTML / JS / CSS / HLS /
PWA 資産とライセンスを remote exe に内包し、filesystem 上の web root を持たない。通常の
dev-runtime と `restart-remote-web.ps1` は feature を付けず、従来どおり source tree を直接配信して
フロント変更の hot reload を維持する。

内包一覧と公開静的 route の正本は `remote-web/build.rs` が `web/` を再帰走査して生成する。
通常ファイルを対象に、Node テストの `*.test.mjs`、`package.json` / `package-lock.json`、
開発専用 source map の `*.map` だけを除外する。HLS の LICENSE / VERSION は配布追跡情報として
残す。生成 manifest は disk 配信、埋め込み配信、asset token の全てで共有し、Content-Type は
`src/web_assets.rs` の拡張子表だけを正本とする。未知拡張子は build error にして暗黙の
binary 配信を許さない。`cargo:rerun-if-changed=web` によりファイルの追加・削除・更新でも
manifest と埋め込みを再生成する。

署名順は vendor PE → core + remote → launcher → installer。remote は launcher に埋め込む前に署名し、
portable は `mimageviewer.exe` の隣へ Web 資産内包 remote を loose 配置して、package copy を署名する。
distribution clean は通常 target / portable target の両方で `mimageviewer-remote` package も消し、
古い protocol / Web 資産の remote を載せない。

### 12.19 画像補正パネルのタブ構成 (2026-08-07)

リモートの画像補正パネルは、本体の実装順に合わせて `色調` / `AI` / `カラー化` の 3 タブに分ける。
補正モードと色調スライダーは `色調`、AI モデル選択は `AI`、カラー化の方式・プリセット・調整値は
`カラー化` に置く。補正対象と適用スコープは本体と同じくタブより上に置き、現在リモートが提供する
色調リセットと状態表示はタブより下に置く。これにより、タブを跨いで効く操作を特定タブの奥へ
隠さない。色調・カラー化スライダーの pointer 処理は移動せず、押下位置に飛ばない相対ドラッグと
native range のキーボード・支援技術対応を維持する。

タブ選択は出力状態ではなく端末 UI の位置なので、本体の `AdjustmentSettingsTab` とは共有しない。
既存の `miv-remote-local-settings` version 1 に後方互換な `adjustmentTab` を加え、未知値や旧保存値は
`color_tone` に正規化する。

本体の 4 番目の `フィルタ` は現時点の remote IPC が post-filter を公開しておらず、remote の更新時も
本体値をそのまま保持する。操作不能な空タブは現在値を確認・変更できるように見せてしまうため出さない。
PC と共有される post-filter を端末でも可視化・編集する必要は残っており、IPC、表示、書込みを一組で
追加する別作業とする。

### 12.20 補正スライダーの縦パン所有権 (2026-08-07)

補正パネルは縦スクロールする文脈なので、色調・カラー化の range は `touch-action: pan-y` とする。
ブラウザが縦パンを選んだ場合は `pointercancel` / `touchcancel` を通常の確定終了として扱わず、
pointer 開始時の値へ戻す。cancel 前に横ブレの preview が発行済みなら、開始値の preview を最新要求として
再投入し、永続書込みは行わない。touch の pointer event では viewport パンを
`preventDefault()` で所有しようとせず、mouse / pen の native range 操作を抑える既存処理だけを残す。
画像・動画の seek range はスクロール文脈にないため `touch-action: none` のままとする。

太いスクロールバーは、range 上から縦パンできない根因を解消せず狭い画面の内容幅も減らすため追加しない。
タブ分割と通常のスクロール位置で、下に内容が続くことは示す。

`.image-viewer` と `.viewer-stage` はともに `touch-action: none` で、ブラウザの viewport 操作を
許可せず remote が pinch と pan を所有する。この指定はアプリのダブルタップ操作のためではなく、
独自 pinch / pan の ownership として維持する。`manipulation` への変更は browser pinch/pan と remote の
gesture owner を競合させるため行わない。アプリ外周の browser zoom policy は §12.21 のとおりとする。

### 12.21 browser double-tap zoom の ownership と計測 (2026-08-07、2026-08-11 更新)

2026-08-08 の利用者判断により、静止画 viewer のダブルタップによる `Page` ⇔ `Original` 切替は廃止した。
原寸表示は操作メニュー → 表示の「原寸 (100%)」に残す。これに伴い、中央 tap の pair 候補、320 ms の
保留 timer、保留 tap の commit、原寸トグル専用 command と候補棄却 telemetry も撤去し、中央 tap は
各 `pointerup` で即時に上下バーを切り替える。アプリ自身はダブルタップに意味を割り当てない。

一方、browser の double-tap zoom は独立して起こり得るため、document level の単一 observer は維持する。
アプリ外周 (`html, body, #app`) の既定は `touch-action: manipulation` とし、2 回の単指 tap の時間差と
距離を `browserDoubleTapZoomDecision` の純粋関数で判定する。ただし observer は成立した2打目を含む
**どの `touchend` も `preventDefault()` しない**。button / link / input / 素の要素を selector で分けず、
対象種別にかかわらず同じ認識経路を通す。12 CSS px を超えて移動した gesture は候補にせず、2接点以上も
sequence を破棄するため、drag と multi-touch は pair 観測へ入らない。

`document-double-tap.mjs` は telemetry を知らず、任意 callback に固定形式の判定事実だけを返す。app は
これを既存 schema の通常段 `browser_double_tap/suppression_decision` として、直前の実 tap からの経過 ms
と距離 px、double-tap と認識したか、`event.cancelable` を記録する。成立した対は `pair_recognized` とし、
`suppressed` / `excluded` は常に false、除外理由は null である。action 名はログ schema の互換名であって、
現在の挙動が抑止を行うことを意味しない。成立 pair 直後の3打目でも実際の直前 tap との差を失わない。
pair には SPA の実行中に単調増加する数値だけの相関番号を付け、session ID、path、DOM target は記録しない。

`visualViewport` の resize は従来どおり 250 ms 落ち着いた時点で、前回記録値から scale が 0.01 以上
変わった場合だけ `visual_viewport/scale_changed` を通常段へ送る。この event には直前の tap pair 相関番号、
最後の resize 観測時点での pair からの経過 ms、pair の時間・距離・認識・抑止・除外理由・`cancelable` を
併記し、browser zoom の発生と直前 pair を直接照合できるようにする。

520 ms の認識窓と距離は変更せず、要素別の抑止 selector は持たない。browser zoom を止める境界は
§14.8.1 の viewport meta (`maximum-scale=1, user-scalable=no`) であり、通常タブで iOS がこの指定を
無視して拡大する場合は受け入れる。原寸表示後に pan できず menu が開くという観測は、この変更で
修正済みとは扱わない。操作メニューから原寸へ入った場合にも起こるかを別途確認し、必要なら独立した
不具合として調査する。

### 12.22 ページ応答 identity の検査 (2026-08-08)

`/api/page` と AI result の画像応答は、画素生成側が確定した `RemoteAddress` を
`PagePayload.identity` として返す。identity は HTTP 要求値の echo ではない。core が
canonicalize 済み絶対 path、実際に選んだ file / PDF page / archive entry から、画素を
応答へ載せる境界で再構成する。IPC protocol v31 はこの identity を page payload の一部として運ぶ。

remote-web は core から受けた identity を percent-encoded JSON にし、
`X-mIV-Page-Identity` 応答ヘッダーへ載せる。SPA は単ページ、見開き、補正プレビュー、AI result の
すべてで、blob 化・cache 格納・decode・DOM 差替えより前に要求 identity と完全一致するか検査する。
欠落、decode 不能、構文不正、不一致はいずれも fail closed とし、要求外の画像を別ページとして表示しない。
不一致時は専用エラーを表示し、通常段 telemetry に要求 identity と応答 identity の両方を記録するが、
同じ応答の自動再取得は行わない。thumbnail はこの検査の対象外である。

identity の型は絶対 path と file / PDF page / archive entry だけを含む。
PIN、Bearer token、session id などの接続秘密・session 情報は IPC payload と HTTP header のどちらにも
含めない。

### 12.23 パス allowlist の撤去 (protocol v37, 2026-08-08)

リモートで一覧に表示できる項目は、お気に入りとの包含関係にかかわらず絶対 path の住所をそのまま
開ける。タグ、★1〜5、動画・音声／本のブックマーク、閲覧履歴、本棚、スマートフォルダの候補を
一覧変換時に allowlist で落とさない。ホームの「お気に入り」「スマートフォルダ」「場所」の 3 タブと
操作は変えず、これらは入口・検索範囲としてだけ使う。

保証できない制限を保護として提示しないという判断により `crates/remote-registered-roots` は削除した。
同クレートへ移されていた `pictures_root()` は `capture.rs`、閲覧履歴上限は `reading_history_db.rs`、
path 正規化は `adjustment_db.rs` へ戻した。本棚既定値が capture 既定出力の `books` 子である関係は維持する。

protocol v37 の `RemoteAddress` / `RemoteEntry` は絶対 `path` と subresource を持つ。folder / container
payload の `root_name` は表示用であり、アクセス境界を表さない。SPA は絶対 path から親移動とパンくずを
構成し、favorite 配下では従来どおり favorite 名をパンくずの入口表示に使う。

読み取り、動画開始に加え、`POST /api/write` と `POST /api/ai/jobs` の入れ子 address も絶対 path を
反復 query 引数へ移し、JSON body には残さない。request log は query 全体を落とし、telemetry は
クライアントと remote-web の両境界で path field と絶対 filesystem path 文字列を除去する。

### 12.24 物理フォルダー一覧の設定同期 (2026-08-09)

`ContainerEngine` は起動時の `Settings` を保持するが、物理フォルダー一覧を 1 回生成するたびに、走査・
materialize・並び順固定・サムネイル縦横比の判断へ使う設定だけを `settings.db` から読み直す。対象は
`sort_order`、hidden 表示、grid 行順、変換対象 archive の扱いと旧互換値、ZIP/archive/画像/video の
同名重複処理、video sidecar 採用、画像拡張子優先順、books root、画像フォルダー自動表示の派生述語に
必要な値、thumbnail aspect である。17 key を単一の indexed query と 1 回の DB lock で取得し、起動時
snapshot へ overlay する。旧 DB に key が無い場合だけ、起動時ロードで serde default 適用済みの値を使う。
これ以外の container 表示・再生設定は引き続き起動時 snapshot であり、集約系一覧は従来どおり
`CollectionSettingsSource::Live` から全設定を読む。

方式は必要項目だけを読む案を採用した。2026-08-09 の warm read 計測では、従来の `sort_order` 1 key が
p50 3.0 µs / p95 3.4 µs、17 key の point read が p50 52.8 µs / p95 60.1 µs、同じ 17 key の単一 query が
p50 22.1 µs / p95 26.3 µs だった。一方、`load_into_settings()` 相当の SQL と JSON parse だけでも
p50 38.2 ms / p95 43.8 ms だった。この DB では一覧と無関係な VST state が約 33.6 MB、chain slot state が
約 4.4 MB あり、全設定方式はさらに Rust の `Settings` 再構築と VST hash 計算を伴う。したがって 1 listing
あたり約 22 µs の限定読取を選び、listing の判断項目を追加するときは
`RemoteListingSettings`、固定 query、overlay test を同時に更新する。

### 12.25 変換アーカイブの長時間ジョブ基盤 (protocol v41, 2026-08-09)

RAR / 7z / LZH の準備は、通常の IPC worker 要求の中で同期実行しない。protocol v41 は
archive 専用の start / state / recoverable / cancel / confirm / password / result と、確認待ち・
パスワード待ち・変換枠待ち・変換・finalize・終端を表す snapshot を追加した。AI job の registry、
snapshot、terminal 型は流用せず、session が参照する nonterminal 判定と drain 通知だけを
`RemoteLongJobRegistry` として共有する。start は session operation を長時間 job へ lease して即時に
job id を返し、検査・変換は専用 thread で進めるため Heavy worker 枠を保持しない。

archive cache は App が起動時に開いた `Option<Arc<ArchiveCacheDb>>` を AI bridge と同じ session 登録
境界から注入する。リモート側で DB を開き直さず、ローカル変換・cache maintenance と同じ
`begin_convert()` lock を使う。DB 初期化失敗の `None` でも直接読み可能な RAR は開けるが、変換が
必要な形式は確認前に `CacheUnavailable` で明示的に終端し、unwrap や無言の no-op にしない。
Windows では canonical path が `\\?\` prefix 付きになるため、共有 DB の lookup / reserve / record の
source key は通常表記の `ResolvedPath.logical` に統一し、fingerprint と converter の filesystem I/O
だけに `ResolvedPath.canonical` を使う。これにより本体と remote が同じ cache record を参照する。

公開 identity は利用者が選んだ元 archive の `RemoteAddress` のまま保持する。実際の読み込み先は
core 内だけの `DirectRar { resolved_path }` または
`CachedZip { path, source_path }` とし、cache path を serialize、履歴、URL、表示名へ出さない。
prepared backing を実際の読み込みへ渡す直前に、元ファイルの fingerprint と backing の実在を
core 内で再検証する。state / result の軽量な問い合わせでは filesystem I/O を行わない。

パスワードは archive job が入力待ちの間に password API から入力の都度受け取り、executor の一時変数と
有界 input channel だけを通す。request の Debug は値を redact し、snapshot / recoverable / terminal /
result / URL / log / error message へは保存しない。password 再試行の可否は呼び出し側で形式判定せず、
converter の `PasswordRequired` / `BadPassword` / `PasswordUnsupported` を正本にする。現行 converter が
password を扱える RAR だけが前二者を返して該当段階から再開し、sevenz-rust2 が識別した暗号化 7z は
`PasswordUnsupported` になる。delharc 0.6.x は password API も暗号化専用 error / flag も公開しないため、
LZH の失敗は `Archive` / 非対応圧縮方式のまま通常エラーとし、文字列から password prompt を推測しない。

owner が別端末へ移る場合は `Superseded`、本体が操作権を戻す場合は `DiscardedByHost`、background
保持期限は `BackgroundExpired` を archive registry 自身へ通知して cancel する。`.part` は既存 converter
の atomic publish guard が除去する。一方、record まで完了して整合状態になった cache ZIP は、元の
remote 操作が同時に cancel 終端へ移っても共有 cache として残す。公開 job result だけは要求された
owner 終端を優先する。

新 owner の acquire は `DrainingRemote` を即時応答し、旧 job の終了を connection reader 上で待たない。
`begin_convert()` 待ちは snapshot の `WaitingForConversionSlot` で識別でき、drain が 3 秒を超えた場合は
reason、elapsed、App drain 完了、operation / streaming worker 数をログし、その後 10 秒ごとに再記録する。
パスやパスワードはこの診断へ含めない。変換進捗は job 内の high-water 値で単調性を保つ。
C-2 では `/api/archive/jobs` 以下を remote-web へ接続した。HTTP worker が行うのは start / state /
recoverable / cancel / confirm / password / result の短い IPC だけで、検査・変換を同期実行しない。
Web は Ask のときだけ画像数・展開後サイズ・入れ子数を示して確認し、Convert は確認を挟まず、
直接読み可能な RAR と fingerprint が一致する既存 cache はどちらの設定でも確認なしで開く。変換中は
Core snapshot の files done / total と bytes written だけを表示して推定 percentage を作らず、中止は
archive job の cancel へ送る。password は専用 modal から POST body にだけ置き、送信直後に input を
空にする。変換 cache ZIP が暗号化されないことも確認・password 画面で明示する。

`result` の取得を、session 内の公開元 address → Core-only backing の登録境界にした。ここでは
filesystem I/O を行わず、以後の
container / page / thumbnail / metadata / write は元 archive address のままで、`ContainerEngine` の
resolve が最初の読み込み時に fingerprint と backing の実在を再検証してから Direct RAR または
cache ZIP へ差し替える。identity、履歴、resume、bookmark、
補正・編集 DB key、catalog の所有元は元 archive のままで、backing path と job id は URL や payload に
出ない。terminal job の 10 分保持を過ぎても active backing は session 中保持し、owner 交代・host discard・
background expiry の drain で epoch を進めて破棄する。drain と backing 検証が競合した場合は古い結果を
読み込みへ渡さない。これは共有 cache ZIP 自体を削除する操作ではない。

folder / collection の Web 側 archive kind 除外は撤去した。Ignore の正本は Core に置き、物理 folder は
§12.24 の live listing settings、bookmark の `OtherArchive`、reading history、rating、smart folder 等の
集約系は `CollectionEngine` がその要求ですでに `load_into_settings()` した同じ `Settings` で archive
候補を bound 前に除外する。tag item も同じロード済み値を mapping に渡し、追加の全設定 read は行わない。

### 12.26 ページ先読みの実効化と表示履歴 (2026-08-09)

実機の成功ページ 200 件では応答サイズが p50 1.5 MB、p95 7.9 MB、max 8.1 MB だった。
以前の 32 MiB は単ページの現在 1 + 前方 3 + 後方 1 の 5 ページ作業集合すら p95 付近で
保持できず、先読み済み Blob を使用前に LRU から捨て得た。要求解像度、画質 preset、
`PAGE_JPEG_QUALITY`、`viewerImageLayout` は変更しない。

この段階では 8 MiB × 作業集合ページ数による 48 MiB 下限と entry 上限 12 を採ったが、
窓を広げるほど byte 予算まで増えるため、2026-08-11 に固定 64 MiB admission と保護集合方式へ
置き換えた。さらに 2026-08-12 に §14.6 の枚数設定へ移し、byte admission だけを撤去した。
以下の 503 / 同時実行 / foreground 相乗り契約は引き続き有効である。

HTTP 503 `ipc_busy` / `admission_busy` は計画無効ではなく一時的な入場拒否として扱う。
失敗ページを pending の末尾へ戻し、他の計画ページへ機会を渡したうえで `Retry-After`
(既定 1 秒) 後に再開する。pending 全消去と retry budget 消費は行わない。abort 済み要求は
再投入しない。

ブラウザの `PageResourceCache`、remote-web HTTP admission は同時最大 2 件とする。本体の
remote heavy worker は利用者設定の半分・最大 3 本で、prefetch 上限を
`min(heavy_workers - 1, 2)` とし、常に foreground 1 本を予約する。同時 prefetch 同士は
互いを cancel しない。本体の進行中 prefetch は対象 `RemoteAddress` と cancel token を組で
登録し、foreground 到着時は自ページと `render_context.spread_partner` に一致する登録を保持して、
無関係な prefetch だけを cancel する。ブラウザ側も foreground が来た時点で無関係な prefetch を
abort して直接開始する。このため同時実行数を上げても foreground が prefetch の後ろへ並ぶ構造には
しない。

`page_display` の実機ログにより、前景が進行中の先読みへ相乗りした直後、新しい先読み計画が
現在 group の key を含まないため、その通信を中止していたことが 2026-08-09 に確定した。
`PageResourceCache.active` は `prefetchPlanned` と `foregroundWaiters` を持ち、先読み計画の所有と
前景待機者を明示的に分ける。`schedule` は計画外かつ前景待機者 0 件の通信だけを中止し、計画外の
通信から最後の前景待機者が離れた場合はその時点で中止する。別の foreground 到着による無関係な
prefetch の中止も、既に前景待機者がいる通信は対象外とする。前景自身の `AbortSignal` は
`awaitWithAbort` でその待機だけを従来どおり中止し、共有通信の controller とは分離する。
503 `ipc_busy` に相乗りした前景が admission 失敗を継承せず直接 foreground 取得へ進む契約も維持する。
現在 group を `schedule` の計画へ足すだけの例外処理は採らない。

同じ実機ログの追跡で、本体側にも foreground 開始時に登録済み prefetch を一律 cancel する経路が
あり、見開きの片方へ相乗り中にもう片方を foreground 取得すると、相乗り先を本体自身が止めて
`MediaErrorCode::Busy` にしていたことが確定した。登録を token だけの列から
`{ address, cancel }` へ変え、foreground がこれから使う自ページと見開き相方を所有対象として保護する。
単ページでも同じ address の prefetch は保護し、対象外だけを従来どおり止める。完了時の解除は
cancel token の `Arc` identity で当該登録だけを除く。

heavy worker 1 本の foreground 予約により、prefetch を全件止めて入場枠を作る必要はない。ただし
無関係な進行中 prefetch は decode、archive / PDF、I/O など foreground と共有する下位資源を消費し、
遷移後は stale work にもなるため、対象を識別した選択的 cancel は維持する。

ページの `miv_media_busy` に対する追加の自動再試行はこの増分では入れない。この Busy は core queue の
一時的な入場拒否だけでなく、無関係になった prefetch の意図的 cancel と session 失効も同じ code で
表す。ページ単位で一律再試行すると、優先度制御が止めた stale prefetch を IPC 内で再生成してしまう。
現在表示に必要な prefetch を誤って cancel する原因は所有修正で除き、foreground 枠も予約済みである。
将来 retry を広げる場合は、入場拒否と所有終了による cancel を protocol 上で型分けしてから行う。
既存の HTTP admission 503 `ipc_busy` の計画内再投入は維持する。

要求画像を表示できなかった場合の位置不整合は、この所有修正とは分けて未解決とする。
`fetch_failed`、decode / apply failure、identity / generation 拒否でも `loadGroup()` は `false` を返すが、
同じ `false` は新しい要求による active / pending の supersede にも使われる。単純に `false` で
`discardRequestedPageGroup` を呼ぶと、同じ group の resize / 再描画として後続している正当な要求まで
描画位置へ巻き戻し得る。また既存 discard はタイトルを描画ページ名へ戻すため、終端失敗メッセージと
履歴 URL の扱いも同時に定義する必要がある。後続では queue outcome を terminal failure と supersede に
型分けしたうえで、終端失敗だけ要求位置を描画位置へ戻し、エラー表示と history を整合させる。
自動再試行、delay、追加 repaint は採らない。

既存 telemetry の `page_display` は、要求 group、1-origin の要求ページ番号、シークバーへ出した
page label、取得候補と実際に DOM へ適用した HTTP request ID、`applied` / `not_applied`、固定列挙の
理由を client timestamp / sequence 付きで残す。理由は `dom_committed`、`pending_superseded`、
`queue_cleared`、`load_sequence_mismatch`、`abort`、identity / generation / session の既存判定名、
fetch / decode / apply failure に限定する。request ID は server の単調数値識別子で、path、address、
PIN、Bearer、remote session 生値は event builder の入力に持たせない。既存 normal/debug 段階化と
server 最終 redaction は維持する。

### 12.27 混在フォルダの本判定と seek overlay parity (protocol v43, 2026-08-11)

通常フォルダの保存値が無い見開き既定は、本体と同じ
`crate::app::physical_page_order_locked` を `spread_payload` から直接呼んで決める。remote 側に
画像のみ・自動フルスクリーン等の条件を再記述しない。本と判定された場合だけ端末共通の
`default_spread_mode` / `default_reading_direction` を使い、それ以外は本体の
`SpreadRestoreDefaults::NON_BOOK` と同じ Single / LTR とする。明示 request と `spread.db` の保存値は
従来どおり既定より優先する。ZIP / PDF は `is_open_as_container` により本のままである。

folder container は画像 entry / page group へ絞る前の materialized items をこの判定へ渡す。
同じ full items から本体 seek overlay の nav item 分類を通して画像・動画・その他の件数を作り、
protocol v43 の `ContainerPayload` に載せる。Web は全 nav item が画像かを本体と同じ式で判定し、
真なら従来の seek range、偽なら `fullscreen_mixed_media_summary` と同じ順序・単位・読点の件数サマリを
表示する。判定と文面生成は `command-core.mjs` の純関数が所有する。

## 13. 作業運用メモ (セッションをまたぐ引き継ぎ用)

この節は設計ではなく**開発手順**の記録。会話ログにしか残らない知識を失わないために書く。

### 13.1 検証環境

実利用中の `%APPDATA%\mimageviewer` を汚さないため、**隔離データディレクトリ**で検証する。

```powershell
# 本体 (ユーザーが実行する。エージェントは mIV 本体を起動しない)
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe `
  -ArgumentList '--data-dir','.\target\dev-runtime\data'
```

- データディレクトリが既定と異なると、mutex / activate event / open-path pipe /
  shutdown event の 4 つは別名前空間になる。ただし remote IPC の pipe 名だけは固定かつ常設なので、
  **隔離側の remote を検証するときは通常版 mIV を終了する** (§11)
- 隔離データは `%APPDATA%\mimageviewer` から robocopy で作った。`fts_index` / `runtime` /
  `tensorrt*` / `logs` / `_recovery` は除外して約 11.6GB (元は 113GB)

### 13.2 ビルドと再起動

```powershell
.\scripts\restart-remote-web.ps1     # remote-web: 停止 → release ビルド → 隔離データで再起動
.\scripts\build-dev.ps1              # 本体 (core) のビルド
```

- `restart-remote-web.ps1` は既定で隔離データディレクトリを使う。起動出力 (接続 URL / QR /
  Bearer) は `remote-web-console.log` へ落ちる
- **`.ps1` は ASCII のみで書く。** 日本語コメントを入れると Windows PowerShell 5.1 が
  BOM 無しスクリプトを ANSI として読み、パースエラーになる
- 開発用 remote の `crates/remote-web/web/` はディスクから直接配信される。**フロントだけの変更なら
  ビルドも再起動も不要**で、ブラウザの再読み込みで反映される

### 13.3 テスト実行時の注意

- **フロントのテストは `crates/remote-web/web` から実行する。** リポジトリルートから
  `node --test` を打つと相対 import が解決できず失敗する

  ```bash
  cd crates/remote-web/web && node --test
  ```

- **本体の lib テストは `target/debug/deps` に FFmpeg DLL のコピーが必要。**
  無いと `STATUS_DLL_NOT_FOUND` で落ちる

  ```bash
  cp vendor/ffmpeg/bin/*.dll target/debug/deps/
  cargo test -p mimageviewer --lib
  ```

- 本体 UI (`app.rs` / `ui_fullscreen.rs` 等) に触れた増分では、絞り込みではなく
  **`cargo test -p mimageviewer --lib` を全件**流す (4,600 件前後 / 約 165 秒)

### 13.4 Codex への依頼で毎回書くこと

- **`scripts/build-dev.ps1` を実行させない。** 動作中の本体を停止させてしまう。
  本体ビルドは ClaudeCode 側で行う
- release ビルドは**別 target** へ出させる (稼働中プロセスが exe を掴んでいるため)
- コミットはさせない。Codex のサンドボックスは worktree の Git 管理領域
  (`C:\home\mimageviewer\.git\worktrees\...`) に書けず `index.lock` を作れない
- 本体 `src/` に触れた場合は**全箇所と理由を報告**させる
- detached / viewport 述語に触れる場合は `docs/detached-rework-plan.md` への記録も求める
  (凍結ルール。§11 の判断例を参照)

### 13.4.1 決定は本書へ書き戻す (2026-08-09)

**`docs/briefs/` は git 管理外である。** ブリーフにしか書かれていない決定は履歴に残らず、
次のセッションが本書とコードだけを読んで**逆の方針を再実装する**。

2026-08-09 に実害が出た。「セッションを版の正本にし、再接続で cache を破棄する」という決定
([briefs/codex-remote-session-epoch-addendum.md](briefs/codex-remote-session-epoch-addendum.md))
が本書へ書き戻されず、§12.16 が世代方式を詳細に記したままだった。後のセッションが本書と
コードを読み、`/api/home` の再取得を**世代側へ載せた**。世代は内容差分でしか進まないので
本体設定の変更を拾えず、実機で反映されない不具合になった。さらにその修正案として
**世代の計算を変える**提案まで出しており、利用者の指摘が無ければ誤った方向へ深く進んでいた。

- **方針を変えたら、その increment のうちに本書の該当節を書き換える。** ブリーフは作業指示で
  あって決定の保管場所ではない
- 既存節を**置き換える**こと。新しい節を足して古い記述を残すと、次の読者がどちらを信じるか
  分からない。差し替えたなら古い方に「§X で差し替え済み」と書く
- 「採用済み」「確定」と書かれた記述を見つけたら、それが**いつの決定か**を確認する。
  日付の新しいブリーフに覆されていることがある

### 13.5 プロトコル版数

`crates/remote-ipc` の protocol version を上げた増分では、**本体と remote-web の両方を
再ビルドして再起動する**必要がある。片方だけだとハンドシェイクで弾かれる。
collection の session spread request と address-based `page_groups` を追加した現行版は **v49**。
v48 はブラウザの明示 logout で current owner を release する `SessionRelease` を追加した。
v47 は root 以外の Serve path を型付きで通知する。v46 は
`RemoteWebConnectionInfo.tailscale_https_certificate` と
`tailscale_key_expiry_unix_seconds` を追加する。v45 で追加した
`RemoteWebConnectionInfo.tailscale_serve_conflict` も保持する。v44 で削除した
`RemoteWebConnectionInfo.pin_configured` は戻さない。v43 で `ContainerPayload` に追加した
本体 seek overlay と同じ画像・動画・その他の件数内訳は引き続き保持する。
v42 の `PageRequest` job / optional display request ID、batched `PageDemand` の promote / release、
typed な `Cancelled`、v37 以降の絶対 path + subresource、表示前照合、検索契約、長時間ジョブ契約も
引き続き両側が同じ版であることを前提とする。

### 13.6 残タスク (2026-08-01 時点)

1. **動画・音声ストリーミングのフロント (増分 7)** — server / IPC は増分 6 で完了。
   正本は [web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md)
2. **検索** (Ctrl+S / F / G 相当)、タグ
3. **接続診断ウィザード**。配布への `mimageviewer-remote.exe` 組込みは 2026-08-07 に完了。
   単体 exe / installer は launcher が core と同じ版別 runtime へ展開し、portable は core の隣へ
   loose 同梱する

### 13.7 未消化の宿題

- **完了 (2026-08-07)**: release の core → remote → launcher ビルド後に
  `cargo test -p mimageviewer-launcher` を実行し、7 件すべて成功。single-instance 名前空間に加え、
  埋め込んだ remote が core と同じ版別 runtime directory へ hash 一致で展開されることも固定した

## 14. ページ表示パイプラインの所有権 (2026-08-09 決定)

リモートでページを捲ると、**シークバーのページ数だけ進み、画面が前のページのまま残る**
不具合を 3 回、別々の場所で直した。3 件は独立した実装ミスではなく、1 つの誤りの現れである。

> **打ち切りの判断はページ単位でされているのに、「必要か」は表示グループ単位で決まる。**

Web の先読み計画・本体の前景描画・見開きの兄弟同士 — どれも「この仕事はもう要らない」を、
**今の表示が何を必要としているかを知らないまま**決めていた。個別に条件 (待機者数 /
アドレス一致) を足すたびに判断の入口が増え、次の入口で同じことが起きた。

### 14.1 確認した事実 (再調査不要)

| 事実 | 根拠 |
|---|---|
| ブラウザの `abort` は本体の処理を止めない | `crates/remote-web/src/ipc_client.rs` の `PAGE_RESPONSE_TIMEOUT` のコメント |
| 前景 1 枠の予約は厳密な優先レーンではない | heavy queue は FIFO。サムネイル・コンテナ列挙が予約枠を使う |
| 補正プレビューは cache / coordinator の外から独立した前景 `/api/page` を送る。`signal` を渡していないので中止手段が無い | `app.js` `AdjustmentPanel.runPreview` |
| 遅れて着いた前景は、アドレスと `spread_partner` に一致しない先読みを取り消す | `src/remote_ipc/container.rs` `begin_page_render` |
| その取消は `MediaErrorCode::Busy` → HTTP 503 `miv_media_error` になる | `crates/remote-web/src/http.rs` の `media_error` 変換 |
| 前景の相乗りが許容する 503 は `ipc_busy` **だけ**。上記は再送出されて表示要求ごと失敗する | `app.js` `PageResourceCache.loadForeground` |
| 相乗り中の PDF は本体で `Normal` lane のまま。前景の `HighNormal` へ昇格しない | `src/thumb_loader.rs` の `pdf_priority` |

ページ 1 枚は実測 p50 1.5 MB / p95 7.9 MB、本体の生成に p50 0.6s / p95 2.0s。

### 14.2 決定

**ブラウザ内だけの修正 (段階 B) では構造的解決にならない。所有権の境界は Web と本体を
またいで一度に切り替える。**

当初 ClaudeCode は「A + B + C を実施し D / E は保留」を推奨した。別セッションの Codex が
反対し、**具体的な破壊経路がコード上に実在する**ことを示した。上表の 3 行 (補正プレビュー →
`begin_page_render` → `ipc_busy` 以外の 503) が繋がると、**前景 lease を持つ仕事が別の前景に
取り消される**。lease はブラウザの中にしか無いので、本体はそれを知らない。ClaudeCode が
実物を読んで裏を取り、判断を変えた。

さらに、暫定パッチ (`143fc596` / `eed5ff93`) を残したまま新しい lease を足すと、
**取消の所有者が 2 つ並存する**。これは CLAUDE.md「相互排他的な状態を複数の bool / `Option` /
pending で表現しない」に反し、今回 3 周した誤りと同じ形である。暫定パッチは cutover で
**撤去する**。段階の途中で止めて完了としない。

### 14.3 段階

**A — 契約と回帰テスト (先行)**

表示グループの outcome 契約を固定する。`loadGroup` の戻り値を `bool` から
`Applied / Superseded / Failed` の typed outcome にし、失敗時にシークバーと画面が食い違わない
ことをテストで固定する。この時点では既存構造のまま落ちるテストを置く。

**B + C + D0 — 所有権の cutover (3a、2026-08-11 完了)**

当初ここに記した「heavy queue への切替まで一体」の段階分けは §14.11 で差し替え済み。
取消 owner を二重にしない一体性は維持し、admission / queue 順序と位置 ownership は切り離した。

- Web: 表示グループが需要を持つ (`DisplayRequestId` による lease)。見開き全ページの
  foreground lease を fetch 開始前に**同期登録**する。計画の更新は plan lease の解放だけ。
  `demands.is_empty()` のときだけ `abort`
- `loadForeground` の「他の active を打ち切る」走査を**撤去**。`foregroundWaiters` /
  `prefetchPlanned` も同じ変更で削除
- 本体: job ID、先読み → 前景の**昇格**、明示的な release / cancel、cancel 理由の型分け
  (取消 / admission / セッション失効)。protocol version を上げる
- 本体の旧アドレス近似 (`begin_page_render` の retain) を**無効化・撤去**
- 補正プレビューも同じ coordinator を通す

**3b — admission / heavy queue (2026-08-11 完了)**

- 段階 2 の `heavy_queue` へ `sync_channel` を差し替え、待機中 job の剪定と lane 昇格を配線した
- `try_acquire_prefetch` の入口拒否を撤去し、1 worker の実行不能な prefetch へ typed 応答を返す
- remote-web の `IpcAdmission` は HTTP worker を守る別 owner として維持する
- 実測は §14.12 末尾。目視では判断できなかったのでログで確かめた

**3c — 位置 ownership (2026-08-12 完了)**

- requested / displayed の identity snapshot を持つ純粋状態機械へ位置所有権を集約し、
  URL / history も同じ owner 境界へ移した。詳細は §14.13

**その他の後段 (保留)**

- 前景専用 lane の高度化
- URL `prefetch=1` 互換の撤去
- telemetry 拡張、性能計測、残存フィールドの整理

### 14.4 壊れない理由

打ち切りの入口が増えても同じ誤りが起きないのは、**入口が cancel token を触らないから**である。
入口は lease を取る / 返すだけで、実際の取消は coordinator が `demands.is_empty()` を見て
決める。有効優先度は需要の最大値で単調に上がる (降格しない)。

`page_display` テレメトリは今回の 3 件を特定した観測の仕組みなので壊さない。

### 14.5 段階 A: 表示グループ outcome 契約 (2026-08-09)

段階 A では、`LatestPageLoadQueue` から `ImageViewer.loadGroup`、単ページ / 見開きの
fetch・decode・DOM 適用、呼び出し側の完了処理までを、次の 3 outcome で統一した。

- `applied` — DOM 適用済み。AI 通知、読書位置、補正 / ブックマーク更新、先読みなどの
  post-display 処理を実行する
- `superseded` — active / pending の追い越しまたは queue clear。位置復元も post-display
  処理も行わず、新しい要求に確定を任せる
- `failed { message }` — 現行要求の fetch / decode / apply / `AbortError` の終端失敗。
  post-display 処理は行わず、失敗メッセージを表示する。未知 outcome と message の無い
  `failed` は契約違反として例外にする

位置を巻き戻すのは、ページ送り / seek の commit が stack-local な位置要求 owner を明示的に
渡し、その owner が完了時にも同じ viewer、`pageGroups` 配列、group object、group index、
group identity、container / folder context を指す場合だけである。fit 変更、補正保存、viewport
resize、generation 更新、見開き再構成など位置変更を所有しない再描画の失敗では位置を動かさない。
`pageGroups` の再構成後は同じ数値 index でも identity が一致しないため、古い失敗は新しい配列の
別グループを選ばない。失敗メッセージは位置復元と表示 feedback の同期後に表示し、ページ名への
上書きで消えないようにした。既存の `page_display` telemetry の `outcome` / `reason` / candidate /
applied request ID は変更していない。

失敗メッセージは**失敗した事実だけ**を述べ、位置を戻したかどうかは完了処理が付け足す。
中断メッセージに「前のページに戻りました」を焼き込むと、位置を所有しない再描画の失敗
(fit 変更・resize・見開き再構成) で戻していないのに戻したと言うことになるため。

**解消済みの残存不整合**: 段階 A で残った URL / history と jump 系の不整合は、
段階 3c で位置変更入口と history を同じ owner へ集約して解消した。失敗時は
requested を displayed へ戻した同じ境界で `replaceState` する。`viewerDepth` と grid 復帰の
判断は §14.13 に固定した。

#### 14.5.1 段階 A で未収束だった経路 (3c で解消済み)

段階 A では要求ごとの token が巻き戻し義務を所有していたため、次の 2 経路が未収束だった。

1. ページ送り B を fit 変更 / viewport resize / generation 更新が追い越し、token を持たない
   後着再描画が終端失敗すると、表示 A に対して位置と seek だけ B に残った。
2. ブックマーク jump が `state.pageGroupIndex` を直接動かし、token を渡さないまま終端失敗すると、
   新しい位置・seek と古い DOM が残った。

段階 3c は「要求が所有する」モデルをやめ、**requested が displayed より先にあること
自体が収束義務を生む**モデルへ移した。そのため、上のどちらにも入口別の分岐や token 配線を
追加せずに収束する。displayed も grouping identity snapshot を持ち、再構成時は `reanchor` でだけ
新しい `pageGroups` へ張り直す。詳細は §14.13。

**テストの現状**: `viewer-position.test.mjs` は上の 2 経路、非位置再描画、古い grouping の
遅延終端、displayed unresolved、長さ 4 以下の操作列総当たりを固定する。`pwa.test.mjs` は
ページ送り / seek / bookmark の 3 入口、history owner、`state.pageGroupIndex` 代入境界、
`open` / `reanchor` / DOM commit の構造を固定する。実ブラウザでの通信失敗と history 操作の目視は
依然として実機 smoke の対象とする。

### 14.6 端末ごとの先読み枚数と画質別処理時間 (2026-08-12)

これは §14.3 の B + C + D0 所有権 cutover とは別の増分である。Web の
`foregroundWaiters` / `prefetchPlanned` / `abortUnownedActive`、本体の page cancel と lease は
変更しておらず、段階 A の `applied` / `superseded` / `failed` outcome と `page_display`
telemetry も維持する。

2026-08-12 の実機ログでは `/api/page` 成功 154 回 / 実質 72 レンディションのうち、再取得が
総転送 306.3 MiB 中 163.7 MiB (**53%**) を占めた。ズーム時は 1 ページ p50 **6.0 MiB** で、
従来の固定 64 MiB には約 **11 枚**しか入らず、前 12 + 後ろ 4 の計画 16 枚が構造的に収まらない。
利用者から見えない byte gate が外側の取得済みページを落とし、表示到達前の再取得を作っていた。

このため byte を prefetch 開始や破棄の判断に使うことをやめ、先読み窓を端末ごとの設定とする。
既定は進む方向 8 / 戻る方向 4、入力範囲はそれぞれ 2〜32 / 0〜16 である。この範囲は入力の
妥当性だけを表し、安全上限ではない。実行時の空きメモリやページ byte で挙動を変えず、設定値だけで
計画を再現できる決定性を優先する。大きすぎる設定でブラウザのタブが落ちる可能性は許容し、端末の
設定を下げて開き直すことを復帰手段とする。

`PageResourceCache` の保持上限は表示最大 2 + **有効な窓**の前後枚数から毎回導出し、窓が変われば
上限も変える。保持 byte の走る合計は破棄 telemetry と調査のために残すが、admission と eviction の
判断には使わない。保護集合は表示中の cache key と、近い順の `pagePrefetchPlan` である。通常の
trim はこの全 key を保護する。満杯から次候補を取得するときは、表示中 key と候補以下の近い key
だけを保護し、候補より遠い取得済み key を LRU 順に交換する §14.9 契約 10 を維持する。近い key
しかなければ開始を止め、保護対象は上限を超えても捨てない。

この枚数設定が抑えるのは先読み storm であり、**1 ページ単体が大きすぎる場合は守れない**。今回の
最大ページは展開後 bitmap が約 186 MB になり得る。1 枚だけでタブが落ちる場合の逃げ道は、同じ
端末設定にある画像の画質を下げて `target_px` と展開寸法を小さくすることである。展開後 bitmap は
表示中の最大 2 ページだけが DOM の object URL を所有する従来契約を維持する。

`/api/page` の成功応答は、既存 log の `ipc_ms` と同じ値を
`X-mIV-Page-Render-Ms` で返す。Web は段階 A の表示 outcome が `applied` になった foreground
ページだけを対象に、生成 = header、転送 = fetch 全体 - 生成 (下限 0)、展開 = image decode を
画質 preset ごとの直近 10 件リングへ保存する。保存先は秘密を含まない端末 localStorage の
versioned な別 key である。画質 UI は標本がある preset にだけ
「直近 N ページの平均 — 生成 / 転送 / 展開」を表示する (N は実際の標本数。
3 件しかないときに 10 と書かない)。この値は preset 固有の性能値ではなく、
現在読んでいる PDF / archive / JPEG の寸法・補正内容・端末・通信に依存する実測であり、標本の
ない preset に推測値は出さない。

#### 14.6.1 レビューで塞いだ 3 点 (2026-08-11)

1. **保持の有界性を `pump()` に依存しない。** `pump()` は開始枠が無いと破棄まで到達
   しない。1 ページだけのコンテナを media route 間で順に開くと先読み計画が空になり、
   `loadForeground` → `remember` だけが走って過去の非保護 entry が 1 件も落ちなかった。
   保持を増やす側 (`remember`) でも枚数上限を確認し、保護集合の外を削る。byte gate の撤去後も
   この境界と保護集合を捨てない性質は維持する。
2. **表示するページは admission の一時的な満杯で失敗させない。** 見開きは 2 ページを
   同時に foreground 要求するので、先読みが枠を持っている瞬間に 2 枚目だけ 503
   (`ipc_busy`) になり得る。先読み窓が広いほど枠占有の継続時間が伸び、この窓が
   広がる。foreground の直接取得だけ、`Retry-After` に従って最大 3 回まで再試行する
   (中止されたら即座にやめる)。先読み側の再試行方針は変えていない。
3. **同じ取得結果の再表示を新しい標本として数えない。** 戻る / fit 変更 / resize で
   同じ blob を出し直すたびに計上され、「直近 N ページ」が 1 枚の再表示で埋まっていた。
   request ID で重複を弾く (記憶は有界)。

### 14.7 見開き再構成の単一 owner と先読み HUD (2026-08-11)

見開き保存後と viewport resize がそれぞれ `refreshContainerSpread` → `loadContainer` を
直接呼び、共有 `requestController` を互いに abort していた。負けた `loadContainer` は
`false` を返し、呼び出し側も表示を更新せず終了するため、最後に残るべき表示義務そのものが
消え得た。また見開き保存の promise は write と refresh を同じ `catch` で囲んでいたため、
refresh の追い越しまで write 失敗として扱い、fallback refresh をもう 1 件発行していた。

見開き再構成は `LatestOnlyTaskQueue` を owner とする 1 本の入口へ集約した。実行中の 1 件は
完了させ、待機中は最新だけを残す。置き換えられた待機要求は `superseded`、実行失敗は
`failed { message }`、表示まで完了した要求は `applied` を返すため、保存、端末設定、resize の
各呼び出し側は追い越しと失敗を混同しない。write の失敗処理も refresh から分離し、成功時・
失敗時とも server の確定状態を読む refresh は 1 件だけ要求する。同じ viewer / container /
現在ページ / `forceSinglePage` の要求は active または pending の同じ promise へ join し、保存と
resize が同じ再構成を要求しても `/api/container` を重ねない。異なる重複要求は直列 owner が
`loadContainer` を実行し、最後の要求が必ず新しい page group の表示まで進む。

その後の実機再現で、owner の直列化後も表示直前に失敗する別の原因を特定した。browser の
double-tap fit ownership を撤去した `acf317e0` で `ImageViewer.cancelPendingCenterTap()` 自体は
削除されたが、見開き再構成と bookmark jump の呼び出しが 2 箇所残っていた。見開き再構成は
ここで同期 `TypeError` となるため `updateViewerImage` / `loadGroup` に届かず、`page_display` が
1 件も出なかった。失効した 2 呼び出しを撤去し、旧 gesture state を別の形で復活させていない。

同じ観測欠落を繰り返さないため、refresh owner の例外は `spread_refresh_error` として stack を
含めて `recordClientError` へ渡し、画面には生の JS message ではなく
「見開き表示を更新できませんでした。」だけを出す。`performContainerSpreadRefresh` の viewer /
container 不一致、load abort / 非適用、現在ページ / group 不在、表示失敗を固定 reason へ分け、
`updateViewerImage` も `loadGroup` 前の viewer / session / cache epoch / group 変化と preload 失敗を
固定 reason で telemetry に残す。normal tier には reason / stage が残り、message / stack は既存の
privacy 契約どおり詳細記録 tier だけに残る。

計測 HUD の先読み表示は最後に動いた `pageDirection` では分割せず、現在の表示 page index より
前を区切りの左、後を右へ置く。このため戻る操作の直後も左右は反転せず、先頭ページでは左側が
0 件になる。page group の順序を使うため RTL / LTR にかかわらずこれから読む側を右へ置く意図も
維持する。先読み計画自体の進行方向優先は変更しない。状態は
`PageResourceCache.statusForKeys()` だけから取得し、内部 map は UI へ公開しない。色は緑 =
`ready` (取得済み)、黄 = abort されていない `active` (取得中)、
黒 = どちらでもない (未取得)。色だけに依存せず、`title` / `aria-label` に取得済み・取得中・
未取得の件数を表示する。通常の `updateHud()` に加え、先読み開始、完了、破棄、cache clear / evict
で更新し、先読み専用タイマーは持たない。12 + 4 + 区切りの全件を減らさず、HUD の先頭は flex
wrap して狭い画面では見出しとドットを別行へ収める。

予算と窓は変更しない。標準画質の実測はページ p50 1.35 MiB / p95 2.61 MiB、本体生成
`ipc_ms` p50 552 ms、ページ要求 222 件中 503 が 52 件 (24%、prefetch 146 / foreground 22)、
同時間帯の `/api/thumb` は 193 件だった。64 MiB は p50 換算約 47 ページに対し窓上限 18 ページ
(約 24 MiB) なので限界要因ではなく、heavy 枠の admission が先に詰まっている。したがって
`PAGE_RESOURCE_CACHE_CONFIGURED_BYTES = 64 MiB`、前方 12 / 後方 4、entry 上限 18 を据え置き、
HUD で詰まり方を観測してから別の変更として判断する。

#### 14.8.1 ブラウザ拡大は viewport で止める (2026-08-11、利用者判断)

ボタンやページ送りを連打すると UI 全体が拡大し、ボタンが見切れる。telemetry の
`visual_viewport_scale` が 1.03 や 1.6 へ上がったまま戻らないことで裏が取れている。

**JS の二度打ち抑止では止まらない。** 2 打目の `touchend` を `preventDefault()` した
**46ms 後**に scale が 1 → 1.03 へ上がった記録がある (`pair_suppressed` /
`suppressed: true` の直後)。`* { touch-action: manipulation }` も全体に効いている状態
での結果なので、抑止対象を広げても解決しない。実際、ボタンへ広げる案は click 再送を
伴い、click 以外で起動する部品を壊しかけた (`2ca1454f` を revert)。

**JS の抑止は tap も落としている。** 直近の実機ログでは tap 判定 65 件に対して command は
59 件だった。差の多くは正常な drag (`travel_exceeded`) だが、`pair_suppressed` の直後に
command が無い tap が含まれていた。2 打目の既定を止めると合成 click も失われ、ページ送りや
seek の入力が消える。拡大を止められない一方で入力欠落だけを作るため、この抑止には利点が残らない。

そこで viewport meta に **`maximum-scale=1, user-scalable=no`** を入れる。ホーム画面へ
追加した standalone では iOS がこれを尊重する。通常タブでは無視されるが害は無い。

この実測を受け、document listener は 2 打目を含む**すべての tap の既定動作を残す**観測専用へ
変更した。button / link / input / 素の要素を分けていた `DEFAULT_TAP_EXCLUSIONS` は撤去し、
対象種別による分岐を持たない。時間窓・距離による pair 認識、移動量判定、multi-touch の除外、
`onDecision` 通知は残す。成立した対は `pair_recognized` として記録し、`suppressed: false` を伴う。

- **失うもの**: UI 全体をピンチで拡大する操作。**画像のピンチ拡大はアプリが自前で
  持っている**ので影響しない
- 以前は逆の判断 (ブラウザのピンチ拡大を残す) を `pwa.test.mjs` の
  `assert.doesNotMatch(html, /maximum-scale|user-scalable/i)` で固定していた。上の実測を
  根拠に反転させ、テストも現在の意図を固定する形へ書き換えた
- standalone ではない通常タブでは iOS が viewport 指定を無視し、browser zoom が起こり得る。
  JS の `preventDefault()` でも止められないことが実測済みなので、通常タブでの拡大は入力欠落を
  再導入せず受け入れる
- document listener は zoom の再発と tap 入力の相関を取る唯一の観測点なので撤去しない。
  pair 認識と `visualViewport` の scale 変化の相関を引き続き telemetry に残す

### 14.9 表示所有権 cutover 段階 1: coordinator 契約 (2026-08-11)

段階 1 では `page-coordinator.mjs` を DOM / fetch / timer / `AbortController` に依存しない
純粋な状態機械として追加し、次の契約を固定した。

1. ジョブを打ち切るのは表示需要と plan 需要がともに空になったときだけとする。例外は
   session 失効 / context reset が需要全体を無効化する `invalidate` だけであり、通常の入口は
   cancel token を直接触らず lease を取る / 返す。
2. ジョブの有効優先度は需要の最大値で、`prefetch` から `foreground` へ単調に上がる。
   表示需要が外れても降格しない。
3. `promote` は同じジョブに高々 1 回だけ出す。最初から foreground のジョブには出さない。
4. まだ開始していない plan 項目を外しても `cancel` effect は出さない。
5. 表示グループは 1〜2 ページを一単位とし、全員 ready のときだけ `group_ready`、1 枚でも
   failed / aborted なら `group_failed` を高々 1 回出す。見開きの片側失敗を部分適用せず、
   反対側で取得済みの byte は cache の判断まで保持する。
6. `releaseDisplay` 後の要求へ `group_ready` / `group_failed` を出さない。遅れて届いた結果は
   typed な `ignored` とする。
7. 完了は key ではなく job ID で照合する。同じ key の新ジョブ開始後に旧 job ID が届いても
   `stale_job` として無視し、新ジョブと現在の表示要求を変えない。
8. `releaseDisplay` は冪等で、二重 release は `unknown_request`、同じ request ID の二重 open は
   `duplicate_request_id` として無視し、需要を二重に増減しない。
9. 1 要求内の同じ key は 1 需要として数える。
10. `protectedKeyIds()` は全表示需要を先に、続いて plan の近い順を返す。cache は通常の
    trim ではこの全 key を保護し、prefetch admission ではこの順序を唯一の正本として
    候補 key までを保護する。
11. failed / aborted は key の恒久状態ではなく、その job attempt の終端結果とする。
    `settle` 時点でその attempt を待っていた要求だけへ結果を記録し、key から current job を
    外す。plan 需要だけでは同じ失敗を自動再試行せず、新しい `openDisplay` を明示的な再試行
    シグナルとして対象 key の失敗記憶を解除する。key が全需要を失ったときと `invalidate`
    でも失敗記憶を破棄し、window への再進入時には再試行できるようにする。

effect の順序も `ignored` → `cancel` → `promote` → `start` → group 終端通知に固定した。
coordinator が所有するのは需要、単調な優先度、job ID とジョブ生死、表示グループ結果である。
圧縮 Blob の保持 / LRU / 破棄 / 枚数 admission と観測用の byte 会計は `PageResourceCache` が持ち、
coordinator は `hasBytes` と `prefetchAdmits(candidateKeyId)` でその状態を読むだけとする。
prefetch 開始判断では plan 順に現在の候補 key を渡し、cache は `protectedKeyIds()` の先頭から
その候補までを admission の保護集合にする。これにより候補より遠い取得済み key は交換できるが、
表示中 key と候補以下の近い key は捨てない。ただしジョブ登録簿を cache と coordinator の
2 箇所へ分裂させないため、foreground / prefetch を問わず全 job ID の発行と開始判断は
coordinator に一本化する。

完了 / cancel 済み job の履歴は 256 件を上限とし、running job は破棄しない。**まだ需要のある
key の current job も破棄しない。** 上限は記憶量の制限であって、そこで生きた routing を
書き換えてはならない (ready のまま current な job を落とすと、読者が既に持っているページを
もう一度取得しにいく)。これらは需要のある key の数で上限が付き、需要が消えた時点で
`#reconcile` が current から外して破棄可能になるため、それまでは履歴が上限を少し超える。
上限から押し出された job ID への遅い `settle` は、保持中なら判別できた `stale_job` /
`already_settled` から typed な `unknown_job` へ理由の精度だけを落とす。

retry は coordinator に持たせない。`Retry-After`、待機 timer、HTTP の一時失敗判定は副作用を
実行する adapter の責務であり、再送しても coordinator からは同じ job ID のままである。
coordinator の `settle` が受けるのは ready / failed / aborted の終端結果だけとする。

`pageResourceKey` は address、target size、render revision、generation、session ID、
`sessionCacheEpoch`、正規化した render context (`context_address` / `display_slot` /
`spread_partner`)、補正 preview を固定順の JSON 配列へ直列化する。object は再帰的に key を
sort する。改行連結を使わないため値中の改行や引用符で欄境界が曖昧にならない。現状は
`sessionCacheEpoch` が session ID の変化からだけ進み、同時に cache clear されるが、これは
偶然維持されている不変条件である。epoch 自体を key に含め、将来 epoch だけ進む入口が増えても
旧 byte を共有しない。補正 preview は内容を identity に含め、cacheable にはしない。

位置の requested / displayed 所有権はこの段階へ前倒ししない。`openDisplay` は将来の位置 owner を
載せる `groupKey` だけを保持し、§14.5.1 の非位置再描画による追い越しと bookmark jump の 2 経路は
B + C + D0 cutover で `DisplayRequestId` 境界へまとめて移す。

この増分は dormant である。`app.js` から import せず、既存の `loadSequence` /
`fetchController` / `foregroundWaiters` / `prefetchPlanned` / `abortUnownedActive` と本体の
`begin_page_render` はそのまま残るため、実機の表示・protocol version・telemetry は変わらない。

### 14.10 表示所有権 cutover 段階 2: 本体側基盤 (2026-08-11)

段階 2 では本体側に `remote_ipc::page_jobs` と `remote_ipc::heavy_queue` を追加した。
前者は connection ごとの page job ID、display request ID、需要の生死、単調な
Prefetch → Foreground 優先度、明示的な release 理由と cancel token を持つ。後者は
payload に依存しない 3 レーン (Foreground / Interactive / Prefetch) の順序、レーン別容量、
Prefetch が最後の 1 worker を使わない制約、待機中 job の昇格と剪定、blocking pop /
shutdown だけを持つ。

優先度の正本は registry であり、queue の lane はその写しである。registry が queue を
呼ぶ、または queue が registry を呼ぶ構造にはせず、段階 3b の配線側が両者を同じ
critical section で揃える。両操作は冪等なので、途中の no-op や失敗は typed な結果として
観測し、次の昇格操作で写しを正本へ追従させられる。

registry の記録は job の `finish` で消える。ただし **connection が切れたときは誰も `finish` を
呼ばない** (待機中の payload は剪定され、実行中の render は自分の cancel token だけを見る)
ので、`close_connection` は打ち切りと同時にその connection の記録ごと破棄する終端操作とする。
再接続のたびに 1 件ずつ残す形にしない。既に走っていた render が後から `finish` を呼ぶと
`UnknownJob` になるが、これは close 後の想定どおりの結果であり異常ではない。段階 3a の
配線でこれを警告として記録しないこと。session 失効は connection が生き残り、各 job が
完了を報告できるので `cancel_all` を使い、記録は残す。

3 レーン化だけで現在の入口拒否を外すのは不十分である。昇格と D0 の需要ベース剪定が
無いままでは、読者が先へ進んだ後も古い先読みが queue に残り、後着の前景が待たされる。
現行の `begin_page_render` は worker が queue から pop した後に走るため、queue で待機中の
先読みを剪定できない。そのため段階 3a では enqueue 時から registry token を持たせ、
段階 3b で待機 payload の物理的な剪定を同じ ownership へ配線する。

`heavy_queue::prune` は落とした payload を黙って破棄せず呼び出し側へ返す。段階 3a で載せた
`Work` が client への `reply` を所有しており、剪定した要求へ typed な終端応答を返さないと
呼び出し元が永久に待つためである。先読みの 1 worker 予約は、入口で要求を拒否する
方式から queue の pop 条件 (`active < workers - 1`) へ移す設計とした。ただしこの制約は
**Prefetch に対してだけ**働く。Interactive のサムネイルやコンテナ列挙は全 worker を使えるため、
それらが実行中なら後着の Foreground page も待つ。読者が列挙の完了を待っているときは
Interactive も読者が待つ仕事であり、ここまで予約で制限すると grid を不要に遅くするため、
この狭い保証を意図した仕様とする。この増分では
`try_acquire_prefetch` の入口拒否を撤去していない。

worker が 1 本の場合は `workers - 1 == 0` のため Prefetch は queue から pop されない。
現行の入口は `remote_page_prefetch_limit(1) == 0` として typed な 503 を即応答するので、
段階 2 では回帰ではない。段階 3b で入口拒否を撤去すると、実行不能な Prefetch は client timeout まで
queue に残り得る。**1-worker pool の実行不能 Prefetch へ誰がいつ typed 応答を返すか**は、
段階 3b の admission / prune 配線で必ず決める未解決事項とする。

この増分も dormant である。新しい 2 モジュールは request 経路から呼ばれず、現行の
`sync_channel`、worker 数、queue 容量、`begin_page_render` は変更していない。protocol version も
据え置きで、実機の表示や telemetry に変化は無い。

### 14.11 表示所有権 cutover 段階 3a: Web / 本体の同時配線 (2026-08-11)

段階 3 は 3a / 3b / 3c に分割した。3a は取消と優先度の ownership だけを Web と本体で
同時に切り替え、3b は queue 順序と admission、3c は requested / displayed 位置を扱う。
取消 owner を一度に 1 つへ切り替える §14.2 の決定は変えていない。一方、現行
`sync_channel` と入口拒否を同時に変えると、実機で ownership 配線と並べ替えの副作用を
切り分けられないため分けた。§14.3 の従来の段階記述はこの分割で差し替え済みである。

3a の判断は次のとおり。

1. plan だけが需要を持つ prefetch job には display request ID が無いため wire / registry とも
   optional とし、表示需要による promote 時にその display request ID を記録する。
2. 実効優先度の正本は `PageJobRegistry` とする。`PageRequest.priority` は初期値であり、pipe が
   dispatch 直前に registry から再解決する。container は registry を知らない。
3. release、接続断、停止で捨てる page job には `MediaErrorCode::Cancelled` を必ず返す。
   remote-web は HTTP 409 + `miv_media_cancelled` とし、503 の busy 再試行へ載せない。
4. render が見る取消源は registry 発行 token だけとする。session drain は
   `cancel_all(SessionInvalidated)`、接続断は `close_connection(ConnectionClosed)` を呼ぶ。
   page 以外の仕事が使う `SessionOperation::cancel_flag()` は維持する。
5. release が GET / register を追い越す競合は、connection ごとの有界な released-job 墓標で
   塞ぐ。後着 register は最初から取消済み token を得て queue へ入らない。promote の追い越しは
   prefetch のまま走るだけなので best-effort とする。
6. 1 worker 環境で実行不能になる prefetch の扱いは 3b に残す。3a では
   `try_acquire_prefetch` の入口拒否が残るため新しい待ち詰まりを作らない。
7. remote-web の `IpcAdmission` は HTTP worker を守る別目的の owner なので変更しない。

Web の `PageDisplayCoordinator` が唯一の需要 owner となり、見開き全ページを fetch 開始前に
同期登録する。effect adapter は start を fetch、promote / release を同一 tick の batched
`PageDemand`、group 終端を既存の `applied / superseded / failed` 契約へ接続する。補正 preview も
要素数 1 の consumer として同じ coordinator を通す。`PageResourceCache` は圧縮 byte、LRU、
64 MiB 予算だけの owner となり、保護集合を `protectedKeyIds()` から受け取る。通常の trim は
全表示 key と全 plan key を保護する一方、coordinator は prefetch 開始候補を
`prefetchAdmits(candidateKeyId)` へ渡し、admission だけは ordered な `protectedKeyIds()` を
候補までで切る。候補より遠い取得済み plan key は LRU 順に交換可能だが、表示 key と候補以下の
近い plan key は捨てない。別の plan 順序は持たない。

この cutover で、本体の `begin_page_render` / `page_prefetches` による住所・spread partner 近似と、
Web の `loadForeground` による他 active の取消走査、`foregroundWaiters`、`prefetchPlanned`、
`abortUnownedActive` を撤去した。これら 4 系統の取消判断は、表示需要と plan 需要を合わせた
1 つの lease と registry token に置き換わった。

protocol は、作業開始時の実リポジトリが既に v41 だったため、ブリーフの旧 baseline
v37 → v38 ではなく **v42** とした。v42 は `PageRequest.job_id`、optional な
`display_request_id`、batched `PageDemand` と typed `Cancelled` を追加する。本体と remote-web は
必ず両方を再ビルド・再起動する。3a では `heavy_queue.rs`、`sync_channel`、worker 数 / 容量、
両側の prefetch admission、先読み窓 12/4、64 MiB 予算、画質 preset、位置 ownership、
既存 `page_display` telemetry field を変更していない。

**実機確認 (2026-08-11)**: 通常の閲覧は問題なしと利用者が確認した。ページ送り、見開き、
遅い PDF / ZIP、先読み中ページを開く、補正プレビューの追い越しを含む。
**未確認は 2 つある**。① 64 MiB 予算を埋めた深い高解像度コンテナで、遠い取得済みページを
近い候補と交換する経路 (§14.6 の規則。候補スコープを外すと `full byte budget evicts a farther
planned page before fetching a nearer one` が実際に落ちることは自動テストで確認済み)。
② 表示中の切断 / 再接続とセッションの解放 / 再取得。どちらも段階 3b が queue の剪定と
入口拒否の撤去で同じ経路へ触れるため、**3b の実機確認で必ず併せて見る**。

### 14.12 表示所有権 cutover 段階 3b: heavy queue / admission 配線 (2026-08-11)

段階 3b では本体の heavy lane を単一 FIFO の `sync_channel` から、段階 2 で追加した
`HeavyQueue` の Foreground / Interactive / Prefetch 3 レーンへ切り替えた。page は
`PageJobRegistry` の実効優先度を enqueue 直前にも読み、Foreground または Prefetch へ入れる。
サムネイル、コンテナ列挙、AI、video jump など、それ以外の heavy work は Interactive とする。
優先度の正本は引き続き registry、queue lane は写しであり、どちらからも相手を呼ばない。
pipe の glue mutex が job ID と `(connection_id, request_id)` の対応を持ち、promote / release
時に両 owner を直列に更新する。dispatch 直前にも registry を再解決する契約は変えていない。

この切替により、本体側の `try_acquire_prefetch`、`PrefetchPermit`、`prefetch_in_flight`、
`MAX_CONCURRENT_PAGE_PREFETCH`、`remote_page_prefetch_limit` を撤去した。先読みが最後の
1 worker を占有しない制限は入口の拒否ではなく、queue の pop 条件
`active < workers - 1` が所有する。サムネイルが待機中というだけでは先読みを拒否しない。
ただし worker が 1 本ならこの条件を満たす先読みは 1 件も無いため、enqueue 時に
`prefetch_unavailable_with_single_worker` として typed に記録し、従来どおり busy を即応答する。
実行不能な payload を 10 分の page timeout まで保持しないための、§14.10 の宿題への回答である。

page release は registry token を立てた同じ glue 操作で、まだ queue に待機している payload を
物理的に剪定する。接続断ではその connection の待機 work だけをまとめて剪定し、別 connection
には触れない。shutdown は `HeavyQueue::shutdown` で全待機 payload を返す。release / 接続断で
剪定した page には `MediaErrorCode::Cancelled`、shutdown には各 message 種別の停止応答を返す。
`Work.reply` を黙って drop せず、queue から返った全 payload に typed な終端応答を返すことを
glue 層の義務とする。既に実行中なら queue からは取り出さず、registry token による段階 3a の
取消をそのまま使う。

容量は単一 16 件をやめ、Foreground 8 / Interactive 16 / Prefetch 8 の lane 別上限とした。
remote-web の `MAX_CONCURRENT_IPC = 6` / `MAX_CONCURRENT_HEAVY_IPC = 4` /
`MAX_CONCURRENT_PAGE_PREFETCH = 2` が通常の queue 深さを既に数件へ制限しているため、
Foreground / Prefetch の 8 は promote や再接続の重なりを含めても 2 倍の余裕がある。
Interactive 16 は direct IPC client がサムネイルと列挙を混在させる場合に旧上限を維持する。
重要なのは合計を深くすることではなく、Prefetch lane が満杯でも Foreground admission が
独立して成功することである。この queue は大量滞留を捌く owner ではなく、先読み 1〜2 件の後ろへ
後着 foreground を置かないための短い並べ替え境界である。

heavy の `queued` / `active` は `QueueMetrics` との二重会計をやめ、queue snapshot だけを
出所とする。既存の `queue=heavy`、`queued`、`active`、worker 名、`queue_wait_ms`、
`outcome`、`duration_ms`、`reply_ok` は維持し、`lane` と Foreground / Interactive /
Prefetch ごとの queued / active を追加した。home / write / stream の channel、worker、
`QueueMetrics` は変更していない。worker 数の `(設定値 / 2).clamp(1, 3)`、remote-web の
`IpcAdmission`、wire 形式と protocol v42、位置 ownership も据え置きである。

queue に入った page job の対応表への insert は release 最適化でも必ず実行する通常文とし、
想定外の mapping 置換は `page_queue_reconcile` の typed log に残す。worker が pop した active
key は lane と page mapping を持つ completion guard が所有し、正常 return だけでなく handler
の unwind でも `HeavyQueue::complete` を呼ぶ。page render、container 列挙、collection query
などが panic しても active slot と prefetch 用 N-1 判定を永久に消費しない。

**実機確認 (2026-08-11)**: 利用者の操作では異常なし。ただしサムネイル読み込みが速く終わるため、
先読みの詰まりは目視で判断できなかった。**ログで測った** (`target/dev-runtime/data/logs/mimageviewer.log`)。

| 項目 | 実測 |
|---|---|
| heavy lane の処理 | thumbnail 272 / page_prefetch 60 / page_foreground 22 / container 12 / folder_list 5 |
| 拒否・剪定 | `queue_full` / `lane_full` / `queue_pruned` / 1-worker 拒否 / `outcome=cancelled` **すべて 0 件** |
| 昇格が queue へ届いた回数 | 9 件 (`registry=Promoted queue=Promoted` 1 / `queue=Running{Prefetch}` 8) |
| queue 待ち | foreground p50 0ms / max 228ms、prefetch p50 0ms / max 340ms |

**サムネイル 272 件と先読み 60 件が同じ lane で混在したうえで、先読みは 1 件も拒否されていない。**
旧実装は `queued > 0` で無条件に拒否したので、この構成が 22-24% の 503 の発生源だった。
`queue=Promoted` が 1 件出ていることで、release ビルドでのみ壊れていた昇格経路
(`debug_assert!` が insert ごと消えていた) が実機ビルドで動いていることも確認できた。
残り 8 件の `Running{Prefetch}` は pop 済みで queue に動かす対象が無い正常な結果である。

**未確認は 3a から持ち越した 2 つのまま**。64 MiB 予算を埋めた深いコンテナでの遠近交換と、
表示中の切断 / 再接続・セッション解放。どちらもこの回の操作では発生条件に届いていない。

**実機確認 (2026-08-11)**: 混在フォルダが単ページで開き件数サマリが出ること、画像のみフォルダの
既定見開き、保存値の優先、ZIP / PDF の本既定がいずれも従来どおりであることを利用者が確認した。

### 14.13 表示所有権 cutover 段階 3c: requested / displayed 位置 (2026-08-12)

段階 3c では、位置変更要求ごとの `positionRequest` token を廃止し、
`viewer-position.mjs` の純粋状態機械が持つ `(requested, displayed)` の identity snapshot 対へ
巻き戻し義務を移した。snapshot の中身は状態機械が解釈せず、
`viewerPageGroupRequestMatches` だけで同一性を比較する。終端判定は
`viewerGroupLoadCompletionPlan({ loadRequest, currentRequest, displayedRequest })` の 1 本に集約し、
状態機械からこれを呼ぶ。

判定順は次で固定した。`superseded` と現 requested に一致しない `loadRequest` は
`IGNORE`、一致する `applied` は `POST_DISPLAY`、一致する `failed` で
requested ≠ displayed なら `ROLLBACK`、requested == displayed なら `REPORT_FAILURE` とする。
displayed が unresolved (`null`) の場合は戻し先がないため巻き戻さず `REPORT_FAILURE` にする。
これにより、非位置再描画がページ送り B を追い越しても requested(B) ≠ displayed(A) から
自動的に巻き戻し義務が生じる。bookmark jump も同じ `requestPageGroup` を通るので、
§14.5.1 の 2 経路は入口別の分岐を追加せずに閉じた。

`requested` と `displayed` はどちらも `pageGroups` 配列、group object、index、entry identity、
container context を含む snapshot である。DOM 差し替え後の既存 commit 点
(`ImageViewer.commitPagePresentation`) だけが `display(snapshot)` を呼ぶ。grouping 再構成は
`setSinglePageGroups` / `setContainerPageGroups` が再構成前の両 snapshot を保持し、再構成後に
`reanchorViewerPageGroups` を通じて `reanchor` する 1 境界に集約した。requested は再構成前の
requested anchor entry、displayed は DOM commit 時の displayed anchor entry を新配列で引き直す。
見つからない側は unresolved とし、古い数値 index を新配列へ当てない。

adapter 境界では、owner と `state.pageGroupIndex` が食い違わないこと、displayed は生きている
`pageGroups` 配列へ解決済みの snapshot しか持たないことを不変条件とする。DOM commit は読み込み開始時の
snapshot を `resolveReanchoredViewerPosition` で現在の配列へ解決できた場合だけ `display` し、解決不能なら
displayed を据え置く。request / rewind の snapshot を `state.pageGroupIndex` へ適用できなかった場合は、
型付き理由を記録した上で `reanchorViewerPageGroups` により owner と index を生きている配列へ収束させる。

**URL は常に requested を写す**。ページ送り、seek commit、bookmark jump の位置変更は
`requestPageGroup` の 1 owner だけを通り、`request` が実際に動いたときだけ
`history.pushState` する。巻き戻しが実際に起きたときは displayed の hash へ
`history.replaceState` する。request 時の `viewerDepth + 1` は従来どおりとし、rewind では
depth を減らさない。このため rewind 後の back が 1 回同じページへ戻る無駄な操作になるが、
履歴 entry 数と `viewerDepth` を食い違わせず、`history.go(-viewerDepth)` による grid 復帰を壊さない判断である。

`rewind({ expected })` は冪等で、seek の session 取得失敗など表示開始前の中止では、
後着要求を戻さないよう自分の expected snapshot がまだ requested に一致する場合だけ戻す。
`updateViewerImage` は token を受け取らず、読み込み開始時の `loadRequest` と状態機械の現在値だけで
終端処理を決める。`page_display` / `viewer_update` telemetry の既存 field / outcome、
段階 3a / 3b の coordinator / registry / heavy queue / lease / `PageDemand`、本体コード、protocol v43 は変更していない。

回帰は `viewer-position.test.mjs` の契約・2 経路・非位置再描画・再構成・unresolved・
長さ 4 以下の全操作列、`command-core.test.mjs` の終端判定、`pwa.test.mjs` の単一 owner 構造で固定した。

**実機確認 (2026-08-12)**: 通常のページ送り / seek / bookmark jump が従来どおり動くこと、
および機内モードで読み込みを失敗させたときに画面・ページ番号・タイトル・URL が揃って前ページへ
戻ることを利用者が確認した。**読み込み中に通信を切る**形では発生タイミングを合わせられず、
**先に機内モードにしてからページを送る**手順で再現した。以後この経路の実機確認はこの手順を使う。

**残るテストの負債**: 巻き戻し時に state / seek / title / URL が揃うことは `pwa.test.mjs` の
構造 assertion 止まりである。`app.js` の `state` がテストから触れないため、実挙動テストには
テスト用 seam が要る。純粋状態機械側の判定は総当たりで固定済み。

### 14.14 リモート接続 PIN と保存先の所有権 (2026-08-12)

PIN の永続化は本体だけが所有し、remote-web の `--set-pin` は撤去した。認証ファイルの形式、
PIN 長検証、Argon2id パラメータ、salt / session 署名鍵生成、読み書きと record 検証は
`mimageviewer-ipc` を正本とし、同じ hash 形式を両 executable へ重複実装しない。
本体の service owner worker が `<data_dir>/remote-web-auth.json` を temp file + rename で更新し、
有効中なら managed child を再起動する。UI は本体が検証した設定状態を表示し、未設定時の有効化を拒否する。

接続ダイアログは checkbox と OK / キャンセルによる保留を廃止し、有効化 / 無効化ボタンの 1 クリックで
設定保存と service 制御まで即時反映する。閉じるボタン、×、Esc はダイアログを閉じるだけである。
無効時は PIN 未設定なら有効化ボタンを disabled にして理由を隣へ示す。有効化が 1 クリックで確定するため、
対象範囲の警告は無効状態のあいだ有効化ボタンの直上へ常時表示し、利用者が押す前に認識できるようにする。
有効状態では警告を表示しない。状態、利用状況、`tailscale serve`、PIN、QR、URL は同じダイアログ内で
更新し続け、有効化後の準備中から接続情報受信まで開き直しを必要としない。

保存場所は書き手で分ける。認証ファイルは本体が書くため data directory 内に置き、remote-web からの
読み取りだけを許す。診断ログは remote-web が書くため、解決済み `<data_dir>` の末尾名へ
`-remote` を足した兄弟に置き、本体が directory を作成して解決済み path を渡す。
data directory の外にする理由は「remote-web は本体 data directory へ書き込まない」不変条件であり、
同時に data directory から導出する理由は `--data-dir` の隔離と同時起動する名前空間を保つためである。
兄弟を作れない filesystem root は別領域へ黙って fallback せず、service 起動を明示的に失敗させる。

この機能は未リリースで旧 cwd 相対の `remote-web-auth.json` は利用者環境に存在しないため、
移行コードは追加しない。開発機に残る旧ファイルは参照されなくなるだけとする。

### 14.15 `tailscale serve` の検出と設定導線 (2026-08-12)

従来は `serve status --json` の `Web` に `.ts.net` のキーがあるだけで設定済みと判定していたため、
別サービスを配信中でも mIV 用と誤認していた。現在は各 handler の `Proxy` の host / port が、本体所有の
`127.0.0.1:<port>` と一致し、かつ handler path が `/` のときだけ設定済みとする。SPA の asset / API /
stream / Service Worker は origin root を正本とするため、`/miv` のようなサブパスに自分の proxy があっても
動作 URL として案内しない。非 root path は v47 の未対応サブパス情報として本体へ運び、接続できない理由を
警告したうえで、root へ設定し直すボタンは有効なまま保つ。`/` が別 proxy 先なら衝突情報も本体へ運び、
上書きになることをボタン前に表示する。同時に自分の root handler が見つかれば、非 root handler より
root を優先して設定済みとする。

設定変更は接続ダイアログに `tailscale serve --bg <port>` そのものと意味を示し、利用者が押したときだけ
owner worker が最大 8 秒の CLI 実行を行う。成功時は所有する remote-web child を再起動し、その service が
実際に配信する公開 URL を起動時に選び直す。serve の確認と設定ボタンは service の稼働には依存せず、
リモート接続が無効の間も表示・操作できる。

tailnet JSON の解釈は `mimageviewer-ipc` の共有 probe が 1 つだけ持ち、本体と remote-web はそれを呼ぶ。
remote-web は起動時に 1 回読んで自分の公開 URL を決める。本体は接続ダイアログの案内のため、ダイアログを
開いたとき、利用者が再確認を押したとき、serve 設定の成否が返ったときに随時読み直す。両者は同じ関数を
通るので解釈が分かれない。本体から remote-web へ状態再読込を要求する逆向き IPC は追加しない。service が
停止中でも本体自身が確認できることが、PIN や有効化より先に不足を案内するための不変条件である。

解除の代行は行わない。Tailscale 1.98 の documented な解除は対象限定を保証できず、`reset` は利用者の
別用途の serve 設定まで消すためである。今回は Tailscale 側で解除する案内だけを出す。将来代行するなら
`get-config` / `set-config` を往復し、mIV の handler だけを外して他の設定を保存する形を前提とする。
`tailscale funnel` は使わず、bind も `127.0.0.1` のままとする。

### 14.16 tailnet 前提条件の検出と案内 (2026-08-12)

`tailscale status --json` の top-level `CertDomains` と `Self.KeyExpiry` から前提条件を検出する。
解釈は §14.15 の共有 probe に集約し、remote-web の起動時 URL 選択と本体の随時確認で重複実装しない。
`CertDomains` が 1 件以上なら
HTTPS 証明書を有効、空または欠落なら無効とし、CLI の不在・実行失敗・JSON 不正は不明にする。
`Self.KeyExpiry` は解釈できる RFC3339 文字列だけを unix 秒へ変換し、null・欠落・解釈不能は
情報なしとする。CLI 自体が見つからない場合は実行失敗・JSON 不正と区別し、PIN 行より前に Tailscale を
インストールする案内と入手先を表示する。CLI はあるが状態を読めない場合は別の案内にする。

HTTPS 証明書が無効なら `tailscale serve` は証明書を取得できず必ず失敗するため、接続ダイアログは
serve 設定ボタンだけを無効にし、Tailscale 管理コンソールの DNS ページへ案内する。不明時は従来どおり
状態を読み取れない旨を示し、serve ボタンの可否は変えない。リモート接続自体の有効化は PIN だけで決め、
証明書、期限、Tailscale の有無では止めない。ローカルの `http://127.0.0.1` 確認経路を残すためである。
公開 URL と QR コードだけは共有 probe の候補 URL ではなく、接続中の remote-web が通知した
`RemoteWebConnectionInfo.public_url` を使う。これは tailnet の現在候補ではなく、service が実際に
配信している場所だからである。**表示条件も同じ snapshot の `tailscale_serve` が設定済みのときだけ**とする。
serve が無い間の `public_url` は bind fallback (`http://127.0.0.1:<port>/`) になり得るが、QR は
読まずに scan されるため、端末から届かない宛先を描いてはならない。案内側の probe が設定済みを示していても
snapshot がまだ古い場合は QR を出さない。これは service を再起動するまで公開 URL が更新されないためで、
その場合に古い URL を配るより出さない方が正しい。

期限が得られた場合は日付と残り日数を表示し、30 日以内と期限切れを警告色にする。期限切れでは
PC が tailnet から外れて外出先から接続できないことを明示し、デバイス一覧へ案内する。情報なしでは
期限の行を出さず、無期限とは断定しない。開発機では `KeyExpiry: null` だけが観測され、期限が入る JSON の
実機形を確認できていないためである。この 2 設定は tailnet / デバイスの管理設定でローカル CLI から
変更できないので、mIV は検出と案内に留め、変更を代行しない。

### 14.17 起動直後の先読み窓を絞る (2026-08-12)

ブラウザのタブがメモリ不足で落ちても、読書位置の再開と URL hash のどちらも同じ本の同じページへ
戻す。設定した先読み窓を直ちに再現すると、設定画面を開く前に同じ負荷がかかり、再読み込みのたびに
落ちるクラッシュループになり得る。その復帰余地を作るため、本を開いた直後は進む方向 / 戻る方向を
それぞれ `min(設定値, 4)` とする。4 未満の設定を広げることはしない。

その本で最初のページ移動が成立した時点で制約を解除し、端末に保存された前後の設定値へ戻す。別の本を
開いた場合は移動済み状態をリセットし、再び最大 4 枚へ絞る。判定は
`{ configuredAhead, configuredBehind, movedSinceOpen }` だけを入力にする純関数であり、時刻、端末の
空きメモリ、直前の失敗などで変化させない。これにより §14.6 の「設定だけで挙動が決まる」契約と、
クラッシュ後に先読み枚数または画質を下げられる復帰経路を両立する。

### 14.18 保存済み編集の座標系をカタログから分離 (2026-08-12)

実機では PDF の 1 ページ目に対する foreground 要求が成功扱いのまま 27×44 ピクセル / 822 バイトに
縮み、全画面へ引き伸ばされて青一色に見えた。PDF catalog の `source_width/source_height` は raster
ピクセルからページ box の 1/1000 point 固定小数点へ意味が変わっていたが、remote がこの値を
保存済み crop とコミック注釈の編集座標として渡していたため、実ピクセル座標の crop が約 1/140 に
縮小されたことが原因である。これを remote がその要求で描いた raster 寸法へ置き換えた最初の修正も
不十分だった。2894px 幅の raster で作られた矩形を、8192 要求で 5676px 幅になった raster へ等倍で
適用したため、今度はページの左上だけが全画面へ拡大された。

カタログの `source_*` はレイアウトと縦横比を求めるための値であり、保存済み編集の絶対座標の基準には
しない。また `CropSettings` は矩形だけを保存し、その矩形を作った基準寸法を持たない。このため PDF の
編集基準は、ページ固有で要求解像度に依存しない正準ラスタ寸法でなければならない。remote は raster
PDF では `canonical_pdf_raster_dims`、すなわち canonical renderer が返す `native_dims` と同じ寸法を
`StoredEditSpace` に入れ、target_px が 1024 / 4096 / 8192 のどれでも同じページ割合を切り出す。
通常画像は従来どおり decode 後の元画素寸法を使う。安定した native pixel space がない vector PDF は、
現在要求の raster へ黙ってフォールバックすると再び範囲が動くため、保存済み crop / コミック注釈が
あってもその raster へ矩形を適用しない。一方、本体は vector かどうかで分岐せずページ表示自体を
失敗させないため、remote もページ全体の描画を継続し、適用しなかった対象と理由を型付きログに残す。
これにより誤った範囲を返さず、文書系 PDF の主流である vector ページを remote だけ閲覧不能にしない。
vector ページにも解像度非依存の正準寸法を与える案は、本体側のページ枠まわりが落ち着いてから扱う。

カタログ値を使う remote の残り 1 箇所は縦横比による見開き分類だけであり、絶対座標へは変換しない。
`StoredEditSpace` は downstream でカタログ値との混同を防ぐだけでなく、constructor でも PDF に
要求時 raster を fallback できない形にした。

本体側は `source_*` を px の意味へ戻す方向で別途扱うが、この remote 修正はその決着に依存せず、
カタログ列の意味が将来変わっても保存済み編集の座標系を維持する。

### 14.19 表示単位のデコード先読み (2026-08-12)

取得済み byte があっても高画質 8192 の表示待ちのほぼ全てを `<img>.decode()` が占める実機結果を受け、
byte 先読みとは別にデコード済み要素を保持する。保持単位はページではなく `pageGroups` の表示単位とする。
見開きは 2 ページが揃うまで表示できず、片側だけをデコードしても待ち時間を解消できないためである。

窓は現在の表示単位と前 2 単位、後ろ 2 単位の最大 5 単位で固定する。各単位の全ページ byte が
`PageResourceCache` に揃っている場合だけ `<img>` と object URL を作り、単位内を全て decode できてから
再利用可能にする。byte が無い単位の取得完了は待たない。デコード開始は現在表示の DOM commit 後とし、
ページ表示を待たせない。表示要求が変わった時点で窓を更新し、窓外の進行中 decode は src を外して
object URL を revoke する。ready 要素も窓外では同様に解放するが、DOM 表示中の要素は display lease が
外れるまで URL 解放を遅らせる。

表示時に単位 identity とページごとの resource key が一致する ready 要素があれば、byte demand、fetch、
新しい `<img>` の作成、再 decode を行わず、その要素をそのまま DOM へ移す。通常表示でその場 decode した
要素も現在単位として decoded cache へ所有権を移す。decoded cache の単位上限と解放は byte cache の
枚数上限、LRU、先読み枚数設定とは別勘定であり、§14.6 の規則は変更しない。

`image` telemetry と表示単位ごとの `page_decode_ahead_display` には、再利用の成否、固定理由、
`tap_to_display_ms`、保持中の decoded 単位数を残す。この増分の効果は仕様では保証されない。画面外の
`<img>` の decode 結果をブラウザが維持するかを実機値で判断し、再利用できない、または再利用しても
待ち時間が下がらない場合は、次の独立した選択肢として canvas 描画と `createImageBitmap` を検討する。

### 14.20 診断ログから Tailscale アカウント値を除外 (2026-08-12)

常時出力される診断ログは、`Tailscale-User-Login`、`Tailscale-User-Name`、
`Tailscale-User-Profile-Pic` を含む `Tailscale-User-*` ヘッダの値を一切持たない。
`details.proxy.tailscale_user_header_names` には、Tailscale が呼び出し元を識別したかを診断できるよう
ヘッダ名だけをソート済み配列で残す。これはクライアント telemetry のファイルパスを伏せる契約と
同じく、診断に不要な個人識別値を永続ログへ入れないための不変条件である。

`x_forwarded_for` は値を維持する。tailnet 内の `100.x.x.x` アドレスは氏名、メールアドレス、
プロフィール画像とは性質が異なり、どの利用者端末から要求されたかを切り分けるために必要である。
`remote_addr`、HTTPS 判定とその根拠も従来どおりとし、認証でのヘッダ利用可否には影響させない。
### 14.21 リモート通常ページ生成の段別計測 (2026-08-12)

高画質 8192、46.6 MP の 1 ページでは、remote-web から見た本体処理 `ipc_ms` の中央値が
2398 ms、うち PDF worker の `pool_send` → `pool_recv` (`rtt_ms`) が 881 ms、
`pool_dispatch.wait_ms` は中央値 0.1 ms だった。差分の本体内後処理は約 1517 ms (63%) である。
先読み同時数を 2 → 3 に増やすと 1 本あたり 2016 → 2952 ms へ伸びた一方、供給は
0.99 → 1.02 ページ/秒 (+2%) に留まった。

`remote_page.stage` を resolve / source / compose / trim / resize / jpeg / total に分け、各段へ
画素・byte、開始時の `active_others`、明示 mutex の `wait_ms` を記録する。Auto 見開き相手の
raw 取得は現在ページの source 比較を崩さないため trim に分離する。`analyze_perf.py remote-page` で
段別 p50 / p90 と ms/MP、同時本数別所要時間、lock wait を集計する。これにより、lock 待ちが
増える直列化と、待ちはほぼ 0 だが実処理が伸びる CPU / メモリ帯域飽和を区別してから速度変更を決める。
この増分は計測だけで、pool、並列上限、画質、先読み、表示 ownership は変更しない。

2026-08-13 に PDF worker 内訳計測を追加した。`pdf.pool_recv` は従来の `rtt_ms` に加え、
PDFium bitmap までの `worker_render_ms`、RGBA / response 組立の `worker_serialize_ms`、
stdout 完了までの `worker_write_ms`、親側の `parent_read_ms`、両側の byte / call 数を持つ。
write/read は同一 pipe 区間なので、内訳の critical path は
`render + serialize + max(write, read)` とし、`timing_consistent` で `rtt_ms` 以下を検査する。
自動テストでは既存 render response の header / RGBA が変わらず、計測 frame の encode/decode、
framing byte 数、call 数、error 応答で追加 frame を待たないことを確認した。実 PDF の dominance
（render 対 transfer）は、隔離した debug worker で 4096 px の同一ページを 3 回計測した。
45.25 MiB 応答の warm 2 回は render 30.7–30.9 ms、serialize 24.9–27.0 ms、
write 24.2–28.3 ms、worker RTT 87.0–87.4 ms だった。cold 1 回目は render 86.4 ms、
serialize 23.9 ms、write 15.6 ms、RTT 137.9 ms である。warm 時は転送だけが単独で支配しておらず、
render、RGBA 組立、pipe 転送が同程度だった。ただし debug profile / 4096 px の確認値なので、
8192 px の通常利用に対する高速化判断はアプリの性能ログで再計測してから行い、この増分では高速化しない。

Rust 1.94.1 の標準ライブラリ実装も確認した。`stdout().lock()` の実体は容量 1024 byte の
`LineWriter<StdoutRaw>` で、`write_all` は入力内の最後の改行までをまとめて inner writer へ渡し、
残りだけを buffer へ置く。RGBA 中の改行ごとに OS write する実装ではない。今回の
`worker_write_calls` は既存 `write_msg` がその LineWriter 公開境界へ行った呼出回数、
`parent_pipe_read_calls` は `BufReader` 内側の実 `ChildStdout.read` 回数を記録する。上記の実測でも、
画像 frame は `write` 2 回（4-byte length と payload）+ `flush` 1 回で、改行単位の細切れはなかった。
