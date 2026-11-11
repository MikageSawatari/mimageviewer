# 検索欄の入力履歴 (§1.756) 設計

状態: 設計 (2026-11-11、設計担当 ClaudeCode Opus 5.5)。実装は Codex GPT-6.1 Sol / high、レビューは別の Sol / xhigh。
正本の仕様は [next-release-backlog.md §1.756](next-release-backlog.md)。本書はその実装設計。
関連: §1.690 (一覧の項目から検索欄へ条件を渡す「検索要求」の型。同じ入力欄を触る)、保留側 §1.692。

## 1. 利用者が決めたこと (変えない)

- 検索欄ごと (コンテナ検索・現在地の絞り込み・アイテム検索・Everything 検索) に、実行した検索語を
  最近のものから 20 件覚え、選び直せる。
- 環境設定に「検索欄の入力履歴を保存する」(**既定オン**) と「履歴を消去」。1 件ずつの削除は付けない。
- 一覧は入力欄を開いただけでは出さない。▼か ↓ キーで自分で開いたときだけ出す。
- 診断ログ・mIV Remote・AI 連携 (MCP) には渡さない。
- 見た目: 入力欄の右端に▼だけのボタンを置き、枠ごと 1 つの入力欄に見える形。▼で入力欄のすぐ下に一覧。
  **フォルダバーの「履歴▼」も同じ形にそろえる** (リリース済みの見た目の変更。更新履歴で知らせる)。
  部品は検索欄とフォルダバーで共有する。

## 2. 対象の入力欄

| 欄 | 描画箇所 | input id |
| --- | --- | --- |
| 常設の検索欄 ×4 | `ui_pinned_search::draw_pinned_search` | `PinnedSearchKind::input_id()` |
| 現在地の絞り込み (一時バー) | `ui_main.rs` `render_search_bar` | `Local.temporary_id()` |
| コンテナ検索 (一時バー) | `ui_main.rs` (favsearch バー) | `Container.temporary_id()` |
| アイテム検索 (一時バー) | `global_search_ui.rs` | `Metadata.temporary_id()` |
| Everything 検索 (一時バー) | `global_search_ui/everything/ui.rs` | `Everything.temporary_id()` |
| フォルダバーのアドレス欄 | `ui_main.rs` `FolderTailPart::Input` | `"folder_address_input"` |

検索の履歴は常設欄と一時バーで**同じ種類 (kind) の 1 つの一覧を共有する** (同じ検索の入力欄なので)。
タグビューの入力欄 (「一覧▼」) は対象外 (既存の「最近使ったタグ」がある)。

## 3. 共有部品 `src/ui_history_field.rs` (新規)

1 行入力欄の右端に▼を内蔵し、▼ / ↓ で入力欄の下に一覧を出す部品。検索欄・フォルダバーで共有し、
§1.690 でも使えるよう、一覧の中身と「選んだら何をするか」は呼び出し側が決める。

### 3.1 形

- TextEdit は従来どおり **`crate::ime_focus` の helper (`show_singleline` など) で描く**。部品は呼び出し側の
  builder closure (id・幅・margin・hint) をそのまま受け取り、末尾に
  `.suffix(egui::Atom::custom(<▼の atom id>, <▼の大きさ>))` を足すだけにする。
  egui 0.37 の TextEdit は suffix を枠の内側に並べ、フォーカス時の枠色 (selection stroke) も TextEdit 自身が
  描くので、「枠ごと 1 つの入力欄」に見える。外側に偽の枠を描かない。
- 描画後に `output.response.rect(<atom id>)` で▼の位置を取り、その矩形へ枠なしの▼ボタン
  (`ui.put` / `ui.interact` で TextEdit より後に追加 = ヒットテストで手前) を置く。
  ▼を押したら一覧を開閉する。入力欄にフォーカスがあれば保ち (§3.4)、無ければ次の pass で IME helper の
  `focus_request` 引数を通して入力欄へフォーカスを渡す (helper の契約を保つため、helper の外で
  `request_focus` を呼ばない)。
- 入力欄が無効 (disabled) のときは▼も無効。▼だけを無効にする理由 (検索中は使えない等) は
  呼び出し側が tooltip 文字列で渡す。
- 履歴の機能自体を出さないとき (検索: 設定オフ、フォルダ: `show_address_bar_history_menu` オフ) は suffix を
  付けず、従来と同じ TextEdit にする (空白も予約しない)。

### 3.2 一覧 (popup) と、その状態の所有者

