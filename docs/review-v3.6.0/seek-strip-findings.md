# v3.6.0 出荷前レビュー — 静止画シークバー / サムネイル列

実施: 2026-09-06 / Codex `gpt-6-astra` (reasoning=xhigh, sandbox=read-only)
対象: master `15d88a0d4`

依頼した観点と経緯は [brief.md](brief.md)。以下はレビューの回答全文。

---

**現状のままの出荷は保留を勧めます。** 特に下記 F1・F2 は、今回繰り返し問題になった「状態の所有と寿命」の穴が残っています。

対象は `master` の `15d88a0d4`。コード・履歴・仕様・テストを読み、幾何については到達可能な160通りを式から補助計算しました。**実機確認、テスト再実行、性能計測はしていません。** ファイル変更はなく、指定の未追跡ファイル・生成物にも触れていません。

**出荷前に解消すべき問題**

**F1［P1］ページ着地が、そのフレームで発行した列・プレビューの要求を取り消せる**

通常の単ページ／見開きシークでは、次の順で処理されます。

1. overlay が列とプレビューのページを exact set に登録する。
2. 返した `jump_to` を、同じフレームの後段で着地させる。
3. `open_fullscreen_with_materialization_and_contract` が **exact set を空にする**。
4. 遠隔ページの worker は、次の取消チェックで要求を不要と判定できる。

根拠は [要求の登録](C:/home/mimageviewer/src/ui_fullscreen.rs:17290)、[描画後のナビゲーション](C:/home/mimageviewer/src/ui_fullscreen.rs:20410)、[着地時の clear](C:/home/mimageviewer/src/app.rs:45677)、[worker の判定](C:/home/mimageviewer/src/thumb_loader.rs:41) です。

例えば `keep_range=2..8`、プレビュー要求 `idx=5000` は、登録直後は許可され、着地後は同じ要求が不許可になります。ZIP/PDF は I/O 後にも再確認するため、既に始めた仕事も取り消せます。

**集合の消失はコード上確定。表示停滞・再要求の頻度は worker の実行タイミング次第で未測定です。** 連結読みの着地は別経路なので、この clear を通りません。同じシークでも構成によって寿命が違います。

方向性は、同一コンテナ内の着地に全要求の終了を持たせず、列・プレビューの現在の要求を所有する側で更新することです。ジェスチャを残した修正の隣に、要求側の二重終了が残っています。

**F2［P1］`StillSeekGesture` が viewer context に属していない**

`fs_seek_gesture` は [App のフィールド](C:/home/mimageviewer/src/app.rs:12492) ですが、`ViewerContextBundle` にありません。隣の `fs_seek_drag_active` と `fs_seek_overlay_visible` は [context 交換対象](C:/home/mimageviewer/src/app/viewer_context_registry.rs:2047) です。

そのため、窓 A で確定した列中心が、窓 B を mount しても App に残ります。

- A・B の現在 source position が同じなら、B が A の列中心を引き継ぐ。
- 異なるなら、B の `recenter_if_page_changed` が A の状態を消す。
- B で別の列中心を確定しても、A 用の状態を復元する場所がない。

[再中心化の比較対象](C:/home/mimageviewer/src/ui_fullscreen.rs:3604) はページ位置だけで、context identity を識別できません。**交換漏れは確定、実窓での再現は未実施**です。detached リワークの分類では BA-7 の所有状態分散に該当します。

修正方向は、ジェスチャと確定済み中心を owning context に持たせ、交換・中断・close をそこで完結させることです。

**次版対応でもよい、限定条件の問題**

**F3［P2］release フレームの最終移動を取りこぼす**

横シークが既に `Track(Scrub)` になっている状態で、「最後の移動＋release」が同じフレームに届くと、最後の位置へシークしません。

[ハンドラ](C:/home/mimageviewer/src/ui_fullscreen.rs:3645) は `dragged()` の間だけ Seek を返し、[release 時](C:/home/mimageviewer/src/ui_fullscreen.rs:3669) に Seek するのは `Undecided` だけです。egui 0.33.3 の実装も確認しましたが、release フレームは `drag_stopped=true`、`dragged=false` になります。

同様に、列の下方向 close も [dragged のみで判定](C:/home/mimageviewer/src/ui_fullscreen.rs:3720) するため、最後の移動で境界を越えて離すと閉じません。動画側にも同型の判定があります。

既存テストは直前の移動と同じ座標で release しており、この条件を見ていません。

**F4［P2］通常バーを隠すと、全画面で下ドラッグ close に到達できない**

