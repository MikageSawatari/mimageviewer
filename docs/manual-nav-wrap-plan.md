# 通常閲覧の手動前後移動ループ (§1.400) 設計

作成: 2026-10-18 (設計 = ClaudeCode Opus 5.5)。状態: 設計確定 第3版 (実装前独立レビュー 2 回、§14)。
バックログ: [next-release-backlog.md §1.400](next-release-backlog.md)。5ch >>585〜587。

関連: [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)、
[display-pipeline.md](display-pipeline.md)、[collection-playback-plan.md](collection-playback-plan.md)、
[page-turn-animation-plan.md](page-turn-animation-plan.md) (D3 は別ブランチ `pageturn-d3` で進行中)。

## 1. 要望と結論

フルスクリーン (静止画・ZIP/PDF・動画・音声) の**手動の前後移動**で、末尾の「次」が先頭へ、
先頭の「前」が末尾へ移るようにする設定を追加する。既定 OFF。OFF の動作は現行と完全に同じ。

- 設定: `Settings::fullscreen_manual_nav_wrap: bool` (既定 `false`)。
  環境設定の表示ページ「案内と状況表示」の直前に「前後移動」見出しを置き、
  チェックボックス「先頭 / 末尾で反対側の端へ移る (ループ)」を置く。
- 折り返すのは**現在の閲覧順・表示対象の中だけ**。別フォルダ・別の本へは移らない。
- 一覧のカーソル (`grid_cursor_wrap`)、スライドショーの末尾動作 (`slideshow_end_action`)、
  動画の連続再生 + ループとは連動しない (3 つは対象の操作が違う。§9)。

## 2. 対象にする操作 / しない操作

「1 表示単位ぶんの手動の前後移動」だけを対象にする。

| 対象 (折り返す) | 対象外 (現行のまま) |
| --- | --- |
| ページ送りキー (←→ / ↑↓ / PageUp/PageDown のうち前後移動に割り当てたもの、`KeyAction` の前後ページ系) | Home / End (`FsJumpFirst` / `FsJumpLast`)。既に端なら従来どおり境界案内 |
| ホイールによる前後移動 | 固定量ジャンプ (`FsFixedJump*`)。端なら従来どおり境界案内 |
| 画面の左右クリック (`FsCanvasClickAction::PageSide`)、HUD の前後ボタン | 見開きの 1 ページずらし (Ctrl+←→ の nudge)。ペアリング編集であり前後移動ではない |
| ゲームパッドの前後移動 | Ctrl+↑↓ / Ctrl+PageUp/PageDown (フォルダ横断・兄弟移動) |
| 動画・音声フルスクリーンの plain ↑↓ / ホイール / 前後ファイルボタン | スライドショー (独自の末尾動作) / 動画・音声の連続再生 (独自のループ) |
| マウスボタン割り当てで「次 / 前のページ」を選んだもの (同じ入口を通る) | 連結読みのスクロールそのもの (端でスクロールが止まる現行のまま) |
| コレクション直下の項目の前後移動 (最新 prepared 順で解決) | シークバー、ファイル名スタックの Shift+↓↑、一覧のカーソル移動 |

連結読みでは、項目をまたぐ明示の前後移動 (静止画単位の移動) だけが折り返す。スクロールが
終端に達しても次の項目へは移らない (現行どおり)。連結読みのスライドショーは対象外。

## 3. 折り返し先 (着地)

| 表示 | 末尾で「次」 | 先頭で「前」 |
| --- | --- | --- |
| 単ページ | 閲覧順の最初の前後移動対象 (`adjacent_navigable_idx` の対象集合の先頭) | 同集合の末尾 |
| 連結読み | 静止画表示順 (`get_still_image_indices`) の先頭 | 同末尾 |
| 見開き | 最初の表示単位 (`units[0].anchor_idx()`) | 最後の表示単位 (`units[last].anchor_idx()`) |
| 左右分割 | ステップ列の先頭 (`steps[0]`: 最初のページの読み始め側) | ステップ列の末尾 (最後のページの読み終わり側) |

