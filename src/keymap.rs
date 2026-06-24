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
    Minus,
}

impl KeyName {
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
            "MINUS" => KeyName::Minus,
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
            KeyName::Minus => egui::Key::Minus,
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
            KeyName::Minus => 0xBD,
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
            KeyName::Minus => "-",
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
    ToggleDetachedViewerMode,
    GridSelectAll,
    GridDeselect,
    GridToggleCheck,
    GridDelete,
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
    FsCtrlNavPrev,
    FsCtrlNavNext,
    FsSiblingPrev,
    FsSiblingNext,
    FsFixedJumpPrev,
    FsFixedJumpNext,
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
    VideoAddToActiveBook,
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
    KeyAction::ToggleDetachedViewerMode,
    KeyAction::GridSelectAll,
    KeyAction::GridDeselect,
    KeyAction::GridToggleCheck,
    KeyAction::GridDelete,
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
    KeyAction::FsCtrlNavPrev,
    KeyAction::FsCtrlNavNext,
    KeyAction::FsSiblingPrev,
    KeyAction::FsSiblingNext,
    KeyAction::FsFixedJumpPrev,
    KeyAction::FsFixedJumpNext,
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
    KeyAction::VideoAddToActiveBook,
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
            ToggleDetachedViewerMode => "ToggleDetachedViewerMode",
            GridSelectAll => "GridSelectAll",
            GridDeselect => "GridDeselect",
            GridToggleCheck => "GridToggleCheck",
            GridDelete => "GridDelete",
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
            FsCtrlNavPrev => "FsCtrlNavPrev",
            FsCtrlNavNext => "FsCtrlNavNext",
            FsSiblingPrev => "FsSiblingPrev",
            FsSiblingNext => "FsSiblingNext",
            FsFixedJumpPrev => "FsFixedJumpPrev",
            FsFixedJumpNext => "FsFixedJumpNext",
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
            VideoAddToActiveBook => "VideoAddToActiveBook",
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

