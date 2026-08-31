# R-07 + R-14 の進捗と設計判断

[local-adjust-ownership-brief.md](local-adjust-ownership-brief.md) の作業記録。
**ブリーフが「何を直すか」、これが「どこまで直したか / どう直すと決めたか」**。

作業場所は worktree `C:/home/mimageviewer-localadjust`、ブランチ `local-adjust-ownership`
(master HEAD `cfa5e309` 起点)。master 側では別セッションが細かいバグ修正を並行している。

## 済み — 段 (c) 契約変更

| コミット | 内容 |
| --- | --- |
| `116ca8cd` | 毎フレームの文書複製を廃止し、`LocalAdjustCanvasEdit` で「何をしたか」を申告する契約へ |
| `a9f69d14` | Codex レビュー指摘 3 件 (P2 × 2 / P3 × 1) の修正 |

`cargo test -p mimageviewer --lib` は 6759 passed / 0 failed。新規テスト 15 件。
**各修正は変異させた実装で落ちることを確認済み** (ブリーフ §9 の要求)。

### 判断の記録

- **`PreparedOnly` は保存しない。** 材質化 (昇格 / リサイズ) を「変更だから保存」に
  すると、空振りの操作 1 回ごとに DB 保存と Undo が増える。メモリにだけ残し、次の
  実際の編集で一緒に保存される。
- **材質化は 3 種類に分かれ、取り消してよいのは「作成」だけ。** 空スロットの作成は
  編集を当て込んだ先行投資なので、編集が効かなければ取り消す。昇格とリサイズは内容を
  保つ正規化なので残す (毎回やり直すのは無駄)。
- **ブラシの override 経路だけは寸法違いの既存マスクを置き換える** (他はリサイズ)。
  ここを一律「作成」扱いにして `None` へ戻すと**利用者のマスクを消す**ので、元が
  `None` だったときだけ取り消し可能として扱う。この分岐は既存仕様であり、今回は
  揃えていない。

## 済み — 段 (d-1) 文書の所有権を `Arc` へ

| コミット | 内容 |
| --- | --- |
| `a6367fb1` | 文書を `local_adjust_core::LocalAdjustmentLayers` (= `Arc<Vec<LocalAdjustmentLayer>>`) で配り、書き手だけが `Arc::make_mut` で複製を払う形へ |
| `c093f0d7` | Codex レビュー指摘 (P2 / P3) の修正 |

`cargo test -p mimageviewer --lib` は 6763 passed / 0 failed。新規テスト 4 件
(所有権 3 + サイドカー形式 1)。**4 件とも変異させた実装で落ちることを確認済み**
(強制複製 / `Arc::get_mut` / before を編集後に取る / 直列化名の変更)。

### 消えた複製

着手前の見積もりは「サイドカー用の 2 枚目と `items_mut` の 3 枚目」だったが、
実際に見つかったのはもっと多い。**一番大きいのは見落としていた**:

- `apply_local_adjust_panel_actions` が **1 フレームに 2 枚**深い複製を作っていた
  (`.cloned()` と `before_layers = layers.clone()`)。しかも要求の有無を見る**前**なので、
  補正パネルを開いているだけで毎フレーム 2 枚。
- `LocalMaterializeLayers::Memory` — 描画 worker へ渡すたびに 1 枚。
- `set_local_adjust_layers_for_idx` のサイドカーミラー用に 1 枚。
- `set_local_adjust_layers_for_idx_with_undo` の `after` 再取得で 1 枚。
- `*_before_layers` 3 フィールド (図形ドラッグ / キャンバスドラッグ / ブラシ) が
  各操作の開始時に 1 枚。

### 判断の記録

- **型エイリアスを `local_adjust_core` に置いた。** `undo_stack` / `sidecar` /
  `edit_bundle` / `books` が全部この型を持つので、所有権の方針を型が定義されている場所に
  1 度だけ書く。
- **Undo の `before` は編集より前に取る**という要件がここで初めて**壊れうる**ものになった。
  複製していた頃は「後で取っても複製だから別物」だったが、共有所有では live と同じ割り当てを
  指すので、後で取ると `Arc::make_mut` で分岐した後の姿を掴み、before == after で
  Undo が捨てられる。seam にコメントを置き、テストで固定した。
