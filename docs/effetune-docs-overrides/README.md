# EffeTune 説明書の再生成

日本語スナップショットだけを使用する、ネットワーク不要の変換器。
生成 HTML は編集せず、`scripts/gen-effetune-docs.py` またはこのフォルダーの
訂正ルールを変更する。

2026-10-10 の v0.13.0 実装レビュー訂正:

- `resonator.json`: Adaptive Prediction は同梱 VST の native 処理を使うため、
  独立版の WebAssembly 音声設定を有効にする条件を除去する。
- `analyzer.json`: Rhythm Analyzer の曲変更による自動再解析は独立版プレーヤーの
  説明と区別し、同梱版では Reset 操作を案内する。
- `test_bundled_prediction_and_rhythm_exclude_standalone_requirements` は生成結果を検査し、
  新版の更新時にも独立版の操作条件が再流入しないことを確認する。

```powershell
python scripts/gen-effetune-docs.py --version v0.15.0
python scripts/test_gen_effetune_docs.py
python scripts/gen-effetune-docs.py --version v0.15.0 --check
```

`vendor/effetune-mixwright/VERSION` がある場合は、その値が `--version` より優先する。
スナップショットと同梱版の対応は `SOURCE.md` でも検証する。

## 出力とリンク

- `htdocs/mimageviewer/manual/effetune/index.html`: 同梱版の目次への入口。
  meta refresh、canonical、表示リンクを持ち、noindex のためサイトマップには入らない。
- `<version>/<category>.html`: カテゴリの導入・共通操作・プラグイン一覧。
- `<version>/<category>-<heading-id>.html`: 一覧で指定された各プラグインの説明。
- バス機能、Visualizer、README から選別した基本操作は単一ページ。
  版ごとの目次、ライセンス、画像も生成する。

カテゴリの `プラグイン一覧` のリンクと実際の h2 を照合する。一覧以降の h2 が
未掲載、一致しない、重複している場合は失敗する。導入の h2 は一覧までに置く。
前後ナビゲーションと目次・サイドバーの順序は一覧の順序を使う（本文順ではない）。
各見出しの ID はカテゴリ全体で割り当ててから分割する。同名の小見出しに付く
`-1` なども維持する。旧 `plugins/eq.md#15band-geq` や `#15band-geq` は
`eq-15band-geq.html`、小見出しへのリンクは所有する個別ページと元のフラグメントに
変換する。カテゴリ導入へのリンクはカテゴリページに残す。
非掲載 Markdown へのリンクは従来どおりテキストにする。外部リンクは維持し、
`/dsp/...` などは上流サイトを指す。

`--check` は版フォルダー内のファイルに加え入口ページの欠落・古い内容も検出する。
現在版だけを公開する。入口の切り替え後は、以前の版フォルダーを明示的に公開ツリーから除く。
旧版URLは `.htaccess` の生成ブロックで現在版へ301転送する。同名ページが無ければ目次へ送る。
`--check` は空の旧版フォルダーも拒否する。予期しないファイルがある場合、通常生成も
勝手に削除せず失敗する。
生成 HTML には `.gitattributes` の `effetune/**/*.html` の LF 指定が適用される。

## 訂正ルールと分割

各 JSON のファイル名は上流ページの stem に対応する（`eq.json` など）。
`selection.json` はページ採否の完全な台帳。未知のページが増減したら生成を止める。
`sections` があるページは、Markdown の見出し名を完全一致で選別してから変換する。
各節には下位の見出しも含め、同じか上位の見出しの直前で止める。採用名が欠落・重複、
または親子の採用範囲が重複していたら失敗する。`title` が選別後のページタイトルになる。
README の Web アプリ用 HTML は変換対象に入らない。

訂正はカテゴリ全体の HTML に適用してから分割する。このため既存の
`heading_id`、`scope_heading_id`、段落・インラインの完全一致アンカーは移行不要。
各アンカーは一意でなければ失敗する。更新で見出しや本文が変わった場合に、
訂正を黙って省略しない。訂正で削除された小見出しをリンクが参照しても失敗する。

2026-10-01 の分割で追加・変更したルール:

- `control.json`: 原文には一覧がないため、`section` 見出しの直前に明示的な
  一覧を挿入する `insert_before_heading` ルールを追加した。
  見出しが変わればアンカー検証で止まり、未掲載のプラグインが増えても照合で止まる。
