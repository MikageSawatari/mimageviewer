use crate::package::{Package, SpineItem};
use lopdf::{Document, Object, dictionary};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

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
        let path = if force_iframe {
            &item.path
        } else {
            item.direct_image.as_ref().unwrap_or(&item.path)
        };
        let src = escape(path);
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
    let style =
        "<style>@page{size:1200px 1700px;margin:48px}html,body{print-color-adjust:exact}</style>";
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
    // Encode path segments for browser URLs while retaining directory separators.
    let encoded = path
        .split('/')
        .map(|s| {
            percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("https://epub.invalid/{encoded}")
}
pub fn merge_pdf(files: &[PathBuf], out: &Path, rtl: bool) -> Result<(), String> {
    if files.is_empty() {
        return Err("no printed PDF segments".into());
    }
    let mut merged = Document::with_version("1.7");
    let mut roots = Vec::new();
    let mut count = 0;
    for file in files {
        let mut doc = Document::load(file).map_err(|e| format!("{}: {e}", file.display()))?;
        doc.renumber_objects_with(merged.max_id + 1);
        merged.max_id = doc.max_id;
        let catalog = doc
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|e| e.to_string())?;
        let pages = doc
            .get_object(catalog)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Pages"))
            .and_then(Object::as_reference)
            .map_err(|e| e.to_string())?;
        roots.push(pages);
        count += doc.get_pages().len();
        for (id, obj) in doc.objects {
            if id != catalog {
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
    merged.save(out).map_err(|e| e.to_string())?;
    Ok(())
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
    #[test]
    fn merge_keeps_order_boxes_and_rtl() {
        let dir = std::env::temp_dir().join(format!("epub-pdf-merge-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        let b = dir.join("b.pdf");
        let out = dir.join("merged.pdf");
        one_page(&a, 300, 400);
        one_page(&b, 500, 600);
        merge_pdf(&[a.clone(), b.clone()], &out, true).unwrap();
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
        fs::remove_file(a).unwrap();
        fs::remove_file(b).unwrap();
        fs::remove_file(out).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
