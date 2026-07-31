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
                              └─ read-only で直接読む: 実ファイル / settings DB
```

### 2.1 読み書きの分離 (重要な不変条件)

| 種別 | 担当 | 理由 |
|---|---|---|
| 読み取り (画像・動画バイト・一覧) | remote-web が **直接** read-only で読む | HTTP の range / 一覧走査を本体 UI から分離する |
| サムネイル参照・生成 | **IPC → 本体** | catalog の実態が当初想定と異なったため。本体の既存生成経路とキャッシュ方針を一元利用する (§9) |
| 書き込み (読書履歴・ブックマーク・見開き・トリム・タグ・レーティング) | **必ず IPC → 本体** | 全永続ストアの writer を本体 1 つに固定する |
| 重い生成 (PDF レンダ・AI アップスケール・カラー化・補正合成) | **IPC → 本体** | PDFium プール・ONNX セッション・GPU をステートフルに保持しているのは本体 |

SQLite は WAL なので「本体が唯一の writer / remote-web は reader」は安全に成立する。
**この境界を崩す変更を入れないこと。**

remote-web 専用サムネイルキャッシュは §9 の縦串増分で撤去した。settings / catalog の writer は
引き続き本体だけであり、remote-web はこれらへ書き込まない。

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
| `GET /api/list?fav=<uuid>&path=<rel>` | 指定フォルダの直下と、そのフォルダでの実効サムネイル高さ比を JSON で返す。各要素は種別 (dir / image / video / audio / zip / pdf / other)・表示名・相対パス・サイズ・mtime |
| `GET /api/thumb?fav=<uuid>&path=<rel>&w=<px>` | 画像・フォルダのサムネイルを本体へ IPC 中継し WebP で返す。本体未接続時は 503 と利用者向け理由を返す |
| `GET /api/image-info?fav=<uuid>&path=<rel>` | EXIF 回転反映後の元画像寸法を返す。クライアントの実描画幅計算に使う |
| `GET /api/image?fav=<uuid>&path=<rel>&w=<px>` | 画像を `w` に合わせて縮小し WebP で返す。リサイズ不要・EXIF identity・ブラウザ対応形式なら元バイトを素通しする |
| `GET /` および静的ファイル | フロントエンドを配信 |

- サムネイルの catalog 参照・キー生成・生成・保存判定は本体の既存経路に集約する。remote-web は
  catalog の内部構造を知らず、専用サムネイル DB も持たない。今回扱う source は通常画像と
  フォルダ代表で、ZIP / PDF / 動画 / 音声は後続増分とする
- 本体側でも favorite allowlist と canonical path の包含を検証し、remote-web の検証結果を信頼しない
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
サムネイルと集約一覧の重い要求を 4 に制限する。したがって IPC がすべて待機中でも少なくとも
6 worker は `/api/favorites` / `/api/list` / 認証等へ使え、重い IPC が上限でも Home 用 IPC
2 枠が残る。上限超過は queue 待ちせず HTTP 503 + `Retry-After: 1` と
`ipc_status=admission_busy` を返す。ブラウザの thumbnail 再試行がこの応答を処理する。

IPC 応答期限は 10 秒とする。実測の最も遅い単発 RAW decode 1.7 秒に約 6 倍の余裕があり、
本体側の remote heavy worker 2 本に Web 側の重い要求 4 件が並んでも通常は期限内に収まる一方、
異常要求が HTTP worker を分単位で保持しない値である。timeout は 503 +
`Retry-After: 1`、`ipc_status=response_read_timeout` として記録し、同じ HTTP worker 内では
再試行しない。多重化接続では期限切れ request id を tombstone として保持し、遅着応答だけを
読み捨てるため、1 件の timeout が他の進行中要求や接続全体を巻き添えにしない。

本体 IPC は Home を専用 queue + 1 worker に分離した。サムネイルと集約評価は別の heavy queue で
処理し、利用者設定 worker 数の半分かつ最大 2 worker (`clamp(configured/2, 1, 2)`) に制限する。
remote IPC は UI thread を使わず、ローカル表示用 worker とも別である。CPU / disk の物理競合は
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
WebP で返す。`GET /api/thumb` は同じ address query (`entry` / `prefix` / `page` のいずれか)
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
