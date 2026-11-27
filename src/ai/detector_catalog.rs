//! Worker-only BYO detector discovery and strict Ultralytics format inspection.

use std::collections::HashMap;
use std::path::Path;

use ort::value::{TensorElementType, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::runtime::{ExternalModelKey, temporary_detector_catalog_session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectorFormat {
    UltralyticsDetect,
    UltralyticsSegment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetectorShape {
    Rect,
    Ellipse,
    Contour,
}

#[derive(Debug, Clone)]
pub(crate) struct DetectorInfo {
    pub(crate) key: ExternalModelKey,
    pub(crate) name: String,
    pub(crate) format: DetectorFormat,
    pub(crate) names: Vec<String>,
    pub(crate) thresholds: Vec<f32>,
    pub(crate) input_size: usize,
    pub(crate) shape: Option<DetectorShape>,
    pub(crate) license: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DetectorEntry {
    pub(crate) name: String,
    pub(crate) result: Result<DetectorInfo, DetectorReadError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetectorReadError {
    Io(String),
    Json(String),
    Model(String),
    Metadata(String),
    InputShape,
    OutputShape,
    ClassCount,
    EndToEnd,
}

impl DetectorReadError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Io(_) => "ファイルを読めません",
            Self::Json(_) => "JSON を読めません",
            Self::Model(_) => "ONNX を読めません",
            Self::Metadata(_) => "メタデータを読めません",
            Self::InputShape => "入力の形式を読めません",
            Self::OutputShape => "出力の形式を読めません",
            Self::ClassCount => "分類の数が出力と一致しません",
            Self::EndToEnd => "重なり除去済みの形式には対応していません",
        }
    }
}

impl std::fmt::Display for DetectorReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())?;
        match self {
            Self::Io(detail)
            | Self::Json(detail)
            | Self::Model(detail)
            | Self::Metadata(detail) => {
                write!(f, ": {detail}")
            }
            _ => Ok(()),
        }
    }
}

/// The caller must wait for the process runtime owner before calling this on a
/// worker. Missing detector folders are an empty catalog; no folder is created.
pub(crate) fn scan_detectors(folder: &Path) -> Result<Vec<DetectorEntry>, String> {
    scan_detectors_with(folder, &mut |path| {
        Ok(Box::new(SessionModel::load(path)?) as Box<dyn CatalogModel>)
    })
}

pub(crate) fn read_detector(path: &Path) -> Result<DetectorInfo, DetectorReadError> {
    let placement = if path.file_stem().is_some_and(|stem| stem == "model") {
        Placement::Package
    } else {
        Placement::File
    };
    read_detector_with(path, detector_name(path), placement, &mut |path| {
        Ok(Box::new(SessionModel::load(path)?) as Box<dyn CatalogModel>)
    })
}

type ModelLoader<'a> = dyn FnMut(&Path) -> Result<Box<dyn CatalogModel>, DetectorReadError> + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Placement {
    File,
    Package,
}

fn scan_detectors_with(
    folder: &Path,
    loader: &mut ModelLoader<'_>,
) -> Result<Vec<DetectorEntry>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        if ty.is_file() && is_onnx(&path) {
            let name = detector_name(&path);
            candidates.push((path, name, Placement::File));
        } else if ty.is_dir() {
            // One supported folder package, never recursively scan arbitrary trees.
            for child in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                let child = child.map_err(|e| e.to_string())?;
                if child.file_name() == "model.onnx"
                    && child.file_type().map_err(|e| e.to_string())?.is_file()
                {
                    candidates.push((
                        child.path(),
                        entry.file_name().to_string_lossy().into_owned(),
                        Placement::Package,
                    ));
                }
            }
        }
    }
    candidates.sort();
    Ok(candidates
        .into_iter()
        .map(|(path, name, placement)| DetectorEntry {
            name: name.clone(),
            result: read_detector_with(&path, name, placement, loader),
        })
        .collect())
}

fn is_onnx(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("onnx"))
}

