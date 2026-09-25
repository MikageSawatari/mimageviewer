use crate::render::{PdfImage, PdfPage, Segment};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct Report {
    pub file: String,
    pub status: String,
    pub exit_code: i32,
    pub title: Option<String>,
    pub layout: Option<String>,
    pub direction: Option<String>,
    pub profile: String,
    pub blocked_requests: Vec<String>,
    pub blocked_request_count: usize,
    pub book_script_ran: bool,
    pub spine_count: Option<usize>,
    pub output_page_count: Option<usize>,
    pub pdf_pages: Vec<PdfPage>,
    pub segments: Vec<Segment>,
    pub timings: BTreeMap<String, u128>,
    pub input_bytes: u64,
    pub output_bytes: Option<u64>,
    pub drm: String,
    pub errors: Vec<String>,
    pub source_images: Vec<SourceImage>,
    pub pdf_images: Vec<PdfImage>,
    pub image_fidelity: Option<String>,
    pub webview_runtime: Option<String>,
    pub web_resource_filter: Option<String>,
    pub user_data_folder_redirected: Option<String>,
    pub user_data_folder_check_error: Option<String>,
    pub print_to_pdf_comparison: Option<String>,
    pub user_data_cleanup: Option<UserDataCleanup>,
}
#[derive(Debug, Serialize)]
pub struct SourceImage {
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct UserDataCleanup {
    pub path: String,
    pub deleted: bool,
    pub held_ms: u128,
    pub error: Option<String>,
}
impl Report {
    pub fn new(path: &std::path::Path) -> Self {
        Self {
            file: path.display().to_string(),
            status: "pending".into(),
            exit_code: 0,
            title: None,
            layout: None,
            direction: None,
            profile: crate::render::REFLOW_PROFILE.into(),
            blocked_requests: Vec::new(),
            blocked_request_count: 0,
            book_script_ran: false,
            spine_count: None,
            output_page_count: None,
            pdf_pages: Vec::new(),
            segments: Vec::new(),
            timings: BTreeMap::new(),
            input_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            output_bytes: None,
            drm: "not_checked".into(),
            errors: Vec::new(),
            source_images: Vec::new(),
            pdf_images: Vec::new(),
            image_fidelity: None,
            webview_runtime: None,
            web_resource_filter: None,
            user_data_folder_redirected: None,
            user_data_folder_check_error: None,
            print_to_pdf_comparison: None,
            user_data_cleanup: None,
        }
    }
    pub fn fail(&mut self, code: i32, error: impl Into<String>) {
        self.exit_code = code;
        self.status = match code {
            2 => "drm",
            3 => "invalid_epub",
            4 => "runtime_missing",
            8 => "webview2_unsupported",
            6 => "timeout",
            _ => "render_failure",
        }
        .into();
        self.errors.push(error.into());
    }
}