- `basics.json`、`spatial.json`: カテゴリ先頭のステレオ制約注記に
  `repeat_on_plugins: true` を追加した。注記はカテゴリにも残し、各個別ページの
  タイトル直後にも配置する。この指定はカテゴリ h1 の `insert_note` に限定する。
  ほかの訂正ルールとスコープは変更していない。

個別ページには共通操作を読めるカテゴリへのリンクも置く。
公開するページ構成や訂正内容が変わったら、テストとサイトマップを再生成・検証する。
サイトマップは今回の段Dでは変更せず、コミット後に設計担当が再生成する。

## README の節採否 (2026-10-03)

`README.html` を「基本操作」として生成し、目次・サイドバーと mIV の EffeTune ページから案内する。
以下の8節は同梱 VST でも使う操作なので採用する。`README.json` の完全一致アンカーで
プラットフォーム固有の説明だけ訂正・除外する。上流スナップショットは変更しない。

| 採用節 | 訂正・除外した説明 |
| --- | --- |
| Visualizerで音を表示する | ヘッダーのボタンを案内。モバイル／デスクトップのメニューと、渡されない曲情報・ジャケットを除外。詳細は既存 Visualizer ページへリンク |
| エフェクトチェーンの作成 | モバイルの追加操作、コントローラーマッピングへの案内を維持。ヘルプは既定のブラウザで開く説明に訂正 |
| プリセットの使用 | Windows の Ctrl+S に限定。ファイルのドラッグ＆ドロップ読み込み、名前付き保存、バックアップ、URL共有を案内。同梱 VST で無効の `.effetune_preset` ファイル書き出しと区別 |
| 保存データのバックアップと復元 | 同梱版のバックアップ機能を説明。Web／デスクトップ／拡張版間の移行を断定する説明を除外 |
| セクション機能の使用方法 | エフェクトのグループ化、バイパス、復帰を維持 |
| ABパイプライン機能の使用 | A/B の切り替えとコピーを維持。Aのみで始まるのは初回に限定し、以後は保存済みA/Bと選択状態を復元すると訂正。非表示のブラインドテストだけ除外 |
| エフェクト選択とキーボードショートカット | EffeTune の窓固有の操作と明記。独立プレーヤーのキー操作を除外 |
| よく使われるエフェクトの組み合わせ | 同梱エフェクトで構成できる4例を維持 |

残りは次の理由で基本操作ページへ取り込まない。

| 除外した節・内容 | 理由 |
| --- | --- |
| 冒頭の紹介・ダウンロード／インストール操作、ブラウザ拡張版、コンセプト、機能 | 他製品・Web版の機能を含む概要。mIV ページが同梱版を紹介 |
| 紹介動画、リンク | 作者・公式サイト・YouTube の案内は mIV ページに掲載済み |
| セットアップガイド（音楽ファイル、ストリーミング、外部機器） | 独立アプリのプレーヤー・入出力デバイス設定。mIV は自身の再生音を処理 |
| アプリケーション設定、Music Libraryで音楽を探す、ギャップレス再生 | 独立アプリの起動設定・OpenHome・ライブラリ・再生機能。同梱版の設定画面も一部非表示 |
| オーディオファイルの処理、エフェクトチェーンの共有 | 原文には Web音声処理と独立デスクトップのファイルメニューによる書き出しの説明が含まれるため、節全体は転載しない。URL共有は同梱 VST でも実装されており、ドラッグ＆ドロップ読み込み・名前付き保存・バックアップとともに「プリセットの使用」で案内。`.effetune_preset` ファイルの書き出しは無効 |
| オーディオのリセット、周波数特性測定と補正 | 同梱版で各入口が非表示。保存済み測定を使う Room EQ の説明は既存ページに残す |
| トラブルシューティングとFAQ、技術情報（ブラウザ互換性、ブラウザサポートの詳細、推奨サンプルレート） | ブラウザ／独立アプリの要件・設定。mIV の利用条件は mIV ページで案内 |
| 利用可能なエフェクト、開発ガイド | エフェクト一覧は生成目次に全111件掲載済み。開発ガイドは利用者向け操作の対象外 |

## 現在の同梱版と訂正 (2026-10-28)

既定 snapshot は `c506bbab8a3dcd75bc5049ebb9b92572b526c557`。
Mixwright v0.15.0 release commit `8c9a0a561d2ee88d2eef6b06bab138bbd639b3c1` の
`external/effetune` に対応する。原文は変更せず、133 HTML + 4画像を生成した。
旧 `v0.14.0/` の公開生成物だけを除き、入口2本と `.htaccess` を更新する。
旧 snapshot・承認資料・state fixture は履歴として保持する。

- 新しい `measurement.md`、`music-player.md`、`openhome.md` は独立版のマイク測定・
  内蔵プレーヤー・ネットワーク操作であり、VSTに入口/APIがないため selection で除外。
  Room EQの既存測定JSON取込は維持する。上流の測定書出しオプションの既定値変更に合わせ、
  `eq.json` の説明を「オン」に訂正した。
- READMEの見出し変更に追従し、新しい「エフェクトの共通操作」も採用する。
  名前付き保存、A/B復元、バックアップ、共有、コントローラー案内の従来の訂正を維持。
- Counterは曲の再生位置ではなく、hostの処理サンプル累積値を表示する。
  seek先時刻や速度には対応せず、先読み・無音flush・EOSも含まれ、曲変更時に必ず0にはならない。
  番号は00、外部入力用クリックresetも使えない。これを `visualizer.json` で訂正。
- Lyricsに加えてVideoもAdd候補にない（`visualizer-editor.js:575`）。取込済みitemの編集は残る。
  mIVは映像を送らず、Videoは設定に応じて代表画像へfallbackすることを案内。
  Lyric Stageは除き、Cinema Stripの音声グラフと新Counter Galleryを維持。
- HDR設定は `vst-bootstrap.js:36` でVSTにも公開されるため採用し、独立版のClean Feed言及を除く。
  実機HDR出力の確認済みを意味しない。Color Adjust / Color Map、Mid / Treble / Beat / Width、
  数字フォントと編集操作は残す。新フォントは同梱の自作MITデータで、外部配信を使わない。
- SFZの原文が書き直されたため、ファイル/フォルダー登録、元ファイル直接read、
  `sfz-references.json`、参照だけの削除、バックアップ範囲を新アンカーへ移した。
  Classic / Eventの説明を残すが、DAWの動的遅延補償をmIVの実機受入結果とは扱わない。
- analyzer末尾のカテゴリ共通例、basics/eq/saturationの一覧後に移った共通スペクトラム説明は、
  `move_section_before_heading` で一覧前へ移す。本文・小見出し・既存IDを保ち、個別pluginへ誤分割しない。
  旧来の「一覧以降の全h2は掲載plugin」という検査は緩めない。
- controller、analyzer、IR、WASM前提などの完全一致アンカーを新原文に追従した。
  生成回帰は版の付替拒否、共通節の移動、Counter/Video/HDR/SFZ、新3ページの非掲載も検査する。

## 同梱版と訂正 (2026-10-18、履歴)

既定 snapshot は `01c96f00f8e93595aefab96de4bc4d210e61c53b`。
Mixwright v0.14.0 release commit `e4147644305f0d373e362fc1485b4cb9889e5b63` の
`external/effetune` submodule に対応する。旧snapshotと上流原文は変更しない。
公開出力は133 HTML + 4画像、エフェクト113件。再生成コマンドは冒頭のとおり。
生成後は `manual/effetune/v0.13.0/` を公開ツリーから除き、入口・mIVマニュアルのリンク・
`.htaccess` の転送先・sitemapを一緒に更新する。サイトマップの日付確定はコミット後に再生成する。

- `README.json`: 初回案内を同梱VSTの設定メニューから開く説明を追加。
  `effetune.html:181` は英語の Take the tour、`js/ui/state-manager.js:126–127` は
  `tour.menu` を適用し、`js/locales/ja.json5:2` の表示名は「画面の案内を見る」。
  独立版のヘルプメニューや内蔵プレーヤーを案内せず、案内中のDAWはmIVを指すと補う。
  READMEの節選別は維持し、新しいMCP節・LANリモート操作を採用しない。
  `vst-bootstrap.js` の electronAPI に remoteV1 はない。
- `visualizer.json`: Lyricsを追加できる説明と埋込歌詞／LRCの自動取得説明を除く。
  `visualizer-editor.js:575` はLyricsを追加メニューから除外するが、既存・取り込んだ
  Lyricsアイテムの編集は残る。mIVは歌詞を送らず、ライブの歌詞表示はできないと明記する。
  同梱presetにLyric Stageはない。Cinema StripにはLyricsアイテムが一つ残るため、
  プリセット全体を除外せず、波形とスペクトログラムを案内する。
  Edge Fadeの処理説明、Analog Meter Custom、新しいレイアウトは採用し、
  Edge FadeのClean Feedへの言及だけ除く。拡大・サイズ変更のMac用修飾キーも訂正する。
