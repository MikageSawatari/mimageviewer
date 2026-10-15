> 状態: 設計案 第3版 (2026-10-16、再レビューへの対応、確認前)。目標は利用者決定により egui 0.37 系。正本の backlog は §1.382。

# 改訂履歴

## 第3版 — 2026-10-16

対象は第2版 `b693945bb`。再レビュー全文と該当ソースを読み取り専用で照合した。本版の再レビュー、実装、ビルド、テスト、製品起動は未実施。

- **再レビュー P2-1 — 反映:** SW_HIDE、最小化、visible child、RedrawRequested期限超過を区別。全入力共通の受付・終端配送、eguiの「clickを作らないrelease」API、backend保留入力のフィルタを必須設計から撤回し、vendor台帳・段階・見積りを縮小。
- **再レビュー P2-2 — 反映:** ゲームパッドの物理イベント取込み・状態更新とUI操作配送を分離。短い最小化・復帰もgilrs取込み側の局所的な受付判断で扱い、WH_GETMESSAGE由来pendingを入力源一覧へ追加。
- **再レビュー P2-3 — 反映:** hide時の`pending`／`frame`／`operation_frame`一律pruneを撤回。受信時点のviewport所有、focus喪失後も有効な離散キー、既存release確定を保持し、これらを変更する案は利用者相談事項へ明記。

再レビューの「推奨する最小設計」を採用する。前回P1-2のうち、未消費RawInputの保留という事実は維持するが、それを根拠に通常のトレイ退避へ汎用遮断機構を追加した判断は撤回する。

## 第2版 — 2026-10-16

対象は初版 `226985787`。以下の対応のうち、P1-2の対策範囲は第3版で訂正した。

- **P1-1 — 反映:** TexturesDeltaの適用・drain・引継ぎ・意図的破棄を経路別に定義し、段3の必須移行へ追加。
- **P1-2 — 反映後に訂正:** backendとWin32 queueへ汎用の非表示受付・終端機構を設計したが、第3版で撤回。実入力源と既存所有契約に絞る。
- **P2-3 — 反映:** 無描画配送にもqueue submissionと適用後freeの完結条件を追加。
- **P2-4 — 反映:** unsmoothed wheelの取得元、単位換算、軸変換、イベントごとの修飾キー、消費順を定義。
- **P2-5 — 反映:** Ctrl+Shift+C/X/Vの従来操作保持を推奨し、通常窓・native入力欄の共通契約と相談事項へ追加。
- **P2-6 — 反映:** 実ウィンドウを起動する既存テストを自動gateから分離する準備段と、承認・隔離条件を追加。
- **P2-7 — 反映:** accessibility修正を候補画像生成より前へ移し、両integration targetとlib snapshotを更新対象化。
- **P3-8 — 反映:** 通常版immediate viewportの段階別時間・delta bytes・遅延ログを独立したvendor保持項目へ追加。
- **P3-9 — 反映:** 0.33.3上の準備段を分割。固定の「4票」とPNGごとの重複記録を撤去し、画面群の共通理由と例外を一台帳で管理。

# egui 0.37 更新の影響調査と段階設計

## 0. 調査範囲と結論

調査・改訂日: 2026-10-16  
対象: `G:\home\mimageviewer-egui036`、ブランチ`egui-036`  
製品ソースの基準: `05b6a15de26897369a35e5aa00377d174ca3760d`  
初版保存: `226985787`  
第2版保存・今回照合時HEAD: `b693945bb`

リポジトリ、指定された更新履歴要約、Cargo registryの0.33.3／0.37.0、Rust 1.99の既存checkログ、初回レビュー・再レビュー全文を読み取り専用で照合した。ファイル変更、依存解決、ビルド、テスト、ネットワーク利用、製品起動は行っていない。

以下の実行時影響はコードからの見立てである。既存文書に記録された他担当の結果と、今回のソース確認を区別する。

目標は利用者決定によりegui系0.37.0。実装開始時に0.37.xのパッチ版へ合わせる場合は、該当版のソースで判断を再照合する。

主な結論:

1. Appの責務分離、root Ui、入力イベント、texture delta、wgpu surfaceの移行が必要。
2. `vendor/egui-winit`は削除できない。本家にHidden／Forwardedと同等の配送保証がない。
3. `vendor/eframe`も削除できない。非表示scheduler以外の製品・診断差分がある。
4. `ui_fonts.rs`の外形・行計測は既にttf-parser。ab_glyphを直接使うフォントプレビューとは分ける。
5. 動画本体はD3D11 native presenter。wgpu更新の主対象はHUD overlay、比較、panorama、静止画縮小。
6. `raw_scroll_delta`は、その消費位置で未消費のイベント列から作るunsmoothed投影へ移す。
7. **TexturesDeltaは非空のままdropできない。** 描画・無描画・早期終了・headlessの各消費責任を段3で成立させる。
8. **SW_HIDEと最小化は同じ経路ではない。** 未消費RawInputは本家どおり保持し、logicから古い入力を操作として読まない。汎用の非表示入力機構は追加しない。
9. ゲームパッドは独立threadから継続発生するため、物理状態更新と操作配送を分け、取込み時点の受付判断を局所化する。
10. snapshotは現ツリーで445枚。accessibility修正後に両integration targetとlibを更新する。
11. f32 fallback警告は既存ログで445件。多くは0.37の明確な引数型で解消する見込みなので、旧版上で一括修正しない。

### 参照表記

