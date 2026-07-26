## 1. サマリ

- **不一致: 11 件**
- **リファクタ候補: 5 件**（P1: 1 / P2: 1 / P3: 3）
- **バグ: 3 件**
- 不一致 11 件は、すべて「文書が古い／文書内で矛盾」に分類した。コード側の不変条件違反はバグ 3 件として分離した。
- ファイル変更、ビルド、テスト実行は行っていない。テストは `target/` 等へ書き込むため、read-only 制約下では未実施。

## 2. 不一致リスト

### 1. display pipeline の crop 適用位置が誤っている

- **文書**: [display-pipeline.md:731](../display-pipeline.md:731)、[同:996](../display-pipeline.md:996)、[同:1032](../display-pipeline.md:1032)、[同:1163](../display-pipeline.md:1163)
- **コード**: [app.rs:4411](../../src/app.rs:4411)、[同:47865](../../src/app.rs:47865)、[同:49310](../../src/app.rs:49310)
- **矛盾**:
  - 文書は crop を `edit_result_cache` に含め、crop 変更で edit/final cache を失効し、連結読みレイアウトにも反映するとしている。
  - 実装の `EditResultKey` に crop 世代はなく、合成順は raw → erase → local-adjust → conceal。crop は通常表示で暗転 overlay のみ、実切り出しは capture/export 最終段。
  - [preset-and-adjustment.md:588](../preset-and-adjustment.md:588)、[同:622](../preset-and-adjustment.md:622)、[architecture-overview.md:176](../architecture-overview.md:176) はコードと一致している。
- **判定**: **display-pipeline.md が古い**。コードと preset-and-adjustment.md が正しい。
- **修正案**:
  - §2.3、§3.0 から crop を edit pipeline の構成要素として削除。
  - crop は「通常表示 overlay」「保存／書き出し時に final composite へ適用」と明記。
  - crop 変更時は edit preview WebP のみ失効し、edit/final AI cache は保持すると訂正。
  - 連結読みの配置サイズは export crop では変化しないと訂正する。

### 2. panorama 文書全体が「着手前」のまま

- **文書**: [panorama-360-view-plan.md:1](../panorama-360-view-plan.md:1)、[同:8](../panorama-360-view-plan.md:8)、[同:2039](../panorama-360-view-plan.md:2039)、[README.md:34](../README.md:34)
- **コード**: [xmp_reader.rs:904](../../src/xmp_reader.rs:904)、[panorama.rs:41](../../src/panorama.rs:41)、[app.rs:53957](../../src/app.rs:53957)、[keymap.rs:4903](../../src/keymap.rs:4903)、[ui_fullscreen.rs:16332](../../src/ui_fullscreen.rs:16332)
- **矛盾**: 冒頭は「調査＋設計まで」「承認後に着手」、Phase 1 のコード項目は全て未チェックだが、XMP、V キー、WGSL、GPU upload、final composite 入力、settle refinement まで実装済み。
- **判定**: **文書が古い**。
- **修正案**:
  - 冒頭に現在の実装ステータスを置く。
  - コード実装項目と手動検証項目を分離し、コード項目は完了へ更新。
  - 2,761 行のレビュー過程は「設計経緯」へ移し、現在仕様と未実装項目を先頭に再構成する。
  - 2K/11K/実機性能など手動検証の実施状況は今回未確認なので、根拠なく完了扱いにしない。

### 3. GPano partial panorama と mipmap が「将来／未実装」のまま残っている

- **文書**: [panorama-360-view-plan.md:45](../panorama-360-view-plan.md:45)、[同:2115](../panorama-360-view-plan.md:2115)、[同:2495](../panorama-360-view-plan.md:2495)
- **コード**: [panorama.rs:119](../../src/panorama.rs:119)、[app.rs:55214](../../src/app.rs:55214)、[panorama_wgpu.rs:239](../../src/panorama_wgpu.rs:239)、[同:373](../../src/panorama_wgpu.rs:373)、[同:397](../../src/panorama_wgpu.rs:397)
- **矛盾**:
  - 冒頭の GPano 表では `FullPano*` / `CroppedArea*` が「将来」だが、Phase 1.5 とコードでは実装済み。
  - Phase 2b の mipmap は未チェックだが、8K base は完全 mip chain と trilinear sampler を使用済み。
