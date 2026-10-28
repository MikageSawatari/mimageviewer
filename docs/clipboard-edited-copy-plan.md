# 編集を反映した画像をクリップボードへコピー (§1.261)

**状態**: 設計 (2026-10-28)。実装は `clip-edited-1261` ブランチ。
**出典**: 利用者要望 2026-09-21 / 焼き込み段の設定 2026-09-23 ([next-release-backlog.md](next-release-backlog.md) §1.261)。
**関連**: [bake-stage-unification-plan.md](bake-stage-unification-plan.md) (段 `BakeStage` と共通 worker 合成)、
[ui-responsiveness.md](ui-responsiveness.md) §4、[virtual-folders.md](virtual-folders.md)、
[context-menu-unification-plan.md](context-menu-unification-plan.md)。

## 1. 何を作るか

右クリックメニューに **「編集を反映した画像をコピー」** を足す。補正・回転・切り取り・消しゴム・
補正レイヤー・隠蔽・テキスト注釈 (と、設定した段に応じて AI 処理・表示用補正) を焼き込んだ画像を
CF_DIB でクリップボードへ置く。

- 元ファイル・保存済みの編集設定は変えない (読むだけ)。
- 画面キャプチャではない。HUD・パネル・選択枠は入らない。
- 合成は単枚 Ctrl+E と**同じ関数** (`books::decode_composite_source_for_materialization` →
  `books::compose_export_entry` → 見開きは `capture::combine_spread_color_images`) を通す。
  コピー専用の合成経路は作らない。
- どの段まで焼くかは、環境設定「書き出しの焼き込み」の表に行を 1 つ足して選ぶ
  (`Settings::bake_stage_clipboard`)。

## 2. 既存の「画像をクリップボードにコピー」との関係

**既存の項目は変えずに残し、新しい項目を別に足す。**

- 既存は「元画像をデコードして EXIF 向きと回転だけを当てる」出荷済みの挙動。置き換えると、
  編集の無い画像でも AI や表示用補正の再計算で遅くなる・結果が変わる、という退行になり得る。
- 両方が並ぶのが煩わしい利用者は、右クリックメニューのカスタマイズでどちらかを隠せる。
- 新項目は既存項目の直後に置く (同じコピー系のグループ)。

## 3. 対象の決め方

| 場所 | 対象 | 理由 |
| --- | --- | --- |
| 一覧 (グリッド) | 右クリックした 1 項目 (`target.item_index`) | 既存コピーと同じ。チェック集合は使わない (クリップボードに置けるのは 1 枚) |
| フルスクリーン | **Ctrl+E と同じ対象**。見開き表示中なら左右を結合した 1 枚 (`resolve_visible_spread_output_pair`) | 「Ctrl+E と結果を揃える」。表示中の見開きを 1 枚で貼れるほうが用途に合う |
| 別ウィンドウ (F12) | フルスクリーンと同じ (その窓の bundle がマウントされた状態でメニューが dispatch される) | detached 固有のコードは足さない。§6 参照 |

対象にできる項目: `Image` / `ZipImage` / `PdfPage` (ZIP・PDF のページを含む)。
`Stack`・フォルダ・書庫本体・動画・音声は対象外 (メニューに出さない)。

- PDF ページは Ctrl+E と同じ長辺 4096 で描画し、パスワードは `pdf_current_password` を使う。
- アニメーション画像は**最初のフレーム**をコピーする (既存コピー・一括書き出しと同じ)。
  Ctrl+E は「黙って 1 フレーム目を保存する」のを避けて拒否しているが、コピーは既存項目が
  1 フレーム目を置く挙動で出荷済みなので、そちらに揃える。
- 一覧から未表示のページを選んでもよい。編集 snapshot は一括書き出しと同じく DB から引く
  (`book_baked_edit_snapshot(&key, Some(idx), …)`)。
- **結果の一致の範囲**: 表示済みのページ (フルスクリーンで開いているページ) は単枚 Ctrl+E と同じ結果、
  一覧の未表示ページは一括書き出しと同じ結果。未表示ページでは表示側の canonical 寸法が無いため、
  注釈の基準寸法と旧形式の切り取りの基準寸法が decode 後の寸法になる (一括書き出し・製本と同じ)。
  この既存の差 (旧形式 crop + AI 拡大、PDF ページの注釈) はコピー固有ではないので、本件では直さず
  backlog §1.462 として起票する。

