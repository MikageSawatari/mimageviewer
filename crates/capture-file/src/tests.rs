use super::*;

fn timestamp() -> CaptureTimestamp {
    CaptureTimestamp {
        stamp: "20261002-153012-345".into(),
        month: "2026-10".into(),
    }
}

#[test]
fn browser818_capture_file_names_match_released_single_and_batch_outputs() {
    assert_eq!(
        capture_filename(&timestamp(), None, 1, 1, 1, "png"),
        "20261002-153012-345_01.png"
    );
    assert_eq!(
        capture_filename(&timestamp(), Some("my_site.example.com"), 1, 125, 1, "png"),
        "20261002-153012-345-my-site.example.com_001.png"
    );
    assert_eq!(
        capture_filename(
            &timestamp(),
            Some("my_site.example.com"),
            125,
            125,
            2,
            "jpg"
        ),
        "20261002-153012-345-2-my-site.example.com_125.jpg"
    );
    assert_eq!(
        capture_filename(&timestamp(), Some("a/b:c?d*e\\f\n"), 1, 1, 1, "png"),
        "20261002-153012-345-a-b-c-d-e-f-_01.png"
    );
    assert_eq!(
        capture_filename(&timestamp(), Some("画像.example"), 2, 2, 3, "webp"),
        "20261002-153012-345-3-画像.example_02.webp"
    );
    assert_eq!(
        capture_filename(&timestamp(), Some(""), 1, 0, 0, "png"),
        "20261002-153012-345_01.png"
    );
}

#[test]
fn browser818_capture_file_domain_prefers_page_without_changing_fallback_rules() {
    let mut origin = CaptureOrigin {
        page_url: Some("https://page.example/post".into()),
        image_url: Some("https://image.example/a.png".into()),
    };
    assert_eq!(origin.domain().as_deref(), Some("page.example"));
    origin.page_url = None;
    assert_eq!(origin.domain().as_deref(), Some("image.example"));
    // Preserve the released Option::or behavior: a present invalid page is not
    // replaced by the image URL during domain selection.
    origin.page_url = Some("invalid URL".into());
    assert_eq!(origin.domain(), None);
    assert_eq!(CaptureOrigin::default().domain(), None);
}

#[test]
fn browser818_capture_file_url_sanitization_preserves_unicode_ipv6_and_query() {
    assert_eq!(
        sanitize_url("https://user:pw@例え.テスト/画像?q=値#fragment"),
        Some("https://xn--r8jz45g.xn--zckzah/%E7%94%BB%E5%83%8F?q=%E5%80%A4".into())
    );
    assert_eq!(
        sanitize_url("https://user:pw@[2001:db8::1]:8443/a#fragment"),
        Some("https://[2001:db8::1]:8443/a".into())
    );
    assert_eq!(
        CaptureOrigin::from_browser_urls("https://[2001:db8::1]/post", "https://cdn.example/a")
            .domain()
            .as_deref(),
        Some("[2001:db8::1]")
    );
    assert_eq!(
        sanitize_url("https://example.com/a%0D%0AZoneId=0"),
        Some("https://example.com/a%0D%0AZoneId=0".into())
    );
}

#[test]
fn browser818_capture_file_motw_matches_released_bytes_and_uses_final_url() {
    let origin = CaptureOrigin {
        page_url: Some("https://user:secret@page.example.com/path?q=1#fragment".into()),
        image_url: Some("https://img.example.com/a.png#fragment".into()),
    };
    assert_eq!(
        motw_contents(&origin).unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.example.com/path?q=1\r\nHostUrl=https://img.example.com/a.png\r\n"
    );
    let browser = CaptureOrigin::from_browser_urls(
        "https://user:pw@page.example/post#part",
        "https://user:pw@final.example/image.jpg?q=1#part",
    );
    assert_eq!(
        motw_contents(&browser).unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.example/post\r\nHostUrl=https://final.example/image.jpg?q=1\r\n"
    );
    assert_eq!(
        motw_contents(&CaptureOrigin {
            page_url: None,
            image_url: origin.image_url.clone()
        })
        .unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://img.example.com/a.png\r\n"
    );
    assert_eq!(
        motw_contents(&CaptureOrigin {
            page_url: Some("https://page.example/".into()),
            image_url: None
        })
        .unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://page.example/\r\n"
    );
    assert!(motw_contents(&CaptureOrigin::default()).is_none());
}

