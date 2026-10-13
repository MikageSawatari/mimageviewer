//! グリッド選択の一括エクスポート (Ctrl+E)。
//!
//! 合成は製本と同じ [`crate::books::write_composited_page`] を使う。この module が
//! 持つのは「出力先 / ファイル名 / 縮小 / 進捗とキャンセル」だけで、デコード → 合成 →
//! エンコードのパイプラインを二重に持たない。
//!
//! 進捗は Ctrl+E 単ページと同じ [`crate::export_dialog::ExportEvent`] /
//! [`crate::export_dialog::ExportPending`] で報告するので、進捗ダイアログと
//! `poll_export_pending` はそのまま動く。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::export_dialog::{ExportEvent, ExportFailure, ExportPending, ExportScale, ExportSuccess};

/// テンプレートに使える置換子。ダイアログのヒントと [`expand_filename_template`] の
/// 唯一の定義元。
pub const TEMPLATE_PLACEHOLDER_HINT: &str = "<filename> / <dirname> / <num> が使えます";

/// 書き出し 1 件ぶん。
pub struct BatchExportItem {
    /// `<filename>` に入る名前 (拡張子なし)。
    pub filename: String,
    /// `<dirname>` に入る名前 (実フォルダ名 / ZIP・PDF のファイル名)。
    pub dirname: String,
    pub source: crate::books::CompositeSource,
    pub edits: crate::books::BakedEditSnapshot,
    pub(crate) composition_check: Option<crate::book_composition::CompositionOutputCheck>,
}

impl BatchExportItem {
    /// Creates an item for callers without an App composition context.
    /// App selection factories must attach their captured visibility check.
    pub fn new(
        filename: String,
        dirname: String,
        source: crate::books::CompositeSource,
        edits: crate::books::BakedEditSnapshot,
    ) -> Self {
        Self {
            filename,
            dirname,
            source,
            edits,
            composition_check: None,
        }
    }
}

pub struct BatchExportRequest {
    pub output_dir: PathBuf,
    pub template: String,
    pub scale: ExportScale,
    pub items: Vec<BatchExportItem>,
    pub raw: Option<crate::raw::RawDecodeContext>,
    /// worker が終わるまで「ローカル AI を使い得る作業が走っている」ことを示す借用。
    ///
    /// 合成は消しゴム (MI-GAN) や AI 拡大を回し得るので、リモートへ操作権を渡す前の
    /// 静止確認 (`local_ai_remote_barrier_snapshot`) はこの worker を数えなければならない。
    /// 数えていなかったので、消しゴム付きの一括書き出し中に接続するとローカル AI が
    /// 走ったまま操作権が移り、リモート側の AI と GPU / モデルを取り合っていた
    /// (v3.5.0 レビュー F09)。
    ///
    /// **どの項目が AI を回すかを起動前に決めない。** 実際に何を回すかは worker が
    /// 合成しながら決めるので、ここで先読みすると「AI を使うか」の綴りが 2 つになる。
    /// 借用は worker の寿命そのもので、cancel でも panic でも drop で返る。
    ///
    /// **`Option` にしない。** 一括書き出しは必ず合成を通るので、渡し忘れを型で防ぐ
    /// (単枚の `ExportRequest` 側が `Option` なのは、AI を通らない経路が実在するため)。
    pub local_ai_activity: crate::app::LocalAiActivityLease,
}

/// `<filename>` / `<dirname>` / `<num>` を展開する。未知の `<...>` は展開せずそのまま
/// 残す (テンプレートの打ち間違いを黙って消さない)。`index` は 1 origin。
pub fn expand_filename_template(
    template: &str,
    filename: &str,
    dirname: &str,
    index: usize,
) -> String {
    template
        .replace("<filename>", filename)
        .replace("<dirname>", dirname)
        .replace("<num>", &format!("{index:04}"))
}

/// テンプレート展開 → ファイル名として使える文字へ正規化。
///
/// `basename_from_text` は禁止文字を `_` へ置き換え、空にはならない (空入力は
/// `capture` になる) ので、ここに追加の空文字フォールバックは置かない。
pub fn resolve_item_stem(template: &str, item: &BatchExportItem, index: usize) -> String {
    let expanded = expand_filename_template(template, &item.filename, &item.dirname, index);
    crate::capture::basename_from_text(&expanded)
}

/// まだ使われていない出力パスを決める。既存ファイルも、この実行で既に書いた名前も
/// 避ける (テンプレート次第で複数の元画像が同じ名前へ落ちる)。上書きはしない。
fn unique_target_path(
    output_dir: &Path,
    stem: &str,
    extension: &str,
    claimed: &mut HashSet<PathBuf>,
) -> Result<PathBuf, String> {
    for seq in 0..=9999u32 {
        let candidate = if seq == 0 {
            output_dir.join(format!("{stem}.{extension}"))
        } else {
            output_dir.join(format!("{stem}_{seq}.{extension}"))
        };
        if !claimed.contains(&candidate) && !candidate.exists() {
            claimed.insert(candidate.clone());
            return Ok(candidate);
        }
    }
    Err(format!("同名ファイルが多すぎます: {stem}.{extension}"))
}