- 分割のステップ列は綴じ方向を反映した読み順なので、先頭 / 末尾のステップがそのまま
  「綴じ方向に合った側」になる。Home / End は分割の側を選ばない (末尾ページの読み始め側に着く)
  ため、折り返しは Home / End へ委ねず、ステップ列から直接選ぶ。
- 閲覧順は絞り込み・詳細ソート・検索の仮想一覧・ファイル名スタックの表示順を適用済みの
  `current_reader_order()` / `get_nav_indices()` / `get_still_image_indices()` をそのまま使う。
  新しい順序の計算は足さない。
- **動けないときは折り返さない**: 前後移動対象が 1 件 (見開きなら表示単位 1 つ、分割なら
  ステップ 1 つ) で折り返し先が現在位置と同じなら、OFF と同じく境界案内を出して何もしない。
  非表示ページを飛ばした結果、許可された移動先が起点の表示単位へ戻る場合も含む
  (単ページは同じ idx、見開きは同じ表示単位の実ページ、分割は同じ source)。
  0 件ではフルスクリーン自体が開かないので扱わない。
- 1 枚の横長ページを分割した 2 ステップだけの一覧では、後半の「次」は同じページの前半へ移る
  (同一ページ内の左右移動。既存の `Split` 同一 source 経路で処理し、再読込しない)。
- **既存の着地規則に従う経路 (本項目では変えない)**: 動画・音声の前後移動とコレクションの
  前後移動は、非同期に移動先 index を決めて通常の open で着地する。また本の構成の許可待ち
  (Checking) を経た別ページ `Split` 着地も、要求した側を保持しない。これらの経路で分割ページへ
  着地するときの側は、既存の側の調整 (`ui_fullscreen.rs` の slice reconcile) が決める
  (直前の側が Left / Right ならそれを保ち、無ければ読み始め側)。折り返さない通常の
  「前」(動画→直前の横長画像) と同じ規則で、折り返しもこれに従い、側を保持する仕組みは足さない
  (折り返しで期待と反対の側に着く場合がある)。直すなら非折り返しの同じ経路とまとめて別項目で扱う。

## 4. 構造: 折り返しは「移動先の解決」で 1 回だけ決める

前後移動は、入力ごとの producer が `spread_page_nav(base_delta)` を呼んで `FsPageNav` を作り、
`handle_fs_navigation` (静止画) / `navigate_native_video_fullscreen` (動画・音声) がそれを
解決・着地させる。境界判定は次の 4 か所にあり、ここだけを折り返し対応にする。

1. `split_page_nav` (分割: `StepMove::AtEnd` → `Boundary`)
2. `spread_page_nav_for_indices` (見開き: 表示単位の範囲外 → `Boundary`)
3. 単ページ `FsPageNav::Delta` の解決 (`adjacent_idx_in_order` / `adjacent_page_navigation_idx`
   が `None`)。`handle_fs_navigation` の通常経路とページめくり barrier 前の先行解決、
   `queue_spread_shift_action_for_frame` の単ページ fallback
4. 動画・音声の候補列 (`collect_manual_media_navigation_candidates`) とコレクションの
   manual request の境界

### 4.1 型

- `FsPageNav` に `Wrap(FsWrapTarget)` を追加する。
  `FsWrapTarget { idx: usize, slice: Option<PageSlice>, forward: bool }`。
  見開き・分割の解決が折り返したときだけ作る。`Target` / `Split` に紛れ込ませない
  (折り返しは隣接移動ではないので、ページめくり演出・隣接判定・先読み方向の判断が
  `Target` と同じ扱いでは誤る。§5)。
- 単ページの `Delta` は現行どおり後段で解決し、解決 helper が「折り返したか」を返す:
  `adjacent_idx_in_order_wrapping(nav, current, delta, wrap) -> Option<AdjacentStep>`、
  `AdjacentStep { idx, wrapped: bool }`。`wrap == false` のときの戻り値は現行の
  `adjacent_idx_in_order` と同値 (既存テストをそのまま通す)。
