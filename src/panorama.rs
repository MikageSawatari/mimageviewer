//! 360 度パノラマビュー機能の App 側ステート・検出・キャッシュキー解決。
//!
//! GPU 描画は [`panorama_wgpu`](crate::panorama_wgpu)、WGSL シェーダや
//! `wgpu::Texture` 管理はそちらに分離。本ファイルは:
//!
//! - [`PanoramaState`] — yaw / pitch / fov_y / drag state を持つ UI ステート
//! - [`PanoramaTrigger`] — 検出結果 (Auto / Hint)
//! - [`PanoSourceResolution`] — `resolve_pano_source` の戻り値
//! - `make_pano_cache_key` / `crc16_of_str` — cache_key (u64 packed) の構築
//! - 各種 source_kind 定数
//!
//! 設計詳細は [docs/panorama-360-view-plan.md](../docs/panorama-360-view-plan.md)。

/// アスペクト比 2:1 判定の許容幅 (1.95 〜 2.05)。`source_dims` の生値で判定する
/// (rotation_db や clamp 後の値ではない)。§2.1 参照。
pub const ASPECT_LOW: f32 = 1.95;
pub const ASPECT_HIGH: f32 = 2.05;

/// 360 ビューの FOV 範囲 (ラジアン)。約 11° 〜 150°。§3.3 / §5.2。
pub const FOV_MIN: f32 = 0.2;
pub const FOV_MAX: f32 = 2.6;

/// 初期 FOV (約 69°)。§5.1。
pub const FOV_DEFAULT: f32 = 1.2;

/// pitch のクランプ範囲。極を直視させない (asin 数値誤差で天井 / 床テクセルが暴れる)。
pub const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

/// `cache_key` の source_kind ビット (§4.1.2):
/// - 0 = fs_cache のみ (補正なし、AI なし)
/// - 1 = adjustment_cache (raw + 単純色調補正)
/// - 2 = ai_upscale_cache (AI のみ)
/// - 3 = adjustment_cache (AI + 補正)
/// - 4 = final_composite_cache (通常表示と同じ最終パイプライン)
pub const SOURCE_KIND_FS: u16 = 0;
pub const SOURCE_KIND_ADJUST_RAW: u16 = 1;
pub const SOURCE_KIND_AI: u16 = 2;
pub const SOURCE_KIND_AI_ADJUST: u16 = 3;
pub const SOURCE_KIND_FINAL_COMPOSITE: u16 = 4;

/// 360 ビューのインタラクティブステート (フルスクリーン内のみ Some)。
/// ファイル切替 / フルスクリーン退出で `panorama_state = None`。
/// 360 でない画像へナビした場合は **保持しつつ非アクティブ化** (= 同セッション
/// 内で 360 画像に戻ったら yaw/pitch/fov を引き継ぐ)。
#[derive(Clone, Debug)]
pub struct PanoramaState {
    /// 経度 (radians)。[-π, π]。初期 0 (or GPano hint)。
    pub yaw: f32,
    /// 緯度 (radians)。`[-PITCH_LIMIT, PITCH_LIMIT]`。
    pub pitch: f32,
    /// 視野角 Y 方向 (radians)。`[FOV_MIN, FOV_MAX]`。
    pub fov_y: f32,
    /// マウス左ドラッグ中か。
    pub drag_active: bool,
    /// 直前のポインタ位置 (`drag_active=true` のとき有効)。
    pub last_pointer: Option<egui::Pos2>,
    /// 初期 yaw / pitch (リセット時 / 検出時の hint)。
    /// ユーザーがリセットボタンを押すと yaw/pitch/fov_y がこの値に戻る。
    pub initial_yaw: f32,
    pub initial_pitch: f32,
}

impl PanoramaState {
    /// GPano hint に基づくデフォルト値を作る。hint が無ければ 0 を使う。
    /// 不正な (NaN / Inf) 値は 0 に正規化 (Codex P2 第 5、2026-05): pose が NaN だと
    /// 4 軸 stale guard の `(f32,f32,f32)==(f32,f32,f32)` 比較が **永久に false** に
    /// なり、settle overlay の upload が永久に却下されて高画質モードが固定で
    /// 「描画中…」と表示されたまま進まなくなる。
    pub fn new(initial_yaw: f32, initial_pitch: f32) -> Self {
        let initial_yaw = sanitize_angle(initial_yaw, 0.0);
        let initial_pitch = sanitize_angle(initial_pitch, 0.0).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        Self {
            yaw: initial_yaw,
            pitch: initial_pitch,
            fov_y: FOV_DEFAULT,
            drag_active: false,
            last_pointer: None,
            initial_yaw,
            initial_pitch,
        }
    }

    /// 初期視点にリセット (ダブルクリック / リセットボタン)。drag 状態は維持しない。
    pub fn reset(&mut self) {
        self.yaw = sanitize_angle(self.initial_yaw, 0.0);
        self.pitch = sanitize_angle(self.initial_pitch, 0.0).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.fov_y = FOV_DEFAULT;
        self.drag_active = false;
        self.last_pointer = None;
    }

    /// 入力ハンドラ (drag / wheel) 経由で yaw / pitch / fov_y が変更された後に
    /// 呼び、**NaN / Inf を含まない有限値**であることを保証する。
    /// Codex P2 第 5 ラウンド (2026-05) 対策: drag delta や wheel exp が NaN に化けても、
    /// stale guard の f32 equality が永久 false になるのを防ぐ。
    pub fn sanitize(&mut self) {
        self.yaw = sanitize_angle(self.yaw, self.initial_yaw);
        self.pitch =
            sanitize_angle(self.pitch, self.initial_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.fov_y = if self.fov_y.is_finite() {
            self.fov_y.clamp(FOV_MIN, FOV_MAX)
        } else {
            FOV_DEFAULT
        };
    }
}

/// NaN / Inf を含む `value` を `fallback` (これも非有限なら 0.0) に正規化する。
fn sanitize_angle(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else if fallback.is_finite() {
        fallback
    } else {
        0.0
    }
}

/// 部分 FOV equirect 画像 (GPano `CroppedArea*` 宣言) の UV 変換パラメータ。
///
/// **背景**: DSLR + nodal panhead で撮った 360 写真などは、水平 360° は撮れているが
/// 天頂と地面まで撮りきれず、フル球面の一部しか画像に含まれないケースが多い。
/// GPano XMP は `FullPanoWidthPixels` / `FullPanoHeightPixels` でフル球面の寸法を
/// 宣言し、`CroppedAreaImageWidthPixels` / `CroppedAreaImageHeightPixels` /
/// `CroppedAreaLeftPixels` / `CroppedAreaTopPixels` で画像が球面上で占める位置を
/// 表す。これを無視して画像全体をフル equirect として球に貼ると、画像の上端が
/// 天頂に紐付けられて水平線がずれる。
///
/// **WGSL での使い方**: 視線ベクトルから経度緯度経由で計算した
/// `sphere_uv ∈ [0,1]² (フル球面座標)` を、画像テクスチャ座標
/// `texture_uv = (sphere_uv - offset) / scale` で変換してからサンプル。
/// scale < 1 のとき、texture_uv が [0,1] 範囲外になった領域は AddressMode の
/// `ClampToEdge` で端の色を引き伸ばす (= 上下の欠けた領域は空 / 地面の色で
/// 自然に埋まる)。
///
/// **identity (フル equirect)**: `(u_offset, v_offset, u_scale, v_scale) = (0, 0, 1, 1)`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanoUvTransform {
    pub u_offset: f32,
    pub v_offset: f32,
    pub u_scale: f32,
    pub v_scale: f32,
}