pub fn spawn_batch_export_worker(request: BatchExportRequest) -> Result<ExportPending, String> {
    let total = request.items.len();
    if total == 0 {
        return Err("エクスポートする画像がありません".to_string());
    }
    let BatchExportRequest {
        output_dir,
        template,
        scale,
        items,
        raw,
        local_ai_activity,
    } = request;
    let request = BatchExportPlan {
        output_dir,
        template,
        scale,
        items,
        raw,
    };
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("ctrl-e-batch-export".into())
        .spawn(move || {
            // 借用は worker が終わるまで生かす (cancel / panic でも drop で返る)。
            let _local_ai_activity = local_ai_activity;
            run_batch_export(request, worker_cancel, tx)
        })
        .map_err(|e| format!("一括エクスポート worker を開始できません: {e}"))?;
    Ok(ExportPending {
        cancel,
        rx,
        total,
        done: 0,
        last_message: "準備中".to_string(),
        successes: Vec::new(),
        errors: Vec::new(),
        finished: false,
        cancel_requested: false,
    })
}

/// 借用を外した、worker が実際に読む部分。
struct BatchExportPlan {
    output_dir: PathBuf,
    template: String,
    scale: ExportScale,
    items: Vec<BatchExportItem>,
    raw: Option<crate::raw::RawDecodeContext>,
}

