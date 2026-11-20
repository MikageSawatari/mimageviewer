# 環境設定 / 操作カスタマイズのホバー負荷計測 (5ch 報告 805、backlog 番号は未予約)

調査用の計測だけを追加する。描画、入力、保存、repaint の条件は変更しない。
製品実行ファイルは起動しない。ヘッドレスの値は実機の CPU 使用率・GPU 使用率・present FPS ではない。

## 製品側の perf event

`--perf-log` 有効時、開いている各ダイアログの `Window::show` の前後だけを計測し、
`cat="ui", kind="dialog_frame"` を1呼び出しにつき1件記録する。
無効時は `perf::is_enabled()` の判定だけで、時計の取得・文字列整形・イベント生成を省く。

| field | 内容 |
| --- | --- |
| `n` | `App.frame_counter`。`frame/begin` と同じ番号 |
| `seq` | 既存の `input_seq` |
| `dialog` | `preferences` / `operation_customize` |
| `dialog_ms` | `Window::show` 全体の壁時計 ms。ログ構築・出力時間は含めない |
| `page` | 環境設定は `Debug(state.selected)`。操作カスタマイズは設定サブタブ / 各入力の context |
| `tab` | 操作カスタマイズの `Debug(state.operation_tab)`。環境設定では省略 |
| `search_results` | 環境設定の検索結果一覧を表示しているか |

操作カスタマイズの Keyboard / Commands の `page` は、全場所なら `None`、
絞り込み時は `Some(Grid)` など。Settings は `Behavior` など、他タブは対象 context。
環境設定のページ値は右ペインの描画直前に捕捉し、検索結果クリックや Esc が同じフレーム内で
表示状態を変更しても、そのフレームで描いた内容へ対応させる。
操作カスタマイズは描画呼び出し直後の state を記録する。
初期化・描画前の refresh、追加の編集/確認 modal、適用/保存、egui の end-pass、
tessellation、GPU submit / present は `dialog_ms` に含まない。
両ダイアログが開いていれば、それぞれ1件出る。

```powershell
python scripts/analyze_perf.py <perf_events.jsonl> dialog-frames
```

ダイアログ / タブ / ページ / 検索結果表示ごとに集計する。
**同じ `n` の `frame.begin.prev_update_ms` は前フレームの App::update 時間**である。
集計は依頼どおり同じ `n` で照合し、`lagged_prev_update_ms` として表示する。
現在フレームの `dialog_ms` からこの値を引いて残りの時間を求めてはいけない。
FPS はログに残った連続フレームの間隔から算出する。render/present を直接計測した値ではない。
区間数÷区間時間合計の平均FPSに加え、各有効区間の `1 / 間隔秒` の中央値・p90を
`FPS_interval` として表示する。一定時間幅で数えたFPS分布ではなく、フレーム間隔由来の値である。
観測されたページ切替・セッション・欠落をまたぐ間隔を分母に混ぜない。
連続番号の間に起きた未記録の閉じ開きや idle は識別できず、その経過時間は FPS に含まれる。
perf event の時刻 `t` は 1ms 単位なので、`FPS_interval` の中央値・p90 は丸めの影響を強く受ける
(144Hz 相当でも 6ms / 7ms → 166.7 / 142.9 FPS)。精密な FPS 分布としては扱わず、平均 FPS を主に見る。
ページ切替直後のフレームの `lagged_prev_update_ms` は旧ページの update 時間であり、新ページの値として比較しない。

## ヘッドレス計測

テスト: `ui_dialogs::preferences::hover_perf::headless_preferences_hover_measurement`
(`src/ui_dialogs/preferences/hover_perf.rs`, `#[ignore]`)。
App のテスト用構築でも SQLite / Tantivy / notify の実スレッドが動くため、
描画だけを切り分ける代替経路を採用した。`Context` を直接回し、操作カスタマイズは
900×640 の Window 内で実際のタブ・scroll・`draw_operation_customize_page`、
環境設定は 720×520 の Window 内で検索欄・`draw_tree`・既定の General ページを描く。
eframe / wgpu の実行環境や GPU device、native event loop、App は作らない。
ページの描画関数は製品と共通、Window shell だけテストに複製している。
メイン窓の背面一覧・ツールバー・画像や App の refresh / polling は含まない。

- 画面は1280×900 logical points、egui既定の theme/style、製品の既定日本語フォント設定。
- 各ケース40フレーム warm-up、240フレーム移動、30フレーム静止。
- K は Keyboard タブ、場所フィルタなし、F13～F24 の行を水平に2 points/frameで移動。
- P は折りたたんだ左ツリーのカテゴリを縦に2 points/frameで往復。General ページを維持。
- B は同じ環境設定を描き、何もない窓外でポインタを動かす。
- K の20 overrides は先頭の有効なユーザー操作20件へその既定 chord を明示指定する。
  実効 chord を維持しながら、override 解析・lookup・customized による競合警告を通す。
  任意の利用者設定・多数の新しい衝突を網羅する条件ではない。
- 描画済みテキストの矩形と実 widget の interactive rect を照合する。キー/項目間の
  layout gap は別に記録し、4項目以上・120フレーム以上の実 widget 上の移動を assert する。
- クリックやキー入力は送らず、設定・選択・展開・編集ダイアログが変化しないことと
  使い捨て data-dir に settings.db を保存しないことを assert する。

`RawInput.time` だけを1/60秒ずつ進め、sleep しない。
入力・repaint の時間は合成60Hzであり、実機の描画周波数や CPU 使用率ではない。
このツリーの egui 0.37 では旧 `Context::run` 相当の Context callback API が `run_pass` なので、
`run_ms` は `ctx.run_pass` 全体の壁時計を測る。GPUへ渡さないtexture deltaは計測区間外で明示的に破棄する。
tessellation はその外で測る。中央値・p90はソート後の線形補間（rank = (n−1)×q）で求める。
補助の `tessellate_ms` は shapes の clone を含めず、`ctx.tessellate` だけを測る。
`moving_wall_ms` は JSON / assert / tessellation も含むループ全体なので、これから FPS を求めない。
`prev_pass_repaint_causes` は egui が返す**前 pass**の原因一覧で、
同じ行の現在フレーム `repaint_delay_ms` と1 passずれる。複数passがあればframeとpassの単位も異なる。
`null` の delay は `Duration::MAX`（再描画期限なし）。
停止後も30回は delay にかかわらず強制的に `ctx.run_pass` を呼ぶ。
持続フレーム/合成msはこの強制サンプリング内の観測であり、native の起床時間の再現ではない。