- **`egui::Popup` は使わない**。`egui::Popup` は IME 変換中かどうかを見ずに Esc で自分を閉じ
  (`vendor/egui/src/containers/popup.rs` の `should_close`)、開閉の正本が egui 側に移るため、
  §4 の Esc の扱いと二重になる。代わりに部品が自分の `egui::Area` (`Order::Foreground`) を
  **TextEdit 全体の rect の直下**に左揃えで描く。
- 一覧の状態は **viewport ごとに 1 つの値** として ctx の temp data に置く (部品の module だけが読み書きする):

  ```rust
  struct HistoryPopupState {
      owner: egui::Id,          // 一覧を開いた入力欄の id
      highlight: Option<usize>, // ↑↓ の選択位置
      drawn_pass: u64,          // owner の部品を最後に描いた cumulative_pass_nr
  }
  ```

  `None` = 閉じている。開閉・選択位置・持ち主はこの 1 つだけで表し、別の bool や egui の popup 状態と
  二重に持たない。部品の module に `close(ctx)` を 1 つ置き、次の**既存の遷移の境界**で明示的に閉じる
  (同時表示を追跡せず、境界で閉じる):
  - owner の部品が前の pass で描かれていない (バーを閉じた・検索終了・別の表示へ切り替えた)。次に触った時点で閉じる。
  - owner の入力欄が無効 (disabled) で描かれた。
  - 本物のモーダルが入力を取った: `App::note_main_chrome_pointer_block_transition` (`src/app/main_chrome.rs`) が
    `egui::Popup::close_all` を呼ぶ箇所で同じく `close`。モーダルが入力欄の描画前に Esc で閉じる pass でも、
    §4.2 の手順 2 で閉じる (Esc はモーダルのもの)。
  - 別の egui の popup / menu が開いている (`egui::Popup::is_any_open(ctx)`。入力欄の右クリックの編集メニュー
    `ui_helpers::singleline_text_edit_context_menu` を含む。履歴の一覧は egui の Popup ではないので数に入らない)。
    部品の描画時と、§4.2 の `take_escape` の両方で見て閉じる (編集メニューは部品の後に開くので、
    次の pass の Esc を履歴が取らないように)。
  - 持ち主の viewport が OS のフォーカスを失った (`ctx.input(|i| i.focused)` が false)。
- 幅は呼び出し側指定の最小幅 (既定 = 入力欄の幅) 以上。配色は `os_theme` の popup helper
  (`dark_popup_style` 等、coding-conventions の配色の節) を使う。
- 一覧は 1 行のボタンの並び。項目の表示文字列は呼び出し側が渡す (検索 = 検索語、フォルダ = フル
  パスの monospace)。入りきらない文字列は省略し、hover で全文を出す。0 件なら弱い文字で
  「履歴はありません」。
- 高さが画面に入らないときに備えて `ScrollArea` に入れる。**popup が開いている frame では
  wheel を消費する** (coding-conventions「Popup / menu wheel passthrough」、既存例
  `suppress_menu_button_wheel_passthrough`)。
- 閉じる条件: 項目を選んだ / Esc (§4) / ▼をもう一度押した / 入力欄・一覧の外でポインタを押した /
  入力欄を編集した (文字入力・削除・貼り付け) / 入力欄からフォーカスが離れた / 上の境界。
- 一覧の中・▼の操作でフォーカスを失わない方法は §3.4。
- 一覧の中身は開いている pass ごとに呼び出し側から受け取る (snapshot を持たない)。highlight が
  件数を超えたら末尾へ寄せる。

### 3.3 戻り値 (案)

```rust
pub struct HistoryFieldOutput {
    pub text: <ime_focus helper の既存の出力型>, // 呼び出し側は従来どおり response / changed / lost_focus を見る
    pub picked: Option<usize>, // 一覧から選んだ (呼び出し側の entries の index)。通常の確定とは排他
}
```

API の細部 (entries を `&[String]` で渡すか表示用 closure にするか等) は実装担当が既存コードに合わせて
決めてよい。条件は: (1) IME helper を経由する、(2) 呼び出し側は `Picked` に対して自分の確定経路を呼ぶ、
(3) 部品は履歴の保存・記録を一切知らない、(4) `picked` が `Some` の pass では、呼び出し側は
その欄の従来の確定判定 (`lost_focus && enter` など) を**実行しない** (排他の根拠はフォーカスではなくこの契約)。
「入力欄から離れた」通知は部品から出さない (離脱の判定は §5.2 で App が 1 か所で行う)。

### 3.4 一覧・▼の操作で入力欄のフォーカスを手放さない

