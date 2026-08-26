# stage-r2e-2d — 保管・binding・transaction の一括切替

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 全体 (作業環境と②-pre〜②-d-pre の記録) を読むこと。**

**設計が仕様である。** この指示書は設計を要約しない。読むべきは
[briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) の
**§3 (型) / §4 (5 つの transaction) / §5 (不変条件の表) / §7 ②-d**。
この指示書が足すのは **① 現物の作業リスト ② 順序 ③ これまでの段で判明した罠 ④ 完了の門**。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2d)` を含める。

---

## 0. この段だけ性質が違う

②-pre 以降のすべての段は**挙動不変**だった。**この段は違う。**
detached viewer のライフサイクルそのものが変わるので、
**利用者の実機 smoke が要る** (F12 別ウィンドウ / 動画の park・resume / promote / 複数ウィンドウ)。

**分けられない理由**: 保管が 2 箇所で 2 つの問いに答えているのが病因なので、
移行中に 3 つ目の保管を作るのは悪化にしかならない。**1 コミットで切り替える。**

**終われないと思ったら、中途半端に「コンパイルは通る」状態を作らずに報告すること。**
どこまで進んだか・何が残っているか・何が判断できなかったかを書けば、それは失敗ではない。

---

## 1. 現物の作業リスト (実測、`a4380ee0` 時点、非テスト)

存在検査は②-d-pre で名前が付いた。**残っているのは所有操作 59 箇所 / 47 関数。**

### 1.1 `active_detached_viewer_context` — 23 箇所 / 18 関数

| 数 | 関数 | 位置 |
| --- | --- | --- |
| 3 | `park_active_detached_context_as_live_media` | [app.rs:41011](../src/app.rs:41011) |
| 2 | `with_active_detached_viewer_context` | [app.rs:15646](../src/app.rs:15646) |
| 2 | `pause_current_active_detached_viewer_context` | [app.rs:37383](../src/app.rs:37383) |
| 2 | `open_bookmark_media_in_detached_context` | [startup_ops.rs:584](../src/app/startup_ops.rs:584) |
| 1 | `resume_metadata_transfer_context_readers` / `take_and_close_current_active_detached_viewer_context` / `close_all_detached_viewers_for_mode_change` / `pause_local_progress_for_remote_session` / `save_detached_video_resume_positions_for_exit` / `activate_parked_live_media_window_snapshot` / `start_active_detached_book_context_with_start` / `activate_detached_image_window_snapshot` / `update_active_detached_viewer_context` / `route_materialized_physical_still_open_to_active_context` / `promote_active_detached_video_for_main_context_change` / `raise_active_detached_media_for_grid_open` / `should_defer_final_ai_result_for_detached_context` / `add_detached_lanczos_textures` | — |

### 1.2 `paused_bundle` — 36 箇所 / 29 関数

| 数 | 関数 | 位置 |
| --- | --- | --- |
| 3 | `poll_parked_live_detached_windows` | [app.rs:38869](../src/app.rs:38869) |
| 2 | `with_paused_detached_context` / `parked_window_owns_video_tile_companion` / `closing_parked_windows_own_native_video_mode_switch` / `activate_parked_live_media_window_snapshot` / `activate_detached_image_window_snapshot` | — |
| 1 | 残り 24 関数 (`clone` / `transition_detached_window_state` / `detached_window_references_removed` / `consume_deferred_vst3_media_open_in_parked_contexts` / `rehydrate_contexts_after_rename_migration` / `parked_live_media_window_ids` / `detached_music_window_exists` / `tray_resident_media_updates_needed` / … ) | — |

⚠ **1.2 の多くは読み取り走査である** (`parked_live_media_window_ids` /
`detached_music_window_exists` / `parked_live_media_window_exists` /
`parked_live_music_window_info_for_window_id` など)。
これらは設計 §4.6 の `any_viewer_context` / `for_each_viewer_context` に寄る。
**所有を動かす経路と読み取り走査を混ぜないこと。**

---

## 2. 順序 (この順で進める)

**1 コミットにするが、作業は順序を守る。** 各段でコンパイルが通る必要は無い。

1. **registry を production に繋ぐ**: `App::viewer_contexts: ViewerContextRegistry` を足し、
   ステージ①の `ContextTable<P>` を `P = Box<ViewerContextBundle>` で使う。
   `#![allow(dead_code)]` 相当の個別 `#[allow(dead_code)]` 13 個を**外す**
   (②-b で付けたもの。ここで生きたコードになる)。
