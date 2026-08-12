# リモート接続ダイアログから `tailscale serve` を設定する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。PIN 導線 (`733faa85` / `6d078969`)
の続き。設定導線で残っている最後の壁。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§6.5.1** (TLS を `tailscale serve` へ委譲する
  決定と、非管理者で実行できる実測)、**§3.2** (本体は独自検出しない)、**§3.2.1** (service lifecycle)
- `crates/remote-web/src/connection_url.rs` — `choose_connection_url` /
  `tailscale_executable` / `command_json` / `tailscale_serve_status` / `serve_hostname` /
  `ts_hostname` / `normalize_base_url`
- `crates/remote-ipc/src/lib.rs` — `RemoteWebConnectionInfo` / `RemoteWebFeatureStatus`
- `src/remote_ipc/service.rs` — owner worker (`RemoteServiceCommand`、PIN 設定と再起動の実装)
- `src/remote_ipc/ui.rs` — 接続ダイアログ (即時反映設計に変更済み)

## 1. 事実 (実機で確認済み、再調査不要)

`tailscale` 1.98.10 (開発機) の `serve status --json` は次の形である。

```json
{
  "TCP": { "443": { "HTTPS": true } },
  "Web": {
    "desktop4090.taild260d0.ts.net:443": {
      "Handlers": { "/": { "Proxy": "http://127.0.0.1:8787" } }
    }
  }
}
```

| 事実 | 場所 |
|---|---|
| 現在の検出は `Web` のキーに `.ts.net` があるかだけを見る。**どこへ proxy しているかを見ていない** | `connection_url.rs` `serve_hostname` |
| そのため**別の宛先を配信していても「設定済み」**になる。URL も `https://<host>/` を組み立てるだけ | 同上 |
| `tailscale serve --bg <port>` は非管理者で実行できる (2026-07-29 実測) | plan §6.5.1 |
| 待受ポートは remote-web 側の `DEFAULT_PORT = 8787` 固定で、本体は `--port` を渡していない | `config.rs` 5、`service.rs` `remote_command` |
| IPC の向きは remote-web → 本体だけである。本体から常駐プロセスへ要求を送る経路は無い | `ThumbnailClient` / `pipe.rs` |
| 1.98 の `serve` に、**対象を限定した解除の documented な形が無い**。`reset` は serve 設定を全部消す | `tailscale serve --help` |

## 2. 決定 (2026-08-12、利用者判断)

**外部ツールの操作方針**: **参照系は無条件で実行してよい。設定を変える系は、何をするかを
利用者に説明したうえでボタンを押させ、mIV が代行する。** この方針を plan に記録すること
(今後 tailscale 以外の外部ツールを触るときも同じ)。

- 参照 = `tailscale status --json` / `serve status --json`。従来どおり起動時に無条件で実行する
- 変更 = `tailscale serve --bg <port>`。ダイアログで**実行するコマンドとその意味**を示し、
  ボタンを押されたときだけ実行する

## 3. 変更内容

### 3.1 「設定済み」の判定を宛先まで見る

`tailscale_serve_status` を、**自分の待受ポートへ proxy している handler があるときだけ**
`Configured` とする形に直す。

- `Web` の各エントリの `Handlers` を見て、`Proxy` の host:port が自分の bind
  (`127.0.0.1:<port>`) と一致するものを探す
- 見つかったら、その**エントリのホストと handler のパス**から接続 URL を組み立てる
  (現在は常に `/` を仮定している)
- 見つからないが `.ts.net` のエントリがある場合は `NotConfigured` とし、
  **`/` を占めている既存の宛先**を本体へ伝える (次項)
- CLI が無い / 実行できない / JSON を読めない場合は従来どおり `Unknown`

### 3.2 衝突を本体へ伝える (protocol v44 → v45)

`serve --bg <port>` は `https://<host>:443/` に割り当てるので、**そこに別の宛先が入っていれば
上書きになる**。黙って上書きしないため、`RemoteWebConnectionInfo` に既存の宛先を運ぶ
フィールドを足す (例: `tailscale_serve_conflict: Option<String>`、値は `http://127.0.0.1:3000`
のような proxy 先の表示用文字列)。衝突が無ければ `None`。

protocol version を **45** へ上げ、plan §13.5 を更新する。

### 3.3 待受ポートの所有者を本体にする

serve へ渡すポートを本体が知る必要がある。PIN / ログのときと同じく、**本体が決めて渡す**形にする。

- 既定ポートの定数を共有クレート (`mimageviewer_ipc`) へ移し、本体は `--port` を明示して
  子プロセスを起動する
- remote-web 側は渡された値をそのまま使う (既定値は残してよいが、本体は必ず渡す)

### 3.4 tailscale CLI の起動を共有クレートへ

