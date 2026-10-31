//! Test-script-only bridge from native wait evidence to the perf time base.

use super::*;
use eframe::miv_test_script_wait_witness::{self as witness, WaitCause, WaitRecord};

pub(super) fn install() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        witness::install(Box::new(|record| {
            let extras = fields(record, program_start());
            event("eventloop", "wait", None, 0, &extras);
        }));
    });
}

fn fields(record: WaitRecord, origin: Option<Instant>) -> [(&'static str, Value); 5] {
    let seconds = |instant: Instant| {
        origin
            .and_then(|start| instant.checked_duration_since(start))
            .and_then(|duration| serde_json::Number::from_f64(duration.as_secs_f64()))
            .map_or(Value::Null, Value::Number)
    };
    [
        (
            "cause",
            match record.cause {
                WaitCause::Cancelled => "cancelled",
                WaitCause::ResumeTimeReached => "resume_time_reached",
            }
            .into(),
        ),
        ("wait_start_t", seconds(record.start)),
        ("wait_end_t", seconds(record.end)),
        (
            "requested_resume_t",
            record.requested_resume.map_or(Value::Null, seconds),
        ),
        ("callback_during_wait", record.callback_during_wait.into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_precision_times_and_absent_deadline() {
        let origin = Instant::now();
        let record = WaitRecord {
            cause: WaitCause::Cancelled,
            start: origin + Duration::from_nanos(123_456_789),
            end: origin + Duration::from_nanos(234_567_891),
            requested_resume: None,
            callback_during_wait: false,
        };
        let values: serde_json::Map<_, _> = fields(record, Some(origin))
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(values["wait_start_t"], 0.123456789);
        assert_eq!(values["wait_end_t"], 0.234567891);
        assert_eq!(values["requested_resume_t"], Value::Null);
        assert_eq!(values["cause"], "cancelled");
        assert_eq!(values["callback_during_wait"], false);
    }

    #[test]
    fn before_origin_and_missing_origin_are_null() {
        let before = Instant::now();
        let origin = before + Duration::from_secs(1);
        let record = WaitRecord {
            cause: WaitCause::ResumeTimeReached,
            start: before,
            end: origin,
            requested_resume: Some(origin + Duration::from_secs(2)),
            callback_during_wait: true,
        };
        let values = fields(record, Some(origin));
        assert_eq!(values[0].1, "resume_time_reached");
        assert_eq!(values[1].1, Value::Null);
        assert_eq!(values[2].1, 0.0);
        assert_eq!(values[3].1, 2.0);
        assert_eq!(values[4].1, true);
        for (_, value) in &fields(record, None)[1..4] {
            assert_eq!(*value, Value::Null);
        }
    }
}
