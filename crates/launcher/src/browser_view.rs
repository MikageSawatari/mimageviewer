//! Browser startup is a terminal lane: a failed forward cannot activate/start a core.
use mimageviewer_browser_view_ipc as ipc;
use std::io;

pub(crate) fn decide(
    probe: impl FnOnce() -> io::Result<bool>,
    forward: impl FnOnce() -> io::Result<()>,
) -> io::Result<ipc::StartupRoute> {
    ipc::route(probe()?, forward)
}
pub(crate) fn log(line: &str) {
    miv_startup::record_detail("browser_view", line);
    eprintln!("{line}");
}
#[cfg(windows)]
pub(crate) fn try_existing(request: &ipc::Request) -> Result<ipc::StartupRoute, String> {
    decide(existing_mutex, || {
        let mut stream = ipc::transport::Stream::open(ipc::PIPE_NAME)?;
        ipc::deliver(&mut stream, request, log)
    })
    .map_err(|error| {
        log(&format!("browser_view: forwarding failed: {error}"));
        ipc::RESTART_MESSAGE.to_owned()
    })
}
#[cfg(windows)]
fn existing_mutex() -> io::Result<bool> {
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND},
            System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS},
        },
        core::PCWSTR,
    };
    let name = super::wide_nul(env!("MIMV_MUTEX_NAME"));
    match unsafe {
        OpenMutexW(
            SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000),
            false,
            PCWSTR(name.as_ptr()),
        )
    } {
        Ok(handle) => {
            unsafe {
                let _ = CloseHandle(handle);
            }
            Ok(true)
        }
        Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(false),
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser818_ipc_launcher_routes_once_without_legacy_or_spawn_fallback() {
        let mut sends = 0;
        assert_eq!(
            decide(
                || Ok(false),
                || {
                    sends += 1;
                    Ok(())
                }
            )
            .unwrap(),
            ipc::StartupRoute::StartCore
        );
        assert_eq!(
            decide(
                || Ok(true),
                || {
                    sends += 1;
                    Ok(())
                }
            )
            .unwrap(),
            ipc::StartupRoute::Forwarded
        );
        for error in ["pipe missing", "version mismatch"] {
            assert!(
                decide(
                    || Ok(true),
                    || {
                        sends += 1;
                        Err(io::Error::other(error))
                    }
                )
                .is_err()
            );
        }
        assert_eq!(sends, 3);
        assert!(
            decide(
                || Err(io::Error::other("mutex access denied")),
                || panic!("no send after failed probe")
            )
            .is_err()
        );
    }
    #[cfg(windows)]
    #[test]
    fn browser818_cli_launcher_keeps_literal_delimiter_and_skips_rectangle_value() {
        let args: Vec<_> = ["--browser-view", "-800,0,0,600,0", "--", "日本語.png"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect();
        let request = ipc::parse_cli(&args).unwrap().unwrap();
        assert_eq!(
            super::super::parse_startup_open_path_arg_from(&args),
            Some("日本語.png".into())
        );
        let command = super::super::core_command(std::path::Path::new("core.exe"), &args);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter().map(|arg| arg.as_os_str()).collect::<Vec<_>>()
        );
        assert!(request.path.ends_with("日本語.png"));
        assert!(super::super::mcp_launch::arguments(&args).is_none());
    }
}
