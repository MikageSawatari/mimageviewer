//! Allowlisted failure information travels separately from local detailed text.
use super::AiError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Inference,
    ModelLoad,
    ImageProcessing,
    Io,
    Unknown,
}
impl FailureKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Inference => "推論エラー",
            Self::ModelLoad => "モデル読込",
            Self::ImageProcessing => "画像処理",
            Self::Io => "入出力",
            Self::Unknown => "詳細不明",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FailureDiagnostic {
    pub(crate) kind: FailureKind,
    pub(crate) ort_code: Option<i32>,
}

pub(crate) struct FinalAiFailure {
    detail: String,
    diagnostic: FailureDiagnostic,
}
impl FinalAiFailure {
    pub(crate) fn diagnostic(&self) -> FailureDiagnostic {
        self.diagnostic
    }
    pub(crate) fn model_load(error: AiError) -> Self {
        let mut failure = Self::from(error);
        failure.diagnostic.kind = FailureKind::ModelLoad;
        // load_requested_model already logged the original error. Preserve the
        // former public pipeline summary for Remote/materialization consumers.
        failure.detail = "no usable AI model (load failed)".into();
        failure
    }
    pub(crate) fn model_unavailable(detail: &'static str) -> Self {
        Self {
            detail: detail.into(),
            diagnostic: FailureDiagnostic {
                kind: FailureKind::ModelLoad,
                ort_code: None,
            },
        }
    }
}
impl From<AiError> for FinalAiFailure {
    fn from(error: AiError) -> Self {
        let (kind, ort_code) = match &error {
            AiError::OrtStatus { code, .. } => (FailureKind::Inference, Some(*code)),
            AiError::Ort(_) => (FailureKind::Inference, None),
            AiError::ModelNotFound(_) => (FailureKind::ModelLoad, None),
            AiError::ImageProcessing(_) => (FailureKind::ImageProcessing, None),
            AiError::Io(_) => (FailureKind::Io, None),
            AiError::Cancelled => (FailureKind::Unknown, None),
        };
        Self {
            detail: error.to_string(),
            diagnostic: FailureDiagnostic { kind, ort_code },
        }
    }
}
impl From<String> for FinalAiFailure {
    fn from(detail: String) -> Self {
        Self {
            detail,
            diagnostic: FailureDiagnostic {
                kind: FailureKind::Unknown,
                ort_code: None,
            },
        }
    }
}
impl std::fmt::Display for FinalAiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ai_failure_diagnostic_classifies_types_without_parsing_raw_messages() {
        let raw = r"OOM C:\private\secret.zip file.png search=query driver DEVICE_REMOVED";
        for (error, kind, code) in [
            (AiError::Ort(raw.into()), FailureKind::Inference, None),
            (
                AiError::OrtStatus {
                    code: 15,
                    detail: raw.into(),
                },
                FailureKind::Inference,
                Some(15),
            ),
            (
                AiError::ImageProcessing(raw.into()),
                FailureKind::ImageProcessing,
                None,
            ),
            (
                AiError::Io(std::io::Error::other(raw)),
                FailureKind::Io,
                None,
            ),
        ] {
            let failure = FinalAiFailure::from(error);
            assert_eq!(
                failure.diagnostic(),
                FailureDiagnostic {
                    kind,
                    ort_code: code
                }
            );
            assert!(
                failure.to_string().contains(raw),
                "local detailed text is retained"
            );
        }
        let unknown = FinalAiFailure::from(raw.to_owned());
        assert_eq!(unknown.diagnostic().kind, FailureKind::Unknown);
        let load = FinalAiFailure::model_load(AiError::OrtStatus {
            code: 3,
            detail: raw.into(),
        });
        assert_eq!(
            load.diagnostic(),
            FailureDiagnostic {
                kind: FailureKind::ModelLoad,
                ort_code: Some(3)
            }
        );
        assert_eq!(load.to_string(), "no usable AI model (load failed)");
    }
}
