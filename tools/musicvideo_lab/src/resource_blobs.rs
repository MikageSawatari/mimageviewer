use std::any::Any;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use musicvideo_core::{ImageAsset, ImageId};
use musicvideo_ui::{ResourceLease, ResourceManifest};

use crate::decode::AudioData;

enum CleanupCommand {
    Drop(Box<dyn Any + Send>),
}

#[derive(Clone)]
pub struct ResourceBlobStore {
    cleanup_tx: mpsc::Sender<CleanupCommand>,
}

impl ResourceBlobStore {
    pub fn new() -> Result<Self, String> {
        let (cleanup_tx, cleanup_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("musicvideo-resource-cleanup".to_owned())
            .spawn(move || {
                while let Ok(CleanupCommand::Drop(value)) = cleanup_rx.recv() {
                    drop(value);
                }
            })
            .map_err(|error| format!("素材履歴の解放ワーカーを開始できません: {error}"))?;
        Ok(Self { cleanup_tx })
    }

    pub fn store_image(
        &self,
        source_path: PathBuf,
        image: &ImageAsset,
    ) -> Result<ResourceLease, String> {
        let mut file = temporary_blob()?;
        write_blob(&mut file, &image.rgba)?;
        Ok(ResourceLease::new(Arc::new(ResourceBlob {
            file: Mutex::new(Some(file)),
            source_path,
            kind: BlobKind::Image {
                width: image.width,
                height: image.height,
            },
            cleanup_tx: self.cleanup_tx.clone(),
        })))
    }

    pub fn store_audio(
        &self,
        source_path: PathBuf,
        audio: &AudioData,
    ) -> Result<ResourceLease, String> {
        let mut file = temporary_blob()?;
        write_f32_blob(&mut file, &audio.stereo_pcm)?;
        Ok(ResourceLease::new(Arc::new(ResourceBlob {
            file: Mutex::new(Some(file)),
            source_path,
            kind: BlobKind::Audio {
                sample_rate: audio.sample_rate,
                duration_secs: audio.duration_secs,
                samples: audio.stereo_pcm.len(),
            },
            cleanup_tx: self.cleanup_tx.clone(),
        })))
    }

    pub fn restore_image(
        &self,
        lease: &ResourceLease,
        cancel: &AtomicBool,
    ) -> Result<ImageAsset, String> {
        let blob = resource_blob(lease)?;
        let BlobKind::Image { width, height } = blob.kind else {
            return Err("画像素材の履歴が音声を参照しています".to_owned());
        };
        let rgba = blob.with_file(|file| read_blob(file, cancel))?;
        ImageAsset::new(width, height, rgba).map_err(|error| {
            format!(
                "画像素材の履歴を復元できません ({}): {error}",
                blob.source_path.display()
            )
        })
    }

    pub fn restore_audio(
        &self,
        lease: &ResourceLease,
        cancel: &AtomicBool,
    ) -> Result<AudioData, String> {
        let blob = resource_blob(lease)?;
        let BlobKind::Audio {
            sample_rate,
            duration_secs,
            samples,
        } = blob.kind
        else {
            return Err("音声素材の履歴が画像を参照しています".to_owned());
        };
        let stereo_pcm = blob
            .with_file(|file| read_f32_blob(file, samples, cancel))
            .map_err(|error| {
                format!(
                    "音声素材の履歴を復元できません ({}): {error}",
                    blob.source_path.display()
                )
            })?;
        Ok(AudioData {
            stereo_pcm,
            sample_rate,
            duration_secs,
        })
    }

    pub fn defer_drop<T>(&self, value: T)
    where
        T: Any + Send,
    {
        let command = CleanupCommand::Drop(Box::new(value));
        if let Err(error) = self.cleanup_tx.send(command) {
            let command = error.0;
            let _ = std::thread::Builder::new()
                .name("musicvideo-resource-cleanup-fallback".to_owned())
                .spawn(move || drop(command));
        }
    }
}

#[derive(Clone)]
pub struct RestoredResources {
    pub manifest: ResourceManifest,
    pub audio: Option<Arc<AudioData>>,
    pub images: Vec<(ImageId, Arc<ImageAsset>)>,
}

impl fmt::Debug for RestoredResources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestoredResources")
            .field("manifest", &self.manifest)
            .field(
                "audio_samples",
                &self.audio.as_ref().map(|audio| audio.stereo_pcm.len()),
            )
            .field("images", &self.images.len())
            .finish()
    }
}