fn detector_name(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone)]
struct ModelDescription {
    // None represents a non-fp32-tensor type, which is rejected below.
    inputs: Vec<Option<Vec<i64>>>,
    outputs: Vec<Option<Vec<i64>>>,
    metadata: HashMap<String, String>,
}

trait CatalogModel {
    fn description(&self) -> &ModelDescription;
    fn probe(&mut self) -> Result<Vec<Option<Vec<i64>>>, DetectorReadError>;
}

struct SessionModel {
    session: ort::session::Session,
    description: ModelDescription,
}

fn tensor_shape(ty: &ValueType) -> Option<Vec<i64>> {
    match ty {
        ValueType::Tensor {
            ty: TensorElementType::Float32,
            shape,
            ..
        } => Some(shape.to_vec()),
        _ => None,
    }
}

impl SessionModel {
    fn load(path: &Path) -> Result<Self, DetectorReadError> {
        let session = temporary_detector_catalog_session(path)
            .map_err(|e| DetectorReadError::Model(e.to_string()))?;
        let metadata = {
            let metadata = session
                .metadata()
                .map_err(|e| DetectorReadError::Metadata(e.to_string()))?;
            ["names", "task", "imgsz", "end2end", "nms", "license"]
                .into_iter()
                .filter_map(|key| metadata.custom(key).map(|value| (key.into(), value)))
                .collect()
        };
        let description = ModelDescription {
            inputs: session
                .inputs()
                .iter()
                .map(|input| tensor_shape(input.dtype()))
                .collect(),
            outputs: session
                .outputs()
                .iter()
                .map(|output| tensor_shape(output.dtype()))
                .collect(),
            metadata,
        };
        Ok(Self {
            session,
            description,
        })
    }
}

impl CatalogModel for SessionModel {
    fn description(&self) -> &ModelDescription {
        &self.description
    }

    fn probe(&mut self) -> Result<Vec<Option<Vec<i64>>>, DetectorReadError> {
        // Only used when neither metadata nor the static channel dimension gives
        // the class count. Reuse this inspection session exactly once.
        let input = ndarray::Array4::<f32>::from_elem((1, 3, 640, 640), 114.0 / 255.0);
        let input = ort::value::Tensor::from_array(input)
            .map_err(|e| DetectorReadError::Model(e.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| DetectorReadError::Model(e.to_string()))?;
        Ok(outputs
            .iter()
            .map(|(_, value)| tensor_shape(value.dtype()))
            .collect())
    }
}

fn optional_json(path: &Path) -> Result<Option<Value>, DetectorReadError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| DetectorReadError::Json(format!("{}: {e}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DetectorReadError::Io(format!(
            "{}: {error}",
            path.display()
        ))),
    }
}

fn json_error(field: &str) -> DetectorReadError {
    DetectorReadError::Json(field.into())
}

