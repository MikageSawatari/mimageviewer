# 補正プリセット・AI キャッシュ設計

画像補正 (adjustment) と AI アップスケール/デノイズ/Inpaint は、複数レイヤーのキャッシュと優先順位の決定ロジックで
成り立っている。「補正したら元に戻った」「AI 結果が一瞬消える」といった不具合は、ここの無効化ルールの間違いから起きる。

---

## 1. スコープ (v0.8.1 で 3 層化)

補正パラメータは **3 スコープ + 10 個の全設定スロット + 4 個のカラー化専用スロット**
で構成される:

```
スコープ              保存先
────────────────────────────────────────────────
グローバル            settings.json の global_preset
お気に入り標準        adjustment.db の favorite_params テーブル (favorite_id TEXT PK)
ページ個別            adjustment.db の page_params テーブル

保存スロット 0〜9     settings.json の preset_slots  (独立)
カラー化スロット 1〜4 settings.json の colorize_preset_slots (独立)
```

旧 (v0.5.0〜v0.6.0 開発版) の「フォルダ単位 4 プリセット + ページ→プリセット idx」方式は廃止した。
未リリース機能だったため DB マイグレーションは行わず、`AdjustmentDb::open()` が
旧テーブル `presets` / `page_presets` を `DROP TABLE IF EXISTS` で破棄し、
新しい `page_params(page_path TEXT PK, params_json TEXT)` を作成する。

v0.8.1 で「お気に入り単位の標準 (favorite_params)」を追加した。フォルダ単位より粗く・
グローバルより細かい中間層で、用途別 (スキャン画像 / AI 生成 / カメラ写真 / Twitter DL など)
に既定を切り替えたい要件に応える。

### 1.1 有効パラメータの決定

表示時のページ `idx` の有効パラメータは 3 層のカスケードで決まる:

```
effective =
    adjustment_page_params.get(idx)
      ?? adjustment_favorite_params.get(nearest_favorite_id_of(container(idx)))
      ?? settings.global_preset
```

解決は `App::effective_params(idx)` に集約される。お気に入り層のルックアップには
`App::current_favorite_id_for_idx(idx)` → `App::find_nearest_favorite(container_path)`
を使う。**最も近い祖先** (パス最長一致) を選ぶため、ネスト登録されたお気に入り
(例: `G:\pics` と `G:\pics\AI` が両方登録されている) のときは `G:\pics\AI` が優先される。

ZIP/PDF を開いている最中は `container(idx)` が ZIP/PDF 本体のパスになるため、
「AI 生成画像」お気に入り配下の ZIP 内ページにもそのお気に入り標準が自動適用される。

`adjustment_page_params: HashMap<usize, AdjustParams>` はフォルダ/ZIP/PDF ロード時に
`AdjustmentDb::load_page_params(prefix)` で一括読込される。
★固定 (snapshot) の activate / deactivate / list 復帰のように `load_folder` を通さずに
`items` を差し替える経路では、`App::rehydrate_page_edit_state_for_current_items(prefix)`
が同じ clear + DB ロードを行って idx-keyed 状態 (この `adjustment_page_params` に加え
`local_adjust_pages` (バッジ用 key 集合) / `local_adjust_page_layers` (遅延ロード済み JSON cache) /
`local_adjust_selected_layers` /
`export_crop_page_settings` / `export_crop_pages` / `mask_pages` / `conceal_pages`) を新しい
idx へ hydrate し直す。これをやらないと差し替え前 idx の補正・マスクが別ページに乗る
(Codex P1 2026-06-05)。補正レイヤーはフォルダロード / rehydrate 時点では
`local_adjust.db` の `page_path` exact lookup (`IN`) で `local_adjust_pages` だけを復元し、
巨大な `layers_json` はフルスクリーン表示 / 補正レイヤーパネルに入ったタイミングで
1 ページ分だけ遅延要求する。DB read / JSON deserialize は UI thread では行わず、
`local-adjust-render` worker が `LocalAdjustDb` を read-only で開いて mask resize / compose まで
完了させる。
**`load_folder` 側の hydration を変えたら同関数も揃えること**。
ただし **cross-folder 検索 (Ctrl+S/Ctrl+G) 由来 snapshot は `App::clear_page_edit_state()`
で clear のみ** (= subset が cross-folder で単一 prefix hydrate できず、検索 view は元々
ページ編集 overlay を出さない)。Ctrl+F (単一フォルダ構造フィルタ) は検索ではないので
rehydrate 側。`clear_page_edit_state()` 単独は上記 idx-keyed セットの正準 clear で、
`replace_search_view_items` (Ctrl+G 結果差し替え) からも呼ばれる。
`adjustment_favorite_params: HashMap<Uuid, AdjustParams>` は **起動時に 1 回**
(`App::hydrate_adjustment_favorite_params`) 全件ロードされ、
`settings.favorites` に存在しない orphan 行は `prune_favorite_params` で掃除される。

### 1.2 自動個別化

補正パネルのスライダーや AI モデル選択を操作した瞬間に、その変更を含む
`AdjustParams` が「現在のページ個別パラメータ」として書き込まれる
(`App::set_page_params(idx, params)`)。スコープ切替という明示操作は存在しない。

`set_page_params` は **「個別パラメータが `effective_default_for_idx(idx)` (= お気に入り標準
があればそれ、なければグローバル) と完全一致」** したときだけ個別レコードを削除する
(フォールバックでその標準が使われるため保存不要)。v0.8.1 以前はグローバル
との等価比較だったが、お気に入り標準が登場したのでそこに切り替えた。

旧バージョンは `is_removable()` (= identity かつ AI 未使用) で削除判定していたが、
**「グローバルが AI ON、特定ページだけ AI OFF」のような上書き** を保存した直後に
個別が消えてしまい、ユーザの意図 (デノイズ OFF など) が反映されない不具合があった。
同じ原理で、「お気に入り標準が AI ON、特定ページだけ AI OFF」も `effective_default_for_idx`
との等価比較によって保存される。DB 側 (`AdjustmentDb::set_page_params`) の
`is_removable` 判定は廃止済みで、呼び出し側 (`App::set_page_params`) で
削除/保存の振り分けを行う構造になっている。

### 1.3 アクションボタン

補正パネルに 6 つのボタンがある (v0.8.1):

| ボタン | 動作 |
| --- | --- |
| このフォルダの全画像に適用 | 現在のパラメータを、現フォルダ/ZIP/PDF の全画像ページに一括書込 (`apply_params_to_all_pages`)。`matches_default` 判定はその一覧の先頭 idx の `effective_default_for_idx` で代表する (一覧内は同じコンテナに属するため同じ標準を共有する) |
| このフォルダの全画像から解除 | 現フォルダ/ZIP/PDF の全画像ページから個別設定を削除 (`clear_all_page_params`)。個別を解除するとお気に入り標準 or グローバルにフォールバック |
| このお気に入り「{name}」の標準にする | 現在のパラメータをそのお気に入りの標準として保存 (`set_favorite_default`)。お気に入り登録フォルダ配下にいないとき disabled |
| このお気に入り「{name}」の標準を解除 | そのお気に入りの標準を削除 (`clear_favorite_default`)。以後そのお気に入りはグローバルにフォールバック。未登録のとき disabled |
| 標準にする | 現在のパラメータを `settings.global_preset` にコピー (`copy_params_to_global`) |
| 個別設定を解除 [Q] | 現ページの個別レコードを削除 (`clear_page_params`)。フォールバックで「お気に入り標準 → グローバル」の順にマッチしたものが効く。フルスクリーン・グリッドとも `Q` / `Ctrl+Backspace`、グリッドはチェック済み (なければ選択 1 件) に一括適用可能 (`clear_page_params_for_selection`) |

ボタンラベル内の `{name}` は `ui_helpers::truncate_name(name, 10)` で 10 文字に切り詰める。
スコープ表示 (ヘッダー直下) も同様に 3 種類: `個別設定を適用中` / `お気に入り「{name}」の標準を適用中` / `標準設定を適用中`。

### 1.4 保存スロット

10 個の名前付きスロット。フルスクリーンで `Ctrl+1〜9` / `Ctrl+0` を押すと
`App::apply_slot_to_current_page(slot_idx)` が呼ばれ、該当スロットのパラメータを
**現在のページ個別設定として書き込む** (= そのページを個別化する)。

`Ctrl+Alt+1〜9` / `Ctrl+Alt+0` は、同じスロットを**標準設定側へ読み込む**。
対象ページがお気に入り配下ならそのお気に入り標準、配下でなければアプリ全体標準を
スロット内容で置き換えたうえで、対象ページの個別設定を解除する。見開き Double では
補正パネルの L/R 対象に従う。この操作は標準側の更新と個別解除を 1 件の Undo として記録する。

> 旧来は `Shift+0〜9` だったが、egui の logical-key 方式ではキーボード配列によって
> Shift+数字が記号 (`!"#$%&'()` など) に置き換わり `Key::Num1` 等にマッチしないため
> Ctrl 修飾に変更した (JIS 配列の Shift+0 は文字を生成しないため特に致命的だった)。

補正パネルの保存スロット欄 (`💾` ボタン) で現在のパラメータをスロットに保存できる。
カラー化タブ内の 4 スロットは `ColorizeParams` だけを保存するため、呼び出しても色調・AI・
ポストフィルタを上書きしない。カラー化専用スロットはカラー化タブの末尾に置き、
「カラー化設定保存スロット」の見出しと、1〜4 のロード／保存ボタン行を分けて表示する。
全設定を扱う共通欄は、専用欄と区別して「画像補正保存スロット」と表示する。

動画補正は画像用とは独立した 10 個の名前付きスロット (`Settings::video_preset_slots`) を持つ。
native presenter の画像補正パネル末尾に読込行と保存行を表示し、読込は空スロットを disabled にする。
`Ctrl+1〜9` / `Ctrl+0` は動画表示中だけ動画補正スロットを読み込み、音声モード中は発火しない。
保存はパネルのボタンだけから行う。読み込んだ明るさ・コントラスト・ガンマ・彩度・色温度・
レベル補正・Creative LUT は viewer-wide な `Settings::video_adjustments` を置き換え、
再生中の presenter へ即時同期する。ページ個別記憶は持たない。

補正パネルは、上から「見開き対象／適用中スコープ」「フォルダ・お気に入り・グローバル操作」
「色調／AI／カラー化／フィルタのサブタブ」「すべてリセット」「全設定保存スロット」の順。
サブタブは中央の設定部分だけを切り替え、スコープ表示と共通操作はどのタブでも残す。
フィルタタブはグループ見出し付きの縦 Radio で全候補を表示する。

### 1.5 見開き表示中の左右独立補正 + コピー

`adjustment.db` の `page_params` はもともと「画像 / ZIP エントリ / PDF ページ」単位で
独立しているため、見開き Double 表示中の左右ページは別々の `AdjustParams` を持てる。
補正パネルは見開き Double のときだけヘッダー直下に **L/R セレクタ + コピーボタン** を出し、
編集対象の切替と片側→もう片側への転写を 1 パネルで完結させる。

| 要素 | 仕様 |
| --- | --- |
| L/R セレクタ | 「左ページ」「右ページ」の 2 ボタン (selectable_label)。既定は **常に左ページ**。`open_fullscreen` (= ページ送り、ペア切替、初回フルスクリーン) と spread_mode 切替で `AdjustSpreadTarget::Left` にリセット。Single (単ページ / 表紙単独 / 横長単独) のときは表示しない |
| L/R 基準 | **画面上の左右で固定** (LTR/RTL 不変)。消しゴム `EraseSpreadCtx` の慣習と統一 |
| 編集対象解決 | パネル冒頭で `resolve_spread_pair(fs_root_idx)` を引き、Double のとき `adjust_spread_target` に応じて `target_idx = left or right`。以降のスライダー読み書き / 個別化 / Undo はすべて `target_idx` 経由 (= 単ページ経路と同一) |
| コピーボタン | 「← コピー」(右→左) と「コピー →」(左→右) の対称配置。`App::copy_spread_adjust(src, dst)` を呼ぶ |
| 同一判定 | `effective_params(left) == effective_params(right)` (実効値の `==`)。一致しているときは両コピーボタン disabled (= 「揃っている」状態が UI から自明) |

#### コピー操作の中身

`copy_spread_adjust(src, dst)` は `effective_params(src)` (3 層解決後) を `set_page_params(dst, ...)`
で書く。`set_page_params` 内部の `matches_default` 比較で、dst の標準 (お気に入り標準 or
グローバル) と一致するなら page_params エントリは作られず、カスケード解決に任せる挙動が
自動で得られる (= DB エントリが無駄に増えない)。書き換えは `capture_adjust_full` で囲んで
あるので Undo に乗る。AI 設定 (upscale_model / denoise_model) が変わるなら
dst の final pipeline cache / pending を落として final AI を再実行させる。

