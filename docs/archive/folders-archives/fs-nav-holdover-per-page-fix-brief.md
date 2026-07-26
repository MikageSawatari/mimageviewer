# 実装ブリーフ: Ctrl+↑↓ の nav holdover が新しい本の全ページ枠に貼られる

作成日: 2026-07-26
対象: mImageViewer v2.8.0 / `master` (`32f68fed` 時点)
実装: Codex Sol / 設計・検収: ClaudeCode

---

## 1. 症状 (実機報告)

連結読み (`ReadingFlow::Vertical` / `Horizontal`) の本へ Ctrl+↑↓ で移動すると、
**移動元の PDF のページが連結ページ形式で一瞬表示され**、その後ページの中身が
順に本来の内容へ差し替わる。

複数ウィンドウ (detached) に限らず、**フル機能ウィンドウモードでも同じ**。

## 2. 根本原因 (コードから確定)

`fs_holdover_tex` は「Ctrl+↑↓ の直前にユーザーが見ていた**ビュー**」を保持するための
**単一テクスチャ**で、`capture_fs_nav_holdover(fs_idx)` が移動元の1ページから取る
(`src/ui_fullscreen.rs:2415-2418`)。

ところが描画側はこれを**ページ枠ごとの最後の fallback テクスチャ**として消費している。

`src/ui_fullscreen.rs:17961` (`draw_fs_spread_page`):

```rust
let display_tex = display_tex.or(thumb_tex.as_ref()).or(holdover_tex);
```

呼び出し側:

| 描画経路 | 位置 | ページ枠数 | 結果 |
| --- | --- | --- | --- |
| 連結読み `draw_fs_continuous_reading` | 15763 で1回計算 → 15827 で**全ページ**へ渡す | 可視範囲の N 枚 | **移動元の同じ1ページが N 回並ぶ** |
| 見開き `draw_fs_spread` | 17782 / 17858 で計算 → 左右**両方**へ渡す | 2 | 同じページが2回並ぶ |
| 単ページ `prepare_fullscreen_state` | 9311 / 9317 | 1 | ビュー＝ページなので自然に見える |

単ページでは「ビュー単位の hold」と「ページ単位の fallback」が一致するため
これまで問題として現れなかった。連結読みで初めて N 枚に増幅されて露見した。

### 表示窓 (いつからいつまで)

`fs_nav_holdover_tex_for_draw()` (`src/ui_fullscreen.rs:2368-2393`) は
`fs_nav_locked_gen` が立っている間、**アンカーページ (`fullscreen_idx`) の表示物が
用意できるまで** holdover を返す。

- 開始: 移動先の items が入った時点 (PDF はメタキャッシュ hit で N 枚の空
  `PdfPage` placeholder が即座に入る = サムネも本文も無い枠が N 個できる)
- 終了: アンカーページの `resolve_fs_display_tex` が Some になる or Failed 確定

連結読みではこの窓の間、可視範囲の全枠が holdover にフォールバックする。

### 副作用 (同じ原因の別の症状)

`draw_fs_spread_page` は渡されたテクスチャに **移動先ページの**
回転 (`get_rotation(page.idx)`)・透過背景スタイル・`content_bbox` (view trim) を
適用する。したがって移動元のページ画像が、移動先ページの回転・トリム設定で
描かれる。単ページでも起こりうる (移動先の1ページ目に回転が保存してあると、
旧ページが一瞬その角度で出る)。

### 混同してはいけないもの

`continuous_page_transition_texture(page.idx)` (`67f07069` 追加) は
**ページ単位の直前テクスチャ**で、カラー化再読込のちらつき防止に使う正しい仕組み。
今回触るのは `fs_holdover_tex` (ビュー単位) だけ。`continuous_page_transitions` は
そのまま残すこと。

## 3. 修正方針

**nav holdover は「ビュー単位の hold」であり、「ページ単位のテクスチャ代用」ではない。**
この所有権の取り違えを直す。

既に `fullscreen_idx == None` の gap では正しくビュー単位で描かれている
(`keep_fullscreen_viewport_alive` の holdover 分岐 = `src/ui_fullscreen.rs:6809-6849`、
`render_embedded_fs_nav_holdover` = 7098-7119。どちらも **中央 contain フィットで1枚だけ**
描く)。`fullscreen_idx == Some(新ページ)` かつ nav ロック継続中の窓も、これと
同じ描き方に揃える。

### [F1] `draw_fs_spread_page` から holdover を構造的に締め出す

`holdover_tex: Option<&egui::TextureHandle>` 引数を**削除**する。
呼び出し側 (`draw_fs_continuous_reading` / `draw_fs_spread` の2箇所) の
`holdover_for_locked` 受け渡しも削除する。

引数を消すことで「ページ枠が別の本のページを代用することはあり得ない」が
**コンパイル時に保証**される (テストより強い保証)。

ページ枠にテクスチャが無ければ従来どおり `読込中...` を描く (else 分岐はそのまま)。

