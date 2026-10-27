# egui 0.37 段7: clean install 起動時 atlas panic の補修

対象は `egui-036` / `7946400b2` 後の未コミット差分。2026-10-17。
段5・段6の成功記録は変更せず、段7で見つかった P1 の原因と追加検証を本台帳に分ける。
以下の実行は**実装担当の内部確認**。製品起動・live・`test-full.ps1` は実行しない。

## 観測と原因

設計担当の clean install 起動は約1.4秒で `egui_texid_Managed(0)` の
`Device::create_texture` / `Dimension X is zero` により終了した。
`target/dev-runtime/data/logs/panic.log` と `mimageviewer.log` の stack / atlas-probe を照合した。

原因は初期 font atlas 予約の早期配送である。

1. `WrappedTextureManager::default` は本家0.37と同じく Managed(0) を0×0で予約する。
   `TextureManager::alloc` によって full set が共有 manager の既存 pending に入る。
2. 通常の `ContextImpl::end_pass` は `fonts.font_image_delta()` を同じIDへ `set` してから
   `take_delta` する。最初の実 atlas は非ゼロの whole update なので、
   `TexturesDelta::push` が初期予約を置換する。空の UI でも白画素を持つ atlas が公開される。
3. 段3で追加した `take_viewport_retirement_texture_delta` は、名前によらず共有 manager 全体を
   無条件に drain していた。両 backend の WindowEvent 後の close dispatcher は、
   退役要求がない場合も finalizer を呼ぶため、最初の Full 前にもこの drain に到達した。
4. renderer は既に native window に対して準備済みだったので、初期予約が実 GPU へ届いた。
   7cb0494f9 の「非空 batch だけ配送」は空 batch の余分な処理を防ぐが、0×0 full は非空なので防がない。

本家原本はローカル registry の `egui-0.37.0` / `epaint-0.37.0` /
`egui-wgpu-0.37.0` / `eframe-0.37.0` / `egui_glow-0.37.0` の source と照合した。
本家の WGPU renderer は画像寸法をそのまま `device.create_texture` に渡し、0×0を捨てる契約はない。
本家 eframe は通常の FullOutput で上記初期化後の delta を受け、今回の pass 外退役 collector を持たない。
glow の texture upload にもこの予約を除く特例はない。WGPU と同じ validation panic の証明とは区別する。

## 修正と所有境界

製品変更は `vendor/egui/src/context.rs` の退役 delta 取得境界だけ。
既存 TextureManager の Managed(0) metadata の両寸法が正になったことを、同じ write lock 内で確認する。
初回 atlas 公開前は `take_delta` 自体を呼ばず、**全 pending を元の manager に保持する**。
最初の通常 end_pass が予約を実 atlas に置換し、user texture の set / partial / free も一緒に出力する。
公開後は既存の非描画 collector / renderer owner / free-only 配送を維持する。

0×0を除去・clear・filter せず、renderer の fallback、別 pending、追加 bool、repaint、UI 強制を作らない。
`fonts.is_some()` は begin_pass 時点で成立し、初回公開そのものを示さないため使わない。
font reload は公開済み metadata を0×0へ戻さず、再生成した未公開 Fonts があっても非描画 free を止めない。
§4.9 第4.6版の「renderer 未準備の delta は既存 owner に残す」契約に、共有 atlas の公開準備を補った。

実装前の独立 source 確認（GPT-6.1 Sol / xhigh）は、この所有境界の補修を構造的修正として ACCEPT。
完成差分も独立 source 確認し、reload fixture が同じ定義を再指定する no-op だった指摘を補修後 ACCEPT。
この独立確認は source の確認であり、下のコマンド実行者とは別である。

## なぜ既存回帰が見逃したか

- eframe WGPU の idle retirement 回帰は、最初に `run_ui` で atlas を初期化してから実 device へ配送していた。
  準備済み renderer と準備済み atlas を同一視し、clean install の初回 WindowEvent を検査していなかった。
- glow の同回帰は、初期0×0を fake GL に渡した後で呼出記録を消していた。
  fake が寸法を検証しないため不正な早期公開を成功扱いしていた。
- 全件 gate はこれらの fixture を実行して成功したが、この起動前提は含まれていなかった。

今回、WGPU 回帰を実際の `dispatch_child_close_requests` から resource finalizer / 実 Renderer / 実 queue
へ通す形に改め、Full 前に32回 dispatcher を実行する。App の UI は呼ばれたら失敗する。
native window inventory だけが fake であり、実 WindowEvent/OS window を作る試験ではない。
別の egui-wgpu 回帰では、private test module の既存 offscreen factory で実 device を持つ Painter を用意し、
製品の `apply_textures_delta` → `TextureDelivery` → Renderer をそのまま実行する。
両回帰とも adapter 不成立を成功・skip にしない。

## 回帰と同型経路

