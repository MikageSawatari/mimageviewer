# ClaudeCode 調査報告: detached PDF の Ctrl+↓ でウィンドウが閉じる

作成日: 2026-07-26
対象: mImageViewer v2.8.0 / `master` (HEAD `bd0fadb0`)
依頼元: `docs/archive/detached/detached-folder-nav-pdf-claudecode-investigation-request.md`
段階: 第1段階 (調査 + 修正設計)。**コード変更・コミットは行っていない** (作業ツリーは調査前と同一)。

---

## 0. 結論 (先出し)

`src/app.rs:35428-35435`、`update_active_detached_viewer_context()` 末尾の

```rust
let detached_viewport_finalized = if app.fullscreen_idx.is_none()
    && let Some(viewport_id) = close_viewport_id
{
    app.finalize_closed_active_detached_viewport(ctx, viewport_id);
    true
} else { false };
```

が根本原因。**`fullscreen_idx.is_none()` + フレーム冒頭の `close_viewport_id` という2つの
field presence だけで「ユーザーが viewer を終了した」と推論している**。PDF/ZIP の非同期列挙を
挟む internal reopen では、この2条件が同一フレーム内で両方成立するため、
`ViewportCommand::Close` + session finish + runtime remove が実行され、detached window が
物理的に破棄される。

依頼書 §4 の最優先仮説は**正しい**。ただし2点訂正がある (§4 参照)。

- 導入コミットは `c754a133` ではなく **`7ee84fdb` (2026-06-28, "Stabilize detached book viewer windows")**。
- `c754a133` は原因を作ったのではなく、**壊れていた終端前提へ folder-nav を到達可能にした**。

BA 分類は **BA-7 が主** (複数 field から暗黙に終端を推論)。BA-5 は結果として起きるが従属。

---

## 1. 再現結果

### 1.1 実施した再現方法

GUI 実機再現は行っていない (依頼書 §5.3 の portable-smoke 手順は未実施)。代わりに
**production の `update_active_detached_viewer_context()` を1フレーム通す unit test** で
再現・確定した。これは依頼書 §6 が「既存テストが通していない」と指摘した合成そのものである。

再現テスト (一時追加 → 検証後に revert 済み。全文は §8.1):

- App-global: `fs_viewport_shown = true`, `fs_viewport_presentation = Some(DetachedWindow)`,
  `begin_active_detached_session(21, DetachedSource::Image)` (= runtime placement 登録済み)
- mounted bundle: `navigation_scope = DetachedPhysical`, `fullscreen_idx = None`,
  `fs_nav_after_pdf_enumerate = Some(DeferredFsReopen { target: None, .. })`
  (= `close_fullscreen_for_folder_nav_reopen()` + `load_folder_nav_target(pdf)` +
  `reopen_fullscreen_after_folder_nav_load() == "enumerate_defer"` 直後の状態)
- `ctx.run()` 内で `update_active_detached_viewer_context(ctx)` を1回呼ぶ

### 1.2 観測結果 (1フレーム後)

```text
session=None
runtime_placement=false
detached_window_state=None
fs_viewport_shown=false
fs_viewport_presentation=None
mounted=true                 <-- bundle は残っている
deferred=Some(true)          <-- 「同じ窓へ reopen する」意図も残っている
```

`cargo test --lib repro_detached_pdf_enumerate_gap_must_not_finalize_active_viewport`
→ FAILED (`PDF enumerate gap is an internal reopen: the detached session must survive`)。

**つまり、アプリは「この window に次の PDF を出す」という予約 (`deferred=Some(true)`) を
保持したまま、その window を同じフレームで破棄している。**

### 1.3 因果の切り分け (実験)

`fullscreen_idx.is_none()` の条件に `&& !fs_nav_deferred_reopen_wait_active()` を一時的に
足すと、同じテストが以下になり **pass**:

```text
session=Some(ActiveDetachedSession { window_id: 21, source: Image })
runtime_placement=true
detached_window_state=Some(Active)
fs_viewport_shown=true
fs_viewport_presentation=Some(DetachedWindow)
```

→ 破壊しているのは `finalize_closed_active_detached_viewport()` **ただ1箇所**であることが確定
(`keep_fullscreen_viewport_alive` の cleanup 分岐ではない。§2.3 で理由を示す)。

### 1.4 最小マトリクスの判定 (コード読解による。GUI 未確認)

判別子は「**移動先コンテナが非同期列挙 / 変換を必要とするか**」であって、
「PDF から出るか」ではない。`apply_folder_nav_result()` の `Fullscreen | SiblingFullscreen |
SlideshowNext` arm (`src/app.rs:31159-31206`) は

`close_fullscreen_for_folder_nav_reopen()` → `load_folder_nav_target()` →
`reopen_fullscreen_after_folder_nav_load()`

の順に走り、最後が `"enumerate_defer"` / `"conversion_defer"` を返す場合だけ
`fullscreen_idx` が `None` のままフレーム末尾に到達する。

