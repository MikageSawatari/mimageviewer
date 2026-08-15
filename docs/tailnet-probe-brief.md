# 実装ブリーフ: tailnet 状態を本体が自分で読めるようにする (backlog §1.88)

対象: v3.0.1 (明日夕方リリース予定)。正本は `docs/web-remote-plan.md`、
背景は `docs/next-release-backlog.md` の §1.88。

## 1. 直したい利用者報告 (2026-08-15、v3.0.0 実機)

1. **Tailscale を入れる前に PIN 設定へ進めてしまう。** リモート接続ダイアログは PIN 入力欄が
   先頭にあり、Tailscale の有無に触れないまま始められる。止まるのは 2 手あとの
   `tailscale serve` 設定なので、そこから手順を戻ることになる。
2. **tailnet 側を後から変えても表示が追随しない。** 利用者は HTTPS 証明書の要件に後から気付き、
   管理コンソールで有効にした。しかしダイアログの表示は変わらず、**リモート接続を無効 → 有効**
   をやり直して初めて反映された。

## 2. いまの構造 (事実)

- `choose_connection_url` (`crates/remote-web/src/connection_url.rs:54`) が
  `tailscale serve status --json` と `tailscale status --json` を読み、URL と tailnet の
  状態をまとめて返す。**remote-web の `run()` (`crates/remote-web/src/main.rs:55`) で
  1 回だけ**呼ばれ、結果を `set_remote_web_connection_info` で本体へ焼き付ける。以後読み直さない。
- ダイアログ (`src/remote_ipc/ui.rs:2659`) が読む `info` は動いている service の snapshot。
  したがって **service を再起動しない限り古いまま**。
- 自力で更新される経路は 1 つだけ: アプリ内の「tailscale serve を設定する」を押した場合、
  `tailscale_serve_completion_plan` が service を再起動する (`src/remote_ipc/service.rs:318`)。
  管理コンソールや別のコマンドラインで変えた場合は取り残される。
- そのため今は **手順「有効にする」と「serve を設定する」を入れ替えられない**。無効の間は
  service が動いておらず、serve の状態も設定ボタンも存在しないため。

## 3. 決めた方針 — 探索を共有 crate へ移し、本体が随時読む

`crates/remote-ipc` に tailnet 探索を移し、**本体と remote-web が同じ 1 つの実装を呼ぶ**形にする。
本体は service の snapshot ではなく自分の探索結果で案内を出す。

これは `docs/web-remote-plan.md` §14.15 / §14.16 の
「JSON の解釈は remote-web だけが所有し、本体は独自検出しない」
「本体から remote-web へ状態再読込を要求する IPC は無く、再起動が再検出の手段」
という記述を**変更する**。**同じコミットで §14.15 / §14.16 を書き直すこと。**
新しい規則はこう書ける:

> tailnet JSON の解釈は共有 crate が 1 つだけ持ち、本体と remote-web はそれを呼ぶ。
> remote-web は起動時に 1 回読んで自分の公開 URL を決める。本体はダイアログの案内のために
> 随時読み直す。両者は同じ関数を通るので解釈が分かれない。

**なぜ IPC ではなくこの形か**: 本体が IPC server、service が client なので、本体から
「読み直せ」を送るには逆向き要求の仕組みが要る。しかもそれでは **service が動いていない間
(= リモート無効の間) は何も分からない**ままで、報告 1 の順序問題が解けない。共有 crate 方式は
無効の間も読めるので、両方が同時に解ける。

### 3.1 これで手順の入れ替えも可能になる

案内ブロック (Tailscale の有無 / HTTPS 証明書 / 鍵の期限 / serve 状態と設定ボタン) が
service の稼働に依存しなくなるので、**リモート接続が無効の間も表示・操作できる**ようにする。
`tailscale serve --bg <port>` 自体はもともと service の稼働と無関係に実行できる。

**URL と QR コードだけは従来どおり service の snapshot (`info.public_url`) を使う** —
それは「service が実際に配信している場所」であって tailnet の状態ではない。

## 4. 変更する場所

### 4.1 `crates/remote-ipc` (共有)

`connection_url.rs` から tailnet を読む部分を移す。移すのは
`TailscaleServeState` / `TailscaleStatusState` と
`tailscale_serve_status` / `tailscale_serve_status_json` / `inspect_tailscale_serve` /
`unknown_serve_state` / `proxy_targets` / `serve_url` /
`tailscale_status` / `tailscale_status_json` / `inspect_tailscale_status` /
`unknown_tailscale_status` / `parse_rfc3339_unix_seconds` / `valid_date` /
`days_from_civil` / `ts_hostname`、および**それらに対応する既存テスト**。

公開 API (名前は提案、より良い案があれば可):

```rust
pub struct TailnetProbe {
    /// tailscale.exe を見つけられたか。見つからない場合、以下はすべて Unknown / None になる。
    pub cli_found: bool,
    pub serve: RemoteWebFeatureStatus,
    pub serve_url: Option<String>,
    pub serve_conflict: Option<String>,
    pub serve_unsupported_path: Option<String>,
    pub status_url: Option<String>,
    pub https_certificate: RemoteWebFeatureStatus,
    pub key_expiry_unix_seconds: Option<i64>,
}

pub fn probe_tailnet(address: SocketAddr) -> TailnetProbe;
```

`serde_json` は `crates/remote-ipc/Cargo.toml` に既に入っている。

### 4.2 `crates/remote-web/src/connection_url.rs`

`choose_connection_url` は `probe_tailnet` を呼ぶだけの薄い層になる。
**URL の決定順序 (`--url` → serve → status → bind fallback) と `normalize_base_url` は
現状のまま変えない。** `detect_peer_info` は `tailscale status --json` を別途読むので、
必要なら共有 crate の実行 helper を使う形に寄せてよいが、**挙動は変えない**。

