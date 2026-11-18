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
`vendor/ffmpeg/bin/*.dll` を `target/dev-runtime/deps/` へコピーして同じ runner で実行する。

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
