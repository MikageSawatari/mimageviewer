# 音声サムネイル: 同名 sidecar・MP3 埋め込み画像の計画 (§1.347)

作成: 2026-10-07 / Line D (`next-audio-art`)。v4.4.0 後の §1.347。
改訂: 2026-10-08 / 84258858a の P2 二件を確認。利用者が先頭タグ限定を決定し、Rust gate から実装再開。
**製品実装・指定自動検証完了、確認用build完了・先頭 ID3v2 タグ限定は利用者決定・Q1訂正 / Q2〜Q7を保持・実アプリ未起動。**
利用者から伝達されたレビュー履歴: 前版は **ACCEPT**、R4 の改訂は **ACCEPT WITH CHANGES**。
R6 (4fe5979e0) の再レビューは **REVISE** (永続コレクションの出所伝達 1 件)。
R7 対応後の独立レビュー受入・全利用者質問決定済みは、2026-10-08 の実装依頼で伝達された。
旧方式の停止は設計レビューの不合格ではなく、§15 の FFmpeg 実証ゲート不成立による。
**coordinator 指示 (2026-10-08): album-art 経路から FFmpeg を完全に外す。**
84258858a のレビューは REVISE (複数タグの置換 / 更新と末尾タグ配置の P2 二件)。
利用者は完全規則の実装を見送り、先頭タグだけを読む選択肢 (a) を採用し、Rust gate と製品実装の継続を指示した。
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
| ID3 対応範囲 (追加決定、2026-10-08) | **ファイル先頭の ID3v2 タグ一つだけ**を読む。そのタグ自身の extended header / frame / unsync / 圧縮は扱う。後続タグ・末尾だけのタグは無視し、複数タグの置換 / update / 末尾配置規則を実装しない。まれな素材で旧表紙やアイコンになる場合は同名 sidecar で補える |

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
| 専用 ID3 依存は現在ない | root Cargo.toml / Cargo.lock / offline metadata に id3 / lofty はない。既存 flate2 1.1.9 は rust_backend + miniz_oxide 0.8.9 (§3.1)。新案はこれと既存画像 helper を使用し、通常依存を追加しない |
| 添付 packet の ABI は利用できるが抽出には使わない | 同梱 n7.1.5-16-g9a4bb2c579 の open-only は圧縮 APIC で約229 MiB の追加 private commit (§15)。再生の ATTACHED_PIC 除外は保持し、album-art から FFmpeg init / input / FFI も呼ばない |
| 成功画像は既存 catalog に入るが、空結果の型はない | `src/catalog.rs` の CATALOG_VERSION=2、thumbnails の必須 WebP BLOB / width / height と mtime / file_size。空 BLOB や 0 寸法を no-art の sentinel にすると既存 reader と衝突する |
| 合成一覧にも Audio がある | folder_scan、tag view からの materialize、smart_folder、reading history、rating、collection prepare の Audio 分岐。サブ展開だけは `src/app/subfolder_expansion.rs` の collect 後 retain で Audio を除外している |
| 詳細 hover は既存サムネ状態を使う | `src/ui_main.rs::render_details_thumbnail_tooltip` は Loaded texture を描き、Failed は「表示できません」、その他は「読み込み中...」+ repaint。no-art の終端を追加しないと hover も止まらない |
| Remote の入口は既存 endpoint で足りる | `/api/thumb` → ThumbnailRequest → `src/remote_ipc/thumbnail.rs::ThumbnailEngine`。現在 MP3 は supported image/video でなく拒否される。app.js の createGridTile は Audio に img を付けず thumbnail binding から除外している |
| 実装前 IPC の基準は v66 | 調査時の `crates/remote-ipc/src/lib.rs::PROTOCOL_VERSION` と版 assertion は66。実装は67へ更新 (§19)。web-remote-plan §13.5 の旧「現行 v65」は実装基準にしない |
| sidecar の設定と一覧省略は別の既存 field | `src/settings.rs:4753,5691,7415,7676` の skip_image_if_video_exists / video_thumb_use_sidecar_image は既定 true。folder_scan.rs:314,831 は省略 ON なら画像行を除外し、採用も ON の場合だけ override を返す |
| sidecar 発見と Remote 出所は共通化済み | folder_scan.rs:139 の aggregate 発見、collection_grid.rs:403、smart_folder.rs:4718,4920、remote_ipc/mod.rs:77,107,133 の RemoteThumbnailSources、thumbnail.rs:549 の same-parent / stem / image guard。今は Video 限定なので Audio を同じ owner に加える |
| 表示設定には既存の 3 択 pattern がある | `src/settings.rs:617,4437,7368,9805` の VideoThumbnailIndicator、`src/settings_transfer.rs:269` の plain 分類、`src/ui_dialogs/preferences/pages.rs:1621`。現在 Remote はこの動画設定を反映せず、音声設定の配信は追加が必要 |
| 無画像応答は既に wire にある | ThumbnailErrorCode::NoThumbnail、HTTP の 422、command-core.mjs の非 retry 判定を再利用できる。調査時の HTTP error 名は miv_thumbnail_error にまとめられていた。実装は no_thumbnail を正常な NoArt に分離 (§7.2 / §19) |

バックログの表示・所有境界の主前提は保持する。FFmpeg の有界抽出という前提は §15 で不成立。
上記は実装前の前提照合。§3 の reader と割当前制限・取消は §19 の Rust gate で実証し、IPC は実装時に67へ更新する。

## 3. 抽出・表紙選択・入力上限

### 3.1 代替依存の評価と推奨 (2026-10-08、提案)

**推奨: APIC / PIC 読取だけの小さい safe Rust parser と既存 flate2 の Rust backend。**
新しい `src/audio_album_art.rs` (仮称) に本体共通関数を置き、Remote からもこれを呼ぶ。
FFmpeg の open / stream-info / packet / init / allocator / FFI は album-art 経路に入れず、
失敗時にも再生 decoder や別 parser へ fallback しない。同名 sidecar は §3.3 の既存画像経路を先に使う。

#### 版・保守・調査の限界

2026-10-08 のローカル Cargo registry (source / index) に id3 / lofty はない。
`cargo metadata --no-deps --format-version 1 --offline --locked` は成功したが、未登録 crate の最新版照会にはならない。
crates.io API への直接接続は WinError 10061 で失敗した。公式 docs.rs の公開版を下表に記す。
**crates.io 上の最新版・yank・checksum は未確認**。採用候補を変える場合は接続可能な環境で
registry API / index を取得し、固定版の Cargo.toml・LICENSE・実装を照合する。
`latest` の source cache は版が混在する (id3 tag.rs は 1.17.0、lofty の一部 API は 0.25.3)。
これらを 1.17.2 / 0.25.4 の inflate 実装を監査済みという証拠にしない。

