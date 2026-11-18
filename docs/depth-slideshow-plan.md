# 3D 写真のままスライドショー (次のページの奥行きの先読み) — 設計書 (§1.771)

状態: **設計第 8 版 (2026-11-18)。独立設計レビュー ACCEPT (第 5 回で全体、第 8 回で利用者の決定を反映した差分)。利用者の決定は §10。実装完了、独立実装レビュー ACCEPT (第 3 回)。実機確認は未実施**。
worktree `G:\home\mimageviewer-771`、ブランチ `feat-771-depth-slideshow` (master `96ed160a0` から)。
3D 写真の全体設計は [depth-parallax-plan.md](depth-parallax-plan.md) が正本。本書はその §1「v1 に含めない: 先読み」、
§6.3「ON にするときの後始末: スライドショー停止」、§6.5 と §10 段 B の全箇所分類表 (topbar・リング・S キーの抑止) を
変える差分だけを書く。実装後に正本の該当節へ要約と本書へのリンクを足す。

## 1. 要望と現状

- 要望 (利用者 2026-11-12、バックログ §1.771): 3D 表示中にスライドショーを使えるようにする。1 つ先のページを先読みし、
  3D のまま滑らかに切り替える。
- 現状 (コード):
  - 3D に入ると `enter_depth_parallax` (`src/app/depth_view.rs:339`) が `close_fs_exclusive_conflicts`
    (`src/app.rs:96863`、360° と共用) を呼び、スライドショーを止める。
  - 3D 中は S キー (`ui_fullscreen.rs:36440`)、topbar の再生ボタン (`ui_fullscreen.rs:52888`) と「…」
    (`ui_fullscreen.rs:52745`) が、表示を占有するモードの述語 `fs_exclusive_view_active(..).is_some()` で抑止される
    (360° でも抑止)。リングの `ImageSlideshow` は **3D のときだけ**抑止される (`gamepad_input.rs:8366`。360° 中は通常どおり使える)。
  - 3D のページ送りでは、新しいページの表示テクスチャが安定してから深度を作る。深度の入力は表示中のテクスチャの GPU 読み戻し
    (正本 §4.1・§4.2)。推論は dGPU で 30〜50 ms、内蔵 GPU で約 1 秒、CPU で約 3.6 秒 (初回はさらに 2〜3.5 秒)。
  - したがって今のまま自動送りを許すと、毎ページ「平面 → 計算中の箱 → 立体」の切り替わりが見える。
- スライドショーの仕組み (コード):
  - 状態 (`slideshow_playing`・`slideshow_next_at`・`slideshow_anchor_idx` ほか) は viewer context ごと
    (`ViewerContextBundle`、`viewer_context_registry.rs:2095`)。
  - 送るかどうかは `handle_fs_navigation_with_render_perf` のタイマー (`ui_fullscreen.rs:44666`) が毎フレーム決め、
    期限を過ぎたら `advance_slideshow` (`ui_fullscreen.rs:42312`) が次の位置を選んで `accept_book_slideshow_target` で開く。
    スライドショーの送りは `NavigationSequence` を作らない (手動送りは作る、`ui_fullscreen.rs:44578`)。
  - 間隔は「今のページが表示できた時点」から数える。`sync_slideshow_anchor_for_frame` (`ui_fullscreen.rs:42638`) が
    `current_slideshow_frame_ready` を満たした時点で期限を設定する。次のページの準備を待つ仕組みは無い。
  - 送り先は `get_still_image_indices` (動画・音声を除く。アニメーション画像は含む) の次。末尾では設定どおり
    停止・先頭へ戻る (`loop_slideshow_to_first`)・次のフォルダ (`try_start_slideshow_next_folder`)。
    コレクションの直接登録画像では、送り先を非同期の要求で最新の一覧から決める (`start_collection_slideshow_navigation`)。
  - **フォルダ内の手動のページ送りはスライドショーを止めない** (`ui_fullscreen.rs:44516`、keymap-spec)。
  - 次のフォルダへの移動は全画面をいったん閉じて開き直す。全画面を閉じる処理 (`close_fullscreen`、`src/app.rs:82856`) が
    3D を終えるので、**今は 3D がトーストなしで終わり、スライドショーは 2D で続く**。
- 次のページの表示テクスチャを表示前に作る仕組み (コード):
  - 補正なしのページは、元画像の先読み (`prefetch_forward`、既定 12) のテクスチャを final が共有する (同じ TextureId、
    `shared_resident_texture_for_edit`、`app.rs:88165`)。
  - 補正・ポストフィルタ・局所補正・隠蔽・注釈ありのページは、**紙めくり ON のときだけ** D3 の次ユニット準備
    (`prefetch_next_final_unit`、`app/final_prepare.rs:426`) が作る。OFF では移った後に UI スレッドで作るので、テクスチャは移動後にできる。
  - `resident_final_prepare_material` (`app/final_prepare.rs:147`) は、通常の表示が採用するテクスチャを、アップロードや I/O なしで返す
    (紙めくりの受付と canonical demand の照合で使用中)。

## 2. 目標

1. 3D のままスライドショーを開始・継続できる。3D に入ってもスライドショーを止めない。
2. 自動送りで同じ一覧の次のページへ移った最初のフレームから、そのページを 3D で描く (平面 → 立体の切り替わりを見せない)。
   対象外 (切り替わりが見えてよい) とするのは §5.8 の場合だけ。
3. 次のページの深度が間に合わなければ自動送りを待たせ、今のページの 3D を表示し続ける。待ちに時間の上限は置かず、作る仕事が無い・
   失敗・取消で終端したら送る (§5.4・§5.4.1)。永久に止まらない。
4. 手動のページ送り・360°・書き出し・別ウィンドウ・紙めくりの既存の挙動を変えない。

## 3. 状態の組み合わせを減らす検討 (CLAUDE.md の規則)

| 案 | 採否 | 理由 |
| --- | --- | --- |
| 先読みの持ち主を 1 つにする (3D の context 所有の `DepthViewState` に「次」の枠を 1 つだけ足す) | **採用** | 先読みの寿命・取消・VRAM の会計が 3D の状態と同じ所有者に入り、3D を終えれば枠ごと消える。別の queue・map・ready フラグを作らない |
| 「準備できた」を別の状態にせず、既存の深度 LRU に入っているかで判定する | **採用** | 結果の置き場所は LRU だけ。枠は「計算中」だけを持ち、完了したら LRU に入れて枠を空にする |
| 送ると準備の照合を 1 か所にする (`advance_slideshow` で送り先を決めた直後) | **採用** | 「送る予定」の pending を作らない。待つときは何も変えず、期限切れのタイマーが次のフレームでもう一度同じ照合をする |
| 「今のページを 3D で見せた」事実を 1 つにし、間隔の開始と D3 の開始の両方で使う | **採用** (§5.7) | 描画時に記録した 1 つの事実を両方が読む。phase だけで判断する述語を 2 つ作らない (第 1 回レビュー P2-5) |
| 次のページの素材は、紙めくりと同じ D3 の次ユニット準備で作り、有効条件を 1 つの述語にまとめる | **採用** (§5.6) | 3D 専用の final 生成を書かない。D3 の需要生成・開始・保持・採用が同じ述語を読む (第 1 回レビュー P1-1) |
| 推論の順番: 今のページを先に、次のページは今のページの深度が終わってから | **採用** | worker は同じ context の新しい要求で実行中の要求を取り消す (`depth_estimation.rs:249`)。優先度の仕組みを足さない |
| 先読みは 3D のスライドショー中だけ。手動のページ送りでは新しく先読みを始めない | **採用** | 正本 §1 の判断 (dGPU では不要) を変えない。「次」が一意に決まるのはスライドショーだけ |
| 3D 中のスライドショーでアニメーション画像を飛ばす | 不採用 (§10 の 2 で「着地して 3D を終える」に決定) | 飛ばすと、送り先が D3 の次ユニット・元画像の先読み範囲とずれ、両方へ同じ送り先を渡す改修が要る (第 1 回レビュー P2-3)。着地すれば既存の継続条件だけで閉じる |
| 間に合わないページで、前のページの 3D を表示したまま新しいページへ移る | 不採用 | 表示中のページと位置の所有者が食い違う状態を作る |
| 待ち中をモーダルにする | 不採用 | 待ちはスライドショーの進行を遅らせるだけで、利用者の操作と食い違う状態は無い |
| 次のフォルダ・コレクションの次・先頭への折り返し・分割表示も先読みする | 不採用 (§5.8) | 先読みの対象が既存の先読み範囲・D3 の次ユニットの外にあり、所有と組み合わせが増える |

## 4. 変える入口 (3D とスライドショーの共存)

1. **3D の入場でスライドショーを止めない。** `close_fs_exclusive_conflicts` のスライドショー停止を、360° の入場だけで行う形に分ける
   (360° の挙動は変えない)。3D の入場時の他の後始末 (補正パネル等を閉じる) はそのまま。
2. **3D 中もスライドショーの操作を受け付ける。**
   - S キー・topbar の再生 / 一時停止ボタン: 抑止の述語を `fs_exclusive_view_active(..).is_some()` から 360° 固有
     (`Some(ExclusiveViewKind::Panorama)`) に変える。360° 中は今までどおり抑止。
   - リング: 3D 専用の抑止 (`gamepad_input.rs:8366`) から `ImageSlideshow` だけを外す。360° 中のリングは今までどおり使える
     (第 1 回レビュー P2-7)。
   - topbar の「…」(スライドショーの設定の小パネルを含む) は、3D 中は今までどおり出さない。間隔などの設定は 3D に入る前に行う。
     「…」には 3D と両立しない項目もあり、項目ごとの分類は今回の範囲外にする。
3. **開始の条件は今までどおり** (`toggle_fullscreen_slideshow`: 静止画のページで開始)。

## 5. 先読みと送りの照合

### 5.1 先読みの対象 (`InItems`) と、その送り先を決める関数 (1 つ)

`advance_slideshow` から、送り先を決める部分を関数に切り出す。自動送り・先読み・照合が同じ答えを使う。

```rust
pub(crate) enum SlideshowNextTarget {
    /// 同じ一覧の中の、順番どおりの次の静止画 (末尾の折り返しを除く)。先読みと照合の対象
    InItems { idx: usize },
    /// それ以外 (分割表示、コレクションの要求、末尾の停止・折り返し・次のフォルダ)。先読みも照合もしない
    Other,
}
fn slideshow_next_target(&self, cur: usize) -> SlideshowNextTarget
```

- `InItems` になるのは、分割表示でなく (`spread_mode.is_split()` が偽)、コレクションの要求を出さない一覧で、
  `adjacent_idx_in_order(still, cur, 1)` が次を返すときだけ。3D は 1 ページ表示・ページ表示 (連結読みでない) に限られる。
- 送り先の選び方 (アニメーション画像を含む) は今までと同じ。3D 中に変えない。
- この範囲は、D3 の次ユニット (`next_final_prepare_occurrences`。スライドショーの静止画の送り先を使う、`ui_fullscreen.rs:21470`) と、
  元画像の先読み範囲 (前方 `prefetch_forward` 枚) と一致する。実装担当は一致をテストで固定する (§8)。

### 5.2 先読みの持ち主

`DepthViewState` (context 所有) に枠を 1 つ足す。

```rust
pub(crate) struct DepthNextSlot {
    pub target: DepthNextTarget,          // idx・item の識別子・items_generation
    pub key: DepthSourceKey,              // 送り先の表示テクスチャの id
    pub phase: DepthNextPhase,
}
pub(crate) enum DepthNextPhase {
    ReadingBack { readback: DepthReadback },
    Estimating { pending: DepthPending }, // Drop で取消 (既存)
    Failed { reason: String },            // 同じ key は再試行しない (今のページの Failed と同じ規則)
}
pub next: Option<DepthNextSlot>,
```

- 完了した深度は LRU (既存、4 件) に入れ、枠を `None` に戻す。「準備できた」は **LRU に `key` があること**だけで表す。
- 枠を持つのは `FsExclusiveView::Depth` の間だけ。3D を終えれば `DepthViewState` の Drop で消える (取消は既存の Drop)。
  park / fork / F12 / close は既存の Depth の退役を通るので、枠のための追加の経路は要らない。
- VRAM の会計: 読み戻し中の buffer は `readback_bytes` に、完了した深度は既存の `depth_bytes` (LRU) に入る。

### 5.3 毎フレームの進め方 (`advance_depth_source` の中、今のページの選択と進行の**後**)

1. **枠を残すか**: 次のどちらかなら残す。どちらでもなければ捨てる (Drop で取消)。
   - (a) 枠の送り先が、今の「望む送り先」(下の 2) と同じ。
   - (b) 枠の送り先が今のページ (`fullscreen_idx`・item の識別子・items_generation が一致) で、今のページの素材がまだ決まっていない
     (このパスの素材が `NotYet`、または選んだ `key` が枠の `key` と同じ)。手動で送り先へ移ったとき、`NavigationSequence` の
     `PageTransition` の間に枠を捨てないため (第 1 回レビュー P2-6)。素材が決まって `key` が違えば捨てる。
2. **望む送り先**: 次をすべて満たすときだけ `Some(idx)`。
   - `slideshow_playing`
   - 今のページについて §5.7 の「3D で見せた / 失敗が確定した」事実がある (今のページの深度が終わっている)
   - `slideshow_next_target(cur)` が `InItems { idx }`
   - 書き出しの操作中でない (`depth_export_operation.is_none()`)。窓が見えている (既存の hidden / minimized の早期 return と同じ)
   - 透過背景のモードが既定 (`fs_transparent_bg_mode == 0`)。§5.9 の「背景モード」を参照。
3. **送り先の素材**: `resident_final_prepare_material(ctx, idx)` の元テクスチャ。無ければ待つ (枠は作らない)。
   renderer に backing が無ければ待つ。
