use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyContext {
    Global,
    Grid,
    FsCommon,
    Rating,
    FsImage,
    FsVideo,
    Erase,
    Conceal,
    Crop,
    Text,
    LocalAdjust,
}

impl KeyContext {
    pub fn ini_name(self) -> &'static str {
        match self {
            KeyContext::Global => "Global",
            KeyContext::Grid => "Grid",
            KeyContext::FsCommon => "FsCommon",
            KeyContext::Rating => "Rating",
            KeyContext::FsImage => "FsImage",
            KeyContext::FsVideo => "FsVideo",
            KeyContext::Erase => "Erase",
            KeyContext::Conceal => "Conceal",
            KeyContext::Crop => "Crop",
            KeyContext::Text => "Text",
            KeyContext::LocalAdjust => "LocalAdjust",
        }
    }

    fn parse(section: &str) -> Option<Self> {
        let base = section.split('.').next().unwrap_or(section).trim();
        [
            KeyContext::Global,
            KeyContext::Grid,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsImage,
            KeyContext::FsVideo,
            KeyContext::Erase,
            KeyContext::Conceal,
            KeyContext::Crop,
            KeyContext::Text,
            KeyContext::LocalAdjust,
        ]
        .into_iter()
        .find(|ctx| ctx.ini_name().eq_ignore_ascii_case(base))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyTrigger {
    Press,
    ModifierHold,
    KeyHold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModKind {
    Ctrl,
    Shift,
    Alt,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyName {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    OpenBracket,
    CloseBracket,
}

impl KeyName {
    pub fn parse(s: &str) -> Option<Self> {
        let normalized = s.trim().replace([' ', '_', '-'], "");
        let upper = normalized.to_ascii_uppercase();
        Some(match upper.as_str() {
            "A" => KeyName::A,
            "B" => KeyName::B,
            "C" => KeyName::C,
            "D" => KeyName::D,
            "E" => KeyName::E,
            "F" => KeyName::F,
            "G" => KeyName::G,
            "H" => KeyName::H,
            "I" => KeyName::I,
            "J" => KeyName::J,
            "K" => KeyName::K,
            "L" => KeyName::L,
            "M" => KeyName::M,
            "N" => KeyName::N,
            "O" => KeyName::O,
            "P" => KeyName::P,
            "Q" => KeyName::Q,
            "R" => KeyName::R,
            "S" => KeyName::S,
            "T" => KeyName::T,
            "U" => KeyName::U,
            "V" => KeyName::V,
            "W" => KeyName::W,
            "X" => KeyName::X,
            "Y" => KeyName::Y,
            "Z" => KeyName::Z,
            "0" | "NUM0" | "DIGIT0" | "NUMPAD0" => KeyName::Num0,
            "1" | "NUM1" | "DIGIT1" | "NUMPAD1" => KeyName::Num1,
            "2" | "NUM2" | "DIGIT2" | "NUMPAD2" => KeyName::Num2,
            "3" | "NUM3" | "DIGIT3" | "NUMPAD3" => KeyName::Num3,
            "4" | "NUM4" | "DIGIT4" | "NUMPAD4" => KeyName::Num4,
            "5" | "NUM5" | "DIGIT5" | "NUMPAD5" => KeyName::Num5,
            "6" | "NUM6" | "DIGIT6" | "NUMPAD6" => KeyName::Num6,
            "7" | "NUM7" | "DIGIT7" | "NUMPAD7" => KeyName::Num7,
            "8" | "NUM8" | "DIGIT8" | "NUMPAD8" => KeyName::Num8,
            "9" | "NUM9" | "DIGIT9" | "NUMPAD9" => KeyName::Num9,
            "F1" => KeyName::F1,
            "F2" => KeyName::F2,
            "F3" => KeyName::F3,
            "F4" => KeyName::F4,
            "F5" => KeyName::F5,
            "F6" => KeyName::F6,
            "F7" => KeyName::F7,
            "F8" => KeyName::F8,
            "F9" => KeyName::F9,
            "F10" => KeyName::F10,
            "F11" => KeyName::F11,
            "F12" => KeyName::F12,
            "LEFT" | "ARROWLEFT" => KeyName::Left,
            "RIGHT" | "ARROWRIGHT" => KeyName::Right,
            "UP" | "ARROWUP" => KeyName::Up,
            "DOWN" | "ARROWDOWN" => KeyName::Down,
            "HOME" => KeyName::Home,
            "END" => KeyName::End,
            "PAGEUP" | "PGUP" => KeyName::PageUp,
            "PAGEDOWN" | "PGDN" => KeyName::PageDown,
            "SPACE" => KeyName::Space,
            "ENTER" | "RETURN" => KeyName::Enter,
            "ESC" | "ESCAPE" => KeyName::Esc,
            "TAB" => KeyName::Tab,
            "BACKSPACE" | "BS" => KeyName::Backspace,
            "DELETE" | "DEL" => KeyName::Delete,
            "[" | "OPENBRACKET" | "LBRACKET" => KeyName::OpenBracket,
            "]" | "CLOSEBRACKET" | "RBRACKET" => KeyName::CloseBracket,
            _ => return None,
        })
    }

    pub fn to_egui(self) -> egui::Key {
        match self {
            KeyName::A => egui::Key::A,
            KeyName::B => egui::Key::B,
            KeyName::C => egui::Key::C,
            KeyName::D => egui::Key::D,
            KeyName::E => egui::Key::E,
            KeyName::F => egui::Key::F,
            KeyName::G => egui::Key::G,
            KeyName::H => egui::Key::H,
            KeyName::I => egui::Key::I,
            KeyName::J => egui::Key::J,
            KeyName::K => egui::Key::K,
            KeyName::L => egui::Key::L,
            KeyName::M => egui::Key::M,
            KeyName::N => egui::Key::N,
            KeyName::O => egui::Key::O,
            KeyName::P => egui::Key::P,
            KeyName::Q => egui::Key::Q,
            KeyName::R => egui::Key::R,
            KeyName::S => egui::Key::S,
            KeyName::T => egui::Key::T,
            KeyName::U => egui::Key::U,
            KeyName::V => egui::Key::V,
            KeyName::W => egui::Key::W,
            KeyName::X => egui::Key::X,
            KeyName::Y => egui::Key::Y,
            KeyName::Z => egui::Key::Z,
            KeyName::Num0 => egui::Key::Num0,
            KeyName::Num1 => egui::Key::Num1,
            KeyName::Num2 => egui::Key::Num2,
            KeyName::Num3 => egui::Key::Num3,
            KeyName::Num4 => egui::Key::Num4,
            KeyName::Num5 => egui::Key::Num5,
            KeyName::Num6 => egui::Key::Num6,
            KeyName::Num7 => egui::Key::Num7,
            KeyName::Num8 => egui::Key::Num8,
            KeyName::Num9 => egui::Key::Num9,
            KeyName::F1 => egui::Key::F1,
            KeyName::F2 => egui::Key::F2,
            KeyName::F3 => egui::Key::F3,
            KeyName::F4 => egui::Key::F4,
            KeyName::F5 => egui::Key::F5,
            KeyName::F6 => egui::Key::F6,
            KeyName::F7 => egui::Key::F7,
            KeyName::F8 => egui::Key::F8,
            KeyName::F9 => egui::Key::F9,
            KeyName::F10 => egui::Key::F10,
            KeyName::F11 => egui::Key::F11,
            KeyName::F12 => egui::Key::F12,
            KeyName::Left => egui::Key::ArrowLeft,
            KeyName::Right => egui::Key::ArrowRight,
            KeyName::Up => egui::Key::ArrowUp,
            KeyName::Down => egui::Key::ArrowDown,
            KeyName::Home => egui::Key::Home,
            KeyName::End => egui::Key::End,
            KeyName::PageUp => egui::Key::PageUp,
            KeyName::PageDown => egui::Key::PageDown,
            KeyName::Space => egui::Key::Space,
            KeyName::Enter => egui::Key::Enter,
            KeyName::Esc => egui::Key::Escape,
            KeyName::Tab => egui::Key::Tab,
            KeyName::Backspace => egui::Key::Backspace,
            KeyName::Delete => egui::Key::Delete,
            KeyName::OpenBracket => egui::Key::OpenBracket,
            KeyName::CloseBracket => egui::Key::CloseBracket,
        }
    }

    pub fn from_egui(key: egui::Key) -> Option<Self> {
        Some(match key {
            egui::Key::A => KeyName::A,
            egui::Key::B => KeyName::B,
            egui::Key::C => KeyName::C,
            egui::Key::D => KeyName::D,
            egui::Key::E => KeyName::E,
            egui::Key::F => KeyName::F,
            egui::Key::G => KeyName::G,
            egui::Key::H => KeyName::H,
            egui::Key::I => KeyName::I,
            egui::Key::J => KeyName::J,
            egui::Key::K => KeyName::K,
            egui::Key::L => KeyName::L,
            egui::Key::M => KeyName::M,
            egui::Key::N => KeyName::N,
            egui::Key::O => KeyName::O,
            egui::Key::P => KeyName::P,
            egui::Key::Q => KeyName::Q,
            egui::Key::R => KeyName::R,
            egui::Key::S => KeyName::S,
            egui::Key::T => KeyName::T,
            egui::Key::U => KeyName::U,
            egui::Key::V => KeyName::V,
            egui::Key::W => KeyName::W,
            egui::Key::X => KeyName::X,
            egui::Key::Y => KeyName::Y,
            egui::Key::Z => KeyName::Z,
            egui::Key::Num0 => KeyName::Num0,
            egui::Key::Num1 => KeyName::Num1,
            egui::Key::Num2 => KeyName::Num2,
            egui::Key::Num3 => KeyName::Num3,
            egui::Key::Num4 => KeyName::Num4,
            egui::Key::Num5 => KeyName::Num5,
            egui::Key::Num6 => KeyName::Num6,
            egui::Key::Num7 => KeyName::Num7,
            egui::Key::Num8 => KeyName::Num8,
            egui::Key::Num9 => KeyName::Num9,
            egui::Key::F1 => KeyName::F1,
            egui::Key::F2 => KeyName::F2,
            egui::Key::F3 => KeyName::F3,
            egui::Key::F4 => KeyName::F4,
            egui::Key::F5 => KeyName::F5,
            egui::Key::F6 => KeyName::F6,
            egui::Key::F7 => KeyName::F7,
            egui::Key::F8 => KeyName::F8,
            egui::Key::F9 => KeyName::F9,
            egui::Key::F10 => KeyName::F10,
            egui::Key::F11 => KeyName::F11,
            egui::Key::F12 => KeyName::F12,
            egui::Key::ArrowLeft => KeyName::Left,
            egui::Key::ArrowRight => KeyName::Right,
            egui::Key::ArrowUp => KeyName::Up,
            egui::Key::ArrowDown => KeyName::Down,
            egui::Key::Home => KeyName::Home,
            egui::Key::End => KeyName::End,
            egui::Key::PageUp => KeyName::PageUp,
            egui::Key::PageDown => KeyName::PageDown,
            egui::Key::Space => KeyName::Space,
            egui::Key::Enter => KeyName::Enter,
            egui::Key::Escape => KeyName::Esc,
            egui::Key::Tab => KeyName::Tab,
            egui::Key::Backspace => KeyName::Backspace,
            egui::Key::Delete => KeyName::Delete,
            egui::Key::OpenBracket => KeyName::OpenBracket,
            egui::Key::CloseBracket => KeyName::CloseBracket,
            _ => return None,
        })
    }

    pub fn to_vk(self) -> u32 {
        match self {
            KeyName::A => 0x41,
            KeyName::B => 0x42,
            KeyName::C => 0x43,
            KeyName::D => 0x44,
            KeyName::E => 0x45,
            KeyName::F => 0x46,
            KeyName::G => 0x47,
            KeyName::H => 0x48,
            KeyName::I => 0x49,
            KeyName::J => 0x4A,
            KeyName::K => 0x4B,
            KeyName::L => 0x4C,
            KeyName::M => 0x4D,
            KeyName::N => 0x4E,
            KeyName::O => 0x4F,
            KeyName::P => 0x50,
            KeyName::Q => 0x51,
            KeyName::R => 0x52,
            KeyName::S => 0x53,
            KeyName::T => 0x54,
            KeyName::U => 0x55,
            KeyName::V => 0x56,
            KeyName::W => 0x57,
            KeyName::X => 0x58,
            KeyName::Y => 0x59,
            KeyName::Z => 0x5A,
            KeyName::Num0 => 0x30,
            KeyName::Num1 => 0x31,
            KeyName::Num2 => 0x32,
            KeyName::Num3 => 0x33,
            KeyName::Num4 => 0x34,
            KeyName::Num5 => 0x35,
            KeyName::Num6 => 0x36,
            KeyName::Num7 => 0x37,
            KeyName::Num8 => 0x38,
            KeyName::Num9 => 0x39,
            KeyName::F1 => 0x70,
            KeyName::F2 => 0x71,
            KeyName::F3 => 0x72,
            KeyName::F4 => 0x73,
            KeyName::F5 => 0x74,
            KeyName::F6 => 0x75,
            KeyName::F7 => 0x76,
            KeyName::F8 => 0x77,
            KeyName::F9 => 0x78,
            KeyName::F10 => 0x79,
            KeyName::F11 => 0x7A,
            KeyName::F12 => 0x7B,
            KeyName::Left => 0x25,
            KeyName::Right => 0x27,
            KeyName::Up => 0x26,
            KeyName::Down => 0x28,
            KeyName::Home => 0x24,
            KeyName::End => 0x23,
            KeyName::PageUp => 0x21,
            KeyName::PageDown => 0x22,
            KeyName::Space => 0x20,
            KeyName::Enter => 0x0D,
            KeyName::Esc => 0x1B,
            KeyName::Tab => 0x09,
            KeyName::Backspace => 0x08,
            KeyName::Delete => 0x2E,
            KeyName::OpenBracket => 0xDB,
            KeyName::CloseBracket => 0xDD,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            KeyName::A => "A",
            KeyName::B => "B",
            KeyName::C => "C",
            KeyName::D => "D",
            KeyName::E => "E",
            KeyName::F => "F",
            KeyName::G => "G",
            KeyName::H => "H",
            KeyName::I => "I",
            KeyName::J => "J",
            KeyName::K => "K",
            KeyName::L => "L",
            KeyName::M => "M",
            KeyName::N => "N",
            KeyName::O => "O",
            KeyName::P => "P",
            KeyName::Q => "Q",
            KeyName::R => "R",
            KeyName::S => "S",
            KeyName::T => "T",
            KeyName::U => "U",
            KeyName::V => "V",
            KeyName::W => "W",
            KeyName::X => "X",
            KeyName::Y => "Y",
            KeyName::Z => "Z",
            KeyName::Num0 => "0",
            KeyName::Num1 => "1",
            KeyName::Num2 => "2",
            KeyName::Num3 => "3",
            KeyName::Num4 => "4",
            KeyName::Num5 => "5",
            KeyName::Num6 => "6",
            KeyName::Num7 => "7",
            KeyName::Num8 => "8",
            KeyName::Num9 => "9",
            KeyName::F1 => "F1",
            KeyName::F2 => "F2",
            KeyName::F3 => "F3",
            KeyName::F4 => "F4",
            KeyName::F5 => "F5",
            KeyName::F6 => "F6",
            KeyName::F7 => "F7",
            KeyName::F8 => "F8",
            KeyName::F9 => "F9",
            KeyName::F10 => "F10",
            KeyName::F11 => "F11",
            KeyName::F12 => "F12",
            KeyName::Left => "Left",
            KeyName::Right => "Right",
            KeyName::Up => "Up",
            KeyName::Down => "Down",
            KeyName::Home => "Home",
            KeyName::End => "End",
            KeyName::PageUp => "PageUp",
            KeyName::PageDown => "PageDown",
            KeyName::Space => "Space",
            KeyName::Enter => "Enter",
            KeyName::Esc => "Esc",
            KeyName::Tab => "Tab",
            KeyName::Backspace => "Backspace",
            KeyName::Delete => "Delete",
            KeyName::OpenBracket => "OpenBracket",
            KeyName::CloseBracket => "CloseBracket",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Option<KeyName>,
}

impl Chord {
    pub const NONE: Chord = Chord {
        ctrl: false,
        shift: false,
        alt: false,
        key: None,
    };

    pub const fn new(ctrl: bool, shift: bool, alt: bool, key: KeyName) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            key: Some(key),
        }
    }

    pub const fn key(key: KeyName) -> Self {
        Self::new(false, false, false, key)
    }

    pub const fn ctrl(key: KeyName) -> Self {
        Self::new(true, false, false, key)
    }

    pub const fn shift(key: KeyName) -> Self {
        Self::new(false, true, false, key)
    }

    pub const fn alt(key: KeyName) -> Self {
        Self::new(false, false, true, key)
    }

    pub const fn ctrl_shift(key: KeyName) -> Self {
        Self::new(true, true, false, key)
    }

    pub const fn modifier(kind: ModKind) -> Self {
        match kind {
            ModKind::Ctrl => Self {
                ctrl: true,
                shift: false,
                alt: false,
                key: None,
            },
            ModKind::Shift => Self {
                ctrl: false,
                shift: true,
                alt: false,
                key: None,
            },
            ModKind::Alt => Self {
                ctrl: false,
                shift: false,
                alt: true,
                key: None,
            },
        }
    }

    fn matches_egui(self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        self.key.is_some_and(|name| name.to_egui() == key)
            && modifiers.ctrl == self.ctrl
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
            && !modifiers.mac_cmd
    }

    fn matches_modifiers(self, modifiers: egui::Modifiers) -> bool {
        self.key.is_none()
            && modifiers.ctrl == self.ctrl
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
            && !modifiers.mac_cmd
    }

    fn matches_vk_parts(self, virtual_key: u32, ctrl: bool, shift: bool, alt: bool) -> bool {
        self.key.is_some_and(|name| name.to_vk() == virtual_key)
            && self.ctrl == ctrl
            && self.shift == shift
            && self.alt == alt
    }

    fn validate_for_trigger(self, trigger: KeyTrigger) -> Result<(), &'static str> {
        match trigger {
            KeyTrigger::Press => {
                if self.key.is_some() {
                    Ok(())
                } else {
                    Err("Press actions require a normal key")
                }
            }
            KeyTrigger::ModifierHold => {
                let mod_count = self.ctrl as u8 + self.shift as u8 + self.alt as u8;
                if self.key.is_none() && mod_count == 1 {
                    Ok(())
                } else {
                    Err("ModifierHold actions accept exactly one modifier key")
                }
            }
            KeyTrigger::KeyHold => {
                if self.key.is_some() && !self.ctrl && !self.shift && !self.alt {
                    Ok(())
                } else {
                    Err("KeyHold actions accept one normal key without modifiers")
                }
            }
        }
    }

    pub fn display_name(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if let Some(key) = self.key {
            parts.push(key.display_name().to_string());
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ChordList {
    chords: [Chord; 3],
    len: usize,
}

impl ChordList {
    pub const EMPTY: Self = Self {
        chords: [Chord::NONE; 3],
        len: 0,
    };

    pub const fn one(a: Chord) -> Self {
        Self {
            chords: [a, Chord::NONE, Chord::NONE],
            len: 1,
        }
    }

    pub const fn two(a: Chord, b: Chord) -> Self {
        Self {
            chords: [a, b, Chord::NONE],
            len: 2,
        }
    }

    pub const fn three(a: Chord, b: Chord, c: Chord) -> Self {
        Self {
            chords: [a, b, c],
            len: 3,
        }
    }

    pub fn iter(self) -> impl Iterator<Item = Chord> {
        self.chords.into_iter().take(self.len)
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyAction {
    GlobalLocalSearch,
    GlobalFavSearch,
    GlobalMetadataSearch,
    GlobalOpenFolder,
    GridSelectAll,
    GridDeselect,
    GridToggleCheck,
    GridRotateCw,
    GridRotateCcw,
    GridPin,
    GridComparePin,
    GridColumnCount1,
    GridColumnCount2,
    GridColumnCount3,
    GridColumnCount4,
    GridColumnCount5,
    GridColumnCount6,
    GridColumnCount7,
    GridColumnCount8,
    GridColumnCount9,
    GridColumnCount10,
    GridAdjustSlot1,
    GridAdjustSlot2,
    GridAdjustSlot3,
    GridAdjustSlot4,
    GridAdjustSlot5,
    GridAdjustSlot6,
    GridAdjustSlot7,
    GridAdjustSlot8,
    GridAdjustSlot9,
    GridAdjustSlot10,
    GridClearAdjust,
    FsToggleMetadata,
    FsCtrlNavPrev,
    FsCtrlNavNext,
    FsSiblingPrev,
    FsSiblingNext,
    RatingItem1,
    RatingItem2,
    RatingItem3,
    RatingItem4,
    RatingItem5,
    RatingItemClear,
    RatingContainer1,
    RatingContainer2,
    RatingContainer3,
    RatingContainer4,
    RatingContainer5,
    RatingContainerClear,
    FsContinuousScrollForward,
    FsContinuousScrollBack,
    FsSpreadShiftLeft,
    FsSpreadShiftRight,
    FsSlideshow,
    FsSpaceCheck,
    FsCapture,
    FsExport,
    FsCompareToggle,
    FsCompareCycle,
    FsCompareWipe,
    FsCompareDiff,
    FsRotateCw,
    FsRotateCcw,
    FsAnalysis,
    FsPanorama,
    FsPixelGrid,
    FsLoupeLockToggle,
    FsLoupeHold,
    FsEraseMode,
    FsConcealMode,
    FsTextMode,
    FsBgCycle,
    FsPin,
    FsSpreadSingle,
    FsSpreadLtr,
    FsSpreadLtrCover,
    FsSpreadRtl,
    FsSpreadRtlCover,
    FsReadingFlowCycle,
    FsReadingDirectionToggle,
    FsFitModeCycle,
    FsAiModelNext,
    FsAiModelPrev,
    FsAiModelReset,
    FsDenoiseCycle,
    FsPostFilterNext,
    FsPostFilterPrev,
    FsPostFilterReset,
    FsAdjustSlot1,
    FsAdjustSlot2,
    FsAdjustSlot3,
    FsAdjustSlot4,
    FsAdjustSlot5,
    FsAdjustSlot6,
    FsAdjustSlot7,
    FsAdjustSlot8,
    FsAdjustSlot9,
    FsAdjustSlot10,
    FsClearAdjust,
    FsApplyErase1,
    FsApplyErase2,
    FsApplyConceal1,
    FsApplyConceal2,
    FsDeleteEraseMask,
    FsDeleteConcealMask,
    VideoExternalPlayer,
    VideoPlayPause,
    VideoSeekStart,
    VideoVolumeUp,
    VideoVolumeDown,
    VideoNextFile,
    VideoPrevFile,
    VideoMute,
    VideoLoop,
    VideoMarkerPrev,
    VideoMarkerNext,
    VideoPin,
    VideoPerfOverlay,
    VideoTileMode,
    VideoBookmark,
    VideoCapture,
    VideoCompareToggle,
    VideoCompareCycle,
    VideoCompareWipe,
    VideoCompareDiff,
    EraseConfirm,
    EraseUndo,
    EraseDeleteShape,
    EraseToolSelect,
    EraseToolBrush,
    EraseToolLasso,
    EraseToolPolygon,
    EraseToolVLine,
    EraseToolHLine,
    EraseToolLine,
    EraseToolRect,
    EraseToolEllipse,
    ErasePaintMode,
    EraseEraseMode,
    EraseSpacePan,
    ConcealExit,
    ConcealUndo,
    ConcealDeleteShape,
    ConcealPixelGrid,
    ConcealTypeCycle,
    ConcealPreset1,
    ConcealPreset2,
    ConcealPreset3,
    ConcealPreset4,
    ConcealPaintMode,
    ConcealEraseMode,
    ConcealToolSelect,
    ConcealToolBrush,
    ConcealToolLasso,
    ConcealToolPolygon,
    ConcealToolLine,
    ConcealToolVLine,
    ConcealToolHLine,
    ConcealToolRect,
    ConcealToolEllipse,
    ConcealSpacePan,
    CropSpacePan,
    CropExecute,
    TextConfirm,
    TextRedo,
    TextUndo,
    TextSpacePan,
    LaShowSource,
    LaShowMask,
    LaPaintAdd,
    LaPaintErase,
    LaToolBrush,
    LaToolEdgeBrush,
    LaToolGapFill,
    LaToolLasso,
    LaToolPolygon,
    LaToolSelect,
    LaToolLine,
    LaToolVLine,
    LaToolHLine,
    LaToolRect,
    LaToolEllipse,
    LaSpacePan,
}

const ALL_ACTIONS: &[KeyAction] = &[
    KeyAction::GlobalLocalSearch,
    KeyAction::GlobalFavSearch,
    KeyAction::GlobalMetadataSearch,
    KeyAction::GlobalOpenFolder,
    KeyAction::GridSelectAll,
    KeyAction::GridDeselect,
    KeyAction::GridToggleCheck,
    KeyAction::GridRotateCw,
    KeyAction::GridRotateCcw,
    KeyAction::GridPin,
    KeyAction::GridComparePin,
    KeyAction::GridColumnCount1,
    KeyAction::GridColumnCount2,
    KeyAction::GridColumnCount3,
    KeyAction::GridColumnCount4,
    KeyAction::GridColumnCount5,
    KeyAction::GridColumnCount6,
    KeyAction::GridColumnCount7,
    KeyAction::GridColumnCount8,
    KeyAction::GridColumnCount9,
    KeyAction::GridColumnCount10,
    KeyAction::GridAdjustSlot1,
    KeyAction::GridAdjustSlot2,
    KeyAction::GridAdjustSlot3,
    KeyAction::GridAdjustSlot4,
    KeyAction::GridAdjustSlot5,
    KeyAction::GridAdjustSlot6,
    KeyAction::GridAdjustSlot7,
    KeyAction::GridAdjustSlot8,
    KeyAction::GridAdjustSlot9,
    KeyAction::GridAdjustSlot10,
    KeyAction::GridClearAdjust,
    KeyAction::FsToggleMetadata,
    KeyAction::FsCtrlNavPrev,
    KeyAction::FsCtrlNavNext,
    KeyAction::FsSiblingPrev,
    KeyAction::FsSiblingNext,
    KeyAction::RatingItem1,
    KeyAction::RatingItem2,
    KeyAction::RatingItem3,
    KeyAction::RatingItem4,
    KeyAction::RatingItem5,
    KeyAction::RatingItemClear,
    KeyAction::RatingContainer1,
    KeyAction::RatingContainer2,
    KeyAction::RatingContainer3,
    KeyAction::RatingContainer4,
    KeyAction::RatingContainer5,
    KeyAction::RatingContainerClear,
    KeyAction::FsContinuousScrollForward,
    KeyAction::FsContinuousScrollBack,
    KeyAction::FsSpreadShiftLeft,
    KeyAction::FsSpreadShiftRight,
    KeyAction::FsSlideshow,
    KeyAction::FsSpaceCheck,
    KeyAction::FsCapture,
    KeyAction::FsExport,
    KeyAction::FsCompareToggle,
    KeyAction::FsCompareCycle,
    KeyAction::FsCompareWipe,
    KeyAction::FsCompareDiff,
    KeyAction::FsRotateCw,
    KeyAction::FsRotateCcw,
    KeyAction::FsAnalysis,
    KeyAction::FsPanorama,
    KeyAction::FsPixelGrid,
    KeyAction::FsLoupeLockToggle,
    KeyAction::FsLoupeHold,
    KeyAction::FsEraseMode,
    KeyAction::FsConcealMode,
    KeyAction::FsTextMode,
    KeyAction::FsBgCycle,
    KeyAction::FsPin,
    KeyAction::FsSpreadSingle,
    KeyAction::FsSpreadLtr,
    KeyAction::FsSpreadLtrCover,
    KeyAction::FsSpreadRtl,
    KeyAction::FsSpreadRtlCover,
    KeyAction::FsReadingFlowCycle,
    KeyAction::FsReadingDirectionToggle,
    KeyAction::FsFitModeCycle,
    KeyAction::FsAiModelNext,
    KeyAction::FsAiModelPrev,
    KeyAction::FsAiModelReset,
    KeyAction::FsDenoiseCycle,
    KeyAction::FsPostFilterNext,
    KeyAction::FsPostFilterPrev,
    KeyAction::FsPostFilterReset,
    KeyAction::FsAdjustSlot1,
    KeyAction::FsAdjustSlot2,
    KeyAction::FsAdjustSlot3,
    KeyAction::FsAdjustSlot4,
    KeyAction::FsAdjustSlot5,
    KeyAction::FsAdjustSlot6,
    KeyAction::FsAdjustSlot7,
    KeyAction::FsAdjustSlot8,
    KeyAction::FsAdjustSlot9,
    KeyAction::FsAdjustSlot10,
    KeyAction::FsClearAdjust,
    KeyAction::FsApplyErase1,
    KeyAction::FsApplyErase2,
    KeyAction::FsApplyConceal1,
    KeyAction::FsApplyConceal2,
    KeyAction::FsDeleteEraseMask,
    KeyAction::FsDeleteConcealMask,
    KeyAction::VideoExternalPlayer,
    KeyAction::VideoPlayPause,
    KeyAction::VideoSeekStart,
    KeyAction::VideoVolumeUp,
    KeyAction::VideoVolumeDown,
    KeyAction::VideoNextFile,
    KeyAction::VideoPrevFile,
    KeyAction::VideoMute,
    KeyAction::VideoLoop,
    KeyAction::VideoMarkerPrev,
    KeyAction::VideoMarkerNext,
    KeyAction::VideoPin,
    KeyAction::VideoPerfOverlay,
    KeyAction::VideoTileMode,
    KeyAction::VideoBookmark,
    KeyAction::VideoCapture,
    KeyAction::VideoCompareToggle,
    KeyAction::VideoCompareCycle,
    KeyAction::VideoCompareWipe,
    KeyAction::VideoCompareDiff,
    KeyAction::EraseConfirm,
    KeyAction::EraseUndo,
    KeyAction::EraseDeleteShape,
    KeyAction::EraseToolSelect,
    KeyAction::EraseToolBrush,
    KeyAction::EraseToolLasso,
    KeyAction::EraseToolPolygon,
    KeyAction::EraseToolVLine,
    KeyAction::EraseToolHLine,
    KeyAction::EraseToolLine,
    KeyAction::EraseToolRect,
    KeyAction::EraseToolEllipse,
    KeyAction::ErasePaintMode,
    KeyAction::EraseEraseMode,
    KeyAction::EraseSpacePan,
    KeyAction::ConcealExit,
    KeyAction::ConcealUndo,
    KeyAction::ConcealDeleteShape,
    KeyAction::ConcealPixelGrid,
    KeyAction::ConcealTypeCycle,
    KeyAction::ConcealPreset1,
    KeyAction::ConcealPreset2,
    KeyAction::ConcealPreset3,
    KeyAction::ConcealPreset4,
    KeyAction::ConcealPaintMode,
    KeyAction::ConcealEraseMode,
    KeyAction::ConcealToolSelect,
    KeyAction::ConcealToolBrush,
    KeyAction::ConcealToolLasso,
    KeyAction::ConcealToolPolygon,
    KeyAction::ConcealToolLine,
    KeyAction::ConcealToolVLine,
    KeyAction::ConcealToolHLine,
    KeyAction::ConcealToolRect,
    KeyAction::ConcealToolEllipse,
    KeyAction::ConcealSpacePan,
    KeyAction::CropSpacePan,
    KeyAction::CropExecute,
    KeyAction::TextConfirm,
    KeyAction::TextRedo,
    KeyAction::TextUndo,
    KeyAction::TextSpacePan,
    KeyAction::LaShowSource,
    KeyAction::LaShowMask,
    KeyAction::LaPaintAdd,
    KeyAction::LaPaintErase,
    KeyAction::LaToolBrush,
    KeyAction::LaToolEdgeBrush,
    KeyAction::LaToolGapFill,
    KeyAction::LaToolLasso,
    KeyAction::LaToolPolygon,
    KeyAction::LaToolSelect,
    KeyAction::LaToolLine,
    KeyAction::LaToolVLine,
    KeyAction::LaToolHLine,
    KeyAction::LaToolRect,
    KeyAction::LaToolEllipse,
    KeyAction::LaSpacePan,
];
// Keep this list in sync with `KeyAction`. The keymap tests compare the enum
// inventory and this array so newly added actions cannot silently miss ini generation.

const RATING_ITEM_ACTIONS: &[(KeyAction, u8)] = &[
    (KeyAction::RatingItem1, 1),
    (KeyAction::RatingItem2, 2),
    (KeyAction::RatingItem3, 3),
    (KeyAction::RatingItem4, 4),
    (KeyAction::RatingItem5, 5),
    (KeyAction::RatingItemClear, 0),
];

const RATING_CONTAINER_ACTIONS: &[(KeyAction, u8)] = &[
    (KeyAction::RatingContainer1, 1),
    (KeyAction::RatingContainer2, 2),
    (KeyAction::RatingContainer3, 3),
    (KeyAction::RatingContainer4, 4),
    (KeyAction::RatingContainer5, 5),
    (KeyAction::RatingContainerClear, 0),
];

fn rating_actions(container: bool) -> &'static [(KeyAction, u8)] {
    if container {
        RATING_CONTAINER_ACTIONS
    } else {
        RATING_ITEM_ACTIONS
    }
}

impl KeyAction {
    pub fn all() -> &'static [KeyAction] {
        ALL_ACTIONS
    }

    pub fn ini_name(self) -> &'static str {
        use KeyAction::*;
        match self {
            GlobalLocalSearch => "GlobalLocalSearch",
            GlobalFavSearch => "GlobalFavSearch",
            GlobalMetadataSearch => "GlobalMetadataSearch",
            GlobalOpenFolder => "GlobalOpenFolder",
            GridSelectAll => "GridSelectAll",
            GridDeselect => "GridDeselect",
            GridToggleCheck => "GridToggleCheck",
            GridRotateCw => "GridRotateCw",
            GridRotateCcw => "GridRotateCcw",
            GridPin => "GridPin",
            GridComparePin => "GridComparePin",
            GridColumnCount1 => "GridColumnCount1",
            GridColumnCount2 => "GridColumnCount2",
            GridColumnCount3 => "GridColumnCount3",
            GridColumnCount4 => "GridColumnCount4",
            GridColumnCount5 => "GridColumnCount5",
            GridColumnCount6 => "GridColumnCount6",
            GridColumnCount7 => "GridColumnCount7",
            GridColumnCount8 => "GridColumnCount8",
            GridColumnCount9 => "GridColumnCount9",
            GridColumnCount10 => "GridColumnCount10",
            GridAdjustSlot1 => "GridAdjustSlot1",
            GridAdjustSlot2 => "GridAdjustSlot2",
            GridAdjustSlot3 => "GridAdjustSlot3",
            GridAdjustSlot4 => "GridAdjustSlot4",
            GridAdjustSlot5 => "GridAdjustSlot5",
            GridAdjustSlot6 => "GridAdjustSlot6",
            GridAdjustSlot7 => "GridAdjustSlot7",
            GridAdjustSlot8 => "GridAdjustSlot8",
            GridAdjustSlot9 => "GridAdjustSlot9",
            GridAdjustSlot10 => "GridAdjustSlot10",
            GridClearAdjust => "GridClearAdjust",
            FsToggleMetadata => "FsToggleMetadata",
            FsCtrlNavPrev => "FsCtrlNavPrev",
            FsCtrlNavNext => "FsCtrlNavNext",
            FsSiblingPrev => "FsSiblingPrev",
            FsSiblingNext => "FsSiblingNext",
            RatingItem1 => "RatingItem1",
            RatingItem2 => "RatingItem2",
            RatingItem3 => "RatingItem3",
            RatingItem4 => "RatingItem4",
            RatingItem5 => "RatingItem5",
            RatingItemClear => "RatingItemClear",
            RatingContainer1 => "RatingContainer1",
            RatingContainer2 => "RatingContainer2",
            RatingContainer3 => "RatingContainer3",
            RatingContainer4 => "RatingContainer4",
            RatingContainer5 => "RatingContainer5",
            RatingContainerClear => "RatingContainerClear",
            FsContinuousScrollForward => "FsContinuousScrollForward",
            FsContinuousScrollBack => "FsContinuousScrollBack",
            FsSpreadShiftLeft => "FsSpreadShiftLeft",
            FsSpreadShiftRight => "FsSpreadShiftRight",
            FsSlideshow => "FsSlideshow",
            FsSpaceCheck => "FsSpaceCheck",
            FsCapture => "FsCapture",
            FsExport => "FsExport",
            FsCompareToggle => "FsCompareToggle",
            FsCompareCycle => "FsCompareCycle",
            FsCompareWipe => "FsCompareWipe",
            FsCompareDiff => "FsCompareDiff",
            FsRotateCw => "FsRotateCw",
            FsRotateCcw => "FsRotateCcw",
            FsAnalysis => "FsAnalysis",
            FsPanorama => "FsPanorama",
            FsPixelGrid => "FsPixelGrid",
            FsLoupeLockToggle => "FsLoupeLockToggle",
            FsLoupeHold => "FsLoupeHold",
            FsEraseMode => "FsEraseMode",
            FsConcealMode => "FsConcealMode",
            FsTextMode => "FsTextMode",
            FsBgCycle => "FsBgCycle",
            FsPin => "FsPin",
            FsSpreadSingle => "FsSpreadSingle",
            FsSpreadLtr => "FsSpreadLtr",
            FsSpreadLtrCover => "FsSpreadLtrCover",
            FsSpreadRtl => "FsSpreadRtl",
            FsSpreadRtlCover => "FsSpreadRtlCover",
            FsReadingFlowCycle => "FsReadingFlowCycle",
            FsReadingDirectionToggle => "FsReadingDirectionToggle",
            FsFitModeCycle => "FsFitModeCycle",
            FsAiModelNext => "FsAiModelNext",
            FsAiModelPrev => "FsAiModelPrev",
            FsAiModelReset => "FsAiModelReset",
            FsDenoiseCycle => "FsDenoiseCycle",
            FsPostFilterNext => "FsPostFilterNext",
            FsPostFilterPrev => "FsPostFilterPrev",
            FsPostFilterReset => "FsPostFilterReset",
            FsAdjustSlot1 => "FsAdjustSlot1",
            FsAdjustSlot2 => "FsAdjustSlot2",
            FsAdjustSlot3 => "FsAdjustSlot3",
            FsAdjustSlot4 => "FsAdjustSlot4",
            FsAdjustSlot5 => "FsAdjustSlot5",
            FsAdjustSlot6 => "FsAdjustSlot6",
            FsAdjustSlot7 => "FsAdjustSlot7",
            FsAdjustSlot8 => "FsAdjustSlot8",
            FsAdjustSlot9 => "FsAdjustSlot9",
            FsAdjustSlot10 => "FsAdjustSlot10",
            FsClearAdjust => "FsClearAdjust",
            FsApplyErase1 => "FsApplyErase1",
            FsApplyErase2 => "FsApplyErase2",
            FsApplyConceal1 => "FsApplyConceal1",
            FsApplyConceal2 => "FsApplyConceal2",
            FsDeleteEraseMask => "FsDeleteEraseMask",
            FsDeleteConcealMask => "FsDeleteConcealMask",
            VideoExternalPlayer => "VideoExternalPlayer",
            VideoPlayPause => "VideoPlayPause",
            VideoSeekStart => "VideoSeekStart",
            VideoVolumeUp => "VideoVolumeUp",
            VideoVolumeDown => "VideoVolumeDown",
            VideoNextFile => "VideoNextFile",
            VideoPrevFile => "VideoPrevFile",
            VideoMute => "VideoMute",
            VideoLoop => "VideoLoop",
            VideoMarkerPrev => "VideoMarkerPrev",
            VideoMarkerNext => "VideoMarkerNext",
            VideoPin => "VideoPin",
            VideoPerfOverlay => "VideoPerfOverlay",
            VideoTileMode => "VideoTileMode",
            VideoBookmark => "VideoBookmark",
            VideoCapture => "VideoCapture",
            VideoCompareToggle => "VideoCompareToggle",
            VideoCompareCycle => "VideoCompareCycle",
            VideoCompareWipe => "VideoCompareWipe",
            VideoCompareDiff => "VideoCompareDiff",
            EraseConfirm => "EraseConfirm",
            EraseUndo => "EraseUndo",
            EraseDeleteShape => "EraseDeleteShape",
            EraseToolSelect => "EraseToolSelect",
            EraseToolBrush => "EraseToolBrush",
            EraseToolLasso => "EraseToolLasso",
            EraseToolPolygon => "EraseToolPolygon",
            EraseToolVLine => "EraseToolVLine",
            EraseToolHLine => "EraseToolHLine",
            EraseToolLine => "EraseToolLine",
            EraseToolRect => "EraseToolRect",
            EraseToolEllipse => "EraseToolEllipse",
            ErasePaintMode => "ErasePaintMode",
            EraseEraseMode => "EraseEraseMode",
            EraseSpacePan => "EraseSpacePan",
            ConcealExit => "ConcealExit",
            ConcealUndo => "ConcealUndo",
            ConcealDeleteShape => "ConcealDeleteShape",
            ConcealPixelGrid => "ConcealPixelGrid",
            ConcealTypeCycle => "ConcealTypeCycle",
            ConcealPreset1 => "ConcealPreset1",
            ConcealPreset2 => "ConcealPreset2",
            ConcealPreset3 => "ConcealPreset3",
            ConcealPreset4 => "ConcealPreset4",
            ConcealPaintMode => "ConcealPaintMode",
            ConcealEraseMode => "ConcealEraseMode",
            ConcealToolSelect => "ConcealToolSelect",
            ConcealToolBrush => "ConcealToolBrush",
            ConcealToolLasso => "ConcealToolLasso",
            ConcealToolPolygon => "ConcealToolPolygon",
            ConcealToolLine => "ConcealToolLine",
            ConcealToolVLine => "ConcealToolVLine",
            ConcealToolHLine => "ConcealToolHLine",
            ConcealToolRect => "ConcealToolRect",
            ConcealToolEllipse => "ConcealToolEllipse",
            ConcealSpacePan => "ConcealSpacePan",
            CropSpacePan => "CropSpacePan",
            CropExecute => "CropExecute",
            TextConfirm => "TextConfirm",
            TextRedo => "TextRedo",
            TextUndo => "TextUndo",
            TextSpacePan => "TextSpacePan",
            LaShowSource => "LaShowSource",
            LaShowMask => "LaShowMask",
            LaPaintAdd => "LaPaintAdd",
            LaPaintErase => "LaPaintErase",
            LaToolBrush => "LaToolBrush",
            LaToolEdgeBrush => "LaToolEdgeBrush",
            LaToolGapFill => "LaToolGapFill",
            LaToolLasso => "LaToolLasso",
            LaToolPolygon => "LaToolPolygon",
            LaToolSelect => "LaToolSelect",
            LaToolLine => "LaToolLine",
            LaToolVLine => "LaToolVLine",
            LaToolHLine => "LaToolHLine",
            LaToolRect => "LaToolRect",
            LaToolEllipse => "LaToolEllipse",
            LaSpacePan => "LaSpacePan",
        }
    }

    pub fn parse_ini_name(name: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|action| action.ini_name().eq_ignore_ascii_case(name.trim()))
    }

    pub fn context(self) -> KeyContext {
        use KeyAction::*;
        match self {
            GlobalLocalSearch | GlobalFavSearch | GlobalMetadataSearch | GlobalOpenFolder => {
                KeyContext::Global
            }
            GridSelectAll | GridDeselect | GridToggleCheck | GridRotateCw | GridRotateCcw
            | GridPin | GridComparePin | GridColumnCount1 | GridColumnCount2 | GridColumnCount3
            | GridColumnCount4 | GridColumnCount5 | GridColumnCount6 | GridColumnCount7
            | GridColumnCount8 | GridColumnCount9 | GridColumnCount10 | GridAdjustSlot1
            | GridAdjustSlot2 | GridAdjustSlot3 | GridAdjustSlot4 | GridAdjustSlot5
            | GridAdjustSlot6 | GridAdjustSlot7 | GridAdjustSlot8 | GridAdjustSlot9
            | GridAdjustSlot10 | GridClearAdjust => KeyContext::Grid,
            FsToggleMetadata | FsCtrlNavPrev | FsCtrlNavNext | FsSiblingPrev | FsSiblingNext => {
                KeyContext::FsCommon
            }
            RatingItem1 | RatingItem2 | RatingItem3 | RatingItem4 | RatingItem5
            | RatingItemClear | RatingContainer1 | RatingContainer2 | RatingContainer3
            | RatingContainer4 | RatingContainer5 | RatingContainerClear => KeyContext::Rating,
            FsContinuousScrollForward
            | FsContinuousScrollBack
            | FsSpreadShiftLeft
            | FsSpreadShiftRight
            | FsSlideshow
            | FsSpaceCheck
            | FsCapture
            | FsExport
            | FsCompareToggle
            | FsCompareCycle
            | FsCompareWipe
            | FsCompareDiff
            | FsRotateCw
            | FsRotateCcw
            | FsAnalysis
            | FsPanorama
            | FsPixelGrid
            | FsLoupeLockToggle
            | FsLoupeHold
            | FsEraseMode
            | FsConcealMode
            | FsTextMode
            | FsBgCycle
            | FsPin
            | FsSpreadSingle
            | FsSpreadLtr
            | FsSpreadLtrCover
            | FsSpreadRtl
            | FsSpreadRtlCover
            | FsReadingFlowCycle
            | FsReadingDirectionToggle
            | FsFitModeCycle
            | FsAiModelNext
            | FsAiModelPrev
            | FsAiModelReset
            | FsDenoiseCycle
            | FsPostFilterNext
            | FsPostFilterPrev
            | FsPostFilterReset
            | FsAdjustSlot1
            | FsAdjustSlot2
            | FsAdjustSlot3
            | FsAdjustSlot4
            | FsAdjustSlot5
            | FsAdjustSlot6
            | FsAdjustSlot7
            | FsAdjustSlot8
            | FsAdjustSlot9
            | FsAdjustSlot10
            | FsClearAdjust
            | FsApplyErase1
            | FsApplyErase2
            | FsApplyConceal1
            | FsApplyConceal2
            | FsDeleteEraseMask
            | FsDeleteConcealMask => KeyContext::FsImage,
            VideoExternalPlayer | VideoPlayPause | VideoSeekStart | VideoVolumeUp
            | VideoVolumeDown | VideoNextFile | VideoPrevFile | VideoMute | VideoLoop
            | VideoMarkerPrev | VideoMarkerNext | VideoPin | VideoPerfOverlay | VideoTileMode
            | VideoBookmark | VideoCapture | VideoCompareToggle | VideoCompareCycle
            | VideoCompareWipe | VideoCompareDiff => KeyContext::FsVideo,
            EraseConfirm | EraseUndo | EraseDeleteShape | EraseToolSelect | EraseToolBrush
            | EraseToolLasso | EraseToolPolygon | EraseToolVLine | EraseToolHLine
            | EraseToolLine | EraseToolRect | EraseToolEllipse | ErasePaintMode
            | EraseEraseMode | EraseSpacePan => KeyContext::Erase,
            ConcealExit | ConcealUndo | ConcealDeleteShape | ConcealPixelGrid
            | ConcealTypeCycle | ConcealPreset1 | ConcealPreset2 | ConcealPreset3
            | ConcealPreset4 | ConcealPaintMode | ConcealEraseMode | ConcealToolSelect
            | ConcealToolBrush | ConcealToolLasso | ConcealToolPolygon | ConcealToolLine
            | ConcealToolVLine | ConcealToolHLine | ConcealToolRect | ConcealToolEllipse
            | ConcealSpacePan => KeyContext::Conceal,
            CropExecute | CropSpacePan => KeyContext::Crop,
            TextConfirm | TextRedo | TextUndo | TextSpacePan => KeyContext::Text,
            LaShowSource | LaShowMask | LaPaintAdd | LaPaintErase | LaToolBrush
            | LaToolEdgeBrush | LaToolGapFill | LaToolLasso | LaToolPolygon | LaToolSelect
            | LaToolLine | LaToolVLine | LaToolHLine | LaToolRect | LaToolEllipse | LaSpacePan => {
                KeyContext::LocalAdjust
            }
        }
    }

    pub fn trigger(self) -> KeyTrigger {
        use KeyAction::*;
        match self {
            FsLoupeHold => KeyTrigger::ModifierHold,
            EraseSpacePan | ConcealSpacePan | CropSpacePan | TextSpacePan | LaSpacePan => {
                KeyTrigger::KeyHold
            }
            GlobalLocalSearch
            | GlobalFavSearch
            | GlobalMetadataSearch
            | GlobalOpenFolder
            | GridSelectAll
            | GridDeselect
            | GridToggleCheck
            | GridRotateCw
            | GridRotateCcw
            | GridPin
            | GridComparePin
            | GridColumnCount1
            | GridColumnCount2
            | GridColumnCount3
            | GridColumnCount4
            | GridColumnCount5
            | GridColumnCount6
            | GridColumnCount7
            | GridColumnCount8
            | GridColumnCount9
            | GridColumnCount10
            | GridAdjustSlot1
            | GridAdjustSlot2
            | GridAdjustSlot3
            | GridAdjustSlot4
            | GridAdjustSlot5
            | GridAdjustSlot6
            | GridAdjustSlot7
            | GridAdjustSlot8
            | GridAdjustSlot9
            | GridAdjustSlot10
            | GridClearAdjust
            | FsToggleMetadata
            | FsCtrlNavPrev
            | FsCtrlNavNext
            | FsSiblingPrev
            | FsSiblingNext
            | RatingItem1
            | RatingItem2
            | RatingItem3
            | RatingItem4
            | RatingItem5
            | RatingItemClear
            | RatingContainer1
            | RatingContainer2
            | RatingContainer3
            | RatingContainer4
            | RatingContainer5
            | RatingContainerClear
            | FsContinuousScrollForward
            | FsContinuousScrollBack
            | FsSpreadShiftLeft
            | FsSpreadShiftRight
            | FsSlideshow
            | FsSpaceCheck
            | FsCapture
            | FsExport
            | FsCompareToggle
            | FsCompareCycle
            | FsCompareWipe
            | FsCompareDiff
            | FsRotateCw
            | FsRotateCcw
            | FsAnalysis
            | FsPanorama
            | FsPixelGrid
            | FsLoupeLockToggle
            | FsEraseMode
            | FsConcealMode
            | FsTextMode
            | FsBgCycle
            | FsPin
            | FsSpreadSingle
            | FsSpreadLtr
            | FsSpreadLtrCover
            | FsSpreadRtl
            | FsSpreadRtlCover
            | FsReadingFlowCycle
            | FsReadingDirectionToggle
            | FsFitModeCycle
            | FsAiModelNext
            | FsAiModelPrev
            | FsAiModelReset
            | FsDenoiseCycle
            | FsPostFilterNext
            | FsPostFilterPrev
            | FsPostFilterReset
            | FsAdjustSlot1
            | FsAdjustSlot2
            | FsAdjustSlot3
            | FsAdjustSlot4
            | FsAdjustSlot5
            | FsAdjustSlot6
            | FsAdjustSlot7
            | FsAdjustSlot8
            | FsAdjustSlot9
            | FsAdjustSlot10
            | FsClearAdjust
            | FsApplyErase1
            | FsApplyErase2
            | FsApplyConceal1
            | FsApplyConceal2
            | FsDeleteEraseMask
            | FsDeleteConcealMask
            | VideoExternalPlayer
            | VideoPlayPause
            | VideoSeekStart
            | VideoVolumeUp
            | VideoVolumeDown
            | VideoNextFile
            | VideoPrevFile
            | VideoMute
            | VideoLoop
            | VideoMarkerPrev
            | VideoMarkerNext
            | VideoPin
            | VideoPerfOverlay
            | VideoTileMode
            | VideoBookmark
            | VideoCapture
            | VideoCompareToggle
            | VideoCompareCycle
            | VideoCompareWipe
            | VideoCompareDiff
            | EraseConfirm
            | EraseUndo
            | EraseDeleteShape
            | EraseToolSelect
            | EraseToolBrush
            | EraseToolLasso
            | EraseToolPolygon
            | EraseToolVLine
            | EraseToolHLine
            | EraseToolLine
            | EraseToolRect
            | EraseToolEllipse
            | ErasePaintMode
            | EraseEraseMode
            | ConcealExit
            | ConcealUndo
            | ConcealDeleteShape
            | ConcealPixelGrid
            | ConcealTypeCycle
            | ConcealPreset1
            | ConcealPreset2
            | ConcealPreset3
            | ConcealPreset4
            | ConcealPaintMode
            | ConcealEraseMode
            | ConcealToolSelect
            | ConcealToolBrush
            | ConcealToolLasso
            | ConcealToolPolygon
            | ConcealToolLine
            | ConcealToolVLine
            | ConcealToolHLine
            | ConcealToolRect
            | ConcealToolEllipse
            | CropExecute
            | TextConfirm
            | TextRedo
            | TextUndo
            | LaShowSource
            | LaShowMask
            | LaPaintAdd
            | LaPaintErase
            | LaToolBrush
            | LaToolEdgeBrush
            | LaToolGapFill
            | LaToolLasso
            | LaToolPolygon
            | LaToolSelect
            | LaToolLine
            | LaToolVLine
            | LaToolHLine
            | LaToolRect
            | LaToolEllipse => KeyTrigger::Press,
        }
    }

    pub fn default_chords(self) -> ChordList {
        use KeyAction::*;
        use KeyName::*;
        match self {
            GlobalLocalSearch => ChordList::one(Chord::ctrl(F)),
            GlobalFavSearch => ChordList::one(Chord::ctrl(S)),
            GlobalMetadataSearch => ChordList::one(Chord::ctrl(G)),
            GlobalOpenFolder => ChordList::one(Chord::ctrl(O)),
            GridSelectAll => ChordList::one(Chord::ctrl(A)),
            GridDeselect => ChordList::two(Chord::ctrl(D), Chord::ctrl_shift(A)),
            GridToggleCheck => ChordList::one(Chord::key(Space)),
            GridRotateCw => ChordList::one(Chord::key(R)),
            GridRotateCcw => ChordList::one(Chord::key(L)),
            GridPin => ChordList::one(Chord::key(P)),
            GridComparePin => ChordList::one(Chord::key(X)),
            GridColumnCount1 => ChordList::one(Chord::alt(Num1)),
            GridColumnCount2 => ChordList::one(Chord::alt(Num2)),
            GridColumnCount3 => ChordList::one(Chord::alt(Num3)),
            GridColumnCount4 => ChordList::one(Chord::alt(Num4)),
            GridColumnCount5 => ChordList::one(Chord::alt(Num5)),
            GridColumnCount6 => ChordList::one(Chord::alt(Num6)),
            GridColumnCount7 => ChordList::one(Chord::alt(Num7)),
            GridColumnCount8 => ChordList::one(Chord::alt(Num8)),
            GridColumnCount9 => ChordList::one(Chord::alt(Num9)),
            GridColumnCount10 => ChordList::one(Chord::alt(Num0)),
            GridAdjustSlot1 => ChordList::one(Chord::ctrl(Num1)),
            GridAdjustSlot2 => ChordList::one(Chord::ctrl(Num2)),
            GridAdjustSlot3 => ChordList::one(Chord::ctrl(Num3)),
            GridAdjustSlot4 => ChordList::one(Chord::ctrl(Num4)),
            GridAdjustSlot5 => ChordList::one(Chord::ctrl(Num5)),
            GridAdjustSlot6 => ChordList::one(Chord::ctrl(Num6)),
            GridAdjustSlot7 => ChordList::one(Chord::ctrl(Num7)),
            GridAdjustSlot8 => ChordList::one(Chord::ctrl(Num8)),
            GridAdjustSlot9 => ChordList::one(Chord::ctrl(Num9)),
            GridAdjustSlot10 => ChordList::one(Chord::ctrl(Num0)),
            GridClearAdjust => ChordList::two(Chord::ctrl(Backspace), Chord::key(Q)),
            FsToggleMetadata => ChordList::two(Chord::key(I), Chord::key(Tab)),
            FsCtrlNavPrev => ChordList::one(Chord::ctrl(Up)),
            FsCtrlNavNext => ChordList::one(Chord::ctrl(Down)),
            FsSiblingPrev => ChordList::one(Chord::ctrl(PageUp)),
            FsSiblingNext => ChordList::one(Chord::ctrl(PageDown)),
            RatingItem1 => ChordList::one(Chord::key(F1)),
            RatingItem2 => ChordList::one(Chord::key(F2)),
            RatingItem3 => ChordList::one(Chord::key(F3)),
            RatingItem4 => ChordList::one(Chord::key(F4)),
            RatingItem5 => ChordList::one(Chord::key(F5)),
            RatingItemClear => ChordList::one(Chord::key(F6)),
            RatingContainer1 => ChordList::one(Chord::shift(F1)),
            RatingContainer2 => ChordList::one(Chord::shift(F2)),
            RatingContainer3 => ChordList::one(Chord::shift(F3)),
            RatingContainer4 => ChordList::one(Chord::shift(F4)),
            RatingContainer5 => ChordList::one(Chord::shift(F5)),
            RatingContainerClear => ChordList::one(Chord::shift(F6)),
            FsContinuousScrollForward => ChordList::one(Chord::key(PageDown)),
            FsContinuousScrollBack => ChordList::one(Chord::key(PageUp)),
            FsSpreadShiftLeft => ChordList::one(Chord::ctrl(Left)),
            FsSpreadShiftRight => ChordList::one(Chord::ctrl(Right)),
            FsSlideshow => ChordList::one(Chord::key(S)),
            FsSpaceCheck => ChordList::one(Chord::key(Space)),
            FsCapture => ChordList::one(Chord::ctrl(S)),
            FsExport => ChordList::one(Chord::ctrl(E)),
            FsCompareToggle => ChordList::one(Chord::key(X)),
            FsCompareCycle => ChordList::one(Chord::key(C)),
            FsCompareWipe => ChordList::one(Chord::shift(C)),
            FsCompareDiff => ChordList::one(Chord::alt(C)),
            FsRotateCw => ChordList::one(Chord::key(R)),
            FsRotateCcw => ChordList::one(Chord::key(L)),
            FsAnalysis => ChordList::one(Chord::key(Z)),
            FsPanorama => ChordList::one(Chord::key(V)),
            FsPixelGrid => ChordList::one(Chord::key(G)),
            FsLoupeLockToggle => ChordList::one(Chord::key(M)),
            FsLoupeHold => ChordList::one(Chord::modifier(ModKind::Shift)),
            FsEraseMode => ChordList::one(Chord::key(E)),
            FsConcealMode => ChordList::one(Chord::ctrl(M)),
            FsTextMode => ChordList::one(Chord::ctrl(T)),
            FsBgCycle => ChordList::one(Chord::key(B)),
            FsPin => ChordList::one(Chord::key(P)),
            FsSpreadSingle => ChordList::one(Chord::key(Num1)),
            FsSpreadLtr => ChordList::one(Chord::key(Num2)),
            FsSpreadLtrCover => ChordList::one(Chord::key(Num3)),
            FsSpreadRtl => ChordList::one(Chord::key(Num4)),
            FsSpreadRtlCover => ChordList::one(Chord::key(Num5)),
            FsReadingFlowCycle => ChordList::one(Chord::key(Num6)),
            FsReadingDirectionToggle => ChordList::one(Chord::key(Num7)),
            FsFitModeCycle => ChordList::one(Chord::key(Num0)),
            FsAiModelNext => ChordList::one(Chord::key(U)),
            FsAiModelPrev => ChordList::one(Chord::shift(U)),
            FsAiModelReset => ChordList::one(Chord::alt(U)),
            FsDenoiseCycle => ChordList::one(Chord::key(N)),
            FsPostFilterNext => ChordList::one(Chord::key(T)),
            FsPostFilterPrev => ChordList::one(Chord::shift(T)),
            FsPostFilterReset => ChordList::one(Chord::alt(T)),
            FsAdjustSlot1 => ChordList::one(Chord::ctrl(Num1)),
            FsAdjustSlot2 => ChordList::one(Chord::ctrl(Num2)),
            FsAdjustSlot3 => ChordList::one(Chord::ctrl(Num3)),
            FsAdjustSlot4 => ChordList::one(Chord::ctrl(Num4)),
            FsAdjustSlot5 => ChordList::one(Chord::ctrl(Num5)),
            FsAdjustSlot6 => ChordList::one(Chord::ctrl(Num6)),
            FsAdjustSlot7 => ChordList::one(Chord::ctrl(Num7)),
            FsAdjustSlot8 => ChordList::one(Chord::ctrl(Num8)),
            FsAdjustSlot9 => ChordList::one(Chord::ctrl(Num9)),
            FsAdjustSlot10 => ChordList::one(Chord::ctrl(Num0)),
            FsClearAdjust => ChordList::two(Chord::ctrl(Backspace), Chord::key(Q)),
            FsApplyErase1 => ChordList::one(Chord::key(F7)),
            FsApplyErase2 => ChordList::one(Chord::key(F8)),
            FsApplyConceal1 => ChordList::one(Chord::key(F9)),
            FsApplyConceal2 => ChordList::one(Chord::key(F10)),
            FsDeleteEraseMask => ChordList::two(Chord::shift(F7), Chord::shift(F8)),
            FsDeleteConcealMask => ChordList::two(Chord::shift(F9), Chord::shift(F10)),
            VideoExternalPlayer => ChordList::one(Chord::shift(Enter)),
            VideoPlayPause => ChordList::two(Chord::key(Space), Chord::key(Enter)),
            VideoSeekStart => ChordList::one(Chord::key(W)),
            VideoVolumeUp => ChordList::one(Chord::shift(Up)),
            VideoVolumeDown => ChordList::one(Chord::shift(Down)),
            VideoNextFile => ChordList::one(Chord::key(Down)),
            VideoPrevFile => ChordList::one(Chord::key(Up)),
            VideoMute => ChordList::one(Chord::key(M)),
            VideoLoop => ChordList::one(Chord::key(L)),
            VideoMarkerPrev => ChordList::one(Chord::key(J)),
            VideoMarkerNext => ChordList::one(Chord::key(K)),
            VideoPin => ChordList::one(Chord::key(P)),
            VideoPerfOverlay => ChordList::one(Chord::key(F)),
            VideoTileMode => ChordList::one(Chord::key(S)),
            VideoBookmark => ChordList::one(Chord::key(B)),
            VideoCapture => ChordList::one(Chord::ctrl(S)),
            VideoCompareToggle => ChordList::one(Chord::key(X)),
            VideoCompareCycle => ChordList::one(Chord::key(C)),
            VideoCompareWipe => ChordList::one(Chord::shift(C)),
            VideoCompareDiff => ChordList::one(Chord::alt(C)),
            EraseConfirm => ChordList::one(Chord::key(E)),
            EraseUndo => ChordList::one(Chord::ctrl(Z)),
            EraseDeleteShape => ChordList::one(Chord::key(Delete)),
            EraseToolSelect => ChordList::one(Chord::key(S)),
            EraseToolBrush => ChordList::one(Chord::key(B)),
            EraseToolLasso => ChordList::one(Chord::key(L)),
            EraseToolPolygon => ChordList::one(Chord::key(P)),
            EraseToolVLine => ChordList::one(Chord::key(V)),
            EraseToolHLine => ChordList::one(Chord::key(H)),
            EraseToolLine => ChordList::one(Chord::key(I)),
            EraseToolRect => ChordList::one(Chord::key(R)),
            EraseToolEllipse => ChordList::one(Chord::key(O)),
            ErasePaintMode => ChordList::one(Chord::key(D)),
            EraseEraseMode => ChordList::one(Chord::key(F)),
            EraseSpacePan => ChordList::one(Chord::key(Space)),
            ConcealExit => ChordList::one(Chord::ctrl(M)),
            ConcealUndo => ChordList::one(Chord::ctrl(Z)),
            ConcealDeleteShape => ChordList::one(Chord::key(Delete)),
            ConcealPixelGrid => ChordList::one(Chord::key(G)),
            ConcealTypeCycle => ChordList::one(Chord::key(T)),
            ConcealPreset1 => ChordList::one(Chord::key(Num1)),
            ConcealPreset2 => ChordList::one(Chord::key(Num2)),
            ConcealPreset3 => ChordList::one(Chord::key(Num3)),
            ConcealPreset4 => ChordList::one(Chord::key(Num4)),
            ConcealPaintMode => ChordList::one(Chord::key(D)),
            ConcealEraseMode => ChordList::one(Chord::key(F)),
            ConcealToolSelect => ChordList::one(Chord::key(S)),
            ConcealToolBrush => ChordList::one(Chord::key(B)),
            ConcealToolLasso => ChordList::one(Chord::key(L)),
            ConcealToolPolygon => ChordList::one(Chord::key(P)),
            ConcealToolLine => ChordList::one(Chord::key(I)),
            ConcealToolVLine => ChordList::one(Chord::key(V)),
            ConcealToolHLine => ChordList::one(Chord::key(H)),
            ConcealToolRect => ChordList::one(Chord::key(R)),
            ConcealToolEllipse => ChordList::one(Chord::key(O)),
            ConcealSpacePan => ChordList::one(Chord::key(Space)),
            CropSpacePan => ChordList::one(Chord::key(Space)),
            CropExecute => ChordList::one(Chord::ctrl(E)),
            TextConfirm => ChordList::one(Chord::ctrl(T)),
            TextRedo => ChordList::two(Chord::ctrl(Y), Chord::ctrl_shift(Z)),
            TextUndo => ChordList::one(Chord::ctrl(Z)),
            TextSpacePan => ChordList::one(Chord::key(Space)),
            LaShowSource => ChordList::one(Chord::key(Q)),
            LaShowMask => ChordList::one(Chord::key(W)),
            LaPaintAdd => ChordList::one(Chord::key(D)),
            LaPaintErase => ChordList::one(Chord::key(F)),
            LaToolBrush => ChordList::one(Chord::key(B)),
            LaToolEdgeBrush => ChordList::one(Chord::key(A)),
            LaToolGapFill => ChordList::one(Chord::key(G)),
            LaToolLasso => ChordList::one(Chord::key(L)),
            LaToolPolygon => ChordList::one(Chord::key(P)),
            LaToolSelect => ChordList::one(Chord::key(S)),
            LaToolLine => ChordList::one(Chord::key(I)),
            LaToolVLine => ChordList::one(Chord::key(V)),
            LaToolHLine => ChordList::one(Chord::key(H)),
            LaToolRect => ChordList::one(Chord::key(R)),
            LaToolEllipse => ChordList::one(Chord::key(O)),
            LaSpacePan => ChordList::one(Chord::key(Space)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    overrides: HashMap<KeyAction, Vec<Chord>>,
    warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IniTemplateKind {
    UserConfig,
    DefaultReference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RatingKey {
    pub container: bool,
    pub stars: u8,
}

impl Keymap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load_from_file(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_ini_str(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::empty(),
            Err(err) => {
                let mut keymap = Self::empty();
                keymap.warnings.push(format!(
                    "failed to read keymap.ini ({}): {}",
                    path.display(),
                    err
                ));
                keymap
            }
        }
    }

    pub fn write_user_ini_if_missing(path: &Path) -> std::io::Result<bool> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(Self::user_ini_template().as_bytes()) {
                    let _ = std::fs::remove_file(path);
                    return Err(err);
                }
                Ok(true)
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(err),
        }
    }

    pub fn write_default_reference_ini(path: &Path) -> std::io::Result<bool> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        let text = Self::default_reference_ini();
        if std::fs::read_to_string(path).is_ok_and(|existing| existing == text) {
            return Ok(false);
        }
        std::fs::write(path, text)?;
        Ok(true)
    }

    pub fn from_ini_str(text: &str) -> Self {
        let mut warnings = Vec::new();
        let mut current_section: Option<KeyContext> = None;
        let mut grouped: HashMap<KeyAction, ParsedAction> = HashMap::new();

        for (line_idx, raw_line) in text.lines().enumerate() {
            let line_no = line_idx + 1;
            let line = strip_inline_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let section_name = section.trim();
                current_section = KeyContext::parse(section_name);
                if current_section.is_none() {
                    warnings.push(format!(
                        "line {line_no}: unknown keymap section [{section_name}]"
                    ));
                }
                continue;
            }

            let Some((lhs, rhs)) = line.split_once('=') else {
                warnings.push(format!("line {line_no}: expected Action = Key"));
                continue;
            };
            let lhs = lhs.trim();
            let rhs = rhs.trim();
            let (action_name, slot) = match parse_action_lhs(lhs) {
                Ok(v) => v,
                Err(msg) => {
                    warnings.push(format!("line {line_no}: {msg}"));
                    continue;
                }
            };
            let Some(action) = KeyAction::parse_ini_name(action_name) else {
                warnings.push(format!(
                    "line {line_no}: unknown key action '{action_name}'"
                ));
                continue;
            };
            if let Some(section) = current_section
                && section != action.context()
            {
                warnings.push(format!(
                    "line {line_no}: action '{}' belongs to [{}], not [{}]",
                    action.ini_name(),
                    action.context().ini_name(),
                    section.ini_name()
                ));
            }

            let parsed = grouped.entry(action).or_default();
            if rhs.is_empty() || rhs.eq_ignore_ascii_case("none") {
                parsed.disabled = true;
                continue;
            }
            match parse_chord(rhs) {
                Ok(chord) => {
                    if let Err(msg) = chord.validate_for_trigger(action.trigger()) {
                        warnings.push(format!(
                            "line {line_no}: '{}' ignored: {msg}",
                            action.ini_name()
                        ));
                        continue;
                    }
                    if parsed.chords.insert(slot, chord).is_some() {
                        warnings.push(format!(
                            "line {line_no}: duplicate slot .{slot} for '{}', using the last value",
                            action.ini_name()
                        ));
                    }
                }
                Err(msg) => warnings.push(format!("line {line_no}: {msg}")),
            }
        }

        let mut overrides = HashMap::new();
        for (action, parsed) in grouped {
            if parsed.disabled {
                if !parsed.chords.is_empty() {
                    warnings.push(format!(
                        "'{}' mixes 'none' with chord slots; disabling the action",
                        action.ini_name()
                    ));
                }
                overrides.insert(action, Vec::new());
                continue;
            }
            if parsed.chords.is_empty() {
                continue;
            }
            let chords: Vec<Chord> = parsed.chords.into_values().take(3).collect();
            overrides.insert(action, chords);
        }

        Self {
            overrides,
            warnings,
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn consume_rating_action(&self, ctx: &egui::Context, container: bool) -> Option<u8> {
        rating_actions(container)
            .iter()
            .copied()
            .find_map(|(action, stars)| self.consume_action(ctx, action).then_some(stars))
    }

    #[cfg(windows)]
    pub fn native_video_rating_action(
        &self,
        key: &crate::video::native_window::NativeVideoKeyEvent,
    ) -> Option<RatingKey> {
        if key.repeat {
            return None;
        }
        for container in [true, false] {
            for (action, stars) in rating_actions(container).iter().copied() {
                if self.matches_vk_action(action, key) {
                    return Some(RatingKey { container, stars });
                }
            }
        }
        None
    }

    pub fn consume_action(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            for chord in chords.iter().copied() {
                if self.consume_chord(ctx, chord) {
                    return true;
                }
            }
            return false;
        }
        for chord in action.default_chords().iter() {
            if self.consume_chord(ctx, chord) {
                return true;
            }
        }
        false
    }

    pub fn pressed_action(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            return chords
                .iter()
                .copied()
                .any(|chord| self.pressed_chord(ctx, chord));
        }
        action
            .default_chords()
            .iter()
            .any(|chord| self.pressed_chord(ctx, chord))
    }

    pub fn key_held_action(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::KeyHold);
        if let Some(chords) = self.overrides.get(&action) {
            return chords
                .iter()
                .copied()
                .any(|chord| self.key_held_chord(ctx, chord));
        }
        action
            .default_chords()
            .iter()
            .any(|chord| self.key_held_chord(ctx, chord))
    }

    pub fn modifier_held_action(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::ModifierHold);
        if let Some(chords) = self.overrides.get(&action) {
            return chords
                .iter()
                .copied()
                .any(|chord| self.modifier_held_chord(ctx, chord));
        }
        action
            .default_chords()
            .iter()
            .any(|chord| self.modifier_held_chord(ctx, chord))
    }

    pub fn matches_vk_action_parts(
        &self,
        action: KeyAction,
        virtual_key: u32,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            return chords
                .iter()
                .copied()
                .any(|chord| chord.matches_vk_parts(virtual_key, ctrl, shift, alt));
        }
        action
            .default_chords()
            .iter()
            .any(|chord| chord.matches_vk_parts(virtual_key, ctrl, shift, alt))
    }

    #[cfg(windows)]
    pub fn matches_vk_action(
        &self,
        action: KeyAction,
        key: &crate::video::native_window::NativeVideoKeyEvent,
    ) -> bool {
        self.matches_vk_action_parts(action, key.virtual_key, key.ctrl, key.shift, key.alt)
    }

    pub fn install_global_native_video_shortcuts(&self) {
        let mut chords = Vec::new();
        for action in KeyAction::all()
            .iter()
            .copied()
            .filter(|action| matches!(action.context(), KeyContext::FsVideo | KeyContext::Rating))
        {
            if let Some(override_chords) = self.overrides.get(&action) {
                chords.extend(override_chords.iter().copied());
            } else {
                chords.extend(action.default_chords().iter());
            }
        }
        let cell = GLOBAL_NATIVE_VIDEO_CHORDS.get_or_init(|| RwLock::new(Vec::new()));
        if let Ok(mut guard) = cell.write() {
            *guard = chords;
        }
    }

    pub fn user_ini_template() -> String {
        Self::ini_template(IniTemplateKind::UserConfig)
    }

    pub fn default_reference_ini() -> String {
        Self::ini_template(IniTemplateKind::DefaultReference)
    }

    fn ini_template(kind: IniTemplateKind) -> String {
        let mut out = String::new();
        match kind {
            IniTemplateKind::UserConfig => out.push_str("# mimageviewer keymap.ini\n"),
            IniTemplateKind::DefaultReference => {
                out.push_str("# mimageviewer keymap.ini.default\n")
            }
        }
        out.push_str("#\n");
        out.push_str("# Advanced users only.\n");
        match kind {
            IniTemplateKind::UserConfig => {
                out.push_str(
                    "# Generated at %APPDATA%\\mimageviewer\\keymap.ini when this file is missing.\n",
                );
                out.push_str(
                    "# Uncomment only the actions you want to override, then edit the values after '='.\n",
                );
                out.push_str(
                    "# Commented lines follow the current built-in defaults, including future updates.\n",
                );
                out.push_str(
                    "# See keymap.ini.default for the latest full reference after an update.\n",
                );
            }
            IniTemplateKind::DefaultReference => {
                out.push_str("# Reference only. Do not edit this file.\n");
                out.push_str(
                    "# This file is overwritten by mimageviewer when built-in defaults change.\n",
                );
                out.push_str(
                    "# Copy lines to keymap.ini and uncomment them if you want to override defaults.\n",
                );
            }
        }
        out.push_str("# The file is loaded once at startup.\n");
        out.push_str("#\n");
        out.push_str("# Rules:\n");
        out.push_str("# - All key definition lines below are commented out by default.\n");
        out.push_str("# - Leaving a line commented keeps the built-in default.\n");
        out.push_str(
            "# - Uncommenting one line for an action replaces all defaults for that action.\n",
        );
        out.push_str("# - Use Action.1, Action.2, Action.3 to bind up to three chords.\n");
        out.push_str("# - Use \"none\" to disable an action.\n");
        out.push_str("# - Conflicts are not detected; the first matching handler wins.\n");
        out.push_str("# - Press actions can use Ctrl/Shift/Alt plus one normal key.\n");
        out.push_str("# - ModifierHold actions accept Ctrl, Shift, or Alt only.\n");
        out.push_str("# - KeyHold actions accept one normal key only.\n");
        out.push_str("# - Numpad number keys are treated the same as the top-row number keys.\n");
        out.push_str("#   Names such as Numpad1 are accepted only as aliases for 1.\n");
        out.push_str("# - OS-reserved shortcuts such as Alt+F4, Alt+Tab, Alt+Esc, Alt+Space,\n");
        out.push_str("#   Ctrl+Alt+Del, and Windows-key combinations cannot be overridden here.\n");
        out.push_str("# - Alt combinations are not forwarded from the native video presenter.\n");
        out.push_str(
            "# - Mouse, gamepad, drag-and-drop, OS clipboard shortcuts, IME confirmation,\n",
        );
        out.push_str("#   right-click menus, Escape/Enter navigation, and most arrow\n");
        out.push_str("#   navigation are intentionally fixed.\n");
        out.push_str("#\n");
        out.push_str("# Examples:\n");
        out.push_str("# [FsImage]\n");
        out.push_str("# FsSlideshow.1 = P\n");
        out.push_str("# FsSlideshow.2 = S\n");
        out.push_str("# FsCapture = none\n");
        out.push_str("# FsLoupeLockToggle = L\n");
        out.push_str("# FsLoupeHold = Ctrl\n");
        out.push_str("#\n");
        out.push_str("# [Text]\n");
        out.push_str("# TextRedo.1 = Ctrl+Y\n");
        out.push_str("# TextRedo.2 = Ctrl+Shift+Z\n\n");
        let sections = [
            KeyContext::Global,
            KeyContext::Grid,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsImage,
            KeyContext::FsVideo,
            KeyContext::Erase,
            KeyContext::Conceal,
            KeyContext::Crop,
            KeyContext::Text,
            KeyContext::LocalAdjust,
        ];
        for (section_idx, section) in sections.iter().copied().enumerate() {
            out.push_str(&format!("[{}]\n", section.ini_name()));
            for action in KeyAction::all()
                .iter()
                .copied()
                .filter(|action| action.context() == section)
            {
                let defaults: Vec<String> = action
                    .default_chords()
                    .iter()
                    .map(Chord::display_name)
                    .collect();
                if defaults.len() <= 1 {
                    let default = defaults
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "none".to_string());
                    out.push_str(&format!("# {} = {default}\n", action.ini_name()));
                } else {
                    for (idx, default) in defaults.iter().enumerate() {
                        out.push_str(&format!(
                            "# {}.{} = {default}\n",
                            action.ini_name(),
                            idx + 1
                        ));
                    }
                }
            }
            if section_idx + 1 < sections.len() {
                out.push('\n');
            }
        }
        out
    }

    fn consume_chord(&self, ctx: &egui::Context, chord: Chord) -> bool {
        if chord.key.is_none() {
            return false;
        }
        ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|event| {
                let consume = !found
                    && matches!(
                        event,
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } if chord.matches_egui(*key, *modifiers)
                    );
                if consume {
                    found = true;
                }
                !consume
            });
            found
        })
    }

    fn pressed_chord(&self, ctx: &egui::Context, chord: Chord) -> bool {
        if chord.key.is_none() {
            return false;
        }
        ctx.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if chord.matches_egui(*key, *modifiers)
                )
            })
        })
    }

    fn key_held_chord(&self, ctx: &egui::Context, chord: Chord) -> bool {
        if chord.ctrl || chord.shift || chord.alt {
            return false;
        }
        let Some(key) = chord.key.map(KeyName::to_egui) else {
            return false;
        };
        #[cfg(windows)]
        if let Some(name) = chord.key {
            if key_held_via_os(name) {
                return true;
            }
        }
        ctx.input(|i| i.key_down(key))
    }

    fn modifier_held_chord(&self, ctx: &egui::Context, chord: Chord) -> bool {
        #[cfg(windows)]
        if chord.key.is_none() {
            if chord.ctrl {
                return modifier_held_via_os(ModKind::Ctrl);
            }
            if chord.shift {
                return modifier_held_via_os(ModKind::Shift);
            }
            if chord.alt {
                return modifier_held_via_os(ModKind::Alt);
            }
        }
        ctx.input(|i| chord.matches_modifiers(i.modifiers))
    }
}

