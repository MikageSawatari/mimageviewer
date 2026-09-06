# X 予約投稿 (X Scheduled Post) 設計 — 正本

mIV から X (Twitter) へ、指定時刻に自動で投稿する機能の設計。

- 状態: **設計合意済み・未実装**。§14 の未決が埋まるまで実装に入らない。
- 作業場所: 別 worktree (branch `x-scheduled-post`)。master と並行させる。
- 参考実装: `C:\home\xupload` (利用者の自作 Python ツール)。X 側の仕様・課金・
  失敗モードはここで実運用済みなので、**推測ではなくこの実証を出発点にする**。
- 関連: [sns-split-export-plan.md](sns-split-export-plan.md) (X の表示実測。§2.1 が
  カルーセル表示と継ぎ目比の正本)、[web-remote-plan.md](web-remote-plan.md)
  (別プロセス + HTTP + token の既存例)。

---

## 1. 何を作るのか

1. **予約投稿**: 本文 + メディア (画像 1〜4 / 動画 1 / GIF 1) と投稿時刻を登録すると、
   その時刻に mIV が X へ投稿する。
2. **投稿プレビュー**: X のタイムラインでの見え方に近い形で、本文・改行・リンク色・
   文字数・メディアの並びを確認できる。
3. **費用の警告**: 本文に URL を含むと 1 投稿あたりの単価が跳ねる (§2.3)。予約前に
   はっきり見せる。
4. **ローカル API**: `127.0.0.1` からのみ、token 付きで予約を登録できる HTTP API。
   既存の xupload を後から「mIV へ予約を投げるだけ」に作り替えられるようにする。
5. **不在時でも投稿する**: 投稿の実行は Windows タスクスケジューラに登録した
   1 タスクが担う。**mIV を閉じていても予約時刻に投稿される** (§4.1、利用者判断
   2026-09-06)。常駐と OS 自動起動は「ローカル API を受けるため」に用意する。

**やらないこと**は §11。**pixiv への自動投稿はしない**。規約を読んだうえでの判断は
§11.1 に書いた (自動送信ゼロの「下ごしらえ」だけなら成立する)。

---

## 2. 前提 — X 側の仕様 (xupload の実運用で確定している事実)

以下は `C:\home\xupload\CLAUDE.md` (2026-08-30) と `post_worker.py` に記録された
**実運用で確認済み**の事実。実装時には最新を再確認するが、設計の土台はこれ。

### 2.1 X には「予約投稿 API」が無い

- Ads API の `scheduled_tweets` は 2026-08 に `nullcast=false` (= タイムラインに出る
  通常投稿) の作成が **X 側で禁止**された。`nullcast=true` は Promoted-Only で
  タイムラインに出ないので用途を満たさない。
- X API v2 に `scheduled_at` 相当は **無い**。

→ **「時刻が来たらローカルから投稿する」以外の方法が無い。**mIV が常駐する / 時刻に
起動される必要があるのは、この制約が理由であって設計の都合ではない。

### 2.2 認証と API

- **OAuth 1.0a のユーザーコンテキスト**。利用者が X Developer Portal で作った App の
  API Key / Secret + Access Token / Secret の 4 つを mIV に貼る。ブラウザ認可フローは
  要らない (= 利用者の要望どおり「API キーを取ってもらって設定」で成立する)。
- Bearer Token (アプリ単位認証) では **投稿できない**。
- App が **v2 Project 配下**に無いと `POST /2/tweets` が 403。Portal の作り方次第で
  詰まるので、設定画面のヘルプに書く。
- App permissions を Read and Write に変えたら **Access Token の再生成が必須**
  (古いトークンは Read のまま)。これは実際に踏まれている落とし穴なので、
  接続確認が 403 のときの案内文に入れる。
- 投稿は v2 `POST /2/tweets`、メディアは v1.1 `media/upload` (xupload が使っている
  経路)。**v2 のメディアアップロードへ移っているかは実装時に確認する**。

### 2.3 課金 (pay-per-use, 2026-02 以降)

| 項目 | 単価 |
| --- | --- |
| Post: Create | $0.015 / 回 |
| Post: Create (**本文に URL を含む**) | **$0.200 / 回** (13 倍) |
| Media Metadata (alt text) | $0.005 / 回 |
| メディアアップロード | 課金表に項目なし |

- **これが「リンク付きだと料金が大きく変わる」の中身。**単価は X の公表値に依存し
  変わり得るので、mIV は**単価を設定値として持ち** (既定 0.015 / 0.200 / 0.005)、
  文言は「URL を含むと単価が上がる」+ 現在の設定値、という書き方にする。
  mIV が「$0.20 かかります」と断定して外したら信用問題になる。