2. **5 つの transaction を実装**: mount / build / fork / retire / promote (設計 §4)。
   `ContextMut` もここで入る (設計 §3.6、retire の digest 用)。
3. **保管 2 つを削除**: `App::active_detached_viewer_context` と
   `DetachedImageWindowSnapshot::paused_bundle`。**コンパイルエラーが作業リストになる。**
   §1 の 59 箇所がここで全部出る。
4. **window binding を入れる**: 設計 §7 ②-d の対応表 6 行のとおり。
   `bind_window` / `unbind_window` / `transfer_window_binding` /
   `reserve_window_binding_for_build`。
5. **暫定回避策を撤去** (§4)。
6. **監査 A1 / A5 を有効化**し、failpoint を刺す (§5 / §6)。

---

## 3. これまでの段で判明した罠 (設計に載っていないものを含む)

1. ⚠ **「今マウント中の context」を二重に処理しない** (設計 §7 ②-d)。
   ②-pre で追加した
   `all_context_clear_processes_a_paused_context_that_is_already_mounted` は
   **どの context に対して何回走ったかを記録して `[mounted, active, sibling]` と完全一致で
   検証する。このテストを弱めないこと。**
2. ⚠ **②-pre が holder 側に残した前置きフィルタ 2 つ**を `ContextRef` 述語へ移す:
   vst3 の pending 判定 ([app.rs:18810](../src/app.rs:18810)) と rename 述語
   ([app.rs:28058](../src/app.rs:28058))。どちらも `.as_deref().is_some_and(..)` の形。
3. ⚠ **VST3 の parked-live marker は呼び出し元所有のまま**。②-pre が入れた
   panic 安全な復元 (`catch_unwind` → marker 復元 → `resume_unwind`) を
   `residence()` へ置き換えるときに落とさない。テスト
   `parked_vst3_consume_panic_restores_marker_bundle_and_main_projection` が番人。
4. ⚠ **`swap_viewer_context_bundle` の中で rating 同期と visible index 再構築が走る**
   = **op の内部で panic し得る**。①の failpoint sweep は op と op の**境目**にしか
   刺さないのでこれを検出できない。§5 を参照。
5. ⚠ **`ViewerSession` には mounted 側の実体が無い** (②-c1 で判明)。App は
   `last_viewer_sync_stamp` / `detached_viewer_window_id` /
   `detached_viewer_independent_active` を別々のミラーとして持ち、
   `ViewerSession::swap_with_mounted` が対応付ける。
   **「mounted 側は bundle の逐語ミラー」という前提を置かないこと。**
6. ⚠ **「4 つの one-shot がすべて false」という不変条件が、経路によって別の場所で
   維持されている** (②-c1b の調査、§9.5 参照)。passive は park 時に 2 つ、
   activate 時に 2 つ。ParkedLive は activate 時に 4 つ。promote は 3 つ。
   **②-d の mount / activate transaction が 1 箇所で持てば挙動を変えずに揃う。**
   揃えるなら**そのときに揃える**こと。今の集合をコピーして 3 通り作らない。
7. ⚠ **`retire` は `main` が指す context を拒否する** (`RetireError::IsMain`、I4)。
   ①の実装レビューで、main を retire できて `main()` が Retired な id を指す経路が
   実在した。この拒否を外さない。
8. ⚠ **`Retired` と `Unknown` の判定は `highest_reserved_serial` (= `next_serial - 1`)**。
   `highest_committed` では build abort した id が `Unknown` に落ちる (設計 §3.2)。

---

## 4. 撤去する暫定回避策 (設計 §6.5 / §7 ②-d)

**撤去はこの段の完了条件の一部である。**