4. **照合と進行** (望む送り先があるとき):
   - LRU に `key` があれば何もしない (準備済み)。
   - 枠が同じ送り先と `key` なら枠を進める: `ReadingBack` は既存と同じ非ブロッキング poll と repaint、完了したら同じ worker へ
     `submit` (今のページは終わっているので何も取り消さない)、`Estimating` は `try_result` で受け取り、R16 を上げて LRU へ入れ、
     枠を空にする。R16 のアップロードは今のページと共有の「1 フレーム 1 枚」(`last_depth_upload_frame`) に従う。
   - 枠が無い・別の送り先・別の `key` なら作り直す。今のページと同じ経路で一時的な色テクスチャ (`copy_color`) を作って
     `readback` を出し、色テクスチャはすぐ捨てる (submit 後に捨ててよい)。同じ経路なので、先読みした深度は送った後に
     計算した場合と同じ結果になる。
5. 今のページが新しい要求を出す場合 (素材が変わった等)、worker の既存の取消で枠の要求は止まる。そのとき 1 の規則で枠も捨てる
   ((a) は 2 の条件を満たさなくなり、(b) は送り先が今のページでない)。
6. perf: 先読みの開始・読み戻し完了・推論完了・LRU 投入に `perf::event("depth", "next_prefetch", ...)` を置く (idx・key・所要時間)。

### 5.4 送るときの照合

`advance_slideshow` の中で、送り先が `InItems { idx }` に決まり `accept_book_slideshow_target` を呼ぶ直前に、
3D 中だけ次の 1 つの関数で照合する。`InItems` でない送り先 (§5.8) は照合せず今までどおり送る。

```rust
enum DepthSlideshowGate { Go, Wait }
fn depth_slideshow_gate(&mut self, ctx, idx, now) -> DepthSlideshowGate
```

- 3D 中はこの照合を含む自動送り全体を、所有 viewer の見えている描画パスでだけ評価する (§5.7.1)。見えていないパスや root の
  キー入力の転送では評価しない。
- **待ちに時間の上限は置かない** (利用者決定 2026-11-18、§10 の 1)。代わりに、待つのは「送り先の素材か深度を作る、終わりが保証された
  仕事が動いている間」だけにする。仕事が無い・失敗した・取り消された状態は終端として送る (§5.4.1)。永久に止まらないことは、
  時間ではなく終端の分類で保証する。
- `Go`: 3D でない / 送り先の素材のテクスチャの `key` が LRU にある / 送り先が読み込み済みでアニメーション (`fs_entry_is_animated`。
  着地して既存の継続条件で 3D を終える、§10 の 2) / **先読みの対象外** (透過背景のモードが既定でない、書き出しの操作中。
  §5.3 の 2 のうち、待っても状態が変わらない条件) / §5.4.1 の分類で送り先が `Unavailable` (作る仕事が無い・始まっていない・終端した)。
- `Wait`: §5.4.1 の分類で今のページか送り先が `Producing`。`advance_slideshow` は何も変えずに戻る。
  `slideshow_next_at` は過ぎたままなので、次のフレームで同じ照合をする。タイマーの既存の
  `request_repaint_after(remaining.min(100ms))` (remaining = 0) で再描画が続く。先読みの完了でも worker が repaint する。
- 待ち中は今のページの 3D を描き続ける。画面上の表示は足さない (§10 の 1)。
- `Unavailable` で送ったページは、今の 3D の経路で表示する: 素材ができれば 2D と計算中の箱 → 3D、深度が失敗すれば 2D と既存の失敗の
  通知 (正本 §5.4)。先読みが途中なら §5.5 で引き継ぐ。
- **照合の後の移動先の変更** (第 1 回レビュー P2-4): `accept_book_slideshow_target` の後で、本の構成の許可待ち
  (`book_composition_permission`) が送り先を別の候補へ移すことがある (非表示のページ)。その場合の着地は照合を通らず、
  今の 3D の経路で表示する (平面 → 立体が見え得る)。非表示のページを含む本の構成は 3D の写真の閲覧ではまれで、
  許可待ちの継続の所有者 (再入の仕組み) に照合を足すと状態が増えるため、制限として受け入れる (§5.8)。

#### 5.4.1 待ちの終端 — ページの準備の分類 (今のページと送り先で共通の 1 つの関数)

```rust
enum DepthPageProgress {
    Ready,        // 今のページ: §5.7 の事実がある。送り先: LRU に素材の key がある
    Producing,    // すでに始まっていて、終わりが保証された仕事が動いている
    Unavailable,  // それ以外 (仕事が無い・始まっていない・失敗・取消・登録を見送られた)
}
fn depth_page_progress(&self, ctx, idx, role: DepthPageRole /* Current | Next */) -> DepthPageProgress
// 読み取りだけ。要求の発行・再試行・待ちの開始時刻の記録をしない
```

**原則 (第 6 回レビュー P1-2・P2-4 を受けて)**: `Producing` に数えるのは、**すでに始まっていて、既存の所有者が成功・失敗・取消のどれかで
必ず終える仕事だけ**。まだ始まっていない仕事 (読み込みの未発行、D3 の未開始、CPU 予約の拒否で始められない仕事) は数えない。
分類に迷うものは `Unavailable` に倒す (誤って `Producing` にすると止まり、誤って `Unavailable` にすると平面 → 立体が 1 回見えるだけ)。
送り先の仕事は、今のページを 3D で見せてから期限までの間隔 (最短 0.5 秒、既定 3 秒) の間に始まるので、通常は期限の時点で
始まっているか終わっている。始まっていなければ待たずに送る。

上から順に判定し、最初に当てはまったものを返す。

| 段 | 状態 | 分類 | 終わりの保証 |
| --- | --- | --- | --- |
| 1 | 今のページ: §5.7 の事実がある (`Presented` / `Failed`)。送り先: 素材の key が LRU にある | `Ready` | — |
| 2 | 深度の仕事が動いている (今のページ: `source` が `ReadingBack` / `Estimating`。送り先: 枠がその key で `ReadingBack` / `Estimating`)。`Estimating` はその要求の cancel token が立っていないこと | `Producing` | 読み戻しは map の成功 / 失敗、推論は結果 / 失敗 / 取消で終わる (既存)。取り消された要求は `try_result` が `None` を返し続けるので数えない |
| 3 | 送り先の枠がその key で `Failed` | `Unavailable` | — |
| 4 | 素材が resident だが renderer の backing がまだ無い | `Producing` | managed texture はそのフレームの末尾で登録される (既存の `TexturePending` と同じ) |
| 5 | 元画像の読み込みが、対象ページ・今の世代の実際の要求として発行済みで未取消 (`fs_page_load_state(idx)` の `LoadPending` / `UploadPending` のうち、RAW の `PreviewAbsent` で現像が始まっていない場合などの投影を除く) | `Producing` | 既存の読み込みは成功か `LoadFailed` で終わる |
| 6 | 元画像は読み込み済みで、final 系の仕事 (`final_effect_pending` の今の key の項目) が `Computing` | `Producing` | worker の job は結果 / 失敗 / 取消で終わる |
| 6b | 素材を作る、D3 の外の既存の独立した worker が、今の条件 (同じ下地・世代・key) で動いている: 注釈のベイク (`comic_bake_pending`、`app.rs:92066`)、局所補正・隠蔽・消しゴムの段の producer (`current_edit_stage_producer_pending`、`app.rs:88368` 付近) 等 | `Producing` | 各 worker は結果 / 失敗 / 取消で終わる (既存)。読み取りだけで、深度の `source` は変えない (第 7 回レビュー P2) |
| 7 | 同じく `CpuReady` / `Registering` で、残りの登録が、登録の受付と**同じ純粋な予算判定**で受け付けられる | `Producing` | ROOT の共通の登録枠で 1 フレーム 1 枚ずつ終わる |
| 8 | 同じく `CpuReady` / `Registering` で、予算判定が受け付けない | `Unavailable` | 着地すれば表示の要求が既存の手順で引き取る (`take_prepared_final_for_display`) |
| 9 | final AI の仕事が今の key で動いている (既存の AI の pending) | `Producing` | AI worker の結果 / 失敗 / 取消で pending が除かれる (第 6 回レビューで確認) |
| 10 | 上のどれでもない (読み込みが `NeedsLoad` / `LoadFailed`、D3 が未開始、D3 の対象外、AI が要るのに AI の仕事が無い、取消後に誰も作らない、元画像の保持だけ 等) | `Unavailable` | — |

- 段 7・8 の予算判定は、登録を受け付ける既存の判定 (`final_prepare_budget.rs` の GPU admission) と同じ純粋な関数を、副作用なしで呼ぶ。
  診断用の記録 (`FinalPrepareGpuDeferred`、perf 無効では記録されない) を状態として使わない (第 6 回レビュー P1-1)。
  純粋な判定を切り出せない場合は、`CpuReady` / `Registering` を `Unavailable` に倒し、設計担当へ報告する。
- 実装担当は着手時に、段 2〜9 (6b を含む) の各状態の読み方 (関数・フィールド) をコードで特定し、各 `Producing` の段について「その仕事が必ず
  終端へ進み、終端したらこの関数が `Ready` か `Unavailable` を返すこと」をテストで固定する。各 `Producing` の段は「対象ページ・今の世代の
  実際の要求が存在し、取り消されていない」ことを条件に含める (状態名だけで判定しない)。段 6b は、今のページの素材の選択
  (`resolve_fs_processed_payload_and_depth` の `NotYet` の理由) を作っている worker を漏れなく列挙する。
- **今のページ (`role = Current`)**: 間隔の開始 (§5.7.1) と照合の最初の段で使う。`Unavailable` (読み込みの終端失敗、素材の生産者が
  無い、元画像の保持だけ 等) は終端として扱い、間隔を数え始め、期限で送る (第 6 回レビュー P2-3)。D3 と先読みの開始は
  §5.7 の事実 (`Ready`) だけを証拠にし、`Unavailable` では始めない (提示の証拠を緩めない)。そのとき送り先は段 10 で `Unavailable` になり、待たずに送る。

照合 (§5.4) での使い方: まず今のページの分類が `Producing` なら `Wait`。次に送り先の分類が `Ready` なら `Go`、`Producing` なら `Wait`、
`Unavailable` なら `Go`。

### 5.5 送った後の引き継ぎ (先読みの途中を捨てない)

今のページの `select_source(key)` が LRU に無い `key` を選んだとき、枠が同じ送り先 (今のページ) と同じ `key` を持っていれば、
新しく読み戻しを出さずに枠の段階 (`ReadingBack` / `Estimating` / `Failed`) を今のページの段階へ移し、枠を空にする。
移す前に、枠の送り先 (idx・item の識別子・items_generation) と `key` が今のページの素材と一致すること、`Estimating` なら
その要求が取り消されていないこと (`DepthPending` の cancel token) を確かめる。外れたら枠を捨て、今のページは通常どおり読み戻しから始める
(第 2 回レビュー P1)。
色テクスチャは描画用に今までどおり `copy_color` で作る。終端 (`Unavailable`) で送った場合も、手動で送り先へ移った場合も同じ。
推論のやり直し (同じ context の新しい要求による取消) が起きない。

### 5.6 送り先の素材を表示前に作る (D3 の次ユニット準備を 3D のスライドショー中にも動かす)

| ページ | 表示前のテクスチャ | 作る者 |
| --- | --- | --- |
| 補正なし | `fs_cache` の先読みテクスチャを final が共有する (同じ TextureId) | 既存の元画像の先読み |
| 補正・ポストフィルタ・局所補正・隠蔽・注釈あり | D3 の次ユニット準備が final を作る (紙めくり OFF では今は動かない) | **D3 (有効条件を広げる)** |
| カラー化・LUT | 既存の `prefetch_final_effects`、D3 | 既存 |
| AI 拡大・ノイズ除去あり | 既存の AI 先読み (`prefetch_final_ai`) の CPU 結果の後、D3 の after-AI 計画で final を作る | 既存 + D3 |

1. **有効条件の述語を 1 つにする** (第 1 回レビュー P1-1)。
   `App::next_unit_final_prepare_enabled() -> bool` = 今の条件 (ページ表示・分割でない・Remote の操作中でない等) かつ
   (紙めくり ON **または** `depth_slideshow_prefetch_active()`)。
   `depth_slideshow_prefetch_active()` = `FsExclusiveView::Depth` かつ `slideshow_playing` かつ ページ表示・分割でない。
   D3 の次ユニットの経路で `settings.page_turn_animation_enabled` を見ている箇所をすべてこの述語に置き換える。少なくとも:
   需要トポロジーの生成 (`prepare_final_demand_topologies`、`ui_fullscreen.rs:21302`)、ユニットの決定 (`d3_final_prepare_unit_at`、
   `final_prepare.rs:495`)、投機要求の保持 (`speculative_final_unit_is_needed`、`final_prepare.rs:516`。採用前の破棄
   `app.rs:90585` もこれを読む)。
   - 実装担当は D3 の次ユニットの開始・保持・採用・取消・予算の経路で紙めくりの設定を読む箇所を全部列挙し (`final_prepare*.rs`、
     `prepared_edit*.rs`、`ui_fullscreen.rs` の final demand、`app.rs` の採用)、置き換える / 置き換えない (紙めくりの演出そのものの
     判断、例: 見開きの端の配置 `ui_fullscreen.rs:21134`、legacy 先読みの登録の延期 `final_prepare_budget.rs:526`) を理由つきの表にして
     本書 §12 に記録する。紙めくりの受付・演出・source 先読みの範囲 (`page_turn_prefetch_policy_enabled`) は変えない。
   - Previous / Current の位置の準備は今までどおり紙めくり ON のときだけ。
2. **開始の条件「今のユニットを提示した」** (`page_turn_current_unit_presented()`、`final_prepare.rs:447`): 紙めくりの提示状態
   (`PresentationState::Flat` の受理済みの証拠) を見る。3D の描画は submission を `LegacyRendered` にする (`ui_fullscreen.rs:45337`)
   ので、3D 中は新しい証拠が作られない。ただし 3D に入る前の同じページの受理済みの `Flat` は残り、照合は Depth の有無を見ないので、
   3D に入ったページでは古い証拠で真になり得る (実装担当の前提調査、§12)。
   **`depth_slideshow_prefetch_active()` の間は、この条件を §5.7 の「今のページを 3D で見せた」事実 (`depth_current_settled()` が真で
   `Presented`) だけに置き換える**。紙めくりの提示状態は (古い証拠を含めて) 読まない。OR で併用しない。同じ述語の中で選び、
   別の開始経路を作らない。紙めくりの提示状態の寿命は変えない (3D でないときの紙めくり・D3 の挙動を保つ)。
   3D のスライドショー中でないとき (3D の手動閲覧を含む) は今の条件のまま。
