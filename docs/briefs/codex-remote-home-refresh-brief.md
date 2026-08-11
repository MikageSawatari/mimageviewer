# リモート閲覧: 本体側の変更が「場所」「スマートフォルダ」へ反映されない

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 観測された失敗

本体のフォルダバー設定メニュー「場所▼に出す項目」で各ドライブ等をオフにしても、リモートの
「場所」から消えない。**リモートを再読み込みすると反映される**ことを利用者が実機で確認済み。

## 2. 調査済みの原因 (再調査不要)

| 層 | 状態 |
| --- | --- |
| 設定の保存 | 正しい。変更時に `settings.save()` される |
| 世代の更新 | 正しい。`remote_state_generation` は settings.db の `data_version` 由来なので、この設定でも上がる |
| サーバの応答 | 正しい。`CollectionSettingsSource::Live` が要求のたび設定を読み直す |
| **クライアント** | **ここが原因。`/api/home` を起動時に 1 回しか取得しない** |

[crates/remote-web/web/app.js](../crates/remote-web/web/app.js) の `applyRemoteStateGeneration` は
世代が変わったとき `refreshRemoteFavorites()` を呼び、**`/api/favorites` だけ**を取り直す。
`places` と `smart_folders` を持つ **`/api/home` は再取得されない**。`/api/home` の取得は
起動時の 1 か所だけ (`state.home = await apiJson("/api/home")`)。

**これは以前からある穴。** 従来「場所」は読書履歴・レーティング・本棚・ブックマークの固定
4 種で設定に依存しなかったため表面化しなかった。ドライブ等を設定連動にしたことで見えた。

**同じ理由でスマートフォルダの定義も反映されない。** 本体で追加・改名しても、リモートを
再読み込みするまで出ない。根は 1 つ。

## 3. 直し方

**`/api/home` を呼ぶ行を 1 つ足すだけにしない。**

形は「起動時に取る home 画面のデータが 2 本 (`/api/favorites` と `/api/home`) あり、
片方だけが無効化に繋がっている」。次に増えた API も同じ確率で漏れる。

- **世代が変わったときに取り直す対象を 1 か所で決める**構造にし、両方をそこへ載せる
- その 1 か所が正本であること、home 画面のデータを増やすときはここへ足すことを
  コメントに残す

## 4. 守ること

- **失敗しても既存の一覧を消さないこと。** 起動時の失敗経路は
  `state.home = { places: [], smart_folders: [] }` で空にしている。再取得でこの経路を
  そのまま使うと、一時的な通信失敗で利用者の「場所」が空になる。再取得の失敗は
  **前の内容を保つ**
- **重複要求を出さないこと。** `refreshRemoteFavorites` は promise を握って同時実行を
  防いでいる (`remoteFavoritesRefreshPromise`)。同じ扱いにする
- **利用者の現在の画面を奪わないこと。** ビューアで閲覧中に世代が変わっても、画面を
  home へ戻したりスクロール位置を失わせたりしない。表示中の一覧は更新されてよい
- 認証・セッション・秘密の扱いを変えないこと

## 5. テスト

- 世代が変わったとき `/api/home` が取り直されること
- 世代が連続で変わっても要求が重複しないこと
- 再取得が失敗したとき、**前の `places` / `smart_folders` が残ること**
- ビューア表示中に世代が変わっても画面が奪われないこと
- 既存の `/api/favorites` 再取得が従来どおり動くこと (回帰)

## 6. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- `cargo check` の警告が増えていないこと
- ビルドとコミットは行わない。`htdocs/` は触らない