#### 見開き描画と補正適用の隠れた前提 (実装メモ)

これまで `adjustment_active = self.adjustment_mode && !is_spread_double` でパネル全体が
disabled だった頃は顕在化していなかった 2 点を併せて修正している:

- 見開き左右ページも `resolve_fs_processed_texture` を通して `final_composite_cache` を
  取得する。これがないと片側だけ final pipeline が走らず、スライダー変更や final AI 完了が
  見開きに反映されない。
- final composite は各ページ idx ごとに `edit_result_cache` から lazy 生成するため、
  見開きの右ページも描画時に同じ経路で色調補正・final AI・スマートシャープ・カラー化・
  Creative LUT・post_filter が適用される。

#### 「両ページ同時編集」を入れない理由

設計初期は「両ページ」モードを検討したが、左右が異なる状態で「両ページ」を選んだ瞬間に
片方の補正値がもう片方で上書きされる破壊的操作が避けられず、デフォルト選択
(同一なら両ページ / 異なれば左) のロジックも暗黙的になりやすい。代わりに
「常に左ページから始まる + 揃えたいときはコピーボタン」という明示的 UI に絞った。
両ページ同値の運用要望は **お気に入り標準 (favorite_params) で代替できる** ため、
今回は意図的にスコープ外。

---

## 2. AdjustParams の中身

`adjustment.rs::AdjustParams`:

- `brightness`, `contrast`, `gamma`, `saturation` (色情報の定番)
- `temperature` (±100。色温度。±0 以外だと f32 パイプライン必須)
- `black_point`, `white_point`, `midtone` (トーンカーブのレベル補正)
- `auto_mode`: `None` / `Auto` / `MangaCleanup` (自動補正モード)
- AI 関連: `upscale_model`, `denoise_model` (`Option<String>`)
- **カラー化**: `colorize: ColorizeParams` (近モノクロ判定、階調パレット、
  スクリーントーン濃淡変換。final pipeline 専用)
- **Creative LUT**: `creative_lut: CreativeLutSelection` (環境設定で登録した 3D `.cube` の
  ID と適用量。入力色空間変換ではなく、最終的なルックを変える表示用 LUT)
- **ポストフィルタ**: `post_filter: PostFilter` (レトロ系表示エフェクト、色調補正の後に適用)
- **シャープ化**: `smart_sharpen: u8` (0 = OFF, 1..=100。最終表示段スマートシャープの強度。
  final pipeline 専用でサムネイルには反映しない。§2.7 と
  [archive/editing/final-smart-sharpen-plan.md](archive/editing/final-smart-sharpen-plan.md) を参照)

AI 関連フィールドは「ページ / お気に入り / グローバルに保存された希望設定」であり、
実際に走るモデル範囲はアプリ全体設定 `Settings::ai_feature_mode` でさらに制限される。
`Disabled` ではアップスケール / ノイズ除去を実行しない。`Light` ではアップスケールを
`Real-ESR General V3` と `Real-CUGAN 4x` のみに制限し、ノイズ除去は実行しない。
`HighQuality` では全アップスケールモデルとノイズ除去を許可する。モード変更時も
`AdjustParams` 自体は破棄しないため、低負荷モードから高画質へ戻すと保存済みの
モデル指定が再び有効になる。

### 2.1 適用順序

色調そのものは `adjustment.rs::apply_adjustments_fast` で次の順に適用する:

```
Levels (黒点/白点/中間調) → Gamma → Brightness/Contrast → Saturation → Temperature
```

- `temperature == 0` なら u8→u8 LUT で高速処理
- `temperature != 0` なら f32 パイプライン (やや遅い)
- v1.1.0 以降のフルスクリーン通常表示では、source 解像度の edit 結果
  (`edit_result_cache`) に色調補正を掛け、その後に final AI、スマートシャープ
  (`smart_sharpen`、§2.7)、カラー化、Creative LUT、最後に `post_filter::apply` を掛けて
  `final_composite_cache` に格納する。

### 2.2 カラー化

`ColorizeParams` はモノクロ系画像の専用着色設定で、画像補正パネルの「カラー化」タブから編集する。
ページ / お気に入り / グローバル / フォルダ一括適用は他の `AdjustParams` と同じスコープを使う。
カラー化だけを保存・呼び出しできるユーザースロットを 4 個持ち、既存の補正スロット 10 個にも
`AdjustParams` の一部として含まれる。カラー化自体の初期値は OFF。有効化したときは
`MonochromeOnly` が初期選択となる。

- 標準パレット: `Legacy4Color` / `LegacySkin`。旧疑似カラーフィルタと同じ 256-entry LUT。
- カスタムパレット: 2〜10 個の「RGB 色 + 強さ」制御点。隣接点の平均強度から区間長を求め、
  強度比を通る二次補間で 256-entry LUT を生成する。
- カラー化プリセットの直下に、暗部→明部の入力グレースケールと、現在のパレット・制御点・
  強さ・「元画像の明るさを保持」を反映したカラー化結果を 2 本の階調バーで常時プレビューする。
  画像固有の輝度分布を必要とする「濃さを整える」と、近傍画素を必要とする
  スクリーントーン濃淡変換は 1 次元バーの対象外。
- 元画像の明るさを保持: LUT 色の BT.709 chroma を使い、入力輝度をどの程度再現するかを
  0〜100% で指定する。新規設定の初期値は 100%。旧ポストフィルタから移行した設定は
  見た目を維持するため 0% を明示する。
- 近モノクロ判定: サンプル RGB の主成分軸に 95% 以上の画素が収まるかで判定する。
  単純な RGB 差だけではないため、黒インクから黄ばんだ紙色へ伸びる一方向の色分布も許容する。
  UI の「色味の許容量」は主軸からの距離閾値を調整する。
- 濃さを整える: 着色対象と判定した画像だけについて、輝度ヒストグラムの上下 0.5% 点を
  黒点／白点とする自動レベル補正を行い、元輝度から補正後輝度へ寄せる強度を 0〜100% で指定する。
  初期値は 0%（補正なし）。有効範囲が 16 階調未満のほぼ単色な画像ではノイズ増幅を避けて
  補正しない。`MonochromeOnly` で対象外になったカラー画像には一切適用しない。

処理順は近モノクロ判定 → 濃さを整える → スクリーントーン濃淡変換 → 階調 LUT 着色。
スクリーントーン濃淡変換は補正後の輝度に対して行う。`検出スケール` は長辺 2048px を基準に
0.1〜4.0 を 0.1 刻みで指定し、初期値は 1.0。実際の画素半径は処理画像の長辺に比例させるため、
final AI のアップスケール結果へ処理する場合も、アップスケール前と同じ絵柄上の範囲を狙う。
実効半径は最大 64px に制限する。1px 未満の実効半径は元輝度と半径 1px の結果を補間するため、
0.1 から連続的に弱く掛けられる。周期判定や文字・線画保護の領域マスクは、トーンと線画の
境界だけが未変換の縁として残るため使用しない。復元候補は画像全体へ「変換の強さ」で直接合成する。
`Fast` は50ms級の処理を優先し、実効スケールを最寄り整数へ丸めた縮小平均画像へ3x3平均を1回適用し、
全解像度へ再拡大せず最終着色ループから直接参照する。1px 未満だけは元輝度から連続的に補間する。
`Gaussian` の小数半径は従来どおり floor / ceil の結果を補間するが、隣接半径で第1 box blur が
同一になる組み合わせはその結果を共有し、補間後の画素値を変えずに重複処理を省く。
全方式とも、LUT index 決定後の輝度保持RGBを256段階で事前計算して画素ごとの色空間演算を省く。
丸め前の輝度で毎回計算する旧経路との差は各RGBチャンネル最大1階調。

| 方式 | 処理 |
| --- | --- |
| `Off` | 元画素の輝度をそのまま LUT へ入力 |
| `Fast` / UI「高速」 | 縮小平均 + 低解像度3x3平均。スケールを整数化し、再拡大を省く高速経路 |
| `LocalMean` / UI「弱」 | 指定半径の局所平均を1回適用。網点を軽くなじませる |
| `Gaussian` / UI「強」 | 局所平均を3回重ねてガウス分布を近似。より広く滑らかに濃淡化 |

旧設定の `EdgePreserving` / `MultiScale` は読み込み時に `Gaussian` へ移行する。比較用素材は
`samples/tone-algorithm-comparison/01_source.png` にあり、長辺 2048px、検出スケール 1.0、
変換の強さ 100% で「弱」「強」を切り替えると差を確認できる。

カラー化とトーン変換は大画像で重くなるため、`final_effect_pending` のページ／viewer context
単位 worker で処理する。環境設定の「AI・カラー化の先読み」で指定した前後枚数について、
非表示ページも `final_composite_cache` まで 1 枚ずつ背景生成する。先読み開始時は白黒の
provisional texture を GPU upload せず、完成結果だけを upload するため、完成済みページへの
移動では白黒→カラーの差し替えを挟まない。先読み中の同じページを表示した場合は worker を
cancel / 再起動せず表示用へ昇格し、完成までは直前ページを holdover する。カラー化待ちのページは
生画像やサムネイルへフォールバックしない。ページ入場そのものでは完成済み final composite や
進行中 worker を無効化せず、設定変更、AI 結果到着、cache eviction のときだけ不要な背景 worker を
cancel する。`FinalCompositeKey` と items 世代が一致する完了結果だけを採用する。
`--perf-log` の `fs.final_effect_worker` はカラー化判定／適用、前後の補正段、CPU画像準備、
texture登録を分けて記録し、`scripts/analyze_perf.py <log> colorize` で解像度・方式別に集計できる。
GPU上限内の完成 `Arc<ColorImage>` は `egui::ImageData` へ同じ `Arc` を渡し、texture登録前の
全画素cloneを行わない。上限超過時だけ `clamp_for_gpu` の縮小結果を新しい `Arc` にする。
PDF の Z ズーム再描画などで表示中ページの source が高解像度版へ差し替わる場合も、差し替え直前の
完成済みカラー化 texture を display-only holdover として保持する。旧世代の live cache は通常どおり
無効化し、新 source の final composite が完成した時点で holdover を解放して置き換える。
AI 待ちの `complete=false` カラー化結果が先にできた場合は、その結果を次の holdover に昇格して
AI 完了後の再合成まで維持する。フォルダ横断の nav lock も raw / thumbnail や
`complete=false` の到着だけでは解放せず、`complete=true` の final composite を待つ。
連結読みでは単一の nav holdover を使わず、keep-set 内のページごとに直前の表示済み
カラー化 texture と items 世代を保持する。PDF の再読込や AI 再合成中もそのページだけは
旧表示を続け、`complete=true` の差し替え先を GPU 登録した後に解放する。ページが keep-set を
外れた場合は raw / final cache と同時に破棄し、表示枚数が多い場合も VRAM 上限を迂回しない。
サムネイル自体には適用しない。

### 2.2.1 Creative 3D LUT

環境設定の「表示 > LUT」で組み込みプリセットまたは `LUT_3D_SIZE` を持つユーザー `.cube` を
管理し、画像補正パネルの「フィルタ」でページごとに選択する。組み込みプリセットは
「ウォームフィルム」「クールフェード」「シネマティック (Teal & Orange)」「ヴィンテージ」
「モノクロフィルム」の 5 種類で、既存ユーザーの Settings にも `sanitize` 時に安定 UUID で
自動追加する。17³ table をアプリ内で生成するため外部ファイルへ依存せず、登録解除はできない。

ユーザー LUT の登録情報は表示名・UUID・登録元ファイルのパスを Settings に保存し、検証済みの
ファイル本体を `<data_dir>/luts/<UUID>.cube` へコピーする。通常版では
`%APPDATA%/mimageviewer/luts`、portable / `--data-dir` では各データディレクトリ配下になる。
実行時は管理コピーを優先して読み込むため、登録後に元ファイルを移動・削除しても利用できる。
旧バージョンが外部パスだけを保存した登録は `creative-lut-managed-copy` worker が管理コピーへ
移行する。組み込み table の生成、ユーザーファイルの読み込みと parse は
`creative-lut-loader` worker で行い、UI thread では完成済みの immutable table だけを参照する。
ユーザー LUT の初期表示名はファイル stem を使う。
DaVinci Resolve が複数ファイルへ同じ `TITLE "Generated by Resolve"` を書くケースがあるため、
`.cube` 内の TITLE は初期表示名に使わない。旧 importer が保存したこの汎用名は Settings の
`sanitize` 時にファイル stem へ自動修復し、ユーザーが変更した表示名は維持する。

環境設定での新規登録は `creative-lut-import` worker がサイズ上限確認、UTF-8 / 3D LUT parse、
管理コピーの作成を同じ処理内で行う。環境設定の一時 Settings とファイル所有権は
`CreativeLutTransaction` がまとめて管理し、キャンセルではその画面で作成した管理コピーを削除、
OK では新規コピーを確定して登録解除分だけを削除する。登録解除は元ファイルを削除しない。

