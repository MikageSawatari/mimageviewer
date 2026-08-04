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

`--remote-ipc` 起動時だけ本体に単一の remote session owner を置く。ブラウザは認証後に
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

### 2.3 既存資産の再利用状況

| 対象 | 状況 |
|---|---|
| サムネイル | remote-web は `favorite_id + relative_path + target_px` を本体へ IPC 中継する。本体が catalog 参照、既存生成経路、`CachePolicy` に従う保存を担当する (§9) |
| お気に入り | `settings.rs` の `FavoriteEntry` (安定 UUID + root path)。**そのまま公開 allowlist として使う** |
| 補正・回転・トリム・モザイク・消しゴム・ローカル調整・コミック注釈 | `books::BookPageSource::Composited` + `BakedEditSnapshot` に**ヘッドレス合成が既にある**。入力も File / ZipEntry / PdfPage をカバー済み |
| AI アップスケール・カラー化 | `page_requires_full_composite` から display-only として**意図的に除外**されている。ヘッドレス経路への追加は**新規作業** |
| ZIP / PDF 列挙 | `zip_loader` / `pdf_loader` をそのまま利用 |
| 検索 | Tantivy / `fts_meta.db`。`favorite_id` 単位の絞り込みが既にある |

## 3. 設計制約 (全フェーズ共通)

### 3.1 パス表現 — 絶対パスを URL に出さない

クライアントとのやり取りは必ず **`favorite_id` (UUID) + お気に入り root からの相対パス**
で行う。絶対パスを受け付ける API を作らない。

- 相対パスは正規化し、`..` を含む・ドライブ指定を含む・正規化後に root の外へ出るものは拒否
- シンボリックリンク / junction による脱出も、正規化後の実パスが root 配下かで判定する
- **お気に入りに登録されていない場所は、いかなる方法でも読めないこと**

### 3.2 認証

- ブラウザ認証にはユーザー設定の **6 文字以上の PIN / パスフレーズ**を使う。
  `--set-pin <PIN>` で設定・更新し、Argon2id の salt 付き hash とランダムなセッション署名鍵だけを
  認証ファイルへ永続化する。平文 PIN は保持しない
- 認証ファイルの既定はカレントディレクトリの `remote-web-auth.json`。`--auth-file` で変更できるが、
  `%APPDATA%\mimageviewer` および `--data-dir` 配下は拒否する。PIN 未設定時は fail-closed で起動を拒否する
- PIN 検証は Argon2id の定数時間検証を使う。失敗回数はプロキシ経由で送信元を識別できない場合も
  効くようサーバ全体で数え、5 回失敗で 30 秒、以後の失敗は解除後に 60 秒、120 秒……と
  指数バックオフする。失敗時刻・接続元・累積失敗回数を診断ログへ記録する
- 成功時は HMAC-SHA256 署名付き HttpOnly / SameSite=Lax Cookie を発行する。通常は Max-Age 90 日、
  「この端末を記憶しない」選択時は Max-Age のないセッション Cookie とする。`Secure` は direct TLS
  または `X-Forwarded-Proto: https` を検出したリクエストでだけ付ける
- curl 等の診断用には起動時生成の 256bit `Authorization: Bearer <token>` も残す。Bearer は
  定数時間比較し、PIN・hash・セッション署名値・Bearer をログへ出さない。認証失敗本文に内部情報を出さない
- 接続用 QR コードには URL だけを含め、PIN や Bearer は含めない。URL は `--url`、Tailscale の
  `--json` 状態、bind 先の順で決める。remote-web は確定 URL、`tailscale serve` 状態、PIN 設定済み
  bool だけを protocol v8 の接続情報として本体へ通知する。本体は独自検出せず、ヘルプの
  「リモート接続…」に QR、URL、接続状態を表示する。`--remote-ipc` 無しでは無効理由を表示する。
  remote-web はブラウザ要求と独立した常駐 worker で IPC を維持し、250 ms から 5 秒上限の指数
  backoff で再接続する。接続 / 再接続の handshake 完了時に接続情報を必ず再通知する。本体 UI は
  handshake 済み接続を URL 受信前から追跡し、「remote-web が起動していない」と
  「remote-web は起動済みだが接続情報を受信中」を区別する

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
- read-only 不変条件は remote-web が mIV の settings / catalog 等を変更しないことを指す。PoC の
  診断ログと認証ファイルだけは `--log` / `--auth-file` で指定した
  `%APPDATA%\mimageviewer` および `--data-dir` 配下ではない別パスへ出力する

