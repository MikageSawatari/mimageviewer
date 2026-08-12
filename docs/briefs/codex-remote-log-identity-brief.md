# 診断ログに Tailscale アカウントを残さない

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。`eaa7038d` の続き。

## 0. 前提 — 先に読むもの

- `crates/remote-web/src/http.rs` — `request_proxy_details` (4200 行付近)、`limit_log_value`
- `crates/remote-web/web/command-core.mjs` — `redactFilesystemPaths` (541 行付近)。
  **クライアント側の記録がすでに取っている方針**がここにある
- `src/remote_ipc/service.rs` — `RemoteServicePaths` (診断ログの置き場所)

## 1. 事実 (実機ログで確認済み。再調査不要)

診断ログ (`<data_dir>-remote\remote-web-log.jsonl`) は**常に書かれる**。オプトインは無い
(`service.rs` が `--log` を無条件に渡す)。

そのログの `details.proxy` に、Tailscale の識別ヘッダが**値のまま**入っている。

```
"tailscale_user_headers": {
  "Tailscale-User-Login": "mikage@sawatari.info",
  "Tailscale-User-Name": "=?utf-8?q?...?=",
  "Tailscale-User-Profile-Pic": "https://lh3.googleusercontent.com/a/...=s96-c"
}
```

`limit_log_value` は長さを切るだけで、値を伏せていない。

一方で**ファイルパスは伏せている**。1.3 MB のログをドライブレター (`X:\`) で検索して 0 件、
クライアント側の記録も `redactFilesystemPaths` で `[redacted-path]` に置換している。
**同じログの中で方針が割れている**のが問題である。

## 2. 決定

**診断で要るのは「Tailscale が呼び出し元を識別したか」までで、「誰か」は要らない。**

- `tailscale_user_headers` は**ヘッダ名の一覧だけ**を記録する。値は記録しない
- `x_forwarded_for` (tailnet の IP、`100.x.x.x`) は**そのまま残す**。利用者自身の
  tailnet 内のアドレスで、どの端末からの要求かを切り分けるのに要る。氏名やメールアドレスとは
  性質が違う

守るべき不変条件は 1 つ。**これをテストの中心に据えること。**

> **診断ログに `Tailscale-User-*` ヘッダの値が現れることはない。**

この不変条件は、ファイルパスについて既に成立しているものと同じ形である。

## 3. 変更内容

- `request_proxy_details` の `tailscale_user_headers` を、値の入った object ではなく
  **ヘッダ名の配列**にする。並びは安定させる (順不同の HTTP ヘッダで結果が揺れないよう
  ソートする)
- **ヘッダの有無は残す。**「Tailscale 経由で認証済みの要求だったか」は診断に要る情報である
- 値を落とすことでフィールドの形が変わるので、**フィールド名も実態に合わせて改める**
  (例: `tailscale_user_header_names`)。古い名前のまま中身だけ変えると、過去のログと
  同じ名前で意味が違うものが混ざる
- doc comment に、**このログは個人を特定できる値を持たない**という約束を書く。
  ファイルパスと同じ扱いであることが分かる形にする

## 4. 触らないもの

- `x_forwarded_for` / `remote_addr` / `https_detected` / `https_source`
- クライアント側の記録階層 (`telemetry_tier`) と `redactFilesystemPaths`
- 認証そのもの (PIN、Tailscale ヘッダを認証に使っているならその判定ロジック)。
  **記録の内容だけを変える**
- 診断ログの出力先・ローテーション (別件)
- 本体 (`src/`) と protocol

## 5. テスト

```
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer-remote
cargo test -p mimageviewer --lib remote_ipc::
cargo fmt --all -- --check
```

既存の `proxy_headers_and_remote_address_are_exposed_to_request_log_details` は
**メールアドレスがログに出ることを assert している**ので、§2 の不変条件と正面から矛盾する。
不変条件を表す名前へ変え、中身も入れ替えること。

- `Tailscale-User-Login` / `-Name` / `-Profile-Pic` を付けた要求のログに、**その値が
  どこにも現れない** (details 全体を文字列化して検査する。特定フィールドだけ見ない)
- ヘッダ名の一覧は残り、並びが安定している
- Tailscale ヘッダが無い要求では一覧が空になる

## 6. ドキュメント

- plan に **§14.20** (または次の空き番号) を追加する。ログが個人を特定できる値を持たない
  約束と、`x_forwarded_for` を残した理由を書く
- `docs/briefs/HANDOFF.md` は**触らない** (並行して編集している)

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`crates/` と `src/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する