fn json_names(value: &Value) -> Result<Vec<String>, DetectorReadError> {
    let values = value
        .as_array()
        .ok_or_else(|| json_error("names/labels must be an array"))?;
    let names = values
        .iter()
        .map(|name| {
            name.as_str()
                .map(str::to_owned)
                .ok_or_else(|| json_error("names/labels must contain strings"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Err(json_error("names/labels must not be empty"));
    }
    Ok(names)
}

fn read_detector_with(
    path: &Path,
    name: String,
    placement: Placement,
    loader: &mut ModelLoader<'_>,
) -> Result<DetectorInfo, DetectorReadError> {
    let key = ExternalModelKey::read(path).map_err(|e| DetectorReadError::Io(e.to_string()))?;
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    // Read all relevant JSON even if a higher-priority source supplies the value.
    // A broken sidecar is visible to the user instead of silently ignored.
    let overrides = optional_json(&path.with_extension("json"))?;
    let (labels, threshold, artifacts) = if placement == Placement::Package {
        (
            optional_json(&folder.join("labels.json"))?,
            optional_json(&folder.join("threshold.json"))?,
            optional_json(&folder.join("model_artifacts.json"))?,
        )
    } else {
        (None, None, None)
    };
    if overrides.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(json_error("model override must be an object"));
    }
    let mut model = loader(&key.path)?;
    let desc = model.description().clone();
    let meta = &desc.metadata;
    for key in ["end2end", "nms"] {
        if let Some(value) = meta.get(key) {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => return Err(DetectorReadError::EndToEnd),
                "false" | "0" => (),
                _ => return Err(DetectorReadError::Metadata(format!("invalid {key}"))),
            }
        }
    }
    let input = match desc.inputs.as_slice() {
        [Some(shape)]
            if shape.len() == 4
                && matches!(shape[0], 1 | -1)
                && shape[1] == 3
                && shape[2..].iter().all(|&n| n > 0 || n == -1)
                && !(shape[2] > 0 && shape[3] > 0 && shape[2] != shape[3]) =>
        {
            shape
        }
        _ => return Err(DetectorReadError::InputShape),
    };
    let override_names = overrides
        .as_ref()
        .and_then(|v| v.get("names"))
        .map(json_names)
        .transpose()?;
    let onnx_names = meta
        .get("names")
        .map(|value| parse_python_names(value))
        .transpose()?;
    let label_names = labels.as_ref().map(json_names).transpose()?;
    let names = override_names.or(onnx_names).or(label_names);
    let task = match meta.get("task").map(|s| s.trim()) {
        Some("detect") => Some(DetectorFormat::UltralyticsDetect),
        Some("segment") => Some(DetectorFormat::UltralyticsSegment),
        Some(_) => return Err(DetectorReadError::OutputShape),
        None => None,
    };
    let format = match (task, desc.outputs.len()) {
        (Some(DetectorFormat::UltralyticsDetect) | None, 1) => DetectorFormat::UltralyticsDetect,
        (Some(DetectorFormat::UltralyticsSegment) | None, 2) => DetectorFormat::UltralyticsSegment,
        _ => return Err(DetectorReadError::OutputShape),
    };
    let mut outputs = desc.outputs;
    validate_output_layout(&outputs, format)?;
    let extra = if format == DetectorFormat::UltralyticsSegment {
        36
    } else {
        4
    };
    let mut channels = outputs[0].as_ref().unwrap()[1];
    if channels == -1 {
        if let Some(names) = &names {
            channels = i64::try_from(names.len())
                .ok()
                .and_then(|n| n.checked_add(extra))
                .ok_or(DetectorReadError::ClassCount)?;
        } else {
            outputs = model.probe()?;
            validate_output_layout(&outputs, format)?;
            channels = outputs[0].as_ref().unwrap()[1];
        }
    }
    let classes = channels
        .checked_sub(extra)
        .filter(|n| *n > 0)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(DetectorReadError::OutputShape)?;
    if names.as_ref().is_some_and(|names| names.len() != classes) {
        return Err(DetectorReadError::ClassCount);
    }
    let names = names.unwrap_or_else(|| (0..classes).map(|n| format!("分類 {n}")).collect());
    let artifact_size = artifacts
        .as_ref()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| json_error("model_artifacts must be an object"))?;
            let Some(args) = object.get("args") else {
                return Ok(None);
            };
            let args = args
                .as_object()
                .ok_or_else(|| json_error("args must be an object"))?;
            args.get("imgsz")
                .map(|value| {
                    square_size(value).ok_or_else(|| json_error("args.imgsz must be square"))
                })
                .transpose()
        })
        .transpose()?
        .flatten();
    let fixed_size = input[2..]
        .iter()
        .copied()
        .find(|&n| n > 0)
        .map(usize::try_from)
        .transpose()
        .map_err(|_| DetectorReadError::InputShape)?;
    let input_size = if let Some(value) = meta.get("imgsz") {
        square_size(
            &serde_json::from_str::<Value>(value)
                .map_err(|e| DetectorReadError::Metadata(format!("imgsz: {e}")))?,
        )
        .ok_or_else(|| DetectorReadError::Metadata("imgsz must be square".into()))?
    } else {
        artifact_size.or(fixed_size).unwrap_or(640)
    };
    if input[2..]
        .iter()
        .any(|&n| n > 0 && usize::try_from(n).ok() != Some(input_size))
    {
        return Err(DetectorReadError::InputShape);
    }
    let package_threshold = match threshold.as_ref() {
        Some(value) => valid_threshold(
            value
                .get("threshold")
                .ok_or_else(|| json_error("missing threshold"))?,
        )?,
        None => 0.25,
    };
    let base_threshold = overrides
        .as_ref()
        .and_then(|value| value.get("threshold"))
        .map(valid_threshold)
        .transpose()?
        .unwrap_or(package_threshold);
    let mut thresholds = vec![base_threshold; classes];
    if let Some(values) = overrides.as_ref().and_then(|v| v.get("thresholds")) {
        let values = values
            .as_object()
            .ok_or_else(|| json_error("thresholds must be an object"))?;
        for (name, value) in values {
            let threshold = valid_threshold(value)?;
            if let Some(index) = names.iter().position(|candidate| candidate == name) {
                thresholds[index] = threshold;
            }
        }
    }
    let shape = overrides
        .as_ref()
        .and_then(|v| v.get("shape"))
        .map(|value| {
            serde_json::from_value::<DetectorShape>(value.clone())
                .map_err(|e| json_error(&e.to_string()))
        })
        .transpose()?;
    if format == DetectorFormat::UltralyticsDetect && shape == Some(DetectorShape::Contour) {
        return Err(json_error("contour shape requires segmentation"));
    }
    Ok(DetectorInfo {
        key,
        name,
        format,
        names,
        thresholds,
        input_size,
        shape,
        license: meta.get("license").cloned(),
    })
}