| # | 場所 | 置き換え先 |
| --- | --- | --- |
| 1 | `right_drag_viewer_identity_for_window_id` の `native_video_parked_live_input_window_id == Some(window_id)` 分岐 | `locate_window_context()` の match |
| 2 | keep-alive backstop の 3 分岐 ([ui_fullscreen.rs](../src/ui_fullscreen.rs)、②-d-pre で `active_detached_context_is_at_rest()` になっている箇所) | 同上。**コード中の「別の detached sentinel を足すな」というコメントに対する正解がこれである** |
| 3 | `mounted_projection_owns_active_detached_session()` の 2 箇所 ([app.rs:41160](../src/app.rs:41160) / [:42772](../src/app.rs:42772) 相当) | `ContextResidence::Mounted` の問い合わせ。②-d-pre のコメントが「holder が空であることは mounted session の所有を証明しない」と書いてある — **それを証明できる形にするのがこの段** |

---

## 5. failpoint と I8

設計 §7 ②-d が名指ししている検査。**①の sweep では足りない。**

- production の `ReplaceProjectionWithFreshEmpty` /
  `RestoreProjectionAndDropDisplacedEmpty` は `swap_viewer_context_bundle` を通り、
  その中で rating 同期と visible index 再構築が走る = **op の内部で panic し得る**。
- **swap の内側にも failpoint を刺し、I8 を確認する**:
  **`Abort` でも panic unwind でも binding が 1 つも公開されない**こと。
- R2e は op panic の完全なロールバックを保証しない。**保証するのは I1b と I8 の 2 つだけ**
  (設計 §7)。それ以外を保証すると書かないこと。

---

## 6. 監査 A1 / A5

`tools/viewer_context_audit` に追加して有効化する (設計 §6.3)。

- **A1**: `ViewerContextBundle` が型位置に出てよいのは registry モジュールだけ。
  `use ... as X` / 型エイリアス / 再エクスポートによる別名も禁止。
- **A5**: 識別子 `paused_bundle` / `active_detached_viewer_context` が存在しないこと。

②-c2 と同じ規律: **fixture テストで弾く例と弾かない例の両方**を書く。
「本番 0 件」を根拠にしない。既存の**既知の指摘 1 件 (`activate_snapshot`) を消さない**こと。

---

## 7. テスト

- **既存 6251 件が第一の番人。** §3-1 / §3-3 のテストを弱めない。
- 設計 §5 の不変条件表で「テスト」列に ○ が付いているもの
  (I1 / I1b / I2 / I3 / I4 / I5 / I6 / I7) に**状態遷移テスト**を足す。
  ステージ①の `ContextTable` テストは production payload に繋がると
  **同じ性質を production 経路で確かめ直す価値がある**。
- **落ちようがないテストを書かない。** 「違反が 0 件」を assert するテストは、
  規則を消しても通る。**必ず「壊れた入力を与えて弾かれること」を見ること。**
- 実機でしか出ない挙動 (HWND / focus / z-order / 実 viewport) はテストにしない。
  **利用者の smoke に回す**ので、何を確認してほしいかを報告に書く。

---

## 8. 完了条件

1. `cargo test -p mimageviewer --lib` が緑。**件数は増える** (§7 の追加分)。
   減っていたら理由を報告する。
2. `cargo fmt --check` が無出力。
3. `cargo run -p viewer_context_audit` が exit 0。
   **A1 / A5 が有効で、既知の指摘は `activate_snapshot` の 1 件のまま。**
4. `cargo test -p viewer_context_audit` が緑 (A1 / A5 の fixture を含む)。
5. `cargo check -p mimageviewer --bin mimageviewer-core` が exit 0。
6. 次が **0 件**:

   ```
   rg -n "active_detached_viewer_context|paused_bundle" src/ -g "!viewer_context_registry.rs"
   ```

   (A5 が機械的に同じことを見るが、実出力も貼る。)
7. `git diff --numstat HEAD` を貼る。

---

## 9. 報告に必ず含めること

- §8 の 7 項目の実出力。
- **§3 の 8 つの罠それぞれについて、どう扱ったか。**「該当しなかった」も回答である。
- **§4 の暫定回避策 3 種が撤去されたことの、コード上の証拠。**
- **利用者に実機で確認してほしいシナリオの一覧** (§7 末尾)。具体的な操作手順で書く。
- 設計と食い違う点が見つかったら、**回避する前に報告する**。
  設計は第 3 版で 6 巡のレビューを通っているが、無謬ではない。
- **終わらなかった場合は、終わらなかったと書く。** 途中経過と残りの一覧を出すこと。
  コンパイルを通すためだけの妥協を入れない。