K の内訳は `pages::hover_measurement::breakdown` のテスト専用ラッパーで、
同じ4関数を各フレーム1回ずつ呼ぶ独立した warmed Context / layout で測る。
全体・内訳とも、測定対象フレームは egui が1 passで完了したことを assert する。
製品描画中の計測点ではなく、全体 `run_ms` に加算できる分解値でもない。
`from_settings` の値には内部の最初の `binding_conflicts` が含まれる。
`effective_chords` はソースの呼び出し経路から計算する。空の Vec はヒープ確保しないため、
この呼び出し数をそのまま確保回数とは呼ばない。

実行は必ず指定 runner を通す（ロック領域が書けない場合は runner の警告付き低優先度経路）。
任意の JSON 保存先は repository 内の存在する親フォルダの下、未作成のファイル名を指定する。

```powershell
$hoverCommand = @'
$env:MSBUILDDISABLENODEREUSE = '1'
$env:TEMP = 'G:\home\mimageviewer-hover805\target\hover-tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
$env:MIV_HOVER_PERF_JSON = 'target/hover-805.json'
cargo test -j 1 -p mimageviewer --lib --profile dev-runtime headless_preferences_hover_measurement -- --ignored --nocapture --test-threads=1 > target/hover-805-test.log 2>&1
'@
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner codex-hover805 -WorkDir G:\home\mimageviewer-hover805 -Command $hoverCommand
```

標準出力にも `HOVER_PERF {json}` をケースごとに出す。JSON保存は任意で、環境変数を省いても測定できる。
同名artifactは上書きしないため、再測定時には新しい名前にする。
初回 native build の MSBuild 並列実行で失敗したため、この測定では Cargo の並列数を1に限定する。
runner 自体は常に通し、ロック制限を迂回しない。一時領域・依存物・成果物はこのツリー内に置く。
新規 worktree の test exe が `0xc0000135` でテスト0件のまま終了した場合は、
`.\scripts\setup-test-dlls.ps1 -CargoProfile dev-runtime` で `target/dev-runtime/` へ置いてから同じ runner で実行する
(Cargo 1.100 から `deps/` に置いても効かない)。

## 実行結果

2026-11-18、Windows x86_64、rustc 1.99.1、`dev-runtime` (opt-level 2 / LTO off)、
製品と同じ mimalloc で実行。runner はロック/TEMP領域への書込み不可を警告し、
BelowNormalで実行した。子コマンドで TEMP/TMP をこのツリーの `target/hover-tmp` に指定し、
`-j 1` でビルドした。他プロセスの負荷は固定していない。1回の測定であり、最大値には
スケジューラ等による待ちも含み得る。CPU thread cycles や CPU 使用率は測っていない。

成功ログ: `target/hover-805-test6.log`。
`1 passed; 0 failed; 18620 filtered out; finished in 13.93s`、runner exit 0。
生データ: `target/hover-805.json`（4ケースの全1,080フレームと2ケースの単体内訳）。
4ケースの測定/停止フレームは全て1 pass。Kは213/240フレームで実キー上、10キーを通過。
Pは223/240フレームで実項目上、8項目を通過した。残りはレイアウト上の隙間である。
全ケースで設定・選択・展開・編集状態の不変条件と settings.db 未保存の検査が成功した。

時間は **中央値 / p90 / 最大**、単位 ms/frame。

| ケース | Context全体 `run_ms` | `tessellate_ms` |
| --- | ---: | ---: |
| K 既定（overridesなし） | 6.947 / 7.845 / 9.001 | 0.197 / 0.231 / 0.322 |
| K 20 overrides | 15.672 / 18.551 / 22.244 | 0.174 / 0.222 / 0.791 |
| P General / 左ツリー移動 | 0.091 / 0.137 / 0.609 | 0.025 / 0.031 / 0.168 |
| B General / 窓外移動 | 0.083 / 0.130 / 0.227 | 0.024 / 0.032 / 0.107 |

下記の描画数は**中央値・p90・最大が一致したため**、各値をまとめて表示する。
Pの頂点/indicesは先頭7フレームで2,608 / 8,136と少なく、全フレームが一定という意味ではない。
GPU callback はない。

| ケース | shapes | ClippedPrimitive | 頂点 | indices |
| --- | ---: | ---: | ---: | ---: |
| K 既定 | 551 | 7 | 10,506 | 39,693 |
| K 20 overrides | 488 | 7 | 8,022 | 28,647 |
| P | 80 | 7 | 2,680 | 8,454 |
| B | 80 | 7 | 2,536 | 7,704 |

20 overrides は実効chordを既定と同じにしても customized として競合警告が有効になる。
このfixtureでは警告部分が増え、F13の描画矩形はY=468からY=610へ下がった。
K20の頂点/indices減少はこの可視範囲の違いも含むので、同一内容のGPU処理が軽くなったとは解釈しない。
PとBも完全に同じmeshではなく、ホバー表示分の頂点/indicesの差がある。

Kの単体内訳（全体フレーム内に埋め込んだ計測ではなく、別Context / layout）。

| 関数 | K 既定 | K 20 overrides |
| --- | ---: | ---: |
| `Keymap::from_settings`（内部の競合検出込み） | 0.289 / 0.351 / 0.620 | 0.285 / 0.404 / 0.730 |
| `binding_conflicts`（ページが呼ぶ2回目） | 0.278 / 0.435 / 0.802 | 0.223 / 0.433 / 0.725 |
| `keyboard_chord_picker` | 1.554 / 1.970 / 2.397 | 1.821 / 2.543 / 3.405 |
| `command_list` | 5.441 / 6.807 / 8.064 | 12.635 / 15.608 / 19.415 |

全体の中央値はK20がK既定の約2.26倍。内訳では一覧が最大で、一覧の中央値は約2.32倍だった。
`from_settings` と2回目の競合検出には、このfixtureで同程度の増加は見られなかった。
各段の中央値の和を全体フレームの中央値と比較することはできない。

