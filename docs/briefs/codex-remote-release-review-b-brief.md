# リリースレビュー対応 B: RR-03 / RR-05

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

出典は `docs/briefs/codex-remote-release-review-2026-08-13.md`。A (RR-06 / RR-01 / RR-02) は
`e6a5c488` / `a0860c00` / `5cd4fba0` で入っている。

- **1 件 = 1 コミット**。RR-03 と RR-05 でファイルは重ならない。
- 高速化・リファクタの同乗はしない。
- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- `cargo fmt --all` を通し、下記テストを走らせる。
- **commit も行う**。前回 sandbox で `.git` へ書けなかった場合は、変更を残したまま報告すれば
  こちらでコミットする (それも報告に書くこと)。

---

## RR-03: UNC / device namespace をリモートの住所として受け付けない

**利用者の判断は取得済み**: レビューの選択肢 1 (remote では UNC を拒否する) を採る。
理由は、ネットワークドライブに**ドライブ文字を割り当てている場合は UNC ではない**ので、
実用上の大半を保ったまま「認証済みクライアントが任意の SMB server へ本体を接続させる」経路を閉じられるため。

### 現状

- 共有 `validate_absolute_path` (`crates/remote-ipc/src/lib.rs:120-135`) は
  `value.starts_with(r"\\")` を**明示的に許可**しており、テスト (`:3397-3404`) が成功を固定している。
- remote-web (`crates/remote-web/src/path_guard.rs:12-16`) と本体 (`src/remote_ipc/path_guard.rs:70-80`)
  の両方が、ブラウザから来た絶対 path をこの validator に通してから `std::fs::canonicalize` する。
- HTTP API に「UI が発行した address か」を示す capability は無いので、認証済みクライアントは
  query を自分で組み立てられる。到達不能な UNC を渡せば worker を長時間占有でき、悪意ある
  server を渡せば Windows の SMB client が outbound の認証を試みる。

### 実装

#### 1. 構文検証で拒否する

`validate_absolute_path` を、**先頭 2 文字がどちらも `/` または `\`** の値を拒否するように変える。
`\\server\share`、`//server/share`、`\\?\C:\...`、`\\.\PhysicalDrive0`、および `\/` `/\` の混在形を
まとめて閉じる (Windows はこれらを同一視する)。既存の `starts_with(r"\\")` の許可節は削除する。

- ドライブ文字の絶対 path (`C:\...`, `Z:/...`) は従来どおり受理する。
- `\foo` のようなドライブ相対 path は従来どおり拒否のまま。
- `AddressError` に新しい variant を足し、「絶対 path でない」と「ネットワーク path なので対象外」を
  呼び出し側が区別できるようにする。
- `crates/remote-ipc/src/lib.rs:3397-3404` の UNC 受理テストは、新しい契約へ書き換える。

#### 2. 理由を利用者へ見せる

`ResolveError` にも対応する variant を足し、HTTP 応答のメッセージを
「ネットワーク共有 (`\\server\share` 形式) はリモートからは開けません。ドライブ文字を割り当ててください。」
という趣旨にする。汎用の「不正なパス」で潰さない — お気に入りが UNC で登録されている利用者は、
一覧には出るが開けない状態になるので、**理由が分からないと直しようがない**。

#### 3. ドライブ文字の往復を壊さない (重要)

住所は必ず「本体 → ブラウザ → 本体」と往復する。したがって**本体が publish する `logical` に
UNC 形が出ると、次の要求が 1. で拒否される**。

`src/remote_ipc/path_guard.rs:82-91` の `logical_path_from_canonical` は
`\\?\UNC\server\share\...` を `\\server\share\...` へ戻す。つまり
`std::fs::canonicalize` が**ネットワークドライブ (`Z:` 等) に対して UNC 形を返す**なら、
ドライブ文字を割り当てていても住所が UNC になり、この修正でリモートから開けなくなる。

**この点はこの PC では検証できなかった** (ネットワークドライブが 1 つも割り当てられておらず、
ローカルの SMB server も動いていないため、実際に mount して測れない)。どちらの挙動でも
正しく動くように実装すること:

- `resolve_existing` (remote-web 側・本体側の両方) で、**canonical が `\\?\UNC\` で始まり、
  かつ呼び出し元が渡した path がドライブ文字の絶対 path だったとき**は、
  publish 用の `logical` に**呼び出し元のドライブ文字形を採用する**。
  `..` などが残らないよう `std::path::absolute` で字句正規化した値を使う。
- それ以外は現在の挙動を変えない (canonical が drive path ならこの分岐は発火しない)。
- I/O に使う `canonical` は常に canonicalize の結果のままにする。**この分岐は publish 用の
  表記だけを変える**。
- 判定は `(caller_path, canonical_path) -> logical_path` の**純関数**として書き、
  実際のネットワーク mount 無しでテストできるようにする。
  例: `("Z:\\photo\\..\\photo", r"\\?\UNC\nas\share\photo")` -> `Z:\photo`。

これは独立した意味を持つので、**RR-03 の中で別コミットにしてよい** (壊れたときに単独で戻せる)。

#### 4. マニュアル

`htdocs/mimageviewer/manual/remote.html` の「できないこと」に、
`\\server\share` 形式のネットワーク共有はリモートからは開けないこと、ドライブ文字を
割り当てれば開けることを書く。実装用語 (UNC / device namespace) は使わない。

### テスト

- `\\nas\share\a.jpg`、`//nas/share/a.jpg`、`\\?\C:\a.jpg`、`\\.\PhysicalDrive0` を拒否する。
- `C:\a.jpg`、`Z:/a.jpg` は従来どおり受理する。
- **拒否が filesystem I/O より前に起きる**ことを固定する (存在しない host を渡しても
  canonicalize へ進まない)。実機から未知の SMB server へ接続してはならない。