### 拒否する条件 (Ctrl+E と同じ文言の系統で通知)

- フルスクリーン経由で、編集モード・補正パネル・解析モードが開いている
  (`is_overlay_edit_mode_active` / `adjustment_mode.is_open()` / `analysis_mode`)。
  編集途中の状態を取り込まないため。一覧からは編集モードは開いていないので判定しない。
- 対象ページの消しゴム補完 (Commit) が進行中 (`erase_inpaint_pending`)。
- 編集データ (隠蔽マスク・消しゴム・補正レイヤー・注釈) を読めない
  (`book_baked_edit_snapshot` の Err をそのまま通知)。

## 4. 出力の中身

- **永続データへは書かない**: `book_baked_edit_snapshot` が返す `crop_legacy_writeback` は、コピー用には
  `None` にして worker から DB・sidecar へ書き戻さない (遅れて終わる worker が、その間に利用者が変えた
  切り取りを古い矩形で上書きするのを防ぐ)。producer が UI スレッドで行う旧形式 crop の寸法補完
  (`ensure_export_crop_source_size`) は、表示時に同じページで行われる移行と同じもので、矩形の意味は
  変えない。こちらはそのまま使う。
- 段: `Settings::bake_stage_clipboard`。合成は `compose_export_entry(image, &edits, None, None, cancel)`、
  つまり Ctrl+E の `_0` (現在の設定) と同じ。隠蔽プリセット出力 (`_1`〜`_4`) と SNS 分割は対象外。
- 拡大縮小: **しない (原寸)**。Ctrl+E ダイアログの「倍率」は書き出し先ごとの選択で、
  クリップボードには持ち込まない。
- 透過: CF_DIB (24bit) は透過を運べないので、Ctrl+E を JPEG で書き出すときと同じ下地
  (`edits.jpeg_matte` = フルスクリーンの透過背景設定) に重ねて平らにする。
  既存コピーは alpha を捨てるだけ (透過部分の色は不定) なので、ここは新項目だけの扱い。
  透過を保ったまま渡す (PNG 形式の追加) は v1 の範囲外。
- 結果の一致: 同じ段の Ctrl+E (倍率 等倍、`_0`) を PNG で書き出した画素と、平らにする前の
  画素が一致する (テストで固定、§8)。

## 5. 非同期の形 (UI スレッドで重い処理をしない)

```
UI スレッド (メニュー dispatch)                     worker "clipboard-edited-copy"
  対象決定 → CompositeSource + BakedEditSnapshot      decode (ZIP 読み出し・PDF 描画を含む)
  を固定 (Ctrl+E / 一括書き出しと同じ producer)  →   compose_export_entry (段まで)
  mIV 世代 my_seq = reserve_clipboard_write_sequence   見開きは結合
  OS 世代 os_seq = GetClipboardSequenceNumber         matte で平らにする → DIB
  前の job の cancel を立てて置き換える               書き込み (下の「書き込みの順序」)
                                                        結果を送る → ROOT の repaint を要求
  update で poll → 結果を通知                   ←
```

- **job は App に 1 本だけ** (`edited_clipboard_copy: Option<EditedClipboardCopyPending>`)。
  新しい要求は前の job の cancel を立てて置き換える。結果の channel も置き換わるので、古い
  job の結果は届かない。組み合わせを「最後の 1 本だけが生きている」に減らす。
- **後から行ったコピーを上書きしない**: 規則は「要求した後にクリップボードが誰かに変えられたら、
  遅れて届いた画像は古い」の 1 つ。2 つの世代で判定する。
  - mIV 内の画像コピー同士 (既存コピー・動画フレーム等): 既存の `CLIPBOARD_SEQ` を予約して共有する。
  - それ以外 (パスやファイル名のテキストコピー、Ctrl+C のファイルコピー、他のアプリ): 要求時に
    `GetClipboardSequenceNumber` を baseline として記録する。
- **書き込みの順序** (共通 writer `write_image_clipboard_if_latest` に定義し、fake API で固定する):
  1. `CLIPBOARD_WRITE_MUTEX` を取り、DIB のメモリを用意する (重い DIB 構築は lock の外で済ませる)。
  2. `OpenClipboard` に成功してから判定する。ここから Close まで他のプロセスは書けないので、
     判定と書き込みの間に割り込まれない。
  3. 判定: `CLIPBOARD_SEQ == my_seq` かつ「OS 世代 == baseline、または OS 世代 == 直前の mIV 画像書き込みの
     直後の OS 世代 (`LAST_MIV_IMAGE_WRITE_OS_SEQ`)」。後者は、要求の時点で判定を済ませていた古い mIV の
     画像書き込みが、要求の後に書き終えた場合を「他人の変更」と誤認しないため (古い書き込みは
     mIV 世代で既に負けているので、上書きしてよい)。
  4. 通れば Empty → DIB → origin marker (同じ open 区間、既存の契約どおり) → Close。
  5. Close の後、mutex を持ったまま `LAST_MIV_IMAGE_WRITE_OS_SEQ` に現在の OS 世代を記録する。
     これは既存の画像コピーの書き込みでも記録する (記録するだけで、既存コピーの判定は変えない)。
  - 既存コピーの判定 (mIV 世代のみ・open 前) は変えない。OS 世代の判定は新項目だけが渡す。
  - UI スレッドを待たせる lock は増やさない (UI スレッドは予約と baseline 採取だけ)。
- **AI 利用中の印**: コピー worker は段に関係なく、終端まで `local_ai_activity_lease` を持つ。
  「編集まで」でも消しゴム補完 (MI-GAN) が worker 内で走るので、段で条件を分けない。
  置き換えで取り消された worker も、実際に終わるまで持つ (lease は worker が所有する)。
- **完了の wake**: worker は結果を送った後に ROOT の repaint を要求する。成功・失敗・取消を同じ終端で
  扱い、常時 polling の timer は置かない。
- RAW は `RawDecodeContext` (Ctrl+E と同じ `raw_develop_executor` + `raw_brightness`)。
- `perf::event` で要求・完了 (所要時間・寸法・段) を記録する。

### 通知

| 結果 | 通知 |
| --- | --- |
| 要求を受けたとき、段が AI まで進み AI が動く見込み | 「編集を反映した画像を準備しています…」(時間がかかるため) |
| 書けた | 「編集を反映した画像をコピーしました」 |
| 失敗 (decode・合成・メモリ確保・OpenClipboard など) | エラー通知 (理由つき) |
| 後のコピーに追い越された / 置き換えで取り消された | 何も出さない (利用者が後から行った操作が勝っただけ) |

既存コピーは失敗しても何も出さないが、そちらは変えない。

### まれな失敗

ディスク・メモリ不足・クリップボードを他のアプリが握っている等は、既定の割り切り方針どおり
**ログ + エラー通知のみ**。再試行はしない。終了時に job が残っていても待たない (worker は放置で終わる)。

## 6. 複数ウィンドウ (detached) との関係

- 新しいコードは detached 述語・viewport 経路に触れない。メニュー dispatch が既に
  「その窓の状態がマウントされた App」で動く前提に乗る (既存の「画像をクリップボードにコピー」と同じ)。
- job と通知は App 全体で 1 本。クリップボードは OS 全体で 1 つなので、窓ごとに持つ理由が無い。
  別の窓へ切り替えても job は続き、完了通知は通常の通知経路に出る。
- 実装時に「detached 窓のメニューから dispatch されたとき、`self.items` / `fullscreen_idx` が
  その窓のものを指す」ことをコードで確かめる。違っていたら止めて報告する (症状パッチを入れない)。

## 7. 設定・メニュー・キー

- `Settings::bake_stage_clipboard: BakeStage` (`#[serde(default = …)]`)。**未リリースの新設定**で
  移行は不要 (旧ファイルには欄が無く既定値になる)。
  - **既定値: 表示用補正まで (単枚 Ctrl+E と同じ)** — 利用者確認待ち (§9)。
  - 環境設定「書き出しの焼き込み」の表に行「クリップボードへコピー」(ヒント「右クリックの
    編集を反映した画像をコピー」) を足す。説明文の「画像をファイルへ出すとき」を
    「画像をファイルやクリップボードへ出すとき」に直す。
  - 設定の持ち運び (`settings_transfer`) に 1 行足す (他の 4 段と同じ扱い)。
