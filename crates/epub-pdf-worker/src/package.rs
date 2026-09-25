//! EPUB container and package parsing. This module does not start WebView2.
use percent_encoding::percent_decode_str;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Cursor, Read, Seek},
    path::{Component, Path},
};
use zip::ZipArchive;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EpubErrorKind {
    Drm,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct EpubError {
    pub kind: EpubErrorKind,
    pub message: String,
}
impl std::fmt::Display for EpubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for EpubError {}
fn invalid(s: impl Into<String>) -> EpubError {
    EpubError {
        kind: EpubErrorKind::Invalid,
        message: s.into(),
    }
}
fn drm(s: impl Into<String>) -> EpubError {
    EpubError {
        kind: EpubErrorKind::Drm,
        message: s.into(),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Rendition {
    pub layout: Option<String>,
    pub spread: Option<String>,
    pub orientation: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub path: String,
    pub media_type: String,
    pub properties: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SpineItem {
    pub idref: String,
    pub path: String,
    pub media_type: String,
    pub linear: bool,
    pub rendition: Rendition,
    pub page_spread: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_source: Option<String>,
    pub direct_image: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub opf_path: String,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub rendition: Rendition,
    pub direction: String,
    pub manifest: Vec<ManifestItem>,
    pub spine: Vec<SpineItem>,
    pub drm: String,
}

fn local(n: &[u8]) -> &[u8] {
    n.rsplit(|b| *b == b':').next().unwrap_or(n)
}
fn attrs(e: &BytesStart<'_>, r: &Reader<&[u8]>) -> Result<HashMap<String, String>, EpubError> {
    let mut out = HashMap::new();
    for a in e.attributes().with_checks(false) {
        let a = a.map_err(|x| invalid(x.to_string()))?;
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let v = a
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, r.decoder())
            .map_err(|x| invalid(x.to_string()))?
            .into_owned();
        out.insert(k, v);
    }
    Ok(out)
}
fn attr<'a>(a: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    a.get(name).map(String::as_str).or_else(|| {
        a.iter()
            .find(|(k, _)| local(k.as_bytes()) == name.as_bytes())
            .map(|(_, v)| v.as_str())
    })
}