- **判定**: **古い表と古い Phase 2b checklist が誤り**。
- **修正案**: 現行仕様を GPano UV transform、水平 crop 時の ClampToEdge、完全 mip chain に統一する。settle overlay が 1 mipなのは現行どおり別記する。

### 4. panorama の App 状態と XMP cache 型が違う

- **文書**: [panorama-360-view-plan.md:97](../panorama-360-view-plan.md:97)、[同:1879](../panorama-360-view-plan.md:1879)
- **コード**: [app.rs:7375](../../src/app.rs:7375)、[panorama.rs:46](../../src/panorama.rs:46)
- **矛盾**:
  - 文書は `HashMap<String, XmpPanoramaInfo>`。
  - 実装は `HashMap<String, Option<XmpPanoramaInfo>>` で、`None` を「GPano なし」の負キャッシュとして使う。
  - 文書の `PanoramaState` には未実装の `inertia` があり、実装済みの `initial_yaw` / `initial_pitch` がない。
- **判定**: **文書の型定義が古い**。
- **修正案**: 現行構造体をそのまま写し、`Option` の負キャッシュ意味と初期姿勢の reset 不変条件を説明する。inertia は future backlog へ移す。

### 5. panorama_state のナビゲーション時ライフサイクルが文書内で矛盾

- **文書**: [panorama-360-view-plan.md:1895](../panorama-360-view-plan.md:1895) はファイル切替で `None`、[同:1994](../panorama-360-view-plan.md:1994) は保持して非アクティブ化。
- **コード**: [app.rs:7388](../../src/app.rs:7388)、[同:53994](../../src/app.rs:53994)
- **判定**: **後者とコードが正しい**。非パノラマへ移動しても pose を保持し、現在ページが panorama 候補のときだけ active。
- **修正案**: 「明示 OFF／fullscreen 終了で破棄、通常ナビでは保持して非アクティブ化」に統一する。

### 6. panorama の Wheel 仕様が文書内で二重化

- **文書**: [panorama-360-view-plan.md:1901](../panorama-360-view-plan.md:1901)、[同:1908](../panorama-360-view-plan.md:1908)、[同:2086](../panorama-360-view-plan.md:2086)
- **コード**: [ui_fullscreen.rs:16284](../../src/ui_fullscreen.rs:16284)
- **矛盾**: 旧記述と Phase 1 checklist は「Ctrl+Wheel のみ FOV、通常 Wheel は維持」。後続表と実装は「修飾キー不問で全 Wheel を FOV」に変更済み。
- **判定**: **後続表とコードが正しい**。
- **修正案**: 旧仕様と checklist を削除する。[ui_fullscreen.rs:16225](../../src/ui_fullscreen.rs:16225) の関数コメントにも旧説明が残るため、文書更新時に同時訂正対象。

### 7. panorama の PDF／スライドショー対応表が現行動作と違う

- **文書**: [panorama-360-view-plan.md:2029](../panorama-360-view-plan.md:2029)、[同:2033](../panorama-360-view-plan.md:2033)、[同:2315](../panorama-360-view-plan.md:2315)、[同:2500](../panorama-360-view-plan.md:2500)
- **コード**: [app.rs:54723](../../src/app.rs:54723)、[同:55123](../../src/app.rs:55123)
- **矛盾**:
  - PDF は「対象外」と「BaseOnly」が混在。実装では 2:1 判定による base panorama は可能だが、高解像度 settle は BaseOnly。
  - 表は panorama 中も slideshow 継続とするが、実装は panorama ON 時に slideshow を停止する。slideshow 連動は Phase 2b 未実装とも書かれている。
- **判定**: **コードと後続の実装メモが正しい**。
- **修正案**: PDF は「GPano XMP 読み込み対象外、アスペクト検出による base view は対応、高品質 settle は非対応」。slideshow は「入場時停止、pose 引き継ぎ連動は未実装」に統一する。

### 8. auto-thumb-aspect-plan の「コード未着手」

- **文書**: [auto-thumb-aspect-plan.md:7](../auto-thumb-aspect-plan.md:7)、[README.md:53](../README.md:53)
- **コード**: [auto_aspect.rs:33](../../src/auto_aspect.rs:33)、[同:190](../../src/auto_aspect.rs:190)、[app.rs:11335](../../src/app.rs:11335)、[同:11572](../../src/app.rs:11572)
- **判定**: **plan 冒頭だけが古い**。README とコードが正しい。
- **修正案**: 「実装済み。現行判定仕様と保守上の不変条件」に変更する。アルゴリズム本文は概ね一致。

