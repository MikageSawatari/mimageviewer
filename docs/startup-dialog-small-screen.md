# 起動時ダイアログの小画面対応 (2026-10-06)

対象: `next-first-setup-fit`。認定端末の原因は未確認。Store の仮説とサブPC Sandbox の
観測の出典は [backlog §1.241](next-release-backlog.md) を参照。

## 不変条件と修正

OS DPI基準の1093×614 / 1366×728相当の画面を100%/200%のUI表示倍率で使っても、
開始/閉じる/終了の操作行を viewport 内に残す。200%ではeguiの利用可能領域は
546.5×307 / 683×364 pointsに縮む。フォントの大きさも本体のUI表示倍率に従う。
初回設定の旧 Modal は本文高を制限せず、背面入力を停止したまま開始ボタンを押し出し得た。
本文/選択肢だけを縦スクロールにし、見出しと操作行を本文の外に置く。

`ui_dialogs::startup_dialog_scroll_body` は `content_rect()` の高さから上下16ptの画面余白、
実際のFrame余白、描画済み見出し高、操作行の予算を引く。操作行はボタンのfont/paddingと幅から
折り返し段数と単独の長いボタンの複数行高を見積もる。復元一覧の説明はBody font/実幅で測定する。
各画面の明示的な間隔/区切り（退避中は状態表示）も予算へ含める。
一律128ptや本文700pt等の上限は使わない。中央Modalは高さ基準で計算し、位置との循環を防ぐ。
Windowは現在の本文開始位置と下端余白、Resize ownerの `available_height()` も上限とする。
初期のResize高さはviewportから初期位置/枠用の80ptを引いた値とし、短文は内容に合わせて縮む。
ユーザーの縦リサイズを維持する。親のmax_rectを明示確保しつつ、縦auto_shrinkを有効にし、
短文は自然高、はみ出す本文は算出上限まで広げる。スクロール最小高で余分な空白を作らない。
横 auto_shrink を止め、floating scrollbar の幅を予約し、本文上の wheel の raw/smooth/event を
処理後に消費する。新しい pending state、worker、同期I/O、起動順変更は追加しない。
復元表は単一の縦横 ScrollArea が表とエラーを所有し、既存の縦/横ドラッグを維持する。

初回設定の Enter は既存の `dialog_enter_pressed` を使い、IME 変換中は確定しない。
選択肢/リンクへのフォーカス中はその widget の Enter 操作を優先する。
Esc/背景クリックでセットアップを省略しない。選択肢・既定値・保存/AI反映経路を維持する。
新しい非同期制御を増やす必要はなく、既存 Modal の本文レイアウトだけで解決できる。

## 起動時/初回設定直後の監査

| 画面/経路 | 判断と対応 |
| --- | --- |
| 初回設定 | 高さ無制限。見出し/開始を固定し、選択肢をスクロール化。幅もviewportに制限 |
| 起動時の書庫変換 | `startup_ops::open_default_startup_target` の前回RAR/7z/LZHにキャッシュが無い場合も表示。長い入力名/エラーが操作を押し出す構造を修正。列挙中/確認/変換中/エラーの本文を制限、変換/取消/閉じるを固定。画像なしの無効な変換操作も維持。手動の同名ZIP保存も同じ描画で検査 |
| 重要な変更点/バージョンまたぎ告知 | 既存320pt本文と固定操作行で614ptには収まる。さらにviewport/Windowリサイズへ追従 |
| EffeTune の一度だけの告知 | 独立画面は無く `version_highlights` に含まれる。同じ経路で対応。portableの非表示契約を維持 |
| 更新通知/更新チェック結果 | 自動チェックはバッジのみ。手動/バッジから開くWindowの長いエラー/更新本文を収め、操作行を固定 |
| 設定読み込み不可/保護起動 | 非互換/読込失敗の2文面を点検。本文を制限、復元/終了を固定 |
| 設定復元一覧 | 保護起動案内から到達。旧一覧外のエラー文が無制限。一覧/エラーを共通本文へ。表の横スクロールと復元操作を維持、完全リセットは本文外 |
| 復元/完全リセット確認 | 短文でも200%を検査。本文を制限、確定/キャンセルを固定し、タイトル×を含め到達性を確認 |
| 復元/リセット結果・Remote reader再接続エラー | ファイル名列挙/長文エラーが唯一の終了/閉じる行を押し出せる。本文制限と固定操作行、状態遷移は維持 |
| 復元処理中/reader再接続中 | スピナーと固定1行、操作ボタンを持たない既存待機表示。変更なし |
| ネットワーク data_dir 案内 | 初回設定等の後に表示。長いUNCパス/本文を制限、閉じる/今後非表示を固定。200%でタイトル×が右端に溢れる幅指定もviewportへ制限 |
| PDF/Susie/TensorRT 通知 | 前回の場所や自動AIの準備で起動直後にも出得る。OSエラー/ログパスを制限、閉じる/既存TRT再起動を固定 |
| マウス戻る/進むの移行案内 | 起動時判定あり。本文を制限し、標準/従来の2操作を固定。Windowはdefault_posで移動可能。200%とタイトル×も検査 |
| 名前変更の復旧記録 | 起動時journal再開から到達。本文と幅をviewportに制限、復旧3操作/退避中の取消を固定。200%の両状態を検査 |
| タスクトレイ常駐案内 | 起動時フラグはfalse、設定操作から表示。固定短文とOK/タイトル×を点検、変更なし |
| PDF/書庫パスワード | 利用者の指定により今回の起動時監査対象から除外。変更なし |