## 4. PoC のスコープ (現在のフェーズ)

**目的は「実回線で実用になるか」を最短で確かめること。** 機能の網羅は目的ではない。

### 4.1 成果物

`crates/remote-web` (bin 名 `mimageviewer-remote`) を新規追加する。本体 lib
(`mimageviewer`) に依存してよい。

#### API (すべて認証必須)

| エンドポイント | 内容 |
|---|---|
| `GET /api/favorites` | お気に入り一覧 (id, 表示名) を JSON で返す |
| `GET /api/list?fav=<uuid>&path=<rel>` | 本体の通常フォルダ一覧を IPC で取得する。従来の種別・表示名・相対パス・サイズ・mtime に加え、セルの `address` と吸収済み sidecar を含む `thumbnail_address` を返す |
| `GET /api/thumb?fav=<uuid>&path=<rel>&w=<px>` | 画像・フォルダのサムネイルを本体へ IPC 中継し WebP で返す。本体未接続時は 503 と利用者向け理由を返す |
| `GET /api/image-info?fav=<uuid>&path=<rel>` | EXIF 回転反映後の元画像寸法を返す。クライアントの実描画幅計算に使う |
| `GET /api/image?fav=<uuid>&path=<rel>&w=<px>` | 画像を `w` に合わせて縮小し WebP で返す。リサイズ不要・EXIF identity・ブラウザ対応形式なら元バイトを素通しする |
| `GET /` および静的ファイル | フロントエンドを配信 |

- サムネイルの catalog 参照・キー生成・生成・保存判定は本体の既存経路に集約する。remote-web は
  catalog の内部構造を知らず、専用サムネイル DB も持たない。この段階で扱う source は通常画像と
  フォルダ代表である。ZIP / PDF の container page は後続増分で対応済みであり、通常フォルダの動画は
  本体で同名画像を sidecar として吸収した場合、その画像 address を `/api/thumb` へ渡す
- 本体側でも favorite allowlist と canonical path の包含を検証し、remote-web の検証結果を信頼しない
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
  したがって mIV から通常権限の子プロセスとして設定できる。UAC 昇格は不要
- `tailscale serve status --json` / `tailscale status --json` も **非管理者で読める**。
  `Self.DNSName` と serve の `Web` キーから接続 URL を自動組み立てできることを実測で確認済み。
  製品版の設定ウィザードはこの経路で「serve が設定済みか」まで自動判定できる
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
| touch / pen | 左右 34% の即時タップ、中央タップ、中央ダブルタップ、上下左右スワイプ、ピンチ、拡大中パン | 前 / 次、上下バー切替、全体表示 ⇔ 原寸、一覧 / メニュー / 前 / 次、ズーム、パン |
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
  認証しており fail-closed。全応答に `X-Content-Type-Options` / `Referrer-Policy` が付く
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
本体は `--remote-ipc` がある場合だけローカル named pipe を開き、受信と生成を UI thread 外の
上限制御された worker で処理する。同名 pipe が既に存在する場合は二重起動せずエラーを記録する。

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

`ThumbnailRequest { address, target_px }` の `address` は §12 の共通アドレス型であり、本体側で
再度 allowlist と canonical
path containment を検証する。通常画像とフォルダ代表は `thumb_loader::process_load_request`
(`load_one_cached`) へ渡し、catalog 参照、DCT / WIC / Susie、回転 DB、利用者のサイズ・画質、
`CacheDecision::from_settings` による保存判断を既存経路へ揃える。同一要求の同時到着は flight を
共有して重複生成しない。remote-web の `/api/thumb` はこの要求・応答を中継するだけで、IPC 往復
時間を `ipc_ms` として診断ログへ記録する。

