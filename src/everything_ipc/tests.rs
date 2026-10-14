use super::*;
use std::collections::VecDeque;

pub(super) fn reply_bytes(rows: &[(&str, u32)], total: u32, sort: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for v in [total, rows.len() as u32, 0, FULL_PATH, sort] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.resize(REPLY_HEADER_BYTES + rows.len() * ITEM_BYTES, 0);
    for (i, (text, flags)) in rows.iter().enumerate() {
        let offset = bytes.len() as u32;
        let item = REPLY_HEADER_BYTES + i * ITEM_BYTES;
        bytes[item..item + 4].copy_from_slice(&flags.to_le_bytes());
        bytes[item + 4..item + 8].copy_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&(text.encode_utf16().count() as u32).to_le_bytes());
        for unit in text.encode_utf16().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes
}

enum FakeEvent {
    Reply {
        at: Duration,
        id: Option<u32>,
        bytes: Vec<u8>,
    },
    Cancel(Duration),
    CancelAndFail(Duration),
    Disconnect(Duration),
}

struct FakeEverythingEndpoint {
    version: Result<EverythingVersion, EverythingError>,
    loaded: bool,
    active: Option<u32>,
    sends: Vec<Vec<u8>>,
    send_error: Option<EverythingError>,
    cancel_send_error: bool,
    cancel_on_send: Option<CancelHandle>,
    time: Duration,
    send_duration: Duration,
    events: VecDeque<FakeEvent>,
}

impl Default for FakeEverythingEndpoint {
    fn default() -> Self {
        Self {
            version: Ok(EverythingVersion {
                major: 1,
                minor: 4,
                revision: 1,
            }),
            loaded: true,
            active: None,
            sends: Vec::new(),
            send_error: None,
            cancel_send_error: false,
            cancel_on_send: None,
            time: Duration::ZERO,
            send_duration: Duration::ZERO,
            events: VecDeque::new(),
        }
    }
}

impl EverythingEndpoint for FakeEverythingEndpoint {
    fn version(&mut self, _: &CancelHandle) -> Result<EverythingVersion, EverythingError> {
        self.version.clone()
    }
    fn database_loaded(&mut self) -> Result<bool, EverythingError> {
        Ok(self.loaded)
    }
    fn reply_hwnd(&self) -> u32 {
        0xfedc_ba98
    }
    fn activate(&mut self, id: u32) {
        self.active = Some(id);
    }
    fn deactivate(&mut self) {
        self.active = None;
    }
    fn send(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), EverythingError> {
        assert_eq!(
            timeout,
            if self.sends.is_empty() {
                SEND_TIMEOUT
            } else {
                CANCEL_SEND_TIMEOUT
            }
        );
        self.sends.push(bytes.to_vec());
        if self.sends.len() == 1 {
            self.time += self.send_duration;
            if self.send_duration > timeout {
                return Err(EverythingError::Busy);
            }
            assert_eq!(
                self.active,
                Some(dword(bytes, 4).unwrap()),
                "correlation installed before send"
            );
            if let Some(cancel) = &self.cancel_on_send {
                cancel.cancel();
            }
            if let Some(error) = self.send_error.take() {
                return Err(error);
            }
        } else {
            assert_eq!(
                self.active, None,
                "late replies rejected before cancel send"
            );
            if self.cancel_send_error {
                return Err(EverythingError::SendFailed);
            }
        }
        Ok(())
    }
    fn elapsed(&self) -> Duration {
        self.time
    }
    fn wait(
        &mut self,
        cancel: &CancelHandle,
        deadline: Duration,
    ) -> Result<EndpointReply, EverythingError> {
        cancelled(cancel)?;
        match self.events.pop_front() {
            Some(FakeEvent::Reply { at, id, bytes }) if at < deadline => {
                self.time = at;
                Ok(EndpointReply {
                    reply_id: id.unwrap_or(self.active.unwrap()),
                    bytes,
                })
            }
            Some(FakeEvent::Cancel(at)) if at < deadline => {
                self.time = at;
                cancel.cancel();
                Err(EverythingError::Cancelled)
            }
            Some(FakeEvent::CancelAndFail(at)) if at < deadline => {
                self.time = at;
                cancel.cancel();
                Err(EverythingError::SendFailed)
            }
            Some(FakeEvent::Disconnect(at)) if at < deadline => {
                self.time = at;
                Err(EverythingError::Disconnected)
            }
            _ => {
                self.time = deadline;
                Err(EverythingError::Timeout)
            }
        }
    }
}

