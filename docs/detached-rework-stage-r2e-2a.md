# stage-r2e-2a — 終端 2 経路を「所有値の digest」へ

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
§7 ②-a と **§4.4 (retire)**。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2a)` を含める。

**触ってよいファイルは `src/app.rs` と `src/app/tests.rs` の 2 本だけ。**

**この段は挙動不変。** 保管の形 (`active_detached_viewer_context` /
`paused_bundle`) も型も変えない。②-pre と違い、panic 経路も含めて挙動は変わらない。
したがって憲法 §2 の追加合意は不要 (構造の付け替えのみ)。

---

## 1. やること

生の `ViewerContextBundle` を**所有権ごと別の関数へ渡している終端経路が 2 本**残っている。

| # | 経路 | 現行の受け渡し |
| --- | --- | --- |
| (a) | ブックマーク照合 | `take_and_close_current_active_detached_viewer_context` が生 bundle を返し、`reconcile_closed_bookmark_detached_context(&closed)` が読む |
| (b) | メディア teardown | `dropped_bundles: Vec<Box<ViewerContextBundle>>` を作り `teardown_paused_media_bundles(dropped_bundles, reason)` へ渡す |

どちらも **「読む → 所有値 → drop → App を使う」** の形へ組み替える。

### 1.1 なぜ今やるのか

この 2 本が生 bundle を外へ出しているのは、**`&mut App` と bundle を同時に借りられない**
からである。読みたいものが drop の前に必要なので、bundle ごと外へ出して借用を分けている。

②-d の `retire` transaction は「bundle は関数の中で drop され、外へは出ない」形なので、
digest を先に確定させておかないと、②-d でこの 2 本だけが型に乗らない。**保管が変わらない
今のうちに形だけ合わせておけば、②-d の差分は機械的になる。**

digest の中身 (`pdf_password_request` / tile-companion / mode-switch) をここで確定させることが、
この段のもう 1 つの目的である。設計 §4.4 の P1 指摘 2 件は、どちらも
「digest から落ちると壊れるもの」の指摘だった。

---

## 2. 経路 (a) — ブックマーク照合

### 2.1 現行

- `take_and_close_current_active_detached_viewer_context`
  ([app.rs:38660](../src/app.rs:38660)〜[:38689](../src/app.rs:38689))
  — holder から取り出し、close の儀式をして、**生 bundle を返す**。
  戻す直前に **PDF パスワードダイアログの App-global state を畳む判定**を自分でしている
  ([app.rs:38683](../src/app.rs:38683)〜[:38688](../src/app.rs:38688))。
- 呼び出し元は 2 つ:
  - [app.rs:38694](../src/app.rs:38694) `close_current_active_detached_viewer_context` —
    `.is_some()` だけ見て捨てる。
  - [app.rs:41579](../src/app.rs:41579)〜[:41592](../src/app.rs:41592) —
    `detached_viewport_finalized` かどうかで**2 通りの取り出し方**をし、
    どちらも `reconcile_closed_bookmark_detached_context(&closed)` へ渡す。

### 2.2 `reconcile_closed_bookmark_detached_context` が bundle から読むもの

全部で 4 つ。これで**打ち止め**である
([app.rs:41345](../src/app.rs:41345)〜[:41431](../src/app.rs:41431))。

| 読み | 用途 |
| --- | --- |
| `bookmark_view_state` → `BookmarkViewState::target()` | `main_target` と比較して `stayed_at_opened_bookmark` |
| `selected` + `items[idx]` の `Video`/`Audio` パス | 復帰後に同じファイルを選び直す |
| `archive_source_override` | `destination` の第 1 候補 |
| `current_folder` | `destination` の第 2 候補 |

### 2.3 やること

```rust
/// 終端 close された context から、閉じた後に必要な所有値だけを取り出したもの。
#[cfg(windows)]
struct ClosedBookmarkSummary {
    /// `bundle.bookmark_view_state.as_ref().and_then(BookmarkViewState::target).cloned()`
    bookmark_target: Option<crate::bookmark_browser::BookmarkViewReturnTarget>,
    /// `selected` が指す item が `Video`/`Audio` のときだけ `Some`。
    selected_media_path: Option<PathBuf>,
    /// `archive_source_override` を優先し、無ければ `current_folder`。
    destination: Option<PathBuf>,
    /// `pdf_password_request.is_some()`。
    had_pdf_password_request: bool,
}

