# リモート接続の前提条件 (HTTPS 証明書・キーの有効期限) を検出して案内する

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。`tailscale serve` 導線
(`628d8347`) の続き。設定導線の最後のピース。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§6.5.1**、**§3.2** (本体は独自検出しない)、
  **§14.15** (`tailscale serve` 導線の判断)
- `crates/remote-web/src/connection_url.rs` — `tailscale_status_url` /
  `inspect_tailscale_serve` / `detect_peer_info` (status JSON を既に読んでいる)
- `crates/remote-ipc/src/lib.rs` — `RemoteWebConnectionInfo` / `RemoteWebFeatureStatus`
- `src/remote_ipc/ui.rs` — 接続ダイアログの `tailscale serve` 節、`REMOTE_MANUAL_URL` と
  `ui.hyperlink_to` の使い方

## 1. 事実 (開発機で実測済み、再調査不要)

`tailscale status --json` の top-level と `Self` に次がある。

```
CertDomains:      ["desktop4090.taild260d0.ts.net"]   ← HTTPS 証明書が有効なら埋まる
Self.KeyExpiry:   null                                 ← 有効期限を無効化済みなら null
MagicDNSSuffix:   taild260d0.ts.net
```

| 事実 | 根拠 |
|---|---|
| HTTPS 証明書は **tailnet 全体の管理設定**。`tailscale set` にも `up` にも該当オプションが無い | `--help` を実測 |
| キーの有効期限も **管理コンソール (デバイス単位) または管理者トークン付き API** の設定。ローカル CLI では変えられない | 同上 |
| 証明書が無効だと `tailscale serve` は証明書を取得できず失敗する。現状は生のエラーが出るだけ | §14.15 の導線 |
| 期限が切れると端末が tailnet から外れ、**外出先からの接続が不能になる**。既定は約 180 日 | Tailscale の既定 |
| 開発機は 2 つとも整っているため、今回の実機確認では踏んでいない | 上の実測値 |

**したがって、この 2 つは mIV が代行しない。読んで案内するだけとする** (利用者判断、2026-08-12)。
外部ツールの方針 (参照系は無条件・変更系は説明してボタン) の延長で、**そもそも代行できない
設定は案内に留める**という 3 つ目の区分になる。この区分も plan に記録すること。

## 2. 変更内容

### 2.1 検出 (常駐プロセス側)

status JSON は既に読んでいるので、その中から 2 つを拾う。**判定は常駐プロセスに置く**
(plan §3.2 の「本体は独自検出せず」を維持)。

- **HTTPS 証明書**: top-level `CertDomains` が 1 件以上あれば有効。空 / 欠落なら無効。
  CLI が無い / 実行失敗 / JSON を読めない場合は不明。`RemoteWebFeatureStatus` の
  3 値をそのまま使う
- **キーの有効期限**: `Self.KeyExpiry` を **unix 秒 (i64)** へ変換して運ぶ。
  **null・欠落・解釈できない値は「情報なし」として `None`** とし、
  「無期限である」と断定しない (開発機では null しか観測できておらず、
  期限が入っている形は未確認のため)

### 2.2 protocol v45 → v46

`RemoteWebConnectionInfo` に 2 つ足す。

- HTTPS 証明書の状態 (`RemoteWebFeatureStatus`)
- キーの有効期限 (`Option<i64>`、unix 秒)

plan §13.5 を v46 へ更新する。

### 2.3 表示 (本体側)

**HTTPS 証明書が無効のとき**:

- `tailscale serve` の設定ボタンを**押せなくする**。証明書が無いと必ず失敗するため、
  押させて生のエラーを読ませる形にしない (PIN 未設定で有効化を止めているのと同じ扱い)
- 理由と、管理コンソールの DNS ページで有効にする旨を出し、リンクを置く
- 不明のときは従来どおり「読み取れません」。ボタンの扱いも従来どおり

**キーの有効期限が分かるとき**:

- 期限の日付と残り日数を出す (例: `接続キーの有効期限: 2026-02-08 (あと 179 日)`)
- **残り 30 日以内、または期限切れなら警告色**にし、管理コンソールのデバイス設定で
  無期限にできる旨とリンクを出す
- 期限切れのときは「この PC は tailnet から外れているため、外出先から接続できません」の
  趣旨を明示する。**外出先で初めて気付く壊れ方**なので、家にいるうちに気付かせるのが目的である
- 情報が無いとき (§2.1 の `None`) は**何も出さない**。「無期限です」とは書かない

**リンク**: 管理コンソールの DNS ページとデバイス一覧の URL は、`REMOTE_MANUAL_URL` と同じ
場所に定数として 1 か所へ置く。実装時に現在の URL を確認すること
(`https://login.tailscale.com/admin/dns` / `https://login.tailscale.com/admin/machines` の想定)。

### 2.4 純関数に切る

残り日数の計算と表示区分 (情報なし / 通常 / 警告 / 期限切れ) は**純関数**にし、
`now` を引数で受ける。UI から時刻を直接読まない。

## 3. 触らないもの

- PIN の所有と保存場所、`tailscale serve` の設定導線そのもの
- 表示パイプライン (段階 3a / 3b / 3c)
- 有効化の可否は PIN だけで決める。**証明書や有効期限でリモート接続の有効化そのものは
  止めない** (証明書が無くても `http://127.0.0.1` でのローカル確認は成立するため)
- `tailscale funnel` は使わない

## 4. テスト

```
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

- 検出: `CertDomains` が 1 件以上 / 空 / 欠落 / JSON 不正 の 4 通り。
  `Self.KeyExpiry` が RFC3339 文字列 / null / 欠落 / 解釈不能 の 4 通り
- 純関数: 情報なし / 残り 179 日 / 残り 30 日ちょうど / 残り 1 日 / 期限切れ の区分。
  境界値を含めること
- ダイアログ: 証明書が無効なら serve ボタンが押せない。不明なら従来どおり。
  期限情報が無いときに期限の行が出ない
- protocol: 新フィールド込みで v46 の round-trip

## 5. ドキュメント

- plan **§6.5.1** に、この 2 つが tailnet 側の管理設定であり mIV は代行しないことを書く
- plan に **§14.16** (または次の空き番号) を追加し、検出内容・案内に留める判断・
  「無期限と断定しない」理由 (期限が入っている形を実機で観測していない) を記録する
- plan **§13.5** を v46 へ更新する
- マニュアルはこの増分の後に書く (今回は書かない)

## 6. 実行と報告

- §4 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- **開発機の Tailscale 設定を変更しない**。参照系のコマンドだけを使うこと
- ブリーフと意図的に違えた点があれば、その理由を報告する
