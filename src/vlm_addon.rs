//! VLM-specific index contract. Transfer and publication are shared by addon kind.

pub use crate::addon::{
    AddonEvent, AddonOperation, AddonStatus, FileKind, InstallProgress, ModelFile, ModelIndex,
    ModelPart, VlmAddon,
};
use crate::addon::{INDEX_NAME, MAX_MODEL_BYTES, MAX_PART_BYTES, validate_hash, validate_name};
use std::collections::HashSet;

impl ModelIndex {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("Unsupported VLM index schema {}", self.schema));
        }
        validate_name(&self.model_id)?;
        if self.files.len() != 4 {
            return Err("VLM index must contain model, mmproj, LICENSE and NOTICE".into());
        }
        let mut names: HashSet<String> = [INDEX_NAME, "INSTALL_OK", "active.json"]
            .into_iter()
            .map(str::to_ascii_lowercase)
            .collect();
        let mut total = 0u64;
        for file in &self.files {
            validate_name(&file.name)?;
            if !names.insert(file.name.to_ascii_lowercase()) {
                return Err("Duplicate or reserved VLM filename".into());
            }
            validate_hash(&file.sha256)?;
            if file.bytes == 0 || file.bytes > MAX_MODEL_BYTES || file.license != "Apache-2.0" {
                return Err("Invalid VLM file size or license".into());
            }
            total = total.checked_add(file.bytes).ok_or("VLM size overflow")?;
            if file.parts.is_empty() {
                if file.bytes > MAX_PART_BYTES {
                    return Err("VLM release asset exceeds 2 GiB".into());
                }
            } else {
                if file.parts.len() > 32 || file.kind != FileKind::Model {
                    return Err("Invalid VLM split file".into());
                }
                let mut sum = 0u64;
                for part in &file.parts {
                    validate_name(&part.name)?;
                    validate_hash(&part.sha256)?;
                    if !names.insert(part.name.to_ascii_lowercase()) {
                        return Err("Duplicate VLM asset name".into());
                    }
                    if part.bytes == 0 || part.bytes > MAX_PART_BYTES {
                        return Err("Invalid VLM part size".into());
                    }
                    sum = sum
                        .checked_add(part.bytes)
                        .ok_or("VLM part size overflow")?;
                }
                if sum != file.bytes {
                    return Err("VLM part sizes do not equal file size".into());
                }
            }
        }
        if total > MAX_MODEL_BYTES {
            return Err("VLM model is too large".into());
        }
        for kind in [
            FileKind::Model,
            FileKind::Mmproj,
            FileKind::License,
            FileKind::Notice,
        ] {
            if self.files.iter().filter(|f| f.kind == kind).count() != 1 {
                return Err("Duplicate or missing VLM file kind".into());
            }
        }
        let model = self.file(FileKind::Model);
        if self.model_id
            != format!(
                "qwen3.5-4b-miv-q4km-{}",
                &model.sha256[..12].to_ascii_lowercase()
            )
        {
            return Err("VLM model_id does not identify the model SHA-256".into());
        }
        if self.file(FileKind::License).name != "LICENSE"
            || self.file(FileKind::Notice).name != "NOTICE"
        {
            return Err("VLM license files must be named LICENSE and NOTICE".into());
        }
        for kind in [FileKind::Model, FileKind::Mmproj] {
            if !self.file(kind).name.ends_with(".gguf") {
                return Err("VLM model files must be GGUF".into());
            }
        }
        // Check final filenames against parts appearing earlier in the list too.
        let mut storage = HashSet::new();
        for file in &self.files {
            for name in std::iter::once(&file.name).chain(file.parts.iter().map(|p| &p.name)) {
                if !storage.insert(name.to_ascii_lowercase()) {
                    return Err("VLM filename collision".into());
                }
            }
        }
        Ok(())
    }
}
