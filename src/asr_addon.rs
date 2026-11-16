//! Whisper large-v3-turbo Q5_0 addon contract and worker-side model resolution.
//! Transfer, resumable downloads and publication use the shared model-addon owner.

use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use crate::addon::{
    AddonEvent, AddonOperation, AddonStatus, FileKind, InstallProgress, ModelFile, ModelIndex,
    ModelPart,
};
use crate::addon::{
    AddonKind, MAX_PART_BYTES, ModelAddon, validate_hash_for_kind, validate_name_for_kind,
};

/// A single GGML Q5_0 model plus its MIT license and notice, with no split assets.
pub fn validate_index(index: &ModelIndex) -> Result<(), String> {
    if index.schema != 1 {
        return Err("Unsupported ASR index schema".into());
    }
    validate_name_for_kind(AddonKind::Asr, &index.model_id)?;
    if index.files.len() != 3 {
        return Err("ASR index must contain model, LICENSE and NOTICE".into());
    }
    let mut names: HashSet<String> = [AddonKind::Asr.index_name(), "INSTALL_OK", "active.json"]
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    for file in &index.files {
        validate_name_for_kind(AddonKind::Asr, &file.name)?;
        validate_hash_for_kind(AddonKind::Asr, &file.sha256)?;
        if !names.insert(file.name.to_ascii_lowercase()) {
            return Err("Duplicate or reserved ASR filename".into());
        }
        if file.bytes == 0 || file.bytes > MAX_PART_BYTES || file.license != "MIT" {
            return Err("Invalid ASR file size or license".into());
        }
        if !file.parts.is_empty() {
            return Err("ASR files must not be split".into());
        }
    }
    for kind in [FileKind::Model, FileKind::License, FileKind::Notice] {
        if index.files.iter().filter(|file| file.kind == kind).count() != 1 {
            return Err("Duplicate or missing ASR file kind".into());
        }
    }
    let model = index.file(FileKind::Model);
    if !model.name.starts_with("ggml-") || !model.name.ends_with("-q5_0.bin") {
        return Err("ASR model must be a GGML Q5_0 .bin file".into());
    }
    if index.model_id
        != format!(
            "whisper-large-v3-turbo-q5_0-{}",
            &model.sha256[..12].to_ascii_lowercase()
        )
    {
        return Err("ASR model_id does not identify the model SHA-256".into());
    }
    if index.file(FileKind::License).name != "LICENSE"
        || index.file(FileKind::Notice).name != "NOTICE"
    {
        return Err("ASR license files must be named LICENSE and NOTICE".into());
    }
    Ok(())
}

pub fn addon_root() -> PathBuf {
    crate::data_dir::get().join("addons").join("asr")
}

/// Worker-only I/O: verifies the pointer, INSTALL_OK and every indexed file.
/// UI callers use the cached AsrAddon status instead.
pub fn model_path() -> Option<PathBuf> {
    match crate::addon::scan_for_kind(AddonKind::Asr, &addon_root(), &AtomicBool::new(false)) {
        AddonStatus::Valid { model_path, .. } => Some(model_path),
        AddonStatus::Corrupt(error) => {
            crate::logger::log(format!("[ASR addon] {error}"));
            None
        }
        AddonStatus::Missing => None,
    }
}

/// The shared nonblocking lifecycle owner, fixed to the ASR addon kind.
pub struct AsrAddon(ModelAddon);

impl Default for AsrAddon {
    fn default() -> Self {
        Self::new(addon_root())
    }
}

impl AsrAddon {
    pub fn new(root: PathBuf) -> Self {
        Self(ModelAddon::new_for_kind(AddonKind::Asr, root))
    }

    pub fn installed_model_path(&self) -> Option<&Path> {
        match self.status() {
            AddonStatus::Valid { model_path, .. } => Some(model_path),
            _ => None,
        }
    }
}

impl Deref for AsrAddon {
    type Target = ModelAddon;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AsrAddon {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::vlm_addon_download::tests::digest;

    pub(crate) fn fixture() -> (ModelIndex, Vec<Vec<u8>>) {
        let bodies = vec![
            b"ggml-model".to_vec(),
            b"license".to_vec(),
            b"notice".to_vec(),
        ];
        let files = [
            ("ggml-large-v3-turbo-q5_0.bin", FileKind::Model),
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
            license: "MIT".into(),
            parts: vec![],
        })
        .collect();
        (
            ModelIndex {
                schema: 1,
                model_id: format!("whisper-large-v3-turbo-q5_0-{}", &digest(&bodies[0])[..12]),
                files,
            },
            bodies,
        )
    }

    #[test]
    fn asr_contract_rejects_wrong_schema_assets_identity_and_licenses() {
        let (index, _) = fixture();
        index.validate_for(AddonKind::Asr).unwrap();
        assert!(index.validate_for(AddonKind::Vlm).is_err());
        assert!(index.validate_for(AddonKind::Depth).is_err());
        for change in 0..18 {
            let mut bad = index.clone();
            match change {
                0 => bad.schema = 2,
                1 => bad.files[0].name = "../ggml-model-q5_0.bin".into(),
                2 => bad.files[0].name = "ggml-model-q5_0.gguf".into(),
                3 => bad.files[0].name = "ggml-model-q8_0.bin".into(),
                4 => bad.model_id = "whisper-large-v3-turbo-q5_0-000000000000".into(),
                5 => bad.files[0].license = "Apache-2.0".into(),
                6 => bad.files[1].license = "other".into(),
                7 => bad.files[0].sha256 = "x".repeat(64),
                8 => bad.files[1].kind = FileKind::Mmproj,
                9 => bad.files.push(bad.files[0].clone()),
                10 => bad.files[2].name = "active.json".into(),
                11 => {
                    bad.files.pop();
                }
                12 => bad.files[0].bytes = 0,
                13 => bad.files[0].bytes = MAX_PART_BYTES + 1,
                14 => {
                    let file = &mut bad.files[0];
                    file.parts.push(ModelPart {
                        name: "model.part01".into(),
                        bytes: file.bytes,
                        sha256: file.sha256.clone(),
                    });
                }
                15 => bad.files[1].name = "license".into(),
                16 => bad.files[2].kind = FileKind::Model,
                _ => bad.files[0].name = "model-q5_0.bin".into(),
            }
            assert!(validate_index(&bad).is_err(), "change {change}");
        }
    }

    #[test]
    fn asr_contract_accepts_release_basename_and_hash_case() {
        let (mut index, _) = fixture();
        index.files[0].name = "ggml-whisper-large-v3-turbo-q5_0.bin".into();
        index.files[0].sha256 = index.files[0].sha256.to_ascii_uppercase();
        index.files[0].bytes = MAX_PART_BYTES;
        validate_index(&index).unwrap();
    }

    #[test]
    fn published_v1_index_validates() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/asr-model/asr-model-index.v1.json");
        let raw = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("Read published ASR index {}: {error}", path.display()));
        let index: ModelIndex = serde_json::from_slice(&raw).expect("published ASR index JSON");
        validate_index(&index).expect("published ASR v1 index contract");
    }
}