fn validate_output_layout(
    outputs: &[Option<Vec<i64>>],
    format: DetectorFormat,
) -> Result<(), DetectorReadError> {
    let expected = if format == DetectorFormat::UltralyticsSegment {
        2
    } else {
        1
    };
    if outputs.len() != expected {
        return Err(DetectorReadError::OutputShape);
    }
    let first = outputs[0].as_ref().ok_or(DetectorReadError::OutputShape)?;
    if first.len() != 3
        || !matches!(first[0], 1 | -1)
        || !(first[1] > 0 || first[1] == -1)
        || !(first[2] > 0 || first[2] == -1)
    {
        return Err(DetectorReadError::OutputShape);
    }
    if expected == 2 {
        let proto = outputs[1].as_ref().ok_or(DetectorReadError::OutputShape)?;
        if proto.len() != 4
            || !matches!(proto[0], 1 | -1)
            || proto[1] != 32
            || proto[2..].iter().any(|&n| n != -1 && n <= 0)
        {
            return Err(DetectorReadError::OutputShape);
        }
    }
    Ok(())
}

fn valid_threshold(value: &Value) -> Result<f32, DetectorReadError> {
    let n = value
        .as_f64()
        .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
        .ok_or_else(|| json_error("threshold must be in 0..=1"))?;
    Ok(n as f32)
}

fn square_size(value: &Value) -> Option<usize> {
    let n = match value {
        Value::Array(values) if values.len() == 2 && values[0] == values[1] => {
            values[0].as_u64()?
        }
        _ => value.as_u64()?,
    };
    usize::try_from(n).ok().filter(|n| *n > 0)
}

