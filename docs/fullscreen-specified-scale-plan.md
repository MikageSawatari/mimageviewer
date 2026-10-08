# 指定サイズのフルスクリーンfit計画（§1.342）

作成: 2026-10-08 / ラインE。状態: **設計案・利用者判断と独立設計レビュー待ち**。
コード照合基準: `6078f6595`。本ラウンドは文書のみ。製品コード・保存データを変更しない。

## 1. 要求と境界

[backlog §1.342 / §1.343](next-release-backlog.md)の利用者決定を前提とする。
幅と高さは独立・各軸任意、単位は％ / px。既存のPage / Width / Height / Originalに
一方式を追加し、既存の循環と直接選択で切り替える。縦横比を変える指定ではない。
高さだけ800pxなどを指定できる。ウィンドウ・F11・F12・見開き・縦横連結・回転・画像フォルダを対象とする。

以下は推奨仕様であり、利用者判断に属する部分は§10のQ1〜Q10で確認する。
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

例: P=1.5で高さ800pxだけなら高さ枠533.33 point。
1000×2000の単ページは物理400×800pxになる（倍率制限OFF・manual zoom=1）。
幅50％＋高さ800pxなら両方の小さい倍率を採る。

### 4.2 縦横比・倍率制限・手動操作

回転・trim・表示単位合成後の基準寸法を`Sw, Sh`とする。各ページの縦横比を保持する。

- 幅だけ: `s = Bw / Sw`。高さだけ: `s = Bh / Sh`。
- 両方: `s = min(Bw / Sw, Bh / Sh)`。引き伸ばし・cover cropはしない。
- 両方なし: Page。片軸未指定を100％で埋めない。
  高さだけ800pxの横長画像は、幅がviewportを超える場合がある。
- 次に既存`FullscreenFitScaleLimits::apply`を適用（Q3）。
  no_upscale ONなら小画像を原寸以上に拡大しない。既定OFFなら指定サイズまで拡大する。
  no_downscale ONは原寸以上を優先するため指定枠を超え得る。両方ONなら原寸。
- 制約は自動fit倍率に対するもの。見開きの内部高さ合わせや手動zoom / Zは既存どおり。
  個々のページの全倍率を1以下にする機能へ変更しない。
- 自動fitの後に既存manual zoom / pan / Zを掛ける。手動zoomは指定枠を超えられる。

指定はfit計算の基準であり、新しい画面枠やcropではない。
viewportを超える画像は既存clipと閲覧操作を使う。物理格子整列と丸めの画素単位の差は既存契約を維持。
paint / hit / UVを異なる倍率で解かない。

## 5. 表示単位・媒体・窓（Q5〜Q9）

| 場面 | 推奨契約 |
| --- | --- |
| 単ページ・画像フォルダ | 同じfit入口。通常ファイル、ZIP/CBZ・PDF/EPUBページ、RAW、アニメーション画像の既存分類を保つ。フォルダ検出や専用modeを追加しない |
| trim / 分割 | 既存content bbox / 分割後の表示単位を基準にする。取得済みcanonical layout寸法を使い、preview / AI textureの到着で指定サイズが変わらない |
| 見開き | Original以外の既存高さ合わせを維持。2枚と間隔を合成した全体へ枠を適用（Q5）。各ページへ別々に800pxを適用しない。左右・読み順は変更しない |
| 単独ページのslot | compositionが仮想slotを持つ場合は空き側も含める。中央単ページは1枚。SpecifiedScaleはPageと同じcomposition。縦連結Width専用の中央単独表紙特例を自動追加しない |
| 縦 / 横連結 | 各単ページ / 見開きunitへ適用（Q6）。全stripの総幅・総高さを枠へ潰さない。gap・unit長・可視判定・scroll範囲は最終描画寸法へ揃える |
| flow変更 | 既存Page / Width / Heightへの既定fit変更を維持（Q6）。変更後に指定サイズを選べる。flowを理由に新方式を選択不能にしない |
| 90° / 270° | 既存rotation適用後の画面上の幅・高さ（Q7）。高さ800pxは回転後の縦方向。EXIF / 保存回転を再加算しない |
| 自由回転・Z・比較 | 既存free rotation / trim解除 / 専用placement・viewportを維持（Q7）。自由回転AABBが必ず指定枠に入る新しいfitは追加しない。共通式・hit写像を使う |
| 動画 / 音楽 | 画像用のみ（Q8）。既存循環guard、native videoのplacement / shader / HUD、音楽の中央表示方式は変更しない |
| resize / F11 / F12 / DPI | 値と選択方式を保持。％は現在のimage rectへ追従、pxは同じ物理寸法（Q9）。小窓でもpxを窓内へ自動縮小しない。既存clip / pan制約を保つ |
| 複数窓・保存範囲 | 既存fit設定と同じアプリ共通設定（Q9）。各表示先のrect / pppで解く。凍結passive / holdoverはcapture時のpolicyを保持し、新設定を後読みしない |
| prefs OK / import適用 | 指定値だけの変更は既存確定境界で公開し、次のlive描画で再計算（Q9）。選択modeと手動zoom / panは保持。mode自体のprefs変更は選択どおり適用。メニュー等で別方式へ切り替えると既存リセット。同じ方式の再選択はno-op。窓を閉じたり再生成したりしない |