呼出し数の計算は両Kで同じ：102キー × 583ユーザー操作 = **59,466回**の eager tooltip scan。
これに587操作の競合走査を2回、一覧の583回を加え、
`effective_chords` は **61,223回 / ページ描画pass**。
tooltip内で非空のVecを返すものは294操作×102キー = **29,988回**。
残り29,478回は空のVecを返す。これは呼出し経路の計算であって、実際のallocator呼出し数の測定ではない。

追加のソース上の候補として、一覧の状態欄は各583行で `Settings.keymap.override_chord_labels` を呼ぶ。
これは overrides を走査し、各bindingのaction名を `KeyAction::parse_ini_name` で再解析する。
`parse_ini_name` 自体も全actionから線形検索する。
この20件・583行では status 判定だけで **11,470回**の名前解析が発生する
（1+…+20 + 563×20）。一覧の増加に関わる候補だが、
このhelperを単独計測していないため、約7.2msの増加全ての原因と断定しない。

## 再描画要求の観測

動かしている240フレームの `ROOT.repaint_delay == 0` は **全ケース100%**。
窓外Bでも同じなので、この割合だけではツリーやtooltip固有の要求を区別できない。
入力由来の要求は `context.rs:632`（`InputState::wants_repaint_after` の反映）として確認した。
前passの原因一覧では、Kに `tooltip.rs:424`（動いている間の即時要求）が237回、
`tooltip.rs:436`（表示待ちの遅延要求）が2回あり、P/Bにはtooltip由来の記録がなかった。
これは前passの一覧であり、現在行と原因を同一フレームとして直接対応させない。

停止後の時刻は、最後の移動フレームからの**合成60Hz時間**。

| ケース | 即時要求 | その後の要求 | 30フレーム観測の終端 |
| --- | --- | --- | --- |
| K 既定 / K20 | 1～6フレーム目（16.7～100ms）が0 | 7～30フレーム目が正の有限delay。466.667→83.333ms | 500ms時点でも遅延要求あり。全要求の終了は観測窓外（少なくとも30フレーム持続） |
| P / B | 1フレーム目（16.7ms）だけ0 | 2～30フレーム目は全て `Duration::MAX` | 33.3ms時点から期限なし。後から0へ戻るburstなし |

Kの停止中の前pass原因は `tooltip.rs:424` が6回、`:436` が24回。
全30フレームのdelayと原因は生JSONに残している。
実機で500msの連続描画が発生した、という観測ではない。遅延要求はその期限での再描画予約であり、
その間の各refreshでの描画を要求するものではない。

## 数値から言えること・観測限界

- この条件ではKはPより大幅にCPU壁時計の費用が高い。単体では一覧の費用が最大だった。
  tooltipのeager走査と競合検出2回はソースと呼出し数で確認できた。
- 20 overridesで増えたのは主に一覧の計測値で、競合検出が主因という数値にはなっていない。
  名前の再解析という追加候補があるが、その寄与率は未測定。
- 「1フレーム約6万回のVec確保」は未確認。約6万回の呼出しはあるが、tooltip分だけでも
  約半分は空Vecであり、全てをヒープ確保と数えるのは不正確。
- Pが軽いという仮説の前提は支持される。移動中の即時要求はBでも100%で、
  この割合がツリー特有という結果ではなかった。
- 本ハーネスは描画要求に従うnative schedulerもrefresh rateも再現せず、
  全シーンを合成60Hzで強制サンプリングする。**実FPSやGPU使用率は計測していない**。
  CPU費用の差がフレーム頻度の差になり、GPU使用率の分かれ方を生む、という因果関係は未確認。
- shapes / primitives / mesh数はGPU負荷の代理指標。背面のメイン画面、面積・overdraw、
  texture upload、ドライバ・present等を含まない。利用者報告の実機原因とは断定しない。

## 検証と引き渡し

- `cargo fmt` を実行、最終の `cargo fmt --check` / `git diff --check` は成功。
- `python scripts/test_analyze_perf.py`：最終の既存込み113件成功（`DialogFrameReportTests` の7件を含む）。
- `python scripts/check_ui_glyphs.py`：危険グリフ0件。
- 上記の最適化headlessテスト1件成功。独立レビューは生データから統計を再計算し、全ケースの値・停止履歴・pass数を照合。
- 初回のnative CMake並列buildはexit 101・テスト0件。生成済みCMakeを同じrunner経由で
  `--parallel 1 -- /nr:false` でbuildすると成功した。以降はCargoも `-j 1`。
  テストAPIのコンパイルエラー（E0599/E0308）を修正し、製品計装のコンパイルエラーはなかった。
- 最適化コンパイル成功後の `0xc0000135` はFFmpeg DLL未配置による起動前失敗。
  指定どおり6本をdepsへ配置し、同じソース/実行体で成功した。これらを製品テストの失敗とは数えない。
- 製品バイナリ・live UIは起動していない。挙動変更を含まない調査用の計装であり、
  確認用製品バイナリのbuildや全workspace gateは実施していない。コミットもしていない。

## §1.808 修正後

2026-11-18 の利用者指定の5件だけを実装する。行の仮想化、chord→actions 索引、
repaint の間引きは加えず、表示・入力・保存の仕様と UI 文言を維持する。

- 状態欄の「上書き」は、生の `KeymapSettings.overrides` の action 名を一覧の先頭で
  一度だけ解析した集合で判定する。chord が不正でも、空配列による解除でも対象とする。
  解析済み `Keymap.overrides` の有無とは区別する。
- キーボード図の tooltip は、AccessKit node があるキーでは従来どおり先に本文を作って
  `on_hover_text` へ渡す。それ以外は表示時の `on_hover_ui` 内で本文を作り、従来と同じ
  `tooltip_width` と `Label` を使う。vendor/egui は変更しない。
- 一覧ですでに全行の最大幅を計測した列は、各行で同じ文字幅を再計測しない。
  ほかの `assignment_values` 呼び出し元は従来の計測を維持する。
- 競合報告は通常・予約キーとも customized binding が前提なので、解析済み overrides が
  空なら実効 binding の生成を省く。短絡しない競合走査も既定設定で空になることを検査する。
- `PreferencesState` が設定値の複製と Keymap・競合一覧を一組として保持する。
  読み出すたびに `KeymapSettings` 全体を `PartialEq` で比較し、異なるときだけ作り直す。
  編集・既定復帰・取り込みの各経路に無効化処理は足さない。描画中の借用は `Arc` で保持する。

