//! Opt-in I2 diagnostics. Observe the actual appended events; never alter input.

use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};

use winit::{
    event::{ElementState, Ime, WindowEvent},
    keyboard::{Key, PhysicalKey},
};

static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

pub(super) struct Trace {
    started: web_time::Instant,
    active: Arc<AtomicBool>,
    sender: Sender<String>,
}

/// Read the environment once, when the first egui-winit State is constructed.
/// All windows share one clock and writer. Unset/invalid means no worker or I/O.
pub(super) fn init() -> Option<&'static Trace> {
    TRACE
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os("MIV_IME_KEY_TRACE")?);
            if !path.is_absolute() {
                return None;
            }
            let started = web_time::Instant::now();
            let active = Arc::new(AtomicBool::new(true));
            let worker_active = Arc::clone(&active);
            let (sender, receiver) = mpsc::channel();
            // Opening, appending and flushing can block: keep them off the UI thread.
            std::thread::Builder::new()
                .name("miv-ime-key-trace".into())
                .spawn(move || {
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        Ok(file) => write_lines(file, receiver, &worker_active),
                        Err(_) => worker_active.store(false, Ordering::Relaxed),
                    }
                })
                .ok()?;
            Some(Trace {
                started,
                active,
                sender,
            })
        })
        .as_ref()
}

fn write_lines(mut writer: impl std::io::Write, receiver: Receiver<String>, active: &AtomicBool) {
    for line in receiver {
        if writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
        {
            // Diagnostics are disposable. No retries, recovery, or shutdown waits.
            active.store(false, Ordering::Relaxed);
            break;
        }
    }
}

impl Trace {
    pub(super) fn begin(&self, event: &WindowEvent, events_start: usize) -> Option<Pending<'_>> {
        if !self.active.load(Ordering::Relaxed)
            || !matches!(
                event,
                WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)
            )
        {
            return None;
        }
        Some(Pending {
            trace: self,
            elapsed_ms: self.started.elapsed().as_millis(),
            events_start,
        })
    }
}

pub(super) struct Pending<'a> {
    trace: &'a Trace,
    elapsed_ms: u128,
    events_start: usize,
}

impl Pending<'_> {
    pub(super) fn finish(
        self,
        viewport: egui::ViewportId,
        event: &WindowEvent,
        events: &[egui::Event],
    ) {
        if !self.trace.active.load(Ordering::Relaxed) {
            return;
        }
        // ViewportId's Debug is only a 16-bit abbreviation. Keep the full identity.
        let mut line = format!(
            "ms={} viewport={:#018x} ",
            self.elapsed_ms,
            viewport.0.value()
        );
        match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                keyboard_description(
                    &mut line,
                    event.state,
                    event.repeat,
                    *is_synthetic,
                    &event.logical_key,
                    event.physical_key,
                    event.text.as_deref(),
                );
            }
            WindowEvent::Ime(ime) => {
                line.push_str("Ime ");
                ime_description(&mut line, ime);
            }
            _ => return,
        }
        line.push_str(" egui=[");
        appended_events(&mut line, &events[self.events_start..]);
        line.push_str("]\n");
        if self.trace.sender.send(line).is_err() {
            self.trace.active.store(false, Ordering::Relaxed);
        }
    }
}

// Never Debug-format text-bearing winit/egui events or logical Character/Dead keys.
// Printable physical/egui keys are masked too: their sequence can reveal passwords.
fn text_description(line: &mut String, text: &str) {
    let _ = write!(
        line,
        "bytes={},empty={},newline={}",
        text.len(),
        text.is_empty(),
        text.contains(['\r', '\n'])
    );
}