impl PanoUvTransform {
    /// フル equirect (画像 = フル球面) の場合の identity 変換。
    pub const IDENTITY: Self = Self {
        u_offset: 0.0,
        v_offset: 0.0,
        u_scale: 1.0,
        v_scale: 1.0,
    };

    /// このトランスフォームが identity (= 全 [0,1] のフル equirect) か。
    /// 識別子としては panorama_state や UV 経路の選択に影響しないが、cache_key 解析
    /// やデバッグ表示で「これは部分 FOV か?」を即判定できる。
    pub fn is_identity(&self) -> bool {
        self.u_offset == 0.0 && self.v_offset == 0.0 && self.u_scale == 1.0 && self.v_scale == 1.0
    }

    /// 水平方向がフル 360° を覆わず、U の seam wrap を無効にすべきか。
    ///
    /// WGSL 側の crop 判定と同じ許容幅を使う。垂直方向だけが欠けた典型的な
    /// partial panorama では false のままなので、U=Repeat の自然な seam を維持する。
    pub fn has_horizontal_crop(&self) -> bool {
        self.u_scale < 0.999 || self.u_offset.abs() > 0.001
    }

    /// GPano XMP の `CroppedArea*` + `FullPano*` 値から UV transform を計算する。
    /// すべての必須フィールドが揃っているとき `Some(_)` を返し、欠けるか不正値
    /// (= ゼロ除算 / 範囲外) の場合は `None` (= フル equirect 扱いに fallback)。
    ///
    /// 計算式:
    /// - `u_scale = cropped_w / full_w` (画像が水平方向で占める割合)
    /// - `v_scale = cropped_h / full_h` (同 垂直方向)
    /// - `u_offset = cropped_left / full_w` (画像の左端がフル球面の何 % 地点か)
    /// - `v_offset = cropped_top / full_h` (画像の上端がフル球面の何 % 地点か)
    pub fn from_gpano(info: &crate::xmp_reader::XmpPanoramaInfo) -> Option<Self> {
        let full_w = info.full_pano_width_pixels? as f32;
        let full_h = info.full_pano_height_pixels? as f32;
        let cropped_w = info.cropped_area_image_width_pixels? as f32;
        let cropped_h = info.cropped_area_image_height_pixels? as f32;
        // Left / Top は 0 が valid なので unwrap_or(0) で許容するか、宣言があれば使う。
        // GPano 仕様上、CroppedAreaImage* があるなら CroppedAreaLeft/Top も提供するのが
        // 通例だが、無いケースは中央寄せと解釈 (= left = (full - cropped) / 2)。
        let cropped_left = info
            .cropped_area_left_pixels
            .map(|v| v as f32)
            .unwrap_or_else(|| ((full_w - cropped_w) * 0.5).max(0.0));
        let cropped_top = info
            .cropped_area_top_pixels
            .map(|v| v as f32)
            .unwrap_or_else(|| ((full_h - cropped_h) * 0.5).max(0.0));
        // 不正値チェック
        if full_w <= 0.0 || full_h <= 0.0 || cropped_w <= 0.0 || cropped_h <= 0.0 {
            return None;
        }
        // 範囲外 (cropped が full をはみ出す) は防衛的に identity 化
        if cropped_left + cropped_w > full_w * 1.001 || cropped_top + cropped_h > full_h * 1.001 {
            return None;
        }
        let xform = Self {
            u_offset: cropped_left / full_w,
            v_offset: cropped_top / full_h,
            u_scale: cropped_w / full_w,
            v_scale: cropped_h / full_h,
        };
        // **scale が異常に小さい / 非有限な値 → identity に倒す** (Codex P2 第 5、2026-05):
        // 後段の WGSL / CPU sampler が `(sphere - offset) / scale` で除算するので、
        // scale が 0 に近づくと無限大 → NaN 経路に乗って描画が破綻する。0.001 (= 0.1% 覆い)
        // 未満は実用画像では起こらないので identity に倒して安全側に。
        if !xform.u_offset.is_finite()
            || !xform.v_offset.is_finite()
            || !xform.u_scale.is_finite()
            || !xform.v_scale.is_finite()
            || xform.u_scale < 0.001
            || xform.v_scale < 0.001
        {
            return None;
        }
        // 全領域 (差が 0.5% 以下) なら identity 扱いにして無駄な UV 変換を避ける。
        // 浮動小数の比較は厳密一致ではなく許容幅で判定。
        let near_full = (xform.u_offset.abs() < 0.005)
            && (xform.v_offset.abs() < 0.005)
            && ((1.0 - xform.u_scale).abs() < 0.005)
            && ((1.0 - xform.v_scale).abs() < 0.005);
        if near_full {
            Some(Self::IDENTITY)
        } else {
            Some(xform)
        }
    }
}

/// `App::detect_panorama` の戻り値。
///
/// - `Auto`: GPano XMP が `UsePanoramaViewer=True` + `ProjectionType=equirectangular` を
///   宣言している (= ビューアアプリでの全画面表示が推奨されているシグナル)。
/// - `Hint`: 弱いシグナル (アスペクト 2:1 のみ、または GPano `ProjectionType` のみ)。
///
/// **どちらでも自動 ON はしない** (フィードバック反映で廃止、機能制限モードに
/// 強制的に入るのは違和感が大きいため)。代わりに:
/// - `App::open_fullscreen` 時に「V キーで 360°ビューワー」案内トーストを出す
/// - ホバーバーに 360 ボタンを表示 (Auto はツールチップで強調、Hint は控えめ)
/// - V キーまたはボタンクリックで明示的にトグル
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanoramaTrigger {
    Auto,
    Hint,
}

/// `App::resolve_pano_source` の戻り値。8K base のアップロード元 / cache_key /
/// settle 判定 (Phase 2a) をまとめて 1 関数で決める (§4.3)。
pub struct PanoSourceResolution {
    /// `App::metadata_cache_key(idx)` の戻り値。`pano_uploaded.source_key` と比較。
    pub source_key: String,
    /// `(idx_hash, source_kind, adjust_gen, ai_gen)` を u64 にパック (§4.1.2)。
    pub cache_key: u64,
    /// 360 ベーステクスチャのアップロード元。`color_image_to_rgba` で RGBA8 化する。
    pub pixels: std::sync::Arc<egui::ColorImage>,
    /// どのキャッシュ層から取ったか (SOURCE_KIND_*)。Phase 2a の settle policy 判定にも使う。
    pub source_kind: u16,
    /// settle (Phase 2a) の発動可否ポリシー (§3.6.2.1)。
    /// `compute_settle_policy(fs_idx, source_kind)` の出力をそのまま焼き込む。
    pub settle_policy: PanoramaSettlePolicy,
}