fn query_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes[QUERY_HEADER_BYTES..bytes.len() - 2]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

#[test]
fn wire_query_is_seven_le_dwords_and_lossless_utf16() {
    let query = "  <\"日本語 🖼\" | ext:jpg;png> !foo regex:bar  ";
    let bytes = encode_query(query, 0xfedc_ba98, 123).unwrap();
    let fields: Vec<_> = (0..7).map(|i| dword(&bytes, i * 4).unwrap()).collect();
    assert_eq!(fields, [0xfedc_ba98, 123, 0, 0, 10_000, 4, 1]);
    assert_eq!(query_text(&bytes), query);
    assert_eq!(&bytes[bytes.len() - 2..], [0, 0]);
}

#[test]
fn wire_query_local_limits_are_checked_before_endpoint_access() {
    for (text, error) in [
        (" \t\r\n", EverythingError::EmptyQuery),
        ("foo\0bar", EverythingError::EmbeddedNul),
    ] {
        let mut endpoint = FakeEverythingEndpoint::default();
        assert_eq!(
            query_with_endpoint(text, &CancelHandle::new().unwrap(), &mut endpoint),
            Err(error)
        );
        assert!(endpoint.sends.is_empty());
    }
    assert!(encode_query(&"x".repeat(MAX_QUERY_UNITS - 1), 1, 1).is_ok());
    assert_eq!(
        encode_query(&"x".repeat(MAX_QUERY_UNITS), 1, 1),
        Err(EverythingError::QueryTooLong)
    );
    assert_eq!(
        encode_query(&"😀".repeat(MAX_QUERY_UNITS / 2), 1, 1),
        Err(EverythingError::QueryTooLong)
    );
}

#[test]
fn wire_reply_preserves_unicode_unc_flags_totals_and_actual_sort() {
    let unc = format!("\\\\server\\share\\日本語🖼{}\\a.jpg", "long".repeat(300));
    let bytes = reply_bytes(
        &[(&unc, 0), ("C:\\folder", 1), ("C:", 2), ("D:", 3)],
        50_000,
        2,
    );
    let reply = parse_reply(&bytes).unwrap();
    assert_eq!(reply.total_items, 50_000);
    assert_eq!(reply.actual_sort, 2);
    assert_eq!(reply.rows[0].full_path, unc);
    assert!(reply.rows[0].is_file());
    assert!(reply.rows[1..].iter().all(|r| !r.is_file()));
}

#[test]
fn wire_reply_rejects_malformed_whole_payload() {
    let good = reply_bytes(&[("C:\\a.jpg", 0)], 1, 1);
    let mut cases = vec![vec![], good[..19].to_vec(), good[..27].to_vec()];
    let mut odd = good.clone();
    odd.push(0);
    cases.push(odd);
    for (offset, value) in [
        (4, 10_001u32),
        (8, 1),
        (12, 0),
        (12, 5),
        (20, 4),
        (24, 0),
        (24, 29),
        (24, u32::MAX),
        (28, u32::MAX),
    ] {
        let mut bytes = good.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        cases.push(bytes);
    }
    let mut terminator = good.clone();
    *terminator.last_mut().unwrap() = 1;
    cases.push(terminator);
    let mut embedded = good.clone();
    embedded[32..34].copy_from_slice(&0u16.to_le_bytes());
    cases.push(embedded);
    let mut surrogate = good.clone();
    surrogate[32..34].copy_from_slice(&0xd800u16.to_le_bytes());
    cases.push(surrogate);
    for bytes in cases {
        assert!(parse_reply(&bytes).is_err(), "malformed reply accepted");
    }
    assert_eq!(
        parse_reply(&vec![0; MAX_REPLY_BYTES + 2]),
        Err(EverythingError::ReplyTooLarge)
    );
    let mut overlap = reply_bytes(&[("C:\\a.jpg", 0), ("D:\\b.jpg", 0)], 2, 1);
    let offset = overlap[24..28].to_vec();
    overlap[32..36].copy_from_slice(&offset);
    assert_eq!(
        parse_reply(&overlap),
        Err(EverythingError::Malformed("overlapping item data"))
    );
}