- 折り返すかどうかは**呼び出し側が明示的に渡す**。helper や resolver の中で設定を読まない。
  手動前後移動の入口だけが `self.manual_nav_wrap_enabled()` (= 設定値) を渡し、
  スライドショー、先読み、隣接判定 (`adjacent_page_turn_direction`)、本の構成の許可待ちの
  継続 (`book_composition_permission`)、Home/End などは `false` を渡す。

### 4.2 呼び出し側の変更

- `spread_page_nav(base_delta)` は手動入口の共通関数なので、内部で
  `spread_page_nav_for_indices_wrapping(nav, fs_idx, base_delta, self.manual_nav_wrap_enabled())`
  を呼ぶ。producer (キー・ホイール・クリック・HUD・ゲームパッド・動画) 側は変更しない。
- `spread_page_nav_for_indices` の既存呼び出し (スライドショー、`adjacent_page_turn_direction`、
  本の構成の継続、テスト) は `wrap = false` の現行関数のまま残す。
- `handle_fs_navigation_with_render_perf`:
  - `FsPageNav::Wrap(target)`: 現在項目が動画・音声なら `start_manual_media_navigation` へ
    `wrap = true` で渡す。静止画なら `Target` / 別ページ `Split` と同じ着地経路
    (`admit_book_page_navigation` → `begin_fs_page_navigation_sequence` →
    `land_still_page_navigation_target`、slice は着地できたときだけ確定) を通し、
    ページめくりは `page_turn_excluded_navigation_with_operation(.., "navigation_wrap", ..)`。
    同一ページ内の `slice` 移動 (`target.idx == fs_idx`) は既存の同一 source 分岐と同じ扱い
    (コレクション分岐にも渡さない。§4.3)。
  - `FsPageNav::Delta`: 解決 helper へ `wrap` を渡し、`wrapped == true` ならページめくりを
    `"navigation_wrap"` で除外する。境界案内は `None` のとき (折り返せなかったとき) だけ出す。
    **各呼び出し箇所は現在使っている列をそのまま使う** (単ページは `adjacent_page_navigation_idx`
    と同じ Audio を含む前後移動対象の列、連結読みは `get_still_image_indices`)。
    `get_nav_indices` (Audio を含まない) へ置き換えない。
  - **本の構成の許可判定へ入力方向を渡す**: `admit_book_page_navigation` は移動先と現在位置の
    列内順から `Next` / `Previous` を推定する (`book_target_navigation_purpose`)。折り返しでは
    これが反転し (末尾→先頭が `Previous` になる)、折り返し先が非表示ページのとき逆方向へ探して
    閉じてしまう。折り返し (`Wrap` と `wrapped == true` の Delta) では、入力方向から
    `BookTargetPurpose::Next` (forward) / `Previous` (backward) を明示して渡す口
    (`admit_book_page_navigation_with_purpose` 等) を使う。折り返し以外は現行の推定のまま。
    許可待ちの継続探索 (`book_composition_permission` の Hidden skip) は `wrap = false` のまま。
  - **Hidden skip の分割対応 (既存不具合の同時修正)**: Hidden skip
    (`book_composition_permission.rs` の `spread_page_nav_for_indices` 結果の match) は `Target` と
    `Delta` だけを受け、分割表示で返る `Split` を `None` 扱いにして fullscreen を閉じる。
    折り返さない通常の分割送りでも同じで、折り返しでは「先頭が非表示」で必ず踏む。
    分割表示 (ステップ列あり) の skip は**source 単位**で進める: `Split(step)` が別 source なら
    その source を次候補にし、同じ source の反対側なら飛ばして次の source へ進む (非表示 source の
    残り半分を再採用しない)。探索は nav 列の端で止まる有限探索 (`wrap = false`) のまま。
    見開き (`Target`) と単ページ (`Delta`) の扱いは変えない。
  - ページめくり barrier 前の先行解決 (`resolved`) も同じ helper と `Wrap` を扱い、
    barrier に渡る移動先が通常経路と一致するようにする。barrier 経由で defer された継続は
    既存の continuation 方針 (演出なしの即時切り替え、理由 `stage1_continuation_instant`、
    `accept_rendition = false`) のまま扱う。`navigation_wrap` の理由は defer されなかった
    通常経路でだけ記録する (barrier 内部は変更しない)。
    **既存の制約 (本項目では変えない)**: barrier の continuation は slice しか保持せず、
    flat 採用後の着地は `BookTargetPurpose::Explicit` で許可判定に入る。演出中 (Turning /
    Completing) に入力した前後移動の移動先が Checking → Hidden になると、折り返しかどうかに
    関係なく次候補を探さず `RequiredTargetHidden` で終わる。折り返しはこの経路に新しい分岐を
    足さず、非折り返しの continuation と同じ扱いにする。continuation へ purpose を持たせる修正は
    barrier を変更中の D3 側 (§5 の契約) で、非折り返しと合わせて扱う。
  - コレクション分岐 (`collection_delta`) は `Wrap { forward }` (別 source のとき) を `±1` として
    渡し、request に `wrap` を載せる (§4.3)。
