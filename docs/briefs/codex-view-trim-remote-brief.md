# 段 2b-2: 表示トリムをリモートに載せる

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `515336b9`
(master マージ直後。**見開きレイアウトと表示トリムに触る master の変更が入っている**ので、
起点より古いコードを前提にしないこと)

## 0. 立場

**本体が正本。独自の規則を発明しない。**
以下は私が読んで確認した内容だが、**実際と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 18 回訂正されている。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 決まっていること — 本体側で切る

**`mimageviewer-core` が切ってから JPEG にする。端末側では切らない。**
利用者と相談して決めた。以下が根拠:

- リモートは見開きを合成せず、**ページを 1 枚ずつ送り、端末が横に並べている**
  ([command-core.mjs:474](../../crates/remote-web/web/command-core.mjs) の `viewerSpreadLayout`)。
  そして最終レイアウトは**デコード後の実寸から計算し直している**
  ([app.js:7345](../../crates/remote-web/web/app.js) 付近の `naturalWidth/naturalHeight`)。
  → 切った後の縦横比がそのまま届けば、**端末側のレイアウトは無改造で追随する**
- ZIP/PDF ページの `imageInfo` はサーバに寸法を問い合わせておらず、推定値を
  `dynamic: true` で返すだけ ([app.js:3442](../../crates/remote-web/web/app.js))。
  同期させるべき寸法 API が存在しない
- Auto の bbox は復号画素の走査が要る
  ([margin_fit.rs:247](../../src/margin_fit.rs) の `detect_content_bbox`)。
  画素を持っているのは本体だけなので、端末側で切っても本体の作業は減らず、
  通信 (protocol bump) と端末側実装が**上乗せになるだけ**

**この方針自体に構造的な問題を見つけたら、実装せずに報告してほしい。**

## 2. 本体側の解決順序 (正本。ここを写す)

`src/ui_view_trim.rs` にある。**この優先順位を変えない。**

1. `view_trim_base_apply_mode()` ([ui_view_trim.rs:824](../../src/ui_view_trim.rs))
   — 保存されたモード。ただし `Page` は基底としては無効で `None` に正規化される
2. `effective_view_trim_base_apply_mode()` ([ui_view_trim.rs:825](../../src/ui_view_trim.rs))
   — `None` かつ旧 margin_fit 設定 (`fullscreen_fit_mode == MarginFit` または
   `margin_fit_enabled`) が ON なら **`Auto` に昇格**。
   **これを落とすと、旧設定の利用者は本体で切れて端末で切れない**
3. `effective_view_trim_apply_mode_for_idx(idx)` ([ui_view_trim.rs:835](../../src/ui_view_trim.rs))
   — 基底が `Book` で、そのページに **enabled な**個別行があれば `Page` に昇格
4. bbox の解決:
   | モード | 出所 |
   |---|---|
   | `None` | なし |
   | `Auto` | `detect_content_bbox(pixels, DEFAULT_TOLERANCE)` |
   | `Book` | `view_trim_book_settings.single_bbox()` / `.spread_bbox(side)` |
   | `Page` | 個別行の bbox。**`spread_side` の左右変換あり** (`margins_for_spread_side`) |
5. **見開きの `Auto` のみ** `harmonize_spread_auto_bboxes(left, right)`
   ([view_trim.rs:277](../../src/view_trim.rs)) で左右をそろえる。
   `Book`/`Page` は保存値が既に側ごとなので、そろえ直さない

## 3. 差し込み口

`encode_remote_page_jpeg` ([container.rs:29](../../src/remote_ipc/container.rs)) が
縮小 → エンコードの単一の通り道。**縮小の手前**に crop を入れる。
表示トリムは正規化 bbox なので、`DynamicImage` の crop に落ちるはず。

**表示専用**であることを守る。export crop や補正 / AI パイプラインの出力には影響させない
([view_trim_db.rs](../../src/view_trim_db.rs) 冒頭に明記されている)。