| 候補 | 公式公開情報・保守 | MIT 製品 / Windows への影響 | 割当前制限の評価 |
| --- | --- | --- | --- |
| id3 | [公開 crate](https://docs.rs/crate/id3/latest) / [API](https://docs.rs/id3/latest/id3/) は **1.17.2 (2026-09-22)**。2026 年にも複数更新。v2.2 / 2.3 / 2.4、unsync / compression を扱う ID3 専用 reader / writer | [確認できた 1.17.0 source の license 表示](https://docs.rs/id3/latest/src/id3/stream/tag.rs.html) は **MIT**。1.17.2 の manifest / LICENSE 本文は取得できず採用前再確認。bitflags / byteorder / flate2 が通常依存、tokio は optional。Rust 経路なら追加 native DLL は不要と見込むが crt-static build は未実証 | 入力 header の外側 preflight は可能。公開 Tag 読取 API に picture 数 / 展開出力 / inflate 途中取消の hook を確認できない。確認できた tag.rs は frame を decode してから Tag に保持する。呼出後に pictures().take(16) では割当前制限にならない |
| lofty | [公開 crate](https://docs.rs/crate/lofty/latest) は **0.25.4 (2026-09-20)**。同月複数更新。多形式の metadata reader / writer | [0.25.4 manifest](https://docs.rs/crate/lofty/latest/source/Cargo.toml) は **MIT OR Apache-2.0**、edition 2024 / rust-version 1.89。既定 id3v2_compression_support は flate2 ^1.1.10 で現 lock 1.1.9 の更新を伴う。lofty_attr 等も追加。Rust backend を選べば native zlib / DLL は不要と見込むが未ビルド | [GlobalOptions::allocation_limit](https://docs.rs/lofty/latest/lofty/config/struct.GlobalOptions.html) は **現在の thread の単一 tag item** の制限で有用。ただし全 picture 数 / 合計保持 / inflate 途中の cancel hook は確認できない。read_cover_art(false) は抽出要件を満たさず、read_properties(false) だけでもこれらは解決しない |
| APIC 専用 reader | 自分で維持する範囲は header / frame walk / PIC・APIC envelope / unsync のみ。zlib と画像 codec は既存実装を使う | product と同じ MIT。**通常依存・lock・CRT・配布 DLL の追加なし**。既存 flate2 / miniz_oxide の license notice を照合する | 画像を確保する前に candidate 数・image 長を検査し、固定出力 buffer の inflate 各呼出に cancel / deadline を置ける。証明とテストの所有者が一つ (§3.2) |

lofty の options は FFmpeg の process 共通 allocator とは違い **thread-local**。
採用するなら再使用 worker で設定の復元も必要だが、その仕組みだけで picture-count 制限にはならない。
両 crate の現行圧縮実装を「無制限」と断定しない。固定版 source 全体を未取得であり、
**公開 API で要求全体の制限を証明できず、17 枚目の画像確保前に止める契約を持てない**ことが不採用理由。
外側で frame を全走査し、圧縮・unsync を復号して単一候補を合成 tag として渡す案は、
必要な自作部分がほぼ同じになり、汎用 parser と二重に検証する面が増える。fork / upstream patch も初版では推奨しない。
汎用タグ編集・非 MP3 抽出を加えない今回の範囲では、APIC 専用 reader の方が制限を保ちやすい。

license の配布条件: mIV は root LICENSE の **MIT** (root manifest の license=None とは区別)。
id3 採用ならその著作権・MIT 本文、lofty 採用なら選んだ MIT 本文と各通常依存の著作権・license を
配布 notice に含める。Apache-2.0 を選ぶ場合は LICENSE と upstream NOTICE があるか確認し必要な notice を保持する。
MIT 製品の license 自体を変える設計ではない。新案は他 crate の parser source をコピーせず仕様から記述する。
既存 flate2 1.1.9 = MIT OR Apache-2.0、miniz_oxide 0.8.9 = MIT OR Zlib OR Apache-2.0 は
ローカル Cargo manifest / metadata で確認し、選択する MIT 本文・著作権とその依存の notice を配布物で照合する。
notice の完全性を今回の調査だけで保証せず、配布資料への不足追記は実装時の co-update に含める。

`cargo metadata --offline --locked --format-version 1` と
`cargo tree -i flate2 -e features --offline --locked` は成功。
現 graph は flate2 1.1.9 の default / rust_backend / miniz_oxide / any_impl で、native zlib feature はない。
調査 facts は `target/id3-redesign-evidence/metadata-facts.json`。
`.cargo/config.toml` の Windows MSVC +crt-static を変えず、ビルドの実証は実装後の normal / portable check に残す。

### 3.2 有界 APIC reader と新しい技術ゲート

利用者採用の Q4 は保持: **ID3 領域合計 32 MiB、候補 16 枚、画像 bytes 16 MiB、
一枚 40 MP / decode 割当 160 MiB、抽出開始から 10 秒**。
下記は、その上限を割当前に検査する具体化の提案。製品 reader / 新 harness はまだ書いていない。

#### header → frame walk → APIC の最小経路

1. worker が MP3 実ファイルと fresh source stamp を確立する (§5.3)。一覧の日時を使わない。
   File の read / seek、走査・unsync・inflate に同じ request token / 開始期限を渡す。
   10 秒は協調的な期限であり、阻害不能な OS read / stat / 一回の画像 decode の強制終了保証にはしない。
   UI は join せず、期限後の結果は Failed、取消は Canceled として §4 の既存終端へ返す。
2. 最初の 10-byte ID3 header を stack buffer で読む。syncsafe の各上位 bit、version / flags、
   checked offset + length と file 範囲を確認し、**宣言 tag が 32 MiB 超なら payload 読込・heap 確保前に拒否**。
   読むのは offset=0 のタグ一つだけ。header / body / そのタグ自身の footer を含む物理領域に
   32 MiB を適用する。後続タグを preflight / 合算せず、EOF footer・ID3v1・SEEK から探索もしない。
   先頭が ID3 でなければ、末尾に正常な APIC があっても NoArt。先頭タグに表紙があればそれを使い、
   後続タグの表紙交換・削除・update flag は結果に影響させない (利用者決定 2026-10-08)。
3. header 検査後、**宣言 body 以下の固定容量 raw tag 領域を一つだけ**確保し、小 chunk で読む。
   読込と解釈の双方に同じ上限を検査し、公開前の fresh stat は §5.3 に従う。
   Q6 の同 stamp 編集の検出保証は増やさない。read_to_end、ファイル全体の mmap / read、
   入力長に合わせた無制限 Vec growth は使わない。
4. v2.2 の 6-byte PIC frame (24-bit BE size)、v2.3 の 10-byte APIC frame (32-bit BE size)、
   v2.4 の 10-byte APIC frame (syncsafe size) を各 version 専用 helper で歩く。
   frame の header / body が tag 範囲内、size > 0、offset が単調増加であることを checked arithmetic で検査。
   padding / extended header / footer の境界を分け、v2.3 の extended size は 4-byte size 自身を含まず、
   v2.4 は含む。未知の正常な frame は size で skip し、非 APIC は decode / inflate / 文字列化しない。
   不明 format flags / 暗号化 frame は安全に skip し、境界自体が壊れた tag は salvage 検索せず拒否する。
   v2.2 の仕様未定義の tag 全体 compression は対応外 (NoArt + log)、v2.3 / 2.4 の zlib APIC は対応する。
5. v2.2 / 2.3 の tag unsync は frame walk **前**に body を in-place で詰める。
   v2.4 は raw frame 長で範囲を切り、その frame の unsync を一回だけ解く。
   tag 全体 flag と frame flag の両方が立っても二重処理しない。unsync は出力を増やさない。
   frame format の group / compression / DLI の補助 field を version に従って読み、復号順は
   unsync → format fields → zlib → APIC envelope。frame walk 時に一度だけ in-place unsync を行い、
   descriptor は復号済み範囲を参照する。type 調査と実 decode の再 inflate で unsync を繰り返さない。
   暗号解読・説明文字列の Unicode 変換は作らない。
6. 先頭 tag だけを走査し、PIC / APIC の **物理出現数**を数える (URL / 不正・対応外候補も含む)。
   **17 枚目で候補 descriptor / 画像 bytes / inflater の追加確保前に要求を NoArt + log で終える**。
   最初の 16 枚だけを採用して不完全な front 優先にしない。descriptor は最大 16 個の固定容量で、
   raw 領域内の復号済み offset / length・version / format fields / 元順だけを保持し、
   画像 bytes や全 frame object を複製しない。
7. 各 descriptor の APIC prefix から picture type を得る。v2.2 は 3-byte format、v2.3 / 2.4 は
   MIME の終端、type、description の終端を検査する。Latin1 / UTF8 は 1-byte 終端、
   UTF16 / UTF16BE は encoding の version と 2-byte alignment / BOM の必要条件を守る。
   説明は読み飛ばし、String 化しない。URL 型 `-->` は開かず skip。壊れた prefix は候補不適格。
   圧縮候補の type 調査も固定 scratch で必要 prefix だけ inflate する (§下段)。
8. type=3 の前面表紙を元順で全部試し、次に残る候補を元順で試す (Q2)。同じ description の
   APIC を Tag の重複排除規則で失わない。候補を一枚ずつ完全性検査・画像 decode し、最初の有効画像を返す。
   圧縮候補は prefix 調査と実 decode 時に最大二回 inflate するが、同時に一個の inflater / image だけを持つ。
   JPEG / PNG は必須対応。MIME だけを信用せず signature / dimensions / checked 画素数 / codec Limits を確認し、
   既存縮小・WebP encode に渡す。EXIF orientation は既存 byte decode と同じ、音声の補正 / AI / 回転を適用しない。
9. 保存・公開前 (cache hit も同様) に source を fresh stat し、stamp と context 世代が一致した場合だけ採用。
   MP3 を書き換えない。source_dims はジャケット画素寸法、audio の video_meta.width/height は NULL のまま。

仕様照合: [ID3v2.2](https://id3.org/id3v2-00)、[v2.3 structure / APIC](https://id3.org/id3v2.3.0)、
[v2.4 structure](https://id3.org/id3v2.4.0-structure)、[v2.4 APIC](https://id3.org/id3v2.4.0-frames)。
version の size / unsync / prefix 順を一つの「寛容」helper にまとめず、fixture で各仕様を固定する。

#### 圧縮 frame: 宣言値と実出力の二重制限

v2.3 compressed frame の 4-byte BE decompressed-size、v2.4 の compression に必須の
syncsafe data-length-indicator (DLI) を **inflate 前**に検査する。欠落 / 不正 / overflow は候補不適格。
DLI は画像だけでなく encoding / MIME / type / description を含む **APIC body** の長さである。
そのため DLI > 16 MiB を一律拒否すると、16 MiB 以下の画像と説明を持つ frame も拒否してしまう。

- APIC body の宣言展開長には既存 tag budget と同じ **32 MiB** の hard cap を置き、それを超えたら
  inflater 作成前に拒否する。宣言長そのものを Vec capacity / zlib 出力確保量にしない。
- 既存 `flate2::Decompress::new(true)` と `decompress(input_slice, output_slice, ...)` を使う。
  input / output を各 **64 KiB 以下**に分け、呼出前後に token / deadline / total_in / total_out を検査する。
  whole-frame read_to_end / uncompress / 宣言長の一括 buffer は使わない。decompress_vec の自動成長にも頼らない。
- prefix は固定 scratch で流して捨てる。終端が見つかった時点で prefix の論理長を宣言展開長から引き、
  **宣言画像長 > 16 MiB を、画像用 buffer の確保と残りの inflate の前に拒否**する。
  prefix 自体も total_out <= 宣言長 <= 32 MiB と期限で止め、長い description を heap に保持しない。
  §15 の 24 MiB bomb は prefix 調査までで拒否され、24 MiB の展開 buffer / image copy を作らない。
- 画像用 buffer は検査済みの宣言画像長だけを固定容量で確保し、実 image bytes も 16 MiB 以下に制限する。
  残量までの出力 slice と境界検出用一 byte の scratch を使い、偽の小さい DLI でも grow せず拒否する。
  上限外の一 byte は境界検出用 scratch だけへ出し、image buffer に入れず拒否する。
  完了時には **StreamEnd / 宣言長との一致 / zlib checksum /
  frame input 消費**を確認。truncated・dictionary 要求・連結 member / trailing junk は不正候補とする。
  input / output が進まない状態も失敗として停止し、retry loop にしない。
- 非圧縮候補も同じ prefix 解釈後に **実画像長 <= 16 MiB** を検査し、raw 領域を借用する。
  v2.4 の非圧縮 DLI も復号後の body 長と一致させる。unsync の escape bytes を画像長に数えない。

これにより「tag の物理長32 / frame の物理範囲 / APIC 論理出力32 / image16」の異なる長さを区別する。
新しい利用者設定・別の decode policy は増やさず、Q4 の **画像** 16 MiB を保つ。
制限・不正 frame は候補 skip、使える候補が無ければ NoArt + log。
全要求の tag / candidate 数超過は直ちに NoArt、timeout は Failed、取消は Canceled で永続 absence にしない。

一要求の抽出領域は raw <=32 MiB + image <=16 MiB + 固定 scratch と inflater 内部領域。
flate2 / miniz の内部領域も allocator 計測で確認し、宣言値に比例した隠れた割当を許さない。
候補間では image / inflater を解放し、decode 後は raw / inflate 領域を WebP encode 前に解放できる境界にする。
画像 codec の 160 MiB は抽出領域と別予算であり、**全要求160 MiB以下とは主張しない**。
最大同時一時領域は抽出48 MiB + codec160 MiB + 固定領域・既存縮小/encode buffer の重なりを
実装時に積算し、§4.1 の共通 semaphore で Local / Remote 全 context に適用する。
10 秒期限は全候補・prefix 再 inflate を通して一つ。候補ごとにリセットしない。

#### 新 gate harness の説明・成立条件 (2026-10-08 実行済み、§19)

次の実装依頼の **最初のゲート**は、製品共通にする safe Rust reader / bounded inflater を
そのまま dependency-only test harness から呼び、画像 decode / DB / UI より前の境界を実証すること。
別の簡略 parser を harness にだけ書いて通過扱いにしない。ffmpeg crate / DLL の初期化はゼロ。
既存 `audio_album_art_gate.c` / `check_audio_album_art_gate.py` は **旧方式の FAIL 再現資料**として保持する。
新方式の合否には使わず、旧 exit 1 を成功へ書き換えない。fixture の bytes / SHA を再利用する。
新 harness の出力は `target/audio-art-rust-gate/` とし、source / fixture / dependency / compiler の版・SHA、
実 exit code、requested allocation / peak live heap / private commit 差、読込 / inflate byte 数、
cancel / deadline 検査回数を保存する。private commit だけで割当前制限を証明しない。

| 新 gate / 回帰 | 必須証拠 |
| --- | --- |
| header / frame | 32 MiB 境界と +1、先頭 tag の自分自身の footer、巨大 header で payload read / heap=0。v2.2/3/4 size endian / syncsafe、高 bit、不正 flags、extended header、padding、cut / overflow / zero length、unknown frame skip |
| 候補 / 選択 | 16 / 17 枚、17 枚目の画像 / inflater 確保=0、同じ description の複数 front、逆順、壊れた front→次の front→他候補、URL、encoding 0/1/2/3 と UTF16 alignment / terminator / BOM、先頭タグの表紙を使用、後続タグの交換 / 削除 / update 有無を無視、末尾単独タグと末尾タグ+ID3v1はNoArt |
| 圧縮 / unsync | v2.3 decompressed-size / v2.4 DLI、group flags、圧縮 + unsync の組合せ、tag + frame unsync を一回だけ適用、FF 00 / FF E0 / FF 00 00 と chunk 境界。DLI 超過 / 欠落 / 小さすぎ・大きすぎ / CRC不正 / truncated / dictionary / multi-member / junk / no-progress を拒否 |
| 割当前制限 | 既存 24 MiB bomb に加え、宣言32MiB超の少量圧縮入力、宣言画像16MiB超、偽小 DLI で実出力過多、長い description と小画像、非圧縮16MiB境界。allocation instrumentation で tag / image の最大要求量と同時保持を確認し、image / whole expanded-frame 超過 allocation=0 |
| cancel / 時間 | open前、header / tag read / unsync / frame walk / prefix / image inflate 中の fake token / fake clock 検査、16候補で期限がリセットされない。小chunk一回分で協調停止し、NoArt保存=0、旧結果採用=0。実時刻でも10秒期限を検証し、OS call の例外を記録 |
| fuzz / regression | header/frame の純関数と capped inflate の二つの fuzz target。小さい注入 Limits で全分岐を短時間に探索し、実32/16MiB fixtureも回帰に残す。panic / OOB / overflow / 超過割当 / 無進捗loop=0、出力は入力領域か検査済み所有画像だけ。クラッシュ corpus を lib regression に固定 |

fuzz driver (cargo-fuzz / libFuzzer 等) は開発専用で製品依存に入れない。Windows native の deterministic
regression / allocation gate と、対応する toolchain での sanitizer fuzz を分けて結果を明記する。
新 gate 不成立ならここで停止し、FFmpeg / 別 crate / 制限緩和を実装者判断で代用しない。

利用者決定による簡素化: 有効タグ集合の置換 / merge / update の状態と、末尾タグ位置探索を作らない。
先頭タグ限定によって古い表紙 / アイコンになる限界は明示し、同名 sidecar を案内する。

状態の簡素化として、汎用 Tag object、全画像展開、二つの parser の fallback、別 reader worker、
inflate の resume / retry / recovery state を作らない。一要求の local 変数と最大16 descriptor だけで
同期的に (既存 worker 内で) 完了させ、既存 typed result / context cancel owner へ返す。
一覧の可視 thumbnail を modal にすると通常操作を止めるため採用しない。既存 worker と取消だけを使う。

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

detached 述語・viewport routing・registry の protocol / binding / 構造状態は変更しない。既存 bundle のサムネ状態として
NoArt を保持し、fork は結果を継承しても進行中要求を共有しない。
実装上の cache 削除完了は既存 bundle / projection の Audio NoArt / Failed だけを memory 上で Evicted へ戻す。
registry の狭い helper が所有 field を直接更新し、scope 外の親・drive、Loaded texture、Pending、generation / token を保持する。
with_viewer_context による一時 mount は reconciliation を起こすため使わず、registry protocol は触らない。
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

**利用者仕様の未決質問はない。** Q1 は 2026-10-08 の訂正を正本とし、Q2〜Q7 は既決事項のまま (§1)。
coordinator は旧ゲート失敗を受領し、FFmpeg を抽出経路から除く再設計を依頼した (2026-10-08)。
§3 の APIC 専用 reader は、先頭タグ限定の利用者決定と Rust gate を経て実装した (§19)。
差分と検証証拠を独立実装レビューへ引き継ぐ。追加の利用者仕様質問はない。
旧 TG1 の二択は撤回し、失敗履歴は §15、再生への別観測は §16。
初版は同名画像 → 前面表紙優先の MP3 埋め込み → アイコン。手動画像指定の質問・操作・DB 設計は撤回した。
「動画と同じ」は二つの既存設定に従う sidecar 採用・画像行省略を意味し、利用者の OFF 設定を引き継ぐ。

## 9. 実装順序・受入条件・検証所有

1. coordinator は Q1 訂正と Q2〜Q7 の決定を引き継ぎ、R6 の共通 sidecar source・一覧省略・
   既存 setting の互換・画像 stamp / cache・Remote path guard を重点として変更境界をレビューする。
   R7 の永続コレクション snapshot / navigation replacement の出所伝達も再レビューし、
   R2 の catalog maintenance 境界・422 分類と、R1 の 6 件の変更境界を実装 brief に含める。
   extraction の有界性、terminal 契約、親 catalog、Remote session Flight も維持する。
2. 抽出再設計の合意後、最初に §3.2 の共通 Rust reader / bounded inflate gate を実装・検証する。
   header / frame / image の割当前制限・取消・fuzz / 回帰が成立しなければ停止して設計を戻す。
   旧 FFmpeg harness の成功ケースを新 reader の証明として流用しない。
3. 型付き結果契約と NoArt の共通 consumer を挙動不変の chunk で整え、source catalog の
   追加 schema / helper、共通 sidecar source / discovery、Audio 要求 / 全 surface の UI、
   表示マーク設定 / transfer / snapshots、Remote を同じ feature の完了範囲にする。
   「PC だけ先に完成」で項目を完了扱いにしない。
4. 実装者が以下の自動検証を所有し、coordinator は有効な結果を再利用する。

| 層 | 必須の受入テスト (実行結果は §19) |
| --- | --- |
| extraction | §3.2 の新 Rust gate 全件・allocation 計測・fuzz corpus に加え、JPEG / PNG の実 byte decode、dimensions / 40 MP / 160 MiB Limits、front 優先・不正候補 fallback、取消 / timeout の終端・保存禁止。album-art から FFmpeg input / init を呼ばない |
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
共有 ThumbMsg / catalog maintenance 境界も [build/test policy](development-build-and-test.md) に従うが、
2026-10-08 の実装依頼の **test-full / build-dist 禁止**を優先する。full lib と core / Remote / UI の指定 gate を実行する。
Remote IPC / Web tests を省かず、CI 依存・timeout は十分な時間を確保する。

実装後は build-dev.ps1 で通常 core の確認 binary を用意し、Remote も同時 build する。
R7 までの設計のみの作業では build / test / binary 起動は不要で、実行済み件数は 0。
旧 FFmpeg 経路での最初の実装着手時は §15 の依存 gate 11 件 (10 成功・1 失敗) で停止し、
製品の build / test は未実行だった。現在の Rust gate と製品検証は §19 に記録する。
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
manual の文言計画: 「MP3 の埋め込み画像はファイル先頭の ID3v2 タグだけが対象です。
後続タグ・末尾だけのタグは読みません。まれに古い画像が表示されたり画像が出ない場合は、
song.mp3 と同じ場所に song.jpg など同名の画像を置いてください。」
対応 version と先頭タグ内の front 優先を説明し、全 ID3 配置に対応すると書かない。
video_pins.db、rename / metadata transfer の user-data 形式、keymap に音声操作は追加しない。privacy の cache 保存 / 通信と製品ページの
「安心して使えます」も照合し、新しい外部通信は増えないこと、既存認証済み Remote への
ジャケット配信が画像配信の記述に含まれることを確認する。

coordinator への引き継ぎ: 受入済み R7 と Q1 訂正・既決 Q2〜Q7 は保持する。§3 の
依存比較 / 有界 APIC 専用 reader と新 gate を独立レビューし、その合意と新 gate の成立まで
Remote / sidecar / 設定を含む製品実装へ進まない。§16 の再生時割当は別 backlog 候補として判断する。
今回の変更は抽出設計・索引・旧 harness の説明だけ。commit は行わない。
今回の英語メッセージは `target/D-1347-redesign-msg.txt` に置く。
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
R7 文書修正時点は再レビュー待ちだった。その後の受入は冒頭の利用者伝達記録を参照。
R7 では追加の利用者質問はなく、既決仕様は変更していない。

| 指摘 | 確認したコードと解消内容 |
| --- | --- |
| R7-P2 永続コレクションの出所伝達 | persistent_collections.rs:1566 が thumbnail_address: None を作り、mod.rs:153 の populate_remote_entries は通常 RemoteEntry のみが対象であることを確認。snapshot (503,538) と navigate (635) は view_facts / navigation_view_facts を使い、wire_snapshot (1449) が facts.entries をコピーする。§7.1 に共通 discovery→Available.thumbnail_address の付与を byte 計算 / token 確定前に行う契約と、設定を含む既存 facts reuse key・取消 / deadline・refresh の所有境界を追記。app.js:4921,7307 は既存 optional 出所を tile / query に渡せるため wire field の追加は不要。§9 に両経路の FLAC / MP3、設定・再利用・frame 上限・token・Web 伝達の回帰条件を追加 |

文書のみの修正。製品コードの実装・commit・製品 binary 起動なし、製品テスト実行数は 0。

## 15. 実装着手時の FFmpeg 技術ゲート (2026-10-08、FAIL / 停止)

利用者の実装依頼どおり、他の製品コードを変更する前に **当時の** §3.1 / §3.2 の FFmpeg 依存実証を行った。
この節は失敗履歴。coordinator は証拠を受領し、現在の抽出方式は §3 へ再設計した。
**11 件中 10 成功・1 失敗、gate runner の実 exit code は 1。割当ゲートは不成立。**
この結果を以て機能実装を停止し、代替 parser / 子プロセス / 制限の緩和へは進んでいない。

### 実証した境界

- [依存専用 C harness](../scripts/audio_album_art_gate.c) を、この worktree の FFmpeg headers / import
  library で MSVC x64 compile し、vendor/ffmpeg/bin の DLL をロードした。
  実行版は `n7.1.5-16-g9a4bb2c579-20260816`。製品 executable の起動ではない。
- File + custom AVIO read / seek に取消・deadline と累積 callback read 上限 (32 MiB + 64 KiB) を置き、
  連続した **全 ID3 header** の宣言サイズ合計を open 前に 32 MiB 以下と検査する。
  seek はファイル範囲内に限定。AVFormatContext の interrupt callback と max_streams=17 も設定する。
  事前 header 検査の read と AVIO の累積 read は別計測で、報告の bytes_read は後者。
- `avformat_open_input` を MP3 demuxer 指定で一回呼び、attached picture の size / comment を借用して検査する。
  `avformat_find_stream_info`、packet ループ、audio / video decoder、画像 decode は呼ばない。
  共有 libavutil の allocator 上限は変更しない。user settings / DB / 実 MP3 にはアクセスしない。
- 素材は Python / Pillow で生成する 1x1 JPEG / PNG、synthetic MP3 frame、ID3 APIC / PIC のみ。
  [再現 runner](../scripts/check_audio_album_art_gate.py) が harness を compile し、ケースごとに独立 child で実行する。
  child timeout は 20 秒で実験を停止するための上限であり、製品の 10 秒保証の代用ではない。

| 確認 | 結果 / 保証できる範囲 |
| --- | --- |
| v2.3 JPEG / v2.4 PNG / v2.2 PIC | 3 件成功。open-only で attached packet と front metadata を取得 |
| back → front-a → front-b / 無タグ | 2 件成功。複数 attached stream と二つの front metadata、無画像 0 stream を確認。画像 decode / 選択全体の製品実装試験ではない |
| open 前 cancel / deadline、read 中 cancel、累積 read 制限 | 4 件成功。前者は open 0 回、後者は callback が停止して open 失敗。FFmpeg 内の全 CPU / 割当箇所へ取消が届く保証とはしない |
| 連続 ID3 の合計上限 | 1 件成功。各 17 MiB の二つの tag は一つ目を検査した後、FFmpeg open 前に拒否 |
| 圧縮 APIC の割当上限 | **1 件失敗**。以下の入力は header / read 上限を通るが、open 内の割当と画像 packet が予算を超える |

### 失敗の証拠と原因

v2.4 の compressed + data-length-indicator flag を持つ APIC に、1x1 PNG と padding を合わせた
24 MiB の展開データを入れた。圧縮済み ID3 は **24,597 bytes** で 32 MiB 未満。
データ長と frame 長は v2.4 の syncsafe 形式で生成し、巨大 RGBA decode を使わず再現する。

- callback read は **42,037 bytes**、open は ret=0、**32 ms** で成功した。
- returned attached packet は **25,165,806 bytes** (約24 MiB) で、候補 16 MiB の上限を超えた。
- Windows `GetProcessMemoryInfo` の PeakPagefileUsage の open 前後差は
  **239,661,056 bytes (約228.56 MiB)**。
  これは private commit の peak 差分であり、RSS / GPU / 画像 decoder の計測ではない。
  画像 decode を始める前に、160 MiB を超える追加割当を観測した。
- 圧縮 frame のサイズ情報に基づく libavformat 内の展開 buffer 確保は、AVIO callback の
  物理 read budget / seek guard や attached_pic の事後 size 検査では拒否できない。
  ID3 合計 32 MiB だけを割当前に検査する構成は、展開後の割当上限の証明にならない。
  max_streams も既に読んだ ID3 frame の展開・候補確保の上限にはならない。

上流 n7.1.5 の [id3v2_parse / read_apic](https://github.com/FFmpeg/FFmpeg/blob/n7.1.5/libavformat/id3v2.c)
では compressed data length による展開 buffer 確保と、その後の APIC buffer 確保が parser 内にある。
これはコード照合の補助資料であり、同梱 +16 版の挙動の根拠は上記の **実 DLL の計測**。
ローカルの `vendor/ffmpeg/include/libavutil/mem.h:588` の av_max_alloc は一 block の共通 allocator
上限を変える API で、要求 / context ごとの budget ではない。動画 / 音声の再生と共有する allocator を
抽出中だけ変える案は accepted design の既存再生維持・worker 所有境界を満たす根拠にならないため採用しない。

### 再現・引き継ぎ

repository root から `python scripts/check_audio_album_art_gate.py` を実行する。
Windows x64 MSVC build tools と Python Pillow が必要。出力はこの worktree の
`target/audio-art-gate/` のみ。fixture、build log、各 stderr、DLL SHA-256 と HEAD / harness SHA-256 を含む
[results.json](../target/audio-art-gate/results.json) を保持する。正常系 10 件成功・圧縮 allocation 1 件失敗、
exit 1 が当時の再現結果。ゲート失敗を green として扱わず、割当の事後検査だけで実装を再開しない。
再設計時の script 冒頭説明の変更により現 source SHA は当時計測版と異なるが、実行 logic は不変。
results.json の元 SHA / DLL・fixture・計測結果は更新せず、旧再現資料と新 Rust gate を区別する。

**当時の技術相談 TG1 (現在は撤回):** FFmpeg adapter の追加有界化か、ID3 crate への変更かを相談した。
2026-10-08 に coordinator が失敗証拠を受領し、FFmpeg を album-art から完全に除外する再設計を指示。
§3 の代替評価・APIC 専用 reader 提案へ置き換えた。旧ゲートの計測・exit 1 はそのまま保持する。
利用者の Q1〜Q7 と Remote / sidecar / 表示マーク仕様は変更していない。

**旧 FFmpeg gate で停止した時点の記録:** 製品実装・IPC 更新・settings / catalog schema 更新・manual の完成扱いは行っていない。
Rust / Web の focused / full lib / normal・portable core / ui_snapshot / build-dev は、ゲート成立後の工程のため未実行。
製品確認 binary は作らず、製品の手動確認を利用者へ依頼しない。commit・製品 binary 起動なし。

当時の付随検証: `cargo fmt --all -- --check` は exit 0、`python scripts/check_ui_glyphs.py` は
危険 glyph 0 / exit 0。gate runner の `py_compile`、ローカルリンク 13 件、変更 5 ファイルの
UTF-8 / CRLF、`git diff --check` も成功。既存製品の不具合修正ではないため bug-fix red はなく、
gate の allocation 失敗を成立した技術試験として保存する (機能の合格・実装済みとはしない)。

## 16. 別観測: 再生時の圧縮 ID3 割当 (2026-10-08、修正せず coordinator 判断)

**backlog 候補 (未採番・未決定): 「MP3 再生準備 / 音声解析の ID3 展開割当を調査」**。
§15 の synthetic MP3 / 実 FFmpeg DLL で open-only が **239,661,056 bytes (228.56 MiB)** の
追加 peak private commit を発生させた。画像 decode なし、圧縮 ID3 は24,597 bytes、
attached packet は25,165,806 bytes。元証拠は target/audio-art-gate/results.json / fixture / DLL SHA。

| 既存経路のコード照合 | 観測から言えること / 限界 |
| --- | --- |
| src/video/decoder.rs:2334 → video/avio_progress.rs:476,499 | 再生 demux は input_with_progress から avformat_open_input、次に avformat_find_stream_info。同じ libavformat の ID3 open を通り、今回の要求単位 tag / 展開上限はない。custom AVIO は進捗用で、内部展開割当の cap ではない |
| src/video/avio_progress.rs:364,370 | custom AVIO 不可 / 無効時は ffmpeg::format::input の fallback。Rust wrapper も format open を行うため、APIC 除外で open 内の割当を回避する構造ではない |
| src/app.rs:68819,68843 (build_audio_player_for_open)、68325,68391 (動画) / src/video/decoder.rs:2376 | audio-only MP3 も headless VideoPlayer::open_with_output_consumer から同じ decoder を使う。ATTACHED_PIC の映像除外は **open 後**であり、cover を HW decode しない保護であって ID3 展開防止ではない |
| src/app/native_video.rs:13787,13915,14970 | 動画→音声モードへの単なる入場は presenter を hide して **既存 player / decode を継続**し、新 MP3 を開き直す操作ではない。一方 source swap / 次の MP3 を開く場合は build_video_player_for_open を通って同じ open が走る |
| src/audio_decode.rs:189,196,682 | 音声解析の open_audio_decode と別 decode 経路も ffmpeg::format::input。共通 player の open だけでなく波形等の別 input open も調査対象候補 |

従って既存 MP3 再生 / 解析にも同種の追加割当が起きる可能性がある。
**製品の各経路で229 MiBを計測したという主張ではない**。製品 binary は起動しておらず、
wrapper・probe・同時 open 数による実際の peak / 取消挙動は未計測。
coordinator が backlog 化・優先度・専用調査を判断する。今回は再生の open、attached-picture 除外、
動画→音声モード、波形、decode、共通 allocator に変更を加えない。

## 17. 抽出再設計時の変更規模と技術確認 (当時の記録)

通常依存・product code・IPC・DB・settings の変更は **0**。文書の抽出節と gate 説明、
README 索引、旧 harness の冒頭説明だけを改訂し、旧 gate の executable logic / evidence を保持した。
APIC 専用 reader の見積は core **約400〜650行**、synthetic regression / allocation harness / fuzz driver は
**約700〜1,000行** (実装前の概算、既存画像 decode helper と Remote / UI / catalog 本体の実装量は別)。
FFmpeg adapter を置き換える範囲だけが増減し、受入済み feature の完了範囲は縮小しない。

残件は設計担当の技術判断と新 reader gate。crate を採用する変更へ戻る場合は、固定版の
registry / checksum / license 本文 / inflate source / build を取得して証明し直す。
利用者仕様の追加質問はない。今回の再設計を独立レビュー受入済みとは扱わない。

今回の文書確認: UTF-8 / CRLF 5 ファイル、ローカルリンク13件、旧 Python runner の executable AST 不変、
py_compile、UI glyph lint (危険 glyph 0)、git diff --check が成功。offline metadata / tree は実 exit 0。
新 parser / gate と製品テストは実行0件、旧FFmpegゲートも再実行せず既存証拠を保持した。

## 18. 84258858a の P2 二件と利用者決定 (2026-10-08)

両指摘を仕様 §3.2 / §5 と照合し採用。前設計のファイル順連結・EOF直前だけの
footer 検査では完全な置換 / update / ID3v1併記を満たさない。利用者が (a) 先頭タグ限定を決定した。
§1 / §3.2 / §10 で制限を明示し、gate は先頭を使用・後続を無視・末尾単独をNoArtとして検証する。
完全規則・末尾探索の実装は要求しない。追加の利用者質問なし。

## 19. 先頭タグ限定版の実装・検証記録 (2026-10-08)

利用者の先頭タグ限定決定に従い、共有 Rust reader の gate を先に実行してから製品実装へ進めた。
`python scripts/audio-art-rust-gate/run.py` は同じ `src/audio_album_art.rs` と regression を呼び、
`target/audio-art-rust-gate/results.log` / `results.json` に実 exit code・rustc・source / lock SHA・allocator 計測を残す。
42 件 (parser 35 + gate 7) 成功。宣言 tag 超過 / 開始前取消は allocation 0、17候補は inflater 前に拒否。
旧24MiB圧縮 bomb は最大要求43,296 bytes・peak67,785 bytesで拒否し、画像16MiB境界は16,777,216 bytes固定要求、+1は画像割当を行わない。
header / inflate の決定的 mutation corpus 各16,000ケースも regression に含む。外部 libFuzzer / sanitizer の長時間実行はこの記録に含めない。
旧 FFmpeg gate の FAIL 証拠と再生経路未計測の観察は §15〜§17 の履歴として保持する。

製品実装では typed ThumbMsg、worker-only art catalog / prune、maintenance の Local / Remote 接続 retirement、
CacheOnly 継続、全 Audio 一覧の出所、三択マーク、Remote IPC67 / DOM外終端 owner を同時に接続した。
履歴と合成一覧 refresh は entries / source map を既存世代 owner で採用し、UI から再探索しない。
一時 mount / 別 Audio pin owner / bookmark 専用 refresh reason は不要と判断し、既存の明示 refresh へ集約した。

memory の保守的積算: raw32MiB + picture16MiB、codec予算160MiB、decode raster最大160MiB、
回転 / 色変換最大160MiB、表示RGBA / ColorImage / 縮小作業領域を各16MiB (最大2048px) とする。
同時に存在しない領域も合算した上側見積りは576MiB/permit。既定Low1、Medium2、High4で最大576 / 1152 / 2304MiB。
同じpermitをLocal/Remote/旧contextが共有し、画像を返す時点まで保持する。これは実測RSS保証ではなく、
codec以外の既存allocator/encoder領域を含む保守的な一時領域見積り。parserの実測割当は上記別記録を使う。

最終製品 gate の結果は下記に記録する。実アプリ起動・commit・test-full・build-dist は行わない。
未決の利用者質問はない。coordinator は差分と自動検証を独立実装レビューへ渡し、実機確認を行う。

既存 DEFLATE 依存の MIT 本文・著作権は registry の固定版から照合し、
`RUST-DEFLATE-NOTICES.txt` に flate2 1.1.9 / miniz_oxide 0.8.9 / crc32fast 1.5.0 /
adler2 2.0.1 / simd-adler32 0.3.10 / cfg-if 1.0.4 を収録した。
バージョン情報へ include_str で埋め込み、通常・portable の binary 内から同じ本文を参照できる。
新 crate / DLL / feature / lock 変更はない (独立 gate の固定 Cargo.lock のみ新規)。

全 lib の初回実行で、物理一覧の catalog 掃除をセル距離で順位付けする誤りを検出した。
`embedded_similar_move_update_pump` の実 handler でも 1成功 / 4失敗の red を確認
(thumb_loader.rs の距離 +1 overflow → queue mutex poison)。掃除は型が所有する非セル仕事とし、
通常サムネイル・prefetch の後に順位付け、keep / prefetch suppression / 単一セル eviction で
破棄しない。別 queue・index sentinel・新しい bool 状態は設けず、context cancellation は維持する。

製品 gate の次の全 lib は 11,044 成功 / 1 失敗 / 52 ignore (exit101、1,221.55秒)。
原ログを `target/audio-full-lib-r2.log` に保存した。失敗は既存
`video::decoder::audio_track_fixture_tests::failed_seek_flush_packet_and_seek_completed_retry_use_real_demux` の
PCM recv (decoder.rs:10921) の250ms timeout。HEADにも同じ unwrap があり、
テスト全体の10秒 deadlineを待たず panic にしていた。隣接のaudio-only seek fixtureと同じく
Timeoutを期限内で継続し、Disconnectedは即失敗とする **テスト内だけ**の変更を行った。
PCM serial / PTS / seek-target / event retry の検証と10秒期限は維持し、製品decoderは変更しない。
単独のEOF loop回帰は1件成功 (75.44秒)、失敗したdemux回帰も修正前の単独では1件成功。
全体負荷で起きた実timeoutをredとして保持した。修正後のfixture群は33成功 / 1 ignore / exit0
(4.23秒、`target/audio-demux-fixture-final.log`)。全 libを再検証し、結果を下記へ記録する。

契約照合で「候補 skip後の使用可能画像なし → NoArt + log」の後半に不足を確認した。
32MiB tag / 17候補のError拒否は既にログしていたが、画像decode不適格やper-picture上限で
候補が尽きる経路も、共通generatorのsource NoArt判定時に一行記録する。
cache absence hitでは再記録せず、Local / Remoteで同じ実装を使う。parser / 状態 / 保存条件は不変。
この追加を最終ソースに含めるため、途中の全lib再実行は所有するsessionのCtrl+Cで終了した
(exit1、summaryなし)。成功にも製品test失敗にも数えず、ソース変更は終了確認後に行った。
最終全libは全ケースを実行し、重いheadless UI群の同時資源競合を減らすため
`RUST_TEST_THREADS=8`を指定する (feature / assertion / skip条件 / deadlineは不変)。

### 最終製品検証 (HEAD 84258858a + 本差分)

| コマンド / 範囲 | 実結果 |
| --- | --- |
| `python scripts/audio-art-rust-gate/run.py` | 42成功・0失敗、exit0。source/lock SHA・allocator記録は `target/audio-art-rust-gate/results.json` |
| `cargo test -p mimageviewer --lib audio_` (最後のNoArt log追加後) | 346成功・2既存ignore・0失敗、14.28秒、exit0 |
| `--lib embedded_similar_move_update_pump` | 5成功・0失敗、exit0 (修正前1成功/4失敗) |
| `--lib multiwindow_scenario_tests` | 35成功・0失敗、exit0 |
| `--lib settings_transfer::tests` | 16成功・0失敗、exit0。447分類 / 135 export / 312 exclude / wire133 |
| `--lib video::decoder::audio_track_fixture_tests` | 33成功・1既存ignore・0失敗、exit0 (上記timeout修正後) |
| `cargo test -p mimageviewer --lib` (`RUST_TEST_THREADS=8`) | 11,045成功・52既存ignore・0失敗、1,112.82秒、exit0。`target/audio-full-lib-final.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 105成功・0失敗、exit0。追加の音声三択・fallbackと既存UIを確認 |
| Remote IPC lib tests | 65成功・0失敗、exit0 |
| Remote Web lib tests | 135成功・1既存ignore・0失敗、exit0 |
| Node runtime / audio-art state | 163 + 21 = 184成功・0失敗、exit0。`target/remote-audio-runtime-node.txt` / `remote-audio-state-node.txt` |
| `cargo fmt` / 最終 `cargo fmt --check` | 成功・exit0。Rust/文書/HTML/JSは既存CRLFを維持 |
| normal core check / `--features portable` core check | 両方exit0、最後のNoArt log追加後にも実行 |
| `python scripts/check_ui_glyphs.py` | 危険glyph0・exit0 |
| `git diff --check` / 追加したローカル文書リンク | 成功・11リンク欠落0。新しいgate Cargo.lockだけCargo標準LF |
| `scripts/build-dev.ps1 -PreserveRuntime` | 全lib成功後、上記の直列build環境で実行しexit0。normal core / IPC67 Remote / EPUB workerを生成、FFmpeg・VCRT・EffeTuneを配置。PE依存チェックruntime4 / pe3成功。core 36分27秒、Remote 3分41秒、EPUB worker 2分30秒。`target/audio-build-dev-final.log` / `target/vcrt-pe-reports/dev-runtime.json`。製品binaryは未起動 |

Rust/Remote/UIの機能検証を、変更のない依存・対象について再利用する。
UIの最後の変更以降はheadless UI105件が成功済みで、その後はdemux fixture待機とsource NoArtログのみ変更した。
Remote IPC/Web/Nodeは対応するwire/HTTP/JSの最終変更後の結果を再利用し、core側生成処理は最終全libにも含む。
通常/portableの最初のcold checkはMSBuildの並列native buildで失敗した。
診断した同じCMake projectの直列buildはexit0で、以後は `CARGO_BUILD_JOBS=1` と
`MSBUILDDISABLENODEREUSE=1` で両core checkが成功した。製品/dependency/build-scriptへの回避変更はない。

### 利用者の確認手順と引き継ぎ

確認用buildは通常featureの `target/dev-runtime/mimageviewer-core.exe` と同じIPC67のRemoteを含む。
`dev-runtime`はCargo最適化profileで、通常 `%APPDATA%\mimageviewer` の実settings/dataを使い、
起動により設定/データが更新される場合がある。single-instance mutexはinstalled版と共通のため、
利用者がinstalled / tray常駐を閉じてから、repository rootのPowerShellで起動する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. 先頭ID3v2のJPEG/PNG付きMP3、無画像MP3、FLAC等と同名JPEG/PNGを用意し、
   同名画像 > MP3埋め込み > アイコンの優先を確認する。sidecar用の二設定をONにし、
   既存boolをOFFにした場合も、画像行省略と音声側への反映を確認する。
2. 同名画像の追加/交換/削除後にF5、詳細一覧hover、Auto比率を確認する。
   音楽アイコン / 文字バッジ / なしを切り替え、画像なしは常に音楽アイコンになることを確認する。
3. 検索・★・ブックマーク・履歴・スマート・コレクションの同じAudioを確認し、
   多数項目のscroll/再訪で無画像の再試行が続かないことを確認する。
   ダブルクリック時の音楽再生と、既存動画sidecar/frame pinも従来通りか確認する。
4. mIV Remoteの通常/合成/永続コレクションで同じ画像と三択マークを確認する。
   無画像を画面外へscrollして再訪し、外部画像編集後は明示refreshで再取得する。
   同じmtime秒+sizeの編集と後続/末尾ID3v2は既決の検出/対応範囲外で、
   前者はcache削除と再読込または明示SourceOnly再生成、後者は同名sidecarで対応する。

利用者質問はなし。coordinatorは本差分・本節の自動検証・`target/D-1347-msg.txt`を
独立実装レビューへ渡し、上記の実機確認を調整する。
§16の再生/解析時ID3展開割当の観察をbacklog化するかは別途coordinatorが判断する。
製品binaryの起動、commit、test-full、build-distは行っていない。