- `navigate_native_video_fullscreen`: `Wrap { forward, .. }` は `start_manual_media_navigation`
  へ `delta = ±1, wrap = true` で渡す。`Delta` の場合も `wrap = 設定値` を渡す。
- `start_manual_media_navigation(.., wrap)` → `collect_manual_media_navigation_candidates(.., wrap)`:
  `wrap` のとき、現在位置の後ろ (前) から端まで → 反対の端から現在位置の直前 (直後) までを
  候補列にする。欠損した実ファイルを 1 件ずつ飛ばす現行の解決をそのまま使うので、
  末尾の動画が欠損していても先頭側へ進める。候補列に現在項目自身は入れない。
  既存の `collect_matching_media_navigation_candidates` (連続再生のループ) と同じ回転の形にする。
  **現行の `while let Some(idx) = adjacent_navigable_idx(..)` の歩進ループに折り返し版の helper を
  入れてはならない** (端で `None` にならず無限ループになる)。前後移動対象の列を 1 回作り、
  位置から回転させた有限の列にする。
- 音楽ビューの前後ファイルボタン (`music_navigate_file`、`src/ui_music_panels.rs`) も
  `start_manual_media_navigation` を呼ぶので、同じ `wrap = 設定値` を渡す。この入口は
  `current_grid_order()` を使っている (現行のまま。順序の取り方は本項目で変えない)。
- 単ページの見開きずらし系操作 (`FsSpreadShift*`、リングの `ImageSpreadShift*`) は単ページ /
  分割では通常の 1 ページ移動 (`Delta`) なので、Delta の解決側で一律に折り返る。分割表示中は
  これらは**ページ (source) 単位**の移動で、半ページのステップ列を使わない (現行どおり)。
  折り返しても先頭 / 末尾ページへ既定の側で着地する (通常の半ページ送りの §3 規則とは異なる)。
  `queue_spread_shift_action_for_frame` の単ページ分岐 (barrier 中に直接
  `adjacent_page_navigation_idx` を呼ぶ箇所) も同じ helper へ置き換える。見開きでの nudge
  (`compute_spread_offset_nudge`) は対象外で、端では現行どおり何もしない。

### 4.3 コレクション

`CollectionNavigationAction::Manual` に `wrap: bool` を追加し、request 作成時の設定値を載せる。

- 解決 (`resolve_prepared_collection_navigation`、`src/collection_store/prepare.rs`) の既存
  `CollectionNavigationTail::Loop` (スライドショーの `LoopFolder` と連続再生の EOF ループ用) は
  **現在項目を候補に含む** (前進は `0..=position`、後退は見開き partner を含み得る)。これは
  自動送りでは正しい (1 件なら同じものを再生し直す) が、手動の折り返しでは 1 件・1 見開き・
  他候補全欠損のときに自分へ戻ってしまい、§3 の「動けないときは折り返さない」に反する。
  そこで Tail に手動用の `WrapExcludingCurrent` (名前は実装で確定) を足す。折り返し部分から
  **現在の表示単位に属する位置 (primary と、解決できた partner の両方)** を除く。
  既存の `Loop` の意味は変えない。
