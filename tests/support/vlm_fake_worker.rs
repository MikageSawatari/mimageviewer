//! A dependency-free pipe peer, compiled by the client tests with rustc.
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const LIMIT: usize = 16 * 1024 * 1024;

fn frame(input: &mut impl Read) -> io::Result<(String, Vec<u8>)> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    let size = u32::from_le_bytes(prefix) as usize;
    assert!((4..=LIMIT).contains(&size));
    let mut body = vec![0; size];
    input.read_exact(&mut body)?;
    let json_size = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
    assert!(json_size > 0 && json_size <= size - 4);
    Ok((
        String::from_utf8(body[4..4 + json_size].to_vec()).unwrap(),
        body[4 + json_size..].to_vec(),
    ))
}

fn send(output: &mut impl Write, json: &str, bytes: &[u8]) {
    output
        .write_all(&((4 + json.len() + bytes.len()) as u32).to_le_bytes())
        .unwrap();
    output
        .write_all(&(json.len() as u32).to_le_bytes())
        .unwrap();
    output.write_all(json.as_bytes()).unwrap();
    output.write_all(bytes).unwrap();
    output.flush().unwrap();
}

fn field<'a>(json: &'a str, name: &str) -> &'a str {
    let key = format!("\"{name}\"");
    json[json.find(&key).unwrap() + key.len()..]
        .trim_start()
        .strip_prefix(':')
        .unwrap()
        .trim_start()
}

