# §1.115 linked detached 静止画の再確認計画

作成: 2026-09-12

対象: `docs/next-release-backlog.md` §1.115

状態: 現行ソースの構造確認と現行バイナリでの利用者目視再確認を完了。独立レビューと親の検収により、製品変更なしで今回完了。

**2026-09-12 17:00追加報告は§1.227へ分離・保留**: 上記は旧連続ちらつきの判定。メインと別窓の両方を最大化すると
open直後に画像とHUDが約2フレーム横へ伸びる別症状を録画で確認した。単発前面化の許容には含めない。
以下の正常経路のpresent順は再利用するが、最大化先のsurface寸法まで一致したframeを保証するかは未確認。
最大化/resize/present境界の再調査で中〜高リスクと判断し、利用者の意向と独立レビューに従い次版から除外した。
旧§1.115の完了は維持する。[§1.227調査記録](section227-maximized-first-frame-stretch.md)を参照。

## 1. 今回確かめる現象

§1.115 は、F12 でメイン表示から切り替えた linked detached 静止画窓をタイトルバーから
最大化し、Enter で画像表示を閉じてメイン一覧から Enter で再び開くと、一覧と別窓が何度も
入れ替わるように見えた問題である。

次の経路は対象外である。

- 「複数ウィンドウ（編集機能なし）」で画像を開くたびに独立窓を増やす経路
- 動画の Enter（再生 / 一時停止）
- F11 の装飾なし全画面。§1.115 の増幅条件はタイトルバーの最大化であり、F11 は別の状態である
- Ctrl+↑/↓ など、同じ detached host を保持するフォルダ内 viewer-to-viewer navigation

## 2. 既に確定していること

### 2.1 旧記録から再利用できる事実

- 旧ログの open / close はすべて利用者の Enter 押下だった。内部イベントによる自動再openと、
  `Enter:down` の重複配送という初期推定は訂正済みである。
- タイトルバー最大化が症状を見えやすくする条件だった。portable / normal のbuild flavor差が
  原因ではない。
- viewport teardownを理由に5フレーム描画を捨てるfont-atlas resyncは、commit `1457b670b` で
  lifecycle producerごと撤去済みである。現行の `main_font_update_pending` は実際のUIフォント
  設定変更だけを扱う。5フレームresyncを「残件」として扱わない。

この旧証拠は、当時の症状、入力の意味、増幅条件の確認には使える。旧v3.2.0の動画、ログ、portable
結果は、現行バイナリが直ったことの証明には使わない。

### 2.2 現行ソースのhost lifecycleと表示順

- Enterによる真のcloseとF12 OFFは、linked detached session / runtimeをterminalに終了できる。
  次のopenやF12 ONで新しいwindow id / HWNDを割り当てることは現契約上許されており、identityが
  変わるだけでは失敗ではない。
- 内部folder navigationは別契約で、同じdetached host、generation、placement、F11 intentを保持する。
- detached hostの新規生成 / 再生成は、positionとsizeを持つhidden builderで開始する。
  hidden builderへ`maximized=true`は入れない。Windowsではmaximize要求自体がHWNDを表示するため、
  保存済みmaximize intentは後段のvisible commitへ移されている（commit `e627f4fcc`）。
- hidden scaffoldは利用者のplacementをpublishできない。可視hostだけがcurrent placementを更新する
  （commit `488372688`）。F11 borderless intentと復元placementはF12のhost lifetimeから分離済み
  （commit `a98518ebd`）。
- 静止画child viewportのcallback内で、実画像、サムネイル、holdover、失敗表示、またはdark loading
  surfaceを描いた後に`Maximized(true) -> Visible(true)`をqueueする。maximizeが最初のvisibility edgeで
  ある点もこの順序に含める。