- `CollectionNavigationAction::tail()` は `Manual { wrap: true }` で新 Tail、`wrap: false` で
  現行の `Stop` を返す。
- `finish_collection_navigation_without_target` の境界案内は折り返せなかったときだけ出る
  (既存の結果分岐のまま)。
- **`wrap` は継続にも引き継ぐ**: `AwaitingManualContinuation` は request とは別の payload を持ち、
  `accumulate_manual` はこの状態にも合流し、残りの入力は新しい Manual request として再発行される。
  `wrap` を continuation にも保持し、両方の合流経路で `wrap` が一致するときだけ合流させ、
  再発行には保持していた `wrap` を渡す (途中で設定を変えても、進行中の連打は開始時の値で揃う。
  設定変更後の新しい入力は合流せず新しい request になる)。
- `handle_fs_navigation` のコレクション分岐へは、別 source の `Wrap { forward }` を `±1` として
  渡す。同じ source 内の左右移動 (`Wrap.idx == fs_idx`) は既存の同一 source `Split` と同じく
  コレクション分岐より前にページ内で処理する。

## 5. ページめくり演出

- **折り返しではページめくり演出を行わない** (Home / End と同じ平らな切り替え)。
  理由: (1) 末尾→先頭は読み進めではなく位置のジャンプで、演出の向きを決められない
  (2 表示単位しかない本では「先頭」は「1 つ前」とも隣接しており、既存の
  `adjacent_page_turn_direction` は入力と逆向きの演出を選んでしまう)。
  (2) 演出の素材準備 (D3 の次表示単位の先回り準備) は最終表示単位の次を `Boundary` としており、
  折り返し先は準備されていない。
- 実装は `Wrap` / `wrapped == true` の時点で `"navigation_wrap"` として除外する。
  隣接判定 (`adjacent_page_turn_direction`) には折り返しを入れない。
- **D3 への契約 (ブランチ `pageturn-d3` が後で追従する)**:
  - ループ ON のとき、手動の「最後の表示単位の次」は「最初の表示単位」、
    「最初の表示単位の前」は「最後の表示単位」になる。
  - 折り返しは演出対象外なので、`next_final_unit_demand` が最終表示単位で `Boundary` を返す
    現在の契約は演出に対しては正しいまま。D3 が「次に表示される単位を先に用意する」目的で
    この需要を使う場合だけ、ループ ON の最終表示単位で最初の表示単位を需要に含めてよい
    (含めなくても折り返しは平らな切り替えとして正しく動く。速さだけの差)。
  - 折り返しの ON/OFF は `App::manual_nav_wrap_enabled()` を単一の読み口とする。
  - barrier の continuation は入力方向 (本の構成の許可判定の `Next` / `Previous`) を保持しない
    (§4.2 の既存の制約)。D3 が continuation の payload を変えるときに、通常の前後移動と折り返しの
    両方で入力方向を保持できるようにするのが望ましい (折り返しでは移動先との位置関係から方向を
    推定してはならない)。
  - 本ブランチは D3 が変更中の nav-lock / sequence / 受信・採用の境界を書き換えない。
    `handle_fs_navigation` には `Wrap` の分岐を追加するだけで、既存分岐の順序と条件は変えない。

## 6. 先読み

先読み (前後ページの準備: `update_prefetch_window`、`page_turn_prefetch_targets`、
`reconcile_paged_prefetch_window`、AI / final effects の先回り、連結読みの先読み、RAW 現像需要)
は変更しない。末尾で先頭を先回りして読み込むことはしない。
折り返し先は Home / End と同じく、移動時に読み込まれる (直前に見ていればキャッシュが効く)。
先頭 / 末尾の先回りは、必要が実際に出てから別項目で扱う (状態を増やさない、§設計の簡素化)。

## 7. 複数窓 (F12 / detached)

