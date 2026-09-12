# 機能別レビュー

対象・最終 HEAD・結論は [README](README.md)、個々の問題は [findings](findings.md) を参照。
この文書の「確認」は、差分と周辺の producer / consumer / lifecycle の静的確認、および関連する自動テストの結果を指す。Windows GUI の実機合格を示さない。

## 読んだ方針と評価基準

`docs/README.md` から、architecture、UI 応答性、表示、キー操作、設定、外部ツール、動画、Remote 等の対応する正本へ進み、CLAUDE.md の関連節を確認した。過去の brief の未実装案を、そのまま現在の仕様とは見なしていない。特に外部ツールのプレースホルダ削減、Grid 専用キー、メニューの平坦化、Merged の例外は、後の利用者判断と対応状況で照合した。

主な正本: `architecture-overview.md`、`ui-responsiveness.md`、`display-pipeline.md`、`keymap-spec.md`、`key-customization-impl-plan.md`、`detached-rework-plan.md` §2、`external-tool-launch-plan.md`、`context-menu-unification-plan.md`、`item-kind-capability-matrix.md`、`bake-stage-unification-plan.md`、`video-seek-strip-plan.md`、`video-architecture.md`、`fullscreen-side-panel-mode-plan.md`、`rating-list-view-plan.md`、`settings-sqlite-migration.md`、`panorama-360-view-plan.md`、`preset-and-adjustment.md`、`web-remote-plan.md`、`web-remote-ai-plan.md`、`touch-support-plan.md`。開発・リリースの検証手順も確認した。

判断基準は「待たせる時間を短くしたか」だけではない。重い処理が UI から外れているか、非同期結果の所有者が特定できるか、同じ操作が別の入力や viewer からも同じ結果になるか、部分失敗を成功で隠していないかを重視した。

## R01 外部ツール登録・引数・起動

変更: 旧「プログラムから開く」の手動登録を外部ツール設定へ移行。Exe / 関連付け、`{files}`、作業フォルダ、Each / Batch、件数確認・上限、対象種別、固定キースロットとピッカーを追加。

確認した設計:

- 引数を parse して OS の引数へ渡す。ユーザーのファイル名を shell command へ再解釈させる組み立てではない。空白、引用符、末尾バックスラッシュ、複数ファイルと Windows の長さ制限の扱いを確認。
- 操作対象は typed な launch target で解決。通常ファイル、コンテナ、仮想ページ、スタック、音声/動画フレームを区別し、仮想識別子を実ファイル名として外部へ渡さない。
- 件数制限、確認、変更された選択との分離は materialize request の入口にある。コンテナの入口は選択集合へ勝手に拡大しない。
- 外部ツールは登録順に全件をメニューへ出し、10 はキーのスロット数。登録上限と取り違えていない。
- `Command` / Shell 起動に既存プロセスを kill する処理は確認されなかった。Shell/関連付けアプリ自体の内部挙動はこの保証の外。

残る問題: **F02** (Batch の部分失敗)、**F15** (移行失敗後の通常保存)。関連付け handler と Windows 側の既存プロセスへの転送は実機で確認が必要。

## R02 実体化・一時ファイル・起動 ACK

変更: ZIP / PDF 内ページ、動画の現在フレーム、補正済み出力、見開き単体/両ページ/合成を外部ツールへ渡す。一時ファイルを要求・プロセス・再利用 cache の寿命で管理。

確認した設計:

- TempOriginal と TempEdited、VideoFile と VideoFrame の境界が明示されている。音声を画像フレーム扱いする条件は除外され、TempOriginal + Merged の不一致は effective policy で解決されている。
- 元データと編集 snapshot を固定し、navigate で別のページへすり替えない。明示 cancel / supersede / close / launch failure の cleanup と、起動済み temp の保持を区別する。
- 一時出力は create-new と handle を使用。名前の検証、reparse point を辿らない確認、プロセス単位 directory と孤児判定を確認。通常ファイルの削除やユーザーの portable directory の掃除へ広がる変更は見つからなかった。
- cache は source / policy / stage / edit fingerprint を持ち、出力ファイル側の変更も stamp で再利用から外す。識別子だけで古い出力を無条件再利用してはいない。
- UI frame の処理を `update_frame` に分けた後も ACK の tail を通る。native 動画の早期 return、modal の操作元 viewport、viewport 消滅時の引継ぎを確認。frame number だけで別窓の ACK と同一視する旧問題には対応がある。
- main HWND を外部起動の owner とする方針、メニュー HWND とアプリ viewport の寿命が違う点は、現在の設計判断に沿う。

残る問題: **F03 / F05** (Merged の段・UI 合成)、**F09** (Remote AI barrier)、**F10** (スタックの LUT・お気に入り継承)。F02 の結果粒度は temp ownership にも関係する。

## R03 右クリックメニュー統一・HWND

変更: native と egui fallback が共通の純粋な menu model を使用。画像/動画/音声/実フォルダ/ZIP/PDF/仮想ページ/スタック/検索コンテナ、単一/checked/背景を表現。Windows Shell メニューを遅延構築。

確認した設計:

- model の capability と実行時 resolver を併せて確認。実 path を持たないページや混在選択を、誤った削除/コピー対象へ変換しない。表示項目だけに依存した保護ではない。
- native / fallback の command dispatch を共有。背景コンテナ、checked 対象、クリックした項目の関連付け起動の意味を分けている。
- HMENU、Shell interface、subclass callback の lifetime と message forwarding を確認。遅延 Shell submenu を開くまで `QueryContextMenu` しない構造は適切。
- tooltip HWND の owner / z-order とメニュー終了時の破棄を確認。外部ツール実行の HWND へ偶然メニュー HWND を渡す構造ではない。

残る問題: **F13** (メニューを表示する前の関連付け列挙が同期)、**F16** (キー表示が固定)。Windows extension、DPI、別窓 foreground は実機項目。

## R04 編集一括貼付・解除・undo

変更: 複数ページに編集内容を貼付、または種類を選んで解除。対象外・成功・失敗・未処理を集計。既存の単一処理と共有する保存・runtime 反映・undo を拡張。

確認した設計:

- confirm / producer fence 待ち / worker 実行 / 結果表示が typed な phase で表現されている。
- local-adjust の producer 完了と保存 worker の fence を待ち、後着の古い保存が一括操作を上書きする順序逆転を防ぐ。
- DB snapshot の strict read は破損/読み取り失敗を「編集なし」として消去しない。選ばなかった編集には memory override を保持する。tag / rating はこの解除の対象にしない。
- worker の transaction 成功結果だけを適用。cancel は既に成功した編集を戻さず未処理を止める。終了時は bounded channel を drain してから join し、send と join の相互待ちを避ける。
- 解除した種類だけの undo 失効、回転のみの経路、bundle 失敗時に回転だけ進めない処理を確認。

残る問題: **F08**。request / completion の viewer context identity が欠ける。保存順の修正は根本的だが、複数窓での runtime 更新先は根本解決に至っていない。detached BA-7 に該当し、機能全体を context/key を持つ owner に接続する修正が必要。

## R05 一括書き出し・隠蔽 preset・焼き込み

変更: 一覧の Ctrl+E、複数出力、隠蔽 preset の各版出力、共通 `BakeStage`、AI runner の受け口、LUT 等の表示補正。

確認した設計:

- 同名ファイルを無言で上書きしない出力予約、出力名の整形、形式別 alpha/matte、共通 resize 経路を確認。
- single preset 版では隠蔽前の base と段を固定し、別 preset の結果を次の版の入力に累積しない。元画像版と各 preset 版を区別し、AI activity lease を持つ。
- compositor は編集 → AI (runner がある場合) → 表示補正 → 注釈 → 回転/crop を一つの経路へ集約。部品としての段適用・指紋はテストされている。
- batch の cancel は item 間で、実行中 item の完了を待つ。マニュアルも「まだ始まっていない分」を対象としており、この仕様を新しい不具合には数えない。