- **Undo スタックも `Arc` にした。** ここを `Vec` のまま残すと `capture_local_adjustment_undo`
  で `(*arc).clone()` が要り、消したはずの複製が戻る。スコープ拡大ではなく、
  変更が意味を持つための必要条件。
- **必要になったときだけ複製する形へ 2 箇所直した**。`materialize_local_adjust` は
  寸法が合っているなら複製しない (以前は無条件に複製してから寸法を見ていた)。
  `local_adjust_layers_until` は prefix が有効なときだけ `to_vec` する。
- **サイドカーの形式は変わらない** (`serde` の `rc` feature が `Arc<T>` を `T` として
  直列化する)。出荷済みのディスク形式なので、配列であることをテストで固定した。
  フォルダ移動時の復元がここを読む。

### Codex レビューで直したこと (`c093f0d7`)

- **[P2] 確定のたびに必ず複製していた。** 両 persist 経路が「live の `Arc` を複製 →
  無条件に `Arc::make_mut`」の形だった。マップが常に 1 つ握っているので、**refcount は
  必ず 1 より大きく、`make_mut` は必ず実複製になる**。落とすものが無い通常の
  ストロークでも 96MB。この段の目的の大半をここで失っていた。
  「落とすものがあるときだけ複製する」を単一の owner
  (`compact_local_adjust_manual_override_in`) に集約し、判定と実際の圧縮が一致することを
  テストで固定した (両方向に食い違うと害があるため)。
- **[P3] ポインタ同一性のテストが無かった。** 上の退行を検出できる形
  (複製しない確定 / 複製する確定) と、未検査だった `PageEditBundle::prepare` の
  直列化経路 (貼り付け → `local_adjust.db` JSON) を追加。

Codex は **意味的な別名化・スナップショット順序の欠陥は無し**、サイドカーと
`local_adjust.db` の形式は読み書き両方向で不変、と確認している。

## 未修正で見つかったもの — 補正パネルの編集器が毎フレーム 1 レイヤーを複製する

**[Codex P1、段 (b) で扱う]** `draw_selected_local_adjust_layer_editor`
([src/ui_adjustment_panel.rs](../../src/ui_adjustment_panel.rs) `3566` 付近) の
`let mut edited = layer.clone();` が、補正パネルを開いている間**毎フレーム**走る
(`draw_local_adjust_panel` の入口ガードは `local_adjust_mode` + フルスクリーンだけ)。
`LocalAdjustmentLayer` の複製は `RasterVectorMask::alpha` を再帰的に複製するので、
24MP なら 1 枚 96MB。`manual_override` の add / subtract も持てば最大 3 枚。

**これは段 (c) で直したのと同じ形の契約**である。「文書を複製 → UI に貸す →
変わっていたら publish」で、複製が正しさ (何が変わったかの判定) を支えている。

調査済み: **編集器はマスクの画素を書かない。** 書くのは
`mask_inverted` / `opacity` / `mask_before_effect` / `mask_after_effect` /
`mask_expand_px` / `mask_feather_px` の scalar、`mask` の**パラメータ**
(グラデーション幾何・許容幅・target RGB)、`shapes` (ベクタ、太さスライダを動かしたときだけ)、
`effect` の 4 種類だけ。原寸の `alpha` 配列は読むだけ。つまり**複製の大部分は純粋な無駄**。

直し方の候補を検討した結果:

- `Arc<LocalAdjustmentLayer>` にしても効かない。egui のウィジェットは毎フレーム
  `&mut f32` を要求するので、`Arc::make_mut` が毎フレーム走る。
- 画素配列だけ抜いて貸し、戻すときに差し戻す形は、抜き忘れが静かに壊れる。
- **正解は「編集中のレイヤー」を App の state owner に持たせること** — 選択が変わった
  ときに 1 度作り、変更を publish する。これは**段 (b) が作ろうとしている
  「保留中の編集の単一 state owner」と同じもの**なので、段 (b) に畳む。

## 済み — 段 (d-2) 保存の worker 化 (R-26 を開け直さない)

| コミット | 内容 |
| --- | --- |
| `976aef4a` | 保存を [src/local_adjust_write_worker.rs](../../src/local_adjust_write_worker.rs) へ出し、`EditStoreOutcome` の境界を完了側へ移した |
| `98f4dc9b` | ドキュメント (async-architecture §5.5.x / §5.7、preset-and-adjustment §4.0.1) |
| `393fb6d4` | backlog の R-07 (c)(d) / R-14 を閉じ、(b) へ引き継ぎ |
| `d6a591b2` | Codex レビュー指摘 (P1 × 4 / P2 × 3 / P3) の修正 |

