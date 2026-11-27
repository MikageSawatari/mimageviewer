# 編集ツールの途中で表示中の画像が外から替わるときの扱い (§1.920)

作成: 2026-11-27 / ClaudeCode (設計担当)。状態: 実装済み (ブランチ conceal-920、`e20097823`)。独立設計レビュー r1〜r5 (r4 ACCEPT の後、実装担当の指摘で所有者の経路を
追記し、r5 の指摘を反映)、実装中の追記 (スマートフォルダ・補正レイヤーの例外・同じ画像の open の消しゴム)、独立実装レビュー
r1〜r4 (r4 ACCEPT)。本体 lib 全件成功。実機未確認。

関連: [next-release-backlog.md §1.920](next-release-backlog.md)、[conceal-feature-plan.md](conceal-feature-plan.md)、
[detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) と §11 (リワーク外からの変更記録)。

## 1. 観測と原因

観測 (サブ PC の Claude、記録は §1.920 と `C:\home\mimageviewer-subpc\v590-914\results\notes.md` の E-3):
F12 の別ウィンドウ (一覧に連動する linked 窓) で隠蔽加工の矩形を描いたまま、メインの一覧で別の画像を
ダブルクリックすると、別ウィンドウのページだけが替わり、図形が新しい画像の縦横へ引き伸ばされて残る。
パネルを閉じると新しい画像の隠蔽加工として保存される (`conceal: active edit rescaled idx=1 reason=exit_save`)。
v5.8.0 にも同じ経路がある。

原因 (コード):

1. 隠蔽加工・消しゴム・テキスト・SNS 分割の**編集中の下書き** (`conceal_mask` / `conceal_shapes` /
   `conceal_mask_size`、`erase_mask`、`text_selected`、`sns_split` の layout、各 `*_spread_ctx`) は
   viewer context の束 (`ViewerContextBundle`) ではなく App にあり、対象のページを持たない。対象は
   暗黙に「今の `fullscreen_idx`」である (`finish_conceal_edit_for_current` は退出時の `fullscreen_idx` へ保存する)。
2. viewer 内のページ送りは、入力層 (`handle_conceal_keys` など、`is_overlay_edit_mode_active` を見る
   マウス・ページ一覧・シークバー・ゲームパッド) で編集中は止まっている。しかし**表示中の画像を替える
   共通の境界** (`open_fullscreen_with_metadata_boundary_inner`) は、編集ツールを終えずに
   `fullscreen_idx = Some(新)` へ進む。ここで終えているのは消しゴムだけで、それも idx の代入の**後**
   (`reset_erase_mode`、見開きから入っていた場合は退避先のページへ巻き戻す)。
3. linked の別ウィンドウは、メインの一覧のダブルクリック・Enter・選択の変更 (`sync_detached_viewer_to_selected`、
   毎フレーム) のたびに、park も close もせず同じ context のまま別 idx で開き直す。close・park を通る経路
   (`finish_fullscreen_edit_modes`、`reset_detached_pause_foreground_modes`) なら編集ツールは終わるが、
   この経路はどちらも通らない。
4. 下書きが残った状態で、合成の `compose_source_mismatch` が「同じ画像の解像度が変わった」(AI 拡大など) 用の
   `rescale_active_conceal_edit_to_size` を呼び、図形を新しい画像の大きさへ合わせ、退出時に新しい idx へ保存する。

detached リワークの分類では **BA-7** (状態の所有者がはっきりしない — context 固有であるべき編集の下書きが
App にあり、その寿命を決める遷移 (表示中の画像の交代) が下書きの所有者を通らない) に当たる。

## 2. 同型の経路の列挙

### 2.1 編集ツール (`is_overlay_edit_mode_active` の 6 つ)

