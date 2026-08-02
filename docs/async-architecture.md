# 非同期・並列アーキテクチャ

「どの処理がどのスレッド/プロセスで走るか」「どうやってキャンセルするか」「キャッシュ競合をどう避けるか」
の一覧。並列処理を追加・変更するときの設計テンプレートとして使う。

---

## 1. ワーカー一覧

| ワーカー | 実装 | 個数 | 用途 |
| --- | --- | --- | --- |
| サムネイル (通常) | `std::thread` + mpsc | `parallelism - 重I/O` | Image / ZipImage / PdfPage の軽いデコード + PdfFile のフォルダ代表画 (PDFium pool への IPC 待ちなのでメインプロセス内 CPU は消費しない。PDFium pool 5 並列を活かすためここに置く) |
| サムネイル (重 I/O) | `std::thread` + mpsc | 1〜2 (総数 ≤4 なら 1) | Folder / ZipFile の全体走査 (本物の同期 I/O。`fs::read_dir` 再帰探索 / ZIP セントラルディレクトリ読み込みなどメインプロセス内ブロッキング) |
| 製本並べ替えサムネイル | `std::thread` + mpsc | 最大 4 in-flight | 本の並べ替え専用ビューの焼き込み済みページを小サムネとして先行 decode。通常グリッドのキャッシュ/drag-out 経路とは分離し、UI 側は結果 backlog から `load_texture` を 1 フレーム 1 枚だけ実行する |
| フルスクリーンロード | `std::thread` (使い捨て) | 1 枚ごとに spawn | フルサイズ画像デコード + アニメ展開 |
| PDF ワーカー | **別プロセス** (`--pdf-worker`) + 各プロセス専用のディスパッチャースレッド | 5 (`POOL_SIZE`、うち 1 を Critical 予約) | PDFium は非スレッドセーフ → マルチプロセスで並列化。要求は JobQueue に enqueue |
| PDF ページ列挙 | `std::thread` | 1 (PDF 開く都度) | PDF ワーカーに列挙要求を送る |
| PDF メタ catch-up / 隣接 prefetch | `std::thread` (常駐、`pdf-meta-catchup`) | 1 | `pdf_meta` テーブルへの背景書き込みを統括 (v1.0.0)。WebP cache hit で render_page を skip した PDF (= アップグレードユーザーの既存サムネ) の `pdf_meta` 補完 (`MetaOnly`、low lane) と、`load_pdf_as_folder` 直後の ±1 隣接 PDF の page 0 render + WebP 温め (`NeighborPrefetch`、high lane) を、`CatchupQueue` 経由でシリアル処理する。重複は pending HashSet で dedup、low → high の優先昇格あり |
| Susie ワーカー | **別プロセス** (`mimageviewer-susie32.exe`、32bit ビルド) + ディスパッチャースレッド | 3 (設定で 1 に落とせる) | 32bit の Susie 画像プラグイン (`.spi`) をロードし IsSupported/GetPicture を呼び出す。プラグインクラッシュの隔離も兼ねる |
| AI 推論 (final pipeline) | `std::thread` (`final-ai-worker`, 常駐) + 優先度キュー (`AiJobQueue`) + 共有 mpsc | 1 | final AI (upscale/denoise) を `AiJob` キューから逐次処理。`AiRuntime` の sessions Mutex が全推論を直列化するため worker は 1 本で十分。**モデルロード (`load_model`) / 推論を worker スレッド上で実行し、UI スレッドは sessions ロックに触らない** (= per-job spawn だった旧設計の「UI THREAD HANG: 推論ロック飢餓」を解消、§3.2.1)。優先度は Display(表示中ページ, LIFO) → Prefetch(先読み, FIFO) |
| カラー化 / 最終エフェクト | `std::thread` (`miv-final-effect-{idx}` / `miv-final-effect-prefetch-{idx}`、要求ごとの短命 worker) + 個別 mpsc | 表示要求 + 背景先読み最大 1 本 | final AI / sharpen 後の `ColorImage` に、モノクロ系判定、スクリーントーン濃淡変換、階調カラー LUT、ポストフィルタを順に適用する。AI と共通の前後枚数を候補にし、ページ送りでは従来どおり、連結読みでは `fs_vertical_cache_keep_set` との積に絞り、実テクスチャ会計が共有 pool 由来の LOW 未満のときだけ遠方ページを先読みする。厳密可視の前後 1 ユニットの準備帯は LOW をバイパスして非表示ページを 1 枚ずつ先行処理し、色調補正・smart sharpen も worker 上で行う。可視ページの表示要求は水位をバイパスする。先読み／表示開始時とも無着色の provisional texture を upload せず、完成結果だけを `final_composite_cache` へ upload する。表示要求が先読み中の同じ key に来た場合は job を昇格して再利用し、同一 viewer のページ送りでは完成まで直前ページを holdover する。連結読みでは keep-set 内の各表示済みページがページ別 transition texture を持ち、raw source 差し替えや incomplete → complete 再合成で live final が一時的に消えても黒い読込表示へ戻さない。complete final の GPU 登録後に差し替え、keep-set 離脱時は旧 texture も破棄する。ページ入場だけでは完成 cache / pending を破棄しない。別 key の表示要求、設定変更、keep-set 除外、context close / drop では `Arc<AtomicBool>` を立て、`items_generation` と composite key が一致する結果だけを採用する。pending map とページ別 transition は viewer context の swap / park と一緒に所有権移動するため、別ウィンドウの要求や表示を誤って共有しない |
| edit materialization (local / conceal) | `std::thread` (`local-adjust-render` / `conceal-materialize`、要求ごとの短命 worker) + idx 別 mpsc | viewer context 内で idx ごとに最新 1 本。可視ページが複数なら別 idx は並行し得る | source 解像度 edit chain の lazy LocalAdjust DB / JSON load、mask resize、local compose、および Conceal DB read、deflate / JSON decode、shape raster、conceal compose を UI thread 外で行う。local と conceal は同じ `local_adjust_pending[idx]` slot を共有し、後段が必要な local 完了前に conceal を誤った下位 source へ合成しない。CPU 結果だけを返し、generation / key を UI 側で検証した後、共通予算で 1 フレーム 1 枚だけ GPU upload する。詳細は §3.2.2 |
| AI 消しゴム (MI-GAN inpaint) | `std::thread` (使い捨て) + mpsc | preview/commit ごと | erase ツールの補完推論 (`erase_inpaint_pending`、final pipeline とは別経路、§3.3) |
| Ctrl+E エクスポート | `std::thread` (`ctrl-e-export`) + mpsc | ダイアログ確定ごとに 1 本 | UI スレッドで snapshot した base pixels / composite mask / preset を使い、隠蔽合成と JPEG/PNG/WebP 保存を順番に実行する。元画像メタデータ転記と `create_new` 書き込みも worker 側で実行し、キャンセルは各エントリ開始前に `Arc<AtomicBool>` を確認する |
| 操作カスタマイズ共有 / 世代取り込み | `std::thread` (操作ごとの短命 worker) + mpsc | 「設定の復元」ダイアログ中に最大 1 本 | 過去世代 DB の一時コピーと読み込み、`.mivkeys.json` の読み書き、取り込み前の自動退避を UI スレッド外で行う。UI は native ファイルダイアログでパスを選び、50ms polling で結果を受け取ってから差分表示またはライブ適用する |
| サブ展開 snapshot | `std::thread` (`subfolder-expansion` / `subfolder-view-prepare`) + mpsc | 最大 1 scan + 1 prepare | 現在地以下の画像 / 動画、ZIP/PDF 本体、設定上の画像フォルダ本を共通 recursive snapshot walker で列挙する。ZIP/PDF 内部は開かない。`GlobalIoSemaphore` Normal priority と `ActivityGate` を通し、`Arc<AtomicBool>` で cancel、generation で stale 結果を破棄する。大量ソート、metadata 構築、コンテナピンの一括照会も worker 側で行う |
| スマートフォルダ snapshot | `std::thread` (`smart-folder-scan` / `smart-folder-prepare`) + mpsc | 最大 1 scan + 1 prepare | 保存済みの複数ルールを OR 結合し、ルールごとの実検索元 / 再帰指定からフォルダ / 画像 / 動画 / 音声 / ZIP / PDF / 対応アーカイブを列挙する。各 `read_dir` の候補へ通常一覧と同じ同名ファイル規則を物理フォルダ単位で適用してから保存条件を判定し、動画 sidecar は full-path key の snapshot で表示準備へ渡す。★ / タグ / 編集状態と一覧復元用の個別編集状態は prepare worker で exact-key batch 取得し、変換アーカイブ対応表、catalog、固定代表も同 worker で準備する。開始時に snapshot した現在の全体ソート順と定義固有のグループ化単位でフラット一覧を構築し、UI は同期 DB I/O をせず完成 snapshot だけを install する。削除は scan 開始世代以後の tombstone を成功 snapshot へ適用してから破棄し、全 source 失敗では保持する。★ / タグ / 編集状態・定義変更は再 prepare する。通常一覧のソート順・サムネイル / 詳細表示は上書きしない。定義変更・移動・終了時は cancel、generation と定義 snapshot の一致で stale 結果を拒否する。cancel時はreceiver内に到着済みの巨大な`Done`結果と大件数確認待ちsnapshotをpending所有者ごと専用drop workerへ移し、UIスレッドで破棄しない |
| 孤児メタデータ整理 | `std::thread` (`metadata-cleanup-scan` / `metadata-cleanup-delete`) + mpsc | 明示操作ごとに 1 本 | `rename_key_migration::STORES` の全行列挙と path 存在確認、確認後の DELETE を UI スレッド外で行う。`Arc<AtomicBool>` で行境界キャンセル、atomic 進捗、削除直前のオフライン再判定、descriptor 単位 transaction rollback を持つ |
| 削除 purge journal 再試行 | `std::thread` (`delete-purge-retry`) + mpsc | 最大 1 | Shell 削除成功後の hard purge が最終失敗した path を `delete_purge_journal.json` から読み、起動時 / 1 秒入力 idle 後 / 失敗時 10 秒 backoff 後にピンポイント再 purge。孤児整理と同じ親到達可能 + path 不在を再確認し、成功 entry だけ atomic に消し込む |
| リネーム移行 journal writer | `std::thread` (`rename-migration-journal`) + Mutex/Condvar | App-global 1 (初回保存時に lazy 起動) | App が組み立てる in-flight + FIFO queue + boot-retry の完全 snapshot を latest-value で置換し、単一 worker が順番に atomic 保存する。中間 snapshot は coalesce し、終了時は最新 revision まで flush する。起動時 load は回復 entry と新規 enqueue の順序を守るため初回 enqueue / poll 前の同期 1 回を維持する |
| 閲覧履歴 writer | `std::thread` (`reading-history-writer`) + mpsc | 1 | フルスクリーンで読んだ画像フォルダ / ZIP / PDF / 変換アーカイブを `reading_history.db` へ upsert / prune する。UI スレッドは履歴 entry を送るだけで、ファイルサイズ / mtime の `metadata()` 補完も writer 側で行う。キャンセルは持たず、App drop 時に tx close → queue drain → join |
| 本ブックマーク DB | `std::thread` (`book-bookmarks`) + mpsc | App-global 1 | `book_bookmarks.db` の追加 / コンテナ別一覧 / 全件一覧 / 削除 / path migration を直列処理する。製本 worker は最初のファイル変更前に、最終 path mapping と永続 temp 名・copy/move identity を含む filesystem plan を `Prepared` journal として保存する。`Applying` の step ごとに進捗を保存し、全 filesystem step 完了後の `FilesystemCommitted` だけ bookmark migration と journal 消去を同一 transaction で commit する。通常エラーは先に `RollingBack` を保存し、全 inverse step の成功を証明できた場合だけ journal を消す。起動時は `Prepared` を no-op 破棄し、`Applying` / `RollingBack` を実 filesystem 状態から冪等再開する。ただし同一プロセスで writer が生存中の job は active ownership registry で回復対象外にし、複数 service が実行中操作を crash 残存と誤認しない。DB busy / parse / I/O failure の entry と診断は消さず次回 retry に残す。UI は正規化前の表示値と identity を request で送り、result event からメモリ上の現在本一覧を更新する。SQLite open / schema 作成を含め UI スレッドでは行わない |
| 横断ブックマーク一覧 | `std::thread` (`bookmark-browser-build` / `bookmark-browser-delete`) + mpsc | 一覧再読込または削除ごとに最大 1 | `video_bookmarks.db` と `book_bookmarks.db` を共通 read model にまとめ、登録日時順ソートと元コンテナ / ページの存在確認を行う。動画・音声の種別判定、保存済み WebP の decode、ZIP entry / PDF page の確認も worker 側。結果は通常の `App.items` グリッドと同順の sidecar row として install し、保存済み WebP も通常のフレーム当たり texture upload 上限を通す。削除 worker は DB 行だけを削除し、元メディアへ filesystem 操作を行わない。UI は在メモリの media / book subtype filter と通常 facet だけを評価する |
| ブックマーク状態フィルタ | `std::thread` (`bookmark-presence-build`) + mpsc | 状態条件の初回使用または CRUD 後に最大 1 | 動画・音声 path、本 container / page identity の軽量集合だけを両 DB から読み、通常一覧へ渡す。行ごとのフィルタ判定は在メモリ。snapshot完了時はmain / active detached / paused detachedを順にmountし、各context所有の表示集合をDB I/Oなしで再計算する。スマートフォルダは既存 prepare worker 内で同じ snapshot を読み、UI スレッドから DB を開かない |
| 明示メタ情報転送 | `std::thread` (`metadata-export` / `metadata-import-preview` / `metadata-import` / `metadata-import-refresh`) + mpsc + `AtomicBool` | 完全モーダル中にimport本体と終端refreshを直列で最大 1 | `mimageviewer.meta.miv` の再帰列挙、JSON / media-kind / file-size検証・原子的exportと、評価 / タグ / ブックマーク / 見開き / 表示トリム / 回転 / ページ編集 / サムネピンDBのimportをUI thread外で行う。変換archiveはsource containerとcache ZIP pageのaliasをportable identityへ変換する。列挙は`read_dir`を逐次消費し、メタ情報DBは一時scope表とpath indexで列挙済み項目だけを読む。開始前にpending view-trimをメモリsnapshotとしてworkerへ渡す。main / active detached / paused detachedのXMP rating readerは取消し、legacy tag seedは完了結果をcontextへ適用してから、既存metadata writer / rename migrationとともにdrainする。転送完了・開始失敗・待機cancel後はXMP readerを全contextへ再生成する。dirtyな`mimageviewer.dat`群とAppのTagsDb connectionも所有権ごとimport workerへ移してflush / journal-mode handoff / 再openを行い、いずれかのflush失敗時はDB更新を開始しない。importは15ストアをattached connectionへまとめ、256項目 / 実record 64 MiB / 500 msの外側transactionと項目SAVEPOINTを使う。target照合はファイル本体を読まず相対path / kind / sizeを使う。family削除はindex range seek、sidecar syncはfolder / section単位、file種別外storeはskipする。異常終了では現在batch、項目エラーでは当該SAVEPOINTだけをrollbackし、明示cancelでは現在batchもcommitする。DB更新中は既存UI cacheを変更しない。終端時だけ影響viewer contextの現在キー、folder-pin identity、legacy seed pathを3 ms / 2048項目ずつcompact snapshot化し、refresh workerがDBを一括再取得する。タグcacheは未タグキーも空Vecの読込済みsentinelとして含める。page-state importは永続edit-preview cacheのclear ACKを待ち、main / active detached / paused detachedへ失効を伝播し、旧/新編集状態の和に当たるmaterialized thumbnailを再要求する。UIは世代・window identity一致後、各contextをmount中にcache swapとvisible/facet/details/selection再計算、およびXMP rating hydration / legacy tag seed workerの再生成を行う。外部snapshotによる表示再計算はApp-globalなfacet scope / suppressionを同期せず、不一致ならモーダル中に再取得する。video pin削除も全対象動画の再生成として表現する。終端refreshはimport本体と別cancel tokenを持ち、終了・破棄時はcontext/DB chunk境界で中断して巨大な途中結果をworker側でdropする。本体と終端refreshの段階時間は常時logと任意の構造化perf logへ記録する |
| テキスト注釈ベイク | `std::thread` (`comic-bake`) + mpsc | 閲覧時最大 2 | Ctrl+T 注釈を final composite 上へ焼き込む。閲覧時は stamp 画像の cache miss デコードも worker 側で行い、完了時に `comic_stamp_cache` へ merge する。編集中はライブ追従を優先し、プレビュー解像度で同期ベイクする |
| 音声出力 warm-up | `std::thread` (`cpal-warmup`) | 起動時 1 本 | WASAPI の初回 audio session 確立をバックグラウンドで済ませる。小さな無音 cpal stream を短時間だけ開いて閉じ、初回動画 open の UI スレッド停止を避ける |
| 動画サムネイル | `std::thread` | 1 | Windows Shell API を逐次呼び出し |
| シークサムネイル | `std::thread` (`video-thumb`) | 動画 1 つにつき 1 本 | seek hover preview と左 jump panel warmup のサムネイル抽出。初回 cache miss で同じ動画ファイルを別 `Input` と長寿命の補助 video decoder で開く。HW decode 有効時は FFmpeg-owned D3D11VA を優先し、RGBA 生成は CPU readback + swscale。失敗時は worker 内で SW decode にフォールバックし、本編 fast-swap の `LIVE_VIDEO_DECODE_THREADS` には入れない |
| 動画タイルサムネイル | `std::thread` (`video-tile-thumbs`) | S タイルモード 1 セッションにつき 1 本 | タイル表示用に N 個の絶対 PTS を順番に抽出する。キャッシュ hit 済み slot は FFmpeg open 前に埋め、残りだけ別 `Input` + 補助 video decoder で処理する。HW decode 有効時はシークサムネイルと同じ FFmpeg-owned D3D11VA を優先し、RGBA 生成は CPU readback + swscale。HW 初期化 / decode 失敗時は worker 内で SW decode にフォールバックし、本編 fast-swap の `LIVE_VIDEO_DECODE_THREADS` には入れない |
| 動画 demux | `std::thread` (`video-demux`、= `run_decoder` の本体) | 動画 1 つにつき 1 本 | FFmpeg `Input` の packet を `video_pkt_tx` (bounded=32) / `audio_pkt_tx` (bounded=64) へ振り分ける。packet channel は `Packet/Eof`、seek の `Flush` は各 bounded=8 の `video_ctl_tx` / `audio_ctl_tx` へ分離し、decode 側は control を優先受信する。seek 要求は demux thread が単独 pull。packet 送信待ち中の新 seek は旧 packet を捨てて loop に戻る。thread panic は `info_tx(Err)` + `DecoderEvent::Failed` に変換する |
| 動画 video decode | `std::thread` (`video-decode`、= `run_video_decode`) | 動画 1 つにつき 1 本 | `VideoPacketMsg::{Packet,Eof}` と別 channel の `VideoControlMsg::Flush` を受け、HW (D3D11VA + GPU blit) / SW (readback + swscale) で frame を生成して `video_tx` (bounded=8) へ送る。PLAYING の Full は drop 可、Loading/Buffering/Seeking の Full は pending frame を保持して cancel/seek-aware retry。Paused/Eof は park。mIV Remote video tap は decoded PTS / seek / preroll 確定後かつ GPU/CPU 分岐前で、D3D11 frame だけ producer 上で即時 SW download、SW frame は shallow-ref し、SW-only bounded queue へ `try_send` する。queue 内の decoder HW surface 上限は容量に関係なく 0、同期処理中は 1。満杯時は readback / ref 作成前に drop を計上する。stream scale / H.264 encode は将来の session worker 側 `VideoStreamEncoder` が行い、未接続時は一切走らない |
| mIV Remote headless video output | `std::thread` | remote-owned video player につき 1 本 | native presenter を持たない remote player の `video_tx` を連続 drain し、通常出力 frame を GPU slot 返却後に破棄する。tap は decoder 側の別分岐なので配信用 frame は維持する。seek 世代ごとの最初の frame で `FirstFrameReady` を発火し、event queue が Full でも video drain を止めず再試行する。通常 player は起動せず presenter / UI receiver が従来どおり consumer。player cancel / shutdown / Drop で停止・join する |
| 動画 audio decode | `std::thread` (`video-audio-decode`、= `run_audio_decode`) | 動画 1 つにつき 1 本 (音声無し動画では起動しない) | `AudioPacketMsg::{Packet,Eof}` と別 channel の `AudioControlMsg::Flush` を受け、avcodec decode + swresample で output device rate の f32 stereo に変換し、`audio_tx` (bounded=32) へ送る。device rate を取得できない場合だけ 48kHz fallback。解析用 `audio_decode.rs` の 48kHz 固定とは別。Paused/Eof は park、EOF は decoder delay を drain |
| 動画音声 pump | `std::thread` (`audio-pump`) | 動画 1 つにつき 1 本 | `audio_tx` から受けたフレームを ring buffer に押し込む。RT 出力は cpal の専用スレッドが担当。1.0x 以外では Signalsmith Stretch で pitch を維持したまま output/wall 秒へ time-stretch し、その後 VST3 enable + プラグインロード済みなら ring buffer に push する直前に `DspBridge::process_block` 経由で bridge プロセスへ往復する (= ~1-2ms IPC roundtrip)。mIV Remote の audio tap も最終 `ProcessedChunk` の push 直前 1 箇所だけで、bounded channel へ non-blocking 送信する。満杯は `dropped` として PC 再生を優先し、接続時の sample clone は cpal 共有 mutex の外で行う |
| mIV Remote streaming generation | `std::thread` (`remote-stream-generation`) + bounded tap/resource channels | streaming session の現 generation につき 1 本 | audio/video tap lease と receiver、H.264/AAC encoder、A/V interleave、fMP4 segmenter/ring を所有する。重い encoder open は worker 内だけで実行し、UI の `App::poll_remote_session` は owner/status/seek serial の照合と worker 交換だけを行う。remote owner cancel、seek、画質変更、session drop で cancel flag を立て、FFmpeg teardown の join は `remote-stream-generation-join` へ逃がす。作りかけ fragment/interleave backlog が 6 秒を超えた場合は session を停止する |
| mIV Remote streaming IPC | `std::thread` (`remote-stream-ipc-0..3`) + bounded queue (32) | remote IPC server ごとに 4 本 | protocol v15 の start/control/seek/playlist/segment/state/stop 専用 lane。エンコード済み resource の pull が持つ最大 2 秒の応答待ちを thumbnail/page の heavy queue から分離する。stream queue の飽和は Busy を即応答し、heavy / Home / write worker の枠を消費しない |
| Normalize 全尺 scan | `std::thread` (`normalize-scan`) + mpsc + `Arc<AtomicBool>` | App-global の active request 最大 1 本 | `NormalizeScanState` が file path / fs_idx / cancel / atomic progress / receiver / worker handle を所有する。600 秒で `Provisional` を送り、同じ worker が全尺 `Done` まで継続する。Provisional は同じ fs_idx の player path が一致するときだけ live 適用して通常操作を再開。Done は file 単位結果なので stale でも DB 保存するが、live player/UI への適用は同じ fs_idx + exact path の場合だけ。新規 scan、cancel、対象 cache cleanup は cancel token を立て、古い結果を別動画へ適用しない |
| 動画 native presenter | `std::thread` (`native-video-presenter`, Windows) | フルスクリーン動画 1 つにつき 1 本。ただし動画タイルモード中の動画→動画移動では `SwitchSource` で再利用 | 専用 HWND + D3D11 presenter + egui overlay を保持し、`video_rx` から受けた `VideoFrame` を表示する。`NativeVideoOutputCommand::SwitchSource` で source binding (`video_rx` / `AvClock` / engine event tx / duration / displayed_frame_seq) を差し替え、HWND と overlay を破棄せず次動画へ切り替える |
| VST3 host bridge | **別プロセス** (`mimageviewer-vst3-host.exe`、C++) | アプリ起動中 0 or 1 本 | VST3 SDK は C++ 前提なので bridge プロセス分離。bridge 内部は 3 thread (audio loop + GUI message pump + stdin pump)。詳細は [docs/vst3-integration.md](vst3-integration.md) と `crates/vst3-host/` ソースコメント |
| VST3 plugin GUI worker | bridge 内 per-slot STA thread (lazy) | editor を生成した slot ごと。表示した plugin 数まで増え得る | C++ bridge の plugin loader が slot ごとに STA message thread を lazy 生成し、bridge-owned editor surface と `IPlugView::attached()` を管理する。Rust 側に単一 `vst3-plugin-gui` thread はない |
| 動画音声 RT 出力 | `cpal::Stream` 内部スレッド | 動画 1 つにつき 1 本 | WASAPI Shared モード。コールバックで ring buffer から f32 stereo を pop し、**実消費サンプル数 (= `real_consumed`) 分のみ** `next_pts_secs` を進めて `AvClock::set_audio_pts` でマスタークロックを更新。silence 出力中 (= `real_consumed=0`) は pts 進行 skip。`!clock.is_playing()` (= 一時停止 / EOF) と `pump_seek_serial < clock_serial` (= pre-seek サンプル全消去) は早期 return。`AvClock::set_audio_pts` 側に defensive wall-rate cap (= `wall_dt + 5ms` で pts 進行を頭打ち) を保持し、buffer 非空 pre-fill burst の異常前進への保険にしている (Phase 9 後の cleanup refactor、詳細は [docs/video-engine-redesign.md](video-engine-redesign.md) の「Phase 9 後の Post-cleanup refactor」節) |
| 起動 / activation パス解決 | `std::thread` (`startup-open-resolve`) | 起動引数 / 2 重起動 activation ごとに最大 1 本 | `resolve_openable_path_detailed` (`Path::is_dir` / `is_file` + 親探索) を UI スレッド外で実行する。400ms 以上未完了ならメインウィンドウに「パスを確認しています…」toast を出し、完了後だけ UI スレッドで既存の `load_folder_or_convert_archive...` に戻す。新しい activation が来たら旧 pending を cancel し、古い結果は適用しない |
| RAR / 7z / LZH / ZIP スキャン・変換 | `std::thread` (scan / convert ごとの使い捨て) + mpsc | `ArchiveConvertState` 1 件 | 直接 RAR 判定、画像 inventory、パスワード再試行、キャッシュ ZIP 変換を行う。`ArchiveConvertState.cancel` は事前 scan から変換完了までの単一 owner で、RAR / 7z / LZH / ZIP の各 entry 境界が同じ token を確認する。`OpenRequestOwner::Navigation` はarchive種別判定より前にvisible-open lifecycleを取得し、archive Aのscan中にarchive Bを開く場合もAのtokenとreceiverを終了してからBのstateを作る。state drop、Esc / cancel、activation、競合する通常 navigation、後続 bookmarkも同じ規約で古いworkerとlate resultを無効化する。ブックマーク起点では`completion`がrequest IDとtarget identityも保持し、直接RARまたは元アーカイブ→キャッシュZIPのmount後に同じownerでページ待機へ進める。確認・パスワード・変換中は通常の45秒resolve timeoutを適用せず、完了はownerが現在値と一致する場合だけ表示へ適用する |
| フォルダナビゲーション | `std::thread` | 1 (常時 ≤ 1 本) | 深さ優先で次フォルダを検索。連打は `pending_folder_nav_steps` に累積され、完了ごとに連鎖実行する (並行 DFS による FS 競合を避ける) |
| メディア前後送り候補の存在確認 | 常駐 `std::thread` (`media-nav-resolver`) + mpsc mailbox | App-global 0 or 1 (初回要求時に lazy 起動) | EOF / 手動前後送り時、UI が bundle items から抽出した方向付き候補列の実ファイルだけを `Path::exists` で順次検証する。worker は受信時に mailbox を drain して最新 request だけを処理し、キャンセル不能な `Path::exists` 中の新要求も同じ 1 thread に滞留させる。結果は単調増加 request id が最新 pending と一致する場合だけ検討し、`items_generation` / owner window / 開始時 `fullscreen_idx` を context-local stale 条件とする。App-global `input_seq` の一致は owner なしの mounted context だけで要求し、ParkedLive では無関係な main 入力を stale 理由にしない。owner 不一致中は結果を受信せず保留する。EOF action を stale / superseded / context close・load / resolver 切断 / apply 拒否のいずれかで捨てる場合は、対応する `(fs_idx, seek_serial)` と現在値が一致するときだけ video / 動画音声モード / music 共通 dedup latch を解除して次 tick の再試行を許す。ZIP/PDF 仮想 entry は I/O せず候補に残す。App drop で request sender が drop され、in-flight I/O が戻った後に receiver disconnect で自然終了する (終了時 join なし)。 |
| ファイル名スタック分類 | `std::thread` (`stack-script`) + mpsc | スタックモード ON かつスクリプト有効時、フォルダ読込ごとに最大 1 本 | ユーザー定義 Rhai スクリプト ([`filename_stack_script`](../src/filename_stack_script.rs)) でフォルダ内画像のグループキーを算出する。重くなり得る (実測 10 万件 ~1 秒) ので UI スレッドから外す。通常フォルダを先に表示し、完了後 `poll_stack_script` が `start_loading_items` 経由で集約ビューへ差し替える。`StackScriptPending` の cancel + folder 一致で stale 判定、別フォルダ移動 / スタック OFF / 再ロードで cancel、失敗は組み込み既定へフォールバック。組み込み既定 (separator) ルールは軽量なので同期構築のまま。詳細 [filename-stack-scripting-plan.md](filename-stack-scripting-plan.md) |
| 音楽ビュー解析 | `std::thread` (`miv-music-analysis`) + mpsc | 音楽ビューを開くごとに最大 1 本 | `run_music_analysis` が FFmpeg 全尺 decode + `analyze_stereo_timeline` を**全て UI スレッド外**で行う (Inc 3b)。**永続 DB (`audio_analysis.db`) はやめ in-memory LRU に置換 (2026-07-03)**: 結果は UI スレッドが `music_analysis_lru` (直近 N 曲、path+size+mtime キー) に保持し、`ensure_music_analysis` が `image_metas` の (mtime,size) で LRU を楽観的に lookup (UI スレッド stat しない) してヒットならタイムラインを即セット + ワーカーを `want_analysis=false` で起動、miss なら `want_analysis=true`。ワーカーは DB は触らないが背景で実ファイルを **fresh stat** し、ヒットに使った `hit_meta` と食い違えば (外部更新) 解析し直す + LRU 挿入キーに検証済み (mtime,size) を返す (`TimelineComplete{analysis,meta}`、image_metas スナップショットが stale でも正しい key)。**Inc 4**: 同ワーカーが spectrum 用に全尺 PCM も**1 回だけ**デコードし (timeline と共有)、`MusicAnalysisMsg::{Timeline(progressive 部分), TimelineComplete(全尺確定=LRU 挿入), Pcm, Probe}` を送る。`poll_music_analysis` が届いた分を Disconnected まで drain (pending は取り出して末尾で戻す)、pending 中は `request_repaint_after(50ms)`。新ファイル / `close_fullscreen` で cancel。decode ループは cancel を確認するが post-decode の解析パスは単一 pass で cancel 不可 (支配的コストの decode はキャンセル可)。**progressive spectrum PCM (2026-07-07)**: `Pcm` は**デコード開始前**に空の共有バッファ (`Arc<MusicPcm>` = `RwLock<Vec<f32>>` 追記式) として送り、デコードは新 API `audio_decode::decode_audio_file_progressive` (差分 `on_delta` を渡す) で回して差分を `append` (write)。timeline partial / 最終確定は同じ共有バッファのプレフィックス (`with_prefix` = read ロック下解析) から作る。これで下段スペクトラムが全尺デコード完了を待たず出る (§5.6)。**`RwLock`** なのは、長い解析 read (`with_prefix`) と spectrum の窓コピー read (`copy_window`) を並行させて spectrum を固まらせないため (`Mutex` だと解析中フリーズの実機 FB → RwLock で解消)。共通セットアップ `open_audio_decode` / partial スケジュール `PartialEmitSchedule` は既存 streaming decode と共有 |
| 音楽ビュー row raster | `std::thread` (`miv-music-timeline-raster`) + mpsc | 音楽ビュー 1 つにつき 1 本 (`TimelineTextureCache`) | `TimelineAnalysis` から DJ 風波形タイムラインを **1 行 (row) ずつ** `egui::ColorImage` にラスタライズ。UI は request に `Arc<TimelineAnalysis>` を渡すだけ (行ウィンドウ切り出しは worker 側で zero-copy) で、結果は generation / row_version / key が現要求と一致するものだけを 1 フレーム 1 枚 `load_texture` する。旧 key / generation の結果は採用側で破棄。`ensure` (key 変更) / `clear` (ファイル変更・close) で worker を作り直す。詳細は [`src/ui_music_timeline.rs`](../src/ui_music_timeline.rs) |
| 音楽ビュー spectrum | `std::thread` (`miv-music-spectrum`) + mpsc | 音楽ビュー 1 つにつき 1 本 (`MusicSpectrumState`) | 常駐 `SpectrumAnalyzer` (E0-C#10 の MIDI 半音バー、多解像度 FFT) を所有し、UI から `Arc<MusicPcm>` + `center_secs` を受けて再生位置周辺 **±1 秒**の窓を worker 側でスライス (`copy_window`、lock 下で ~1MB 未満コピー) → `analyze_moving_window`。cpal ring buffer は約 100ms 分しか無く ±1s 窓に足りないため、ラボと同じく展開済み PCM をスライスする (案A、[music-integration-plan.md](music-integration-plan.md) §11)。**`MusicPcm` は追記式共有バッファ (`RwLock<Vec<f32>>` + `complete: AtomicBool`)**: 解析ワーカーがデコード進行に合わせて末尾 `append` (write) し、spectrum worker は現デコード済み範囲から窓を read で取る (未デコード領域が中心なら `None`)。`RwLock` = 長い解析 read と spectrum 窓コピー read を並行させ spectrum を固まらせない。高々 1 リクエスト in-flight + 溜まった分は最新へ coalesce。UI (`update`) は playing / 位置変化時に throttle (16ms) 付きで送り、pending / 再生中は `request_repaint_after(16ms)`。窓がまだ取れない間は空バンド = 鍵盤ベースライン + 「解析中…」表示 (`source_complete` で抑制)。新ファイル / close で `clear`。詳細は [`src/ui_music_spectrum.rs`](../src/ui_music_spectrum.rs) |
| キャッシュ一括生成 | `rayon` | (ユーザー設定) | ダイアログから起動するバッチ処理 |
| メタ索引 supervisor (Ctrl+F/G 用) | `std::thread` (常駐) | お気に入りごとに 1 本 (`auto_index_metadata=true`) | 初期スキャン + notify-rs 監視 + ingest を統括。共有 `Arc<Mutex<IndexWriter>>` 経由で Tantivy writer を直列化 (Tantivy は Index あたり writer 1 本制約) |
| メタ ingest worker | `std::thread` (supervisor 内部) | 速度プロファイルで 1 / 2 / 4 | メタ抽出 + Tantivy buffer + バッチ commit (100 件 or 5 秒) + commit 成功後に fts_meta upsert_meta_ok / delete_paths (Tantivy First) |
| メタ walker | `std::thread` (supervisor 内部、1 回) | 1 | 起動時 3-way diff (FS vs fts_meta.db) |
| メタ FsWatcher | `std::thread` (notify-rs 内部) | お気に入りごとに 1 本 | `ReadDirectoryChangesW` + 500ms debounce → `DebouncedChange` 送信 |
| 名前索引 supervisor (Ctrl+S 用) | `std::thread` (常駐) | お気に入りごとに 1 本 (`auto_index_structure=true`) | `search_index.db` は SQLite 単独なので複数 supervisor が真並列で動く |
| Ctrl+G クエリワーカー | `std::thread` (使い捨て) | 1 入力ごとに spawn | Tantivy ページング (Searcher snapshot 固定) + token matching (post-filter で Tantivy STORED 原文を引く) + streaming 送信 |
| タグ書き込みワーカー | `std::thread` (常駐) | 1 | UI の Toggle / Clear を serial に処理: XMP 書込 → 共有 writer で Tantivy upsert (タグ含む全 STORED 原文を更新) → 32 件 or 500ms でバッチ commit |
| タスクトレイ (v0.9) | `std::thread` (常駐) | 1 (設定 ON 時のみ) | `mimv-tray` スレッド。`tray-icon` クレートで隠し HWND を作成 → `PeekMessageW` ポンプ (50ms 周期) + `TrayIconEvent` / `MenuEvent` の try_recv → `TrayEvent::Open / TogglePause / Quit` を UI に送信。`ActivityGate::set_paused` + `GlobalIoSemaphore::set_throttled` はメインスレッドで適用 |

**rayon は通常サムネイル生成には使っていない** (逐次ワーカーの方がキャンセル制御しやすいため)。

---

## 2. スレッド間通信

### 2.1 共有アトミック

| 名前 | 型 | 書き手 | 読み手 | 用途 |
| --- | --- | --- | --- | --- |
| `cancel_token` | `Arc<AtomicBool>` | UI (フォルダ切替) | 全ワーカー | 停止シグナル |
| `scroll_hint` | `Arc<AtomicUsize>` | UI (スクロール) | サムネワーカー | 優先度計算の基準 |
| `keep_start_shared` / `keep_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 範囲外の要求を破棄する境界 |
| `visible_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 可視範囲の終端 (exclusive)。先読み forward 側の距離計算に使用 |
| `display_px_shared` | `Arc<AtomicU32>` | UI (設定変更) | サムネワーカー | 生成時の目標ピクセル数 |
| `cache_gen_done` | `Arc<AtomicUsize>` | キャッシュ生成 rayon | UI | 進捗カウンタ |
| `SupervisorHandle.cancel` | `Arc<AtomicBool>` | UI (お気に入り OFF, App drop) | メタ / 名前索引 supervisor | supervisor 全体の停止シグナル |
| `GlobalSearchHandle.cancel` | `Arc<AtomicBool>` | UI (クエリ変更, バー閉じ, folder 遷移, Handle drop) | Ctrl+G クエリワーカー | Tantivy ページングループの中断 |
| `tag_write_worker.cancel` | `Arc<AtomicBool>` | App drop | タグ書き込みワーカー | 書込ループ + commit の中断 |
| `NativeVideoOutput.source_epoch` | `Arc<AtomicU64>` | UI / native presenter | UI / native presenter | native presenter 再利用時の stale event 防止。`SwitchSource` ごとに epoch を進め、presenter から UI へ送る `NativeVideoOutputEvent` に付与する。UI は現在の player epoch と一致しない event を破棄する |
| `VideoDynamicState.present_path` | `Arc<AtomicU8>` | native-video-presenter (= `record_present`) | UI (右パネル overlay 描画) | per-frame のプレゼン経路 (Pending / GPU / CPU)。`d3d11_shared` なら GPU、`cpu_upload` なら CPU を store。デインターレース ON で CPU 経路に落ちた場合の右パネル「フレーム表示」表示根拠 |
| `VideoDynamicState.deinterlace_status` | `Arc<AtomicU8>` | video-decode (`run_video_decode`) | UI (右パネル overlay 描画) | bwdif フィルタの動的状態 (Pending / Inactive / Active / Failed)。フィルタ初期化成功 → Active、失敗 → Failed、Auto モードで素材プログレッシブ判定 → Inactive、seek 直後 → Pending。Settings = Off は decode 開始時に Inactive |
| `VideoDynamicState.interlace_detected` | `Arc<AtomicBool>` | video-decode (`run_video_decode`) | UI (右パネル overlay 描画) | `stream_interlaced || frame_interlaced` の latched 検出。一度 true になったら同 source 再生中は維持 (= 微小な interlaced フレーム混入でも表示安定)。`VideoPlayer::open` ごとに新 Arc 生成で false 初期化 |
| `ActivityGate.paused` (v0.9) | `AtomicBool` | UI (トレイメニュー「一時停止」 / ウィンドウ hide) | `wait_until_idle` を呼ぶ全ワーカー (walker / ingest / name_bulk_indexer) | true の間 wait ループが解除 or cancel まで抜けない。cancel は貫通 (終了時の固まり防止)。Ctrl+G 検索中は paused ではなく `bump()` を継続して、検索完了後に通常の quiet threshold で自然再開させる |
| `resident_media_wake_enabled` / `resident_media_wake_pending` | `Arc<AtomicBool>` | App / tray thread | tray thread / App | enabled は既存 media owner の再生継続 projection。pending は posted `WM_PAINT` 1 件の claim で、tray thread は false→true を取れた場合だけ投函し、`App::update` 入口が false に戻す。可視化・enabled=false・投函失敗でも reset し、未消化 wake を高々 1 件に保つ |
| `GlobalIoSemaphore.throttled` (v0.9) | `Mutex` ガード | UI (ウィンドウ hide/show) | 全インデクサ worker | true の間、実効 permit=1 (in_use ≥ 1 なら新規 acquire 不可)。解除で `notify_all` |

**ルール**: アトミックは単発の値伝搬にのみ使う。リスト/辞書の共有は `Arc<Mutex<...>>` か mpsc。

### 2.2 チャネル

| 名前 | 方向 | 内容 |
| --- | --- | --- |
| `tx / rx` (App) | ワーカー → UI | `ThumbMsg`: (idx, ColorImage, `ThumbLoadOrigin`, source_dims, canceled, finalized)。origin は `Source` / `UpgradeableCache` / `FinalCache` の 3 状態で、最後は編集 preview・drive-list・再帰 pin WebP など元ソースへ idle upgrade しない完成済み派生 cache。from-source 経路 (cache miss) では **2 シグナル**: ① 第 1 シグナル = display ColorImage (canceled=false, finalized=false) → UI は Loaded 化、`requested` は保持 ② 第 2 シグナル = cache save 完了通知 (None + finalized=true) → UI は `requested` を抜くだけで **`thumbnails[i]` は変更しない**。cache hit は 1 ショット (canceled=false で即 remove)。`canceled=true` は STALE 専用 (worker bail-out) で、UI は Evicted に戻して再試行可能にする (Failed にしない)。`finalized=true` と `canceled=true` は排他 |
| `fs_pending[idx].1` | フルスクリーンスレッド → UI | `FsLoadResult`: **DimsOnly (非終端) / Static / Animated / Failed**。`DimsOnly` はヘッダ解析直後に先行送信される原寸ヒントで、UI は `fs_early_dims` に積み fs_pending は維持する (本デコードが続く)。詳細は [display-pipeline.md §2.2](display-pipeline.md) 参照 |
| `ai_upscale_pending[idx].1` | AI スレッド → UI | `UpscaleResult` |
| `final_effect_pending[key].rx` | カラー化 / 最終エフェクト worker → UI | `FinalEffectResult::Ready { pixels, elapsed_ms, timing }` / `Cancelled`。`timing` は色調補正、smart sharpen、カラー化判定／適用、Creative LUT、post filter の段階別時間を保持する。receiver は composite key、画像 index、`items_generation`、表示 / 先読み区分と同じ viewer context に保持し、stale 結果を texture cache へ公開しない。先読み結果も UI 側で完成 texture を 1 枚ずつ upload する |
| `local_adjust_pending[idx].rx` | local / conceal materialization worker → UI | `EditMaterializeResult`。request は `items_generation`、`input_seq` と、local なら `LocalAdjustResultKey`、conceal なら `EditResultKey` を持つ。pending map は `ViewerContextBundle` 所有で context の swap / park とともに移動し、bundle drop / clear では cancel token を立てる。受信時は pending request identity と現在の generation key の両方を検証してから cache へ公開する |
| `export_pending.rx` | Ctrl+E エクスポート worker → UI | `ExportEvent`: `Started` / `Completed` / `Failed` / `Cancelled` / `AllDone`。UI は毎フレーム `try_recv` で進捗モーダルを更新し、エラーがあればモーダルを残す |
| `pdf_enumerate_pending` | PDF 列挙スレッド → UI | `(pages, password_needed)` |
| PDF ワーカー stdin/stdout | UI プロセス ↔ PDF ワーカープロセス | 長さプレフィクス付きバイナリプロトコル (Enumerate / Render / Shutdown) |
| Ctrl+G `SearchStreamEvent` | Ctrl+G ワーカー → UI | `Batch { hits, scanned_candidates, valid_hits }` / `Done { truncated, reason }` / `Error`。毎フレーム `try_recv` を MAX_EVENTS_PER_FRAME=8 までループ消費 |
| `DebouncedChange` (notify-rs) | FsWatcher → supervisor | 500ms ウィンドウで集約した変更イベント (`favorite_id`, `path`, `ChangeKind`) |
| `SupervisorCommand` | UI (`IndexerManager`) → supervisor | 一時停止 / 再開 / フル再スキャン要求 |
| `IndexerManager.writer` | 全書き込み経路で共有 | `Arc<FtsWriterDispatcher>` — Tantivy は Index あたり writer 1 本制約。専用ディスパッチャースレッドが優先度キュー (Interactive > Background) でジョブを直列処理する。 ingest worker (Background) と tag_write_worker (Interactive) は `WriterJob::Upsert` / `Delete` / `Commit` / `Batch` を `submit` するだけで、writer に直接触らない (§5.5)。 |

### 2.3 ワーカーキュー

| キュー | 型 | 内容 |
| --- | --- | --- |
| `reload_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | 通常サムネイル要求 (Image/ZipImage/PdfPage に加え、PdfFile のフォルダ代表画も IPC 待ちのためここに振る)。**スクロール中 / visible 待ち中は `prefetch_allowed_now` gate で `req.priority=false` の prefetch enqueue が抑制され、queue 内の既存 prefetch も `q.retain` で prune される** (= PDF pool に prefetch が流れる前に止めて in-flight 占有を防ぐ、docs/prefetch-suppression-during-scroll-plan.md) |
| `heavy_io_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | Folder/ZipFile/ConvertibleArchive/ZipDir 要求 (本物の同期 I/O または ZIP 内 prefix の代表解決)。Folder の再帰 pin 伝播で行う既存 catalog の read-only open / exact-row WebP lookup もここで実行し、UI スレッドへ SQLite cold open を持ち込まない。reload_queue と同じ prefetch suppression gate を共有 |
| `pdf_pool.queue` | `Arc<(Mutex<JobQueue>, Condvar)>` | PDF ワーカーへのレンダ/列挙要求。`critical` / `high_normal` / `normal` VecDeque + `normal_in_flight` + `workers_busy` + `in_flight_started_at: Vec<Option<Instant>>` (POOL_SIZE 固定、worker_id index) を同一 Mutex で保護。dispatcher は `critical → high_normal → normal` の順で pop し、HighNormal + Normal で `normal_in_flight` 枠 (= `worker_count - 1`) を共有する。**`CRITICAL_RESERVATION_ACTIVE` (v1.0.0 から常時 ON、最低 1 ワーカーを Critical 用に予約)** によってグリッドからの `Enter` (= Critical な `enumerate_pages_async`) がサムネ先読みの in-flight 待ちで詰まらないようにする。HighNormal は `req.priority=true` の可視セル用 (= 画面に見えているサムネ render を画面外先読みより先に処理)。**Context epoch (`CURRENT_CONTEXT_EPOCH`)** で UI ナビゲーション (フォルダ移動 / Ctrl+G 結果差替え) ごとに HighNormal/Normal ジョブを世代管理し、bump で stale を一括 prune + dispatcher pop 時にも stale 判定。Critical と epoch=0 (background) はプルーン対象外。**`CancelWaitPolicy::HarvestOnCancel`** (thumbnail PDF render の cache-savable 経路のみ) では cancel が立っても in-flight IPC の reply を待ち、PDFium が既に処理した render 結果を harvest して cache 保存に進ませる (= 再エントリ時の再 render 地獄を防ぐ)。**`promote_to_high_normal`** で App 側がスクロール後の現可視 PDF を Normal lane から HighNormal lane に昇格 (= prefetch として enqueue された後で可視になったジョブを救う) |
| `CatchupQueue` (`thumb_loader.rs`) | `Arc<(Mutex<CatchupQueueState>, Condvar)>` | `pdf_meta` 背景書き込みキュー (v1.0.0)。`high: VecDeque<NeighborPrefetch>` (cap 16) + `low: VecDeque<MetaOnly>` (cap 256) + `pending: HashSet<PathBuf>` を同一 Mutex で保護。worker は high → low の順で pop。同 path が low にいる時に高優先が後から来ると **`high` 空き確認後に `low` から remove → `high` に push** で昇格する (lane が満杯のときだけ drop、lane 間は独立)。詳細は [docs/pdf-page-count-cache-plan.md の「最終形」セクション](pdf-page-count-cache-plan.md) |
| `AiJobQueue` (`app.rs`) | `Arc<(Mutex<AiJobQueueState>, Condvar)>` | final AI (upscale/denoise) ジョブ。`display: VecDeque` (表示中ページ, push_front=LIFO) + `prefetch: VecDeque` (先読み, push_back=FIFO) + `shutdown` を同一 Mutex で保護。`final-ai-worker` が `display → prefetch` の順で pop。enqueue 重複は呼び出し側 `final_ai_pending.contains_key` で dedup。cancel は `final_ai_pending[key].cancel` (Drop でも立つ) を worker が pop 時に確認し、立っていれば推論せず `Cancelled` を返す (= 高速ページ送りで keep_set 外になったジョブは GPU 推論が始まる前に止まる)。PDF の表示中 final AI だけは、保持 LRU に入れる価値が高いので session close / keep-set evict 時に最大 1 件まで `retained_final_ai_orphans` へ移し、live pending から外したまま完走を許可する |
| `texture_backlog` | ローカル Vec (App) | GPU アップロード未完の ColorImage。MAX_TEXTURES_PER_FRAME=8 超過分 |

ワーカーが要求を取り出すときは **優先度 (priority フラグ) → 距離 → forward/backward** でソート。
距離計算は可視範囲の端からの歩数: backward は `scroll_hint - idx`, forward は `idx - visible_end + 1`
で、同距離では forward (次ページ方向) が先。これは `fs_cache` 先読み / AI アップスケール先読み /
サムネイルグリッドワーカーの全てで統一されており、`+1, -1, +2, -2, ...` の順 (forward 先) となる
(共通ヘルパ: `interleaved_prefetch_targets`)。

### 2.4 GlobalIoSemaphore (I/O 横断調停)

`src/io_semaphore.rs`。PDF ワーカー (5 プロセス) / サムネイル背景ジョブ /
インデクサ (walker + ingest) が同時に HDD をシークすると UI スクロールがつまる。
これを防ぐため、**全ワーカー横断で同時 I/O 数を優先度付きで制限する**。

| 優先度 | 用途 |
| --- | --- |
| `High` | UI が今見ているフォルダ / ページ (UI 経路、PDF critical) |
| `Normal` | PDF 背景レンダ、通常サムネイル |
| `Low` | インデクサ (メタ / 名前、速度プロファイルで 1 / 2 / 4 permit) |

実装は `Mutex + Condvar` で自前 (`try_lock + sleep` 禁止、§5.5 参照)。permit の
drop で自動 release + `notify_all` で起床。spurious wakeup 耐性のため条件は
`while` ループで再確認。

**飢餓ポリシー (明示)**: High が連続投入される間 Low は無制限に待つ。これは
UI 応答性最優先という方針の意図的な選択。アイドル 数秒で High キューが空き、
Low が進む。不足する場面は「AC 電源時のみインデックス」等の別機構で制御する。

---

## 3. キャンセル規約

### 3.1 フォルダ切替時

`load_folder()` が呼ばれたら:

1. 旧 `cancel_token` に `true` をセット
2. 新しい `cancel_token` を作って `Arc` を差し替え
3. 旧 mpsc 受信は drop (新しい tx/rx に置き換え)
4. 新しいワーカーを新トークン付きで spawn
5. 各種キャッシュ (`fs_cache`, `adjustment_cache`, `ai_upscale_cache`, `rotation_cache` …) をクリア

**旧プールを毎回捨てる**のが肝。同じプールを使い回さないので競合を気にしなくてよい。

### 3.1.5 フォルダナビゲーション (Ctrl+↑↓) のキャンセル + アキュームレート

Ctrl+↑/↓ はフォルダツリーを DFS で辿って次の「画像/動画/ZIP/PDF/変換アーカイブがあるフォルダ」を
見つけるが、キーリピート (30Hz) で連打すると、過去は毎プレスで新スレッドを spawn +
旧スレッドに cancel を投げる設計だった。ただし `navigate_folder_with_skip` 自体は
cancel を見ていなかったので、cancel 済みスレッドも DFS を最後まで走り切り、
並行 DFS が FS を奪い合って単発 DFS が 200ms → 1s 級に遅延する事故を起こしていた
(2026-04 セッションで実測、PDF だらけの scan フォルダで顕著)。

現在の挙動 (2026-04 修正後):

- `navigate_folder_with_skip` と `folder_should_stop` は `Option<&AtomicBool>` を受け取り、
  各 DFS ステップとディレクトリエントリ走査のたびに cancel をチェックする。旧スレッドは
  cancel 検出時点で `None` を返して即終了 → FS 競合が消える。
- `start_folder_nav` は in-flight 中の追加プレスを `pending_folder_nav_steps: i32` に
  累積する (forward=+1 / backward=-1)。**新スレッドは spawn しない**。
  累積は `±MAX_PENDING_NAV = 5` で飽和する (それ以上のプレスは捨てる) ので、
  キーを離した後に「離したのに動き続ける」違和感が出ない (drain は最長 ~500ms)。
- 現 nav が完了 → `apply_folder_nav_result` がモード別後処理 → `chain_folder_nav_if_pending`
  で累積が残っていれば 1 消費して次のステップ (新しい current からの DFS) を連鎖起動する。
- 連打中に別経路のナビ (click / favsearch / address / BS) が入ると累積はクリアされ、
  in-flight もキャンセルされる (`load_folder` → `start_loading_items` の既存処理)。

これにより 30 回連打は 30 ステップ分の DFS を逐次的に進める (並行ではなく直列)。
各 DFS 間で cancel チェックが入るので、途中で方向が反転しても即座に対応できる。

#### 3.1.5.1 モード別後処理 (grid / fullscreen / favsearch / smart folder)

Ctrl+↑↓ は複数の起点から発火し、DFS 完了時に異なる後処理が必要になる。同じ非同期
パイプラインで扱えるように `FolderNavMode` をキーにして後処理を分岐している:

| モード | 発火元 | DFS 完了時の処理 |
| --- | --- | --- |
| `Grid` | `navigate()` の Ctrl+↑↓ (通常グリッド) | `load_folder_nav_target(p)` のみ。RAR/7z/LZH などは変換ダイアログまたはキャッシュ経由で開く |
| `Fullscreen` | `handle_fs_navigation` の Ctrl+↑↓ (フルスクリーン中) | `close_fullscreen` → `load_folder_nav_target(p)` → `open_fullscreen(先頭 image-like idx)` |
| `Favsearch { root, fullscreen: false }` | `favsearch_ctrl_nav` (お気に入り検索中) | `is_under(p, root)` が真なら `load_folder_nav_target + nav_stack.push + update_favsearch_address`、偽なら `favsearch_navigate_sibling(±1)` |
| `Favsearch { root, fullscreen: true }` | フルスクリーン中の Ctrl+S スコープナビ | 上記に加えて `close_fullscreen` → `open_fullscreen(先頭 image-like idx)` でフルスクリーンを維持 |
| `SmartFolder { state, fullscreen }` | スマートフォルダ root / scoped drill のグリッド、リング、ゲームパッド、フルスクリーン | `entry_root` 内だけを DFS。端では root snapshot の表示順で前後 entry へ移る。着地前に `SmartFolderViewState` を更新し、`fullscreen=true` は先頭 image-like を再表示 |

実装上の要点:

- `FolderNavPending { cancel, rx, forward, mode }` が進行中 DFS の状態を持ち、
  `poll_folder_nav` が `FolderNavResult { path, forward, mode }` を返す。
- Windows の `ViewerContextBundle` は `FolderNavPending`、通常画像 open 用の typed
  `FolderPaneOpenPending`、`pending_folder_nav_steps`、`pending_folder_nav_mode`、
  `ViewerNavigationScope` を一式で所有する。main と
  `DetachedPhysical` の active independent 静止画窓は bundle swap で完全に分離され、
  active detached の結果はその bundle を mount 中の
  `update_active_detached_viewer_context` だけが poll / apply する。pause / Drop は
  cancel token を立て、遅延結果を sibling context へ持ち越さない。
- 仮想一覧から通常画像を detached open するときは、親フォルダの `read_dir` / metadata scan を
  `FolderOpenScanPurpose::DetachedImage { image_path }` として worker へ出す。完了後に
  pre-scan 付き `load_folder_with_scan` で完全な物理一覧を materialize し、保持した path を
  新しい index へ解決するため、main の検索部分集合や並び順を再利用しない。pending は
  active detached context の生存条件でもあり、遅いディスクやネットワークフォルダで scan が
  複数フレームにまたがっても session / runtime とともに保持する。明示 target は Windows の
  path identity で厳密に解決し、一覧に存在しなければ先頭へ fallback せず request を正常終了する。
- `apply_folder_nav_result` がモードに応じて分岐。Fullscreen ブランチで
  `close_fullscreen` を呼ぶが、そこは既に `folder_nav_pending = None` なので
  再帰的な自己キャンセルは起きない。
- DFS の結果が変換アーカイブファイルの場合は `load_folder_nav_target` が
  `load_folder_or_convert_archive` に振り分ける。変換確認ダイアログや無視結果では pending を解除し、
  その時点ではフルスクリーン再オープンや検索 nav_stack 更新を行わない。
- 連鎖ステップでも同じモードを引き継ぐ。`pending_folder_nav_mode` を現在 mount 中の
  viewer context に保持し、
  `chain_folder_nav_if_pending` がそれを参照して次の `spawn_folder_nav` に渡す。
- Favsearch モードでは起点フォルダが `nav_stack.last()` なので、連鎖時には
  `current_folder` ではなくスタックトップを使う。
- SmartFolder モードでは worker に渡した state snapshot だけで範囲を判定する。連鎖時は
  `TopLevelGridView` の最新 scoped current / entry index から mode を作り直し、古い entry へ戻らない。

モード境界のキャンセル:

- ユーザーが ESC / 右クリックでフルスクリーンを抜ける → `close_fullscreen` が
  走行中の Fullscreen モード nav を検出してキャンセル + pending クリア。
- Favsearch を閉じる (`close_favsearch`) や favsearch_back → `load_folder` 経由で
  `start_loading_items` が folder_nav_pending を一律キャンセル。
- モード違いで `start_folder_nav` が呼ばれた場合 (理論的エッジケース) は、
  旧 DFS をキャンセルしてから新モードで仕切り直す。

### 3.1.6 動画タイルモード中の動画→動画切替

Windows native presenter 有効時、動画タイルモード中にホイールで隣の動画へ移動する場合は
通常の fullscreen reopen 経路ではなく fast path を使う。

処理順序:

1. 旧 `VideoPlayer` から `NativeVideoOutput` を取り外す。
2. 新 `VideoPlayer` を `native_output_config=None` で構築する。
3. 新 player の `SwitchSourcePayload` を既存 `NativeVideoOutput` に送る。
4. 新 player に native output を attach し、`fs_cache[target_idx]` に入れる。
5. `fullscreen_idx` と overlay / metadata 同期を新 idx へ更新する。
6. 最後に旧 video entry を remove する。

旧 entry の remove を最後にする理由は、旧 decoder を先に drop すると presenter thread が
まだ旧 `video_rx` を見ている間に sender が close し、SwitchSource 到着まで disconnected
状態を経由するため。source binding を先に差し替えてから旧 player を shutdown する。

`video_tile_swap_pending` は新動画の `player.info()` 到着を待つ UI 側 pending state。
pending 中は追加ホイール入力を捨て、queue も delta 累積もしない。これは Ctrl+↑↓ の
ロックと同じく、ユーザーが操作を止めたあとに溜まった移動が遅れて発火しないようにするため。
`info()` が来たら新しい `VideoTileState` を構築し、来なければ既存 reopen 経路へ fallback する。

### 3.2 フルスクリーン / AI のキャンセル

1 枚ごとに `Arc<AtomicBool>` を `fs_pending[idx]` / `ai_upscale_pending[idx]` に持たせる。
要求を取り下げるときは個別にこのフラグを立てる。
ワーカーは大きな処理の合間 (タイル推論の各タイル、フレームデコード直後、など) でフラグを確認する。

### 3.2.1 final AI パイプライン (upscale/denoise) の単一ワーカー + 優先度キュー

現行の final AI 経路 (`maybe_start_final_ai` / `final_ai_pending` / `final_ai_cache`) は、
**ジョブごとに `std::thread::spawn` していた旧設計を `AiJobQueue` (単一ワーカー + 優先度
キュー) に置き換えてある**。背景は「フルスクリーンを高速にめくりながら 4x アップスケールを
連発すると UI が 15 秒級にフリーズ (`UI THREAD HANG`)」という実害 (2026-06 のクラッシュ
ログ)。原因は 2 つで、本キュー化で両方を断つ:

1. **UI スレッドが推論ロックを待っていた**: `AiRuntime` は `sessions: Mutex<HashMap<…,
   Session>>` を `session.run()` 実行中ずっと握る (= 全推論が単一 Mutex で直列化される)。
   旧 `maybe_start_final_ai` は **UI スレッド上で** `is_loaded` / `load_model` を呼んで
   いたため、推論 backlog がロックを握りっぱなしのとき UI スレッドが飢餓状態になっていた。
   → **モデルロード (`load_model`) と推論はすべて `final-ai-worker` 上で実行**し、UI
   スレッドはモデル「種別」決定 (`model_path` 存在チェックのみ、sessions 非接触) と
   enqueue だけを行う。`run_final_ai_job` / `ensure_model_loaded` が worker 側の実体。
2. **ジョブ滞留に上限が無かった**: 通過したページの display ジョブは止めず (キャッシュを
   埋める意図)、`maybe_start_final_ai` のゲートは「同じ idx の二重起動」しか防がないため、
   高速ページ送りで spawn 済みスレッドが無制限に積み上がっていた。→ 単一ワーカー +
   キューにすることで「実行は常に 1 件、残りはキューで待つ」になり、cancel 済みジョブは
   pop 時に推論せず `Cancelled` を返す。`evict_final_pipeline_cache` が keep_set 外の
   pending に cancel フラグを立てれば、GPU 推論が始まる前に捨てられる。

優先度とキャンセル規約:

- **優先度**: `fullscreen_idx == idx` の display ジョブは `display` lane に **push_front
  (LIFO)** で積む (= 最後に表示したページを最優先)。先読みは `prefetch` lane に
  **push_back (FIFO)**。worker は `display → prefetch` の順で pop。
- **キャンセル**: `final_ai_pending[key].cancel: Arc<AtomicBool>` を共有。
  `cancel_final_ai_for_idx` / `evict_final_pipeline_cache` / `clear_*` が立てる。
  `FinalAiPending` の Drop も cancel を立てる。worker は pop 直後とタイル境界
  (`upscale` / `denoise` 内) で確認する。
- **PDF retained orphan**: PDF ページの display ジョブは、`close_fullscreen()` や
  keep-set eviction で live 表示対象から外れても、保持 LRU が有効なら最大 1 件だけ
  `retained_final_ai_orphans` に `FinalAiKey + job_id` で移して cancel せず完走させる。
  これは「完了前キャンセルで retained LRU に入らない」問題を避けるための例外で、live
  `final_ai_cache` には戻さず stable item key 付きの `retained_final_ai_cache` だけへ保存する。
  外部変更や AI 設定変更で retained epoch が進んだ古い結果は store 時に捨てる。
- **結果回収**: 全ジョブ共有の単一 mpsc (`final_ai_rx`)。`poll_final_ai` が毎フレーム
  drain し、**pending に残っている key** または **retained orphan として追跡中の key** の
  結果だけを、どちらも `job_id` 一致を確認してから適用する。通常の取り消し済み key の結果は
  捨てる (= 旧 per-thread 設計で rx drop により失われていたのと同じ挙動。stale な
  `final_ai_cache` 挿入を防ぐ)。
- **同時起動ポリシー**: 先読みは `prefetch_final_ai` が `has_uncancelled_final_ai_pending`
  で gate するため、未キャンセル pending がある間は新しい先読みを enqueue しない
  (= キューに先読みが溜まりすぎない)。display ジョブはこの gate の対象外で即 enqueue。

> 注: fullscreen session をまたぐ final AI pixels は `retained_final_ai_cache` で
> 枚数 + MiB の LRU 管理を行う。これは CPU 側の推論結果保持で、表示中の
> `final_ai_cache` / `final_composite_cache` や GPU テクスチャの keep-set eviction とは別層。
> PDF retained orphan はこの retained layer へ store するためだけの例外であり、表示中
> キャッシュ / GPU 常駐分を延命するものではない。残る課題は、表示中キャッシュ / GPU 常駐分の
> バイト予算と、高速ページ送り中の AI 起動デバウンスである。

### 3.2.2 edit materialization の ownership / generation / cancel / upload

通常の local-adjust / conceal materialization pending は `ViewerContextBundle` の
`local_adjust_pending: HashMap<idx, LocalAdjustRenderPending>` が所有する。active detached /
parked context を mount し直すと pending も cache と一緒に移り、別 context の read-only close が
sibling の要求を cancel または drain してはならない。context が破棄されれば pending の Drop で
cancel token が立つ。layer bypass / prefix preview の別 lane はこの context-owned map とは別物である。

結果採用は 2 段階で行う。まず receiver から来た request 全体が、同じ idx の現在 pending request
(request identity。`input_seq` を含む) と一致することを確認する。次に
`items_generation` が現在値と一致し、local は現在の `LocalAdjustResultKey`、conceal は現在の
`EditResultKey` と一致するときだけ cache へ insert / texture upload する。cancel が間に合わず
worker が完走しても、この検証で旧 folder、旧 input、旧 erase / local / conceal generation の
結果は公開されない。

cancel 境界は次の 3 箇所をそろえる:

1. **同じ idx の新要求**: slot を置き換える前に旧 pending の token を立てる。
2. **無効化 / context 終了**: input・mask・local・conceal generation の更新、folder / fullscreen
   lifecycle、context pause / drop で owner が該当 pending を cancel する。
3. **worker 内**: DB read / decode の境界、local layer / mask resize、conceal raster / compose など
   長い処理の途中でも token を確認して早期終了する。

worker は CPU pixels までを作り、`ctx.load_texture` は UI スレッドに残す。ただし
`last_edit_materialize_upload_frame` を App 全体の frame budget とし、local / conceal、layer bypass /
prefix preview、`edit_result` texture 化を合わせて **1 フレーム 1 枚**だけ upload する。ready result が
複数あれば表示中ページを優先し、残りは pending の `ready` に保持して repaint を要求する。
materialization 待ちの edit / final assembly は `None` を返し、未完成の上位レイヤーを作らない。

### 3.3 フルスクリーン読み込みの優先度制御

`start_fs_load` はプールを持たない使い捨て `std::thread::spawn` なので、素朴に先読みを
並列起動すると現在表示中の画像のデコードが先読みスレッドに CPU を奪われて遅延する。
これを防ぐため `update_prefetch_window` は以下のルールで動く:

1. 現在画像が `fs_cache` に入っていない (デコード中) 間は、**他の全ての pending スレッドを
   キャンセル**する (KEEP 範囲内でも)。現在画像が CPU を独占する。
2. 同時に、先読みの新規 spawn も **延期**する。
3. `poll_prefetch` が現在画像の完了を検出したら、再度 `update_prefetch_window` を呼び、
   そこで初めて先読みが起動する。

AI アップスケール (`maybe_start_ai_upscale`) も同様: 同時実行は 1 枚のみで、現在画像が
来たら古い先読みをキャンセル。**ただしこれは旧 `ai_upscale_*` 経路 (`#[allow(dead_code)]`)
の記述。現行の final AI 経路は §3.2.1 の `AiJobQueue` (単一ワーカー + 優先度キュー) を使う。**

消しゴム MI-GAN (`ui_erase.rs`) は `erase_inpaint_pending[(idx, kind)]` で管理する。
`kind` は `Preview` / `Commit` の 2 種で、preview 押下が同じ idx の commit ジョブを
キャンセルしないように分離している。commit は投入時の `input_generation` と
`erase_mask_generation` を保持し、完了時は `fs_cache` ではなく
`erase_result_cache[EraseResultKey]` に書き戻す。入力やマスクが変わったときは該当
commit pending を cancel し、古い結果が表示レイヤへ昇格しないようにする。
worker は結果チャネルとは別の進捗チャネルで、モデル準備 / パス・タイル番号 / 合成 /
diffusion fallback を UI へ通知する。UI は pending が存在する間だけ持続ステータスを描き、
保存済みマスクの自動再生成を含めて、短時間トーストが消えた後も処理中であることを示す。

### 3.4 サムネイルワーカーの STALE 取消と重複エンキュー抑制

サムネイルは「keep_range 内かどうか」が毎フレーム変化するため、単純なキャンセルでは
**同じ idx が in-flight なのに scroll 戻りで再エンキューされ、PDF 再レンダが二重に走る**
事故を起こす。2026-04 のセッションで以下のルールを確立した:

- **`update_keep_range_and_requests` は `self.requested` を範囲外一括 remove しない**。
  ワーカー処理中の idx まで抜いてしまい再エンキューを誘発するため。step 1 は Loaded→Evicted
  の遷移だけ行う。
- **`requested` の cleanup 経路は 4 本**:
  1. エンキュー済・pop 前の取消 → step 2 の `q.retain` が dropped idx を `requested.remove`
  2. ワーカー pop 後の STALE → ワーカーが `ThumbMsg` に `canceled=true` を載せて送信 →
     `poll_thumbnails` が `requested.remove` + `Evicted` (Failed にしない)
  3. cache hit 正常完了 → 第 1 シグナルで `poll_thumbnails` が `requested.remove`
  4. cache miss 正常完了 → **第 1 シグナル (display ColorImage) では remove しない**、
     第 2 シグナル (cache save 完了、`finalized=true`) で remove。`finalized=true` の場合は
     `thumbnails[i]` を **変更しない** (texture アップロード待ちの Pending を Evicted に
     書き換えると次フレームに再エンキュー → 重複デコード地獄になる事故を防ぐ)。
     旧実装は STALE と同じ `canceled=true` で送信していたため、texture_backlog に
     アップロード待ちが詰まっている時に Pending → Evicted の上書きが起きていた。

     **さらに finalized-vs-backlog レース**: 第 2 シグナルが先着したが第 1 シグナルの
     ColorImage は `texture_backlog` に積まれてアップロード待ち、というケースで
     即 `requested.remove` すると `Pending && !requested.contains` → 次フレーム再エンキュー
     の無限ループになる。対策として `pending_finalize: HashSet<usize>` を追加し、
     finalized 受信時に thumbnails[i] が **Pending のとき** は idx を pending_finalize
     へ積む。アップロード完了 (新規 or backlog から) で Loaded 化した瞬間に
     `pending_finalize.remove(&i)` が true を返せばその場で `requested.remove` する。

     **さらに finalized-vs-evict レース (v0.7.3)**: 第 1 シグナル到着後にユーザーが
     スクロールして keep_range から外れると、`update_keep_range_and_requests` が
     Loaded → Evicted に落とす (この時点では `requested` は意図的に残す = cache-save 完了
     待ち)。その直後に第 2 シグナルが届くと、旧実装は state=Evicted でも pending_finalize
     に積んでしまい、pending_finalize は Loaded 遷移時にしか掃除されないため、
     `requested[i]` が永久に居座る。スクロールで戻ってきても再エンキューループの
     `if requested.contains_key { continue; }` に弾かれてサムネが Evicted のまま固着する。
     対策として finalize ハンドラを 3 分岐に分け、**Evicted / Failed のときは
     `requested.remove + pending_finalize.remove` で即時掃除**する (ワーカーは第 2 シグナル
     送信済みなので「処理中の idx を抜くな」の規約には違反しない)。ログ `[poll] finalize
     on Evicted idx=N → cleanup requested` で発動を可視化する。
     `canceled` / 失敗 / `load_folder` リセット時にも pending_finalize をクリア
- **STALE チェックはワーカーパイプラインの 3 箇所**:
  - `spawn_worker` が pop 直後 (app.rs): キャッシュ lookup すら不要な明白な範囲外
  - `process_load_request` の heavy I/O resolve 後 (thumb_loader.rs): ZIP/folder の
    I/O (秒単位) 完了後に範囲外になっていないか
  - `process_load_request` の PDF レンダ直前 (thumb_loader.rs): cache miss で PDFium
    に投げる前。これがないと scroll 往復で同じページの 1 秒レンダが重複する
- 3 箇所とも `canceled=true` を送信して `requested` cleanup する。`continue` だけでは
  `requested` に残って「再エンキューされない idx=Pending」状態で固まる。

**なぜ 2 シグナル方式か**: `load_one_cached` は decode → tx.send (display) → WebP encode →
DB save → cache_map.insert の順で処理する。もし第 1 シグナル到着時に `requested` を抜くと、
cache save 進行中 (数百 ms) は `requested` 空かつ cache_map にも未登録の窓が開き、
その間に scroll 往復が起きると別 worker が同じ idx を cache miss 扱いで取得し重い decode
(ZIP 取り出し・PDFium レンダ等) を二重に走らせる。第 2 シグナルで cache save 完了後に
初めて `requested` を抜くことで、cache save 中の再エンキューは `requested.contains_key=true`
で弾かれる。

### 3.4.1 検索系のキャンセル規約

| ワーカー | 発火元 | シグナル |
| --- | --- | --- |
| Ctrl+G クエリワーカー (`global_search::run`) | クエリ変更 / フィルタ変更 / バー閉じ / folder 遷移 / `GlobalSearchHandle` drop | `Arc<AtomicBool>` を Tantivy ページングループ頭と post-filter ループ頭で check。pending/debounce 中は App が `ActivityGate::bump()` を継続し、背景インデクサの walker/ingest を次 checkpoint で待たせる |
| IndexerSupervisor (メタ / 名前) | `IndexerManager::sync_with_favorites` で OFF 化、App drop | `SupervisorHandle::stop()` → cancel + FsWatcher drop + thread join (最大 ~250ms) |
| walker / ingest (supervisor 内部) | supervisor cancel | 各ループ checkpoint で `Ordering::Relaxed` read。大ファイル走査中も数百 ms 以内に抜ける |
| tag_write_worker | App drop | `None` 送信 + cancel フラグ。commit 後のループ先頭で check |

**Tantivy writer 共有ルール**: `IndexerManager.writer: Arc<Mutex<IndexWriter>>` を
ingest worker と tag_write_worker が共有する。独自に `fts.writer()` を呼ぶと
`LockBusy` で **全 upsert が無効化される**。新しい書き込み経路を足すなら必ず
共有 writer を使う。

#### Indexer shutdown の有界化 (v2.3.0 第12弾)

- App drop は全 supervisor に cancel を先行送信し、全 supervisor 合計 4 秒の
  manager-wide deadline までだけ join する。期限を超えた JoinHandle は detach し、
  プロセス終了を将来追加される長時間処理でも塞がない。
- walker は entry / 3-way diff の各ループ、ingest は delete / ingest の各ループで cancel を見る。
  GlobalIoSemaphore の permit 待ちと FtsWriterDispatcher の reply 待ちは50ms timeoutで再確認する。
  cancel 後の未flush batchはsubmitせず、受信済みを含む watcher queue も処理しない。
- final commit と dispatcher の最終所有は indexer-writer-finalizer へ移し、main thread は
  commit / queue drain / dispatcher join を待たない。submit済みbatchの応答をcancelで放棄しても、
  Tantivy First により SQLite側を先行更新しない。次回起動時の FS / Tantivy / fts_meta
  3-way diff が未反映・片側反映を再照合するため、detachは索引整合性を犠牲にしない。

### 3.5 新ワーカー追加時のテンプレ

```rust
let cancel = Arc::clone(&self.cancel_token);  // フォルダ単位のキャンセル
let my_cancel = Arc::new(AtomicBool::new(false));  // 個別キャンセル (必要なら)
let tx = self.tx.clone();
std::thread::spawn(move || {
    // 大きな処理の合間で両方チェック
    if cancel.load(Relaxed) || my_cancel.load(Relaxed) {
        return;
    }
    // ... 処理 ...
    let _ = tx.send(result);
});
```

送信失敗 (受信側 drop) は無視する。フォルダ切替で既に捨てられているだけ。

---

## 4. GPU テクスチャ予算

### 4.1 keep_set ベースの退去 (display list vs filesystem list)

mImageViewer は 2 つのリストを使い分ける:

| 変数 | 役割 |
| --- | --- |
| `App::items: Vec<GridItem>` | **filesystem list** (ソース)。raw idx はフォルダセッション中 stable。 |
| `App::visible_indices: Vec<usize>` | **display list**。`items` への参照を ★フィルタ / 検索フィルタ通過後だけに絞ったもの。 |

**prefetch / eviction / retain 系のループは必ず display list (の部分列) を使う**。
`items` の raw idx 連続範囲 (`keep_start..keep_end`) を直接回してはいけない。
`visible_indices` が疎になっているとき (例: 1300 フォルダ中★5 のみ 3 件可視) に、
連続範囲を回すと非可視の 1000 件近くを prefetch キューに流し込んでしまう。

具体像:

- **`App::keep_range: (usize, usize)`** — `keep_set` の bounding box。worker 側で
  atomic に読める `keep_start_shared` / `keep_end_shared` の値を供給する。
  疎な keep_set に対しては「広め」の判定になるので、worker が稀に非可視 idx を
  掴んでしまうが、main thread 側で enqueue しなければほぼ発生しない。
- **`App::keep_set: HashSet<usize>`** — 実際に prefetch / 保持したい idx 集合。
  `visible_indices[vis_keep_start..vis_keep_end]` から毎フレーム構築する。
  enqueue / eviction / retain / idle upgrade / tag prewarm / 補正テクスチャの
  バックグラウンド処理はすべてこの `keep_set.contains(&i)` で判定する。
- **eviction は前回 keep_set との差分だけを処理する** — 同一 `items_generation` では
  `previous_keep_set - current_keep_set` の idx だけを `Evicted` にする。`items` 全件を
  毎フレーム走査すると、数百万件の一覧では可視セル描画が仮想化されていても UI thread が
  停止する。並べ替え等で Loaded を別 idx へ再利用する可能性があるため、items 世代ごとの
  初回だけ `thumbnail_eviction_generation` を使って全件照合する。

新しく「可視範囲の画像に対する背景処理」を追加する場合:

1. 反復対象は `self.keep_set.iter()` (必要なら `sorted` に clone してから)。退去処理は
   前回 keep_set との差分を使う。
2. `keep_start..keep_end` の range ループは絶対に書かない。
3. `rebuild_visible_indices()` は `keep_set` を直接触らない。次フレームの
   `update_keep_range_and_requests` が再計算する (フィルタ変更で疎になっても
   1 フレーム遅れで自然に収束する)。

### 4.2 通常ロードの流れ

- 可視範囲 + prev/next ページ分のみ GPU に保持 (`keep_set` の範囲)
- 範囲外に出た瞬間に `TextureHandle` を drop (eviction)
- `egui_ctx.load_texture` でアップロードするコマ数を MAX_TEXTURES_PER_FRAME=8 に制限
- 超過分は `texture_backlog` に積んで次フレーム以降に処理

### 4.3 共有 VRAM pool と会計

- `gpu_info.rs` で検出した VRAM に `gpu_memory_percent` を掛け、mImageViewer 全体で 1 つの
  pool を作る。検出失敗時は 4 GiB を仮定し、設定 0% は無制限として上限を作らない
- 一覧中はサムネイル 80% / フルスクリーン表示系 20%、フルスクリーン中は 20% / 80% とする。
  比率は内部定数であり、モード移行時に専用の全破棄は行わない
- 連結読みの HIGH はフルスクリーン配分 byte を RGBA8 の 4 byte/texel で割った値、LOW は
  HIGH の 75%。HIGH 超過でだけ trim を開始し、可視ページと準備帯を除く遠方 unit を LOW
  以下まで外す。無制限時は HIGH/LOW と trim/admission の両方を無効化する
- サムネイル保持帯はモード別のサムネイル配分を使い、ロード済み texture の実寸が上限を
  超えた場合に display list 上の区間を縮める。フルスクリーン中の中心は一覧のスクロール位置
  ではなく `fullscreen_idx` に追従する

`app/vram_accounting.rs` は `fs_cache` から表示系の各派生 cache、連結 transition、
`thumbnails` / `thumb_textures` / `thumb_adjust_tex` までを横断する。推定表示サイズではなく
`TextureHandle::size()` と完全な mip chain を使い、同じ `TextureId` は cache をまたいでも
1 回だけ数える。チェック柄・スタンプ・font preview など小さく有界な texture は対象外とする。

`--perf-log` が有効な場合だけ、約 1 秒に 1 回 `gpu.vram_accounting` を記録する。イベントには
モード、全 texel/byte、共有 pool と両配分の上限、無制限フラグ、および subsystem ごとの
texel/byte を含める。会計のための I/O・decode・GPU readback は行わず、UI thread 上では既存の
handle metadata を走査するだけに限定する。

新しいテクスチャキャッシュを追加する時は、会計対象か小さく有界な対象外かを決め、対象なら
accountant と所有モードの配分へ登録すること。

---

## 5. よくある事故パターン

### 5.1 キャンセル忘れ

新機能を作った時、`cancel_token` を参照し忘れると、フォルダ切替後もゾンビとして動き続ける。
→ 最悪 mpsc が満杯になるか、UI に古い結果が届く。必ずテンプレに従う。

### 5.2 キャッシュの部分更新

「補正は変わったけど AI は変わってない」のような時、`adjustment_cache` だけクリアして
`ai_upscale_cache` を残す。両方同時に消すと AI の再実行 (数秒) が発生してユーザーを待たせる。
詳細は [preset-and-adjustment.md](preset-and-adjustment.md) の無効化ルール表。

### 5.2.1 items 差し替え / 削除時の世代 bump 忘れ

`items` / `thumbnails` / `image_metas` を書き換える全経路で `items_generation` の
bump + idx ベース状態の破棄を忘れずに行う。忘れると、進行中ワーカーが旧 idx 向けに
生成した `ThumbMsg` が新 items の同じ idx に着地して**サムネが化ける**。

現在の経路と使うヘルパー:

- **フォルダ切替**: `start_loading_items` → `install_new_items`
- **Ctrl+G 結果差し替え**: `replace_search_view_items` → `install_new_items` +
  `invalidate_idx_state_and_queues` + path-keyed cache clear
- **削除**: `start_delete_files` (ゴミ箱移動を別 thread で実行) → 完了時に
  `poll_delete_pending` が path から現在の idx を引き直して `remove_items_batch` を
  呼ぶ。`remove_items_batch` は降順 idx 配列を受け取り、items/thumbnails/image_metas の
  物理 shift + `items_generation` bump + `adjustment_page_params` / `mask_pages` /
  `search_filter` の O(K log K) idx shift + `invalidate_idx_state_and_queues` を行う。
  ゴミ箱移動は `delete_worker` が Windows Shell `IFileOperation` へ最大 100 件ずつ
  `DeleteItem` を予約し、チャンクごとに `PerformOperations` を 1 回だけ呼ぶ。チャンク完了の
  `DeleteMsg::Batch` は recycle 進捗として即時通知する一方、Shell 成功 path と PDF candidate は
  worker 内へ蓄積し、全チャンク完了後 (途中 cancel なら recycle 済み成功分まで) に
  `rename_key_migration::STORES` の全 path-keyed SQLite 行を **1 回だけ** hard purge する。
  `Done` は purge / journal 永続化の後に送るため、UI の items 除去と presence set /
  rating・tag・rotation 等の in-memory clear は永続 purge より後になる。キー照合は exact +
  `<key>/` + `<key>::`。hash key の PDF password は保存行が 1 件以上ある場合だけ、削除前に
  worker が配下 PDF を列挙して hash を確定する。ストア不在 / 空なら read_dir 走査を丸ごと省く。
  第21弾では SQLite purge の exact を最大500 parameterの `IN` batchへまとめ、`<key>/` /
  `<key>::` は `col >= prefix AND col < next(prefix)` の BINARY index range scanへ変更した。
  `next(prefix)` は末尾 Unicode scalar を1つ進め、構築不能時だけ旧 `substr` 条件へfallbackする。
  対象列は既定 BINARY collationのPRIMARY KEY、複合PK先頭列、または明示path indexであり、
  keep-drive / drive-strippedの既存小文字化キーをそのまま境界に使う。1ストア1transactionと
  `Done`後送の順序は変えず、削除数×総行数の全表scanだけを除いた。
  UI 側は DB I/O を行わず、missing の観測からこの purge 経路は呼ばない。
  初回 + 3 回の purge 後もエラーが残れば path 単位で `delete_purge_journal.json` に永続化し、
  `delete-purge-retry` が起動時と後続 idle 時に同じ共通 purge を冪等再実行する。PDF password 用の
  削除前列挙 path も entry に保持する。sidecar backup は `flush()` 成功時だけ purge rows に数える。
  mIV 側の削除確認 / 進捗 UI を正とするため通常の Shell UI は抑制しつつ、
  ゴミ箱不可時の完全削除警告 (`FOF_WANTNUKEWARNING`) は残す。mIV 側キャンセルは
  チャンク間で判定する。Shell 側の中断は `GetAnyOperationsAborted` と削除後の
  存在確認でチャンク内の失敗として反映し、それだけでは未処理チャンクを捨てない。

新しい差し替え経路を増やすときは、必ず以下を揃える:

1. `items_generation` を必ず bump (install_new_items 経由か直接 +1)
2. `invalidate_idx_state_and_queues()` を呼ぶ — requested / idle upgrade の idx memo /
   pending_finalize /
   texture_backlog / checked / keep_range / keep_set / keep_*_shared / idx-keyed
   HashMap 群 / in-flight pending (fs_pending / ai_upscale_pending) / reload_queue /
   heavy_io_queue を一括で片付ける
3. path-keyed キャッシュ (metadata_cache / exif_cache / xmp_cache / tags_cache) も
   items が総入れ替わりする経路ではリセット (部分削除ならリセット不要)
4. `items.remove` / `items.push` を直接書かない — 必ずヘルパー経由に通す。
   レビューでは `rg 'self\.items\.(remove|push|clear)'` で直接触っていないか確認する

この設計が崩れると 2026-04 に発生した「削除後に別 item のサムネが表示される」
「Ctrl+G 直後に重い ZIP/PDF decode が worker を占有して新結果のサムネが来ない」
といった再発しやすいバグが戻ってくる。

### 5.2.2 孤児メタデータ整理 worker

`metadata_cleanup.rs` はスキャン結果を候補 snapshot として UI へ返し、ユーザー確認後に別 worker で
削除する。候補条件は `try_exists() == Ok(false)` かつ物理実体の直上親が `is_dir()` の場合だけ。
切断ドライブ等で親ごと見えない行は `Protected` として残す。スキャン後にドライブ状態が変わる race を
避けるため DELETE 直前にも同じ判定と本棚除外を再実行する。cancel は scan では候補全体を破棄し、
delete では処理中 descriptor の transaction を rollback する。完了通知は削除済み exact key だけを
返し、UI 側は SQLite I/O をせず rating count / path cache / presence set を無効化・更新する。

削除 purge journal はこの全走査 UI を自動実行するものではない。Shell 削除成功済み path だけを
ピンポイント対象にし、`STORES` と `PathClassification::Orphan` の安全条件を共有する。再作成済みの
同名 path や親へ到達不能な path は新しいメタを誤削除しないよう journal に残して繰り延べる。

### 5.3 UI スレッドで重処理

`App::update` 内で CPU 重めの処理をすると fps が落ちる。
- 補正の LUT 計算: 軽いので同期 OK (`maybe_apply_adjustment`)
- AI 推論: 絶対に別スレッド
- 画像デコード: 絶対に別スレッド
- **GPU 上限超過画像のリサイズ**: 2026-04 に 7168×9216 の PNG をフルスクリーンで開くと
  UI が 10 秒近く固まる事故があった。`clamp_for_gpu(&ColorImage)` を UI スレッドで
  呼ぶと ColorImage → DynamicImage への premultiply 往復 (ピクセル毎ループ) と
  `resize_exact(Triangle)` が同期で走って 1 発 5 秒級になる。`start_fs_load` の
  worker 側で `clamp_dynamic_for_gpu(DynamicImage)` を先に掛ける方針に変更し、
  `fs_cache` / `ai_upscale_cache` / `adjustment_cache` の `Static.pixels` は
  **常に 8192px 以内** という不変条件に格上げした。UI スレッドの `clamp_for_gpu`
  は異常経路の安全網として残してあるが、通常パスでは `Cow::Borrowed` で返り
  リサイズは走らない。発動したらログに `clamp_for_gpu (UI-thread fallback)` が出る。
  詳細は [display-pipeline.md §2.2](display-pipeline.md) 参照。

**Ctrl+↑↓ / Ctrl+F / Ctrl+S / open_fullscreen の UI ブロック事件 (2026-04)**:
ファイル読込・SQLite・GPU アップロード・read_dir など、一見軽そうな処理が per-operation
で 100ms超ブロックする事例が複数判明した。対策と設計方針は
[ui-responsiveness.md](ui-responsiveness.md) にまとめてある。新機能追加前に
§4 チェックリストを必ず見ること。Windows 特有の罠として `Path::is_dir()` が
per-entry で `GetFileAttributes` syscall を呼ぶ件も記載。

### 5.4 PDF ワーカー / Susie ワーカーの想定外終了

ワーカープロセスがクラッシュしたら、親は検出して再起動する仕組みになっている。
新しい PDF / Susie 操作を追加する時はタイムアウト処理を忘れずに (stdout 読み取りで詰まらない)。

**Susie プラグインの並列実行に関する注意**: Susie 画像プラグインは 1990〜2000 年代の
レガシー規格で、並列実行 (特にプロセス跨ぎ) を想定していないプラグインが稀にある。
別プロセス隔離によりスレッド不安全性は解消されるが、以下は残る:

- 一時ファイル衝突 (固定名で temp を書くプラグイン)
- INI / レジストリの race 書き込み
- プラグインが間接ロードする外部 DLL にプロセス跨ぎのロックがある場合

対策として `Settings::susie_allow_parallel = false` でプールサイズを 1 に固定する
オプションを用意している。環境設定 → Susie プラグイン → 「プラグインを並列実行する」
チェックで切り替え可能。問題プラグインの切り分けはユーザー側に委ねる方針。

**Susie プール初期化の race (v0.7.0 修正済み)**: `susie_loader::supports_extension()`
は初期は `try_get_pool()` (プール未初期化時は None→false を返す) で判定していたが、
起動直後に Susie 対応拡張子を含む ZIP / フォルダを開くと、バックグラウンド init
スレッドの完了前に列挙が走って PI / MAG / Q0 などが無視されていた。
`get_pool()` (未初期化ならブロック) に切り替え、一度だけ数百 ms ブロックして
プールを取得する方式に変更。ネイティブ拡張子は `is_recognized_image_ext` 内の
`SUPPORTED_EXTENSIONS.contains` でショートサーキットされるためここに到達しない。
Susie を無効化していれば `get_pool()` は即座に empty プールを返すので無害。

**Susie プールキューの 2 レベル優先度**: `Job::priority` フィールドで可視セルかどうかを
区別し、`SusieWorkerPool::execute(req, hint, priority, cancel)` の `priority=true` 引数で
キュー先頭 (`push_front`)、`false` でキュー末尾 (`push_back`) に挿入する。

スクロール中の動作:
- 既に Susie キューに居座っていた古い (画面外) ジョブは末尾側
- 新しく投入された可視セルは先頭側 → ワーカーが次に pop する
- 結果として **画面外ジョブを待たずに可視セルが処理される**

priority 内の順序は LIFO (後着の push_front が先着を追い越す) になるが、可視範囲内で
あればどのセルから埋まっても体感上問題ないため許容。完全な FIFO 内優先度が必要に
なったら priority 用のサブキューを足すと良い。

`thumb_loader::process_load_request` から `req.priority` を `load_one_cached` に
渡し、その中の `decode_file` / `decode_bytes` 呼び出しに伝播。フルスクリーン読み込み
は常に priority=true (現在表示中の画像)。

**Susie 1 ジョブごとの計測ログ (環境変数で ON/OFF)**: 環境変数
`MIV_SUSIE_PERF_LOG=1` を設定して起動すると、各 Susie デコード呼び出しごとに
`mimageviewer.log` へ次の形式で計測ログを出す。サムネイル一括ロード時に
何が遅いか (キュー待ち / IPC 自体 / プラグイン処理) を切り分けるため。

```
susie: w0 OK  P ext=mag    queue=  0.4ms ipc=   12.3ms req=64B resp=512080B
```

- `w0` … ワーカー番号 (0..2)
- `queue` … `execute()` で enqueue された時刻 から ディスパッチャが pop した時刻
- `ipc` … `write_msg`+`read_msg` の合計 (32bit ifmag.spi 等のプラグイン処理時間も含む)
- `req`/`resp` … バイナリフレーム長
- `P`/`-` … priority フラグ (P=可視セル、-=背景ロード)

常時 ON だと数千件のサムネイルロードでログが膨大になるため、調査時のみ手動で
ON にする運用。GUI 設定には出していない。

### 5.5 try_lock + sleep ポーリングループ (禁止パターン)

「`Mutex` を `try_lock` して、失敗したら sleep して再試行」というループは、**複数スレッドが
同じ Mutex を奪い合う場面では飢餓 (starvation) を起こす**。10ms の sleep 中に fresh arrival が
割り込んで Mutex を横取りできるため、先に待ち始めたスレッドが秒単位で待たされる。

2026-04 に PDF ワーカープールで実際にこの現象が発生し、Critical 要求が 10 秒ブロックされた
(1 ワーカーに 62 件の連続ディスパッチが集中、他の 2 ワーカーは完全にアイドル)。

**代わりに使うべき設計**: **Mutex + Condvar で保護した優先度キュー + 専用ディスパッチャー
スレッド**。

```rust
// リソース要求側 (UI スレッド等)
fn execute(&self, job: Job) -> Result<R> {
    let (tx, rx) = mpsc::channel();
    {
        let (mtx, cv) = &*self.queue;
        let mut q = mtx.lock().unwrap();
        q.push(job);           // critical / normal などにソート
        cv.notify_one();       // ディスパッチャーを 1 つ起こす
    }
    // タイムアウト付き受信で cancel チェックを挟む
    rx.recv_timeout(Duration::from_millis(50))
}

// ディスパッチャースレッド (ワーカーごとに 1 本)
fn dispatcher(queue: Arc<(Mutex<JobQueue>, Condvar)>, resource: Resource) {
    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown { return; }
                if let Some(j) = q.pop_with_priority() { break j; }
                q = cv.wait(q).unwrap();    // Condvar で起床
            }
        };
        // Mutex 外でリソースを使って処理
        let result = resource.process(job);
        let _ = job.reply.send(result);
    }
}
```

**この設計の利点**:
- 同一優先度内で **FIFO 公平性** (Condvar が queue に並んだ順で起こす)
- 10ms ポーリングの無駄なスピン消費がなくレイテンシも低い
- ワーカー選択が「先に空いた方の勝ち」ではなく「空いた瞬間に push されたジョブを pop」になる
- `shutdown` フラグと `notify_all()` だけで停止シグナルが全スレッドに伝わる
- cancel は pop 時と requester 側 (`recv_timeout` ループ) の両方でチェック可能

実装は `src/pdf_loader.rs` の `PdfWorkerPool` / `JobQueue` / `run_dispatcher` を参照。

**いつ try_lock を使って良いか**: 非ブロッキングな best-effort 取得 (「取れたら使う、取れなければ
今回は諦める」) のみ。`try_lock` の後に sleep して再試行する構造は避ける。

---

### 5.6 タスクトレイ常駐 + インデクサ throttle / pause (v0.9)

「ビューワ特性上、使い終わったらアプリを閉じる」 → notify-rs が止まり、次回起動時に
初回スキャンが必要になる問題への対策。ウィンドウの `[×]` でプロセス終了する代わりに
タスクトレイへ格納し、notify-rs を継続走行させる。

- **エントリポイント**: `src/tray.rs` (常駐スレッド) + `src/tray_integration.rs` (App メソッド)
- **単一インスタンス保証**: `src/single_instance.rs` の `Global\mImageViewerInstance_v1`。
  `installer/mimageviewer.iss` の `AppMutex` と一致させることで、インストーラが自動で
  「閉じてください」ダイアログを出してくれる (常駐中に DLL 上書きが失敗するのを防ぐ)。
- **ウィンドウ hide/show**: `App::maybe_intercept_close` が `viewport().close_requested()`
  を検出して `ViewportCommand::CancelClose` に差し替え、Win32 `ShowWindow(SW_HIDE)`
  で隠す。`ViewportCommand::Visible(false)` は eframe/winit の `App::update` を止め、
  トレイメニューから復帰できなくなるため使わない。トレイメニュー「開く」や
  トレイアイコン左クリックはトレイスレッド側から `ShowWindow(SW_SHOW)` +
  `SetForegroundWindow` を直接呼ぶ。
- **インデクサ throttle**: `hide_to_tray` から `IndexerManager::set_io_throttled(true)`。
  `GlobalIoSemaphore` の実効 permit を 1 に絞るため、ユーザーが選んだ速度プロファイル
  (Low/Medium/High) に関係なく常駐中は 1 permit 相当になる。`show_from_tray` で解除。
- **インデクサ pause (オプトイン)**: 設定 `pause_indexer_while_minimized = true` のときだけ、
  `hide_to_tray` から `ActivityGate::set_paused(true)`。既存の `wait_until_idle` は
  paused 中ループブロックし、`show_from_tray` で解除されると通常動作に戻る。
  **cancel は paused を貫通** させる (アプリ終了時に supervisor スレッドが固まらないため)。
- **media / viewport hide**: `hide_to_tray` は transport を変更せず、mounted `fs_cache`、active detached
  bundle、全 ParkedLive bundle、source-swap pending の native output へ FIFO の
  `SetWindowVisible(false)` を送る。presenter は typed `WindowHostState::Hidden` と consume-and-hold へ入り、
  decoder channel を drain して最新 frame を保持する。close を横取りした `App::update` は早期 return せず、
  active fullscreen/F12 と passive/ParkedLive viewport を同じ ID で登録したうえで `Visible(false)` にする。
  これにより egui host HWND とその WS_CHILD presenter を teardown しない。単体音楽は presenter 無しで同じ
  transport を継続し、元から paused / EOF ならその intent も維持する。
- **resident media wake の背圧**: hidden root HWND は通常 repaint で `App::update` を起こせないため、
  再生 intent / 連続 EOF 遷移中だけ tray thread が 50ms pump から posted `WM_PAINT` を送る。
  `resident_media_wake_pending` の false→true claim に成功した 1 件だけを許し、`App::update` 入口の
  ack まで次を投函しない。可視化・wake 不要化では pending を reset し、`PostMessageW` 失敗時は
  claim を戻して失敗状態への遷移時だけログを出す。
- **GPU リソース**: `App::release_gpu_resources` は media entry、稼働中 decoder/presenter lease、DComp/swap
  chain、最新 frame を保持する一方、mounted context の非 media `TextureHandle` と cache 所有の D3D11VA
  frames ref、idle shared output slot、video processor cache を解放する。detached session は既存 keepalive policy
  どおり active viewer cache も保持する。不可視中も decode は full rate で続くため CPU/GPU/電力コストを負う。
  VST3 bridge / plugin chain は停止しない。
- **media restore**: `sync_after_restore(ctx)` は retained viewport と visual presenter を Visible へ戻し、
  hold した最新 frame を present して既存 focus grace / focus restore を再適用する。Play / Pause / seek / recreate
  は送らない。復帰時の外部フォルダ変更と main-focus consumer は既存の detached / switching 述語を共有し、
  running / paused / EOF context と typed placement completion を閉じない。
- **UI heartbeat watchdog**: `App::update` は SW_HIDE 中に止まるため、`hide_to_tray` で
  watchdog を suspended にし、復帰時に resume する。これにより正常なトレイ常駐を
  `panic.log` の `UI THREAD HANG suspected` として記録し続けない。detached viewer
  session が開いている場合は、別 viewport / native presenter の event pump を継続するため
  heartbeat を suspend しない。

設計上の注意:
- notify-rs の crossbeam-channel (unbounded) は paused 中も受信し続けるので、溜まった
  イベントは復帰時にスパイク的に処理される (OS 側の `ReadDirectoryChangesW` リングバッファ
  overflow リスクは notify-rs が即ドレインするので増えない)。
- throttle 有効化で既存 permit holder は revoke しない (drop まで維持)。hide 直後に 1 permit
  分の処理が残るが、通常は数百 ms で収まる。
- トレイ常駐中の `quit_requested` は `[×]` 乗っ取りロジックを貫通させるため、先に立ててから
  `ViewportCommand::Close` を送る。

---

## 5.6 親コンテナ代表サムネピン (folder thumb pin) の UI スレッド経路

`folder_thumb_pins.db` のアクセスは **UI スレッド同期**で行うが、各操作は cheap な
single-row I/O に収めてあるため `cargo run --perf-log` でも hitches を起こさない。

| 操作 | スレッド | 頻度 | 内容 |
| --- | --- | --- | --- |
| `lookup_many` | UI (load_folder 内) | フォルダロード 1 回ごと | 親コンテナ N 件分のピンを 500 件 chunked IN クエリで一括取得し `App::folder_pin_map: HashMap` に格納。N=数百でも `<5ms` |
| `set` / `remove` | UI (アドレスバー 📌 / 右クリックメニュー) | ユーザー操作 1 回ごと | single-row INSERT/DELETE。`folder_thumb_pin_dirty = true` を立てるだけで再ロードは別経路 |
| `apply_folder_thumb_pin` の `pin_map.get(&key)` | UI (`make_load_request`) | 親コンテナアイテム 1 つにつき 1 回 | DB ヒットなし (HashMap lookup のみ)。pin source 解決時に **追加で 1 回 `std::fs::metadata`** (target ファイル) を取る点だけ注意 — Folder pin source でサブフォルダを再帰探索する場合は worker thread 側の `resolve_folder_thumb_image` に委譲する |
| `seed_folder_video_pin_thumbs` | UI (load_folder 内) | フォルダロード 1 回ごと | folder_pin_map の Video pin だけ走査 → `video_pins.db` lookup → 既存 cache_map と byte 比較 → 差分があるときだけ `catalog.save_thumb_bytes` (single-row UPSERT)。典型的なフォルダで 0〜数件しか該当しない |

**再ロードトリガ**: 書き換え反映は `App::consume_folder_thumb_pin_dirty` が
`folder_thumb_pin_dirty` を take して `load_folder` を呼ぶ。これは `App::update` の
`render_address_bar` 直後 (= fullscreen でないとき) と `close_fullscreen` の両方から
拾うので、UI クリックの 1 フレーム後にグリッドへ反映される (egui の auto repaint と
連動)。fullscreen 中は load_folder が close_fullscreen を呼ぶため、抜けるまで dirty を
保留する (Codex Phase D P2 指摘の対応)。

手動 pin が固定の Image / ZipEntry / PdfPage leaf へ解決された場合、親用
`LoadRequest` に leaf の canonical `edit_preview_key` も載せる。ZIP/PDF の親要求は
page size を持たないため、編集 preview 保存 worker が container size を別記録し、
thumbnail worker が `LoadRequest.path` を stat して container mtime + size で検証する。
UI thread に archive 列挙・追加 stat・edit preview DB lookup は置かない。Saved /
Invalidated 通知は per-context の `thumb_edit_preview_keys` を使い、直接ページと同じ leaf を
固定している親セルをまとめて evict / reload する。

**Codex Phase D P2 (drill-down dead pin) 対応**: `archive_source_override.is_some()`
(= RAR/7z/LZH の変換キャッシュ ZIP を drill-down 中) では UI 経路の `compute_folder_pin_
button_state` / `render_folder_pin_menu_entry` が `None`/false を返してエントリ自体を
出さない。キャッシュ ZIP に書いてもユーザーに到達しないため。

---

## 6. 参考 (実測値)

`docs/archive/performance-refactoring/bench-scroll-report.md` に詳細あり。要点:

- キャッシュヒット時のサムネ読み込み: 2〜3 ms/枚
- PDF レンダリング: 5 ワーカー並列 (うち 1 を Critical 予約) で Cold 1441ms → 10ms (2 枚目以降)
- JPEG デコード: turbojpeg + DCT scale (1/8〜1/1) でサムネ用 5-30MB カメラ JPEG を 2.5-6× 高速化 ([docs/dct-scale-plan.md](dct-scale-plan.md))。128MB 超は image crate / WIC にフォールバック
- キャンセル遅延: 最大 1 枚デコード分 (数百 ms)

---

## 7. パフォーマンス計装 (perf.rs)

「キー入力 → 画面表示」レイテンシを後から解析するための構造化イベントログ。
既存 `logger.rs` (人間可読) はそのまま残り、`perf.rs` が JSON Lines を別ファイルに書く。

### 7.1 有効化

- **CLI 引数**: `mimageviewer.exe --perf-log` を付けたときのみ ON
- **無効時のコスト**: `perf::is_enabled()` の Atomic 1 回読みのみで `perf::event` は即 return
- **出力先**: `%APPDATA%\mimageviewer\logs\perf_events.jsonl` (起動毎に truncate)

### 7.2 `input_seq` の伝搬規約

`App` が `input_seq: u64` を持ち、**ユーザー入力イベント発生時のみ** `bump_input_seq()` で +1 する。
フレーム境界では増えない。0 は「相関なし」として予約。

| 発火箇所 | 種別 | 備考 |
| --- | --- | --- |
| `ui_fullscreen.rs::render_fullscreen_viewport` | `fs_key` / `fs_wheel` / `fs_close_*` | nav_delta / wheel_nav / close が確定した直後 |
| `app.rs::handle_keyboard` | `grid_key` | カーソルキーで selected が変わった時 |
| `app.rs::process_scroll` | `grid_wheel` / `grid_cols` | スクロールオフセットまたは列数が変わった時 |
| `app.rs::open_fullscreen` | `fs_open` | フルスクリーン遷移 |

**ワーカーへの伝搬**: UI スレッドは enqueue 時点の `input_seq` をタスク構造体にコピーする。

- `thumb_loader::LoadRequest.input_seq` — サムネイルワーカー用
- フルスクリーン非同期ロード: `start_fs_load` が `perf_seq` をクロージャにムーブする
- AI アップスケール / 色調補正ジョブ: 同様にクロージャへ
- PDF ワーカー IPC は seq=0 (プロセス間相関は現状非対応)

### 7.3 イベント構造

```json
{"t":12.345,"tid":5,"cat":"fs","kind":"paint","key":"C:\\a.jpg","seq":42,"idx":3}
```

主なカテゴリ:

- `input`  — ユーザー入力 (seq が振られる唯一のカテゴリ)
- `frame`  — 毎フレーム begin。`n` はフレーム番号
- `fs`     — フルスクリーン画像: `load_begin` / `decode_begin` / `decode_end` / `ready` / `paint`。`final_effect_worker` は `worker_ms` に加えて `colorize_check_ms` / `colorize_apply_ms`、各補正段、`clamp_ms` / `load_texture_ms`、`colorize_applied`、方式・設定スケール・長辺換算後の実効スケール、`prefetch` / `complete` を記録する。完成済み final composite を drop 後 30 秒以内に同じ key で再生成した場合は `final_effect_recompute` に `idx` / `age_ms` / `drop_reason` / 窓内・累計件数を記録し、窓内 8 件超では 30 秒に 1 回だけ通常ログへ警告する。先読み admission の拒否は `final_effect_prefetch_blocked` に `idx` / `reason` (`not_in_keep_set` または `over_low_watermark`) / `loaded_texels` / `low_watermark` を記録する。同じ idx・理由は 1 秒に 1 回へ間引き、理由変更時は即時記録する
- `thumb`  — サムネイル: `enqueue` / `pick` / `skip` / `decode_begin` / `decode_end` / `ready`。アイドル高画質化の最終判定は `idle_upgrade_enqueue` / `idle_upgrade_ineligible` に `key` / `idx` / `items_gen` を載せ、同一状態の反復を検出できるようにする
- `pdf`    — PDF ワーカー IPC: `pool_send` / `pool_recv` / `inproc_*` / `enumerate_send`
- `ai`     — AI: `upscale_begin` / `upscale_tile` / `upscale_end` / `denoise_*` / `job_start` / `job_ready`
- `ui`     — UI フレーム: `tail_repaint` / `slow_frame_breakdown` / `pre_grid_breakdown`。
  `pre_grid_breakdown` は `n` / `total_ms` と、検索・お気に入り・タグ・ファセット・遅延状態・
  下部情報・フォルダペイン・選択 overlay・scroll routing・stack reconcile の各 `*_ms` を持つ
- `folder_pane` — 左フォルダツリーペイン: `scan_subfolders` (子ディレクトリ列挙の ms / 件数 / cancel)

### 7.4 解析

`scripts/analyze_perf.py` で集計。主要サブコマンド:

```bash
python scripts/analyze_perf.py <path>/perf_events.jsonl summary   # 件数/カテゴリ breakdown
python scripts/analyze_perf.py <path>/perf_events.jsonl latency   # seq → ready/paint ms
python scripts/analyze_perf.py <path>/perf_events.jsonl priority  # 優先度違反検出
python scripts/analyze_perf.py <path>/perf_events.jsonl thumbs    # decode 時間分布
python scripts/analyze_perf.py <path>/perf_events.jsonl colorize  # カラー化の解像度・方式別時間
python scripts/analyze_perf.py <path>/perf_events.jsonl pre-grid  # グリッド直前の要素別時間
python scripts/analyze_perf.py <path>/perf_events.jsonl dump 42   # 特定 seq の全イベント
python scripts/analyze_perf.py <path>/perf_events.jsonl timeline  # ガントチャート (matplotlib)
python scripts/analyze_perf.py <path>/perf_events.jsonl idle-health --start-t 10 --end-t 25
```

### 7.5 アイドル健全性

`idle-health` は静止区間の `frame.begin` 件数、`ui.tail_repaint` reason の継続時間、同一
thumbnail work の反復を検査し、閾値超過で exit 1 を返す。アプリが正常に sleep すると
区間内 event が 0 件になるため、wall time はプロセス外から `--start-t` / `--end-t` で渡す。
空区間は既定では窓ずれとして失敗し、外部 sampler が perf log の `session.start.pid` と測定対象
PID の一致を確認した場合だけ `--allow-sleeping-window` で明示的に許可する。
CPU time とログ増加量も含むリリース前手順は
[idle-health-check.md](idle-health-check.md) を参照する。

### 7.6 新ワーカー追加時のテンプレ

1. ワーカーに渡すタスク構造体に `input_seq: u64` フィールドを追加
2. UI スレッドの enqueue 箇所で `req.input_seq = self.input_seq` を設定
3. UI 側で `perf::event("<cat>", "enqueue", key, self.input_seq, &[...])` を emit
4. ワーカー側で `perf::event("<cat>", "begin"/"end", key, req.input_seq, &[...])` を emit
5. Ready 遷移 (texture upload 完了) で `perf::event("<cat>", "ready", ...)` を emit
6. `docs/async-architecture.md` のこの表にエントリを追加