- backendのdeferred viewport処理は、正常にsurfaceを取得できたframeではcallbackの出力をtessellateし、
  `Painter::paint_and_update_textures`でGPU queue submitと`output_frame.present()`を完了してから、
  `handle_viewport_output -> process_viewport_commands`で`Maximized` / `Visible`を適用する。
  よって正常描画経路では、有効なsurface frameのpresent前にmaximizeでhidden hostを見せる順序には
  なっていない。画像未準備ならpresent済みdark loading frameを先に見せるため、短い暗転と
  未初期化の白 / 既定サイズhost露出は分けて判定する。
- `paint_and_update_textures`がsurface不在・取得失敗を返す経路にも、現状は後続command処理へ進み得る。
  applicationログのvisibility releaseはGPU完了やDWM表示完了のtyped ACKではない。今回の目視では
  そのfailureを示す露出を観測していないが、全failure pathまでACKで強化済みとは扱わない。

正常描画経路の構造は、§1.115の旧完了条件にある「terminal recreateを採るなら、新hostをhiddenのまま
surface準備し、その後1回だけ可視化する」という方式に該当する。DWMが実際に何を見せるかと
surface failure時の動作はソースだけで完了判定せず、次の最大化Enter反復で確認する。

## 3. 現行バイナリでの目視手順

所要時間は約10分。現在起動中のアプリをそのまま使える。新しいbuild、portable、環境変数は不要。

1. 環境設定の「全体設定」→「ビューワモード」を
   **フル機能ウィンドウ（編集機能あり）**にする。
   **複数ウィンドウ（編集機能なし）**は使わない。
2. 通常の静止画像があるフォルダをメイン一覧で開き、1枚選んでEnterを押す。画像がメイン側の
   viewerに開くことを確認する。画像を開いただけで独立窓が毎回増えるなら対象外の経路である。
3. 画像表示中にF12を1回押す。画像がメインから**1つだけ**の連動別窓へ移り、メインは一覧を
   表示することを確認する。これが§1.115の開始状態である。
4. 対照として、必要なら通常サイズのまま「別窓でEnterを1回→メイン一覧でEnterを1回」を1周する。
   通常サイズで安定することを確認する。
5. 別窓のタイトルバー右上の最大化ボタンで最大化する。F11は押さない。
6. 最大化別窓にフォーカスし、Enterを1回押す。別窓が閉じ、メイン一覧だけが見えることを確認する。
   約1秒待ってからメイン一覧でEnterを1回押す。最大化された別窓が1つだけ開くことを確認する。
7. 手順6を5周繰り返す。各キーの間を約1秒空け、1回の物理入力と1回の画面遷移を対応させる。
8. 最大化別窓が開いている状態でF12を1回押し、画像がメイン側viewerへ移ることを確認する。
   メイン側viewerでF12を1回押し、最大化別窓が1つだけ戻ることを確認する。これを3周する。

Enterで画像表示を閉じた後はfullscreen自体が終わる。その後の再開はメイン一覧のEnterで行う。
close直後の一覧でF12だけを押して再openさせる手順ではない。

### 3.1 2026-09-12 利用者確認結果

- 使用したのは2026-09-12時点で利用者が稼働させていた現行バイナリ。exact executable SHAは
  確認前に採取していないため不明であり、後から別成果物のSHAを対応付けない。
- 最大化linked detached静止画のEnter close→メイン一覧Enter openを反復し、利用者回答は
  **「移動はスムーズ」**。旧報告の一覧と別窓が何度も入れ替わる連続ちらつきは再現しなかった。
- 最大化中のF12では、ときどきメイン窓が一瞬前面へ出るように見えた。複数回の往復や白いhost露出ではなく、
  表示先を分離・移譲する1回の切替であれば許容範囲、と利用者が明示した。
- 目視で旧症状を再現しなかったため、診断環境変数付きの再起動と追加buildは行っていない。

## 4. 合否

合格条件:

- 1回のEnter / F12に対して表示遷移は1回だけで、無入力の自動再openがない
- rootと別窓が複数回交互に点滅せず、同時に複数のlinked detached窓が見えない。F12の表示先移譲で
  メイン窓が1回だけ前面へ出ることは、利用者が今回許容した範囲に含む
- 新しい別窓が可視になる前に、白いclient、既定小サイズ、復元前の非最大化窓が露出しない
- 再open / F12 ON後もタイトルバー最大化状態を保持する
- close / recreateがfont update要求やdiscard passを作らない
- F11を使っていないのにborderless表示へ変わらない

次は単独では失敗に数えない。

- 真のclose / F12後にwindow idやHWNDが変わること
- 画像が未準備のとき、テーマに沿うdark loading surfaceが短く見えること
- 最大化中に復元用windowed rectを別に保持すること

## 5. 再現した場合だけ使う診断ログ

目視で再現しなければ、通常の確認に診断環境変数は要らない。再現した場合、または再生成経路を
記録として残したい場合だけ、同じバイナリを次の環境変数付きで起動して手順6を1〜2周する。

```powershell
$env:MIV_DETACHED_WINDOW_DEBUG = '1'
Start-Process -FilePath '<現在使っている mimageviewer.exe のフルパス>'
```

`MIV_DETACHED_WINDOW_DEBUG`はprocess内の`OnceLock`で起動時に一度読む。既に起動しているアプリへ
後から有効化できないため、この段階だけ利用者自身が現在のmImageViewerとトレイ常駐を終了する。
同じsingle-instance mutexを持つ別processへ入力を誤送信しないためにも必要である。新しいbuildは不要。

ログは通常 `%APPDATA%\mimageviewer\logs\mimageviewer.log` にある。確認時刻を控える。
`[active-detached-session]`、`[viewport] cleanup_visible_false`、`[detached-window-debug]`の
`open_visibility_probe stage=before_show / visibility_release / after_show`、
`[presentation-window]`を使い、次を区別する。

- F12 linked sessionを通ったか、常時新規の複数ウィンドウ経路へ入ったか
- 1回の真のcloseに対しcleanupが1回か
- terminal recreateならwindow id / HWNDがどこで変わったか
- hidden scaffold後、描画callbackからmaximize / visible commitが1回だけ発行されたか
- font lifecycle resync / discardが復活していないか

アプリログはcommandをqueueした時点を記録し、DWMの視覚結果そのものは記録しない。最終判定には
目視結果を併記する。PowerShell sessionを閉じれば上の環境変数は通常起動へ残らない。

## 6. 判定後

- 現行バイナリのEnter反復で旧連続ちらつきは再現せず、移動はスムーズとの利用者確認を得た。
  F12で時々見えるメイン窓の一瞬の前面化も、1回の表示移譲なら許容された。現行ソースの正常surface
  経路におけるpresent-before-visible順と合わせ、§1.115は後続の§1.139修正で同じ可視化原因が
  解消されたものとして**今回完了**と判断する。host reuseの追加実装は要求しない。
- 再現した場合はログと時刻から、誤経路、複数入力、ready前visibility、placement intent消失を分ける。
  delay、追加repaint、最大化解除、保存停止、App-level bool、rect heuristicは修正候補にしない。
- 現行unit testはfont lifecycle作業なし、hidden builder、maximize/visible順、hidden placement拒否、
  F11/F12 intent、terminal recreateを固定している。DWMの見え方を含む今回の手動結果は代替できない。
- 最大化中もruntimeへ同じwindowed restore rectと`maximized=true`を再設定する内部no-opは残っている。
  現行setterは値が変わらない限りpresentation command、generation更新、repaint、settings永続化を発行せず、
  今回の連続ちらつきの原因ではない。現在ジオメトリとrestoreジオメトリをさらに明示的な別型へ分ける
  設計課題を扱う場合は、§1.115の再発修正ではなくplacement所有権の独立残件として評価する。
- surface不在・取得失敗でもvisibility commandを処理し得る経路をtyped present ACKで閉じる強化も、
  今回観測されなかったfailure pathの独立残件である。今回の目視合格をその経路の証明には流用しない。