LUT はカラー化の後、既存ポストフィルタの前に適用し、元画素との混合量を 0〜100% で指定できる。
サムネイルには反映しない。これは Log / HLG などを表示色へ変換する入力 LUT ではなく、すでに
通常表示できる画像へルックを加える Creative LUT 専用である。1D LUT は登録時に拒否する。
静止画・動画の選択 popup は最大 420 pt 高とし、多数の LUT をスクロール選択できる。静止画の
補正パネルはアプリの Light / Dark 設定にかかわらず暗色固定なので、ComboBox の popup style と
内部 widget にも暗色 Visuals を明示的に適用する。

旧 JSON の `post_filter = pseudo_color4` / `pseudo_color_skin` は読み込み時に
`ColorizeParams` へ変換し、旧挙動維持のため移行時だけ `AllImages` にする。旧キーマップ名と
ゲームパッドリング入口は互換エイリアスとして新しい標準カラー化へルーティングする。

### 2.3 ポストフィルタ (PostFilter enum)

レトロ系 (CRT ブラウン管風・機種別減色・複合) と写真系 (カラーグレーディング・アナログ・絵画風・実用)、
を扱う表示エフェクト。疑似カラーは専用カラー化へ移動した:

| グループ | バリアント | 内容 |
| --- | --- | --- |
| 基本 | `None` | フィルタなし (LINEAR サンプラー、デフォルト) |
| 基本 | `Nearest` | ピクセル補完なし (NEAREST サンプラーのみ、CPU 変換は clone) |
| CRT | `CrtSimple` | sin² スキャンライン + RGB アパーチャマスク + 微 glow |
| CRT | `CrtFull` | CrtSimple + 樽型歪み + 強 phosphor glow |
| CRT | `CrtArcade` | 太スキャンライン + 濃マスク + 高輝度 |
| 減色 (色数昇順) | `Dither1bit` | 2 階調 Bayer ディザ |
| 減色 | `GameBoy` | 緑系 4 階調 (固定) |
| 減色 | `Pc98` | PC-98 アナログモード (適応 16 色、`palette_gen::generate` で median cut) |
| 減色 | `GameGear` | ゲームギア (12bit → 32 色適応) |
| 減色 | `Famicom` | NES 固定 ~52 色ハードパレット |
| 減色 | `MegaDrive` | メガドライブ (9bit → 61 色適応、3bit/ch の階段階調) |
| 減色 | `Msx2Plus` | MSX2+ SCREEN 8 (256 色固定 GRB 3:3:2) |
| 減色 | `Sfc` | スーパーファミコン (15bit → 256 色適応) |
| 複合 | `ComboFamicomCrt` | Famicom 固定 + CRT Simple |
| 複合 | `ComboPc98Crt` | PC-98 適応 + CRT Simple |
| 複合 | `ComboMsx2PlusCrt` | MSX2+ 固定 + CRT Simple |
| 複合 | `ComboMegaDriveCrt` | メガドライブ適応 + CRT Simple |
| 複合 | `ComboSfcCrt` | スーパーファミコン適応 + CRT Simple |
| カラーグレーディング | `Sepia` | 古写真風の暖色モノクロ (Microsoft 係数マトリクス) |
| カラーグレーディング | `MonoNeutral` / `MonoCool` / `MonoWarm` | ITU-R BT.601 輝度モノクロ、冷/暖ティント版 |
| カラーグレーディング | `WarmTone` / `CoolTone` | 全体 +R/-B、-R/+B の温度補正 |
| カラーグレーディング | `TealOrange` | 影=青緑 / ハイライト=橙、シネマ調 + 彩度 +12% |
| カラーグレーディング | `KodakPortra` | 落ち着いた彩度 + 肌色 tweak + lift 5 のフィルム調 |
| カラーグレーディング | `FujiVelvia` | 青緑チャンネルゲイン + S 字 + 彩度 +35% |
| カラーグレーディング | `BleachBypass` | 彩度 -55% + S 字 0.55 の銀残しシネマ調 |
| カラーグレーディング | `CrossProcess` | 影=青緑 / ハイライト=黄、彩度 +25% |
| カラーグレーディング | `Vintage` | lift + 赤寄りシャドウ + 黄色ハイライト + 彩度 -20% |
| アナログ | `FilmGrain` | 暗部強めの粒状ノイズ (wang-hash 由来の決定論ノイズ) |
| アナログ | `Vignette` | 対角正規化距離^2 で周辺を最大 -45% 減光 |
| アナログ | `LightLeak` | 左上から対角減衰、暖色 (240,150,80) の Screen ブレンド |
| アナログ | `SoftFocus` | 明部抽出 + 分離可能ボックスブラー 11-tap + Screen blend |
| 絵画風 | `Halftone` | 6×6 セルの輝度平均 + 距離判定ドット、2 階調グレー |
| 絵画風 | `OilPaint` | 7×7 Kuwahara フィルタ (4 象限の輝度分散最小領域の平均色) |
| 絵画風 | `Sketch` | Sobel 3×3 + 強度反転グレー |
| 実用 | `Sharpen` | 5-tap 分離可能ブラーのアンシャープマスク (amount 0.6) |

**複合プリセットの方針**: **非液晶機種 (CRT TV / モニタ接続が標準)** とブラウン管フィルタを
セットにして、実機の視聴体験に近づける。GameBoy / ゲームギアは LCD なので CRT 合成は除外。

**写真系の alpha 保持**: 全フィルタが元ピクセルの alpha を `from_rgba_unmultiplied` で伝播する。
透過 PNG / WebP / GIF を通しても透過部分が不透明化しない。CRT 系は bilinear で alpha も補間
(`BilinearYCtx::sample_rgba`)、減色系・写真系は元ピクセルから単純継承。

### 標準カラー化LUTの設計方針 (`post_filter::pseudocolor`)

モノクロ漫画ビューア「マンガミーヤ」で使われていた **GiCoCu (AviSynth + GIMP トーンカーブ)
由来の「疑似四色刷り」** を再現したもの。元データは GIMP `.cur` カーブファイル (5ch ×
制御点形式) で、合成 (value) カーブを先に適用してから R/G/B カーブを当てる。実装では
**輝度 (0..255) → RGB の最終 256-entry LUT** にオフラインで畳み込んだ定数 (`C4_*` / `SKIN_*`)
を持ち、ピクセル輝度 (`pixel_lum_f32`) を index に LUT を引くだけ (`#[rustfmt::skip]` の固定表)。

- `Legacy4Color` = `4color4.cur` (影=青 / 明部=橙のクアッドトーン)
- `LegacySkin` = `c4.cur` (中間〜明部を肌色寄りの暖色に着色)
- 専用カラー化は既定で近モノクロ画像だけへ適用する。「すべての画像」を選ぶと従来どおり
  カラー入力も輝度化して duotone 的に着色する。
- 退行ガード: `post_filter::tests::pseudocolor_maps_known_gray_values` が代表点の RGB を固定
  (LUT 破損・取り違えの検知用。下記「忠実度」のとおり GiCoCu 厳密出力との一致は主張しない)。

#### どの `.cur` が「正」か (新形式に寄せない理由)