egui は既定 (`SurrenderFocusOn::Clicks`) で、フォーカス中の TextEdit の外でクリックが成立した pass
(離しの pass) にフォーカスを手放す。押下の pass で戻しても離しの pass を守れず、IME helper の
フォーカス契約も helper の外の `request_focus` では戻らない (`crates/egui-ime` は描画後に、フォーカスを
失った入力欄の記録を消す)。そこで egui 0.37 の TextEdit 補完 (`vendor/egui/src/widgets/text_edit/completion.rs`
の `CompletionPopup::show`) と同じく、**フォーカスを最初から失わせない**:

- 部品は TextEdit (= IME helper の呼び出し) を描く直前に、ポインタが「履歴の一覧の layer の上」または
  「この欄の▼の矩形 (前 pass の値を temp data に置く)」にあれば、`ctx.options_mut` で
  `input_options.surrender_focus_on` を `Never` にし、描画直後に元へ戻す。押下・離し・ドラッグ
  (スクロールバー・空白部分のクリックを含む) のどの pass でも同じ判定なので、一覧の中の操作は
  離しまで内部の操作として扱われ、離脱にも閉鎖にもならない。
- 一覧の layer は部品が描く `Area` の `LayerId` (id は owner から作る) で判定する (`ctx.layer_id_at`)。

## 4. キー操作

入力欄にフォーカスがあり、**IME の変換中でない** (`ime_focus::ime_input_active` が false) ときだけ部品が扱う。
変換中の ↓ / ↑ / Enter / Esc は従来どおり IME に渡す。

| キー | 一覧が閉じているとき | 一覧が開いているとき |
| --- | --- | --- |
| ↓ | 一覧を開き、先頭を選択状態にする (履歴 0 件なら開いて「履歴はありません」) | 選択を 1 つ下へ (末尾で止まる) |
| ↑ | 何もしない (従来どおり) | 選択を 1 つ上へ (先頭で止まる) |
| Enter (NumpadEnter を含む) | 従来どおり (各欄の確定) | 選択中の項目があれば選ぶ (`Picked`)。**選択が無ければ消費せず**一覧を閉じ、従来の確定へそのまま流す |
| Esc | 従来どおり (検索の終了など) | **一覧だけを閉じる** (検索は終了せず、入力欄のフォーカスも残す) |

#### 4.1 egui のフォーカス移動と Enter

- egui は `Memory::begin_pass` で、フォーカス中の widget の event filter に含まれない ↑↓ をフォーカス移動に、
  Esc をフォーカス解除に使う (`vendor/egui/src/memory/mod.rs` の `begin_pass` の Key 処理)。IME helper
  (`crates/egui-ime` の描画後処理) はフォーカス中の入力欄に `horizontal_arrows` / `vertical_arrows` を含む
  filter を毎 pass 設定しているので、**↓ / ↑ はすでに入力欄に残る**。部品は ↑↓ の filter には手を出さない。
  Esc だけは §4.2 のとおり、helper の呼び出しの**後で**部品が filter に足す (同じ pass で後に設定した値が効く)。
  ↑ は一覧が閉じているとき、今の ↑ と同じ結果を保つ (部品は消費しない)。
- ↓ / ↑ / (選択中の) Enter は TextEdit を描く**前に**部品が消費する (TextEdit が Enter でフォーカスを
  手放したり、カーソルを動かしたりしないように)。各欄は Enter を描画前に `bool` で先読みしている
  (`ui_main.rs` の一時バー、`global_search_ui/everything.rs` 等) ので、**排他は §3.3 の契約 (`picked` が
  `Some` の pass では従来の確定判定を実行しない) で保証する**。
- `picked` の確定は、検索欄では既存の `App::submit_search_input` を通す (egui / native の押下をまとめて
  claim する `finish_search_input_submit` を含む。確定後は Enter と同じくフォーカスを手放す)。フォルダ欄も
  同じ `claim_text_input_submit(ctx, <アドレス欄の id>)` で押下を claim してから移動する。同じ backend frame の
  再 pass (egui の discard による同じ入力の再実行) で 2 回選ばれないことを、この claim で保証し、テストで確かめる。
- 消費した ↓ / ↑ / Enter / Esc の押下が native 側の KeySlot から後段のショートカットへ漏れないことを確かめる
  (§1.723 と同型の二重消費を作らない)。

#### 4.2 Esc

- egui は `begin_pass` で、フォーカス中の widget の filter に Esc が無ければフォーカスを外す。部品は、
  **一覧が開いていて入力欄がフォーカスを持つ pass では、IME helper の呼び出しの後に
  `memory.set_focus_lock_filter(id, <helper と同じ filter に escape: true を足したもの>)`** を設定する
  (`set_focus_lock_filter` は前の pass にもフォーカスがあった欄だけを更新する。helper が描画後に
  毎 pass 設定する filter より後に呼ぶので、その pass の値として残る)。次の pass の Esc では egui はフォーカスを外さず、
  Esc の event も残る。一覧を閉じた pass からは設定しない (従来の Esc に戻る)。