`cargo test -p mimageviewer --lib` は 6785 passed / 0 failed。新規テスト 19 件
(worker 10 + 状態遷移 9)。ブリーフ §4 の指示どおり**失敗経路を含む状態遷移テストを
先に書いてから**実装し、**6 つの変異がそれぞれ意図したテストだけを落とす**ことを確認した。

### 着手前の見積もりとの差

- **保存はストローク単位ではなく毎フレームだった。** `Slider::changed()` はドラッグ中
  毎フレーム真なので、マスク系スライダーを動かしている間ずっと 70.6ms の直列化 +
  SQLite が走っていた。ブリーフの「70.6ms/stroke」は実際にはフレーム単位。
  → **合体は最適化ではなく必須**になった。要求 1 件が原寸マスクを抱えるので、
  合体しないとドラッグ中にキューへ 96MB/frame が積み上がる。
- **終了時の drain に停止フラグは要らなかった。** 送信端を落とすことを停止の合図に
  すると、`Receiver::recv` はキューが空になって初めて `Disconnected` を返すので、
  在庫は必ず書き切ってから止まる。`rating_write_worker` の
  「フラグ + 200ms ポーリング」より待ちもポーリングも無い。
- **見落としていた順序ハザードが 1 つあった。** 製本ページの key 付け替え
  (`copy_book_page_edit_key` / `move_book_page_edit_key`) は worker を経由しない。
  同期保存だった頃は構造的に起きなかった「付け替えの後に古い key の保存が着地して
  行を書き戻す」順序が、非同期化で成立する。両バッチの先頭で
  `wait_for_local_adjust_writes` を呼んで塞いだ。

### R-26 の境界をどこへ移したか

判断は 1 か所 (`App::apply_local_adjust_write_completion`) に集約した。

| 結果 | ミラー | トースト | 画面 |
| --- | --- | --- | --- |
| `Committed` | **worker が実際に書いた文書**を写す | なし | そのまま |
| `Unavailable` / `Failed` | **書かない** | 出す | そのまま (やり直せる) |
| `Superseded` | 書かない | **出さない** | そのまま |

- **ミラーする文書は完了が運んでくる。**メモリから取り直すと、積んでから完了までの間に
  入った編集を「保存済み」として写す (書いた内容とミラーがずれる)。
- **`Superseded` は成功でも失敗でもない。**トーストを出すと誤報、ミラーを書くと
  新しい編集を古い内容で上書きする。
- **サイドカー座標は積む時点で固定する。**完了が返る頃には一覧の idx が別のページを
  指し得るので、そのとき解決し直すと無関係なページへ書く。idx 依存の後続処理
  (`record_content_identity_for_idx`) は `page_path_key(idx)` が同じ key を返すときだけ。

同期版より**強い**点: `Unavailable` の意味が「起動時に開けなかった」から
「**書き手が開けなかった**」に変わり、判定する主体と書く主体が一致する。一時的に
開けなかっただけのときに以後ずっと保存できない状態にも落ちない。

### 計装

この経路には perf イベントが 1 つも無かったので、同時に入れた:
`local_adjust/save_enqueue` (UI 側)、`local_adjust/save_done` (worker 側、所要時間 +
`outcome` + レイヤー数)。ブリーフ §10 の「perf log で確認」はこれで測れる。

### Codex レビューで直したこと (`d6a591b2`)

**P1 が 4 件。うち 2 件は自分が問題の半分だけを見て作り込んだもの。**

- **[P1] `drain_blocking` が永久ハングし得た。** 「積んだ数と終わった数が一致するまで
  受け取る」形にし、取りこぼしを防ぐつもりで `done` を送信の**後**に進めた。取りこぼしは
  塞がったが、**最後の結果を受け取った直後・カウンタが進む前にもう一度 `recv` して
  永久に待つ**ロストウェイクアップを作った。製本 key 付け替えの両バッチが UI ごと
  固まる。**カウンタを前に進めても後に進めても、どちらかの欠陥が残る**ので、
  カウンタ待ちをやめて**フェンス + ACK** にした。
