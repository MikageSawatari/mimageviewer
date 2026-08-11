# リモート閲覧: 返ってきたページが要求したページであることを検査可能にする

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 観測された失敗

利用者報告 (2026-08-08): リモートで PDF を閲覧中、**2 ページ目に、その直前に開いていた
別の PDF のページ (おそらく同じ 2 ページ目) が表示された**。再現性は低く、再起動後は
再現せず、本体単体でも再現していない。

**原因は未特定。原因の推測に基づく修正を入れないこと。** この brief で入れるのは、
同じ事象が次に起きたときに「どこで取り違えたか」が確定し、かつ**別文書のページを黙って
表示しない**ようにするための検査である。

## 2. 調査済みの事実 (再調査不要)

以下はすべて文書 (PDF ファイルパス) を識別に含んでおり、静的な key 衝突は起こさない。

- クライアントの `pageResourceCache` の key は `mediaImageInfoKey(address)` を含む。
  これは `addressIdentity()` = favorite_id + relative_path + subresource kind + page_number。
  [app.js `addressIdentity` / `mediaImageInfoKey`]
- 要求 URL も `fav` + `path` + `page` を含む。異なる PDF は異なる URL になるので、
  ブラウザ HTTP cache でも混ざらない。[app.js `addressQueryParams`]
- server の `RemoteCompositeCacheKey.page_key` は `page_key_for_remote()` 由来で、
  PDF は `zip_entry_key(pdf_path, "page_N")`。文書パスを含む。
  [src/edit_source.rs `page_key_for_remote` / `page_key_for_pdf`]
- full page 要求は `skip_cache: full_page` でサムネイル catalog を経由しない。
  [src/remote_ipc/container.rs `load_image`]

したがって残る候補は、**PDF レンダ経路 (PdfWorkerPool / PDFium) か、応答の取り違え**である。
再現性が低いことから、静的な key 誤りではなく競合の可能性が高い。

## 3. 入れるもの

### 3.1 応答に「実際に描いたページ」の identity を載せる

**identity は、画素を作った場所と同じ場所から出すこと。** HTTP 層で要求をそのまま
echo すると何も検査したことにならない。core 側で、実際に render に使った
`resolved.logical` と `subresource` から identity を作り、応答に含める。

- core → remote-web の IPC 応答に、解決済みページ identity を足す。
  最低限 favorite_id / relative_path / subresource 種別 / PDF page number または
  ZIP entry_name を再構成できること。
- `remote-web` は `/api/page` の応答ヘッダとして返す。パスはクライアントが送った値なので
  新たな情報露出にはならないが、**PIN / Bearer token / session ID は載せない**。
- 既存の secret redaction 経路を迂回しないこと。

### 3.2 クライアントは受け取った identity を検査する

- 要求した address と応答の identity が一致しない場合、**その画像を表示しない**。
- 表示せずにエラーとして扱い、telemetry に要求側と応答側の identity を両方記録する
  (通常段でよい。session ID は生で書かない)。
- **再取得・リトライ・別ページへの読み替えを行わないこと。** 取り違えを隠す方向の
  復旧処理は入れない。次に起きたときに記録が残り、利用者にも分かることが目的。
- 一致した場合は現在と同じ経路で表示する。追加の遅延を入れない。

### 3.3 対象範囲

- `/api/page` の通常ページと見開きの各ページ。
- AI result の適用経路も同じ identity を持つなら同様に検査する。持たないなら今回は対象外とし、
  対象外である旨をコード comment に残す。
- サムネイル (`/api/thumb` 等) は対象外。

## 4. やってはいけないこと

- 原因の推測に基づく修正 (PdfWorkerPool の待ち方変更、cache の一括 clear、
  ページ取得前の sleep / retry、cache 無効化)。**この brief は検査だけを入れる。**
- HTTP 層で要求を echo して identity とすること (§3.1)。
- 取り違えを検出したときの自動再取得 (§3.2)。
- `/stream/`, `/api/ai/jobs`, `/api/video/*` の認証・fail-closed guard を弱めること。
- URL に秘密情報を載せること。

## 5. テスト

- identity の生成と比較を純関数として切り出し、単体テストを付ける。最低限:
  - 同じ PDF の同じページ → 一致
  - 同じページ番号で文書が違う → 不一致
  - ZIP entry / 通常ファイルでも同様に区別できる
- クライアント側で、不一致の応答を渡したときに画像が表示されず、telemetry に
  両方の identity が載ることを検証する。
- 既存の web テスト 214 件と Rust テストを維持すること。

## 6. 確認

- web テスト一式と、変更した Rust crate のテストが通ること。
- `cargo fmt --check` が通ること。
- `git diff --check` が通ること。
- **ビルドとコミットは行わない。** 変更ファイルと追加テストの一覧を報告する。
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない。
