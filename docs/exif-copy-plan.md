# §1.889 閲覧中の画像の EXIF をまとめてクリップボードへコピーする

状態: 設計 (2026-11-25、設計担当 ClaudeCode Opus)。実装・実機確認はこれから。
要件の正本は [バックログ §1.889](next-release-backlog.md)。

## 1. 範囲

- 画像フルスクリーンの情報パネル「EXIF」見出しの行に「Copy」ボタンを置く。
- 同じ処理を新しい `KeyAction::FsCopyExif` (文脈 `FsImage`、`Press`、既定キーなし) から呼べるようにする。
- 一覧 (Grid)・リング・マウスジェスチャ・ゲームパッド・右クリックメニュー・動画/音声フルスクリーンには今回足さない
  (バックログの必須範囲外)。

## 2. 出力の内容と書式

- 対象は、情報パネルが表示に使う `exif_cache` の `ExifInfo`。新しい読み取り・パーサーは作らない。
- **非表示タグの所有者を「使う時点の 1 つの helper」に移す** (設計レビュー r1 の P1)。
  - 今は worker が読み取り時に、開始時点の `exif_hidden_tags` で除外し (`app.rs` の `start_metadata_load`)、設定変更時は
    今マウントしている context の `exif_cache` だけを `clear()` している (`preferences.rs`)。そのため
    (a) 読み込み中に GPS を非表示にすると、旧設定の結果がそのまま cache に入る、(b) 別の窓 (退避中の context) の cache は
    clear されない、の 2 つの順序で、隠したタグが残る。これはパネルの表示にも既にある穴で、コピーでは隠したつもりの
    情報が外へ出るので放置できない。どちらも通常の操作の順序で起きる。
  - 直し方: worker は非表示で絞らずに読む (`read_exif*` に `&[]`、`run_metadata_load` の `hidden` 引数と
    `start_metadata_load` の設定の複製を外す)。`exif_cache` は絞っていない値を持つ。
  - 純関数 `ExifInfo::visible(&self, hidden: &[String]) -> Option<ExifInfo>` (`exif_reader`) を足す。非表示のタグを除き、
    空になった節を除き、何も残らなければ `None`。生の GPS 座標 (`gps`) は持ったままでよい (既存の `gps_coordinates()` が
    節に緯度・経度が残っているときだけ使う)。
  - パネル (`get_exif_for_idx`) とコピーは、どちらもこの helper を **その時点の `settings.exif_hidden_tags`** で通した値を使う。
    これで設定を変えた直後・読み込み中に変えた場合・別の窓・旧結果の到着のどれでも、表示とコピーは最新の設定どおりになる。
    世代番号や再読み込みの仕組みは足さない。
  - 設定変更時の `exif_cache.clear()` (`preferences.rs`、非表示設定のためだけにある) は不要になるので外す。
  - 費用: パネル描画のたびに数十〜百程度のタグ × 非表示一覧の比較が増えるだけ (現状も `get_exif_for_idx` が毎フレーム clone している)。
  - `exif_cache` は他から読まれていない (MCP の `item_info`・検索の索引は自分で `&[]` で読む) ことを実装担当が確かめる。
    永続データではないので移行は要らない。
- 折りたたみ (`exif_sections_open`) は見た目の状態なので出力に影響しない。閉じている節も出す。
- 「Google マップで開く」のボタンや URL、AI 生成情報・XMP・外部メタデータ・ファイル名は出力に入れない。
- 書式 (純関数 `exif_reader` か `ui_metadata_panel` に置く `exif_clipboard_text(&ExifInfo) -> Option<String>`):

  ```text
  [カメラ]
  メーカー: Canon
  モデル: Canon EOS R5

  [撮影設定]
  露出時間: 1/200 秒
  ```

  - 節の見出しは `TagGroup::display_name()` を `[ ]` で囲む。節の順は `sections` の順 (パネルと同じ)。
  - 各行は `tag_display_name(tag) + ": " + 値`。パネルの表示名と同じ。
  - 節の間は空行 1 つ。改行は `\n` (既存の複数パスのコピーと同じ)。値の中の `\r\n` / `\r` は `\n` に揃える。
    末尾に改行を付けない。
  - 項目が 1 つもない (全節が空) なら `None`。
