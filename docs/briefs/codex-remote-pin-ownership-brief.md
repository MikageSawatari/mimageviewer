# リモート接続の PIN を本体が所有し、設定ダイアログから設定できるようにする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

**用語**: 「常駐プロセス」= この PC 上で動く `mimageviewer-remote.exe` (HTTP サーバ)。
本体 (`mimageviewer-core.exe`) が起動・停止を所有する子プロセスであり、リモート側の端末ではない。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§3.2** (認証)、**§3.2.1** (service lifecycle)、
  **§6.5.2** (tailscale の権限)
- `crates/remote-web/src/auth.rs` — `set_pin_file` / `load_pin_file` / `validate_pin` /
  `validate_record` / `AuthRecord` / `production_argon2` / `MIN_PIN_CHARS`
- `crates/remote-web/src/config.rs` — `--auth-file` / `--log` / `--set-pin` / `default_data_dir`
- `crates/remote-web/src/diagnostics.rs` — `resolve_external_file_path` / `reject_protected_path`
- `crates/remote-web/src/main.rs` — 起動順 (PIN 読み込み → ログ → library → bind)
- `src/remote_ipc/service.rs` — `remote_command` (子プロセスの引数)
- `src/remote_ipc/ui.rs` — 接続ダイアログ (2300-2430 行付近)
- `src/data_dir.rs` — `get()` と portable の分岐
- CLAUDE.md の **「IME 対応 (日本語入力)」** (TextEdit を足すので必読) と
  **「UI 文字列の Unicode グリフ選定ルール」**

`tailscale serve` の導線は**この増分に含めない**。別ブリーフで扱う。

## 1. 事実 (再調査不要、コードで確認済み)

| 事実 | 場所 |
|---|---|
| PIN の設定手段は `mimageviewer-remote --set-pin <PIN>` の CLI だけ | `config.rs` 71、`main.rs` 42-47 |
| 常駐プロセスは本体が子プロセスとして起動する (`--data-dir` と `--managed-by-core` だけ渡す) | `service.rs` 211-222 |
| PIN 未設定なら常駐プロセスは fail-closed で起動を拒否し、「先に `--set-pin` を実行してください」を返す | `auth.rs` 148-160 |
| その文言はダイアログに出るが、**利用者にコンソールは無い** | `ui.rs` 2357 |
| 認証ファイルと診断ログの既定は **cwd 相対**。Explorer から起動すると cwd はインストール先で、書き込めない | `config.rs` 30-31 |
| 認証ファイル / ログは `%APPDATA%\mimageviewer` と `--data-dir` 配下を拒否する | `diagnostics.rs` 170-185 |
| `RemoteWebConnectionInfo.pin_configured` は `true` のハードコード | `main.rs` 66 |

拒否している理由は「**常駐プロセスは本体のデータディレクトリに一切書かない**」を保つため
(常駐プロセスは `settings.db` を read-only で開く)。この不変条件は維持する。

## 2. 決定 (2026-08-12、利用者判断)

**PIN を書くのは本体とする。** 常駐プロセスは読むだけになる。

- 書き手が 1 つになるので、常駐プロセスの `--set-pin` は**撤去**する。書き手を 2 つ並存させない
- 本体が持ち主なので、**常駐プロセスを起動しなくても「PIN 未設定」が分かる**。
  未設定を専用の終了コードや stderr 文言で表現する必要は無い
- 上の不変条件は「**書き込み**を禁止する」ものだったので、認証ファイルは本体が書く場所
  (= データディレクトリ) に置ける。guard は読み取りだけ許す形に直す

## 3. 変更内容

### 3.1 認証ファイルの形式と書き込みを共有クレートへ移す

`crates/remote-ipc` (= `mimageviewer_ipc`、本体と常駐プロセスの両方が依存する契約クレート) に
認証ファイルの正本を置く。移すのは次だけとする。

- `AuthRecord` と `AUTH_FILE_VERSION`
- PIN 文字数の検証 (`MIN_PIN_CHARS` = 6、上限は現行値を維持)
- Argon2id のパラメータ (`production_argon2` と同一) と、salt 生成・hash 化・session 署名鍵生成
- 認証ファイルの書き込みと読み込み・`validate_record`

`AuthService` (PIN 照合、lockout、Cookie 署名、Bearer) は常駐プロセスに残す。
**同じハッシュ形式を 2 か所に実装しない**ことがこの節の目的である。

書き込みは **temp ファイル + rename** にする。常駐プロセスが起動時に読んでいる最中に本体が
truncate すると、壊れた JSON を読ませ得るため。

### 3.2 保存場所を「書き手」で決める

