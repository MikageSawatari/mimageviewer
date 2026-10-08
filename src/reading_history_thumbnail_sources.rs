//! Worker-prepared thumbnail provenance for the existing reading-history surface.
//! The entries and shared video/audio sidecar map are adopted together by the App owner.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::reading_history_db::{ReadingHistoryEntry, ReadingHistoryKind};

const MAX_PARENT_SCANS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub(crate) items_generation: u64,
    pub(crate) context_id: crate::app::ViewerContextId,
    pub(crate) quick_folder_switch_sequence: u64,
}

impl Owner {
    pub(crate) fn is_current(self, current: Self) -> bool {
        self == current
    }
}

/// Only the flags consumed by the canonical folder scan and sidecar selection.
#[derive(Clone, Copy)]
pub(crate) struct DiscoverySettings {
    skip_image_if_video_exists: bool,
    video_thumb_use_sidecar_image: bool,
    show_hidden_files: bool,
    archive_file_handling: crate::settings::ArchiveFileHandling,
    epub_file_handling: crate::settings::EpubFileHandling,
}

impl DiscoverySettings {
    pub(crate) fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self {
            skip_image_if_video_exists: settings.skip_image_if_video_exists,
            video_thumb_use_sidecar_image: settings.video_thumb_use_sidecar_image,
            show_hidden_files: settings.show_hidden_files,
            archive_file_handling: settings.archive_file_handling,
            epub_file_handling: settings.epub_file_handling,
        }
    }

    fn worker_settings(self) -> crate::settings::Settings {
        crate::settings::Settings {
            skip_image_if_video_exists: self.skip_image_if_video_exists,
            video_thumb_use_sidecar_image: self.video_thumb_use_sidecar_image,
            show_hidden_files: self.show_hidden_files,
            archive_file_handling: self.archive_file_handling,
            epub_file_handling: self.epub_file_handling,
            ..Default::default()
        }
    }
}

pub(crate) struct Prepared {
    pub(crate) entries: Vec<ReadingHistoryEntry>,
    pub(crate) sidecars: HashMap<String, PathBuf>,
}

pub(crate) struct Pending {
    pub(crate) rx: mpsc::Receiver<Prepared>,
    pub(crate) owner: Owner,
    cancel: Arc<AtomicBool>,
}

