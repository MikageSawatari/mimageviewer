//! Stable line-delimited JSON contract for the S2 host. Exit code 7 is unused.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Progress {
        phase: Phase,
        done: usize,
        total: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pages: Option<usize>,
    },
    Result {
        status: Status,
        exit_code: i32,
        page_count: usize,
        direction: String,
        layout: String,
        profile: String,
        blocked_requests: usize,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Parse,
    Extract,
    Init,
    Print,
    Merge,
    Verify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Drm,
    Invalid,
    Webview2Missing,
    Webview2Unsupported,
    RenderFailed,
    Timeout,
}

impl Event {
    pub fn failure(exit_code: i32, message: String) -> Self {
        let status = match exit_code {
            2 => Status::Drm,
            3 => Status::Invalid,
            4 => Status::Webview2Missing,
            8 => Status::Webview2Unsupported,
            6 => Status::Timeout,
            _ => Status::RenderFailed,
        };
        Self::Result {
            status,
            exit_code,
            page_count: 0,
            direction: "default".into(),
            layout: "unknown".into(),
            profile: crate::render::REFLOW_PROFILE.into(),
            blocked_requests: 0,
            message,
        }
    }

    pub fn from_report(report: &crate::Report) -> Self {
        let status = match report.exit_code {
            0 => Status::Success,
            2 => Status::Drm,
            3 => Status::Invalid,
            4 => Status::Webview2Missing,
            8 => Status::Webview2Unsupported,
            6 => Status::Timeout,
            _ => Status::RenderFailed,
        };
        Self::Result {
            status,
            exit_code: report.exit_code,
            page_count: report.output_page_count.unwrap_or(0),
            direction: report.direction.clone().unwrap_or_else(|| "default".into()),
            layout: report.layout.clone().unwrap_or_else(|| "unknown".into()),
            profile: report.profile.clone(),
            blocked_requests: report.blocked_request_count,
            message: report.errors.first().cloned().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for event in [
            Event::Progress {
                phase: Phase::Print,
                done: 2,
                total: 10,
                pages: Some(7),
            },
            Event::Result {
                status: Status::Timeout,
                exit_code: 6,
                page_count: 0,
                direction: "rtl".into(),
                layout: "mixed".into(),
                profile: "reflow-v1".into(),
                blocked_requests: 3,
                message: "deadline".into(),
            },
            Event::failure(8, "interface".into()),
        ] {
            let encoded = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&encoded).unwrap(), event);
        }
        assert!(matches!(
            Event::failure(8, "interface".into()),
            Event::Result {
                status: Status::Webview2Unsupported,
                ..
            }
        ));
        assert_eq!(
            serde_json::from_str::<Event>(
                r#"{"event":"progress","phase":"print","done":1,"total":0}"#
            )
            .unwrap(),
            Event::Progress {
                phase: Phase::Print,
                done: 1,
                total: 0,
                pages: None,
            }
        );
    }
}