#[test]
fn browser818_capture_file_rejects_controls_and_non_http_motw_fields() {
    for control in ['\0', '\r', '\n', '\t', '\u{0085}', '\u{007f}'] {
        let value = format!("https://example.com/{control}ZoneId=0");
        assert_eq!(sanitize_url(&value), None);
        assert!(motw_contents(&CaptureOrigin::from_browser_urls(&value, &value)).is_none());
    }
    for value in [
        "file:///secret",
        "data:image/png;base64,AA==",
        "javascript:alert(1)",
        "not a URL",
    ] {
        assert_eq!(sanitize_url(value), None);
    }
    assert_eq!(
        motw_contents(&CaptureOrigin::from_browser_urls(
            "https://example.com/",
            "file:///secret"
        ))
        .unwrap(),
        "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://example.com/\r\n"
    );
}

#[test]
fn browser818_capture_file_current_timestamp_keeps_month_and_milliseconds() {
    let now = CaptureTimestamp::now();
    assert_eq!(now.stamp.len(), 19);
    assert_eq!(&now.stamp[8..9], "-");
    assert_eq!(&now.stamp[15..16], "-");
    assert!(
        now.stamp
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 8 || i == 15 || c.is_ascii_digit())
    );
    assert_eq!(
        now.month,
        format!("{}-{}", &now.stamp[..4], &now.stamp[4..6])
    );
    assert!((1..=12).contains(&now.stamp[4..6].parse::<u32>().unwrap()));
}

#[cfg(windows)]
#[test]
fn browser818_capture_file_writes_exact_ads_without_changing_image_bytes() {
    let fixture = tempfile::tempdir().unwrap();
    let image = fixture.path().join("image.png");
    std::fs::write(&image, b"original image bytes").unwrap();
    let origin =
        CaptureOrigin::from_browser_urls("https://page.example/post", "https://final.example/a");
    write_motw(&image, &origin).unwrap();
    let mut ads = image.as_os_str().to_os_string();
    ads.push(":Zone.Identifier");
    assert_eq!(
        std::fs::read(PathBuf::from(ads)).unwrap(),
        motw_contents(&origin).unwrap().as_bytes()
    );
    assert_eq!(std::fs::read(image).unwrap(), b"original image bytes");
}

#[test]
fn browser818_capture_file_destination_boundaries_and_missing_directories() {
    let fixture = tempfile::tempdir().unwrap();
    let profile = fixture.path().join("profile");
    let host = profile.join("browser-host");
    for path in [
        host.clone(),
        host.join("generation"),
        host.join("generation/2026-11"),
    ] {
        assert!(
            is_browser_host_destination(&path, &host).unwrap(),
            "{path:?}"
        );
    }
    for path in [
        profile.clone(),
        profile.join("browser-host-other"),
        profile.join("browser"),
        host.join("../browser"),
    ] {
        assert!(
            !is_browser_host_destination(&path, &host).unwrap(),
            "{path:?}"
        );
    }
    assert!(
        is_browser_host_destination(&profile.join("other/../browser-host/generation"), &host)
            .unwrap()
    );
    assert!(
        !profile.exists(),
        "comparison must not create either directory"
    );
    std::fs::create_dir_all(host.join("generation")).unwrap();
    assert!(is_browser_host_destination(&host, &host).unwrap());
    assert!(is_browser_host_destination(&host.join("generation/../next"), &host).unwrap());
    assert!(!is_browser_host_destination(&host.join("generation/../../browser"), &host).unwrap());
}

#[cfg(windows)]
#[test]
fn browser818_capture_file_destination_case_trailing_separator_and_verbatim_alias() {
    let fixture = tempfile::tempdir().unwrap();
    let host = fixture.path().join("利用者Ä/browser-host");
    let alias = fixture.path().join("利用者ä/BROWSER-HOST/generation");
    assert!(is_browser_host_destination(&alias, &host).unwrap());
    let mut trailing = host.as_os_str().to_os_string();
    trailing.push(r"\\");
    assert!(is_browser_host_destination(Path::new(&trailing), &host).unwrap());
    let canonical = std::fs::canonicalize(fixture.path()).unwrap();
    assert!(
        is_browser_host_destination(&canonical.join("利用者Ä").join("browser-host"), &host)
            .unwrap()
    );
    assert!(
        is_browser_host_destination(
            &canonical
                .join("other")
                .join("..")
                .join("利用者Ä")
                .join("browser-host"),
            &host
        )
        .unwrap()
    );
}