- 右クリックメニュー: `ContextMenuItemId::CopyEditedImageToClipboard` / `MenuCommand::CopyEditedImageToClipboard`
  (stable name `CopyEditedImageToClipboard`、表示「編集を反映した画像をコピー」)。
  native / egui 両方のメニューに同じ model から出る。保存済みのメニュー並びを持つ利用者にも、
  既定位置 (既存コピーの直後) に現れること。
- キー: `KeyAction::FsCopyEditedImage` を足し、**既定キーは割り当てない** (フルスクリーン context、
  環境設定で割り当て可能)。対象はフルスクリーンのメニューと同じ。`docs/keymap.ini.default`・
  `docs/keymap-spec.md` を揃える。一覧のキーとリング操作は v1 では足さない。
- mIV Remote は対象外。

## 8. テスト

- 段ごとに出力が変わる (編集 / AI 処理 / 表示用補正 × 代表 params)。AI は既存テストの fake runner を使う。
- 同じ段の Ctrl+E (`_0`、等倍) と画素が一致する (単ページ・見開き)。
- matte: 透過画素が `JpegMatte` の色に平らになる。
- 世代: mIV 世代が進んだら書かない / OS 世代が変わったら書かない / どちらも同じなら書く
  (書き込み API を fake にした既存の `ClipboardImageApi` テストの形に乗せる)。
- 書き込みの順序: OS 世代の判定が `OpenClipboard` 成功後・`EmptyClipboard` 前に行われる。
  「古い A が判定済み → B が要求 (baseline 採取) → A が書き終える」の後でも B は書く。
  「B が要求 → テキストコピー」の後では B は書かない。DIB と origin marker は同じ open 区間。
- 永続データ: 旧形式 crop のページでコピーしても、worker が crop DB・sidecar へ書かない
  (`crop_legacy_writeback` が `None`)。コピー中に切り取りを変えても巻き戻らない。
- lease: 「編集まで」でもコピー worker が終わるまで AI 利用中に見える。
- wake: 失敗で終わったときも repaint 要求が出る。
- job の置き換え: 2 本目の要求で 1 本目の cancel が立ち、1 本目の結果は通知されない。
- 対象決定: 一覧の `Image` / `ZipImage` / `PdfPage` は 1 ページ、フルスクリーン見開きは左右、
  `Stack` 等はメニューに出ない。
- 設定: 環境設定の表で行を選ぶと `bake_stage_clipboard` が変わり、保存して開き直しても保たれる
  (編集 → OK → 再読み込みまで通す)。既定値のテスト。
- メニュー: stable name の catalog テスト、保存済み並びに新項目が既定位置で入る。
- 元データ非変更: 実行前後で編集 DB / 元ファイルが変わらない。
- UI スナップショット: 環境設定の焼き込み表が変わるなら更新して目視。

## 9. 決めること (利用者)

1. **段の既定値**: 推奨は「表示用補正まで」(単枚 Ctrl+E と同じ。画面の見た目に一番近い)。
   代案は「編集まで」(速い。AI を再計算しない)。
2. **見開きの扱い** (推奨のまま進める): フルスクリーンでは Ctrl+E と同じく左右を結合した 1 枚。

## 10. 規模

| 部分 | 行数の目安 (テスト込み) |
| --- | --- |
| 設定・環境設定の表・持ち運び | 80 |
| メニュー項目・dispatch・KeyAction | 120 |
| 対象決定と snapshot (producer の共通化) | 120 |
| worker・世代確認・matte・結果 channel・通知 | 350 |
| 文書 (マニュアル・設計文書) | 80 |
| 合計 | 約 750 |

実装 + 独立レビュー + 修正で **1〜1.5 日**。次の版に全部入れる前提で進める。
外すなら §7 の KeyAction だけを後回しにできる (約 50 行)。
