//! Worker-only, lossless image-folder ZIP preparation and output.
//! Publication is deliberately separate from copying: the process operation owner
//! fences metadata writers and validates its collection plan before claiming commit.
use std::collections::HashSet;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::SystemTime;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    identity: (u64, u64),
    length: u64,
    modified: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageMember {
    pub path: PathBuf,
    pub entry: String,
    stamp: FileStamp,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageFolderManifest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub members: Vec<ImageMember>,
    pub total_bytes: u64,
    pub auxiliary_files: usize,
    directory_identity: (u64, u64),
    compiled_root: Option<PathBuf>,
}

/// A single atomic ordering boundary for cancellation versus publication.
/// 0 = cancellable, 1 = cancelled, 2 = committing. No post-commit cancellation.
#[derive(Default)]
pub(crate) struct ConversionControl(AtomicU8);

impl ConversionControl {
    pub fn cancel(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub fn can_cancel(&self) -> bool {
        self.0.load(Ordering::Acquire) != 2
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    pub fn claim_commit(&self) -> Result<()> {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "変換を取り消しました。".to_owned())
    }
    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err("変換を取り消しました。".into())
        } else {
            Ok(())
        }
    }
}

fn io_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn regular_metadata(path: &Path, directory: bool) -> Result<Metadata> {
    let metadata = std::fs::symlink_metadata(path).map_err(io_error)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("リンク先のフォルダや画像は ZIP に変換できません。".into());
        }
    }
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err("通常のフォルダと画像ファイルだけを ZIP に変換できます。".into());
    }
    Ok(metadata)
}

fn read_handle(path: &Path, directory: bool) -> Result<File> {
    regular_metadata(path, directory)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Readers may coexist. Writers and delete/rename may not replace this member.
        options.share_mode(1);
        options.custom_flags(0x0020_0000 | if directory { 0x0200_0000 } else { 0 });
    }
    options.open(path).map_err(io_error)
}

fn identity(file: &File) -> Result<(u64, u64)> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(io_error)?;
        if info.dwFileAttributes & 0x400 != 0 {
            return Err("リンク先は変換できません。".into());
        }
        Ok((
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().map_err(io_error)?;
        Ok((meta.dev(), meta.ino()))
    }
}

fn stamp(file: &File) -> Result<FileStamp> {
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err("画像が通常のファイルではありません。".into());
    }
    Ok(FileStamp {
        identity: identity(file)?,
        length: metadata.len(),
        modified: metadata.modified().map_err(io_error)?,
    })
}

pub(crate) fn destination_for(source: &Path) -> Result<PathBuf> {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("フォルダ名を ZIP の名前にできません。")?;
    let parent = source
        .parent()
        .ok_or("このフォルダは ZIP に変換できません。")?;
    if name.is_empty() || parent.as_os_str().is_empty() {
        return Err("このフォルダは ZIP に変換できません。".into());
    }
    Ok(parent.join(format!("{name}.zip")))
}

pub(crate) fn check_destination(destination: &Path) -> Result<()> {
    let expected = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("作成先の名前が不正です。")?
        .to_lowercase();
    for entry in
        std::fs::read_dir(destination.parent().ok_or("作成先が不正です。")?).map_err(io_error)?
    {
        let entry = entry.map_err(io_error)?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.to_lowercase() == expected)
        {
            return Err("同じ名前の ZIP がすでにあるため、変換できません。既存のファイルは変更していません。".into());
        }
    }
    Ok(())
}