### 9. fullscreen-side-panel-mode-plan が未実装扱い

- **文書**: [README.md:35](../README.md:35)、[fullscreen-side-panel-mode-plan.md:18](../fullscreen-side-panel-mode-plan.md:18)、[同:215](../fullscreen-side-panel-mode-plan.md:215)、[同:360](../fullscreen-side-panel-mode-plan.md:360)
- **コード／現行正本**: [display-pipeline.md:520](../display-pipeline.md:520)、[settings.rs:567](../../src/settings.rs:567)、[app.rs:52494](../../src/app.rs:52494)、[video/native_presenter/mod.rs:462](../../src/video/native_presenter/mod.rs:462)
- **判定**: **実装済みで、plan のターゲットモデルは概ねコードと一致**。§1 の「現状」と§3の実装指示、README の未実装表示が古い。
- **修正案**: 実装済みバナーを追加し、§1を「旧挙動」、§3を「実装マップ／保守箇所」へ変更する。実機検証完了の有無は今回未確認。

### 10. DPI 文書の viewport command 棚卸しとモジュール位置が古い

- **文書**: [dpi-multimonitor-issue.md:47](../dpi-multimonitor-issue.md:47)、[同:97](../dpi-multimonitor-issue.md:97)、[同:169](../dpi-multimonitor-issue.md:169)
- **コード**: [lib.rs:992](../../src/lib.rs:992)、[同:1132](../../src/lib.rs:1132)、[app.rs:37317](../../src/app.rs:37317)、[同:58059](../../src/app.rs:58059)
- **矛盾**: 「Title/Close の2箇所のみ、位置・サイズ操作なし」は現在は成り立たない。初回 `InnerSize` 再適用、ROOT 最大化、detached の位置・サイズ操作がある。起動コードも `main.rs` ではなく `lib.rs`。
- **判定**: **文書が古い**。
- **修正案**: 現在の viewport 操作を main ROOT と detached に分けて列挙する。「問題は必ず winit 側」とする断定は外す。元の Win+Shift+Arrow 症状の現在の根因は、実機・上流コードを再検証していないため**未確認**。

### 11. fullscreen-navigation-consistency のマウス hook 所在が古い

- **文書**: [fullscreen-navigation-consistency.md:255](../fullscreen-navigation-consistency.md:255)
- **コード**: [lib.rs:492](../../src/lib.rs:492)、[同:573](../../src/lib.rs:573)
- **判定**: **モジュール移動後の文書更新漏れ**。`main.rs` ではなく `lib.rs`。
- **修正案**: パスのみ訂正。ナビゲーションの挙動記述は、今回確認した範囲では概ね一致している。

## 3. リファクタ候補リスト

### P1. 単一ページの「実表示 transform」を一元化する

現在の active root viewer には、実表示矩形／transform の生産系が**12系統**ある。

- 正本: [fs_image_draw_rect_for_size](../../src/ui_fullscreen.rs:1448)
- 特殊レイアウト: Z [draw_fs_zoom_mode](../../src/ui_fullscreen.rs:2655)、見開き [layout_spread_page_rects](../../src/ui_fullscreen.rs:1638)、連結読み [continuous_reading_layout](../../src/ui_fullscreen.rs:15333)
- 独自再計算: ルーペ [ui_fullscreen.rs:17408](../../src/ui_fullscreen.rs:17408)、比較 [同:16151](../../src/ui_fullscreen.rs:16151)、crop [同:8315](../../src/ui_fullscreen.rs:8315)、capture [同:20392](../../src/ui_fullscreen.rs:20392)
- 編集系4本: [ui_erase.rs:892](../../src/ui_erase.rs:892)、[ui_conceal.rs:759](../../src/ui_conceal.rs:759)、[ui_adjustment_panel.rs:2781](../../src/ui_adjustment_panel.rs:2781)、[ui_text.rs:2338](../../src/ui_text.rs:2338)

このうち、ルーペ／比較／crop／capture／編集4本の**少なくとも8箇所**が contain 式を再実装している。分析モードは専用 viewport を作るが、画像描画自体は正本ヘルパを使うため独立コピーには数えていない。detached の frozen snapshot には別途、見開きレイアウトの複製があるが、この12系統には含めていない。