- **[P1] 合体していなかった。** 取り出し**後**に追い越しを判定する形では、判定するまで
  キューが全要求を抱えたままになる。要求 1 件が 96MB なので、この作業が消そうとしている
  メモリそのものが積み上がる。key ごとに未処理の文書を 1 つだけ置く枠へ変更した
  (`Mutex + Condvar` キュー)。
- **[P1] 死んだ worker への保存が黙って消えていた。** spawn が `expect`、`submit` が
  送信失敗を無視、ハンドルは `Some` のまま。メモリと presence だけが進み、正本には
  何も書かれず、トーストも出ない。生存フラグ (drop guard が落とす) を見て
  `submit` がその場で `Failed` を返す形にした。panic した worker が残したフェンスも
  drop guard が落とすので、待ち手が死体を待たない。
- **[P1] リネーム移行の barrier に入っていなかった。** `rename_migration_writers_busy` は
  tag / rating / preview を見て待つのに、補正レイヤーだけ抜けていた。付け替えの後に
  古い key の保存が着地すると、移した行が復活する。未回収の完了も「終わっていない」に
  数える (完了を適用するとサイドカーへミラーが書かれるため)。
- **[P2] 古い完了を捨てるのが間違いだった。** 「新しい要求が控えているから」と受理済みの
  古い完了を捨てると、その新しい要求が失敗したときに**正本には古い内容があるのに
  sidecar には何も無い**状態で止まる。完了は key ごとに順に届くので、受理された完了は
  すべて publish し、記録を畳むのは最新の完了だけにした。
- **[P2] 完了しても repaint を要求していなかった。** 最後の保存の直後に egui が眠ると、
  失敗トーストとミラーが次の入力まで出ない。積んでいる間だけ 50ms の repaint を要求する。
- **[P2] worker の接続に busy_timeout を明示した。** 非 WAL の DB に 2 本目の書き込み接続を
  足したので、rusqlite の既定 5 秒に任せると「競合で止まっている」のか「壊れている」のか
  区別できない。ここは早く失敗する方がよい (画面の編集は残り、やり直せる)。
- **[P3] R-26 の doc comment を `EditStoreOutcome` へ戻した。** 構造体を doc comment と
  enum の間へ挿入してしまい、**境界の説明そのものが別の型を指していた**。

## 未修正で残したもの (Codex レビュー、いずれも既存の形)

いずれも今回の非同期化で**新しく作った欠陥ではない**が、2 本目の書き込み接続ができた
ことで届きやすくはなっている。段 (b) 以降か、別の作業で扱う。

- **`LocalAdjustDb::get_layers_json` がエラーを `None` へ潰す。** SQLITE_BUSY を
  「行が無い」と読むと、sidecar import が正本の行を上書きし得る。判定と読み取りを
  分ける必要があり、import 側の再試行方針とセットで考える話。
- **起動時に `local_adjust_db = None` になったら、そのセッション中は復帰しない。**
  worker は開き直すので「worker は書けるが UI は読めない」状態があり得る。
  遅延ロードとページ hydration が空を返す。
- **完了時のサイドカーミラーが UI スレッドで同期 I/O になり得る。** フォルダを離れると
  sidecar cache が落ちるので、`with_sidecar_coords_mut` が古いフォルダの
  `mimageviewer.dat` を読み直す。同期保存だった頃は「編集した瞬間 = そのフォルダが
  現在地」だったので起きなかった。**ミラーを捨てる方が悪い**ので現状を選んでいる。
- **`ContentIdentitySource` を積む時点で控えていない。**idx が動くと content identity の
  記録が単に行われない (誤ったページへ書きはしない)。

## 済み — 段 (b) キー移動 / 回転の確定タイミング

| コミット | 内容 |
| --- | --- |
| `368499d9` | キー編集をセッション化し、編集状態を畳む 8 箇所を単一の router へ集約 |
| `9ccbb934` | Codex レビュー指摘 (P1 × 4 / P2 × 3 / P3) の修正 |

`cargo test -p mimageviewer --lib` は 6800 passed / 0 failed。新規テスト 9 件
(セッション 6 + 監査 3)。**6 つの変異がそれぞれ意図したテストだけを落とす**ことを確認。

### 着手前の見積もりとの差

- **畳む箇所は 14 ではなく 8 だった** (`local_adjust_mode = false` を書く箇所)。
  ただし内訳は想定より悪く、**5 箇所が破棄・3 箇所は何もしていなかった**。
  何もしない 3 箇所はモードを抜けたあとも保留を残すので、次にモードへ入ったときに
  古いジェスチャが生き返る。