## 自動検証とスナップショット

### Sandbox での縮小後表示に対する追加修正 (`next-first-setup-fit2`)

利用者のサブPC Windows Sandbox の報告と `target/smallwin/` の画像では、旧版の初期窓は
モニター作業領域の下端を超え、前回修正版も表示後に SetWindowPos で縮小すると初回設定の
見出しと開始が切れた。前回の静的な小画面 harness と、製品での観測は別の証拠である。

egui 0.33 の Area は前フレームの寸法で中央位置を決めてから本文を描く。本文の上限を
現在の content_rect から計算しても、そのフレームの描画位置は古い寸法に基づく。
Area::end は新しい寸法を保存するだけで、通常の寸法変更には再配置を要求しない。
この追加修正ではPNGの更新・追加・削除は0。安定した小画面のレイアウトは同じで、
retained-contextの縮小/拡大/再縮小をgeometry/操作到達性の検査に追加する。
起動時 Modal の共通描画 owner が測定前後の寸法を比較し、変化したときだけ
request_discard で同一フレームを再レイアウトする。pass予算を使い切った場合だけ同じ寸法変更を
通常のrepaint schedulerへ渡す。Area ID、フォーカス、本文の
スクロール状態を作り直さず、静的な小画面と縮小・拡大の両方を扱う。
Window は既存 Resize owner と共通本文の上限計算を維持する。

可視領域の不足は root の初期 geometry で直す。ダイアログだけをモニターに clamp すると、
画面外のメインUIと normal restore 矩形を残し、各画面へ native 座標の責務が広がる。
creatorで最初のegui入力/paintより前に、実 HWND の作業領域・DPI・装飾幅を使って
SetWindowPosでclientサイズとouter位置を収める。表示/非表示/フォーカス/最大化は変更しない。
WM_SIZEはwinitのResumed中にbufferされ、AboutToWaitのBootstrap paintより先にeframeの
surfaceサイズを更新する。egui入力も実client寸法を読む。元の希望通常サイズを保持し、
初回updateのviewport再適用でも同じ作業領域計算を使う。
作業領域が既定の最小 client サイズより小さい場合は native の最小値も
収まる値へ制限する。大画面で収まるサイズ/位置は維持し、UI倍率とは独立して計算する。
既存の first paint/show 後の viewport command 適用、および visible commit 後に一度だけ
最大化する順序は変更しない。異常なSTARTUPINFO/外部ownerによるcreatorより前のearly showは
従来の§1.327契約と同様に対象外。native補正の実適用結果はログへ記録する。

長い処理の状態や専用の resize pending を追加する案は採用しない。本文/操作行は既存の
描画を維持し、寸法を所有する描画経路と、窓の初期 geometry owner でそれぞれ閉じる。
ユーザーが起動完了後に意図的に画面外へ移動する一般的な窓配置を継続的に強制補正はしない。

Sandbox スクリプトは core プロセスの可視・非ownedトップレベル窓を列挙し、非空タイトルを
持つ process main window、なければ最大面積の窓を選ぶ。起動待ち後に再選択し、縮小から
5秒後に撮影する。本セッションの権限制限で外部原本を書けないため、修正版は
`target/auto-smallwin.ps1` に引き渡す。スクリプト/製品の実行はしていない。