本体未起動・pipe 未接続時も HTTP サーバ自体は起動する。`/api/thumb` は 503 と機械可読な
`miv_not_running` を返し、フロントは「mIV 本体が起動していません」と画面内に表示する。
仮想グリッドの tile は binding 世代と相対 path を応答時に照合し、破棄時は fetch を abort する。
これにより scroll で detach / 再表示された tile に古い in-flight 応答を適用しない。
ブラウザからのサムネイル HTTP 要求は同時 4 件に制限し、ネットワーク失敗・502・一時的な 503
だけを 200 / 400 / 800 ms の指数 backoff で最大 3 回再試行する。404 / 422 と protocol 版不一致は
再試行しない。上限到達時は tile に「再試行上限」を表示する。

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
本体の `show_location_*` 設定で表示対象になっている場合は「読書履歴」を先頭に置く。これにより
起動後 1 タップで「この前の続きを読む」一覧へ到達できる。ブックマークには本体側にも専用の
非表示設定がないため常に表示する。場所タブには読書履歴、レーティング ★1〜5、本棚、
ブックマークだけを載せ、ドライブ、デスクトップ、ピクチャ、ダウンロードは載せない。

スマートフォルダの定義一覧と、読書履歴・レーティング・本棚・ブックマーク・スマートフォルダの
評価結果は本体 IPC から取得する。remote-web は DB の集約条件や並び順を再実装しない。
本体側は次の既存 read model / evaluator を利用する。

- 読書履歴: `ReadingHistoryDb::list_recent`
- レーティング: `RatingDb::list_by_stars` → `rating_view::rating_row_to_view_row` →
  `rating_view::sort_rows(RatedAtDesc)`
- ブックマーク: `bookmark_browser::build_rows_readonly` で既存の行構築と
  `BookmarkViewSort::CreatedAtDesc` を共有する。内部 DB だけ read-only open に切り替える
- スマートフォルダ: `app::smart_folder` の候補走査、metadata filter、表示順計算をそのまま使う
- 本棚: `Settings::books_root_path` に対して `books::list_books` を使う

`/api/home` は保存済みスマートフォルダの ID / 名前と `show_location_*` の表示対象だけを
返し、ファイルシステム走査や各集約ビューの内容取得は行わない。`scan_smart_folder` は利用者が
該当スマートフォルダを開いて `/api/collection` を要求した時だけ実行する。読書履歴は本体設定
由来の上限 (最大 1000) を既に持つ。他のレーティング・本棚・ブックマーク・スマートフォルダも
IPC 応答をお気に入り境界で絞り込んだ後に最大 1000 件へ制限し、`truncated` /
`entry_limit` を返して Web 画面に打ち切りを表示する。現段階ではページングは実装しない。

IPC の `RemoteEntry` は `favorite_id`、お気に入り root からの `relative_path`、表示名、種別、
進捗・レーティング等の表示用メタデータだけを持つ。候補の絶対 path は本体内で canonicalize し、
最も深く一致するお気に入り root へ写像する。どのお気に入りにも属さない項目、欠落項目、
junction / symlink で root 外へ出る項目は IPC 応答を作る前に除外する。remote-web も受信後に
UUID、相対 path、canonical containment を再検証する。したがって集約 DB にお気に入り外の履歴や
レーティングが含まれていてもブラウザへは出ない。

HTTP は認証必須の `GET /api/home` と `GET /api/collection` を追加する。集約一覧の通常画像・
フォルダは既存 `/api/thumb?fav=<UUID>&path=<relative>&w=<px>` を使う。ZIP / PDF は §12 の増分で
コンテナ内ページまで閲覧可能になった。動画・音声・変換アーカイブの再生、および
読書履歴・レーティング・ブックマーク等への書き込みは後続のセッションロック設計と一緒に行う。

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
  -ArgumentList '--data-dir', '.\target\dev-runtime\data', '--remote-ipc'
```

remote-webにも同じdata directoryを渡し、同じお気に入り・設定を読む。

```powershell
Start-Process -FilePath .\target\remote-home-release\release\mimageviewer-remote.exe `
  -ArgumentList '--data-dir', '.\target\dev-runtime\data'
```

remote thumbnail IPC の `\\.\pipe\mimageviewer-remote-thumbnail` はremote-web側の接続先との互換性を
保つため固定のままとする。2つの本体へ同時に `--remote-ipc` を付けると、後発側は
`FILE_FLAG_FIRST_PIPE_INSTANCE` による作成失敗を「同名サーバが既に存在する可能性」と stderr と
各data directoryの `logs/mimageviewer.log` に記録し、GUI本体自体は起動を継続する。通常運用側には
`--remote-ipc` を付けず、検証側だけをremote-webの接続先にする。

## 12. ZIP / PDF のリモート閲覧 (2026-07-31)

### 12.1 共通アドレスと境界検証

`crates/remote-ipc` の `RemoteAddress` を本体と remote-web の唯一のアドレス表現とする。
実ファイルは常に `favorite_id` と favorite root からの `relative_path` で表し、
`RemoteSubresource` に `File`、`ZipDirectory { prefix }`、
`ZipEntry { entry_name }`、`PdfPage { page_number }` を持つ。ZIP entry / prefix は
`/` 区切りの相対表現だけを許し、先頭 slash、drive 指定、backslash、NUL、
`..` component を両プロセスの共通検証で拒否する。PDF page は 0-origin とし、本体が実際に
列挙した page count 未満であることを確認してからレンダリングする。

remote-web は IPC 前に favorite UUID、実コンテナの canonical containment、内部アドレス構文を
検証する。本体も同じ構文検証の後に `remote_ipc::path_guard::resolve_existing` を通す。
したがって remote-web の検証を迂回しても、favorite 外のコンテナ、junction / symlink 脱出、
悪性 ZIP entry、範囲外 PDF page は本体境界で再度拒否される。絶対 path は IPC 応答にも
HTTP / hash route にも含めない。

動画ストリーミングの実ファイルも同じ `RemoteAddress::File` と二重検証を通す。remote-web と
本体はそれぞれ favorite allowlist、canonical containment、実ファイル種別を検証し、本体 IPC
境界では remote-web の判定を信頼しない。

通常フォルダ一覧も要求 address を両側で検証する。本体が返した各セルの `address` と
`thumbnail_address` は remote-web が favorite allowlist と canonical containment を再検証し、
sidecar だけが root 外を指す応答も除外する。同一 address の場合は同じ検証結果を共有する。

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
へ拡張する。コンテナは最大 1000 項目を返し、超過時は `truncated=true` と `entry_limit` を
画面に表示する。ページングはこの増分では行わない。

フロントは ZIP/PDF を通常フォルダと同じ仮想グリッドで表示し、ページを既存の swipe、
pinch zoom、表示モード、keyboard / mouse command layer へ渡す。hash route は
`RemoteAddress` の JSON を percent encode した相対情報だけを保持する。パンくずは favorite、
実フォルダ、コンテナ、ZIP 内 prefix を 1 DOM 上で組み立て、親の実フォルダへ戻れる。

HTTP heavy admission は従来どおり最大 4、IPC 応答期限は 10 秒とする。本体の既存実測は
PDF cold render 1.441 秒、通常の page render 0.7〜3 秒であり、remote heavy worker 2 本へ
同時要求 4 件を制限した条件では 10 秒以内に収まる見積もりである。remote は Critical 予約枠を
使わないため、ローカル UI の現在ページを優先できる。診断 JSONL には `container` / `page`
別に `ipc_ms`、`ipc_status`、retry 回数、entry 数、target / output 寸法、応答 byte 数を残す。

### 12.4 明示的な非スコープ

RAR / 7z / LZH の変換、リモートからの PDF password 入力、コンテナの 1000 件超のページング、
読書履歴・rating・bookmark 等の書き込みは含めない。nested ZIP は本体の列挙文字列と
`read_entry_bytes` をそのまま使うため対応するが、nested RAR / 7z / LZH は変換増分まで扱わない。

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
先読み通信は同時 1 件、圧縮 Blob の LRU は最大 12 件かつ 32 MiB のままとする。viewer を離れた時、
別 target の foreground を要求した時、または計画から外れた時は不要な fetch を中断する。

