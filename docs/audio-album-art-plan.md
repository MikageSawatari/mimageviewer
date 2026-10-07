# MP3 埋め込みアルバムアートの一覧表示計画 (§1.347)

作成: 2026-10-07 / Line D (`next-audio-art`)。v4.4.0 後の設計のみ。
改訂: 独立設計レビュー R1 の REVISE 指摘 6 件への対応。
**未実装・利用者質問は未回答・独立レビュー R1 は REVISE・改訂版の再レビュー待ち・実アプリ未起動。**
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
MP3 から始めることは既決事項。画像形式・一覧外への拡張に関する未決範囲だけを Q1 に残す。

公式 [AVStream の契約](https://ffmpeg.org/doxygen/trunk/structAVStream.html) は
ATTACHED_PIC の packet を demuxer が所有すると定義している。
公式 [ID3 実装](https://ffmpeg.org/doxygen/trunk/id3v2_8c_source.html) は
APIC の picture type を stream の comment、description を title に公開する。
これは upstream の照合資料であり、同梱 n7.1 系への適用は fixture で検証する。

1. worker で MP3 拡張子 (大小文字を区別しない) と実ファイルを確認し、fresh stat から
   source stamp を確立する (§5.3)。一覧の表示・ソート日時や LoadRequest.mtime を使わない。
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
6. fresh stat を再取得し、この worker が最初に確立した source stamp と一致する場合だけ
   結果を公開・保存する。cache hit の公開前にも同じ検査を行う。

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

ローカルは既存 heavy I/O queue / GlobalIoSemaphore に乗せる案とする。
**ActivityGate は使わない**。`src/activity_gate.rs:13,103` の wait_until_idle は
indexer 用の無操作待ち・pause 待ちであり、現在の thumbnail worker はこれを待たない
(`src/app.rs:46599,46628`)。可視・hover は即時に enqueue し、画面外の先読みは既存の
prefetch idle 制御で投入前に抑える。MP3 を取り出した heavy worker 内で idle / pause を
待たせず、同じ queue の Folder / ZIP まで停止させない。新たな通常操作の待機は設けない。
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
- **indexer の ActivityGate 待機を共用する**: 不採用。heavy worker を待機で占有しないよう、
  既存の enqueue 前の prefetch idle 制御に揃える (R1-2)。
- **Remote の全成功画像を新 RAM cache に保存する**: 不採用。既存 HTTP cache を使い、
  DOM の寿命より長く必要な NoArt / Failed だけを一覧 owner に保持する (R1-5)。
- **Remote の更新 timer / 自動再検証を追加する**: 不採用案。更新保証を明示 refresh に限定して
  epoch を変える Q7 を推奨し、期限や再試行 owner を増やさない (R1-6)。
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
作り直してよいとは扱わない。既存 CATALOG_VERSION=2 は保持する。
**既存 init_schema は worker 専用ではない**。通常一覧の `src/app.rs:36864` の
get_or_open_catalog → `src/catalog.rs:408` の init_schema と、`src/app.rs:36938` の
delete_missing は UI から同期到達する (R1-1)。したがって共通 init_schema に
audio_art_absence の CREATE / migration を追加せず、delete_missing に absence の SQL を足さない。

新設する `ensure_audio_art_schema` (仮称) は **AudioAlbumArt worker の入口だけ**から呼ぶ。
呼出境界はローカル `spawn_thumbnail_workers → process_load_request の Audio 専用 dispatch`
または Remote `pipe heavy handler → ThumbnailEngine の Audio 専用 dispatch` とする。
worker が source 親の catalog handle を開き、既存の一般 schema 確立を済ませた後に
album-art の追加 schema を確立し、その handle を worker 所有 LRU に保持する。
UI の catalog handle / mutex を追加 DDL のために借用せず、準備完了を待つのも worker だけとする。
UI に schema 準備待ちを追加しない。必要な schema が作れなければ art cache だけ使用せず、
source 取得へ進む (Q5)。
positive / absence の SELECT・保存・scope prune もこの worker 境界内に限定する。

成功行を既存 thumbnails に置くため、既存の UI 向け load_all / load_source_dims 等の一括 SELECT
では audioart namespace を SQL で除外する。先に全 BLOB を読んでから UI で filter しない。
既存の一般一括読込・掃除の呼出回数を増やさず、Audio の worker 専用 load_one / scoped query
だけが art 行を読む。UI の aspect / preview は worker の結果を受け、同期の Audio cache seed を
追加しない。cache 管理の全件削除は既存の明示操作を保ち、新しい Audio 分の SQL は worker に置く。
旧 DB への初回 open / 再 open / 旧 read-only reader との共存で既存行不変をテストする。
read-only handle は追加 table 不在を cache miss と扱い、CREATE / migration を行わない。

成功保存時は同じ key の absence を削除、NoArt 保存時は同じ key の成功行を削除し、
同一 transaction で排他にする。Ready フラグや第三の状態表は追加しない。
cache miss は「未調査」であり、no-art と推測しない。

### 5.2 key と親 catalog

R1-4 に合わせ、未実装の key 案を **親の所有 scope が prefix で限定できる形式**へ改訂する。
全 caller 共通の builder が `audioart:<scope_hash>:v1:<normalized basename>` を作る。
scope_hash は `SHA-256(normalize_keep_drive(logical source parent))` であり、
source 親の完全な identity (ドライブ・大小文字・区切りの既存規則) を保持する。
catalog DB 自体の hash 規約は変更しない。型付き `AudioArtCatalogScope` (仮称) が
元の source 親と `audioart:<scope_hash>:` prefix を一緒に持ち、正規化済み basename から
成功・absence の同じ key を作る。キー文字列を切り分けてパスを復元しない。
algorithm version は前面表紙選定・対応形式・
上限を変更したときに進め、旧 negative の意味を持ち越さない。

保存先は常に **元 MP3 の実親に対応する catalog**。通常一覧でも完全な source identity の key を使い、
検索・rating・history・smart・collection と Remote で同じ行を参照する。
合成一覧の synthetic current_folder の DB に MP3 行をコピーして保存しない。
worker が有界な親 catalog handle LRU (推奨 8 親、worker 所有) を使って lookup する。
既に開いた worker 所有の同じ親 handle は再利用できる。UI の一括 matching map は art 行を持たず、
「通常一覧なら UI が Audio cache を事前ロードする」という経路は作らない。
異なる親の同名 song.mp3 と、親 hash が共有される別 drive の同名 path を key で分離する。
logical / canonical path の使い分けは Remote path_guard と既存 catalog 対応に揃える。

`src/catalog.rs:39` は通常の親 path から drive を除いて DB を決めるため、C:\Music と D:\Music は
同じ DB を共有する。`delete_missing` (`src/catalog.rs:963`) は全 thumbnails 行を現在の
existing 集合だけで削除するので、art key を集合へ追加するだけでは別 drive の行を消す。
**既存の一般 delete_missing は audioart namespace の全行を対象外にする**。
同じ除外を一般の一括読込 / cleanup caller へ揃え、absence table を一般掃除に加えない。
`folder_thumb_existing_keys_for` は一般媒体用のままとし、art の保存先や scope を決める owner にしない。

新しい `prune_audio_art_scope(scope, complete_inventory, cancel)` (仮称) は worker 上で、
scope が持つ `audioart:<scope_hash>:` の prefix 範囲だけを positive / absence 両方から読む。
prefix を含む key は version をまたいで **その直接の source 親だけ**を所有するため、
親 path 以下の部分文字列検索・DB 全件列挙・別 drive / 子親の巻き込みは行わない。
削除はその範囲内で physical inventory に存在しない basename の exact key に限る。
inventory は source 親に属する完走した物理 scan snapshot を、facet 適用前に worker へ渡す。
source 親・完走・取消 / 一覧世代を確認できない snapshot では prune しない。
既存 Audio worker のその scope の初回準備にまとめ、専用掃除 thread / 毎 MP3 の再走査を作らない。
合成一覧はその表示行だけを完全 inventory とみなさず、source 親の complete snapshot がなければ
掃除を見送る。再生成可能な古い行は次の適格な物理一覧準備か明示 cache 管理まで残してよい。
取消・古い一覧の maintenance は保存と同様に実行 / commit しない。
facet / 検索で隠れた行を物理的な missing としない。

ファイル・フォルダ単位の明示 cache 削除も exact key / source scope に限定し、
共有 DB のファイル自体を消して別 drive の art を巻き込まない。全 cache の明示削除は従来どおり。
smart/collection prepare、rename/move の cache consumer もこの境界を共有する。
rename/move 後は新 key で再生成し、
旧行は上記の scoped prune へ任せる。利用者のタグ・★・collection migration には変更を入れない。

### 5.3 stamp・方針・再生成

**表示・ソート metadata と source stamp を分離する (R1-3)。**
`src/bookmark_browser.rs:564` の image_meta は MP3 mtime を bookmark.created_at_ms の秒へ
置換し、`src/app.rs:48125,90062` はそれを要求 mtime に渡す。
`reading_history_meta_for_entry` (`src/app.rs:89660`) は未取得 mtime / size を 0 にする。
これらの値を要求時の「原本 stamp」と扱う前提を撤回する。

AudioAlbumArt の要求は source path / scope / policy / context 相関を持ち、stamp の正本は
**worker 開始後の実ファイル stat だけ**とする。共通 LoadRequest に残る mtime / file_size は
Audio dispatch の cache lookup / freshness 判定へ渡さず、新たな stamp Option や別 bool で補わない。
Audio 専用 dispatch は一般画像の caller-stamp cache lookup より前に分岐する。
enqueue も Audio を `image_metas == None` による skip より先に扱い、未知の表示 meta の MP3 も
要求できるようにする。bookmark の登録日時 / history の表示値・ソート順は変更しない。

worker が最初の stat 成功時に known `AudioArtSourceStamp { mtime_secs, file_size }` (仮称) を作り、
lookup / decode / 保存 / 公開をこの一つの stamp に揃える。stat 失敗は一覧内 Failed で止め、
0 を代入した cache lookup / 書込をしない。同じ MP3 の別 bookmark は source key と stamp が同じで、
bookmark の時刻・ID は cache identity に入らない。
lookup は scope key / algorithm version / **MP3 自体の mtime 秒 + file size の完全一致**を条件とする。
image bytes の長さ・親フォルダ mtime・登録日時を stamp にしない。
worker は読込前と公開前・保存前に最初の fresh stat と再取得値を照合し、
ロック / transaction 内で stat しない。
mtime の大小から新旧を推測しない。遅い旧 worker が上書きしても異なる stamp なら次回 miss になる。
同時編集が同じ秒・同じ size の場合はこの規約だけでは検出できないため、その限界を Q6 に残す。

成功・absence の永続化とも既存 CacheDecision に従う提案。
Off は永続書込なし、Always は確定結果を書込、Auto は元 MP3 size と抽出・decode・縮小時間で
判定する。元の無画像結果でも調査時間を測る。Auto で保存しない場合も NoArt は
一覧世代内に残るため idle loop は生まれない。本体は既存 ThumbnailState を終端 owner とする。
Remote の DOM 外の終端 owner は §7.2 で定義し、永続 cache の保存有無に依存させない。
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
| 閲覧履歴 | ReadingHistoryKind::Audio の materialize / restore と同じ source key。履歴の未取得 mtime / size の 0 は worker source stamp へ使わない。履歴にない image を page として追加しない |
| スマートフォルダ root / scoped 子 | SmartFolderEntryKind::Audio → GridItem::Audio、prepare / root 退避復元でも source catalog に一致。sort-only rebuild で取得済み終端を失わない |
| 名前付きコレクション root / 物理子 | CollectionResolvedKind::Audio / duplicate reference を保ち、欠落 placeholder は要求しない。UUID / manual order を cache key に使わない |
| ブックマークの集約一覧 | bookmark_browser が Audio を materialize するため対象に含める。登録日時の display meta は保持し、worker が別途 source stamp を確立。同じ MP3 の複数時刻は同じジャケット、既存 bookmark ID / jump は保持 |
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

**終端 owner は DOM ではなく、採用済み一覧の世代に置く (R1-5)。** 現行の
`app.js:8947` の image._thumbnailSettled は binding 作成時にリセットされ、
VirtualGrid は `app.js:9741,9766` でセルを破棄・再作成する。既存 binding だけでは不十分。
新しい `AudioArtListState { generation, artEpoch, terminalByAddress }` (仮称) を一覧 owner に置き、
terminalByAddress は正規化した MP3 logical address に `NoArt | Failed` だけを保持する。
画像 bytes / 成功 cache / DOM reference は保持せず、同じ MP3 の重複 bookmark は同じ結果を参照する。
容量は現一覧に含まれる unique MP3 address 数を上限とし、LRU eviction で終端を忘れない。

初回要求は既存の有界 retry / limiter を使う。NoThumbnail / 422 は NoArt、source error・
応答画像の decode 失敗・通信 retry の打切りは Failed として一覧 owner に記録してから DOM へ投影する。
fetch の意図的な abort と auth / session 失効は terminalByAddress に記録しない。
結果は同じ session・一覧世代に属し、その address が現一覧に残る場合だけ受理する。
DOM が既に消えていても確定応答を受理できれば一覧 owner に残し、旧 binding への描画だけ拒否する。
abort で結果を受け取れなかった要求を「無画像」と推測することはない。

再マウント時は fetch 前に terminalByAddress を参照し、終端なら音楽アイコンと tracker settled を
復元して HTTP / core 抽出を開始しない。Cache Off / Auto の absence 未保存でも同じである。
scroll eviction、同じ一覧 payload の sort / resize / 表示形式切替では owner を保持する。
cleanupScreen / VirtualGrid.destroy による DOM 破棄と、一覧 payload の廃棄を区別する。
一覧を再取得して採用した時、明示 refresh、session 切替、別一覧への移動では旧 owner を捨てる。
同じ一覧の単なる render を新しい一覧世代として扱わず、streaming append は同じ世代へ追加する。
refresh 後の旧応答は新しい map に記録しない。複数の過去一覧を保持する cache は作らない。

HTTP の NoThumbnail 応答には識別可能な `error: no_thumbnail` を付ける提案。
IPC enum の追加は不要で、http.rs の error mapping と app.js の表示 / telemetry を揃える。
通常の無画像を client image_load_error として大量記録しない。既存 retry は network/busy のみで、
NoThumbnail / 422 は終端とし、no-store を維持する。auth / session 失効は既存の通信・ログアウト動作を維持する。

Remote の通常・検索 / tag・rating・history・smart・永続 collection・bookmark で
RemoteEntryKind::Audio の MP3 を同じタイルに渡す。catalog / UI の一覧ごとに別フラグを足さない。
原本と同じ logical address を保ち、remote-web が album-art の寸法から kind=image と推測しない。

**「外部編集後、最大 60 秒で更新」の保証を撤回する (R1-6)。** 成功応答は
`http.rs:3601` の private, max-age=60 だが、取得側は `app.js:9126` の force-cache。
[Fetch Standard の cache mode](https://fetch.spec.whatwg.org/#concept-request-cache-mode) は
force-cache で期限切れの一致応答も使うと定める。さらに `app.js:1893` の
remoteSessionCacheEpoch は session identity 変更時の更新であり、一覧再描画で変わる前提は成立しない。

**更新保証は明示 refresh に限定する提案 (新規 Q7)。** 通常の取得は既存 force-cache と
max-age=60 を保持し、timer / polling / 自動再検証は追加しない。既存 remoteSessionCacheEpoch は
session 用のままとし、新しい MP3 一覧 payload の採用時に非秘密の artEpoch nonce を作る。
`/api/thumb` の MP3 URL に artEpoch を追加し、同じ payload の再描画では変えない。
これは HTTP cache の URL 区別だけであり、IPC の address / catalog key には含めない。
一覧再取得の採用では generation と artEpoch を同時更新し、terminalByAddress も空にする。
明示 refresh の確実な既存操作は **ブラウザのページ再読込**。一覧内の refresh 操作がある経路も
再取得・採用へ揃えるが、新しい refresh ボタンが既にあるとは扱わない。
新 URL が旧 HTTP cache を迂回し、core worker が §5.3 の実 source stamp を再確認する。

受入条件は、外部でジャケットを交換・削除・追加して stamp が変わった MP3 が、明示 refresh 後に
新画像または NoArt へ更新されること。refresh 前は 60 秒を超えて旧表示が残り得ると説明する。
同じ秒・同じ size の編集では refresh だけで永続 cache を迂回できず、Q6 の削除 / SourceOnly が必要。
Q7 が否なら MP3 の再検証方針と終端結果の再調査契約を再設計し、保証だけを先に記載しない。

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
| Q1 (改訂) | MP3 から・初回から Remote 対応は決定済み。未決の画像形式は JPEG / PNG を必須、追加形式は既存 decoder で安全に扱える静止画に限定し、外部 cover.jpg・手動指定・再生画面への表示は後回しでよいか。**推奨: はい**。FLAC/M4A 等は MP3 の次の拡張として扱う |
| Q2 | 前面表紙がない、または前面表紙が壊れているとき、他の埋め込み画像を表示するか。**推奨: 前面表紙を元順で全部試し、その後は残りを元順で試して最初の使える画像。候補ゼロなら音楽アイコン** |
| Q3 | 自動表示を既定とし、画像がある Audio は隅に小さな音楽印を付け、Auto 比率にもジャケット寸法を使ってよいか。**推奨: はい。新設定は追加せず、詳細左端の種別アイコンは保持**。音楽印の位置は既存 badge / 長さ / 再生位置と競合させない |
| Q4 | §3.2 の巨大タグ / 候補 / decode 上限を超えるものは、音楽アイコンとログで扱ってよいか。**推奨: はい**。全画像対応なら memory と実装検証費用が増える。10 秒 timeout は永続 no-art にせず次の明示 reload で再調査 |
| Q5 (改訂) | まれな album-art cache DB の schema 確立・読込・書込失敗は、source からの画像表示を継続し、ログと一度の通知、次回読込で再生成するだけでよいか。**推奨: はい**。自動 retry / 復旧 journal は作らない。album-art の追加部分だけを対象とし、既存画像 cache や設定・タグ・collection を削除しない |
| Q6 | mtime 秒 + size が同じになる外部タグ編集は、自動検出を保証せず、既存の cache 削除 / 明示再生成で直す扱いでよいか。**推奨: はい**。より強い保証なら subsecond stamp または content hash の追加形式・移行が必要。普通の reload だけでは同じ stamp の永続行を再利用し得る |
| Q7 (新規・R1-6) | Remote の外部編集・ジャケット削除 / 追加の反映保証は、ブラウザ再読込などの明示 refresh に限定してよいか (同 stamp は Q6 の例外)。**推奨: はい**。新しい一覧採用時の artEpoch で HTTP cache と終端結果を更新し、「最大 60 秒」保証や自動 polling は設けない |

## 9. 実装順序・受入条件・検証所有

1. coordinator が Q1〜Q7 の回答を記録し、別 context の Sol / xhigh に改訂設計の再レビューを依頼する。
   R1 の 6 件の変更境界、extraction の有界性、terminal 契約、親 catalog、Remote session Flight を重点とする。
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
| worker 境界 (R1-1/2) | 通常 / 合成一覧の UI 呼出を計測し、追加 schema / art SELECT / absence / scoped prune が UI に 0 回、UI bulk query が art BLOB を読まない。ActivityGate pause / 連続入力中でも可視・hover を gate 待機させず、同じ heavy queue の Folder / ZIP も進む |
| source stamp (R1-3) | bookmark 登録日時≠原本 mtime、同一 MP3 の異なる日時の複数 bookmark、history の未知 stamp 0 / 表示 meta None でも worker stat で Loaded / NoArt になる。stat 失敗で stamp 0 の cache 行を書かず、読込中の実 stamp 変更は公開・保存しない |
| catalog | 旧 v2 DB の画像 / ZIP / PDF / video_meta 行不変、追加 table 不在の read-only miss、positive↔negative、mtime だけ / size だけの変更、同名別親、Auto / Off / Always、SourceOnly、prune / rename / clear、small Remote が large row を上書きしない |
| prune 所有 (R1-4) | 同一 DB を共有する C:\Music / D:\Music を交互に物理一覧で開いて両方の positive / negative を保持。片親で消えた MP3 だけを両 table から scoped prune、別 drive / 子親 / facet 非表示は保持。一般 delete_missing も art 行を消さず、合成一覧・未完走 scan・旧世代は prune しない |
| cross-context | A / B の同じ・異なる MP3、片方の switch / close / cancel / reload で sibling texture / queue / NoArt 不変、park / restore / fork。製品起動を要さない fake worker / state tests |
| UI | 全 surface の MP3 Loaded / NoArt / Failed、詳細 hover・preview・選択バー、切り取り、暗 / 明 theme、duration / resume / badge、Auto aspect の終端。headless snapshots を追加する |
| Remote | endpoint の認証 / path guard、MP3 WebP / NoThumbnail、PC 未訪問の cache miss、全 Audio 一覧、422 非 retry、terminal tracker、stale DOM、virtualization、session cancel / next owner / 同一 session Flight、版 handshake・round-trip。Rust handler と Node runtime tests |
| Remote 終端 owner (R1-5) | Cache Off / Auto absence 未保存で NoArt / Failed を受信後、DOM eviction→再訪しても同じ一覧世代の HTTP / core 抽出は 0 回。重複 address・sort / resize でも保持、refresh / 別一覧で破棄。旧世代完了は新 map に入らず、意図的 abort / session 失効は結果を汚染しない |
| Remote 更新 (R1-6/Q7) | fake HTTP cache で 60 秒超の force-cache 再利用を再現し、refresh 前の自動更新を要求しない。stamp が変わった外部交換・削除・追加後、明示 refresh が新 artEpoch URL で core へ到達し、新画像 / NoArt になる。同 stamp は Q6 の限界として別検証 |
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

初版では本計画と docs/README.md の索引を追加した。今回の follow-up は本計画だけを改訂する。
backlog の利用者決定や
未実装のマニュアル・製品紹介は完成形に書き換えない。
実装時に catalog-design、display-pipeline、async-architecture、architecture-overview、
music-integration-plan D2 / §3.2 (過去の決定は履歴として保存)、web-remote-plan §13.5 と新仕様、
spec、マニュアル、製品ページを co-update する。privacy の cache 保存 / 通信と製品ページの
「安心して使えます」も照合し、新しい外部通信は増えないこと、既存認証済み Remote への
ジャケット配信が画像配信の記述に含まれることを確認する。

coordinator への引き継ぎ: Q1〜Q7 の回答、R1 対応箇所の再レビュー、入力有界化の技術ゲート、
全 producer / consumer を含む実装 brief と file ownership、統合 IPC 番号の決定が次の作業。
commit は行わない。HEAD 上の follow-up 用英語メッセージは `target/D-design-r2-msg.txt` に置く。
初版の `target/D-design-msg.txt` は変更しない。

## 11. 独立設計レビュー R1 への対応記録

6 件ともコードで再確認して採用した。指摘への異論はない。独立レビューの合格を主張せず、
改訂した所有境界・更新契約は再レビュー待ち、Q1〜Q7 は利用者回答待ちとする。

| 指摘 | 確認したコードと解消内容 |
| --- | --- |
| R1-1 | app.rs:36864,36938 / catalog.rs:408 の同期経路を確認。§5.1 で共通 init_schema / delete_missing への追加を禁止し、追加 schema・art 読込・掃除を worker の専用 dispatch に限定、UI bulk query は art を SQL で除外 |
| R1-2 | activity_gate.rs:13,103 と app.rs:46599,46628 を確認。§4.1 / §4.3 で ActivityGate 不使用、可視・hover は待機せず、先読みは既存の投入前 idle 制御のみ |
| R1-3 | bookmark_browser.rs:564 / app.rs:48125,90062,89660 を確認。§3.1 / §5.3 / §6 で表示日時を保持しつつ worker fresh stat を唯一の source stamp とし、未知表示 meta でも Audio 要求可能。§9 に bookmark・history 回帰条件を追加 |
| R1-4 | catalog.rs:39,963 を確認。§5.2 で DB hash 維持、drive を含む親 scope prefix の key と complete inventory による worker prune を定義。一般 delete_missing から art を除外し、§9 に別 drive の positive / negative 保持を追加 |
| R1-5 | app.js:8947,9741,9766 の binding 初期化・DOM eviction を確認。§7.2 に一覧世代 / address の NoArt・Failed owner と破棄条件を定義。§9 に Cache Off / Auto と再マウントの無再要求テストを追加 |
| R1-6 | http.rs:3601 / app.js:9126,1893 と Fetch Standard を確認。§7.2 の 60 秒保証を撤回し、明示 refresh と artEpoch 更新を提案。新規 Q7 と §9 の外部交換・削除・追加テストで契約を一致させた |