| ツール | 下書きの置き場 | 表示中の画像が外から替わったとき (今) | 害 |
| --- | --- | --- | --- |
| 隠蔽加工 | App のみ、対象 idx なし | そのまま残る。新しい画像へ引き伸ばされ、退出時に新しい idx へ保存 | **§1.920 本体** |
| 消しゴム | App のみ | idx の代入の後に `reset_erase_mode` (未適用の筆跡は破棄、もともと保存は [E] の適用だけ) | 見開きから入っていると退避先へ巻き戻る。post-filter の戻しが新しい idx に効く |
| テキスト | 本文は `comic_docs[page_path_key]`、選択・Undo は App | そのまま残る。新しいページの本文を編集する形になる | 旧ページの未保存 (debounce 中) の取りこぼし、選択 id の取り違え、巻き戻り |
| 補正レイヤー | 束、layers は idx キー、ドラッグは owner 付き | モードが残り、新しいページの layers を表示 | 小 (対象は明示)。退避先への巻き戻り |
| 書き出しの切り取り | 設定は idx キー、ドラッグは idx 付き | モードが残る | 巻き戻りだけ |
| SNS 分割 | App のみ (旧画像の大きさの layout) | layout が新しいページに載ったまま残る | 書き出し結果が誤り得る (未検証) |

### 2.2 表示中の画像が「外から」替わる入口

**メインウィンドウの一覧から** (linked の別ウィンドウで編集中だけ届く。メインの全画面で編集中は
`viewer_session_blocks_main_window` で一覧に触れない): ダブルクリック・Enter・ゲームパッドの決定
(`open_fullscreen_*` → inner)、選択の変更による追従 (`sync_detached_viewer_to_selected`、毎フレーム)、
一覧からの削除 (`remove_items_batch` が表示中の画像を閉じずに隣へずらす)、操作のスクリプト
(`operation_script_eligibility` が編集中を見て止めている)。

**一覧の入力を通らないもの** (メインの全画面で編集中にも届き得る):

- スライドショーの自動送り。タイマーは編集中かを見ず、編集ツールへ入ってもスライドショーは止まらない。
  本の構成の許可待ちになった `Slideshow` の送りは、タイマーを通らずに採用される
  (`book_composition_permission.rs` の採用条件は `slideshow_playing` だけを見る)。
- 本の構成の採用 (`book_composition_runtime.rs`): items を置き換え、対象が消えたら次・前・先頭のページへ
  `fullscreen_idx = landing` を直接代入する。inner の冒頭の `poll_and_remap_book_target` からも採用される。
- 同じ idx の項目が置き換わる場合 (`sync_detached_viewer_to_selected` の `item_key_changed`)。検出した時点で
  `items[idx]` は既に新しい項目なので、そこで終えても保存先 (`page_path_key`) は新しい項目になる。
- そのほか items を書き換える非同期の完了 (サイドカー復元・内容の同一性の復元・アーカイブ内の本・ほか)。
  表示順・絞り込みの変更で idx が付け替わる場合も含む。
- MCP の `open_in_viewer`・二重起動・フォルダ移動・D&D・フォルダ監視・ブラウザ・Remote は、先に close か park を
  通るので今も終わっている。評価の移動は `finish_rating_membership_edit_boundary` で先に終えている (前例)。

**閉じる経路** (`close_fullscreen_now_for_destination`): `fullscreen_idx = None` の後で
`finish_fullscreen_edit_modes` を呼んでいる (v5.8.0 も同じ順)。`finish_conceal_edit_for_current` と
`commit_text_page` は先頭で `fullscreen_idx` を読むので、閉じると隠蔽加工の下書きは捨てられ、テキストの
debounce 中の変更も保存されない。窓の × (viewport close、`ui_fullscreen.rs:29477`) は編集中にも届く。
さらに見開きから入った消しゴム・隠蔽加工は、その後の reset が退避先を戻し、閉じた後に `fullscreen_idx` を
`Some` に戻し得る。独立設計レビュー r1 で実在を確認 (コード読み)。

**同じ項目のまま idx だけ付け替わる場合も安全ではない** (r1 P2): 隠蔽加工・消しゴム・テキスト・切り取り・
SNS 分割の見開きの退避 (`PageEditSpreadPivot`) は数値の idx を持ち、`remove_items_batch` などの付け替えで
移されない (`reproject_page_edit_owners` が移すのは補正レイヤーだけ)。

## 3. 設計

### 3.1 利用者の決定 (2026-11-27)

統括経由の利用者の回答: **「A' 一覧を止める (推奨)」**。閉じる経路の保存の順序の不具合とスライドショーの
自動送りも一緒に直す。

