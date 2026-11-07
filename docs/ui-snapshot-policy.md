# UI スナップショットテスト運用方針

3D写真 段C（2026-11-07）は `depth_export_settings_small_dark` /
`depth_export_progress_small_dark` の2枚（1093×614、55,224 / 29,457 bytes）を追加した。
設定の各選択・既定4ループ・固定操作行・タイトル×、進行数と取消の表示を実装担当と
独立reviewerが目視した。`depth_export_small_viewport_keeps_actions_visible_and_reaches_settings`
は1093×614と546.5×307（200%相当の論理面積）でスクロール末尾・固定操作行と
保存／取消／タイトル×の実クリックを検査する。進行表示は短文の自然高を維持する。
純描画fixtureで、App・worker・製品アプリは起動しない。UPDATEなしのdepth関連6/6成功、
既存535枚のSHA-256変更0。実行ログと段C全体の検証は[3D計画 §10](depth-parallax-plan.md#10-実装の段階-codex-への委任単位)に集約する。

§1.730 は共通 modal の Confirm を純粋描画する `caption_delete_confirmation_snapshot` で
`caption_delete_confirmation_dark.png` 1枚を保持する。App・DB・worker は fixture に含めない。
Counting / Deleting の可視性・全 phase の modal、集計取消後の遅着結果、Enter の非確定、Escape と IME、
1093×614 の Confirm のボタン・×は別の headless 回帰で検査する。
設定 catalog の PNG は `preferences_context_menu_layout{,_open_with}.png` の2枚。実アプリ操作の証拠とは分ける。

§1.727の加工内容選択は、製品と共通のWindow描画関数を使うlibの
`effect_picker_default_dark_snapshot`で、既定サイズ860×560の窓（PNGは1000×660）の
`local_adjust_effect_picker_default_dark.png`を1枚保持する。製品の暗色テーマ・
スクロールバー設定・既定フォントを適用し、App・保存先・worker・製品ウィンドウを
構築しない。既存PNGは更新しない。非整数DPI 1.25、製品UI倍率200%、Meiryoの
各条件はPNGを増やさず、全表示名の幅・30pt高・実列数・24passの窓寸法不変と
長名の実paint/clipをheadless回帰で検査する。Meiryoは選択faceの登録と既定との
実字幅差も必須にし、フォールバックによる条件の空振りを許さない。
追加PNGは95,352 bytes。見出し・検索行・長い効果名・列配置・待機中のスクロールバーを
生成後に目視確認した。製品起動・実機確認の証拠とは分ける。

§1.704 の場所メニューは、本体共通のメニューボタン／既知フォルダ行rendererを固定pathの
fixtureから描く `folder_location_menu_dark`（260×230、10,055 bytes）を追加する。
デスクトップ・ピクチャ・ダウンロード・ビデオ・ミュージックの順と文字を目視確認した。
既存 `folder_bar_organize_destinations_toggle`（480×700、82,453 bytes）だけを更新し、
新しい2項目のチェック表示と本文のscrollbarを目視確認した。2枚は計92,508 bytes。
既存のApp fixtureを更新するが、新しいメニューPNGはApp・OSの実フォルダ・workerを使わない。
実際の設定popupの614pt／900ptで、必要なときだけscrollし、wheelで末尾へ到達して
「フォルダバーを隠す」を操作できることと背面へのwheel非伝播をheadless検査する。
並び順・欠損・重複・表示OFFは共通列挙のstateテスト、実フォルダ取得はKnown Folder APIの
専用テストで分ける。更新指定なしの対象PNG比較は成功。製品起動・実機検収の証拠とはしない。

v5受入B（2026-11-04）の登録確認は`agent_registration_confirmation_small_light`
（1093×614、70,109 bytes）を1枚追加する。実環境・設定ファイル・登録workerを使わない
固定fixtureで製品と共通の確認Windowを描く。本文・設定path・追加する内容・backupの説明・
固定操作行とタイトル×が収まることを生成後に目視した。既存10枚は更新しない。
`cargo test --test ui_snapshot agent_`の更新指定なし通常比較11件が成功した。
100%/200%の40frame geometry不変と長文の末尾到達／操作可視性はlibの専用回帰で検査し、
実アプリ初回クリック・実際の登録ファイル書き換えの証拠とは分ける。

§1.677の種類iconは利用者採択の配色A・既定ON・セル枠左上(4,4)pt・図形20pt＋背景24ptで固定する。
`thumbnail_type_icon_dark/light`の既存baselineはそのまま維持し、今回の仕上げでPNGは更新しない。
比較候補のB/C・色なし・16/24ptはsourceから撤去する。ignoredの`thumbnail_type_icon_write_previews`は残し、
確定仕様のOFF/ON・狭幅・最小32pt・多数badge・代表画像なし・名前200%最大4行・中央bookmark titleを
dark/light各6枚、`target/1677-preview/adopted/`へ明示実行時だけ生成する。過去の比較PNGと混ぜない。
通常のテストではPNG書き出しを行わず、2枚のbaseline比較と純layout・実paint回帰を維持する。
製品起動や実速度測定の代わりには扱わない。検証結果の正本は[§1.677の実装記録](next-release-backlog.md)。

§1.673 のバケツは、lib の純描画 fixture で `bucket_drawn_mask_options_dark`
（250×300、27,771 bytes）を1枚追加する。3ツール共通の基準選択・色差の無効表示・既存の範囲と
漏れ止め／はみ出しを描き、切替後の色差値保持もheadless入力で検査する。
生成PNGを目視し、選択状態と無効な色差表示を区別でき、文字・最下行が収まることを確認した。
既存PNGは更新しない。App・保存worker・製品起動を含めない。
3ツールのhandlerと保存再読込は別のlib回帰で検査する。

v0.7.0 で導入した `egui_kittest` ベースの UI 回帰テストの運用ルール。

§1.663のファイル名幅優先は `thumbnail_filename_width_dark`（460×440）の1枚で、
ON・100%・1行のフォルダ代表画像あり／なし、ZIP、VIDEO表示と名前・評価・長さ・前回位置を保持する。
既存の名前表示fixtureと同じ純粋レイアウト・セル描画を共有し、OFFの既存一覧PNGは更新しない。
設定追加による `preferences_favorite_view_state_dark` のみ更新する。
文字サイズ最小／最大・1〜4行・狭いセル・全形式・四隅とメーターの組み合わせは純粋レイアウト、
明暗×DPIの名前のink・当たり判定はheadless実painter、環境設定で編集→OK→本体配置はheadless入力回帰で検査する。
製品起動や実機検証の証拠とは区別する。
新規一覧52,855 bytesと更新設定ページ75,586 bytes（計128,441 bytes）を生成後に目視し、
形式と名前・評価・長さの分離、設定の説明・既存操作が表示範囲に収まることを確認した。

## 目的

§1.672 の実親移動は `physical_parent_folder_bar_dark`（760×90）の1枚で、
論理親と独立したフォルダ＋上矢印ボタンのON表示を保持する。本体共通のpainterと
folder controls／幅調整／address inputを使う純描画fixtureで、App・実ファイルを開かない。
入力欄も既存IME helperとラベルの関連付けを使う。
設定checkbox追加により既存 `folder_bar_organize_destinations_toggle` だけを更新し、
新規10,592 bytes＋更新81,653 bytes（計92,245 bytes）を目視確認した。他の既存PNGは変更しない。
表示ON/OFF、360pt／1200ptでの入力欄の幅、メニューから表示へ反映、既定へのresetは
headless UI／geometryテストで検査する。製品起動・実機確認とは区別する。

mIV Sound F2a の名称変更では `preferences_effetune_input_limit_dark` と
`preferences_effetune_auto_open_light`、`preferences_effetune_taskbar_narrow_dark` の3枚を更新し、
文字以外の項目・配置を目視確認する。
About の上流2本の著作権と credit は、本体と共用する描画helperを使う
`miv_sound_credits_light`（480×180）と `miv_sound_credits_narrow_dark`（280×180）で確認する。
追加2枚は計40,255 bytes。狭幅の折り返し後にも全文が収まり、製品の起動や実データ操作は含まない。

ローカル AI 説明文のモデル導入 (V1-3b) は `vlm_setup_confirm_dark` (1093×614) と
`vlm_setup_preferences_dark` (600×260) の2枚を追加する。製品と共通の純粋描画を使い、
App・実モデル・HTTP・worker process を起動しない。確認／取得／非対応／完了／長いエラー／取消／削除確認の
7状態は1093×614と546.5×307 logical points (同じ物理画面の200%相当) で、全ボタンとタイトル×の矩形、
各状態の主操作のクリック到達性をheadlessで検査する。追加PNGは計133,451 bytes。
2枚を目視し、本文・注意・ボタン・busy時の無効表示が切れていないことを確認した。
既存 `preferences_search_results` は「AI 説明文」の検索結果が1行増えるため更新した。
旧画像／新画像／diffを目視し、既存4行は同じで追加行だけが差分 (閾値0.6で367px) と確認した。

タグビューの付与時刻（§2.23）は lib の `tag_sort_choices_dark`（320×240）の1枚で、
本体共通の専用3択（正式名・選択中表示）、列ヘッダソート中の無効な短名ボタン、
「タグ付与時刻」列の見出しと日時表示を保持する。App・DB・実ウィンドウを起動しない純描画fixtureを使う。
実際のメニュー・toolbar・facetで通常ソートが出ないこと、列ソート中の無効化と更新操作の維持は
headless UI 回帰で検査する。既存PNGは更新しない。

サムネイル一覧の名前表示（§1.456）は `thumbnail_name_style_multiline_dark`（460×440）の
1枚で、名前150%・最大3行の長い日本語名、フォルダ代表画像あり／なし、ZIP、動画を保持する。
実ファイルやAppを起動せず、本体共通の純粋レイアウトとセル描画へ生成textureを渡す。
既定の一覧セルのPNGは維持し、設定ページ全体の `preferences_favorite_view_state_dark` は
名前の文字サイズ・最大行数の項目追加に合わせて更新する。
明暗×DPIと描画ink／当たり判定の一致はheadless実painter回帰、名前の折り返し・行数削減・
全バッジの非交差は純粋レイアウト回帰で検査し、PNGを条件の全組合せへ増やさない。
生成後に新規・更新の両画像を目視確認した。新規1枚82,937 bytes、更新1枚70,101 bytes、
計153,038 bytesで、文字・説明・最下行が表示領域内に収まることを確認した。

フォルダツリーの現在地 (§1.451) は `folder_pane_location_light` / `folder_pane_location_dark`
の2枚 (420×380) で、本体と共通の行描画を使う。「現在地のみ・フォーカスなし」、
「操作カーソルと現在地が別行・フォーカスあり」、「同じ行・フォーカスあり」を各画像にまとめる。
背景判定の全8状態は純関数のunit testで検査し、現在地同期やworkerをsnapshot fixtureへ持ち込まない。
2枚は計64,747 bytes。生成後に両方を目視し、現在地の薄い背景と操作カーソルの選択背景が
明暗とも区別でき、文字・展開矢印・最下行が切れないことを確認した。既存PNGは変更しない。

Ctrl+G の短い語の拒否 (§1.229) は `global_search_short_term_reject_light` /
`global_search_short_term_reject_dark` の2枚で、単語1つと複数語の表示を保持する。
製品と共通の幅制限付きバッジを hover し、省略されたメッセージの全文も確認する。
App・索引・実ウィンドウは起動しない。

Everything 検索の無効理由（§1.386）は `everything_search_disabled_menu_hover_light` /
`everything_search_disabled_menu_hover_dark` の2枚で保持する。製品と共通の無効buttonを実際にhoverし、
理由本文・disabled状態を検査する。640×180の2枚は代表サイズであり、最小幅を意味しない。
`MIN_INNER_SIZE` はUI倍率で割ってviewportへ設定され、小さい作業領域では最小寸法自体も縮む。
最初の480ptのはみ出しをfixture拡大で避けた判断は誤りだった。共通tooltipを表示可能な論理幅から
枠幅を引いた幅で折り返し、480pt・200%で320pt・小さい作業領域の200%で240ptもgeometry検査する。
文言全文の矩形がviewport内に収まることをLight/Darkで検査し、共通helperの変更時は他のUI snapshotも比較する。
カーソル回避の位置はOSの現在の矢印／Iビーム等に依存させない。Everythingの公開fixtureは
ホットスポットの上／下への物理px寸法を引数で受け、snapshotと倍率geometry検査は `(0, 34)` を明示する。
製品は従来どおり実OSカーソルを計測する。両経路は寸法→アンカー変換とtooltip本文・配色・幅制限を共有し、
テスト検出用の環境変数／Context状態やOSカーソル変更は使わない。
2026-10-20に固定寸法で2枚を再生成し、目視確認した。既存PNGとbyte単位で一致するため画像差分はなく、
更新指定なしのUI nextest全191件も成功。検証ログはEverything計画§20.6を参照する。

egui の描画結果を PNG スナップショットとして保存し、意図しない見た目の変化を
`cargo test` の段階で検出する。「ダイアログの色が変わった」「ラベルが折り返して
レイアウトが崩れた」「テーマ切替で文字が読めない配色になった」などを早期にキャッチする。

**目的外の用途:**

- 機能テスト (値の正しさ、ロジックの挙動) → 通常のユニット/統合テストで行う
- パフォーマンステスト → `bench_scroll` 等のベンチマークで行う
- E2E シナリオテスト → `docs/e2e-smoke-test.md` の Computer Use 手動実行で行う

## 実行

通常フォルダの既定ページ構成（§1.425）は、本体と共通のチェック項目・説明文を描く
`preferences_plain_folder_spread_light_off` と `preferences_plain_folder_spread_dark_on` を追加する。
400×140 の明暗・OFF/ON を目視確認し、同じ headless fixture でチェック切替も検査する。
2枚は計42,193 bytes。既存 `preferences_viewer_notice_visibility` も更新指定で実行し、
表示範囲内の本文・スクロールバーが従来PNGとbyte単位で一致することを確認した。

```bash
# 通常実行 (既存スナップショットと比較)
cargo test --test ui_snapshot

# 意図的に見た目を変更した場合のスナップショット更新
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot
```

更新後は `tests/snapshots/*.png` の差分を必ず目視確認してからコミットする。
git diff のバイナリ比較では変化の意図 (改善なのか回帰なのか) は判断できない。

音楽ビデオ P4d（2026-10-15）は既存の
`preferences_context_menu_layout{,_open_with}.png` の2枚を更新する。
本体と共通の設定表へ「音楽ビデオを作成」を追加し、確認場面へ一覧／フルスクリーンの音声を追加した。
画像の確認場面では新項目は「対象外」。1枚目の本文は同一で、スクロールバーだけ102px変わる
（範囲 x=604..611 / y=453..466、174397→174395 bytes）。2枚目は新しい1行と末尾への
スクロール位置が変わり、283125px変わる（範囲 x=24..611 / y=5..588、163198→163441 bytes）。
実装担当が更新前後の2枚を目視し、文字の欠け・重なりがないことを検査した。
寸法620×700、2枚合計337836 bytes。新規PNGは追加しない。
非対話の描画fixtureによる検査であり、製品GUIの起動や独立レビュー、実機検収ではない。
テスト件数と変異検証は [音楽ビデオ計画のP4d実装記録](music-video-export-plan.md#p4d-実装記録2026-10-15) を参照。

2026-11-03のv5統合では、ローカルAI説明文と音楽ビデオの双方を含むコードから、
libの `preferences_context_menu_layout_snapshot` だけを `UPDATE_SNAPSHOTS=1` で実行して
同じ2枚を再生成した（各620×700、168,289 / 159,378 bytes）。片親の画像は採用しない。
1枚目に「AI 説明文を作る」と既存の表示・順序・区切り線の操作、2枚目に
「音楽ビデオを作成」（画像の確認場面なので対象外）と「アプリケーションで開く…」内の
外部ツール設定があること、操作列の重なり・文字欠けがないことを目視確認した。
更新指定なしの比較は関連nextestに含めて成功。非対話のheadless fixtureで、製品は起動しない。
統合・検証の記録は [v5統合記録](v5-integration.md) の§11を参照。

閲覧中のページ一覧（§1.340）は専用8枚に加え、unit snapshotの
`fullscreen_overflow_panel_root`で「ページ一覧 [O]」を含む7行の入口を検査する。
行のtouch targetは48pxを維持し、狭い面では共通のscroll／clampを使う。
rootの入口追加で変わったPNGは1枚だけを更新して目視確認し、navigator submenuの基準は維持する。
これはnext-tile側の統合前の検証履歴。2026-10-27のmaster取り込み後は、共通overflowの
行高・行間修正と一覧入口を併せて比較する。再生成したRoot／RootTall／PageTurnOnは親担当が
実PNGを目視し、先頭の構成編集、次のページ一覧[O]、既存のページめくり／スライドショーを確認した。
Tall fixtureは10行×48ptにviewport高608ptが必要なため600→614へ調整した。
600ptで8ptスクロールする製品layoutは正しく、製品layoutは変更していない。
overflow panel8件は成功したが、同時実行のF11 fixture1件は失敗。guide2枚も再生成・目視確認済み。
後続のF11診断では実child 340×420／inner 326×372の全域に不透明BLACKを確認した。
固定canvasを前提にしたfixtureを、同passの実Area採取と独立した大きなWHITE背景に対する
全域BLACKの検査へ修正した。r7の1372件中1371 passed／1 F11 fixture failedを経て、
F11だけの最終focused検証は2 passed／0 failed、コンパイル込み95.47秒。
full rendererのPrime→Apply／SettleのBLACK・期限終了後のoverview復帰と、既存placement境界の
`ctx.run_pass`内でlive fake hostを渡して実`step()`がSettleへ進む検査を分ける。
実`IsWindow`がfake hostを無効化するheadlessの制約を明示し、実HWND遷移の検証と混同しない。
製品変更・新しいtest seam・Settle強制代入は行わず、既存deadlineだけをfixtureで設定する。
r7の他1371件は再利用し、focused 2件との重複合算や一度の1372件全成功は主張しない。
統合後の部分結果と未完了事項は[一覧設計書§25.5](detached-book-page-overview-plan.md#255-追加の部分検証と画像確認2026-10-27検証中)へ記録する。

音楽タイムラインの調・低域音量（鍵盤設計§14、2026-10-14）は、libの純ラスタ描画で
`timeline_bass_partial_raster`（240×80）と`timeline_key_states_raster`（660×80）を検査する。
前者はベース高さの低域音量化とKey既定色の灰色化で更新（変更2798px、範囲x=8..207/y=23..39）。
計算済み側のベース色と未計算側の灰色、上段波形が保たれることを更新前後のPNGで目視確認した。
後者は新規で、未計算／計算済み調なし／C長調を横並びにし、前2つの灰色と推定調の色を確認する。
新規PNGの説明ラベルも本体と同じ`ui_fonts::configure_fonts`で描く。
第3段階時点の2枚の合計は9336 bytes（1544＋7792）。実アプリ・Appの起動は行わない。

音名配色と音楽メニュー（鍵盤設計§15、2026-10-14）は、上記2枚の音名色を更新する。
C=赤と暗い音名だけの明度補正を鍵盤・ベース／調で共有する変更による。
上段の波形や未計算／調なしの灰色は維持する。
`music_overflow_hud_normal`（820×300）と`music_overflow_hud_narrow`（320×300）は
本体と共通の純粋な上部HUD描画を使い、「…」・Row・タイトル・閉じる操作が重ならないことを確認する。
`music_overflow_panel`（820×300）は共通overflow surfaceの「音名の色」の2択と選択中表示を確認する。
これら3枚は本体フォントを設定したlib harnessで生成し、App全体をsnapshot対象にしない。
実入力回帰は別のdisposable App fixtureで本体の`render_fullscreen_viewport`を通す。
第4段階の既存2枚は計9343 bytes（1544＋7799）、新規3枚は計46877 bytes、5枚合計56220 bytes。
既存2枚の変更はベース音の1820px（x=8..147/y=23..35）とC長調の3400px（x=424..623/y=47..63）だけ。
更新前後と新規3枚を目視確認し、波形・灰色の保持、文字表示、HUDと操作の非重複を確認した。
実行結果は鍵盤設計§15.6へ記録する。

第4段階の追加レビュー修正（鍵盤設計§15.7）は、共通パネルの行間3pxを高さ算定に含める。
`fullscreen_overflow_panel_root`（720×440）と`fullscreen_overflow_panel_navigator_submenu`（720×440）、
`music_overflow_panel`（820×300）は、基準画像を変更せずに通常比較で候補PNGを生成し、
更新前に左右比較を目視した。不要なbarが消え、パネル高がそれぞれ15／12／3px増え、
最終行と選択行が切れずに収まる。行間・文字・HUDは維持する。
3枚の変更は9883／10231／2883px、新しいbytesは29893／15285／22472。
通常幅／狭幅の音楽HUD画像は変更しない。確認画像と統計は`target/review-fixes-validation/`に保存する。
`spread_shift_hud_{resolved,unresolved}_{normal,narrow}_{light,dark}`の8枚も、
更新前の左右比較で目視確認した。本文は同じ自然な高さを保ち、余分な下部空白を除いて
パネル高を348→282pxへ縮める。差分はy=338..411の下端だけで、本文・marker・disabled表示・
チェック欄は一致する。normalは各26832px、narrowは各18792pxが変化し、画像寸法とbytesは維持する。

pitch-labのmaster統合では、上記P3の高さ・行間修正をmasterの共通実装へ統一する。
本文の行間は0、固定行高48ptで計算し、音楽の2択も同じ計算と
`test_ui::scroll_layout::ScrollLayout`の収まり／実入力による末尾到達検査を使う。
上記P3の画像説明は統合前の履歴。静止画Root／Navigatorと見開き調整HUD8枚はmaster側PNGを採用し、
以後の通常比較で維持を確認する。音楽パネルの更新前後は目視確認し、変更は鍵盤設計§15.8へ記録する。

本別見開き調整 (§1.364 D2) は`spread_shift_hud_{resolved,unresolved}_{normal,narrow}_{light,dark}`の
8ケースを定義する。normalは720×500、narrowは320×480。実HUDの調整submenuと調整の印を
使い、resolvedの基点jump・解除・全体checkboxの到達性を確認する。unresolvedはjumpのみ無効にし、
保存済み調整の明示解除を維持する。fixtureの操作は保存や製品起動を行わない。
基準PNGの生成・目視確認・実行結果の記録は検証ownerの担当で、テスト定義の追加だけで完了扱いにしない。

詳細一覧の名前色 (§1.346) は `details_name_colors_*` と `details_name_color_settings_*` の
Light／Dark × 標準／強い、計8枚を保持する。前者は六分類・長い日本語名・hover・選択・チェック・
切り取り・選択情報バー・カスタム・OFF、後者は本体と共通の環境設定表・比・警告・強い固定色を描く。
`cut_item_appearance_light/dark` は名前が不透明、他列とプレビューが薄い境界を合わせて確認する。
名前色の追加8枚は合計1,157,523 bytes (約1.10 MiB)。一覧620×580、設定表760×800で全項目を収める。
`preferences_favorite_view_state_dark` は同じページ全体を縦スクロールするため、名前色表を追加すると
スクロールバーのつまみが短くなる。既存本文が変わらないことを差分画像で確認して期待画像を更新する。

外部ツールの `{file_list}` (§1.329) は `external_tool_file_list_help_light/dark` と
`external_tool_preparing_light/dark` の4枚を追加する。本体と同じ記法説明・準備中の件数・
キャンセルボタンを描き、アプリの起動や外部プロセスの実起動は行わない。

環境設定の位置復元 (§1.365) は `preferences_restore_position_light_on` と
`preferences_restore_position_dark_off_narrow` の2枚で、本体と共通のチェック項目を描く。
既定 ON のライト表示と、OFF のダーク表示・狭い幅での折り返しを確認する。

本のページ一覧 (§1.340) は `page_overview_light/dark`、
`page_overview_narrow_light/dark`、`page_overview_rtl_dark`、
`page_overview_white_companion_dark`、`page_overview_eight_columns_dark`、
`page_overview_twenty_columns_dark` の8ケースを定義する。通常は600×420、狭幅は200×440、
8列は960×480、20列は2200×440。初期8列と上限20列を実際に描画し、通常／狭幅は
最小セル幅で実効列数が減ることも確認する。Light／Darkは親のテーマを表し、一覧自体は
共通rendererのchild Uiに`os_theme::apply_dark_ui`を適用して常に黒ベースにする。
親visuals／Contextの両theme style／theme preferenceの不変と強い文字コントラストの維持はunitテストで検査する。
本体と共通のrendererで固定ヘッダ・戻る・列数・番号／短名・現在ページとcursor・
placeholder／失敗セル・RTL順・白紙／表示余白の補助表示を描く。
fixtureはApp・実ファイル・workerを使わず、全ページ生成や製品起動を行わない。
取消／選択／wheel消費と小面の戻る到達性はheadless handlerテストで別に検査する。
PNG生成・目視確認・実行結果の記録は検証ownerが担当し、定義追加だけを実行済みと扱わない。
2026-10-27のmaster統合では、構成採用後の可視実items順とhiddenを一覧へ反映する。
白紙補助annotationは`None`のまま未接続で、白紙fixtureの描画はadapter契約の検査に限る。
Selectは既存照合の全通過でClosed→既存landing一回とし、Checking後許可・拒否や同期拒否でも
一覧を再openしない（一覧設計書§25.1追補）。この遷移はPNGと別のhandler回帰で確認し、
以下はgate導入前の画像更新・部分検証の履歴。現行gate導入後の指定自動検証成功は一覧設計書§25.6へ記録する。
初回`ui_snapshot`全件は203 passed／overview8件の画像差分。8枚の候補は親担当がすべて目視し、
geometry・背景・列数・RTLの保持を確認した。`ui-overview-update`は8 passed／8枚再生成、
コンパイル込み143.17秒。dark／eight columns／narrow lightの実PNGも親担当が追加で開き、
確認済み候補との見え方の一致を確認した。更新指定なしの`ui-snapshot-final`全件比較はexit 0、
211 passed／0 failed／0 ignored、コンパイル込み87.60秒。ここまでは通常／test-script check後の変更は
test-fixtureの診断だけだった。その後coordinatorは、受理済み移動を保持し、既存barrierの
Turning／Completing中だけ新規overview入場を拒否する新手順2を採択した。旧選択肢2の即退役ではない。O／HUD／ring／padは
同じavailability／open受付を使い、完了後にもう一度操作する。入場要求の保留・replay・新取消は作らない。
`app/page_overview.rs`の既存gate変更後にも全UI snapshotを通常比較し、211 passed／0 failed／0 ignored、
コンパイル込み135.37秒・テスト実行30.65秒で成功した。前回再生成した13枚はそのままで追加再生成はない。
新規O回帰2件、関連lib 1374件も特定除外なしで成功し、指定自動検証は完了、未merge。
最新の条件・件数と独立レビュー／full-lib／build-dev／実機確認の引継ぎは一覧設計書§25.6に記録し、
自動検証成功をこれらの受入まで完了したとは扱わない。
同frameのfront pointer captureをprimary終端／一覧／通常canvasで共有するmaster合流には、
old-owned releaseを取り逃さない3surfaceの実renderer回帰を追加した。追加時点の記録は§25.3、
その後の実行結果は§25.4へ記録する。PNGやテスト追加だけを入力終端の成功証拠にはしない。

TILE11の右ドラッグ／gesture認識とX長押しguideの接続漏れは、一覧rendererのearly returnが
共通入力・overlayを飛ばしたため、labelのpure testや`draw_page_overview`単体snapshotでは
検出できなかった。これらの検証とは別に、通常fullscreen／メイン内閲覧／active detachedの
実rendererへ入力を注入し、認識→paintされたguide→許可／禁止slotのreleaseまで検査する。
中ボタン短押し、primary取消／suppression終端、F11 stepとprime black frameの描画抑止も
共有境界の回帰に含める。passive Aとactive Bでmode／集合が異なる場合は、Aのimmutable
実行可能集合に基づくlabelを確認する。製品起動を伴わないheadless renderer回帰で補い、
App全体のsnapshotへ置き換えない。実数・結果は[一覧設計書§23](detached-book-page-overview-plan.md#23-一覧で省略された共通入力案内の接続tile112026-10-13)に記録する。

音響調整のタスクバー表示 (§1.378) は既存の`preferences_effetune_input_limit_dark` /
`preferences_effetune_auto_open_light`と、追加した320px幅の`preferences_effetune_taskbar_narrow_dark`で確認する。
本体と共通の描画helperを使い、既定ONのcheckbox、明暗の文字コントラスト、狭幅での折り返しと到達性を検査する。
native caption / taskbarの実Shell表示はこれらegui PNGの範囲外であり、利用者の実機確認で確定する。

2026-10-13の表示・前面化専用ボタン (§1.378) は`effetune_show_only_active_tooltip_dark`と
`effetune_show_only_inert_tooltip_light`で確認する。本体と共通のbutton/tooltip helperを使い、
点灯はDSP状態、操作は表示・前面化、hideは×という説明の折り返しと明暗コントラストを検査する。
App全体やhostを構築せず、単純な描画fixtureで生成する。2枚の基準PNGは目視確認済み。

画像folder→ZIP (§1.242 G) は `image_folder_zip_{notice_small_light,confirm_small_200_dark,
copying_small_light,result_small_dark}` の代表4枚を保持する。製品と共有する純粋なmodal描画を使い、
1093×614の長い日本語path・確認本文・進捗・結果と固定操作行を検査する。
`image_folder_zip_small_viewport_controls` は案内・準備・先行保存待ち・確認・画像保存・保存確定・
表示反映・成功・失敗の9phase × 2画面（1093×614 / 1366×728）× 100%/200% × Light/Dark、
計72条件で確定/取消/閉じる・タイトル×のviewport内矩形を確認する。全条件へPNGを増やさない。
代表4枚は合計388,195 bytes（約0.37 MiB）。生成後に全4枚を目視確認した。
App、変換worker、製品起動、実IME入力はこの純描画検査に含めない。

§1.482段Dは`tests/ui_snapshot.rs`の`ai_failure_feedback_hover_small_light`の1枚（1093×614）で、
本体と同じtoast actionのhover詳細を確認する。固定分類・エラー番号・画面/処理番号・ページ/寸法・観測日時だけの本文で、
hoverと明示クリックのコピーが同一であること、hoverだけではコピーしないこと、100%/200%の通知矩形が
画面内に収まることをheadless回帰で別に検査する。新しいdialogやviewportを作らず、既存の暗色tooltip helperを使う。
PNGは生成後に目視確認する。libの`ai_failure_feedback_hover_and_explicit_copy`は操作/コピー検査を保持し、PNG比較はintegration側だけが所有する。実ORT/GPU障害や製品起動の証拠とは区別する。

tooltip位置がカーソル画像寸法に依存するfixtureは、実デスクトップの`GetCursorInfo/GetIconInfo`を呼ばない。
integrationの`SNAPSHOT_CURSOR_EXTENT_PHYSICAL`（上0・下34 physical pixels）を明示して本体共通helperへ渡す。
enabledな暗色tooltipには`hover_tip_dark_with_cursor_extent`、disabled tooltipには既存の同等helperを使う。
製品の実カーソル寸法対応は維持する。§1.482の旧lib PNGは実寸/取得不可fallbackの差により
本文が同一でもtooltip全体が16px移動した。基準画像の単純な再生成や許容差拡大で済ませず、外部入力を固定する。

§1.482の前回終了通知は短い要点を常時表示し、「詳細（報告用）」は既定で畳む。
libの`previous_run_notice_{small_light,small_200_dark,observations_small_light}`は畳んだ表示、
`previous_run_notice_checkpoint_small_light`は展開した詳細と固定操作行を検査する。
要点・詳細見出し・操作行はScrollAreaの外、詳細本文だけがスクロールする。
常時表示の要点を優先するため、展開した詳細は要点と見出しを引いた残り高さを使う（本文だけの従来の45%要件とは分ける）。
明暗・小/大画面・100%/200%で折り畳み中の全文コピー、展開後の操作可視性、末尾到達後の操作行固定を検査する。
日時fixtureはUTC+09:00の数値オフセットを注入し、OSのタイムゾーンや現在日時に依存させない。

## ディレクトリ構成

§1.341の両端配置は環境設定`preferences_singleton_spread_placement_setting*`と本別HUDの
`singleton_spread_endpoint_popup_{on,off}_{light,dark}`でON/OFFの実効表示・灰色・説明を保持する。
操作時の保存値非変更とOFF復帰はheadless入力回帰で検査し、PNGを機能証明の代わりにしない。

```
tests/
├── ui_snapshot.rs              # 全スナップショットテストの定義
└── snapshots/                  # 期待スナップショット PNG (コミット対象)
    ├── smoke_label_and_button_light.png
    ├── smoke_label_and_button_dark.png
    ├── susie_diagnostic_*.png    # Susie プラグイン診断 UI
    ├── changelog_markdown_*.png  # 更新履歴 Markdown の整形描画
    └── ...
```

## 設計方針

### 1. 再利用可能な UI 関数として切り出したものを対象にする

`App` を丸ごと構築するスナップショットは書かない (状態の組み合わせ爆発でメンテが
破綻する)。代わりに、以下のような純粋な描画関数を対象にする:

- `ui_susie_diagnostic::render_diagnostic(ui, status, plugins)`
- `changelog_markdown::render(ui, body)` — 更新履歴 Markdown サブセットの整形描画
- 必要に応じて `ui_helpers` などから切り出した整形系関数

UI コードをスナップショットしたい場合は、まずロジックを `fn foo(ui, args) -> ()`
の形に切り出してから、その関数を `test_ui::builder().build_ui(|ui| foo(ui, ...))`
でラップするのが基本パターン。

### 2. ハーネスは `tests/ui_snapshot.rs` 内のヘルパーを使う

- `snapshot_with_theme(name, theme, build_ui)`: 任意 UI + テーマ指定（文字コントラストは標準）
- `snapshot_with_theme_and_contrast(name, theme, contrast, build_ui)`: 任意 UI + テーマ + 文字コントラスト指定
- `snapshot_with_ui_font(name, settings, build_ui)` (Windows): 任意 UI フォント + 縦位置補正指定
- `snapshot_diagnostic(name, status, plugins)`: Susie 診断専用

これらはすべて `install_app_fonts()` を呼んで、本体と同じ `ui_fonts::configure_fonts()`
のフォント fallback (YuGothM / meiryo / msgothic + 記号・絵文字補完) を登録する。
豆腐化したスナップショットは「描画が崩れている」のか「フォントが無いだけ」のか
区別できないため常に登録する。

通常文字・薄い文字・ボタン状態の配色を変える場合は、Light / Dark の標準 snapshot に加えて
`TextContrast::Strong` の Light / Dark snapshot も更新し、背景色まで意図せず変わっていないことを
目視確認する。

UI フォント経路を変更する場合は `custom_ui_font_meiryo_bold_alignment_dark` に加え、
`recommended_ui_font_*_alignment_dark` (BIZ UDPGothic 9pt / Meiryo 10pt /
Meiryo UI 10pt) も更新し、通常文字、ツールバー専用 family、日本語 / CJK / 記号
fallback の縦位置を確認する。9/10pt は 96 DPI の typographic point を egui logical
point へ換算して描画する。同じ snapshot の「動画・音声 HUD」は `x1`、`Norm`、`0.0dB`、
再生時間を `miv-hud-text` で描き、選択フォントを変えても字面・位置が変わらないことを確認する。

Windows の integration / lib UI テストは共通 `tests/support/ui_harness.rs` の builder を使う。
`tests/ui_snapshot.rs` では `test_ui::builder()`、lib では `crate::test_ui::builder()` を入口とし、
raw `egui_kittest::Harness::builder()` / `Harness::new()` へ戻さない。
製品と同じ `DX12 | VULKAN` mask (`src/wgpu_backend_policy.rs`) を要求し、製品が使わない
WGL context の初期化を避ける。`WGPU_BACKEND` による mask 上書きも製品と同様に無効。
CPU adapter 優先 / predictable rendering / lazy 初期化は従来どおりで、各 Harness の
Device / Renderer は独立し、並列性と画像比較は維持する。非 Windows の設定は変更しない。
背景と採取 stack は [GPU snapshot 調査記録](ui-snapshot-gpu-investigation.md) を参照。

### 3. サイズは固定

高さを計算で決めるパネル・popup・メニューでは、`item_spacing`・separator・padding・
Frame の margin を式へ含めるか、該当する間隔を 0 にする。`show_rows` の引数は
spacing を含まない行高であり、Grid が gap 込みの slot を所有する場合は親の縦 spacing を 0 にする。
折り返す本文は固定の行数だけで高さを見積もらない。

`tests/support/scroll_layout.rs` の共通 helper（lib は `crate::test_ui::scroll_layout`、
integration は `test_ui::scroll_layout`）で、十分な高さでは `HeightExpectation::Fits` により
`content_size.y <= inner_rect.height()`（丸め許容は1物理pixel）・offset 0 を検査する。
高さ不足では `HeightExpectation::Scrolls` と、実際の wheel 等の入力後の
`assert_scrolled_to_end` で末尾の到達性を検査する。screen/panel 内の viewport も併せて検査する。
採取値は最終描画 pass の `ScrollAreaOutput` を使い、offset を直接書き換えて成功扱いにしない。
**snapshot は見た目の記録であり、スクロールの要否の検査にはならない。**

デフォルトは 480 × 360 px。理由:

- サイズ可変だと差分検出が過敏になる (描画誤差の累積)
- 小さすぎるとラベルが折り返しで消えて検出不能になる
- 大きすぎると PNG が肥大化してリポジトリが重くなる

起動時ダイアログの小画面回帰は例外として **1093×614 / 1366×728 logical points** を使う。
`startup_dialogs_small_viewport` は App を構築せず、本体と共有する Modal/Window 描画関数を
固定フィクスチャで呼び、本体のUI倍率100%/200%を固定画面へ適用する。
27状態×2サイズ×2倍率の108ケースすべてで、固定操作行の全ボタンとタイトル×を、
viewport/clip内の矩形・hover・mouse clickで検査する。到達性はこのgeometry検査で保証する。
Remote reader再接続、起動時書庫変換の各phase、disabled変換も含む。
共通本文の別の回帰検査で、小/大画面の100%/200%における長文の本文高（viewport高の45%以上）と
短文の自然高への縮小も確認する。操作ボタンだけ可視でも、本文が不必要に小さければ失敗する。
幅200ptへ縮めたWindowの長い操作ボタンと復元一覧の説明の折り返しも、100%/200%で
固定操作のviewport/clipを検査する。本文高は縮小後の実割当高を測り、操作行を含めない。
PNGは初回設定/書庫変換確認の1093×614・100%/200%の代表4枚と、
意図的に更新した変更点の100%・2サイズの2枚だけを保持する。
ほかの状態/サイズ/倍率にはsnapshot呼び出しもbaselineも増やさず、geometry検査を維持する。
対象と除外を [起動時画面の監査](startup-dialog-small-screen.md) に列挙する。
長いエラー文・ネットワークパス・全バージョンの累積告知を含める。
初回設定のクリック到達性と IME-safe Enter / Esc は `--lib first_setup` の回帰テストで検査する。
表示後の縮小/拡大は、同じContextを1920×1440→1093×614→1920×1440→1093×614と変更し、
全27状態の固定操作とタイトル×を最大2frame後に検査する。初回設定は本体フォントで見出しも
検査し、最終layout pass内でResponseを採取する。Context::run終了後のread_responseはdiscard
されたpassのwidgetを優先し得るため、過去passの矩形を現在の描画として検査しない。
通常の2passでは縮小したframe内に収まり、pass予算1の検査は強制frameを足さずrepaint scheduler
だけで再配置/hover/clickへ到達することを確認する。追加PNGは作らず代表6枚の比較を維持する。
実アプリや認定端末での観測とは区別する。ダイアログの設計要件は CLAUDE.md の
「ダイアログ (egui::Window)」節を参照。

### 4. 文字列や色のリテラルはフィクスチャに集約

複数テストで共通するプラグイン情報などは `*_fixture()` ヘルパー関数に切り出す。
同じ入力・別テーマのようなパターンで一貫性を保つ。

## 新しいスナップショットテストを追加する手順

1. 対象の UI コードが `fn(ui: &mut egui::Ui, ...)` の形になっていることを確認。
   そうでなければ先に `ui_*` モジュールに切り出す (ui_susie_diagnostic.rs を参考に)。
2. `tests/ui_snapshot.rs` に新しい `#[test]` を追加。
3. `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` で PNG を生成。
4. 生成された PNG を目視で確認 (想定通りの描画か)。
5. PNG と `.rs` の変更を一緒にコミット。

## リポジトリサイズ管理

PNG の容量は画面サイズや文字量によって変わるため、追加時に枚数と合計容量を確認する。
画面サイズ・倍率・状態の組合せはgeometry/操作の検査を中心にし、PNGは見た目を確認する
少数の代表画面へ絞る。起動時ダイアログは上記6枚を保持し、108条件へPNGを増やさない。
肥大化してきた場合:

- サイズを縮小 (例: 480×240 に変更)
- 冗長なテーマ差し替え分の統合 (Light / Dark のうち代表 1 枚のみ残す)
- git LFS 化 (最終手段)

## CI での扱い

現時点では CI 環境を持たないためローカル実行のみ。将来 GitHub Actions 等で
自動化する場合、Windows runner が必要 (本体と同じ Windows フォント fallback を登録するため
`C:\Windows\Fonts\*.ttc` を参照している)。Linux runner に移行する際は、
CJK フォントを自前で vendored するか `Noto Sans CJK` をインストールする必要がある。

## 既知の制限

- **フルスクリーンビューポート** (eframe の `show_viewport_immediate`) の単体
  スナップショットは現状対応外。メインビューポートと別管理のためハーネスで
  扱いづらい。必要になったら `Harness` の `run_steps` を複数サイクル回す方式で
  試す。
- **実ファイルの読み込みを伴う画像 UI** (グリッドセル等) は、この純描画 harness の対象外。
  読み込みを行わず固定の生成 texture を渡す純描画は検証できる。別バージョンの一時表示は
  `src/ui_fullscreen.rs::tests::similar_preview_navigator_snapshot_dark` で geometry と navigator
  paint を共有し、本文の clip と可視範囲枠を確認する。App・worker・native GPU の検証とは区別する。

## integration test と lib unit test の使い分け

`tests/ui_snapshot.rs` は外部の integration test なので、lib crate が公開する
`mimageviewer::*` の pub API だけを利用できる。現在の `src/lib.rs` は実際の `app`
module を含み、実行ファイルは薄い入口である。ただし `app` module は非公開で、
`App::draw_local_adjust_*` のような `pub(crate)` 関数は integration test から直接呼べない。

crate 内部の純粋な UI 描画をスナップショットしたいときは、**lib unit test
(`src/<module>.rs` 内 `#[cfg(test)] mod`) の中で `crate::test_ui::builder()` を使う**。
これは App 全体を構築する snapshot を許可するものではなく、対象は上記の純粋な描画関数とする:

```rust
// src/ui_adjustment_panel.rs::local_adjust_segmentation_tests 内
#[test]
fn local_adjust_panel_snapshot_empty_layer_list() {
    let mut fonts_ready = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(280.0, 200.0))
        .build(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                super::draw_local_adjust_layer_list(ui, /* args... */);
            });
        });
    harness.run();
    harness.snapshot("local_adjust_panel_empty_layer_list");
}
```

スナップショットは **同じ `tests/snapshots/<name>.png`** に保存される (integration test と
lib unit test でディレクトリ共有)。**`UPDATE_SNAPSHOTS=1`** も同じ:

```bash
UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib local_adjust_panel_snapshot
```

実例: `src/ui_adjustment_panel.rs::local_adjust_segmentation_tests::local_adjust_panel_snapshot_*`
(5 件、補正レイヤーパネル + レイヤーリスト UI)。


§1.402 の通知時間設定は `notification_duration_preferences_light_standard`（480×115）と
`notification_duration_preferences_dark_short_narrow`（260×145）で、本体と共通の全体設定helperを描く。
3択の選択状態・狭幅の補足文の折り返しを確認する。App・実製品の起動は含めない。
2枚は計35,683 bytes。生成画像を目視確認し、更新モードを外した通常比較も2件成功した。

§1.683 の本棚管理は、App を構築せず製品と共通の Window / 本文 renderer を描く。
既存 `book_manager_numbered_dark`（900×600、名前順6冊）を更新し、
`book_manager_manual_light`（1093×614、手動順21冊）、
`book_manager_saving_small_dark`（547×307 PNG、1093×614・200%相当の546.5×307論理領域）、
`book_manager_unsaved_light`（700×460、1冊）、
`book_manager_readonly_dark`（700×460、順序読取異常・名前順6冊）を追加した。
各行の通常サイズ↑↓と端の無効化、方式ボタン、保存中/未保存/読取警告の明暗、
短い状態表示の固定枠、名前欄と右端scrollbarの非重複を全5枚で目視確認した。
合計428,180 bytes（約0.41 MiB）。製品起動・実IME・実保存をこのPNGの証拠にはしない。

`saved_group_number_ui_tests` の標準geometryは2画面×実効倍率2通り×5状態×5冊数の100条件で、
実wheelによる末尾到達・最終矢印と方式ボタンのclip/viewport内配置・可視行だけの描画を検査する。
21冊/255文字名/深いrootを使う6条件はnative pixels-per-point 1.25/1.5/2.0を実際に設定し、
546.5×307と折り返す350×460で物理pixelとGUI rounding後の行間隔・末尾到達を検査する。
各行は宣言したslotを先に予約し、自然な本文の上限と隣接行の実stepを別に検査する。
番号は先頭20冊、pinは保存なし/Manual/保存済みNameのresolver結果の部分列として検査する。
実Enterで同じ本をviewport外へ18回移動し、最初の保存で最後の可視行を動かす境界も検査する。
これらは状態表示のために本文の位置を変えず、組込scroll_toを即時適用してfocusを維持する回帰である。
閉じた管理の保存中再表示は共有描画の実pointer clickで検査する。
integrationの `book_manager_window_close_and_mode_controls_in_small_viewport` は12条件で×と方式ボタンを検査し、
geometryの全組合せへPNGを増やさない。
更新モードを外した `ui_snapshot` 全224件の通常比較も成功した。
