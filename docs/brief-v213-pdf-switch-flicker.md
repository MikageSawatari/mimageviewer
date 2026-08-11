# ブリーフ: PDF 切り替えで黒が 1 枚挟まる (§3.3 の退行)

対象: v2.13.0 出荷ブロッカー。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

前提: master。`536169ef` (§3.3 世代刻印) まで入っている状態で発生。
着手前に `git log --oneline -3` で HEAD を確認すること。

---

## 1. 症状 (実機 2026-08-10)

PDF を開いた状態で **Ctrl+↑↓ で次の PDF へ移る**と、こう見える。

1. **画像が消えて黒画面になる** ← これが新しく増えた
2. 前の PDF の同じページが再表示される
3. 次のファイルの画像が表示される

**F12 の別ウィンドウでは、このちらつきは起きない** (利用者確認)。

## 2. 分かっていること

- 2 の「前の PDF のページ」は元からある **holdover** (フォルダ移動中に前ページを見せて
  黒画面を防ぐ仕組み) で、意図された動作。**問題は 1 の黒が入るようになったこと**
- §3.3 で `fs_cache` / `fs_early_dims` / `fs_pending` / `fs_upload_backlog` に
  `items_generation` を刻み、`set_items_generation` が世代更新時に `retain` で
  **古い entry を即座に purge** するようにした。以前は purge されず残っていた
- **main と detached で差が出る**ので、両者で描画経路か holdover の張り方が違う。
  main は in-window / embedded 経路 (`render_embedded_fs_nav_holdover`、
  `render_still_fullscreen_viewport_enter_holdover`)、detached は viewport 経路
  (`keep_fullscreen_viewport_alive` + `render_fullscreen_viewport`)

## 3. 直し方の方針

**世代刻印は外さないこと。** あれは構造の穴を塞ぐ修正で、今回の黒はその副作用として
「今まで stale entry が偶然埋めていた 1 フレーム」が露出したもの。

**やること**: purge と holdover の**順序**を確定させ、遷移中に描くものが途切れないようにする。

- **まず、main と detached で何が違うのかをソースで確定させること。** detached で起きない
  なら、main 側に足りていない holdover の張り方が detached にはある
- holdover は既にある仕組みなので、**新しい holdover や遅延・ガードを作らない**。
  「前の内容を捨てる前に、次に描くものを確保する」順序へ直す
- **時間ガード (N ms は黒を出さない等) は禁止**
- 原因が確定できなければ、**推測で直さずに報告すること**

## 4. 確認すること

- Ctrl+↑↓ の PDF 間移動で黒が挟まらないこと (main / detached の両方)
- 同じ経路の**画像フォルダ間移動**、**ZIP 間移動**でも黒が挟まらないこと
  (PDF だけ直して他が残る、を避ける)
- §3.3 の世代照合が効いたままであること (別の本のページが出ない)
- `[fs-generation] stale entry discarded` が**通常操作で大量に出ていないか**。
  出ているなら、それ自体が別の発見なので報告する

## 5. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **バックログ §3.3 に、この退行と直し方を追記**
- detached 経路に触れるなら [docs/detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  触れた範囲を記録する

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドと実機確認は ClaudeCode と利用者が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す

---

完了したら次を報告すること:

1. main と detached の差がどこにあったか (ソース上の根拠)
2. purge と holdover の順序をどう直したか
3. 画像フォルダ / ZIP でも同じ経路を通ることの確認結果
4. テスト結果
5. **実機で確認してほしいこと**
