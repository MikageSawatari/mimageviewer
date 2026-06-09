# キー / マウス操作仕様 (静止画 vs 動画)

mimageviewer のフルスクリーン操作におけるキー / マウス アサインの整理。
画像 (静止画 / アニメーション GIF / PDF / ZIP 内画像) と動画でアサインが
異なる項目を一覧化し、整合性 / 不整合を明らかにする。

フルスクリーン境界ヒント、Ctrl+S/F/G 検索スコープ、動画タイルモードまで含めた
横断仕様と現状差分は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)
を参照すること。

一部のキーボード操作は上級者向けの `%APPDATA%\mimageviewer\keymap.ini`
で上書きできる。Action 名・書式・固定扱いの入力は
[keymap.ini.default](keymap.ini.default) と、起動時に生成される `keymap.ini` /
`keymap.ini.default` の先頭コメントを正とする。マウス、ゲームパッド、
OS/egui clipboard、D&D、IME 確定、右クリックメニューは keymap 対象外。
レーティングは専用の `[Rating]` グループ (`RatingItem1..5/Clear`、
`RatingContainer1..5/Clear`) で、グリッド / 画像フルスクリーン / 動画フルスクリーンが
同じ割り当てを共有する。テンキー数字は通常の数字キーと区別できない。
OS 予約ショートカット (例: Alt+F4 / Alt+Tab / Win キー系) は keymap では上書きできない。

開発者向けメモ: 新しいキーボード操作を追加・変更するときは、ユーザーから明示されて
いなくても keymap 対応要否を確認する。通常ショートカットは `KeyAction` に追加し、
`docs/key-customization-impl-plan.md` の「新しいキー操作を追加するとき」に従って
`docs/keymap.ini.default` まで更新する。固定扱いにする入力は、この文書の該当節に理由を残す。

## グリッドビュー (フルスクリーン外) 共通

