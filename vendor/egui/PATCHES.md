# egui 0.33.3 のローカル変更

別バージョン検索の長押し表示を修正する際、ナビゲーターの入力順序を
egui のクリック判定と対応付けるために追加した、読み取り専用の API である。
この読み取り専用APIはクリックの判定条件、時刻、入力イベントの生成・消費を変更しない。
2026-10-10には、前面popup終了時に未配送のwheel平滑化分を破棄する限定APIを追加した。
以下の元APIの検証・レビュー記録は当時の差分だけを示す。追加APIの境界は末尾に記録する。

## InteractionSnapshot::clicked_primary_release_index（2026-10-13）

音響調整ボタンのnative入力factを、eguiが実際にクリック対象としたwidgetのreleaseへ対応付ける。
同一passにUP A→DOWN Bが入る場合、現在のpress Bや最後のreleaseだけではAのclickを識別できない。
`src/interaction.rs`でclickedを代入する同じ場所から、全primary Released（click=Noneも含む）中の
序数を公開する。clickedの後続secondary/middle/long-touch代入ではNoneへ置換する。
読み取り専用の補助metadataであり、click/dragの条件・widget選択・入力生成/消費は変更しない。
`src/context.rs`のdebug用snapshot分解にも追加fieldを明示する。
callerはROOT raw-input projection・選択widget ID・同frame/passを検査して使う。
keyboard/long-touchは序数を持たず、native releaseを流用しない。
6件のheadless回帰でUP A→DOWN B、非click releaseを含む序数、secondary置換、
後続無効release、frame境界、long-touchを検査する。filterは`clicked_primary_release_index`。
依存更新時はこのmetadataと、既存release順序/scroll終端APIをそれぞれ保持する。

## 出典

- crates.io package: `egui-0.33.3.crate`
- package SHA-256: `6a9b567d356674e9a5121ed3fedfb0a7c31e059fe71f6972b691bcd0bfc284e3`
- upstream repository: `https://github.com/emilk/egui`
- `.cargo_vcs_info.json` の commit: `44cdd653e2317d300fb8a6c9c36b03f23991e803`
- upstream package path: `crates/egui`
- 検証日: 2026-09-08

Cargo registry の archive checksum を導入前の root `Cargo.lock` と照合し、
archive 内の106ファイルと展開済み registry source の全 bytes が一致することを確認した。
registry 管理用の `.cargo-ok` と生成された `target/` はコピーしない。
この文書とライセンス本文の追加を除き、原本からの Rust 差分は
`src/input_state/mod.rs` の API と回帰テストだけである。

## API の境界

`PointerState::released_buttons_with_click_counts()` は、当該 pass で egui が判定した
全 button release を順序通りに返す。

```rust
impl Iterator<Item = (PointerButton, Option<(Pos2, u32)>)> + '_
```

click でない release も `None` として返すため、raw release 列と序数が一致する。
返すのは値の射影であり、内部の mutable state や内部イベント型は公開しない。
呼出側は全 button・全 focus 区間へ注釈してから、viewport、位置、操作 owner、focus を
判定する。この API 自体は、特定 widget が click を所有することを保証しない。

通常の `Response::double_clicked()` は pass 全体の判定を参照するため、同一 pass 内で
focus が切り替わる場合の release を特定する用途には情報が不足する。
アプリで別のクリック時間窓を実装したり、egui の入力処理を再実行したりしないための境界である。

## 統合

- root workspace では本 crate を除外し、`[patch.crates-io]` から同じ path を指定する。
- standalone の `vendor/eframe` と `vendor/egui-wgpu` にも `../egui` の patch を置く。
- 直接依存だけを path 化し、推移的な registry egui と型を分裂させない。
- root と両 standalone の dependency graph / lock を確認し、同じ local egui 一実体へ揃える。
- `scripts/test-full.ps1` は既存の検証に加え、本 crate の無 filter lib tests を実行する。

## ライセンスの保持

package の `MIT OR Apache-2.0` 宣言は変更しない。原本 archive はライセンス本文を
含まないため、既存 `vendor/egui-wgpu` の同名本文を変更せず同梱する。