3. **着地後の同一性**: D3 が作った final が、送った後に `ensure_final_composite_texture_impl` の cached exact-complete
   (`app.rs:91333`) としてそのまま採用され、`resident_final_prepare_material` が返した TextureId と同じになることをテストで固定する。
   さらに、送った後の最初の描画で深度の callback が実際に出ることまで確かめる (§8)。
4. 元画像の先読み数が 0、VRAM 予算外、AI 先読み 0 で AI が要るページなど、表示前の素材が作られない設定では、
   §5.4.1 の分類が `Unavailable` (段 10) になり、待たずに送る (そのページは平面 → 立体が見える)。設定の組み合わせとして受け入れ、マニュアルに書く。

### 5.7 「今のページを 3D で見せた」事実と、間隔の数え方

- `DepthViewState` に、描画のパスで記録する事実を 1 つ置く:

  ```rust
  pub(crate) struct DepthSettled {
      pub fs_idx: usize,
      pub edit_key: EditResultKey,          // item の識別子・items_generation・入力世代 (source_gen) 等を含む
      pub key: DepthSourceKey,
      pub outcome: DepthSettledOutcome,     // Presented | Failed
  }
  pub settled: Option<DepthSettled>,
  ```

  - `Presented`: `try_paint_depth` が深度の callback を出したとき (その key・今のページ)。
  - `Failed`: `advance_depth_source` で、このパスで選んだ素材の key について `Failed` が確定したとき。
- **照合 (1 つの関数 `depth_current_settled(&self) -> bool`)**: 次をすべて満たすときだけ真。
  1. 同じ context の `DepthViewState` が持つ事実で、`fs_idx == fullscreen_idx`
  2. `edit_key == current_edit_result_key(fs_idx)` — 描画前でも、RAW の現像の到着 (一覧世代を変えずに入力世代だけを進める、
     `raw_page_store.rs:2479`) や編集の世代の変化を検出できる (第 2 回レビュー P1)
  3. `DepthViewState.source` が `Ready { key }` または `Failed { key }` で、その `key` が事実の `key` と同じ —
     補正の変更などで素材のテクスチャが変わり、今のページが新しい要求を出した時点で偽になる
  4. **直前の描画パスで今のページに選んだ素材の key** (`DepthViewState.current_material: Option<DepthSourceKey>`。
     `advance_depth_source` が毎回の見えている描画パスで書く唯一の場所。素材が `Ready` ならその key、`NotYet` なら `None`) が
     事実の `key` と同じ (実装中の前提調査、§12「Failed 事実と NotYet」)。既存の `wait_for_source` は失敗した key の再試行を避けるため
     `Failed` を残すので、3 だけでは、補正・AI の設定の変更で新しい素材を作っている間 (`NotYet`) も旧 `Failed` の事実が通ってしまう。
     4 で、新しい素材を待つ間は事実が外れ、§5.4.1 の段 6 / 6b / 9 (新しい素材の仕事) へ進む。素材が旧 key に戻れば `Failed` は有効に戻り、
     再試行しない既存の規則と一致する。
  古い事実を消す処理は足さない (照合で外れるだけ)。
  素材の変化が描画のパスで初めて分かる場合 (補正の変更など、2 が変わらない変化) は、D3 の開始が 1 フレームだけ古い事実で
  許され得る。D3 は次ユニットを作るだけで今のページの素材の生産を止めないので、これは受け入れる。先読みの `submit` は 5 で防ぐ。
  5. 先読みの `submit` (§5.3 の 4) と枠の保持 (§5.3 の 1 (a)) は、`advance_depth_source` の中で今のページの選択と進行を済ませた
     **後**に、同じパスで照合する。今のページが新しい要求を出したパスでは 3 が偽になるので、先読みは出ず、枠も捨てられる。
- **今のページの素材の key は `current_material` だけから読む**: §5.4.1 の分類の `role = Current` (段 1・2・4 の key) も
  `current_material` を使い、表示前の素材を返す `resident_final_prepare_material` は使わない (送り先 `role = Next` だけに使う)。
  RAW の安定した preview のように、通常の selector が深度の入力にするが resident の helper が返さない素材でも、今のページの
  読み戻し・推論中を `Producing` と分類できる (実装中の独立レビュー P2)。`current_material` が `None` (NotYet) のときは段 2 に
  当たらず、段 5〜9 で新しい素材の仕事を見る。
- この照合を読むのは 2 か所: (1) 3D 中の間隔の開始 (§5.7.1)、(2) §5.3 の 2 の先読みの開始と §5.6 の 2 の D3 の開始。
- `current_material` は描画パスで書くので、D3 の開始など描画より前に読む側は直前のパスの値を見る (2 の世代の照合と組み合わせる)。

#### 5.7.1 間隔の数え方 (3D 中)

- **3D 中の自動送りを評価するパス** (第 3 回レビュー P2): スライドショーのタイマー (`handle_fs_navigation_with_render_perf` の
  タイマー部分) を直接呼ぶのは 2 か所: 所有 viewer の描画の後 (`render_fullscreen_viewport_with_root_input` の中、
  `ui_fullscreen.rs:30773`) と、汎用の入力処理の wrapper (`handle_fs_navigation_with_list_order_operation`、`ui_fullscreen.rs:43910`)。
  後者には root のキー入力の転送 (`ui_fullscreen.rs:34225`、画像 viewport の描画より前)、ゲームパッド (`gamepad_input.rs:9611`)、
  ホイール操作の途中処理 (`ui_fullscreen.rs:41682`) などが入る (第 4 回レビュー P3)。
  3D 中は、タイマーの評価 (送り先が `InItems` か `Other` かによらず、自動送り全体) を**所有 viewer の描画後の呼び出しだけ**で行う。
  呼び出し側が明示する引数で区別する:

  ```rust
  enum SlideshowTimerPass {
      /// 30773: 所有 viewer の描画の後。描画 closure の中で取った値を運ぶ
      ViewerRender { viewport: egui::ViewportId, visible: bool },
      /// 43910 の wrapper 経由のすべて (root の転送・ゲームパッド・ホイール等): 手動操作だけを配送する
      InputOnly,
  }
  ```

  3D でないときは、今までどおり両方でタイマーを評価する (挙動を変えない)。新しい pending は作らない。
  45729 の `advance_slideshow` の呼び出しは連結読み用で、3D の対象外 (3D は連結読みを入口で弾く)。
  所有描画の後のタイマーには既存の 50 / 100 ms の起床要求があり、子 viewport の再描画要求は親へ接続されるので、`InputOnly` で
  評価しなくても自動送りは止まらない (第 4 回レビューの確認)。
- **「見えているパス」の定義**: 所有 viewer の描画 closure の中で、`advance_depth_source` と同じ述語 (`self.window_visible` かつ
  その viewport の `ctx.input(|i| i.viewport().minimized != Some(true))`、`depth_view.rs:560`) を値として取り、viewport id と一緒に
  `ViewerRender { viewport, visible }` で運ぶ。30773 で渡す外側の `ctx` は ROOT のもの (`ui_fullscreen.rs:27741`) で、所有 viewport の
  `ctx` は描画 closure の中だけの変数 (`ui_fullscreen.rs:28081`) なので、タイマー側で `ctx` から可視性や viewport id を読み直さない
  (`viewport_id()` は現在の viewport stack を見る、`vendor/egui/src/context.rs:751`。第 4 回レビュー P2)。
  main の埋め込み表示・通常全画面 (child viewport)・detached のどれでも、描画 closure の中で同じ式で取る。
- 3D 中の `current_slideshow_frame_ready` = 見えているパス かつ 今のページの §5.4.1 の分類が `Ready` か `Unavailable`
  (`Unavailable` は元画像の読み込みの終端失敗 (`LoadFailed`、既存の `fs_load_failed` の通知はそのまま) や素材の生産者が無い状態。
  終端として数え始め、期限で送る。第 2 回レビュー P3-4、第 3 回 P3-2、第 6 回 P2-3)。
- 3D 中の別のページへの着地 (`complete_book_slideshow_target`、`ui_fullscreen.rs:43181`) では、期限と anchor を設定せず
  anchor を未設定のまま残す。描画後の `sync_slideshow_anchor_for_frame` が事実を確認して起算する (第 2 回レビュー P3-3)。
  先読みが当たったページは移った最初のパスで `Presented` を記録するので、その sync で起算され、追加の待ちは生じない。
- 間隔は「3D で見えている時間」になる。
- **非表示・最小化** (第 2 回レビュー P2): 見えていないパスでは ready が偽になるので、既存の分岐 (anchor と同じページで未 ready なら
  anchor を外す、`ui_fullscreen.rs:42658`) で起算が止まる。3D 中は見えていないパスでは自動送り全体を評価しない
  (`InItems` の照合だけでなく `Other` の送りも)。復帰したら、今のページを 3D で描いたパスから間隔を数え直す。
  **例外**: 最小化の間に描画パスが 1 つも走らなかった場合は、アプリからは長いフレームと同じに見え、anchor が残ったまま期限を
  過ぎているので、復帰後の最初の描画パスで送ることがある。そのパスでも描画 (先読みの結果の採用を含む) がタイマーより先に走る
  (上の「評価するパス」) ので、先読みが終わっていれば当たったまま送る。この場合を検出する状態は足さない。

### 5.8 先読みしない場合 (平面 → 立体が見えてよい場合)

| 場合 | 扱い |
| --- | --- |
| 次のフォルダ (`NextFolder`) | 全画面を閉じて開き直すので 3D が終わる。§10 の 3 (トーストを足す) |
| コレクションの直接登録画像 | 送り先は送る時点の最新一覧から非同期に決まる (先に推測しない設計、collection-playback-plan §5.3)。先読みも照合もしない。§10 の 4 |
| 末尾から先頭への折り返し (`LoopFolder`) | 先頭は D3 の次ユニット (末尾では Boundary) と元画像の先読み範囲の外。照合せずに送る (1 周に 1 回、平面 → 立体)。§10 の 4 |
| 分割表示 | D3 は分割表示を対象外にしている。照合せずに送る。§10 の 4 |
| 本の構成の許可待ちで送り先が変わった | §5.4 |
| 透過背景のモードを切り替えたページから送る | §5.9 |

### 5.9 他の機能との関係

- **手動のページ送り**: フォルダ内の手動送りはスライドショーを止めない (既存)。新しい先読みは始めないが、送り先へ移った場合は
  §5.3 の 1 (b) と §5.5 で途中の枠を引き継ぐ。別のページへ移れば枠は捨てる。スライドショーの期限の扱いは既存のまま。
- **透過背景のモード** (第 1 回レビュー P1-2): AI 拡大の final のキーは透過背景のモード (白背景) を含み (`app.rs:88441`)、ページへ
  入ると既定に戻る (`app.rs:72530`)。今のページで背景を切り替えている間は、表示前の素材が着地後の素材と一致しない。
  その間は先読みも照合もしない (§5.3 の 2・§5.4)。背景は着地で既定に戻るので、その次のページから先読みに戻る。
  着地の条件をすべての生産者へ渡す改修 (背景を引数にする) は、白背景にしたページからの 1 回だけのために状態を広げるので採らない。
- **書き出し**: 書き出しの modal 中はスライドショーのタイマーが止まる (既存)。先読みも出さない (§5.3)。
- **360°**: 変更なし。360° の入場は今までどおりスライドショーを止め、S キー・topbar の抑止も残す。リングは今までどおり使える。
- **紙めくり**: 3D 中は紙めくりの対象外 (既存)。紙めくり ON でも 3D 中の自動送りは通常の切り替えになる。
- **別ウィンドウ・複数ウィンドウ**: 新しい状態はすべて 3D の context 所有の `DepthViewState` と、context ごとのスライドショーの状態の中。
  App 全体の状態を足さない。3D を持てるのはプロセスで 1 context (既存) なので、先読みも 1 context だけ。
  detached の述語には触れない (第 1 回レビューも同じ判断)。§5.7.1 でスライドショーのタイマーの呼び出し 2 か所 (所有 viewer の
  描画後、汎用の入力処理の wrapper) に `SlideshowTimerPass` の引数を足し、描画 closure で取った viewport id と可視性を運ぶ。
  どの viewport・context が所有者かの判定は変えず、3D でないときの挙動も変えない。これは viewport の経路に触れるので CLAUDE.md の
  リワーク中のルールの対象であり、**入力の処理と描画後の自動処理を区別し、所有描画の時点の事実を渡す構造的な修正で、症状パッチでは
  ない**ことに、独立レビュー (第 4 回) と設計担当 (ClaudeCode) が合意した。範囲と理由を
  [detached-rework-plan.md](detached-rework-plan.md) §11 に記録する。実装レビューでもこの主張への合意を取り直す。
  それ以外で detached の述語・viewport の経路に触れる必要が出たら、止めて設計担当へ返す。
- **非表示・最小化**: 先読みも今のページと同じく進めない。3D 中の自動送りと間隔の起算は休止し、復帰後に今のページを 3D で描いてから
  数え直す (§5.7.1)。
- **Remote**: 3D は Remote に出さない (既存)。変更なし。

## 6. UI スレッドと GPU (ui-responsiveness §4)

- 同期 I/O なし。素材は resident のものだけを使い、読み込みは既存の先読み・D3 の worker が行う。
- 先読み 1 回あたりの UI スレッドの GPU 仕事は、今のページの深度と同じ (blit + mip 生成 + 504² と長辺 1536 の blit・読み戻しの
  命令の発行)。読み戻しの完了は非ブロッキング poll。R16 のアップロードは今のページと共有の 1 フレーム 1 枚。
- 先読みの一時的な色テクスチャ (8K で約 340 MB) は命令の発行後すぐ捨てる。今のページの色テクスチャと同時に持つのは
  その 1 フレームだけ。
- 紙めくり OFF での D3 の動作は、紙めくり ON と同じ予算・登録枠 (1 フレーム 1 枚) に従う。
- 区間に `perf::event` を置く (§5.3 の 6)。

