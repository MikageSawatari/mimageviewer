# リモート閲覧: パスの制限をやめ、mIV 本体と同じ範囲にする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 決定と理由

**リモートから読める範囲の制限をやめる。mIV 本体が開けるものは、リモートからも開ける。**

利用者判断。理由:

- **保証できない制限を保護として提示するほうが危険。** 現状の検証水準で「お気に入りの外は
  読めません」を保証しきれない。にもかかわらずそう表示すると、利用者は検証されていない
  保証を信じて判断することになる
- 実際、この制限の実装で出た不具合 2 件は**どちらも攻撃を防がず、正当なアクセスを壊した**
  (お気に入り 1 つの不在で全件消える / Pictures 取得失敗で fallback に落ちない)。
  守る側として働いた実績が無い
- 到達には **tailnet 内の端末であること + PIN** の 2 つが要る (待受は 127.0.0.1、外部からは
  `tailscale serve` 経由のみ)。この前提で残る露出に対し、制限の複雑さが見合わない
- **代わりに、有効化の時点で正直に伝える** (§4)
- 副次的に、ホームの「場所」など**すべての一覧が本体と完全に一致**し、特別扱いが消える

**モードを分けない。** 2 モードにすると制限側は結局正しくなければならず、複雑さが減らない。

## 2. 何を削除するか

**中途半端に残さない。効かない制限を置いておくのが一番危ない。**

- クレート `crates/remote-registered-roots/` を丸ごと削除し、workspace から外す
  - ただし `pictures_root()` は `src/capture.rs` が使っている。**本体側へ戻す**
    (元は `capture.rs` にあった。`books::default_books_root()` が
    `capture::default_output_dir().join("books")` である関係は維持する)
  - `reading_history_db` の上限定数も本体側の定義へ戻す
  - `adjustment_db::normalize_path` も本体側の実装へ戻す
- `src/remote_ipc/path_guard.rs` の**お気に入り相対の解決**を削除する
  (`resolve_existing` / `map_existing_to_favorite` / `canonicalize_within` の
  root 判定 / `ResolvedFavoritePath` の root 系フィールド)
- `src/remote_ipc/live_favorites.rs` の allowlist 用途を削除する。
  **お気に入りの一覧そのもの (ホームの「お気に入り」タブ) は残す** — 入口としては有用で、
  アクセス境界ではなくなるだけ
- `crates/remote-web/src/store.rs` の二重検査を削除する
  (`retain_allowed_remote_entries` / `retain_allowed_folder_list_entries` /
  `validate_remote_address*` の allowlist 部分)
- 一覧の写像で**候補を落とす処理を削除**する。落ちる候補が無くなる

## 3. 住所の形

`RemoteAddress` を **絶対パス + subresource** にする。

    pub struct RemoteAddress {
        /// 対象の絶対パス。
        pub path: String,
        pub subresource: RemoteSubresource,
    }

- `root_id` / `relative_path` を廃止する (直前の増分で `favorite_id` → `root_id` に
  改名した箇所が、そのままパスへ変わる)
- **HTTP ではクエリ引数で渡す** (`?path=...`)。HTTP ログは記録前にクエリ文字列を落とす
  ([crates/remote-web/src/diagnostics.rs](../crates/remote-web/src/diagnostics.rs) の
  `request_path`) ので、**パスはログに残らない**。この性質を壊さないこと
- ブラウザの route (hash) にはパスが入る。利用者自身の端末の履歴に出るだけなので許容する
- **検証は残す**: `\0` を含まない / 絶対パスであること / 実在すること。
  正規化 (`canonicalize`) はキーの一貫性のために従来どおり行う
- 相対パスの traversal 検証 (`..` 等) は、相対パスが無くなるので不要になる。
  ただし **subresource (ZIP entry / ZIP prefix) の検証は残す**

## 4. 有効化時の説明 (必須)

制限をやめる判断は「利用者が正しく認識していること」を前提にしている。**認識させる部分が
実装の一部**になる。

- 場所は**接続ダイアログ** ([src/remote_ipc/ui.rs](../src/remote_ipc/ui.rs) の
  `show_remote_connection_dialog`。チェックを入れて OK で `remote_service_enabled` が立つ)
- **有効にする操作の前に見える位置**へ置く。OK の後に出す形にしない
- 文面は利用者と確定済み。次をそのまま使う:

  > リモート閲覧を有効にすると、**mIV で閲覧できるすべてのファイル**が、この PC の
  > Tailscale アドレスへ接続でき、PIN を知っている人から見えるようになります。
  > 対象はお気に入りの中だけではなく、mIV が開ける画像・動画・PDF すべてです。

- **一度きりのバナーにしない。** 有効にしようとするたび見えること
- 無効化のときは出さない
- 説明の下に**マニュアルへのリンク**を置く。クリックで既定ブラウザが開くこと
  (`ui.hyperlink_to`)。リンク先は
  `https://mikage.to/mimageviewer/manual/tut-remote.html`。
  **ページ本体はこの増分では作らない** (ClaudeCode が別途書く)。
  リンクのラベルは「詳しい説明を見る」等、押すと外部ブラウザが開くと分かる表現にする

## 5. 残すもの (弱めない)

- PIN 認証、失敗時のロックアウト、セッションの排他
- `/stream/` `/api/ai/jobs` `/api/video/*` の認証と fail-closed
- **PIN と Bearer token をどの記録層にも出さない**。session ID も生値では出さない
- Service Worker が API 応答・画像・サムネイルをキャッシュしない
- 待受は 127.0.0.1 のまま

## 6. ドキュメント

`docs/web-remote-plan.md` §3.1 の不変条件を**書き直す**。現在の記述

> お気に入りに登録されていない場所は、いかなる方法でも読めないこと

は無効になる。新しい記述に次を含めること:

1. リモートから読める範囲は **mIV 本体と同じ**であること
2. その判断の理由 (§1)。特に「保証できない制限を保護として提示しない」
3. 到達には tailnet 内であること + PIN が要ること
4. 有効化時に利用者へ明示すること (§4) が、この判断の前提であること

## 7. やってはいけないこと

- 制限を「効かない状態で」コードに残すこと
- 認証・セッション・秘密の扱いを弱めること (§5)
- パスをクエリ文字列以外の場所 (URL path、ログの details) に出すこと
- 説明を出さずに有効化できる経路を残すこと
- 削除に伴って**お気に入りタブ / スマートフォルダ / 場所** の見た目や操作を変えること
  (範囲が広がるだけで、画面の構成は変えない)

## 8. テスト

- お気に入りの外のフォルダ / 画像 / PDF / 動画が列挙でき、開けること
- お気に入りの中は従来どおり開けること
- 実在しないパス、`\0` を含むパス、相対パスが拒否されること
- ZIP entry / ZIP prefix の検証が従来どおり効くこと
- **クエリ文字列がログに出ないこと** (既存の `request_path` のテストを維持 / 追加)
- 認証なしの要求が従来どおり拒否されること
- 有効化ダイアログに説明が出ること (文言の存在を固定する)
- 既存の web テスト 247 件を維持すること

## 9. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- **ビルドとコミットは行わない**
- 削除したファイル / クレートと、§2 で本体側へ戻したものを明記する
- **`htdocs/` と `README.md` は触らない**
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない
