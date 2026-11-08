//! Bounded HTTP transport shared by model addon kind. Call only from a worker.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::addon::AddonKind;
use sha2::{Digest, Sha256};

pub(crate) const BACKOFFS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(7),
    Duration::from_secs(15),
];
const CHUNK: usize = 256 * 1024;
pub(crate) const CANCELLED: &str = "VLM installation cancelled";

pub(crate) fn check_cancel_for_kind(kind: AddonKind, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(kind.cancelled().into())
    } else {
        Ok(())
    }
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(60))
        .build()
}

enum AttemptError {
    Retry(String),
    Stop(String),
}

fn http_error(error: ureq::Error) -> AttemptError {
    match error {
        ureq::Error::Status(code, _) if (500..600).contains(&code) || code == 429 => {
            AttemptError::Retry(format!("HTTP {code}"))
        }
        ureq::Error::Transport(error) => AttemptError::Retry(format!("HTTP: {error}")),
        other => AttemptError::Stop(format!("HTTP: {other}")),
    }
}

fn retry_for_kind<T>(
    kind: AddonKind,
    cancel: &AtomicBool,
    backoffs: &[Duration],
    mut attempt: impl FnMut() -> Result<T, AttemptError>,
) -> Result<T, String> {
    let mut retries = 0;
    loop {
        check_cancel_for_kind(kind, cancel)?;
        match attempt() {
            Ok(value) => return Ok(value),
            Err(AttemptError::Stop(error)) => return Err(error),
            Err(AttemptError::Retry(error)) => {
                let Some(delay) = backoffs.get(retries) else {
                    return Err(error);
                };
                retries += 1;
                let until = Instant::now() + *delay;
                while let Some(remaining) = until.checked_duration_since(Instant::now()) {
                    check_cancel_for_kind(kind, cancel)?;
                    std::thread::sleep(remaining.min(Duration::from_millis(50)));
                }
            }
        }
    }
}

pub(crate) fn fetch_index_for_kind(
    kind: AddonKind,
    url: &str,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let label = kind.label();
    const LIMIT: u64 = 256 * 1024;
    let agent = agent();
    retry_for_kind(kind, cancel, &BACKOFFS, || {
        let response = agent.get(url).call().map_err(http_error)?;
        if response.status() != 200 {
            return Err(AttemptError::Stop("Unexpected index HTTP response".into()));
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| AttemptError::Retry(format!("Read index: {e}")))?;
        if bytes.len() as u64 > LIMIT {
            return Err(AttemptError::Stop(format!("{label} index is too large")));
        }
        check_cancel_for_kind(kind, cancel).map_err(AttemptError::Stop)?;
        Ok(bytes)
    })
}

pub(crate) fn sha256_file_for_kind(
    kind: AddonKind,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Open {}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    loop {
        check_cancel_for_kind(kind, cancel)?;
        let n = file.read(&mut buffer).map_err(|e| format!("Hash: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// A completed partial is verified before any request, avoiding Range 416.
pub(crate) fn download_for_kind(
    kind: AddonKind,
    url: &str,
    partial: &Path,
    bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    download_with_backoffs_for_kind(
        kind,
        url,
        partial,
        bytes,
        sha256,
        cancel,
        &mut progress,
        &BACKOFFS,
    )
}

fn download_with_backoffs_for_kind(
    kind: AddonKind,
    url: &str,
    partial: &Path,
    bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
    backoffs: &[Duration],
) -> Result<(), String> {
    let label = kind.label();
    if bytes == 0 {
        return Err(format!("{label} download size must be positive"));
    }
    super::addon::reject_links_for_kind(kind, partial)?;
    let existing = match fs::metadata(partial) {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => return Err(format!("{label} partial is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(format!("Read partial: {error}")),
    };
    if existing >= bytes && existing > 0 {
        if existing == bytes
            && sha256_file_for_kind(kind, partial, cancel)?.eq_ignore_ascii_case(sha256)
        {
            progress(bytes);
            return Ok(());
        }
        fs::remove_file(partial).map_err(|e| format!("Remove corrupt partial: {e}"))?;
    }
    let agent = agent();
    retry_for_kind(kind, cancel, backoffs, || {
        super::addon::reject_links_for_kind(kind, partial).map_err(AttemptError::Stop)?;
        let start = match fs::metadata(partial) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(AttemptError::Stop(format!("Read partial: {error}"))),
        };
        if start == bytes {
            return Ok(());
        }
        let mut request = agent.get(url).set("Accept-Encoding", "identity");
        if start > 0 {
            request = request.set("Range", &format!("bytes={start}-"));
        }
        let response = request.call().map_err(http_error)?;
        let offset = match response.status() {
            200 => 0, // Server ignored Range: truncate instead of appending.
            206 => {
                let expected = format!("bytes {start}-{}/{bytes}", bytes - 1);
                if response.header("Content-Range") != Some(expected.as_str()) {
                    return Err(AttemptError::Stop("Invalid HTTP Content-Range".into()));
                }
                start
            }
            code => return Err(AttemptError::Stop(format!("Unexpected HTTP {code}"))),
        };
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .append(offset > 0)
            .open(partial)
            .map_err(|e| AttemptError::Stop(format!("Open partial: {e}")))?;
        let mut reader = response.into_reader();
        let mut buffer = vec![0; CHUNK];
        let mut done = offset;
        progress(done);
        while done < bytes {
            check_cancel_for_kind(kind, cancel).map_err(AttemptError::Stop)?;
            let remaining = (bytes - done).min(CHUNK as u64) as usize;
            let n = reader
                .read(&mut buffer[..remaining])
                .map_err(|e| AttemptError::Retry(format!("Read download: {e}")))?;
            if n == 0 {
                return Err(AttemptError::Retry(format!(
                    "Download truncated: {done}/{bytes}"
                )));
            }
            file.write_all(&buffer[..n])
                .map_err(|e| AttemptError::Stop(format!("Write download: {e}")))?;
            done += n as u64;
            progress(done);
        }
        file.flush()
            .map_err(|e| AttemptError::Stop(format!("Flush download: {e}")))?;
        Ok(())
    })?;
    if !sha256_file_for_kind(kind, partial, cancel)?.eq_ignore_ascii_case(sha256) {
        fs::remove_file(partial).map_err(|e| format!("Remove corrupt download: {e}"))?;
        return Err(format!("Downloaded {label} file SHA-256 does not match"));
    }
    Ok(())
}

// VLM regression callers keep their original API and exact diagnostics.
#[cfg(test)]
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    check_cancel_for_kind(AddonKind::Vlm, cancel)
}

#[cfg(test)]
fn retry<T>(
    cancel: &AtomicBool,
    backoffs: &[Duration],
    attempt: impl FnMut() -> Result<T, AttemptError>,
) -> Result<T, String> {
    retry_for_kind(AddonKind::Vlm, cancel, backoffs, attempt)
}

#[cfg(test)]
pub(crate) fn fetch_index(url: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    fetch_index_for_kind(AddonKind::Vlm, url, cancel)
}

#[cfg(test)]
pub(crate) fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    sha256_file_for_kind(AddonKind::Vlm, path, cancel)
}

#[cfg(test)]
pub(crate) fn download(
    url: &str,
    partial: &Path,
    bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    progress: impl FnMut(u64),
) -> Result<(), String> {
    download_for_kind(
        AddonKind::Vlm,
        url,
        partial,
        bytes,
        sha256,
        cancel,
        progress,
    )
}

#[cfg(test)]
fn download_with_backoffs(
    url: &str,
    partial: &Path,
    bytes: u64,
    sha256: &str,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
    backoffs: &[Duration],
) -> Result<(), String> {
    download_with_backoffs_for_kind(
        AddonKind::Vlm,
        url,
        partial,
        bytes,
        sha256,
        cancel,
        progress,
        backoffs,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    #[test]
    fn transport_diagnostics_choose_kind_without_rewriting_lower_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        for (kind, label) in [(AddonKind::Vlm, "VLM"), (AddonKind::Depth, "Depth")] {
            assert_eq!(
                download_for_kind(kind, "unused", dir.path(), 0, "unused", &cancel, |_| {})
                    .unwrap_err(),
                format!("{label} download size must be positive")
            );
            assert_eq!(
                download_for_kind(kind, "unused", dir.path(), 1, "unused", &cancel, |_| {})
                    .unwrap_err(),
                format!("{label} partial is not a regular file")
            );
            let source = "underlying VLM error at C:/VLM-user/model.onnx";
            let result: Result<(), String> = retry_for_kind(kind, &cancel, &[], || {
                Err(AttemptError::Stop(source.into()))
            });
            assert_eq!(result.unwrap_err(), source);
        }
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            check_cancel(&cancelled).unwrap_err(),
            "VLM installation cancelled"
        );
        assert_eq!(
            check_cancel_for_kind(AddonKind::Depth, &cancelled).unwrap_err(),
            "Depth installation cancelled"
        );
    }

    #[test]
    fn transport_hash_diagnostics_preserve_vlm_in_user_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("VLM-user-model.onnx");
        let source = File::open(&missing).err().unwrap();
        let expected = format!("Open {}: {source}", missing.display());
        for kind in [AddonKind::Vlm, AddonKind::Depth] {
            assert_eq!(
                sha256_file_for_kind(kind, &missing, &AtomicBool::new(false)).unwrap_err(),
                expected
            );
        }
        fs::write(&missing, b"VLM content").unwrap();
        assert_eq!(
            sha256_file(&missing, &AtomicBool::new(false)).unwrap(),
            digest(b"VLM content")
        );
    }

    #[test]
    fn oversized_index_diagnostics_keep_exact_vlm_and_depth_labels() {
        for (kind, label) in [(AddonKind::Vlm, "VLM"), (AddonKind::Depth, "Depth")] {
            let (base, _, join) = server(vec![ok(&vec![b'x'; 256 * 1024 + 1])]);
            let error = fetch_index_for_kind(kind, &base, &AtomicBool::new(false)).unwrap_err();
            join.join().unwrap();
            assert_eq!(error, format!("{label} index is too large"));
        }
    }

    pub(crate) fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Loopback only. Each supplied response gets its own Connection: close.
    pub(crate) fn server(
        responses: Vec<Vec<u8>>,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        server_with_hook(responses, |_| {})
    }

    pub(crate) fn server_with_hook(
        responses: Vec<Vec<u8>>,
        mut on_request: impl FnMut(usize) + Send + 'static,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        let join = std::thread::spawn(move || {
            for (ordinal, response) in responses.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                let _ = tx.send(String::from_utf8(request).unwrap());
                on_request(ordinal);
                let _ = stream.write_all(&response);
            }
        });
        (url, rx, join)
    }

    pub(crate) fn response(code: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response =
            format!("HTTP/1.1 {code}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
        response.extend_from_slice(body);
        response
    }

    pub(crate) fn ok(body: &[u8]) -> Vec<u8> {
        response(
            "200 OK",
            &format!("Content-Length: {}\r\n", body.len()),
            body,
        )
    }

    #[test]
    fn resumes_a_partial_with_validated_content_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"abc").unwrap();
        let (url, requests, join) = server(vec![response(
            "206 Partial Content",
            "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\n",
            b"def",
        )]);
        download(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abcdef");
        assert!(
            requests
                .recv()
                .unwrap()
                .to_ascii_lowercase()
                .contains("range: bytes=3-")
        );
        join.join().unwrap();
    }

    #[test]
    fn truncation_retries_the_remaining_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        let (url, requests, join) = server(vec![
            response("200 OK", "Content-Length: 6\r\n", b"abc"),
            response(
                "206 Partial Content",
                "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\n",
                b"def",
            ),
        ]);
        download_with_backoffs(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            &mut |_| {},
            &[Duration::ZERO],
        )
        .unwrap();
        requests.recv().unwrap();
        assert!(
            requests
                .recv()
                .unwrap()
                .to_ascii_lowercase()
                .contains("range: bytes=3-")
        );
        join.join().unwrap();
    }

    #[test]
    fn ignores_range_by_restarting_and_never_writes_past_declared_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"bad").unwrap();
        let (url, _, join) = server(vec![ok(b"abcdef-extra-untrusted-bytes")]);
        download(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abcdef");
        join.join().unwrap();
    }

    #[test]
    fn response_without_content_length_is_bounded_and_hash_checked() {
        for (body, valid) in [
            (b"abcdef-extra-untrusted-bytes".as_slice(), true),
            (b"xxxxxx-extra-untrusted-bytes".as_slice(), false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("model.partial");
            let (url, requests, join) = server(vec![response("200 OK", "", body)]);
            let mut observed = Vec::new();
            let result = download_with_backoffs(
                &url,
                &path,
                6,
                &digest(b"abcdef"),
                &AtomicBool::new(false),
                &mut |done| observed.push(done),
                &[],
            );
            join.join().unwrap();
            assert_eq!(requests.try_iter().count(), 1);
            assert!(observed.iter().all(|&done| done <= 6));
            assert_eq!(observed.last(), Some(&6));
            if valid {
                result.unwrap();
                assert_eq!(fs::metadata(&path).unwrap().len(), 6);
                assert_eq!(fs::read(path).unwrap(), b"abcdef");
            } else {
                assert!(result.unwrap_err().contains("SHA-256 does not match"));
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn short_response_without_content_length_exhausts_bounded_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        let (url, requests, join) =
            server((0..5).map(|_| response("200 OK", "", b"abc")).collect());
        let error = download_with_backoffs(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            &mut |done| assert!(done <= 6),
            &[Duration::ZERO; 4],
        )
        .unwrap_err();
        join.join().unwrap();
        assert!(error.contains("Download truncated: 3/6"));
        let requests: Vec<_> = requests.try_iter().collect();
        assert_eq!(requests.len(), 5);
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
        assert!(
            requests[1..]
                .iter()
                .all(|request| request.to_ascii_lowercase().contains("range: bytes=3-"))
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), 3);
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }

    #[test]
    fn corrupt_full_size_partial_is_removed_before_refetch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"xxxxxx").unwrap();
        let observed_path = path.clone();
        let (url, requests, join) = server_with_hook(vec![ok(b"abcdef")], move |_| {
            assert!(
                !observed_path.exists(),
                "corrupt partial must be removed before requesting its replacement"
            );
        });
        download_with_backoffs(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            &mut |_| {},
            &[],
        )
        .unwrap();
        join.join().unwrap();
        let requests: Vec<_> = requests.try_iter().collect();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
        assert_eq!(fs::read(path).unwrap(), b"abcdef");
    }

    #[test]
    fn corrupt_resumed_partial_is_rejected_before_fresh_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"bad").unwrap();
        let (url, requests, join) = server(vec![response(
            "206 Partial Content",
            "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\n",
            b"def",
        )]);
        let error = download_with_backoffs(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            &mut |_| {},
            &[],
        )
        .unwrap_err();
        join.join().unwrap();
        assert!(
            requests
                .recv()
                .unwrap()
                .to_ascii_lowercase()
                .contains("range: bytes=3-")
        );
        assert!(error.contains("SHA-256 does not match"));
        assert!(!path.exists(), "corrupt resumed bytes must not be reusable");

        let (url, requests, join) = server(vec![ok(b"abcdef")]);
        download_with_backoffs(
            &url,
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            &mut |_| {},
            &[],
        )
        .unwrap();
        join.join().unwrap();
        assert!(
            !requests
                .recv()
                .unwrap()
                .to_ascii_lowercase()
                .contains("range:")
        );
        assert_eq!(fs::read(path).unwrap(), b"abcdef");
    }

    #[test]
    fn invalid_range_cannot_append_wrong_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"abc").unwrap();
        let (url, _, join) = server(vec![response(
            "206 Partial Content",
            "Content-Length: 3\r\nContent-Range: bytes 0-2/6\r\n",
            b"def",
        )]);
        assert!(
            download(
                &url,
                &path,
                6,
                &digest(b"abcdef"),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"abc");
        join.join().unwrap();
    }

    #[test]
    fn completed_partial_is_checked_without_a_network_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"abcdef").unwrap();
        download(
            "http://127.0.0.1:1",
            &path,
            6,
            &digest(b"abcdef"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    }

    #[test]
    fn hash_mismatch_discards_only_the_corrupt_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        let (url, _, join) = server(vec![ok(b"bad")]);
        assert!(
            download(
                &url,
                &path,
                3,
                &digest(b"yes"),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        assert!(!path.exists());
        join.join().unwrap();
    }

    #[test]
    fn retry_count_is_bounded_and_permanent_errors_do_not_retry() {
        for (status, count) in [("500 Server Error", 5), ("404 Not Found", 1)] {
            let dir = tempfile::tempdir().unwrap();
            let (url, requests, join) = server(
                (0..count)
                    .map(|_| response(status, "Content-Length: 0\r\n", b""))
                    .collect(),
            );
            assert!(
                download_with_backoffs(
                    &url,
                    &dir.path().join("p"),
                    1,
                    &digest(b"x"),
                    &AtomicBool::new(false),
                    &mut |_| {},
                    &[Duration::ZERO; 4]
                )
                .is_err()
            );
            join.join().unwrap();
            assert_eq!(requests.try_iter().count(), count);
        }
        assert_eq!(BACKOFFS.map(|v| v.as_secs()), [1, 3, 7, 15]);
    }

    #[test]
    fn index_body_is_bounded() {
        let (url, _, join) = server(vec![ok(&vec![b'x'; 256 * 1024 + 1])]);
        assert!(
            fetch_index(&url, &AtomicBool::new(false))
                .unwrap_err()
                .contains("too large")
        );
        join.join().unwrap();
    }

    #[test]
    fn cancellation_interrupts_backoff_and_preserves_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.partial");
        fs::write(&path, b"abc").unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let result: Result<(), String> = retry(&cancel, &[Duration::from_secs(15)], || {
            cancel.store(true, Ordering::Relaxed);
            Err(AttemptError::Retry("temporary".into()))
        });
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }
}