/// A bounded grammar for the Python dict exported by Ultralytics. No evaluation,
/// identifiers, expressions, nested structures or nonsequential indices.
fn parse_python_names(source: &str) -> Result<Vec<String>, DetectorReadError> {
    struct Parser<'a> {
        rest: &'a str,
    }
    impl Parser<'_> {
        fn ws(&mut self) {
            self.rest = self.rest.trim_start();
        }
        fn token(&mut self, token: char) -> bool {
            self.ws();
            if let Some(rest) = self.rest.strip_prefix(token) {
                self.rest = rest;
                true
            } else {
                false
            }
        }
        fn string(&mut self) -> Option<String> {
            self.ws();
            let quote = self.rest.chars().next()?;
            if !matches!(quote, '\'' | '"') {
                return None;
            }
            self.rest = &self.rest[quote.len_utf8()..];
            let mut result = String::new();
            loop {
                let c = self.rest.chars().next()?;
                self.rest = &self.rest[c.len_utf8()..];
                if c == quote {
                    return Some(result);
                }
                if c != '\\' {
                    result.push(c);
                    continue;
                }
                let escape = self.rest.chars().next()?;
                self.rest = &self.rest[escape.len_utf8()..];
                match escape {
                    '\\' | '\'' | '"' => result.push(escape),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    'u' | 'x' => {
                        let width = if escape == 'u' { 4 } else { 2 };
                        let hex = self.rest.get(..width)?;
                        if !hex.is_ascii() {
                            return None;
                        }
                        let n = u32::from_str_radix(hex, 16).ok()?;
                        result.push(char::from_u32(n)?);
                        self.rest = &self.rest[width..];
                    }
                    _ => return None,
                }
            }
        }
    }
    let invalid = || DetectorReadError::Metadata("invalid names dictionary".into());
    let mut p = Parser { rest: source };
    let mut result = Vec::new();
    if !p.token('{') {
        return Err(invalid());
    }
    loop {
        if p.token('}') {
            break;
        }
        p.ws();
        let end = p
            .rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(p.rest.len());
        let index = p.rest[..end].parse::<usize>().map_err(|_| invalid())?;
        p.rest = &p.rest[end..];
        if index != result.len() || !p.token(':') {
            return Err(invalid());
        }
        result.push(p.string().ok_or_else(invalid)?);
        if p.token('}') {
            break;
        }
        if !p.token(',') {
            return Err(invalid());
        }
    }
    p.ws();
    if !p.rest.is_empty() || result.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct FakeModel {
        description: ModelDescription,
        probed: Vec<Option<Vec<i64>>>,
        probes: Arc<AtomicUsize>,
    }

    impl CatalogModel for FakeModel {
        fn description(&self) -> &ModelDescription {
            &self.description
        }
        fn probe(&mut self) -> Result<Vec<Option<Vec<i64>>>, DetectorReadError> {
            self.probes.fetch_add(1, Ordering::Relaxed);
            Ok(self.probed.clone())
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("model.onnx");
        std::fs::write(&path, b"fake model: injected reader").unwrap();
        (temp, path)
    }

    fn description() -> ModelDescription {
        ModelDescription {
            inputs: vec![Some(vec![1, 3, -1, -1])],
            outputs: vec![Some(vec![1, 6, -1])],
            metadata: HashMap::new(),
        }
    }

    fn load(
        desc: ModelDescription,
        probes: Arc<AtomicUsize>,
    ) -> impl FnMut(&Path) -> Result<Box<dyn CatalogModel>, DetectorReadError> {
        move |_| {
            Ok(Box::new(FakeModel {
                description: desc.clone(),
                probed: vec![Some(vec![1, 6, 8400])],
                probes: probes.clone(),
            }))
        }
    }

    fn inspect(path: &Path, desc: ModelDescription) -> Result<DetectorInfo, DetectorReadError> {
        read_detector_with(
            path,
            detector_name(path),
            Placement::Package,
            &mut load(desc, Arc::new(AtomicUsize::new(0))),
        )
    }

    #[test]
    fn mask_detect_catalog_placements_are_one_level_and_errors_remain_visible() {
        let (temp, _) = fixture();
        std::fs::create_dir_all(temp.path().join("package/deeper")).unwrap();
        std::fs::write(temp.path().join("package/model.onnx"), b"model").unwrap();
        std::fs::write(temp.path().join("package/other.onnx"), b"ignored").unwrap();
        std::fs::write(temp.path().join("package/deeper/model.onnx"), b"ignored").unwrap();
        std::fs::write(temp.path().join("broken.onnx"), b"model").unwrap();
        std::fs::write(temp.path().join("broken.json"), b"{").unwrap();
        let entries = scan_detectors_with(
            temp.path(),
            &mut load(description(), Arc::new(AtomicUsize::new(0))),
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "package" && entry.result.is_ok())
        );
        assert!(entries.iter().any(|entry| entry.name == "broken"
            && matches!(entry.result, Err(DetectorReadError::Json(_)))));
        assert!(
            scan_detectors_with(
                &temp.path().join("absent"),
                &mut load(description(), Arc::new(AtomicUsize::new(0)))
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn mask_detect_catalog_root_model_uses_file_name_and_its_override() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("model.onnx"), b"model").unwrap();
        std::fs::write(temp.path().join("model.json"), r#"{"names":["a","b"]}"#).unwrap();
        let entries = scan_detectors_with(
            temp.path(),
            &mut load(description(), Arc::new(AtomicUsize::new(0))),
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "model");
        let info = entries[0].result.as_ref().unwrap();
        assert_eq!(info.name, "model");
        assert_eq!(info.names, ["a", "b"]);
    }

    #[test]
    fn mask_detect_catalog_file_sidecars_do_not_read_sibling_package_metadata() {
        let (temp, _) = fixture();
        let foo = temp.path().join("foo.onnx");
        std::fs::write(&foo, b"model").unwrap();
        for name in [
            "model.json",
            "labels.json",
            "threshold.json",
            "model_artifacts.json",
        ] {
            std::fs::write(temp.path().join(name), "{").unwrap();
        }
        let entries = scan_detectors_with(
            temp.path(),
            &mut load(description(), Arc::new(AtomicUsize::new(0))),
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.name == "model"
            && matches!(entry.result, Err(DetectorReadError::Json(_)))));
        let foo = entries
            .iter()
            .find(|entry| entry.name == "foo")
            .unwrap()
            .result
            .as_ref()
            .unwrap();
        assert_eq!(foo.names, ["分類 0", "分類 1"]);
        assert_eq!(foo.thresholds, [0.25, 0.25]);
        assert_eq!(foo.input_size, 640);
    }

    #[test]
    fn mask_detect_catalog_names_thresholds_shape_and_license_priority() {
        let (temp, path) = fixture();
        let mut desc = description();
        desc.metadata
            .insert("names".into(), "{0: 'onnx a', 1: \"onnx b\"}".into());
        desc.metadata.insert(
            "license".into(),
            "AGPL-3.0 https://example.invalid/raw".into(),
        );
        std::fs::write(temp.path().join("labels.json"), r#"["label a", "label b"]"#).unwrap();
        assert_eq!(
            inspect(&path, desc.clone()).unwrap().names,
            ["onnx a", "onnx b"]
        );
        std::fs::write(temp.path().join("threshold.json"), r#"{"threshold":0.278}"#).unwrap();
        std::fs::write(path.with_extension("json"), r#"{"names":["override a","override b"],"thresholds":{"override a":0.15},"shape":"ellipse"}"#).unwrap();
        let info = inspect(&path, desc.clone()).unwrap();
        assert_eq!(info.names, ["override a", "override b"]);
        assert_eq!(info.thresholds, [0.15, 0.278]);
        assert_eq!(info.shape, Some(DetectorShape::Ellipse));
        assert_eq!(
            info.license.as_deref(),
            desc.metadata.get("license").map(String::as_str)
        );
        std::fs::remove_file(path.with_extension("json")).unwrap();
        desc.metadata.remove("names");
        assert_eq!(
            inspect(&path, desc.clone()).unwrap().names,
            ["label a", "label b"]
        );
        std::fs::remove_file(temp.path().join("labels.json")).unwrap();
        assert_eq!(inspect(&path, desc).unwrap().names, ["分類 0", "分類 1"]);
    }

    #[test]
    fn mask_detect_catalog_malformed_relevant_json_is_not_hidden_by_priority() {
        let (temp, source) = fixture();
        let folder = temp.path().join("package");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("model.onnx");
        std::fs::rename(source, &path).unwrap();
        std::fs::write(path.with_extension("json"), r#"{"names":["a","b"]}"#).unwrap();
        for file in [
            "labels.json",
            "threshold.json",
            "model_artifacts.json",
            "model.json",
        ] {
            std::fs::write(folder.join(file), "{").unwrap();
            let entries = scan_detectors_with(
                temp.path(),
                &mut load(description(), Arc::new(AtomicUsize::new(0))),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert!(
                matches!(entries[0].result, Err(DetectorReadError::Json(_))),
                "{file}"
            );
            std::fs::remove_file(folder.join(file)).unwrap();
        }
    }

    #[test]
    fn mask_detect_catalog_python_dict_handles_quotes_escapes_punctuation_and_sequential_keys() {
        assert_eq!(
            parse_python_names(r#"{0: 'a,b:c', 1: "a\"b", 2: 'it\'s\\ok', 3: '\u65e5\x41',}"#)
                .unwrap(),
            ["a,b:c", "a\"b", "it's\\ok", "日A"]
        );
        for bad in [
            "{}",
            "{1:'a'}",
            "{0:'a',0:'b'}",
            "{0:__import__('os')}",
            "{0:'a'};x",
            "{0:'\\q'}",
            "[\"a\"]",
        ] {
            assert!(parse_python_names(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mask_detect_catalog_dynamic_channels_use_names_or_probe_once() {
        let (_temp, path) = fixture();
        let mut desc = description();
        desc.outputs[0] = Some(vec![-1, -1, -1]);
        let probes = Arc::new(AtomicUsize::new(0));
        let info = read_detector_with(
            &path,
            detector_name(&path),
            Placement::Package,
            &mut load(desc.clone(), probes.clone()),
        )
        .unwrap();
        assert_eq!(info.names.len(), 2);
        assert_eq!(probes.load(Ordering::Relaxed), 1);
        desc.metadata.insert("names".into(), "{0:'a',1:'b'}".into());
        read_detector_with(
            &path,
            detector_name(&path),
            Placement::Package,
            &mut load(desc, probes.clone()),
        )
        .unwrap();
        assert_eq!(probes.load(Ordering::Relaxed), 1);
        let mut loader = |_: &Path| {
            Ok(Box::new(FakeModel {
                description: ModelDescription {
                    outputs: vec![Some(vec![1, -1, -1])],
                    ..description()
                },
                probed: vec![Some(vec![1, -1, 8400])],
                probes: probes.clone(),
            }) as Box<dyn CatalogModel>)
        };
        assert!(matches!(
            read_detector_with(&path, detector_name(&path), Placement::Package, &mut loader),
            Err(DetectorReadError::OutputShape)
        ));
    }

    #[test]
    fn mask_detect_catalog_validates_format_class_count_and_all_tensor_types() {
        let (_temp, path) = fixture();
        let mut desc = description();
        desc.metadata.insert("task".into(), "segment".into());
        desc.outputs = vec![Some(vec![1, 38, -1]), Some(vec![1, 32, -1, -1])];
        assert_eq!(
            inspect(&path, desc.clone()).unwrap().format,
            DetectorFormat::UltralyticsSegment
        );
        desc.metadata.insert("names".into(), "{0:'a'}".into());
        assert!(matches!(
            inspect(&path, desc.clone()),
            Err(DetectorReadError::ClassCount)
        ));
        desc.metadata.remove("names");
        desc.outputs[1] = None;
        assert!(matches!(
            inspect(&path, desc.clone()),
            Err(DetectorReadError::OutputShape)
        ));
        desc.outputs[1] = Some(vec![1, 31, 160, 160]);
        assert!(matches!(
            inspect(&path, desc),
            Err(DetectorReadError::OutputShape)
        ));
        for input in [
            None,
            Some(vec![2, 3, 640, 640]),
            Some(vec![1, 4, 640, 640]),
            Some(vec![1, 3, 640, 320]),
        ] {
            let mut desc = description();
            desc.inputs = vec![input];
            assert!(matches!(
                inspect(&path, desc),
                Err(DetectorReadError::InputShape)
            ));
        }
        for output in [
            None,
            Some(vec![1, 8400, 6, 1]),
            Some(vec![1, 4, 8400]),
            Some(vec![1, 6, 0]),
        ] {
            let mut desc = description();
            desc.outputs = vec![output];
            assert!(matches!(
                inspect(&path, desc),
                Err(DetectorReadError::OutputShape)
            ));
        }
        for key in ["end2end", "nms"] {
            let mut desc = description();
            desc.metadata.insert(key.into(), "True".into());
            assert!(matches!(
                inspect(&path, desc),
                Err(DetectorReadError::EndToEnd)
            ));
        }
    }

    #[test]
    fn mask_detect_catalog_scalar_threshold_overrides_package_default_but_not_class_values() {
        let (temp, path) = fixture();
        std::fs::write(temp.path().join("threshold.json"), r#"{"threshold":0.278}"#).unwrap();
        std::fs::write(
            path.with_extension("json"),
            r#"{"threshold":0.4,"thresholds":{"分類 0":0.15}}"#,
        )
        .unwrap();
        assert_eq!(
            inspect(&path, description()).unwrap().thresholds,
            [0.15, 0.4]
        );
        let file = read_detector_with(
            &path,
            "model".into(),
            Placement::File,
            &mut load(description(), Arc::new(AtomicUsize::new(0))),
        )
        .unwrap();
        assert_eq!(file.thresholds, [0.15, 0.4]);
        for value in ["null", "\"0.4\"", "-0.1", "1.1"] {
            std::fs::write(
                path.with_extension("json"),
                format!("{{\"threshold\":{value}}}"),
            )
            .unwrap();
            assert!(
                matches!(
                    inspect(&path, description()),
                    Err(DetectorReadError::Json(_))
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn mask_detect_catalog_mixed_dynamic_spatial_axes_keep_fixed_input_contract() {
        let (temp, path) = fixture();
        for spatial in [[320, -1], [-1, 320]] {
            let mut desc = description();
            desc.inputs = vec![Some(vec![1, 3, spatial[0], spatial[1]])];
            assert_eq!(inspect(&path, desc.clone()).unwrap().input_size, 320);
            desc.metadata.insert("imgsz".into(), "640".into());
            assert!(matches!(
                inspect(&path, desc),
                Err(DetectorReadError::InputShape)
            ));
        }
        for contents in ["[]", r#"{"args":[]}"#, r#"{"args":{"imgsz":"640"}}"#] {
            std::fs::write(temp.path().join("model_artifacts.json"), contents).unwrap();
            let mut desc = description();
            desc.metadata.insert("imgsz".into(), "640".into());
            assert!(
                matches!(inspect(&path, desc), Err(DetectorReadError::Json(_))),
                "{contents}"
            );
        }
    }

    #[test]
    fn mask_detect_catalog_input_size_priority_and_fixed_shape_consistency() {
        let (temp, path) = fixture();
        let mut desc = description();
        assert_eq!(inspect(&path, desc.clone()).unwrap().input_size, 640);
        desc.inputs = vec![Some(vec![1, 3, 320, 320])];
        assert_eq!(inspect(&path, desc.clone()).unwrap().input_size, 320);
        desc.inputs = description().inputs;
        std::fs::write(
            temp.path().join("model_artifacts.json"),
            r#"{"args":{"imgsz":1280}}"#,
        )
        .unwrap();
        assert_eq!(inspect(&path, desc.clone()).unwrap().input_size, 1280);
        desc.metadata.insert("imgsz".into(), "[640,640]".into());
        assert_eq!(inspect(&path, desc.clone()).unwrap().input_size, 640);
        desc.metadata.insert("imgsz".into(), "[640,320]".into());
        assert!(matches!(
            inspect(&path, desc.clone()),
            Err(DetectorReadError::Metadata(_))
        ));
        desc.metadata.insert("imgsz".into(), "640".into());
        desc.inputs = vec![Some(vec![1, 3, 320, 320])];
        assert!(matches!(
            inspect(&path, desc),
            Err(DetectorReadError::InputShape)
        ));
    }
}
