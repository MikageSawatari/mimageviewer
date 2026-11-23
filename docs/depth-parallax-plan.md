# 3D 写真 (AI 深度による視差表示と書き出し) — 設計書

状態: **設計確定 (2026-11-06)。実装前の独立レビュー 4 回で段 A〜C とも着手可 (§16)**。段 A は実装・指定自動テスト・独立レビュー・確認用 build 完了 (§10)。ClaudeCode の検収待ち。worktree `G:\home\mimageviewer-depth`、ブランチ
`depth-parallax`。調査とスパイクの記録は [depth-parallax-research.md](depth-parallax-research.md)。
試験実装の結果がよければ、**v5 リリースの次のリリース**に入れる (2026-11-06 利用者方針)。
バックログ番号は未採番。

## 0. 利用者の決定 (2026-11-06)

| 項目 | 決定 |
| --- | --- |
| モデル | **DA3 Mono-Large を fp16 化したものだけ**。本体に同梱せず、**追加ダウンロード** (約 0.67 GB) |
| 品質 | スパイクの比較動画で「十分良さそう」(利用者の目視) |
| 操作 | フルスクリーンで見るときに、**パネルで強さ等の項目を選び**、その設定でアニメーション表示と書き出しを行う |
| 動き | 自動の動き (左右など) に加え、**マウスに合わせて見るモード**を付ける |
| 書き出し | GIF / APNG / MP4 (調査時の要望) |

既定値はスパイクの基準 (強さ 2%、左右、周期 3 秒、中間の深さが止まる、奥行き整形あり) を使う。
利用者は比較動画でこの基準を含めて品質を了承しており、個別の値は試験実装で見て調整する。

## 1. 範囲

### v1 に含める

- ウィンドウ表示 / 全画面 / 別ウィンドウの **3D 表示モード** (以下「3D モード」、§6.4.3)。main と独立 context のどちらでも使え、同時に持てるのは 1 context。ON/OFF はキーと閲覧上部の
  ボタン。入ると右側に **3D パネル**が出る。
- 動き: **マウス追従 / 左右 / 円 / 8 の字 / 上下**。強さ・周期・止まる深さ・奥行き整形をパネルで選ぶ。
  設定は画像ごとではなく全体の設定として保存する。
- 書き出し: パネルの「書き出し…」から、今の設定で **MP4 / GIF / APNG** の 1 周期ループを書き出す。
- 対象: 静止画 1 ページ表示。通常ファイル・ZIP 内画像・PDF ページ (表示されている画素から推定するので
  経路の違いは無い。§4)。
- モデルの追加ダウンロード (§3)。
- スライドショー中は次のページの深度を1件先読みし、進行中の準備が終わるまで自動送りを待つ。
  対象範囲・終端の分類・間隔と可視性の契約は [3Dスライドショー設計](depth-slideshow-plan.md) を正本とする。

### v1 に含めない (理由)

| 対象 | 理由 |
| --- | --- |
| 動画、アニメーション画像 (GIF / APNG / WebP) | フレームごとに深度が変わりちらつく。研究調査 §6 と同じ |
| 見開き・連結読み | 2 ページを 1 つの奥行きとして扱えない。360° と同じく入口で弾く |
| **複数 context での同時 3D** | §6.4.3。複数ウィンドウモード・メディアウィンドウも対象だが、一度に 1 context だけが持つ |
| 360° パノラマ表示との同時使用 | どちらも表示を占有するモード。3D に入ると 360° は終わり、3D 中は 360° に入れない (§6.2) |
| 深度の手修正、左右並び (SBS)、2 層化と背景補完 | 研究調査 §6 の v1.x / v2 |
| 手動閲覧中の深度先読み | スライドショー中だけ先読みする。手動送りでは既存の先読み枠の引継ぎのみ行う ([§1.771](depth-slideshow-plan.md)) |
| mIV Remote | 表示はローカルだけ。Remote 側には何も出さない |

## 2. 全体の流れ

```
[D キー / 上部ボタン] → entry_allowed (§6.1) → モデル導入済み? ─no→ ダウンロード案内 (§3.3)、モードは始めない
                                                  │yes
                                                  ▼
                          3D モード ON (context 所有の DepthViewState, §6.2)
                                                  │ 毎フレーム
          表示中テクスチャ (resolve_fs_processed_payload) の TextureId が安定 (§4.1)
                                                  │ 新しい TextureId なら
          GPU で 504×504 の推論入力 + ガイド画像を描いて読み戻す (UI スレッドは非同期待ち, §4.2)
                                                  ▼
          深度 worker: 推論 (mutex 直列) → 前処理 (正規化・guided upsample・膨張・ぼかし・中央値)
                                                  ▼
          UI: 深度を R16Float テクスチャへ上げる (小さい) → 視差シェーダで描画 (§5)
```

結果待ちの間は通常の2D画像を保ち、パネルの状態行と、150ms後に画像上へ出す待機の箱で知らせる。
パネルを隠していても箱を出す。初期化中は「3D 写真のモデルを準備中…」、それ以外は
「奥行きを計算中…」とし、実効キーがある場合だけ「<キー> で取り消し」を小さく添える (§5.4)。

## 3. モデルと追加ダウンロード

### 3.1 配布するファイル

- `da3mono_large_fp16_504.onnx` (約 0.67 GB)。**入力の形を固定済み**のもの:
  `image` [1,1,3,504,504]、`extrinsics_in` [1,1,4,4]、`intrinsics_in` [1,1,3,3]。
  入出力は fp32 のまま (fp16 は重みと内部計算だけ)。
  - 形を固定する理由: DirectML は動的な次元のままだと約 8 倍遅く、DA3 Mono-Large は動的なままだと
    Reshape で失敗する (研究調査 §8.1)。配布ファイル側で固定すれば、ランタイムでの
    dimension override も、サイズごとのセッション (セッションごとに重みを持ち VRAM が倍になる) も要らない。
  - 504×504 1 つに固定する理由: 画像の縦横比のまま推論した場合と比べ、**引き伸ばして 504×504 に
    入れた深度**は 8 枚で相関 0.987〜0.999、縁を複製して余白を足した場合は 0.858〜0.996 だった
    (`check_static.py`、研究調査 §8.6)。引き伸ばしを採る。極端な縦横比 (3:1 以上) では劣化が増える
    見込み (未計測)。
- `LICENSE` (Apache-2.0 の全文) と `NOTICE` (元のモデル名・入手元・**fp16 化と入力形の固定をした旨**)。
- 作り方はスクリプトにする (`tools/depth_model/build_depth_addon.py`: SpatialHub の ONNX → fp16 → 形の固定 →
  sha256 と index の生成)。スパイクの `to_fp16.py` と `check_static.py` を製品用にまとめ直したもの。
  公開手順は配布チャネルの作業として [release-checklist](release-checklist.md) に足す。

### 3.2 ダウンロードの仕組み

- 既存の **VLM アドオン** (`src/vlm_addon.rs` / `vlm_addon_download.rs`) の index 形式
  (`ModelIndex`: ファイルごとの sha256・bytes・license、分割、取り消し) と downloader を、
  **アドオンの種類を引数に取る形へ共通化して使う**。コピーして 2 つ持たない。
  - 実装担当は着手前に VLM 側のコードを読み、共通化で VLM の既存挙動 (保存先・index の URL・
    取り消し・失敗時の表示) が変わらないことを確かめる。変わるなら止めて設計担当へ返す。
  - 共通にするのは転送と配置 (ダウンロード・分割の結合・sha256・一時ファイルからの配置・取り消し)。
    VLM 固有の検証 (4 ファイル必須・GGUF・model ID、vlm_addon.rs:73) は VLM 側に残し、深度側は
    「ONNX 1 つ + LICENSE + NOTICE」の検証を持つ。
  - 編集アドオンパック (フォント + BiRefNet、約 550 MB) には入れない。入れると被写体マスクだけ
    使いたい人も 1.2 GB を落とすことになり、パックのビルダーも全 `*.onnx` を被写体マスクとして扱う。
- 保存先は `<data_dir>/addons/depth/` (VLM と同じ規則。ポータブルはその data_dir 配下)。
- 公開は GitHub Release のタグ `depth-model-v1` (VLM の `vlm-model-v1` と同じ扱い)。
- 導入状態の確認はファイル I/O を伴うので、App に結果を持ち、毎フレームは読まない (編集アドオンと同じ)。

### 3.3 未導入のとき

- lazy scan の未確認は未導入扱いにせず、確認中のモーダルを出す。D／上部3Dボタン由来で
  導入済みと確認できた場合は窓を閉じ、その時点の `entry_allowed` と受付時の画像の同一性
  （context・items世代・位置・item識別子）を再照合して、そのまま3Dへ入る。
  環境設定由来の確認は自動入場しない。確認中に画像が変わっていれば入場要求は終了する。
  ローカル閲覧入力はモーダルで止まるので、再開・rollbackの状態機械は足さない。
  関連付けのopenやRemoteなどモーダル外の変更には、上記の照合と操作権の確認で応答する。
- 未導入なら **ダウンロード案内ダイアログ**を出してモードは始めない。
  ダイアログには大きさ (約 0.67 GB)、ライセンス (Apache-2.0)、入手元、保存先を出す。
- ダウンロード中・完了後に **自動で 3D モードへ入らない**。完了したら現在のキー割当を使って再入場を
  案内する。ダウンロード中にページを移る・フルスクリーンを閉じる、と 3D モードの組み合わせを作らないため。
  導入済み確認だけを継続するのは、起動ごとにDを2回押す手間を除き、短いモーダル確認の既存境界を使うため。
- 設定画面のアドオン一覧 (VLM と同じ場所) からも導入・削除できる。

### 3.4 推論

- `ModelKind::DepthDa3MonoLarge` を足し、既存の `AiRuntime` のセッション管理 (`load_model` /
  `with_session`) で動かす。DirectML、失敗時は既存どおり CPU。
- 入力: 504×504 の sRGB RGBA (§4.2) → ImageNet の平均・分散で正規化 → [1,1,3,504,504]。
  カメラ入力は −1 埋め (SpatialHub の書き出しの約束)。
- 出力: `depth` と `sky` [1,1,504,504]。`sky > 0.5` は視差 0、それ以外は `1 / depth` (有限・正のときだけ)。
- **所要時間** (研究調査 §8.5): RTX 4090 で約 30〜45 ms、内蔵 GPU で約 1 秒、CPU のみで約 3.6 秒。
  初回はセッション作成と DirectML のコンパイルで 2〜3.5 秒。モデルは他の AI モデルと同じく
  一度読み込んだら保持する (VRAM 約 1.3 GB)。解放は既存のモデル再読み込みの経路だけ。
- **AI の mutex**: mIV の DirectML 推論はすべて `AiRuntime.sessions` の 1 つの mutex で直列になる。
  深度推論の間 (dGPU で数十 ms、内蔵 GPU で 1 秒、CPU で数秒) は AI 拡大のタイルも待つ。
  3D モード中は AI 拡大が後回しになることを受け入れる (優先度の仕組みは足さない)。
- 推論中は `local_ai_activity_lease()` を持つ (Remote の取得待ちが数える)。

## 4. 深度の入力と前処理

### 4.1 「表示しているもの」を 1 つの鍵にする

深度は **今画面に出している画素**から作る。補正・AI 拡大・消しゴム・ZIP / PDF のどれで作られた
画素かを 3D 側で選び直さない。表示側が毎フレーム選んでいるテクスチャをそのまま使う。

- 入口: `resolve_fs_processed_payload` (ui_fullscreen.rs、`resolve_fs_processed_texture` の中身) が返す
  `FullscreenPaintResource` の元テクスチャ。表示の優先順位 (display-pipeline §2.3) の正本はこの関数で、
  3D 側に同じ判断をもう 1 つ書かない。
- **鍵は `DepthSourceKey { texture_id: egui::TextureId }`**。egui の managed texture の id は
  プロセス内で再利用されない。画素が変われば (補正の確定、AI 拡大の到着、ページ移動) 表示側が
  別のテクスチャを作るので、鍵も自然に変わる。final / processed の経路で同じ id のまま画素を
  書き換える箇所は無い (独立レビューの確認、app.rs:89872 付近の新規生成)。実装担当は着手時に
  `TextureHandle::set` の呼び出し元を一覧にして確かめ直す。
- **素材の契約 (表示側が返す)**: 3D 側は表示側から次の 1 つを受け取り、自分で分岐を足さない。

  ```rust
  pub enum DepthSourceMaterial {
      Ready { texture: egui::TextureHandle },   // 表示側がこれ以上差し替える予定の無い、読める元テクスチャ
      NotYet(DepthSourceWait),                  // 2D のまま待つ (理由はパネル表示とテスト用)
  }
  ```

  `NotYet` になるのは: サムネイルの fallback、ページ送り中の通過表示、`complete=false` の final
  composite、元画像ホールド中、**注釈のベイク待ち** (comic が None を返し完成済みの下地へ fallback
  している間。下地の `complete=true` だけでは最終と言えない、app.rs:90704 / ui_fullscreen.rs:13396)、
  **元テクスチャを持たない Native の presentation** (保持済みの GPU 縮小結果など。`source_texture()` が
  意図的に `None`、ui_fullscreen.rs:11953 / gpu_lanczos.rs:938)。
  - Native だけが残っている場合、3D 側は既存の元素材の再準備の経路 (紙めくりから戻るときと同じ経路) を
    要求する。実装担当はその経路を特定し、無ければ設計担当へ返す (3D 用の再準備を新しく書かない)。
  - RAW の現像待ちで元画像の preview を出している間は、その preview を最終として扱う。現像が終われば
    表示側が別テクスチャへ替えるので、鍵が変わって深度を作り直す。
  - 判定は `resolve_fs_processed_payload` の分岐が知っている事実を返すだけにする。待ち時間の窓は使わない。
- 回転 (R / L の 90° 単位) はテクスチャに焼き込まれていないので、深度もテクスチャと同じ向きで作り、
  描画で両方に同じ回転をかける (§5.2)。

### 4.2 GPU から推論入力を作る (UI スレッドを止めない)

CPU の画素 (`ColorImage`) を探さず、**表示中のテクスチャを GPU で縮小して読み戻す**。

1. 表示中テクスチャの `wgpu::Texture` を `renderer.read().texture(&id)` で借りる (gpu_lanczos.rs と同じ。
   backing が `None` なら準備できていないとして 2D のまま)。
2. 3D 用の**色テクスチャ**を作る: egui の managed texture は `TEXTURE_BINDING | COPY_DST` だけで
   `COPY_SRC` が無い (vendor/egui-wgpu/src/renderer.rs:776) ので、`copy_texture_to_texture` は使えない。
   **元テクスチャをサンプルして level 0 へ描く blit** を行い、そのあと mipmap を生成する
   (`egui_wgpu::MipmapGenerator`、360° と同じ)。縮小表示でのちらつきを防ぐため、3D の描画と
   書き出しはこの mip 付きの色テクスチャから読む。CPU からの大きなアップロードは無い。
   - **色空間**: egui のテクスチャは sRGB で符号化した値を `Rgba8Unorm` に入れている
     (renderer.rs:782)。framebuffer も非 sRGB の形式が優先される (vendor/egui-wgpu/src/lib.rs:526)。
     3D の色テクスチャ・読み戻し・書き出しの描画先は**すべて `Rgba8Unorm`** にそろえ、シェーダは
     値をそのまま通す (sRGB の変換を挟まない)。推論入力もこの値 (= スパイクと同じ sRGB の 8 bit) になる。
3. 色テクスチャから 2 枚を描く: **推論入力** 504×504 (縦横比を無視して引き伸ばし) と
   **ガイド画像** (長辺 1536、元より大きくしない。縦横比はテクスチャのまま)。
4. 2 枚を `copy_texture_to_buffer` → `map_async`。**UI スレッドでは `poll(Wait)` しない**。
   map の callback は `Queue::submit` でも進む (wgpu-30 buffer.rs:363) ので、常時の別 poll は要らない。
5. **起こす責任**: egui は再描画の要求が無いと休む (ui-responsiveness.md:249)。マウス追従で
   ポインタが止まっていても進むよう、`ReadingBack` の間は毎フレーム非ブロッキングの
   `device.poll(PollType::Poll)` と `request_repaint` を行う。map が完了した frame で worker へ渡す。
   worker は結果を送ったあと、**要求元の viewport** に `request_repaint_of` を出す (要求に viewport id を持たせる)。
   - 回帰検査: ポインタを動かさずに 3D を ON にして `Ready` まで進むこと。

読み戻すのは 1 MB + 長辺 1536 の 9 MB 程度で、8K の画像でも CPU 側の大きなコピーは発生しない。

### 4.3 前処理 (深度 worker、スパイクの `parallax.prepare_depth` と同じ)

1. 視差を 1% / 99% パーセンタイルで 0〜1 に正規化
2. ガイド画像の大きさへ bicubic で拡大
3. ガイド画像の輝度で guided filter (半径 `max(2, round(長辺 / 256))`、eps 1e-3)
4. 0〜1 に clip、楕円で 3 px 膨張 (前景を少し太らせ、輪郭のハローを減らす)、σ 1.5 px のぼかし
   (px は長辺 1600 基準。長辺に比例させる)
5. **中央値**を求めて結果に添える (奥行き整形とシェーダの既定の焦点に使う)

奥行き整形 (中央値を 0.5 へ持っていくガンマ) と焦点は**パネルの設定なので worker では掛けない**。
シェーダで `pow(d, g)` を掛ける (§5.1)。設定を動かしても再計算は要らない。

結果は `DepthMap { key, width, height, values: Vec<f16>, median }`。UI スレッドで R16Float テクスチャに
上げる (長辺 1536 で約 3.5 MB、1 フレーム 1 枚。`fs_upload_backlog` の上限の考え方に合わせる)。

### 4.4 深度 worker

- プロセスに 1 本の常駐 thread `depth-estimation`。要求は **1 つの slot (最新が勝つ)** を
  `Mutex + Condvar` で渡す (coding-conventions の並行処理。try_lock の再試行ループは書かない)。
  3D モードを持てるのはプロセス全体で 1 context (§6.4.3) なので、同時に要る要求は 1 つ。
- 要求: `DepthRequest { context, request_id, viewport, key, input_504, guide, cancel: CancelToken, reply }`。
  - `request_id` は単調増加。**要求の寿命は `TextureId` ではなく context + `request_id` で識別する**。
    同じテクスチャで OFF → ON した場合に、取り消した前の要求の結果を受け取らないため。
  - `reply` は**要求ごとの受け口** (pending が持つ専用の receiver)。共有 channel にしない。
    pending を捨てれば受け口も消え、結果は届き先を失って捨てられる。
  - ランタイム初期化待ちは `AiRuntimeInitOwner::wait_terminal_while(|| !cancel)` (ui-responsiveness §2.1.2)。
  - 推論そのものは途中で止められない (数十 ms〜数秒)。終わったら cancel を見て、取り消し済みなら捨てる。
- UI は pending の受け口から受け取り、鍵が `DepthViewState` の今欲しい鍵と一致するときだけ採る。
  取り消しは: 3D モード OFF、フルスクリーン終了、フォルダ移動、context の terminal close
  (`ViewerContextBundle::cancel_all_context_work`)、アプリ終了。要求の `Drop` で cancel を立てる
  (BiRefNet の pending と同じ)。slot に新しい要求を置くと、前の要求は取り消し扱いになる。
- 深度の LRU: context 所有で 4 件 (GPU の R16Float テクスチャ + 中央値)。前後のページへ戻る・
  補正の ON/OFF で元のテクスチャに戻る、ときに計算し直さないため。ディスクには保存しない。

## 5. 描画

### 5.1 シェーダ (wgpu の paint callback)

- 360° と同じく `egui_wgpu::CallbackTrait`。**callback はテクスチャを持たない**。色テクスチャ・深度
  テクスチャ・bind group は context 所有の `DepthViewState` が `Arc` で持ち、毎フレーム
  `callback_resources` に入れる。paint 時に `DepthSourceKey` と context の一致を確かめてから描く
  (panorama_wgpu.rs の stale guard と同じ)。
- **slot の後始末**: `callback_resources` に入れた `Arc` は state を捨てただけでは消えない。
  360° と同じく (`clear_pano_upload`、app.rs:95685)、OFF・フルスクリーン終了・context の terminal close で
  **所有 context が一致する slot を明示的に取り除く**。
- **VRAM の会計**: 色テクスチャ (mip 込み。8K で約 340 MB) と深度テクスチャの LRU を、既存の VRAM
  accountant に登録する (async-architecture.md §4 の末尾「新しいテクスチャキャッシュを追加する時は」)。
  色テクスチャは今のページの 1 枚だけ持つ。
- アルゴリズムはスパイクの `parallax.render_view` (研究調査 §4)。画面上の点ごとに、手前から奥へ
  40 段で深度面を探し、二分探索 5 回で詰める。深さ `t` の面は `offset × (t − focus)` だけずれて見える。
- uniform: `offset` (画像幅に対する割合、x / y)、`focus`、`gamma`、`zoom`、uv の範囲、回転、段数。
  - `gamma`: 奥行き整形が ON なら `clamp(ln 0.5 / ln median, 0.3, 1.5)`、OFF なら 1。
    `median` が 0.02〜0.98 の外なら 1。
  - `zoom = 1 + 2.2 × 強さ`。画像の外が見えないよう少し拡大する。
- サンプラは clamp-to-edge + trilinear。uv は表示している範囲 (切り取り後の範囲) の中に clamp する。

### 5.2 置き場所と変換

- 画像の四角形と uv の範囲・90° 回転は、**通常の 2D 描画が計算したもの**を使う。
  2D と倍率・パン・自由回転・フィット設定を共有し、入る前と抜けた後にも同じ表示状態を使う。
  3D 独自の表示変換は持たない ([表示変換設計](depth-view-transform-plan.md) §3.1〜§3.2)。
- callback は四角形の外接矩形と painter の clip の交わり。絶対フラグメント位置を自由回転の逆変換、
  paint_rect、局所 uv の順に写し、画像外は最後の select で透明にする。微分は分岐より前に求める。
  動きは画面の軸を保つため offset を自由回転の逆向きへ回してから aspect を掛ける。
- Depth の採用判定は素材の鍵まで照合し、2D の GPU 縮小準備を通らない。素材決定後の初期配置とパン補正の
  確定は 2D と共通の境界を使う (同設計 §3.5.1)。
- 画像の外の背景・上部バー・ステータスは通常どおり。

### 5.3 動きと時刻

- 自動の動きは時刻だけで決まる関数 `motion_offset(kind, strength, phase)`。
  `phase = (経過時間 / 周期) mod 1`。左右 = (sin, 0)、円 = (cos, 0.6 sin)、8 の字 = (sin, 0.5 sin 2θ)、
  上下 = (0, 0.7 sin)。書き出しは同じ関数に `phase = i / N` を渡す (継ぎ目が出ない)。
- **マウス追従**: 画像の外接矩形とキャンバスの交わりの中心 (交わりが空ならキャンバスの中心) からの
  ポインタ位置を、呼出元の media rect の短辺 × 0.25 を半径として
  縦横同じ分母で −1〜1 に正規化し、`offset = −強さ × (nx, 0.6 ny)`
  を目標にして時定数 0.15 秒で近づける。ポインタが 3D パネルの上にあるときは目標を動かさない
  (パネルを操作すると絵が揺れるのを防ぐ)。ポインタが窓の外なら中心へ戻す。
  基準の逆引き shader で手前がポインタから離れる向きとする (右 pointer → 手前は左)。
  画像全体が見えるときは片側配置を含め従来の画像中心と一致する。自由回転時は外接矩形の近似であり、
  実画像内の中心は保証しない ([表示変換設計](depth-view-transform-plan.md) §3.3)。
  段 B の純粋追従・GPU sample 方向のテストで固定し、実機の操作感は別に確認する。
- 再描画: 自動の動きの間は毎フレーム `request_repaint`。マウス追従は目標に近づき切るまで。
  フルスクリーンの viewport が最小化・非表示のときは要求しない。
  - 3D の自動の動きは常に再描画するので、アイドル時の負荷検査 (`check-idle-health.ps1`) の対象外になる
    ことを development-build-and-test-native に書く。

### 5.4 結果待ちの表示

`FsExclusiveView::Depth` で source phase が `Ready` / `Failed` 以外の間は、素材 `NotYet` も含めて
画像上に非モーダルの状態箱を出す。状態の正本は `depth_wait_status` 一つとし、パネルの状態行と共有する。
runtime が Dormant / Initializing なら準備中、それ以外は計算中。Failed のパネル文言と既存通知は維持し、箱は出さない。

見た目は既存のアニメーション展開中表示と同じ角丸・半透明の暗い箱、17ptの白文字、下の細い不定進捗バー。
初回入場と terminal から再び待つ時だけ提示時計を開始し、NotYet → ReadingBack → Estimating や待ち中の素材変更では
一続きのbusy表示を維持する。150msはちらつき防止だけの条件で、仕事の生存・完了・取消を決めない。
上部chromeと下のページ情報、現在表示中の右パネルを除いた画像領域に置き、1093×614 logicalで収める。
取消の2行目は keymap の現在の実効ラベルを使い、未割当なら省く。

gateに達するための遅延wakeを一度予約し、箱が見えている間だけ33ms周期でバーを再描画する。
hidden / minimized は予約せず、Ready / Failed / OFFでは箱と箱由来の再描画を止める。
結果採用の起床と自動の3D運動は既存ownerが引き続き担当する。

**設計の簡素化の検討**: modalも検討したが、待ちは初回2〜3.5秒、以後はページごとの短い推論であり、
毎ページmodalにすると通常の閲覧に操作待ちを加える。待ち中もDでの取消とページ送りを受け付ける必要がある。
割込み・要求取消・新しい画像での再推論は段Bの要求ownerが既に扱っており、新しい状態の組合せは増えないため、
非モーダルの提示だけを足す。

## 6. モードの状態と寿命

### 6.1 入口条件 (1 つの関数)

`depth_parallax::entry_allowed(...) -> Result<(), DepthEntryBlock>` を 1 つだけ作り、キー・上部ボタン・
パネルの表示判定が同じ答えを使う (360° の `panorama::entry_allowed` と同じ考え方)。弾く条件:

- 画像の閲覧中でない、動画、アニメーション画像
- 見開きの 2 ページ表示中、連結読み
- ほかの context が 3D を持っている (§6.4.3)。`other_viewer_context_ids` と `with_viewer_context_ref` で読み取り、`OtherWindow` で拒否する。presentation と独立性の許可集合は持たない
- wgpu の描画状態が無い
- 編集系のモード (消しゴム・隠蔽加工・補正レイヤー・テキスト注釈・切り取り・SNS 分割) の最中

モデル未導入は入口条件ではなく、通過したあとに案内ダイアログを出す分岐 (§3.3)。
弾いたときは理由をトーストで出す (既存の `FsNavNoOpReason` の流儀)。
360° が ON のときに 3D へ入るのは弾かず、§6.2 の 1 つの持ち主が 360° を終わらせてから 3D にする。

**同じ条件を「続けてよいか」にも使う**: 入口条件のうちページと表示の状態に関わるもの
(静止画・1 ページ・連結読みでない・ほかが 3D を持っていない) は、3D 中も毎フレーム同じ関数で照合する (§6.3)。

### 6.2 状態 — 表示を占有するモードの持ち主を 1 つにする

360° の `panorama_state` と 3D の状態を別々の `Option` で持つと、両方が `Some` になり得る。
現行の 360° には V キーと `panorama_intent` の自動復帰という 2 つの入口があり、それぞれ 360° の
state を作る (ui_fullscreen.rs:36651、app.rs:94965)。さらに 360° は「state を持ったまま非アクティブ」
(360° でないページ) を許すので、「3D の入口で 360° が ON か」を見るだけでは、後で 360° のページへ移った
ときに 2 つが同時に有効になる。

```rust
// ViewerContextBundle の swap 対象 (viewer ごと。App 全体に置くと別 context に漏れる)
// 既存の panorama_state を置き換える
pub(crate) fs_exclusive_view: FsExclusiveView,

pub enum FsExclusiveView {
    None,
    Panorama(PanoramaState),   // 今の panorama_state の中身 (非アクティブを含む)
    Depth(DepthViewState),
}
```

- 遷移は **1 つの持ち主** `set_fs_exclusive_view(target, cause)` だけが行う。360° の V キー・
  上部ボタン・intent の自動復帰、3D の D キー・上部ボタン・継続条件の OFF (§6.3)、フルスクリーン終了の
  すべてがここを通る。別の値へ移るときは、前の値の OFF の後始末 (360° なら `clear_pano_upload` 等、
  3D なら §5.1 の slot 除去と要求の取り消し) を先に行う。
- 360° の既存コードは `panorama_state()` / `panorama_state_mut()` の accessor で読む形にし、
  中身の扱いは変えない。
- **3D に入るときは 360° の state を捨て、360° の intent (`panorama_intent.on`) も落とす**。利用者が
  明示的に 3D を選んだので、3D を抜けたあと 360° のページで自動的に 360° に戻らないようにする。
  360° の自動復帰は `panorama::entry_allowed` を通るので、3D 中は復帰しない (3D 中であることを
  入口条件に入れる)。

```rust
pub struct DepthViewState {
    pub panel_visible: bool,
    pub started_at: Instant,                 // 自動の動きの時刻原点
    pub mouse: MouseFollow,                  // 目標と現在の offset
    pub source: DepthSourcePhase,            // 下
    pub lru: DepthLru,                       // §4.4
}

pub enum DepthSourcePhase {
    WaitingForStableTexture,                 // 2D 表示
    ReadingBack { key, readback },           // GPU → CPU (§4.2)
    Estimating { key, pending: DepthPending }, // worker (Drop で取り消し)
    Ready { key },                           // 3D 描画
    Failed { key, reason },                  // 2D 表示 + パネルに失敗を表示
}
```

- 相互に排他な段階を bool / Option の組で持たず、1 つの enum にする (CLAUDE.md「バグ修正の一般原則」)。
- パネルの設定 (動き・強さ・周期・焦点・整形) は `Settings` に置く。**画像ごと・context ごとには持たない**。
  `Settings` への項目追加は既存の serde 既定値で読めるので移行は要らない (機能は未リリース)。
- 3D の ON/OFF を閲覧終了や presentation の実切替の後へ持ち越さない。360° の `panorama_intent` に当たるものは作らない
  (§6.3 の「閉じたら終わり」で済ませる)。

### 6.3 遷移

| きっかけ | 動作 |
| --- | --- |
| D キー / 上部の 3D ボタン | 入口条件を通れば ON。ON なら OFF |
| 上部バーの × | 3D を OFF (閲覧表示は閉じない。360° と同じ) |
| Esc / 一覧へ戻る / フォルダ移動 | 既存の閲覧終了で 3D も終わる |
| MainWindow ⇔ Fullscreen (F11)、MainWindow / Fullscreen ⇔ DetachedWindow (F12)、session の独立性の実切替 | 共通の `commit_viewer_session` で 3D を終了し、実効キー (未割当なら上部ボタン) で再入場できるとトーストで知らせる。同じ presentation かつ同じ独立性の再確定では継続 |
| 別ウィンドウ内の F11 (枠なし全画面)、リサイズ・DPI 変更 | presentation・context・viewport は同じなので 3D と実行中要求を継続。現在の pass の矩形・倍率で描く |
| 静止画の次 / 前ページへ移動 | ON のまま。新しいページの表示が安定したら深度を作る |
| **続ける条件を満たさなくなった** (動画・アニメーション画像のページへ移った、同じページでアニメーションと後から分かった、見開き・連結読みに切り替わった) | **3D を OFF にする** (§6.2 の持ち主を通る)。トーストで知らせる |
| 3D パネルの「書き出し…」 | モーダルの書き出しダイアログ (§7) |

- 「ON のまま続ける条件を満たさない」状態を作らない。360° は「state を保ったまま非アクティブ」
  にしていて、戻ったときに入場時の後始末を通らない既知の穴がある (360° 設計書 §5.1.1、backlog §1.169)。
  3D は満たさなくなった時点で OFF にし、この組み合わせを最初から無くす。
- **照合は 1 つの関数、呼ぶのは 3D を描く直前**: 各ページ移動の関数 (open_fullscreen の各種、
  スライドショー再開、フォルダをまたぐ移動、動画タイル、ページ一覧) や見開き・連結読みの切替に
  それぞれ OFF を足さない。継続の照合は `depth_view_may_continue(fs_idx)` (§6.1 と同じ条件) 1 つにし、
  `FsExclusiveView::Depth` を持つ viewer の pass で **2 か所**から呼ぶ。満たさなければ §6.2 の持ち主で OFF にする。
  1. **viewer の pass の先頭**: 動画・見開き・連結表示などの媒体別の early return と描画分岐より前
     (ui_fullscreen.rs:27309、28482 の分岐より前)。3D の描画分岐へ届かない frame (動画へ移った、
     見開きになった) でも `Depth` が残らないようにする。残すと静止画へ戻ったときに入場の後始末なしで
     3D が復活する (独立レビュー第 3 回 P1)。
  2. **3D の描画分岐に入る直前**: 1 のあとにその pass の中で条件が変わった場合。満たさなければ
     その frame は 2D で描く。
  - どちらも、深度の状態の借用と renderer の lock を取る前に行う。GPU の `CallbackTrait::paint` の中では
    照合も遷移もしない。
  - root の frame の固定位置 (たとえば 360° の intent の照合の位置) に 1 回置くだけでは足りない。
    フルスクリーンの準備は root の入力より前に走り (app.rs:99412)、子 viewport の中でも結果の採用と
    キー処理がある (ui_fullscreen.rs:34209、28206) ので、固定位置のあとで条件が変わって同じ frame に
    3D を描き得る (独立レビュー第 2 回 P1)。描画の直前なら、その frame の入力と採用がすべて済んでいる。
  - これは時間窓や再試行ではなく、描く時点の事実 (今のページ・表示の状態) での状態の照合である。
  - open_fullscreen の途中では照合しない (360° と同じ理由。呼び出し元がその後でモードを戻すことがある)。
    描画の直前に呼ぶので、open_fullscreen の途中に当たることは無い。
- ON にするときは補正パネル・補正レイヤー・切り取り・SNS 分割・ルーペ・分析・消しゴム・比較・
  メタデータパネルを閉じる。3Dへの入場ではスライドショーを継続し、360°への入場では従来どおり停止する。
  3D終了時はcontext所有の次ページ枠もDropし、その要求を取り消す ([§1.771](depth-slideshow-plan.md))。

### 6.4 別ウィンドウ (detached viewer) と 3D モード

- **main・独立 context のどの画像閲覧表示でも使える** (§6.4.3)。複数ウィンドウモードで main 自身が独立 session になっている窓も含む。
  GPU slot・worker・ダイアログの組合せは、入口で「プロセス全体で 1 context」に制限して減らす。
- 状態は `ViewerContextBundle` に置き、scoped mount では維持する。ほかの context の 3D は読み取りだけで照合し、
  新しい入場のために兄弟の状態を終えたり資源を消したりしない。
- **「メイン」の定義**は context の識別子。main 所有と環境設定由来のダイアログの見え方・受付規則は維持する。
  presentation・session の独立性は入場の許可条件ではなく、実 session 切替の終了判定にだけ使う。
- **presentation の切替を確定する所で 3D を終える**: 通常の F12 は
  `apply_egui_viewer_presentation_for_current_item` (app.rs:91819 → 91624) で presentation を変え、
  `reset_detached_pause_foreground_modes` は通らない。F12 のキー処理に OFF を足すのではなく、
  **viewer の session を確定する共通の確定点** (`commit_viewer_session`。presentation だけの入口は `commit_viewer_presentation`) で、実際の表示・独立性の切替があれば §6.2 の持ち主で
  `Depth` を終える。360° は別ウィンドウでも使えるので、ここでは `Depth` だけを終える。
  - 加えて §6.3 の描画直前の照合が、媒体・ページ・排他条件を満たさない状態で 3D を
    描かないことを保証する (2 つは役割が違う: 確定点は状態の持ち主の後始末、照合は描画の安全)。
  - F12 の表示先切替では終了し、切替先で新しく D を押せば入場する。窓内 F11 は確定点を通らず継続する。
- live pass を失う park / fork / close では Depth を終了する。独立した live-media park も snapshot 作成前に
  その context の Depth だけを退役し、360° の意図を変えない。ダイアログの寿命・表示先・独立所有窓の modal 保護は §6.4.3 の規則に従う。
- detached の述語に触れる箇所 (live・dialog 所有・modal の受付・park 時の OFF) は、
  CLAUDE.md「Detached viewer リワーク中のルール」に従い、**ClaudeCode と Codex の双方のレビューで
  症状パッチではない構造的な変更だと合意**し、detached-rework-plan.md §11 に記録する。

#### 6.4.1 MainWindow 対応 (2026-11-08、案1を採択・実装済み)

以下は MainWindow 対応時点の記録。現在の適用範囲は §6.4.3 を正本とする。
当時の対象は **メイン context の `MainWindow` と `Fullscreen`**。`DetachedWindow` と独立 context は
引き続き対象外で、§6.4 の複数 viewer・GPU slot・worker・park/fork の理由を変更しない。
設計担当 ClaudeCode が案1を採択し、独立 Sol / xhigh と症状パッチではない構造変更であることに合意した。
入口と確定点の適格性を共有し、実切替で既存 owner を終える。完了通知の宛先移管や追加 repaint は作らない。
以下の経路表の参照行は調査基点 `58816f030`（関数名を追跡の正本とする）。実装基点は `29bb7f54b`、証拠は§10。

**描画までの経路と依存先**

| 対象 | MainWindow で通る経路／参照 | 全画面への依存・設計上の扱い |
| --- | --- | --- |
| 入口と継続 | `app/depth_view.rs::depth_entry_allowed`、`depth_view_may_continue`、`depth_presentation_eligible` | メイン context かつ MainWindow / Fullscreen の共通許可条件へ拡張済み。単ページ・静止画・編集・wgpu 等の他条件は維持 |
| F11 / F12 の表示確定 | F11: `ui_fullscreen.rs`:36329 → `native_video.rs::toggle_egui_viewer_window_mode_for_input`:5229 → `toggle_still_window_mode`:5170 → `depth_view.rs::commit_viewer_presentation`:171。F12: `app.rs::apply_egui_viewer_presentation_for_current_item`:91808 → 同じ確定点 | `viewer_presentation` と `native_video_in_window_active` が既存経路で更新される。Depth 用の別 viewport や表示切替 owner は作らない |
| 静止画 canvas | `ui_fullscreen.rs::fullscreen_embedded_still_active`:32568、`render_fullscreen_viewport_with_root_input`:27376 / `render_fs_body` 呼出し:30172 | MainWindow は root の CentralPanel、Fullscreen は `show_viewport_immediate(fs_id, ...)`。**同じ render_fs_body** を通る。静止画は native fullscreen video presenter / D3D11 HWND を使わない |
| 表示素材・Depth 描画 | `ui_fullscreen.rs::resolve_fs_processed_payload_and_depth`:13268 → `advance_depth_source`:28777 → `draw_fs_image_with_receipt` → `try_paint_depth`:44604、`depth_view.rs`:252 | 素材選択・fit transform・UV・回転・下地は共通。Depth は表示画素の texture ID を使い、fullscreen の面積を素材 key に含めない。各 pass 先頭:27387 と描画直前:44603 の継続照合は両方残す |
| GPU と callback_resources | `depth_view.rs::enter_depth_parallax`:234 / `try_paint_depth`:323、`depth_parallax.rs::attach_renderer` / `publish` / `Drop` | `App.wgpu_render_state.renderer` が所有し、OS 窓ごとの renderer ではない。slot は context と resource ID で照合。サイズ変更だけなら色・深度資源は共用可能。別 context への譲渡はしない |
| worker の wake | `depth_view.rs::advance_depth_source`:411、`depth_estimation.rs` の要求 viewport、`depth_parallax.rs::advance`:395 | `ctx.viewport_id()` を要求に捕捉して完了時にその viewport を起こす。MainWindow で新規開始すれば ROOT。継続案では、旧 viewport 宛ての実行中要求を切替後どう起こすかの設計が必要 |
| パネルと入力除外 | `depth_view.rs::draw_depth_panel`:362、`ui_depth_parallax.rs::draw_depth_parallax_panel_with_export`:32、`ui_fullscreen.rs`:28356 / 39988 | 現在の `ctx.content_rect()` 内に右パネルを配置し、`depth_panel_rect` を同じ ctx の一時データへ記録。MainWindow も同じ関数。移行時は前 viewport の rect を移植せず、新しい pass で描いてから canvas 入力を判定 |
| 待機表示 | `ui_fullscreen.rs::draw_depth_wait_indicator`、`app/depth_view.rs::depth_wait_status` | 現在の page transform / content rect から領域を求め、上部160pt・下部48pt・表示中パネルを避ける。専用 viewport ID 固定ではない。小窓の実描画でパネルとの非交差・取消表示を検証 |
| キー・リング・pointer | `ui_fullscreen.rs::handle_fullscreen_root_key_input`:33766、`handle_fs_key_input_with_operation`、`gamepad_input.rs::apply_ring_action` | embedded では root 側 shortcut probe は処理を委譲し、viewer 本体が同じ ROOT のキーを処理する。Grid 用入力へ落とさない。Viewer の surface / owner gate を維持し、F11・D・I/Tab・Shift+B・ページ送りを各1回だけ受理 |
| マウス追従 | `depth_view.rs::try_paint_depth`:284、`depth_parallax.rs::MouseFollow::update`:116 | 現在 ctx の `hover_pos` と `transform.paint_rect` は同じ論理座標。OS screen 座標や専用 fullscreen rect に変換しない。パネル上で凍結、窓外では中央へ戻す。リサイズ後は新しい fit rect を使う |
| カーソル自動非表示 | `ui_fullscreen.rs::fs_ui_is_clean`:51284、`update_fullscreen_cursor_visibility`:33615、呼出し:30079 | 現行 MainWindow 閲覧も同じ仕組みで現在の `ctx.viewport_id()` を対象にする。modalはcleanを偽にするが、`DepthViewState.panel_visible` はこの関数で直接参照していない。パネル表示だけで非表示を止めるとは断定せず、実効挙動を検証する。今回の案では独自の非表示規則を加えず、既存入力境界を維持 |
| 書き出しの表示と前面 | `viewer_chrome.rs::draw_viewer_shared_dialogs`:396 → `depth_export.rs::draw_depth_export_dialog`:793、`ui_fullscreen.rs`:29868、`ui_depth_export.rs::draw`:66 | canvas 描画後に capture・open・dialog 描画までを同じ viewer ctx で実行。MainWindow も ROOT 内でここを通る。root tail は embedded 時 early return (`app.rs`:99967) するため、tail に追加してはいけない。backdrop と Foreground Window の `set_sublayer` / `move_to_top` を維持し、パネルより前面にする |
| modal の止め方 | `app.rs::common_modal_dialog_open`:23988 / `modal_dialog_block_reason` の depth_export、`document_open_modal_admission_blocked`:42338、`depth_export.rs::admit_depth_export`:670 | Settings〜Finished の operation がある間は共通 modal。キー・wheel・canvas・リングの既存 modal gate を使う。Remote の local modal lease は窓によらず取得〜dismiss まで保持。関連付け・D&D・単一起動等の開く要求も受付停止。外クリックは backdrop が所有し、背面へ渡さない |

**表示切替の組合せを先に減らす**

| 案 | 資源・状態と利点 | 失われる使い勝手／費用 |
| --- | --- | --- |
| 1: MainWindow ⇔ Fullscreen の実際の切替で Depth を終了し、D で再入場 | 共通確定点で既存 setter を通して終了。推論取消・slot 破棄・旧 viewport の wake の移管を扱わずに済む。終了後は通常2Dの既存 F11 経路だけになる | 切替後に D がもう1回必要。**深度 LRU は DepthViewState 所有で Drop される** (`depth_parallax.rs`:283 / 294 / 488)。新しい D は `new` で空 LRU、readback と推論をやり直す。表示側の色テクスチャが残っていても「キャッシュで即復帰」とは言えない。動きの時刻・mouse offset・panel の開閉も新規状態になる（Settings の値は残る） |
| 2: 同じ Depth を継続 | context が同じで renderer も共通なので、Ready の色・深度・LRU、Settings、panel_visible、動きの時刻を保持できる。fit / UV / aspect は次 pass の transform から再計算。切替のための再推論と D の手間が無い | ReadingBack / Estimating での切替も普通に起きる。worker 完了の repaint 宛先が旧 viewport に固定されており、現在は移管が無い。パネル rect・pointer・表示 snapshot は新 viewport で描き直す必要がある。Ready だけを扱って継続できたとする実装にはできない |

**採択したのは案1**。表示切替中の pending 推論の宛先移管を新設せず、既存の終了経路で組合せを減らせる。
現行 Fullscreen → MainWindow も Depth を終了するので、その方向の挙動を悪化させない。
逆方向にも同じ規則を適用する。ただし新たに使える MainWindow から全画面へ広げる際にも D と再推論が要る。
設計担当がこの再操作・再推論の負担を了承した（2026-11-08）。案2は不採択。
同じ presentation への再確定は切替でなく、Depth を終了しない。
終了の案内は `FsDepthParallax` の実効キー割り当て（既定 D）を既存helperで表示する。
割り当てなしの場合は上部の3Dボタンを案内し、解除したDキーを案内しない。

**MainWindow 特有の組合せ（採択仕様）**

| 操作・状態 | 扱い |
| --- | --- |
| サイドバー・グリッド | 同時表示を新設しない。現行 embedded 閲覧は main の menubar / toolbar / grid を描かず early return (`app.rs`:99954)。Depth を許すために一覧や通常全画面の機能を削る必要はない。閲覧内の補正・metadata・navigator等は§6.5どおり、I/Tabは3Dパネルのみ |
| リサイズ・最大化 | そのまま。色・深度の key はサイズ非依存。新しい content rect / fit transform / aspect を毎描画使用。PDF の既存再レンダリング等で**素材**が変わった場合のみ§4.1で新規推論 |
| 最小化・非表示 | モードはそのまま、既存 `advance_depth_source` の非表示／minimized gate で poll・採用を休止。自動モーションの追加 repaint も抑止。復帰の通常 pass で再開。推論結果保持の新しい中間状態は作らない |
| 別アプリへ切替 | 終了させない。非focusは最小化とは違い、現状は自動モーションが継続し得る。既存 focus / keyboard permit で他アプリの入力を受けないことを確認し、今回の拡張で休止規則は変更しない |
| Esc・Enter・Backspace・一覧へ戻る | 既存の viewer close / return-to-list を通して Depth 終了 (`app.rs::close_fullscreen`:81688付近)。単に panel を隠して state を残さない。フォルダ移動も既存の終了・継続規則を維持 |
| ページ一覧（O）、見開き・連結、編集、360° | ページ一覧は `page_overview_available` の占有中入場禁止を維持 (`app/page_overview.rs`:230付近)。Depth中はOを止める。既にページ一覧ならその実行可能キー集合にDは含めない。見開き・連結等へ状態が変わった場合は共通継続照合で終了。3Dから360°への切替を新しく許さない |
| 書き出し Settings / Running / Finished | modal のまま。F11/F12・D・ページ移動・Shift+B・panel操作を止め、開く要求とRemote取得も拒否する。ROOT上のbackdrop＋dialogでpanel/canvasより前面を保証。native保存先選択もそのmodalの内側。外クリックでcanvasやpanelが動くならNG（依頼19 NG1と同じ受入条件） |

**detached 述語に触れる範囲と構造合意**

- 入口 `DepthEntryInput.main_viewer_presentation` と `commit_viewer_presentation` の終了規則を、
  同じ「メイン context かつ MainWindow / Fullscreen」の適格性へ接続する。許可集合を明記するので
  `DetachedWindow` は引き続き除外される。context 比較を落としたり `!= DetachedWindow` だけにしない。
  `App::depth_presentation_eligible` へ接続した。適格性に加え「実切替で終了」の規則を確定点に置く。
- `fullscreen_embedded_still_active`、`render_fs_body` の root/child routing、
  `handle_fullscreen_root_key_input`、`apply_egui_viewer_presentation_for_current_item` は読み取り監査対象。
  それらの変更が必要になれば detached viewport 経路に触れる変更として扱う。
  `reset_detached_pause_foreground_modes`、fork/park、bundle swap、window_id/HWND/placement は今回拡張しない。
- CLAUDE.md「Detached viewer リワーク中のルール」と detached-rework-plan §2 の対象。
  実装着手前に ClaudeCode と独立 Sol / xhigh レビューで、適格な単一contextの表示許可を広げる構造変更であり、
  detached の症状を guard 等で隠す変更ではないと合意済み（2026-11-08）。
- detached-rework-plan §11 へ記録: 日付・基点・合意者、変更した共通適格性と確定点、
  MainWindow/Fullscreen許可・DetachedWindow/独立context拒否、案1の切替時終了、fork/park不変、
  ROOT/child双方の照合・slot破棄・モーダルテストの証拠。実装と併せて§11へ記録する。

**受入テスト計画（実行結果は§10）**

1. 採択した案1の入口・確定点の共通適格性を一塊で検査する。
   状態遷移テストは main × MainWindow/Fullscreen/DetachedWindow、独立contextの全表示、
   同じpresentationへの再確定、F11両方向、F12両方向、close/fork/parkを含める。
   pending取消・slot退役・D再入場で空LRUを確認。ReadingBack / Estimating 中にF11で切替し、
   D再入場後に旧結果が届いても新ownerとslotが保たれることを確認する。
2. ROOT embedded の実viewer headless passで、入口直後と描画直前の照合、同pass内の動画・見開き等への変更、
   不一致素材のcallback不送出、fit固定・下地・回転・リサイズ・panel rectを検査。
   既存Fullscreenの入力・ページ送り・2D transform保存・推論/描画テストを再利用する。
3. ROOTの本物のパネル/canvasを背面に、書き出し全phase・初回/後続pass・再open・形式変更を検査。
   modalの外クリック・wheel・キー・リングで背面が変化せず、dialog内の操作だけが届くこと、
   lease/開く要求拒否、UI snapshotと§7の凍結素材を検査。tailへ依存した二重描画を許さない。
4. 1093×614 logical points と通常サイズ、UI倍率100%/200%で、右panelの×・書き出しボタン、
   dialogの見出し・全固定操作行が収まり、必要時だけ本文scrollで末尾へ届くことをgeometryとsnapshotで確認。
   リサイズ・最小化復帰・Alt+Tab・OS保存先dialogのowner/focusは、実装後の承認済みportable実機枠で確認する。
   今回は製品起動・実機操作を行わない。

まれな失敗は既定の割り切り（ログ＋通知）で閉じ、失敗別の回復設計は加えない。
公開文書は段Dでまとめて更新する。現時点の `htdocs/mimageviewer/manual/` のgrepでは「3D 写真」の節自体が
未追加だった。更新予定は `manual/fullscreen.html` の3D写真節（使用できる表示、F11、F12対象外、Dで再入場の有無、
パネル・背景・書き出し）、`manual/shortcuts.html`（DとF11）、`manual/faq.html`（対応範囲）、
`index.html` の3D紹介・対応表示説明。既存Creative 3D LUTの説明は別機能なので対象にしない。
段Dで公開説明を更新する。案1の再操作・再推論は採択済みであり、wake所有の移管は対象外。

#### 6.4.2 F12 の別ウィンドウ対応 (§1.767、2026-11-11)

以下は第 1 段の設計・実装の記録。現在は §6.4.3 で独立した窓も対象にしている。
本節の決定 B・C・E は維持し、main 限定の適格性と決定 D は §6.4.3 の所有・表示先の規則へ一般化した。

設計: ClaudeCode Opus 5.5。worktree `G:\home\mimageviewer-depth2`、ブランチ `feat-766-767-depth-ui`。
利用者決定 (2026-11-11): 360° は別ウィンドウで使えるので、3D も F12 の別ウィンドウで使えるようにする。
行番号は基点 `18d528908` の調査時点 (関数名を追跡の正本とする)。

**範囲を 2 段に分ける (設計担当の判断)**

- **第 1 段 (実装済み)**: フル機能ウィンドウモードで F12 を押し、**main context をそのまま別ウィンドウに出している表示**
  (linked F12。`ViewerPresentation::DetachedWindow` かつ独立 session でない) とその枠なし全画面。バックログ §1.767 の
  「まず F12 で開く閲覧ウィンドウの通常表示と枠なし全画面が対象」に当たる。
- **第 2 段 (§6.4.3 で利用者採択)**: 複数ウィンドウモードの各窓、フル機能モードのメディアウィンドウなど、
  **main 以外の独立 context** での 3D。下の課題を単一所有と独立所有窓だけの modal 保護へ整理した。

**調査で分かったこと (コード確認)**

1. **描画経路は既に共通**。別ウィンドウ (main context の linked F12 も、独立 context も) は
   `render_fullscreen_viewport_with_root_input` から `show_viewport_immediate(fs_id, …, render_fs_body)` (ui_fullscreen.rs:30305) を通り、
   `render_fs_body` の中で `draw_depth_panel`・`advance_depth_source`・`try_paint_depth`・待機表示・`draw_viewer_shared_dialogs` を呼ぶ。
   ctx は子 viewport の ctx。キー (D・I/Tab・F11・F12) は子 viewport の pass が処理し、D・I/Tab に detached 専用の抑止は無い。
   上部の 3D ボタンも描かれる。カーソル自動非表示はその viewport id を対象にする。推論完了の wake は `ctx.viewport_id()` を要求に捕捉し
   (depth_view.rs:511)、eframe は immediate 子 viewport の repaint を親へ回す。
   **今 3D を止めているのは `depth_presentation_eligible` (depth_view.rs:168) だけ**。
2. **統括の前提「動く閲覧 context は 1 つ」は main の linked F12 では成り立つ** (context は main 1 つ)。ただし、プロセス全体では
   アクティブな独立 detached context の pass の後、同じ frame に main の埋め込み表示も描かれる
   (`should_render_main_fullscreen_viewport_after_detached_context`、app.rs:4635)。組み合わせ (コード調査):
   - 複数ウィンドウモード: 静止画は常に別ウィンドウで開き (`requested_viewer_presentation_for_open`、app.rs:70409)、main が静止画を
     MainWindow / Fullscreen で表示することは無い。main の埋め込みはメディア (音楽表示など) だけ。
   - フル機能モード + メディアウィンドウ: メディアウィンドウ (独立 context) から次へ進むと静止画を表示でき、main の埋め込み静止画と同時に
     通常の body (`render_fs_body`) が 2 つ走り得る。
   - linked F12 + メディアウィンドウ: アクティブな独立 context を描いた frame では main は埋め込みのときだけ描かれるので
     (app.rs:4635、テスト `main_embedded_fullscreen_still_renders_while_detached_context_is_active`)、linked F12 と通常の body が
     2 つ並ぶことは無い。ParkedLive のメディアウィンドウは専用の描画経路 (ui_fullscreen.rs:24763) で、通常の body ではない。
   第 1 段では main 以外は 3D に入れないので、3D を持つ context は常に main 1 つ。深度 worker (最新 1 件の待ち、depth_estimation.rs:235) と
   GPU の callback slot (1 つ、depth_parallax_wgpu.rs:737) の持ち主も main 1 つのまま。
3. **3D のダイアログは live な viewer pass すべてで描かれる** (既存の穴)。書き出し画面 (`draw_viewer_shared_dialogs` → `draw_depth_export_dialog`)
   と追加モデルの案内 (ui_fullscreen.rs:30222 の `show_depth_addon_dialog`) は context を見ないので、上の 2 live pass の frame では
   main 以外の窓 (メディアウィンドウ) にも出る。さらに追加モデルの案内は ROOT (app.rs:100957、`!main_viewer_blocked`) でも描く。
   linked F12 中は main が塞がっていない扱い (`viewer_session_blocks_main_window` は detached では偽、app.rs:63778) なので、
   F12 の窓から D を押してモデル未導入だと、メインウィンドウと F12 の窓の両方に出る。書き出しの ROOT 側
   (`main_window_should_draw_export_dialogs`) は detached 中は偽なので重ならない。
4. **別ウィンドウ内の F11 は presentation を変えない**。`toggle_egui_viewer_window_mode_for_input` は detached では
   `toggle_detached_viewer_borderless_fullscreen` (app.rs:70884) に分岐し、同じ viewport の枠なし全画面を切り替えるだけ
   (`commit_viewer_presentation` を通らない)。viewport id は window_id から決まり世代に依らない (`detached_image_window_viewport_id`)。
   egui が OS の窓を作り直しても同じ id、renderer はプロセス共通 (設計レビュー r1 で確認)。切替中の数 frame は
   `paint_viewer_presentation_interlude` が描画前に返る (ui_fullscreen.rs:28593)。
5. keep-alive の backstop (`render_active_detached_viewport_backstop`、ui_fullscreen.rs:27038) は、通常の描画が走らなかった frame ごとに
   保持画像 (`FsPageLayoutSource::Captured`) で描き、3D は描かない。
6. egui の `ctx.data()` の一時データ (`depth_panel_rect`、`depth_export_display_snapshot`) は viewport をまたいで共有だが、第 1 段では
   書くのは main の 3D の pass だけで、読む側は mount 中の context が 3D のときだけ読む。

**決定 (第 1 段)**

A. **適格性を「main context の閲覧表示 (linked F12 を含む)」にする**。`depth_presentation_eligible` を
   `projected == main` かつ (`MainWindow` | `Fullscreen` | (`DetachedWindow` かつ main の現在の session が独立でない)) に広げる。
   独立かどうかは既存の事実 (`detached_viewer_independent_active`: 「現在の detached 静止画 session がメイン一覧と独立しているか」) で判定する。
   複数ウィンドウモードで main 自身が独立の静止画窓になっている場合は、他の独立窓と同じく対象外 (窓によって使える・使えないが分かれないように)。
   入口 (`depth_entry_allowed`) と確定点 (`commit_viewer_presentation_deferred_feedback`) は今までどおりこの 1 つを使う。
   `detached_viewer_independent_active` は `ViewerSession.independent_active` の mounted projection で、presentation と一緒に swap される
   (context ごとの事実。App 全体のフラグではない)。
   - **確定点は「確定先の session」で判定する** (設計レビュー r4 P1)。今の確定点は presentation だけを受け取り、独立フラグは呼出し元が
     確定の**後**に書く (`activate_mounted_as_independent_detached`、`prepare_viewer_presentation_open`)。linked な DetachedWindow から
     独立な DetachedWindow へ移るとき、presentation は同じなので 3D が終わらず、独立 session に 3D が残る。
     確定点の入力を (presentation, 独立か) の組にした `commit_viewer_session(target, independent)` を共通の確定点とし、
     既存の `commit_viewer_presentation(target)` は「独立かどうかは今のまま」の薄い入口として残す。独立フラグを変える呼出し元は
     新しい値を渡して確定し、その後でフラグを書く (書く順序と値の持ち主は変えない)。3D の継続は「同じ presentation かつ同じ独立かどうか
     の再確定」だけ。独立フラグを書く箇所をすべて列挙し、確定点を通るか、3D と両立し得ない (main 以外・媒体だけ等) ことをコードで示す。
     新しい field・待ちは作らない。
   - 描画直前と pass 先頭の継続の照合 (§6.3) も同じ述語を読むので、仮に確定点を通らない書き換えがあっても、次の pass で 3D は終わる
     (2 つは役割が違う: 確定点は状態の持ち主の後始末、照合は描画の安全)。
   - switching (切替途中) の除外は A の述語の役目ではない。静止画の linked F12 の切替は同期で確定し、native の切替は既存の媒体条件で扱われる。
   - 拒否の文言 (`DepthEntryBlock::Detached`) を「3D 写真は、メインの画像表示と F12 の別ウィンドウで使えます」に直す。
     確定点の不適格な切替先の通知も同じ趣旨に直す。
B. **表示先の移し替えは §6.4.1 の案 1 のまま**: F12 の ON / OFF は presentation の実切替なので `commit_viewer_presentation` で 3D を終え、
   通知で D (実効キー) の再入場を案内する (MainWindow / Fullscreen ⇔ DetachedWindow のどちら向きも)。同じ presentation の再確定は継続。
C. **F12 の窓の中の F11 (枠なし全画面の切替) では 3D を続ける**。viewport・ctx・context が変わらず、推論の wake 先・GPU 資源・パネルの持ち主が
   移らないので、§6.4.1 のリサイズ・最大化と同じ扱い (毎 pass の content rect / fit transform から描き直す)。案 1 がメインの F11 で終えるのは
   viewport が ROOT ⇔ 全画面の子で入れ替わるからで、その理由がここには無い。interlude の数 frame は深度の前進・3D の描画が止まるだけで、
   状態は変わらない。メインの F11 (終了) と挙動が違うことはマニュアルに書く。モニタをまたぐ移動 (DPI の変化) も続ける。
D. **3D のダイアログは main context の表示だけで描く**。3D の持ち主は常に main なので、ダイアログの持ち主も main と決める。
   - viewer pass の 2 か所 (`draw_viewer_shared_dialogs` の `draw_depth_export_dialog`、ui_fullscreen の `show_depth_addon_dialog`) は、
     mount 中の context が main のときだけ描く (メディアウィンドウ等の pass では描かない)。
   - ROOT の `show_depth_addon_dialog` は、既存の `!main_viewer_blocked` に加えて **main に開いている閲覧表示が無い** (`fullscreen_idx` が None) ときだけ描く。
     main の閲覧表示が開いていれば (MainWindow / Fullscreen / linked F12) その pass が描く。今の main の Fullscreen での見え方
     (ROOT は描かず全画面の pass が描く) と同じ規則を linked F12 に広げるもの。ROOT の書き出し (`main_window_should_draw_export_dialogs`) は変えない。
   - 所有の終わり方は今の main と同じ: F12 の窓を閉じる・一覧へ戻ると main の閲覧表示が閉じ、書き出しは既存の取消 (`cancel_depth_export_for_fullscreen_close`)
     → ROOT、追加モデルの案内も ROOT に出る。新しい gate・ハンドオーバー・frame フラグは作らない。
   - main の埋め込み表示と独立のメディアウィンドウが同時に動く既存の組み合わせでも、メディアウィンドウに 3D のダイアログが出なくなる (今の穴を閉じる)。
E. **backstop の frame は 2D のまま**。描画経路を 2 つにしないため。通常の pass が走らない frame の間は平面に見え得ることを実機確認の項目に入れる。

**採らなかった案**: F12 で別ウィンドウへ 3D を持ち越す (案 2。§6.4.1 と同じ理由)。独立 context も今回まとめて対象にする (下の第 2 段)。

**状態を減らす検討**: 3D の持ち主を main 1 つに保つことで、資源の取り合い・ダイアログの持ち主・所有 viewer の終了後の表示先は、
すべて今の main の挙動の範囲に収まる。新しい状態・gate・待ちは足さない。

**第 2 段へ整理した課題 (設計レビュー r1〜r3 の調査記録。採択仕様は §6.4.3)**

独立 context を対象にすると、3D とダイアログの持ち主が main 以外になる。r1〜r3 のレビューで次が必要と分かった:
- プロセス全体で 3D は 1 context (main の埋め込みと独立 context が同じ frame に live pass を持つため。入口で他の bundle を読んで拒否)。
- live でなくなる経路の終了漏れ (`park_active_detached_context_as_live_media` は 3D を終えない)。
- ダイアログの表示先を、所有 viewer が生きていればそこ、終わればメインの表示、と事実から決める関数。ただし表示先が通常の body を描かない経路
  (main の列挙待ち holdover・native 動画など) に当たると modal が見えなくなるため、modal の間は所有窓が終わらない保証が要る:
  窓の切替・外部からの開く要求を追加モデルの確認中も拒否する (**今は受け付けている操作への新しい制限**)、所有窓の OS の閉じる操作を拒否する、
  modal 中の自動送り (スライドショー・フォルダをまたぐ送り) と受付済みの遷移の扱い。
- 独立 context から D を押したときの、モデル確認の完了後の入場の再開 (ROOT の poll から要求元 context へ配送)。
これらの新しい制限は、利用者が第 2 段を進めると決めた後、§6.4.3 の範囲で採択した。

**detached 述語・viewport 経路に触れる範囲 (第 1 段)** (CLAUDE.md「Detached viewer リワーク中のルール」)

- 変える: 3D の適格性 (`depth_presentation_eligible`) の許可集合に「main context の linked な DetachedWindow」を加えること (既存の事実
  `detached_viewer_independent_active` を読むだけ)、3D のダイアログの描く場所 (viewer pass 2 か所と ROOT の追加モデル案内 1 か所)、文言。
- 変えない: viewport の生成・id・builder、HWND の同定、placement、focus、z-order、bundle の swap、fork / park の手順、
  `reset_detached_pause_foreground_modes`、`render_fs_body` の root / child の振り分け、backstop、F11 / F12 の経路、
  App の detached 用 bool / Option (新設しない)。時間窓・再試行・追加 repaint は足さない。
- 設計担当と独立 Sol / xhigh が「症状パッチではなく構造的な変更」であることに合意したうえで実装し、detached-rework-plan §11 に記録する。

**テスト** (状態遷移・handler・複数 viewport の headless 駆動。既存の `src/app/depth_view_tests.rs`、
`src/app/depth_main_window_tests.rs`、`src/app/multiwindow_scenario_tests.rs` の fixture を使う)

1. 入口の matrix: main × {MainWindow, Fullscreen, linked DetachedWindow} で許可、main × 独立 DetachedWindow (複数ウィンドウモード) と
   独立 context × 全表示で拒否。上部の 3D ボタンの有効表示も同じ答え。拒否の文言。
2. 確定点: linked DetachedWindow で 3D 中に独立化 (`activate_mounted_as_independent_detached`、`prepare_viewer_presentation_open` の独立 open) →
   3D が確定点で終わる (次の pass を待たない)。独立フラグを書く全箇所の列挙を実装記録に残す。
   F12 の ON / OFF (Fullscreen ⇔ linked DetachedWindow、MainWindow ⇔ linked DetachedWindow) の両方向で終了し、再入場の通知
   (実効キー、割り当てなしは上部ボタン)。同じ presentation の再確定で継続。F12 の窓の中の F11 の両方向で、`FsExclusiveView::Depth`・
   source の段階・LRU・GPU slot・実行中の要求が保たれ、次の pass は新しい content rect で描く。ReadingBack / Estimating 中の F11 でも
   要求が生きて採用される。
3. F12 の窓の実 pass (子 viewport の headless 駆動): D で入場、推論要求の viewport が子 viewport id、パネルの矩形がその pass で書かれ
   入力除外 (マウス・タッチ・Ctrl+クリック) に使われる、I / Tab、書き出しの全 phase の modal と描画、待機表示、カーソル自動非表示の対象 viewport、
   `pixels_per_point` を途中で変えた pass (DPI の異なる画面への移動) で transform とクリックの写像が新しい値に従う。§1.766 の折りたたみも F12 の窓で働く。
4. ダイアログの描く場所: linked F12 で D (モデル未導入) → 追加モデルの案内が F12 の窓の pass だけに出て ROOT では描かれない。
   main の Fullscreen / MainWindow / 一覧 (環境設定から) は今までどおり。main 埋め込み + アクティブな独立 context (静止画・動画) の
   2 つの通常 body の frame で、独立側の pass は 3D の書き出し画面・追加モデルの案内を描かない。linked F12 + ParkedLive のメディアウィンドウの
   併存でも 3D のダイアログは F12 の窓だけ。F12 の窓を閉じたときの書き出しの取消 → ROOT の Stopping / Finished、
   追加モデルの案内 → ROOT。
5. 寿命: F12 の窓を閉じる (推論中を含む) で 3D が終わり要求の取消・slot 除去。メディアウィンドウとの切替など既存の park / fork で終わる。
6. 既存の depth・書き出し・表示切替・360° のテストが通る (`test(/depth|export|presentation|window_mode|panorama/)`)。

**文書**: マニュアル `fullscreen.html` の 3D 写真「使える場面・使えない場面」: F12 の別ウィンドウでも使える、F12 で表示先を切り替えると
終了 (D で再開)、F12 の窓の中の F11 では続く、複数ウィンドウモードの窓とメディアウィンドウでは使えない。`index.html` の 3D の説明と
`faq.html` に対応範囲の記述があれば合わせる。`docs/keymap-spec.md` の 3D 写真の節 (今は「メインだけ・別ウィンドウ不可」) を直す。
本書 §1・§6.1・§6.3・§6.4 の「メインだけ」の記述を本節に合わせて直す。まれな失敗は既定の割り切り (§13)。

**設計レビュー (GPT-6.1 Sol / xhigh、read-only、master `62de527fb` = 製品コードは基点と同一)**

- r1: 調査主張 1〜7 は正しい。P1 2 件 (独立 context の所有窓が閉じた後の書き出し・追加モデル案内の表示先が無い)、P2 1 件
  (独立 context からのモデル確認後の入場再開が捨てられる)、P3 1 件 (keymap-spec)。構造的変更に合意。
- r2: 表示先を事実から決める関数へ改訂 → P1 (表示先が通常の body を描かない経路で modal が見えない)・P3 (受入条件)。合意は維持。
- r3: modal 中は所有窓を終わらせない形へ改訂 → P1 (自動送り・受付済み遷移)・P2 (close 拒否・開く要求の拒否は新しい制限で利用者確認が要る)・
  P3 (modal 段階の分類)。合意は維持、判定は「指摘修正後に実装開始可」。
- 設計担当の判断: r1〜r3 の P1 は「独立 context が 3D とダイアログの持ち主になる」ことから生じ、直すたびに別の組み合わせが出ていた
  (CLAUDE.md「状態の組み合わせを減らす」の経緯と同じ形)。持ち主を main 1 つに保つ第 1 段に絞り、独立 context は第 2 段として判断を仰ぐ。
  main の MainWindow / Fullscreen に既にある一般的な modal・自動送りの挙動は、第 1 段では変えない。
- r4 (第 1 段): P1 1 件 (linked → 独立の確定点が確定前の独立フラグで判定する) → 決定 A に「確定先の session で判定」を追加。
  P3 1 件 (linked F12 と独立 context の通常 body は 2 つ並ばない) → 調査 2・テスト 4 を訂正。独立フラグは context ごとの projection、
  ROOT の地点では main が mount されていることを確認。判定「指摘修正後に実装開始可」、構造的変更に合意 (はい)。
  P1 の修正は独立化の既存の所有境界で閉じるもので、追加の guard・待ち・再試行・repaint は不要との見解。

#### 6.4.3 独立した窓での 3D (§1.767 第 2 段、2026-11-11、設計)

設計: ClaudeCode Opus 5.5。基点 `1ff4082a8` (第 1 段は実装・レビュー・全件済み)。
利用者決定 (2026-11-11): 第 2 段も進める (第 1 段の報告で、独立した窓を持ち主にすると窓をまたぐ新しい制限が要ることを示したうえでの判断)。

**対象**: main 以外の独立 context (複数ウィンドウモードの各窓、フル機能モードでメディアウィンドウから静止画へ進んだ窓、
PDF / ZIP の連動静止画を再開した独立窓など) と、複数ウィンドウモードで main 自身が独立の静止画窓になっている場合。
どの presentation でも、その context の live pass (`render_fs_body`) があれば 3D を使える。

**第 1 段からの変更点だけを書く** (第 1 段の決定 B・C・E はそのまま。D は下の 3 で一般化する)。

1. **3D を持てるのはプロセス全体で 1 context** (§6.4.2 調査 2 の「main 埋め込み + アクティブな独立 context の 2 つの通常 body」のため)。
   - 適格性 (`depth_session_eligible`) から「main context」と「独立でない」を外す。presentation の許可集合は 3 種すべてになるので持たない
     (常に真の述語は残さない)。代わりに入口条件へ「**ほかの context が 3D を持っていない**」を足す: `other_viewer_context_ids()` の各 context を
     `with_viewer_context_ref` で読み、`fs_exclusive_view` が Depth なら拒否 (読み取りだけ。兄弟の状態を変えない。mount 中の自分は含まれない)。
   - 拒否は `DepthEntryBlock::OtherWindow` (`Detached` を置き換え):「3D 写真は別のウィンドウで表示中です。そちらを終了してから使ってください」。
     上部の 3D ボタンの有効表示も同じ答え。継続の照合 (`depth_view_may_continue`) も同じ関数だが、自分が持つ間に他が持つことは入口で起きない。
   - 確定点 `commit_viewer_session` は残す: presentation か独立かどうかが実際に変わったら 3D を終える (第 1 段の案 1 と B)。
     確定点の「不適格な切替先」の分岐は到達しなくなるので消す。
   - 採らなかった案: 後から入った側を優先して他 context の 3D を終える (兄弟の状態を書き換える)、2 つ同時に動かす (worker の待ちは最新 1 件、
     GPU slot は 1 つ)、2 つの通常 body がある間だけ禁止 (frame ごとに答えが揺れる)。
2. **live pass を失う遷移はすべて 3D を終える**。欠けている `park_active_detached_context_as_live_media` (アクティブな独立 context を
   ParkedLive へ移す) に、fork と同じ「Depth だけを `set_fs_exclusive_view(None)` で終える」を、snapshot 作成の前 (その context を mount した closure の中)
   に入れる。`reset_detached_pause_foreground_modes` は呼ばない (360° 動画の意図の扱いを変えないため。fork の前例と同じ)。
   ほかの経路 (pause・静止画の park・fork・close / retire・確定点・Remote の取得) は既に終える (設計レビュー r1〜r3 で確認)。scoped mount では終えない。
3. **3D のダイアログを描く場所を 1 つの関数で決める** (第 1 段の D の一般化)。
   ```text
   depth_dialog_owner_surface(owner: Option<ViewerContextId>) -> DepthDialogSurface
     owner が Some(c)、c ≠ main、viewer_context_live(c)  → Viewer(c)
     それ以外                                              → Main   (第 1 段の規則: main の viewer pass、main に閲覧が無ければ ROOT)
   viewer_context_live(c) (c ≠ main) = active_viewer_context_id() == Some(c) かつ detached_active_window_alive_wanted()
   ```
   - owner: 書き出しは `Operation.resource.context`。追加モデルの案内は、`DepthSetup` がダイアログの寿命の間持つ入口
     (`ModeEntry(request)` → `Some(request.context)`、`Preferences` → `None`)。今は Checking の間しか入口を持たないので、ダイアログと一緒に持たせる。
   - 描く側: viewer pass の 2 か所は `Viewer(mount 中の context)` のとき、または `Main` かつ mount 中が main のとき。ROOT は `Main` のときだけ
     (ROOT の 3D 書き出しは既存条件 `main_window_should_draw_export_dialogs` に、追加モデルの案内は第 1 段の条件に、`Main` を加える。
     独立 context が持ち主の間は main が一覧でも ROOT で描かない)。
   - **main が持ち主のときは第 1 段と同じ** (main の今の見え方を変えない)。
4. **modal の間、独立した持ち主の窓は終わらない**。持ち主が独立 context のとき、3 の答えが `Main` に落ちると、main の閲覧が
   body を描かない状態 (列挙待ちの holdover・native 動画など) では modal が見えなくなる (設計レビュー r2 P1)。そこで、
   **持ち主が main 以外の独立 context で、3D の modal (書き出しの全段階、または 3D の入口から開いた追加モデルの案内の Checking / Confirm。
   RemoveConfirm は環境設定からしか開かず持ち主を持たない) が開いている間**だけ、その持ち主を終わらせる操作を受け付けない:
   - キー・ポインタ (Esc・F12・F11・ページ移動・×・D): 既存の共通 modal が止める (変更なし)。
   - 窓の切替 (他の窓の activation) と外部からの開く要求 (関連付け・D&D・単一起動・Everything 等): `document_open_modal_admission_blocked`
     は書き出しを既に含む。**追加モデルの案内を「持ち主が独立 context の modal 段階」の間だけ**この述語に加える
     (`Preferences` 由来や main が持ち主の案内では今までどおり受け付ける)。
   - 持ち主の窓の OS の閉じる操作: `accept_active_detached_window_close` に、**閉じようとする窓の context が独立した 3D modal の持ち主**なら
     拒否を加える (既存の `viewer_mutation_modal_active` と同じ扱い)。持ち主でない窓は今までどおり閉じられる。
   - 自動送り (スライドショー・フォルダをまたぐ送り): 書き出し中は `handle_fs_navigation_with_render_perf` が既に止める。追加モデルの modal 中も、
     持ち主が独立 context なら同じ条件で止める (持ち主でなければ今までどおり)。
   - Remote の取得: 書き出しは既存の lease が止める。追加モデルの modal 中の取得は全 viewer を閉じ、main は一覧になるので答えは `Main` で ROOT が描ける。
   - **受付済みの遷移がある間は modal を始めない** (設計レビュー r5 P1)。独立 context でフォルダ移動などを受け付けた後、旧画像の表示中に
     modal を始めると、移動の完了 (`apply_folder_nav_result`) が旧 viewer を閉じた後の reopen を、上で広げた開く要求の拒否が自分で拒み、
     持ち主の窓が終わり得る (通常の操作の競合)。そこで、持ち主になる context が独立 context のとき、modal を始める 2 か所
     (3D の入口でモデル確認のダイアログを開く `toggle_depth_parallax`、書き出しの受付 `admit_depth_export`) で、その context が
     **`active_detached_transition_outstanding()`** (アクティブな別ウィンドウの内部遷移が残っているかの唯一の判定。フォルダ移動・
     PDF / ZIP の列挙待ち・遅延 reopen・Preparing などを含む) なら、modal を開かずに「読み込みが終わってから使ってください」と通知して拒否する。
     判断は viewer を閉じる前の、modal を開く時点で行う。main・環境設定由来には広げない。
     modal を開いた後は、新しい遷移は開く要求の拒否・共通 modal・自動送りの停止で受け付けないので、modal 中に持ち主が終わる通常の経路は残らない。
   - これらはすべて、独立した窓で 3D を使うときだけ現れる新しい制限で、main の MainWindow / Fullscreen / linked F12 の挙動は変えない。
   - 非 modal の段階 (追加モデルの Running / Done / Cancelled / Error) は止めない。持ち主が終われば `Main` に移り、main の閲覧が body を描かない間は
     見えなくても入力は止めず、通常の body に戻れば描かれる (今の環境設定由来の案内と同じ性質)。
   - 受入条件: 固定した状態では描く場所が 1 つ。持ち主の退場後は生存する表示先だけが描く。frame 全体での呼出し回数を 1 にそろえる frame フラグは作らない。
5. **入場の再開を要求元 context へ配送する** (設計レビュー r1 P2)。モデル確認は ROOT の `poll_depth_setup` のまま。`ModeEntry(request)` の再開は、
   `request.context` が main 以外なら `with_viewer_context(request.context, …)` で mount してその context の `resume_depth_entry_after_check` を呼ぶ。
   照合は今の items 世代・fs_idx・item_id に加えて `viewer_context_live`。満たさなければ再開しない (既存の「古い要求は捨てる」)。
   mount の中で 1 の「ほかの context が 3D を持っていない」も同じ関数で判定される。`DepthViewState::new(edit_request_owner_context())` は要求元を所有者にし、
   推論の wake 先は後続の子 pass の `advance_depth_source` が子 viewport で捕捉する (r2 で確認済み)。
6. **一時データの id を持ち主の context で分ける**: `depth_panel_rect` と `depth_export_display_snapshot` の id に 3D の持ち主の context を混ぜる
   (1 により同時に 1 つしか書かないが、「context 固有の値は所有 context だけが読み書きする」を形で保証する)。

**まれな失敗の割り切り**: modal 中に所有窓の viewer が失敗で閉じる (読み込み失敗でフォルダ移動が窓を閉じる等) 場合、答えは `Main` に移り、
main の閲覧が body を描かない状態と重なると modal がしばらく見えない。通常の操作では起きない組み合わせなので、仕組みは足さない (ログ + 既存の通知)。

**状態を減らす検討**: 持ち主を常に 1 つに保ち (1)、持ち主が終われば 3D も終わり (2)、modal の間は独立した持ち主を終わらせない (4)。
これにより「持ち主の無い modal」「2 つの 3D」「at rest の 3D」の組み合わせを作らない。新しい保留状態・ハンドオーバー・待ち・再試行は足さない。
入口 (`CheckingOrigin`) をダイアログの寿命の間持つのは、既存の値の寿命を延ばすだけで状態の種類は増えない。

**detached 述語・viewport 経路に触れる範囲** (第 1 段に加えて)

- 変える: 適格性 (presentation・独立の許可集合を外し、プロセス全体で 1 つの条件へ)、`park_active_detached_context_as_live_media` の 3D 終了、
  ダイアログの描く場所の関数、`document_open_modal_admission_blocked` / `accept_active_detached_window_close` / 自動送りの停止条件に
  「独立した 3D modal の持ち主」を加えること、入場の再開の配送、一時データの id。
- 変えない: viewport の生成・id・builder、HWND の同定、placement、focus、z-order、bundle の swap、fork / park の手順、`reset_detached_pause_foreground_modes`、
  `render_fs_body` の root / child の振り分け、backstop、F11 / F12 のキー経路。App に detached 用の bool / Option を新設しない
  (持ち主は既存の operation と `DepthSetup` から読む)。時間窓・再試行・追加 repaint は足さない。

**テスト** (第 1 段のテストに加えて)

1. 入口の matrix: 独立 context × DetachedWindow、main の独立窓で許可。ほかの context (at rest) が 3D を持つときは両方向
   (main 埋め込みが持つ → 独立 context で D、独立 context が持つ → main で D) で `OtherWindow` を返し、どちらの状態も変えない。上部ボタンも同じ。
2. 寿命: アクティブ独立 context の 3D → 他の窓の activation (pause / park) で終わる、`park_active_detached_context_as_live_media` で終わる
   (要求の取消・slot 除去)、閉じる (推論中を含む) で終わる。終えた後に別の context で入場でき、`OtherWindow` にならない。scoped mount では終わらない。
3. ダイアログ: 独立 context で D (モデル未導入) → 案内がその窓の pass だけに出て、main の埋め込み・ROOT では描かれない。独立 context の書き出し →
   その窓だけ。持ち主が main のとき・環境設定由来は第 1 段と同じ。
4. modal 中の保護 (独立した持ち主): 書き出しの各段階と追加モデルの Checking / Confirm で、その窓の OS の閉じる操作・他の窓の activation・外部からの開く要求・
   スライドショーの送り (フォルダをまたぐ送りを含む) が拒否 / 停止され、状態が変わらない。持ち主でない窓は閉じられる。main が持ち主の案内・環境設定由来の
   案内では、開く要求・activation・スライドショーが今までどおり受け付けられる (退行なし)。
4b. 受付済み遷移との排他: 独立 context でフォルダ移動を受け付け → 結果が届く前に D (モデル未導入) / 書き出し → modal は開かず通知で拒否 →
   正常な結果が届き、同じ窓・同じ context で新しいフォルダが開く。通常フォルダと PDF / ZIP (列挙待ち) の両方。遷移が無ければ modal は開く。
5. 非 modal: 独立 context の追加モデル Running 中にその窓を閉じる → 案内は main の表示 (一覧なら ROOT) に移り、操作できる。
6. 入場の再開: 独立 context で D → ROOT の poll で「導入済み」の確認完了 → その独立 context だけが入場 (1 回の D で始まる)。確認中にページが変わった・
   窓がアクティブでない場合は再開しない。
7. 一時データの id: 3D を終えて別の context で始めた最初の pass に、前の context のパネル矩形・書き出し snapshot を読まない。
8. 既存回帰: `test(/depth|export|presentation|window_mode|panorama|detached|multiwindow_scenario|slideshow/)`。

**設計レビュー r5 (GPT-6.1 Sol / xhigh、read-only、基点 `1ff4082a8`)**: P1 1 件 (受付済みのフォルダ移動の完了が、広げた開く要求の拒否で
reopen を拒まれ、modal 中に持ち主の窓が終わる通常の競合) → 4 の「受付済みの遷移がある間は modal を始めない」とテスト 4b を追加。
ほかの点 (1 context 制約、ParkedLive の終了、入場の再開の配送、live 判定と描く場所、modal の分類、close の範囲、main・環境設定由来の非退行、
Remote・トレイ・終了、まれな失敗の割り切り) は妥当との確認。P2・P3 なし。判定「指摘修正後に実装開始可」、構造的変更に合意 (はい)。

**文書**: マニュアル (3D 写真の「使える場面」: 別ウィンドウでも使える、一度に 1 つの窓だけ、独立した窓でモデル確認・書き出し中はその窓を閉じたり
他の窓へ切り替えたりできない)、keymap-spec の 3D の節、本書 §1・§6.1・§6.4・§6.4.2 の「第 2 段は保留」の記述、detached-rework-plan §11 (第 2 段の記録)。

#### 6.4.4 第 2 段の実機課題の修正 (§1.767、2026-11-13、設計)

**対象**: §17.16 の課題 1〜4。サブ PC 確認 依頼 39 (観測はサブ PC 担当) で見つかった。原因の調査は実装担当とは別の GPT-6.1 Sol / high (read-only) が
コードと `C:\miv-subpc\results-39\` のログで行い、設計担当がコードで照合した。課題 1〜3 はどれも v5.1.0 から既存の detached の所有の不整合で、
第 2 段の「同時に 1 context だけが 3D を持つ」規則で利用者に見えるようになった (課題 3 の「断られる」組合せだけが第 2 段で生じた)。
凍結ルールにより、どれも症状パッチではなく所有の移動・確定点を正す構造的修正とし、独立 Sol / xhigh と合意して detached-rework-plan §11 に記録する。

**課題 1 — 独立した窓の中の送りで独立性を失う (BA-7)**

- 原因: 送りの行き先は `requested_viewer_presentation_for_open` が「今の session が独立 detached で、次が静止画」を根拠に DetachedWindow を選ぶ
  (第 1 条件)。ところが確定の `prepare_viewer_presentation_open` は独立性を `detached_still_open_is_independent` (「画像を別ウィンドウで開く」設定か
  一度きりの指定) で決め直し、今の session の独立性を見ない。フル機能モードで独立になったメディアウィンドウ (メインで別のものを開いて
  ParkedLive になり、再び有効にした窓) では、動画→最初の画像で独立性が false になり、次の送りは F12 設定 OFF なら `open_non_detached` でメインへ移る。
  同じ事実 (この session は独立か) を 2 か所で別々に決めているのが根本。
- 修正: 表示先と独立性を 1 回の解決で決め、確定時に設定から決め直さない。**既存の session の中の移動 (送り・ページ移動・フォルダ移動後の reopen など、
  新しい閲覧を始めない遷移) は、今の session の独立性を引き継ぐ**。一覧・外部・コレクションなどから新しく開く場合は、今どおり設定と一度きりの指定で決める。
  解決結果 (presentation, independent) を 1 つの値として `prepare_viewer_presentation_open` の確定点へ渡し、`commit_viewer_session` はその値だけを書く。
  「既存 session の中の移動か」は呼び出し元の既存の区別 (fs navigation と grid open の入口) から型で渡し、App に新しい bool / Option を足さない。
- 通常の使い方への影響 (3D を使わなくても変わる): **独立したメディアウィンドウで画像を送り続けられるようになる** (今は 2 枚目からメインへ移り、
  メディアウィンドウは閉じる)。一覧から動画を開いただけの連動したメディアウィンドウ (メインの閲覧の一部) は変えない (画像へ進むとメインで開く)。
  F12 の連動ウィンドウ・複数ウィンドウモードの窓は、もともと独立性が設定と一致しているので変わらない。1 枚目は今も窓に留まるので、
  「1 枚目は窓、2 枚目はメイン」という食い違いをなくす変更であり、手間が増える・できなくなる操作は無い。
- 3D は、表示先が変わらない送りでは続く (§6.4.1 の継続条件のまま)。

**課題 2 — 再開した窓が、閉じた後に待機中の窓として作り直される (BA-7、BA-5 関連)**

- 原因: `activate_detached_image_window_snapshot` は、再開する窓の snapshot を `detached_image_windows` から先に remove し、その後で
  `park_and_close_current_active_detached_viewer` を呼ぶ。そこで使う `active_viewer_context_id` は、session が無いとき window binding /
  `last_active_detached_window_id` から移動元を探し、「snapshot が無い窓 = active」と判定する。再開する窓が直前の active 窓だった場合
  (pause → 再開)、snapshot を先に消したためにその窓自身が移動元と判定され、`pause_current_active_viewer_context` が同じ context の snapshot を
  もう一度 push する。再開後は active と snapshot の両方に同じ窓番号が残る。active の間は passive 描画から現在の active id が除外されるので見えず、
  × の active close が snapshot を消さないため、閉じた後に `render_detached_image_windows` が残った snapshot を `deferred_passive_seed` で描き直す。
  メインの全画面の close (`main focus guard`) は前提ではない。3D も成立条件ではない (v5.1.0 から既存)。
- 修正: 再開を **移動元と移動先を明示した 1 つの所有の移動**にする。移動元 (今の active owner) を、対象の snapshot を消す前に確定し、
  対象の取り出しで移動元を推定し直さない。snapshot の消費・active の選択・runtime の遷移を同じ処理が確定する。ParkedLive の再開
  (`activate_parked_live_media_window_snapshot`、移動元を park してから対象を remove する) と同じ順序の規則にそろえ、両方が同じ helper を通る形を優先する。
  描画側に「registry に無い snapshot は描かない」guard を足すことはしない (孤児の snapshot と所有の不整合が残るため)。
- 通常の使い方への影響: なし (二重登録の除去のみ)。

**課題 3 — 描かれないメインの全画面が 3D を持ち続ける (BA-7、BA-5 関連)**

- 原因: 独立した窓の context を更新したフレームでは、メインの閲覧は埋め込み表示のときだけ描かれ、通常の全画面は描かれない
  (`should_render_main_fullscreen_viewport_after_detached_context`、既存テスト `main_embedded_fullscreen_still_renders_while_detached_context_is_active`
  が意図として検査)。全画面の child viewport は呼ばれないので egui が退役させ、root は一覧を描く (利用者には「全画面が閉じて一覧に戻った」と見える)。
  一方でメインの `fullscreen_idx`・presentation・Depth はそのまま残る。detached の pause は Depth を終えるが、この「メインが表示を失う」遷移は
  その終了の境界を通らない。第 2 段の入場条件は兄弟の Depth の有無だけを読むので、見えないメインが持ち主として残り、別の窓の D が断られる。
- 修正: **メインが表示を失う遷移 (独立した context を有効にして、メインの全画面が描かれなくなる確定点) で、メイン context の中で既存の
  `set_fs_exclusive_view(None)` を行い 3D を終える**。要求の取消と GPU slot の除去は既存の Depth 終了と同じ経路で済む。埋め込み表示のメインは
  実際に描かれるので終えない (その場合の「別のウィンドウで表示中」の拒否は維持)。確定点は、独立 context を active にする既存の入口
  (ParkedLive の再開、passive 窓の再開、新しい独立窓を開く入口) を列挙し、1 つの helper にまとめる。D の判定から兄弟を書き換えたり、
  毎フレームの描画結果を見て終えたりはしない。
- 採らない案: メインの見えない全画面の閲覧そのものを閉じる (状態は最も減るが、3D を使わない通常の閲覧で閲覧位置・`fullscreen_keep_on_app_switch` の
  保持を変える。リリース済みの挙動の変更なので利用者の判断が要り、今回の範囲を超える)。入場判定で見えないメインの Depth を無視する
  (単一 owner の規則を破り、要求と GPU slot が 2 つになる)。
- 通常の使い方への影響: メインの全画面で 3D 中に独立した窓を有効にすると、メインの 3D が終わる (戻ったときは D で再開)。これは detached の
  pause と同じ規則で、3D 以外の閲覧は変えない。

**課題 4 — 文言**

- マニュアル `fullscreen.html` の「その窓を閉じる操作、ほかの閲覧ウィンドウへの切り替え、外部からファイルを開く操作、自動のページ送りが止められる場合があります」
  を、外部からの要求は画面を閉じた後に開くこと (`src/app/startup_ops.rs` の既存の保留、最後の 1 件) が分かる文言に直す。
- 「一度に 1 つのウィンドウだけ」の説明に、ほかの窓へ切り替えると前の窓の 3D は終わることを加える (断られるのは前の窓が表示されたまま 3D を
  持っているときだけ)。

**記録のみ (課題 5)**: 一覧にマウスが届かなかった件は原因不明のまま (§17.16 の 5)。課題 3 の「一覧が描かれていても、見えないメインの閲覧が
残っている」状態との関係は未確認 (依頼 39 の 1 回目の経路ではメインの全画面は Esc で閉じていた)。

**テスト** (旧実装で落ちること):

1. 状態遷移 / handler: 設定 OFF で独立したメディア session → 画像 A → 画像 B。両方の移動後も同じ context・window_id・DetachedWindow・独立性。
   Depth あり / なし。ありでは表示先変更による Depth 終了が起きない。対照: 連動のメディアウィンドウ (一覧から動画を開いた直後) の画像はメインへ、
   明示的な F12 の切替、動画への往復、一覧から新しく開く場合は今どおり設定で決まる。
2. 状態遷移: 最後の active 窓 A を pause → A を再開した直後に、A の passive snapshot が 0 件。実 close handler を 1 回通すと、context・binding・
   runtime・passive の描画要求がすべて消え、後続の root pass で同じ id を再登録しない (multiwindow scenario で、メインが一覧 / 全画面、Depth あり / なし)。
   通常の A→B 切替、ParkedLive の再開、再開を断る場合の対象の保持も回帰に含める。
3. 状態遷移 / multiwindow scenario: メインの全画面 + Depth → 独立した窓を実際の activation で有効にする → メインの Depth が終わり、要求の取消・slot の除去が
   済み、独立した窓の D が成功する。メインの埋め込み表示 + Depth では終わらず、独立した窓の D は OtherWindow で断られる (対照)。
4. 既存の第 1 段・第 2 段のテストはそのまま通る。

**凍結ルール**: 3 件とも、所有の移動・確定点を 1 か所にそろえる変更で、guard / delay / retry / 追加 repaint / silent fallback を足さない。
App に detached 用の bool / Option を足さない。viewport の生成・HWND・placement・focus・z-order・bundle swap・backstop は変えない
(課題 2 の snapshot の push / remove の順序と、課題 1 の確定点に渡す値は変える)。detached-rework-plan §11 に範囲と理由を記録する。
まれな失敗は既定の割り切り (ログ + 通知)。

**設計レビュー r1 (GPT-6.1 Sol / xhigh、read-only、対象 `3108c2cd2`)**: P1 1 件・P2 2 件、判定「指摘修正後に実装開始可」、構造的変更であることに**合意**。
課題 2・3 の原因、BA 分類、触れない範囲の見通しは妥当とされた。反映:

- **P1 (課題 1)**: 独立性を引き継ぐだけでは、既存の 2 つの操作が変わる。(a) 独立した静止画の窓は動画・音声への送りを断る
  (`should_block_detached_independent_still_navigation_to_video`、`open_fullscreen_from_fs_navigation_with_contract`) ので、3D を使わない
  「動画→画像→動画」の往復が新たに止まる。(b) F12 は `requested_viewer_presentation_for_open` の「今の session が独立」の条件が優先され、
  設定を OFF にしてもメインへ移らなくなる。「できなくなる操作は無い」の評価は誤りだった。**課題 1 は利用者の判断待ちとし、実装から外す**。案:
  - 案 A: 独立したメディアウィンドウを「メディアの窓」として扱い続ける (画像も動画も送れる、F12 でメインへ移せる)。送りの可否・F12・
    フォルダ / 本 / PDF / ZIP の遅延 reopen・再開・新規 open・fork / promote の入口ごとに所有の扱いを決める (P2-3 の分類表) 必要があり、規模が大きい。
  - 案 B: メディアウィンドウを 3D の対象から外す。入場条件を「メインの context か、独立した session (複数ウィンドウモードの窓など)」にし、
    動画から画像へ進んで連動になったメディアウィンドウでは D を断る。行き先の規則は変えず (リリース済みの「2 枚目からメインへ」のまま)、状態の組み合わせが最も少ない。
  - 案 C: 今のまま (メディアウィンドウの 3D は最初の 1 枚だけで、送るとメインへ移り 3D が終わる)。
- **P2-2 (課題 3)**: 表示を失う境界では、Depth だけでなく、その表示から出た未完了のモデル確認の入場要求 (`DepthEntryRequest`、Checking) も同じ寿命として
  失効させる。メインの全画面で初回 D → 確認中に独立した窓を再開 → 確認完了で見えないメインが Depth に入る、という通常の非同期の順序を閉じるため。
  モデルの確認処理そのもの・環境設定由来の案内は取り消さない。境界の入口は、`activate_parked_live_media_window_snapshot`・
  `activate_detached_image_window_snapshot` に加えて `start_active_detached_loading_context`・`start_active_detached_collection_root_image`・
  `start_active_detached_book_context_with_start`・materialized fork の後の open を監査し、通常の更新や一時 mount と、初めて表示の所有が移る確定を区別する。
  パノラマ (360°) は対象に含めない。
- **P2-3 (課題 1 の入口分類)**: 案 A を採る場合に限り、同じ閲覧内の送り・フォルダ / 本 / PDF / ZIP 移動後の reopen (`close_fullscreen_for_folder_nav_reopen`、
  `DeferredFsReopen`)・passive / ParkedLive の再開と descriptor による再構築 (`continue_active_detached_book_context_from_descriptor` は既存窓でも
  `fs_open_intent_from_grid=true` を立てる)・新規 open・F12・fork / promote の分類と、遅延する要求への由来の受け渡しを設計に書いてから実装する。
- 課題 2 の共通 helper は、移動元を先に確定して park し、対象を id で取り直して消費する順序にする。ParkedLive 側の `preserve_fullfeature_linked_still=true`
  と各再開方式に固有の処理は保つ。helper の中で移動元を推定し直さない。テストは `last_active_detached_window_id=A` を持つ実際の pause → 再開を使う。
- pin (切り離し) は現行の正本では廃止済みなので、範囲の記述から外す。「送りでは 3D が続く」は同じ一覧の中の静止画の送りに限る (フォルダ移動は既存の閲覧終了で 3D が終わる)。

**実装の範囲 (r1 の後)**: 課題 2・課題 3 (P2-2 を含む)・課題 4 の文言を先に実装する。課題 1 は利用者の判断の後に別に設計・実装する。

#### 6.4.5 課題 1 — 独立したメディアウィンドウから静止画の窓への交換 (§1.767、2026-11-14、設計)

**利用者の決定 (2026-11-14)**: (a) フル機能モード + 「動画・音声は別ウィンドウで再生」で、**独立した**メディアウィンドウ (メインで別のものを開いて ParkedLive になり、
再び有効にした窓) から同じ閲覧の送りで画像へ進んだら、メディアウィンドウを閉じ、**同じ独立した閲覧 (同じ context・同じ一覧) のまま静止画の窓を開き直す**。
メインは変えない。連動したメディアウィンドウ (一覧から動画を開いた直後) からの画像は今どおりメインへ (F12 ON なら F12 の窓)。(b) 一覧から開くときの今の動きは変えない。
(c)(d) 静止画から動画・音声へ送ったときの今の動きは変えない (独立した静止画の窓は今どおり断る)。(e) 開き直した窓は元の窓の位置と大きさを引き継ぐ。
独立性を設定から決め直している根本もあわせて直す。本節は §6.4.4 の課題 1 (と r1 の案 A〜C) を置き換える。下書きは GPT-6.1 Sol / high (read-only) が master
`403725b81` のコードで作り、設計担当が範囲を絞った (下の「由来の扱い」)。

**適用範囲**: フル機能モード (複数ウィンドウモードでない) かつ `effective_media_in_media_window()`。複数ウィンドウモードのメディア窓 → 画像は今どおり同じ窓・独立のまま
(利用者の決定の範囲外)。分類は `GridItem` で行う: メディア = `Video`・`Audio` (音声モード・音楽表示を含む)、静止画 = `Image`・`ZipImage`・`PdfPage`。

**不変条件**:
1. フル機能モードのメディアウィンドウ (専用の表示先としての役割) は Video / Audio だけを表示する。F12 ON の連動ホストを静止画表示へ再利用するのは既存の F12 の静止画表示であり、
   この役割ではない (「一度でも動画を出した OS 窓には画像を出さない」という定義にはしない)。
2. **独立性は閲覧 (context と窓の結び付き) の性質であり、確定点で設定から決め直さない**。独立性が決まるのは、新しく窓を結び付けるとき (一覧などからの新規 open は
   設定・一度きりの指定で決める)、fork / promote / 再開の既存の所有移動、F12 (明示的な表示先の変更) だけ。
3. 独立した閲覧のメディア → 静止画は、窓の役割の明示的な交換として 1 か所で確定する。

**由来の扱い (設計担当の決定、下書きからの簡素化)**: 下書きは、送り・遅延 reopen・descriptor 再構築など全ての入口に「新規 / 継続 / 表示先変更」の由来の型を付け、
遅延する要求 (`DeferredFsReopen`・book composition の `OpenIntent`・`MediaNavigationPending`・source-swap pending・descriptor 継続) まで運ぶ案だった
(実装 3〜5 人日)。由来を運ぶ代わりに、**確定点で「この context に、今この窓が結び付いた独立の session があるか」という既存の事実から決める**:
- `prepare_viewer_presentation_open` で、開く context に detached の window binding があり、その session を続ける (同じ window id のまま確定する) なら、
  独立性は今の値 (`detached_viewer_independent_active`、その context の bundle に属する) を引き継ぐ。設定 (`detached_still_open_is_independent`) は見ない。
- binding が無い (新しく窓を結び付ける) ときだけ、今どおり設定と一度きりの指定で決める。一覧からの新規 open は、既存の `prepare_detached_context_for_grid_open` が
  先に park / 再開を済ませるので、ここに来る時点で binding が無い (下書きの分類と同じ結果になる)。
- F12 は既存の `toggle_detached_viewer_mode` / `always_new_media_f12_target_presentation` が表示先と独立性を明示的に決める。その経路では「今が独立」という理由で
  DetachedWindow を優先しない (`requested_viewer_presentation_for_open` の第 1 条件を通常の送りの解決だけに使う)。presentation が同じで独立性だけ変わる場合も確定を通す。
- 遅延 reopen (フォルダ / 本 / PDF / ZIP、`close_fullscreen_for_folder_nav_reopen` は window id を保つ) と descriptor による既存窓の再構築は、binding が残るので引き継ぐ。
  `fs_open_intent_from_grid` は独立性の判定に使わない。
- これで、メディアへの送り (動画 A → 動画 B) でも独立性を false に戻さない (今はメディアへの open でも false に書き戻している)。
この簡素化が成り立たない経路 (binding が残っているのに新しい閲覧として独立性を決め直すべき入口、または binding が一時的に外れている継続) がコードにあれば、
その入口だけに既存の要求の型で由来を足す。App に bool / Option を足さない。

**解決規則** (送り・自動送り・遅延 reopen の確定点。表示先と独立性を 1 回で決め、`commit_viewer_session` はその値だけを書く):

| 今の閲覧と送り先 | 結果 | 窓 |
| --- | --- | --- |
| フル機能・独立メディア → 静止画 | DetachedWindow・独立 | A を退役し、新しい B を同じ context に結ぶ (下) |
| フル機能・連動メディア → 静止画、F12 OFF | メイン (`non_detached_viewer_presentation`)・連動 | 既存の `open_non_detached` |
| 同、F12 ON | DetachedWindow・連動 | 既存ホストを F12 の静止画表示に再利用 |
| 独立静止画 → 静止画 | DetachedWindow・独立 | 同じ窓 |
| 独立静止画 → Video / Audio | 既存どおり断る | 変えない |
| メイン / F12 の静止画 → Video / Audio | 今の presentation のまま | 変えない |
| メディア → メディア | 今の presentation と独立性のまま | 同じ窓 (既存の source-swap) |
| 複数ウィンドウモードのメディア → 静止画 | DetachedWindow・独立 | 今どおり同じ窓 |

**窓の交換 (独立メディア A → 独立静止画 B、同じ context C)**: 閲覧の終了ではなく C の表示ホストの交換。park・別 context の作成・フォルダの読み直しをしない。
`close_fullscreen()` → `open_fullscreen()` は使わない (スライドショー停止・pending 取消・キャッシュ破棄が起きる。`close_fullscreen_now_for_destination`)。1 つの helper で:
1. 送り先が採用できる (候補・提示許可が決まった) 境界で行う。決まる前に A を閉じない。交換を起こした要求は局所値に取り出してから進める。
2. A の placement を Closing の前に捕捉する (`capture_detached_window_placement_at_decision` → `ensure_detached_window_runtime_placement`)。最大化も含む。
3. C の旧メディア資源だけを既存の項目離脱の規則で退役する (player、native presenter、音声モード / VST、音楽表示)。**メディアの再生は止まる** (同じ閲覧で別の項目へ進むため)。
   C 全体の `fs_cache.clear()` はしない。join・sleep・終了待ちを足さない。
4. A の session と出力を `close_active_detached_session_exact` / `finish_active_detached_session_close` の部品で退役し、A–C の binding を明示的に外す
   (独立 C では自動の unbind が効かない)。`remove_detached_window_runtime_inner` が A 所有の pending を取り消すので、手順 1 の後に呼ぶ。
5. `allocate_detached_viewer_window_id` で B を作って C に結ぶ。B の runtime を `Opening`・連動でない・placement P (A のもの、offset なし) で、最初の builder より前に確定する
   (`set_detached_window_runtime_placement`)。`last_active_detached_window_id` なども B にし、A を指す passive snapshot・deferred view・入力要求を残さない。
6. (DetachedWindow, 独立) で B の静止画 session を始め、以後は通常の画像ロード・描画経路。前景の送りからの交換では B を前に出す (既存の表示・focus の確定に渡す)。
   メインの reclaim は起動しない。新しい foreground 奪取・繰り返しの raise・時間窓を足さない。
7. A が F11 の枠なし全画面だった場合: B も枠なし全画面で開く (既存の F11 の状態を B の最初の確定で適用する)。既存の部品で確定前に適用できない場合は、通常窓で開いて
   設計担当へ戻す (新しい待ちや再試行で合わせない)。
同じ window id のまま役割だけ替える案は、利用者の決定 (閉じて開き直す) を表さず、旧メディアと新しい静止画の寿命が同じ lease に残るので採らない。BA-4 (同種の送りでは
窓 id を保つ) とは、種類変更だけを明示的な新しい窓にする点で整合する。B は静止画の runtime で旧 player を持たないので、一覧から新しいメディアを開いても
`close_parked_live_media_windows_for_new_media` (ParkedLive でメディア資源を持つ窓を置き換える) の対象にならないことをテストで確かめる (除外用のフラグは足さない)。

**3D**: メディアウィンドウに静止画を出さないので、メディアウィンドウでの 3D の入口は無くなる。A は動画を表示していたので交換時に引き継ぐ 3D は無い。B では第 2 段の規則で
3D が使え、同じ一覧の静止画の送りでは C・B・presentation・独立性が変わらないので 3D は続く。

**テスト** (旧実装で落ちること。resolver の期待値だけでなく実 handler と所有移動の結果まで):
- 独立メディア A → 画像 1 → 画像 2 (実 navigation handler): 画像 1 で B≠A、同じ C・一覧・順序、B は独立。画像 2 も同じ B。メインの一覧・選択・閲覧は変わらない (multiwindow scenario)。
- 独立メディア → 同種メディア → 画像: 途中も独立のまま、画像で 1 回だけ交換。音声・動画の音声モードでも同じ。
- A の完全退役: session・binding・runtime・passive / deferred の描画要求が消え、後続の ROOT pass で A を再登録しない。B を 1 回の × で閉じられる。
- 配置: A の P が B の runtime と最初の builder seed に一致。F11 の枠なし全画面から交換しても枠なしで開く。
- 遅延 reopen (フォルダ / PDF / ZIP) と descriptor 再構築: 独立した窓で独立性を保つ。`fs_open_intent_from_grid=true` でも既存の閲覧を新規扱いしない。
- F12: 実 F12 handler で、独立した窓から設定に従って表示先・独立性が変わる。遅延の完了が F12 を取り消さない。
- 対照 (変えない): 連動メディア → 画像 (F12 OFF はメイン、ON は同じ連動ホスト)、一覧からの画像 → メディア → 画像 (静止画の窓の再利用・ParkedLive の再生継続・
  同じメディアの再開)、静止画 → Video / Audio (メイン / F12 は今の表示先、独立静止画は断る)、複数ウィンドウモード、B を残して一覧から新しいメディアを開いても B が閉じない、
  B の 3D → 同じ一覧の次の画像で 3D が続く。候補なし・取消のときは A を閉じない。
- 既存テストの対照: `independent_detached_still_blocks_navigation_to_video`、`independent_still_video_navigation_uses_detached_hint_not_global_toast`、
  `fullfeature_linked_still_is_preserved_before_different_media_open`、`fullfeature_image_media_image_reuses_parked_linked_still_window`、
  `fullfeature_image_media_media_image_reuses_same_still_window`、`same_media_grid_open_activates_parked_live_without_close`、
  `parked_live_audio_window_is_identified_for_minimal_live_display`、`parked_live_video_audio_mode_uses_music_display_and_preserves_mode`。

**触る範囲**: `src/app.rs` (表示先と独立性の解決、`prepare_viewer_presentation_open`、F12 の確定、窓交換の helper)、`src/app/viewer_context_registry.rs`
(同じ C の A → B の binding 交換を既存の単一 binding の規則の中で行う helper)、`src/app/native_video.rs` (旧メディア資源の退役の接続)、テスト、本書・
detached-rework-plan §11・関連文書・マニュアル。**触らない範囲**: 課題 2・3 の修正そのもの、decoder・presenter・音声処理・VST bridge、HWND の同定、
placement の計算・保存形式、focus / z-order の基盤、メインへの C の移管、複数ウィンドウモードの種類変更、静止画 → メディアの仕様。App に detached 用の
bool / Option / pending を足さない。guard / delay / retry / 追加 repaint / silent fallback を足さない。まれな失敗は既定の割り切り (ログ + 通知)。

**BA 分類**: BA-7 主 (表示先と独立性の二重決定、種類変更の窓交換の分散を 1 つの確定に直す)、BA-4 関連 (同種の送りは安定した id、明示交換だけ新しい id)、
BA-6 関連 (runtime の placement を次の窓の seed に渡す。新しい保存先・同期経路を作らない)、BA-5 関連 (旧窓の描画要求を残さない)。

**規模**: 由来を状態から決める簡素化で、実装は課題 2・3 の確定後に 1 人の実装担当が 1.5〜2.5 人日、独立レビューと修正で 0.5〜1 人日の見込み
(利用者が了承した 1〜1.5 日を超える。統括に報告済み)。

**設計レビュー r1 (GPT-6.1 Sol / xhigh、read-only、対象はこの節の案)**: P1 1 件・P2 2 件・P3 1 件、判定「指摘修正後に実装開始可」、構造的変更であることに**合意**
(BA-7 主、BA-4・BA-6 関連、BA-5 を退役の回帰対象とする分類も妥当)。由来を状態から決める簡素化は、下の補正つきで採用できるとされた。反映:

- **P1**: descriptor による再構築 (`activate_detached_image_window_snapshot` → `resume_active_detached_book_context_from_descriptor` →
  `start_active_detached_book_context_with_start`) は、同じ window id でも**新しい context** を作り、`ViewerSession::default` の独立性は false。
  「binding が残るので引き継ぐ」は誤りだった。**binding を予約・公開する時点で、解決済みの (presentation, 独立性) を bundle へ初期確定する**:
  既存の `resume_window_id` / `DetachedBookContextStart` の開始・再開の区別を使い、再開なら再開元の独立性、新規 detached book open なら新規 open の規則で決める
  (コレクション root の `start_active_detached_collection_root_image` は既に独立性を初期設定している)。以後のページ open は現在値を引き継ぐ。
  由来を `DeferredFsReopen` や media pending へ広げる必要はない。回帰: 設定から独立性を得られない状態で、実 activation → descriptor 再構築 → 列挙完了 → 最初・次のページ。
- **P2**: 複数ウィンドウモードの新規 Video / Audio は今 `independent_detached_still=false` で確定し、次の画像で設定から true になっている。継承の規則では false を引き継いで
  「同じ窓・独立」にならない。**新しく binding するときの規則で、複数ウィンドウモードのメディアは (DetachedWindow, 独立) を初期確定**し、以後は引き継ぐ。
  一覧の選択追従・F12 は今の動きを保つ (対照)。回帰は独立フラグを手で立てた fixture ではなく、通常の一覧から新規 Video / Audio → 画像 1 → 画像 2 で、
  窓 id・独立性・メインの選択・静止画からメディアへの拒否を検査する。
- **P2 (F11)**: F11 は即座に bool を切り替えず `DetachedViewerBorderlessTransition::target_borderless` に要求を持つ。F11 直後の送りは通常の操作で起こる
  (`fs_nav_is_locked` は遷移をロックしない)。**交換の前に、遷移中ならその target、無ければ確定した状態を捕捉し、通常時の restore placement と一緒に B の最初の
  生成 (`build_detached_viewer_viewport_builder`、初回から枠なしを指定できる) へ渡す**。`reset_active_detached_viewport_runtime_for_new_window` が borderless /
  restore / transition を消すので、その後に B へ設定する。上の手順 7 の「確定前に適用できない場合は通常窓で開いて戻す」は削除する (通常の操作の表示の変更であり、
  まれな失敗の割り切りではない)。回帰: F11 確定後・入り途中・戻り途中の交換と、B で F11 を戻したときの通常の配置・最大化の復元
  (枠なしのときは runtime の通常配置 P と builder の全画面矩形が違うので、単純な一致を期待しない)。
- **P3**: session の確定 (`commit_viewer_session_deferred_feedback`、`src/app/depth_view.rs`) は `target_independent` を 3D の終了判定に使うだけで独立性を書かない。
  **`src/app/depth_view.rs` を変更範囲に入れ、確定点が (presentation, 独立性) を一緒に書く**。F12 の egui 経路 (`apply_egui_viewer_presentation_for_current_item`) と
  native の確定経路 (`apply_video_presentation_switched`) も解決済みの独立性を確定する (presentation だけの helper を呼び続けると古い独立性が残る)。
- 訂正: 一覧などからの新規 open は「全て binding が無い」わけではない (連動の静止画の窓は再利用されて binding が残る)。その場合の現在値 (連動 = false) の継承は結果を変えない。
  外部・single instance・MCP・検索・タグ・履歴・sidecar / パスワード待ち・source-swap・EOF・スライドショーで、誤って独立性を引き継ぐ追加の経路は見つからなかった。
- 通常の使い方への影響に追記: 画像へ移った後は、その窓から動画・音声へ送り戻せない (独立した静止画の窓は動画・音声への送りを断る、利用者の決定 (c)(d) に含まれる)。
  窓の作り直しによる表示の間隔・フォーカスの変化は未測定。
- 規模: 実装 2〜3 人日、レビューと修正 0.5〜1 人日 (全件テストと実機確認は別)。

**実装中の追加決定 (2026-11-14、設計担当)**:
種類の変更（独立メディア A → 静止画 B の窓交換）は、所有する閲覧が active のときだけ確定する。
受理済み Manual の結果が A の ParkedLive 後に届き、送り先が交換を要する静止画なら、理由をログに残して
結果を採用せず捨てる。A は現在の動画・音声を再生する ParkedLive のまま、active な別窓 D の session・binding・
window は変更しない。同種メディアの結果は従来どおり背景で採用する。再 activation 待ちの pending と背景 B の生成は追加しない。

判定は `admit_manual_media_navigation_target` の1か所へ集約する。通常の `MediaNavigationPending` と
Collection 専用 Manual の各 request owner が currentness と対象を照合した後、一覧・表示・資源の採用前に呼ぶ。
交換の要否は送り先の `GridItem` と現在の閲覧から、active かどうかは owner の window binding と
`active_detached_session.window_id` の一致から決める。native の Manual landing / AwaitingNative 中の結果も
同じ採用規則を通す。描画側の guard、新しい App の bool / Option、待機や再試行は加えない。

理由: フル機能モードのメディアウィンドウが背景で静止画を表示する旧動作は、不変条件1に反する課題1の一部である。
背景交換には B の準備・非同期採用・F11 / focus の追加設計が要り、再 activation までの保留は状態を増やすため採らない
（CLAUDE.md「設計の簡素化」）。画像が出ないのは要求の受理から結果までの間に別窓へ切り替えた場合だけで、
元の窓を選んで送り直せる。設計担当の決定として採用し、利用者への追加照会は行わない。

#### 6.4.6 サブPC依頼50の停止窓・外部open・HWND登録（2026-11-14、調査・修正設計）

観測・コード原因・未確定点・修正案・旧版比較の正本は
[依頼50の調査と修正設計](depth-parallax-realmachine50-investigation.md)。
今回は製品コードを変えず、失敗回帰だけを追加する。
停止窓のcallbackクリック受付欠落と、外部openによる旧Depth ownerの孤児化、
rectに依存するキーsubclassの誤同定を分け、Aの実機入力を通常ログだけで断定しない。
BA-1 / BA-7 / BA-5の所有境界を直す設計とし、設計担当の検収と独立設計レビュー後に実装範囲を確定する。

### 6.5 他の機能との関係 — 「表示を占有するモード」を 1 つの述語にする

- スライドショーは3D中もSキー・上部の再生ボタン・リングから開始 / 停止できる。
  上部の「…」は引き続き表示しない。360°のSキー / 上部ボタンの抑止とリングの既存動作は維持する。
  先読み・引継ぎ・間隔・最小化中の休止は [3Dスライドショー設計](depth-slideshow-plan.md) に従う。

360° が他の機能を止めている箇所 (調査では 30 か所余り: キー・編集ツールの入口・補正パネル・
ズーム / パン / ホイール・見開き・ルーペ・メタデータパネル・ナビゲータ・ページ一覧・
トリミング・動画タイル) に、3D 用の同じ判定を 1 つずつ書き足さない。

- §6.2 の `FsExclusiveView` を読む述語 `fs_exclusive_view_active(fs_idx) -> Option<ExclusiveViewKind>`
  (`Panorama` / `Depth`) を 1 つ作る。これは判定の集約で、状態の持ち主は §6.2 の 1 つ。
- 360° の判定箇所を 2 種類に分ける: **「表示を占有するモード中だから止める」**箇所は新しい述語に置き換え、
  **360° の意味に依存する**箇所 (ホイール = 画角、ドラッグ = 視線、ナビゲータの球面表示など) は
  `is_panorama_mode_active` のまま残す。
- 単純に置き換えられない箇所 (独立レビューの指摘): ズームの持ち主 `FsZoomOwner` (ui_fullscreen.rs:15104)、
  タッチのピンチ・パン、紙めくりの可否 (page_turn.rs:1809)、連結表示の判定。3D 中の挙動は次のとおり
  決めておき、実装担当は表に書く:
  - ズーム・パン・自由回転: 通常 2D の持ち主 `FsZoomOwner::Image` へ通す。ページ内スクロールと
    読み始めの初期位置も共用する。Z の部分拡大ズームとナビゲータは引き続き休止する (§5.2)。
  - **紙めくり**: 単ページの静止画は紙めくりの対象 (`page_turn_view_eligible`) なので、入口条件では
    除外されない。**3D 中は紙めくりの対象外**にする (`page_turn_view_eligible` の 360° の判定を占有モードの
    述語に置き換える)。3D 中のページ移動は紙めくり無しの通常の切替になる。
  - **紙めくりの最中に D を押した**: 入口条件で弾く (理由付き)。紙めくりは短いので待ってもらう。
    紙めくりの表示 (Native) を 3D の素材にしないので、表示の持ち主の切替を 3D 側で扱わない。
    紙めくりが終わると表示は既存の通常合成 → 素材の準備の経路 (app.rs:90001、86998) に戻るので、
    次に D を押せば元テクスチャがある。
  - 連結表示の判定: 連結読みは入口条件で除外済み。3D 中に連結へ切り替わったら §6.3 で OFF。
- 実装担当は全箇所を表にして (箇所・どちらに分類したか・理由)、本書の §10 に記録する。
  置き換えで 360° の挙動が変わらないことを、既存の 360° のテストで確かめる。
- 3D 中のホイール: **通常 2D と同じ規則**。Ctrl+ホイールはズーム、はみ出している間の修飾なしホイールは
  設定に従いページ内スクロールを先に行う。同じ pass の予測と実適用も同じ分類を使う。
- 3D 中の修飾なしマウス左クリックは、2D と同じく変換がないときにページ送り。拡大・パン・回転中は
  ページを送らず、ダブルクリックで表示をリセットする。タッチのタップは既存の 2D の規則を保つ。
  マウスの Ctrl+左クリックは release イベントの修飾で判定し、同じ pass の描画資源・実効 transform から
  止まる深さを決める (§17.2)。
  計算中だけ通知し、失敗・元画像ホールド・画像外は何もせず、いずれもページは送らない。
  表示中の 3D パネルではクリック・タップに加え、ドラッグ・中ボタン・スクラブ・ホイール・ピンチを始めない。
- 3D 中はフィット方式・指定倍率と上限・拡大しない／縮小しないを変える閲覧操作を受け付けない。
  環境設定の画面は例外。PDF の表示倍率由来の需要を 2 入口で止めて印を需要 owner に保持し、非 Depth の
  最初の通常／Z 単ページ描画で記録を空にして再照合する。AI 用 native の再ラスタは倍率によらず継続する。
  setter に PDF の副作用は追加しない ([表示変換設計](depth-view-transform-plan.md) §3.4)。
- 段 B の §10 の分類表は当時の記録。今回の全出現の分類は [表示変換設計の実装記録](depth-view-transform-plan.md#13-実装記録) に置く。
  リングの Z モード・pixel grid・比較 pin の抑止、metadata のパネル切替は変えない。
- 3D 中の左右パネルの表示切替 (I / Tab): **3D パネルの表示・非表示を切り替える**。他のパネルは
  3D 中は出ない。
- **透過部分の背景切替 (Shift+B / ImageBackgroundCycle)**: Depth中も許可する（利用者決定2026-11-08）。
  表示占有による制限ではなく、キーの抑止は360°固有に分類する。Panoramaの既存キー抑止は維持。
  リングは基点でもPanoramaを抑止していないため、その既存挙動を維持する。AIアップスケール中は従来の黒/白2段と
  表示キャッシュ失効を共用し、素材が変われば§4.1どおり深度も作り直す。書き出しの凍結は§7のまま。

## 7. 書き出し

### 7.1 流れ

書き出しの基準は閲覧中の変換と分ける。Page fit・zoom_pan なし・自由回転 0・倍率制限なし、
同じ表示切り取りと 90 度回転で解決した変換を `record_display` に渡す。
閲覧中のズーム・パン・自由回転・フィット設定は出力範囲と市松の比に影響しない
([表示変換設計](depth-view-transform-plan.md) §3.6)。

1. 3D パネルの「書き出し…」→ **モーダルの書き出しダイアログ** (`App::common_modal_dialog_open` に登録)。
   - 形式: MP4 / GIF / APNG (APNG には「ファイルが大きくなります」の注記)
   - 長辺: 720 / 1080 / 1280 / 1600 / 2160 px (元のテクスチャより大きいものは出さない。
     720 px 未満なら元の長辺を1つ表示)
   - 長辺の既定: MP4 1280 / GIF 720 / APNG 1080 px。形式を切り替えるたびにその形式の既定へ戻す。
     選択肢にないときは既定以下で最大の候補、なければ最小の候補。設定は保存せず、開くたびに既定から始める
   - 動き: 自動の動き 4 種から (パネルがマウス追従なら左右を初期値にする)
   - MP4 だけ: 長さ (ループ回数 1〜10、既定 4)
   - 強さ・周期・焦点・整形はパネルの設定をそのまま使う (ダイアログで二重に持たない)
2. 「保存先を選ぶ」→ 既存の core と同じ `rfd::FileDialog::save_file` (既定名 `<元の名前>_3d.mp4` 等)。
3. **モーダルの進行ダイアログ** (フレーム数 / 全体、キャンセル)。書き出し中は他の操作を受け付けない。
4. 終わったらトーストで保存先とファイルの大きさを知らせる。worker が rename 後のファイルから大きさを取り、
   完了事象に載せる。取得できなければ大きさだけ省く。UI スレッドで metadata を読まない。

書き出しをモーダルにするので、書き出し中のページ移動・3D OFF・設定変更の組み合わせは起きない
(CLAUDE.md「長い処理はモーダルにして割り込みを消せないか」)。ただし `common_modal_dialog_open` への
登録だけでは、外から来る「開く」要求と mIV Remote の取得は止まらない (独立レビューの指摘):

- **Remote**: 書き出しダイアログを開く時点で既存の `try_local_modal_lease` (remote_ipc/session.rs:1448)
  を取り、取れなければ書き出しを始めない (既存の通知付きの版を使う)。lease は書き出しの進行ダイアログが
  閉じるまで持つ。
- **開く要求** (関連付け・D&D・単一起動への引数など): `document_open_modal_admission_blocked`
  (app.rs:42299) に書き出し中を足し、既存のモーダルと同じく受付を止める。
- **凍結**: 書き出しの要求を作る時点で、パネルの設定・回転・uv の範囲・画像の寸法・色テクスチャと
  深度テクスチャの `Arc`・**透過部分の下地** (§7.2) を `DepthExportRequest` に写す。worker はこれだけを見る。

### 7.2 フレームの描画

- **画面と同じシェーダ**でオフスクリーンのテクスチャ (書き出しの大きさ、**`Rgba8Unorm`**。§4.2 の
  色空間と同じ) に描き、読み戻す。描き方 (WGSL) を画面用と書き出し用で 2 つ持たない。
  - pipeline・uniform buffer・bind group は**書き出し用に別に作る**。画面用の prepare は毎フレーム
    uniform を書き換える (panorama_wgpu.rs:526 と同じ作り) ので、共有すると書き出しのフレームに
    画面の値が混ざる。
  - **透過**: 書き出しはすべて不透明にする (MP4 は透過を持てず、GIF の 1 bit 透過は縁が汚れるため。
    形式で挙動を分けない)。下地は**画面で透過部分に敷いている下地と同じもの** (黒・白・市松。
    画像の外の余白色とは別、ui_fullscreen.rs:10742、10819) を使い、その種類と市松の升目の大きさ
    (画面の升目を書き出しの寸法の比で拡大縮小したもの、画像の左上を原点) を要求に凍結する。
    egui のテクスチャは premultiplied alpha なので (renderer.rs:456)、合成は gamma 空間で
    `rgb + 下地 × (1 − a)`、合成後の alpha は 1。
  - 回帰検査: 中間の灰色 (sRGB 128) と半透明の画像で、黒・白・市松の各下地について画面の描画と
    書き出しの画素が一致すること。
- 描画・読み戻し・エンコードは **書き出し worker** で行う。wgpu 30 の `Device` / `Queue` は
  `Clone + Send + Sync` なので、worker が encode → submit → `poll(Wait)` してよい (UI スレッドは待たない)。
  色テクスチャ・深度テクスチャは凍結した要求の `Arc` で持つ (モーダル中は差し替わらない)。
- フレーム数 `N = round(周期 × fps)`。fps は形式で固定: MP4 30、GIF 15、APNG 15 (スパイクの大きさの計測に合わせる)。
  `phase = i / N`。

### 7.3 形式ごと

| 形式 | 使うもの | 要点 |
| --- | --- | --- |
| GIF | `image` 0.25 の `GifEncoder` | `Repeat::Infinite`。256 色化は gif crate の NeuQuant (フレームごと)。**遅延は 10 ms 単位**で、`image` は ms の整数へ切り捨ててから 10 ms 単位にする (image-0.25.10 codecs/gif.rs:586)。`1000 / 15` ms をそのまま渡すと各 60 ms になり 1 周期が約 10% 短くなるので、累積時刻から各フレームに 60 / 70 ms を配分し、1 周期の合計を周期に一致させる |
| APNG | `png` 0.18 の `Encoder::set_animated` / `set_frame_delay` | 可逆なので大きい (720 px・2 秒で 14〜18 MB、研究調査 §8.3) |
| MP4 | `video::mvexport` の部品 | H.264 の encoder 選択 (`open_h264_encoder`)、BT.709 の YUV 変換、`PartialOutput` (一時ファイル → 完了時に rename)、faststart。**映像だけの `Mp4Muxer`** を足す (今の `Mp4Muxer::new` は音声 stream を要求する)。寸法は偶数に丸める |

- MP4 の部品は `pub(super)` のものがあるので `pub(crate)` に広げる。音楽ビデオの書き出しの挙動は変えない
  (既存のテストで確かめる)。
- 取り消し: 進行ダイアログのキャンセルと、窓を閉じる・アプリ終了 (`Drop` で cancel)。途中のファイルは残さない
  (MP4 は `PartialOutput`、GIF / APNG も一時ファイル → rename にそろえる)。

## 8. パネルの UI

- 右側の固定パネル (Area + Frame::popup + ScrollArea。coding-conventions の「固定パネル」と
  `tests/support/scroll_layout.rs` の高さ検査に従う)。見出し「3D 写真」は ScrollArea の外。
- 見出しの三角と「3D 写真」をクリックすると見出し 1 行に折りたたみ、もう一度で展開する。折りたたみは設定に保存する
  (`panel_collapsed`、§17.12)。I / Tab の完全非表示、× と D の 3D 終了とは別の操作。
- 項目:
  - 動き: マウス追従 / 左右 / 円 / 8 の字 / 上下
  - 強さ: 0.5〜5%、既定 2%
  - 周期: 1〜10 秒、既定 3 秒 (マウス追従のときは無効表示)
  - 止まる深さ: 奥 ↔ 手前 (0〜1)、既定 0.5 (整形 ON なら中央値の深さ)
    - 下に「画像を左 Ctrl+クリックすると、その場所の深さになります」と小さな「既定値」ボタン。
      スライダーのドラッグ・変更、既定値、Ctrl+クリックで1秒保持＋0.3秒フェードのハイライトを出す。
      表示だけの状態で、書き出しには含めない (§17.4)。
  - 奥行きを整える: ON / OFF、既定 ON (説明: 手前の床などが奥行きを占めて平らに見えるのを防ぐ)
  - 「書き出し…」
  - 状態の1行: 「モデルを準備中」「奥行きを計算中」「失敗しました (理由)」。待機中は画像上にも
    §5.4の状態箱を出し、パネルを隠していても知らせる。準備／計算中の判定はパネルと箱で共有する。
- 配色は `os_theme` の helper、文字色は `ui.visuals()`。固定文言に絵文字を使わない
  (`scripts/check_ui_glyphs.py`)。
- 上部ホバーバーに 3D ボタンを足す (360° ボタンの隣)。入口条件で弾かれる画像では無効表示。
- UI スナップショット (`tests/ui_snapshot.rs`) にパネルを足す。

## 9. キー

- `KeyAction::FsDepthParallax`、既定 **D**、文脈は画像フルスクリーン (`FsImage`)。
  D は消しゴム・隠蔽加工・補正レイヤーの描画モードでも使うが、それらは 3D と同時に入れない
  モード内のキーなので衝突しない (360° の V と消しゴムの V と同じ関係)。
- 実装担当は `keymap.rs` の `default_chords()` と `ALL_ACTIONS` で D が画像フルスクリーンと共通の文脈で
  空いていることを確かめる。`ini_name()` / `context()` / `trigger()` / `docs/keymap.ini.default` /
  keymap-spec をそろえる。

## 10. 実装の段階 (Codex への委任単位)

| 段 | 内容 | 受入れ |
| --- | --- | --- |
| A | **実装済み (2026-11-07)**。配布用モデルの作成スクリプト、アドオン (VLM の共通化)、`ModelKind`、深度 worker、前処理の Rust 版。詳細・検証記録は直下 | Python 固定 fixture 比較を含む対象 82/82、VLM アドオン既存テスト・snapshot 成功。実モデル 1/1、Python stretch 相関 1.00000000 |
| B | 3D モード: 状態・入口・遷移・占有モードの述語 (§6.5 の表)、GPU の色テクスチャと読み戻し、シェーダ、マウス追従、パネル、キー | 状態遷移テスト、入口条件のテスト、360° の既存テスト、シェーダの不変条件テスト (§11) |
| C | **実装済み (2026-11-07、ClaudeCode検収・利用者実機確認待ち)**。書き出し (GIF / APNG / MP4、映像だけの muxer、進行ダイアログ)。直下に実装・検証記録 | 限定lib286/286、UI関連6/6成功。全件gateはClaudeCode担当 |
| D | **文書更新済み (2026-11-08)**。製品ページ・マニュアル・日英 privacy・ライセンス案内・モデル公開手順。更新一覧とコード照合は §12 | 公開文書の検査、Rust はライセンス文言のみの check・関連テスト（結果は §12） |

各段のあと独立レビュー (Sol / xhigh)。実機確認は利用者 (`build-dev.ps1` のバイナリ)。

### 段 A の実装記録 (2026-11-07)

- `tools/depth_model/build_depth_addon.py` でローカルの SpatialHub 原本を fp16 化・504 固定・単一 ONNX 化。
  Apache-2.0 全文は既存の `vendor/egui/LICENSE-APACHE` を流用し、変更内容入り NOTICE と
  `depth-model-index.json` を生成する。実変換先は git 管理外の `target/depth-addon/`。
- `addon.rs` に転送・検証・連結・配置・管理 worker を集約し、`AddonKind` で VLM / 深度を選ぶ。
  VLM 固有の GGUF / model ID / 4 ファイル検証は `vlm_addon.rs`、深度の固定 ONNX / LICENSE /
  NOTICE 検証は `depth_addon.rs`。保存先・配布タグは §3.2 のとおり。既存 downloader の
  Range 再開・取消時 staging 保持・転送再試行を共用する (§13 に新たな回復機構は足さない)。
- App の `DepthSetup` は導入状態を cache し、設定の「アドオン」に導入・削除・取消・進行を追加。
  VLM とボタン・進捗描画を共用する。`DepthSetup::installed_model_path` (cached status の判定) と
  `App::request_depth_addon_setup` が段 B の入口用 API。3D モードの状態・案内からの入場は未実装。
- `ModelKind::DepthDa3MonoLarge` と `depth_estimation.rs` を追加。共通 runtime owner を待ち、
  `load_model` / `with_session` で固定形の fp32 I/O を実行する。モデルの完全検証は cold load 時だけで、
  保持済み session があればページごとのファイル hash は行わない。TRT pack / 拡大モデル一覧には含めない。
- worker は process に 1 本、`Mutex + Condvar` の待機 slot は最新 1 件。同 context の active 要求を
  supersede し、別 context の active 要求は取り消さない。slot の差替は旧 queued 要求を取消・解放する。
  `DepthPending` は要求ごとの receiver と cancel を持ち、Drop と採用時にも取消を照合する。
  `context + request_id + key` を照合し、完了後に要求元 viewport を repaint。lease は実処理終端まで保持する。
- `depth_preprocess.rs` に percentile / cubic / guided filter / scaled 膨張・Gaussian / median を実装。
  `DepthMap.values` は `Vec<half::f16>`、median は量子化前の f32。ガンマ・焦点は掛けない。
  非有限の disparity は前処理入口で拒否する (モデル出力の非正・非有限 depth は先に視差 0 へ変換)。
  Python / OpenCV の段別 fixture は 16,861 bytes。大サイズ case は全処理後の境界・中央 sample と全量 median を記録する。

設計の機能変更はない。膨張半径の整数化は Python / OpenCV に合わせた round-to-even、ぼかしは
長辺比例の実数 sigma とする。取り消し token は既存の viewer / BiRefNet と同じ `Arc<AtomicBool>`。
要求は `App::local_ai_activity_lease()` を受け取り、待機中も含めて実処理終了まで保持する。
ViewerContextBundle / GPU / キー・3D パネルは段 B、公開文書は段 D に残す。
段 B で導入・削除と 3D 要求を接続するときは、§3.4 の session 保持／既存 reload 契約と
表示側の取消を整合させる。UI から `unload_model` を直接呼んで推論 mutex を待つ経路は作らない。
要求を出す側は既存 AI と同じ `App::ensure_ai_runtime()` で初期化を開始してから共有 owner を渡す。
初回の導入確認は lazy scan の完了を待って判定し、未確認を未導入として再取得させない。

#### 段 A の検証記録

対象は `depth-parallax` の本段の未コミット差分。全 Cargo コマンドで
`MSBUILDDISABLENODEREUSE=1`。native 依存の初回 24 並列 build で FFmpeg bindgen の
access violation と TurboJPEG の MSBuild 失敗が発生したため、TurboJPEG を `/m:1 /nr:false`
で構築し、その後は `CARGO_BUILD_JOBS=1` で検証した。製品テストの失敗とは区別する。

| 実行 | 結果・証拠 |
| --- | --- |
| `python -m unittest discover -s tools/depth_model -p test_build_depth_addon.py` (spike venv) | 2/2 成功。fp32 I/O、fp16 initializer、固定入力、外部 tensor なし、index の hash / bytes、別 export の拒否。`target/depth-python-tests.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/^addon::/) \| test(/depth\|vlm_addon/) \| test(=ai::trt_worker_lifecycle::tests::models_outside_the_pack_never_start_the_worker)'` | 最終 82/82 成功、exit 0。UPDATE 指定なし。前処理 4 件、worker の取消・相関・別 context、VLM 共通化の既存回帰・snapshot、深度 pack の欠落 / 同長 hash 破損 / size 不一致、UI 小画面・busy を含む。`target/depth-tests-final.log` |
| 新規 UI snapshot だけ `UPDATE_SNAPSHOTS=1` で明示生成 | 2/2 成功。その後上の通常比較も成功。`depth_addon_confirm_dark.png` 64,353 bytes、`depth_addon_preferences_dark.png` 49,555 bytes。実装者・独立 reviewer が日本語、ライセンス、取得元、保存先、ボタンの欠け・重なりが無いことを目視。`target/depth-snapshot-update.log` |
| `MIV_DEPTH_MODEL_DIR=target/depth-addon`、nextest `--run-ignored only -E 'test(=depth_estimation::tests::depth_real_model_matches_python_static_stretch)' --success-output immediate` | 1/1 成功 (2.69 秒)。実写真 `target/depth-spike/samples/IMG_0001.JPG` (EXIF 反映後 720×960) を同じ `INTER_CUBIC` stretch で入れ、504×504 出力・Python 相関 **1.00000000**。共有 `AiRuntimeInitOwner` / DirectML で実行。通常 profile は使わず test data dir に隔離。`target/depth-real-model.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit 0。`target/depth-check-final.log` |
| `cargo fmt` (引数なし) → `cargo fmt --check` | 成功、exit 0。`target/depth-fmt.log` |
| `python scripts/check_ui_glyphs.py` | 危険グリフ 0、exit 0。`target/depth-glyphs.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` (`CARGO_NET_OFFLINE=true`) | 成功、exit 0。通常 feature / 非 portable の core と Remote / EPUB / MCP / music-video editor の 5 exe、FFmpeg 6 DLL と CRT 4 DLL を配置。PE 依存検査 runtime=4 / pe=5 成功 (`target/vcrt-pe-reports/dev-runtime.json`)。core は `target/dev-runtime/mimageviewer-core.exe`、391,114,240 bytes。実行していない |

実変換したモデルは 671,032,016 bytes、SHA-256
`0f5ff3be208d04bc34937946546d75b6d12d8aba4aa2cf1cc26a6bde2c26ed95`。
`tools/depth_model/README.md` に生成・fixture・実写真 oracle の再生成コマンドを残した。
モデル・写真 oracle・build/test log はすべて git 管理外。ネットからモデルや大きなファイルは取得していない。
全体 gate / 実アプリの対話試験は未実施 (今回の指定は範囲を絞った検証)。製品起動・実機検収の代用とはしない。
公開配信先での実取得も未確認 (転送・配置はローカル HTTP サーバで検証)。利用者の確認対象は、設定の
「アドオン」から「3D 写真の追加モデル」の表示と取得案内の開閉。3D 表示の入口自体は段 B。
通常版／トレイ常駐を終了してから、利用者が次のコマンドで起動する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

これは高速ビルド用の通常 profile で、既定の `%APPDATA%\mimageviewer` の実設定・データを更新し得る。
single-instance mutex を通常版と共有する。エージェントによる起動は行っていない。

変更ファイル:

- 依存・module: `Cargo.toml`、`Cargo.lock`、`src/lib.rs`。
- 転送・導入: `src/addon.rs`、`src/depth_addon.rs`、`src/vlm_addon.rs`、`src/vlm_addon_download.rs`。
- worker・推論: `src/depth_estimation.rs`、`src/depth_preprocess.rs`、`src/ai/mod.rs`、
  `src/ai/model_manager.rs`、`src/ai/tensorrt_builder.rs`、`src/ai/trt_worker_lifecycle.rs`、
  `src/previous_run_diagnostics.rs`、`src/previous_run_diagnostics/checkpoint.rs`。
- 設定 UI・App: `src/app.rs`、`src/ui_dialogs/mod.rs`、`src/ui_dialogs/depth_addon.rs`、
  `src/ui_dialogs/model_addon.rs`、`src/ui_dialogs/vlm_addon.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`。
- 生成・fixture: `tools/depth_model/build_depth_addon.py`、`generate_fixtures.py`、
  `test_build_depth_addon.py`、`requirements.txt`、`README.md` (いずれも同ディレクトリ)、
  `tests/fixtures/depth/preprocess.json`、`tests/snapshots/depth_addon_confirm_dark.png`、
  `tests/snapshots/depth_addon_preferences_dark.png`。
- 設計文書: 本書、`docs/async-architecture.md`、`docs/architecture-overview.md`。

独立レビュー (GPT-6.1 Sol / xhigh) は事前の所有境界と完成差分・追加テスト・新 PNG・検証ログを確認し、
P1/P2 指摘なし、段 A の実装は受入可と判断。最終検収と commit は ClaudeCode が行う。
この worktree では stage / commit を行っていない。

#### Codex P1〜P3 対応 (2026-11-07、基点 `6e20dfa43`)

独立レビュー指摘を修正した。ここからは上記の初回実装記録とは別の未コミット差分。

- **P1**: `AddonKind` の scan / install / remove 名 helper を実際の spawn に渡す。
  VLM は従来の `vlm-model-{scan,install,remove}` を復元し、深度は
  `depth-model-{scan,install,remove}`。6 名の完全一致を回帰テストで固定した。
- **P3**: エラー本文全体の `VLM` → `Depth` 置換を廃止した。HTTP・検証・配置の生成点で
  `kind.label()` を埋め込み、パスと下位エラー文字列はそのまま保持する。取消 sentinel も
  種別ごとの producer / consumer を揃えた。VLM の既存文字列と回帰テストは維持。
  `VLM` を含むパス・JSON 下位エラー・HTTP 下位エラーの保持など、P1/P3 合計 7 テストを追加。
- **P2**: 504×504 の二軸 ramp・矩形段差・整数 seed noise を両言語で再生成し、
  1536×1024 の二次元ガイドへ `parallax.prepare_depth` と同じ前処理を行う検査を追加。
  境界・段差の両側・全体を分散して **797 画素**、および全量の中央値を比較する。
  fixture 全体は **38,053 bytes**。許容差は f16 丸めの最大 1/4096 と float32 の
  OpenCV SIMD / Rust 加算差を合わせて画素 3e-4、量子化前中央値 2e-5。
  実測の最大差 **0.00024390**、中央値の差 **0.00000003** (debug / 最適化版とも成功)。
- 実モデル ignore テストを推論から前処理まで `depth-estimation` thread 上で通す形に広げた。
  同じ実写真 (720×960、ガイドは拡大しない) の **691,200 画素すべて**を Python の f32 結果と比較。
  推論相関 **1.00000000**、前処理後の最大差 **0.00024426**、平均差 **0.00005896**、
  中央値の差 **0.00000003**。1/1 成功。画像・全量 oracle は `target/depth-addon/` だけに保存。
- 200 ms の目安を超えたため、percentile / median の全件 sort を隣接順位の選択へ変更。
  box filter の縦 rolling sum を行順に処理し、Gaussian は横の reflect を padded row に事前計算、
  縦を連続行の加算にした。各画素の f64 加減算・Gaussian の tap 加算順は維持し、並列化は追加していない。
  percentile は旧 sort とビット一致する回帰も追加した。

**前処理の計測条件**: Intel Core Ultra 9 285、rustc 1.99.1、Windows x64。
`tools/depth_model/preprocess_probe/` は製品 `src/depth_preprocess.rs` を `#[path]` で直接コンパイルする
独立したテスト用 workspace。アルゴリズムのコピーを持たず、アプリと同じ MiMalloc 0.1.52
(`default-features=false`、同じ vendored libmimalloc-sys)、root の linker / `+crt-static` を使う。
最適化は root と同じ `dev-runtime` (opt-level=2、lto=off、codegen-units=64 など)。
repo root から `--offline --test-threads=1`、他の Cargo / テスト実行と重ねずに計測した。
同じ `depth-estimation` thread で 1 回 warm-up 後に 3 回測定し、その中央値。
入力生成・thread 作成・返却 map の drop は除外し、`prepare_depth` 内の確保・scratch 解放・全フィルター・
中央値・f16 化は含める。入力と完成結果を `black_box` で観測する。3D 表示全体の応答時間の計測ではない。

| profile | 修正前 3 回 (ms) | 修正前中央値 | 修正後 3 回 (ms) | 修正後中央値 |
| --- | --- | ---: | --- | ---: |
| `dev-runtime` (最適化) | 214.360 / 214.752 / 216.848 | **214.752 ms** | 119.596 / 121.536 / 123.314 | **121.536 ms** |
| debug (`test`) | 2206.732 / 2193.411 / 2193.933 | **2193.933 ms** | 1317.034 / 1310.109 / 1309.065 | **1310.109 ms** |

最適化版は 200 ms の目安を下回った。debug は診断用であり、200 ms の判定には用いない。
計測ログは `target/depth-perf-mimalloc-{before,after}-{optimized,debug}.log`。
修正前は `git show 6e20dfa43:src/depth_preprocess.rs` の製品コード部分を改変せず、現行テスト部分だけを
組み合わせた一時 host (`target/depth-preprocess-baseline/`) で計測した。
元製品コード部分の SHA-256 は `354fe1e3243fc1ceb7ba47eefea25df19284b48286c11990f7a8a072bffd03e6`。
同じ条件で修正前後の全 1,572,864 画素の f16 と中央値を保存し、**ビット単位で全量一致**も確認した
(3,145,732 bytes、両方の SHA-256 は `ad7c934e1664a3133bd5bfce0ad71c7a615b3e8afc9e17e96b2e183c082eac02`)。
一時 host と比較データは git 管理外。単離用 host の再実行方法は `tools/depth_model/README.md` に記載。

**検証** (`MSBUILDDISABLENODEREUSE=1`、ネット取得なし):

- 本体の前回と同じ nextest 絞り込み: **91/91 成功**、exit 0 (`target/depth-review-tests.log`)。
  VLM の既存テスト・snapshot と、新規実サイズ・順位選択・診断回帰を含む。
- 実モデル ignore テスト: **1/1 成功**、exit 0 (`target/depth-review-real-model.log`)。
- 単離 host の通常テスト: debug / dev-runtime **各 6/6 成功** (同じ製品テストを再利用、重複合算しない)。
  timing テストは上の 4 条件で各 1/1、全量ビット比較 host は前後各 1/1 成功。
- `cargo check --offline -p mimageviewer --bin mimageviewer-core`: 成功、exit 0 (`target/depth-review-check.log`)。
- 最後に standalone host とワークスペース全体へ `cargo fmt` を実行。`cargo fmt --check` も成功。
  UI グリフ検査は危険グリフ 0、exit 0 (`target/depth-review-{probe-fmt,fmt,glyphs}.log`)。
- `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`: 成功、exit 0 (通常 feature、非 portable)。
  core と 4 コンパニオンを更新し、PE 依存検査 runtime=4 / pe=5 成功。
  `target/dev-runtime/mimageviewer-core.exe` を起動せず引き渡す。通常 profile の起動手順・
  実設定への注意は上記のとおり。全体 gate / 実 UI / 公開配信先の実取得は今回も未実施。
- 独立 reviewer (Sol / xhigh) は P1/P3 の生成点・取消分類と数値同値、上のログ・基点ソースを確認し、
  新規指摘なし、P1〜P3 修正は受入可と判断した。

変更ファイル: `src/addon.rs`、`src/vlm_addon_download.rs`、`src/depth_addon.rs`、
`src/depth_preprocess.rs`、`src/depth_estimation.rs`、`tools/depth_model/generate_fixtures.py`、
`tools/depth_model/README.md`、`tools/depth_model/preprocess_probe/{Cargo.toml,Cargo.lock,src/lib.rs}`、
`tests/fixtures/depth/preprocess.json`、本書。stage / commit は行わない。

### 段 B の実装記録 (2026-11-07、実装・独立コードレビュー済み)

段 A の上に状態・表示・入力を接続する未コミット差分。独立コードレビューは ACCEPT、
P1/P2 残件なし。ClaudeCode の検収・利用者の実機確認は未実施であり、全件 gate の失敗と
補修後の限定検査は下記の実行記録で区別する。stage / commit、製品バイナリの起動は行わない。

- `fs_exclusive_view.rs` の `FsExclusiveView::{None, Panorama, Depth}` を App と
  `ViewerContextBundle` の一つの owner とする。非アクティブな panorama state も同じ enum に残す。
  生きた状態の入替えは `set_fs_exclusive_view` が旧 owner を退役してから公開する。
  `app/depth_view.rs` が入口、継続照合、presentation の commit、パネル操作、worker 接続を担う。
- 型の本体 (`MotionKind`、`DepthParallaxSettings`、`MouseFollow`、`DepthEntryInput` / block、
  `DepthSourceMaterial` / wait、`DepthSourcePhase`、`DepthLru`、`DepthViewState`) は
  `depth_parallax.rs` に置く。設定本体を `settings.rs` に分散させず、`Settings.depth_parallax` が参照する。
  全体設定として保存し、欠落時の既定・未知の動き・非有限値・範囲外値は既定または範囲内へ正規化する。
  設定の転送は既存の「環境設定にある項目だけ」方針に従い、3D パネルで操作するこの値を対象外とする。
  import に同名値があっても転送先の値を維持する。全体保存と転送の対象範囲を混同しない。
- 表示の正本を複製せず、`resolve_fs_processed_payload_and_depth` が通常 selector の選択した
  payload と readiness を一緒に返す。元画像ホールド、通過表示、注釈ベイク、不完全な final、
  thumbnail fallback、元 source のない Native は `NotYet`。RAW preview は既存契約のまま扱う。
  Native の再準備は §15 の既存の通常 final/edit 準備を使う。
- `depth_parallax_wgpu.rs` は managed source を sample して sRGB 符号値の `Rgba8Unorm`
  色テクスチャへ blit し、mipmap を作る。504×504 と長辺最大1536のガイドを別に描いて
  bounded buffer へ読み戻す。UI は `map_async` と非 blocking の `PollType::Poll` / receiver poll だけを使う。
  深度は `R16Float`、現在色だけを保持し深度 LRU は4件、callback は context / key / resource ID を照合する。
  `DepthViewState` の Drop は要求を取消し、自分の context の callback slot だけを除く。
  色・深度・readback は `VramSubsystem::DepthParallax` と既存の context 会計に含める。
- shader は基準の逆引き式を維持する。マウス target はポインタの正規化変位の **負** とし、
  `source = pixel - offset * (depth - focus)` で手前がポインタから離れる。
  右へ動かすと手前は左、下へ動かすと手前は上へ動く。純粋な追従テストと GPU の色 ramp テストで
  target の符号と実際の sample 方向を検査する。これは実機での見え方の検収済みを意味しない。
- 新規 managed texture の GPU 登録は UI の後であり、`load_texture` 自体は次の frame を要求しない。
  `Ready` 素材の backing が未登録の場合だけ要求元 viewport を再描画し、登録後に読み戻しを始める。
  一般の `NotYet` は既存 producer の完了通知を使い、ReadingBack は Poll + repaint、推論結果は worker の
  viewport repaint、同 frame の upload 繰越は次 frame repaint が起床を所有する。
  実際の texture delta の登録を挟む App 回帰と、ポインタ入力なしの state → Ready 回帰で確認した。
- 固定パネルは明示的な位置・本文高の制約を正本とし、Area の推定サイズによる二重の位置補正を無効にする。
  1093×614 と 546.5×307 の contains / scroll end / header固定 / close の検査、PNG 目視を行う。
- 独立実装レビュー (GPT-6.1 Sol / xhigh) は最終コードと新規回帰を **ACCEPT、P1/P2 なし**とした。
  ClaudeCode の検収、利用者による実機確認とは分けたコードレビューの結論である。
- `ui_depth_parallax.rs` は固定ヘッダと高さを制限した scroll body を持つ右パネル。
  動き・強さ・周期・焦点・奥行き整形を保存し、マウス追従では周期を無効表示する。
  段 C 前の「書き出し…」は準備中の無効表示。3D 中は fit 固定、通常 wheel はページ送り、
  Ctrl+wheel / mouse・touch の zoom・pan は停止し、I / Tab は3Dパネルを切り替える設計に接続する。
- detached との境界は [detached-rework-plan §11](detached-rework-plan.md#11-リワーク外からの変更記録)
  に記録した。main context ID と `ViewerPresentation::Fullscreen` を両方要求し、
  `commit_viewer_presentation` が別 presentation の公開前に Depth を終える。
  native 側の静止画 window mode 切替・動画 presentation・rating 後の遷移もこの commit 境界へ通す。
  継続照合は owning pass の先頭と描画直前の2箇所とし、不適格な表示へ持ち越さない。
  読み取り専用の兄弟 context の open / close から別 owner を失効させない。

#### §6.5 の全箇所分類表

追補の入口監査では、キー以外の既存リング dispatch も調べた。段 B で追加する `Depth` のときだけ、
`apply_ring_action` が zoom / pixel grid / 比較 pin を抑止し、metadata は
3D パネルの表示切替へ送る。キー・リング共通の `cycle_fullscreen_fit_mode` も Depth 中は変更しない。
これにより非表示の 2D zoom / pan / 自由回転が fit 切替で初期化されない。
既存 Panorama のリング挙動は今回の変更対象に広げていない。
段Bで抑止していたslideshowは§1.771で許可した ([差分設計](depth-slideshow-plan.md))。
背景切替は2026-11-08利用者決定でDepthの抑止対象から外した。Panoramaのキー抑止と、
元から抑止されていないリングの挙動は維持する。

素材の追加監査では、local edit の materialization が未完了で final が `None` の間に、
通常表示が raw 下地を返す経路を確認した。2D の fallback は維持し、深度素材だけ
`IncompleteComposite` とする。`RawPreview` / `PreviewShown` の現像待ち preview は正本 §4.1 の
明示例外として `Ready` にできる。両方を実 resolver の回帰で検査した。
失敗済みの key は元画像 hold 等の `NotYet` でも保持し、同じ key で推論を再試行しない。

| 追加監査箇所 | 分類 | 理由 |
| --- | --- | --- |
| `app/gamepad_input.rs::apply_ring_action` | Depth の入力規則 | キー以外の direct dispatch でも表示の占有と 2D 状態保持を守る。既存 360° は変更しない |
| `cycle_fullscreen_fit_mode` | Depth の fit 固定 | フィット切替は描画に反映しないだけでなく、保存している 2D transform の reset も行わない |

**基点は実装前 HEAD `8adc67829`**。以下の行番号はこの commit の source を指す。
段 B の編集で行番号や field 名が変わるため、関数名と併記して追跡する。
「占有」は `fs_exclusive_view_active` へ集約する制限、「360」は panorama 固有の意味を維持する箇所、
「分岐」は Depth の仕様を別に決める必要のある箇所。兄弟をまとめた行も対象の出現位置を列挙した。

| 基点の箇所 | 分類 | 判断・段 B の接続方針 |
| --- | --- | --- |
| `app.rs:52806,52809` 補正・分析 panel | 占有 | 他パネルの停止 |
| `app.rs:79046,79147` edit continuation / target switch | 占有 | 非同期再開も同じ編集入口条件 |
| `ui_fullscreen.rs:14656` zoom mode eligibility | 占有 | fit 固定の3Dへ zoom mode を開始しない |
| `ui_fullscreen.rs:22124` similar preview | 占有 | 通常画像を置換する preview の入場停止 |
| `ui_fullscreen.rs:25027` info lock、`28007` overflow、`28041` PDF analysis | 占有 | panel / 通常表示の領域・入力所有 |
| `ui_fullscreen.rs:28382,28387` analysis draw | 占有 | 通常分析 renderer を出さない |
| `ui_fullscreen.rs:29179,29184,29215,29223` local adjust / SNS / crop / trim | 占有 | 各ツール panel の停止 |
| `ui_fullscreen.rs:29281,29677,39757` callouts / overflow request / side chrome | 占有 | パネルの表示と入力受付を揃える |
| `ui_fullscreen.rs:30576` seek menu blocker | 占有 | 通常静止画 menu の停止 |
| `ui_fullscreen.rs` ordinary wheel zoom / canvas click | 分岐（依頼18で訂正） | wheel zoom は占有。canvas click は360°の意味に依存し、Depthの通常ページ送りを許可。保存2D transform は不変 |
| `ui_fullscreen.rs:43016,49713` edit admission / loupe | 占有 | 共通入口と描画の両方を停止 |
| `app/page_overview.rs:230` overview | 占有＋360 | 占有中は入場停止。既存の `panorama_intent.on` による停止は別に維持 |
| `ui_video_tile.rs:69`、`ui_view_trim.rs:993` | 占有 | tile の入口、trim の準備完了後の再開も停止 |
| `ui_fullscreen/page_turn.rs:1813` | 占有 | Depth 中は紙めくりなしで通常 navigation |
| `ui_fullscreen.rs:15104,15110` `FsZoomOwner` | 分岐 | Depth owner を明示し、Image へ落として不可視の zoom 値を動かさない |
| `ui_fullscreen.rs:15127,15192,15225,15233,15245,15281,15292,15374,15405,15430,15655,40331` | 分岐 | pivot・キー/ピンチ・pan・drag・PDF rerender の owner dispatch 全体で Depth は無操作 |
| `ui_fullscreen.rs:15140,15163` panorama touch helper | 360 | FOV と yaw/pitch の専用操作を維持 |
| `ui_fullscreen.rs:40728` wheel navigation 判定 | 360＋分岐 | 360 は全 wheel を FOV に所有。Depth の通常 wheel は navigation、Ctrl+wheel だけ停止 |
| `ui_fullscreen.rs:37999` touch tap zones | 分岐 | 360 の視線 drag と3Dの fit 固定を区別。zoom/pan を止めても3Dの通常ページ移動は維持 |
| `ui_fullscreen.rs:501,24970,24971,29393,29396,29401,29413,29510,29511,51223` topbar inputs | 分岐 | 占有制限と、360 の active / projection 値を分離 |
| `ui_fullscreen.rs:51336,51420,51455,51481,51593` topbar restriction | 占有 / 360 | その他・編集・VSTは占有中に抑止。slideshowは§1.771で3D中も許可、360°中の抑止を維持 |
| `ui_fullscreen.rs:51315,51322` close、`51784,51815,51821` 360 button | 分岐＋360 | × の終了先と tooltip はモード別、360 の選択表示・投影操作は360固有 |
| `ui_fullscreen.rs:34359,34531` Escape / early × | 分岐 | Depth の終了を panorama toggle へ渡さない |
| `ui_fullscreen.rs:35908` と派生 `35915,35916,35918,35919,35920,35921,35922,35923,35924,35925` | 分岐 | Z/M/grid/compareは占有制限、Sは§1.771で3D中も許可し360°中だけ抑止。Iは3D panel切替。編集は共通入口へ通す |
| `ui_fullscreen.rs::handle_fs_key_input_with_operation` の `FsBgCycle`、`gamepad_input.rs::apply_ring_action` の `ImageBackgroundCycle` | 360（キー抑止）／通常操作（Depth） | 2026-11-08決定。Depthは下地を毎描画反映するため背景切替を許可。キーは `is_panorama_mode_active` だけで止める。Panoramaリングは基点でも許可されており変更しない |
| `ui_fullscreen.rs:36011,36104` | 整理 | 前者は診断値、後者は未使用の `_pano_active_now`。機能 gate ではない |
| `ui_fullscreen.rs:36647` Shift+V | 360 | 投影方式だけの操作。V の入場判定は Depth 中の入場を拒否 |
| `ui_fullscreen.rs:28512` | 360 | panorama shader の paint。Depth shader は同じ通常描画境界で別に選ぶ |
| `ui_fullscreen.rs:29246` confirmation banner | 分岐＋360 | banner は360専用だが、その分岐が metadata への fallthrough も防ぐ。Depth panel を別枝にする |
| `ui_fullscreen.rs:38028,38058,38300,38351,38360,38389,38418,38497,39149` | 360 | 球面 navigator の layout / pose / input / paint。Depth の通常 navigator は停止 |
| `ui_fullscreen.rs:37258,37328` navigator admission（Depth のため条件追加） | 分岐 | Depth だけ navigator の描画・入力を止め、保存済み2D pan/zoom を維持する。従来の通常 flat / 360 navigator の可否は維持 |
| `ui_fullscreen.rs:45254,45788` non-paged cleanup / continuous | 分岐 | 連結へ切替で Depth を終了。継続照合は reading-flow の事実を使い、表示 predicate で変化を隠さない |
| `ui_fullscreen.rs:48511,48515,48566,48569,48615` | 360 | drag pose、wheel FOV、panorama renderer |
| `ui_fullscreen.rs:7100,7121,7136,7147,7257,7271` pure chrome / touch helpers | 占有＋分岐 | 呼出元から占有制限を渡す。canvas の意味を持つ touch 判定は360とDepthを区別 |
| `app.rs:94171,94172` | 360 | panorama state と検出結果の意味を維持 |
| `app.rs:94481,94488,94600,95546` | 360 | refinement / GPU preparation の source と pose |
| `app.rs:94969,95037,95067` | 360 | auto-reentry intent / hint / 未使用 current helper。Depth 中の360復帰は共通 owner が拒否 |
| `app.rs:95364,95375,95394,95408,95439` | 360＋owner | projection / intent の意味は維持し、状態の作成・破棄は共通 setter へ移す |
| `app.rs:64157,81507` park / fullscreen close | 分岐 | Depth を終了。park の360 intent は既存どおり保持、close は既存の終了規則 |
| `app/native_video.rs:6137,6145,6154,6162,9097,9102,9104,9114,9136,9140,9157,9161,9173,9176,9418,12915` | 360 | native video の panorama 同期・FOV・pose。Depth へ置換しない |
| `app.rs:17444,21367`、`app/viewer_context_registry.rs:1695,2974,3404,3736,4117,4380` | owner | 保存・初期化・bundle swap を enum へ移す。独立した gate ではない |

テスト側の `panorama_state` 直接設定も同じ enum owner に追従させる。
360 の既存テストは変更前の意図を維持し、Depth の入口・遷移・別 context・GPU の stale resource・
マウス方向・設定転送・パネルの snapshot に回帰を追加する。

#### 段 B の変更ファイル (39 件)

- 新規の型・GPU・パネル: `src/fs_exclusive_view.rs`, `src/depth_parallax.rs`,
  `src/depth_parallax_wgpu.rs`, `src/ui_depth_parallax.rs`, `src/app/depth_view.rs`。
- App の接続・会計: `src/app.rs`, `src/app/gamepad_input.rs`, `src/app/native_video.rs`,
  `src/app/page_overview.rs`, `src/app/rating_membership_ops.rs`,
  `src/app/viewer_context_registry.rs`, `src/app/vram_accounting.rs`。
- 表示・入力・設定・導入: `src/lib.rs`, `src/keymap.rs`, `src/settings.rs`,
  `src/settings_transfer.rs`, `src/ui_dialogs/depth_addon.rs`,
  `src/ui_dialogs/preferences/search_index.rs`, `src/ui_fullscreen.rs`,
  `src/ui_fullscreen/page_turn.rs`, `src/ui_video_tile.rs`, `src/ui_view_trim.rs`,
  `src/vram_budget.rs`。
- 回帰・fixture・snapshot: `src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`,
  `src/app/tests.rs`, `src/ui_fullscreen/front_popup_input_tests.rs`,
  `src/depth_estimation.rs` (test 用完了 receiver fixture のみ), `tests/ui_snapshot.rs`,
  `tests/snapshots/depth_parallax_panel_dark.png`。
- 文書: `docs/depth-parallax-plan.md`, `docs/detached-rework-plan.md`,
  `docs/panorama-360-view-plan.md`, `docs/display-pipeline.md`, `docs/async-architecture.md`,
  `docs/ui-responsiveness.md`, `docs/keymap-spec.md`, `docs/keymap.ini.default`,
  `docs/development-build-and-test-native.md`。

#### 設計との差分と検証の限界

段 B の機能範囲・入口条件・継続条件を変える仕様変更はない。小さな配置・型の置き場所と
設定転送の既存方針への接続は上記に記録した。WGSL は画面 uniform / pipeline から分離し、
段 C の書き出しは実装せず、ボタンを無効表示する。段 D の製品ページ・マニュアル・privacy は未変更。

全件検査で、追加設定の分類済み件数の期待値更新漏れと、段 A のモデル管理ページの検索索引未登録を検出した。
前者は総数489・対象155・除外334へ更新し、後者は実 anchor / page / 見出しで索引登録した。
索引の既存検査は描画 helper の実呼出と `depth_addon.rs` 内の見出しを照合し、検査を省略しない。
この補修と実描画 source の検査追従も独立コードレビューで ACCEPT、指摘なし。

実モデルの品質・所要時間・実 HWND / GPU での操作感は今回の自動検査では検収していない。
右 pointer で手前が左へ動く方向、透過下地、90度回転、ページ移動後の再準備、V / D 相互排他、
F11 / F12 / 見開き・連結・動画への切替での終了を、利用者の実機確認へ引き継ぐ。

#### 段 B の実行記録

- 3D / panorama / 占有モード / keymap の限定検査: 403/403 成功
  (`target/depth-b-focused-final.log`)。その後の最終 3D 検査は88/88成功
  (`target/depth-b-depth-final2.log`)。GPU offscreen の不変条件・逆引き方向・slot / LRU、
  App の repaint 所有・要求相関・一 frame 一枚・兄弟 context 会計を含む。
- 素材 resolver の追補85/85、リング入力の追補1/1成功
  (`target/depth-b-material-tests.log`, `target/depth-b-ring-test.log`)。
- UI snapshot は全240/240成功 (`target/depth-b-ui-snapshot-final.log`)。
  同じ headless test target を直接実行したもので、製品 exe は起動していない。
  1093×614 の新規 PNG を実装担当と独立 reviewer が目視し、見出し・全項目・状態・無効の書き出しボタンが
  読め、重なりや切れがないことを確認した。546.5×307 の scroll end / header固定も検査した。
- 指示どおり `scripts/test-lib-full.ps1` は一度だけ実行。
  16490件中16485成功、5失敗、73skip、796.553秒 (10 slow, 2 leaky)、exit100。
  正本ログは `target/test-lib-full/runs/20261107T033036728Z-50edeaff/` の stderr / JUnit。
  失敗は上記の分類件数・検索索引各1件、既存 IPC 2件、既存 audio EOF 1件。
- 既存3失敗を同じ built lib で serial 単独実行
  (`target/depth-b-isolated-failures.log`): audio EOF は成功、IPC 2件は再び Windows の
  Os code5 / PermissionDenied で失敗。audio は全件負荷時の失敗を観測しただけで原因を断定しない。
  IPC は一意な名前の `CreateNamedPipe` を作る低レベル検査と fixture server 準備の両方で失敗し、
  Depth state を作る前の権限エラーである。関連コードや検査を緩めず、全件 gate 成功とは扱わない。
- 補修後の設定転送・検索索引は63/63成功 (`target/depth-b-settings-final2.log`)。
  全件での失敗5件を最終 source の全件成功へ読み替えず、補修した2箇所の限定再検査として残す。
- 最終 production source の `cargo check --offline -p mimageviewer --bin mimageviewer-core` は成功
  (`target/depth-b-check-final.log`)。workspace 全体へ引数なしの `cargo fmt` を実行し、
  `cargo fmt --check` と `python scripts/check_ui_glyphs.py` (危険な文字0) も成功。
- `scripts/build-dev.ps1 -PreserveRuntime` を一度実行し exit0。
  core (3分51秒) と remote / EPUB PDF / MCP / music-video の4 companion を normal feature set の
  `dev-runtime` へ用意した。portable feature は追加していない。VCRT PE 検査も runtime4 / PE5 成功。
  `target/depth-b-build-dev.log` に完了記録を残し、成果物は起動していない。

確認用の起動は、インストール済み・トレイ常駐の mImageViewer を終了してから、利用者自身で実行する。
通常の `%APPDATA%\mimageviewer` プロファイルを使うため、実際の設定・データが更新され得る。
単一起動 mutex は通常版と共通である。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

静止画で D / 上部3Dボタン、未導入案内と導入後の手動入場、素材準備中の2Dから3Dへの切替、
パネル全項目と I / Tab、ポインタ側から覗き込む方向とパネル上の追従停止を確認する。
通常 wheel のページ移動・Ctrl+wheel の zoom停止・90度回転での色と深度の一致・OFF後の2D変換保持、
360から3Dへの切替とV拒否、動画・見開き・連結・F11/F12・全画面終了での終了と非復活も確認対象。
自動の動き中は idle health の対象外。実モデルの品質・速度と操作感の検収は利用者・ClaudeCodeへ引き継ぐ。

#### 段 B 独立レビュー P1〜P2 の対応 (2026-11-07、基点 `dee2fd1ec`)

ClaudeCode が正当と判断した別 session の独立指摘3件に対応する未コミット差分。
先の実装記録・コードレビュー ACCEPT はこの追加指摘の解消証拠とは分ける。

- P1: `fs_zoom_mode_context_ok` は Z / 非キー操作の入場判定のまま使い、保存済み2Dズームの
  寿命判定とは分けた。二つの blocked caller は共通の `fs_zoom_reset_for_blocked_context` を通り、
  Depth だけは既存の transient reset で照準・release待ち・edgeを畳む。確定 active / factor は残す。
  Z edge の消費と held level の追跡は維持し、Depth から新しい照準やズーム解除を始めない。
  Panorama 等の対象外 context は従来どおり full reset とする。
- P2: `activate_mounted_as_independent_detached` を `commit_viewer_presentation` に接続した。
  live / materialized の両 fork が通る `split_current_context_preserving_main_grid` の入口で、
  context を移す前に Depth だけを共通 setter で退役する。要求取消と自 context の callback slot 除去を
  Drop の既存契約で完了し、Panorama の state / intent 移譲は変えない。
- P2 の配線検査: 実 Z press / release → D → idle / 拒否される Z → D を
  `handle_fs_key_input` へ流す。継続検査は video / spread / continuous / 後判明 animation の各ケースを
  実 `render_fullscreen_viewport` から通し、still 帰還後の pass でも復活しないことを検査する。
  headless immediate viewport callback は専用 thread、資源 slot は各ケース専有 renderer に隔離する。
  実 independent activation と両 fork では、推論中は cancel token、Ready は slot 除去と LRU 破棄、
  逆の phase に置いた兄弟の要求・資源不変を相補的に検査する。
- 素材は実 resolver で、complete な2D下地があっても `AnnotationBake` の間は `NotYet` とする検査を追加。
  source-free Native しか残らない fixture では、元 managed allocation が消えていることを検査し、
  既存の `ensure_original_material_for_entry(MaterialOnly)` → `fs_pending` → `poll_prefetch` → final 完成を通す。
  Native の実 payload が選択されたとき、下層の generic `IncompleteComposite` より `NativeOnly` の理由を
  優先するよう素材契約を揃えた。OriginalHold / PageTransition / AnnotationBake の優先と2D選択は維持。
  新しい素材 producer・再ロード経路は作らない。
- GPU は製品 callback / WGSL を使い、非一様な二つの深度面、90度回転、非中央の切り取り uv、
  逆向きの変位と、両端を越える大きな変位での crop UV clamp を固定画素の期待値で検査する。
  白 / 市松の透明・半透明・不透明部分と余白は、
  実際の `paint_image_underlay` と通常2D texture mesh の描画結果に照合する。

**2D 状態の兄弟経路監査**:

| 箇所 | 結論 |
| --- | --- |
| 毎 frame の Z handler、非キー Z action | 今回の admission / 保存寿命分離の対象。Depth の確定状態だけ保持する |
| draw の fit / zoom / pan / 自由回転 | Depth が先に選ばれ、描画引数だけ fit / zero にする。保存値を書き換えない |
| key / touch / gamepad / middle-drag の zoom owner | `FsZoomOwner::Depth` は無操作。保存状態を reset しない |
| wheel | Depth の Ctrl+wheel は Z 分岐より前に消費。通常 wheel はページ移動の既存契約 |
| navigator / drag / double-click | Depth は navigator 不許可、transform変更は無操作。依頼18でclick採用を分離し、保存transformを参照せずページ送りを許可。drag origin の破棄は一時状態だけ |
| fit cycle | Depth は入口で拒否し、保存 fit や2D transformを変更しない |
| `reset_fullscreen_fit_transform` / `reset_continuous_reading_transform` | 明示 fit / reading-mode 切替の既存境界。Depth中の毎 frame reset ではない |
| `open_fullscreen` の変換初期化 | ページ変更の既存境界。同一ページ D 往復の per-frame 処理ではない |

**production presentation 代入の全件監査** (テスト・scenario fixture は除外):

| 箇所 | 扱いと理由 |
| --- | --- |
| `app/depth_view.rs::commit_viewer_presentation` | 生きた presentation を変更する唯一の直接代入。旧 Depth を先に退役する |
| `viewer_context_registry.rs::activate_mounted_as_independent_detached` | 今回 bypass を除き共通 commit へ接続。ほかの live transition も同じ commit を使用 |
| `app.rs` の App initializer / `viewer_session.rs::ViewerSession::default` | 新しい空 context の初期値で、退役対象が無い。commit は通さない |
| `ViewerSession::swap_with_mounted` (registry の mount / fork) | context 所有の既存値を exclusive owner とともに運ぶ操作。別 owner の transition として再 commit しない。fork の Depth は搬送前に退役する |
| `cfg(test)` 内と `test_script.rs` の scenario fixture | 実行中の製品 presentation の変更ではないので監査対象外 |

detached §11 に BA-7 の退役順序と BA-4 の既存 identity / commit 境界を記録する。
追加 modal・rollback state・bool は不要で、表示時 guard ではなく公開・移譲元で不適格な組合せを消す。
既存 IPC 2件は利用者から「sandbox 外では成功」との連絡を受けた。本対応で IPC の変更・再検査はしない。
全件 gate は依頼どおり実行せず ClaudeCode に引き継ぐ。

今回の変更は11ファイル: `src/ui_fullscreen.rs`, `src/app/viewer_context_registry.rs`,
`src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`, `src/ui_fullscreen/page_turn.rs`,
`src/depth_parallax_wgpu.rs`, `src/depth_estimation.rs`, `src/addon.rs`,
`src/ui_dialogs/depth_addon.rs`, `docs/depth-parallax-plan.md`, `docs/detached-rework-plan.md`。
新規 test は9件（実キー1・実viewer pass1に4条件・所有境界3・素材2・GPU2）。
`depth_estimation.rs` の cancel token accessor、addon / DepthSetup の status 注入は `cfg(test)` だけで、
実モデルや通常 profile を用いず実 request / entry の契約を検査するための fixture とする。

実行結果（全 cargo / build / test に `MSBUILDDISABLENODEREUSE=1`）:

- 深度関連97/97成功、12.545秒 (`target/depth-b-review-depth2.log`)。
  初回は test fixture API の公開範囲2箇所で compile error を検出し、test 専用委譲と公開 enum の照合へ修正した。
- 既存の panorama / pano / exclusive / keymap / fs_zoom / viewer_context_registry は、成功した depth 選択を
  除いて430/430成功、19.392秒 (`target/depth-b-review-related.log`)。
  この二つの選択の計527件を成功とし、重複する後続のGPU限定実行は件数へ加算しない。
- production core の `cargo check --offline -p mimageviewer --bin mimageviewer-core` は成功、42.94秒
  (`target/depth-b-review-check.log`)。
- workspace 全体へ引数なしの `cargo fmt` を実行。UI glyph checker は危険な文字0
  (`target/depth-b-review-glyphs.log`)、`cargo fmt --check` と `git diff --check` も成功。
- 補強した crop overshoot は該当 GPU test だけ再実行し1/1成功、1.122秒
  (`target/depth-b-review-crop-clamp.log`)。uv端の半 texel 補間値を固定し、全画像端への誤った clamp では通らない。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は11ファイルと上記の実行ログ・文書を照合し ACCEPT、
  P1/P2 残件なし。BA-7 / BA-4 の構造変更であることも再確認した。全件 gate / 実機検収は対象外。
- `scripts/build-dev.ps1 -PreserveRuntime` は exit0。core 2分22秒、付属4バイナリも成功し、
  VCRT PE 検査は runtime4 / PE5 成功 (`target/depth-b-review-build-dev.log`)。
  normal feature set で確認用 binary を更新し、製品バイナリ・実UIは起動していない。
  利用者の実機確認は Z確定 → D → D の拡大表示復帰、Depth中のZ拒否、D中のF12 / 独立窓への切替と
  帰還後の非復活、360表示の従来のZ解除とfork移譲を対象とする。起動方法と実profileの注意は上記を使う。
  HEAD `dee2fd1ec` を維持し、stage / commit は行わない。

#### 段 B 再レビュー P1: 休止 Z の消費側 (2026-11-07、基点 `551040002`)

Z確定 → D では2Dの保存状態を残すが、その値をタッチ認識へ直接渡すと Depth の通常ページ送りまで停止する。
保存と実効の区別を全 consumer の境界で揃える。`fs_zoom_mode_engaged()` は保存 active / aiming の述語として残し、
`fs_zoom_mode_effective()` は `fs_zoom_owner() == FsZoomOwner::ZoomMode` を返す。
active / aiming の runtime consumer もそれぞれ owner を通す helper を使う。
owner の優先順（Depth → 保存 Z → Panorama → Analysis → Image）は変更しない。
新しい状態・bool・復元用snapshotは追加せず、Depth の入口で保存ズームを捨てる対処もしない。
通常2Dと360では旧 raw 判定と実効判定が一致する既存の順序を維持する。

以下は **基点 `551040002` の行番号**。`src/` の全 `fs_zoom_active` / `fs_zoom_mode_engaged()` と
照準の兄弟 consumer を検索し、テスト・コメント・宣言・書込みを分けて全てを分類した。

| 基点の消費箇所 | 分類・対応 |
| --- | --- |
| `ui_fullscreen.rs:14704-14705` 保存述語 | raw維持。保存 state 自体を表す |
| 同 `14760` toggle action | raw維持。admission を通った後の既存 Z state の切替であり、Depth は到達しない |
| 同 `14862,14872,14882` key state machine | raw維持。admission後の確定／照準・held repaint。対象外の transient reset と level latch は既存どおり |
| 同 `15186` owner解決 | raw維持。Depth を先に選び、Zの保存値から実効 owner を一度だけ決める。helper と循環しない |
| `app.rs:18247-18249,21656-21657` field/default | 保存値の宣言・初期化。変更なし |
| `viewer_context_registry.rs:1787,3047,3477,3810,4199,4429` field/default/extract/swap/move | context所有の保存・搬送。active/aiming を保持し、描画や入力の可否を決めないので変更なし |
| `ui_fullscreen.rs:14967` 単ページZ描画 | 実効 active。保存中の Depth からZ描画を導かない |
| 同 `15159` pan clamp | 実効 owner。Z中は通常pan clampを適用しない既存の分岐 |
| 同 `15648,15670` gamepad zoom/pan | 実効 owner。占有モード拒否は既存どおり、その後のZ判定も統一 |
| 同 `22224` similar preview | 実効 owner。既存の占有モード拒否は維持 |
| 同 `25131` info panel lock適格性 | 実効 owner。表示幅の確保と実描画に同じ値を渡す |
| 同 `28651` 単ページのZ描画分岐 | 実効 owner。重複するDepth専用guardは除く |
| 同 `29343` view trim panel | 実効 owner。保存Zをパネル抑止へ使わない。Depth固有の適格性は既存 owner が守る |
| 同 `29376` metadata panel | 実効 owner。パネルの既存占有規則は維持 |
| 同 `29404` callout/touch handles | 実効 owner。共通 chrome 入力へ渡す |
| 同 `30699` page-turn context blocker | 実効 owner。Depth は保存Zでなく既存の占有モード判定で除外 |
| 同 `32171` PDF display demand | 実効 owner。休止Zを単ページZの解像度需要と誤認しない |
| 同 `33618` cursor visibility | 実効 aiming。休止した照準状態で ZoomAiming のcursor非表示を作らない |
| 同 `38222` navigator current zoom | 実効 owner。Zの倍率から通常倍率へ換算するかの判定 |
| 同 `38301` navigator adopt zoom | 実効 owner。実際のZ操作だけが保存値を通常panへ採用しresetする |
| 同 `38884` navigator gesture | 実効 owner。Depthのnavigator入場拒否は維持 |
| 同 `39915` side panel chrome | 実効 owner。表示・hit判定の共通入力 |
| 同 `39988` panel hit領域 | 実効 owner。保存Zでpanel上のpointer判定を変えない |
| 同 `40092` adjustment hover | 実効 owner。Depth等の別の抑止条件は維持 |
| 同 `40317` touch recognition | 実効 owner。本件の修正境界。tap/pinch driver を保存Zで停止しない |
| 同 `40786-40787` wheel診断 | 実効 active/aiming。実際のwheel処理で使う値を記録する |
| 同 `40891-40904` wheel分類 | 実効 active/aiming。Depthの通常wheel=ページ移動、Ctrl+wheel拒否は維持 |
| 同 `41082,41088` wheel適用 | 実効 active/aiming。重複するDepth専用guardを除き、保存倍率を動かさない |
| 同 `41396` wheel後のclick用transform | 実効 owner。通常描画のズーム投影だけを計算する |
| 同 `50546` spread clip | 実効 owner。Zの表示クリップかの判定 |
| 同 `50592` spread Z入口 | 実効 owner。保存Zから専用solveを開始しない |
| 同 `50615,50632,50647` spread solve/placement/gap | 実効 active。確定状態を描画へ反映するかの判定 |
| `app.rs:52810` adjustment panel reachability | 実効 owner。描画側との適格性を揃える |
| `ui_fullscreen/page_turn.rs:1817` animation適格性 | 実効 owner。Depth自体の紙めくり除外は既存の占有判定で維持 |
| raw flag の reset/代入 | state machine/lifecycleの書込みでありruntime consumerではない。変更なし |
| `cfg(test)` の直接設定・assert | fixtureと保存値の検査。runtime 判定へ置換しない |

**派生する入力・表示の確認（依頼18で訂正）**: 当初マウス canvas click を占有 owner 判定で無操作にしていたが、
Depthでも通常ページ送りを許可するのが正しい。タップ領域も Depth の通常ページ送りを許可し、360/Analysis を除く既存規則のまま。
初回タッチヘルプの開始／終了と上部バーのtouch target登録は `40317` の認識可否から派生するため、同じ実効値で揃う。
上部バーの表示自体には保存Zの直接consumerは無い。navigatorのDepth入場拒否とパネル固有の占有規則も変更しない。

回帰は実 Z press/release → D → Iでパネルを隠す → child viewer の egui-winit と同じ完全な
Touch/Pointer signature（Start・移動・primary press、End・primary release・PointerGone）→ 実 PageSide 移動 → D を通す。
Z未使用のDepthと移動先が一致し、移動中は実効ownerがDepth、解除後は保存active/factorを持つZoomModeへ戻ることを検査する。
別の owner matrix / cursor 回帰で、保存値と実効値の違い、および2D/360の既存優先順を固定する。
全件gateは実行せず、ClaudeCodeが使用中の `target/test-lib-full/` は参照・変更しない。
変更は7ファイル: `src/ui_fullscreen.rs`, `src/app.rs`, `src/ui_fullscreen/page_turn.rs`,
`src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`, `docs/depth-parallax-plan.md`,
`docs/display-pipeline.md`。状態field・keymap・通常2D/360の仕様は変更しない。

検証記録（全cargo/build/testに `MSBUILDDISABLENODEREUSE=1`）:

- 新規回帰2件を含む限定670/670成功、45.305秒、exit0 (`target/depth-b-touch-related.log`)。
  選択は `depth|panorama|pano|fs_zoom|touch|keymap|fs_cursor`。先の狭い選択で通ったmatrixは重複加算しない。
  初回はfixtureの借用順を修正し、次の狭い選択はmatrix成功・touch baseline失敗だった。
  touch fixtureの不完全なイベント列を、既存correlationが受理する完全signatureへ直し、
  時刻・App frame番号も進めた後に上記の全選択が成功した。製品の相関処理を緩めていない。
- `cargo check --offline -p mimageviewer --bin mimageviewer-core` 成功、45.59秒
  (`target/depth-b-touch-check.log`)。追加のsource変更はその後もtest fixtureだけである。
- workspace全体へ引数なしの `cargo fmt` を実行し、最終 `cargo fmt --check` 成功
  (`target/depth-b-touch-fmt.log`)。UI glyph checkerは危険な文字0 (`target/depth-b-touch-glyphs.log`)。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は最終fixture・670件の実行ログ・本分類表まで照合して
  ACCEPT、P1/P2残件なし。
- `scripts/build-dev.ps1 -PreserveRuntime` は exit0。normal feature set の core は2分08秒、
  付属4バイナリも成功。VCRT PE検査は runtime4 / PE5 成功 (`target/depth-b-touch-build-dev.log`)。
  確認用binaryは起動していない。実機では Z確定 → D → タッチのページ送り → D の拡大表示復帰と、
  通常2D / 360の従来操作を利用者に確認してもらう。起動方法・実profileの注意は上記を使う。
  HEAD `551040002` を維持し、stage/commit・製品起動はしていない。
- 作業制約の逸脱: reviewerがログ名探索で `rg --files target -g 'depth-b-touch*' -g '!test-lib-full/**'`
  を一度実行した。禁止領域のディレクトリ走査を完全には除外できないため「一切アクセスなし」とは扱わない。
  `target/test-lib-full/` のファイル内容は未読・未変更。利用者へ報告し、以後は指定済みログの直接参照だけに限定した。

#### 段 B 実機フィードバック: 画像上の待機表示 (2026-11-07、基点 `acc937311`)

利用者から「Dを押してから動き始めるまで反応がない」との実機報告を受けた追加。
§5.4の非モーダル箱を実装し、既存の取消・ページ送り・要求ownerは維持する。

- `app/depth_view.rs::depth_wait_status` が source phase と runtime snapshot から準備／計算中を一度だけ決め、
  パネルと箱が共有する。Ready / Failed / Depth以外はNone。既存の失敗文字列と通知は変更しない。
- `DepthViewState.wait_started_at` は常に存在する提示時計であり、新しいpending状態や生存sentinelではない。
  Ready / Failed後の新しい待ちだけをresetし、NotYet → 読み戻し → 推論、待ち中の別keyは継続する。
  既存 `animation_expansion_progress_visible` と同じ150ms gateを使う。
- `draw_feedback_toast` のviewer共通経路で、source採用とcurrent panel矩形の公開後に描く。
  パネルを隠しても表示し、表示中のpanelだけを避ける。上160pt／下48ptと現在の画像領域を使ってchromeを避ける。
  gate前は到達用wake、描画した箱だけ33ms再描画とし、hidden/minimized/terminalでは箱由来の予約を止める。
- `ui_depth_wait.rs` は状態・I/O・repaintを持たない純描画。アニメーション展開中表示に揃えた色・角丸・17pt見出し・
  細いバーと、実効割当の取消captionを描く。未割当captionは省略し、狭幅は折返す。
- 実Ready採用までのpass回帰で、既存Depth shader成功枝が素材traceを空にしてSingle layoutだけを返す不整合を検出した。
  `layout_rebound=true` / `sources_rebound=false` の既存debug assertionが失敗したため、assertを維持したまま、
  同じ成功境界で実際に描画したresourceを同じNavigation occurrenceのLive sourceとして公開するよう補修した。
  LegacyRenderedとDepthのnavigator／紙めくり禁止は維持し、通常2D／360の描画経路は変更しない。
  独立reviewerもこの素材／layoutの公表境界の構造修正に同意した。
- 初回fixtureのUI textures deltaの明示破棄と、raw textureではなく正本final sourceを使う準備を揃えた。
  Readyの回帰はcompleted pending → 実advance → R16 upload／Ready採用まで同passで通し、直後に箱が消えることを検査する。
  パネルとの非交差・上部／下部の予約帯・1093×614内のcontainsも実viewer出力で検査する。
- snapshotは `depth_wait_indicator_small_dark.png` を1枚追加。生成は該当testだけ `UPDATE_SNAPSHOTS=1`。
  既存534枚を事前SHA-256と照合し変更0。実装担当と独立reviewerがPNGを目視し、文字・取消行・バーに欠けや重なりがなく、
  上部表示と右パネルを避けて画像領域内に収まることを確認した（snapshotは模擬chrome、実placementは上記pass回帰で検査）。

変更は11ファイル: `src/app/depth_view.rs`, `src/depth_parallax.rs`, `src/ui_fullscreen.rs`, `src/lib.rs`,
新規 `src/ui_depth_wait.rs`, `src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`,
`tests/ui_snapshot.rs`, 新規 `tests/snapshots/depth_wait_indicator_small_dark.png`,
`docs/depth-parallax-plan.md`, `docs/ui-responsiveness.md`。

検証（全cargo/build/testに `MSBUILDDISABLENODEREUSE=1`）:

- libの追加9回帰を含む深度＋既存animation gateの限定110/110成功、14.056秒
  (`target/depth-wait-verified.log`)。初回のfixture修正と公表漏れの修正前失敗は別ログに残し、最終成功と区別する。
- 新規snapshot生成1/1成功、1.69秒 (`target/depth-wait-snapshot-generate.log`)。
  UPDATEなしの新規箱・既存3Dパネル・従来page waitの関連4/4比較も成功、2.12秒
  (`target/depth-wait-snapshot-compare.log`)。新規testはlib9件とsnapshot1件の計10件。
- core check成功、48.21秒 (`target/depth-wait-check.log`)、workspace全体の `cargo fmt` / `cargo fmt --check` 成功、
  UI glyph checkerは危険な文字0 (`target/depth-wait-glyphs.log`)。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は最終source・文書・110件ログ・新PNGを照合し ACCEPT、P1/P2なし。
- `scripts/build-dev.ps1 -PreserveRuntime` は起動中の確認用core（PID85540）を検出し、上書き前にexit1で停止
  (`target/depth-wait-build-dev.log`)。利用者のプロセスを終了せず、確認用binaryは今回の変更へ更新できていない。
  起動中のcore終了後に再buildが必要。コンパイル失敗とは区別する。

全件gateは実行していない。stage/commit・製品バイナリの起動、通常profileの設定変更もしていない。

#### 段 B サブ PC 実機確認（依頼18）への対応 (2026-11-07)

観測者はサブ PC 担当。正本は `C:/home/mimageviewer-subpc/depth-b/results/notes.md`。
修正前に確認した原因経路・検証方針を以下に記録する。

- クリック: `plan_fs_canvas_click` が `fs_exclusive_view_active(...).is_some()` で
  Depth の通常ページ送りまで拒否し、採用側も Depth の transform 停止分岐内で click action を捨てていた。
  360° のドラッグ所有と Depth の fit 固定を混同した分類が原因。§10 の過去の canvas 無操作の記録は誤りで、
  transform の変更停止と通常ページ送りを分離する。保存済み2D transform は判定にも使わない。
- 動き欄: egui 0.37 の ComboBox popup と親パネルがともに Foreground。親へのクリック／slider drag による
  Area の前面化と、保持された popup の相対順序の組合せで popup が親の背面に残る経路を確認。
  実 panel painter の「上下選択→強さ5%へdrag→再開」で、押下／解放を同じ RawInput に入れると、
  再開した左右の行の最前面layerが親パネルになることを修正前テストで再現した。
  開いている ComboBox の実 response から popup IDを導き、`set_sublayer` で親子関係を登録する。
  親とpopupのForegroundを維持し、前面化の順序競合を所有境界で除く。guard・retry・repaintは加えない。
  サブPCのpress/releaseのframe分割はログにないので、実機でも同じ分割だったとは断定しない。
- 導入確認: `toggle_depth_parallax` → `request_depth_addon_setup` の Checking は呼出元を持たず、
  `poll_depth_setup` が scan 完了の installed を常に Done へ変えていたため、起動ごとに再度 D が必要だった。
  確認元を Checking の typed state に持ち、D／上部ボタン由来の installed 完了だけ、同じ画像と入口条件を再照合して入る。
  環境設定・download 完了からは入らない。外部の画像変更経路も照合対象に含める。

動き欄の調査範囲は、製品panel／viewer描画・toggle、既知menuのclose、main modal遷移時のpopup close、
wheel消費と、egui 0.37 の ComboBox／Popup／Area／Memory／ScrollArea。
`vendor/egui/src/containers/combo_box.rs` の popup ID は response IDの `with("popup")`、
`containers/popup.rs` の menu Area はForeground、`containers/area.rs` は親へのpress／dragで前面要求を出し、
`memory/mod.rs::Areas::end_pass` は同一順序の相対関係を保持する。
`Context::set_sublayer` は同じOrderの子を親の直上に置く契約なので、この固定panel／menuの所有関係に適用する。
popup memoryはviewport単位だが、独自のdisable latch／popup state resetを要する証拠はなかった。
既存のwheel消費は入力deltaだけを変更し、modal遷移のcloseはdialog開始境界だけなので、今回の永続する背面化の原因ではない。

クリック以外も入口と採用先を確認した（行番号は依頼18の基点）。

| 箇所／操作 | 分類と確認結果 |
|---|---|
| `plan_fs_canvas_click` / `apply_fs_wheel_frame_proposal` 左クリック | 360°の意味に依存。Depthはfitの実効transformでページ送りを分類し、transform適用と独立して採用する |
| 同 double-click | 通常2Dのtransform resetは維持。Depthでは保存transformを参照／resetせず、通常のpage-side clickとして扱う。360°は既存pose resetへ |
| `handle_middle_drag_zoom` 中ドラッグ、Ctrl+左ドラッグ／pan | 占有。Depthはfit固定なので停止。短い中クリックの割当実行とは独立 |
| `40622` 中短クリック → `viewer_chrome::dispatch_viewer_middle_short_click` | 入力slotの意味。Depthの一律拒否なし。既定は未割当。共有mouse/ring resolverへ渡す |
| `35826,37235` Extra1/Extra2／browser back-forward → `apply_mouse_back_forward_button` | 入力slotの意味。Depthの一律拒否なし。既定のfolder履歴／従来互換の移動を保持 |
| `recognize_viewer_right_drag_frame` / `apply_fs_secondary_input` 右短クリック・ring・gesture | 入力slotの意味。Depth固有の一律拒否なし。右短クリック既定の全画面終了は維持 |
| `app/gamepad_input.rs::apply_ring_action` 共通mouse/ring採用 | DepthでZ/補正と衝突する操作だけ止め、metadata割当はDepthパネル切替へ。ページ移動／表示終了の割当は止めない |
| wheel / Ctrl+wheel | 通常wheelはページ送り、Ctrl+wheelはfit固定のため停止。今回変更なし |
| front pointer所有・popup／パネル／seek矩形・focus復帰抑制 | モードに依存しないchrome所有。Depthパネルを触った入力を背面のページ送りに流さない規則は維持 |

Checking のモーダル外経路も確認した。`app/startup_ops.rs::poll_activation_open_paths` →
`activation_open_precheck` → `document_open_modal_admission_blocked` は Depth setup を含めず、
関連付けopenの解決・採用が進み得る。`remote_ipc/ui.rs::service_remote_session_requests` も
別に acquisition を開始し、viewer close barrierへ進む。したがって確認完了時には画像の同一性に加えて
`remote_session_blocks_local_control()` を照合する。Remoteが取得中でまだ旧画像が残る場合も入場させない。
clipboardからの新規取込は既存の fullscreen／common modal の受付条件で止まる。
新しい外部openの保留機構や再試行は足さず、照合不一致なら一回の入場要求を終える。

再現テストは、クリックが実child viewerへ届く経路（1920×1080、Zの有無×パネル表示の有無の4条件で左右往復）、
導入確認の8件（実D・小さなローカルpackの非同期scan、未導入／破損、環境設定、画像／世代／context変更、
入口再拒否、Remote取得中、案内移行後の非継続）、実panelの選択→slider→再展開1件
（1920×1080／1093×614 × 同一／別frameの4条件）を追加した。
マウスfixtureは初回passのfocus取得を落ち着いた前面状態へ揃える。既存の300ms focus復帰抑制は変更しない。
独立コードレビューで、click採用の分離・scanのtyped origin・popup親子layerの修正が所有境界の変更であり、
症状だけを隠すguardではないことを確認した。最終の自動検証結果は以下に記録する。

| 検証 | 結果 |
|---|---|
| `MSBUILDDISABLENODEREUSE=1 cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_\|panorama\|pano\|canvas_double_click/)' --test-threads 4` | **215/215 成功**（追加10件を含む）。`target/depth-subpc18-final.log`。全件は依頼どおり実行しない |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core`（同env） | 成功。`target/depth-subpc18-check.log` |
| `cargo test --offline -p mimageviewer --test ui_snapshot depth_`（同env、更新modeなし） | **3/3 成功**。通常panel・1093×614の全control到達性・待機表示。`target/depth-subpc18-ui.log`。既存PNG変更なし、追加PNGなし |
| `cargo fmt`（引数なし）／`cargo fmt --check`／`python scripts/check_ui_glyphs.py` | 成功、危険glyph 0件 |
| `scripts/build-dev.ps1 -PreserveRuntime`（同env、offline） | 成功。core／remote／EPUB worker／MCP／music-videoを準備。core更新日時 2026-11-07 18:20:54、391,422,464 bytes。製品は起動していない |

修正前の再現と途中のfixture補正は `target/depth-subpc18-focused.log`／`depth-subpc18-regression.log` に残す。
初回のmotionテストでは Harness のevent queueがpress/releaseを別frameへ分けていたため成功していた。
RawInputへの同時注入へ直した版で背面化を再現してから製品を修正し、最終215件で回復を確認した。
commit／stageは行わず、全件台帳用ディレクトリには触れていない。
実機での最終確認は利用者側へ引き継ぐ（D後の左右click、上下選択→強さ5%→再選択、導入済み起動直後の最初のD）。

### 段 C の実装記録 (2026-11-07)

- `depth_export.rs` に凍結した要求、形式・寸法・フレーム時刻、要求専用channelとcancelを
  持つ `DepthExportJob` を追加した。設定画面から進行・終端画面までは App 共通の単一
  `Operation` が所有し、local-modal lease は開始前取得→終端画面の閉鎖まで保持する。
  共通modalとdocument-open admissionに登録し、長い処理をモーダルにして割込みの組合せを除いた。
- パネルの書き出しはReadyかつ現在の描画snapshotが一致するときだけ有効。
  設定画面は形式・元テクスチャを超えない長辺・自動4動作・MP4のループ数だけを持つ。
  720px未満の素材では選択肢が空にならないよう元の長辺を1つ表示する。
  強さ・周期・焦点・整形は現パネル設定を凍結し、別の編集項目は作らない。
  パネルのクリックはframe内の意図だけを返し、実際の3D描画後に素材・ページ・一覧世代を
  照合して設定画面へ入る。保存時も最新のReady素材と描画snapshotを一組で読み、
  設定画面中のresizeや背景の素材更新を古い市松寸法／素材と混ぜない。
  worker要求を作った後はその一組を固定する。要求を作れるまで保存操作は理由付きで無効。
- `depth_parallax_wgpu/export.rs` が専用pipeline / uniform / bind group / readbackを所有する。
  3Dの描画は画面と同じ `DEPTH_SHADER`。下地専用のshaderへ黒・白・市松を描いた後、
  既存と同じpremultiplied blendを行うことでgamma空間の `rgb + bg*(1-a)` とalpha=1を得る。
  3Dの描き方は複製せず、画面用uniformを共有しない。市松の升目は画面の8logical pxを
  出力寸法比で縮尺し、表示画像の左上を原点にする。画面の `paint_quad()` と同じく、
  90度回転は画像sampleへ適用し、市松の原点は表示矩形の左上に保つ。
- GIFはimage 0.25のNeuQuantと無限ループ、APNGはpng 0.18のanimated frame。
  GIFは累積時刻を10ms単位、APNGは1ms単位へ丸めて各frameへ差分配分する。
  既定3秒はGIF45frame合計3000ms、APNG45frame合計3000msとなる。
  任意の小数周期も形式が表せる時刻精度で累積端点を一致させる。
- `mvexport::VideoOnlyMp4Writer` / `Mp4Muxer::new_video_only` を追加し、H.264選択・
  BT.709変換・faststart・`PartialOutput`を再利用。音楽ビデオとRemoteの入口は維持する。
  新しいoffline専用encoder入口ではOpenH264のframe skipを無効化し、全frame数を保証する。
  GIF/APNGも同じ一時ファイル→公開へ統一する。取消・失敗・Dropは未完了出力を回収する。
- workerだけがGPU submit / `poll(Wait)` / encode / I/Oを実行し、進行・終端でrootを起こす。
  UIに常時repaintを追加せず、モーダル中の3D自動運動のrepaintを止める。
  進行窓×はcancel→実処理の終了待ち表示、終端画面を閉じるまでleaseを維持する。
  fullscreenの所有contextを閉じたときも同じ取消遷移を通す。別contextの終了では触れない。
  ナビゲーションの共通受付でも書き出しモーダルを照合し、同frameの遅延入力と
  スライドショーの自動送りを止める。OSの窓閉鎖は通して既存の取消境界へ渡す。
  UIはjoinしない。まれな失敗はログ＋通知だけでretry・復旧stateは足さない。

機能上の設計変更はない。小画像の元寸法選択、APNGの拡張子を`.png`とすること、下地を
先に描くpremultiplied blend、OpenH264のoffline専用設定は§7の契約を満たす実装上の具体化。
公開文書・製品ページ・privacyは指示どおり段Dに残す。
#### 段 C の検証記録

対象は `depth-parallax` の本段の未コミット差分。全Cargo / build / testに
`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`。
製品・通常profileのアプリは起動せず、GUI検査はheadless fixtureだけで行った。

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|mvexport\|export/)' --test-threads 4` | **286/286成功**、exit0。実行34.573秒（compile5分47秒）。`target/depth-c-related-final.log`。新規lib19件を含む。形式ごとのフレーム数・遅延・周期合計・寸法・loop、MP4のCFR timestamps／音声なし／BT.709／faststart、各形式の取消／Drop／既存出力保持、GPUの灰色128完全一致・半透明／黒白市松の画面照合（既存GPU helperの±1LSB）、専用uniform、lease／外部open／終了owner／resize／新素材／凍結／manual・slideshow排他、既存音楽ビデオ回帰を確認 |
| 新規snapshot2枚とgeometry/clickを限定して`UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_` | **3/3成功**、exit0。`target/depth-c-ui-update.log`。1093×614と546.5×307でスクロール末尾、固定操作行、Save／Cancel／タイトル×の実クリックを確認 |
| workspace全体へ引数なし`cargo fmt`後、UPDATEなし`cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、4.43秒。新規3と既存panel2・wait1。`target/depth-c-ui-compare.log` |
| snapshot目視／既存画像の照合 | 新規2枚は55,224＋29,457＝84,681 bytes。実装担当・親担当・独立reviewerが、文字・選択肢・ループ既定4・操作行・タイトル×・進行数に欠け／重なりがないことを目視。進行窓は自然高を維持。既存535枚のSHA-256変更0（`target/depth-c-snapshot-before.json`） |
| `cargo fmt --check` / `python scripts/check_ui_glyphs.py` / `git diff --check` | 全てexit0、危険なUI文字0。`target/depth-c-fmt-check.log`, `target/depth-c-glyphs.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、55.14秒。`target/depth-c-check-final.log` |
| `scripts/build-dev.ps1 -PreserveRuntime`（1回実行） | **成功、exit0**。normal feature setのcoreと付属4バイナリを構築し、VCRT／PE検査はruntime4・PE5成功。`target/depth-c-build-dev.log`, `target/vcrt-pe-reports/dev-runtime.json`。確認用coreは`target/dev-runtime/mimageviewer-core.exe`（391,680,000 bytes、2026-11-07 19:26 JST更新）。製品は起動していない |
| 独立コードレビュー（GPT-6.1 Sol / xhigh、別context） | **最終ACCEPT、未解決P1/P2なし**。要求凍結、frame-local受付、OS closeと兄弟context、lease、GPU、時刻、既存MP4経路、最終286/286とUI6/6のログを照合。最新PNG2枚も目視し、既存535枚不変を確認。全件gate／実機検収は別担当 |

途中の失敗と補正は最終成功と区別する。初回型検査のUI参照外し1件、test compileの
FFmpeg packet iteratorのResult展開2件を修正した。初回focusedは40件中39成功／GPU1失敗
（`target/depth-c-focused.log`）。切り取り後のpaint_rectを固定48×64と仮定したfixtureを、
実paint_rectの24×48・実originを使う比較へ直した。併せて画面の市松はquarter-turnで回転せず
表示矩形左上が原点であることを確認し、exportの下地も揃えた。最終286件で再確認済み。
UI初回は3件中2成功／geometry1失敗。自然高のWindowよりScrollAreaのinput/clip範囲が
大きく残るため、設定／終端は実viewportを確保し、短い進行表示は自然高の直接描画とした。

sourceレビューで、fullscreen終了時の取消漏れ、ページ・回転・resizeと受付順序の混合、
設定画面中の新素材、slideshowによるモーダル迂回を発見し、上記の所有境界へ接続した。
`ctx.data_mut`内のpass番号参照によるlock再入候補も、pass取得をclosure外へ移して解消した。
後者はコードでの発見で、製品の実機deadlockを観測したという意味ではない。
新規検査はlib19＋integration3＝22件。全件gateは依頼どおり実行せずClaudeCodeへ引き継ぐ。

利用者の実機確認は次を対象にする（製品は起動していない）:

- Readyの3Dパネルから設定画面を開き、MP4／GIF／APNG、動き、長辺、MP4のループ数を選んで保存する。
  マウス追従からは左右が初期値、APNGには容量の注記があることを確認する。
- 保存したGIF／APNGの繰り返しと周期、MP4の無音再生・指定ループ数、回転と透過下地の見た目を確認する。
- 進行中の取消・進行窓×・元のfullscreen窓の閉鎖で中断し、途中ファイルが残らないことを確認する。
  完了／取消画面を閉じるまでページ移動・Remote取得・外部からのopenが止まり、閉じると復帰する。
- 設定画面を小さくしたり表示窓をresizeしてから保存し、操作ボタンに届くことと市松の縮尺が合うことを確認する。

実機起動はインストール版／tray常駐版を終了してから、repository rootで
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
このnormal feature setのcoreは実利用中の`%APPDATA%\mimageviewer`を使い、設定・データを更新し得る。
single-instance mutexも通常版と共有する。段Dの公開文書・実機検収・全件gateは本段の実施済み結果に含めない。

変更ファイル（段Cのみ、stage／commitはしない）:

- 状態・描画・worker: `src/depth_export.rs`, `src/depth_export/tests.rs`,
  `src/depth_parallax_wgpu.rs`, `src/depth_parallax_wgpu/export.rs`, `src/lib.rs`。
- UI・App接続: `src/app.rs`, `src/app/depth_export.rs`, `src/app/depth_view.rs`,
  `src/app/depth_view_tests.rs`, `src/app/update_phases.rs`, `src/ui_depth_export.rs`,
  `src/ui_depth_parallax.rs`, `src/ui_fullscreen.rs`, `src/ui_fullscreen/viewer_chrome.rs`。
- MP4共用部品: `src/video/mvexport/driver.rs`, `src/video/mvexport/mod.rs`,
  `src/video/mvexport/mux.rs`, `src/video/mvexport/video_only.rs`,
  `src/video/mvexport/video_only_tests.rs`, `src/video/stream/encoder.rs`。
- UI検査: `tests/ui_snapshot.rs`, `tests/snapshots/depth_export_settings_small_dark.png`,
  `tests/snapshots/depth_export_progress_small_dark.png`。
- 文書: 本書、`docs/architecture-overview.md`, `docs/async-architecture.md`,
  `docs/ui-responsiveness.md`, `docs/music-video-export-plan.md`, `docs/detached-rework-plan.md`,
  `docs/ui-snapshot-policy.md`。

#### 段 C 独立レビュー1回目 P2: 正常終了時の一時ファイル回収 (2026-11-08)

基点は段C commit `9f38dcd85` と master merge `c4425da6d`。指摘は正当。
旧実装はspawnの`JoinHandle`を破棄し、JobのDropでcancelを立てるだけだった。
プロセスがGPU待ち／圧縮の途中で終わると、workerの`PartialOutput`がDropされる前にthreadが
消える。従来のDropテストは別の完了通知を待ってから検査しており、正常終了の回収境界を検証していなかった。

**既存経路への接続**:

- `src/lib.rs` の `run_native` 外で `DepthExportProcessOwner` を保持し、Appへは受付clientだけを渡す。
  collection／book-composition／spread-shiftと同じ、既存 `finish_run_native` cleanupへ載せた。
  Ok／Errの共通cleanup先頭でcancel→handle取出し→lock外joinを完了し、他ownerの長いdrainと
  scriptの最終`process::exit`より前に回収する。outer ownerのDropも同じ回収関数を使う。
- `App::on_exit` と AppのDropの先頭ではclientをretireし、受付を閉じて登録済みworkerへcancelを立てるだけ。
  UI callback／UI frame内でjoinしない。Job receiverが先に失われてもprocess ownerがhandleを保持する。
- 受付確認・spawn・handle登録を一つの短いmutex区間で行い、retire／drainとの間に登録漏れを作らない。
  worker本体はこのmutexに触れない。完了済みthreadのhandleは次の受付で`is_finished`を確認して破棄し、
  activeなhandleは外側の終了回収へ残す。汎用reaper・retry・待機deadlineは追加しない。
- 既存TRT reaperはpool専用で、exportの回収口にはならない。音楽ビデオは別アプリ
  (`musicvideo-editor`) のClose受付が `ConfirmExport → StoppingExport → receiver終端` を待つ個別方式。
  これらを複製／変更せず、既にある本体のprocess cleanupを利用した。

**照合した終了入口**（新しいroot Close policyは追加しない）:

| 入口 | 到達先と回収 |
| --- | --- |
| main窓×／OS Close、通常のMainWindowClose割当 | root lifecycleのClose policyが実終了を受理するとeframe `on_exit` → `run_native`復帰 → 共通cleanupでjoin |
| tray「終了」 | `tray.rs` のquit flag＋WM_CLOSE → tray常駐へのhideを迂回 → 同じon_exit／共通cleanup |
| ファイルメニュー「終了」、QuitApplication割当（gamepad／ringを含む） | `request_application_quit` → shutdown flag＋root Close → 同じ経路 |
| installerのshutdown event | `single_instance.rs` が同じshutdown flagとroot Closeへ配送 → 同じ経路 |
| fullscreenだけ閉じる／兄弟readerを閉じる | 既存のcontext照合付き取消を維持。sourceのworkerはcancel、兄弟は不変。プロセス受付は閉じず、継続利用できる。最終app終了時は残るhandleも共通cleanupで回収 |
| trayへ隠す／CloseがCancelCloseされた場合 | 実終了ではないためprocess ownerをretireしない。既存のhide／fullscreen閉鎖policyを維持 |
| Remoteのセッション終了・ローカルへ戻る | Remote／agent IPCに独立の本体Quit命令は無い。セッション退役とプロセス終了を区別。Remote稼働中の本体終了も通常のon_exit／共通cleanup |
| rendererのErr復帰／creator失敗、on_exitを通らないApp Drop | App Dropのcancelと、Ok／Err共通のouter cleanup。App生成前の失敗ではworker未起動 |
| test-scriptの正常終了 | `exit_after_run_native`より前に共通cleanupが完了 |

`--play-test` の診断用直接`process::exit`、GUI起動前の早期exit、別worker／開発CLI、
強制終了・abortは通常の3D書き出し中の終了入口に含めない。異常終了の再試行／孤児復元機構は追加しない。

**cancel確認の区切り**:

全形式で毎frameのGPU描画／読み戻しの前後にcancelを確認し、確認後は次frameへ進まない。
GIFはそのframeのNeuQuant＋圧縮、APNGはそのframeの`write_image_data`、MP4はそのframeの
BT.709変換・encoder送信・packet排出が一つの区切り。MP4 finishの入口と全形式のpublish直前にも確認する。
したがって残り全frame／全loopを処理してから止まる経路はない。既存のGPU `poll(Wait)`、
圧縮、FFmpeg／ファイルI/Oの一呼出し自体は中断できず、その現在の区切りが返るまでは待つ。
最大長辺2160の1frameを区切りとし、時間制限つきの待ち・polling sleep・retryは足していない。
正常に戻る呼出しの完了と資源破棄まで外側で回収する。GPU driver等の異常停止にまで時間上限を約束するものではない。

変更ファイル: `src/depth_export.rs`, `src/depth_export/tests.rs`, `src/app.rs`,
`src/app/depth_export.rs`, `src/lib.rs`, `docs/async-architecture.md`, 本書。
**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export::tests::depth_export_(process\|outer\|job_drop)/)' --test-threads 2` | **4/4成功**、exit0、実行6.441秒（初回compile8分19秒）。新規3件と既存Job Drop。`target/depth-c-exit-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|mvexport\|export\|root_lifecycle\|tray.*quit/)' --test-threads 4` | **303/303成功**、exit0、48.099秒。新規3件を含む終了回収、従来3D/GPU/全形式/音楽ビデオとroot lifecycle／tray回帰。先行4件はこの303件に含まれ、重複加算しない。`target/depth-c-exit-related.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/^run_native_failure::/)' --test-threads 4` | **14/14成功**、exit0、0.551秒。既存の正常／Err／scriptとcleanup順の終了境界を確認。`target/depth-c-exit-boundary.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、1分14秒。`target/depth-c-exit-check.log` |
| 最後にworkspace全体へ引数なし`cargo fmt` → `cargo fmt --check` | 成功、exit0。`target/depth-c-exit-fmt.log`。変更のないsourceで上記test結果を再利用 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方exit0、危険なUI文字0。`target/depth-c-exit-glyphs.log` |
| 独立review（Sol / xhigh、同じreview contextで継続） | **最終ACCEPT、未解決P1/P2なし**。登録と終了の直列化、UIで待たない取消、outer join、実部分ファイル6条件と303/303・14/14ログを照合。reviewerによるCargo重複実行なし |

新しい終了回収testは、GIF／APNG／MP4それぞれで本物の途中ファイルが存在する区切りまで進め、
workerをgateで停止したまま実`eframe::App::on_exit`またはApp Dropを呼ぶ（6条件）。
UI側は完了を待たずcancel／新規受付拒否となり、worker再開後は別の完了receiverを待たず
`shutdown_and_join`だけで回収を確定する。return時のworker完了・一時ファイル消失・既存保存先保持を確認した。
spawnとshutdownの競合、shutdown後に残るclientの拒否、on_exitを経由しないouter owner Dropも検査した。
全件gateは指示どおりClaudeCode担当。stage／commit／製品起動は未実施、build-devも指示により実行しない。

#### 段 C 利用者決定: SNS 投稿向けの選択肢と既定 (2026-11-08、基点 `eda886076`)

利用者が実機で書き出し・保存を確認したうえで、次の3点を採択した。SNS の名称・上限は UI に表示しない。

- 長辺に1280 pxを追加。利用者提示の X 推奨（横1280×720／縦720×1280／正方形720×720）を理由に、
  長辺1280を選べるようにする。縦横比は従来どおり保ち、元のテクスチャより拡大しない。
- 形式ごとの長辺の既定を MP4 1280／GIF 720／APNG 1080 px とし、形式を切り替えたときにも
  共通の候補解決で既定へ戻す。利用者提示の GIF 上限（X 15MB／pixiv 16MB）に対し1080では大きくなりやすいため、
  GIF は720へ下げる。SNS ごとのプリセットは仕様変更が頻繁で中身もほぼ同じになるため作らない。
- 保存完了事象を保存先と任意のバイト数にし、worker がファイル公開後に metadata を取得する。
  取得失敗は通知のサイズ省略だけとし、再試行しない。トーストと完了ダイアログで既存の MB／GB 表記を共用する。

MP4 のループ回数の既定4回は、繰り返し再生しない投稿先でも短すぎないようにする利用者決定として維持する。
fps、MP4 12Mbps、縦横比、書き出しの非永続性、既存のモーダル／終了時回収経路は変更しない。
設計との差異なし。形式の押し直しでは手動の長辺を保持し、実際に形式が変わったときだけ既定へ戻す。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export/)' --test-threads 4` | **20/20成功**、exit0、11.043秒（compile6分16秒）。`target/depth-c-sns-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | **292/292成功**、exit0、42.017秒。先行20件を含み重複加算しない。3D・全形式・音楽ビデオの回帰。`target/depth-c-sns-related.log` |
| `UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_settings_small_dark -- --exact` | **1/1成功**、exit0、3.27秒（compile7分12秒）。指定の設定PNGだけ更新。`target/depth-c-sns-snapshot-update.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、2.31秒。設定／進行の通常比較と1093×614・546.5×307での操作可視性・本文末尾への到達・実クリックを含む。`target/depth-c-sns-snapshot-compare.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、47.72秒。`target/depth-c-sns-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。sourceに追加差分なし。`target/depth-c-sns-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-sns-glyphs.log` |

長辺の全閾値の前後、元の長辺4000／1280／1279／1000／600の実UIクリックによる形式切替、
同じ形式を押し直したときの手動長辺保持を検査した。worker の実一時ファイル→rename 後の新しいサイズが
完了事象に載ること、公開後にファイルが消えた場合はサイズなしで成功すること、App の完了事象処理で
保存先と任意のサイズがトースト・完了メッセージの両方に入ることを検査した。

更新した`depth_export_settings_small_dark.png`（1093×614、56,335 bytes）を生成後に目視した。
1280 pxの選択表示、MP4、左右、ループ4回（約12.0秒）、説明文が読み取れ、保存先を選ぶ／キャンセル／
タイトルの×が画面内に収まる。元の長辺2000のfixtureなので2160は表示しない。他のPNGは変更しない。
製品の起動・実機操作は実施していない。

変更ファイル: `src/ui_depth_export.rs`, `src/depth_export.rs`, `src/depth_export/tests.rs`,
`src/app/depth_export.rs`, `tests/snapshots/depth_export_settings_small_dark.png`, 本書。
未解決事項なし。lib全件gateとbuild-devは指示どおりClaudeCode担当。stage／commit／製品起動は未実施。

#### 段 C サブ PC 依頼19: モーダルの層と初期サイズ (2026-11-08、基点 `b26f1b969`)

修正前にコードで特定した原因を記録する。実機の観測者はサブ PC 担当（RTX 4060 Ti、1920×1080、100%）。
`eda886076` での記録は `C:\home\mimageviewer-subpc\depth-c\results\notes.md` と同フォルダの
`screens/NG1*`・`screens/NG2*`。記録とNG2の前後画像を読み取り専用で照合した。

- **NG1**: `ui_depth_export::draw` は空の `Modal` と `Window` を独立した Foreground Area にしており、
  親子関係を登録していなかった。egui 0.37 の `Area::begin` は外側の backdrop のクリックでも
  `move_to_top` を実行する。同一 order の順序は `Areas::end_pass` に保存され、
  `Memory::allows_interaction` は top modal より下の Window の入力を止める。逆に backdrop より上に
  残った3Dパネルは操作できる。Windowだけ開き直しても同じ layer の順序を再利用する。
  既存 `draw_export_progress_dialog` は通常の Window だけで、この二層の問題を持たない。
  モーダルの実装は既存 `ui_dialogs::agent_integration::draw_registration_dialog` と同じく、
  backdrop を親、Window を sublayer にして一つの前面グループとして扱う方針とした。
- **NG2**: 同じ `depth_export_dialog` ID を設定340／進行170／終端220の各phaseで使い、
  `default_size` と `max_size` だけを指定していた。egui の `Resize::begin` は既存stateがあると
  defaultを再採用しないため、進行／終端から設定へ開き直しても小さい割当が残る。
  本文もその `available_height` に依存する。操作行の控除は `interact_size.y` ベースの概算で、
  実フォント＋paddingによるボタン高とseparator／item spacingの実占有を計測していない。
  `Resize` が前回の `last_content_size` まで自動拡大するため、入力で次のframeが来るたびに成長し得る。
  phaseと画面寸法からResizeの割当を毎回固定し、操作行を既存のフォント／折返し計測で予約する。
  設定／終端は本文の親領域も明示確保する。短い進行表示の外形は従来どおり本文の自然高とする。
  追加のrepaintやstate消去では隠さない。
- **注・Shift+B（2026-11-08利用者決定で更新）**: 依頼19の調査時は§6.5の占有制限をDepthにも適用していた。
  背景だけを止める描画上の必然性は無く、利用者がDepth中の切替を許可したため、今回キーとリングの両方を通す。
  キーは `is_panorama_mode_active` のみで抑止し、リングはDepth専用の拒否集合から背景切替を除く。
  Panoramaのキー抑止は維持。Panoramaのリングは基点 `58816f030` でも背景切替を許可しており、変更していない。
  下地は `try_paint_depth` へ毎描画渡し、書き出しは従来どおり開いた時の下地を保持する。

独立Sol/xhighの実装前レビューは、NG1の層所有とNG2のサイズ決定を直す構造修正として合意。
実Depthパネルとcanvasを背面にした入力、全phaseと再open、2160を含む素材、全形式、初回と後続frameのgeometry、
通常画面の明示Fits・小画面のscroll末尾到達を回帰対象にした。

実装は `ui_depth_export::draw` のbackdrop／Windowを `set_sublayer` と親の `move_to_top` でまとめた。
設定・進行・終端すべてと、main／fullscreen双方の既存描画入口に同じ処理を使う。
phaseごとに `fixed_size` でResize割当を決め直し、共通の `startup_dialog_button_rows_height` を
`pub(crate)` にしてボタンの実フォント・padding・折り返しを計測する。本文→separator→操作行の
2つのitem spacingと明示したseparator高2を控除し、設定／終端のScrollAreaには親の領域を確保した。
既存ダイアログのhelperの計算内容は変えず、guard・retry・追加repaint・Window stateの消去は足していない。

新規 `ui_depth_export_tests` の回帰:

- 実3Dパネルとcanvasがモーダルなしでは操作できるpositive controlを置き、モーダル中の外側クリックと
  パネルの「書き出し…」クリックを、同一frame／別frameのpress-release × 1093×614／1920×1080で検査。
  背面の操作が起きず、設定のGIF→APNG・保存・キャンセル、進行のキャンセル、終端の閉じる、全phaseの
  ドラッグ・タイトル×、Esc後の再openが動作する。backdrop直上がWindow、パネルはmodal未満であることも検査。
- 同じContextの設定→進行→終端→閉鎖→設定について、source長辺4000・全3形式で、最初と後続7frameの
  rect／操作行がidle・pointer移動によらず一致し、画面内に収まることを検査。
  通常の本文は `ScrollLayout::assert_height(Fits)` として不要なscrollを許さない。
- 長い保存先／失敗文は両解像度で初回からのgeometry不変・画面内包含・`Scrolls`を検査。
  既存integration到達性検査も、1093×614では明示Fits、546.5×307ではScrolls＋末尾到達とする。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/ui_depth_export::regression_tests/)' --test-threads 2` | 先行の入力／初回geometry **2/2成功**、exit0、1.352秒（compile6分20秒）。`target/depth-c-subpc-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | 最終の新規3件を含む **295/295成功**、exit0、44.811秒（compile3分38秒）。先行2件はこの中に含む。3D／GPU／全形式／音楽ビデオ／終了回収の回帰。`target/depth-c-subpc-related.log` |
| `UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_` | **3/3成功**、exit0、1.80秒（compile1分32秒）。設定／進行の2枚のみ更新。`target/depth-c-subpc-ui-update.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、1.22秒。強化したFits／小画面到達性と通常画像比較。`target/depth-c-subpc-ui-compare.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、19.03秒。`target/depth-c-subpc-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-c-subpc-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-subpc-glyphs.log` |
| 独立レビュー（Sol / xhigh、設計と同じcontextで継続） | **最終ACCEPT、未解決P1/P2なし**。最終source・295件・UI3件/6件のログとPNG2枚を照合。重複Cargo／製品起動なし |

PNGは1093×614の設定（56,335 bytes）と進行（29,457 bytes）を更新し、生成後に両方と変更前のPNGを目視比較した。
操作行・区切り線・下端の配置が高さ計算に合わせて変わる。設定のMP4／1280／左右／ループ4と本文、
進行のフレーム数・キャンセル、双方のタイトル×が画面内に収まり、文字・操作の見切れがない。他のPNGは変更しない。
サブ PC の観測と修正後のheadless検証を区別し、製品起動・修正後の実機操作は実施していない。

変更ファイル: `src/ui_depth_export.rs`, 新規`src/ui_depth_export_tests.rs`, `src/ui_dialogs/mod.rs`,
`tests/ui_snapshot.rs`, `tests/snapshots/depth_export_settings_small_dark.png`,
`tests/snapshots/depth_export_progress_small_dark.png`, 本書。
未解決のコード上の指摘なし。Shift+Bは上記の既存設計に従って維持した。
全件gateはClaudeCode担当。今回の指示に従いstage／commit／製品起動／build-devは実施していない。

#### 段 C 独立レビュー3回目: 下端のphase拡大 (2026-11-08、基点 `85062d72e`)

原因は `ui_depth_export::draw` の同一Window IDと、eguiの寸法採用の順序。
`AreaState` が保持する前回の外形で `Area::begin` が画面内への位置制限を済ませた後に、
`Window` の `fixed_size` が新しいphaseの外寸を適用する。下端の進行→終端、下端の終端を閉じて設定へ
開き直す場合は、新しい本文・操作行が初回frameだけ画面外へ伸び、次frameに位置が変わる。
先行回帰は中央でのphase遷移だけだったため、この保持位置の条件を覆っていなかった。

`current_pos` だけを渡す方法も検討したが、`Area::begin` はその後にドラッグ原点＋累積deltaで位置を
上書きするため、ドラッグを続けたまま非同期完了すると補正が失われる。
設定／終端だけ、既存 `AreaState::load` の既知寸法を新外寸へ同期し、左上位置を維持したまま
公開APIの `Areas::set_state` で同じlayerへ描画前に渡す。これで `Area::begin` 自身が新外寸を使って
ドラッグとconstrainを処理する。`set_state` は `Area::end` 自身と同じAPIであり、このpassで表示する
同じWindowのstateを更新する。初回／復元時の寸法未確定stateはeguiのsizingに任せ、作成・消去しない。
新しい位置stateは持たず、default_pos・ドラッグ・constrainを維持する。自然高の進行表示には固定割当170を
外形として使わず、下端へ移動した進行窓を不要に押し上げない。追加repaintは行わない。
この方針は独立Sol/xhighの実装前レビューでも確認した。

Shift+Bの注の指示元を利用者から設計担当（ClaudeCode）へ訂正した。利用者への確認中のため挙動は変更しない。
回帰は1093×614の同一Contextで、実pointer入力で進行窓を下端へドラッグし、解放後と押したままの両方で完了へ遷移する。
さらに下端の終端を実ボタンで閉じ、設定を再表示する。`Harness::step` を1回だけ実行して初回の矩形を採り、
初回＋後続5frameでWindowと操作行の画面内包含と位置不変を確認する。進行窓の下端位置が不要に上がらないことも検査する。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| 修正前 `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export_bottom_drag/)' --test-threads 1` | 新規回帰 **1/1失敗（意図したred）**、exit100、0.294秒（compile3分36秒）。初回rectが`[[337,481],[757,701]]`となり画面高さ614を超える。`target/depth-c-bottom-red.log` |
| 同じ限定コマンド、修正後（ドラッグ継続中の条件も追加） | **1/1成功**、exit0、0.466秒（compile3分41秒）。`target/depth-c-bottom-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | **296/296成功**、exit0、45.951秒。新規回帰1件と従来の3D／モーダル／各形式／終了回収を含む。先行1件はこの中に含み重複加算しない。`target/depth-c-bottom-related.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、1.94秒（compile1分31秒）。通常の設定／進行PNGと小画面到達性は不変。PNG更新なし。`target/depth-c-bottom-ui.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、18.14秒。`target/depth-c-bottom-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-c-bottom-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-bottom-glyphs.log` |
| 独立レビュー（Sol / xhigh、実装前と同じcontextで継続） | **最終ACCEPT、未解決P1/P2なし**。最終差分・red／green・296件・UI6件と最終検査の証拠を照合。重複Cargo／製品起動なし |

変更ファイルは `src/ui_depth_export.rs`, `src/ui_depth_export_tests.rs`, 本書の3本。
Shift+Bの挙動、PNG、他のダイアログの実装は変更していない。
stage／commit／製品起動／build-devは指示どおり実施していない。全件gateはClaudeCode担当。

#### 段 C 追補: Depth 中の背景切替 (2026-11-08、基点 `58816f030`)

利用者決定に従い `handle_fs_key_input_with_operation` の背景キーの抑止を
`is_panorama_mode_active` へ限定し、`apply_ring_action` のDepth拒否集合から
`ImageBackgroundCycle` を外した。他の占有制限は変更していない。
**前提との差**: Panoramaのキーは従来どおり停止するが、リングは基点でも停止していなかった。
「Panoramaの既存挙動を変えない」を優先し、この差を回帰テストに明示した。

素材・下地の監査結果:

- 通常の背景変更は `fs_bg_style` (`ui_fullscreen.rs`:49784付近) が現在値から下地を作り、
  静止画描画:28778付近 → `draw_fs_image_with_receipt` → `try_paint_depth`
  (`app/depth_view.rs`:252) が毎回受け取る。下地は `paint_image_underlay` でcallbackの直前に描く。
  `DepthSourceKey` はtexture IDだけで、`DepthRenderParams`・GPU色/深度資源・LRUに下地を焼き込まない。
  AI無効なら同じresource IDのままでよい。GPU回帰は同一callback・uniform・色/深度で
  黒→白→市松→黒を描き、各下地の不透明/半透明画素を2Dと比較する。
- AIアップスケール有効時の既存キー／リングは黒/白2段で、`clear_adjustment_caches`
  (`app.rs`:93984) → `clear_final_pipeline_caches_for_idx_inner`:85120 を呼ぶ。
  adjustment・final AI/composite・comic等の派生結果を失効し、pendingを取消す。
  `final_ai_key_for_pixels`:87267 と `final_composite_key_for_pixels`:89011 はbgをkeyに含む。
  実行側 `ai/final_pipeline.rs`:142付近は `background_mode` の黒/白へ合成してから拡大する。
  再合成は `ctx.load_texture` で新規IDを作る (`app.rs`:90069 / 90356付近)。
  `TextureHandle::set` の使用元も確認し、final/processedの同一ID更新は無く、該当した
  `ui_adjustment_panel.rs`:8896 は編集パネルのmask preview用だった。
- **深度推論はやり直しになる**。新規texture IDを `resolve_fs_processed_payload_and_depth`
  (`ui_fullscreen.rs`:13268) がそのまま素材として返し、`DepthViewState::select_source`
  (`depth_parallax.rs`:342) が旧slot/color/renderedを退役する。LRUは旧IDで新素材には一致しないためreadbackへ進む。
  未完成の合成は `NotYet(IncompleteComposite)` → `wait_for_source`:329 で2D待機となる。
  これは§4.1の「表示中の画素が変われば新規素材」に従う動作で、推論を省く変更は行わない。
  ここでいう再推論は**実際に背景を焼き込んだAI出力が置換される場合**。AI有効フラグだけで処理が無く、
  無変換素材の共有IDが保たれる経路では再推論は不要。同じ表示画素のkeyを維持するのも§4.1どおり。
- **古い素材を参照したまま3Dを描かない**。`try_paint_depth`:275付近は現在payloadのtexture IDと
  `resource.key.texture_id` が違えばcallbackを作らない。GPU callbackもcontext / key / resource IDを照合する。
  旧深度がLRUに残っても表示中素材として使用されない。**3Dモード自体は終了しない**。
  背景値やキャッシュ失効は入口・継続条件ではなく、素材の段階だけがReady→待機→新規Readyへ進む。
- 書き出しは変更なし。`depth_export.rs::admit_depth_export`:670付近が開いた時のdisplay/settingsとleaseを持ち、
  `ExportCapture::into_request`:547付近が下地を要求へ写す。Settings中の同一ページcapture更新は既存どおりだが、
  `app.rs::modal_dialog_block_reason` の `depth_export` と `document_open_modal_admission_blocked`:42338、
  `ui_fullscreen.rs`:34893付近のキー早期returnが背景変更・移動を止める。
  Runningはその要求だけを見るため、書き出しとShift+Bの競合を新たに扱う必要はない。
  既存の `depth_export_freezes_display_and_retains_modal_admission_until_dismissal` 等でlease/受付停止を確認。

公開文書の監査は `htdocs/mimageviewer/manual/` に `3D` / `Shift.*B` を検索した。
3D写真節は段Dで未追加のため今回同期すべきキー記述は無く、Creative 3D LUTの記述は別機能。
既存の背景キー説明は今回の動作と矛盾しない。キー仕様と表示パイプラインの3D節を同期した。
MainWindow案は§6.4.1だけに記録し、Bの製品コード・detached-rework-plan §11は変更していない。

検証記録:

- handler回帰2件を追加: キー／リング×AI有無の循環・不透明画像の同じ案内・Depth owner維持、
  Panoramaの既存キー抑止／リング許可。既存GPU回帰1件を黒・白・市松の連続切替へ拡張。
- 実child viewer回帰1件を追加: 毎passの下地shapeと同一Depth resource、AI有効cycleのfinal cache失効・
  置換素材のtexture ID照合・旧GPU slot/callback退役を確認。実モデルは使わず、背景焼込み済みの
  `FinalAiEntry` と既存fixtureの失敗済みlazy runtimeで隔離する。
  初回はAI有効フラグだけでtexture IDの変更を期待して1件失敗した。無変換素材のID共有という正しい挙動だったため、
  製品コードを変えず、実際に背景を焼き込んだAI結果を与えるfixtureへ修正した。
- `cargo nextest run -p mimageviewer --lib -E 'test(/depth|background|bg_cycle|ring/)'`: **731件成功**。
- 上記成功後に追加した実viewer回帰は `cargo nextest run -p mimageviewer --lib -E 'test(depth_background_cycle_actual_viewer)'`
  で **1件成功**（fixture修正後、実行1.357秒）。既存731件の実行時から製品コードの追加変更はなく、結果を再利用した。
- `cargo test -p mimageviewer --test ui_snapshot`: **245件成功**。
- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功（既存warningあり）。
- ワークスペース全体の `cargo fmt`、`git diff --check`: 成功。
  `python scripts/check_ui_glyphs.py`: dangerous glyph **0件**。
- `MSBUILDDISABLENODEREUSE=1`。全件lib/full gateは指示どおり未実施。
  build-dev・製品起動・stage・commitも行わない。UI文言の追加変更はない。
- 独立GPT-6.1 Sol / xhighレビュー: BのF11参照誤りを訂正し、未解決指摘なし。
  Bの採択とdetached構造合意は別途ClaudeCodeの判断待ち。

#### 段 C 続き: MainWindow 対応・案1 (2026-11-08、基点 `29bb7f54b`)

設計担当 ClaudeCode の採択と独立 Sol / xhigh の構造合意に従い、§6.4.1 の案1を実装した。

- `app/depth_view.rs::depth_presentation_eligible` に projected main context と
  `MainWindow | Fullscreen` の共通適格性を置く。入口の pure input はこの結果1つを受け取り、
  context と presentation を別々に判断する経路を持たない。`DepthEntryBlock` の画像表示・メイン表示の文言を更新した。
- `commit_viewer_presentation` は実切替または不適格な切替先で既存 setter を通して終了する。
  同じ適格な presentation では要求・LRU・slot・動きの時計を維持する。終了後のDは空LRUで新規開始。
  適格な切替先では終了トーストに実効キー（既定D）の再入場案内を出し、割り当てなしなら上部3Dボタンを案内する。
  対象外の別窓には再入場できるとの案内を出さない。
- MainWindow の canvas・fit・下地・回転・パネル・待機表示・マウス追従・書き出し・modal は
  既存共通経路を利用する。ROOT tail・入力側のOFF・推論通知先移管・追加repaint・fork/parkの変更はない。
- 状態遷移の全context/presentation matrix、同じ表示への再確定、F11両方向、F12、close/fork/parkを検査する。
  新しい遅延reply fixtureは本物のper-request channelを保持し、ReadingBack / Estimating 中のF11→D後に
  旧要求の完了が新owner／slotへ採用されないことを検査する。同じcontext／texture keyで再要求し、空LRUからの開始も確認する。
- ROOT embedded の実passでcallback、通常の素材再採用後に旧Depth描画を送らないこと、入口／描画直前の照合、保存2D transform不変、
  fit・下地・回転・リサイズ・パネルrect、待機／マウス／modal／小画面を検査する。
  書き出しは実パネルから開き、Settings／Running／Finishedの初回・後続・再open、
  外click／wheel／key／ring、dialog内操作、Remote leaseと外部openの拒否を確認する。
  素材再採用は既存 `FsUploadResult` → `poll_prefetch` を使い、source世代・新texture ID・旧edit/final失効を確認する。
  旧Readyを人工的に残してpayload ID guardだけを直接実行する検査ではない。同guardは変更せず、callback／receipt発行より前にあることを独立レビューで確認した。

変更ファイル（12本）:

- 製品境界・文言: `src/app/depth_view.rs`, `src/depth_parallax.rs`。
- 状態回帰・遅延reply fixture: `src/app/depth_view_tests.rs`, `src/depth_estimation.rs`（後者の変更は`cfg(test)`だけ）。
- 新規ROOT回帰: `src/app/depth_main_window_tests.rs`, `src/app/depth_export_main_window_tests.rs`,
  `src/ui_fullscreen/depth_main_window_tests.rs`。追加は計11テスト（状態・案内3、ROOT8）。
- テストmodule登録だけ: `src/app.rs`, `src/app/depth_export.rs`, `src/ui_fullscreen.rs`。
- 本書と `docs/detached-rework-plan.md` §11。公開文書・PNGは変更しない。

**検証記録**（全Cargoに `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、通常feature/test profile、基点＋今回の未コミット差分）:

| 実行 | 結果・証拠 |
| --- | --- |
| 最終 `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode/)' --test-threads 4` | **975/975成功**、exit0、116.616秒（compile1.22秒）。今回の11件と既存全画面3D・書き出し・表示切替の回帰を含む。`target/depth-mainwindow-final-related.log` |
| 初回の関連run（通知の実効キー対応前） | **974/974成功**、exit0、123.643秒。`target/depth-mainwindow-related.log`。最終runと重複加算しない |
| 素材再採用1件の先行確認（上記と重複加算しない） | **1/1成功**、exit0、1.523秒（compile3分05秒）。`target/depth-mainwindow-material.log` |
| 通知の実効キーと共通matrixの限定確認 | **2/2成功**、exit0、1.294秒（compile3分44秒）。`target/depth-mainwindow-toast.log`。既定D／F13／none×両方向を含む |
| `cargo test --offline -p mimageviewer --test ui_snapshot`（更新指定なし） | **245/245成功**、exit0、45.68秒。PNG変更なし。`target/depth-mainwindow-ui.log`。後続の通知文だけの修正はこのpure描画fixture群から呼ばれず、描画・fixtureも変わらないため独立レビューで結果の再利用を確認 |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、28.37秒（既存warningあり）。`target/depth-mainwindow-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-mainwindow-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI glyph **0件**。`target/depth-mainwindow-glyphs.log` |
| 独立レビュー（GPT-6.1 Sol / xhigh、同じcontextで継続） | **最終受入可、未解決指摘なし**。固定D案内のP2・fixture修正と上記証跡を照合し、症状パッチではない構造変更として確認。レビュー側の重複Cargo・製品起動なし |

反復中の失敗はfixtureの問題として修正した。最初のcompile2件（MouseWheelのphase/source、モデルpathの借用順）と
後のprivate入力surface helper呼出し1件はコンパイルエラーであり、テスト成功・失敗の実行件数には算入しない。
初回実行は11件中5成功・6失敗。通常2D callbackの誤計数を同passのDepth receipt＋callbackで区別し、
初回Area sizingを済ませてからpanel geometryを検査する形にした。modal検査は下流のring関数直呼びから
実gamepad dispatchへ変更し、透明素材とdismiss後のpositive controlを追加した。再openも実パネルclickへ揃え、
非同期workerの終端を復帰1passで決め打ちしない。続く8件は7成功・1失敗（素材のseedだけでは採用にならないfixture）。
通常の `FsUploadResult` 採用へ直し、上記1件と初回の関連974件で成功した。ここまでのfixture対応はテスト側だけで行った。
974件成功後の独立レビューP2は、再入場トーストの固定Dが再割り当て・解除に合わない点。
既存 `first_chord_label(FsDepthParallax)` と割り当てなしの上部3Dボタン案内に修正し、既定D／F13／none×両方向の回帰を追加した。
限定libのfilterなし全件・workspace full gateは指示どおり実施していない。
製品起動・実機操作・build-dev・stage／commitは今回の指示に従い実施しない。公開文書は段Dへ残す。

#### 段 C 独立レビュー P3: F12 の終了案内を最後の通知へ統合 (2026-11-08、基点 `29bb7f54b`＋MainWindow対応差分)

MainWindow／Fullscreen の3D中に通常F12を使うと、共通commitが出した終了案内を
`toggle_detached_viewer_mode` 末尾の「別ウィンドウモード ON」が上書きしていた。
通知欄は既存の1値を維持し、発行順や追加slotで対処しない。

- `commit_viewer_presentation_deferred_feedback` を共通内部commitとし、既存と同じ適格性・setterでの退役・
  presentation代入を行って、終了時だけ通知文 `Option<String>` を返す。
  既存 `commit_viewer_presentation` はその文を発行するwrapperとして残し、他の単独通知callsiteは変更しない。
- F12専用 `apply_egui_viewer_presentation_for_current_item` は通知文を呼び出し元へ返し、
  通常F12はON/OFFの最後の1通知へ理由を括弧付きで統合する。途中の終了通知は発行しない。
- もう一つの同helper呼出し（メディア別窓の同期案内）も同じ規則で統合し、既存のViewer面・Warning・3.5秒を維持する。
  この枝は現在media限定でDepth対象外だが、返却した通知を捨てたり2回発行する経路を作らない。
  native動画の非同期確定にも既存案内があるが、Depthに入れないため非同期ownerへ変更を広げない。
- F11の後段には同型の通知上書きはない。新context構築、fork／park、close／openの各commitも監査した。
  detached述語・表示切替規則・資源寿命・通知slotは変更しない。
- 実際のF12処理関数を通す回帰を強化し、main×MainWindow／Fullscreenからの終了で
  最後の通知がONと3D終了理由を両方含み、発行は1回だけであることを検査する。
  3DなしのON/OFFの既存文言、メディア案内の文言・面・種類・時間も検査する。

今回追加で変更したファイルは `src/app/depth_view.rs`, `src/app.rs`, `src/app/depth_view_tests.rs`,
本書と `docs/detached-rework-plan.md` の5本。直前の未コミットMainWindow対応差分を維持する。

評価の経路でpresentationが変わって3Dが終わるときは、後の評価の通知が案内を上書きし得る。まれで、3Dは正しく終わるので割り切る（2026-11-08 設計担当判断）。

検証記録（全Cargoに `MSBUILDDISABLENODEREUSE=1`, `CARGO_BUILD_JOBS=1`、通常feature/test profile）:

| 実行 | 結果・証拠 |
| --- | --- |
| F12／表示切替の先行限定run | **624/624成功**、exit0、81.427秒（compile4分39秒）。`target/depth-mainwindow-p3-tests.log`。後続runと重複加算しない |

再レビュー後、評価の3ファイルと `app.rs` のrating publication部分を基点 `29bb7f54b` へ完全に戻した。
所有境界テスト・評価の既存通知を維持し、F12とメディア案内の統合だけを残す。

復元後の検証（同じ環境変数・通常feature/test profile）:

| 実行 | 結果・証拠 |
| --- | --- |
| `grid_cursor_live_selection_writes_are_owned_not_input_helpers` のexact filter | **1/1成功**、exit0、1.291秒（compile5分20秒）。`target/depth-mainwindow-rating-revert-grid.log`。テスト本体・許可リストは変更していない |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/rating_membership\|undo_ops\|undo_stack\|meta_undo\|f12\|media_window_main_hint/)' --test-threads 4` | **134/134成功**、exit0、34.357秒（compile2.14秒）。`target/depth-mainwindow-rating-revert-related.log`。基点へ戻した評価・Undoと、残すF12通知の回帰を含む |
| `depth_media_main_hint_combines_feedback_without_changing_its_surface` のexact filter | **1/1成功**、exit0、0.417秒。`target/depth-mainwindow-rating-revert-hint.log`。メディア案内の合成・属性を維持 |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、1分00秒（既存warningあり）。`target/depth-mainwindow-rating-revert-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-mainwindow-rating-revert-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI glyph **0件**。`target/depth-mainwindow-rating-revert-glyphs.log` |
| 基点との一致 | fmt後も指定3ファイルはバイト単位で基点と一致し、指定の `git diff 29bb7f54b -- src/app/rating_membership_ops.rs src/app/rating_membership_tests.rs src/undo_ops.rs` は空・exit0。`app.rs` のrating publication部分も基点と一致。`target/depth-mainwindow-rating-revert-parity.log` |

stage／commit・製品起動・build-dev・filterなし全件は実施しない。

## 11. テスト

- **状態遷移**: OFF → ON → (ページ移動) → ON、× / D で OFF、フルスクリーン終了・フォルダ移動で取り消し。
- **続ける条件の照合** (§6.3): open_fullscreen の各種、スライドショー再開、フォルダをまたぐ移動、
  動画タイル、ページ一覧からの移動、同じページでの見開き・連結読みの切替、アニメーションと後から
  分かる場合の各経路で 3D が OFF になる。F12 の表示先切替でも終了し、linked の別ウィンドウでは
  新しい D の再入場で 3D を描く。独立 session/context も許可し、ほかの context が 3D を持つ間は `OtherWindow` で拒否する (§6.4.3)。
- **占有モードの持ち主** (§6.2): 360° ON から 3D に入ると `Depth` に切り替わり、360° の state と
  `intent.on` が消え、`intent.projection` は残る、3D 中に
  360° のページへ移っても 360° が復帰しない、3D 中の V キーは弾かれる、360° の非アクティブ state が
  残っていても 3D と同時に有効にならない。
- **動画を挟む**: 3D 中に動画のページへ移ると `Depth` が消え、静止画へ戻っても 3D は復活しない
  (見開き・連結表示でも同じ)。
- **要求の相関**: 古い鍵の結果を捨てる、取り消した要求の結果を捨てる、同じテクスチャで OFF → ON したとき
  前の要求の結果を受け取らない、slot の置き換えで前の要求が取り消される、`Drop` で cancel が立つ、
  別の context の状態が変わらない。
- **起こす責任**: ポインタを動かさずに 3D を ON にして `Ready` まで進む。
- **GPU 資源**: OFF・終了・terminal close で `callback_resources` の slot が消え、VRAM の会計から外れる。
- **入口条件**: 動画・アニメーション・見開き・連結読み・他 context の 3D 所有・編集モード・紙めくりの最中で弾き、
  理由が正しい。360° ON では弾かずに切り替わる (上)。3D 中は紙めくりが起きない。
- **占有モードの述語**: 3D 中に止めるキー・入口が 360° 中と同じであること (分類表の「止める」側)。
  360° 固有の挙動 (ホイール = 画角) が 3D 中に出ないこと。
- **シェーダの不変条件** (オフスクリーン描画のテスト。`egui037_stage4_gpu_tests.rs` と同じ仕組み):
  深度が一様で焦点と等しいとき出力は入力と同じ、深度が一様なとき全体が同じだけずれる、offset 0 のとき
  入力と同じ (zoom を除く)。
- **動きの関数**: phase 0 と 1 で同じ値 (継ぎ目が無い)、各動きの範囲。
- **前処理**: 正規化・guided filter・中央値をスパイクの出力と固定値で比べる。
- **書き出し**: 各形式でフレーム数・遅延・**1 周期の合計時間**・寸法・ループ指定を読み戻して確かめる。
  取り消しで途中ファイルが残らない。画面と書き出しの色が一致する (中間の灰色・半透明)。
  書き出し中は開く要求と Remote の取得が受け付けられない。
- **パネル**: UI スナップショット、小さい画面 (1093×614) で全項目に届くこと。

## 12. 文書の同時更新

- 製品: `htdocs/mimageviewer/manual/` (3D 写真の節)、`index.html` (機能カード)、`privacy.html` と
  製品ページの「安心して使えます」(**追加ダウンロードが 1 つ増える**: 通信先と保存されるデータ)、
  ライセンス一覧 (DA3 Mono-Large、Apache-2.0、fp16 化した旨)。
- 設計: architecture-overview (新しい worker とモジュール)、display-pipeline (3D の描画層)、
  async-architecture (深度 worker と読み戻し)、ui-responsiveness (読み戻し・アップロードの上限)、
  virtual-folders (ZIP / PDF は表示中の画素から作るので差が無いこと)、keymap-spec、
  360° の設計書 (占有モードの述語へ移したこと)、detached-rework-plan §11 (§6.4)。
- 公開手順: release-checklist にモデルの公開 (タグ `depth-model-v1`) を足す。

### 段 D の更新記録 (2026-11-08、基点 `930fbc7a4`)

再起動後は `0c988a36d` 上で継続。元モデルIDの正本は同commitで訂正済みの
`tools/depth_model/build_depth_addon.py::NOTICE` にある `depth-anything/DA3MONO-LARGE`。
本作業ではbuilderを再編集せず、公開手順の参照先もこのURLに揃えた。
再開時に製品ページとshortcutsの全内容がNULになっていたため、この2ファイルだけHEADから復元して
記録済みの段D差分を再適用した。他の編集は維持し、下記検査で14ファイルのUTF-8とNULなしを確認した。

公開名は **「3D 写真」で確定**。実装用語はライセンスの変更通知と開発者向け手順に限定し、
公開マニュアル・製品カードに本体の版番号を追加しない。コードの挙動・更新履歴・画像素材は変更しない。
Rust の差分は `src/ui_dialogs/about.rs` のライセンス案内の文言追加のみ。

| 更新ファイル（repository root から） | 内容 |
| --- | --- |
| `htdocs/mimageviewer/manual/fullscreen.html` | `#depth-photo` に入口・初回取得・右パネル・書き出し・制限・F11切替時終了・透過背景切替を追加。`#depth-export`、`#depth-availability` も設ける |
| `htdocs/mimageviewer/manual/shortcuts.html` | D、3D中の I / Tab と Shift+B、連結読みで使えない操作の一覧を更新 |
| `htdocs/mimageviewer/manual/settings.html` | `#depth-addon` に導入・削除・保存先・ライセンスと派生物の変更内容を追加。保存場所一覧にも追加 |
| `htdocs/mimageviewer/manual/index.html` | 目次から `fullscreen.html#depth-photo` へ直接リンク |
| `htdocs/mimageviewer/manual/troubleshooting.html` | 開始できない条件・取得後の再操作・初回準備と内蔵GPU等の待ち時間を短く説明。FAQに同じ説明を重複追加しない |
| `htdocs/mimageviewer/index.html` | AIカード付近に3D写真カードを1枚追加。安心欄のPC内処理に奥行き計算を含め、ライセンス欄にDA3派生物を追加。通信のカードは既に任意機能とprivacyへのリンクで包括できるため維持 |
| `htdocs/mimageviewer/privacy.html` | 日英の任意ダウンロード・保存データ・PC内のAI処理に深度モデルを追加。GPU専用との読み違いを避け、PC内処理と記載 |
| `README.md`、`installer/readme.txt`、`installer/readme_portable.txt`、`src/ui_dialogs/about.rs` | 第三者ライセンス案内にDepth Anything 3（DA3 Mono-Large、Apache-2.0）・fp16化・入力固定・再学習なし・LICENSE/NOTICEの参照方法を追加 |
| `docs/release-checklist.md` | Phase 5に初回・モデル更新時の公開手順を追加。builder、4 assets、独立タグ、latestにしない、既存資産維持と新タグへの更新を明記 |
| `htdocs/sitemap.xml` | 生成スクリプトで更新した公開ページのlastmodを反映（URL増減なし） |
| `docs/depth-parallax-plan.md` | 段D・本節・§15の名称確定を記録 |

**記載した事実のコード根拠**（行番号より関数名を追跡の正本とする）:

| 事実 | 根拠 |
| --- | --- |
| メインウィンドウ/全画面の静止画・単ページに限定。動画/音声/アニメ/見開き/連結/編集/紙めくり/別窓/GPU描画なしを拒否 | `src/depth_parallax.rs::DepthEntryBlock / entry_allowed`、`src/app/depth_view.rs::depth_entry_allowed / depth_presentation_eligible`。実際の2ページ表示判定を使う |
| F11等の実表示切替で終了、再度D/ボタンで開始 | `src/app/depth_view.rs::commit_viewer_presentation_deferred_feedback / toggle_depth_parallax`、`src/ui_fullscreen.rs` の3D上部ボタン |
| D、I/Tab、Shift+Bの既定・3D中のパネル切替/背景切替 | `src/keymap.rs::default_chords` の `FsDepthParallax / FsToggleMetadata / FsBgCycle` と `docs/keymap.ini.default`、`src/ui_fullscreen.rs::handle_fs_key_input_with_operation`、`src/app/depth_view.rs::toggle_depth_panel` |
| 初回約0.67GB、同意後取得、完了で自動入場しない、導入・削除・保存先表示 | `src/ui_dialogs/depth_addon.rs::draw_dialog / poll_depth_setup / depth_setup_completion_message / draw_preferences`、`src/ui_dialogs/model_addon.rs::draw_actions`。環境設定ツリーは `src/ui_dialogs/preferences.rs` の `DepthAddon` |
| GitHub Releasesの固定タグ、データフォルダ内addons/depth、ポータブルはdata配下 | `src/addon.rs::AddonKind::default_base_url / index_name`、`src/depth_addon.rs::addon_root`、`src/data_dir.rs` |
| 奥行き計算はローカル、画像を計算のために外部送信しない | `src/depth_estimation.rs::estimate / infer_disparity` がローカルモデルを `AiRuntime::load_model / with_session` で実行。取得処理は別の `src/addon.rs` と `src/vlm_addon_download.rs` |
| 動き5種、強さ/周期/止まる深さ/整形の範囲と既定、共通設定として保存 | `src/depth_parallax.rs::MotionKind / DepthParallaxSettings`、`src/ui_depth_parallax.rs::draw_depth_parallax_panel_with_export`、`src/app/depth_view.rs::draw_depth_panel` |
| 書き出し3形式、fps・ループ、長辺5候補と形式別既定・縮小側の候補・拡大なし | `src/depth_export.rs::DepthExportFormat / DepthExportOptions`、`src/ui_depth_export.rs::long_edge_choices / default_long_edge / draw` |
| マウス追従時の書き出し初期値は左右、現在の設定と背景を反映、保存先と大きさを通知 | `src/app/depth_export.rs::admit_depth_export / poll_depth_export / ExportCapture::into_request`、`src/depth_export.rs` の `DepthExportSummary`（大きさ取得失敗時は省略） |
| 書き出し設定から完了まで閲覧を止め、進捗・取消を表示 | `src/app/depth_export.rs::Operation`、`src/app.rs::common_modal_dialog_open / document_open_modal_admission_blocked`、`src/ui_depth_export.rs::draw` |
| Remoteでは3Dを表示せず、リモート操作中のローカル開始も拒否、360°との切替・編集との排他 | `src/app/depth_view.rs::resume_depth_entry_after_check / set_fs_exclusive_view`、`src/app.rs::close_fs_exclusive_conflicts`、`src/ui_fullscreen.rs` の編集/360°入口と操作権判定 |
| DA3 Mono-Large、Apache-2.0、fp16化・1枚504×504入力固定・再学習なし、モデルとLICENSE/NOTICEを取得 | `tools/depth_model/build_depth_addon.py::NOTICE / build`、`src/depth_addon.rs::validate_index`、`src/addon.rs::install_for_kind` |

**設計との照合・記述を控えた事項**:

- §6.1の箇条書きに明記のない「紙めくり中」の拒否も、コードの `DepthEntryBlock::PageTurning` に従って掲載した。
- §9の「画像フルスクリーン」はkeymapの文脈名。利用者向けには§6.4.1と実コードどおりメインウィンドウ内表示も含めた。
- §1の書き出し「1周期」だけではMP4の長さを説明しきれないため、§7と実コードのループ回数1〜10・既定4回を記載した。
- 待ち時間の根拠は [研究調査 §8.5](depth-parallax-research.md)。
  初回GPU準備2〜3.5秒と内蔵GPU初回13〜19秒の記録があり、公開文書は「数秒以上かかることがある」に留めた。
  一般的なGPU/低速CPUへの所要時間の保証、極端な縦横比での品質、書き出しファイル容量の目安は掲載しない。
- モデルReleaseの公開状態・公開URLからの実取得は今回未確認。公開手順として記載し、取得済みとは主張しない。

**検査記録**（`0c988a36d`＋段Dの未コミット差分、通常feature）:

| 検査 | 結果 |
| --- | --- |
| `python scripts/check_site_cards.py` | exit 0、53枚・違反0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0 |
| `python scripts/gen-effetune-docs.py --check` | exit 0、137 version files＋entry＋redirectsが現行と一致 |
| `python scripts/test_gen_effetune_docs.py` | exit 0、33/33 |
| `python scripts/gen-sitemap-xml.py --check` | exit 0、216 URLs。生成によるURL増減なし |
| 内部リンク・HTML・UTF-8/NUL検査 | `target/depth-docs-audit.py`、exit 0。変更した14ファイルはUTF-8/NULなし、公開ページのfragment91件・新規ローカルリンク9件に不在先なし、新規の閉じ忘れ/重複IDなし |
| `python scripts/test_public_privacy_scope.py` | exit 1、7/9。privacyの既存「確認では」の検出と旧SFZ保存先の期待値で2件失敗。同じscriptをHEADの公開3ファイルへ適用して同じ2件失敗を再確認。今回の深度追記による追加失敗はなく、範囲外の表記・期待値は変更しない |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、通常dev。ログ `target/depth-docs-cargo-check.log`、既存warningあり |
| `cargo fmt --check` | exit 0 |
| `cargo test -p mimageviewer --lib ui_dialogs::about::tests::` | exit 0、7/7（snapshotを含むheadless）。通常test profile、コンパイル6分44秒・実行3.26秒、既存warningあり。再起動前の中断結果は採用せず再実行した。ログ `target/depth-docs-about-tests.log` |
| 同じモデル作成用venvで `python -m unittest discover -s tools/depth_model -p test_build_depth_addon.py` | exit 0、2/2。モデル取得・製品起動なし |
| `python scripts/verify-site.py htdocs https://www.mikage.to` | exit 1、392件すべてWinError 10061（接続拒否）で取得できず。公開内容の一致は未確認。ログ `target/depth-docs-verify-site.log` |
| `git diff --check` | exit 0。stageされたファイルなし |

`gen-changelog-html.py` は更新履歴、`gen-moire-sample.py` は画像素材を生成するため、依頼の範囲外として実行しない。

#### 段 D の公開・検収 (2026-11-08、`dcabbd100`)

- **独立レビュー** (GPT-6.1 Sol / xhigh、read-only、`git diff 0c988a36d dcabbd100`): P1〜P3 とも指摘なし。
  公開文書の事実 (入口条件・F11/F12・パネル・書き出し・保存先・同意・既定キー・360°/Remote) がコードと一致、
  privacy の日英が同内容、ライセンス表記が NOTICE と一致、release-checklist の手順が実物と整合。
- 製品ページの安心欄で、Codex の差分が既存の AI 説明文へのリンクと「画像内の文字」の記述を削っていたため、
  設計担当が元の文言に戻して 3D を足す形にした (コミット前に修正)。
- **モデルの公開**: 利用者の承認 (2026-11-08) を受けて GitHub Release `depth-model-v1` を作成。
  プレリリース・latest にしない、target は v5.0.0 (`446c59562`)。資産は `da3mono_large_fp16_504.onnx`
  (671,032,016 B)、`LICENSE`、`NOTICE` (704 B、元モデル URL 訂正版)、`depth-model-index.json`。
  公開前に元モデルのライセンスを再確認した: 公式 GitHub README の表・公式 Hugging Face
  (`depth-anything/DA3MONO-LARGE`) のモデルカードとも Apache-2.0。変換元 SpatialHub の ONNX リポジトリは
  見出しの license 欄が cc-by-nc-4.0 だが、本文で da3mono_large.onnx は Apache-2.0 を継承すると明記している
  (Giant / Large だけが CC BY-NC)。この食い違いを示したうえで利用者が公開を判断した。
- **サブ PC 確認依頼 21** (観測者: サブ PC 担当、ビルド `dcabbd100` の dev-runtime、モデルなしの隔離 data から実取得):
  14 項目中 13 OK。同意画面の文言 (約 0.67 GB・Apache License 2.0・depth-model-v1・保存先)、
  「今はしない」で何も作られない、途中取消で導入済みにならず次回は続きから再開 (`.partial` は完了後に消える)、
  完了で自動入場しない・完了通知の文言、`packs/da3mono-large-fp16-504-0f5ff3be208d/` の内容と onnx の SHA-256 が
  目録と一致、D で約 3 秒後に 3D が動く、再起動後は案内なしで入れる、環境設定からの削除と再導入、
  バージョン情報の「3D 写真」節、ログに ERROR / WARN / panic なし。
  - NG 1 件 (ダウンロード中に後ろのページが送れる) は、依頼文の期待が誤っていた。§3.3 のとおり
    取得中は modeless で、`depth_management_modal_boundary_keeps_download_modeless` がこれを固定している。不具合ではない。
  - 気づいた点: 環境設定の検索で枠が付いたとき、ライセンス行の末尾「)」が枠線に掛かって見えた。
    文言と配置は AI 説明文 (リリース済み) の行と同じで、3D 固有ではない。段 D の対象外として残す。
    取得完了後に `downloads/…` の空フォルダが残る点も VLM と共通の既存動作。
製品起動・確認用build・stage・commitは行っていない。

## 13. まれな失敗の扱い

まれな失敗は割り切り (ログ + 通知)。ダウンロードの失敗・推論の失敗・GPU の読み戻しの失敗・
書き出し中のディスクの失敗は、ログと通知を出してその操作を終える。再試行や途中状態の保持は足さない。
推論が失敗した画像はパネルに失敗を出し、2D のまま表示する (同じ鍵では再試行しない。3D を入れ直すと再計算)。

## 14. 決めたこと・採らなかった案

| 論点 | 採った案 | 採らなかった案と理由 |
| --- | --- | --- |
| 深度の入力 | 表示中テクスチャを GPU で縮小して読み戻す | CPU の画素を 360° の `resolve_pano_source` のように選び直す: 表示側と別の選択をもう 1 つ持つことになり、補正・AI・編集の組み合わせで表示とずれ得る。8K では CPU 側のコピーも大きい |
| 色の元 | 表示中テクスチャの GPU コピー + mipmap | 360° のように CPU から上げ直す: UI スレッドで 40〜110 ms 止まる (360° 設計書 §4.1.1) |
| 推論サイズ | 504×504 固定・引き伸ばし | 縦横比ごとのセッション: セッションごとに重み (VRAM 約 1.3 GB) を持つ。余白で埋める: 深度が崩れた (§3.1) |
| 形の固定 | 配布ファイルで固定 | ランタイムの dimension override: 固定ファイルなら不要で、読み込みの経路が他のモデルと同じになる |
| モデル | DA3 Mono-L fp16 の 1 つ | DAv2-Small を既定に足す: 利用者がイラストで Mono-L を最も自然と判断。fp16 で内蔵 GPU でも約 1 秒 |
| 配布 | 独立したアドオン (VLM の仕組みを共通化) | 本体に同梱: 0.67 GB 増える。編集パックに同梱: 被写体マスクだけの人まで落とす |
| 3D の ON を持ち越す | 持ち越さない | 360° の intent のように持ち越す: 復帰の契機・入口条件・park の扱いが増える |
| 360° との排他 | 1 つの enum (`FsExclusiveView`) と 1 つの遷移の持ち主 | 別々の `Option` + 判定の集約だけ: 360° の非アクティブ state や自動復帰で両方が有効になり得る (独立レビュー P1) |
| 満たさなくなったとき | OFF にする | 360° のように保ったまま非アクティブ: 戻ったときに入場の後始末を通らない穴 (§6.3) |
| OFF にする場所 | 毎フレーム 1 か所で照合 | 各ページ移動・見開き切替・F12 の関数に OFF を足す: 経路が多く網羅できない (独立レビュー P2) |
| 別ウィンドウ | v1 は対象外 | 対応する: 再描画・GPU 資源・worker の取り合いの組み合わせが増える (§6.4) |
| 書き出し | モーダル、画面と同じシェーダ | 非モーダル: 書き出し中の操作との組み合わせ。CPU で描く: 描き方が 2 つになる |
| AI の優先度 | 足さない (既存の mutex のまま) | 深度推論を後回しにする優先度: 3D は利用者が今待っている前面の処理 |

## 15. 調査で解消した点と未確認の点

段 B 着手時の source 監査 (基点 `8adc67829`) で次を確認した。

- `TextureHandle::set` / `set_partial` の production 呼出元は
  `ui_adjustment_panel.rs:8731` (local-adjust mask preview)、`ui_conceal.rs:1189`
  (conceal mask の部分更新)、`ui_erase.rs:1364` (erase mask の部分更新)、および別アプリの
  `crates/musicvideo-editor/src/app.rs:1905` (動画編集 preview)。前三者は編集 UI の mask overlay、
  後者は fullscreen の producer ではない。final / processed の元画素には到達しない。
  final の生成は `app.rs:89372,89875,90162,90820` の新しい `ctx.load_texture`、
  edit material の再生成は `86889,86929` の upload helper を使う。
- Native だけが残っている間も `resolve_fs_processed_payload` は held payload を選ぶ前に
  通常 material selector を呼ぶ。通常の final 準備 (`ensure_final_composite_texture_impl`)
  → `ensure_edit_result_pixels` → `assemble_edit_result_pixels` →
  `ensure_original_material_for_entry(idx, OriginalMaterialEntry::MaterialOnly)`
  (`app.rs:89960` 以降、`86864,86998,87001,78804`) が既存 pending / upload owner へ再準備を要求する。
  CPU edit pixels が残る場合は既存の paced upload を使う。紙めくり中もこの通常準備は続き、
  終了後は同じ selector へ戻る。Depth 中は `page_turn_view_eligible` が false となり、
  `page_turn_held_resource` が None を返すため、通常 selector の素材がそのまま選択される。
  専用の「紙めくり再準備 API」は不要で、3D 専用 producer は追加しない。
  source-less payload は `NotYet(NativeOnly)` として既存の準備を待つ。
- マウス方向は §10 の負の target に決めた。基準の shader 式を変えず、右 pointer で手前が左へ動く。
  符号と実際の GPU sample 方向の回帰を追加した。実機での操作感の確認は残る。

段Cで、映像だけのmuxer、GIFの累積遅延、独立GPU資源と画面の色・半透明下地の一致、
Remote leaseと外部open受付の排他をコードと回帰検査の対象へ具体化した。実行結果は§10に集約する。

公開名は「3D 写真」で確定し、この名前で公開文書を更新した（段D）。以下は未確認。

- 極端な縦横比 (3:1 以上) での深度の劣化 (§3.1)。
- 一般的な GPU・遅い CPU での所要時間 (研究調査 §8.5)。

## 16. 独立レビューの記録

### 第 1 回 (2026-11-06、GPT-6.1 Sol / xhigh、read-only、実装前)

P1 4 件・P2 6 件。ClaudeCode が主要な根拠 (egui のテクスチャの usage と形式、F12 の経路、
`image` の GIF 遅延の切り捨て) をコードで確かめ、すべて採って本書を直した。

| 指摘 | 対応 |
| --- | --- |
| P1 egui のテクスチャに `COPY_SRC` が無く GPU コピーできない | blit で level 0 を描いてから mipmap (§4.2) |
| P1 通常の F12 は `reset_detached_pause_foreground_modes` を通らない。メインの context も別ウィンドウになる | 「メイン」を識別子 + presentation で定義。F12 は毎フレームの照合で OFF (§6.4) |
| P1 判定の集約は排他状態の持ち主にならない。360° の 2 つの入口と非アクティブ state で両方が有効になり得る | `FsExclusiveView` の 1 つの enum と遷移の持ち主。3D に入ると 360° の intent も落とす (§6.2) |
| P1 書き出しを `Rgba8UnormSrgb` にすると sRGB が二重にかかる | 3D の描画先をすべて `Rgba8Unorm`。透過は背景色へ合成して不透明に (§4.2・§7.2) |
| P2 OFF の境界がページ移動以外 (後から分かるアニメーション、見開き・連結の切替) を覆わない | 毎フレーム 1 か所で照合 (§6.3) |
| P2 payload が元テクスチャを持たない場合 (Native、注釈のベイク待ち) | 素材の契約 `DepthSourceMaterial` (§4.1) |
| P2 マウス追従で止まっている間に非同期処理が進まない | `ReadingBack` 中の poll + 再描画、結果は要求元 viewport へ repaint (§4.2) |
| P2 `callback_resources` の slot が残る、VRAM の会計、要求の寿命を `TextureId` で識別している | slot の明示除去、accountant 登録、context + `request_id` と要求ごとの受け口 (§4.4・§5.1) |
| P2 モーダル登録だけでは開く要求と Remote の取得が止まらない、uniform が画面と混ざる | `try_local_modal_lease`、`document_open_modal_admission_blocked`、要求の凍結、書き出し用の pipeline (§7.1・§7.2) |
| P2 GIF の遅延が 60 ms に切り捨てられ周期が約 10% 短くなる | 累積時刻から 60 / 70 ms を配分し、合計時間をテストする (§7.3) |

レビュー担当は、bundle 所有と既存の識別子による対象制限 (§6.4) を「症状パッチではなく構造的な変更」
と認めた (上の P1 2 件を反映する条件付き)。ClaudeCode も同じ判断。detached-rework-plan.md §11 への
記録は段 B の実装時に行う。

### 第 2 回 (2026-11-06、同じセッションの継続)

第 1 回の 10 件は、解決 7・一部 3 (F12 の終了の時点、書き出しの下地、照合の位置)。新しい指摘 4 件を採った。

| 指摘 | 対応 |
| --- | --- |
| P1 照合の位置が実際のフレームの順序と合わない (準備が root の入力より前、子 viewport 内にも採用とキー処理) | 照合は 1 つの関数で、3D の描画分岐の直前に呼ぶ。加えて presentation の切替の確定点で `Depth` を終える (§6.3・§6.4) |
| P2 紙めくりとの関係が未決定。単ページの静止画は紙めくりの対象 | 3D 中は紙めくりの対象外。紙めくりの最中の D は弾く (§6.5) |
| P2 書き出しの下地が凍結されていない (黒・白・市松、余白色とは別) | 下地の種類と市松の升目を凍結、合成後 alpha 1、各下地で一致を検査 (§7.1・§7.2) |
| P3 360° からの入場テストが旧仕様 | 切替・`intent.on` 解除・projection 保持の検査に変更 (§11) |

レビュー担当の判断: 段 A (モデル・アドオン・推論・前処理) は段 B / C の確定を待たずに着手してよい。
注: `codex exec resume` は sandbox の指定を引き継がず workspace-write で動いた。作業ツリーに変更が無いことを確かめた。

### 第 3 回 (2026-11-06、同じセッションの継続、read-only)

段 A・C は着手可。段 B は 1 件 (P1) を確定してから。

| 指摘 | 対応 |
| --- | --- |
| P1 照合が 3D の描画分岐へ届く経路だけで、動画の early return や見開き・連結で飛ばされ、静止画へ戻ると復活する | viewer の pass の先頭 (媒体別の分岐より前) と 3D の描画分岐の直前の 2 か所で同じ関数を呼ぶ。借用・lock の前、paint callback の中では行わない。動画を挟むテストを追加 (§6.3・§11) |

F12 の確定点で終える変更と、viewer の pass の中で持ち主を呼ぶこと (既存の描画も `&mut self` で状態を
更新している、ui_fullscreen.rs:48594) は妥当と確認された。

### 第 4 回 (2026-11-06、同じセッションの継続、read-only)

第 3 回の P1 は解消。新しい P1 / P2 は無し。段 B も着手可。

## 17. 次の版の改善: クリックで止める点・マウス追従の範囲・止まる深さのハイライト (§1.751〜§1.753、2026-11-10)

v5.1.0 の配布ビルドを利用者が試して出した要望 3 件 (`next-release-backlog.md` §1.751・§1.752・§1.753)。
設計: ClaudeCode Opus 5.5。worktree `G:\home\mimageviewer-3d-751`、ブランチ `feat-3d-751`。

### 17.1 方針 (状態を増やさない)

- **止まる深さの持ち主は今までどおり `DepthParallaxSettings::focus` の 1 つだけ**。クリックで決めた点を
  画像ごと・別の値 (「ピン」) として持たず、クリックは「その場所の深さを `focus` に入れる」操作にする。
  パネルのスライダーがそのまま新しい値を示し、スライダーで戻せる。書き出しも同じ `focus` を使うので、
  書き出しへの反映のための経路は要らない。
  - 採らなかった案: 画像ごとのピン (位置または深さ) を `DepthViewState` に持ち、スライダーの値と別に
    優先させる。持ち主が 2 つになり、ページ移動・書き出し・スライダー操作との優先順位と寿命を決める
    必要が出る。深度は画像ごとに 0〜1 へ正規化され、整形 ON では中央値が 0.5 に揃うので、共通の値を
    次の画像へ持ち越しても意味は保たれる。
- **ハイライトは表示だけの一時状態**。設定にも書き出しの要求にも入れない。書き出しはモーダルで、
  要求は `DepthRenderParams::from_settings` から作るので、そこでハイライトを常に 0 にすれば書き出しに
  混ざる経路は無い。
- 新しいモード・新しい非同期処理は足さない。クリック位置の深さは CPU に残した深度マップから同期で読む
  (最大 1536² の f16 の 1 点の双線形補間。I/O なし)。

### 17.2 §1.751 クリックした場所を止まる点にする

**操作**: 3D 写真の表示中に、画像の上で **Ctrl+左クリック** (マウス)。案内は「**左 Ctrl**+クリック」とする。

- 修飾なしの左クリックは変換がないときにページを送る。拡大・パン・自由回転中は 2D と同じく送らず、
  ダブルクリックで変換をリセットする。Ctrl+クリックは `belongs_to_view` の抑止より先、ダブルクリックの
  リセットより後に分類する ([表示変換設計](depth-view-transform-plan.md) §3.3)。3D 以外の Ctrl+クリックは変えない。
- **右 Ctrl は既定で元画像の一時表示 (`FsOriginalPreviewHold`) に割り当てられている** (keymap.rs:7147)。押している間は
  素材が `NotYet(OriginalHold)` になり 3D の描画資源が外れる (ui_fullscreen.rs:13303)。この優先は変えない
  (元画像ホールドの挙動を変えるとリリース済み機能の変更になる)。元画像ホールド中の Ctrl+クリックは
  止まる深さを変えず、ページも送らない。そのため案内は「左 Ctrl」にする。
- **Ctrl の判定は、受理した左ボタンの release イベントが持つ修飾** (`egui::Event::PointerButton { modifiers, .. }`、
  egui-winit が押下時の状態を記録する) で行う。OS の現在値 (`ctrl_held_via_os`) は、release から処理の pass までに
  Ctrl を離すと通常クリックへ落ちるので使わない。Shift / Alt が同時に押されていても Ctrl があればこの操作。
- クリックの分類は既存の `plan_fs_canvas_click` に `FsCanvasClickAction::DepthFocusAt(pos)` を足して行う。
  パネル・シークバー・ポップアップ・オーバーフロー・modal・Remote 等で `None` / `DismissOverflow` になる条件と、
  Ctrl+Space の scrub など planner より前に primary を所有する操作は、既存の順のまま先に効く。`PageSide` を返す
  直前で「3D 中かつ Ctrl」なら `DepthFocusAt` を返す。ドラッグ (3 px 超)・ダブルクリックの扱いは従来どおり。
- `DepthFocusAt` の処理 (どの場合も**ページは送らない**):
  - 深度が計算中 (`ReadingBack` / `Estimating` / `WaitingForStableTexture`) で、元画像ホールド中でない: 「奥行きの計算が終わると使えます」を短く通知。
  - 失敗 (`Failed`): 何もしない (失敗はパネルと既存の通知で知らせ済み。同じ鍵では再試行しない §13)。
  - 元画像ホールド中: 何もしない。
  - 描画できる (`ready()` あり): 下の「描画時の解決」へ要求を渡す。画像の外だった場合は描画時に何もしない。
    深さが決まったCtrl+クリックは既存パネルと同じ操作時の `settings.save()` を1クリックにつき1回だけ同期実行する。
    [UI応答性 §2.5](ui-responsiveness.md#25-静止画シーク回転-reader-の所有境界)の明示的な例外に従い、再passやハイライト中の再描画では保存しない。
- **保証の範囲はマウスのクリック**。タッチのタップは Ctrl の有無にかかわらず従来どおり (ページ送り) で、止まる深さは
  スライダーで調整する。ただし 3D パネルの上のタップが背面のページ送りになる既存の穴は、パネルに新しい
  ボタンを置くのでこの機会に塞ぐ: 表示中の 3D パネルの矩形をタッチの除外領域 (ui_fullscreen.rs:40477 の組立て) にも加える
  (マウス側は ui_fullscreen.rs:40117 で既に除外している)。
- **keymap**: 押した位置を使うポインター操作で、離散コマンドではないので `KeyAction` にしない。
  `docs/keymap-spec.md` の 3D 写真の節に固定入力として理由とともに書く。

**描画時の解決 (前の pass の記録を使わない)**: 入力の処理は描画より前にあり、前の pass で記録した幾何は
ページ移動・回転・大きさの変更の直後に今の表示と食い違い得る (独立レビュー P2)。そこで、クリックは
「同じ pass の要求」として `DepthViewState` に `pending_focus_click: Option<(egui::Pos2, u64 /* cumulative pass nr */)>` で置き、
**同じ pass の `try_paint_depth` が、実際に描く資源 (`resource.key` が payload の source と一致した後) と transform で解決して消費する**。
pass 番号が違う要求は解決せずに捨てる (その pass で 3D を描かなかった = 資源が無い・素材が変わった)。
新しい記録や待ちの状態は増えない (1 pass だけ生きる要求)。

**クリック位置 → 深さ**: シェーダと同じ写像を CPU の純粋関数で持つ。

- 画面の点 `pos` → full_image_rect の中心を軸に自由回転の逆変換 →
  `in_uv = (逆変換した点 − paint_rect.min) / paint_rect.size` (範囲外なら `None`)
  → `base = (in_uv − 0.5) / zoom + 0.5` → `rotate_uv(base)` (WGSL の `rotate_uv` と同じ 4 通り)
  → `source = mix(uv.min, uv.max, …)` を uv の範囲に clamp
  → 深度マップを **clamp-to-edge の双線形** (texel 中心 `(i + 0.5) / w`) で読む
  → `pow(clamp(d, 0, 1), gamma)`。`gamma = depth_gamma(shape_depth, median)`。シェーダは整形後の深さと
  `focus` を比べるので、`focus` に入れるのは整形後の値。`zoom` は `DepthRenderParams` と同じ値を使う
  (式を 2 か所に書かない。解決は `try_paint_depth` 内で作った `params` から読む)。
- 幾何は `DepthScreenMapping` の純粋写像を CPU と uniform の生成で共有する。シェーダは絶対フラグメント座標を
  使い、callback viewport の丸めに合わせて画像全体を縮めない。補間と有限探索 (40 段 + 二分 5 回) の
  数値差は残るため、検査は許容誤差つきで行う (§17.6、[表示変換設計](depth-view-transform-plan.md) §3.2〜§3.3)。
- **今の動き (offset) は使わない** (offset 0 の写像)。`focus` を変えると全体が `offset × Δfocus` だけずれるので、
  「クリックの瞬間に動いて見えていた被写体の点」を選んでもその点はポインタから離れる。offset 0 の写像なら、
  `t = focus` で逆引き位置が常に `base` になるので、新しい `focus` で描いたときに**クリックした画面上の位置にある面が
  そこで止まる** (遮るものが無い限り)。「クリックした場所が動かない」に合うのはこちら。左右方向の「回転の中心」は無い
  (このシェーダは画面全体を同じ量ずらすので、止まるのは深さで決まる面全体)。
- 保証はクリックした時点の強さ (zoom)・回転・整形に対するもの。後で「奥行きを整える」や強さを変えると、
  `focus` は数値のまま保ち、以前の被写体点を固定し直さない (利用者がもう一度クリックすればよい)。
- CPU の深度: `DepthTexture::upload` で深度マップの値 (`Vec<half::f16>` と寸法) を GPU へ上げた後も `DepthTexture` に残す。
  LRU は同じ `Arc<DepthTexture>` を渡すので、LRU から戻った場合も使える。最大 1536² × 2 B ≈ 4.7 MB、
  LRU 4 枚で最大約 19 MB の CPU メモリ。VRAM の会計 (`byte_size`) には入れない。
- 決まった値は `settings.depth_parallax.focus` に入れて `normalize` 後に保存し (パネルの変更と同じ保存経路)、
  §17.4 のハイライトを始める。

### 17.3 §1.752 マウス追従を画面中央の狭い範囲で端まで動かす

- `MouseFollow::update` の正規化を、画像の矩形の大きさではなく**表示領域 (キャンバス) の短辺の半分の正方形**に
  する: `r = 0.25 × min(canvas.w, canvas.h)`、`n = (p − visible_center) / r` を各軸 ±1 で頭打ち。
  中心は画像の外接矩形の可視範囲の中心。画像全体が見えるときは従来の画像中心と同じで、片側配置も維持する。
  自由回転時は外接矩形の近似を使う。縦の 0.6 倍と符号は変えない ([表示変換設計](depth-view-transform-plan.md) §3.3)。
  - 1920×1080 の全画面なら、中央の 540×540 px の正方形の縁で動きの端に届く。上下の HUD の帯
    (上端・下端) まで寄る必要が無い。
  - 縦横で同じ `r` を使う (横長の画像で左右だけ感度が鈍くならない)。
  - 小さい画像でも感度が極端にならないよう、画像ではなくキャンバスを基準にする。
- 範囲は**固定** (設定項目を足さない)。0.25 は利用者の実機確認で調整する前提の初期値で、定数 1 か所に置く。
- HUD の帯・パネルの上にポインタがあるときの扱いは変えない (帯の中は頭打ちで端のまま、パネル上は目標を
  固定)。帯へ寄せる必要がなくなるので、帯専用の分岐は足さない。
- キャンバスの矩形は、`try_paint_depth` の呼出元が既に持つ画像の表示領域 (固定バー等を除いた media rect。
  `resolved_fullscreen_media_rect` / 呼出元の `image_rect`、ui_fullscreen.rs:5065) を引数で渡して使う。
  painter の clip 矩形は使わない。中心は呼出元が四角形の外接矩形とキャンバスの交わりから求めて渡し、
  交わりが空ならキャンバス中心を使う。

### 17.4 §1.753 止まる深さのハイライト

- **出る時**: パネルの「止まる深さ」スライダーをドラッグ中・値を変えたフレーム (キーボードでの変更を含む)、
  §17.5 の「既定値」ボタン、§17.2 の Ctrl+クリック。最後にこれらが起きてから **1.0 秒は強さ 1、その後 0.3 秒で 0 へ**
  フェードして消える。ホバーだけでは出さない (パネルへポインタを動かすたびに色が付くのを避ける)。
- **持ち主**: `DepthViewState` の `focus_highlight: Option<Instant>` (最後に起きた時刻) と、時刻から強さを返す純粋関数
  `focus_highlight_intensity(last, now) -> f32` だけ。3D を抜ける・ページを移ると `DepthViewState` の寿命や
  深度の作り直しに従う (特別な解除は足さない。ページを移ってもフェードが残るだけで害は無い)。
- **再描画**: 強さが 0 でない間は毎フレーム `request_repaint` (最長 1.3 秒)。マウス追従で止まっているときも
  フェードが進む。最小化・非表示・書き出しの modal 中は要求しない (既存の動きの再描画と同じ条件、depth_view.rs:347)。
- **描き方 (シェーダ)**: uniform の空き (`view.w`、今は 0) にハイライトの強さを入れる。uniform の大きさは変えない。
  レイマーチで決まった面の深さ `t_hit = (near + far) / 2` と `focus` の差で
  `w = h × (1 − smoothstep(0.03, 0.05, |t_hit − focus|))`。色は
  `rgb = mix(c.rgb × (1 − 0.2h), vec3(0.0, 0.85, 1.0) × c.a, 0.55w)` (premultiplied のまま。帯の外を 20% 暗くして
  帯を見やすくする)。色・幅・暗さは初期値で、実機確認で調整する。
- **書き出しには含めない**: `DepthRenderParams::from_settings` はハイライト 0 を返し、画面の paint だけが
  `with_focus_highlight(h)` で上書きする。書き出し側は from_settings の値をそのまま使う (変更しない)。

### 17.5 パネル

- 「止まる深さ」スライダーの下 (奥 ↔ 手前の行の下) に、小さな説明 1 行:
  「画像を左 Ctrl+クリックすると、その場所の深さになります」。
- 同じ行の右か次の行に小さな「既定値」ボタン: `focus = 0.5` にしてハイライトを出す (クリックで変えた値を戻す入口)。
- パネルの戻り値 `DepthPanelResponse` に `focus_adjusting: bool` (スライダーのドラッグ中・変更・既定値ボタン) を足し、
  `draw_depth_panel` がハイライトの時刻を更新する。保存は今までどおり `changed` で行う。
- 固定パネルの高さ検査 (`tests/support/scroll_layout.rs`) と UI スナップショットを更新する。

### 17.6 テスト

GPU を使う比較は許容誤差つき (補間とレイマーチの有限探索、§17.2 の近似)。

- `MouseFollow::update`: 中心から `r` の位置で目標が端 (±強さ、縦は 0.6 倍)、`r` を越えると頭打ち、縦横で同じ `r`、
  キャンバスが 0 の大きさなら中心、パネル上で固定・ポインタなしで中心へ (既存の検査を新しい分母で直す)。
  キャンバスとして media rect が使われ、clip 矩形が大きくても感度が変わらないこと。
- 位置 → 深さの純粋関数: 回転 0〜3、切り取り (uv 範囲)、zoom、gamma、双線形、画像外で `None`、端数の fit 矩形。
- GPU (既存の 2 面の深度を使う検査に加え、模様付きの色と連続した深度の画像も使う): 求めた `focus` で、正負の offset・
  切り取り・回転・gamma ありで描いても、クリックした位置の画素が offset 0 のときと許容誤差内で同じ。
  ハイライト 0 の出力は今までと同じ、1 なら帯の中の面だけ色が付く。alpha 0 / 中間 / 1 で premultiplied が保たれる。
- `plan_fs_canvas_click` と処理 (handler-level): 3D + Ctrl → `DepthFocusAt`、3D + Ctrl なし → `PageSide`、
  3D でない + Ctrl → `PageSide` (変わらない)。Ctrl は release イベントの修飾で判定され、release 後に Ctrl を離しても
  `DepthFocusAt` のまま。ダブルクリック・3 px 超のドラッグ・パネル / popup / overflow / modal / Remote で従来どおり。
  画像外・計算中・失敗・元画像ホールド中の Ctrl+クリックでページが動かず、計算中だけ通知が出る。
- 描画時の解決: 同じ pass の要求は今の資源と transform で解決され、pass が違う要求・資源の鍵が payload と合わない pass
  (ページ移動・素材の変更直後) の要求は解決されずに捨てられる。LRU から戻った深度でも解決できる。
- タッチ: 表示中の 3D パネル内のタップでページが送られない。パネル外のタップは従来どおりページ送り。
- 書き出し: params のハイライトが 0。画面でハイライト中に作った書き出し要求にもハイライトが入らない。
- ハイライトの強さの時刻関数 (1.0 秒保持・0.3 秒フェード・その後 0)。マウス追従で止まっているときもフェードが最後まで
  進む (再描画が要求される)。hidden / minimized / 書き出しの modal 中は再描画を要求しない。
- 整形を切り替えても `focus` の数値が保たれる。
- パネル: スライダー操作と既定値ボタンで `focus_adjusting`、既定値ボタンで 0.5。

### 17.7 文書

- マニュアル `htdocs/mimageviewer/manual/fullscreen.html` の 3D 写真の節: 止まる深さの行に Ctrl+クリック・既定値・
  ハイライトを、マウス追従の説明に「画面中央の狭い範囲で端まで動く」を足す。製品ページの 3D のカードは
  機能の要約だけなので、文言が今も正しいかだけ確かめる。
- `docs/keymap-spec.md` の 3D 写真の節に、3D 中の Ctrl+左クリックを固定入力として理由とともに書く。
- 本書 §5.3 (マウス追従の正規化)、§6.5 (3D 中のクリック)、§8 (パネル) を本節に合わせて直す。
- まれな失敗は既定の割り切り (§13)。この改善で新しい失敗の経路は増えない。

### 17.8 独立レビュー (設計、2026-11-10、GPT-6.1 Sol / xhigh、read-only、基点 `c89cc2cbc`)

P2 5 件・P3 2 件。判定「指摘修正後に実装開始可」。ClaudeCode が根拠のコード (keymap.rs の RightCtrl、
タッチの除外領域の組立て、`single_clip_rect`) を確かめ、すべて採って §17 を直した。

| 指摘 | 対応 |
| --- | --- |
| P2 前 pass の `DisplaySnapshot` は `resource_id` が一致しても今の表示と食い違い得る (入力は描画より前、回転・resize は id を変えない) | 記録を使わず、同じ pass の `try_paint_depth` が実際に描く資源と transform でクリックを解決する (§17.2) |
| P2 OS の Ctrl の現在値では release 後に離すと通常クリックへ落ちる | release イベントの修飾で判定 (§17.2) |
| P2 右 Ctrl は元画像ホールドの既定キーで、ホールド中は 3D 資源が外れる | ホールドを優先、その間は何もしない。案内は「左 Ctrl」(§17.2・§17.5) |
| P2 painter の clip 矩形は media rect と一致しない | 呼出元の media rect を渡す (§17.3) |
| P2 3D パネルがタッチの除外領域に無い | パネル矩形をタッチの除外にも加える。保証はマウスのクリックに限る (§17.2) |
| P3 「完全一致」には精度の定義が要る | 幾何上の近似と明記し、GPU 検査は許容誤差つき (§17.2・§17.6) |
| P3 失敗時に「計算が終わると使えます」は不正確 | 通知は計算中だけ。失敗は既存の表示に任せる (§17.2) |

再確認 (同じセッションの継続、基点 `7f179a692`): 7 件とも解消、新しい P1 / P2 なし。判定「実装開始可」。
実装上の注意として、`pending_focus_click` の登録は**入力計画の時点** (`render_fs_body` 内の入力計画、
ui_fullscreen.rs:28487 付近) で行う。既存の action 適用には描画後へ遅らせる経路 (複数ホイール入力が混ざるとき、
ui_fullscreen.rs:41011・30497) があり、そこへ置くと同じ pass の `try_paint_depth` (44869) に間に合わない。
MainWindow (30278) と Fullscreen (30290) はどちらも `render_fs_body` を通り、入力計画 → 素材更新 (28871) → 描画 →
`try_paint_depth` の順で、間に pass の終了は無い。両 presentation について、この遅延経路と egui の discard による
再 pass を含めた呼出順の回帰検査を入れる。

### 17.9 実装記録

2026-11-10、`feat-3d-751`、基点 `6e2ed9049`。実装担当による未コミット差分。
製品の起動・デスクトップ入力・push は行わず、全件 gate とコミットは coordinator が担当する。

| 対象 | 変更 |
| --- | --- |
| `src/ui_fullscreen.rs` | release の位置・修飾と egui の click 判定を対応づけ、従来の除外後に `DepthFocusAt` を分類。`render_fs_body` から呼ばれる入力計画内で直ちに要求を登録し、遅延 action 適用では何もしない。表示パネルを touch の除外へ追加。media rect を Depth paint へ渡す |
| `src/app/depth_view.rs` | 同じ pass の要求を消費し、resource key 確認後、実際の transform と params で解決。既存の設定保存経路とハイライトへ接続。既存の可視・最小化・書き出し条件の下だけ連続 repaint |
| `src/depth_parallax.rs` | canvas 短辺の 0.25 を共通半径にした追従、pass 限りの要求、ハイライトの時刻と強さの純粋関数 |
| `src/depth_parallax_wgpu.rs` | CPU 深度を `DepthTexture` に保持し、回転・crop・zoom・bilinear・gamma の純粋 lookup を追加。`view.w` の premultiplied ハイライト。from_settings は常に強さ0 |
| `src/ui_depth_parallax.rs` | 弱い文字色の操作案内、既定値ボタン、`focus_adjusting`。固定 header／scroll body と本文上限380ptを維持（追加内容も収まることを高さ検査で確認） |
| `src/depth_focus_sampling_tests.rs`、`src/ui_fullscreen/depth_focus_tests.rs` | CPU/GPU 比較、両 presentation の実入力→描画順序と discard、pass/key 不一致・画像外・LRU、タッチ・除外・repaint の回帰 |
| `src/app/depth_export.rs` (test)、`src/app/depth_main_window_tests.rs`、`src/ui_depth_parallax_tests.rs`、`tests/ui_snapshot.rs`、パネル PNG | ハイライト中の実書き出し要求で強さ0、新しい追従半径の期待値、実パネルの操作、固定 header／本文高さ・末尾到達と外観 |
| `htdocs/mimageviewer/manual/fullscreen.html`、`docs/keymap-spec.md`、本書 | 操作と表示仕様を同期。製品ページの3Dカードは説明が引き続き正確なため変更なし |

§17からの仕様変更なし。新しい非同期処理、回復経路、設定項目は追加していない。
Ctrl+クリックは深さが決まった操作時に、既存パネルと同じ `settings.save()` を1クリックにつき1回同期実行する。
[UI応答性 §2.5](ui-responsiveness.md#25-静止画シーク回転-reader-の所有境界)の明示的な例外に従い、
毎フレーム、同じクリックの再pass、ハイライト中の再描画では保存しない。writerの新設や部分的な非同期化は行わない。
CPU深度はGPU資源と同じArc/LRUの寿命を使い、VRAM会計は従来の値を維持する。
操作感（半径0.25）、ハイライトの色・幅、非整数DPIでの見え方は実機確認待ち。

検証（Windows、通常 feature、未コミット差分。実アプリの操作感・画質は未確認）:

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth/)'` | **174成功 / 0失敗**、exit 0。`target/depth-751-final.log`。CPU/GPU・入力計画・both presentation/discard・touch・LRU・modal/Remote・repaint・実書き出し要求を含む |
| `cargo nextest run -p mimageviewer --lib -E 'test(/fullscreen_click\|canvas_click\|touch/)'` | **261成功 / 0失敗**、exit 0。`target/depth-751-input-tests.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0件 |
| `python scripts/check_site_cards.py` | exit 0、53カード / 超過0件（製品ページの変更なし） |
| `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot depth_parallax` | **2成功 / 0失敗**、exit 0。`target/depth-751-snapshot-update.log`。PNG 1枚（46,134 bytes）を目視確認。案内は1行で収まり、既定値・状態行・×に欠けや重なりなし |
| `cargo test --test ui_snapshot depth_parallax` | 更新モードなしの通常比較も **2成功 / 0失敗**、exit 0。`target/depth-751-snapshot-compare.log`。1093×614で不要scrollなし、546.5×307で実wheelによる末尾・既定値の到達と固定header/×を確認 |
| `cargo fmt`、`cargo fmt --check`、`git diff --check` | exit 0。整形は引数なしで実行。`target/depth-751-fmt.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | **exit 0**。通常feature・`dev-runtime`、portableなし。core / remote / EPUB worker / MCP adapter / music-video editor とDLLを配置。CRT検査 **runtime=4 / pe=5 成功**。`target/depth-751-build.log`、`target/vcrt-pe-reports/dev-runtime.json`。製品は起動していない |

途中の depth 検査は158件中157成功・旧追従感度の期待値1件を修正。
追加テストの最初のコンパイルでは新eguiのRawInput APIとtest fixtureの借用2箇所を修正し、
172件中170成功の段階で、書き出しのsame-pass受付をpass外から呼んでいたfixture2件を
既存のcapture/admit経路へ直した。製品側の受付条件は変更していない。
同じ位置の複数クリックも最後の受理primary releaseの修飾で分類する回帰を追加した。
さらにeguiのheadless probeで、release `[300,300]` の後に同じ入力束で `[500,300]` へ移動すると、
`Response::interact_pointer_pos` は後者になることを確認した。位置をResponseに合わせてreleaseを探すと
CtrlクリックをPageSideへ落としてしまうため、位置と修飾を同じ受理releaseから採る形へ修正。
マウス移動・Ctrl解放がreleaseの後に入る入力を、plannerと両presentationの実描画テストに追加した。
Ctrl+Shift+Altも確認する。これは§17の入力所有の実装上の修正で、仕様変更ではない。
上表はそれらを含む最終ソースでの成功結果。

パネル本文の上限は、途中に440ptも検査したが、追加内容が従来の380pt内に収まると確認できたため
380ptを維持した。上表のdepth 174件、入力261件、snapshot更新・通常比較は380ptの最終ソースで成功。
画像を覆う面積と入力除外の範囲を不必要に広げない。
入力座標の最終修正でpanel painter・UI harness・PNG・依存条件は変わらないため、
snapshotの成功結果は検証方針どおり再利用した。

確認用 `build-dev.ps1 -PreserveRuntime` の初回はTurboJPEGのCMake/MSBuild段階でexit 101。
同じworktreeの生成先に対して `cmake --build target/dev-runtime/build/turbojpeg-sys-ad9ab6b47bbc75e3/out/build --target install --config RelWithDebInfo --parallel 1`
を実行するとexit 0で依存物を構築できた（`target/depth-751-native-build.log`）。
製品コード・ビルドscript・個人設定は変えず、一時的な `CARGO_BUILD_JOBS=1` の依存準備も経て、
最終の入力座標修正後は通常の並列数で上表の確認用buildを完了した。ネイティブ依存構築後の再実行には
`-WaitForOtherBuildsMinutes 0` を使用。初回失敗の原因を製品の不具合とは判定していない。

確認用coreは `target/dev-runtime/mimageviewer-core.exe`。normal profileなので、引数なしの起動は
実利用中の `%APPDATA%/mimageviewer` を使い、設定やデータを更新し得る。
利用者がインストール版／常駐tray版を終了してから
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` で起動する。
両presentationで左Ctrlクリックがページを送らずその深さを設定すること、右Ctrlホールド中は変更しないこと、
既定値0.5・調整時のハイライトとフェード、中央の狭い範囲で追従の端に届くこと、書き出しにハイライトが入らないことが
実機の確認項目。全件gate・実機確認・coordinatorによるcommitはこの実装作業では行っていない。

#### Codex P2/P3 対応

2026-11-10、`4299e510c` に対する独立レビュー後の対応。製品コードの変更はなく、
`src/app/depth_main_window_tests.rs`、`src/ui_fullscreen/depth_focus_tests.rs`、本書だけを変更した。

- P2: §17.2 と本節に、Ctrl+クリックが既存パネルと同じ操作時の同期 `settings.save()` を
  1クリックにつき1回呼ぶことを明記した。UI応答性 §2.5 の明示的な例外を適用し、
  writer の新設や非同期化は行わない。実入力から描画までのテストで、保存世代が press では進まず、
  release に discard 再pass があっても1だけ進み、その後のハイライト付き8回の実描画では進まないことを検査する。
- P3: 共通fixtureで深度値を指定できるようにし、x方向の0→1勾配と画像中央以外のクリックを使った
  回転90°／resizeの2テストを追加した。release と同じ pass 内の回転変更、または同じ入力の新しい表示領域を
  実描画経路に渡し、実際の Depth callback の矩形と描画後の transform の一致、同一 resource の維持を確認する。
  現在と直前の transform それぞれから期待深度を計算し、差が0.1を超えること、許容誤差 `1e-4` の
  同じ合否判定が旧 transform の値を拒否し、実際に設定された値を受理することを assert する。
  旧テストは検査内容に合わせて pass／resource／画像外／LRU／media半径の名前へ変更した。

§17からの仕様変更なし。テストと文書のみのため確認用バイナリの再buildは不要。
全件suite、コミット、push、製品起動は行っていない。

検証（同worktree、通常feature）:

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth_focus_click_(saves_once\|uses_same_pass)/)'` | **3成功 / 0失敗**、exit 0。`target/depth-751-review-focused.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth/)'` | **177成功 / 0失敗**、exit 0。`target/depth-751-review-depth.log`。上の追加3件を含む |
| `cargo fmt`（引数なし）、`git diff --check` | exit 0 |

### 17.10 操作の案内: パネルを隠す・出す (§1.760、5ch >>755) と Ctrl+クリック (2026-11-11)

利用者の判断 (2026-11-11): §1.751 は左 Ctrl+クリックのままとする。そのうえで、パネルを隠せることと
Ctrl+クリックで止まる深さを決められることを、画面上から分かるようにする。

現状 (コード確認): パネルの表示・非表示は `App::toggle_depth_panel` (`src/app/depth_view.rs`) の 1 か所で、
呼び出しはキー `KeyAction::FsToggleMetadata` (既定 `I` / `Tab`、`src/ui_fullscreen.rs` の `key_i`) と
ゲームパッドのリング `RingActionId::ImageToggleMetadata` (`src/app/gamepad_input.rs`) の 2 つ。
Ctrl+クリックの案内は §17.5 でスライダー下 (ScrollArea の中) に置いたため、本文をスクロールすると見えない。

**決定**

1. **パネル下部に固定の案内欄** (フッター) を置く。ヘッダと同じく ScrollArea の**外**で、本文の高さは
   フッターの実測高さを差し引いて確保する (固定パネルの規則、`tests/support/scroll_layout.rs`)。小さな弱い文字色で 2 行:
   - `左 Ctrl+クリック: その場所の深さで止める` (マウス操作なので固定文言)
   - `<キー>: パネルを隠す・出す`。`<キー>` は `keymap.chord_labels(KeyAction::FsToggleMetadata)` を ` / ` で
     つないだもの (既定は `I / Tab`)。割り当てが空なら、この行は出さない (存在しない操作を案内しない)。
   §17.5 のスライダー下の Ctrl+クリック説明は**消して**フッターへ移す (同じ意味を 2 か所に書かない)。
   「既定値」ボタンはスライダーの近くに残す。
2. **隠したときに通知を 1 回出す**。`toggle_depth_panel` が表示→非表示へ切り替えたとき、既存の
   `show_feedback_toast` で `3D 写真のパネルを隠しました。<キー> でもう一度表示します` を出す
   (`<キー>` は `keymap.first_chord_label(KeyAction::FsToggleMetadata)` による最初の割り当て1つ、既定は `I`。
   割り当てが空なら `3D 写真のパネルを隠しました` だけ)。どの割り当てでも再表示できるので、通知には1つで足りる。
   フッターには全chordを表示する。キーとリングの両方の入口が同じ関数を通るので、
   通知もそこに 1 か所だけ置く。表示に戻すときは通知しない。
   通知は毎回出す。初回だけにするには永続の「見た」状態が要り、状態を増やすため採らない。
   1つのchord自体が極端に長い場合は通知が欠け得る。これは折り返さない共通トーストの既知の制約として残す。
3. 文言は描画のたびに今の keymap から作る (キャッシュしない)。ui 関数 (`draw_depth_parallax_panel_with_export`) は
   keymap を持たず、App が作った案内文字列 (`Option<String>`) を引数で受け取る (パネル側に I/O・設定参照を足さない)。

**採らなかった案**: 隠したあと画像の隅に小さな「3D」ボタンを常に出す (隠した目的の「画像を広く見る」に反する)。
右端ホバーで出す・鍵で固定する (backlog の未定案。状態と入力経路が増えるので今回は入れない)。

**テスト**: フッター文言が keymap の今の割り当てに従う (既定・別キーへ変更・複数・空) handler/UI テスト。
隠す切替で通知が 1 回、出す切替で通知なし (キーとリングの両方)。1093×614 と小さい画面でヘッダ・フッターが
ScrollArea の外に見え、本文が末尾まで届く高さ検査。UI スナップショット更新。

**文書**: マニュアル (3D 写真の節) に「パネル下部に操作の案内が出る」「隠したときの通知」を足す。
`docs/keymap-spec.md` に、3D パネルの案内が `FsToggleMetadata` の割り当てから作られることを書く。

### 17.11 実装記録 (§17.10)

2026-11-11、`feat-3d-751`、基点 `da4485dcc`。§17.10からの仕様変更なし。

前提のコード確認:

- パネル表示の切替は `src/app/depth_view.rs:281` の `App::toggle_depth_panel` が所有する。
  `src/ui_fullscreen.rs:36920` の `key_i` と `src/app/gamepad_input.rs:8099` の `ImageToggleMetadata` は
  両方この関数を通る。入口の受付条件やkeymap自体は変更していない。
- `App::draw_depth_panel` (`src/app/depth_view.rs:445`) が状態と書き出し可否を純描画関数へ渡し、返されたパネル矩形を
  入力除外へ渡す既存の境界を維持する。
- `src/ui_depth_parallax.rs:58,102` は固定headerと明示的に確保したScrollAreaの親領域を持つ。
  描画するフッター文字の折り返し済みgalleyを実測し、行間と本文との間隔も含めて本文高から差し引く。
  本文の上限380ptと既定値ボタンを維持し、旧スライダー下の説明は撤去した。

| ファイル | 変更 |
| --- | --- |
| `src/app/depth_view.rs` | 毎回現在の `FsToggleMetadata` のchord表示を結合し、案内文字列を渡す。隠すときだけ共通の通知を1回発行。割り当てなしではキー案内を省略 |
| `src/ui_depth_parallax.rs` | 小さい弱い文字色の固定フッター2行。純描画APIへ `Option<String>` を追加し、keymap参照・I/O・状態の追加なし |
| `src/app/depth_view_tests.rs` | 実App描画で既定・変更・複数・空の割り当てに追従。キーとリングの両入口から2周ずつ、隠す通知1回／出す通知なしを検査 |
| `src/ui_depth_parallax_tests.rs`、`src/ui_depth_export_tests.rs` | 共通描画APIへ案内文字列を渡し、既存のパネル操作・書き出し回帰を維持 |
| `tests/ui_snapshot.rs`、パネルPNG | 1093×614と546.5×307でheader/footerの可視性とScrollArea外の固定配置、実wheelによる末尾と各controlへの到達を検査。小画面の代表snapshotを追加 |
| `htdocs/mimageviewer/manual/fullscreen.html`、`docs/keymap-spec.md`、本書 | 固定案内・現在の割り当て・隠した通知の説明を同期。製品ページの3Dカードの主張は変わらないため変更なし |

検証（Windows、通常feature、同worktreeの未コミット差分）。利用者の実機確認中のcoreを保護するため、
`build-dev.ps1` は実行していない。確認用成果物の再作成と全件suiteはcoordinatorが担当する。
本作業ではコミット・push・製品起動を行っていない。

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth/)'` | **179成功 / 0失敗**、exit 0。`target/depth-panel-1760-tests.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/keymap/)'` | **210成功 / 0失敗**、exit 0。`target/depth-panel-1760-keymap.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0件 |
| `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot depth_parallax` | **3成功 / 0失敗**、exit 0。`target/depth-panel-1760-snapshot-update.log`。既存の通常サイズPNGを更新し、小画面PNGを追加 |
| `cargo test --test ui_snapshot depth_parallax` | 更新指定なしの通常比較も **3成功 / 0失敗**、exit 0。`target/depth-panel-1760-snapshot-compare.log` |
| `cargo fmt`（引数なし）、`git diff --check` | exit 0 |

`tests/snapshots/depth_parallax_panel_dark.png`（1093×614、46,626 bytes）と
`depth_parallax_panel_small_dark.png`（546×307、19,219 bytes）を目視確認した。
小画面の入力は546.5×307 logical points。両方で見出し・×・固定案内2行が収まり、文字の欠けや重なりなし。
小画面の本文にはscrollbarがあり、別の高さ検査で実wheelによる末尾と各controlへの到達を確認した。
純描画snapshotとheadless handlerテストの結果であり、製品起動・実機操作の確認とは分ける。

#### Codex P2 対応

2026-11-11、`29b023df0` に対する独立レビュー後の対応。複数chordを連結した通知が
546.5×307の画面からはみ出す指摘に対し、coordinatorの判断で§17.10の通知仕様を更新した。
`src/app/depth_view.rs` の隠す通知だけを既存の `first_chord_label(FsToggleMetadata)` へ変更し、
最初の割り当てキー1つを知らせる（既定 `I`、未割り当てなら隠した事実だけ）。
共通トースト描画と、全chordを折り返して表示するフッターの実装は変更しない。
1つのchord自体が極端に長い場合に欠け得る制約は§17.10に明記した。

`src/app/depth_view_tests.rs` の既存handlerテストを更新し、既定・単一・複数・未割り当ての通知を検査する。
`Ctrl+Shift+Alt+F13 / F14 / F15` の3割り当てでも、後ろのキーやリングで隠した通知が
最初の `Ctrl+Shift+Alt+F13` だけを含むこと、表示時の通知なし・隠すたび1回を確認する。
同じ3割り当てで、実App描画のフッターには全chordが残ることも確認する。
`docs/keymap-spec.md` は通知のキーが最初の1つであると明記した。
マニュアルの「もう一度表示するキーを短い通知で知らせます」は今回の動作と矛盾しないため変更なし。

変更ファイルは上記の製品コード・テスト・キー仕様と本書の4ファイル。
コミット・push・製品起動・`build-dev.ps1`・全件suiteは行っていない。

検証（Windows、通常feature、同worktreeの未コミット差分）:

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth/)'` | **179成功 / 0失敗**、exit 0。`target/depth-panel-1760-p2-tests.log`。上記の既存2テストの追加条件を含む |
| `cargo fmt`（引数なし）、`git diff --check` | exit 0 |

フッターの描画・レイアウト・既定表示とsnapshot fixtureは変更していないため、
直前のsnapshot更新／通常比較3件成功・PNG2枚の目視結果は引き続き有効として再利用した。

### 17.12 §1.766 パネルを見出し 1 行に折りたたむ (5ch >>761、2026-11-11)

設計: ClaudeCode Opus 5.5。worktree `G:\home\mimageviewer-depth2`、ブランチ `feat-766-767-depth-ui`、基点 `62de527fb`。
要望: `Tab` で完全に隠せることは確認済み。パネルを小さくしておき、マウスで開き直したい。

**現状 (コード確認)**

- パネルの表示・非表示は `DepthViewState::panel_visible` (3D に入るたびに `true`) を `App::toggle_depth_panel` だけが切り替える。
- パネルは `App::draw_depth_panel` が描き、返された矩形を同じ ctx の一時データ `depth_panel_rect` に書く。
  読む側 (マウス追従の凍結 `try_paint_depth`、マウスのパネル判定 `cursor_in_depth_panel`、タッチ除外と
  クリック分類の `visible_depth_panel_rect`、待機表示の `draw_depth_wait_indicator`) は、どれも
  「`panel_visible` かつ一時データの矩形」を読む。`draw_depth_panel` は入力判定より前に呼ばれる
  (ui_fullscreen.rs の「Paint foreground controls first」)。したがって**描いたパネルの矩形がそのまま入力除外になる**。

**決定**

1. **折りたたみは `DepthParallaxSettings::panel_collapsed: bool` (既定 `false`) の 1 つで持つ**。設定に保存する。
   - 表示・非表示 (`panel_visible`、I / Tab) とは**独立した 2 つの事実**で、組み合わせ 4 通りはどれも意味がある
     (非表示のまま折りたたみを覚えている = 次に出すと 1 行)。相互排他な状態を 2 つの bool で持つ形ではない。
     持ち主と寿命が違う (表示・非表示は 3D の 1 回分、折りたたみは利用者の好み) ので 1 つの enum にまとめない。
   - 記憶の範囲を設定にした理由: 案 1 (§6.4.1) で F11・F12 の切替ごとに 3D が終わり `DepthViewState` が作り直される。
     折りたたみを `DepthViewState` に置くと、切替のたびにパネルが開き直り、「小さくしておきたい」という要望に合わない。
     設定に置けば値 1 つで済み、ページ移動・再入場・再起動で同じ見え方になる。保存はクリック 1 回につき 1 回の
     `settings.save()` (パネルの他の項目と同じ、ui-responsiveness §2.5 の操作時保存の例外)。
   - `DepthParallaxSettings` は v5.1.0 で出荷済み。`#[serde(default)]` の構造体に項目を足すだけなので、旧設定はそのまま読める
     (足りない項目は既定 `false`)。移行コードは要らない。書き出しの要求 (`DepthRenderParams::from_settings`) はこの項目を読まない。
2. **見出し行の左側 (三角 + 「3D 写真」) を 1 つのクリック可能な部品にする**。クリック (タップを含む) で折りたたみ ⇔ 展開。
   - 三角は egui の `collapsing_header::paint_default_icon` で描く図形 (フォントのグリフではない。`check_ui_glyphs.py` の対象外)。
     展開中は下向き、折りたたみ中は右向き。ホバーで背景を薄く強調し、ポインタを手の形にし、ヒント
     「設定を折りたたむ」/「設定を表示する」を出す。
   - × (3D の終了) は今までどおり右端。折りたたみ中も見出し行に残す。
   - パネルの幅・位置は変えない (折りたたみで×と見出しの位置が動かない)。高さだけが見出し 1 行になる。
3. **折りたたみ中は見出し行だけを描く**。区切り線・本文 (ScrollArea)・状態の行・フッター (§17.10 の操作案内) は描かない。
   - 操作案内は展開すれば見える。折りたたみは利用者が自分で選ぶ操作なので、案内を 1 行に詰め込まない。
   - 計算中の状態は画像上の待機表示 (§5.4) がパネルの有無によらず出すので、折りたたみ中も知らせられる。
     失敗は既存の通知で知らせ済み (§13)。
   - 折りたたみ中は ScrollArea を作らないので、本文の高さ計算・スクロール位置は展開時にそのまま戻る。
4. **入力除外は描いた矩形だけ**。折りたたみ中に `draw_depth_panel` が書く矩形は見出し行のパネル (Area の実際の矩形) なので、
   上の読む側はすべて小さい矩形を使う。読む側に折りたたみ用の分岐は足さない。
   - 確かめること: Area の応答矩形が**その pass の中身**から決まり、前の pass の大きさ (展開時) を持ち越さないこと。
     持ち越す場合は、折りたたみを切り替えた次の pass で、旧本文の位置へのクリック・マウス追従・タッチが遮られる。
     テストで「折りたたんだ次の pass に、旧本文の位置への左クリックがページ送り、Ctrl+クリックが止まる深さ、
     マウス追従が凍結しない、タッチがページ送り」を確かめる。持ち越すと分かった場合は、Area の id を
     折りたたみ状態ごとに分けるなど、**描く矩形自体を正しくする**側で直す (読む側で矩形を削る補正はしない)。
   - 切り替えたその pass は、クリックがパネル内なので背面に渡らない (従来どおり)。
   - **実装で分かったこと (2026-11-11)**: Area の返す矩形と保存する大きさはその pass の中身から決まるが、egui の当たり判定は
     **前の pass に登録された widget 矩形**を使い、Area の move widget は `begin` で前 pass の大きさのまま登録される。そのため
     折りたたんだ pass の次に再描画が無いまま来た最初の入力 1 回は、旧本文の位置でも Area に吸われる (実装担当が handler テストで再現)。
   - **決定: egui 本体 (`vendor/egui`) は改変しない**。Area の共通処理を変えると mIV のすべての Area に影響し、egui の更新のたびに
     持ち越しが要る。代わりに、パネルの開閉を切り替えた pass で `ctx.request_discard("depth panel collapsed/expanded")` を出す。
     egui が「この pass の配置は古いので同じ frame の中でもう一度組み直す」ために用意している仕組み (egui 自身の Grid が同種の
     大きさのずれに使う。`Options::max_passes` 既定 2、mIV の既存の利用例 detached_placement.rs:1302) で、組み直しの pass の `begin` は
     新しい大きさで widget を登録する。時間窓・次 frame の再描画待ちではなく、同じ frame の中で配置を確定させるもの。
     組み直しの pass には入力が無いので、開閉の切替と保存は 1 回だけ (§17.6 の discard 再 pass の扱いと同じ)。
     max_passes が 1 の環境 (テストの一部) では組み直されず、上の 1 回の吸い込みが残り得る (製品は既定の 2)。
5. **I / Tab・D・×との関係**
   - I / Tab: 今までどおり完全に隠す・出す。折りたたみの値は変えない (出したとき、折りたたんでいれば 1 行で出る)。
     隠したときの通知 (§17.10) もそのまま。
   - D・×: 3D を終了する。折りたたみは設定なので次に 3D を始めたときも保たれる。
   - キー割り当て (`KeyAction`) は足さない。パネル上の部品のマウス・タッチ操作で、他のパネルの開閉部品と同じ扱い
     (キーボードでは I / Tab で隠せる)。keymap-spec の 3D 写真の節にその旨を 1 行書く。
6. **ウィンドウ内 (MainWindow) と全画面**は同じ `draw_depth_panel` と `render_fs_body` を通るので、分岐を足さない。
   テストは両方の表示で行う。

**状態を減らす検討**: 折りたたみ専用のアニメーション (egui の `CollapsingState` の開閉中の高さ) は使わない。
開閉途中の高さが入力除外の矩形に混ざり、途中の pass の組み合わせが増えるため。切り替えはその場で 1 行 ⇔ 全体。
egui の `CollapsingHeader` もアニメーションと独自の記憶 (egui memory) を持つので使わず、記憶は設定の 1 か所にする。

**採らなかった案**: 折りたたみを `DepthViewState` に置く (上の理由で、F11・F12 のたびに開き直る)。
I / Tab を「展開 → 1 行 → 非表示」の 3 段の循環にする (リリース済みの I / Tab の挙動が変わる。2 回押す必要が出る)。
折りたたみ中に画像の隅に小さなボタンだけを出す (§17.10 で採らなかった案と同じ)。

**テスト**

- UI (純描画): 見出しのクリックで `panel_collapsed` が反転し `changed` が立つ。折りたたみ中の矩形の高さが見出し 1 行
  (展開時より十分小さい)、本文・フッター・書き出しボタンが無い (応答の `export_rect` / `footer_rect` が `NOTHING`)、×は押せる。
  見出しの位置と×の位置が展開時と同じ。
- handler (App、MainWindow と Fullscreen の両方): 折りたたみ → 次の pass で `depth_panel_rect` が小さい矩形。旧本文の位置への
  左クリックでページ送り、左 Ctrl+クリックで `DepthFocusAt`、ホバーでマウス追従が凍結しない、タッチでページ送り。
  見出し行の上のクリック・タップは背面へ渡らない。展開に戻すと本文の位置が再び除外される。
- I / Tab で隠す・出すの後も折りたたみが保たれ、隠したときの通知は従来どおり 1 回。D で終了・再入場しても保たれる。
- 設定: `panel_collapsed` の無い旧 JSON を読むと `false`。クリック 1 回で保存 1 回、再描画では保存しない。
- 待機表示: 折りたたみ中は小さい矩形だけを避ける (本文があった場所に出せる)。
- UI スナップショット: 折りたたみの代表を追加し、展開の既存 PNG を見出しの三角の分だけ更新する。1093×614 と 546.5×307 で
  見出し・×が欠けないこと。

**文書**: マニュアル `fullscreen.html` の「右パネルで動きを調整する」に、見出しをクリックして 1 行に折りたためること、
もう一度クリックで開くこと、I / Tab の完全非表示との違い、折りたたみは次回も保たれることを足す。
`docs/keymap-spec.md` の 3D 写真の節に固定のマウス操作として 1 行。本書 §8 に折りたたみの 1 行。

まれな失敗は既定の割り切り (§13)。この変更で新しい失敗の経路は増えない。

### 17.13 実装記録 (§17.12)

2026-11-11、`feat-766-767-depth-ui`、基点 `18d528908`。§17.12の仕様を実装。

前提のコード確認:

- `DepthParallaxSettings` は構造体の `#[serde(default)]` で旧JSONを受け入れる。
  `DepthRenderParams::from_settings` は描画用の項目だけを取り、パネルの折りたたみを参照しない。
- `App::draw_depth_panel` は現在の描画矩形を `depth_panel_rect` に公開し、`changed` のときだけ
  normalizeとsaveを1回ずつ行う。保存経路と矩形を読む側は変更していない。
- MainWindowとFullscreenは同じパネル描画・canvas入力分類・タッチ・マウス追従を通る。
  detachedの述語・viewport経路と§1.767は変更していない。
- `vendor/egui/src/containers/area.rs::Prepared::end` はそのpassの `content_ui.min_size()` を
  `AreaState.size` と返却する矩形へ反映していたが、`begin` が前passの大きさで登録したmove widgetは
  更新していなかった。次passのhit-testは前passのwidgetを使うため、実handler回帰で旧本文へのクリック遮断を再現した。
  設計担当の§17.12決定4追記に従い、egui本体の変更と追加単体テストを撤回した。
  パネルの見出しをクリックして設定を反転したpassだけ `ctx.request_discard("depth panel collapsed/expanded")` を出し、
  同じframe内の組み直しでAreaの登録矩形を確定する。読む側の補正、Area IDの分割、追加状態は使わない。
- 対象の純描画Harnessと `RootDepthViewer` 由来の両presentationのContextは `max_passes` を変更せず、
  製品と同じ既定2であることをassertする。`Context::run_pass` も `run_ui` と同じ複数pass経路を通る。
  共通fixtureや他のテストのpass数設定は変更していない。

| ファイル | 変更 |
| --- | --- |
| `src/depth_parallax.rs` | 既定falseの `panel_collapsed` と旧JSON・保存往復のテスト |
| `src/ui_depth_parallax.rs` | 三角と見出しを1つのクリック部品にし、ホバー強調・手形カーソル・開閉ヒントを追加。折りたたみ中は見出しと×だけを描き、scroll応答をOptionに変更。開閉を切り替えたpassだけrequest_discard |
| `src/ui_depth_parallax_tests.rs` | 2サイズ・同時/別frameの実pointerで開閉、changed回数、固定見出し/×、本文/フッター不在、旧本文の層解放と×を検査。初めから折りたたむ場合も見出し/×の矩形・文字描画・×のクリックを検査 |
| `src/ui_fullscreen/depth_focus_tests.rs` | egui既定2 passで、両presentationの旧本文へのmouse/Ctrl/touch/hover、見出しの入力所有、再展開後の除外を検査。開閉frameが2 pass、再passが入力なし、保存はクリック1回につき1回、通常再描画では保存しないことを検査 |
| `src/app/depth_view_tests.rs` | I/Tab・リングで折りたたみを維持し、既存の隠す通知を検査。D終了・再入場の保持を検査 |
| `src/app/depth_main_window_tests.rs` | 待機表示が折りたたんだ見出しだけを避け、旧本文位置を使えることを検査 |
| `tests/ui_snapshot.rs`、パネルPNG | 展開の既存2枚を更新し、546.5×307の折りたたみ代表を追加。既存のスクロール到達性検査を維持 |
| `htdocs/mimageviewer/manual/fullscreen.html`、`docs/keymap-spec.md`、本書 | 折りたたみ・完全非表示との違い・次回保持と固定入力の理由を同期 |

検証はWindows、通常feature、同worktreeの未コミット差分で実施する。
製品起動・コミット・push・`build-dev.ps1`・`test-lib-full.ps1`・`test-full.ps1` は依頼どおり実施しない。
検証用成果物と全件gateは設計担当へ引き継ぐ。

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth/)'` | **186成功 / 0失敗**、exit 0。`target/depth-panel-1766-discard-depth.log`。追加6件とdiscard再passの検査を含む |
| `cargo nextest run -p mimageviewer --lib -E 'test(/fullscreen_click\|canvas_click\|touch/)'` | **262成功 / 0失敗**、exit 0。`target/depth-panel-1766-discard-input.log` |
| `cargo test --test ui_snapshot depth_parallax`（更新指定なし） | **4成功 / 0失敗**、exit 0。`target/depth-panel-1766-discard-snapshot.log`。PNG3枚は今回の修正前後のSHA-256が一致、更新なし |
| `python scripts/check_ui_glyphs.py`（前回実装の結果を再利用） | exit 0、危険グリフ0件。UI文言の追加変更なし。`target/depth-panel-1766-glyphs.log` |
| `python scripts/check_site_cards.py`（前回実装の結果を再利用） | exit 0、53カード・上限超過0件。サイト変更なし。`target/depth-panel-1766-site.log` |
| `cargo fmt`（引数なし）、`git diff --check` | exit 0。最後の整形以外のRust変更後に上記の最終回帰を実行 |

全cargo/testに `MSBUILDDISABLENODEREUSE=1` を使用した。初回のTurboJPEG CMake/MSBuildは並列ビルドでexit 1となり、
`CARGO_BUILD_JOBS=1` で依存物を構築した後、通常の並列設定へ戻して最終検証した。並列失敗の原因自体は未確定。
修正前のhandler回帰は旧本文へのクリックが遮断されて5件中1件失敗 (`target/depth-panel-1766-focused-fixed.log`)。
今回の再検証はeguiを基点どおりに戻したツリーで行う。`git status --short -- vendor/egui` の出力は空で、
`git diff --exit-code -- vendor/egui` もexit 0。設計との差分はない。
比較許容値は変更していない。実機・独立完成レビュー・全件gateは未実施。

PNGの目視確認:

| PNG (`tests/snapshots/`) | 実寸 | バイト数 |
| --- | --- | --- |
| `depth_parallax_panel_dark.png` | 1093×614 | 46,845 |
| `depth_parallax_panel_small_dark.png` | 547×307 | 19,438 |
| `depth_parallax_panel_collapsed_small_dark.png`（追加） | 547×307 | 7,535 |

小画面の入力は546.5×307 logical points。計3枚、73,818 bytes。
展開の2枚と折りたたみの見出し部分の拡大表示で、三角・見出し・×の欠け/重なりとデバッグ表示がないことを確認した。
本文・固定フッターの配置と小画面のscrollbarも確認し、スクロール末尾への到達は既存のgeometry/実wheel検査で保証する。
純描画・headless handlerの証拠であり、実製品起動・実機操作の確認とは区別する。

#### 独立実装レビュー (2026-11-11、GPT-6.1 Sol / xhigh、read-only、対象 `96e4fb28a`)

P1 / P2 / P3 の指摘なし。判定「受入可」。§17.12 の決定 1〜6 への適合、`depth_panel_rect` の全読み手が折りたたみ後の実矩形を使うこと、
切替 pass の `request_discard` が同じ frame で Area の登録矩形を更新する egui の仕組みとして妥当で時間窓・追加 repaint の症状パッチに当たらないこと、
保存が操作 1 回につき 1 回、両表示の共通経路、テストの検査力、文書の一致、`vendor/egui` が親コミットと同一 tree であることを確認した。
`max_passes = 1` では次の入力 1 回が旧矩形に吸われ得るが、設計に明記済みで製品の 2 pass と整合する、との見解。

### 17.14 実装記録 (§6.4.2 第 1 段)

2026-11-11、`feat-766-767-depth-ui`、基点 `9280249bf`。§6.4.2 の決定 A〜E を実装。
設計担当は ClaudeCode Opus。独立 GPT-6.1 Sol / xhigh の設計レビュー r1〜r4 による
構造的変更への合意を前提とし、完成差分も独立した同設定の担当が確認して ACCEPT と判定した。

前提のコード確認:

- `ViewerSession.independent_active` は context の session に属し、presentation と同じ
  `swap_with_mounted` で投影される。App 全体の detached 設定から独立性を推測していない。
- linked の F12 は既存の presentation 確定点で表示先を切り替える。切替後に新しい D で入場し、
  worker の wake はその pass の子 viewport id を捕捉する。旧要求の wake 先を移管しない。
- 窓内 F11 は `toggle_detached_viewer_borderless_fullscreen` であり、presentation/context/viewport id を
  変更せず確定点を通らない。既存の interlude と backstop の 2D 描画はそのまま。
- `commit_viewer_session` は確定先の presentation と独立性で共通の適格性を判定する。
  呼出し元が独立フラグを書く順序と所有者は維持し、書く前に旧 Depth の Drop を済ませる。
  継続できるのは presentation・独立性がともに同じで、確定先が適格な再確定だけ。
- 3D の共有ダイアログは main が所有する。private な registry API の公開範囲は広げず、
  `mounted_viewer_owns_depth_dialogs` で projected == main を返して viewer pass の 2 か所から読む。
  ROOT の追加モデル案内だけに `fullscreen_idx.is_none()` を加え、ROOT 書き出しの規則は変更していない。
- `park_active_detached_context_as_live_media` の対象は main 以外の at-rest active context。
  その bundle 内の終了漏れは第 2 段へ残す。関数末尾の main 側の既存確定点には確定先 false を渡すため、
  main が linked の Depth を所有していればそこで終了する。park の手順は変えていない。
- 新しい同期 I/O、decode、GPU upload、待ち、repaint、detached 用状態は追加していない。
  まれな失敗は §13 の既定どおりログと通知で終える。

独立フラグの書き込み全箇所 (製品コード。関数名を追跡の正本とする):

| ファイル・関数 | 値 | 分類・3D の所有境界 |
| --- | --- | --- |
| `app.rs::new_from_settings_with_load_meta_book_query_repaint_and_similar_capability` | false | (b) App 構築時。Depth は未生成 |
| `viewer_session.rs::ViewerSession::default` | false | (b) 新しい空の session。Depth は未生成 |
| `app.rs::poll_detached_physical_folder_open` | true | (a) 現 presentation と true を確定してから書く |
| `app.rs::close_all_detached_viewers_with_cause` | false | (b) projected session・全独立 context の既存 close/retire 後。`close_fullscreen_content` が Depth を終了 |
| `app.rs::park_and_close_current_active_detached_viewer_inner` | false | (a) 非 detached presentation と false を既存確定点に渡す |
| `app.rs::start_active_detached_loading_context` | true | (a) DetachedWindow と true を確定してから書く |
| `app.rs::start_active_detached_collection_root_image` | true | (a) DetachedWindow と true を確定してから書く |
| `app.rs::activate_detached_image_window_snapshot` | true | (a) 現 presentation と true を確定してから書く |
| `app.rs::park_current_viewer_context_as_live_media_inner` | false | (a) main 側の非 detached presentation と false を既存確定点に渡す |
| `app.rs::park_active_detached_context_as_live_media` | false | (a) 同上。at-rest の独立 context 内は既存 activation の true 確定点を通る |
| `app.rs::preserve_active_detached_image_window_for_main_context_change_inner` | false | (b) 成功した park の後だけ。既存 `reset_detached_pause_foreground_modes` が Depth を終了済み |
| `app.rs::promote_active_detached_video_for_main_context_change` | false | (a) main 側の非 detached presentation と false を既存確定点に渡す |
| `app.rs::prepare_detached_image_windows_for_open` | false | (b) 独立 session は 3D 不可。linked なら元から false で変化せず、park 成功時は既存 owner が終了済み |
| `app.rs::take_parked_fullfeature_linked_still_for_grid_open` | false | (b) 呼出し元 `prepare_viewer_presentation_open` が linked/false を確定済み。同値の再代入 |
| `app.rs::prepare_viewer_presentation_open` (主代入) | independent_detached_still | (a) 計算した確定先の独立性で先に確定。linked → 独立も即座に終了 |
| 同関数 (非 detached / 非 egui 媒体の末尾代入) | false | (b) 当該条件では independent_detached_still は false。上の確定と同値 |
| `app.rs::prepare_viewer_presentation_close` | false | (a) 非 detached presentation と false を既存確定点に渡す |
| `app.rs::close_fullscreen_for_folder_nav_reopen` | 保存した独立性 | (a) 保存した presentation と独立性を確定してから復元。既存 content close でも Depth は終了 |
| `viewer_context_registry.rs::activate_mounted_as_independent_detached` | true | (a) DetachedWindow と true を確定してから書く |
| `viewer_session.rs::swap_with_mounted` (`swap_viewer_context_bundle` から) | 相手の値 | (b) 同じ context の session と fs_exclusive_view を一括交換する所有搬送。session の意味の変更ではない |
| 同関数 (`split_current_context_preserving_main_grid` から) | 新規/相手の値 | (b) 既存 fork の共通 primitive が Depth を終了してから搬送。新規 session は空 |

`rg` で `detached_viewer_independent_active` の代入・初期化、`independent_active` の初期化・swap と
可変参照の呼出し元を照合した。残る直接代入は `#[cfg(test)]` の fixture だけで、製品の状態遷移ではない。
新しい session 判定のために bundle swap・fork・park の手順は変えていない。

| ファイル | 変更 |
| --- | --- |
| `src/app/depth_view.rs` | linked 適格性、確定先 session の共通確定点、main の dialog 所有判定、再入場/拒否通知 |
| `src/app.rs`、`src/app/viewer_context_registry.rs` | 独立性の変更を確定先の値で確定。ROOT の追加モデル案内の描画条件 |
| `src/depth_parallax.rs` | 非対応場面の通知に F12 の対応を反映 |
| `src/depth_estimation.rs` | `cfg(test)` の実 submit 観測だけを追加し、要求の context・viewport・key を検査可能にする。製品 worker の動作は不変更 |
| `src/app/depth_export.rs` | `cfg(test)` の paint receipt 観測だけを追加。前 pass の snapshot を破壊せず、子 pass の実 Depth 描画を検査 |
| `src/ui_fullscreen.rs`、`src/ui_fullscreen/viewer_chrome.rs` | 3D のダイアログ 2 か所だけを main 所有に限定 |
| `src/app/depth_view_tests.rs`、`depth_main_window_tests.rs`、`depth_export_main_window_tests.rs`、`multiwindow_scenario_tests.rs` | 入口・確定・資源寿命、子 viewport・dialog 所有の headless 回帰 |
| `src/ui_fullscreen/depth_focus_tests.rs` | linked fixture と実 pass の入力・パネル・DPI 回帰。既存 `depth_main_window_tests.rs` の描画直前検査は変更せず回帰実行 |
| `htdocs/mimageviewer/manual/fullscreen.html`、`shortcuts.html`、`htdocs/mimageviewer/index.html`、`docs/keymap-spec.md` | 対応範囲・F12 終了と再開・窓内 F11 継続を同期。FAQ に該当記述なし |
| 本書、`docs/detached-rework-plan.md` §11 | 現行仕様、構造上の理由、書き込み列挙、検証証拠 |

検証は Windows、通常 feature、上記基点への未コミット差分で実施。全 Cargo に
`MSBUILDDISABLENODEREUSE=1` を使用する。製品起動・コミット・push・`build-dev.ps1`・
`test-lib-full.ps1`・`test-full.ps1` は依頼どおり実施せず、全件 gate・確認用 build を設計担当へ引き継ぐ。

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached/)'` | **1,311 成功 / 0 失敗、exit 0**。`target/depth-1767-regression.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/multiwindow_scenario/)'` | **48 成功 / 0 失敗、exit 0**。`target/depth-1767-multiwindow.log` |
| `cargo test --test ui_snapshot` (更新指定なし) | **250 成功 / 0 失敗、exit 0**。`target/depth-1767-snapshot.log`。PNG 更新なし |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0。`target/depth-1767-check.log`。以後の Rust 意味変更はテストと `cfg(test)` 観測だけで、通常 graph の結果を再利用 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ 0 件。`target/depth-1767-glyphs.log` |
| `python scripts/check_site_cards.py` | exit 0、53 カード・上限超過 0 件。`target/depth-1767-site.log` |
| `cargo fmt` (引数なし) | exit 0。`target/depth-1767-fmt.log`。最終の意味変更後に上記回帰を実行し、その後は整形と文書だけ |
| `git diff --check` | exit 0。`target/depth-1767-diff-check.log`。`git diff --exit-code -- vendor/` も exit 0、snapshot に差分なし |

§6.4.2 の受入条件と回帰の対応:

1. `depth_presentation_entry_continuation_and_commit_share_context_eligibility` と
   `depth_top_button_color_matches_the_context_and_session_entry_matrix` で入口・確定先の matrix と
   上部ボタンの実色、拒否文言を検査する。
2. 既存 independent activation / fork 所有テストを linked に広げ、独立 open の即時終了・F12 双方向の
   終了と実効キー/未割当の通知・同 session 再確定を検査する。
   `depth_linked_f11_keeps_readback_estimation_and_ready_owners_in_both_directions` は実子 pass の F11 入力で
   ReadingBack / Estimating / Ready・LRU・slot・要求が残り、その結果を採用できることを確認する。
3. linked の実子 pass で D・I/Tab、panel/canvas/mouse/Ctrl/touch、折りたたみ、待機表示、書き出し全 phase の
   modal・wheel・key・ring と dismiss 後の positive control を検査する。
   `depth_linked_actual_readback_submits_the_child_viewport_to_the_worker` は実 submit の宛先を観測し、
   `depth_linked_child_entry_panel_keys_and_dpi_use_the_current_pass_geometry` は子の実効 ppp と cursor 宛先、
   DPI/rect が変わる release 同 pass の gradient 深度を新 transform で照合する (旧 transform との差も確認)。
4. `multiwindow_depth_addon_*` / `multiwindow_depth_export_*` の 12 条件で実 ROOT 更新を駆動する。
   main の各表示・一覧・独立 still/video・ParkedLive を検査し、全非 grid host の viewer 寿命も確認する。
   所有側の close 後の addon と export の Stopping / Finished の ROOT 描画も検査する。
5. `depth_linked_close_retires_request_or_gpu_slot_immediately` と既存所有回帰で close・fork・独立化の
   要求取消/slot 除去・兄弟の要求/slot 不変を検査する。

設計との差分はない。実 submit と paint receipt の観測は `cfg(test)` のみで、製品の状態や処理は増やしていない。
独立 GPT-6.1 Sol / xhigh の完成レビューは **ACCEPT、未解決 P1/P2/P3 なし**。
レビュー担当は同じ成功ログを再利用し、テストを重複実行していない。

未確認: F11 の実 HWND 切替、実モニタ間 DPI 移動、native 動画との併存、OS 保存先画面の owner/focus、
backstop / interlude の数 frame の見え方、実モデルでの推論。headless/offscreen の成功とは区別する。
確認用 build と全件 gate は依頼どおり設計担当に残す。製品起動・commit・push は実施していない。

検証中に直した fixture の不備 (製品の不具合や期待値の緩和として扱わない):

- 初回 core check は UI module から private な registry API を呼んだため exit 101。
  main 所有判定を `depth_view.rs` の小さな accessor にまとめ、API の公開範囲を維持して再 check は exit 0。
- 初回 depth compile は一時 path の取得先と借用競合の 2 件で exit 101、テスト実行 0 件。
  path を fixture 構築側で捕捉して修正。
- depth 初回実行は **194 実行 / 189 成功 / 5 失敗、exit 100** (`target/depth-1767-focused-r2.log`)。
  子 fixture が各 frame で DisplaySnapshot を消し、前 pass の capture を使う書き出しボタンを無効にしていた。
  削除を撤去し、実 paint receipt を子 pass 内で読み取る観測だけを使う。
  F11 は ROOT 単独の入力 pass が子 viewport 登録を退役させていたため実子 pass で駆動し、
  cursor command は egui の集約先である親 FullOutput からも採取する。
  ScenarioDriver には既存の indexer 準備完了 fixture を与え、startup overlay を検査対象に混ぜない。
- 上記 5 件の限定再実行は **3 成功 / 2 失敗、exit 100** (`target/depth-1767-focused-r3.log`)。
  残りは ROOT の `ctx.memory` から子の modal layer を読んでいたことと、Fullscreen fixture の first pass で
  閲覧が終了し案内が ROOT に出たこと。layer は子 pass 内で観測し、通常の Fullscreen child focus を
  明示して各 ROOT pass 後の session 継続も検査する。時計や keep 設定で終了規則を回避していない。
- 残り 2 件の限定再実行は **1 成功 / 1 失敗、exit 100** (`target/depth-1767-focused-r4.log`)。
  linked を含む書き出し全 phase・リングの実検査は成功。Fullscreen の ROOT 更新 fixture は
  first pass 後に閲覧が閉じることを再検出した。focus だけでは退役を説明できず、logic/UI の境界を
  一時観測して切り分けた。
- host を独立した 12 テストに分けた実行は **9 成功 / 3 失敗、exit 100** (`target/depth-1767-focused-r5.log`)。
  addon の Fullscreen / linked / linked + ParkedLive が UI 段で終了することを確認。
  `setup_app_for_test` は `initialized=false` のままで、初回 UI の `render_initial_open` が
  開始フォルダを開き seed 済み閲覧を置換していた。export の open admission はそれを拒否するため
  export だけ成功していた。所有の検査 fixture だけを起動済み (`initialized=true`) と明示し、
  全非 grid host で各 ROOT pass 後の idx/presentation 継続を検査する。
  共通 fixture と製品の初期 open 規則は変更せず、一時 trace は撤去済み。
  修正後の同 12 件は **12 成功 / 0 失敗、exit 0** (`target/depth-1767-focused-r6.log`)。
- 独立レビュー P2 の linked ring 検査の省略を撤去し、全 phase の modal と dismiss 後の positive control を実行する。
  P3 の §11 の旧 detached 全拒否記述は linked 再入場と独立 session/context 拒否へ同期した。

#### 独立実装レビューと全件 (2026-11-11、設計担当)

- 独立実装レビュー (GPT-6.1 Sol / xhigh、read-only、対象 `08d2e0005`、実装者とは別のセッション): P1 / P2 / P3 なし、判定「受入可」。
  構造的変更であることに合意 (はい)。§6.4.2 の A〜E への適合、独立フラグの書き込み全箇所の列挙、ダイアログの表示先が通常の各表示で 1 つで、
  今回新しく恒久的に見えない modal を作っていないこと (holdover・native 動画で body を描かない性質は既存の main Fullscreen と同じ)、
  テストの検査力 (旧来の全拒否では positive control が通らない)、観測が `cfg(test)` に閉じることを確認。
- `test-lib-full.ps1` (共有ロック取得、`08d2e0005`): **17,232 成功 / 0 失敗、75 skipped、949 秒**、exit 0。receipt `target/test-lib-full/d293317d….json`。
- `cargo test --test ui_snapshot` (更新指定なし): **250 成功 / 0 失敗**、exit 0。
- `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-run`: exit 0。launcher を含めると、この worktree に
  release の exe 5 本が無いため launcher の build.rs で止まる (CLAUDE.md の test-full の前提どおり。launcher は今回変更なし)。
- `build-dev.ps1`: exit 0。確認用 `target/dev-runtime/mimageviewer-core.exe`。製品は起動していない (実機確認は統括経由でサブ PC へ依頼)。

### 17.15 実装記録 (§6.4.3 第 2 段)

2026-11-11 着手、2026-11-12 に専用 worktree `G:\home\mimageviewer-767s2`、ブランチ
`feat-767-stage2` で再開。基点は設計追加の `cc0264f3a`。中断時の 20 ファイルの差分は
設計担当が `196dec7fc` として保存し、再開時に基点との差分を確認した。旧 worktree は変更していない。
設計担当 ClaudeCode Opus と独立 GPT-6.1 Sol / xhigh の設計レビュー r5 による構造的変更への合意を前提に、
§6.4.3 の 1〜6 を実装した。第 1 段の決定 B・C・E は維持する。

前提のコード確認:

- 3D の状態・要求・LRU・GPU slot は既存の context bundle / Depth owner に属する。
  `other_viewer_context_ids` と `with_viewer_context_ref` で兄弟の所有だけを読み、入場のために兄弟を mount / reset しない。
  presentation と独立性の許可集合をなくし、同時に 1 context の条件へ置き換えた。
- `commit_viewer_session` は実際の presentation / 独立性の切替だけで Depth を終える。
  F12 の表示先切替と窓内 F11 の違いは既存経路に由来し、キー処理は変更していない。
  第 1 段の独立フラグの書き込み一覧も再照合した。`prepare_detached_image_windows_for_open` の false 代入は、
  `should_park_active` が現ページを持つことを保証し、park 入口の既存 reset が snapshot の成否より前に Depth を退役する。
  旧「独立 session は Depth 不可」という根拠には依存しない。ほかの書き込みも既存確定点・close / fork / park の退役、
  または同じ context の bundle 一括投影であることを維持する。
- `park_active_detached_context_as_live_media` は対象を mount した closure 内で snapshot を作る。
  その直前に Depth だけを終了し、既存 fork と同じ所有境界で要求の取消・slot の除去を済ませる。
  360° の意図を扱う `reset_detached_pause_foreground_modes` は追加しない。
- dialog の所有者は書き出しの既存 `Operation.resource.context` と追加モデル案内の既存 `CheckingOrigin` から決める。
  `OwnedDialog` は入口と phase を 1 つの寿命で保持し、別の owner sentinel を作らない。
  live 判定と `depth_dialog_owner_surface` を 1 か所に置き、viewer / ROOT の 4 か所から同じ答えを読む。
- 独立所有者の modal の間だけ既存 document-open admission・OS close 受付・自動送り停止を広げる。
  所有窓以外の close、main 所有の案内、環境設定由来の案内、非 modal の進行画面は従来どおり。
- 受付済み遷移の判定は既存 `active_detached_transition_outstanding()` を読む。
  新しい pending / 完了待ちを作らず、modal 開始時に拒否することで通常の reopen を自分で拒まないようにした (r5 P1)。
  入口の導入済みモデルを使う通常 3D と、処理中の Running 案内はこの拒否の対象外。
- ROOT の model scan は維持し、main 以外の要求だけを要求元へ scoped mount して再開する。
  live・context・世代・ページ・item の既存照合で古い要求を捨て、main の配送規則は変えない。
- 一時データの panel / display snapshot は context を含む id で所有を分ける。
  viewport の生成・id・builder、HWND / placement / focus / z-order、bundle の swap、fork / park の搬送手順、
  backstop、root / child 振り分けは変更せず、App の detached 用状態・待ち・再試行・追加 repaint は足していない。

| ファイル | 変更 |
| --- | --- |
| `src/app/depth_view.rs`、`src/depth_parallax.rs` | 単一 context の入口条件、`OtherWindow`、実 session 切替の終了、要求元への再開、context 付き panel id |
| `src/app/depth_dialog.rs` (新規) | live / dialog 表示先・独立 modal 所有・遷移中の modal 受付を既存の事実から導く |
| `src/ui_dialogs/depth_addon.rs` | 入口と dialog phase の共通寿命、実 modal 開始の判定、ROOT からの要求元配送 |
| `src/app/depth_export.rs` | 全 phase の所有 context、遷移中の受付拒否、context 付き display snapshot id |
| `src/app.rs`、`src/app/viewport_retirement.rs`、`src/app/update_phases.rs` | 独立 live-media park の Depth 退役、独立 modal の open / close 受付、ROOT dialog 表示先 |
| `src/ui_fullscreen.rs`、`src/ui_fullscreen/viewer_chrome.rs` | viewer dialog 表示先・所有者の自動送り停止・context 付き panel 参照 |
| `src/addon.rs` | `cfg(test)` の既存 operation fixture を setter として再利用。worker / HTTP / I/O なしで非 modal を検査 |
| `src/app/depth_view_tests.rs`、`src/app/depth_main_window_tests.rs`、`src/app/depth_export_main_window_tests.rs` | 入口・上部ボタン・資源寿命・export 全 phase・一時データ所有の回帰 |
| `src/app/depth_setup_entry_tests.rs`、`src/app/multiwindow_scenario_tests.rs`、`src/ui_fullscreen/depth_focus_tests.rs` | ROOT 再開・独立実 pass・modal 排他・受付済み遷移・main 非退行、既存 fixture の context id 更新 |
| `htdocs/mimageviewer/manual/fullscreen.html`、`shortcuts.html`、`htdocs/mimageviewer/index.html`、`docs/keymap-spec.md` | 独立した窓・一度に 1 窓・modal と読み込み中の受付範囲。FAQ に該当記述なし |
| 本書、`docs/detached-rework-plan.md` §11、`docs/async-architecture.md` | 現行仕様、構造上の理由、OS close の所有者別の規則、検証記録 |

書き換えた第 1 段のテストと理由:

- 入口・継続・上部ボタンの matrix は、main の独立 DetachedWindow と独立 context の旧「拒否」を「許可」へ変更。
  独立性そのものを拒否する規則を廃止したため。代わりに main → 独立 / 独立 → main の両方向で `OtherWindow` と
  兄弟の状態・要求・slot の不変を検査する。同 session の再確定は継続、実独立性切替では終了する。
- panel / display snapshot の fixture と receipt は context を含む id へ合わせた。
  `RootDepthViewer` の receipt context は作成時の main 固定ではなく、各 frame の mount に追従させる。
  製品の描画・状態を変えてテストに合わせてはいない。
- main の要求を兄弟 mount 中に直接再開しない既存テストは保持する。新しい ROOT 配送は main 以外の要求にだけ適用する。

検証は Windows・通常 feature。全 Cargo に `MSBUILDDISABLENODEREUSE=1` を使用する。
新しい target の初回 core check と lib test graph のビルドは、それぞれ TurboJPEG の CMake 並列ビルドで exit 101 となり、
`CARGO_BUILD_JOBS=1` で再実行した。vendor・依存設定は変更していない。
直列の lib test ビルドも rustc の `memory allocation of 2097152 bytes failed` で exit 101 となった
(`target/depth-1767-phase2-focused-serial.log`)。この時点でテスト実行は 0 件であり、製品テストの失敗とは区別する。
空きメモリを確認し、以後のテストビルドは `CARGO_BUILD_JOBS=1` と `CARGO_INCREMENTAL=0` を使用する。
通常 feature・ソース・依存設定は同じで、ビルドのための環境変数だけを変えた。
§6.4.3 の受入条件と回帰の対応:

1. `depth_presentation_entry_continuation_and_commit_share_context_eligibility` と
   `depth_top_button_color_matches_the_context_and_session_entry_matrix` は main / 独立の全表示と上部ボタンの実色を検査する。
   `depth_other_window_entry_is_readonly_in_both_directions_and_scoped_mount_keeps_owner` は at rest の兄弟を両方向で照合し、
   拒否側と既存所有者の状態・要求を変えず、一時 mount では Depth を終えないことを検査する。
2. `depth_independent_live_media_park_retires_request_and_slot_and_releases_entry` と
   `depth_independent_os_close_retires_request_and_slot_and_releases_entry` は要求中 / Ready の取消・slot 除去・次の入場を検査する。
   独立化・fork / pause の既存所有テスト、F12 双方向・窓内 F11 の資源保持も回帰として残す。
3. `multiwindow_depth_independent_addon_dialog_is_drawn_only_in_owner_live_pass` と
   `multiwindow_depth_independent_export_dialog_is_drawn_only_in_owner_live_pass` は実 ROOT 更新と子 viewport を駆動し、
   main の埋め込みと独立 viewer が同時にある frame で所有側だけに描かれることを検査する。main の各表示・環境設定由来は第 1 段の実 pass テストを維持する。
4. `depth_setup_independent_checking_and_confirm_protect_owner_but_allow_sibling_close` と
   `depth_independent_export_all_phases_protect_owner_and_allow_sibling_close` は Checking / Confirm と export の
   Settings / Running / Stopping / Finished で実 close・activation・外部 open の受付と manual / timer / NextFolder 停止を検査する。
   `depth_setup_main_and_preferences_keep_external_open_activation_and_slideshow_admission` は追加制限の非退行を対照にする。
   `multiwindow_depth_accepted_{folder,zip,pdf}_transition_rejects_entry_and_export_then_reopens_same_context` は 4b の
   実 DFS / enumerate / hydration 完了と、同じ context / window の正常 reopen・modal 開始まで検査する。
5. `multiwindow_depth_independent_running_addon_returns_to_actual_root_after_os_close` は非 modal のまま owner close を受け付け、
   実 ROOT pass に操作可能な案内が戻ることを検査する。
6. `depth_setup_independent_installed_scan_*` の 3 条件は ROOT の poll で要求元だけを入場させ、ページ変更・非 active では破棄する。
7. `depth_first_pass_in_new_context_cannot_read_previous_panel_or_export_snapshot` は旧 snapshot の resource / 世代も新所有者に一致させ、
   id の context による分離自体で拒否されることを検査する。
8. depth・export・presentation・window_mode・panorama・detached・multiwindow_scenario・slideshow・admission を指定 filter で回帰実行する。

実装担当のセッション内のレビュー (GPT-6.1 Sol / xhigh) では、削除 API / variant の既存テスト追従漏れ 3 か所を P2 1 件、
非 modal の Running 案内にまで遷移拒否を広げていた点を P3 1 件として修正した。
後者は phase 選択と同じ `entry_check_opens_modal()` を入口でも読み、Installing / Cancelling / Removing の Running を拒否しない。
`depth_setup_running_operations_remain_modeless_during_accepted_independent_transition` がその対照を持つ。
再レビューは構造的変更として受入可、未解決 P1/P2/P3 なし。検証は実装担当が所有し、レビュー担当は重複実行しない。

初回の depth 実行は **222 件中 217 成功・5 失敗、exit 100**
(`target/depth-1767-phase2-focused-noincremental.log`)。失敗した追加 fixture を次のように修正した:

- live-media park の既存末尾は main を一覧へ戻す。閲覧していない main に直接 entry Ok を期待せず、
  main の `fullscreen_idx == None` を確認し、通常の `open_fullscreen` で別の閲覧を開いてから入場可能を検査する。
- 独立 dialog の 3 条件は main の linked F12 を残して別の active context を直接構築しており、
  通常は handoff / park で解消される組合せを捏造していた。設計対象の main 埋め込み + 独立 child に直し、
  各 ROOT pass で両 viewer の寿命と唯一の描画先を確認する。export の準備は既存 display fixture と実受付 handler に分け、
  独立 child は実 ROOT の `ScenarioDriver` から描く。共有 dialog の描画条件や製品の routing は変更していない。
- PDF の 4b は既存 PDF テスト用の path 限定 `RemotePdfTestBackend` を登録しておらず、test exe を製品の subprocess worker として
  起動する経路へ落ちていた。既存 fixture と同じ worker 内の実 PDFium 解析へ接続し、guard を App より長寿命にする。
  通常の非同期所有者が行う `poll_book_composition_read` も補い、projection / permission 待ちから reopen まで進める。
  15 秒の待機上限は変えず、失敗時に残る待機状態を表示する。旧 items の復元や製品 guard は加えていない。

修正差分も同じセッション内のレビューで検査条件の弱体化なしと確認した。修正後の実行結果は下表で確認する。
この修正後の 5 件再実行は **2 成功・3 失敗、exit 100** (`target/depth-1767-phase2-focused-r2.log`)。
park と PDF は成功し、残った dialog 3 件は新 Window の最初の不可視 sizing pass でタイトルを要求していた。
既存の成功 fixture と同じ 4 pass の準備後に検査するよう観測点を修正し、全 7 pass の両 viewer の寿命確認と、
後続 3 pass の唯一 surface の検査を維持した。egui の通常の初回計測に合わせる修理であり、
製品への待ち・repaint・frame フラグや vendor の変更はない。同じセッション内のレビューでも owner / mount / live の矛盾なしと確認した。
続く 3 件再実行は **2 成功・1 失敗、exit 100** (`target/depth-1767-phase2-focused-r3.log`)。
唯一 surface の検査は 3 条件とも通ったが、export 条件の末尾で addon 用 helper を誤って検査していた。
addon が無いときの `Main` fallback は正しいため、存在する dialog に合わせて export / addon の helper を選ぶよう修正した。
この修正も同じセッション内のレビューで妥当と確認し、個別の重複実行はせず、以下の指定回帰 filter 内で全 depth 条件を再確認する。

| コマンド | 結果・証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)'` | **1,523 成功 / 0 失敗、exit 0** (15,801 skipped、106.632 秒)。`target/depth-1767-phase2-regression.log`。全 depth 222 件と修正した独立 dialog 3 件を含む |
| `cargo test --test ui_snapshot` (更新指定なし) | **250 成功 / 0 失敗、exit 0**。`target/depth-1767-phase2-snapshot.log`。`UPDATE_SNAPSHOTS` 未設定、PNG 差分なし |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0。`target/depth-1767-phase2-check-serial.log`。以後の Rust 変更はテスト fixture と整形だけで、製品の意味変更なし |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ 0 件。`target/depth-1767-phase2-glyphs.log` |
| `python scripts/check_site_cards.py` | exit 0、53 カード・上限超過 0 件。`target/depth-1767-phase2-site.log` |
| `cargo fmt` (引数なし、最終) | exit 0。`target/depth-1767-phase2-fmt.log`。上記回帰以後は整形と実装記録だけで、Rust の意味変更なし |
| `git diff --check` | exit 0。`target/depth-1767-phase2-diff-check.log`。基点からの vendor / PNG の diff も exit 0、差分なし |

最後の意味変更はテストの export / addon helper 選択だけで、その後に上記の指定回帰を実行した。
製品コードに追加の guard・待ち・repaint は加えず、失敗した fixture の観測条件を修理した。
設計との差分はない。上の「同じセッション内のレビュー」は実装担当のセッションの中で行われたもので、正式な独立レビューには数えない。
**正式な独立実装レビュー (GPT-6.1 Sol / xhigh、read-only、実装者とは別のセッション、対象 `08cc9da88`、差分 `cc0264f3a..08cc9da88`)**:
P1 / P2 / P3 なし、判定「受入可」。単一所有と寿命、dialog 表示先の集約、独立所有者に限った modal 保護、r5 P1 の遷移中拒否と正常 reopen、
ROOT からの再開配送の照合、context 付き一時 id、テストの強さ (旧実装で通るだけの検査なし、fixture 修理で条件の弱体化なし)、文書の整合を確認。
「症状パッチではなく構造的な変更」であることに**合意** (既存の所有者と遷移状態から受付・表示先・退役を導き、追加の detached 状態・時間窓・待ち・
再試行・追加 repaint を作らない)。レビュー担当は上記 1,523 件と snapshot 250 件の成功ログを参照し、Cargo を重複実行していない。

設計担当による最終確認 (2026-11-12、対象 `47a93b304` = 製品コードは `08cc9da88` と同一、いずれも共有の `run-heavy.ps1` 経由):

| コマンド | 結果 |
| --- | --- |
| `.\scripts\test-lib-full.ps1` (`-FullTest`、共有ロック test-lib-full 下) | **17,249 成功 / 0 失敗 / 75 skipped、795.6 秒、exit 0** |
| `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-run` | exit 0 (launcher はこの worktree に release exe 5 本が無く build.rs で止まるため除外。コード起因ではない) |
| `.\scripts\build-dev.ps1` | exit 0、`target\dev-runtime\mimageviewer-core.exe` |
| `cargo test --test ui_snapshot` | 実装担当の 250 成功を再利用 (その後の変更はテスト fixture・整形・文書だけ) |

### 17.16 第 2 段の実機確認と、次の版へ回した課題 (2026-11-12)

サブ PC 確認 依頼 39 (ビルド `865eeac20`、観測はすべてサブ PC 担当、結果は `C:\miv-subpc\results-39\`) の後、利用者の決定 (2026-11-12 夜) で第 2 段は次の版へ回した。第 1 段 (`1ff4082a8`) は今回の版に入る。ブランチ `feat-767-stage2` はそのまま残す。

期待どおりだったもの: モデル未導入の案内はメディアウィンドウにだけ出て、案内中は × で閉じない。メディアウィンドウ・複数ウィンドウモードの窓で 3D を開始でき、パネル・折りたたみ・Ctrl+クリック・F11 往復が動く。書き出し画面は所有の窓にだけ出て、書き出し中・結果画面の間は × で閉じない (GIF 45 フレーム)。F12 の別ウィンドウ (第 1 段) に退行なし。ログに ERROR / WARN / panic なし。

次の版で直す課題 (設計担当がログとコードで調べた範囲。実機での再確認は未実施):

1. **メディアウィンドウで画像を送ると窓が閉じ、3D が終わる。** 一覧から動画を開いた直後のメディアウィンドウはメインの閲覧の一部で、画像へ進むと閉じて画像はメインに開く。メインで別のものを開いて独立した窓になった後でも、動画から画像へ進んだ時点で `prepare_viewer_presentation_open` が独立性を `detached_still_open_is_independent` (「画像を別ウィンドウで開く」設定か一度きりの指定) で決め直すため「連動」になり、次の送りは F12 の別ウィンドウ表示がオフなら `open_non_detached` でメインへ行く。行き先の関数 (`requested_viewer_presentation_for_open`・`prepare_viewer_presentation_open`・`activate_parked_live_media_window_snapshot`・`park_and_close_current_active_detached_viewer_for_media_handoff`・`activate_mounted_as_independent_detached`) は v5.1.0 と第 2 段の基点で同一で、第 2 段は表示先が変わったときの 3D の終了だけを足している。既定の設定ではメディアウィンドウの 3D は最初の 1 枚に限られる。対象に残すなら、独立した窓で画像へ進んでも独立のまま保つ行き先の変更 (detached の規則の変更、構造的修正の合意が要る) を設計する。対象から外す案も比べる。
2. **3D 中のメディアウィンドウの × で窓が作り直される。** ログ (`logs-part2-resumed`) では 491.359 秒に `close_active_viewer_context` で閲覧が閉じ、491.372 秒に見えないメインの全画面が `main focus guard closing session` で閉じた直後、同じ窓番号 3 が `deferred_passive_seed` で待機中の窓として作り直された。2 回目の × で閉じた。窓番号が有効な閲覧と待機中の窓の記録の両方に残っていた疑いがあるが未特定。3D なしでも起きるか、メインの全画面が開いていない場合に起きるかは未確認。detached-rework-plan §2 の BA 分類と構造的修正の合意の手順で直す。
3. **見えないメインの全画面が 3D の持ち主として残る。** メインが全画面のとき、独立した窓の有効中はメインを描かない (既存)。その間もメインの閲覧は開いたまま (ハートビートの `fullscreen=Some(2)`) で 3D を持ち続け、メディアウィンドウの D は「別のウィンドウで表示中」で断られた。持ち主の判定は事実としては正しいが、利用者には見えない窓のせいで断られる。設計レビュー r5 で「生きている判定は描画を保証しない」と割り切った点が表に出たもの。見えない閲覧を持ち主として扱わない (その時点で 3D を終える) か、文言で持ち主の窓を示すかを決める。
4. **手順書と文言の想定違い。** 別の窓を開く・有効にすると前の窓は既存の切り替え (`pause_active_context`) で止まり、その窓の 3D も終わる (§6.4.3 の park / pause での終了)。そのため 2 つ目の窓の D は断られず、断る場面は前の窓が止まらずに 3D を持ち続けるとき (課題 3) だけ。外から開く要求は、書き出し・モデル確認の画面の間は保留され、閉じた後に最後の 1 件が開く (`src/app/startup_ops.rs` の既存の保留)。マニュアルの「外部からファイルを開く操作…が止められる場合があります」を「画面を閉じた後に開きます」に直し、手順書の期待もこれに合わせる。
5. **記録: 一覧にマウスが届かなかった件 (原因不明)。** 依頼 39 の 1 回目、「動画・音声は別ウィンドウで再生」ON で動画をメディアウィンドウで再生 → ↑ (窓が閉じ画像がメインの全画面) → Esc で一覧へ、の約 2 秒後から一覧へのクリック・ホバーが届かず、キーボードは届いた。ログ (`logs-part1-first-try`) では 144〜174 秒にクリックの時刻にだけ描画が起き、マウス移動では起きていない。押した位置は閉じたメディアウィンドウの範囲の中。build-759 と、新しい版の第 2 部の同じ経路では再現しなかった。再現したら `C:\miv-subpc\whichwin.ps1` (カーソル下の窓・mIV の全トップレベル窓・capture を出す) で記録する。

未確認: 実 OS 窓での close / activation / focus、native の保存先画面、実モニタ間の DPI 移動、実モデルでの推論、
backstop / interlude の数 frame の見え方。headless の成功とは区別する。
製品起動・`build-dev.ps1`・`test-lib-full.ps1`・`test-full.ps1`・コミット・push は依頼どおり実施しない。
確認用 build・全件 gate・実機確認は設計担当へ引き継ぐ。vendor は変更していない。

### 17.17 実装記録 (§6.4.4 実機課題 2・3・4)

2026-11-14、`G:\home\mimageviewer-767s2` / `feat-767-stage2`、基点 `bbad4507d`。
§6.4.4 の設計レビュー r1 後の範囲に従い、課題 2・3（P2-2 を含む）・4 を実装した。
課題 1 は利用者判断待ちのままとし、行き先・独立性の解決規則は変更しない。

着手前の主要前提の確認:

- `active_viewer_context_id` は active session が無いとき、window binding と
  `detached_viewer_window_id().or(last_active_detached_window_id)` を使い、同 id の passive snapshot があれば active としない。
  旧 passive activation は先に snapshot を remove してから park を呼ぶため、実際に pause した直前の A が自分自身の移動元になり、
  pause が同 id の snapshot を再度 push する。active close はその孤児を消さず、後の passive renderer が同 id を再登録する。
  ParkedLive は既に park → id で取り直し → remove の順であり、linked still 保持を true で呼ぶことを確認した。
- `should_render_main_fullscreen_viewport_after_detached_context` は独立 context の更新中、main の embedded active / pending だけを描く。
  main の通常 Fullscreen の閲覧状態は残る。Depth の Drop は要求取消と自分の GPU slot 除去を持ち、
  `set_fs_exclusive_view(None)` が既存の終了経路であることを確認した。
- `DepthSetup::OwnedDialog` は Checking / Confirm の間も `ModeEntry(DepthEntryRequest)` を保持し、ROOT の poll で確認後に入場を再開する。
  main は `viewer_context_live` が常に true のため、表示所有の移動時にその入場要求も失効させる必要がある。
  ModeEntry の dialog だけを閉じれば確認処理は継続し、Preferences と Running 以降には影響しない。
- 以上は設計の主要前提と一致した。毎フレームの描画検査、D からの兄弟の変更、孤児描画 guard は追加していない。

変更ファイル:

| ファイル | 変更 |
| --- | --- |
| `src/app.rs` | 明示した移動元を使う park / pause / live-media / close の共用、passive / ParkedLive の共通 snapshot 消費、成功した独立 activation / build / fork-open の確定点への接続 |
| `src/app/depth_view.rs` | main context 内で、表示を失う Fullscreen の Depth と未完了の入場だけを終える helper |
| `src/ui_dialogs/depth_addon.rs` | 指定 context の Checking / Confirm の ModeEntry 失効。確認処理と管理由来の案内を維持 |
| `src/app/depth_view_tests.rs`、`src/app/depth_setup_entry_tests.rs`、`src/app/multiwindow_scenario_tests.rs` | 実 activation、要求取消・GPU slot、Checking / Confirm / Preferences、pause→再開→実 close→ROOT の回帰 |
| `htdocs/mimageviewer/manual/fullscreen.html` | 切替時の 3D 終了と、外から開く要求の最後の 1 件を画面を閉じた後に開く説明 |
| 本書、`docs/detached-rework-plan.md` §11、`docs/async-architecture.md` | 変更境界、BA 分類、入場の寿命、検証記録 |

課題 3 の入口監査:

| 入口 | 表示所有の移動の確定点 |
| --- | --- |
| `activate_detached_image_window_snapshot` の既存 context 再開 | mount で独立化が成功し、active session を設定した後 |
| `activate_parked_live_media_window_snapshot` | 固有の pending owner 引継ぎ・runtime / session の再開を確定した後 |
| `start_active_detached_loading_context` | Building を Commit し binding と active session を公開した後。Abort では終了しない |
| `start_active_detached_collection_root_image` | anchor を解決して build を Commit し session を公開した後、画像を open する前 |
| `start_active_detached_book_context_with_start` | descriptor / scanned-folder の受理で build を Commit し session を公開した後 |
| materialized fork 後の open | fork context の open と binding が成功した後。この入口は main の fullscreen_idx が無い場合に限り、通常は終了対象を持たない |
| stamp による旧 still snapshot の再開 | 既存の main / linked の open。別 context を有効にしないので新しい終了境界を足さない |
| descriptor の継続、通常の更新・scoped mount | 既存 owner の処理。helper を呼ばない |

全画面の閲覧自体を閉じる簡素化は既存の閲覧保持を変えるため採用しない（§6.4.4）。
既存の owner と成功確定点だけを共用し、新しい App 状態、待ち、回復の仕組みは追加しない。

検証環境: Windows、通常 feature、test / dev profile。Cargo のビルド・テストはすべて
`powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "codex/767-p2b" -WorkDir G:\home\mimageviewer-767s2 -Command "<下記コマンド>"`
を使った。ロック領域へ書けない警告が出たため、そのまま BelowNormal で継続した。
Command 内で `CARGO_BUILD_JOBS=1`、`CARGO_INCREMENTAL=0`、`MSBUILDDISABLENODEREUSE=1` を設定した
（この worktree の第 2 段の既存メモリ不足時と同じ条件）。重いコマンドは同時に 1 本だけ実行した。

| コマンド / 対象 | 件数・exit code・証拠 |
| --- | --- |
| 旧製品コード `bbad4507d` + 新テスト: `cargo nextest run -p mimageviewer --lib -E 'test(/paused_last_active_resume\|main_live_pass_handoff/)' --no-fail-fast` | **4 実行 / 0 成功 / 4 失敗、exit 100**。`target/767-p2b-red.log`。A の passive が 1 件残る、main の Depth が残る（通常再開と loading build）、Checking が残る、の各不変条件で失敗。コンパイル成功、製品の不変条件違反を検出した |
| 修正後の焦点: `cargo nextest run -p mimageviewer --lib -E 'test(/paused_last_active_resume\|main_live_pass_handoff\|snapshot_activation_handoff/)' --no-fail-fast` | **5 実行 / 4 成功 / 1 失敗、exit 100**。`target/767-p2b-focused-r1.log`。失敗と補修の理由は下記 |
| 最終回帰: `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)' --no-fail-fast` | **1,537 成功 / 0 失敗、exit 0**（16,161 skipped、117.004 秒）。`target/767-p2b-regression.log`。新規 5 件と既存の第 1 段・第 2 段・detached・multiwindow の指定回帰を含む |
| `python scripts/check_ui_glyphs.py` | 危険グリフ **0 件、exit 0**。`target/767-p2b-glyphs.log` |
| `python scripts/check_site_cards.py` | **53 カード、超過 0 件、exit 0**。`target/767-p2b-site.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0**（テスト実行なし、4 分 22 秒）。`target/767-p2b-check.log` |
| `cargo fmt`（最後に引数なし、同じ run-heavy 経由） | **exit 0**。`target/767-p2b-fmt.log`。最終回帰・core check 後の Rust 変更は整形だけ |
| `git diff --check` | **exit 0**。`target/767-p2b-diff-check.log`。変更は上記 10 ファイル、vendor 差分なし |

旧コード比較の間だけ製品 3 ファイルを HEAD と同一にし、終了時に実装を戻した。
旧コード比較後には、資源検査の renderer を main / child で同じ専用 renderer に固定し、
A→B→A と再開拒否時の保持の対照 1 件も加えた。旧コードの 4 件の失敗点と根拠は変わらない。

焦点検証の multiwindow 1 件は、修正で再開直後の snapshot 0 件の assertion を通過し、その先の ROOT pass で
`fullscreen navigator input tracking must run at begin-pass` により停止した。
新しい fixture で `install_fs_navigator_input_tracking` の登録を省いていたため、既存 scenario と同じ登録を追加した。
assertion の削除・緩和や製品コードの変更はせず、上記の最終回帰で新規 5 件を含めてすべて成功した。
最終回帰の製品コードは焦点検証と同じで、追加変更は fixture 初期化 1 行だけである。

新規回帰と根拠:

- `multiwindow_depth_paused_last_active_resume_close_does_not_register_passive_again`:
  実際の pause と `last_active_detached_window_id=A` を使い、再開直後は A の passive が 0 件。
  実 close handler を 1 回通し、context の Retired、binding / runtime / passive 要求の消滅、
  後続 3 回の実 ROOT pass で callback / viewport / deferred view の再登録が無いことを検査した。
  main 一覧 / 全画面 × Depth なし / ありの 4 条件を完走した。
- `depth_main_live_pass_handoff_retires_only_fullscreen_on_actual_still_and_media_activation`:
  passive / ParkedLive × 推論待ち / Ready × main 全画面 / 埋め込みの 8 条件で、main 全画面だけの終了、
  cancel token / GPU slot の退役、子の実 D の成功を検査した。埋め込み main は保持し、子の D は OtherWindow。
  事前の scoped mount は main の資源を維持する。
- `depth_main_live_pass_handoff_invalidates_checking_and_confirm_entry_but_keeps_preferences`:
  初回 D の実 Checking とローカル pack の実確認完了を使い、表示を失った main は後から入場しない。
  main 由来の Confirm も失効する。確認処理は完了してモデルの状態を採用し、Preferences の案内は残る。
  埋め込み main の Checking / Confirm は保持し、Checking の完了で main だけに入場する。
- `depth_main_live_pass_handoff_new_loading_owner_commits_but_aborted_build_keeps_depth`:
  build Commit で終了し、Abort では main の Depth と要求を保持する。
- `detached_snapshot_activation_handoff_switches_a_b_and_keeps_rejected_destination`:
  A→B→A で active と passive の二重所有を作らず、再開を断った対象と現在の active owner を保持する。

見た目・レイアウト・アプリ内 UI 文字列は変更していないため、`cargo test --test ui_snapshot` は実施していない。
unit 側の既存 depth snapshot は上記の指定回帰に含まれる。

未確認 / 引継ぎ:

- 実 OS 窓の activation / close / HWND / focus / z-order、backstop の見え方、実モデルでの推論、実モニタ間 DPI は未確認。
  headless の実 handler / ROOT pass と、利用者の実機結果は区別する。課題 5 の一覧へのマウス入力は原因不明のまま。
- `test-lib-full.ps1`・`test-full.ps1`・`build-dev.ps1`、製品起動、コミット・push は依頼どおり実施しない。
  全件 gate・確認用 build・実機確認と独立実装レビューは設計担当へ引き継ぐ。
  vendor は変更していない。独立レビュー後の構造合意は detached-rework-plan §11 に設計担当が追記する。

独立実装レビュー (2026-11-14、GPT-6.1 Sol / xhigh、read-only、対象 `53d224a5c`): **受入可、P1 / P2 / P3 なし**。
再開の順序 (移動元の確定 → park → 対象を id で取り直して消費)、Depth と入場要求を終える 6 か所の確定点、マニュアルの文言、テストの条件、
範囲外の変更が無いことを確認した。**症状パッチではなく構造的な変更であることに合意** (snapshot の二重所有を作る順序を所有移動の入口で正し、
表示を失う確定点で Depth と入場要求の寿命を閉じる)。設計担当 (ClaudeCode) も同じ判断で検収した。

### 17.18 実装記録 (§6.4.5 課題 1)

2026-11-14、`G:\home\mimageviewer-767s2` / `feat-767-stage2`、基点 `01f5e898f`。
§6.4.5 の r1 補正を優先して実装した。課題 2・3 の所有移動 helper と main Depth 退役の確定点は変更しない。

変更ファイル:

- `src/app.rs`: 新規 binding の初期解決と既存 session の継承、descriptor build 初期化、
  採用後・旧項目更新前の窓交換 helper、F12 の明示解決（遅延 reopen 中を含む）、
  遅着 Manual の窓交換を active に限定する共通採用判定。
- `src/app/collection_navigation.rs`: 同じ採用判定を exact target 確認後・root の install 前に適用。
- `src/app/depth_view.rs`: presentation と独立性の同時 commit。
- `src/app/viewer_context_registry.rs`: session 退役後の同じ mounted context の A→B binding 交換。
- `src/app/native_video.rs`: 旧 media 項目だけの退役、native F12 と通常の同 host 同期の区別。
- `src/app/presentation_transition.rs`: 既存 request に解決済み独立性を保持し、旧 media output の
  drop とともに旧 request/effects を終端する（次の request id は巻き戻さない）。
- `src/app/detached_media_exchange_tests.rs`、`src/app/multiwindow_scenario_tests.rs`、
  `src/ui_fullscreen.rs`、`src/app/tests.rs`: handler・所有移動・初回 builder の回帰。
- 本書、`docs/detached-rework-plan.md` §11、`docs/video-architecture.md`、`docs/keymap-spec.md`、
  `htdocs/mimageviewer/manual/fullscreen.html`: 仕様・所有境界と公開説明。

着手前の前提確認（実装者と独立 GPT-6.1 Sol / xhigh のコード照合）:

| 前提 | コードで確認した結果 |
| --- | --- |
| 独立性の二重決定 | prepare が静止画設定から再計算し、media で false を書き戻していた |
| binding の予約・公開 | Building は私有予約、Commit 後に公開して session を開始する。継続は同じ binding を読む |
| descriptor 再構築 | resume_window_id を保持しても新しい context / default session になる。予約直後に解決済み pair を初期化した |
| 複数ウィンドウの新規 media | 旧コードは最初 false。新規 open の binding 初期解決で独立とし、その後は引き継ぐ |
| F11 | transition が target を保持。return Settle では restore は消費済みで runtime に通常配置が戻っている |
| session commit | target_independent を Depth 終了判定にだけ使用していた。同じ確定点で独立性も書く |

r1 補正を優先すれば実装を妨げる矛盾はなかった。追加の入口監査で DetachedImage の scan 完了が
独立性と one-shot を再設定する箇所も確認し、binding 初期確定に一本化した。
F12 の native 同 host request は rect 同期 / VST 退出にも使われるため、通常要求は継承し、F12 の明示要求だけ
解決し直す。現在の表示先へ戻す反転は reducer が Apply を省略できるので、採用後に独立性を同期確定する。

交換は admission / 候補採用後だけ実行する。旧 C の media 資源を項目離脱の部品で終端し、
C 全体の close、cache clear、park、再走査は行わない。既存の窓を閉じて開く簡素化を窓の役割交換に採用し、
閲覧全体の close→open は slideshow / pending / cache を失うため使わない。
App の新しい状態、待ち、リトライ、救済用 repaint、focus / z-order の仕組みは加えていない。

検証はこの worktree だけで、Cargo のビルド・テストをすべて次で直列実行する:

`powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "codex/767-issue1" -WorkDir G:\home\mimageviewer-767s2 -Command "<コマンド>"`

ロック領域へ書けない警告は指定どおり継続。nextest は既存のこの worktree の検証条件と揃え、
`CARGO_BUILD_JOBS=1` / `CARGO_INCREMENTAL=0` / `MSBUILDDISABLENODEREUSE=1` を使う。

追加決定前の検証履歴（当時の未解決 P1 による非ゼロを含む。追加決定後の最終結果は後掲）:

| コマンド | 件数 / exit code / 証拠 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core`（初回） | exit 0、4分09秒。最終検証前の中間状態 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/independent_media_exchange/)' --no-fail-fast`（r1） | **10成功 / 0失敗、exit 0**。compile 8分54秒、実行2.858秒。`target/767-issue1-focused-r1.log`。遅着Manualの追加回帰前 |
| 同じ焦点コマンド（r2、遅着 Manual / native request 回帰と F12 OFF 完了を追加） | **12実行 / 11成功 / 1失敗、exit 100**。compile 8分57秒、実行2.941秒。`target/767-issue1-focused-r2.log`。失敗は下記の設計境界だけ |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)' --no-fail-fast`（回帰 r1） | **1,548実行 / 1,546成功 / 2失敗、exit 100**。109.438秒。`target/767-issue1-regression.log`。設計境界の P1 と下記 fixture |
| 同じ回帰コマンド（r2、fixture 初期化だけ補正） | **1,548実行 / 1,547成功 / 1失敗、exit 100**。compile 8分38秒、実行121.461秒。`target/767-issue1-regression-r2.log`。未解決の遅着 Manual P1 だけが失敗。指定の既存対照・課題2/3回帰を含む |
| `cargo check -p mimageviewer --bin mimageviewer-core`（最終） | **exit 0**、1分21秒（テスト実行なし）。`target/767-issue1-check.log` |
| `cargo fmt`（最後に引数なし、run-heavy 経由） | **exit 0**。`target/767-issue1-fmt.log`。最終回帰後の Rust 変更は整形のみ |
| `git diff --check` | **exit 0**。`target/767-issue1-diff-check.log`。LF→CRLF の既存設定による警告のみ。HEAD は `01f5e898f` のまま、vendor 差分なし |
| `python scripts/check_ui_glyphs.py` | 危険グリフ 0、exit 0 |
| `python scripts/check_site_cards.py` | 53カード、超過0、exit 0 |

初回 nextest 呼出は PowerShell の filter 引数が未引用で解釈され、Cargo 起動前に exit 1。
ASCII の target 内実行スクリプトに filter を引用して直した（テスト失敗件数には含めない）。

回帰 r1 のもう1件は `detached_book_active_context_mount_restores_main_after_fullscreen_open`。
生の `build_active_context_for_test` が default session のまま binding を作り、列挙完了後に設定から
独立性を決める旧 prepare の副作用を前提にしていた。fixture 内の items 準備直後に
`new_detached_open_is_independent` で新規規則を解決し、session を初期 commit するよう修正した。
main の復元・ページ open・独立性の assertion はすべて維持し、共通 fixture や製品側の例外は追加しない。
独立レビューもこの fixture 補正を妥当と確認した。製品の新規 descriptor 初期化は別の実 activation 回帰で検査している。

未確認 / 引継ぎ:

- 実 OS 窓の表示間隔、focus / z-order、実 player の停止音・VST GUI、F11 の実画面・最大化復元、
  マルチモニター / DPI、実モデルでの3Dは未確認。headless handler / ROOT / builder の検証と区別する。
  「active」は owner と active session の window 一致を指す。結果待ち中に他アプリへ前景を移した場合の
  B の表示・focus は実機未確認で、OS 前景判定や最初の builder の既定動作は今回変更していない。
- 旧製品コードへ差し戻して新規回帰を実行する比較は未実施。描画部品・レイアウト・UI文字列は変更していない。
  `cargo test --test ui_snapshot` は未実施で、窓の初回条件は headless builder と ROOT scenario で検査した。
- 依頼どおり `test-lib-full.ps1`・`test-full.ps1`・`build-dev.ps1`、製品起動、commit / push は実施しない。
  vendor は変更しない。全件 gate、検証用 build、実機確認は設計担当へ引き継ぐ。

#### 追加監査と設計担当の追加決定: 遅延 Manual の採用境界

独立レビューで、独立 A の Manual 要求 → 別窓 D の実 activation → ParkedLive A への結果到着、という経路を確認した。
park は input_seq を進めず、通常要求は `rebind_native_video_pending_owners` で `None` → `Some(A)` へ移る。
従来の交換 helper は global active session を A とみなして D を閉じ、binding の所有 assert が失敗した。
焦点 r2 と回帰 r2 の遅着テストで、D の `ViewerContextId(2) / AtRest` と C の `ViewerContextId(1) / Mounted` の
不一致を再現した。これは timeout / harness の失敗ではない。追加決定前の独立レビューは P1 1件で未受入だった。

設計担当が §6.4.5 末尾の追加決定を行った。交換の必要な遅着 Manual は inactive なら採用せずログ付きで破棄し、
A の再生と ParkedLive、D の session / binding / window を保持する。同種メディアは背景で採用を続ける。
利用者への照会は不要となり、保留状態や背景 B の寿命は作らない。

追加の入口監査では、通常 resolver と別に Collection の request owner があり、その passive poll でも
Manual が採用されることを確認した。共通 `admit_manual_media_navigation_target` へ判定を1か所に集約し、
通常 resolver は result の currentness 照合後、Collection は exact target の照合後・root install 前に通す。
Collection の対象は最新 prepared entry の `GridItem` を使い、旧一覧の index から分類しない。
これは2つの request owner に同じ規則を適用する所有境界の修正であり、描画側の症状 guard ではない。

追加回帰:

- 遅着静止画: 通常の一覧から実 open / park / activation で独立 A を作り、実 Manual 要求後に D を実 activation。
  A で結果を採用せず、元の media identity・再生意図・items / generation・binding・ParkedLive を保持する。
  D の session・binding・runtime、共有 F11 / focus / viewport 状態、main の一覧と選択も不変。
  Video / Audio と Image / ZipImage / PdfPage の組合せを検査する。
- 遅着同種メディア: Video / Audio とも、A は同じ窓で次の media に進み、D と main は維持する。
- native request: A の実同 host request を AwaitingNative まで進めてから Manual → D activation。
  静止画結果を同じ採用規則で捨て、A の request / player と D を保持する。
- Collection: 実 actor・最新 prepared root・実 Manual → D activation → passive poll を通し、
  Video / Audio の静止画結果は捨て、同種メディアは背景で採用する。

追加決定後の焦点 r3 は **15実行 / 13成功 / 2失敗、exit 100**（compile 9分11秒、実行11.308秒、
`target/767-issue1-focused-r3.log`）。通常の遅着静止画と AwaitingNative 中の破棄は通過した。
同種メディア対照で、旧 Manual の Fullscreen landing と native swap 不成立時の通常 open が
`prepare_viewer_presentation_open` の active session publish へ入る旧経路を確認した。
Collection の検査は native continuation の完了待ちで失敗した。コード上でも同じ通常 open を通り、
A が Resuming に変わると ParkedLive poll の対象から外れる経路を確認した。
これはテスト runner の timeout ではなく、回帰内の完了条件の未達である。

同種メディアの背景採用を保つため、旧 Audio EOF の bundle 内採用部品を Video / Audio の共通部品にした。
通常 resolver / Collection の Manual landing は共通入口を通し、native の成功した output swap は維持する。
swap 不成立時と egui landing の背景メディアは、通常の全体 open に入らず、所有 C の idx / cache / marker と
既存の media load だけを更新する。Video↔Audio も同じ窓で扱う。
新旧 player の退役・resume 保存・normalize は既存の load に委ね、旧 output を drop したときだけ
同じ C の presentation request/effects を終端する。成功 swap の output は終端しない。
Manual の history / NewSource と EOF の ContinuousAdvance を保持し、cached / Audio へ動画の one-shot を残さない。
Audio→Video の解析資源は `music_analysis_source.owner_context_id` が C に一致するものだけ退役する。
起動時の音響処理待ちは既存 `vst3_deferred_media_open` へ送り、既存の所有 context ごとの完了処理から再開する。
native の背景 swap の履歴も、候補の受付時ではなく確定点で記録する。

さらに、破棄後に残した旧 same-host request の NativeReady / NativeCommitted は背景でも正当な完了である。
その ApplyPresentation が A を active に戻す旧経路も確認し、ParkedLive の同 host Detached→Detached の確定を
C の pair / native flag / sync stamp / A の runtime linked flag の更新に閉じた。
active session / front 同期 / DSP owner の公開は前景側に属するため、この背景確定では行わない。
別 host への明示切替の背景化・取消の仕様は追加していない。HWND 同定や focus / z-order の基盤は変更しない。

回帰は Video↔Audio の両方向（通常 / Collection）、旧 AwaitingNative がある背景 media 採用での
output 退役と遅着無効、既存の起動待ち gate と owner ごとの再開も追加した。
同種メディアの対照でも D の共有 F11 / focus / viewport と one-shot 不変を検査する。
静止画を捨てた native ケースは、その後の NativeReady / NativeCommitted と実 ApplyPresentation effect を通し、
A の ParkedLive と D の session / 共有状態をもう一度検査する。
native の host readiness / decoder と native 完了の事実だけに既存の headless seam を用い、
request owner / current item / pending 完了 / continuation / App 側の確定 handler は実経路を使う。

焦点 r4 は **18実行 / 16成功 / 2失敗、exit 100**（compile 9分34秒、実行4.156秒、
`target/767-issue1-focused-r4.log`）。通常の背景採用、破棄後の同 host native 完了、旧 output の退役、
起動待ち gate は通過した。2失敗は accepted Collection media の世代を旧値に固定した新規テストの期待だった。
この fixture では既存の root replacement を1回通るため、
`apply_collection_grid_prepared_install` → `install_new_items_inner` が世代を1回進める。
accepted Collection media だけ `old_generation.wrapping_add(1)` とし、rows / 順序・C / A・D の所有不変と
旧 player の退役を引き続き検査する。破棄する静止画は root install 前なので旧世代の完全不変を維持する。
独立レビューでこの期待の補正を確認し、製品側の root reuse / replacement は変更しない。
Collection 一般を常に世代更新する仕様にはしていない（既存の同一 root 再利用は世代不変）。

追加決定後の最終検証（Cargo はすべて上記 runner 経由、重い処理は直列）:

| コマンド | 件数 / exit code / 証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/independent_media_exchange/)' --no-fail-fast`（r5） | **18成功 / 0失敗、exit 0**。compile 17分23秒、実行11.213秒。`target/767-issue1-focused-r5.log`。r4 以後の変更は上記の新規 Collection テストの世代期待だけ |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)' --no-fail-fast`（回帰 r3） | **1,554成功 / 0失敗、exit 0**。build確認1.16秒、実行117.535秒。`target/767-issue1-regression-r3.log`。指定の既存対照・課題2/3回帰を含む |
| `cargo nextest run -p mimageviewer --lib -E 'test(/healthy_.*swap.*same_output_identity\|parked_audio_continuous_target_stays_inside_bundle\|effetune_and_vst_startup_gate\|parked_video_deferred_swap_completion\|parked_source_swap_pending_/)' --no-fail-fast` | **7成功 / 0失敗、exit 0**。build確認1.20秒、実行0.952秒。`target/767-issue1-background-controls.log`。既存の音声EOF、起動gate、成功したnative output再利用、背景swapとowner照合 |
| `cargo check -p mimageviewer --bin mimageviewer-core`（追加決定後） | **exit 0**、1分19秒（テスト実行なし）。`target/767-issue1-check-r2.log` |
| `cargo fmt`（最後の Cargo 操作、引数なし） | **exit 0**。`target/767-issue1-fmt-r2.log`。以後の Rust 変更なし |
| `git diff --check` | **exit 0**。`target/767-issue1-diff-check-r2.log`。既存 LF→CRLF 設定の警告のみ |
| `python scripts/check_site_cards.py` | **53カード / 超過0、exit 0** |
| `python scripts/check_ui_glyphs.py` | **危険グリフ0、exit 0** |

r4 は共有 runner が別セッションの全件テストのロック解放を待ってから開始した。
待機中はこちらの Cargo は起動せず、ロックを変更しない。再ビルド中も中断せず完了まで待った。
こちらから全件テストは起動していない。

追加決定に沿った実装と指定の自動検証を完了した。独立 GPT-6.1 Sol / xhigh の再監査では
追加 P1 / P2 はなく、同じソースの焦点18件・指定回帰1,554件・背景対照7件の成功ログを照合した。
core check / fmt と整形後の差分を含む最終証跡も独立レビューが照合し、今回の指定スコープを受入可と判定した。
未解決 P1 / P2 はない。実機確認・全件 gate・検証用 build は上記の引継ぎ事項として残す。
HEAD は `01f5e898f`、ブランチは `feat-767-stage2` のまま。変更は上記15ファイルだけで vendor 差分なし。
製品起動・検証用 build・全件 gate・commit / push は依頼どおり実施しない。
描画 / レイアウトは変更しておらず ui_snapshot は実施しない。実 OS / 再生音 / focus・F11・DPI / 実3Dなど、
上記の未確認点は引き続き実機確認へ引き継ぐ。

#### 独立実装レビュー r1 (2026-11-14、Sol / xhigh)

上記実装 `9044445e6` に対する設計担当の独立実装レビューは「指摘修正後に受入可」。
構造的変更であることには合意し、P2 1件を指摘した。今回の修正基点は文書記録を含む `bc5c3415a`。
上記の当時の検証・レビュー記録は履歴として残し、この指摘の修正と検証を以下に追記する。

**P2 — 遅着した同一ホストの保守要求が、fork / 再開で確定した独立性を巻き戻す。**
連動 media A で rect 同期等を発行すると、generic native request が発行時の独立性 false を保存していた。
D の実 activation は `park_current_viewer_context_as_live_media_inner` で A を新 context C に fork し、
独立 true と runtime linked=false を確定する。request も C に移るが、旧要求の Apply が保存済み false を
session / runtime に書き戻していた。A の再 activation 後に完了した場合、次の画像送りが B への交換にならない。
D が active のままの完了でも、A の独立性が巻き戻る。前回の same-host apply 所有分離だけでは防げなかった。

**修正（BA-7）:**

- `src/app/presentation_transition.rs`: 既存 `PresentationRequest.target_independent` を `Option<bool>` にし、
  factory は `None`、明示 F12 setter だけ `Some(解決値)` を保存する。
- `src/app/native_video.rs`: generic request では独立性を捕捉しない。実 Apply effect で `None` を
  その時点の mounted C の独立性へ解決し、session と runtime linked flag へ同じ値を渡す。
  明示 F12 は従来どおり保存した解決値を確定する。同 host の同期 F12 / 反転時の同期確定も維持する。
- `src/app/detached_media_exchange_tests.rs`: 実 linked open → D activation / fork → A 再 activation →
  旧 native 完了 → 実画像 navigation で B へ交換する回帰と、D が active のまま完了する回帰を追加。
  どちらも bool を手で立てず、C の独立性・runtime・binding、一覧 / 順序・main・D の内容 / binding / runtime を検査する。
  明示 F12 の対照は実 Fullscreen open → 実 F12 toggle による Detached 要求 → 既存の独立化部品で現在値 true →
  native 完了の実 Apply で `Some(false)` を確定する。host / decoder の事実だけを headless seam で供給する。
- `src/app/tests.rs`: 既存 host effect fixture の request literal を新しい保守要求の型（`None`）に合わせる。
  assertion は変更しない。
- 本書と `docs/detached-rework-plan.md` §11 に記録する。変更は以上の6ファイル。

待ち / activation 制限や guard を追加して組合せを抑える案は、通常操作を制限するため採らない。
保守要求が独立性の決定を運ばない型にすることで、時間順の例外処理を増やさず正本を C に一本化する。
新しい App の bool / Option / pending、retry、追加 repaint、silent fallback は加えない。
request の abort / retire 中の後継は外側 `Preparing.request` が所有し、既存 setter で更新できる。
内側の旧 aborted / retiring request は変更しない。setter 前に新要求の payload が effects に逃げる経路もない。

**同型経路の列挙監査（実装者と独立 Sol / xhigh）:**

| 経路 | 独立性の扱い・結論 |
| --- | --- |
| native rect 同期、音声モード復帰の generic switch、F11 の main / fullscreen 間要求 | `None`。発行時の独立性を持たず、完了時の C を継承する。activate の有無を F12 の代用にはしない |
| native 明示 F12、同 host F12、反転して現在の表示先へ戻る F12 | 明示の非同期要求だけ `Some`。同期確定 / reducer の Stable 短絡は既存 handler がその場で確定する |
| `apply_video_presentation_switched` 同期 wrapper | 呼出時に解決し、非同期 request に古い bool を保存しない。非同期の実 Apply は上記 typed request の確定点に集約する |
| `apply_egui_viewer_presentation_for_current_item`、`commit_viewer_session_deferred_feedback` | F12 の同 stack 同期確定。後者で deferred なのは通知だけであり、古い独立性を非同期保存しない |
| descriptor 開始 / 再開、列挙完了 | binding の初期確定は開始時の runtime / 設定から同期解決。列挙後の open は現在の binding / session を継承し、旧 bool を持たない |
| folder / PDF / ZIP の遅延 reopen | close を挟む pair の捕捉・復元は同 stack。worker 完了は現在の binding / session で open し、独立性の旧値を書き戻さない |
| ViewerSession / registry の swap、fork / resume | C 自身の正本の所有移動、または独立への明示移行。遅着結果による scalar の書き戻しではない |
| NativeVideoOpenPending、source-swap、VST deferred、Manual / Collection の遅延確定 | 独立 bool を payload に持たず、現在の C の load / open に着地する。追加の同型 P2 は確認できなかった |

**旧コードでの再現:**

`cargo nextest run -p mimageviewer --lib -E 'test(/independent_media_exchange_request_independence/)' --no-fail-fast`
を指定 `run-heavy.ps1` 経由で実行。**3実行 / 0成功 / 3失敗、exit 100**、compile 9分05秒、実行0.465秒。
証拠は `target/767-issue1-r1-red.log`。保守要求の2件は、実 fork 後 true の事前条件を満たしてから、
旧 Apply 後 false（期待 true）になることを確認し、P2 の再現とした。
残る1件は旧 F12 対照 fixture の setup failure。同 host の AwaitingHost 要求を置換すると reducer が Stable へ
短絡するため遅延 Apply に到達しなかった。上記の Fullscreen→Detached fixture に直し、製品 reducer は変更しない。

**修正後の検証:** すべて同じ worktree で指定 runner 経由・直列とし、
設計担当の全件ロックがあれば解放を待つ。全件 gate / 検証用 build / 製品起動 / commit / push は実施しない。

| コマンド | 件数 / exit code / 証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/independent_media_exchange/)' --no-fail-fast` | **21成功 / 0失敗、exit 0**。compile 12分42秒、実行15.204秒。`target/767-issue1-r1-focused.log`。新規3件と既存同 host / 反転 F12・descriptor / 遅延 reopen を含む |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)' --no-fail-fast` | **1,557成功 / 0失敗、exit 0**。build確認3.06秒、実行240.304秒。`target/767-issue1-r1-regression.log`。depth focus の1件は SLOW 表示（65.364秒）だったが成功 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/native_video\|video_audio\|video_presentation\|presentation_transition/)' --no-fail-fast` | **197成功 / 0失敗、exit 0**。build確認2.45秒、実行26.901秒。`target/767-issue1-r1-native.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0**、3分08秒（テスト実行なし）。`target/767-issue1-r1-check.log` |
| `cargo fmt`（最後の Cargo 操作、引数なし） | **exit 0**。`target/767-issue1-r1-fmt.log`。以後の Rust 変更なし |
| `git diff --check` | **exit 0**。`target/767-issue1-r1-diff-check.log` |

指定の自動検証を完了した。描画 / レイアウト / UI文字列の変更はなく、ui_snapshot / glyph / site card の再実行は不要。
実 HWND / focus / F11・DPI / 実再生などの実機確認は今回未実施で、headless の状態・handler 検証と区別する。
こちらから全件 gate・検証用 build・製品起動・commit / push は実施していない。vendor 差分なし、
ブランチ `feat-767-stage2` / HEAD `bc5c3415a` のまま。独立 Sol / xhigh が整形後の差分と全証跡を照合し、
r1 P2 の指定スコープを **受入可、未解決 P1 / P2 なし** と判定した。

#### 独立実装レビュー r2 (2026-11-14、設計担当が依頼した別セッションの GPT-6.1 Sol / xhigh、read-only、対象 `01c9b2195`)

**受入可、P1 / P2 / P3 なし**。r1 の P2 は解消。保守要求の完了は `None` を所有 context の確定値へ解決し、追加テストは実 linked open → D の activation / fork →
A の再 activation 後の完了と、D が active のままの完了を検査している。F12 以外で表示先が変わる実呼出は `toggle_video_window_mode` の MainWindow ↔ Fullscreen だけで、
非 detached の独立性 false を継承するので正しい。新規 open とメディアウィンドウ設定による移動は `prepare_viewer_presentation_open` が同期で確定する。
F12 の経路 (native・egui・`toggle_detached_viewer_mode`) は明示値を保つ。**症状パッチではなく構造的な変更であることに合意** (保守完了が参照する独立性の正本を
所有 context に一本化した)。r1 (`9044445e6`) の判定「指摘修正後に受入可・構造的変更に合意」と合わせ、課題 1 の実装を設計担当 (ClaudeCode) が検収した。

#### 全件 gate の失敗 2 件の修正 (2026-11-14)

設計担当が `600b0a99d` で実行した本体 lib 全件は **17,643実行 / 17,641成功 / 2失敗、exit 100**。
証拠は `target/test-lib-full/runs/20261113T192240046Z-e0d51430/stderr.log`。
今回の作業基点も同じ HEAD とし、以下の2件を修正した。全件 gate 自体はこちらから再実行しない。

**prepared 内部本の独立性（BA-7）:**

`section1675_multiwindow_scope_literal_prefix_prepared` は、loading shell が private binding を予約し、
`(DetachedWindow, true)` を確定した後、prepared ZIP の同期 install 中に失っていた。
`finalize_zip_enumerate` → `install_prepared_zip_grid` → 一覧の内容入替 → 内部 content close の
`prepare_viewer_presentation_close` が、公開後の active session だけで生存を判定していたためである。
Building の予約はまだ逆引きに公開されておらず、session pair が非 detached / false に消された。
`9044445e6` より前は、その後の open が設定から独立性を再決定してこの欠落を隠していた。
設定の再参照や install 後の true の再代入で修復せず、予約から公開まで同じ所有者の確定値を保つ。

- `src/app.rs`: 内容だけの close は、公開済みの既存生存条件に加え、Building の projected context が
  Detached session と private binding の生存 runtime を所有している間、その初期 pair を維持する。
  Closing / runtime 不在にはこの保持を適用しない。連動 false もそのまま保持し、独立へ強制しない。
  terminal close / abort の binding 退役・非公開規則は変更しない。
- loading shell と Collection root の新規 context は、予約直後に session pair と runtime linked flag を
  一緒に初期確定する。descriptor builder と同じ順序とし、ページ open 前の runtime も独立にそろえる。
  新しい App 状態、描画 guard、待ち、retry、追加 repaint、一括 reset は追加しない。
- `src/app/archive_inner_book_s3.rs`: 元の期待をすべて残し、通常 ZIP / 変換元付き ZIP の両方で
  prepared install 前後の pair・runtime と、公開前の private binding を追加検査する。
  同じ context の内容・literal prefix / raw scope・main 不変も従来どおり検査する。
- `src/app/tests.rs`: 実 loading-shell 入口と Collection root の既存テストを強化し、
  新規 `detached_scanned_folder_binding_session_follows_new_open_setting` は実 ScannedFolder handler で
  設定から得た独立 true / 連動 false の両方、runtime / binding、main の一覧・選択・世代不変を検査する。

**同型入口の確認:** production の新規 `build_viewer_context` は次の3系統で、残りの直接呼出しは test fixture。

| 予約・公開の所有者 | 入口 / prepared・deferred の扱い |
| --- | --- |
| `start_active_detached_loading_context` | §1.675 の prepared scope / literal prefix 回帰が直接使う先行 shell。production では Bookmark / startup、FolderCandidate / ConvertibleArchive が共用する。同期 prepared install の欠落を上記共通 close で修正し、非同期完了は公開済みの同じ pair を継承する |
| `start_active_detached_book_context_with_start` | 新規 / 再開の descriptor（フォルダ・PDF・ZIP・画像、§1.675 の ZIP BookScope を含む）と ScannedFolder。予約時の pair / runtime 初期確定は既存どおり。同期 ScannedFolder でも今回の保持規則が必要。deferred enumerate / reopen は所有 context の現在値を継承し、新たな設定の再決定はしない |
| `start_active_detached_collection_root_image` | 独自の一覧 install は共有 content close を通らず pair の欠落はないが、予約時の runtime 初期値も session に一致させる。既存の独立 true 方針と一覧 policy / identity は変えない |

ScannedFolder の直接入口は現在 test wrapper のみだが、共有 factory の同期 install 分岐も実経路で検査する。

**close seam の source-text test:**

`the_close_and_retire_seam_finishes_the_transition_it_starts` は課題2の `53d224a5c` で本体が
明示 id の `take_and_close_context_with_cause` へ移ったことに追従していなかった。
検査対象をその本体へ移し、帰還の移譲 → `close_and_retire_context` の順序、terminal closure の
`close_fullscreen_to_completion`、Windows 側に素の `close_fullscreen()` がないこと、終端 owner の一意性を維持する。
active wrapper が active id を先に解決して typed cause とともに本体へ渡すことも追加検査する。
明示 id のもう1つの入口は `park_detached_viewer_for_handoff` で、snapshot がない移動元を同じ本体へ渡す。
`take_detached_snapshot_for_activation` が、取り出す前に捕捉した source をこの handoff へ渡す順序も検査する。
これらはテストの更新だけで、close 本体や課題2の所有移動は変更しない。

変更ファイルは上記 Rust 3ファイルと本書、`docs/detached-rework-plan.md` の5ファイル。
独立 Sol / xhigh は実行前の差分と同型入口の監査を確認し、追加 P1 / P2 なし、予約中の所有境界を直す構造修正と評価した。

**検証記録:** Cargo は指定 `run-heavy.ps1`（Owner `codex/767-issue1`、WorkDir は本 worktree）経由で直列実行し、
他セッションの全件ロックがあれば変更せず解放を待つ。既存の検証と同じく jobs=1 / incremental=0 / MSBuild node reuse 無効を使用する。

- 修正前の `600b0a99d` で失敗2件だけを再実行し、**2実行 / 0成功 / 2失敗、exit 100**。
  `cargo nextest run -p mimageviewer --lib -E 'test(section1675_multiwindow_scope_literal_prefix_prepared) | test(the_close_and_retire_seam_finishes_the_transition_it_starts)' --no-fail-fast`。
  compile 9分31秒、実行0.580秒、`target/767-gate-current-red.log`。
- 旧版比較は worktree 内の `target/767-gate-old-01f5e898f` に Git blob から物理コピーを作り、
  `src` の1,036 blob 全件が `01f5e898f` と一致することを検証した。
  同じ版の依存（`Cargo.toml` / lock / build.rs / crates / vendor に基点との差分なし）を使うため、
  snapshot の manifest だけ path dependency を既存実体の絶対パスへ変更し、共通 target を指定する。
  native の必要資産は実体コピーし、junction / symlink は作らない。比較対象の App ソースと既存テストは変更しない。
  一致検証の証拠は `target/767-gate-old-source-verify.log`（exit 0）。
  `CARGO_TARGET_DIR` を本 worktree の target にして、
  `cargo nextest run --manifest-path .\target\767-gate-old-01f5e898f\Cargo.toml -p mimageviewer --lib -E 'test(section1675_multiwindow_scope_literal_prefix_prepared)' --no-fail-fast`
  を同じ runner 経由で実行し、**1実行 / 1成功 / 0失敗、exit 0**。compile 12分24秒、実行0.827秒。
  `target/767-gate-old-test.log`。旧版で同じ既存テストが通り、基点で落ちる退行を確認した。
  runner は別セッションの全件ロック解放を待ち、解放後は書込不可の警告に従って slot なし・低優先度で続行した。

修正途中の最初の2件検証は **2実行 / 1成功 / 1失敗、exit 100**（compile 8分16秒、実行0.551秒、
`target/767-gate-two-r1.log`）。close seam は成功したが、prepared install 後の追加 pair 検査で Fullscreen を検出した。
Building の保持条件で用いた `viewer_session_is_detached()` は `fullscreen_idx.is_some()` も要求するため、
ページがない初期予約を表せなかった。同じ所有境界で予約済みの `viewer_presentation == DetachedWindow` を直接参照する形に直し、
テストでも最初のページ前であることを明示する。設定の再決定や後段の修復は加えない。

**修正後の最終検証（指定 runner 経由・直列）:**

| コマンド | 件数 / exit code / 証拠 |
| --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(section1675_multiwindow_scope_literal_prefix_prepared) \| test(the_close_and_retire_seam_finishes_the_transition_it_starts)' --no-fail-fast` | **2成功 / 0失敗、exit 0**。compile 7分58秒、実行0.820秒。`target/767-gate-two-green.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section1675\|archive_inner_book\|multiwindow\|close_and_retire/)' --no-fail-fast` | **160成功 / 0失敗、exit 0**。build確認1.02秒、実行16.511秒。`target/767-gate-related.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/independent_media_exchange/)' --no-fail-fast` | **21成功 / 0失敗、exit 0**。build確認0.98秒、実行3.690秒。`target/767-gate-focused.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode\|panorama\|detached\|multiwindow_scenario\|slideshow\|admission/)' --no-fail-fast` | **1,558成功 / 0失敗、exit 0**。build確認0.97秒、実行102.260秒。`target/767-gate-regression.log`。新規 ScannedFolder の両設定と、強化した loading shell（所属 module の `window_mode` で選択）/ Collection root、課題2/3・abortの既存対照を含む |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0**、56.95秒（テスト実行なし）。`target/767-gate-check.log` |
| `cargo fmt`（最後の Cargo 操作、引数なし） | **exit 0**。`target/767-gate-fmt.log`。以後の Rust 変更なし |
| `git diff --check` | **exit 0**。`target/767-gate-diff-check.log`。既存 LF→CRLF 設定の警告のみ |

指定の自動検証を完了した。既存テストの期待をすべて維持し、予約中の pair / runtime と入口の委譲検査を追加した。
描画・レイアウト・UI文字列の変更はなく、ui_snapshot / glyph / site card は再実行しない。
実 HWND / focus / F11・DPI / 実再生などの実機確認と、本体 lib 全件 gate の再実行は未実施。
全件 gate・検証用 build・製品起動・commit / push は今回の指示どおり実行していない。
変更は上記5ファイルで vendor 差分なし、ブランチ `feat-767-stage2` / HEAD `600b0a99d` のまま。
独立 GPT-6.1 Sol / xhigh が整形後の5ファイル差分と全証跡を照合し、今回の修正は **受入可、未解決 P1 / P2 なし** と判定した。

#### 独立実装レビュー r3 (2026-11-14、設計担当が依頼した別セッションの GPT-6.1 Sol / xhigh、read-only、対象 `178a3ece2`)

**受入可、P1 / P2 / P3 なし**。原因 (同期 install の内容 close が `prepare_viewer_presentation_close` で公開前の Building の session pair を非 detached / false に解除) の説明は正しく、
`preserve_detached_for_folder_nav` の条件は registry 所有の私的予約と同じ窓の runtime から生存を判定するもので、Mounted には影響せず、Closing / runtime 不在は保持しない。
取消と build abort で保持した pair が残る経路は無い。同型の経路 (loading shell / collection root / descriptor / ScannedFolder) に取りこぼしは無い。テストは弱めていない。
**症状パッチではなく構造的な変更であることに合意** (内容の close と予約済み表示 session の寿命を区別し、公開前後で同じ所有者の確定値を保つ)。設計担当 (ClaudeCode) も同じ判断で検収した。

#### 全件 gate と確認用 build (2026-11-14、設計担当、すべて run-heavy.ps1 経由)

| コマンド | 結果 |
|---|---|
| `.\scripts\test-lib-full.ps1` (`-FullTest`、コード `178a3ece2`) | 1 回目 (`01c9b2195`) は 17,643 件中 2 件失敗 (上記で修正)。2 回目 **17,644 件成功 / 0 件失敗、exit 0** (901.6 秒)。実行中に文書だけのコミットを入れたため、再利用の記録は残っていない |
| `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-run` | 1 回目はページング ファイル不足 (os error 1455、並行ビルドによる) で失敗、`-j 4` で再実行して **exit 0** |
| `.\scripts\build-dev.ps1` | **exit 0**。`target\dev-runtime\mimageviewer-core.exe` (2026-11-14 06:51) |

描画・レイアウトの変更が無いので ui_snapshot は回していない。実機確認 (サブ PC、手順 `C:\miv-subpc\steps-767-stage2.md`) は未実施。

### 17.19 サブPC依頼50の原因調査・修正設計・失敗回帰（2026-11-14、製品未変更）

基点 `d96e47c0b` / 製品 `178a3ece2`。
サブPC担当の観測と通常ログ、ソース、現コード／旧コードの同じテストを照合した記録は
[依頼50の調査と修正設計](depth-parallax-realmachine50-investigation.md) を正本とする。
停止した窓のclick配送、外部openの旧Depth ownerの孤児化、rectによるキーHWND誤同定を分ける。
実機Aの入力欠落の瞬間は通常ログから未確定であり、再現できるcallback欠落の機構と区別する。
既存3テストファイルへ5件の失敗回帰を追加し、製品・ログ・vendorは変更しない。
失敗出力・コマンド・件数・exit code、BA分類、修正案、次段のscope判断、未確認点は正本§1〜8に記録する。