- 部品の module に前処理 `take_escape(ctx) -> bool` を 1 つ置き、**メイン viewport の update で
  `EscapeClaim::for_frame` を初期化した直後、`capture_search_escape_input` より前** (app.rs の描画前、
  現在 `capture_search_escape_input` を呼んでいる箇所) に呼ぶ。`EscapeClaim` は 1 回だけ初期化し、
  `take_escape` が true ならそれを claim 済みにする (その後に初期化し直さない)。処理は次の順:
  1. この pass の入力に**修飾キーなしの Esc の押下**が無い、または IME 変換中なら何もしない。
  2. `HistoryPopupState` が無ければ何もしない。前の pass で owner が描かれていない (退役) なら状態を消して何もしない。
  3. 別の egui の popup / menu が開いている (`egui::Popup::is_any_open`)、または共通モーダル・ダイアログが
     開いている (`EscapeClaim::for_frame` が claim 済みになる理由と同じ判定) なら、状態を消して一覧を閉じるが、
     Esc は消費しない (前面のメニュー・モーダルのもの)。
  4. それ以外: 状態を消して一覧を閉じ、Esc を egui から消費し、同じ viewport の native の Esc 押下を claim する。
     true を返し、`EscapeClaim` を claim 済みにする。検索の Esc 振り分け (`route_search_escape`) と
     フォーカス回復 (`recovering_text_input`) による Esc の扱いに渡さない。入力欄のフォーカスは、上の filter で
     egui が外していないので、そのまま残る (部品は `request_focus` を呼ばない)。
- 割り切り: 一覧を開いた pass と次の pass の間に Esc が来た場合 (filter がまだ効いていない。たとえば
  フォーカスの無い欄の▼で開いた直後の 1〜2 pass) は、egui がフォーカスを外すことがある。その場合も
  手順 4 で一覧は閉じ、検索は終了しない。フォーカスが外れるだけなので、回復処理は足さない。
- claim は「検索を終了させない」ためのもので、一覧を閉じるのは `take_escape` 自身が行う。
- `take_escape` が true を返した pass は、§5.2 の離脱の判定 (`settle_search_input_departure`) から除外する
  (戻り値を同じ pass の update の中で settle へ渡すだけで、状態は持たない)。filter が効く前の Esc で
  フォーカスが外れても、一覧を閉じただけの操作として記録しない。
- 既存の挙動 (今回は変えない): 入力欄の右クリックの編集メニュー (egui の Popup) は Esc を見て閉じるが消費しないので、
  今の版でも、編集メニューを開いた検索欄で Esc を押すと、メニューが閉じたうえで検索の Esc 振り分けも動く。
  履歴の一覧はこの場合に Esc を取らない (手順 3) ので、この挙動を変えも悪化させもしない。直すなら別の項目にする。
- 確かめる操作: 一覧が閉じた状態での Esc (従来どおり)、▼で開いてから Esc、↓で開いてから Esc、
  一覧を閉じた直後にもう一度 Esc (従来の Esc の動作になる)、履歴を開いたまま入力欄を右クリック→Esc
  (履歴の一覧は閉じ、その Esc は従来どおり編集メニューと検索に渡る)、アドレス欄での同じ操作。
  フォーカスの無い欄の▼で開き、フォーカスが入った直後の Esc で、一覧が閉じ、検索は続き、記録されないこと。
- これらは入力欄の中だけで効く固定の部品操作 (ComboBox と同じ扱い) なので `KeyAction` にはしない。
  固定にする理由を [keymap-spec.md](keymap-spec.md) の固定入力の節に書く。

## 5. 履歴の記録と選び直し

### 5.1 データ

`Settings` に 2 項目を足す (新機能なので移行は不要。`serde(default)`):

- `save_search_input_history: bool` (既定 `true`)
- `search_input_history: SearchInputHistory` — `PinnedSearchKind` ごとの `Vec<String>` (新しい順、最大 20 件)

規則 (純関数 + 単体テストで固定):

- 前後の空白を除いた文字列を記録する。空なら記録しない。1,000 文字を超える語は記録しない。
- 同じ語 (除いた後で完全一致、大文字小文字を区別) は先頭へ移す。21 件目以降は捨てる。
- 読み込み時も同じ規則で正規化する (壊れた値・重複・件数超過を落とす)。