### [F2] ビュー単位の holdover オーバーレイを描く

単ページ / 見開き / 連結読みの各描画経路で、**ページ描画ループの後**に、
nav holdover が有効なら `image_rect` へ 1 枚だけ中央 contain フィットで描く。

- 描画位置はページの上 (= 直前のビューを保持して見せる意味論)
- `image_rect` にクリップする
- 回転・`content_bbox`・透過背景スタイルは**適用しない**
  (移動元テクスチャに移動先ページの設定を当てない)
- 既存の gap 用描画 (6821-6847) と同じ contain フィット計算を使う。
  3箇所で同じ式を書かず、共有ヘルパ (例
  `fn paint_fs_nav_holdover_overlay(painter: &egui::Painter, rect: egui::Rect,
  tex: &egui::TextureHandle)`) に切り出して gap 側からも使えるようにすること。

### [F3] `prepare_fullscreen_state` の holdover 利用を整理

`src/ui_fullscreen.rs:9309-9318` は `thumb_tex` の fallback として
`fs_nav_holdover_tex_for_draw()` を使っている (単ページ経路)。F2 のオーバーレイに
一本化し、ここからは外す。

ただし `waiting_for_colorize` 分岐 (9306-9311) の意図
「カラー化待ちで生の白黒サムネイルを見せない」は壊さないこと。
`fs_nav_holdover_tex_for_draw()` は nav ロック中しか Some を返さないので、
カラー化単独の再読込 (nav なし) には元々効いていない。挙動が変わらないことを
確認して報告すること。変わるなら、その分岐だけ従来どおり残す判断も可 (要報告)。

### やってはいけないこと

- 連結読みのときだけ holdover を無効化する、といった flow 依存の条件分岐
  (= 症状パッチ。見開きの二重表示と回転誤適用が残る)
- `continuous_page_transitions` / `continuous_page_transition_texture` の変更
- `fs_nav_holdover_tex_for_draw()` の解放条件 (`items_generation` / 新コンテンツ
  readiness) の変更。**いつ holdover を出すか**は現状維持で、**どう描くか**だけ直す
- 新しい App-global フィールドの追加

## 4. 触ってよいファイル

- `src/ui_fullscreen.rs`
- `src/app/tests.rs`
- `docs/display-pipeline.md` (§5 参照)

`git status` に上記以外の差分が見えても別セッションの作業なので触らないこと
(`git checkout` / `git restore` / `git stash` を使わない)。
`git add` / `commit` / branch 操作も禁止。

## 5. ドキュメント更新

`docs/display-pipeline.md` の表示テクスチャ優先順位の節に、

- ページ枠の fallback 順は `final/processed → ページ単位の transition テクスチャ →
  サムネイル → 読込中`。**nav holdover はここに入らない**
- `fs_holdover_tex` は Ctrl+↑↓ 中の**ビュー単位**の hold で、`image_rect` に 1 枚だけ
  重ねて描く。`fullscreen_idx` が None の gap 経路と Some の nav ロック経路で
  同じ描き方をする

を追記する。

## 6. テスト

描画そのものは unit test しにくいので、次の3点で担保する。

1. **F1 のシグネチャ変更**が「ページ枠に holdover が入らない」ことの構造的保証。
   これが主たる担保なので、無理に描画テストを書かない。
2. `fs_nav_holdover_tex_for_draw()` の既存テストがあれば維持。無ければ
   `holdover_view_overlay_is_active_only_until_new_content_is_ready` を追加:
   - lock 無し → None
   - lock 有り + `items_generation` 未進行 → Some
   - lock 有り + generation 進行 + 新 idx の表示物なし → Some
   - lock 有り + generation 進行 + 新 idx の `fs_cache` が Static → None
   - lock 有り + generation 進行 + 新 idx が Failed → None
3. contain フィット計算をヘルパに切り出したら、その純関数の unit test を1本
   (縦長 / 横長 / 正方形で `image_rect` からはみ出さず中央に来ること)。

## 7. 検証

```bash
cargo fmt
cargo fmt --check
cargo clippy --all-targets
cargo test --lib
cargo test --test ui_snapshot        # 見た目を変えるのでスナップショット差分を確認
```

- `cargo test --lib` の baseline は **4262 passed / 0 failed / 18 ignored**。
- `ui_snapshot` に差分が出た場合、**勝手に `UPDATE_SNAPSHOTS=1` で更新しない**。
  どのスナップショットがどう変わったかを報告すること
  (`docs/ui-snapshot-policy.md`)。

## 8. 報告してほしいこと

1. F1〜F3 の変更内容 (シンボル単位)
2. F3 で `waiting_for_colorize` の挙動が変わるか、変わらないと判断した根拠
3. 追加したテスト名と固定した内容
4. fmt / clippy / test の実数、`ui_snapshot` の差分有無
5. 実装中に見つけた、同じ「ビュー単位の状態をページ単位に流用している」箇所
   (あれば報告のみ)
