# リリースレビュー対応 A: RR-06 / RR-01 / RR-02

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

出典は `docs/briefs/codex-remote-release-review-2026-08-13.md` (自分が書いたレビュー)。
そのうち **実機確認を必要としない 3 件** を、独立した 3 コミットで実装する。

- 3 件は互いに独立している。**1 件 = 1 コミット**にして、レビューしやすくする。
- 高速化・リファクタの同乗はしない。表示速度 / 先読みの作業は別セッションで進行中。
- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- 実装後に `cargo fmt --all` を通し、下記のテストを走らせる。コミットまで行う (push はしない)。

---

## RR-06: すべての HTTP 応答を frame 埋め込み拒否にする

### 現状

`crates/remote-web/src/http.rs`

- 共通 header は `route()` の**末尾**で付いている (`:831-833`)。`X-Content-Type-Options: nosniff` と
  `Referrer-Policy: no-referrer` の 2 つだけ。
- `route()` には**早期 return が 2 つ**あり、この末尾を通らない。
  - query parse error → 400 (`:527-532`)
  - remote session 未取得 → `session_response_http` (`:551-563`)
- `video_route` のときだけ付ける `with_body_error_code_log()` (`:826-830`) も同じ理由で早期 return を素通りする。

### 実装

1. **応答の最終化を `handle()` の 1 箇所へ移す。** `route()` は「route して応答を作る」だけにし、
   共通 header と `with_body_error_code_log()` の付与は `handle()` が `route()` の戻り値へ行う。
   `video_route` の判定 (path が `/api/video/` または `/stream/` で始まる) も `handle()` 側で
   `request.url()` から求める。これで**早期 return を含む全応答**が同じ最終化を通る。
2. 共通 header に次の 2 つを追加する。
   - `Content-Security-Policy: frame-ancestors 'none'`
   - `X-Frame-Options: DENY`
3. `default-src` などを含む完全な CSP は inline bootstrap の nonce / hash 設計が要るので**今回はやらない**。
   `frame-ancestors` だけ先に入れる。

`headers` は `Vec` に push される (`:446-449`, `:4165-4169`) ので、二重付与すると同名 header が
2 つ出る。最終化を 1 箇所にした結果、同名 header が重複しないことをテストで固定する。

### テスト

`crates/remote-web/src/http.rs` の既存テストに追加する。

- 次のすべてで 4 つの共通 header が付く: index (200)、静的 asset、401 (未認証)、
  400 (query parse error)、session 未取得応答、`/stream/` の error、405、404。
- 同名 header が 1 つずつしか出ないこと。
- 既存の `X-mIV-Request-Id` 付与と log 出力が壊れていないこと。

---

## RR-01: 英字・記号の PIN をスマートフォンで入力できるようにする

### 現状

- 本体の正本 validator は `!`〜`~` の印字可能 ASCII を許可する (`crates/remote-ipc/src/auth.rs:31-44`)。
- 本体 UI とマニュアルも「半角英数字・記号」と案内している。
- 一方で Web の PIN 欄は数字キーボードを要求している
  (`crates/remote-web/web/app.js:2185-2193`)。`pin.inputMode = "numeric"`、
  placeholder は `"6桁以上の PIN"`。

iOS / Android の数字キーボードからは英字・記号へ到達できない、または著しく難しい。
`Abc!23` のような有効な PIN を設定した利用者が、主対象であるスマートフォンから login できない。

### 実装

`crates/remote-web/web/app.js` の `renderPinLogin`:

- `pin.inputMode = "numeric"` を**削除する** (既定の text キーボードにする)。
- placeholder を `"6文字以上の PIN"` にする (「桁」は数字を連想させる)。
- `pin.autocapitalize = "none"` と `pin.spellcheck = false` を設定する
  (モバイルの自動大文字化・自動修正を防ぐ)。`pin.type = "password"` と
  `autocomplete = "current-password"` は現状のまま維持する。

**PIN の許容文字を数字だけへ狭める修正はしない。**既存仕様の機能低下になる。

本体 validator の printable ASCII テストは 1 つも消さない。

### テスト

- Web の DOM 契約テスト (`crates/remote-web/web/` の `node --test` 側) に、PIN 欄が
  numeric keyboard を要求しないこと、`autocapitalize`/`spellcheck` が固定されること、
  placeholder が「6文字」であることを固定するテストを追加する。
  既存の DOM テストの置き場所と作法に合わせること。
