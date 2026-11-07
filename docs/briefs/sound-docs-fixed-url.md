# 申し送り: mIV Sound 説明書を版番号なしの固定 URL にする (2026-11-08)

> 起票: ClaudeCode Opus 5.5 (サイト点検セッション)。宛先: メインの開発セッション。
> **利用者の決定 (2026-11-08): mIV Sound 説明書の URL から版番号を外し、固定 URL にする。**
> 前段の経緯は [effetune-old-docs-removal.md](effetune-old-docs-removal.md) (2026-10-12、旧版を残さない方針)。

## 1. 現状と問題

- 説明書は `htdocs/mimageviewer/manual/sound/v0.15.0-miv.7/` に 133 HTML + 4 画像が生成されている
  (`scripts/gen-effetune-docs.py`)。各ページの canonical もこの版付き URL。
  `manual/sound/index.html` は `noindex` + meta refresh で版付きの index へ飛ばす入口。
- 版を上げるたびに URL 全体が変わる。v4.3.0 以降の URL の移り変わり:
  `effetune/v0.12.0/` → `v0.13.0/` → `v0.14.0/` → `v0.15.0/` → `sound/v0.15.0-miv.2/` … `miv.7/`。
  旧 URL はすべて 301 で現行版へ送っている (`.htaccess` の生成ブロック) ので、利用者がリンク切れに当たることはない。
- **Search Console では、Google の切り替えが版の更新に追いつかない**
  (2026-11-08 点検、/mimageviewer/ 配下・28 日、11/04 まで):

  | Google が表示した URL | ページ数 | クリック / 表示 |
  |---|---|---|
  | `effetune/v0.12.0/` (10/12 から 301) | 85 | 17 / 556 |
  | `effetune/v0.13.0/` | 47 | 11 / 277 |
  | `effetune/v0.14.0/` | 15 | 7 / 73 |
  | `effetune/v0.15.0/` | 16 | 0 / 108 |
  | `sound/...` | 0 | 0 / 0 |

  Google がようやく v0.15.0 に移り始めたところで、v5.0.0 で `sound/v0.15.0-miv.7/` へまた移った。
  301 は評価を引き継ぐが、Google が旧 URL を巡回し直すまで数週間かかるので、版の更新が速いと常に古い URL が出続ける。
- 版付き URL に依存しているのは生成器・その出力・テストと、製品マニュアルの 2 リンクだけ:
  - アプリ本体 (`src` / `crates`) と `config` は版付き URL を持っていない (`git grep 'manual/sound\|manual/effetune' -- src crates config` が 0 件)。
  - `docs/effetune-docs-overrides/*.json` が参照するのは `manual/effetune.html` (版なし) だけ。
  - `htdocs/mimageviewer/manual/effetune.html` に `sound/v0.15.0-miv.7/README.html` と
    `sound/v0.15.0-miv.7/visualizer.html#miv-counter` の 2 本。`manual/index.html` などは `sound/index.html` を指しているのでそのまま使える。

## 2. 目標の形

- 説明書の正規 URL は **`https://mikage.to/mimageviewer/manual/sound/<ページ名>.html`** (画像は `manual/sound/images/`)。
  `manual/sound/index.html` は入口の転送ページではなく、説明書の目次そのものにする (今の版付き `index.html` の中身)。
- **版を上げても URL は変わらない**。中身だけを書き換える。どの版の説明かは URL ではなく本文 (例: フッターの
  「mIV Sound v0.15.0-miv.7 の説明」) で示す。表示するかどうか・書き方はお任せ。
- 旧 URL は **1 回の 301** で固定 URL の同名ページへ。対象は `manual/effetune/<版>/X`、`manual/effetune/` 入口、
  `manual/sound/<版>/X` (今の `v0.15.0-miv.7` を含むすべての版付き URL)。同名のページが無ければ `manual/sound/index.html` へ。
  - 今の生成ブロックは現行版の名前とファイル一覧を規則に埋め込んでいるので、版を上げるたびに `.htaccess` が書き換わる。
    固定 URL にすれば規則から版番号が消える。ファイル一覧の扱いと合わせて、版の更新で `.htaccess` を触らずに済む形にできると良い
    (例: 「版付きパス → 固定パスの同名」を一律に送り、無いページは `RewriteCond %{REQUEST_FILENAME} !-f` などで index へ)。
    具体的な規則はお任せ。**ループしないこと**と、www 経由が 2 ホップで着地することは確認する。
