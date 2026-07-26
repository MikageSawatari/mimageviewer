# 実装ブリーフ: detached PDF の Ctrl+↓ で window が閉じる回帰の根本修正 (Stage 1)

作成日: 2026-07-26
対象: mImageViewer v2.8.0 / `master` (HEAD `bd0fadb0`)
実装: Codex Sol / 設計・検収: ClaudeCode
関連: `docs/detached-folder-nav-pdf-claudecode-investigation-request.md` (依頼)、
`docs/detached-folder-nav-pdf-claudecode-investigation-report.md` (調査報告・**先に読むこと**)

---

## 1. 症状と根本原因 (調査済み・確定)

複数ウィンドウモードの independent detached 静止画 viewer で PDF 表示中に Ctrl+↓ を押すと、
同じフォルダに次の PDF があるのに detached window が閉じる。

根本原因は `src/app.rs:35428-35435`、`update_active_detached_viewer_context()` 末尾の
**ungated な finalize 呼び出し**:

```rust
let detached_viewport_finalized = if app.fullscreen_idx.is_none()
    && let Some(viewport_id) = close_viewport_id      // フレーム冒頭 (35304) で保存
{
    app.finalize_closed_active_detached_viewport(ctx, viewport_id);
    true
} else { false };
```

`fullscreen_idx.is_none()` + フレーム冒頭の `close_viewport_id` という2つの field presence
だけで「ユーザーが viewer を終了した」と推論している (BA-7)。PDF/ZIP の非同期列挙を挟む
internal reopen では両方が同一フレーム内で成立するため、`ViewportCommand::Close` +
`finish_active_detached_session_close` + `remove_detached_window_runtime` が実行される。

1フレーム内の順序 (確定済み):

```
35304  close_viewport_id = Some(vp)                      <- fs_viewport_shown=true なので保存
35330  poll_folder_nav -> apply_folder_nav_result
45468    close_fullscreen_for_folder_nav_reopen()        <- fullscreen_idx=None (shown は true 維持)
14842    load_folder_nav_target(pdf)                     <- pdf_enumerate_pending=Some
30791    reopen_fullscreen_after_folder_nav_load()       <- "enumerate_defer"、fs_nav_after_pdf_enumerate=Some
35421  keep_fullscreen_viewport_alive()                  <- holdover 分岐で「窓を維持する」と決定して return
35428  ★ finalize_closed_active_detached_viewport()      <- 2行後に「終端だ」と決めて窓を破棄
35436  should_drop = false (deferred 待ちのため)          <- bundle は mount されたまま残る
```

**2行離れた位置で、片方が「gap 中だから窓を維持」と決め、もう片方が「`fullscreen_idx` が
無いから終端」と決めて後者が勝っている。**

導入は `7ee84fdb` (2026-06-28)。当時この関数には「無条件版」と「`should_drop` gated 版」の
finalize が両方書かれており、無条件版が先に走るため gated 版 (= 意図された正しい方、
`src/app.rs:34506-34509` のコメントが説明している方) が構造的に dead だった。`4c2a9e7c` は
その dead code = 正しい方を削除した。`c754a133` が `poll_folder_nav()` を本 closure へ入れた
ことで、壊れていた終端前提へ folder-nav が到達可能になった。

### guard の非対称な蓄積 (= 直すべき構造)

| 条件 | `should_drop` (35436) | repaint 条件 (35362) | finalize (35428) |
| --- | --- | --- | --- |
| `folder_nav_pending.is_none()` | ✓ | ✓ | ✗ |
| `folder_pane_open_pending.is_none()` | ✓ | ✓ | ✗ |
| `pdf_enumerate_pending.is_none()` | ✓ | ✓ | ✗ |
| `zip_enumerate_pending.is_none()` | ✓ | ✓ | ✗ |
| `!fs_nav_deferred_reopen_wait_active()` | ✓ | ✓ | ✗ |
| `pdf_password_request.is_none()` | ✓ | ✓ | ✗ |
| `bookmark_open_pending != Book` | ✓ | ✓ | ✗ |

同じ「内部遷移が未完了か」という概念が3箇所に手書き複製され、1つ (finalize) だけ更新から
取り残された。これが再発の構造要因。

---

## 2. 実装する変更 (Stage 1)

> **重要**: これは「PDF のときだけ finalize しない」という局所ガードではない。
> 「終端かどうかは typed lifecycle だけが決める」「内部遷移中かどうかは単一述語だけが決める」
> という所有権の修正である。`pdf_enumerate_pending` を finalize 条件へ足すだけの実装は却下する。