本家の`egui-0.37.0/src/...:line`等は、次のregistry rootからの相対パス。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\
```

mIVの行番号は今回照合した製品ソースに対するもの。件数は特記しない限り`src`、`crates`、`tests`、`tools`のRustソースの一致行数で、コメントを含む。ASTによる呼出数ではない。

## 1. 不変条件と簡素化方針

- 非表示でも、必要なwakeに応じてトレイ復帰、非同期完了、Remote、保存終端、メディア制御を進める。常時tickは追加しない。
- 正当に受信済みの入力は、受信時点の所有者と順序を保つ。未消費という理由だけで失効させない。
- viewportへ配送済みの離散キーは、その後のfocus喪失だけで取消さない。現在のキーlevelを読む許可とは分ける。
- release、focus、IME終端、復帰後の新規入力、表示中の兄弟viewportの入力を保持する。
- ゲームパッドは、操作先のない期間に取り込んだ押下を復帰後の操作開始にしない。物理状態のdrain・release・軸・disconnectは止めない。
- viewportごとの入力、IME、cache、texture、要求世代、workerの所有を維持する。
- popupが所有したwheelは、終了frameと平滑化残量を含めて背面へ漏らさない。
- texture deltaは、適用・drain、既存ownerへの引継ぎ、対象rendererの明示終了のいずれかへ到達させる。
- queueへ書いたuploadはsubmissionまで完結させ、使用中textureはsubmit前にfreeしない。
- フォントfallback、選択設定、リリース済み機能・データを保持する。
- snapshot更新で入力・到達性・所有権の回帰を承認しない。

状態の組合せを減らす案を先に比較した結果は次のとおり。

| 対象 | 採った案と理由 | 採らなかった案と理由 |
|---|---|---|
| 非表示処理 | serviceをlogicへ分離し、backendの未消費入力保持と既存入力ownerを利用する | 全入力共通の「通常／終端のみ」配送。SW_HIDEの実経路と既存契約から必要性を示せず、状態を増やす |
| 受信済みキー | event時点のviewport所有と既存permitを保持する | hide時の一律prune。focus喪失後も離散入力を採る既存契約を壊す |
| ゲームパッド | 物理取込みを一つのownerへ分離し、既存非前面抑止を取込み元の事実と接続する | 全入力源への受付層拡張、復帰時の現在foregroundだけでの選別。前者は過大、後者は短い退避中のpressを識別できない |
| texture delta | 描画・無描画を共通の配送・完結処理へ揃える | 各経路の借用適用と最後のclear。消費・submit・freeの責任が分裂する |
| wheel | 消費位置の未消費eventsから純粋に投影する | raw snapshotからの復元cache。消費済み入力が復活する |
| snapshot | 画面群の共通理由と例外を一台帳で管理する | PNGごとの重複票、固定数のレビュー票。確認範囲を増やさず記録だけ増える |
| 変換・保存・パスワード | 既存モーダル境界を維持する | 更新を理由に並行操作を増やす専用経路 |
| 非表示中のUI | 本家のpass省略と既存wakeを使う | UIを回し続ける、窓を作り直して入力を捨てる。必要性がなく、消費電力・状態・操作を変える |

新しいpending bool、時間窓、retry、追加repaint、silent fallbackは設けない。ゲームパッドの取込み時点の受付事実は、その入力源のイベント／既存保持状態へ局所化し、Appの別pending状態にしない。

まれな失敗は割り切り、ログ＋通知［＋作り直し］を基本とする。更新専用の回復機構は作らない。

## 2. 前提・更新履歴要約との相違

| 前提・要約 | ソース確認 | 反映 |
|---|---|---|
| egui-winitは削除できる見込み | 本家0.37もHashSet。現vendorの配送済みpress保持との差が残る | vendor維持 |
| eframeの変更はhidden repaint修正 | scheduler、visible commit、delta配送、通常版計測、診断もある | 全差分を台帳化 |
| ui_fontsはab_glyph計測 | 外形・行計測はttf-parser。プレビューがab_glyph | 別々に移行 |
| `CentralPanel::show`廃止 | Contextを受ける形がなくなり、Uiを受ける`show`は残る | 親Uiを移行 |
| `AsId`削除 | 0.37.0では非推奨として残る | 削除済みと書かない |
| Windowはタイトルバーのみ移動 | 既定`WindowDrag::OnTouch`。タッチでは本文からも移動する | 入力機器別に確認 |
| clipboardは余分な修飾キーなし | X/C/Vは`command_only()`。Windows Insert/Delete系は別条件 | Ctrl+Shift操作を保持 |
| texture deltaはmap化 | 非空drop assertionと明示破棄APIもある | 消費契約を必須移行 |
| 非表示はすべてlogic-only | SW_HIDEと最小化で異なる。schedulerとUI省略判定も異なる | §4.2の条件表で区別 |
| logic-onlyの入力保留は操作再実行 | 保留するのは未消費RawInput。保留自体は消費済み操作の再実行ではない | 本家どおり保持 |
| snapshot約350枚 | 追跡PNG445枚 | 工数を修正 |

根拠: `egui-winit-0.37.0/src/lib.rs:656`、`:1538`、`egui-0.37.0/src/containers/panel.rs:1377`、`src/id.rs:8`、`src/containers/window.rs:17`、`src/data/input/viewport_info.rs:98`、`epaint-0.37.0/src/textures.rs:335`、`eframe-0.37.0/src/native/epi_integration.rs:398`、`src/native/run.rs:216`、`src/native/wgpu_integration.rs:702`。

## 3. 影響件数

| 検索対象 | 一致行／ファイル | 代表箇所 |
|---|---:|---|
| `\bId::new\s*\(` | 571／84 | `crates/egui-ime/src/lib.rs:159` |
| `ui\.id\(\)` | 99／5 | `src/ui_music_panels.rs:706` |
| `\bSidePanel::` | 9／5 | `src/ui_folder_pane.rs:218` |
| `\bTopBottomPanel::` | 23／9 | `src/ui_main.rs:7400` |
| `\bCentralPanel::` | 339／60 | `src/ui_main.rs:17580` |
| `show_viewport_(immediate\|deferred)` | 38／8 | `src/ui_fullscreen.rs:22139` |
| `raw_scroll_delta` | 58／16 | `src/app.rs:58760` |
| `Event::MouseWheel\s*\{` | 103／20 | `src/video/native_presenter/render_core.rs:9751`付近 |
| `impl eframe::App` | 6／6 | `src/app.rs:97613` |
| `fetch_update\(` | 製品・テスト14／12、別途vendor 1件 | `src/log_writer.rs:105` |

```powershell
rg -n '\bId::new\s*\(|ui\.id\(\)|\bAsId\b' src crates tests tools -g '*.rs'
rg -n 'SidePanel::|TopBottomPanel::|CentralPanel::|show_viewport_' src crates tests tools -g '*.rs'
rg -n 'RawInput|raw_scroll_delta|Event::MouseWheel|\.modifiers' src crates tests tools -g '*.rs'
rg -n 'textures_delta|drop_without_applying_deltas|\.run\(|run_ui|run_pass' src crates tests tools vendor -g '*.rs'
rg -n 'sample_gamepad_input|dispatch_gamepad_batch|suppress_pending_actions|take_pending_mouse_nav' src -g '*.rs'
```

`.run(`の1274一致行／130ファイルにはHarnessやworkerも含むため、Contextの移行件数には使わない。FullOutputの破棄候補も変数名だけで完了とせず、戻り値の所有を追う。

## 4. 項目別設計

### 4.1 文字描画、補正、fallback、絵文字

#### 本家の変更

0.34以降の文字描画交換と0.35以降のshapingを経た0.37は、skrifa、vello_cpu、harfrustを使う。

| 内容 | 本家ソース |
|---|---|
| units-per-em基準の倍率 | `epaint-0.37.0/src/text/font_face.rs:88` |
| outline／color glyph選択 | 同`:96` |
| color glyph描画 | 同`:190` |
| shaping data | 同`:332` |
| メトリクス・offset | 同`:596` |
| hinting／subpixel binning既定有効 | `epaint-0.37.0/src/text/mod.rs:77` |
| 行内配置 | `epaint-0.37.0/src/text/text_layout.rs:1100` |

`FontTweak`のscale／offsetは残り、hinting target、variation coordinates等が加わる。根拠: `epaint-0.37.0/src/text/font_tweak.rs:6`。

#### 影響箇所と方針

| 箇所 | 内容・方針 |
|---|---|
| `src/ui_fonts.rs:4`、`:7`、`:13` | `miv-user-text`／`miv-hud-text`／toolbar familyを維持 |
| 同`:235`、`:420` | fallback順と主フォント登録を保持 |
| 同`:360`、`:373` | 主フォントの視覚中央補正を再検証 |
| 同`:546`、`:567` | fallback補正を新rendererのメトリクスと比較 |
| 同`:652`、`:687`、`:699` | ttf-parserのbbox／行計測。API更新だけを理由に全面交換しない |
| `src/ui_font_catalog.rs:6`、`:78` | ab_glyphプレビューを本体表示へ近づける |
| `src/video/native_presenter/bottom_hud.rs` | 実galleyの計測・描画共有と失効を確認 |
| `build.rs:700`、`src/comic_stamp.rs` | Twemoji SVGスタンプと生成物を保持 |

選択フォント、face index、利用者の縦位置調整値は自動リセットしない。toolbarの固定3.5pt、主フォント補正、CJK／数学／emojiの補正は別々に判断する。

color glyphはoutline bboxと可視範囲が異なり得る。emojiのoutline優先計測は、色付き描画の実際のinkと比較して採否を決める。補正を全部残す／全部捨てる方針にはしない。

判断材料は、同じ文字列・font・point size・DPIについての行高、baseline、galley矩形、実際の描画範囲とする。hinting由来の差と補正式の誤りを分けるfixtureを用意し、製品設定の追加は前提にしない。

フォントプレビューは既存workerでの準備を維持する。本体と異なるラスタライザによる見本のずれを解消する方式は、小設計としてレビューする。`comic-core`の注釈焼き込み用`ab_glyph_rasterizer`は別経路なので変更しない。

#### default_fontsとsystem fallback

egui単体のdefault featureは空だが、eframe defaultには`default_fonts`が残る。現rootの指定方法で全default fontが直ちに消えるわけではない。

根拠: `egui-0.37.0/Cargo.toml:62`、`eframe-0.37.0/Cargo.toml:61`、`epaint-0.37.0/src/text/font_definitions.rs:110`。

eframeのsystem font fallbackは既定有効。初期移行ではfeatureと`NativeOptions::system_font_fallback`を明示し、mIVの既存chainを基準にする。環境依存の自動探索を無条件に重ねない。

根拠: `eframe-0.37.0/src/epi.rs:384`、`src/native/epi_integration.rs:260`。

`monochrome_emoji_fonts`は別featureであり、旧組み込みemoji fontと同じ集合が既定登録されるとは限らない。Twemojiは文字fallbackではなく、`vendor/twemoji`から生成される注釈用SVG資産である。色付き文字対応で代替しない。

**リリース済みの見た目:** 字面、太さ、字幅、合字、折り返し、行高、baseline、emojiの色が変わり得る。代表比較を用意し、§5と段4bで採択する。

### 4.2 App／Context、非表示処理と入力所有

#### 本家の呼出契約

`App::logic`はrootのUIより前と、UIを省略する処理で呼ばれる。常時tickではない。logic-onlyではegui passが進まず、`Context::input`はviewport情報の一部を除き最後の表示passの値を持つ。

根拠: `eframe-0.37.0/src/epi.rs:152`、`egui-0.37.0/src/context.rs:1174`。

Windows上では、次の二つの判定を区別する。

- schedulerは`window.is_visible()`と最小化状態を使って、要求済みrepaintをどう実行するか決める。
- UI省略判定は`ViewportInfo::visible()`等を使う。これはminimized／occludedの合成であり、HWNDの`IsWindowVisible`そのものではない。

根拠: `eframe-0.37.0/src/native/run.rs:216`、`:243`、`src/native/wgpu_integration.rs:682`、`:702`、`egui-0.37.0/src/data/input/viewport_info.rs:98`。

#### SW_HIDE／最小化／子窓／期限超過の条件表

以下は本家0.37.0のWindows経路からの見立てであり、製品を起動して確認した結果ではない。表のroot処理は、そのrootについて処理要求がある場合を指す。

| 条件 | scheduler／pass | rootのlogic・UI | 入力上の扱い |
|---|---|---|---|
| 非最小化rootを直接`SW_HIDE` | 隠れた窓の期限到達済みrepaintを`Full`で直接実行 | 通常はlogic＋UIへ進む | SW_HIDEだけを理由にRawInput保留が累積するとはいえない |
| root最小化、visible扱いの子なし、強制paint要求なし | `Full`要求でも`show_ui=false` | logic-only。egui passなし | 未消費RawInputを本家どおり保持 |
| root最小化、visible扱いの子あり | `Full`なら子を登録する親UIも実行 | logic＋UI | 子の入力は子の既存ownerへ。root一括遮断を追加しない |
| 最小化等でもscreenshot／`RequestPaintWhileHidden`によるpaint要求あり | `Full`で`wants_paint`が成立 | UI実行可能 | 既存capture ownerと入力順を維持 |
| `RedrawRequested`が期限超過 | 可視性と別に`LogicOnly`を実行 | rootならlogicのみ。visible childがいても、この呼出ではUIなし | 未消費入力を保持。次のFull passで消費 |
| 期限超過等の対象がchild | childについてUI省略 | childから`App::logic`は呼ばれない | child callbackを無理に回す対策を追加しない |
| repaint・wake等の要求なし | `Wait`等でsleep | 呼出なし | 常時UI／logicが回るとは扱わない |

根拠:

- 隠れた窓の直接Full処理とsleep: `eframe-0.37.0/src/native/run.rs:216`、`:243`
- RedrawRequested期限超過のLogicOnly: 同`:266`
- 強制paint・子を含むUI実行判定: `eframe-0.37.0/src/native/wgpu_integration.rs:682`、`:702`、`:1189`
- rootだけのlogic-only処理: 同`:717`
- minimized更新: `egui-winit-0.37.0/src/lib.rs:1507`

winit Windowsの`is_visible()`は`IsWindowVisible`、`is_minimized()`は`IsIconic`を読む。Windowsでは`Occluded`は未対応で、`WM_SIZE`もそれを生成しない。

根拠: `winit-0.30.13/src/platform_impl/windows/window.rs:147`、`:668`、`util.rs:143`、`event_loop.rs:1397`、`winit-0.30.13/src/event.rs:418`。

mIVのトレイ退避は`src/tray_integration.rs:276`、`:323`の直接`SW_HIDE`である。したがって「トレイ退避したら必ずlogic-onlyとなる」という前提は採らない。

#### 影響箇所

| 箇所 | 内容 |
|---|---|
| `src/app.rs:94680`、`:97631` | update本体・App trait |
| 同`:94969` | tray／media wake |
| 同`:97661`以降 | 非同期完了 |
| 同`:97700`以降 | post-renderのmodal／許可終端 |
| `src/tray_integration.rs:276`、`:323` | hide／restore |
| `src/key_input.rs:2023`、`:2099`、`:3381`、`:3418` | Win32 queue owner・enqueue・pass drain |
| 同`:3960`以降 | subclassのedge／focus終端 |
| `src/keyboard_input.rs:106`、`:159` | event時点のdiscrete permitとcurrent level permit |
| `src/app.rs:55249`、`src/ui_fullscreen.rs:32137` | permit消費 |
| `src/gamepad.rs:170` | gilrs producer |
| `src/app.rs:95772` | gamepad sample入口 |
| `src/app/gamepad_input.rs:2110`、`:2158`、`:2342`、`:2373`、`:2398`、`:2510`、`:3657` | sample・配送・抑止・終端 |
| `src/lib.rs:786`、`:801`、`:836` | WH_GETMESSAGE由来pending |
| `src/ui_fullscreen.rs:22139`、`:22364`、`:24745` | viewport callback |
| `src/video/native_presenter/render_core.rs:12505` | 独立Context |
| `tools/*_lab/src/main.rs`、`crates/vst3-host-tester/src/main.rs` | App実装 |

#### Appの責務分離

| 処理 | 移行先 |
|---|---|
| worker結果、取消・保存終端、Remote完了、トレイ要求 | `logic`。UI入力を読まないservice |
| ゲームパッドの物理queue drain・保持状態更新 | logicから進められる、App-globalな単一owner |
| gamepadのUI操作batch・multi-pass再配送 | 既存batch ownerとmounted contextの配送経路 |
| key／wheel／pointer／focus／IMEのpass処理 | `ui`／対象viewport |
| panel／dialog／viewport登録・paint | `ui` |
| 表示済みを前提にする外部ツールACK等 | 実際の描画側 |
| contextごとの結果採用 | 既存registry／mount owner |
| native動画frame | 既存presenter thread |

旧updateを丸ごとlogic／uiのどちらかへ改名しない。logicは古いevents・キーlevel・時刻を操作入力として読まず、UI pass counterの進行をserviceの条件にしない。

副作用は既存要求の採用・消費で一度だけ実行する。post-render終端を前へ移す際は、そのframeのmodal gateを早く外さない。

#### 入力源ごとの最小契約

| 入力源 | 残り得るもの | 方針 |
|---|---|---|
| 通常OSキー／ポインタ | 非表示化直前の同一batch、遅着release、capture関連終端 | 受信済み入力の所有と順序を保持 |
| focus／Modifiers／IME | focus喪失、合成key release、composition完了・取消 | 既存正規化とviewport所有を保持 |
| Win32 edge queue | WMキー配送の別記録、pass待ちのedge | 既存enqueue／focus／destroy終端とdrain条件を保持 |
| ゲームパッド | foregroundに無関係な継続イベント | 物理状態更新と操作受付を局所的に分離 |
| WH_GETMESSAGE | UI thread宛Browserキー／WM_APPCOMMANDのpending | 既存main／fullscreen配送先とdrain条件を保持 |
| 合成入力、Remote、トレイ、IPC | 明示的な制御要求 | 通常hardware入力と一括して捨てない |

#### backendの保留RawInputとraw_input_hook

本家はlogic-onlyで未消費のRawInputを保持する。

1. `prepare_raw_input`で前回保留分と今回分を連結。
2. `App::raw_input_hook`を呼ぶ。
3. logic-only実行後、未消費RawInputを`pending_raw_input`へ保存。
4. 次の処理で再び連結し、Full passで消費する。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:384`、`:398`、`:418`、`egui-0.37.0/src/data/input/raw_input.rs:168`。

**この保持を本家どおり残す。** 未消費は無効を意味せず、保留自体は消費済み操作の再実行ではない。hide／minimizeだけを理由にText、clipboard、press、releaseを除去しない。

`raw_input_hook`は連結後なので、復帰時の新規入力と過去の保留入力を単純には区別できない。汎用の復帰フィルタには使わない。既存のhost固有正規化へ使う場合も、呼出順とviewport所有を保ち、第二のpending queueや受付gateを作らない。

#### Win32 edgeと受信時点の所有権

`DiscreteKeyInputSource::RoutedKeyEdge`は、そのviewportへ既に届いた離散キーの証拠である。後から同じframe内でfocusを失ってもdiscrete入力として採用する。一方、OS直読みやframe最終levelにはfocused permitが必要である。

根拠: `src/keyboard_input.rs:106`、`:159`、`src/app.rs:55249`、`src/ui_fullscreen.rs:32137`。

次を保持する。

- `pending`／`frame`／`operation_frame`をhide時に一律pruneしない。
- 受信済みキーを処理時点の`GetAsyncKeyState`でfresh／staleに分類しない。
- `ReturnKeyState`のenqueue時更新、`WM_KILLFOCUS`での対象viewport終端、destroy時の所有解除を共用する。
- 兄弟viewportの記録をrootのhideで変更しない。
- 既存回帰`routed_edge_keeps_only_discrete_input_after_focus_loss`の期待値を維持する。

根拠: `src/key_input.rs:2089`、`:3971`、`src/keyboard_input.rs:809`。

focus喪失時の合成releaseは`winit-0.30.13/src/platform_impl/windows/keyboard.rs:100`、egui-winitのfocus処理は`egui-winit-0.37.0/src/lib.rs:453`、`:483`。IMEには既存`UnfocusedComposition`と終端正規化がある。根拠: `crates/egui-ime/src/lib.rs:15`、`:78`、`:99`。

`PointerGone`は押下解除ではなく、Windowsの`WM_CAPTURECHANGED`もwinitではreleaseを生成しない。これは回帰対象とするが、この事実だけで汎用終端APIの必要性を認定しない。

根拠: `egui-0.37.0/src/input_state/mod.rs:1219`、`winit-0.30.13/src/platform_impl/windows/event_loop.rs:1927`。

#### ゲームパッドの移行

gilrs threadはforegroundに関係なくqueueへ追加しrepaintを要求する。現sampleはqueueのpressから操作を作り、配送時にforeground／modal等を判定する。

根拠: `src/gamepad.rs:170`、`src/app/gamepad_input.rs:2158`、`:2373`、`:2510`。

このままUI側だけでdrainすると、最小化中に別アプリで行った操作を、復帰後のforeground判定だけで操作候補にし得る。sample全体をそのままlogicへ移す案も、egui frame番号・pass indexによる再配送制御があるため採らない。根拠: 同`:2110`。

移行契約は次のとおり。

1. **物理取込みを一つのownerへ分ける。** drain、押下／軸／disconnect更新はUI passが進まなくても実行可能にする。logicとuiの二重drainを作らない。
2. **操作batchの配送は既存経路へ残す。** frame内の一度だけの配送とmulti-pass replayを維持し、logic呼出回数をegui frame番号で抑止しない。
3. **操作先がない期間も物理状態は更新する。** release、disconnect、中立への復帰を失わない。設定OFFと非前面を同一視してgilrs threadを停止しない。
4. **取込み時点の受付事実を残す。** logicが一度も動かない短い最小化→復帰も、gilrs取込み側で既存OS前面判定を共用して扱う。復帰時の現在foregroundだけでは判定しない。
5. 受付事実はPadイベントの局所的なmetadata、または既存Pad保持状態の型へ集約する。全入力共通層、新しいApp pending bool、時間窓は作らない。producerからApp／eguiのlockを取得しない。
6. 正式な操作先・modal判断は、従来どおり対象contextをmountした配送側で決める。producer側へ第二のviewer resolverを作らない。
7. 非前面時の`reset_gamepad_continuous_steps`と`suppress_pending_actions`を再利用し、受付不可期間に始まったpress／repeatを復帰後に発火させない。
8. 軸は物理値を更新しながら、受付不可期間に始まった操作を復帰だけで開始しない。既存中立待ちを利用し、足りない部分だけPadの同じ物理保持owner内で扱う。新しい独立ラッチ群を追加しない。
9. **一時popup等の既存挙動を一括変更しない。** 現`suppress_pending_actions`は、短いpopup後の軸継続を意図している。操作先不在期間の入力と、既存modal中の抑止を混同しない。
10. 既に開始したringの意味終端は正しいcontextで行う。disconnect／設定OFF時のring確定とUndo構築を単に捨てたり、root投影で確定したりしない。物理終端と意味終端は既存batch／context所有へ接続し、別終端queueを増やさない。

根拠: `src/gamepad.rs:380`、`:497`、`src/app/gamepad_input.rs:2342`、`:2398`、`:2510`、`:2732`、`:3657`。前面判定の既存入口は同`:509`。

段1bでは旧呼出順のまま抽出し、受付仕様の追加とlogic接続は段3で行う。UI pass番号が進まない複数回のlogic、短い退避・復帰、release／disconnect、ring所有をfake入力で回帰化する。

#### WH_GETMESSAGE由来pending

`src/lib.rs:786`のWM_APPCOMMAND、`:801`のBrowserキーはpending countへ入り、`:836`で現在のUI threadにhookを設置する。他アプリへ届く全キーを拾うglobal keyboard hookではない。

消費側は`src/app.rs:55234`、`:59997`、`src/ui_fullscreen.rs:32101`。既存のmain／fullscreen配送先、重複防止のdrain条件を保持し、非表示入力の新しい一括処理へ移さない。

#### 撤回する設計と回帰範囲

次は本計画の必須移行から撤回する。

- 全入力共通の「通常配送／終端のみ配送」
- eguiへの「clickを作らないrelease」API追加
- hide境界でのRawInputフィルタ
- Win32 queueの一律prune
- 復帰直後の一回捨て、時間窓、常時追加repaint

回帰対象はSW_HIDE、最小化、visible child、期限超過LogicOnly、focus移動前の受信済みキー、IME終端、pointer capture、最小化中のgamepadと短い復帰。具体的な失敗が示された所有境界だけを修正する。

**リリース済み挙動:** 受信済み離散キー・release確定を保持する。ゲームパッドの既存非前面抑止を、UI省略時にも成立させる。既存操作を取消す仕様へ変える場合は§5の相談対象とする。

#### API移行

- 描画は`run_ui`と受け取ったroot Ui。
- 入力・状態中心のheadlessは`run_pass`、必要箇所だけ`root_ui`。
- viewport callbackは`&mut Ui`、Contextは`ui.ctx()`。
- Contextの`style`系は`global_style`系へ。Uiの局所styleを一括置換しない。
- UiのDerefで解決される旧forwarding methodを無意味に書き換えない。

根拠: `egui-0.37.0/src/context.rs:950`、`:983`、`:1024`、`:2599`、`:4662`、`src/ui.rs:77`。

`on_exit`の署名はglow featureと揃える。終了処理本体は保持する。根拠: `eframe-0.37.0/src/epi.rs:227`、`src/app.rs:97792`。

### 4.3 Panel統一と計算高さ

影響箇所:

- `src/ui_main.rs:7400`、`:10291`、`:12590`、`:15558`: menu／toolbar／検索
- 同`:20434`: 下部情報
- `src/ui_folder_pane.rs:218`: side panel
- `src/ui_main.rs:17580`、`:19160`: central
- `src/ui_text.rs:3002`、`:3331`: Area固定パネル
- `tests/support/scroll_layout.rs:6`、`:31`、`:65`: geometry検査

`SidePanel`／`TopBottomPanel`を`Panel::show(&mut Ui, …)`と`default_size`／`min_size`等へ移す。`CentralPanel::show`も親Uiへ接続する。

根拠: `egui-0.37.0/src/containers/panel.rs:252`、`:372`、`:425`、`:1377`。

panelの順序・残り領域を一つの親Uiに揃え、不要な子Uiによるmargin／scope変更を避ける。slide、drag-to-close、端double-clickによるcollapseは既存操作と比較して明示設定する。

根拠: 同`:337`、`:454`、`:741`。separatorは同`:348`。

Area＋Frame＋ScrollAreaの固定HUDを新Panelへ全面交換しない。separator幅修正はmIV独自の計算高さを自動修正しない。新版ScrollAreaも親のavailable sizeを上限にする。根拠: `egui-0.37.0/src/containers/scroll_area.rs:842`。

| backlog §1.380 | 影響・扱い |
|---|---|
| clipboard失敗カード | `src/ui_dialogs/clipboard_capture.rs:378`、`:490`。可変高と固定仮想行の問題は残る |
| 長い保存基点名 | 固定本文予算の問題を別途扱う。更新だけで解決済みにしない |
| 注釈右詳細・左一覧 | `src/ui_text.rs:2745`、`:2761`、`:3252`、`:3348`。sink・最低高・本文高を同じpassで検査 |
| 切り取り・表示トリム | 内外枠と固定操作の実矩形で確認 |
| 構成editorの行 | §1.242の既存作業へ残す |

Fits／Scrolls、1物理pixel許容、実wheelによる末尾到達を維持する。snapshot一致だけで高さを受け入れない。

**リリース済みの見た目・操作:** separator、余白、collapse、panel移動が変わり得る。既存操作保持を初期案とし、変更採用は§5へ送る。

### 4.4 フォーカス、wheel、drag、Ctrl+Q

#### フォーカス・drag

影響箇所: `crates/egui-ime/src/lib.rs:242`、`:305`、`:329`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/ui_music_panels.rs:706`、native HUDのseek／音量。

Area／Window／ScrollAreaの非focus化と入力部品のfocus回復を区別する。Ctrl+矢印の改善だけでhelperを撤去しない。全域Tab traversal無効という製品方針を保持する。本家もTabをfocus移動として扱う。

根拠: `egui-0.37.0/src/memory/mod.rs:590`。

dragは、上位widgetとの重なり、press後のwidget外移動、外側release、disabled復帰、次の新規押下を検査する。press位置から離れた場合のdrag判定と`Sense::drag`の変更に対し、背景Senseを増やしてclickを奪う修正をしない。

ScrollAreaは`DragScroll::OnTouch`が既定。旧版でmouse drag scrollが使えた画面は必要箇所に`Always`を明示する方針。削減する場合は利用者相談を先に行う。

根拠: `egui-0.37.0/src/containers/scroll_area.rs:224`、`:286`。

#### raw_scroll_deltaの代替契約

影響箇所:

- `src/app.rs:58738`、`:58760`: 一覧・列数変更
- `src/ui_main.rs:1462`: menu wheel消費
- `src/ui_helpers.rs:414`、`:475`: 筆半径wheel
- `src/ui_fullscreen.rs:25754`、`:27352`: 前面ownerと残量終端
- `src/app/runtime_ops.rs`、dialog、ラボ、関連テスト

旧`raw_scroll_delta`は単純な生delta合計ではない。単位換算と修飾キーによる軸変換を含み、その後scroll／zoomの平滑化へ分かれる。

根拠: `egui-0.33.3/src/input_state/mod.rs:440`。

**取得元は、その消費位置での`InputState.events`に残るMouseWheel。** `raw.events`、WheelStateのsmooth値、OS現在の修飾キーは使わない。

共通の純粋な投影関数で各イベントを順に解釈する。

| 処理 | 契約 |
|---|---|
| Point | deltaをlogical pointとして使用 |
| Line | `InputOptions::line_scroll_speed * delta` |
| Page | 当該passのviewport高さ`* delta`。content高さではない |
| 横修飾だけ一致 | `(x+y, 0)` |
| 縦修飾だけ一致 | `(0, x+y)` |
| 両方一致／どちらも不一致 | 軸を保持 |
| 修飾キー | 各MouseWheelの`modifiers` |
| phase／source | 保持する。Start／End／Cancel自体を余分な移動へ変換しない |

0.37 WheelStateのgesture中のmodifier latchと、mIVの離散操作の解釈は分ける。

根拠: `egui-0.37.0/src/input_state/wheel_state.rs:104`。

同一frameにCtrlあり／なし等が混ざる場合は、受理先とmodifiersを保持した連続区間として処理する。同一操作の区間内では従来どおり集約し、通常wheelのnotch数だけ行移動を増やす変更をしない。Ctrl+wheelも従来の列数変更単位を維持する。異なる修飾条件をframe最終値でまとめない。

筆半径等、既に独自のnotch換算を行う操作は既存換算を保持する。全consumerを同じpoint量へ押し込まない。

消費順序:

1. 正当に対象viewportへ配送された入力列を既存ownerが受け取る。
2. fullscreenの前面ownerをキー・navigatorより前に確定する。
3. 必要な原イベントsnapshotは、その既存ownerの範囲だけで保持する。
4. menu／popup／前面領域がwheelを受け、対応するeventsと公開smooth値を消費する。
5. Appの一覧処理は従来どおりmenu／toolbar等の後、grid直前で、**残ったeventsから**unsmoothed値を取得する。
6. 操作を受理したconsumerが、同じ入力所有境界でイベントと対応するsmooth値を消費する。
7. 前面popupの終了時だけ、既存`discard_pending_scroll`の新版相当でそのownerの平滑化残量を終端する。

共通投影関数に未消費入力をcacheさせない。consume後にraw snapshotから復活させない。診断用原snapshotとconsumer用配送投影を混同しない。

本家ScrollAreaのイベント消費だけに依存せず、mIVのraw-event consumerとの順序を維持する。

#### Ctrl+Q

本家はend-passで`quit_shortcuts`を消費しCloseを送る。

根拠: `egui-0.37.0/src/memory/mod.rs:337`、`src/context.rs:2935`。

各製品Contextで本家既定を明示無効化し、既存keymap／終了・tray ownerへ揃えることを推奨する。新しい終了操作として採用する場合は別途相談する。

**リリース済み挙動:** wheel所有、Tab方針、キー割当を保持する。mouse drag scroll、drag判定、終了操作に差が出る場合はsnapshot更新で済ませず、操作回帰として扱う。

### 4.5 非推奨API削除とIME

本家はTextEditのIME owner、下線／cursor、候補窓位置を改善する。

根拠: `egui-0.37.0/src/memory/mod.rs:1064`、`src/widgets/text_edit/builder.rs:617`、`:1049`、`:1382`、`egui-winit-0.37.0/src/lib.rs:1291`。

`owns_ime_events`等の旧API依存を新owner契約へ合わせる。これらはmIVのshortcut gate、viewport別snapshot、取消補完、focus contract、Tab方針、Win32 Process filterを一括代替しない。

| 対象 | 方針 |
|---|---|
| 300ms grace／timestamp／旧pending-event API | 撤去済み。復活させない |
| 確定Enter除去／IME専用Tab lock | 撤去済みのまま |
| 全域Tab方針 | 保持 |
| dialog Enter／Escape helper | 保持 |
| viewport別composition owner | 保持 |
| 空Preedit補完 | 実TextEdit bufferの取消回帰を照合して判断 |
| focus recovery／次pass限定claim | 本家と重複する条件を特定してから縮小 |
| Win32 0xE5 press／repeat除外 | 保持 |
| Hidden／Forwarded | 保持 |
| focus喪失・IME終端の順序 | 既存正規化を保持。汎用非表示機構を重ねない |

本家TextEditのEnabled／Disabled自体はcomposition textの置換対象ではない。空Preedit補完が不要になったとは判断しない。

根拠: `egui-0.37.0/src/widgets/text_edit/builder.rs:1417`。

主なmIV箇所は`crates/egui-ime/src/lib.rs:15`、`:78`、`:99`、`:242`、`:305`、`:329`、`src/ime_focus.rs`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/key_input.rs:3960`以降、native入力producer。

#### egui-winitを削除できない理由

前セッションのIME修正段2a／2bは本家#7967を基にしているが、現vendorは本家そのものではない。

本家0.37はProcessのphysical keyをHashSetで追跡する。`allow_ime=false`では判定を通過し、許可解除時は集合をclearする。

根拠: `egui-winit-0.37.0/src/lib.rs:483`、`:656`、`:1258`。

現vendorは次を保証する。

- 一度配送したpressは、後続Process repeatでHiddenへ戻さない。
- releaseはallow_ime／logical keyに依存せず記録をremoveし、Hiddenだけ遮断する。
- focus喪失時は当該Stateだけ終端する。
- 通常press→Process repeat、Process→通常repeat→Process repeatの両方で配送済みreleaseを失わない。

したがって本家への単純置換では同等にならない。`vendor/egui-winit/PATCHES.md`冒頭の3差分と網羅回帰を新版へ移植する。

traceは既定無効、文字秘匿、worker I/Oを保持する。削除条件は、これらの配送保証が本家と同等になり、診断traceも不要と判断できた時点である。

Windowの移動は既定`OnTouch`を評価する。本文dragを減らす場合は§5の相談事項とする。

**リリース済み挙動:** 日本語確定直後の通常Enter／Escape、候補操作、兄弟viewportのIME独立性を保持する。下線・cursor・候補窓位置は改善に伴う外観差があり得る。

### 4.6 Modifiersイベント化、keymap、clipboard

`RawInput.modifiers`はなくなり、`Event::ModifiersChanged`がlevel stateを更新する。イベントごとのmodifiersは残る。

根拠: `egui-0.37.0/src/data/input/event.rs:94`、`src/input_state/mod.rs:434`、`egui-winit-0.37.0/src/lib.rs:548`。

| 影響箇所 | 内容 |
|---|---|
| `src/app.rs:59309` | OS resync |
| `src/key_input.rs:2459`、`:2466` | 合成入力 |
| `src/video/native_presenter/render_core.rs:2716`、`:9780`付近、`:12505` | native queue／独立Context |
| `src/ui_fullscreen.rs:67268`、`src/ui_adjustment_panel.rs:10985` | テスト入力 |
| `src/ui_erase.rs:815`、`src/ui_conceal.rs:742`、`:1341` | stale対策 |
| `src/ui_adjustment_panel.rs:12014`、`:12510` | Space／Ctrl編集 |
| `src/keymap.rs:7468`、`src/keyboard_input.rs:305` | exact chord／consume |

producerはmodifier変化を対象入力より前に並べる。frame末尾へ最終値を一つ追加するだけにしない。OS resyncは過去の離散イベントの修飾キーを書き換えない。

FS viewportのOS直読み、`*_held_via_os`、focused permit、物理ラッチ、左右Ctrl、Enter／NumpadEnterを更新だけで撤去しない。ModifiersChangedは失われたKeyUpを保証しない。

0.35の修飾キーKey、0.37のWindows固有Keyを理由に、`KeyAction`の保存名・既定割当を変更しない。native VK判定とegui Keyの変換表は、既存割当の到達性を基準に合わせる。

#### Ctrl+Shift+C/X/V

現vendorは`modifiers.command`を使いShiftを許す。本家0.37はX/C/Vに`command_only()`を使いShift／Altを拒否する。

- 旧判定: `vendor/egui-winit/src/lib.rs:2314`
- 新判定: `egui-winit-0.37.0/src/lib.rs:1538`
- native入力欄: `src/video/native_presenter/render_core.rs:9863`。Ctrlかつ非AltでShiftを許す。

**推奨は、Ctrl+Shift+C/X/Vを通常窓とnative入力欄で保持すること。** 通常窓だけ本家の狭い条件へ変更しない。

Windowsのtext clipboard判定を、Ctrl、Alt、Shift、対象キーから決まる共通の純粋な契約へ揃える。実clipboard I/Oは各integrationに残す。Process filter、IME owner、press／release、空clipboard時の二重Text防止を保持する。

Ctrl+Alt系やInsert／Delete系は既存の別条件を確認する。必須互換範囲から外れる条件を、ついでに拡大・縮小しない。

**リリース済み挙動:** keymapとCtrl+Shift clipboardを保持する。本家条件をそのまま採る場合の操作縮小は§5の相談事項である。

### 4.7 Id、scope、永続化

- `Id::new`は`Id::unique`を呼ぶ非推奨API。
- `AsId`は非推奨として残る。
- `AsIdSalt`／`IdSalt`が導入され、`Id::with`の内部ハッシュ構成が旧版と異なる。
- `Ui::id`は`scope_id`。
- `From<&str> for Id`を使う移行にはしない。

根拠: `egui-0.37.0/src/id.rs:8`、`:91`、`:102`、`:121`、`:142`、`src/id_salt.rs:43`。

`Id::new`を残してもchild Idまで旧値と同じとは限らない。非推奨警告保留は永続互換対策にならない。

root identityは`Id::unique`、局所塩は`IdSalt`。`ui.id()`を機械的に`unique_id()`へ変えず、scopeの意味なら`scope_id()`を使う。生成側とlookup側を同時に移す。Panelへ渡す値は`AsIdSalt`の意味へ合わせる。

代表: `crates/egui-ime/src/lib.rs:159`、`src/ui_music_panels.rs:706`、`:1563`、`tests/ui_snapshot.rs:443`、`tests/book_composition_editor_snapshots.rs:405`。件数・検索式は§3。

Id非推奨は切替直後の短期間だけ許容し、後段で整理する。crate全体の`allow(deprecated)`は置かない。

現manifest・確認範囲ではeframe persistenceは有効化されておらず、独自のegui memory保存も見当たらない。本家保存処理はfeature内である。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:470`。

**リリース済みデータ:** settings.dbを初期化・全面migrationしない。実装時のgraph確認でリリース済みId永続経路が発見された場合だけ、旧読込・変換・互換試験を設計する。独自窓配置保存とsession内memoryを区別する。

### 4.8 vendorパッチ台帳

分類:

- **(a)** 本家に同等の仕組みがあり、旧差分を外せる。
- **(b)** mIVの保証・責務が残り、付け直す。
- **(c)** 旧実装が不要。必要な機能は新APIへ移す。

元ソースとの差は、eframe srcが9ファイル・約2270行追加／124行削除、egui-wgpu srcが9ファイル・約2004行追加／192行削除。READMEだけを移植台帳にしない。

#### egui

| 項目 | 分類・方針 |
|---|---|
| releaseとclick countの射影 | (b) `vendor/egui/src/input_state/mod.rs:1485`。順序・非click・focus・discard回帰を保持 |
| `discard_pending_scroll` | (b) 同`:595`。新WheelStateの所有に合わせる |
| repaint delay witness | (b) `src/context.rs:1794`、feature。observerだけでは同じgetterを代替しない |
| 上記テスト・license・standalone接続 | (b) 新構造へ移植 |

新wheel所有箇所は`egui-0.37.0/src/input_state/mod.rs:228`、`wheel_state.rs:54`。

第2版の「非表示入力の状態終端API追加」は撤回する。既存差分ではなく、今回追加設計した項目だったため、付け直し対象に含めない。

#### egui-winit

| 項目 | 分類・方針 |
|---|---|
| Process除外の基本機構 | (a) 新本家を土台にする |
| Hidden／Forwarded、release／focus終端 | (b) 保持 |
| trace・文字秘匿 | (b) 保持 |
| 配送／State網羅回帰 | (b) 新modifier／clipboard経路へ接続 |
| Windows clipboard互換 | (b) Ctrl+Shiftを含む§4.6の契約 |
| 旧300ms補正 | (c) 撤去済み。戻さない |

第2版の汎用「非表示受付」追加は撤回する。既存physical key配送履歴へ別のhide状態を重ねない。

#### eframe

| 項目 | 分類・方針 |
|---|---|
| #7905 hidden repaint基本修正 | (a) 新本家採用。旧backportを重ねない |
| dirty／painting、surface generation、outer phase | (b) `native/run.rs:45`、`:118`、`:425` |
| 非ゼロresizeのinline paint provenance | (b) 同`:1282`、`native/wgpu_integration.rs:978` |
| 初回visible commit／Frame getter | (b) `src/epi.rs:736`、`native/epi_integration.rs:305` |
| startup window observer | (b) `src/startup_window_observer.rs:8` |
| max_texture_side統一 | (b) `native/wgpu_integration.rs:735`、`:1235`。同等性を回帰で確認してから外す |
| 非描画・消滅viewportのdelta配送 | (b) 同`:828`、`:1313`。§4.9の共通ownerへ接続 |
| immediate child screenshot | (b) 機能をcallbackへ移行。旧Event方式は(c) |
| 通常版immediate viewport計測 | (b) 同`:1206`、`:1396`。段階別時間、delta bytes、遅いdelta付き描画ログを保持 |
| test-script window witness／capture・focus診断 | (b) callback-local identityを保持 |
| 実ウィンドウ試験の隔離 | (b) 段0a。試験を削除しない |
| manifest／Win32 feature／standalone patch | (b) 新原本を基準に移植 |

本家の`pending_raw_input`保持・連結順序はそのまま使う。第2版の「pending RawInputの受付・終端」追加パッチは撤回する。

通常版immediate計測はstartup／test-script診断と別責務である。現在の遅い描画かつdelta bytesがある場合の記録を維持し、常時I/Oへ拡大しない。

本家pending deltaは`eframe-0.37.0/src/native/wgpu_integration.rs:795`、`:1290`。保持だけで消費完了とは判断しない。

#### egui-wgpu

| 項目 | 分類・方針 |
|---|---|
| managed mipmap／再生成／sampler | (b) `src/renderer.rs:712`、`:777`、`:1151` |
| 公開MipmapGenerator／texel count | (b) `src/mipmap.rs`、`src/mipmap.wgsl`、`src/lib.rs:35` |
| atlas診断／renderer identity／texture_size | (b) 新delta列へ適応 |
| 無描画delta適用 | (b) `src/winit.rs:523`。drain／submit／freeまで完結 |
| `set_window(id,None)`の対象surfaceのみ破棄 | (b) 本家も全surface clearのため保持 |
| startup GPU stage／root present observer | (b) `src/startup_diagnostics.rs`、`src/lib.rs:322` |
| screenshot／readback診断 | (b) callback方式へ移行 |
| shader空行等 | (c) 再適用しない |
| license／テスト接続 | (b) 新原本を基準にする |

本家surface差: `egui-wgpu-0.37.0/src/winit.rs:203`、`:218`、`:229`、`:247`。

既存atlas overflowのskip-writeは新しい根治策として拡張しない。診断を保ち、配送不変条件の回帰でoverflowが生じないことを受け入れる。既存skip処理の撤去可否は履歴を含め別判断する。

#### 取り込み手順

1. 新版原本、checksum、VCS、licenseを記録する。
2. 新原本へ論理項目ごとに必要差分を付け直す。
3. 旧manifestをコピーせず、新manifestを基準にする。
4. root／standaloneが同じ新版実体を使うことを確認する。
5. PATCHES／READMEを実差分と一致させる。
6. `.cargo-ok`、target等を取り込まない。
7. 他のDLL・モデル・SDKを巻き込まない。junction／symlink等のreparse pointを作らない。

**リリース済み挙動:** 保持項目は機能・性能・診断保証の継続が目的である。撤去は本家同等性または不要理由と回帰証拠を伴わせる。

### 4.9 wgpu、texture delta、native presenter

#### 現行経路とWindows依存

実コードに直接hal／`windows_058`の利用は見当たらず、旧共有経路のコメントが残る。現native presenterはD3D11の`OpenSharedResource1`を使う。

- `src/video/native_presenter/render_core.rs:8347`、`:8379`: 共有resource
- 同`:6708`: 動画Present
- `src/video/native_presenter/overlay_gpu.rs:43`: overlay surface
- `src/app.rs:17366`付近: 旧経路の説明

未使用の直接`wgpu-hal`／`windows_058`除去とコメント訂正は0.33.3上へ前倒しする。未使用`windows_062`別名を追加しない。hal30を将来直接使う場合のWindows crateは0.62系である。

根拠: `wgpu-hal-30.0.1/Cargo.toml:518`。

D3D11本体のwindows 0.61は今回全面更新しない。

#### 影響箇所とAPI

| mIV箇所 | 内容 |
|---|---|
| `Cargo.toml:368` | wgpu30 |
| `src/lib.rs:495`、`:526` | surface設定 |
| `overlay_gpu.rs:31` | Instance／DeviceEpoch |
| `render_core.rs:12089` | configure |
| 同`:13840`、`:13957` | delta適用・free・消費 |
| 同`:13891`以降 | render pass／submit／present |
| `src/compare_wgpu.rs:274`、`:539` | pipeline／callback |
| `src/panorama_wgpu.rs:332`、`:669`、`:506`、`:833` | pipeline／mipmap／callback |
| `src/gpu_lanczos.rs:680`、`:708` | 縮小 |
| `build.rs:140`、`:230`、`Cargo.toml:495` | 独立Naga27→HLSL→DXBC |

必須差分:

- Instance descriptorの値渡し: `wgpu-30.0.1/src/api/instance.rs:63`
- bind group layoutのOption配列／`immediate_size`: `src/api/pipeline_layout.rs:38`、`:44`
- `multiview_mask`: `render_pipeline.rs:238`、`render_pass.rs:682`
- color space: `surface.rs:90`
- `CurrentSurfaceTexture`: 同`:148`
- `Queue::present`: `queue.rs:377`
- `WgpuConfiguration.surface`: `egui-wgpu-0.37.0/src/lib.rs:325`

Surface／DeviceEpochの寿命、configureのwrite lock、submit／presentのread lockを維持する。新presentをgate外へ出さない。surface状態だけでdevice-lost latchを立てず、新しいretryを作らない。

#### TexturesDeltaの所有・消費契約

新版はsetがIDごとのdelta列、freeがsetであり、非空dropはdebug assertionになる。

根拠:

- `epaint-0.37.0/src/textures.rs:284`、`:315`、`:335`
- `egui-0.37.0/src/data/output.rs:72`

通常APIは`&mut TexturesDelta`または所有権移譲を受け、**借用適用しただけの非空出力を返さない**。diagnosticsは借用観測し、適用権を持つcloneを作らない。

| 経路 | ownerと完結条件 |
|---|---|
| 通常eframe描画 | backendがFullOutputから既存pending ownerへ移譲。rendererがsetを順にdrain、描画commandをsubmit、その後freeをdrain |
| 通常native overlay | render_coreの描画batchが所有。借用二重ループだけでなくset／freeを消費する |
| surfaceなし・無描画配送 | 共有rendererがあれば共通処理がsetをdrainしuploadをsubmit。batchの描画を行わないことを確定後freeをdrain |
| renderer未準備・存続する出力の早期終了 | 既存backend pending／logical batch ownerへ引継ぐ。未適用の局所変数をdropしない |
| acquire不成立 | 適用済みsetと未処理freeを区別。uploadをsubmitし、中止batchを完結。引継ぐ場合は残りだけ移す |
| viewport／window消滅 | 共有rendererが存続し得るため、deltaをviewportと一緒に捨てない |
| renderer／Contextの明示終了 | 今後の配送先がないことをownerが確定。対象単位で終端し、診断付きの明示破棄に限る |
| 描画不要のheadless test | 必要なoutputを検査後、`FullOutput::drop_without_applying_deltas()` |
| 実描画／upload検証test | 実consumerで適用・drain。明示破棄で成功扱いしない |

whole→partial、同一IDの複数partialの順序を保持し、map全体の反復順へ依存しない。freeは当該frameの使用がsubmitされた後に行う。

早期return／`?`より前に、deltaを完結責任のあるownerへ移す。再利用するContextの出力をheadless APIで捨てて製品を続行しない。

製品で「assertを避けるため最後に無条件clear」は禁止する。明示破棄は描画不要のtest、または対象renderer／Contextの終了が確定した境界に限る。

`let _ = ctx.run(...)`等のtestも変更対象。例: `src/ime_focus.rs:795`。output検査後の破棄を共通helperへ揃え、新版APIへの接続は段3で行う。

#### 無描画配送とsubmission

本家は途中returnでもqueue内部のwrite bufferを滞留させないsubmission guardを持つ。

根拠: `egui-wgpu-0.37.0/src/winit.rs:502`、`:514`。

ローカル`apply_textures_delta`の旧update／freeだけでは足りない。共通の完結処理は次を満たす。

1. rendererがある場合、surface lookupより前にsetを適用・drainする。
2. mipmap等のcommand bufferを含めてsubmitする。
3. command bufferがなくてもqueue writeを行った経路はsubmissionを完結する。GPU待機は追加しない。
4. 通常描画でsubmit済みなら重複した空submitを追加しない。
5. freeはbatchのsubmit後にdrainする。
6. acquire／capture等の途中returnでも同じbatch ownerが完結または引継ぎを行う。
7. 引継ぎ時にsetを再適用せず、未処理部分だけ渡す。

本家の通常freeもsubmit後にdrainされる。根拠: `egui-wgpu-0.37.0/src/winit.rs:768`以降。

headless GPUでno-surface、acquire不成立相当、whole＋partial、mipmap、free-onlyを通し、drain／引継ぎ、submission、readback、解放順を確認する。textureサイズの更新だけを受入証拠にしない。

これは段2bと段3の必須条件である。

#### 表示・性能

AutoVsync／latency 1の既存明示設定と環境変数の意味を新SurfaceConfigへ移す。低遅延既定の変更を理由に製品設定を暗黙変更しない。

stencil対応を理由に新たなbufferを要求せず、callbackのformat／sample数を揃える。RGBA atlasへdiagnosticsを合わせる。

根拠: `epaint-0.37.0/src/image.rs:16`。

D3D11用Naga27→HLSL→DXBCは別経路なので、wgpu30更新だけで同時更新しない。WGSLの検証とnative生成物の検証を分ける。

**リリース済み挙動・見た目:** 動画本体の表示方式、アップスケール方式、VSync方針を変更しない。HUD文字・透明合成・frame latency・mipmap品質の差は段4cと実機で確認する。

### 4.10 egui_kittest、accessibility、snapshot

新版はvisible inputのaccessible nameを検査し、read-onlyを除外する。label／placeholderを認識する。

根拠: `egui_kittest-0.37.0/src/accessibility.rs:14`、`:26`、`:46`、`src/builder.rs:151`。

**検査はsettled後と画像生成前に行われる。** 候補画像収集後に名前を直す順序では成立しない。

根拠: `egui_kittest-0.37.0/src/lib.rs:518`、`src/snapshot.rs:739`。

影響箇所:

- `tests/ui_snapshot.rs:28`
- `tests/book_composition_editor_snapshots.rs:132`、`:247`付近以降
- `tests/support/ui_harness.rs:14`
- `tests/support/scroll_layout.rs:6`
- lib内snapshot／描画fixture
- `src/ime_focus.rs:1268`等のraw TextEdit allowlist

kittestは0.3→0.5。NodeT／`accesskit_node()`は残るが、containerの名前・scope変更でqueryの一致先が変わり得る。

根拠: `kittest-0.5.0/src/node.rs:9`、`egui_kittest-0.37.0/src/node.rs:21`。

方針:

- 名前付与は本体描画側へ行い、0.33上で前倒しする。
- 0.37で増えた検査対象は、切替後に候補画像生成前に補完する。
- 共通builderのcheckを無効化しない。
- role／親containerでqueryを限定し、固定node ID依存を増やさない。
- 最終passのgeometryを採取する。
- DX12|VULKAN、lazy、CPU adapter優先、renderer独立を維持する。

更新順序:

1. accessibility修正により、画像生成前の検査が完走する状態にする。
2. 旧baselineを更新せず、両integration targetとlib snapshotの通常比較で候補を生成する。
3. 文字ラスタ差、文字幅・行高差、panel／margin差、操作回帰へ分類する。
4. 入力・geometry回帰を直し、外観を採択する。
5. 同じrenderer／font方針で全対象を更新する。
6. 更新モードを外して同じ全対象を通常比較する。
7. 画面群、共通理由、対象ファイル集合、例外、確認者・結果を一台帳に記録する。

445枚は対象漏れ確認の基準とする。目視は変更対象全体をカバーするが、各PNGへ重複した担当欄を作らない。contact sheetを索引にし、文字位置・欠け・例外差分を原寸で見る。入力・到達性検査をPNGへ置き換えない。

lib対象は`--lib snapshot`だけで決めず、PNG生成呼出とfixtureの呼出元から列挙し、具体filterを台帳へ記録する。

**リリース済みの見た目:** 文字描画交換による広範な差が見込まれる。外観差の一括更新は段5に集め、挙動回帰を混ぜない。

### 4.11 その他の挙動と複数viewport

| 変更 | mIVの影響・方針 |
|---|---|
| popup sizing pass再実行 | 計測passで保存・選択commandを発行しない |
| press-open後のrelease即閉じ防止 | 右click、外側終了、wheel owner終端を検査 |
| menu hover-open | menu barの選択・wheel消費を維持 |
| ScrollArea端fade | 旧外観維持を初期案。採用は比較後 |
| Window margin／min size | 小画面の本文・固定操作行を再計測 |
| tooltip／container | 入力ownerやqueryへ混ざらないことを検査 |
| repaint-after削減 | 予定外frameに依存する完了処理を直す。追加repaintで旧頻度に戻さない |
| rootのみ保存／復元 | 独自配置と二重所有しない |
| raw mouse motion | 配送先の変更を確認。unfocused受信をkeyboard ownerの根拠にしない |
| Screenshot callback | 既存batch owner／worker I/Oへ接続 |
| set_fonts即時適用 | 不要な初期frame追加を整理し、atlas／galley失効を検査 |

本家根拠: `egui-0.37.0/src/containers/popup.rs:629`、`:660`、`menu.rs:440`、`scroll_area.rs:1704`、`window.rs:633`、`:696`、`context.rs:133`、`:2505`、`:4557`、`eframe-0.37.0/src/native/wgpu_integration.rs:511`、`:903`。

代表的なmIV側は、panel群が§4.3、dialog／入力が§4.4、完了処理が`src/app.rs:97661`以降、viewport群が`src/ui_fullscreen.rs:22139`以降。撮影は`src/test_script/capture.rs:282`、`:375`。callback内でPNG encode／I/Oを行わない。

**リリース済み挙動・見た目:** popupの開閉、menu、fade、margin、最小サイズ、tooltipは変わり得る。操作到達性を保持し、外観採用は§5で扱う。

#### detached凍結

今回のApp分離、callback Ui化、gamepad接続、scheduler／delta／surface移植はviewport・context経路へ触れる。

次は行わない。

- rect捕捉条件の追加
- geometry由来recreate
- 新しいdetached用pending bool
- 時間窓での入力遮断
- 兄弟cache／surface／入力の一括reset

着手前にClaudeCodeと独立Codex reviewerが、該当範囲を症状パッチではない構造修正として合意し、`docs/detached-rework-plan.md` §11へ範囲と理由を記録する。本改訂案を合意済みとは扱わない。

第2版の汎用非表示受付・終端追加は、この合意対象からも撤回する。

### 4.12 Rust 1.99警告とMSRV

既存ログはf32 fallback 445件／36ファイル、lib全体688警告。今回実行した結果ではない。

代表: `src/ui_main.rs:184`、`src/ui_text.rs:519`、`src/ui_adjustment_panel.rs`、`src/ui_fullscreen.rs`、`src/video/native_presenter/overlay_draw.rs`。

| 対象 | 方針 |
|---|---|
| Stroke／PathStroke | 更新後に再集計。0.37の幅はf32 |
| その他の曖昧float | 必要な境界へ局所型注釈 |
| 旧vendor警告 | 新原本で再評価。旧修正を重ねない |
| Atomic::fetch_update | egui更新では解消しない。MSRV互換を確認 |
| dead code | feature／test／Windowsを区別。機能削除で減らさない |

根拠: `epaint-0.33.3/src/stroke.rs:27`と`epaint-0.37.0/src/stroke.rs:26`、`:140`。

Atomic代表: `src/log_writer.rs:105`、`:165`、`src/everything_ipc.rs:226`、`src/ime_focus.rs:225`、`src/similar_index.rs:1431`、vendor witness。

egui／eframe 0.37のMSRVは1.95である。根拠: `egui-0.37.0/Cargo.toml:14`、`eframe-0.37.0/Cargo.toml:14`。

workspaceの`rust-version = "1.88"`は更新が必要。開発toolchain 1.99固定とは分け、1.95で実際に検証して宣言する。`try_update`の最小安定版を未確認のまま採用してMSRVをさらに上げない。必要なら旧APIと対象限定の警告説明を残す。全crateの警告抑制は行わない。

**段階:** 切替時に解消分を再集計し、残りを段5で整理する。警告対応でリリース済み挙動を変更しない。

### 4.13 Remote／他crate

| 対象 | 波及 |
|---|---|
| `crates/egui-ime` | 版、入力owner、helper、testを移行 |
| 4ラボ／`vst3-host-tester` | App／Ui／panel／RawInputを同じ切替段へ |
| `remote-web`／`remote-ipc` | egui直接移行・protocol変更は不要。非表示中の応答を確認 |
| launcher | egui非依存。MSRVと埋め込みcoreの更新 |
| epub-pdf-worker | egui移行なし。companion build対象 |
| comic-core | 独立ラスタライザを維持 |
| local-adjust-core／music-core | egui非依存を維持 |
| Susie／LibRaw／ORT／VST bridge | 今回のABI更新に含めない |

manifestはrootの`Cargo.toml`、各`crates/*/Cargo.toml`、`tools/*_lab/Cargo.toml`を対象に列挙する。ラボの旧eguiと本体の新版型を分裂させず、manifestと必要APIを同時更新する。

**リリース済み挙動:** Remote、launcher、workerの仕様変更は意図しない。共有serviceのlogic移動による完了採用の退行を確認する。

## 5. 利用者に相談する事項

0.37を目標にする決定は受領済み。比較可能な結果を用意し、次の外観・操作変更を採用する前に確認する。

| 項目 | 推奨 |
|---|---|
| 字面／baseline／hinting | fallback・保存値を保持して補正を比較整理 |
| color emoji | user textの代表比較を提示 |
| Window本文drag | 必要箇所で従来操作を保持 |
| mouse drag scroll | 旧版で使えた画面は保持 |
| Panel collapse／double-click | 既存操作維持を初期案 |
| 端fade | 旧外観維持を初期案 |
| margin／min size | 操作到達性を保証した比較案 |
| system font fallback | 既存明示chainを基準 |
| Ctrl+Shift+C/X/V | 通常窓・native入力欄とも従来操作を保持 |
| 受信済み入力・release確定 | 既存契約を保持。取消仕様へ変更する案は採らない |

特に、非表示化前に正当に受信したキーを後から取消す、releaseによる既存の確定を抑止する、短いpopup後のgamepad軸継続を一括変更する案は、具体的な操作差を示して相談する。第3版はこれらの変更を必須にしない。

clipboard保持のために版選択を再確認しない。縮小案を採る場合だけ、失われる操作と費用を示し着手前に相談する。

Ctrl+Q追加、OS直読み撤去、動画描画方式変更、パネル本文省略は自動採用しない。通常操作が不便になる変更は、実装前に利用者判断を得る。

## 6. 段階設計

### 6.1 共通条件

各段で通常製品ビルドを通し、対象自動検証と残る検証を記録してコミット可能にする。旧baselineの不一致は候補差分であり、成功と記録しない。

依存切替には型が結合する原子範囲がある。旧新版を同一製品graphで混在させる互換層は作らない。レビューは領域と規模に応じて分割し、固定数の票を要求しない。原子範囲が大きすぎる場合は、旧版上の挙動不変抽出を追加する。

規模は推定であり、原本置換、lockfile、PNG、純移動と実ロジック変更を別集計する。ファイル数は段間で重複するため合算しない。

反復中は`.\scripts\test-focused.ps1`または明示filterを使う。無filterのmain libは`.\scripts\test-lib-full.ps1`を使い、最終gateは段0aで非対話対象を分離した`.\scripts\test-full.ps1`で行う。

### 6.2 段0a: 実ウィンドウ試験を自動gateから分離

**最初に実施する前提修正。現test-fullを非対話gateとして先に実行しない。**

`scripts/test-full.ps1:85`のeframe全libには非ignoredの`synchronous_damage_returns_before_outer_paint_gate`が含まれ、子processで`run_native`を実行する。

根拠: `vendor/eframe/src/native/run.rs:782`、`:809`、`:828`。

AGENTS.mdは、live testの起動・foreground切替・入力の前に具体シナリオ、時間、desktop使用、使い捨てdata範囲を提示し、明示承認を待つよう定めている。testコマンド、短時間の窓、子processであることは例外ではない。

範囲:

- live testを既定自動対象から明示分離する。削除・条件緩和はしない。
- ignored等の区分と承認付き専用入口を設ける。
- pure scheduler testは自動実行を継続する。
- 他のdefault testからlive入口へ到達しないことを監査する。
- runner・文書・台帳の区分を一致させる。

既存standalone eframe testを、承認だけでエージェントへ解禁しない。エージェントのUI検証は、同等の確認を正規disposable portableシナリオへ載せ、`target/portable-smoke/mimageviewer.exe`と同dataだけを使う。standalone窓試験を残す場合は利用者実行の隔離試験とする。

**受入条件:** core check、vendor test compile、pure scheduler回帰、runner区分の静的／fake検査。窓は起動しない。

**規模:** 3～6ファイル、80～220行。live結果は段7待ち。

### 段0b: 基準固定

source、toolchain、feature、native fixture、警告、PNG、既知失敗を記録する。detached構造合意の範囲を確定する。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt --check
```

旧版焦点テストと有効な台帳を利用する。過去のAppサイズ制限失敗を現在の失敗と混同せず、現在もある失敗は上限緩和で隠さない。

**規模:** 文書・台帳3～5ファイル、100～250行。

### 段1a: 0.33.3上の小さい準備

- 未使用の直接`wgpu-hal`／`windows_058`除去。
- 旧共有経路コメント訂正。
- 本体描画側のaccessibility名付与。
- 可視layoutを変えずに済む箇所は、既存label・説明との関連付けを使う。
- 可視文字・tooltip追加を伴うものは差分を分ける。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo fmt --check
python scripts/check_ui_glyphs.py
```

変更UIの焦点試験と旧PNG通常比較を行う。名前付与だけなら画像不変を確認する。manifest変更を含む適切な区切りで`test-lib-full.ps1`を実行する。

**規模:** 12～30ファイル、180～500行。複数の小コミットへ分割可能。

### 段1b: 0.33.3上の挙動不変抽出

- service／input／render／post-renderを抽出し、旧updateから同じ順で呼ぶ。
- root／mounted／parkedのservice ownerを明記する。
- gamepadの物理drain・保持更新と、UI操作生成・multi-pass replay・配送を分離可能な形へ抽出する。
- 既存foreground判定の純粋な入口を共用可能にする。
- wheel投影、clipboard判定、出力完結・headless破棄の共通入口を抽出する。
- Win32 queue、mouse-nav pendingの既存配送先・drain条件は変更しない。

ここでは新しい非表示入力除外を導入しない。gamepadも旧呼出位置・旧順序から呼び、受付事実の利用とlogic接続は段3へ残す。

```powershell
cargo test -p mimageviewer --lib tray
cargo test -p mimageviewer --lib startup
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib sidecar_restore
cargo test -p mimageviewer --lib gamepad
.\scripts\test-lib-full.ps1
```

core check、旧snapshot、既存入力所有・gamepad終端回帰を含める。特にfocus喪失後のrouted edge採用を維持する。

**規模:** 14～28ファイル、650～1400行。App service抽出と入力／描画抽出を分ける。第2版の汎用非表示境界抽出を外し、gamepadの局所抽出へ置き換える。

### 段2: 新版vendor再構築

準備用の実ディレクトリとstandalone manifestで新版を検証し、root patchは旧vendorのまま維持する。各小段は準備差分としてコミット可能にし、正式vendorへの置換と準備物の整理を段3で行う。junction等は使わない。

| 小段 | 範囲 | 推定規模 |
|---|---|---|
| 2a | egui既存3項目、egui-winitのIME配送保証・trace・clipboard互換 | 8～14ファイル、450～950行 |
| 2b | egui-wgpuのmipmap、delta drain／submit／free、surface、診断 | 10～16ファイル、1100～2100行 |
| 2c | eframe scheduler、visible commit、viewport、delta、通常版計測、診断 | 10～16ファイル、1200～2400行 |

原本置換行数は別計上する。

第2版からの縮小点:

- 段2aからegui汎用終端APIとegui-winit非表示受付を削除。
- 段2cからpending RawInputフィルタ・受付区分追加を削除。
- 本家の未消費RawInput保持、SW_HIDE／最小化の処理分岐を維持する。

受入条件:

- standaloneの非対話区分lib test。
- Process／repeat／release／focus／allow_ime、Ctrl+Shift clipboard。
- delta whole／partial／free、no-surface、submission、headless readback。
- per-window scheduler、outer phase、SW_HIDE／最小化相当の分岐、visible child、期限超過LogicOnly。
- rootの未消費RawInput連結とhook順序を変更していないこと。
- pure testの状態・順序確認を実Window確認と呼ばない。
- 通常featureと診断featureを分ける。
- 旧rootのcore checkを維持する。
- 全ローカル差分を台帳に記載する。

live testのcompileは行えるが実行は段7とする。

### 段3: 依存と必須契約の原子的切替

範囲:

- egui系0.37、wgpu30、ラボ、egui-ime、kittest。
- App trait、root Ui、Panel、Context改名、on_exit。
- serviceのlogic接続。古いContext入力を操作として読まない。
- ModifiersChanged、wheel投影、source／phase。
- gamepadの物理更新とUI配送の接続、取込み側の局所的な受付判断。
- backendの未消費RawInput保持、Win32受信済みedge、mouse-nav pendingの既存契約維持。
- native overlay／pipeline／surface／present。
- **全製品deltaの消費・引継ぎと、headless出力の明示破棄。**
- clipboard互換、screenshot callback。
- 全test targetのcompileに必要なAPI移行。
- 新版accessibilityの追加欠落を候補画像生成前に補完。

必須の所有・消費契約を段4へ残さない。一方、汎用非表示機構は実装しない。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo test -p egui-ime
cargo test -p egui-winit --lib
cargo test -p mimageviewer --lib --no-run
cargo test -p mimageviewer --test ui_snapshot --no-run
cargo test -p mimageviewer --test book_composition_editor_snapshots --no-run
cargo fmt --check
```

compileに加えて、実接続点の次の回帰を実行する。

- delta通常描画・無描画・早期終了・headlessのdrain／引継ぎ／submit／free。
- logic-only後の未消費RawInputと復帰後の新規入力。
- focus移動前に受信した離散キーの採用。
- gamepadの同じegui frame番号中の複数logic、最小化中のpress／軸／release／disconnect、logic未実行の短い退避・復帰。
- gamepadのmulti-pass二重配送防止とringのcontext所有・Undo。
- root／childの入力所有とWH_GETMESSAGE既存drain。

graphの旧egui混入、型分裂、古いpathも確認する。compileだけでは受け入れない。

#### 段3の再見積り

以下は段1b・段2完了後の接続・移行差分であり、各行は重複計上しない。

| 内訳 | 概算変更行 |
|---|---:|
| App／Ui／Panel／一般API／ラボ | 1200～2300 |
| event／wheel／native GPU接続。delta完結処理を除く | 650～1300 |
| delta消費／引継ぎ／headless破棄接続 | 450～1000 |
| gamepad物理更新・取込み受付・UI batch接続と必須回帰 | 150～350 |
| **合計** | **2450～4950** |

**95～165ファイル程度。** 原本・lockfile・PNGを除く。

第2版の2700～5300行から、汎用非表示入力接続400～700行を削除し、gamepad局所接続150～350行を追加した。段2a／2cの削減は上表に重ねて計上しない。

実装前に抽出後の差分で再評価する。なお、gamepadの既存保持状態だけで契約を表せず大きな再構成が必要と分かった場合は、全入力へ一般化せず、その物理状態ownerの小段を追加する。

### 段4a: 入力・非表示・popupの等価経路確認

段3で成立した契約を、close／cancel／focus loss／repeat／復帰新規入力、root／FS／native／兄弟contextへ広げて確認する。

```powershell
cargo test -p mimageviewer --lib ime
cargo test -p mimageviewer --lib focus
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib keymap
cargo test -p mimageviewer --lib gamepad
cargo test -p mimageviewer --lib specified_scale
cargo test -p mimageviewer --lib fullscreen_overflow
cargo test -p mimageviewer --lib navigator
```

重点:

- §4.2の条件表に沿うSW_HIDE／最小化／visible child／期限超過。
- `routed_edge_keeps_only_discrete_input_after_focus_loss`を含む受信済み入力所有。
- IME終端、capture喪失、外側release、次の新規press。
- gamepadの非前面入力と短い復帰、既存popup後の軸継続。
- wheelのPoint／Line／Page、軸変換、混在修飾、consume前後、popup終了残量と新規wheel。
- 通常／native clipboardのCtrl+C/X/V、Ctrl+Shift+C/X/V、既存Insert/Delete、IME、空clipboard。

filterはsubstringであり実選択件数を記録する。PNGを含むfilterの外観差と機能回帰を区別する。

**規模:** 8～18ファイル、300～800行。汎用非表示機構の試験群は作らず、実経路の回帰へ絞る。

### 段4b: フォント・layout採択

baseline／fallback／toolbar／HUD、preview、margin／separator／小画面を確認する。

```powershell
cargo test -p mimageviewer --lib ui_fonts
cargo test -p mimageviewer --lib ui_font_catalog
cargo test -p mimageviewer --lib calculated_
cargo test -p mimageviewer --lib startup_dialog
python scripts/check_ui_glyphs.py
```

Fits／Scrolls、末尾到達、固定操作のviewport／clip内到達を維持する。既存条件を減らさず、§1.380の残件と今回退行を分ける。

**規模:** 8～18ファイル、400～1000行。preview変更が大きければ独立小段へ分ける。

### 段4c: GPU・native HUD整合

```powershell
cargo test -p mimageviewer --lib compare
cargo test -p mimageviewer --lib panorama
cargo test -p mimageviewer --lib gpu_lanczos
cargo test -p mimageviewer --lib native_presenter
cargo test -p mimageviewer --lib bottom_hud
```

delta・mipmap・callback、透明合成、font／DPI／galley cache、surface寿命、submit gateを検査する。診断featureが必要なheadless testも実行する。実Window試験は分離したままにする。

**規模:** 8～16ファイル、300～800行。

### 段5: 全snapshot更新と警告整理

accessibilityを通した後、通常比較で候補を収集し、外観採択後に更新する。

対象は以下のすべて。

1. `ui_snapshot`
2. `book_composition_editor_snapshots`
3. PNG生成呼出から列挙したlib snapshotのfilter集合

更新専用の子PowerShellで実行する。環境変数は全対象終了後に復元し、各コマンドの非ゼロ終了を伝播させる。

```powershell
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 台帳に列挙した全lib filterを同じ更新環境で実行
```

更新process終了後、変数なしの別processで両integrationと同じ全lib集合を通常比較する。

```powershell
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 同じ全lib filterを更新なしで再比較
```

コメント部分は任意選択ではない。段0～1で生成箇所とtestを対応付けた台帳から具体コマンド列を確定する。`--lib snapshot`だけで全件と見なさない。

画面群ごとに旧／新／diffと共通理由・例外を確認する。実装担当が候補・分類を用意し、設計担当が外観採択と目視結果を記録する。独立レビューは採択理由・例外・操作回帰の証拠を確認する。固定票やPNGごとの重複担当記録は要求しない。

Id非推奨と残るRust警告も整理する。

**規模:** UI／test 10～30ファイル、250～800行、PNG最大445枚。前倒し済みaccessibilityを再計上しない。

### 段6: 非対話全体gateと確認用build

**段0aの分離が完了したrunnerを使う。** 未修正の現test-fullをそのまま実行しない。

```powershell
cargo fmt --check
python scripts/check_ui_glyphs.py
.\scripts\test-lib-full.ps1
.\scripts\test-full.ps1
.\scripts\build-dev.ps1 -PreserveRuntime
```

通常feature、必要なtest-script、vendor非対話区分、MSRV、core／Remote／EPUBを確認する。

- source、command、feature/profile、fixture、exit、件数、ログを台帳に記録する。
- 同じ条件の成功結果を再利用する。
- focused runnerの広域変更exit 2、0件選択を成功にしない。
- 現存する既知失敗を除外・閾値緩和で隠さない。
- 自動gate成功と、分離desktop試験未実施を別報告する。
- build/checkは最低10分、広域testは最低15分以上、重い既存試験には追加余裕を取る。
- timeout後のBrokenPipe等を製品失敗と誤分類しない。

**規模:** 文書・検証補修3～8ファイル、100～300行。

### 段7: 承認付きdesktop検証・利用者実機受入れ

エージェントによるdesktop検証は、具体suite、時間、PC使用、使い捨てdataを提示し、明示承認後だけ行う。改訂依頼や一般的な開発継続指示から承認を推定しない。

- エージェントは正規portable-smoke成果物のみ起動する。
- standalone vendor窓試験を残す場合は利用者実行の隔離手順を用意する。
- scheduler live試験は短い独立枠とし、環境不成立・未実施をPASSへ変えない。
- native／DPI等の製品受入れは、段6の通常buildを利用者が起動する。
- UI確認20～35分程度と、必要なReleaseChecks／live試験を分けて見積もる。

各段はビルド・自動検証済みのcheckpointを残す。Windows native変更の最終受入れ・コミット時期はAGENTS.mdの実機確認規則に従い、未確認を完了扱いしない。

## 7. 実機確認手順

通常版・トレイ常駐版を先に終了する。single-instance mutexを共有する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

既定で実利用中の`%APPDATA%\mimageviewer`を開き、設定・データを更新し得る。隔離確認を明示的に選ぶ場合は同じ成果物へ`--data-dir .\target\dev-runtime\data`を渡す。エージェントは通常profileを起動しない。

### 7.1 フォント・ダイアログ

1. 既定、BIZ UDPGothic 9pt、Meiryo 10pt、Meiryo UI 10ptで一覧・詳細・検索・metadata・toolbarを表示する。
2. 開発側fixtureの日本語、Latin、数字、CJK、数学英字、記号、emojiを比較する。
3. baseline、字面、折り返し、欠け、豆腐化を確認する。
4. 動画／音楽HUDの時間、`x1`、`Norm`、dBを確認する。
5. 小窓・UI倍率200%で長文dialogを開き、本文・固定ボタン・×へ到達する。
6. 採択したWindow移動、mouse／touch scroll、Panel操作を確認する。

### 7.2 日本語入力

Google日本語入力とMS-IMEで少数の共通シナリオを行う。

1. 注釈本文をEnter確定し、確定だけで改行しないことを確認する。
2. 直後の通常Enterで改行が一つ入る。
3. 検索欄の確定直後Enterで検索する。
4. 変換中Escape／候補Tab／Spaceがアプリ操作へ漏れない。
5. 確定直後の通常Escapeが働く。
6. composition中に別窓へfocusを移し、重複・消失・キー残留がない。
7. 長い欄・複数行末尾で候補窓がcursorへ追従する。
8. main／F12／nativeタグ入力の代表経路を確認する。

### 7.3 shortcut・wheel・clipboard

1. main／F12でCtrl／Shift／Altを押して離し、通常操作へ戻る。
2. 編集のCtrl+矢印、Space pan、筆切替を確認する。
3. Enter／NumpadEnter、右Ctrl、Tabの代表割当を確認する。
4. popup内のwheel／Ctrl+wheelとclick／key終了で背面が動かない。
5. popup終了後の新しいwheelは働く。
6. slider／seekを外で離し、次の操作へdragが残らない。
7. 通常入力欄とnative入力欄でCtrl+C/X/V、Ctrl+Shift+C/X/Vを比較する。
8. clipboard条件・IME終了直後でも二重文字が入らない。

### 7.4 非表示・複数窓・Remote・ゲームパッド

1. 非同期処理を開始してトレイへ隠し、完了後に復帰する。SW_HIDE経路の完了採用と復帰を確認する。
2. 別にrootを最小化し、完了処理・復帰後の新規入力が働くことを確認する。
3. visible F12を残してrootを隠す／最小化し、子の新規入力と処理継続を確認する。
4. gamepadを有効にしてrootを最小化し、別アプリ前面中にボタン・軸を操作してから復帰する。過去のpressやrepeatが復帰後の操作にならないことを確認する。
5. gamepadを中立・releaseへ戻して新しく操作し、通常どおり働くことを確認する。
6. ring操作の終了・disconnect後に、対象contextとUndoが保たれることを代表一例で確認する。
7. composition中のfocus移動、drag中の外側releaseを行い、復帰後の文字・押下状態を確認する。正当な確定まで取消されていないことも見る。
8. 同じPDF／画像の兄弟reader片方を閉じ、もう片方の表示を保持する。
9. root非表示中のRemote要求が応答・完了する。
10. idle／wakeは既存ReleaseChecksの承認済み手順で確認する。

「非表示化前後の入力はすべて捨てる」を期待結果にしない。focus移動前の受信済みキー、同一batch内の順序、logic未実行の短い退避・復帰、期限超過分岐はheadless／fake回帰で確認する。実PCへ任意キーを大量送信する試験は行わない。

### 7.5 動画HUD・GPU

1. 再生、HUD文字、時間、速度、音量、音声トラックを確認する。
2. 820px程度から狭幅へ縮小し、重なり・menu・seek／音量dragを確認する。
3. pause／resume／seek／track変更／動画→音声→動画を行う。
4. F11／F12往復、focus、透明合成を確認する。
5. 比較wipe／diff、panorama、縮小でmipmap・黒表示・境界を確認する。
6. fullscreenから一覧へ戻り、黒サムネ・atlas破損がないことを確認する。

### 7.6 マルチモニターDPI

1. 100%と150%／200%の画面間でmain／F12／動画窓を移動する。
2. 字面・baseline、候補窓、HUD hit rect、slider／seek位置を確認する。
3. 最大化・復元・F11往復で操作が画面外へ失われないことを確認する。
4. 子窓終了後もrootの表示・入力・撮影が継続する。

## 8. 完了条件と文書更新

完了には次を必要とする。

- 新版graphとvendor全差分を説明できる。
- egui-winit保持理由と将来削除条件が記録されている。
- texture deltaのdrain／引継ぎ／明示終了とsubmissionが全経路で成立する。
- serviceはUI省略時にも必要なwakeで進み、logicから古いContext入力を操作として読まない。
- SW_HIDE／最小化／visible child／期限超過の分岐を区別した回帰が通る。
- backendの未消費RawInput保持、受信済み離散キーの所有、Win32終端、mouse-nav pendingの既存契約を保持する。
- gamepadの物理更新とUI配送が分離され、操作先不在期間の入力・短い復帰・release／disconnect・ring所有の回帰が通る。
- IME終端・pointer captureの回帰結果に基づき、必要な所有境界だけを修正している。
- 汎用終端API、hide時の一律prune、復帰時の一回捨てを追加していない。
- Ctrl+Shift clipboardの従来操作を通常／nativeで保持する。
- accessibilityを通した全snapshot対象を更新・通常比較・目視確認する。
- 非対話gate、確認用build、必要な承認付きdesktop試験、利用者実機確認が完了する。
- 未実施・環境不成立・未確認機器を確認済みと書かない。
- リリース済み機能・設定・データを黙って縮小・初期化しない。

更新対象はdisplay-pipeline、video-architecture、coding-conventions、keymap-spec、ime-process-keys-plan、ui-snapshot-policy、development-build-and-test、interactive-release-verification、detached-rework-plan §11、vendor台帳、backlog §1.382。§1.380は個別解決の証拠がある項目だけ更新する。

採択した操作・外観変更はマニュアルと製品ページへ反映する。版更新だけを理由に未確認の改善・互換性を告知しない。