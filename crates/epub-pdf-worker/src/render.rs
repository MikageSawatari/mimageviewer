use crate::package::{Package, SpineItem};
use lopdf::{Document, Object, ObjectId, StringFormat, dictionary};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub const REFLOW_PROFILE: &str = "reflow-v1";
pub const REFLOW_WIDTH: u32 = 720;
pub const REFLOW_HEIGHT: u32 = 1024;
pub const REFLOW_MARGIN: u32 = 32;
const VIRTUAL_HOST: &str = "epub.invalid";

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub kind: String,
    pub first_spine: usize,
    pub spine_count: usize,
    pub output_pages: usize,
    pub print_ms: u128,
    pub chunks: usize,
    pub method: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct PdfPage {
    pub number: usize,
    pub width_pt: f64,
    pub height_pt: f64,
    pub size_source: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct PdfImage {
    pub filter: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub sha256: String,
}
fn sha256(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn segments(package: &Package) -> Vec<(usize, Vec<&SpineItem>)> {
    let mut result: Vec<(usize, Vec<&SpineItem>)> = Vec::new();
    for (n, item) in package.spine.iter().enumerate().filter(|(_, x)| x.linear) {
        let fixed = item.rendition.layout.as_deref() == Some("pre-paginated");
        if fixed
            && let Some((_, items)) = result.last_mut()
            && items[0].rendition.layout.as_deref() == Some("pre-paginated")
        {
            items.push(item);
            continue;
        }
        result.push((n, vec![item]));
    }
    result
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub fn print_html(items: &[&SpineItem], force_iframe: bool) -> String {
    let mut styles = String::new();
    let mut body = String::new();
    let mut names = HashMap::<(u32, u32), String>::new();
    for (index, item) in items.iter().enumerate() {
        let w = item.width.unwrap_or(1200);
        let h = item.height.unwrap_or(1700);
        let name = names
            .entry((w, h))
            .or_insert_with(|| {
                let n = format!("p{w}x{h}");
                styles.push_str(&format!("@page {n} {{size:{w}px {h}px;margin:0}}\n"));
                n
            })
            .clone();
        let url = if force_iframe {
            &item.url
        } else {
            item.direct_image
                .as_ref()
                .map(|image| &image.url)
                .unwrap_or(&item.url)
        };
        let src = escape(url.as_str());
        let content = if force_iframe || item.direct_image.is_none() {
            format!("<iframe src=\"{src}\" scrolling=\"no\"></iframe>")
        } else {
            format!("<img src=\"{src}\"/>")
        };
        body.push_str(&format!(
            "<section class=\"page {name}\" data-spine=\"{index}\">{content}</section>\n"
        ));
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{styles}html,body{{margin:0;padding:0}}section.page{{box-sizing:border-box;break-after:page;overflow:hidden}}section.page:last-child{{break-after:auto}}section.page img,section.page iframe{{display:block;width:100%;height:100%;border:0;object-fit:contain}}{}</style></head><body>{body}</body></html>",
        names
            .iter()
            .map(|((w, h), n)| format!("section.{n}{{page:{n};width:{w}px;height:{h}px}}"))
            .collect::<String>()
    )
}
pub fn reflow_print_copy(root: &Path, item: &SpineItem, index: usize) -> Result<String, String> {
    let source = root.join(&item.path);
    let data = fs::read_to_string(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let style = format!(
        "<style>@page{{size:{REFLOW_WIDTH}px {REFLOW_HEIGHT}px;margin:{REFLOW_MARGIN}px}}</style>"
    );
    let lower = data.to_ascii_lowercase();
    let content = if let Some(pos) = lower.find("</head>") {
        format!("{}{}{}", &data[..pos], style, &data[pos..])
    } else if let Some(pos) = lower.find("<body") {
        format!("{}<head>{style}</head>{}", &data[..pos], &data[pos..])
    } else {
        format!("{style}{data}")
    };
    let path = Path::new(&item.path);
    let parent = path.parent().unwrap_or(Path::new(""));
    let name = format!("_miv_reflow_{index}.xhtml");
    let out = root.join(parent).join(&name);
    fs::write(&out, content).map_err(|e| e.to_string())?;
    Ok(parent.join(name).to_string_lossy().replace('\\', "/"))
}
pub fn write_fixed_html(
    root: &Path,
    items: &[&SpineItem],
    force_iframe: bool,
    index: usize,
) -> Result<String, String> {
    let name = format!("_miv_print_{index}.html");
    fs::write(root.join(&name), print_html(items, force_iframe)).map_err(|e| e.to_string())?;
    Ok(name)
}
pub fn virtual_url(path: &str) -> String {
    crate::package::member_url(path).to_string()
}
pub fn reflow_url(path: &str, item: &SpineItem) -> String {
    let mut url = crate::package::member_url(path);
    url.set_query(item.url.query());
    url.set_fragment(item.url.fragment());
    url.to_string()
}
fn info_text(value: &str) -> Object {
    let mut bytes = vec![0xfe, 0xff];
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Literal)
}

fn internal_virtual_host_uri(uri: &[u8]) -> bool {
    let decoded;
    let uri = if uri.starts_with(&[0xfe, 0xff]) {
        if !(uri.len() - 2).is_multiple_of(2) {
            return false;
        }
        let units = uri[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let Ok(text) = String::from_utf16(&units) else {
            return false;
        };
        decoded = text;
        decoded.as_bytes()
    } else {
        uri
    };
    let authority = if let Some(rest) = uri.strip_prefix(b"//") {
        rest
    } else {
        let Some(marker) = uri.windows(3).position(|part| part == b"://") else {
            return false;
        };
        let scheme = &uri[..marker];
        if !scheme.first().is_some_and(u8::is_ascii_alphabetic)
            || !scheme
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'-' | b'.'))
        {
            return false;
        }
        &uri[marker + 3..]
    };
    let authority = authority
        .split(|byte| matches!(byte, b'/' | b'?' | b'#'))
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit(|byte| *byte == b'@')
        .next()
        .unwrap_or_default()
        .split(|byte| *byte == b':')
        .next()
        .unwrap_or_default();
    host.strip_suffix(b".")
        .unwrap_or(host)
        .eq_ignore_ascii_case(VIRTUAL_HOST.as_bytes())
}

fn internal_virtual_link(doc: &Document, annot: &Object) -> bool {
    let Ok((_, annot)) = doc.dereference(annot) else {
        return false;
    };
    let Ok(annot) = annot.as_dict() else {
        return false;
    };
    if annot.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Link") {
        return false;
    }
    let Ok(action) = annot.get(b"A") else {
        return false;
    };
    let Ok((_, action)) = doc.dereference(action) else {
        return false;
    };
    let Ok(action) = action.as_dict() else {
        return false;
    };
    if action.get(b"S").and_then(Object::as_name).ok() != Some(b"URI") {
        return false;
    }
    let Ok(uri) = action.get(b"URI") else {
        return false;
    };
    doc.dereference(uri)
        .ok()
        .and_then(|(_, uri)| uri.as_str().ok())
        .is_some_and(internal_virtual_host_uri)
}

fn remove_internal_virtual_links(doc: &mut Document) -> Result<bool, String> {
    let mut changed = false;
    for page_id in doc.get_pages().into_values() {
        let page = doc
            .get_object(page_id)
            .and_then(Object::as_dict)
            .map_err(|error| error.to_string())?;
        let Ok(annots) = page.get(b"Annots") else {
            continue;
        };
        let annots = annots.clone();
        let (array_id, array) = doc
            .dereference(&annots)
            .map_err(|error| error.to_string())?;
        let array = array.as_array().map_err(|error| error.to_string())?;
        let kept = array
            .iter()
            .filter(|annot| !internal_virtual_link(doc, annot))
            .cloned()
            .collect::<Vec<_>>();
        if kept.len() == array.len() {
            continue;
        }
        changed = true;
        if kept.is_empty() {
            doc.get_object_mut(page_id)
                .and_then(Object::as_dict_mut)
                .map_err(|error| error.to_string())?
                .remove(b"Annots");
        } else if let Some(array_id) = array_id {
            *doc.get_object_mut(array_id)
                .map_err(|error| error.to_string())? = Object::Array(kept);
        } else {
            doc.get_object_mut(page_id)
                .and_then(Object::as_dict_mut)
                .map_err(|error| error.to_string())?
                .set("Annots", kept);
        }
    }
    Ok(changed)
}

fn push_references(object: &Object, ids: &mut Vec<ObjectId>) {
    let mut pending = vec![object];
    while let Some(object) = pending.pop() {
        match object {
            Object::Reference(id) => ids.push(*id),
            Object::Array(items) => pending.extend(items),
            Object::Dictionary(dict) => pending.extend(dict.iter().map(|(_, value)| value)),
            Object::Stream(stream) => {
                pending.extend(stream.dict.iter().map(|(_, value)| value));
            }
            _ => {}
        }
    }
}

fn prune_unreachable_after_link_removal(doc: &mut Document) {
    let mut pending = Vec::new();
    for (_, value) in doc.trailer.iter() {
        push_references(value, &mut pending);
    }
    let mut reachable = HashSet::new();
    while let Some(id) = pending.pop() {
        if reachable.insert(id)
            && let Some(object) = doc.objects.get(&id)
        {
            push_references(object, &mut pending);
        }
    }
    doc.objects.retain(|id, _| reachable.contains(id));
}

pub fn merge_pdf(
    files: &[PathBuf],
    out: &Path,
    rtl: bool,
    title: &str,
    author: Option<&str>,
) -> Result<(), String> {
    if files.is_empty() {
        return Err("no printed PDF segments".into());
    }
    let mut merged = Document::with_version("1.7");
    let mut roots = Vec::new();
    let mut count = 0;
    let mut removed_internal_links = false;
    for file in files {
        let mut doc = Document::load(file).map_err(|e| format!("{}: {e}", file.display()))?;
        removed_internal_links |= remove_internal_virtual_links(&mut doc)?;
        doc.renumber_objects_with(merged.max_id + 1);
        merged.max_id = doc.max_id;
        let catalog = doc
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|e| e.to_string())?;
        // The printed part's Info is unrelated to its pages; never embed its
        // Chromium title as an orphan object in the merged PDF.
        let printed_info = doc.trailer.get(b"Info").and_then(Object::as_reference).ok();
        let pages = doc
            .get_object(catalog)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Pages"))
            .and_then(Object::as_reference)
            .map_err(|e| e.to_string())?;
        roots.push(pages);
        count += doc.get_pages().len();
        for (id, obj) in doc.objects {
            if id != catalog && Some(id) != printed_info {
                merged.objects.insert(id, obj);
            }
        }
    }
    let new_pages = merged.new_object_id();
    for root in &roots {
        merged
            .get_object_mut(*root)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?
            .set("Parent", new_pages);
    }
    merged.objects.insert(new_pages,Object::Dictionary(dictionary!{"Type"=>"Pages","Kids"=>roots.iter().map(|r|Object::Reference(*r)).collect::<Vec<_>>(),"Count"=>count as i64}));
    let new_catalog = merged.new_object_id();
    let mut catalog = dictionary! {"Type"=>"Catalog","Pages"=>new_pages};
    if rtl {
        catalog.set("ViewerPreferences", dictionary! {"Direction"=>"R2L"});
    }
    merged
        .objects
        .insert(new_catalog, Object::Dictionary(catalog));
    merged.trailer.set("Root", new_catalog);
    let mut info = lopdf::Dictionary::new();
    info.set("Title", info_text(title));
    if let Some(author) = author {
        info.set("Author", info_text(author));
    }
    let info_id = merged.add_object(info);
    merged.trailer.set("Info", info_id);
    if removed_internal_links {
        // Prune after replacing the source catalogs too, so no dropped source
        // structure can retain an internal Link or its /A action dictionary.
        prune_unreachable_after_link_removal(&mut merged);
    }
    merged.save(out).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn merge_pdf_for_package(
    files: &[PathBuf],
    out: &Path,
    package: &crate::package::Package,
    input: &Path,
    source_stem: Option<&str>,
) -> Result<(), String> {
    let fallback: Cow<'_, str> = source_stem
        .filter(|stem| !stem.is_empty())
        .map(Cow::Borrowed)
        .or_else(|| {
            input
                .file_stem()
                .filter(|stem| !stem.is_empty())
                .map(|stem| stem.to_string_lossy())
        })
        .unwrap_or(Cow::Borrowed("EPUB"));
    let title = package
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(fallback.as_ref());
    merge_pdf(
        files,
        out,
        package.direction == "rtl",
        title,
        package
            .creator
            .as_deref()
            .filter(|author| !author.trim().is_empty()),
    )
}
pub fn inspect_pdf(
    path: &Path,
    source: &[Option<String>],
) -> Result<(Vec<PdfPage>, Vec<PdfImage>), String> {
    let doc = Document::load(path).map_err(|e| e.to_string())?;
    let mut pages = Vec::new();
    for (index, (_, id)) in doc.get_pages().iter().enumerate() {
        let mut box_obj = None;
        let mut current = *id;
        for _ in 0..12 {
            let dict = doc
                .get_object(current)
                .and_then(Object::as_dict)
                .map_err(|e| e.to_string())?;
            if let Ok(b) = dict.get(b"MediaBox") {
                box_obj = Some(b.clone());
                break;
            }
            current = match dict.get(b"Parent").and_then(Object::as_reference) {
                Ok(p) => p,
                Err(_) => break,
            };
        }
        let b = box_obj.ok_or("PDF page lacks MediaBox")?;
        let a = b.as_array().map_err(|e| e.to_string())?;
        let get = |n: usize| -> Result<f64, String> {
            match doc.dereference(&a[n]).map_err(|e| e.to_string())?.1 {
                Object::Integer(v) => Ok(*v as f64),
                Object::Real(v) => Ok(*v as f64),
                _ => Err("invalid MediaBox".into()),
            }
        };
        pages.push(PdfPage {
            number: index + 1,
            width_pt: get(2)? - get(0)?,
            height_pt: get(3)? - get(1)?,
            size_source: source.get(index).cloned().flatten(),
        });
    }
    let mut images = Vec::new();
    for obj in doc.objects.values() {
        if let Object::Stream(s) = obj {
            if s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image") {
                continue;
            }
            let width = s
                .dict
                .get(b"Width")
                .and_then(Object::as_i64)
                .unwrap_or(0)
                .max(0) as u32;
            let height = s
                .dict
                .get(b"Height")
                .and_then(Object::as_i64)
                .unwrap_or(0)
                .max(0) as u32;
            let filter = s
                .dict
                .get(b"Filter")
                .ok()
                .map(|v| match v {
                    Object::Name(x) => String::from_utf8_lossy(x).into_owned(),
                    Object::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_name().ok())
                        .map(|x| String::from_utf8_lossy(x).into_owned())
                        .collect::<Vec<_>>()
                        .join("+"),
                    _ => "other".into(),
                })
                .unwrap_or_else(|| "none".into());
            images.push(PdfImage {
                filter,
                width,
                height,
                bytes: s.content.len(),
                sha256: sha256(&s.content),
            });
        }
    }
    Ok((pages, images))
}
pub fn image_verdict(source: &[(String, u32, u32, usize, String)], pdf: &[PdfImage]) -> String {
    if source.is_empty() {
        return "no_source_images".into();
    }
    let mut exact = 0;
    let mut dct_similar = 0;
    let mut lossless = 0;
    let mut bad = 0;
    for (kind, w, h, len, hash) in source {
        let mut candidates = pdf.iter().filter(|p| p.width == *w && p.height == *h);
        if kind == "jpeg" {
            if candidates
                .clone()
                .any(|p| p.filter.contains("DCTDecode") && p.sha256 == *hash)
            {
                exact += 1
            } else if candidates.clone().any(|p| {
                p.filter.contains("DCTDecode")
                    && (*len as f64 * 0.8..=*len as f64 * 1.2).contains(&(p.bytes as f64))
            }) {
                dct_similar += 1
            } else {
                bad += 1
            }
        } else if kind == "png" {
            if candidates.any(|p| p.filter.contains("FlateDecode")) {
                lossless += 1
            } else {
                bad += 1
            }
        }
    }
    format!(
        "jpeg_exact={exact}; jpeg_dct_similar={dct_similar}; png_flate={lossless}; unmatched={bad}"
    )
}
pub fn source_images(package: &Package, root: &Path) -> Vec<(String, u32, u32, usize, String)> {
    let mut out = Vec::new();
    for item in &package.manifest {
        let kind = if item.media_type == "image/jpeg" {
            "jpeg"
        } else if item.media_type == "image/png" {
            "png"
        } else {
            continue;
        };
        let path = root.join(&item.path);
        if let Ok(data) = fs::read(&path)
            && let Ok((w, h)) = image::image_dimensions(&path)
        {
            out.push((kind.into(), w, h, data.len(), sha256(&data)));
        }
    }
    out
}
pub fn pdf_sizes(path: &Path) -> Result<Vec<(f64, f64)>, String> {
    Ok(inspect_pdf(path, &[])?
        .0
        .into_iter()
        .map(|p| (p.width_pt, p.height_pt))
        .collect())
}
pub fn summarize(reports: &[crate::Report]) -> String {
    let successes = reports.iter().filter(|r| r.exit_code == 0).count();
    let mut s = format!(
        "# EPUB → PDF batch\n\n{} books; {} converted; {} failed.\n\n| File | Status | Spine | PDF pages | Time (s) | Bytes ratio | Image fidelity | Error |\n|---|---|---:|---:|---:|---:|---|---|\n",
        reports.len(),
        successes,
        reports.len() - successes
    );
    for r in reports {
        let ratio = match r.output_bytes {
            Some(o) if r.input_bytes > 0 => format!("{:.2}", o as f64 / r.input_bytes as f64),
            _ => "—".into(),
        };
        let path = Path::new(&r.file);
        let file = format!(
            "{}/{}",
            path.parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy(),
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} | {} | {} | {} |\n",
            file.replace('|', "/"),
            r.status,
            r.spine_count.unwrap_or(0),
            r.output_page_count.unwrap_or(0),
            r.timings.get("total_ms").copied().unwrap_or(0) as f64 / 1000.0,
            ratio,
            r.image_fidelity.as_deref().unwrap_or("not_measured"),
            r.errors
                .first()
                .map_or("—", String::as_str)
                .replace('|', "/")
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_sources_encode_real_names_before_html_escaping() {
        let mut package = package_with_metadata("");
        let item = &mut package.spine[0];
        item.path = "OPS/a#% 日本語.xhtml".into();
        item.url = crate::package::member_url(&item.path);
        item.url.set_query(Some("v=1&b=2"));
        item.url.set_fragment(Some("page"));
        item.direct_image = Some(crate::package::MemberReference {
            path: "images/a#% b.svg".into(),
            url: {
                let mut url = crate::package::member_url("images/a#% b.svg");
                url.set_fragment(Some("view"));
                url
            },
        });
        assert!(print_html(&[item], true).contains(r#"src="https://epub.invalid/OPS/a%23%25%20%E6%97%A5%E6%9C%AC%E8%AA%9E%2Exhtml?v=1&amp;b=2#page""#));
        assert!(
            print_html(&[item], false)
                .contains(r#"src="https://epub.invalid/images/a%23%25%20b%2Esvg#view""#)
        );
        assert_eq!(
            virtual_url("OPS/a%20b.xhtml"),
            "https://epub.invalid/OPS/a%2520b%2Exhtml"
        );
        assert_eq!(
            reflow_url("OPS/#% _miv_reflow_0.xhtml", item),
            "https://epub.invalid/OPS/%23%25%20%5Fmiv%5Freflow%5F0%2Exhtml?v=1&b=2#page"
        );
        // The report remains compatible: URLs are internal rendering metadata.
        let report = serde_json::to_value(item).unwrap();
        assert_eq!(report["path"], "OPS/a#% 日本語.xhtml");
        assert_eq!(report["direct_image"], "images/a#% b.svg");
        assert!(report.get("url").is_none());
    }
    fn package_with_metadata(metadata: &str) -> Package {
        use std::io::{Cursor, Write};
        use zip::{ZipWriter, write::SimpleFileOptions};
        let opf = format!(
            r#"<package><metadata>{metadata}</metadata><manifest><item id="p" href="p.xhtml" media-type="application/xhtml+xml"/></manifest><spine page-progression-direction="rtl"><itemref idref="p"/></spine></package>"#
        );
        let mut epub = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut epub);
            for (name, bytes) in [
                ("META-INF/container.xml", br#"<container><rootfiles><rootfile full-path="OPS/content.opf"/></rootfiles></container>"#.as_slice()),
                ("OPS/content.opf", opf.as_bytes()),
                ("OPS/p.xhtml", br#"<html><body>page</body></html>"#.as_slice()),
            ] {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        crate::package::inspect_bytes(&epub.into_inner()).unwrap()
    }
    fn one_page(path: &Path, width: i64, height: i64) {
        let mut d = Document::with_version("1.7");
        let pages = d.new_object_id();
        let page = d.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "MediaBox" => vec![0.into(),0.into(),width.into(),height.into()],
            "Resources" => dictionary! {}
        });
        d.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1
            }),
        );
        let catalog = d.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        d.trailer.set("Root", catalog);
        d.save(path).unwrap();
    }
    fn printed_page(path: &Path, title: &str) {
        one_page(path, 300, 400);
        let mut doc = Document::load(path).unwrap();
        let info = doc.add_object(dictionary! {
            "Title" => title,
            "Author" => "Chromium-generated author",
            "Creator" => "Chromium printToPDF"
        });
        doc.trailer.set("Info", info);
        doc.save(path).unwrap();
    }
    fn linked_page(path: &Path, links: &[(&str, bool)], indirect_annots: bool) {
        one_page(path, 300, 400);
        let mut doc = Document::load(path).unwrap();
        let page = doc.get_pages()[&1];
        let content = doc.add_object(lopdf::Stream::new(dictionary! {}, b"q Q".to_vec()));
        let mut annots = Vec::new();
        for (uri, indirect_action) in links {
            let action = dictionary! {
                "Type" => "Action", "S" => "URI",
                "URI" => Object::String(uri.as_bytes().to_vec(), StringFormat::Literal)
            };
            let action = if *indirect_action {
                Object::Reference(doc.add_object(action))
            } else {
                Object::Dictionary(action)
            };
            let annot = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link",
                "Rect" => vec![0.into(), 0.into(), 100.into(), 20.into()],
                "A" => action
            });
            annots.push(Object::Reference(annot));
        }
        let annots = if indirect_annots {
            Object::Reference(doc.add_object(Object::Array(annots)))
        } else {
            Object::Array(annots)
        };
        doc.get_object_mut(page)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Annots", annots);
        doc.get_object_mut(page)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", content);
        doc.save(path).unwrap();
    }
    fn link_uris(path: &Path) -> Vec<String> {
        let doc = Document::load(path).unwrap();
        let page = doc.get_pages()[&1];
        let page = doc.get_object(page).unwrap().as_dict().unwrap();
        let Ok(annots) = page.get(b"Annots") else {
            return Vec::new();
        };
        let (_, annots) = doc.dereference(annots).unwrap();
        annots
            .as_array()
            .unwrap()
            .iter()
            .map(|annot| {
                let (_, annot) = doc.dereference(annot).unwrap();
                let action = annot.as_dict().unwrap().get(b"A").unwrap();
                let (_, action) = doc.dereference(action).unwrap();
                String::from_utf8(
                    action
                        .as_dict()
                        .unwrap()
                        .get(b"URI")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_vec(),
                )
                .unwrap()
            })
            .collect()
    }
    #[test]
    fn merge_removes_internal_uri_links_and_their_actions() {
        let dir =
            std::env::temp_dir().join(format!("epub-pdf-internal-links-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("part.pdf");
        let out = dir.join("merged.pdf");
        linked_page(
            &part,
            &[
                (
                    "https://epub.invalid/OEBPS/xhtml/introduction.xhtml#start",
                    false,
                ),
                ("https://epub.invalid:443/OEBPS/xhtml/next.xhtml", true),
            ],
            true,
        );
        merge_pdf(std::slice::from_ref(&part), &out, false, "Book", None).unwrap();
        assert!(link_uris(&out).is_empty());
        let merged = Document::load(&out).unwrap();
        assert_eq!(
            merged.get_page_content(merged.get_pages()[&1]).unwrap(),
            b"q Q"
        );
        assert!(
            !fs::read(&out)
                .unwrap()
                .windows(b"epub.invalid".len())
                .any(|part| part == b"epub.invalid")
        );
        fs::remove_file(part).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn merge_keeps_external_uri_links() {
        let dir =
            std::env::temp_dir().join(format!("epub-pdf-external-links-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("part.pdf");
        let out = dir.join("merged.pdf");
        let links = [
            ("https://example.com/?next=epub.invalid", false),
            ("https://epub.invalid/OPS/chapter.xhtml#top", false),
            ("https://epub.invalid.evil/book", true),
        ];
        linked_page(&part, &links, false);
        merge_pdf(std::slice::from_ref(&part), &out, false, "Book", None).unwrap();
        assert_eq!(
            link_uris(&out),
            [links[0].0.to_owned(), links[2].0.to_owned()]
        );
        fs::remove_file(part).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    fn assert_no_print_url_in_info(path: &Path) {
        let doc = Document::load(path).unwrap();
        let titles = doc
            .objects
            .values()
            .filter_map(|object| object.as_dict().ok())
            .filter_map(|dict| dict.get(b"Title").ok())
            .filter_map(|value| value.as_str().ok())
            .collect::<Vec<_>>();
        assert_eq!(
            titles.len(),
            1,
            "printed parts left their Info dictionaries behind"
        );
        for title in titles {
            assert!(
                !title
                    .windows(b"epub.invalid".len())
                    .any(|part| part == b"epub.invalid")
            );
        }
        // lopdf ignores unreachable objects while loading, but the bytes of an
        // imported Chromium Info dictionary can still be present in the PDF.
        let raw = fs::read(path).unwrap();
        for leaked in [
            b"epub.invalid".as_slice(),
            b"Chromium-generated author".as_slice(),
            b"Chromium printToPDF".as_slice(),
        ] {
            assert!(!raw.windows(leaked.len()).any(|part| part == leaked));
        }
    }
    #[test]
    fn merge_fixed_print_parts_drop_chromium_info() {
        let dir = std::env::temp_dir().join(format!("epub-pdf-fixed-info-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("fixed-1.pdf");
        let b = dir.join("fixed-2.pdf");
        let out = dir.join("merged.pdf");
        printed_page(&a, "epub.invalid/_miv_print_1%2Ehtml");
        printed_page(&b, "epub.invalid/_miv_print_2%2Ehtml");
        let package = package_with_metadata(
            r#"<dc:title xmlns:dc="x">本の題名</dc:title><dc:creator xmlns:dc="x">著者</dc:creator><meta property="rendition:layout">pre-paginated</meta>"#,
        );
        merge_pdf_for_package(
            &[a.clone(), b.clone()],
            &out,
            &package,
            Path::new("book.epub"),
            None,
        )
        .unwrap();
        let doc = Document::load(&out).unwrap();
        let info = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = doc.get_object(info).unwrap().as_dict().unwrap();
        assert_eq!(info_text_value(info.get(b"Title").unwrap()), "本の題名");
        assert_eq!(info_text_value(info.get(b"Author").unwrap()), "著者");
        assert_no_print_url_in_info(&out);
        for file in [a, b, out] {
            fs::remove_file(file).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
    fn info_text_value(value: &Object) -> String {
        let bytes = value.as_str().unwrap();
        assert_eq!(&bytes[..2], &[0xfe, 0xff]);
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units).unwrap()
    }
    #[test]
    fn merge_without_opf_title_uses_epub_stem_and_no_author() {
        let dir = std::env::temp_dir().join(format!("epub-pdf-stem-info-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("part.pdf");
        let out = dir.join("saved.pdf");
        printed_page(&part, "epub.invalid/_miv_print_1%2Ehtml");
        let package = package_with_metadata("");
        merge_pdf_for_package(
            std::slice::from_ref(&part),
            &out,
            &package,
            Path::new("書名なし.epub"),
            None,
        )
        .unwrap();
        let doc = Document::load(&out).unwrap();
        let info = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = doc.get_object(info).unwrap().as_dict().unwrap();
        assert_eq!(info_text_value(info.get(b"Title").unwrap()), "書名なし");
        assert!(info.get(b"Author").is_err());
        assert_no_print_url_in_info(&out);
        fs::remove_file(part).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn merge_without_opf_title_uses_original_stem_when_input_is_source_copy() {
        let dir =
            std::env::temp_dir().join(format!("epub-pdf-original-stem-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("part.pdf");
        let out = dir.join("saved.pdf");
        printed_page(&part, "epub.invalid/_miv_print_1%2Ehtml");
        merge_pdf_for_package(
            &[part.clone()],
            &out,
            &package_with_metadata(""),
            Path::new("source.epub"),
            Some("MyNovel"),
        )
        .unwrap();
        let doc = Document::load(&out).unwrap();
        let info = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = doc.get_object(info).unwrap().as_dict().unwrap();
        assert_eq!(info_text_value(info.get(b"Title").unwrap()), "MyNovel");
        assert_no_print_url_in_info(&out);
        fs::remove_file(part).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn merge_keeps_order_boxes_and_rtl() {
        let dir = std::env::temp_dir().join(format!("epub-pdf-merge-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        let b = dir.join("b.pdf");
        let out = dir.join("merged.pdf");
        one_page(&a, 300, 400);
        one_page(&b, 500, 600);
        merge_pdf(&[a.clone(), b.clone()], &out, true, "書名", Some("著者")).unwrap();
        assert_eq!(
            pdf_sizes(&out).unwrap(),
            vec![(300.0, 400.0), (500.0, 600.0)]
        );
        let d = Document::load(&out).unwrap();
        let root = d.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let catalog = d.get_object(root).unwrap().as_dict().unwrap();
        let prefs = catalog
            .get(b"ViewerPreferences")
            .unwrap()
            .as_dict()
            .unwrap();
        assert_eq!(prefs.get(b"Direction").unwrap().as_name().unwrap(), b"R2L");
        let info_id = d.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = d.get_object(info_id).unwrap().as_dict().unwrap();
        for (key, expected) in [
            (b"Title".as_slice(), "書名"),
            (b"Author".as_slice(), "著者"),
        ] {
            let bytes = info.get(key).unwrap().as_str().unwrap();
            let units = bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            assert_eq!(&bytes[..2], &[0xfe, 0xff]);
            assert_eq!(String::from_utf16(&units).unwrap(), expected);
        }
        fs::remove_file(a).unwrap();
        fs::remove_file(b).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn merge_uses_title_and_first_creator_from_opf() {
        let package = package_with_metadata(
            r#"<dc:title xmlns:dc="x">本の題名</dc:title><dc:creator xmlns:dc="x">第一著者</dc:creator><dc:creator xmlns:dc="x">第二著者</dc:creator>"#,
        );
        let dir =
            std::env::temp_dir().join(format!("epub-pdf-package-info-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("page.pdf");
        let out = dir.join("book.pdf");
        one_page(&part, 300, 400);
        merge_pdf_for_package(
            std::slice::from_ref(&part),
            &out,
            &package,
            Path::new("book.epub"),
            None,
        )
        .unwrap();
        let doc = Document::load(&out).unwrap();
        let info_id = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = doc.get_object(info_id).unwrap().as_dict().unwrap();
        for (key, expected) in [
            (b"Title".as_slice(), "本の題名"),
            (b"Author".as_slice(), "第一著者"),
        ] {
            let bytes = info.get(key).unwrap().as_str().unwrap();
            assert_eq!(&bytes[..2], &[0xfe, 0xff]);
            let utf16: Vec<_> = bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            assert_eq!(String::from_utf16(&utf16).unwrap(), expected);
        }
        fs::remove_file(part).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