依存と寿命をソースで確認した。`Keymap::from_settings` は渡された `KeymapSettings` と
コード内の action / chord / scope / 予約キー定義だけを使い、他の設定・ファイル・実行時の
グローバル状態を読まない。キャッシュキーには移行用フィールドも含めた設定値全体を使う。
操作カスタマイズを破棄すると `operation_customize_state = None`、適用時には `take()` した
`PreferencesState` を消費する。環境設定側も破棄・適用時に編集 state を消費するため、
ダイアログを閉じた後にこのキャッシュだけを残す所有者や別途の解放処理はない。

同じファイルの `page_command_overview` は別の Commands タブなので変更しない。
キーボードページから開く割り当て編集ダイアログの毎フレームの読み出しは同じキャッシュを使う。
単一選択の編集状態表示や、選択変更時に一度だけ動く `ensure_command_editor_loaded` の
`override_chord_labels` はそのまま残す。ページ冒頭で確保した Keymap の snapshot と、
ページ内の「すべて既定に戻す」が同じフレームに起きる順序も従来どおりにする。
`preview_command_editor_conflicts` は編集中の候補を別の設定値として構築する処理なので、
表示中の設定用キャッシュを使わない。App への適用・取り込み時の構築もイベント単位のため維持する。

### 追加した回帰テスト

`ui_dialogs::preferences::pages::tests`:

- `hover808_raw_override_status_matches_original_helper`:
  overrides なし・有効 chord・空 chords・不正 chord・未知 action に加え、大文字小文字・空白・重複の判定を旧 helper と照合。
- `hover808_cache_reuses_unchanged_and_tracks_assignment_reset_and_direct_edits`:
  未変更時の同一 Arc、適用・個別解除・直接変更・全件既定復帰・移行用フィールド変更後の
  新規構築との一致、古い snapshot の不変、state 破棄時の解放を検査。
- `hover808_page_render_refreshes_cache_on_next_frame`:
  実際の `page_command_settings` を描画し、適用・全件既定復帰・直接変更の次フレームで
  表示用キャッシュが最新設定と一致することと、未変更フレームの再利用を検査。
- `hover808_keyboard_tooltip_is_lazy_without_hover_and_accesskit_keeps_descriptions`:
  AccessKit 無効・非ホバー時の本文構築0回と、有効時の全102キーの説明登録を検査。
- `hover808_hovered_lazy_tooltip_matches_eager_text`:
  AccessKit 有効・無効の双方で tooltip の描画本文と構築回数を検査。

`keymap::tests::binding_conflicts_default_scan_is_empty_without_fast_path`:
既定 keymap の短絡あり・なし双方の空結果と、不正な raw chord の警告保持を検査。

### 最終の自動検証結果

指定の `run-heavy.ps1`、Owner `codex-fix808` を使用。子コマンドで TEMP/TMP を
このツリーの `target\hover-tmp`、`MSBUILDDISABLENODEREUSE=1` に指定した。
`dev-runtime`、テストは1 threadで、次の focused filter が全て成功した。
既存 keymap テストには overrides ありの通常・予約キー競合検査も含む。

| filter | Cargo jobs | 結果 | ログ |
| --- | ---: | ---: | --- |
| `keymap::tests::` | 4 | 196 passed / 0 failed | `target/hover-808-keymap.log` |
| `ui_dialogs::preferences::pages::tests::` | 1 | 32 passed / 0 failed | `target/hover-808-preferences-pages.log` |

計228種類成功、両 runner exit 0。上記には追加6件と、既存の dark snapshot、
列幅・折り返し・DPI・フォントのレイアウト検査を含む。
ページ全体の既存回帰を確認する前に、`hover808_` の5件と `command_chords_` の2件も
Cargo `-j 4` で成功した（`target/hover-808-focused3.log` / `target/hover-808-layout.log`）。
`cargo fmt`（引数なし）、`cargo fmt --check`、`git diff --check`、
`python scripts/check_ui_glyphs.py` も成功（危険グリフ0件）。
最初の compile は追加テストの旧 egui API 名を検出し、`global_style_mut` へ修正した。
製品コードの compile error や最終テストの失敗はない。

### 同条件の前後比較

正式な修正後データ: `target/hover-808-after-final.json`。
成功ログ: `target/hover-808-after-final-test.log`。
前掲と同じヘッドレステスト、`dev-runtime`、Cargo `-j 1`、1 test thread、
画面・フォント・fixture・warm-up 40 / 移動240 / 停止30フレームで再計測した。
テストは `1 passed; 0 failed; 18626 filtered out; finished in 5.94s`、runner exit 0。
runner は前回と同じロック/TEMP 領域の制限を警告して BelowNormal で実行し、
子コマンド内で TEMP/TMP を上記のツリー内パスへ指定し直した。

初回にも計測本体は成功して `target/hover-808-after.json` を保存したが、
依頼側が子 scriptblock 末尾に付けた `exit` により runner の結果代入を飛ばし、
runner の終了表示が初期値 `exit=1` のままになった。その `exit` を除き、
上書きせず別名へ再計測したのが上記の正式データである。以降の表は正式データを使う。

時間は **中央値 / p90 / 最大**、単位 ms/frame。修正前は `target/hover-805.json`。

| ケース | `run_ms` 修正前 | `run_ms` 修正後 |
| --- | ---: | ---: |
| K 既定 | 6.947 / 7.845 / 9.001 | 4.382 / 4.711 / 5.490 |
| K 20 overrides | 15.672 / 18.551 / 22.244 | 4.327 / 4.707 / 6.674 |
| P General / 左ツリー移動 | 0.091 / 0.137 / 0.609 | 0.082 / 0.096 / 0.203 |
| B General / 窓外移動 | 0.083 / 0.130 / 0.227 | 0.074 / 0.109 / 0.237 |

| ケース | `tessellate_ms` 修正前 | `tessellate_ms` 修正後 |
| --- | ---: | ---: |
| K 既定 | 0.197 / 0.231 / 0.322 | 0.150 / 0.167 / 0.306 |
| K 20 overrides | 0.174 / 0.222 / 0.791 | 0.124 / 0.147 / 0.189 |
| P | 0.025 / 0.031 / 0.168 | 0.023 / 0.027 / 0.102 |
| B | 0.024 / 0.032 / 0.107 | 0.022 / 0.025 / 0.108 |

