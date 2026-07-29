# リモート閲覧 (Web) 機能 計画書

v3.0.0 の目玉として、外出先のスマートフォン / タブレット / PC のブラウザから
自宅 PC の mIV ライブラリを閲覧できるようにする。本書がこの機能の正本。

- ブランチ: `web-remote` (worktree: `C:\home\mimageviewer-web`)
- 実装: Codex Sol (xhigh) / レビュー・統合: ClaudeCode / 実機検証: ユーザー
- 現在のフェーズ: **PoC (§4)**

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
                              └─ read-only で直接読む: 実ファイル / catalog DB / settings DB
```

### 2.1 読み書きの分離 (重要な不変条件)

| 種別 | 担当 | 理由 |
|---|---|---|
| 読み取り (サムネ・画像・動画バイト・一覧) | remote-web が **直接** read-only で読む | サムネ一覧は 1 画面で数百リクエスト。IPC 中継すると本体のフレーム予算を食う |
| 書き込み (読書履歴・ブックマーク・見開き・トリム・タグ・レーティング) | **必ず IPC → 本体** | 全永続ストアの writer を本体 1 つに固定する |
| 重い生成 (PDF レンダ・AI アップスケール・カラー化・補正合成) | **IPC → 本体** | PDFium プール・ONNX セッション・GPU をステートフルに保持しているのは本体 |

SQLite は WAL なので「本体が唯一の writer / remote-web は reader」は安全に成立する。
**この境界を崩す変更を入れないこと。**

PoC の remote-web 専用サムネイルキャッシュはこの表の mIV 永続ストアには含めない診断用成果物で、
外部パスにだけ書く。settings / catalog の単一 writer 境界は変えない。

### 2.2 セッションと排他

リモート接続中、本体は「外部から閲覧中 [切断する]」ダイアログを出して**ローカル操作を
ロックする**。同時に 1 つの操作者しか存在しない状態を作り、多重書き込みの整合性問題を
構造的に消す。

- アイドルタイムアウトで自動解放する (外出先でタブを閉じ忘れて自宅 PC が固まる事故を防ぐ)
- ローカル側からは常に強制奪還できる
- 2 台目の接続は「乗っ取りますか？」を確認する
- セッション中は `ES_SYSTEM_REQUIRED` でスリープを抑止する

### 2.3 既存資産の再利用状況

| 対象 | 状況 |
|---|---|
| サムネイル | catalog に存在する WebP (`folderthumb:` を含む) は無変換で返す。ただし個別画像行は通常ほとんど永続化されていないため、欠落時は remote-web 専用キャッシュへオンデマンド生成する |
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
  `--json` 状態、bind 先の順で決める

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
- read-only 不変条件は mIV の settings / catalog 等を変更しないことを指す。PoC の診断ログ、認証
  ファイル、remote-web 専用サムネイルキャッシュだけは `--log` / `--auth-file` / `--thumb-cache`
  で指定した `%APPDATA%\mimageviewer` および `--data-dir` 配下ではない別パスへ出力する

## 4. PoC のスコープ (現在のフェーズ)

**目的は「実回線で実用になるか」を最短で確かめること。** 機能の網羅は目的ではない。

### 4.1 成果物

`crates/remote-web` (bin 名 `mimageviewer-remote`) を新規追加する。本体 lib
(`mimageviewer`) に依存してよい。

#### API (すべて認証必須)

| エンドポイント | 内容 |
|---|---|
| `GET /api/favorites` | お気に入り一覧 (id, 表示名) を JSON で返す |
| `GET /api/list?fav=<uuid>&path=<rel>` | 指定フォルダの直下と、そのフォルダでの実効サムネイル高さ比を JSON で返す。各要素は種別 (dir / image / video / audio / zip / pdf / other)・表示名・相対パス・サイズ・mtime |
| `GET /api/thumb?fav=<uuid>&path=<rel>` | 画像・フォルダのサムネイルを返す。catalog WebP → remote-web 専用 SQLite → オンデマンド生成の順に参照する |
| `GET /api/image-info?fav=<uuid>&path=<rel>` | EXIF 回転反映後の元画像寸法を返す。クライアントの実描画幅計算に使う |
| `GET /api/image?fav=<uuid>&path=<rel>&w=<px>` | 画像を `w` に合わせて縮小し WebP で返す。リサイズ不要・EXIF identity・ブラウザ対応形式なら元バイトを素通しする |
| `GET /` および静的ファイル | フロントエンドを配信 |

- catalog DB は `catalog::db_path_for` でフォルダごとに解決し、**`mode=ro` で開く**
- catalog の画像キーと `folderthumb:auto-v2:numeric:d3:<name>` (pin 派生を含む) は無変換で
  再利用する。catalog miss 時だけ remote-web 専用 SQLite へ生成し、mIV catalog には書かない。
  専用キーは source path + mtime(ns) + file size + 生成サイズを SHA-256 化する
- サムネイル生成は HTTP ワーカー間で並列実行するが最大 4 本に制限し、同じキーの生成中は
  共有 flight + Condvar で待ち合わせて重複デコード / エンコードを防ぐ
- サムネイルのキーは `grid_item.rs` の既存規約 (`image_full_path_cache_key` /
  `zipdir_cache_key` / `pdf_page_cache_key` 等) に従う。PoC で扱うのは通常の画像ファイルのみ
- 一覧の走査は `entry.file_type()` を使う (`Path::is_dir()` を per-entry で呼ばない)

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
2. **仕上げ** — 動画 (直接再生 + fMP4 remux + 非対応時の音声フォールバック)、音声再生と
   音量ノーマライズ (WebAudio GainNode。`volume` 属性だけでは減衰しかできない)、検索
   (Ctrl+S / F / G 相当)、タグ・レーティング、AI アップスケール・カラー化のヘッドレス化、
   接続診断ウィザード、exe 埋め込みと配布

**動画のトランスコードは実装しない。** コンテナ非対応 (MKV 等) は remux で救い、
コーデック非対応 (HEVC / AV1 / WMV) は音声フォールバックに逃がす。

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
`toggle_menu` / `back` / `parent_folder` / `open` 等の共通コマンドへ変換する。コマンド実行時の
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
| touch / pen | 左右 34% のタップ、中央タップ、左右スワイプ、ピンチ、拡大中パン | 前 / 次、メニュー、前 / 次、ズーム、パン |
| mouse | 左右クリックゾーン、中央クリック、右クリック、通常ホイール、Ctrl/Cmd+ホイール | 前 / 次、メニュー、メニュー、前 / 次、ズーム |
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

- **F1 (構造・中)**: 本体 lib に依存せず、catalog のキー規約を remote-web 内に複製している。
  理由 (FFmpeg の import library リンクにより Windows ローダが exe ロード時に DLL を要求し、
  単体サーバにも 6 個の DLL 同居が必要になる) は妥当だが、**master 側で規約が変わっても
  気付けない**。`src/path_key.rs` (約 40 行) と cache key helper を依存ゼロの小クレートに
  切り出し、本体と remote-web の双方が参照する形にする。**次フェーズの冒頭で実施する**
- **F2 (解消済み)**: PIN 認証への変更時に、通常のセッション Cookie を Max-Age 90 日とした。
  端末に残したくない場合は画面からセッション Cookie を選べる
- **F3 (小)**: catalog の blob が WebP でない行 (旧形式で JPEG が入っている可能性) は 404 に
  なる。実機でサムネイル欠けが多発したらこれを疑う。content-type の出し分けで救える
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

このため §4.2 からオンデマンド生成を外し、参照順を catalog → remote-web 専用 SQLite → 生成に
変更した。専用 DB の既定はカレントディレクトリの `remote-web-thumbs.db`、変更は
`--thumb-cache <path>`。mIV の settings / catalog は従来どおりすべて read-only である。

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
- fMP4 remux は維持する。スマートフォンのライブラリに MKV は普通に存在し、
  300〜600 行で「再生できる / できない」が変わるため費用対効果が高い
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

### 9.3 暫定措置 (撤去予定)

commit `865d9c2a` で `crates/remote-web/src/thumb_cache.rs` にオンデマンド生成と専用
SQLite を入れた。**これは IPC が入るまでの暫定であり、縦串フェーズで撤去する。**
撤去対象: `thumb_cache.rs` 全体、`--thumb-cache` オプション、`store.rs` の生成経路、
`image_support.rs` の生成用デコード。catalog 直読みの fast path も本体へ移す。

### 9.4 この発見の位置づけ

PoC の目的は「実回線で実用になるか」の確認だったが、**設計前提の誤りを実装が深くなる前に
発見できた**点が最大の収穫となった。縦串フェーズを IPC から始める根拠でもある。
