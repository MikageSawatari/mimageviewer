# 動画のサブモニター点滅調査（5ch >>557〜570）

調査日: 2026-10-15。対象報告は v4.6.0、source inspection は master
`ddcc0166d`。製品アプリを起動せず、コード確認と非表示の独立 Win32 probe を実施した。
報告者の物理画面の点滅は開発側で再現できていない。**原因確定・修正完了の記録ではない。**

2026-10-16追補: §3・§6の自己通知対策はv4.7.0で実装済み。修正前の経緯はそのまま残し、
残存報告と以後の全対策候補は§7を正本とする。バックログは§1.387。
同日I1: D1の設計受入・構造合意に基づくA1〜A3の実装と本体lib全件検証を行った。対象はv4.7.1の次のリリース。
実装・検証の現在地は§8.9を正本とし、物理点滅の原因・解消は引き続き未確認である。

## 1. 回答済み事項と今回の判断

出典: [mImageViewer スレ >>557〜567](https://egg.5ch.io/test/read.cgi/software/1782419551/557-567)。
以下は報告者の観測であり、開発側の実測に読み替えない。

| 投稿 | 報告内容 |
| --- | --- |
| 557 | サブモニターで動画を全画面にすると UI が点滅。メイン側では起きない |
| 561 | v4.6.0。F11 と F12→F11 の両方で発生。UI 全体が点滅し、上バー固定または一時停止で収まる。WSXGA+ / FHD、拡大率100%、60Hz |
| 563 | マウスの場所に関係なく点滅し、上バーを固定すると他の UI も安定する |
| 565 | 下バー固定では収まらない。同一 GPU の DVI-D / HDMI。480p の4:3動画は正常、16:9動画で発生 |
| 566 | 作者から同一動画の拡大方式比較、発生するモニター、拡張／複製、実解像度を質問 |
| 567 | 拡張表示。点滅は FHD 側だけ。1280×720 は「アニメ塗り拡大」から「OSに任せる」で収まる。1920×1080 は「OSに任せる」でも収まらない |

- 複製表示固有の問題を今回の前提にしない。DPI・リフレッシュレート・接続構成・カーソル位置を再質問しない。
- アニメ塗りだけの問題では説明できない。「標準もとりあえず試して」と追加依頼する優先度は低い。
- 「解像度だけが原因」「拡大方式は関係ない」のいずれも断定しない。720pでは方式による差が既にある。
- 開発側では複数モニターと検証動画による試行でも点滅を再現できていない（このチャットの利用者報告）。
  過去の画面取得で明確な点滅が見えなかったことは、物理表示が正常という証拠にはしない。

## 2. 動画の表示面サイズと配置

正本: `src/video/native_presenter/surface_policy.rs::decide_video_surface_size` と
`render_core.rs::{video_bar_reserved_points,compute_video_visual_target_rect,compute_video_visual_transform_for_surface}`。

通常表示で `OSに任せる` は元動画サイズの swap chain を作り、DComp transformで表示先へ合わせる。
アニメ塗りなどの shader 方式は表示先サイズの swap chainを使う。ただし**物理等倍ならshader方式も
元動画サイズの直接コピーへ合流する**。回転なし・正方画素・ズームなし・固定パネルなし・
1920×1080の全画面を仮定した計算は次のとおり。

| 元動画 | 方式 | 動画表示面 | DCompでの拡大 |
| --- | --- | --- | --- |
| 1280×720 | アニメ塗り（正常にshaderを準備できた場合） | 1920×1080 | 等倍 |
| 1280×720 | OSに任せる | 1280×720 | 1.5倍 |
| 1920×1080 | アニメ塗り／OSに任せる | 1920×1080 | 等倍。shader方式も同じ直接コピーへ合流 |

この分岐は >>567 と矛盾しないが、分岐が点滅の原因である証拠ではない。
固定状態・SAR・回転・実際のclient寸法等はログなしでは確定できない。

上固定は `HUD_TOP_HEIGHT = 54pt` と設定された間隔を映像領域の上から除く。
下固定は下から領域を除く。従って同じ16:9でも:

- 上固定: 映像サイズに加え、映像の上端座標が下へ移る。
- 下固定: 映像サイズは変わるが、映像の上端は0のまま。

「上固定だけ効く」は、単に等倍や全面表示を避けるだけの説明では不足する。
動画とHUDの重なり、画面端との位置関係も調べる必要がある。

安定したsource寸法・SAR・orientation・layoutに対し、`present_for_surface_typed` は同じ面を再利用する。
`present_reusing_surface` が通常の各frameでswap chainを作り直す実装にはなっていない。
`update_video_visual_transform` もframe metadata／layoutの変更時に呼ぶため、ソース解像度だけを理由に
継続的な作り直しが発生する経路は今回のsource inspectionでは見つかっていない。

v4.6.0から調査時masterまで、`surface_policy.rs` とHUDの前面化関連ファイルに差分はない。
`render_core.rs` の差分はIME関連とその回帰テストで、上記の面選択・配置分岐は同じ。

## 3. HUD自身の前面化が再度の前面化要求になる

次のコード上の問題を特定した。ただし、**この問題が報告者の点滅を発生させたかは未確認**。

1. `hud_window.rs::raise_to_top` は `SetWindowPos(HWND_TOPMOST, SWP_NOMOVE |
   SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER)` を自分のHUDへ行う。
2. 同ファイルの `WM_WINDOWPOSCHANGING` は `hwndInsertAfter` が0または-1なら
   `RequestRaiseHud` を送る。`SWP_NOZORDER` や、自分自身が開始した変更かを区別しない。
3. `native_window_pump.rs::schedule_hud_raise` は0／16／50ms後の要求を登録し、
   `run_periodic_work` → `try_raise_hud_to_top` → 1へ戻る。

確認対象の不変条件は「外部の変化を修復するHUD前面化が、自分自身を次の修復理由にしない」。
単なる追加delayや回数制限で隠すのではなく、必要なz-order復旧要求の発生元・実際の前後関係と、
自分で適用した操作の通知を同じownerで区別する修正が必要。

独立した `gpt-6.1-sol / xhigh` の読み取りレビューも、この誤検出をコード上の欠陥と判断した。
追加で次の成立条件・限界を確認した。

- 通常構成で独立HUDを作る `FullscreenBorderless` が対象。F11とF12→F11の両方が該当し得る。
- `editor_ui_snapshot` が `Some`、前面windowが許可対象、presentation遷移中ではないことが実raiseの条件。
  通常動画のopenはVSTの有効／無効によらずsnapshotを渡すので、VSTプラグインの使用は必須ではない。
- 要求はlatest-value slotでcoalesceされるため、queueが際限なく増えなくても自己循環は成立する。
  同期再帰ではなく、pumpの次のiterationで即時raiseが再登録される形になる。
- **pause・上固定・下固定を条件に止める処理は、この循環経路にはない。**
  従って一時停止や上固定で点滅が収まることを、この欠陥だけで直接説明できない。
  「前面化は続くが映像の更新／配置との組合せで物理点滅だけ止まる」は未検証の追加仮説。
- ソース解像度やモニターを選別する条件もこの経路にはない。報告の全条件を説明する原因として扱わない。
- `set_geometry` 内にはコメントに書かれた同一矩形比較もなく、呼ばれるとSetWindowPosを発行する。

[SetWindowPosの仕様](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos)
でも、`SWP_NOZORDER` は前後関係を維持してinsertAfterを無視する指定で、
`WM_WINDOWPOSCHANGING` 自体を止める指定ではない。

### 非表示Win32 probeによる追加証拠

再実行用の `scripts/probe-hidden-hud-zorder.ps1` は独立した非表示HWNDを生成し、
製品と同じ `SetWindowPos` のflagsを3回適用して通知を採取する。
`WS_VISIBLE` は付けず、ShowWindow・foreground変更・入力送信を行わず、終了時に破棄する。
製品アプリの起動・実データの使用はない。

2026-10-15の実行結果（exit 0）:

| 操作 | 通知のinsertAfter | 通知のflags | 現行判定が前面化を要求するか |
| --- | ---: | ---: | --- |
| 自分へHWND_TOPMOST（1回目） | 0 | 531（0x213） | true |
| 自分へHWND_TOPMOST（2回目） | 0 | 531（0x213） | true |
| 自分へHWND_TOPMOST（3回目） | 0 | 531（0x213） | true |
| NOZORDER付きの位置変更 | 0 | 532（0x214） | true |

同じ前面化を繰り返しても通知は来る。さらにNOZORDERでも現行条件は成立する。
これは独立した非表示HWNDでのWin32通知の観測であり、製品内で前面化が無限に続いた、
または物理画面が点滅したという実測ではない。製品側の実行可否条件は上記のコード確認に基づく。

## 4. DWM／ドライバー仮説の位置づけ

画面とbufferの寸法・拡大・重なりによってWindowsの合成経路が変わり得ることは、
[MicrosoftのDXGI flip model解説](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/for-best-performance--use-dxgi-flip-model)
で確認できる。解像度・固定位置で発生が変わる今回の報告とは整合するが、MPO／ドライバー原因の確定ではない。

現行rendererには、v0.9.2由来の完全透明な常駐カバーが既にある。
`MIV_DISABLE_MPO_COVER` が無い限り、`NativeTestOverlay`を動画visualの上へ置く。
過去に似た症状を改善した記録はあるが、この実装の存在だけで、今回のPCでもMPOが必ず無効に
なっているとは証明できない。現行ログにはWindowsが実際に選んだscanout／合成modeの測定がない。
同じカバーを追加し直す、録画で見えないから正常とする、といった扱いはしない。

## 5. 次に開発側で確かめること

追加質問より先に、次の証拠を開発側で整える。

1. HUD前面化の発行元と適用回数を記録し、入力・外部window変更のない状態で自走するかを検証する。
   製品アプリでの可視試験は既存の隔離portable／実機承認ルールに従う。
2. 動画のsource／swap chain／最終映像矩形／HUD実矩形／固定状態を同一window epochと時刻で対応づける。
   surface swap、HUD可視性、region変更の反復をログから先に排除する。
3. アプリ側の状態が安定していても点滅する場合に、PresentMon等のETWで実合成modeを確認する。
   内部ログに「点滅を検知した」とは書かない。成功したPresentも物理表示の正常を証明しない。

既存perflogの `video_surface_swap`／`resize`／`video_geometry`／`egui_overlay_visual`／
`hud_region_publish` は上記2の一部に利用できる。
`presentation_observer::observe` はactive transition IDが無いとreturnするため、
定常再生中のすべてのSetWindowPosを記録できるとは限らない。前面化の原因・連鎖の証拠には不足する。

報告者へ「標準も試して」「既出の環境情報をもう一度」とは依頼しない。
後日ログ提供が必要になった場合も、開発側で記録内容を検証した後、短い1回の手順にまとめる。

## 6. 別セッションへの対策案（2026-10-15 追調査）

利用者の指示: 原因究明を継続する。原因と確定していなくても、可能性のあるコード欠陥を
機能退行なく直せるなら次版へ入れたい。**このセッションは調査・対策整理まで。製品修正は別セッション。**

### 6.1 次版の第一候補: 自己通知とNOZORDERの誤検出を止める

限定案A:

1. `HudOverlayWindow::raise_to_top` が発行する、移動もサイズ変更もしない自前のSetWindowPosに
   `SWP_NOSENDCHANGING` を追加する。この操作だけは自分の事前通知を発生させない。
2. HUDの `WM_WINDOWPOSCHANGING` は、`WINDOWPOS.flags` に `SWP_NOZORDER` がある場合、
   `RequestRaiseHud` を生成しない。前後関係を変更しない通知だからである。
3. 既存の `DefWindowProcW` 呼び出し、外部由来の前面化要求、VSTの明示hook、hover polling、
   foreground／transition permit／window epochの判定は維持する。

`gpt-6.1-sol / xhigh` の独立設計レビューも案Aを次版の第一候補と判断した。
追加の制約:

- NOSENDCHANGINGは自前raiseだけに付ける。汎用SetWindowPos wrapper、geometry／DPI／
  ShowWindow、presenterやVSTの操作へ広げない。raiseにNOZORDERを付けて前面化自体を止めない。
- 0／16／50msの既存burstとforeground／snapshot／permitは変更しない。
  geometry由来の誤要求が偶然提供していた復帰効果もなくなるので、初回表示・再表示・配置切替の
  HUD復帰を受け入れ条件から外さない。
- この案の受け入れに、診断ログの実装完了や報告者の点滅再現を必須条件として追加しない。
  コード欠陥の修正と、点滅原因を探る診断は別々に検証できる単位として扱う。

同じ入力を受けたら同じ結果にするだけのdelay・rate limit追加ではなく、
**自分が適用する操作を、次の復旧要求の発生元にしない**境界の修正として扱う。
同じ問題を別のboolや一時的な再入防止flagで覆う設計は不要。

この案の変更範囲は基本的に `native_window_host/hud_window.rs` の2箇所と関連テスト。
新しい診断を併せる場合は次節のpump／source側に広がる。修正時間の見積もりは実装者が
テストfixtureと所有境界を確認してから行う。

#### Win32プリミティブの追加検証結果

probeに非表示のownerと、同じownerを持つTOPMOSTのHUD／editorを追加した。
毎回editorをHUDより前へ置き直してから各操作を行った。

| 操作 | 事前通知数 | HUDがeditorより前へ移るか | HUDの位置・大きさ | TOPMOST／foreground／非表示 |
| --- | ---: | --- | --- | --- |
| 現行のraise（0x213） | 1 | はい | 不変 | 維持 |
| 自前raise＋NOSENDCHANGING（0x613） | 0 | はい | 不変 | 維持 |
| NOZORDER付きgeometry（0x214） | 1 | いいえ。指定した前後関係を維持 | 指定どおり移動 | 維持 |
| NOSENDCHANGINGを付けない通常のraise | 1 | はい | 不変 | 維持 |

再実行: `powershell -NoProfile -File .\scripts\probe-hidden-hud-zorder.ps1`。
2026-10-15に上記4条件を検査してexit 0。現行の通知4件を採取し、候補の意味論もassertする。

NOZORDER通知は現行述語でtrue、flagsを確認する候補述語でfalse。
自前操作だけにNOSENDCHANGINGを付けても、通常の通知経路は残る。
以上は独立した非表示windowでのUSER32動作の検証であり、製品pump／実VST／EffeTuneや
物理点滅を検証したものではない。**「退行なし」を確定するには下の製品回帰が必要**。

この案は対象の明確なコード欠陥を修正する価値があるが、点滅修正済みとは告知しない。
pauseや上固定で収まる理由との因果関係は引き続き未確認。

### 6.2 併せて入れる診断: 原因を区別できる軽い記録

通常の性能ログONだけで比較でき、環境変数の設定や長い試験を報告者に求めない形を推奨する。
実装する場合は次の3つを同じwindow epochとmonotonic timestampで関連づける。

- **前面化**: 要求の出所（VST hook／hover polling／WINDOWPOS通知）、発行数、適用数、
  foreground／permitによる不適用数。WINDOWPOSのflagsとinsertAfterは変化時の代表sampleに残す。
- **配置**: source／swap chain寸法、SAR・回転、最終映像rect、presenter／HUDの実rect、
  上下の実効固定、HUD regionと可視性。source変更・F11切替・monitor移動・lock変更時だけ詳細を記録。
- **継続状態**: 再生／一時停止、直近のvideo PresentとHUD Presentの回数、surface swap回数。
  1秒程度の集約で良く、各frameや4ms pumpごとの大量ログを常時書かない。

ログのために同期ディスクI/Oをpump／renderへ追加しない。既存perf routeを使い、
性能ログOFFでは集計・文字列生成の費用を抑え、context終了時に別contextの記録と混ぜない。
既存の `presentation_observer` は遷移中に限定されるので、そこへ記録を足すだけでは不十分。

この診断で判別したいのは、(a) 不要な前面化の継続、(b) surface／geometryの交互変更、
(c) HUDのattach／detachやregionの交互変更。これらが安定している場合にWindows合成側の
調査へ進む。アプリのPresent成功を、物理点滅がなかった証拠として扱わない。

### 6.3 今回まとめて変更しない案と、その理由

| 案 | 判断／残る問題 |
| --- | --- |
| WM_WINDOWPOSCHANGINGからのraiseを全廃 | 自己循環は消えるが、既存の外部復旧経路を丸ごと削る。明示hook／pollingだけで全操作を維持できる証拠が先に必要。第一候補にはしない |
| 実z-orderを見て必要時だけraise | hover polling由来の不要なraiseも減らせるが、許可対象のeditor／popup／複数windowの順序規則が必要。予約時点でHUDが上でも、非同期VST操作が後から反映されるためburst全体を取り消さない。今回の小さい修正へ混ぜず、別の設計単位にする |
| 同じHUD矩形ならSetWindowPosを省く | 所有境界での冪等化としては妥当。ただし自前raiseの自己通知は直らない。安定再生中にgeometryが反復する証拠を得てから対象を決める |
| 透明MPOカバーを既定で外す／描き直し方を変える | 旧版で改善した症状の退行リスク。既存のMIV_DISABLE_MPO_COVERを開発側の診断比較に使えるが、今回の根拠で配布時既定を変えない |
| 常に上HUD固定／映像に1pxの余白／微小な不透明面 | 表示領域・画素・使い方を変える症状回避。利用者の既存機能を保つ修正にはならない |
| HUDをpresenter内へ戻して単一window化 | VST editorよりHUDを前へ出すための現行構造を失う。所有・入力・重なり全体の大きい設計変更 |
| OS拡大を強制／等倍でも全件shaderへ回す | 選択したフィルタ、画質・GPU負荷・等倍経路が変わる。1080pの報告も説明しきれておらず、根拠が足りない |
| Windows全体のMPO無効化・driver設定変更を依頼 | 他アプリにも影響し、現状の報告者負担を増やす。開発側のコード欠陥と診断を先に扱う |
| Swap chainをbitblt／DXGI_SCALING_NONEへ変更 | 現行CreateSwapChainForCompositionの構成に沿わない。小さい修正として扱えない |

[CreateSwapChainForCompositionの仕様](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgifactory2-createswapchainforcomposition)
はflip modelとDXGI_SCALING_STRETCHを前提とする。buffer属性を単発で変えてMPO対策とする案は採らない。

### 6.4 別セッションの受け入れ条件と出荷判断

実装前に、限定案AがVST／EffeTuneのHUD復旧を残すことを設計担当と独立レビューで合意する。
detachedの述語／viewport経路へ実装が広がるなら、`detached-rework-plan.md` §2と§11の手順に従う。

自動回帰（既存の実handler／event routeを使い、別実装の述語だけをテストしない）:

1. 自前raiseの後、追加のRequestRaiseHudを自分で作らない。
2. NOZORDER付きgeometryから前面化要求を作らないが、geometry自体は適用される。
3. 明示VST hook／通常の復旧要求は適用でき、HUDをeditorより前へ出せる。
4. foreground不許可・permit=false・退役epochではraiseしない。解除後の正規復旧は維持する。
5. 操作のない状態でburstが終端し、pause／lockを変えても自己循環を作らない。

hover端にいる間は既存50ms pollingが正規要求を作るため、試験5では**入力起点の要求と自己再投入を分ける**。
修正後に常時raiseが完全にゼロになると主張しない。実VSTやEffeTuneの前後関係保持も必要。

実機検証は隔離portableの了承済みsuiteか、利用者が起動する通常確認buildで行う。
検証対象: F11／F12→F11、F11解除・再突入、Alt+Tab往復、VST・EffeTune editorの表示／操作／非表示、
上下HUDのhover・固定・クリック、再生／pause、常に最前面ON/OFF、monitor間移動、
複数windowの切替・park／resume。既存のTOPMOST・owner・focus・click-throughを壊さないこと。
window無しのprobeはこの実機確認の代替にしない。

EffeTune editorは一般VST editorと異なり、ownerless・非TOPMOST等の別方針を持つ。
`docs/effetune-integration-plan.md`の契約を読み、今回の修正を理由にHUD復旧のallowlistへ追加しない。
また、予約時点のz-orderだけで後続burstを消すと非同期VST操作後のHUD復帰を失い得る。

最初にnarrow regression、続いて既存のWindows window-host／foreground関連回帰と通常core checkを行う。
ただし既存 `hud_focus_claim_uses_registered_editor_and_preserves_presenter_return` は子processが
ShowWindowを呼ぶため、非表示probeと同列の非対話試験として自動実行しない。
表示・focusを動かす試験は了承済みの検証枠へ分ける。
共有pumpの挙動に変更が広がる場合は `scripts/test-full.ps1` を含む既存gateへ広げる。
製品動作を変えた実装セッションは通常coreの確認buildと起動commandを用意する。

**次版への判断**: 限定案Aと必要な診断を先行候補にする。点滅を再現できなくても、
コード欠陥の回帰がred→greenとなり、既存のHUD／VST／window操作を維持できた証拠が揃えば、
「不要なHUD前面化の繰り返しを修正」として収録を検討できる。
「報告された点滅を修正」は、報告者または開発側で当該症状の改善を確認するまで区別する。

### 6.5 実装担当への短い申し送り

本書§1〜3の既出条件と自己通知の欠陥、§6.1の案Aを前提に、製品handlerに接続した回帰を
先に作り、HUDの自前raiseだけのNOSENDCHANGINGと、NOZORDER通知からのraise要求除外を実装する。
VST hook、hover polling、0／16／50ms burst、foreground／epoch／transition許可を維持する。
EffeTuneを一般VSTの復旧対象へ広げない。DComp構成・映像サイズ・フィルタ・MPOカバーは変更しない。
実装時は関連する動画／VST文書のイベント説明も同期する。
§6.4の自動回帰・実機条件と独立レビューを満たせば、点滅未再現でもコード欠陥の修正として
次版候補にできる。点滅原因の診断追加は§6.2を別単位で扱う。

## 7. v4.7.0後の残存報告と対策台帳（2026-10-16）

### 7.1 観測と利用者決定

- >>569で作者は、自前raiseの自己循環を修正したが点滅との関係は未確認と説明した。
  `27fa289ee`はv4.7.0に含まれ、現行masterでも自前raiseの`SWP_NOSENDCHANGING`と
  NOZORDER通知からの要求除外を維持している。これを未着手へ戻さない。
- [>>570](https://egg.5ch.io/test/read.cgi/software/1782419551/570)は
  「完全な解消は難しかったよう」とのお礼。未解消の趣旨と読めるが、改善量・再試験の
  設定・旧条件が全て継続するかまでは記載がない。「全く変わらない」と読み替えない。
- 開発側は現在も未再現。DWM/MPO/driver原因、残存する前面化ループのいずれも確定していない。
- 利用者決定: 再現待ちで止めず、取れる対策を候補から落とさず記録し、根拠のある改善・
  計測・比較を順に進める。機能を失う回避策や未評価の構成変更を全件同時投入する意味ではない。

### 7.2 先行する実装候補

| ID | 対策と根拠 | 維持する契約・確認事項 |
| --- | --- | --- |
| A1 | §6.2の定常診断。raise要求元/適用/skip、geometry要求/実変更、region hash、実描画状態、attach/detach/Commit、動画/HUD Presentとsurface変更を同一epoch・時刻へ関連付ける | 1秒程度の集約＋変更時詳細、既存非同期logger、OFF時の低コスト。新しい常時tickやUI/pump同期I/Oを追加しない。source/SAR/回転、全矩形、実効固定、monitor/DPI、動画D3D11/HUD DX12のadapter identityも比較可能にする |
| A2 | `HudOverlayWindow::set_geometry`の同一矩形操作をowner境界で省く。`native_window_pump`のResize/GeometryChanged/DpiChangedから直接届き、上流のz-order-only除外やrendererの同サイズ抑制だけでは止まらない | HWND再生成・外部位置変更・F11・monitor/DPI変更後も正しい実矩形へ適用する。現在のコードで重複操作が可能という根拠と、定常再生で反復しているか未確認という限界を分離 |
| A3 | `run_native_event_batch`がlogical passを一回のPresentへまとめる契約に、実DComp可視性の適用も揃える。`render_logical_once`の中間非表示でdetachし、最終表示ならPresent後にattachし直し得る | logical passごとの入力所有・IME・コマンド・deltaを変えず、実attach/detachだけ最終結果で一度適用。最終非表示/無描画/失敗/close/cancelを同じownerで完結する。無操作の定常ちらつきの原因とは未認定 |
| A4 | 実z-orderと正規要求の由来に基づき、既に正しい順序なら不要なraiseを省く設計 | §1.385のVSTによる遮蔽を悪化させない。非同期editor/popupの後着変更、0/16/50ms burst、foreground/epoch/transition permitを維持。予約時点の判定だけで後続burstを捨てない。EffeTuneとの区別は継続 |
| A5 | §1.382のegui 0.37 / wgpu 30更新と、同条件の前後比較 | HUDの描画・Presentとeguiの余分なrepaint修正は変わり得る。動画D3D11/分離HWND/基本Present方式は維持する計画で、更新を点滅修正と断定しない。既存の透明alpha・色空間・AutoVsync/latency 1・実矩形を照合 |

A3の主な根拠は`render_core.rs::run_native_event_batch`、
`NativeEguiOverlay::render_logical_once`、`NativeEguiOverlayState::render_logical_once`の
`if !overlay_visible { detach_visual()?; }`、`present_logical_batch`後の再attach。
単一logical passの通常描画や、同値なら戻る`set_visual_attached`だけを調べて終了しない。
「表示→途中非表示→最終表示」のbatchで不要なCommitがない回帰と、最終非表示になる回帰を置く。
A2も直接helperだけでなくpumpのResize/GeometryChanged/DPI経路を通す。

egui更新計画は作業ツリー
`G:\home\mimageviewer-egui036\docs\egui-0.37-update-plan.md`第3.1版、特に§4.9/段4c。
master未取込の計画と製品実装を混同しない。本家の
[0.37更新履歴](https://github.com/emilk/egui/releases/tag/0.37.0)には余分なrepaintの修正があるが、
今回の解像度・上固定依存との因果を示すものではない。
調査したwgpu-hal 27.0.4/30.0.1のDX12では、CompositionVisualに対する
`CreateSwapChainForComposition`・FLIP_DISCARD・最終的なDXGI Presentの基本方式は共通。
更新による変化は、前後の設定と実geometryを揃えて評価する。

### 7.3 診断比較と、結果に応じて進める対策

| ID | 開発側の比較 | 結果を受けた次の対策 |
| --- | --- | --- |
| B1 | 同一1080pで映像寸法とHUD状態を固定し、映像Y位置/上HUDとの重なりだけを変える。既知の上/下固定・720p/1080p・4:3/16:9・OS/アプリ拡大と照合 | 位置・重なりが原因なら、画素や表示領域を削らずに配置/合成ownerを修正する。1px余白や強制固定を通常仕様へ混ぜない |
| B2 | 通常の分離HUDと`MIV_HUD_OVERLAY=0`の既存fallbackを比較 | 分離構成が関係するなら、入力/IME/z-orderを維持するDComp構成の設計へ進む。fallbackはVSTの裏へHUDが潜るため、診断で改善しても既定化は別判断 |
| B3 | `MIV_DISABLE_MPO_COVER`の有無を一つずつ比較し、既存透明カバーの実効性を確認 | カバーやMPOが関係すると分かったら、重複追加ではなく実効性を測れる合成構成を設計する。完全透明なvisualの存在だけでMPO無効化を保証しない |
| B4 | A1でアプリ状態の反復を排除してから、PresentMon/ETWで動画/HUD swap chain別のPresentMode・表示時刻とアプリ時刻を照合 | compose/scanout/synchronizationの問題を分離し、submit/present順・同期、surface寿命、alpha/色空間/latencyの対象を絞る。物理ちらつきは必要なら画面撮影と対応付け、Present成功を無点滅の証拠にしない |
| B5 | 局所対策で解決できず、B1〜B4で構成の関与を裏付けられた場合のみ、同一DComp tree・単一HWND化・対応する別Present方式を設計比較 | 独立した大きい変更として設計/レビュー/負荷測定を行う。VST前面操作、TOPMOST、focus、各パネルと既存画質を維持。維持できない案や通常利用が遅くなる案は費用と代案を示して利用者判断を取る |

環境変数は開発側の比較用。実画面検証はそのrunの明示承認と隔離portableを必要とし、
通常profileや利用者データをagentが起動して試さない。報告者へこの比較表を渡して総当たりを求めない。
追加ログが不可欠になったときも、質問済み事項を繰り返さず、目的を説明した小さい一回の採取へ集約する。

### 7.4 現時点で通常版へ入れない回避策

§6.3の候補も捨てず、採らない理由を維持する。利用者の「取れる対策を進める」を、
次の機能変更まで承認されたという意味にはしない。

- 常に上HUD固定、映像へ1px余白、微小な不透明面: 表示領域や画素を変えるため、診断と通常仕様を分離。
- OS拡大の強制、物理等倍でも全件shaderへ回す: 選択したフィルタ・画質・GPU負荷が変わる。比較用には検討可能だが無断で既定化しない。
- WINDOWPOS由来のraiseを丸ごと削る、予約時点だけのz-order判定でburstを消す: 正規のVST復旧を失う恐れ。A4の所有規則と回帰が先。
- Windows全体のMPO無効化・driver更新等を報告者へ次々依頼: 他アプリへ影響し、負担が大きい。アプリ側の根拠と開発側診断を先にする。
- bitblt / `DXGI_SCALING_NONE`を現在の`CreateSwapChainForComposition`へ属性だけ差し替える: APIの前提に合わない。必要ならB5の独立した構成設計として扱う。
- HUD HWNDの常時廃止・既存透明カバーの無検証撤去: VST復旧や過去に改善した症状を退行させ得るため、B2/B3/B5の結果と設計なしには行わない。

### 7.5 実施順・完了の判定

A1と、根拠のあるA2/A3の小改善、A5の更新前後比較を先行対象とする。A4は§1.385との
所有規則を設計してから着手し、B1〜B4は診断の結果を見ながら小さく実施する。B5は最後の選択肢。
改善ごとに差分・根拠・自動回帰・実機未確認事項を分けて記録し、大きい更新と一括にして原因を見失わない。
detached経路は既存の双方レビュー合意・§11記録を守り、§6.4のVST/EffeTune、
上下HUD/ストリップ/左右パネル、最前面/focus/IME、F11/F12、monitor移動、park/promote、兄弟窓を確認する。
設定・画質・表示領域・既存操作を削る案は、利用者に費用を示して判断を求める。

小改善の回帰が通ったことと、>>570の物理点滅が直ったことは別の受入条件。
開発側か報告者による改善確認まで「点滅を修正済み」とは記載せず、収録済みの対策を具体的に説明する。
§7追補時点では製品コード変更・実画面比較は実施していなかった。その後のA1〜A3の着手は§8.9へ記録する。
§1.382自体の独立した進捗は更新担当が管理し、A5の点滅比較の実施・成功と混同しない。

## 8. A1〜A3 の先行改善設計 D1／実装 I1（2026-10-16）

### 8.1 対象・証拠・判断

利用者の2026-10-16指示により、§7を正本としてD1でA1〜A3だけを設計した。
D1時点ではv4.7.1への収録候補だったが、I1の対象は**v4.7.1の次のリリース**へ変更された。
点滅解消の保証やリリース完了の記録ではない。
source inspection対象は `hud-flicker-1387` の `0272439ac6f03ead6470bdb40787e8969cf6c4f1`。
D1では本節だけを変更し、製品コード・設定・バイナリ・実画面試験には触れていない。
利用者からの引継ぎによると、D1 commit `66708856f` は独立した `gpt-6.1-sol` / `xhigh`
レビューで指摘なしの **ACCEPT**。同レビュアーとClaudeCodeの双方が、A2/A3は症状パッチではなく
既存ownerの契約を直す構造的修正であることへ明示的に合意した。このD1受入をI1の実装レビュー・
自動検証・実機検収へ読み替えない。担当モデルの設定を本書によって変更したとは扱わない。

報告者>>570は未解消の趣旨で、改善量と再試験条件は不明。開発側も未再現である（§7.1）。
以下の根拠はコードの構造であり、報告者の画面で反復が起きたという実測ではない。

| 対策 | コードで確認した前提・根拠 | D1の判断 |
| --- | --- | --- |
| A1 | `presentation_observer::observe` はactive transitionがないと記録しない。`hud_region_publish`、`egui_overlay_visual`、`video_geometry`等のperfはあるが、全経路に共通のoutput/window identity・要求由来・実適用結果は揃わない。`perf.rs::{is_enabled,program_start,event}`と`log_writer`の非同期配送を利用できる | 定常診断を追加する。観測専用で、raiseの可否・頻度や描画cadenceを変更しない |
| A2 | [`HudOverlayWindow::set_geometry`](../src/video/native_window_host/hud_window.rs) は同一矩形を省くコメントと異なり、null HWND以外は毎回SetWindowPosを発行。[`PumpRuntime::handle_command` / `drain_window_events`](../src/video/native_window_pump.rs) のResize / GeometryChanged / DpiChangedはいずれも`NativeWindowHost::set_hud_geometry`へ届く | HUD HWNDの実矩形をownerで照合して冪等化する。定常反復・点滅への因果は未確認 |
| A3 | [`run_native_event_batch`](../src/video/native_presenter/render_core.rs) はwheel/入力所有境界で複数logical passを実行し、`NativeOverlayLogicalBatch::append`が最終shapes/visibilityと累積delta/commandを一回の`batch_present`へ渡す。一方、Stateの`render_logical_once`は中間非表示でもdetach closureを呼び、GPUの`set_visual_attached(false)`がRemoveVisual＋Commitする。最終表示なら`present_logical_batch`がPresent後に再attachする | 実可視性も最終描画境界へ移す。単一passの同値skipだけでは防げない余分なDComp操作を除く。無操作の定常点滅の原因とは認定しない |

指示された関数名・3つのpump入口・中間detach／最終attachの前提に食い違いはない。
ただし終端の設計には次の現状も含める。`video/mod.rs`のoverlay input/tick、初期描画、
GeometryChanged/DpiChangedのresize等には、描画エラーをログだけにして継続する呼出し元がある。
「どの描画失敗も既にrender faultで終了する」とは前提にしない（§8.5）。
また、描画なしの正常returnは既に存在し、毎batch必ずPresentするわけではない。

### 8.2 共通の不変条件・変更所有者

1. HWNDのcreate/query/move/region/z-order/focus/IMEは既存のpump / `NativeWindowHost`、
   D3D/DXGI/DComp/wgpuはrender / `NativeRenderCore`の所有を維持する。
   renderにUSER32 queryを持ち込まず、pumpからGPU ackやjoinを同期waitしない。
2. 新しい常時tick、repaint、delay、retry、z-order判定、focus操作、設定・永続化を足さない。
   0/16/50msのraise burst、hover polling、foreground/epoch/permit、VST hookは既存のまま。
   v4.7.0のNOSENDCHANGING/NOZORDER修正も維持する。
3. 動画のsource/表示領域/画素/フィルタ、SAR/回転/zoom、swap chain選択、MPO cover、
   alpha/色空間/Present mode/latency、各パネルと入力操作は変更しない。
4. 資源と計数はexact output/window epochで所有する。同じ動画path/index、同じHWND値の再利用、
   共有DX12 deviceを理由に別windowの記録・可視性・regionを変更しない。
5. logical passごとのegui run、入力segmentのrouting、IME、hidden cleanup、command順、
   `FullOutput::append`によるtexture set/freeとplatform output、repaint deadlineの扱いを維持する。
   最終shapes/visibility/strip inventoryだけが最後のpassを表す現契約も維持する。

実装予定の所有範囲は、A1が `video/mod.rs`（output生成・source/raise由来の配送）、
`native_window.rs`（通知由来の採取）、`native_window_pump.rs` / `native_window_host{,/hud_window}.rs`
（要求・実操作）、`native_presenter/{render_core,overlay_gpu}.rs`（GPU事実）、
必要最小限のvideo内診断helperと解析テスト。A2はHUD ownerと上記pump経路の回帰、
A3はrender_coreのCPU/GPU境界とmod.rsの該当エラー伝播・回帰である。
`presentation_observer`の遷移専用契約、Appのdetached述語、viewport/placement reducerは変更しない。
実装時の関連動画文書更新と、必要になった場合のdetached計画§11への記録は別作業に含める。

### 8.3 A1: 定常診断の相関・項目・コスト

#### 相関と集約のowner

- perf有効時だけ、native output作成時にprocess内で一意な診断用`output_id`を割り当て、
  pump spawnとrender configへ同じ値を渡す。既存window epochはoutputを跨ぐ一意性を保証しないので、
  `output_id + window_epoch + target_role`を主キーにする。既存source epoch、placement、
  既存transition/request ID（あれば）、overlay device generationとsurface revisionも記録する。
  診断IDをイベント受理や可視性の新しいgateとして使用しない。
- 時刻は`perf::program_start()`と同じInstant原点から採取した`observed_us`とする。
  perf行の`t`は発行時刻なので別に残す。要求発行／pump消費／実操作の時刻とsequenceを区別し、
  異なるthreadの行順を因果順とみなさない。各snapshotに観測時刻・revisionを付け、
  pumpの矩形とrenderのsource情報が厳密に同時だったかのように一行へ合成しない。
- pumpとrenderは自分のowner内の固定個数counter・最新snapshotを更新し、各既存loop/event/
  Presentの出口で約1秒経過したときだけ集約をperfへ送る。診断専用thread/timer/wakeは作らない。
  無操作で既存処理が止まる間は記録も止め、次の既存起床または退役で残区間を出す。
  区間のstart/end/elapsedを記録し、「1秒間隔の行が無い」をゼロ操作の証拠にしない。
- source/geometry/visibility/region/target/device/surface変更時は詳細を残す。
  振動時にも無制限に行を増やさないよう、owner・1秒区間ごとに詳細は最大8件、
  以降は種別別の変更総数・省略数・最初/最後のsampleを集約する。退役時も同じ上限に従う。
  最初のsnapshotと最終snapshot、正常終端/abort/faultを識別できるようにする。
  logger journalの欠落とは別の「詳細省略」であり、途中順序が不明なら解析結果も不明とする。
- latest-value coalesceを維持する。raiseは少なくともVST hook由来のApp要求、renderのhover要求、
  HUDのWINDOWPOS通知を、同じ無情報なRaiseHudへ合流させる前に分類する。
  診断用の由来/sequence/sampleを既存command/event envelopeからpumpまで運び、burstは元要求へ紐付ける。
  混在してcoalesceした要求を一つの由来へ断定せず、由来別受付数と置換数・代表sampleを区別する。
  VST内部hook回数、App latchをdrainした要求数、pump要求数、burst実行数は別の量である。
  この設計での必須「要求数」はApp/render/wndprocがnative経路へ発行した数とし、
  windowの確定していないVST内部hookを現epochへ遡って帰属させない。

#### 計装する事実

| 所有境界 | 集約・変化時詳細 | 読み方・費用の制約 |
| --- | --- | --- |
| raiseのproducer / pump / HUD owner | origin、要求/消費/coalesce数、burst slot、適用試行/Win32成功/失敗、permit無効・hostなし/退役・HUDなし・snapshotなし・foreground不許可によるskip。WINDOWPOS flags/insertAfter、foreground HWND/分類、要求と操作のsequence | `try_raise_hud_to_top`のboolはWin32成功を保証しない（内部raiseは現在結果を捨てる）。既存判定を一回だけ評価して診断結果を返し、ログのために再判定しない。snapshotやpopupの許可規則は変更しない |
| HUD geometry owner / pump | Resize / GeometryChanged / DpiChanged別の要求数・要求rect、照合した実rect、same skip、query失敗、SetWindowPos試行/成功/失敗、操作後の実変更/不変/未確認。presenterの要求・window/client rect、HUD window/client rect、各観測revision | 呼出数やSetWindowPos成功数を実矩形変更数と同一視しない。A2の前照合を再利用し、perf ON時の実操作後だけ追加queryする。外部変更は実sampleの差として記録し、自己操作と区別する |
| pumpのregion / HWND表示 | 要求hash/count/empty、foreground抑止前後のhash、実SetWindowRgnの試行/成功/同値skip/失敗、適用成功hash、HWND show/hide/破棄とtopology | `NativeWindowHost::apply_hud_regions`のpublish hashと`HudOverlayWindow::last_regions_hash`の成功hashを分ける。shared hit regionとOS適用結果も混同しない。既存hash計算を再利用し、詳細rect列は変更時だけ |
| renderのCPU描画・最終batch | logical pass数と中間/最終overlay_visible、top_bar_drawn_visible / bottom_hud_visible、strip/左右panel/preview/tile/modal等の実描画区分、raw固定とexact hostへ投影した実効固定、ppp/OS DPI/UI scale、最終HUD各rectとpaint job数 | regionやhoverから実描画を再推定しない。logical visible・実paint有無・DComp attach・HWND visibleを別フィールドにする。文字列・画素内容・IME本文は記録しない |
| renderのDComp | target/visual role、AddVisual/RemoveVisual/SetContent/transform/SetRoot、Commitの理由・試行/成功/失敗、attachの旧/要求/成功後状態。生成・surface交換・geometry・HUD最終可視性・退役を区別 | 全Commit callsite（core初期化、video surface切替、transform、HUD attach/detach）を棚卸しして計数。COM成功はDWMでの可視完了ではない。記録用Commit/WaitForCommitCompletionは発行しない |
| renderのPresent / surface | video/HUD別のattempt/success/error、直近時刻・所要時間、video sync interval、hidden consume-and-hold、surface作成/交換/configure/ResizeBuffers/dropとrevision、寸法/content種別、format/alpha/colorspace情報/present mode/latency | wgpuのpresentは返り値がないため「present呼出完了」と書き、DXGI HRESULT成功や物理表示完了と同じsuccessにしない。background/透明coverのPresentも別roleで識別し、video/HUD回数へ混ぜない |
| renderのvideo geometry | source width/height、SAR、orientation、filter、zoom/compact、video surface size/content、予約領域、target rect、最終visual transform/映像rect、letterbox/canvas寸法 | `set_video_geometry`、surface policy、`update_video_visual_transform`の既に確定した値を採取し、診断用に配置・shader選択を再計算しない |
| pumpのmonitor / renderのadapter | exact presenter/HUD HWND、monitor handle/device名・monitor/work rect、DPIと観測時刻。video D3D11とoverlay DX12それぞれのLUID、vendor/device/subsystem等の取得可能なID、名称/backend、DX12 device generation、surface対応関係 | monitor/HWND queryはpumpだけ。生成・geometry/DPI event・既存起床の約1秒sampleで採取し、4ms/8ms毎には追加しない。adapter照会はrenderの生成時だけキャッシュし、monitor移動でGPUが移ったと推定しない |

D3D11のadapterは現行`anime4k_adapter_and_key`と同じDXGI device→GetAdapter→GetDesc1から
LUIDを読むが、Anime4Kの有効化やbenchmarkを起動しない。HUDは
`overlay_gpu.rs::DeviceEpoch`の実wgpu DX12 deviceからLUIDを取得する。
ローカル依存ソースのwgpu 27.0.1 `Device::as_hal`、wgpu-hal 27.0.4 DX12
`Device::raw_device`、Windows bindingの`ID3D12Device::GetAdapterLuid`が利用可能なことを読んだ。
unsafeなborrow guard内でread-only照会して値だけを保持し、raw COM pointerを別ownerへ公開しない。
`Adapter::get_info().name`やvendor/deviceの一致だけを同一adapterの証明にしない。
照会不能は`unknown`と理由を残し、失敗のために描画deviceを変更しない。
通常DX12経路で両LUIDを比較できることをA1受入に含める。

#### コストとOFF条件

perf OFFでは入口のatomic check以外に新たな時計読取・集計・JSON/format・hash走査・monitor/
adapter queryを行わない。A2の正しさのためのGetWindowRectとA3の最終適用はperfから独立する。
perf ONでも毎frameは固定counterと必要なscalar sample更新だけとし、既存Instantを再利用する。
詳細/集約のJSON化は境界でのみ行い、`perf::event`の外側で必ずenabledを判定する。
出力は既存`log_writer`の有界非blocking journalへ送る。UI/pump/renderでopen/write/flush、
flush完了待ち、ログ専用の共有mutex待ち、GPU同期を追加しない。
既存perfイベントは互換性を保って相関fieldを補う。新規診断の定常追加量はownerごと約1行/秒、
変化詳細は上記上限内とし、既存イベント分と分けて測る。

### 8.4 A2: 実矩形照合によるHUD geometry冪等化

ownerは`HudOverlayWindow::set_geometry`のままにする。要求値キャッシュを新設する案は採らない。
各要求で次の順に一度だけ処理する。

1. 既存と同じw/hの正規化（最低1px）で要求screen pixel rectを作る。
   HWNDが無い場合は従来どおり操作せず、A1へ`no_hwnd`を記録する。
2. pump owner threadで当該HUDの`GetWindowRect`を読む。HUDは枠なしWS_POPUPなので、
   位置・幅・高さを同じscreen座標で比較できる。regionで切られた表示範囲や
   DWM extended frame boundsは比較に使わない。負座標を維持し、DPI awarenessも現行pumpと揃える。
3. query成功かつ4辺が要求と完全一致ならSetWindowPosを省く。
   query失敗・不一致では、従来のflags（NOACTIVATE/NOZORDER/NOOWNERZORDER）のまま一回適用する。
   「前回要求が同じ」「前回SetWindowPos成功」だけでは省かない。
4. 操作成功を実矩形一致のキャッシュへ変換しない。次要求でも実矩形を読むため、
   OSのclampや外部移動・サイズ変更後は同じ要求でも再適用される。query/適用失敗はログ対象、
   retry・別のpending・recreate・新しい復旧経路を作らない。

これでHWND再生成/F11/placement switch/monitor-DPI変更時も、旧要求の記憶がskipを作らない。
DpiChangedはsuggested rectを従来どおり要求し、後続GeometryChangedが別rectならそれも適用する。
要求順・coalesce・採用epochは変えず、DPI/ppp更新やsurface resizeをgeometry skipに連動して省かない。
外部操作が照合後に到着すれば既存の次の要求で再照合する。新しい外部監視・補正tickは足さず、
外部移動後に要求が来ない場合まで自動復元する仕様は追加しない。

コストはgeometry要求あたりGetWindowRect一回（perf OFFでも必要）。同一要求が少ない場合の
純増と、重複時に減るSetWindowPos/通知の両方を測る。WM_WINDOWPOSCHANGEDでcacheを管理する案は、
全外部変更・失敗・再生成の失効ownerを増やすため、D1では現在OS状態を照合する単純な案を選ぶ。

### 8.5 A3: 最終batchの可視性とGPU終端

`NativeEguiOverlayState::render_logical_once`から実detach closureと呼出しだけを除く。
`if !overlay_visible`内のseek/preview/frame-step cleanupとegui runは残す。
既存`NativeOverlayLogicalBatch::overlay_visible`を最終要求として使い、pending visible boolや
passごとのGPU状態を増やさない。`NativeEguiOverlay`が最終描画と失敗のownerになる。

| batch/終了結果 | 最終境界の処理 |
| --- | --- |
| logical passが0、dirtyなし正常return | 既存と同じ無描画。実attach状態を保ち、Commit/Presentを追加しない。regionの既存publishは維持する |
| 最終表示、成功 | 累積texture set、最終shapesのencode/submit/Presentを従来どおり一回実行。その後に`set_visual_attached(true)`を一回だけ呼ぶ。同値ならDComp操作なし。成功後に最終strip inventory等をcommitし、texture freeを完結する |
| 最終非表示、成功 | 空shapesでもegui deltaを捨てず、既存の透明clear/submit/Presentを一回維持。その後に`set_visual_attached(false)`を一回だけ呼ぶ。同値ならCommitなし。hidden inventory、texture free、command/window intentは既存の成功tailで完結する |
| 表示だがpaint jobが0 | `overlay_visible`とpaint job数を混同しない。既存どおり透明clear/Present後、最終visibilityへ適用する。前frameのpixelを残さない |
| prepare/logical/texture/acquire/submit/Present/attach-Commit等の失敗 | 成功inventory・成功commandを公開せず、当該batchを終端する。最終可視性適用が成功したように状態を書かない。通常の最終適用を重ねず、ownerを既存render fault終端へ渡してそのcore/target/rendererを破棄する |
| close/cancel/host lost、candidate abort/retire、device/surface終端 | 既存のwindow-host close/abort/retire/quarantineとrender core detach/dropで当該ownerだけを終了する。中断batchのdelta/commandを後継surfaceへ移さず、後続の再attachや追加Presentを作らない |

最終非表示のdetachはPresent成功後に揃えるため、GPU処理中の旧frameの見え方は従来の
「logical pass中に直ちにdetach」と異なる。batchを既に一回のPresentへ畳む契約に合わせる変更で、
新しい待機を入れるものではない。失敗時は成功tailへ進まず、古いframeを表示したまま
log-onlyで次のframeへ進むことも避ける。

実装では`NativeEguiOverlay`のbatch実行・単発`render_once`・`render_if_dirty`・resizeの
同じfinalizerを通す。最終可視性は必ず一回、再attachはPresent後だけとし、
`set_visual_attached`の既存boolは成功したAdd/Remove＋Commit後だけ更新する。
Commit失敗でDComp側の状態が不明になった場合はdropで終え、別のboolやrollback Commitを足さない。
途中失敗時のtexture freeだけを後継へ再送せず、renderer全体のdropが未完deltaの資源終端を担う。
正常hiddenのdelta freeまで省く最適化は行わない。

現在log-onlyの呼出し元（§8.1）は、A3対象のfinalizerが失敗した場合に既存の
`run_native_video_output`のErr→`render_fault`へ伝播させる。初期描画、入力batch、tick、
overlayを描くresize/設定更新/placement primeの全callerを確認し、prime失敗は既存の
candidate abortで旧coreを保持する。非描画の通常操作エラーを一括でfatalに変えない。
これにより資源が不確定なrendererを使い続ける経路を残さず、通知は既存output error経路を使う。
まれな失敗は既定の割り切り（ログ＋通知）とし、device再生成、surface再configure retry、
command再生、rollback、完了待ちを追加しない。

### 8.6 同型の入口・終了とdetached構造規則

| 経路 | 適用・保持する境界と検証対象 |
| --- | --- |
| main fullscreen / F11往復 | FullscreenBorderlessの独立HUDへA2、各overlayへA3。main内Childとの切替は新epoch/topologyでA1を分け、再表示・初回attachとHUD復帰を落とさない |
| F12 detached / F12→F11 | DetachedViewerChildは独立HUDなしのpresenter内overlayでA3。同じset_visual_attachedの`after_visual=Some(video)`を維持。F11で独立HUDへ移る場合も既存placement switchのhidden prime→publish/retireだけを使う |
| park / resume / promote / 兄弟窓 | context mount/depositだけでA1 counterやGPU状態をresetしない。outputが同じならidentity継続、target再生成ならepoch更新。同じpath/indexや共有DX12 deviceでもsiblingをdetach/region更新しない。旧output teardownと新configureの既存device gateも維持 |
| source switch / video→audio / VST shell / tray hide-show | source epoch変更とwindow epoch変更を区別。HWND非表示とDComp非表示を同一視しない。hidden consume-and-hold、音声transport、復帰時held frame、strip session資源を既存ownerのまま保持 |
| VST editor / popup / EffeTune | 明示hook・hover・WINDOWPOSの要求源のみ観測。allowlist、TOPMOST、owner、focus、HUD click-throughとeditor前面操作は変更しない。EffeTuneのownerless/非TOPMOST方針を維持し、一般VST allowlistへ加えない |
| 上下HUD / strip / 左右panel / modal / IME | 描画済み状態とregionを同じ最終結果に対応付けるが、各logical passの入力所有とhidden cleanupは維持。途中hiddenをstrip worker終了やIME cancelへ読み替えない |
| monitor移動 / DPI / Resize / 外部HUD移動 | A2の実rect照合へ全要求を通す。monitor/DPI snapshotはpump、adapterはrenderに残す。負座標・同サイズ位置移動・DPIだけの変更でも必要な既存ppp/surface処理は進む |
| close / cancel / destroy / prepare失敗 / device-surface終端 | exact epochのhost/coreで完結。staging失敗は旧表示先を変更せず、成功済inventoryや旧記録を新epochへ流用しない |

本設計はdetached述語・viewport生成・HWND捕捉・placement保存先を変更しない。
A2は既に所有しているHUD HWNDへの副作用を現在の実矩形で冪等化し、A3は既存render ownerの
「logical結果は一回の描画で公開する」契約の不一致を直す。geometry heuristic、新しい
detached bool/pending、host_lost再生成、時間窓、追加repaintで症状を覆わないため、
CodexのD1判断は「症状パッチではなく構造的修正」である。

この構造判断はD1 `66708856f` の独立レビュアーとClaudeCodeが明示的に合意した
（利用者からの引継ぎ、§8.1）。I1の対象範囲と理由は`detached-rework-plan.md` §11へ記録する。
実装差分がdetached述語/viewport経路へ広がる場合は
先に範囲を戻してレビューし、触れる範囲・理由・合意を`detached-rework-plan.md` §11へ記録する。
D1の「この文書だけを変更する」段階では同計画へ追記していなかった。

設計簡素化として、再生やwindow切替をmodal化／windowを閉じて再生成する案も検討した。
連続するHUD入力・VST操作・再生を制限し、不要な再生成を増やすため採らない。
代わりに「geometryは現在OS状態を一箇所で照合」「可視性の確定点は既存batch末尾一箇所」
として扱う組合せを減らす。A1の1秒は記録集約だけで、動作のdebounceには使わない。

### 8.7 テスト計画・受入条件

D1では製品テスト・buildを実行しない。実装時は担当者が次の証拠を一つの台帳に残し、
独立レビュアーは同じsource/条件の成功結果を再利用する。各改善を別のcoherent差分として検証する。

**A1（非対話・注入時計／記録sink）**

- OFFで追加のclock/query/format/集計が走らないことをspyで検査する。ONでは1秒境界、
  複数秒の無起床後、終了時の端数集約、8件を超える変化、coalesceと欠落を検査する。
  要求=適用とみなさず、origin/skip/Win32失敗と実変更の各counterを突合する。
- output A/Bの同epoch・再利用HWND・source切替・共有device・staging abortを同一sinkへ流し、
  keyと観測時刻/revisionが混ざらないことを検査する。実際のproducer→pump受信経路で
  VST/hover/WINDOWPOS由来が保持される回帰を置く。
- Commit/Present/surfaceの成功・失敗・無描画・hidden holdの記録をproduction境界で検査し、
  現行の全Commit/Present callsiteをrole別一覧と照合する。取得不能adapterはunknownを保つ。
  logger停止中もproducerが待たない既存回帰を再利用し、新規診断が同期I/Oを呼ばないことを確認する。

**A2（pump経路＋非表示の実HUD HWND）**

- `hud_window.rs`の既存`hidden_hud`はproduction class/wndprocを使いShowWindowしない。
  これをcfg(test)のhost注入fixtureへ拡張して`PumpRuntime::handle_command(Resize)`と
  stamped event→`drain_window_events(GeometryChanged / DpiChanged)`を通す。
  独立helperに同じ分岐を複製するテストでは済ませない。
- 3経路それぞれで同rectを反復し、最初の必要な移動は一回、以後のSetWindowPosは0回とする。
  A1のattemptと実wndproc通知を併用して、単に最終rectが合っているだけのテストにしない。
  NOZORDERでRequestRaiseHudが増えないv4.7.0回帰も維持する。
- 同サイズ位置移動、負座標、0寸法正規化、外部SetWindowPos後に同じ要求、別epochの再生成、
  DPI suggested→presenter geometryの連続、HUDなしtopology、退役event、兄弟host不変を検査する。
  query/SetWindowPos失敗は狭いOS-call seamで注入し、same skipに誤分類しないことを確認する。
  fixtureがproduction createのSW_SHOWNAを通らないことも検査する。

**A3（実CPU描画／実batch reducer＋最終GPU境界の記録adapter）**

- `NativeEguiOverlayState`の既存fixtureと`run_native_event_batch`を組み合わせ、
  実入力segmentで「表示→途中非表示→最終表示」を作る。既にattachedなら
  Present=1、RemoveVisual/AddVisual/可視性Commit=0とする。未attachedからは
  Present後のAddVisual/Commit=各1で、中間detach=0とする。
- 「表示→最終非表示」はPresent=1、末尾のRemoveVisual/Commit=各1、再attach=0。
  既にdetachedなら可視性Commit=0。最終非表示でもtexture set/free、command、
  IME/window intents、hidden cleanup、strip最終inventoryが完結することを検査する。
- 無入力・dirtyなしはPresent/attach/detach/Commit=0。visibleで空paint、単発render、
  resize、fallback tree、全panel/strip/固定chromeも同じfinalizerへ届くことを検査する。
  既存のwheel/文字入力/IME/escape/command attribution/repaint deadlineのbatch回帰を維持する。
- prepare/logical/acquire/最終Commit等の失敗とclose/cancel/abort/retireを注入し、成功inventoryの
  公開や後継への再attachがないこと、delta所有がそのrendererで終了することを確認する。
  既存headless batch targetだけでは実GPU呼出回数を検証できないため、production finalizerが
  使う狭いGPU操作境界を記録adapterで差し替える。GPU resourceのOption化やテスト専用可視性ロジックは作らない。

反復は上の新規回帰＋該当moduleの明示filterから開始し、通常core check、関連する既存
native window/input/overlay/placement/seek-strip回帰へ広げる。共有pump/render変更のD1最終gateは
`scripts/test-full.ps1`。I1では利用者の検証指定に従い、必要なtest-script回帰と
`cargo fmt --check`に加え、最後に`scripts/test-lib-full.ps1`で本体lib全件を実行する。
workspace/vendor/doctest全体の`test-full.ps1`は設計lead側の後続gateとして区別して残す。
UI文字列を変更した場合だけglyph検査も行う。検証用`build-dev.ps1`は通常は実装者が作るが、
I1では利用者の明示指示により省略し、設計lead側へ引き継ぐ（§8.9）。
これらのcommand・対象commit・環境・件数・exitと未実施理由を記録し、0件一致を成功としない。

実画面の受入は§6.4の了承済み隔離portable検証枠、または利用者起動の通常確認buildで行う。
目安20〜30分＋build待ち、desktop/マウス/キーボードを使用し、使い捨て動画と設定だけを使う。
agentが起動・入力するrunは別途、具体的suiteについて明示承認を得る。
ShowWindowを行う既存`hud_focus_claim_uses_registered_editor_and_preserves_presenter_return`等もこの枠へ分ける。
F11/F12/park-promote/兄弟窓、monitor/DPI移動、再生/pause、上下hover/固定、strip/左右panel、
VST/EffeTune表示・操作・非表示、IME/focus/Alt+Tab/最前面、close/reopenを§8.6の表に沿って確認する。
上固定だけで安定する等の報告条件は既知比較用に保持し、設定を強制しない。

受入は、(1)要求源から実操作まで相関できる低負荷診断、(2)3つのpump入口で実rectが同じ時だけ
geometryを省く回帰、(3)中間hiddenでDCompを動かさず最終hiddenも正しく閉じる回帰、
(4)自動gateと既存機能の実機受入、(5)独立設計/実装レビューとClaudeCode検収である。
perf OFFの追加診断処理がないこと、ONの行数上限とpump遅延/CPU・render所要時間に有意な
悪化がないことを前後同条件で測り、未測定のまま低負荷達成とは記載しない。
内部Present完了や回帰greenを、報告者>>570の物理点滅解消の証拠にはしない。

### 8.8 実施順・見積り・採らない案

| 単位 | 内容 | 実装・自動検証の概算 |
| --- | --- | --- |
| A1 | 相関ID/由来の既存route接続、owner別集約、geometry/region/DComp/Present/surface/adapter記録、解析回帰 | 1.5〜2.5人日 |
| A2 | 実rect照合、pump3経路の非表示fixture回帰、A1の要求/実変更突合 | 0.5〜1人日 |
| A3 | CPU副作用除去、最終可視性finalizer、全callerの失敗終端、batch/終了回帰 | 1〜2人日 |
| 共通 | 最終gate・関連文書・build・独立レビュー指摘対応・承認後の実機受入 | 1〜1.5人日＋実機枠 |

合計4〜7人日程度。実測工数ではなくfixture/失敗伝播の範囲を含む概算。
A1の最小相関を先に接続し、A2/A3の各差分を別々に回帰で固定する。全計装完了や点滅再現を
A2/A3の根拠ある改善の前提にはしない。設計レビューで所有境界の拡張が必要なら再見積りする。

- A4の実z-orderに基づくraise抑制、A5のegui/wgpu更新比較、B1〜B5、§7.4の回避策は今回の範囲外。
  origin/skipを記録するためにallowlistやburstを変えることも含めない。
- 同一「要求」rectのcache、rendererの同寸法skipだけに依存する案は、pump直通と外部変更を取りこぼす。
  geometry通知の全廃やDPI/resize抑止も採らない。
- 全logical passの一本化やhidden pass自体の省略は、入力/IME/command/deltaとcleanupの契約を変える。
  detachだけの時間抑止、後からrepaintで戻す案も所有境界の修正にならない。
- hidden時のPresent省略、device再生成/失敗retry、HUD HWND廃止や単一DComp tree化は採らない。
  後二者の構成比較はB5等の別設計で、A3を理由に混ぜない。
- 常時全frameログ、ログ専用tick/同期I/O/同期flush、物理ちらつきを内部ログだけで判定する案は採らない。

D1時点で追加の利用者仕様判断は不要と判断し、その後の設計レビュー・構造合意は§8.1のとおり受けた。
実装で通常操作の遅延/機能縮小/表示領域や画質変更が必要と判明した場合だけ、変更前に
費用・代案を示して利用者判断へ戻す。収録判断、実機未確認事項、点滅への効果は別々に報告する。

### 8.9 I1実装と検証台帳（2026-10-16、作業中）

実装先は `hud-flicker-1387`。対象はA1/A2/A3だけであり、A4のraise抑制、A5の依存更新、
B1〜B5の構成比較や新しい回避策は含めない。実装・自動検証・独立実装レビュー・ClaudeCode検収・
実機受入を分けて記録する。実装・本体lib全件の自動検証は完了したが、低負荷の実測達成や
報告者の物理点滅解消は主張しない。本台帳作成時点ではGit管理領域の書き込み拒否により、
I1担当によるcommitは未作成である。

| 対象 | I1で実装する境界 | 現在地 |
| --- | --- | --- |
| A1 | `NativeWindowHealth`が同じoutputの診断IDをpump/renderへ渡す。`native_diagnostics.rs`はownerごとの固定counter・scalar snapshot・観測時刻/revisionを保持し、既存の出口で約1秒ごと、詳細最大8件と種別別total/omitted/first/lastを既存perf journalへ送る。終了時も同じ上限で端数を出す | 実装・自動検証済み。OFF spy・注入時計/sink・同epochの複数output・省略/終端の回帰を追加。raiseの発行/置換/消費/burstを元sequence・由来で相関する |
| A1 adapter | `DeviceEpoch`の生成時だけperf ONで実DX12 deviceのLUIDとadapter情報をキャッシュする。D3D11側のLUIDと比較できる値を残し、query不能はunknownとする。診断値をdevice選択・loss判定へ使わない | 実装済み。通常core checkで型/APIを確認。実機のD3D11/DX12照合と生成時payloadの採取は未実施 |
| A2 | HUD geometry ownerが各要求で正規化済みscreen rectと`GetWindowRect`の4辺を比較し、完全一致のときだけ`SetWindowPos`を省く。query失敗・不一致では従来flagsの一回適用を維持する。Resize/GeometryChanged/DpiChangedを同じownerへ通す | 実装・自動検証済み。要求・前照合・試行・結果・操作後観測を別計数。geometryの入口とowner観測時刻を記録し、上流発行時刻まで追跡済みとは扱わない |
| A3 | CPU logical passからDComp detach副作用を外し、最終batchのPresent後に最終visibilityを一回適用する。中間hidden cleanup、egui delta、入力/IME/command、無描画returnは維持する | 実装・自動検証済み。production finalizerと成功tailを実CPU batch＋GPU記録adapterの回帰で共有する |
| A3 caller失敗 | GPU終端失敗を成功tailやlog-only継続へ流さず、既存render fault/target dropへ伝える。candidate prime失敗は既存abortで旧coreを保持し、非描画エラーを一括fatal化しない | 実装済み。初期描画・入力・tick・GeometryChanged/DpiChanged/同placement resizeの描画失敗を既存render faultへ接続。candidate prime失敗は既存abortを維持 |

固定tableが満杯の場合はcounter/snapshot/fieldの省略を明示し、journal欠落とは区別する。
診断はowner内だけに保持し、新しいtick/thread・同期I/O・flush待ち・共有診断mutex・GPU同期を追加しない。
1秒は記録区間の条件であり、geometryや表示・入力のdebounceには使わない。
`pump`/`pump_raise`/video core/HUD/background/cover等の論理ownerは別々に上限を持つため、
一つのoutput/epochで定常summaryが一行だけになるという意味ではない。
geometryの前照合値と、操作後だけ採った実矩形をmonitor/window snapshotにも再利用する。
video transformは同じ確定layout/targetから描画と診断へ渡し、診断用に配置を再計算しない。
明示detachは通常eventとし、video/HUDのsurface_dropと終端flushは各GPU ownerのDropで一度だけ行う。

実行環境は本worktreeのWindows/MSVC、通常featureと`test-script`、cargo-nextest 0.9.148。
対象はHEAD `66708856f` に対するI1差分である。10個の製品Rustファイルの実バイトhash一覧は
`target/hud-flicker-i1/source-manifest.txt`、その一覧をLFに正規化した文字列のSHA-256は
`81e206641d4e09eee9fee9789d6ac3e8a8ad34d87bcde5b6a76d14387a9a760a`。
全lib成功時のsource fingerprintは
`00d4d92f2663d042d92a5dba53f1e06bcaeecf9ba509ca5be9198a29717e05c9`。
実行後に更新したのは本書・`video-architecture.md`・`detached-rework-plan.md`だけであり、
製品コードやtestを変更していない。receiptを書き換えず、同じ製品source/条件の証拠を再利用する。

| 検証 | source / command / 環境 / 件数 / exit / ログ | 状態・限界 |
| --- | --- | --- |
| A1注入時計/sink・OFF spy・相関/集約 | helper 4件＋raiseのproducer/coalesce/bus/pump 4件成功。新規計14件と既存1件を選ぶ明示nextest filterは15件成功、exit 0。`target/hud-flicker-i1/restored-narrow.xml` | OFFではsample/query/format/追加時計/sinkを呼ばないspy、interval/終端・相関・overflowを検査。実perf負荷の測定ではない |
| A2 pump3入口・非表示HUD・外部変更/失敗/兄弟owner | 新規pump回帰2件成功、module 7件成功、exit 0。`target/test-focused/runs/20261016T031217421Z-f7e8a11d/` | production class/wndprocの非表示HWND、実pump command/event経路、試行数と位置通知を検査。実monitor/DPI移動は未確認 |
| A3実CPU batch・GPU記録境界・終端/失敗 | 新規4件成功。実logical CPU passとproduction `finalize_native_overlay_gpu_batch` / `apply_native_overlay_visibility` / `commit_native_overlay_presented`を共有 | 中間hidden・最終hidden・無描画・操作失敗・free/command/intentsを検査。最終inventoryのVisible→Hidden公開と失敗時の旧inventory維持も最終feature/全libで確認 |
| 関連native input/window/overlay/placement/seek-strip回帰 | `scripts/test-focused.ps1 -Path`でdiag/render_core/overlay_gpu/hud_window: 4+169+3+2件。pump/window/host/health/strip/strip_layout/host_contract: 7+19+5+4+56+17+11件。両run exit 0 | 計297実行（hostとhud_windowの2件は重複）。ログは`target/test-focused/runs/20261016T030955978Z-63bc5c32/`から`20261016T031231314Z-3fe1527c/`の各run |
| 通常core check・fmt・diff・test-script | `cargo check -p mimageviewer --bin mimageviewer-core`、引数なし全workspace `cargo fmt`、`cargo fmt --check`、`git diff --check`: exit 0。`test-focused.ps1 -Features test-script -Path`はrender_core 173＋pump 7＋window 20＋native_ui_smoke 47件成功、exit 0 | 最新inventory assertを含むrender_coreは`target/test-focused/runs/20261016T032016232Z-bffc992f/`で再確認。UI文言変更なしのためglyph検査は対象外 |
| `scripts/test-lib-full.ps1`本体lib最終gate | `$env:NEXTEST_TEST_THREADS='4'; .\scripts\test-lib-full.ps1`: **13,245成功、59除外、0失敗、1295.369秒、exit 0**。`target/test-lib-full/runs/20261016T033708384Z-c2ef5c2b/`。receipt: `target/test-lib-full/144fd8232b5368b881b7b8fc904e40f0fa5daaa408a936ea628ecc1f4e11589d.json` | I1の明示的なsuite実行指示に基づき、defaultの非activate fixtureを含めた。実foreground切替/OS入力を行うignoredテストは従来どおり除外。再試行回数0・assert/timeout/ignore変更なし |
| `scripts/test-full.ps1` workspace/vendor/doctest | I1では未実施 | I1の利用者指定は本体lib全件。後続gateとして設計leadへ引き継ぐ |
| 独立実装レビュー・ClaudeCode検収 | I1担当内の独立`gpt-6.1-sol` / `xhigh`ソースレビューは指摘対応後ACCEPT。変異XMLと復元greenも照合済み。その後の完成時独立レビューはA1のP2二件によりREVISE（利用者引継ぎ、§8.10） | D1 ACCEPT・担当内レビュー・完成時レビューを区別する。実機受入は未実施 |
| perf OFF/ON前後の行数・pump遅延/CPU・render所要時間 | 未記入 | 未測定。低負荷の達成は未判定 |
| 確認用binary・実画面suite | binary作成/起動とも未実施 | 利用者の明示指示でI1担当の`build-dev.ps1`を省略し、設計lead側で後に扱う。agent起動・入力はsuiteごとの明示承認が必要 |

変異確認ではA2の実rect一致gateを一時的に無効にし、A3のproduction finalizerの可視性適用を
一時的にbypassした。同じ実装境界を使う6件の選択で、A2のpump回帰はSetWindowPos試行が
期待1回に対し5回となって失敗し、A3の4件もAdd/Remove/Commitや失敗伝播の欠落で失敗した。
無関係の既存1件は成功し、合計5失敗・exit 100。`target/hud-flicker-i1/mutation.xml`へ保存した。
両変異を戻し、A2はbyte単位の元hash一致、A3は元blockの復元と変異marker消失を確認した。
操作回数/順のassertを保ったままinventory assertを強化し、その後のfeature回帰と全libも成功した。

初回全libは13,207成功・38失敗・59除外、575.674秒、exit 100
（`target/test-lib-full/runs/20261016T032119853Z-6cda44ac/`）。37件は
`vendor/raw-samples`未配置、1件は変更していないmask previewの100ms閾値に対する189.8049msだった。
既存ローカルの正規CC0 sample 27個（360,971,087 byte）を`tests/raw-samples.json`のSHA-256/sizeへ
照合して実copyし、失敗38件を`--test-threads 4`で再確認したところ38成功、143.143秒、exit 0
（`target/hud-flicker-i1/fixture-recheck.xml`）。製品コード・assert・除外を変更せず全件も再実行した。
また初回build前に不足していたLibRawの固定archive由来build inputを復元し、FFmpeg DLLを
`vendor/ffmpeg/bin`から`target/debug/deps`へ実copyした。いずれも無視対象の作業tree前提であり、
依存versionの更新やreparse pointの追加ではない。

本台帳作成時のI1担当のcommit操作は、共有Git管理領域
`C:/home/mimageviewer/.git/worktrees/mimageviewer-hudflicker/index.lock`の作成が
Permission denied（exit 128）となり未作成。権限やhookを変更して回避していない。
許可された通常shell用に、英語の本文と明示path指定の2件のcommit手順を
`target/hud-flicker-i1/commit-i1.ps1`へ用意した。新規fileだけ先に`git add`し、
branchと対象file hashも確認する。`git add -A`、master統合、pushは行っていない。

実画面では§8.6/§8.7のF11/F12・park/promote・兄弟窓・monitor/DPI・HUD/panel/strip・
VST/EffeTune・IME/focus・close/reopenを確認する。現時点では実機結果を記録していない。

### 8.10 I1完成時レビューのA1 P2二件への対応（2026-10-16）

利用者引継ぎ: 設計担当がI1を検証し、製品`53c57394d`・文書`5c9a83ada`としてcommitした。
完成時の独立レビュー（`gpt-6.1-sol` / `xhigh`）は**REVISE**。A1の下記二件を設計担当も
妥当と判断し、修正・production snapshot回帰・変異検証を指示した。A2/A3には修正要求がなく、
detachedの変更も構造的修正との明示合意を得た。§8.9の担当内ACCEPTと後続REVISEを混同しない。
今回の対象は`5c9a83ada`からの診断内容の修正だけで、commitは利用者指示により試みない。

1. **surface交換の適用済みtransform**: `present_with_surface_swap`と
   `update_video_visual_transform`を共通の`commit_video_visual_transform`へ通す。
   描画のために一度算出したtarget/行列を実COM closureへ渡し、Commit成功後だけ
   同じ値とsurface寸法/revisionを`video_transform`へ記録する。swapは確定前のnew値を使い、
   geometry-only更新も同じsample形式で記録する。失敗時は古い適用済みsnapshotを保つ。
   SetContent→SetTransform2→Commit、既存の待機、surface状態確定/退役の順序は維持し、
   診断用の再計算・Commit・GPU待機は追加しない。
2. **HUD各部の実描画rect**: `NativeOverlayDrawFacts`へ上下バー・左右panel・stripの
   固定5個のoptional rectを追加する。描画helperが実際にpaintへ渡したrectを採り、
   logical output→最終batch→productionの`record_native_overlay_drawn_rects`へ渡す。
   popup類の既存rectと同じHUD client物理pxで記録し、非描画部位はfalseにする。
   各logical passとegui再passで初期化するため、最終hiddenへ中間の矩形が残らない。
   drag時の全面入力region・hover・可視性boolから位置を推定しない。描画/入力所有・IME・
   コマンド・delta・A2/A3の契約は変更しない。

固定counter 128・snapshot 32・field 32・詳細8件/owner/区間は変更なし。
`drawn_rects`はpppと17部位の計18fieldで上限内に収まる。perf OFFではrect採取を無効にし、
sample closure/JSON/時計/同期I/O/新しいtickを追加しない。診断helperへのtest-only注入で
production snapshot tableを直接検査し、process-global perfやGPU/実アプリ起動を使わない。
detached述語・viewport/placement経路は変更せず、既存render owner内の診断の完備化である。
まれなGPU失敗は既存のログ＋通知・終端を維持し、回復機構は追加しない。

新規回帰は、productionと共通のcommit境界・snapshot tableを使う
`diagnostic_snapshot_tracks_surface_swap_applied_transform_and_failed_commit`と、実CPU描画・
最終logical output・production snapshotを通す
`diagnostic_snapshot_contains_final_painted_hud_rects_independent_of_drag_region`の2件。
前者はSAR/90度回転/viewport/surface/revision更新、実COM closureへ渡した6成分、
target、geometry-only更新、失敗時の前snapshot保持、OFF時の描画継続を検査する。
後者はpaint shapeとの一致、ppp 1.5での物理px、strip有無と通常seek行Show/Hide、
全面drag regionとの独立、最終hiddenへの置換、表示中OFFの5rect未採取、field省略なしを検査する。

| 検証 | 結果・証拠 | 限界 |
| --- | --- | --- |
| 新規snapshot回帰の正常版 | 明示nextest filterで2成功・0失敗、exit 0。`target/hud-flicker-i1-a1-review/baseline.xml` | GPU/実アプリは使わず、production境界のCOM closureを記録adapterへ置換 |
| 記録内容の変異 | surface revision 0以外のtransform snapshot更新を一時停止し、HUD5部位の記録をNoneへ置換。上記2件がそれぞれ旧snapshot残留・top_barのfalseを検出し、2失敗・exit 100。`target/hud-flicker-i1-a1-review/mutation.xml` | 描画/COM操作は変異させず、診断の欠落を検出することを確認 |
| 変異の復元 | `render_core.rs`を保存byteへ戻し、SHA-256 `86c1d9bd663f29b89ceb570f8e65ade9dd7c5e54f2304df8d41cfce35861987e`の一致を確認 | 復元後focusedと本体lib全件も成功（下記） |
| 復元後の通常focused | `scripts/test-focused.ps1 -Path`で変更した3Rust fileを指定。診断4＋render_core 171＋overlay_draw 43＝218成功・0失敗、exit 0。ログは`target/test-focused/runs/20261016T044407805Z-a7e0707c/`、`20261016T044741921Z-cba48c7b/`、`20261016T044748949Z-5c438d6a/` | 新規2回帰の復元後成功を含む。全libの代用にはしない |
| 復元後のtest-script focused | 同じ3fileを`-Features test-script`で指定。診断4＋render_core 175＋overlay_draw 45＝224成功・0失敗、exit 0。ログは`target/test-focused/runs/20261016T044849359Z-2d265428/`、`20261016T045316327Z-d56329d7/`、`20261016T045323714Z-c8c66171/` | feature固有のlogical output/UI inventory・draw helperも確認 |
| 整形・通常core check | 引数なし全workspace `cargo fmt`、`cargo fmt --check`と`cargo check -p mimageviewer --bin mimageviewer-core`: exit 0（check 17.62秒）。`git diff --check`: exit 0 | UI文言変更なし。glyph検査、build-dev、製品起動は実施しない |
| 本体lib全件の最終gate | `$env:NEXTEST_TEST_THREADS='4'; .\scripts\test-lib-full.ps1`: **13,247成功・59除外・0失敗、1221.054秒、exit 0**。通常feature / test profile / nextest default、retries 0。`target/test-lib-full/runs/20261016T045504307Z-e7e422f2/`。receiptは`target/test-lib-full/cf4a4f562095c0c1c542d11a2c724a83f26b15aae60084376e6dfeadbb45ba7a.json` | JUnit総数13,306・除外59・failure/error 0、新規2回帰の成功、receiptのsource/件数/exitを照合。今回の全件完走は一回。workspace/vendor/doctest・実機/負荷測定は含まない |

全lib成功時のsource fingerprintは
`7b95f029ab99cf73fdab1f91477a12f0f145f59823b95f6565a3365d985ed355`。
変更した製品Rust 3fileのbyte hash一覧は`target/hud-flicker-i1-a1-review/source-manifest.txt`、
そのfile自体のSHA-256は`46c2578a08c9aa0d3f3a476241eadd16dcde1a42886dba246a8b75d5096df9eb`。
全lib前後で3fileとも一致を確認し、実行中は文書も含めてfreezeした。実行後の更新は本節の
検証記録だけで、receiptを書き換えず同じ製品sourceの成功証拠として扱う。
今回の変更は製品Rust 3fileと本書・`video-architecture.md`の計5file。
利用者指示によりcommitを試みず、設計担当へ変更一覧と英語commit案を引き継ぐ。

新規HUD回帰の初回はfixtureの誤前提（strip中に下バー全体が消えると想定）で失敗した。
正本`VideoSeekBarWithStrip`と`resolved_video_bottom_bar_height`を確認し、Showでは64pt、
Hideでも操作行40ptが残る既存仕様を検査する期待値へ訂正した。製品の描画をテストへ合わせて
変更していない。初回XMLは`target/hud-flicker-i1-a1-review/iteration-strip-default.xml`。
誤前提の中間版の再compile一回は中断し、成功証拠に数えない。

今回の独立`gpt-6.1-sol` / `xhigh`レビューでは、旧ACCEPTを流用せずこの差分とtest修正・文書を
確認し、正常2件・変異2件失敗のXMLと復元byte SHAも照合して指摘なし・ACCEPT。
復元後の通常218件・test-script 224件と3file hashも独立照合済み。
本体lib 13,247成功・59除外・0失敗のreceipt/JUnit、新規2回帰の成功と現sourceのhash一致も
独立照合し、今回のA1修正・回帰・自動検証証拠・文書は未解決指摘なし・最終ACCEPT。
完成時レビューのREVISEを受けた再修正であり、設計担当の
検収・実機/負荷測定・点滅解消確認は別の受入として残す。