条件は「列表示」「通常バー非表示」「画面下方へカーソルが抜けられないモニター配置」です。固定状態・読み方向・高さには依存しません。

通常バー高が 0 になると、[列の下端](C:/home/mimageviewer/src/ui_fullscreen.rs:3252) は画面下端そのものになります。一方、[close の共有関数](C:/home/mimageviewer/src/video/seek_strip.rs:1179) は `pointer.y >= strip_bottom` を要求します。

100%・高さ1080の全画面なら、閾値は1080、画面内の最下端カーソルは1079です。動画は通常シーク行を隠しても40ptのコントロール行が残るため、この問題になりません。

フィルムボタン／Shift+S で閉じることはできますが、要求された操作が一つ失われています。

**F5［P2・性能懸念］列の描画から同期 SQLite 読み出しへ到達する**

[毎フレームの回転取得](C:/home/mimageviewer/src/ui_fullscreen.rs:16914) は、キャッシュ未読時に [get_rotations_for_indices](C:/home/mimageviewer/src/app.rs:49425) → [RotationDb::get_many](C:/home/mimageviewer/src/rotation_db.rs:144) → SQL 実行へ到達します。

件数は列幅で制限されていますが、worker 化されていません。新しい範囲へ列を送る場合や回転キャッシュ失効後が該当します。列の開閉・固定操作にも同期 `settings.save()` があります。

**同期到達は確定、フレーム時間への悪影響は未測定**です。「UI スレッドで DB アクセスなし」とは判定できません。回転値を非同期で準備し、描画をメモリ参照に限定する方向が妥当です。

**F6［P2］プレビューだけ保存回転を無視する**

列は回転後寸法と回転 Mesh を使いますが、プレビューには [TextureHandle だけを渡し](C:/home/mimageviewer/src/ui_fullscreen.rs:17300)、[回転なしの painter.image](C:/home/mimageviewer/src/ui_fullscreen.rs:4012) で描きます。

保存回転90°／270°の画像では、本体と列が縦向きでもプレビューだけ横向きになります。180°も上下が一致しません。通常フォルダ・ZIP・PDF・変換アーカイブに共通です。

**F7［P2］画像・動画混在フォルダで、描画しない列の高さを確保する**

列表示設定を ON にしたまま混在フォルダの画像を開くと、[geometry の許可判定](C:/home/mimageviewer/src/ui_fullscreen.rs:16319) は現在項目が画像であることまでしか見ず、列高を含めます。

その後の [全ナビ対象が画像かという判定](C:/home/mimageviewer/src/ui_fullscreen.rs:16832) でサムネイル列を描かず、件数サマリーへ戻ります。`BarAndStrip` なら、実際には列がないのにその高さまで画像フィット領域から除かれます。

動画側で分けた「何を表示しているか」という述語が、静止画側では geometry と内容描画で一致していません。

**F8［P2］通常バー非表示＋固定で、常時ページ番号も消える**

`fullscreen_page_number_overlay=true` でも、`BarOnly`／`BarAndStrip` なら [固定フラグだけで右下番号を抑止](C:/home/mimageviewer/src/ui_fullscreen.rs:16562) します。通常バー非表示時には、代わりとなるバーの番号も描かれません。

特にプレビューを「常時非表示」にすると、現在ページ／総数がどこにも出ません。「同じ情報の重複を避ける」条件に、実際の通常バー表示を含める必要があります。

**軽微な状態不整合**

**F9［P3］列を隠している間のページ往復を検出できない**

「ページ A で列を別位置へ送る → Shift+S で隠す → B へ移動 → A へ戻る → 列を再表示」で、古い列中心が復活します。

[再中心化は列を描く場合だけ](C:/home/mimageviewer/src/ui_fullscreen.rs:16892) 呼ばれ、比較するのは現在位置と確定時位置だけです。「実際のページが一度でも変わったら戻る」という仕様に対して、A→B→A の途中を観測できません。

**動画との一致・仕様判断**

| 観点 | 判断 |
|---|---|
| 横ドラッグの向き | 現在の静止画ハンドラの RTL 反転は正しい。バー方向設定とも分離されている |
| 横ドラッグの量 | **一致していないが、現行文書には明記済み**。静止画は可変幅セルなのに動画の固定幅で割り、整数中心へ丸める |
| release の意味 | 動画は release で seek・再生、静止画は列中心だけ確定。今回の仕様に沿った意図的な差 |
| 通常バー高 | 静止画38→0pt、動画64→40ptは仕様どおり。ただし F4・F8 の付随条件を取りこぼしている |
| 上ドラッグ初動 | 静止画は押下位置を逆算するが、動画は `drag_started` 時点の位置を原点にする。共有関数でも初動は同一ではない |

