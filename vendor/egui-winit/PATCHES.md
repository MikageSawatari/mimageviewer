# egui-winit 0.37.0 のローカル変更（段2a-1）

原本の版・archive checksum・VCS・全ファイルhashは
[upstream-provenance.json](../upstream-provenance.json) に記録する。
0.37.0原本へ必要な旧差分を付け直し、旧manifestを上書きコピーしていない。
README、Cargo.toml.orig、同梱Cargo.lockと、元のclipboard等の他ソースは原本を保持する。
準備graphは親の [Cargo.toml](../Cargo.toml)、製品graphは旧vendorのままである。

## 差分と分類

| 項目 | 分類 | 実装・根拠 |
| --- | --- | --- |
| Windows Process基本機構 | (a) 本家を土台 | 0.37にもProcess遮断があるため旧基本機構の二重追加はしない。HashSetの配送履歴不足だけを次行で置換する |
| Hidden / Forwarded | (b) 保持 | StateごとのHashMapが物理holdの配送履歴を所有。Process repeatでForwardedをHiddenへ戻さない |
| releaseとfocus終端 | (b) 保持 | releaseはIME許可・logicalに依存せずremoveしてHiddenだけ遮断。focus喪失は対象Stateだけclear。IME許可変更・Disabledでは履歴を消さない |
| Disabled producer通知 | (b) 付け直し | `State::on_ime`で非推奨通知を限定的に配送。WindowsのDisabled-only取消を共有normalizerへ届ける。Enabledも保持（旧Linux例外を維持）。理由付き非推奨許容は変換armに限定する |
| State→正規化→TextEdit回帰 | (b・新規) | `disabled_ime_tests.rs`。本物のState変換・準備用egui-ime plugin・実run_ui TextEditを通す。合成normalizerやwindowは使わない |
| trace・文字秘匿 | (b) 保持 | `ime_key_trace.rs`の既定無効・共通時計・worker I/O・append範囲観測を維持。構造体PreeditとDeleteSurroundingへ適応。新版の印字キーIntlBackslashも他の文字キーと同じく秘匿。通常ログのlogical key全文とinvalid rangeのpreedit全文も出さず、入力文字と範囲を秘匿する |
| 配送・State・網羅回帰 | (b) 保持 | `keyboard_input_tests.rs`へ既存17件を全移植。6入力×IME許可2通りの長さ1〜6全3,257,436列を省略しない |
| egui-ime依存・clipboard共有 | (b) 保持 | 通常path依存は準備用`../egui-ime`。Key/Text/clipboardより前にProcess判定。共有winit条件でCtrl+Shiftを保持し、専用キー・Insert/Delete・論理優先/物理fallback・CRLF・空Paste・early returnも保持 |
| 新modifierとclipboard API | (a) 本家を土台 | Stateのmodifier owner・イベント順・runtime OS判定を維持。0.37のclipboard_shortcuts設定とimage paste fallbackも維持し追加回帰で確認。modifier変換をprivate helperへ挙動不変で抽出 |
| 旧has_sent_ime_enabled・合成通知 | (c) 不要 | 新TextEditと正規化はPreedit/Commitを直接処理する。旧enable latchやCommit後の合成Disabledを戻さず、実OS通知だけを互換配送する。新Preeditのactive_range_chars・Noneを維持 |
| 旧300ms補正 | (c) 撤去済み | 時間窓・pending boolを復活させない |
| Windows枠なし窓の影 (§1.426) | (b・新規) mIV固有契約の復元 | 本家 #8169 の常時影要求と #8449 のwinit fullscreen限定除外は、独自borderless全画面を使うmIVではv4.8.0からclient原点を1pxずらす回帰になる。生成時の `with_undecorated_shadow(false)` とDecorations時の `set_undecorated_shadow(false)` で全枠なしviewportをv4.7.1の影なしへ戻す。影を必要とする窓がないためper-viewport APIは追加しない。本家へは報告しない。非表示実HWNDで生成・true→false更新のclient/window原点・寸法一致を固定し、修正前は両方の原点不一致で失敗。タスクバーとの因果・native動画の解消は実機未確認 |

新manifestの追加は共有crateの依存edgeと、headlessテスト専用egui default_fontsだけ。
通常の共有crate依存から既定フォントは入れない。新版依存版・featureは原本を基準にする。

## 回帰の境界

Disabledは同一/別pass、focus喪失後、Commit後、重複通知/空Commit、通常選択、兄弟State/viewport、
直後の通常Enter/Escapeを検査する。Disabledでphysical holdが消えないことも別に検査する。
Unicode byte→character範囲変換、invalid/None範囲、Enabled配送、modifierイベント順、
Ctrl+Shift+C、clipboard無効設定、画像paste、traceの新variant秘匿も検査する。
実windowを必要とする`take_egui_input`のgeometry更新だけを除外し、Stateが所有するRawInputを
`take`して実Contextへ渡す。headless FullOutputは検査後に明示破棄する。