- 設定は全窓共通。折り返しの範囲は**操作した窓の mount 中 context の閲覧順だけ**。
  解決はその窓の `items` / `current_reader_order()` / 表示単位キャッシュだけを読み、
  他の窓の状態・キャッシュ・pending を作らない / 変えない。
- 独立 detached 静止画窓の物理順 (`DetachedPhysical` bundle) も、その窓の一覧として同じに扱う。
- 新しい窓間の状態や IPC は追加しない。detached 述語 / viewport 経路には触れない
  (触れる必要が出たら止めて `detached-rework-plan.md` §2 に従う)。
- **既存の制約 (本項目では変えない)**: 動画・音声の候補解決 (`media_navigation_pending` /
  resolver) は App 共通で、新しい要求は owner を比べず進行中の要求を置き換える
  (`start_media_navigation_candidate_resolution`)。別の窓で動画の前後移動の解決中に、ある窓で
  動画の前後移動をすると先の解決が取り消される。これは折り返しの有無に関係ない既存の構造で、
  本項目は新しい共通状態を足さないが、この干渉を解消もしない。上の「他の窓を変えない」は、
  静止画の解決・表示単位・演出状態と、本項目が足す状態についての保証である。
  媒体解決の context 所有化が必要なら detached リワーク側の項目として扱う。

## 8. 境界案内との関係

- 境界案内の表示設定 (`fullscreen_boundary_notice_visible`) とは独立。ループ ON で
  実際に折り返した操作では `FsBoundaryHint::Edge` を作らない。折り返せなかったとき
  (対象 1 件など) は OFF と同じく作る (表示するかは案内設定に従う)。
- 境界案内の文言 (Home / End / Ctrl+↑↓ の案内) は変えない。

## 9. 3 種類のループの違い (マニュアルにも書く)

| 設定 | 対象 | 末尾での動作 |
| --- | --- | --- |
| 一覧のカーソル移動をループ (`grid_cursor_wrap`) | 一覧 (サムネイル) のカーソル移動 | 末尾→先頭へカーソルが回る |
| スライドショーの末尾動作 (`slideshow_end_action`) | スライドショーの自動送り | フォルダ内でループ / 次のフォルダへ / 停止 |
| 動画・音声の連続再生 + ループ | 再生終了時の自動の次へ | 最後の動画・音声の後に最初へ |
| **閲覧中の前後移動をループ (本設定)** | フルスクリーンでの手動の前後移動 | 末尾の次→先頭、先頭の前→末尾 |

## 10. 保存・移行

`Settings` へ `#[serde(default)]` の bool を 1 つ足すだけ。旧版の設定には項目が無く既定 OFF に
なるので移行は不要 (リリース済みの設定 DB を壊さない)。`settings_transfer` の書き出し / 読み込み
対象 (網羅トグルテストを含む) と、環境設定の検索索引 (`ui_dialogs/preferences/search_index.rs`)
にも追加する。`docs/spec.md` の該当表にも追記する。

mIV Remote の Web UI は端末側 (`crates/remote-web/web/app.js` の `changeImage` /
`changeVideoFile`) で独自に前後移動しており、本体の `handle_fs_navigation` を通らない。
本項目では Remote を変更しない (Remote の閲覧は端で止まるまま)。Remote でも同じ設定を効かせる
かは、IPC の設定同期を伴うため別項目で扱う。

## 11. キー操作

新しい操作は足さない (既存の前後移動の意味を設定で変えるだけ)。KeyAction の追加は無い。

## 12. テスト

- helper: `adjacent_idx_in_order_wrapping` の OFF 同値、ON の両端、1 件、current が一覧外。
- 見開き: 最後の表示単位→最初、最初→最後、表示単位 1 つで折り返さない、表紙単独・最終表紙
  設定でも `units[0]` / `units[last]` に着地。