fn integer(json: &str, name: &str) -> u64 {
    field(json, name)
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn quote_json(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn stall() -> ! {
    loop {
        std::thread::park();
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list-devices") {
        let executable = PathBuf::from(&args[0]);
        let name = executable.file_stem().unwrap().to_string_lossy();
        if name.contains("stall") {
            stall();
        }
        if name.contains("nonzero") {
            std::process::exit(1);
        }
        if name.contains("malformed") {
            println!("invalid inventory");
            return;
        }
        println!(
            r#"{{"dxgi":[{{"name":"fake GPU","luid":"00000000:00000001","dedicated_bytes":8589934592,"software":false}}],"vulkan":[{{"luid":"00000000:00000001","luid_valid":true,"mapping":"pci-id"}}]}}"#
        );
        return;
    }
    assert!(args.iter().any(|arg| arg == "--serve"));
    let option = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].clone())
    };
    let mode = option("--fake-mode").unwrap_or_else(|| "ok".into());
    let notify = option("--notify").map(PathBuf::from);
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut captions = 0;
    while let Ok((json, rgb)) = frame(&mut input) {
        let kind = field(&json, "type");
        if kind.starts_with("\"hello\"") {
            assert!(rgb.is_empty());
            if let Some(path) = &notify {
                std::fs::write(path, b"hello").unwrap();
            }
            if mode == "stall-hello" {
                stall();
            }
            let version = if mode == "hello-version" { 999 } else { 2 };
            let prompt = if mode == "hello-prompt" { "999" } else { "1" };
            let prompt_field = if mode == "hello-missing-prompt" {
                String::new()
            } else {
                format!(",\"prompt_version\":{prompt}")
            };
            send(
                &mut output,
                &format!(
                    "{{\"type\":\"hello\",\"protocol_version\":{version},\"source_identity\":\"fake-worker-v1\"{prompt_field}}}"
                ),
                &[],
            );
        } else if kind.starts_with("\"load\"") {
            assert!(rgb.is_empty());
            assert!(integer(&json, "cpu_threads") > 0);
            if let Some(path) = &notify {
                std::fs::write(path, b"load").unwrap();
            }
            match mode.as_str() {
                "stall-load" => stall(),
                "disconnect-load" => return,
                "fatal-load" => {
                    send(
                        &mut output,
                        r#"{"type":"load","status":"fatal","error":"synthetic fatal load"}"#,
                        &[],
                    );
                    return;
                }
                _ => {}
            }
            let luid = if mode == "wrong-loaded-device" {
                "00000000:00000002"
            } else {
                "00000000:00000001"
            };
            send(
                &mut output,
                &format!(
                    r#"{{"type":"load","status":"ok","model_load_s":0.125,"device":{{"luid":"{luid}","name":"fake GPU","physical_index":0,"dxgi":{{"name":"fake GPU","luid":"{luid}","dedicated_bytes":8589934592,"software":false}}}}}}"#
                ),
                &[],
            );
            if mode == "stall-read" {
                stall();
            }
        } else if kind.starts_with("\"caption\"") {
            let id = integer(&json, "request_id");
            let width = integer(&json, "width");
            let height = integer(&json, "height");
            assert_eq!(rgb.len() as u64, width * height * 3);
            captions += 1;
            if let Some(path) = &notify {
                std::fs::write(path, b"caption").unwrap();
            }
            match mode.as_str() {
                "stall-caption" => stall(),
                "disconnect" => return,
                "fatal" => {
                    send(
                        &mut output,
                        &format!(
                            "{{\"type\":\"caption\",\"request_id\":{id},\"status\":\"fatal\",\"error\":\"synthetic fatal caption\"}}"
                        ),
                        &[],
                    );
                    return;
                }
                "malformed-json" => {
                    send(&mut output, "{", &[]);
                    continue;
                }
                "oversized-frame" => {
                    output
                        .write_all(&((LIMIT + 1) as u32).to_le_bytes())
                        .unwrap();
                    output.flush().unwrap();
                    stall();
                }
                "invalid-json-length" => {
                    output.write_all(&4u32.to_le_bytes()).unwrap();
                    output.write_all(&1u32.to_le_bytes()).unwrap();
                    output.flush().unwrap();
                    stall();
                }
                "truncated-frame" => {
                    output.write_all(&100u32.to_le_bytes()).unwrap();
                    output.write_all(b"x").unwrap();
                    output.flush().unwrap();
                    return;
                }
                "image-failure-once" if captions == 1 => {
                    send(
                        &mut output,
                        &format!(
                            "{{\"type\":\"caption\",\"request_id\":{id},\"status\":\"image_failure\",\"error\":\"synthetic bad image\"}}"
                        ),
                        &[],
                    );
                    continue;
                }
                "delayed-captions" => std::thread::sleep(Duration::from_millis(150)),
                _ => {}
            }
            let id = if mode == "mismatch-id" { id + 1 } else { id };
            let kind = if mode == "unknown-type" {
                "unknown"
            } else {
                "caption"
            };
            let status = if mode == "unknown-status" {
                "unknown"
            } else {
                "ok"
            };
            let hex: String = if mode == "invalid-caption-once" && captions == 1 {
                "画".repeat(401)
            } else {
                rgb.iter()
                    .take(64)
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            };
            let content = if mode == "incomplete-content-once" && captions == 1 {
                "{".into()
            } else if mode == "mismatched-content-once" && captions == 1 {
                r#"{"caption":"other","text":"","keywords":["binary"]}"#.into()
            } else {
                format!(r#"{{"caption":"{hex}","text":"","keywords":["binary"]}}"#)
            };
            let reason = if mode == "length-finish-once" && captions == 1 {
                "length"
            } else {
                "stop"
            };
            let response = format!(
                "{{\"type\":\"{kind}\",\"request_id\":{id},\"status\":\"{status}\",\"content\":{},\"finish_reason\":\"{reason}\",\"caption\":\"{hex}\",\"text\":\"\",\"keywords\":[\"binary\"],\"validation\":{{\"encoding_valid\":true,\"ended_on_eog\":true,\"json_complete\":true,\"field_types\":true,\"limits\":true,\"token_limit\":true,\"ok\":true}},\"generated_tokens\":7,\"wall_s\":0.01}}",
                quote_json(&content)
            );
            let extra: &[u8] = if mode == "unexpected-rgb" { &[1] } else { &[] };
            send(&mut output, &response, extra);
        } else if kind.starts_with("\"shutdown\"") {
            return;
        } else {
            panic!("unexpected request type");
        }
    }
}