| 起点 | 操作 | 移動先 | 判定 | 根拠 |
| --- | --- | --- | --- | --- |
| PDF | Ctrl+↓ | PDF | **壊れる** | 着地が `pdf_enumerate_pending` → `enumerate_defer` (app.rs:30791) |
| PDF | Ctrl+PageDown | PDF | **壊れる** | `SiblingFullscreen` も同一 arm (app.rs:31159)。ui_fullscreen.rs:13878-13885 |
| ZIP | Ctrl+↓ | PDF | **壊れる** | 着地側だけが条件 |
| PDF | Ctrl+↓ | ZIP | **壊れる** | `zip_enumerate_pending` も同じ defer (app.rs:30791) |
| 通常画像フォルダ | Ctrl+↓ | PDF | **壊れる** | 退出側は無関係 |
| PDF | Ctrl+↓ | 通常画像フォルダ | 壊れない | 同フレームで `open_fullscreen(new_idx)` が走り `fullscreen_idx = Some` |
| 末尾 PDF | Ctrl+↓ | なし | 壊れない | `result.path == None` で早期 return、`fullscreen_idx` 不変 (app.rs:31009-31051) |
| PDF | Ctrl+↓ | RAR/7z/LZH (未変換) | **壊れる (未確認)** | `ConversionDialogOpened` + `attach_archive_convert_deferred_fullscreen` で同じ gap |

> **Ctrl+PageDown は Ctrl+↓ と同一原因**であり、切り分けの必要はない。両者とも
> `detached_physical_folder_nav_available()` 分岐 (ui_fullscreen.rs:13765 / 13879) から
> `start_folder_nav` に入り、`FolderNavMode::Fullscreen` / `SiblingFullscreen` の違いだけで
> `apply_folder_nav_result` の同じ arm に合流する。

---

## 2. 確定したイベント時系列

1フレーム内で完結する。`window_id = W`、ViewportId = `detached_image_window_viewport_id(W)`。

| # | 位置 | 動作 | 主要 state |
| --- | --- | --- | --- |
| F0 | ui_fullscreen.rs:13765-13773 | Ctrl+↓ 入力。`capture_fs_nav_holdover(fs_idx)` → `fs_nav_locked_gen = Some(gen)`、`start_folder_nav(effective_folder(), fwd, Fullscreen)` | `fullscreen_idx=Some(i)`, `fs_viewport_shown=true`, `presentation=DetachedWindow`, session=Some(W), state=Active |
| F1 | app.rs:35304-35306 | **フレーム冒頭**: `fs_viewport_shown && presentation==DetachedWindow` なので `close_viewport_id = Some(vp)` を保存 | 同上 |
| F2 | app.rs:35330-35333 | `poll_folder_nav()` が次 PDF の path を返す → `apply_folder_nav_result()` | `folder_nav_pending=None` |
| F3 | app.rs:31173 → 45468-45514 | `close_fullscreen_for_folder_nav_reopen()`。detached preserve 経路: `close_fullscreen()` 後に `fs_viewport_shown=true` / `presentation` / `window_id` / `fs_viewport_generation` を復元し、`detached_viewer_folder_nav_reuse_window_once=true` | **`fullscreen_idx=None`**, `fs_viewport_shown=true` (維持), session=Some(W) |
| F4 | app.rs:31174 → 14842 | `load_folder_nav_target(pdf)` → `load_folder_with_scan` → `load_pdf_as_folder` | `pdf_enumerate_pending=Some(..)` |
| F5 | app.rs:31197 → 30791-30801 | `reopen_fullscreen_after_folder_nav_load()` が `pdf_enumerate_pending.is_some()` を見て `fs_nav_after_pdf_enumerate = Some(DeferredFsReopen)` を立て `"enumerate_defer"` で return | **`fullscreen_idx=None` のまま**, `fs_nav_deferred_reopen_wait_active()==true` |
| F6 | app.rs:35421 → ui_fullscreen.rs:6790-6891 | `keep_fullscreen_viewport_alive()`: `fs_viewport_shown && fs_nav_deferred_reopen_wait_active()` の holdover 分岐に入り、**同じ viewport を直前ページで描画して return**。session/runtime は触らない (設計どおり) | 変化なし。window は生存 |
| F7 | app.rs:35422 | `render_fullscreen_viewport()`: `fullscreen_idx=None` なので実質 no-op | 変化なし |
| F8 | **app.rs:35428-35435** | **`fullscreen_idx.is_none()` (F3 由来) かつ `close_viewport_id=Some(vp)` (F1 由来) → `finalize_closed_active_detached_viewport(ctx, vp)`** | — |
| F9 | app.rs:34497-34515 | `ViewportCommand::Close` 送信 → `fs_viewport_shown=false` → `fs_viewport_presentation=None` → `clear_detached_viewer_host_hwnd()` → `finish_active_detached_session_close()` → `remove_detached_window_runtime(W)` → font atlas resync → `focus_main_after_detached_window_close_if_idle()` | **session=None, runtime 削除, window 破棄** |
| F10 | app.rs:35436-35447 | `should_drop` を計算。`fs_nav_deferred_reopen_wait_active()==true` なので **false** → bundle は mount されたまま残る | mounted=true, deferred=Some |
| F11 | 次フレーム以降 | `detached_active_window_alive_wanted()` が false になったため `render_active_detached_viewport_backstop()` (ui_fullscreen.rs:6973) も描かない。`fullscreen_viewport_id()` の指す窓は既に破棄済み | 窓は戻らない |