K の `run_ms` 中央値は既定で36.9%、20 overridesで72.4%減った。
P/B は今回変更した経路を通らず、tessellation のアルゴリズムも変更していない。
他プロセスの負荷を固定していない別時点の測定なので、これらの時間差を本修正の効果とは断定しない。
実機 CPU 使用率・GPU 使用率・FPS の改善率へ換算することもできない。

内訳は従来と同じ独立 Context / layout であり、全体フレームへ加算する分解値ではない。
修正後は実際の保持経路を測るため、最初の2段を `keymap_and_conflicts_cache` の1段へ置き換えた。
warm-up 後の未変更設定を測っており、初回構築や設定変更フレームの費用はこの段の値に含まない。

| 関数 / 経路 | K 既定 修正前 | K 既定 修正後 | K20 修正前 | K20 修正後 |
| --- | ---: | ---: | ---: | ---: |
| `from_settings`（内部競合検出込み） | 0.289 / 0.351 / 0.620 | 毎フレーム実行なし | 0.285 / 0.404 / 0.730 | 毎フレーム実行なし |
| ページの2回目の `binding_conflicts` | 0.278 / 0.435 / 0.802 | 毎フレーム実行なし | 0.223 / 0.433 / 0.725 | 毎フレーム実行なし |
| 設定比較 + キャッシュ取得 | — | 0.0005 / 0.0007 / 0.0010 | — | 0.0019 / 0.0029 / 0.0131 |
| `keyboard_chord_picker` | 1.554 / 1.970 / 2.397 | 0.304 / 0.382 / 0.783 | 1.821 / 2.543 / 3.405 | 0.244 / 0.358 / 0.571 |
| `command_list` | 5.441 / 6.807 / 8.064 | 4.953 / 5.479 / 6.474 | 12.635 / 15.608 / 19.415 | 4.560 / 5.278 / 6.363 |

AccessKit 無効・tooltip を表示していない定常 pass では、呼び出し経路から求めた
`effective_chords` 呼び出し数は61,223回から一覧の583回へ減る。
tooltip を表示する場合はそのキーの操作走査が必要で、AccessKit node があるキーは従来どおり先に走査する。
この計算は allocator の確保回数ではない。

全4ケース・1,080サンプルが引き続き1 pass。修正前とフレームごとに照合し、
時刻・pointer・hover 対象・キー矩形・shapes / primitives / vertices / indices・
repaint delay / 即時要求の有無は全て同一だった。各フレームの repaint 原因も同一。
設定・選択・展開・編集状態の不変と `settings.db` 未保存の harness 検査も成功した。
観測した範囲では表示・入力・保存の意味が変わる差はなく、コード上もその変更は加えていない。
製品バイナリは起動せず、lib 全件・build-dev・コミットは利用者指定により実施していない。
独立レビューは実装・テスト・正式 JSON の統計再計算と前後の描画/repaint 照合を行い、
最終 ACCEPT、未解決指摘なし。レビュー側でビルド・テストの重複実行はしていない。

## §1.811 と説明文の保持

対象はコマンド一覧タブ、キーボードタブ内の一覧、キーボード図の説明文。
行の仮想化や再描画の間引きは入れず、全行を従来の Grid で配置する。
製品バイナリは起動しない。初回表示や入力変更時には再計算し、変更のない描画で再利用する。

### 保持する値と入力

`PreferencesState` ごとに両一覧の行・幅を別々に保持する。両一覧は同じ保持用の型を使い、
Keymap と競合は §1.808 の `command_keymap()` の Arc を使う。
読み出すときに入力を借用して等値比較し、不一致のときだけ入力を複製して作り直す。
編集・既定復帰・取り込み等の各変更経路に無効化は足さない。

| 保持値 | 照合する入力 |
| --- | --- |
| 一覧の行 | `CommandKeymapCache` の Arc 同一性、生の `settings.keymap` 全体、`settings.ring_shortcuts` 全体、`operation_keyboard_context`、`command_filter`、`command_key_filter` |
| 割り当て列の幅 | 上記の行の更新に加え、pixels per point、通常文字・monospace の実 FontId、`override_text_style`、`text_styles` 全体、稼働中の `FontDefinitions` と `TextOptions`、`item_spacing.x`、`extra_text_line_spacing`、`text_valign` |
| キーの説明文 | 描画用 `CommandKeymapCache` の Arc 同一性、Ctrl / Shift / Alt、`operation_keyboard_context`。この組の中で chord ごとに保持 |

行の元関数が読むのは、Keymap の実効 chord、競合、raw override の有無、リング設定、場所、
両検索語と、コード内の action / context / 表示名 / 説明 / 並び順の定義である。
生の KeymapSettings も鍵に含めるのは、キーボードタブの既定復帰ボタンを押したフレームに、
従来どおり実効 chord と競合は描画冒頭の snapshot、状態欄だけは変更後の生設定を読むため。
「上書き」は行を作り直すときに `raw_overridden_actions` を1回呼んだ集合で判定する。
キーボード内一覧ではリング設定は出力に影響しないが、共通の鍵として保守的に照合する。

フォントは egui の実際の優先順位に合わせ、`override_font_id` が通常・monospace の双方で
最優先となる。定義内の font data / family / tweak も等値比較の対象である。
本体には独自 FontProvider / GlyphRasterizer の登録経路がない。
幅の測定は Extend / 無限幅なので、窓の幅・可視範囲は幅の鍵にしない。
色、選択、ホバー、AccessKit の有効・無効も文字幅や行データの値を変えないため鍵にしない。
選択中の表示とクリックの処理、Grid の配置、tooltip の現在の幅・書式は描画時に評価する。
幅を毎フレーム測る例外は残さず、一覧の描画では計測済み幅を使う helper を通す。
ほかの割り当て表示の呼び出し元では従来の幅計測を維持する。

キーの本文は必要なキーだけ遅延作成する。AccessKit 無効・非ホバー時には作らず、
有効時は全102キーで最初に要求される本文を保持し、以後の説明登録・ホバー表示で共用する。
保持値は Arc を持つ WidgetText で、旧 String の Label と同じ書式になるよう PLACEHOLDER 色を
明示する。一覧の説明は元から静的文字列なので、String に複製せず参照として保持する。
共通 helper は、AccessKit node があれば `on_hover_text` で登録し、それ以外はホバー時の
`on_hover_ui` で同じ `tooltip_width` と `Label` を使う。