- 分割: 綴じ方向 LTR / RTL それぞれで末尾の次がステップ列先頭、先頭の前が末尾 (側も一致)。
- 単ページ: 絞り込み後・詳細ソート後の順で折り返す。連結読みは静止画順。
- 動画・音声: 末尾の動画→先頭の画像、欠損ファイルを飛ばして折り返す、1 件で折り返さない。
- コレクション: 最新 prepared 順の両端、`wrap` 違いの request を合流させない。
- ページめくり: ON で折り返し時に演出が除外され、理由が `navigation_wrap`。OFF では現行どおり。
- 入口一致: キー・ホイール・クリック・ゲームパッドが同じ `FsPageNav` を得る (spread_page_nav 経由)。
- 境界案内: 折り返したときは `fs_boundary_hint` が None、折り返せないときは `Edge`。
- 複数窓: detached context で折り返しても main context の `fullscreen_idx` / items が変わらない。
- 設定: 既定 OFF、JSON 往復、旧 JSON (項目なし) で OFF、settings_transfer の往復。
- コレクション: 1 画像・1 媒体・1 見開き・他候補全欠損で、両方向とも折り返さず境界案内。
  native swap 待ち中の queued steps が設定変更をまたいでも開始時の `wrap` で揃う。
  1 枚の横長画像 (2 step) のコレクションで同一 source の左右移動がコレクション要求にならない。
- 本の構成: 折り返し先が非表示ページのとき、forward は先頭から後ろへ、backward は末尾から
  前へ有限に探して着地する (閉じない)。Checking からの再開でも方向が保たれる。分割表示
  (LTR / RTL) でも非表示 source を source 単位で飛ばして着地する (非折り返しの分割送りでも同じ)。
- 対象集合: 単ページで Audio が混在する順でも折り返し先が現行の前後移動対象と一致する。
  空列・current が一覧外。
- 入口: touch の PageSide、マウス割り当て、音楽 HUD の前後ボタン、単ページ / 分割のずらし操作。
- 対象外操作の不変: ループ ON でも Home / End・固定ジャンプ・見開き nudge・seek・Ctrl+↑↓・
  スライドショー・再生 EOF の動作が OFF と同じ。境界案内 OFF でも折り返せない場合に
  `FsBoundaryHint::Edge` が生成される。

## 13. 検討して採らなかった案

- **Home / End への置き換え**: 分割で末尾ページの読み終わり側に着地できない。見開きでは同等
  だが、分割と型を分けるより全表示で同じ規則 (表示単位列の端) にした。
- **`Target` / `Split` を流用して隣接判定で演出を外す**: 2 表示単位の本で隣接と区別できない。
- **キーリピート中は折り返さない**: 状態 (押し直し判定) が増え、ホイールでは定義できない。
  利用者が明示的に ON にする設定なので一律に折り返す。
- **末尾で先頭の先読み**: §6。

## 14. 実装前の独立レビュー記録

レビュー担当 GPT-6.1 Sol / xhigh (read-only)、Codex session `01a14f72-a63b-7840-9a4b-779ffad0621d`。

- 第 1 回 (e7ec649c6): ACCEPT WITH CHANGES、P2 5 件・P3 2 件。採用: コレクション Loop が現在項目を
  含む → 手動用 Tail、本の構成の purpose 反転 → 入力方向を明示、continuation の wrap 引き継ぎ、
  同一 source の左右移動をコレクション分岐に渡さない、列を置き換えない規則、試験追加。
  既存制約として明記 (解消しない): 非同期経路・許可待ちでの分割側の非保持、媒体 resolver の
  App 共通 pending。
- 第 2 回 (5b2ee8f4a): ACCEPT WITH CHANGES、P2 2 件。採用: Hidden skip の分割対応
  (非折り返しにもある既存不具合を同じ経路で修正)、分割側の説明を実際の reconcile 挙動に訂正。
  不採用: barrier continuation へ purpose を持たせる変更。非折り返しの continuation にも同じ制約が
  あり、barrier は D3 (`pageturn-d3`) が変更中のため、§4.2 に既存の制約、§5 に D3 への申し送りとして
  記録した。レビュー担当は分割側と媒体 pending を既存制約とする判断には反対しないとした。
