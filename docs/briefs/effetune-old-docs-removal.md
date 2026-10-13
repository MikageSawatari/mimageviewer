# 申し送り: EffeTune 説明ページの旧版 (v0.12.0) を撤去する (2026-10-12)

> 起票: ClaudeCode Opus 5.5 (サイト点検セッション)。宛先: メインの開発セッション。
> 利用者の決定 (2026-10-12): **EffeTune の説明ページは現在の版だけを残し、旧版は残さない。**

## 1. 現状 (v4.6.0 公開時点)

- `htdocs/mimageviewer/manual/effetune/` に `v0.12.0/` と `v0.13.0/` が並んでいる。
  v4.6.0 は Mixwright v0.13.0 を同梱しているので、v0.12.0 は不要になった旧版。
- 両版のページはほぼ同じ内容 (例: `eq-15band-peq.html` は版番号以外の差が 4 行)。
  **どちらのページも自分自身を canonical にしていて、両方が `htdocs/sitemap.xml` に載っている**
  (sitemap 343 URL のうち v0.12.0 が 131、v0.13.0 が 133)。Google から見ると同じ内容のページが 2 組あり、
  どちらを検索に出すかを Google 任せにしている。
- ファイル一覧の比較: v0.13.0 は v0.12.0 の**全ファイルを含み**、2 ページ増えている
  (`others-sfz-note-player.html` / `resonator-adaptive-prediction.html`)。
  つまり v0.12.0 の URL はすべて、同じファイル名の v0.13.0 へ 1 対 1 で移せる。
- 旧版 URL は既に Google に登録済みで、検索の表示も出ている
  (例: `effetune/v0.12.0/reverb-dattorro-plate-reverb.html`、GSC 2026-10 時点)。
- アプリ本体は版付きの説明ページ URL を持っていない (`git grep 'manual/effetune' -- src crates` が 0 件。
  v4.5.0 タグも同じ)。外から入ってくるのは検索と外部リンクだけ。
- 生成スクリプト `scripts/gen-effetune-docs.py` は `effetune/<現在の版>/` と入口の `effetune/index.html` だけを書く。
  **他の版のフォルダは消さない**ので、版が上がるたびに旧版が残る作りになっている (今回の原因)。

## 2. やってほしいこと

1. **旧版フォルダを削除する**: `htdocs/mimageviewer/manual/effetune/v0.12.0/` を丸ごと消す (html と images/)。
2. **旧版 URL から新版への 301 を置く**: 登録済みの旧版 URL の評価を新版へ移し、外部リンクを 404 にしないため。
   置き場所は既存の `htdocs/mimageviewer/.htaccess` (www 統一の 301 がある。パターンはこのディレクトリからの相対パス)。例:

   ```apache
   # EffeTune の説明ページは現在の版だけを公開する。旧版の URL は同じページ名の現在版へ。
   RewriteRule ^manual/effetune/v0\.12\.0/(.*)$ https://mikage.to/mimageviewer/manual/effetune/v0.13.0/$1 [R=301,L]
   ```

   - `<IfModule mod_rewrite.c>` の中、www の規則の後に置けばよい (www の規則が先に終わっても 2 ホップで着地する)。
   - `.htaccess` は `.gitattributes` で `eol=lf` 指定済み。CRLF にすると Apache が 500 を返し得る。
3. **再発防止 (設計はお任せ)**: 次に Mixwright の版を上げたときに同じことが起きないようにする。案:
   - `gen-effetune-docs.py` が現在の版以外の `effetune/v*/` を削除する (または `--check` で旧版の残存を失敗にする)。
   - 301 の規則を「現在の版以外の `effetune/v<版>/` はすべて現在版へ」という形にして、版の更新時に 1 か所だけ直せばよいようにする。
     新しい版でページが無くなった場合に 404 へ飛ばさないよう、存在しないページは現在版の `index.html` へ送る、などの扱いも検討する。
   - どちらにしても、版を上げる手順 (docs/effetune-integration-plan.md の該当節、必要ならリリース手順) に 1 行足す。
4. **sitemap を作り直す**: 上をコミットした後に `python scripts/gen-sitemap-xml.py` (lastmod が git 日付なのでコミット後)。
   v0.12.0 の 131 URL が消えて 212 URL 前後になるはず。
5. テスト: `python -m unittest scripts/test_gen_effetune_docs.py` など、生成スクリプトに手を入れた場合は既存テストを通す。
   (`entry_page('v0.12.0')` を使うテストは文字列生成だけなので、フォルダ削除の影響は受けない見込み。)

## 3. 公開 (mikage.to への反映) の注意

- この変更は**今公開中の v4.6.0 の内容と矛盾しない**ので、次のリリースを待たずに公開してよい
  (利用者判断。待つ理由は特にない)。
- ただし master に未リリースの htdocs 変更が混ざってからだと、sitemap.xml にその分の lastmod や新ページが入る。
  アップロード前に `git diff v4.6.0 master --stat -- htdocs` で、差分が今回の変更 (.htaccess / sitemap.xml / 旧版フォルダ削除) だけであることを確かめる。
  混ざっている場合は、今回分だけを公開する方法を利用者と相談する。
- **アップロードではサーバ上のファイルは消えない**。利用者に、サーバの
  `/mimageviewer/manual/effetune/v0.12.0/` フォルダの削除と、`.htaccess` / `sitemap.xml` のアップロードを依頼する。
  (フォルダが残っていても 301 が先に効くので見た目は同じだが、残骸を残さないため。)

## 4. 公開後の確認 (サイト点検セッションでも引き受けられる)

- `curl -sI https://mikage.to/mimageviewer/manual/effetune/v0.12.0/eq-15band-peq.html` が 301 で
  `.../effetune/v0.13.0/eq-15band-peq.html` を指し、その先が 200。
- `https://www.mikage.to/mimageviewer/manual/effetune/v0.12.0/...` も最終的に v0.13.0 の 200 に着地する。
- 公開中の sitemap.xml に `v0.12.0` が 0 件。
- Search Console でサイトマップを再送信する (利用者の了承を得てから)。旧版 URL は数週間かけて
  「ページにリダイレクトがあります」へ移っていく。

## 5. 結果 (2026-10-12)

- 実施: fae7f3a81 (旧版削除・生成スクリプトが .htaccess の転送ブロックを所有・--check)、238ea96cc (sitemap 212 URL)。
- 公開: 利用者が .htaccess / sitemap.xml をアップロードし、サーバの v0.12.0 フォルダを削除。coordinator が公開後に確認 (旧版ページ・www・版フォルダ・存在しないページ・画像の 301 → v0.13.0 の 200、sitemap 212 URL / v0.12.0 0 件)。利用者が Search Console でサイトマップを再送信。