現行 protocol の `PageRequest.priority` は `Foreground` / `Prefetch` を共有型で表す。remote-web は
prefetch admission を1件に制限し、all / heavy の最終1枠を使用させない。本体も全接続合計の
prefetch queued + active を1件に制限し、remote heavy worker が1本しかない設定では prefetch を
`Busy` で拒否する。2 worker 時も heavy queue / active が空の時だけ先読みを開始し、最大1本なので
もう1本を foreground に残す。既存 heavy 処理があれば先読みは待たせず `Busy` にする。PDF pool
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
横長判定は本体 catalog の既存 `source_dims`（無い場合は既存 thumbnail の寸法）を read-only で
参照し、寸法未確定時は fullscreen と同じく非横長として扱う。応答の `page_groups` はグループ列を
読み進める順に並べ、各 `pages` は画面上の左→右順、`anchor` はそのグループの読み順先頭とする。
remote-web は受信した各 address を favorite allowlist で再検証し、組み直しや独自 sort をしない。

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
渡り、本体側でも favorite allowlist、canonical containment、コンテナ種別を再検証する。worker は
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
`{"version":1,"portraitSinglePage":boolean,"gestureHelpDismissed":boolean}` として保存する。
既定は `portraitSinglePage=true`。☰ の「端末の設定」で OFF にすると縦持ちでも保存済み見開きを
維持する。parse / normalize / serialize は純粋関数とし、未知 version、壊れた JSON、型不正は
安全な既定値へ戻す。localStorage の取得・保存例外は wrapper で捕捉し、そのタブ内のメモリ値で
動作を継続する。項目追加時は version 方針を決め、`defaultLocalSettings`、
`normalizeLocalSettings`、設定画面、round-trip / 不正値テストを同じ aggregate objectへ追加する。

### 12.9 閲覧中の操作 UI (2026-08-01)

上下バーの表示状態はファイルや localStorage ではなく、フロントのセッション状態
`viewerBarsVisible` が所有する。初期値は表示で、中央 32% のタップとメニューの
「上下バーを表示 / 隠す」が同じ `toggle_viewer_bars` コマンドを通る。ページ移動や一覧からの
別画像オープンではリセットせず、タブを閉じると既定へ戻る。自動非表示 timer は持たない。

下バーの range は通常フォルダと ZIP / PDF のどちらも `pageGroups` を入力にする。Single では
1目盛り1ページ、見開きでは1目盛り1グループ（1見開き）とし、ラベルはグループ数でなく
`state.images` 上の実ページ番号を `12-13 / 240` の形式で出す。LTR は物理左端を先頭グループ、
RTL は range の物理値と読み順 group index の対応を反転して、物理左端を最終グループにする。
range の `input` 中は thumb とラベルだけを更新し、`change` で確定した1回だけ
`changeImageTo` を呼ぶため、ドラッグ途中の画像 fetch / decode は発生しない。実ページが1枚だけなら
range だけを隠し、`1 / 1` の位置表示は維持する。位置、実ページラベル、LTR / RTL の物理値変換は
`command-core.mjs` の純粋関数でテストする。

上下左右 swipe は同じ純粋判定を使う。開始点から主軸が **52 CSS px を超え**、かつ直交軸の
**1.25倍を超えた**場合だけ成立する。左端32pxの browser edge gesture guard も従来どおり適用する。
上 swipe はメニュー、下 swipe は一覧、左右 swipe は綴じ方向に従う前後ページへ送る。
`scale > 1.01` では1本指移動を常に pan とし、swipe を発火しない。幅フィットの縦 drag は、
`scrollHeight > clientHeight` かつ指の移動方向へ `scrollTop` を実際に変えられる場合だけ scroll を
優先する。内容が収まる場合、上端から下へ引く場合、下端から上へ引く場合は pan 扱いにせず、
縦 swipe の一覧 / メニュー操作へ渡す。drag 中に一度でも実スクロールした場合は同じ gesture の
残りを pan とする。一方、幅フィット中の明確な横 swipe は従来のページ送りを維持する。

