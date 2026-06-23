use std::cmp::Ordering;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileNameSortKey {
    windows_key: Option<Vec<u8>>,
    folded: String,
    original: String,
}

impl FileNameSortKey {
    pub fn new(name: &str) -> Self {
        Self {
            windows_key: windows_file_name_sort_key(name),
            folded: name.to_lowercase(),
            original: name.to_string(),
        }
    }
}

impl Ord for FileNameSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let primary = match (&self.windows_key, &other.windows_key) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => self.folded.cmp(&other.folded),
        };
        primary
            .then_with(|| self.folded.cmp(&other.folded))
            .then_with(|| self.original.cmp(&other.original))
    }
}

impl PartialOrd for FileNameSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortNameKey {
    file_name: FileNameSortKey,
    natural: Option<Vec<crate::ui_helpers::NaturalChunk>>,
}

impl SortNameKey {
    pub fn file_name(name: &str) -> Self {
        Self {
            file_name: FileNameSortKey::new(name),
            natural: None,
        }
    }

    pub fn with_natural(name: &str) -> Self {
        Self {
            file_name: FileNameSortKey::new(name),
            natural: Some(crate::ui_helpers::natural_sort_key(name)),
        }
    }

    pub fn compare_file_name(&self, other: &Self) -> Ordering {
        self.file_name.cmp(&other.file_name)
    }

    pub fn compare_natural(&self, other: &Self) -> Ordering {
        let self_fallback;
        let self_key = match &self.natural {
            Some(key) => key,
            None => {
                self_fallback = crate::ui_helpers::natural_sort_key(&self.file_name.original);
                &self_fallback
            }
        };

        let other_fallback;
        let other_key = match &other.natural {
            Some(key) => key,
            None => {
                other_fallback = crate::ui_helpers::natural_sort_key(&other.file_name.original);
                &other_fallback
            }
        };

        self_key
            .cmp(other_key)
            .then_with(|| self.compare_file_name(other))
    }
}

pub fn compare_file_names(a: &str, b: &str) -> Ordering {
    SortNameKey::file_name(a).compare_file_name(&SortNameKey::file_name(b))
}

#[cfg(windows)]
fn windows_file_name_sort_key(name: &str) -> Option<Vec<u8>> {
    if name.is_empty() {
        return Some(Vec::new());
    }

    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Globalization::{
        LCMAP_SORTKEY, LCMapStringEx, NORM_IGNORECASE, NORM_IGNOREKANATYPE, NORM_IGNOREWIDTH,
        SORT_DIGITSASNUMBERS,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = name.encode_utf16().collect();
    let flags = LCMAP_SORTKEY
        | NORM_IGNORECASE.0
        | NORM_IGNOREWIDTH.0
        | NORM_IGNOREKANATYPE.0
        | SORT_DIGITSASNUMBERS.0;

    let needed =
        unsafe { LCMapStringEx(PCWSTR::null(), flags, &wide, None, None, None, LPARAM(0)) };
    if needed <= 0 {
        return None;
    }

    // LCMapStringEx returns sort keys as opaque bytes, but the windows crate
    // wrapper exposes the destination as [u16]. Allocate enough u16 slots and
    // copy only the returned byte count.
    let mut buf = vec![0_u16; needed as usize];
    let written = unsafe {
        LCMapStringEx(
            PCWSTR::null(),
            flags,
            &wide,
            Some(buf.as_mut_slice()),
            None,
            None,
            LPARAM(0),
        )
    };
    if written <= 0 {
        return None;
    }

    let bytes =
        unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, written as usize).to_vec() };
    Some(bytes)
}

#[cfg(not(windows))]
fn windows_file_name_sort_key(_name: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_key_is_case_insensitive_with_deterministic_tiebreak() {
        let upper = SortNameKey::file_name("BETA.jpg");
        let lower = SortNameKey::file_name("alpha.jpg");
        assert_eq!(lower.compare_file_name(&upper), Ordering::Less);
    }

    #[cfg(windows)]
    #[test]
    fn file_name_key_uses_windows_digit_sorting() {
        let mut names = vec!["file10.jpg", "file2.jpg", "file1.jpg"];
        names.sort_by(|a, b| {
            SortNameKey::file_name(a).compare_file_name(&SortNameKey::file_name(b))
        });
        assert_eq!(names, vec!["file1.jpg", "file2.jpg", "file10.jpg"]);
    }

    #[test]
    fn file_name_key_mixed_windows_fallback_keeps_total_order() {
        let mut keys = [
            FileNameSortKey {
                windows_key: Some(vec![2]),
                folded: "z".to_string(),
                original: "z".to_string(),
            },
            FileNameSortKey {
                windows_key: None,
                folded: "a".to_string(),
                original: "a".to_string(),
            },
            FileNameSortKey {
                windows_key: Some(vec![1]),
                folded: "m".to_string(),
                original: "m".to_string(),
            },
        ];
        keys.sort();
        assert_eq!(
            keys.iter()
                .map(|key| key.original.as_str())
                .collect::<Vec<_>>(),
            vec!["m", "z", "a"]
        );
    }

    #[test]
    fn natural_key_remains_separate_from_file_name_key() {
        let hash = SortNameKey::file_name("#1.jpg");
        let plain = SortNameKey::file_name("1.jpg");
        assert_eq!(
            crate::ui_helpers::natural_sort_key("#1.jpg"),
            crate::ui_helpers::natural_sort_key("1.jpg")
        );
        assert_ne!(hash.compare_file_name(&plain), Ordering::Equal);
    }
}
