use super::*;

impl App {
    // -------------------------------------------------------------------
    // サムネイル画質設定ダイアログ (A/B 比較)
    // -------------------------------------------------------------------
    pub(crate) fn open_thumb_quality_dialog(&mut self, _ctx: &egui::Context) {
        // 既存状態をリセット
        self.tq.sample = None;
        self.tq.sample_display_name = None;
        self.tq.sample_original_size = 0;
        self.tq.load_failed = false;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.a_bytes = 0;
        self.tq.b_bytes = 0;
        self.tq.load_pending = None;

        // A/B スライダー初期値はダイアログを開いた瞬間に確定しておく
        // (decode 待ち中にユーザがスライダーを触っても同期的に反映できるように)
        self.tq.a_size = self.settings.thumb_px;
        self.tq.a_quality = self.settings.thumb_quality;
        self.tq.b_size = self.settings.thumb_px;
        self.tq.b_quality = (self.settings.thumb_quality as u32 + 10).min(95) as u8;

        // 最後に選択した有効な画像サンプルを取得
        let Some(source) = self.last_selected_thumb_sample.clone() else {
            // None のままダイアログを開く (メッセージだけ出る)
            self.tq.show = true;
            return;
        };

        // decode を worker に回す。20MP 超や巨大 RAW の image::open、ZIP entry 読込は UI を
        // 数百ms〜秒単位止めるため同期実行しない。ダイアログは即座に「読み込み中」で開く。
        let (tx, rx) = mpsc::channel();
        let display_name = source.display_name();
        std::thread::Builder::new()
            .name("thumb-quality-sample-decode".into())
            .spawn(move || {
                let result = match source {
                    ThumbSampleSource::File(path) => image::open(&path).ok().map(|img| {
                        let orig = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        (img, orig)
                    }),
                    ThumbSampleSource::ZipEntry {
                        zip_path,
                        entry_name,
                    } => crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                        .ok()
                        .and_then(|bytes| {
                            let orig = bytes.len() as u64;
                            image::load_from_memory(&bytes).ok().map(|img| (img, orig))
                        }),
                };
                let _ = tx.send(result);
            })
            .ok();
        self.tq.load_pending = Some(ThumbQualityLoadPending { display_name, rx });
        self.tq.show = true;
    }