| 対象 | 回帰 / 根拠 | 検査内容 |
| --- | --- | --- |
| WindowEvent 完結の dispatcher | `retirement_texture_delivery_wgpu_startup_before_full_and_free_only`（1件） | Full前32回、実 Renderer の validation、consumer非進入、初回user set保持、公開後idle/free-only一度配送 |
| 実 Painter / 実 device | `retirement_texture_delivery_offscreen_startup_preserves_first_full_atlas_and_user_texture`（1件・内部3シナリオ） | pristine Context、初回 empty / discard / immediate、実atlasの正寸法、user texture画素readback、後発freeとfont存続 |
| manager 所有と reload | `viewport_retirement_before_first_full_preserves_unpublished_sets_partials_and_frees`（1件） | LogicOnly反復、Full前whole→partialとdrop/free保持、非ゼロatlasへの置換、実際に変更したfont定義、free-only、後続Fullの二重配送なし |
| glow | `retirement_texture_delivery_glow_skips_empty_context_and_delivers_free_only`（1件） | Full前resource context選択0回、初回空Full後に非ゼロatlas受領、idle/free-only。GL呼出のみfake、実GLの証明ではない |
| discard 再pass直前 | 上記実Painterのdiscard + 既存 `discard` filters | scopeのsinkは end_pass / output hook完了後だけ。初回0×0予約は既に実atlasに置換済み |
| root / immediate の pending | 上記実Painterのempty / immediate + 既存 `minimized_full` / retirement filters | Full由来のdeltaを既存共有pendingへ渡す順序を維持。immediateの最初のend_passも実atlasを先に公開 |
| stale native replacement の raw drain | `replace_stale_wgpu_viewport` / `replace_stale_glow_viewport` source監査 + 既存discard/reopen回帰 | 旧native登録が必要。通常生成順では先行Fullが既にatlasを公開。paused parent内のdeltaは既存pendingへ移し、後続出力と合流してから消費 |
| 初回Full前の退役 / root終了 | source監査（実行済み実機と区別） | cleanupはcollectorより先なので遮断しない。未公開deltaはmanagerに残す。FullなくContext終了なら未GPU配送の既存 `TextureManager::Drop` が明示clear。実native child生成にはFull出力が先行する |

修正前の新回帰は、Painter（`before-fix-painter.log`）・dispatcher（`before-fix-dispatcher-compiled.log`）とも
選択1 / 失敗1 / exit101。両方が wgpu error scope で、実機と同じ `Managed(0)` の
`InvalidDimension(Zero(X))` を検出した。単なる compile failure / adapter 不成立ではない。
最初の dispatcher 試行だけは import 不足で compile error（別ログ）だったため再現証拠に数えない。
新 Context fixture の初回試行では `TextureManager::free` が set を相殺するという誤った前提を補修し、
reload の成立確認も実際の即時適用を検査する形へ補修した。製品の期待値変更ではない。

## コマンド結果

ログと JSON は `target/egui037-stage7-startup-atlas/`。
全 Cargo は offline、`CARGO_BUILD_JOBS=1`、`MSBUILDDISABLENODEREUSE=1`、workspace の target を使用。
UPDATE_SNAPSHOTS は未設定。下の件数は実選択数であり、0件を成功としない。filterには重複がある。

| コマンド対象（全て `cargo test --lib`） | filter | 選択 / 成功 / skip | exit |
| --- | --- | --- | --- |
| `--manifest-path vendor/egui/Cargo.toml` | retirement | 15 / 15 / 0 | 0 |
| 同上 | discard | 8 / 8 / 0 | 0 |
| `--manifest-path vendor/egui-wgpu/Cargo.toml --features winit` | retirement | 1 / 1 / 0 | 0 |
| 同上 | discard | 2 / 2 / 0 | 0 |
| `--manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu` | retirement | 13 / 13 / 0 | 0 |
| 同上 | discard | 5 / 5 / 0 | 0 |
| 同上 | minimized_full | 9 / 9 / 0 | 0 |
| 同上 | child_close | 3 / 3 / 0 | 0 |
| `--manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow` | retirement | 14 / 14 / 0 | 0 |
| 同上 | discard | 5 / 5 / 0 | 0 |
| 同上 | minimized_full | 11 / 11 / 0 | 0 |
| 同上 | child_close | 4 / 4 / 0 | 0 |

