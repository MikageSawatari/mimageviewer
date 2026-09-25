//! Paths that may be deleted or published by a conversion.
use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

fn normalized(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut existing = lexical.as_path();
    let mut suffix = Vec::<OsString>::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
    }
    let mut resolved = fs::canonicalize(existing).map_err(|e| e.to_string())?;
    for name in suffix.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn same_or_under(path: &Path, root: &Path) -> bool {
    let path: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let root: Vec<_> = root
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    path.len() >= root.len() && path.iter().zip(&root).all(|(a, b)| a == b)
}

pub fn validate_convert_paths(
    input: &Path,
    out: &Path,
    work: &Path,
    user_data: &Path,
) -> Result<(), String> {
    let input = normalized(input)?;
    let out = normalized(out)?;
    let work = normalized(work)?;
    let user_data = normalized(user_data)?;
    let out_dir = out.parent().ok_or("output has no parent directory")?;
    if same_or_under(out_dir, &user_data) || same_or_under(&out, &user_data) {
        return Err("user data directory contains the output".into());
    }
    if same_or_under(&input, &user_data) {
        return Err("user data directory contains the input".into());
    }
    if same_or_under(&work, &user_data) {
        return Err("user data directory contains the work directory".into());
    }
    Ok(())
}

pub fn output_temp_path(out: &Path, run_id: &str, sequence: usize) -> Result<PathBuf, String> {
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = out
        .file_name()
        .ok_or("output has no file name")?
        .to_string_lossy();
    Ok(parent.join(format!("{name}.tmp-{run_id}-{sequence}")))
}

pub struct OutputTemp {
    path: PathBuf,
}

impl OutputTemp {
    pub fn new(out: &Path, run_id: &str, sequence: usize) -> Result<Self, String> {
        let path = output_temp_path(out, run_id, sequence)?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        Ok(Self { path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn publish(&self, out: &Path) -> Result<(), String> {
        fs::rename(&self.path, out).map_err(|e| e.to_string())
    }
}

impl Drop for OutputTemp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("epub-pdf-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn output_temp_is_sibling_and_removed_on_failure() {
        let root = test_root("path-test");
        let out = root.join("book.pdf.part");
        {
            let temp = OutputTemp::new(&out, "run", 2).unwrap();
            assert_eq!(temp.path().parent(), out.parent());
            assert_eq!(temp.path().file_name().unwrap(), "book.pdf.part.tmp-run-2");
            fs::write(temp.path(), b"partial").unwrap();
        }
        assert!(!output_temp_path(&out, "run", 2).unwrap().exists());
        {
            let temp = OutputTemp::new(&out, "run", 3).unwrap();
            fs::write(temp.path(), b"complete").unwrap();
            temp.publish(&out).unwrap();
        }
        assert_eq!(fs::read(&out).unwrap(), b"complete");
        assert_eq!(
            output_temp_path(Path::new("book.pdf"), "run", 1).unwrap(),
            Path::new(".").join("book.pdf.tmp-run-1")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_user_data_ancestors() {
        let root = test_root("validation");
        let input = root.join("input/book.epub");
        let out = root.join("output/book.pdf.part");
        let work = root.join("work");
        assert!(validate_convert_paths(&input, &out, &work, &root).is_err());
        assert!(validate_convert_paths(&input, &out, &work, &root.join("input")).is_err());
        assert!(
            validate_convert_paths(&input, &out, &work, &root.join("output/../input")).is_err()
        );
        assert!(validate_convert_paths(&input, &out, &work, &root.join("output")).is_err());
        assert!(validate_convert_paths(&input, &out, &work, &root.join("work")).is_err());
        assert!(validate_convert_paths(&input, &out, &work, &root.join("work/user-data")).is_ok());
        assert!(
            validate_convert_paths(
                &input,
                &root.join("work/user-data/book.pdf"),
                &work,
                &root.join("work/user-data")
            )
            .is_err()
        );
    }
}