| ファイル | 書き手 | 置き場所 |
|---|---|---|
| 認証ファイル | 本体 | `<data_dir>/remote-web-auth.json` (portable は `<exe_dir>/data/`) |
| 診断ログ | 常駐プロセス | データディレクトリの**外**。通常版は `%LOCALAPPDATA%\mimageviewer\remote\`、portable は `<exe_dir>\remote\` |

- 本体が両方のパスを決め、`--auth-file` / `--log` で子プロセスへ渡す。常駐プロセスは
  自分で場所を推測しない
- ログ側のディレクトリは本体が作る。`src/data_dir.rs` の cfg 分岐の隣に helper を置く
  (portable 判定を別の場所で書き直さない)
- `reject_protected_path` は**書き込み用途にだけ**適用する。認証ファイルの読み込みは
  データディレクトリ配下を許す。「常駐プロセスは本体のデータディレクトリへ書かない」を
  述語の名前とコメントで明示すること
- **移行コードは不要**。リモート接続機能は未リリースであり、旧 cwd 相対のファイルは
  ユーザーの手元に存在しない (開発機の `remote-web-auth.json` は使われなくなるだけ)。
  この判断をコミットメッセージに 1 行残す

### 3.3 `pin_configured` を protocol から外す

本体が持ち主になったので、`RemoteWebConnectionInfo.pin_configured` は不要になる
(現状ハードコード `true` で、嘘を運んでいる)。フィールドを削除し **protocol version を
43 → 44 へ**上げる。ダイアログは本体が持つ状態を表示する。

### 3.4 接続ダイアログの PIN 導線

`src/remote_ipc/ui.rs` の接続ダイアログに次を足す。

- **状態表示**: `PIN: 設定済み` / `PIN: 未設定`。判定は本体が認証ファイルを読んで行う
  (存在 + `validate_record` が通ること)。壊れている場合は未設定と同じ導線に落とす
- **未設定のとき**: PIN 入力欄と「設定」ボタンを出す。**有効化のチェックボックスは
  PIN が設定されるまで無効**にし、理由を隣に出す。PIN 無しで有効化させて、
  起動失敗のエラーを読ませる形にしない
- **設定済みのとき**: 「PIN を変更」で同じ入力欄を出す
- 入力欄は伏字にする。文字数の検証と失敗メッセージは §3.1 の共有クレートのものを使い、
  UI 側で書き直さない。**エラー文へ PIN を混ぜない**
- 設定に成功したら、**有効なら常駐プロセスを再起動する**。常駐プロセスは起動時に
  認証ファイルを読むので、再起動しないと新しい PIN も新しい署名鍵も効かない。
  変更は session 署名鍵も更新するため、**接続中の端末は PIN の再入力が必要になる**。
  この 2 つを UI に明記する

**IME**: 新しい single-line TextEdit なので `crate::ime_focus` の helper 経由で描画すること
(raw TextEdit は unit test で禁止されている)。Enter / Escape を拾うなら
`dialog_enter_pressed` / `dialog_escape_pressed` を使い、借用衝突を避けて closure の前で
ローカルへ取る。CLAUDE.md の該当節に従う。

**文字**: 新しい UI 文字列を足したら `python scripts/check_ui_glyphs.py` を通す。

## 4. 触らないもの

- `tailscale serve` の導線 (別増分)
- 表示パイプラインの段階 3a / 3b / 3c で入れたもの (coordinator / registry / heavy queue /
  lease / `viewer-position.mjs`)
- PIN 照合、lockout、Cookie / Bearer の仕組みそのもの (置き場所と書き手だけを変える)
- 「常駐プロセスは本体のデータディレクトリへ書かない」不変条件
- 診断ログのローテーション (別件。現状無制限に増えるが、この増分では扱わない)

## 5. テスト

```
cd crates/remote-web/web && node --test
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

- 共有クレート: 書いた認証ファイルを読み戻せる、version 不一致を拒否する、
  6 文字未満と上限超過を拒否する、書き込みが temp + rename である
- 本体: 認証ファイルの有無・破損から「設定済み / 未設定」を正しく出す。
  PIN 設定が有効時に常駐プロセスの再起動を要求する。PIN 未設定では有効化できない
- 常駐プロセス: `--set-pin` が撤去されている。認証ファイルをデータディレクトリ配下から
  **読める**。ログ等の**書き込み**先はデータディレクトリ配下を拒否し続ける
- protocol: `pin_configured` が無い v44 で round-trip する

## 6. ドキュメント

- plan **§3.2** を更新する。PIN の書き手が本体であること、認証ファイルの場所、
  ログの場所、`--set-pin` 撤去、不変条件の言い換え (書き込み禁止 / 読み取り可)
- plan に **§14.14** (または次の空き番号) を追加し、この増分の決定と理由を記録する。
  「未リリースにつき移行不要」と判断した旨も残す
- plan **§13.5** の protocol 版数を 44 に更新する
- マニュアルはまだ書かない (`tailscale serve` の導線が決まってから「準備する」を書く)

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する
- 実機で見るべき箇所を列挙する
