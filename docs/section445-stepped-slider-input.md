# §1.445 Slider自身の丸めと外部変更の区別

対象はgpuslider-1445、v4.8.2 (da9a64cd5) と前回案4d7b1b0eaからの未commit差分。
検証で製品exeを起動せず、OS入力・通常データprofile・他worktreeを使わない。index.htmlは変更しない。

## 方針変更と原因

前回の確定待ち化は撤回した。独立レビューで、補正の入力が切替後の標準へ書かれるP1、
設定ページ/補正タブ切替で入力が消えるP2、消しゴムの最初の操作が旧値を使うP2が出た。
widget単体の134件成功は、これらの呼出元の退行を検査していなかった。

新方針はegui 0.33.3と同じ即時反映。「2」で値は0へ丸めるが文字列は2を保ち、
続く「5」で文字列25・値25となる。Enterや余分なクリックを必須にしない。
元の原因はSlider::value_uiがDragValueへ渡したローカルf64と、後から丸めて保存した値の差。
EditState.value=2と保存値0の差が次frameで外部変更と判定され、文字列が破棄された。
判定の導入は上流 [#8403](https://github.com/emilk/egui/pull/8403)、
commit d802a982ce959839d92f4f6718fdffa41143110e（0.36.2収録）。
0.33.3はStringだけを保持する。旧版にも途中値の丸めはある。原因の認定は維持する。

## 共通部品の境界

- SliderはDragValue::from_get_setを使い、setter内で既存core.roundedを通して保存する。
  getterは従来のSlider::get_valueと同じcore.existing(get(storage))を返す。
  EditStateが次frameに見える有効値を記録するため、自分のstep・小数丸め・clamp・
  f32変換・custom setterを外部変更と誤認しない。本当の外部変更では文字列を更新する。
- 候補と現在の有効値が同じときはsetterを呼ばない。未操作27をEnter/blurで25へ変えない。
  step付きAlwaysの毎frame処理は前回から引き続き範囲clampだけにし、stepなしの
  既存小数桁丸めは維持する。
- SliderCoreの数値欄設定はget/set版helperで共通化し、従来のf64版はそのwrapperにする。
  RangeSliderの動作と呼出元は変更しない（mIV未使用）。
- DragValueの矢印/AccessKit等の数値操作は、保存直後にgetterを読み返して表示文字列を
  生成する。27→上矢印で保存30・表示32になる不一致を避ける。手入力文字列は保持する。
- 前回のupdate_while_editing(false)強制とEsc終了時の状態掃除を撤去する。
  明示的なupdate_while_editing指定と#8403の比較判定は従来どおり尊重する。
- response.changed()は有効値の変化で判定する。25へ26を入力して25へ丸めるだけならfalse。
  ドラッグ・矢印・アクセシビリティの操作刻みは既存のまま。

追加のpending状態・callback保存・入力終了待ち・アプリ側の処理順変更は導入しない。
get/set所有境界に正規化を移し、丸め前と丸め後の二重の値をなくす。
clamp→step丸めの算法は維持する。端が格子外の0..98/step5でも、次frameのAlways clampを
外部変更と誤認しないことを検査する。stepなし・暗黙整数step・単体DragValueも回帰に含む。

## GPU上限のdraftと実配分

コード監査では、OK前にdraftのGPU percentが実配分へ流れる経路はない。

- preferences.rs::PreferencesState::from_settingsはs.preferences_snapshot()を取得する。
  settings.rsのpreferences_snapshotはSettingsのclone。
- pages.rs::page_gpu_memoryはstate.settings.gpu_memory_percentだけを編集する。
  vram_cap_from_percent(pct)は説明文のMiB計算で、実配分を書き換えない。
- 実配分はapp/vram_accounting.rs:421のvram_pool_budgetが
  self.settings.gpu_memory_percentから生成する。サムネイル保持帯、連続表示、AI先読み、
  page-turn、GPU admissionは共通のlive budgetを読む。
- OKのapply_preferences_dialogがdraftをtakeし、install_preferences_settingsで
  App.settingsへ置換して初めて実配分に反映する。
- Cancel/×の破棄確定はdiscard_preferences_dialogがpref_stateを捨てる。

「2」で0が一時的にdraftへ入ることは許容し、追加の警告・保護動作・保存方式は足さない。
headlessではlive50→draft0/25でもbudget50を維持、Cancel後50、再編集OK後25を検査する。

## 回帰

vendorの実Context/Textイベントで分割25/50/80/100、編集文字列、削除/桁修正、paste、
Enter/blur、0/100・clamp、Escが即時値を巻き戻さないこと、未操作27、矢印/drag、外部変更、
f32/custom setter、changed、stepなし小数丸め、AccessKitを検査する。
代表のcache時間/サイズ、文字縦位置、補正値にも適用する。rg step_by srcの54行は
Slider30行（macro定義含む）とiterator24行。暗黙整数stepも共通の橋渡しを使う。

mIVの追加headlessは実panel/handlerと製品の順序を使う（製品ロジック変更なし）:

1. draw_adjustment_panelで個別明るさ10→20手入力→Enterなし標準。標準値を保持。
2. show_preferences_dialogでcache60→25→直接ツリー切替→戻る→OK。25を保持。
3. handle_erase_paint→draw_erase_panelの順で、最初のpressが新bucket許容値を使う。
4. 同じ順でブラシ半径1→8後の最初のpressが新半径のmaskを作る。
5. GPU draft/OK/Cancelとlive budgetの隔離。

AppTestEnvForTestの一時データを使う。消しゴムは200x200画像/maskと表示transformの最小fixture。
fullscreen全体の描画やGPU overlayは検査対象外。

## 今回の検証台帳

前回の成功は今回の成功に読み替えない。ログはtarget/section1445-live/。
UI snapshotの基準画像は更新しない。

| コマンド/条件 | exit / 結果 |
| --- | --- |
| da9a64cd5のeguiを隔離展開し新stepped_slider_accepts_digits_separatelyを実行 | 101、次frameで文字2を期待したが0を表示。v482-text-before.log |
| HEAD4d7b1b0eaの確定待ちvendorで下記nextest 5件 | 100、5件とも意図した値/paint結果で失敗。deferred-nextest.log |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib` | 0、140成功（新方針のwidget回帰23件を含む）。vendor-final.log |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。check-final.log |
| `cargo test -p mimageviewer --test ui_snapshot` | 0、195成功、基準画像の変更なし。snapshot-final.log |
| `cargo nextest run -p mimageviewer --lib -E 'test(section1445_)' --no-tests fail --no-fail-fast --test-threads 2` | 0、実panel/handler追加5件成功。live-nextest.log |
| `cargo nextest run -p mimageviewer --lib -E 'test(ui_adjustment_panel::) & test(adjust_scope)' --no-tests fail --no-fail-fast --test-threads 2` | 0、関連する既存8件成功。scope-nextest.log |
| `cargo fmt --check` | 0 |
| `rustfmt --check --edition 2024 vendor/egui/src/widgets/slider.rs vendor/egui/src/widgets/slider_core.rs vendor/egui/src/widgets/drag_value.rs vendor/egui/src/widgets/slider_tests.rs` | 0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 0、core/Remote/EPUB worker生成、DLL配置・PE検査(runtime=4、pe=3)成功。build-dev.log。製品起動なし |

旧版比較はGit archiveのruntime eguiソースへtest moduleだけ追加した。
隔離Cargo.tomlでは存在しないunused patch参照のみ除去。実装側sourceには変更を加えていない。
実IME・製品exeでの操作・初発製品版の実行比較は未実施。

旧案の比較時だけvendor製品3ファイルをHEADへ戻し、実行終了後に新方式をbyte backupから
復元した。fixtureの型・numeric editor特定の問題は準備段階で修正し、意図した失敗と区別した。
旧案では標準brightnessが3から20になる、cacheが25でなく60、GPU draftが0でなく50、
最初のbucket/brush maskが旧設定の結果になることを確認。新方式では同じ5件すべて成功。
独立gpt-6.1-sol/xhighのコード・設計レビューはACCEPT、追加P1/P2なし。
最終検証中は製品コードを変更していない。mIV側3ファイルの追加はtest-only。
check/nextest/snapshot/buildにはprocess内のCARGO_BUILD_JOBS=4、MSBUILDDISABLENODEREUSE=1を使用。
本体lib全件は前回の担当分担どおりClaudeCode側の後続検証で、今回の13件をfull成功とは扱わない。

## 確認用バイナリ

normal feature setでtarget/dev-runtime/mimageviewer-core.exeと同じディレクトリの
Remote/EPUB workerを用意した。起動は利用者が行う。
インストール版・トレイ常駐版を先に終了する（single-instance mutexを共有）。
通常の%APPDATA%\mimageviewerを使い、実設定・データを更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

代表の確認はGPU上限の「2」「5」分割入力と、補正の「このページ→標準」、設定ページ切替、
数値入力直後の最初の塗り。GPU上限の途中値0はdraftだけに入り、実配分はOK保存後に変わる。
