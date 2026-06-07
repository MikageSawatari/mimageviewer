# 吹き出し・テキスト注釈ツール 設計・機能リサーチ

Status: **Claude 案 (実コード接続)** / Draft / 機能リサーチ + 設計案
Date: 2026-06-03

関連ドキュメント:

- [speech-bubble-tool-design.md](speech-bubble-tool-design.md) — **Codex 案** (対になる設計メモ)。本書と独立に書かれ、結論は一致。§1.2 で関係を整理
- [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) — 補正レイヤー・パイプライン処理順・キャッシュ優先順位の前提
- [display-pipeline.md](display-pipeline.md) — 表示テクスチャ優先順位の決定版
- [preset-and-adjustment.md](preset-and-adjustment.md) — 補正/AI キャッシュ無効化ルール
- [virtual-folders.md](virtual-folders.md) — ZIP/PDF ページのキー正規化
- CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」「IME 対応」— テキスト入力 UI で必読

> 本書は `auto-mask-detection-plan.md` (Claude 案) ↔ `ai-suggested-mask-v1.1.0-plan.md`
> (Codex 案) と同じペア構成。本書 (Claude 案) は**実コードの型 / 関数 / 統合点に接続**して
> 設計することに重きを置き、Codex 案は機能要件・日本語組版 (縦中横等)・検証項目を厚く書いている。
> 両者を併読する前提。

---

## 1. 背景と目的

補正レイヤー機能の追加に続き、**漫画の吹き出し + 任意テキスト書き込み**の需要が見込める。

- **漫画 / 同人誌**: コマにセリフを入れる、ナレーション枠、効果音文字。
- **AI イラスト投稿**: キャラに一言セリフを付ける、画像にタイトル / キャプションを焼き込む、
  ミーム的な上下テキスト。SNS 投稿で「吹き出し付き」「セリフ入り」の作品をよく見かける。

これらは **非破壊・後から編集できる注釈オブジェクト**として持ち、表示と書き出しの両方で
ピクセルに焼き込めれば実用になる。mIV はすでに消しゴム / 補正レイヤー / 隠蔽加工 / crop と
いう非破壊編集スタックを持っているため、その流儀にそのまま 1 段足すのが自然。

### 1.1 このドキュメントのスコープ

- 既存パイプラインのどこに、どういう形 (専用ツール vs 補正レイヤーの 1 効果) で入れるかの結論。
- 「吹き出しツールとして通常求められる機能」の網羅的リサーチ (競合調査込み)。
- データモデル / レンダリング / 永続化 / キャッシュ統合の設計案。
- MVP とフェーズ分け、要決定事項。

実装着手の指示ではなく、設計合意のためのたたき台。

### 1.2 Codex 案との関係 (独立検証)

[speech-bubble-tool-design.md](speech-bubble-tool-design.md) (Codex 案) と本書 (Claude 案) は
別々に書かれたが、**主要な設計判断はすべて一致**した。独立した 2 案が同じ結論に至ったこと自体が
方針の確からしさの裏付けになる。

両案が一致した点:

- 補正レイヤーの 1 効果にはせず、**独立した注釈オブジェクトツール**にする。
- 処理順は **隠蔽加工の後・crop の前**。
- ベクタオブジェクトをページ単位で持ち、**専用 DB + `mimageviewer.dat` サイドカー**で永続化。
- 表示は **編集中ライブ egui + 確定後 worker ベイク**の 2 段、WYSIWYG を狙う。
- **サムネイル非反映 (バッジのみ)**、右 Ctrl 元画像プレビューでは注釈を隠す。
- 日本語**縦書き + IME 安全 (Enter/Escape を奪わない)** を初期設計の中核に。
- 単体テキスト (枠なし + 縁取り) を AI イラスト投稿の主用途として MVP に含める。

役割分担 (併読時の使い分け):

| 観点 | 本書 (Claude 案) が厚い | Codex 案が厚い |
| --- | --- | --- |
| 実コード統合点 | `resolve_fs_processed_texture` / `ensure_conceal_texture` / `export_page_pixels_for_idx` / `page_path_key` 等に file 単位で接続 (§7) | 概念レベルの pipeline 図 |
| レンダリング実装 | グリフラスタライザ選定 (cosmic-text + 縦書き自前)、共有レイアウトエンジン、グリフキャッシュ (§5) | egui galley / worker ラスタの方針 |
| キャッシュ無効化 | 既存 generation 方式に合わせた無効化表 (§7.2) | `balloon_overlay_cache` / `composite_cache` のキー |
| 日本語高度組版 | 概要のみ (将来送り) | **縦中横 / 横倒し / 正立の自動ルールと UI を詳述** (必読) |
| 機能網羅・検証 | 機能表 + 競合比較 | オブジェクト整列 / 均等 / グループ、見開き、検証チェックリストが詳しい |

→ **縦中横・縦書きインライン方向の詳細仕様は Codex 案 §4.4.1 を正とする**。本書はそこは
重複させず、実装統合とレンダリング基盤に集中する。データモデルは両案で命名が違うだけで
構造はほぼ同型 (本書 `AnnotationObject` ≒ Codex `BalloonObject`)。実装時にどちらの命名を
採るかは要決定 (§10)。

---

## 2. パイプライン上の位置づけ (結論)

### 2.1 結論: 隠蔽加工の「後」・crop の「前」に、専用ツールとして入れる

ユーザーの直感どおり **モザイク (隠蔽加工) より後・crop より前** が正しい。現行の処理順
([local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) §3) に
吹き出し段を 1 つ挿入する:

```
元画像
  -> AI denoise / upscale
  -> 全体画像補正
  -> ポストフィルタ
  -> 消しゴム / 補完
  -> 補正レイヤー x N
  -> 隠蔽加工 (モザイク)
  -> 吹き出し・テキスト注釈 x N   ← 新規
  -> crop (任意、最後段)
  -> 表示 / 書き出し
```

本体フルスクリーン左パネルのヘッダーアイコン順は:

```
消しゴム -> 補正レイヤー -> 隠蔽加工 -> 吹き出し -> 切り取り -> エクスポート
```

(現行は `消しゴム -> 補正レイヤー -> 隠蔽加工 -> エクスポート`。間に「吹き出し」、その後ろに
既存の「切り取り」パネルを置く。)

### 2.2 なぜ「隠蔽加工の後」か

- 吹き出し・テキストは画像の**最前面に乗せる装飾**で、下の絵をモザイクのように加工する
  ものではない。隠蔽加工の結果の上に重ねたい。
- 隠蔽加工でモザイクをかけた領域の上に吹き出しを置いて隠す、という使い方もあり得る。
  吹き出しが先だとモザイクが吹き出しの上にかかってしまい不自然。

### 2.3 なぜ「crop の前」か

- crop は最後段で「切り出すだけ」(画像サイズと座標系を変える)。吹き出しは**ソース画像座標**に
  アンカーするので、crop はその結果をクリップする。これで「キャラの口元を指すしっぽ」のように
  絵の内容に追従させられる。
- crop を後に置けば、後から構図を詰め直しても吹き出しの相対位置が壊れない
  (補正レイヤー / 隠蔽加工と同じ「ソース座標で持つ」流儀)。
- 軽い留意点: 「投稿の最終フレーム基準で下部にタイトルを置きたい」用途では、crop 後の座標で
  置きたくなる可能性がある。ただし全段がソース座標で統一されている方が破綻が少ないため、
  v1 はソース座標 + crop クリップで統一し、必要なら将来「crop 後オーバーレイ」を別途検討する。