- 該当テストが**現行コードでは落ちる**ことを一度確認してから直す (書いた瞬間に緑のテストは意味がない)。

---

## RR-02: Tailscale Serve のサブパス構成を「設定済み」と誤判定しない

### 現状

- `inspect_tailscale_serve` (`crates/remote-web/src/connection_url.rs:168-207`) は、自分の
  proxy が `/miv` のようなサブパスにあっても `Configured` とし、`https://host/miv/` を接続 URL にする。
- テスト `configured_handler_path_is_part_of_the_connection_url` (`:538-547`) がこの挙動を成功として固定している。
- しかし SPA は origin root 固定である。manifest / icon / CSS / module は `/...`
  (`crates/remote-web/web/index.html:18-20`, `:103`)、API は `/api/...`、stream は `/stream/...`、
  service worker の scope は `/`。したがって `/miv/` を案内しても、asset と API は root へ飛ぶ。
  root が未設定なら 404、別サービスがあればそのサービスへ誤送信になる。

Tailscale は `--set-path` によるサブパス mount を正式に提供しているので、実在し得る設定である。

### 実装

**サブパス対応はしない。**「自分の proxy が root にあるときだけ Configured」に狭め、
サブパスにいる場合は「未設定」ではなく**サブパス構成は未対応**と区別できるようにする。

1. `inspect_tailscale_serve`:
   - 自分の proxy が `/` の handler → 従来どおり `Configured` + URL。
   - 自分の proxy が非 root の handler → `Configured` にしない。その handler path を記録する。
     ループは続ける (同じ host の別 handler が root にいる可能性がある)。
   - ループ後に「非 root の自分の proxy」が見つかっていたら
     `NotConfigured` + その path を返す。
   - 既存の `conflict` (root を別 proxy が占有) の意味は変えない。
2. 状態の運び方: `TailscaleServeState` と `RemoteWebConnectionInfo`
   (`crates/remote-ipc/src/lib.rs:1288-1295`) に
   `tailscale_serve_unsupported_path: Option<String>` を追加する。
   `RemoteWebFeatureStatus` (`:1297-1303`) には variant を足さない
   — 同じ enum を HTTPS 証明書の表示にも使っており、そちらに無関係な arm が増えるため。
3. `PROTOCOL_VERSION` を 46 → 47 へ上げ、`protocol_v46_...` の round-trip テストを新版へ更新する。
   **この feature は未リリースなので migration は不要**。旧版互換コードを足さないこと。
4. 本体 UI (`src/remote_ipc/ui.rs` の tailscale serve 節、`:2672-2700` 付近):
   `tailscale_serve_unsupported_path` が `Some(path)` のとき、
   「tailscale serve は {path} に設定されています。mIV はサブパス構成に対応していないため、
   この URL では接続できません。」という趣旨の警告を出す。
   設定ボタン (`tailscale serve --bg {port}` = root へ mount) はそのまま押せる状態を保つ
   — それがこの状況の正しい解決手段だから。
   文言は CLAUDE.md の「UI 文字列の Unicode グリフ選定ルール」に従う (絵文字・環境依存記号を足さない)。

### テスト

`crates/remote-web/src/connection_url.rs` のテスト:

- `/` の自分 proxy → `Configured`、URL は `https://host/`。
- `/miv` の自分 proxy → `Configured` ではない。`tailscale_serve_unsupported_path` が `Some("/miv")`。
  **`url` は `None`** (動く URL だと誤認させない)。
- `/miv` と `/` の両方に自分の proxy がある → `Configured` + `https://host/`。
- 既存の conflict テスト (`another_proxy_port_is_not_configured_and_reports_the_root_conflict`) は
  そのまま通ること。
- `configured_handler_path_is_part_of_the_connection_url` は**新しい契約に書き換える**
  (削除ではなく、非 root が Configured にならないことを固定する名前へ)。

本体側は `remote_tailscale_serve_elements` 相当の純関数テストで、unsupported path が
あるときに警告要素が出ることを固定する。

---

## 実行するテスト

```
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc
node --test
cargo fmt --all -- --check
```

`cargo test -p mimageviewer --lib` を走らせる前に、必要なら
`cp vendor/ffmpeg/bin/*.dll target/debug/deps/` を実行すること。

## 報告してほしいこと

- 3 コミットの hash と、それぞれで何を変えたか。
- RR-01 / RR-02 のテストが**修正前に落ちること**を確認したか。
- ブリーフと意図的に違えた点があれば、その理由。