提案する型は単なる `Rect` では不足する。例えば以下を1回だけ解決する `DisplayedImageTransform` が必要。

- source/page idx、source/texture size
- full image rect、paint rect、hit rect、UV rect
- rotation/free rotation、content bbox
- screen↔source の逆変換
- fit mode／scale limit
- normal zoom/pan または Z transform

通常描画、Z、ルーペ、crop、capture、編集ツールはこの結果を消費し、自分で fit 式を持たない。

- **なぜ問題か**: バグ #2、#3 が実際に発生している。ポインタ座標のずれは、表示位置の問題だけでなく誤った画素へマスクや crop を確定する可能性がある。
- **影響範囲**: 6〜7ファイル、約600〜1,000行。比較・capture・編集4系統へ波及。detached は凍結対象なので、純関数を将来利用できる形に留め、現行 detached 経路を変更しない。
- **回帰リスク**: 高。fit全種、no-up/downscale、回転、表示トリム、Z、PDF再レンダ、編集座標が対象。Windows実機で DPI とポインタ位置確認が必要。
- **テストで担保できるか**: 大半は純関数テーブルテスト可能。既存の Z／spread／capture geometry テストを基盤に、screen→source→screen 往復、fit×rotation×trim の組合せを追加できる。最終的なポインタ感覚は実機確認が必要。
- **規模**: **Medium**
- **優先度**: **P1**

比較モードが常に Page fit を意図しているかは文書に明示がなく**未確認**。統合前に仕様を確定する必要がある。

### P2. Single／見開き／連結読みを `FullscreenPageLayout::hit_test()` に集約する

現状は、見開きだけが永続的な [FsSpreadLayout](../../src/ui_fullscreen.rs:1277) を持ち、連結読みの [VerticalReadingPage](../../src/ui_fullscreen.rs:1333) は描画関数内の一時値。Single は各 consumer が再構築する。

`FullscreenPageLayout { pages: SmallVec<DisplayedPage> }` をフレーム単位で作り、`hit_test(pos) -> DisplayedPage` に統一する。ページには上記 `DisplayedImageTransform` を持たせる。

- **なぜ問題か**: ルーペの連結読みバグが発生済み。今後の capture、選択、注釈機能追加でも `spread_double` bool に分岐を足す事故が起こる。
- **影響範囲**: 主に `ui_fullscreen.rs` と geometry テスト、新規モジュールを含め1〜3ファイル、約300〜600行。
- **回帰リスク**: 中〜高。RTL/LTR、横・縦連結、表紙、横長ページ、見開き gap、表示トリム、anchor 更新。
- **テストで担保できるか**: 純粋な hit-test とレイアウトテストで大半を担保可能。縦横スクロールの視覚確認は追加した方がよいが、Windows native 機能への依存は薄い。
- **規模**: **Medium**
- **優先度**: **P2**

### P3. panorama の session/resource ownership を typed state にする

[app.rs:7388](../../src/app.rs:7388) 以降で、pose、upload、high-res source、refinement、quality、worker channel、pending、failed、request sequence が別々のフィールドとして管理されている。

`PanoramaRuntime::Off | Active(PanoramaSession)` と、セッションをまたぐ XMP/quality catalog を分離する。GPU upload、refinement、high-res request/cancel は `PanoramaSession` が所有する。

- **なぜ問題か**: OFF 後に worker が完了する、古い upload と新 pose が共存する、といった無効状態を型で防げない。[toggle_panorama_mode](../../src/app.rs:55130) の多数の coordinated clear と stale guard に正しさが依存している。
- **影響範囲**: `app.rs`、`panorama.rs`、`panorama_wgpu.rs`、`ui_fullscreen.rs` の4ファイル、約800〜1,500行。
- **回帰リスク**: 高。最大GB級 RGBA の寿命、GPU resource drop、cancel、再ON、ナビゲーション、viewer context。
- **テストで担保できるか**: pose/quality/stale transition は単体テスト可能。GPU resource寿命、大画像、実際の settle は実GPU確認が必要。
- **規模**: **Large**
- **優先度**: **P3**

### P3. folder navigation burst を1つの状態所有者へまとめる