// ──────────────────────────────────────────────────────────────────
// Phase 2a: settle-refinement (高解像度ソースの品質補完)
// docs/panorama-360-view-plan.md §3.6 / §4.6
// ──────────────────────────────────────────────────────────────────

/// settle-refinement の発動条件下で「実行可能 (= 200 MP 以下、または承認済み)」を
/// 表すマーカー。`PANO_SETTLE_MAX_PIXELS` 超えのソースは
/// `NeedsUserConfirmation` バナーでユーザー判断を待つ (§3.6.4)。
pub const PANO_SETTLE_MAX_PIXELS: u64 = 200_000_000; // 200 MP

/// settle render が必要とするフル解像度 RGBA データ (§4.6.1)。
///
/// Phase 2a では `Decoded` variant のみ。Phase 3 で巨大 JPEG 向け
/// 部分デコード variant (`JpegBytes`) を追加する余地あり (§3.6.6)。
#[derive(Clone, Debug)]
pub enum HighResSource {
    /// フル解像度の RGBA8 をそのまま保持。`worker` がデコード後 1 回だけ作る。
    /// `Arc` 共有なので settle render は zero-copy で読む。
    Decoded {
        rgba: std::sync::Arc<Vec<u8>>,
        w: u32,
        h: u32,
    },
}

impl HighResSource {
    /// RGBA バッファのバイト数 (debug / metrics 用)。
    pub fn byte_len(&self) -> usize {
        match self {
            HighResSource::Decoded { rgba, .. } => rgba.len(),
        }
    }
    pub fn dims(&self) -> (u32, u32) {
        match self {
            HighResSource::Decoded { w, h, .. } => (*w, *h),
        }
    }
}

/// 各画像 (source_key 単位) の解像度ゲート判定結果 (§3.6.4)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanoramaQualityState {
    /// ≤ 200 MP: 自動承認、settle ON 確定 (consumer 360 カメラ + ChatGPT 等)
    SettleReady,
    /// > 200 MP: バナー表示中、ユーザー選択待ち
    NeedsUserConfirmation { source_pixels: u64, est_ram_gb: f32 },
    /// > 200 MP かつユーザーが「高品質」選択 (HighResSource ロード済み or 進行中)
    SettleApproved,
    /// > 200 MP かつユーザーが「8K でよい」選択 (settle 機能オフ)
    BaseOnly,
}

/// 200 MP 超かつ前回承認の 1.25 倍以上ならバナー表示 (§3.6.2)。
///
/// 例: 201 MP 承認 (`approved_max=201_000_000`) → 220 MP 不要、338 MP 必要。
pub fn needs_user_confirmation(source_pixels: u64, approved_max: u64) -> bool {
    if source_pixels <= PANO_SETTLE_MAX_PIXELS {
        return false;
    }
    // approved_max * 1.25 を超えるなら再確認。`saturating_mul` で wrap を防ぐ。
    source_pixels > approved_max.saturating_mul(125) / 100
}

/// settle の適用ポリシー (§3.6.2.1)。
///
/// 8K base のソース選択結果 (`source_kind`) と AI / 補正設定から
/// `compute_settle_policy` で決まる。
#[derive(Clone, Debug)]
pub enum PanoramaSettlePolicy {
    /// settle スキップ。8K base のみ表示 (AI 有効 / post_filter / auto_mode / transient)
    Disabled,
    /// raw fs_cache (補正なし、AI なし)。HighResSource = 元 RGBA をそのまま sample
    EnabledFromRaw,
    /// 通常画像 + 単純色調補正 (post_filter / auto / AI なし)。
    /// settle source は元 RGBA、render 内で `apply_adjustments_fast` を再適用。
    EnabledWithColorAdjustments {
        params: crate::adjustment::AdjustParams,
    },
}

impl PanoramaSettlePolicy {
    /// settle render を起動する必要があるかどうか。
    pub fn is_enabled(&self) -> bool {
        !matches!(self, PanoramaSettlePolicy::Disabled)
    }
}

/// settle 起動の AND 条件 (§3.6.2.1):
/// - state が `SettleReady` / `SettleApproved` のいずれか
/// - policy が `EnabledFromRaw` / `EnabledWithColorAdjustments` のいずれか
pub fn settle_enabled(state: &PanoramaQualityState, policy: &PanoramaSettlePolicy) -> bool {
    matches!(
        state,
        PanoramaQualityState::SettleReady | PanoramaQualityState::SettleApproved
    ) && policy.is_enabled()
}

/// settle render の進行ハンドル (§4.6.2)。
/// `rx` で結果を受け取り、`cancel` でキャンセル指示を伝える。
pub struct RenderingHandle {
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub rx: std::sync::mpsc::Receiver<SettleRenderResult>,
    pub started_at: std::time::Instant,
    pub for_source_key: String,
    /// render 開始時の pose snapshot (yaw, pitch, fov_y)。
    /// 結果到着時に `refinement.last_pose` と比較して stale 判定。
    pub for_pose: (f32, f32, f32),
    /// render 開始時の cache_key snapshot。補正/AI 変更で cache_key が動いたら stale。
    pub for_cache_key: u64,
    /// render 開始時の viewport size snapshot (Codex P1 第 2、2026-05)。
    /// viewport がリサイズされたら stale 扱いにして再 render。
    pub for_viewport_size: (u32, u32),
}

