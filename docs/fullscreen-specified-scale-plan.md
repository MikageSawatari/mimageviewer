# 指定サイズのフルスクリーンfit計画（§1.342）

作成: 2026-10-08 / 改訂: 2026-10-09（r5の決定仕様・設計合意に基づく実装接続の記録）/ ラインE。
状態: **Q1〜Q11は決定済み（§10.1）。設計・構造判定への合意を受領し実装中。実装差分の独立レビュー・自動gate・実機確認は未完了（§15）**。
コード照合基準: 初版 `6078f6595`、r2 `2d327da31`、r3 `ab99c0a6b`（レビュー対象文書 `d648a5d54`）、r4 `511694cd7`。v4.5.0出荷後の `840c309ec` での再照合は§14。実装の受入設計基準は `ebc6ce987`。過去のfile:lineと文書のみの作業範囲は各ラウンド時点の記録として残し、現在の実装状況は§15へ集約する。

## 1. 要求と境界

[backlog §1.342](next-release-backlog.md)と、v4.5.0で出荷済みの[§1.343のfit循環仕様](keymap-spec.md)の利用者決定を前提とする。
幅と高さは独立・各軸任意、単位は％ / px。既存のPage / Width / Height / Originalに
一方式を追加し、既存の循環と直接選択で切り替える。縦横比を変える指定ではない。
高さだけ800pxなどを指定できる。ウィンドウ・F11・F12・見開き・縦横連結・回転・画像フォルダを対象とする。

決定仕様は§10.1に2026-10-09付で記録する。利用者への未決質問はない。
§1.343の確定済みの除外ID規則、割り当ての既定なし、単一循環ownerは再判断しない。
今回の範囲は画像表示のfit。動画・音楽のサイズ方式、加工・書き出し解像度、
モニター選択、窓の配置やサイズ自体、画像別プリセットは追加しない。
指定値は共通設定に加えて既存のお気に入り別表示状態へ参加させる（Q9、§7.1）。

## 2. コードで確認した前提

file:lineは上記HEAD時点。既存の状態・入口を拡張できる根拠は次のとおり。

