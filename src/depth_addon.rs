//! DA3 Mono-Large addon contract and worker-side installed-model resolution.
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::addon::{
    AddonKind, AddonStatus, FileKind, MAX_MODEL_BYTES, MAX_PART_BYTES, ModelAddon, ModelIndex,
    validate_hash_for_kind, validate_name_for_kind,
};

pub const MODEL_FILENAME: &str = "da3mono_large_fp16_504.onnx";

/// The depth contract deliberately accepts exactly one fixed ONNX plus its notices.
pub fn validate_index(index: &ModelIndex) -> Result<(), String> {
    if index.schema != 1 {
        return Err("Unsupported depth index schema".into());
    }
    validate_name_for_kind(AddonKind::Depth, &index.model_id)?;
    if index.files.len() != 3 {
        return Err("Depth index must contain ONNX, LICENSE and NOTICE".into());
    }
    let mut names: HashSet<String> = [AddonKind::Depth.index_name(), "INSTALL_OK", "active.json"]
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let mut total = 0u64;
    for file in &index.files {
        validate_name_for_kind(AddonKind::Depth, &file.name)?;
        validate_hash_for_kind(AddonKind::Depth, &file.sha256)?;
        if !names.insert(file.name.to_ascii_lowercase()) {
            return Err("Duplicate or reserved depth filename".into());
        }
        if file.bytes == 0 || file.bytes > MAX_MODEL_BYTES || file.license != "Apache-2.0" {
            return Err("Invalid depth file size or license".into());
        }
        total = total.checked_add(file.bytes).ok_or("Depth size overflow")?;
        if file.parts.is_empty() {
            if file.bytes > MAX_PART_BYTES {
                return Err("Depth release asset exceeds 2 GiB".into());
            }
        } else {
            if file.parts.len() > 32 || file.kind != FileKind::Model {
                return Err("Invalid depth split file".into());
            }
            let mut sum = 0u64;
            for part in &file.parts {
                validate_name_for_kind(AddonKind::Depth, &part.name)?;
                validate_hash_for_kind(AddonKind::Depth, &part.sha256)?;
                if !names.insert(part.name.to_ascii_lowercase()) {
                    return Err("Duplicate depth asset name".into());
                }
                if part.bytes == 0 || part.bytes > MAX_PART_BYTES {
                    return Err("Invalid depth part size".into());
                }
                sum = sum
                    .checked_add(part.bytes)
                    .ok_or("Depth part size overflow")?;
            }
            if sum != file.bytes {
                return Err("Depth part sizes do not equal file size".into());
            }
        }
    }
    if total > MAX_MODEL_BYTES {
        return Err("Depth model is too large".into());
    }
    for (kind, name) in [
        (FileKind::Model, MODEL_FILENAME),
        (FileKind::License, "LICENSE"),
        (FileKind::Notice, "NOTICE"),
    ] {
        if index
            .files
            .iter()
            .filter(|file| file.kind == kind && file.name == name)
            .count()
            != 1
        {
            return Err("Depth index requires the fixed ONNX, LICENSE and NOTICE filenames".into());
        }
    }
    let model = index.file(FileKind::Model);
    if index.model_id
        != format!(
            "da3mono-large-fp16-504-{}",
            &model.sha256[..12].to_ascii_lowercase()
        )
    {
        return Err("Depth model_id does not identify the model SHA-256".into());
    }
    Ok(())
}

pub fn addon_root() -> PathBuf {
    crate::data_dir::get().join("addons").join("depth")
}

/// Worker-only I/O. Half-installed packs and any mismatched indexed file are rejected.
/// UI callers use the App-owned DepthAddon status cache instead.
pub fn model_path() -> Option<PathBuf> {
    match crate::addon::scan_for_kind(AddonKind::Depth, &addon_root(), &AtomicBool::new(false)) {
        AddonStatus::Valid { model_path, .. } => Some(model_path),
        AddonStatus::Corrupt(error) => {
            crate::logger::log(format!("[Depth addon] {error}"));
            None
        }
        AddonStatus::Missing => None,
    }
}

/// Its kind cannot be changed through the public management API.
pub struct DepthAddon(ModelAddon);
impl Default for DepthAddon {
    fn default() -> Self {
        Self::new(addon_root())
    }
}
impl DepthAddon {
    pub fn new(root: PathBuf) -> Self {
        Self(ModelAddon::new_for_kind(AddonKind::Depth, root))
    }
    pub fn installed_model_path(&self) -> Option<&std::path::Path> {
        match self.status() {
            AddonStatus::Valid { model_path, .. } => Some(model_path),
            _ => None,
        }
    }
}
impl Deref for DepthAddon {
    type Target = ModelAddon;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for DepthAddon {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::addon::ModelFile;
    use crate::vlm_addon_download::tests::digest;

    pub(crate) fn fixture() -> (ModelIndex, Vec<Vec<u8>>) {
        let bodies = vec![
            b"onnx-model".to_vec(),
            b"license".to_vec(),
            b"notice".to_vec(),
        ];
        let files = [
            (MODEL_FILENAME, FileKind::Model),
            ("LICENSE", FileKind::License),
            ("NOTICE", FileKind::Notice),
        ]
        .into_iter()
        .zip(&bodies)
        .map(|((name, kind), body)| ModelFile {
            name: name.into(),
            kind,
            bytes: body.len() as u64,
            sha256: digest(body),
            license: "Apache-2.0".into(),
            parts: vec![],
        })
        .collect();
        (
            ModelIndex {
                schema: 1,
                model_id: format!("da3mono-large-fp16-504-{}", &digest(&bodies[0])[..12]),
                files,
            },
            bodies,
        )
    }

    #[test]
    fn depth_contract_rejects_vlm_extra_file_unsafe_names_and_bad_identity() {
        let (index, _) = fixture();
        validate_index(&index).unwrap();
        assert!(index.validate_for(AddonKind::Vlm).is_err());
        for change in 0..10 {
            let mut bad = index.clone();
            match change {
                0 => bad.files[0].name = "../escape.onnx".into(),
                1 => bad.files[0].name = "another.onnx".into(),
                2 => bad.model_id = "da3mono-large-fp16-504-000000000000".into(),
                3 => bad.files[0].license = "other".into(),
                4 => bad.files[0].sha256 = "x".repeat(64),
                5 => bad.files[1].kind = FileKind::Mmproj,
                6 => bad.files.push(bad.files[0].clone()),
                7 => bad.files[2].name = "active.json".into(),
                8 => {
                    bad.files.pop();
                }
                _ => bad.files[0].bytes = 0,
            }
            assert!(validate_index(&bad).is_err(), "change {change}");
        }
    }

    /// Checks a real installed tree, e.g. one made by `scripts/install-local-depth-addon.ps1`.
    /// `MIV_DEPTH_ADDON_ROOT` points at `<data_dir>/addons/depth`.
    #[test]
    #[ignore = "needs MIV_DEPTH_ADDON_ROOT with an installed depth add-on"]
    fn installed_tree_from_env_scans_valid() {
        let root = std::env::var_os("MIV_DEPTH_ADDON_ROOT").expect("MIV_DEPTH_ADDON_ROOT");
        let status = crate::addon::scan_for_kind(
            AddonKind::Depth,
            std::path::Path::new(&root),
            &AtomicBool::new(false),
        );
        assert!(
            matches!(status, AddonStatus::Valid { .. }),
            "unexpected status: {status:?}"
        );
    }
}