#[test]
fn wire_reply_accepts_zero_and_raw_cap_but_not_total_inconsistency() {
    assert!(
        parse_reply(&reply_bytes(&[], 0, 1))
            .unwrap()
            .rows
            .is_empty()
    );
    let strings: Vec<_> = (0..MAX_RESULTS).map(|n| format!("C:\\{n}.jpg")).collect();
    let rows: Vec<_> = strings.iter().map(|s| (s.as_str(), 0)).collect();
    assert_eq!(
        parse_reply(&reply_bytes(&rows, 50_000, 1))
            .unwrap()
            .rows
            .len(),
        10_000
    );
    assert!(parse_reply(&reply_bytes(&[("C:\\a.jpg", 0)], 0, 1)).is_err());
}

#[test]
fn fake_unavailable_unsupported_unloaded_and_rejection_have_no_fallback() {
    let cancel = CancelHandle::new().unwrap();
    let mut absent = FakeEverythingEndpoint {
        version: Err(EverythingError::Unavailable),
        ..Default::default()
    };
    assert_eq!(
        query_with_endpoint("a", &cancel, &mut absent),
        Err(EverythingError::Unavailable)
    );
    assert!(absent.sends.is_empty());
    let mut busy = FakeEverythingEndpoint {
        version: Err(EverythingError::Busy),
        ..Default::default()
    };
    assert_eq!(
        query_with_endpoint("a", &cancel, &mut busy),
        Err(EverythingError::Busy)
    );
    assert!(
        busy.sends.is_empty(),
        "failed status must not send query/cancel/retry"
    );
    for version in [
        EverythingVersion {
            major: 1,
            minor: 3,
            revision: 9,
        },
        EverythingVersion {
            major: 1,
            minor: 4,
            revision: 0,
        },
        EverythingVersion {
            major: 2,
            minor: 0,
            revision: 0,
        },
    ] {
        let mut e = FakeEverythingEndpoint {
            version: Ok(version),
            ..Default::default()
        };
        assert_eq!(
            query_with_endpoint("a", &cancel, &mut e),
            Err(EverythingError::UnsupportedVersion)
        );
        assert!(e.sends.is_empty());
    }
    let mut unloaded = FakeEverythingEndpoint {
        loaded: false,
        ..Default::default()
    };
    assert_eq!(
        query_with_endpoint("a", &cancel, &mut unloaded),
        Err(EverythingError::DatabaseNotLoaded)
    );
    assert!(unloaded.sends.is_empty());
    let mut rejected = FakeEverythingEndpoint {
        send_error: Some(EverythingError::Rejected),
        ..Default::default()
    };
    assert_eq!(
        query_with_endpoint("a", &cancel, &mut rejected),
        Err(EverythingError::Rejected)
    );
    assert_eq!(rejected.sends.len(), 1);
}

#[test]
fn fake_immediate_reply_and_syntax_passthrough() {
    for text in [
        "a",
        "!foo",
        "\"a b\" | c",
        "<a | b> !c",
        "ext:jpg;png",
        "regex:^foo.*",
    ] {
        let mut endpoint = FakeEverythingEndpoint::default();
        endpoint.events.push_back(FakeEvent::Reply {
            at: Duration::ZERO,
            id: None,
            bytes: reply_bytes(&[], 0, 1),
        });
        assert!(
            query_with_endpoint(text, &CancelHandle::new().unwrap(), &mut endpoint)
                .unwrap()
                .rows
                .is_empty()
        );
        assert_eq!(query_text(&endpoint.sends[0]), text);
        assert_eq!(endpoint.sends.len(), 1);
        assert_eq!(dword(&endpoint.sends[0], 12).unwrap(), 0);
    }
}

