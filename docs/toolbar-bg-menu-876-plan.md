# §1.876 ツールバーの空白の右クリックを全行で効かせる (設計メモ)

- 状態: 設計 (2026-11-23)。v5.7.0 に入れる (利用者決定、統括経由)。Sol / xhigh 設計レビュー: ACCEPT with changes
  (P2-1 AccessKit・P2-3 テスト・P3 説明を反映済み、P2-2 近接クリックは利用者の判断待ち)。
- 対象: `src/ui_main.rs` の `render_toolbar` の背景 interact (`bg_resp`) と、そのメニュー表示。
- 関連: [toolbar-customization-plan.md](toolbar-customization-plan.md) の「空き領域 背景 interact」、
  backlog §1.876。

## 1. 今の作りと問題

- 背景の当たり判定は `ui.interact(capped, ui.scope_id().with("toolbar_bg_menu"), Sense::click())`。
  矩形は**前フレーム**の `toolbar_content_rect` (= ツールバー本体の `ui.scope` の min_rect) を、
  **先頭 1 行の高さ** (`interact_size.y + 6`) に切ったもの。
- 切った理由 (a61dedec3、Codex P2): 前フレームの矩形をこのフレームに登録すると、hit test は
  さらに次のフレームで行われる (= 2 フレーム遅れ)。ツールバーが複数行から 1 行に縮んだ直後、
  背の高い古い矩形がアドレスバー・グリッドの空き部分まで伸び、そこの右クリックを奪い得た。
- 結果、折り返した 2 行目以降の空白では右クリックのメニューが出ない (サブ PC 担当の観測、
  幅 1576・2400 の kittest で 1 行目の右端は出る / 2 行目の右端は出ないことを確認)。
  マニュアルは「ツールバーの空き領域を右クリック」とだけ書いている。

## 2. 直し方

ツールバー本体の `ui.scope(...)` を `ui.scope_builder(UiBuilder::new().sense(Sense::CLICK), ...)`
に替え、その `Ui` 自身の背景 response で右クリックを受ける。`bg_resp` (前フレーム矩形・1 行の切り詰め)
と、その矩形を作るためだけの読み出しは消す。

- **z-order**: egui は `UiBuilder::sense` の付いた `Ui` を、作った時点 (= 子より前) に widget として
  登録し、終わりに矩形だけ min_rect へ更新する (`vendor/egui/src/ui.rs` の `Ui::new_child` /
  `remember_min_rect`)。子の部品 (ボタン・コンボ・検索欄・★・ドライブ・タグ・セクション見出し) は
  背景より後に登録されるので上になり、左右クリック・ドラッグは今までどおり子が受ける。
  今の `bg_resp` を scope より前に登録しているのと同じ並び。
- **矩形**: 登録される矩形はそのフレームの min_rect で、hit test は他の widget と同じく前 pass の矩形で行う
  (1 pass 遅れ)。今は前フレームの矩形をこのフレームに登録し直しているので 2 pass 遅れで、縮んだ直後に
  古い背の高い矩形が下へはみ出した (a61dedec3 の懸念)。新しい作りでは、背景の矩形と下の絞り込みバー・
  CentralPanel の部品の矩形が同じ pass のものになり、同じ pass の中では背景はツールバーのパネル内にあって
  下段と重ならない。これが安全の根拠 (下段のパネル自体は click を感じないので「後に登録されて上になる」は
  根拠にならない。Sol P3)。`interact_rect` は clip_rect と交わる。よって 1 行への切り詰めは要らない。
  なお、アドレスバー (`TS::Folder`) は外部パネルではなくツールバーの scope の中の一部なので、その空白は
  背景メニューの対象になる (今の 1 行目と同じ扱い)。
- **id**: `scope_builder` に `id_salt` を付けない。`ui.scope` と同じく `next_auto_id_salt` を使うので、
  ツールバー内の部品の id (ComboBox・TextEdit・popup の状態) は今と変わらない。
  メニューの popup id は背景 `Ui` の id から作られる (変わってよい。開いたままの状態は持ち越さない)。
- **Sense**: `Sense::CLICK` だけ (FOCUSABLE を付けない。egui 自身の modal 背景と同じ形)。
  `Sense::click()` (= CLICK | FOCUSABLE) だと、`Ui::new_child` が `Rect::NOTHING` で登録した時点で
  AccessKit ノードに Focus/Click と無限大の bounds を書き、終わりの `remember_min_rect` (allow_focus=false)
  では更新されないため、壊れた bounds の背景ノードが毎 pass 出る (Sol 設計レビュー P2-1、
  `vendor/egui/src/context.rs` の `create_widget`)。FOCUSABLE を外せば普通の Ui と同じく bounds を書かない。
  本番は Tab のフォーカス移動を全体で無効にしている (`src/egui_focus_policy.rs`) ので、focus を外して失う操作は無い。
  背景メニューは「設定」→「ツールバー」からも開けるので、支援技術向けの Click 操作を背景に置かなくてよい。