本体が変更コマンドを実行するので、**実行ファイルの探索と、timeout 付きの実行**を共有クレートへ
移し、本体と remote-web が同じ 1 つを使う。

- 移すのは `PREFERRED_TAILSCALE_EXE` を含む探索と、timeout 付き実行 (`TAILSCALE_COMMAND_TIMEOUT`)
- **JSON の解釈 (何が設定済みか) は remote-web に残す**。plan §3.2 の「本体は独自検出せず」を
  維持する。本体がやるのは「変更コマンドを実行する」ことだけである

### 3.5 本体の owner worker に設定コマンドを足す

CLI 実行は最大 8 秒かかるので **UI スレッドで走らせない**。PIN 設定と同じく
owner worker (`RemoteServiceCommand`) へ 1 コマンド足し、結果を `mpsc` で返す。

- worker は `tailscale serve --bg <port>` を実行する
- **成功したら所有している子プロセスを再起動する。** 本体から常駐プロセスへ要求を送る経路は
  無いため、再検出の手段は再起動である。再起動すると `choose_connection_url` が走り直し、
  新しい URL と `Configured` が本体へ再通知される。この「再起動が再検出の手段である」という
  判断を plan に残すこと
- 結果は typed に返す (成功 / tailscale が見つからない / 実行失敗 (stderr 付き))。
  失敗時の stderr はそのまま出してよい (秘密を含まない)

### 3.6 ダイアログ

`tailscale serve:` の行を、状態に応じて次のようにする。

| 状態 | 表示 |
|---|---|
| `Configured` | 「設定済み」。ボタンなし |
| `NotConfigured` (衝突なし) | 何をするかの説明 + **実行するコマンドそのもの** (`tailscale serve --bg 8787`) + 「tailscale serve を設定する」ボタン |
| `NotConfigured` (衝突あり) | 上に加えて「現在 `https://<host>/` は `<既存の宛先>` に割り当てられています。設定すると置き換わります」 |
| `Unknown` | 「Tailscale が見つからないか、状態を読み取れません」。ボタンなし |

- 説明文は「この PC の `<port>` 番を、tailnet 内から HTTPS で開けるようにします。
  TLS は Tailscale が処理します。インターネットには公開されません」の趣旨にする
  (Funnel ではないことが分かる文にする)
- 実行中は spinner を出し、ボタンを押せなくする。結果 (成功 / 失敗) をその場に出す。
  成功後は再起動を挟んで状態表示が「設定済み」へ変わる
- 押していないのに実行しない。押す前に必ず上の説明が見えていること

### 3.7 解除は今回のスコープ外

1.98 の `serve` には対象を限定した解除の documented な形が無く、`reset` は**利用者が別用途で
設定した serve 設定まで消す**。代行するなら「mIV の分だけを消す」ことを保証できる形
(`get-config` / `set-config` の往復) が要るが、この増分では確認できていない。

- ダイアログには**解除は Tailscale 側で行う**旨だけを出す (コマンドの代行はしない)
- 見送りの理由と、将来やるなら `get-config` / `set-config` で mIV の handler だけを外す形に
  なることを plan に記録する

## 4. 触らないもの

- PIN の所有と保存場所、認証ファイル、`--auth-file` / `--log` の受け渡し
- 表示パイプライン (段階 3a / 3b / 3c)
- `tailscale funnel` は**使わない** (plan §6.5.1)。今回も一切触れない
- bind は `127.0.0.1` のまま。LAN へ出さない

## 5. テスト

```
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

- 検出: §1 の実 JSON で `Configured` になる。proxy 先が別ポートなら `NotConfigured` +
  衝突あり。`.ts.net` エントリが無ければ `NotConfigured` + 衝突なし。handler のパスが `/` 以外の
  ときは URL にそのパスが入る。JSON が壊れていれば `Unknown`
- protocol: 新フィールド込みで v45 の round-trip
- 本体: 設定コマンドが owner worker 上で走り、成功時に子プロセスを再起動する。
  失敗時は再起動しない
- ダイアログ: 状態ごとに出る要素が変わること (ボタンの有無、衝突の警告)

## 6. ドキュメント

- plan **§6.5.1** に、設定を代行することと §2 の外部ツール方針を追記する
- plan に **§14.15** (または次の空き番号) を追加し、判定を宛先まで見る形へ直した理由、
  再起動が再検出の手段である理由、解除を今回やらない理由を記録する
- plan **§13.5** を v45 へ更新する
- マニュアルの「準備する」はこの増分の後に書く (今回は書かない)

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- **開発機では既に `tailscale serve` が 8787 に設定済み**である。テストでこの状態を変えないこと
  (実際に `tailscale serve` を実行しない。単体テストは JSON とコマンド組み立てまでで止める)
- ブリーフと意図的に違えた点があれば、その理由を報告する