保存は `settings.db` の他の項目と同じ経路 (メモリ上の `Settings` を通常の保存時機で書く) に乗せ、
新しい I/O・ワーカー・ファイルを作らない (フォルダの履歴 `quick_folder_recent_folders` と同じ)。
したがって保存先は通常版 `%APPDATA%\mimageviewer\settings.db`、ポータブル版は `data\settings.db` で、
data_dir の解決に従う。

- 環境設定の OK による全体差し替えで巻き戻さないよう、`settings.rs` の live 値を引き継ぐ一覧
  (`quick_folder_recent_folders` を引き継いでいる関数) に `search_input_history` を足す。
  `save_search_input_history` は環境設定で編集するので draft の値を使う。
- `settings_transfer`: `search_input_history` は除外 (`"利用データ・登録先・履歴・検索対象"`)。
  `save_search_input_history` は移行できる設定として扱う (既存の bool 設定と同じ書き方)。
- 設定の世代バックアップ (`settings.db.bak1`〜`bak10`、`src/db_backup.rs`) には、消去前の履歴が残る。
  フォルダの履歴と同じ扱いとし、消去の対象にしない (世代バックアップは設定の復旧手段なので、履歴のためだけに
  書き換えない)。その代わり**「消去」の説明では、現在の入力履歴を消すこと・設定の自動バックアップには
  しばらく残ることを書く** (環境設定の説明・マニュアル・privacy の日英)。

### 5.2 記録する時点 (2 つだけ)

1. **確定したとき**: `App::submit_search_input(ctx, kind, widget)` が受け付けた時点 (利用可否の検査を
   通った後) に、その kind の下書きを記録する。Enter・「検索」ボタン・履歴からの選び直しは全部ここを通る
   (常設欄・一時バー・`route_pending_search_submit` の予約経路とも)。結果が 0 件・失敗でも記録する
   (仕様: 見つからなかった語も入る)。
2. **入力欄から通常に離れたとき、その語で検索を実行済みなら**: 入力するだけで検索するコンテナ検索と
   アイテム検索で、「入力して結果をクリックした」語を残すため。各欄の `lost_focus()` では判定しない
   (描画の途中では、描画後に処理される Esc による終了や常設欄の × と区別できない。また egui の
   `lost_focus()` は 2 pass にわたって true になり得て、描画順によって古い離脱が新しいものを上書きする)。
   代わりに、**前の pass のフォーカス所有者と今のフォーカスを、pass の中の 1 か所で比べる**:
   - 既存の `App::record_search_input_focus` は、フォーカス中の検索入力欄 (`SearchInputId`。常設・一時バー) を
     viewport の temp data `search_input_owner` に記録している。ただし update が途中で return する pass
     (起動時の overlay、native 動画、埋め込みのフルスクリーン等、app.rs の `record_search_input_focus` より前の
     return) では更新されず、古い値が残る。そこで**記録を `(SearchInputId, 記録した cumulative_pass_nr)` の
     一体の値に変え**、`capture_search_escape_input` もこの値の `SearchInputId` を使うように合わせる。
   - 新しい `App::settle_search_input_departure(ctx)` を、メイン viewport の update で
     **`route_search_escape` の直後、`record_search_input_focus` の直前** (app.rs の現在 101051〜101052 行の間。
     `render_grid` より前) に呼ぶ。ここでは記録 (`search_input_owner` の古い値) を読み、それが
     **直前の pass (`記録した pass + 1 == 今の pass`) の記録**で、この pass で `take_escape` が Esc を取っておらず、`Some(input)` で、今フォーカスがその入力欄に
     無ければ、`input.kind()` の離脱として次をすべて満たすときだけ記録する (途中 return を挟んだ古い記録は
     使わず、過去の離脱を後から補わない):
     - その kind の検索がまだ続いている (`edit_owner(kind)`。この pass の Esc・× で終了・解除していれば false)
     - 下書き (`search_draft(kind)`) が空でない
     - その下書きで**検索を実行済み** (結果の到着は問わない)。kind ごとの「最後に実行した要求の検索語」:
       - 現在地: `App::submitted_local_search()` (実行中の要求 → 適用済みの順。検索対象・OR の変更から
         `submit_search_input` を通らず直接実行される要求も含む) の検索語
       - コンテナ: `favsearch.last_executed`
       - アイテム: `global_search.source == Metadata` のときの `global_search.last_executed`
       - Everything: 実行中の要求、無ければ受け付けた結果 (`SearchAccepted::Everything`) の送った検索語
       実装担当が各 owner の既存の値から取り出す (新しい状態は足さない)。
   - 部品自身の操作 (▼・一覧内の操作・Esc で一覧を閉じた) では、入力欄はフォーカスを失わない (§3.4・§4.2) ので
     離脱にならない。`picked` の確定は 1 で記録済みで、ここで再び記録されても先頭へ移すだけ。
   - 判定は pass ごとに 1 回で、候補を持ち越さない。ここは `render_grid` より前なので、グリッドの操作
     (別ウィンドウの準備・閲覧の開始) で検索の状態が変わる前に判定できる。結果のクリックで入力欄が
     フォーカスを手放すのはツールバーの描画中 (グリッドより前) なので、同じ pass の判定で捕まる。
   入力途中の語 (デバウンス待ち) は、フォーカスが入力欄にある間は判定しないので残らない。
   結果の取得中 (Running) に結果をクリックしても、実行済みの語なので記録される。
   現在地の絞り込みと Everything でも、実行済みの語のまま離れれば記録される (多くは 1 の重複で、先頭へ移すだけ)。