- 空振り (投稿対象なし) では API を 1 回も叩かないので課金ゼロ。ポーリング間隔を
  短くしても費用は増えない。

### 2.4 メディアの制約 (実装時に再確認する)

- 画像 4 枚 **または** GIF 1 **または** 動画 1。**混在不可**。
- 画像 5MB (PNG)、GIF 15MB、動画は容量・尺の上限あり。xupload は 5MB 超を JPEG へ
  再圧縮している (mIV は書き出し時に長辺と品質を持っているので、同じ判断を
  書き出し設定として持つ)。
- 動画は chunked upload (INIT / APPEND / FINALIZE) + 変換処理の完了待ちが要る。
  **画像より一段重い**。v1 に含める (§14 決定 8)。
- `media_id` は発行から 24 時間で失効する。**予約時に先行アップロードはしない**
  (数日先の予約で失効するため)。投稿時にアップロードする。

---

## 3. mIV 側の現状 — 再利用する部品

| 部品 | 場所 | 使い方 |
| --- | --- | --- |
| HTTPS クライアント | `ureq` (rustls, 既存依存) | X API 呼び出し。OpenSSL 非依存で crt-static と両立 |
| DPAPI 秘密保存 | [src/pdf_passwords.rs](../src/pdf_passwords.rs) | 4 つの API キーの保存にそのまま踏襲 (`Scope::User`) |
| SQLite | `rusqlite` (bundled) | 予約キュー `x_posts.db` |
| ローカル HTTP | `tiny_http` (remote-web が既に使用) | ループバック API。core にも依存追加する |
| 合成 → ファイル書き出し | [src/export_batch.rs](../src/export_batch.rs) / `books::write_composited_page` | 予約時にメディアを**焼き込んで凍結**する (§5.2) |
| X の表示実測 | [sns-split-export-plan.md](sns-split-export-plan.md) §2.1 | プレビューのカルーセル・継ぎ目 |
| トレイ常駐 | [src/tray.rs](../src/tray.rs) / `Settings.minimize_to_tray_on_close` | 常駐の導線 |
| 別プロセス監督 + token 認証 | [src/remote_ipc/service.rs](../src/remote_ipc/service.rs) / remote-web `auth.rs` | ローカル API の token 設計の下敷き |
| 起動引数 | [src/lib.rs](../src/lib.rs) の `--pdf-worker` 等 | `--x-post-worker` / `--start-minimized` を同じ流儀で足す |

**新規に足す外部クレート**: `hmac` / `sha1` (OAuth 1.0a の HMAC-SHA1 署名)、`tiny_http`。
いずれも純 Rust なので crt-static・ポータブル・署名対象 PE に影響しない。
**新しい exe は作らない** ([sns-split-export-plan.md](sns-split-export-plan.md) §7-1 と
同じ理由: 署名対象 PE / launcher 内包 / ポータブル同梱 / AV 誤検知面を増やさない)。

---

## 4. アーキテクチャ

### 4.1 所有権

```
Windows タスクスケジューラ (5 分間隔・ログオン中のみ・1 タスク)
└─ mimageviewer.exe --x-post-worker
    └─ mimageviewer-core.exe --x-post-worker   ← ★ 投稿するのはここだけ
        GUI を作らず、DB から due を claim → 投稿 → 結果を DB へ → 終了

mimageviewer-core.exe (通常の GUI)
├─ UI スレッド (App)
│   └─ 作成ダイアログ / 予約一覧 / 設定 …… 予約を DB へ書くだけ。投稿はしない
└─ ローカル API スレッド (設定 ON のときだけ)
    └─ tiny_http on 127.0.0.1 → 検証 → staging へ凍結 → DB へ書く
```

- **投稿する主体は 1 つだけ** — `--x-post-worker` の一発プロセス。GUI は投稿経路を
  持たない。「今すぐ投稿」も `scheduled_at = now` の予約を作って worker を spawn する
  ので、経路は 1 本に保たれる (CLAUDE.md「1 つの意味を 2 か所に書かない」)。
  例外は**接続確認**だけで、これは投稿ではないので GUI が直接叩く。
- **唯一の真実は `x_posts.db`**。UI・API・worker はすべて DB を介してだけ会話する。
  「UI が持っている予約」と「worker が持っている予約」を別々に持たない。
- GUI と worker は**別プロセスから同じ DB を触る**。`journal_mode=WAL` と
  `busy_timeout` を明示的に設定する (rusqlite の既定 busy_timeout は 5 秒だが、
  `journal_mode` の変更自体が競合で即 `locked` になり得るので順序に注意)。