現在は [FolderNavPending](../../src/app.rs:3457) が `mode` を持つ一方、[app.rs:6660](../../src/app.rs:6660) に `folder_nav_pending`、`pending_folder_nav_steps`、`pending_folder_nav_mode` が分離され、「pending 中は mode が一致する」というコメント上の不変条件になっている。

`FolderNavBurst::Idle | Running { pending, queued_delta }` にし、mode を重複保持しない。

- **なぜ問題か**: cancel／take／chain／scope変更の各経路に reset が分散している。mode がずれると、累積した次ステップを別の fullscreen/search/smart-folder scope へ適用しうる。
- **影響範囲**: `app.rs` と `app/tests.rs`、約250〜500行。AppBundle と context ownership に波及。
- **回帰リスク**: 中〜高。Ctrl+↑↓、兄弟移動、検索、smart folder、変換アーカイブ、detached navigation。
- **テストで担保できるか**: 既存の pending/cancel/context テストが多く、state-transition テストでかなり担保可能。native動画入口は実機確認が望ましい。
- **規模**: **Medium**
- **優先度**: **P3**。detached freeze と交差するため、現在のリワーク段階と調整せず v2.8.1 に入れるのは避ける。

### P3. ui_fullscreen.rs の責務分割

[ui_fullscreen.rs](../../src/ui_fullscreen.rs:1) は26,235行あり、geometry、入力、navigation、連結読み、比較、パノラマ、crop/capture、音楽、detached snapshot、約129個のテストが同居している。

- **なぜ問題か**: 今回確認した矩形計算の複製や、panorama の古いコメントが同一巨大ファイル内で見逃されている。変更箇所の所有者が分からず、同型経路の棚卸しコストが高い。
- **影響範囲**: 8〜12ファイル、数千行の移動。geometry/layout、navigation、panorama adapter、overlay consumer 単位の分割が候補。
- **回帰リスク**: 高。大規模な機械移動でも借用境界や cfg(windows) を壊しうる。
- **テストで担保できるか**: コンパイルと既存単体テストで多くを確認できるが、静止画／動画／音楽／native presenter の手動 sweep が必要。
- **規模**: **Large**
- **優先度**: **P3**。P1/P2 の geometry ownership を確定してから行うべきで、先にファイルだけ分割しても重複構造は残る。

## 4. バグリスト

### 1. 連結読み中のルーペがカーソル直下ページを解決しない

- **症状**: 連結読み中、カーソルが別ページ上にあってもアンカーページ `fullscreen_idx` を拡大する。バックログにも実機確認済みとして記録されている。[next-release-backlog.md:142](../next-release-backlog.md:142)
- **壊れている不変条件**: 「ルーペの page idx・page rect・texture は、そのフレームでカーソル直下に実際に描画されたページから得る」。
- **原因経路**:
  - 連結読みはページごとの矩形を作るが、描画関数内だけの一時値。[ui_fullscreen.rs:15333](../../src/ui_fullscreen.rs:15333)、[同:15713](../../src/ui_fullscreen.rs:15713)
  - 描画後に `fs_spread_layout` を破棄。[同:15832](../../src/ui_fullscreen.rs:15832)
  - ルーペは Single／Spread の二分岐しかなく、Single 側で引数の anchor texture と `full_rect` から矩形を再構築。[同:17408](../../src/ui_fullscreen.rs:17408)
- **同型入口・終了経路**:
  - 見開きは `FsSpreadLayout` の hit rect を使うため別経路。
  - capture region は連結読みを明示的に拒否する。[同:20353](../../src/ui_fullscreen.rs:20353)
  - panorama／分析／一部編集はルーペを抑止。
  - したがって、確認できた漏れはルーペ経路。解決は P2 の統一 hit-test で行うべき。

### 2. Z ズーム中のルーペが通常 zoom/pan の矩形を使う

- **症状**: Z ズーム中、ルーペの拡大位置がカーソル位置からずれる。バックログに実機確認済み。[next-release-backlog.md:162](../next-release-backlog.md:162)
- **壊れている不変条件**: 「画面→UV逆変換は、画像本体を描いたのと同じ transform を使う」。
- **原因経路**:
  - Z 描画は `fs_zoom_factor` とカーソル基準の cover pan を解決して `draw_fs_image` へ渡す。[ui_fullscreen.rs:2655](../../src/ui_fullscreen.rs:2655)、[同:2750](../../src/ui_fullscreen.rs:2750)
  - ルーペは `fs_zoom_pan()`、すなわち通常の `fs_zoom` / `fs_pan` だけで矩形を再構築。[同:17449](../../src/ui_fullscreen.rs:17449)
  - 状態自体も [app.rs:8120](../../src/app.rs:8120) の通常系と [同:8129](../../src/app.rs:8129) のZ系に分離されている。