pub(crate) fn prepare_manifest(
    source: &Path,
    compiled_root: Option<&Path>,
    control: &ConversionControl,
) -> Result<ImageFolderManifest> {
    control.check()?;
    if source.to_str().is_none() {
        return Err("フォルダの場所を ZIP の登録に使えません。".into());
    }
    if compiled_root.is_some_and(|root| crate::books::is_direct_book_folder(root, source)) {
        return Err("製本した本のフォルダは、この操作では ZIP に変換できません。".into());
    }
    let directory = read_handle(source, true)?;
    let directory_identity = identity(&directory)?;
    let destination = destination_for(source)?;
    check_destination(&destination)?;
    let mut members = Vec::new();
    let mut unique = HashSet::new();
    let mut auxiliary_files = 0usize;
    let mut total_bytes = 0u64;
    for entry in std::fs::read_dir(source).map_err(io_error)? {
        control.check()?;
        let entry = entry.map_err(io_error)?;
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let metadata = regular_metadata(&path, false)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "ファイル名を ZIP に保存できません。")?;
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();
        if crate::folder_tree::is_apple_double(&path) {
            auxiliary_files += 1;
            continue;
        }
        match crate::file_type_visibility::category_for_extension(&extension) {
            Some(
                crate::file_type_visibility::FileTypeCategory::Image
                | crate::file_type_visibility::FileTypeCategory::Raw,
            ) => {}
            Some(_) => {
                return Err(
                    "画像以外の本や動画・音声があるフォルダは ZIP に変換できません。".into(),
                );
            }
            None => {
                auxiliary_files += 1;
                continue;
            }
        }
        validate_member_name(&name, &mut unique)?;
        let file = read_handle(&path, false)?;
        let stamp = stamp(&file)?;
        if metadata.len() != stamp.length {
            return Err("変換の準備中に画像が変更されました。".into());
        }
        total_bytes = total_bytes
            .checked_add(stamp.length)
            .ok_or("画像の合計サイズが大きすぎます。")?;
        members.push(ImageMember {
            path,
            entry: name,
            stamp,
        });
    }
    if !crate::file_type_visibility::is_image_only_book_contents(
        false,
        members
            .iter()
            .map(|_| crate::file_type_visibility::FileTypeCategory::Image),
    ) {
        return Err("画像がないフォルダは ZIP に変換できません。".into());
    }
    members.sort_by_cached_key(|member| crate::filename_sort::FileNameSortKey::new(&member.entry));
    if identity(&read_handle(source, true)?)? != directory_identity {
        return Err("変換の準備中にフォルダが変更されました。".into());
    }
    Ok(ImageFolderManifest {
        source: source.to_owned(),
        destination,
        members,
        total_bytes,
        auxiliary_files,
        directory_identity,
        compiled_root: compiled_root.map(Path::to_owned),
    })
}

/// Nonblocking preparation boundary for the process modal owner. Completion is
/// enqueued before waking the display-independent poll; no user input is needed.
pub(crate) fn spawn_prepare(
    source: PathBuf,
    compiled_root: Option<PathBuf>,
    control: Arc<ConversionControl>,
    wake: impl FnOnce() + Send + 'static,
) -> Result<mpsc::Receiver<Result<ImageFolderManifest>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("image-folder-zip-prepare".into())
        .spawn(move || {
            let result = prepare_manifest(&source, compiled_root.as_deref(), &control);
            let _ = tx.send(result);
            wake();
        })
        .map_err(io_error)?;
    Ok(rx)
}

fn validate_member_name(name: &str, unique: &mut HashSet<String>) -> Result<()> {
    crate::book_composition::PageDomain::ImageFolder
        .validate_page(&crate::book_composition::CompositionPageRef::RelativePath {
            path: name.to_owned(),
        })
        .map_err(io_error)?;
    crate::book_composition::PageDomain::ArchiveLevel {
        prefix: String::new(),
    }
    .validate_page(&crate::book_composition::CompositionPageRef::ArchiveEntry {
        entry_name: name.to_owned(),
    })
    .map_err(io_error)?;
    if name.len() > u16::MAX as usize
        || !unique.insert(crate::book_bookmarks::normalize_page_path(name))
    {
        return Err("画像の名前を区別して ZIP に保存できません。".into());
    }
    Ok(())
}

impl ImageFolderManifest {
    pub(crate) fn revalidate_source_directory(&self) -> Result<()> {
        if identity(&read_handle(&self.source, true)?)? != self.directory_identity {
            return Err(
                "変換中に元のフォルダが置き換えられました。元のフォルダは残しています。".into(),
            );
        }
        Ok(())
    }
    pub fn revalidate(&self, control: &ConversionControl) -> Result<()> {
        // DAT saves may change the directory mtime; only its identity and image inventory matter.
        let current = prepare_manifest(&self.source, self.compiled_root.as_deref(), control)?;
        if self.directory_identity != current.directory_identity || self.members != current.members
        {
            return Err(
                "変換中にフォルダや画像が変更されました。元のフォルダは残っています。".into(),
            );
        }
        Ok(())
    }
}

