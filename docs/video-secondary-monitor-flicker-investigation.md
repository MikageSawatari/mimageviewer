# 動画のサブモニター点滅調査（5ch >>557〜570）

調査日: 2026-10-15。対象報告は v4.6.0、source inspection は master
`ddcc0166d`。製品アプリを起動せず、コード確認と非表示の独立 Win32 probe を実施した。
報告者の物理画面の点滅は開発側で再現できていない。**原因確定・修正完了の記録ではない。**

2026-10-16追補: §3・§6の自己通知対策はv4.7.0で実装済み。修正前の経緯はそのまま残し、
残存報告と以後の全対策候補は§7を正本とする。バックログは§1.387。

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
本追補で製品コード変更・実画面比較は実施しておらず、本項目の対策を開始済みとは扱わない。
§1.382自体の独立した進捗は更新担当が管理し、A5の点滅比較の実施・成功と混同しない。