**F6 と F8 の隣接が症状の本質**: 2行離れた位置で、片方が「gap 中だから窓を維持する」と決め、
もう片方が「`fullscreen_idx` が無いから終端だ」と決めて、後者が勝っている。

> 未確定 (推測ではなく未検証): F11 以降、`poll_pdf_enumerate` 完了時に
> `open_deferred_fullscreen_after_enumerate()` が何を開くか (新 window_id を割り当てるか、
> 何も起きないか) はテストで通していない。実機の「次の PDF が表示されない」がここに
> 対応するが、原因確定には不要なので追わなかった。修正後の検証項目とする (§7.4)。

---

## 3. 根本原因

### 3.1 破られた不変条件

> **internal folder-nav reopen と terminal close を区別する** (依頼書 §2 / `docs/archive/detached/detached-rework-stage-folder-nav.md`)

### 3.2 該当箇所

| 種別 | 位置 |
| --- | --- |
| 直接原因 | `src/app.rs:35428-35435` — ungated な `finalize_closed_active_detached_viewport` 呼び出し |
| 破壊処理の実体 | `src/app.rs:34483-34524` — `finalize_closed_active_detached_viewport()` |
| 矛盾するコメント | `src/app.rs:34506-34509` — 「ここは should_drop 経路でのみ呼ばれる。folder-nav reopen や初回 image scan 中は呼ばれない」 |
| gap を維持する側 | `src/ui_fullscreen.rs:6790-6891` — keep-alive holdover 分岐 |
| gap を作る側 | `src/app.rs:30791-30801` — `"enumerate_defer"` |

### 3.3 導入コミットと到達可能化コミット

`git log -S "let detached_viewport_finalized = if app.fullscreen_idx.is_none()"` → **`7ee84fdb` のみ**。

`7ee84fdb` (2026-06-28) 時点のコードは以下だった:

```rust
let detached_viewport_finalized = if app.fullscreen_idx.is_none()
    && let Some(viewport_id) = close_viewport_id { ... finalize ...; true } else { false };
let should_drop = app.fullscreen_idx.is_none()
    && app.pdf_enumerate_pending.is_none()
    && app.zip_enumerate_pending.is_none()
    && !app.fs_viewport_shown;
if should_drop && !detached_viewport_finalized && let Some(viewport_id) = close_viewport_id {
    app.finalize_closed_active_detached_viewport(ctx, viewport_id);   // <-- 常に dead
}
```

**同じ finalize を「無条件版」と「`should_drop` gated 版」の2つ書き、無条件版が先に走るため
gated 版が構造的に到達不能になっている。** 意図された意味論は gated 版であり、コメント
(34506-34509) は gated 版を説明している。`4c2a9e7c` は dead code として **gated 版 (= 正しい方) を削除**
したため、コメントと実装の乖離が固定された。

到達可能にしたのは `c754a133` (2026-07-24)。`git log -S "app.poll_folder_nav()"` は
`c754a133` のみを返す。それ以前は active detached の closure 内で `fullscreen_idx` が
`None` になる経路が「明示 close」または「初回 open 失敗」しかなく、無条件 finalize でも
たまたま正しく見えていた。

つまり依頼書 §5.1-4 の分類では **「新しい folder-nav が既存の壊れた終端前提へ到達可能にした」**。
後続修正 (`171a8737` / `4c2a9e7c`) は終端条件を直接壊してはいない。

### 3.4 BA-7 の証拠 (guard の非対称な蓄積)

`should_drop` は問題が見つかるたびに条件が増えている一方、finalize は1つも増えていない:

| 条件 | `should_drop` | finalize |
| --- | --- | --- |
| `fullscreen_idx.is_none()` | ✓ | ✓ |
| `folder_nav_pending.is_none()` | ✓ (`4c2a9e7c` で追加) | ✗ |
| `folder_pane_open_pending.is_none()` | ✓ (`171a8737` で追加) | ✗ |
| `pdf_enumerate_pending.is_none()` | ✓ | ✗ |
| `zip_enumerate_pending.is_none()` | ✓ | ✗ |
| `!fs_nav_deferred_reopen_wait_active()` | ✓ (`4c2a9e7c` で追加) | ✗ |
| `pdf_password_request.is_none()` | ✓ (`4c2a9e7c` で追加) | ✗ |
| `bookmark_open_pending != Book` | ✓ | ✗ |
| `!fs_viewport_shown` | ✓ | ✗ |

同じ「内部遷移中リスト」は `App::update` 内でさらに3箇所目として複製されている
(app.rs:35362-35374 の `request_repaint_after` 条件)。**同一概念が3つの場所に手書きで
複製され、1つ (finalize) だけ更新から取り残された**というのが構造的な失敗である。

---