- **同型入口・終了経路**:
  - 見開きZは描画後の `FsSpreadLayout` をルーペが参照するため、今回確認された単ページ経路とは異なる。
  - Z は連結読み／panorama／動画／分析では無効。
  - ルーペ側へ `fs_zoom_factor` 分岐を追加せず、P1のtransform共有で直すべき。

### 3. 非Page fit／表示トリム時に編集・crop overlay の座標が画像本体とずれる

- **症状／条件**: Width、Height、Original、MarginFit、no-upscale、no-downscale、表示トリムのいずれかが有効な状態で、消しゴム・隠蔽・局所補正・テキスト・cropへ入ると、描画された画像と overlay／hit-test の矩形が異なる。回転については crop 側にも既知制限としてコメントされている。
- **壊れている不変条件**: 「編集入力の screen↔source 変換は、画面に描いた画像の fit・trim・rotation・zoom/pan を全て共有する」。
- **原因経路**:
  - 画像本体は fit mode、scale limit、content bbox を正本ヘルパへ渡す。[ui_fullscreen.rs:8034](../../src/ui_fullscreen.rs:8034)
  - 編集呼び出しは `image_rect` と `fs_zoom_pan()` しか渡さない。[同:8236](../../src/ui_fullscreen.rs:8236)
  - 各編集系は独自の Page contain 式を持つ。[ui_erase.rs:892](../../src/ui_erase.rs:892)、[ui_conceal.rs:759](../../src/ui_conceal.rs:759)、[ui_adjustment_panel.rs:2781](../../src/ui_adjustment_panel.rs:2781)、[ui_text.rs:2338](../../src/ui_text.rs:2338)
  - crop overlay も同じ簡略式。[ui_fullscreen.rs:8315](../../src/ui_fullscreen.rs:8315)
- **同型入口・終了経路**:
  - `enter_erase_mode`、`enter_conceal_mode`、`enter_local_adjust_mode`、`enter_text_mode`、`enter_export_crop_mode` の5入口が同型。
  - 見開きからSingleへのpivotは各入口にあるが、fit mode／trimは共通リセットされず、独自矩形側にも渡らない。
  - capture は別の重複実装だが、現状は fit mode／scale limit／content bbox を受け取るため、このバグの確定対象には含めていない。
  - 比較表示も fit mode／trimを受け取らないが、「比較中はPage fit固定」が意図か文書から判断できず、**未確認の疑義**としてバグ件数には含めていない。
- **確認水準**: 実機症状の見え方は未確認。ただし条件成立時に2つの計算式が異なることはコード上で確定している。

## 5. 総評

この領域で**最も信頼できない文書は panorama-360-view-plan.md**。実装前レビュー、実装済み追補、未更新チェックリスト、将来案が同居しており、同じ機能について相反する記述が複数ある。README の「現行仕様」という分類も現状には合わない。現在仕様、実装履歴、将来計画の3文書へ分割するのが望ましい。

`display-pipeline.md` は全体としてコードに追従しており、パノラマの final composite、mipmap、連結読み、サイドパネルは信頼できる。ただし、**crop の位置だけは恒久正本として重大な誤り**があり、`preset-and-adjustment.md` と実装に合わせて優先訂正すべき。

文書別の信頼度は次のとおり。

- **高**: `preset-and-adjustment.md`、`downscale-moire-lod-plan.md`
- **概ね高、一部訂正必須**: `display-pipeline.md`、`fullscreen-navigation-consistency.md`
- **ターゲット設計は正しいが実装状況が古い**: `auto-thumb-aspect-plan.md`、`fullscreen-side-panel-mode-plan.md`
- **調査履歴としてのみ利用すべき**: `dpi-multimonitor-issue.md`
- **現行仕様としては信頼不可**: `panorama-360-view-plan.md`

v2.8.1にコード変更を載せるなら、症状別の分岐追加ではなく、P1の単一ページ transform owner とP2のpage-layout hit-testを順に導入するのが整合的。detached viewer は凍結ルールに従い、今回確認した複製についても構造報告に留めるべき。