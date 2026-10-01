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
use url::Url;
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
    #[serde(skip)]
    pub url: Url,
    pub media_type: String,
    pub properties: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SpineItem {
    pub idref: String,
    pub path: String,
    #[serde(skip)]
    pub url: Url,
    pub media_type: String,
    pub linear: bool,
    pub rendition: Rendition,
    pub page_spread: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_source: Option<String>,
    pub direct_image: Option<MemberReference>,
}
/// A resolved local reference owns the exact member name and its browser URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberReference {
    pub path: String,
    pub url: Url,
}
impl std::ops::Deref for MemberReference {
    type Target = str;
    fn deref(&self) -> &str {
        &self.path
    }
}
// Keep the conversion report's direct_image field as the raw member name.
impl Serialize for MemberReference {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.path)
    }
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

/// Validate a raw ZIP name without interpreting it as a URL or renaming it.
pub fn safe_path(path: &str) -> Result<String, EpubError> {
    let name = path.strip_suffix('/').unwrap_or(path);
    if name.is_empty() || name.starts_with('/') || name.contains('\\') {
        return Err(invalid(format!("unsafe archive path: {path}")));
    }
    for component in name.split('/') {
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        let reserved = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            || component.encode_utf16().count() > 255
            || reserved
        {
            return Err(invalid(format!("unsafe archive path: {path}")));
        }
    }
    Ok(path.to_string())
}
/// Directory entries create directories only: harmless aliases need not name a member.
/// File entries always retain the strict raw-name validation above.
fn safe_directory_path(path: &str) -> Result<String, EpubError> {
    if path.starts_with('/') || path.contains('\\') {
        return Err(invalid(format!("unsafe archive directory: {path}")));
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components
                    .pop()
                    .ok_or_else(|| invalid(format!("unsafe archive directory: {path}")))?;
            }
            _ => {
                // Validate before normalization, so a drive/reserved/invalid component
                // cannot disappear behind a later '..'. Raw '%' and '#' remain literal.
                safe_path(component)?;
                components.push(component);
            }
        }
    }
    Ok(components.join("/"))
}
/// Encode real member names segment by segment. '%' and '#' are literal name characters.
pub fn member_url(path: &str) -> Url {
    let encoded = path
        .split('/')
        .map(|segment| {
            percent_encoding::utf8_percent_encode(segment, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("/");
    Url::parse(&format!("https://epub.invalid/{encoded}")).expect("encoded member URL")
}
// WHATWG preprocessing removes TAB/LF/CR anywhere and trims edge C0 controls/spaces.
fn clean_href(href: &str) -> String {
    href.trim_matches(|c| c <= '\u{20}')
        .replace(['\t', '\n', '\r'], "")
}
/// `http:`, `data:`, `//host/...` etc. point outside the archive. They are not archive paths:
/// the parser skips them instead of rejecting the book (the renderer blocks the request).
fn is_external_href(href: &str) -> bool {
    // Chromium treats backslashes as separators in special-scheme URLs.
    // Classification keeps the historical Unicode-whitespace trim. It does not
    // trim local member names or change WHATWG resolution of local references.
    let normalized = clean_href(href).trim().replace('\\', "/");
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
fn resolve(base: &str, href: &str) -> Result<MemberReference, EpubError> {
    let href = clean_href(href);
    // Explicit schemes (including C:/x and file:) and network paths never map to members,
    // even when their host happens to be epub.invalid. Callers skip these resources.
    if is_external_href(&href) {
        return Err(invalid(format!("external EPUB reference: {href}")));
    }
    if !base.is_empty() {
        safe_path(base)?;
    }
    let path = href
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .replace('\\', "/");
    let mut depth = if path.starts_with('/') {
        0
    } else {
        base.matches('/').count()
    };
    // Url::join clamps traversal at the root. Reject escape before that evidence is lost.
    for (index, segment) in path.split('/').enumerate() {
        if index == 0 && segment.is_empty() {
            continue;
        }
        let decoded = percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| invalid("invalid href encoding"))?;
        if decoded.contains(['/', '\\']) {
            return Err(invalid(format!(
                "encoded separator in EPUB reference: {href}"
            )));
        }
        match decoded.as_ref() {
            "." => {}
            ".." => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid(format!("unsafe archive path: {href}")))?;
            }
            _ => depth += 1,
        }
    }
    let url = member_url(base)
        .join(&href)
        .map_err(|e| invalid(format!("invalid EPUB reference: {e}")))?;
    let member = url
        .path_segments()
        .ok_or_else(|| invalid("invalid local URL"))?
        .map(|segment| {
            percent_decode_str(segment)
                .decode_utf8()
                .map(|s| s.into_owned())
                .map_err(|_| invalid("invalid href encoding"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    safe_path(&member)?;
    // Rebuild using the same encoding as generated HTML, retaining query/fragment separately.
    let mut browser_url = member_url(&member);
    browser_url.set_query(url.query());
    browser_url.set_fragment(url.fragment());
    Ok(MemberReference {
        path: member,
        url: browser_url,
    })
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
                    return Ok(resolve("", p)?.path);
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
type XhtmlInfo = (Option<(u32, u32, String)>, Option<MemberReference>);
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
        if f.is_dir() {
            safe_directory_path(f.name())?;
        } else {
            safe_path(f.name())?;
        }
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
                        let properties = attr(&a, "properties")
                            .unwrap_or("")
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>();
                        // EPUB allows remote resources in the manifest; they are not in the
                        // archive, so they are skipped rather than treated as unsafe paths.
                        if !is_external_href(&href) {
                            let reference = resolve(&opf_path, &href).map_err(|e| {
                                invalid(format!("manifest item {id} href {href:?}: {e}"))
                            })?;
                            manifest.push(ManifestItem {
                                id,
                                path: reference.path,
                                url: reference.url,
                                href,
                                media_type: attr(&a, "media-type").unwrap_or("").into(),
                                properties,
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
                    if k == "title" && title.is_none() {
                        title = Some(value);
                    } else if k == "creator" && creator.is_none() {
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
        let item = manifest.iter().find(|x| x.id == idref).ok_or_else(|| {
            invalid(format!(
                "spine idref missing or external in manifest: {idref}"
            ))
        })?;
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
                Some(MemberReference {
                    path: item.path.clone(),
                    url: item.url.clone(),
                }),
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
            url: item.url.clone(),
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
        let relative = if f.is_dir() {
            safe_directory_path(f.name())?
        } else {
            safe_path(f.name())?
        };
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
    fn synthetic_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut out);
            for (name, bytes) in files {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }
    const ROOT_CONTAINER: &[u8] =
        br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#;

    #[test]
    fn root_opf_and_subdirectory_opf_can_reference_root_cover_page() {
        let image = png(30, 40);
        for opf_path in ["content.opf", "OPS/content.opf"] {
            let prefix = if opf_path.contains('/') { "../" } else { "" };
            let opf = format!(
                r#"<package><manifest><item id="cover" href="{prefix}titlepage.xhtml" media-type="application/xhtml+xml"/><item id="text" href="{prefix}text/part0000.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="cover"/><itemref idref="text"/></spine></package>"#
            );
            let container = format!(
                r#"<container><rootfiles><rootfile full-path="{opf_path}"/></rootfiles></container>"#
            );
            let package = inspect_bytes(&synthetic_zip(&[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container.as_bytes()),
                (opf_path, opf.as_bytes()),
                (
                    "titlepage.xhtml",
                    br#"<html><body><svg><image xlink:href="cover.jpeg"/></svg></body></html>"#,
                ),
                ("cover.jpeg", &image),
                ("text/part0000.xhtml", b"<html><body>text</body></html>"),
            ]))
            .unwrap();
            assert_eq!(package.opf_path, opf_path);
            assert_eq!(
                package
                    .spine
                    .iter()
                    .map(|item| item.path.as_str())
                    .collect::<Vec<_>>(),
                ["titlepage.xhtml", "text/part0000.xhtml"]
            );
            assert_eq!(package.spine[0].direct_image.as_deref(), Some("cover.jpeg"));
            assert_eq!(
                (package.spine[0].width, package.spine[0].height),
                (Some(30), Some(40))
            );
        }
    }
    #[test]
    fn root_page_img_svg_href_and_xlink_href() {
        for element in [
            r#"<img src="cover.jpg"/>"#,
            r#"<svg><image href="cover.jpg"/></svg>"#,
            r#"<svg><image xlink:href="cover.jpg"/></svg>"#,
        ] {
            let page = format!("<html><body>{element}</body></html>");
            assert_eq!(
                xhtml_info(page.as_bytes(), "titlepage.xhtml")
                    .unwrap()
                    .1
                    .as_deref(),
                Some("cover.jpg")
            );
        }
    }
    #[test]
    fn local_url_resolution_matches_browser_paths() {
        for (base, href, expected) in [
            ("content.opf", "titlepage.xhtml", "titlepage.xhtml"),
            ("OPS/content.opf", "p.xhtml", "OPS/p.xhtml"),
            ("OPS/content.opf", "text/p.xhtml", "OPS/text/p.xhtml"),
            ("OPS/content.opf", "./p.xhtml", "OPS/p.xhtml"),
            ("OPS/content.opf", "../p.xhtml", "p.xhtml"),
            ("OPS/content.opf", "/x", "x"),
            ("OPS/p.xhtml", r"a\b", "OPS/a/b"),
            ("OPS/p.xhtml", r"\x", "x"),
            ("OPS/p.xhtml", r"\host/x", "host/x"),
            ("p.xhtml", "a//../../x", "x"),
            ("OPS/p.xhtml", "a.png?v=1#f", "OPS/a.png"),
            ("OPS/p.xhtml", "a.png?../../x#../../x", "OPS/a.png"),
            ("OPS/p.xhtml", "#id", "OPS/p.xhtml"),
            ("OPS/p.xhtml", "?v=1", "OPS/p.xhtml"),
            ("OPS/p.xhtml", "", "OPS/p.xhtml"),
        ] {
            let reference = resolve(base, href).unwrap();
            assert_eq!(reference.path, expected, "{base} + {href}");
            let browser = member_url(base).join(href).unwrap();
            assert_eq!(
                percent_decode_str(reference.url.path())
                    .decode_utf8()
                    .unwrap(),
                percent_decode_str(browser.path()).decode_utf8().unwrap(),
                "{base} + {href}"
            );
            assert_eq!(reference.url.query(), browser.query());
            assert_eq!(reference.url.fragment(), browser.fragment());
        }
        assert_eq!(
            container_path(br#"<container><rootfile full-path="OPS/a%23b.opf"/></container>"#)
                .unwrap(),
            "OPS/a#b.opf"
        );
    }
    #[test]
    fn above_root_paths_and_encoded_separators_are_rejected() {
        for (base, href) in [
            ("p.xhtml", "../x"),
            ("OPS/p.xhtml", "../../x"),
            ("p.xhtml", "a/../../x"),
            ("p.xhtml", "%2e%2e/x"),
            ("p.xhtml", ".%2E/x"),
            ("p.xhtml", "%2E./x"),
            ("OPS/p.xhtml", "%2e%2e/%2E%2e/x"),
            ("p.xhtml", "..\t/x"),
            ("p.xhtml", " \n../x\r "),
            ("OPS/p.xhtml", "a%2fb"),
            ("OPS/p.xhtml", "a%5Cb"),
            ("OPS/p.xhtml", "a%2Fb?q=1"),
        ] {
            assert!(resolve(base, href).is_err(), "{base} + {href}");
        }
        let opf = br#"<package><manifest><item id="p" href="../evil" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#;
        let error = inspect_bytes(&synthetic_zip(&[
            ("META-INF/container.xml", ROOT_CONTAINER),
            ("content.opf", opf),
            ("evil", b"<html/>"),
        ]))
        .unwrap_err();
        assert!(error.message.contains("unsafe archive path"));
    }
    #[test]
    fn encoding_is_once_only_and_base_is_a_real_name() {
        for (href, name) in [
            ("a%20b.png", "a b.png"),
            ("日本語.png", "日本語.png"),
            ("a%23b.png", "a#b.png"),
            ("a%25b.png", "a%b.png"),
            ("a%2520b.png", "a%20b.png"),
        ] {
            let reference = resolve("dir%20#日本語/p.xhtml", href).unwrap();
            assert_eq!(reference.path, format!("dir%20#日本語/{name}"));
            assert_eq!(
                resolve("content.opf", reference.url.path()).unwrap().path,
                reference.path
            );
        }
        let space = png(10, 20);
        let percent = png(30, 40);
        for (href, expected, size) in [
            ("a%20b.png", "OPS/a b.png", (10, 20)),
            ("a%2520b.png", "OPS/a%20b.png", (30, 40)),
        ] {
            let opf = format!(
                r#"<package><manifest><item id="p" href="{href}" media-type="image/png"/></manifest><spine><itemref idref="p"/></spine></package>"#
            );
            let package = inspect_bytes(&book(
                &opf,
                &[("OPS/a b.png", &space), ("OPS/a%20b.png", &percent)],
            ))
            .unwrap();
            assert_eq!(package.spine[0].path, expected);
            assert_eq!(
                (package.spine[0].width, package.spine[0].height),
                (Some(size.0), Some(size.1))
            );
        }
    }
    #[test]
    fn encoded_container_opf_and_page_bases_keep_raw_names() {
        let image = png(12, 34);
        let package = inspect_bytes(&synthetic_zip(&[
            ("META-INF/container.xml", br#"<container><rootfile full-path="dir%2520%23/content.opf"/></container>"#),
            ("dir%20#/content.opf", br#"<package><manifest><item id="p" href="p%2520%23.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#),
            ("dir%20#/p%20#.xhtml", br#"<html><body><svg><image href="a%25%23%20b.png?v=1#view"/></svg></body></html>"#),
            ("dir%20#/a%# b.png", &image),
        ])).unwrap();
        assert_eq!(package.opf_path, "dir%20#/content.opf");
        let item = &package.spine[0];
        assert_eq!(item.path, "dir%20#/p%20#.xhtml");
        assert_eq!(item.direct_image.as_deref(), Some("dir%20#/a%# b.png"));
        assert_eq!((item.width, item.height), (Some(12), Some(34)));
        assert!(
            crate::render::print_html(&[item], false)
                .contains(r#"src="https://epub.invalid/dir%2520%23/a%25%23%20b%2Epng?v=1#view""#)
        );
    }
    #[test]
    fn raw_zip_names_are_preserved_and_windows_invalid_names_are_rejected() {
        for name in ["a%20b.png", "a#b.png", "日本語/a b.png", "%2e%2e/x", "dir/"] {
            assert_eq!(safe_path(name).unwrap(), name);
        }
        for name in [
            "",
            "/x",
            "//host/x",
            "C:/x",
            r"a\b",
            r"\\host\x",
            "a/../x",
            "a/./x",
            "a//x",
            "a?b",
            "a:b",
            "a*b",
            "a|b",
            "a<b",
            "a>b",
            "a\"b",
            "a\0b",
            "a.",
            "a ",
            "NUL",
            "con.png",
            "COM1.txt",
            "lpt²",
            "a//",
        ] {
            assert!(safe_path(name).is_err(), "{name}");
        }
        assert!(safe_path(&"a".repeat(256)).is_err());
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("synthetic.epub");
        let bytes = synthetic_zip(&[
            ("a%20b.png", b"literal percent"),
            ("a b.png", b"space"),
            ("a#b.png", b"hash"),
        ]);
        fs::write(&input, bytes).unwrap();
        let extracted = dir.path().join("extract");
        extract_file(&input, &extracted).unwrap();
        for (name, data) in [
            ("a%20b.png", b"literal percent".as_slice()),
            ("a b.png", b"space"),
            ("a#b.png", b"hash"),
        ] {
            assert_eq!(fs::read(extracted.join(name)).unwrap(), data);
        }
        assert!(!extracted.join("a").exists());
    }
    #[test]
    fn redundant_directory_entries_are_safe_for_inspection_and_extraction() {
        let opf = opf("ltr", "");
        let bytes = synthetic_zip(&[
            (
                "META-INF/container.xml",
                br#"<container><rootfile full-path="OPS/content.opf"/></container>"#,
            ),
            ("OPS/content.opf", opf.as_bytes()),
            ("OPS/p.xhtml", PAGE),
            ("OPS/./", b""),
            ("OPS//", b""),
            ("./OPS///./", b""),
            ("OPS/unused/../", b""),
            ("./", b""),
            ("OPS/a%20#b//./", b""),
            ("OPS/a%20#b/kept.txt", b"raw name"),
        ]);
        assert_eq!(inspect_bytes(&bytes).unwrap().spine[0].path, "OPS/p.xhtml");
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("synthetic.epub");
        let out = dir.path().join("extracted");
        fs::write(&input, bytes).unwrap();
        extract_file(&input, &out).unwrap();
        assert!(out.join("OPS").is_dir());
        assert_eq!(fs::read(out.join("OPS/p.xhtml")).unwrap(), PAGE);
        assert_eq!(
            fs::read(out.join("OPS/a%20#b/kept.txt")).unwrap(),
            b"raw name"
        );
        assert!(!out.join("OPS/a ").exists());
        assert!(!out.join("OPS/unused").exists());
    }
    #[test]
    fn directory_normalization_does_not_relax_files_or_unsafe_directories() {
        for name in [
            "/OPS/",
            "//host/OPS/",
            "C:/OPS/",
            r"\\host\OPS/",
            r"OPS\x/",
            "../OPS/",
            "OPS/../../x/",
            "OPS/..//../x/",
            "C:/../OPS/",
            "NUL/../OPS/",
        ] {
            assert!(safe_directory_path(name).is_err(), "{name}");
            let bytes = book(&opf("ltr", ""), &[("OPS/p.xhtml", PAGE), (name, b"")]);
            assert!(inspect_bytes(&bytes).is_err(), "{name}");
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("synthetic.epub");
            fs::write(&input, bytes).unwrap();
            assert!(
                extract_file(&input, &dir.path().join("out")).is_err(),
                "{name}"
            );
        }
        for name in ["OPS/./p.xhtml", "OPS//p.xhtml", "OPS/a/../p.xhtml"] {
            assert!(safe_path(name).is_err(), "{name}");
            let bytes = book(
                &opf("ltr", ""),
                &[("OPS/p.xhtml", PAGE), (name, b"replacement")],
            );
            assert!(inspect_bytes(&bytes).is_err(), "{name}");
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("synthetic.epub");
            fs::write(&input, bytes).unwrap();
            let out = dir.path().join("out");
            assert!(extract_file(&input, &out).is_err(), "{name}");
            assert_eq!(fs::read(out.join("OPS/p.xhtml")).unwrap(), PAGE);
        }
        // A directory alias cannot replace an existing file.
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("synthetic.epub");
        fs::write(&input, synthetic_zip(&[("OPS", b"keep"), ("OPS/./", b"")])).unwrap();
        let out = dir.path().join("out");
        assert!(extract_file(&input, &out).is_err());
        assert_eq!(fs::read(out.join("OPS")).unwrap(), b"keep");
    }
    #[test]
    fn absolute_reference_selects_root_and_generated_html_keeps_that_member() {
        let root_image = png(10, 20);
        let ops_image = png(30, 40);
        let opf = r#"<package><manifest><item id="p" href="/p.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#;
        let package = inspect_bytes(&book(
            opf,
            &[
                (
                    "p.xhtml",
                    br#"<html><body><img src="/x.png?v=1&amp;b=2#view"/></body></html>"#,
                ),
                ("OPS/p.xhtml", b"<html><body>different page</body></html>"),
                ("x.png", &root_image),
                ("OPS/x.png", &ops_image),
            ],
        ))
        .unwrap();
        let item = &package.spine[0];
        assert_eq!(item.path, "p.xhtml");
        assert_eq!(item.direct_image.as_deref(), Some("x.png"));
        assert_eq!((item.width, item.height), (Some(10), Some(20)));
        assert!(
            crate::render::print_html(&[item], false)
                .contains(r#"src="https://epub.invalid/x%2Epng?v=1&amp;b=2#view""#)
        );
        assert!(
            crate::render::print_html(&[item], true)
                .contains(r#"src="https://epub.invalid/p%2Exhtml""#)
        );
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("p.xhtml"),
            b"<html><head></head><body><img src=\"/x.png\"/></body></html>",
        )
        .unwrap();
        let copy = crate::render::reflow_print_copy(dir.path(), item, 0).unwrap();
        let content = fs::read_to_string(dir.path().join(&copy)).unwrap();
        assert!(content.contains("src=\"/x.png\""));
        assert_eq!(member_url(&copy).join("/x.png").unwrap().path(), "/x.png");
    }
    #[test]
    fn external_schemes_and_unc_do_not_map_to_local_members() {
        for href in [
            "//host/x",
            r"\\host/x",
            r"\/host/x",
            "C:/x",
            r"C:\x",
            "file:",
            "https:",
            "https://epub.invalid/x",
            "data:image/png;base64,AA",
            "a\t:foo",
        ] {
            assert!(is_external_href(href), "{href}");
            assert!(resolve("OPS/p.xhtml", href).is_err(), "{href}");
            // XML normalizes literal attribute TABs to spaces; a character reference
            // preserves the TAB that the URL parser then removes.
            let attribute = href.replace('\t', "&#9;");
            let page = format!(r#"<html><body><img src="{attribute}"/></body></html>"#);
            assert!(
                xhtml_info(page.as_bytes(), "OPS/p.xhtml")
                    .unwrap()
                    .1
                    .is_none(),
                "{href}"
            );
        }
    }
    #[test]
    fn package_metadata_uses_first_title_and_creator_for_pdf_info() {
        let opf = opf("ltr", "").replace(
            "</metadata>",
            "<dc:creator xmlns:dc=\"x\">最初の著者</dc:creator><dc:creator xmlns:dc=\"x\">後の著者</dc:creator><dc:title xmlns:dc=\"x\">後の題名</dc:title></metadata>",
        );
        let package = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap();
        assert_eq!(package.title.as_deref(), Some("Test"));
        assert_eq!(package.creator.as_deref(), Some("最初の著者"));
    }
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
    fn unicode_whitespace_external_manifest_items_stay_skipped() {
        for href in [
            "&#160;https://example.invalid/font.woff2",
            "&#8195;http://example.invalid/font.woff2&#160;",
            "&#12288;//example.invalid/font.woff2",
            "&#160;\\\\host/font.woff2",
            "&#160;C:/font.woff2",
            "&#160;file:/font.woff2",
            "&#160;data:font/woff2;base64,AA",
        ] {
            let opf = opf("ltr", "").replace(
                "</manifest>",
                &format!(r#"<item id="unused" href="{href}" media-type="font/woff2"/></manifest>"#),
            );
            let package = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap();
            assert_eq!(package.manifest.len(), 1, "{href}");
            assert_eq!(package.manifest[0].id, "p");
            assert_eq!(package.spine[0].path, "OPS/p.xhtml");
        }
        // Unicode whitespace is significant in local member names. Classification
        // must not feed its trimmed string back to the WHATWG resolver.
        let href = "\u{a0}p.xhtml";
        assert!(!is_external_href(href));
        assert_eq!(
            resolve("OPS/content.opf", href).unwrap().path,
            "OPS/\u{a0}p.xhtml"
        );
        let opf = opf("ltr", "").replace("href=\"p.xhtml\"", "href=\"&#160;p.xhtml\"");
        let package = inspect_bytes(&book(&opf, &[("OPS/\u{a0}p.xhtml", PAGE)])).unwrap();
        assert_eq!(package.spine[0].path, "OPS/\u{a0}p.xhtml");
    }
    #[test]
    fn unmappable_spine_references_are_errors() {
        for href in [
            "&#160;https://example.invalid/p.xhtml",
            "//host/p.xhtml",
            "C:/p.xhtml",
            "a%2Fp.xhtml",
            "../..//p.xhtml",
        ] {
            let opf = opf("ltr", "").replace("href=\"p.xhtml\"", &format!("href=\"{href}\""));
            let error = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap_err();
            assert_eq!(error.kind, EpubErrorKind::Invalid);
            assert!(
                error
                    .message
                    .contains("spine idref missing or external in manifest: p")
                    || error.message.contains("manifest item p href"),
                "{}",
                error.message
            );
        }
    }
    #[test]
    fn unused_cover_metadata_does_not_require_a_local_member() {
        for cover in [
            r#"<metadata/><manifest><item id="c" href="https://example.invalid/cover.jpg" properties="cover-image" media-type="image/jpeg"/>"#,
            r#"<metadata/><manifest><item id="c" href="&#160;https://example.invalid/cover.jpg" properties="cover-image" media-type="image/jpeg"/>"#,
            r#"<metadata><meta name="cover" content="c"/></metadata><manifest><item id="c" href="&#160;//host/cover.jpg" media-type="image/jpeg"/>"#,
            r#"<metadata/><manifest><item id="c" href="missing.jpg" properties="cover-image" media-type="image/jpeg"/>"#,
        ] {
            let opf = format!(
                r#"<package>{cover}<item id="p" href="p.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="p"/></spine></package>"#
            );
            let package = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap();
            assert_eq!(package.spine.len(), 1);
            assert_eq!(package.spine[0].idref, "p");
            assert_eq!(package.spine[0].path, "OPS/p.xhtml");
        }
        // A non-spine cover remains valid when its reference maps to a local member.
        let opf = opf("ltr", "").replace("</manifest>", r#"<item id="c" href="cover.jpg" properties="cover-image" media-type="image/jpeg"/></manifest>"#);
        assert!(
            inspect_bytes(&book(
                &opf,
                &[("OPS/p.xhtml", PAGE), ("OPS/cover.jpg", &png(10, 20))]
            ))
            .is_ok()
        );
    }
    #[test]
    fn stale_cover_meta_id_does_not_invalidate_local_spine() {
        let opf = opf("ltr", "").replace(
            "</metadata>",
            r#"<meta name="cover" content="obsolete-id"/></metadata>"#,
        );
        let package = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap();
        assert!(!package.manifest.iter().any(|item| item.id == "obsolete-id"));
        assert_eq!(package.spine.len(), 1);
        assert_eq!(package.spine[0].path, "OPS/p.xhtml");
    }
    #[test]
    fn external_cover_item_in_spine_still_fails() {
        let opf = opf("ltr", "").replace(
            r#"href="p.xhtml""#,
            r#"href="https://example.invalid/cover.jpg" properties="cover-image""#,
        );
        let error = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap_err();
        assert_eq!(error.kind, EpubErrorKind::Invalid);
        assert_eq!(
            error.message,
            "spine idref missing or external in manifest: p"
        );
    }
    #[test]
    fn unsafe_local_cover_href_still_fails() {
        let opf = opf("ltr", "").replace("</manifest>", r#"<item id="c" href="a%2Fb.jpg" properties="cover-image" media-type="image/jpeg"/></manifest>"#);
        let error = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap_err();
        assert!(error.message.contains("manifest item c href"));
        assert!(error.message.contains("encoded separator"));
    }
    #[test]
    fn unsafe_unused_local_manifest_items_are_not_silently_skipped() {
        for href in [
            "a%2Fb.woff2",
            "a%5Cb.woff2",
            "../../font.woff2",
            "a%3Ab.woff2",
            "NUL.woff2",
        ] {
            let opf = opf("ltr", "").replace(
                "</manifest>",
                &format!(r#"<item id="unused" href="{href}" media-type="font/woff2"/></manifest>"#),
            );
            let error = inspect_bytes(&book(&opf, &[("OPS/p.xhtml", PAGE)])).unwrap_err();
            assert!(
                error.message.contains("manifest item unused href"),
                "{href}: {}",
                error.message
            );
        }
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