pub fn restore_manifest(
    store: &ResourceBlobStore,
    manifest: ResourceManifest,
    cancel: &AtomicBool,
) -> Result<RestoredResources, String> {
    let audio = manifest
        .audio()
        .map(|lease| store.restore_audio(lease, cancel).map(Arc::new))
        .transpose()?;
    let mut images = Vec::new();
    for (id, lease) in manifest.image_leases() {
        if cancel.load(Ordering::Relaxed) {
            return Err("素材履歴の復元をキャンセルしました".to_owned());
        }
        images.push((id.clone(), Arc::new(store.restore_image(lease, cancel)?)));
    }
    Ok(RestoredResources {
        manifest,
        audio,
        images,
    })
}

#[derive(Debug, Clone)]
enum BlobKind {
    Image {
        width: u32,
        height: u32,
    },
    Audio {
        sample_rate: u32,
        duration_secs: f64,
        samples: usize,
    },
}

struct ResourceBlob {
    file: Mutex<Option<File>>,
    source_path: PathBuf,
    kind: BlobKind,
    cleanup_tx: mpsc::Sender<CleanupCommand>,
}

impl ResourceBlob {
    fn with_file<T>(&self, read: impl FnOnce(&mut File) -> Result<T, String>) -> Result<T, String> {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = file
            .as_mut()
            .ok_or_else(|| "素材履歴の一時ファイルは既に解放されています".to_owned())?;
        read(file)
    }
}

impl Drop for ResourceBlob {
    fn drop(&mut self) {
        let file = self
            .file
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(file) = file {
            let command = CleanupCommand::Drop(Box::new(file));
            if let Err(error) = self.cleanup_tx.send(command) {
                let command = error.0;
                let _ = std::thread::Builder::new()
                    .name("musicvideo-resource-cleanup-fallback".to_owned())
                    .spawn(move || drop(command));
            }
        }
    }
}

fn resource_blob(lease: &ResourceLease) -> Result<Arc<ResourceBlob>, String> {
    lease
        .payload::<ResourceBlob>()
        .ok_or_else(|| "素材履歴の所有形式が一致しません".to_owned())
}

fn temporary_blob() -> Result<File, String> {
    tempfile::tempfile().map_err(|error| format!("素材履歴の一時ファイルを作成できません: {error}"))
}

fn write_blob(file: &mut File, bytes: &[u8]) -> Result<(), String> {
    let mut writer = BufWriter::new(file);
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .map_err(|error| format!("素材履歴を書き込めません: {error}"))
}

fn read_blob(file: &mut File, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("素材履歴を先頭へ戻せません: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut output = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("素材履歴の復元をキャンセルしました".to_owned());
        }
        let read = reader
            .read(&mut chunk)
            .map_err(|error| format!("素材履歴を読めません: {error}"))?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
    }
    Ok(output)
}