- **失う使い方**: F12 の別ウィンドウで編集ツール (隠蔽加工・消しゴム・テキスト・補正レイヤー・書き出しの
  切り取り・SNS 分割) を開いたまま、メインの一覧で画像を替える (ダブルクリック・Enter・クリックや矢印での
  選択・削除など)。今はできるが、編集中の図形を新しい画像へ持ち越す誤った形で動いている。
  別ウィンドウで Esc (またはツールの終了) で編集を終えれば、一覧は従来どおり使える。

### 3.2 比べた案 (CLAUDE.md「設計の簡素化」)

- **案 A' (採る): 別ウィンドウで編集中は、メインの一覧の項目への入力を受け付けない。** メインの全画面で
  編集中に一覧へ触れないのと同じ扱いにする。一覧の入力は既存の共通の門 (`grid_item_input_block`、
  「キー・ポインタ・タッチ・リング・ゲームパッド・メニューの全部がこの門を通る」) を通るので、理由を 1 つ
  足せば、ダブルクリック・選択の追従・削除などメインの一覧からの持ち込みが 1 か所で止まる。
- 案 B (採らない): 一覧は使えるまま、替わる前に編集を保存して終える。r1 の指摘のとおり、置き換え・
  削除・構成の採用など items を書き換える側を全部数えて、それぞれ旧項目が有効なうちに終える必要があり
  (items への書き込みは 500 か所以上)、漏れが残る。
- 下書きへ対象のページを持たせる案 (束へ移す・対象の identity を記録する) は、交代の後も下書きが生きる
  組み合わせを残すので採らない。

案 A' で、メインの一覧からの持ち込みは「編集中は起きない」の 1 通りになる。一覧の門を通らない残りの
切り替え (§2.2 の後半) は、旧項目が有効なうちに「Esc で抜けたのと同じ終了」で終える (§3.4)。

### 3.3 一覧の門に理由を足す

- `GridItemInputBlock` に「別ウィンドウで編集中」(例: `DetachedViewerEditing`) を足す。条件は、今の viewer の
  session が別ウィンドウ (`current_viewer_session_is_detached_or_switching` など、編集ツールが使える linked の
  session) で、`is_overlay_edit_mode_active()` が真であること。新しい bool・Option・印は足さず、既存の状態から
  導く。
- `item_independent_controls_allowed()` は偽 (一覧の項目に依存しない操作も、この門を見ている所では止める)。
  `GridItemInputBlock` を `match` している所 (キー処理の Esc での表示の切り替え、ゲームパッド・リング、操作の
  スクリプト、場所の移動など) は、新しい理由での振る舞いを決める。Esc はメインウィンドウでは何もしない
  (編集の終了は別ウィンドウの操作)。
- 一覧への入力を受け付けなかったとき (操作の時点) に、メインウィンドウへ通知する:
  「別ウィンドウで編集中です。別ウィンドウで Esc を押して編集を終えると、一覧を使えます」。
  getter の評価時や毎フレームではなく、拒否した入力の時点だけ出す。一覧は `ui.disable()` されるので、通常の
  clicked 応答では拾えない (`ui_main.rs:22196` 付近)。一覧の領域への押下・キーを拒否した時点で出す。同じ操作の
  繰り返しで通知が溜まらないようにする (既存の通知の置き換えに従う)。
- 既存の `handle_stack_refresh_keyboard` (`app.rs:61167` 以降) は、理由によって Esc の stack 切替・再読込・変換などを
  許している。新しい理由はそのまま既存の分岐へ流さず、許す非項目操作を明示する (既定は何も許さない)。
  メインウィンドウの viewport・文字入力の所有は今のまま (入力欄や IME の Esc は奪わない)。
- 背景のコンテキストメニューには門の例外がある (`context_menu.rs:2003` 付近)。新しい理由では例外を広げず、
  既に開いていたメニューの項目の実行も止める。ゲームパッド・リングは既存の門で止まる。拒否中に押されたままの
  入力が、編集の終了直後に発火しないことを確かめる。