### 計測条件

修正前データは `target/hover-811-before.json`、ログは `target/hover-811-before-test.log`。
既存4場面にキーボードの AccessKit 有効版2場面と、コマンド一覧の8場面を追加した。
コマンド一覧は全場所、変更0 / 20件 × AccessKit 無効 / 有効 × ホバー / スクロール。
ホバーは実描画した「編集」ボタンの列を2 pt/frameで往復する。
スクロールは一覧の上にポインタを置き、実 ScrollArea にホイール -2 pt/frame を送る。
4条件すべて240フレームで厳密に2 ptずつ、合計480 pt移動した。
ホバー条件ではスクロール位置0を維持し、142/240フレームで実ボタン上を通過した。
キーボードは既存と同じF13～F24の水平移動で、213/240フレーム、10キー上を通った。

各場面40 warm-up / 240測定 / 30停止フレーム、全14場面・3,780フレームが1 pass。
画面、フォント、20件の作り方、合成60Hz、tessellation等の条件は前節と同じ。
設定・選択・編集状態の不変と、隔離 data-dir に `settings.db` がないことを検査する。
区間計測は別の warmed Context で実際の製品描画関数を呼び、テスト専用の時計で
Keymap+競合、行作成、幅、Grid、キー本文を分ける。キー本文は picker の内数である。
これらは窓全体の `run_ms` に加算する分解値ではない。修正後だけ追加する `page_total` は、
同じ単体 Context の描画区間に対する残存割合を求めるために使う。
Commands はページ全体、Keyboard は Keymap 取得・図・一覧の部分である。
区間内訳の JSON は集計値のみを保持し、窓全体の JSON は全フレームを保持する。

Windows x86_64、`dev-runtime`、製品と同じ mimalloc、1 test thread。
すべて Owner `codex-811` の指定 runner を通し、子コマンドで TEMP/TMP をこのツリーの
`target/hover-tmp`、`MSBUILDDISABLENODEREUSE=1` とした。
修正前の Cargo 並列数は1。runner はロック/TEMP領域への書き込み制限を警告して
BelowNormal で実行した。初回の最適化コンパイルは約37分、計測テスト自体は340.08秒で成功。
他プロセスの負荷は固定していない。実機の CPU / GPU 使用率・FPS への換算はしない。

### 追加回帰

`ui_dialogs::preferences::pages::tests::` に次の8件を追加した。

- `hover811_rows_match_original_overview_and_list_for_all_contexts_and_filters`:
  変更0 / 20件、リング割り当てあり、全13場所と「すべて」、操作検索・キー検索で、
  overview の元 helper と一覧の元の手順に行・割り当て・状態・説明が一致する。
  同じ全組合せで両一覧の保持幅を元の行からの fresh 幅と照合し、構築1回・同入力の2回目は再利用する。
- `hover811_rows_rebuild_for_each_input_and_preserve_mid_frame_snapshot`:
  各入力変更・適用・全件既定復帰で新しい行を作り、同じ入力では Arc を再利用する。
  既定復帰フレームの実効 chord / raw 状態の参照時点と、選択・修飾だけでは行を作り直さないことも検査する。
- `hover811_width_reuses_and_tracks_actual_font_selection_spacing_and_alignment`:
  元の幅計測との一致、再利用、monospace・style・FontId・間隔・行間・配置の変更を検査する。
- `hover811_width_tracks_pixels_fonts_definitions_and_text_options_without_rebuilding_rows`:
  pixels per point・フォント定義・hinting の変更で幅だけを作り直し、行の Arc は維持する。
- `hover811_keyboard_tooltip_cache_matches_original_and_invalidates_each_dependency`:
  全102キー・全場所・修飾の組と、適用・既定復帰で元の本文と一致し、同じ要求は再利用する。
- `hover811_keyboard_tooltip_rebuilds_once_for_each_owner_input_and_keeps_lazy_subset`:
  Ctrl / Shift / Alt / 場所を個別に変え、必要なキーだけ1回作って保持する。
- `hover811_cached_keyboard_tooltip_uses_original_label_layout_format`:
  フォント・色・縦配置の上書きがあるときも、旧 String と保持した WidgetText の Label 用 LayoutJob が一致する。
- `hover811_row_tooltips_are_lazy_and_accesskit_preserves_every_description`:
  両一覧の初回・定常フレームで全行の AccessKit 説明を照合し、定常1 pass の登録回数も検査する。
  無効時は両フレームとも非ホバーの本文要求0回。全102キーの説明登録と無効時0回は既存 `hover808_` の検査も維持する。

最初のコンパイルでは追加テストの `Options.text_options` という存在しない API を検出し、
現行の `global_style_mut` / `Style.visuals.text_options` へ修正した（製品コードのエラーではない）。
438件の focused 実行は437件成功し、上記の行説明テストだけが初回 Grid の不可視 sizing pass まで
登録回数に含める誤った期待で失敗した。cold の最終出力の文字列検査を残し、steady の1 passで
各行1回を厳密に数える形へテストだけ修正した。製品の条件や期待する説明文字列は変更していない。
成功済み437件は同じ製品ソース・条件の証拠として再利用する。

### 修正前後の結果

修正前は2026-11-18、修正後は2026-11-19。同じハーネス・profile・fixtureで測定した。
修正後: `target/hover-811-after.json`、`target/hover-811-after-test.log`。
修正後も24ケースが成功し、`1 passed; 0 failed; 18911 filtered out; finished in 50.77s`、
runner exit 0。Cargo の並列数だけ4とした（測定時は両方1 test thread）。
以下は **中央値 / p90、ms/frame**。最大値と全フレームは JSON に残している。