/// Owns only the unique file created by this operation, never a fixed .zip.part.
pub(crate) struct PreparedZip {
    temporary: PathBuf,
    destination: PathBuf,
}

impl Drop for PreparedZip {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.temporary)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "image folder ZIP temporary cleanup {}: {error}",
                self.temporary.display()
            );
        }
    }
}

impl PreparedZip {
    /// Caller must validate metadata/actor guards before making this final claim.
    pub fn publish(
        self,
        manifest: &ImageFolderManifest,
        control: &ConversionControl,
    ) -> Result<PublishedZip> {
        if self.destination != manifest.destination {
            return Err("作成先の確認に失敗しました。".into());
        }
        // Metadata/actor preflight may take time after ZIP verification.
        manifest.revalidate(control)?;
        check_destination(&self.destination)?;
        let expected_identity = identity(&File::open(&self.temporary).map_err(io_error)?)?;
        control.claim_commit()?;
        crate::archive_converter::replace_file_atomic(&self.temporary, &self.destination, true)
            .map_err(io_error)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Keep the output's identity through metadata adoption. DELETE access
            // permits cleanup by this exact handle without a pathname race.
            options
                .access_mode(0x8000_0000 | 0x0001_0000)
                .share_mode(1)
                .custom_flags(0x0020_0000);
        }
        let file = options.open(&self.destination).map_err(io_error)?;
        if identity(&file)? != expected_identity {
            return Err(
                "作成した ZIP が別のファイルに置き換えられました。登録や編集は複製していません。"
                    .into(),
            );
        }
        Ok(PublishedZip {
            path: self.destination.clone(),
            file,
        })
    }
}

pub(crate) struct PublishedZip {
    path: PathBuf,
    file: File,
}

impl PublishedZip {
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Called only on metadata-copy failure. Successful drop keeps the ZIP.
    pub fn remove_owned(self) -> Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                SetFileInformationByHandle(
                    HANDLE(self.file.as_raw_handle()),
                    FileDispositionInfo,
                    (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&disposition) as u32,
                )
            }
            .map_err(io_error)
        }
        #[cfg(not(windows))]
        {
            if identity(&File::open(&self.path).map_err(io_error)?)? != identity(&self.file)? {
                return Err("ZIP が別のファイルに置き換えられたため削除しませんでした。".into());
            }
            std::fs::remove_file(&self.path).map_err(io_error)
        }
    }
}