見開き幅枠は間隔を含む。実効間隔`g`、画像合成寸法`Sw, Sh`なら幅候補は`(Bw - g) / Sw`。
指定幅が小さすぎる場合、その表示だけ`g = min(saved_gap, max(0, Bw - 1/P))`とし、
画像へ最低1物理px分を残す（Q5）。保存gapは保持。draw / unit layout / hitへ同じ実効gapを渡す。
既存方式の`max(1 point)`を新方式へ流用して物理px指定を拡大しない。
極小サイズは既存格子丸めの限界があり、全ページが常に1px以上見えるとは保証しない。

### PDF・Remote

幾何だけ変えてraster要求をPageのままにしない。既存`PdfDisplayTarget`の数値と4種のfitへ投影する。

- 両軸: 指定枠の物理width / height＋Page。
- 幅だけ: width指定＋Width。未指定height欄は実表示領域（高さ制約としては使わない）。
- 高さだけ: height指定＋Height。未指定widthは実表示領域。
- 両方なし: 実表示領域＋Page。no_downscale時の既存Original要求、native cap / headroomを保つ。

単ページに限らず、見開き / unitのgap・trim・rotationと同じ解決済み基準を要求側へ渡す。
`fs_pdf_display_target`の比較、初回・partner・zoom要求、context/source identityと既存workerを再利用。
上の軸別投影は単ページの場合。見開き・連結unitでは先に共通resolverでgap / slotと倍率制限を解き、
各ページの必要可視幅・高さへ分配してから既存targetへ渡す。合成幅を左右のPDFへ二重に渡して
別々にfitしない。両軸指定のページtargetは解決済み可視extent＋Pageで表せる。
既存targetの値へ投影できるため、PDF protocol tag・wire layout・IPC版は追加しない。
実装時に正しい要求を表せないと分かった場合は止めて報告する。

Remoteブラウザのfit操作や数値UIは対象外。本体SettingsをRemoteのfit / zoom ownerへ転送しない。
ページgroup / source / mutation契約を保持し、その他のIPC変更も予定しない。
凍結表示にはmodeだけでなく指定値もcaptureする。同じSpecifiedScaleで値だけ変わっても、
旧captureが新しい幅へ変形する経路を禁止する。

## 6. 所有境界・接続点

純粋なfit resolverへmode・指定値snapshot・既存limits・実viewport rect / ppp・
canonical表示寸法・compositionのgapを値で渡す。枠解決・基準倍率を共通化し、式を各consumerで複製しない。
接続対象は単ページgeometry / transform、`fs_image_draw_geometry_for_size`、通常 / Z / navigatorの
見開きfit式、similar preview、連結unit layout、PDF target。
paintから倍率を逆算せず、確定したscale / rect / hit / UVを使う。
captureされたgeometry / 表示単位には同じfit descriptorの指定値を保持し、factory / replayも揃える。
既存geometry / layout cache keyがfit条件を含む場合は指定値も含める。
常にPageを使う縮図・overviewへ指定値を誤適用しない。

共通helper接続時も既存4方式のgap・zoom順・Original・丸めを変えない。
手動入力・navigation・loader / worker / generationのownerは追加・変更しない。
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

**構造的変更である理由**: サイズを決める幾何ownerへfit policyを追加し、単ページ・見開き・連結・
draw / hit / capture / PDF要求を同じ値へ接続する。detached症状guard、時間待ち、rect一致捕捉、
窓の誤同定やviewport再生成で直す案ではない。host identity / placement reducer / generation /
focus / captureのowner、既存reserve計算を変更しない。