fn write_f32_blob(file: &mut File, samples: &[f32]) -> Result<(), String> {
    let mut writer = BufWriter::new(file);
    let mut chunk = Vec::with_capacity(64 * 1024);
    for samples in samples.chunks(16 * 1024) {
        chunk.clear();
        for sample in samples {
            chunk.extend_from_slice(&sample.to_le_bytes());
        }
        writer
            .write_all(&chunk)
            .map_err(|error| format!("素材履歴を書き込めません: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("素材履歴を書き込めません: {error}"))
}

fn read_f32_blob(file: &mut File, samples: usize, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    let bytes = read_blob(file, cancel)?;
    let expected = samples
        .checked_mul(4)
        .ok_or_else(|| "音声素材の履歴サイズが大きすぎます".to_owned())?;
    if bytes.len() != expected {
        return Err(format!(
            "音声素材の履歴サイズが一致しません: expected {expected}, got {}",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DropThread(mpsc::Sender<std::thread::ThreadId>);

    impl Drop for DropThread {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    #[test]
    fn image_and_audio_blobs_round_trip_without_the_source_file() {
        let store = ResourceBlobStore::new().unwrap();
        let image = ImageAsset::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 128]).unwrap();
        let image_lease = store
            .store_image(PathBuf::from("deleted.png"), &image)
            .unwrap();
        let audio = AudioData {
            stereo_pcm: vec![-1.0, 0.25, 0.5, 1.0],
            sample_rate: 48_000,
            duration_secs: 2.0 / 48_000.0,
        };
        let audio_lease = store
            .store_audio(PathBuf::from("deleted.wav"), &audio)
            .unwrap();
        let cancel = AtomicBool::new(false);

        let restored_image = store.restore_image(&image_lease, &cancel).unwrap();
        let restored_audio = store.restore_audio(&audio_lease, &cancel).unwrap();

        assert_eq!(restored_image.rgba, image.rgba);
        assert_eq!(restored_audio.stereo_pcm, audio.stereo_pcm);
        assert_eq!(restored_audio.sample_rate, audio.sample_rate);
        assert_eq!(restored_audio.duration_secs, audio.duration_secs);
    }

    #[test]
    fn same_source_path_creates_distinct_revisions_and_content() {
        let store = ResourceBlobStore::new().unwrap();
        let path = PathBuf::from("same.png");
        let first = ImageAsset::new(1, 1, vec![1, 0, 0, 255]).unwrap();
        let second = ImageAsset::new(1, 1, vec![2, 0, 0, 255]).unwrap();
        let first_lease = store.store_image(path.clone(), &first).unwrap();
        let second_lease = store.store_image(path, &second).unwrap();

        assert_ne!(first_lease.revision(), second_lease.revision());
        assert_eq!(
            store
                .restore_image(&first_lease, &AtomicBool::new(false))
                .unwrap()
                .rgba[0],
            1
        );
        assert_eq!(
            store
                .restore_image(&second_lease, &AtomicBool::new(false))
                .unwrap()
                .rgba[0],
            2
        );
    }

    #[test]
    fn separate_stores_cannot_allocate_the_same_resource_revision() {
        let first_store = ResourceBlobStore::new().unwrap();
        let second_store = ResourceBlobStore::new().unwrap();
        let image = ImageAsset::new(1, 1, vec![1, 2, 3, 255]).unwrap();
        let first = first_store
            .store_image(PathBuf::from("same.png"), &image)
            .unwrap();
        let second = second_store
            .store_image(PathBuf::from("same.png"), &image)
            .unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn one_blob_can_be_restored_concurrently_without_sharing_a_file_cursor() {
        let store = ResourceBlobStore::new().unwrap();
        let expected = (0..262_144_u32)
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let image = ImageAsset::new(256, 1_024, expected.clone()).unwrap();
        let lease = store
            .store_image(PathBuf::from("shared.png"), &image)
            .unwrap();
        let first_store = store.clone();
        let first_lease = lease.clone();
        let first = std::thread::spawn(move || {
            first_store
                .restore_image(&first_lease, &AtomicBool::new(false))
                .unwrap()
                .rgba
        });
        let second = std::thread::spawn(move || {
            store
                .restore_image(&lease, &AtomicBool::new(false))
                .unwrap()
                .rgba
        });

        assert_eq!(first.join().unwrap(), expected);
        assert_eq!(second.join().unwrap(), expected);
    }

    #[test]
    fn deferred_values_are_destroyed_by_the_cleanup_worker() {
        let store = ResourceBlobStore::new().unwrap();
        let ui_thread = std::thread::current().id();
        let (tx, rx) = mpsc::channel();

        store.defer_drop(DropThread(tx));

        let drop_thread = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_ne!(drop_thread, ui_thread);
    }
}