#[allow(clippy::too_many_arguments)]
fn keyboard_description(
    line: &mut String,
    state: ElementState,
    repeat: bool,
    synthetic: bool,
    logical: &Key,
    physical: PhysicalKey,
    text: Option<&str>,
) {
    let state = match state {
        ElementState::Pressed => "pressed",
        ElementState::Released => "released",
    };
    let _ = write!(
        line,
        "KeyboardInput state={state} repeat={repeat} is_synthetic={synthetic} logical="
    );
    match logical {
        Key::Named(winit::keyboard::NamedKey::Space) => {
            // Use the same metadata as Character(" "), not a Space-specific label.
            line.push_str("Character(");
            text_description(line, " ");
            line.push(')');
        }
        Key::Named(key) => {
            let _ = write!(line, "Named({key:?})");
        }
        Key::Character(text) => {
            line.push_str("Character(");
            text_description(line, text);
            line.push(')');
        }
        Key::Dead(value) => {
            let _ = write!(line, "Dead(present={})", value.is_some());
        }
        Key::Unidentified(_) => line.push_str("Unidentified"),
    }
    line.push_str(" physical=");
    match physical {
        PhysicalKey::Code(code) => {
            // Only non-text keys are named. Unknown codes may also be text keys.
            if let Some(key) = super::key_from_key_code(code).filter(|key| !is_text_key(*key)) {
                let _ = write!(line, "Code({code:?},egui={key:?})");
            } else {
                line.push_str("Code(CharOrOther)");
            }
        }
        PhysicalKey::Unidentified(_) => line.push_str("Unidentified"),
    }
    line.push_str(" text=");
    if let Some(text) = text {
        line.push_str("Some(");
        text_description(line, text);
        line.push(')');
    } else {
        line.push_str("None");
    }
}

fn is_text_key(key: egui::Key) -> bool {
    use egui::Key::*;
    // Space and the ISO 102nd printable key are outside the contiguous
    // punctuation/digit/letter groups (IntlBackslash was added in egui 0.37).
    matches!(key, Space | IntlBackslash) || (Colon..=Z).contains(&key)
}

fn ime_description(line: &mut String, ime: &Ime) {
    match ime {
        Ime::Enabled => line.push_str("Enabled"),
        Ime::Disabled => line.push_str("Disabled"),
        Ime::Preedit(text, cursor) => {
            line.push_str("Preedit(");
            text_description(line, text);
            let _ = write!(line, ",cursor={})", cursor.is_some());
        }
        Ime::Commit(text) => {
            line.push_str("Commit(");
            text_description(line, text);
            line.push(')');
        }
    }
}

fn appended_events(line: &mut String, events: &[egui::Event]) {
    if events.is_empty() {
        line.push_str("None");
    }
    for (index, event) in events.iter().enumerate() {
        if index != 0 {
            line.push(',');
        }
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                ..
            } => {
                line.push_str("Key(");
                egui_key_description(line, *key);
                line.push_str(",physical=");
                if let Some(key) = physical_key {
                    egui_key_description(line, *key);
                } else {
                    line.push_str("None");
                }
                let _ = write!(line, ",pressed={pressed},repeat={repeat})");
            }
            egui::Event::Cut => line.push_str("Cut"),
            egui::Event::Copy => line.push_str("Copy"),
            egui::Event::Paste(text) | egui::Event::Text(text) => {
                line.push_str(if matches!(event, egui::Event::Paste(_)) {
                    "Paste("
                } else {
                    "Text("
                });
                text_description(line, text);
                line.push(')');
            }
            egui::Event::Ime(ime) => {
                line.push_str("Ime.");
                match ime {
                    #[expect(
                        deprecated,
                        reason = "describe compatibility notifications without text"
                    )]
                    egui::ImeEvent::Enabled => line.push_str("Enabled"),
                    #[expect(
                        deprecated,
                        reason = "describe compatibility cancellation without text"
                    )]
                    egui::ImeEvent::Disabled => line.push_str("Disabled"),
                    egui::ImeEvent::DeleteSurrounding { .. } => line.push_str("DeleteSurrounding"),
                    egui::ImeEvent::Preedit { text, .. } | egui::ImeEvent::Commit(text) => {
                        line.push_str(if matches!(ime, egui::ImeEvent::Preedit { .. }) {
                            "Preedit("
                        } else {
                            "Commit("
                        });
                        text_description(line, text);
                        line.push(')');
                    }
                }
            }
            _ => line.push_str("Other"),
        }
    }
}