| 場面 | 修正前 | 修正後 |
| --- | ---: | ---: |
| コマンド一覧・変更なし・AK無効・ホバー | 74.926 / 80.271 | 4.874 / 5.728 |
| コマンド一覧・変更なし・AK無効・スクロール | 79.637 / 88.987 | 5.107 / 6.000 |
| コマンド一覧・変更なし・AK有効・ホバー | 85.092 / 92.099 | 8.348 / 9.657 |
| コマンド一覧・変更なし・AK有効・スクロール | 83.034 / 90.281 | 10.560 / 13.120 |
| コマンド一覧・20件・AK無効・ホバー | 83.618 / 98.990 | 6.660 / 8.053 |
| コマンド一覧・20件・AK無効・スクロール | 85.770 / 97.400 | 6.035 / 7.455 |
| コマンド一覧・20件・AK有効・ホバー | 88.493 / 100.030 | 10.058 / 13.256 |
| コマンド一覧・20件・AK有効・スクロール | 83.588 / 89.199 | 10.022 / 11.649 |
| キーボード・変更なし・AK無効 | 4.808 / 5.306 | 3.817 / 4.263 |
| キーボード・20件・AK無効 | 4.697 / 5.440 | 3.665 / 4.235 |
| キーボード・変更なし・AK有効 | 8.650 / 10.132 | 4.895 / 5.943 |
| キーボード・20件・AK有効 | 9.455 / 12.193 | 5.605 / 6.632 |
| P General / 左ツリー | 0.144 / 0.219 | 0.104 / 0.164 |
| B General / 窓外 | 0.136 / 0.200 | 0.093 / 0.122 |

この1回の測定で、コマンド一覧8条件の中央値は87.3～93.6%減った。
キーボードはAK無効で20.6 / 22.0%、有効で43.4 / 40.7%減った（変更なし / 20件）。
一方、対象経路を通らないP/Bも27.7 / 31.2%減り、Grid単体には増加した条件もある。
他プロセスの負荷を固定していない別時点の測定であり、全時間差を保持の効果とは断定しない。
行・幅・キー本文を定常フレームで作り直さなくなったことは、時間とは別にテストでも確認する。

全14場面・3,780フレームを前後で照合し、時間の測定値以外は全項目が一致した。
shape / primitive / 頂点 / index数、pointer・hover対象と矩形、スクロールの入力と位置、
pass数、AccessKit更新の有無、repaint delay、前passのrepaint原因も同一である。
独立レビューでも全フレーム・全体の中央値/p90/最大・ログ24記録との一致を再計算し、相違0。
設定・選択・編集状態の不変と隔離data-dirへの未保存も、同じharnessのassertが成功した。
実機の操作感・CPU/GPU使用率・FPSの検証ではない。

単体区間の前後比較（中央値 / p90、ms）。幅・行の修正後には入力照合と保持値の取得を含む。
Grid は配置だけでなく、全行の Label・割り当て表示・説明登録も含む。
説明登録に必要な egui の文字列所有化・node構築は残る。

| コマンド一覧条件 | 区間 | 修正前 | 修正後 |
| --- | --- | ---: | ---: |
| 変更なし・AK無効 | Keymap+競合 | 0.0016 / 0.0020 | 0.0008 / 0.0010 |
| 変更なし・AK無効 | 行 | 73.1403 / 81.0896 | 0.0038 / 0.0050 |
| 変更なし・AK無効 | 幅 | 0.3036 / 0.3740 | 0.0103 / 0.0129 |
| 変更なし・AK無効 | Grid | 4.6066 / 5.3475 | 5.6384 / 6.7648 |
| 変更なし・AK有効 | Keymap+競合 | 0.0018 / 0.0024 | 0.0009 / 0.0014 |
| 変更なし・AK有効 | 行 | 76.8117 / 92.1880 | 0.0038 / 0.0049 |
| 変更なし・AK有効 | 幅 | 0.2827 / 0.3748 | 0.0094 / 0.0118 |
| 変更なし・AK有効 | Grid | 6.4666 / 8.3927 | 8.0084 / 8.9432 |
| 20件・AK無効 | Keymap+競合 | 0.7023 / 0.8022 | 0.0041 / 0.0057 |
| 20件・AK無効 | 行 | 98.7992 / 109.8285 | 0.0064 / 0.0087 |
| 20件・AK無効 | 幅 | 0.3248 / 0.4081 | 0.0100 / 0.0130 |
| 20件・AK無効 | Grid | 5.0922 / 6.1565 | 5.7633 / 7.0830 |
| 20件・AK有効 | Keymap+競合 | 0.7114 / 0.8016 | 0.0047 / 0.0060 |
| 20件・AK有効 | 行 | 98.6185 / 108.4471 | 0.0069 / 0.0090 |
| 20件・AK有効 | 幅 | 0.3196 / 0.4027 | 0.0100 / 0.0123 |
| 20件・AK有効 | Grid | 6.9461 / 8.4429 | 9.4389 / 11.9497 |

| キーボード条件 | 区間 | 修正前 | 修正後 |
| --- | --- | ---: | ---: |
| 変更なし・AK無効 | Keymap+競合の保持値取得 | 0.0006 / 0.0008 | 0.0008 / 0.0010 |
| 変更なし・AK無効 | 一覧の行 | 0.9216 / 1.0645 | 0.0042 / 0.0052 |
| 変更なし・AK無効 | 一覧の幅 | 0.1261 / 0.1554 | 0.0103 / 0.0136 |
| 変更なし・AK無効 | 一覧Grid | 4.1369 / 4.6612 | 4.6252 / 5.3988 |
| 変更なし・AK無効 | キーボード図 | 0.3130 / 0.3964 | 0.4228 / 0.5880 |
| 変更なし・AK無効 | キー本文（図の内数） | 0 / 0 | 0 / 0 |
| 変更なし・AK有効 | Keymap+競合の保持値取得 | 0.0007 / 0.0010 | 0.0009 / 0.0014 |
| 変更なし・AK有効 | 一覧の行 | 0.9855 / 1.1387 | 0.0041 / 0.0055 |
| 変更なし・AK有効 | 一覧の幅 | 0.1371 / 0.1785 | 0.0103 / 0.0130 |
| 変更なし・AK有効 | 一覧Grid | 5.6245 / 6.7846 | 6.5192 / 7.8323 |
| 変更なし・AK有効 | キーボード図 | 1.6429 / 1.9412 | 0.6310 / 0.8282 |
| 変更なし・AK有効 | キー本文（図の内数） | 1.1325 / 1.3157 | 0 / 0 |
| 20件・AK無効 | Keymap+競合の保持値取得 | 0.0026 / 0.0034 | 0.0034 / 0.0045 |
| 20件・AK無効 | 一覧の行 | 0.9566 / 1.0959 | 0.0060 / 0.0085 |
| 20件・AK無効 | 一覧の幅 | 0.1111 / 0.1356 | 0.0092 / 0.0122 |
| 20件・AK無効 | 一覧Grid | 4.2612 / 4.7657 | 4.1703 / 5.0716 |
| 20件・AK無効 | キーボード図 | 0.3178 / 0.3861 | 0.4044 / 0.5413 |
| 20件・AK無効 | キー本文（図の内数） | 0 / 0 | 0 / 0 |
| 20件・AK有効 | Keymap+競合の保持値取得 | 0.0027 / 0.0035 | 0.0041 / 0.0057 |
| 20件・AK有効 | 一覧の行 | 0.9760 / 1.1018 | 0.0063 / 0.0082 |
| 20件・AK有効 | 一覧の幅 | 0.1341 / 0.1651 | 0.0099 / 0.0128 |
| 20件・AK有効 | 一覧Grid | 5.4837 / 6.1941 | 6.1925 / 7.2659 |
| 20件・AK有効 | キーボード図 | 2.2210 / 2.5344 | 0.5923 / 0.7808 |
| 20件・AK有効 | キー本文（図の内数） | 1.6991 / 1.9030 | 0 / 0 |

