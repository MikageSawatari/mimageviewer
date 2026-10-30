//! Independent OVERLAPPED operations are required: duplicating a synchronous
//! Windows pipe handle still serializes reads and writes on the same file object.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    sync::Arc,
    time::{Duration, Instant},
};

const CONNECT_BUDGET: Duration = Duration::from_secs(2);
const ERROR_PIPE_BUSY_CODE: i32 = 231;
const ERROR_SEM_TIMEOUT_CODE: i32 = 121;

/// Busy is an ordinary listener-reuse race. The caller's single total budget
/// covers every WaitNamedPipe/open attempt, including contention after a wake.
fn connect_with_budget<T>(
    mut open: impl FnMut() -> io::Result<T>,
    mut wait: impl FnMut(u32) -> io::Result<()>,
    mut elapsed: impl FnMut() -> Duration,
) -> io::Result<T> {
    match open() {
        Ok(stream) => return Ok(stream),
        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY_CODE) => {}
        Err(error) => return Err(error),
    }
    loop {
        let remaining = CONNECT_BUDGET
            .checked_sub(elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::from_raw_os_error(ERROR_PIPE_BUSY_CODE))?;
        let timeout_ms = remaining.as_millis().max(1) as u32;
        match wait(timeout_ms) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(ERROR_SEM_TIMEOUT_CODE | ERROR_PIPE_BUSY_CODE)
                ) => {}
            Err(error) => return Err(error),
        }
        if elapsed() >= CONNECT_BUDGET {
            return Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY_CODE));
        }
        match open() {
            Ok(stream) => return Ok(stream),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY_CODE) => {}
            Err(error) => return Err(error),
        }
    }
}

#[derive(Clone)]
pub(super) struct PipeStream {
    file: Arc<File>,
    #[cfg(test)]
    regular_file: bool,
}
impl PipeStream {
    pub(super) fn connect(name: &str) -> io::Result<Self> {
        let started = Instant::now();
        connect_with_budget(
            || Self::open(name),
            |timeout_ms| {
                #[cfg(windows)]
                {
                    use windows::{
                        Win32::{Foundation::GetLastError, System::Pipes::WaitNamedPipeW},
                        core::PCWSTR,
                    };
                    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
                    if unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), timeout_ms) }.as_bool() {
                        Ok(())
                    } else {
                        Err(io::Error::from_raw_os_error(
                            unsafe { GetLastError() }.0 as i32,
                        ))
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = timeout_ms;
                    Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "mIV IPC requires Windows",
                    ))
                }
            },
            || started.elapsed(),
        )
    }
    fn open(name: &str) -> io::Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    #[test]
    fn busy_connect_reuses_one_total_budget_when_another_client_wins() {
        let elapsed = Cell::new(Duration::ZERO);
        let mut opens =
            VecDeque::from([Err(ERROR_PIPE_BUSY_CODE), Err(ERROR_PIPE_BUSY_CODE), Ok(7)]);
        let mut waits = vec![];
        let result = connect_with_budget(
            || {
                opens
                    .pop_front()
                    .unwrap()
                    .map_err(io::Error::from_raw_os_error)
            },
            |remaining| {
                waits.push(remaining);
                elapsed.set(elapsed.get() + Duration::from_millis(100));
                Ok(())
            },
            || elapsed.get(),
        )
        .unwrap();
        assert_eq!(result, 7);
        assert_eq!(waits, [2000, 1900]);
        assert!(opens.is_empty());
    }

    #[test]
    fn busy_connect_deadline_never_restarts_and_timeout_stays_busy() {
        let elapsed = Cell::new(Duration::ZERO);
        let opens = Cell::new(0);
        let mut waits = vec![];
        let error = connect_with_budget::<()>(
            || {
                opens.set(opens.get() + 1);
                Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY_CODE))
            },
            |remaining| {
                waits.push(remaining);
                elapsed.set(if waits.len() == 1 {
                    Duration::from_millis(1999)
                } else {
                    CONNECT_BUDGET
                });
                Err(io::Error::from_raw_os_error(ERROR_SEM_TIMEOUT_CODE))
            },
            || elapsed.get(),
        )
        .unwrap_err();
        assert_eq!(waits, [2000, 1]);
        assert_eq!(opens.get(), 2);
        assert_eq!(error.raw_os_error(), Some(ERROR_PIPE_BUSY_CODE));
    }

    #[test]
    fn nonbusy_open_and_wait_errors_are_not_retried() {
        for code in [2, 5, 6] {
            let error = connect_with_budget::<()>(
                || Err(io::Error::from_raw_os_error(code)),
                |_| panic!("only ERROR_PIPE_BUSY may wait"),
                || panic!("nonbusy error must return immediately"),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
            let error = connect_with_budget::<()>(
                || Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY_CODE)),
                |_| Err(io::Error::from_raw_os_error(code)),
                || Duration::ZERO,
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
        }
        assert_eq!(
            connect_with_budget(
                || Ok(9),
                |_| panic!("successful open must not wait"),
                || panic!("successful open must return immediately")
            )
            .unwrap(),
            9
        );
    }

    #[test]
    fn wait_success_does_not_allow_open_after_deadline() {
        let elapsed = Cell::new(Duration::ZERO);
        let opens = Cell::new(0);
        let error = connect_with_budget::<()>(
            || {
                opens.set(opens.get() + 1);
                Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY_CODE))
            },
            |remaining| {
                assert_eq!(remaining, 2000);
                elapsed.set(CONNECT_BUDGET);
                Ok(())
            },
            || elapsed.get(),
        )
        .unwrap_err();
        assert_eq!(opens.get(), 1);
        assert_eq!(error.raw_os_error(), Some(ERROR_PIPE_BUSY_CODE));
    }
}