### 4.3 `src/remote_ipc/service.rs`

`RemoteServiceControl` に探索要求を足す:

```rust
pub(crate) type RemoteTailnetProbeReceiver = mpsc::Receiver<TailnetProbe>;
pub(crate) fn probe_tailnet(&self) -> Result<RemoteTailnetProbeReceiver, String>;
```

**owner worker (`run_service_manager`) では実行しないこと。** あのループは service プロセスの
生死を見ており、探索は CLI 2 回で最大 16 秒かかり得る。読み取り専用で service の状態を一切
触らないので、**要求ごとに専用スレッドを立てて結果を mpsc で返す**。
(`configure_tailscale_serve` が owner worker なのは成功時に service を再起動するため。
探索にその必要は無い。)

探索に渡すアドレスは **service に渡しているポートと同じ出所**にする:
`SocketAddr::new(IpAddr::from([127, 0, 0, 1]), self.port)`。
`remote_command` (`src/remote_ipc/service.rs`) は `--bind` を渡していないので service 側は
既定の `127.0.0.1` で bind する。**この 2 つがずれると `proxy_targets` の一致判定が壊れ、
設定済みの serve を「未設定」と誤判定する。** ずれない形で書き、その理由をコメントに残すこと。

### 4.4 `src/remote_ipc/ui.rs` (ダイアログ)

状態を足す (既存の `RemoteTailscaleServeSetupState` と同じ形):

```rust
enum RemoteTailnetProbeState {
    Idle,
    Running { receiver: RemoteTailnetProbeReceiver },
    Finished(TailnetProbe),
}
```

**探索を走らせるタイミング** (毎フレームは禁止):

1. ダイアログが閉から開へ変わったとき
2. 新設する「再確認」ボタンを押したとき
3. `tailscale serve` 設定が完了したとき (成功・失敗とも。serve 状態が変わるため)

実行中は spinner を出しつつ、**直前の結果は消さずに表示したままにする** (点滅させない)。

**表示の変更点**:

- **Tailscale が見つからないとき**: PIN 行より**前**に、先に Tailscale を入れる必要がある旨を
  出す。tailscale.com の入手ページとマニュアルへのリンクを添える。文言は
  「見つかりません」で終わらせず、**次にすべきこと**を書く。
- 見つかっているが状態を読めないときは、従来の「状態を読み取れません」相当を出す
  (この 2 つは今 1 つの文言に混ざっている — 分けること)。
- HTTPS 証明書 / 鍵の期限 / serve 状態と設定ボタンは、**`accepting` の条件から外し**、
  探索結果から描く。リモート接続が無効でも見えて操作できる。
- **URL と QR は従来どおり `accepting` かつ `info` があるときだけ**、`info.public_url` から描く。
- serve の衝突警告は今 `info.public_url` を文中に使っている。案内ブロックが `info` に依存
  しなくなるので、**探索結果から文を組み立て直す**こと。`info` が無い場合に文が壊れないように。

**変えてはいけないこと**: リモート接続の有効化は **PIN だけ**で決める。証明書・鍵の期限・
Tailscale の有無で有効化ボタンを止めない (plan §14.16 の判断: ローカルの
`http://127.0.0.1` 確認経路を残すため)。

### 4.5 マニュアル

`htdocs/mimageviewer/manual/tut-remote.html` と `htdocs/mimageviewer/manual/remote.html` に、
**現状の制限として書いた記述**がある:

- 「リモート接続を有効にした時点のもの」
- 「Tailscale 側を変えたら、いったん無効にしてからもう一度有効にする」
- 番号付き手順の直後にある `box warn`

**この制限が無くなるので、該当箇所を削除して現在の挙動に合わせる。** 手順の番号も、
serve 設定が有効化の前にできるようになるなら見直す (現在は 1 Tailscale 導入 →
2 HTTPS 証明書 → 3 PIN → 4 有効化 → 5 serve)。文体は既存に合わせ、
実装語 (IPC / snapshot / プローブ 等) を出さない。

## 5. テスト

- 共有 crate: 移した既存テストがそのまま通ること。`cli_found` の分岐 (CLI 不在時に
  serve / https が Unknown、URL が None) を足す。
- `src/remote_ipc/ui.rs` の `mod tests`: 表示要素を決める純関数に
  「CLI 不在」「CLI はあるが読めない」「証明書無効」「serve 設定済み」の分岐テスト。
  `remote_tailscale_serve_elements` の入力が変わるならテストも更新。
- `src/remote_ipc/service.rs` の `mod tests`: 探索が owner worker を塞がないこと
  (= 専用スレッドで走ること) を、構造で確認できる形にする。
- 実行: `cargo test -p mimageviewer-ipc`、`cargo test -p mimageviewer-remote`、
  `cargo test -p mimageviewer --lib remote_ipc::`。
- `cargo fmt` をワークスペース全体にかけてから終えること (pre-commit フックが `--check` で弾く)。

## 6. 対象外

- `tailscale serve` の解除代行 (plan §14.15 の判断どおり行わない)。
- 定期ポーリング。探索は上の 3 契機だけ。
- IPC protocol version の変更。この方式では不要なはず。必要になったら**手を止めて報告**すること。
- リモート接続の有効化条件の変更。

## 7. 進め方

- コミットは分けてよいが、**plan とマニュアルの更新は実装と同じコミットに含める**。
- 途中で「これは §1.88 の範囲を超える」と判断したら、**症状パッチを入れずに報告**する
  (CLAUDE.md「バグ修正の一般原則」)。
- Windows ネイティブの実機確認 (実際の tailnet) は利用者が行う。実装側はビルドとテストまで。