- 見出しの記号は `[` `]` だけにし、環境依存のグリフを使わない。

## 3. 対象の決め方 (ボタンとキーで同じ)

- 対象は「情報パネルの情報タブが表示する対象」と同じ `info_idx` とする。
  既存の `resolve_metadata_info_target` と `MetadataInfoTarget::idx` が唯一の所有者で、
  これを「対象の idx を返す」1 つの helper に切り出し、パネルの描画とキー操作の両方がそれを呼ぶ。
  - 単ページ: `fullscreen_idx`。
  - 見開き: パネルで選んでいる側。未選択・組が変わった後は読む順の先頭 (既存の規則)。
    パネルを閉じていても、同じ組なら選んだ側が保たれる (既存の規則)。両ページを結合しない。
  - 一覧のカーソル (`selected`) とチェック集合は使わない。
- ボタンは、その描画 pass で描いている `exif` (同じ `info_idx` から取ったもの) をそのまま渡す。
  キーは helper で同じ idx を解いて `exif_cache` から取る。両方が同じ `App` の関数
  (例 `copy_exif_info_to_clipboard(ctx, &ExifInfo)`) を通り、同じ文字列・同じ通知になる。

## 4. 読み込み前・EXIF なし・失敗

組み合わせを減らすため、**読み込みが終わるのを待ってからコピーする予約は作らない**。

| 状態 (対象 idx の `metadata_cache_key`) | 動き | クリップボード |
| --- | --- | --- |
| `exif_cache` に `Some(Some(info))` で、出力が `Some` | コピーして通知「EXIF をコピーしました」 | 書き換える |
| `exif_cache` に `Some(None)`、または出力が `None` (全部非表示など) | 通知「この画像にはコピーできる EXIF がありません」 | 触らない |
| `exif_cache` に無い (読み込み中・未要求) | `ensure_metadata_load(idx)` を呼び (同じ key の読み込み中なら既存どおり再起動しない)、通知「EXIF を読み込んでいます。少し待ってからもう一度実行してください」 | 触らない |
| 対象 idx がない・key が作れない (動画・フォルダ等) | 何もしない (キーは下の受付条件で既に止まる) | 触らない |

- 予約を持たないので、ページ・窓を切り替えた後に古い結果でクリップボードを上書きすることは起きない。
- PDF ページ・ZIP 内画像・相対ページの検証失敗は、既存の `run_metadata_load` の結果どおり (ZIP 内画像は EXIF を読み、
  PDF ページと検証失敗は EXIF なし) に従い、新しい分岐を作らない。
- 通知はビューア面 (`ActionSurface::Viewer`) の既存の通知 helper を使う。文言は上の 3 つ。

## 5. キー操作の受付

- `FsCopyExif`: `ini_name` = `FsCopyExif`、表示名「EXIF をテキストでコピー」、`context()` = `FsImage`、
  `trigger()` = `Press`、`default_chords()` = 空、`ALL_ACTIONS` に `FsCopyImage` の近くで追加。
  `docs/keymap.ini.default` に `# FsCopyExif = none ; EXIF をテキストでコピー` を足す。
- 受付は既存のコピー 5 種と同じ位置・同じ条件 (`ui_fullscreen.rs` の `handle_copy_action_keys(ctx, true)` の呼び出しと同じ
  `!current_item_is_video && !fs_music_view_active && fs_context_menu_idx.is_none()`) にし、`consume_action_no_repeat` で消費する。
  `handle_copy_action_keys` は右クリックメニューのコマンドへ流す関数なので、そこへは混ぜず、隣に別の 1 行を置く
  (メニュー項目を持たない操作のため)。
