use std::io;
use std::path::{Component, Path, PathBuf};

pub fn checked_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    let meta = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = meta.file_type().is_symlink();
    if reparse {
        return Err(io::Error::other(format!(
            "reparse point forbidden: {}",
            path.display()
        )));
    }
    Ok(meta)
}

pub fn exists_checked(path: &Path) -> io::Result<bool> {
    match checked_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn relative_name(path: &Path) -> io::Result<String> {
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::other("invalid bundle relative path"));
    }
    for component in path.components() {
        let part = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| io::Error::other("bundle path must be UTF-8"))?;
        let base = part.split('.').next().unwrap().trim_end().to_uppercase();
        let numbered_device = ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        if part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || numbered_device
        {
            return Err(io::Error::other("invalid Windows bundle component"));
        }
    }
    let name = path
        .to_str()
        .ok_or_else(|| io::Error::other("bundle path must be UTF-8"))?
        .replace('\\', "/");
    if name.is_empty() || name.contains([':', '\n', '\r', '\t']) {
        return Err(io::Error::other("invalid bundle path characters"));
    }
    Ok(name)
}

/// Sorted inventory, rejecting links at every level before following directories.
pub fn inventory(root: &Path) -> io::Result<Vec<(String, bool, PathBuf)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool, PathBuf)>) -> io::Result<()> {
        if !checked_metadata(dir)?.is_dir() {
            return Err(io::Error::other("bundle directory required"));
        }
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let meta = checked_metadata(&path)?;
            if !meta.is_dir() && !meta.is_file() {
                return Err(io::Error::other("bundle contains non-regular file"));
            }
            out.push((
                relative_name(path.strip_prefix(root).unwrap())?,
                meta.is_dir(),
                path.clone(),
            ));
            if meta.is_dir() {
                walk(root, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    validate_case_unique(out.iter().map(|(name, _, _)| name.as_str()))?;
    Ok(out)
}

fn validate_case_unique<'a>(names: impl IntoIterator<Item = &'a str>) -> io::Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !seen.insert(name.to_lowercase()) {
            return Err(io::Error::other("case-insensitive bundle path collision"));
        }
    }
    Ok(())
}

pub fn remove_tree(root: &Path) -> io::Result<()> {
    match checked_metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(meta) if !meta.is_dir() => return Err(io::Error::other("directory required")),
        Ok(_) => {}
    }
    // Validate the complete tree before any recursive delete; never follow junctions.
    inventory(root)?;
    std::fs::remove_dir_all(root)
}

pub fn remove_owned(root: &Path) -> io::Result<()> {
    match checked_metadata(root) {
        Ok(meta) if meta.is_file() => std::fs::remove_file(root),
        Ok(_) => remove_tree(root),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_ads_and_case_collisions() {
        for path in [
            "../escape",
            "C:/escape",
            "x:stream",
            "./x",
            "x\tname",
            "dir/.. /escape",
            "NUL.txt",
            "COM1",
            "test.",
            "name ",
            "x|name",
        ] {
            assert!(relative_name(Path::new(path)).is_err(), "{path}");
        }
        assert_eq!(
            relative_name(Path::new("Contents/test.txt")).unwrap(),
            "Contents/test.txt"
        );
        assert!(validate_case_unique(["Contents/Test.txt", "contents/test.txt"]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_junction_without_following_or_removing_target() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let root = temp.path().join("bundle");
        std::fs::create_dir(&outside).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(outside.join("keep"), b"safe").unwrap();
        let junction = root.join("junction");
        // mklink /J needs no symlink privilege. Paths come only from TempDir.
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .creation_flags(0x08000000)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(inventory(&root).is_err());
        assert!(remove_tree(&root).is_err());
        assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"safe");
        std::fs::remove_dir(junction).unwrap();
    }
}