## 7. まれな失敗

既定の割り切り (ログ + 通知)。先読みの読み戻し・推論の失敗は枠の `Failed` に記録してログに出し、送った後に今のページの失敗として
既存の通知を出す (§5.5)。先読みの段階では通知しない (まだ見ていないページの失敗を出さない)。再試行の仕組みは足さない。

## 8. テスト

状態遷移・handler レベルで次を固定する (GPU を使うものは既存の depth のテスト helper と test-group gpu)。

1. 3D に入ってもスライドショーが止まらない。360° に入ると今までどおり止まる。
2. 3D 中に S キー・topbar の再生ボタン・リングで開始 / 停止できる。360° 中の S キー・topbar は今までどおり抑止、
   360° 中のリングは今までどおり使える。「…」は 3D 中も出ない。
3. 先読みは今のページの事実 (§5.7) ができるまで出ない。今のページの新しい要求で先読みが取り消され、枠も捨てられる。
4. 先読みした深度が LRU に入り、送った最初のフレームで深度の callback が出る (読み戻し・submit が起きない)。
   補正なしのページ (fs_cache 共有) と、補正ありのページ (紙めくり OFF、新しいトポロジー、D3 の worker 開始から完成テクスチャの
   登録まで) の両方。TextureId の一致を確かめる。
5. §5.4.1 の各段: `Producing` の間は送らず (`Wait`)、時間が経っても送らない。各 `Producing` の仕事が終端 (成功・失敗・取消) すると
   `Ready` か `Unavailable` になって送る。`Unavailable` の各段 (読み込みの未発行・失敗、D3 の未開始・対象外、予算判定で登録を見送られた
   `CpuReady` / `Registering`、深度の失敗) で待たずに送る。今のページが `Unavailable` (読み込みの終端失敗など) でも止まらず、
   間隔を数えて送る。送った後に推論中の先読みが今のページへ引き継がれ、取り消されない。
6. 手動で送り先へ移ったとき (`NavigationSequence` の `PageTransition` を経る)、枠が引き継がれる。別のページへ移ると捨てられる。
7. 送り先が読み込み済みのアニメーションなら待たずに送り、着地で 3D が終わる (トースト)。
8. 透過背景を切り替えたページからは先読みも照合もしない。着地の次のページから先読みに戻る。
9. スライドショー停止・3D 終了・context の終了・フォルダ移動で枠が捨てられ、取消が届く。兄弟 context に作用しない。
10. 間隔は 3D の事実ができてから数える (先読みが当たったページはすぐ、外れたページは 3D になってから)。
11. 書き出し中は先読みを出さない。
12. `slideshow_next_target` の `InItems` が、D3 の次ユニットと一致する (単ページ・ページ表示)。分割・折り返し・コレクションは `Other`。
13. 紙めくり ON / OFF の両方で、3D でないときの D3・紙めくりの既存テストが変わらない。
14. 今のページの素材が変わったとき (RAW の現像の到着で入力世代だけが進む場合、補正の変更でテクスチャが変わる場合) に、
    事実の照合が偽になり、先読みが出ない・枠が捨てられる・取消済みの要求を引き継がない。枠が `ReadingBack` / `Estimating` の両方で。
15. 3D 中に窓を最小化すると送りと起算が止まり、戻すと今のページから数え直す。3D 中は root のキー入力の転送の呼び出しで期限切れの
    タイマーを評価しない (3D でないときは今までどおり評価する)。main の通常全画面 (child viewport) と detached の両方で。元画像の読み込みが終端失敗したページでは止まらずに送る。
16. 3D 中のスライドショーが「次のフォルダへ」で全画面を開き直すと、3D が終わってトーストが 1 回出て、スライドショーは 2D で続く。
    3D でないとき・手動の Ctrl+↓ のフォルダ移動では、このトーストは出ない (手動のフォルダ移動の既存の挙動を変えない)。
17. 既存回帰: `test(/depth|slideshow|page_turn|final_prepare|prepared_edit|panorama|export|admission|ring/)`。

## 9. 文書

- [depth-parallax-plan.md](depth-parallax-plan.md): §1 の「先読み」の行、§6.3 の後始末、§6.5 / 段 B 分類表の該当行を更新し、本書へリンク。
- マニュアルの 3D 写真の節: スライドショーを使えること、待つことがあること、アニメーション画像・次のフォルダ・コレクション・
  折り返し・分割表示の扱い。
- [display-pipeline.md](display-pipeline.md) の 3D の節、[async-architecture.md](async-architecture.md) の 3D の節に先読みの持ち主を 1 段落。
  [page-turn-animation-plan.md](page-turn-animation-plan.md) §28 に D3 の有効条件の述語の変更を 1 段落。
- キーの追加は無い (keymap の変更なし)。

## 10. 利用者の決定 (2026-11-18、統括経由)

設計第 5 版の §10 で推奨案つきで尋ね、次のとおり決まった。

1. **次のページの深度が間に合わないとき: 上限なしで待つ** (推奨は「間隔 1 回分を上限」だったが、利用者が選択)。
   待ち中の表示は出さない。ただし、待つのは計算が進んでいる間だけにし、深度の失敗・取消・入力が無い (表示前の素材が作られない設定等)
   で終端した場合は永久に止まらず、そのページは 2D の扱いで送る (終端状態を必ず持つ)。→ §5.4・§5.4.1。
   推奨案の時点で書いた「表示前の素材が作られない設定では止まったように見える」は、§5.4.1 の終端の分類で起きないようにした。
2. **アニメーション画像のページ: 着地して 3D を終え、トーストで知らせて 2D で続ける** (推奨どおり)。→ §5.4 の `Go`。
3. **「次のフォルダへ」の境目: 今どおり 3D を終えて 2D で続け、トーストを足す** (推奨どおり)。全画面を閉じて開き直す経路で、
   3D を持っていた場合だけ、フォルダ移動の結果を適用する側 (`FolderNavMode::SlideshowNext` を確認できる `app.rs:62385` 付近) で、
   3D を退役させる前にトーストを出す。共通の `close_fullscreen` には足さない (手動の移動まで通知してしまうため。第 6 回レビュー)
   (例: 「3D 写真を終了しました: 次のフォルダへ進みました。<キー> キーでもう一度使えます」。キーの表記は既存の終了の通知と同じ helper)。
4. **先読みできない場合を制限として受け入れ、マニュアルに書く** (推奨どおり): コレクションの直接登録画像のスライドショー
   (毎ページ平面 → 立体)、末尾から先頭への折り返し (1 周に 1 回)、分割表示 (毎ページ)、透過部分の背景を既定から切り替えたページ
   (白・市松) からの送り (その 1 回)、本の構成で非表示にしたページのために送り先が変わった場合 (その 1 回)。
5. **先読みは 3D のスライドショー中だけ** (推奨どおり)。
6. **最小化・非表示の間は 3D の自動送りを休止し、戻したら今のページから数え直す** (推奨どおり)。最小化の間に画面の更新が
   1 回も走らなかった場合は、戻した直後に送ることがある。2D のスライドショーの最小化中の動きは変えない。

## 11. 独立設計レビューの記録

### 第 1 回 (2026-11-17、GPT-6.1 Sol / xhigh、read-only、基点 `0c1e2f7e4`、session `01a1ea05-59cf-73d1-bc1b-233a34593b07`)

**REVISE** (P1 2 件、P2 5 件、P3 1 件)。設計担当がコードで確かめ、すべて採用して第 2 版に反映した。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P1-1 | D3 は `d3_final_prepare_unit_at` 以外 (需要トポロジー `ui_fullscreen.rs:21302`、保持判定 `final_prepare.rs:516`、採用前の破棄 `app.rs:90585`) も紙めくりの設定を見る | §5.6 の 1: 有効条件を 1 つの述語にし、全箇所を列挙して表にする |
| P1-2 | AI の final のキーは透過背景のモードを含み、ページ入場で既定に戻るので、白背景のページからは着地後の素材と一致しない | §5.9: 背景を切り替えている間は先読みも照合もしない |
| P2-3 | 3D 中にアニメーションを飛ばすと、D3 の次ユニット・折り返し・分割表示と送り先がずれる | §3・§5.1・§5.8: 飛ばさない案を推奨に変え、`InItems` を D3 の次ユニットと一致する範囲に限る |
| P2-4 | 本の構成の許可待ちで送り先が変わると照合を通らない | §5.4・§5.8: 制限として記録 (許可待ちの継続に照合を足すと状態が増える) |
| P2-5 | 深度の `Ready` は今のページを見せた証拠にならない (D3 は描画前に動き、`Failed` は古いキーを残す) | §5.7: 描画で記録する事実を 1 つにし、間隔の開始と D3・先読みの開始で共有する |
| P2-6 | フォルダ内の手動送りはスライドショーを止めない。手動送りの `PageTransition` で枠を捨ててしまう | §1・§5.3 の 1 (b)・§5.9 を訂正 |
| P2-7 | リングの抑止は 3D だけ。360° 中は使える | §4 の 2 を訂正 |
| P3-8 | 未読のアニメーションの例外を選択肢に明記 | §10 の 2 |

### 第 2 回 (2026-11-18、同じセッションの継続、read-only、基点 `a8d7c9930`)

**REVISE** (P1 1 件、P2 1 件、P3 3 件)。第 1 回の P1-1・P1-2・P2-3・P2-4・P2-6・P2-7・P3-8 は解消と評価。P2-5 は下の P1 で未解消。
設計担当がコードで確かめ、すべて採用して第 3 版に反映した。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P1 | 事実の照合に今の素材との一致が無い。RAW の現像は一覧世代を変えずに入力世代を進めるので古い `Presented` が有効なまま残り、取消済みの先読みを引き継ぎ得る | §5.7 の照合に `EditResultKey` と今の `source` の key の一致を加え、submit と枠の保持は今のページの処理の後の同じパスで照合する。§5.5 の引き継ぎで送り先・key・取消を確かめる |
| P2 | 非表示・最小化で、照合は送るのに今のページの事実が作られず、間隔が止まる | §5.4・§5.7.1: 見えていないパスでは `Wait`、起算も休止し、復帰後に数え直す。§10 の 6 |
| P3-3 | 着地の直後に期限が設定され、3D を見せる前から数え始める | §5.7.1: 3D 中の着地では anchor を設定せず、描画後の sync で起算する |
| P3-4 | 元画像の読み込みの終端失敗で 3D の事実ができず止まる | §5.7.1: 既存の `fs_load_failed` を ready に含める |
| P3-5 | §10 の制限一覧に本の構成の許可待ちと、白以外 (市松) の背景が無い | §10 の 4 |

### 第 3 回 (2026-11-18、同じセッションの継続、read-only、基点 `cd9832ebf`)

**REVISE** (P2 1 件、P3 2 件)。第 2 回の P1・P3-3・P3-5 は解消、P2・P3-4 は改善と評価。`EditResultKey.source_gen` が
RAW の入力世代を含むこと (`app.rs:87458`・`88074`)、着地で anchor を設定しない方式が本の構成の継続 (`slideshow_playing` で生存判定)
と両立することも確認された。設計担当がすべて採用して第 4 版に反映した。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P2 | スライドショーのタイマーは root のキー入力の転送 (描画より前、可視性は ROOT の状態) からも評価される | §5.7.1: 3D 中の自動送りは所有 viewer の描画後の呼び出しだけで評価し、可視性はその viewport の述語で定義する。`Other` の送りにも適用 |
| P3-2 | `fs_load_failed` が可視性の条件の外にある | §5.7.1: `見えているパス かつ (事実 または fs_load_failed)` |
| P3-3 | 描画パスが走らなかった最小化の例外を §10 の 6 にも書く。番号の順 | §10 の 5・6 |

### 第 4 回 (2026-11-18、同じセッションの継続、read-only、基点 `8954ef4a4`)

**REVISE** (P2 1 件、P3 1 件)。第 3 回の指摘は可視性の取得場所を除き解消と評価。`InputOnly` でタイマーを評価しなくても自動送りは
止まらないこと (所有描画後の既存の起床要求、子 viewport の再描画要求の親への接続) も確認された。新しい引数は viewport の経路に触れるので
凍結ルールの対象であり、**症状パッチではない構造的修正であることに合意**した (設計担当も合意、detached-rework-plan.md §11 に記録)。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P2 | 30773 で渡す `ctx` は ROOT のもので、所有 viewport の `ctx` は描画 closure の中だけ | §5.7.1: 描画 closure の中で viewport id と可視性を値で取り、`ViewerRender { viewport, visible }` で運ぶ |
| P3 | 描画しない側の呼び出しは root の転送だけでなく、ゲームパッド・ホイール操作等を含む汎用の wrapper | §5.7.1: `InputOnly` に改名し、範囲を明記 |

### 第 5 回 (2026-11-18、同じセッションの継続、read-only、基点 `56fbe71e0`)

**ACCEPT** (指摘なし)。第 4 回の 2 件の解消と、detached-rework-plan.md §11 の合意記録 (設計段階の合意として正確、実装・検証済みとは
扱っていない) を確認。§10 の判断を実装仕様へ反映する前提で、実装を妨げる点は無い。

### 第 6 回 (2026-11-18、同じセッションの継続、read-only、利用者の決定の差分だけ、基点 `78dac76d0`)

**REVISE** (P1 2 件、P2 2 件)。待ちの分類で `Producing` に数えた段に、終わらない場合が残っていた。NextFolder のトーストの方針は妥当
(移動の結果を適用する側で、3D の退役の前に出す)。段 11 (AI の pending) の終端は確認された。設計担当がすべて採用して第 7 版に反映した。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P1-1 | `Registering` のまま予算で保持される場合が永久待ち。診断用の記録は状態に使えない | §5.4.1 段 7・8: `CpuReady` / `Registering` とも、登録の受付と同じ純粋な予算判定で分ける |
| P1-2 | 「D3 の対象」は「始められる」ではない (AI 待ち、CPU 予約の拒否) | §5.4.1 の原則: 始まっていない仕事は `Producing` に数えない |
| P2-3 | 今のページが読み込みに失敗すると、段 0 が永久に `Producing` | §5.4.1: 今のページにも同じ分類を使い、`Unavailable` は終端として数え始めて送る。D3・先読みの開始の証拠は緩めない |
| P2-4 | 紙めくり OFF では先読みの調整が動かず、保持範囲の中でも読み込みが出ない場合がある | §5.4.1: `NeedsLoad` は `Unavailable` (未発行の読み込みを待たない) |