| コピー元 | コピー元の SHA-256 |
| --- | --- |
| `../egui-wgpu/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../egui-wgpu/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |

MIT の本文・著作権表示は、同じ upstream commit の
[LICENSE-MIT](https://raw.githubusercontent.com/emilk/egui/44cdd653e2317d300fb8a6c9c36b03f23991e803/LICENSE-MIT)
でも確認した。上表の hash は既存 repository 内のコピー元の値であり、remote ファイルの
byte hash としては主張しない。About の既存ライセンス本文表示は維持する。

## 検証記録

- 原本との差分、読み取り専用 API、2件の回帰、manifest / gate 接続を独立 Astra がレビュー済み。
- `cargo test --manifest-path vendor/egui/Cargo.toml --lib`: 25成功、0失敗。
  記録: `target/r4-vendor-egui-tests2.log`。初回 offline 実行は未取得依存で開始できず、製品失敗ではない。
- 追加回帰は、非 click を含む全 button の release 順序と、discard 後の第二 pass への非再生を確認する。
- アプリ側の Flat / Panorama ordered 入力は54件成功、focus snapshot と release 後移動の補強も各1件成功。
- root の `cargo fmt --all -- --check` 成功時、本 crate の変更ファイルの bytes が変わらないことを確認済み。

ライセンス2本文の配置と上表の hash 一致を確認済み。原本106ファイルとの差分は引き続き
`src/input_state/mod.rs` だけで、追加は本書とライセンス2本文（合計109ファイル、生成 target を除く）。
root / eframe / egui-wgpu / egui standalone の cargo tree は、すべて同じ local egui 一実体。
既存3 lock の差分は egui の registry source / checksum 2行の除去だけで、version 更新はない。
記録: `target/r4-cargo-tree-{root-egui,eframe-egui,egui-wgpu-egui,egui-standalone}1.log`。
アプリ全体の最終 gate は target/r4-test-full-final4-j1.log で成功（main lib 7693成功、38ignored）。
portable のビルド・24 runtime files の更新照合も成功した。実機確認は利用者の返答待ちである。
段階結果と残作業は [レビュー修正記録](../../docs/duplicate-detection-review-fixes-20260907.md) を参照。

## InputState::discard_pending_scroll（2026-10-10）

### 必要性と呼出境界

publicなraw_scroll_delta / smooth_scroll_delta / Event::MouseWheelを消費しても、egui内部の
平滑化待ちwheelが次frameに再生成される。前面popupを閉じた後、同じwheelの残りが背面panelへ
配送されることを防ぐため、InputState自身に未配送分の終端APIを設ける。

fullscreen共通HUD境界でframeの前面ownerをキー処理・ナビゲータ入力より前に確定し、
入力を退避して対象regionの描画直前だけ戻す。全前面描画後にpublic delta・wheel eventの残りを
消費し、選択済みtargetが閉じている終了frameだけ本APIを呼ぶ。開いている間の平滑化を毎frame
破棄したり、Appの遅延フラグ・時間窓で遮断したりしない。キー終了でも既存キー転送は維持する。

### 破棄対象と保持対象

`src/input_state/mod.rs`の本APIはprivateな`unprocessed_scroll_delta`と
`unprocessed_scroll_delta_for_zoom`だけをゼロにする。publicなdelta・eventの消費は呼出側の
既存境界が行う。pointer / focus / touch・gestureの状態、次frameに届く新規wheelは変更しない。
内部fieldを公開せず、既存入力ownerが保有する未配送queueの終端だけを提供する。

### 回帰と依存更新時に保持すべき差分

src/ui_fullscreen.rsの実render_fullscreen_viewport回帰で、実HUDから開くfit / overflowの
クリック終了＋wheel、overflowのキー終了＋wheel、後続idle frameの平滑化残り消費、終了後の
新規wheel配送を確認する。固定ナビゲータとの実重なりでは、元raw snapshotを保持し、即時・
予約consumerへの射影だけから前面所有wheelを除くことを確認する。

依存更新時は本APIの宣言と上記2内部queueの破棄、fullscreenの終了時呼出、キー・raw navigator
処理より前のframe所有確定、上記アプリ回帰を保持する。upstreamに同等の終端APIがある場合は
同じ破棄範囲・終了境界・回帰を満たすAPIへ置換する。読み取り専用の
released_buttons_with_click_counts APIと元の順序／focus回帰も別途保持する。
追加APIについて本台帳は独立レビュー完了を主張しない。

2026-10-10の追加実経路2件（specified_scale_overflow_key_close_owns_wheel_and_forwards_key /
specified_scale_overflow_wheel_owns_real_pinned_navigator_overlap）は、修正前にそれぞれ背面offsetの変化、
固定ナビゲータsizeの変化で失敗することを確認した。修正後のspecified 88件とnavigator 58件は成功。
対象APIの実装は本追補では変更せず、終了境界への接続とconsumer用射影を検証した。