- 文字入力中・IME・モーダル・ページ一覧などの既存の入力所有の判定は、その位置より前の既存の gate をそのまま通る。
  新しい gate は作らない。実装担当は、TextEdit にフォーカスがあるとき `FsCopyExif` に `Ctrl+C` を割り当てても
  消費されず、TextEdit の通常のコピーが働くことをテストで確かめる。既存 gate がそれを満たさないと分かったら、
  実装を止めて設計担当へ返す。
- **OS の Copy イベントとの関係** (設計レビュー r1 の P2): Windows では Ctrl+C が `egui::Event::Copy` にもなる。
  keymap が消費するのは `Event::Key` と native の key edge だけなので、`FsCopyExif` に Ctrl+C を割り当てると、
  情報パネルの選択できるラベル (値の文字列) に選択範囲があるとき、同じ pass の終わりにラベルの選択文字列の
  `CopyText` も積まれ、EXIF 全文を上書きし得る。
  - 優先順位: **TextEdit にフォーカスがある (文字入力中) なら、既存の gate で `FsCopyExif` は受け付けず、TextEdit の
    通常のコピーが働く。それ以外で `FsCopyExif` を受け付けた押下では、操作が勝つ** (利用者がそのキーを割り当てたため)。
  - 実装: `FsCopyExif` を受け付けた pass では、その viewport の入力から `Event::Copy` を取り除く
    (`ctx.input_mut(|i| i.events.retain(..))`)。ラベル選択のコピーがその pass の終わりにイベントを読む前に取り除かれることを、
    実装担当が egui のコードと pass の順序 (情報パネルの描画とキー処理の前後) で確かめる。順序の都合で取り除けない場合は、
    実装を止めて設計担当へ返す。
  - `FsCopyExif` が Ctrl+C 以外 (例 Ctrl+Shift+E) なら `Event::Copy` は出ないので、取り除くものはない。
  - テストは、`Event::Copy` と key の押下を同じ pass に入れ、ラベル選択がある状態で、pass 全体の最終出力の `CopyText` が
    EXIF 全文の 1 つだけであることを確かめる。
- キーの変更・解除・競合は既存の操作カスタマイズの仕組み (競合検出・保存・`parse_ini_name`) に乗るだけで、専用処理は足さない。

## 6. 複数ウィンドウ (メイン / F12 / 独立窓)

- フルスクリーンの key 経路と情報パネルは、既存の viewer context 交換で各窓の状態 (`fullscreen_idx`・`similar_panel.info_target`・
  `exif_cache`・`metadata_pending`) を読む。本機能はその窓の状態を読んで `ctx.copy_text` を呼ぶだけで、
  既存の `FsCopyPath` / `FsCopyFileName` と同じ形。