- **ブラシも破棄してはいけなかった。**ブリーフは「ブラシは破棄でよい」としていたが、
  塗った画素はメモリの文書に入っているだけで、保存は `persist_*` でしか行われない。
  破棄すると保存されず、`start_loading_items_inner` はそのあと文書ごと消す。
  4 種類とも確定する形にした (取り消しは Esc の別経路が持つ)。
- **キー離しでは閉じられない。**フルスクリーンの編集キャンバスでは egui のキー状態が
  stale になり得る (修飾キーだけ OS 直読みにしてあるのはそのため)。無操作時間で
  閉じる形にした。窓は 700ms — OS の auto-repeat 遅延 (既定 250〜500ms) より長くないと、
  ホールド 1 回が 2 セッションに割れる。

### 監査テスト

`layer_parse_audit` と同じソース走査。実行時テストでは「まだ書かれていない 9 番目の
入口」を捕まえられないため:

1. `local_adjust_mode = false` を書く箇所は、同じ関数内で `fold_local_adjust_edit_state()`
   を呼んでいること
2. ジェスチャの所有者 (`ui_adjustment_panel.rs`) 以外は保留を `= None` で潰さないこと
3. 確定が 4 種類すべての `persist_*` を呼んでいること

### Codex レビューで直したこと (`9ccbb934`)

**P1 が 4 件。うち 1 件は「実装したのに一度も走らない」位置に置いていた。**

- **[P1] 無操作判定が本番経路で走っていなかった。**他の poll と並べて `App::update` の
  末尾に置いたが、そこは `embedded_fs_active` gate (**毎フレーム無条件に return**) の
  後ろだった。補正の編集はまさに in-window フルスクリーン中に行うので、セッションは
  自力で閉じず、畳む箇所まで memory-only のまま残る。保存結果の回収も同じ位置にあり、
  同じく走っていなかった。両方 gate の前へ移し、**位置をソース走査で固定**した
  (到達可能性は実行時テストでは見えない)。
- **[P1] 破棄がまだ 2 箇所あった。**見開きのページ切替とマスクツール切替。監査が
  `ui_adjustment_panel.rs` を丸ごと除外していたので見えなかった。**監査を関数単位へ
  変えたら、さらに 2 箇所** (パネル側のツール切替・マスク対象切替) が出てきた。
  ファイル単位の許可は「同じファイルの別の破棄」を見逃す。
- **[P1] 別の変更が Undo に載る前にセッションを閉じていなかった。**移動 → 削除の順で
  操作すると、削除が B→C を積み、後からセッションが A→C を積む。Undo 1 回で削除まで
  巻き戻り、2 回で A ではなく B に着く。`set_local_adjust_layers_for_idx_with_undo` の
  入口で閉じる形にした (セッション自身の確定は先に `take` するので再入しない)。
- **[P2] Esc がキャンバスドラッグを取り消していなかった。**取り消せないので Esc が
  モード終了へ落ち、**畳む処理が「取り消したはずのドラッグ」を確定していた**。
  ここが「畳む = 確定」と「Esc = 取り消し」が唯一ぶつかる場所。Esc 側にキャンバス
  ドラッグとキー編集を足し、**確定と取り消しが同じ種類を扱う**ことを監査で固定した。
- **[P2] セッションの identity にレイヤーとマスク対象が無かった。**図形 index は
  マスクごとに独立しているので、別レイヤーの同じ番号を 1 つの Undo にまとめていた。
- **[P2] `close_fullscreen_now` の確定位置が遅すぎた。**edit preview の snapshot と
  Undo スタック破棄の**後**に畳んでいたので、確定前の画素がサムネイルとして焼かれ、
  捨てたはずのスタックへ Undo が 1 件戻っていた。関数の先頭へ移した。
- **[P2] 無操作時間が短かった。**`SPI_GETKEYBOARDDELAY` は約 1 秒まであるので、
  700ms では最初の 1 打とリピート以降が別セッションに割れ得る。1200ms にした。
- **[P3] 監査が主張を固定していなかった。**±40 行の窓は関数の所属も順序も見ない。
  実際 `close_fullscreen_now` の確定を関数先頭へ動かしたら窓から外れた。
  関数単位の走査へ書き直した。