| 前提 | コードと確認結果 |
| --- | --- |
| 方式・通常順 | [settings.rs:3627](../src/settings.rs#L3627)のenum、:3647の`all()`はPage / Width / Height / Original。MarginFitは互換carrierで新規選択しない |
| 除外ID | [settings.rs:3676](../src/settings.rs#L3676)の`cycle_id / cycle_enabled / next_for_flow`は明示除外のみ無効。:9547は最後の候補を保護、:9578は読込時の全候補除外を補修。:14396の`FutureScale`は未知ID保持用のテスト文字列で、予約IDではない |
| 入力owner | [ui_fullscreen.rs:37188](../src/ui_fullscreen.rs#L37188)は画像適格性・音楽・編集guardを持つ共通循環。[keymap.rs:6621](../src/keymap.rs#L6621)は0 / Numpad0、[ring_shortcut.rs:1074](../src/ring_shortcut.rs#L1074)は既存`image_fit_mode_cycle` ID。[gamepad_input.rs:6203](../src/app/gamepad_input.rs#L6203)は同じ循環へdispatchし、:8157以降のfit pickerも共通候補を参照 |
| 候補UI | [preferences/pages.rs:9628](../src/ui_dialogs/preferences/pages.rs#L9628)の循環checkboxと:9784のfit ComboBoxは`all()`を使う。直接選択を除外集合で絞らない |
| 幾何owner | [displayed_image_transform.rs:304](../src/displayed_image_transform.rs#L304)がfit・trim中心・倍率制限・zoom/panを解き、:195以降のgeometryがpaint / hit / UV / scaleを保持。[display-pipeline.md §2.4](display-pipeline.md#24-変換の合成順序)と一致 |
| 倍率制限・原寸 | [displayed_image_transform.rs:65](../src/displayed_image_transform.rs#L65)はno_upscale / no_downscaleを順に適用。:1316の原寸は`1 / effective pixels_per_point`。両方ONなら原寸。手動zoomは:365以降でその後に掛かる |
| 表示領域 | [ui_fullscreen.rs:20858](../src/ui_fullscreen.rs#L20858)はseek geometryと右パネルの実効予約からmedia rectを作る。:23564以降で描画先Contextのrect / pppが揃う。モニター全体や保存した窓サイズを使わない |
| 見開き | [ui_fullscreen.rs:8366](../src/ui_fullscreen.rs#L8366)はOriginal以外で高さを揃える。:18359、:31514、:40978にもfit式がある。新enumを足すだけではwildcard経由のPage扱いになり指定値を使わない |
| 連結読み | [ui_fullscreen.rs:37627](../src/ui_fullscreen.rs#L37627)はunitごとの寸法・gap・scaleを計算。:8370の仮想slotと縦連結Width専用の単独表紙特例は別物。:37303のflow切替は既定fitへ戻す |
| 対象媒体 | [ui_fullscreen.rs:37376](../src/ui_fullscreen.rs#L37376)はImage / ZipImage / PdfPageを対象とし、通常の画像フォルダでも使える。動画・音楽は循環対象外 |
| 凍結描画 | [ui_fullscreen.rs:2921](../src/ui_fullscreen.rs#L2921)はcapturedのfit mode・limits・placementを渡す。:3159等もcapture値を使う。指定値を現在のSettingsから後読みする入口を作らない |
| PDF要求 | [ui_fullscreen.rs:23574](../src/ui_fullscreen.rs#L23574)と:37089が初回targetを構築。[pdf_loader.rs:4974](../src/pdf_loader.rs#L4974)は物理pxと既存4種のfit、:6547は必要raster長辺を導出。[app.rs:69828](../src/app.rs#L69828)は同じtargetをズーム再レンダへ使う |
| PDF再レンダの循環 | [app.rs:69974](../src/app.rs#L69974)は結果のpage boxを捨て、:69988でraster寸法をsource_dimsへ採用。:78199のsource_dims_for_idxもその画素寸法を返す。原寸制限後に手動zoomを掛ける:365の幾何から次の需要を作ると、原寸・需要が自己増幅し得る。PDF page boxは[pdf_loader.rs:5118](../src/pdf_loader.rs#L5118)の1/1000 point、content typeは:5017、Original最低長辺4096とnative capは:5010 / :6556〜:6563で確認。新方式だけPDF原寸をraster出力から分離する |
| 単ページZ / 見開きZ | [displayed_image_transform.rs:1127](../src/displayed_image_transform.rs#L1127)はPage・制限なし・自由回転なしを強制。:1815のfull_image_zoomはPage基準。一方[ui_fullscreen.rs:40978](../src/ui_fullscreen.rs#L40978)と:13169の見開きZは選択fit / limitsから得たfit_scale・gapを照準の基準にする。[display-pipeline.md:1662](display-pipeline.md#L1662)にこの違いと見開きPDFのZ再レンダなしという現行制限がある |
| 寸法の単位と原寸制限 | [display-pipeline.md:34](display-pipeline.md#L34)はsource_dims＝画素、PDF layout_dims＝1/1000 point。[ui_fullscreen.rs:8746](../src/ui_fullscreen.rs#L8746)はcanonical aspectをpixel sourceの長辺へ正規化。[同:36428](../src/ui_fullscreen.rs#L36428)と[display-pipeline.md:1705](display-pipeline.md#L1705)の単ページ / 見開きは現在の描画textureを原寸基準にできるため、AI到着で制限の結果も変わる。連結は:37481と:8944の既存raw/source優先規則が異なる |
| 回転＋trimの経路差 | [ui_fullscreen.rs:24028](../src/ui_fullscreen.rs#L24028)の通常単ページは保存回転でもtrimを渡すが、見開き:40758、連結:37562、capture生成:10465、PDF要求:37103は回転ページのtrimを落とす。:17114はこの差をcallerの責務として明記。fit式だけの共通化では同じ入力にならない |
| 保存・転送 | [settings.rs:4995](../src/settings.rs#L4995)に既存fit設定、[settings_db.rs:16](../src/settings_db.rs#L16)にsettings_kv。[settings_transfer.rs:174](../src/settings_transfer.rs#L174)は全フィールド分類、:334はfit関連のplain転送、:224は受け入れたキーだけ適用 |
| prefs確定 | [preferences.rs:2155](../src/ui_dialogs/preferences.rs#L2155)の`install_preferences_settings`と:2709のOK境界。UIで管理する新設定を「環境設定外の最新値移送」に混ぜない |

実装前に再照合し、別の前提が必要と判明した場合は止めて設計担当へ返す。

### 2.1 r4で追加照合した表示単位・DPI・保存経路

以下は`511694cd7`のsource inspection。製品を起動した観測ではない。

| 対象 | file:lineと結果 |
| --- | --- |
| 一覧サムネイルの表示サイズ | [settings.rs:4149](../src/settings.rs#L4149)では`grid_cols`が画面上のサイズを決め、`thumb_px`とは区別。[app.rs:36965](../src/app.rs#L36965)がcellのpoint寸法とeffective pppを[thumb_loader.rs:1395](../src/thumb_loader.rs#L1395)へ渡し、`ceil(max(cell_w, cell_h) × ppp)`を表示用物理pxへ解く。固定の画面px指定ではない |
| サムネイルキャッシュ解像度 | [settings.rs:4486](../src/settings.rs#L4486)の`thumb_px`は画像長辺の実画素数。[thumb_loader.rs:1169](../src/thumb_loader.rs#L1169)はdecode targetに直接使う。表示寸法用pxと画像生成用pxは同一規則ではない |
| 静止画seek strip | [settings.rs:6711](../src/settings.rs#L6711)は100%時のlogical pointと明記。[ui_fullscreen.rs:20594](../src/ui_fullscreen.rs#L20594)は`.points()`の値をmedia rectと同じpoint座標へ渡す |
| 動画seek strip | [seek_strip_layout.rs:87](../src/video/seek_strip_layout.rs#L87)・:130の`.points()`もlogical point。[preferences/pages.rs:9390](../src/ui_dialogs/preferences/pages.rs#L9390)は「100%表示時のpx相当」、:9421はpx suffix。nativeも[render_core.rs:12518](../src/video/native_presenter/render_core.rs#L12518)で描画先pppをeguiへ渡す |
| 固定バー間隔 | [preferences/pages.rs:9899](../src/ui_dialogs/preferences/pages.rs#L9899)はpx表記だが、[ui_fullscreen.rs:4206](../src/ui_fullscreen.rs#L4206)・:13410では値をそのままpoint矩形へ加減する（`/ppp`しない）。物理格子の丸めは別段階 |
| DPI / UI倍率 | [settings.rs:107](../src/settings.rs#L107)のUI倍率をzoom_factorへ適用。[egui/context.rs:451](../vendor/egui/src/context.rs#L451)は`effective ppp = native DPI × zoom_factor`。pointの表示寸法がこの倍率で物理pxになる |
| 連結の通常fit | [ui_fullscreen.rs:37634](../src/ui_fullscreen.rs#L37634)は各unitの幅 / 高さに同じfit規則を適用。:37773〜:37807で毎frame全unitの寸法を解く。:38343〜:38353のscroll再アンカーはscroll / current idxを変え、fit / fs_zoomをresetしない。中央unitの変化を倍率変更の理由にしないが、全unit共通の「選択時倍率」を保存する実装でもない |
| 連結の安全上限例外 | [ui_fullscreen.rs:37826](../src/ui_fullscreen.rs#L37826)〜:37839は可視ページ数過多でzoomを増やし、:37841以降はさらに遠い可視unitを除く。「既存方式はscrollで絶対に倍率が変わらない」とは言えない |
| AI前の入力 | [final_pipeline.rs:22](../src/ai/final_pipeline.rs#L22)の出力は`source_size / used_upscale`を持ち、:110でAI前inputの寸法を取る。[ui_fullscreen.rs:36428](../src/ui_fullscreen.rs#L36428)ではsource_sizeのない経路はtexture寸法でfitする。新方式ではそのtexture由来の原寸をAI出力へ更新しない |
| お気に入りの状態 | [settings.rs:4147](../src/settings.rs#L4147)は`FavoriteViewState`の8項目。指定値・fit mode・倍率制限は現在含まれない。[app.rs:80466](../src/app.rs#L80466)が行先の状態を投影し、:80507のcapture / :80531のtransitionが既存owner。[adjustment_db.rs:389](../src/adjustment_db.rs#L389)・:411は既存state_jsonを読む / 書く。新フィールド欠落で既存行全体のdeserializeを失敗させない |
| 共通値の保護 | [settings.rs:8564](../src/settings.rs#L8564)のoverlay、:8589のprefs snapshot、:8601のprefs route、:10297の保存前overlay解除が共通値を守る。[preferences.rs:2073](../src/ui_dialogs/preferences.rs#L2073)の標準 / 有効値の分離、既存DB workerも再利用する |

**Q2の決定（2026-10-09）**: 指定サイズは画像生成解像度ではなく表示寸法なので、pxをlogical point相当へ揃える。
既存の表示寸法pxと異なる物理pxを維持する具体的な必要性は見つからない。Originalの物理1:1や
PDF要求の物理pxは用途が異なるため、表示寸法の単位変更によってそれらを変更しない。

## 3. fit cycle・ID・入力

追加enum / stable IDは共に **`SpecifiedScale`**、表示名の推奨は **「指定サイズ」**。
通常順は **Page → Width → Height → Original → SpecifiedScale → Page**。
`all / selectable_for_flow / label / cycle_id`と全match consumerを拡張する。
Originalは引き続き物理1:1。指定値を消さず往復できる。MarginFitの互換処理は変更しない。

- `fullscreen_fit_cycle_excluded`は既定空・未知ID保持・最低1候補を維持。
  `"SpecifiedScale"`が明示除外されなければ有効。過去に対象集合を編集済みでも同じ。
- `"FutureScale"`を新IDへ改名・移行しない。未知IDのまま保持する。
- 全5方式を除外した読込値はPageを有効に戻す。UIでは最後の1つをOFFにできない。
  従来4方式だけ除外された値は新方式が有効なので空集合ではない。転送も5方式で検証する。
- 直接メニュー・環境設定fit選択・リングの既存fit pickerには全5方式を表示する。
- `FsFitModeCycle` / `RingActionId::ImageFitModeCycle`と既存key / ring / gesture / mouse / pad routingを再利用。
  個別の指定方式Action、新しい既定割り当て、cycle indexは追加しない。
- 1候補・現在が対象外のときも§1.343の動作を維持。除外集合編集で現在方式を勝手に変えない。
- メニュー・循環で方式が変わるときは既存setter / `reset_fullscreen_fit_transform`を使う。
  zoom=1 / pan=0 / free rotation=0と既存PDF要求の意味を揃える。同じ方式を再選択したときのno-opも維持。

## 4. 指定値と計算（Q1〜Q4）

### 4.1 軸・単位・基準

各軸は「指定なし」「％」「px」。幅％＋高さpxなどの混在を認める。
％の100は**その表示先の現在の画像表示領域**の同じ軸の長さ（Q1）。
既存media rect / image_rectを使い、OSタイトルバー・固定HUD・固定右パネルの予約分は含めない。
hover HUD / popup等の重なりは新たに差し引かない。§1.344で予約が抑制されれば％の基準も広がる。
元画像画素数に対する％、モニター解像度、保存した窓サイズにはしない。

pxは**100%表示時のlogical point相当**へ揃える（Q2決定、2026-10-09、§2.1）。
指定値をそのままpoint矩形の長さに使い、`/ppp`しない。OS DPIとUI倍率を含む描画先の
effective pppで物理表示寸法へ変換される。main Contextのpppを別窓へ使い回さない。
表示領域を`Vw, Vh`（point）、pppを`P`、解決済み各軸枠を`Bw, Bh`とすると:

- ％: `Bw = Vw × width_percent / 100`、高さも同様。
- px: `Bw = width_px`、高さも同様。物理表示幅は`Bw × P`。
- 未指定軸: 制約なし。両方未指定だけはPageと同じfitへ解決する（Q4）。
- 初期値は**幅80％・高さ80％（両軸指定）**で決定（Q4、2026-10-09）。現在のfit modeを変えない。
  画像表示領域の幅・高さとも80％の枠へ内接し、周囲に余白を残す設定例とする。
  利用者が用途に合わせて変更する前提。原寸制限・手動zoomで指定枠を超える場合は下記の契約に従う。
  未編集でも循環へ参加し、動的に候補を減らさない。
  極小領域のgap補正は§5による。両Noneはその補正も行わず既存Page基準を使う。
  両NoneのPage相当はfit式に限る。selected modeはSpecifiedScaleで、原寸入力と上限の契約は
  §4.3 / 下記のまま。trimは§5.1の既存経路別規則を使う。

初期値の例: image rectが1000×800pointなら枠は800×640point。
1000×2000の単ページは320×640pointへ内接する（倍率制限OFF・上限未到達・manual zoom=1）。
片軸へ変更した例: P=1.5で高さ800pxだけなら高さ枠800 point、物理1200px。
1000×2000の単ページは物理600×1200pxになる（倍率制限OFF・上限未到達・manual zoom=1）。
幅50％＋高さ800pxなら両方の小さい倍率を採る。

### 4.2 縦横比・倍率制限・手動操作

§4.3でアスペクトと物理原寸を区別して正規化し、回転・trim・表示単位合成後の
基準寸法を`Sw, Sh`とする。各ページの縦横比を保持する。

- 幅だけ: `s = Bw / Sw`。高さだけ: `s = Bh / Sh`。
- 両方: `s = min(Bw / Sw, Bh / Sh)`。引き伸ばし・cover cropはしない。
- 両方なし: Page。片軸未指定を100％で埋めない。
  高さだけ800pxの横長画像は、幅がviewportを超える場合がある。
- 次に既存`FullscreenFitScaleLimits::apply`を適用（Q3）。
  no_upscale ONなら小画像を原寸以上に拡大しない。既定OFFなら指定サイズまで拡大する。
  no_downscale ONは原寸以上を優先するため指定枠を超え得る。両方ONなら原寸。
- 制約は自動fit倍率に対するもの。見開きの内部高さ合わせや手動zoom / Zは既存どおり。
  個々のページの全倍率を1以下にする機能へ変更しない。
- 自動fitの後に既存manual zoom / panを掛ける。手動zoomは指定枠を超えられる。
  **Zはこの一律の順序に含めない。** 単ページと見開きの違いは§4.4による。

指定はfit計算の基準であり、新しい画面枠やcropではない。
viewportを超える画像は既存clipと閲覧操作を使う。物理格子整列と丸めの画素単位の差は既存契約を維持。
paint / hit / UVを異なる倍率で解かない。

**過大拡大の上限（Q4決定、2026-10-09）**: SpecifiedScaleの自動fitに固定 **5.0倍**を追加する。
過大な拡大による画質低下を避ける元の要望に対応し、この既定の固定上限も確認ケースにする。
原寸基準Oをpointへ正規化した物理原寸倍率`1/P`に対し、
`s_auto = min(FullscreenFitScaleLimits::apply(s_requested), 5.0 / P)`とする。
「拡大しない」は1倍までなので5倍上限より強い。「縮小しない」は1倍を下限として指定枠より
優先し、その上で5倍上限を守る。両方ONは1倍。両NoneのPage相当fitにも5倍上限を適用する。
5.0は初版では固定値とし、別の上限設定・保存fieldは作らない。画像生成・AIの拡大率や
PDF raster要求の上限ではない。見開きでは既存高さ合わせ後の合成単位に掛かる自動倍率を
制限する（従来no_up / no_downと同じ段階）。小さい相方を高さ合わせする内部倍率まで5以下に
する契約ではない。連結は基準unitの自動倍率に適用し、周辺unitを個別に再fitしない。
手動zoomはこの後に掛け、5倍を超えられる。単ページZはPage・制限なし、見開きZの照準は
上限適用済みfitを使い、確定Zは既存専用solveを維持する。Zの結果や手動操作まで5倍に
制限しない。追加の上限入力欄は作らない。

### 4.3 resolver入力: アスペクトとAI前の物理原寸（Q3決定、2026-10-09）

canonical寸法を「原寸の画素数」として渡さない。共通resolverへ渡す各ページの値を区別する。

| 入力 | 単位・役割 |
| --- | --- |
| アスペクト基準 `A` | ページの縦横比と構成を決める寸法。取得済みcanonical layoutを優先。PDFはpage boxの1/1000 pointであり、長さを物理pxやpppへ直接変換しない |
| 物理原寸基準 `O` | 自動fitの拡大 / 縮小制限・5倍上限に用いるfull-pageの画素基準。通常画像はAI前にそのconsumerが使う原寸を維持し、新方式のPDFは下記の固定原寸方針。AI出力と現在のPDF raster寸法を渡さない |
| content bbox・rotation | §5.1で解決したsource座標の正規化bboxと保存回転。分割・trimの優先を解決してから渡し、回転とbbox変換を一度だけ行う |
| 表示先・構成 | 実image rect（point）、そのContextのeffective ppp、ページのlogical scale・slot・実効gap。frozenはcapture時の入力を使う |

**新方式ではAI到着前後にOを変えない。** AIなしの通常画像は、各consumerの既存原寸と
no_upscale / no_downscaleの順序を保つ。AIが有効なら、その画像の**AI前の表示原寸基準**を
同じsource identity / geometryへ対応付ける。単ページ / 見開きで現在textureを原寸にする
経路を、そのままAI出力textureへ適用しない。AI前inputの寸法はfinal_pipeline:110 / :25で
既に扱うが、RAWのdeveloped_dims等、consumerがcanonical pixel原寸を使う場合はその基準を
維持する。AI inputがdecode / GPU clampで小さくても、canonical原寸を使うconsumerのOまで
小さくしない。逆に通常consumerが既存の非AI rasterを原寸にするなら、AI前のその基準を使う。
AI倍率で結果texture寸法を割り戻す推測や、初めて到着したAI結果をOとして保存する案は採らない。
AI前のsource / layout ownerの値を、派生textureの既存source metadataとの対応に使う。
必要な対応を同じsource / contextの既存metadataから得られなければ、実装時に止めて報告する。
新方式のPDFは、以下の固定原寸方針を使う。
見開きの:40879はcanonical layoutをそのまま基準寸法へ採る枝もあるため、新方式へ
その単位を物理原寸として持ち込まない。新方式adapterは媒体ごとのOを画素基準へ解決して正規化し、
既存4方式のその枝は変更しない。
連結もAI前のraw/source基準を使い、AIだけの画素数増加・丸めをアスペクト変更として扱わない。
denoise、retained AI、final composite、注釈 / mask / conceal等をAI結果の上へ合成したtextureも、
親sourceの同じA / Oを引き継ぐ。派生画像のGPU clamp寸法を原寸へ戻さない。
AI以外の明示的な編集でページ構成・canvas / bboxが本当に変わる場合は既存のsource / geometry
更新境界で再計算する。AIだけの差替えとは区別する。旧4方式の原寸規則は今版では変更しない。
根拠は上の§2、[raw_page_store.rs:2291](../src/app/raw_page_store.rs#L2291)、
[ui_fullscreen.rs:27000](../src/ui_fullscreen.rs#L27000)・:40879。

**新方式のPDF原寸は再レンダ出力から独立させる（Q3補足）。** PDFのpage boxによるAと、
ページ固有のcontent type / raster native寸法から、次の固定原寸長辺`L0`を求める。

- Vector: `L0 = 4096px`。既存Original最低長辺定数を原寸基準にも再利用する。
- Raster: `L0 = min(4096px, native_long_edge)`（正のnative寸法）。既存native capを優先する。
- `O = A × L0 / max(Aw, Ah)`。全ページ・非回転・trim前の寸法で、bbox / rotationは後で一度だけ適用する。
  1/1000 pointを画素数とみなさず、72dpiなどの新しい固定DPIも導入しない。
  page boxから見た実効DPIはこのL0とページのpoint寸法から導けるが、raster結果や表示先から逆算しない。

4096は**要求の最低値だけでなく、新方式の原寸制限用の固定参照長辺**にする推奨案。
この基準はviewport / ppp / 指定値 / manual zoom / Z / 現在のraster target / AI textureに依存しない。
ページのcontent typeとnative寸法はPDF自体のmetadataであり、再レンダ画像のwidth / heightで更新しない。
同じsource identityの同じmetadataであれば、thumbnail→初回raster→再レンダ→AI差替えでもA / Oは同じ。
ページ内容・page box・content typeの真正な変更は既存source更新境界で再評価する。
既存4方式のrasterを原寸にする動作は変更しない。注釈などのsource座標に使う
`source_dims_for_idx` / `FsLoadResult::Static.source_dims`も現行契約のまま保持し、
**新方式のPDF fit adapterはその値をOとして読まない**。

単ページの正規化は既存:8746と同じく `N = A × max(Ow, Oh) / max(Aw, Ah)`。
これでアスペクトはA、長辺のpixel基準はOになる。Nへrotation / bboxを適用し、見開きは
各ページの既存高さ合わせを合成した後に§4.2を解く。pixel基準上のscaleへ
`FullscreenFitScaleLimits::apply`（物理原寸は`1/P`）を適用する。
canonicalの1/1000 pointをそのまま`1/P`と比較しない。
一時textureのアスペクトがAと違えば、既存のcontain処理で歪ませず描く。

**AI到着時の不変保証（決定）**: アスペクト基準・bbox・構成・rect / ppp・指定値・操作が同じなら、
AI差替えだけでpaint / hit / capture / navigatorの表示geometryと連結の配置長・scroll範囲を変えない。
no_up / no_downの4通り、5倍上限の有無、単ページ / 見開き / 連結のいずれも対象。
例: P=1・trim後O=400px・高さ800px・拡大禁止では、4倍AI到着前も後も400px表示。
800pxへ拡大するr2 / r3案は、今回の利用者決定により撤回する。AI出力の画素丸めはsampling側で
扱い、A / O・fitの外枠を解き直す理由にしない。画質だけを更新する。
PDFも上記固定A / Oを保ち、再レンダ / AI結果採用だけでgeometry・需要を変えない。
RAWの本来の寸法判明やsource編集による正当な変更はAI到着とは別の境界で扱う。
frozen captureはその時点のA / O・geometryを保持し、AI完了の通知で変形させない。

### 4.4 Zの表示基準（Q11新規）

現行Zの単ページ / 見開きの差を維持する。指定fitの倍率へ一律にZ倍率を掛けない。

- **単ページ（単独slotを含む）**: `ResolvedZTransform`がPage・倍率制限なし・自由回転なしへ
  解決する既存経路。照準も確定表示も実image rectを基準とし、指定幅 / 高さはZ中の枠にはしない。
  `full_image_zoom`はページ全体Page fitとの比で、SpecifiedScaleの通常fitとの比ではない。
- **2ページ見開き**: 先に選択中のfit・limitsで合成幾何と実効gapを解く。
  SpecifiedScaleもそのfit_scaleを既存`resolve_spread_zip_zoom`へ渡す。
  照準はその実表示基準、確定Zは既存の単ページ幅約1.2倍を基準にした専用solve。
  `fit_scale × resolved zoom`が最終倍率で、単ページのfull_image_zoomを流用しない。
- 連結・動画・パノラマ・分析などの既存Z非対象は維持する。入力・factor・pan・hold / toggleの
  ownerとkeymapは変更しない。指定値は消さず、Z解除後は保持していた指定fitと通常zoom / panへ戻す。
- 通常指定fit、Z照準、Z確定、解除後、resizeは同じ表示policyからその時点のgeometryを再解決する。
  Zの倍率比が同じでもrect / pppが変わればPDFの必要pxも変わる。要求契約は§5.2による。

## 5. 表示単位・媒体・窓（Q5〜Q9）

| 場面 | 決定契約 |
| --- | --- |
| 単ページ・画像フォルダ | 同じfit入口。通常ファイル、ZIP/CBZ・PDF/EPUBページ、RAW、アニメーション画像の既存分類を保つ。フォルダ検出や専用modeを追加しない |
| trim / 分割 | §5.1の既存経路別入力を使い、分割がtrimに優先する。canonicalはアスペクト基準、原寸は§4.3のAI前pixel基準。AI到着だけで表示寸法を変えない |
| 見開き | Original以外の既存高さ合わせを維持。2枚と間隔を合成した全体へ枠を適用（Q5）。各ページへ別々に800pxを適用しない。左右・読み順は変更しない |
| 単独ページのslot | compositionが仮想slotを持つ場合は空き側も含める。中央単ページは1枚。SpecifiedScaleはPageと同じcomposition。縦連結Width専用の中央単独表紙特例を自動追加しない |
| 縦 / 横連結 | 選択 / 変更時に中央の現在unit（単ページまたは見開き）を基準に解き、その自動倍率を保持（Q6、§5.3）。幅 / 高さとも同じ式。周辺unitや全stripを別々の指定枠へfitしない。gap・可視判定・scroll範囲は最終描画寸法へ揃える |
| flow変更 | 既存Page / Width / Heightへの既定fit変更を維持（Q6）。変更後に指定サイズを選べる。flowを理由に新方式を選択不能にしない |
| 90° / 270° | §5.1の既存経路別trim入力を維持（Q7）。高さ800pxは回転後の縦方向。EXIF / 保存回転を再加算せず、既存4方式も変更しない |
| 自由回転・Z・比較 | free rotation時の既存effective_bbox / trim解除・専用placement・viewportを維持。自由回転AABBへの新しいfitは追加しない。Zは§4.4の単ページ / 見開き別契約（Q11） |
| 動画 / 音楽 | 画像用のみ（Q8）。既存循環guard、native videoのplacement / shader / HUD、音楽の中央表示方式は変更しない |
| resize / F11 / F12 / DPI | 値と選択方式を保持。％は現在image rectへ追従、pxは同じpoint寸法（Q2決定）。小窓でもpxを窓内へ自動縮小しない。連結は同じ保持基準unitで再計算し、現在中央へ基準を移さない。既存clip / pan制約を保つ |
| 複数窓・保存範囲 | 共通指定値＋既存favorite overlay（Q9、§7.1）。各表示先のrect / pppで解く。凍結passive / holdoverはcapture時policyを保持し、新設定を後読みしない |
| prefs OK / import適用 | 指定値だけの変更は既存確定境界で公開し、次のlive描画で再計算（Q9）。選択modeと手動zoom / panは保持。mode自体のprefs変更は選択どおり適用。メニュー等で別方式へ切り替えると既存リセット。同じ方式の再選択はno-op。窓を閉じたり再生成したりしない |

見開き幅枠は間隔を含む。実効間隔`g`、画像合成寸法`Sw, Sh`なら幅候補は`(Bw - g) / Sw`。
指定幅が小さすぎる場合、その表示だけ`g = min(saved_gap, max(0, Bw - 1/P))`とし、
画像へ最低1物理px分を残す（Q5）。保存gapは保持。draw / unit layout / hitへ同じ実効gapを渡す。
既存方式の格子丸め・最小表示領域と、指定値の単位変換を混同しない。
極小サイズは既存格子丸めの限界があり、全ページが常に1px以上見えるとは保証しない。

### 5.1 回転＋trim: 現行例外を新方式でも維持（Q7決定、2026-10-09）

§2のsource inspectionで次の差を確認した。旧「すべて回転後・trim後で同じ」という説明を撤回する。

| 現行経路 | 保存回転のあるページ |
| --- | --- |
| 通常単ページのlive描画 | trimを渡し、transformでbboxを回転する（:24028） |
| 見開きlive描画 | 回転ページのtrimをNoneにする。未回転の相方はtrimを維持（:40758） |
| 縦 / 横連結unit | 回転ページのtrimをNoneにする。ただし分割はrotation込みのsliceを優先（:37562） |
| display-unit capture生成 | 単ページを含め回転ページのtrimを落とす（:10465） |
| 通常PDF要求のbbox取得 | 回転ページはNone（:37103）。live単ページ描画と同じbboxではない |

**決定: SpecifiedScaleも既存方式と同じtrim入力経路・上表の例外を維持する。**
r2 / r3の「新方式だけrotation＋trim入力を統一する」推奨を撤回する。今回はfitの計算を追加し、
各consumerのtrim取得・保存回転の条件を変更しない。新方式のための共通trim ownerも追加しない。
live単ページ→captureでの切取り範囲の差など、上表の経路間差が今版では残ることを仕様として明記する。
draw / hit / UVは同じ経路内の解決済みgeometryを使い、captureは既存capture経路が取得したbboxを保存する。
PDFのbbox取得は既存規則を維持する。ただし§5.2の最終絶対需要は実際の描画full-image extentから
求め、初回の暫定target / bboxと最終需要を混同しない。全経路が同じtrim入力という保証は撤回する。
`fs_page_content_bbox`の分割優先、自由回転時の`effective_bbox`解除、既存Zのtrim入力も維持する。
統一は[backlog §1.358](next-release-backlog.md)へ別件として起票した。
その作業の設計・費用を1.342の質問へ戻さず、本版のSpecifiedScale実装範囲には含めない。

### 5.2 PDF要求・Remote（Z要求はQ11）

幾何だけ変えてraster要求をPageのままにしない。通常指定fitの枠解決は、既存
`PdfDisplayTarget`の数値と4種のfitへ次のように投影できる。

- 両軸: 解決済みpoint枠に描画先Pを掛けた物理width / height＋Page（px指定も同じ変換）。
- 幅だけ: width指定＋Width。未指定height欄は実表示領域（高さ制約としては使わない）。
- 高さだけ: height指定＋Height。未指定widthは実表示領域。
- 両方なし: 実表示領域＋Page。no_downscale時の既存Original要求、native cap / headroomを保つ。

これは枠を表す投影であり、**最終的なraster demandは実際の表示geometryに合わせる**。
新方式は通常 / 手動zoom / Z照準 / Z確定の各描画で、§4.3・§5.1の入力から解決した
ページごとのfull-image extentとpppを使う。見開き / 連結はgap / slot / limits / Zを
合成unitで解いてから各ページへ分配し、合成幅を左右のPDFへ二重に渡して別々にfitしない。
clip後に見えている小片の長辺だけを要求しない。full-page rasterなので、その拡大率で
ページ全体を描くための物理長辺が必要。trim効果を含んだfull-image extentへもう一度bboxを
掛けたり、保存回転で軸を二度交換したりしない。

#### PDFの原寸・需要・結果採用の一方向契約（r3）

依存は **ページmetadata → 固定A / O → 表示geometry → 絶対需要 → raster結果** とする。
raster結果はsampling / texture / source座標へ採用するが、A / Oへ戻さない。
headroomや最小 / 最大 / native capを適用した要求値を、次frameの原寸にもしない。
手動zoomは原寸制限の後に掛ける現行順を維持するため、zoomによる需要増加は正当だが、
その結果を原寸へ採用して再びzoom / headroomを掛ける循環は禁止する。
retained rasterが需要より大きい場合も、既存cache許容・再利用の判断だけに使う。
AI用native再レンダとfinal保持の優先は既存のまま、どちらの画素寸法も新方式PDFのOへ戻さない。

初回loadは現在page boxを捨てている（app.rs:69401）。既存PDF結果に含まれる
`page_size_points`と`content_type`を、同じload completionのcontext / source / generation検証を
通して採用できるようにする。Aは既存page layout owner（ui_fullscreen.rs:16702）、
content type / native寸法はそのページの既存metadata ownerへ渡す。別の原寸cache・DPI設定・
待機flag・UI同期metadata読込・新しいworker / IPCは作らない。PDF結果の既存payloadに必要な
metadataを保持する型境界も実装範囲に含め、表示用pixel source_dimsと混同しない。
再レンダcompletionが同じmetadataを持ってもA / Oを変えず、描画画像だけを差し替える。
古いsource / 別contextの結果でページmetadataや原寸を更新しない。

寸法 / content type未判明の初回要求は既存loaderの暫定targetで行い、raster寸法をOの代用品にしない。
通常の完成PDF表示へ採用する時点でpage boxとcontent typeを揃え、指定fit / 原寸制限から
最終需要を一度解く。暫定画像から完成表示への変化と、同じmetadataでの再レンダ採用を区別する。
既存page metadataを正しいcontextへ採用できないと判明した場合は、第二のmetadata所有者を
足して迂回せず、止めて設計担当へ報告する。

例: 正方形vector PDF、P=1、指定高さ100px、縮小禁止、手動zoom=1.2。
現在のrasterが4096 / 5407 / 7138 / 8192pxのいずれでも、固定Oは4096×4096px。
原寸制限後は4096px、zoom後の表示長辺は4915.2px、headroom込み要求は5407pxになる。
5407pxの結果をsource_dimsへ採用した次frameも表示4915.2px・需要5407px。
要求が5407→7138→8192pxへ増幅したり、表示枠まで拡大したりしない。
丸めによるrasterの縦横比差は既存containで扱い、Aや原寸をrasterへ追従させない。
この収束は§9の結果採用を通す回帰で保証する。要求値の比較だけで循環を隠す案は採らない。

#### Z中の要求基準と通常復帰

現行:12539は単ページZの`full_image_zoom`を送り、:69762 / :69867は通常
`fs_pdf_display_target`由来のbase_pxへzoomを掛ける。指定枠100pxをそのままbaseにすると
Page基準のZ倍率とは一致しないため、**SpecifiedScaleではこの掛け算を再利用しない**。

- 単ページZ: 照準・確定で実際に解決されたPage / 制限なしgeometryから需要を算出。
  単独slotも同じ原則。Page基準full_image_zoomを通常の指定fit targetへ掛けない。
- 見開きZ: 指定fit由来の照準geometryまたは専用Z solve後の各ページgeometryから算出。
  `maybe_rerender_pdf`は現在idxだけを扱う（:12922）ため、それだけを呼んで両ページ対応とはしない。
  新方式では表示中の両PDF occurrenceへ各需要を既存request ownerで送る。既存4方式の
  「PDF見開きZでは再レンダしない」という既知制限はこの版では変えない。
- 絶対的な必要pxを既存raster要求値へ投影し、headroom 1.10・最小256 / 最大8192・
  scanned rasterのnative cap・AI native入力 / retained finalの既存優先を維持する。
  full-image extent＋Page / bboxなしで表すか同じextentから長辺を直接算出し、
  既存`request_pdf_rerender_at_target`へ渡す。wire tag・layout・IPC版・worker ownerは追加しない。
  headroom・capは一度だけ適用する。通常指定fitで有効なno_downには既存Original要求の
  最低長辺4096px（pdf_loader:5010）を維持するが、Page・制限なしへ解決した単ページZへ
  raw設定のno_downからOriginal要求を持ち込まない。raster native capは既存どおり優先する。
- 例: 正方形vector PDF、通常は高さ100px、ppp=1・image rect 1000×1000でZ確定。
  最終geometryが1000×1000なら必要長辺1000px、headroom込み約1100pxを要求する。
  通常fit targetの256px下限へPage基準zoom=1を掛けた256pxに留めない。
  native capのあるscanned PDFは1000px未満へ制限され得るため、この例の要求とは区別する。
- Z解除は保持した指定fit / limits / 通常zoomで再解決。通常需要へ復帰し、既存cache許容率・
  retained raster / AI再利用規則に従う。Z専用targetを通常fitの保存値へ上書きしない。
  resize / DPI / 実効予約変更は、factorが同じでもgeometryの必要pxが変われば再評価する。

#### 要求の重複抑止と所有

`:12539`の(idx, zoom)比較だけではZ進入 / 解除やresizeを網羅できない。
また`:69872`の既存request-at-targetは進行中jobをcancelするので、毎frame直接呼ぶ案は採らない。
既存`fs_zoom_pdf_rerender_idx / fs_zoom_pdf_rerender_zoom`の2フィールドを、**同じcontextに属する
単一の型付きrequest比較owner**へ置き換える計画とする。新しいpending / bool / 3番目のcacheは足さない。
旧方式の(idx, zoom・既存2％閾値)比較はlegacy variantで保持し、挙動を変更しない。
新方式variantは既存source identityに属するpageごとの絶対target_pxを比較する。
同じ要求を再送せず、変わったpageだけを既存非同期requestへ渡す。相方の同一in-flight jobを
毎frame cancelしない。source変更・mode変更・closeで既存reset / context drop境界を使う。
新方式では:23574〜:23628の通常target更新 / ensureと、入力handlerの倍率request、
Z描画の倍率requestを並行して送らない。入力は既存zoom / panを更新し、描画で解決した
一つの絶対需要をrequest比較ownerへ通す。寸法未判明時の初回loadも、この新方式用
target adapterを使う。旧normal targetがZのin-flight要求を毎framecancelする経路を残さない。
`app.rs`と`app/viewer_context_registry.rs:965`の型・mount / deposit / swapもそのownerに追従させる。
generation / worker / cancel / viewportのownerは変更せず、失敗時の独自retryも追加しない。
凍結描画からlive siblingの需要を発行しない。初回 / partner / 通常zoomも新方式では同じ
需要解決を通し、寸法未判明時は既存loaderへ任せ、UIで同期metadata取得を追加しない。
正しい要求を既存owner / wireで表せないと分かった場合は止めて報告する。

Remoteブラウザのfit操作や数値UIは対象外。本体SettingsをRemoteのfit / zoom ownerへ転送しない。
ページgroup / source / mutation契約を保持し、その他のIPC変更も予定しない。
凍結表示にはmodeだけでなく指定値もcaptureする。同じSpecifiedScaleで値だけ変わっても、
旧captureが新しい幅へ変形する経路を禁止する。

### 5.3 連結の中央unit基準と倍率保持（Q6決定、2026-10-09）

**既存の正確な意味**は§2.1のとおり。各unitのauto fitを毎frame計算するので、単なるscrollや
再アンカーがそのunitの倍率を替えることは通常ないが、「中央ペアを選択時に一度fitして全unitへ
同じ倍率を配る」ownerはない。可視枚数制限の自動zoom例外もある。この2点を既存実装の事実として残す。

SpecifiedScaleは利用者決定に従い、選択 / 指定値変更時の中央unitに対して§4.1〜4.3を解き、
そのauto倍率を周辺unitにも使う。幅だけ・高さだけ・両軸は同じresolverへ渡し、横連結だから高さ、
縦連結だから幅、という強制はしない。全streamの外接寸法へfitしない。周辺unitは元のサイズ比と
既存見開き高さ合わせを保ち、指定枠を超えることもある。gapは別に既存point / 物理丸め規則で足す。
手動`fs_zoom`は既存の倍率ownerのままauto倍率の後に掛ける。

scroll / reanchorで中央idxが変わっても基準unitを変更しない。毎frameのcurrent_posを基準に
再fitする案は採らない。resize / DPI / F11の実効予約変更は**同じ保持基準unit**へ新しいrect / Pを
適用して再計算するので、Q9の％追従と矛盾しない。指定値・limits変更は、その明示変更時の中央unitへ
基準を更新し、通常zoomを保つ。方式を別方式から選び直すときは既存transform resetを維持する。
同じ方式の再選択は既存no-op。flow変更は既存既定fitへ戻る。source / trim / 回転 / 構成が本当に
変わる境界では基準geometryを更新するが、AI差替え・ref texture退去・scrollだけでは変更しない。

必要な保持は**既存連結ビューに属する一つの型付きfit基準owner**に閉じる。
旧方式 / 新方式の基準unit＋A / O・bbox・構成snapshotを表し、参照idx、保存scale、更新boolを
別々のfieldへばらまかない。選択時にsource寸法が未判明なら同じownerでunit identityを保持し、
既存source寸法の採用境界で一度解く。scroll先やAI結果を代用にしない。専用worker、UI同期読込、
待機timer、複数pendingは追加しない。既存layout/captureへ値で渡し、main / detachedのmount / deposit
で同じcontextと共に移送、source close / flow / mode離脱で破棄する。favoriteにこの基準は保存しない。

**可視ページ数の安全策**: 新方式では:37826の枚数超過からauto倍率や`fs_zoom`を変えない。
既存:37841以降の遠いunitを描画対象から除く上限処理を使い、全体のscroll geometryは保持する。
これにより倍率固定とUI応答の上限を両立する。極端に小さい指定では可視範囲の遠いunitが
描画上限の対象外となり得る。旧4方式の自動zoom / 可視上限挙動は今版では変えない。

## 6. 所有境界・接続点

純粋なfit resolverへmode・指定値snapshot・既存limits・実viewport rect / ppp・
§4.3のアスペクト基準とpixel原寸基準・§5.1で解決済みのbbox・compositionのgapを値で渡す。
枠解決・基準倍率を共通化し、式を各consumerで複製しない。
接続対象は単ページgeometry / transform、`fs_image_draw_geometry_for_size`、通常 / Z / navigatorの
見開きfit式、similar preview、連結unit layout、PDF target。
paintから倍率を逆算せず、確定したscale / rect / hit / UVを使う。
captureされたgeometry / 表示単位には同じfit descriptorの指定値を保持し、factory / replayも揃える。
PDFはcapture時のpage metadata / 固定A・Oも同じ入力として保持し、replay時のraster寸法から再算出しない。
既存geometry / layout cache keyがfit条件を含む場合は指定値も含める。
常にPageを使う縮図・overviewへ指定値を誤適用しない。

共通helper接続時も既存4方式のgap・zoom順・Original・丸めを変えない。
新方式のfit式とAI前原寸入力、絶対PDF demandを共有する。rotation＋trimの取得は§5.1の
既存consumerごとの入力を維持する。連結の保持基準は§5.3のcontext-local ownerに閉じる。
PDF request比較cacheの型は§5.2に従って置き換えるが、手動入力・navigation・
loader / worker / generationのownerは追加・変更しない。
既存layout再計算以外にdecode・同期I/O・folder scan・GPU uploadを設定操作へ足さない。

## 7. 保存・転送・環境設定（Q4・Q9・Q10）

新規`Settings::fullscreen_specified_scale`を一つ追加する案。

```rust
struct FullscreenSpecifiedScale {
    width: Option<SpecifiedScaleAxis>,
    height: Option<SpecifiedScaleAxis>,
}
enum SpecifiedScaleAxis {
    PercentTenths(u32), // 1000 = 100.0%
    LogicalPixels(u32), // 表示寸法のpoint相当。Q2決定、未リリースなので改名migrationなし
}
```

Noneは指定なし。％は0.1％単位の整数でNaN / infinity / 等価な複数表現をなくす。
初期値は幅・高さとも`Some(PercentTenths(800))`。新フィールドにserde defaultを付け、
新設定が欠落した既存Settingsもこの両軸80％を使う。各軸Noneは利用者が明示した指定なしであり、
初期値で埋め直さない。お気に入り行のfield欠落は§7.1の共通値継承、旧転送の欠落は転送先保持に従う。
**新規・未リリース**の機能・キー・型なので専用migration・schema version・DDLは不要。
旧指定倍率設定の読み替えはしない。汎用settings_kvを使い、既存fit modeや実データを保持する。
旧版の新variant読込は既存互換性保護に従い、旧版用silent fallbackを増設しない。

転送は既存fit関連と同じ値schemaのexport / import（共通 / favoriteへのrouteは§7.1）。
各軸の単位・範囲・Noneを検証し、混在と両Noneは正当。
無効単位・範囲は既存field拒否 / issue報告へ乗せ、部分正規化して受け入れない。
旧転送文書にキーがなければ転送先の値を維持。未知ID保持と5方式の最低1候補も検証する。
新設定はprefs draftの編集対象なので「環境設定外の最新値移送」には分類しない。
保存失敗に独自retry / rollback / pendingを追加せず既存保存・通知契約を使う。

配置は環境設定「見開き / 連結」のズーム/フィット・倍率制限付近。
「指定サイズ」の小見出し、幅 / 高さを2行、各行に「指定する」checkbox・数値・％/pxのComboBox。
現在modeが別でも編集でき、指定方式へ戻ったとき値を使う。
新タブや長い固定横並びを避け、狭い右ペインで折り返す。
[レイアウト指針](preferences-layout-guidelines.md)のsolid scrollbar / available_widthを使う。
preferences/search_indexへ幅・高さ・％・px・指定サイズの検索anchorを追加。
表示領域100％、100%表示時のpx相当（DPI / UI倍率で拡大）、縮小しない優先、
5倍は自動fitだけの上限であること、初期の両軸80％は余白を残す変更可能な設定例であることを短く示す。

Q10の決定範囲は**0.1〜1000.0％ / 1〜32768px**。数値widgetとvalidatorで範囲を共有。
0・負値・超過・小数pxを保存値にしない。ローカル読込範囲補修はsanitize、転送は範囲外拒否。
単位切替は画面から逆算せず数値を維持（px化は丸め、新範囲へclamp）。
OFFはNone、再ONは100％。以前の単位・数値の別保存は作らない（Q10）。OK前は既存draft内に留める。
DragValue等の既存widgetを使い、独自TextEdit parserや確定用キーを追加しない。
TextEditが必要ならime_focus helperを使用。Enter / Escは既存dialog helperとwidget focusの優先を保つ。

### 7.1 共通値とお気に入り別表示状態（Q9決定、2026-10-09）

新しい指定値を既存`FavoriteViewState`の対象へ加える。favoriteに保存するのは
**幅 / 高さそれぞれの未指定・単位・数値からなる`fullscreen_specified_scale`だけ**。
現行でfavorite対象外のfit mode、no_up / no_down、循環除外、固定5倍の定数まで新たに
favorite別へ広げない。手動zoom / pan、連結の基準unit、viewport / DPIも保存しない。
既存8項目とUUID・最長一致・reset / 削除の意味を維持する。

- remember OFF: 共通値だけを使う。保存済みfavorite値を削除しない。ONへ戻せば既存状態を使う。
- ON・新しいfavorite: :80531の既存transitionが初回入場時の有効値を引き継いでseedする。
  指定サイズ値の欠落は下記の継承規則に従い、入場だけでfavorite専用の具体値へ固定しない。
  :80466の`favorite_view_state_for_path`も同じ指定値を含む投影にする。別のpath resolverを追加しない。
- ON・保存済みfavorite: その指定値を適用する。現在地の表示用fit popupへ、prefsと同じ2行の
  指定値編集helperを置く。live変更は同じeffective設定setterと既存メモリcapture / 500ms DB workerへ
  渡し、ページを開き直さない。favorite外は共通値を変更し、既存の共通設定確定・保存境界を使う。
  widgetごとの同期I/O・新しい保存workerは作らない。
- prefsは従来どおり**標準値**を表示・編集する（settings:8589）。OK / importは:8601と
  preferences:2073のrouteで共通値を変更し、現在のfavorite専用値を維持する。favoriteの値を
  変えたい操作は上記live表示設定へrouteし、「標準を編集したのにfavoriteも変わる」挙動にしない。
- 保存は既存settings_kvの共通値とadjustment.dbの`favorite_view_states.state_json`。
  `FavoriteViewState::from_settings / apply_to_settings`、overlay.common、save時のoverlay解除、
  context mount / deposit、reset / OFF / favorite削除に新フィールドを含める。既存fieldやUUIDを失わない。
- **既存state_jsonの欠落互換**: 新フィールド欠落で行全体を拒否しない。favoriteのwire adapterでは
  追加値の欠落を表せるserde defaultを使い、適用時は共通値へ継承する。これは「幅 / 高さを両Noneに
  指定した」という保存値とは異なる。明示変更を確定した指定値は有効な構造体を保存する。既存行の強制一括書換え、
  migration、DDL、schema version変更は不要。単なる読込でfavorite行や共通値を書き換えない。
- **継承解決は変更ではない（r5 P2対応）**: 既存のcapture / reconcile ownerで、実効指定サイズ値と
  保存済みfavorite stateを比較するとき、欠落した指定値を共通値へ解決したことだけによる差分を
  変更から除外する。保存stateの欠落をメモリ上でも維持し、その差分だけでは
  `favorite_view_writes.note_at`を呼ばず、DB書込みを予約しない。idleのreconcile、離脱時capture、
  終了時reconcile / flushにも同じ規則を適用する。指定値への**明示的な利用者変更だけ**が
  favorite専用値を作る。現在地の指定値編集を既存setterで確定する境界から同じstate ownerへ渡し、
  継承後の値を`from_settings`で採っただけで明示変更と判定しない。共通値の変更もfavoriteへの
  明示変更ではない。他の既存8項目の正当な変更でstateを保存する場合も、指定値への明示変更が
  なければ欠落を保持する。明示両Noneと欠落を混同せず、専用の書込みworker / 同期boolは追加しない。
  根拠は現行[app.rs:83269](../src/app.rs#L83269)の実効state比較と:83274の予約、:83299の離脱capture、
  :83422のidle reconcile、:83557の終了時reconcile。継承解決だけの比較差をここで除外しなければ、
  共通値を具体値として固定して次の入場で新しい共通値を継承できなくなる。
- **転送**: 共通の指定値だけを転送する。favorite UUID / path / 専用行とruntime overlayは既存除外のまま。
  通常のprefs draftは標準snapshotだが、export helper単体もoverlay.commonから共通値を取るgetterに
  接続する（capture_export:185のplain getterが有効値を直接取る前提はこのfieldに使わない）。
  importは標準値へ適用するrouteを使い、既存favorite行へコピー・上書きしない。旧転送に新キーが
  なければ転送先の共通値を保持。キーがあれば両Noneを含め検証済みの値をそのまま適用する。

初期共通値は幅80％ / 高さ80％。既存favorite行の追加値欠落はその時点の共通値を使い、
共通値を利用者が変更済みでも固定の両軸80％へ戻さない。既存favorite表示stateのJSONを破棄する案、
別のfavorite指定値テーブル、標準とfavoriteを独自に同期するboolは採らない。

## 8. 簡素化とdetached憲法

[CLAUDE.mdの簡素化規則](../CLAUDE.md)と[detached計画 §2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要)を読んだ。

- modeは既存enumのみ。指定ON bool、cycle index、窓別override、包含集合、変更済みflagを追加しない。
  軸はNone / ％ / pxの型で、不正な「有効だが単位なし」をなくす。
- モーダルで割り込みを消す案: 編集は既存prefs draftに閉じOK / import適用で一括確定する。
  fitは純粋描画計算なので、閲覧・resizeを止める長期modalは不要。
- 設定変更で窓を閉じて作り直す案: 表示領域の値からfitするだけで済み、余計な閲覧手順とownership変更になるため採らない。
  独自live-rebuild / async handshake / 復元状態も不要。
- resize / DPIを保存状態にしない。現在のrect / pppで解き、凍結表示は既存capture値を保持する。
  source generationやplacementをfitのproxyにしない。
- OFF値の別記憶、画面を読んでの単位換算、未設定時の自動cycle除外を採らず組合せを減らす。
- Q3決定に従い、新方式はAI前の原寸に固定する。AI結果の到着で表示倍率を変える組合せを
  なくし、sampling更新へ限定する。通常画像の非AI原寸規則と旧4方式は維持する。PDFは新方式で
  ページmetadataと既存Original最低解像度 / native capからOを純粋計算する。
  最初のraster寸法を保存する案や、要求後にOを更新・復元する案を採らず、自己増幅する状態の組合せをなくす。
- Q7決定に従い、trimの既存経路差を今版では維持する。新方式だけの共通trim入力ownerを
  追加せず、統一は§1.358へ分離する。異なる経路のbbox一致を追加stateで模倣しない。
- 連結を毎frameの中央unitへ再fitする案はQ6の倍率固定に反するため採らない。旧方式の
  per-unit純粋fitを新方式にも使う案は、基準が中央ペアという要件を満たさない。
  §5.3の一つの型付き基準だけをcontext-localに保持し、scale cache / 更新bool / 別pendingを増やさない。
  可視上限では既存の描画対象制限を使い、倍率を変更する組合せを新方式から除く。
- favoriteは既存overlay / UUID / DB workerへ加える。別の指定値テーブルや同期ownerを作らない。
  wire上の新field欠落と、明示両Noneを区別するための欠落互換は保存データ保護のみに使う。
- Z専用の保存target・別pendingを増やす案は採らない。§5.2の絶対px需要と既存request gateの
  型付き置換で、通常fitのtargetを保存・上書き・復元する組合せを除去する。

**構造的変更である理由**: サイズを決める幾何ownerへfit policyを追加し、単ページ・見開き・連結・
draw / hit / capture / PDF要求を同じ値へ接続する。detached症状guard、時間待ち、rect一致捕捉、
窓の誤同定やviewport再生成で直す案ではない。host identity / placement reducer / generation /
focus / captureのowner、既存reserve計算を変更しない。

以下の承認未記録という記述はr5の設計改訂時点の履歴。受領済みの設計・構造合意と現在の実装状況は§15に記録する。
viewport / detached経路へ触れる実装前に、**ClaudeCodeと独立reviewerがこの構造判定へ合意する**。
本案がレビュー済みだとは記録しない。合意後、実装時にdetached §11へ実際の変更範囲だけを記録する。
記録案は§1.342、shared fit resolver / transform、ui_fullscreenの各consumer・capture・PDF要求、
AI前原寸adapterと派生texture / capture入力、連結の型付きfit基準、app.rsの絶対PDF要求adapterと既存比較cacheの型置換、
PDF load completionのpage metadata採用と固定原寸入力、既存page layout / content typeへの接続、
viewer_context_registryの同じcontext-local fieldの型・移送境界、必要なsnapshot型境界、
設定 / FavoriteViewStateと既存overlay・projection・workerの接続 / prefs・live fit popup / transfer、回帰。
既存4方式はlegacyの表示・trim / Z / request判定・連結の可視上限時zoomを維持する。
理由は上の所有境界共通化、
非変更境界はhost / placement / viewport生成 / focus。別owner変更が必要なら先に再設計・合意する。

## 9. 実装時の検証・文書更新

1. **純粋計算**: 片軸 / 両軸 / 両None、％ / px / mixed、縦横比、各範囲端、大小画像、
   no_up / no_downの4通り、5倍上限の直前 / 一致 / 超過、manual zoom、P=1 / 1.25 / 1.5 / 2とUI倍率。
   Q2決定の指定800px=800point、物理幅 / 高さはP倍になること、Originalは物理1:1を固定。
   default幅80％ / 高さ80％の内接、両None fallback、既存4方式の幾何不変。
   初期値と大きい表示領域の組合せでも自動fitがAI前原寸の5.0倍を超えず、no_upは1倍、
   手動zoom / Zは5倍超を許すことを確認する。
2. **表示単位**: 高さ違い見開き、左右読み、指定幅とgap境界、virtual slot / 中央単独表紙、
   trim / 分割・90° / 270°・RAW preview→full / AI差替え。paint / hit / UV / navigator / Zとscaleの一致。
   アスペクト基準Aとpixel原寸Oを別入力で検証。PDF page box（1/1000 point）の倍率を変えても
   同じ比率とcontent typeなら固定Oが同じこと、比率変更ならOの比率だけが追従することを確認。
   Vectorの長辺4096、Rasterのnative cap優先、任意の現在raster寸法がOへ混入しないことを固定する。
   no_upありのtrim後O=400px→4倍AI・高さ800px・P=1は400px→400pxを固定。
   no_up OFF / no_down / 両ON、5倍上限、denoise→upscale→final / comic / retained AI、GPU clampを通し、
   AI後textureを原寸へ戻せば失敗する採用→次frame回帰にする。paint / hit / capture枠、連結長・scroll範囲を比較。
   AI画素丸めをAの変更として扱わず、明示的なcanvas / bbox変更とRAW寸法判明は別境界で検証。
3. **連結**: 縦 / 横とも幅のみ / 高さのみ / 両軸で中央pair基準を解く。サイズの違うunitへscroll / reanchorしても
   保持auto倍率・refが不変。pixel丸め・gap・描画長・scroll範囲を比較する。resize / DPIは同じrefへ再計算、
   値 / limits変更は中央unitを取り直し、手動zoom保持。mode / flow / source離脱でrefを破棄する。
   寸法未判明の選択→metadata採用、refのtexture退去→再採用、AI差替え、context mount / swap / captureを通す。
   極小指定・サイズが異なる周辺unitで可視枚数が上限を超えても新方式はfs_zoom / auto倍率を変えず、
   既存の遠いunit除外とprefetch上限を守る。旧4方式のper-unit fit・自動zoom例外は不変。
4. **handler**: key / ring / gesture / mouse3ボタン / padが同じownerを通る。
   除外なし / 明示指定方式除外 / 1候補 / 現在対象外 / 全5除外 / 未知ID保持 / 従来4だけ除外。
   直接pickerには5方式。動画 / 音楽 / 編集では循環しない。
5. **保存・prefs・転送**: Settingsの新設定欠落は両軸80％、mixed / None / 範囲のround-trip、無効単位・数値拒否、
   全field分類、旧転送文書でdestination維持、OK / Cancel / import、IME Enter / Esc、狭幅widget到達。
   remember OFF / ON、新規favorite継承、既存state_jsonのfield欠落（全既存項目保持）、両Noneとの差、
   最長一致、行先projection、標準prefsとlive専用値のroute、reset / 削除 / context切替・500ms書込 / dropを通す。
   **欠落継承のlifecycle回帰**: 指定サイズfieldを持たないlegacy JSONを読み、favoriteへ入場
   →複数idle frameの実reconcile→離脱 / 終了の実capture / flushをそれぞれ通す。
   他項目を変更しない条件でメモリstateの欠落が維持され、書込み予約が一度も作られず、
   store commandも送られないことを確認する。その後共通指定値を変更→再入場し、変更後の共通値を
   継承することを確認する（終了経路は永続JSONを再読込して再入場）。予約後に取消しただけでは合格にしない。
   次に現在地で指定値を明示変更し、その操作だけが専用値を作って既存ownerへ保存予約すること、
   明示両Noneを保持することを確認する。他の8項目だけを変更して保存した場合も指定値の欠落を保持し、
   後の共通値変更を継承できることを固定する。解決済み値のbool比較だけでなく実handler / lifecycleを通す。
   共通値のsave / exportにfavorite専用値・UUID・pathが漏れず、importが専用行を上書きしないことを固定。
6. **context / lifecycle**: main / F12 / F11 / 異DPI、実効HUD / 右予約と抑制ON/OFF。
   capture後の値変更が旧holdover / frozen siblingを変形させない。正しいpppと既存ownerを使い、
   resize / 設定変更でsource reload / viewport再生成を起こさない。PDFの需要変更は既存requestの
   更新 / cancel契約を使い、同じ需要のjobやsiblingのworkerはcancelしない。
7. **PDF**: 初回 / partner / zoomの必要px、両軸 / 片軸 / mixed / gap / rotation、
   no_downのOriginal、native cap / headroom、context適用、既存wire round-trip不変。
8. **Zの実幾何とPDF lifecycle**: 実Z resolver / draw consumerとrequest adapterを通す。
   単ページの通常指定高さ100px→Z照準→確定（1000×1000、ppp=1、必要1000 / headroom約1100px）
   →解除→通常指定fitを検証。factor変更なしのresize（1000→1500）、DPI / 実効予約変更でも需要が追従する。
   no_up / no_downをONにしても単ページZはPage・制限なしになること、解除後だけ制限が戻ることを確認。
   見開きは指定fit・gap・limitsの照準基準を維持し、確定後の左右別geometryとPDF需要を検証。
   片方だけPDF / 両PDF・単独slotも対象。既存4方式のZ幾何・見開きPDFの既存制限は不変。
   同じ需要の連続frameで再送 / in-flight cancelが増えないこと、左右で別の必要pxを送ること、
   大小resize中のsupersessionとsource変更 / closeの既存終端、context mount / swapで要求が混ざらないことを確認。
9. **rotation＋trimの経路回帰**: 新方式で90° / 270°、Book / Page / Auto trim、片側だけ回転した
   見開き、縦 / 横連結、単独slot / split、自由回転の既存trim解除を通す。
   新方式とlegacy4方式の双方で§5.1の経路別例外を固定する。通常単ページのtrim保持、見開き / unit /
   capture / 暫定PDF bboxでの回転ページtrim除外、未回転の相方・分割優先・自由回転解除を検証。
   経路間のbbox一致を期待値にしない。各経路内で画面軸・paint / hit・回転 / trimの一度だけ適用を確認。
   最終PDF需要はlive geometry由来のfull-image extentに必要なpxを満たし、旧bboxを再適用しないことを検証。
10. **PDF需要の収束（結果採用を通す回帰）**: 実fit / draw geometry・需要adapter・request比較owner・
    load completionの結果採用を通し、**要求→FsLoadResult採用→次frame**を連続して検証する。
    正方形Vector・指定高さ100px・P=1・no_down ON・manual zoom=1.2・現在raster4096pxから、
    表示4915.2px / 要求5407px→結果source_dims=5407px→同じ表示 / 需要となることを確認。
    初期rasterを7138 / 8192pxにした場合も同じ幾何 / 需要へ解き、保持画像の再利用判断と分ける。
    複数frame後も5407→7138→8192pxの増幅、再送、job cancelが起きないことを確認する。
    純粋helperの同一要求比較だけでは証拠にしない。旧rasterをOへ接続すれば失敗する回帰にする。
    no_up / no_downの4通り、片軸 / 両軸、raster native cap（256px未満も含む）、丸め / 最大8192、
    単ページ / 見開き / 連結・rotation＋trim・単独slot・P=1 / 2も対象。
    Z進入 / 解除、resize / DPI / zoom変更では需要が正当に変わり、各結果採用後は再び安定する。
    初回metadataの採用前後、thumbnail不在、AI native / final差替え、source変更 / close / context swapを通し、
    metadataの真正な変更以外でA / Oが変わらず、別contextの結果が混ざらないことを確認する。

snapshotは純粋な設定描画helperをtests/ui_snapshotへ渡す（App全体のsnapshotにしない）。
循環5checkbox、指定2行、初期の幅80％ / 高さ80％（両軸ON）、片軸のみ / mixed / None、ライト / ダーク、通常 / 狭幅、説明折返し、限界値を撮る。
同じhelperのlive fit popupも標準 / favoriteの表示と幅・高さ・上限説明を撮る。
必要なら単ページ・見開き幾何fixtureも追加。PNGを目視し、widget欠け・scrollbar重なり・説明の読みやすさを確認。
入力routingの証明にはhandler testを使う。

実装時gate: cargo fmt、焦点回帰、`cargo test -p mimageviewer --lib`（pipeなし・実exit）、
通常 / portable core check、ui_snapshot、`python scripts/check_ui_glyphs.py`。
新機能期待をテストで先に固定し、既存bug修正が必要なら有効redを別途確認する。
実機用buildは実装依頼とbuild/test方針に従い、製品はエージェントが起動しない。
**本ラウンドは文書のみで、製品テスト・buildは実行しない。**

実装時co-update: display-pipeline、keymap-specの5方式順、必要なkey-customization / ring文書、
backlog、prefs検索、manual fullscreen / settings、detached §11。実装・検証・実機結果を本計画へ集約。
manualには「初期は幅・高さ80％の枠へ内接して余白を残す設定例。用途に合わせて変更できる」、
表示寸法pxのDPI / UI倍率への追従、AI前原寸の自動fit上限5.0倍と手動zoom / Zの例外を記す。
動画 / Remoteの仕様変更が必要なら、今回の範囲へ黙って追加しない。

## 10. 利用者決定（2026-10-09、全質問決定済み）

幅・高さを独立かつ任意に指定、両軸なら内接・片軸ならその軸で倍率を決めること、
既存方式との切替と％ / px両対応は依頼済み要件。Q1〜Q11のIDは追跡用に保持する。

### 10.1 決定済み仕様

| 質問 | 利用者決定（2026-10-09） |
| --- | --- |
| Q1 | 100％は現在の画像表示領域。固定HUD / 右パネルの予約を除く。F11抑制後は広がった領域を使う |
| Q2 | 推奨どおり、pxは他の表示寸法設定と同じ100％時のlogical point相当。800pxは800pointで、Windows拡大率とアプリUI倍率に追従する。両方100％なら800物理px、Windows150％・アプリ100％なら1200物理px。画像生成解像度やOriginalの物理1:1は変更しない |
| Q3 | 通常画像の既存「拡大しない / 縮小しない」を維持。新方式のAI結果は拡大前の原寸を使い、到着だけで表示を大きくも小さくもしない。単ページ / 見開き / 連結・AI後の合成textureも対象。PDF固定A / Oは§4.3のまま |
| Q4 | Q4-aは推奨と異なり、初期幅80％・高さ80％の両軸指定へ決定。枠へ内接して周囲に余白を残す設定例で、利用者が変更する前提。Q4-bは推奨どおり、指定サイズの自動fitだけAI前原寸基準の固定5.0倍上限。「拡大しない」は1倍まで、「縮小しない」は1〜5倍、両方ONは1倍。手動zoom / Zは5倍超を許し、上限入力欄を追加しない。見開きは高さ合わせ後の全体への自動倍率を制限し、内部の高さ合わせ倍率までは制限しない。画質低下を避ける元の要望に対する確認ケースにもする。両軸未指定fallback / 明示除外による循環は維持 |
| Q5 | 見開きは2枚と間隔を合わせた全体へ指定枠を適用。既存高さ合わせと空き側slotを維持。極小指定の実効gap補正は表示だけへ適用 |
| Q6 | 連結は選択 / 変更時に焦点となる中央page pair（単ページはその1枚）を基準にし、scrollで倍率を替えない。幅 / 高さとも同じ式。既存flow変更時の既定fitは維持。コードのper-unit計算・可視枚数zoom例外との差と接続は§5.3 |
| Q7 | 新方式も既存trim入力経路と回転時の例外を今版では維持。全方式の統一は別件§1.358。費用を1.342の追加質問にしない |
| Q8 | 画像 / 本のページのみ。動画・音楽はfit modeを持たず、その表示方式は変更しない |
| Q9 | アプリ共通の指定値を持ち、既存remember_favorite_view_state / favorite_view_state_for_pathへ参加。favoriteには指定幅・高さ・単位・未指定を保存し、共通値の転送と専用値の保持を分ける（§7.1） |
| Q10 | ％0.1〜1000.0 / px1〜32768。単位変更は数値維持、pxは整数へ丸め範囲へclamp。OFFから再ONは100％。OFF値の別保存なし、既存draft / widget / IME境界を使う |
| Q11 | 単ページZはPage・制限なし、見開きZは選択fitを照準基準にする現行差を維持。新方式のPDFは実Z geometryに合わせた絶対需要を送り、解除 / resize / 結果採用後の収束も検証。既存4方式のZ / PDF制限は今版では変更しない |

## 11. 独立設計レビューへの対応（r2、2026-10-09）

以下はr2当時の記録であり、Q3 / Q7の最終方針は§10 / §13が優先する。

3件ともHEAD `2d327da31`のコードで確認し、指摘に同意する。反論・製品コード変更はない。

| 指摘 | 確認・改訂 |
| --- | --- |
| P2: Z倍率とPDF要求の基準不一致 | :1127 / :1815 / :12539 / :69762 / :69867を確認。§4.4で単ページと見開きの差を明記し、§5.2を実geometry由来の絶対需要とcontext-local gate置換へ改訂。§9に進入・照準・確定・解除・resizeと両ページの回帰、Q11を追加 |
| P2: canonical寸法と原寸制限の両立 | display-pipeline:34 / :1705、ui_fullscreen:8746 / :36428を確認。§4.3でアスペクトAとpixel原寸Oを区別し、PDFの単位変換境界と制限時の寸法変化を明記。不変保証を限定、Q3を変更 |
| P2: 回転＋trimの現行例外 | :24028 / :40758 / :37562 / :10465 / :37103 / :17114を確認。§5.1へ例外表を追加。新方式だけ共通入力で揃え、既存4方式の変更は別判断とする推奨案。§9の経路別回帰とQ7を変更 |

Q1 / Q2 / Q5 / Q6 / Q8〜Q10は推奨を維持。Q3 / Q7は変更、Q4は同じfit式とQ7の入力ルールを区別する説明補足、
Q11は新規で、すべて利用者判断待ち。
再レビューが必要。文書のdiff / EOLを確認するだけで、cargo / snapshot / buildや製品起動は行わない。

## 12. 独立設計再レビューへの対応（r3、2026-10-09）

以下はr3当時の記録。PDFの一方向契約は維持し、利用者回答で変更した範囲は§13へ記録する。

P2「PDF再レンダ結果から要求解像度が自己増幅する」に同意する。HEAD `ab99c0a6b`で
app.rs:69974 / :69988 / :78199、displayed_image_transform.rs:365、
pdf_loader.rs:5010 / :5017 / :5118 / :6556〜:6563を確認した。
r2の「PDFのOに現在rasterを使い、そのgeometryから需要を作る」案を撤回する。
要求比較は増幅したtargetの再送を防げないため、それだけで直るとは扱わない。

§4.3で新方式のPDFだけpage boxと固定Original参照長辺 / native capからA / Oを解く契約へ改訂。
§5.2でmetadata→geometry→需要→結果の一方向依存、初回metadata採用の型境界とcontext検証、
既存pixel source座標を変更しない境界、4915.2px表示 / 5407px需要の収束例を追加した。
§6 / §8へcapture入力・実装接続点と簡素化を反映し、§9項目10へ実completion採用を通す
要求→結果→次frameの回帰を追加した。自己増幅を再送guardで隠す案、最初のrasterを保存する案は採らない。

Q1〜Q11の番号・件数を維持する。Q3だけPDF固定原寸の推奨と通常画像との違いを説明追加した。
他の質問と推奨は維持、全11問は利用者判断待ち。独立再レビューも未完了。
製品コード・保存データを変更せず、文書のdiff / EOLのみ確認する。

## 13. 利用者回答の反映（r4、2026-10-09）

以下はr4当時の記録。後続回答でQ2 / Q4も決定し、現在仕様は§10.1が優先する。

Q1 / Q3 / Q5〜Q11とQ4の初期幅80％を決定仕様へ移した。Q2は表示用pointと生成用物理pxを
§2.1で区別し、他の表示寸法と揃えるLogicalPixelsを推奨する。物理pxを維持する理由は見つからない。
当時は初期高さNoneを推奨し、px基準・高さ初期値・自動fit上限の3問を残した。
後続のQ4-a回答は高さ80％を採用したため、この初期高さ推奨は撤回する。

Q3はr2 / r3のAI到着時の条件付き保証を撤回し、新方式のAI前O / Aと各派生textureの対応を
固定する。PDFのmetadata→geometry→需要→結果採用の一方向契約と収束回帰は維持する。
Q6は現行のper-unit計算・scroll時非reset・可視枚数超過のauto zoomを照合した。中央unit基準を
保持する新方式の型付きownerが必要で、可視上限は既存の描画対象制限を使う。旧4方式を変更しない。
Q7は新方式だけのtrim入力統一案を撤回し、例外表を維持。全方式の統一は§1.358へ起票した。
Q9は既存FavoriteViewState / overlay / 行先投影 / 保存workerに指定値を追加する仕様へ改訂し、
既存JSON欠落互換、標準prefsと有効値のroute、共通値だけの転送、live編集入口を記述した。

新しいユーザー仕様を保持するためのfit基準と、既存source / favorite ownerの責務を明示した
設計変更であり、scrollやAI到着へguard / retry / 別窓判定を足す症状パッチではない。
detached §11への記録は実装時に実際に触れた境界だけを記録する。r4は独立レビュー待ち。
文書差分・リンク先・CRLFを確認した。製品コード変更・cargo / snapshot / build・製品起動・commitは行わない。

## 14. v4.5.0出荷後の前提再照合（2026-10-09）

利用者指示により、masterへfast-forward済みのnext-viewer `840c309ec` で設計を再開した。
以下はsource inspectionとr4基準との差分照合であり、製品起動・テストによる観測ではない。
再照合時に設計前提との矛盾は見つからず、既存の決定仕様・推奨案を維持した。
続く利用者回答により§10.1のQ2 / Q4を確定し、初期高さだけ当時の推奨と異なる80％へ変更した。
過去のレビュー記録とfile:lineを残し、現行コードの主な接続点をここに記録する。

| 再照合対象 | `840c309ec`のfile:lineと結果 |
| --- | --- |
| 出荷済みfit循環 | [settings.rs:3664](../src/settings.rs#L3664)のenumと`all / cycle_id / next_for_flow`は既存4方式。除外集合の既定は:7543で空、:9630以降で全候補除外を補修する。新方式はまだ未実装。§1.343は出荷済みの[keymap仕様](keymap-spec.md)を参照する |
| 入力再利用 | [keymap.rs:6621](../src/keymap.rs#L6621)は既存0 / Numpad0、[ring_shortcut.rs:1074](../src/ring_shortcut.rs#L1074)は既存ID。[ui_fullscreen.rs:30134](../src/ui_fullscreen.rs#L30134)のキー入口は音楽を除外する。新しい操作ownerを足す理由はない |
| 表示寸法pxとDPI | [settings.rs:6759](../src/settings.rs#L6759)、[seek_strip_layout.rs:76](../src/video/seek_strip_layout.rs#L76)、[preferences/pages.rs:9440](../src/ui_dialogs/preferences/pages.rs#L9440)は100％時のpoint / px相当。[ui_fullscreen.rs:4207](../src/ui_fullscreen.rs#L4207)の固定間隔はpointへ直接使い、[egui/context.rs:451](../vendor/egui/src/context.rs#L451)でnative DPIとUI倍率が物理pxへ反映される。Q2の表示寸法に揃える推奨を維持する |
| AI前原寸の接続 | [final_pipeline.rs:23](../src/ai/final_pipeline.rs#L23)の出力はAI前`source_size`と`used_upscale`を保持する。同ファイルと`displayed_image_transform.rs`はr4基準から差分なし。§4.3のAI前A / Oを使う契約と、全派生textureの入力対応を実装時に確認する条件を維持する |
| PDFの固定原寸・需要 | [pdf_loader.rs:5010](../src/pdf_loader.rs#L5010)は4096pxのOriginal最低長辺、:5024はraster native cap、:5110以降はpage boxを1/1000 pointへ変換、:6559以降はheadroom / cap。[app.rs:72715](../src/app.rs#L72715)は再レンダのpage boxを捨て、:72729以降でraster寸法をsource_dimsへ採用し、:80936のgetterが画素寸法を返す。§4.3 / §5.2の固定A / Oと一方向の需要、結果採用後の収束回帰が引き続き必要 |
| 連結の倍率 | [ui_fullscreen.rs:37636](../src/ui_fullscreen.rs#L37636)はunitごとのfit、:37784以降は毎frameの計算、:37828以降は可視枚数によるzoom増加。r4基準からこの経路の変更なし。§5.3の新方式だけ中央unit基準を保持する契約を維持する |
| favoriteの既存owner | [settings.rs:4184](../src/settings.rs#L4184)のstateは既存8項目、:8616以降のoverlayと標準値保護を維持。[app.rs:83222](../src/app.rs#L83222)が行先投影、:83261が変更capture、:83285以降がtransition / seed / 適用。指定値の追加、欠落互換、共通値だけの転送は§7.1へ接続する |
| 転送分類 | [settings_transfer.rs:338](../src/settings_transfer.rs#L338)は循環除外を転送し、現在の`all()`で最低1候補を検証する。新方式追加時は5方式へ追従させ、新設定を分類する。件数テストは実装時のベースに追加分を足して再計算する |

r4基準からの`ui_fullscreen.rs`差分はseek stripのNoArt扱い、`pdf_loader.rs`差分は
テスト用のPDF分析経路であり、ここで使う幾何・需要規則を変更していない。
`settings.rs`の追加設定もfit循環・FavoriteViewStateの既存ownerを変更していない。
出荷済みHUDの変更を本ラウンドで再評価・変更しない。

**残る利用者回答の反映（2026-10-09）**: Q2は表示寸法pxのlogical point基準、Q4-aは
幅80％・高さ80％の初期内接、Q4-bは自動fitだけAI前原寸の固定5.0倍上限へ決定した。
§10.1へ記録し、未決質問節を削除した。初期設定・例・欠落default・snapshot / 回帰・
実装時のmanual説明を両軸80％へ揃えた。片軸None / 両Noneの明示設定は引き続き有効。
本節のv4.5.0後のコード照合と所有境界は維持する。
独立設計レビュー承認・構造判定への合意は、通知されていないため記録しない。
実装はレビュー合意後に着手し、detached §11は実際に触った境界を記録する。
今回は文書・参照先・EOLだけを検証し、製品コード・保存データ・snapshotは変更しない。

## 15. 実装接続と検証結果（2026-10-09）

利用者の実装指示と独立設計レビュー・構造判定への合意を受領し、`ebc6ce987` の設計を基準に
実装へ移った。§11〜§14の「承認未記録」「文書のみ」は各改訂時点の履歴として保存する。
今回の実装差分の独立レビュー、自動gate、確認用build、実機確認は別の検証であり、
設計承認から完了したとは推定しない。下記は未コミット差分の自動検証結果であり、
実装差分の独立レビュー・実機確認は未実施。製品はエージェントが起動していない。

### 接続する所有境界

| 境界 | 実装接続・現時点の記録 |
| --- | --- |
| 設定 / 入力 / UI | `SpecifiedScale` とstable IDを既存5方式循環へ追加。各軸は `SpecifiedScaleAxis::{PercentTenths, LogicalPixels}`、初期は両軸80％。prefsとlive fit popupは同じ2行helperを使い、既存KeyAction / cycle setterへ接続する。動画・音楽のguardと旧MarginFit互換は保持する |
| 共通fit / geometry | `fullscreen_specified_scale.rs` の `FullscreenFitInput` がLegacy、指定値＋A/O basis、単ページZの制限なしCanonicalPageを区別する。共通resolverが任意軸、gap、limits、5倍上限を解き、単ページ・合成unitへ渡す。`DisplayedImageGeometry::layout_full_image_rect` はcanonical fit枠を保持し、実際のrasterは `from_layout_rect` で内接する。paint / hit / UVはsampling矩形を共有する |
| AI前来歴 | 既存edit / final AI / final compositeのsource metadataを同じsource keyへ対応付け、AI前canvasを参照する。RAW・連結・passthroughのsource原寸と、通常の単ページ・見開きで選択された非AI rasterの原寸を既存consumerに合わせる。AI結果の初着、拡大率の割戻し、AI派生textureのGPU clamp後寸法をOの保存先にしない |
| PDF metadata / retained | `StaticPdf` のpage box / content typeを既存completion検証の後で採用し、既存retained PDF source keyにmetadata snapshotを伴わせる。Vector4096、Raster native capの固定Oを解き、source座標用raster寸法と分離する。再レンダ結果をA/Oへ戻さない |
| PDF需要 | context-local `PdfRerenderDemand` が既存Legacyのidx / zoom比較とAbsoluteのsource付きページtarget比較を所有する。通常・相方・manual・Zの新方式需要は安定したfull-page fit枠から導出し、既存headroom / capを一度だけ適用する。同じ需要でjobを再送・取消しない。旧4方式の要求判定と見開きZの既知制限は維持する |
| 連結 / context | `ContinuousSpecifiedFitReference` に選択時unitとbasisを保持し、scrollで基準を移さない。resize / DPIは同じ基準へ再計算する。連結のAは既存source layout基準へ固定し、AI入力の画素丸めで置換しない。PDF需要比較と基準は既存ViewerContextBundleのmount / deposit / swapと共に移送する。環境設定・live picker・flow既定方式の明示的な方式変更時は既存registryのmounted / parked / retiring ownerで基準だけを終端し、途中の描画がなくても旧選択unitを再使用しない。同じ方式の再設定では基準を保持し、別contextのworkerを取消さない |
| capture / frozen | typed fit descriptor、指定値、A/O、倍率制限、canonical fit枠をcapture / replayへ接続する。新方式の捕捉済みpolicyを現在のSettingsやAI / PDF出力寸法から後読みしない。既存4方式のholdover動作は維持する。凍結passive / holdoverはlive siblingへPDF需要を発行しない |
| favorite / 転送 | 既存overlay、行先投影、capture / reconcile / 離脱 / 終了workerへ指定値を加える。欠落継承と明示両Noneを区別し、captureの比較で継承解決だけの差を除外する。明示live編集だけが専用値を作る。prefs / import / exportは共通値へrouteし専用行を変更しない |

上表は実装の接続範囲を示す。自動検証の結果と、未実施の実機確認・独立実装レビューを下記に分けて記録する。
既存4方式、単ページ / 見開きZの違い、§5.1の回転＋trim例外、Remoteのfit非転送は維持する。
detachedへ実際に触れた境界は [detached計画 §11](detached-rework-plan.md#11-リワーク外からの変更記録) に記録した。
viewport predicates、host identity / HWND取得、生成 / recreate、placement / focus / native lifecycleは変更しない。

### 検証の記録

共通resolver / geometryに任意軸・default・logical point・4通りのlimits・5倍境界・gap・
PDF固定原寸・AI差替え・raster丸め・rotation / trim・singleton・Zの回帰を追加した。
実draw / handler、PDF要求→completion採用→次frame、連結基準、context移送、capture、
favorite欠落JSON→入場→idle→離脱 / 終了→共通値変更後の再入場まで検証した。

| コマンド（未コミット差分、通常feature） | 結果 / ログ（`target/`） |
| --- | --- |
| `cargo fmt --check` | exit 0。`E-1342-fmt.log`。最終CRLF整形後にも再確認済み |
| `python scripts/check_ui_glyphs.py` | exit 0、危険glyph 0。`E-1342-glyph.log` |
| `cargo test -p mimageviewer --lib <filter>` | `specified` 63、`fit_cycle` 4、`fit_mode_menu` 1、`settings_transfer` 20、`displayed_image_transform` 43、`continuous_reading` 21、既存snapshot remap 1の計153件（重複あり）、全exit 0。`E-1342-final-focused-*.log` |
| `cargo test -p mimageviewer --lib` | **11,470成功 / 52 ignore / 0失敗、exit 0**（1247.97秒）。pipeなし、`E-1342-full-lib.log` |
| `cargo test -p mimageviewer --test ui_snapshot -- --test-threads=1` | **143成功、exit 0**。`E-1342-ui-snapshot.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0。`E-1342-normal-check.log` |
| 同上 `--features portable` | exit 0。`E-1342-portable-check.log` |
| `cargo test -p viewer_context_audit` / `cargo run -p viewer_context_audit` | 36成功 / 実監査成功、両exit 0。`E-1342-context-audit-tests.log` / `E-1342-context-audit.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | **exit 0**、通常featureのcore / remote service / EPUB PDF workerを `target/dev-runtime/` へ配置。`E-1342-build-dev.log` |
| `cargo test -p mimageviewer --lib video::dsp::` | vendor依存復元後に影響範囲を再検証し、30成功 / exit 0。`E-1342-restored-host-dsp.log` |

snapshotは30枚追加・2枚更新の全32枚を目視した。指定2行・5方式・mixed / None / 限界値、
共通 / favorite、ライト / ダーク、通常 / 狭幅でwidget欠け・説明重なりがないことを確認。
既存notice snapshotの変更は新しい設定節によるscrollbar thumbの26画素だけで、本文は維持する。
複数Harnessのsnapshot結果は一つへ集約し、比較失敗を捨てない。

有効redは狭幅220ptのunit selector到達と、旧方式のsnapshot remapを確認した。
前者はunit widgetの実測幅をwrap ownerへ先に渡すことで修正し、後者はsource失効で
新方式のAbsolute需要だけを終端し、Legacyのremap済みidx / zoom比較を保持することで修正した。
テスト期待値の緩和や既存PNGの一括更新で通していない。redは `E-1342-focused-red2.log` /
`E-1342-snapshot-remap-red.log`、初回lib結果は `E-1342-full-lib-red.log` に保存した。

初回buildはこのworktreeの既存vendor hostのsource identity不一致でexit 101になった。
SDK未配置のため、CLAUDE.md記載の正規vendorバックアップの現ソース一致hostとCRT4本を
このworktreeへ実ファイルで復元した。source identity / CRT正本一致を既存gateで検証してから
再buildし、成功した。C++ソース・SDK・gateは変更せず、APPDATAや他worktreeからhostを輸入しない。
復元前の全lib結果を保持し、依存変更で影響するDSP回帰30件を復元後に再実行した。
復元記録は `E-1342-vendor-restore.log`、初回buildログは `E-1342-build-dev-red.log`。
host PE SHA256は `6ddac733f87b255fe87ae24c55c392af43bcd30bfc07e170b628d2e398ef5890`、sizeは846,936 bytes。

確認用buildはportable featureを使わず、通常の `%APPDATA%\mimageviewer` を使う。
稼働中だった別worktreeのruntimeを停止せず、製品runtimeは起動しなかった。
実機手順は `target/E-1342-hardware-check.md`。独立実装レビュー・実機確認・commitは未実施。

## 16. 独立実装レビューのP2修正（2026-10-09）

`4b8fe1c07` のレビュー2件をコードで確認し、両指摘に同意した。
連結の通常prefetch退去が元textureと派生cacheを除去すると、同じ参照unitでも原寸再取得が
失敗し、確定済みの `ContinuousSpecifiedFitReference::Specified.base` を失っていた。
同じunit・指定値・原寸制限なら確定済みgeometryを保持し、参照unit自身の配置計算にも
同じ値を使う。倍率だけを保持してviewport fallbackでページ配置を変える修正にはしない。
未確定unitは従来どおり寸法到着を待ち、明示値・方式・source変更の既存失効ownerは維持する。
resize / DPIでは同じ物理geometryからfitを解き、scroll / 手動zoomを変更しない。

live数値編集は既存共通投影をメモリへ更新するだけにし、widgetの `changed()` から
`Settings::save()` を呼ばない。環境設定の確定や既存の
`persist_window_state_and_flush`（トレイ退避・終了）の共通設定保存境界が永続化する。
favorite専用値は既存 `FavoriteViewWrites` ownerへ渡すままで、別の保存worker・dirty bool・
debounce・release時保存を追加しない。既存geometryと保存ownerへ戻す構造修正である。

回帰は寸法fallbackのない画像で実際の `update_continuous_reading_prefetch_window` 退去を通し、
単ページ / 見開き、縦 / 横連結のfit・配置・scroll範囲・resize / DPIを確認する。
live編集は実ビューのfit popupから共通preferenceのDragValueを操作し、連続変更・外側releaseで
保存予約ではなく同期保存が増えないことと、既存保存境界を通した再読込を確認する。
両指摘の修正前失敗ログは `target/E-1342-fix-red.log` と
`target/E-1342-fix-live-red.log`。最終gate結果は `target/E-1342-fix-msg.txt` に記録する。

2026-10-10の修正後検証: 焦点111件（重複あり）、全lib **11,472成功 / 52 ignore / 0失敗**、
ui_snapshot **143成功**、viewer context監査テスト **36成功** と実監査がすべてexit 0。
`cargo fmt --check`、glyph lint（危険glyph 0）、通常 / portable core checkもexit 0。
snapshot画像を変更せず、指定サイズの狭幅表示を目視確認した。CRLFと差分を確認し、
独立再レビュー・実機確認・commitの成功とは区別する。

`.\scripts\build-dev.ps1 -PreserveRuntime` もexit 0で完了した。別worktreeのnative build完了を
待って通常featureの確認用runtimeを配置し、稼働プロセスを停止せず製品も起動しなかった。
修正の実機手順は `target/E-1342-hardware-check.md` の追加確認を参照する。

## 17. 退去後の明示trim変更と参照入力の分離（2026-10-10）

独立再レビューのP2を確認し、指摘に同意した。§16で保持したgeometryには古いbboxと回転・
高さ合わせ済み寸法が含まれ、実退去後の「本全体」trim変更に追従しなかった。
単に参照をresetすると選択時のunitを失うため、その案は採用しない。

既存 `ContinuousSpecifiedFitReference::Specified.base` の確定値を、非回転・AI前の正規化原寸
`sources[].size` と現在の解決結果 `resolved` を一組で持つ型へ置き換える。新しい保存先や
独立したpending / boolを設けない。取得済み元寸法だけを退去後も保持し、共有の
`continuous_unit_base_size` に渡して現在のtrim・回転・分割bbox・高さ合わせを再解決する。
解決結果はlayout準備で一度作り、fitと参照unitの配置が共有する。新しいUI同期I/Oはない。
モーダル化でtrim変更を禁止する案は既存の閲覧中編集を損なうため採用しない。

| 参照ページ退去中に変わる入力 | 現在の解決／再選択境界 |
| --- | --- |
| 本全体／ページ個別の表示trim | `ui_view_trim.rs` の既存scope値とreanchorを維持。次のlayoutで現在の `view_trim_single_content_bbox / view_trim_spread_content_bboxes` を保持元寸法へ適用する。scroll再anchorだけで古いbboxを残さない |
| 自動トリムの検出入力 | 画素から既に検出したsource座標のraw bboxも同じsource recordへ保持する。手動設定bbox・左右調停後bbox・回転済bboxを保持入力にしない。現在のAuto／Manual policyで毎layout解決し、画素があるときは既存detectorのload sequence／pixel pointer検証から結果を更新する |
| 保存回転・回転一括reset | `app.rs::set_image_rotation / apply_rotation` とrotation cacheを使用。毎layoutで非回転原寸へ現在の回転を適用し、連結の回転ページはtrimを落とす既存例外を維持する。構成unitが変われば既存の中央unit選択へ戻る |
| 左右分割・見開き構成 | `apply_fullscreen_spread_mode` が既存連結ownerを終端し、`build_presentation_steps_for_nav / continuous_reading_units_and_pos` が新しいhalf / pairを作る。新構成の現在の中央unitからfitを解く |
| 見開き端のslot・白slot設定 | `invalidate_singleton_spread_placement_display` の既存resetとcompositionを使い、実ページ＋仮想slotの新構成で中央unitを選び直す |
| 指定値・原寸制限・fit方式の明示編集 | 既存live setter／fit設定入口とpolicy比較が中央unitを選び直す。古いunit・指定値のgeometryを使わない。§16のlive共通値の保存境界は維持する |
| viewport resize / DPI / UI倍率・gap | 同じ参照unitの元寸法へ現在のimage rect、ppp、実効gap、5倍上限を適用する。resizeだけでは参照unitを選び直さず、手動zoomを保持する |
| index remap / source交換・削除 | 同じsourceを保つ純粋なindex remapは元寸法とpresentation順を保持し、occurrenceを対応付け直す。sourceの削除・交換は既存source失効ownerで参照を終端する |

未確定の新しいunitは既存どおり元寸法到着を待つ。構成／policy編集で新しい中央unitを選ぶ場合に、
古い参照ページの元寸法を別ページへ流用しない。保持元寸法は同じcontext・参照unitにだけ属し、
別contextのcache・worker・設定を変更しない。

回帰は寸法fallbackなしの実prefetch退去→本全体trim変更／参照ページ再入場時の個別trim変更→
同じlayoutを通す。縦／横連結・単ページ／見開きでfit倍率とbboxを確認する。
実退去後の回転・分割・slot・指定値・resize / DPI、および純粋remapも確認し、
§16の通常退去・live共通値保存回帰を保持する。結果は `target/E-1342-fix2-msg.txt` に記録する。
利用者の実機確認中につき、`scripts/build-dev.ps1` は実行せず `target/dev-runtime` へ書き込まない。

横断確認では `cached_margin_bbox` が画素未取得時に結果を破棄するため、Autoを単純に再読込すると
通常退去の倍率保持が退行することも確認した（実検出→実退去のredでは1.6339871→1.0）。
参照source recordの既知検出結果を入力として保持し、共有geometry入口で現在のAuto policy・
左右調停・rotationを適用する。Manual切替では現在の設定を使い、Autoへ戻してもManual bboxを
自動検出結果へ流用しない。新しい画素がある場合は既存detector／cacheの検証を使い、
異なるload sequence／pixel pointerの古い検出を持ち越さない。既存4方式や検出cacheの破棄規則は変えない。
実退去→Auto保持→Manual→Auto→新画素での再検出の回帰を追加した。
この追加確認のため最初の全libを中断し、最終差分で全件を再実行する。中断を製品test失敗とは数えない。

別窓のAutoは既存のwindow identityに対応するruntimeの既知検出結果を優先する。
画素がない場合も、runtimeの受理済みbbox（既知の「trimなし」を含む）を退去用入力で覆わない。
既存のread-only getterへ接続し、runtimeの検出・保存・失効ownerは変えない。
実退去した入力を持つ別contextをmountする回帰で、runtimeのbbox変更とtrimなし、および
main参照／trim状態の不変を確認する。この横断確認後の最終差分で全libを実行し直す。

最終検証: 焦点延べ **205件**（specified 69、continuous_reading 21、view_trim 48、fit_cycle 4、
settings_transfer 20、displayed_image_transform 43。重複あり）、全lib **11,476成功 / 52 ignore /
0失敗**、ui_snapshot **143成功**、viewer context監査テスト **36成功** と実監査がexit 0。
`cargo fmt --check`、glyph lint（危険glyph 0）、通常 / portable core checkもexit 0。
レビュー関連6件には既存の実退去保持・live保存回帰を含む。修正前失敗は
`target/E-1342-fix2-red.log` と `target/E-1342-fix2-auto-red.log` に保存した。
最終全libの前に中断した2回は途中ログを分離し、上記の完走結果とは区別する。
snapshot画像を変更せず、指定サイズの通常幅・狭幅を目視確認した。今回の差分の独立再レビュー・
実機確認・commitは未実施。利用者指示により確認用buildを作らず、`target/dev-runtime` は変更しない。
次のbuild向けの追加実機手順は `target/E-1342-hardware-check.md`、全結果と変更範囲は
`target/E-1342-fix2-msg.txt` に記録する。

## 18. HUD fit popupの既存意匠へ復元（利用者指示、2026-10-10）

ff068456eの実機指摘をコードで確認した。旧masterのcustom-painted行を標準の
selectable_label / checkboxと環境設定用editorへ置換したことが外観差の原因である。
HUDの行描画・割当rectを共通化し、11pt gray150の見出し（現在のkeymap循環キー付き）、
32ptの描画行、13pt gray220のlabel、半径4のblue current / gray hover、区切り線、
「倍率制限」と右端ON/OFFを復元する。「指定サイズ」は同じ5番目のmode行とし、
共通／お気に入り用の小見出しと幅・高さON/OFF行を続ける。

数値・単位のwidgetとsanitize / 単位切替は環境設定と同じaxis controlsへ集約する。
環境設定は従来のcheckbox / draft外観を保ち、HUDだけdark UI / dark_popup_styleを使う。
数値・単位のchild rectをON/OFFの右端から分離し、狭幅では同じcontrolsを下へ折り返す。
全体は既存のviewport内clampとScrollAreaを維持する。helpは11pt gray150の短い説明にする。
新しい保存・focus・IME・循環・popup寿命ownerは作らず、live値を既存handlerへ返す。
同期保存・再描画待ち・viewport再生成で外観を直す案は不要であり採らない。

実HUD回帰は1093×614／狭幅の旧header・blue row、実handlerへのmode／倍率制限、
実数値dragで軸がOFFにならないことと既存の保存境界、wheel scroll→軸ON/OFF→単位変更、
IME Enter / Esc→外側clickを通す。scrollの既存animationはCPUフレームを進めて確認し、
productにdelay / 追加repaintを入れない。修正前header欠落のredは `target/E-1342-ui-red.log`。
既存のlive editor PNGだけを更新し、同じ描画関数の全popup PNGを追加する。対象test限定で
UPDATE_SNAPSHOTSを使い、環境設定と他のPNGを更新しない。結果・PNG一覧は
`target/E-1342-ui-msg.txt` に記録する。利用者指示によりbuild-dev・製品起動・commitは行わない。

最終検証（2026-10-10）: `cargo fmt --check`、glyph lint（危険glyph 0）、指定サイズ71件、
fit循環4件、dark style8件成功。全libは11,478件成功・52件ignore・失敗0（991.23秒、exit 0）、
全ui_snapshotは144件成功。対象PNGは既存live editor 8枚更新／全popup 8枚新規で、全16枚を目視確認した。
通常／portable core check、viewer context audit 36件と実監査もexit 0。CRLFとdiff checkを確認し、
build-dev・`target/dev-runtime`への書込み・製品起動・commitは行っていない。

## 19. popup再レビューの回帰・利用者説明修正（2026-10-10）

P2を確認した。§18の描画fixtureは数値編集にフォーカスせず、Appの全画面キー処理も
通していなかった。狭幅の描画fixtureはscroll・軸・単位・外側clickだけを検証する名前へ
改め、IME保証は実際の数値編集を始める二つのApp回帰へ置換する。

`specified_scale_real_numeric_ime_enter_is_owned_by_popup` / `_escape_is_owned_by_popup`
は1093×614と320×320で実popupのDragValueをクリックし、TextEditのfocusと実bufferを確認する。
既存IME input plugin、`App::update_ime_state`、`render_fullscreen_viewport`を通してpreeditを
数値欄へ届け、変換中のEnter / EscとCommit / Disabled→同frameのキーを検証する。
全画面とpopupの継続、IME keyboard owner、数値の確定／取消を確認する。
DragValueのEnterによる編集終了はwidgetの既存動作として区別し、popup／全画面終了とは扱わない。

検出力はAppのclose経路へキー所有権を迂回する一時mutationを入れて確認した。
Enterは変換中、EscはDisabledと同frameのキーで`fullscreen_idx=None`となり、両テストが
失敗した（exit 101、`target/E-1342-ui2-ime-leak-red.log`）。このmutationを撤去した状態で両回帰のgreen（exit 0）も確認した。
製品のIME・keyboard・viewport predicate・保存ownerは変更せず、待機・repaint・追加stateを作らない。

P3はpopup／環境設定の説明を「自動フィット」「手動ズーム」「AI拡大前の元画像サイズ」へ
統一した。「両軸／内接／UI倍率」も幅と高さ・枠に収まる・アプリの表示倍率と説明する。
意図した文言変更だけを指定サイズeditor・live editor・全popupのexact snapshot testで更新する。
最終検証、変更PNGと新規ファイルの一覧は`target/E-1342-ui2-msg.txt`へ記録する。
利用者の実機確認中のためbuild-dev／`target/dev-runtime`への書込み・製品起動・commitは行わない。

最終検証（2026-10-10）: fmt check・glyph lint（危険glyph 0）、指定サイズ73件、IME helper23件、
全ui_snapshot144件成功。変更PNG34枚を9枚の確認用一覧画像と個別画像で目視確認し、
操作部・見出し領域の画素とPNG寸法が旧版と不変であることも比較した（scroll thumbは比較領域外）。
初回の全libは11,479件成功／既知の負荷依存EOF 1件失敗／52件ignore（exit 101、1056.79秒）。
EOF単独再実行は1件成功、全libの4並列再実行は11,480件成功／失敗0／52件ignore
（exit 0、1010.08秒）。テストの除外・改変はしていない。通常／portable core checkもexit 0。
新規source・test・docs・snapshotファイルなし。変更PNG全名、red→greenと検証の実ログ、
新規確認成果物は`target/E-1342-ui2-msg.txt`へ記録する。製品起動・build-dev・commitは行わず、
利用者が実機確認中の`target/dev-runtime`には書き込まない。