残る問題: **F03** (段の受け口と UI が先行し、実際の source/model producer が未接続)、**F04** (ダイアログ前に N 件を UI で準備)、**F09** (新しい worker の AI ownership)。**F01** は更新後の全体 gate で解消を確認。

既知計画の未完了を「設計文書に残っているだけ」とは扱わない。現在の UI で選択できる値が期待した出力に結び付くかがリリース判断の基準。

## R06 描画 geometry・端数ピクセル・Lanczos

変更: source texel と表示矩形の共通化、physical pixel への extent / origin の量子化、表示逆変換、連結読みでのページ間配置、GPU 拡大縮小の出力寸法。

確認した組合せと不変条件:

| 組合せ | 確認内容と結論 |
|---|---|
| 等倍/縮小/拡大、DPI 100/125/150/200% | f64 の寸法計算、整数近傍の誤差許容、それ以外の floor と min 1px。extent と origin の区別は適切 |
| 90/180/270°回転、左右/上下反転 | source UV と表示軸の入替え、clip 後の UV、screen→image 逆変換が共通 transform に接続 |
| trim/crop とクリップ境界 | PixelTexels と Proportional の意味を区別。切抜いた可視範囲と元 source 全幅を混ぜない |
| 単ページ/見開き/縦横連結 | 共通の見える寸法を用いる改善は適切。ただし origin をページごとに round する F12 が残る |
| source / 縮小 texture / final composite の差替え | 同じ表示 contract で寸法を再計算。異なる texture size が偶然ページ間 gap を変える旧構造を縮小 |
| Lanczos / nearest / NIS / Anime4K / freeze | 出力 physical size と表示 rect が別々の端数処理で乖離しないかを確認。live と frozen の表示 transform を共有 |
| 奇数の縦長ページ、負原点、スクロール | **F12 を計算実行で確認**。round の .5 tie は整数移動と可換でなく、隣接辺が 1px 分離 |

「幅を揃えたので隙間も正しい」という推論は成立しなかった。共通 extent への変更は根本方向として適切だが、unit 間の共有辺/原点も同じ格子の所有者で解決する必要がある。

すべての組合せを実 GPU で描画して測定したものではない。数値再現条件と次の描画回帰ケースは verification.md に記録。

## R07 動画ストリップ全尺・高さ・波形 cache

変更: 非表示・周辺サムネイル・周辺波形・全体サムネイル・全体波形、cycle 対象、高さ、全体範囲の absolute seek、永続波形 cache。

確認した設計:

- 5 状態の typed view から表示内容・範囲を求める。cycle 候補がすべて OFF でも有効な候補へ正規化される。
- whole 範囲は duration と cell 数から区間を定義し、end の epsilon、短い動画、極小 duration、cell 上限を処理する。
- window の相対操作と whole の絶対 seek、drag release 時の commit を区別。全体ストリップ上の wheel が背後のズーム/ページ操作へ流れない設計を確認。
- waveform は worker 側で処理し、cache key はファイル identity と解像度。表示の window/whole だけで同じ音声を別データにしない。末尾 chunk の不足を受理しない。
- native presenter の strip / panel / surface policy は予約領域を共有する方向に変更されている。

残る問題: **F07**。Hidden の layout 用 span を、最後に利用者が選んだ span の代わりに使っている。key と drag 復元の結果が異なる。native wheel の既存テスト登録漏れも検証の穴として記録。

## R08 情報パネル固定

変更: 静止画・動画・音楽の情報パネルを viewer ごとに固定し、表示領域を予約する。

確認した設計:

- lock 状態の bundle 初期化 / mount / unmount / swap、navigate 継続と exit、編集モード/360等との有効判定を確認。
- 描画側と主要 wheel / touch classifier の可視述語を追跡。背景パネルの click 消費があるため、古い述語が残るだけで click-through と断定していない。
- 動画 native 側の surface reservation へ同じ lock が伝わることを確認。

