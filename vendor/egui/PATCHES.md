# egui 0.37.0 のローカル変更（段2a-1／2a-2／2a-3）

原本とchecksum・VCSの記録は [upstream-provenance.json](../upstream-provenance.json) を参照。
段2a-1では0.37.0原本を基準に、旧`vendor/egui`の既存パッチを付け直した。
段2a-2のIME中断要求の生成・集約修正と段2a-3の非描画退役は末尾の追補に記録する。
rootの依存切替・製品操作差の受入れはこの段階に含めない。
README、Cargo.toml.orig、同梱Cargo.lockは原本を保持する。

## 差分台帳

分類は [更新計画 §4.8](../../docs/egui-0.37-update-plan.md#48-vendorパッチ台帳) に従う。

| 項目 | 分類 | 変更と保証 |
| --- | --- | --- |
| `PointerState::released_buttons_with_click_counts` | (b) 保持 | `src/input_state/mod.rs`。本家が判定した全button releaseを順に値へ射影し、非clickは`None`で残す。判定やイベントの生成・消費は変更しない |
| 旧クリック分類fixture | (c) 新分類へ移行 | 旧位置列`(20,20)→(30,30)→(40,40)→(50,50)`は全てcount 1。別の近接位置列で1/2/3/3を検査し、全button・非click・focus区間・順序・discard後の非再生を維持する |
| `InputState::discard_pending_scroll` | (b) 新ownerへ適応 | `src/input_state/mod.rs`。0.37の`WheelState::unprocessed_wheel_delta`だけをゼロにし、scrollとwheel zoomの未来passへの残量を終端する |
| repaint delay witness | (b) 保持 | `src/context.rs`とCargo.tomlの`miv-repaint-witness` feature。現在viewportのschedulerを読み、outstandingがあれば0、有限delayならその値、要求なしなら`None`。状態を変更しない |
| 本家click分類 | (a) 差分不要 | 0.37の距離・時間条件をそのまま使用する。旧分類を再実装せず、本家の事実を別回帰で固定する |
| license本文 | (b) 保持 | 旧vendorのMIT / Apache本文をbyte変更なしで同梱する |

段2a-1ではIME中断要求のviewport所有・pass出力集約を変更していない（段2a-2追補を参照）。
非描画viewport退役入口は段2a-3追補を参照。

## Wheel終端の境界

前面popup等の既存入力ownerが終了するpassの終端に呼ぶ。現在passのpublic deltaと
wheel eventの消費は呼出側が行う。新WheelStateでは旧scroll／zoomの二つのqueueが一つに
統合されているため、未配送deltaだけを破棄する。現在delta、pointer、focus、touch、
gestureのstatus・source・時刻・modifier latchは保持し、次passの新規wheelは通常どおり処理する。
開いている間の平滑化を毎pass消す用途や、汎用の非表示入力終端には使わない。

## 回帰と検証の扱い

- 射影は旧2回帰を移植し、実`Context::run_ui`でdiscardを通す。検査後のFullOutputは
  `drop_without_applying_deltas`でheadless明示破棄する。
- 分類は近接／離れたclick、6 logical points未満・ちょうど・超過（倍率1／2）、
  doubleの厳密な時間境界、tripleの直近二区間条件を検査する。
- wheelはscroll／zoom双方の未配送残量、現passと所有状態の保持、後続idleでの非再生、
  新規wheelの受理を検査する。終端しない比較経路では残量が再生されることも確認する。
- witnessは要求なし／遅延／即時／outstandingを読み、schedulerが変わらないことを検査する。

準備graph・実行件数・command・結果は親担当の段2a検証台帳に記録する。
本書は旧版の成功結果を0.37へ流用せず、未実行の回帰を成功済みとは扱わない。

## ライセンス

packageの`MIT OR Apache-2.0`宣言は維持する。原本crateには本文がないため、
旧`vendor/egui`の同名本文をコピーする。

| コピー元 | SHA-256 |
| --- | --- |
| `../../vendor/egui/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../../vendor/egui/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |

## 段2a-2: IME中断要求の生成・集約（2026-10-17）

設計第4.4版§4.5と、detached計画§11で合意済みの所有境界を準備graphで実装した。
対象HEADは`8023e4bdd`。以下は**実装担当の内部確認**であり、正式な独立レビュー・受入れではない。

| 項目 | 分類 | 変更と保証 |
| --- | --- | --- |
| IME中断要求のviewport所有 | (b・新規) | `src/memory/mod.rs`。Memory共通のrequested_interrupt_imeを既存のviewport別Focusへ移す。interrupt_ime、request_focus、request_focus_next_frameは現在のFocusだけを更新する |
| pass開始・出力の対象 | (b・新規) | Focus::begin_passで対象entryだけresetする。end_passの既存getterは同じviewportのFocusを読み、子end後の既存viewport復帰で親Focusへ戻る。手動退避・復元や第二mapは作らない |
| 未配送PlatformOutput集約 | (b・新規) | `src/data/output.rs`。同じviewportの新IME metadataを採り、中断要求は旧・新のORを保持。新Noneなら旧Someを保持し、take後や次の新規runへ要求を持ち越さない |
| 実入れ子・実discard回帰 | (b・新規) | `src/context/ime_interrupt_tests.rs`、登録はcontext.rs。共通名ime_interruptで選択する6件。親子のPlatformOutputは混ぜない |

Focus map全体が既存のserde(skip)対象であるため、新しい永続形式・設定は作らない。
移管した既存要求以外のbool、App補助フラグ、追加repaint、新queue、retryは追加していない。
後続の非描画退役・eframe・egui-wgpu・root製品graphには触れない。

回帰は実show_viewport_immediateのheadless rendererから、同じContextの子run_uiを実行する。
実TextEditと事前focus passを使い、親のみ／子のみ／両方／なしについて3要求入口を検査する。
親子ともSome(IMEOutput)、各1pass、親へのviewport復帰、次の要求なしrunで両方falseを確認する。
rendererはthread-localの元callbackへ必ず戻す。Memoryのviewport IDをテストから切り替えない。

実discard回帰は第1passだけ要求し、第2passでTextEditの位置とpurposeを変える。
再passの実行、最新rect／cursor_rect／purposeと中断要求true、次の新規runでfalseを検査する。
補助回帰はSome/None・要求有無の16組とtake後の消失を検査する。
全FullOutputは各viewport別に検査用metadataを採取してから、headlessで明示破棄する。

修正前は6件中4失敗。所有修正だけ／集約修正だけを戻す変異はそれぞれ4成功・2失敗で、
修正済みbytesへ復元後は6件全成功。準備graphの全lib結果はegui 98件、egui-winit 43件、
egui-ime 26件が成功（各exit 0）。commandと証拠は[段2台帳](../../docs/egui-0.37-stage2-verification.md)に記録する。

本家にも報告する価値がある。実入れ子run_ui／実discardの最小回帰と、各修正の独立した修正前後の証拠を添えられる（今回は未送信）。

## 段2a-3: 承認済み子viewportの非描画退役（2026-10-17）

開始HEAD `9c54f4f1d`。設計第4.4版§4.2・§4.8・段2a順序11と、detached計画§11の
Close lifecycle構造合意に基づく。以下は**実装担当の内部確認**である。

| 項目 | 分類 | 変更と保証 |
| --- | --- | --- |
| `Context::retire_viewport` | (b・新規) | 承認済み子とその子孫の登録を終了し、実際に除去したIDを返す。ROOT／pass中は拒否、未知／重複はno-op。同じIDの明示的な新規openは可能 |
| 通常GCと共通の退役処理 | (b・新規) | `src/context.rs`。viewport登録・親関係・viewport別Memory・font galley cacheを同じ処理で除去。子孫を同じ終了境界へ合流する |
| viewport別Memory終了 | (b・新規) | `src/memory/mod.rs`。対象のareas／interactions／Focus（IME中断要求を含む）／popupsだけを除去。現在ownerが対象ならROOTへ戻す。兄弟は変更しない |
| callback所有物の破棄 | (b・新規) | 明示入口と通常GCとも、除去したViewportStateをContext lockの外へ移してからdropする |
| 実Context回帰 | (b・新規) | `src/context/viewport_retirement_tests.rs`の8件。兄弟の入力・実TextEdit IME・状態、後続出力、hide/minimize、通常GC、owner復帰、lock外drop、未配送texture delta、入口制約を検査 |

呼出しはcompleteなrun_ui／run_pass／run_logicの終了後、plugin callbackおよびbackend／Contextの
lock外とする。先行出力・texture deltaの配送と、閉じたIDの新規publish停止はbackend ownerの責務。
共有texture manager・widget data／cache・plugin所有状態は消去しない。新しい抑止ラッチ・
退役済みIDリスト・App補助フラグ・追加repaint・保持queue・retryは追加しない。
egui-ime pluginの状態は従来のRawInput live-set処理に従うため、本入口でplugin全体をresetしない。

Memory終了を外す変異は4成功／4失敗、通常GCの共通処理を外す変異は6成功／2失敗（各exit 101）。
修正済みbytesへの復元後は8件全成功。全libはegui 106件、egui-winit 43件、egui-ime 26件が成功。
egui-winitの既存兄弟IME回帰は、metadataだけでなく親からdeferred子登録を維持するfixtureへ適応した。
producerとassertは変更していない。command・初回失敗の原因・証拠は[段2台帳](../../docs/egui-0.37-stage2-verification.md)に記録する。

eframeのClose受付・消費・backend window／surface／State終了は段2cへ残す。
本家へは通常GCのviewport別Memory残留とlock下callback破棄を、最小回帰と共に報告する価値がある（未送信）。

## 段2c-4a: 処理済み子Closeのcached metadata消費（2026-10-17）

`Context::consume_viewport_close`を限定追加した（b・新規）。**実装担当の内部確認**。
eframeの子Close hookが承認／拒否した一回だけ、既存の全ViewportState.input.raw.viewportsから
対象Closeを消費する。root／UI pass中は拒否し、通常input events・兄弟Close・他metadataは保つ。
public input_mut_forで未知IDを列挙すると登録を作るため、既存内部mapだけを走査する。
未知IDで登録を作らず、新しいCloseは抑止しない。新状態・queue・一括resetは追加しない。
実Contextのrun_logic後snapshotと次の新Closeを検査する回帰を追加した。
承認時の登録終了は既存retire_viewportのままで、IME policy整理は準備egui-imeの明示入口へ接続する。
検証結果は段2台帳。正式な独立ACCEPT・実機確認ではない。


## Stage 3: approved viewport retirement (design revision 4.5, 2026-10-17)

Implementation record only; independent acceptance and execution evidence belong
in the stage-3 verification ledger.

- ViewportRegistration identifies one real Context registration. The read-only
  Context::viewport_registration lookup never creates a target. Existing App
  bindings may retain Context plus this identity after Context GC.
- approve_viewport_retirement captures the exact target and owned descendants,
  normalizes overlapping approvals in the existing root command column, and
  emits RetireApproved. It does not request Close, repaint, UI, or native cleanup.
- Full/LogicOnly/direct begin-end/pass-outside drains share the output contract.
  Dynamic run_depth and the existing viewport stack keep authority in that same
  root column through immediate nesting and discard retries. No second queue,
  pending flag, retirement history, or retry was added.
- ViewportOutput carries its own registration and exact parent registration.
  append never mixes builders/callbacks/ordinary commands from different lives.
  Old output cannot modify or recreate a reopened same-ID native viewport.
- retire_approved_viewports uses the normal target-limited Context GC cleanup;
  user callbacks drop outside the Context lock. Additional TextureHandle frees
  can be transferred to the existing shared renderer owner.
- consume_registered_viewport_close checks and consumes atomically, preserving
  Close metadata belonging to a new same-ID registration created by a hook.

The new stage3_viewport_retirement_tests module adds six real Context scenarios
for logic, GC/discard, immediate nesting, append/reopen, overlap/direct end_pass,
and unlocked callback drop/additional free. Existing stage-2 retirement tests
remain intact. The first compile found a module-name collision and an associated
setter called as a method; both source errors have been corrected.

An additional discard/native-reopen texture ordering gap remains unresolved:
run_dyn can retain pass-1 texture output while a pass-2 immediate renderer submits
an old native callback's free first. No new delivery callback has been added;
the design owner must decide that new ownership boundary before implementation.

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


## Stage 3: discard texture delivery (design revision 4.6, 2026-10-17)

Implementation record and implementer internal inspection only. Independent
acceptance and command results belong in the stage-3 verification ledger.

- Context::enter_discard_texture_delivery installs a Context-identity-bound,
  thread-local resource dispatcher with an RAII restore guard. Captures can be
  non-Send; native backends capture Weak references to existing renderer owners.
  The dispatcher owns no pending delta and invokes consumers outside its TLS
  borrow and Context/texture-manager locks.
- run_dyn moves only its accumulated textures_delta after a real retry is
  confirmed, after output hooks and before the next begin_pass. Final/limited
  passes and direct begin/end keep ordinary FullOutput delivery; no matching
  scope preserves headless behavior. Nested immediate retries share the scope.
- Four discard_texture_delivery regressions cover Context isolation, normal /
  early-return / unwind restoration, limits/direct/empty output, actual plugin
  mutation/IME output and receipt-model observation, and nested immediate retry.
  Receipt-model tests are not evidence for the product page-turn plugin.

## Stage 5: Rust 1.99 warning cleanup (2026-10-17)

- `src/containers/scene.rs`: remove the `core::f32` module import so
  `f32::EPSILON` resolves to the primitive associated constant instead of the
  deprecated module constant. The numeric value and scene behavior are unchanged.
  No TextEdit compatibility patch is added; the upstream 0.37 same-batch pointer
  focus/Text behavior was explicitly accepted by the mIV user.

## Stage 7: retain the unpublished font reservation (2026-10-17)

`Context::take_viewport_retirement_texture_delta` now checks the existing
Managed(0) TextureManager metadata under the same write lock before taking any
pending delta. Until the first real atlas is published, every update remains
with TextureManager. The first normal end_pass replaces the upstream 0x0 font
reservation with the valid atlas and exports the complete batch. Nothing is
filtered, discarded, replayed, or moved into an additional pending owner.

The pass-outside retirement collector previously drained that reservation on
the first ordinary WindowEvent, even without a close request. A ready wgpu
renderer then attempted to create a zero-width texture. Upstream 0.37's ordinary
Full path publishes the real font atlas before draining, and its renderer has
no zero-size discard contract. Published-atlas metadata (rather than the
existence of Fonts during begin_pass) defines this resource publication boundary.
Subsequent no-paint updates/free-only delivery and font reload remain supported.
The Context regression checks pre-Full whole/partial/free ownership, real font
definition changes, and no duplicate delivery. See the stage-7 startup ledger.

## §1.445: stepped Slider text commit boundary (2026-10-26)

- `src/widgets/slider.rs`: stepped sliders use DragValue's existing deferred
  text-edit mode. Typed text stays private until Enter or focus loss; only then
  does Slider apply its step rounding. Integer sliders have an implicit step of
  one and follow the same rule. Dragging, arrow-key increments, and accessibility
  increments remain immediate. Sliders without a step retain live text updates.
- The old local-value bridge let DragValue store `EditState.value = 2`, then
  Slider rounded storage to 0. On the next frame DragValue's external-change
  check discarded the text. egui 0.33.3 stored only a String and did not discard
  it this way. That check was introduced by upstream
  [#8403](https://github.com/emilk/egui/pull/8403), merge commit
  `d802a982ce959839d92f4f6718fdffa41143110e`, included in 0.36.2.
  The external-change check itself remains intact.
- Passive `Always` clamping of stepped sliders clamps the existing value without
  step snapping, preserving existing off-grid settings. Sliders without a step
  retain their existing decimal rounding as well as live updates. No settings
  migration or application-specific workaround is added.
- `src/widgets/drag_value.rs`: remove finished edit state after commit/cancel,
  including the lost-focus path. Otherwise Escape could leave text to be
  committed on the next frame, after the Escape event was gone.
- `src/widgets/slider_tests.rs`: headless Context input regressions cover
  digit-by-digit values, partial-value isolation, commit/cancel, deletion/caret,
  paste/clamping, off-grid storage, rail drag/arrows, numeric arrows, siblings,
  external changes, implicit integer steps and standalone deferred DragValue.
  A click-frame check ensures an apply button sees the committed value before
  closing the UI, without requiring another frame.

Evidence, scope limits and verification commands are recorded in
[`docs/section445-stepped-slider-input.md`](../../docs/section445-stepped-slider-input.md).
