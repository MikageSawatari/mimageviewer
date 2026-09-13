# §1.164 修飾キーによる切り取り枠の新規作成

更新日: 2026-09-13  
状態: 実装・自動検証・確認ビルド完了（Windows 実機確認待ち）

## 1. 目的と現状

切り取りモードでは、ページに保存済みの枠がない場合も表示用に
`CropRect::full` を使う。`ui_crop.rs` の `target_at` は枠内を
`CropHandle::Body` と判定するため、画像内の通常ドラッグは全画面枠の移動となり、
任意の位置から小さい枠を作り始められない。

修飾キーを押した状態で画像上の左ドラッグを始めたときだけ、ハンドルや枠内の
hit test より新規作成を優先する。既定は Ctrl とし、操作カスタマイズから変更・
無効化できるようにする。画像外、左のツールパネル、ダイアログ上から新規作成を
始める機能は追加しない。

## 2. 現行の所有境界

- `App::handle_export_crop_pointer` が、crop overlay のポインタ入力を受ける唯一の入口である。
  通常開始時は `target_at` の結果から `ExportCropDrag`（Body / 8 ハンドル）か
  `ExportCropCreateDrag` を選ぶ。
- 開始後の移動・リサイズは `ExportCropDrag`、新規作成は
  `ExportCropCreateDrag` が release / cancel まで所有する。release 時だけ現在値を
  `set_export_crop_for_idx` へ渡し、既存 DB・sidecar・preview 無効化経路を通す。
- 一時パンは `CropSpacePan` と `fs_pan_drag_start` が所有する。crop より先に
  `handle_overlay_space_pan_drag` へ渡される。
- crop は単一 foreground editor である。linked fullscreen では現在 mount された
  App projectionを使い、独立・parked viewer では編集入口を出さない。park / close /
  page switch / mode reset は既存の edit-mode teardown で drag owner を破棄する。
  今回、別 context 用の状態や App bool は追加しない。
- `focused_key_state_permit` と `keymap_owner_blocks_shortcuts` が、現在 viewport の focus と
  modal / TextInput / IME の keyboard owner を守る。新しい hold 判定もこの permit を必須にする。

## 3. 開始時の一回分類

開始フレームだけ、次の typed classifier で操作を決める。

1. `CropSpacePan` が有効なら `Pan`
2. 新 Action `CropForceCreateHold` が有効で、press origin が表示画像内なら `Create`
3. `target_at(origin)` が返れば `Edit(CropHandle)`
4. target はなくても origin が表示画像内なら `Create`
5. それ以外は `None`

`Pan` は既存 `fs_pan_drag_start`、`Create` は既存 `ExportCropCreateDrag`、`Edit` は既存
`ExportCropDrag` へ格納する。以後、Ctrl の押下状態を読み直して owner を切り替えない。
したがって、create 開始後に Ctrl を離しても create のまま、通常の Body / handle drag
開始後に Ctrl を押しても edit のままである。Space pan 自身の「Space を離すと終了」は
既存仕様を維持する。

hover 中は、force-create が有効な画像内では Crosshair、Space pan が有効なら既存の Grab
を表示する。開始後の cursor は各既存 owner が決める。

## 4. Space と修飾キーの競合

`CropSpacePan` は `KeyTrigger::KeyHold / SinglePlainKey` である。通常の
`key_held_action` は Ctrl / Shift / Alt の同時押下を拒否するため、判定順を入れ替えるだけでは
Ctrl+Space 時に Space を優先できない。

汎用 KeyHold の完全一致規則は緩めない。`Keymap` に crop の開始分類からだけ使う限定 API を
追加し、次をすべて満たすときに限り `CropSpacePan` の物理 key level を認める。

- 現 viewport の `FocusedKeyStatePermit` があり、modal / TextInput / IME owner が拒否していない。
- `CropSpacePan` の実効 KeyHold binding と `CropForceCreateHold` の実効 ModifierHold binding
  を使う。`Space` や `Ctrl` を caller に直書きしない。
- 押されている修飾 group は force-create の実効 binding と一致し、無関係な Ctrl / Shift /
  Alt group は押されていない。右側限定 binding は既存の物理キー判定を使う。
- どちらかの Action が無効化されていれば、この組合せ判定は成立しない。

これにより、既定の Ctrl+Space と、たとえば force=Shift / pan=P のカスタム設定では pan が
優先される。一方、Alt を余分に加えた組合せや、他 context の KeyHold は新たに許可しない。

## 5. KeyAction と UI の統合

`CropForceCreateHold` は次の定義を一つの Action として追加する。

- context: `KeyContext::Crop`
- trigger / policy: `ModifierHold / SingleModifier`
- default: `Ctrl`
- description: 「押している間、ドラッグで新しい切り取り枠を作る」

`KeyAction` enum、`ALL_ACTIONS`、`ini_name`、description、context、trigger、
`default_chords` を同時に更新する。`KeyAction::all()` を使う設定画面と crop の
`CommandScope` を使うショートカットヘルプには同じ Action が自動で現れる。
`docs/keymap.ini.default` は既存生成器から更新する。`docs/keymap-spec.md`、
`docs/key-customization-impl-plan.md`、`htdocs/mimageviewer/manual/tut-crop.html` も、既定 Ctrl、
Space 優先、開始後ラッチを同じ表現にする。

