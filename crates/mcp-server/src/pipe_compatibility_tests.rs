use super::*;

#[test]
fn only_exact_hello_sends_business_request_and_other_responses_use_neutral_guidance() {
    let response = |result| {
        let mut bytes = vec![];
        write_frame(&mut bytes, &Response { id: 1, result }).unwrap();
        bytes
    };
    let control = CallControl::default();
    for (bytes, accepted) in [
        (
            response(Ok(Payload::Hello {
                version: AGENT_PROTOCOL_VERSION,
            })),
            true,
        ),
        (response(Ok(Payload::Hello { version: 3 })), false),
        (response(Ok(Payload::Hello { version: 5 })), false),
        (
            response(Ok(Payload::Hello {
                version: AGENT_PROTOCOL_VERSION + 1,
            })),
            false,
        ),
        (
            response(Err(AgentError::new(
                ErrorCode::VersionMismatch,
                "mIV と AI 連携の版が違います。設定の案内に従って登録し直してください",
            ))),
            false,
        ),
        (response(Ok(Payload::Cancelled)), false),
        (vec![1, 0, 0, 0, b'?'], false),
        (vec![1, 0], false),
        (vec![2, 0, 0, 0, b'{'], false),
        (
            {
                let mut b = vec![];
                write_frame(
                    &mut b,
                    &serde_json::json!({"id":1,"result":{"Ok":{"type":"future","data":{}}}}),
                )
                .unwrap();
                b
            },
            false,
        ),
        (
            {
                let mut b = vec![];
                write_frame(
                    &mut b,
                    &Response {
                        id: 2,
                        result: Ok(Payload::Hello {
                            version: AGENT_PROTOCOL_VERSION,
                        }),
                    },
                )
                .unwrap();
                b
            },
            false,
        ),
    ] {
        let mut sent = vec![];
        let result = after_hello(&mut bytes.as_slice(), &control, |_| {
            write_frame(
                &mut sent,
                &Request {
                    id: 2,
                    command: Command::Status,
                },
            )
            .unwrap();
            Ok(())
        });
        if accepted {
            result.unwrap();
            let request: Request = read_frame(&mut sent.as_slice()).unwrap().unwrap();
            assert_eq!(request.command, Command::Status);
        } else {
            let error = result.unwrap_err().into_agent();
            assert_eq!(error.code, ErrorCode::VersionMismatch);
            assert_eq!(error.message, VERSION_MISMATCH_MESSAGE);
            assert!(sent.is_empty());
        }
    }
}

#[test]
fn cancelled_handshake_never_sends_a_business_request() {
    let control = CallControl::default();
    control.cancelled.store(true, Ordering::Release);
    let mut frame = vec![];
    write_frame(
        &mut frame,
        &Response {
            id: 1,
            result: Ok(Payload::Hello {
                version: AGENT_PROTOCOL_VERSION,
            }),
        },
    )
    .unwrap();
    assert_eq!(
        after_hello(
            &mut frame.as_slice(),
            &control,
            |_| -> Result<(), BackendError> { panic!("cancelled handshake sent business request") }
        )
        .unwrap_err()
        .into_agent()
        .code,
        ErrorCode::Cancelled
    );
}
