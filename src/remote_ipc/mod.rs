mod collections;
mod path_guard;
mod thumbnail;

#[cfg(windows)]
mod pipe;

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
}
