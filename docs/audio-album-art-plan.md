# MP3 埋め込みアルバムアートの一覧表示計画 (§1.347)

作成: 2026-10-07 / Line D (`next-audio-art`)。v4.4.0 後の設計のみ。
**未実装・利用者質問は未回答・独立レビュー未実施・実アプリ未起動。**
以下の「提案」は実装担当の推奨であり、利用者の決定済み事項とは区別する。

## 1. 決定済みの範囲と守る契約

[次の版のバックログ §1.347](next-release-backlog.md) の
**「次の版の決定 (利用者 2026-10-07): ライン D。MP3 から。最初から Remote にも対応する。」**
を優先する。古い [音楽設計 D2 / §3.2](music-integration-plan.md) の
「音楽アイコン固定」は、MP3 の一覧サムネイルについて今回の決定で更新する。
FLAC/M4A 等への拡張、外部 cover.jpg の探索、画像の手動指定、タグ書込、
音楽再生画面へのジャケット表示は今回の提案範囲に含めない (Q1)。

- 埋め込み画像がない・使えない MP3 と他の音声は従来の音楽アイコンを表示する。
- Audio の種別・ソート・検索・★・タグ・ブックマーク・再生位置・連続再生は保持する。
  ジャケットがあっても画像ページにはせず、ダブルクリックは音楽再生のままとする。
- UI から MP3 読込、stat、DB open、画像 decode、縮小、WebP encode を呼ばない。
  GPU upload は既存 texture backlog の予算へ入れる。
- 再生 decoder の attached-picture 除外を維持する。Playback の demux / seek / channel、
  native presenter、波形・PCM・VST の寿命とアルバムアート取得は独立させる。
- 別 viewer の要求・取消・GPU cache を共有しない。Remote は本体の生成関数を使い、
  remote-web に ID3 parser や catalog 読込を複製しない。

参照: [表示](display-pipeline.md)、[非同期](async-architecture.md)、
[応答性 §4](ui-responsiveness.md)、[catalog](catalog-design.md)、
[一覧所有](top-level-grid-view.md)、[Remote](web-remote-plan.md)、
[detached 憲法 §2](detached-rework-plan.md)。新しいキー操作は追加しないため、
KeyAction / 固定キーの仕様変更はない。実装で操作を追加する場合は keymap 2 文書を再読する。

## 2. コードで照合した前提 (実行時の観測ではない)

| 前提 | 参照と設計への影響 |
| --- | --- |
| Audio は固定アイコンで、要求が作られない | `src/grid_item.rs` の Audio、`src/app/grid_paint.rs` の Audio 描画、`src/app.rs::make_load_request` の末尾 `_ => None`。画像 decode に MP3 をそのまま渡しても解決しない |
| Pending にすると再描画し続け得る | `src/app.rs::install_new_items_inner` は Audio を Failed に初期化する理由として Pending / prefetch / repaint を明記している。単に初期化を Pending へ変える修正は不可 |
| 再生はジャケットを video に数えない | `src/video/decoder.rs` の `is_real_video_stream` は ATTACHED_PIC を除外し、best(Video) が cover の場合も実映像を再探索する。この述語を変更しない |
| 専用 ID3 依存は現在ない | root `Cargo.toml` に id3 / lofty はなく、ffmpeg-the-third 3、image、turbojpeg、WebP がある。FFmpeg 案なら追加依存不要 |
| 添付 packet の ABI は利用できる | `vendor/ffmpeg/include/libavformat/avformat.h` に AVStream.attached_pic と ATTACHED_PIC がある。`vendor/ffmpeg/VERSION` は n7.1.5-16-g9a4bb2c579 |
| 成功画像は既存 catalog に入るが、空結果の型はない | `src/catalog.rs` の CATALOG_VERSION=2、thumbnails の必須 WebP BLOB / width / height と mtime / file_size。空 BLOB や 0 寸法を no-art の sentinel にすると既存 reader と衝突する |
| 合成一覧にも Audio がある | folder_scan、tag view からの materialize、smart_folder、reading history、rating、collection prepare の Audio 分岐。サブ展開だけは `src/app/subfolder_expansion.rs` の collect 後 retain で Audio を除外している |
| 詳細 hover は既存サムネ状態を使う | `src/ui_main.rs::render_details_thumbnail_tooltip` は Loaded texture を描き、Failed は「表示できません」、その他は「読み込み中...」+ repaint。no-art の終端を追加しないと hover も止まらない |
| Remote の入口は既存 endpoint で足りる | `/api/thumb` → ThumbnailRequest → `src/remote_ipc/thumbnail.rs::ThumbnailEngine`。現在 MP3 は supported image/video でなく拒否される。app.js の createGridTile は Audio に img を付けず thumbnail binding から除外している |
| IPC の基準はコード v66 | `crates/remote-ipc/src/lib.rs::PROTOCOL_VERSION` と版 assertion は 66。web-remote-plan §13.5 の「現行 v65」は取り残された記述であり、v65 を実装基準にしない |
| 無画像応答は既に wire にある | ThumbnailErrorCode::NoThumbnail、HTTP の 422、command-core.mjs の非 retry 判定を再利用できる。現在 HTTP error 名は miv_thumbnail_error にまとめられる |

バックログの主前提に矛盾はない。FFmpeg が同梱版で APIC 種別をどの metadata として
公開するか、Rust wrapper から packet を借用する具体 API、巨大タグでの割当上限は
この設計だけでは実証していない。§3 の fixture / adapter 検証を実装最初のゲートにする。

## 3. 抽出・表紙選択・入力上限

### 3.1 推奨する抽出経路

新しい `src/audio_album_art.rs` (仮称) に、MP3 の埋め込み画像を取得する本体共通関数を置く。
**既存 FFmpeg の libavformat で APIC → attached picture を得る案を推奨**する。
ID3v2.2 PIC / v2.3・v2.4 APIC のパーサを自作せず、unsynchronization、extended header、
description の文字コードを FFmpeg に任せる。JPEG / PNG を初期の必須対応素材とする。
他の形式を既存 image decoder が安全に読める場合の採用範囲は Q1 に含める。

公式 [AVStream の契約](https://ffmpeg.org/doxygen/trunk/structAVStream.html) は
ATTACHED_PIC の packet を demuxer が所有すると定義している。
公式 [ID3 実装](https://ffmpeg.org/doxygen/trunk/id3v2_8c_source.html) は
APIC の picture type を stream の comment、description を title に公開する。
これは upstream の照合資料であり、同梱 n7.1 系への適用は fixture で検証する。

1. worker で MP3 拡張子 (大小文字を区別しない) と実ファイルを確認し、fresh stat を取得する。
2. worker 専用の input を開く。再生中の input、VideoPlayer、codec decoder を借用しない。
   `best(Video)` は使わず ATTACHED_PIC の stream だけを列挙する。
3. front-cover 相当 (`Cover (front)` / ID3 type 3) を優先し、同順位は元の stream 順で安定化する。
   非表紙候補を後段に置き、候補を一枚ずつ検査・decode する (Q2)。
4. attached_pic の data / size を検査する。input の生存中だけ借用し、null / 負の size / 上限超過
   を拒否する。所有境界を越える必要があるときだけ上限内の bytes をコピーする。
   借用 packet を unref / 改変しない。unsafe はこの adapter 内に閉じ込める。
5. JPEG は既存 byte decode の縮小経路、PNG は image の Limits を使い、既存縮小・WebP encode
   に渡す。ファイル MIME、説明文字列だけで decoder を決めず、実データも検査する。
   GIF 等を許可するなら静止画一枚だけで、アニメーション timer は作らない。
6. fresh stat を再取得し、要求時 stamp と一致する場合だけ結果を公開・保存する。

URL 型 APIC (`-->`) を解決したり、説明にあるパス・URL を開いたりしない。
MP3 を書き換えない。EXIF orientation は既存画像 decode と揃えるが、音声パスの回転・補正・
AI・注釈・トリムをジャケットに適用しない。source_dims はジャケットの画素寸法であり、
音声の詳細メタデータ `video_meta.width/height` は引き続き NULL とする。

### 3.2 巨大タグ・壊れた画像で worker を占有しない

推奨初期値 (Q4): ID3 領域合計 32 MiB、候補 16 枚、候補 bytes 16 MiB、
一枚 40 MP / decode 割当 160 MiB、抽出開始から 10 秒。
数値は製品仕様として回答後に確定する。候補ごとに bytes / 寸法の上限を先に検査し、
全候補を RGBA 展開してから選ばない。画像ヘッダの dimensions と checked arithmetic で
overflow / allocation bomb を防ぎ、decode 側にも allocation limit を設定する。

**既存 `input_with_interrupt` を呼ぶだけでは読込・割当の上限が保証できない。**
details probe では open と stream-info を一括取得するが、ジャケットだけのために音声を probe / decode
する必要はない。推奨 adapter は `avformat_open_input` の段階で MP3 の添付 stream を取得し、
`avformat_find_stream_info` / packet 全尺ループを使わない。File + bounded AVIO の read / seek
に取消・期限・累積読込予算を置き、タグ header のサイズも割当前に検査する。
複数 ID3 header / skip / seek を含む malformed fixture で上限の有効性を検証する。
単一 header の事前検査だけを「全入力が有界」の根拠にしない。

これは技術ゲートであり、同梱 FFmpeg の open-only / allocation を有界化できない場合は
実装を進めず設計担当へ戻す。代案は保守された ID3 crate の直接 APIC 読込。
その場合は依存追加・license・tag limits を設計変更として再レビューする。
FFmpeg の packet 再生 / HW decode への fallback、第二の parser を自動併用する案は採用しない。

## 4. Worker・取消・状態の簡素化

### 4.1 所有者と要求

新しい常駐 album-art pool、全 MP3 の先行走査、再生分析との統合は作らない。
既存 `LoadRequest` の `ResolveStrategy` に AudioAlbumArt を追加し、対応 MP3 だけ要求可能にする。
一覧種別ごとのフラグではなく共通 `audio_art_request_for(item)` (仮称) が適格性と key を決める。
通常・合成一覧・復帰・streaming 追加・clone/fork の初期状態も同じ helper へ揃える。

ローカルは既存 heavy I/O queue / GlobalIoSemaphore / ActivityGate に乗せる案とする。
`GridItem::is_heavy_io` の Audio 分岐を MP3 のみに拡張し、全 enqueue・prune・品質更新 caller
で同じ判定を使う。現在は Audio が false なのでコメント・対応テストも更新する。
可視・詳細 hover を優先し、既存 keep / prefetch idle 制御を保持する。
Remote は既存 pipe heavy worker / admission を使う。並列度を無制限に増やさず、
入力・decode の各上限と既存 worker 数から最大 memory を積算して実装前に設計担当へ提示する。
ローカルの I/O worker は `spawn_thumbnail_workers` で一覧ごとに 1〜2 本なので、
取消中の旧一覧や別 context を含む全体上限の根拠にはそれだけを使わない。
本体共通 GlobalIoSemaphore の permit を抽出開始から decode の大きい一時領域の解放まで保持し、
Remote も同じ上限に参加させる。上流が取得済みなら helper で二重取得しない。
待機は既存 `acquire_cancellable` と Condvar を使い、permit 取得後にも取消を検査する。
catalog だけの hit は source 抽出をせず、WebP decode の既存予算に従う。

要求相関は既存 idx / items_gen / input_seq、結果採用は所有 context の世代で行う。
DB/cache の identity は idx や current_folder ではなく元 MP3 path + stamp とする。
worker で open 前、候補間、decode 前後、stat 前後、保存前、送信前に取消を確認する。
阻害不能な OS stat / 一回の decode 自体は途中で強制停止できないので、UI は join しない。
一覧切替・reload・close・drop・keep 外の取消は既存の context token / keep predicate を使う。
同じ bytes を使う別 context の token は取り消さない。取り消された結果は no-art として保存しない。

### 4.2 一つの終端表現

提案する状態は `Pending → Loaded / NoArt / Failed`、Loaded のみ `Evicted → Pending`。
非 MP3 は最初から NoArt、MP3 は要求を作れる場合だけ Pending とする。
NoArt は「表示に使える埋め込み画像がない」の正常終端で、無画像・候補が全部不正・
対応外・入力上限超過を含む。Failed はアクセス失敗 / timeout / runtime 障害の一覧世代内終端。
Failed を永続 negative cache にしない。次回の明示 reload / 一覧採用でのみ試し直す。
取消による現世代の keep 外は Evicted、旧世代の結果は破棄する。
fresh stat 不一致も現世代では Failed として止め、watch / reload から新しい stamp を要求する。
timeout を canceled と扱って Pending を復活させる再試行ループは作らない。

`ThumbnailState::NoArt` (仮称) に状態を置き、App に has_art / attempted / pending bool を足さない。
共通の terminal / has_pixels 判定を poll、progress、prefetch、idle upgrade、eviction、hover、
auto-aspect の各 consumer に使わせる。NoArt / Failed は requested と upload 待ちを終了し、
終端後の repaint・自動再投入をしない。取得完了時だけ所有 UI を wake する。

worker の共通結果契約は現在 `ThumbMsg.image/canceled/finalized` に分かれている。
no_art bool を足す代わりに、排他的 payload を
`Pixels { image, origin, dims, edit_preview... } / NoArt / Failed / Canceled / Finalized`
へ集約する案とする。相関フィールドは共通のまま。Finalized は既存の
「requested を抜くだけ」の第二通知を保持し、upload 待ちの画像状態を戻さない。
この変更は他媒体にも届くため、実装 brief で全 producer / consumer を列挙し、
独立設計レビュー後に一つの挙動不変な契約変更として先行させる。

### 4.3 組み合わせを減らすために検討した案

- **modal 化**: 一覧の裏方処理であり、スクロール・フォルダ移動・再生を止める退行になるので不採用。
- **一つの生成関数**: 採用。PC / 詳細 hover / Remote に別の抽出・選定・キャッシュ規約を持たせない。
- **既存 worker / reload を使う**: 採用。別 retry owner、progress dialog、live-rebuild manager を作らない。
  設定を設けるなら既存の一覧再読込へ揃え、表示中の全 context を専用 rebuild で同期しない。
- **再生中の demux から取る**: 不採用。再生開始前の一覧・Remote に使えず、seek / close と画像寿命が絡む。
- **全キャッシュを削除して新 schema にする**: 不採用。既存 DB は出荷済みで他媒体の行を失う。
- **まれな DB 失敗を retry で救う**: 自動 retry は不採用案。ログ・通知・次回再生成の Q5 を利用者に相談する。

detached 述語・viewport routing・registry は変更しない。既存 bundle のサムネ状態として
NoArt を保持し、fork は結果を継承しても進行中要求を共有しない。
実装でこの境界を越える必要が判明した場合は止め、detached §2 の構造合意と §11 記録を先に行う。

## 5. Catalog・key・更新と出荷済みデータ

### 5.1 成功行の既存形式は十分、negative は追加が必要

成功画像は既存 `thumbnails` の WebP / dimensions / stamp で表現できる。
一方 no-art を空 WebP として同じ行に入れる形式は不可。提案は同じ親 catalog 内に
次の **追加テーブル**を作る (新しいグローバル DB は作らない)。

```sql
CREATE TABLE IF NOT EXISTS audio_art_absence (
    filename TEXT NOT NULL PRIMARY KEY,
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL
);
```

catalog 自体と成功行は v4.4.0 以前から出荷済み。新機能が未出荷だからといって既存 DB を
作り直してよいとは扱わない。既存 CATALOG_VERSION=2 は保持し、追加 schema の確立を
worker の既存 init_schema に置く。必要な schema が作れなければ art cache だけ使用しない。
旧 DB への初回 open / 再 open / 旧 read-only reader との共存で既存行不変をテストする。
read-only handle は追加 table 不在を cache miss と扱い、CREATE / migration を行わない。

成功保存時は同じ key の absence を削除、NoArt 保存時は同じ key の成功行を削除し、
同一 transaction で排他にする。Ready フラグや第三の状態表は追加しない。
cache miss は「未調査」であり、no-art と推測しない。

### 5.2 key と親 catalog

全 caller 共通の builder が `audioart:v1:<normalized full logical MP3 path>` を作る。
path の正規化は既存 path key helper の大小文字・区切り規則を再利用し、ドライブは保持する。
キー文字列を切り分けてパスを復元しない。algorithm version は前面表紙選定・対応形式・
上限を変更したときに進め、旧 negative の意味を持ち越さない。

保存先は常に **元 MP3 の実親に対応する catalog**。通常一覧でも full-path key を使い、
検索・rating・history・smart・collection と Remote で同じ行を参照する。
合成一覧の synthetic current_folder の DB に MP3 行をコピーして保存しない。
worker が有界な親 catalog handle LRU (推奨 8 親、worker 所有) を使って lookup する。
通常一覧では既に開いた同じ親 catalog / matching map を再利用できる。
異なる親の同名 song.mp3 と、親 hash が共有される別 drive の同名 path を key で分離する。
logical / canonical path の使い分けは Remote path_guard と既存 catalog 対応に揃える。

`delete_missing`、`folder_thumb_existing_keys_for`、smart/collection の cache prepare、
ファイル・フォルダ・全件の cache 削除、rename/move の cache 移行を横断して更新する。
source 親の物理一覧に存在する MP3 の art key を掃除の existing 集合に含める。
facet / 検索で隠れた行を物理的な missing としない。absence も同じ物理 inventory で掃除する。
親以外の合成一覧から source 親を prune しない。rename/move 後は新 key で再生成し、
旧行は通常の cache 掃除へ任せる。利用者のタグ・★・collection migration には変更を入れない。

### 5.3 stamp・方針・再生成

lookup は path key / algorithm version / **MP3 自体の mtime 秒 + file size の完全一致**を条件とする。
image bytes の長さや親フォルダ mtime を stamp にしない。stamp 未取得を 0 として保存しない。
worker は読込前と公開前・保存前に fresh stat を照合し、ロック / transaction 内で stat しない。
mtime の大小から新旧を推測しない。遅い旧 worker が上書きしても異なる stamp なら次回 miss になる。
同時編集が同じ秒・同じ size の場合はこの規約だけでは検出できないため、その限界を Q6 に残す。

成功・absence の永続化とも既存 CacheDecision に従う提案。
Off は永続書込なし、Always は確定結果を書込、Auto は元 MP3 size と抽出・decode・縮小時間で
判定する。元の無画像結果でも調査時間を測る。Auto で保存しない場合も NoArt は
一覧世代内に残るため idle loop は生まれない。RAM cache を増やすために専用の全件 map を作らない。
CacheOnly miss は「未調査」のまま source を開かず、absence は捏造しない。
SourceOnly の明示再生成は成功・absence を両方迂回する。

watch の stamp 変更 / reload / cache 削除 / algorithm version 変更で再調査する。
成功画像の品質 upgrade は既存 origin / evaluated_display_px の規則を使い、NoArt は対象外。
Remote が小さい画素を要求しても、大きい既存 catalog row を縮小行で置き換えない。
保存は設定された thumb_px で生成し、HTTP 出力だけ要求寸法へ縮小する。
アートなしは target_px に依存しないため absence key にサイズを含めない。

## 6. Audio セルを表示する全 surface

| 一覧 / producer | 実装で揃える点 |
| --- | --- |
| 通常物理フォルダ / filter・facet / reload | folder_scan → install_new_items_inner → make_load_request。MP3 のみ Pending、source 親 catalog に一致させる |
| 検索 (名前・メタ・タグ、結果 / drill-down) | Audio が実項目として materialize される各経路と streaming append の初期状態を共通化。SearchContainer の画像代表探索は拡張しない |
| レーティング一覧 / ★固定 | rating / tag view の Audio、snapshot 退避復元・sort・追加も同じ状態契約。複数親の同名 MP3 を誤用しない |
| 閲覧履歴 | ReadingHistoryKind::Audio の materialize / restore と同じ path key。履歴にない image を page として追加しない |
| スマートフォルダ root / scoped 子 | SmartFolderEntryKind::Audio → GridItem::Audio、prepare / root 退避復元でも source catalog に一致。sort-only rebuild で取得済み終端を失わない |
| 名前付きコレクション root / 物理子 | CollectionResolvedKind::Audio / duplicate reference を保ち、欠落 placeholder は要求しない。UUID / manual order を cache key に使わない |
| ブックマークの集約一覧 | bookmark_browser が Audio を materialize するため対象に含める。同じ MP3 の複数時刻は同じジャケット、既存 bookmark ID / jump は保持 |
| サブフォルダ展開 | 現在 Audio を除外している。今回も列挙仕様を変えず、Audio を追加しない |
| Drive / 本棚 / folder representative | 音声から自動代表を選ぶ機能には広げない。Audio セルとして出た対象だけ共通経路を使う |

共通 grid_paint の Audio は Loaded なら画像を fit、Pending / Evicted / NoArt / Failed なら
音楽アイコンとする。現在の切り取り表示は後段 `defer_primary_markers` でも大きな音符を描くため、
そこも同じ描画決定に揃え、切り取り時だけ画像に大きなアイコンが重ならないようにする。
★・タグ・長さ・再生位置バー・選択・切り取り opacity / ハサミは既存 overlay layout を保つ。
画像と Audio の識別表示は Q3 で決める。

詳細一覧では **左端の種別アイコンは Audio のまま**。既存 preview 列 / 選択情報バーの設定を尊重し、
画像表示を有効にしている面では同じ Loaded を使用する。hover は既存の
`set_details_hover_thumbnail_idx` の keep / priority 経路で要求し、hover 内で decode しない。
NoArt / Failed / 非 MP3 は音楽アイコンを表示して終端化し、「読み込み中...」repaint を止める。
hover を離れても他の可視 Audio の要求を取り消さない。

Auto サムネイル比率ではジャケット寸法を統計候補にする提案 (Q3)。現在の collection の
Audio 除外 eligible 数と、一般一覧の適格性 / dimensions の consumers を揃える必要がある。
NoArt の件数は終端として集計し、寸法が来るまで永久に確定を待たない。
PageDims に入る場合も Audio の has_page_data を変えず、画像編集・見開きページ数へ伝播させない。

## 7. mIV Remote を初回から含める

### 7.1 endpoint と本体生成

既存 `/api/thumb` の RemoteAddress { MP3 logical path, File } を使う。
新 album-art endpoint、APIC bytes の wire 転送、RemoteEntryKind の新種別は不要。
MP3 は Audio のまま、source_address は None。動画 sidecar 専用の source 指定を
音声へ開放せず、path guard / 認証 / session admission の既存検査を通す。

ThumbnailEngine は image/video/container の dispatch に MP3 AudioAlbumArt を追加し、
§3〜5 の本体共通生成関数へ渡す。catalog、選定、limits、CacheDecision、WebP を PC と共有する。
NoArt は既存 ThumbnailErrorCode::NoThumbnail、他の失敗は既存 error code に写像する。
absence cache hit も同じ終端応答。HTTP の 422 は再試行なし、画像 bytes は従来の image/webp。

現コードの ThumbnailEngine::handle は session_cancel を受け取らず、
generate_catalog_resolved 内の token は常に false。新抽出では pipe heavy handler に既にある
RemoteOperationCancellation を helper へ渡すことを必須とし、session release / takeover / shutdown
で source 読込を止める。Flight の結果待ちは既存 Condvar、UI thread を待たせない。
Flight を共有する waiter と owner は同じ session 世代の要求に限定し、session identity を
RequestKey に含める。旧 session の取消結果を次 session の MP3 要求へ返さない。
PC と Remote の in-flight token は共有せず、catalog の確定結果だけを再利用する。

ブラウザの AbortController は DOM / fetch を止めるが、既存 Thumbnail wire には
個別 cancel command がない。tile 離脱だけで core の I/O が即止まるとは主張しない。
旧 DOM への採用を拒否し、core の source job は 10 秒 / 入力上限内で終了させる。
session 取消は別途伝播する。個別 Thumbnail cancel IPC は今回新設しない。

### 7.2 Web UI と cache

createGridTile に MP3 の img と音楽アイコン fallback を両方置き、既存 thumbnail binding /
virtualization / request limiter / binding generation を使う。他の Audio は固定アイコンのまま。
成功時だけアイコンを隠し、NoThumbnail / source error ではアイコンを残して tracker を settled にする。
NoThumbnail は「対象ファイルが missing」の 404 と区別し、tile の open を disabled にしない。
再マウント・画面 idle で terminal tile を同じ binding に自動再要求しない。

HTTP の NoThumbnail 応答には識別可能な `error: no_thumbnail` を付ける提案。
IPC enum の追加は不要で、http.rs の error mapping と app.js の表示 / telemetry を揃える。
通常の無画像を client image_load_error として大量記録しない。既存 retry は network/busy のみで、
NoThumbnail / 422 は終端とする。auth / session 失効は既存の通信・ログアウト動作を維持する。

Remote の通常・検索 / tag・rating・history・smart・永続 collection・bookmark で
RemoteEntryKind::Audio の MP3 を同じタイルに渡す。catalog / UI の一覧ごとに別フラグを足さない。
原本と同じ logical address を保ち、remote-web が album-art の寸法から kind=image と推測しない。
成功画像の HTTP private max-age=60 と `remoteSessionCacheEpoch` の既存規約を維持する。
明示 refresh では新しい epoch / binding にして source stamp を再確認し、NoThumbnail は
no-store とする。外部タグ編集後の HTTP cache の最大 60 秒の遅延は明記して検証する。

### 7.3 IPC 版

基準は **コード v66**。既存 ThumbnailRequest / NoThumbnail で表現できても、
旧 core が MP3 生成を拒否するため、EPUB の意味拡張時と同様に **意味契約の変更として版を上げる**
提案とする。単独で次なら v67 だが、他ラインの統合と合わせて coordinator が番号を一つに決める。
この設計作業では定数も assertion も変更しない。

実装時は `crates/remote-ipc` の定数・handshake / serialization の版 assertion と、
web-remote-plan §13.5 の履歴・現行表記を同時更新する。本体・remote-web を両方 rebuild / restart
することが必要。版不一致は従来どおり拒否し、旧 core へ黙って対応しない。
wire に内部状態を追加する必要が出た場合は実装前に全 reader を数えて追加レビューする。

## 8. 利用者への質問 (回答前に決定として扱わない)

| ID | 質問と推奨回答 |
| --- | --- |
| Q1 | 初回は MP3 の埋め込み画像だけで、FLAC/M4A・外部 cover.jpg・再生画面は後回しでよいか。**推奨: はい。必須対応は JPEG / PNG、追加形式は既存 decoder で安全に扱える静止画に限定** |
| Q2 | 前面表紙がない、または前面表紙が壊れているとき、他の埋め込み画像を表示するか。**推奨: 前面表紙を元順で全部試し、その後は残りを元順で試して最初の使える画像。候補ゼロなら音楽アイコン** |
| Q3 | 自動表示を既定とし、画像がある Audio は隅に小さな音楽印を付け、Auto 比率にもジャケット寸法を使ってよいか。**推奨: はい。新設定は追加せず、詳細左端の種別アイコンは保持**。音楽印の位置は既存 badge / 長さ / 再生位置と競合させない |
| Q4 | §3.2 の巨大タグ / 候補 / decode 上限を超えるものは、音楽アイコンとログで扱ってよいか。**推奨: はい**。全画像対応なら memory と実装検証費用が増える。10 秒 timeout は永続 no-art にせず次の明示 reload で再調査 |
| Q5 | まれな album-art cache DB の作成・書込失敗は、画像表示を継続し、ログと一度の通知、次回読込で再生成するだけでよいか。**推奨: はい**。自動 retry / 復旧 journal は作らない。album-art の追加部分だけを対象とし、既存画像 cache や設定・タグ・collection を削除しない |
| Q6 | mtime 秒 + size が同じになる外部タグ編集は、自動検出を保証せず、既存の cache 削除 / 明示再生成で直す扱いでよいか。**推奨: はい**。より強い保証なら subsecond stamp または content hash の追加形式・移行が必要。普通の reload だけでは同じ stamp の永続行を再利用し得る |

## 9. 実装順序・受入条件・検証所有

1. coordinator が質問の回答を記録し、別 context の Sol / xhigh に本設計の独立レビューを依頼する。
   extraction の有界性、terminal 契約、親 catalog、Remote session Flight を重点とする。
2. 合意後の最初の作業は §3 の synthetic ID3 fixture と同梱 FFmpeg adapter 検証。
   front / back の選定 metadata、open-only、取消 / 上限が成立しなければ設計を戻す。
3. 型付き結果契約と NoArt の共通 consumer を挙動不変の chunk で整え、source catalog の
   追加 schema / helper、Audio 要求 / 全 surface の UI、Remote を同じ feature の完了範囲にする。
   「PC だけ先に完成」で項目を完了扱いにしない。
4. 実装者が以下の自動検証を所有し、coordinator は有効な結果を再利用する。

| 層 | 必須の受入テスト (今は未実行) |
| --- | --- |
| extraction | ID3v2.3 JPEG (要望素材相当の synthetic)、v2.4 PNG、v2.2 PIC、front/back 逆順、複数 front、壊れた front と正常な別画像、タグなし、URL APIC、unsynchronization、extended header、切れた tag、巨大 tag・dimensions・候補過多、期限 / cancel |
| state / routing | 対応 MP3 だけ要求が作られる、NoArt と非 MP3 で requested=0 / repaint=0 / upgrade=0、cancel 後に NoArt を保存しない、timeout で再投入しない、late message / upload backlog / Finalized の既存挙動、Loaded eviction と再表示 |
| catalog | 旧 v2 DB の画像 / ZIP / PDF / video_meta 行不変、追加 table 不在の read-only miss、positive↔negative、mtime だけ / size だけの変更、unknown stamp、同名別親・別 drive、Auto / Off / Always、SourceOnly、prune / rename / clear、small Remote が large row を上書きしない |
| cross-context | A / B の同じ・異なる MP3、片方の switch / close / cancel / reload で sibling texture / queue / NoArt 不変、park / restore / fork。製品起動を要さない fake worker / state tests |
| UI | 全 surface の MP3 Loaded / NoArt / Failed、詳細 hover・preview・選択バー、切り取り、暗 / 明 theme、duration / resume / badge、Auto aspect の終端。headless snapshots を追加する |
| Remote | endpoint の認証 / path guard、MP3 WebP / NoThumbnail、PC 未訪問の cache miss、全 Audio 一覧、422 非 retry、terminal tracker、stale DOM、virtualization、session cancel / next owner / 同一 session Flight、版 handshake・round-trip。Rust handler と Node runtime tests |
| playback 回帰 | cover-only MP3 は audio-only のまま、cover + 実映像は実映像を選ぶ。decoder の既存テストを保持し、ジャケット取得が player / 音声出力を作らないことを検証 |

既存固定アイコン仕様を変える feature なので、今回の設計作業に bug-fix red はない。
実装時に既存不具合も直す場合は、その違反境界の failing regression を修正前に実行し有効な red を残す。
targeted → full lib `cargo test -p mimageviewer --lib` (pipe なし・実 exit code)、
`cargo fmt`、normal / portable の core check、`python scripts/check_ui_glyphs.py` を行う。
共有 ThumbMsg 変更では [build/test policy](development-build-and-test.md) の full gate も必要。
Remote IPC / Web tests を省かず、CI 依存・timeout は十分な時間を確保する。

実装後は build-dev.ps1 で通常 core の確認 binary を用意し、Remote も同時 build する。
この設計のみの作業では build / test / binary 起動は不要で、実行済み件数は 0。
利用者による実機確認の候補は、多数 MP3 の scroll / 再訪・タグ編集後の更新、全一覧と詳細 hover、
art なしで idle CPU / repaint、再生中の一覧・F12・Remote の取得 / 切断、スマートフォン表示。
エージェントは製品 binary を起動しない。具体的な確認枠と利用者の明示承認は実装後に調整する。

## 10. 文書更新と今回の引き継ぎ

今回更新するのは本計画と docs/README.md の索引だけ。backlog の利用者決定や
未実装のマニュアル・製品紹介は完成形に書き換えない。
実装時に catalog-design、display-pipeline、async-architecture、architecture-overview、
music-integration-plan D2 / §3.2 (過去の決定は履歴として保存)、web-remote-plan §13.5 と新仕様、
spec、マニュアル、製品ページを co-update する。privacy の cache 保存 / 通信と製品ページの
「安心して使えます」も照合し、新しい外部通信は増えないこと、既存認証済み Remote への
ジャケット配信が画像配信の記述に含まれることを確認する。

coordinator への引き継ぎ: Q1〜Q6 の回答、独立設計レビュー、入力有界化の技術ゲート、
全 producer / consumer を含む実装 brief と file ownership、統合 IPC 番号の決定が次の作業。
commit は行わない。英語メッセージは `target/D-design-msg.txt` に置く。
