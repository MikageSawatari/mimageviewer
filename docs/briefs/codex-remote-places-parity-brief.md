# リモート閲覧: 「場所」を本体と一致させる + 説明の色 + 警告の掃除

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

直前の増分 (`6cecec99` パスの制限撤去) の実機確認で出た指摘。

## 1. 「場所」に本体と同じ項目を出す (主目的)

利用者報告: 「まだ場所に各ドライブがなく、mIV 本体と一覧の内容が異なります」。

### 1.1 本体の正本

`src/ui_main.rs` の「場所▼」メニュー (`show_location_drive_list` の分岐から始まる箇所) が
正本。順序と表示条件は次のとおり。

| # | 項目 | 表示条件 | 出どころ |
| --- | --- | --- | --- |
| 1 | ドライブ一覧 | `show_location_drive_list` | 合成ビュー (`AddressBarNav::DriveList`) |
| 2 | 閲覧履歴 | `show_location_reading_history` | |
| 3 | ブックマーク | 常時 (本体に設定が無い) | |
| 4 | レーティング ▸ ★1〜5 | `show_location_rating` | |
| 5 | 本棚フォルダ | `show_location_bookshelf` | |
| — | (区切り) | | |
| 6 | デスクトップ / ピクチャ / ダウンロード | `show_location_desktop` / `_pictures` / `_downloads` | `known_folders::desktop_dir()` 等 |
| — | (区切り) | | |
| 7 | 各ドライブ | `show_location_drive_roots` | `known_folders::available_drives()` |

6 は `folder_tree::path_eq` で重複を除き、取得できなかったものは出さない。

### 1.2 リモートの現状と、やること

リモートは **2〜5 しか無い**。1・6・7 が欠けている。

- **6 と 7 を追加する。** どちらも実在する絶対パスなので、既存の `RemoteEntry` の
  フォルダとしてそのまま返せる
- **1 も追加する。** 本体では合成ビューだが、リモートでは `available_drives()` の各ドライブを
  項目に持つ collection として返せる。7 と内容が重なるが、本体に合わせる
- **表示条件は本体と同じ `show_location_*` 設定を読む。** リモート専用の判断を作らない。
  設定を切ると本体とリモートの両方から同時に消えること
- **順序と区切りも本体と同じ**にする
- 一覧の生成規則は本体側 1 か所に集約する。remote-web に別の列挙を持たせない
  (計画書 §10 の既存方針)

### 1.3 なぜ今できるか

これらを載せていなかったのは、お気に入り allowlist の外にあって開けなかったから。
計画書 §10 に「場所タブには読書履歴、レーティング ★1〜5、本棚、ブックマークだけを載せ、
ドライブ、デスクトップ、ピクチャ、ダウンロードは載せない」と明記されている。

**制限を外したので、載せない理由が消えた。** §10 のその記述も更新すること。

### 1.4 規模が合わない場合

1 (ドライブ一覧) の再現が別増分相当の規模になるなら、6 と 7 を先に入れ、1 は理由を添えて
報告してよい。6 と 7 は今回必ず入れること (利用者が指摘したのはここ)。

## 2. 有効化時の説明を赤系にする

利用者報告: 「今は黄色系なのでそこまで警告感がありません」。

`src/remote_ipc/ui.rs` の説明で使っている `ui.visuals().warn_fg_color` を
**`ui.visuals().error_fg_color`** に変える。`os_theme.rs` が Light / Dark 双方で赤系かつ
contrast 対応済みの値を持っている。

- **色を直接書かない。** `Color32::from_rgb` 等をこの箇所へ入れないこと
- 文面・強調・表示条件は変えない。既存の文面固定テストも維持する

## 3. 新しく出た dead_code 警告を消す

直前の増分でビルド警告が増えた。

- `REMOTE_ENABLE_WARNING_FIRST` (`src/remote_ipc/ui.rs`) — テストからしか使わないので
  `mod tests` の中へ移す。3 つの部分を連結すると確定文面に一致する、という検査は残すこと
- `CollectionEngine::new` / `new_with_favorites`、`CollectionSettingsSource::Snapshot`、
  `RemoteSortSettingsSource::Snapshot`、`LiveFavorites::snapshot`、`FavoritesSource::Snapshot`
  — テスト専用になったものは `#[cfg(test)]` にする

production から使うものが残っていた場合は消さず、なぜ残すのかをコメントに書くこと。
`#[allow(dead_code)]` で黙らせるのは最後の手段にし、使う場合は理由を書く。

## 4. やってはいけないこと

- 「場所」の項目をリモート専用の条件で出し分けること (本体の `show_location_*` に従う)
- 一覧の列挙規則を remote-web 側にも書くこと
- 説明の文面・表示条件を変えること
- 色をハードコードすること
- `htdocs/` を触ること (ClaudeCode がマニュアルを持っている)

## 5. テスト

- `show_location_*` の各設定で、リモートの「場所」の項目が本体と同じ集合・同じ順序になること
- 取得できない既知フォルダ (デスクトップ等) が出ないこと
- 重複するパスが 1 つにまとまること
- 追加した項目から実際にフォルダを開けること
- 文面固定テストが維持されていること

## 6. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- **`cargo build` の警告が直前の増分より増えていないこと**を確認して報告する
- ビルド (`build-dev.ps1` 等) とコミットは行わない