### 第 7 回 (2026-11-18、同じセッションの継続、read-only、差分だけ、基点 `7200b6b8f`)

**REVISE** (P2 1 件、P3 1 件)。第 6 回の永久待ち (D3 の未開始・CPU 予約の拒否・`NeedsLoad`・登録の予算) は解消、通常時に永久に
`Wait` する経路は他に無いと確認された。§10 の 3 のトーストの位置も適切。設計担当が採用して第 8 版に反映した。

| 指摘 | 内容 | 反映 |
| --- | --- | --- |
| P2 | 今のページの注釈のベイク等、D3 の外の素材の worker を拾えず、3D を一度も見せずに送り得る | §5.4.1 段 6b |
| P3 | 状態名だけでは「未取消の開始済みの仕事」を示せない (取消後の `Estimating`、RAW の `PreviewAbsent` の投影) | §5.4.1 段 2・5 と実装担当への指示 |

### 第 8 回 (2026-11-18、同じセッションの継続、read-only、差分だけ、基点 `3fec6b688`)

**ACCEPT** (指摘なし)。第 7 回の 2 件の解消を確認。追加の永久待ちや深度の `source` の状態変更につながる問題は無く、実装を妨げる設計上の指摘は無い。

### 独立実装レビュー (2026-11-18、Sol / xhigh、実装担当とは別の会話、read-only)

| 回 | 基点 | 判定 | 指摘と対応 |
| --- | --- | --- | --- |
| 第 1 回 | `4f4bd0b60..e96789a9c` (全体) | REVISE | P2: ZIP/PDF など physical staged の「次のフォルダへ」で 3D 終了の通知が出ない → 共有の通知 helper を通常・staged・Collection 外側の 3 経路の close 直前に接続 (`c5596544c`)。`SlideshowTimerPass` は構造的修正であることに合意 ([detached-rework-plan §11](detached-rework-plan.md)) |
| 第 2 回 | `e96789a9c..c5596544c` | ACCEPT | P2 解消。拒否時に全画面を残す変化は Collection 設計 §7 に沿い妥当。P3: root installer の拒否条件が呼出し側と二重 → installer に集約し、確定後に呼出し側の `FnOnce` で通知・close (`e4c3535c6`) |
| 第 3 回 | `c5596544c..e4c3535c6` | ACCEPT | P3 解消、新しい指摘なし |

## 12. 実装記録

### 着手前の前提調査 (2026-11-18、実装保留 → 設計担当が §5.6 の 2 を明確化して再開)

**前提 1 がコードと食い違うため、指示どおり製品の実装を開始せず、設計担当へ返す。**
新しい `DepthNextSlot` / `DepthSettled` / `SlideshowTimerPass` はまだ実装していない。
下記の再現テストは未実行であり、実行時の結果としては扱わない。

#### 前提 1: 新しい提示証拠を作らないことと、既存の証拠が偽になることは異なる

- `try_paint_depth` が成功した描画は `FsDisplayUnitSubmission::LegacyRendered` を返し
  (`ui_fullscreen.rs` の `draw_fs_image_with_receipt`)、新しい紙めくりの受理証拠を作らない。
  この部分は §5.6.2 の見込みと一致する。
- しかし、同じページを通常表示して受理済みの `PresentationState::Flat` がある場合、
  `enter_depth_parallax` → `set_fs_exclusive_view` / `close_fs_exclusive_conflicts` は
  `page_turn_presentation` を退役させない。
- `page_turn_current_unit_presented` → `page_turn_presented_unit` は受理済みの `Flat` を返す。
  さらに `page_turn_unit_matches_current` → `PageTurnUnitBinding::matches_current` は
  ページ・context・一覧世代・layout・content の一致を調べるが、Depth の有無を調べない。
  したがって、その一致を保ったまま 3D に入れば古い証拠で真になり得る。
- 紙めくりの `PageTurnPresentation::cancel` も `Flat` はそのまま保持するため、
  対象外の描画を通っただけでは証拠が無効になる保証はない。

§5.6.2 の「3D 中は証拠が作られない」から「提示述語が偽」とする前提を修正する設計判断が必要。
既存の証拠の寿命を変更するか等の代案は実装担当では選んでいない。

**設計担当の判断 (2026-11-18)**: §5.6 の 2 は元から「3D のスライドショー中はこの条件を 3D の事実に置き換える」としており、
古い証拠が真でも判定に使わないので、設計の結論は変わらない。前提の文言 (「3D 中はこの証拠が作られない見込み」を根拠にしていた部分) を
直し、置き換えが排他的 (紙めくりの提示状態を読まない) であることを明記した。証拠の寿命は変えない。

#### D3 の設定参照の調査表 (置換方針は本設計によるもの、変更は未実施)

| 箇所 (調査基点の行) | 区分 | 設計上の置換 / 維持と理由 |
| --- | --- | --- |
| `ui_fullscreen.rs:21302` `prepare_final_demand_topologies` | 需要トポロジー | 置換。Depth スライドショーの Next もトポロジーを必要とする |
| `ui_fullscreen.rs:21434` `observe_next_final_unit_demand` | 需要の perf 観測 | 置換。同じ Next の需要を観測する |
| `final_prepare.rs:495` `d3_final_prepare_unit_at` | ユニットの決定 | Next は置換。Previous / Current の準備は紙めくり ON の条件を維持 (§5.6.1) |
| `final_prepare.rs:516` `speculative_final_unit_is_needed` | 保持 / 取消 / 採用 | 置換。Next の所有を広げる。着地済みの同じユニットの保持もこの判定を通る |
| `app.rs:90585` 付近 `adopt_final_prepare_results`、`prepared_edit.rs` `adopt_prepared_edit` | 採用前の取消 | 設定の直接参照はなく、上記の保持述語を読む。別の有効条件は足さない |
| `final_prepare_budget.rs:526` `defer_legacy_final_prefetch_registration` | legacy の登録延期 / 予算 | 維持。紙めくり ON の legacy 登録方針そのもの。新しい D3 の Speculative origin は既存の予算 / 登録枠を通る |
| `app.rs:83921` `page_turn_prefetch_policy_enabled` | source / AI の先読み範囲と保持 | 維持。§5.6.1 で変更しないと指定されている範囲 |
| `ui_fullscreen.rs:21134` | 見開き端の配置 | 維持。紙めくりの演出そのもの |
| `ui_fullscreen.rs` の紙めくりの受付・操作・演出、`app.rs:19907` の設定初期化 | 紙めくり本体 | 維持。D3 Next の有効条件ではない |

`final_prepare*.rs` / `prepared_edit*.rs` の直接参照は上表の 495 / 516 / 526
(および既存テストの設定代入) だった。開始点 `prefetch_next_final_unit` の
`page_turn_current_unit_presented` は §5.6.2 の事実による切替対象であり、設定の直接参照ではない。
Previous の開始点 `prefetch_previous_final_unit` の提示条件は広げない。

#### その他の前提の到達点

| 前提 | 調査結果 / 未確認 |
| --- | --- |
| 2: D3 の設定参照 | 上表の静的調査まで。置換・回帰テストは未実施 |
| 3: 各 Producing の実要求・終端 | `DepthPending::try_result` は取消後に `None` を返し続けること、`FinalEffectState` の Computing / CpuReady / Registering、`validate_final_prepare_pending`、`current_edit_stage_producer_pending`、RAW PreviewAbsent の LoadPending 投影を確認。全 producer の列挙と終端テストは前提 1 で保留 |
| 3: 段 7 / 8 の純粋な予算判定 | `speculative_vram_admitted` は既存の副作用なしの関数で、`admit_final_prepare_registration` が使っている。複数の未登録素材を持つ Registering の残り全体との同一性の切出し・テストは未実施 |
| SlideshowTimerPass | `handle_fs_navigation_with_render_perf` の直接呼出しは所有描画後と汎用 wrapper の 2 か所。所有 viewport の値を描画 closure 内で捕捉する実装と、起床・最小化のテストは未実施 |
| 4: 着地の TextureId と最初の深度 callback | 未確認。補正なし / 紙めくり OFF の D3 完成品についての着地テストは未追加 |

#### 変更と検証

- `src/app/depth_view.rs`: `#[cfg(all(test, windows))] mod gpu_tests` に
  `depth_slideshow_premise_existing_flat_evidence_survives_depth_draw` を追加。
  通常描画の受理 → 実際の Depth 入場 → 同じ表示素材の LRU を使用した深度 callback の送出後も
  以前の提示述語が真かを調べる調査用テスト。`gpu_tests` の既存 filter に所属する。
  ウィンドウや製品 exe、実モデルは起動しない。現挙動を記録するためのテストであり、
  この挙動を今後の仕様として承認するものではない。
- `docs/depth-slideshow-plan.md`: 本調査・保留理由を記録。製品動作・公開マニュアルは変更していない。
- 焦点コマンド:
  `cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow_premise_existing_flat_evidence_survives_depth_draw/)'`。
  全試行で指定の `run-heavy.ps1 -Owner "771-codex" -WorkDir G:\home\mimageviewer-771 -Command ...` を使用。
  初回は完了出力を取得できず結果不採用。再取得した 01:43:20 の試行は `turbojpeg-sys v1.2.0` の
  CMake / MSBuild が exit 1、Cargo / runner の報告 exit 101 (外側のコマンドツールは exit 1)。
  `MSBUILDDISABLENODEREUSE=1` を指定した 01:43:56 の試行も同じ結果。
  **テスト実行 0 件。成功証拠なし。** テスト用の Rust コードのコンパイルも未確認。
- runner は共有ロックと G: の TEMP に書けない警告後、既定の BelowNormal / 6 build jobs /
  既定 TEMP で実行。ビルド設定・依存ファイルは変更していない。
- 本体 lib 全件、`build-dev.ps1`、製品起動、commit は実施しない。
- 最後の Cargo 操作は同じ runner 経由の `cargo fmt` (引数なし)、exit 0。
  `git diff --check` も exit 0。UI 文字列は変更していない。

### 再開後の実装途中記録 (2026-11-18、依存ビルドの指定停止条件により中断)

設計担当の §5.6.2 明確化 (`fedd1ed4f`) を受け再開した。以下は**未コンパイル・未検証の作業差分**であり、実装完了・ACCEPTの証拠ではない。
`cargo check` が依存 `turbojpeg-sys` を再ビルドして MSBuild で失敗したため、利用者の
「依存物の再ビルドでまた MSBuild が失敗したら、回避策を探さずにその時点で報告」の指示に従い停止した。
別の Cargo コマンドでの回避や再試行は行っていない。§9 の関連文書・マニュアルの更新も未着手。

#### 差分の所有と内容

| ファイル | 実装途中の内容 |
| --- | --- |
| `src/depth_parallax.rs` / `src/depth_estimation.rs` | context 所有 Next 枠、settled 事実、LRU 読取、readback / 推論 / 失敗の引継ぎ、取消読取、Next readback の会計、現ページと共有する1frame1枚のR16登録、perfイベント |
| `src/app/depth_view.rs` | 事実の照合、3D専用のD3開始条件、現在source進行後のNext保持・開始・進行、共通gate、描画callback / 選択済素材の失敗で事実を記録 |
| `src/app/depth_progress.rs` (新規) | Current / Next 共通のread-only分類。実要求・世代・key・未取消の照合、既存producerと終端の分類 |
| `src/app/final_prepare.rs` | Next需要 / 保持を新述語へ接続。Previous / Currentの準備は紙めくりONを維持。Depthスライドショー時はPresentedだけを開始証拠にする |
| `src/app/final_prepare_budget.rs` / `src/app/prepared_edit.rs` | 実登録と共通の純粋なVRAM判定、共有 / 既登録handleを除いた残り全allocationの読取 |
| `src/app/raw_page_store.rs` / `src/raw/executor.rs` | RAWの実develop要求・取消のread-only確認。1回のtry_lockで読めない場合は待たずUnavailable |
| `src/ui_fullscreen.rs` / `src/app/gamepad_input.rs` | 3Dの開始停止入口、共通送り先、送り前gate、SlideshowTimerPass、描画closure内で捕捉する所有viewportの可視性、3D着地後の起算、停止時Next破棄 |
| `src/app.rs` | 分類module登録、Panoramaだけの入場時停止、SlideshowNext結果のDepth終了通知 |
| `src/ui_fullscreen/depth_slideshow_tests.rs` (新規) | 入口・キー・リング・HUD・送り先・タイマー・停止の7件 |
| `src/ui_fullscreen/page_turn/{merge_master_tests,slideshow_regression_tests}.rs` | 既存2箇所のタイマーpass引数追従のみ |
| 本書 | 停止理由と未確認事項の記録 |

#### D3設定参照の実装状況

| 箇所 | 今回の差分 |
| --- | --- |
| `prepare_final_demand_topologies` / `observe_next_final_unit_demand` | `next_unit_final_prepare_enabled` へ。前者のfullscreen_idxガードは維持 |
| `d3_final_prepare_unit_at` | 共通有効条件へ。Next以外は紙めくりONが必要、元のpost_filter_bypassed条件を維持 |
| `speculative_final_unit_is_needed` | 共通有効条件へ。採用・取消側の既存callerも同じ判定を読む |
| `prefetch_next_final_unit` | `next_unit_current_presented` へ。Depthスライドショー中は `depth_current_settled && Presented` だけ、それ以外は従来の紙めくり提示述語 |
| `prefetch_previous_final_unit` | 従来の紙めくり提示述語を維持 |
| legacy登録延期 / source・AI先読み範囲 / 見開き端配置 / 演出・受付 | 変更なし。理由は上の着手前調査表と同じ |

#### 前提とテストの到達点

1. 古いFlatが真でもDepthスライドショーのNext開始判定には使わない形を実装。
   旧調査テストは `depth_slideshow_ignores_retained_flat_until_depth_is_presented` へ置換し、
   `update_texture` のSmallVecを要素ごとに渡す型誤りも修正した。実行は未確認。