impl ClosedBookmarkSummary {
    #[cfg(windows)]
    fn read(bundle: &ViewerContextBundle) -> Self { ... }
}
```

- `take_and_close_current_active_detached_viewer_context` の戻り値を
  `Option<ClosedBookmarkSummary>` にする。中では **summary を作ってから bundle を drop し、
  その後で PDF ダイアログの判定を `summary.had_pdf_password_request` で行う**。
  判定に使う `self.pdf_password_request` は swap で戻した main 側の値なので、
  **判定を swap より前へ動かさないこと**。
- `reconcile_closed_bookmark_detached_context` の引数を
  `&ClosedBookmarkSummary` にする。中の 4 つの読みを summary のフィールド参照に置換する。
- [app.rs:41579](../src/app.rs:41579) の `detached_viewport_finalized` 側の分岐
  (`active_detached_viewer_context.take().map(|active| active.bundle)`) も
  **summary を作って bundle をその場で drop する**形にする。

### 2.4 落とさないこと

- ⚠ **PDF パスワードの畳みは、現行どおり `take_and_close_...` の経路にだけ効かせる。**
  `detached_viewport_finalized` 側の分岐は現行も畳んでいない。**この非対称はこの段では直さない。**
  ただし「意図的な非対称か、落ちているのか」を読んで**報告だけする**
  (`finalize_closed_active_detached_viewport` が代わりに畳んでいるなら意図的)。
  直すべきだと判断した場合も、この段では入れずに BA 番号付きで報告する (憲法 §2)。
- ⚠ **summary は無条件に作る。** 現行の 4 つの読みは `reconcile` の early return
  (`bookmark_view_state` が `Opening` でない / `items_are_bookmark_view` が false) の**後**に
  評価されている。digest 化すると early return されるケースでも clone が走るが、
  読みはすべて純粋なので**挙動は変わらない**。「無駄だから」と lazy 化しないこと
  (`Option<&ViewerContextBundle>` を持ち回る形に戻すと、この段の目的が消える)。
- ⚠ `destination` は 2 フィールドを 1 つに畳む。これは `reconcile` が現にやっている
  優先順位そのものなので、**畳んだ後の 1 値だけを持つ**のが正しい。

---

## 3. 経路 (b) — メディア teardown

### 3.1 現行の順序

`teardown_paused_media_bundles_for_window_ids`
([app.rs:39507](../src/app.rs:39507)〜[:39545](../src/app.rs:39545)) →
`teardown_paused_media_bundles` ([app.rs:39548](../src/app.rs:39548)〜[:39566](../src/app.rs:39566))。

1. `clears_tile_companion` を決める (閉じる窓ごとに `parked_window_owns_video_tile_companion`)
2. `clears_mode_switch` を決める (`closing_parked_windows_own_native_video_mode_switch`)
3. 窓ごとに pending 破棄 / nav pending cancel
4. tile companion の App-global を畳む
5. mode switch を畳む
6. **閉じる窓すべてから `paused_bundle` を take して `Vec` に集める**
7. 全 bundle の plan を作る
8. `save_viewer_context_media_teardown_resumes(&plans)`
9. `cleanup_viewer_context_media_teardown_globals(&plans, reason)`
10. 全 bundle の `normalize_*` を clear し、関数を抜けて drop

### 3.2 やること

6〜10 を**窓ごとの「take → plan → normalize clear → drop」ループ + plans での後始末**にする。

```rust
let mut plans = Vec::new();
for window in &mut self.detached_image_windows {
    if !window_ids.contains(&window.id) { continue; }
    let Some(mut bundle) = window.paused_bundle.take() else { continue; };
    let plan = viewer_context_media_teardown_plan(&bundle);
    bundle.normalize_ui_states.clear();
    bundle.normalize_auto_scan_suppressed.clear();
    drop(bundle);
    plans.push(plan);
}
if plans.is_empty() { return; }
self.save_viewer_context_media_teardown_resumes(&plans);
self.cleanup_viewer_context_media_teardown_globals(&plans, reason);
```

- `teardown_paused_media_bundles` は**呼び出し元が 1 つだけ**なので、この形にすると消える。消す。
- `plans.is_empty()` の早期 return は現行の `dropped_bundles.is_empty()` と同じ意味を保つため
  (窓が 1 つも bundle を持っていなければ save/cleanup を呼ばない)。

### 3.3 動かしてはいけないもの

- ⚠ **1 と 2 はループより前のままにする。** どちらも
  **閉じる側と生存側の両方の `paused_bundle` を見る**判定
  ([app.rs:39479](../src/app.rs:39479)〜[:39501](../src/app.rs:39501))。
  take の後に評価すると生存判定が壊れ、生き残る動画を見落として
  tile overlay や mode-switch を誤って畳む。
- ⚠ **3〜5 の順序も変えない。**

### 3.4 この段で確かめること (推測しない)

新しい形では、**2 つ目の bundle の plan を作る時点で 1 つ目は既に drop されている**。
現行は全 plan を作り終えてから drop していた。

**plan の入力に context を跨ぐものが無いことを、コードを読んで確定させること。**
`viewer_context_media_teardown_plan` ([app.rs:9287](../src/app.rs:9287)) が読むのは
`items` / `fullscreen_idx` / `fs_cache` の `VideoPlayer` / `video_audio_mode` /
`video_audio_vst` で、いずれも bundle 自身のフィールドに見える。
ただし `VideoPlayer` の `position()` / `last_displayed_pts_secs()` / `duration()` /
`is_at_eof()` と、`viewer_context_bundle_is_music_consumer` が
**共有 (VST3 bridge / 音声出力 / music state) に触れていないか**は読んで確かめる。

**跨ぐ入力が 1 つでもあれば、この形にせず止めて報告すること。**
その場合の回避 (plan を全部作ってから drop) は②-d の retire 形と両立しないので、
設計側を直す必要がある。

---

## 4. やらないこと

- **保管を変えない。** `active_detached_viewer_context` も `paused_bundle` もそのまま。
- `save_detached_video_resume_positions_for_exit` ([app.rs:39814](../src/app.rs:39814)) は
  `as_ref` / `as_deref` の**読み取り借用**で所有権を出していない。②-d で
  `any_viewer_context` へ寄せる対象なので、**この段では触らない**。
- `closing_parked_windows_own_native_video_mode_switch` の
  「マウント中 / active holder / parked 群」**3 経路の読み分け**もそのまま。
  1 本化は②-d (設計 §4.4 末尾)。
- 残る所有プリミティブ 3 本 — `take_current_viewer_context_bundle`
  ([app.rs:16667](../src/app.rs:16667))、`split_current_context_preserving_main_grid`
  ([app.rs:42005](../src/app.rs:42005))、
  `split_materialized_physical_context_for_independent_still_open`
  ([app.rs:42504](../src/app.rs:42504)) — は**終端ではない** (fork / 取り出し) ので対象外。
- **新しいモジュールを作らない。** `ClosedBookmarkSummary` は `src/app.rs` に置く。
  型の移設は②-b。`viewer_context_registry.rs` には**触らない**。
- **`.ok()` / `let _ =` / 無言の `continue` で失敗を潰さない** (憲法 5)。
  この段では新しい typed error は増えないが、既存の分岐を握り潰さないこと。

---

## 5. テスト

`src/app/tests.rs` は**追加と、既存 2 本の付け替えだけ**。既存テストを消さない・弱めない。

**付け替え (2 本)**

1. `detached_bookmark_book_close_restores_main_grid_even_after_page_navigation`
   ([tests.rs:12693](../src/app/tests.rs:12693)) — 今は bundle を組んで
   `reconcile_closed_bookmark_detached_context(&closed)` を呼んでいる。
   **bundle を組む部分は残したまま `ClosedBookmarkSummary::read(&closed)` を通す**こと。
   summary を直接組み立てる形にすると、read 側のカバレッジが消える。
2. 同様に `reconcile` を直接呼ぶテストが他にあれば同じ扱い
   (`rg "reconcile_closed_bookmark_detached_context" src/app/tests.rs` で確認)。

**追加 (最低 5 本)**

3. `ClosedBookmarkSummary::read` が 4 つの読みを正しく畳むこと。特に
   **`archive_source_override` が `Some` のとき `current_folder` を見ない**ことと、
   `selected` が画像を指しているときは `selected_media_path` が `None` になること。
4. **終端 close で PDF パスワードダイアログが畳まれること** —
   閉じる context が `pdf_password_request` を持ち、main が持たないとき、
   `show_pdf_password_dialog` / `pdf_password_input` / `pdf_password_error` /
   `pdf_password_save` がすべて初期化される。**main 側が request を持っているときは
   畳まれない**ことも同じテストで見る (2 ケース)。
5. **teardown で閉じる窓が 2 つあるとき、両方の resume が保存されること。**
   `VideoPlayer::disconnected_for_test(path, duration)` +
   `set_last_displayed_pts_for_test` で 2 窓分の parked bundle を作り、
   **異なる位置**を入れて、両方が `settings.video_resume_positions` に入ることを見る。
   これが 3.4 の「早く drop しても plan が壊れない」の番人になる。
   → **1 窓だけのテストにしないこと。** 1 窓では順序の違いが出ない。
6. **生存する窓が動画を持っているとき `native_video_mode_switch` が畳まれないこと。**
   閉じる窓と生存窓の両方に動画を入れ、teardown 後も `native_video_mode_switch` が
   `Some` のままであることを見る。3.3 の順序の番人。
7. **閉じる窓が bundle を持っていないとき、save も cleanup も呼ばれないこと。**
   現行の `dropped_bundles.is_empty()` 早期 return と同じ挙動であることを、
   App-global が触られないことで見る。

⚠ **落ちないテストを書かないこと。** 5 は 2 窓、6 は生存側にも動画、7 は「呼ばれない」を
観測可能な形で。②-pre のレビューで「落ちようがないテスト」を 4 本書いて全部差し戻している。

---

## 6. 完了条件

すべて満たしてから報告する。

1. `src/app.rs` から次の 2 つのシグネチャが消えていること。

   ```
   rg -n "fn take_and_close_current_active_detached_viewer_context" src/app.rs   # -> Option<ClosedBookmarkSummary>
   rg -n "Vec<Box<ViewerContextBundle>>" src/app.rs                              # -> 0 件
   rg -n "fn teardown_paused_media_bundles\b" src/app.rs                         # -> 0 件
   ```

2. 所有 bundle を跨がせている関数が**プリミティブ 3 本だけ**になっていること。

   ```
   rg -n "\-> ViewerContextBundle|\-> Box<ViewerContextBundle>|: Vec<Box<ViewerContextBundle>>|: ViewerContextBundle\b" src/app.rs
   ```

   期待される残りは `take_current_viewer_context_bundle` /
   `split_current_context_preserving_main_grid` /
   `split_materialized_physical_context_for_independent_still_open` /
   `ActiveDetachedViewerContext::bundle` / `DetachedImageWindowSnapshot::paused_bundle` /
   `ViewerContextBundle` を `&` で受ける読み取り関数。**それ以外が残っていたら報告する。**

3. **PowerShell から** `cargo test -p mimageviewer --lib` が緑
   (bash 経由だと FFmpeg DLL が解決できず `STATUS_DLL_NOT_FOUND` になる。
   プランの「R2e の作業環境」を読むこと)。件数を実出力で示す。

4. `cargo fmt --check` が無出力。

5. `git diff --numstat HEAD` を貼る。**`src/app.rs` と `src/app/tests.rs` 以外に
   差分が出ていないこと。**

---

## 7. 報告に必ず含めること

- 2.4 の **PDF パスワード非対称**をどう読んだか (意図的か / 落ちているか / 根拠)。
- 3.4 の **plan 入力が context を跨がないこと**を、どのコードを読んで確定させたか。
  跨ぐものが見つかったら**その場で止めて**報告する (回避策を入れない)。
- 追加したテストが、**変更前のコードで実際に落ちるか**。落ちないテストがあれば正直に言う。
- 途中で「この形にすると挙動が変わる」と気づいた点があれば、直す前に報告する。
  この段は**挙動不変が条件**なので、変わるなら設計側の問題である。