- **一覧の領域への D&D**: 門が閉じると main drop の grid geometry が空になり (`main_drop.rs:126`)、native の
  hit-test はその位置を `ExistingDropArea` とし、外部ファイルを `GridFiles` として現在フォルダへのコピーに読み替えて
  しまう (`main_drop_target.rs:147`・`:253`、コピーは `app.rs:54242` で直接始まる)。従来はフォルダのセルへの
  コピーだった操作が別の場所へのコピーに変わるので、新しい理由では一覧の領域への drop を拒否し (背景へのコピーへ
  読み替えない)、通知する。
- tray の隠す・戻す、activation は、新しい理由を全体のモーダルとして扱わない (既存の session を保つ。activation は
  旧対象を close / park / 終了してから進む今の経路のまま)。
- 判定に使う述語は、sibling を含まない現 session の述語 (`current_viewer_session_is_detached_or_switching`、
  `app.rs:66546` 付近) を使う。at-rest を含む `viewer_session_is_detached_or_switching` (`:66564`) と取り違えない。
- **inner の受付 (`open_fullscreen_with_metadata_boundary_inner` の `grid_item_input_allowed`) は、この理由では
  拒否しない。** 一覧以外から届く open (延期の再開・同じ画像の open) まで拒否すると、延期の後始末と同じ画像の
  更新が壊れる。inner では、別の画像への切り替えを §3.4 で扱う。
- 表示順・絞り込み・フォルダの移動など、一覧の項目に依存しない上のバーやツリーの操作がこの門を見ていない場合、
  フォルダの移動は close / park を通るので今のままでよい。表示順・絞り込みで idx が付け替わるものは §3.4 の
  規則に入れる。

### 3.4 届いた切り替えは、旧項目が有効なうちに終える

`App::end_page_edit_tools_for_viewer_item_change(reason)` (名前は実装担当が決めてよい) を 1 つ置く。
`is_overlay_edit_mode_active` の 6 ツールを、それぞれの**利用者による退出と同じ処理**で終える
(隠蔽加工 `reset_conceal_mode`、消しゴム `reset_erase_mode`、テキスト `reset_text_mode`、補正レイヤーはパネルを
閉じるのと同じ退出、切り取り `reset_export_crop_mode`、SNS 分割 `reset_sns_split_mode`)。

**返ったときに成り立つこと**: 6 ツールがすべて終わっている。保存は旧項目へ確定している (または旧項目を
明示した保存の要求が出ている)。編集の見開きの退避 (`*_spread_ctx`、補正レイヤーの退避を含む) は全部消費され、
次のフレームの `reconcile_page_edit_spread_pivots` などで遅れて位置が戻ることがない (r1 P2、補正レイヤーは
パネルの退出だけでは位置が戻らず毎フレームの reconcile に頼っている)。

**所有 context の契約 (r2 P1)**: 下書きは App 共通で、束と一緒には交換されない。編集ツールを持つのは、
入った時点で mounted だった context だけ (park で終わるので sibling は持たない)。helper は、**今の投影先が
その所有 context であるときだけ**作用する。構成のサービスなどが `with_viewer_context` で park 中の sibling を
一時投影して items を書き換える間 (`book_composition_runtime.rs:761`、`viewer_context_registry.rs:6013`) は、
active の編集に触れず、終えもしない (sibling の書き換えは active の表示中の項目を替えない)。所有の確かめは
既存の context の identity で行い、新しい印は足さない。一時投影の間は `mounted_viewer_context_id()` と
`projected_viewer_context_id()` がどちらも投影先を返す (`viewer_context_registry.rs:501`・`:509`・`:869`) ので、
この 2 つの一致は所有の証明にならない。一時投影の前の正当な所有者 (投影の仕組みが退避している元の context) と
照合する (r3)。

**元の所有者を読む経路 (実装担当の指摘で追記)**: 今の `with_viewer_context_state` は元の id をローカル変数
`original` にだけ持ち、callback から読めない。そこで、一時投影の元の id を **registry の投影の表 (`ContextTable`)
が持つ** ようにする。`with_viewer_context_state` が一時の mount に入るときに元の id を表へ記録し、戻すときに消す
(入れ子なら最も外の元の id を保つ。callback が panic しても既存の catch_unwind の後で必ず消す)。表は
「前面の context」(`foreground_viewer_context_id()` など、一時投影中は記録した元の id、そうでなければ mounted の id)
を返す。これは投影の所有者 (registry) の状態で、App に足す detached 用の印ではない。

