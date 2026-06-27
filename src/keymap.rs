use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
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

    pub fn description(self) -> &'static str {
        match self {
            KeyContext::Global => "全体共通",
            KeyContext::Grid => "サムネイル一覧",
            KeyContext::FsCommon => "フルスクリーン共通",
            KeyContext::Rating => "レーティング",
            KeyContext::FsImage => "画像フルスクリーン",
            KeyContext::FsVideo => "動画フルスクリーン",
            KeyContext::Erase => "消しゴムモード",
            KeyContext::Conceal => "隠蔽加工モード",
            KeyContext::Crop => "切り取りモード",
            KeyContext::Text => "テキスト注釈モード",
            KeyContext::LocalAdjust => "補正レイヤー",
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

pub type CommandScope = KeyContext;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyTrigger {
    Press,
    ModifierHold,
    KeyHold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BindingPolicy {
    FullChord,
    SingleModifier,
    SinglePlainKey,
    Reserved,
    NotBindable,
}

impl BindingPolicy {
    fn for_trigger(trigger: KeyTrigger) -> Self {
        match trigger {
            KeyTrigger::Press => BindingPolicy::FullChord,
            KeyTrigger::ModifierHold => BindingPolicy::SingleModifier,
            KeyTrigger::KeyHold => BindingPolicy::SinglePlainKey,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModKind {
    Ctrl,
    Shift,
    Alt,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeySlot {
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
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
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
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
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
    Semicolon,
    Colon,
    Comma,
    Period,
    Backslash,
    Slash,
    Minus,
    JisCaret,
    JisAt,
    IntlYen,
    IntlRo,
}

pub type KeyName = KeySlot;

impl KeySlot {
    const JIS_CARET_SCAN: u16 = 0x0d;
    const JIS_AT_SCAN: u16 = 0x1a;
    const INTL_YEN_SCAN: u16 = 0x7d;
    const INTL_RO_SCAN: u16 = 0x73;

    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if matches!(trimmed, "-" | "−") {
            return Some(KeyName::Minus);
        }
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
            "0" | "NUM0" | "DIGIT0" => KeyName::Num0,
            "1" | "NUM1" | "DIGIT1" => KeyName::Num1,
            "2" | "NUM2" | "DIGIT2" => KeyName::Num2,
            "3" | "NUM3" | "DIGIT3" => KeyName::Num3,
            "4" | "NUM4" | "DIGIT4" => KeyName::Num4,
            "5" | "NUM5" | "DIGIT5" => KeyName::Num5,
            "6" | "NUM6" | "DIGIT6" => KeyName::Num6,
            "7" | "NUM7" | "DIGIT7" => KeyName::Num7,
            "8" | "NUM8" | "DIGIT8" => KeyName::Num8,
            "9" | "NUM9" | "DIGIT9" => KeyName::Num9,
            "NUMPAD0" | "NP0" => KeyName::Numpad0,
            "NUMPAD1" | "NP1" => KeyName::Numpad1,
            "NUMPAD2" | "NP2" => KeyName::Numpad2,
            "NUMPAD3" | "NP3" => KeyName::Numpad3,
            "NUMPAD4" | "NP4" => KeyName::Numpad4,
            "NUMPAD5" | "NP5" => KeyName::Numpad5,
            "NUMPAD6" | "NP6" => KeyName::Numpad6,
            "NUMPAD7" | "NP7" => KeyName::Numpad7,
            "NUMPAD8" | "NP8" => KeyName::Numpad8,
            "NUMPAD9" | "NP9" => KeyName::Numpad9,
            "NUMPADADD" | "NUMADD" | "NPADD" | "NUMPADPLUS" | "NPPLUS" => KeyName::NumpadAdd,
            "NUMPADSUBTRACT" | "NUMSUBTRACT" | "NPSUBTRACT" | "NUMPADMINUS" | "NPMINUS" => {
                KeyName::NumpadSubtract
            }
            "NUMPADMULTIPLY" | "NUMMULTIPLY" | "NPMULTIPLY" | "NUMPADASTERISK" | "NPASTERISK" => {
                KeyName::NumpadMultiply
            }
            "NUMPADDIVIDE" | "NUMDIVIDE" | "NPDIVIDE" => KeyName::NumpadDivide,
            "NUMPADDECIMAL" | "NUMDECIMAL" | "NPDECIMAL" | "NUMPADDOT" | "NPDOT" => {
                KeyName::NumpadDecimal
            }
            "NUMPADENTER" | "NUMENTER" | "NPENTER" => KeyName::NumpadEnter,
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
            "F13" => KeyName::F13,
            "F14" => KeyName::F14,
            "F15" => KeyName::F15,
            "F16" => KeyName::F16,
            "F17" => KeyName::F17,
            "F18" => KeyName::F18,
            "F19" => KeyName::F19,
            "F20" => KeyName::F20,
            "F21" => KeyName::F21,
            "F22" => KeyName::F22,
            "F23" => KeyName::F23,
            "F24" => KeyName::F24,
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
            ";" | "SEMICOLON" => KeyName::Semicolon,
            ":" | "COLON" => KeyName::Colon,
            "," | "COMMA" => KeyName::Comma,
            "." | "PERIOD" | "DOT" => KeyName::Period,
            "\\" | "BACKSLASH" => KeyName::Backslash,
            "/" | "SLASH" => KeyName::Slash,
            "MINUS" => KeyName::Minus,
            "^" | "CARET" | "JISCARET" => KeyName::JisCaret,
            "@" | "AT" | "JISAT" => KeyName::JisAt,
            "YEN" | "¥" | "￥" | "INTLYEN" | "JISYEN" => KeyName::IntlYen,
            "RO" | "ろ" | "＼" | "INTLRO" | "JISRO" => KeyName::IntlRo,
            _ => return None,
        })
    }

    pub fn to_egui(self) -> Option<egui::Key> {
        Some(match self {
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
            KeyName::Numpad0 => egui::Key::Num0,
            KeyName::Numpad1 => egui::Key::Num1,
            KeyName::Numpad2 => egui::Key::Num2,
            KeyName::Numpad3 => egui::Key::Num3,
            KeyName::Numpad4 => egui::Key::Num4,
            KeyName::Numpad5 => egui::Key::Num5,
            KeyName::Numpad6 => egui::Key::Num6,
            KeyName::Numpad7 => egui::Key::Num7,
            KeyName::Numpad8 => egui::Key::Num8,
            KeyName::Numpad9 => egui::Key::Num9,
            KeyName::NumpadAdd
            | KeyName::NumpadSubtract
            | KeyName::NumpadMultiply
            | KeyName::NumpadDivide
            | KeyName::NumpadDecimal
            | KeyName::NumpadEnter
            | KeyName::JisCaret
            | KeyName::JisAt
            | KeyName::IntlYen
            | KeyName::IntlRo => return None,
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
            KeyName::F13 => egui::Key::F13,
            KeyName::F14 => egui::Key::F14,
            KeyName::F15 => egui::Key::F15,
            KeyName::F16 => egui::Key::F16,
            KeyName::F17 => egui::Key::F17,
            KeyName::F18 => egui::Key::F18,
            KeyName::F19 => egui::Key::F19,
            KeyName::F20 => egui::Key::F20,
            KeyName::F21 => egui::Key::F21,
            KeyName::F22 => egui::Key::F22,
            KeyName::F23 => egui::Key::F23,
            KeyName::F24 => egui::Key::F24,
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
            KeyName::Semicolon => egui::Key::Semicolon,
            KeyName::Colon => egui::Key::Colon,
            KeyName::Comma => egui::Key::Comma,
            KeyName::Period => egui::Key::Period,
            KeyName::Backslash => egui::Key::Backslash,
            KeyName::Slash => egui::Key::Slash,
            KeyName::Minus => egui::Key::Minus,
        })
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
            egui::Key::F13 => KeyName::F13,
            egui::Key::F14 => KeyName::F14,
            egui::Key::F15 => KeyName::F15,
            egui::Key::F16 => KeyName::F16,
            egui::Key::F17 => KeyName::F17,
            egui::Key::F18 => KeyName::F18,
            egui::Key::F19 => KeyName::F19,
            egui::Key::F20 => KeyName::F20,
            egui::Key::F21 => KeyName::F21,
            egui::Key::F22 => KeyName::F22,
            egui::Key::F23 => KeyName::F23,
            egui::Key::F24 => KeyName::F24,
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
            egui::Key::Semicolon => KeyName::Semicolon,
            egui::Key::Colon => KeyName::Colon,
            egui::Key::Comma => KeyName::Comma,
            egui::Key::Period => KeyName::Period,
            egui::Key::Backslash => KeyName::Backslash,
            egui::Key::Slash => KeyName::Slash,
            egui::Key::Minus => KeyName::Minus,
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
            KeyName::Numpad0 => 0x60,
            KeyName::Numpad1 => 0x61,
            KeyName::Numpad2 => 0x62,
            KeyName::Numpad3 => 0x63,
            KeyName::Numpad4 => 0x64,
            KeyName::Numpad5 => 0x65,
            KeyName::Numpad6 => 0x66,
            KeyName::Numpad7 => 0x67,
            KeyName::Numpad8 => 0x68,
            KeyName::Numpad9 => 0x69,
            KeyName::NumpadMultiply => 0x6A,
            KeyName::NumpadAdd => 0x6B,
            KeyName::NumpadSubtract => 0x6D,
            KeyName::NumpadDecimal => 0x6E,
            KeyName::NumpadDivide => 0x6F,
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
            KeyName::F13 => 0x7C,
            KeyName::F14 => 0x7D,
            KeyName::F15 => 0x7E,
            KeyName::F16 => 0x7F,
            KeyName::F17 => 0x80,
            KeyName::F18 => 0x81,
            KeyName::F19 => 0x82,
            KeyName::F20 => 0x83,
            KeyName::F21 => 0x84,
            KeyName::F22 => 0x85,
            KeyName::F23 => 0x86,
            KeyName::F24 => 0x87,
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
            KeyName::NumpadEnter => 0x0D,
            KeyName::Esc => 0x1B,
            KeyName::Tab => 0x09,
            KeyName::Backspace => 0x08,
            KeyName::Delete => 0x2E,
            KeyName::OpenBracket => 0xDB,
            KeyName::CloseBracket => 0xDD,
            KeyName::Semicolon => 0xBB,
            KeyName::Colon => 0xBA,
            KeyName::Comma => 0xBC,
            KeyName::Period => 0xBE,
            KeyName::Backslash => 0xDC,
            KeyName::Slash => 0xBF,
            KeyName::Minus => 0xBD,
            KeyName::JisCaret => 0xDE,
            KeyName::JisAt => 0xC0,
            KeyName::IntlYen => 0xDC,
            KeyName::IntlRo => 0xE2,
        }
    }

    pub fn matches_win32(self, virtual_key: u32, scan_code: u16, _extended: bool) -> bool {
        match self {
            KeyName::JisCaret => scan_code == Self::JIS_CARET_SCAN,
            KeyName::JisAt => scan_code == Self::JIS_AT_SCAN,
            KeyName::IntlYen => scan_code == Self::INTL_YEN_SCAN,
            KeyName::IntlRo => scan_code == Self::INTL_RO_SCAN,
            KeyName::Enter => virtual_key == self.to_vk() && !_extended,
            KeyName::NumpadEnter => virtual_key == self.to_vk() && _extended,
            // On JIS keyboards VK_OEM_5 is the Yen key.  Keep the legacy
            // Backslash slot for layouts that really report the US backslash
            // position, but do not let it steal the JIS Yen physical key.
            KeyName::Backslash => virtual_key == self.to_vk() && scan_code != Self::INTL_YEN_SCAN,
            _ => virtual_key == self.to_vk(),
        }
    }

    pub fn from_win32(virtual_key: u32, scan_code: u16, _extended: bool) -> Option<Self> {
        if scan_code == Self::JIS_CARET_SCAN {
            return Some(KeyName::JisCaret);
        }
        if scan_code == Self::JIS_AT_SCAN {
            return Some(KeyName::JisAt);
        }
        if scan_code == Self::INTL_YEN_SCAN {
            return Some(KeyName::IntlYen);
        }
        if scan_code == Self::INTL_RO_SCAN {
            return Some(KeyName::IntlRo);
        }
        if virtual_key == 0x0D && _extended {
            return Some(KeyName::NumpadEnter);
        }
        Some(match virtual_key {
            0x41 => KeyName::A,
            0x42 => KeyName::B,
            0x43 => KeyName::C,
            0x44 => KeyName::D,
            0x45 => KeyName::E,
            0x46 => KeyName::F,
            0x47 => KeyName::G,
            0x48 => KeyName::H,
            0x49 => KeyName::I,
            0x4A => KeyName::J,
            0x4B => KeyName::K,
            0x4C => KeyName::L,
            0x4D => KeyName::M,
            0x4E => KeyName::N,
            0x4F => KeyName::O,
            0x50 => KeyName::P,
            0x51 => KeyName::Q,
            0x52 => KeyName::R,
            0x53 => KeyName::S,
            0x54 => KeyName::T,
            0x55 => KeyName::U,
            0x56 => KeyName::V,
            0x57 => KeyName::W,
            0x58 => KeyName::X,
            0x59 => KeyName::Y,
            0x5A => KeyName::Z,
            0x30 => KeyName::Num0,
            0x31 => KeyName::Num1,
            0x32 => KeyName::Num2,
            0x33 => KeyName::Num3,
            0x34 => KeyName::Num4,
            0x35 => KeyName::Num5,
            0x36 => KeyName::Num6,
            0x37 => KeyName::Num7,
            0x38 => KeyName::Num8,
            0x39 => KeyName::Num9,
            0x60 => KeyName::Numpad0,
            0x61 => KeyName::Numpad1,
            0x62 => KeyName::Numpad2,
            0x63 => KeyName::Numpad3,
            0x64 => KeyName::Numpad4,
            0x65 => KeyName::Numpad5,
            0x66 => KeyName::Numpad6,
            0x67 => KeyName::Numpad7,
            0x68 => KeyName::Numpad8,
            0x69 => KeyName::Numpad9,
            0x6A => KeyName::NumpadMultiply,
            0x6B => KeyName::NumpadAdd,
            0x6D => KeyName::NumpadSubtract,
            0x6E => KeyName::NumpadDecimal,
            0x6F => KeyName::NumpadDivide,
            0x70 => KeyName::F1,
            0x71 => KeyName::F2,
            0x72 => KeyName::F3,
            0x73 => KeyName::F4,
            0x74 => KeyName::F5,
            0x75 => KeyName::F6,
            0x76 => KeyName::F7,
            0x77 => KeyName::F8,
            0x78 => KeyName::F9,
            0x79 => KeyName::F10,
            0x7A => KeyName::F11,
            0x7B => KeyName::F12,
            0x7C => KeyName::F13,
            0x7D => KeyName::F14,
            0x7E => KeyName::F15,
            0x7F => KeyName::F16,
            0x80 => KeyName::F17,
            0x81 => KeyName::F18,
            0x82 => KeyName::F19,
            0x83 => KeyName::F20,
            0x84 => KeyName::F21,
            0x85 => KeyName::F22,
            0x86 => KeyName::F23,
            0x87 => KeyName::F24,
            0x25 => KeyName::Left,
            0x26 => KeyName::Up,
            0x27 => KeyName::Right,
            0x28 => KeyName::Down,
            0x24 => KeyName::Home,
            0x23 => KeyName::End,
            0x21 => KeyName::PageUp,
            0x22 => KeyName::PageDown,
            0x20 => KeyName::Space,
            0x0D => KeyName::Enter,
            0x1B => KeyName::Esc,
            0x09 => KeyName::Tab,
            0x08 => KeyName::Backspace,
            0x2E => KeyName::Delete,
            0xDB => KeyName::OpenBracket,
            0xDD => KeyName::CloseBracket,
            0xBB => KeyName::Semicolon,
            0xBA => KeyName::Colon,
            0xBC => KeyName::Comma,
            0xBE => KeyName::Period,
            0xDC => KeyName::Backslash,
            0xBF => KeyName::Slash,
            0xBD => KeyName::Minus,
            _ => return None,
        })
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
            KeyName::Numpad0 => "Numpad0",
            KeyName::Numpad1 => "Numpad1",
            KeyName::Numpad2 => "Numpad2",
            KeyName::Numpad3 => "Numpad3",
            KeyName::Numpad4 => "Numpad4",
            KeyName::Numpad5 => "Numpad5",
            KeyName::Numpad6 => "Numpad6",
            KeyName::Numpad7 => "Numpad7",
            KeyName::Numpad8 => "Numpad8",
            KeyName::Numpad9 => "Numpad9",
            KeyName::NumpadAdd => "NumpadAdd",
            KeyName::NumpadSubtract => "NumpadSubtract",
            KeyName::NumpadMultiply => "NumpadMultiply",
            KeyName::NumpadDivide => "NumpadDivide",
            KeyName::NumpadDecimal => "NumpadDecimal",
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
            KeyName::F13 => "F13",
            KeyName::F14 => "F14",
            KeyName::F15 => "F15",
            KeyName::F16 => "F16",
            KeyName::F17 => "F17",
            KeyName::F18 => "F18",
            KeyName::F19 => "F19",
            KeyName::F20 => "F20",
            KeyName::F21 => "F21",
            KeyName::F22 => "F22",
            KeyName::F23 => "F23",
            KeyName::F24 => "F24",
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
            KeyName::NumpadEnter => "NumpadEnter",
            KeyName::Esc => "Esc",
            KeyName::Tab => "Tab",
            KeyName::Backspace => "Backspace",
            KeyName::Delete => "Delete",
            KeyName::OpenBracket => "[",
            KeyName::CloseBracket => "]",
            KeyName::Semicolon => ";",
            KeyName::Colon => ":",
            KeyName::Comma => ",",
            KeyName::Period => ".",
            KeyName::Backslash => "\\",
            KeyName::Slash => "/",
            KeyName::Minus => "-",
            KeyName::JisCaret => "^",
            KeyName::JisAt => "@",
            KeyName::IntlYen => "￥",
            KeyName::IntlRo => "＼",
        }
    }

    pub fn settings_name(self) -> &'static str {
        match self {
            KeyName::IntlYen => "Yen",
            KeyName::IntlRo => "Ro",
            _ => self.display_name(),
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
        self.key.is_some_and(|name| name.to_egui() == Some(key))
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

    fn matches_win32_parts(
        self,
        virtual_key: u32,
        scan_code: u16,
        extended: bool,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        self.key
            .is_some_and(|name| name.matches_win32(virtual_key, scan_code, extended))
            && self.ctrl == ctrl
            && self.shift == shift
            && self.alt == alt
    }

    #[cfg(windows)]
    fn matches_key_edge(self, edge: crate::key_input::KeyEdge) -> bool {
        self.matches_win32_parts(
            edge.virtual_key,
            edge.scan_code,
            edge.extended,
            edge.ctrl,
            edge.shift,
            edge.alt,
        )
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
        self.format_name(false)
    }

    pub fn settings_name(self) -> String {
        self.format_name(true)
    }

    fn format_name(self, settings: bool) -> String {
        let question_mark = self.shift && self.key == Some(KeyName::Slash);
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift && !question_mark {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if question_mark {
            parts.push("?".to_string());
            return parts.join("+");
        }
        if let Some(key) = self.key {
            parts.push(if settings {
                key.settings_name().to_string()
            } else {
                key.display_name().to_string()
            });
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

const fn digit_pair(main: KeyName, numpad: KeyName) -> ChordList {
    ChordList::two(Chord::key(main), Chord::key(numpad))
}

const fn ctrl_digit_pair(main: KeyName, numpad: KeyName) -> ChordList {
    ChordList::two(Chord::ctrl(main), Chord::ctrl(numpad))
}

const fn alt_digit_pair(main: KeyName, numpad: KeyName) -> ChordList {
    ChordList::two(Chord::alt(main), Chord::alt(numpad))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyAction {
    GlobalLocalSearch,
    GlobalFavSearch,
    GlobalMetadataSearch,
    GlobalOpenFolder,
    ToggleDetachedViewerMode,
    HelpShowContextShortcuts,
    GridFavoritePrev,
    GridFavoriteNext,
    GridOpenFavorite1,
    GridOpenFavorite2,
    GridOpenFavorite3,
    GridOpenFavorite4,
    GridOpenFavorite5,
    GridOpenFavorite6,
    GridOpenFavorite7,
    GridOpenFavorite8,
    GridOpenFavorite9,
    GridOpenFavorite10,
    GridOpenFavorite11,
    GridOpenFavorite12,
    GridOpenFavorite13,
    GridOpenFavorite14,
    GridOpenFavorite15,
    GridOpenFavorite16,
    GridOpenFavorite17,
    GridOpenFavorite18,
    GridOpenFavorite19,
    GridOpenFavorite20,
    GridOpenDriveC,
    GridOpenDriveD,
    GridOpenDriveE,
    GridOpenDriveF,
    GridOpenDriveG,
    GridOpenDriveH,
    GridOpenDriveI,
    GridOpenDriveJ,
    GridOpenDriveK,
    GridOpenDriveL,
    GridOpenDriveM,
    GridOpenDriveN,
    GridOpenDriveO,
    GridOpenDriveP,
    GridOpenDriveQ,
    GridOpenDriveR,
    GridOpenDriveS,
    GridOpenDriveT,
    GridOpenDriveU,
    GridOpenDriveV,
    GridOpenDriveW,
    GridOpenDriveX,
    GridOpenDriveY,
    GridOpenDriveZ,
    GridOpenLocationDriveList,
    GridOpenLocationReadingHistory,
    GridOpenLocationRating1,
    GridOpenLocationRating2,
    GridOpenLocationRating3,
    GridOpenLocationRating4,
    GridOpenLocationRating5,
    GridOpenLocationBooksRoot,
    GridOpenLocationDesktop,
    GridOpenLocationPictures,
    GridOpenLocationDownloads,
    GridTogglePinnedTag1,
    GridTogglePinnedTag2,
    GridTogglePinnedTag3,
    GridTogglePinnedTag4,
    GridTogglePinnedTag5,
    GridTogglePinnedTag6,
    GridTogglePinnedTag7,
    GridTogglePinnedTag8,
    GridTogglePinnedTag9,
    GridTogglePinnedTag10,
    GridTogglePinnedTag11,
    GridTogglePinnedTag12,
    GridTogglePinnedTag13,
    GridTogglePinnedTag14,
    GridTogglePinnedTag15,
    GridTogglePinnedTag16,
    GridTogglePinnedTag17,
    GridTogglePinnedTag18,
    GridTogglePinnedTag19,
    GridTogglePinnedTag20,
    GridSelectAll,
    GridDeselect,
    GridToggleCheck,
    GridDelete,
    GridOpenSelected,
    GridOpenExternalPlayer,
    GridParentFolder,
    GridHistoryBack,
    GridHistoryForward,
    GridMoveFirst,
    GridMoveLast,
    GridPagePrev,
    GridPageNext,
    GridTreeFolderPrev,
    GridTreeFolderNext,
    GridSiblingFolderPrev,
    GridSiblingFolderNext,
    GridToggleMaximize,
    GridToggleFolderTreePane,
    GridToggleStackMode,
    GridTagApply,
    GridTagView,
    GridRotateCw,
    GridRotateCcw,
    GridPin,
    GridComparePin,
    GridAddToActiveBook,
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
    GridToggleDetailsView,
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
    GridApplyErase1,
    GridApplyErase2,
    GridApplyConceal1,
    GridApplyConceal2,
    GridDeleteEraseMask,
    GridDeleteConcealMask,
    FsToggleMetadata,
    FsClose,
    FsBackToList,
    FsToggleWindowMode,
    FsJumpFirst,
    FsJumpLast,
    FsCtrlNavPrev,
    FsCtrlNavNext,
    FsSiblingPrev,
    FsSiblingNext,
    FsPagePrev,
    FsPageNext,
    FsFixedJumpPrev,
    FsFixedJumpNext,
    FsFixedJumpPrevNoRtl,
    FsFixedJumpNextNoRtl,
    FsStackJumpPrev,
    FsStackJumpNext,
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
    FsAddToActiveBook,
    FsExport,
    FsCompareToggle,
    FsCompareCycle,
    FsCompareWipe,
    FsCompareDiff,
    FsRotateCw,
    FsRotateCcw,
    FsImageAnalysis,
    FsZoomMode,
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
    FsAiModelAuto,
    FsAiModelRealEsrganX4Plus,
    FsAiModelRealEsrganAnime6B,
    FsAiModelRealCugan4x,
    FsAiModelNmkdSiax4x,
    FsAiModelRealEsrGeneralV3,
    FsDenoiseCycle,
    FsPostFilterNext,
    FsPostFilterPrev,
    FsPostFilterReset,
    FsPostFilterNearest,
    FsPostFilterCrtSimple,
    FsPostFilterCrtFull,
    FsPostFilterCrtArcade,
    FsPostFilterDither1bit,
    FsPostFilterGameBoy,
    FsPostFilterPc98,
    FsPostFilterGameGear,
    FsPostFilterFamicom,
    FsPostFilterMegaDrive,
    FsPostFilterMsx2Plus,
    FsPostFilterSfc,
    FsPostFilterComboFamicomCrt,
    FsPostFilterComboPc98Crt,
    FsPostFilterComboMsx2PlusCrt,
    FsPostFilterComboMegaDriveCrt,
    FsPostFilterComboSfcCrt,
    FsPostFilterSepia,
    FsPostFilterMonoNeutral,
    FsPostFilterMonoCool,
    FsPostFilterMonoWarm,
    FsPostFilterWarmTone,
    FsPostFilterCoolTone,
    FsPostFilterTealOrange,
    FsPostFilterKodakPortra,
    FsPostFilterFujiVelvia,
    FsPostFilterBleachBypass,
    FsPostFilterCrossProcess,
    FsPostFilterVintage,
    FsPostFilterFilmGrain,
    FsPostFilterVignette,
    FsPostFilterLightLeak,
    FsPostFilterSoftFocus,
    FsPostFilterHalftone,
    FsPostFilterOilPaint,
    FsPostFilterSketch,
    FsPostFilterPseudoColor4,
    FsPostFilterPseudoColorSkin,
    FsPostFilterSharpen,
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
    VideoCloseFullscreen,
    VideoPlayPause,
    VideoSeekStart,
    VideoSeekBackSmall,
    VideoSeekForwardSmall,
    VideoSeekBackLarge,
    VideoSeekForwardLarge,
    VideoFrameStepBack,
    VideoFrameStepForward,
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
    VideoAddToActiveBook,
    VideoCompareToggle,
    VideoCompareCycle,
    VideoCompareWipe,
    VideoCompareDiff,
    EraseConfirm,
    EraseConfirmPolygon,
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
    ConcealConfirmPolygon,
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
    LaConfirmPolygon,
    LaSpacePan,
}

const ALL_ACTIONS: &[KeyAction] = &[
    KeyAction::GlobalLocalSearch,
    KeyAction::GlobalFavSearch,
    KeyAction::GlobalMetadataSearch,
    KeyAction::GlobalOpenFolder,
    KeyAction::ToggleDetachedViewerMode,
    KeyAction::HelpShowContextShortcuts,
    KeyAction::GridFavoritePrev,
    KeyAction::GridFavoriteNext,
    KeyAction::GridOpenFavorite1,
    KeyAction::GridOpenFavorite2,
    KeyAction::GridOpenFavorite3,
    KeyAction::GridOpenFavorite4,
    KeyAction::GridOpenFavorite5,
    KeyAction::GridOpenFavorite6,
    KeyAction::GridOpenFavorite7,
    KeyAction::GridOpenFavorite8,
    KeyAction::GridOpenFavorite9,
    KeyAction::GridOpenFavorite10,
    KeyAction::GridOpenFavorite11,
    KeyAction::GridOpenFavorite12,
    KeyAction::GridOpenFavorite13,
    KeyAction::GridOpenFavorite14,
    KeyAction::GridOpenFavorite15,
    KeyAction::GridOpenFavorite16,
    KeyAction::GridOpenFavorite17,
    KeyAction::GridOpenFavorite18,
    KeyAction::GridOpenFavorite19,
    KeyAction::GridOpenFavorite20,
    KeyAction::GridOpenDriveC,
    KeyAction::GridOpenDriveD,
    KeyAction::GridOpenDriveE,
    KeyAction::GridOpenDriveF,
    KeyAction::GridOpenDriveG,
    KeyAction::GridOpenDriveH,
    KeyAction::GridOpenDriveI,
    KeyAction::GridOpenDriveJ,
    KeyAction::GridOpenDriveK,
    KeyAction::GridOpenDriveL,
    KeyAction::GridOpenDriveM,
    KeyAction::GridOpenDriveN,
    KeyAction::GridOpenDriveO,
    KeyAction::GridOpenDriveP,
    KeyAction::GridOpenDriveQ,
    KeyAction::GridOpenDriveR,
    KeyAction::GridOpenDriveS,
    KeyAction::GridOpenDriveT,
    KeyAction::GridOpenDriveU,
    KeyAction::GridOpenDriveV,
    KeyAction::GridOpenDriveW,
    KeyAction::GridOpenDriveX,
    KeyAction::GridOpenDriveY,
    KeyAction::GridOpenDriveZ,
    KeyAction::GridOpenLocationDriveList,
    KeyAction::GridOpenLocationReadingHistory,
    KeyAction::GridOpenLocationRating1,
    KeyAction::GridOpenLocationRating2,
    KeyAction::GridOpenLocationRating3,
    KeyAction::GridOpenLocationRating4,
    KeyAction::GridOpenLocationRating5,
    KeyAction::GridOpenLocationBooksRoot,
    KeyAction::GridOpenLocationDesktop,
    KeyAction::GridOpenLocationPictures,
    KeyAction::GridOpenLocationDownloads,
    KeyAction::GridTogglePinnedTag1,
    KeyAction::GridTogglePinnedTag2,
    KeyAction::GridTogglePinnedTag3,
    KeyAction::GridTogglePinnedTag4,
    KeyAction::GridTogglePinnedTag5,
    KeyAction::GridTogglePinnedTag6,
    KeyAction::GridTogglePinnedTag7,
    KeyAction::GridTogglePinnedTag8,
    KeyAction::GridTogglePinnedTag9,
    KeyAction::GridTogglePinnedTag10,
    KeyAction::GridTogglePinnedTag11,
    KeyAction::GridTogglePinnedTag12,
    KeyAction::GridTogglePinnedTag13,
    KeyAction::GridTogglePinnedTag14,
    KeyAction::GridTogglePinnedTag15,
    KeyAction::GridTogglePinnedTag16,
    KeyAction::GridTogglePinnedTag17,
    KeyAction::GridTogglePinnedTag18,
    KeyAction::GridTogglePinnedTag19,
    KeyAction::GridTogglePinnedTag20,
    KeyAction::GridSelectAll,
    KeyAction::GridDeselect,
    KeyAction::GridToggleCheck,
    KeyAction::GridDelete,
    KeyAction::GridOpenSelected,
    KeyAction::GridOpenExternalPlayer,
    KeyAction::GridParentFolder,
    KeyAction::GridHistoryBack,
    KeyAction::GridHistoryForward,
    KeyAction::GridMoveFirst,
    KeyAction::GridMoveLast,
    KeyAction::GridPagePrev,
    KeyAction::GridPageNext,
    KeyAction::GridTreeFolderPrev,
    KeyAction::GridTreeFolderNext,
    KeyAction::GridSiblingFolderPrev,
    KeyAction::GridSiblingFolderNext,
    KeyAction::GridToggleMaximize,
    KeyAction::GridToggleFolderTreePane,
    KeyAction::GridToggleStackMode,
    KeyAction::GridTagApply,
    KeyAction::GridTagView,
    KeyAction::GridRotateCw,
    KeyAction::GridRotateCcw,
    KeyAction::GridPin,
    KeyAction::GridComparePin,
    KeyAction::GridAddToActiveBook,
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
    KeyAction::GridToggleDetailsView,
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
    KeyAction::GridApplyErase1,
    KeyAction::GridApplyErase2,
    KeyAction::GridApplyConceal1,
    KeyAction::GridApplyConceal2,
    KeyAction::GridDeleteEraseMask,
    KeyAction::GridDeleteConcealMask,
    KeyAction::FsToggleMetadata,
    KeyAction::FsClose,
    KeyAction::FsBackToList,
    KeyAction::FsToggleWindowMode,
    KeyAction::FsJumpFirst,
    KeyAction::FsJumpLast,
    KeyAction::FsCtrlNavPrev,
    KeyAction::FsCtrlNavNext,
    KeyAction::FsSiblingPrev,
    KeyAction::FsSiblingNext,
    KeyAction::FsPagePrev,
    KeyAction::FsPageNext,
    KeyAction::FsFixedJumpPrev,
    KeyAction::FsFixedJumpNext,
    KeyAction::FsFixedJumpPrevNoRtl,
    KeyAction::FsFixedJumpNextNoRtl,
    KeyAction::FsStackJumpPrev,
    KeyAction::FsStackJumpNext,
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
    KeyAction::FsAddToActiveBook,
    KeyAction::FsExport,
    KeyAction::FsCompareToggle,
    KeyAction::FsCompareCycle,
    KeyAction::FsCompareWipe,
    KeyAction::FsCompareDiff,
    KeyAction::FsRotateCw,
    KeyAction::FsRotateCcw,
    KeyAction::FsImageAnalysis,
    KeyAction::FsZoomMode,
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
    KeyAction::FsAiModelAuto,
    KeyAction::FsAiModelRealEsrganX4Plus,
    KeyAction::FsAiModelRealEsrganAnime6B,
    KeyAction::FsAiModelRealCugan4x,
    KeyAction::FsAiModelNmkdSiax4x,
    KeyAction::FsAiModelRealEsrGeneralV3,
    KeyAction::FsDenoiseCycle,
    KeyAction::FsPostFilterNext,
    KeyAction::FsPostFilterPrev,
    KeyAction::FsPostFilterReset,
    KeyAction::FsPostFilterNearest,
    KeyAction::FsPostFilterCrtSimple,
    KeyAction::FsPostFilterCrtFull,
    KeyAction::FsPostFilterCrtArcade,
    KeyAction::FsPostFilterDither1bit,
    KeyAction::FsPostFilterGameBoy,
    KeyAction::FsPostFilterPc98,
    KeyAction::FsPostFilterGameGear,
    KeyAction::FsPostFilterFamicom,
    KeyAction::FsPostFilterMegaDrive,
    KeyAction::FsPostFilterMsx2Plus,
    KeyAction::FsPostFilterSfc,
    KeyAction::FsPostFilterComboFamicomCrt,
    KeyAction::FsPostFilterComboPc98Crt,
    KeyAction::FsPostFilterComboMsx2PlusCrt,
    KeyAction::FsPostFilterComboMegaDriveCrt,
    KeyAction::FsPostFilterComboSfcCrt,
    KeyAction::FsPostFilterSepia,
    KeyAction::FsPostFilterMonoNeutral,
    KeyAction::FsPostFilterMonoCool,
    KeyAction::FsPostFilterMonoWarm,
    KeyAction::FsPostFilterWarmTone,
    KeyAction::FsPostFilterCoolTone,
    KeyAction::FsPostFilterTealOrange,
    KeyAction::FsPostFilterKodakPortra,
    KeyAction::FsPostFilterFujiVelvia,
    KeyAction::FsPostFilterBleachBypass,
    KeyAction::FsPostFilterCrossProcess,
    KeyAction::FsPostFilterVintage,
    KeyAction::FsPostFilterFilmGrain,
    KeyAction::FsPostFilterVignette,
    KeyAction::FsPostFilterLightLeak,
    KeyAction::FsPostFilterSoftFocus,
    KeyAction::FsPostFilterHalftone,
    KeyAction::FsPostFilterOilPaint,
    KeyAction::FsPostFilterSketch,
    KeyAction::FsPostFilterPseudoColor4,
    KeyAction::FsPostFilterPseudoColorSkin,
    KeyAction::FsPostFilterSharpen,
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
    KeyAction::VideoCloseFullscreen,
    KeyAction::VideoPlayPause,
    KeyAction::VideoSeekStart,
    KeyAction::VideoSeekBackSmall,
    KeyAction::VideoSeekForwardSmall,
    KeyAction::VideoSeekBackLarge,
    KeyAction::VideoSeekForwardLarge,
    KeyAction::VideoFrameStepBack,
    KeyAction::VideoFrameStepForward,
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
    KeyAction::VideoAddToActiveBook,
    KeyAction::VideoCompareToggle,
    KeyAction::VideoCompareCycle,
    KeyAction::VideoCompareWipe,
    KeyAction::VideoCompareDiff,
    KeyAction::EraseConfirm,
    KeyAction::EraseConfirmPolygon,
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
    KeyAction::ConcealConfirmPolygon,
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
    KeyAction::LaConfirmPolygon,
    KeyAction::LaSpacePan,
];
// Keep this list in sync with `KeyAction`. The keymap tests compare the enum
// inventory and this array so newly added actions cannot silently miss ini generation.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandSpec {
    pub action: KeyAction,
    pub scope: CommandScope,
    pub trigger: KeyTrigger,
    pub binding_policy: BindingPolicy,
}

impl CommandSpec {
    pub fn from_action(action: KeyAction) -> Self {
        Self {
            action,
            scope: action.context(),
            trigger: action.trigger(),
            binding_policy: BindingPolicy::for_trigger(action.trigger()),
        }
    }

    pub fn ini_name(self) -> &'static str {
        self.action.ini_name()
    }

    pub fn description(self) -> &'static str {
        self.action.description()
    }
}

pub fn command_catalog() -> impl Iterator<Item = CommandSpec> {
    KeyAction::all()
        .iter()
        .copied()
        .map(CommandSpec::from_action)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum TopMenuId {
    File,
    Favorites,
    Books,
    Video,
    Tags,
    Settings,
    Help,
}

impl TopMenuId {
    pub const ALL: &'static [Self] = &[
        Self::File,
        Self::Favorites,
        Self::Books,
        Self::Video,
        Self::Tags,
        Self::Settings,
        Self::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TopMenuId::File => "ファイル",
            TopMenuId::Favorites => "お気に入り",
            TopMenuId::Books => "製本",
            TopMenuId::Video => "動画",
            TopMenuId::Tags => "タグ",
            TopMenuId::Settings => "設定",
            TopMenuId::Help => "ヘルプ",
        }
    }

    pub fn stable_name(self) -> &'static str {
        match self {
            TopMenuId::File => "File",
            TopMenuId::Favorites => "Favorites",
            TopMenuId::Books => "Books",
            TopMenuId::Video => "Video",
            TopMenuId::Tags => "Tags",
            TopMenuId::Settings => "Settings",
            TopMenuId::Help => "Help",
        }
    }

    pub fn parse_stable_name(name: &str) -> Option<Self> {
        match name {
            "File" => Some(TopMenuId::File),
            "Favorites" => Some(TopMenuId::Favorites),
            "Books" => Some(TopMenuId::Books),
            "Video" => Some(TopMenuId::Video),
            "Tags" => Some(TopMenuId::Tags),
            "Settings" => Some(TopMenuId::Settings),
            "Help" => Some(TopMenuId::Help),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum MenuCommandId {
    FileOpenFolder,
    FileReadingHistory,
    FileLocalSearch,
    FileOpenCaptureFolder,
    FileOpenRecycleBin,
    FileQuit,
    FavoritesAddCurrentFolder,
    FavoritesEdit,
    FavoritesFavSearch,
    FavoritesMetadataSearch,
    BooksAddSelectionToActiveBook,
    BooksAddClipboardImage,
    BooksOpenRoot,
    BooksOpenActiveBook,
    BooksReorderCurrentBook,
    BooksManage,
    VideoRegisterUpscale,
    VideoDeleteUpscale,
    VideoShowUpscaleTasks,
    TagsManagePinned,
    TagsTagView,
    SettingsThumbnailCache,
    SettingsArchiveCache,
    SettingsThumbnailQuality,
    SettingsStats,
    SettingsResetRotation,
    SettingsRestoreSettings,
    SettingsOperationCustomize,
    SettingsPreferences,
    HelpOpenManual,
    HelpOpenLogs,
    HelpShowWhatsNew,
    HelpAbout,
}

impl MenuCommandId {
    pub const ALL: &'static [Self] = &[
        Self::FileOpenFolder,
        Self::FileReadingHistory,
        Self::FileLocalSearch,
        Self::FileOpenCaptureFolder,
        Self::FileOpenRecycleBin,
        Self::FileQuit,
        Self::FavoritesAddCurrentFolder,
        Self::FavoritesEdit,
        Self::FavoritesFavSearch,
        Self::FavoritesMetadataSearch,
        Self::BooksAddSelectionToActiveBook,
        Self::BooksAddClipboardImage,
        Self::BooksOpenRoot,
        Self::BooksOpenActiveBook,
        Self::BooksReorderCurrentBook,
        Self::BooksManage,
        Self::VideoRegisterUpscale,
        Self::VideoDeleteUpscale,
        Self::VideoShowUpscaleTasks,
        Self::TagsManagePinned,
        Self::TagsTagView,
        Self::SettingsThumbnailCache,
        Self::SettingsArchiveCache,
        Self::SettingsThumbnailQuality,
        Self::SettingsStats,
        Self::SettingsResetRotation,
        Self::SettingsRestoreSettings,
        Self::SettingsOperationCustomize,
        Self::SettingsPreferences,
        Self::HelpOpenManual,
        Self::HelpOpenLogs,
        Self::HelpShowWhatsNew,
        Self::HelpAbout,
    ];

    pub fn stable_name(self) -> &'static str {
        match self {
            MenuCommandId::FileOpenFolder => "FileOpenFolder",
            MenuCommandId::FileReadingHistory => "FileReadingHistory",
            MenuCommandId::FileLocalSearch => "FileLocalSearch",
            MenuCommandId::FileOpenCaptureFolder => "FileOpenCaptureFolder",
            MenuCommandId::FileOpenRecycleBin => "FileOpenRecycleBin",
            MenuCommandId::FileQuit => "FileQuit",
            MenuCommandId::FavoritesAddCurrentFolder => "FavoritesAddCurrentFolder",
            MenuCommandId::FavoritesEdit => "FavoritesEdit",
            MenuCommandId::FavoritesFavSearch => "FavoritesFavSearch",
            MenuCommandId::FavoritesMetadataSearch => "FavoritesMetadataSearch",
            MenuCommandId::BooksAddSelectionToActiveBook => "BooksAddSelectionToActiveBook",
            MenuCommandId::BooksAddClipboardImage => "BooksAddClipboardImage",
            MenuCommandId::BooksOpenRoot => "BooksOpenRoot",
            MenuCommandId::BooksOpenActiveBook => "BooksOpenActiveBook",
            MenuCommandId::BooksReorderCurrentBook => "BooksReorderCurrentBook",
            MenuCommandId::BooksManage => "BooksManage",
            MenuCommandId::VideoRegisterUpscale => "VideoRegisterUpscale",
            MenuCommandId::VideoDeleteUpscale => "VideoDeleteUpscale",
            MenuCommandId::VideoShowUpscaleTasks => "VideoShowUpscaleTasks",
            MenuCommandId::TagsManagePinned => "TagsManagePinned",
            MenuCommandId::TagsTagView => "TagsTagView",
            MenuCommandId::SettingsThumbnailCache => "SettingsThumbnailCache",
            MenuCommandId::SettingsArchiveCache => "SettingsArchiveCache",
            MenuCommandId::SettingsThumbnailQuality => "SettingsThumbnailQuality",
            MenuCommandId::SettingsStats => "SettingsStats",
            MenuCommandId::SettingsResetRotation => "SettingsResetRotation",
            MenuCommandId::SettingsRestoreSettings => "SettingsRestoreSettings",
            MenuCommandId::SettingsOperationCustomize => "SettingsOperationCustomize",
            MenuCommandId::SettingsPreferences => "SettingsPreferences",
            MenuCommandId::HelpOpenManual => "HelpOpenManual",
            MenuCommandId::HelpOpenLogs => "HelpOpenLogs",
            MenuCommandId::HelpShowWhatsNew => "HelpShowWhatsNew",
            MenuCommandId::HelpAbout => "HelpAbout",
        }
    }

    pub fn parse_stable_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|id| id.stable_name() == name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MenuCommandSpec {
    pub id: MenuCommandId,
    pub parent: TopMenuId,
    pub label: &'static str,
    pub action: Option<KeyAction>,
}

impl MenuCommandSpec {
    pub fn description(self) -> &'static str {
        self.action
            .map(KeyAction::description)
            .unwrap_or(self.label)
    }
}

const MENU_COMMAND_SPECS: &[MenuCommandSpec] = &[
    MenuCommandSpec {
        id: MenuCommandId::FileOpenFolder,
        parent: TopMenuId::File,
        label: "フォルダを開く…",
        action: Some(KeyAction::GlobalOpenFolder),
    },
    MenuCommandSpec {
        id: MenuCommandId::FileReadingHistory,
        parent: TopMenuId::File,
        label: "読書履歴を開く",
        action: Some(KeyAction::GridOpenLocationReadingHistory),
    },
    MenuCommandSpec {
        id: MenuCommandId::FileLocalSearch,
        parent: TopMenuId::File,
        label: "現在地フィルタ",
        action: Some(KeyAction::GlobalLocalSearch),
    },
    MenuCommandSpec {
        id: MenuCommandId::FileOpenCaptureFolder,
        parent: TopMenuId::File,
        label: "キャプチャ保存フォルダを開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::FileOpenRecycleBin,
        parent: TopMenuId::File,
        label: "ゴミ箱を開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::FileQuit,
        parent: TopMenuId::File,
        label: "終了",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::FavoritesAddCurrentFolder,
        parent: TopMenuId::Favorites,
        label: "このフォルダを追加…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::FavoritesEdit,
        parent: TopMenuId::Favorites,
        label: "編集",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::FavoritesFavSearch,
        parent: TopMenuId::Favorites,
        label: "コンテナ検索",
        action: Some(KeyAction::GlobalFavSearch),
    },
    MenuCommandSpec {
        id: MenuCommandId::FavoritesMetadataSearch,
        parent: TopMenuId::Favorites,
        label: "アイテム検索",
        action: Some(KeyAction::GlobalMetadataSearch),
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksAddSelectionToActiveBook,
        parent: TopMenuId::Books,
        label: "追加先の本に追加",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksAddClipboardImage,
        parent: TopMenuId::Books,
        label: "クリップボードの画像を本に追加",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksOpenRoot,
        parent: TopMenuId::Books,
        label: "本棚フォルダを開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksOpenActiveBook,
        parent: TopMenuId::Books,
        label: "追加先の本を開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksReorderCurrentBook,
        parent: TopMenuId::Books,
        label: "この本を並べ替え…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::BooksManage,
        parent: TopMenuId::Books,
        label: "製本の管理…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::VideoRegisterUpscale,
        parent: TopMenuId::Video,
        label: "この動画をアップスケール登録…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::VideoDeleteUpscale,
        parent: TopMenuId::Video,
        label: "この動画のアップスケールを削除",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::VideoShowUpscaleTasks,
        parent: TopMenuId::Video,
        label: "アップスケールタスク表示",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::TagsManagePinned,
        parent: TopMenuId::Tags,
        label: "ピン留めタグの管理…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::TagsTagView,
        parent: TopMenuId::Tags,
        label: "タグビュー",
        action: Some(KeyAction::GridTagView),
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsThumbnailCache,
        parent: TopMenuId::Settings,
        label: "サムネイルキャッシュ管理",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsArchiveCache,
        parent: TopMenuId::Settings,
        label: "変換済みアーカイブキャッシュ管理",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsThumbnailQuality,
        parent: TopMenuId::Settings,
        label: "サムネイル画質…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsStats,
        parent: TopMenuId::Settings,
        label: "統計…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsResetRotation,
        parent: TopMenuId::Settings,
        label: "回転情報をリセット…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsRestoreSettings,
        parent: TopMenuId::Settings,
        label: "設定の復元…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsOperationCustomize,
        parent: TopMenuId::Settings,
        label: "操作カスタマイズ…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::SettingsPreferences,
        parent: TopMenuId::Settings,
        label: "環境設定…",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::HelpOpenManual,
        parent: TopMenuId::Help,
        label: "ヘルプサイトを開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::HelpOpenLogs,
        parent: TopMenuId::Help,
        label: "ログフォルダを開く",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::HelpShowWhatsNew,
        parent: TopMenuId::Help,
        label: "重要な変更点を表示",
        action: None,
    },
    MenuCommandSpec {
        id: MenuCommandId::HelpAbout,
        parent: TopMenuId::Help,
        label: "バージョン情報",
        action: None,
    },
];

pub fn menu_command_catalog() -> &'static [MenuCommandSpec] {
    MENU_COMMAND_SPECS
}

pub fn menu_commands_for_parent(parent: TopMenuId) -> impl Iterator<Item = MenuCommandSpec> {
    MENU_COMMAND_SPECS
        .iter()
        .copied()
        .filter(move |spec| spec.parent == parent)
}

pub fn menu_command_can_be_hidden(id: MenuCommandId) -> bool {
    !matches!(id, MenuCommandId::SettingsPreferences)
}

pub fn menu_command_spec(id: MenuCommandId) -> Option<MenuCommandSpec> {
    MENU_COMMAND_SPECS
        .iter()
        .copied()
        .find(|spec| spec.id == id)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MenuLayoutSettings {
    /// Top menu order by `TopMenuId::stable_name()`. Missing menus are appended in default order.
    #[serde(default)]
    pub top_menu_order: Vec<String>,
    /// Per-top-menu command order by `MenuCommandId::stable_name()`.
    #[serde(default)]
    pub command_order: Vec<MenuCommandOrderSettings>,
    /// Commands hidden by `MenuCommandId::stable_name()`.
    #[serde(default)]
    pub hidden_commands: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MenuCommandOrderSettings {
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMenuLayout {
    pub menus: Vec<ResolvedTopMenu>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTopMenu {
    pub id: TopMenuId,
    pub commands: Vec<MenuCommandId>,
}

pub fn default_menu_layout_settings() -> MenuLayoutSettings {
    MenuLayoutSettings {
        top_menu_order: TopMenuId::ALL
            .iter()
            .map(|id| id.stable_name().to_string())
            .collect(),
        command_order: TopMenuId::ALL
            .iter()
            .map(|parent| MenuCommandOrderSettings {
                parent: parent.stable_name().to_string(),
                commands: menu_commands_for_parent(*parent)
                    .map(|spec| spec.id.stable_name().to_string())
                    .collect(),
            })
            .collect(),
        hidden_commands: Vec::new(),
    }
}

pub fn resolve_menu_layout(settings: &MenuLayoutSettings) -> ResolvedMenuLayout {
    let hidden: std::collections::BTreeSet<MenuCommandId> = settings
        .hidden_commands
        .iter()
        .filter_map(|name| MenuCommandId::parse_stable_name(name))
        .filter(|id| menu_command_can_be_hidden(*id))
        .collect();

    let mut parents = Vec::with_capacity(TopMenuId::ALL.len());
    for name in &settings.top_menu_order {
        if let Some(parent) = TopMenuId::parse_stable_name(name) {
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }
    }
    for &parent in TopMenuId::ALL {
        if !parents.contains(&parent) {
            parents.push(parent);
        }
    }

    let menus = parents
        .into_iter()
        .filter_map(|parent| {
            let commands = resolve_menu_commands_for_parent(settings, parent, &hidden);
            (!commands.is_empty()).then_some(ResolvedTopMenu {
                id: parent,
                commands,
            })
        })
        .collect();

    ResolvedMenuLayout { menus }
}

fn resolve_menu_commands_for_parent(
    settings: &MenuLayoutSettings,
    parent: TopMenuId,
    hidden: &std::collections::BTreeSet<MenuCommandId>,
) -> Vec<MenuCommandId> {
    let mut out = Vec::new();

    for order in &settings.command_order {
        if TopMenuId::parse_stable_name(&order.parent) != Some(parent) {
            continue;
        }
        for name in &order.commands {
            let Some(id) = MenuCommandId::parse_stable_name(name) else {
                continue;
            };
            if hidden.contains(&id) || out.contains(&id) {
                continue;
            }
            if menu_command_spec(id).is_some_and(|spec| spec.parent == parent) {
                out.push(id);
            }
        }
    }

    for spec in menu_commands_for_parent(parent) {
        if !hidden.contains(&spec.id) && !out.contains(&spec.id) {
            out.push(spec.id);
        }
    }

    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandDisplayRow {
    pub spec: CommandSpec,
    pub shortcut_labels: Vec<String>,
}

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

const FAVORITE_LOCATION_ACTIONS: [KeyAction; 20] = [
    KeyAction::GridOpenFavorite1,
    KeyAction::GridOpenFavorite2,
    KeyAction::GridOpenFavorite3,
    KeyAction::GridOpenFavorite4,
    KeyAction::GridOpenFavorite5,
    KeyAction::GridOpenFavorite6,
    KeyAction::GridOpenFavorite7,
    KeyAction::GridOpenFavorite8,
    KeyAction::GridOpenFavorite9,
    KeyAction::GridOpenFavorite10,
    KeyAction::GridOpenFavorite11,
    KeyAction::GridOpenFavorite12,
    KeyAction::GridOpenFavorite13,
    KeyAction::GridOpenFavorite14,
    KeyAction::GridOpenFavorite15,
    KeyAction::GridOpenFavorite16,
    KeyAction::GridOpenFavorite17,
    KeyAction::GridOpenFavorite18,
    KeyAction::GridOpenFavorite19,
    KeyAction::GridOpenFavorite20,
];

const DRIVE_LOCATION_ACTIONS: [KeyAction; 24] = [
    KeyAction::GridOpenDriveC,
    KeyAction::GridOpenDriveD,
    KeyAction::GridOpenDriveE,
    KeyAction::GridOpenDriveF,
    KeyAction::GridOpenDriveG,
    KeyAction::GridOpenDriveH,
    KeyAction::GridOpenDriveI,
    KeyAction::GridOpenDriveJ,
    KeyAction::GridOpenDriveK,
    KeyAction::GridOpenDriveL,
    KeyAction::GridOpenDriveM,
    KeyAction::GridOpenDriveN,
    KeyAction::GridOpenDriveO,
    KeyAction::GridOpenDriveP,
    KeyAction::GridOpenDriveQ,
    KeyAction::GridOpenDriveR,
    KeyAction::GridOpenDriveS,
    KeyAction::GridOpenDriveT,
    KeyAction::GridOpenDriveU,
    KeyAction::GridOpenDriveV,
    KeyAction::GridOpenDriveW,
    KeyAction::GridOpenDriveX,
    KeyAction::GridOpenDriveY,
    KeyAction::GridOpenDriveZ,
];

const PINNED_TAG_ACTIONS_ARRAY: [KeyAction; 20] = [
    KeyAction::GridTogglePinnedTag1,
    KeyAction::GridTogglePinnedTag2,
    KeyAction::GridTogglePinnedTag3,
    KeyAction::GridTogglePinnedTag4,
    KeyAction::GridTogglePinnedTag5,
    KeyAction::GridTogglePinnedTag6,
    KeyAction::GridTogglePinnedTag7,
    KeyAction::GridTogglePinnedTag8,
    KeyAction::GridTogglePinnedTag9,
    KeyAction::GridTogglePinnedTag10,
    KeyAction::GridTogglePinnedTag11,
    KeyAction::GridTogglePinnedTag12,
    KeyAction::GridTogglePinnedTag13,
    KeyAction::GridTogglePinnedTag14,
    KeyAction::GridTogglePinnedTag15,
    KeyAction::GridTogglePinnedTag16,
    KeyAction::GridTogglePinnedTag17,
    KeyAction::GridTogglePinnedTag18,
    KeyAction::GridTogglePinnedTag19,
    KeyAction::GridTogglePinnedTag20,
];

pub const PINNED_TAG_ACTIONS: &[KeyAction] = &PINNED_TAG_ACTIONS_ARRAY;

pub const LOCATION_NAVIGATION_ACTIONS: &[KeyAction] = &[
    KeyAction::GridFavoritePrev,
    KeyAction::GridFavoriteNext,
    KeyAction::GridOpenFavorite1,
    KeyAction::GridOpenFavorite2,
    KeyAction::GridOpenFavorite3,
    KeyAction::GridOpenFavorite4,
    KeyAction::GridOpenFavorite5,
    KeyAction::GridOpenFavorite6,
    KeyAction::GridOpenFavorite7,
    KeyAction::GridOpenFavorite8,
    KeyAction::GridOpenFavorite9,
    KeyAction::GridOpenFavorite10,
    KeyAction::GridOpenFavorite11,
    KeyAction::GridOpenFavorite12,
    KeyAction::GridOpenFavorite13,
    KeyAction::GridOpenFavorite14,
    KeyAction::GridOpenFavorite15,
    KeyAction::GridOpenFavorite16,
    KeyAction::GridOpenFavorite17,
    KeyAction::GridOpenFavorite18,
    KeyAction::GridOpenFavorite19,
    KeyAction::GridOpenFavorite20,
    KeyAction::GridOpenDriveC,
    KeyAction::GridOpenDriveD,
    KeyAction::GridOpenDriveE,
    KeyAction::GridOpenDriveF,
    KeyAction::GridOpenDriveG,
    KeyAction::GridOpenDriveH,
    KeyAction::GridOpenDriveI,
    KeyAction::GridOpenDriveJ,
    KeyAction::GridOpenDriveK,
    KeyAction::GridOpenDriveL,
    KeyAction::GridOpenDriveM,
    KeyAction::GridOpenDriveN,
    KeyAction::GridOpenDriveO,
    KeyAction::GridOpenDriveP,
    KeyAction::GridOpenDriveQ,
    KeyAction::GridOpenDriveR,
    KeyAction::GridOpenDriveS,
    KeyAction::GridOpenDriveT,
    KeyAction::GridOpenDriveU,
    KeyAction::GridOpenDriveV,
    KeyAction::GridOpenDriveW,
    KeyAction::GridOpenDriveX,
    KeyAction::GridOpenDriveY,
    KeyAction::GridOpenDriveZ,
    KeyAction::GridOpenLocationDriveList,
    KeyAction::GridOpenLocationReadingHistory,
    KeyAction::GridOpenLocationRating1,
    KeyAction::GridOpenLocationRating2,
    KeyAction::GridOpenLocationRating3,
    KeyAction::GridOpenLocationRating4,
    KeyAction::GridOpenLocationRating5,
    KeyAction::GridOpenLocationBooksRoot,
    KeyAction::GridOpenLocationDesktop,
    KeyAction::GridOpenLocationPictures,
    KeyAction::GridOpenLocationDownloads,
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

    pub fn is_user_facing(self) -> bool {
        !matches!(
            self,
            KeyAction::VideoCompareToggle
                | KeyAction::VideoCompareCycle
                | KeyAction::VideoCompareWipe
                | KeyAction::VideoCompareDiff
        )
    }

    pub fn favorite_slot_action(slot: usize) -> Option<Self> {
        FAVORITE_LOCATION_ACTIONS.get(slot.checked_sub(1)?).copied()
    }

    pub fn favorite_slot_number(self) -> Option<usize> {
        FAVORITE_LOCATION_ACTIONS
            .iter()
            .position(|action| *action == self)
            .map(|idx| idx + 1)
    }

    pub fn drive_action(letter: char) -> Option<Self> {
        let upper = letter.to_ascii_uppercase();
        if !('C'..='Z').contains(&upper) {
            return None;
        }
        DRIVE_LOCATION_ACTIONS
            .get((upper as u8 - b'C') as usize)
            .copied()
    }

    pub fn drive_letter(self) -> Option<char> {
        DRIVE_LOCATION_ACTIONS
            .iter()
            .position(|action| *action == self)
            .map(|idx| (b'C' + idx as u8) as char)
    }

    pub fn pinned_tag_slot_action(slot: usize) -> Option<Self> {
        PINNED_TAG_ACTIONS_ARRAY.get(slot.checked_sub(1)?).copied()
    }

    pub fn pinned_tag_slot_number(self) -> Option<usize> {
        PINNED_TAG_ACTIONS_ARRAY
            .iter()
            .position(|action| *action == self)
            .map(|idx| idx + 1)
    }

    pub fn location_rating_stars(self) -> Option<u8> {
        match self {
            Self::GridOpenLocationRating1 => Some(1),
            Self::GridOpenLocationRating2 => Some(2),
            Self::GridOpenLocationRating3 => Some(3),
            Self::GridOpenLocationRating4 => Some(4),
            Self::GridOpenLocationRating5 => Some(5),
            _ => None,
        }
    }

    pub fn is_location_navigation_action(self) -> bool {
        LOCATION_NAVIGATION_ACTIONS.contains(&self)
    }

    pub fn ini_name(self) -> &'static str {
        use KeyAction::*;
        if let Some(slot) = self.favorite_slot_number() {
            return match slot {
                1 => "GridOpenFavorite1",
                2 => "GridOpenFavorite2",
                3 => "GridOpenFavorite3",
                4 => "GridOpenFavorite4",
                5 => "GridOpenFavorite5",
                6 => "GridOpenFavorite6",
                7 => "GridOpenFavorite7",
                8 => "GridOpenFavorite8",
                9 => "GridOpenFavorite9",
                10 => "GridOpenFavorite10",
                11 => "GridOpenFavorite11",
                12 => "GridOpenFavorite12",
                13 => "GridOpenFavorite13",
                14 => "GridOpenFavorite14",
                15 => "GridOpenFavorite15",
                16 => "GridOpenFavorite16",
                17 => "GridOpenFavorite17",
                18 => "GridOpenFavorite18",
                19 => "GridOpenFavorite19",
                20 => "GridOpenFavorite20",
                _ => unreachable!("favorite slot is constrained to 1..=20"),
            };
        }
        if let Some(letter) = self.drive_letter() {
            return match letter {
                'C' => "GridOpenDriveC",
                'D' => "GridOpenDriveD",
                'E' => "GridOpenDriveE",
                'F' => "GridOpenDriveF",
                'G' => "GridOpenDriveG",
                'H' => "GridOpenDriveH",
                'I' => "GridOpenDriveI",
                'J' => "GridOpenDriveJ",
                'K' => "GridOpenDriveK",
                'L' => "GridOpenDriveL",
                'M' => "GridOpenDriveM",
                'N' => "GridOpenDriveN",
                'O' => "GridOpenDriveO",
                'P' => "GridOpenDriveP",
                'Q' => "GridOpenDriveQ",
                'R' => "GridOpenDriveR",
                'S' => "GridOpenDriveS",
                'T' => "GridOpenDriveT",
                'U' => "GridOpenDriveU",
                'V' => "GridOpenDriveV",
                'W' => "GridOpenDriveW",
                'X' => "GridOpenDriveX",
                'Y' => "GridOpenDriveY",
                'Z' => "GridOpenDriveZ",
                _ => unreachable!("drive letter is constrained to C..=Z"),
            };
        }
        if let Some(slot) = self.pinned_tag_slot_number() {
            return match slot {
                1 => "GridTogglePinnedTag1",
                2 => "GridTogglePinnedTag2",
                3 => "GridTogglePinnedTag3",
                4 => "GridTogglePinnedTag4",
                5 => "GridTogglePinnedTag5",
                6 => "GridTogglePinnedTag6",
                7 => "GridTogglePinnedTag7",
                8 => "GridTogglePinnedTag8",
                9 => "GridTogglePinnedTag9",
                10 => "GridTogglePinnedTag10",
                11 => "GridTogglePinnedTag11",
                12 => "GridTogglePinnedTag12",
                13 => "GridTogglePinnedTag13",
                14 => "GridTogglePinnedTag14",
                15 => "GridTogglePinnedTag15",
                16 => "GridTogglePinnedTag16",
                17 => "GridTogglePinnedTag17",
                18 => "GridTogglePinnedTag18",
                19 => "GridTogglePinnedTag19",
                20 => "GridTogglePinnedTag20",
                _ => unreachable!("pinned tag slot is constrained to 1..=20"),
            };
        }
        match self {
            GlobalLocalSearch => "GlobalLocalSearch",
            GlobalFavSearch => "GlobalFavSearch",
            GlobalMetadataSearch => "GlobalMetadataSearch",
            GlobalOpenFolder => "GlobalOpenFolder",
            ToggleDetachedViewerMode => "ToggleDetachedViewerMode",
            HelpShowContextShortcuts => "HelpShowContextShortcuts",
            GridFavoritePrev => "GridFavoritePrev",
            GridFavoriteNext => "GridFavoriteNext",
            GridOpenFavorite1
            | GridOpenFavorite2
            | GridOpenFavorite3
            | GridOpenFavorite4
            | GridOpenFavorite5
            | GridOpenFavorite6
            | GridOpenFavorite7
            | GridOpenFavorite8
            | GridOpenFavorite9
            | GridOpenFavorite10
            | GridOpenFavorite11
            | GridOpenFavorite12
            | GridOpenFavorite13
            | GridOpenFavorite14
            | GridOpenFavorite15
            | GridOpenFavorite16
            | GridOpenFavorite17
            | GridOpenFavorite18
            | GridOpenFavorite19
            | GridOpenFavorite20
            | GridOpenDriveC
            | GridOpenDriveD
            | GridOpenDriveE
            | GridOpenDriveF
            | GridOpenDriveG
            | GridOpenDriveH
            | GridOpenDriveI
            | GridOpenDriveJ
            | GridOpenDriveK
            | GridOpenDriveL
            | GridOpenDriveM
            | GridOpenDriveN
            | GridOpenDriveO
            | GridOpenDriveP
            | GridOpenDriveQ
            | GridOpenDriveR
            | GridOpenDriveS
            | GridOpenDriveT
            | GridOpenDriveU
            | GridOpenDriveV
            | GridOpenDriveW
            | GridOpenDriveX
            | GridOpenDriveY
            | GridOpenDriveZ
            | GridTogglePinnedTag1
            | GridTogglePinnedTag2
            | GridTogglePinnedTag3
            | GridTogglePinnedTag4
            | GridTogglePinnedTag5
            | GridTogglePinnedTag6
            | GridTogglePinnedTag7
            | GridTogglePinnedTag8
            | GridTogglePinnedTag9
            | GridTogglePinnedTag10
            | GridTogglePinnedTag11
            | GridTogglePinnedTag12
            | GridTogglePinnedTag13
            | GridTogglePinnedTag14
            | GridTogglePinnedTag15
            | GridTogglePinnedTag16
            | GridTogglePinnedTag17
            | GridTogglePinnedTag18
            | GridTogglePinnedTag19
            | GridTogglePinnedTag20 => {
                unreachable!("handled by compact slot helpers")
            }
            GridOpenLocationDriveList => "GridOpenLocationDriveList",
            GridOpenLocationReadingHistory => "GridOpenLocationReadingHistory",
            GridOpenLocationRating1 => "GridOpenLocationRating1",
            GridOpenLocationRating2 => "GridOpenLocationRating2",
            GridOpenLocationRating3 => "GridOpenLocationRating3",
            GridOpenLocationRating4 => "GridOpenLocationRating4",
            GridOpenLocationRating5 => "GridOpenLocationRating5",
            GridOpenLocationBooksRoot => "GridOpenLocationBooksRoot",
            GridOpenLocationDesktop => "GridOpenLocationDesktop",
            GridOpenLocationPictures => "GridOpenLocationPictures",
            GridOpenLocationDownloads => "GridOpenLocationDownloads",
            GridSelectAll => "GridSelectAll",
            GridDeselect => "GridDeselect",
            GridToggleCheck => "GridToggleCheck",
            GridDelete => "GridDelete",
            GridOpenSelected => "GridOpenSelected",
            GridOpenExternalPlayer => "GridOpenExternalPlayer",
            GridParentFolder => "GridParentFolder",
            GridHistoryBack => "GridHistoryBack",
            GridHistoryForward => "GridHistoryForward",
            GridMoveFirst => "GridMoveFirst",
            GridMoveLast => "GridMoveLast",
            GridPagePrev => "GridPagePrev",
            GridPageNext => "GridPageNext",
            GridTreeFolderPrev => "GridTreeFolderPrev",
            GridTreeFolderNext => "GridTreeFolderNext",
            GridSiblingFolderPrev => "GridSiblingFolderPrev",
            GridSiblingFolderNext => "GridSiblingFolderNext",
            GridToggleMaximize => "GridToggleMaximize",
            GridToggleFolderTreePane => "GridToggleFolderTreePane",
            GridToggleStackMode => "GridToggleStackMode",
            GridTagApply => "GridTagApply",
            GridTagView => "GridTagView",
            GridRotateCw => "GridRotateCw",
            GridRotateCcw => "GridRotateCcw",
            GridPin => "GridPin",
            GridComparePin => "GridComparePin",
            GridAddToActiveBook => "GridAddToActiveBook",
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
            GridToggleDetailsView => "GridToggleDetailsView",
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
            GridApplyErase1 => "GridApplyErase1",
            GridApplyErase2 => "GridApplyErase2",
            GridApplyConceal1 => "GridApplyConceal1",
            GridApplyConceal2 => "GridApplyConceal2",
            GridDeleteEraseMask => "GridDeleteEraseMask",
            GridDeleteConcealMask => "GridDeleteConcealMask",
            FsToggleMetadata => "FsToggleMetadata",
            FsClose => "FsClose",
            FsBackToList => "FsBackToList",
            FsToggleWindowMode => "FsToggleWindowMode",
            FsJumpFirst => "FsJumpFirst",
            FsJumpLast => "FsJumpLast",
            FsCtrlNavPrev => "FsCtrlNavPrev",
            FsCtrlNavNext => "FsCtrlNavNext",
            FsSiblingPrev => "FsSiblingPrev",
            FsSiblingNext => "FsSiblingNext",
            FsPagePrev => "FsPagePrev",
            FsPageNext => "FsPageNext",
            FsFixedJumpPrev => "FsFixedJumpPrev",
            FsFixedJumpNext => "FsFixedJumpNext",
            FsFixedJumpPrevNoRtl => "FsFixedJumpPrevNoRtl",
            FsFixedJumpNextNoRtl => "FsFixedJumpNextNoRtl",
            FsStackJumpPrev => "FsStackJumpPrev",
            FsStackJumpNext => "FsStackJumpNext",
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
            FsAddToActiveBook => "FsAddToActiveBook",
            FsExport => "FsExport",
            FsCompareToggle => "FsCompareToggle",
            FsCompareCycle => "FsCompareCycle",
            FsCompareWipe => "FsCompareWipe",
            FsCompareDiff => "FsCompareDiff",
            FsRotateCw => "FsRotateCw",
            FsRotateCcw => "FsRotateCcw",
            FsImageAnalysis => "FsImageAnalysis",
            FsZoomMode => "FsZoomMode",
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
            FsAiModelAuto => "FsAiModelAuto",
            FsAiModelRealEsrganX4Plus => "FsAiModelRealEsrganX4Plus",
            FsAiModelRealEsrganAnime6B => "FsAiModelRealEsrganAnime6B",
            FsAiModelRealCugan4x => "FsAiModelRealCugan4x",
            FsAiModelNmkdSiax4x => "FsAiModelNmkdSiax4x",
            FsAiModelRealEsrGeneralV3 => "FsAiModelRealEsrGeneralV3",
            FsDenoiseCycle => "FsDenoiseCycle",
            FsPostFilterNext => "FsPostFilterNext",
            FsPostFilterPrev => "FsPostFilterPrev",
            FsPostFilterReset => "FsPostFilterReset",
            FsPostFilterNearest => "FsPostFilterNearest",
            FsPostFilterCrtSimple => "FsPostFilterCrtSimple",
            FsPostFilterCrtFull => "FsPostFilterCrtFull",
            FsPostFilterCrtArcade => "FsPostFilterCrtArcade",
            FsPostFilterDither1bit => "FsPostFilterDither1bit",
            FsPostFilterGameBoy => "FsPostFilterGameBoy",
            FsPostFilterPc98 => "FsPostFilterPc98",
            FsPostFilterGameGear => "FsPostFilterGameGear",
            FsPostFilterFamicom => "FsPostFilterFamicom",
            FsPostFilterMegaDrive => "FsPostFilterMegaDrive",
            FsPostFilterMsx2Plus => "FsPostFilterMsx2Plus",
            FsPostFilterSfc => "FsPostFilterSfc",
            FsPostFilterComboFamicomCrt => "FsPostFilterComboFamicomCrt",
            FsPostFilterComboPc98Crt => "FsPostFilterComboPc98Crt",
            FsPostFilterComboMsx2PlusCrt => "FsPostFilterComboMsx2PlusCrt",
            FsPostFilterComboMegaDriveCrt => "FsPostFilterComboMegaDriveCrt",
            FsPostFilterComboSfcCrt => "FsPostFilterComboSfcCrt",
            FsPostFilterSepia => "FsPostFilterSepia",
            FsPostFilterMonoNeutral => "FsPostFilterMonoNeutral",
            FsPostFilterMonoCool => "FsPostFilterMonoCool",
            FsPostFilterMonoWarm => "FsPostFilterMonoWarm",
            FsPostFilterWarmTone => "FsPostFilterWarmTone",
            FsPostFilterCoolTone => "FsPostFilterCoolTone",
            FsPostFilterTealOrange => "FsPostFilterTealOrange",
            FsPostFilterKodakPortra => "FsPostFilterKodakPortra",
            FsPostFilterFujiVelvia => "FsPostFilterFujiVelvia",
            FsPostFilterBleachBypass => "FsPostFilterBleachBypass",
            FsPostFilterCrossProcess => "FsPostFilterCrossProcess",
            FsPostFilterVintage => "FsPostFilterVintage",
            FsPostFilterFilmGrain => "FsPostFilterFilmGrain",
            FsPostFilterVignette => "FsPostFilterVignette",
            FsPostFilterLightLeak => "FsPostFilterLightLeak",
            FsPostFilterSoftFocus => "FsPostFilterSoftFocus",
            FsPostFilterHalftone => "FsPostFilterHalftone",
            FsPostFilterOilPaint => "FsPostFilterOilPaint",
            FsPostFilterSketch => "FsPostFilterSketch",
            FsPostFilterPseudoColor4 => "FsPostFilterPseudoColor4",
            FsPostFilterPseudoColorSkin => "FsPostFilterPseudoColorSkin",
            FsPostFilterSharpen => "FsPostFilterSharpen",
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
            VideoCloseFullscreen => "VideoCloseFullscreen",
            VideoPlayPause => "VideoPlayPause",
            VideoSeekStart => "VideoSeekStart",
            VideoSeekBackSmall => "VideoSeekBackSmall",
            VideoSeekForwardSmall => "VideoSeekForwardSmall",
            VideoSeekBackLarge => "VideoSeekBackLarge",
            VideoSeekForwardLarge => "VideoSeekForwardLarge",
            VideoFrameStepBack => "VideoFrameStepBack",
            VideoFrameStepForward => "VideoFrameStepForward",
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
            VideoAddToActiveBook => "VideoAddToActiveBook",
            VideoCompareToggle => "VideoCompareToggle",
            VideoCompareCycle => "VideoCompareCycle",
            VideoCompareWipe => "VideoCompareWipe",
            VideoCompareDiff => "VideoCompareDiff",
            EraseConfirm => "EraseConfirm",
            EraseConfirmPolygon => "EraseConfirmPolygon",
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
            ConcealConfirmPolygon => "ConcealConfirmPolygon",
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
            LaConfirmPolygon => "LaConfirmPolygon",
            LaSpacePan => "LaSpacePan",
        }
    }

    pub fn parse_ini_name(name: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|action| action.ini_name().eq_ignore_ascii_case(name.trim()))
    }

    pub fn description(self) -> &'static str {
        use KeyAction::*;
        if let Some(slot) = self.favorite_slot_number() {
            return match slot {
                1 => "お気に入り1を開く",
                2 => "お気に入り2を開く",
                3 => "お気に入り3を開く",
                4 => "お気に入り4を開く",
                5 => "お気に入り5を開く",
                6 => "お気に入り6を開く",
                7 => "お気に入り7を開く",
                8 => "お気に入り8を開く",
                9 => "お気に入り9を開く",
                10 => "お気に入り10を開く",
                11 => "お気に入り11を開く",
                12 => "お気に入り12を開く",
                13 => "お気に入り13を開く",
                14 => "お気に入り14を開く",
                15 => "お気に入り15を開く",
                16 => "お気に入り16を開く",
                17 => "お気に入り17を開く",
                18 => "お気に入り18を開く",
                19 => "お気に入り19を開く",
                20 => "お気に入り20を開く",
                _ => unreachable!("favorite slot is constrained to 1..=20"),
            };
        }
        if let Some(letter) = self.drive_letter() {
            return match letter {
                'C' => "C:\\を開く",
                'D' => "D:\\を開く",
                'E' => "E:\\を開く",
                'F' => "F:\\を開く",
                'G' => "G:\\を開く",
                'H' => "H:\\を開く",
                'I' => "I:\\を開く",
                'J' => "J:\\を開く",
                'K' => "K:\\を開く",
                'L' => "L:\\を開く",
                'M' => "M:\\を開く",
                'N' => "N:\\を開く",
                'O' => "O:\\を開く",
                'P' => "P:\\を開く",
                'Q' => "Q:\\を開く",
                'R' => "R:\\を開く",
                'S' => "S:\\を開く",
                'T' => "T:\\を開く",
                'U' => "U:\\を開く",
                'V' => "V:\\を開く",
                'W' => "W:\\を開く",
                'X' => "X:\\を開く",
                'Y' => "Y:\\を開く",
                'Z' => "Z:\\を開く",
                _ => unreachable!("drive letter is constrained to C..=Z"),
            };
        }
        if let Some(slot) = self.pinned_tag_slot_number() {
            return match slot {
                1 => "ピン留めタグ1を付与/解除する",
                2 => "ピン留めタグ2を付与/解除する",
                3 => "ピン留めタグ3を付与/解除する",
                4 => "ピン留めタグ4を付与/解除する",
                5 => "ピン留めタグ5を付与/解除する",
                6 => "ピン留めタグ6を付与/解除する",
                7 => "ピン留めタグ7を付与/解除する",
                8 => "ピン留めタグ8を付与/解除する",
                9 => "ピン留めタグ9を付与/解除する",
                10 => "ピン留めタグ10を付与/解除する",
                11 => "ピン留めタグ11を付与/解除する",
                12 => "ピン留めタグ12を付与/解除する",
                13 => "ピン留めタグ13を付与/解除する",
                14 => "ピン留めタグ14を付与/解除する",
                15 => "ピン留めタグ15を付与/解除する",
                16 => "ピン留めタグ16を付与/解除する",
                17 => "ピン留めタグ17を付与/解除する",
                18 => "ピン留めタグ18を付与/解除する",
                19 => "ピン留めタグ19を付与/解除する",
                20 => "ピン留めタグ20を付与/解除する",
                _ => unreachable!("pinned tag slot is constrained to 1..=20"),
            };
        }
        match self {
            GlobalLocalSearch => "現在地の一覧を絞り込み検索する",
            GlobalFavSearch => "お気に入りフォルダを横断検索する",
            GlobalMetadataSearch => "全フォルダのメタデータを検索する",
            GlobalOpenFolder => "フォルダを開くダイアログを表示する",
            ToggleDetachedViewerMode => "画像・動画ビューアの別ウィンドウモードを切り替える",
            HelpShowContextShortcuts => "現在のコンテキストで使えるショートカット一覧を表示する",
            GridFavoritePrev => "前のお気に入りへ移動する",
            GridFavoriteNext => "次のお気に入りへ移動する",
            GridOpenFavorite1
            | GridOpenFavorite2
            | GridOpenFavorite3
            | GridOpenFavorite4
            | GridOpenFavorite5
            | GridOpenFavorite6
            | GridOpenFavorite7
            | GridOpenFavorite8
            | GridOpenFavorite9
            | GridOpenFavorite10
            | GridOpenFavorite11
            | GridOpenFavorite12
            | GridOpenFavorite13
            | GridOpenFavorite14
            | GridOpenFavorite15
            | GridOpenFavorite16
            | GridOpenFavorite17
            | GridOpenFavorite18
            | GridOpenFavorite19
            | GridOpenFavorite20
            | GridOpenDriveC
            | GridOpenDriveD
            | GridOpenDriveE
            | GridOpenDriveF
            | GridOpenDriveG
            | GridOpenDriveH
            | GridOpenDriveI
            | GridOpenDriveJ
            | GridOpenDriveK
            | GridOpenDriveL
            | GridOpenDriveM
            | GridOpenDriveN
            | GridOpenDriveO
            | GridOpenDriveP
            | GridOpenDriveQ
            | GridOpenDriveR
            | GridOpenDriveS
            | GridOpenDriveT
            | GridOpenDriveU
            | GridOpenDriveV
            | GridOpenDriveW
            | GridOpenDriveX
            | GridOpenDriveY
            | GridOpenDriveZ
            | GridTogglePinnedTag1
            | GridTogglePinnedTag2
            | GridTogglePinnedTag3
            | GridTogglePinnedTag4
            | GridTogglePinnedTag5
            | GridTogglePinnedTag6
            | GridTogglePinnedTag7
            | GridTogglePinnedTag8
            | GridTogglePinnedTag9
            | GridTogglePinnedTag10
            | GridTogglePinnedTag11
            | GridTogglePinnedTag12
            | GridTogglePinnedTag13
            | GridTogglePinnedTag14
            | GridTogglePinnedTag15
            | GridTogglePinnedTag16
            | GridTogglePinnedTag17
            | GridTogglePinnedTag18
            | GridTogglePinnedTag19
            | GridTogglePinnedTag20 => {
                unreachable!("handled by compact slot helpers")
            }
            GridOpenLocationDriveList => "ドライブ一覧を開く",
            GridOpenLocationReadingHistory => "読書履歴を開く",
            GridOpenLocationRating1 => "★1一覧を開く",
            GridOpenLocationRating2 => "★2一覧を開く",
            GridOpenLocationRating3 => "★3一覧を開く",
            GridOpenLocationRating4 => "★4一覧を開く",
            GridOpenLocationRating5 => "★5一覧を開く",
            GridOpenLocationBooksRoot => "本棚フォルダを開く",
            GridOpenLocationDesktop => "デスクトップを開く",
            GridOpenLocationPictures => "ピクチャを開く",
            GridOpenLocationDownloads => "ダウンロードを開く",
            GridSelectAll => "表示中のチェック可能な項目をすべてチェックする",
            GridDeselect => "チェックをすべて解除する",
            GridToggleCheck => "選択中の項目のチェックを切り替える",
            GridDelete => "選択中またはチェック済みの実ファイル/実フォルダを削除する",
            GridOpenSelected => "選択中の項目を開く",
            GridOpenExternalPlayer => "選択中の動画を外部プレイヤーで開く",
            GridParentFolder => "親フォルダへ移動する",
            GridHistoryBack => "フォルダ履歴を戻る",
            GridHistoryForward => "フォルダ履歴を進む",
            GridMoveFirst => "先頭の項目へ移動する",
            GridMoveLast => "末尾の項目へ移動する",
            GridPagePrev => "1ページ分前へ移動する",
            GridPageNext => "1ページ分先へ移動する",
            GridTreeFolderPrev => "ツリー順で前のフォルダへ移動する",
            GridTreeFolderNext => "ツリー順で次のフォルダへ移動する",
            GridSiblingFolderPrev => "前の兄弟フォルダへ移動する",
            GridSiblingFolderNext => "次の兄弟フォルダへ移動する",
            GridToggleMaximize => "メインウィンドウを最大化/復元する",
            GridToggleFolderTreePane => "フォルダツリーペインの表示を切り替える",
            GridToggleStackMode => "スタック表示を切り替える",
            GridTagApply => "タグを付ける/外すダイアログを開く",
            GridTagView => "タグビューを開く",
            GridRotateCw => "選択中の画像を右に90度回転する",
            GridRotateCcw => "選択中の画像を左に90度回転する",
            GridPin => "選択中の項目を代表サムネイルに固定または解除する",
            GridComparePin => "選択中の画像を比較スロットに固定または解除する",
            GridAddToActiveBook => "選択中またはチェック済みのページを追加先の本へ追加する",
            GridColumnCount1 => "サムネイル列数を1列にする",
            GridColumnCount2 => "サムネイル列数を2列にする",
            GridColumnCount3 => "サムネイル列数を3列にする",
            GridColumnCount4 => "サムネイル列数を4列にする",
            GridColumnCount5 => "サムネイル列数を5列にする",
            GridColumnCount6 => "サムネイル列数を6列にする",
            GridColumnCount7 => "サムネイル列数を7列にする",
            GridColumnCount8 => "サムネイル列数を8列にする",
            GridColumnCount9 => "サムネイル列数を9列にする",
            GridColumnCount10 => "サムネイル列数を10列にする",
            GridToggleDetailsView => "サムネイル一覧と詳細一覧を切り替える",
            GridAdjustSlot1 => "補正プリセットスロット1を適用する",
            GridAdjustSlot2 => "補正プリセットスロット2を適用する",
            GridAdjustSlot3 => "補正プリセットスロット3を適用する",
            GridAdjustSlot4 => "補正プリセットスロット4を適用する",
            GridAdjustSlot5 => "補正プリセットスロット5を適用する",
            GridAdjustSlot6 => "補正プリセットスロット6を適用する",
            GridAdjustSlot7 => "補正プリセットスロット7を適用する",
            GridAdjustSlot8 => "補正プリセットスロット8を適用する",
            GridAdjustSlot9 => "補正プリセットスロット9を適用する",
            GridAdjustSlot10 => "補正プリセットスロット10を適用する",
            GridClearAdjust => "選択中の画像の補正を解除する",
            GridApplyErase1 => "消しゴムマスクスロット1を選択中またはチェック済み画像に適用する",
            GridApplyErase2 => "消しゴムマスクスロット2を選択中またはチェック済み画像に適用する",
            GridApplyConceal1 => "隠蔽マスクスロット1を選択中またはチェック済み画像に適用する",
            GridApplyConceal2 => "隠蔽マスクスロット2を選択中またはチェック済み画像に適用する",
            GridDeleteEraseMask => "選択中またはチェック済み画像の消しゴムマスクを削除する",
            GridDeleteConcealMask => "選択中またはチェック済み画像の隠蔽マスクを削除する",
            FsToggleMetadata => "メタデータパネルの固定表示を切り替える",
            FsClose => "画像フルスクリーンを閉じる",
            FsBackToList => "フルスクリーンを閉じて一覧へ戻る",
            FsToggleWindowMode => "ウィンドウ表示と全画面表示を切り替える",
            FsJumpFirst => "先頭の項目へ移動する",
            FsJumpLast => "末尾の項目へ移動する",
            FsCtrlNavPrev => "前のフォルダまたは検索結果へ移動する",
            FsCtrlNavNext => "次のフォルダまたは検索結果へ移動する",
            FsSiblingPrev => "前の兄弟フォルダへ移動する",
            FsSiblingNext => "次の兄弟フォルダへ移動する",
            FsPagePrev => "前のページへ移動する",
            FsPageNext => "次のページへ移動する",
            FsFixedJumpPrev => "設定した量だけ前へジャンプする (右→左読みで反転)",
            FsFixedJumpNext => "設定した量だけ先へジャンプする (右→左読みで反転)",
            FsFixedJumpPrevNoRtl => "設定した量だけ前へジャンプする (右→左読みで反転しない)",
            FsFixedJumpNextNoRtl => "設定した量だけ先へジャンプする (右→左読みで反転しない)",
            FsStackJumpPrev => "前のスタックの先頭画像へジャンプする",
            FsStackJumpNext => "次のスタックの先頭画像へジャンプする",
            RatingItem1 => "星1を付ける（アイテム）",
            RatingItem2 => "星2を付ける（アイテム）",
            RatingItem3 => "星3を付ける（アイテム）",
            RatingItem4 => "星4を付ける（アイテム）",
            RatingItem5 => "星5を付ける（アイテム）",
            RatingItemClear => "レーティングを解除する（アイテム）",
            RatingContainer1 => "星1を付ける（コンテナ）",
            RatingContainer2 => "星2を付ける（コンテナ）",
            RatingContainer3 => "星3を付ける（コンテナ）",
            RatingContainer4 => "星4を付ける（コンテナ）",
            RatingContainer5 => "星5を付ける（コンテナ）",
            RatingContainerClear => "レーティングを解除する（コンテナ）",
            FsContinuousScrollForward => "連結表示中に次の画面分へスクロールする",
            FsContinuousScrollBack => "連結表示中に前の画面分へスクロールする",
            FsSpreadShiftLeft => "見開き表示を左方向へ1ページずらす",
            FsSpreadShiftRight => "見開き表示を右方向へ1ページずらす",
            FsSlideshow => "スライドショーの再生または停止を切り替える",
            FsSpaceCheck => "現在の画像のチェックを切り替える。スライドショー中は停止する",
            FsCapture => "現在の表示画像をキャプチャ保存する",
            FsAddToActiveBook => "現在のページを追加先の本へ追加する",
            FsExport => "現在の表示結果を別ファイルへ書き出す",
            FsCompareToggle => "現在の画像を比較スロットに固定または解除する",
            FsCompareCycle => "比較スロットのピン画像と現在画像を切り替えて表示する",
            FsCompareWipe => "ワイプ比較を切り替える",
            FsCompareDiff => "差分比較を切り替える",
            FsRotateCw => "現在の画像を右に90度回転する",
            FsRotateCcw => "現在の画像を左に90度回転する",
            FsImageAnalysis => "画像分析モードを開く",
            FsZoomMode => "全画面ズームモード (押している間ズーム範囲を指定)",
            FsPanorama => "360度パノラマモードを切り替える",
            FsPixelGrid => "ピクセルグリッド表示を切り替える",
            FsLoupeLockToggle => "ルーペの固定表示を切り替える",
            FsLoupeHold => "押している間だけルーペを表示する",
            FsEraseMode => "消しゴムモードを開始または確定する",
            FsConcealMode => "隠蔽加工モードを開始または終了する",
            FsTextMode => "テキスト注釈モードを開始または終了する",
            FsBgCycle => "透過背景色を切り替える",
            FsPin => "現在の項目を代表サムネイルに固定または解除する",
            FsSpreadSingle => "単ページ表示に切り替える",
            FsSpreadLtr => "左開き見開き表示に切り替える",
            FsSpreadLtrCover => "左開き表紙単独の見開き表示に切り替える",
            FsSpreadRtl => "右開き見開き表示に切り替える",
            FsSpreadRtlCover => "右開き表紙単独の見開き表示に切り替える",
            FsReadingFlowCycle => "ページ単位/縦連結/横連結を切り替える",
            FsReadingDirectionToggle => "横方向の読み進み方向を切り替える",
            FsFitModeCycle => "ズーム/フィット方式を切り替える",
            FsAiModelNext => "AIモデルを次へ切り替える",
            FsAiModelPrev => "AIモデルを前へ切り替える",
            FsAiModelReset => "AIモデルを標準に戻す",
            FsAiModelAuto => "AIモデルを自動（画像タイプ判別）にする",
            FsAiModelRealEsrganX4Plus => "AIモデルを写真・CG（ノイズ除去強）にする",
            FsAiModelRealEsrganAnime6B => "AIモデルをイラスト・アニメにする",
            FsAiModelRealCugan4x => "AIモデルを漫画（トーン保持）にする",
            FsAiModelNmkdSiax4x => "AIモデルを写真（質感保持）にする",
            FsAiModelRealEsrGeneralV3 => "AIモデルを高速汎用にする",
            FsDenoiseCycle => "デノイズ設定を切り替える",
            FsPostFilterNext => "ポストフィルタを次へ切り替える",
            FsPostFilterPrev => "ポストフィルタを前へ切り替える",
            FsPostFilterReset => "ポストフィルタを標準に戻す",
            FsPostFilterNearest => "ポストフィルタをニアレスト（補間なし）にする",
            FsPostFilterCrtSimple => "ポストフィルタをCRT シンプル（控えめ）にする",
            FsPostFilterCrtFull => "ポストフィルタをCRT フル（歪み+強グロー）にする",
            FsPostFilterCrtArcade => "ポストフィルタをCRT アーケード（高コントラスト）にする",
            FsPostFilterDither1bit => "ポストフィルタを1bit ディザにする",
            FsPostFilterGameBoy => "ポストフィルタをGameBoy（緑4階調）にする",
            FsPostFilterPc98 => "ポストフィルタをPC-98（16色・適応）にする",
            FsPostFilterGameGear => "ポストフィルタをゲームギア（32色・12bit）にする",
            FsPostFilterFamicom => "ポストフィルタをファミコン（52色・固定）にする",
            FsPostFilterMegaDrive => "ポストフィルタをメガドライブ（61色・9bit）にする",
            FsPostFilterMsx2Plus => "ポストフィルタをMSX2+（256色・GRB）にする",
            FsPostFilterSfc => "ポストフィルタをスーパーファミコン（256色・15bit）にする",
            FsPostFilterComboFamicomCrt => "ポストフィルタをCRT × ファミコンにする",
            FsPostFilterComboPc98Crt => "ポストフィルタをCRT × PC-98にする",
            FsPostFilterComboMsx2PlusCrt => "ポストフィルタをCRT × MSX2+にする",
            FsPostFilterComboMegaDriveCrt => "ポストフィルタをCRT × メガドライブにする",
            FsPostFilterComboSfcCrt => "ポストフィルタをCRT × スーパーファミコンにする",
            FsPostFilterSepia => "ポストフィルタをセピアにする",
            FsPostFilterMonoNeutral => "ポストフィルタをモノクロ（ニュートラル）にする",
            FsPostFilterMonoCool => "ポストフィルタをモノクロ（冷調）にする",
            FsPostFilterMonoWarm => "ポストフィルタをモノクロ（暖調）にする",
            FsPostFilterWarmTone => "ポストフィルタを暖色調にする",
            FsPostFilterCoolTone => "ポストフィルタを寒色調にする",
            FsPostFilterTealOrange => "ポストフィルタをTeal & Orange（シネマ調）にする",
            FsPostFilterKodakPortra => "ポストフィルタをKodak Portra 風にする",
            FsPostFilterFujiVelvia => "ポストフィルタをFuji Velvia 風にする",
            FsPostFilterBleachBypass => "ポストフィルタをブリーチバイパスにする",
            FsPostFilterCrossProcess => "ポストフィルタをクロスプロセスにする",
            FsPostFilterVintage => "ポストフィルタをビンテージ / 褪色にする",
            FsPostFilterFilmGrain => "ポストフィルタをフィルムグレインにする",
            FsPostFilterVignette => "ポストフィルタをビネット（周辺減光）にする",
            FsPostFilterLightLeak => "ポストフィルタをライトリークにする",
            FsPostFilterSoftFocus => "ポストフィルタをソフトフォーカスにする",
            FsPostFilterHalftone => "ポストフィルタをハーフトーン（漫画風）にする",
            FsPostFilterOilPaint => "ポストフィルタをオイルペイント風にする",
            FsPostFilterSketch => "ポストフィルタをスケッチ風にする",
            FsPostFilterPseudoColor4 => "ポストフィルタを疑似カラー（4色刷り）にする",
            FsPostFilterPseudoColorSkin => "ポストフィルタを疑似カラー（肌色）にする",
            FsPostFilterSharpen => "ポストフィルタをシャープ化にする",
            FsAdjustSlot1 => "補正プリセットスロット1を適用する",
            FsAdjustSlot2 => "補正プリセットスロット2を適用する",
            FsAdjustSlot3 => "補正プリセットスロット3を適用する",
            FsAdjustSlot4 => "補正プリセットスロット4を適用する",
            FsAdjustSlot5 => "補正プリセットスロット5を適用する",
            FsAdjustSlot6 => "補正プリセットスロット6を適用する",
            FsAdjustSlot7 => "補正プリセットスロット7を適用する",
            FsAdjustSlot8 => "補正プリセットスロット8を適用する",
            FsAdjustSlot9 => "補正プリセットスロット9を適用する",
            FsAdjustSlot10 => "補正プリセットスロット10を適用する",
            FsClearAdjust => "現在の画像の補正を解除する",
            FsApplyErase1 => "消しゴムマスクスロット1を現在ページに適用する",
            FsApplyErase2 => "消しゴムマスクスロット2を現在ページに適用する",
            FsApplyConceal1 => "隠蔽マスクスロット1を現在ページに適用する",
            FsApplyConceal2 => "隠蔽マスクスロット2を現在ページに適用する",
            FsDeleteEraseMask => "現在ページの消しゴムマスクを削除する",
            FsDeleteConcealMask => "現在ページの隠蔽マスクを削除する",
            VideoExternalPlayer => "現在の動画を外部プレイヤーで開く",
            VideoCloseFullscreen => "動画フルスクリーンを閉じて一覧へ戻る",
            VideoPlayPause => "動画の再生または一時停止を切り替える",
            VideoSeekStart => "動画の先頭へ移動して再生する",
            VideoSeekBackSmall => "動画を1秒戻す",
            VideoSeekForwardSmall => "動画を1秒進める",
            VideoSeekBackLarge => "動画を30秒戻す",
            VideoSeekForwardLarge => "動画を30秒進める",
            VideoFrameStepBack => "動画を1フレーム戻す",
            VideoFrameStepForward => "動画を1フレーム進める",
            VideoVolumeUp => "動画音量を上げる",
            VideoVolumeDown => "動画音量を下げる",
            VideoNextFile => "次のファイルへ移動する",
            VideoPrevFile => "前のファイルへ移動する",
            VideoMute => "動画のミュートを切り替える",
            VideoLoop => "動画のループ方式を順に切り替える",
            VideoMarkerPrev => "前のチャプター/ブックマーク/ピンへ移動する",
            VideoMarkerNext => "次のチャプター/ブックマーク/ピンへ移動する",
            VideoPin => "現在の再生位置を代表フレームとしてピン留めする",
            VideoPerfOverlay => "動画の性能オーバーレイを切り替える",
            VideoTileMode => "動画タイルモードを切り替える",
            VideoBookmark => "現在の再生位置にブックマークを追加する",
            VideoCapture => "現在の動画フレームをキャプチャ保存する",
            VideoAddToActiveBook => "現在の動画フレームを追加先の本へ追加する",
            VideoCompareToggle => "動画では比較表示キーを何もしない操作として消費する",
            VideoCompareCycle => "動画では比較切り替えキーを何もしない操作として消費する",
            VideoCompareWipe => "動画ではワイプ比較キーを何もしない操作として消費する",
            VideoCompareDiff => "動画では差分比較キーを何もしない操作として消費する",
            EraseConfirm => "消しゴム処理を実行して終了する",
            EraseConfirmPolygon => "消しゴム多角形を確定する",
            EraseUndo => "消しゴム編集を元に戻す",
            EraseDeleteShape => "選択中の消しゴム図形を削除する",
            EraseToolSelect => "選択ツールに切り替える",
            EraseToolBrush => "筆ツールに切り替える",
            EraseToolLasso => "囲みツールに切り替える",
            EraseToolPolygon => "多角形ツールに切り替える",
            EraseToolVLine => "縦線ツールに切り替える",
            EraseToolHLine => "横線ツールに切り替える",
            EraseToolLine => "直線ツールに切り替える",
            EraseToolRect => "矩形ツールに切り替える",
            EraseToolEllipse => "楕円ツールに切り替える",
            ErasePaintMode => "描画モードに切り替える",
            EraseEraseMode => "消去モードに切り替える",
            EraseSpacePan => "押している間だけ画像をパン操作する",
            ConcealExit => "隠蔽加工モードを終了する",
            ConcealConfirmPolygon => "隠蔽加工の多角形を確定する",
            ConcealUndo => "隠蔽加工編集を元に戻す",
            ConcealDeleteShape => "選択中の隠蔽図形を削除する",
            ConcealPixelGrid => "ピクセルグリッド表示を切り替える",
            ConcealTypeCycle => "隠蔽タイプを切り替える",
            ConcealPreset1 => "隠蔽プリセット1を呼び出す",
            ConcealPreset2 => "隠蔽プリセット2を呼び出す",
            ConcealPreset3 => "隠蔽プリセット3を呼び出す",
            ConcealPreset4 => "隠蔽プリセット4を呼び出す",
            ConcealPaintMode => "描画モードに切り替える",
            ConcealEraseMode => "消去モードに切り替える",
            ConcealToolSelect => "選択ツールに切り替える",
            ConcealToolBrush => "筆ツールに切り替える",
            ConcealToolLasso => "囲みツールに切り替える",
            ConcealToolPolygon => "多角形ツールに切り替える",
            ConcealToolLine => "直線ツールに切り替える",
            ConcealToolVLine => "縦線ツールに切り替える",
            ConcealToolHLine => "横線ツールに切り替える",
            ConcealToolRect => "矩形ツールに切り替える",
            ConcealToolEllipse => "楕円ツールに切り替える",
            ConcealSpacePan => "押している間だけ画像をパン操作する",
            CropSpacePan => "押している間だけ画像をパン操作する",
            CropExecute => "切り取りを実行する",
            TextConfirm => "テキスト注釈モードを確定または終了する",
            TextRedo => "テキスト注釈編集をやり直す",
            TextUndo => "テキスト注釈編集を元に戻す",
            TextSpacePan => "押している間だけ画像をパン操作する",
            LaShowSource => "元画像表示を切り替える",
            LaShowMask => "マスク表示を切り替える",
            LaPaintAdd => "補正マスクの追加描画モードにする",
            LaPaintErase => "補正マスクの消去描画モードにする",
            LaToolBrush => "筆ツールに切り替える",
            LaToolEdgeBrush => "境界ブラシツールに切り替える",
            LaToolGapFill => "すき間塗りツールに切り替える",
            LaToolLasso => "囲みツールに切り替える",
            LaToolPolygon => "多角形ツールに切り替える",
            LaToolSelect => "選択ツールに切り替える",
            LaToolLine => "直線ツールに切り替える",
            LaToolVLine => "縦線ツールに切り替える",
            LaToolHLine => "横線ツールに切り替える",
            LaToolRect => "矩形ツールに切り替える",
            LaToolEllipse => "楕円ツールに切り替える",
            LaConfirmPolygon => "補正レイヤーの多角形マスクを確定する",
            LaSpacePan => "押している間だけ画像をパン操作する",
        }
    }

    pub fn context(self) -> KeyContext {
        use KeyAction::*;
        match self {
            GlobalLocalSearch
            | GlobalFavSearch
            | GlobalMetadataSearch
            | GlobalOpenFolder
            | ToggleDetachedViewerMode
            | HelpShowContextShortcuts => KeyContext::Global,
            GridFavoritePrev
            | GridFavoriteNext
            | GridOpenFavorite1
            | GridOpenFavorite2
            | GridOpenFavorite3
            | GridOpenFavorite4
            | GridOpenFavorite5
            | GridOpenFavorite6
            | GridOpenFavorite7
            | GridOpenFavorite8
            | GridOpenFavorite9
            | GridOpenFavorite10
            | GridOpenFavorite11
            | GridOpenFavorite12
            | GridOpenFavorite13
            | GridOpenFavorite14
            | GridOpenFavorite15
            | GridOpenFavorite16
            | GridOpenFavorite17
            | GridOpenFavorite18
            | GridOpenFavorite19
            | GridOpenFavorite20
            | GridOpenDriveC
            | GridOpenDriveD
            | GridOpenDriveE
            | GridOpenDriveF
            | GridOpenDriveG
            | GridOpenDriveH
            | GridOpenDriveI
            | GridOpenDriveJ
            | GridOpenDriveK
            | GridOpenDriveL
            | GridOpenDriveM
            | GridOpenDriveN
            | GridOpenDriveO
            | GridOpenDriveP
            | GridOpenDriveQ
            | GridOpenDriveR
            | GridOpenDriveS
            | GridOpenDriveT
            | GridOpenDriveU
            | GridOpenDriveV
            | GridOpenDriveW
            | GridOpenDriveX
            | GridOpenDriveY
            | GridOpenDriveZ
            | GridOpenLocationDriveList
            | GridOpenLocationReadingHistory
            | GridOpenLocationRating1
            | GridOpenLocationRating2
            | GridOpenLocationRating3
            | GridOpenLocationRating4
            | GridOpenLocationRating5
            | GridOpenLocationBooksRoot
            | GridOpenLocationDesktop
            | GridOpenLocationPictures
            | GridOpenLocationDownloads
            | GridTogglePinnedTag1
            | GridTogglePinnedTag2
            | GridTogglePinnedTag3
            | GridTogglePinnedTag4
            | GridTogglePinnedTag5
            | GridTogglePinnedTag6
            | GridTogglePinnedTag7
            | GridTogglePinnedTag8
            | GridTogglePinnedTag9
            | GridTogglePinnedTag10
            | GridTogglePinnedTag11
            | GridTogglePinnedTag12
            | GridTogglePinnedTag13
            | GridTogglePinnedTag14
            | GridTogglePinnedTag15
            | GridTogglePinnedTag16
            | GridTogglePinnedTag17
            | GridTogglePinnedTag18
            | GridTogglePinnedTag19
            | GridTogglePinnedTag20
            | GridSelectAll
            | GridDeselect
            | GridToggleCheck
            | GridDelete
            | GridOpenSelected
            | GridOpenExternalPlayer
            | GridParentFolder
            | GridHistoryBack
            | GridHistoryForward
            | GridMoveFirst
            | GridMoveLast
            | GridPagePrev
            | GridPageNext
            | GridTreeFolderPrev
            | GridTreeFolderNext
            | GridSiblingFolderPrev
            | GridSiblingFolderNext
            | GridToggleMaximize
            | GridToggleFolderTreePane
            | GridToggleStackMode
            | GridTagApply
            | GridTagView
            | GridRotateCw
            | GridRotateCcw
            | GridPin
            | GridComparePin
            | GridAddToActiveBook
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
            | GridToggleDetailsView
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
            | GridApplyErase1
            | GridApplyErase2
            | GridApplyConceal1
            | GridApplyConceal2
            | GridDeleteEraseMask
            | GridDeleteConcealMask => KeyContext::Grid,
            FsToggleWindowMode | FsBackToList | FsJumpFirst | FsJumpLast | FsCtrlNavPrev
            | FsCtrlNavNext | FsSiblingPrev | FsSiblingNext => KeyContext::FsCommon,
            RatingItem1 | RatingItem2 | RatingItem3 | RatingItem4 | RatingItem5
            | RatingItemClear | RatingContainer1 | RatingContainer2 | RatingContainer3
            | RatingContainer4 | RatingContainer5 | RatingContainerClear => KeyContext::Rating,
            FsContinuousScrollForward
            | FsContinuousScrollBack
            | FsToggleMetadata
            | FsClose
            | FsSpreadShiftLeft
            | FsSpreadShiftRight
            | FsPagePrev
            | FsPageNext
            | FsFixedJumpPrev
            | FsFixedJumpNext
            | FsFixedJumpPrevNoRtl
            | FsFixedJumpNextNoRtl
            | FsStackJumpPrev
            | FsStackJumpNext
            | FsSlideshow
            | FsSpaceCheck
            | FsCapture
            | FsAddToActiveBook
            | FsExport
            | FsCompareToggle
            | FsCompareCycle
            | FsCompareWipe
            | FsCompareDiff
            | FsRotateCw
            | FsRotateCcw
            | FsImageAnalysis
            | FsZoomMode
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
            | FsAiModelAuto
            | FsAiModelRealEsrganX4Plus
            | FsAiModelRealEsrganAnime6B
            | FsAiModelRealCugan4x
            | FsAiModelNmkdSiax4x
            | FsAiModelRealEsrGeneralV3
            | FsDenoiseCycle
            | FsPostFilterNext
            | FsPostFilterPrev
            | FsPostFilterReset
            | FsPostFilterNearest
            | FsPostFilterCrtSimple
            | FsPostFilterCrtFull
            | FsPostFilterCrtArcade
            | FsPostFilterDither1bit
            | FsPostFilterGameBoy
            | FsPostFilterPc98
            | FsPostFilterGameGear
            | FsPostFilterFamicom
            | FsPostFilterMegaDrive
            | FsPostFilterMsx2Plus
            | FsPostFilterSfc
            | FsPostFilterComboFamicomCrt
            | FsPostFilterComboPc98Crt
            | FsPostFilterComboMsx2PlusCrt
            | FsPostFilterComboMegaDriveCrt
            | FsPostFilterComboSfcCrt
            | FsPostFilterSepia
            | FsPostFilterMonoNeutral
            | FsPostFilterMonoCool
            | FsPostFilterMonoWarm
            | FsPostFilterWarmTone
            | FsPostFilterCoolTone
            | FsPostFilterTealOrange
            | FsPostFilterKodakPortra
            | FsPostFilterFujiVelvia
            | FsPostFilterBleachBypass
            | FsPostFilterCrossProcess
            | FsPostFilterVintage
            | FsPostFilterFilmGrain
            | FsPostFilterVignette
            | FsPostFilterLightLeak
            | FsPostFilterSoftFocus
            | FsPostFilterHalftone
            | FsPostFilterOilPaint
            | FsPostFilterSketch
            | FsPostFilterPseudoColor4
            | FsPostFilterPseudoColorSkin
            | FsPostFilterSharpen
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
            VideoExternalPlayer
            | VideoCloseFullscreen
            | VideoPlayPause
            | VideoSeekStart
            | VideoSeekBackSmall
            | VideoSeekForwardSmall
            | VideoSeekBackLarge
            | VideoSeekForwardLarge
            | VideoFrameStepBack
            | VideoFrameStepForward
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
            | VideoAddToActiveBook
            | VideoCompareToggle
            | VideoCompareCycle
            | VideoCompareWipe
            | VideoCompareDiff => KeyContext::FsVideo,
            EraseConfirm | EraseConfirmPolygon | EraseUndo | EraseDeleteShape | EraseToolSelect
            | EraseToolBrush | EraseToolLasso | EraseToolPolygon | EraseToolVLine
            | EraseToolHLine | EraseToolLine | EraseToolRect | EraseToolEllipse
            | ErasePaintMode | EraseEraseMode | EraseSpacePan => KeyContext::Erase,
            ConcealExit
            | ConcealConfirmPolygon
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
            | ConcealSpacePan => KeyContext::Conceal,
            CropExecute | CropSpacePan => KeyContext::Crop,
            TextConfirm | TextRedo | TextUndo | TextSpacePan => KeyContext::Text,
            LaShowSource | LaShowMask | LaPaintAdd | LaPaintErase | LaToolBrush
            | LaToolEdgeBrush | LaToolGapFill | LaToolLasso | LaToolPolygon | LaToolSelect
            | LaToolLine | LaToolVLine | LaToolHLine | LaToolRect | LaToolEllipse
            | LaConfirmPolygon | LaSpacePan => KeyContext::LocalAdjust,
        }
    }

    pub fn trigger(self) -> KeyTrigger {
        use KeyAction::*;
        match self {
            FsLoupeHold => KeyTrigger::ModifierHold,
            EraseSpacePan | ConcealSpacePan | CropSpacePan | TextSpacePan | LaSpacePan
            | FsZoomMode => KeyTrigger::KeyHold,
            GlobalLocalSearch
            | GlobalFavSearch
            | GlobalMetadataSearch
            | GlobalOpenFolder
            | ToggleDetachedViewerMode
            | HelpShowContextShortcuts
            | GridFavoritePrev
            | GridFavoriteNext
            | GridOpenFavorite1
            | GridOpenFavorite2
            | GridOpenFavorite3
            | GridOpenFavorite4
            | GridOpenFavorite5
            | GridOpenFavorite6
            | GridOpenFavorite7
            | GridOpenFavorite8
            | GridOpenFavorite9
            | GridOpenFavorite10
            | GridOpenFavorite11
            | GridOpenFavorite12
            | GridOpenFavorite13
            | GridOpenFavorite14
            | GridOpenFavorite15
            | GridOpenFavorite16
            | GridOpenFavorite17
            | GridOpenFavorite18
            | GridOpenFavorite19
            | GridOpenFavorite20
            | GridOpenDriveC
            | GridOpenDriveD
            | GridOpenDriveE
            | GridOpenDriveF
            | GridOpenDriveG
            | GridOpenDriveH
            | GridOpenDriveI
            | GridOpenDriveJ
            | GridOpenDriveK
            | GridOpenDriveL
            | GridOpenDriveM
            | GridOpenDriveN
            | GridOpenDriveO
            | GridOpenDriveP
            | GridOpenDriveQ
            | GridOpenDriveR
            | GridOpenDriveS
            | GridOpenDriveT
            | GridOpenDriveU
            | GridOpenDriveV
            | GridOpenDriveW
            | GridOpenDriveX
            | GridOpenDriveY
            | GridOpenDriveZ
            | GridOpenLocationDriveList
            | GridOpenLocationReadingHistory
            | GridOpenLocationRating1
            | GridOpenLocationRating2
            | GridOpenLocationRating3
            | GridOpenLocationRating4
            | GridOpenLocationRating5
            | GridOpenLocationBooksRoot
            | GridOpenLocationDesktop
            | GridOpenLocationPictures
            | GridOpenLocationDownloads
            | GridTogglePinnedTag1
            | GridTogglePinnedTag2
            | GridTogglePinnedTag3
            | GridTogglePinnedTag4
            | GridTogglePinnedTag5
            | GridTogglePinnedTag6
            | GridTogglePinnedTag7
            | GridTogglePinnedTag8
            | GridTogglePinnedTag9
            | GridTogglePinnedTag10
            | GridTogglePinnedTag11
            | GridTogglePinnedTag12
            | GridTogglePinnedTag13
            | GridTogglePinnedTag14
            | GridTogglePinnedTag15
            | GridTogglePinnedTag16
            | GridTogglePinnedTag17
            | GridTogglePinnedTag18
            | GridTogglePinnedTag19
            | GridTogglePinnedTag20
            | GridSelectAll
            | GridDeselect
            | GridToggleCheck
            | GridDelete
            | GridOpenSelected
            | GridOpenExternalPlayer
            | GridParentFolder
            | GridHistoryBack
            | GridHistoryForward
            | GridMoveFirst
            | GridMoveLast
            | GridPagePrev
            | GridPageNext
            | GridTreeFolderPrev
            | GridTreeFolderNext
            | GridSiblingFolderPrev
            | GridSiblingFolderNext
            | GridToggleMaximize
            | GridToggleFolderTreePane
            | GridToggleStackMode
            | GridTagApply
            | GridTagView
            | GridRotateCw
            | GridRotateCcw
            | GridPin
            | GridComparePin
            | GridAddToActiveBook
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
            | GridToggleDetailsView
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
            | GridApplyErase1
            | GridApplyErase2
            | GridApplyConceal1
            | GridApplyConceal2
            | GridDeleteEraseMask
            | GridDeleteConcealMask
            | FsToggleMetadata
            | FsClose
            | FsBackToList
            | FsToggleWindowMode
            | FsJumpFirst
            | FsJumpLast
            | FsCtrlNavPrev
            | FsCtrlNavNext
            | FsSiblingPrev
            | FsSiblingNext
            | FsPagePrev
            | FsPageNext
            | FsFixedJumpPrev
            | FsFixedJumpNext
            | FsFixedJumpPrevNoRtl
            | FsFixedJumpNextNoRtl
            | FsStackJumpPrev
            | FsStackJumpNext
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
            | FsAddToActiveBook
            | FsExport
            | FsCompareToggle
            | FsCompareCycle
            | FsCompareWipe
            | FsCompareDiff
            | FsRotateCw
            | FsRotateCcw
            | FsImageAnalysis
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
            | FsAiModelAuto
            | FsAiModelRealEsrganX4Plus
            | FsAiModelRealEsrganAnime6B
            | FsAiModelRealCugan4x
            | FsAiModelNmkdSiax4x
            | FsAiModelRealEsrGeneralV3
            | FsDenoiseCycle
            | FsPostFilterNext
            | FsPostFilterPrev
            | FsPostFilterReset
            | FsPostFilterNearest
            | FsPostFilterCrtSimple
            | FsPostFilterCrtFull
            | FsPostFilterCrtArcade
            | FsPostFilterDither1bit
            | FsPostFilterGameBoy
            | FsPostFilterPc98
            | FsPostFilterGameGear
            | FsPostFilterFamicom
            | FsPostFilterMegaDrive
            | FsPostFilterMsx2Plus
            | FsPostFilterSfc
            | FsPostFilterComboFamicomCrt
            | FsPostFilterComboPc98Crt
            | FsPostFilterComboMsx2PlusCrt
            | FsPostFilterComboMegaDriveCrt
            | FsPostFilterComboSfcCrt
            | FsPostFilterSepia
            | FsPostFilterMonoNeutral
            | FsPostFilterMonoCool
            | FsPostFilterMonoWarm
            | FsPostFilterWarmTone
            | FsPostFilterCoolTone
            | FsPostFilterTealOrange
            | FsPostFilterKodakPortra
            | FsPostFilterFujiVelvia
            | FsPostFilterBleachBypass
            | FsPostFilterCrossProcess
            | FsPostFilterVintage
            | FsPostFilterFilmGrain
            | FsPostFilterVignette
            | FsPostFilterLightLeak
            | FsPostFilterSoftFocus
            | FsPostFilterHalftone
            | FsPostFilterOilPaint
            | FsPostFilterSketch
            | FsPostFilterPseudoColor4
            | FsPostFilterPseudoColorSkin
            | FsPostFilterSharpen
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
            | VideoCloseFullscreen
            | VideoPlayPause
            | VideoSeekStart
            | VideoSeekBackSmall
            | VideoSeekForwardSmall
            | VideoSeekBackLarge
            | VideoSeekForwardLarge
            | VideoFrameStepBack
            | VideoFrameStepForward
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
            | VideoAddToActiveBook
            | VideoCompareToggle
            | VideoCompareCycle
            | VideoCompareWipe
            | VideoCompareDiff
            | EraseConfirm
            | EraseConfirmPolygon
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
            | ConcealConfirmPolygon
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
            | LaToolEllipse
            | LaConfirmPolygon => KeyTrigger::Press,
        }
    }

    pub fn default_chords(self) -> ChordList {
        use KeyAction::*;
        use KeySlot::*;
        match self {
            GlobalLocalSearch => ChordList::one(Chord::ctrl(F)),
            GlobalFavSearch => ChordList::one(Chord::ctrl(S)),
            GlobalMetadataSearch => ChordList::one(Chord::ctrl(G)),
            GlobalOpenFolder => ChordList::one(Chord::ctrl(O)),
            ToggleDetachedViewerMode => ChordList::one(Chord::key(F12)),
            HelpShowContextShortcuts => ChordList::one(Chord::shift(Slash)),
            GridFavoritePrev
            | GridFavoriteNext
            | GridOpenFavorite1
            | GridOpenFavorite2
            | GridOpenFavorite3
            | GridOpenFavorite4
            | GridOpenFavorite5
            | GridOpenFavorite6
            | GridOpenFavorite7
            | GridOpenFavorite8
            | GridOpenFavorite9
            | GridOpenFavorite10
            | GridOpenFavorite11
            | GridOpenFavorite12
            | GridOpenFavorite13
            | GridOpenFavorite14
            | GridOpenFavorite15
            | GridOpenFavorite16
            | GridOpenFavorite17
            | GridOpenFavorite18
            | GridOpenFavorite19
            | GridOpenFavorite20
            | GridOpenDriveC
            | GridOpenDriveD
            | GridOpenDriveE
            | GridOpenDriveF
            | GridOpenDriveG
            | GridOpenDriveH
            | GridOpenDriveI
            | GridOpenDriveJ
            | GridOpenDriveK
            | GridOpenDriveL
            | GridOpenDriveM
            | GridOpenDriveN
            | GridOpenDriveO
            | GridOpenDriveP
            | GridOpenDriveQ
            | GridOpenDriveR
            | GridOpenDriveS
            | GridOpenDriveT
            | GridOpenDriveU
            | GridOpenDriveV
            | GridOpenDriveW
            | GridOpenDriveX
            | GridOpenDriveY
            | GridOpenDriveZ
            | GridOpenLocationDriveList
            | GridOpenLocationReadingHistory
            | GridOpenLocationRating1
            | GridOpenLocationRating2
            | GridOpenLocationRating3
            | GridOpenLocationRating4
            | GridOpenLocationRating5
            | GridOpenLocationBooksRoot
            | GridOpenLocationDesktop
            | GridOpenLocationPictures
            | GridOpenLocationDownloads
            | GridTogglePinnedTag1
            | GridTogglePinnedTag2
            | GridTogglePinnedTag3
            | GridTogglePinnedTag4
            | GridTogglePinnedTag5
            | GridTogglePinnedTag6
            | GridTogglePinnedTag7
            | GridTogglePinnedTag8
            | GridTogglePinnedTag9
            | GridTogglePinnedTag10
            | GridTogglePinnedTag11
            | GridTogglePinnedTag12
            | GridTogglePinnedTag13
            | GridTogglePinnedTag14
            | GridTogglePinnedTag15
            | GridTogglePinnedTag16
            | GridTogglePinnedTag17
            | GridTogglePinnedTag18
            | GridTogglePinnedTag19
            | GridTogglePinnedTag20 => ChordList::EMPTY,
            GridSelectAll => ChordList::one(Chord::ctrl(A)),
            GridDeselect => ChordList::two(Chord::ctrl(D), Chord::ctrl_shift(A)),
            GridToggleCheck => ChordList::one(Chord::key(Space)),
            GridDelete => ChordList::one(Chord::key(Delete)),
            GridOpenSelected => ChordList::one(Chord::key(Enter)),
            GridOpenExternalPlayer => ChordList::one(Chord::shift(Enter)),
            GridParentFolder => ChordList::two(Chord::key(Backspace), Chord::alt(Up)),
            GridHistoryBack => ChordList::one(Chord::alt(Left)),
            GridHistoryForward => ChordList::one(Chord::alt(Right)),
            GridMoveFirst => ChordList::one(Chord::key(Home)),
            GridMoveLast => ChordList::one(Chord::key(End)),
            GridPagePrev => ChordList::one(Chord::key(PageUp)),
            GridPageNext => ChordList::one(Chord::key(PageDown)),
            GridTreeFolderPrev => ChordList::one(Chord::ctrl(Up)),
            GridTreeFolderNext => ChordList::one(Chord::ctrl(Down)),
            GridSiblingFolderPrev => ChordList::one(Chord::ctrl(PageUp)),
            GridSiblingFolderNext => ChordList::one(Chord::ctrl(PageDown)),
            GridToggleMaximize => ChordList::one(Chord::key(F11)),
            GridToggleFolderTreePane => ChordList::one(Chord::key(F)),
            GridToggleStackMode => ChordList::EMPTY,
            GridTagApply => ChordList::one(Chord::key(T)),
            GridTagView => ChordList::one(Chord::ctrl(T)),
            GridRotateCw => ChordList::one(Chord::key(R)),
            GridRotateCcw => ChordList::one(Chord::key(L)),
            GridPin => ChordList::one(Chord::key(P)),
            GridComparePin => ChordList::one(Chord::key(X)),
            GridAddToActiveBook => ChordList::one(Chord::ctrl(B)),
            GridColumnCount1 => alt_digit_pair(Num1, Numpad1),
            GridColumnCount2 => alt_digit_pair(Num2, Numpad2),
            GridColumnCount3 => alt_digit_pair(Num3, Numpad3),
            GridColumnCount4 => alt_digit_pair(Num4, Numpad4),
            GridColumnCount5 => alt_digit_pair(Num5, Numpad5),
            GridColumnCount6 => alt_digit_pair(Num6, Numpad6),
            GridColumnCount7 => alt_digit_pair(Num7, Numpad7),
            GridColumnCount8 => alt_digit_pair(Num8, Numpad8),
            GridColumnCount9 => alt_digit_pair(Num9, Numpad9),
            GridColumnCount10 => alt_digit_pair(Num0, Numpad0),
            GridToggleDetailsView => ChordList::one(Chord::alt(Minus)),
            GridAdjustSlot1 => ctrl_digit_pair(Num1, Numpad1),
            GridAdjustSlot2 => ctrl_digit_pair(Num2, Numpad2),
            GridAdjustSlot3 => ctrl_digit_pair(Num3, Numpad3),
            GridAdjustSlot4 => ctrl_digit_pair(Num4, Numpad4),
            GridAdjustSlot5 => ctrl_digit_pair(Num5, Numpad5),
            GridAdjustSlot6 => ctrl_digit_pair(Num6, Numpad6),
            GridAdjustSlot7 => ctrl_digit_pair(Num7, Numpad7),
            GridAdjustSlot8 => ctrl_digit_pair(Num8, Numpad8),
            GridAdjustSlot9 => ctrl_digit_pair(Num9, Numpad9),
            GridAdjustSlot10 => ctrl_digit_pair(Num0, Numpad0),
            GridClearAdjust => ChordList::two(Chord::ctrl(Backspace), Chord::key(Q)),
            GridApplyErase1 => ChordList::one(Chord::key(F7)),
            GridApplyErase2 => ChordList::one(Chord::key(F8)),
            GridApplyConceal1 => ChordList::one(Chord::key(F9)),
            GridApplyConceal2 => ChordList::one(Chord::key(F10)),
            GridDeleteEraseMask => ChordList::two(Chord::shift(F7), Chord::shift(F8)),
            GridDeleteConcealMask => ChordList::two(Chord::shift(F9), Chord::shift(F10)),
            FsToggleMetadata => ChordList::two(Chord::key(I), Chord::key(Tab)),
            FsClose => ChordList::one(Chord::key(Enter)),
            FsBackToList => ChordList::one(Chord::key(Backspace)),
            FsToggleWindowMode => ChordList::one(Chord::key(F11)),
            FsJumpFirst => ChordList::one(Chord::key(Home)),
            FsJumpLast => ChordList::one(Chord::key(End)),
            FsCtrlNavPrev => ChordList::one(Chord::ctrl(Up)),
            FsCtrlNavNext => ChordList::one(Chord::ctrl(Down)),
            FsSiblingPrev => ChordList::one(Chord::ctrl(PageUp)),
            FsSiblingNext => ChordList::one(Chord::ctrl(PageDown)),
            FsPagePrev | FsPageNext => ChordList::EMPTY,
            FsFixedJumpPrev => ChordList::one(Chord::shift(Left)),
            FsFixedJumpNext => ChordList::one(Chord::shift(Right)),
            FsFixedJumpPrevNoRtl => ChordList::one(Chord::key(PageUp)),
            FsFixedJumpNextNoRtl => ChordList::one(Chord::key(PageDown)),
            FsStackJumpPrev => ChordList::one(Chord::shift(Up)),
            FsStackJumpNext => ChordList::one(Chord::shift(Down)),
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
            FsAddToActiveBook => ChordList::one(Chord::ctrl(B)),
            FsExport => ChordList::one(Chord::ctrl(E)),
            FsCompareToggle => ChordList::one(Chord::key(X)),
            FsCompareCycle => ChordList::one(Chord::key(C)),
            FsCompareWipe => ChordList::one(Chord::shift(C)),
            FsCompareDiff => ChordList::one(Chord::alt(C)),
            FsRotateCw => ChordList::one(Chord::key(R)),
            FsRotateCcw => ChordList::one(Chord::key(L)),
            // Z は ZipPla 風の全画面ズームモード (KeyHold) に明け渡し、画像分析モードは
            // Shift+Z へ移動・アクション名も FsImageAnalysis へ改名した (v2.0.0、旧 `FsAnalysis`
            // のカスタム割当は未知アクションとして無視され、新既定へ移行する)。
            FsImageAnalysis => ChordList::one(Chord::shift(Z)),
            FsZoomMode => ChordList::one(Chord::key(Z)),
            FsPanorama => ChordList::one(Chord::key(V)),
            FsPixelGrid => ChordList::one(Chord::key(G)),
            FsLoupeLockToggle => ChordList::one(Chord::key(M)),
            FsLoupeHold => ChordList::one(Chord::modifier(ModKind::Shift)),
            FsEraseMode => ChordList::one(Chord::key(E)),
            FsConcealMode => ChordList::one(Chord::ctrl(M)),
            FsTextMode => ChordList::one(Chord::ctrl(T)),
            FsBgCycle => ChordList::one(Chord::key(B)),
            FsPin => ChordList::one(Chord::key(P)),
            FsSpreadSingle => digit_pair(Num1, Numpad1),
            FsSpreadLtr => digit_pair(Num2, Numpad2),
            FsSpreadLtrCover => digit_pair(Num3, Numpad3),
            FsSpreadRtl => digit_pair(Num4, Numpad4),
            FsSpreadRtlCover => digit_pair(Num5, Numpad5),
            FsReadingFlowCycle => digit_pair(Num6, Numpad6),
            FsReadingDirectionToggle => digit_pair(Num7, Numpad7),
            FsFitModeCycle => digit_pair(Num0, Numpad0),
            FsAiModelNext => ChordList::one(Chord::key(U)),
            FsAiModelPrev => ChordList::one(Chord::shift(U)),
            FsAiModelReset => ChordList::one(Chord::alt(U)),
            FsAiModelAuto
            | FsAiModelRealEsrganX4Plus
            | FsAiModelRealEsrganAnime6B
            | FsAiModelRealCugan4x
            | FsAiModelNmkdSiax4x
            | FsAiModelRealEsrGeneralV3 => ChordList::EMPTY,
            FsDenoiseCycle => ChordList::one(Chord::key(N)),
            FsPostFilterNext => ChordList::one(Chord::key(T)),
            FsPostFilterPrev => ChordList::one(Chord::shift(T)),
            FsPostFilterReset => ChordList::one(Chord::alt(T)),
            FsPostFilterNearest
            | FsPostFilterCrtSimple
            | FsPostFilterCrtFull
            | FsPostFilterCrtArcade
            | FsPostFilterDither1bit
            | FsPostFilterGameBoy
            | FsPostFilterPc98
            | FsPostFilterGameGear
            | FsPostFilterFamicom
            | FsPostFilterMegaDrive
            | FsPostFilterMsx2Plus
            | FsPostFilterSfc
            | FsPostFilterComboFamicomCrt
            | FsPostFilterComboPc98Crt
            | FsPostFilterComboMsx2PlusCrt
            | FsPostFilterComboMegaDriveCrt
            | FsPostFilterComboSfcCrt
            | FsPostFilterSepia
            | FsPostFilterMonoNeutral
            | FsPostFilterMonoCool
            | FsPostFilterMonoWarm
            | FsPostFilterWarmTone
            | FsPostFilterCoolTone
            | FsPostFilterTealOrange
            | FsPostFilterKodakPortra
            | FsPostFilterFujiVelvia
            | FsPostFilterBleachBypass
            | FsPostFilterCrossProcess
            | FsPostFilterVintage
            | FsPostFilterFilmGrain
            | FsPostFilterVignette
            | FsPostFilterLightLeak
            | FsPostFilterSoftFocus
            | FsPostFilterHalftone
            | FsPostFilterOilPaint
            | FsPostFilterSketch
            | FsPostFilterPseudoColor4
            | FsPostFilterPseudoColorSkin
            | FsPostFilterSharpen => ChordList::EMPTY,
            FsAdjustSlot1 => ctrl_digit_pair(Num1, Numpad1),
            FsAdjustSlot2 => ctrl_digit_pair(Num2, Numpad2),
            FsAdjustSlot3 => ctrl_digit_pair(Num3, Numpad3),
            FsAdjustSlot4 => ctrl_digit_pair(Num4, Numpad4),
            FsAdjustSlot5 => ctrl_digit_pair(Num5, Numpad5),
            FsAdjustSlot6 => ctrl_digit_pair(Num6, Numpad6),
            FsAdjustSlot7 => ctrl_digit_pair(Num7, Numpad7),
            FsAdjustSlot8 => ctrl_digit_pair(Num8, Numpad8),
            FsAdjustSlot9 => ctrl_digit_pair(Num9, Numpad9),
            FsAdjustSlot10 => ctrl_digit_pair(Num0, Numpad0),
            FsClearAdjust => ChordList::two(Chord::ctrl(Backspace), Chord::key(Q)),
            FsApplyErase1 => ChordList::one(Chord::key(F7)),
            FsApplyErase2 => ChordList::one(Chord::key(F8)),
            FsApplyConceal1 => ChordList::one(Chord::key(F9)),
            FsApplyConceal2 => ChordList::one(Chord::key(F10)),
            FsDeleteEraseMask => ChordList::two(Chord::shift(F7), Chord::shift(F8)),
            FsDeleteConcealMask => ChordList::two(Chord::shift(F9), Chord::shift(F10)),
            VideoExternalPlayer => ChordList::one(Chord::shift(Enter)),
            VideoCloseFullscreen => ChordList::EMPTY,
            VideoPlayPause => ChordList::two(Chord::key(Space), Chord::key(Enter)),
            VideoSeekStart => ChordList::one(Chord::key(W)),
            VideoSeekBackSmall => ChordList::one(Chord::shift(Left)),
            VideoSeekForwardSmall => ChordList::one(Chord::shift(Right)),
            VideoSeekBackLarge => ChordList::one(Chord::ctrl(Left)),
            VideoSeekForwardLarge => ChordList::one(Chord::ctrl(Right)),
            VideoFrameStepBack => ChordList::one(Chord::ctrl_shift(Left)),
            VideoFrameStepForward => ChordList::one(Chord::ctrl_shift(Right)),
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
            VideoAddToActiveBook => ChordList::one(Chord::ctrl(B)),
            VideoCompareToggle => ChordList::one(Chord::key(X)),
            VideoCompareCycle => ChordList::one(Chord::key(C)),
            VideoCompareWipe => ChordList::one(Chord::shift(C)),
            VideoCompareDiff => ChordList::one(Chord::alt(C)),
            EraseConfirm => ChordList::one(Chord::key(E)),
            EraseConfirmPolygon => ChordList::one(Chord::key(Enter)),
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
            ConcealConfirmPolygon => ChordList::one(Chord::key(Enter)),
            ConcealUndo => ChordList::one(Chord::ctrl(Z)),
            ConcealDeleteShape => ChordList::one(Chord::key(Delete)),
            ConcealPixelGrid => ChordList::one(Chord::key(G)),
            ConcealTypeCycle => ChordList::one(Chord::key(T)),
            ConcealPreset1 => digit_pair(Num1, Numpad1),
            ConcealPreset2 => digit_pair(Num2, Numpad2),
            ConcealPreset3 => digit_pair(Num3, Numpad3),
            ConcealPreset4 => digit_pair(Num4, Numpad4),
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
            LaConfirmPolygon => ChordList::one(Chord::key(Enter)),
            LaSpacePan => ChordList::one(Chord::key(Space)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    overrides: HashMap<KeyAction, Vec<Chord>>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeymapSettings {
    #[serde(default)]
    pub overrides: Vec<KeyBindingOverride>,
    #[serde(default)]
    pub legacy_ini_migration_done: bool,
    #[serde(default)]
    pub legacy_ini_backup: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyBindingOverride {
    pub action: String,
    #[serde(default)]
    pub chords: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyKeymapIniImport {
    pub changed: bool,
    pub imported: bool,
    pub backup_path: Option<PathBuf>,
    pub warnings: Vec<String>,
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

pub const GRID_ACTIVE_SCOPES: &[CommandScope] =
    &[KeyContext::Global, KeyContext::Grid, KeyContext::Rating];

pub const FS_IMAGE_ACTIVE_SCOPES: &[CommandScope] = &[
    KeyContext::Global,
    KeyContext::FsCommon,
    KeyContext::Rating,
    KeyContext::FsImage,
];

pub const FS_VIDEO_ACTIVE_SCOPES: &[CommandScope] = &[
    KeyContext::Global,
    KeyContext::FsCommon,
    KeyContext::Rating,
    KeyContext::FsVideo,
];

const ACTIVE_SCOPE_SETS: &[&[CommandScope]] = &[
    GRID_ACTIVE_SCOPES,
    FS_IMAGE_ACTIVE_SCOPES,
    FS_VIDEO_ACTIVE_SCOPES,
    &[KeyContext::Erase],
    &[KeyContext::Conceal],
    &[KeyContext::Crop],
    &[KeyContext::Text],
    &[KeyContext::LocalAdjust],
];

pub fn command_scopes_overlap(a: CommandScope, b: CommandScope) -> bool {
    a == b
        || ACTIVE_SCOPE_SETS
            .iter()
            .any(|set| set.contains(&a) && set.contains(&b))
}

fn is_overlay_edit_scope(scope: CommandScope) -> bool {
    matches!(
        scope,
        KeyContext::Erase
            | KeyContext::Conceal
            | KeyContext::Crop
            | KeyContext::Text
            | KeyContext::LocalAdjust
    )
}

fn help_context_shortcuts_overlaps_scope(action: KeyAction, scope: CommandScope) -> bool {
    action == KeyAction::HelpShowContextShortcuts && is_overlay_edit_scope(scope)
}

fn actions_can_overlap(first: KeyAction, second: KeyAction) -> bool {
    command_scopes_overlap(first.context(), second.context())
        || help_context_shortcuts_overlaps_scope(first, second.context())
        || help_context_shortcuts_overlaps_scope(second, first.context())
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BindingConflictKind {
    Hard,
    ActiveOverlap,
    TriggerMismatch,
    Reserved,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BindingConflict {
    pub kind: BindingConflictKind,
    pub chord: Chord,
    pub action: KeyAction,
    pub other_action: Option<KeyAction>,
    pub scope: CommandScope,
    pub other_scope: CommandScope,
    pub trigger: KeyTrigger,
    pub other_trigger: KeyTrigger,
    pub reserved_name: Option<&'static str>,
}

impl BindingConflict {
    fn warning(self) -> String {
        let chord = self.chord.display_name();
        match self.kind {
            BindingConflictKind::Reserved => format!(
                "binding warning: '{}' uses reserved shortcut {} ({}) in overlapping scope [{}]",
                self.action.ini_name(),
                chord,
                self.reserved_name.unwrap_or("reserved input"),
                self.scope.ini_name()
            ),
            BindingConflictKind::Hard
            | BindingConflictKind::ActiveOverlap
            | BindingConflictKind::TriggerMismatch => {
                let Some(other) = self.other_action else {
                    return format!(
                        "binding warning: '{}' uses {} in overlapping scope [{}]",
                        self.action.ini_name(),
                        chord,
                        self.scope.ini_name()
                    );
                };
                format!(
                    "binding warning: '{}' and '{}' both use {} ({:?}; [{}]/{:?} vs [{}]/{:?})",
                    self.action.ini_name(),
                    other.ini_name(),
                    chord,
                    self.kind,
                    self.scope.ini_name(),
                    self.trigger,
                    self.other_scope.ini_name(),
                    self.other_trigger
                )
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct EffectiveBinding {
    action: KeyAction,
    scope: CommandScope,
    trigger: KeyTrigger,
    chord: Chord,
    customized: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct ReservedBinding {
    scope: CommandScope,
    trigger: KeyTrigger,
    chord: Chord,
    name: &'static str,
}

const RESERVED_BINDINGS: &[ReservedBinding] = &[
    ReservedBinding {
        scope: KeyContext::Global,
        trigger: KeyTrigger::Press,
        chord: Chord::key(KeyName::Esc),
        name: "Escape navigation / cancel",
    },
    ReservedBinding {
        scope: KeyContext::Global,
        trigger: KeyTrigger::Press,
        chord: Chord::key(KeyName::Left),
        name: "plain arrow navigation",
    },
    ReservedBinding {
        scope: KeyContext::Global,
        trigger: KeyTrigger::Press,
        chord: Chord::key(KeyName::Right),
        name: "plain arrow navigation",
    },
    ReservedBinding {
        scope: KeyContext::Global,
        trigger: KeyTrigger::Press,
        chord: Chord::key(KeyName::Up),
        name: "plain arrow navigation",
    },
    ReservedBinding {
        scope: KeyContext::Global,
        trigger: KeyTrigger::Press,
        chord: Chord::key(KeyName::Down),
        name: "plain arrow navigation",
    },
    ReservedBinding {
        scope: KeyContext::Grid,
        trigger: KeyTrigger::Press,
        chord: Chord::shift(KeyName::Left),
        name: "grid range selection",
    },
    ReservedBinding {
        scope: KeyContext::Grid,
        trigger: KeyTrigger::Press,
        chord: Chord::shift(KeyName::Right),
        name: "grid range selection",
    },
    ReservedBinding {
        scope: KeyContext::Grid,
        trigger: KeyTrigger::Press,
        chord: Chord::shift(KeyName::Up),
        name: "grid range selection",
    },
    ReservedBinding {
        scope: KeyContext::Grid,
        trigger: KeyTrigger::Press,
        chord: Chord::shift(KeyName::Down),
        name: "grid range selection",
    },
];

fn reserved_binding_overlaps_scope(
    binding_scope: CommandScope,
    reserved_scope: CommandScope,
) -> bool {
    reserved_scope == KeyContext::Global || command_scopes_overlap(binding_scope, reserved_scope)
}

impl Keymap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_settings(settings: &KeymapSettings) -> Self {
        let mut warnings = Vec::new();
        let mut overrides = HashMap::new();
        for binding in &settings.overrides {
            let action_name = binding.action.trim();
            let Some(action) = KeyAction::parse_ini_name(action_name) else {
                warnings.push(format!("settings: unknown key action '{}'", binding.action));
                continue;
            };
            let chords = match parse_setting_chords(action, &binding.chords, &mut warnings) {
                Some(chords) => chords,
                None => continue,
            };
            if overrides.insert(action, chords).is_some() {
                warnings.push(format!(
                    "settings: duplicate override for '{}', using the last value",
                    action.ini_name()
                ));
            }
        }
        let mut keymap = Self {
            overrides,
            warnings,
        };
        keymap.append_binding_conflict_warnings();
        keymap
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

        let mut keymap = Self {
            overrides,
            warnings,
        };
        keymap.append_binding_conflict_warnings();
        keymap
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn override_chords(&self, action: KeyAction) -> Option<&[Chord]> {
        self.overrides.get(&action).map(Vec::as_slice)
    }

    pub fn effective_chords(&self, action: KeyAction) -> Vec<Chord> {
        self.overrides
            .get(&action)
            .cloned()
            .unwrap_or_else(|| action.default_chords().iter().collect())
    }

    pub fn first_chord_label(&self, action: KeyAction) -> Option<String> {
        self.effective_chords(action)
            .into_iter()
            .next()
            .map(|chord| chord.display_name())
    }

    pub fn first_chord_action_label(&self, label: &str, action: KeyAction) -> String {
        match self.first_chord_label(action) {
            Some(key_label) => format!("{label} ({key_label})"),
            None => label.to_owned(),
        }
    }

    pub fn chord_labels(&self, action: KeyAction) -> Vec<String> {
        self.effective_chords(action)
            .into_iter()
            .map(|chord| chord.display_name())
            .collect()
    }

    pub fn consume_context_shortcuts_help_action(&self, ctx: &egui::Context) -> bool {
        let chords = self.effective_chords(KeyAction::HelpShowContextShortcuts);
        let mut consumed = false;
        for chord in chords.iter().copied() {
            if self.consume_chord_no_repeat(ctx, chord) {
                consumed = true;
            }
        }
        consumed
    }

    pub fn context_shortcuts_help_label(&self) -> String {
        let labels = self.chord_labels(KeyAction::HelpShowContextShortcuts);
        if labels.is_empty() {
            "未設定".to_string()
        } else {
            labels.join(" / ")
        }
    }

    pub fn menu_command_label(&self, id: MenuCommandId) -> String {
        let Some(spec) = menu_command_spec(id) else {
            return id.stable_name().to_string();
        };
        if let Some(action) = spec.action {
            self.first_chord_action_label(spec.label, action)
        } else {
            spec.label.to_string()
        }
    }

    pub fn command_display_rows_for_active_scopes(
        &self,
        active_scopes: &[CommandScope],
        include_unassigned: bool,
    ) -> Vec<CommandDisplayRow> {
        command_catalog()
            .filter(|spec| active_scopes.contains(&spec.scope))
            .filter(|spec| spec.action != KeyAction::HelpShowContextShortcuts)
            .filter(|spec| spec.action.is_user_facing())
            .filter_map(|spec| {
                let shortcut_labels = self.chord_labels(spec.action);
                if shortcut_labels.is_empty() && !include_unassigned {
                    return None;
                }
                Some(CommandDisplayRow {
                    spec,
                    shortcut_labels,
                })
            })
            .collect()
    }

    pub fn first_chord_bracket_label(&self, label: &str, action: KeyAction) -> String {
        match self.first_chord_label(action) {
            Some(key_label) => format!("{label} [{key_label}]"),
            None => label.to_owned(),
        }
    }

    pub fn chord_list_bracket_label(&self, label: &str, action: KeyAction) -> String {
        let key_labels = self.chord_labels(action);
        if key_labels.is_empty() {
            label.to_owned()
        } else {
            format!("{label} [{}]", key_labels.join(" / "))
        }
    }

    pub fn compact_single_key_label(&self, action: KeyAction) -> Option<&'static str> {
        self.effective_chords(action)
            .into_iter()
            .next()
            .and_then(|chord| {
                if chord.ctrl || chord.shift || chord.alt {
                    None
                } else {
                    chord.key.map(KeyName::display_name)
                }
            })
    }

    pub fn compact_action_label(&self, label: &str, action: KeyAction) -> String {
        match self.compact_single_key_label(action) {
            Some(key_label) => format!("{label} [{key_label}]"),
            None => label.to_owned(),
        }
    }

    pub fn resolve_first_action_for_chord(
        &self,
        chord: Chord,
        active_scopes: &[CommandScope],
        priority: &[KeyAction],
    ) -> Option<KeyAction> {
        priority.iter().copied().find(|action| {
            action.trigger() == KeyTrigger::Press
                && active_scopes.contains(&action.context())
                && self.action_has_chord(*action, chord)
        })
    }

    pub fn consume_first_action(
        &self,
        ctx: &egui::Context,
        active_scopes: &[CommandScope],
        priority: &[KeyAction],
    ) -> Option<KeyAction> {
        priority.iter().copied().find(|action| {
            action.trigger() == KeyTrigger::Press
                && active_scopes.contains(&action.context())
                && self.consume_action(ctx, *action)
        })
    }

    fn action_has_chord(&self, action: KeyAction, chord: Chord) -> bool {
        self.effective_chords(action)
            .into_iter()
            .any(|c| c == chord)
    }

    pub fn binding_conflicts(&self) -> Vec<BindingConflict> {
        let bindings = self.effective_bindings();
        let mut conflicts = Vec::new();

        for (i, first) in bindings.iter().copied().enumerate() {
            for second in bindings.iter().copied().skip(i + 1) {
                if first.chord != second.chord
                    || first.action == second.action
                    || !first.action.is_user_facing()
                    || !second.action.is_user_facing()
                    || !actions_can_overlap(first.action, second.action)
                    || !(first.customized || second.customized)
                {
                    continue;
                }
                let kind = if first.trigger != second.trigger {
                    BindingConflictKind::TriggerMismatch
                } else if first.scope == second.scope {
                    BindingConflictKind::Hard
                } else {
                    BindingConflictKind::ActiveOverlap
                };
                conflicts.push(BindingConflict {
                    kind,
                    chord: first.chord,
                    action: first.action,
                    other_action: Some(second.action),
                    scope: first.scope,
                    other_scope: second.scope,
                    trigger: first.trigger,
                    other_trigger: second.trigger,
                    reserved_name: None,
                });
            }
        }

        for binding in bindings
            .iter()
            .copied()
            .filter(|binding| binding.customized && binding.action.is_user_facing())
        {
            for reserved in RESERVED_BINDINGS {
                if binding.chord == reserved.chord
                    && reserved_binding_overlaps_scope(binding.scope, reserved.scope)
                {
                    conflicts.push(BindingConflict {
                        kind: BindingConflictKind::Reserved,
                        chord: binding.chord,
                        action: binding.action,
                        other_action: None,
                        scope: binding.scope,
                        other_scope: reserved.scope,
                        trigger: binding.trigger,
                        other_trigger: reserved.trigger,
                        reserved_name: Some(reserved.name),
                    });
                }
            }
        }

        conflicts
    }

    fn append_binding_conflict_warnings(&mut self) {
        for conflict in self.binding_conflicts() {
            self.warnings.push(conflict.warning());
        }
    }

    fn effective_bindings(&self) -> Vec<EffectiveBinding> {
        let mut bindings = Vec::new();
        for spec in command_catalog() {
            let customized = self.overrides.contains_key(&spec.action);
            for chord in self.effective_chords(spec.action) {
                bindings.push(EffectiveBinding {
                    action: spec.action,
                    scope: spec.scope,
                    trigger: spec.trigger,
                    chord,
                    customized,
                });
            }
        }
        bindings
    }

    pub fn rating_key_action(container: bool, stars: u8) -> Option<KeyAction> {
        rating_actions(container)
            .iter()
            .copied()
            .find_map(|(action, action_stars)| (action_stars == stars).then_some(action))
    }

    pub fn first_rating_chord_label(&self, container: bool, stars: u8) -> Option<String> {
        Self::rating_key_action(container, stars).and_then(|action| self.first_chord_label(action))
    }

    pub fn rating_chord_summary_label(&self, container: bool) -> Option<String> {
        let labels = rating_actions(container)
            .iter()
            .copied()
            .filter_map(|(action, stars)| {
                self.first_chord_label(action).map(|label| (stars, label))
            })
            .collect::<Vec<_>>();
        if labels.is_empty() {
            return None;
        }

        let expected = if container {
            [
                (1, "Shift+F1"),
                (2, "Shift+F2"),
                (3, "Shift+F3"),
                (4, "Shift+F4"),
                (5, "Shift+F5"),
                (0, "Shift+F6"),
            ]
        } else {
            [
                (1, "F1"),
                (2, "F2"),
                (3, "F3"),
                (4, "F4"),
                (5, "F5"),
                (0, "F6"),
            ]
        };
        if labels.len() == expected.len()
            && labels.iter().zip(expected).all(
                |((stars, label), (expected_stars, expected_label))| {
                    *stars == expected_stars && label == expected_label
                },
            )
        {
            let prefix = if container { "Shift+" } else { "" };
            return Some(format!("{prefix}F1〜F6"));
        }

        Some(
            labels
                .into_iter()
                .map(|(stars, label)| {
                    if stars == 0 {
                        format!("解除:{label}")
                    } else {
                        format!("{stars}:{label}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" / "),
        )
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
                    #[cfg(windows)]
                    crate::key_debug::record_consumed_action(
                        action,
                        action.context(),
                        chord,
                        "consume_action",
                    );
                    return true;
                }
            }
            return false;
        }
        for chord in action.default_chords().iter() {
            if self.consume_chord(ctx, chord) {
                #[cfg(windows)]
                crate::key_debug::record_consumed_action(
                    action,
                    action.context(),
                    chord,
                    "consume_action",
                );
                return true;
            }
        }
        false
    }

    pub fn consume_action_no_repeat(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            for chord in chords.iter().copied() {
                if self.consume_chord_no_repeat(ctx, chord) {
                    #[cfg(windows)]
                    crate::key_debug::record_consumed_action(
                        action,
                        action.context(),
                        chord,
                        "consume_no_repeat",
                    );
                    return true;
                }
            }
            return false;
        }
        for chord in action.default_chords().iter() {
            if self.consume_chord_no_repeat(ctx, chord) {
                #[cfg(windows)]
                crate::key_debug::record_consumed_action(
                    action,
                    action.context(),
                    chord,
                    "consume_no_repeat",
                );
                return true;
            }
        }
        false
    }

    pub fn pressed_action(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            for chord in chords.iter().copied() {
                if self.pressed_chord(ctx, chord) {
                    #[cfg(windows)]
                    crate::key_debug::record_pressed_action(
                        action,
                        action.context(),
                        chord,
                        "pressed_action",
                    );
                    return true;
                }
            }
            return false;
        }
        for chord in action.default_chords().iter() {
            if self.pressed_chord(ctx, chord) {
                #[cfg(windows)]
                crate::key_debug::record_pressed_action(
                    action,
                    action.context(),
                    chord,
                    "pressed_action",
                );
                return true;
            }
        }
        false
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

    /// KeyHold アクションに割り当てられたキーについて、このフレームの egui Key イベント
    /// (押下 / 離し) を取り出して消費する。戻り値 = (押下イベントあり, 離しイベントあり)。
    /// OS 直読みのエッジ (`key_held_action` のフレーム間差分) では取りこぼす高速タップ
    /// (idle からの同フレーム 押下+離し) を救うために併用する。修飾キー付きは対象外。
    pub fn take_key_hold_edges(&self, ctx: &egui::Context, action: KeyAction) -> (bool, bool) {
        debug_assert_eq!(action.trigger(), KeyTrigger::KeyHold);
        let keys: Vec<egui::Key> = if let Some(chords) = self.overrides.get(&action) {
            chords
                .iter()
                .filter_map(|c| c.key.and_then(KeyName::to_egui))
                .collect()
        } else {
            action
                .default_chords()
                .iter()
                .filter_map(|c| c.key.and_then(KeyName::to_egui))
                .collect()
        };
        if keys.is_empty() {
            return (false, false);
        }
        ctx.input_mut(|i| {
            let mut pressed = false;
            let mut released = false;
            i.events.retain(|e| {
                if let egui::Event::Key {
                    key,
                    pressed: p,
                    modifiers,
                    repeat,
                    ..
                } = e
                    && !modifiers.ctrl
                    && !modifiers.shift
                    && !modifiers.alt
                    && keys.contains(key)
                {
                    if *p {
                        if !*repeat {
                            pressed = true;
                        }
                    } else {
                        released = true;
                    }
                    return false; // consume
                }
                true
            });
            (pressed, released)
        })
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
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        let matches = |chord: Chord| {
            chord.matches_win32_parts(
                key.virtual_key,
                key.scan_code,
                key.extended,
                key.ctrl,
                key.shift,
                key.alt,
            )
        };
        if let Some(chords) = self.overrides.get(&action) {
            if let Some(chord) = chords.iter().copied().find(|chord| matches(*chord)) {
                crate::key_debug::record_consumed_action(
                    action,
                    action.context(),
                    chord,
                    "native_match",
                );
                return true;
            }
            return false;
        }
        if let Some(chord) = action.default_chords().iter().find(|chord| matches(*chord)) {
            crate::key_debug::record_consumed_action(
                action,
                action.context(),
                chord,
                "native_match",
            );
            return true;
        }
        false
    }

    #[cfg(windows)]
    pub fn matching_vk_action(
        &self,
        actions: &[KeyAction],
        key: &crate::video::native_window::NativeVideoKeyEvent,
    ) -> Option<KeyAction> {
        actions
            .iter()
            .copied()
            .find(|action| self.matches_vk_action(*action, key))
    }

    pub fn install_global_native_video_shortcuts(&self) {
        let mut chords = Vec::new();
        for action in KeyAction::all().iter().copied().filter(|action| {
            matches!(
                action.context(),
                KeyContext::FsCommon | KeyContext::FsVideo | KeyContext::Rating
            ) || *action == KeyAction::ToggleDetachedViewerMode
        }) {
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

        let help_chords = self.effective_chords(KeyAction::HelpShowContextShortcuts);
        let cell = GLOBAL_CONTEXT_HELP_CHORDS.get_or_init(|| RwLock::new(Vec::new()));
        if let Ok(mut guard) = cell.write() {
            *guard = help_chords;
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
        out.push_str("# キーボード割り当ての Action 名と既定キーの参照です。\n");
        match kind {
            IniTemplateKind::UserConfig => {
                out.push_str("# 旧 keymap.ini 互換のテンプレートです。\n");
                out.push_str(
                    "# GUI 設定が未作成の環境では、起動時に 1 回だけ読み込まれて settings.db へ移行されます。\n",
                );
                out.push_str(
                    "# 移行後の keymap.ini は keymap.ini.imported.bak へ退避され、以後は読み込まれません。\n",
                );
                out.push_str(
                    "# 変更したい行だけ先頭の # を外し、= の右側のキーを編集してください。\n",
                );
            }
            IniTemplateKind::DefaultReference => {
                out.push_str("# 参照用です。このファイルは編集しないでください。\n");
                out.push_str(
                    "# アプリ内蔵の既定キーが変わると mimageviewer がこのファイルを上書きします。\n",
                );
                out.push_str(
                    "# 旧 keymap.ini を使った手動設定は、初回起動時に settings.db へ移行されます。\n",
                );
                out.push_str(
                    "# 移行後の keymap.ini は keymap.ini.imported.bak へ退避され、以後は読み込まれません。\n",
                );
            }
        }
        out.push_str("# 通常は設定メニュー「操作カスタマイズ…」から編集します。\n");
        out.push_str("#\n");
        out.push_str("# 書式:\n");
        out.push_str("# - 下のキー定義行はすべてコメントアウトされています。\n");
        out.push_str("# - コメントアウトされた行は、内蔵の既定キーに追従します。\n");
        out.push_str(
            "# - 1 つの Action でもコメント解除すると、その Action の既定キーは全置換されます。\n",
        );
        out.push_str(
            "# - 既定キーを残したい場合は、残したいキーも Action.1..3 として明示してください。\n",
        );
        out.push_str("# - 1 つの Action には Action.1 / Action.2 / Action.3 で最大 3 個まで割り当てできます。\n");
        out.push_str("# - = none を指定すると、その Action を明示的に無効化できます。\n");
        out.push_str("# - 既定キーが無い Action は # Action = none と表示されます。\n");
        out.push_str("# - 同時に有効になり得る Action へ同じキーを割り当てると起動時に警告ログを出します。\n");
        out.push_str(
            "# - Esc / 修飾なし矢印キー、サムネイル一覧の Shift+矢印範囲選択は予約扱いです。\n",
        );
        out.push_str("#   割り当てても警告ログを出します。\n");
        out.push_str("# - 行末の ; 以降は説明コメントです。コメント解除後も残してかまいません。\n");
        out.push_str("# - 競合は拒否しません。競合時は先に判定された操作が有効になります。\n");
        out.push_str("# - 通常の押下操作は Ctrl/Shift/Alt + 通常キーを指定できます。\n");
        out.push_str("# - ModifierHold は Ctrl / Shift / Alt のいずれか 1 つだけ指定できます。\n");
        out.push_str("# - KeyHold は修飾キーなしの通常キー 1 つだけ指定できます。\n");
        out.push_str(
            "# - キー名の例: A..Z, 0..9, Numpad0..Numpad9, F1..F24, Left, Right, Up, Down,\n",
        );
        out.push_str(
            "#   Home, End, PageUp, PageDown, Space, Enter, Esc, Tab, Backspace, Delete,\n",
        );
        out.push_str(
            "#   [, ], ;, :, ,, ., \\, /, ?, -, ^, @, Yen, Ro, NumpadAdd, NumpadSubtract, NumpadEnter\n",
        );
        out.push_str("# - テンキー数字は通常の数字キーとは別キーとして扱われます。\n");
        out.push_str("#   従来の数字キー既定操作は互換のため 1 と Numpad1 の両方を既定割り当てにしています。\n");
        out.push_str(
            "# - Alt+F4 / Alt+Tab / Alt+Esc / Alt+Space / Ctrl+Alt+Del / Win キー系など、\n",
        );
        out.push_str("#   OS が予約しているショートカットは keymap.ini では上書きできません。\n");
        out.push_str(
            "# - マウス、ゲームパッド、ドラッグ&ドロップ、OS/egui のコピー/切り取り/貼り付け、\n",
        );
        out.push_str("#   IME 確定、右クリックメニュー、Escape ナビゲーション、修飾なし矢印ナビゲーション、\n");
        out.push_str("#   サムネイル一覧の Shift+矢印範囲選択は固定です。\n");
        out.push_str("#\n");
        out.push_str("# 例:\n");
        out.push_str("# [FsImage]\n");
        out.push_str("# FsSlideshow.1 = P      ; スライドショーを P に変更\n");
        out.push_str("# FsSlideshow.2 = S      ; S も残したい場合は明示的に併記\n");
        out.push_str("# FsCapture = none       ; キャプチャ保存キーを無効化\n");
        out.push_str("# FsLoupeLockToggle = L  ; ルーペ固定表示のトグルを L に変更\n");
        out.push_str(
            "# FsLoupeHold = Ctrl     ; 押している間だけルーペ表示する修飾キーを Ctrl に変更\n",
        );
        out.push_str("#\n");
        out.push_str("# [Rating]\n");
        out.push_str("# RatingItem1 = Ctrl+F1  ; 星1を付ける（アイテム）\n");
        out.push_str("#\n");
        out.push_str("# [Text]\n");
        out.push_str("# TextRedo.1 = Ctrl+Y        ; やり直し\n");
        out.push_str("# TextRedo.2 = Ctrl+Shift+Z  ; やり直しの別割り当て\n\n");
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
            out.push_str(&format!(
                "[{}] ; {}\n",
                section.ini_name(),
                section.description()
            ));
            for action in KeyAction::all()
                .iter()
                .copied()
                .filter(|action| action.context() == section && action.is_user_facing())
            {
                let description = action.description();
                let defaults: Vec<String> = action
                    .default_chords()
                    .iter()
                    .map(Chord::settings_name)
                    .collect();
                if defaults.len() <= 1 {
                    let default = defaults
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "none".to_string());
                    out.push_str(&format!(
                        "# {} = {default} ; {description}\n",
                        action.ini_name()
                    ));
                } else {
                    for (idx, default) in defaults.iter().enumerate() {
                        out.push_str(&format!(
                            "# {}.{} = {default} ; {description}\n",
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
        self.consume_chord_inner(ctx, chord, true)
    }

    fn consume_chord_no_repeat(&self, ctx: &egui::Context, chord: Chord) -> bool {
        self.consume_chord_inner(ctx, chord, false)
    }

    /// Fixed, documented inputs that are intentionally outside `KeyAction` can
    /// still consume through the same Win32 KeySlot queue. This prevents a
    /// remapped KeyAction and a fixed egui `consume_key` path from both seeing
    /// the same physical key in one frame.
    pub fn consume_fixed_chord(&self, ctx: &egui::Context, chord: Chord) -> bool {
        self.consume_chord(ctx, chord)
    }

    fn consume_chord_inner(&self, ctx: &egui::Context, chord: Chord, allow_repeat: bool) -> bool {
        if chord.key.is_none() {
            return false;
        }
        #[cfg(windows)]
        if crate::key_input::is_frame_active() {
            if ctx.wants_keyboard_input() {
                return false;
            }
            if crate::key_input::frame_had_key_down() {
                return crate::key_input::consume_key_down(allow_repeat, |edge| {
                    chord.matches_key_edge(edge)
                });
            }
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
                            repeat,
                            modifiers,
                            ..
                        } if (allow_repeat || !*repeat) && chord.matches_egui(*key, *modifiers)
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
        #[cfg(windows)]
        if crate::key_input::is_frame_active() {
            if ctx.wants_keyboard_input() {
                return false;
            }
            if crate::key_input::frame_had_key_down() {
                return crate::key_input::pressed_key_down(|edge| chord.matches_key_edge(edge));
            }
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
        let Some(key) = chord.key.and_then(KeyName::to_egui) else {
            return false;
        };
        // KeyHold は「修飾キーなしの通常キー」契約 (validate_for_trigger 参照)。修飾キーが
        // 同時に押されている間は不成立にして、Ctrl+Z (undo) / Shift+Z (分析) 等が KeyHold
        // アクション (例: 全画面ズーム) を誤起動しないようにする (Codex P1)。FS ビューポートで
        // stale な egui modifiers を避け、押下中判定と同じく OS 直読みを使う。
        #[cfg(windows)]
        {
            if modifier_held_via_os(ModKind::Ctrl)
                || modifier_held_via_os(ModKind::Shift)
                || modifier_held_via_os(ModKind::Alt)
            {
                return false;
            }
            if let Some(name) = chord.key
                && key_held_via_os(name)
            {
                return true;
            }
        }
        ctx.input(|i| {
            let m = i.modifiers;
            !m.ctrl && !m.shift && !m.alt && i.key_down(key)
        })
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

impl KeymapSettings {
    pub fn from_keymap(keymap: &Keymap) -> Self {
        let overrides = KeyAction::all()
            .iter()
            .copied()
            .filter_map(|action| {
                keymap
                    .override_chords(action)
                    .map(|chords| KeyBindingOverride {
                        action: action.ini_name().to_string(),
                        chords: chords.iter().copied().map(Chord::settings_name).collect(),
                    })
            })
            .collect();
        Self {
            overrides,
            legacy_ini_migration_done: true,
            legacy_ini_backup: None,
        }
    }

    pub fn import_legacy_ini_if_needed(&mut self, path: &Path) -> LegacyKeymapIniImport {
        let mut result = LegacyKeymapIniImport::default();
        if self.legacy_ini_migration_done {
            return result;
        }

        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.legacy_ini_migration_done = true;
                result.changed = true;
                return result;
            }
            Err(err) => {
                result.warnings.push(format!(
                    "failed to read legacy keymap.ini ({}): {}",
                    path.display(),
                    err
                ));
                return result;
            }
        };

        let imported_keymap = Keymap::from_ini_str(&text);
        result
            .warnings
            .extend(imported_keymap.warnings().iter().cloned());
        if self.overrides.is_empty() {
            self.overrides = Self::from_keymap(&imported_keymap).overrides;
        } else {
            result.warnings.push(
                "legacy keymap.ini was found but GUI keymap settings already exist; keeping GUI settings"
                    .to_string(),
            );
        }

        self.legacy_ini_migration_done = true;
        result.changed = true;
        result.imported = true;
        result
    }

    pub fn rename_imported_legacy_ini(&mut self, path: &Path) -> LegacyKeymapIniImport {
        let mut result = LegacyKeymapIniImport::default();
        let backup_path = next_legacy_keymap_backup_path(path);
        match std::fs::rename(path, &backup_path) {
            Ok(()) => {
                self.legacy_ini_backup = Some(backup_path.to_string_lossy().into_owned());
                result.backup_path = Some(backup_path);
                result.changed = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                result.warnings.push(format!(
                    "failed to rename legacy keymap.ini ({}): {}",
                    path.display(),
                    err
                ));
            }
        }
        result
    }

    pub fn override_chord_labels(&self, action: KeyAction) -> Option<Vec<String>> {
        self.overrides
            .iter()
            .find(|binding| KeyAction::parse_ini_name(&binding.action) == Some(action))
            .map(|binding| {
                binding
                    .chords
                    .iter()
                    .map(|label| {
                        parse_chord(label)
                            .map(|chord| chord.display_name())
                            .unwrap_or_else(|_| label.clone())
                    })
                    .collect()
            })
    }

    pub fn set_override_chords(&mut self, action: KeyAction, chords: Vec<Chord>) {
        self.remove_override(action);
        self.overrides.push(KeyBindingOverride {
            action: action.ini_name().to_string(),
            chords: chords.into_iter().map(Chord::settings_name).collect(),
        });
    }

    pub fn disable_action(&mut self, action: KeyAction) {
        self.remove_override(action);
        self.overrides.push(KeyBindingOverride {
            action: action.ini_name().to_string(),
            chords: Vec::new(),
        });
    }

    pub fn remove_override(&mut self, action: KeyAction) {
        self.overrides
            .retain(|binding| KeyAction::parse_ini_name(&binding.action) != Some(action));
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
    if native_video_fixed_shortcut_key(key) {
        return true;
    }
    if let Some(cell) = GLOBAL_NATIVE_VIDEO_CHORDS.get()
        && let Ok(guard) = cell.read()
    {
        return guard.iter().copied().any(|chord| {
            chord.matches_win32_parts(
                key.virtual_key,
                key.scan_code,
                key.extended,
                key.ctrl,
                key.shift,
                key.alt,
            )
        });
    }
    let fallback = Keymap::empty();
    KeyAction::all()
        .iter()
        .copied()
        .filter(|action| {
            matches!(
                action.context(),
                KeyContext::FsCommon | KeyContext::FsVideo | KeyContext::Rating
            ) || *action == KeyAction::ToggleDetachedViewerMode
        })
        .any(|action| fallback.matches_vk_action(action, key))
}

fn is_context_shortcuts_help_question_chord(chord: Chord) -> bool {
    chord.key == Some(KeyName::Slash) && chord.shift && !chord.ctrl && !chord.alt
}

#[cfg(windows)]
pub(crate) fn native_video_context_shortcuts_help_key_down(
    key: &crate::video::native_window::NativeVideoKeyEvent,
) -> bool {
    const VK_OEM_2: u32 = 0xBF; // US/JIS: slash key, Shift produces '?'.

    if key.repeat {
        return false;
    }
    let matches = |chord: Chord| {
        if is_context_shortcuts_help_question_chord(chord) {
            key.virtual_key == VK_OEM_2 && key.shift && !key.ctrl && !key.alt
        } else {
            chord.matches_win32_parts(
                key.virtual_key,
                key.scan_code,
                key.extended,
                key.ctrl,
                key.shift,
                key.alt,
            )
        }
    };
    if let Some(cell) = GLOBAL_CONTEXT_HELP_CHORDS.get()
        && let Ok(guard) = cell.read()
    {
        return guard.iter().copied().any(matches);
    }
    KeyAction::HelpShowContextShortcuts
        .default_chords()
        .iter()
        .any(matches)
}

fn native_video_fixed_shortcut_key(key: &crate::video::native_window::NativeVideoKeyEvent) -> bool {
    match key.virtual_key {
        0x1B => !key.ctrl && !key.shift && !key.alt, // Escape
        0x25 | 0x26 | 0x27 | 0x28 => !key.ctrl && !key.shift && !key.alt, // Plain arrows
        0xA6 | 0xA7 => true,                         // Browser back / forward
        _ => false,
    }
}

static GLOBAL_NATIVE_VIDEO_CHORDS: OnceLock<RwLock<Vec<Chord>>> = OnceLock::new();
static GLOBAL_CONTEXT_HELP_CHORDS: OnceLock<RwLock<Vec<Chord>>> = OnceLock::new();

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

fn parse_setting_chords(
    action: KeyAction,
    chord_names: &[String],
    warnings: &mut Vec<String>,
) -> Option<Vec<Chord>> {
    if chord_names.is_empty() {
        return Some(Vec::new());
    }
    let mut chords = Vec::new();
    for (idx, name) in chord_names.iter().take(3).enumerate() {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            return Some(Vec::new());
        }
        match parse_chord(trimmed) {
            Ok(chord) => {
                if let Err(msg) = chord.validate_for_trigger(action.trigger()) {
                    warnings.push(format!(
                        "settings: '{}'.{} ignored: {}",
                        action.ini_name(),
                        idx + 1,
                        msg
                    ));
                    continue;
                }
                chords.push(chord);
            }
            Err(msg) => warnings.push(format!(
                "settings: '{}'.{} ignored: {}",
                action.ini_name(),
                idx + 1,
                msg
            )),
        }
    }
    if chords.is_empty() {
        None
    } else {
        Some(chords)
    }
}

pub fn parse_chord_for_action(action: KeyAction, text: &str) -> Result<Option<Chord>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let chord = parse_chord(trimmed)?;
    chord
        .validate_for_trigger(action.trigger())
        .map_err(str::to_string)?;
    Ok(Some(chord))
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
            if token == "?" || token.eq_ignore_ascii_case("questionmark") {
                chord.shift = true;
                chord.key = Some(KeyName::Slash);
            } else {
                let key =
                    KeyName::parse(token).ok_or_else(|| format!("unknown key name '{token}'"))?;
                chord.key = Some(key);
            }
            key_seen = true;
        }
    }
    if !key_seen && !chord.ctrl && !chord.shift && !chord.alt {
        return Err("empty key chord".to_string());
    }
    Ok(chord)
}

fn next_legacy_keymap_backup_path(path: &Path) -> PathBuf {
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "keymap.ini".into());
    for index in 0..1000 {
        let name = if index == 0 {
            format!("{base}.imported.bak")
        } else {
            format!("{base}.imported-{index}.bak")
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.with_file_name(format!("{base}.imported-extra.bak"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn native_video_shortcut_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("native video shortcut test lock poisoned")
    }

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

    fn enum_variant_names_from_source(enum_name: &str) -> std::collections::BTreeSet<String> {
        let source = include_str!("keymap.rs");
        let marker = format!("pub enum {enum_name} {{");
        let start = source.find(&marker).expect("enum not found");
        let body = &source[start + marker.len()..];
        let end = body.find("\n}").expect("enum end not found");
        body[..end]
            .lines()
            .filter_map(|line| {
                let name = line.trim().trim_end_matches(',');
                (!name.is_empty() && !name.starts_with("//")).then(|| name.to_string())
            })
            .collect()
    }

    fn ring_action_enum_names_from_source() -> std::collections::BTreeSet<String> {
        let source = include_str!("ring_shortcut.rs");
        let start = source
            .find("pub enum RingActionId {")
            .expect("RingActionId enum not found");
        let body = &source[start + "pub enum RingActionId {".len()..];
        let end = body.find("\n}").expect("RingActionId enum end not found");
        body[..end]
            .lines()
            .filter_map(|line| {
                let name = line
                    .trim()
                    .trim_end_matches(',')
                    .split_once('(')
                    .map_or_else(
                        || line.trim().trim_end_matches(','),
                        |(name, _)| name.trim(),
                    );
                (!name.is_empty() && !name.starts_with("//")).then(|| name.to_string())
            })
            .collect()
    }

    fn add_numbered_names(
        out: &mut std::collections::BTreeSet<String>,
        prefix: &str,
        range: std::ops::RangeInclusive<usize>,
    ) {
        out.extend(range.map(|number| format!("{prefix}{number}")));
    }

    fn add_drive_names(out: &mut std::collections::BTreeSet<String>, prefix: &str) {
        out.extend(('C'..='Z').map(|letter| format!("{prefix}{letter}")));
    }

    #[test]
    fn all_actions_inventory_matches_key_action_enum() {
        assert_eq!(
            key_action_enum_names_from_source(),
            all_actions_names_from_source()
        );
    }

    #[test]
    fn ring_actions_are_classified_for_key_action_parity() {
        let mut key_handled = std::collections::BTreeSet::from([
            "AddToBook".to_string(),
            "PinRepresentativeThumb".to_string(),
            "ToggleDetachedViewer".to_string(),
            "ToggleWindowMode".to_string(),
            "ToggleMaximize".to_string(),
            "GridToggleDetails".to_string(),
            "GridToggleCheck".to_string(),
            "GridSelectAll".to_string(),
            "GridParentFolder".to_string(),
            "TreeFolderPrev".to_string(),
            "TreeFolderNext".to_string(),
            "SiblingFolderPrev".to_string(),
            "SiblingFolderNext".to_string(),
            "ImageRotateLeft".to_string(),
            "ImageRotateRight".to_string(),
            "ImageCapture".to_string(),
            "ImageToggleMetadata".to_string(),
            "ImageSlideshow".to_string(),
            "ImagePixelGrid".to_string(),
            "ImageBackgroundCycle".to_string(),
            "ImageComparePin".to_string(),
            "ImageHome".to_string(),
            "ImageEnd".to_string(),
            "VideoCapture".to_string(),
            "VideoMute".to_string(),
            "VideoLoop".to_string(),
            "VideoBookmark".to_string(),
            "VideoMarkerPrev".to_string(),
            "VideoMarkerNext".to_string(),
            "VideoTileMode".to_string(),
            "VideoExternalPlayer".to_string(),
            "OpenLocationDriveList".to_string(),
            "OpenLocationReadingHistory".to_string(),
            "OpenLocationRating1".to_string(),
            "OpenLocationRating2".to_string(),
            "OpenLocationRating3".to_string(),
            "OpenLocationRating4".to_string(),
            "OpenLocationRating5".to_string(),
            "OpenLocationBooksRoot".to_string(),
            "OpenLocationDesktop".to_string(),
            "OpenLocationPictures".to_string(),
            "OpenLocationDownloads".to_string(),
        ]);
        add_numbered_names(&mut key_handled, "OpenFavorite", 1..=20);
        add_drive_names(&mut key_handled, "OpenDrive");
        add_numbered_names(&mut key_handled, "GridColumnCount", 1..=10);

        let fixed_or_ring_only = std::collections::BTreeSet::from([
            // Favorite picker / snapshot lock / Explorer open-folder are input-layer features.
            "CycleFavorite".to_string(),
            "GridToggleSnapshotLock".to_string(),
            "ImageOpenFolder".to_string(),
            // Gamepad fixed-button defaults are exposed as assignable pad actions so users can
            // swap A/B/X/Y/Select/Start/LB/RB/LT/RT without turning them into keyboard commands.
            "GamepadAccept".to_string(),
            "GamepadBack".to_string(),
            "GamepadPicker".to_string(),
            "GamepadAuxiliary".to_string(),
            "GamepadSelectPanel".to_string(),
            "GamepadFavoritePanel".to_string(),
            "GamepadPrevFolder".to_string(),
            "GamepadNextFolder".to_string(),
            "GamepadLeftTrigger".to_string(),
            "GamepadRightTrigger".to_string(),
            // These are intentionally fixed because they depend on OS/browser/clipboard routes
            // or reserved navigation semantics rather than ordinary keymap dispatch.
            "GridHistoryBack".to_string(),
            "GridHistoryForward".to_string(),
            "ImageCopyToClipboard".to_string(),
            "ImageCopyPath".to_string(),
            "ImageCopyFileName".to_string(),
        ]);

        let mut classified = key_handled;
        classified.extend(fixed_or_ring_only);

        let mut ring_actions = ring_action_enum_names_from_source();
        ring_actions.remove("None");
        ring_actions.remove("Unknown");

        assert_eq!(
            ring_actions, classified,
            "Every RingActionId must be classified as KeyAction-backed or intentionally fixed/ring-only"
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
            for chord in action.default_chords().iter() {
                assert!(
                    chord.validate_for_trigger(action.trigger()).is_ok(),
                    "{} has an invalid default chord: {}",
                    action.ini_name(),
                    chord.display_name()
                );
            }
        }
    }

    #[test]
    fn command_catalog_has_one_spec_per_key_action() {
        let specs: Vec<CommandSpec> = command_catalog().collect();
        assert_eq!(specs.len(), KeyAction::all().len());

        let mut actions = std::collections::BTreeSet::new();
        for spec in specs {
            assert!(
                actions.insert(spec.action.ini_name()),
                "duplicate command spec for {}",
                spec.action.ini_name()
            );
            assert_eq!(spec.scope, spec.action.context());
            assert_eq!(spec.trigger, spec.action.trigger());
            assert_eq!(
                spec.binding_policy,
                BindingPolicy::for_trigger(spec.action.trigger())
            );
        }
    }

    #[test]
    fn top_menu_inventory_matches_top_menu_enum() {
        let all_names: std::collections::BTreeSet<String> =
            TopMenuId::ALL.iter().map(|id| format!("{id:?}")).collect();
        assert_eq!(enum_variant_names_from_source("TopMenuId"), all_names);
    }

    #[test]
    fn menu_command_inventory_matches_menu_command_enum() {
        let all_names: std::collections::BTreeSet<String> = MenuCommandId::ALL
            .iter()
            .map(|id| id.stable_name().to_string())
            .collect();
        assert_eq!(enum_variant_names_from_source("MenuCommandId"), all_names);
    }

    #[test]
    fn menu_command_catalog_has_unique_ids_and_valid_actions() {
        assert_eq!(menu_command_catalog().len(), MenuCommandId::ALL.len());
        let mut ids = std::collections::BTreeSet::new();
        for id in MenuCommandId::ALL {
            assert!(
                menu_command_spec(*id).is_some(),
                "missing menu command spec for {}",
                id.stable_name()
            );
        }
        for spec in menu_command_catalog() {
            assert!(
                ids.insert(spec.id),
                "duplicate menu command id: {}",
                spec.id.stable_name()
            );
            assert_eq!(menu_command_spec(spec.id), Some(*spec));
            assert!(!spec.label.is_empty());
            assert!(!spec.parent.label().is_empty());
            if let Some(action) = spec.action {
                assert!(
                    KeyAction::all().contains(&action),
                    "menu command action is not registered: {}",
                    action.ini_name()
                );
                assert_eq!(spec.description(), action.description());
            }
        }
    }

    #[test]
    fn menu_commands_for_parent_follow_top_menu_order() {
        let mut flattened_ids = Vec::new();
        for parent in TopMenuId::ALL {
            let specs: Vec<_> = menu_commands_for_parent(*parent).collect();
            assert!(
                !specs.is_empty(),
                "top menu has no catalog entries: {}",
                parent.label()
            );
            for spec in specs {
                assert_eq!(spec.parent, *parent);
                flattened_ids.push(spec.id);
            }
        }

        let catalog_ids: Vec<_> = menu_command_catalog().iter().map(|spec| spec.id).collect();
        assert_eq!(flattened_ids, catalog_ids);
    }

    #[test]
    fn menu_ids_parse_stable_names() {
        for &id in TopMenuId::ALL {
            assert_eq!(TopMenuId::parse_stable_name(id.stable_name()), Some(id));
        }
        assert_eq!(TopMenuId::parse_stable_name("FutureMenu"), None);

        for &id in MenuCommandId::ALL {
            assert_eq!(MenuCommandId::parse_stable_name(id.stable_name()), Some(id));
        }
        assert_eq!(MenuCommandId::parse_stable_name("FutureCommand"), None);
    }

    #[test]
    fn default_menu_layout_settings_resolves_to_catalog_order() {
        let resolved = resolve_menu_layout(&MenuLayoutSettings::default());
        let default_settings = default_menu_layout_settings();
        let resolved_from_explicit = resolve_menu_layout(&default_settings);
        assert_eq!(resolved, resolved_from_explicit);

        let resolved_parents: Vec<_> = resolved.menus.iter().map(|menu| menu.id).collect();
        assert_eq!(resolved_parents, TopMenuId::ALL.to_vec());

        for menu in &resolved.menus {
            let expected: Vec<_> = menu_commands_for_parent(menu.id)
                .map(|spec| spec.id)
                .collect();
            assert_eq!(menu.commands, expected);
        }
    }

    #[test]
    fn menu_layout_resolution_reorders_hides_and_appends_missing() {
        let settings = MenuLayoutSettings {
            top_menu_order: vec![
                "Help".to_string(),
                "File".to_string(),
                "FutureMenu".to_string(),
                "Help".to_string(),
            ],
            command_order: vec![
                MenuCommandOrderSettings {
                    parent: "Help".to_string(),
                    commands: vec![
                        "HelpAbout".to_string(),
                        "FileOpenFolder".to_string(),
                        "FutureCommand".to_string(),
                        "HelpOpenManual".to_string(),
                        "HelpAbout".to_string(),
                    ],
                },
                MenuCommandOrderSettings {
                    parent: "File".to_string(),
                    commands: vec![
                        "FileQuit".to_string(),
                        "FileOpenFolder".to_string(),
                        "FileQuit".to_string(),
                    ],
                },
            ],
            hidden_commands: vec![
                "HelpOpenLogs".to_string(),
                "FileReadingHistory".to_string(),
                "SettingsPreferences".to_string(),
                "FutureCommand".to_string(),
            ],
        };

        let resolved = resolve_menu_layout(&settings);
        assert_eq!(resolved.menus[0].id, TopMenuId::Help);
        assert_eq!(
            resolved.menus[0].commands,
            vec![
                MenuCommandId::HelpAbout,
                MenuCommandId::HelpOpenManual,
                MenuCommandId::HelpShowWhatsNew,
            ]
        );
        assert_eq!(resolved.menus[1].id, TopMenuId::File);
        assert_eq!(
            resolved.menus[1].commands,
            vec![
                MenuCommandId::FileQuit,
                MenuCommandId::FileOpenFolder,
                MenuCommandId::FileLocalSearch,
                MenuCommandId::FileOpenCaptureFolder,
                MenuCommandId::FileOpenRecycleBin,
            ]
        );
        assert_eq!(resolved.menus[2].id, TopMenuId::Favorites);
        let settings_menu = resolved
            .menus
            .iter()
            .find(|menu| menu.id == TopMenuId::Settings)
            .expect("settings menu should remain reachable");
        assert!(
            settings_menu
                .commands
                .contains(&MenuCommandId::SettingsPreferences)
        );
    }

    #[test]
    fn settings_preferences_menu_command_cannot_be_hidden() {
        assert!(!menu_command_can_be_hidden(
            MenuCommandId::SettingsPreferences
        ));
        let settings = MenuLayoutSettings {
            top_menu_order: vec!["Settings".to_string()],
            command_order: Vec::new(),
            hidden_commands: menu_commands_for_parent(TopMenuId::Settings)
                .map(|spec| spec.id.stable_name().to_string())
                .collect(),
        };

        let resolved = resolve_menu_layout(&settings);
        assert_eq!(resolved.menus[0].id, TopMenuId::Settings);
        assert_eq!(
            resolved.menus[0].commands,
            vec![MenuCommandId::SettingsPreferences]
        );
    }

    #[test]
    fn menu_layout_resolution_drops_empty_top_menus() {
        let settings = MenuLayoutSettings {
            top_menu_order: vec!["Help".to_string(), "File".to_string()],
            command_order: Vec::new(),
            hidden_commands: menu_commands_for_parent(TopMenuId::Help)
                .map(|spec| spec.id.stable_name().to_string())
                .collect(),
        };

        let resolved = resolve_menu_layout(&settings);
        let resolved_parents: Vec<_> = resolved.menus.iter().map(|menu| menu.id).collect();
        assert!(!resolved_parents.contains(&TopMenuId::Help));
        assert_eq!(resolved_parents.first(), Some(&TopMenuId::File));
    }

    #[test]
    fn menu_layout_settings_roundtrip_uses_stable_names() {
        let settings = MenuLayoutSettings {
            top_menu_order: vec!["Help".to_string(), "File".to_string()],
            command_order: vec![MenuCommandOrderSettings {
                parent: "Help".to_string(),
                commands: vec!["HelpAbout".to_string(), "HelpOpenManual".to_string()],
            }],
            hidden_commands: vec!["HelpOpenLogs".to_string()],
        };

        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("HelpAbout"));
        assert!(!json.contains("ヘルプ"));
        let back: MenuLayoutSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn menu_command_labels_follow_keymap_overrides() {
        let keymap = Keymap::empty();
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileOpenFolder),
            "フォルダを開く… (Ctrl+O)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileReadingHistory),
            "読書履歴を開く"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FavoritesAddCurrentFolder),
            "このフォルダを追加…"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FavoritesEdit),
            "編集"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::TagsManagePinned),
            "ピン留めタグの管理…"
        );
        for (id, label) in [
            (
                MenuCommandId::BooksAddSelectionToActiveBook,
                "追加先の本に追加",
            ),
            (
                MenuCommandId::BooksAddClipboardImage,
                "クリップボードの画像を本に追加",
            ),
            (MenuCommandId::BooksOpenRoot, "本棚フォルダを開く"),
            (MenuCommandId::BooksOpenActiveBook, "追加先の本を開く"),
            (MenuCommandId::BooksReorderCurrentBook, "この本を並べ替え…"),
            (MenuCommandId::BooksManage, "製本の管理…"),
            (
                MenuCommandId::VideoRegisterUpscale,
                "この動画をアップスケール登録…",
            ),
            (
                MenuCommandId::VideoDeleteUpscale,
                "この動画のアップスケールを削除",
            ),
            (
                MenuCommandId::VideoShowUpscaleTasks,
                "アップスケールタスク表示",
            ),
            (
                MenuCommandId::SettingsThumbnailCache,
                "サムネイルキャッシュ管理",
            ),
            (
                MenuCommandId::SettingsArchiveCache,
                "変換済みアーカイブキャッシュ管理",
            ),
            (MenuCommandId::SettingsThumbnailQuality, "サムネイル画質…"),
            (MenuCommandId::SettingsStats, "統計…"),
            (MenuCommandId::SettingsResetRotation, "回転情報をリセット…"),
            (MenuCommandId::SettingsRestoreSettings, "設定の復元…"),
            (
                MenuCommandId::SettingsOperationCustomize,
                "操作カスタマイズ…",
            ),
            (MenuCommandId::SettingsPreferences, "環境設定…"),
            (MenuCommandId::HelpOpenManual, "ヘルプサイトを開く"),
            (MenuCommandId::HelpOpenLogs, "ログフォルダを開く"),
            (MenuCommandId::HelpShowWhatsNew, "重要な変更点を表示"),
            (MenuCommandId::HelpAbout, "バージョン情報"),
        ] {
            assert_eq!(keymap.menu_command_label(id), label);
        }
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileLocalSearch),
            "現在地フィルタ (Ctrl+F)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FavoritesFavSearch),
            "コンテナ検索 (Ctrl+S)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FavoritesMetadataSearch),
            "アイテム検索 (Ctrl+G)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::TagsTagView),
            "タグビュー (Ctrl+T)"
        );

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            GlobalOpenFolder = F3
            GlobalLocalSearch = F2
            GlobalFavSearch = none
            [Grid]
            GridOpenLocationReadingHistory = Ctrl+L
            "#,
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileOpenFolder),
            "フォルダを開く… (F3)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileReadingHistory),
            "読書履歴を開く (Ctrl+L)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FileLocalSearch),
            "現在地フィルタ (F2)"
        );
        assert_eq!(
            keymap.menu_command_label(MenuCommandId::FavoritesFavSearch),
            "コンテナ検索"
        );
    }

    #[test]
    fn command_display_rows_filter_active_scopes_and_hide_unassigned() {
        let keymap = Keymap::empty();
        let rows = keymap.command_display_rows_for_active_scopes(GRID_ACTIVE_SCOPES, false);
        assert!(
            !rows
                .iter()
                .any(|row| row.spec.action == KeyAction::HelpShowContextShortcuts),
            "context help action is rendered as the dialog trigger row, not as a duplicate command row"
        );

        let local_search = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::GlobalLocalSearch)
            .expect("global actions should be visible in grid context");
        assert_eq!(local_search.spec.scope, KeyContext::Global);
        assert_eq!(local_search.shortcut_labels, vec!["Ctrl+F".to_owned()]);

        let grid_pin = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::GridPin)
            .expect("grid actions should be visible in grid context");
        assert_eq!(grid_pin.spec.scope, KeyContext::Grid);
        assert_eq!(grid_pin.shortcut_labels, vec!["P".to_owned()]);

        let rating = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::RatingItem1)
            .expect("rating actions should be visible in grid context");
        assert_eq!(rating.spec.scope, KeyContext::Rating);
        assert_eq!(rating.shortcut_labels, vec!["F1".to_owned()]);

        assert!(
            !rows
                .iter()
                .any(|row| row.spec.action == KeyAction::FsSlideshow)
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.spec.action == KeyAction::GridToggleStackMode)
        );

        let rows = keymap.command_display_rows_for_active_scopes(GRID_ACTIVE_SCOPES, true);
        let stack_toggle = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::GridToggleStackMode)
            .expect("include_unassigned should include default-unassigned actions");
        assert_eq!(stack_toggle.spec.scope, KeyContext::Grid);
        assert!(stack_toggle.shortcut_labels.is_empty());
    }

    #[test]
    fn fullscreen_metadata_action_is_image_scoped_not_video_scoped() {
        assert_eq!(KeyAction::FsToggleMetadata.context(), KeyContext::FsImage);
        for action in [
            KeyAction::FsToggleWindowMode,
            KeyAction::FsCtrlNavPrev,
            KeyAction::FsCtrlNavNext,
            KeyAction::FsSiblingPrev,
            KeyAction::FsSiblingNext,
        ] {
            assert_eq!(action.context(), KeyContext::FsCommon);
        }

        let keymap = Keymap::empty();
        let image_rows =
            keymap.command_display_rows_for_active_scopes(FS_IMAGE_ACTIVE_SCOPES, false);
        assert!(
            image_rows
                .iter()
                .any(|row| row.spec.action == KeyAction::FsToggleMetadata),
            "image fullscreen help should include metadata toggle"
        );

        let video_rows =
            keymap.command_display_rows_for_active_scopes(FS_VIDEO_ACTIVE_SCOPES, false);
        assert!(
            !video_rows
                .iter()
                .any(|row| row.spec.action == KeyAction::FsToggleMetadata),
            "video fullscreen active scopes should not advertise metadata toggle"
        );
        for action in [
            KeyAction::FsToggleWindowMode,
            KeyAction::FsCtrlNavPrev,
            KeyAction::FsCtrlNavNext,
            KeyAction::FsSiblingPrev,
            KeyAction::FsSiblingNext,
        ] {
            assert!(
                video_rows.iter().any(|row| row.spec.action == action),
                "{action:?} should remain visible in video fullscreen"
            );
        }
    }

    #[test]
    fn video_close_fullscreen_is_video_scoped_and_unassigned_by_default() {
        assert_eq!(
            KeyAction::VideoCloseFullscreen.context(),
            KeyContext::FsVideo
        );
        assert!(KeyAction::VideoCloseFullscreen.default_chords().is_empty());

        let keymap = Keymap::empty();
        let visible_rows =
            keymap.command_display_rows_for_active_scopes(FS_VIDEO_ACTIVE_SCOPES, false);
        assert!(
            !visible_rows
                .iter()
                .any(|row| row.spec.action == KeyAction::VideoCloseFullscreen),
            "unassigned video close command should not clutter shortcut help by default"
        );

        let all_rows = keymap.command_display_rows_for_active_scopes(FS_VIDEO_ACTIVE_SCOPES, true);
        let close_row = all_rows
            .iter()
            .find(|row| row.spec.action == KeyAction::VideoCloseFullscreen)
            .expect("operation customization should list default-unassigned video close command");
        assert_eq!(close_row.spec.scope, KeyContext::FsVideo);
        assert!(close_row.shortcut_labels.is_empty());
    }

    #[test]
    fn command_display_rows_follow_overrides_and_none() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Ctrl+Shift+S
            GridPin = none
            "#,
        );
        assert!(
            keymap.warnings().is_empty(),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );

        let rows = keymap.command_display_rows_for_active_scopes(GRID_ACTIVE_SCOPES, false);
        let stack_toggle = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::GridToggleStackMode)
            .expect("customized default-unassigned action should be visible");
        assert_eq!(
            stack_toggle.shortcut_labels,
            vec!["Ctrl+Shift+S".to_owned()]
        );
        assert!(!rows.iter().any(|row| row.spec.action == KeyAction::GridPin));

        let rows = keymap.command_display_rows_for_active_scopes(GRID_ACTIVE_SCOPES, true);
        let grid_pin = rows
            .iter()
            .find(|row| row.spec.action == KeyAction::GridPin)
            .expect("include_unassigned should include explicitly disabled actions");
        assert!(grid_pin.shortcut_labels.is_empty());
    }

    #[test]
    fn command_scope_overlap_table_matches_current_dispatch_model() {
        assert!(command_scopes_overlap(KeyContext::Global, KeyContext::Grid));
        assert!(command_scopes_overlap(KeyContext::Rating, KeyContext::Grid));
        assert!(command_scopes_overlap(
            KeyContext::Rating,
            KeyContext::FsImage
        ));
        assert!(command_scopes_overlap(
            KeyContext::FsCommon,
            KeyContext::FsImage
        ));
        assert!(!command_scopes_overlap(
            KeyContext::FsCommon,
            KeyContext::Erase
        ));
        assert!(!command_scopes_overlap(KeyContext::Grid, KeyContext::Erase));
        assert!(!command_scopes_overlap(
            KeyContext::FsImage,
            KeyContext::FsVideo
        ));
        assert!(!command_scopes_overlap(
            KeyContext::Erase,
            KeyContext::Conceal
        ));
    }

    #[test]
    fn fs_common_bindings_do_not_conflict_with_edit_mode_bindings() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Fullscreen]
            FsCtrlNavPrev = Ctrl+M
            "#,
        );
        let conflicts = keymap.binding_conflicts();
        assert!(
            !conflicts.iter().any(|conflict| {
                conflict.chord == Chord::ctrl(KeyName::M)
                    && ((conflict.action == KeyAction::FsCtrlNavPrev
                        && conflict.other_action == Some(KeyAction::ConcealExit))
                        || (conflict.action == KeyAction::ConcealExit
                            && conflict.other_action == Some(KeyAction::FsCtrlNavPrev)))
            }),
            "FsCommon and edit-mode commands are not active together: {:?}",
            conflicts
        );
    }

    #[test]
    fn grid_toggle_stack_mode_is_default_unassigned() {
        assert!(KeyAction::GridToggleStackMode.default_chords().is_empty());
        assert_eq!(KeyAction::GridToggleStackMode.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridToggleStackMode.trigger(), KeyTrigger::Press);
    }

    #[test]
    fn context_help_binding_conflicts_with_edit_mode_commands() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            HelpShowContextShortcuts = S
            "#,
        );
        let conflicts = keymap.binding_conflicts();
        assert!(
            conflicts.iter().any(|conflict| {
                conflict.kind == BindingConflictKind::ActiveOverlap
                    && conflict.chord == Chord::key(KeyName::S)
                    && ((conflict.action == KeyAction::HelpShowContextShortcuts
                        && conflict.other_action == Some(KeyAction::EraseToolSelect))
                        || (conflict.action == KeyAction::EraseToolSelect
                            && conflict.other_action == Some(KeyAction::HelpShowContextShortcuts)))
            }),
            "{conflicts:?}"
        );

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            GlobalOpenFolder = S
            "#,
        );
        assert!(!keymap.binding_conflicts().iter().any(|conflict| {
            conflict.chord == Chord::key(KeyName::S)
                && (conflict.action == KeyAction::EraseToolSelect
                    || conflict.other_action == Some(KeyAction::EraseToolSelect))
        }));
    }

    #[test]
    fn grid_mask_actions_match_existing_default_shortcuts() {
        assert_eq!(
            KeyAction::GridApplyErase1.default_chords().iter().next(),
            Some(Chord::key(KeyName::F7))
        );
        assert_eq!(
            KeyAction::GridApplyErase2.default_chords().iter().next(),
            Some(Chord::key(KeyName::F8))
        );
        assert_eq!(
            KeyAction::GridApplyConceal1.default_chords().iter().next(),
            Some(Chord::key(KeyName::F9))
        );
        assert_eq!(
            KeyAction::GridApplyConceal2.default_chords().iter().next(),
            Some(Chord::key(KeyName::F10))
        );
        assert_eq!(
            KeyAction::GridDeleteEraseMask
                .default_chords()
                .iter()
                .collect::<Vec<_>>(),
            vec![Chord::shift(KeyName::F7), Chord::shift(KeyName::F8)]
        );
        assert_eq!(
            KeyAction::GridDeleteConcealMask
                .default_chords()
                .iter()
                .collect::<Vec<_>>(),
            vec![Chord::shift(KeyName::F9), Chord::shift(KeyName::F10)]
        );
        for action in [
            KeyAction::GridApplyErase1,
            KeyAction::GridApplyErase2,
            KeyAction::GridApplyConceal1,
            KeyAction::GridApplyConceal2,
            KeyAction::GridDeleteEraseMask,
            KeyAction::GridDeleteConcealMask,
        ] {
            assert_eq!(action.context(), KeyContext::Grid);
            assert_eq!(action.trigger(), KeyTrigger::Press);
        }
    }

    #[test]
    fn fullscreen_stack_jump_actions_match_existing_default_shortcuts() {
        assert_eq!(
            KeyAction::FsStackJumpPrev.default_chords().iter().next(),
            Some(Chord::shift(KeyName::Up))
        );
        assert_eq!(
            KeyAction::FsStackJumpNext.default_chords().iter().next(),
            Some(Chord::shift(KeyName::Down))
        );
        for action in [KeyAction::FsStackJumpPrev, KeyAction::FsStackJumpNext] {
            assert_eq!(action.context(), KeyContext::FsImage);
            assert_eq!(action.trigger(), KeyTrigger::Press);
        }
    }

    #[test]
    fn window_and_parent_actions_match_existing_default_shortcuts() {
        assert_eq!(
            KeyAction::GridParentFolder
                .default_chords()
                .iter()
                .collect::<Vec<_>>(),
            vec![Chord::key(KeyName::Backspace), Chord::alt(KeyName::Up)]
        );
        assert_eq!(
            KeyAction::GridHistoryBack.default_chords().iter().next(),
            Some(Chord::alt(KeyName::Left))
        );
        assert_eq!(
            KeyAction::GridHistoryForward.default_chords().iter().next(),
            Some(Chord::alt(KeyName::Right))
        );
        assert_eq!(
            KeyAction::GridToggleMaximize.default_chords().iter().next(),
            Some(Chord::key(KeyName::F11))
        );
        assert_eq!(
            KeyAction::FsToggleWindowMode.default_chords().iter().next(),
            Some(Chord::key(KeyName::F11))
        );
        assert_eq!(
            KeyAction::GridTreeFolderPrev.default_chords().iter().next(),
            Some(Chord::ctrl(KeyName::Up))
        );
        assert_eq!(
            KeyAction::GridTreeFolderNext.default_chords().iter().next(),
            Some(Chord::ctrl(KeyName::Down))
        );
        assert_eq!(
            KeyAction::GridSiblingFolderPrev
                .default_chords()
                .iter()
                .next(),
            Some(Chord::ctrl(KeyName::PageUp))
        );
        assert_eq!(
            KeyAction::GridSiblingFolderNext
                .default_chords()
                .iter()
                .next(),
            Some(Chord::ctrl(KeyName::PageDown))
        );
        assert_eq!(KeyAction::GridParentFolder.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridHistoryBack.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridHistoryForward.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridTreeFolderPrev.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridTreeFolderNext.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridSiblingFolderPrev.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridSiblingFolderNext.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridToggleMaximize.context(), KeyContext::Grid);
        assert_eq!(
            KeyAction::FsToggleWindowMode.context(),
            KeyContext::FsCommon
        );
        assert_eq!(KeyAction::GridParentFolder.trigger(), KeyTrigger::Press);
        assert_eq!(KeyAction::GridHistoryBack.trigger(), KeyTrigger::Press);
        assert_eq!(KeyAction::GridHistoryForward.trigger(), KeyTrigger::Press);
        assert_eq!(KeyAction::GridTreeFolderPrev.trigger(), KeyTrigger::Press);
        assert_eq!(KeyAction::GridTreeFolderNext.trigger(), KeyTrigger::Press);
        assert_eq!(
            KeyAction::GridSiblingFolderPrev.trigger(),
            KeyTrigger::Press
        );
        assert_eq!(
            KeyAction::GridSiblingFolderNext.trigger(),
            KeyTrigger::Press
        );
        assert_eq!(KeyAction::GridToggleMaximize.trigger(), KeyTrigger::Press);
        assert_eq!(KeyAction::FsToggleWindowMode.trigger(), KeyTrigger::Press);
    }

    #[test]
    fn location_navigation_actions_are_grid_scoped_and_default_unassigned() {
        assert_eq!(KeyAction::GridOpenFavorite1.favorite_slot_number(), Some(1));
        assert_eq!(
            KeyAction::GridOpenFavorite20.favorite_slot_number(),
            Some(20)
        );
        assert_eq!(
            KeyAction::favorite_slot_action(1),
            Some(KeyAction::GridOpenFavorite1)
        );
        assert_eq!(
            KeyAction::favorite_slot_action(20),
            Some(KeyAction::GridOpenFavorite20)
        );
        assert_eq!(KeyAction::GridOpenDriveC.drive_letter(), Some('C'));
        assert_eq!(KeyAction::GridOpenDriveZ.drive_letter(), Some('Z'));
        assert_eq!(
            KeyAction::drive_action('c'),
            Some(KeyAction::GridOpenDriveC)
        );
        assert_eq!(
            KeyAction::drive_action('Z'),
            Some(KeyAction::GridOpenDriveZ)
        );
        assert_eq!(
            KeyAction::GridTogglePinnedTag1.pinned_tag_slot_number(),
            Some(1)
        );
        assert_eq!(
            KeyAction::GridTogglePinnedTag20.pinned_tag_slot_number(),
            Some(20)
        );
        assert_eq!(
            KeyAction::pinned_tag_slot_action(1),
            Some(KeyAction::GridTogglePinnedTag1)
        );
        assert_eq!(
            KeyAction::pinned_tag_slot_action(20),
            Some(KeyAction::GridTogglePinnedTag20)
        );

        for action in LOCATION_NAVIGATION_ACTIONS {
            assert_eq!(action.context(), KeyContext::Grid);
            assert_eq!(action.trigger(), KeyTrigger::Press);
            assert!(action.default_chords().is_empty());
            assert!(action.is_location_navigation_action());
        }
        for action in PINNED_TAG_ACTIONS {
            assert_eq!(action.context(), KeyContext::Grid);
            assert_eq!(action.trigger(), KeyTrigger::Press);
            assert!(action.default_chords().is_empty());
            assert!(!action.is_location_navigation_action());
        }

        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridOpenDriveC = Y
            GridTogglePinnedTag1 = U
            "#,
        );
        assert!(keymap.warnings().is_empty());
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::key(KeyName::Y),
                GRID_ACTIVE_SCOPES,
                LOCATION_NAVIGATION_ACTIONS,
            ),
            Some(KeyAction::GridOpenDriveC)
        );
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::key(KeyName::Y),
                FS_VIDEO_ACTIVE_SCOPES,
                LOCATION_NAVIGATION_ACTIONS,
            ),
            None
        );
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::key(KeyName::U),
                GRID_ACTIVE_SCOPES,
                PINNED_TAG_ACTIONS,
            ),
            Some(KeyAction::GridTogglePinnedTag1)
        );
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::key(KeyName::U),
                FS_IMAGE_ACTIVE_SCOPES,
                PINNED_TAG_ACTIONS,
            ),
            None
        );
    }

    #[test]
    fn grid_location_shortcuts_do_not_overlap_fullscreen_compare_defaults() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridOpenFavorite1 = X
            "#,
        );
        let conflicts = keymap.binding_conflicts();
        assert!(conflicts.iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Hard
                && conflict.chord == Chord::key(KeyName::X)
                && (conflict.action == KeyAction::GridOpenFavorite1
                    && conflict.other_action == Some(KeyAction::GridComparePin)
                    || conflict.action == KeyAction::GridComparePin
                        && conflict.other_action == Some(KeyAction::GridOpenFavorite1))
        }));
        assert!(!conflicts.iter().any(|conflict| {
            conflict.action == KeyAction::FsCompareToggle
                || conflict.other_action == Some(KeyAction::FsCompareToggle)
                || conflict.action == KeyAction::VideoCompareToggle
                || conflict.other_action == Some(KeyAction::VideoCompareToggle)
        }));
    }

    #[test]
    fn fullscreen_page_and_fixed_jump_actions_match_default_shortcuts() {
        assert!(KeyAction::FsPagePrev.default_chords().is_empty());
        assert!(KeyAction::FsPageNext.default_chords().is_empty());
        assert_eq!(
            KeyAction::FsFixedJumpPrev.default_chords().iter().next(),
            Some(Chord::shift(KeyName::Left))
        );
        assert_eq!(
            KeyAction::FsFixedJumpNext.default_chords().iter().next(),
            Some(Chord::shift(KeyName::Right))
        );
        assert_eq!(
            KeyAction::FsFixedJumpPrevNoRtl
                .default_chords()
                .iter()
                .next(),
            Some(Chord::key(KeyName::PageUp))
        );
        assert_eq!(
            KeyAction::FsFixedJumpNextNoRtl
                .default_chords()
                .iter()
                .next(),
            Some(Chord::key(KeyName::PageDown))
        );
        for action in [
            KeyAction::FsPagePrev,
            KeyAction::FsPageNext,
            KeyAction::FsFixedJumpPrev,
            KeyAction::FsFixedJumpNext,
            KeyAction::FsFixedJumpPrevNoRtl,
            KeyAction::FsFixedJumpNextNoRtl,
        ] {
            assert_eq!(action.context(), KeyContext::FsImage);
            assert_eq!(action.trigger(), KeyTrigger::Press);
        }
    }

    #[test]
    fn fullscreen_ai_and_post_filter_direct_actions_are_unassigned_image_commands() {
        let actions = [
            KeyAction::FsAiModelAuto,
            KeyAction::FsAiModelRealEsrganX4Plus,
            KeyAction::FsAiModelRealEsrganAnime6B,
            KeyAction::FsAiModelRealCugan4x,
            KeyAction::FsAiModelNmkdSiax4x,
            KeyAction::FsAiModelRealEsrGeneralV3,
            KeyAction::FsPostFilterNearest,
            KeyAction::FsPostFilterCrtSimple,
            KeyAction::FsPostFilterCrtFull,
            KeyAction::FsPostFilterCrtArcade,
            KeyAction::FsPostFilterDither1bit,
            KeyAction::FsPostFilterGameBoy,
            KeyAction::FsPostFilterPc98,
            KeyAction::FsPostFilterGameGear,
            KeyAction::FsPostFilterFamicom,
            KeyAction::FsPostFilterMegaDrive,
            KeyAction::FsPostFilterMsx2Plus,
            KeyAction::FsPostFilterSfc,
            KeyAction::FsPostFilterComboFamicomCrt,
            KeyAction::FsPostFilterComboPc98Crt,
            KeyAction::FsPostFilterComboMsx2PlusCrt,
            KeyAction::FsPostFilterComboMegaDriveCrt,
            KeyAction::FsPostFilterComboSfcCrt,
            KeyAction::FsPostFilterSepia,
            KeyAction::FsPostFilterMonoNeutral,
            KeyAction::FsPostFilterMonoCool,
            KeyAction::FsPostFilterMonoWarm,
            KeyAction::FsPostFilterWarmTone,
            KeyAction::FsPostFilterCoolTone,
            KeyAction::FsPostFilterTealOrange,
            KeyAction::FsPostFilterKodakPortra,
            KeyAction::FsPostFilterFujiVelvia,
            KeyAction::FsPostFilterBleachBypass,
            KeyAction::FsPostFilterCrossProcess,
            KeyAction::FsPostFilterVintage,
            KeyAction::FsPostFilterFilmGrain,
            KeyAction::FsPostFilterVignette,
            KeyAction::FsPostFilterLightLeak,
            KeyAction::FsPostFilterSoftFocus,
            KeyAction::FsPostFilterHalftone,
            KeyAction::FsPostFilterOilPaint,
            KeyAction::FsPostFilterSketch,
            KeyAction::FsPostFilterPseudoColor4,
            KeyAction::FsPostFilterPseudoColorSkin,
            KeyAction::FsPostFilterSharpen,
        ];

        for action in actions {
            assert_eq!(action.context(), KeyContext::FsImage);
            assert_eq!(action.trigger(), KeyTrigger::Press);
            assert!(action.default_chords().is_empty(), "{action:?}");
            assert!(action.is_user_facing(), "{action:?}");
        }
    }

    #[test]
    fn fullscreen_vertical_resolver_uses_active_scope() {
        let keymap = Keymap::empty();
        let priority = [
            KeyAction::VideoVolumeUp,
            KeyAction::VideoVolumeDown,
            KeyAction::FsStackJumpPrev,
            KeyAction::FsStackJumpNext,
        ];
        let video_scopes = [
            KeyContext::Global,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsVideo,
        ];
        let image_scopes = [
            KeyContext::Global,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsImage,
        ];

        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::shift(KeyName::Up),
                &video_scopes,
                &priority,
            ),
            Some(KeyAction::VideoVolumeUp)
        );
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::shift(KeyName::Up),
                &image_scopes,
                &priority,
            ),
            Some(KeyAction::FsStackJumpPrev)
        );
    }

    #[test]
    fn fullscreen_vertical_resolver_honors_overrides_per_scope() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsStackJumpNext = Ctrl+Alt+J
            [FsVideo]
            VideoVolumeDown = Ctrl+Alt+J
            "#,
        );
        assert!(keymap.warnings().is_empty());
        let priority = [
            KeyAction::VideoVolumeDown,
            KeyAction::FsStackJumpNext,
            KeyAction::VideoNextFile,
        ];
        let video_scopes = [
            KeyContext::Global,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsVideo,
        ];
        let image_scopes = [
            KeyContext::Global,
            KeyContext::FsCommon,
            KeyContext::Rating,
            KeyContext::FsImage,
        ];

        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::new(true, false, true, KeyName::J),
                &video_scopes,
                &priority,
            ),
            Some(KeyAction::VideoVolumeDown)
        );
        assert_eq!(
            keymap.resolve_first_action_for_chord(
                Chord::new(true, false, true, KeyName::J),
                &image_scopes,
                &priority,
            ),
            Some(KeyAction::FsStackJumpNext)
        );
    }

    #[test]
    fn parses_multiple_slots_and_replaces_defaults() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsSlideshow.1 = Ctrl+Alt+P
            FsSlideshow.2 = Ctrl+Alt+S
            "#,
        );
        assert!(keymap.warnings().is_empty());
        let chords = keymap.overrides.get(&KeyAction::FsSlideshow).unwrap();
        assert_eq!(chords.len(), 2);
        assert_eq!(chords[0], Chord::new(true, false, true, KeyName::P));
        assert_eq!(chords[1], Chord::new(true, false, true, KeyName::S));
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
    fn legacy_fs_analysis_binding_is_dropped_after_rename() {
        // v2.0.0 改名: 旧 `FsAnalysis = Z` のカスタム割当は未知アクションとして無視され、
        // 新既定 (FsImageAnalysis=Shift+Z / FsZoomMode=Z) へ移行する (Z=ズーム衝突回避)。
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsAnalysis = Z
            "#,
        );
        assert!(
            keymap
                .warnings()
                .iter()
                .any(|w| w.contains("unknown key action") && w.contains("FsAnalysis")),
            "旧 FsAnalysis は未知アクション警告になる"
        );
        // どのアクションにも override は付かない (= 全員新既定へ)。
        assert!(!keymap.overrides.contains_key(&KeyAction::FsImageAnalysis));
        assert!(!keymap.overrides.contains_key(&KeyAction::FsZoomMode));
    }

    #[test]
    fn zoom_and_analysis_default_chords_and_triggers() {
        assert_eq!(KeyAction::FsZoomMode.trigger(), KeyTrigger::KeyHold);
        assert_eq!(KeyAction::FsImageAnalysis.trigger(), KeyTrigger::Press);
        assert_eq!(
            KeyAction::FsZoomMode.default_chords().iter().next(),
            Some(Chord::key(KeyName::Z))
        );
        assert_eq!(
            KeyAction::FsImageAnalysis.default_chords().iter().next(),
            Some(Chord::shift(KeyName::Z))
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
    fn rating_shortcut_labels_follow_effective_chords() {
        let keymap = Keymap::empty();
        assert_eq!(
            Keymap::rating_key_action(false, 0),
            Some(KeyAction::RatingItemClear)
        );
        assert_eq!(
            Keymap::rating_key_action(true, 5),
            Some(KeyAction::RatingContainer5)
        );
        assert_eq!(
            keymap.first_rating_chord_label(false, 3).as_deref(),
            Some("F3")
        );
        assert_eq!(
            keymap.first_rating_chord_label(true, 0).as_deref(),
            Some("Shift+F6")
        );
        assert_eq!(
            keymap.rating_chord_summary_label(false).as_deref(),
            Some("F1〜F6")
        );
        assert_eq!(
            keymap.rating_chord_summary_label(true).as_deref(),
            Some("Shift+F1〜F6")
        );

        let keymap = Keymap::from_ini_str(
            r#"
            [Rating]
            RatingItem3 = Ctrl+Alt+3
            RatingContainer1 = Alt+F1
            RatingContainerClear = none
            "#,
        );
        assert_eq!(
            keymap.first_rating_chord_label(false, 3).as_deref(),
            Some("Ctrl+Alt+3")
        );
        assert_eq!(keymap.first_rating_chord_label(true, 0), None);
        assert_eq!(
            keymap.rating_chord_summary_label(true).as_deref(),
            Some("1:Alt+F1 / 2:Shift+F2 / 3:Shift+F3 / 4:Shift+F4 / 5:Shift+F5")
        );
    }

    #[test]
    fn numpad_names_parse_as_distinct_key_slots() {
        assert_eq!(KeyName::parse("Numpad1"), Some(KeyName::Numpad1));
        assert_eq!(KeyName::parse("Numpad0"), Some(KeyName::Numpad0));
        assert_eq!(KeyName::parse("NumpadEnter"), Some(KeyName::NumpadEnter));
        assert_eq!(KeyName::parse("NumEnter"), Some(KeyName::NumpadEnter));
        assert_eq!(KeyName::parse("Num1"), Some(KeyName::Num1));
        assert_eq!(KeyName::parse("1"), Some(KeyName::Num1));
        assert_eq!(KeyName::Numpad1.display_name(), "Numpad1");
        assert_eq!(KeyName::Numpad1.to_vk(), 0x61);
        assert_eq!(KeyName::NumpadEnter.display_name(), "NumpadEnter");
        assert_eq!(KeyName::NumpadEnter.to_vk(), 0x0D);
        assert_eq!(KeyName::NumpadEnter.to_egui(), None);
    }

    #[test]
    fn extended_function_keys_parse_display_and_roundtrip() {
        let cases = [
            (KeyName::F13, egui::Key::F13, 0x7C, "F13"),
            (KeyName::F14, egui::Key::F14, 0x7D, "F14"),
            (KeyName::F15, egui::Key::F15, 0x7E, "F15"),
            (KeyName::F16, egui::Key::F16, 0x7F, "F16"),
            (KeyName::F17, egui::Key::F17, 0x80, "F17"),
            (KeyName::F18, egui::Key::F18, 0x81, "F18"),
            (KeyName::F19, egui::Key::F19, 0x82, "F19"),
            (KeyName::F20, egui::Key::F20, 0x83, "F20"),
            (KeyName::F21, egui::Key::F21, 0x84, "F21"),
            (KeyName::F22, egui::Key::F22, 0x85, "F22"),
            (KeyName::F23, egui::Key::F23, 0x86, "F23"),
            (KeyName::F24, egui::Key::F24, 0x87, "F24"),
        ];
        for (name, egui_key, vk, label) in cases {
            assert_eq!(KeyName::parse(label), Some(name));
            assert_eq!(name.display_name(), label);
            assert_eq!(name.to_egui(), Some(egui_key));
            assert_eq!(KeyName::from_egui(egui_key), Some(name));
            assert_eq!(name.to_vk(), vk);
        }
        assert_eq!(parse_chord("Ctrl+F24").unwrap().display_name(), "Ctrl+F24");
        assert_eq!(
            parse_chord("Shift+F13").unwrap(),
            Chord::shift(KeyName::F13)
        );
    }

    #[test]
    fn punctuation_keys_parse_display_and_roundtrip() {
        let cases = [
            (KeyName::OpenBracket, egui::Key::OpenBracket, 0xDB, "["),
            (KeyName::CloseBracket, egui::Key::CloseBracket, 0xDD, "]"),
            (KeyName::Semicolon, egui::Key::Semicolon, 0xBB, ";"),
            (KeyName::Colon, egui::Key::Colon, 0xBA, ":"),
            (KeyName::Comma, egui::Key::Comma, 0xBC, ","),
            (KeyName::Period, egui::Key::Period, 0xBE, "."),
            (KeyName::Backslash, egui::Key::Backslash, 0xDC, "\\"),
            (KeyName::Slash, egui::Key::Slash, 0xBF, "/"),
            (KeyName::Minus, egui::Key::Minus, 0xBD, "-"),
        ];
        for (name, egui_key, vk, label) in cases {
            assert_eq!(KeyName::parse(label), Some(name));
            assert_eq!(name.display_name(), label);
            assert_eq!(name.to_egui(), Some(egui_key));
            assert_eq!(KeyName::from_egui(egui_key), Some(name));
            assert_eq!(name.to_vk(), vk);
        }
        assert_eq!(KeyName::parse("Semicolon"), Some(KeyName::Semicolon));
        assert_eq!(KeyName::parse("Colon"), Some(KeyName::Colon));
        assert_eq!(KeyName::parse("Comma"), Some(KeyName::Comma));
        assert_eq!(KeyName::parse("Period"), Some(KeyName::Period));
        assert_eq!(KeyName::parse("Backslash"), Some(KeyName::Backslash));
        assert_eq!(KeyName::parse("Yen"), Some(KeyName::IntlYen));
        assert_eq!(KeyName::parse("￥"), Some(KeyName::IntlYen));
        assert_eq!(parse_chord("Ctrl+\\").unwrap().display_name(), "Ctrl+\\");
    }

    #[test]
    fn jis_keys_parse_and_do_not_use_egui_fallback() {
        let cases = [
            (KeyName::JisCaret, "^", 0xDE),
            (KeyName::JisAt, "@", 0xC0),
            (KeyName::IntlYen, "￥", 0xDC),
            (KeyName::IntlRo, "＼", 0xE2),
        ];
        for (name, label, vk) in cases {
            assert_eq!(name.display_name(), label);
            assert_eq!(name.to_egui(), None);
            assert_eq!(name.to_vk(), vk);
        }
        assert_eq!(KeyName::parse("JisCaret"), Some(KeyName::JisCaret));
        assert_eq!(KeyName::parse("JisAt"), Some(KeyName::JisAt));
        assert_eq!(KeyName::parse("IntlYen"), Some(KeyName::IntlYen));
        assert_eq!(KeyName::parse("IntlRo"), Some(KeyName::IntlRo));
        assert_eq!(KeyName::parse("＼"), Some(KeyName::IntlRo));
        assert_eq!(KeyName::parse("ろ"), Some(KeyName::IntlRo));
    }

    #[test]
    fn key_slot_settings_names_parse_back_without_collisions() {
        let cases = [
            KeyName::Backslash,
            KeyName::IntlRo,
            KeyName::IntlYen,
            KeyName::JisAt,
            KeyName::JisCaret,
            KeyName::NumpadEnter,
            KeyName::Numpad1,
            KeyName::Num1,
        ];
        for key in cases {
            assert_eq!(KeyName::parse(key.settings_name()), Some(key));
        }
        assert_eq!(KeyName::IntlYen.settings_name(), "Yen");
        assert_eq!(KeyName::IntlRo.settings_name(), "Ro");
        assert_eq!(Chord::key(KeyName::IntlYen).display_name(), "￥");
        assert_eq!(Chord::key(KeyName::IntlYen).settings_name(), "Yen");
        assert_eq!(Chord::key(KeyName::IntlRo).display_name(), "＼");
        assert_eq!(Chord::key(KeyName::IntlRo).settings_name(), "Ro");
    }

    #[test]
    fn win32_key_slots_distinguish_numpad_and_jis_physical_keys() {
        assert_eq!(KeyName::from_win32(0x31, 0x02, false), Some(KeyName::Num1));
        assert_eq!(
            KeyName::from_win32(0x61, 0x4f, false),
            Some(KeyName::Numpad1)
        );
        assert!(KeyName::Num1.matches_win32(0x31, 0x02, false));
        assert!(!KeyName::Num1.matches_win32(0x61, 0x4f, false));
        assert!(KeyName::Numpad1.matches_win32(0x61, 0x4f, false));
        assert!(!KeyName::Numpad1.matches_win32(0x31, 0x02, false));
        assert_eq!(KeyName::from_win32(0x0D, 0x1c, false), Some(KeyName::Enter));
        assert_eq!(
            KeyName::from_win32(0x0D, 0x1c, true),
            Some(KeyName::NumpadEnter)
        );
        assert!(KeyName::Enter.matches_win32(0x0D, 0x1c, false));
        assert!(!KeyName::Enter.matches_win32(0x0D, 0x1c, true));
        assert!(KeyName::NumpadEnter.matches_win32(0x0D, 0x1c, true));
        assert!(!KeyName::NumpadEnter.matches_win32(0x0D, 0x1c, false));

        assert_eq!(
            KeyName::from_win32(0xDC, KeyName::INTL_YEN_SCAN, false),
            Some(KeyName::IntlYen)
        );
        assert!(KeyName::IntlYen.matches_win32(0xDC, KeyName::INTL_YEN_SCAN, false));
        assert!(!KeyName::Backslash.matches_win32(0xDC, KeyName::INTL_YEN_SCAN, false));
        assert_eq!(
            KeyName::from_win32(0xE2, KeyName::INTL_RO_SCAN, false),
            Some(KeyName::IntlRo)
        );
        assert_eq!(
            KeyName::from_win32(0xDE, KeyName::JIS_CARET_SCAN, false),
            Some(KeyName::JisCaret)
        );
        assert_eq!(
            KeyName::from_win32(0xC0, KeyName::JIS_AT_SCAN, false),
            Some(KeyName::JisAt)
        );
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

    #[test]
    fn effective_chord_labels_follow_defaults_overrides_and_none() {
        let keymap = Keymap::empty();
        assert_eq!(
            keymap
                .first_chord_label(KeyAction::GlobalLocalSearch)
                .as_deref(),
            Some("Ctrl+F")
        );
        assert_eq!(
            keymap.first_chord_label(KeyAction::GridToggleStackMode),
            None
        );

        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Ctrl+Shift+S
            GridPin = none
            "#,
        );
        assert_eq!(
            keymap
                .first_chord_label(KeyAction::GridToggleStackMode)
                .as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            keymap.first_chord_action_label("スタック", KeyAction::GridToggleStackMode),
            "スタック (Ctrl+Shift+S)"
        );
        assert_eq!(keymap.first_chord_label(KeyAction::GridPin), None);
        assert_eq!(
            keymap.first_chord_action_label("代表サムネイル", KeyAction::GridPin),
            "代表サムネイル"
        );
    }

    #[test]
    fn bracket_action_labels_follow_first_or_all_effective_chords() {
        let keymap = Keymap::empty();
        assert_eq!(
            keymap.first_chord_bracket_label("分析ツール", KeyAction::FsImageAnalysis),
            "分析ツール [Shift+Z]"
        );
        assert_eq!(
            keymap.chord_list_bracket_label("メタデータ", KeyAction::FsToggleMetadata),
            "メタデータ [I / Tab]"
        );

        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsToggleMetadata = Ctrl+I
            FsToggleMetadata.2 = M

            [FsImage]
            FsImageAnalysis = none
            "#,
        );
        assert_eq!(
            keymap.chord_list_bracket_label("メタデータ", KeyAction::FsToggleMetadata),
            "メタデータ [Ctrl+I / M]"
        );
        assert_eq!(
            keymap.first_chord_bracket_label("分析ツール", KeyAction::FsImageAnalysis),
            "分析ツール"
        );
    }

    #[test]
    fn compact_single_key_label_omits_modified_chords() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Erase]
            EraseToolBrush = Ctrl+B
            EraseToolBrush.2 = J
            EraseToolLasso = L
            "#,
        );
        assert_eq!(
            keymap.compact_single_key_label(KeyAction::EraseToolLasso),
            Some("L")
        );
        assert_eq!(
            keymap.compact_single_key_label(KeyAction::EraseToolBrush),
            None
        );
        assert_eq!(
            keymap.compact_action_label("筆", KeyAction::EraseToolLasso),
            "筆 [L]"
        );
        assert_eq!(
            keymap.compact_action_label("筆", KeyAction::EraseToolBrush),
            "筆"
        );
    }

    #[test]
    fn binding_conflicts_warn_for_same_scope_override() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Space
            "#,
        );
        let conflicts = keymap.binding_conflicts();
        assert!(conflicts.iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Hard
                && conflict.action == KeyAction::GridToggleCheck
                && conflict.other_action == Some(KeyAction::GridToggleStackMode)
                || conflict.kind == BindingConflictKind::Hard
                    && conflict.action == KeyAction::GridToggleStackMode
                    && conflict.other_action == Some(KeyAction::GridToggleCheck)
        }));
        assert!(keymap.warnings().iter().any(|warning| {
            warning.contains("GridToggleStackMode") && warning.contains("GridToggleCheck")
        }));
    }

    #[test]
    fn binding_conflicts_warn_for_active_overlap() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            GlobalOpenFolder = T
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::ActiveOverlap
                && conflict.chord == Chord::key(KeyName::T)
                && (conflict.action == KeyAction::GlobalOpenFolder
                    || conflict.other_action == Some(KeyAction::GlobalOpenFolder))
        }));
    }

    #[test]
    fn binding_conflicts_ignore_duplicate_chords_within_same_action() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsSlideshow.1 = Ctrl+Alt+S
            FsSlideshow.2 = Ctrl+Alt+S
            "#,
        );
        assert!(
            keymap.warnings().is_empty(),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );
        assert!(keymap.binding_conflicts().is_empty());
    }

    #[test]
    fn binding_conflicts_ignore_disjoint_scopes() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Erase]
            EraseToolBrush = Ctrl+J
            [Conceal]
            ConcealToolBrush = Ctrl+J
            "#,
        );
        assert!(
            keymap.warnings().is_empty(),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );
        assert!(keymap.binding_conflicts().is_empty());
    }

    #[test]
    fn binding_conflicts_warn_for_reserved_navigation_keys() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Esc
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Reserved
                && conflict.action == KeyAction::GridToggleStackMode
                && conflict.chord == Chord::key(KeyName::Esc)
        }));
        assert!(keymap.warnings().iter().any(|warning| {
            warning.contains("GridToggleStackMode") && warning.contains("reserved shortcut Esc")
        }));

        let keymap = Keymap::from_ini_str(
            r#"
            [Erase]
            EraseToolBrush = Left

            [Text]
            TextConfirm = Esc
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Reserved
                && conflict.action == KeyAction::EraseToolBrush
                && conflict.chord == Chord::key(KeyName::Left)
                && conflict.reserved_name == Some("plain arrow navigation")
        }));
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Reserved
                && conflict.action == KeyAction::TextConfirm
                && conflict.chord == Chord::key(KeyName::Esc)
                && conflict.reserved_name == Some("Escape navigation / cancel")
        }));
    }

    #[test]
    fn binding_conflicts_warn_for_grid_shift_arrow_range_selection_only_in_grid() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Shift+Left
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Reserved
                && conflict.action == KeyAction::GridToggleStackMode
                && conflict.chord == Chord::shift(KeyName::Left)
                && conflict.reserved_name == Some("grid range selection")
        }));
        assert!(keymap.warnings().iter().any(|warning| {
            warning.contains("GridToggleStackMode")
                && warning.contains("reserved shortcut Shift+Left")
                && warning.contains("grid range selection")
        }));

        let keymap = Keymap::from_ini_str(
            r#"
            [Text]
            TextConfirm = Shift+Left
            "#,
        );
        assert!(
            keymap
                .binding_conflicts()
                .iter()
                .all(|conflict| conflict.kind != BindingConflictKind::Reserved),
            "grid-only Shift+arrow reservation must not warn in text context: {:?}",
            keymap.binding_conflicts()
        );
    }

    #[test]
    fn enter_is_assignable_and_not_reserved() {
        let keymap = Keymap::from_ini_str(
            r#"
            [Grid]
            GridToggleStackMode = Enter
            "#,
        );
        assert!(keymap.binding_conflicts().iter().all(|conflict| {
            !(conflict.kind == BindingConflictKind::Reserved
                && conflict.chord == Chord::key(KeyName::Enter))
        }));
        assert!(
            keymap
                .warnings()
                .iter()
                .all(|warning| !warning.contains("reserved shortcut Enter")),
            "unexpected warnings: {:?}",
            keymap.warnings()
        );
    }

    #[test]
    fn binding_conflicts_warn_for_trigger_mismatch() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsImage]
            FsZoomMode = S
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::TriggerMismatch
                && conflict.chord == Chord::key(KeyName::S)
                && (conflict.action == KeyAction::FsZoomMode
                    || conflict.other_action == Some(KeyAction::FsZoomMode))
        }));
    }

    #[test]
    fn question_mark_chord_parses_and_displays_as_primary_help_key() {
        let question = Chord::shift(KeyName::Slash);
        assert_eq!(parse_chord("?").unwrap(), question);
        assert_eq!(parse_chord("Questionmark").unwrap(), question);
        assert_eq!(parse_chord("Shift+/").unwrap(), question);
        assert_eq!(question.display_name(), "?");
        assert_eq!(
            Chord::new(true, true, false, KeyName::Slash).display_name(),
            "Ctrl+?"
        );
        assert_eq!(
            KeyAction::HelpShowContextShortcuts
                .default_chords()
                .iter()
                .next(),
            Some(question)
        );
    }

    #[test]
    fn context_shortcuts_help_label_follows_overrides_and_none() {
        let keymap = Keymap::empty();
        assert_eq!(keymap.context_shortcuts_help_label(), "?");

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            HelpShowContextShortcuts = F1
            "#,
        );
        assert_eq!(keymap.context_shortcuts_help_label(), "F1");

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            HelpShowContextShortcuts = none
            "#,
        );
        assert_eq!(keymap.context_shortcuts_help_label(), "未設定");
    }

    #[cfg(windows)]
    #[test]
    fn native_video_context_help_keydown_follows_keymap() {
        let _guard = native_video_shortcut_test_guard();
        Keymap::empty().install_global_native_video_shortcuts();
        let mut event = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0xBF,
            scan_code: 0,
            extended: false,
            shift: true,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(native_video_context_shortcuts_help_key_down(&event));

        event.shift = false;
        assert!(!native_video_context_shortcuts_help_key_down(&event));
        event.shift = true;
        event.repeat = true;
        assert!(!native_video_context_shortcuts_help_key_down(&event));
        event.repeat = false;
        event.ctrl = true;
        assert!(!native_video_context_shortcuts_help_key_down(&event));

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            HelpShowContextShortcuts = F1
            "#,
        );
        keymap.install_global_native_video_shortcuts();

        let shift_slash = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0xBF,
            scan_code: 0,
            extended: false,
            shift: true,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(!native_video_context_shortcuts_help_key_down(&shift_slash));

        let f1 = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0x70,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(native_video_context_shortcuts_help_key_down(&f1));

        let keymap = Keymap::from_ini_str(
            r#"
            [Global]
            HelpShowContextShortcuts = none
            "#,
        );
        keymap.install_global_native_video_shortcuts();
        assert!(!native_video_context_shortcuts_help_key_down(&shift_slash));
        assert!(!native_video_context_shortcuts_help_key_down(&f1));
    }

    #[cfg(windows)]
    #[test]
    fn native_video_window_mode_shortcut_follows_keymap() {
        let _guard = native_video_shortcut_test_guard();
        let event = |virtual_key| crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };

        Keymap::empty().install_global_native_video_shortcuts();
        assert!(native_video_fullscreen_shortcut_key(&event(0x7A))); // F11

        let keymap = Keymap::from_ini_str(
            r#"
            [FsCommon]
            FsToggleWindowMode = F13
            "#,
        );
        keymap.install_global_native_video_shortcuts();
        assert!(!native_video_fullscreen_shortcut_key(&event(0x7A)));
        assert!(native_video_fullscreen_shortcut_key(&event(0x7C))); // F13

        let keymap = Keymap::from_ini_str(
            r#"
            [FsCommon]
            FsToggleWindowMode = none
            "#,
        );
        keymap.install_global_native_video_shortcuts();
        assert!(!native_video_fullscreen_shortcut_key(&event(0x7A)));
        assert!(!native_video_fullscreen_shortcut_key(&event(0x7C)));
    }

    #[cfg(windows)]
    #[test]
    fn native_video_fs_common_shortcuts_follow_keymap() {
        let _guard = native_video_shortcut_test_guard();
        let event = |virtual_key| crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        let alt_event = |virtual_key| crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: true,
            repeat: false,
        };

        Keymap::empty().install_global_native_video_shortcuts();
        assert!(native_video_fullscreen_shortcut_key(&event(0x08))); // Backspace
        assert!(native_video_fullscreen_shortcut_key(&event(0x24))); // Home
        assert!(native_video_fullscreen_shortcut_key(&event(0x23))); // End
        assert!(native_video_fullscreen_shortcut_key(&alt_event(0x43))); // Alt+C
        assert!(!native_video_fullscreen_shortcut_key(&alt_event(0x25))); // Alt+Left is not a fixed seek

        let keymap = Keymap::from_ini_str(
            r#"
            [FsCommon]
            FsBackToList = F13
            FsJumpFirst = F14
            FsJumpLast = none
            "#,
        );
        keymap.install_global_native_video_shortcuts();

        assert!(!native_video_fullscreen_shortcut_key(&event(0x08)));
        assert!(!native_video_fullscreen_shortcut_key(&event(0x24)));
        assert!(!native_video_fullscreen_shortcut_key(&event(0x23)));
        assert!(native_video_fullscreen_shortcut_key(&event(0x7C))); // F13
        assert!(native_video_fullscreen_shortcut_key(&event(0x7D))); // F14

        let keymap = Keymap::from_ini_str(
            r#"
            [FsVideo]
            VideoMute = Alt+Left
            "#,
        );
        keymap.install_global_native_video_shortcuts();
        assert!(native_video_fullscreen_shortcut_key(&alt_event(0x25)));
    }

    #[cfg(windows)]
    #[test]
    fn native_video_close_fullscreen_shortcut_follows_keymap() {
        let _guard = native_video_shortcut_test_guard();
        let event = |virtual_key| crate::video::native_window::NativeVideoKeyEvent {
            virtual_key,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };

        Keymap::empty().install_global_native_video_shortcuts();
        assert!(!native_video_fullscreen_shortcut_key(&event(0x7E))); // F15

        let keymap = Keymap::from_ini_str(
            r#"
            [FsVideo]
            VideoCloseFullscreen = F15
            "#,
        );
        keymap.install_global_native_video_shortcuts();
        assert!(native_video_fullscreen_shortcut_key(&event(0x7E)));
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
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(keymap.matches_vk_action(KeyAction::VideoMute, &event));
    }

    #[cfg(windows)]
    #[test]
    fn vk_match_supports_extended_function_keys() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsVideo]
            VideoMute = F16
            "#,
        );
        let event = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0x7F,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(keymap.matches_vk_action(KeyAction::VideoMute, &event));
    }

    #[cfg(windows)]
    #[test]
    fn native_video_key_slot_match_distinguishes_numpad_and_jis_keys() {
        let keymap = Keymap::from_ini_str(
            r#"
            [FsVideo]
            VideoMute = Numpad1
            VideoLoop = @
            "#,
        );
        let numpad1 = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0x61,
            scan_code: 0x4f,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        let digit1 = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0x31,
            scan_code: 0x02,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        let jis_at = crate::video::native_window::NativeVideoKeyEvent {
            virtual_key: 0xC0,
            scan_code: KeyName::JIS_AT_SCAN,
            extended: false,
            shift: false,
            ctrl: false,
            alt: false,
            repeat: false,
        };
        assert!(keymap.matches_vk_action(KeyAction::VideoMute, &numpad1));
        assert!(!keymap.matches_vk_action(KeyAction::VideoMute, &digit1));
        assert!(keymap.matches_vk_action(KeyAction::VideoLoop, &jis_at));
    }

    #[test]
    fn user_ini_template_is_parseable_and_comment_only() {
        let user_ini = Keymap::user_ini_template();
        assert!(user_ini.contains("キーボード割り当ての Action 名と既定キーの参照です。"));
        assert!(user_ini.contains("settings.db へ移行されます。"));
        assert!(user_ini.contains("F1..F24"));
        assert!(user_ini.contains("[FsImage]"));
        assert!(user_ini.contains("[Rating] ; レーティング"));
        assert!(user_ini.contains("# HelpShowContextShortcuts = ?"));
        assert!(user_ini.contains("# RatingItem1 = F1 ; 星1を付ける（アイテム）"));
        assert!(user_ini.contains("# GridToggleStackMode = none ; スタック表示を切り替える"));
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
        assert!(default_ini.contains("参照用です。このファイルは編集しないでください。"));
        assert!(default_ini.contains("[Rating] ; レーティング"));
        assert!(
            default_ini
                .contains("# RatingContainerClear = Shift+F6 ; レーティングを解除する（コンテナ）")
        );
        assert!(default_ini.contains("# GridToggleStackMode = none ; スタック表示を切り替える"));
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

    #[test]
    fn keymap_settings_roundtrip_overrides_and_none() {
        let keymap = Keymap::from_ini_str(
            "[Grid]\n\
             GridPin = F13\n\
             GridToggleStackMode = Ctrl+Shift+S\n\
             [FsImage]\n\
             FsSlideshow = none\n\
             FsPixelGrid = Ro\n",
        );
        let settings = KeymapSettings::from_keymap(&keymap);
        assert!(settings.legacy_ini_migration_done);
        assert_eq!(settings.overrides.len(), 4);
        assert!(settings.overrides.iter().any(|binding| {
            binding.action == "FsPixelGrid" && binding.chords == vec!["Ro".to_string()]
        }));

        let restored = Keymap::from_settings(&settings);
        assert_eq!(
            restored.effective_chords(KeyAction::GridPin),
            vec![Chord::key(KeyName::F13)]
        );
        assert_eq!(
            restored.effective_chords(KeyAction::GridToggleStackMode),
            vec![Chord::ctrl_shift(KeyName::S)]
        );
        assert!(restored.effective_chords(KeyAction::FsSlideshow).is_empty());
    }

    #[test]
    fn keymap_settings_edit_helpers_set_disable_and_restore_default() {
        let mut settings = KeymapSettings::default();
        settings.set_override_chords(
            KeyAction::GridToggleStackMode,
            vec![
                Chord::ctrl_shift(KeyName::S),
                Chord::key(KeyName::F13),
                Chord::key(KeyName::IntlYen),
            ],
        );
        assert_eq!(
            settings.override_chord_labels(KeyAction::GridToggleStackMode),
            Some(vec![
                "Ctrl+Shift+S".to_string(),
                "F13".to_string(),
                "￥".to_string()
            ])
        );
        assert!(settings.overrides.iter().any(|binding| {
            binding.action == "GridToggleStackMode" && binding.chords.contains(&"Yen".to_string())
        }));
        settings.set_override_chords(
            KeyAction::GridToggleStackMode,
            vec![Chord::key(KeyName::IntlRo)],
        );
        assert_eq!(
            settings.override_chord_labels(KeyAction::GridToggleStackMode),
            Some(vec!["＼".to_string()])
        );
        let keymap = Keymap::from_settings(&settings);
        assert_eq!(
            keymap.effective_chords(KeyAction::GridToggleStackMode),
            vec![Chord::key(KeyName::IntlRo)]
        );

        settings.disable_action(KeyAction::GridToggleStackMode);
        assert_eq!(
            settings.override_chord_labels(KeyAction::GridToggleStackMode),
            Some(Vec::new())
        );
        let keymap = Keymap::from_settings(&settings);
        assert!(
            keymap
                .effective_chords(KeyAction::GridToggleStackMode)
                .is_empty()
        );

        settings.remove_override(KeyAction::GridToggleStackMode);
        assert_eq!(
            settings.override_chord_labels(KeyAction::GridToggleStackMode),
            None
        );
        let keymap = Keymap::from_settings(&settings);
        assert!(
            keymap
                .effective_chords(KeyAction::GridToggleStackMode)
                .is_empty()
        );

        settings.set_override_chords(KeyAction::GridPin, vec![Chord::key(KeyName::F13)]);
        assert_eq!(
            Keymap::from_settings(&settings).effective_chords(KeyAction::GridPin),
            vec![Chord::key(KeyName::F13)]
        );
        settings.remove_override(KeyAction::GridPin);
        assert_eq!(
            Keymap::from_settings(&settings).effective_chords(KeyAction::GridPin),
            vec![Chord::key(KeyName::P)]
        );
    }

    #[test]
    fn parse_chord_for_action_validates_trigger_shape() {
        assert_eq!(
            parse_chord_for_action(KeyAction::GridPin, "F13"),
            Ok(Some(Chord::key(KeyName::F13)))
        );
        assert_eq!(parse_chord_for_action(KeyAction::GridPin, "none"), Ok(None));
        assert!(parse_chord_for_action(KeyAction::FsLoupeHold, "Ctrl+F13").is_err());
        assert!(parse_chord_for_action(KeyAction::EraseSpacePan, "Ctrl+Space").is_err());
    }

    #[test]
    fn keymap_settings_warn_and_ignore_invalid_entries() {
        let settings = KeymapSettings {
            overrides: vec![
                KeyBindingOverride {
                    action: "NoSuchAction".to_string(),
                    chords: vec!["F13".to_string()],
                },
                KeyBindingOverride {
                    action: "FsLoupeHold".to_string(),
                    chords: vec!["Ctrl+F13".to_string()],
                },
            ],
            legacy_ini_migration_done: true,
            legacy_ini_backup: None,
        };
        let keymap = Keymap::from_settings(&settings);
        assert!(keymap.override_chords(KeyAction::FsLoupeHold).is_none());
        assert!(
            keymap
                .warnings()
                .iter()
                .any(|warning| warning.contains("unknown key action"))
        );
        assert!(
            keymap
                .warnings()
                .iter()
                .any(|warning| warning.contains("ModifierHold actions"))
        );
    }

    #[test]
    fn legacy_keymap_ini_imports_once_and_renames_file_after_backup_request() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("keymap.ini");
        std::fs::write(
            &path,
            "[Grid]\nGridPin = F13\nGridToggleStackMode = Ctrl+Shift+S\n",
        )
        .unwrap();

        let mut settings = KeymapSettings::default();
        let result = settings.import_legacy_ini_if_needed(&path);
        assert!(result.changed);
        assert!(result.imported);
        assert!(result.backup_path.is_none());
        assert!(path.exists());
        assert!(settings.legacy_ini_migration_done);
        assert!(settings.legacy_ini_backup.is_none());

        let restored = Keymap::from_settings(&settings);
        assert_eq!(
            restored.effective_chords(KeyAction::GridPin),
            vec![Chord::key(KeyName::F13)]
        );
        assert_eq!(
            restored.effective_chords(KeyAction::GridToggleStackMode),
            vec![Chord::ctrl_shift(KeyName::S)]
        );

        let backup = settings.rename_imported_legacy_ini(&path);
        let backup_path = backup.backup_path.expect("backup path");
        assert!(backup.changed);
        assert!(!path.exists());
        assert!(backup_path.exists());
        assert!(backup_path.ends_with("keymap.ini.imported.bak"));
        assert_eq!(
            settings.legacy_ini_backup.as_deref(),
            Some(backup_path.to_string_lossy().as_ref())
        );

        let second = settings.import_legacy_ini_if_needed(&path);
        assert!(!second.changed);
        assert!(!second.imported);
    }

    #[test]
    fn legacy_keymap_ini_missing_marks_migration_done() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("keymap.ini");
        let mut settings = KeymapSettings::default();
        let result = settings.import_legacy_ini_if_needed(&path);
        assert!(result.changed);
        assert!(!result.imported);
        assert!(settings.legacy_ini_migration_done);
        assert!(settings.overrides.is_empty());
    }
}