## 4. 最優先仮説の判定

依頼書 §4 の7ステップ仮説は **成立** (テストで確定)。訂正 / 補足は2点。

**訂正1: 導入コミット**
「folder-nav が終端条件を壊した」のではなく、終端条件は `7ee84fdb` 時点で既に壊れていた
(しかも同じ関数内に正しい版が dead code として同居していた)。

**訂正2: 「finalize 判定が `should_drop` 計算より前に独立している」の因果方向**
依頼書は矛盾として指摘しているが、この順序自体は意図的である可能性が高い。
`close_fullscreen()` は `fs_viewport_shown` を意図的に true のまま残す仕様
(app.rs:45523-45527 の doc comment) なので、finalize が先に走って `fs_viewport_shown=false`
にしないと `should_drop` の `!fs_viewport_shown` が永久に成立しない。
つまり **finalize は `should_drop` の前提条件を作る役割も持たされており、単に順序を
入れ替えるだけの修正は「bundle が drop されない」別バグを作る**。§7 の設計はこれを踏まえる。

**中心命題への回答**

> `fullscreen_idx=None` は「viewer を閉じる意思」ではなく、PDF/ZIP の internal reopen 中にも現れる。
> それにもかかわらず、active detached の終端処理が field presence だけで terminal close を推論していないか。

→ **Yes。推論している。** しかも本コードベースには既に**明示的な終端宣言**が存在する
(`DetachedWindowState::Closing` / `begin_active_detached_session_close()`、12箇所の呼び出し)。
finalize はそれを一切参照していない。

---

## 5. 既存テストが見逃した理由

| テスト | 見逃した理由 |
| --- | --- |
| `active_detached_update_polls_only_its_bundle_folder_nav_result` | production update を通す唯一のテストだが、`outcome: None` (= 境界) を送る。境界は `apply_folder_nav_result` の早期 return (app.rs:31009-31051) で `fullscreen_idx` を触らないため、**問題のフレーム末尾条件に到達しない**。 |
| `detached_folder_nav_close_preserves_viewport_host_for_reopen` | `close_fullscreen_for_folder_nav_reopen()` を**直接呼ぶ helper 単体テスト**。`update_active_detached_viewer_context()` を通さないので、同一フレーム末尾の finalize と合成されない。 |
| `folder_nav_close_preserves_detached_window_id_for_reuse` / `folder_nav_reopen_reuses_active_detached_window_id` | 同上。window identity の保存は検証するが、**window が生きているか**は検証していない (`active_detached_session` / runtime presence を assert していない)。 |
| `detached_book_pdf_open_keeps_main_grid_on_parent_list` | main 側の不変を見るテストで、detached window の生存を見ない。 |
| `detached_folder_nav_reopen_reuses_window_even_if_grid_intent_returns` | 同一フレーム内で reopen が完了する (= 同期の通常フォルダ) ケース。**async gap を1フレーム以上またがない**。 |

共通の穴は3つ:

1. **helper 単体呼び出しに寄っている** — `update_active_detached_viewer_context()` の
   フレーム末尾処理と合成されない。
2. **async gap を作らない** — `fs_nav_after_pdf_enumerate` / `pdf_enumerate_pending` を
   立てた状態でフレームを回すテストが1本も無い。
3. **assert 対象が identity に偏っている** — `window_id` / ViewportId / reuse flag は
   検査するが、`active_detached_session` の生存・`detached_window_runtime_placement()` の
   存在・`fs_viewport_shown` という「窓が実在するか」を検査していない。

---

## 6. sibling 経路監査

### 6.1 同じ finalize で壊れる (同一原因)

| 経路 | gap を作る field | 状態 |
| --- | --- | --- |
| ZIP 着地 (Ctrl+↓ / Ctrl+PageDown) | `zip_enumerate_pending` + `fs_nav_after_pdf_enumerate` | 壊れる (コード上確定) |
| 変換アーカイブ着地 (RAR/7z/LZH) | `archive_convert_deferred_fullscreen_active()` | 壊れる (未再現。`fs_nav_deferred_reopen_wait_active()` の第2項) |
| protected PDF | `pdf_password_request` | **より早く壊れる**。gap 1フレーム目で既に finalize 済みなので、パスワードダイアログ OK 後に戻る窓が無い |
| detached 物理フォルダ open の scan 待ち | `folder_pane_open_pending` | 同一クラス。`should_drop` にだけ guard がある (`171a8737`) |
| bookmark Book open | `bookmark_open_pending == Book` | 同一クラス。`should_drop` にだけ guard がある |
| required target 失敗 | — | `DetachedPhysicalFolderOpenPoll::Failed` は明示 close を宣言しない (app.rs:30512/30538)。現状は finalize の暗黙推論に依存しているため、修正時に**明示宣言へ移す必要がある** (§7.3) |

### 6.2 現在は壊れないが同型の推論をしている箇所 (要統合)

`src/ui_fullscreen.rs:6898-6914` の `keep_alive_cleanup`:

```rust
if self.fs_viewport_shown && self.fs_nav_deferred_reopen_wait_active() { ...holdover...; return; }
if !self.fs_viewport_shown { return; }
// ここに来る = 「本当に detached/fullscreen を閉じた」cleanup フレーム
if self.active_detached_session.is_some() { begin_close; finish_close; remove_runtime; }
```

これも `fullscreen_idx.is_none()` + `fs_viewport_shown` + `!deferred` からの推論である。
**内部遷移の判定に `fs_nav_deferred_reopen_wait_active()` しか使っていない**ため、
`should_drop` の9条件より狭い。具体的には

- `pdf_enumerate_pending = Some` かつ `fs_nav_after_pdf_enumerate = None`
  (= folder-nav 由来ではない detached コンテナ open。`4c2a9e7c` "Fix detached container open lifecycle" が扱った領域)
- `folder_pane_open_pending = Some`
- `pdf_password_request = Some`

のいずれでも、この cleanup が session を畳んで runtime を消す。
**finalize だけを直しても、この経路が残っていると同種の症状が別条件で再発する。**
§7 の設計はこの2箇所を同じ述語の下に置く。

### 6.3 影響しないことを確認した経路

| 経路 | 理由 |
| --- | --- |
| main window (非 detached) の Ctrl+↓ → PDF | `finalize_closed_active_detached_viewport` の production 呼び出しは app.rs:35431 の1箇所のみで、`update_active_detached_viewer_context()` 内 = active detached 専用 |
| Esc / × による明示 close | `handle_fullscreen_close_request()` (app.rs:45343-45356) が `begin` + `finish` + `remove_runtime` を `close_fullscreen()` **より前**に実行する。finalize 到達時には session が既に None |
| gap 中の Esc / × | keep-alive の cancel 分岐 (ui_fullscreen.rs:6863-6889) が `fs_nav_after_pdf_enumerate = None` + `release_fs_nav_lock()` + session begin/finish + runtime remove を実行。以後は通常の terminal 扱い |
| 境界 (次が無い) | `apply_folder_nav_result` 早期 return。`fullscreen_idx` 不変 → finalize 不成立 |
| passive detached window (B) | 別レンダリング経路 (`DeferredDetachedImageWindowEvent`)。本 finalize は active context 専用 |

---

## 7. 推奨する修正設計

### 7.1 方針

**「終端かどうか」を field presence から推論するのをやめ、既に存在する typed lifecycle
(`DetachedWindowState::Closing` / `ActiveDetachedSession`) を唯一の根拠にする。**
新しい App-global bool / Option は追加しない (AGENTS.md 制約)。

コードベースには既に「今フレーム detached 窓を生かすべきか = 唯一の述語 (§3.1)」と
doc comment されたメソッドがある:

```rust
// src/app.rs:32633-32637
pub(crate) fn detached_active_window_alive_wanted(&self) -> bool {
    self.active_detached_window_id().is_some() && !self.active_detached_window_is_closing()
}
```

`render_active_detached_viewport_backstop()` は既にこれを使っている。**finalize だけが
この述語を無視している**。

### 7.2 Stage 1 (最小・構造的)

**(a) finalize を typed lifecycle で gate する**

```rust
let detached_viewport_finalized = if app.fullscreen_idx.is_none()
    && !app.detached_active_window_alive_wanted()   // 明示的に終端宣言された窓だけ
    && let Some(viewport_id) = close_viewport_id
{ app.finalize_closed_active_detached_viewport(ctx, viewport_id); true } else { false };
```

`alive_wanted()` は「session が存在し、かつ `Closing` でない」なので:

- internal reopen (PDF/ZIP/変換/scan/password) → session は Active のまま → **finalize しない** ✓
- Esc / × / grid 復帰 / presentation 切替 → `handle_fullscreen_close_request` 等が
  `begin`+`finish` 済み → session None → **finalize する** ✓
- gap 中の Esc → keep-alive cancel が `begin`+`finish` → **finalize する** ✓

**検証済み**: この1行を当てた状態で `cargo test --lib` = **4250 passed / 0 failed / 18 ignored**
(既存 detached lifecycle テスト群を含めて回帰なし)、かつ §8.1 の再現テストが pass する。

> §4 訂正2 で述べた「finalize が `should_drop` の前提を作る」問題は、この gate では発生しない。
> terminal 経路では従来どおり finalize が走って `fs_viewport_shown=false` を立てるため。

**(b) 「内部遷移が未完了か」を単一の述語に集約する**

現在3箇所に複製されている条件列 (app.rs:35362-35374 / 35436-35447 /
ui_fullscreen.rs:6790 の `fs_nav_deferred_reopen_wait_active()`) を1つのメソッドにまとめる:

```rust
/// active detached bundle が「まだ内部遷移の途中」か。
/// 窓を維持すべき理由がこのフレームに1つでもあるなら true。
fn active_detached_transition_outstanding(&self) -> bool {
    self.folder_nav_pending.is_some()
        || self.folder_pane_open_pending.is_some()
        || self.pdf_enumerate_pending.is_some()
        || self.zip_enumerate_pending.is_some()
        || self.fs_nav_deferred_reopen_wait_active()
        || self.pdf_password_request.is_some()
        || matches!(self.bookmark_open_pending, Some(PendingBookmarkOpen::Book(_)))
}
```