2. 設定参照は上表のとおり接続。Single / Spread singletonの入口許可範囲で、動画を挟む送り先とD3の一致を試験に追加。
   非3DのSpread Doubleは従来の2ページ歩幅を保持する。
3. 分類の段2は現resident素材のkeyとsource / slot・取消、段4はmanaged backing、段5はfs_pending / backlog / RAW実develop、
   段6はcurrent final_effect_pending、6bはcomicの現下地 / 世代 / cancelとlocal・conceal・eraseの既存helper、
   段7/8は残り全allocationと純粋VRAM判定、段9は現AI keyのpendingを読む。
   完了 / 失敗 / 取消 / 切断の焦点試験13件を追加したが、実行による終端保証は未確認。
   **RAWのCurrent深度推論中にresident素材でkeyを証明できない場合の分類と、実adoptionとの予算完全一致は独立レビューも未完**。
4. 補正なしのsource共有と、紙めくりOFFのD3 worker開始→完成登録→Next readback→注入した深度結果→LRU→
   slideshow着地→実 `resolve_fs_processed_payload_and_depth`→最初の深度callbackを通すGPU試験を追加。未実行。
   他にreadback / Estimating / Failed引継ぎ、cancel / wrong target拒否、1frame1枚の現ページとの共有を追加。

追加23件・旧調査テストの更新1件、計24件の追加 / 更新テストを用意した。GPU生成4件は `gpu_tests` に置いた。
新規テスト・既存回帰とも実行0件。実窓・製品exe・実モデルは起動していない。

独立Sol / xhighレビューは停止時点で判定保留。旧FlatをORしない条件とcontext所有枠の引継ぎを差分で確認し、
`SlideshowTimerPass` は所有描画の事実を渡す構造的修正で症状パッチではないことに合意した。
全Producingの終端・RAW素材同一性・予算同一性・試験網羅性のレビューは未完で、ACCEPTではない。

#### 検証コマンド

`MSBUILDDISABLENODEREUSE=1` を設定し、指定の
`powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "771-codex" -WorkDir G:\home\mimageviewer-771 -Command "cargo check -p mimageviewer --bin mimageviewer-core"`
を実行 (02:04:07開始)。check用の依存ビルドで `turbojpeg-sys v1.2.0` の
`cmake --build ...turbojpeg-sys-e58481dd5d2e733f/out/build --target install --config Debug --parallel 6` が
MSBuild exit 1、build script exit 101で停止。今回のRust変更の型検査へは未到達。
共有ロック / G: TEMPへ書けない警告の後、runner既定のBelowNormal / 6 build jobs / 既定TEMPを使用。
本体lib全件とbuild-devは設計担当の担当のまま。コミットは行わない。

runner終了報告は02:07:28、exit 101 (外側のコマンドツールはexit 1)。焦点nextestは未実行。
停止時の整形として、同じrunnerと `MSBUILDDISABLENODEREUSE=1` で最後に `cargo fmt` (引数なし) を実行しexit 0。
`python scripts/check_ui_glyphs.py` は危険グリフ0件・exit 0、`git diff --check` もexit 0。

### WIP `a2c04d79c` 後の再開と新たな設計確認 (2026-11-18)

設計担当が依存ビルドを外側で完了した後、`target/771-logs/check1.log` / `notest1.log` の本体エラーを修正した。
`ItemsGenerationMap::current_items_generation` はtest限定APIだったため既存の `accepts_generation` を使用し、
AppTestEnv経由の同時borrowを事前の値取得へ変更した。texture deltaはDropを持つownerからmoveせず参照で登録してclearし、
fs_cacheの添字を既存getへ直した。

指定runner / `MSBUILDDISABLENODEREUSE=1` で `cargo check -p mimageviewer --bin mimageviewer-core` を実行し、
02:20:35にexit 0 (1分29秒)を確認。依存MSBuildの再失敗は発生していない。
HUD / timer試験は実Depthを退役させないGPU fixtureへ修正し、未取消の実pendingを期限後も待つ試験と、
失敗 / 取消 / 未発行の終端で送る試験を追加した。これらの実行確認はまだ完了していない。

§9の文書はdepth-parallaxの範囲 / 後始末 / 入力分類、display / async / page-turn設計、manual/fullscreen.html、
製品ページを更新中。RAWCurrent用の非blocking照合に備え、既存のpassthrough cache照合とcolorize判定の純粋部分を
共有helperへ切り出したが、RAWCurrentの分類への接続と試験は未着手。

#### 設計担当へ返す食い違い: Failed事実とNotYet (§5.7) → 設計担当が §5.7 の照合 4 と `current_material` を追加して解決

独立Sol / xhighレビューで次の経路を確認した。§5.7は「一時的な素材待ちでも事実は消さないが、sourceとの照合3で外れる」
としているが、既存の `DepthViewState::wait_for_source` は失敗済みkeyの再試行を避けるため **Failedだけは保持する**。
補正 / AI / post_filterパラメータは `EditResultKey` に含まれないため、次の順序では3条件すべてが成立したままになる。

1. 現ページの深度がFailedで確定し、同じkeyのFailed事実が記録される。
2. 補正 / AI等のパラメータが変わり、新素材を作るworkerが始まる。EditResultKeyは同じ。
3. 通常selectorが `NotYet(IncompleteComposite)` 等を返し、`advance_depth_source` は既存 `wait_for_source` を呼ぶ。
4. sourceの旧Failedは保持され、`depth_current_settled` が真のまま。分類段1がReadyを返すため、
   新素材の実producerを待つ段6 / 6b / 9へ届かず、起算や先読みの継続を認め得る。

「既存の失敗keyを再試行しない」「古い事実を消さない」「新状態を増やさない」と合わせ、現在素材の証明をどこで持つかの
設計判断が必要なため、利用者の指示に従い機能実装の追加を停止した。代案や制限追加は選んでいない。
`depth_view::premise_tests::depth_slideshow_failed_fact_survives_not_yet_after_parameter_change` を調査用に追加し、
既存のwait操作とparams変更で旧事実 / Current=Readyが残ることを検査する。現挙動の記録であり仕様の承認ではない。

独立レビューはREVISE。上の設計確認のほか、通常selectorが深度入力にする安定RAW previewをresident helperが返さず
実ReadingBack / EstimatingをUnavailableとする分類漏れをP2として確認した。Nextの先読み範囲を広げず、Currentだけの
現在素材の証明を既存selectorと共有する修正は未接続。残り登録量の純粋な予算判定に確定不具合は見つからなかったが、
実CpuReady / Registeringに小予算を与えた分類 / adoption一致試験が不足している。
perfの `next_prefetch.elapsed_ms` は現状pollのCPU時間で、readback / 推論の全所要時間ではない点も未修正(P3)。

#### 今回の検証の確定結果

すべて指定runner / `MSBUILDDISABLENODEREUSE=1` を使用した。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、02:20:35完了、1分29秒 |
| `cargo nextest run -p mimageviewer --lib --no-run` | exit 0、02:35:23完了、9分39秒。現在のテスト実行体のコンパイルを確認、実行0件 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow_failed_fact_survives_not_yet_after_parameter_change/)'` | 別作業 `778 lib-full` の共有全件ロック待ちで実行未開始。設計判断待ちの停止に合わせ、待機中の自分のrunnerだけを中断 (外側exit 1)。テスト失敗ではなく実行0件 |
| `python scripts/check_ui_glyphs.py` | 危険グリフ0件、exit 0 |
| `python scripts/check_site_cards.py` | 53カード、超過0件、exit 0 |
| `git diff --check` | exit 0 |

未実行の設計§8焦点 / 指定既存回帰を成功扱いにしない。今回の調査用テストも実行での再現確認は未完。
機能実装の追加を停止した。`cargo fmt` も指定runnerで共有全件ロック待ちとなり未開始だったため、
引継ぎ後に差分が自動変更されないよう自分の待機runnerを中断 (外側exit 1)。今回の整形は未実施で、再開後の最終整形に残す。

### `e7248c54d` 後の実装記録 (2026-11-18)

以下を今回の到達点とする。上の途中記録は各時点の履歴であり、現在の未修正事項や検証結果を表すものではない。
実装の独立レビュー・本体 lib 全件・`build-dev.ps1` は設計担当の担当。今回の実装側では行わず、ACCEPT とも記録しない。

#### 設計担当の追加判断の反映

- `DepthViewState.current_material: Option<DepthSourceKey>` を追加。製品コードでの代入は
  `advance_depth_source` だけで、見えている描画パスの通常 selector が Ready ならその key、NotYet なら None を書く。
  `depth_current_settled` は従来の context・位置・EditResultKey・source の一致に加え、この素材との一致も要求する。
  旧 Failed 事実は保存し、素材が旧 key に戻れば再び有効になる。同じ失敗の再試行はしない。
- `depth_page_progress` の Current の段 1・2・4 は現在素材の観測を使う。
  `resident_final_prepare_material` は Next だけが使う。安定 RAW preview の Current の読み戻し・推論は Producing になり、
  表示前の RAW 先読み対象は広げない。調査用 Failed テストはこの動作の回帰試験に置き換えた。
- 実 worker の CpuReady / Registering に小予算を与え、分類と実登録受付の一致を試験する。
  テスト専用 RAII は検出 VRAM 容量だけを注入し、予算計算・admission の判定は製品と同じ関数を通す。
- Next 枠の `started_at` は perf 専用。枠の作成前から LRU 投入までの累積所要時間を
  `next_prefetch.elapsed_ms` に記録する。期限・受付・状態の判断には使わない。
  RAW 調査用に一時切り出した passthrough / colorize helper は不要になったため元の関数内へ戻した。

#### 変更ファイル (本課題全体)

| ファイル | 要点 |
| --- | --- |
| `src/depth_parallax.rs` / `src/depth_estimation.rs` | context 所有 Next 枠・settled 事実・現在素材、途中仕事の引継ぎと取消、共有 upload 枠、LRU と readback 会計、perf、試験用結果注入 |
| `src/app/depth_view.rs` | 入口・事実照合・D3 の有効条件 / 提示条件、Current 後の Next 進行、送り前 gate、GPU 回帰試験 |
| `src/app/depth_progress.rs` (新規) | Current / Next の read-only 分類と各 producer の終端試験、実 CpuReady / Registering の予算一致試験、RAW Current 試験 |
| `src/app/final_prepare.rs` | D3 Next の需要・ユニット・保持を共通有効条件へ。Previous / Current は従来どおり |
| `src/app/final_prepare_budget.rs` / `src/app/prepared_edit.rs` | 実登録と同じ純粋な VRAM 判定、残りの未共有・未登録 allocation の列挙 |
| `src/app/raw_page_store.rs` / `src/raw/executor.rs` | 実 RAW develop 要求・世代・取消の非ブロッキング読取 |
| `src/app/vram_accounting.rs` | 実登録の小予算試験に検出 VRAM 容量を注入する test 限定 RAII |
| `src/ui_fullscreen.rs` / `src/app/gamepad_input.rs` | S・再生ボタン・リング、共通送り先、gate、SlideshowTimerPass、所有描画 closure 内の可視性捕捉、停止時取消 |
| `src/app.rs` | 分類 module、Panorama のみ入場時停止、次フォルダ結果適用時の終了通知と handler 試験、完成 D3 素材の表示採用条件 |
| `src/ui_fullscreen/depth_slideshow_tests.rs` (新規) | 操作・HUD・送り先・無期限待ち・終端・停止・アニメーション着地・main child / detached の休止復帰試験 |
| `src/app/depth_view_tests.rs` | 既存リング試験の ImageSlideshow だけを新しい共存仕様へ更新。他の競合モード抑止・変形保持の assertion は維持 |
| `src/ui_fullscreen/page_turn/merge_master_tests.rs` / `slideshow_regression_tests.rs` | 既存呼出し 2 か所の TimerPass 引数追従 |
| `docs/depth-parallax-plan.md` / `display-pipeline.md` / `async-architecture.md` / `page-turn-animation-plan.md` | §9 の範囲・所有者・D3 条件・入力分類を更新 |
| `docs/architecture-overview.md` / `spec.md` / `keymap-spec.md` / `detached-rework-plan.md` | 状態所有・製品動作・既存 S 操作の条件と承認済み TimerPass 範囲の記録 |
| `htdocs/mimageviewer/manual/fullscreen.html` / `htdocs/mimageviewer/index.html` | 3D スライドショー、待ち、休止、終了条件、先読みの制限を記載 |
| 本書 | 設計判断・実装・前提確認・検証の記録 |

#### D3 の紙めくり設定参照 (最終実装)

| 経路 / 関数 | 置換 / 維持 | 理由 |
| --- | --- | --- |
| 単ページの可視ユニット生成: `spread_display_composition_for_anchor` | `page_turn_prefetch_policy_enabled` 呼出しを `next_unit_final_prepare_enabled` へ置換 | 紙めくり OFF でも通常描画の所有者が D3 の可視 baseline を作る必要がある。pure な需要派生や採用側で可視証拠を作らない。元画像 / AI の先読み範囲を決める呼出しではない |
| 需要: `prepare_final_demand_topologies` | `next_unit_final_prepare_enabled` へ置換 | 紙めくり OFF の Depth slideshow Next にも同じ需要を作る。既存 fullscreen_idx guard は維持 |
| 需要観測: `observe_next_final_unit_demand` | 同述語へ置換 | 実需要と perf 観測を一致させる |
| ユニット: `d3_final_prepare_unit_at` | Next は同述語へ置換 | Previous / Current は別途紙めくり ON を要求。既存 post_filter_bypassed 条件を維持 |
| 保持 / 取消: `speculative_final_unit_is_needed` | 同述語へ置換 | 需要・保持の有効条件を一致させ、着地済みユニットも既存手順で保持 |
| 採用: `adopt_final_prepare_results` / `adopt_prepared_edit` | 直接の設定参照なし。上の保持述語を共有 | 採用だけ別の有効条件を増やさない |
| 表示採用: `fs_display_page_load_state` | `page_turn_prefetch_policy_enabled` 呼出しを `next_unit_final_prepare_enabled` へ置換 | D3 の GPU 完成品がある場合は、元画像の CPU cache 退去後もその完成品を表示できる。対象一覧 / 先読み数 / keep 範囲の計算は変えず、既存の完成品を採用する条件を D3 の有効条件へ合わせる |
| 開始: `prefetch_next_final_unit` | `next_unit_current_presented` へ置換 | Depth slideshow 中は照合済み Presented だけ。古い Flat を OR で併用しない |
| 開始: `prefetch_previous_final_unit` | 従来の紙めくり提示条件を維持 | Previous の開始範囲は広げない |
| 予算: `defer_legacy_final_prefetch_registration` | 設定参照を維持 | legacy 登録延期という紙めくり固有の方針。D3 Speculative は既存の GPU 予算・共通登録枠を使用 |
| source / AI: `page_turn_prefetch_policy_enabled` と先読み範囲 / 保持の caller | 設定参照を維持 | 元画像・AI の先読み範囲は変更しない (§5.6)。上の可視ユニット生成の caller は D3 の用途なので区別する |
| `previous_final_prepare_occurrences` の同 policy 呼出し | 維持 | Previous は従来どおり紙めくり ON の範囲に限る |
| 見開き端配置、紙めくり受付 / 操作 / 演出、設定初期化 | 設定参照を維持 | D3 Next の有効条件ではなく紙めくり本体の動作 |