ドラッグ量は、例えば「大」で縦横比2:3なら実セル間隔が約66.7ptですが、1ページ送るための計算上の幅は152ptです。3:1なら実間隔286ptでも計算幅は152ptです。**「動画と同じ式」は満たしていても、「指と同じ距離だけ帯が動く」は満たしません。** 文書どおりなので実装違反とはせず、操作仕様として判断すべき項目です。

分割表示も source page 単位のシークで、左右半分を target に持ちません。プレビューも全ページです。「着地後に見える半分まで予告する」要求なら未対応ですが、その要求が確定しているとは読み取れないため、不具合として断定していません。

**問題を認めなかった範囲と根拠**

- **固定状態と4入口の設定更新**：ボタン・ドラッグ・Shift+S・環境設定は同じ設定 setter へ収束しています。`None → BarOnly → BarAndStrip`、バー解除、列 close 時の `BarOnly` への遷移は到達可能です。`(false,true)` の読み込み正規化もあります。[設定の遷移](C:/home/mimageviewer/src/settings.rs:7333)
- **列のレイアウト数・要求数**：セル幅下限、左右の停止条件、外側2枚ずつの要求により、ページ総数ではなく画面幅で制限されます。未ロードは成長を止めても要求範囲に残り、Failed は後続を妨げません。[レイアウト](C:/home/mimageviewer/src/ui_fullscreen.rs:3426)
- **keep_range の拡張**：列・プレビューを bounding box に含めず、exact set として保持する実装を確認しました。[keep_range 算出](C:/home/mimageviewer/src/app.rs:34436) F1 はこの設計自体ではなく、集合の終了経路の問題です。
- **先読み gate と優先度**：両キューで、列対象を投機的先読みの削除対象から外し、既存要求も priority へ昇格します。[通常キュー](C:/home/mimageviewer/src/app.rs:34755)、[heavy キュー](C:/home/mimageviewer/src/app.rs:34793)
- **コンテナ別の要求生成**：Image／ZipImage／PdfPage は既存 worker 用の要求を組み立てます。この分岐には画像読み出し・デコードがありません。変換済みアーカイブ内部も ZipImage 経路です。[要求生成](C:/home/mimageviewer/src/app.rs:72542)
- **0件・1件・端の処理**：空列の早期 return、1件時の除算回避、source position の clamp を確認しました。対象の純関数に範囲外アクセスの問題は認めませんでした。
- **動画の2述語**：波形は通常シーク行非表示の対象になり、サムネイル表示中だけプレビューを隠す設定の対象にはなりません。tile 中は両方 false。呼び出し側も使い分けています。[述語と geometry](C:/home/mimageviewer/src/video/native_presenter/render_core.rs:204)

なお、**overlay 全体がページ数に対して定数時間というわけではありません**。見開きでは unit 検索、`pages_on_screen_with`、nav の比較に全体走査が残ります。キャッシュで再構築は減っていますが、数万ページ時のフレーム時間については性能計測が必要です。

**追加すべき回帰テスト**

現在のテストには改善がありますが、まだ production のつながりを十分には固定していません。

| 固定すべき不変条件 | 現在の穴 |
|---|---|
| overlay→実着地→worker取消判定→次フレームを通して、必要な要求とジェスチャが残る | 着地テストはジェスチャを確認するだけ。exact set のテストは真偽値を直接渡す純関数テスト |
| 最終移動と release が同じフレームでも、最終位置へ着地／closeする | release 座標が直前の移動と同じ |
| 実 geometry で、通常バー Show/Hide・高さ4段階の close が可能 | strip ハーネスは下に余白のある固定矩形 |
| A/B context を交換・closeしても他方の中心が変わらない | ジェスチャの context 分離テストがない |
| 列非表示中の A→B→A でも、仕様どおり中心が戻る | 再中心化関数へ変更後位置を直接渡している |
| 回転済み実テクスチャのプレビュー、混在時の予約高、バー非表示時の番号が正しい | 関連 snapshot は production overlay ではなく専用 fixture |

特に [現在の snapshot fixture](C:/home/mimageviewer/src/ui_fullscreen.rs:12849) は、列を色付き矩形で描き、プレビューには `[None, None]` を渡しています。**45件の snapshot が緑でも、実画像の回転、要求の寿命、入力から着地までの不整合は検出できない構成です。**