- App 共通の編集の下書き・編集ツールのモードは、前面の context のもの。今の投影先が前面の context と一致する
  ときだけ、終了 helper は作用し、§3.3 の一覧の門の新しい理由・§3.5 の入場の判定も、前面の context の
  ことして評価する (一時投影中の sibling の処理から見て「編集中」と判定しない)。
- 一時投影中に sibling の処理が `is_overlay_edit_mode_active()` を読むと前面の編集の値が見える問題は今もある。
  本件で足す判定は上の一致を条件にする。既存の読み手の監査は範囲外とし、見つけたものは報告する。
- **閉じる経路も同じ条件にする (r5 P1)**: close は一時投影中にも走る (`close_and_retire_context_with_close_owner` が
  `with_viewer_context_state` の中で `close_fullscreen_to_completion` を呼ぶ、`viewer_context_registry.rs:6419`・
  `app.rs:67823`、全窓の終了では at-rest の active を先に閉じる `app.rs:67959`)。今の `finish_fullscreen_edit_modes`
  は App 共通の編集を無条件に終えるので、前面 A で編集中に sibling B を投影して閉じると、A の下書きを B の保存先へ
  書き得る (前倒しの後) か、捨てる (今)。close の context 固有の後始末は必ず行い、**App 共通の前面の編集の保存・
  終了だけを、投影先が前面の context と一致するときに限る**。B の close の後も A の下書き・モード・保存先が
  変わらず、A 自身の close では正常に保存されることをテストする。
- 表の記録の細則 (r5): 入れ子では最も外の元の id を保ち、内側の退出では消さない。記録した外側だけが、元の投影へ
  戻した後に消す。callback の panic では既存の戻しの後に消してから再送出する。`plan_mount` の受付の失敗では記録を
  残さない (交換処理そのものの panic に回復は足さない)。Building の間 (scope なし) は mounted の id が `None` なので、
  予約中の projected の id を前面の所有者として扱わない。scope の外の mount の切り替え (`plan_mount` / `finish_mount`、
  build / promote) は scope を始めも消しもせず、前面の id は今の mounted の id に従う (古い元の id を残さない)。
  正常終了・入れ子・callback の panic・受付の失敗・build の commit / abort・promote をテストする。

呼ぶ場所 (どれも「旧項目がまだ有効」な時点):

1. **inner**: 受付の拒否 (Remote・`grid_item_input_allowed` の既存の理由) の後、`poll_and_remap_book_target` と
   延期 (`defer_*`) の**前**で、`fullscreen_idx` が `Some(今)` かつ `今 != idx` のとき。「利用者が別の画像を求めた
   時点で編集を終える」とし、延期・後の受付の失敗で開けなくても、終えたままにする (Esc の後に開けなかったのと
   同じ)。終了で見開きへ戻ると `fullscreen_idx` は退避先になる。その後の open はそこからの open として進む
   (退避先 == idx なら同じ画像の open)。r1 の指摘のとおり、終了は表示の移動の lock・holdover にも触れるので、
   旧の編集対象・戻った退避先・新しい対象の関係ごとにテストする。
   **同じ画像の open は今の挙動のまま**にする (実装担当の確認で改訂): idx の代入の後にある既存の
   `reset_erase_mode` は無条件のまま残し、消しゴムは同じ画像の open で終わって見開きへ戻る。これはリリース済みの
   挙動で、既存のテスト `section1458_adjust_target_resets_on_open_mode_shift_and_direction` が「open は編集の
   単ページの退避も畳む」として固定している。ほかの 5 ツールは同じ画像の open で今も終わらないので、続けたまま。
   別の画像への open では helper が代入の前に 6 ツールを終えているので、この `reset_erase_mode` は何もしない
   (見開きの退避先への巻き戻り・post-filter の戻しの idx のずれは起きない)。
