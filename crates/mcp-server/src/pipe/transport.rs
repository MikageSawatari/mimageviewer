//! Independent OVERLAPPED operations are required: duplicating a synchronous
//! Windows pipe handle still serializes reads and writes on the same file object.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    sync::Arc,
};

#[derive(Clone)]
pub(super) struct PipeStream {
    file: Arc<File>,
    #[cfg(test)]
    regular_file: bool,
}
impl PipeStream {
    pub(super) fn connect(name: &str) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(windows::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED.0);
        }
        Ok(Self {
            file: Arc::new(options.open(name)?),
            #[cfg(test)]
            regular_file: false,
        })
    }
    #[cfg(test)]
    pub(super) fn from_file_for_test(file: File) -> Self {
        Self {
            file: Arc::new(file),
            regular_file: true,
        }
    }
}
#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE},
            Storage::FileSystem::{ReadFile, WriteFile},
            System::{
                IO::{GetOverlappedResult, OVERLAPPED},
                Threading::CreateEventW,
            },
        },
        core::PCWSTR,
    };
    struct Event(HANDLE);
    impl Drop for Event {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    impl PipeStream {
        fn operation(&self, read: Option<&mut [u8]>, write: Option<&[u8]>) -> io::Result<usize> {
            let event = Event(
                unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
                    .map_err(io::Error::other)?,
            );
            let mut op = OVERLAPPED {
                hEvent: event.0,
                ..Default::default()
            };
            let handle = HANDLE(self.file.as_raw_handle());
            let started = unsafe {
                match read {
                    Some(bytes) => ReadFile(handle, Some(bytes), None, Some(&mut op)),
                    None => WriteFile(handle, write, None, Some(&mut op)),
                }
            };
            if started.is_err() {
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(error.0 as i32));
                }
            }
            // Wait only for this operation's event; a simultaneous cancel write has
            // its own OVERLAPPED/event and is never serialized behind this read.
            // Both the buffer and OVERLAPPED remain alive until completion.
            let mut transferred = 0;
            unsafe { GetOverlappedResult(handle, &op, &mut transferred, true) }
                .map_err(io::Error::other)?;
            Ok(transferred as usize)
        }
    }
    impl Read for PipeStream {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            #[cfg(test)]
            if self.regular_file {
                return (&*self.file).read(bytes);
            }
            self.operation(Some(bytes), None)
        }
    }
    impl Write for PipeStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            #[cfg(test)]
            if self.regular_file {
                return (&*self.file).write(bytes);
            }
            self.operation(None, Some(bytes))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
#[cfg(not(windows))]
impl Read for PipeStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        (&*self.file).read(bytes)
    }
}
#[cfg(not(windows))]
impl Write for PipeStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&*self.file).write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