| キー | 動作 |
|---|---|
| <kbd>Backspace</kbd> | 親フォルダへ。ドライブルート (`C:\` など) ではドライブ一覧へ戻り、元ドライブを選択状態にする。検索 (Ctrl+S / Ctrl+G) 中は検索仮想階層を 1 段ドリルアップ、最上位 (集約ビュー / 結果一覧) では no-op (検索を閉じるには <kbd>Esc</kbd> / 検索バーの <kbd>×</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd>・<kbd>Ctrl</kbd>+<kbd>S</kbd> 再押下)。Ctrl+F フィルタ中は、フィルタを実行したフォルダだけ親移動を no-op にする。検索結果から子フォルダへ入った後は通常どおり親へ戻れる |
| <kbd>Alt</kbd>+<kbd>↑</kbd> | 親フォルダへ (<kbd>Backspace</kbd> と同じ。Explorer 慣習に合わせた代替ショートカット。ドライブルートではドライブ一覧へ戻る。Ctrl+F フィルタ元フォルダでは no-op) |
| <kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | フォルダ履歴を戻る / 進む (フォルダバーの ←/→ と同じ。検索中・ドライブ一覧中は無効) |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> | ツリー順で前のフォルダへ (DFS pre-order、画像なしフォルダは skip_limit までスキップ)。検索中は前のヒットフォルダへ移動 (`global_search_ctrl_nav` / `favsearch_ctrl_nav`)。★固定 中は snapshot 内の前 entry へ |
| <kbd>Ctrl</kbd>+<kbd>↓</kbd> | ツリー順で次のフォルダへ (DFS pre-order)。検索中は次のヒットフォルダへ移動。★固定 中は snapshot 内の次 entry へ |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。同じ親の直下だけを対象にし、空フォルダも skip せず、子や祖先の兄弟には入らない。検索中は無効。★固定 中は snapshot 内の前/次 image-like entry へ (Folder/Zip/Pdf entry は skip) |
| <kbd>F1</kbd>〜<kbd>F5</kbd> | レーティング 1〜5。ドライブ一覧中は無効 |
| <kbd>F6</kbd> | レーティング解除。ドライブ一覧中は無効 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚) |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚) |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | チェック済み画像 / 選択中画像から消しゴムマスクを削除 |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | チェック済み画像 / 選択中画像から隠蔽マスクを削除 |
| <kbd>P</kbd> | 選択中アイテムを現在のコンテナの代表サムネに固定 / 解除 (toggle、フォルダバー 📌 の左クリックと同等)。pin 不能アイテム / 検索アグリゲート / 変換キャッシュ drill-down では silent no-op。**動画フルスクリーンの P と合わせて P = Pin に統一** |
| <kbd>X</kbd> | 選択中の画像 / ZIP 内画像 / PDF ページを比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>Space</kbd> | 選択中アイテムをチェック ON/OFF。画像 / 動画 / ZIP・PDF 本体 / 変換前アーカイブ / ZIP 内画像 / PDF ページが対象 (**フォルダとドライブ一覧は整理対象外**) |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | 表示中のチェック可能なアイテムを全選択 |
| <kbd>Alt</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Alt</kbd>+<kbd>0</kbd> | サムネイル列数を 1〜9 / 10 列に切り替え。詳細表示中はサムネイル表示へ戻してから列数を適用 |
| <kbd>Alt</kbd>+<kbd>-</kbd> | サムネイル表示 / 詳細表示を切り替え |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> / <kbd>X</kbd> | チェック済み、または選択中の実ファイルをコピー / カット (**フォルダは対象外** — v1.1.0 で一旦無効化) |
| <kbd>Ctrl</kbd>+<kbd>V</kbd> | クリップボードのファイルを現在の実フォルダへペースト (**フォルダは skip**)。ZIP/PDF/検索結果グリッドなど実フォルダ以外では無効 |
| <kbd>Delete</kbd> | チェック済み、または選択中の実ファイルを削除 (通常はゴミ箱。**フォルダは対象外**) |
| マウス左ドラッグ | グリッドのセルを掴んでエクスプローラ等へファイル D&D 送出 (コピー)。複数チェック選択時はその実パス群をまとめてドラッグ。フォルダ / ZIP・PDF 本体 / 変換前アーカイブも対象。ZIP/PDF 内画像 (仮想フォルダ) とドライブ一覧は対象外 |
| エクスプローラ等からのドロップ | mIV ウィンドウへファイルをドロップすると現在表示中のフォルダへコピー (**フォルダは v1.1.0 で一旦無効化・skip**)。ZIP/PDF / 検索結果グリッドなど実フォルダ以外を表示中は拒否 |
| グリッド空白の右クリック | 現在の実フォルダに新しいフォルダを作成、またはクリップボードのファイルをペースト (**フォルダは skip**) |

## フルスクリーン共通 (画像 / 動画とも)

| 入力 | 動作 |
|---|---|
| <kbd>Esc</kbd> | フルスクリーン解除。**ZIP/PDF 自動 1 ページ目オープン (環境設定 `auto_fullscreen_zip_pdf`) で入った場合は、ページ一覧 (L2) を経由せず親フォルダの一覧 (L1) へ直帰** (`handle_fullscreen_close_request` → `pending_return_to_parent` → 次フレーム `handle_keyboard` が親へナビ) |
| <kbd>Enter</kbd> | (画像) フルスクリーン解除 (Esc と同等、右手側ホームポジションからの解除キー)。グリッドで Enter / ダブルクリックで開く動作とトグル成立。自動オープン時の親直帰も Esc と同じ / (動画) 再生・一時停止トグル |
| <kbd>I</kbd> / <kbd>Tab</kbd> | メタデータパネル固定表示トグル (右パネル) |
| <kbd>Space</kbd> | (画像) 選択 (チェック) トグル — スライドショー再生中なら停止 / (動画) 再生・一時停止トグル |
| <kbd>Backspace</kbd> | 親フォルダへ → グリッドビュー。**自動 1 ページ目オープンで入った ZIP/PDF の場合のみ、親フォルダではなくそのコンテナのページ一覧 (L2) を表示** (= フルスクリーンを閉じるだけ。Esc/Enter の「L1 へ直帰」と対をなす) |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。同じ親の直下だけを対象にし、移動先に image-like があればフルスクリーンを維持して先頭 image-like を開く。なければ一覧へ戻る |
| マウスホイール | 前 / 次のファイル。縦/横連結モードでは連結方向へスクロール |
| マウス左クリック | (画像) ページめくり / (動画) 再生・一時停止トグル |
| <kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>F6</kbd> | 表示中アイテムへレーティング 1〜5 / 解除 |
| <kbd>Shift</kbd>+<kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | 現在のコンテナへレーティング 1〜5 / 解除 |
| <kbd>F11</kbd> | ウィンドウ内表示 ⇔ 全画面表示 を切り替え (右上 × の左のトグルボタンと同等)。静止画は egui 経路 (`toggle_still_window_mode` = 設定 flip のみ)、動画は native presenter 経路 (`toggle_video_window_mode` = presenter rebuild)。消しゴムモード中は無効化 (ホバーバーのトグルボタンも同モード中は非表示、`erase_mask_texture` の ctx-bound 問題回避) |

## フルスクリーン編集モード共通 (静止画)

消しゴム / 隠蔽加工 / 切り取り / テキスト注釈 / 補正レイヤーでは、ツール固有の描画・選択・ハンドル操作中でも、以下の閲覧操作を共通で使えるようにする。パネル上で開始した操作はパネル UI を優先し、画像上で開始した Space パンや中ボタンズームは途中でパネル上を横切っても継続する。

| 入力 | 動作 |
|---|---|
| <kbd>Space</kbd>+左ドラッグ | 一時パン。進行中の描画 / 図形 / crop / 注釈ドラッグは途中でパンへ切り替えず、現在の操作を完結させる |
| マウスホイール | 画像上ではズーム。スクロール可能なツールパネル上ではパネルスクロールを優先 |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| ホイール押し込み+上下ドラッグ | 中ボタンドラッグズーム。パネル上で開始した場合は無視 |
| 右 <kbd>Ctrl</kbd> 押しっぱなし | 元画像表示。補正 / AI / 消しゴム / 補正レイヤー / 隠蔽 / 注釈を一時的に外す。補正レイヤーの <kbd>Ctrl</kbd>+<kbd>Shift</kbd> は選択レイヤーバイパス表示を優先 |

## ゲームパッド操作

ゲームパッドは閲覧専用の固定割り当て。編集、削除、レーティング、チェック切り替え、
エクスポート、操作割り当て変更 UI は対象外。

| 入力 | 動作 |
|---|---|
| 方向パッド / 左スティック | グリッドでは選択移動。詳細一覧では上下が 1 行移動、左右が表示行数ぶん前後にスキップ。画像ではページ送り。縦連結では上下がスクロール、左右がページ送り。横連結では左右がスクロール、上下がページ送り。左スティックは連結方向を連続スクロール。動画では左右がシーク / タイルカーソル移動、上下が前後ファイル移動 |
| <kbd>A</kbd> / <kbd>B</kbd> | 決定・開く・再生 / 戻る・閉じる |
| <kbd>LB</kbd> / <kbd>RB</kbd> | Ctrl+↑ / Ctrl+↓ と同じ前 / 次フォルダ移動 |
| <kbd>LT</kbd> / <kbd>RT</kbd> | グリッドでは連続スクロール、画像ではズームアウト / ズームイン、動画では連続シーク |
| 右スティック上下 | 画像フルスクリーンのズーム。LT/RT より速め |
| <kbd>Select</kbd> | 画像フルスクリーンの連結方式切り替え (ページ単位 / 縦連結 / 横連結) |
| <kbd>Y</kbd> | 動画では S キー相当。画像では Y+左右で Ctrl+左右相当の見開き 1 ページずらし |
| <kbd>Y</kbd>+方向パッド左右 (動画) | J / K キーと同じ前 / 次のチャプター・ブックマーク・ピン移動 |
| <kbd>Start</kbd> | お気に入りを登録順に巡回。どのお気に入りにも含まれない現在地では 1 件目へ移動 |

## 画像 フルスクリーン

| キー | 動作 |
|---|---|
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次のファイル (見開き中は前 / 次の見開き = 2 ページ送り) |
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次のファイル (= 一般慣例で左右と同義)。縦連結では縦スクロール、横連結では前 / 次ファイル。スライドショー中もフォルダ内移動は再生を止めない |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 縦/横連結モードでは画面単位で連結方向へスクロール |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 見開きの「1 ページずらし」(現在ページを軸に見開きを 1 ページぶんずらす。空白/欠落ページでの綴じずれ補正。1 回押すごとに必ず 1 ページ動く)。**専用の状態は持たず見開きモードの cover/非cover (例: 左開き ⇄ 左開き+表紙) を切り替える形で表現する**ので、ホバーバー/数字キーと表示が一致し、`spread_db` でフォルダ単位に永続化される (開き直しても保持)。Single モードでは前 / 次ファイル。RTL では左右の意味を反転 |
| <kbd>0</kbd> 〜 <kbd>7</kbd> | <kbd>1</kbd>〜<kbd>5</kbd>: ページ構成切替 (<kbd>1</kbd>: 単ページ / <kbd>2</kbd>: 見開き 左開き / <kbd>3</kbd>: 見開き 左開き+表紙単独 / <kbd>4</kbd>: 見開き 右開き / <kbd>5</kbd>: 見開き 右開き+表紙単独)。<kbd>6</kbd>: 連結方式をページ単位 → 縦連結 → 横連結で循環。<kbd>7</kbd>: 横方向 左→右 / 右→左を切替。<kbd>0</kbd>: ズーム/フィットをページ全体 → ページ全体（余白カットフィット）→ 横幅フィット → 縦幅フィット → 100%原寸で循環 (連結モード中は余白カットフィットをスキップ)。見開き中は表紙あり/なしを保ったまま左開き / 右開きも連動して切り替える。ZIP の作品区切り表示上でも有効。ホバーバーの表示モード/フィットボタンからも切替可 |

連結モード (縦連結 / 横連結) は画像 / ZIP 内画像 / PDF ページの通常閲覧用。比較、360度パノラマ、分析、消しゴム、隠蔽加工、テキスト注釈、補正レイヤーなどの編集・解析系モードはページ単位モードでのみ起動する。
| <kbd>R</kbd> / <kbd>L</kbd> | 右 / 左 90° 回転 |
| <kbd>Z</kbd> | 画像分析モード |
| <kbd>S</kbd> | スライドショー 再生 / 停止。末尾動作 (ループ / 次フォルダ / 停止) は環境設定で選択。動画はスキップして継続。フォルダ内移動 (矢印 / ホイール / クリック / Home / End) では止まらず、Ctrl+↑↓ のフォルダ移動・S・Space・Esc で止まる |
| <kbd>M</kbd> | ルーペ トグル。360 度パノラマモード中は無効 |
| <kbd>Shift</kbd> (押しっぱ) | ルーペ。360 度パノラマモード中は無効 |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>B</kbd> | 透過背景色サイクル (黒 → 白 → 市松)。AI アップスケール時は黒 ↔ 白の 2 段 (市松は出力に焼き込まれるため不可) + 背景変更時に `clear_adjustment_caches` を呼び背景別 `(idx,bg)` 結果を表示し直す (idx キーの派生キャッシュ取り違えによる固着防止)。透過 (alpha) の無い画像では無効化してトースト案内 |
| <kbd>E</kbd> | 消しゴムモード開始 / 確定 |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード開始 / 終了 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | 現在ページの消しゴムマスクを削除 |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | 現在ページの隠蔽マスクを削除 |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | 現在の表示結果を別ファイルへエクスポート |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | 画像パイプラインのデバッグ出力 (`%APPDATA%\mimageviewer\debug-pipeline\...` に段階別 PNG と manifest を保存) |
| <kbd>P</kbd> | 現在表示中アイテムを現在のコンテナの代表サムネに固定 / 解除 |
| <kbd>X</kbd> | 現在表示中を比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>C</kbd> | 比較スロットのピン画像と現在画像をトグル表示 |
| <kbd>Shift</kbd>+<kbd>C</kbd> | Wipe 比較を ON/OFF (左=ピン、右=現在) |
| <kbd>Alt</kbd>+<kbd>C</kbd> | 差分比較を ON/OFF (RGB チャンネルごとの差分を色付きで強調表示) |
| <kbd>U</kbd> / <kbd>Shift</kbd>+<kbd>U</kbd> / <kbd>Alt</kbd>+<kbd>U</kbd> | AI モデル 次 / 前 / リセット |
| <kbd>T</kbd> / <kbd>Shift</kbd>+<kbd>T</kbd> / <kbd>Alt</kbd>+<kbd>T</kbd> | ポストフィルタ 次 / 前 / 標準 (リセット) |
| <kbd>V</kbd> | 360 度パノラマモード トグル (360 候補画像のみ) |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在画像 / アニメーション現在フレーム / ZIP 内画像 / PDF ページをキャプチャ保存フォルダへ保存 |

## 消しゴムモード

| 入力 | 動作 |
|---|---|
| <kbd>E</kbd> / <kbd>Esc</kbd> | 補完を実行して終了。選択中オブジェクトがあるときの <kbd>Esc</kbd> はまず選択解除 |
| <kbd>S</kbd> / <kbd>B</kbd> / <kbd>L</kbd> / <kbd>I</kbd> / <kbd>V</kbd> / <kbd>H</kbd> / <kbd>R</kbd> / <kbd>O</kbd> | 選択 / 筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円ツール |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | マスクまたは選択オブジェクトを 1px / 10px 移動 |
| <kbd>[</kbd> / <kbd>]</kbd>, <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd> | マスクまたは選択オブジェクトを ±0.1° / ±1° 回転 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 隠蔽加工モード

| 入力 | 動作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード終了 |
| <kbd>Esc</kbd> | 選択中オブジェクトがあるときは選択解除、なければ隠蔽加工モード終了 |
| <kbd>T</kbd> | 隠蔽タイプを順に切替 |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>1</kbd>〜<kbd>4</kbd> | プリセット 1〜4 を呼び出し |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>S</kbd> | 選択ツール |
| <kbd>B</kbd> | 筆ツール |
| <kbd>L</kbd> | 囲みツール |
| <kbd>I</kbd> | 直線ツール |
| <kbd>V</kbd> | 縦線ツール |
| <kbd>H</kbd> | 横線ツール |
| <kbd>R</kbd> | 矩形ツール |
| <kbd>O</kbd> | 楕円ツール |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | 選択オブジェクト、またはオブジェクト全体を 1px / 10px 移動 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 動画 フルスクリーン (Phase 7.H 適用後)

| キー / 入力 | 動作 | 備考 |
|---|---|---|
| <kbd>Space</kbd> / <kbd>Enter</kbd> | 再生 / 一時停止トグル | 動画 HUD 2 段化リデザイン (Phase 1) で Space を再生/停止に変更 (旧: 選択トグル)。チェックしたい場合は Esc で一覧へ戻る |
| <kbd>Backspace</kbd> | 一覧へ戻る | 画像フルスクリーンと同じ。native presenter 経路でも App 側へ転送する |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 外部プレイヤー起動 | |
| <kbd>←</kbd> / <kbd>→</kbd> | 5 秒シーク (デフォルト) | |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 1 秒シーク (細かい) | Phase 7.H |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 30 秒シーク (大きい) | Phase 7.H |
| <kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを前 / 次へ移動 | seek しない。現在位置より後の最初のタイルを時刻ラベル込みで強調表示 |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを 1 行分移動 | 列数分だけ前 / 次へ移動 |
| <kbd>Space</kbd> / <kbd>Enter</kbd> (タイル中) | タイルカーソル位置から再生 | S / Esc で閉じた場合は再生位置を変更しない |
| <kbd>P</kbd> (タイル中) | タイルカーソル位置のサムネイルを代表フレームとしてピン留め | マウス hover ではカーソルを動かさない。マウス操作はタイルクリックだけが seek として反応する |
| <kbd>↑</kbd> / <kbd>↓</kbd> | **前 / 次のファイル** (画像と同じ、マウスホイールと同じ) | Phase 7.H |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 音量を dB フェーダー目盛りの 1/4 幅で上下 | |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 現在コンテキストの前 / 次フォルダまたは検索結果へ移動 | native presenter 経路でも有効 |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ | 同じ親の直下だけを対象にし、空フォルダも skip しない。検索中は無効 |
| <kbd>M</kbd> | ミュート トグル | |
| <kbd>L</kbd> | ループ再生 トグル | 連続再生 ON 中は無効化し、「連続再生中はループ無効」を表示 |
| <kbd>B</kbd> | ブックマーク追加 (現在位置 🔖) | |
| <kbd>S</kbd> | タイルモード ON/OFF | |
| HUD 連続再生ボタン | オフ → 連続再生 → 連続再生 + ループを循環 | ループ再生とは排他。アプリ再起動時は OFF |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在フレームをキャプチャ保存フォルダへ保存 | v0.10 MVP。egui / native presenter 両経路で有効 |
| <kbd>X</kbd> / <kbd>C</kbd> / <kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>Alt</kbd>+<kbd>C</kbd> | 比較ビュー対象外のため silent no-op | native presenter 経路でも passthrough しない |
| <kbd>P</kbd> | 現在再生位置をピン留め (= HUD 📌 ボタンと同等)。タイル中はタイルカーソル位置をピン留め | v0.9.x、グリッドの P (folder_thumb_pin toggle) と統一した「P = Pin」 |
| <kbd>F</kbd> | Perf / フレームレート オーバーレイ トグル | v0.9.x、以前は P。P を Pin に再割り当てしたため F (Frames) へ移動 |
| <kbd>Esc</kbd> (タイル中) | タイルモード解除 | |
| マウス左クリック | 再生 / 一時停止トグル (HUD/パネル除く) | |
| マウスホイール | 前 / 次のファイル | 画像と同じ |
| <kbd>Ctrl</kbd>+ホイール (タイル中) | 列数切替 (4/6/10/16/20/26/30) | 上部バーの 3x3 / 5x5 アイコンボタンでも同じ操作可 |

## 不整合の解消 (Phase 7.H 適用後)

| 入力 | 画像モード | 動画モード | 状態 |
|---|---|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| マウスホイール | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次フォルダまたは検索結果 | 前 / 次フォルダまたは検索結果 | ✅ 揃った |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル (= ↑↓ と同義) | 音量を dB フェーダー目盛りの 1/4 幅で上下 | ⚠ 残った差異 (許容、動画プレイヤー慣例) |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次ファイル | 5 秒シーク | ⚠ 動画プレイヤー慣例 (mpv/VLC/YouTube) で許容 |
| マウス左クリック | ページめくり | 再生 / 一時停止 | ⚠ 動画プレイヤー慣例で許容 |

## ★固定 (Snapshot Lock) 中の挙動

★ filter ツールバー右端の `[★固定]` ボタンで現在の絞り込み結果を一時 snapshot 化して
凍結する機能 (v1.1.0+、設計: [star-lock-snapshot-design.md](star-lock-snapshot-design.md))。
snapshot 中のキー操作は以下のように再定義される:

| 入力 | snapshot 中の挙動 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | snapshot 内の前/次 entry へ (= 混合 nav: Folder/Image/Video 全部対象、Folder entry は中の最初の image を fullscreen で開く) |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | snapshot 内の前/次 **image-like** entry のみへ (Folder/Zip/Pdf entry は skip) |
| スライドショー末尾 | snapshot 内の次 playable entry へ自動遷移 (= ★5 folder 巡回の主用途) |
| ★ filter ボタン | 操作可能、ただし top-level grid 表示は snapshot のまま凍結 (= captured folder の中身には作用する) |
| <kbd>Backspace</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd> / <kbd>Alt</kbd>+<kbd>←</kbd>/<kbd>→</kbd> / フォルダツリー / お気に入りクリック / フォルダパス入力 | 無効、toast「スナップショット中は他のフォルダに移動できません」 |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> / <kbd>Ctrl</kbd>+<kbd>S</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd> | snapshot 自動解除 + 検索 mode 起動 (= scope mutual exclusion) |
| `[★固定]` ボタン再クリック | snapshot 解除 (= 元のフォルダ表示に戻る) |

snapshot 末尾到達時は `FsBoundaryHint::NoImageFolder` で boundary hint を表示。

## 設計メモ

- 動画モードで ↑↓ をファイル移動に再アサインする方針は、旧 egui 経路では
  `handle_video_input` がプレーン ArrowUp/ArrowDown を consume せず後段へ流すことで
  実現していた。現行 Windows native presenter 経路では
  `app/native_video.rs::handle_native_video_key_event` が plain ↑↓ を直接
  `navigate_native_video_fullscreen` に流している。
- Ctrl+↑↓ も同じ思想で native key handler から
  `handle_fullscreen_ctrl_nav_context` へ流し、フォルダ / Ctrl+S / Ctrl+G の
  スコープ解決を画像系と共有する。
- 5/1/30 秒シークの粒度は動画プレイヤー一般の慣例 (mpv: ←→=5s, Shift+←→=1s,
  ←/→ alone in YouTube=5s, J/L=10s) を踏襲しつつ、modifier で粒度切替できる
  ようにした。
- 既に先頭 / 末尾に居て ←→ シークが動かない場合は、シークを発行せず
  「動画先頭です」「動画末尾です」のトーストを出す (詳細は
  [video-architecture.md](video-architecture.md) の seek HUD 節)。
- タイルモード中の ←→ は修飾キーの有無に関係なくタイルカーソル移動を優先する。
  Shift は無視し、Ctrl が含まれる場合だけ 1 行分移動にする。
- 音量は HUD 下部の dB フェーダーをマウスでドラッグして調整可能。
  キーボードでは Shift+↑↓ で -∞/-60/-40/-20/-10/-5/0/+6/+12/+18dB の
  目盛り間を 1/4 幅ずつ移動し、plain ↑↓ アサインは廃止 (= プレーン ↑↓ を
  ファイル移動に譲るため)。