2. **items を書き換える側**: 表示中の viewer を開いたまま、所有 context の**項目の identity か index 空間**を変える
   採用 (置き換え・削除・付け替え・並べ替え) は、**編集ツールが動いていれば、その採用の処理に入る前に終える**。
   表示中の項目が残るかどうかで分けない (分けると見開きの退避の付け替えが要る、r1 P2)。同じ項目の
   `content_type` などのメタデータの更新は対象にしない。500 か所の書き込みを個別に直すのではなく、membership /
   index 空間を変える採用の境界に集める。
   - **補正レイヤーの例外 (全件テストの結果で追記)**: 補正レイヤーは、編集の対象と見開きの退避を
     `PageEditOwner` で持ち、同じ項目のまま idx が付け替わる採用 (読み込みの差し替え・metadata の stream の再投影・
     再 index) では `reproject_page_edit_owners` が対象と退避を移して編集を続ける。これはリリース済みの挙動で、
     既存のテスト (`page_edit_owner_tests`・`page_edit_release_tests`) が固定している。表示中の項目が同じ identity の
     まま残る付け替えでは、**補正レイヤーだけ**この既存の付け替えで続け、ほかの 5 ツール (数値の idx の退避しか
     持たない) は終える。表示中の項目の identity が変わる・消える採用では 6 ツールとも終える。
   - **終える時点は、採用が既存の鮮度の検証・状態の捕捉をする前** (r2 P1)。本の構成の採用は、編集の snapshot の
     write epoch の検証 (`book_composition_runtime.rs:1226`)、landing の捕捉 (`:1282`)、補正レイヤーの退避
     (`:1346`) を `install_prepared_aggregate_items` より前に行い、その後 `install_prepared_page_edits` (`:1386`) で
     準備済みの編集を採用する。隠蔽加工の保存は write epoch を進める (`conceal_db.rs:255`)。終了の保存を先に
     済ませ、その結果を含めて既存の鮮度の検証 (古ければ既存の再準備) と状態の捕捉を行う。inner の冒頭の
     `poll_and_remap_book_target` からの採用も同じ。
   - 監査する採用の境界 (r2 が挙げたもの): `install_new_items_inner`、本の構成の採用、`remove_items_batch`
     (一覧からは §3.3 で届かなくなるが、本の追加の取り消しなど一覧以外の呼び出し元がある)、★固定 (snapshot) の
     activate / deactivate (`snapshot_ops.rs:1052` は items の代入より前に viewer の付け替えの状態を捕捉するので、
     その捕捉より前)、smart-folder の準備済み一覧の採用、direct-media reader の items の採用。同じ idx の置き換えを
     起こす実際の producer を特定し、そこで置き換える前に終える (`sync_detached_viewer_to_selected` の検出では遅い)。
   - 実装担当は、編集中に届くかで採用の境界を列挙し、届くものに入れ、届かない理由 (close / park を先に通る・
     一覧の門・モーダル・所有 context でない) を報告に書く。
   - **スマートフォルダ (実装担当の指摘で追記)**: 設置済みの smart session は意図的に固定した結果で、
     「membership / 順序と再利用する metadata (隠蔽・消しゴム・注釈のバッジの集合を含む) は明示の開き直しまで
     変えない」のが既存の仕様 (`invalidate_smart_folder_resort_metadata` は session があると
     `smart_folder_metadata_revision` を進めない、`prepare_smart_folder` の `reuse_metadata` は既存の集合を使う)。
     本件はこの仕様を変えない。終了は、準備済みの結果の検証 (RootReady・install・resident restore) と、
     ChildReady の旧 root の捕捉 (`take_visible_smart_folder_grid`) より**前**に置く。そうすれば終了の保存は、
     利用者がその時点で Esc で保存したのとまったく同じ扱いになる: session が無い間は既存の失効 (保存の経路が
     通る revision) と検証で準備がやり直しになり、session がある間は固定結果の既存の扱い (直接のセルの状態は
     更新され得るが、再利用する metadata は変わらない) のまま。固定結果のバッジが保存の後に古く見える場合が
     あるのは今の Esc の保存と同じ既存の挙動で、本件の範囲外 (見つけたら報告する)。テストは、終了が検証・捕捉より
     前に起きること、旧項目へ保存されること、session が無い間の準備が既存の検証でやり直されることを確かめる。
