//! Argument parsing has no filesystem or environment access.
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OpenOnStartup {
    Audio(PathBuf),
    Project(PathBuf),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupArgs {
    pub open: Option<OpenOnStartup>,
    pub data_dir: Option<PathBuf>,
    pub error: Option<String>,
}

pub(crate) fn resolve_path(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if !joined.is_absolute() {
        return Err(format!("絶対パスを解決できません: {}", path.display()));
    }
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    Ok(resolved)
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> StartupArgs {
    let mut parsed = StartupArgs::default();
    let mut args = args.into_iter().peekable();
    let mut errors = Vec::new();
    while let Some(flag) = args.next() {
        if !matches!(flag.to_str(), Some("--audio" | "--project" | "--data-dir")) {
            errors.push(format!("知らない引数です: {}", flag.to_string_lossy()));
            continue;
        }
        let name = flag.to_str().unwrap();
        if args
            .peek()
            .is_none_or(|value| value.is_empty() || value.to_string_lossy().starts_with("--"))
        {
            errors.push(format!("{name} にはパスが必要です"));
            if args.peek().is_some_and(|value| value.is_empty()) {
                args.next();
            }
            continue;
        }
        let path = match resolve_path(cwd, Path::new(&args.next().unwrap())) {
            Ok(path) => path,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        match name {
            "--data-dir" if parsed.data_dir.is_none() => parsed.data_dir = Some(path),
            "--audio" if parsed.open.is_none() => parsed.open = Some(OpenOnStartup::Audio(path)),
            "--project" if parsed.open.is_none() => {
                parsed.open = Some(OpenOnStartup::Project(path))
            }
            "--data-dir" => errors.push("--data-dir は一度だけ指定してください".into()),
            _ => errors.push("--audio または --project はどちらか一度だけ指定してください".into()),
        }
    }
    if !errors.is_empty() {
        parsed.open = None;
        parsed.error = Some(errors.join("\n"));
    }
    parsed
}

pub(crate) enum RuntimeAdmission {
    Ready { _lease: Option<std::fs::File> },
    Blocked(String),
}

impl RuntimeAdmission {
    pub fn acquire(acquire: impl FnOnce() -> std::io::Result<Option<std::fs::File>>) -> Self {
        match acquire() {
            Ok(lease) => Self::Ready { _lease: lease },
            Err(error) => Self::Blocked(format!(
                "runtime の保護を取得できません: {error}\nmIV から音楽ビデオエディタを開き直してください。"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str], cwd: &Path) -> StartupArgs {
        parse_args(args.iter().map(OsString::from), cwd)
    }
    #[test]
    fn p4c_args_valid_modes_and_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        assert_eq!(parse(&[], cwd), StartupArgs::default());
        assert_eq!(
            parse(&["--audio", "./sub/../song.wav"], cwd).open,
            Some(OpenOnStartup::Audio(cwd.join("song.wav")))
        );
        assert_eq!(
            parse(&["--project", "work.json"], cwd).open,
            Some(OpenOnStartup::Project(cwd.join("work.json")))
        );
        assert_eq!(
            parse(&["--data-dir", "data"], cwd).data_dir,
            Some(cwd.join("data"))
        );
        for flag in ["--audio", "--project"] {
            let parsed = parse(&["--data-dir", "data", flag, "file"], cwd);
            assert!(parsed.error.is_none());
            assert!(parsed.open.is_some());
        }
        let absolute = cwd.join("日本語.wav");
        assert_eq!(
            parse_args([OsString::from("--audio"), absolute.clone().into()], cwd).open,
            Some(OpenOnStartup::Audio(absolute))
        );
        assert_eq!(
            resolve_path(cwd, Path::new("../../../file"))
                .unwrap()
                .file_name()
                .unwrap(),
            "file"
        );
    }
    #[test]
    fn p4c_args_errors_clear_open_and_preserve_explicit_data_dir() {
        let temp = tempfile::tempdir().unwrap();
        for args in [
            vec!["--audio", "a", "--project", "p"],
            vec!["--project", "p", "--audio", "a"],
            vec!["--audio", "a", "--audio", "b"],
            vec!["--project", "a", "--project", "b"],
            vec!["--data-dir", "a", "--data-dir", "b"],
            vec!["--audio"],
            vec!["--project"],
            vec!["--data-dir"],
            vec!["--audio", ""],
            vec!["--project", "--data-dir", "data"],
            vec!["--wat"],
            vec!["file.json"],
            vec!["--audio", "a", "extra"],
        ] {
            let parsed = parse(&args, temp.path());
            assert!(parsed.error.is_some(), "{args:?}");
            assert!(parsed.open.is_none(), "{args:?}");
        }
        let parsed = parse(&["--audio", "--data-dir", "data"], temp.path());
        assert_eq!(parsed.data_dir, Some(temp.path().join("data")));
        assert!(parsed.error.unwrap().contains("--audio"));
    }
}