| 共通確認 | 結果 | exit / 秒 |
| --- | --- | --- |
| `cargo test -p mimageviewer --tests --no-run` | test executable 23本をコンパイル | 0 / 216.18 |
| `cargo fmt --check` | root workspace clean | 0 / 21.52 |
| `rustfmt --edition 2024 --check vendor/egui/src/context.rs vendor/egui-wgpu/src/winit_tests.rs vendor/eframe/src/native/wgpu_integration.rs vendor/eframe/src/native/glow_integration.rs` | workspace除外vendorの変更Rustもclean | 0 / 0.17 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-dev.ps1 -PreserveRuntime` | core / remote / EPUB の確認用build。製品起動・既存process停止なし | 0 / 147.72 |

## 最後の lib 全件（一回）と失敗分類

`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-lib-full.ps1 -Force` は
**一回実行、exit100**。run は `target/test-lib-full/runs/20261017T125323485Z-5772cef4`、
source fingerprint は `b8e5c6c6492e1be2012f4e0979b0565f20467495227bf85ee8685ad36aa4f5e0`。
nextest 実行354.611秒、wrapper全体382.18秒。retry0 / no-fail-fast、既存skip方針のまま。

| 区分 | 件数 |
| --- | ---: |
| 登録総数 | 13,507 |
| 実行・成功 | 13,447 |
| 既存skip | 59 |
| 失敗 / error | 1 / 0 |
| PNG由来失敗 | 0 |
| 今回のatlas修正の失敗 | 0 |

失敗名は `similar_index::tests::startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker`。
`src/similar_index.rs:11083` の、再起動側worker終了後に `scanned_once_matches(&fingerprint)` が
trueであるというassertで失敗した（test自身の実行約0.12秒）。**全件を成功にはしない**。
同じsourceで追加の限定再実行を行い、下記は成功した。全件は再実行していない。

| 限定コマンド | 選択 / 成功 / skip | exit / 秒 |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker -- --nocapture` | 1 / 1 / 0 | 0 / 0.78 |
| `cargo test -p mimageviewer --lib similar_index::tests::startup_ -- --nocapture` | 15 / 15 / 0 | 0 / 0.98 |

判断は**索引側の既存系統の非決定失敗、根本原因は未確定・設計担当の別管理判断待ち**。
今回のatlas修正による機能回帰としては分類しない。根拠は次のとおり。

- 本差分の製品変更はvendor Contextの退役collectorだけで、索引の製品・testは変更していない。
- 該当test本文はローカルmasterと完全一致（SHA-256
  `32e0bb80d5e000b5b9f632425e6e033e7d7d06ac89729fe96ad07530dfa37b15`）。
  `similar_index.rs`全体のmasterとの差も、段5のtelemetry counterの`fetch_update`→`try_update`だけ。
- このfixtureは軽量な`SimilarIndexManager::new`（通知closureはno-op）とDB / workerを使い、
  sourceにegui / eframe / 今回のcollectorへの呼出しがない。UI / native dispatcherを動かす経路ではない。
- 全件時の失敗と、単独・関連15件での成功を分けて記録する。
  負荷・DB・workerのどの前提によるものかは、この限定確認から断定しない。

JUnit 13,507 caseとroot属性、59skip・1failure・0errorを機械照合した。
`target/egui037-stage7-startup-atlas/lib-full-classification.json` が分類結果。
generic wrapperが失敗case内のlibtest stdoutを全件数と誤認するため、`lib-full.json`では
その抽出値を`libtest_failure_excerpt`へ残し、全件数はJUnit由来の値に明示的に分けた。
既存HUD z-order / metadata transferの2件は今回成功しており、別管理方針を変更しない。
索引への修正、ignore、期待値変更、全件の再実行は行わない。

## 最終整合と確認用成果物

`target/egui037-stage7-startup-atlas/finish-audit.json` で、検証前後の追跡Rust / PNG / manifest /
lockの2,056入力が全てhash一致、PNG変更0、staged0、HEAD `7946400b2` 不変を確認した。
全件後は結果の文書追記だけを行い、テスト対象sourceは変えていない。
確認用core SHA-256は `b3d0060b4e1b82b71bed7475ee3ad43c8b32160fa0e463c09cb8f37a074017a2`。

設計担当用の新しい使い捨てdata-dirでの起動例（実装担当は実行しない。指定先は引継ぎ時点で存在しない）:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--data-dir', '.\target\dev-runtime\data-s7-atlas-fix'
```

確認は初回起動の最初のFull前イベントと、その後の通常表示・終了。
`--data-dir`を外すと通常の`%APPDATA%\mimageviewer`を使用し、実際の設定・dataを更新し得る。
通常profileはsingle-instanceを共有するため、確認前にインストール版／常駐tray版を終了する。
今回の既存`target/dev-runtime/data`のpanic / applicationログは保持し、削除・初期化していない。

## 残る確認

本補修の実機起動は設計担当が行う。こちらでは製品・live・通常profileを起動しない。
起動前に renderer が準備済みでも、初回 atlas 公開はまだである、という二つの寿命を分ける修正であり、
既存の z-order / metadata transfer の別件や、採択済みフォント外観・snapshot baseline は変更しない。
lib全件の索引marker失敗1件も、上記の証拠と未確定点を添えて別管理判断へ返す。
段7の修正後実機確認、設計担当の最終受入、必要な全体gateの扱いは設計担当に残す。