3. **閉じる経路**: `close_fullscreen_now_for_destination` で、編集ツールの終了を `fullscreen_idx = None` より前へ
   移す。保つ順序 (r2): 評価の履歴の退避を fold / reset より前 → local pending の fold → edit preview の snapshot →
   編集の保存 → preview の enqueue。`restore_grid_cursor_to_fullscreen_item` が退避先を読む前に、テキスト・
   切り取りなどの退避を捨てない。閉じた後は `fullscreen_idx == None` で、退避 (補正レイヤーを含む。今の
   `finish_fullscreen_edit_modes` は補正レイヤーの退避を消費しないので、呼ぶ位置を変えるだけでは足りない) が全部
   消費されている。park と true close を混同しない (park は `reset_detached_pause_foreground_modes` のまま)。
   窓の × (viewport close) も同じ保存・終端の契約へ届く。

### 3.5 スライドショー

**編集ツールへ入るとスライドショーを止める** (既存の停止と同じ処理、`slideshow_playing = false`)。編集中は
スライドショーの開始を受け付けない (キーは編集のキー処理が吸収、HUD・メニュー・ゲームパッドなど他の開始の
入口も編集中は止める)。受理済みの `Slideshow` の送り (本の構成の許可待ち) は、採用条件が `slideshow_playing`
を見るので、止めた時点で採用されない (実装で確かめる。採用されるなら同じ要求の所有者で取り消す)。

受理済みの自動送りは、ほかにも次の形で残る (r2 P2): 次のフォルダへの送りは開始時に `slideshow_playing = false`
にしてから、完了側が `resume_slideshow` で再開する (`ui_fullscreen.rs:45471`、`app.rs:64441`)。ZIP の DFS の
`Slideshow` の continuation は完了時に再生を true にする (`ui_fullscreen.rs:45387`)。既存の停止 (`:45297`) は
これらを取り消さない。紙めくりの完了待ち (`PresentationState::Completing.continuation`) も、解決済みの移動を
保持して完了後に実行し、`Slideshow` でも `slideshow_playing` を見ない (`page_turn/barrier.rs:296`・`:451`、r3 P2)。
**受理済みの移動 (自動送り由来か手動かを問わず、fs_nav の lock・フォルダ移動の pending・DFS の continuation・
構成の許可待ちの受理済み open・紙めくりの完了待ち) がある間は、編集ツールへ入らない** (入場を断り、
「移動が終わってから編集してください」と通知する)。移動が着地すれば viewer の画像が替わるので、入ってから
終わらせるより、入らないほうが組み合わせが少ない。静止した状態での操作は変わらない (遅い読み込みでは待ちが
長くなることがある)。判定は、同じ所有 context の既存の移動の述語を組み合わせて 1 つの関数にし、新しい印は足さない:
fs_nav の lock、フォルダ移動・DFS の pending、構成の許可待ちは受理済み open を表す `open_pending()`
(`book_composition_permission.rs:344`、受動的な許可の読み取りも含む `pending()` は使わない)、紙めくりは
`page_turn_barrier_active()` (`page_turn/barrier.rs:131`)。6 ツールすべて (切り取り・SNS 分割を含む) の入場と、
入場の非同期の完了 (隠蔽加工・消しゴムの素材の準備の後の入場など) に同じ受付の規則を当てる。

タイマーに編集中の条件を足して「一時停止」にする案は、編集の終了後に再開するかという組み合わせを残すので
採らない。編集とスライドショーは同時に成り立たない。

### 3.6 変えないもの

- 見開きの中の左右の切り替え (`switch_*_target_in_spread`、`enter_page_edit_single_view` で idx を直接替える)。
- viewer 内の入力層の止め方、park の既存の退出、detached の述語・viewport・窓の所有・session の開始。
- 下書きの置き場 (App のまま)。メインの全画面の編集中の一覧の扱い (今も触れない)。

### 3.7 detached リワークの憲法との関係

App に新しい detached 用の bool / Option を足さない (§2.3)。一覧の門の理由は既存の状態から導く enum の値。
時間窓・再試行・追加の repaint を使わない (§2.5)。一括 reset でもない: 終えるのは「表示中の画像が替わる」
という所有の境界だけで、終え方は各ツールの既存の退出 (保存を含む)。一覧を止めるのは、メインの全画面での
編集と同じ「編集中は一覧という別の入口から viewer の画像を替えない」を別ウィンドウにも当てるもので、
linked の別ウィンドウだけが一覧を viewer の入口として残していたことを直す。BA-7 (下書きの所有と、その寿命を
決める遷移の所有のずれ) の構造的修正。sibling の context (park 中の束) には触れない。