適用先:

| 箇所 | 変更後 |
| --- | --- |
| app.rs:35362-35374 (repaint) | `if self.active_detached_transition_outstanding() { request_repaint_after(16ms) }` |
| app.rs:35436-35447 (`should_drop`) | `fullscreen_idx.is_none() && !transition_outstanding() && !fs_viewport_shown` |
| ui_fullscreen.rs:6790 / 6898 (keep-alive) | holdover 分岐と cleanup 分岐の判定を `fs_nav_deferred_reopen_wait_active()` から `active_detached_transition_outstanding()` へ広げる (§6.2 の残存穴を閉じる) |

これで「終端の所有者 = typed lifecycle」「内部遷移の所有者 = 単一述語」となり、
今後 pending が増えても更新漏れが起きるのは1箇所だけになる。

### 7.3 Stage 2 (所有権の一本化。Stage 1 とは別コミット推奨)

現在 terminal close は「`begin_active_detached_session_close` +
`finish_active_detached_session_close` + `remove_detached_window_runtime`」の3点セットを
**12箇所で手書き複製**している (`git grep begin_active_detached_session_close`)。
`DetachedPhysicalFolderOpenPoll::Failed` (app.rs:30512 / 30538) のように宣言を
持たない終端経路もあり、Stage 1 後はそれらが「窓が閉じない」側へ倒れる。

したがって Stage 2 として、terminal close を単一の typed request に集約する:

```rust
enum ActiveDetachedCloseReason {
    UserClose,            // Esc / × / グリッド復帰
    PresentationSwitch,   // detached -> in-window / native video
    OpenFailed,           // required target missing / scan failed / 列挙失敗
    ModeChange,           // 複数ウィンドウモード切替
}
fn request_active_detached_close(&mut self, reason: ActiveDetachedCloseReason);
```

`request_*` が `Closing` 遷移と runtime 予約を行い、`update_active_detached_viewer_context()`
末尾の finalize が唯一の実行者になる。これにより `finalize` が「宣言された終端を実行する」
だけの reducer になり、推論が消える。

> Stage 2 を Stage 1 の前提にはしない。Stage 1 は既存の宣言 (12箇所) だけで整合が取れており、
> `cargo test --lib` 全緑で確認済み。Stage 2 は再発防止のための構造整理であり、
> 実機 smoke を挟んでから着手するのが安全。

### 7.4 やらないこと

- `pdf_enumerate_pending` だけを finalize 条件に足す局所ガード
  (→ ZIP / 変換 / password / scan / bookmark が残る。依頼書 §4 の指摘どおり)
- repaint / delay / retry / 時間窓 / geometry ヒューリスティクスの追加
- detached 用 App-global bool / Option の追加
- window identity の作り直し (BA-4 は今回の原因ではない。`window_id` は
  `close_fullscreen_for_folder_nav_reopen` が正しく保存している)

### 7.5 修正後に実機で確認すべきこと

Stage 1 は「窓が破棄されない」ことまでを保証する。**列挙完了後に次の PDF が同じ窓へ
実際に描画されるか** (§2 の F11 以降) は未検証なので、以下を portable-smoke
(`scripts/prepare-portable-smoke.ps1`) で確認する:

1. PDF → Ctrl+↓ → 次 PDF が**同じ位置・同じサイズ**で開く (window_id 不変)
2. gap 中に直前ページの holdover が見え、黒フラッシュが無い
3. Ctrl+PageDown、ZIP↔PDF の相互移動、末尾での境界ヒント
4. gap 中の Esc / × で確実に閉じる (閉じられない退行が無い)
5. main window の一覧・選択・スクロール・検索状態が不変

---

## 8. 追加すべき回帰テスト

### 8.1 再現テスト (本調査で作成・検証済み。revert 済みなので再投入が必要)

依頼書 §6 の要求1〜5に対応。`src/app/tests.rs` の `still_window_mode_key_tests` に置く。

