# 指定サイズのフルスクリーンfit計画（§1.342）

作成: 2026-10-08 / 改訂: 2026-10-09（r2）/ ラインE。
状態: **独立設計レビューREVISEの3件を反映した案・再レビューと利用者判断待ち**。
コード照合基準: 初版 `6078f6595`、本改訂 `2d327da31`。本ラウンドは文書のみ。製品コード・保存データを変更しない。

## 1. 要求と境界

[backlog §1.342 / §1.343](next-release-backlog.md)の利用者決定を前提とする。
幅と高さは独立・各軸任意、単位は％ / px。既存のPage / Width / Height / Originalに
一方式を追加し、既存の循環と直接選択で切り替える。縦横比を変える指定ではない。
高さだけ800pxなどを指定できる。ウィンドウ・F11・F12・見開き・縦横連結・回転・画像フォルダを対象とする。

以下は推奨仕様であり、利用者判断に属する部分は§10のQ1〜Q11で確認する。
§1.343の確定済みの除外ID規則、割り当ての既定なし、単一循環ownerは再判断しない。
今回の範囲は画像表示のfit。動画・音楽のサイズ方式、加工・書き出し解像度、
モニター選択、窓の配置やサイズ自体、フォルダ別・画像別プリセットは追加しない。

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
| 単ページZ / 見開きZ | [displayed_image_transform.rs:1127](../src/displayed_image_transform.rs#L1127)はPage・制限なし・自由回転なしを強制。:1815のfull_image_zoomはPage基準。一方[ui_fullscreen.rs:40978](../src/ui_fullscreen.rs#L40978)と:13169の見開きZは選択fit / limitsから得たfit_scale・gapを照準の基準にする。[display-pipeline.md:1662](display-pipeline.md#L1662)にこの違いと見開きPDFのZ再レンダなしという現行制限がある |
| 寸法の単位と原寸制限 | [display-pipeline.md:34](display-pipeline.md#L34)はsource_dims＝画素、PDF layout_dims＝1/1000 point。[ui_fullscreen.rs:8746](../src/ui_fullscreen.rs#L8746)はcanonical aspectをpixel sourceの長辺へ正規化。[同:36428](../src/ui_fullscreen.rs#L36428)と[display-pipeline.md:1705](display-pipeline.md#L1705)の単ページ / 見開きは現在の描画textureを原寸基準にできるため、AI到着で制限の結果も変わる。連結は:37481と:8944の既存raw/source優先規則が異なる |
| 回転＋trimの経路差 | [ui_fullscreen.rs:24028](../src/ui_fullscreen.rs#L24028)の通常単ページは保存回転でもtrimを渡すが、見開き:40758、連結:37562、capture生成:10465、PDF要求:37103は回転ページのtrimを落とす。:17114はこの差をcallerの責務として明記。fit式だけの共通化では同じ入力にならない |
| 保存・転送 | [settings.rs:4995](../src/settings.rs#L4995)に既存fit設定、[settings_db.rs:16](../src/settings_db.rs#L16)にsettings_kv。[settings_transfer.rs:174](../src/settings_transfer.rs#L174)は全フィールド分類、:334はfit関連のplain転送、:224は受け入れたキーだけ適用 |
| prefs確定 | [preferences.rs:2155](../src/ui_dialogs/preferences.rs#L2155)の`install_preferences_settings`と:2709のOK境界。UIで管理する新設定を「環境設定外の最新値移送」に混ぜない |

実装前に再照合し、別の前提が必要と判明した場合は止めて設計担当へ返す。

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

pxは**物理ピクセル**（Q2）。描画先Contextのeffective pppでpointへ変換する。
OS DPIとUI倍率の両方を含む。main Contextのpppを別窓へ使い回さない。
表示領域を`Vw, Vh`（point）、pppを`P`、解決済み各軸枠を`Bw, Bh`とすると:

- ％: `Bw = Vw × width_percent / 100`、高さも同様。
- px: `Bw = width_px / P`、高さも同様。
- 未指定軸: 制約なし。両方未指定だけはPageと同じfitへ解決する（Q4）。
- 初期値は幅100％・高さ100％。現在のfit modeを変えない。
  未編集でも新方式は通常の領域ではPageと同じ幾何として循環へ参加し、動的に候補を減らさない。
  極小領域のgap補正は§5による。両Noneはその補正も行わず既存Page基準を使う。
  ここでPageと同じなのはfit式。selected modeはSpecifiedScaleなので、回転＋trimの
  入力準備は両None / 初期100％でも§5.1の新方式規則を使う。旧Pageのtrim例外へ戻さない。

例: P=1.5で高さ800pxだけなら高さ枠533.33 point。
1000×2000の単ページは物理400×800pxになる（倍率制限OFF・manual zoom=1）。
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

### 4.3 resolver入力: アスペクトと物理原寸（Q3改訂）

canonical寸法を「原寸の画素数」として渡さない。共通resolverへ渡す各ページの値を区別する。

| 入力 | 単位・役割 |
| --- | --- |
| アスペクト基準 `A` | ページの縦横比と構成を決める寸法。取得済みcanonical layoutを優先。PDFはpage boxの1/1000 pointであり、長さを物理pxやpppへ直接変換しない |
| 物理原寸基準 `O` | そのconsumerが現在採用するpixel基準のfull-page寸法。自動fitの拡大 / 縮小制限とOriginalに用いる。アスペクトの単位とは別に、必ず画素数で渡す |
| content bbox・rotation | §5.1で解決したsource座標の正規化bboxと保存回転。分割・trimの優先を解決してから渡し、回転とbbox変換を一度だけ行う |
| 表示先・構成 | 実image rect（point）、そのContextのeffective ppp、ページのlogical scale・slot・実効gap。frozenはcapture時の入力を使う |

`O`の選択も現在のconsumerの責務を維持する。通常画像の単ページ / 見開きは現在の
描画texture・final compositeを基準にする経路がある。RAW / PDF / pass-throughでは
既存で利用可能なpixel coordinate source / rasterを使い、canonical page boxを画素とみなさない。
見開きの:40879はcanonical layoutをそのまま基準寸法へ採る枝もあるため、新方式へ
その単位を物理原寸として持ち込まない。新方式adapterは別途Oをpixel寸法から取得して正規化し、
既存4方式のその枝は変更しない。
連結は同じアスペクトのprocessedが来てもraw/sourceを優先し、処理でアスペクトが変わった
場合はprocessedを採る既存規則を維持する。すべてをraw基準またはAI texture基準へ統一しない。
根拠は上の§2、[raw_page_store.rs:2291](../src/app/raw_page_store.rs#L2291)、
[ui_fullscreen.rs:27000](../src/ui_fullscreen.rs#L27000)・:40879。

単ページの正規化は既存:8746と同じく `N = A × max(Ow, Oh) / max(Aw, Ah)`。
これでアスペクトはA、長辺のpixel基準はOになる。Nへrotation / bboxを適用し、見開きは
各ページの既存高さ合わせを合成した後に§4.2を解く。pixel基準上のscaleへ
`FullscreenFitScaleLimits::apply`（物理原寸は`1/P`）を適用する。
canonicalの1/1000 pointをそのまま`1/P`と比較したり、PDFの72dpiを新しい原寸規則にしたりしない。
一時textureのアスペクトがAと違えば、既存のcontain処理で歪ませず描く。

**到着時の不変保証を限定する。** アスペクト・bbox・構成・rect / ppp・指定値が変わらず、
原寸制限が結果を変えない場合、同じアスペクトのpreview / AI差替えだけで指定fitの
layout枠は変わらない。paintのcontain差・既存pixel丸めまで寸法一致とは保証しない。
no_upscale / no_downscaleが効く場合、Oが変わればlayout寸法が変わり得る。
例: trim後の原寸400px・高さ800px・拡大禁止では400px表示。4倍AI到着でその経路の
原寸基準が1600pxになると800px表示になる。これは現在の原寸制限を維持する結果（Q3）。
連結のraw/source安定規則やfrozen captureの入力はそれぞれ維持し、差替えを抑止する追加stateは作らない。

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

| 場面 | 推奨契約 |
| --- | --- |
| 単ページ・画像フォルダ | 同じfit入口。通常ファイル、ZIP/CBZ・PDF/EPUBページ、RAW、アニメーション画像の既存分類を保つ。フォルダ検出や専用modeを追加しない |
| trim / 分割 | §5.1の新方式用入力解決を使い、分割がtrimに優先する。canonicalはアスペクト基準、原寸は§4.3のpixel基準。preview / AI到着時の寸法不変は同節の条件付き保証 |
| 見開き | Original以外の既存高さ合わせを維持。2枚と間隔を合成した全体へ枠を適用（Q5）。各ページへ別々に800pxを適用しない。左右・読み順は変更しない |
| 単独ページのslot | compositionが仮想slotを持つ場合は空き側も含める。中央単ページは1枚。SpecifiedScaleはPageと同じcomposition。縦連結Width専用の中央単独表紙特例を自動追加しない |
| 縦 / 横連結 | 各単ページ / 見開きunitへ適用（Q6）。全stripの総幅・総高さを枠へ潰さない。gap・unit長・可視判定・scroll範囲は最終描画寸法へ揃える |
| flow変更 | 既存Page / Width / Heightへの既定fit変更を維持（Q6）。変更後に指定サイズを選べる。flowを理由に新方式を選択不能にしない |
| 90° / 270° | 新方式では§5.1のrotation / trim入力を揃えた後の画面上の幅・高さ（Q7）。高さ800pxは回転後の縦方向。EXIF / 保存回転を再加算しない。既存4方式の経路差は変更しない |
| 自由回転・Z・比較 | free rotation時の既存effective_bbox / trim解除・専用placement・viewportを維持。自由回転AABBへの新しいfitは追加しない。Zは§4.4の単ページ / 見開き別契約（Q11） |
| 動画 / 音楽 | 画像用のみ（Q8）。既存循環guard、native videoのplacement / shader / HUD、音楽の中央表示方式は変更しない |
| resize / F11 / F12 / DPI | 値と選択方式を保持。％は現在のimage rectへ追従、pxは同じ物理寸法（Q9）。小窓でもpxを窓内へ自動縮小しない。既存clip / pan制約を保つ |
| 複数窓・保存範囲 | 既存fit設定と同じアプリ共通設定（Q9）。各表示先のrect / pppで解く。凍結passive / holdoverはcapture時のpolicyを保持し、新設定を後読みしない |
| prefs OK / import適用 | 指定値だけの変更は既存確定境界で公開し、次のlive描画で再計算（Q9）。選択modeと手動zoom / panは保持。mode自体のprefs変更は選択どおり適用。メニュー等で別方式へ切り替えると既存リセット。同じ方式の再選択はno-op。窓を閉じたり再生成したりしない |

見開き幅枠は間隔を含む。実効間隔`g`、画像合成寸法`Sw, Sh`なら幅候補は`(Bw - g) / Sw`。
指定幅が小さすぎる場合、その表示だけ`g = min(saved_gap, max(0, Bw - 1/P))`とし、
画像へ最低1物理px分を残す（Q5）。保存gapは保持。draw / unit layout / hitへ同じ実効gapを渡す。
既存方式の`max(1 point)`を新方式へ流用して物理px指定を拡大しない。
極小サイズは既存格子丸めの限界があり、全ページが常に1px以上見えるとは保証しない。

### 5.1 回転＋trim: 現行例外と新方式の範囲（Q7改訂）

§2のsource inspectionで次の差を確認した。旧「すべて回転後・trim後で同じ」という説明を撤回する。

| 現行経路 | 保存回転のあるページ |
| --- | --- |
| 通常単ページのlive描画 | trimを渡し、transformでbboxを回転する（:24028） |
| 見開きlive描画 | 回転ページのtrimをNoneにする。未回転の相方はtrimを維持（:40758） |
| 縦 / 横連結unit | 回転ページのtrimをNoneにする。ただし分割はrotation込みのsliceを優先（:37562） |
| display-unit capture生成 | 単ページを含め回転ページのtrimを落とす（:10465） |
| 通常PDF要求のbbox取得 | 回転ページはNone（:37103）。live単ページ描画と同じbboxではない |

**推奨: SpecifiedScaleだけrotation＋trimの入力を揃える。既存Page / Width / Height /
Original（およびMarginFit互換経路）の例外は今版では変えない。** 新方式でも旧例外を維持する
案を検討したが、通常単ページからcaptureへ移ると切れる範囲が変わり、PDFの要求基準も違う。
本案のdraw / hit / capture / PDF同一入力契約を満たせないため推奨しない（Q7）。
既存4方式をまとめて統一する改修は別件として利用者・設計担当の範囲判断が必要。

実装にはfit式の追加以外に、**新方式の表示単位を準備する共通入力解決**が必要。
selected fit descriptorから新方式かどうかを決め、各page occurrenceのsource座標bboxを
既存single / spread trim helperで取得する。自動trimの見開き調和も同じ準備で一度行う。
保存回転だけを理由にbboxを落とさず、draw / unit layout / navigatorへ同じ値を渡す。
captureも同じ入力を保存し、PDF demandは同じ最終geometryを使う。新方式のconsumerに
独立したrotation.is_none()分岐や別のtrim再取得を残さない。
`fs_page_content_bbox`の「分割優先」、`effective_bbox`の自由回転時trim解除は維持する。
新方式のpolicy解決は単ページZが有効fitをPageへ上書きする**前**に行い、Z中も同じbboxを使う。
これは指定サイズfitの入力準備であり、新しいtrim設定・保存回転・表示状態は追加しない。
既存`ui_view_trim.rs:855 / 863 / 877`のownerと正規化座標を再利用し、trim検出の同期I/Oを足さない。

### 5.2 PDF要求・Remote（Z要求はQ11）

幾何だけ変えてraster要求をPageのままにしない。通常指定fitの枠解決は、既存
`PdfDisplayTarget`の数値と4種のfitへ次のように投影できる。

- 両軸: 指定枠の物理width / height＋Page。
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

## 6. 所有境界・接続点

純粋なfit resolverへmode・指定値snapshot・既存limits・実viewport rect / ppp・
§4.3のアスペクト基準とpixel原寸基準・§5.1で解決済みのbbox・compositionのgapを値で渡す。
枠解決・基準倍率を共通化し、式を各consumerで複製しない。
接続対象は単ページgeometry / transform、`fs_image_draw_geometry_for_size`、通常 / Z / navigatorの
見開きfit式、similar preview、連結unit layout、PDF target。
paintから倍率を逆算せず、確定したscale / rect / hit / UVを使う。
captureされたgeometry / 表示単位には同じfit descriptorの指定値を保持し、factory / replayも揃える。
既存geometry / layout cache keyがfit条件を含む場合は指定値も含める。
常にPageを使う縮図・overviewへ指定値を誤適用しない。

共通helper接続時も既存4方式のgap・zoom順・Original・丸めを変えない。
新方式のrotation＋trim入力準備と絶対PDF demandは通常 / Z / captureで共有する。
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
    PhysicalPixels(u32),
}
```

Noneは指定なし。％は0.1％単位の整数でNaN / infinity / 等価な複数表現をなくす。
初期値は両軸PercentTenths(1000)。新フィールドにserde defaultを付ける。
**新規・未リリース**の機能・キー・型なので専用migration・schema version・DDLは不要。
旧指定倍率設定の読み替えはしない。汎用settings_kvを使い、既存fit modeや実データを保持する。
旧版の新variant読込は既存互換性保護に従い、旧版用silent fallbackを増設しない。

転送は既存fit関連と同じ**plain export / import**。各軸の単位・範囲・Noneを検証し、混在と両Noneは正当。
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
表示領域100％、物理px、縮小しない優先を短い説明で示す。

Q10の推奨範囲は**0.1〜1000.0％ / 1〜32768px**。数値widgetとvalidatorで範囲を共有。
0・負値・超過・小数pxを保存値にしない。ローカル読込範囲補修はsanitize、転送は範囲外拒否。
単位切替は画面から逆算せず数値を維持（px化は丸め、新範囲へclamp）。
OFFはNone、再ONは100％。以前の単位・数値の別保存は作らない（Q10）。OK前は既存draft内に留める。
DragValue等の既存widgetを使い、独自TextEdit parserや確定用キーを追加しない。
TextEditが必要ならime_focus helperを使用。Enter / Escは既存dialog helperとwidget focusの優先を保つ。

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
- 原寸をrawで固定して到着時の全寸法を不変にする案は、現在のAI後の原寸制限を変えるため採らない（Q3）。
  アスペクトと原寸を区別し、到着時の不変保証を条件付きにする。
- rotation＋trimの旧例外を新方式にも複製する案は、capture / PDFとの食い違いを残すため採らない（Q7）。
  新方式だけ表示単位の入力を一度準備し、既存4方式の挙動は今回の統一対象にしない。
- Z専用の保存target・別pendingを増やす案は採らない。§5.2の絶対px需要と既存request gateの
  型付き置換で、通常fitのtargetを保存・上書き・復元する組合せを除去する。

**構造的変更である理由**: サイズを決める幾何ownerへfit policyを追加し、単ページ・見開き・連結・
draw / hit / capture / PDF要求を同じ値へ接続する。detached症状guard、時間待ち、rect一致捕捉、
窓の誤同定やviewport再生成で直す案ではない。host identity / placement reducer / generation /
focus / captureのowner、既存reserve計算を変更しない。

viewport / detached経路へ触れる実装前に、**ClaudeCodeと独立reviewerがこの構造判定へ合意する**。
本案がレビュー済みだとは記録しない。合意後、実装時にdetached §11へ実際の変更範囲だけを記録する。
記録案は§1.342、shared fit resolver / transform、ui_fullscreenの各consumer・capture・PDF要求、
新方式だけのrotation＋trim入力準備、app.rsの絶対PDF要求adapterと既存比較cacheの型置換、
viewer_context_registryの同じcontext-local fieldの型・移送境界、必要なsnapshot型境界、
設定 / prefs / transfer、回帰。既存4方式はlegacyの表示・trim / Z / request判定を維持する。
理由は上の所有境界共通化、
非変更境界はhost / placement / viewport生成 / focus。別owner変更が必要なら先に再設計・合意する。

## 9. 実装時の検証・文書更新

1. **純粋計算**: 片軸 / 両軸 / 両None、％ / px / mixed、縦横比、各範囲端、大小画像、
   no_up / no_downの4通り、manual zoom、P=1 / 1.25 / 1.5 / 2とUI倍率。既存4方式の幾何不変。
2. **表示単位**: 高さ違い見開き、左右読み、指定幅とgap境界、virtual slot / 中央単独表紙、
   trim / 分割・90° / 270°・RAW preview→full / AI差替え。paint / hit / UV / navigator / Zとscaleの一致。
   アスペクト基準Aとpixel原寸Oを別入力で検証。PDF page box（1/1000 point）と同アスペクトの
   pixel寸法を別々に変え、原寸判定へpage boxの大きさが漏れないことを固定する。
   no_upなしの同アスペクトAI差替えは同じlayout枠、no_upありのtrim後400px→4倍AI・高さ800pxは
   400px→800pxになることを確認する。no_down / 両ON、連結raw/source優先、texture aspect差のcontainも別途確認。
3. **連結**: 縦 / 横で各unitへ適用。pixel丸め・gap・描画長・scroll範囲、resize / fit / flow切替。
   既存anchor / zoom / scroll意味とsource generationの不変。
4. **handler**: key / ring / gesture / mouse3ボタン / padが同じownerを通る。
   除外なし / 明示指定方式除外 / 1候補 / 現在対象外 / 全5除外 / 未知ID保持 / 従来4だけ除外。
   直接pickerには5方式。動画 / 音楽 / 編集では循環しない。
5. **保存・prefs・転送**: serde欠落default、mixed / None / 範囲のround-trip、無効単位・数値拒否、
   全field分類、旧転送文書でdestination維持、OK / Cancel / import、IME Enter / Esc、狭幅widget到達。
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
   live単ページ / 見開き / unit、capture生成→replay、PDF需要が同じ解決済みbboxと画面軸を使い、
   回転・trimを二重適用しないこと、未回転の相方も正しいことを検証する。
   legacy4方式は§5.1の経路別例外を明示的に固定し、今回の共通helper接続で無意識に統一しない。

snapshotは純粋な設定描画helperをtests/ui_snapshotへ渡す（App全体のsnapshotにしない）。
循環5checkbox、指定2行、mixed / None、ライト / ダーク、通常 / 狭幅、説明折返し、限界値を撮る。
必要なら単ページ・見開き幾何fixtureも追加。PNGを目視し、widget欠け・scrollbar重なり・説明の読みやすさを確認。
入力routingの証明にはhandler testを使う。

実装時gate: cargo fmt、焦点回帰、`cargo test -p mimageviewer --lib`（pipeなし・実exit）、
通常 / portable core check、ui_snapshot、`python scripts/check_ui_glyphs.py`。
新機能期待をテストで先に固定し、既存bug修正が必要なら有効redを別途確認する。
実機用buildは実装依頼とbuild/test方針に従い、製品はエージェントが起動しない。
**本ラウンドは文書のみで、製品テスト・buildは実行しない。**

実装時co-update: display-pipeline、keymap-specの5方式順、必要なkey-customization / ring文書、
backlog、prefs検索、manual fullscreen / settings、detached §11。実装・検証・実機結果を本計画へ集約。
動画 / Remoteの仕様変更が必要なら、今回の範囲へ黙って追加しない。

## 10. 利用者への質問（未決・推奨案）

幅・高さを独立かつ任意に指定、両軸なら内接・片軸ならその軸で倍率を決めること、
既存方式との切替と％/px両対応は依頼済み要件。残る判断を確認後、日付付き決定へ置き換える。

1. **Q1 ％の基準** — 100％はモニター全体ではなく、今の画像表示領域でよいですか？
   **推奨: 画像表示領域。** 固定バー・右パネルの予約を除き、小窓・F11・F12で同じ意味にします。
2. **Q2 pxの意味** — pxはWindowsの拡大率に関係なく、実際の画面のピクセル数でよいですか？
   **推奨: 物理ピクセル。** 高さ800pxは別DPIの窓でも800px。UIの論理サイズにはしません。
3. **Q3 拡大・縮小の制限（変更）** — AIの高解像度画像が届くと表示サイズが変わる場合も含め、今の「拡大しない」「縮小しない」を指定方式にも適用しますか？
   **推奨: 今の制限を維持する。** 単ページ・見開きでは、描画する画像が高解像度になると原寸の基準も大きくなる経路があります。
   例えばトリム後400pxの画像を高さ800px・拡大禁止で見ると、最初は400px、4倍AI到着後は800pxになり得ます。
   「縮小しない」は指定上限を超え得て、両方ONなら原寸を優先します。連結の今の原寸基準も維持し、
   「AI到着でも必ず同じ寸法」は保証しません。
4. **Q4 初期値・指定なし（説明補足）** — 初期値は幅100％・高さ100％、両方OFFならページ全体と同じ大きさ合わせでよいですか？
   **推奨: その動作。** 回転＋トリムの扱いは、両方OFFでもQ7の新方式のルールを使います。
   未指定だから自動で循環から外さず、外したい場合は既存循環設定を使います。
5. **Q5 見開き** — 指定枠は2枚と間隔を合わせた全体へ適用してよいですか？
   **推奨: 全体へ適用し、今の高さ合わせを維持。** 空き側のある単独ページはその枠も含めます。
   指定幅より間隔が大きい場合は、その表示だけ間隔を縮め、保存値は変えません。
6. **Q6 連結読み** — 各単ページ・見開きに適用し、連結方式変更時の既定fitへの切替は維持してよいですか？
   **推奨: 維持する。** 連結全体を一枠へ縮めず、方式変更後は指定サイズを選び直せます。
7. **Q7 回転・トリム（変更）** — 新しい指定方式では、回転後にトリムした画像部分を、見開き・連結・凍結表示・PDF要求でも共通の基準にしてよいですか？
   **推奨: 新方式だけ揃え、既存4方式は今版では変えない。** 現状は通常単ページで回転＋トリムが使えても、
   見開き・連結・凍結表示の生成・PDF要求では回転ページのトリムを外す経路があります。
   新方式を揃えるにはfit計算だけでなく、トリム入力の準備も共通化します。高さは回転後の画面の縦方向です。
   分割優先と自由回転時のトリム解除は維持し、自由回転の特別な自動合わせは追加しません。
8. **Q8 対象媒体** — 画像と本のページ用とし、動画・音楽は現在の表示方式のままでよいですか？
   **推奨: 画像・本のページのみ。** 通常画像フォルダ、ZIP/PDF等、RAW、アニメーション画像も既存経路で使います。
9. **Q9 保存と変更時** — アプリ共通で保存し、指定値だけの変更や窓サイズ変更では今の方式と手動ズームを保って再計算してよいですか？
   **推奨: その動作。** ％は各窓の表示領域、pxは小窓でも勝手に縮めません。
   凍結窓は現在の画像を保ち、通常描画へ戻ったときに新設定を使います。
10. **Q10 入力欄** — ％は0.1〜1000.0、pxは1〜32768、単位変更は数値維持、OFFから再ONは100％でよいですか？
    **推奨: その仕様。** px化は整数へ丸め、単位の範囲へ収めます。OFF中の以前の値を別保存せず、幅・高さの2行で編集します。
11. **Q11 ZズームとPDF（新規）** — Zの照準は、単ページでは指定サイズに関係なく表示領域を基準にし、見開きでは選んだ表示方式を基準にする今の違いを維持してよいですか？
    **推奨: その違いを維持する。** 解除後は指定サイズへ戻します。新方式のPDFは照準・確定・窓サイズ変更時の
    実表示に合わせて解像度を要求し、見開きなら両ページを対象にします。既存4方式のZとPDFの制限は今版では変えません。

## 11. 独立設計レビューへの対応（r2、2026-10-09）

3件ともHEAD `2d327da31`のコードで確認し、指摘に同意する。反論・製品コード変更はない。

| 指摘 | 確認・改訂 |
| --- | --- |
| P2: Z倍率とPDF要求の基準不一致 | :1127 / :1815 / :12539 / :69762 / :69867を確認。§4.4で単ページと見開きの差を明記し、§5.2を実geometry由来の絶対需要とcontext-local gate置換へ改訂。§9に進入・照準・確定・解除・resizeと両ページの回帰、Q11を追加 |
| P2: canonical寸法と原寸制限の両立 | display-pipeline:34 / :1705、ui_fullscreen:8746 / :36428を確認。§4.3でアスペクトAとpixel原寸Oを区別し、PDFの単位変換境界と制限時の寸法変化を明記。不変保証を限定、Q3を変更 |
| P2: 回転＋trimの現行例外 | :24028 / :40758 / :37562 / :10465 / :37103 / :17114を確認。§5.1へ例外表を追加。新方式だけ共通入力で揃え、既存4方式の変更は別判断とする推奨案。§9の経路別回帰とQ7を変更 |

Q1 / Q2 / Q5 / Q6 / Q8〜Q10は推奨を維持。Q3 / Q7は変更、Q4は同じfit式とQ7の入力ルールを区別する説明補足、
Q11は新規で、すべて利用者判断待ち。
再レビューが必要。文書のdiff / EOLを確認するだけで、cargo / snapshot / buildや製品起動は行わない。