キー本文の0は、定常の測定フレームで元の本文作成関数が呼ばれず、JSONの該当区間が
存在しなかったことを表す。説明登録・保持値の検索は図の区間に含まれる。
初回や修飾・場所・割り当て変更時の本文作成は残す。

### 保持後に残る費用

以下の割合は **同じ単体ContextのGrid中央値 / page_total中央値**。
各フレームの割合の中央値や、別Contextの窓全体への寄与率ではない。

| 区間・条件 | Grid（中央値 / p90、ms） | page_total（同） | 中央値同士の比 |
| --- | ---: | ---: | ---: |
| コマンド一覧・変更なし・AK無効 | 5.6384 / 6.7648 | 5.7234 / 6.8427 | 98.51% |
| コマンド一覧・変更なし・AK有効 | 8.0084 / 8.9432 | 8.1517 / 9.1032 | 98.24% |
| コマンド一覧・20件・AK無効 | 5.7633 / 7.0830 | 5.9217 / 7.2888 | 97.32% |
| コマンド一覧・20件・AK有効 | 9.4389 / 11.9497 | 9.7251 / 12.1822 | 97.06% |
| キーボード内一覧・変更なし・AK無効 | 4.6252 / 5.3988 | 5.1372 / 5.9318 | 90.03% |
| キーボード内一覧・変更なし・AK有効 | 6.5192 / 7.8323 | 7.1707 / 8.5795 | 90.91% |
| キーボード内一覧・20件・AK無効 | 4.1703 / 5.0716 | 4.6341 / 5.6810 | 89.99% |
| キーボード内一覧・20件・AK有効 | 6.1925 / 7.2659 | 6.8549 / 7.9171 | 90.34% |

コマンド一覧は行の作り直しが約73～99 msから入力照合の約0.004～0.007 msとなり、
幅も約0.28～0.32 msから約0.009～0.010 msとなった。保持後は全行Grid描画が約97～99%を占める。
キーボード内の一覧も行・幅の作り直しを省けたが、一覧Gridが対象区間の約90%で最大の費用として残る。
AccessKit有効時のnode生成・説明登録を含むため、これを純粋なレイアウト計算だけの費用とは呼ばない。
可視行だけ配置する変更は今回入れていない。必要なら、可変行高・列幅・編集ボタンの識別と
読み上げ情報を保つ別の設計として検討する。

### 最終検証と引き渡し

`dev-runtime`、default features、1 test thread、次の focused 範囲を確認した。
nextest のビルド並列数は `--build-jobs 4` とした。

| 対象 | 結果 | ログ / JUnit |
| --- | --- | --- |
| `test(/ui_dialogs::preferences::/) \| test(/keymap::tests::/)` | 438実行、437成功、追加テストの初期pass数の期待だけ1失敗 | `target/hover-811-focused3.log` / `target/hover-811-focused3-junit.xml` |
| 初期/定常の行説明テスト（修正後、exact名） | 1成功、0失敗 | `target/hover-811-focused4.log` / `target/hover-811-focused4-junit.xml` |
| 行・幅の全場所/検索matrix（幅の直接比較を補強後、exact名） | 1成功、0失敗 | `target/hover-811-focused5.log` / `target/hover-811-focused5-junit.xml` |
| 修正前 / 修正後の ignored headless 計測（名前フィルタ） | 各1成功、0失敗、24ケース | `target/hover-811-{before,after}-test.log` / `target/hover-811-{before,after}.json` |

成功の有効範囲は **keymap 200種類 + preferences 238種類 = 438種類**。
最初の成功437種類のうち、後から幅の直接比較を追加した1種類は最新の単独成功で置き換えた。
それ以外の436種類に、補強した幅テスト1種類と修正した行説明テスト1種類を合わせ、
最終の438種類が全て成功している。
製品ソースを変えずテストだけを修正・補強したため、他の成功済みテストを一括で再実行していない。
JUnitの種類数・唯一の初回失敗と再確認は独立レビューでも照合した。

幅matrixの補強は修正後計測の後だが、変更したのは `#[cfg(test)]` の当該テスト本体だけ。
製品と計測の経路は同じなので、修正後の実測を再利用する。
測定時のソースhash・差分は `target/hover-811-after-source.json` と
`target/hover-811-after-measured.patch`、比較集計は `target/hover-811-comparison.txt` に保持する。

途中の最初の nextest 呼び出しでは `-j` をビルド並列数と誤認し、`--test-threads` と重複して
CLI exit 2（コンパイル・テスト0件）になった。`--build-jobs` へ修正した。
前述の追加テストAPIのコンパイルエラーは exit 101・テスト0件。
これらを製品テストの失敗や成功実行数には含めない。

`cargo fmt`（引数なし）、`cargo fmt --check`、`git diff --check`、
`python scripts/check_ui_glyphs.py`（危険グリフ0件）を実行した。
独立レビューは GPT-6.1 Sol / xhigh が設計・実装・追加テスト・JSON・数値表を確認し、
最終 ACCEPT、未解決指摘なし。重複ビルド/テストはしていない。
観測範囲で表示・入力・保存の意味が変わる差は見つかっていない。
製品バイナリ起動、lib全件、`build-dev`、コミットは利用者の指定により実施していない。
