# リモート: JSON 応答の圧縮と、診断ログのローテーション

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

レビュー対応 A / B は `e6a5c488` / `a0860c00` / `5cd4fba0` / `71c5f8ef` / `2c19be8d` で完了済み。
本ブリーフは別件 2 つ。**1 件 = 1 コミット**。

- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、下記テストを走らせる。

---

## 1. JSON 応答を gzip で返す

### 背景 (実測値)

一覧の応答は **1 件あたり 250 バイト**で、**現在まったく圧縮していない**
(`Content-Encoding` の実装が remote-web に無い)。パスは前方一致が多いので gzip が非常によく効く:

| 件数 | 無圧縮 | gzip | 比 |
|---:|---:|---:|---:|
| 1,000 | 245 KiB | 16 KiB | 15.6x |
| 5,000 | 1.24 MiB | 68 KiB | 18.6x |
| 6,757 | 1.70 MiB | 94 KiB | 18.2x |

現在の上限 1000 件でも 245 KiB を毎回送っている。細い回線・relay 経由の tailnet では体感に出る。

`flate2` は `zip` 経由で既に `Cargo.lock` にあるので、依存は増えない (1.1.9)。

### 実装

**RR-06 で応答の最終化を `handle()` の finalizer 1 箇所へ集約した。圧縮もそこへ入れる。**
個々の handler へ散らさないこと。

条件をすべて満たすときだけ圧縮する:

1. リクエストの `Accept-Encoding` に `gzip` が含まれる (`q=0` の指定は尊重する)。
2. 応答の `Content-Type` が JSON (`application/json...`)。
   **画像・動画・WebP・JPEG は圧縮しない** (既に圧縮済みで CPU の無駄)。
3. body が閾値以上 (例: 1 KiB 未満は圧縮しない。ヘッダ分で逆に増えるため)。
4. **`/api/auth/` 配下は圧縮しない**。認証応答に攻撃者が制御できる入力と秘密が同居する形を
   作らない (BREACH 系の回避)。現状そうなっていなくても、将来の変更で踏まないようにする。
5. `/stream/` 配下は対象外 (JSON ではないうえ、byte offset を持つ経路を触らない)。

圧縮したときは `Content-Encoding: gzip` と `Vary: Accept-Encoding` を付ける。
`Content-Length` は tiny_http が body から算出するので、**圧縮後の body を渡す**こと。

診断ログの `response_bytes` は、**実際に送ったバイト数** (= 圧縮後) を記録する。
圧縮前後の両方を残せるなら `response_bytes` は送信量、圧縮前は別 field にする
(既存の解析スクリプトが `response_bytes` を転送量として読むため、意味を変えない)。

### テスト

- `Accept-Encoding: gzip` あり + 大きい JSON → `Content-Encoding: gzip` が付き、
  展開すると元の JSON と一致する。
- `Accept-Encoding` なし → 圧縮されない (body がそのまま)。
- `Accept-Encoding: gzip;q=0` → 圧縮されない。
- 画像応答 (`image/jpeg` 等) → `Accept-Encoding: gzip` があっても圧縮されない。
- 1 KiB 未満の JSON → 圧縮されない。
- `/api/auth/status` → 圧縮されない。
- RR-06 の共通ヘッダ 4 種が、圧縮した応答にも 1 個ずつ付く (既存テストを壊さない)。

---

## 2. 診断ログをローテーションする

### 背景

`crates/remote-web/src/diagnostics.rs` の `DiagnosticsLogger::open` は
`OpenOptions::append(true)` で開くだけで、**サイズ上限もローテーションも無い**。
実測で 1 ファイル 59 MB まで育った。詳細ログを ON にしたまま使い続けると際限なく増える。

本体の perf log (`src/perf.rs`) は `perf_events.jsonl` / `.1.jsonl` … `.4.jsonl` の
**5 世代**を起動時にずらす方式を持っている (`rotate_logs`)。同じ見た目・同じ世代数に揃えると
利用者がログを探しやすい。ただし本体は**起動時だけ**ローテーションするので、
1 セッションが長い remote-web ではそれだけでは足りない。

### 実装

- 起動時に本体と同じ形でローテーションする (`<name>.jsonl` → `.1.jsonl` → … → `.4.jsonl`、
  最古は削除)。世代数は本体と同じ 5。
- **加えて、書き込み中に現在ファイルが上限を超えたらその場でローテーションする**。
  上限は定数にし、なぜその値かをコメントに残す (目安 16 MiB。5 世代で合計 80 MiB)。
  上限判定のために毎回 `metadata()` を呼ばない — 書いたバイト数を自分で数え、
  open 時の既存サイズに足していく。
- ローテーション失敗は静かに無視してよい (診断用途なので、失敗で本機能を止めない)。
  ただし `eprintln!` で 1 行残す。
- 排他: `writer` の `Mutex` を持っている間にローテーションする。ロック外で
  ファイルを差し替えない。

`src/perf.rs` の `rotate_logs` を remote-web から呼べるようにするための共通化はしない
(別 crate で、片方は起動時のみ・片方はサイズ契機と条件が違うため)。同じ**見た目**に
揃えるだけでよい。

### テスト

- 上限を小さくしたロガーへ書き込むと `.1.jsonl` が生まれ、現在ファイルが小さくなる。
- 5 世代を超えると最古が消える。
- ローテーション後も `path()` は現在ファイルを指す。
- 既存の「秘密値を残さない」テストが引き続き通る (ローテーションで redaction を飛ばさない)。

---

## 実行するテスト

```
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc
node --test
cargo fmt --all -- --check
```

`node --test` が sandbox の `spawn EPERM` で動かない場合は、各 `*.test.mjs` を直接実行してよい
(その旨を報告に書くこと)。

## 報告してほしいこと

- 何を変えたか (コミットはこちらで行う)。
- gzip の閾値・除外条件を変えた場合はその理由。
- 診断ログの上限値を変えた場合はその理由。