### 2.1 (a) 内部遷移述語を1つに集約する

`src/app.rs` に `#[cfg(windows)]` の helper を追加する (呼び出し元がすべて cfg(windows) のため)。

```rust
/// active detached bundle が「まだ内部遷移の途中」か。
/// このフレームで detached window を生かしておく理由が1つでもあれば true。
///
/// 同じ条件列が repaint 要求 / should_drop / keep-alive の3箇所に複製されており、
/// finalize だけが更新から取り残されて 2026-07 の「Ctrl+↓ で detached window が閉じる」
/// 回帰を生んだ。以後 pending を増やすときはこの1箇所だけを更新すること。
#[cfg(windows)]
pub(crate) fn active_detached_transition_outstanding(&self) -> bool {
    self.folder_nav_pending.is_some()
        || self.folder_pane_open_pending.is_some()
        || self.pdf_enumerate_pending.is_some()
        || self.zip_enumerate_pending.is_some()
        || self.fs_nav_deferred_reopen_wait_active()
        || self.pdf_password_request.is_some()
        || matches!(
            self.bookmark_open_pending,
            Some(crate::bookmark_browser::PendingBookmarkOpen::Book(_))
        )
}
```

適用先 (**条件を変えず、重複を消すだけ。挙動は不変であること**):

- `src/app.rs:35362-35374` の `request_repaint_after` 条件 → `if app.active_detached_transition_outstanding() { ... }`
- `src/app.rs:35436-35447` の `should_drop` →
  `app.fullscreen_idx.is_none() && !app.active_detached_transition_outstanding() && !app.fs_viewport_shown`

### 2.2 (b) finalize を typed lifecycle で gate する ← **本丸**

`src/app.rs:35428` を次にする。

```rust
let detached_viewport_finalized = if app.fullscreen_idx.is_none()
    && !app.detached_active_window_alive_wanted()
    && let Some(viewport_id) = close_viewport_id
{
    app.finalize_closed_active_detached_viewport(ctx, viewport_id);
    true
} else { false };
```

`detached_active_window_alive_wanted()` (`src/app.rs:32633-32637`) は
「session が存在し、かつ `DetachedWindowState::Closing` でない」= 既に
「今フレーム detached 窓を生かすべきか = **唯一の述語** (§3.1)」と doc comment されており、
`render_active_detached_viewport_backstop()` は既にこれを使っている。
**finalize だけがこの述語を無視していた。**

この gate で各経路がどうなるか (すべて確認済み):

| 経路 | session 状態 | finalize |
| --- | --- | --- |
| PDF/ZIP/変換/scan/password の internal reopen | Active のまま | **走らない** ✓ |
| Esc / × / グリッド復帰 (`handle_fullscreen_close_request`, app.rs:45343-45356) | `begin`+`finish` 済み → None | 走る ✓ |
| gap 中の Esc / × (keep-alive cancel, ui_fullscreen.rs:6863-6889) | `begin`+`finish` 済み → None | 走る ✓ |
| presentation 切替 (`open_non_detached` 他) | `begin_active_detached_session_close` 済み | 走る ✓ |
| `keep_alive_cleanup` (ui_fullscreen.rs:6905) | `begin`+`finish` 済み → None | 走る ✓ |

> **順序を入れ替えてはいけない**: `close_fullscreen()` は `fs_viewport_shown` を意図的に
> true のまま残す仕様 (`src/app.rs:45523-45527` の doc comment)。finalize が先に走って
> `fs_viewport_shown=false` を立てないと `should_drop` の `!fs_viewport_shown` が永久に
> 成立しない。finalize を `should_drop` の後ろへ動かす修正は「bundle が drop されない」
> 別バグを作る。現在の順序を保つこと。

### 2.3 (c) keep-alive の同型ホールを塞ぐ (detached にスコープを限定して)