```rust
#[test]
#[cfg(windows)]
fn detached_pdf_enumerate_gap_keeps_active_window_alive() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let window_id = 21u64;

    // App-global viewport state: the detached window is on screen right now.
    app.settings.detached_viewer_open_images_in_window = true;
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    set_detached_host_for_test(&mut app, window_id, 0x4321, true);
    app.begin_active_detached_session(window_id, DetachedSource::Image);
    assert!(app.active_detached_session.is_some());
    assert!(app.detached_window_runtime_placement(window_id).is_some());

    // Bundle state exactly as produced by
    //   close_fullscreen_for_folder_nav_reopen() + load_folder_nav_target(pdf)
    //   + reopen_fullscreen_after_folder_nav_load() == "enumerate_defer".
    let mut bundle = ViewerContextBundle::empty();
    bundle.navigation_scope = ViewerNavigationScope::DetachedPhysical;
    bundle.current_folder = Some(PathBuf::from(r"C:\books\second.pdf"));
    bundle.fullscreen_idx = None;
    bundle.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
        resume_slideshow: false,
        target: DeferredFsTarget::None,
        resume_to_last_page: false,
        from_explicit_open: false,
        preserve_after_password_prompt: false,
    });
    bundle.viewer_session.presentation = ViewerPresentation::DetachedWindow;
    bundle.viewer_session.independent_active = true;
    bundle.viewer_session.detached_window_id = Some(window_id);
    app.active_detached_viewer_context = Some(ActiveDetachedViewerContext { bundle });

    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        app.update_active_detached_viewer_context(ctx);
    });

    assert!(app.active_detached_session.is_some(),
        "PDF enumerate gap is an internal reopen: the detached session must survive");
    assert!(app.detached_window_runtime_placement(window_id).is_some(),
        "PDF enumerate gap must not remove the detached window runtime");
    assert!(app.fs_viewport_shown,
        "PDF enumerate gap must keep the detached viewport shown");
    assert!(app.active_detached_viewer_context.is_some(),
        "the bundle must stay mounted across the async gap");
}
```

`--lib` で走る (`app::tests::still_window_mode_key_tests::...`)。`--bin mimageviewer-core`
では 0 tests になるので注意。

### 8.2 追加すべきテスト一覧

| # | テスト名 | 初期状態 | 刺激 | assert する不変条件 |
| --- | --- | --- | --- | --- |
| 1 | `detached_pdf_enumerate_gap_keeps_active_window_alive` | §8.1 | 1フレーム | session / runtime / `fs_viewport_shown` / mount が全て生存 |
| 2 | `detached_zip_enumerate_gap_keeps_active_window_alive` | 同上 (ZIP) | 1フレーム | 同上 (`zip_enumerate_pending` 側の対称性) |
| 3 | `detached_folder_scan_gap_keeps_active_window_alive` | `folder_pane_open_pending = Some` | 1フレーム | 同上 (§6.1 の scan 経路) |
| 4 | `detached_pdf_password_gap_keeps_active_window_alive` | `pdf_password_request = Some` | 1フレーム | 同上 (§6.1 の protected PDF) |
| 5 | `detached_gap_survives_multiple_frames` | §8.1 | **3フレーム**連続 | 各フレーム後も session / runtime / window_id が同一。依頼書 §6-3「1フレーム以上 pending」に対応 |
| 6 | `detached_gap_reopen_uses_same_window_id` | §8.1 + 疑似列挙完了 | gap → 完了 → reopen | reopen 後の `active_detached_session.window_id` と ViewportId が gap 前と同一 (依頼書 §6-5) |
| 7 | `detached_explicit_close_still_finalizes_viewport` | active detached、deferred 無し、`handle_fullscreen_close_request()` 済み | 1フレーム | finalize が走り `fs_viewport_shown=false` / session None / runtime 削除 (**修正が terminal 側を弱めていないことの対テスト**) |
| 8 | `detached_gap_escape_finalizes_viewport` | §8.1 + Esc 入力 | 1フレーム | cancel 分岐経由で終端し、窓が閉じる (gap 中に閉じられない退行の防止) |
| 9 | `detached_gap_does_not_touch_main_context` | §8.1 + main に folder/items/visible_indices/filter/selection/scroll/history を設定 | 1フレーム | main 側7項目が全て不変 (依頼書 §6-6) |
| 10 | `detached_folder_nav_boundary_keeps_current_pdf_and_window` | 境界 (`outcome: None`) + PDF 表示中 | 1フレーム | `fullscreen_idx` 不変 + window 生存 + `FsBoundaryHint` (依頼書 §6-7) |

テスト 7 が最重要の対テスト。Stage 1 の gate は「終端宣言が無ければ閉じない」なので、
宣言漏れがあると窓が閉じなくなる。7 と 8 を同時に固定して初めて双方向が守られる。

egui の `ViewportCommand::Close` 送信を直接観測するのは難しいため、
すべて `active_detached_session` / `detached_window_runtime_placement()` /
`detached_window_state()` / `fs_viewport_shown` / `fs_viewport_presentation` の
state transition で assert する (依頼書 §6 末尾の指示どおり)。

---

## 9. 追加ログの提案

### 9.1 現状の不足

`update_active_detached_viewer_context()` の `active_context_state` ログ (app.rs:35307) は
**`frame_counter % 60 == 0` のときだけ**出る。今回の症状は1フレームで完結するため、
決定的なフレームがログに残る確率は約 1/60。実機ログで追えなかったのはこれが原因と思われる。

また `active_close_finalize begin` ログ (app.rs:34488) には **なぜ finalize したのか**
(どの条件で終端と判断したか) が入っていない。

### 9.2 提案する計装

**(a) finalize 判定点で、判定に使った全条件を必ず出す** (app.rs:35428 の直前)