/// Normalize an archive member or resolved href; reject paths that escape the archive.
pub fn safe_path(path: &str) -> Result<String, EpubError> {
    normalize_path(path, false)
}
fn normalize_path(path: &str, allow_parent: bool) -> Result<String, EpubError> {
    let decoded = percent_decode_str(path.split(['#', '?']).next().unwrap_or(path))
        .decode_utf8()
        .map_err(|_| invalid("invalid href encoding"))?;
    let decoded = decoded.replace('\\', "/");
    if decoded.starts_with('/') || decoded.starts_with("//") || decoded.contains(':') {
        return Err(invalid(format!("unsafe archive path: {path}")));
    }
    let mut stack = Vec::new();
    for c in decoded.split('/') {
        match c {
            "" | "." => {}
            ".." if allow_parent => {
                if stack.pop().is_none() {
                    return Err(invalid(format!("unsafe archive path: {path}")));
                }
            }
            ".." => return Err(invalid(format!("unsafe archive path: {path}"))),
            _ => stack.push(c),
        }
    }
    if stack.is_empty() {
        return Err(invalid(format!("empty archive path: {path}")));
    }
    Ok(stack.join("/"))
}
/// `http:`, `data:`, `//host/...` etc. point outside the archive. They are not archive paths:
/// the parser skips them instead of rejecting the book (the renderer blocks the request).
fn is_external_href(href: &str) -> bool {
    // Chromium treats backslashes as separators in special-scheme URLs.
    let normalized = href.trim().replace('\\', "/");
    let href = normalized.as_str();
    if href.starts_with("//") {
        return true;
    }
    match href.find(':') {
        Some(i) => {
            let scheme = &href[..i];
            !scheme.is_empty()
                && scheme
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && !href[..i].contains('/')
        }
        None => false,
    }
}
fn resolve(base: &str, href: &str) -> Result<String, EpubError> {
    let parent = base.rsplit_once('/').map(|x| x.0).unwrap_or("");
    normalize_path(&format!("{parent}/{href}"), true)
}
fn read_member<R: Read + Seek>(z: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>, EpubError> {
    let mut f = z
        .by_name(name)
        .map_err(|_| invalid(format!("missing EPUB member: {name}")))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| invalid(e.to_string()))?;
    Ok(b)
}
fn maybe_member<R: Read + Seek>(z: &mut ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    z.by_name(name).ok().and_then(|mut f| {
        let mut b = Vec::new();
        f.read_to_end(&mut b).ok()?;
        Some(b)
    })
}
fn xml(bytes: &[u8]) -> Reader<&[u8]> {
    let mut r = Reader::from_reader(bytes);
    r.config_mut().trim_text(true);
    r
}
fn container_path(bytes: &[u8]) -> Result<String, EpubError> {
    let mut r = xml(bytes);
    let mut buf = Vec::new();
    loop {
        match r
            .read_event_into(&mut buf)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"rootfile" => {
                if let Some(p) = attr(&attrs(&e, &r)?, "full-path") {
                    return safe_path(p);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Err(invalid("container.xml has no rootfile"))
}
fn encryption_is_drm(bytes: &[u8]) -> Result<bool, EpubError> {
    let mut r = xml(bytes);
    let mut b = Vec::new();
    let mut encrypted = 0;
    let mut allowed = 0;
    loop {
        match r
            .read_event_into(&mut b)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"EncryptedData" => {
                encrypted += 1;
            }
            Event::Start(e) | Event::Empty(e)
                if local(e.name().as_ref()) == b"EncryptionMethod" =>
            {
                let a = attrs(&e, &r)?;
                let alg = attr(&a, "Algorithm").unwrap_or("");
                if alg != "http://www.idpf.org/2008/embedding"
                    && alg != "http://ns.adobe.com/pdf/enc#RC"
                {
                    return Ok(true);
                }
                allowed += 1;
            }
            Event::Eof => break,
            _ => {}
        }
        b.clear();
    }
    Ok(encrypted != allowed)
}
fn rendition_property(r: &mut Rendition, key: &str, value: &str) {
    match key {
        "rendition:layout" => r.layout = Some(value.into()),
        "rendition:spread" => r.spread = Some(value.into()),
        "rendition:orientation" => r.orientation = Some(value.into()),
        _ => {}
    }
}
fn dimensions(content: &str) -> Option<(u32, u32)> {
    let vals: Vec<u32> = content
        .split([',', ';', ' '])
        .filter_map(|part| {
            part.split_once('=').and_then(|(k, v)| match k.trim() {
                "width" | "height" => v.trim().trim_end_matches("px").parse().ok(),
                _ => None,
            })
        })
        .collect();
    if vals.len() == 2 && vals[0] > 0 && vals[1] > 0 {
        Some((vals[0], vals[1]))
    } else {
        None
    }
}
fn size_attr(v: &str) -> Option<u32> {
    v.trim()
        .trim_end_matches("px")
        .parse::<f64>()
        .ok()
        .filter(|x| *x > 0.0)
        .map(|x| x.round() as u32)
}
fn svg_size(a: &HashMap<String, String>) -> Option<(u32, u32)> {
    if let Some(v) = attr(a, "viewBox") {
        let p: Vec<f64> = v
            .split([',', ' ', '\t'])
            .filter_map(|s| s.parse().ok())
            .collect();
        if p.len() == 4 && p[2] > 0.0 && p[3] > 0.0 {
            return Some((p[2].round() as u32, p[3].round() as u32));
        }
    }
    Some((
        size_attr(attr(a, "width")?)?,
        size_attr(attr(a, "height")?)?,
    ))
}
type XhtmlInfo = (Option<(u32, u32, String)>, Option<String>);
fn xhtml_info(bytes: &[u8], path: &str) -> Result<XhtmlInfo, EpubError> {
    let mut r = xml(bytes);
    let mut b = Vec::new();
    let mut viewport = None;
    let mut svg = None;
    let mut image = None;
    let mut images = 0;
    let mut other = false;
    let mut in_body = false;
    loop {
        match r
            .read_event_into(&mut b)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let name = local(e.name().as_ref()).to_vec();
                let a = attrs(&e, &r)?;
                if name == b"body" {
                    in_body = true;
                }
                if name == b"meta" && attr(&a, "name") == Some("viewport") {
                    viewport = attr(&a, "content").and_then(dimensions);
                }
                if name == b"svg" && svg.is_none() {
                    svg = svg_size(&a);
                }
                if (name == b"img" || name == b"image") && in_body {
                    images += 1;
                    if let Some(src) = attr(&a, "src").or_else(|| attr(&a, "href")) {
                        // An external image is not a local page image; the page takes the
                        // iframe path, where the request is blocked.
                        if !is_external_href(src) {
                            image = Some(resolve(path, src)?);
                        }
                    }
                }
                if in_body
                    && !matches!(
                        name.as_slice(),
                        b"body" | b"div" | b"span" | b"svg" | b"image" | b"img" | b"a" | b"p"
                    )
                {
                    other = true;
                }
            }
            Event::End(e) if local(e.name().as_ref()) == b"body" => in_body = false,
            Event::Text(e) if in_body && !e.as_ref().iter().all(u8::is_ascii_whitespace) => {
                other = true
            }
            Event::Eof => break,
            _ => {}
        }
        b.clear();
    }
    let size = viewport
        .map(|(w, h)| (w, h, "xhtml_viewport".into()))
        .or_else(|| svg.map(|(w, h)| (w, h, "svg_viewbox".into())));
    Ok((size, if images == 1 && !other { image } else { None }))
}
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<Package, EpubError> {
    let mut z =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| invalid(format!("invalid ZIP: {e}")))?;
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|e| invalid(e.to_string()))?;
        safe_path(f.name())?;
    }
    if maybe_member(&mut z, "META-INF/rights.xml").is_some() {
        return Err(drm("META-INF/rights.xml detected"));
    }
    if maybe_member(&mut z, "META-INF/license.lcpl").is_some() {
        return Err(drm("Readium LCP license detected"));
    }
    if let Some(e) = maybe_member(&mut z, "META-INF/encryption.xml")
        && encryption_is_drm(&e)?
    {
        return Err(drm("encrypted resource uses a DRM algorithm"));
    }
    let opf_path = container_path(&read_member(&mut z, "META-INF/container.xml")?)?;
    let opf = read_member(&mut z, &opf_path)?;
    let mut r = xml(&opf);
    let mut b = Vec::new();
    let mut title = None;
    let mut creator = None;
    let mut rendition = Rendition::default();
    let mut opf_viewport = None;
    let mut manifest = Vec::new();
    let mut refs = Vec::<(String, bool, Vec<String>)>::new();
    let mut direction = "default".to_string();
    let mut current_meta: Option<String> = None;
    let mut current_text: Option<String> = None;
    loop {
        match r
            .read_event_into(&mut b)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let n = local(e.name().as_ref()).to_vec();
                let a = attrs(&e, &r)?;
                match n.as_slice() {
                    b"item" => {
                        let id = attr(&a, "id")
                            .ok_or_else(|| invalid("manifest item lacks id"))?
                            .to_string();
                        let href = attr(&a, "href")
                            .ok_or_else(|| invalid("manifest item lacks href"))?
                            .to_string();
                        // EPUB allows remote resources in the manifest; they are not in the
                        // archive, so they are skipped rather than treated as unsafe paths.
                        if !is_external_href(&href) {
                            manifest.push(ManifestItem {
                                id,
                                path: resolve(&opf_path, &href)?,
                                href,
                                media_type: attr(&a, "media-type").unwrap_or("").into(),
                                properties: attr(&a, "properties")
                                    .unwrap_or("")
                                    .split_whitespace()
                                    .map(str::to_string)
                                    .collect(),
                            });
                        }
                    }
                    b"spine" => {
                        direction = attr(&a, "page-progression-direction")
                            .unwrap_or("default")
                            .into()
                    }
                    b"itemref" => refs.push((
                        attr(&a, "idref")
                            .ok_or_else(|| invalid("itemref lacks idref"))?
                            .into(),
                        attr(&a, "linear") != Some("no"),
                        attr(&a, "properties")
                            .unwrap_or("")
                            .split_whitespace()
                            .map(str::to_string)
                            .collect(),
                    )),
                    b"meta" => {
                        if let Some(k) = attr(&a, "property") {
                            current_meta = Some(k.into());
                        }
                        if let Some(k) = attr(&a, "name") {
                            let v = attr(&a, "content").unwrap_or("");
                            rendition_property(&mut rendition, k, v);
                            if k == "rendition:viewport" {
                                opf_viewport = dimensions(v);
                            }
                        }
                    }
                    b"title" | b"creator" => {
                        current_text = Some(String::from_utf8_lossy(&n).into_owned())
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                let value =
                    quick_xml::escape::unescape(&e.decode().map_err(|x| invalid(x.to_string()))?)
                        .map_err(|x| invalid(x.to_string()))?
                        .into_owned();
                if let Some(k) = &current_meta {
                    rendition_property(&mut rendition, k, &value);
                    if k == "rendition:viewport" {
                        opf_viewport = dimensions(&value);
                    }
                }
                if let Some(k) = &current_text {
                    if k == "title" {
                        title = Some(value);
                    } else if k == "creator" {
                        creator = Some(value);
                    }
                }
            }
            Event::End(e) => match local(e.name().as_ref()) {
                b"meta" => current_meta = None,
                b"title" | b"creator" => current_text = None,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        b.clear();
    }
    if refs.is_empty() {
        return Err(invalid("empty EPUB spine"));
    }
    let mut spine = Vec::new();
    for (idref, linear, props) in refs {
        let item = manifest
            .iter()
            .find(|x| x.id == idref)
            .ok_or_else(|| invalid(format!("spine idref missing from manifest: {idref}")))?;
        let mut applied = rendition.clone();
        let mut page_spread = None;
        for p in &props {
            for (prefix, field) in [
                ("rendition:layout-", "layout"),
                ("rendition:spread-", "spread"),
                ("rendition:orientation-", "orientation"),
            ] {
                if let Some(v) = p.strip_prefix(prefix) {
                    match field {
                        "layout" => applied.layout = Some(v.into()),
                        "spread" => applied.spread = Some(v.into()),
                        _ => applied.orientation = Some(v.into()),
                    }
                }
            }
            if let Some(v) = p.strip_prefix("page-spread-") {
                page_spread = Some(v.into());
            }
        }
        let data = read_member(&mut z, &item.path)?;
        let (mut size, direct_image) = if item.media_type == "image/svg+xml" {
            // An SVG page is printed as a document (iframe), never as <img>: SVG loaded as an
            // image may not fetch external resources, so its <image href="page.jpg"> would be blank.
            let (s, _) = xhtml_info(&data, &item.path)?;
            (s, None)
        } else if item.media_type.starts_with("image/") {
            (
                image_size(&data).map(|(w, h)| (w, h, "image_intrinsic".into())),
                Some(item.path.clone()),
            )
        } else if item.media_type == "application/xhtml+xml" || item.media_type == "text/html" {
            xhtml_info(&data, &item.path)?
        } else {
            (None, None)
        };
        if size.is_none()
            && let Some(img) = &direct_image
            && let Some(data) = maybe_member(&mut z, img)
        {
            size = image_size(&data).map(|(w, h)| (w, h, "image_intrinsic".into()));
        }
        if size.is_none() {
            size = opf_viewport.map(|(w, h)| (w, h, "opf_viewport".into()));
        }
        let (width, height, size_source) =
            size.map_or((None, None, None), |(w, h, s)| (Some(w), Some(h), Some(s)));
        spine.push(SpineItem {
            idref,
            path: item.path.clone(),
            media_type: item.media_type.clone(),
            linear,
            rendition: applied,
            page_spread,
            width,
            height,
            size_source,
            direct_image,
        });
    }
    Ok(Package {
        opf_path,
        title,
        creator,
        rendition,
        direction,
        manifest,
        spine,
        drm: "none".into(),
    })
}
pub fn inspect_file(path: &Path) -> Result<Package, EpubError> {
    let bytes = fs::read(path).map_err(|e| invalid(e.to_string()))?;
    inspect_bytes(&bytes)
}

pub fn extract_file(input: &Path, dir: &Path) -> Result<(), EpubError> {
    fs::create_dir_all(dir).map_err(|e| invalid(e.to_string()))?;
    let mut z = ZipArchive::new(File::open(input).map_err(|e| invalid(e.to_string()))?)
        .map_err(|e| invalid(e.to_string()))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| invalid(e.to_string()))?;
        let relative = safe_path(f.name())?;
        let relative = Path::new(&relative);
        if relative.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        }) {
            return Err(invalid("unsafe extraction destination"));
        }
        let out = dir.join(relative);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| invalid(e.to_string()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| invalid(e.to_string()))?;
        }
        let mut writer = File::create(&out).map_err(|e| invalid(e.to_string()))?;
        std::io::copy(&mut f, &mut writer).map_err(|e| invalid(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn book(opf: &str, extras: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut z = ZipWriter::new(&mut out);
            for (name,data) in [("META-INF/container.xml",br#"<container><rootfiles><rootfile full-path="OPS/content.opf"/></rootfiles></container>"#.as_slice()),("OPS/content.opf",opf.as_bytes())].into_iter().chain(extras.iter().copied()) {
                z.start_file(name,SimpleFileOptions::default()).unwrap(); z.write_all(data).unwrap();
            }
            z.finish().unwrap();
        }
        out.into_inner()
    }
    fn opf(direction: &str, override_props: &str) -> String {
        format!(
            r#"<package><metadata><dc:title xmlns:dc="x">Test</dc:title><meta property="rendition:layout">pre-paginated</meta><meta property="rendition:spread">auto</meta></metadata><manifest><item id="p" href="p.xhtml" media-type="application/xhtml+xml"/></manifest><spine page-progression-direction="{direction}"><itemref idref="p" properties="{override_props}"/></spine></package>"#
        )
    }
    const PAGE:&[u8]=br#"<html><head><meta name="viewport" content="width=1200,height=1700"/></head><body><img src="a.jpg"/></body></html>"#;
    #[test]
    fn direction_and_override() {
        for d in ["rtl", "ltr"] {
            let p = inspect_bytes(&book(
                &opf(d, "rendition:layout-reflowable page-spread-left"),
                &[("OPS/p.xhtml", PAGE)],
            ))
            .unwrap();
            assert_eq!(p.direction, d);
            assert_eq!(p.spine[0].rendition.layout.as_deref(), Some("reflowable"));
            assert_eq!(p.spine[0].page_spread.as_deref(), Some("left"));
        }
    }
    #[test]
    fn viewport_priority_and_opf_fallback() {
        let p = inspect_bytes(&book(&opf("rtl", ""), &[("OPS/p.xhtml", PAGE)])).unwrap();
        assert_eq!(
            (p.spine[0].width, p.spine[0].height),
            (Some(1200), Some(1700))
        );
        assert_eq!(p.spine[0].size_source.as_deref(), Some("xhtml_viewport"));
        let opf = opf("ltr", "").replace(
            "</metadata>",
            "<meta property=\"rendition:viewport\">width=800,height=900</meta></metadata>",
        );
        let p = inspect_bytes(&book(
            &opf,
            &[("OPS/p.xhtml", b"<html><body>text</body></html>")],
        ))
        .unwrap();
        assert_eq!(p.spine[0].size_source.as_deref(), Some("opf_viewport"));
    }
    #[test]
    fn external_references_do_not_invalidate_the_book() {
        assert!(is_external_href("http://example.invalid/a.png"));
        assert!(is_external_href("HTTPS://example.invalid/a.png"));
        assert!(is_external_href("data:image/png;base64,AAAA"));
        assert!(is_external_href("//example.invalid/a.png"));
        assert!(is_external_href("\\\\example.invalid/a.png"));
        assert!(is_external_href("\\/example.invalid/a.png"));
        assert!(!is_external_href("a.jpg"));
        assert!(!is_external_href("img/a%3Ab.jpg"));
        assert!(!is_external_href("../img/a.jpg"));
        // A page whose only image is remote is not a local direct image, and a remote
        // manifest item is skipped instead of rejecting the book as an unsafe path.
        let opf = opf("rtl", "").replace(
            "</manifest>",
            r#"<item id="r" href="https://example.invalid/font.woff2" media-type="font/woff2"/></manifest>"#,
        );
        let page = br#"<html><head><meta name="viewport" content="width=1200,height=1700"/></head><body><img src="http://example.invalid/remote.png"/></body></html>"#;
        let p = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", page)])).unwrap();
        assert_eq!(p.spine.len(), 1);
        assert_eq!(p.spine[0].direct_image, None);
    }
    #[test]
    fn svg_viewbox() {
        assert_eq!(
            svg_size(&HashMap::from([("viewBox".into(), "0 0 600 700".into())])),
            Some((600, 700))
        );
        assert!(
            xhtml_info(br#"<svg viewBox="0 0 600 700"/>"#, "OPS/p.svg")
                .unwrap()
                .0
                .is_some()
        );
        assert!(
            xhtml_info(
                br#"<svg viewBox="0 0 600 700" xmlns="http://www.w3.org/2000/svg"/>"#,
                "OPS/p.svg"
            )
            .unwrap()
            .0
            .is_some()
        );
        let opf = r#"<package><manifest><item id="p" href="p.svg" media-type="image/svg+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#;
        let p = inspect_bytes(&book(
            opf,
            &[(
                "OPS/p.svg",
                br#"<svg viewBox="0 0 600 700" xmlns="http://www.w3.org/2000/svg"/>"#,
            )],
        ))
        .unwrap();
        assert_eq!(
            (p.spine[0].width, p.spine[0].height),
            (Some(600), Some(700))
        );
    }
    #[test]
    fn image_intrinsic_and_percent_decoded_href() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(30, 40))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let opf = r#"<package><manifest><item id="p" href="a%20b.png" media-type="image/png"/></manifest><spine><itemref idref="p"/></spine></package>"#;
        let p = inspect_bytes(&book(opf, &[("OPS/a b.png", png.get_ref())])).unwrap();
        assert_eq!((p.spine[0].width, p.spine[0].height), (Some(30), Some(40)));
        assert_eq!(p.spine[0].size_source.as_deref(), Some("image_intrinsic"));
        let opf = r#"<package><manifest><item id="p" href="p.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#;
        let p = inspect_bytes(&book(
            opf,
            &[
                (
                    "OPS/p.xhtml",
                    br#"<html><body><img src="a%20b.png"/></body></html>"#,
                ),
                ("OPS/a b.png", png.get_ref()),
            ],
        ))
        .unwrap();
        assert_eq!(p.spine[0].size_source.as_deref(), Some("image_intrinsic"));
    }
    #[test]
    fn font_obfuscation_is_not_drm() {
        let enc=br#"<encryption><EncryptedData><EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/></EncryptedData></encryption>"#;
        assert!(
            inspect_bytes(&book(
                &opf("rtl", ""),
                &[("OPS/p.xhtml", PAGE), ("META-INF/encryption.xml", enc)]
            ))
            .is_ok()
        );
        let enc=br#"<encryption><EncryptedData><EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/></EncryptedData></encryption>"#;
        assert_eq!(
            inspect_bytes(&book(
                &opf("rtl", ""),
                &[("OPS/p.xhtml", PAGE), ("META-INF/encryption.xml", enc)]
            ))
            .unwrap_err()
            .kind,
            EpubErrorKind::Drm
        );
        assert_eq!(
            inspect_bytes(&book(
                &opf("rtl", ""),
                &[("OPS/p.xhtml", PAGE), ("META-INF/rights.xml", b"<rights/>")]
            ))
            .unwrap_err()
            .kind,
            EpubErrorKind::Drm
        );
        assert_eq!(
            inspect_bytes(&book(
                &opf("rtl", ""),
                &[
                    ("OPS/p.xhtml", PAGE),
                    (
                        "META-INF/encryption.xml",
                        b"<encryption><EncryptedData/></encryption>"
                    )
                ]
            ))
            .unwrap_err()
            .kind,
            EpubErrorKind::Drm
        );
    }
    #[test]
    fn zip_slip_and_missing_container() {
        assert!(safe_path("a/../b").is_err());
        assert!(safe_path("/absolute").is_err());
        assert!(safe_path("C:/absolute").is_err());
        assert!(
            inspect_bytes(&book(
                &opf("rtl", ""),
                &[("OPS/p.xhtml", PAGE), ("../bad", b"x")]
            ))
            .is_err()
        );
        let mut out = Cursor::new(Vec::new());
        {
            let mut z = ZipWriter::new(&mut out);
            z.start_file("OPS/content.opf", SimpleFileOptions::default())
                .unwrap();
            z.write_all(opf("rtl", "").as_bytes()).unwrap();
            z.finish().unwrap();
        }
        assert!(inspect_bytes(&out.into_inner()).is_err());
    }
}
