//! Preparation barrier over diagnostic projections, never a request owner.
use serde_json::Value;

pub(super) fn close_ready(observation: &Value, mode: &str) -> bool {
    observation["keep_count"].as_u64().is_some_and(|n| n > 8)
        && observation["items_len"].as_u64().is_some_and(|n| n > 0)
        && observation["index_settled"] == true
        && match mode {
            "after-loading" => observation["loading_complete"] == true,
            "while-loading" => {
                observation["loading_complete"] == false
                    && observation["worker_pending_count"]
                        .as_u64()
                        .is_some_and(|n| n > 0)
            }
            _ => false,
        }
}

fn time(event: &Value, by: f64) -> Option<f64> {
    event["t"]
        .as_f64()
        .filter(|t| t.is_finite() && *t >= 0.0 && *t <= by)
}

fn same_request(a: &Value, b: &Value) -> bool {
    a["request_id"].as_u64().is_some_and(|id| id > 0)
        && ["request_id", "key", "idx", "items_gen"]
            .iter()
            .all(|field| a[field] == b[field])
        && a["key"].as_str().is_some_and(|key| !key.is_empty())
        && a["idx"].as_u64().is_some()
        && a["items_gen"].as_u64().is_some()
}

pub(super) fn established(events: &[Value], mode: &str, by: f64) -> Option<Value> {
    if !by.is_finite() || !matches!(mode, "while-loading" | "after-loading") {
        return None;
    }
    let closes: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "close_accept")
        .collect();
    let hides: Vec<_> = events.iter().filter(|e| e["kind"] == "tray_hide").collect();
    if closes.len() != 1 || hides.len() != 1 {
        return None;
    }
    let close = closes[0];
    let hide = hides[0];
    let close_t = time(close, by)?;
    let hide_t = time(hide, by)?;
    if hide_t < close_t
        || hide["window_visible"] != false
        || close["close_sequence"].as_u64().is_none_or(|seq| seq == 0)
        || close["close_sequence"] != hide["close_sequence"]
        || close["items_gen"] != hide["items_gen"]
        || close["folder_path"]
            .as_str()
            .is_none_or(|path| path.is_empty())
        || close["folder_path"] != hide["folder_path"]
        || close["loading_complete"] != (mode == "after-loading")
        || hide["loading_complete"] != close["loading_complete"]
        || close["cache_check_unix_ms"].as_u64().is_none()
        || close["cache_check_unix_ms"] != hide["cache_check_unix_ms"]
    {
        return None;
    }
    for final_event in events.iter().filter(|e| e["kind"] == "finalized") {
        let Some(final_t) = time(final_event, by) else {
            continue;
        };
        if final_event["close_sequence"] != close["close_sequence"]
            || final_event["items_gen"] != close["items_gen"]
            || final_event["window_visible"] != false
            || final_event["image_backlog_contains_request"] != true
        {
            continue;
        }
        let target_matches = |event: &&Value| {
            event["targets"].as_array().is_some_and(|targets| {
                targets.iter().any(|target| {
                    ["key", "idx", "items_gen"]
                        .iter()
                        .all(|field| target[field] == final_event[field])
                        && target["catalog_absent"] == true
                        && target["cache_map_absent"] == true
                })
            })
        };
        if !target_matches(&close) || !target_matches(&hide) {
            continue;
        }
        for source in events
            .iter()
            .filter(|e| e["kind"] == "source" && same_request(e, final_event))
        {
            let Some(source_t) = time(source, by) else {
                continue;
            };
            if source["origin"] != "source_generated"
                || source["from_cache"] != false
                || source["should_save"] != false
                || source["force_cache"] != false
                || source_t > final_t
            {
                continue;
            }
            for begin in events
                .iter()
                .filter(|e| e["kind"] == "request_begin" && same_request(e, final_event))
            {
                let Some(begin_t) = time(begin, by) else {
                    continue;
                };
                if begin_t > hide_t
                    && begin_t <= source_t
                    && begin["evaluated_display_px"]
                        .as_u64()
                        .is_some_and(|px| px > 0)
                {
                    return Some(final_event.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn witnesses(mode: &str) -> Vec<Value> {
        let mut close = json!({"kind":"close_accept","t":1.0,"close_sequence":1,"items_gen":3,
            "folder_path":"fixture/tray","cache_check_unix_ms":1000,
            "loading_complete":mode == "after-loading",
            "targets":[{"key":"dir::fixture/tile","idx":2,"items_gen":3,"catalog_absent":true,"cache_map_absent":true}]});
        let mut hide = close.clone();
        hide["kind"] = json!("tray_hide");
        hide["t"] = json!(1.1);
        hide["window_visible"] = json!(false);
        let mut begin = json!({"kind":"request_begin","t":1.2,"request_id":7,"idx":2,"items_gen":3,
            "key":"dir::fixture/tile","evaluated_display_px":256});
        let mut source = begin.clone();
        source["kind"] = json!("source");
        source["t"] = json!(1.3);
        source["origin"] = json!("source_generated");
        source["from_cache"] = json!(false);
        source["should_save"] = json!(false);
        source["force_cache"] = json!(false);
        let mut final_event = begin.clone();
        final_event["kind"] = json!("finalized");
        final_event["t"] = json!(1.4);
        final_event["close_sequence"] = json!(1);
        final_event["window_visible"] = json!(false);
        final_event["image_backlog_contains_request"] = json!(true);
        // Keep serialization-owned values (no mutable aliases to the request owner).
        close["kind"] = json!("close_accept");
        begin["kind"] = json!("request_begin");
        vec![close, hide, begin, source, final_event]
    }

    #[test]
    fn same_id_is_ready_before_idle_sleep_in_both_modes() {
        for mode in ["while-loading", "after-loading"] {
            assert!(established(&witnesses(mode), mode, 1.4).is_some());
            assert!(established(&witnesses(mode), mode, 1.399).is_none());
        }
    }

    #[test]
    fn other_id_missing_queue_and_late_observe_cannot_start_measurement() {
        for mode in ["while-loading", "after-loading"] {
            for index in [2, 3, 4] {
                let mut events = witnesses(mode);
                events[index]["request_id"] = json!(8);
                assert!(established(&events, mode, 2.0).is_none());
            }
            let mut events = witnesses(mode);
            events[4]["image_backlog_contains_request"] = json!(false);
            assert!(established(&events, mode, 2.0).is_none());
            let mut events = witnesses(mode);
            events[3]["t"] = json!(31.3);
            events[4]["t"] = json!(31.4);
            assert!(established(&events, mode, 10.0).is_none());
            assert!(established(&events, mode, 25.0).is_none());
            assert!(established(&events, mode, 32.0).is_some());
            let mut events = witnesses(mode);
            events[2]["t"] = json!(1.0);
            assert!(established(&events, mode, 2.0).is_none());
            let mut events = witnesses(mode);
            events.remove(3);
            assert!(established(&events, mode, 2.0).is_none());
        }
    }

    #[test]
    fn close_waits_for_the_requested_loading_mode_and_real_worker_activity() {
        let mut observation = json!({"keep_count":20,"items_len":600,"index_settled":true,
            "loading_complete":false,"worker_pending_count":1});
        assert!(close_ready(&observation, "while-loading"));
        assert!(!close_ready(&observation, "after-loading"));
        observation["worker_pending_count"] = json!(0);
        assert!(!close_ready(&observation, "while-loading"));
        observation["loading_complete"] = json!(true);
        assert!(close_ready(&observation, "after-loading"));
    }
}
