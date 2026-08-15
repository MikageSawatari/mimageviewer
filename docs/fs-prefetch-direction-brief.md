# 実装ブリーフ: 先読み表示を前後に分けて、どのページが済んでいるか分かるようにする

対象: v3.0.1。利用者要望 (2026-08-15)。**着手は §1.89 (`docs/fullscreen-notice-visibility-brief.md`)
が入ったあと**。同じステータスボックスを触るため。

## 1. 直したいこと

静止画フルスクリーンの先読み表示はいま **「先読み AI  4 / 12」の進捗バー 1 本**で、
**どちら向きのページが済んでいるかが分からない**。

利用者の実例: 「4/12 と出ていたので先読みされていると思って次のページへ進んだら、
済んでいたのは全部**過去方向**だった」。数値に集約した時点で方向の情報が消えている。

**対象は静止画フルスクリーンの先読み表示のみ。** 動画・音楽・一覧側は触らない。

## 2. リモート表示が既に解いている

mIV Remote の HUD は同じ問題を、現在ページを挟んで前後に分けた点の列で表している。

```
● ● ○ ｜ ● ◐ ○ ○
  過去   ↑   未来
```

- 状態は 3 つ: **ready (取得済み) / active (取得中) / missing (未取得)**
  — `pagePrefetchIndicatorSummary` (`crates/remote-web/web/command-core.mjs:2664`)
- **並び順**: behind は `behindNearToFar.reverse()` なので **遠い → 近い**、
  ahead は **近い → 遠い**。区切りが現在位置になる (`command-core.mjs:2659`)
- 読み上げ用ラベル: `先読み: 取得済み N / 取得中 N / 未取得 N`
- 描画は `●` を並べて間に `｜` を挟むだけ (`crates/remote-web/web/app.js:14379`)
- 色: ready `#43d17b` (緑) / active `#ffd45a` (黄) / missing は暗色 + 白縁
  (`crates/remote-web/web/styles.css:240`)

**PC 側もこの並び順・状態・ラベル文言に合わせる。** 2 つの UI で見え方が食い違うと、
同じ機能なのに読み替えが要るため。

## 3. 必要なデータは PC 側に既にある

`final_ai_prefetch_progress` (`src/app.rs:51477`) は **index の一覧を持っていて、
それを数えているだけ**。捨てているのは方向の情報。

```rust
let targets = self.ai_prefetch_targets(fs_idx);   // Vec<usize>
let done = targets.iter().filter(|&&i| self.is_idx_final_ai_done_or_skipped(i)).count();
```

- `ai_prefetch_targets` (`src/app.rs:51417`) は `+1, -1, +2, -2, …` の順で返す。
  **`fs_idx` との大小で前後に振り分けられる。**
- ready: `is_idx_final_ai_done_or_skipped(i)` (`src/app.rs:51458`)
- active: `final_ai_pending` に `key.edit_key.idx == i` の key がある
- missing: どちらでもない

## 4. やること

### 4.1 純関数に切り出す

判定は表示から分離して単体テストできる形にする。名前は提案:

```rust
pub(crate) enum FsPrefetchPageState { Ready, Active, Missing }

pub(crate) struct FsPrefetchIndicator {
    /// 遠い → 近い (リモートと同じ並び)
    pub behind: Vec<FsPrefetchPageState>,
    /// 近い → 遠い
    pub ahead: Vec<FsPrefetchPageState>,
}
```

`FsPrefetchIndicator` に `ready_count` / `active_count` / `missing_count` を持たせ、
ツールチップ文言をそこから組む。

**既存の非表示条件は変えない。** `final_ai_prefetch_progress` は
「対象 0 件なら出さない」「現在ページの AI 処理中は先読み表示を隠して
『AI 処理中』だけ見せる」を持っている (`src/app.rs:51474` のコメント)。
どちらも意図的な判断なので、そのまま新しい関数へ引き継ぐ。

### 4.2 描画を置き換える

`draw_fs_ai_status` (`src/ui_fullscreen.rs:28665`) の `egui::ProgressBar` の行を、
点の列に差し替える。「先読み AI」のラベルと枠の構造は変えない。

- 色は §2 の 3 色に合わせる。ただし **missing はリモートの暗色 + 白縁をそのまま持ち込まない**。
  PC の枠は `PROGRESS_BG_COLOR` (暗い紺) なので、暗色の点は沈んで見えない。
  暗い枠の上で「未取得」と読める灰色にする。
- ホバー時に §2 の読み上げ用と同じ文言を出す。
- **片側が 16 を超えたら、その側は点をやめて `3 / 20` の数値にする。**
  先読み枚数は設定で増やせる (`ai_upscale_prefetch_forward` / `_back`) ので、
  点の列が横に伸び続けないようにする。既定は前 2 / 後 1 なので通常は点で出る。
  この切り替え判断も純関数側に置いてテストする。

### 4.3 §1.89 の設定に従う

この表示は §1.89 の **「先読み状況を表示」** が OFF なら出ない。
§1.89 の実装をそのまま使い、新しい設定は増やさない。

## 5. テスト

- 純関数の単体テスト:
  - 前後への振り分けと**並び順**(behind = 遠い→近い、ahead = 近い→遠い)
  - ready / active / missing の 3 状態
  - 件数 (取得済み / 取得中 / 未取得) がツールチップ文言と一致すること
  - 片側 16 超で数値表示に切り替わること
  - 対象 0 件と「現在ページの AI 処理中」で出さないこと (既存条件の維持)
- 見た目が変わるので `cargo test --test ui_snapshot`。差分は
  `docs/ui-snapshot-policy.md` の手順で更新し、PNG を目視してからコミットする。
- `cargo test -p mimageviewer --lib app::`、`cargo fmt --all`、
  `python scripts/check_ui_glyphs.py` (点や区切りに使う文字は
  CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」で確認する。
  **リモートは HTML なので使える文字でも、PC 側のフォントに無ければ豆腐になる**)。

## 6. ドキュメント

- `docs/spec.md` — 先読み表示の説明を更新。
- `htdocs/mimageviewer/manual/` — フルスクリーンの表示を説明しているページ。
  前後に分けて表示すること、色が何を意味するかを書く。実装語・バージョン番号は書かない。

## 7. 対象外

- 先読みの動作そのもの (枚数・順序・起動条件) の変更。**表示だけ**を変える。
- 動画・音楽・サムネイル一覧の先読み表示。
- リモート側の変更。
- `③ / ④ の重なり` の扱い (別途対応する)。

## 8. 進め方

- `docs/next-release-backlog.md` は編集しないこと (別セッションが並行で編集している)。
- 途中で範囲を超えると判断したら、症状パッチを入れずに報告する。
