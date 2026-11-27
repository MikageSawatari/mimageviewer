# 編集ツールの途中で表示中の画像が外から替わるときの扱い (§1.920)

作成: 2026-11-27 / ClaudeCode (設計担当)。状態: 設計 (独立設計レビュー待ち)。

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

`open_fullscreen_with_metadata_boundary_inner` を通るもの (共通の境界で扱える):
メインの一覧のダブルクリック・Enter・ゲームパッドの決定、linked の選択追従 (`sync_detached_viewer_to_selected`)、
ブックマーク、★固定の移動、本の構成待ち・サイドカー復元待ちの再開、スライドショーの自動送り (§3.4)。
MCP の `open_in_viewer`・二重起動・フォルダ移動・D&D・フォルダ監視・ブラウザ・Remote は、先に close か park を
通るので今も終わっている。評価の移動は `finish_rating_membership_edit_boundary` で先に終えている (前例)。

境界を通らないもの (個別に扱う):

- `sync_detached_viewer_to_selected` の「同じ idx で item_key だけ変わった」再オープン (`discard_fs_page` → 同じ idx の open)。
- `remove_items_batch`: 表示中の画像が削除されたとき、閉じずに隣の画像へ `fullscreen_idx` を直接ずらす。
- viewer を閉じる `close_fullscreen_now_for_destination`: `fullscreen_idx = None` の**後**で
  `finish_fullscreen_edit_modes` を呼んでいる (v5.8.0 も同じ順)。`finish_conceal_edit_for_current` と
  `commit_text_page` は先頭で `fullscreen_idx` を読むので、閉じる経路で隠蔽加工・テキストの編集が保存されない
  疑いがある (コード読み、未確認。§5 のテストで確かめる)。

同じ item のまま idx だけ付け替わる経路 (他の項目の削除による詰め、snapshot・評価 Keep・本の構成の付け替え) は
画像が替わらないので、編集ツールは続けてよい (§3.2 の条件に入らない)。

## 3. 設計

### 3.1 方針: 「編集を終えてから切り替える」

**編集ツールの作業は、入った時点の 1 枚の画像に属する。表示中の画像が替わるときは、替わる前に、
利用者が Esc で抜けたのと同じ終了処理で全部の編集ツールを終える。** 外からの切り替えの結果は、
「利用者が Esc で編集を終えてから同じ操作をした」結果と同じにする。

組み合わせを減らす案として次の 2 つを比べた (CLAUDE.md「設計の簡素化」)。

- **案 A (採らない): 編集中は外からの切り替えを受け付けない。** 一覧のダブルクリック・選択追従・MCP 等を
  編集中は拒否する。入口ごとに拒否の通知と、呼び出し側が先に立てた印 (`fs_open_intent_from_grid` など) の
  戻しが要る。linked の選択追従は毎フレームなので、拒否の通知の出し方と追従の再開も決めることになる。
  一覧の削除で表示中の画像が消える場合は拒否できない (削除は止められない)。今はダブルクリックで
  別の画像へ替わる (下書きの持ち越しは誤りだが、替わること自体はリリース済みの挙動) ので、通常の操作が
  できなくなる変更にもなる。
- **案 B (採る): 替わる前に編集を終える。** 既存の終了処理 (Esc と同じ `reset_*`) をそのまま使い、
  新しい状態・通知・拒否の分岐を足さない。close・park・評価の移動が既にこの形 (前例)。保存されるのは
  元の画像で、Undo の履歴が消えるのは Esc で抜けたときと同じ。消しゴムの未適用の筆跡は、今も外から
  替わると捨てている (`reset_erase_mode`) ので挙動は変わらない。

案 B では、表示中の画像の交代と編集ツールの寿命の組み合わせが「替わる前に必ず終わる」の 1 通りになる。
下書きへ対象のページを持たせる案 (下書きを束へ移す・対象 idx を記録する) は、交代の後も下書きが生きる
組み合わせを残すので採らない。

### 3.2 終了処理を 1 つにまとめる

`App::end_page_edit_tools_for_viewer_item_change(reason)` (名前は実装担当が決めてよい) を 1 つ置き、
`is_overlay_edit_mode_active` の 6 ツールを、それぞれの**利用者による退出と同じ処理**で終える:
隠蔽加工 `reset_conceal_mode`、消しゴム `reset_erase_mode`、テキスト `reset_text_mode` (保存)、
補正レイヤーはパネルを閉じるのと同じ退出 (`fold_local_adjust_edit_state` を含む)、書き出しの切り取り
`reset_export_crop_mode`、SNS 分割 `reset_sns_split_mode`。見開きから入っていた場合は、各ツールの退出が
見開きとその位置 (`leave_page_edit_single_view`) を戻す。その後の切り替えは「見開きに戻った状態から
新しい画像を開く」として進む (戻した位置が開く画像と同じなら、同じ画像の open になる。Esc の後に
同じ操作をしたのと同じ)。

呼ぶ条件は「編集ツールが動いていて、表示中の画像 (item) が替わる」こと。新しい bool・Option・印は足さない。

