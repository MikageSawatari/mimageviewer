use std::path::{Path, PathBuf};

pub const WORK_DIR_SUFFIX: &str = "miv.work";
pub const MANIFEST_FILE_NAME: &str = "job.miv-upscale.json";
pub const SEGMENTS_DIR_NAME: &str = "segments";

pub fn work_dir_for(source_path: &Path) -> PathBuf {
    source_path.with_file_name(format!("{}.{}", source_stem(source_path), WORK_DIR_SUFFIX))
}

pub fn manifest_path_for(source_path: &Path) -> PathBuf {
    work_dir_for(source_path).join(MANIFEST_FILE_NAME)
}

pub fn segments_dir_for(work_dir: &Path) -> PathBuf {
    work_dir.join(SEGMENTS_DIR_NAME)
}

pub fn segment_file_name(index: u32) -> String {
    format!("{index:06}.mkv")
}

pub fn segment_part_file_name(index: u32) -> String {
    format!("{}.part", segment_file_name(index))
}

pub fn segment_path(work_dir: &Path, index: u32) -> PathBuf {
    segments_dir_for(work_dir).join(segment_file_name(index))
}

pub fn segment_part_path(work_dir: &Path, index: u32) -> PathBuf {
    segments_dir_for(work_dir).join(segment_part_file_name(index))
}

pub fn worker_segment_part_path(work_dir: &Path, index: u32, worker_id: &str) -> PathBuf {
    segments_dir_for(work_dir).join(format!("{}.part.{worker_id}", segment_file_name(index)))
}

pub fn final_output_path_for(source_path: &Path) -> PathBuf {
    source_path.with_file_name(format!("{}.miv.mkv", source_stem(source_path)))
}

pub fn final_part_path_for(source_path: &Path) -> PathBuf {
    source_path.with_file_name(format!("{}.miv.mkv.part", source_stem(source_path)))
}

pub fn final_sidecar_path_for(source_path: &Path) -> PathBuf {
    source_path.with_file_name(format!("{}.miv.json", source_stem(source_path)))
}

pub fn internal_final_part_path(work_dir: &Path) -> PathBuf {
    work_dir.join("final.mkv.part")
}

pub fn is_path_inside(parent: &Path, child: &Path) -> bool {
    if let (Ok(parent), Ok(child)) = (parent.canonicalize(), child.canonicalize()) {
        return child.starts_with(parent);
    }

    if has_parent_dir(parent) || has_parent_dir(child) {
        return false;
    }

    let parent_components = normalized_components(parent);
    let child_components = normalized_components(child);
    child_components.len() >= parent_components.len()
        && parent_components
            .iter()
            .zip(child_components.iter())
            .all(|(parent, child)| path_component_eq(parent, child))
}

pub fn has_work_dir_suffix(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".miv.work"))
}

pub fn is_work_dir_name(path: &Path) -> bool {
    has_work_dir_suffix(path)
}

fn source_stem(source_path: &Path) -> String {
    source_path
        .file_stem()
        .or_else(|| source_path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "video".to_owned())
}

fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn normalized_components(path: &Path) -> Vec<String> {
    path.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(windows)]
fn path_component_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(not(windows))]
fn path_component_eq(a: &str, b: &str) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_expected_work_and_output_paths() {
        let source = Path::new(r"C:\videos\movie.mp4");

        assert_eq!(
            work_dir_for(source),
            PathBuf::from(r"C:\videos\movie.miv.work")
        );
        assert_eq!(
            manifest_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.work\job.miv-upscale.json")
        );
        assert_eq!(
            final_output_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.mkv")
        );
        assert_eq!(
            final_part_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.mkv.part")
        );
        assert_eq!(
            final_sidecar_path_for(source),
            PathBuf::from(r"C:\videos\movie.miv.json")
        );
    }

    #[test]
    fn segment_paths_are_zero_padded() {
        let work = Path::new(r"C:\videos\movie.miv.work");

        assert_eq!(segment_file_name(7), "000007.mkv");
        assert_eq!(segment_part_file_name(7), "000007.mkv.part");
        assert_eq!(
            worker_segment_part_path(work, 7, "abc"),
            PathBuf::from(r"C:\videos\movie.miv.work\segments\000007.mkv.part.abc")
        );
    }

    #[test]
    fn detects_work_dir_names() {
        assert!(has_work_dir_suffix(Path::new("movie.miv.work")));
        assert!(is_work_dir_name(Path::new("movie.miv.work")));
        assert!(!is_work_dir_name(Path::new("movie.miv.mkv")));
    }

    #[test]
    fn containment_uses_canonical_paths() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        let file = child_dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        assert!(is_path_inside(dir.path(), &file));
    }

    #[test]
    fn containment_allows_missing_child_during_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child").join("missing.part");

        assert!(is_path_inside(dir.path(), &child));
    }

    #[test]
    fn containment_rejects_parent_dir_escape() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("..").join("outside");

        assert!(!is_path_inside(dir.path(), &child));
    }
}