pub(crate) fn write_zip(
    manifest: &ImageFolderManifest,
    control: &ConversionControl,
    mut progress: impl FnMut(usize, u64),
) -> Result<PreparedZip> {
    manifest.revalidate(control)?;
    let temporary = manifest
        .destination
        .parent()
        .unwrap()
        .join(format!(".miv-image-zip-{}.tmp", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    let prepared = PreparedZip {
        temporary,
        destination: manifest.destination.clone(),
    };
    let mut writer = zip::ZipWriter::new(file);
    let mut buffer = vec![0u8; 256 * 1024];
    let mut written = 0u64;
    for (index, member) in manifest.members.iter().enumerate() {
        control.check()?;
        let mut file = read_handle(&member.path, false)?;
        if stamp(&file)? != member.stamp {
            return Err("変換中に画像が変更されました。".into());
        }
        writer
            .start_file(&member.entry, crate::archive_converter::store_options())
            .map_err(io_error)?;
        loop {
            control.check()?;
            let read = file.read(&mut buffer).map_err(io_error)?;
            if read == 0 {
                break;
            }
            writer.write_all(&buffer[..read]).map_err(io_error)?;
            written += read as u64;
        }
        if stamp(&file)? != member.stamp {
            return Err("変換中に画像が変更されました。".into());
        }
        progress(index + 1, written);
    }
    let mut file = writer.finish().map_err(io_error)?;
    file.flush().map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    let mut archive = zip::ZipArchive::new(File::open(&prepared.temporary).map_err(io_error)?)
        .map_err(io_error)?;
    if archive.len() != manifest.members.len() {
        return Err("ZIP の画像数を確認できませんでした。".into());
    }
    for (index, member) in manifest.members.iter().enumerate() {
        control.check()?;
        let mut entry = archive.by_index(index).map_err(io_error)?;
        if entry.name() != member.entry
            || entry.size() != member.stamp.length
            || entry.compression() != zip::CompressionMethod::Stored
        {
            return Err("ZIP の画像を確認できませんでした。".into());
        }
        // ZipFile's reader validates CRC when EOF is reached.
        loop {
            control.check()?;
            if entry.read(&mut buffer).map_err(io_error)? == 0 {
                break;
            }
        }
    }
    drop(archive);
    manifest.revalidate(control)?;
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("book.pdf");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("002.jpg"), b"second bytes").unwrap();
        std::fs::write(folder.join("001.PNG"), b"first bytes").unwrap();
        (temp, folder)
    }
    #[test]
    fn stored_output_preserves_all_bytes_literal_names_and_source() {
        let (_temp, folder) = fixture();
        std::fs::write(folder.join("notes.txt"), b"keep here").unwrap();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        assert_eq!(manifest.auxiliary_files, 1);
        assert_eq!(manifest.destination.file_name().unwrap(), "book.pdf.zip");
        let destination = write_zip(&manifest, &control, |_, _| {})
            .unwrap()
            .publish(&manifest, &control)
            .unwrap();
        let mut zip = zip::ZipArchive::new(File::open(destination.path()).unwrap()).unwrap();
        assert_eq!(zip.len(), 2);
        for (index, member) in manifest.members.iter().enumerate() {
            let mut entry = zip.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(entry.name(), member.entry);
            assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
            assert_eq!(bytes, std::fs::read(&member.path).unwrap());
        }
        assert!(folder.join("notes.txt").exists());
    }
    #[test]
    fn reject_empty_mixed_and_nested_without_filtered_view_shortcuts() {
        let temp = tempfile::tempdir().unwrap();
        assert!(prepare_manifest(temp.path(), None, &ConversionControl::default()).is_err());
        for name in [
            "movie.mp4",
            "book.zip",
            "book.pdf",
            "book.epub",
            "book.rar",
            "song.mp3",
        ] {
            let (_temp, folder) = fixture();
            std::fs::write(folder.join(name), b"other").unwrap();
            assert!(
                prepare_manifest(&folder, None, &ConversionControl::default()).is_err(),
                "{name}"
            );
        }
        let (_temp, folder) = fixture();
        std::fs::create_dir(folder.join("nested")).unwrap();
        assert!(prepare_manifest(&folder, None, &ConversionControl::default()).is_err());
    }
    #[test]
    fn existing_destination_and_publication_race_keep_external_file() {
        let (_temp, folder) = fixture();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        let prepared = write_zip(&manifest, &control, |_, _| {}).unwrap();
        std::fs::write(&manifest.destination, b"external").unwrap();
        assert!(prepared.publish(&manifest, &control).is_err());
        assert_eq!(std::fs::read(&manifest.destination).unwrap(), b"external");
        assert!(prepare_manifest(&folder, None, &control).is_err());
    }
    #[test]
    fn changes_and_cancel_clean_only_owned_temporary_file() {
        let (_temp, folder) = fixture();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        let unrelated = folder.parent().unwrap().join("book.pdf.zip.part");
        std::fs::write(&unrelated, b"unrelated").unwrap();
        let result = write_zip(&manifest, &control, |_, _| {
            control.cancel();
        });
        assert!(result.is_err());
        assert!(!manifest.destination.exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"unrelated");
        assert!(
            std::fs::read_dir(folder.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".miv-image-zip"))
        );
        let control = ConversionControl::default();
        std::fs::write(folder.join("003.jpg"), b"new").unwrap();
        assert!(manifest.revalidate(&control).is_err());
    }
    #[test]
    fn commit_claim_and_cancel_are_exclusive() {
        let control = ConversionControl::default();
        assert!(control.cancel());
        assert!(control.claim_commit().is_err());
        let control = ConversionControl::default();
        control.claim_commit().unwrap();
        assert!(!control.cancel());
    }
    #[test]
    fn dat_save_does_not_invalidate_image_manifest() {
        let (_temp, folder) = fixture();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        std::fs::write(folder.join("mimageviewer.dat"), b"{}").unwrap();
        manifest.revalidate(&control).unwrap();
    }

    #[test]
    fn internal_bundle_and_apple_double_are_not_images_or_nested_books() {
        let (_temp, folder) = fixture();
        std::fs::create_dir(folder.join("mimageviewer.meta.miv")).unwrap();
        std::fs::write(folder.join("._001.jpg"), b"apple metadata").unwrap();
        let manifest = prepare_manifest(&folder, None, &ConversionControl::default()).unwrap();
        assert_eq!(manifest.members.len(), 2);
        assert_eq!(manifest.auxiliary_files, 1);
    }

    #[test]
    fn compiled_books_are_rejected_without_affecting_ordinary_folders() {
        let (temp, folder) = fixture();
        assert!(
            prepare_manifest(&folder, Some(temp.path()), &ConversionControl::default()).is_err()
        );
        assert!(
            prepare_manifest(
                &folder,
                Some(&temp.path().join("other")),
                &ConversionControl::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn unsafe_and_case_colliding_member_names_are_refused_without_renaming() {
        for name in [
            "../a.jpg", "/a.jpg", "a\\b.jpg", "a//b.jpg", "nul.jpg", "a.jpg ", "c:a.jpg",
        ] {
            assert!(
                validate_member_name(name, &mut HashSet::new()).is_err(),
                "{name}"
            );
        }
        let mut unique = HashSet::new();
        validate_member_name("A.jpg", &mut unique).unwrap();
        assert!(validate_member_name("a.JPG", &mut unique).is_err());
    }

    #[test]
    fn publication_rechecks_changes_after_zip_verification() {
        let (_temp, folder) = fixture();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        let prepared = write_zip(&manifest, &control, |_, _| {}).unwrap();
        std::fs::write(folder.join("new.jpg"), b"new").unwrap();
        assert!(prepared.publish(&manifest, &control).is_err());
        assert!(!manifest.destination.exists());
    }

    #[test]
    fn metadata_failure_removes_only_owned_published_zip() {
        let (_temp, folder) = fixture();
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        let published = write_zip(&manifest, &control, |_, _| {})
            .unwrap()
            .publish(&manifest, &control)
            .unwrap();
        #[cfg(windows)]
        {
            // The publication owner denies external delete/replacement until adoption.
            assert!(std::fs::remove_file(published.path()).is_err());
            assert!(std::fs::write(published.path(), b"replacement").is_err());
        }
        published.remove_owned().unwrap();
        assert!(!manifest.destination.exists());
        assert_eq!(
            std::fs::read(folder.join("001.PNG")).unwrap(),
            b"first bytes"
        );
    }

    #[cfg(windows)]
    #[test]
    fn copy_handle_blocks_source_write_and_delete_only_while_copying() {
        let (_temp, folder) = fixture();
        let image = folder.join("001.PNG");
        let handle = read_handle(&image, false).unwrap();
        assert!(std::fs::write(&image, b"changed").is_err());
        assert!(std::fs::remove_file(&image).is_err());
        drop(handle);
        std::fs::write(&image, b"changed").unwrap();
    }

    #[test]
    fn preparation_worker_wakes_after_terminal_reply_without_input() {
        let (_temp, folder) = fixture();
        let (wake_tx, wake_rx) = mpsc::channel();
        let rx = spawn_prepare(
            folder.clone(),
            None,
            Arc::new(ConversionControl::default()),
            move || {
                wake_tx.send(()).unwrap();
            },
        )
        .unwrap();
        wake_rx.recv().unwrap();
        let manifest = rx.try_recv().unwrap().unwrap();
        assert_eq!(manifest.source, folder);
        assert_eq!(manifest.members.len(), 2);
    }

    #[test]
    fn full_inventory_keeps_raw_animation_and_duplicate_extension_images() {
        let (_temp, folder) = fixture();
        for name in ["001.jpg", "raw.ARW", "animated.gif", ".hidden.jpg"] {
            std::fs::write(folder.join(name), name.as_bytes()).unwrap();
        }
        let control = ConversionControl::default();
        let manifest = prepare_manifest(&folder, None, &control).unwrap();
        assert_eq!(manifest.members.len(), 6);
        let output = write_zip(&manifest, &control, |_, _| {})
            .unwrap()
            .publish(&manifest, &control)
            .unwrap();
        let mut archive = zip::ZipArchive::new(File::open(output.path()).unwrap()).unwrap();
        assert_eq!(archive.len(), 6);
        for name in ["001.jpg", "raw.ARW", "animated.gif", ".hidden.jpg"] {
            let mut bytes = Vec::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(bytes, name.as_bytes());
        }
    }
}