impl Pending {
    pub(crate) fn start(
        entries: Vec<ReadingHistoryEntry>,
        settings: DiscoverySettings,
        owner: Owner,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("reading-history-thumbnail-sources".into())
            .spawn(move || {
                if let Some(prepared) = prepare(entries, settings, &worker_cancel)
                    && !worker_cancel.load(Ordering::Acquire)
                {
                    let _ = tx.send(prepared);
                }
            })?;
        Ok(Self { rx, owner, cancel })
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn prepare(
    entries: Vec<ReadingHistoryEntry>,
    settings: DiscoverySettings,
    cancel: &AtomicBool,
) -> Option<Prepared> {
    if cancel.load(Ordering::Acquire) {
        return None;
    }
    let paths = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                ReadingHistoryKind::Video | ReadingHistoryKind::Audio
            )
        })
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let settings = settings.worker_settings();
    let sources = crate::app::folder_scan::discover_aggregate_video_sidecars_while(
        &settings,
        &paths,
        MAX_PARENT_SCANS,
        || !cancel.load(Ordering::Acquire),
    )?;
    if sources.skipped_parents > 0 {
        crate::logger::log(format!(
            "reading-history: sidecar parent scan capped scanned={} skipped={} limit={MAX_PARENT_SCANS}",
            sources.scanned_parents, sources.skipped_parents
        ));
    }
    for (parent, error) in &sources.scan_errors {
        crate::logger::log(format!(
            "reading-history: sidecar scan failed parent={} error={error}",
            parent.display()
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return None;
    }
    Some(Prepared {
        entries,
        sidecars: sources.by_video_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: PathBuf, kind: ReadingHistoryKind) -> ReadingHistoryEntry {
        ReadingHistoryEntry::new(path, kind, None, "history".into(), None, None)
    }

    fn settings() -> DiscoverySettings {
        DiscoverySettings::from_settings(&crate::settings::Settings::default())
    }

    fn owner(context: u64) -> Owner {
        Owner {
            items_generation: 12,
            context_id: crate::app::ViewerContextId::for_test(context),
            quick_folder_switch_sequence: 3,
        }
    }

    #[test]
    fn worker_prepares_flac_sidecar_without_changing_history_display_stamp() {
        let temp = tempfile::tempdir().unwrap();
        let audio = temp.path().join("song.flac");
        let image = temp.path().join("song.jpg");
        std::fs::write(&audio, b"audio").unwrap();
        std::fs::write(&image, b"image").unwrap();
        let original = entry(audio.clone(), ReadingHistoryKind::Audio);
        assert_eq!(original.mtime_ms, None);
        let pending = Pending::start(vec![original.clone()], settings(), owner(1)).unwrap();
        let prepared = pending
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(prepared.entries, vec![original]);
        assert_eq!(
            prepared
                .sidecars
                .get(&crate::path_key::normalize_keep_drive(&audio)),
            Some(&image)
        );
    }

    #[test]
    fn canceled_prepare_never_returns_a_partial_source_snapshot() {
        let canceled = AtomicBool::new(true);
        assert!(
            prepare(
                vec![entry(
                    PathBuf::from(r"C:\Music\song.mp3"),
                    ReadingHistoryKind::Audio
                )],
                settings(),
                &canceled
            )
            .is_none()
        );
        let pending = Pending::start(Vec::new(), settings(), owner(1)).unwrap();
        let flag = Arc::clone(&pending.cancel);
        drop(pending);
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn source_parent_budget_is_shared_video_first_with_audio() {
        let temp = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for index in 0..MAX_PARENT_SCANS {
            let parent = temp.path().join(format!("video-{index}"));
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("clip.mp4");
            std::fs::write(&path, b"video").unwrap();
            std::fs::write(parent.join("clip.jpg"), b"image").unwrap();
            entries.push(entry(path, ReadingHistoryKind::Video));
        }
        let parent = temp.path().join("audio");
        std::fs::create_dir(&parent).unwrap();
        let audio = parent.join("song.flac");
        std::fs::write(&audio, b"audio").unwrap();
        std::fs::write(parent.join("song.jpg"), b"image").unwrap();
        entries.insert(0, entry(audio.clone(), ReadingHistoryKind::Audio));
        let prepared = prepare(entries, settings(), &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.entries.len(), MAX_PARENT_SCANS + 1);
        assert_eq!(prepared.sidecars.len(), MAX_PARENT_SCANS);
        assert!(
            !prepared
                .sidecars
                .contains_key(&crate::path_key::normalize_keep_drive(&audio))
        );
    }

    #[test]
    fn owner_requires_context_generation_and_navigation_sequence() {
        let first = owner(1);
        assert!(first.is_current(first));
        assert!(!first.is_current(owner(2)));
        assert!(!first.is_current(Owner {
            items_generation: 13,
            ..first
        }));
        assert!(!first.is_current(Owner {
            quick_folder_switch_sequence: 4,
            ..first
        }));
    }

    #[test]
    fn shared_source_keys_preserve_drive_identity() {
        let c = crate::path_key::normalize_keep_drive(std::path::Path::new(r"C:\Music\song.mp3"));
        let d = crate::path_key::normalize_keep_drive(std::path::Path::new(r"D:\Music\song.mp3"));
        let map = HashMap::from([
            (c.clone(), PathBuf::from(r"C:\Music\song.jpg")),
            (d.clone(), PathBuf::from(r"D:\Music\song.jpg")),
        ]);
        assert_ne!(c, d);
        assert_eq!(map.len(), 2);
        assert_ne!(map[&c], map[&d]);
    }
}