- detached の述語・viewport の routing・生成・identity・geometry には触れない。したがって detached リワークの
  「構造的修正の合意」の対象外とし、[detached 計画 §11](detached-rework-plan.md#11-リワーク外からの変更記録) への記録も不要と判断する
  (実装中に触れる必要が出たら止めて設計担当へ返す)。
- 兄弟の窓の状態 (選択・cache・pending) を変えない。`ensure_metadata_load` は既存どおり今の context の pending だけを扱う。

## 7. UI スレッド

- ファイル読み取り・decode・DB・GPU upload を足さない。読み込みは既存の `start_metadata_load` の worker だけ。
- 文字列の組み立ては数十〜数百行の連結で、押したときだけ。クリップボードへの書き込みは既存のコピー操作と同じ
  `ctx.copy_text` (eframe がフレームの終わりに書く既存経路)。`ui-responsiveness.md` §4 の新しい同期 I/O には当たらない。

## 8. 単純化の検討

- **読み込み完了を待つ予約は採らない。** 予約を持つと、待っている間のページ送り・窓の切り替え・パネルの左右変更・
  設定変更と予約の組み合わせを全部扱う必要がある。フルスクリーンを開いた時点で既存の読み込みが走るので、
  実際に押すころには読み込み済みのことが多く、まれに読み込み中なら通知してもう一度押してもらう方が単純。
- **対象を新しく決めない。** 情報パネルの対象の所有者 (`MetadataInfoTarget`) を使い回し、左右の別状態を足さない。
- **ボタン専用・キー専用の経路を分けない。** 書式の純関数と 1 つの App 関数を両方が通る。
- **非表示は使う時点の 1 つの helper が持つ。** 読み取り時とコピー時の 2 か所で絞ると所有者が 2 つになり、
  読み取り時だけだと設定変更・別窓・読み込み中の順序で古い絞り方が残る (r1 P1)。読み取り時の絞り込みをやめ、
  パネルとコピーが同じ helper を最新の設定で通す形にすると、cache の作り直し・世代番号が要らない。
- まれな失敗 (クリップボードの書き込み失敗) は既存の `ctx.copy_text` の扱いのまま割り切る (既定の割り切り方針)。

## 9. テスト (実装担当が足す)

- 書式の純関数: 節の順・表示名・空行・改行の正規化・空なら `None`・GPS 節を含む/含まない入力。
- 非表示: `read_exif_from_bytes` に非表示タグを渡した結果を整形し、隠したタグ (GPS を含む) が出ないこと。
- 折りたたみ: `exif_sections_open` で節を閉じてもボタン/キーの出力が変わらないこと。
- 同じ文字列: 同じ fixture で、パネルのボタンを押したときとキー (`FsCopyExif` を割り当て) のときのクリップボード出力
  (egui の `OutputCommand::CopyText`) が一致すること。
- 対象: 見開きで左右を選んだとき、キーが選んだ側の EXIF をコピーすること。両ページを結合しないこと。一覧の
  `selected` / `checked` が別項目を指していても使わないこと。
- 状態: cache なし (読み込み中) → コピーしない・通知・同じ key の pending を再起動しない。EXIF なし → コピーしない・通知。
  どちらもクリップボード出力が無いこと。
- 受付: 動画・音楽ビュー・右クリックメニュー表示中は消費しない。TextEdit にフォーカスがあるときは消費しない。
- keymap: `ini_name` の往復、`ALL_ACTIONS` に含まれること、既定キーなし、既存の網羅テスト。
- 非表示の順序 (r1 P1): (a) 読み込み中に GPS を非表示へ変えてから結果が届いても、パネル・コピーとも GPS が出ないこと。
  (b) 別 context に読み込み済みの cache を残したまま設定を変え、その context へ切り替えても GPS が出ないこと。
  (c) 非表示を外すと、読み直さずに出ること。
- 複数 context (r1 P3): 同じ idx に、context ごとに違う EXIF・違う選択側・違う pending を持たせ、操作した窓の内容だけが
  出力されること (既存の multi-context fixture、`ui_metadata_panel.rs` の `metadata_info_target_*` の周辺を使う)。
  別 context の対象・cache・pending を変えないこと。
- 選択の保持と既定 (r1 P3): パネルを閉じたまま同じ組なら、キーも選んだ側をコピーすること。隠れたままページ送り・
  モード往復をした後は、キーも読む順の先頭をコピーすること。どちらも新しい共有 helper 経由で確かめる。
- UI スナップショット: EXIF 見出しにボタンが増えるので、該当する既存スナップショット (`exif_gps_maps_dark` など) を
  更新し目視する。

## 10. 文書

- `docs/keymap-spec.md` のコピー操作の節・`docs/key-customization-impl-plan.md` の FsImage の一覧に `FsCopyExif` を足す。
- マニュアル: `fullscreen.html` の EXIF 情報の節 (ボタンとキー、非表示の扱い、見開きの対象、読み込み中の通知)、
  ショートカット/操作カスタマイズの一覧 (既定なしの操作として)。製品ページは機能カードに載せるほどではないので、
  該当箇所があるかだけ確かめる。
- MCP の操作一覧 (§1.843 第 2 段、ブランチ `mcp-keys-843`) は `KeyAction::all()` (= `ALL_ACTIONS`) を
  `is_user_facing()` と画面の scope (`FsImage` を含む `image_fullscreen`) で絞って作るので、`FsCopyExif` は取り込み後に自動で出る
  (コード読み)。§1.363 のブランチの `KeyAction` 追加とは取り込み時に統括が合わせる。