- **投稿の粒度はタスクの間隔 (既定 5 分)**。予約時刻ちょうどではなく最大 5 分遅れる。
  xupload が 5 分間隔で運用して実害が無かった値をそのまま採る。間隔は設定で 1〜15 分。
  空振りでは API を 1 回も叩かないので、間隔を短くしても課金は増えない (§2.3)。

### 4.2 投稿は UI・GPU に一切依存しない

予約時にメディアを**ファイルとして凍結**する (§5.2) ため、投稿の瞬間に必要なのは
「ファイルを読んで HTTP を叩く」だけ。画像は補正・AI・注釈を合成して書き出し、動画と
GIF は元ファイルをそのまま複製する。合成パイプライン (GPU / UI スレッド) は投稿時には
走らない。これは

- 元ファイルが移動・削除・再編集されても予約内容が変わらない
- トレイ常駐中 (ウィンドウ非表示 = サーフェス無し) でも投稿できる
- 一発起動ワーカーでも同じ結果になる

の 3 つを同時に満たすための中心的な判断。

### 4.3 worker プロセスは「早期分岐」で軽く始める

タスクが 5 分ごとに core を起動するので、**投稿対象が無いときの起動が軽くなければ
ならない**。`--x-post-worker` は [src/lib.rs](../src/lib.rs) の `--pdf-worker` /
`--tensorrt-build` と同じく **GUI 初期化より前**で分岐し、次の 3 つだけを触る:

1. data_dir の解決
2. `x_posts.db` を開いて due を claim
3. due があれば `x_credentials.json` を DPAPI で復号して投稿

**`settings.db` は開かない。** 設定 DB には boot decision tree / bak1..bak10 ローテーション /
quarantine があり、5 分ごとに別プロセスから触ると事故の面が増える (2026-07-19 に
実際に main と bak を順に quarantine した前例がある)。worker が必要とする設定
(猶予時間・連投間隔・単価・上限) は、**GUI が設定を保存するたびに
`x_post_config.json` へ写す**。書き手は GUI だけ、読み手は worker だけにする。

単一インスタンス mutex も取らない (GUI と同時に走ってよい)。多重起動の抑止は
§6.1 の claim が担う。

### 4.4 アイドル時に UI を起こさない

GUI 側は**投稿のためのタイマーを持たない**ので、常駐中に定期的に起きる理由が無い。
予約一覧ダイアログを開いている間だけ DB を見る。ここに「念のため 30 秒ごとに DB を
確認する」を足さないこと。足すと `check-idle-health.ps1` の static-background /
tray-residency シナリオが落ちる (CLAUDE.md リリース手順 Phase 2)。

---

## 5. データモデル

### 5.1 資格情報 — `x_credentials.json` (DPAPI)

- 保存先は data_dir 直下。`pdf_passwords.json` と同じ DPAPI `Scope::User` 暗号化。
- **`settings.db` には入れない。**設定 DB は bak1..bak10 の世代バックアップを取り、
  診断 zip や設定の書き出しにも乗り得るため、秘密を置く場所として不適切。
- ログ・診断・perf ログ・API 応答・エラー文言に **key / token / signature を出さない**。
  remote-web の `redact_serialized_secret` と同じ姿勢を最終境界に置く。

### 5.2 予約キュー — `x_posts.db` (SQLite)

```sql
posts(
  id            TEXT PRIMARY KEY,   -- uuid v4
  created_at    INTEGER,            -- unix ms (UTC)
  scheduled_at  INTEGER,            -- unix ms (UTC)。判定はすべてこれ
  scheduled_local TEXT,             -- 'YYYY-MM-DDTHH:MM' 表示・再編集用の見た目の値
  status        TEXT,               -- pending / claimed / posted / failed / expired / canceled / needs_review
  claim_owner   TEXT, claim_at INTEGER,   -- 二重投稿防止の lease
  body          TEXT,
  reply_settings TEXT,              -- everyone / following / mentioned
  attempts      INTEGER, last_error TEXT, last_attempt_at INTEGER,
  posted_at     INTEGER, tweet_id TEXT,
  cost_class    TEXT,               -- plain / with_url (予約時に凍結した判定)
  client_ref    TEXT UNIQUE,        -- ローカル API の冪等キー
  source        TEXT                -- ui / local_api
)
media(
  post_id TEXT, ord INTEGER, kind TEXT,  -- image / gif / video
  path TEXT,                             -- staging 配下の凍結済みファイル
  alt_text TEXT, bytes INTEGER, width INTEGER, height INTEGER, duration_ms INTEGER
)
```

- **staging**: `<data_dir>/x_posts/<id>/1.jpg` …。予約作成時に書き出す。投稿成功 /
  取り消し / 期限切れ確定で削除する。孤児の掃除は起動時に 1 回 (DB に無い
  ディレクトリを消す)。