まれな失敗は割り切り (ログ + 通知): 保存の失敗は既存の `save_conceal_with_sidecar` 等の扱いのまま。

## 4. 利用者から見た変化

- 別ウィンドウで編集ツールを開いている間、メインの一覧は操作できず、操作すると
  「別ウィンドウで編集中です。別ウィンドウで Esc を押して編集を終えると、一覧を使えます」と出る。
- 編集中の画像の図形が、ほかの画像に付かない。
- 編集ツールを開いたまま viewer を閉じても (窓の × など)、隠蔽加工・テキストの編集が保存される。
- 編集ツールへ入るとスライドショーが止まる。
- マニュアルの編集ツールの説明 (該当箇所がある場合) に、別ウィンドウで編集中はメインの一覧を使えないことを
  1 文足す。

## 5. テスト (状態遷移と sibling 不変)

- 一覧の門: linked の別ウィンドウで 6 ツールそれぞれの編集中に `grid_item_input_block()` が新しい理由を返し、
  ダブルクリック・Enter・選択の変更・削除・ゲームパッドの決定が受け付けられず、通知が出る。編集を終えると
  門が開く。メインの全画面・編集していない別ウィンドウ・独立の別ウィンドウでは返さない。
- inner: 一覧以外からの別の画像の open で、隠蔽加工が終わり、元の項目に元の大きさで保存、新しい項目には保存
  なし、rescale のログなし。見開きから入った隠蔽加工・消しゴムで、`fullscreen_idx` が開いた idx (退避先へ
  巻き戻らない)、見開きの設定が戻る。退避先 == idx の場合。補正レイヤーで終了の後にもう 1 フレーム進めても
  新しい対象から戻らない。同じ画像の open では消しゴムだけ今のまま終わり、ほかの 5 ツールは続く。延期の再開・受付の失敗の場合。
- items の書き換え: 本の構成の採用で、編集対象が残る・消える・全件消える場合に、旧項目へ保存してから
  書き換わる。同じ idx の置き換えを実際の producer から通し、旧キーへ保存・新キーは変わらない。
  `remove_items_batch` の一覧以外の呼び出し元。
- 閉じる経路: 隠蔽加工・テキストの編集が閉じたときに保存され、閉じた後に `fullscreen_idx == None`、退避が
  残らない。見開きから入った場合も。
- 一覧の門の周り: 編集中の一覧の領域への外部ファイルの drop が拒否され、コピーが始まらない (背景への
  コピーへ読み替わらない)。背景メニュー・開いていたメニューの項目が実行されない。拒否中に押されていた
  ゲームパッドの入力が、編集の終了直後に発火しない。入力欄・IME の Esc はメインウィンドウで今のまま効く。
  通知は拒否した入力 1 回につき 1 回で、毎フレーム出ない。
- 本の構成の採用の順序: 終了の保存が鮮度の検証より前に行われ、採用の後の表示・バッジが保存後の状態のまま
  (保存前へ戻らない)。
- スライドショー: 再生中に編集ツールへ入ると止まる。許可待ちの送りが後から完了しても採用されない。
  編集中は開始できない。次のフォルダへの送り・ZIP の DFS の continuation・fs_nav の lock がある間は、編集ツールへ
  入れず通知が出る。紙めくりの完了待ちに次の移動を受理した状態で入場を断り、移動の完了後は入れる。6 ツールと
  入場の非同期の完了に同じ規則が効く。手動の移動が終わった後は入れる。
- sibling: park 中の束の `fullscreen_idx`・items・cache・世代・pending・cancel が、active 側の終了・一覧の門・
  書き換えで変わらない。逆方向: active で隠蔽加工の編集中に、park 中の sibling の構成の更新が一時投影で完了しても、
  active の下書き・モード・保存先が変わらず、sibling の項目へ保存されない。
- 既存の見開き内の切り替えのテストが通る。