残る問題: **F11**。狭い音楽ビューで予約後の幅から予約量を逆算しており、clamp の分岐を失う。640pt で右端が 55pt はみ出す。共通 layout snapshot の rect を渡す修正が必要。cursor hide と touch handle の旧述語は実機追加項目。

## R09 ★日時順・一覧・smart folder

変更: ★設定日時の並び順、専用列、rating view の一時ソートと復元、smart folder の timestamp 経路。

確認した設計:

- timestamp 順の query 結果を、その後のファイル種別の regroup が上書きしないようにしている。
- rows と materialized items の対応を保ち、存在しない項目が落ちても別項目の日時を付けない。通常の名前等の sort は既存経路を保持。
- 一時的な RatedAt 列・幅・header 操作は rating view の所有物。入退出、history restore、smart folder 切替時のグローバル sort と混線しないかを確認。
- Remote collection 側の row order と setting enum 消費も確認。今回の変更による確定した退行は見つからなかった。

手動では、同一時刻の安定順、欠損元ファイル、仮想ページ、複数の smart folder の往復、Remote ページ送りを確認する。

## R10 設定互換保護・移行

変更: 未知 enum / 将来設定を破損として扱わず、非互換として save を抑止。外部ツール table、旧登録/最近利用アプリの移行、設定項目と共有 bundle の追加。

確認した設計:

- 将来版 app_version と未知 enum を、quarantine / defaults 上書きの経路へ流さない設計は適切。新規 bootstrap と既存 DB を区別する基本構造も維持。
- migration 本体は transaction でデータと marker を同時に commit。未公開 table 形状の扱いは公開済みユーザー schema の破壊とは区別。
- gamepad 設定が無い旧共有 bundle は true を既定にし、取り込みだけで勝手に無効化しない。新しい値は import/export/reset に接続。

残る問題: **F15**。migration の transaction は正しくても、load が失敗を飲み込んだ後の通常 save が marker を確定し、retry 契約を破る。成功パスだけの migration テストでは見つからない lifecycle の不整合。

## R11 panorama 引継ぎ・crop のドラッグ

変更: viewer ごとの 360 session intent、投影方式の保持、キー/ボタン/復帰の entry 判定統一。crop 新規作成の anchor と、panel を横切る drag ownership を修正。

確認した設計:

- 素材ごとの PanoramaState、session の intent、環境設定の default projection を分離。素材を変えた時に利用者の既定設定を書き換えない。bundle の引継ぎ箇所を確認。
- V キー、上部バー、復帰は共通 entry predicate を使用。静止画の見開き/連結と動画の制約を混ぜない。
- ratio 付き crop 作成は押した点を anchor とし、四象限と画像端の room から寸法を決める。既存 rect の比率変更は center 基準を保持。
- drag 開始後は画像から panel へ移動しても所有を維持し、button up で終了。単なる hover 判定を追加して凍結症状を隠す修正ではない。
- IME のある新規 text field / dialog の Enter・Escape は既存 helper を確認。

確定指摘なし。最小 1px の rect を置けない画像端では sanitizer による anchor 移動があり、コード上で理由が明示されている。ペン/タッチの capture と極端な比率は実機の確認項目。

## R12 入力と操作カスタマイズの横断