    /// worker からの decode 結果を拾い、サンプル確定時に A/B プレビューを初期生成する。
    pub(crate) fn poll_thumb_quality_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.tq.load_pending.as_ref() else {
            return;
        };
        let msg = match pending.rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => {
                if self.tq.show {
                    // decode 待ちの間はプログレス表示更新のために再描画要求
                    ctx.request_repaint();
                }
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.tq.load_pending = None;
                self.tq.load_failed = true;
                return;
            }
        };
        let display_name = pending.display_name.clone();
        self.tq.load_pending = None;
        let Some((img, orig_size)) = msg else {
            self.tq.load_failed = true;
            return;
        };
        self.tq.sample = Some(Arc::new(img));
        self.tq.sample_display_name = Some(display_name);
        self.tq.sample_original_size = orig_size;
        self.reencode_tq_panel(true);
        self.reencode_tq_panel(false);
    }

    /// A/B プレビューの再エンコードを worker に依頼する。
    /// `encode_thumb_webp` + resize + webp + `decode_thumb_to_color_image` は 20MP 級で
    /// 合計 100-300ms かかる。スライダー操作で連射される場合、前回 pending は cancel して
    /// 最新だけ texture に反映する。
    pub(crate) fn reencode_tq_panel(&mut self, is_a: bool) {
        let Some(sample) = self.tq.sample.clone() else {
            return;
        };
        let (size, quality) = if is_a {
            (self.tq.a_size, self.tq.a_quality)
        } else {
            (self.tq.b_size, self.tq.b_quality)
        };

        // 前回の encode pending は cancel。まだ走っていれば send 前に早期 return してくれる。
        if let Some(prev) = if is_a {
            self.tq.a_encode_pending.as_ref()
        } else {
            self.tq.b_encode_pending.as_ref()
        } {
            prev.cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let cancel_worker = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("thumb-quality-encode".into())
            .spawn(move || {
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let encoded = crate::catalog::encode_thumb_webp(&sample, size, quality as f32);
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let result = encoded.and_then(|(data, _w, _h)| {
                    let bytes = data.len();
                    crate::catalog::decode_thumb_to_color_image(&data)
                        .map(|color_image| TqEncodeResult { bytes, color_image })
                });
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
            })
            .ok();

        let pending = TqEncodePending { cancel, rx };
        if is_a {
            self.tq.a_encode_pending = Some(pending);
        } else {
            self.tq.b_encode_pending = Some(pending);
        }
    }

    /// A/B encode worker の完了を拾う。`load_texture` だけ UI スレッドで実行する。
    /// 未完了の pending が残っている間は再描画要求する。
    pub(crate) fn poll_tq_encode_pending(&mut self, ctx: &egui::Context) {
        // A 側
        let mut a_repaint_needed = false;
        if let Some(pending) = self.tq.a_encode_pending.as_ref() {
            match pending.rx.try_recv() {
                Ok(Some(result)) => {
                    let tex = ctx.load_texture(
                        "tq_preview_a",
                        result.color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.tq.a_bytes = result.bytes;
                    self.tq.a_texture = Some(tex);
                    self.tq.a_encode_pending = None;
                }
                Ok(None) => {
                    // encode 失敗。旧テクスチャは残す (0 にリセットだけ)。
                    self.tq.a_bytes = 0;
                    self.tq.a_encode_pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    a_repaint_needed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tq.a_encode_pending = None;
                }
            }
        }
        // B 側
        let mut b_repaint_needed = false;
        if let Some(pending) = self.tq.b_encode_pending.as_ref() {
            match pending.rx.try_recv() {
                Ok(Some(result)) => {
                    let tex = ctx.load_texture(
                        "tq_preview_b",
                        result.color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.tq.b_bytes = result.bytes;
                    self.tq.b_texture = Some(tex);
                    self.tq.b_encode_pending = None;
                }
                Ok(None) => {
                    self.tq.b_bytes = 0;
                    self.tq.b_encode_pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    b_repaint_needed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tq.b_encode_pending = None;
                }
            }
        }
        if a_repaint_needed || b_repaint_needed {
            ctx.request_repaint();
        }
    }

    pub(crate) fn close_thumb_quality_dialog(&mut self) {
        self.tq.show = false;
        self.tq.sample = None;
        self.tq.sample_display_name = None;
        self.tq.load_failed = false;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.fullscreen = false;
        self.tq.load_pending = None;
        // encode worker 停止要求。pending 構造体が drop されると tx も落ちるので追加送信は無視される。
        if let Some(p) = self.tq.a_encode_pending.as_ref() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(p) = self.tq.b_encode_pending.as_ref() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        self.tq.a_encode_pending = None;
        self.tq.b_encode_pending = None;
    }

    // -------------------------------------------------------------------
    // キャッシュ作成（バックグラウンドで選択フォルダ以下を再帰処理）
    // -------------------------------------------------------------------
    pub(crate) fn start_cache_creation(&mut self) {
        // 選択されたお気に入りを集める（名前とパスのペア）
        let targets: Vec<(String, PathBuf)> = self
            .settings
            .favorites
            .iter()
            .zip(self.cc.checked.iter())
            .filter_map(|(f, &c)| {
                if c {
                    Some((f.name.clone(), f.path.clone()))
                } else {
                    None
                }
            })
            .collect();

        if targets.is_empty() {
            return;
        }

        // 状態リセット
        self.cc.running = true;
        self.cc.counting.store(true, Ordering::Relaxed);
        self.cc.total.store(0, Ordering::Relaxed);
        self.cc.done.store(0, Ordering::Relaxed);
        self.cc.finished.store(false, Ordering::Relaxed);
        self.cc.result = None;
        *self.cc.current.lock().unwrap() = String::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cc.cancel = Arc::clone(&cancel);

        // ベースラインは worker 側で取得する。`cache_stats` は read_dir + metadata の全走査で
        // キャッシュフォルダが大きいと UI スレッドで数百 ms ブロックしうるため、開始ボタン直後は
        // 0 のまま返して worker 内で書き換える (docs/ui-responsiveness.md §4 チェックリスト)。
        self.cc.cache_size.store(0, Ordering::Relaxed);

        // atomic クローン
        let counting = Arc::clone(&self.cc.counting);
        let total = Arc::clone(&self.cc.total);
        let done = Arc::clone(&self.cc.done);
        let size_atomic = Arc::clone(&self.cc.cache_size);
        let finished = Arc::clone(&self.cc.finished);
        let current = Arc::clone(&self.cc.current);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let threads = self.settings.parallelism.thread_count();
        let batch_zip = self.settings.batch_cache_zip_contents;
        let batch_pdf = self.settings.batch_cache_pdf_contents;

        std::thread::spawn(move || {
            // baseline: worker 冒頭で取得 (UI スレッドブロッキング回避)
            let cache_dir = crate::catalog::default_cache_dir();
            let (_, baseline) = crate::catalog::cache_stats(&cache_dir);
            size_atomic.store(baseline, Ordering::Relaxed);

            // Pass 1: カウント
            let mut all_folders: Vec<PathBuf> = Vec::new();
            for (_, path) in &targets {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                walk_dirs_recursive(path, &mut all_folders, &cancel);
            }
            total.store(all_folders.len(), Ordering::Relaxed);
            counting.store(false, Ordering::Relaxed);

            if cancel.load(Ordering::Relaxed) {
                finished.store(true, Ordering::Relaxed);
                return;
            }

            // 処理用 rayon プール
            let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                Ok(p) => p,
                Err(_) => {
                    finished.store(true, Ordering::Relaxed);
                    return;
                }
            };

            // Pass 2: フォルダを順次処理、内部画像は並列デコード
            for folder in &all_folders {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                // お気に入り名 > 相対パス の形式で表示用文字列を生成
                let folder_display = targets
                    .iter()
                    .find(|(_, base)| folder.starts_with(base))
                    .map(|(name, base)| match folder.strip_prefix(base) {
                        Ok(rel) if rel.as_os_str().is_empty() => name.clone(),
                        Ok(rel) => format!("{} > {}", name, rel.to_string_lossy()),
                        Err(_) => folder.to_string_lossy().to_string(),
                    })
                    .unwrap_or_else(|| folder.to_string_lossy().to_string());
                *current.lock().unwrap() = folder_display.clone();

                // ファイル列挙（単一フォルダ、再帰なし — 画像・ZIP・PDF を1パスで分類）
                let mut images: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut zip_files: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut pdf_files: Vec<(PathBuf, i64, i64)> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(folder) {
                    for entry in entries.flatten() {
                        // entry.file_type() は FindFirstFile/FindNextFile の戻りを再利用するので
                        // per-entry GetFileAttributes syscall を避けられる
                        // (docs/ui-responsiveness.md §4)。キャッシュ作成の大量フォルダ走査で効く。
                        let Ok(ft) = entry.file_type() else {
                            continue;
                        };
                        if !ft.is_file() {
                            continue;
                        }
                        let p = entry.path();
                        if is_apple_double(&p) {
                            continue;
                        }
                        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                            continue;
                        };
                        let ext_lower = ext.to_ascii_lowercase();
                        let meta = || {
                            let m = entry.metadata().ok()?;
                            let mtime = crate::ui_helpers::mtime_secs(&m);
                            let file_size = m.len() as i64;
                            Some((mtime, file_size))
                        };
                        if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
                            if let Some((mt, fs)) = meta() {
                                images.push((p, mt, fs));
                            }
                        } else if crate::folder_tree::is_zip_extension(&ext_lower) {
                            if let Some((mt, fs)) = meta() {
                                zip_files.push((p, mt, fs));
                            }
                        } else if ext_lower == "pdf" {
                            if let Some((mt, fs)) = meta() {
                                pdf_files.push((p, mt, fs));
                            }
                        }
                    }
                }

                if images.is_empty() && zip_files.is_empty() && pdf_files.is_empty() {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // カタログを開く（1フォルダ1DB）
                let Ok(catalog) = crate::catalog::CatalogDb::open(&cache_dir, folder) else {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let cache_map = catalog.load_all().unwrap_or_default();

                // ── 画像を並列でデコード + 保存 ──
                if !images.is_empty() {
                    pool.install(|| {
                        use rayon::prelude::*;
                        images.par_iter().for_each(|(path, mtime, file_size)| {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let filename = match path.file_name().and_then(|n| n.to_str()) {
                                Some(n) => n,
                                None => return,
                            };
                            if let Some(entry) = cache_map.get(filename) {
                                if entry.mtime == *mtime && entry.file_size == *file_size {
                                    return;
                                }
                            }
                            if let Some(bytes) = build_and_save_one(
                                path,
                                &catalog,
                                *mtime,
                                *file_size,
                                thumb_px,
                                thumb_quality,
                            ) {
                                size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                            }
                        });
                    });
                }

                // ── ZIP ファイルの中身をキャッシュ ──
                for (zip_path, zip_mtime, zip_file_size) in &zip_files {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let zip_fname = match zip_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let folder_key = format!("{}{}", CACHE_KEY_ZIP, zip_fname);

                    if batch_zip {
                        *current.lock().unwrap() = format!("{} > {}", folder_display, zip_fname);
                        let entries = match crate::zip_loader::enumerate_image_entries(zip_path) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };
                        let zip_catalog =
                            match crate::catalog::CatalogDb::open(&cache_dir, zip_path) {
                                Ok(c) => c,
                                Err(_) => continue,
                            };
                        let zip_cache_map = zip_catalog.load_all().unwrap_or_default();
                        let entry_count = entries.len();

                        // 先頭エントリの WebP を並列処理中にキャプチャ。
                        // DCT scale で縮小済みの場合に親フォルダ catalog でも
                        // **元寸法**を保存するため、source_dims override も同梱する。
                        let first_webp: Arc<
                            Mutex<Option<(image::DynamicImage, Option<(u32, u32)>, String)>>,
                        > = Arc::new(Mutex::new(None));

                        pool.install(|| {
                            use rayon::prelude::*;
                            entries.par_iter().enumerate().for_each(|(i, entry)| {
                                if cancel.load(Ordering::Relaxed) {
                                    return;
                                }
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})",
                                    folder_display,
                                    zip_fname,
                                    i + 1,
                                    entry_count
                                );
                                if let Some(existing) = zip_cache_map.get(&entry.entry_name) {
                                    if existing.mtime == entry.mtime
                                        && existing.file_size == entry.uncompressed_size as i64
                                    {
                                        return;
                                    }
                                }
                                let raw = match crate::zip_loader::read_entry_bytes(
                                    zip_path,
                                    &entry.entry_name,
                                ) {
                                    Ok(b) => b,
                                    Err(_) => return,
                                };
                                let orientation = read_exif_orientation_from_bytes(&raw);
                                // JPEG なら TurboJPEG DCT scale を試す
                                let (img, dct_stats): (
                                    Option<image::DynamicImage>,
                                    Option<ScaleStats>,
                                ) = if is_jpeg_entry(&entry.entry_name) {
                                    match decode_jpeg_turbo_scaled_from_bytes(&raw, thumb_px) {
                                        Ok((img, stats)) => (Some(img), Some(stats)),
                                        Err(DctDecodeError::TerminalRejection(msg)) => {
                                            crate::logger::log(format!(
                                                "cache_creator DCT terminal rejection ZIP {}/{}: {msg}",
                                                zip_fname, entry.entry_name
                                            ));
                                            return;
                                        }
                                        Err(DctDecodeError::Fallback(_)) => (None, None),
                                    }
                                } else {
                                    (None, None)
                                };
                                let img = match img.or_else(|| image::load_from_memory(&raw).ok()) {
                                    Some(i) => i,
                                    None => return,
                                };
                                let img = apply_orientation(img, orientation);
                                let source_dims =
                                    dct_stats.map(|s| s.source_dims_after_exif(orientation));
                                // 先頭エントリをキャプチャ（親フォルダ用サムネイル再利用）。
                                // source_dims も一緒にキャプチャして parent thumb save で再利用。
                                if i == 0 {
                                    *first_webp.lock().unwrap() = Some((
                                        img.clone(),
                                        source_dims,
                                        entry.entry_name.clone(),
                                    ));
                                }
                                if let Some(bytes) = encode_and_save_with_source_dims(
                                    &img,
                                    source_dims,
                                    &entry.entry_name,
                                    &zip_catalog,
                                    entry.mtime,
                                    entry.uncompressed_size as i64,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            });
                        });

                        // 先頭1枚を親フォルダの DB にも保存（フォルダ一覧用サムネイル）
                        if !cache_map.contains_key(&folder_key) {
                            let captured = first_webp.lock().unwrap().take();
                            if let Some((img, source_dims, _)) = captured {
                                if let Some(bytes) = encode_and_save_with_source_dims(
                                    &img,
                                    source_dims,
                                    &folder_key,
                                    &catalog,
                                    *zip_mtime,
                                    *zip_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    } else {
                        // 先頭1枚のみ（フォルダ一覧用サムネイル）
                        if cache_map.contains_key(&folder_key) {
                            continue;
                        }
                        if let Some(first_entry) =
                            crate::zip_loader::first_image_entry(zip_path, None)
                        {
                            if let Ok(raw) =
                                crate::zip_loader::read_entry_bytes(zip_path, &first_entry)
                            {
                                let orientation = read_exif_orientation_from_bytes(&raw);
                                // JPEG なら DCT scale → source_dims override で保存
                                let (img, source_dims): (
                                    Option<image::DynamicImage>,
                                    Option<(u32, u32)>,
                                ) = if is_jpeg_entry(&first_entry) {
                                    match decode_jpeg_turbo_scaled_from_bytes(&raw, thumb_px) {
                                        Ok((img, stats)) => (
                                            Some(img),
                                            Some(stats.source_dims_after_exif(orientation)),
                                        ),
                                        Err(DctDecodeError::TerminalRejection(msg)) => {
                                            crate::logger::log(format!(
                                                "cache_creator first-only DCT terminal rejection {zip_path:?}/{first_entry}: {msg}"
                                            ));
                                            continue;
                                        }
                                        Err(DctDecodeError::Fallback(_)) => (None, None),
                                    }
                                } else {
                                    (None, None)
                                };
                                let img = match img.or_else(|| image::load_from_memory(&raw).ok()) {
                                    Some(i) => i,
                                    None => continue,
                                };
                                let img = apply_orientation(img, orientation);
                                if let Some(bytes) = encode_and_save_with_source_dims(
                                    &img,
                                    source_dims,
                                    &folder_key,
                                    &catalog,
                                    *zip_mtime,
                                    *zip_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }

                // ── PDF ファイルの中身をキャッシュ ──
                if !pdf_files.is_empty() && !cancel.load(Ordering::Relaxed) {
                    let pw_store = crate::pdf_passwords::PdfPasswordStore::load();

                    for (pdf_path, pdf_mtime, pdf_file_size) in &pdf_files {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let pdf_fname = match pdf_path.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        *current.lock().unwrap() = format!("{} > {}", folder_display, pdf_fname);
                        let password = pw_store.get(pdf_path);
                        let pw_ref = password.as_deref();
                        let folder_key = format!("{}{}", CACHE_KEY_PDF, pdf_fname);

                        if batch_pdf {
                            // enumerate_pages がパスワード不正時に Err を返すので
                            // 事前のパスワード判定は不要
                            let pages = match crate::pdf_loader::enumerate_pages(pdf_path, pw_ref) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let pdf_catalog =
                                match crate::catalog::CatalogDb::open(&cache_dir, pdf_path) {
                                    Ok(c) => c,
                                    Err(_) => continue,
                                };
                            let pdf_cache_map = pdf_catalog.load_all().unwrap_or_default();
                            let page_count = pages.len();

                            // PDF メタキャッシュ (v1.0.0) に確定 page_count を書き込む
                            // (Codex P3 対応: バッチキャッシュ生成経路でも pdf_meta を
                            // 投入することで、その後の Enter で instant ヒットさせる)。
                            if let Some(filename) = pdf_path.file_name().and_then(|n| n.to_str()) {
                                // `pw_store.get(pdf_path)` が `password.is_some()` ==
                                // 「この PDF 固有の保存パスワードがある」と等価。session 経由
                                // ではないので true/false 判定がそのまま信頼できる。
                                let password_required = password.is_some();
                                if let Err(e) = catalog.set_pdf_meta(
                                    filename,
                                    *pdf_mtime,
                                    *pdf_file_size,
                                    page_count as u32,
                                    password_required,
                                ) {
                                    crate::logger::log(format!(
                                        "  cache creator: set_pdf_meta failed for {filename}: {e}"
                                    ));
                                }
                            }

                            // PDFium ワーカーはシングルスレッド → 順次処理
                            for i in 0..page_count {
                                if cancel.load(Ordering::Relaxed) {
                                    break;
                                }
                                let page_num = i as u32;
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})",
                                    folder_display,
                                    pdf_fname,
                                    i + 1,
                                    page_count
                                );
                                let key = crate::grid_item::pdf_page_cache_key(page_num);
                                if let Some(existing) = pdf_cache_map.get(&key) {
                                    if existing.mtime == *pdf_mtime
                                        && existing.file_size == *pdf_file_size
                                    {
                                        continue;
                                    }
                                }
                                if let Some(bytes) = crate::thumb_loader::build_and_save_one_pdf(
                                    pdf_path,
                                    page_num,
                                    pw_ref,
                                    &pdf_catalog,
                                    *pdf_mtime,
                                    *pdf_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }

                            // 先頭1ページを親フォルダの DB にも保存
                            if page_count > 0 && !cache_map.contains_key(&folder_key) {
                                // bulk cache creator は background なので epoch=0
                                // + AbortOnCancel (cancel = ユーザ明示中断意図)
                                if let Ok(res) = crate::pdf_loader::render_page(
                                    pdf_path,
                                    0,
                                    thumb_px,
                                    pw_ref,
                                    None,
                                    crate::pdf_loader::JobPriority::Normal,
                                    0,
                                    crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
                                ) {
                                    if let Some(bytes) = encode_and_save_with_source_dims(
                                        &res.image,
                                        res.page_size_points.catalog_layout_dims(),
                                        &folder_key,
                                        &catalog,
                                        *pdf_mtime,
                                        *pdf_file_size,
                                        thumb_px,
                                        thumb_quality,
                                    ) {
                                        size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                    }
                                }
                            }
                        } else {
                            // 先頭1ページのみ（フォルダ一覧用サムネイル）
                            //
                            // **早期 continue より先に pdf_meta を補完**する (Codex P3
                            // follow-up 対応): 既存ユーザーで thumb は既にキャッシュ済み
                            // だが pdf_meta が未投入の PDF が存在する。`continue` で
                            // 飛ばすと pdf_meta が永遠に埋まらず、Enter→placeholder の
                            // 即時化が効かない穴になる。enumerate_pages を 1 回呼んで
                            // page_count を取り pdf_meta を埋めるだけ (中身の render は
                            // 不要なのでサムネ再生成は引き続き skip)。
                            if let Some(filename) = pdf_path.file_name().and_then(|n| n.to_str()) {
                                let meta_missing = catalog
                                    .get_pdf_meta(filename, *pdf_mtime, *pdf_file_size)
                                    .ok()
                                    .flatten()
                                    .is_none();
                                if meta_missing {
                                    if let Ok(pages) =
                                        crate::pdf_loader::enumerate_pages(pdf_path, pw_ref)
                                    {
                                        let password_required = password.is_some();
                                        if let Err(e) = catalog.set_pdf_meta(
                                            filename,
                                            *pdf_mtime,
                                            *pdf_file_size,
                                            pages.len() as u32,
                                            password_required,
                                        ) {
                                            crate::logger::log(format!(
                                                "  cache creator (catch-up): set_pdf_meta failed for {filename}: {e}"
                                            ));
                                        }
                                    }
                                }
                            }
                            if cache_map.contains_key(&folder_key) {
                                continue;
                            }
                            // render_page がパスワード不正時に Err を返すのでそのままスキップ
                            // bulk cache creator は background なので epoch=0 + AbortOnCancel
                            if let Ok(res) = crate::pdf_loader::render_page(
                                pdf_path,
                                0,
                                thumb_px,
                                pw_ref,
                                None,
                                crate::pdf_loader::JobPriority::Normal,
                                0,
                                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
                            ) {
                                // PDF メタキャッシュにも page_count を投入する
                                // (Codex P3 対応)。`password_required` は確信できる場合
                                // (= この PDF 固有の保存パスワードあり) のみ true。
                                if let Some(filename) =
                                    pdf_path.file_name().and_then(|n| n.to_str())
                                {
                                    let password_required = password.is_some();
                                    if let Err(e) = catalog.set_pdf_meta(
                                        filename,
                                        *pdf_mtime,
                                        *pdf_file_size,
                                        res.page_count,
                                        password_required,
                                    ) {
                                        crate::logger::log(format!(
                                            "  cache creator (single): set_pdf_meta failed for {filename}: {e}"
                                        ));
                                    }
                                }
                                if let Some(bytes) = encode_and_save_with_source_dims(
                                    &res.image,
                                    res.page_size_points.catalog_layout_dims(),
                                    &folder_key,
                                    &catalog,
                                    *pdf_mtime,
                                    *pdf_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }

                done.fetch_add(1, Ordering::Relaxed);
            }

            finished.store(true, Ordering::Relaxed);
        });
    }
}