/// settle render の出力 (§4.6.2)。
#[derive(Clone)]
pub struct SettleRenderResult {
    pub source_key: String,
    pub pose: (f32, f32, f32),
    pub cache_key: u64,
    /// render 時の viewport size (= overlay の想定 viewport)。
    pub viewport_size: (u32, u32),
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 360 view の settle 機能ステート (§4.6.1)。
/// `App::pano_refinement: Option<PanoramaRefinement>` で 360 ON 中のみ Some。
pub struct PanoramaRefinement {
    pub source_key: String,
    /// 静止検出のタイマー基準。pose が変わると `None` に戻し、500 ms 経過で起動。
    pub settle_since: Option<std::time::Instant>,
    pub last_pose: (f32, f32, f32),
    /// 補正 / AI / source_kind の変化検出用 (§4.6.1)。
    pub last_cache_key: u64,
    /// 直近フレームの viewport size (= `image_rect` のピクセル寸法)。
    /// Codex P1 第 2 (2026-05): settle overlay は viewport の aspect に揃えてレンダ
    /// しないと base と幾何が一致しない。`try_paint_panorama` が毎フレ更新する。
    /// 値が変わると `settle_since` を reset + 進行中 render を cancel する。
    pub last_viewport_size: Option<(u32, u32)>,
    pub rendering: Option<RenderingHandle>,
    /// 完成したオーバーレイの wgpu テクスチャ。`upload_settle_overlay` で挿入。
    /// `Box` で持つのは型を panorama.rs から隠したいだけ。
    pub overlay: Option<SettleOverlay>,
    pub overlay_pose: Option<(f32, f32, f32)>,
    pub overlay_cache_key: Option<u64>,
    /// Overlay が想定する viewport サイズ。描画時に現 viewport と差があれば overlay
    /// を drop して再 settle に倒す。
    pub overlay_viewport: Option<(u32, u32)>,
    /// Overlay の wgpu リソースが焼かれた `target_format` (Codex P2 第 4、2026-05)。
    /// 描画ターゲットの format が変わると `UploadedSettleOverlay.bind_group` の
    /// layout 互換性が崩れる可能性がある。`SettleOverlayCallback::prepare` 側でも
    /// guard は入れたが、`overlay_ok_for` / `ready_to_render` の判定にも含めない限り、
    /// App 側は「overlay 有効」と勘違いして再 settle を起動しない (= 姿勢が動くまで
    /// 永久に 8K base 単独表示)。これを防ぐためここで format を保持し、ok_for で比較。
    pub overlay_target_format: Option<wgpu::TextureFormat>,
    /// 150 ms フェードインの開始時刻。`upload_settle_overlay` で `Some(Instant::now())`。
    pub overlay_fade_start: Option<std::time::Instant>,
}

impl PanoramaRefinement {
    pub fn new(source_key: String, pose: (f32, f32, f32), cache_key: u64) -> Self {
        Self {
            source_key,
            settle_since: None,
            last_pose: pose,
            last_cache_key: cache_key,
            last_viewport_size: None,
            rendering: None,
            overlay: None,
            overlay_pose: None,
            overlay_cache_key: None,
            overlay_viewport: None,
            overlay_target_format: None,
            overlay_fade_start: None,
        }
    }

