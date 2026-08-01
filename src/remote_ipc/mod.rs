mod collections;
mod container;
mod path_guard;
mod thumbnail;
mod video_stream;

#[cfg(windows)]
mod pipe;
pub(crate) mod session;
pub(crate) mod ui;

pub(crate) struct RemoteIpcServer {
    #[cfg(windows)]
    _guard: pipe::ServerGuard,
}

impl RemoteIpcServer {
    pub(crate) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        #[cfg(windows)]
        {
            return pipe::ServerGuard::start(settings).map(|guard| Self { _guard: guard });
        }
        #[cfg(not(windows))]
        {
            let _ = settings;
            Err("--remote-ipc は Windows の名前付きパイプ専用です".to_owned())
        }
    }

    pub(crate) fn session_handle(&self) -> session::SessionHandle {
        #[cfg(windows)]
        {
            self._guard.session_handle()
        }
        #[cfg(not(windows))]
        {
            unreachable!("remote IPC server is Windows-only")
        }
    }
}
