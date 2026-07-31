/// Extracts a `pub const NAME: &str = "...";` value from Rust source.
///
/// The launcher build script uses this to mirror single-instance names from the
/// core executable. Both cooked string literals (`"Global\\..."`) and raw
/// string literals (`r"\\.\pipe\..."`) are supported.
pub(crate) fn extract_const(src: &str, name: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        let prefix = format!("pub const {name}");
        if !line.starts_with(&prefix) {
            continue;
        }
        let value = line.split_once('=')?.1.trim().strip_suffix(';')?.trim();
        return parse_rust_string_literal(value);
    }
    None
}

fn parse_rust_string_literal(value: &str) -> Option<String> {
    parse_raw_string_literal(value).or_else(|| parse_cooked_string_literal(value))
}

fn parse_raw_string_literal(value: &str) -> Option<String> {
    let rest = value.strip_prefix('r')?;
    let hash_count = rest.chars().take_while(|&ch| ch == '#').count();
    let rest = &rest[hash_count..];
    let body = rest.strip_prefix('"')?;
    let terminator = format!("\"{}", "#".repeat(hash_count));
    let end = body.find(&terminator)?;
    Some(body[..end].to_string())
}

fn parse_cooked_string_literal(value: &str) -> Option<String> {
    let mut chars = value.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => {
                let escaped = chars.next()?;
                match escaped {
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '0' => out.push('\0'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::extract_const;

    #[test]
    fn extracts_cooked_string_and_unescapes_backslashes() {
        let src = r#"pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_v1";"#;

        assert_eq!(
            extract_const(src, "MUTEX_NAME").as_deref(),
            Some(r"Global\mImageViewerInstance_v1")
        );
    }

    #[test]
    fn extracts_raw_string_without_collapsing_pipe_prefix() {
        let src = r#"pub const OPEN_PATH_PIPE_NAME: &str = r"\\.\pipe\mImageViewerOpenPath_v1";"#;

        assert_eq!(
            extract_const(src, "OPEN_PATH_PIPE_NAME").as_deref(),
            Some(r"\\.\pipe\mImageViewerOpenPath_v1")
        );
    }

    #[test]
    fn extracts_hash_raw_string() {
        let src = r##"pub const EXAMPLE: &str = r#"\\.\pipe\name"#;"##;

        assert_eq!(
            extract_const(src, "EXAMPLE").as_deref(),
            Some(r"\\.\pipe\name")
        );
    }

    #[test]
    fn real_core_source_and_installer_keep_legacy_base_names() {
        let core = include_str!("../../../src/single_instance.rs");
        assert_eq!(
            extract_const(core, "MUTEX_NAME").as_deref(),
            Some(r"Global\mImageViewerInstance_v1")
        );
        assert_eq!(
            extract_const(core, "ACTIVATE_EVENT_NAME").as_deref(),
            Some(r"Global\mImageViewerActivate_v1")
        );
        assert_eq!(
            extract_const(core, "OPEN_PATH_PIPE_NAME").as_deref(),
            Some(r"\\.\pipe\mImageViewerOpenPath_v1")
        );

        let installer = include_str!("../../../installer/mimageviewer.iss");
        assert!(installer.contains("ShutdownEventName = 'Global\\mImageViewerShutdown_v1';"));
        assert!(installer.contains("AppMutexName = 'Global\\mImageViewerInstance_v1';"));
    }
}