#[cfg(windows)]
fn key_held_via_os(key: KeyName) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    unsafe { GetAsyncKeyState(key.to_vk() as i32) < 0 }
}

#[cfg(windows)]
fn modifier_held_via_os(kind: ModKind) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    let vk = match kind {
        ModKind::Ctrl => VK_CONTROL.0,
        ModKind::Shift => VK_SHIFT.0,
        ModKind::Alt => VK_MENU.0,
    };
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

#[cfg(windows)]
pub fn native_video_fullscreen_shortcut_key(
    key: &crate::video::native_window::NativeVideoKeyEvent,
) -> bool {
    if key.alt {
        return false;
    }
    if native_video_fixed_shortcut_key(key.virtual_key, key.ctrl, key.shift) {
        return true;
    }
    if let Some(cell) = GLOBAL_NATIVE_VIDEO_CHORDS.get()
        && let Ok(guard) = cell.read()
    {
        return guard
            .iter()
            .copied()
            .any(|chord| chord.matches_vk_parts(key.virtual_key, key.ctrl, key.shift, key.alt));
    }
    let fallback = Keymap::empty();
    KeyAction::all()
        .iter()
        .copied()
        .filter(|action| matches!(action.context(), KeyContext::FsVideo | KeyContext::Rating))
        .any(|action| fallback.matches_vk_action(action, key))
}