- **時刻**: 判定は UTC epoch。入力・表示はローカル壁時計。変換は Win32 の
  `TzSpecificLocalTimeToSystemTime` / `SystemTimeToTzSpecificLocalTime` を使う
  (mIV は chrono を持たず、既に Win32 変換で日付を出している)。夏時間のある地域でも
  「壁時計で 21:00」が正しく解決される。
- **リリース済みではない**ので、実装中のスキーマ変更にマイグレーションは要らない
  (CLAUDE.md「永続データ・スキーマ変更時の判断」)。出荷後は必要。

---

## 6. 投稿の実行

### 6.1 1 回の worker 起動でやること

1. `status='pending' AND scheduled_at <= now` を古い順に取得。
2. `UPDATE ... SET status='claimed', claim_owner=?, claim_at=? WHERE id=? AND status='pending'`
   の 1 行更新で **claim** する。更新行数が 0 なら他が取ったので飛ばす。
   前回の worker がクラッシュして `claimed` のまま残った行は、`claim_at` が
   一定時間 (既定 30 分) より古ければ引き継ぐ。これで、タスクが重なって起動しても /
   利用者が「今すぐ投稿」を押しても / 前回が異常終了していても二重投稿しない。
3. 期限切れ判定: `now - scheduled_at > 猶予 (既定 6 時間、設定可)` なら投稿せず
   `expired`。**PC が寝ていた後に何日分もまとめて投下しないための安全弁**
   (xupload の `GRACE_HOURS` と同じ理由・同じ既定)。
4. 連投間隔: 溜まっていた場合は既定 45 秒あける (xupload と同じ)。

### 6.2 失敗の扱い — at-most-once を守る

| 失敗の種類 | 扱い |
| --- | --- |
| 送信前 (メディア読み込み / 署名 / DNS / 接続失敗) | 再試行可。1 / 5 / 15 / 60 分のバックオフ、上限回数まで。猶予窓を超えたら `expired` |
| 4xx で確定 (401 / 403 / 400 / duplicate) | 恒久失敗。`failed` にして再試行しない。原因が消えるまで叩き続けない |
| 429 | `x-rate-limit-reset` まで待つ |
| 5xx | 再試行可 |
| **`POST /2/tweets` を送った後に応答が取れなかった** (タイムアウト / 接続断) | **自動再試行しない。`needs_review` にして利用者へ出す。** 投稿されたかどうか分からない状態で再送すると二重投稿になる。判定は「利用者が X を見て決める」以外に安全な方法が無い |

`needs_review` は一覧で目立たせ、「投稿済みにする (tweet ID を貼る)」/「未投稿として
再試行する」/「破棄」の 3 択を出す。

### 6.3 通知

- 失敗 / 期限切れ / needs_review は **在庫として残す**。次に mIV を触ったときに
  一覧と本体側のバナーで気付ける。トレイのツールチップにも件数を出す。
- Windows のバルーン通知は tray-icon 0.20 が口を持たないので、必要なら
  `Shell_NotifyIcon` の `NIF_INFO` を自前で足す (§14 の未決 6)。

---

## 7. 作成 UI

### 7.1 導線

- フルスクリーンの現在ページ、またはグリッドの選択 (1〜4 件) から
  「**X に投稿 / 予約**」。
- **SNS 分割書き出しからの連携**: 分割した N 枚をそのまま添付して開く
  ([sns-split-export-plan.md](sns-split-export-plan.md) の出口)。分割はもともと X の
  カルーセルのために作った機能なので、ここが最も自然な入口になる。
- `KeyAction` を足す (既定割り当てなし。ボタン / メニューが入口)。
  `ini_name()` / `context()` / `trigger()` / `default_chords()` / `ALL_ACTIONS` /
  [keymap.ini.default](keymap.ini.default) を揃える。

### 7.2 ダイアログの中身

- 本文 (複数行。**IME 対応必須** — `dialog_enter_pressed` / `dialog_escape_pressed` と
  `ime_focus` helper 経由の TextEdit。生 TextEdit は unit test で禁止されている)
- 添付 (並べ替え・削除・alt text)
- 投稿時刻 (日付 + 時刻。「今すぐ」「30 分後」「明日 9:00」などのクイック指定)
- 返信可能範囲 (everyone / following / mentioned)
- **文字数**: X の重み付き文字数 (ASCII = 1、日本語 = 2、URL は長さに関係なく 23)。
  上限を超えたら予約ボタンを無効化し、理由を出す。上限は既定 280 で、設定で
  伸ばせる (X Premium の長文投稿用)。**既定を伸ばさない** — Premium 未契約の
  利用者が投稿時に弾かれるほうが害が大きい。