#[test]
fn fake_slow_handler_shares_one_send_and_reply_budget() {
    let mut endpoint = FakeEverythingEndpoint {
        send_duration: Duration::from_secs(2),
        ..Default::default()
    };
    endpoint.events.push_back(FakeEvent::Reply {
        at: Duration::from_secs(3),
        id: None,
        bytes: reply_bytes(&[("C:\\slow.jpg", 0)], 1, 1),
    });
    assert_eq!(
        query_with_endpoint("slow", &CancelHandle::new().unwrap(), &mut endpoint)
            .unwrap()
            .rows
            .len(),
        1
    );
    assert_eq!(endpoint.sends.len(), 1);
    let mut endpoint = FakeEverythingEndpoint {
        send_duration: REPLY_TIMEOUT - Duration::from_secs(1),
        ..Default::default()
    };
    endpoint.events.push_back(FakeEvent::Reply {
        at: REPLY_TIMEOUT + Duration::from_secs(1),
        id: None,
        bytes: reply_bytes(&[], 0, 1),
    });
    assert_eq!(
        query_with_endpoint("slow", &CancelHandle::new().unwrap(), &mut endpoint),
        Err(EverythingError::Timeout)
    );
    assert_eq!(endpoint.time, REPLY_TIMEOUT);
    assert_eq!(endpoint.sends.len(), 2);
    assert_eq!(query_text(&endpoint.sends[1]), "nothing:");
}

#[test]
fn busy_and_timeout_messages_are_distinct_from_client_not_running() {
    assert_eq!(
        EverythingError::Busy.user_message(),
        EverythingError::Timeout.user_message()
    );
    assert_ne!(
        EverythingError::Busy.user_message(),
        EverythingError::Unavailable.user_message()
    );
    assert!(EverythingError::Busy.user_message().contains("処理が遅い"));
    assert!(
        EverythingError::Unavailable
            .user_message()
            .contains("クライアントを起動")
    );
}

#[cfg(windows)]
#[test]
fn fake_not_responding_slow_timeout_and_unavailable_have_distinct_guidance() {
    // Inject Windows observations rather than waiting for a real HWND to hang.
    // Status ABORTIFHUNG can fail with error 0; query timeout is error 1460.
    use windows::Win32::Foundation::ERROR_TIMEOUT;
    let not_responding = EverythingError::NotResponding.user_message();
    let slow = EverythingError::Busy.user_message();
    let absent = EverythingError::Unavailable.user_message();
    assert_ne!(not_responding, slow);
    assert_ne!(not_responding, absent);
    assert_ne!(slow, absent);
    assert!(not_responding.contains("処理が終わるまで待って"));
    assert!(!not_responding.contains("絞って"));

    for (win32_error, hung, expected) in [
        (0, true, EverythingError::NotResponding),
        (ERROR_TIMEOUT.0, true, EverythingError::NotResponding),
        (ERROR_TIMEOUT.0, false, EverythingError::Busy),
        (0, false, EverythingError::SendFailed),
        (5, false, EverythingError::SendFailed),
    ] {
        let classified = super::native::classify_send_failure(win32_error, hung);
        assert_eq!(classified, expected);
        let mut status = FakeEverythingEndpoint {
            version: Err(classified.clone()),
            ..Default::default()
        };
        let error =
            query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut status).unwrap_err();
        assert_eq!(error.user_message(), expected.user_message());
        assert!(
            status.sends.is_empty(),
            "no query/cancel/retry after status failure"
        );

        let mut query = FakeEverythingEndpoint {
            send_error: Some(classified),
            send_duration: SEND_TIMEOUT,
            ..Default::default()
        };
        let error =
            query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut query).unwrap_err();
        assert_eq!(error, expected);
        assert_eq!(error.user_message(), expected.user_message());
        assert_eq!(
            query.sends.len(),
            2,
            "one query and existing best-effort cancel"
        );
        assert_eq!(query_text(&query.sends[1]), "nothing:");
        assert_eq!(query.active, None);
    }
    let mut endpoint = FakeEverythingEndpoint {
        version: Err(EverythingError::Unavailable),
        ..Default::default()
    };
    assert_eq!(
        query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut endpoint)
            .unwrap_err()
            .user_message(),
        absent
    );
    assert!(endpoint.sends.is_empty());
}