静止画の**中央 32% だけ**のダブルタップは `Page` (見開きを含む全体が viewport に収まる倍率) と `Original`
(100%) を往復する。`Width` は縦スクロールを伴う幅合わせで「画面に合わせる」ではないため
往復対象にしないが、現在 `Width` の場合の最初のダブルタップは `Original` へ入る。ただし
`scale > 1.01` のピンチ拡大中は、基底の fit mode が `Page` / `Width` / `Original` のどれでも
最初のダブルタップを必ず `Page` とする。transform reset 後に基底 `Page` だけを見て `Original` を
選ぶと、一瞬 Page 全体表示になってから 100% へ戻るためである。動画 viewer はこの判定を通らず、
従来どおり再生 / ±10秒 tap zone と native zoom 抑止だけを持つ。

左右 34% はダブルタップ候補を一切作らず、各 `pointerup` でページ送りを即時実行する。同じ端を
素早く2回叩けば従来どおり2ページ進む。中央だけは最初の tap を最大 320 ms 保留し、36 CSS px
以内の2打目ならバー切替を発火せず `fit_toggle_page_original`、2打目が無ければバー切替とする。
したがって失うのは中央を素早く2回叩いたときのバー切替だけで、ページ送りには遅延を加えない。
動画 viewer はこの sequence 判定そのものを通らず、拡大操作も追加しない。

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
`ContainerEngine` は favorite 境界内の実ディレクトリを走査した後、ローカル
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
`source_dims` による横長単独、RTL の画面左右順が ZIP / PDF と同じになる。純粋関数テストは
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

Service Worker は登録しない。コンテンツの正本は母艦 PC にあり offline shell だけを残す意味がなく、
更新後の JS / CSS を古い cache が保持する故障モードを増やすためである。

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
含め、全 address は従来の `Library::validate_remote_address` を通して favorite root 内の実在パスに
限定する。入口の回帰テストは通常画像 / ZIP entry / PDF page の許可と、動画 / 音声 / テキスト /
traversal の拒否を同じ guard に対して固定する。

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
address は .mp4 のまま、thumbnail_address は吸収した画像になる。Web グリッドはセルの
open には前者、/api/thumb には後者を使い、sidecar 画像を独立 tile として表示しない。

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

## 13. 作業運用メモ (セッションをまたぐ引き継ぎ用)

この節は設計ではなく**開発手順**の記録。会話ログにしか残らない知識を失わないために書く。

### 13.1 検証環境

実利用中の `%APPDATA%\mimageviewer` を汚さないため、**隔離データディレクトリ**で検証する。

```powershell
# 本体 (ユーザーが実行する。エージェントは mIV 本体を起動しない)
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe `
  -ArgumentList '--data-dir','.\target\dev-runtime\data','--remote-ipc'
```

- データディレクトリが既定と異なると、mutex / activate event / open-path pipe /
  shutdown event の 4 つが別名前空間になるため、**通常版 mIV を起動したままでよい** (§11)
- `--remote-ipc` は**検証側だけ**に付ける (IPC の pipe 名は固定のため、両方に付けると
  後発側が拒否される)
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
- `crates/remote-web/web/` はディスクから直接配信される。**フロントだけの変更なら
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

### 13.5 プロトコル版数

`crates/remote-ipc` の protocol version を上げた増分では、**本体と remote-web の両方を
再ビルドして再起動する**必要がある。片方だけだとハンドシェイクで弾かれる。
通常フォルダ一覧 IPC の v16 に、動画 start/resource の stage 別 timeout code を追加した
2026-08-02 時点の現行版は **v17**。

### 13.6 残タスク (2026-08-01 時点)

1. **動画・音声ストリーミングのフロント (増分 7)** — server / IPC は増分 6 で完了。
   正本は [web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md)
2. **検索** (Ctrl+S / F / G 相当)、タグ
3. 配布 (exe 埋め込み、接続診断ウィザード)

### 13.7 未消化の宿題

- `cargo test -p mimageviewer-launcher` が未実行。launcher の `build.rs` が
  `target/release/mimageviewer-core.exe` を要求するため、release core をビルドしてから流す。
  §11 の single-instance 名前空間の変更が配布経路を壊していないかの最終確認になる
