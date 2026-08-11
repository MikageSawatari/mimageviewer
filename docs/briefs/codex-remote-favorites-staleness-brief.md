# mIV Remote: 本体で追加したお気に入りが端末から見えない

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `711ad54c` + 未コミットの作業差分

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。

**未コミットの差分が 2 系統載っている。どちらにも触らないこと。**
- 2b-2b (表示トリムの自動モード)
- AI 通知 / RTL シークバー (別ブリーフ)

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 症状 (利用者報告、実機)

本体でフォルダを**お気に入りに追加しても、端末側の一覧に出ない。**
本体を再起動するまで反映されない。

**これは実害が大きい。** リモートは外出先で使う機能なので、
**出先で気づいても直す手段が無い** (本体を再起動できない)。

## 2. 原因 (特定済み。ここは再確認しなくてよい)

保存側は正しい。**読む側が起動時のスナップショットを握ったまま**で、再読込の経路が無い。

- 追加時に即 `settings.save()` している
  ([fav_add.rs:141](../../src/ui_dialogs/fav_add.rs))。settings.db には入っている
- **remote-web**: `Library::load(&config.data_dir)` が **起動時に 1 回だけ**
  favorites テーブルを読み、`Vec<FavoriteRoot>` として保持する
  ([store.rs:130](../../crates/remote-web/src/store.rs) /
  呼び出しは [main.rs:57](../../crates/remote-web/src/main.rs) の 1 箇所のみ)
- **本体**: `RemoteIpcServer::start(saved.clone())` が **起動時に設定を複製**し、
  `run_native` が返るまで保持する ([lib.rs:1081](../../src/lib.rs))。
  `ContainerEngine.settings` / `CollectionEngine` などがこれを使う

役割が分かれているため、**片側だけ直すと中途半端になる**:

| | 一覧を出す | 開くときの経路検証 |
| --- | --- | --- |
| remote-web | `/api/favorites` → `state.library` | `validate_remote_address` ([store.rs:205](../../crates/remote-web/src/store.rs)) |
| 本体 | — | `self.settings.favorites` ([container.rs:1957](../../src/remote_ipc/container.rs) 他) |

remote-web だけ直すと **一覧には出るのに開けない**。両方が要る。

## 3. 既にある手本 — 補正設定は同じ問題を解いてある

`AdjustmentSettingsSource::Live` ([container.rs:603](../../src/remote_ipc/container.rs)) は
スナップショットを持たず、**使うたび settings_db から読み直している**:

```rust
AdjustmentSettingsSource::Live => {
    crate::settings_db::with_db_result(|db| db.load_adjustment_render_settings())
```

`with_db` は `GLOBAL_DB` の **開きっぱなしのハンドル**を使うので cold open ではない。

**お気に入りも同じ形にできるはず**だが、下の §4 の制約がある。
**この手本に乗せるのが妥当か、判断して報告してほしい。**

## 4. 制約 — 経路検証はリクエストごとに走る

`validate_remote_address` は **サムネイル 1 枚ごとに呼ばれる**。一覧を出すだけの
`/api/favorites` と違い、ここに DB 全読みを足すと**サムネイル一覧が重くなる**。

**「毎回読む」で本当に問題ないかを測って判断してほしい。** 重いなら安く鮮度を取る形が要る。
思いつく候補は挙げるが、**より良い形があればそちらを選んでほしい**:

- SQLite の `PRAGMA data_version` — 他の接続が commit すると値が変わる。
  I/O を伴わないので、変化したときだけ読み直す判定に使える
- settings.db の mtime を見る (WAL があるので `-wal` 側も要るかもしれない)
- 本体から remote へ変更を push する (session / IPC の既存経路に乗せる)

**選んだ理由と、測った数字を一緒に報告してほしい。**

## 5. 追加・削除だけでない

一覧に出るようになるだけでは足りない。以下も揃うこと:

- **削除**: 本体で外したお気に入りは、端末から**開けなくなる**こと。
  これは見た目の問題ではなく**経路検証の問題**なので、確実に落とすこと
- **改名 / 並べ替え**: 端末の一覧に反映されること
- **端末が古い favorite id を持っている場合**: 破綻せず、分かる形で失敗すること
  (端末はページを開いたまま、本体側で削除される、が起こり得る)

## 6. 受け入れ条件

- 本体でお気に入りを追加 → **端末を再読込すると一覧に出る** (本体の再起動なしで)
- そのお気に入りを端末から**開ける** (一覧に出るだけでは不可)
- 本体でお気に入りを削除 → 端末から**開けなくなる**
- 改名・並べ替えが端末の一覧に反映される
- **サムネイル一覧の表示が目立って遅くなっていない** (§4)
- 端末が保持していた古い favorite id で操作しても破綻しない
- **回帰テスト**を付ける。特に「削除したお気に入りの経路が拒否される」ことは
  security 側の不変条件なので必ず固定する
- `cargo test -p mimageviewer --lib` / remote / ipc / web が緑

## 7. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- **未コミットの 2 系統の差分に触らない** (§0)
- `/stream/`、`/api/ai/jobs`、`/api/video/*` の認証・fail-closed guard を弱めない
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい
