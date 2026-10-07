# 音声サムネイル: 同名 sidecar・MP3 埋め込み画像の計画 (§1.347)

作成: 2026-10-07 / Line D (`next-audio-art`)。v4.4.0 後の設計のみ。
改訂: 2026-10-08 / R7。永続コレクションの snapshot / navigation replacement に sidecar 出所を渡す契約を補う。
**未実装・Q1 訂正と Q2〜Q7 は決定済み・利用者への未決質問なし・実アプリ未起動。**
利用者から伝達されたレビュー履歴: 前版は **ACCEPT**、R4 の改訂は **ACCEPT WITH CHANGES**。
R6 (4fe5979e0) の再レビューは **REVISE** (永続コレクションの出所伝達 1 件)。R7 はその対応で、再レビュー待ち。
以下の「提案」は実装担当の推奨であり、利用者の決定済み事項とは区別する。

## 1. 決定済みの範囲と守る契約

[次の版のバックログ §1.347](next-release-backlog.md) の
**「次の版の決定 (利用者 2026-10-07): ライン D。MP3 から。最初から Remote にも対応する。」**
を優先する。古い [音楽設計 D2 / §3.2](music-integration-plan.md) の
「音楽アイコン固定」は、MP3 の一覧サムネイルについて今回の決定で更新する。
**利用者の決定 (2026-10-08):**

| ID | 確定仕様 |
| --- | --- |
| Q1 (訂正、2026-10-08) | 「動画と同じ」は同じフォルダ・stem の画像 (song.mp3 + song.jpg) を使う意味。**同名 sidecar > MP3 埋め込み画像 > 音楽アイコン**。動画の既存 sidecar 機構と設定を音声へ拡張し、自動埋め込み抽出は MP3 から、Remote も初版から |
| Q2 (推奨を採用) | 前面表紙を元順で全部試し、その後に他候補を元順で試す。最初の使える画像を表示、候補なしならアイコン |
| Q3 (変更) | 自動表示と Auto 比率への画像寸法使用を採用。音声専用設定は **音楽アイコンを画像上に描く (既定) / 文字バッジのみ / なし** の 3 択 |
| Q4 (推奨を採用) | §3.2 の入力・decode 上限を採用。上限超過はアイコンとログ、10 秒 timeout は永続 NoArt にしない |
| Q5 (推奨を採用) | まれな album-art **cache** DB 障害は source 表示を継続し、ログ・一度の通知・次回再生成。自動 retry / 復旧 journal は作らず、利用者データを削除しない |
| Q6 (推奨を採用) | mtime 秒 + size が同一の外部編集は検出保証なし。cache 削除 / 明示 SourceOnly 再生成で対応 |
| Q7 (推奨を採用) | Remote の外部編集反映は明示 refresh に限定。新一覧の artEpoch で HTTP cache と終端結果を更新し、60 秒保証 / 自動 polling は設けない |

**訂正の理由 (利用者 2026-10-08):** 埋め込み画像を差し替えることはまれであり、
動画の既存動作に揃える方が理解しやすい。R4 / R5 の手動指定案は coordinator の誤読に基づき撤回する。
既存の実ファイル `GridItem::Audio` 全形式が同名 sidecar の対象。非 MP3 も同名画像があれば表示する。
FLAC/M4A 等の **自動埋め込み抽出**、folder.jpg / cover.jpg の汎用フォルダ表紙探索、タグ書込、
音楽再生画面への画像表示は延期する。同名画像だけを使い、別 stem の folder.jpg を探索しない。
画像 picker・貼付・drag/drop・Remote upload は今回実装しない。将来案は §3.3 の短い注記だけに置く。

- 同名 sidecar も使用可能な埋め込み画像もない音声は従来の音楽アイコンを表示する。
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
[detached 憲法 §2](detached-rework-plan.md)。音声の P / ring / menu 操作は追加しない。
既存の動画 frame pin、Folder 代表指定、keymap の機能は変更しない。

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
| sidecar の設定と一覧省略は別の既存 field | `src/settings.rs:4753,5691,7415,7676` の skip_image_if_video_exists / video_thumb_use_sidecar_image は既定 true。folder_scan.rs:314,831 は省略 ON なら画像行を除外し、採用も ON の場合だけ override を返す |
| sidecar 発見と Remote 出所は共通化済み | folder_scan.rs:139 の aggregate 発見、collection_grid.rs:403、smart_folder.rs:4718,4920、remote_ipc/mod.rs:77,107,133 の RemoteThumbnailSources、thumbnail.rs:549 の same-parent / stem / image guard。今は Video 限定なので Audio を同じ owner に加える |
| 表示設定には既存の 3 択 pattern がある | `src/settings.rs:617,4437,7368,9805` の VideoThumbnailIndicator、`src/settings_transfer.rs:269` の plain 分類、`src/ui_dialogs/preferences/pages.rs:1621`。現在 Remote はこの動画設定を反映せず、音声設定の配信は追加が必要 |
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
自動抽出が MP3 から始まることは既決事項。同名 sidecar は §3.3 の既存画像経路で先に処理し、
MP3 parser を通さない。埋め込み JPEG / PNG の必須対応・上限と、sidecar の既存画像形式を分ける。

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

利用者が採用した初期値 (2026-10-08、Q4): ID3 領域合計 32 MiB、候補 16 枚、候補 bytes 16 MiB、
一枚 40 MP / decode 割当 160 MiB、抽出開始から 10 秒。
候補ごとに bytes / 寸法の上限を先に検査し、
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

### 3.3 動画と同じ同名 sidecar (Q1 訂正、2026-10-08)

#### 発見・候補・一覧での画像行

`song.mp3` と **同じ物理親・同じ stem (大小文字を既存規則で正規化)** の画像だけを採用する。
source 音声 path の正規化は drive を保持する。同名別親、別 drive、別 stem の cover は代用しない。
対象 Audio は `SUPPORTED_AUDIO_EXTENSIONS` の mp3 / flac / wav / m4a / aac / ogg / opus / wma。
動画扱いの mp4 等を音声へ再分類しない。

候補画像は動画 sidecar と同じ `ScanMediaKind::Image` / `folder_tree::is_recognized_image_ext`。
native リストは jpg / jpeg / png / webp / bmp / gif / heic / heif / avif / jxl / tiff / tif と
既存 RAW 一式、有効な Susie 拡張も同じ認識規則に入る。音声専用の JPEG / PNG リストは作らない。
RAW preview / codec 不在 / plugin の扱い、orientation、縮小・WebP、静止サムネイルは既存画像経路を使う。
APIC の §3.2 の入力上限を sidecar の対応形式へ転用して既存動画画像を制限しない。
複数同 stem 画像は既存 filter の順序と map への採用規則を共有し、音声専用の拡張子優先順は設けない。
現状は filter が走査順の全候補を返し、map 化で後の候補が残るため、常に JPEG 優先とは説明しない。

`filter_video_image_duplicates`、`discover_aggregate_video_sidecars_while`、`video_thumb_overrides`、
prepared listing / collection / smart / viewer bundle の対応 map を **媒体共通へ拡張する一経路**とする。
実装で名称を一般化してもよいが、別 audio_thumb_overrides、音声用走査 cache / watcher / DB は作らない。
一つの物理フォルダの Video と Audio を同時に処理し、同 stem の複数音声・動画は同じ画像を参照できる。
通常一覧の map も共通 builder の normalized full media path で引き、aggregate で stem-only を使わない。
現在の動画 worker は full path → legacy stem の順 (`app.rs:46762`) だが、新しい Audio 要求には
full-path provenance を渡し、前のフォルダの stem map を流用しない。

**画像行を隠す条件と採用条件は既存動画のまま音声にも適用する。**
serialized field 名は両方とも変更しない:

| skip_image_if_video_exists | video_thumb_use_sidecar_image | 物理一覧の同 stem 画像行 | Audio の source |
| --- | --- | --- | --- |
| ON | ON | 隠す (全同 stem 画像) | sidecar → 埋め込み → アイコン |
| ON | OFF | 隠す | 埋め込み → アイコン |
| OFF | ON / OFF | 残す | 埋め込み → アイコン (現行動画と同じく override を作らない) |

省略 setting を音声にも適用することは今回の「動画と同じ一覧動作」の範囲。
aggregate 発見は親ごとの物理 scan に既存 filter をかけて出所だけ返し、検索 / rating / history /
bookmark / named collection の明示結果を勝手に削除しない (`remote_ipc/mod.rs:70`)。
smart の物理走査側では現行 duplicate filter の同じ段階で画像を省略し、画像 rule の選定前後の順序を保つ。
親走査の取消・既存 64 親上限・scan_errors を共有し、媒体ごとにもう 64 親を足さない。
既存動画の候補親を先に確保し、残りの予算に Audio の親を dedup して入れ、動画の既存発見を押し出さない。
上限外 / 発見不能の親は sidecar 出所を付けず、MP3 は埋め込み、他 Audio はアイコン。打切りをログへ残す。
サブフォルダ展開の Audio 除外は保持し、その同名画像も Audio だけを理由に隠さない。

#### 共通 worker・更新・cache

一覧準備 worker が media → selected image の出所 snapshot を返し、AudioThumbnail worker は
**同名画像を先に解決・decode → MP3 の埋め込み → NoArt** と進む。
動画は既存の **frame pin → sidecar → Shell** の順を保ち、動画 pin の保存・操作・DB には触れない。
sidecar decode は既存画像 helper を利用し、UI で read_dir / stat / decode しない。
sidecar が不正な画像・取得前に消えた場合はログを残して MP3 の埋め込みへ進む。
path guard / auth 違反は拒否し、権限・timeout・runtime 障害は Failed の終端で止める。自動 retry は追加しない。
動画の既存 Shell retry や sidecar failure 挙動は今回変更しない。

sidecar の cache は既存 **選択画像の画像 cache 経路**を使う。元 Audio の embedded row と混ぜない。
画像 source の full path と fresh mtime / size を相関・検証に使い、Audio の mtime、bookmark 登録日時、
親 folder mtime で画像の更新を判定しない。PC の動画 sidecar は worker の直接 decode、Remote は
`generate_catalog_resolved` の画像 catalog 経路であり、両 caller の既存画像 helper / policy を共有する。
公開前に選択 source / stamp と context 世代を検査し、途中編集・旧一覧の pixels を採用しない。
source の image cache key / catalog の ownership は共通画像 builder に従い、Audio への新しい永続指定は持たない。
album-art 成功 / absence は MP3 の埋め込みだけ (§5)。その cache hit / NoArt より sidecar 解決を先にする。

同名画像の追加・編集・削除・rename、対象音声の改名、設定変更は既存 scan / reload で map を作り直す。
現在の `poll_current_folder_watch → start_external_rescan` は filter **前**の scan signature を使うので、
隠した sidecar の変更も確認対象に含む (`app.rs:23235,23400`)。新しい watcher / repaint timer は作らない。
合成一覧・smart の凍結 snapshot は既存の明示再取得へ揃える。別 context の未変更 map / texture は reset しない。
設定変更は `preferences.rs:2928` の既存 duplicate-settings reload に音声も含め、専用 live-rebuild を作らない。
同じ source stamp の編集の検出限界は Q6、Remote は Q7 の明示 refresh に従う。

#### 既存の出荷済み設定を一つ使う

**video_thumb_use_sidecar_image を動画・音声共通に拡張**する。serde field 名、settings DB key、
既定 true、設定転送の field 名 / bool 型は保持し、migration / 新しい sidecar 用 audio setting は不要。
既存利用者が OFF にしていた場合、音声 sidecar も OFF。勝手に true へ戻さない。
環境設定の既存 checkbox を「同名の画像をサムネイルに使う（動画・音声）」へ relabel し、
movie.mp4 + movie.jpg / song.mp3 + song.jpg の例、省略 setting も必要なこと、OFF 時の fallback を説明する。
動画 pin は引き続き独立して最優先であり、「今後実装予定」という古い説明は出荷済み挙動に合わせる。
`skip_image_if_video_exists` も field 名 / 既定を保ち、「同名の動画・音声がある画像を省略」へ表示を揃える。
既存の preferences anchor を保ち、検索語に音声を加える。設定 transfer の既存 plain / plain 分類は不変で、
表示名・説明を動画・音声へ更新する。manual は同名画像の置き方、二つの setting の関係、OFF の引継ぎを記す。

**将来の短い注記:** 後で利用者から求められた場合だけ、画像 picker で選んだ画像のサムネイルを
選択時に cache の代表画像として書く案を検討する (利用者の提案)。今回は DB pin も picker も追加しない。

## 4. Worker・取消・状態の簡素化

### 4.1 所有者と要求

新しい常駐 album-art pool、全 MP3 の先行走査、再生分析との統合は作らない。
既存 `LoadRequest` の `ResolveStrategy` に AudioThumbnail (仮称) を追加し、実 Audio を要求可能にする。
一つの worker dispatch が共通 sidecar source → MP3 のみ埋め込み抽出 → NoArt と進む。
一覧種別ごとのフラグではなく共通 `audio_art_request_for(item)` (仮称) が適格性と key を決める。
通常・合成一覧・復帰・streaming 追加・clone/fork の初期状態も同じ helper へ揃える。

ローカルは既存 heavy I/O queue / GlobalIoSemaphore に乗せる案とする。
**ActivityGate は使わない**。`src/activity_gate.rs:13,103` の wait_until_idle は
indexer 用の無操作待ち・pause 待ちであり、現在の thumbnail worker はこれを待たない
(`src/app.rs:46599,46628`)。可視・hover は即時に enqueue し、画面外の先読みは既存の
prefetch idle 制御で投入前に抑える。MP3 を取り出した heavy worker 内で idle / pause を
待たせず、同じ queue の Folder / ZIP まで停止させない。新たな通常操作の待機は設けない。
`GridItem::is_heavy_io` の Audio 分岐を sidecar / MP3 抽出対象の実 Audio に拡張し、全 enqueue・prune・品質更新 caller
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
DB/cache の identity は idx や current_folder ではなく、sidecar は画像 source path + stamp、
埋め込み art は元 MP3 path + stamp とする。sidecar 解決 / decode の前後にも取消を検査する。
worker で open 前、候補間、decode 前後、stat 前後、保存前、送信前に取消を確認する。
阻害不能な OS stat / 一回の decode 自体は途中で強制停止できないので、UI は join しない。
一覧切替・reload・close・drop・keep 外の取消は既存の context token / keep predicate を使う。
同じ bytes を使う別 context の token は取り消さない。取り消された結果は no-art として保存しない。

### 4.2 一つの終端表現

提案する状態は `Pending → Loaded / NoArt / Failed`、Loaded のみ `Evicted → Pending`。
実 Audio は source が未調査なら要求を作れる場合だけ Pending。sidecar のない非 MP3 は NoArt。
NoArt は「表示に使える同名画像も埋め込み画像もない」の正常終端で、無画像・候補が全部不正・
対応外・入力上限超過を含む。Failed はアクセス失敗 / timeout / runtime 障害の一覧世代内終端。
Failed を永続 negative cache にしない。次回の明示 reload / 一覧採用でのみ試し直す。
取消による現世代の keep 外は Evicted、旧世代の結果は破棄する。
埋め込み抽出の fresh stat 不一致も現世代では Failed として止め、watch / reload から新しい stamp を要求する。
sidecar は §3.3 の source path / image stamp、埋め込みは §5.3 の Audio stamp で検査する。
Loaded の origin / evaluated_display_px は既存の source / cache 規則へ統合し、取得した画像種別を
新しい has_sidecar / has_embedded bool に分散しない。
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

- **modal 化 (自動抽出)**: 一覧の裏方処理であり、スクロール・フォルダ移動・再生を止める退行になるので不採用。
- **一つの生成関数**: 採用。PC / 詳細 hover / Remote に別の抽出・選定・キャッシュ規約を持たせない。
- **既存 worker / reload を使う**: 採用。別 retry owner、progress dialog、live-rebuild manager を作らない。
  sidecar / 同名省略設定は既存 reload、表示マーク設定は painting の変更だけにする。
  表示中の全 context に専用 rebuild / 全 cache reset を追加しない。
- **再生中の demux から取る**: 不採用。再生開始前の一覧・Remote に使えず、seek / close と画像寿命が絡む。
- **全キャッシュを削除して新 schema にする**: 不採用。既存 DB は出荷済みで他媒体の行を失う。
- **indexer の ActivityGate 待機を共用する**: 不採用。heavy worker を待機で占有しないよう、
  既存の enqueue 前の prefetch idle 制御に揃える (R1-2)。
- **Remote の全成功画像を新 RAM cache に保存する**: 不採用。既存 HTTP cache を使い、
  DOM の寿命より長く必要な NoArt / Failed だけを一覧 owner に保持する (R1-5)。
- **Remote の更新 timer / 自動再検証を追加する**: 不採用案。更新保証は利用者決定 Q7 に従い明示 refresh に限定し、
  epoch を変えて期限や再試行 owner を増やさない (R1-6)。
- **album-art 専用の接続 LRU**: R2-1 で撤回。要求単位の接続と短い DB phase に限定し、
  idle worker ごとの解放 command / ACK / 再開状態を作らない。共通 catalog の削除境界は §5.4。
- **cache 削除で全 viewer / Remote を閉じる**: 不採用。表示・再生・navigation を維持し、
  catalog の受付と接続だけを共通 owner で退役させる。UI の modal 化や worker join は不要。
- **音声専用 sidecar map / 発見 worker / 設定**: 不採用。既存動画の source owner / filter /
  設定 / reload を一般化し、二つの採用規則と更新世代を持たない。
- **音声の手動画像指定と永続 pin**: 利用者訂正により範囲外。選択・保存・移行の状態自体を作らない。
- **Remote に別の音声マーク設定**: 不採用。本体設定の一覧 snapshot を使い、二重の設定 owner を作らない。
- **まれな cache DB 失敗を retry で救う**: 不採用。利用者決定 Q5 のログ・通知・次回再生成に限定する。

detached 述語・viewport routing・registry は変更しない。既存 bundle のサムネ状態として
NoArt を保持し、fork は結果を継承しても進行中要求を共有しない。
実装でこの境界を越える必要が判明した場合は止め、detached §2 の構造合意と §11 記録を先に行う。

## 5. Catalog・key・更新と出荷済みデータ

### 5.1 成功行の既存形式は十分、negative は追加が必要

この節の追加 catalog は **MP3 埋め込み画像だけ**を扱う。同名 sidecar は §3.3 の共通画像経路で
先に処理し、Audio の embedded row / absence を使わない。sidecar hit と sidecar のない非 MP3 は
追加 art schema を作らない。absence は APIC の不存在であり、同名画像の有無は保存しない。
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

新設する `ensure_audio_art_schema` (仮称) は **AudioThumbnail worker の入口だけ**から呼ぶ。
呼出境界はローカル `spawn_thumbnail_workers → process_load_request の Audio 専用 dispatch`
または Remote `pipe heavy handler → ThumbnailEngine の Audio 専用 dispatch` とする。
worker が source 親の catalog handle を開き、既存の一般 schema 確立を済ませた後に
album-art の追加 schema を確立する。R2-1 により **worker 所有 LRU は設けない**。
接続は要求内の DB phase に閉じ込め、lookup の値を所有 bytes / stamp として返して接続を閉じる。
抽出・decode 中には保持せず、保存 / prune では §5.4 の有効な受付証明を検査して worker が開き直す。
UI の catalog handle / mutex を追加 DDL のために借用せず、準備完了を待つのも worker だけとする。
UI に schema 準備待ちを追加しない。必要な schema が作れなければ art cache だけ使用せず、
source 取得へ進む (Q5)。
positive / absence の SELECT・保存・scope prune もこの worker 境界内に限定する。

成功行を既存 thumbnails に置くため、既存の UI 向け load_all / load_source_dims 等の一括 SELECT
では audioart namespace を SQL で除外する。先に全 BLOB を読んでから UI で filter しない。
既存の一般一括読込・掃除の呼出回数を増やさず、Audio の worker 専用 load_one / scoped query
だけが art 行を読む。UI の aspect / preview は worker の結果を受け、同期の Audio cache seed を
追加しない。cache 管理の全件・期限削除は既存の明示操作を保ち、接続・書込境界は §5.4 へ揃える。
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
worker は要求内の一つの DB phase でだけ同じ source 親 handle を再利用し、phase 終了で閉じる。
別 MP3 / 別要求へ接続を持ち越さない。UI の一括 matching map は art 行を持たず、
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
既存の一覧準備から scope ごとに一件の maintenance job を heavy queue へ渡し、
§5.4 の有効な受付証明で worker が実行する。専用掃除 thread / 毎 MP3 の再走査を作らない。
合成一覧はその表示行だけを完全 inventory とみなさず、source 親の complete snapshot がなければ
掃除を見送る。再生成可能な古い行は次の適格な物理一覧準備か明示 cache 管理まで残してよい。
取消・古い一覧の maintenance は保存と同様に実行 / commit しない。
facet / 検索で隠れた行を物理的な missing としない。

ファイル・フォルダ単位の明示 cache 削除も exact key / source scope に限定し、
共有 DB のファイル自体を消して別 drive の art を巻き込まない。
全件・期限削除の選定規則は保ち、実行は §5.4 の共通境界に置く。
smart/collection prepare、rename/move の cache consumer もこの境界を共有する。
rename/move 後は新 key で再生成し、
旧行は上記の scoped prune へ任せる。自動 art は再生成でき、sidecar は新しい同 stem を既存 scan で発見する。
利用者の video pin DB、rename / metadata transfer、タグ・★・collection migration は変更しない。

### 5.3 stamp・方針・再生成

**表示・ソート metadata と source stamp を分離する (R1-3)。**
`src/bookmark_browser.rs:564` の image_meta は MP3 mtime を bookmark.created_at_ms の秒へ
置換し、`src/app.rs:48125,90062` はそれを要求 mtime に渡す。
`reading_history_meta_for_entry` (`src/app.rs:89660`) は未取得 mtime / size を 0 にする。
これらの値を要求時の「原本 stamp」と扱う前提を撤回する。

AudioThumbnail の要求は Audio path / selected sidecar source / policy / context 相関を持つ。
まず共通 source owner の sidecar を処理する。使用可能な sidecar がない MP3 だけが
以下の埋め込み art の scope / stamp 規約へ進み、stamp の正本は
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
同時編集が同じ秒・同じ size の場合はこの規約だけでは検出できないため、その限界は利用者決定 Q6 (2026-10-08) に従う。

成功・absence の永続化とも既存 CacheDecision に従う提案。
Off は永続書込なし、Always は確定結果を書込、Auto は元 MP3 size と抽出・decode・縮小時間で
判定する。元の無画像結果でも調査時間を測る。Auto で保存しない場合も NoArt は
一覧世代内に残るため idle loop は生まれない。本体は既存 ThumbnailState を終端 owner とする。
Remote の DOM 外の終端 owner は §7.2 で定義し、永続 cache の保存有無に依存させない。
CacheOnly miss は「未調査」のまま source を開かず、absence は捏造しない。
SourceOnly の明示再生成は埋め込み成功・absence を両方迂回し、sidecar が選ばれた場合も画像 cache を
迂回して元画像を読む。優先順位は変えず、cache 削除で同名の元画像を削除しない。
埋め込み画像へ戻す場合は同名画像を取り除く / 別 stem に改名するか、共通 sidecar setting を OFF にする。

watch の stamp 変更 / reload / cache 削除 / algorithm version 変更で再調査する。
成功画像の品質 upgrade は既存 origin / evaluated_display_px の規則を使い、NoArt は対象外。
Remote が小さい画素を要求しても、大きい既存 catalog row を縮小行で置き換えない。
保存は設定された thumb_px で生成し、HTTP 出力だけ要求寸法へ縮小する。
アートなしは target_px に依存しないため absence key にサイズを含めない。

### 5.4 全件・期限削除と接続 / 書込の境界 (R2-1)

`src/app.rs:35623` の evict_all_catalog_cache は App の LRU だけを clear する。
`src/ui_dialogs/cache_manager.rs:120,224` の削除入口はこれを呼ぶが、
`src/app.rs:46530` の thumbnail worker は空 queue で Condvar 待機し、終了しない。
`src/catalog.rs:1659,1672` の delete_old_cache / delete_all_cache は remove_file の成功だけを数える。
したがって「App の LRU を解放すれば全接続が閉じる」という前提を置かない。
Remote の既存 `generate_catalog_resolved` (`src/remote_ipc/thumbnail.rs:336`) も実行中は
CatalogDb を保持する。これは通常の所有権の問題であり、Q5 のまれな DB 障害には含めない。

**アートの常駐接続をなくすだけで終わらず、同じ DB を開く共通 catalog に削除境界を置く。**
本体 process / cache_dir に一つの `CatalogAccess` (仮称) を共有し、App と全 viewer context、
通常 / 合成一覧の prepare・thumbnail・writeback、Remote engine、cache-maint に同じ owner を渡す。
remote-web は DB を開かず、この owner は本体内だけに置く。別 IPC command は追加しない。
実装では CatalogDb の全 open variant (通常・folder selection・read-only) と全 SQL caller を棚卸しし、
この経路を通らない接続を残さない。別 cache_dir の owner は共有せず、設定・タグ等の DB は対象外。

owner が持つのは型付きの `Accepting(epoch) / Deleting(epoch, operation)` と DB phase の lease 数、
登録済み接続の weak registry (登録 / maintenance 時に dead entry を除去)。
App に deleting bool / audio pending bool を足さない。
MP3 要求にはこの owner が発行する `Admitted(epoch) / DisplayOnly(epoch)` の受付証明を一つ持たせる。
一覧 / session の取消世代とは役割が異なり、source stamp・HTTP artEpoch・永続 key には含めない。
Audio の SQL phase は受付証明を検査して lease を取得し、open / schema / SELECT / transaction / close を
worker 内で完了する。Connection / statement / transaction を phase 外へ貸し出さない。
close と rollback を含む handle 解放の後でだけ lease を返す。owner の状態 mutex は短い memory 更新
だけに使い、I/O / decode / connection mutex の取得中には保持しない。
一般媒体の既存 SQL も lease で drain 対象に含めるが、既存 UI 同期経路の全面移設をこの文で
暗黙に要求しない。新しい Audio の SQL / close は UI に到達させない。

削除の順序は次の一経路へ集約する。

1. **受付失効**: 既存 spawn_cache_maintenance の削除受付で、worker 起動前に memory 上の epoch を
   進めて Deleting にし、既存 App LRU の warm hit を失効させる。現行の UI 入口の
   evict_all_catalog_cache → spawn の順序を変更し、接続の close / LRU の実解放は worker の退役後に
   行う。UI は要求提出だけで SQL・drain・join をせず、新しい SQLite close を増やさない。
2. **drain / retire**: 既存 cache-maint worker が Condvar で active DB phase の完了を待ち、
   登録された対象 catalog 接続を worker 上で閉じる。長い source 抽出・decode は lease を保持しない。
   既存 worker が持つ Arc が生きていても Connection は残さない。CatalogDb 内は
   `Open { connection, epoch } / Retired` の一つの状態にし、Retired handle は再 open しない。
   App の current_color_catalog、parked context、prepare 結果、virtual writeback、既存 regular / heavy
   worker、Remote の実行中 handle も登録対象。idle worker を起こして ACK を集める方式は使わない。
3. **選定 / 削除**: open / SQL の新規受付を閉じたまま、現行と同じ cache_dir / 日数・DB mtime の
   条件で選定し、全件または期限対象の DB を remove_file する。DeleteOld は選定直前の mtime を使い、
   対象外の DB ファイル・行は保持する。DB に成功・absence が同居するのでどちらも削除対象に含まれる。
   全件・期限削除は cache_dir 全体を短期間この境界に置き、DB ごとの並行削除状態は作らない。
   フォルダ / ファイル削除は同じ接続・書込境界の中で §5.2 の exact key / source scope を使う。
4. **再受付**: 削除完了後に新 epoch の Accepting を公開し、既存 pending / completion 経路で結果を
   返す。UI は退役済みの App LRU を memory 上で clear する。App の古い handle は warm hit にせず、
   次の適格な worker lookup が新 handle を得る。
   拒否された古い要求を自動 retry せず、既存の明示 reload / refresh と次の新要求を使う。
   エラー / worker spawn 失敗 / unwind でも maintenance token の drop で受付を再開し、失敗を既存結果へ
   返す。削除完了を偽装する通知や追加 retry / 復旧 journal は作らない。

**削除をまたぐ書込**: 受付失効前に lease を得た transaction は完了まで drain してから削除する。
開始済み SQLite transaction を途中で強制停止しない。まだ書込 lease を得ていない旧 epoch の
要求は保存・schema 作成・prune を行わず、削除後に DB を再作成しない。lookup を終えて抽出中の
ローカル / Remote MP3 も同じである。結果は有効な表示 context / session に返せるが、DB へ書き戻せない。
削除中に届く MP3 は DisplayOnly として source から生成し、永続化をせず終了する。新しい epoch で
削除完了後に受け付けた要求だけが、CacheDecision に従って DB を再作成・保存できる。
Remote Flight の key に受付証明 (epoch と Admitted / DisplayOnly の区別) も含め、
削除後の新要求を旧要求や削除中の DisplayOnly Flight に合流させない。

この境界は既存一般媒体の SQL にも届く。Retired handle の SELECT は cache miss、書込は無効な
世代として扱い、古い Arc から接続を復活させない。UI の既存 catalog 経路は Deleting 中に待たず、
保持中の表示を使って必要な lookup を worker に渡す。一般 worker の catalog-only 要求は、
実行中に古い handle が Retired になった場合も含め、worker 側の Condvar で再受付を待ち、
open_existing_read_only による一回の lookup を現在の読取 lease で行う。期限外の DB が残れば
その cache を使い、実際に削除済みの DB は通常の cache miss とする。古い要求の書込許可は更新しない。
これは待機中の同じ lookup の継続であり、新しい抽出要求の自動 retry や再試行 loop ではない。
source 読込に勝手に格上げしたり媒体を表示不可にしたりしない。
待機中は DB phase lease を持たず、取消で抜けられる。画像 / ZIP / PDF / RAW・詳細 hover・再生の
既存機能を落とさず、共有境界を独立した挙動維持 chunk として設計再レビューしてから実装する。
削除失敗は実際の残存件数とエラーを既存結果へ反映し、成功件数だけから全削除成功と推測しない。
この通常の同期境界を Q5 に押し込まず、外部ロック等のまれな I/O 障害に限って Q5 を適用する。

## 6. Audio セルを表示する全 surface

| 一覧 / producer | 実装で揃える点 |
| --- | --- |
| 通常物理フォルダ / filter・facet / reload | folder_scan → install_new_items_inner → make_load_request。実 Audio の共通 sidecar 出所を要求へ渡し、MP3 の自動 art は source 親 catalog に一致させる |
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
そこも §6.1 の同じ描画決定に揃え、切り取り時だけ設定に反して音符が復活しないようにする。
★・タグ・長さ・再生位置バー・選択・切り取り opacity / ハサミは既存 overlay layout を保つ。
画像上の Audio 識別表示は利用者決定 Q3 の §6.1 に従う。

詳細一覧では **左端の種別アイコンは Audio のまま**。既存 preview 列 / 選択情報バーの設定を尊重し、
画像表示を有効にしている面では同じ Loaded を使用する。hover は既存の
`set_details_hover_thumbnail_idx` の keep / priority 経路で要求し、hover 内で decode しない。
sidecar のある非 MP3 も Loaded を表示する。NoArt / Failed は音楽アイコンを表示して終端化し、「読み込み中...」repaint を止める。
hover を離れても他の可視 Audio の要求を取り消さない。

Auto サムネイル比率では sidecar / 埋め込み画像の寸法を統計候補にする (利用者決定 Q3)。現在の collection の
Audio 除外 eligible 数と、一般一覧の適格性 / dimensions の consumers を揃える必要がある。
NoArt の件数は終端として集計し、寸法が来るまで永久に確定を待たない。
PageDims に入る場合も Audio の has_page_data を変えず、画像編集・見開きページ数へ伝播させない。

### 6.1 音声表示マークの設定 (Q3 決定)

既存 VideoThumbnailIndicator (`settings.rs:617`) に沿って独立した
`AudioThumbnailIndicator { MusicNoteIcon, BottomLeftBadge, Hidden, Unknown }` (仮称) を追加する。
設定 field は `audio_thumbnail_indicator`、`#[serde(default)]`、既定 MusicNoteIcon。
`all / label / normalized` を同じ enum に持たせ、`#[serde(other)] Unknown` は sanitize で
MusicNoteIcon に正規化する。旧 settings の key 不在・未知値でも起動でき、動画の設定値を流用しない。
Settings::default / Settings::sanitize、settings DB 読込・保存・draft へ揃える。

| 選択肢 | Loaded (sidecar / 埋め込み共通) | Pending / NoArt / Failed |
| --- | --- | --- |
| 音楽アイコン (既定) | 動画の play icon に相当する音符を画像上に描く | 従来の fallback 音楽アイコン |
| 左下バッジ | 文字「音声」のみ。画像上の音符は描かない | 同じ fallback アイコン |
| なし | 音符・音声バッジのどちらも描かない | 同じ fallback アイコン (空白セルにはしない) |

設定は **画像があるときの識別マーク**を切り替えるもので、画像自体や無画像 fallback は隠さない。
左下「音声」は動画「動画」と同じ bottom_left_content の場所を使い、長さ / resume bar と合成する。
詳細一覧の左端の種別アイコンは常に Audio のまま。grid・切り取り後段・preview / hover は一つの
`audio_thumbnail_indicator_parts` (仮称) を使う。描画だけを変更し、art 再抽出 / texture 失効はしない。

`settings_transfer.rs:269` の動画 field の隣に **plain / plain の持ち運べる表示設定**として分類する。
import の未知値は video と同じ validator で issue を報告して採用せず、通常 settings load の
Unknown 正規化とは区別する。全 field の分類・異なる値 fixture / round-trip の coverage を更新する。

環境設定「サムネイル」で動画設定の直後に音声設定を置く。別 combo ID と検索 anchor
`thumbnail/audio-indicator`、音声 / 音楽 / マーク / バッジの検索語を追加する。
動画にだけ作用する説明を保持し、音声の説明には同名画像と埋め込み画像の両方に作用すると記す。
現在動画 helper 内にある共通の duration checkbox は両設定の後に一度だけ配置する。
headless snapshot で両設定の隣接・3 択 label・検索 anchor と、全 Audio 描画経路を検証する。
Remote も本体の同じ設定に従う。一覧 payload とブラウザへの反映は §7.4。

## 7. mIV Remote を初回から含める

### 7.1 endpoint と本体生成

既存 `/api/thumb` の RemoteAddress { Audio logical path, File } を使う。
新 album-art endpoint、APIC bytes の wire 転送、RemoteEntryKind の新種別は不要。
全対象は Audio のまま。sidecar があれば既存の source_address に同名画像を渡し、
元 address は Audio path を保つ。source_address の許可を実 Audio へ拡張し、既存の
path guard / 認証 / session admission / same canonical parent・stem・認識画像の検査を共用する。
別親・別 stem・archive / URL・任意画像への差替えは拒否する。sidecar がなければ source_address は None。

ThumbnailEngine は image/video/container の dispatch に実 Audio の AudioThumbnail を追加し、
§3〜5 の本体共通生成関数へ渡す。RemoteThumbnailSources / source_address の解決を動画・音声で
共有し、sidecar は既存画像の生成経路、なければ MP3 の埋め込みを生成する。
非 MP3 は sidecar があれば WebP、なければ NoThumbnail。catalog、選定、limits、
CacheDecision、WebP を PC と共有する。新しい編集・upload endpoint、user pin DB は作らない。
folder listing / RemoteEntry の thumbnail_address と `thumbnailRequestQueryForEntry` の
thumbnail_source_path をそのまま Audio に使い、remote-web が sidecar を自分で探索しない。
settings の正本は既存 `settings_for_listing` / RemoteListingSettings の live snapshot。
共通 resolver は一要求で両 field の現在値を読み、起動時の ThumbnailEngine.settings だけで判定しない。
OFF 後に古い source hint が届いても採用せず、MP3 埋め込み / アイコンへ進む。動画にも同じ検査を使う。
候補選定は採用済み一覧の共通 source snapshot に一本化し、複数候補のとき PC と別の
音声専用選定をしない。endpoint は hint を same-parent / stem / image guard で検証する。
サムネイルごとの再走査は追加せず、親を走査するのは既存の一覧準備 / 再取得だけとする。
Audio 原本の NotFound と、検証済み同名画像が取得前に消えたケースを区別し、後者は §3.3 の
埋め込み fallback へ進む。外部 path / 認証違反を fallback で隠さない。
NoArt は既存 ThumbnailErrorCode::NoThumbnail、他の失敗は既存 error code に写像する。
absence cache hit も同じ終端応答。HTTP の 422 は再試行なしだが、終端種別は §7.2 の
error 識別子で判別する。画像 bytes は従来の image/webp。

#### 永続コレクションの出所伝達 (R7)

通常の RemoteEntry とは別に、永続コレクションは `wire_entry` が
`PersistentCollectionEntryState::Available { thumbnail_address: None, .. }` を作る
(`persistent_collections.rs:1530,1566`)。RemoteThumbnailSources の媒体拡張だけでは届かないため、
**snapshot と navigation replacement の共通 facts 準備に discovery と出所付与を接続する。**

- `view_facts` (`persistent_collections.rs:310`) で、prepared entries のうち現在の原本 / Remote
  policy 検査を通る Available Video / Audio の実 path を共通 discovery に渡す。
  RemoteThumbnailSources の入力を path / kind と継続判定から受け取れる形へ一般化し、
  通常 RemoteEntry と永続コレクションで同じ §3.3 の discovery / 候補選定を使う。
  ダミー RemoteEntry の作成、永続コレクション専用の再探索や source map owner は追加しない。
- discovery は既存 IPC worker の一覧準備中に一回だけ行う。両設定の現在値、Video 優先・
  Audio と共有する 64 親上限、request lease / cancellation / deadline を渡す。
  現行 `for_remote_entries` の無条件 `|| true` を永続コレクションへ流用しない。
  取消 / 期限切れは既存の interrupted 応答とし、途中の source snapshot を公開しない。
  scan error / 上限外は §3.3 の source なし fallback とし、サムネイル endpoint では再探索しない。
- `stream_bounded_wire_entries` の producer で既存 `wire_entry_with_epub_policy` による分類を終え、
  **Available Video / Audio にだけ** `source_address` の結果を `Available.thumbnail_address` へ付与する。
  元 address / kind、entry_id / source_identity、手動順序と明示画像行は保持する。
  Missing / AccessError / Unsupported / BlockedByRemotePolicy はそのままにし、画像で Available に戻さない。
  sidecar がない / 設定 OFF なら None。path guard は §7.1 の共通契約を保つ。
- 出所付与は retained bytes の計算と `token_digest` の wire entry 消費 **より前**に完了する
  (`persistent_collections.rs:350`)。thumbnail_address も既存 frame 上限と view token に含め、
  `wire_snapshot` の出力後に追加しない。出所 address の変更は新 facts の token に反映される。
  `PersistentCollectionViewFacts.entries` が出所付き entry の唯一の保持先となり、
  snapshot と `bounded_landed_response` の replacement は同じ `wire_snapshot` からこれをコピーする。
- `navigation_cache_candidate` / `navigation_view_facts` の既存 facts 再利用を保つ。
  `PersistentCollectionViewKey` に両 sidecar 設定値を含め、OFF 等への変更で旧出所を再利用しない。
  順序用 prepared snapshot の再利用は残し、新 facts を準備して必要な replacement へ渡す。
  同じ owner / revision / key の navigation では再走査せず、明示 refresh は従来どおり新 facts と
  source snapshot を作る。外部追加 / 交換 / 削除は §7.2 の refresh / artEpoch 契約に従う。
  別の source cache、定期探索、追加 generation は作らず状態の組合せを増やさない。
- 既存 wire の optional `Available.thumbnail_address` を使い、新 field は不要。
  Web の `normalizePersistentCollectionEntry` (`app.js:4921`) が通常 tile へ出所を渡し、
  `thumbnailRequestQueryForEntry` (`app.js:7307`) が thumbnail_source_path を作る。
  元 Audio の open / navigation / resume 対象は変えず、画像をページ / navigation 対象に追加しない。
  IPC 版更新と音声表示マークは本計画の既存方針を維持する。

現コードの ThumbnailEngine::handle は session_cancel を受け取らず、
generate_catalog_resolved 内の token は常に false。新抽出では pipe heavy handler に既にある
RemoteOperationCancellation を helper へ渡すことを必須とし、session release / takeover / shutdown
で source 読込を止める。Flight の結果待ちは既存 Condvar、UI thread を待たせない。
Flight を共有する waiter と owner は同じ session 世代の要求に限定し、session identity を
RequestKey に含める。既存 source_address も Flight identity に含まれているため、別 sidecar source
の要求を混ぜない。HTTP artEpoch は IPC identity に渡さず、完了 Flight は handle 終了時に除去する。
新要求は採用した source と live settings を照合し、source / stamp の途中変更は公開しない。
旧 session の取消結果を次 session の Audio 要求へ返さない。
PC と Remote の in-flight token は共有せず、catalog の確定結果だけを再利用する。

ブラウザの AbortController は DOM / fetch を止めるが、既存 Thumbnail wire には
個別 cancel command がない。tile 離脱だけで core の I/O が即止まるとは主張しない。
旧 DOM への採用を拒否し、core の MP3 抽出 job は 10 秒 / 入力上限内で終了させる。
sidecar は既存画像 worker の decode / RAW / plugin 契約を保ち、全形式の途中停止を保証とは書かない。
session 取消は別途伝播する。個別 Thumbnail cancel IPC は今回新設しない。

### 7.2 Web UI と cache

createGridTile に実 Audio の img と音楽アイコン fallback を両方置き、既存 thumbnail binding /
virtualization / request limiter / binding generation を使う。非 MP3 も sidecar 表示を要求する。
成功時だけ fallback アイコンを隠し、設定に従う画像上のマークを描く (§6.1 / §7.4)。
NoThumbnail / source error では fallback を残して tracker を settled にする。
NoThumbnail は「対象ファイルが missing」の 404 と区別し、tile の open を disabled にしない。

**終端 owner は DOM ではなく、採用済み一覧の世代に置く (R1-5)。** 現行の
`app.js:8947` の image._thumbnailSettled は binding 作成時にリセットされ、
VirtualGrid は `app.js:9741,9766` でセルを破棄・再作成する。既存 binding だけでは不十分。
新しい `AudioArtListState { generation, artEpoch, terminalByAddress }` (仮称) を一覧 owner に置き、
terminalByAddress は正規化した Audio logical address に `NoArt | Failed` だけを保持する。
画像 bytes / 成功 cache / DOM reference は保持せず、同じ Audio の重複 bookmark は同じ結果を参照する。
容量は現一覧に含まれる unique Audio address 数を上限とし、LRU eviction で終端を忘れない。

初回要求は既存の有界 retry / limiter を使う。**NoArt は HTTP 422 かつ JSON の
`error: no_thumbnail` の応答だけ**とする (R2-2)。source error・
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
このエラー識別に新しい IPC error enum は不要で、http.rs の error mapping と app.js の表示 / telemetry を揃える。
表示設定の wire enum 追加 (§7.4) は別の変更である。
`http.rs:4297` では GenerationFailed / NoThumbnail が両方 422 なので、status 単独では判別しない。
HTTP mapping は NoThumbnail のみ no_thumbnail とし、GenerationFailed は既存 miv_thumbnail_error の
まま Failed にする。他の 422 (識別子不明・JSON 不正を含む) も Failed として終端化し、
正常な無画像の map 値 / telemetry に混ぜない。永続 absence に保存するのも本体 NoArt だけである。
通常の無画像を client image_load_error として大量記録しない。既存 retry は network/busy のみで、
どちらの 422 も再試行せず終端とし、no-store を維持する。
auth / session 失効は既存の通信・ログアウト動作を維持する。

Remote の通常・検索 / tag・rating・history・smart・永続 collection・bookmark で
RemoteEntryKind::Audio の全実 Audio を同じタイルに渡す (sidecar は MP3 に限らない)。catalog / UI の一覧ごとに別フラグを足さない。
原本と同じ logical address を保ち、remote-web が album-art の寸法から kind=image と推測しない。

**「外部編集後、最大 60 秒で更新」の保証を撤回する (R1-6)。** 成功応答は
`http.rs:3601` の private, max-age=60 だが、取得側は `app.js:9126` の force-cache。
[Fetch Standard の cache mode](https://fetch.spec.whatwg.org/#concept-request-cache-mode) は
force-cache で期限切れの一致応答も使うと定める。さらに `app.js:1893` の
remoteSessionCacheEpoch は session identity 変更時の更新であり、一覧再描画で変わる前提は成立しない。

**更新保証は明示 refresh に限定する (利用者決定 2026-10-08、Q7)。** 通常の取得は既存 force-cache と
max-age=60 を保持し、timer / polling / 自動再検証は追加しない。既存 remoteSessionCacheEpoch は
session 用のままとし、新しい Audio を含む一覧 payload の採用時に非秘密の artEpoch nonce を作る。
`/api/thumb` の Audio URL に artEpoch を追加し、同じ payload の再描画では変えない。
これは HTTP cache の URL 区別だけであり、IPC の address / catalog key には含めない。
一覧再取得の採用では generation と artEpoch を同時更新し、terminalByAddress も空にする。
明示 refresh の確実な既存操作は **ブラウザのページ再読込**。一覧内の refresh 操作がある経路も
再取得・採用へ揃えるが、新しい refresh ボタンが既にあるとは扱わない。
新 URL が旧 HTTP cache を迂回し、core worker が §5.3 の実 source stamp を再確認する。

受入条件は、外部でジャケットを交換・削除・追加して stamp が変わった MP3 が、明示 refresh 後に
新画像または NoArt へ更新されること。refresh 前は 60 秒を超えて旧表示が残り得ると説明する。
同じ秒・同じ size の編集では refresh だけで永続 cache を迂回できず、Q6 の削除 / SourceOnly が必要。
同名画像の追加・交換・削除と共通 setting の変更も、開いた Remote への即時 push は追加せず
明示 refresh 後に新しい出所と設定を採用する。Audio stamp が同じでも、sidecar の image stamp が
変われば画像経路で再取得する。同名画像の存在が新旧で変われば埋め込み absence の再利用より先に解決する。

### 7.3 IPC 版

基準は **コード v66**。旧 core が音声生成を拒否する意味契約の変更に加え、
§7.4 の一覧 presentation field / enum の wire 追加があるため **IPC 版を上げる**。単独で次なら v67 だが、他ラインの統合と合わせて coordinator が番号を一つに決める。
この設計作業では定数も assertion も変更しない。

実装時は `crates/remote-ipc` の定数・handshake / serialization の版 assertion と、
web-remote-plan §13.5 の履歴・現行表記を同時更新する。本体・remote-web を両方 rebuild / restart
することが必要。版不一致は従来どおり拒否し、旧 core へ黙って対応しない。
NoArt / Failed の内部状態は wire に追加しない。presentation の新しい field / enum と
全 payload producer / reader を実装 brief に列挙し、統合版の serialization を検証する。

### 7.4 同名画像と表示マークの Remote parity

`app.js::createGridTile` は現在 Audio に img を作らず、binding も除外する。実 Audio 全形式へ
img / fallback を付け、§7.2 の一覧 owner・terminalByAddress・取消を共通に使う。
sidecar / embedded の bytes だけで kind=image に変更せず、duration・resume・操作先は Audio のまま。
画像が Loaded のときは §6.1 の note / 文字「音声」/ none を描き、fallback の音符は必ず残す。
現行 Remote の常時 type-badge を音声についてこの同じ決定へ揃え、note + badge の二重表示を避ける。

一覧 payload に一つの `thumbnail_presentation.audio_indicator` (仮称) を載せる。
wire は `MusicNoteIcon / BottomLeftBadge / Hidden` の媒体表示値だけで、entry ごとの bool は追加しない。
FolderListPayload、CollectionPayload (検索 / tag の内包 listing も含む)、
PersistentCollectionSnapshotPayload と、その HTTP mapping / JS の一覧採用を揃える。
共通 grid に渡る ContainerPayload の presentation 受渡しも同じ型にし、Audio を新たに列挙はしない。
未知 / 欠落値の web 防御的 default は MusicNoteIcon。ただし IPC handshake の版不一致拒否は保持する。

本体の **一覧取得時の現在設定**を worker が snapshot 化する。起動時の ThumbnailEngine.settings の
固定 Arc を設定の正本にはしない。既存 CollectionSettingsSource::Live (`collections.rs:33`) と、
ContainerEngine::settings_for_listing (`container.rs:2605`) の live overlay を使い、
`settings_db::RemoteListingSettings` の field / SQL key / apply / normalize に音声設定を追加する。
永続 collection の prepare にも同じ設定 snapshot を渡す。新しい browser ローカル設定は作らない。
本体で設定変更後は Remote の明示 refresh / 新一覧採用で反映する。既存リストへの polling / push は
増やさず、単なる sort / DOM 再構築は同じ presentation snapshot を保持する。
本体側は次の painting で更新し、Remote 更新契約は Q7 と同じ操作境界に揃える。

## 8. 利用者への質問と決定状況

**未決の利用者質問はない。** Q1 は 2026-10-08 の訂正を正本とし、Q2〜Q7 は既決事項のまま (§1)。
初版は同名画像 → 前面表紙優先の MP3 埋め込み → アイコン。手動画像指定の質問・操作・DB 設計は撤回した。
「動画と同じ」は二つの既存設定に従う sidecar 採用・画像行省略を意味し、利用者の OFF 設定を引き継ぐ。

## 9. 実装順序・受入条件・検証所有

1. coordinator は Q1 訂正と Q2〜Q7 の決定を引き継ぎ、R6 の共通 sidecar source・一覧省略・
   既存 setting の互換・画像 stamp / cache・Remote path guard を重点として変更境界をレビューする。
   R7 の永続コレクション snapshot / navigation replacement の出所伝達も再レビューし、
   R2 の catalog maintenance 境界・422 分類と、R1 の 6 件の変更境界を実装 brief に含める。
   extraction の有界性、terminal 契約、親 catalog、Remote session Flight も維持する。
2. 合意後の最初の作業は §3 の synthetic ID3 fixture と同梱 FFmpeg adapter 検証。
   front / back の選定 metadata、open-only、取消 / 上限が成立しなければ設計を戻す。
3. 型付き結果契約と NoArt の共通 consumer を挙動不変の chunk で整え、source catalog の
   追加 schema / helper、共通 sidecar source / discovery、Audio 要求 / 全 surface の UI、
   表示マーク設定 / transfer / snapshots、Remote を同じ feature の完了範囲にする。
   「PC だけ先に完成」で項目を完了扱いにしない。
4. 実装者が以下の自動検証を所有し、coordinator は有効な結果を再利用する。

| 層 | 必須の受入テスト (今は未実行) |
| --- | --- |
| extraction | ID3v2.3 JPEG (要望素材相当の synthetic)、v2.4 PNG、v2.2 PIC、front/back 逆順、複数 front、壊れた front と正常な別画像、タグなし、URL APIC、unsynchronization、extended header、切れた tag、巨大 tag・dimensions・候補過多、期限 / cancel |
| state / routing | 実 Audio の sidecar / MP3 要求が作られる、sidecar のない非 MP3 / NoArt で requested=0 / repaint=0 / upgrade=0、cancel 後に NoArt を保存しない、timeout で再投入しない、late message / upload backlog / Finalized の既存挙動、Loaded eviction と再表示 |
| worker 境界 (R1-1/2) | 通常 / 合成一覧の UI 呼出を計測し、追加 schema / art SELECT / absence / scoped prune が UI に 0 回、UI bulk query が art BLOB を読まない。ActivityGate pause / 連続入力中でも可視・hover を gate 待機させず、同じ heavy queue の Folder / ZIP も進む |
| source stamp (R1-3) | bookmark 登録日時≠原本 mtime、同一 MP3 の異なる日時の複数 bookmark、history の未知 stamp 0 / 表示 meta None でも worker stat で Loaded / NoArt になる。stat 失敗で stamp 0 の cache 行を書かず、読込中の実 stamp 変更は公開・保存しない |
| catalog | 旧 v2 DB の画像 / ZIP / PDF / video_meta 行不変、追加 table 不在の read-only miss、positive↔negative、mtime だけ / size だけの変更、同名別親、Auto / Off / Always、SourceOnly、prune / rename / clear、small Remote が large row を上書きしない |
| prune 所有 (R1-4) | 同一 DB を共有する C:\Music / D:\Music を交互に物理一覧で開いて両方の positive / negative を保持。片親で消えた MP3 だけを両 table から scoped prune、別 drive / 子親 / facet 非表示は保持。一般 delete_missing も art 行を消さず、合成一覧・未完走 scan・旧世代は prune しない |
| cache 削除 (R2-1) | Windows の disposable cache_dir でローカル / Remote の MP3 閲覧後、queue が空の worker と保持 Arc を残して DeleteAll が positive / negative の DB を消す。DeleteOld は古い DB だけ消し、期限外を保持。旧 epoch の保存待ち・prune / lookup 中・実行中 transaction を同期 fixture で削除の前後へ動かし、handle 解放前に remove_file せず、旧要求が削除後に DB を再作成しない。削除中の DisplayOnly は表示を完了し永続化 0、新 epoch の次要求は必要時だけ再生成。通常 / parked / prepare / writeback / read-only / Remote の全 open variant、catalog-only の待機 / cancel、spawn 失敗時の再受付も検証し、製品 binary は起動しない |
| cache 削除中の機能維持 | 期限外 DB の catalog-only 要求が Retired を受けても、一回の背景 read-only lookup で既存画像を返し、旧書込許可は復活しない。Remote は同じ address / target でも Admitted と DisplayOnly の Flight を混ぜない。UI の drain / join / 新しい SQLite close は 0、source 表示・再生・別 context の texture は維持 |
| cross-context | A / B の同じ・異なる MP3、片方の switch / close / cancel / reload で sibling texture / queue / NoArt 不変、park / restore / fork。製品起動を要さない fake worker / state tests |
| UI | 全 surface の MP3 / 非 MP3 sidecar Loaded・埋め込み Loaded / NoArt / Failed、詳細 hover・preview・選択バー、切り取り、暗 / 明 theme、duration / resume / badge、Auto aspect の終端。headless snapshots を追加する |
| Remote | endpoint の認証 / path guard、MP3 WebP / NoThumbnail、PC 未訪問の cache miss、全 Audio 一覧、422 非 retry、terminal tracker、stale DOM、virtualization、session cancel / next owner / 同一 session Flight、版 handshake・round-trip。Rust handler と Node runtime tests |
| Remote 終端 owner (R1-5) | Cache Off / Auto absence 未保存で NoArt / Failed を受信後、DOM eviction→再訪しても同じ一覧世代の HTTP / core 抽出は 0 回。重複 address・sort / resize でも保持、refresh / 別一覧で破棄。旧世代完了は新 map に入らず、意図的 abort / session 失効は結果を汚染しない |
| Remote 422 分類 (R2-2) | Rust HTTP mapping と Node runtime で 422 + no_thumbnail → NoArt、422 + miv_thumbnail_error (GenerationFailed) → Failed、他の 422 / JSON 不正 → Failed。全部 retry 0・tracker settled、DOM 再訪の再要求 0。NoArt だけを absence に保存し、生成障害は normal-no-art telemetry と区別 |
| sidecar discovery / 行省略 | song.mp3 + song.jpg / 非 MP3、同 stem の複数 Audio / Video、大小文字、同名別親 / 別 drive、複数画像候補が video と同じ順、RAW / Susie / codec 条件。両設定の 4 組合せで source 選択と画像行省略を照合。物理 / smart は省略、aggregate の明示画像結果は保持。64 親の共有・取消・上限外・video の既存候補保持、サブ展開で Audio も画像省略も増やさない |
| sidecar invalidation / cache | 埋め込み absence hit の後に画像を追加しても先に sidecar を表示。Audio stamp 不変で画像のみ交換 / 削除 / rename、読込途中の画像 stamp 変更、hidden sidecar の scan signature 変更、両 setting の既存 reload。画像 cache と embedded positive / negative を混ぜず、SourceOnly / Off / Auto / Always / 全件・期限削除を共通 cache 境界で処理。picker / user DB / metadata transfer 変更なし、既存動画 pin と Shell 挙動を保持 |
| Remote sidecar / indicator | 元 Audio address + thumbnail_source_path、非 MP3 の画像、source None の MP3、OFF と古い source hint、same canonical parent / stem / File / 認識画像の guard、PC と同じ候補の検証、相違 source の Flight 分離。refresh 後の追加 / 交換 / 削除、全 Audio 一覧、全 payload の presentation round-trip、live 設定→新一覧、3 択 tile snapshots / fallback / handshake |
| Remote 永続コレクション (R7) | FLAC + 同名 JPEG と MP3 + 同名画像を snapshot / navigation replacement の両方で Available.thumbnail_address へ渡し、Web 正規化→thumbnail_source_path→WebP を検証。元 Audio address / kind / entry identity / 手動順序 / resume、明示画像行を保持。同名別親は混ぜず、Missing / Blocked 等には付与しない。sidecar なしは MP3 埋め込み / 非 MP3 NoThumbnail。両設定の 4 組合せと変更後の facts 再構築 / replacement、同 key navigation の再走査 0、refresh 後の画像追加 / 交換 / 削除を検証。Video と共有する 64 親上限、取消 / deadline 時の未公開、出所付与後の frame byte 上限・view token 変化を Rust handler / wire と Node runtime で検証 |
| released setting compatibility | video_thumb_use_sidecar_image / skip_image_if_video_exists の旧 true / false と key 不在、field 名不変、既定 true、settings DB と transfer の既存 bool / plain 分類の round-trip。OFF を音声だけ true に戻さない、preferences の label / 検索 / 説明 / snapshot、manual の二設定の組合せを更新 |
| audio indicator | key 不在・default・Unknown sanitize、settings DB round-trip、transfer plain 分類・未知値 issue、動画値との独立性。headless snapshots: 3 択 × sidecar / embedded / fallback、cut・明暗・小セル・duration / resume / badge、hover / details、環境設定で動画との隣接と検索 anchor |
| Remote 更新 (R1-6/Q7) | fake HTTP cache で 60 秒超の force-cache 再利用を再現し、refresh 前の自動更新を要求しない。stamp が変わった外部交換・削除・追加後、明示 refresh が新 artEpoch URL で core へ到達し、新画像 / NoArt になる。同 stamp は Q6 の限界として別検証 |
| playback 回帰 | cover-only MP3 は audio-only のまま、cover + 実映像は実映像を選ぶ。decoder の既存テストを保持し、ジャケット取得が player / 音声出力を作らないことを検証 |

既存固定アイコン仕様を変える feature なので、今回の設計作業に bug-fix red はない。
実装時に既存不具合も直す場合は、その違反境界の failing regression を修正前に実行し有効な red を残す。
targeted → full lib `cargo test -p mimageviewer --lib` (pipe なし・実 exit code)、
`cargo fmt`、normal / portable の core check、`python scripts/check_ui_glyphs.py` を行う。
共有 ThumbMsg / catalog maintenance 境界の変更では [build/test policy](development-build-and-test.md) の full gate も必要。
Remote IPC / Web tests を省かず、CI 依存・timeout は十分な時間を確保する。

実装後は build-dev.ps1 で通常 core の確認 binary を用意し、Remote も同時 build する。
この設計のみの作業では build / test / binary 起動は不要で、実行済み件数は 0。
利用者による実機確認の候補は、多数 MP3 の scroll / 再訪・タグ編集後の更新、全一覧と詳細 hover、
art なしで idle CPU / repaint、再生中の一覧・F12・Remote の取得 / 切断、スマートフォン表示。
エージェントは製品 binary を起動しない。具体的な確認枠と利用者の明示承認は実装後に調整する。

## 10. 文書更新と今回の引き継ぎ

初版では本計画と docs/README.md の索引を追加した。R4 / R5 の記録を経て、R6 は利用者訂正に合わせ本計画と索引を同名 sidecar の範囲へ改訂する。
backlog の利用者決定や
未実装のマニュアル・製品紹介は完成形に書き換えない。
実装時に catalog-design、display-pipeline、async-architecture、architecture-overview、
music-integration-plan D2 / §3.2 (過去の決定は履歴として保存)、web-remote-plan §13.5 と新仕様、
settings transfer の既存 field の説明、preferences snapshot policy、spec、
htdocs/mimageviewer/manual の video / music / 設定説明、製品ページを co-update する。
video_pins.db、rename / metadata transfer の user-data 形式、keymap に音声操作は追加しない。privacy の cache 保存 / 通信と製品ページの
「安心して使えます」も照合し、新しい外部通信は増えないこと、既存認証済み Remote への
ジャケット配信が画像配信の記述に含まれることを確認する。

coordinator への引き継ぎ: Q1 訂正と既決 Q2〜Q7、R7 の永続コレクション出所伝達の再レビュー、
入力有界化の技術ゲート、
全 producer / consumer を含む実装 brief と file ownership、統合 IPC 番号の決定が次の作業。
commit は行わない。HEAD 上の follow-up 用英語メッセージは `target/D-r7-msg.txt` に置く。
過去の target/D-*-msg.txt は変更しない。

## 11. 独立設計レビュー R1 への対応記録

6 件ともコードで再確認して採用した。指摘への異論はない。独立レビューの合格を主張せず、
R1 対応時点では改訂した所有境界・更新契約は再レビュー待ち、Q1〜Q7 は利用者回答待ちだった。
現在の決定は §1 / §8 を参照。

| 指摘 | 確認したコードと解消内容 |
| --- | --- |
| R1-1 | app.rs:36864,36938 / catalog.rs:408 の同期経路を確認。§5.1 で共通 init_schema / delete_missing への追加を禁止し、追加 schema・art 読込・掃除を worker の専用 dispatch に限定、UI bulk query は art を SQL で除外 |
| R1-2 | activity_gate.rs:13,103 と app.rs:46599,46628 を確認。§4.1 / §4.3 で ActivityGate 不使用、可視・hover は待機せず、先読みは既存の投入前 idle 制御のみ |
| R1-3 | bookmark_browser.rs:564 / app.rs:48125,90062,89660 を確認。§3.1 / §5.3 / §6 で表示日時を保持しつつ worker fresh stat を唯一の source stamp とし、未知表示 meta でも Audio 要求可能。§9 に bookmark・history 回帰条件を追加 |
| R1-4 | catalog.rs:39,963 を確認。§5.2 で DB hash 維持、drive を含む親 scope prefix の key と complete inventory による worker prune を定義。一般 delete_missing から art を除外し、§9 に別 drive の positive / negative 保持を追加 |
| R1-5 | app.js:8947,9741,9766 の binding 初期化・DOM eviction を確認。§7.2 に一覧世代 / address の NoArt・Failed owner と破棄条件を定義。§9 に Cache Off / Auto と再マウントの無再要求テストを追加 |
| R1-6 | http.rs:3601 / app.js:9126,1893 と Fetch Standard を確認。§7.2 の 60 秒保証を撤回し、明示 refresh と artEpoch 更新を提案。新規 Q7 と §9 の外部交換・削除・追加テストで契約を一致させた |

## 12. 独立設計レビュー R2 への対応記録

2 件ともコードで確認して採用した。異論はない。R1 の直接指摘については R2 で解消方針の
確認を受けたが、R2 時点の全体判定は REVISE だった。後続の判定は §13 に記録する。
R2 対応時点では質問は Q1〜Q7 のまま未回答で、追加・変更はなかった。現在の決定は §1 / §8。

| 指摘 | 確認したコードと解消内容 |
| --- | --- |
| R2-1 | app.rs:35623,46530 / catalog.rs:1659,1672 / cache_maintenance.rs:530,542 / remote_ipc/thumbnail.rs:336 を確認。§5.1 / §5.2 の worker LRU を撤回し、§5.4 で短命接続・全 owner の受付失効 / drain / retire / 削除 / 再受付を定義。古い epoch の書込・prune・DB 再作成を禁止。§4.3 に簡素化、§9 に全件 / 期限削除の回帰条件を記録。通常の所有権を Q5 に分類しない |
| R2-2 | http.rs:4297 の GenerationFailed / NoThumbnail = 422 を確認。§7.1 / §7.2 で no_thumbnail 識別子だけを NoArt、その他の 422 を Failed と定義し、両方とも非 retry。§9 に HTTP mapping / Node terminal owner / absence / telemetry の回帰条件を追加 |

## 13. 利用者訂正と改訂履歴 (R6、2026-10-08)

R4 / R5 の手動画像指定案は coordinator が Q1 の「動画と同じ」を誤読した結果だった。
利用者の訂正に従い全撤回し、既存動画の同名 sidecar を Audio に一般化する §3.3 へ置き換えた。
表示マーク 3 択、MP3 抽出上限、NoArt、catalog 削除境界、Remote の初版対応は保持する。
利用者から伝達された前版 ACCEPT / R4 ACCEPT WITH CHANGES の履歴は保持するが、R6 の合格とは扱わない。

| 確認対象 | コード照合と R6 の反映 |
| --- | --- |
| 発見 / 画像行 | folder_scan.rs:139,314,831、folder_tree.rs:94,110,144。Video 限定を Audio に拡張し、同 stem、認識画像リスト、二設定と画像行省略を共用。複数候補の既存 map 選定も保つ |
| 全一覧 / 更新 | collection_grid.rs:403、smart_folder.rs:4718,4920、app.rs:23235,23400,26799,46762、preferences.rs:2928。full media path の共通 source map、filter 前 scan signature と既存 reload、aggregate の明示結果を消さない |
| Remote | remote_ipc/mod.rs:77,107,133、container.rs:2993,3808、thumbnail.rs:240,482,549,664、app.js:7268,7307。Audio の thumbnail_address / source_address を許可し、動画と同じ path guard と画像生成 helper、source を含む Flight identity を使用 |
| 設定 / 互換 | settings.rs:4753,5691,7415,7676、settings_transfer.rs:312,378、preferences/pages.rs:8107,9068、settings_db.rs:58,306,3133。既存 field 名 / bool / default / plain 分類を保持し、label と意味を動画・音声共通へ更新。user pin DB や metadata transfer の移行は不要 |

未決の利用者質問はない。今回も文書のみで、製品コード・IPC 定数・出荷済み DB は変更せず、
commit・製品 binary 起動なし。製品テスト実行数は 0。

## 14. 独立設計再レビュー R7 への対応記録 (2026-10-08)

4fe5979e0 の再レビューの P2 をコードで確認し採用した。異論はない。判定 REVISE を記録し、
今回の文書修正は再レビュー待ちとする。追加の利用者質問はなく、既決仕様は変更しない。

| 指摘 | 確認したコードと解消内容 |
| --- | --- |
| R7-P2 永続コレクションの出所伝達 | persistent_collections.rs:1566 が thumbnail_address: None を作り、mod.rs:153 の populate_remote_entries は通常 RemoteEntry のみが対象であることを確認。snapshot (503,538) と navigate (635) は view_facts / navigation_view_facts を使い、wire_snapshot (1449) が facts.entries をコピーする。§7.1 に共通 discovery→Available.thumbnail_address の付与を byte 計算 / token 確定前に行う契約と、設定を含む既存 facts reuse key・取消 / deadline・refresh の所有境界を追記。app.js:4921,7307 は既存 optional 出所を tile / query に渡せるため wire field の追加は不要。§9 に両経路の FLAC / MP3、設定・再利用・frame 上限・token・Web 伝達の回帰条件を追加 |

文書のみの修正。製品コードの実装・commit・製品 binary 起動なし、製品テスト実行数は 0。