#[test]
fn fake_timeout_disconnect_and_send_failure_send_one_cancel_and_do_not_wait() {
    for mode in 0..4 {
        let mut endpoint = FakeEverythingEndpoint {
            cancel_send_error: true,
            ..Default::default()
        };
        let expected = match mode {
            0 => EverythingError::Timeout,
            1 => {
                endpoint
                    .events
                    .push_back(FakeEvent::Disconnect(Duration::from_secs(1)));
                EverythingError::Disconnected
            }
            2 => {
                endpoint.send_error = Some(EverythingError::SendFailed);
                EverythingError::SendFailed
            }
            _ => {
                endpoint.send_error = Some(EverythingError::Busy);
                EverythingError::Busy
            }
        };
        assert_eq!(
            query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut endpoint),
            Err(expected)
        );
        assert_eq!(endpoint.sends.len(), 2);
        assert_eq!(query_text(&endpoint.sends[1]), "nothing:");
        assert_eq!(
            dword(&endpoint.sends[0], 0).unwrap(),
            dword(&endpoint.sends[1], 0).unwrap()
        );
        assert_ne!(
            dword(&endpoint.sends[0], 4).unwrap(),
            dword(&endpoint.sends[1], 4).unwrap()
        );
        assert_eq!(endpoint.active, None);
        if mode == 0 {
            assert_eq!(endpoint.time, REPLY_TIMEOUT);
        }
    }
}

#[test]
fn fake_cancel_before_during_send_and_before_late_reply() {
    let cancel = CancelHandle::new().unwrap();
    cancel.cancel();
    let mut endpoint = FakeEverythingEndpoint::default();
    assert_eq!(
        query_with_endpoint("a", &cancel, &mut endpoint),
        Err(EverythingError::Cancelled)
    );
    assert!(endpoint.sends.is_empty());
    for during_send in [false, true] {
        let cancel = CancelHandle::new().unwrap();
        let mut e = FakeEverythingEndpoint::default();
        if during_send {
            e.cancel_on_send = Some(cancel.clone());
        } else {
            e.events
                .push_back(FakeEvent::Cancel(Duration::from_millis(3)));
        }
        e.events.push_back(FakeEvent::Reply {
            at: Duration::from_millis(4),
            id: None,
            bytes: reply_bytes(&[("C:\\late.jpg", 0)], 1, 1),
        });
        assert_eq!(
            query_with_endpoint("a", &cancel, &mut e),
            Err(EverythingError::Cancelled)
        );
        assert_eq!(e.sends.len(), 2);
        assert!(!e.events.is_empty(), "late reply was not consumed/adopted");
        assert_eq!(e.active, None);
    }
}

#[test]
fn fake_cancellation_wins_over_a_simultaneous_wait_failure() {
    let mut endpoint = FakeEverythingEndpoint::default();
    endpoint
        .events
        .push_back(FakeEvent::CancelAndFail(Duration::from_millis(1)));
    assert_eq!(
        query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut endpoint),
        Err(EverythingError::Cancelled)
    );
    assert_eq!(endpoint.sends.len(), 2);
    assert_eq!(query_text(&endpoint.sends[1]), "nothing:");
}

#[test]
fn fake_wrong_id_is_ignored_and_malformed_reply_is_never_partial() {
    let mut e = FakeEverythingEndpoint::default();
    e.events.push_back(FakeEvent::Reply {
        at: Duration::ZERO,
        id: Some(0),
        bytes: reply_bytes(&[("C:\\stale.jpg", 0)], 1, 1),
    });
    e.events.push_back(FakeEvent::Reply {
        at: Duration::from_millis(1),
        id: None,
        bytes: reply_bytes(&[], 0, 2),
    });
    assert_eq!(
        query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut e)
            .unwrap()
            .actual_sort,
        2
    );
    let mut e = FakeEverythingEndpoint::default();
    e.events.push_back(FakeEvent::Reply {
        at: Duration::ZERO,
        id: None,
        bytes: vec![0; 2],
    });
    assert!(matches!(
        query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut e),
        Err(EverythingError::Malformed(_))
    ));
    assert_eq!(e.sends.len(), 1);
}