viewport / detached経路へ触れる実装前に、**ClaudeCodeと独立reviewerがこの構造判定へ合意する**。
本案がレビュー済みだとは記録しない。合意後、実装時にdetached §11へ実際の変更範囲だけを記録する。
記録案は§1.342、shared fit resolver / transform、ui_fullscreenの各consumer・capture・PDF要求、
必要なsnapshot型境界、設定 / prefs / transfer、回帰。理由は上の所有境界共通化、
非変更境界はhost / placement / viewport生成 / focus。別owner変更が必要なら先に再設計・合意する。

## 9. 実装時の検証・文書更新

1. **純粋計算**: 片軸 / 両軸 / 両None、％ / px / mixed、縦横比、各範囲端、大小画像、
   no_up / no_downの4通り、manual zoom、P=1 / 1.25 / 1.5 / 2とUI倍率。既存4方式の幾何不変。
2. **表示単位**: 高さ違い見開き、左右読み、指定幅とgap境界、virtual slot / 中央単独表紙、
   trim / 分割・90° / 270°・RAW preview→full / AI差替え。paint / hit / UV / navigator / Zとscaleの一致。
3. **連結**: 縦 / 横で各unitへ適用。pixel丸め・gap・描画長・scroll範囲、resize / fit / flow切替。
   既存anchor / zoom / scroll意味とsource generationの不変。
4. **handler**: key / ring / gesture / mouse3ボタン / padが同じownerを通る。
   除外なし / 明示指定方式除外 / 1候補 / 現在対象外 / 全5除外 / 未知ID保持 / 従来4だけ除外。
   直接pickerには5方式。動画 / 音楽 / 編集では循環しない。
5. **保存・prefs・転送**: serde欠落default、mixed / None / 範囲のround-trip、無効単位・数値拒否、
   全field分類、旧転送文書でdestination維持、OK / Cancel / import、IME Enter / Esc、狭幅widget到達。
6. **context / lifecycle**: main / F12 / F11 / 異DPI、実効HUD / 右予約と抑制ON/OFF。
   capture後の値変更が旧holdover / frozen siblingを変形させない。正しいpppと既存ownerを使い、
   resize / 設定変更でsource reload / worker cancel / viewport再生成を起こさない。
7. **PDF**: 初回 / partner / zoomの必要px、両軸 / 片軸 / mixed / gap / rotation、
   no_downのOriginal、native cap / headroom、context適用、既存wire round-trip不変。

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
3. **Q3 拡大・縮小の制限** — 現在の「拡大しない」「縮小しない」を指定方式にも適用しますか？
   **推奨: 適用する。** 小画像を拡大しない場合は前者をONにします。後者ONは指定上限を超え得て、両方ONなら原寸を優先します。
4. **Q4 初期値・指定なし** — 初期値は幅100％・高さ100％、両方OFFならページ全体と同じ表示でよいですか？
   **推奨: その動作。** 未指定だから自動で循環から外さず、外したい場合は既存循環設定を使います。
5. **Q5 見開き** — 指定枠は2枚と間隔を合わせた全体へ適用してよいですか？
   **推奨: 全体へ適用し、今の高さ合わせを維持。** 空き側のある単独ページはその枠も含めます。
   指定幅より間隔が大きい場合は、その表示だけ間隔を縮め、保存値は変えません。
6. **Q6 連結読み** — 各単ページ・見開きに適用し、連結方式変更時の既定fitへの切替は維持してよいですか？
   **推奨: 維持する。** 連結全体を一枠へ縮めず、方式変更後は指定サイズを選び直せます。
7. **Q7 回転・トリム** — 回転後・トリム後に見える画像部分を幅・高さの基準にしてよいですか？
   **推奨: その基準。** 90°回転後の高さは画面の縦方向。自由回転の特別な自動合わせは追加しません。
8. **Q8 対象媒体** — 画像と本のページ用とし、動画・音楽は現在の表示方式のままでよいですか？
   **推奨: 画像・本のページのみ。** 通常画像フォルダ、ZIP/PDF等、RAW、アニメーション画像も既存経路で使います。
9. **Q9 保存と変更時** — アプリ共通で保存し、指定値だけの変更や窓サイズ変更では今の方式と手動ズームを保って再計算してよいですか？
   **推奨: その動作。** ％は各窓の表示領域、pxは小窓でも勝手に縮めません。
   凍結窓は現在の画像を保ち、通常描画へ戻ったときに新設定を使います。
10. **Q10 入力欄** — ％は0.1〜1000.0、pxは1〜32768、単位変更は数値維持、OFFから再ONは100％でよいですか？
    **推奨: その仕様。** px化は整数へ丸め、単位の範囲へ収めます。OFF中の以前の値を別保存せず、幅・高さの2行で編集します。