`src/ui_fullscreen.rs:6779-6961` の `keep_fullscreen_viewport_alive()` は、
内部遷移の判定に `fs_nav_deferred_reopen_wait_active()` **しか**使っていない。
そのため `pdf_enumerate_pending = Some` かつ `fs_nav_after_pdf_enumerate = None`
(= folder-nav 由来ではない detached コンテナ open。`4c2a9e7c` "Fix detached container open
lifecycle" が扱った領域)、`folder_pane_open_pending = Some`、`pdf_password_request = Some`
のいずれでも、cleanup 分岐 (6898-6914) が session を畳んで runtime を消す。
**finalize だけ直してもこの経路が残ると同種の症状が別条件で再発する。**

holdover 分岐 (6790) の条件を次のように広げる:

```rust
#[cfg(windows)]
let detached_transition_hold =
    self.active_detached_session.is_some() && self.active_detached_transition_outstanding();
#[cfg(not(windows))]
let detached_transition_hold = false;

if self.fs_viewport_shown
    && (self.fs_nav_deferred_reopen_wait_active() || detached_transition_hold)
{ ...既存の holdover 描画... return; }
```

**`active_detached_session.is_some()` によるスコープ限定は必須**。
`keep_fullscreen_viewport_alive` は main context (`src/app.rs:58438`) からも呼ばれる。
そこで無条件に `active_detached_transition_outstanding()` を使うと、main の
fullscreen 終了フレームで無関係な `pdf_enumerate_pending` が残っていた場合に
「Esc でフルスクリーンを抜けられない」退行を作る。detached session がある場合だけ
holdover を維持すること。

`self` は `App` なので `active_detached_transition_outstanding()` をそのまま呼べる。
non-windows ビルドを壊さないよう cfg を正しく扱うこと (CI に ubuntu の `cargo check` がある)。

---

## 3. やってはいけないこと (AGENTS.md / CLAUDE.md 制約)

- `pdf_enumerate_pending` だけを finalize 条件へ足す局所ガード
- 症状を隠す repaint / delay / retry / 時間窓 / geometry ヒューリスティクスの追加
- detached 用の App-global bool / `Option` を**新規に追加**すること
  (今回追加するのはメソッド1つだけ。フィールドは増やさない)
- `fullscreen_idx` / pending field / shown flag の有無を terminal intent の代用にすること
- window identity (`window_id` / ViewportId / `fs_viewport_generation`) の作り直し
  (BA-4 は今回の原因ではない。`close_fullscreen_for_folder_nav_reopen` の保存処理は正しい)
- main context / active A / passive B の request・result・session・runtime を交差させること
- 明示 close (Esc/×)、internal reopen、失敗による terminal close を同じ曖昧な分岐へ寄せること
- **`git commit` / `git add` / branch 操作**。作業ツリーに変更を置くところまで。
  (このリポジトリは複数セッションが同じ作業ツリーを共有している)
- 下記「4. 触ってよいファイル」以外の変更

## 4. 触ってよいファイル

- `src/app.rs`
- `src/ui_fullscreen.rs`
- `src/app/tests.rs`
- `docs/detached-rework-stage-folder-nav.md` (§5 参照)

これ以外に変更が必要だと判断した場合は、**変更せずに理由を報告**すること。
`git status` に上記以外の差分が見えても、それは別セッションの作業なので**絶対に触らない**
(`git checkout` / `git restore` / `git stash` を使わない)。

---

## 5. ドキュメント更新 (必須)

1. `src/app.rs:34506-34509` の `finalize_closed_active_detached_viewport()` 内コメントを
   実装と一致させる。現在は「ここは should_drop 経路でのみ呼ばれる」と書いてあるが事実と
   異なっていた。新実装に合わせて
   「**terminal close が明示的に宣言された (session が finish 済み、または `Closing`) ときだけ
   呼ばれる。internal reopen (folder-nav / PDF・ZIP 列挙 / scan / password) 中は
   `detached_active_window_alive_wanted()` が true なので呼ばれない**」という趣旨に書き直す。
2. `docs/detached-rework-stage-folder-nav.md` に、
   「terminal close の唯一の根拠は `detached_active_window_alive_wanted()`
   (= session 存在 && not `Closing`) であり、`fullscreen_idx` の有無から推論しない」
   「内部遷移中かどうかは `active_detached_transition_outstanding()` が唯一の述語で、
   pending を増やすときはここだけを更新する」という不変条件を追記する。

---

## 6. 追加する回帰テスト

`src/app/tests.rs` の `still_window_mode_key_tests` モジュールに置く
(`cargo test --lib` で走る。`--bin mimageviewer-core` では 0 tests になるので注意)。

egui の `ViewportCommand::Close` は直接観測できないので、
`active_detached_session` / `detached_window_runtime_placement()` /
`detached_window_state()` / `fs_viewport_shown` / `fs_viewport_presentation` の
state transition で assert すること。

### 6.1 必須 (この4本は必ず入れる)

**T1 `detached_pdf_enumerate_gap_keeps_active_window_alive`** — 本回帰の再現テスト。
ClaudeCode 側で作成・実行して FAIL → 修正で PASS を確認済み。そのまま使ってよい:

```rust
#[test]
#[cfg(windows)]
fn detached_pdf_enumerate_gap_keeps_active_window_alive() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let window_id = 21u64;

    app.settings.detached_viewer_open_images_in_window = true;
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    set_detached_host_for_test(&mut app, window_id, 0x4321, true);
    app.begin_active_detached_session(window_id, DetachedSource::Image);
    assert!(app.active_detached_session.is_some());
    assert!(app.detached_window_runtime_placement(window_id).is_some());

    // close_fullscreen_for_folder_nav_reopen() + load_folder_nav_target(pdf)
    // + reopen_fullscreen_after_folder_nav_load() == "enumerate_defer" 直後の状態。
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

**T2 `detached_gap_survives_multiple_frames`** — T1 と同じ初期状態で
`update_active_detached_viewer_context` を **3フレーム連続**で回し、各フレーム後に
session / runtime / `window_id` / ViewportId が同一のまま生存することを assert。
(依頼元の要求「PDF enumerate を意図的に1フレーム以上 pending にする」に対応)

**T3 `detached_explicit_close_still_finalizes_viewport`** — **対テスト。最重要。**
active detached + deferred なし + `handle_fullscreen_close_request()` 実行済みの状態で
1フレーム回し、finalize が走って `fs_viewport_shown == false` /
`fs_viewport_presentation == None` / session None / runtime 削除になることを assert。
この修正は「終端宣言が無ければ閉じない」方向なので、宣言漏れがあると
**窓が閉じなくなる**逆の退行が起きる。T1 と T3 を同時に固定して初めて双方向が守られる。

**T4 `detached_gap_does_not_touch_main_context`** — T1 の初期状態に加えて main 側へ
`current_folder` / `items` / `visible_indices` / `selected` / `scroll_offset` /
検索・フィルタ状態 / 履歴を設定し、1フレーム後にそれらが**すべて不変**であることを assert。

### 6.2 できれば入れる

- **T5 `detached_zip_enumerate_gap_keeps_active_window_alive`** — `zip_enumerate_pending` 側の対称性。
- **T6 `detached_folder_scan_gap_keeps_active_window_alive`** — `folder_pane_open_pending` 側 (§2.3 の穴)。
- **T7 `detached_pdf_password_gap_keeps_active_window_alive`** — `pdf_password_request` 側。
- **T8 `detached_gap_escape_finalizes_viewport`** — gap 中の Esc で確実に閉じること
  (keep-alive cancel 分岐経由)。閉じられなくなる退行の防止。
- **T9 `detached_folder_nav_boundary_keeps_current_pdf_and_window`** — 境界 (`outcome: None`) で
  `fullscreen_idx` 不変 + window 生存 + `FsBoundaryHint` が出ること。

T5〜T7 が構造上書きにくい (pending の構築に外部リソースが要る等) 場合は、
**無理に書かず理由を報告**すること。T1〜T4 は必須。

---

## 7. 検証

```bash
cargo fmt
cargo fmt --check
cargo test --lib
python scripts/check_ui_glyphs.py   # UI 文言を触った場合のみ
```

- `cargo test --lib` は修正前ベースラインが **4250 passed / 0 failed / 18 ignored**。
  追加テスト分だけ passed が増え、**failed は 0 のままであること**。
- ClaudeCode 側で「2.2 (b) の1行だけ」を当てた状態の `cargo test --lib` が
  4250 passed / 0 failed だったことは確認済み。既存 detached lifecycle テスト群に
  回帰が出たら、それは (a)/(c) の実装ミスを意味する。
- `cargo test --workspace` や `build-dist.ps1` は不要。

## 8. 報告してほしいこと

1. 変更した各ファイル・シンボルと、その変更が §2 のどれに対応するか
2. `cargo fmt --check` と `cargo test --lib` の結果 (passed/failed の実数)
3. 追加したテスト名と、それぞれが固定している不変条件
4. §2.3 の keep-alive 変更で挙動が変わりうると判断した経路と、その根拠
5. 書けなかったテスト・実装できなかった項目とその理由
6. 実装中に見つけた**別の**同型の推論箇所 (あれば。直さず報告だけでよい)