#### 前提の確認結果

1. 3D の描画は LegacyRendered となり新たな紙めくり受理証拠を作らないが、入場前の Flat は真のまま残り得る。
   設計担当が明確化したとおり寿命は変えず、Depth slideshow の開始条件は照合済み Presented だけを読む。
   回帰試験は古い Flat が真でも Ready / Failed では Next を開始できず、深度 callback 後にだけ許可されることを確かめる。
2. 設定参照の需要・決定・保持・採用・取消・予算の全経路は上表のとおり。非 3D と Previous / Current の条件を保持した。
3. 各 Producing の読取と終端は次表。分類は要求発行・再試行・診断記録更新をしない。
4. 実 GPU 試験で、補正なしは fs_cache 共有、補正ありは紙めくり OFF の D3 worker → CpuReady → 実登録 →
   Next readback → 深度結果 → LRU → slideshow 着地 → 実 selector → 最初の深度 callback を検査する。
   key が一致し、その着地描画で再 readback / submit を出さないことを検査する。
   補正ありは元画像 cache が resident の場合と退去済みの場合の両方で、完成品を採用し元画像の再読込みも発行しないことを確認する。

| 段 | 読み取る実所有者 / 関数 | 終端の扱いと試験 |
| --- | --- | --- |
| 2 | Current の current_material / source、Next の現素材 / target / key / phase、DepthPending cancel token | map 成功 / 失敗、推論結果 / 失敗 / 取消。取消済みは Producing にしない。世代・素材置換、引継ぎも試験 |
| 4 | managed texture の tex_manager meta と renderer backing | フレーム末の texture delivery 後はこの段から外れる。user texture の backing 不在は数えない |
| 5 | fs_pending の現世代 ticket と cancel、fs_upload_backlog、depth_raw_development_pending の実要求 | 読込結果 / 失敗 / 取消、RAW 結果 / 取消。PreviewAbsent の表示上の LoadPending だけでは待たない |
| 6 | final_prepare_pending_is_current と final_effect_pending::Computing | worker 完了 / 切断 / 取消・世代変更で外れる |
| 6b | comic_bake_pending の下地 / 世代 / cancel、current_edit_stage_producer_pending、commit_pending_matches_current_erase_key | 注釈・局所補正・隠蔽・消しゴムの完了 / 失敗 / 取消 / 切断で外れる。旧 Failed + NotYet でも現在の producer を待つ |
| 7 / 8 | prepared_edit_remaining_allocations、speculative_vram_allocations_admitted | 実 GPU admission と同じ純粋判定。共有 handle・既登録 handle を除外。全残量が入ると Producing、入らなければ Unavailable。実 CpuReady / Registering の過不足両側を試験 |
| 9 | 現素材 / params からの final AI key と final_ai_pending cancel | 結果 / 失敗 / 取消で既存 poll が項目を除去する。未発行は Unavailable |

`SlideshowTimerPass` 以外の viewport 経路や detached の述語は変更していない。
main の通常全画面 child と detached の両方で、実描画 closure に渡す所有 viewport の hidden / minimized を試験し、
root が minimized でも visible child が進むこと、InputOnly が Depth の期限を評価しないこと、復帰時に間隔を数え直すことを検査する。
実描画 fixture で透過背景の白 / 市松による先読み抑止と既定復帰、実書き出し操作の admission / cancel を通す抑止と再開、
実手動ナビゲーションの NavigationSequence / PageTransition を通す推論要求の引継ぎも試験する。
製品 exe / 実窓 / 実モデルは起動しない。

#### 最終検証

Cargo はすべて指定 `run-heavy.ps1 -Owner "771-codex" -WorkDir G:\home\mimageviewer-771` と
`MSBUILDDISABLENODEREUSE=1` を使用した。今回の共有全件ロック待ちは中断せず待機した。
完了した最終結果は末尾の表のとおり。以下の失敗履歴を成功扱いにはしない。

焦点初回 (`test(/depth_progress::|depth_slideshow|depth_next_/)`) は共有全件ロックの解放を待ち、
03:18:21 に開始、03:24:37 に exit 100。コンパイルは成功し、37 件実行・26 成功・11 失敗 (7 abort を含む)。
7 件は初回 UI pass 完了前の空 font atlas placeholder を test helper が GPU へ登録し、validation panic と
未処理 delta の Drop panic が重なったもの。画像の非ゼロ delta はすべて登録し、空の Managed(0) だけを識別して除外する
test helper に修正した。残る失敗は可視 topology を未構築の D3 / 予算 fixture、sidecar / 本の構成等による
遅延再表示を未 service の NextFolder fixture、Panorama canvas の前提まで混ぜた HUD fixture を修正した。
NextFolder の scan 自体は pre_scan で採用済みだった (当初の scan 待ちという説明を訂正)。製品コードの回避策は入れていない。
初回ログは `target/771-logs/focus-final.{out,err}.log`、JUnit は `target/771-logs/focus-first.junit.xml`。

焦点の第 2〜4 回はいずれも 37 件実行・36 成功・1 失敗、exit 100。残った着地試験を追い、
汎用 Depth fixture が通常 loader と異なる sampler / 所有識別の texture を使っている点を直した後、
単ページの可視 baseline 生成が紙めくりの source policy に依存する間接参照の取りこぼしを確認した。
`spread_display_composition_for_anchor` の該当 caller を D3 の共通有効条件へ接続し、
可視証拠の構築は従来の通常描画の所有者に置いた。`prepare_final_demand_topologies` の pure な派生・
採用 / mount 側で可視証拠を作る変更はしていない。
同じ間接参照の棚卸しで `fs_display_page_load_state` の GPU 完成品の表示採用も接続した。
元画像 / AI の対象範囲・保持数を決める policy と Previous の条件は従来どおり。
単ページの既存 unit builder は回転 DB / landscape metadata の I/O を行わない分岐を使う。

焦点第 5 回は 04:30:39 開始、**37 件全成功・exit 0** (実行 22.582 秒)。
同じ試験名の中で、補正なし / 紙めくり OFF の補正あり / その元画像 cache 退去後の 3 ケースを通した。
実素材・D3 worker・登録・読み戻し・LRU・着地 selector・最初の深度 callback まで成功し、key の同一性を確認した。
本課題の焦点 37 件は旧調査試験の置換を含む。GPU を生成する試験はすべて `gpu_tests` に置き、既存 test-group gpu の所属で実行した。
ログは `target/771-logs/focus-fifth.{out,err}.log`、JUnit は `target/771-logs/focus-passed.junit.xml`。
設計からの逸脱はない。上の間接参照 2 か所は §5.6 の需要生成 / 表示採用の実装漏れの修正である。

#### §8 の試験との対応

| §8 | 試験 / 検証箇所 |
| --- | --- |
| 1・2 | conflict cleanup、S / ring、実 HUD click。Panorama の S / topbar 抑止と ring の既存受付、3D の overflow 非表示 |
| 3・14 | retained Flat の排他条件、Current の NotYet / 入力世代 / texture 変更 × Next ReadingBack / Estimating、旧 Failed の再照合 |
| 4 | `depth_slideshow_prefetched_source_and_d3_off_final_paint_on_first_landing` の 3 ケース。通常の可視構成の生成から D3 worker / 登録を通す |
| 5 | depth_progress の 16 件、期限切れ handler の無期限待ちと取消 / 失敗、Unavailable Current / Next の送り。実小予算 CpuReady / Registering の受付一致を含む |
| 6 | 実手動 handler の NavigationSequence / PageTransition と同じ request ID / 未取消 token の引継ぎ、wrong target / generation / key / cancel の拒否 |
| 7 | アニメーションへの実着地 / 通常描画、終了通知 1 回、2D 継続 |
| 8・11 | 実描画で白 / 市松の背景除外と復帰、実 export admission / cancel による先読み抑止と再開 |
| 9 | stop の Next token 取消、3D 退役時の request 取消と readback 解放。既存の context 終了・フォルダ移動・兄弟 context 所有試験も指定回帰で再検証 |
| 10・15 | 事実 / Unavailable による起算、InputOnly の抑止、実 main child / detached の hidden / minimized / 復帰、minimized ROOT と visible child の区別 |
| 12 | Single / Spread singleton の動画を挟む送り先と D3 の一致、末尾 / collection / split の Other |
| 13・17 | 指定既存回帰 1,671 件全成功。非 3D、紙めくり ON / OFF、360°、D3、書き出し、ring 等を含む |
| 16 | 実 NextFolder 結果適用と遅延再表示の完了、通知 1 回と 2D 継続、2D 自動 / Depth 手動の非通知 |

#### 確定した最終結果と引継ぎ