## 6. 維持する挙動

- 通常の Body 移動、8 ハンドル resize、枠外からの従来 create、アスペクト固定、4 px開始閾値。
- Space pan、wheel / Ctrl+wheel zoom、panel scroll、auto crop、数値入力、Ctrl+E、Esc。
- release 時の DB / sidecar 保存、元画像非変更、crop preview / cache の既存 invalidation。
- Single への spread pivot と復帰、画像 / ZIP / PDF の同じ display transform。
- dialog / TextInput / IME と pointer panel gate、linked / independent / parked viewer の既存権限。

## 7. 実装対象

製品コードの予定変更は次に限定する。

- `src/ui_crop.rs`: 開始 classifier、force-create Action の読取、既存 owner への格納、focused tests。
- `src/keymap.rs`: Action 定義と、force modifier との組合せ時だけ KeyHold の物理 level を解く
  限定 API・keymap tests。
- `docs/keymap.ini.default`: 生成物。
- `src/ui_dialogs/context_shortcuts.rs`: dynamic crop row が実効 binding / disabled に追従する
  既存経路の回帰が必要な場合だけ test を追加する。通常配線の追加は不要。
- `docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`、
  `htdocs/mimageviewer/manual/tut-crop.html`: 操作説明。

`app.rs`、viewer context registry、永続 crop DB、sidecar、display transform は変更しない。

## 8. 回帰確認

### 純粋な開始分類

- Pan は force / Body / handle / 枠外 target より常に優先する。
- ForceCreate は Body と全 handle を無視して Create になるが、表示画像外では始まらない。
- modifier なしでは Body / handle は Edit、枠外かつ画像内は従来どおり Create。
- 同一フレーム press+release、pointer gate 失効、mode exit で drag owner が残らない。
- create 開始後の modifier release、通常 edit 開始後の modifier press で owner が変わらない。

### Keymap

- `CropForceCreateHold` が Crop / ModifierHold / Ctrl で、ini / settings / GUI rowを往復する。
- force=Ctrl + pan=Space、および force=Shift + pan=P の実効 binding で Pan が優先する。
- disabled action、余分な Alt、focus のない別 viewport、modal / TextInput / IME owner では
  組合せ判定が成立しない。
- 既存の一般 KeyHold は Ctrl / Shift / Alt 同時押下を拒否し続ける。

### 実 pointer 経路

- 未設定の `CropRect::full` 内、保存済み Body 内、各 handle 上から force drag して新しい矩形になる。
- 通常 drag は移動 / resize のままで、Space+force drag は pan になる。
- ZIP / PDF を含む回転・zoom済み transformでも、screen→source変換とaspect固定を既存経路で使う。
- release 後に一度だけ確定し、Esc / close / page switch / detached parkで既存 cleanupが働く。
- crop shortcut help と設定画面は現在の割当・無効化を表示する。

狭域の `ui_crop` / `keymap` / context shortcuts testsから始め、shared keymapを変更するため
`cargo check`、keymap関連回帰、fmt、glyph、必要な全体 gate、`build-dev.ps1 -PreserveRuntime`
を実装完了後に行う。通常プロファイルのアプリはagentから起動・停止しない。

## 9. 実装・検証記録（2026-09-13）

- `CropForceCreateHold` と crop 限定の実効 binding resolverを追加し、通常の KeyHold
  完全一致規則は変更していない。pointer-down は Pan / ForceCreate / Edit / Create の順で
  一度だけ分類し、既存3 ownerへ格納する。
- production pointer handlerを通す egui 回帰で、全画面 crop Body 上の force pressから
  Create ownerを開始し、修飾キーを途中で離してもreleaseまで保持・確定することを確認した。
  Space+forceで始めたPanは、Spaceを途中で離しても同じprimary holdをcropへ再分類しない。
- `ui_crop::tests` 10件、`keymap::tests` 139件、core check、fmt、UI glyph、repository
  viewer-context audit、対象diff-checkはいずれも成功した。
- `RUST_TEST_THREADS=1` / `CARGO_BUILD_JOBS=1` / `-SuppressCrashDialogs` の全体gateは、
  main 8337件成功・失敗0・ignored 45、workspace/integration/docとvendor
  egui 25件・egui-wgpu 9件・eframe 15件を含めてexit 0。process error modeの復旧も確認した。
- `scripts/build-dev.ps1 -PreserveRuntime` はexit 0。確認用coreは
  `target/dev-runtime/mimageviewer-core.exe`、SHA-256は
  `B936D7A58CEF25097708994380A6A32B26D1B46010CB273BA08389C15F4D813F`。
- 完全ログは `target/section164-force-new-crop-frame-20260913/` に保存した。
  Agentは確認用アプリを起動・停止していない。実際のWindows物理Ctrl/右Ctrlとcrop pointer
  操作の目視確認は利用者確認へ残す。