fn native_video_fixed_shortcut_key(virtual_key: u32, ctrl: bool, shift: bool) -> bool {
    if matches!(virtual_key, 0x21 | 0x22) {
        return ctrl && !shift;
    }
    matches!(
        virtual_key,
        0x08 // Backspace
            | 0x1B // Escape
            | 0x23 // End
            | 0x24 // Home
            | 0x25 // Left
            | 0x26 // Up
            | 0x27 // Right
            | 0x28 // Down
            | 0x7A // F11
            | 0xA6 // Browser back
            | 0xA7 // Browser forward
    )
}

static GLOBAL_NATIVE_VIDEO_CHORDS: OnceLock<RwLock<Vec<Chord>>> = OnceLock::new();

#[derive(Default)]
struct ParsedAction {
    disabled: bool,
    chords: BTreeMap<usize, Chord>,
}

fn strip_inline_comment(line: &str) -> &str {
    let mut cut = line.len();
    for marker in [';', '#'] {
        if let Some(pos) = line.find(marker) {
            cut = cut.min(pos);
        }
    }
    &line[..cut]
}

fn parse_action_lhs(lhs: &str) -> Result<(&str, usize), String> {
    let trimmed = lhs.trim();
    if trimmed.is_empty() {
        return Err("empty action name".to_string());
    }
    if let Some((name, suffix)) = trimmed.rsplit_once('.') {
        if suffix.chars().all(|ch| ch.is_ascii_digit()) {
            let slot: usize = suffix
                .parse()
                .map_err(|_| format!("invalid chord slot '.{suffix}'"))?;
            if !(1..=3).contains(&slot) {
                return Err(format!("chord slot for '{name}' must be 1..3"));
            }
            return Ok((name.trim(), slot));
        }
    }
    Ok((trimmed, 1))
}

fn parse_chord(rhs: &str) -> Result<Chord, String> {
    let mut chord = Chord::NONE;
    let mut key_seen = false;
    for token in rhs.split('+').map(str::trim).filter(|s| !s.is_empty()) {
        if token.eq_ignore_ascii_case("ctrl") || token.eq_ignore_ascii_case("control") {
            chord.ctrl = true;
        } else if token.eq_ignore_ascii_case("shift") {
            chord.shift = true;
        } else if token.eq_ignore_ascii_case("alt") {
            chord.alt = true;
        } else {
            if key_seen {
                return Err(format!("'{rhs}' has more than one normal key"));
            }
            let key = KeyName::parse(token).ok_or_else(|| format!("unknown key name '{token}'"))?;
            chord.key = Some(key);
            key_seen = true;
        }
    }
    if !key_seen && !chord.ctrl && !chord.shift && !chord.alt {
        return Err("empty key chord".to_string());
    }
    Ok(chord)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn begin_key_pass(ctx: &egui::Context, key: egui::Key, modifiers: egui::Modifiers) {
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![key_event(key, modifiers)],
            ..Default::default()
        });
    }

    fn key_action_enum_names_from_source() -> std::collections::BTreeSet<String> {
        let source = include_str!("keymap.rs");
        let start = source
            .find("pub enum KeyAction {")
            .expect("KeyAction enum not found");
        let body = &source[start + "pub enum KeyAction {".len()..];
        let end = body.find("\n}").expect("KeyAction enum end not found");
        body[..end]
            .lines()
            .filter_map(|line| {
                let name = line.trim().trim_end_matches(',');
                (!name.is_empty() && !name.starts_with("//")).then(|| name.to_string())
            })
            .collect()
    }

    fn all_actions_names_from_source() -> std::collections::BTreeSet<String> {
        let source = include_str!("keymap.rs");
        let start = source
            .find("const ALL_ACTIONS: &[KeyAction]")
            .expect("ALL_ACTIONS not found");
        let body = &source[start..];
        let start = body.find("&[").expect("ALL_ACTIONS body not found") + 2;
        let body = &body[start..];
        let end = body.find("\n];").expect("ALL_ACTIONS end not found");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix("KeyAction::")
                    .map(|name| name.trim_end_matches(',').to_string())
            })
            .collect()
    }

    #[test]
    fn all_actions_inventory_matches_key_action_enum() {
        assert_eq!(
            key_action_enum_names_from_source(),
            all_actions_names_from_source()
        );
    }

    #[test]
    fn all_actions_have_unique_names_and_parse_back() {
        let mut names = std::collections::BTreeSet::new();
        for action in KeyAction::all().iter().copied() {
            assert!(
                names.insert(action.ini_name()),
                "duplicate key action ini name: {}",
                action.ini_name()
            );
            assert_eq!(KeyAction::parse_ini_name(action.ini_name()), Some(action));
            assert!(
                !action.default_chords().is_empty(),
                "{} must have an explicit default chord or be removed from ALL_ACTIONS",
                action.ini_name()
            );
        }
    }

    #[test]
    fn parses_multiple_slots_and_replaces_defaults() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsSlideshow.1 = P
            FsSlideshow.2 = Ctrl+S
            "#,
        );
        assert!(keymap.warnings().is_empty());
        let chords = keymap.overrides.get(&KeyAction::FsSlideshow).unwrap();
        assert_eq!(chords.len(), 2);
        assert_eq!(chords[0], Chord::key(KeyName::P));
        assert_eq!(chords[1], Chord::ctrl(KeyName::S));
    }

    #[test]
    fn action_names_ending_in_digits_do_not_conflict_with_slots() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsAdjustSlot1 = Ctrl+F1
            "#,
        );
        assert!(keymap.warnings().is_empty());
        assert_eq!(
            keymap.overrides.get(&KeyAction::FsAdjustSlot1).unwrap()[0],
            Chord::ctrl(KeyName::F1)
        );
    }

    #[test]
    fn invalid_trigger_pattern_is_ignored() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsLoupeHold = M
            "#,
        );
        assert!(!keymap.warnings().is_empty());
        assert!(!keymap.overrides.contains_key(&KeyAction::FsLoupeHold));
    }

    #[test]
    fn exact_consume_does_not_match_shifted_plain_key() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsSlideshow = S
            "#,
        );
        let ctx = egui::Context::default();
        begin_key_pass(&ctx, egui::Key::S, egui::Modifiers::SHIFT);
        assert!(!keymap.consume_action(&ctx, KeyAction::FsSlideshow));
        let remaining = ctx.input(|i| i.events.len());
        assert_eq!(remaining, 1);
        let _ = ctx.end_pass();
    }

    #[test]
    fn rating_actions_are_exact_and_customizable() {
        let keymap = Keymap::empty();
        let ctx = egui::Context::default();

        begin_key_pass(&ctx, egui::Key::F2, egui::Modifiers::SHIFT);
        assert_eq!(keymap.consume_rating_action(&ctx, false), None);
        assert_eq!(keymap.consume_rating_action(&ctx, true), Some(2));
        assert_eq!(ctx.input(|i| i.events.len()), 0);
        let _ = ctx.end_pass();

        let keymap = Keymap::from_ini_str(
            r#"
            [Rating]
            RatingItem1 = Ctrl+F1
            "#,
        );
        assert!(keymap.warnings().is_empty());
        begin_key_pass(&ctx, egui::Key::F1, egui::Modifiers::NONE);
        assert_eq!(keymap.consume_rating_action(&ctx, false), None);
        assert_eq!(ctx.input(|i| i.events.len()), 1);
        let _ = ctx.end_pass();

        begin_key_pass(&ctx, egui::Key::F1, egui::Modifiers::CTRL);
        assert_eq!(keymap.consume_rating_action(&ctx, false), Some(1));
        assert_eq!(ctx.input(|i| i.events.len()), 0);
        let _ = ctx.end_pass();
    }

    #[test]
    fn numpad_names_parse_as_number_aliases() {
        assert_eq!(KeyName::parse("Numpad1"), Some(KeyName::Num1));
        assert_eq!(KeyName::parse("Numpad0"), Some(KeyName::Num0));
    }

    #[test]
    fn none_disables_action() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsCapture = none
            "#,
        );
        assert_eq!(
            keymap.overrides.get(&KeyAction::FsCapture).unwrap().len(),
            0
        );
    }

    #[cfg(windows)]
    #[test]
    fn vk_match_uses_overridden_chord() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsVideo]
            VideoMute = Q
            "#,
        );
        let event = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0x51,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(keymap.matches_vk_action(KeyAction::VideoMute, &event));
    }

    #[test]
    fn user_ini_template_is_parseable_and_comment_only() {
        let user_ini = Keymap::user_ini_template();
        assert!(user_ini.contains("[FsImage]"));
        assert!(user_ini.contains("[Rating]"));
        assert!(user_ini.contains("# RatingItem1 = F1"));
        assert!(user_ini.contains("# FsSlideshow = S"));
        assert!(user_ini.contains("# TextRedo.1 = Ctrl+Y"));
        assert!(!user_ini.contains("\nFsSlideshow = S"));

        let keymap = Keymap::from_ini_str(&user_ini);
        assert!(
            keymap.warnings().is_empty(),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );
        assert!(keymap.overrides.is_empty());
    }

    #[test]
    fn default_reference_ini_is_parseable_and_comment_only() {
        let default_ini = Keymap::default_reference_ini();
        assert!(default_ini.contains("keymap.ini.default"));
        assert!(default_ini.contains("[Rating]"));
        assert!(default_ini.contains("# RatingContainerClear = Shift+F6"));
        assert!(default_ini.contains("# FsSlideshow = S"));
        assert!(default_ini.contains("# TextRedo.1 = Ctrl+Y"));
        assert!(!default_ini.contains("\nFsSlideshow = S"));

        let keymap = Keymap::from_ini_str(&default_ini);
        assert!(
            keymap.warnings().is_empty(),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );
        assert!(keymap.overrides.is_empty());
    }

    #[test]
    fn write_user_ini_if_missing_does_not_overwrite_existing_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("miv-keymap-test-{}-{stamp}", std::process::id()));
        let path = dir.join("mimageviewer").join("keymap.ini");

        assert!(Keymap::write_user_ini_if_missing(&path).unwrap());
        let generated = std::fs::read_to_string(&path).unwrap();
        assert!(generated.contains("# GlobalLocalSearch = Ctrl+F"));

        std::fs::write(&path, "sentinel").unwrap();
        assert!(!Keymap::write_user_ini_if_missing(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sentinel");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_default_reference_ini_overwrites_stale_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("miv-keymap-test-{}-{stamp}", std::process::id()));
        let path = dir.join("mimageviewer").join("keymap.ini.default");

        assert!(Keymap::write_default_reference_ini(&path).unwrap());
        let generated = std::fs::read_to_string(&path).unwrap();
        assert!(generated.contains("keymap.ini.default"));
        assert!(generated.contains("# GlobalLocalSearch = Ctrl+F"));

        std::fs::write(&path, "stale").unwrap();
        assert!(Keymap::write_default_reference_ini(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            Keymap::default_reference_ini()
        );
        assert!(!Keymap::write_default_reference_ini(&path).unwrap());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bundled_keymap_default_matches_generated_reference() {
        let bundled = include_str!("../docs/keymap.ini.default").replace("\r\n", "\n");
        assert_eq!(bundled, Keymap::default_reference_ini());
    }
}