記録しないもの: mIV Remote・AI 連携 (MCP)・タグビュー・フォルダバー (フォルダは既存のフォルダ履歴)。
Remote と MCP は `submit_search_input` と上の入力欄を通らない。実装時に `submit_search_input` と
`settle_search_input_departure` の呼び出し元を列挙し、UI の入力欄以外から呼ばれていないことを確かめる。
test-script は通常の入力 (Enter・クリック) を配送するので、通常の操作と同じく記録される (遮断しない。
試験は隔離したデータで行う)。

`save_search_input_history == false` のときは記録しない。

### 5.3 選び直し

一覧で選んだら、その kind の下書きをその語に置き換え、`submit_search_input(ctx, kind, <その欄の id>)` を
呼ぶ (Enter と同じ確定。記録で先頭へ移る)。確定後のフォーカスは Enter と同じ状態にする
(入力欄はフォーカスを手放す)。利用できない状態 (`pinned_search_availability(kind)` が Err) では▼も無効。

検索の対象・OR・種類などの絞り込み条件は保存も復元もしない (検索語だけ)。

### 5.4 環境設定

「履歴と復元」ページの末尾に節「検索欄の入力履歴」を足す (閲覧履歴と同じ並び):

- チェック「検索欄の入力履歴を保存する」(既定オン)。説明 (弱い文字):
  「コンテナ検索・現在地の絞り込み・アイテム検索・Everything 検索で実行した検索語を、欄ごとに最近の
  20 件まで PC 内に保存し、入力欄の▼か ↓ キーで選び直せます。オフにすると、今の入力履歴も消去します
  (設定の自動バックアップには、しばらく残ります)。」
- 「入力履歴: N 件を記憶。」と、N > 0 のとき「検索欄の入力履歴をすべて消去」ボタン。押したら App 側で
  すぐ消去し (閲覧履歴の消去と同じ one-shot 方式)、結果の一言を出す。
- **オフで OK したら、保存済みの入力履歴も消去する**。「保存しない」と選んだ人の PC に見えない履歴を
  残さないため、オフの状態を「記録しない・▼を出さない・中身が空」の 1 つにする (§7)。
- 環境設定の検索索引 (`preferences/search_index.rs`) に 2 項目を登録する (「検索」「履歴」「入力履歴」
  「history」などで引ける)。

## 6. フォルダバー

- `show_address_bar_history_menu` がオンのとき、アドレス欄に共有部品で▼を内蔵する。右側の
  「履歴▼」ボタンは無くす。フォルダバーの右側の幅計算 (`ui_main.rs` の right controls の一覧の
  `"履歴▼"`) からも外し、入力欄の最小幅に▼の幅を含める。
- 一覧の中身は今の「履歴▼」と同じ: 現在のフォルダを除いた `recent_folders` を最大 20 件、フルパスの
  monospace、幅は今と同じ `(content_rect.width * 0.72).clamp(560, 1100)` (入力欄より狭くはしない)。
  選んだら今と同じ `AddressBarNav::Direct(resolve_folder_bar_nav_path(..), ExplicitList)`
  (アドレス欄の文字は書き換えない)。0 件なら「履歴はありません」。
- 無効になる条件と tooltip も今と同じ (検索中・★固定中)。★固定中はアドレス欄自体が無効なので▼も無効。
- フォルダバーの右クリックメニューの表示切り替え・「最近開いたフォルダ履歴をクリア」はそのまま。
  切り替えの項目名に「履歴▼」と書いているなら、新しい見た目に合う名前 (例:「履歴 (入力欄の▼)」) にする。
- **フォルダのアドレス欄でも ↓ で一覧が開く**ようになる (部品の共有による新しい操作。今の ↓ は
  1 行入力では実質何もしない)。

