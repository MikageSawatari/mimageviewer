# リモート閲覧: 動画サムネイルと音声アイコン

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 0. 着手前に読むこと

この増分は「既にあるものを見落として作り直す」条件が揃っている。本体には**出どころが 3 つ**
あり、リモートには②だけが部分的に繋がっている。次を守ること。

- **本体側の 3 経路を最後まで読んでから設計する。** 途中まで読んで「こうなっている」と
  決めない。分岐の確定が別の関数にあることを想定する
- **既に受け皿があるかを先に探す。** 新しい仕組みを作る前に、本体の既存経路・共通部品・
  設定を確認する。無いと判断したら、その根拠を報告に書く
- **ドキュメントとコードが一致しても裏付けにしない。** どちらも古い可能性がある。
  実装を正とする
- 種別・状態の集合を広げるときは、その集合を書いている箇所を**全部**述語 1 つへ置き換える。
  リテラルが散っていると 1 か所の更新漏れが実機不具合になる

以下 §2 は 2026-08-09 に現物を読んで確認済み。ここに書いた行番号・関数名から始めてよい。

## 1. 観測された症状

利用者報告:

- タグビュー等の集約系一覧で、動画・音声セルが代替マークのまま
- お気に入り `testimage` 配下の `movie` フォルダでもサムネイルが出ない

## 2. 調査済みの事実 (再調査不要)

### 2.1 リモートのサムネイル生成は画像とフォルダしか受け付けない

[src/remote_ipc/thumbnail.rs:191](../../src/remote_ipc/thumbnail.rs) の `generate_resolved`:

    if !is_folder && !is_supported_image(&resolved.canonical) {
        return Err(error_response(
            ThumbnailErrorCode::Unsupported,
            "この種類のサムネイルは今回の増分では扱いません",
        ));
    }

`is_supported_image` は `folder_tree::is_recognized_image_ext` を見るので、動画・音声は
ここで 415 になる。

### 2.2 本体の動画サムネイルは 3 つの出どころを順に見る

[src/app.rs:29345](../../src/app.rs) 付近の動画スレッド:

1. **ピン留めフレーム** (`video_pins` DB の WebP) — 最優先。spawn 時に snapshot した
   `pin_blobs` から取り、`catalog::decode_thumb_to_color_image` で復号する
2. **同名画像 (sidecar)** — `thumb_overrides` map を `normalize_keep_drive` か stem で引く
3. **Windows Shell** — `crate::video_thumb::get_video_thumbnail(&path, thumb_size)`
   (COM は関数内でスレッドローカル初期化するので呼び出し側は何もしなくてよい)

動画は `thumb_loader` を通らない完全な別経路。

### 2.3 リモートは ② をフォルダ一覧でだけ持っている

- [src/remote_ipc/container.rs:1739](../../src/remote_ipc/container.rs) が
  `FolderListEntry.thumbnail_address` に sidecar 画像の住所を入れている
- **`RemoteEntry` (crates/remote-ipc/src/lib.rs:1018) にはその欄が無い。** `path` だけ。
  タグ・閲覧履歴・ブックマーク・レーティング・スマートフォルダ・検索結果は出所を指定できない
- ①③ はリモート側に配線が無い

### 2.4 音声は本体もサムネイルを作らない

[src/app.rs](../../src/app.rs): `GridItem::Audio(_) => ThumbnailState::Failed`。
代わりに [src/ui_helpers.rs](../../src/ui_helpers.rs) の `draw_music_icon` が
**2 連八分音符をベクター描画**する。doc comment に理由が明記されている:

> 絵文字グリフ (🎵 / 🎶 等) は環境依存フォントで tofu 化しうる
> (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」)。動画セルの再生アイコン
> (`draw_play_icon`) と同様に painter プリミティブで描いてフォント依存を避ける。

### 2.5 ★ 受け皿は既にある — 作り直さないこと

**web 側は `RemoteEntry` に欄が増えれば、変更なしで動く。**
[app.js:4300](../../crates/remote-web/web/app.js):

    export function thumbnailAddressForEntry(entry) {
      return entry.thumbnail_address ?? entryAddress(entry);
    }

`loadThumbnail` も `thumbnailBindingKey` もこの 1 関数を通っている。§3.2 は
**サーバ側で欄を埋めるだけ**で完結する。web に分岐を足さないこと。

### 2.6 リモートのサムネイルは二重にキャッシュされている

- **カタログ DB**: `generate_resolved` は `cache_key_override` を付けた `LoadRequest` を
  `thumb_loader::process_load_request` に渡し、`CatalogDb` を共有する。本体 UI と同じ
  キー体系・同じ DB
- **HTTP**: [http.rs の `api_thumb`](../../crates/remote-web/src/http.rs) は
  `Cache-Control: private, max-age=60`。URL には `epoch` (セッション取得で変わる) が付く

**ただし動画はこのどちらにも乗っていない。** 本体の動画スレッドはカタログへ 1 行も
書かない (§2.2 の経路を最後まで読んで確認した)。Shell 抽出は Windows 自身の
thumbcache に当たるので、本体はそれで足りている。

### 2.7 Shell は「まだ抽出中」を None で返す