## 4. 鍵は既にある

- 個別行は `view_trim_pages(page_path TEXT PRIMARY KEY)`。この `page_path` は
  `crate::edit_source::page_key_for_grid_item(item)` が作る `page_key`
  ([app.rs:49971](../../src/app.rs) の `page_path_key`)
- **リモートは既に同じ `page_key` を補正 / マスク / conceal で使っている**
  ([container.rs:467](../../src/remote_ipc/container.rs) 以降)。新しい鍵体系を作らないこと
- 本全体は `book_key(path) = crate::path_key::normalize(path)`

## 5. 段階

**2b-2a と 2b-2b を分けて出してほしい。** 依存の性質が違う:

- **2b-2a: `None` / `Book` / `Page`** — DB を読むだけ。画素走査なし、ページ間依存なし
- **2b-2b: `Auto`** — 画素走査 + 見開きのそろえ

2b-2a が緑になってから 2b-2b に進む。

## 6. 詰めてほしい点 (答えを持っていない)

1. **`ViewTrimDb` に read-only open が無い。** `open_at` は `Connection::open` +
   `CREATE TABLE` で、リモートから開くと**本体のデータに書き込みうる**。
   `crate::spread_db::SpreadDb::open_existing_read_only_at`
   ([container.rs:419](../../src/remote_ipc/container.rs) で使っている) と同じ形が要る。
   その形でよいか、判断して報告を

2. **見開き `Auto` は、片側を切るのに反対側の bbox が要る。**
   `/api/page` は 1 ページ 1 リクエストなので、左を処理する時点で右の画素が無い。
   端末は見開きの両ページをほぼ同時に要求する (`Promise.all`,
   [app.js:3262](../../crates/remote-web/web/app.js)) が、サーバ側では独立した job になる。
   **どう解くかを 2b-2b の着手前に報告してほしい** (相手ページを余分にデコードするのか、
   bbox を憶えるのか、他の手があるのか)。私は答えを持っていない

3. **端末からの ON/OFF をこの段でやるかどうか。** 利用者は
   「スマホで細かく調整する使い方はあまりなく、あっても自動トリムの ON/OFF くらい」
   と言っている。書き込みなので `RemoteWriteRequest` に載る話になるが、
   **2b-2a / 2b-2b の後の別段にする方が素直だと思う。** 判断して報告を

4. **キャッシュの無効化。** `/api/page` は既に `rev: pageRenderRevision` を持ち、
   端末のキャッシュ鍵にも入っている ([app.js:3400](../../crates/remote-web/web/app.js) /
   [3408](../../crates/remote-web/web/app.js))。補正機能が同じ仕組みを使っている。
   本体側の composite cache (`page_composite_cache`) の鍵に bbox が要るかも見てほしい

## 7. 受け入れ条件

- 本体で表示トリムを設定した本を端末で開くと、**本体と同じ位置で切れている**
- 見開きで左右が本体と同じにそろう
- モードごと (`None` / `Book` / `Page`、後段で `Auto`) に本体と一致する
- 旧 margin_fit 設定の利用者でも本体と一致する (§2-2)
- トリムが無い本・ページで**何も変わらない** (退行なし)
- export crop / 補正 / AI の出力が変わっていない
- 端末側の見開きレイアウト・拡大縮小が壊れていない
- **解決順序の unit test** を付ける。本体側のモード昇格 (§2-2, §2-3) と
  `spread_side` の左右変換を含めること
- `cargo test -p mimageviewer --lib` が緑。web に触るなら node --test も

## 8. 注意

- ビルド (build-dev.ps1 / build-release.ps1) とコミットはしない。テストは走らせてよい
- 原因は分かったが正しい修正が広範囲になる場合は、直さずに報告してほしい
- `/stream/`、`/api/ai/jobs`、`/api/video/*` の認証・fail-closed guard を弱めない