本家同様Disabledを捨てる変異で取消回帰が失敗し、復元後成功する証拠は
[段2検証台帳](../../docs/egui-0.37-stage2-verification.md) に記録する。
この文書は実装担当の内部確認であり、正式な独立レビュー・製品操作差の受入れではない。

段2a-2のIME中断要求・discard集約、段2a-3の非描画退役、eframe/egui-wgpuには触れない。
vendor削除は、Hidden/Forwardedの配送保証とDisabled取消が本家同等になり、traceも不要と
判断できた時点とする。Enabled/Disabledを警告削減だけで削除しない。

## ライセンス

宣言`MIT OR Apache-2.0`を維持。原本archiveに本文がないため、旧vendorの同名本文を
byte変更なしで同梱する。

| コピー元 | SHA-256 |
| --- | --- |
| `../../vendor/egui-winit/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../../vendor/egui-winit/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |


## Stage 3: terminal command ownership (design revision 4.5, 2026-10-17)

RetireApproved belongs to the eframe pass-outside consumer. Reaching ordinary
window-command processing is a contract violation, rather than a Close event or
a silently dropped command. Existing Close/CancelClose behavior is preserved.

## Stage 3: formal vendor graph

Earlier descriptions of vendor-next/preparation-only graphs are historical.
Stage 3 uses the formal vendor/ crates from the root Cargo.toml and Cargo.lock.
Each normalized vendor manifest has its own [workspace] and [patch.crates-io]
pointing at the same formal egui, egui-winit, egui-wgpu, and eframe crates.
Cargo.toml.orig and the upstream README are retained. Original upstream lockfile
hashes remain in the provenance inventory. Active standalone Cargo.lock files were
initially copied from the root lock during WIP setup and are resolved/recorded by
ordinary offline Cargo; they are not original upstream lockfiles.
The shared IME implementation lives in crates/egui-ime; egui-winit and
eframe use ../../crates/egui-ime. Archive/VCS/checksum and original file hashes
were moved to vendor/upstream-provenance.json. Graph migration is not evidence
that runtime checks or release gates have passed.

## 依頼50 A: normalized pointer observer（2026-11-14）

Parked静止画の復帰gestureを、後続passでRawInputを読む時点ではなく、実入力を生成した時点の
ownerに所属させるための観測APIを追加した。Active時に蓄積したDown/Upを、Immediateから
Deferredへ切り替えた後の新callbackが復帰操作として再所属させないために使う。

- `pointer_input_observer`の公開APIはContext共有のobserverを登録・解除する。
  `State::on_window_event`は既存変換をinnerへ委譲し、そのWindowEventが実際に追加した
  `egui_input.events`のsliceだけを観測する。observerはContext data lockを解放してから
  最大一度呼ぶ。利用側は共有input ownerへのenqueueだけを行い、App/eframeへ再入しない。
- observerは`Option<egui::Id>`を返す。Pressedの生成時ownerを証明できた場合のopaque tokenだけを、
  そのWindowEventが追加した最初のraw Touch Startまたはprimary pressの直前へ
  `Event::PointerInputProvenance(id)`として挿入する。NoneとReleased/Cancelledの返値では挿入しない。
  markerは非操作イベントで、古いActive入力と新しいParked入力が同じbatchへ合流しても
  新しいproofを古いDownが奪わないための相関である。event indexによる推測は使わず、
  RawInputのtake/append、旧Stateの保留入力との合流、key releaseの先頭追加、raw列の再利用でも
  markerが対応Startと一緒に移動する。eguiの通常pointer操作を生成・取消するmarkerではない。
  egui自身のdiscard再passはvolatile eventsを再配送しないため、markerもStartも再配送しない。
- sourceは物理WindowEventに従い、MouseInputのtouch simulationもMouse、TouchはTouch(id)とする。
  mouse primaryのraw pressがなければPressedを生成しない。位置消失等でraw releaseを追加できない
  mouse releaseはCancelledを通知し、既存gestureを未来のreleaseへ残さない。
  Touchはraw Start/End/Cancelを使い、primary昇格のない指も通知する。昇格分の二重通知はしない。
  motion、focus、keyboard、非primary mouseは復帰producerにしない。
- RawInput/IME/event列を消去・再構成せず、State fieldを追加しない。winitのWM_POINTER履歴や
  WM_TOUCH、eframeのallocation/lifecycleには手を入れない。headlessテストのため、既存のtouch
  変換が必要とするWindow由来のscaleとcursor座標変換だけをhelper境界へ分けた。

回帰は実WindowEvent・Stateの実mouse/touch producer・追加sliceのmappingを使い、位置なし、
既存queueの非再所属、touch simulation、非昇格touch、取消、二重昇格なし、Context/viewportの
所有、callbackからのContext参照・observer解除を検査する。Window/EventLoopは生成しない。
provenanceのSome/None、fresh Start直前だけの挿入、Touchの昇格有無、terminal返値の無視、
RawInputの実take/appendとkey先頭追加、eguiのdiscardによる実再passも検査する。
実Window経由の配送・実機での復帰はこのheadless回帰の証明範囲には含めない。

上流へ戻す際は、このAPIを削除するだけで利用側を古いRawInputの推測へ戻さないこと。
同等の生成時通知・物理source・一イベント一通知・Context lock外callbackの契約を保ち、
利用側のowner証明と、markerによる実Startとの相関を合わせて移植する。
単なるnative Down/Up通知やbatch内FIFO/indexで置換してはならない。
## 依頼55 独立レビュー: 位置なしreleaseと診断sink（2026-11-15）

winit 0.30.13 Windowsはcapture中にも窓外座標のCursorMovedを送り、後着の
WM_MOUSELEAVEがCursorLeftだけを出し得る。WM_LBUTTONUPはReleaseCaptureの後に
位置なしMouseInputを送り、last_positionを更新しない。logical位置Noneはrelease取消の証拠ではない。

Stateは実MouseInputでforwardした保持をbuttonごとに所有し、位置なしの実Upは
eguiの`PointerButtonReleased`で一度だけ終える。過去の座標をPointerButtonへ代入しない。
observerはこのraw終端をReleasedとして認め、downを見ていないUpは従来どおりCancelled。
touch昇格は物理mouse保持を作らず、State再生成でも保持を移さない。
simulate_touch_screenのraw Touch Endは既存Touch契約の最後に検出した点で終え、hoverを復元しない。
上流へ戻す際はegui側の位置なし終端とAppのordered consumerを一緒に移し、
通常のPointerGoneはdownを消さないという上流の意味を維持すること。

診断Infoのtargetは`egui_winit::pointer_input_observer::diagnostic`へ固定する。
製品loggerはこの正確なtargetだけを許可し、vendor全体のInfoは開けない。
入力破棄の押下境界だけで出力する。通常sinkへの到達も製品側の実writer回帰で検査する。
caption buttonの判定はvendorで推測せず、既存native ownerの解放hit／WM_SYSCOMMANDに委ねる。
根拠・headlessと実機未確認の区別は依頼50調査書§5.8を参照。

## 依頼55: client入力破棄の診断（2026-11-15）

primary押下の境界だけで `PointerInputDiscardReason` を通常ログへ出す。
Stateにpointer位置がない場合は `PointerPositionUnavailable`、raw押下が生成されても
Context共有observerが未設置なら `ObserverNotInstalled`。viewportと現在registrationを併記する。
入力生成・配送・cursor寿命は変更せず、位置のOS再探索、observerの再設置、retryはしない。
上流移植時は観測と対処を分け、この診断の代わりに位置推定や入力再生を導入しないこと。

## 依頼55 A: Windowsの同座標再入とpointer位置の寿命（2026-11-15）

winit 0.30.13のWindows backendはWM_MOUSELEAVEでCursorLeftを出すが、
MouseState::last_positionを保持する。再入時のWM_MOUSEMOVEはCursorEnteredを出しても、
同client座標ならCursorMovedを省略する。egui-winitがLeftでlogical位置を消した後、
Enteredで復元しなかったためMouseInputからPointerButtonを作れなかった。

- Windowsの各Stateは最後のCursorMovedの物理位置を保持する。
  winitのlast_positionと保持位置はCursorMovedごとに一致する。Leftでは従来どおり
  pointer_pos_in_pointsをNoneにしてPointerGoneを出し、物理位置は保持する。
  Enteredは保持した値を現在の実効pixels_per_point（native scale×egui zoom）へ変換し、
  通常のon_cursor_movedと同じPointerMoved生成を通す。後続の別位置Movedはそれを上書きする。
- Touch End／Cancelのlogical位置清掃では物理mouse位置を消さない（winitも消さない）。
  新規StateはNoneで始める。Windows eframeのState再作成はnative window再生成と同じ寿命で、
  別allocation／兄弟Stateへ履歴を引き継がない。根拠範囲に合わせ、Entered復元はWindowsだけとする。
- `State::on_pointer_event` はWindow不要のpointer-only変換API。
  normal WindowEvent経路と同じprivate変換helperを使い、normalized observerを一度だけ呼ぶ。
  normal経路のAccessKit／IME traceとouter observerの位置は維持する。
  非pointer eventはNone／無変更。Appのheadless反復試験もこの正規変換で実observer／proofを通す。

回帰は同座標Left→Entered→Down、別Movedの上書き、履歴なし、倍率変更、touch終端後、
root／child分離、新Stateの履歴なし、observer一回を検査する。
実機の依頼55との対応とpart3の座標差は未観測で、原因機構の証明とは区別する。
上流へ戻す際はwinitのlast_position／Entered／Movedの契約を再確認し、
入力の位置推定・OS照会・rect探索・次frameの再送へ置換しないこと。