| 入力 | 確認範囲 | 結果 |
|---|---|---|
| キーボード | 新しい KeyAction の列挙、stable ID、context、説明、既定割当、実行 handler、IME、repeat、保存/共有 | 外部ツールは既定なし・Grid 専用という合意に一致。Ctrl+E は Grid action。F07 / F16 が残る |
| マウス | native/fallback menu、checked/背景、strip click/drag/wheel、crop drag、panel の占有領域 | 共通 dispatch / gesture owner は適切。F11 と panel の追加確認あり |
| タッチ | pointer emulation、strip absolute seek、panel classifier、crop capture、右クリック相当の既存経路 | 新機能の全操作を新しい touch shortcut にする仕様ではない。キー専用というだけで退行とは扱わない。端末での gesture は未確認 |
| ゲームパッド | 有効化の runtime stop/start、保持 button/axis、repeat、analog、focus context、共有 bundle | **F06**: runtime が止まっても consumer の保持入力が残る。OFF の不変条件を満たさない |
| カスタマイズ | KeyAction resolver、menu label、既定なし slot、旧 bundle、reset/import/export | コマンド追加の主要配線はある。**F16** の固定ラベルは変更/解除に追随しない |

gamepad OFF は読み取り thread だけを止めても十分でない。保持 state と repeat/repaint の producer を同じ有効状態の遷移で終了する必要がある。

## R13 mIV Remote

Remote 自身の差分は小さいが、本体の model policy、compositor、DB、sort、worker ownership の変更を横断した。

確認した設計:

- `remote_ipc/container.rs` の AI policy 呼び出しに加わった引数は `None` で、ローカル出力用の override を Remote の選択へ混入させていない。
- shared settings/AdjustParams と画像合成の新しい段・LUT を、Remote がどの入口から使うかを確認。新しい外部ツールの機能が IPC command として無意識に公開される変更はない。
- acquire / LocalActive / RemoteActive と local runtime の停止確認を追跡。元から登録されている final AI、製本、erase、local-adjust、video upscale、activity lease の待ち合わせ構造を確認。
- Remote Rust テストは全体 gate の対象。Web の 9 ファイル / 382 テストも実行。command / coordinator / runtime / position / double-tap / video / timing / settings / PWA の既存回帰を確認した。

残る問題: **F09**。新しい batch export と materializer の消しゴム AI が barrier の活動数に登録されない。出力 dialog の有無では、既に走っている worker の停止を保証できない。

別途、DB 一括 mutation の進行中に Remote が acquire した場合の画像 cache / selection の更新を実機で確認する必要がある。これは現時点では確定した cache bug に数えていない。スマートフォンの接続・切断・復帰、実 DirectML 同時実行、実動画 stream は今回実施していない。

## R14 小差分・削除・マージ整合

- `tag_legacy_xmp_worker` と手動旧 sidecar 導線の撤去を、現在の tags DB / metadata transfer と照合。新しい正本へ無関係な legacy producer を残さない整理であり、タグ操作全体を削除する変更ではない。
- 仮想 item の file operation refusal、D&D の説明、folder tree / name bulk indexer の CBZ/PDF 判定、metadata_ops の公開範囲を確認。
- mask / conceal / comic / crop の strict DB reader は新しい bundle 経路のための追加で、旧 tolerant reader の全呼出しを無条件変更していない。
- rotation の Result、undo invalidation、content identity の fixture、library/module export、cache manager の波形表示、tray の外部変更チェック呼出しを確認。
- 同名外部上書きの検出で directory mtime だけを信じない修正は原因に対応する。ただし **F14** の UI 同期走査が残る。
- `src` / `crates` / `tests` に merge conflict marker は見つからなかった。型の整合性だけでは F08 / F09 の所有境界は保証されない。
- レビュー途中の version 更新は Cargo/installer/manual/重要変更点に反映。Windows 専用関数の cfg 修正を追認。Linux CI 自体はこの Windows 環境では実行していない。

## R15 自動検証から言える範囲

全体 gate は 8,133 passed / 0 failed / 36 ignored。UI snapshot integration 43 件も含む。Remote Web 382 件は別枠。fmt と glyph 検査も合格。

今回の例が示す通り、純関数・単一 context・成功パス中心のテストは、複数窓の drain 先、保持入力を残した OFF、移行失敗後の save、符号を跨ぐ half-pixel のような組合せを保証しない。実機チェックと、各指摘の境界に置く回帰テストを分けて [verification.md](verification.md) に記録した。
