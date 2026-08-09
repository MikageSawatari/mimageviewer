# ブリーフ: 比較の準備中に古い合成が出る + 通過描画を見開きへ広げる

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

前提: master。着手前に `git log --oneline -3` で HEAD を確認すること。
2 件は独立なので**変更を分離できる形**にすること。

---

## 1. `Shift+C` 中にホイールでページを移ると、一瞬まちがった合成が出る

利用者報告 (2026-08-09、スクリーンショットあり)。「比較表示を準備中」の間、数百 ms
**古い合成**が表示され、その後に正しくなる。

**原因は特定済み**: `draw_compare_prepared_mode`
([src/ui_fullscreen.rs:22773](../src/ui_fullscreen.rs) 付近) は
`self.compare_prepared_pair` が **`Some` でありさえすれば描く**。それが現在ページの組か
どうかを見ていない。一方、今日足したナビゲータ側
(`draw_compare_navigator_content`) は `compare_prepared_pair_matches(fs_idx)` で
**現在ページと一致するかを確認している**。同じ判断の**片側だけに検査がある**状態。

**直し方**: 本文側も `compare_prepared_pair_matches(fs_idx)` を通す。一致しない間は
**古い合成を描かない**。準備中の表示 (既存の「比較表示を準備中」) はそのまま出す。
準備中に何を描くかは既存の非比較経路 (現在ページの通常表示) に合わせること。

- **新しい状態やタイマーを足さないこと**。既存の述語を両側で使うだけにする
- 準備完了後の見た目は変えないこと
- テスト: pair の `current_idx` が現在ページと違うとき、本文が合成を描かないこと

---

## 2. 通過描画 (対策 1) を見開き表示にも広げる

実機ログで **423 件の判定すべてが `ordinary_blocker: "spread_mode"`** で弾かれていた。
`fs_page_turn_materialization_for_frame` が `!self.spread_mode.is_spread()` を要求しており、
**見開きでは対策 1 が一度も発動しない**。利用者の実運用は見開き中心なので、この除外は狭すぎた
(ClaudeCode の指示ミス)。

**直し方**: 見開きでも通過描画を有効にする。**表示する 2 ページとも**カタログサムネイルが
使えるときだけ通過扱いにし、片方でも無ければ従来どおり実体化する (fail-closed)。

- 判定の入力は今までどおり**そのフレームで未消費のページ送りエッジだけ**。
  時間ガードを足さない
- 「1 フレーム 1 ユニット進む」挙動は変えない。見開きは 1 回で 2 ページ進むが、
  それは既存のナビゲーションのままでよい
- 連結読み (`reading_flow.is_paged()` が false) は対象外のまま。元から速い
- テスト: 見開きで pending あり + 両ページのサムネイルあり → 通過、片方欠け → 実体化。
  押しっぱなし中に各ユニットが最低 1 回描かれること

## 3. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- バックログ §1.58 / §1.60 へ追記

## 4. 制約

- **アプリを起動しないこと。** 検証ビルドと実機確認は ClaudeCode と利用者が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す
- 先読み枚数 (`prefetch_back` / `prefetch_forward`) には**触らないこと**。
  後退が遅い件は別途、利用者が設定で切り分け中

---

完了したら次を報告すること:

1. 本文とナビゲータが同じ述語を通っていることの根拠
2. 見開きの通過描画で 2 ページ分の判定をどう fail-closed にしたか
3. テスト結果
4. **実機で確認してほしいこと**