追加のfull gateで、既存の `rename_migration_waits_for_book_bookmark_service_fifo` が
fixture生成直後のidle assertionで失敗した。constructorはedit-previewのPruneと
book-resumeのread_allを非同期投入し、busy判定は両方を含むため、直後のidleは時間依存だった。
対象のbookmark/local-adjust pending fieldを検査する2件だけ、対象外の起動workerを外す
専用fixtureへ変更した。共有fixtureと製品のwriter/busy判定、各testのbusy assertionsは維持する。
今回のcreator/update/modalはこのfixture経路で呼ばれない。独立reviewerが原因とdrop境界を確認した。

追加修正の最終検証 (2026-10-07): normal/portable check、全ui_snapshot 97件、
fmt/diff check、glyph lint、full lib 10810件 (52 ignored) と `test-full.ps1` が成功。
full gateの初回並列compileはWindowsのpaging file/mmap 1455で失敗したため、
以後はコマンドprocessだけ `CARGO_BUILD_JOBS=1` にして実行した。
`build-dev.ps1` と `prepare-portable-smoke.ps1` も成功。通常profile coreとcompanion、
fresh dataのportable-smokeを用意したが、製品は起動していない。コミットせず、
検証結果・外部スクリプトの反映方法・実機確認手順を `target/fs2-msg.txt` に記録した。

`tests/ui_snapshot.rs::startup_dialogs_small_viewport` は本体と共有する純粋な描画関数を呼ぶ。
App/設定DB/worker/ネットワークは起動しない。実アプリを起動した証拠とは区別する。
native ppp=1の固定画面に、本体の `settings::apply_ui_scale_factor(ctx, 2.0)` を適用する。
`content_rect().size() * scale` が元の画面サイズと一致することも検査し、画面自体を拡大して
200%を相殺する検査にはしない。独立したフォントサイズ設定は無いので、同じ倍率で文字も拡大する。
AccessKitの矩形は画面pixels、eguiのResponse/入力はlogical pointsなので、前者は固定画面、
後者はcontent_rectに対して検査し、pointer座標もResponseの中心を使う。
27種類×2画面×2倍率=108ケースはPNGの有無に関係なく実行する。到達性はgeometry検査で保証する。
次の表の**固定操作行の全ボタン**と、存在するタイトルバーの
`Close window`（×）を検査する。viewport内の矩形、clip内の全操作矩形、実pointer hover、
mouse press/releaseのclickをそのフレームで記録して確認する。画像なし書庫の変換ボタンは
disabledのまま表示範囲内にあり、clickを発行しないことも検査する。
選択肢/checkbox/本文リンク/タブと、スクロール内の復元表の各「この時点に戻す…」は固定操作行の
検査に含めない。復元表は既存の縦横ドラッグ回帰で確認する。処理中の操作なし表示と、起動時に
表示されないトレイ告知はsource inspectionのみ。パスワードは依頼どおり対象外。
bootは2文面、更新通知は長いエラーと更新本文の併存、networkは長いUNCパス、restore/workerは
長文を使う。前回コミット `a77fcc5d2` での既存snapshot変更は `network_data_dir_notice_dark.png` のみ。
480×360の旧画像では本文とリンクが全高を使って閉じる行が下に溢れていた。
新画像は本文をスクロール化し、閉じる/今後非表示を固定したため、本文の下半分が初期表示から
スクロール先へ移り、右側の予約幅にバーが現れる。本文やリンク自体は削除していない。

PNGは見た目を確認する代表画面に限定する。最終的に次の6枚のみを保持する。
初回設定/書庫変換確認の1093×614・100%/200%の4枚と、意図的に更新した変更点の2枚である。
前回 `a77fcc5d2` の起動時PNG22枚は、初回設定1093×614と変更点2枚を残して19枚削除する。
1366×728の初回設定を含む削除対象も、108ケースのgeometry/操作検査は維持する。
P2で一度生成した新規86枚は3枚を残して83枚削除し、該当snapshot呼び出しも停止する。

| 最終PNG | 見た目の確認目的 |
| --- | --- |
| `startup_first_setup_1093x614.png` | 100%で見出し/開始が見え、選択肢本文にスクロールバーがある |
| `startup_first_setup_1093x614_ui200.png` | 200%でも見出し/開始が見え、本文が画面の残りの高さを使う |
| `startup_archive_confirm_1093x614.png` | 100%で長い入力名を本文に収め、変換/取消を固定する |
| `startup_archive_confirm_1093x614_ui200.png` | 200%でも長い本文と独立した変換/取消行が見える |
| `startup_whats_new_1093x614.png` | 100%の告知本文と変更履歴/閉じる行、安定後の幅/折り返し |
| `startup_whats_new_1366x728.png` | 大きい側の100%でも同じ操作行と安定後の幅/折り返し |