**修正中に `debug_assert` が 1 件見つけた**: ジェスチャの無い `before` スナップショットが
確定後も残っていた。`persist_*` はジェスチャが無ければ何もせず戻るので落ちない。残ると
次のジェスチャが `unwrap_or_else` でこれを拾い、**別ページの姿**を Undo の起点にする
(2026-08-29 レビュー R-03 と同じ形)。確定が取り残しを捨てるようにした。

## 未修正で残したもの — detached context と App-global な保留 (Codex P1)

進行中の編集 (既存 3 ジェスチャ + 今回のキー編集セッション) は **App-global** だが、
対象の文書は `ViewerContextBundle` 所有で swap される。detached の描画後にメイン
context へ戻ってから確定が走ると、確定は「いま mount されている map」を `fs_idx` だけで
引き直すので、**detached 側の編集が未保存のまま、同じ index のメインページを保存 /
削除し得る**。

**今回の作業が作ったものではない** (既存 3 ジェスチャが元から App-global で、新しい
セッションが同じ形を踏襲している)。CLAUDE.md の detached 凍結ルールに従い、症状パッチを
入れず [detached-rework-plan.md](../detached-rework-plan.md) へ BA-7 系の報告として記録した。
直すなら 4 つとも同時に動かす必要がある (1 つだけ直すと非対称が増える)。

## 未着手 — 補正パネル編集器の毎フレーム複製 (Codex P1、段 (b) には入らなかった)

**前言を訂正する。**段 (d-1) の記録で「段 (b) の state owner に畳む」と書いたが、
実装しながら見直した結果**別物**だった。(b) の owner は「保留中の編集 (ジェスチャ)」を
畳むもので、編集器が毎フレーム作る作業用コピーとは扱う対象が違う。

現象は変わらず: `draw_selected_local_adjust_layer_editor`
([src/ui_adjustment_panel.rs](../../src/ui_adjustment_panel.rs) `3987` 付近) の
`let mut edited = layer.clone();` が、補正パネルを開いている間**毎フレーム** 1 レイヤーを
複製する。24MP なら 96MB、`manual_override` 込みで最大 3 枚。

**追加で分かったこと** (着手時に再調査しなくてよい):

- **編集器はマスクの画素を書かない。**書くのは scalar (`opacity` / `mask_inverted` /
  `mask_*_effect` / `mask_expand_px` / `mask_feather_px`)、マスクの**パラメータ**
  (グラデーション幾何・許容幅・target RGB)、`shapes` (ベクタ、小さい)、`effect` だけ。
  原寸の `alpha` は読むだけなので、**複製の大部分は純粋な無駄**。
- **フレーム内の順序は塗り → パネル描画** (`ui_fullscreen.rs` の
  `handle_local_adjust_canvas_input` が `draw_local_adjust_panel` より前)。
  これが効いてくるのが次の点。
- **有望な形は `RasterVectorMask::alpha` を `Arc<Vec<f32>>` にすること。**
  そうすると `layer.clone()` が参照カウントの複製になる。塗りの `Arc::make_mut` は、
  前フレームのパネル用コピーが既に落ちているので refcount 1 = 無料。Undo / 保存 worker が
  掴んでいる直後だけ 1 回複製する (文書レベルの `Arc` と同じ挙動)。
- **規模**: `alpha` を書き換えている箇所は crate 全体で 37 前後。`serde` の `rc` feature は
  有効なので、`mask_codec` の直列化はそのまま通るはず (要確認)。

**効かない案** (検討済み):

- レイヤー単位で `Arc<LocalAdjustmentLayer>` にする — egui のウィジェットは毎フレーム
  `&mut` を要求するので `Arc::make_mut` が毎フレーム走り、何も変わらない。
- 編集器の作業用コピーを App の state owner に持たせる — 文書へ publish するときに
  結局 1 複製が要る (`mem::swap` だと編集器が 1 つ前の姿を掴んでしまう)。
- パネル描画中だけ文書を map から取り出す (`remove` → `insert`) — 描画中に
  `self.local_adjust_page_layers` を読む経路が複数あり、それらが空を見る。

## 環境メモ

新しい worktree では **FFmpeg DLL を `target/debug/{,deps/}` へ置かないとテスト exe が
`STATUS_DLL_NOT_FOUND` で起動しない**。`cp vendor/ffmpeg/bin/*.dll target/debug/deps/` 等。
`cargo test ... | tail` は exit code を隠すので、出力そのものを読んで判断すること。
