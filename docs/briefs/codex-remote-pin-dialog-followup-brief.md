# リモート接続ダイアログ — PIN を ASCII に限定し、設定を即時反映にする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。直前のコミット `733faa85`
(PIN の所有を本体へ移した増分) への追補。実機確認で出た 2 点を直す。

## 0. 前提 — 先に読むもの

- `crates/remote-ipc/src/auth.rs` — `validate_pin`
- `src/remote_ipc/ui.rs` — 接続ダイアログ (`show_remote_connection_dialog`、
  `RemoteConnectionDialogState`、`RemoteConnectionDialogOutcome`、
  `remote_connection_dialog_outcome`、`remote_enable_warning_visible`、`remote_enabled_choice`)
- `src/ui_dialogs/favorites_editor.rs` の 125-140 行 — **即時反映設計のコメントと構造**。
  今回はこの形に合わせる
- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§3.1** (有効化前に対象範囲を明示する前提)、
  **§3.2**、**§14.14**

## 1. PIN を印字可能 ASCII に限定する

### 1.1 理由 (両方とも記録すること)

1. **入力欄は伏字なので、IME 変換が正しく確定したか利用者が確認できない** (実機確認での指摘)。
   日本語パスフレーズは入力できてしまうが、合っているか見て確かめられない
2. **正規化の不一致で、同じに見える文字列がハッシュ一致しない**。PIN を入力するのは
   iPad などのブラウザで、設定するのは Windows の本体である。合成済み / 分解済みの
   Unicode 正規化形が異なると、画面上は同じでも Argon2id の照合は失敗する。伏字のため
   利用者はこれを誤入力と区別できず、原因の分からない認証失敗になる

### 1.2 規則

`validate_pin` (共有クレート = 唯一の正本) で **U+0021..U+007E の印字可能 ASCII だけ**を
許可する。**空白 (U+0020) も許可しない** — 伏字では末尾や連続の空白が見えず、
上と同じ「確認できない失敗」を作るため。文字数の下限 6 / 上限は現行のまま。

- エラーメッセージは弾いた理由が分かる文にする。**PIN そのものを混ぜない**
- ダイアログの hint text も規則が分かる表記へ更新する
- ブラウザ側の PIN 入力画面は今回変更しない (弾くのは本体の設定時で足りる)。
  変更しない判断を plan に 1 行残すこと

## 2. ダイアログを即時反映にする

### 2.1 いまの問題

有効化はチェックボックスを入れて OK を押し、**ダイアログを閉じて開き直さないと** QR も
URL も状態も見えない。利用者から「導線が分かりづらい」と指摘があった。

### 2.2 直し方

**`src/ui_dialogs/favorites_editor.rs` と同じ即時反映設計にする** (同ファイル 130 行の
コメント「即時反映設計: OK/Cancel を廃止し、編集はその場で反映する」と同じ方針)。

- **checkbox と OK / キャンセルを廃止**する。`RemoteConnectionDialogOutcome` の
  Apply / Discard / Keep という保留モデルもやめ、ダイアログ状態から `enabled` を消す
  (残るのは PIN editor の状態だけ)
- **無効のとき**: 「リモート接続を有効にする」ボタン。押した時点で
  `settings.remote_service_enabled = true` → `save()` → `control.set_enabled(true)` まで走る
- **有効のとき**: 「リモート接続を無効にする」ボタン。押した時点で反映する
- **PIN 未設定のときは有効化ボタンを無効**にし、理由を隣に出す (現行の gate をボタンへ移す)。
  `remote_enabled_choice` の役割もここへ集約する
- 閉じる操作 (「閉じる」ボタン / × / Esc) は**閉じるだけ**で、適用も取り消しもしない
- 状態・利用状況・`tailscale serve`・PIN・QR・URL は**同じダイアログの中でその場で切り替わる**。
  有効化直後に「準備中」が出て、接続情報を受け取ると QR と URL が現れる既存の流れが、
  開き直さずに見えること

### 2.3 警告の出し方を変える

現行の `remote_enable_warning_visible(was_enabled, enabled)` は「チェックを入れてから OK を
押すまでの間」だけ警告を出す。即時反映では押した瞬間に確定するので、**押す前に見えている
必要がある**。

- **無効状態では警告を常に表示**し、有効化ボタンの**すぐ上**に置く (スクロールで
  離れない位置)。plan §3.1 の「有効化のたび、対象範囲を有効化前に認識できる」前提は、
  この形でも満たされる
- 有効状態では出さない (現行と同じく、無効化時には警告を出さない)
- 述語は「無効状態で表示する」意味へ書き換え、テストも合わせる。**1 クリックで確定する
  設計であることと、その代わりに警告を常時見せる**という判断を plan に残すこと

## 3. 触らないもの

- PIN の所有 (本体が書き、常駐プロセスは読むだけ)、保存場所、`--auth-file` / `--log` の受け渡し
- Argon2id のパラメータ、認証ファイル形式、protocol v44
- `tailscale serve` の導線 (別増分)
- 表示パイプライン (段階 3a / 3b / 3c)

## 4. テスト

```
cargo test -p mimageviewer-ipc
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
python scripts/check_ui_glyphs.py
```

- `validate_pin`: 日本語・全角英数・空白入りを拒否し、印字可能 ASCII を受け入れる。
  下限 / 上限は従来どおり。エラー文に PIN が入らない
- ダイアログ: PIN 未設定では有効化できない (既存テストを新しい形へ移す)。
  警告が無効状態で出て有効状態で出ないこと。保留状態を持たないこと
  (`enabled` フィールドが無い / Apply・Discard・Keep が無い) を型で固定する

## 5. ドキュメント

- plan **§3.2** に PIN の文字種制限と、その 2 つの理由 (伏字で確認できない / 正規化不一致) を書く
- plan **§14.14** に、ダイアログを即時反映へ変えたことと、警告を常時表示へ移した理由を追記する
- ブラウザ側の PIN 入力画面を変更しない判断も 1 行残す

## 6. 実行と報告

- §4 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所を全部、理由付きで報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- ブリーフと意図的に違えた点があれば、その理由を報告する