### 3.3 呼ぶ場所

1. `open_fullscreen_with_metadata_boundary_inner`: viewer をまたぐ振り分けの後、既存のコメントが
   「cross-viewer routing 後の最初の共通境界」と書いている位置 (`supersede_required_fullscreen_folder_open` の前) で、
   `fullscreen_idx` が `Some(今)` かつ `今 != idx` のとき。拒否・延期 (`defer_*`) で返る経路より後なので、
   開けなかった open は編集を終えない。延期された open は再開して実際に開く時点で終える。
   ここで終えるので、idx の代入の後にある既存の `reset_erase_mode` は、編集中の画像の交代では何もしなくなる
   (新規の入場と同じ画像の open のためにそのまま残す)。
2. `sync_detached_viewer_to_selected` の `item_key_changed` (同じ idx で item が替わる): `discard_fs_page` の前。
3. `remove_items_batch`: 表示中の画像そのものが削除対象に入っているとき、items と `fullscreen_idx` を
   動かす前 (削除が確定し、画像を隣へずらす前の時点。Esc の後に削除したのと同じ)。表示中の画像が残る削除では呼ばない。
4. `close_fullscreen_now_for_destination`: §2.2 の疑いがテストで確かめられたら、`finish_fullscreen_edit_modes`
   のうち編集の保存に当たる部分を `fullscreen_idx = None` より前へ移す (順序だけを直す。新しい分岐は足さない)。
   確かめられなければ移さず、報告する。

### 3.4 スライドショー

スライドショーのタイマーは編集中かを見ていない。編集ツールへ入るときにスライドショーが止まるかは未確認。
止まらないなら、§3.3 の 1 で自動送りが編集を終わらせてしまう (今は下書きを持ち越す)。viewer 内のほかの
送り (キー・マウス・ページ一覧・シークバー・ゲームパッド) と同じく、`is_overlay_edit_mode_active` の間は
自動送りを進めない (タイマーの条件に同じ判定を足す) ことで揃える。止まることが確かめられたら何もしない。

### 3.5 変えないもの

- 見開きの中の左右の切り替え (`switch_*_target_in_spread`、`enter_page_edit_single_view` で idx を直接替える) は
  ツール内の対象の切り替えで、open を通らないので今のまま。
- viewer 内の入力層の止め方、close・park の既存の退出、detached の述語・viewport・窓の所有・session の開始。
- 下書きの置き場 (App のまま)。

### 3.6 detached リワークの憲法との関係

App に新しい detached 用の bool / Option を足さない (§2.3)。時間窓・再試行・追加の repaint を使わない (§2.5)。
一括 reset でもない: 終えるのは「表示中の画像が替わる」という所有の境界で、終え方は各ツールの既存の退出
(保存を含む) である。下書きの所有者 (表示中の 1 枚) が替わる遷移に、その下書きの寿命を結び付ける構造的修正。
sibling の context (park 中の束) には触れない (編集ツールは mounted の context でだけ動き、park で終わっている)。

まれな失敗は割り切り (ログ + 通知): 保存の失敗は既存の `save_conceal_with_sidecar` 等の扱いのまま。

## 4. 利用者から見た変化

別ウィンドウで編集ツールを開いたまま、メインの一覧で別の画像を開く・選ぶ・表示中の画像を削除すると、
編集中の画像の変更を保存して編集を終え、その後で新しい画像を表示する (Esc で抜けてから操作したのと同じ)。
新しい画像に前の画像の図形は付かない。マニュアルの編集ツールの説明に、別ウィンドウで編集中に一覧から
画像を替えると編集が終わることを 1 文足す (該当箇所がある場合)。

## 5. テスト (状態遷移と sibling 不変)

- 隠蔽加工で図形 1 つ → 一覧からの open で別の idx: モードが終わり、元の idx に図形 1 つ・元の大きさで保存、
  新しい idx には保存なし、`fullscreen_idx` は新しい idx、rescale のログが出ない。
- 同じことを linked の選択追従 (`sync_detached_viewer_to_selected`) と、同じ idx で item_key が替わる場合で。
- 見開きから入った隠蔽加工・消しゴム: 外からの open の後、見開きの設定が戻り、`fullscreen_idx` は開いた idx
  (退避先へ巻き戻らない)。消しゴムの post-filter の戻しは元の idx に効く。
- テキスト: 外からの open で元のページの本文が保存され、選択が残らない。SNS 分割・切り取り・補正レイヤーの
  モードが終わる。
- 開けなかった open (受付の拒否) と同じ画像の open では、編集が続く。
- 表示中の画像の削除: 終えてから隣へ移り、隣の画像に保存されない。他の画像の削除では編集が続く。
- 閉じる経路: 隠蔽加工・テキストの編集が閉じたときに保存される (§2.2 の疑いの確認)。
- park 中の sibling の束の `fullscreen_idx` などが、active 側の外からの open で変わらない。
- 既存の見開き内の切り替えのテストが通る。