    pub fn description(self) -> &'static str {
        use KeyAction::*;
        match self {
            GlobalLocalSearch => "現在地の一覧を絞り込み検索する",
            GlobalFavSearch => "お気に入りフォルダを横断検索する",
            GlobalMetadataSearch => "全フォルダのメタデータを検索する",
            GlobalOpenFolder => "フォルダを開くダイアログを表示する",
            ToggleDetachedViewerMode => "画像・動画ビューアの別ウィンドウモードを切り替える",
            GridSelectAll => "表示中のチェック可能な項目をすべてチェックする",
            GridDeselect => "チェックをすべて解除する",
            GridToggleCheck => "選択中の項目のチェックを切り替える",
            GridDelete => "選択中またはチェック済みの実ファイル/実フォルダを削除する",
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
            FsCtrlNavPrev => "前のフォルダまたは検索結果へ移動する",
            FsCtrlNavNext => "次のフォルダまたは検索結果へ移動する",
            FsSiblingPrev => "前の兄弟フォルダへ移動する",
            FsSiblingNext => "次の兄弟フォルダへ移動する",
            FsFixedJumpPrev => "設定した量だけ前へジャンプする",
            FsFixedJumpNext => "設定した量だけ先へジャンプする",
            FsStackJumpPrev => "前のスタックの先頭画像へジャンプする",
            FsStackJumpNext => "次のスタックの先頭画像へジャンプする",
            RatingItem1 => "現在の画像または動画に星1を付ける",
            RatingItem2 => "現在の画像または動画に星2を付ける",
            RatingItem3 => "現在の画像または動画に星3を付ける",
            RatingItem4 => "現在の画像または動画に星4を付ける",
            RatingItem5 => "現在の画像または動画に星5を付ける",
            RatingItemClear => "現在の画像または動画のレーティングを解除する",
            RatingContainer1 => "現在のフォルダまたはZIP/PDF本体に星1を付ける",
            RatingContainer2 => "現在のフォルダまたはZIP/PDF本体に星2を付ける",
            RatingContainer3 => "現在のフォルダまたはZIP/PDF本体に星3を付ける",
            RatingContainer4 => "現在のフォルダまたはZIP/PDF本体に星4を付ける",
            RatingContainer5 => "現在のフォルダまたはZIP/PDF本体に星5を付ける",
            RatingContainerClear => "現在のフォルダまたはZIP/PDF本体のレーティングを解除する",
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
            FsDenoiseCycle => "デノイズ設定を切り替える",
            FsPostFilterNext => "ポストフィルタを次へ切り替える",
            FsPostFilterPrev => "ポストフィルタを前へ切り替える",
            FsPostFilterReset => "ポストフィルタを標準に戻す",
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
            VideoPlayPause => "動画の再生または一時停止を切り替える",
            VideoSeekStart => "動画の先頭へ移動して再生する",
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
            | ToggleDetachedViewerMode => KeyContext::Global,
            GridSelectAll
            | GridDeselect
            | GridToggleCheck
            | GridDelete
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
            | FsFixedJumpPrev
            | FsFixedJumpNext
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
            | VideoBookmark | VideoCapture | VideoAddToActiveBook | VideoCompareToggle
            | VideoCompareCycle | VideoCompareWipe | VideoCompareDiff => KeyContext::FsVideo,
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
            EraseSpacePan | ConcealSpacePan | CropSpacePan | TextSpacePan | LaSpacePan
            | FsZoomMode => KeyTrigger::KeyHold,
            GlobalLocalSearch
            | GlobalFavSearch
            | GlobalMetadataSearch
            | GlobalOpenFolder
            | ToggleDetachedViewerMode
            | GridSelectAll
            | GridDeselect
            | GridToggleCheck
            | GridDelete
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
            | FsCtrlNavPrev
            | FsCtrlNavNext
            | FsSiblingPrev
            | FsSiblingNext
            | FsFixedJumpPrev
            | FsFixedJumpNext
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
            | VideoAddToActiveBook
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
            ToggleDetachedViewerMode => ChordList::one(Chord::key(F12)),
            GridSelectAll => ChordList::one(Chord::ctrl(A)),
            GridDeselect => ChordList::two(Chord::ctrl(D), Chord::ctrl_shift(A)),
            GridToggleCheck => ChordList::one(Chord::key(Space)),
            GridDelete => ChordList::one(Chord::key(Delete)),
            GridToggleFolderTreePane => ChordList::one(Chord::key(F)),
            GridToggleStackMode => ChordList::EMPTY,
            GridTagApply => ChordList::one(Chord::key(T)),
            GridTagView => ChordList::one(Chord::ctrl(T)),
            GridRotateCw => ChordList::one(Chord::key(R)),
            GridRotateCcw => ChordList::one(Chord::key(L)),
            GridPin => ChordList::one(Chord::key(P)),
            GridComparePin => ChordList::one(Chord::key(X)),
            GridAddToActiveBook => ChordList::one(Chord::ctrl(B)),
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
            GridToggleDetailsView => ChordList::one(Chord::alt(Minus)),
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
            GridApplyErase1 => ChordList::one(Chord::key(F7)),
            GridApplyErase2 => ChordList::one(Chord::key(F8)),
            GridApplyConceal1 => ChordList::one(Chord::key(F9)),
            GridApplyConceal2 => ChordList::one(Chord::key(F10)),
            GridDeleteEraseMask => ChordList::two(Chord::shift(F7), Chord::shift(F8)),
            GridDeleteConcealMask => ChordList::two(Chord::shift(F9), Chord::shift(F10)),
            FsToggleMetadata => ChordList::two(Chord::key(I), Chord::key(Tab)),
            FsCtrlNavPrev => ChordList::one(Chord::ctrl(Up)),
            FsCtrlNavNext => ChordList::one(Chord::ctrl(Down)),
            FsSiblingPrev => ChordList::one(Chord::ctrl(PageUp)),
            FsSiblingNext => ChordList::one(Chord::ctrl(PageDown)),
            FsFixedJumpPrev => ChordList::one(Chord::shift(Left)),
            FsFixedJumpNext => ChordList::one(Chord::shift(Right)),
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
            VideoAddToActiveBook => ChordList::one(Chord::ctrl(B)),
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
    &[KeyContext::Global, KeyContext::Grid, KeyContext::Rating],
    FS_IMAGE_ACTIVE_SCOPES,
    FS_VIDEO_ACTIVE_SCOPES,
    &[KeyContext::Global, KeyContext::FsCommon, KeyContext::Erase],
    &[
        KeyContext::Global,
        KeyContext::FsCommon,
        KeyContext::Conceal,
    ],
    &[KeyContext::Global, KeyContext::FsCommon, KeyContext::Crop],
    &[KeyContext::Global, KeyContext::FsCommon, KeyContext::Text],
    &[
        KeyContext::Global,
        KeyContext::FsCommon,
        KeyContext::LocalAdjust,
    ],
];

pub fn command_scopes_overlap(a: CommandScope, b: CommandScope) -> bool {
    a == b
        || ACTIVE_SCOPE_SETS
            .iter()
            .any(|set| set.contains(&a) && set.contains(&b))
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
        chord: Chord::key(KeyName::Enter),
        name: "Enter navigation / confirm",
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
];

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
                    || !command_scopes_overlap(first.scope, second.scope)
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
            .filter(|binding| binding.customized)
        {
            for reserved in RESERVED_BINDINGS {
                if binding.chord == reserved.chord
                    && command_scopes_overlap(binding.scope, reserved.scope)
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

    pub fn consume_action_no_repeat(&self, ctx: &egui::Context, action: KeyAction) -> bool {
        debug_assert_eq!(action.trigger(), KeyTrigger::Press);
        if let Some(chords) = self.overrides.get(&action) {
            for chord in chords.iter().copied() {
                if self.consume_chord_no_repeat(ctx, chord) {
                    return true;
                }
            }
            return false;
        }
        for chord in action.default_chords().iter() {
            if self.consume_chord_no_repeat(ctx, chord) {
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

    /// KeyHold アクションに割り当てられたキーについて、このフレームの egui Key イベント
    /// (押下 / 離し) を取り出して消費する。戻り値 = (押下イベントあり, 離しイベントあり)。
    /// OS 直読みのエッジ (`key_held_action` のフレーム間差分) では取りこぼす高速タップ
    /// (idle からの同フレーム 押下+離し) を救うために併用する。修飾キー付きは対象外。
    pub fn take_key_hold_edges(&self, ctx: &egui::Context, action: KeyAction) -> (bool, bool) {
        debug_assert_eq!(action.trigger(), KeyTrigger::KeyHold);
        let keys: Vec<egui::Key> = if let Some(chords) = self.overrides.get(&action) {
            chords
                .iter()
                .filter_map(|c| c.key.map(KeyName::to_egui))
                .collect()
        } else {
            action
                .default_chords()
                .iter()
                .filter_map(|c| c.key.map(KeyName::to_egui))
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
        self.matches_vk_action_parts(action, key.virtual_key, key.ctrl, key.shift, key.alt)
    }

    pub fn install_global_native_video_shortcuts(&self) {
        let mut chords = Vec::new();
        for action in KeyAction::all().iter().copied().filter(|action| {
            matches!(action.context(), KeyContext::FsVideo | KeyContext::Rating)
                || *action == KeyAction::ToggleDetachedViewerMode
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
        out.push_str("# 上級者向けのキーボード割り当て設定です。\n");
        match kind {
            IniTemplateKind::UserConfig => {
                out.push_str(
                    "# %APPDATA%\\mimageviewer\\keymap.ini が無いときに自動生成されます。\n",
                );
                out.push_str(
                    "# 変更したい行だけ先頭の # を外し、= の右側のキーを編集してください。\n",
                );
                out.push_str(
                    "# コメントアウトされたままの行は、アプリ内蔵の既定キーを使います。\n",
                );
                out.push_str(
                    "# 更新後の最新の標準一覧は keymap.ini.default を参照してください。\n",
                );
            }
            IniTemplateKind::DefaultReference => {
                out.push_str("# 参照用です。このファイルは編集しないでください。\n");
                out.push_str(
                    "# アプリ内蔵の既定キーが変わると mimageviewer がこのファイルを上書きします。\n",
                );
                out.push_str(
                    "# 変更したい行は keymap.ini へコピーするか、keymap.ini 側の同じ行を編集してください。\n",
                );
            }
        }
        out.push_str("# keymap.ini は起動時に 1 回だけ読み込まれます。\n");
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
            "# - Esc / Enter / 修飾なし矢印キーは予約扱いです。割り当てても警告ログを出します。\n",
        );
        out.push_str("# - 行末の ; 以降は説明コメントです。コメント解除後も残してかまいません。\n");
        out.push_str("# - 競合は拒否しません。競合時は先に判定された操作が有効になります。\n");
        out.push_str("# - 通常の押下操作は Ctrl/Shift/Alt + 通常キーを指定できます。\n");
        out.push_str("# - ModifierHold は Ctrl / Shift / Alt のいずれか 1 つだけ指定できます。\n");
        out.push_str("# - KeyHold は修飾キーなしの通常キー 1 つだけ指定できます。\n");
        out.push_str("# - キー名の例: A..Z, 0..9, F1..F12, Left, Right, Up, Down,\n");
        out.push_str(
            "#   Home, End, PageUp, PageDown, Space, Enter, Esc, Tab, Backspace, Delete, [, ], -\n",
        );
        out.push_str("# - テンキー数字は通常の数字キーと同じ扱いです。\n");
        out.push_str("#   Numpad1 などの名前は受け付けますが、1 の別キーとしては使えません。\n");
        out.push_str(
            "# - Alt+F4 / Alt+Tab / Alt+Esc / Alt+Space / Ctrl+Alt+Del / Win キー系など、\n",
        );
        out.push_str("#   OS が予約しているショートカットは keymap.ini では上書きできません。\n");
        out.push_str(
            "# - native 動画フルスクリーンでは Alt を含む組み合わせはアプリ側へ転送されません。\n",
        );
        out.push_str(
            "# - マウス、ゲームパッド、ドラッグ&ドロップ、OS/egui のコピー/切り取り/貼り付け、\n",
        );
        out.push_str("#   IME 確定、右クリックメニュー、Escape/Enter ナビゲーション、多くの矢印ナビゲーションは固定です。\n");
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
        out.push_str("# RatingItem1 = Ctrl+F1  ; 現在の画像または動画に星1を付ける\n");
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
                .filter(|action| action.context() == section)
            {
                let description = action.description();
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

    fn consume_chord_inner(&self, ctx: &egui::Context, chord: Chord, allow_repeat: bool) -> bool {
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
        .filter(|action| {
            matches!(action.context(), KeyContext::FsVideo | KeyContext::Rating)
                || *action == KeyAction::ToggleDetachedViewerMode
        })
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
            | 0x7B // F12
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
        assert!(command_scopes_overlap(
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
    fn grid_toggle_stack_mode_is_default_unassigned() {
        assert!(KeyAction::GridToggleStackMode.default_chords().is_empty());
        assert_eq!(KeyAction::GridToggleStackMode.context(), KeyContext::Grid);
        assert_eq!(KeyAction::GridToggleStackMode.trigger(), KeyTrigger::Press);
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
            GridToggleStackMode = Enter
            "#,
        );
        assert!(keymap.binding_conflicts().iter().any(|conflict| {
            conflict.kind == BindingConflictKind::Reserved
                && conflict.action == KeyAction::GridToggleStackMode
                && conflict.chord == Chord::key(KeyName::Enter)
        }));
        assert!(keymap.warnings().iter().any(|warning| {
            warning.contains("GridToggleStackMode") && warning.contains("reserved shortcut Enter")
        }));
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
        assert!(user_ini.contains("上級者向けのキーボード割り当て設定です。"));
        assert!(user_ini.contains("[FsImage]"));
        assert!(user_ini.contains("[Rating] ; レーティング"));
        assert!(user_ini.contains("# RatingItem1 = F1 ; 現在の画像または動画に星1を付ける"));
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
        assert!(default_ini.contains(
            "# RatingContainerClear = Shift+F6 ; 現在のフォルダまたはZIP/PDF本体のレーティングを解除する"
        ));
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
}