| コマンド | 件数 / 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 最終の製品差分で exit 0、35.47 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth_progress::\|depth_slideshow\|depth_next_/)'` | 最終 37 / 37 成功、18,614 skip、exit 0、22.582 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|slideshow\|page_turn\|final_prepare\|prepared_edit\|panorama\|export\|admission\|ring/)'` | 1,671 / 1,671 成功、16,980 skip、exit 0、322.815 秒 |
| `cargo fmt` (引数なし、最後の Cargo 操作) | exit 0 |
| `python scripts/check_ui_glyphs.py` | 危険グリフ 0 件、exit 0 |
| `python scripts/check_site_cards.py` | 53 カード、超過 0 件、exit 0 |
| `git diff --check` | exit 0 |

最後の型検査 → 指定回帰 → 成功時の `cargo fmt` は同じ指定 runner 内で順に実行し、04:41:31 に runner exit 0。
ログは `target/771-logs/regression-final.{out,err}.log`、JUnit は `target/771-logs/regression-passed.junit.xml`。
runner はロック領域 / G: TEMP に書けない警告後、既定の BelowNormal / 6 build jobs / 既定 TEMP で実行した。
今回の再開後に依存物の MSBuild 再失敗は発生していない。

本体 lib 全件、`build-dev.ps1`、実機での操作・最小化 / 復帰・実モデルの速度確認、実装の独立レビューは今回の実装側では未実施。
利用者の指示どおり設計担当へ引き継ぐ。製品 exe の起動・commit は行っていない。

### 独立実装レビュー第 1 回 P2: staged フォルダ移動の終了通知 (`e96789a9c` 後)

設計担当から、独立実装レビュー第 1 回は本件 P2 1 件を除き ACCEPT 相当、SlideshowTimerPass の構造的修正にも
合意したとの報告を受けた。今回の実装側では独立レビューを追加しない。

コード調査では、`apply_folder_nav_result` の ZIP / PDF / 変換書庫の `physical_staged` は通常の通知分岐より前に return し、
成功した `commit_physical_history_transition` の採用 closure で実際に閉じることを確認した。
この close に通知が無く、§10-3 の条件を満たす移動でも通知が欠落していた。

- 通知だけの `notify_depth_slideshow_folder_nav_exit(resume_slideshow)` を共有する。
  通常経路と physical staged の採用 closure の、既存 close の直前で呼ぶ。
  既存の再開意図と現在の Depth 所有だけを読み、状態・通知済み flag を増やさない。
- staged 側は採用の検証が通った closure でのみ通知する。準備中・失敗・不採用ではこの処理へ入らない。
  自動 NextFolder は事前に `slideshow_playing = false` にするので、現在の再生 bool を通知条件にはしない。
- close 自体が Depth を退役させるため、通常フォルダの close 後に内部 ZIP の staged 採用で再度 close しても通知は増えない。
  `close_fullscreen_for_folder_nav_reopen` の本体・detached の述語・viewport 分岐は変更しない。

#### 同形経路の棚卸し

| 経路 | 調査結果 / 対応 |
| --- | --- |
| 通常 `apply_folder_nav_result` の Fullscreen / SiblingFullscreen / ListOrder / SlideshowNext | 既存 resume 意図が true で Depth を所有する場合だけ、実 close 直前の共有通知を使用。手動移動は非通知 |
| `commit_physical_history_transition` → `MainListAfterHydration::Dfs` | ZIP / PDF / cached ZIP / 変換書庫 / EPUB の同じ採用 closure。fullscreen の実 close に共有通知を接続 |
| 通常フォルダ → `find_fullscreen_nav_target_filtered` → 内部 ZIP/PDF | `attach_inner_book_reading_continuation` が既存 Dfs に resume 意図を引き継ぐ。通常・staged の両 close を通す ZIP 回帰試験で二重通知を検査 |
| `finish_main_list_after_hydration` / PDF・ZIP enumerate / 本の構成の遅延 open | 再開だけを行う。Depth の close は既に上記境界で完了するため通知を追加しない |
| archive conversion の deferred fullscreen (`resume_archive_convert_viewer_after_load`) | 既存 owner 照合後の再開だけ。通常経路で先に close する場合と physical staged の成功採用で close する場合を上記で扱う |
| favsearch の subtree / sibling / search-end の close | 既存 continuation の resume は false。手動移動の既存挙動を維持し非通知 |
| SmartFolder の staged DFS (`begin_smart_folder_dfs_navigation`) | 独自の SmartFolder 手動意図。採用後の reopen の resume 引数は false。今回の自動再開の通知条件には当たらない |
| Snapshot の prepared archive cache → physical Dfs | 既存 `PhysicalHistoryDfsContinuation` の resume 意図を持って共通の physical 採用に入るため、上記の通知で扱う |
| RequiredFullscreen (`prepare_required_fullscreen_navigation`) | 明示 target の準備 / 再表示で、自動フォルダ移動の resume 意図ではない。共通 close 本体への通知追加はしない |
| ZIP 内 DFS (`zip_nav_dfs_fullscreen`) | 同じ ZIP 内の一覧 / book target 更新で、folder-nav の close を呼ばない。外へ抜ける場合は通常の FolderNav 経路へ進む |
| Collection 外側の自動移動 | 設計担当の回答で §10-3 の対象と確定。下の追補で独自の 3 か所の close も共有通知へ接続 |

#### 追加試験と検証

`src/app.rs` の既存 `depth_slideshow_folder_tests` に以下を追加した。

- ZIP / PDF の staged 採用から 2D の再表示・再生再開まで。Depth 自動 / 2D 自動 / Depth 手動、採用前は非通知、成功後に必要な 1 回だけ通知する。
- ZIP / PDF の preflight 失敗。既存の表示・Depth・停止済み再生意図を保持し、終了通知は出ない。
- 通常フォルダの close → 内部 ZIP の physical staged 採用の両方を通し、通知は合計 1 回。

ZIP は生成した実 ZIP の列挙を使い、失敗試験では不正な ZIP を実際に列挙させる。
PDF は test 専用 enumerate pool の子プロセス応答だけを成功 / 失敗で置き換え、enqueue・preflight・採用・hydration・再表示を通常処理で通す。
製品プロセス・実窓は起動しない。GPU を作る新規試験はない。

焦点 nextest は指定 runner / `MSBUILDDISABLENODEREUSE=1` で開始し、共有全件ロック待ちを中断せず待機した。
05:24:50 に実行開始、コンパイル 5 分 04 秒。**4 件全成功、18,650 skip、実行 2.887 秒、exit 0**。
新規 3 件と既存の通常経路 1 件で、ZIP/PDF の Depth 自動 / 2D 自動 / Depth 手動、失敗と二重 close を検査した。
ログは `target/771-logs/p2-focus-1.{out,err}.log`、JUnit は `target/771-logs/p2-focus-passed.junit.xml`。

通常 / physical staged の修正範囲での最終検証は以下のとおり。
すべて指定 `run-heavy.ps1 -Owner "771-codex" -WorkDir G:\home\mimageviewer-771` と
`MSBUILDDISABLENODEREUSE=1` 経由。共有全件ロック / heavy 枠の待機は中断していない。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、48.33 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow_folder_tests/)'` | 4 / 4 成功、18,650 skip、exit 0、2.887 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|slideshow\|page_turn\|final_prepare\|prepared_edit\|panorama\|export\|admission\|ring/)'` | 1,674 / 1,674 成功、16,980 skip、exit 0、276.439 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_nav\|next_folder\|staged/)'` | 241 / 241 成功、18,413 skip、exit 0、27.113 秒 |
| `cargo fmt` (引数なし、最後の Cargo 操作) | exit 0 |

型検査 → 指定回帰 → フォルダ移動回帰 → 成功時の cargo fmt を同じ runner 内で順に実行し、
05:38:38 に runner exit 0。ログは `target/771-logs/p2-validation.{out,err}.log`、JUnit は
`target/771-logs/p2-{regression,folder}-passed.junit.xml`。
runner のロック領域 / G: TEMP 書込み警告後は既定の BelowNormal / 6 build jobs / 既定 TEMP で続行した。
依存物の MSBuild 再失敗は発生していない。
最終の `python scripts/check_ui_glyphs.py` は危険グリフ 0 件・exit 0、`git diff --check` も exit 0。

変更ファイルは `src/app.rs` (共有通知と 2 か所の接続、新規 3 試験) と本書 (棚卸し・検証記録) の 2 件。
製品の新しい状態・I/O・待機・再試行・キー操作は追加していない。製品 exe 起動・commit・自己レビューは行っていない。
本体 lib 全件・`build-dev.ps1`・実機での表示は今回の実装側では未実施。

**設計確認の残件は解消**: 設計担当から Collection 外側の自動移動も §10-3 の対象との回答を受けた。
上の成功試験は通常 / physical staged の範囲の記録であり、Collection の実装・検証は次の追補に分けて記す。

### Collection 外側の自動移動の終了通知 (設計担当の回答による追補)

「スライドショーの自動送りで別の一覧へ移る同じ境目であり、移動先の種類で通知の有無を変えない」という
設計担当の判断に従い、`try_start_slideshow_next_folder` → `OuterFullscreen { resume_slideshow: true }` も対象にした。

- `PreparedCollectionPhysicalNavigation::install` と `install_collection_outer_physical_adoption` は、
  それぞれ既存の採用 closure 内の close 直前に同じ `notify_depth_slideshow_folder_nav_exit` を呼ぶ。
- `commit_collection_navigation` の main navigation を持たない既存経路も接続する。
  EPUB の設定による拒否と、root installer が拒否する準備済み owner の欠落を close より前で扱い、
  現在の target / revision / request の検証後に通知・close・root の採用へ進む。
- 既存の再生再開意図を `CollectionNavigationAction::resume_slideshow()` に共有する。
  `PreparedCollectionPhysicalNavigation` の従来の判定も同じ問い合わせへ委譲し、手動 / media EOF 等は false のまま。
  新しい状態や通知済み flag、待機・再試行は追加しない。
- 先に別経路で Depth が退役した場合、後続の close は同じ helper が Depth 非所有として通知しない。
  detached の述語・viewport 分岐・close helper 本体は変更しない。

Collection の新規焦点試験と、前回指定回帰・folder_nav / next_folder / staged・該当する Collection 既存回帰を実行した結果は以下に記録する。

追加した `src/app/collection_navigation/depth_slideshow_tests.rs` の 4 試験は、実 Collection actor の
登録・snapshot・prepare・preflight を使う。各採用経路を自動再開 / 手動で通し、2D の着地と再生再開、
通知 1 回 / 0 回を確認する。採用後の再 close でも通知が増えないことを検査する。
拒否試験は legacy の準備済み root owner 欠落と、準備対象に含まれない target の不採用を通し、
元の Depth / ページ / generation を保持して通知しないことを確認する。
physical continuation は Folder の準備済み payload を既存の physical adoption callback へ渡し、
`PreparedCollectionPhysicalNavigation::install` と finish の 2D 再開まで通す。
新規試験による GPU 作成・製品 exe 起動はない。

追補の型検査は 06:04:41 に開始し、23.67 秒で exit 0。
焦点 `test(/depth_slideshow_folder_tests|collection_navigation::depth_slideshow_tests/)` は
コンパイル 5 分 29 秒、**8 / 8 成功、18,650 skip、2.479 秒、exit 0**。
新規 Collection 4 件と通常 / ZIP / PDF の既存 4 件を同じソースで確認した。
型検査ログは `target/771-logs/collection-depth-check.{out,err}.log`、
焦点の JUnit は `target/771-logs/collection-depth-focus-passed.junit.xml`。

指定回帰の初回は **1,678 実行・1,677 成功・1 失敗、16,980 skip、316.110 秒、exit 100**。
既存 `ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`
の逆方向の面積比較が `[144, 162, 0, 0]` で失敗した (line 488)。
同じソースで同試験だけを診断実行すると **1 / 1 成功、18,657 skip、2.862 秒、exit 0**。
試験は `Instant::now()` を使った描画時刻で面積の単調変化を比較するが、初回失敗の原因は未確定であり、
今回の通知修正・製品の紙めくり処理・試験コードは変更せず、指定フィルタ全体を再実行した。
初回失敗ログは `target/771-logs/collection-depth-validation-failed-1.{out,err}.log`、
JUnit は `collection-depth-regression-failed-1.junit.xml`、単独成功は
`collection-depth-page-turn-diagnostic-passed.junit.xml` (いずれも同じログディレクトリ)。

追補の最終検証は以下のとおり。Cargo はすべて指定の
`run-heavy.ps1 -Owner "771-codex" -WorkDir G:\home\mimageviewer-771` と
`MSBUILDDISABLENODEREUSE=1` 経由で実行した。共有全件ロックの待機は中断していない。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、23.67 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow_folder_tests\|collection_navigation::depth_slideshow_tests/)'` | 8 / 8 成功、18,650 skip、exit 0、2.479 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/page_turn_mainwindow_submits_visible_vacant_leaf_both_directions/)'` | 診断実行 1 / 1 成功、18,657 skip、exit 0、2.862 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|slideshow\|page_turn\|final_prepare\|prepared_edit\|panorama\|export\|admission\|ring/)'` | 再実行 1,678 / 1,678 成功、16,980 skip、exit 0、303.506 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_nav\|next_folder\|staged/)'` | 241 / 241 成功、18,417 skip、exit 0、29.644 秒 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/collection_navigation::/) & test(/outer\|slideshow\|preflight\|prune\|reject\|cancel\|reus\|epub\|convert\|root\|intent/)'` | 44 / 44 成功 (既存 40 + 新規 4)、18,614 skip、exit 0、11.215 秒 |
| `cargo fmt` (引数なし、最後の Cargo 操作) | exit 0 |

単独診断 → 指定回帰 → フォルダ移動回帰 → Collection 関連回帰 → cargo fmt を順に実行し、
06:22:52 に runner exit 0。ログは `target/771-logs/collection-depth-validation.{out,err}.log`、
成功時の JUnit は `target/771-logs/collection-depth-{regression,folder,existing}-passed.junit.xml`。
runner の書込み警告後は既定の BelowNormal / 6 build jobs / 既定 TEMP で続行した。
依存物の MSBuild 再失敗は発生していない。

今回までの未コミット変更は次の 4 ファイル。

- `src/app.rs`: 通常 / physical staged 採用の共有通知 helper と接続、ZIP/PDF と二重 close の回帰試験。
- `src/app/collection_navigation.rs`: Collection の 3 か所の接続、再生再開意図の共有問い合わせ、拒否判定を close 前に配置、試験 helper の共有。
- `src/app/collection_navigation/depth_slideshow_tests.rs`: 実 actor / prepared payload による 3 採用経路と不採用の新規 4 試験。
- `docs/depth-slideshow-plan.md`: 同形経路の棚卸し、設計担当の回答、変更・試験・検証の記録。

Collection の適用範囲についての残件は解消した。設計担当の回答から外れた実装はない。
既存の D3 設定参照・先読み・照合・タイマー・perf の実装と前提の確認結果は、本節の先行記録から変更していない。
製品 exe 起動・commit・追加の独立レビューは行っていない。
本体 lib 全件・`build-dev.ps1` は設計担当の担当として未実施。実機表示と初回の既存試験失敗の原因は未確認。
最終の `python scripts/check_ui_glyphs.py` は危険グリフ 0 件・exit 0、`git diff --check` も exit 0。

P3 対応 (独立レビュー第 2 回後): root owner / target の拒否判定を installer 冒頭へ集約し、採用確定後・root 変更前に呼出し側の `FnOnce` で通知・close を実行する形へ変更 (永続状態の追加なし)。既存の owner 欠落試験に「拒否時は closure を呼ばない」検査を追加し、Depth 保持・非通知の既存回帰も成功。指定 runner / `MSBUILDDISABLENODEREUSE=1` 経由で `cargo check -p mimageviewer --bin mimageviewer-core` exit 0、`cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow/)'` 25 / 25 成功 (11.458 秒)、`cargo nextest run -p mimageviewer --lib -E 'test(/collection_navigation::/)'` 63 / 63 成功 (13.771 秒)、最後の `cargo fmt` (引数なし) と `git diff --check` は exit 0。ログ: `target/771-logs/p3-validation.{out,err}.log`、JUnit: `target/771-logs/p3-{depth-slideshow,collection}-passed.junit.xml`。commit・製品 exe 起動なし。

2026-11-19 利用者観測の「3D 写真パネルがスライドショーの円を隠す」件: コードで描画順・通常 layer / Foreground・固定バー時の重なりを確認し、`slideshow_progress_ring_center` で半径 8 + 余白 4 の範囲がパネルと交わる場合だけ円を左へ移すよう修正。`visible_depth_panel_rect` は旧矩形を保持し得るため円には使わず、`draw_depth_panel` のそのフレームの戻り値 `DepthPanelResponse.rect` を渡し、未描画時は None とする (既存キャッシュの寿命・パネル位置・円の半径 / 色 / 進み方を維持、新しい設定 / 永続状態なし、detached 述語 / viewport 経路の変更なし)。純粋関数 2 件・旧キャッシュ無視 1 件・実 3D 描画で展開 / 折りたたみ / 非表示と固定 / 非固定バーを切り替える GPU 試験 1 件を追加し、既存 2 試験の戻り値参照を対応。初回のキャッシュ削除案は既存入力試験 1 件が失敗 (71 / 72、exit 100) したため撤回、戻り値変更に伴う既存試験のコンパイルエラー 1 件 (exit 101) も修正済み。最終検証は指定 run-heavy / MSBUILDDISABLENODEREUSE=1 経由で `cargo check -p mimageviewer --bin mimageviewer-core` exit 0、`cargo nextest run -p mimageviewer --lib -E 'test(/slideshow_progress_ring/)'` 4 / 4 成功 (1.849 秒)、`cargo nextest run -p mimageviewer --lib -E 'test(/depth_slideshow|slideshow_tests|depth_focus|depth_main_window|depth_panel/)'` 72 / 72 成功 (51.281 秒)、最後の `cargo fmt` (引数なし) と `git diff --check` は exit 0。ログは `target/771-logs/ring-placement-validation.{out,err}.log`、最終 JUnit は同ディレクトリの `ring-placement-{focus,regression}-passed.junit.xml`、修正前の失敗ログは `ring-placement-{initial,compile}-failed.*`。マニュアル変更・commit・製品 exe 起動なし。利用者実機での修正後の見た目は未確認。