- **費用**: URL を含むかどうかで単価表示を変える。alt text を付けた枚数分の
  Media Metadata も足す。予約一覧には当月の見込み合計も出す。
- **常駐していないときの警告**: 「mIV が起動していない時刻の予約は投稿されません」
  + 常駐設定へのリンク。

### 7.3 プレビュー

- アバター / 表示名 / @handle / 時刻 / 本文 / メディア / 反応アイコン (灰色) の
  カードを描く。プロフィールは設定画面の「アカウント情報を取得」ボタンを押した
  ときだけ API から取得してキャッシュする (勝手に課金対象の API を叩かない)。
- メディアの並びは [sns-split-export-plan.md](sns-split-export-plan.md) §2.1 の実測に
  従う。**2 枚以上はカルーセル** (1 枚目の全体 + 2 枚目の 3/4 が見える)。旧来の
  2x2 グリッドで描かない。継ぎ目比は 1.7 % を既定にする。
- 本文の折り返し・リンク色・ハッシュタグ色を再現する。
- **「これで必ずこう見える」と書かない。**環境ごとに隙間も幅も違う (同 §2.1)。
  プレビューは構図と文字数の確認用であって校正用ではない、と UI に明記する。

---

## 8. ローカル API

### 8.1 境界

- `127.0.0.1` **のみ** に bind する (`0.0.0.0` にしない)。加えて受信時に peer が
  ループバックであることを確認する (多重防御)。
- `Authorization: Bearer <token>`。token は 256 bit 乱数、比較は定数時間。
- token と実ポートは `<data_dir>/x_local_api.json` に書き、設定画面からも
  コピー / 再生成できる。
- **既定 OFF**。有効化は設定画面での明示操作。この API は「金がかかる公開投稿」を
  作れるので、無言で開けてはいけない。
- リモート閲覧 (mIV Remote) とは**完全に別物**。PIN / Tailscale の認証境界には
  乗せない。混ぜると閲覧用の公開面に投稿権限が漏れる。

### 8.2 エンドポイント (v1)

| メソッド | パス | 用途 |
| --- | --- | --- |
| GET | `/api/v1/ping` | 版・protocol 確認 |
| POST | `/api/v1/x/schedules` | 予約作成。`client_ref` で冪等 |
| GET | `/api/v1/x/schedules` | 一覧 |
| GET/PATCH/DELETE | `/api/v1/x/schedules/{id}` | 取得 / 変更 / 取り消し |
| GET | `/api/v1/x/history` | 投稿済み・失敗の履歴 |

作成の body (案):

```json
{
  "client_ref": "xupload:20260910_0930",
  "scheduled_at": "2026-09-10T09:30:00+09:00",
  "body": "本文",
  "media": [{ "path": "C:\\images\\a.png", "alt_text": null }],
  "reply_settings": "everyone"
}
```

応答は `id` / `status` / `warnings` (`url_detected` / `over_length` /
`media_too_large` / `not_resident`) / `estimated_cost`。
**受理時にメディアを staging へコピーして凍結**するので、呼び出し側は元ファイルを
自由に動かしてよい。

### 8.3 xupload の移行

現在の xupload は「`images/YYYYMMDD_HHMM.png` へリネームして置く」ことが予約であり、
`post_worker.py` が 5 分ごとに投稿している。移行後は editor が
`POST /api/v1/x/schedules` を叩くだけにし、`post_worker.py` は撤去できる。
**mIV 側の API はこの移行が成立する形で決める** (= 画像パス + 本文 + 時刻の 3 つで
予約が作れること、`client_ref` で二重登録が防げること)。

---

## 9. タスクスケジューラ・常駐・OS 自動起動

### 9.1 タスクスケジューラへの登録 (投稿の実行主体)

- タスクは **1 件だけ**。名前は `mImageViewer X 予約投稿` のように、
  タスクスケジューラの一覧で見て何か分かる名前にする。**予約ごとにタスクを作らない。**
- トリガ: ログオン時 + **5 分間隔で無期限に繰り返す**。「ログオンしているときのみ実行」。
  最上位の特権は不要。
- 実行するコマンドは **launcher (`mimageviewer.exe`) + `--x-post-worker`**。
  `%APPDATA%\mimageviewer\runtime\<version>\mimageviewer-core.exe` を直接登録すると
  **更新のたびにパスが変わって壊れる**。launcher のパスはインストール先で安定している。
  - launcher 側に 1 点だけ手を入れる: `--x-post-worker` が付いているときは
    `try_activate_existing`(= 既存インスタンスを前面に出す) を**しない**。
    ここを直さないと、mIV が起動中はタスクがウィンドウを前面に出すだけで終わる。
  - ポータブル版は launcher を使わず core が `mimageviewer.exe` なので、そのまま登録できる。