## 7. 状態の組み合わせを減らす検討 (CLAUDE.md の設計原則)

- **一覧の状態は viewport ごとに 1 つの値** (`HistoryPopupState`: 持ち主・選択位置・最後に描いた pass)。
  egui の popup 状態と二重に持たない。編集・離脱・検索終了・モーダル表示 (入力欄が無効で描かれる)・
  持ち主が描かれなくなったことで閉じるので、「一覧を開いたまま検索が変わる」「開いたまま別の欄・別の
  viewport に移る」「開いたまま検索ソースが切り替わる」組み合わせを扱わない (持ち主の id が変われば
  退役で閉じる)。常設欄と一時バーが同時に描かれても、持ち主は 1 つの入力欄 id なので混ざらない。
- 一覧の中身は開いている pass ごとに Settings から引く (snapshot を持たない)。開いている間に
  中身が変わるのは記録 (§5.2) だけで、記録は確定・離脱のときに起き、どちらでも一覧は閉じる。
- **記録の時点を 2 つに絞る** (確定と、実行済みの語での通常の離脱)。検索の結果到着・デバウンス・ソース切り替え
  などの非同期の完了では記録しない。離脱の判定は pass の中の 1 か所 (`route_search_escape` の直後) だけで、前の pass のフォーカス所有者との比較で
  行い、候補を持ち越さない。
- **オフ = 空** にして「オフだが中身がある」状態を作らない。
- 環境設定を開いている間は検索欄が使えない (`pinned_search_availability` が Err) ので、消去と記録が
  競合しない。ダイアログが開いた瞬間のフォーカス喪失で記録されても、live 値を引き継ぐので消えない。
- まれな失敗は割り切り: 保存は既存の設定保存に任せ、強制終了などで直前の記録が失われても回復処理は
  足さない (ログと通知も既存の設定保存のまま)。

## 8. 診断ログ・Remote・MCP

- 履歴の中身 (検索語) をログ・perf event・診断 ZIP に出さない。新しい perf event を足すなら件数だけ。
  既存の text input のキー診断は文字キーを伏せる (`ime_focus.rs` の非文字 allowlist) ので変えない。
- Remote の一覧系は `settings_kv` から決まったキーだけを読む (`REMOTE_LISTING_SETTINGS_SQL` 等)。新しいキーを
  その一覧に足さない。ただし Remote のコレクション経路 (`remote_ipc/collections.rs`・
  `persistent_collections.rs`) と agent_ipc には `Settings` 全体を読む経路がある。読むこと自体は問題ないが、
  **応答・ログに `search_input_history` が出ないこと**を、実装時に `Settings` を読む経路の出力境界を
  監査して確かめ、§10 の検査で固定する。

## 9. 公開文書・設計文書

- `htdocs/mimageviewer/manual/search.html`: 「検索欄の常設」付近に入力履歴の説明 (記録する時点・▼と ↓・
  欄ごと 20 件・オフと消去の場所・Remote/AI 連携に渡さない)。Everything 節の「検索語と結果一覧は
  mImageViewer の履歴や診断ログに保存しません」を「結果一覧は保存しません。検索語は入力履歴が有効なら
  PC 内の入力履歴に保存し、診断ログには保存しません」の趣旨に書き直す。
- `htdocs/mimageviewer/manual/` のフォルダバーの説明 (「履歴▼」の位置・見た目) と設定リファレンス
  (「履歴と復元」の新項目)、キーボードショートカット (検索欄・アドレス欄の ↓)。
- `htdocs/mimageviewer/privacy.html`: 日本語の Everything の段落を同じ趣旨で書き直し、
  「端末内に保存されるデータ」に「検索欄の入力履歴 (欄ごとに最近 20 件、設定と同じ settings.db。
  環境設定の『履歴と復元』でオフ・消去できる。消去しても設定の自動バックアップにはしばらく残る。
  リモート閲覧と AI 連携には渡さない)」を足す。**英語側には今 Everything の段落が無い**ので、入力履歴の
  説明を新しく足す。最終更新日 (日英)。
- `htdocs/mimageviewer/index.html`: trust-note の「Everything 検索の検索語と結果一覧は保存しません」を
  「Everything 検索の結果一覧は保存しません。検索欄の入力履歴は PC 内に保存し、環境設定でオフにできます」
  の趣旨に。カードの字数規則 (`scripts/check_site_cards.py`) を確かめる。
- 設計文書: `docs/keymap-spec.md` (固定キー)、`docs/README.md` (本書を索引へ)、`docs/ui-responsiveness.md`
  に影響が無いことの確認 (新しい I/O なし)。「検索語を保存しない / 新しい設定なし」と書いている
  `docs/spec.md` (111 行付近)・`docs/architecture-overview.md` (128 行付近)・`docs/everything-search-plan.md`
  (412 行付近) を、§1.756 で入力履歴を保存するようになったことに合わせて直す。