fn run_batch_export(
    request: BatchExportPlan,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ExportEvent>,
) {
    if let Err(error) = std::fs::create_dir_all(&request.output_dir) {
        let message = format!(
            "保存先フォルダを作成できません: {}: {error}",
            request.output_dir.display()
        );
        for item in &request.items {
            let _ = tx.send(ExportEvent::Failed(ExportFailure {
                label: item.filename.clone(),
                message: message.clone(),
            }));
        }
        let _ = tx.send(ExportEvent::AllDone);
        return;
    }

    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut resolver = crate::book_composition::CompositionOutputResolver::default();
    for (index, item) in request.items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let label = item.filename.clone();
        let _ = tx.send(ExportEvent::Started {
            label: label.clone(),
        });
        if let Some(check) = &item.composition_check {
            let refusal = match check.permits_with(&mut resolver) {
                Ok(true) => None,
                Ok(false) => {
                    Some(crate::book_composition::COMPOSITION_OUTPUT_HIDDEN_NOTICE.to_owned())
                }
                Err(reason) => Some(reason),
            };
            if let Some(message) = refusal {
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
                continue;
            }
        }
        let stem = resolve_item_stem(&request.template, item, index + 1);
        let path = match unique_target_path(
            &request.output_dir,
            &stem,
            item.edits.format.extension(),
            &mut claimed,
        ) {
            Ok(path) => path,
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
                continue;
            }
        };
        match crate::books::write_composited_page(
            &item.source,
            &item.edits,
            &path,
            request.scale,
            request.raw.as_ref(),
        ) {
            Ok(_) => {
                let _ = tx.send(ExportEvent::Completed(ExportSuccess { label, path }));
            }
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
            }
        }
    }
    let _ = tx.send(ExportEvent::AllDone);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(filename: &str, dirname: &str) -> BatchExportItem {
        BatchExportItem {
            composition_check: None,
            filename: filename.to_string(),
            dirname: dirname.to_string(),
            source: crate::books::CompositeSource::File {
                path: PathBuf::from("unused"),
            },
            edits: crate::books::BakedEditSnapshot {
                params: crate::adjustment::AdjustParams::default(),
                rotation: crate::rotation_db::Rotation::None,
                conceal: None,
                erase: None,
                local_adjust: None,
                comic: None,
                comic_source_dims: None,
                export_crop: None,
                crop_legacy_writeback: None,
                format: crate::capture::CaptureFormat::Png,
                jpeg_matte: crate::capture::JpegMatte::Black,
                stage: crate::bake_stage::BakeStage::default(),
                creative_lut: None,
                ai: None,
            },
        }
    }

    #[test]
    fn template_expands_every_placeholder() {
        let got = expand_filename_template("<dirname>/<filename>_<num>", "shot", "trip", 7);

        assert_eq!(got, "trip/shot_0007");
    }

    #[test]
    fn template_keeps_unknown_placeholders_instead_of_dropping_them() {
        let got = expand_filename_template("<filename>_<unknown>", "shot", "trip", 1);

        assert_eq!(got, "shot_<unknown>");
    }

    #[test]
    fn stem_replaces_path_separators_so_the_output_stays_in_the_chosen_folder() {
        let got = resolve_item_stem("<dirname>/<filename>", &item("shot", "trip"), 1);

        assert_eq!(got, "trip_shot");
    }

    #[test]
    fn unique_target_path_avoids_existing_files_and_names_claimed_in_this_run() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("out.png"), b"x").unwrap();
        let mut claimed = HashSet::new();

        let first = unique_target_path(temp.path(), "out", "png", &mut claimed).unwrap();
        let second = unique_target_path(temp.path(), "out", "png", &mut claimed).unwrap();

        assert_eq!(first, temp.path().join("out_1.png"));
        assert_eq!(second, temp.path().join("out_2.png"));
    }

    #[test]
    fn two_items_that_expand_to_the_same_name_both_get_written() {
        let temp = tempfile::tempdir().unwrap();
        let mut claimed = HashSet::new();

        let a = resolve_item_stem("<dirname>", &item("one", "same"), 1);
        let b = resolve_item_stem("<dirname>", &item("two", "same"), 2);
        let a = unique_target_path(temp.path(), &a, "png", &mut claimed).unwrap();
        let b = unique_target_path(temp.path(), &b, "png", &mut claimed).unwrap();

        assert_ne!(a, b);
    }
    #[test]
    fn worker_qualifies_captured_selection_against_latest_hidden_and_preserves_visible_order() {
        use crate::book_composition::{
            BookComposition, BookCompositionService, CandidatePage, CompositionOutputCheck,
            CompositionPageRef, PageDomain, ReadingDirection,
        };
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("real.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([50, 60, 70, 255]))
            .save(&input)
            .unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
        let scope = crate::book_scope::BookScopeKey::document("C:/selected.pdf").unwrap();
        service
            .prepare(scope.clone(), PageDomain::Document)
            .recv()
            .unwrap()
            .status
            .unwrap();
        let mut items: Vec<_> = [2, 0, 1]
            .into_iter()
            .map(|page| {
                let mut item = item(&format!("page-{page}"), "book");
                item.source = crate::books::CompositeSource::File {
                    path: input.clone(),
                };
                item.composition_check = Some(CompositionOutputCheck::new(
                    service.clone(),
                    CandidatePage {
                        identity: crate::book_resume_db::PageIdentity::Document {
                            container: "C:/selected.pdf".into(),
                            page_num: page,
                        },
                        physical_archive: None,
                        raw_tree: None,
                    },
                ));
                item
            })
            .collect();
        // A hidden selection may be nonexistent; admission must precede decode.
        items[2].source = crate::books::CompositeSource::File {
            path: temp.path().join("hidden-must-not-decode.png"),
        };
        let mut record = BookComposition::new(
            ReadingDirection::Ltr,
            (0..3)
                .map(|page_num| CompositionPageRef::PdfPage { page_num })
                .collect(),
        );
        record
            .hidden_pages
            .push(CompositionPageRef::PdfPage { page_num: 1 });
        service
            .save(scope, PageDomain::Document, record)
            .recv()
            .unwrap()
            .status
            .unwrap();
        let output = temp.path().join("out");
        let (tx, rx) = mpsc::channel();
        run_batch_export(
            BatchExportPlan {
                output_dir: output.clone(),
                template: "<num>_<filename>".into(),
                scale: ExportScale::Full,
                items,
                raw: None,
            },
            Arc::new(AtomicBool::new(false)),
            tx,
        );
        let events: Vec<_> = rx.into_iter().collect();
        let completed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ExportEvent::Completed(result) => Some(result.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(completed, vec!["page-2", "page-0"]);
        assert!(events.iter().any(|event| matches!(event, ExportEvent::Failed(failure) if failure.label == "page-1" && failure.message == crate::book_composition::COMPOSITION_OUTPUT_HIDDEN_NOTICE)));
        assert!(output.join("0001_page-2.png").exists());
        assert!(output.join("0002_page-0.png").exists());
        assert_eq!(std::fs::read_dir(output).unwrap().count(), 2);
        owner.shutdown_and_join();
    }

    #[test]
    fn worker_reports_unreadable_composition_without_reading_source_or_writing_page() {
        use crate::book_composition::{
            BookCompositionService, CandidatePage, CompositionOutputCheck, PageDomain,
        };
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("book_composition.db"),
            b"preserve unreadable composition",
        )
        .unwrap();
        let (service, owner) = BookCompositionService::spawn_at(data.clone(), None).unwrap();
        service
            .prepare(
                crate::book_scope::BookScopeKey::document("C:/selected.pdf").unwrap(),
                PageDomain::Document,
            )
            .recv()
            .unwrap();
        let mut source = item("unreadable", "book");
        source.composition_check = Some(CompositionOutputCheck::new(
            service,
            CandidatePage {
                identity: crate::book_resume_db::PageIdentity::Document {
                    container: "C:/selected.pdf".into(),
                    page_num: 0,
                },
                physical_archive: None,
                raw_tree: None,
            },
        ));
        let output = temp.path().join("out");
        let (tx, rx) = mpsc::channel();
        run_batch_export(
            BatchExportPlan {
                output_dir: output.clone(),
                template: "<filename>".into(),
                scale: ExportScale::Full,
                items: vec![source],
                raw: None,
            },
            Arc::new(AtomicBool::new(false)),
            tx,
        );
        let events: Vec<_> = rx.into_iter().collect();
        assert!(events.iter().any(
            |event| matches!(event, ExportEvent::Failed(failure) if failure.label == "unreadable")
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ExportEvent::Completed(_)))
        );
        assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
        owner.shutdown_and_join();
        assert_eq!(
            std::fs::read(data.join("book_composition.db")).unwrap(),
            b"preserve unreadable composition"
        );
    }
}