- 登録・解除は **利用者の明示操作 (設定のチェックボックス)** でだけ行う。
  GUI 起動時に「登録済みタスクの実行パスが現在の exe と違う」場合だけ黙って直す
  (インストール先変更・ポータブルの移動への自己修復)。
- 実装は Task Scheduler の COM API (`ITaskService`、windows crate の
  `Win32_System_TaskScheduler`)。`schtasks.exe` を叩くとコンソールが一瞬出る。

> **注意**: 「自分自身を定期起動するタスクを作るプログラム」は AV のヒューリスティクスと
> 相性が悪い。mIV は既にポータブル版で AV 誤検知を踏んでいる。緩和は
> ①配布 PE は全て Authenticode 署名済み ②利用者の明示操作でしか作らない
> ③名前と実行コマンドを一覧で読める形にする、の 3 点。**無言で作らないこと**が一番効く。

### 9.2 常駐 (ローカル API を使うときだけ必要)

- 投稿はタスクが担うので、**予約のためだけなら常駐は要らない**。
- ただし**ローカル API は mIV が起動していないと受けられない**。xupload から予約を
  投げる運用にするなら常駐が要る。設定画面ではこの依存関係をそのまま書く
  (「常駐しないと予約が飛ぶ」ではなく「常駐しないと外部から予約を登録できない」)。
- 常駐中は §4.4 のとおり UI を起こさない。

### 9.3 OS 起動時の自動起動 (最小化)

- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` に値を作る。管理者権限不要、
  ユーザー単位。設定 ON/OFF がそのまま値の作成 / 削除。
- **登録するパスは launcher (`mimageviewer.exe`)** であって
  `%APPDATA%\mimageviewer\runtime\<version>\mimageviewer-core.exe` ではない。
  runtime ディレクトリは版ごとに変わるので、core を直接登録すると更新後に壊れる。
  core は自分が launcher から起動されたかを知る必要がある (env / 引数で受け取る)。
- ポータブル版はフォルダごと移動され得るので、登録時のパスが消えていたら設定画面に
  「登録先が見つかりません」と出して再登録を促す。
- 起動引数 `--start-minimized`: ウィンドウを表示せずトレイへ入る。

**実装上の risk**: 「最初から非表示で起動する」は、この codebase で過去に実害が出た
**サーフェス無しフレーム**の状態そのもの
([tray-residency-cpu-spin-investigation.md](tray-residency-cpu-spin-investigation.md)、
および FS 終了直後の no-surface フレームでテクスチャアップロードを捨てて黒く固着した
v1.8.0 の回帰)。`ViewportBuilder::with_visible(false)` で起動したときに

- テクスチャアップロードが捨てられて黒くならないか
- repaint が来ず初期化が進まないまま止まらないか
- 逆に repaint を回し続けて CPU を食わないか

を `check-idle-health.ps1 -Scenario static-background` / `tray-residency` で確認する。
**ここは設計より実測が先**で、成立しないなら「一瞬表示してから畳む」に落とす。

---

## 10. 設定項目 (案)

環境設定に新ページ `XPost` を足す。`Startup` ページには自動起動を足す。

| 設定 | 既定 | 備考 |
| --- | --- | --- |
| X 予約投稿を使う | OFF | これが OFF なら UI もタスクも API も出てこない |
| API キー 4 種 | 空 | DPAPI。接続確認ボタン付き |
| 予約時刻に自動投稿する (タスク登録) | OFF | ON でタスクスケジューラに 1 タスク作る。OFF で消す (§9.1) |
| タスク間隔 | 5 分 | 1〜15 分。投稿はここまで遅れ得る |
| 猶予時間 | 6 時間 | 超えたら投稿しない |
| 連投間隔 | 45 秒 | |
| 単価 (通常 / URL 入り / alt text) | 0.015 / 0.200 / 0.005 | 表示専用。X の公表値が変わったら利用者が直す |
| ローカル API | OFF | port / token / 再生成 |
| OS 起動時に自動起動 (最小化) | OFF | `Startup` ページ |
| 書き出し上限 (長辺 / 品質) | 長辺 2048 / 自動 | SNS 分割の既定に揃える |

---

## 11. 非対象

| やらないこと | 理由 |
| --- | --- |
| **pixiv への自動投稿** | 公式の投稿 API が存在せず、実現手段がブラウザ自動化しかない。**投稿画面を開いて画像を用意するところまでの「下ごしらえ」は別途検討中** (2026-09-06、§11.1) |
| OAuth 2.0 のブラウザ認可フロー | 利用者が Portal でキーを取る前提。リダイレクト受けの口を増やさない |
| 予約を X 側へ登録する | §2.1 のとおり不可能 |
| 投稿の閲覧・返信・DM・分析 | ビューアの範囲を超える。読み取り API を持たない |
| 複数アカウントの切り替え | v1 は 1 アカウント。DB は将来の口だけ空けておく |
| リモート (mIV Remote) からの予約 | 認証境界が違う (§8.1)。要望が出たら別途設計 |

### 11.1 pixiv — どこまでなら成立するか (2026-09-06 の調査)

利用者から「投稿画面を開いて画像をアップロードするところまでで、実際の投稿操作は
利用者に委ねる形なら規約上の問題を回避できるか」という問いがあったので調べた。

**pixiv 利用規約 (policies.pixiv.net) を読んだ範囲では、「自動化されたプログラムに
よるアクセス」を名指しで一律に禁じる条項は見つからなかった。**関係しそうなのは

- 共通規約の禁止事項のうち「通常の範囲を超えてサーバーに負担をかける行為」
  「不正な操作」「その他当社が不適切と判断する行為」
- 登録商標ガイドラインの「クローラーなどのプログラムを使って作品を収集する行為の禁止」

で、いずれも**投稿フォームの自動入力を名指しで禁じてはいない**。ただし
「その他不適切と判断する行為」は開いた条項で、判断は pixiv 側にある。

**それでも mIV に載せるべきではないと考える理由** (法的評価ではなく、製品としての判断):

1. **「アップロードまで」は既に自動化された書き込みである。**pixiv の投稿画面は
   ファイルを選んだ時点でサーバーへ送る。最後の 1 クリックを人がやっても、
   その前段の通信はプログラムが起こしている。**「フォームを埋めるだけ」ではない。**
2. **実装手段が重い。**ファイル input はセキュリティ上 JS から設定できないので、
   CDP (`DOM.setFileInputFiles`) か WebView2 を使って**ブラウザを駆動**することになる。
   前者は Chromium 一式か debugging port 付きの別プロファイル、後者は
   **mIV が pixiv のログインセッション (Cookie) を抱える**ことを意味する。
   privacy.html に「mIV は閲覧サイトのセッションを持たない」と書ける状態を捨てる。
3. **壊れる。**投稿画面の DOM は pixiv 側の都合で変わる。xupload の PoC でも
   セレクタを実測して固定している。配布物に載せると、壊れるたびに mIV の不具合として
   問い合わせが来る。
4. **責任の所在が変わる。**個人スクリプトなら自分のアカウントの自己責任だが、配布物に
   載せると mIV の名前で不特定多数のアカウントが同じ挙動をする。

**採る案 (自動化ゼロ)**: mIV 側は

- 投稿用に**画像を焼き込んで作業フォルダへ書き出す**
- キャプション / タグを**クリップボードへ入れる**
- 既定ブラウザで**投稿ページの URL を開く** (`opener` クレート、既存依存)
- 必要ならエクスプローラでその書き出し先を**選択状態で開く**

までをやる。ファイルを選ぶのも投稿するのも利用者。**pixiv へは 1 バイトも
自動送信しない**ので、規約の解釈に依存しない。手間は「ドラッグ 1 回」しか増えない。

ブラウザ駆動が要る運用は、**引き続き xupload 側 (利用者個人のツール) に置く**。
mIV のローカル API (§8) で画像の焼き込みと予約を受けられるようにすれば、
xupload は「mIV に素材を作らせて自分は pixiv 画面を操作する」だけになる。

---

## 12. 段階分け

実装順序は利用者から「お任せ」(2026-09-06)。**投稿経路を先に実運用へ載せる**順にする。
xupload が既に毎日投稿しているので、早い段階で本物の運用に晒したほうがバグが出る。

| Phase | 内容 | 規模 |
| --- | --- | --- |
| **P1** | OAuth 1.0a 署名 + `ureq` クライアント + DPAPI 資格情報 + 設定ページ + 接続確認 | M |
| **P2** | `x_posts.db` + `--x-post-worker` + claim / 猶予 / 再試行 / `needs_review` + 画像投稿 | M |
| **P3** | タスクスケジューラ登録 / 解除 / 自己修復 (§9.1) | S |
| **P4** | ローカル API + token + xupload 移行 (ここで **xupload を実運用で切り替えられる**) | M |
| **P5** | 作成ダイアログ + 文字数 + 費用警告 + staging 焼き込み | L |
| **P6** | プレビュー (カード / カルーセル / リンク色) | M |
| **P7** | 予約一覧・履歴ダイアログ + 失敗の気付き口 | M |
| **P8** | 動画 / GIF 添付 (chunked upload + 変換待ち) + 長文投稿 | M〜L |
| **P9** | 常駐導線 + OS 自動起動 + `--start-minimized` + idle health 実測 | M |
| **P10** | 文書 (§15) | S |

P1〜P4 は UI をほとんど持たないので、**偽トランスポートを差した単体テストで挙動を
固められる** (claim・猶予・再試行・needs_review・二重投稿防止・冪等キー)。
ここを先に固めてから UI を作る。

---

## 13. 守る不変条件

1. **at-most-once**。応答が取れなかった送信は自動再試行しない (§6.2)。
2. **投稿する主体は 1 つ** (`--x-post-worker`)。GUI に 2 本目の投稿経路を作らない (§4.1)。
3. **投稿は UI / GPU に依存しない** (§4.2)。
4. **秘密は出さない**。ログ・診断 zip・perf ログ・API 応答・エラー文言のどこにも
   key / token / signature を出さない (§5.1)。
5. **アイドル時に UI を起こさない** (§4.4)。リリース前に idle health で確認する。
6. **既定はすべて OFF**。有効化は明示操作。金と公開投稿が絡む機能を無言で開けない。
7. **ローカル API はループバック + token の両方**を満たさないと受け付けない。
8. **単価を断定しない**。設定値として持ち、変わり得ることを明記する。
9. **pixiv へは自動送信しない** (§11.1)。

---

## 14. 決定済み / 未決

### 決定済み

1. 実装は **core 内**。新しい exe を作らない (§3)。
2. メディアは **予約時に焼き込んで凍結**する (§4.2)。
3. 資格情報は **DPAPI**。`settings.db` に入れない (§5.1)。
4. 真実は **`x_posts.db` 1 つ**。UI / API / worker は DB 越しにだけ会話する (§4.1)。
5. **投稿するのは `--x-post-worker` の一発プロセスだけ**。GUI は投稿経路を持たない (§4.1)。

以下は 2026-09-06 の利用者判断:

6. **配布範囲 = 既定 OFF の上級者向けとして配布する**。全ユーザーに届くが、設定で
   キーを入れた人だけ有効。→ マニュアル・privacy.html・製品ページの更新が必須 (§15)。
   X Developer Portal での取得手順と「**課金は利用者の X アカウントに発生する**」を
   明記する。mIV は代金を扱わない。
7. **不在時の保証 = タスクスケジューラ主体**。常駐は投稿の前提にしない (§9.1)。
8. **v1 の投稿形式 = 画像 1〜4 枚 / 動画 1 / アニメ GIF 1 / 長文投稿 (X Premium)**。
   長文は上限を設定で伸ばせるようにし、既定は 280 のままにする (Premium 未契約で
   投稿が弾かれるのを既定にしない)。
9. **連投 (スレッド) は v1 では作らない**。DB は `in_reply_to` の列だけ空けておく。
10. **pixiv は §11 のとおり別判断** (2026-09-06 に再検討中)。

### 未決

1. **プロフィール取得**: 表示名 / @handle / アバターを API から取ってプレビューに
   使うか、設定に手入力させるか。取得は課金対象の API を 1 回叩く。
   → 現案は「設定画面のボタンを押したときだけ取得してキャッシュ」。
2. Windows バルーン通知を自前実装するか (§6.3)。まずは一覧 + バナー + トレイの
   ツールチップだけで足りるかを見る。
3. 動画の**先行アップロード**をするか。`media_id` は 24 時間有効なので、24 時間以内の
   予約なら変換を先に済ませておける。v1 は投稿時アップロードで始め、遅延が問題に
   なってから考える。

---

## 15. 文書の同時更新 (リリース前に必須)

- `htdocs/mimageviewer/manual/` — 新しいページ (予約投稿の使い方、キーの取り方)
- `htdocs/mimageviewer/index.html` の**「安心して使えます」セクション**
- `htdocs/mimageviewer/privacy.html` の**「ネットワーク通信」「端末内に保存される
  データ」**

> この機能は **外部への送信 (X)**、**秘密の保存 (API キー)**、**ローカルの待ち受け
> ポート**を同時に増やす。CLAUDE.md「通信・データ保存に関わる機能を追加したときは
> 2 か所を突き合わせる」の対象そのもの。特に「通信するのは〜だけ」「画像は外部に
> 送信されません」といった**全称・否定の表現が偽になる**ので、必ず両方直す
> (v3.0.0 で同じ取りこぼしを 8 日間公開した前例がある)。

- [spec.md](spec.md) — 設定項目
- [keymap.ini.default](keymap.ini.default) / [keymap-spec.md](keymap-spec.md)
- [architecture-overview.md](architecture-overview.md) — 永続化ストアに `x_posts.db` /
  `x_credentials.json` を追加
- [async-architecture.md](async-architecture.md) — 新しい常駐スレッド 2 本
- `src/version_highlights.rs` — 出荷版の「重要な変更点」