- **メニュー**: `show_sticky_context_menu(&scope_resp.response, |ui| self.draw_toolbar_visibility_menu(ui, false, true))`。
  中身・「既定に戻す」を出さない点は変えない。
- `self.toolbar_content_rect = Some(scope_resp.response.rect)` は残す (他のテストが読む)。
  背景のためだけだった `toolbar_bg_rect` 変数は消す。
- 範囲外の余白: パネルの外周マージンと scope の上下の `add_space(2.0)` は scope の外なので対象外のまま
  (今の 1 行目と同じ)。

## 3. 変わる挙動 (確認済みの差)

- 2 行目以降の空白、および部品のラベル (hover だけの label、例: 「ソート:」の文字以外の非 interactive 部分)
  の右クリックでメニューが出るようになる。1 行目では今も同じ。
- **近くの部品への吸い寄せが 2 行目以降でも効かなくなる**: egui の hit test は、どの click widget にも
  直接当たっていないときだけ、interact_radius (既定 5pt) 内の最寄りの部品へクリックを渡す。
  背景が click を感じると、部品のすぐ外 (5pt 以内) の左クリックは背景に当たって何も起きない。
  1 行目は v2.0.0 から既にこの挙動で、2 行目以降もそれに揃う。Sol の設計レビュー (P2-2) は、これを
  通常の位置ずれの許容を失う変更とみなし、利用者の判断を求めた。**利用者の判断待ち** (§4 の案 B と比べる)。
- タッチの長押し (egui の `long_touched`) も context menu を開く経路なので、2 行目以降の空白でも開く。

## 4. 簡素化の検討

- 「前フレームの矩形を保持し、縮んだときだけ切る」といった状態を足さず、egui の Ui 背景 sense へ
  寄せて保持している矩形の役割を 1 つ減らす (`toolbar_content_rect` は他用途の読み取りだけになる)。
- 案 B (吸い寄せを保つ): 背景を hover だけにし、secondary click / 長押しのフレームで、ポインタが背景の中にあり
  かつ `ctx.interaction_snapshot` の `clicked` / `long_touched` がどの部品も指していないときだけ、背景メニューを
  自前で開く (`Popup` の開閉も自前)。吸い寄せは全行で保たれ (1 行目も v2.0.0 前の挙動へ戻る)、近くの部品を
  外した右クリックはその部品へ行く。egui の公開 API だけで書けるが、context menu の開閉と判定を自前で持つ分、
  コードとテストが増える。
- まれな失敗は扱わない (本件は I/O・永続データに触れない)。

## 5. テスト

kittest で `render_toolbar` を描く (既存の `toolbar_*_tests` / `grid_edge_buttons_1855` の作りを使う)。

1. 幅 1576 と 2400 (zoom 1.0)、ツールバーが 2 行以上になる構成で、**最終行の右端の空白**
   (どの部品にも当たらず content rect 内) を右クリック → 「表示するセクション」メニューが開く。
   1 行目の右端でも開く (今の挙動の維持)。
2. 2 行目以降にある、自前の右クリックメニューを持つ部品 (例: ドライブのボタン、★、
   「フォルダ:」ラベル、セクション見出しのどれか) を右クリック → その部品のメニューが開き、
   「表示するセクション」の背景メニューは開かない。
3. 2 行目以降のボタンの左クリックが今までどおり効く (例: ドライブのボタン / 列数コンボが開く)。
4. ツールバーが 2 行から 1 行に縮む構成変更 (幅を広げる、またはセクションを減らす) を描く pass を
   **1 回だけ**進め、安定化の `harness.run()` を挟まずに**次の入力 pass**で、以前の 2 行目と重なる位置
   (今は絞り込みバー / CentralPanel 側の空白) を右クリックしても背景メニューが開かない (Sol P2-3)。
5. 既存の `grid_edge_buttons_1855` の "background" ケース (1 行目の隙間) が通ること。
6. 背景の AccessKit ノードに無限大・反転の bounds が出ないこと、子のノードが残ること。
7. 見出しの実ドラッグ (並べ替え許可時)、アドレスバー TextEdit の右クリック・選択、タグの右クリック、
   開いた popup 内の操作、modal を開いている間は 2 行目の背景が反応しないこと、背景の長押しで開くこと。
   既存の toolbar snapshot はそのまま通ること (描画は変えない)。

## 6. 文書

- マニュアル `settings.html` の「ツールバーの空き領域を右クリック」は、折り返した行でも効くことが分かる
  書き方にする (版固有の表現は使わない)。
- `toolbar-customization-plan.md` の「空き領域 背景 interact」の記述を本メモの作りへ直す。
- CHANGELOG の v5.7.0 下書きは統括が持つので、修正の一文を完了報告で渡す。