### 2.4 なぜ「補正レイヤーの 1 効果 (LocalEffect)」にしないか

補正レイヤーは「マスク × 下のピクセルへの加工」モデル
([local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) §4)。
吹き出しは性質が根本的に違う:

| 観点 | 補正レイヤー (LocalEffect) | 吹き出し・テキスト |
| --- | --- | --- |
| 何をするか | 既存ピクセルをマスク範囲で変換 | **新しいコンテンツ (図形 + 文字) を追加** |
| 編集対象 | マスク (塗り) + 効果パラメータ | 図形ハンドル + しっぽ + **文字入力 / フォント / 縦横** |
| 主な入力 | ブラシ / グラデ / 範囲 | テキスト入力 (IME 必須) + ベクタハンドル |
| 出力の依存 | マスク alpha | グリフラスタライズ (フォント) |

→ 隠蔽加工 / crop と同じく **独立したモード + 専用パネル + 専用 DB + 専用合成段**にするのが
正しい。ただし図形のハンドル編集 (移動 / 回転 / 拡縮 / 制御点) は既存 `Shape` / vector-edit の
仕組みを再利用する。

### 2.5 サムネイルへの非反映

補正レイヤー / 隠蔽加工と同じく、**サムネイルには吹き出しを反映しない**。必要ならバッジ表示のみ。
(全体補正だけがサムネに乗る。重い装飾段はフルスクリーン / 書き出し時のみ。)

---

## 3. 機能リサーチ (網羅)

漫画制作ソフト (Clip Studio Paint)、汎用デザインツール (Canva)、Web 吹き出しメーカー
([Studio Genesis C.A.](https://studio-genesis-c-a.com/bubble))、CSP 吹き出し解説
([esinote](https://esinote.com/blog/11751.html))、漫画レタリングの慣習を調査した結果。
「通常求められる機能」を**必須 (MVP) / 望ましい / 将来**で優先度付けする。

> **lab (`tools/comic_lab`) 実装状況 (2026-06)**: 楕円 / 角丸・矩形 / トゲトゲ(叫び) /
> 雲(思考) の 4 形状、三角しっぽ + 思考◯しっぽ (どちらも輪郭一体)、塗り / 枠 / 塗り不透明度 /
> 内側余白、フォント選択 + ランタイム追加、横 / 縦書き + 自動縦中横 (数字・`!?`) の ON/OFF、
> 袋文字 (縁取り)、単体テキスト、配置 / 選択 / 移動 (しっぽ追従) / しっぽ先端ドラッグ / 複製 /
> 前面・背面 / 削除、WYSIWYG 焼き込み表示、`.comic.json` サイドカー保存。
> **未実装**: マーカー記法 (§3.3.1)、横倒し描画、リサイズ / 回転ハンドル、Undo/Redo、
> 手続き生成装飾、同色合体、ルビ、テキスト自動フィット。

### 3.1 吹き出し (容器) の種類

コミックの吹き出しは形が意味を持つ (数十年の慣習で確立)。主要タイプ:

| タイプ | 用途 / 意味 | 形状パラメータ | 優先度 |
| --- | --- | --- | --- |
| 楕円 / 角丸 (oval/round) | 通常のセリフ | 横半径 / 縦半径 / 角丸半径 | 必須 |
| 矩形 (rect) | ナレーション / キャプション枠 (しっぽ無しが多い) | 幅 / 高さ / 角丸 | 必須 |
| 丸ツノ付き | ぽってり長円 / 角丸 / 思考に、丸い接続部と小円列を付ける | `RoundBumpTail` | 望ましい |
| 雲形 / もくもく (cloud/thought) | 心の声・思考。しっぽは小さい丸の連なり | ローブ数 / 振幅 | 望ましい |
| 爆発・トゲトゲ (burst/shout) | 叫び・驚き・強調 | トゲ数 / ジャグ強度 | 望ましい |
| 電子・無線 (electric/radio) | 電話・TV・スピーカー越しの声。均一なギザギザ + 稲妻しっぽ | 歯数 | 望ましい |
| ささやき (whisper) | 小声。**破線アウトライン**で表現 (形は楕円のまま) | アウトラインを破線に | 望ましい |
| 氷柱 (icicle) | 冷たい敵意 | 下辺のトゲ | 将来 |
| 多角形 / フリーフォーム (pen) | 非人間・特殊・手描き感。CSP の Balloon Pen 相当 | 任意の点列・閉曲線 | 将来 |
| 装飾付きふわ系 | 少女漫画風モノローグ。星 / 花 / 泡 / ふわふわ粒子を輪郭に沿わせる | 手続き生成装飾レイヤー | 望ましい |

実装メモ:
- 「ささやき = 破線」「電子 = 均一ギザギザ」は **形状ではなくアウトラインスタイル / 形状バリエーション**
  で表現できるので、コア形状は「楕円・矩形・雲・爆発・ギザギザ・多角形」程度に絞れる。
- 吹き出し結合は CSP の定番機能。厳密な Boolean union は複雑なので v1 は見送るが、Studio Genesis C.A.
  のような「連続する同色レイヤーを 1 グループとして合成する」軽量方式は Phase 2 候補に入れる。

吹き出しの共通スタイル:

| プロパティ | 内容 | 優先度 |
| --- | --- | --- |
| 塗り色 + 不透明度 | 内側の色。白が標準。透明 (塗りなし) も可 | 必須 |
| アウトライン色 / 太さ | 枠線。黒が標準 | 必須 |
| アウトライン種別 | 実線 / 破線 (ささやき) / 二重線 / なし。CSP は「ブラシ形状」でエアブラシ・手描き調も選べる (esinote) | 望ましい |
| 背景モード | 線+塗り / 線のみ (塗り透明) / 塗りのみ (枠なし)。CSP の同名設定相当 (esinote) | 望ましい |
| 外側白フチ | 背景から切り離すため、通常枠線より外側に太い白ストロークを置く | 必須 |
| ドロップシャドウ | 背景から浮かせる影 | 望ましい |
| 内側パディング | テキストと枠の余白 | 必須 |
| グラデーション塗り | 凝った演出 | 将来 |
| 手続き生成装飾 | 星 / 花 / 泡 / レース / 粒子を輪郭・内側・外側へ生成 | 望ましい |

### 3.2 しっぽ (tail / しっぽ)

CSP の調査で、しっぽは独立した編集要素として重要:

| プロパティ | 内容 | 優先度 |
| --- | --- | --- |
| 有無 | しっぽを出す / 出さない (ナレーション枠は無し) | 必須 |
| 曲げ方 (bend) | 直線 (straight) / 折れ線 (polyline、角ばる→ロボット・電子) / スプライン (spline、曲がる→不気味・小声) | 必須 (直線) / 望ましい (他) |
| 先端位置 | 吹き出し内からドラッグして話者へ向ける。キャンバス上のハンドルで動かす | 必須 |
| 根元の太さ | 付け根の幅。先端へテーパ (自動で尖る) | 必須 |
| 中間制御点 | polyline / spline の曲げ点 | 望ましい |
| 丸ツノ | 輪郭上の丸 + 先端へ向かう小円列。ぽってり / 思考系で自然 | 望ましい |
| 思考トレイル (◯ の連なり) | 縮む丸の列で「思考」を表す。**lab 実装済** (`TailKind::Thought`、雲形選択で自動 ON) | 望ましい (雲形とセット) |
| 手描き / ブラシ尾 | 同レイヤーに自由曲線で尾を描く (esinote)。mIV では polyline/spline で代替 | 将来 |
| 複数しっぽ | 1 つの吹き出しが 2 人を指す | 将来 |
| スタイル継承 | しっぽは吹き出しの塗り / 枠線を継承 | 必須 |

しっぽ先端も**ソース画像座標**で持つ。吹き出し本体を動かしたとき先端は本体と相対追従させるのを
既定にする。Studio Genesis C.A. の実装も、移動時に本体と先端を同時に動かし、先端ハンドルの
ドラッグ時だけ `tip` を更新する。通常ツノは輪郭上の出口 2 点と先端をつなぐ連続パス、丸ツノは
輪郭上の丸と小円列を `tip` / 本体サイズ / seed から都度生成する。生成済み頂点列を保存しない。

### 3.3 テキスト (日本語縦書きを含む)

漫画・日本語が主用途なので**縦書き対応が肝**。

| プロパティ | 内容 | 優先度 |
| --- | --- | --- |
| 本文 (複数行) | 改行を含むテキスト。IME 必須 | 必須 |
| フォントファミリ | バンドル + システムフォント選択。漫画標準は**アンチック体** (明朝 + ゴシック混植) | 必須 |
| サイズ | px または吹き出し比 | 必須 |
| 色 | 文字色。黒が標準 | 必須 |
| **組方向** | **横書き (yokogaki) / 縦書き (tategaki)**。漫画は縦書きが基本 | 必須 |
| 行揃え | 横: 左 / 中央 / 右、縦: 上 / 中央 / 下。吹き出し内は中央寄せが定番 | 必須 |
| 行間 / 字間 | 行送り・字送り | 必須 |
| 太字 / 斜体 | フォントウェイト or 合成 | 望ましい |
| **縁取り (袋文字)** | 文字の輪郭線 (色 + 太さ)。画像上の可読性に必須 (§3.4) | 必須 |
| ドロップシャドウ | 文字影。可読性向上 | 望ましい |
| グラデーション塗り | 文字色のグラデ | 将来 |
| **ふりがな (ルビ)** | 漢字の読み。縦書きは右、横書きは上。本文の 50% サイズが目安 | 将来 |
| 圏点 / 傍点 (bouten) | 強調の点 | 将来 |
| **縦中横 — 自動 (lab 実装済)** | 縦書き中の 2〜3 桁数字と混在ペア `!?`/`?!` を横一列クラスタに。**同種連続記号 (`!!`,`!!!!!`) は正立スタック** (漫画慣習) なので `!!!!`+`!` のような分割は起きない。ON/OFF トグルあり | **必須** |
| 縦中横 / 横倒し / 正立 — 手動指定 | 任意の範囲を強制クラスタ化 / 横倒し / 正立。**選択 UI ではなくマーカー記法で指示**する方針 (§3.3.1) | 望ましい |
| 横倒し (sideways) の描画 | 英単語などを 90° 回転して縦行に収める。ab_glyph は回転できないので coverage の自前回転が要る | 将来 |
| 曲線テキスト / パス沿い | 文字を曲線に沿わせる | 将来 |

> 自動縦中横の背景仕様は [speech-bubble-tool-design.md](speech-bubble-tool-design.md)
> (Codex 案) §4.4.1 を参照。**lab では簡素化済**: 数字 (2〜3) と混在ペア `!?`/`?!` のみ自動
> クラスタ、同種連続記号は正立スタック。ルビ / 禁則 / 約物詰めは Phase 3。

#### 3.3.1 手動インライン方向は「マーカー記法」で指示する (推奨)

リッチテキストの範囲選択 UI (選択 → 属性付与) は、選択状態管理・IME 連携・range とテキストの
同期が重く、lab には荷が重い。代わりに **本文をマーカー文字で囲んで指示する**方式を採るのが楽。

- 例: `[AI]でつくりました` の `[AI]` を 1 つの**縦中横クラスタ**として扱う (縦書きで `AI` を横並び)。
  逆に `「AIで…」` をそのまま入れれば既定どおり 1 文字ずつ正立スタックになる。
- マーカー文字は**本文に出現しうる**ため衝突する。対策:
  - **テキストごとに「記法を使う」トグル** (既定 OFF)。OFF なら `[` `]` はただの文字。
  - **マーカー対を数種から選べる**: `[ ]` / `【 】` / `《 》` / `〔 〕` / `｛ ｝`。本文に出ない対を選ぶ。
  - 方向ごとに別マーカーを割り当てる案: `[..]`=縦中横、`《..》`=横倒し、`〔..〕`=正立強制。
    まずは最頻の**縦中横 1 種**から実装し、必要に応じ横倒し / 正立を足す。
- データ: `TextBlock.text` は**生のマーカー入り文字列のまま保存**し、レイアウト時にパースして
  run へ展開する (rich-text の range 同期が不要)。`markup_enabled: bool` と選択マーカーだけ別途保存。
- 利点: 実装が軽い・キーボードだけで速い・単一スタイル `TextBlock` のまま。
- 注意: 入力中はマーカー文字が見える (焼き込み結果はパース後)。lab では許容。

将来、本体統合で本格的な選択 UI が必要になっても、この「記法 → 内部 run 展開」経路を
そのまま流用できる。

実装メモ:
- **縦書きは標準的なテキストレイアウトライブラリが直接対応しない**。グリフを縦に積み、列を
  右→左に並べる**独自レイアウト**が要る。約物 (句読点・括弧) の縦書き字形 (vert feature) や
  小書き文字の位置調整は高度なので段階的に。まず「グリフを縦に等間隔で積む」最小実装から。
- フォントは UI フォント ([src/ui_fonts.rs](src/ui_fonts.rs)) とは別に、**焼き込み用のフォントバイト**を
  扱う必要がある (egui の表示フォントと、書き出しラスタライズのフォントを一致させる)。
  CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」のとおり、UI ラベルへ環境依存記号を増やさない
  方針は維持。ユーザー入力テキストのフォント fallback は別系統で扱う。
- 漫画標準フォントの同梱はライセンス要確認。商用可フリーフォント (源暎アンチック等) は配布許諾を
  個別確認。同梱が難しければシステムフォント参照 + 「フォント追加」導線。

### 3.4 袋文字・影・縁取り (画像上の可読性) ⚠ AI イラスト用途で最重要

AI イラスト投稿では「吹き出しなしで画像に直接キャプション / タイトルを焼く」使い方が最多と
見込まれる。にぎやかな絵の上に乗る文字は、**縁取り (袋文字) かドロップシャドウが無いと読めない**。

- 縁取り: 色 + 太さ。黒地に白文字 + 黒縁が定番。
- 多重縁取り: 既存の袋文字 (`outline`) に加えて追加外フチを重ね、白+黒などの二重フチを作る。
- ドロップシャドウ: オフセット / ぼかし / 広がり / 色。
- 外側発光: 背景から文字を浮かせるソフトハロー。ネオン系プリセットにも使う。
- 背景プレート: 字幕・キャプション用の角丸半透明背景。
- Echo: 簡易スプライス / エコー表現として、ずらした文字コピーを背面に重ねる。
- これらは**吹き出しの中のテキストにも、単体テキストにも**共通で効く。`TextBlock` に閉じた
  文字効果として扱い、表示 / 書き出しの bake 経路を共有する。

### 3.5 単体テキスト・キャプション・効果音

| 要素 | 内容 | 優先度 |
| --- | --- | --- |
| 単体テキスト (吹き出しなし) | 画像に直接の文字。AI イラストのタイトル / キャプション / ミーム上下文字 | 必須 |
| ナレーション / キャプション枠 | しっぽ無しの矩形 + テキスト (吹き出しの一形態として実現) | 必須 |
| 効果音 / 擬音 (SFX) | 大きく崩した文字。多くは縁取りのみで塗りなし。本格的な変形は専用機能 | 望ましい (太縁の単体テキストで代替) / 将来 (専用変形) |

「単体テキスト」は吹き出し容器を持たない `TextBlock` として実装すれば、ミーム文字・タイトル・
簡易 SFX を全部カバーできる (太い袋文字 + 大サイズ)。

### 3.6 編集 UX

| 操作 | 内容 | 優先度 |
| --- | --- | --- |
| 配置 | クリック / ドラッグで新規作成 | 必須 |
| 選択 | クリックで選択。複数オブジェクトを z 順で持つ | 必須 |
| 移動 | ドラッグ。Shift で軸固定 | 必須 |
| 拡縮 | ハンドル。CSP 同様「リサイズしても文字サイズは保つ」モードを基本に | 必須 |
| 回転 | ハンドル。Shift で 45° スナップ | 望ましい |
| しっぽドラッグ | 先端ハンドルを話者へ | 必須 |
| 制御点編集 | フリーフォーム / 多角形の頂点編集 | 将来 |
| 並び順 (z-order) | 前面 / 背面へ | 望ましい |
| 複製 | オブジェクト複製 | 望ましい |
| 削除 | Delete | 必須 |
| Undo / Redo | `Ctrl+Z` / `Ctrl+Y` / `Ctrl+Shift+Z` (既存ツールと統一) | 必須 |
| テキスト自動フィット | 文字に合わせて吹き出しを伸縮 / 吹き出しに合わせて文字を縮小 | 望ましい |
| 形状再生成 | びっくり / 思考 / 手続き装飾の seed を変え、雰囲気だけ変える | 望ましい |
| 同色合体 | 連続する同色 / 同スタイル吹き出しを 1 グループとして一体表示 | 望ましい |
| スナップ / 整列ガイド | 中央・端揃え | 将来 |
| ロック / 表示切替 | オブジェクト単位 | 将来 |
| 元画像 / 注釈表示トグル | 隠蔽加工の `Q` / `W` と同じ確認系 | 望ましい |

⚠ テキスト入力 UI では CLAUDE.md「IME 対応」を厳守。Enter / Escape は
`dialog_enter_pressed` / `dialog_escape_pressed` 経由で拾い、IME 変換中の確定 / キャンセルを
奪わないこと。フルスクリーンビューポートで TextEdit を出すなら closure 先頭で
`update_ime_state(ctx)` を呼ぶ。

### 3.7 プリセット / 素材

| 内容 | 優先度 |
| --- | --- |
| 内蔵吹き出しプリセット (通常 / 思考 / 叫び / ささやき / 無線 / ナレーション枠) | 必須 (各タイプの初期値) |
| 丸ツノプリセット (ぽってり / 角丸 / 思考) | 望ましい |
| 手続き生成装飾プリセット (きらふわ / 花ふわ / 丸ふわ) | 望ましい |
| テキストスタイルプリセット (フォント + サイズ + 縁取りの組) | 望ましい |
| カラープリセット | 望ましい |
| ユーザー定義プリセット保存 | 将来 |
| 素材ライブラリ (CSP の Material 相当) | 将来 |

装飾付き吹き出しは「素材画像」ではなく、自由なサイズ・形状に追従する生成パラメータとして
保存する。CLIP STUDIO ASSETS の「ふわ系フキダシツール3種」のように、丸ふわ / 花ふわ /
きらふわ系は漫画用途で需要がある。mIV では個別素材を取り込まず、同カテゴリの見た目を
`DecorationLayer` の組み合わせで作る。

### 3.8 競合比較 (機能の当たり付け)

| 機能 | Clip Studio Paint | Canva | Photoshop | mIV v1 目標 |
| --- | --- | --- | --- | --- |
| 吹き出し形状プリセット | 豊富 (素材) | 多数 | 手動 / シェイプ | 5〜6 種 |
| しっぽ (直線/折れ/曲線) | ◎ | △ | 手動 | 直線必須 + 折れ/曲線 |
| 縦書き | ◎ | △ | ◎ | **必須** |
| 袋文字 / 縁取り | ◎ | ◎ | ◎ | **必須** |
| ふりがな | ◎ | × | △ | 将来 |
| 制御点編集 | ◎ | × | ◎ | 将来 |
| 吹き出し結合 | ◎ | × | × | Phase 2 で軽量合体 |
| 単体テキスト / キャプション | ◎ | ◎ | ◎ | **必須** |

mIV は「ペイントソフトの完全代替」を目指さない。**閲覧 → 軽い仕上げ → 投稿**の動線で
「セリフ / キャプションを焼ける」ところまでを狙い、縦書き + 袋文字 + 基本吹き出し + 単体テキストを
確実に作る。

---

## 4. データモデル案 (Rust)

1 ページ = 注釈オブジェクトの列。z 昇順で描画。すべて**ソース画像座標**で保持。

```rust
/// 1 ページ分。JSON 1 本で永続化 (local_adjust の layers_json と同じ流儀)。
struct AnnotationPage {
    objects: Vec<AnnotationObject>,
}

struct AnnotationObject {
    id: Uuid,
    enabled: bool,
    z: i32,                  // 並び順
    pivot: (f32, f32),       // 位置 (ソース画像座標)
    rotation_rad: f32,
    kind: AnnotationKind,
}

enum AnnotationKind {
    Bubble(BubbleObject),    // 吹き出し (容器 + 埋め込みテキスト)
    Text(TextBlock),         // 容器なし単体テキスト / キャプション / 簡易 SFX
}
```

```rust
struct BubbleObject {
    shape: BubbleShape,
    shape_seed: u64,            // Burst / Cloud / 装飾追従などの決定論的ゆらぎ
    merge_mode: MergeMode,      // 通常 or 直下の同色同スタイル吹き出しと軽量合体
    fill: Option<Rgba>,          // None = 塗りなし(透明)
    fill_opacity: f32,
    outline: StrokeStyle,        // 色 / 太さ / 実線|破線|なし
    outer_white_stroke: Option<StrokeStyle>, // 背景から分離する外側白フチ
    shadow: Option<ShadowStyle>,
    decorations: Vec<DecorationLayer>, // 星/花/泡/レース等の手続き生成装飾
    tails: Vec<Tail>,            // 0..N
    text: TextBlock,
    autosize: AutoSize,          // FitBubbleToText | FitTextToBubble | Fixed
    padding_px: f32,
}

enum MergeMode {
    Normal,
    MergeWithPreviousSameStyle,
}

enum BubbleShape {
    Ellipse  { rx: f32, ry: f32 },
    RoundRect{ half_w: f32, half_h: f32, corner_px: f32 }, // corner=0 で矩形(ナレーション)
    Cloud    { half_w: f32, half_h: f32, lobes: u32 },     // 思考
    Burst    { rx: f32, ry: f32, spikes: u32, jag: f32 },  // 叫び
    Spiky    { rx: f32, ry: f32, teeth: u32 },             // 電子/無線(均一)
    Polygon  { points: Vec<(f32, f32)> },                  // 多角形/フリーフォーム(将来)
}

struct Tail {
    bend: TailBend,              // Straight | Polyline | Spline
    base_t: f32,                 // 吹き出し輪郭上の出口位置 (0..1)
    mid: Vec<(f32, f32)>,        // 折れ/曲線の中間点
    tip: (f32, f32),             // 先端 (ソース画像座標)
    width_px: f32,               // 根元の太さ (先端へテーパ)
    style: TailStyle,            // Solid | ThoughtTrail(縮む丸) | RoundBump(丸ツノ)
}

enum TailStyle {
    Solid,
    ThoughtTrail,
    RoundBump { max_circles: u32 },
}

struct DecorationLayer {
    enabled: bool,
    kind: DecorationKind,
    placement: DecorationPlacement,
    seed: u64,                   // 決定論的 PRNG。保存/再読込/書き出しで配置を固定
    density: f32,                // outline 長 or 面積あたりの個数
    size_min_px: f32,
    size_max_px: f32,
    offset_px: f32,              // 輪郭法線方向のずらし
    color: Rgba,
    stroke: Option<StrokeStyle>,
    glow: Option<GlowStyle>,
    jitter_pos_px: f32,
    jitter_angle_rad: f32,
}

enum DecorationKind {
    Sparkle { points: u32 },     // 4点/8点星、十字光
    Flower { petals: u32, roundness: f32 },
    BubbleDots,
    Hearts,
    MusicNotes,
    Lace,
    Dust,
}

enum DecorationPlacement {
    Outline,     // 輪郭上
    Outside,     // 輪郭外側
    Inside,      // 塗り内側
    Tail,        // しっぽ周辺
    TextSafe,    // テキスト bbox を避けて内側
}

struct TextBlock {
    text: String,                // MVP は単一スタイル。将来は run 分割
    font: FontRef,               // バンドル名 or システムフォント名
    size_px: f32,
    color: Rgba,
    orientation: Orientation,    // Horizontal | Vertical(縦書き)
    align: TextAlign,            // 行揃え
    line_gap: f32,
    letter_gap: f32,
    outline: Option<StrokeStyle>,// 袋文字 (画像上可読性)
    shadow: Option<ShadowStyle>,
    bold: bool,
    italic: bool,
    // 将来: ruby: Vec<RubySpan>, emphasis_dots, tate_chu_yoko
}
```

設計メモ:
- ナレーション枠 = `RoundRect{corner=0}` + `tails: []`。専用 enum は作らない。
- ささやき = `Ellipse` + `outline.dash = true`。電子 = `Spiky`。形状とスタイルで表現を組み合わせ、
  enum 種別を最小に保つ。
- 通常ツノは `BubbleShape` の輪郭サンプラから付け根 2 点を求め、先端へ向かう連続パスとして生成する。
  丸ツノは輪郭上の丸 + 先端方向の小円列を `TailStyle::RoundBump` として生成し、実体頂点は保存しない。
- `shape_seed` は `Cloud` / `Burst` などの輪郭ゆらぎと、ユーザーの「形状再生成」操作に使う。
  `DecorationLayer::seed` と同じく、保存 / 再読込 / 書き出しで見た目が揺れないための正本。
- 図形のハンドル編集 (移動 / 回転 / 拡縮) は既存の vector-edit ([src/mask_db.rs](src/mask_db.rs) の
  `Shape` 編集系) と発想を揃える。
- 装飾レイヤーは `BubbleShape` の輪郭サンプラから得た点 / 接線 / 法線 / 曲率を使って配置する。
  shape 変更やリサイズ時にも同じ seed で再生成し、見た目の雰囲気だけが自然に追従する。
  生成済み頂点を JSON に保存せず、パラメータだけを正本にする。

---

## 5. レンダリング設計 (WYSIWYG)

吹き出しは表示と書き出しの両方でピクセル化が要る。**表示は応答性、書き出しは正確性**を両立する。

### 5.1 レイアウトとラスタライズの分離

**共有レイアウトエンジン**がグリフ位置 (縦/横、行揃え、字間、袋文字オフセット) を計算する。

- **編集中の表示**: egui Painter で図形 (塗り / 枠 / しっぽ) + グリフを画面座標にライブ描画。
  ドラッグ追従が軽い。グリフ描画は egui に出すが、**位置は共有エンジンが決める**ので縦書きも崩れない。
- **確定後の表示キャッシュ + 書き出し**: 共有エンジンの位置から**CPU ラスタライザ**で
  ソース解像度の RGBA オーバーレイ (alpha 付き) をベイク。表示も書き出しも同じバッファを使い
  WYSIWYG を保証。

egui とラスタライザで個別グリフの字形は同じ TTF を使うので一致。レイアウトは自前エンジンで
統一するため、両者の見た目差は最小化できる。

### 5.2 グリフラスタライザの候補

| 候補 | 長所 | 短所 |
| --- | --- | --- |
| `cosmic-text` | シェイピング / フォント fallback / 行分割が強い。CJK 良好。MIT/Apache | 縦書きは非対応 → 列レイアウトは自前 |
| `fontdue` | 軽量・高品質ラスタライズ | レイアウト / fallback は最小限 |
| `ab_glyph` + `rustybuzz` | 細かく制御可 | 自前実装が増える |

推奨: **`cosmic-text` をフォント読込 / シェイピング / fallback / 横書きレイアウトに使い、
縦書きはグリフ単位の列レイアウトを自前で組む**。袋文字はグリフ輪郭を太らせる (stroke) か、
8 方向オフセット合成で近似。

### 5.3 ライブ vs ベイクのタイミング

[local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) §9 の方針に従う:

- ドラッグ / テキスト入力中はライブ egui 描画のみ。重いベイクは走らせない。
- 操作が止まって短い idle 後にベイク (worker)。世代管理で古い結果は破棄。
- 全画面 RGBA オーバーレイは 20MP で ~80MB と重い。**変更オブジェクトの bbox 範囲だけ**を
  ベイクして部分更新する。タイル分割は隠蔽加工 / 補正レイヤーの手法を流用。

---

## 6. 永続化

既存ツール (conceal_db / local_adjust_db / export_crop_db) と同じ二重化:
**中央 DB を authoritative、フォルダ単位サイドカー `mimageviewer.dat` をバックアップ。**

- 中央 DB: `%APPDATA%/mimageviewer/annotation.db`
  ```sql
  CREATE TABLE annotation_pages (
      page_path  TEXT PRIMARY KEY,  -- adjustment_db::normalize_path と同じ正規化
      objects_json TEXT NOT NULL,
      updated_at INTEGER NOT NULL
  );
  ```
- サイドカー: `mimageviewer.dat` の各エントリに `annotations` を追加 (conceal/local_adjust と同期)。
- キー規則: `App::page_path_key` / `sidecar_relative_key` をそのまま使用 (通常画像 / ZIP 内画像 /
  PDF ページ。[virtual-folders.md](virtual-folders.md) のキー正規化に準拠)。
- バッジ用 `annotation_pages: HashSet<usize>` を App に持つ (conceal_pages と同じ)。
- 焼き込み済みオーバーレイのキャッシュ BLOB は持たず、`objects_json` から都度ベイクする
  (フォントは環境依存なので生のオブジェクトを正本にする)。

#### 6.1 App 状態 — 「保存済みページの実体」と「編集中バッファ」を分ける ⚠ (Codex P1)

`ensure_annotation_texture` や capture 経路 (§7.3) は、**編集モードでないページ**の注釈も
描く / 焼く必要がある。編集中ページ 1 枚だけを持つ設計では描画データが足りず、描画のたびに
DB を引くと **UI スレッドで SQLite を叩く**退行になる。`local_adjust_page_layers`
([src/app.rs:3228](src/app.rs)、フォルダロード時に hydrate、非編集の `current_local_adjust_pixels` /
capture から参照) と同じく、**フォルダロード時に全ページ分を HashMap へ hydrate** する。

```rust
// 保存済み注釈の実体 (authoritative なメモリ表現)。フォルダロード時に DB → hydrate。
annotation_page_objects: HashMap<usize, Vec<AnnotationObject>>,  // ← 追加 (local_adjust_page_layers の写し)
annotation_pages: HashSet<usize>,            // バッジ (= annotation_page_objects の非空キー)
annotation_db: Option<Arc<AnnotationDb>>,

// 編集モード中だけ使うバッファ。確定時に annotation_page_objects[idx] へ書き戻す。
annotation_mode: bool,
annotation_edit_idx: Option<usize>,
annotation_objects: Vec<AnnotationObject>,   // 編集中ページの作業コピー
annotation_selected: Option<Uuid>,
annotation_undo_stack: VecDeque<AnnotationSnapshot>,
annotation_generation: u64,                  // キャッシュ無効化
```

- `ensure_annotation_texture(idx)` / capture は **`annotation_page_objects.get(&idx)`** を読む
  (編集中ページのみ作業バッファを優先)。DB は描画経路で引かない。
- フォルダ移動時は `annotation_page_objects.clear()` → 新フォルダ分を hydrate
  ([src/app.rs:8622](src/app.rs) の local_adjust clear/hydrate に並べる)。
- 入口 `enter_annotation_mode(fs_idx)` (map → 作業バッファへ複製) / 出口 `reset_annotation_mode()`
  (作業バッファ → map + DB + サイドカー保存) を `ui_conceal.rs` の
  `enter_conceal_mode` / `reset_conceal_mode` に倣って実装。

---

## 7. キャッシュ・合成統合

### 7.1 表示優先順位

吹き出しは隠蔽加工の**上**に乗るので、表示チェーンの最上位 (original プレビュー等の確認系を除く) に
入れる。現行 ([local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) §8) に
1 段足す:

```
annotation_cache
  > conceal_cache
  > local_adjust_cache
  > erase_result_cache
  > adjustment_cache
  > ai_upscale_cache
  > fs_cache
```

吹き出し合成の入力 (= 下地) は隠蔽加工の結果:

```
annotation source:
  conceal_cache > local_adjust_cache > erase_result_cache > adjustment_cache > ai_upscale_cache > fs_cache
```

統合点: `resolve_fs_processed_texture` ([src/ui_fullscreen.rs](src/ui_fullscreen.rs)) で
`ensure_conceal_texture()` の戻り (隠蔽加工結果) の**直後**に `ensure_annotation_texture(ctx, idx)`
を挟む。`ensure_annotation_texture` は遅延生成 + 世代検証で、未生成 / stale / worker 実行中は
注釈抜きの下地 (隠蔽加工結果) を即時返す。

キャッシュキー案 (local_adjust の `LocalAdjustResultKey` に倣う):

```rust
struct AnnotationResultKey { idx, input_gen, conceal_gen, annotation_gen }
```

### 7.2 無効化ルール

| 変更 | annotation_cache | conceal_cache |
| --- | --- | --- |
| 全体補正 / AI / 消しゴム / 補正レイヤー変更 | クリア | (既存どおり) |
| 隠蔽加工変更 | クリア (下地が変わる) | クリア |
| 注釈オブジェクト変更 | クリア | 残す (注釈は下流) |
| 最後段 crop 変更 | 残す (表示/書き出し時に切り出すだけ) | 残す |
| 回転変更 | 残す or 要検討 | 残す |

吹き出しは隠蔽加工より後段なので、隠蔽加工 (および上流) の変更は吹き出しの下地を変える →
`annotation_cache` を stale にする。注釈の変更は上流に影響しない。

### 7.3 書き出し・保存経路の統合 — 1 箇所では足りない ⚠ (Codex P1)

注釈をピクセルに焼く保存経路は **Ctrl+E 書き出しだけではない**。実コードには別ワーカー構造体で
動く並行経路が複数あり、いずれも現状 **conceal → crop までしか合成しない**:

| 経路 | 入口 | ワーカー / 合成 | 現状の合成順 |
| --- | --- | --- | --- |
| Ctrl+E エクスポート | `export_page_pixels_for_idx` ([src/ui_fullscreen.rs](src/ui_fullscreen.rs)) | `ExportPagePixels` | base → conceal → crop |
| キャプチャ保存 / クリップボード / 比較ピン | `prepare_capture_pixel_job` → `capture_job_with_conceal` ([src/ui_fullscreen.rs:8728](src/ui_fullscreen.rs)) | `CapturePixelJob` → `run_pixel_job` ([src/capture.rs:326](src/capture.rs)) | adjust → conceal → crop |

`ExportPagePixels` と `CapturePixelJob` は**別構造体・別ワーカー関数**なので、注釈段を片方にだけ
足すと「表示には吹き出しが出るのにキャプチャ保存だと消える」不一致になる。

**対策: conceal → annotation → crop の末尾合成を 1 つの共有関数に集約し、両経路から呼ぶ。**

- 両ジョブに `annotation_overlay: Option<RgbaOverlay>` (alpha 付き・ソース解像度) を追加。
  オーバーレイは §5 の共有ベイク経路で生成し、表示キャッシュと**同一バッファ**を使って WYSIWYG を保証。
- `capture_job_with_conceal` ([src/ui_fullscreen.rs:8728](src/ui_fullscreen.rs)) に注釈付与を足す
  (ここが capture 系の合流点。`conceal_composite_mask_for_export` の隣に
  `annotation_overlay_for_export(idx)` を並べる)。
- `run_pixel_job` ([src/capture.rs:348](src/capture.rs)) の conceal 適用と crop 適用の**間**に
  annotation overlay の alpha 合成を挿入。Ctrl+E ワーカーにも同じ tail を通す。
- 注釈オーバーレイ生成は §6.1 の `annotation_page_objects.get(&idx)` を入力にする
  (capture は非編集ページでも走るため)。

`prepare_capture_pixel_job` は補正レイヤー / 消しゴム補完が未反映なら Err で弾く設計
([src/ui_fullscreen.rs:8669](src/ui_fullscreen.rs)) になっている。注釈は worker ベイクが
未完了でも「注釈抜き下地」で保存されると不一致になるので、**注釈オーバーレイが stale / 生成中の
ときの保存可否**を要決定 (§10): (a) ベイク完了まで待つ / (b) 同期ベイクしてから保存 / (c) Err で弾く。

### 7.4 ライブ編集表示は別パスで描く ⚠ (Codex P2)

§7.1 の `ensure_annotation_texture` は**焼き込み済みテクスチャ**を返すだけ
(`resolve_fs_processed_texture` の戻りは `Option<TextureHandle>`、[src/ui_fullscreen.rs:794](src/ui_fullscreen.rs))。
ここには **選択枠・リサイズ / 回転ハンドル・しっぽハンドル・IME caret・ライブ TextEdit** は載らない。

これらは画像 rect / zoom / pan / 見開きページ rect が確定した**後**、`draw_fs_image` 後段に
**スクリーン座標の専用パス `draw_annotation_overlay(...)`** を 1 本足して描く。conceal の
プレビューオーバーレイ描画と同じ層。整理すると 2 系統:

- **焼き込み (WYSIWYG の正)**: `ensure_annotation_texture` → 表示テクスチャ + 保存 (§7.3)。確定後 idle で worker ベイク。
- **ライブ装飾 (編集中の応答性)**: `draw_annotation_overlay` でベクタのハンドル / caret / preedit を即時描画。

編集中はライブ装飾を主に見せ、操作が止まったら焼き込み結果へ追いつかせる (§5.3 の throttle 方針)。

### 7.5 同色レイヤー合体と白フチ

Studio Genesis C.A. の「1つ下の同色レイヤーと合体」は、厳密なベクター Boolean union ではなく、
連続する同色 / 同不透明度の吹き出しを描画グループにまとめ、個別オブジェクトのまま一体表示する
方式として実装されている。mIV でも Phase 2 ではこの軽量方式を採る。

- `MergeWithPreviousSameStyle` の吹き出しは、直下の同色 / 同 outline / 同 opacity / 同白フチ設定の
  `BubbleObject` と同一グループに入れる。条件を満たさない場合は通常描画に戻す。
- グループ内の各吹き出しは個別選択 / 移動 / resize / text edit 可能なままにする。レイヤー一覧では
  `合体` バッジを表示し、ユーザーが見た目の状態を把握できるようにする。
- 描画は group bbox の scratch buffer で、外側白フチ → 通常枠線 → 塗り → 内側抜き / 後続グループの
  重なり抜き、の順に行う。厳密な union は Phase 3 以降の候補。
- 外側白フチは `outline` の色変更ではなく `outer_white_stroke` として独立させる。通常枠線より外側に
  太く置くため、背景との分離と漫画らしい見え方が安定する。

### 7.6 透明背景素材書き出し

Studio Genesis C.A. は背景画像なしで保存すると透明背景の吹き出し素材として使える。mIV は既定の
Ctrl+E / capture 系では「ページ画像 + 注釈」を WYSIWYG 保存するが、後続機能として
**選択中注釈だけを bbox で透明 PNG 書き出し**できると、吹き出し素材作成にも使える。

- UI は通常保存と混同しないよう、「選択注釈を透明 PNG で保存」のような明示コマンドにする。
- 出力範囲は選択オブジェクト群の bbox + padding。画像本文 / conceal / crop は含めない。
- フォント、白フチ、手続き装飾、合体グループは通常ベイクと同じコードを通し、見た目差を作らない。

---

## 8. パフォーマンス方針

[local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) §9 と同じ精神:

- ベクタなので**表示はライブ egui 描画で軽い**。重いのはラスタライズ (袋文字 / 影 / 縦書き) のみ。
- ベイクは worker 化 + 世代管理 + 変更 bbox / タイル限定更新。
- 手続き生成装飾は object bbox 内で生成し、全画面を走査しない。輪郭長 / 面積から個数を決め、
  上限個数を設ける。
- ドラッグ / テキスト入力中はベイクを debounce。反映が数百 ms 遅れるのは許容、入力は止めない。
- フォントのグリフビットマップはキャッシュ (同一文字・サイズの再ラスタライズを避ける)。
- サムネイルには反映しない (重い装飾を背負わない)。

---

## 9. MVP とフェーズ分け

### 9.0 開発戦略: `comic-core` + `comic_lab` を先に作る (推奨)

補正レイヤーが `crates/local-adjust-core` (pure crate) + `tools/local_adjust_lab` (egui 試作) で
先行検証 → 本体統合、という順で進んだのと**同じ二段構え**を採る。本機能はその方式が
local_adjust 以上に効く (理由は下記)。

```
crates/comic-core   (pure / serde + cosmic-text。egui 非依存・単体テスト可)
  ├─ データモデル (AnnotationObject / BubbleShape / Tail / TextBlock)
  ├─ 図形テッセレーション (吹き出し輪郭 → ポリゴン、しっぽジオメトリ)
  ├─ テキストレイアウトエンジン (横/縦書き・縦中横・行揃え・袋文字オフセット)
  │     → layout(text, style) -> Vec<GlyphPlacement>
  └─ ラスタライズ (オブジェクト列 → ソース解像度 RGBA オーバーレイ)
        → bake(objects, w, h) -> RgbaOverlay

tools/comic_lab     (eframe/egui + image + rfd + comic-core。試作専用 GUI)
  ├─ フラット画像を 1 枚読んで、その上に吹き出し/テキストを配置・編集
  ├─ ライブ egui 描画 (layout の glyph 位置を使う) + 焼き込み (bake) の両方を確認
  ├─ perf ログ + .miv 風サイドカーで試作データ保存 (本体 DB は使わない)
  └─ ⚠ conceal / crop / cache / fullscreen-nav は意図的に持たない (= 統合の複雑さを排除)

本体統合 (後段)  App 状態 hydrate (§6.1) / annotation_db / cache (§7) /
  ensure_annotation_texture + draw_annotation_overlay (§7.4) / 保存経路集約 (§7.3) / IME。
  すべて comic-core を再利用する。
```

この機能で lab + core が特に効く理由:

- **最難所が自己完結の純ロジック**: 縦書き / 縦中横 / 袋文字 / ライブ↔焼き込みの WYSIWYG、
  吹き出し・しっぽのジオメトリ — どれも egui や本体パイプラインから切り離して単体テストできる。
  「`びっくり!!!!` で `!!!!` が縦中横クラスタになる」「縦列が右→左に並ぶ」等をユニットテスト化。
- **新規依存 (cosmic-text) の検証**: CJK fallback・縦書き実現可否・ビルド統合を、本体を揺らさず
  lab で確かめてから入れられる。
- **本体パイプラインを壊さない**: conceal / local_adjust / crop / cache の相互作用は壊れやすい
  (CLAUDE.md 冒頭が繰り返し警告)。ツール UX の試行錯誤を lab に隔離し、固まってから統合する。
- **実装役の分担に綺麗に乗る**: `comic-core` (自己完結・テスト可 = スムーズに書ける領域) と
  本体統合 (ラボ移植的領域) を別ワークストリームにでき、相手モデルはユニットテスト基準で
  レビューできる (前段の議論参照)。

UI のドリフト対策: レイアウト / ベイク / ヒットテストを可能な限り `comic-core` に寄せ、lab と
本体の egui コードを薄く保つ。両者は周辺 (nav / cache / 永続化) だけが違い、ツールの中核挙動は
共有する。crate / lab 名は §10 の命名決定に従う (`comic` か `annotation` か)。

### Phase 1: 基盤 + 単体テキスト (最小で価値が出る)

- `annotation_db.rs` + サイドカー `annotations` + App 状態 + バッジ。
  **`annotation_page_objects` HashMap をフォルダロード時に hydrate** (§6.1、UI スレッド SQLite 回避)。
- `enter/reset_annotation_mode`、左パネル (conceal パターン)。
- **単体テキスト**: 本文 / フォント / サイズ / 色 / **横書き + 縦書き** / 行揃え / 行間 / **袋文字** /
  **短い半角列の自動縦中横** (`!!` / `!?` / 2〜3 桁数字。詳細仕様は Codex 案 §4.4.1)。
- **文字効果**: 追加外フチ / ドロップシャドウ / 外側発光 / 背景プレート / 簡易 Echo。
- 配置 / 選択 / 移動 / 拡縮 / 削除 / Undo・Redo。
- 共有レイアウトエンジン + cosmic-text ラスタライズ + ライブ egui プレビュー。
- 表示統合: 焼き込み `ensure_annotation_texture` **と** ライブ装飾 `draw_annotation_overlay`
  の 2 パス (§7.4)。
- **保存統合は Ctrl+E と capture 系の両方** (`export_page_pixels_for_idx` と
  `capture_job_with_conceal`/`run_pixel_job`)。conceal → annotation → crop の tail を共有関数化 (§7.3)。
- → これだけで「AI イラストにタイトル / キャプションを焼く」需要をカバー。

### Phase 2: 吹き出し容器 + しっぽ

- 吹き出し形状 (楕円 / 角丸 / 矩形 / 雲 / 爆発 / ギザギザ)、塗り / 枠 (実線・破線) / 影 / パディング。
- 手続き生成装飾 (星 / 花 / 泡 / レース / 粒子) と、きらふわ / 花ふわ / 丸ふわプリセット。
- 埋め込みテキスト + テキスト自動フィット (吹き出し ↔ 文字)。
- しっぽ (直線必須、折れ / 曲線は順次)、先端ドラッグ、テーパ、思考トレイル、丸ツノ。
- `shape_seed` によるびっくり / 思考 / 装飾形状の再生成。
- 同色 / 同スタイルの軽量グループ合体 (`MergeWithPreviousSameStyle`) と外側白フチ。
- 内蔵プリセット (通常 / 思考 / 叫び / ささやき / 無線 / ナレーション)。

### Phase 3: 仕上げ・日本語高度組版

- グラデーション文字 / 本格的なスプライス・3D / ワープ・パス沿いなどの高度な文字装飾。
- z-order / 複製 / 回転スナップ / 整列ガイド。
- 制御点編集 (フリーフォーム吹き出し)。
- 選択中注釈だけの透明背景 PNG 書き出し。
- **ふりがな (ルビ) / 圏点**、約物の縦書き字形 / 禁則処理 / 句読点ぶら下げ / 約物詰め、
  選択範囲の縦中横 / 横倒し / 正立の手動指定 (自動縦中横は Phase 1 で導入済み)。
- フォント同梱 (ライセンス確認) または「フォント追加」導線。

---

## 10. 要決定事項 (オープン質問)

1. **対象は v1.1.0 に同梱か、別バージョンか。** 補正レイヤーと同時だと範囲が大きい。Phase 1
   (単体テキスト) だけ先行する案もある。
2. **フォント同梱**: 漫画標準フォント (アンチック体系) を同梱するか、システムフォント参照に
   留めるか。同梱は配布許諾の確認が必要。
3. **縦書きの初期スコープ**: 「グリフを縦に積む最小実装 + 短い半角列の自動縦中横」を Phase 1、
   約物字形 / 選択範囲の手動縦中横 / ルビ / 禁則は Phase 3 — の配分でよいか
   (縦中横の詳細仕様は Codex 案 §4.4.1)。
4. **しっぽ先端の固定モード**: 既定は本体移動に相対追従。追加で「先端をキャンバス上に固定」
   モードも必要か。
5. **オーバーレイのメモリ戦略**: 全画面 RGBA ベイク vs bbox/タイル限定。大画像での上限。
6. **「単体テキスト」で簡易 SFX を兼ねるか**、専用の SFX 変形を将来別に作るか。
7. **crop 後オーバーレイの需要**: 投稿フレーム基準のテキストを置きたい声があるか
   (v1 はソース座標 + crop クリップで統一する想定)。
8. **手続き生成装飾の初期範囲**: きらふわ / 花ふわ / 丸ふわまで Phase 2 に含めるか、
   まず星きらめきだけで開始するか。密度上限と seed の UI も決める。
9. **合体の精度**: Phase 2 は Studio Genesis C.A. 型の軽量グループ合体でよいか、後続で
   Boolean union / 制御点統合まで必要か。
10. **透明背景素材書き出し**: 通常保存とは別に、選択中注釈だけを透明 PNG として出すコマンドを
   Phase 3 に入れるか。
11. **命名の統一**: Codex 案 (`BalloonObject` / `balloon.db`) と本書 (`AnnotationObject` /
   `annotation.db`) で命名が違う。構造はほぼ同型なので、実装着手時にどちらかへ寄せる。
   「吹き出しなし単体テキスト」も扱う以上、`annotation`/`注釈` 系の方が広く、`balloon`/`吹き出し`
   は容器限定の語感 — ただし UI 表示名は「吹き出し」が分かりやすい。内部名と UI 名を分ける案も。
12. **手動インライン方向はマーカー記法でよいか** (§3.3.1): 選択 UI ではなく `[..]` 等で囲んで
   縦中横 / 横倒し / 正立を指示。マーカー対は数種から選択 + 「記法を使う」トグル。まず縦中横
   1 種だけ実装で十分か、横倒し / 正立も同時に要るか。lab に今すぐ入れるかも要決定。
13. **Codex のパイプライン再構成との整合** ([local-adjust-pipeline-refactor-plan.md](local-adjust-pipeline-refactor-plan.md)):
   色調補正 / AI / post_filter が最終段へ移り、edit 系 (erase/conceal/local_adjust/crop) は
   source 解像度で動く 2 段キャッシュ (`edit_result` / `final_composite`) へ再構成される。
   吹き出し注釈は **AI / 色補正の影響を受けない最前面オーバーレイ**として最終段 (色補正・AI 後)
   に合成する位置づけになる見込み。本書 §2 の処理順と §7 のキャッシュ (旧 5 段前提) は、この
   新パイプライン確定後に合わせて見直す。

---

## 11. 参考資料

機能リサーチで参照した主な情報源:

- Clip Studio Paint — Balloons (User Guide): https://help.clip-studio.com/en-us/manual_en/540_comic/Balloons.htm
- CLIP STUDIO ASSETS — ふわ系フキダシツール3種 (丸ふわ / 花ふわ / きらふわ、画像素材ではなくツール): https://assets.clip-studio.com/ja-jp/detail?id=1725549
- Studio Genesis C.A. — 吹き出しメーカー (ツノ直接ドラッグ、丸ツノ、同色レイヤー合体、白フチ、
  形状再生成、透明背景保存、独自フォント): https://studio-genesis-c-a.com/bubble
- esinote — CSP 吹き出し講座 (フリーハンドペン、ブラシ形状アウトライン=破線/エアブラシ/手描き、
  背景モード=線+塗り/線のみ/塗りのみ、折れ線尾、思考の丸尾、「追加方法」で同色合体): https://esinote.com/blog/11751.html
- Clip Studio Paint — Balloon Tool [PRO/EX]: https://www.clip-studio.com/site/gd_en/csp/userguide/csp_userguide/510_tool/510_tool_fukidasshi.htm
- Graphixly — Speech Balloons (tail bend types: straight/polyline/spline 等): https://graphixly.com/blogs/news/speech-balloons
- Speech balloon types (thought / burst / whisper / electronic / caption): https://en.wikipedia.org/wiki/Speech_balloon
- Meaning of Speech Bubbles in Comics: https://ilkaperea.com/2019/08/15/meaning-of-speech-bubbles-in-comics/
- Furigana (縦書きは右・横書きは上、本文 50% サイズ): https://en.wikipedia.org/wiki/Furigana
- 漫画フォント (アンチック体) — Canva: https://www.canva.com/ja_jp/learn/manga-fonts/
- CLIP STUDIO PAINT 講座 フキダシ＋セリフ編: https://ichi-up.net/2016/029
- 袋文字 (outline text) — Canva マンガ文字: https://www.canva.com/ja_jp/features/manga-moji/
- Photoshop outline text (縁取りのサイズ / 位置 / 塗り): https://www.adobe.com/products/photoshop/outline-text.html