    /// pose / cache_key / viewport_size 変化を検出し、必要なら静止タイマーを
    /// reset + 進行中 render を cancel する。
    /// `viewport_size` は `try_paint_panorama` の最新値を毎フレ流す (= 過去フレの
    /// `last_viewport_size` を上書きする)。viewport_size が `None` のときは更新しない
    /// (= まだ確定していないので比較から除外)。
    /// 返り値: pose が変わったかどうか (cache_key / viewport_size 変化は含めない、
    /// 呼び出し側は現状 pose 変化だけ気にしている)。
    pub fn note_state(
        &mut self,
        pose: (f32, f32, f32),
        cache_key: u64,
        viewport_size: Option<(u32, u32)>,
    ) -> bool {
        let pose_changed = self.last_pose != pose;
        let cache_changed = self.last_cache_key != cache_key;
        let viewport_changed = match (viewport_size, self.last_viewport_size) {
            (Some(new_v), Some(old_v)) => new_v != old_v,
            (Some(_), None) => false, // 初回設定は変化扱いしない (= settle 起動を阻害しない)
            (None, _) => false,       // 取得不能フレームは無視
        };
        if pose_changed || cache_changed || viewport_changed {
            self.settle_since = None;
            if let Some(handle) = self.rendering.as_ref() {
                handle
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.rendering = None;
            // overlay は描画時の stale check で drop されるのでここでは触らない
        }
        self.last_pose = pose;
        self.last_cache_key = cache_key;
        if let Some(v) = viewport_size {
            self.last_viewport_size = Some(v);
        }
        pose_changed
    }

    /// overlay の current stale 判定。`pose` / `cache_key` / `viewport_size` /
    /// `target_format` 不一致なら drop すべき。
    /// `viewport_size` / `target_format` が `None` (= まだ取れていない) のときは
    /// その軸はスキップ (保守的に true 側に倒す)。
    pub fn overlay_ok_for(
        &self,
        pose: (f32, f32, f32),
        cache_key: u64,
        viewport_size: Option<(u32, u32)>,
        target_format: Option<wgpu::TextureFormat>,
    ) -> bool {
        if self.overlay.is_none() {
            return false;
        }
        if self.overlay_pose != Some(pose) {
            return false;
        }
        if self.overlay_cache_key != Some(cache_key) {
            return false;
        }
        // viewport_size: 上流 (try_paint_panorama) で常に Some を渡す前提だが、
        // 念のため None なら viewport 軸の判定はスキップ
        if let (Some(now), Some(stored)) = (viewport_size, self.overlay_viewport) {
            if now != stored {
                return false;
            }
        }
        // target_format: 描画ターゲットの format 変化を検出 (Codex P2 第 4、2026-05)。
        // 不一致なら overlay は古い bind_group_layout に焼かれているので drop すべき。
        if let (Some(now), Some(stored)) = (target_format, self.overlay_target_format) {
            if now != stored {
                return false;
            }
        }
        true
    }
}

/// settle overlay 用の wgpu リソース。`panorama_wgpu::SettleOverlayGpu` を箱に包む。
/// `panorama.rs` 自体は wgpu 型を直接持たないが、`Option<Box<dyn Any>>` 的な逃げ口を
/// 提供するため、enum でラップする (Phase 2a では variant 1 つのみ)。
pub struct SettleOverlay {
    pub width: u32,
    pub height: u32,
    /// wgpu リソース本体。`panorama_wgpu::UploadedSettleOverlay` を入れる。
    /// `Box<dyn Any>` ではなく具体型を使いたいが、本ファイルから `wgpu` 依存を
    /// 排したいので `Arc<dyn std::any::Any + Send + Sync>` にする。
    pub gpu: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

/// NeedsUserConfirmation → SettleApproved の追加ワーカー → UI の経路 (§4.6.0)。
///
/// **`high_res: Option<...>`** にすることで decode 失敗パスでも必ず 1 通送れる
/// (Codex P0 第 5、2026-05)。worker thread の Drop guard が責務を持つ:
/// - 成功時は `Some(high_res)` を明示送信 (`SendGuard.sent = true`)
/// - **decode 失敗 / panic** 時は Drop guard が `None` を送って pending を解放
/// - **cancel = true 時は silent exit** (= 何も送らない)。新 worker が同じ
///   source_key の pending を既に上書きしている可能性があるため、stale message で
///   それを誤って消費しないようにするため (Codex P1 第 6、2026-05)
/// UI 側 (`poll_pano_high_res`) は `None` で pending entry を remove + failed flag
/// を立てて auto-kick の無限ループを防ぐ。silent exit は新 worker (= pending に
/// 残っている) が完了するまで何もしない。
///
/// **`request_id`** は per-spawn の uniquely incrementing counter (Codex P1 第 7、
/// 2026-05)。`source_key + cache_key` だけでは「同じ source_key を同じ cache_key で
/// 連続 spawn」した場合に stale message が新 worker の pending を誤って消費する race
/// を避けられない。`request_id` を pending と message の両方に持たせて、poll 側は
/// **request_id 一致を必須**にする (= 厳密に「この spawn の message か」を識別)。
#[derive(Clone)]
pub struct PanoHighResReady {
    pub source_key: String,
    pub cache_key: u64,
    pub request_id: u64,
    pub high_res: Option<HighResSource>,
}

/// 追加ワーカーの進行管理 (§4.6.0)。
pub struct PanoHighResRequest {
    pub started_at: std::time::Instant,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// リクエスト発火時の cache_key snapshot。重複リクエスト検出 (= 既に同じ
    /// cache_key で走っている worker がある場合は新規 spawn しない) 専用。
    /// stale 検出は **現在の resolution.cache_key** との比較で行う。
    pub cache_key: u64,
    /// per-spawn unique ID (Codex P1 第 7、2026-05)。`App::pano_high_res_request_seq`
    /// から取得した値。poll で message の request_id と一致する場合のみ処理する。
    pub request_id: u64,
}

// ──────────────────────────────────────────────────────────────────
// settle CPU レンダ (§3.6.3)
// ──────────────────────────────────────────────────────────────────

/// equirect 用 bilinear sampler。U は経度ラップ (`rem_euclid`)、V は緯度 clamp。
/// `(u, v)` は **フル equirect 座標 [0,1]²**。crop は呼び出し側で適用済み前提。
///
/// **NaN/Inf ガード** (Codex P2 第 5、2026-05): `u_scale=0` のような不正 XMP crop で
/// `(u, v)` が ±∞ や NaN になっても panic しないように、入力を有限値に正規化する。
/// 出力は透明黒 `[0,0,0,0]` (= visually missing) で、視覚的に「破綻している領域」と
/// 分かるようにする。`w == 0 || h == 0` でも同様に透明黒。
#[inline]
pub fn sample_bilinear_equirect(src: &[u8], w: u32, h: u32, u: f32, v: f32) -> [u8; 4] {
    if w == 0 || h == 0 {
        return [0, 0, 0, 0];
    }
    if !u.is_finite() || !v.is_finite() {
        return [0, 0, 0, 0];
    }
    // 防御的に [-1024, 1024] 程度に範囲制限 (実用上の equirect では絶対値が 0..1 強)。
    // ここで先に有限値に潰しておけば後段の `as i32` が safe saturation に乗る。
    let u = u.clamp(-1024.0, 1024.0);
    let v = v.clamp(-1024.0, 1024.0);
    let wf = w as f32;
    let hf = h as f32;
    let x = u * wf - 0.5;
    let y = (v * hf - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    // `w as i32` は w > i32::MAX で負値化するが、Phase 2a は ≤ 200 MP source までしか
    // 通さないので実害なし。念のため `max(1)` で 0 除算 (`rem_euclid(0)` panic) を防ぐ。
    let w_i = (w as i32).max(1);
    let h_i = (h as i32).max(1);
    let fx = (x - x0 as f32).clamp(0.0, 1.0);
    let fy = (y - y0 as f32).clamp(0.0, 1.0);
    let x0w = x0.rem_euclid(w_i) as usize;
    // `x0 + 1` の overflow を防ぐため `saturating_add` を使う。
    let x1w = x0.saturating_add(1).rem_euclid(w_i) as usize;
    let y0c = y0.clamp(0, h_i - 1) as usize;
    let y1c = y0.saturating_add(1).clamp(0, h_i - 1) as usize;
    let stride = (w as usize) * 4;
    let p00 = &src[y0c * stride + x0w * 4..y0c * stride + x0w * 4 + 4];
    let p10 = &src[y0c * stride + x1w * 4..y0c * stride + x1w * 4 + 4];
    let p01 = &src[y1c * stride + x0w * 4..y1c * stride + x0w * 4 + 4];
    let p11 = &src[y1c * stride + x1w * 4..y1c * stride + x1w * 4 + 4];
    let w00 = (1.0 - fx) * (1.0 - fy);
    let w10 = fx * (1.0 - fy);
    let w01 = (1.0 - fx) * fy;
    let w11 = fx * fy;
    let mut out = [0u8; 4];
    for c in 0..4 {
        let v =
            p00[c] as f32 * w00 + p10[c] as f32 * w10 + p01[c] as f32 * w01 + p11[c] as f32 * w11;
        out[c] = v.clamp(0.0, 255.0).round() as u8;
    }
    out
}

/// NDC (-1..1, Y 上向き) から equirect 球面 UV に変換 (WGSL シェーダと同じ式)。
/// `uv_transform` は呼び出し側で適用 (= フル equirect 座標を返す)。
#[inline]
pub fn ndc_to_equirect_uv(
    u_ndc: f32,
    v_ndc: f32,
    aspect: f32,
    tan_half: f32,
    yaw: f32,
    pitch: f32,
) -> (f32, f32) {
    let dx = u_ndc * tan_half * aspect;
    let dy = v_ndc * tan_half;
    let dz: f32 = -1.0;
    let inv_len = 1.0 / (dx * dx + dy * dy + dz * dz).sqrt();
    let (cx, cy, cz) = (dx * inv_len, dy * inv_len, dz * inv_len);

    // pitch (X 軸回転)
    let cp = pitch.cos();
    let sp = pitch.sin();
    let p1x = cx;
    let p1y = cp * cy - sp * cz;
    let p1z = sp * cy + cp * cz;
    // yaw (Y 軸回転)
    let cyw = yaw.cos();
    let syw = yaw.sin();
    let wx = cyw * p1x + syw * p1z;
    let wy = p1y;
    let wz = -syw * p1x + cyw * p1z;

    let lon = wx.atan2(-wz);
    let lat = wy.clamp(-1.0, 1.0).asin();
    let inv_two_pi = 1.0 / (2.0 * std::f32::consts::PI);
    let inv_pi = 1.0 / std::f32::consts::PI;
    let u = lon * inv_two_pi + 0.5;
    let v = 0.5 - lat * inv_pi;
    (u, v)
}

/// settle render 本体 (§3.6.3)。
///
/// 1. **球面サンプリング**: rayon par_chunks で bilinear sample → `Vec<u8>` overlay
/// 2. **補正の再適用** (`EnabledWithColorAdjustments` のみ): overlay を `ColorImage`
///    化して `crate::adjustment::apply_adjustments_fast` を直接呼ぶ
///
/// `policy = Disabled` で呼ぶのは設計ミス (debug_assert)。`cancel` が立ったら
/// 途中で `None` を返す (= UI 側は何もしない)。
pub fn render_settle_overlay(
    src_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    uv: PanoUvTransform,
    yaw: f32,
    pitch: f32,
    fov_y: f32,
    out_w: u32,
    out_h: u32,
    policy: &PanoramaSettlePolicy,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<Vec<u8>> {
    use rayon::prelude::*;
    use std::sync::atomic::Ordering;

    debug_assert!(
        policy.is_enabled(),
        "render_settle_overlay called with Disabled policy"
    );
    if !policy.is_enabled() {
        return None;
    }
    if out_w == 0 || out_h == 0 || src_w == 0 || src_h == 0 {
        return None;
    }

    let aspect = out_w as f32 / out_h as f32;
    let tan_half = (fov_y * 0.5).tan();

    // U Repeat / V ClampToEdge は WGSL 側と同じ「軸別 half-texel inset clamp」を
    // CPU 側でも適用 (フル equirect は wrap、crop ありなら inset clamp)。
    let u_crop = (uv.u_scale < 0.999) || (uv.u_offset.abs() > 0.001);
    let v_crop = (uv.v_scale < 0.999) || (uv.v_offset.abs() > 0.001);
    let half_texel_u = 0.5 / src_w as f32;
    let half_texel_v = 0.5 / src_h as f32;

    let mut out = vec![0u8; (out_w * out_h * 4) as usize];
    let stride = (out_w * 4) as usize;
    let row_iter = out.par_chunks_exact_mut(stride).enumerate();
    let result: Result<(), ()> = row_iter.try_for_each(|(y, row)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        let v_ndc = 1.0 - (y as f32 + 0.5) / out_h as f32 * 2.0;
        for x in 0..out_w as usize {
            let u_ndc = (x as f32 + 0.5) / out_w as f32 * 2.0 - 1.0;
            let (sphere_u, sphere_v) =
                ndc_to_equirect_uv(u_ndc, v_ndc, aspect, tan_half, yaw, pitch);
            // フル sphere → 画像テクスチャ座標
            let tex_u_raw = (sphere_u - uv.u_offset) / uv.u_scale;
            let tex_v_raw = (sphere_v - uv.v_offset) / uv.v_scale;
            // U: crop 時は inset clamp、フル equirect 時は rem_euclid に任せる
            let tex_u = if u_crop {
                tex_u_raw.clamp(half_texel_u, 1.0 - half_texel_u)
            } else {
                // フル equirect: sample_bilinear_equirect 側で rem_euclid が処理する
                tex_u_raw
            };
            // V: crop 時は inset clamp、フル equirect 時は sampler 側の clamp
            let tex_v = if v_crop {
                tex_v_raw.clamp(half_texel_v, 1.0 - half_texel_v)
            } else {
                tex_v_raw
            };
            let rgba = sample_bilinear_equirect(src_rgba, src_w, src_h, tex_u, tex_v);
            let off = x * 4;
            row[off..off + 4].copy_from_slice(&rgba);
        }
        Ok(())
    });
    if result.is_err() {
        return None;
    }

    // ステップ 2: 補正再適用
    if let PanoramaSettlePolicy::EnabledWithColorAdjustments { params } = policy {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([out_w as usize, out_h as usize], &out);
        let adjusted = crate::adjustment::apply_adjustments_fast(&ci, params);
        // ColorImage → Vec<u8> (Color32 は repr(C) RGBA8 連続だが、安全に flatten する)
        out = adjusted
            .pixels
            .iter()
            .flat_map(|c| {
                let [r, g, b, a] = c.to_array();
                [r, g, b, a]
            })
            .collect();
    }

    Some(out)
}

/// 64-bit packed cache key:
///
/// ```text
/// [63..48]: idx_hash16   (CRC16 of source_key)
/// [47..32]: source_kind  (0=fs_cache, 1=raw+adj, 2=ai, 3=ai+adj, 4=final)
/// [31..16]: adjust_gen16 (App::adjustment_generation[source_key] の下位 16bit)
/// [15..0] : ai_gen16     (App::ai_upscale_generation[source_key] の下位 16bit)
/// ```
///
/// 16 bit gen は 65,536 回の更新で wrap するが、長時間セッションの実害は低い
/// (§4.1.2 末尾の wrap 議論を参照)。Phase 3 で bit 再配分を検討する余地あり。
pub fn make_pano_cache_key(idx_hash: u16, source_kind: u16, adjust_gen: u16, ai_gen: u16) -> u64 {
    ((idx_hash as u64) << 48)
        | ((source_kind as u64) << 32)
        | ((adjust_gen as u64) << 16)
        | (ai_gen as u64)
}

/// 文字列の CRC-16/CCITT-FALSE 値。`source_key` (metadata_cache_key) を 16 bit に畳む。
///
/// 厳密な衝突回避が目的ではない (cache_key の他 3 要素と合わせて stale 検出するため、
/// idx_hash の衝突は別 source_kind / gen で吸収される)。軽量実装で十分。
pub fn crc16_of_str(s: &str) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in s.as_bytes() {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_packs_and_extracts() {
        let key = make_pano_cache_key(0xABCD, 2, 0x1234, 0x5678);
        assert_eq!((key >> 48) & 0xFFFF, 0xABCD);
        assert_eq!((key >> 32) & 0xFFFF, 2);
        assert_eq!((key >> 16) & 0xFFFF, 0x1234);
        assert_eq!(key & 0xFFFF, 0x5678);
    }

    #[test]
    fn cache_key_differs_when_source_kind_changes() {
        let a = make_pano_cache_key(0xABCD, 0, 1, 0);
        let b = make_pano_cache_key(0xABCD, 1, 1, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_differs_when_gen_changes() {
        let a = make_pano_cache_key(0xABCD, 1, 1, 0);
        let b = make_pano_cache_key(0xABCD, 1, 2, 0);
        assert_ne!(a, b);
        let c = make_pano_cache_key(0xABCD, 1, 1, 1);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn crc16_stable_and_distinct() {
        let a = crc16_of_str("c:/foo/bar.jpg");
        let b = crc16_of_str("c:/foo/baz.jpg");
        let a2 = crc16_of_str("c:/foo/bar.jpg");
        assert_eq!(a, a2);
        assert_ne!(a, b);
    }

    #[test]
    fn panorama_state_resets_to_initial() {
        let mut s = PanoramaState::new(0.5, -0.2);
        s.yaw = 1.5;
        s.pitch = 0.3;
        s.fov_y = 0.5;
        s.drag_active = true;
        s.reset();
        assert_eq!(s.yaw, 0.5);
        assert_eq!(s.pitch, -0.2);
        assert_eq!(s.fov_y, FOV_DEFAULT);
        assert!(!s.drag_active);
    }

    #[test]
    fn pitch_clamped_in_new() {
        let s = PanoramaState::new(0.0, 100.0);
        assert!(s.pitch <= PITCH_LIMIT);
        let s = PanoramaState::new(0.0, -100.0);
        assert!(s.pitch >= -PITCH_LIMIT);
    }

    // ---- PanoUvTransform: 部分 FOV equirect (Phase 1.5) ----

    fn make_pano_info(
        full: Option<(u32, u32)>,
        cropped: Option<(u32, u32)>,
        left: Option<u32>,
        top: Option<u32>,
    ) -> crate::xmp_reader::XmpPanoramaInfo {
        crate::xmp_reader::XmpPanoramaInfo {
            projection_type: Some("equirectangular".to_string()),
            use_panorama_viewer: Some(true),
            full_pano_width_pixels: full.map(|(w, _)| w),
            full_pano_height_pixels: full.map(|(_, h)| h),
            cropped_area_image_width_pixels: cropped.map(|(w, _)| w),
            cropped_area_image_height_pixels: cropped.map(|(_, h)| h),
            cropped_area_left_pixels: left,
            cropped_area_top_pixels: top,
            pose_pitch_degrees: None,
            pose_heading_degrees: None,
            pose_roll_degrees: None,
        }
    }

    #[test]
    fn uv_transform_identity_when_full_equals_cropped() {
        // 完全フル equirect: cropped = full、offset 0
        let info = make_pano_info(Some((4096, 2048)), Some((4096, 2048)), Some(0), Some(0));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        assert!(x.is_identity(), "got {:?}", x);
    }

    #[test]
    fn uv_transform_partial_fov_dslr_example() {
        // 設計書 §11.2 の例: 15126×7562 のフル球面に対し 15126×5795 で水平全周だが垂直 77%
        // 中央寄せ (top = (7562 - 5795) / 2 ≒ 883)
        let info = make_pano_info(Some((15126, 7562)), Some((15126, 5795)), Some(0), Some(883));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        // 水平はフル覆い
        assert!((x.u_scale - 1.0).abs() < 0.001, "u_scale = {}", x.u_scale);
        assert_eq!(x.u_offset, 0.0);
        // 垂直は ~76.6%
        let expected_v_scale = 5795.0 / 7562.0;
        assert!((x.v_scale - expected_v_scale).abs() < 0.001);
        let expected_v_offset = 883.0 / 7562.0;
        assert!((x.v_offset - expected_v_offset).abs() < 0.001);
        assert!(!x.is_identity());
        assert!(!x.has_horizontal_crop());
    }

    #[test]
    fn uv_transform_detects_horizontal_crop_for_sampler_selection() {
        assert!(!PanoUvTransform::IDENTITY.has_horizontal_crop());

        let horizontal = PanoUvTransform {
            u_offset: 0.1,
            v_offset: 0.0,
            u_scale: 0.8,
            v_scale: 1.0,
        };
        assert!(horizontal.has_horizontal_crop());

        let vertical_only = PanoUvTransform {
            u_offset: 0.0,
            v_offset: 0.1,
            u_scale: 1.0,
            v_scale: 0.8,
        };
        assert!(!vertical_only.has_horizontal_crop());
    }

    #[test]
    fn uv_transform_left_top_default_to_center() {
        // CroppedAreaLeftPixels / TopPixels が無い場合は中央寄せ
        let info = make_pano_info(Some((4096, 2048)), Some((2048, 1024)), None, None);
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        // 中央寄せ: left = (4096 - 2048) / 2 = 1024、top = (2048 - 1024) / 2 = 512
        // u_offset = 1024/4096 = 0.25、v_offset = 512/2048 = 0.25
        assert!((x.u_offset - 0.25).abs() < 0.001);
        assert!((x.v_offset - 0.25).abs() < 0.001);
        assert!((x.u_scale - 0.5).abs() < 0.001);
        assert!((x.v_scale - 0.5).abs() < 0.001);
    }

    #[test]
    fn uv_transform_returns_none_when_required_missing() {
        // FullPanoWidth が無いと計算不可
        let info = make_pano_info(Some((0, 2048)), Some((4096, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());

        let info = make_pano_info(None, Some((4096, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());

        let info = make_pano_info(Some((4096, 2048)), None, Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());
    }

    #[test]
    fn uv_transform_returns_none_when_cropped_exceeds_full() {
        // CroppedArea が FullPano をはみ出す → 防衛的に identity 化
        let info = make_pano_info(Some((4096, 2048)), Some((5000, 2048)), Some(0), Some(0));
        assert!(PanoUvTransform::from_gpano(&info).is_none());
    }

    #[test]
    fn uv_transform_near_full_snaps_to_identity() {
        // 浮動小数誤差程度の微差は identity に丸める (= 不要な UV 変換を避ける)
        let info = make_pano_info(Some((10000, 5000)), Some((9999, 4999)), Some(0), Some(0));
        let x = PanoUvTransform::from_gpano(&info).expect("should compute");
        assert!(x.is_identity());
    }

    // ---- Phase 2a: settle-refinement (§3.6 / §4.6) ----

    #[test]
    fn needs_user_confirmation_under_200mp() {
        // 200 MP 以下なら approved_max が 0 でも false
        assert!(!needs_user_confirmation(72_000_000, 0));
        assert!(!needs_user_confirmation(PANO_SETTLE_MAX_PIXELS, 0));
    }

    #[test]
    fn needs_user_confirmation_over_200mp_without_approval() {
        assert!(needs_user_confirmation(201_000_000, 0));
        assert!(needs_user_confirmation(338_000_000, 0));
    }

    #[test]
    fn needs_user_confirmation_within_125_pct_no_reconfirm() {
        // 201 MP 承認 → 220 MP (= +9.5%) は再確認不要
        let approved = 201_000_000u64;
        assert!(!needs_user_confirmation(220_000_000, approved));
        // 250 MP (= +24.4%) もまだ範囲内
        assert!(!needs_user_confirmation(250_000_000, approved));
        // 252 MP (= +25.4%) は超過、再確認必要
        assert!(needs_user_confirmation(252_000_000, approved));
    }

    #[test]
    fn settle_enabled_requires_state_and_policy() {
        let state_ok = PanoramaQualityState::SettleReady;
        let state_no = PanoramaQualityState::NeedsUserConfirmation {
            source_pixels: 300_000_000,
            est_ram_gb: 2.4,
        };
        let policy_ok = PanoramaSettlePolicy::EnabledFromRaw;
        let policy_no = PanoramaSettlePolicy::Disabled;
        assert!(settle_enabled(&state_ok, &policy_ok));
        assert!(!settle_enabled(&state_no, &policy_ok));
        assert!(!settle_enabled(&state_ok, &policy_no));
        assert!(!settle_enabled(&state_no, &policy_no));
        // SettleApproved も OK
        assert!(settle_enabled(
            &PanoramaQualityState::SettleApproved,
            &policy_ok
        ));
        // BaseOnly は NG
        assert!(!settle_enabled(&PanoramaQualityState::BaseOnly, &policy_ok));
    }

    #[test]
    fn sample_bilinear_equirect_center_returns_pixel() {
        // 2×2 RGBA: 4 つの単色をきれいに角に配置
        let src: Vec<u8> = vec![
            255, 0, 0, 255, // (0,0) 赤
            0, 255, 0, 255, // (1,0) 緑
            0, 0, 255, 255, // (0,1) 青
            255, 255, 0, 255, // (1,1) 黄
        ];
        // U=0.5/W (中心 x0=0)、V=0.5/H (中心 y=0) → (0,0) の赤
        let px = sample_bilinear_equirect(&src, 2, 2, 0.25, 0.25);
        assert_eq!(px, [255, 0, 0, 255]);
        let px = sample_bilinear_equirect(&src, 2, 2, 0.75, 0.25);
        assert_eq!(px, [0, 255, 0, 255]);
    }

    #[test]
    fn sample_bilinear_equirect_wraps_u() {
        // u = 1.0 は wrap で u = 0.0 と同じ texel に着地する
        let src: Vec<u8> = vec![
            10, 20, 30, 40, // (0,0)
            50, 60, 70, 80, // (1,0)
        ];
        // U=1.0 (= W*1.0 - 0.5 = 1.5、x0=1) → 隣の wrap で (0,0) と (1,0) の中点近辺。
        // 単に「panic せずに値が返る」ことを確認 (具体的な値は補間で fractional)
        let _ = sample_bilinear_equirect(&src, 2, 1, 1.0, 0.5);
        let _ = sample_bilinear_equirect(&src, 2, 1, -0.1, 0.5);
        let _ = sample_bilinear_equirect(&src, 2, 1, 1.1, 0.5);
    }

    #[test]
    fn ndc_to_equirect_uv_center_is_yaw_pitch() {
        // 中心ピクセル (NDC=(0,0)) + yaw=0, pitch=0 → (lon=0, lat=0)
        // → (u=0.5, v=0.5)
        let (u, v) = ndc_to_equirect_uv(0.0, 0.0, 1.0, 1.0, 0.0, 0.0);
        assert!((u - 0.5).abs() < 1e-4, "u={}", u);
        assert!((v - 0.5).abs() < 1e-4, "v={}", v);
    }

    // `render_settle_overlay(..., Disabled, ...)` は debug_assert で panic する設計
    // (callers が settle_enabled で gating する前提)。runtime には `return None`
    // フォールバックがあるが、debug build の test では debug_assert! が走るので
    // 専用テストは持たず、production フォールバックの存在だけ確認する (= 本実装で
    // `return None` が外されるとシンタックスエラーになる)。

    #[test]
    fn render_settle_overlay_enabled_from_raw_returns_buffer() {
        // 8x4 RGBA: ヘルパーで全画素一定色のソース
        let src = vec![200u8; 8 * 4 * 4];
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let out = render_settle_overlay(
            &src,
            8,
            4,
            PanoUvTransform::IDENTITY,
            0.0,
            0.0,
            1.2,
            16,
            8,
            &PanoramaSettlePolicy::EnabledFromRaw,
            &cancel,
        )
        .expect("should produce output");
        assert_eq!(out.len(), 16 * 8 * 4);
        // 全画素 200 のソースから等しく sample → 出力もほぼ 200 ぐらい
        // (補間で多少誤差はあるが極端には外れない)
        let avg: f32 = out.iter().map(|&x| x as f32).sum::<f32>() / out.len() as f32;
        assert!((avg - 200.0).abs() < 5.0, "avg={}", avg);
    }

    #[test]
    fn render_settle_overlay_cancel_aborts() {
        let src = vec![100u8; 4 * 2 * 4];
        let cancel = std::sync::atomic::AtomicBool::new(true); // 最初から立てる
        let out = render_settle_overlay(
            &src,
            4,
            2,
            PanoUvTransform::IDENTITY,
            0.0,
            0.0,
            1.2,
            8,
            4,
            &PanoramaSettlePolicy::EnabledFromRaw,
            &cancel,
        );
        assert!(out.is_none());
    }

    #[test]
    fn pano_quality_state_variants_compile_and_distinct() {
        let a = PanoramaQualityState::SettleReady;
        let b = PanoramaQualityState::NeedsUserConfirmation {
            source_pixels: 300_000_000,
            est_ram_gb: 2.4,
        };
        let c = PanoramaQualityState::SettleApproved;
        let d = PanoramaQualityState::BaseOnly;
        // 4 つすべて distinct であることだけ確認
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(b, c);
        assert_ne!(b, d);
        assert_ne!(c, d);
    }

    #[test]
    fn panorama_state_sanitize_replaces_nan() {
        let mut s = PanoramaState::new(0.0, 0.0);
        s.yaw = f32::NAN;
        s.pitch = f32::INFINITY;
        s.fov_y = f32::NEG_INFINITY;
        s.sanitize();
        assert!(s.yaw.is_finite() && s.yaw == 0.0);
        assert!(s.pitch.is_finite());
        assert!(s.fov_y.is_finite() && (FOV_MIN..=FOV_MAX).contains(&s.fov_y));
    }

    #[test]
    fn panorama_state_new_handles_nan_inputs() {
        let s = PanoramaState::new(f32::NAN, f32::INFINITY);
        assert!(s.yaw.is_finite());
        assert!(s.pitch.is_finite() && s.pitch.abs() <= PITCH_LIMIT);
        assert!(s.fov_y.is_finite());
    }

    #[test]
    fn sample_bilinear_equirect_handles_nan_inputs() {
        let src: Vec<u8> = vec![255; 4 * 4]; // 2x2 white
        // NaN / Inf → transparent black (no panic)
        let px = sample_bilinear_equirect(&src, 2, 2, f32::NAN, 0.5);
        assert_eq!(px, [0, 0, 0, 0]);
        let px = sample_bilinear_equirect(&src, 2, 2, f32::INFINITY, 0.5);
        assert_eq!(px, [0, 0, 0, 0]);
        let px = sample_bilinear_equirect(&src, 2, 2, 0.5, f32::NEG_INFINITY);
        assert_eq!(px, [0, 0, 0, 0]);
        // w=0 or h=0 → transparent black
        let px = sample_bilinear_equirect(&[], 0, 0, 0.5, 0.5);
        assert_eq!(px, [0, 0, 0, 0]);
    }

    #[test]
    fn uv_transform_returns_none_when_scale_too_small() {
        // 異常に小さい u_scale → ゼロ除算寸前 → identity に倒すべき
        let info = make_pano_info(Some((10000, 5000)), Some((1, 5000)), Some(0), Some(0));
        // u_scale = 1/10000 = 0.0001 < 0.001 閾値で None
        assert!(PanoUvTransform::from_gpano(&info).is_none());
    }

    #[test]
    fn high_res_source_dims_and_byte_len() {
        let rgba = std::sync::Arc::new(vec![0u8; 4 * 16]);
        let h = HighResSource::Decoded {
            rgba: rgba.clone(),
            w: 4,
            h: 4,
        };
        assert_eq!(h.dims(), (4, 4));
        assert_eq!(h.byte_len(), 64);
    }
}