#[test]
fn browser818_capture_file_destination_errors_do_not_allow_unknown_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let host = fixture.path().join("browser-host");
    assert!(is_browser_host_destination(Path::new(""), &host).is_err());
    assert!(is_browser_host_destination(&host, Path::new("")).is_err());
    let file = fixture.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    assert!(is_browser_host_destination(&file.join("child"), &host).is_err());
    assert!(is_browser_host_destination(&host, &file.join("child")).is_err());
}

#[cfg(windows)]
#[test]
fn browser818_capture_file_destination_rejects_raw_dot_components_before_junction_resolution() {
    use std::os::windows::process::CommandExt;
    use std::path::{Component, Prefix};

    let fixture = tempfile::tempdir().unwrap();
    let host = fixture.path().join("browser-host");
    std::fs::create_dir(&host).unwrap();
    std::fs::write(host.join("keep"), b"user image").unwrap();
    let alias = fixture.path().join("alias");
    // All paths are owned by this TempDir; /J requires no symlink privilege.
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&host)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let canonical = std::fs::canonicalize(fixture.path()).unwrap();
    let has_verbatim_prefix = matches!(
        canonical.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _))
    );
    let results: Vec<_> = [
        r"\missing\..\alias\new",
        r"\missing\..\alias\.\new",
        r"\alias\.\new",
    ]
    .into_iter()
    .map(|suffix| {
        // OsString::push preserves the raw spelling. PathBuf::join would erase
        // the very dot components whose admission this regression must test.
        let mut raw = canonical.as_os_str().to_os_string();
        raw.push(suffix);
        let raw = PathBuf::from(raw);
        let absolute = std::path::absolute(&raw).unwrap();
        let retains_dot = absolute
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir));
        let destination_result = is_browser_host_destination(&raw, &host);
        let host_result = is_browser_host_destination(&host, &raw);
        // Model the later save operation. Whether join normalizes the dots or keeps
        // them, the month folder must never be judged outside the managed host
        // directory: either the dots are rejected or the alias resolves inside it.
        let month_result = is_browser_host_destination(&raw.join("2026-11"), &host);
        (
            raw,
            retains_dot,
            destination_result,
            host_result,
            month_result,
        )
    })
    .collect();
    let direct_alias_result = is_browser_host_destination(&alias.join("new"), &host);
    let mut ordinary_raw = fixture.path().as_os_str().to_os_string();
    ordinary_raw.push(r"\missing\..\alias\new");
    let ordinary_result = is_browser_host_destination(Path::new(&ordinary_raw), &host);
    let missing_exists = fixture.path().join("missing").exists();
    let preserved = std::fs::read(host.join("keep"));

    // Remove only the junction, before TempDir's recursive cleanup or assertions.
    std::fs::remove_dir(&alias).unwrap();
    assert!(has_verbatim_prefix);
    for (raw, retains_dot, destination_result, host_result, month_result) in results {
        assert!(retains_dot, "fixture lost its raw dot component: {raw:?}");
        assert!(
            destination_result.is_err(),
            "{raw:?}: {destination_result:?}"
        );
        assert!(host_result.is_err(), "{raw:?}: {host_result:?}");
        assert!(
            !matches!(month_result, Ok(false)),
            "{raw:?}: {month_result:?}"
        );
    }
    assert!(direct_alias_result.unwrap());
    assert!(ordinary_result.unwrap());
    assert!(!missing_exists);
    assert_eq!(preserved.unwrap(), b"user image");
}

#[cfg(windows)]
#[test]
fn browser818_capture_file_destination_rejects_device_namespaces() {
    let fixture = tempfile::tempdir().unwrap();
    let host = fixture.path().join("browser-host");
    assert!(is_browser_host_destination(Path::new(r"\\.\C:\browser-host"), &host).is_err());
    assert!(
        is_browser_host_destination(
            Path::new(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\browser-host"),
            &host
        )
        .is_err()
    );
}