- 将来の版でページが**無くなった**場合も、固定 URL の旧ページ名は 404 ではなく `manual/sound/index.html` へ送る
  (今の「無いページは入口へ」と同じ扱い)。

## 3. 作業

1. `scripts/gen-effetune-docs.py`: 出力先を `manual/sound/` 直下にする。canonical・ページ内の URL・
   相対リンクの深さを直す (例: ナビの `../../effetune.html` → `../effetune.html`、`../../index.html` → `../index.html`)。
   入口ページ (`entry_page`) は不要になる。生成ブロック (`redirect_block`) を上の形に変える。
   `--check` の「残った版ディレクトリを拒否する」は、`manual/sound/v*/` が 1 つでも残っていたら拒否、に読み替える。
   生成物と無関係なファイルが `manual/sound/` 直下にあれば拒否する、という今の安全策は残す。
2. `htdocs/mimageviewer/manual/sound/v0.15.0-miv.7/` を削除し、`manual/sound/` 直下に作り直す。
3. `htdocs/mimageviewer/manual/effetune.html` の 2 リンクを `sound/README.html`、`sound/visualizer.html#miv-counter` に直す。
   ほかに版付きパスを指すリンクが無いことを `git grep -n 'sound/v0\.' -- htdocs` で確かめる。
4. `scripts/test_gen_effetune_docs.py`: 版付きパスを前提にした検査 (181・215・390・406〜429・507 行付近) を固定パスに直し、
   「版を変えて生成しても URL が変わらない」「旧版付き URL・旧 effetune URL が 1 回で固定 URL へ届く」ことを検査に加える。
5. 手順書の更新: `docs/effetune-integration-plan.md` (420・785〜790 行付近の「`manual/sound/<v付きVERSION>/` に生成」)、
   `docs/effetune-fork-plan.md` の該当節、`docs/release-checklist.md` 9 行目・`docs/release-operations.md` 16 行目の
   「旧生成物の削除と旧 URL からの 301」の記述を、版を上げても URL が変わらない運用に合わせる。
6. htdocs をコミットした後に `python scripts/gen-sitemap-xml.py` (lastmod が git 日付のため)。
   sitemap の `manual/sound/` は 133 件のまま、パスから版番号が消えるはず。版付きの URL が 0 件であることを確かめる。

## 4. 公開

- 説明の中身は今公開中の v5.0.0 と同じなので、**次のリリースを待たずに公開してよい**。ただし公開前に
  `git diff v5.0.0 master --stat -- htdocs` で、今回の変更以外の未リリース分が混ざっていないかを確かめる。
  混ざっていれば今回分だけを公開する方法を利用者と相談する。
- 反映は `.\scripts\deploy-site.ps1` (差分確認) → `-Apply`。直下の `sitemap.xml` は scp で上書きする
  (tar 展開では「Cannot open: File exists」になる。2026-11-07 の記録)。
- `deploy-site.ps1` はサーバにだけあるファイルを消さない。サーバに残る `manual/sound/v0.15.0-miv.7/` などは 301 が先に効くので
  見た目には影響しない。消すなら利用者に確認してから。

## 5. 公開後の確認 (サイト点検セッションで引き受けられる)

- `curl -sIL` で次がすべて 1 回の 301 (www 経由は 2 回) で固定 URL の 200 に着くこと:
  `manual/sound/v0.15.0-miv.7/eq-15band-peq.html`、`manual/effetune/v0.12.0/eq-15band-peq.html`、
  `manual/effetune/v0.15.0/`、`manual/effetune/`、存在しないページ名。
- `manual/sound/eq-15band-peq.html` と `manual/sound/index.html` が転送なしの 200 で、canonical が自分自身であること。
- 公開中の sitemap に `sound/v` も `effetune/v` も 0 件であること。
- Search Console でサイトマップを再送信し、固定 URL の目次と主なページの登録をリクエストする (利用者の了承を得てから)。