fn egui_key_description(line: &mut String, key: egui::Key) {
    if is_text_key(key) {
        line.push_str("Char");
    } else {
        let _ = write!(line, "{key:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{KeyCode, NamedKey};

    #[test]
    fn process_falls_back_to_physical_enter_in_upstream_mapping() {
        assert_eq!(
            super::super::key_from_winit_key(&Key::Named(NamedKey::Process)),
            None
        );
        let physical = super::super::key_from_key_code(KeyCode::Enter);
        assert_eq!(physical, Some(egui::Key::Enter));
        assert_eq!(
            super::super::key_from_winit_key(&Key::Named(NamedKey::Process)).or(physical),
            physical
        );
    }

    #[test]
    fn keyboard_metadata_keeps_process_and_enter_without_text() {
        let mut line = String::new();
        keyboard_description(
            &mut line,
            ElementState::Released,
            true,
            true,
            &Key::Named(NamedKey::Process),
            PhysicalKey::Code(KeyCode::Enter),
            Some("\r"),
        );
        assert_eq!(
            line,
            "KeyboardInput state=released repeat=true is_synthetic=true logical=Named(Process) physical=Code(Enter,egui=Enter) text=Some(bytes=1,empty=false,newline=true)"
        );
    }

    #[test]
    fn space_matches_other_character_keys_in_winit_metadata_and_keeps_process() {
        for state in [ElementState::Pressed, ElementState::Released] {
            for process in [false, true] {
                for has_text in [false, true] {
                    let mut space = String::new();
                    let mut character = String::new();
                    keyboard_description(
                        &mut space,
                        state,
                        false,
                        false,
                        &Key::Named(if process {
                            NamedKey::Process
                        } else {
                            NamedKey::Space
                        }),
                        PhysicalKey::Code(KeyCode::Space),
                        has_text.then_some(" "),
                    );
                    keyboard_description(
                        &mut character,
                        state,
                        false,
                        false,
                        &if process {
                            Key::Named(NamedKey::Process)
                        } else {
                            Key::Character("x".into())
                        },
                        PhysicalKey::Code(KeyCode::KeyX),
                        has_text.then_some("x"),
                    );
                    assert_eq!(space, character);
                    assert!(!space.contains("Space"));
                    assert_eq!(space.contains("logical=Named(Process)"), process);
                }
            }
        }
    }

    #[test]
    fn space_matches_other_character_keys_in_both_egui_key_fields() {
        for pressed in [false, true] {
            for (space_key, space_physical, character_key, character_physical) in [
                (
                    egui::Key::Space,
                    Some(egui::Key::Enter),
                    egui::Key::X,
                    Some(egui::Key::Enter),
                ),
                (
                    egui::Key::Enter,
                    Some(egui::Key::Space),
                    egui::Key::Enter,
                    Some(egui::Key::X),
                ),
                (
                    egui::Key::Space,
                    Some(egui::Key::Space),
                    egui::Key::X,
                    Some(egui::Key::X),
                ),
                (egui::Key::Space, None, egui::Key::X, None),
            ] {
                let describe = |key, physical_key| {
                    let mut line = String::new();
                    appended_events(
                        &mut line,
                        &[egui::Event::Key {
                            key,
                            physical_key,
                            pressed,
                            repeat: false,
                            modifiers: Default::default(),
                        }],
                    );
                    line
                };
                let space = describe(space_key, space_physical);
                assert_eq!(space, describe(character_key, character_physical));
                assert!(!space.contains("Space"));
            }
        }
    }

    #[test]
    fn intl_backslash_matches_other_character_keys_in_all_trace_fields() {
        for pressed in [false, true] {
            for process in [false, true] {
                let describe = |code, character: &str| {
                    let mut line = String::new();
                    keyboard_description(
                        &mut line,
                        if pressed {
                            ElementState::Pressed
                        } else {
                            ElementState::Released
                        },
                        false,
                        false,
                        &if process {
                            Key::Named(NamedKey::Process)
                        } else {
                            Key::Character(character.into())
                        },
                        PhysicalKey::Code(code),
                        Some(character),
                    );
                    line
                };
                let iso = describe(KeyCode::IntlBackslash, "\\");
                assert_eq!(iso, describe(KeyCode::KeyX, "x"));
                assert!(!iso.contains("IntlBackslash"));
                assert_eq!(iso.contains("Named(Process)"), process);
            }
            for (key, physical, ordinary, ordinary_physical) in [
                (egui::Key::IntlBackslash, None, egui::Key::X, None),
                (
                    egui::Key::IntlBackslash,
                    Some(egui::Key::Enter),
                    egui::Key::X,
                    Some(egui::Key::Enter),
                ),
                (
                    egui::Key::Enter,
                    Some(egui::Key::IntlBackslash),
                    egui::Key::Enter,
                    Some(egui::Key::X),
                ),
                (
                    egui::Key::IntlBackslash,
                    Some(egui::Key::IntlBackslash),
                    egui::Key::X,
                    Some(egui::Key::X),
                ),
            ] {
                let describe = |key, physical_key| {
                    let mut line = String::new();
                    appended_events(
                        &mut line,
                        &[egui::Event::Key {
                            key,
                            physical_key,
                            pressed,
                            repeat: false,
                            modifiers: Default::default(),
                        }],
                    );
                    line
                };
                let iso = describe(key, physical);
                assert_eq!(iso, describe(ordinary, ordinary_physical));
                assert!(!iso.contains("IntlBackslash"));
            }
        }
    }

    #[test]
    fn masks_character_dead_physical_and_egui_text_keys() {
        let mut line = String::new();
        keyboard_description(
            &mut line,
            ElementState::Pressed,
            false,
            false,
            &Key::Character("秘密\n".into()),
            PhysicalKey::Code(KeyCode::KeyP),
            Some("秘密\n"),
        );
        keyboard_description(
            &mut line,
            ElementState::Pressed,
            false,
            false,
            &Key::Dead(Some('秘')),
            PhysicalKey::Code(KeyCode::IntlYen),
            None,
        );
        keyboard_description(
            &mut line,
            ElementState::Pressed,
            false,
            false,
            &Key::Unidentified(winit::keyboard::NativeKey::Web("秘密".into())),
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Windows(0x1e)),
            None,
        );
        appended_events(
            &mut line,
            &[
                egui::Event::Key {
                    key: egui::Key::P,
                    physical_key: Some(egui::Key::P),
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                },
                egui::Event::Text("秘密\n".into()),
                egui::Event::Paste("秘密\n".into()),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "秘密\n".into(),
                    active_range_chars: Some(0..1),
                }),
                egui::Event::Ime(egui::ImeEvent::Commit("秘密\n".into())),
                egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
                    before_chars: 12345,
                    after_chars: 67890,
                }),
            ],
        );
        assert!(!line.contains('秘') && !line.contains('密') && !line.contains('\n'));
        assert!(!line.contains("KeyP") && !line.contains("IntlYen"));
        assert!(line.contains("Key(Char,physical=Char"));
        assert!(line.contains("bytes=7,empty=false,newline=true"));
        assert!(line.contains("Ime.DeleteSurrounding"));
        assert!(!line.contains("12345") && !line.contains("67890"));
    }

    #[test]
    fn describes_all_ime_variants_without_contents_or_cursor_positions() {
        let mut line = String::new();
        for ime in [
            Ime::Enabled,
            Ime::Preedit("秘密".into(), Some((3, 6))),
            Ime::Preedit(String::new(), None),
            Ime::Commit("秘密".into()),
            Ime::Disabled,
        ] {
            ime_description(&mut line, &ime);
            line.push('|');
        }
        assert_eq!(
            line,
            "Enabled|Preedit(bytes=6,empty=false,newline=false,cursor=true)|Preedit(bytes=0,empty=true,newline=false,cursor=false)|Commit(bytes=6,empty=false,newline=false)|Disabled|"
        );
    }

    fn trace_fixture() -> (Trace, Receiver<String>) {
        let (sender, receiver) = mpsc::channel();
        (
            Trace {
                started: web_time::Instant::now(),
                active: Arc::new(AtomicBool::new(true)),
                sender,
            },
            receiver,
        )
    }

    #[test]
    #[allow(deprecated, reason = "legacy IME compatibility fixture")]
    fn observes_only_appended_events_and_does_not_modify_them() {
        let (trace, receiver) = trace_fixture();
        let event = WindowEvent::Ime(Ime::Commit("秘密".into()));
        let events = vec![
            egui::Event::Copy,
            egui::Event::Ime(egui::ImeEvent::Commit("秘密".into())),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ];
        let original = events.clone();
        trace
            .begin(&event, 1)
            .unwrap()
            .finish(egui::ViewportId::ROOT, &event, &events);
        let line = receiver.recv().unwrap();
        assert_eq!(events, original);
        assert!(line.contains("viewport=0xffffffffffffffff"));
        assert!(
            line.ends_with("egui=[Ime.Commit(bytes=6,empty=false,newline=false),Ime.Disabled]\n")
        );
        assert!(!line.contains("Copy") && !line.contains("秘密"));
    }

    #[test]
    fn records_no_output_and_distinguishes_viewports_on_one_clock() {
        let (trace, receiver) = trace_fixture();
        let event = WindowEvent::Ime(Ime::Enabled);
        for viewport in [
            egui::ViewportId::ROOT,
            egui::ViewportId::from_hash_of("child"),
        ] {
            trace
                .begin(&event, 0)
                .unwrap()
                .finish(viewport, &event, &[]);
        }
        let a = receiver.recv().unwrap();
        let b = receiver.recv().unwrap();
        assert!(a.ends_with("egui=[None]\n") && b.ends_with("egui=[None]\n"));
        assert_ne!(a.split("viewport=").nth(1), b.split("viewport=").nth(1));
        let ms = |line: &str| {
            line.split_whitespace().next().unwrap()[3..]
                .parse::<u128>()
                .unwrap()
        };
        assert!(ms(&a) <= ms(&b));
    }

    #[test]
    fn inactive_or_unrelated_events_do_not_record() {
        let (trace, receiver) = trace_fixture();
        assert!(trace.begin(&WindowEvent::Focused(true), 0).is_none());
        trace.active.store(false, Ordering::Relaxed);
        assert!(trace.begin(&WindowEvent::Ime(Ime::Enabled), 0).is_none());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn disconnected_writer_silently_disables_trace() {
        let (trace, receiver) = trace_fixture();
        drop(receiver);
        let event = WindowEvent::Ime(Ime::Enabled);
        trace
            .begin(&event, 0)
            .unwrap()
            .finish(egui::ViewportId::ROOT, &event, &[]);
        assert!(!trace.active.load(Ordering::Relaxed));
        assert!(trace.begin(&event, 0).is_none());
    }

    #[test]
    fn writer_flushes_each_line_and_stops_on_write_or_flush_failure() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            flushes: usize,
            fail_write: bool,
            fail_flush: bool,
        }
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.fail_write {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                if self.fail_flush {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                Ok(())
            }
        }
        for (fail_write, fail_flush) in [(false, false), (true, false), (false, true)] {
            let (sender, receiver) = mpsc::channel();
            sender.send("first\n".into()).unwrap();
            sender.send("second\n".into()).unwrap();
            drop(sender);
            let active = AtomicBool::new(true);
            let mut writer = Writer {
                fail_write,
                fail_flush,
                ..Default::default()
            };
            write_lines(&mut writer, receiver, &active);
            assert_eq!(active.load(Ordering::Relaxed), !fail_write && !fail_flush);
            assert_eq!(
                writer.flushes,
                if fail_write {
                    0
                } else if fail_flush {
                    1
                } else {
                    2
                }
            );
            assert_eq!(
                writer.bytes,
                if fail_write {
                    b"".as_slice()
                } else if fail_flush {
                    b"first\n".as_slice()
                } else {
                    b"first\nsecond\n".as_slice()
                }
            );
        }
    }

    #[test]
    fn describes_cut_copy_paste_text_and_none_separately() {
        let mut line = String::new();
        appended_events(&mut line, &[]);
        assert_eq!(line, "None");
        line.clear();
        appended_events(
            &mut line,
            &[
                egui::Event::Cut,
                egui::Event::Copy,
                egui::Event::Paste(String::new()),
                egui::Event::Text("\r\n".into()),
            ],
        );
        assert_eq!(
            line,
            "Cut,Copy,Paste(bytes=0,empty=true,newline=false),Text(bytes=2,empty=false,newline=true)"
        );
    }
}