```rust
if Self::detached_image_window_debug_enabled() {
    app.log_detached_image_window_debug(format!(
        "active_close_decision frame={} input_seq={} window_id={:?} viewport={:?} \
         fs_idx={:?} close_vp={:?} alive_wanted={} state={:?} \
         folder_nav={} pane_open={} pdf_enum={} zip_enum={} nav_wait={} pw={} bookmark_book={} \
         shown={} presentation={:?} decision={}",
        app.frame_counter, app.input_seq, app.active_detached_window_id(),
        app.active_detached_session_viewport_id(), app.fullscreen_idx, close_viewport_id,
        app.detached_active_window_alive_wanted(), app.active_detached_window_state(),
        app.folder_nav_pending.is_some(), app.folder_pane_open_pending.is_some(),
        app.pdf_enumerate_pending.is_some(), app.zip_enumerate_pending.is_some(),
        app.fs_nav_deferred_reopen_wait_active(), app.pdf_password_request.is_some(),
        matches!(app.bookmark_open_pending, Some(PendingBookmarkOpen::Book(_))),
        app.fs_viewport_shown, app.fs_viewport_presentation,
        if would_finalize { "finalize" } else { "keep" },
    ));
}
```

**(b) `finalize_closed_active_detached_viewport()` に `reason: &'static str` を追加**
(`"terminal_declared"` / `"inferred_no_fullscreen_idx"` 等)。現在は呼び出し元が1つだが、
Stage 2 で `ActiveDetachedCloseReason` を通す土台になる。

**(c) correlation key を全 detached イベントへ通す**
依頼書 §5.2 の要求どおり `window_id + input_seq` を最低限のキーにする。
既存の `log_detached_image_window_debug` 呼び出しは `window_id` は出すが `input_seq` を
出さないものが多い。`start_folder_nav` / `apply_folder_nav_result` /
`close_fullscreen_for_folder_nav_reopen` / `reopen_fullscreen_after_folder_nav_load` /
keep-alive holdover / backstop / finalize の7点に `input_seq` を追加すれば、
1回の Ctrl+↓ を1本の系列として grep できる。

**(d) `active_context_state` の 60 フレーム間引きを条件付きにする**
`frame_counter % 60 == 0 || app.active_detached_transition_outstanding() ||
app.fullscreen_idx.is_none()` にすれば、遷移中だけ毎フレーム出て平常時は静かなまま。

### 9.3 期待されるログ (正常 / 異常)

```text
# 異常 (現状)
active_close_decision frame=1204 input_seq=87 window_id=Some(3) fs_idx=None close_vp=Some(..)
  alive_wanted=true state=Some(Active) nav_wait=true pdf_enum=true shown=true decision=finalize
active_close_finalize begin ...
session_finish window_id=3 reason=active_close_finalize

# 正常 (Stage 1 適用後)
active_close_decision frame=1204 ... alive_wanted=true nav_wait=true decision=keep
keepalive_backstop / keep_alive_holdover ... window_id=3
(数フレーム後) pdf enumerate done -> open_deferred_fullscreen_after_enumerate window_id=3
```

`decision=finalize` かつ `alive_wanted=true` の行が出たら、それが常にバグである。

---

## 10. 事実の区分

| 区分 | 内容 |
| --- | --- |
| **コードから確認した事実** | finalize の呼び出し条件と実装 (app.rs:35428-35435 / 34483-34524)、`should_drop` との guard 非対称 (§3.4)、`7ee84fdb` での dead code 同居 (`git show`)、`c754a133` が `poll_folder_nav` を closure に入れた唯一のコミット (`git log -S`)、Ctrl+↓ / Ctrl+PageDown が同一 arm に合流すること、terminal 経路 (Esc/×) が session を事前に畳むこと、finalize の production 呼び出しが1箇所であること |
| **テスト実行で確認した事実** | gap 1フレームで session=None / runtime 削除 / `fs_viewport_shown=false` になること (§1.2)。`!fs_nav_deferred_reopen_wait_active()` を足すと解消すること (§1.3)。`!detached_active_window_alive_wanted()` を足すと解消し、`cargo test --lib` 4250 passed / 0 failed であること (§7.2) |
| **未確認 (要実機 / 要追加テスト)** | GUI 実機での再現 (portable-smoke 未実施)。列挙完了後に次 PDF がどこに開かれるか (§2 F11 以降)。RAR/7z/LZH 変換経路の実挙動。protected PDF の実挙動。§6.2 の keep_alive_cleanup 経路が実機で踏まれる具体条件 |
| **推測 (根拠は示したが未検証)** | `7ee84fdb` の二重 finalize が意図的でなく refactor 由来であること。実機ログで原因が追えなかったのが 60 フレーム間引きのためであること |

---

## 11. 次アクション提案

1. 本報告の §7.2 Stage 1 (a)+(b) を実装 (別ブランチ推奨。`docs/detached-rework-plan.md` の凍結ルールに従い、
   detached リワークのステージとして扱う)
2. §8.2 のテスト 1〜10 を追加 (特に 1 / 5 / 7 / 9 は必須)
3. `scripts/prepare-portable-smoke.ps1` で §7.5 の実機 smoke
4. `src/app.rs:34506-34509` のコメントを実装と一致させる
5. Stage 2 (§7.3) は smoke 通過後に別コミットで検討
