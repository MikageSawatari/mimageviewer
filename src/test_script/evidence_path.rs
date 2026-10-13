//! One path contract for screenshot and release-check evidence.
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const REPARSE_POINT: u32 = 0x400;

fn relative_at_depth<'a>(
    root: &Path,
    directory: &'a Path,
    depth: usize,
) -> Result<&'a Path, String> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err("evidence directory must be absolute without traversal".into());
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| format!("evidence directory must be below {}", root.display()))?;
    if relative.components().count() != depth
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "evidence directory must be exactly {depth} levels below {}",
            root.display()
        ));
    }
    Ok(relative)
}

/// Release sessions are exactly target/ui-smoke/runs/<run-id>/<case>.
/// The fixture belongs to that run, never to a caller-selected ancestor.
pub(super) fn release_fixture(target: &Path, directory: &Path) -> Result<PathBuf, String> {
    let root = target.join("ui-smoke").join("runs");
    let relative = relative_at_depth(&root, directory, 2)?;
    Ok(root
        .join(relative.components().next().unwrap())
        .join("fixture"))
}

fn check_directory_attributes(path: &Path, is_dir: bool, attributes: u32) -> Result<(), String> {
    if !is_dir || attributes & REPARSE_POINT != 0 {
        return Err(format!(
            "refusing non-directory or reparse point: {}",
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn no_reparse_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    check_directory_attributes(path, metadata.is_dir(), metadata.file_attributes())
}

pub(super) fn validate_directory(repository: &Path, directory: &Path) -> Result<PathBuf, String> {
    validate_directory_with(repository, directory, no_reparse_directory)
}

fn validate_layout(repository: &Path, directory: &Path) -> Result<(), String> {
    let target = repository.join("target");
    let legacy = target.join("ui-smoke-runs");
    if directory.starts_with(&legacy) {
        relative_at_depth(&legacy, directory, 1)?;
    } else {
        release_fixture(&target, directory)?;
    }
    Ok(())
}

fn validate_directory_with(
    repository: &Path,
    directory: &Path,
    inspect: impl Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    validate_layout(repository, directory)?;
    // Inspect before canonicalization: resolving a junction would hide it.
    // Include the intermediate run, ui-smoke, target and repository ancestors.
    for ancestor in directory.ancestors() {
        inspect(ancestor)?;
    }
    let repository = fs::canonicalize(repository)
        .map_err(|error| format!("cannot resolve evidence repository: {error}"))?;
    let directory = fs::canonicalize(directory)
        .map_err(|error| format!("cannot resolve evidence directory: {error}"))?;
    // Win32 aliases (e.g. trailing dots/spaces) must not change the root/depth.
    validate_layout(&repository, &directory)?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_release_case_and_legacy_run_only_at_exact_depth() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let run = target
            .join("ui-smoke")
            .join("runs")
            .join("release-checks-one");
        let case = run.join("seed-profile");
        let legacy = target.join("ui-smoke-runs").join("one-run");
        fs::create_dir_all(case.join("nested")).unwrap();
        fs::create_dir_all(legacy.join("nested")).unwrap();
        fs::create_dir_all(
            target
                .join("ui-smoke")
                .join("runs-other")
                .join("run")
                .join("case"),
        )
        .unwrap();
        for accepted in [&case, &legacy] {
            assert_eq!(
                validate_directory(temp.path(), accepted).unwrap(),
                fs::canonicalize(accepted).unwrap()
            );
        }
        assert_eq!(
            release_fixture(&target, &case).unwrap(),
            run.join("fixture")
        );
        for refused in [
            target.clone(),
            run.parent().unwrap().to_owned(),
            run.clone(),
            case.join("nested"),
            legacy.parent().unwrap().to_owned(),
            legacy.join("nested"),
            target
                .join("ui-smoke")
                .join("runs-other")
                .join("run")
                .join("case"),
            run.join("..")
                .join("release-checks-one")
                .join("seed-profile"),
            legacy.join("..").join("one-run"),
            run.join(". "),
            run.join(".. "),
            legacy.parent().unwrap().join(". "),
            legacy.parent().unwrap().join(".. "),
            PathBuf::from("target/ui-smoke/runs/run/case"),
        ] {
            assert!(
                validate_directory(temp.path(), &refused).is_err(),
                "accepted {}",
                refused.display()
            );
        }
        let other = tempfile::tempdir().unwrap();
        let foreign = other.path().join("target/ui-smoke/runs/run/case");
        fs::create_dir_all(&foreign).unwrap();
        assert!(validate_directory(temp.path(), &foreign).is_err());
        let file = run.join("file-case");
        fs::write(&file, "not a directory").unwrap();
        assert!(validate_directory(temp.path(), &file).is_err());
    }

    #[test]
    fn rejects_reparse_at_every_ancestor_before_resolving() {
        let temp = tempfile::tempdir().unwrap();
        let case = temp.path().join("target/ui-smoke/runs/run/case");
        fs::create_dir_all(&case).unwrap();
        // Feed the same Windows attribute check through the production ancestor walk,
        // without creating filesystem links or requiring symlink privileges.
        for reparse in case.ancestors() {
            let result = validate_directory_with(temp.path(), &case, |path| {
                check_directory_attributes(
                    path,
                    true,
                    if path == reparse { REPARSE_POINT } else { 0 },
                )
            });
            assert!(result.unwrap_err().contains("reparse point"));
        }
    }
}
