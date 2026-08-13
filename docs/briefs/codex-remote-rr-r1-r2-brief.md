# リリース保留の 2 件 (RR-R1 / RR-R2)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

- **1 件 = 1 コミット** (2 コミット)。
- `docs/briefs/HANDOFF.md` と他の brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

正本は [codex-remote-release-review-2026-08-13.md](codex-remote-release-review-2026-08-13.md) の §0。
観測事実と行番号はそちらにある。**このブリーフは決まったことだけを書く。**

---

## コミット 1: RR-R1 — 完了処理をダイアログの寿命から外す

### 何が壊れているか

「すべての端末をログアウト」の完了 receiver が `RemoteConnectionDialogState` の中にあり
([ui.rs](../../src/remote_ipc/ui.rs) の `session_logout`)、ダイアログを閉じると
`connection_dialog = None` で receiver ごと捨てられる。署名鍵の rotate 自体は service 側で
続くので Cookie は失効するが、**成功時にだけ呼ばれる `handle.local_disconnect()` が走らず**、
本体側の古いリモート操作権が通常 60 秒 (非終端 job があれば最大 10 分) 残り得る。

### 直し方

**`session_logout` を、ダイアログより長生きする `self.remote_session_ui` の側へ移す。**
ダイアログは描画のためにそれを**読むだけ**にする。

- **poll はダイアログの有無に関係なく毎フレーム走らせる**。`show_remote_connection_dialog` は
  毎フレーム呼ばれているので、**`connection_dialog` が `None` かを見るより前**に
  `Running` の receiver を try_recv する形でよい。
- **`Running` の間は、ダイアログが閉じていても repaint を要求し続ける** (今は開いている分岐でしか
  要求していない)。これを忘れると egui が眠って完了を拾えない。
- 「実行中は閉じるボタンを無効化する」で済ませない。window close や将来の別経路に同じ不変条件が
  残る。**所有者を移すこと自体が修正**。

### 閉じたときの扱い (決定済み。この通りに実装する)

| 閉じた時点の状態 | どうするか |
|---|---|
| `Confirming` | `Idle` に戻す (確認はキャンセル扱い) |
| `Running` | **そのまま走らせて完了させる**。`local_disconnect()` は成功時にちょうど 1 回 |
| `Finished(Ok)` | 黙って消す (成功は報告不要) |
| `Finished(Err)` | `logger::log` に残し、**状態は保持**して次にダイアログを開いたとき表示する |

「成功は報告不要、失敗は見られるまで残す」が規則。

### 触らないもの (意図的な非対称)

`pin_editor` の `Saving` と `tailscale_serve_setup` の `Running` も同じ形で receiver を
ダイアログに持たせているが、**落としても失われるのは結果表示だけ**で、必要な副作用は service 側で
完了する。今回は動かさない。**なぜ session_logout だけ違うのかをコード中のコメントに残すこと**
(後から「揃っていない」と誤読されないように)。

### 回帰テスト

- rotate 開始 → ダイアログ破棄 → 成功完了でも `local_disconnect()` 相当が**必ず 1 回**起きる
- ダイアログを開いたままの成功経路で**二重に drain しない**
- rotate 失敗時は成功表示にならず、PIN と session secret の状態を既存仕様どおり区別する
- 上の表の 4 状態の遷移

`local_disconnect` の呼び出し回数を数えられる形になっていないなら、**状態遷移を純関数へ切り出して
「drain すべきか」を返す**形にしてテストする (`remote_session_logout_transition` が既にあるので
そこへ寄せるのが素直)。

---

## コミット 2: RR-R2 — Cookie に login ごとの nonce を入れる

### 何が壊れているか

session Cookie の署名対象が `v1.{expires}` だけで、`expires` は秒精度。**同じ秒に PIN 認証した
別端末は完全に同一の Cookie になる**。`AuthSessionIdentity` は Cookie 全文の SHA-256 なので
server は 2 台を区別できず、`RemoteClientIdentities::resolve` が先に保存した client id を返し、
session id も同じ entry を上書きする。**常時 1 台だけが操作権を持つ**という不変条件が交差する。

### 直し方

`v2.{expires}.{nonce}.{mac}` にする。

- `nonce` は **login ごとの CSPRNG**。`getrandom` は既に依存にある
  (`AuthToken::generate` が使っている)。**16 バイト / hex 32 文字**とする。
- MAC は `version || expires || nonce` を対象にする (いまと同じ HMAC-SHA256、同じ鍵)。
- 検証は **MAC を定数時間で比べる前に**、部品数・`v2`・`expires` の数値・`nonce` の長さと
  hex 構文を検証する。
- 生成が失敗したら (OS 乱数が取れない) **Cookie を発行せず認証を失敗させる**。
  nonce 無しの Cookie に落とさない。

### v1 の扱い (決定済み)

**v1 は受け付けない。移行期間を設けない。**

リモート閲覧は **v3.0.0 の目玉で、まだ出荷していない**。CLAUDE.md の「永続データ・スキーマ変更時の
判断」に従い、未リリースの機能に移行コードは要らない。手元の検証端末が 1 回ログインし直すだけで済む。
**二重形式を維持しないことが目的**なので、v1 を読む経路は残さない。この判断をコード中のコメントに
残すこと。

### 回帰テスト

- **同一 `now_unix` で連続発行した Cookie が異なり、`AuthSessionIdentity` も異なる**
  (これが本命。時刻を固定して 2 回発行する)
- `remember=true/false` でも token identity を共有しない
- 同時 login の 2 Cookie を `RemoteClientIdentities` へ渡し、**client id と session id が交差しない**
- v1 形式の Cookie は拒否される
- nonce の長さ違い / 非 hex / 部品数違いを MAC 検証の前に拒否する
- 正しい v2 Cookie は通り、期限切れは通らない (既存テストが v1 前提なら v2 へ更新する)

---

## 実行するテスト

```
cargo test -p mimageviewer-remote
cargo test -p mimageviewer --lib remote_ipc
cargo fmt --all -- --check
```

## 報告してほしいこと

- 2 つの変更それぞれで何をしたか (コミットはこちらで行う)。
- RR-R1 で、`local_disconnect()` がちょうど 1 回であることをどう保証したか。
- RR-R2 で、v1 を読む経路が残っていないこと (grep して確認した結果)。
- テストを潰したときに実際に落ちることを確認した結果 (最低 2 つ: 閉じても drain される件、
  同一秒の Cookie が別 identity になる件)。
- ブリーフと意図的に違えた点があれば、その理由。
