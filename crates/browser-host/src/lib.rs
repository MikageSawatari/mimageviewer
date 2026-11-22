//! One-shot native host. All filesystem/process effects occur outside the GUI process.
use mimageviewer_browser_view_ipc::Placement;
use mimageviewer_capture_file::{
    CaptureOrigin, CaptureTimestamp, capture_filename, is_browser_host_destination, sanitize_url,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub mod image_bytes;
pub mod positions;
pub mod protocol;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidOrigin,
    UnsupportedVersion,
    InvalidRequest,
    MessageTooLarge,
    InvalidImage,
    InvalidUrl,
    InvalidPlacement,
    RegistrationMissing,
    LauncherMissing,
    InvalidDestination,
    SaveFailed,
    MotwFailed,
    LaunchFailed,
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub v: u32,
    pub launcher: PathBuf,
    pub output_dir: PathBuf,
}
#[derive(Debug, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}
impl Response {
    pub fn failure(error: ErrorCode, path: Option<PathBuf>) -> Self {
        Self {
            ok: false,
            error: Some(error),
            path,
        }
    }
}

pub fn read_config(executable: &Path) -> Result<(Config, PathBuf), ErrorCode> {
    let generation = executable.parent().ok_or(ErrorCode::RegistrationMissing)?;
    let managed = generation
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("browser-host"))
        })
        .ok_or(ErrorCode::RegistrationMissing)?;
    let file = std::fs::File::open(generation.join("config.json"))
        .map_err(|_| ErrorCode::RegistrationMissing)?;
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::RegistrationMissing)?;
    if bytes.len() > 65_536 {
        return Err(ErrorCode::RegistrationMissing);
    }
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|_| ErrorCode::RegistrationMissing)?;
    if config.v != 1 || !config.launcher.is_absolute() || !config.output_dir.is_absolute() {
        return Err(ErrorCode::RegistrationMissing);
    }
    Ok((config, managed.to_path_buf()))
}

pub trait Effects {
    fn motw(&mut self, path: &Path, origin: &CaptureOrigin) -> Result<(), ErrorCode>;
    fn launch(&mut self, command: &mut Command) -> Result<(), ErrorCode>;
}
pub struct NativeEffects;
impl Effects for NativeEffects {
    fn motw(&mut self, path: &Path, origin: &CaptureOrigin) -> Result<(), ErrorCode> {
        mimageviewer_capture_file::write_motw(path, origin).map_err(|_| ErrorCode::MotwFailed)
    }
    fn launch(&mut self, command: &mut Command) -> Result<(), ErrorCode> {
        let mut child = command.spawn().map_err(|_| ErrorCode::LaunchFailed)?;
        #[cfg(windows)]
        mimageviewer_browser_view_ipc::grant_child_foreground(
            child.id(),
            mimageviewer_browser_view_ipc::transport::allow_foreground,
            |line| eprintln!("{line}"),
        );
        if child.wait().map_err(|_| ErrorCode::LaunchFailed)?.success() {
            Ok(())
        } else {
            Err(ErrorCode::LaunchFailed)
        }
    }
}
pub fn launcher_command(launcher: &Path, placement: Placement, image: &Path) -> Command {
    let mut command = Command::new(launcher);
    command
        .arg(mimageviewer_browser_view_ipc::CLI_FLAG)
        .arg(placement.cli_value())
        .arg("--")
        .arg(image)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub fn handle(
    config: &Config,
    managed: &Path,
    request: protocol::Request,
    timestamp: &CaptureTimestamp,
    effects: &mut impl Effects,
) -> Response {
    let mut saved = None;
    let result = (|| -> Result<(), ErrorCode> {
        if request.v != 1 {
            return Err(ErrorCode::UnsupportedVersion);
        }
        if request.kind != "open_bytes" {
            return Err(ErrorCode::InvalidRequest);
        }
        if config.v != 1 || !config.launcher.is_absolute() || !config.output_dir.is_absolute() {
            return Err(ErrorCode::RegistrationMissing);
        }
        if !config.launcher.is_file() {
            return Err(ErrorCode::LauncherMissing);
        }
        let page = sanitize_url(&request.page_url).ok_or(ErrorCode::InvalidUrl)?;
        sanitize_url(&request.image_url).ok_or(ErrorCode::InvalidUrl)?;
        let final_url = sanitize_url(&request.final_url).ok_or(ErrorCode::InvalidUrl)?;
        let placement = positions::calculate(&request.view, request.zoom, &request.window)?;
        if is_browser_host_destination(&config.output_dir, managed)
            .map_err(|_| ErrorCode::InvalidDestination)?
        {
            return Err(ErrorCode::InvalidDestination);
        }
        let bytes = image_bytes::decode(&request.data_b64)?;
        let extension = image_bytes::extension(&bytes)?;
        // MIME is diagnostic only. Bound and escape this untrusted header;
        // never format the request or its base64 body into a diagnostic.
        let content_type: String = request.content_type.chars().take(128).collect();
        eprintln!("browser_host: content_type={content_type:?}; format={extension}");
        let origin = CaptureOrigin::from_browser_urls(page, final_url);
        let folder = config.output_dir.join(&timestamp.month);
        std::fs::create_dir_all(&folder).map_err(|_| ErrorCode::SaveFailed)?;
        let mut file =
            tempfile::NamedTempFile::new_in(&folder).map_err(|_| ErrorCode::SaveFailed)?;
        file.write_all(&bytes)
            .and_then(|()| file.flush())
            .map_err(|_| ErrorCode::SaveFailed)?;
        let domain = origin.domain();
        let mut collision = 1;
        let path = loop {
            let path = folder.join(capture_filename(
                timestamp,
                domain.as_deref(),
                1,
                1,
                collision,
                extension,
            ));
            match file.persist_noclobber(&path) {
                Ok(_) => break path,
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                    file = error.file;
                    collision += 1;
                }
                Err(_) => return Err(ErrorCode::SaveFailed),
            }
        };
        saved = Some(path.clone());
        effects.motw(&path, &origin)?;
        effects.launch(&mut launcher_command(&config.launcher, placement, &path))?;
        Ok(())
    })();
    match result {
        Ok(()) => Response {
            ok: true,
            error: None,
            path: saved,
        },
        Err(error) => Response::failure(error, saved),
    }
}

pub fn run_one(
    origin: Option<&str>,
    executable: &Path,
    input: &mut impl Read,
    output: &mut impl Write,
    effects: &mut impl Effects,
) -> io::Result<()> {
    let response = (|| -> Result<Response, ErrorCode> {
        if !origin.is_some_and(|origin| protocol::ORIGINS.contains(&origin)) {
            return Err(ErrorCode::InvalidOrigin);
        }
        let request = protocol::read_request(input)?;
        let (config, managed) = read_config(executable)?;
        Ok(handle(
            &config,
            &managed,
            request,
            &CaptureTimestamp::now(),
            effects,
        ))
    })()
    .unwrap_or_else(|error| Response::failure(error, None));
    if let Some(error) = response.error {
        eprintln!("browser_host: {error:?}");
    }
    protocol::write_response(output, &response)
}

#[cfg(test)]
mod tests;
