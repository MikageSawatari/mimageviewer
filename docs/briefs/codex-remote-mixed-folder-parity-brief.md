# 混在フォルダの扱いを本体に合わせる — 見開きの既定とシークバー

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。

## 0. 前提 — 先に読むもの

- [`docs/web-remote-plan.md`](../web-remote-plan.md) **§12.11** (通常フォルダの見開き)、**§12.4** (非スコープ)
- `src/app.rs` — `physical_page_order_locked` (6809 行付近)、`SpreadRestoreDefaults` (6821 行付近)、
  `page_order_locked_for_items` (27098 行付近)、`spread_restore_defaults` の決定 (21956 行付近)
- `src/ui_fullscreen.rs` — `fullscreen_mixed_media_summary` (9658 行付近)、`FsSeekInfo` (6580 行付近)、
  seek overlay の分岐 (9918 行付近)
- `src/remote_ipc/container.rs` — `spread_payload` (2912 行付近)、
  `recompute_folder_listing` の `physical_page_order_locked` 呼び出し (1690 行付近)

段階 3a / 3b の所有権 cutover とは**独立した増分**。coordinator、registry、heavy queue、
lease には触らない。

## 1. 実機で見つかった 2 つの不一致 (再調査不要)

利用者の iPad で、**普段は見開きにならないフォルダがリモートでは見開きになり、
本体ならシークバーが出ないフォルダでシークバーが出ていた**。どちらも本体側に規則があり、
リモートがその規則を参照していないことがコードで確認できている。

| 本体の規則 | 場所 | リモートの現状 |
|---|---|---|
| 保存値が無いときの見開き既定は、**本のときだけ** `settings.default_spread_mode`。本でなければ `SpreadRestoreDefaults::NON_BOOK` (単ページ / Paged / LTR) | `app.rs` 21956-21961、6828-6841 | `spread_payload` が `self.settings.default_spread_mode` を**無条件で**渡す |
| 「本」の判定は `physical_page_order_locked` = 本フォルダ直下 **または** コンテナとして開いた **または** (自動フルスクリーン設定 && items が空でなく全部 `GridItem::Image`) | `app.rs` 6809-6819 | 同じ述語を `recompute_folder_listing` の sort 用には計算しているのに、見開き既定には使っていない |
| シークバーは**全ての nav item が画像のときだけ**描く。そうでなければ種別ごとの件数サマリを描いて `return None` | `ui_fullscreen.rs` 9918-9930 | Web は `seekState.visible = count > 1` だけで判定していて、この規則を持たない |

**つまり動画・音声・サブフォルダ・ZIP が 1 つでも混ざれば、本体では「本ではない」**。
リモートも同じ結論になるようにする。

## 2. 変更内容

### 2.1 見開きの既定を本体の述語で決める

`spread_payload` が `resolve_spread_state` へ渡す既定値を、**本かどうかで分岐**させる。

- 本 → 現行どおり `settings.default_spread_mode` / `default_reading_direction`
- 本でない → 本体の `SpreadRestoreDefaults::NON_BOOK` と同じ **単ページ / LTR**

判定は**本体の述語をそのまま呼ぶ** (`crate::app::physical_page_order_locked`)。リモート側で
条件を書き直さないこと。同じ判定を 2 か所で表現すると、片方だけ直る形の不整合が必ず戻る。

**保存値があるときの挙動は変えない。** 変えるのは「保存が無いときの既定」だけである
(本体のコメントも「明示保存値はどちらでも優先する」と書いている)。

ZIP / PDF は `is_open_as_container` が真なので本のまま。**現行の見開き挙動が変わらないことを
テストで固定する** (ここが壊れると本を開くたびに単ページになる)。

### 2.2 シークバーの可否を本体の規則に合わせる

`ContainerPayload` に、本体の `FsSeekInfo` が持つのと同じ内訳を足す。

- 画像の件数、動画の件数、その他の件数 (本体の `image_indices.len()` / `video_count` /
  `other_count` に対応する値)

Web は「**全 nav item が画像か**」を本体と同じ式で判定し、真ならシークバー、偽なら
本体と同じ文面の件数サマリを出す。文面は `fullscreen_mixed_media_summary` と同じ順序・区切り
にする。

    画像 12 ファイル、動画 3 ファイル、その他 1 件

- 空の欄は出さない (本体と同じく `parts.push` を条件付きにする)
- 区切りは読点 `、`
- **本体の文面を別の言い回しに直さない。** 同じものが 2 つの画面で違う言い方をしていると、
  利用者はどちらかが壊れていると読む

判定と文面組み立ては純関数として `command-core.mjs` に置き、`app.js` からはそれを呼ぶ。

### 2.3 protocol

`ContainerPayload` にフィールドを足すので **`PROTOCOL_VERSION` を 42 → 43** へ上げる。
plan §13.5 の版数記述も更新する。本体と remote-web は両方再ビルドが要る。

## 3. アニメーションは非スコープと明記する (利用者判断、2026-08-11)

リモートは `/api/page` が必ず JPEG に焼くため、**アニメーション GIF / APNG / WebP は
静止画として表示される**。実機で「アニメーションしない」と報告があったが、これは退行では
なく元から未対応である (リモート経路にアニメーションを扱うコードは無い)。

**当面は非対応とする**ことを利用者が決めた。plan **§12.4 の非スコープ**へ 1 行加える。
対応する場合は元バイトを返す経路が要り、先読み予算・cache key・見開き合成の全てに影響する
ため、独立した増分にする — という理由も併記する。

## 4. 触らないもの

- 段階 3a / 3b で入れた coordinator / registry / heavy queue / lease / `PageDemand`
- 位置の requested / displayed 所有権 (段階 3c)
- 本体側の `physical_page_order_locked` と `fullscreen_mixed_media_summary` の**中身**
  (参照するだけ。規則を変える増分ではない)
- 保存済み見開き設定の読み書き規則、端末ローカルの `force_single_page`

## 5. テスト

```
cd crates/remote-web/web && node --test
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib remote_ipc::
cargo test -p mimageviewer-remote
```

**本体側**

- 画像だけのフォルダ (自動フルスクリーン設定 ON) は保存値が無くても既定の見開きになる
- **動画が 1 つ混ざったフォルダは、保存値が無ければ単ページになる**
- サブフォルダや ZIP が混ざった場合も同じ
- **保存値があるフォルダは、混在でもその保存値が勝つ**
- **ZIP / PDF は従来どおり本として既定の見開きになる** (退行防止)
- `ContainerPayload` の件数内訳が実際の item 構成と一致する

**Web**

- 全 nav item が画像ならシークバー、そうでなければ件数サマリ
- サマリの文面が本体と同じ (空の欄を出さない、区切りは `、`)
- 画像 0 件のときの扱いが本体と一致する

## 6. ドキュメント

- plan **§12.4** にアニメーション非対応を追記 (§3 の理由付き)
- plan に **§12.26** (または次の空き番号) を追加し、次を記録する
  - 混在フォルダの既定が本体と食い違っていた事実と、**本体の述語をそのまま呼ぶ**ことで
    直したこと (リモート側に判定を書き直さない)
  - シークバーの可否と件数サマリを本体規則へ合わせたこと
  - protocol 43 の内容。§13.5 も更新する

## 7. 実行と報告

- §5 のコマンドを**毎回実行**して結果を報告する
- **`src/` と `crates/` に触れた箇所は全部と、その理由を報告する**
- **`scripts/build-dev.ps1` を実行しない。コミットもしない**
- 実機で見るべき箇所を報告に列挙する