- 3. の純関数テスト (上の例を含む)。
- 既存の path_guard テストが全て通ること。

---

## RR-05: ログアウトと、全端末の失効

### 現状

- Cookie の有効期限は 90 日 (`crates/remote-web/src/auth.rs:16`, `:237-263`)。
  PIN 画面の「この端末を記憶しない」は**既定 unchecked** なので、通常操作は 90 日保存になる。
- auth route は status と PIN login だけで logout が無い (`crates/remote-web/src/http.rs:576-580`)。
- Cookie は `v1.<expires>.<HMAC>` の**署名済み stateless token** で、端末識別子を含まない。
  したがって「特定の 1 台だけを失効させる」ことは stateless のままでは実現できない。
- `session_secret` は認証ファイル (`remote-web-auth.json`) の中にあり、
  `set_pin_file` が**PIN 設定のたびに再生成**する (`crates/remote-ipc/src/auth.rs:51-71`)。
  つまり PIN を変えると全 Cookie が失効する。remote-web は起動時にこのファイルを読むだけ。

### 実装

**「この端末からログアウト」と「すべての端末を失効」は別の要件**として、両方入れる。
stateless token に revocation を偽装する実装 (field を足して失効した気にさせる等) はしない。

#### 1. `POST /api/auth/logout` (remote-web)

- 認証済み・未認証を問わず 200 を返してよいが、**発行時と同じ属性**で Cookie を削除する:
  `Path=/`、`HttpOnly`、`SameSite=Lax`、`Max-Age=0`、および HTTPS proxy 判定時のみ `Secure`。
  属性が食い違うとブラウザは別 Cookie として扱い、削除に失敗する。
- 他の `/api/auth/*` と同様、remote session 取得を要求しない。
- 許可しない method には 405 を返す (既存の `/api/auth/status` と同じ形)。

#### 2. Web UI のログアウト

メニューに「ログアウト」を追加し、押したら `/api/auth/logout` を呼んでから PIN 画面へ戻す。
取得済みの remote session があれば、先に手放してから戻ること (他端末が使えない状態で
放置しない)。「この端末を記憶しない」で入った場合との差は無い。

#### 3. 「すべての端末をログアウト」(本体)

- `crates/remote-ipc/src/auth.rs` に、**`pin_hash` を保ったまま `session_secret` だけを
  再生成して同じ atomic 書き込みで保存する**関数を足す。
- 本体の接続ダイアログに「すべての端末をログアウト」ボタンを置く。押すと上記を実行し、
  **所有している remote-web child を再起動する** (remote-web は起動時にしか認証ファイルを
  読まないため)。再起動は `tailscale serve` 設定成功時と同じ既存経路を使う。
- 押す前に「この PC に接続中の端末を含め、すべての端末で PIN の再入力が必要になります。
  PIN は変わりません。」という趣旨の確認を出す。
- PIN 変更でも同じ失効が起きることは既存の挙動なので変えない。

#### 4. マニュアル

`remote.html` と `tut-remote.html` に、次を書く。

- 一度 PIN を入力した端末は、既定でしばらく (90 日) 記憶されること。
- 借りた端末などで記憶させたくない場合は「この端末を記憶しない」を使うこと。
- その端末でログアウトする方法。
- 端末を紛失した場合など、**すべての端末で入り直させる**方法 (接続ダイアログのボタン)。

内部用語 (Cookie の署名、HMAC、stateless) は書かない。「記憶」「入り直す」で説明する。

### テスト

- logout 後、同じブラウザ (= 同じ Cookie) からの API が 401 になる。
- 削除 Cookie の属性が発行 Cookie と一致する (`Secure` の有無も含めて、
  proxy 判定が HTTPS のときと平文のときの両方)。
- `/api/auth/logout` に GET 等を投げると 405。
- session secret を rotate すると、rotate 前に発行した Cookie が全て無効になる。
  rotate 後も**同じ PIN で login できる** (`pin_hash` を壊していない)。
- 認証ファイルの書き込みが従来どおり temp file + rename であること。

---

## 実行するテスト

```
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc
node --test
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

`cargo test -p mimageviewer --lib` の前に、必要なら
`cp vendor/ffmpeg/bin/*.dll target/debug/deps/`。

`node --test` が sandbox の子プロセス生成で `spawn EPERM` になる場合は、
前回と同じく isolation 無効で個別実行してよい (その旨を報告に書くこと)。

## 報告してほしいこと

- コミット hash と、それぞれで何を変えたか (コミットできなかった場合はその旨)。
- RR-03 の 3. について、**この PC で実際のネットワークドライブ挙動を検証できたか**
  (できないはず。できないなら「純関数テストで両方の挙動に備えた」と書く)。
- ブリーフと意図的に違えた点があれば、その理由。