本体の動画スレッドは `MAX_CONSEC_RETRIES` まで 200ms→400ms→…→6.4s で**再試行する**。
一方リモートの経路は違う:

- `handle` は同時到着を 1 本にまとめる (`inflight` + `Condvar`) が、**同期的**
- HTTP 側は `state.ipc_admission.run(IpcClass::Heavy, ...)` の枠を 1 つ占有する

つまり**サーバ内でバックオフ再試行を回すと、その間 Heavy 枠を握り続ける**。
そして web 側には既にバックオフ再試行がある
([command-core.mjs:2231](../../crates/remote-web/web/command-core.mjs) の
`thumbnailRetryDecision`): 200·2ⁿ ms・上限 4s・予算 3 回。transient (0 / 502 / 503) を
再試行し、`ipc_busy` だけは予算を消費しない。

## 3. やること

### 3.1 音声アイコン (先に入れる。安く効く)

- 音声セルの代替マークを、本体と同じ **2 連八分音符**にする
- **inline SVG で描く。絵文字文字を使わない** (§2.4 の判断を引き継ぐ)
- サムネイル要求自体を出さない。音声は出どころが無いので 415 を取りに行く意味が無い

### 3.2 集約系の一覧でも sidecar が効くようにする

- `RemoteEntry` にサムネイル出所の欄を足す。**web 側は §2.5 のとおり変更不要**
  (serde の欄名を `thumbnail_address` に揃えること)
- 出所の決定は**本体側 1 か所**に置く。フォルダ一覧と集約系で別々に書かない
- **集約系には フォルダスキャンが無い**。`FolderListEntry` の sidecar は
  `filter_video_image_duplicates` (folder_scan.rs:496) が作る「同 stem の画像が
  同じ一覧に居るか」から来ている。集約系で同じ結果を得る方法を決めて根拠を残すこと。
  親フォルダごとにまとめて 1 回走査するのか、動画 1 件ごとに同 stem の画像拡張子を
  probe するのか。**一覧の件数だけ `read_dir` する実装にしないこと**
- **集約系では sidecar 画像を一覧から消さないこと。** フォルダ一覧は sidecar を吸収して
  独立タイルを消すが (container.rs のテストが `clip.jpg` の不在を検証している)、集約系は
  検索・タグの結果集合なので、条件に合致した画像を消してはいけない。**吸収せず、出所に
  使うだけ**
- sidecar が無い動画は従来どおり代替マークのまま (③ は 3.3 で)

### 3.3 動画自身からのサムネイル (本丸)

- ピン留めフレーム (①) と Windows Shell (③) をリモートのサムネイル経路へ配線する。
  ① は `video_pins::VideoPinDb` を `WorkerContext` (thumbnail.rs:32) で開けば届く
- **キャッシュを新設しないこと。** §2.6 のとおり Shell 抽出は Windows の thumbcache に
  当たり、その上に HTTP の 60 秒がある。カタログへ動画用のキー空間を足すと、リリース済み
  DB の中身と キャッシュ管理ダイアログの集計対象が変わる。今回は足さない。**足すべきだと
  判断したなら、実測値を添えて報告し、実装は次の増分にする**
- **サーバ内でバックオフ再試行を回さないこと** (§2.7、Heavy 枠を握るため)。
  Shell が None を返したら**「まだ用意できていない」と分かる応答**にして、web 側の
  既存 `thumbnailRetryDecision` に拾わせる。どの status / error code に載せるかは
  `thumbnailRetryDecision` の現在の分類を読んでから決める。新しい分類を足す場合は
  `ipc_busy` と同じく**予算を消費しない**扱いが妥当かを検討し、理由を書くこと
- 恒久的な失敗 (壊れたファイル等) は代替マークへ落とす。`VideoThumbDiag` に段階と
  HRESULT が入るので、ログには段階を残す

## 4. やってはいけないこと

- 絵文字文字で音符 / 再生マークを描くこと (§2.4)
- `thumbnailAddressForEntry` と別の経路を web 側に足すこと (§2.5)
- カタログ DB に動画用のキー空間を新設すること (§3.3)
- サーバ側でバックオフ再試行を回すこと (§2.7)
- 音声に対してサムネイル要求を投げること (出どころが無い)
- サムネイル出所の決定規則をフォルダ一覧と集約系で二重に書くこと
- 集約系で sidecar 画像を一覧から消すこと (§3.2)

## 5. テスト

- 音声セルが要求を出さず、音符の図形を描くこと
- sidecar のある動画が**集約系の一覧でも**出所を持つこと
- 集約系で sidecar 画像自身が一覧から消えないこと
- sidecar の無い動画が代替マークになること (3.3 前)
- ピン留めフレームが sidecar と Shell より優先されること
- Shell が「抽出中」を返したときの応答が、`thumbnailRetryDecision` で retry になること
- Shell が恒久的に失敗したときに再試行されないこと

## 6. 進め方

- ビルドとコミットはしない (ClaudeCode が行う)
- `htdocs/` は変更しない
- 3.1 → 3.2 → 3.3 の順に。3.1 と 3.2 が終わった時点で一度報告してよい