- 更新履歴と「重要な変更点」(`src/version_highlights.rs` の must_read) は**リリース作業 (Phase 0) で書く**
  (release-checklist の手順。版番号もそこで決まる)。文面の案をバックログ §1.756 に残しておく。

## 10. テスト

- 単体: `SearchInputHistory` の記録規則 (空白・空・長さ・重複・上限・kind 独立・読み込み正規化)。
- 状態遷移 (App テスト、常設欄と一時バーの両方で):
  - 4 種類とも、確定 (Enter・「検索」・予約経路) で記録されること。
  - コンテナ検索・アイテム検索で、実行済みの語のまま入力欄から離れる (結果のクリック) と記録されること。
    アイテム検索の取得中 (Running) に結果をクリックしても記録されること。
  - 入力途中 (フォーカスあり) の語、まだ実行していない語 (デバウンス待ち) は記録されないこと。
  - Esc で検索を終了した pass、× で解除した pass では記録されないこと。
  - 現在地の絞り込みで検索対象・OR を変えて実行中のまま離れると、新しい要求の語で記録されること。
  - 正常な描画 → 途中 return の pass → 正常な描画、と続いたとき、遅れて離脱を記録しないこと。
  - 検索欄 A → B → 結果一覧と 2 pass で移ったとき、A と B がそれぞれ正しく判定されること (常設欄の並び順を
    入れ替えても同じ)。
  - 設定オフでは記録されないこと。オフで OK すると消えること。消去ボタン。環境設定の OK で live の履歴が
    巻き戻らないこと。
  - 永続化: settings.db への保存と再読込で履歴が戻ること、キーの無い旧設定を読むと既定オン・空になること、
    消去後に保存・再読込しても空のままであること。
- 部品 (handler-level、egui_kittest): ↓ で開く・↑↓・選択中の Enter / NumpadEnter で `Picked` (従来の確定は
  起きない)・選択なしの Enter は従来の確定・同じ入力の再 pass で 2 回選ばれない・Esc で一覧だけ閉じて
  入力欄のフォーカスと検索は続く・IME 変換中の ↓ / Enter / Esc では一覧を触らない (開いていても閉じない)・
  編集で閉じる・▼クリックで開閉し入力欄のフォーカスが保たれる (フォーカスの無い欄では次の pass で入る)・
  一覧の項目・空白・スクロールバーを、押下と離しを別の pass にして操作しても離脱にならず、項目なら選べる・
  外側のクリックで閉じる・無効時は開かない・持ち主が描かれなくなったら閉じる (バーを閉じる・
  モーダルで無効化・検索ソースの切り替え・別 viewport)・一覧が開いている frame で wheel が背面に通らない・
  消費したキーが native 側からショートカットに漏れない。
- 非公開: 識別できる語 (例 `miv-history-sentinel-756`) を履歴に入れた状態で、設定の書き出し
  (`settings_transfer` の既存の非公開値検査を拡張)、Remote のコレクション応答・agent_ipc の代表的な応答に
  その語が含まれないこと。ログ・perf event に出さないことはコードの監査で確かめる。
- フォルダバー: ▼で開き、選ぶと移動し、現在のフォルダは出ない。検索中・★固定中は無効。
  右側の幅計算からボタンが消えても既存の折り返しテスト (`ui_main.rs` の `"履歴▼"` を使うテスト) が通るよう
  更新する。▼を除いた入力部分が従来の最小幅を保つこと。
- settings_transfer の分類テスト (全フィールドの分類) が通ること。
- UI スナップショット: 常設欄 (▼入り)、一覧を開いた状態、フォルダバー。`UPDATE_SNAPSHOTS=1` で更新し、
  PNG を目視確認する ([ui-snapshot-policy.md](ui-snapshot-policy.md))。`python scripts/check_ui_glyphs.py`。

## 11. 実機で確かめること (サブ PC / 利用者)

- 4 種類の常設欄と一時バーで、▼が入力欄の枠の中に見え、フォーカス時に枠ごと強調されること。
- Ctrl+G で入力して結果をクリックしたあと、▼にその語が出ること。入力途中の語が出ないこと。
- ↓ / ↑ / Enter / Esc、日本語の変換中の ↓ / Enter / Esc。
- フォルダバーの▼の見た目・一覧の幅・選んだときの移動。小さいウィンドウ・UI 倍率での折り返し。
- 環境設定の「履歴と復元」でオフ・消去。