- `others.json`: SFZ Libraryのファイル／フォルダ選択、参照先登録、登録だけの削除を案内。
  native-serviceの `selectFolder` / `selectFiles` と `vst-bootstrap.js:255` のoptions転送を照合。
  Web版のサンプルコピー・端末内ライブラリ削除・再取り込みの説明は残さない。
  ARIAバンク、生成波形、Sample Shift、発音表示は同梱コードにあるため維持する。
  保存先の静的確認はClaudeCodeの2026-10-18記録、過去の実行観測はv0.13.0であり、
  v0.14.0で実行した証拠としては扱わない。
- `eq.json`: 独立レビューの P3 対応。新しい「モバイルでは…1バンドだけ表示」の段落を除く。
  `vst-bootstrap.js:399` が `window.electronAPI` を定義し、`layout-mode-manager.js:59` は
  常にデスクトップ表示を選ぶので、同梱版ではモバイルの単一バンド編集に切り替わらない。
- `reverb.json`: IRライブラリの「すべて選択」「全選択解除」を残し、従来どおり
  保存先をmIV同梱版の説明に訂正する。
- Note SpectrogramのInstrument (3)/(9)、Highlight等は同梱コードと照合して採用。
  独立版の表示・省電力・曲変更による自動解析の既存訂正は維持する。
  selectionと各reasonの参照は新版で再確認し、古い版を根拠にした記述を更新した。
- 回帰は旧v0.12.0/v0.13.0 snapshotの新版への付け替え拒否、生成物全137ファイル、
  旧版URLの転送、独立版MCP／LAN／歌詞取得の説明が流入しないことを検査する。

以降は各更新時点の記録。

## 同梱版更新 (2026-10-04)

既定 snapshot は `03bffda3` (Mixwright v0.12.0)。日本語 Markdown の追加・削除は無し。
公開出力は131 HTML + 4画像。エフェクトは111件で、Analog Meter、Rhythm Analyzer、
Tonal Balance EQ の3ページと、設定入口が表示されたコントローラーマッピングの1ページを追加。
旧版の生成フォルダーは公開前のため削除し、入口・基本操作リンクを更新する。

- `analyzer.json`: ステレオ制約、独立版の省電力説明、ファイル処理の説明を訂正。
- `eq.json`: Tonal Balance EQ の独立版の無音時省電力による測定再開説明を除外。
  同梱 VST の `startPowerPolicyController()` はそのポリシーを無効化する（Codex P3 対応）。
  Reset による測定のやり直しは維持する。
- `controller-mapping.json`: EffeTune の窓固有の割り当てと明記。mIVのプレーヤー操作には
  接続されないことを説明し、対応するエフェクト・A/B・プリセット操作を残す。
- `visualizer.json`: 50レイアウト、新しいグラフ、編集・複製・共有リンク・読み込みを維持。
  独立版のClean Feed／設定とMac用キー説明だけ訂正。節全体の削除は行わない。
- 版は出力pathとprovenanceに保持し、利用者ページの転載注記には表示しない。
- サイトマップは本作業で再生成して検査する。コミット後の日付確定はrelease leadが再生成する。

## 同梱版更新 (2026-10-09)

既定 snapshot は `6791afdfc6f4e6c913b9f48c8189a44e29a09bde`。
Mixwright v0.13.0 release commit `230e5ad8ae8f6e866bc923419fbc198ee559c856` の
`external/effetune` submodule と一致する。27原文ファイル（LICENSEを含む）と4画像を
上流の正確なcommitから取得し、旧snapshotは変更しない。
公開出力は133 HTML + 4画像、エフェクト113件。旧v0.12.0の版別HTMLは保持する。

- SFZ Note Player / Adaptive Prediction、Guitarと新Visualizerレイアウト・編集操作を収録。
- `others.json` はnativeの音源フォルダ選択を案内し、Web/Electron固有の保存・取り込み説明を訂正。
  native保存先とRemoveの削除範囲は公開前の実使用検証に残し、ブラウザ方式から推測しない。
- `visualizer.json` / `README.json` はNow Playingの文字・画像、欠落時、Local/Remoteの保持・clear、
  起動時Visualizerを説明する。独立版LANリモート操作は案内に加えない。
- 生成器は空欄への段落置換を段落の削除として扱う。converterの全出力検査で空要素を拒否する。