本文高の追加修正では代表6枚を更新する。初回設定の両倍率は本文の固定上限と一律の操作行予算を
外し、残りの高さへ広がるため選択肢の初期表示範囲が増える。書庫確認の両倍率も同じ理由で、
長い入力名の初期表示範囲が増える。変更点の両サイズは従来の320pt上限を外し、告知の表示範囲と
Windowの高さが画面に追従する。いずれも内容と固定操作行を維持する。
先のP2修正で変更点2枚に生じた幅/折り返しの差は、Areaの幅が安定する12フレーム後まで待つため。
既存の `network_data_dir_notice_dark.png` も480×360の残り高さを使うよう更新し、
スクロール先にあった「おすすめ」の説明が初期表示に入る。これは代表6枚への追加PNGではない。

次のfixture一覧はgeometry検査の対象であり、すべての画面のPNGを保存する一覧ではない。

| geometry fixture | 意図した表示・検査対象 |
| --- | --- |
| `first_setup` | 見出しと開始が固定。全選択肢は縦スクロールで到達 |
| `boot_incompatible` | 非互換設定の説明を本文に収め、設定復元/終了を固定 |
| `boot_unreadable` | 読込失敗の説明を本文に収め、設定復元/終了を固定 |
| `whats_new` | EffeTuneを含む版またぎの長い告知をスクロール、すべての変更/閉じるを固定 |
| `update_notice` | 長い接続エラーと更新情報が共存してもリリース/通知オフ/閉じるを固定 |
| `update_current` / `update_error` | 最新版確認済み/エラーのみでもリリース/閉じるの両方を検査 |
| `network_data_dir` | 長いUNCパスを折り返してスクロール、閉じる/今後非表示を固定 |
| `restore_list` | 10世代の表と長い読込エラーを同じ本文に収め、完全リセットを固定 |
| `restore_result` | 長い終端エラーをスクロール、唯一の終了操作を固定 |
| `restore_success` / `restore_recoverable` | 成功の終了/回復可能エラーの閉じるをそれぞれ検査 |
| `restore_remote` | Remote reader再接続失敗の長文を収め、再接続/終了の両方を固定・検査 |
| `restore_confirm` / `restore_reset` | 復元/完全リセットの確定と取消をそれぞれ検査 |
| `pdf_notice` | 長い準備失敗本文をスクロール、閉じるを固定 |
| `susie_notice` | 長い準備失敗本文をスクロール、閉じるを固定 |
| `trt_notice` | 長い準備失敗本文をスクロール、既存の再起動/閉じるを固定 |
| `mouse_migration` | 標準/従来どおりの両操作と×を検査 |
| `rename_recovery` / `rename_quarantining` | 通常の再読込/退避/閉じると退避中の取消をそれぞれ検査 |
| `archive_scanning` / `archive_converting` | 長い入力名をスクロール、列挙/変換中の取消と×を固定・検査 |
| `archive_confirm` / `archive_empty` / `archive_sibling` | 通常変換/画像なし/同名ZIP保存の確認で変換と取消を検査。画像なしの変換は無効を維持 |
| `archive_error` | 長い入力名とエラーをスクロール、閉じると×を固定・検査 |

初回設定のunit/handler testは小画面の開始ボタン矩形とクリック到達性、既定値のEnter確定、
開始ボタンへのフォーカス中も含むIME中のEnter/Space抑止、Escで省略しないことを検査する。
共通本文のWindow高さ変更、wheelが届いたフレームでの消費、復元表の縦/横ドラッグも検査する。
幅200ptのWindowで、長いボタンと実際の復元一覧の折り返し説明/完全リセット操作について、
100%/200%のviewport/clipを検査し、復元操作のpointer hoverも確認する。
共通本文の長文/短文を1093×614と1920×1440、100%/200%で検査し、長文の本文高がviewport高の
45%以上になること、短文の実割当高はcontent高に縮むことを確認する。108ケースの操作到達性と併せて、
開始だけ見えて本文が1行しか使えない退行も検出する。
同じ小/大画面と倍率で、短いWindow全体も110pt未満の自然高へ縮むことを確認する。
製品・認定端末は起動していない。
初回の検証記録は `target/firstsetup-msg.txt`、Codex P2 対応の検証/画像差分/ビルド引き渡しは
`target/firstsetup-msg-2.txt` に記録する。masterの `978074c21` は設定分類件数テストを修正済みだが、
このworktreeから.gitを書けないため未統合。今回コミットせず、ClaudeCodeが後でmasterをマージする。