当時のコミュニティ運用では、**「疑似四色刷り "旧形式" アルファ補正付き.cur」を `4color4.cur`
にリネームして使う**のが定番だった ([MangaMeeya スレ](https://egg.5ch.net/test/read.cgi/software/1416397467/240-n))。
**"新形式" / "新形式アルファ" は無色化・フリーズの不具合が報告され避けられていた**版である。
したがって本実装の `Legacy4Color` = `4color4.cur` (= 旧形式アルファ補正付きのリネーム) が
**再現の正**であり、"新形式" の数値に寄せる修正は再現性をむしろ損なう。レビューで「新形式に
合わせるべき / 別形式と値が違う」という指摘が来ても、上記理由で**意図的に旧形式を採用**している。

#### 忠実度 — 「制御点に忠実な再現 (近似)」

LUT は元 `.cur` の制御点を **Catmull-Rom 補間**して 256-entry 化したもの。GiCoCu 本体や
GIMP の厳密なカーブ補間とは数レベル差が出うる (= 制御点には忠実だが補間は近似)。**byte-exact な
一致は設計目標ではない**。完全一致を狙う場合は GiCoCu / `ColorCurveOp` の補間アルゴリズムを
移植して再生成する必要があるが、視覚差は小さく現状は近似で運用する。

#### 出所と再生成 (provenance / 監査)

LUT 生成スクリプトと元 `.cur` は `dist/manga_pseudocolor/` にあるが **git 管理外**なので、
クリーン checkout から監査・再生成できるよう、元 `.cur` の制御点と出所を以下に転記する。

- **取得元 (GitHub、いずれも `# GIMP Curves File` の旧形式・公開ソース)**:
  - `4color4.cur` … `nalltama/RAIV` の `cur/4color4.cur` (GitHub 上 564B / LF。原本の
    旧形式アルファ補正付きは ~570B / CRLF = 改行差のみで同一)。
    再構成コピー sha256 `58e84a95ed1994a792208073fa33c1c1179e0dea7bf0b2efd7fa4971c4cee52b`。
  - `c4.cur` … `umjammer/vavi-image-sandbox` の `src/test/resources/c4.cur`。
    再構成コピー sha256 `00082a9fd873d820be3e4b8900c48f44a13b7a1917ef781e8eed8a65a26ccb1f`。
- **`4color4.cur` の制御点** (5 行 = value / R / G / B / alpha、`-1 -1` は未使用点):

  ```
  # GIMP Curves File
  0 0 -1 -1 31 11 -1 -1 63 37 -1 -1 95 80 -1 -1 127 129 -1 -1 159 179 -1 -1 191 222 -1 -1 223 246 -1 -1 255 255
  0 0 -1 -1 31 33 -1 -1 63 55 -1 -1 95 40 -1 -1 127 79 -1 -1 159 160 -1 -1 191 193 -1 -1 223 225 -1 -1 255 255
  0 0 -1 -1 31 65 -1 -1 63 57 -1 -1 95 86 -1 -1 127 114 -1 -1 159 99 -1 -1 191 114 -1 -1 223 181 -1 -1 255 255
  0 0 -1 -1 31 61 -1 -1 63 138 -1 -1 95 118 -1 -1 127 109 -1 -1 159 64 -1 -1 191 38 -1 -1 223 117 -1 -1 255 255
  0 0 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 255 255
  ```

- **`c4.cur` の制御点**:

  ```
  # GIMP Curves File
  0 0 -1 -1 -1 -1 47 11 60 18 -1 -1 98 54 -1 -1 132 101 -1 -1 -1 -1 -1 -1 185 151 212 180 -1 -1 247 222 255 255
  0 0 16 15 -1 -1 -1 -1 68 65 -1 -1 102 125 121 216 132 247 -1 -1 157 255 183 255 -1 -1 -1 -1 223 255 -1 -1 255 255
  0 0 16 18 33 33 -1 -1 63 64 -1 -1 93 91 109 103 125 140 140 163 155 180 173 202 201 229 214 238 231 248 245 253 255 255
  0 0 19 26 33 42 53 62 -1 -1 74 72 95 95 119 119 -1 -1 137 138 155 156 175 177 195 206 -1 -1 217 227 234 241 255 255
  0 0 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 255 255
  ```

- **再生成手順**: 上記制御点を value→R/G/B の順に Catmull-Rom 補間 (端点は複製) して
  gray(0..255)→RGB の 256-entry LUT を作り、`mod pseudocolor` の `C4_*` / `SKIN_*` に焼く。
  `dist/manga_pseudocolor/gen_rust_lut.py` (+ `analyze_cur.py`) がこの処理を行う。
- **ライセンス的位置づけ**: `.cur` は数値カーブデータ (制御点の羅列) で著作物性は低い。出所は
  上記公開リポジトリ。テスト/検証に使ったモノクロ原画はリポジトリには含めない。

### 減色の分類: 固定 vs 適応

- **固定パレット** (GameBoy/Famicom/MSX2+/Dither1bit): ハードウェア仕様で定義された色セットをそのまま使う。
  実機の挙動を再現するが、パレットに無い色味 (例: NES の肌色) は画像によって大きく変化する。
- **適応パレット** (PC-98/ゲームギア/メガドライブ/SFC): 実機では各ゲームが画像に合わせて色を選んで
  いた機種。median cut で画像から最適な N 色を抽出し、ハードウェアの bit 深度に
  `quantize_channel_bits` で丸める。`palette_gen::generate` がサンプル 50k 制限で median cut を行う。

### CRT 系の設計方針

**明るさ均等化**: 3 プリセットとも `brightness_boost = 1 / (scan_atten × mask_atten)` を
適用し、フィルタなしとほぼ同じ輝度になるよう補正。スキャンライン・マスクの輝度低下を
ブーストで相殺する (実機 CRT のビーム出力と同等の考え方)。CrtArcade のみ +10% で高輝度に。

**パターンの滑らかさ**: sin² falloff のスキャンライン + sin² 分布の RGB アパーチャ +
bilinear 補間ソースサンプリング + 水平ブラー (h_blur) で、「硬い矩形ピクセル」ではなく
アナログ CRT 特有の「柔らかく滲む phosphor ドット」感を出す。

**出力解像度**: 適応アップスケール (長辺 1024px 以下 → 4x / 2048px 以下 → 2x / 超 → 1x)、
出力長辺 4096px ハードキャップ。減色系はソース解像度を維持。

### サンプラー選択

- `PostFilter::Nearest` のみ `TextureOptions::NEAREST` でアップロード
- その他 (CRT/減色/複合を含む静止画) は linear + linear mipmap でアップロード。縮小時のモアレを防ぎ、CRT の phosphor
  感を出すため。NEAREST だと CRT 結果を画面スケールに合わせる際に周期的黒線が出る。

**mipmap と旧設定の互換**: v2.7.0 から GPU が静止画の完全な mip chain を自動生成するため、
手動回避策だった `Downscale2x` / `Downscale4x` は撤去した。保存済み設定にこれらが含まれる場合は
`PostFilter::None` として読み込む。サムネイル、animated image、動画、mask、`Nearest` は mipmap
対象外。実装詳細は [downscale-moire-lod-plan.md](downscale-moire-lod-plan.md) を参照。

**並列化**: `rayon::par_chunks_mut` で行単位に並列処理。4K 画像でも 4080ms 程度。

### 2.4 カラー化・Creative LUT・ポストフィルタの一時バイパス

消しゴム / 隠蔽加工 / 分析モード中は `App::post_filter_bypassed: bool` が `true` になり、
final composite の `params_hash` から `colorize` / `creative_lut` / `post_filter` を外す。
モード解除時に false に戻し、
該当 idx の final pipeline cache だけをクリアして最終表示エフェクト適用状態で再生成させる。
これは編集時の見やすさだけの切替なので、source 解像度の edit cache や
`input_generation` は進めない。

- **消しゴム**: 減色プリセット (GameBoy 4 色など) が有効だと境界が潰れてマスクを精密に塗れないため
  編集表示だけカラー化・Creative LUT・post-filter を外す。MI-GAN の preview / apply / auto-apply /
  ensure-result 入力も source 解像度の edit pipeline から取り、post-filter や final AI の
  解像度変更には引きずられない。
- **分析**: ヒストグラムは `fs_cache` の生ピクセルから計算されるため、表示だけを生に揃える

`AdjustParams` には:
- `is_identity()` = 色調 identity **かつ** カラー化無効 **かつ**
  Creative LUT 無効 **かつ** `post_filter == None` **かつ** `smart_sharpen == 0`
- `is_color_identity()` = 色調 identity のみ (バイパス中の早期 return 判定用)

### 2.5 元画像プレビュー

右 Ctrl を押している間だけ、補正 / ポストフィルタ / AI アップスケール・デノイズ /
消しゴム補完結果 / 隠蔽加工を表示選択から外し、元画像を一時表示する。これは「比較用の描画 override」
であり、`edit_result_cache` / final pipeline cache の無効化、DB 書き換え、AI ジョブの停止は
行わない。表示元は常に raw 専用の `fs_cache` で、消しゴム補完済みページでも
`erase_base_cache` は参照しない。

### 2.6 Auto モード

- **Auto**: ヒストグラムの 0.5/99.5 パーセンタイルでレベル補正
- **MangaCleanup**: 紙/インク検出 → グレースケール → S 字カーブ → γ=0.85 → コントラスト ≥15

### 2.7 最終表示段スマートシャープ (`smart_sharpen`、v1.3.0)

設計の出所は [archive/editing/final-smart-sharpen-plan.md](archive/editing/final-smart-sharpen-plan.md)。

- **UI**: 補正パネルのポストフィルタの上に `シャープ化 0..=100` の 1 本スライダー。
  詳細パラメータ (半径 / 輪郭しきい値 / ハロー抑制) は出さない。
- **内部マッピング**: `adjustment::smart_sharpen_params_for_strength` が強度から
  `local_adjust_core::SmartSharpenParams` を生成 (アンカー 0/25/50/75/100 の線形補間。
  100 = amount 2.0 / 半径 3.0 = 計算式の clamp 上限。当初は 30/60/100 配置で最大
  amount 1.25 だったが控えめだったため、未リリースのうちに上へ拡張した。
  目安: 25 弱め / 50 標準 / 75 強め / 100 最大)。
  本体は `local_adjust_core::apply_smart_sharpen_rgba` (補正レイヤーの
  `LocalEffect::SmartSharpen` と同じ計算式、rayon 行並列、radius は 3.0 に clamp)。
- **AI アップスケール実行時のスキップ (固定動作、設定なし)**:
  アップスケールモデルの出力は既に輪郭強調済みのことが多く、二重シャープで見た目が
  悪化しやすいため、アップスケール出力には**常に掛けない**。パネルにはサイズ上限の
  無効表示と同じ形式で「（AI アップスケール実行時は適用されません）」の注記を出す。
  当初はチェックボックス (`smart_sharpen_skip_after_ai`、既定 ON) で切替可能にして
  いたが、「強度 0 のとき意味を持たないフラグが個別設定 (補バッジ / DB 行) として
  残る」問題が、保存時正規化 (チェックが勝手に戻る) とも disabled 化 (操作不能) とも
  UX が両立せず、固定動作に変更した (2026-06-10 ユーザー判断。AI 後に追いシャープ
  したい要望が出たら別途検討)。判定は
  「設定が AI ON か」ではなく **合成ベースが upscaler 経由の AI 結果か**。
  `final_ai_cache` / retained entry の `used_upscale` を参照するため、render-to-target で
  出力サイズが入力と同じになった場合も正しくスキップできる。したがって:
  - デノイズのみの AI 結果 (サイズ不変) には通常どおり掛かる
  - サイズ上限 (`ai_upscale_size_limit`、長辺 x 短辺) で AI がスキップされた
    ページにも掛かる
  - final composite の AI 未完了中の暫定合成 (complete=false、非 AI 画像) には掛かり、AI 完了時の
    再合成で外れる (= complete フラグの再合成機構にそのまま乗る)
  - legacy の `apply_sync_adjustment` / `adjustment_cache` 経路にはスマートシャープを
    焼き込まない。final composite 完了までの暫定表示用 cache に upscaler 実行有無の
    判定を持ち込まず、final composite 側だけで適用する
- **適用位置**: final pipeline の
  `色調補正 → final AI → スマートシャープ → カラー化 → Creative LUT → post_filter`。
  AI 入力には掛けないので、強度変更で final AI は再実行されない
  (`hash_adjust_final_params` には乗るが `hash_adjust_color_ai_params` には乗せない)。
  単ページ書き換え経路 (スライダー / T キーのポストフィルタ循環 / スロット適用 /
  見開き L/R コピー) は `App::clear_caches_for_param_change(idx, old, new)` で差分を
  分類し、「final 専用項目 (post_filter / smart_sharpen) だけの変更」なら
  `clear_final_stage_only_caches` で `final_ai_cache` / pending を保持する (§4 の表参照)。
  bulk 系 (全画像に適用 / 標準にする / お気に入り標準) は従来どおり
  `clear_all_color_caches` で final AI ごと全クリアされる (既知の過剰クリア。
  ワンショット操作なので許容)。
- **サムネ無効化の色調 gate**: `set_page_params` / `clear_page_params` は
  `color_settings_eq` (brightness..midtone + auto_mode) が変わるときだけ
  `thumb_adjust_tex` を落とす。スライダードラッグ release の全クリアも
  `thumb_adjust_drag_color_dirty` (ドラッグ中に色調が動いたときだけ立つ) で gate される。
  シャープ化 / post_filter のみの変更・ドラッグではサムネ補正を再生成しない。
- **サムネイル非反映**: `is_color_identity()` には参加しないため、`thumb_adjust_tex` の
  生成判定・内容に影響しない。
- **post_filter バイパス (消しゴム / 隠蔽 / 分析) 中も final composite では適用したまま**
  (色調補正と同じ扱い)。legacy `adjustment_cache` には焼き込まず、消しゴム / MI-GAN /
  補正レイヤーの入力は source 解像度の edit pipeline から取るので、シャープ結果が
  編集系の入力へ混入することはない。
- **コピー / 書き出し**: `prepare_capture_pixel_job` が final composite pixels を取得して
  `capture::run_pixel_job` へ渡すため、表示どおりに反映される。capture worker は
  AdjustParams / AI / final filter を再実行せず、出力専用の conceal / crop / rotation のみを扱う。
- **alpha**: unmultiplied RGBA に展開して RGB のみ強調 → 再 premultiply。透明部に
  hidden RGB を作らない (`adjustment::apply_final_smart_sharpen`)。
- **360 パノラマ settle**: settle 再レンダリングはシャープ化を再現しないため、
  `smart_sharpen != 0` のページは post_filter と同様 settle Disabled。

---

## 3. キャッシュ構造 (フルスクリーン時)

| キャッシュ | 型 | 内容 |
| --- | --- | --- |
| `fs_cache` | `HashMap<idx, FsCacheEntry>` | 生デコード結果。Static / Animated / Failed |
| `erase_result_cache` | `HashMap<EraseResultKey, EraseResultCacheEntry>` | 消しゴム MI-GAN 確定結果。`idx + input_generation + erase_mask_generation` で識別 |
| `local_adjust_cache` | `HashMap<LocalAdjustResultKey, LocalAdjustCacheEntry>` | 補正レイヤー合成結果。`idx + input_generation + erase_mask_generation + local_adjust_generation` で識別 |
| `conceal_cache` | `HashMap<idx, ConcealCacheEntry>` | 隠蔽加工合成済みテクスチャ |
| `edit_result_cache` | `HashMap<EditResultKey, EditResultEntry>` | `raw -> erase -> local_adjust -> conceal` の source 解像度 edit 結果。AdjustParams / AI / post_filter / crop は含めない |
| `final_ai_cache` | `HashMap<FinalAiKey, FinalAiEntry>` | 色調補正後の edit 結果へ AI アップスケール / デノイズを適用した結果。`FinalAiEntry` は `pixels` と smart sharpen 判定用の `used_upscale` を持つ |
| `retained_final_ai_cache` | `HashMap<RetainedFinalAiKey, RetainedFinalAiEntry>` | fullscreen session をまたいで保持する final AI entry。`metadata_cache_key(idx) + edit_size + color_ai_hash + bg` で識別し、PDF retained page cache と合算した枚数 / MiB の LRU で退去。PDF display job は session close / keep-set eviction 時に最大 1 件だけ retained store 目的で完走を許可できる |
| PDF retained page cache | `HashMap<item_key, Raster \| FinalAi>` | PDF ページ専用の保持スロット。PDF レンダリング後は `Raster`、final AI 完了後は同じスロットを `FinalAi` に昇格するため、同一ページのラスタ結果とAI結果を二重保持しない。`FinalAi` は `pixels` と `used_upscale` を保持する。容量は `retained_final_ai_cache` と同じ設定枠に合算する。`FinalAi` hit 時は raw PDF レンダリングを待たずに final composite を直接復元できる |
| `final_composite_cache` | `HashMap<FinalCompositeKey, FinalCompositeEntry>` | edit 結果に AdjustParams の色調、final AI、スマートシャープ、カラー化、Creative LUT、post_filter を適用した通常表示用テクスチャ |

描画時 ([display-pipeline.md](display-pipeline.md) を参照) は:

```
final_composite_cache > edit_result_cache > fs_cache
```

が通常表示の基本。erase / local_adjust / conceal の編集中だけ、各 UI が source 解像度の
プレビュー (`erase_preview_cache`, `local_adjust_cache`, `conceal_cache` など) を
一時的に表示する。
隠蔽加工のプレビュー合成は `current_conceal_source_pixels` で
`local_adjust > erase_result > fs_cache` を毎回解決する。
`conceal_base_cache` は隠蔽モード入場時ソースの退避で、編集中ページの現行ソースを
一時的に取得できない場合のフォールバックとしてだけ使う。AI アップスケール完了などで
final 表示解像度が変わっても、source 解像度の edit cache とマスク寸法は変えない。

### 3.1 edit pipeline の適用タイミング

`App::ensure_edit_result_pixels(ctx, idx)` が必要に応じて以下を lazy に解決し、
`edit_result_cache` に載せる:

1. `fs_cache` の raw pixels
2. 消しゴムマスクがあれば `erase_result_cache`
3. 補正レイヤーがあれば `local_adjust_cache`
4. 隠蔽マスク / プレビューがあれば `conceal_cache`

この段階では色調補正 / final AI / スマートシャープ / カラー化 / Creative LUT /
post_filter を一切適用しない。色調スライダーの
drag 中でも `edit_result_cache` は再利用されるため、マスクやブラシ stroke の重い再計算が走らない。

local / conceal の DB load、decode、raster、compose は非同期境界である。
`assemble_edit_result_pixels` は必要な materialization を起動するが、UI thread では待たない。
active local の現在世代 pixels、または必要な conceal result がまだ無ければ `None` を返し、
未完成の入力から `edit_result_cache` を作らない。`ensure_final_composite_texture` も edit result の
`None` をそのまま伝播し、不完全な final assembly を開始しない。

表示 resolver は pending 中だけ下位レイヤーへフォールバックする。local 編集中は
`erase_result_cache > fs_cache`、conceal は active local の完成を待ってから合成し、local を飛ばして
erase / raw 上へ conceal を載せない。通常表示で edit / final が未完成なら raw 表示へ落ち、worker
完了後に最新 generation の上位 cache へ差し替える。capture / export はこの表示用下位レイヤーを
完成 edit result として保存せず、必要な materialization 完了後に再開する。

**crop は表示パイプラインに含めない** (通常表示は crop 外を暗くする overlay のみで、
画像そのものは切り取らない)。実際の切り出しは Ctrl+S コピー / Ctrl+E 書き出しの最終段で
`App::export_crop_rect_for_pixels` を使い、final composite (AI アップスケール後で source と
サイズが違いうる) のピクセル座標へ crop 矩形をスケールして適用する。したがって crop の
変更は `edit_result_cache` / final cache を無効化しない (無効化すると crop ドラッグのたびに
AI を無駄に再実行してしまう)。

### 3.2 final pipeline の適用タイミング

`App::ensure_final_composite_texture(ctx, idx)` が `edit_result_cache` を入力にして:

1. 色調補正 (`apply_adjustments_fast`)
2. final AI アップスケール / デノイズ (`final_ai_pending` → `final_ai_cache`)
3. スマートシャープ (`apply_final_smart_sharpen`、`smart_sharpen != 0` のときだけ。§2.7)
4. カラー化 (`colorize::apply_applicable_with_cancel`)
5. Creative LUT (`creative_lut::apply_to_color_image`)
6. post_filter (`post_filter::apply`)
7. GPU upload (`final_composite_cache`)

の順で最終表示を作る。AI 未完了中は色調補正済み (+シャープ化済み) の画像を暫定表示し、
AI 完了時に未完了の final composite を捨てて AI 後の画像へ掛け直して再合成する。
`final_ai_cache` が miss しても `retained_final_ai_cache` が hit した場合は、その entry を
`final_ai_cache` に戻してから同じ合成経路に入る。保持 LRU は `close_fullscreen()` や
keep-set eviction では消さず、AI 入力が変わる編集 (`clear_final_pipeline_caches_for_idx`)
や AI 機能モード / サイズ上限変更で破棄する。fullscreen close / reopen や PDF の表示解像度更新に伴う
同じ item の `fs_cache` 再ロードは `bump_input_generation_for_fs_cache_reload` で live cache だけを
無効化し、保持 LRU は残す。表示中のカラー化済みページなら、無効化前の texture は新しい final
composite が完成するまで display-only holdover として使う。PDF ページは
`retained_final_ai_cache` へ重複保存せず、PDF 専用の
ページ保持スロットで `Raster` と `FinalAi` を同じ容量枠として扱う。PDF レンダリング後は
`Raster` を保持し、final AI 完了後は同じ item_key のスロットを `FinalAi` に昇格して
ラスタ結果を解放する。PDF ページ保持スロットと通常の保持AIは、同じ設定値
(`retained_final_ai_cache_max_entries` / `retained_final_ai_cache_max_mib`) の合算 LRU で
退去する。`FinalAi` が現在の `edit_size + color_ai_hash + bg` と一致する場合は、`fs_cache` の
raw PDF pixels を待たずに final composite を直接復元する。PDF ページの display
final AI は、完了前に live 表示セッションから外れても retained key と retained epoch を job に
持たせておき、最大 1 件だけ cancel せず完走させられる。この orphan result は保持スロットにだけ
store し、idx ベースの live `final_ai_cache` には戻さない。外部変更や AI 設定変更で retained
epoch が進んでいた結果は store 時に捨てる。保持 LRU の `store` / `hit` / `miss` /
`skip` / `evict` / `clear` は通常ログ (`mimageviewer.log`) に出るため、同じページへ
戻ったときに推論再実行ではなく保持結果の復元だったか、または保持前に破棄されたかを
後から確認できる。PDF ページ保持スロットは `[PDF] Retained page ...` としてログに出る。

### 3.3 サムネイル補正

サムネイルは `thumb_pixels` から色調補正のみを同期適用し、`thumb_adjust_tex` に保持する。
環境設定の編集プレビューキャッシュが有効なら、erase / local_adjust / conceal / crop 済みの下地と、
crop 済み comic 注釈ラスターレイヤーを分離して読み込む。色調補正は下地だけへ表示時に適用し、
その後で注釈を合成する。post_filter と final AI はサムネでは実行しない。スライダー drag 中は
補正サムネ生成を止め、release 後に visible 優先で再生成する。

### 3.4 補正レイヤーの適用タイミング

補正レイヤーは `local-adjust-core` を使う CPU 処理で、効果によっては重いため別スレッドで適用する。
入力は `erase_result_cache > fs_cache` の順で解決し、
結果を `local_adjust_cache` に格納する。消しゴムマスクが存在するが現在世代の
`erase_result_cache` がまだ無い場合は、古い補正レイヤー結果を表示せず、消しゴム結果の生成を待つ。
フォルダロード / グリッド表示では `local_adjust_pages` の存在判定だけを使い、
`layers_json` の実体はフルスクリーン表示、補正レイヤーパネル、エクスポート準備など実際に
必要になったページだけ `local-adjust-render` worker で読む。worker が DB read / JSON decode /
mask resize / compose を行い、UI は現在の `items_generation` と `LocalAdjustResultKey` に一致する
完成結果だけを 1 フレーム 1 枚で upload する。

生成中または stale の間は、下位レイヤの画像をそのまま表示する。これにより補正レイヤーの古い結果が
一瞬残ることを避け、非同期完了後に最新の `local_adjust_cache` へ差し替える。サムネイルには
補正レイヤーを反映しない。
手描きブラシ stroke 中は in-memory レイヤーだけを即時更新し、重い worker 再合成は 150ms の
idle まで遅延する。release 時は遅延をキャンセルして 1 回だけ generation を進める。

初期 UI はフルスクリーン左パネルのヘッダーにある「補正レイヤー」アイコンから開く。
ヘッダーの編集入口は左から `消しゴム -> 補正レイヤー -> 隠蔽加工 -> エクスポート` の順で、
補正レイヤーは消しゴム / 隠蔽加工と同じ右上 `×` 付きの独立左パネルを開き、
`local_adjust_lab` と同じく右側に選択レイヤー用のツール / マスク / 効果パラメータパネルを並べる。
通常 F12 の linked 別ウィンドウではこの編集入口を従来どおり使えるが、
「画像/動画を別ウィンドウで開く」設定で開いた always-new 窓では編集入口を無効化する。
全体補正 / ポストフィルタ / AI 表示設定は表示調整として引き続き利用できる。
パネルには効果ピッカー検索、全 `LocalEffect` のパラメータ UI、効果コピー/ペースト/リセット、
マスク種別切替、グラデーション/カラー範囲/手描きマスク編集を置く。プレビューは自動反映のため
手動プレビュー用アイコンは置かない。3D LUT の `.cube` 読み込みは UI スレッドで読まず、
`local-adjust-lut-load` worker で読み込んで完了時にレイヤーへ反映する。

`LocalAdjustmentLayer` の永続 schema では、画像ピクセル単位の長さパラメータを
必ず `_px` で終わるフィールド名にする。編集内容を異なる画像サイズへ貼り付ける際は、
`edit_bundle` がこの接尾辞をスキーマ契約として検出・拡縮する。正規化座標、比率、角度には
`_px` を使わない。変換時はラスターマスクの画素配列と 3D LUT table だけを
`serde_json::Value` 往復から一時退避し、メタデータの `_px` 変換後に復元・再サンプルする。

画像上のキャンバス操作は以下を扱う:

- `LinearGradient` / `RadialGradient`: 画像上ドラッグで開始/終了点または中心/半径を更新する。
- `ColorRange`: 画像クリックで対象 RGB を拾う。
- `SelectiveColor` と各種 RGB パラメータ: 効果 UI のスポイトボタンから画像クリックで色を拾う。
- `Repair`: 基準色は指定半径の不透明画素を平均してスポイトで拾う。`Clone` では
  コピー元 / 塗り先基準点をそれぞれ画像上のクリックで指定し、固定オフセットを作る。
- 位置を持つ効果 (`TiltShift` / `RadialBlur` / `LensFlare` / `Spotlight` など): 位置ハンドル表示中に画像上のハンドルをドラッグする。
- 手描きマスク: `RasterVector` はベースマスクを直接編集し、その他のマスクでは追加/削除マスクを明示的に開いたときだけブラシ入力が有効になる。ドラッグ中は in-memory 更新、release 時に DB 保存する。

#### 修復／塗り効果 (v2.5.0)

`LocalEffect::Repair` は必ず解決済みマスクとレイヤ不透明度の範囲内だけへ適用する。
汎用効果の「マスク適用 前 / 後」切替によって修復結果がマスク外へ逃げない。
修復レイヤーではマスクの `ぼかし境界` を `境界なじませ` と表示し、修復元の探索 / パッチ配置には
ぼかす前のマスク、最終合成にはぼかしたマスクを使う。これにより境界をなじませても生成テクスチャや
参照元は変化しない。なじませ幅を外側へ確保したい場合は、先に `拡張/縮小` で修復範囲を広げる。

- `Solid`: スポイト色の RGB へ置き換える。
- `PreserveLuminance`: スポイト色の色相 / 彩度と入力画素の HSL 輝度を組み合わせる。
- `Surrounding`: マスク外の小パッチを境界から内側へ選択 / 複写し、重なり領域を連続ウェイトで
  合成してパッチ境界の段差を抑えた後、周囲の平均色とコントラストへ寄せる。処理品質は候補数、
  `RepairPatchSize` はパッチ寸法を独立して切り替える。`Auto` (既定) は従来どおり品質に応じた
  10〜14px、`Standard` は 24px、`Large` は 48px。`seed` で別候補を再現可能にする。
- `Clone`: 正規化座標で保存したコピー元 / 塗り先基準点の差をマスク全体へ適用し、入力画像を
  bilinear sample する。

周囲修復とクローンの重い処理は既存の `local-adjust-render` worker 内で行う。品質で候補数を上限付きにし、
一定タイルごとに cancel flag と進捗を確認する。専用の AI session や別のピクセルキャッシュは持たず、既存の
`local_adjust_generation` / `local_adjust_cache` の無効化境界に従う。

補正レイヤー設定があるページには `局` バッジを表示する。環境設定の「編集結果をサムネイル一覧に
保持する」が有効なら、フルスクリーン編集を閉じた時点または別ページへ移動する直前の
`edit_result_cache` とテキスト／スタンプの注釈ラスターレイヤーへ同じ crop を適用し、下地 WebP と
lossless 注釈 WebP に分けて編集プレビューキャッシュへ非同期保存する。表示時は下地へだけ色調補正を
掛けた後で注釈を合成し、fullscreen と同じ処理順にする。final AI / スマートシャープ /
カラー化 / Creative LUT / post-filter は含めない。

Ctrl+E とキャプチャ保存は、補正レイヤーが有効なページでは `local_adjust_cache` が揃ってから
実行する。表示中の暫定フォールバック画像をそのまま保存しない。

---

## 4. キャッシュ無効化ルール (早見表)

**これを間違えると高確率でバグる**。変更する前に必ず以下を確認:

| 変更された内容 | `edit_result_cache` | `final_ai_cache` / `final_composite_cache` | `thumb_adjust_tex` | pending |
| --- | --- | --- | --- | --- |
| 色系パラメータ変更* (ページ個別) | 残す | 該当 idx の final cache をクリア | 該当 idx のみクリア | final AI は該当 idx をキャンセル |
| **final 専用項目 (ポストフィルタ / シャープ化) のみ変更** (ページ個別。パネル / T キー循環 / スロット / 見開きコピー) | 残す | 該当 idx の final **composite** のみクリア (`clear_final_stage_only_caches`)。**final AI cache / pending は保持** (AI 入力不変、Codex P1/P3 2026-06-10) | 触らない (サムネ非対象、`is_color_identity` にも不参加) | final AI は触らない |
| AI モデル変更 (ページ個別) | 残す | 該当 idx の final cache / pending / failed をクリア | 触らない (サムネ非対象) | final AI をキャンセル |
| 消しゴム/隠蔽加工/分析モードの入出 (`post_filter_bypassed` 切替) | 残す | 該当 idx の final cache のみクリア (`input_generation` は進めない) | 触らない | final AI は該当 idx をキャンセル |
| 保存スロット読込 → 現ページに適用 | 残す | `clear_caches_for_param_change` で差分分類 (シャープのみなら final AI 保持) | 色調が変わるときだけ該当 idx をクリア | AI 設定が変われば final AI キャンセル |
| 「全画像に適用」 / 「全画像から削除」 | 残す | final cache を対象範囲でクリア | **全クリア** | AI 設定が変わる idx の final AI をキャンセル |
| 「標準にする」 (global_preset 更新) | 残す | final cache を継承ページ中心にクリア | **全クリア** | AI 設定が変わる idx の final AI をキャンセル |
| 「個別設定を解除」 (Ctrl+Backspace) | 残す | 該当 idx の final cache をクリア | 該当 idx のみクリア | AI 設定が変われば final AI キャンセル |
| スライダードラッグ中 | 残す | 毎フレーム final composite のみ再生成 | **抑制** (描画時 `adjusted_tex = None`) | edit 系 pending は触らない |
| スライダー release (true→false 遷移) | 残す | (変化なし) | ドラッグ中に色調が動いたときだけ**全クリア** → visible 優先で再生成 (`thumb_adjust_drag_color_dirty`)。シャープ化のみのドラッグでは温存 | — |
| フォルダ切替 | 全クリア | 全クリア | **全クリア** + `thumb_pixels` も全クリア | pending をキャンセル |
| keep_range からの eviction | 該当 idx の edit/final を evict | 該当 idx の final を evict | 該当 idx のみクリア + `thumb_pixels` も drop | 対象外 |
| 回転変更 | **クリアしない** (描画時の GPU 行列で回転) | **クリアしない** (同左) | **クリアしない** | — |
| 消しゴムマスク変更 | 該当 idx をクリア | 該当 idx をクリア | 永続編集 preview を非同期削除し、完了通知で該当サムネイルも Evicted。編集終了時に再生成 | erase/local/conceal/final pending をキャンセル |
| 補正レイヤー変更 | 該当 idx をクリア | 該当 idx をクリア | 永続編集 preview を非同期削除し、完了通知で該当サムネイルも Evicted。編集終了時に再生成 | local / downstream conceal / final pending をキャンセル |
| 隠蔽加工変更 | 該当 idx をクリア | 該当 idx をクリア | 永続編集 preview を非同期削除し、完了通知で該当サムネイルも Evicted。編集終了時に再生成 | conceal materialization / final pending をキャンセル |
| crop 変更 | **クリアしない** (表示は overlay のみ) | **クリアしない** | 永続 edit preview WebP のみ非同期削除し、編集終了後に crop 済み preview を再生成 | **触らない** (AI を無駄にキャンセルしない) |

*「色系」= brightness/contrast/gamma/saturation/temperature/levels/auto_mode
(ポストフィルタ / シャープ化は final 専用項目で、単独変更なら上記の
`clear_final_stage_only_caches` 行の扱い。色系と同時に変わった場合は色系変更として扱う)

`retained_final_ai_cache` は、上表で `final_ai_cache` をクリアする idx 単位の変更では同じ
ページキーの entry を削除する。`clear_all_final_pipeline_caches()` は fullscreen close /
folder nav close でも呼ばれるため保持 LRU には触らない。AI 機能モードや AI 処理サイズ上限の
ように全体の実行判定が変わる設定変更では、保持 LRU も全クリアする。フォーカス復帰などで
現在フォルダの実ディスク内容変更を signature 差分として検出した場合も、同じ path / 同じ
画像寸法の上書き差し替えで旧 AI pixels を流用しないよう保持 LRU を全クリアする。
`poll_prefetch` の raw decode 取り込みは `fs_cache` の再構築なので、live cache
(`edit_result_cache` / `final_ai_cache` / `final_composite_cache`) は旧世代分を捨てるが、
session またぎの retained final AI は残す。

消しゴムマスク変更時は `erase_mask_generation[idx]` を進め、`erase_result_cache` と
`local_adjust_cache` / `conceal_cache[idx]` / `edit_result_cache` / final cache を stale 化する。
`fs_cache` は下位入力として保持し、MI-GAN 結果と補正レイヤー結果だけを再生成する。

補正レイヤー変更時は `local_adjust_generation[idx]` を進め、`local_adjust_cache` と
下流の `conceal_cache[idx]` / `edit_result_cache` / final cache を stale 化する。
`erase_result_cache` は
下位入力として保持する。生成中または stale 中は下位入力を表示し、完了後に
`local_adjust_cache` へ差し替える。

### 4.1 ヘルパー関数

`App` には final pipeline 用の無効化ヘルパーがある:

```rust
fn clear_edit_result_caches_for_idx(&mut self, idx: usize)
    // source 解像度 edit result と、その下流 final pipeline をクリア

fn clear_final_pipeline_caches_for_idx(&mut self, idx: usize)
    // final_ai_pending / final_ai_cache / final_ai_failed / final_composite_cache を idx 単位でクリア

fn clear_final_stage_only_caches(&mut self, idx: usize)
    // final composite (+ post_filter 用 legacy adjustment_cache / comic) のみクリア、final AI は保持。
    // post_filter / smart_sharpen のような final 専用項目だけが変わったとき用
    // (clear_caches_for_param_change が differs_only_in_final_stage で振り分ける)

fn clear_all_final_pipeline_caches(&mut self)
    // final AI pending をキャンセルし、final AI / final composite を全クリア
```

AdjustParams / AI / post_filter だけが変わる操作は `clear_final_pipeline_caches_for_idx`、
edit 系 (erase / local_adjust / conceal) が変わる操作は `clear_edit_result_caches_for_idx`
を使う。final cache は edit cache の下流なので、edit 側を落とすと final 側も必ず落ちる。
crop は表示パイプライン外 (save 時のみ適用) なので、crop 変更ではこれらを呼ばない。

`set_page_params` / `clear_page_params` / `apply_params_to_all_pages` /
`clear_all_page_params` / `copy_params_to_global` の実装内でも、必要に応じて
final pipeline cache と `thumb_adjust_tex` をクリアしている (全クリア vs 部分クリア)。
詳細はソース参照。

特に `clear_page_params(idx)` は、削除後の effective params を見て
**old.ai_settings_eq(new) が false なら** その `idx` の final AI cache /
failed / pending をクリアする。これがないと
「個別で AI OFF にしていたページから個別を解除しても、グローバルの AI が
再実行されない」という不具合になる (実際、`ui_fullscreen.rs` から
`Ctrl+Backspace` で解除した直後に上記不具合が発生していた)。

同じ考え方を bulk / global / favorite 系にも横展開している:
- `apply_params_to_all_pages(params)`: 書換前の各 idx の effective params と
  `params` を `ai_settings_eq` で比較し、一致しない idx だけ final AI cache を落とす。
- `clear_all_page_params()`: 個別削除後の effective params は `effective_default_for_idx`
  (お気に入り標準 or global_preset) になるため、書換前の effective params とその標準を
  比較して差がある idx だけ落とす。
- `copy_params_to_global(params)`: 旧 global と新 `params` を比較し、AI 設定が
  変わった場合のみ「個別 / お気に入り標準のどちらも持たない (= global 継承) 画像ページ」を
  対象に落とす。個別やお気に入り標準を持つページは effective params が変わらないので触らない。
- `set_favorite_default(fav_id, params)`: 旧そのお気に入り標準 (無ければ global) と新 `params`
  を比較し、AI 設定が変わった場合のみ「そのお気に入り傘下かつ個別を持たないページ」を対象に落とす。
- `clear_favorite_default(fav_id)`: 削除後は global にフォールバックするため、旧そのお気に入り
  標準と global を比較して差がある場合、同じ対象範囲で落とす。

---

## 5. 消しゴム (Erase) との関係

製本追加の headless bake (`BookPageSource::Composited` / `BakedEditSnapshot`) も同じ早期
erase 契約を使う。UI thread で `mask.db` の bitmap + shapes と AI runtime/model manager を
snapshot し、book worker が raw decode を必要なら黒で不透明化してから保存マスクをラスタライズし、
MI-GAN を再推論する。erase 完了後にだけ local_adjust → conceal → adjustment の下流を合成するため、
erase mask があるページを未消去 base のまま部分焼き込みすることはない。製本では global AI
upscale / denoise に加えて、表示専用の smart sharpen / colorize / Creative LUT /
post-filter も除外する。
MI-GAN が利用できず diffusion fallback になった場合は処理を継続し、追加完了トーストで通知する。

`ui_erase.rs` と `mask_db.rs` で実装された消しゴム機能は、補正パイプラインと連携している:

```
生画像 (fs_cache)
    │
    ├─ mask_db (消しゴムマスク) ─▶ MI-GAN で inpaint ─▶ erase_result_cache
    │                                                       │
    ▼                                                       ▼
local_adjust_cache ─▶ conceal_cache ─▶ edit_result_cache
                                           │
                                           ▼
                         色調補正 ─▶ final AI ─▶ smart sharpen ─▶ colorize
                                                        │
                                                        ▼
                                      Creative LUT ─▶ post_filter ─▶ final_composite_cache ─▶ 画面
                                                                       │
                                                                       ▼
                                     (Ctrl+S / Ctrl+E) crop で切り出し ─▶ 保存
                                           │
                                           └─ (編集 preview) 下地／注釈へ crop
                                                                  ├─▶ 下地 WebP
                                                                  └─▶ 注釈 lossless WebP
```

crop はフルスクリーン通常表示には反映されず (crop 外を暗くする overlay のみ)、保存 / 書き出し時に
final composite を切り出す最終段として働く。編集プレビューでは edit-result と各注釈レイヤーを同じ
矩形で切り出すため、一覧のサムネイルとアスペクト比には反映される。

`fs_cache` は raw decode 専用で、消しゴム確定結果を書き戻さない。マスクが存在する画像は
表示時に `ensure_erase_result_texture` が source 解像度の raw 入力
(`fs_cache`) と保存マスクから inpaint を非同期起動し、
結果を `erase_result_cache` に載せる。入力またはマスクが変わると generation key が変わり、
古い MI-GAN 結果は採用されない。

### MI-GAN の原寸入力と段階補完 (v2.5.0)

MI-GAN の入力は 512x512 固定だが、512px 未満の mask bbox + context を縦横別倍率で
引き伸ばさない。画像が各軸 512px 以上なら、bbox 周囲を画像内で原寸 512px 以上まで広げ、
512px 未満の画像だけを中央 letterbox して余白を hole として扱う。マスク画素は mask channel と
同じ source index の RGB を必ず 0 にするため、原画像の被写体画素は推論入力へ渡さない。

マスクは実在する非マスク画素からの 4-neighbor 距離を求め、48px ごとの帯に分けて外側から
段階的に補完する。画像外周そのものは既知画素とはみなさない。各段では未修復領域全体を
MI-GAN 入力上の hole とし、現在の帯だけを結果へ採用する。確定済みの帯は次段で既知画素として
使うため、大きい単一 hole の中央を周囲情報なしで一度に生成しない。保存形式は従来どおり
bitmap + shapes の 1 マスクで、段階は派生処理なので DB / sidecar へ保存しない。

段階補完はマスクの深さに応じて複数回の推論になるため、処理中は一時トーストとは別に
`AI補完中 3/12（タイル 2/4）` のような持続ステータスを表示する。パス / タイルの更新間も
インジケーターを動かし、preview / commit / 保存済みマスクの自動再生成の全経路を対象にする。
推論時間のばらつきが大きいため、残り時間は表示しない。

この生成規約変更前の焼き込みを一覧へ残さないため、編集プレビューキャッシュは format v3 で
一度再生成する。

### 透過画像の扱い (黒で不透明化)

MI-GAN は RGB 専用で alpha を扱えない。透過 PNG / WebP 等は透明部が premultiplied RGB=0
(= 黒) で格納されるため、そのまま渡すとモデルが透明部を「黒」として補完入力に使い、補完結果も
黒くなる。混乱を避けるため、消しゴム作業ベースは**黒で不透明化**して統一する:

- `App::black_flatten_if_transparent` (`ui_erase.rs`): 1 画素でも alpha<255 があれば、
  premultiplied RGB を保ったまま alpha=255 にした不透明コピーを返す (= 黒背景への合成と等価。
  全不透明なら `None` で no-op)。
- 適用点は 2 つ: `enter_erase_mode` の `erase_base_cache` 初回保存 (表示 =
  `ensure_erase_base_texture`) と、`resolve_erase_input_pixels` 系の返り値 (preview / apply /
  auto-apply / ensure-result の全 MI-GAN 入力)。これで**表示も入力も出力も黒不透明で揃う
  (WYSIWYG)**。
- AI アップスケール ON/OFF は final pipeline のみを変える。消しゴム入場時の
  `erase_mask_size` は source 解像度の raw / erase 入力に固定され、アップスケール切替で
  マスク寸法は変わらない。
- AI アップスケール ON の透過画像でも、消しゴム用の入力は final AI の bg 付き cache を
  参照しない。透過 source は黒で不透明化して MI-GAN に渡す。
- `fs_cache` の透明原本は無変更。マスクを 1 つも作らずに消しゴムを抜ければ通常表示は `fs_cache`
  に戻るので、**元の透明画像がそのまま保持される** (= 加工しなければ破壊しない)。
- 結果として MI-GAN と diffusion フォールバックの出力 alpha も一致する (P3-8 の非一貫も解消)。

### 5.1 マスクのデータ構造 (ビットマップ + ベクタ)

マスクは 2 つのレイヤで構成される:

- **ビットマップ** (`Vec<bool>`): 筆 (Brush) / 囲み (Lasso) でのストロークが直接ラスタライズされる。
  mask_db の `mask_data` 列 (1bit/pixel + deflate 圧縮) に保存。
- **ベクタ** (`Vec<Shape>`): 直線 / 縦線 / 横線 / 矩形 / 楕円はオブジェクトとして保存される。
  各 Shape は `op = add | subtract` を持つ。描画モードで作った Shape は `add`、
  消去モードで作った Shape は `subtract` として保存される。mask_db の `vectors` 列
  (JSON) に保存。

MI-GAN / diffusion に渡す最終マスクと、オーバーレイ描画に使うマスクは、
**毎回ビットマップとベクタを合成した結果 (`composite_mask`)**。
合成順序は「ビットマップ下地 → ベクタ Shape を作成順に適用」で固定。
`add` Shape はマスクを足し、`subtract` Shape はそれ以前のビットマップ/Shape 結果を
削り取る。消去モードで矩形や楕円を重ねても、既存 Shape 自体は削除されない。
ビットマップが空でも、ベクタが残っていればエントリは保存されたままになる
(`mask_db.set` の削除判定は「ビットマップ全 false **かつ** ベクタ空」)。

サイドカー (`mimageviewer.dat`) の `SidecarMask` も `vectors` フィールド (JSON 配列)
を持ち、中央 DB と同じ形式でミラーされる。

### 5.2 ベクタオブジェクトの編集

**選択ツール (`S` キー、または左パネル「選択」ボタン)** に切り替えてベクタ本体をクリックすると選択状態になり:

- 未選択のベクタも編集用アウトラインで表示する。`add` は橙、`subtract` は水色の枠で示す
  (合成マスク上では `subtract` が透明になるため、存在確認用の補助表示として別レイヤーに描く)
- ドラッグ: 平行移動 (Pan)
- 直線の両端: 端点ドラッグで始点/終点の位置変更
- 直線の中点付近の菱形ハンドル: 太さ変更
- 矩形/楕円の角・辺ハンドル: サイズ変更、回転ハンドル: 回転
- `Shift`+ハンドル: 端点角度 / 回転角スナップ、矩形/楕円の角リサイズを等比化
- `Alt`+ハンドル: 矩形/楕円を中心固定でリサイズ
- `Del`: 選択オブジェクトを削除
- `Esc`: 選択中は選択解除、未選択なら消しゴムは補完して終了、隠蔽加工は DB 保存して終了
- 他ツールに切り替えると自動で選択解除。ただし描画直後に `S` で選択ツールへ入る場合は、
  自動選択された Shape をそのまま保持してハンドル編集に移れる

描画系ツールでは任意のベクタ選択は行わない。ただし直近に描いた自動選択 Shape の
ハンドル上をクリックした場合だけ、そのツールのまま微調整できる。

消しゴム / 隠蔽加工の描画ツールでは、Ctrl+ドラッグやホイールで筆サイズ・
線幅・縦横線の傾きを変えない。画像上のホイールは修飾キーなしでも `Ctrl+ホイール`
でも表示ズームに使う。パネル上の修飾なしホイールはパネルスクロールに残す。
筆/線のサイズはパネルスライダー、作成後の形状調整は選択ツールのハンドルで行う。

### 5.3 マスク全体の平行移動・回転

選択が無い状態で矢印キー / `[` / `]` を押すと、**ビットマップとベクタすべてをまとめて**変換する。
偶数ページのスキャンゴミ補正で、位置/角度を微調整してから F7/F8 で再適用する用途。

| キー | 動作 |
| --- | --- |
| 矢印 | 1 px 平行移動 |
| Ctrl+矢印 | 10 px 平行移動 |
| `[` / `]` | ±0.1° 回転 (中心回り) |
| Ctrl+`[` / `]` | ±1° 回転 |

倍率キーは平行移動・回転とも **Ctrl** に統一している。`[` / `]` は Shift+ が
論理キー `{` / `}` に化けて `Key::OpenBracket` / `CloseBracket` にマッチしない
(JIS・US 共通) ため Ctrl にせざるを得ず、矢印キーの方も揃えた。

ビットマップは nearest-neighbor で回転するため、累積回転で若干劣化する (割り切り)。
ベクタは端点を厳密に回転させるので劣化しない。画像外にシフトした部分はクリップ。

### 5.4 マスクスロットと F7〜F10 クイック適用

スロット 1/2 に「現在のマスク (ビットマップ + ベクタ)」を保存できる
(`__slot_1` / `__slot_2` のキーで mask_db に格納)。

- 消しゴムモード内の「ロード」ボタン: 現在のマスク/ベクタを**スロットの内容で差し替える** (上書き)。
  直前の状態は Ctrl+Z で戻せる。OR マージにすると偶数/奇数ページを取り違えたときに
  旧マスクが残って過剰マスクになるため、差し替え仕様にしている (2026-04 仕様)。
- **フルスクリーン表示中の F7/F8**: `apply_slot_in_viewing_mode` — 消しゴムスロットの内容で現ページの
  マスクを**差し替えて** DB へ保存し、MI-GAN で inpaint → `erase_result_cache` に格納する。
  `fs_cache` は raw のまま維持し、generation key で古い inpaint 結果を stale 化する。
  消しゴムモードに入らずワンキーで適用できる経路。既存マスクがあっても上書きなので、
  別スロットを再 F7/F8 するだけで直せる。
- **グリッド画面での F7/F8**: `apply_slot_to_selection` — チェック中のサムネイル (なければ選択
  中 1 枚) にスロットを一括で配布。inpaint はその場で走らせず、**各ページを次回フルスクリーンで
  開いたときに `auto_apply_saved_mask` で自動適用**される。DB への書き込みはスロットの元サイズ
  のまま行い、読み出し側で `get_full` が画像サイズに合わせてリスケールする。
- **フルスクリーン表示中の F9/F10**: `apply_conceal_slot_in_viewing_mode` — 隠蔽スロットの内容で
  現ページの隠蔽マスクを差し替えて DB へ保存し、`conceal_cache` を破棄する。隠蔽加工は
  表示パイプラインで合成されるため worker は起動しない。
- **グリッド画面での F9/F10**: `apply_conceal_slot_to_selection` — チェック中のサムネイル
  (なければ選択中 1 枚) に隠蔽スロットを一括で配布する。
- **Shift+F7/F8 / Shift+F9/F10**: 対象ページから適用済みの消しゴム / 隠蔽マスクを削除する。
  スロットそのものは削除しない。

### 5.5 マスク保有ページのバッジ

サムネイル左上に、ページ個別補正の青「補」バッジと並んで、消しゴムマスクがあるページには
オレンジ系の「消」バッジが表示される。

判定用の `App::mask_pages: HashSet<usize>` をフォルダロード時に `mask_db::load_mask_keys` で
一括取得する (スロットキー `__slot_*` は除外)。`save_mask_with_sidecar` /
`delete_mask_with_sidecar` / `apply_slot_to_selection` の書き込み経路でこの集合も
同時に更新する。

---

## 6. 新しい補正項目を追加する時

チェックリスト:

- [ ] `AdjustParams` に新フィールド追加 (デフォルト値は「効果なし」)
- [ ] `apply_adjustments_fast` の適用順序を決めてその中に挟む
- [ ] LUT で済むか、f32 パイプラインが必要かを判断 (必要なら `temperature != 0` 判定に合流)
- [ ] UI パネル (`ui_adjustment_panel.rs::draw_sliders`) にスライダー追加
- [ ] 変更検出は `draw_sliders` が `(changed, dragging)` を返すので追加対応不要
- [ ] `AdjustParams` の JSON シリアライズ互換性を確認 (`#[serde(default)]` 等)
- [ ] **サムネイルにも色調として乗る**: `apply_adjustments_fast` に相乗りするため、
  色系であれば `thumb_adjust_tex` 経由で自動適用される。`is_color_identity()` の
  判定に参加するよう、新フィールドのデフォルトは「効果なし」にしておくこと
  (そうでないとサムネが常時再生成される)。
- [ ] **final pipeline 専用の項目** (smart_sharpen / colorize / Creative LUT /
  post_filter のようにサムネへ乗せない
  もの) は逆に `is_color_identity()` へ参加させず、`is_identity()` と
  `hash_adjust_final_params` にだけ追加する (§2.7 のシャープ化が実例)。

---

## 7. AI モジュールの構成

`src/ai/` 以下:

| ファイル | 役割 |
| --- | --- |
| `runtime.rs` | ONNX Runtime (ort) + DirectML EP の初期化、セッションキャッシュ |
| `model_manager.rs` | exe 埋め込みモデルを `%APPDATA%/mimageviewer/models/` に展開 |
| `upscale.rs` | タイル分割 + オーバーラップブレンド (2x/4x モデル) |
| `denoise.rs` | 1x ノイズ除去 (タイル推論は upscale を流用) |
| `ui_erase.rs` | MI-GAN によるマスク領域 inpaint (`InpaintMiGan` モデルを直接 `with_session` で呼び出し)。見開き中央ギャップ補完は精度不足で削除済み (タグ `v0.6.0-with-spread-inpaint` 参照) |
| `classify.rs` | 画像種別分類 (ヒューリスティクス、Illustration/Comic/RealLife) |

`ModelKind` でモデルを識別。セッションは最初の推論時に遅延生成される。
メモリ負荷が大きいので、不要になった ModelKind はセッションを drop する (runtime.rs)。

---

## 8. UI / UX メモ

### 8.1 ページ切替時のトースト

`open_fullscreen(idx)` 時、`adjustment_page_params` に該当 idx が含まれていれば
右上に `ページ補正適用` トーストを 1.2 秒表示する
(`FEEDBACK_TOAST_DURATION`, `ui_fullscreen.rs:46`)。

### 8.2 サムネイルの補正済みバッジ

グリッド表示で個別補正があるページの左上に青い「補」バッジを表示する
(`draw_cell` の `has_page_override` フラグ)。

### 8.3 フルスクリーン左ホバーパネル

フルスクリーンの左端 / 上端 / 右端ホバーで開く左パネルに、
`画像補正 / 表示トリム / ブックマーク` の 3 タブを置く。選択タブは `Settings::fullscreen_left_panel_tab`
へ保存し、次回起動後も同じタブで開く。上部ホバーバーには画像補正専用の 🎨 ボタンを置かない。

### 8.4 補正レイヤー編集時の表示補助

補正レイヤーパネルでは、`元画像 [Q]` と `マスク [W]` トグルを `local_adjust_lab`
と同じ位置に置く。`Q` は補正レイヤー直前の入力画像 (消しゴム結果があればそれを含む)、
`W` は選択中レイヤーのマスクを表示する。`Ctrl` 押下中は一時的に元画像表示、
`Ctrl+Shift` 押下中は選択中レイヤーだけを除外した補正結果を非同期に生成して表示する。
生成中は補正レイヤー直前の入力画像を表示し、完了後に差し替える。`Alt` 押下中は
マスク表示状態を一時反転する。

画像上の位置ハンドルを持つ効果は、中心点だけでなく効果範囲のガイドも描画する。
半径系・光源系はリングや光源範囲を表示し、TiltShift は焦点幅とぼかし境界の
ハンドルを画像上で直接ドラッグできる。TiltShift の範囲作成モードでは、
画像上のドラッグで初期範囲を作成する。
`ColorFill` / `ColorOverlay` の線形・円形グラデーションも効果位置ハンドルの対象で、
線形は開始・終了ハンドル、円形は中心・半径ハンドルを表示する。角度だけで保持している
線形グラデーションは表示時に端点へ変換し、端点をドラッグした時点でカスタム端点指定へ
切り替える。半径ハンドルは大きい半径で画像外に出ても掴めるよう、半径ドラッグだけは
画像外の正規化座標を許可する。

補正レイヤーの独立パネルと効果パラメータの popup / combo box は常に dark theme で描画する。
egui の popup は別レイヤーで生成されるため、パネルの `Visuals::dark()` だけでなく
`ThemePreference::Dark` も一時的に適用する。

---

## 8.X 画像補正 / 補正レイヤーの Undo / Redo (v0.8.1+)

`Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) でフルスクリーン中の画像補正操作と補正レイヤー操作を取り消せる。
履歴は [`crate::undo_stack::UndoStack`](../src/undo_stack.rs) に積まれ、
レーティング・タグの Undo と同じスタックを共有する。通常補正は `UndoEntry::Adjustment`、
補正レイヤーは `UndoEntry::LocalAdjustment` として記録する。

### 取り消し対象

- 左パネルの全スライダー / ラジオ / コンボボックス / リセット↩ボタン (= `set_page_params` 経由のページ個別更新)
- アクションボタン: 「お気に入り標準にする / 解除」「標準にする」「個別設定を解除」
  (= `set_favorite_default` / `clear_favorite_default` / `copy_params_to_global` /
  `clear_page_params` 経由)
- ホットキー: U / Shift+U / Alt+U (アップスケール循環)、N (デノイズトグル)、
  T / Shift+T / Alt+T (ポストフィルタ循環)、Q / Ctrl+Backspace (個別解除)
- 保存スロット適用: Ctrl+1〜9 / Ctrl+0、左パネルのスロットボタン
- 補正レイヤー: レイヤー追加/削除/全削除、ON/OFF、効果パラメータ編集、
  効果コピー/ペースト/リセット、LUT 読み込み、カラー/グラデーション/位置ハンドル/手描きマスクのキャンバス操作

### スコープ表現 ([`AdjustUndoScope`](../src/undo_stack.rs))

```rust
enum AdjustUndoScope {
    Page(usize),       // adjustment_page_params[idx]
    Favorite(Uuid),    // adjustment_favorite_params[uuid]
    Global,            // settings.global_preset
}

struct AdjustmentChange {
    scope: AdjustUndoScope,
    before: Option<AdjustParams>,  // None = エントリ無し (Page / Favorite のみ)
    after: Option<AdjustParams>,
}

struct LocalAdjustmentChange {
    idx: usize,
    before: Vec<LocalAdjustmentLayer>, // empty = 補正レイヤーなし
    after: Vec<LocalAdjustmentLayer>,
}
```

`Global` スコープは常に `Some` (`settings.global_preset` は Optional ではない)。
`Page` / `Favorite` の `None` は「そのスコープにエントリが存在しない =
下層 (Favorite default / Global) にフォールバック」を表す。

### 適用ロジック (`apply_adjustment_change_to_app`)

スコープごとに既存の書き込み API を再利用する。ただし `Page(Some)` の復元は
`set_page_params` の後に `clear_caches_for_param_change(old, new)` を通し、通常の
スライダー / スロット適用と同じ final AI 差分無効化を行う。`set_page_params`
単体は DB 更新・サイドカー更新・軽量な表示キャッシュ無効化までを担当し、
final pipeline / AI cache の差分分類は呼び出し側責務。

`Page(None)` は `clear_page_params`、お気に入りとグローバルは
`set_favorite_default` / `clear_favorite_default` / `copy_params_to_global` を使う。
これらの経路は通常操作と同じ副作用に乗せ、Undo 用に別の永続化経路を作らない。

補正レイヤーは `apply_local_adjustment_change_to_app` で `before` / `after` の
`Vec<LocalAdjustmentLayer>` を `set_local_adjust_layers_for_idx` へ戻す。これにより
`local_adjust.db` 更新、`local_adjust_pages` バッジ集合、`local_adjust_generation` bump、
`local_adjust_cache` / 下流 `conceal_cache` の無効化が通常操作と同じ経路で走る。

### スライダードラッグの取り扱い (drag-release granularity)

旧実装はスライダードラッグ中に毎フレーム `set_page_params` を呼んでいたため、
60 frames/sec の DB UPSERT + サイドカー XMP 書き込み + キャッシュクリアが発生して
いた。Undo 機能と一緒に以下のように改修:

- `App::adjustment_drag_session: Option<AdjustmentDragSession>` を追加
- 状態遷移:
  - drag 開始フレーム (`prev_dragging=false && curr_dragging=true`):
    `before = adjustment_page_params.get(&fs_idx).cloned()` をスナップショットして
    session を立てる。
  - drag 中 (`is_dragging=true`): `adjustment_page_params[fs_idx] = edit_params`
    だけ更新 (DB / サイドカー書き込みなし)。色調キャッシュは毎フレームクリアして
    リアルタイムプレビューは維持。
  - drag 終了フレーム (`prev_dragging=true && curr_dragging=false`): session を
    `take()` し、`set_page_params` を **1 回だけ** 呼んで永続化 + Undo エントリを 1 件
    プッシュ。
- 非ドラッグ変更 (ラジオ / コンボ / リセット↩ボタン) は drag セッション無視で
  即時 `set_page_params` + Undo プッシュ (1 操作 = 1 エントリ)。

これにより、CRT エミュレーションのような細かい調整も「1 つ前に戻す」が直感的に
効く + DB 負荷も大幅減 (60 writes/sec → 1 write/release)。

### 履歴クリア境界

`App::clear_meta_undo` で undo / redo 両方を破棄。呼び出し箇所:

- `load_folder` (フォルダ移動)
- `open_fullscreen` (グリッド → フルスクリーン、フルスクリーン中の画像移動)
- `close_fullscreen` (フルスクリーン → グリッド)
- `enter_erase_mode` / `reset_erase_mode` (消しゴムモード遷移)

消しゴムモード中は `erase_undo_stack` (バイトマップスナップショット) が
`Ctrl+Z` を担当するため、行き来の境界で履歴をクリアして文脈を分離する。

`clear_meta_undo` は `adjustment_drag_session` も `None` にリセットするので、
ドラッグ中に境界を跨いでも進行中セッションが残骸として残らない。
補正レイヤーのキャンバスドラッグ / 手描きブラシは開始前レイヤー配列を
`local_adjust_canvas_drag_before_layers` / `local_adjust_mask_brush_before_layers` に保持し、
release 時に 1 Undo として確定する。モード終了・フォルダ切替・360 モード開始では
これらの一時状態も破棄する。

### 副作用に乗る点

- `set_page_params` の **「ページ個別が effective_default と一致するなら個別を削除」**
  正規化は Undo 経路でも有効。`before` に `Some(p)` で記録された値を再適用しても、
  もし `p` が現在の effective_default と一致するなら個別エントリは作られず削除される。
  この振る舞いは normal 操作と同じなので一貫性がある。
- `set_favorite_default` / `clear_favorite_default` は内部で「冗長になった個別を削除」
  処理を行う (Codex P2 対応)。Undo で巻き戻したときも同じロジックが走るので、
  状態は always 正しい不変条件を保つ。

---

## 9. フォルダ側サイドカーバックアップ

ページ個別補正 (`adjustment.db`) と消しゴムマスク (`mask.db`) は、中央 DB だけだと
「フォルダを別ドライブへ移動するとパスキーが無効化されて設定が失われる」という弱点がある。
これを補うため、各ユーザーフォルダ直下に `mimageviewer.dat` (Hidden+System 属性の JSON)
をバックアップとして配置する。

### 9.1 ミラーの原則

- **中央 DB が authoritative**。サイドカーはあくまでバックアップ。
- **すべての書き込みはミラー**: DB 更新と同じタイミングでメモリ上のサイドカー表現
  (`App::sidecars`) を更新し dirty フラグを立てる。
- **実ディスク書き込みのタイミング**:
  1. フォルダ切替時 (`start_loading_items` 冒頭で `flush_all_sidecars`)
  2. アプリ正常終了時 (`on_exit` 内)
  3. 5 秒アイドル時 (毎フレーム `flush_idle_sidecars` で `is_dirty && now - last_change >= 5s` を判定)
- **読み込み**: `start_loading_items` 内で `import_sidecar_to_dbs` が走り、中央 DB に無い
  エントリだけサイドカーからインポートする (冪等)。

### 9.2 キー規則

サイドカー内のエントリはフォルダ**相対**キーで保存する (絶対パスにすると移動で意味が消えるため):

| GridItem         | サイドカー置き場       | 相対キー                                      |
| ---------------- | ---------------------- | --------------------------------------------- |
| `Image(p)`       | `p.parent()`           | `"{filename_lower}"`                          |
| `ZipImage`       | `zip_path.parent()`    | `"{zip_filename_lower}::{entry_name_lower}"`  |
| `PdfPage`        | `pdf_path.parent()`    | `"{pdf_filename_lower}::page_{n}"`            |

これらは `App::page_path_key` が返す絶対 DB キーと 1:1 で対応する。ヘルパー:

- `App::sidecar_folder(idx)` → 置き場
- `App::sidecar_relative_key(idx)` → 相対キー
- `sidecar::reconstruct_image_key(folder, rel)` / `reconstruct_virtual_key(folder, rel)` → 絶対 DB キー再構成

**キーの整合性はユニットテストで担保**。`adjustment_db::normalize_path` の挙動と揃っていないと
インポートで復元されない。

### 9.3 マスクの書き込みタイミング

消しゴムマスクは書き込みコストが大きい (1bit/pixel pack + deflate + JSON 埋め込み) ため、
**消しゴムモードの確定点でのみ** 書く:

1. `ESC` 終了 (ui_erase.rs の ESC ハンドラ内 `save_mask_with_sidecar`)
2. `E` 補完実行 (ui_erase.rs `execute_erase_inpaint` 内 `save_mask_with_sidecar`)
3. 「マスク全削除」ボタン (ui_erase.rs 内 `delete_mask_with_sidecar`)

ストローク毎の書き込みは行わない。中央 DB もサイドカーも同じタイミングでしか書かない。

### 9.4 空になったファイル

サイドカーエントリは `{adjust?, mask?}` 構造。`adjust` と `mask` が両方 `None` になると
`items` マップから削除する。`items` が空のまま flush されると **ファイル自体を `remove_file`**
する (消しゴムマスクも全削除しないと空にはならないので、ユーザの明示的操作が前提)。
ファイル削除失敗は黙って無視。

### 9.5 設定トグル (`sidecar_backup_enabled`)

`Preferences > フォルダ > 設定のバックアップ` に配置。デフォルト ON。
OFF にすると **読み書き両方スキップ** (既存 `.dat` は削除しない)。単一の分岐点 (`App::sidecar_mut`
が `None` を返す) で済むので、デバッグ面でのコストは最小。

同じ `mimageviewer.dat` には、タグ用の任意バックアップ (`tag_sidecar_backup_enabled`、既定 OFF) も
同居できる。タグは `tags.db` が正本で、読み込み gate と sync 状態は補正・マスク系とは独立して管理する。
フォルダ自身のタグは sidecar へは書かず、`tags.db` と世代バックアップのみで保持する。

### 9.6 エラーハンドリング

読み取り専用メディアや権限不足で IO が失敗した場合:
- ログに 1 行書いて無視
- `SidecarFile::disabled = true` を立てて以降同フォルダは再試行しない (ログ汚染防止)
- アプリ再起動で `disabled` はリセット

ユーザへのダイアログ表示はしない (視聴体験の邪魔になるため)。

### 9.7 テスト

サイドカーの動作は 3 層で自動テスト済み:

- **単体テスト**: `src/sidecar.rs` の `#[cfg(test)] mod tests` に 9 件
  (set/remove、空→削除、JSON ラウンドトリップ、キー再構成など)
- **統合テスト**: [tests/sidecar_import.rs](../tests/sidecar_import.rs) に 12 件
  - **フォルダ移動シナリオ**: 空 DB + サイドカー → DB に復元
  - 中央 DB が authoritative (既存エントリが上書きされない)
  - 部分的重複時の正しいスキップ/インポート振り分け
  - ZIP / PDF エントリのキー整合 (`adjustment_db::normalize_path` と一致)
  - サイドカー無しの no-op
  - 将来バージョンの `.dat` をインポートしない
  - 書き込み不能パスで panic しない
  - Hidden+System 属性付きファイルの再読込・上書き
- **手動 E2E**: フルスクリーン UI 経由での編集→フォルダ移動→復元までは
  [docs/e2e-smoke-test.md](e2e-smoke-test.md) を参照。

統合テストは `cargo test --test sidecar_import` で 1 秒程度で走る。
GUI 起動を含まないため CI でも実行可能。テストの多くは
`AdjustmentDb::open_at` / `MaskDb::open_at` で一時 DB を作って隔離している
(デフォルトの `open()` は `%APPDATA%` を使うのでテスト用途には不向き)。
