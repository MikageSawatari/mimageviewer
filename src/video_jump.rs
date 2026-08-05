fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

pub(crate) fn format_time(secs: f64) -> String {
    let total = finite_nonnegative(secs).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub(crate) fn format_time_millis(secs: f64) -> String {
    let total_ms = (finite_nonnegative(secs) * 1000.0).round() as u64;
    let total_secs = total_ms / 1000;
    let ms = total_ms % 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

pub(crate) fn format_jump_entry_time(
    secs: f64,
    all_positions: impl IntoIterator<Item = f64>,
) -> String {
    let normalized = finite_nonnegative(secs);
    let second_key = normalized.round() as u64;
    let same_second_count = all_positions
        .into_iter()
        .filter(|other| finite_nonnegative(*other).round() as u64 == second_key)
        .take(2)
        .count();
    let has_millis = ((normalized * 1000.0).round() as u64) % 1000 != 0;
    if has_millis || same_second_count > 1 {
        format_time_millis(normalized)
    } else {
        format_time(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_time_uses_millis_for_fractional_or_duplicate_seconds() {
        let positions = [80.0, 80.04, 597.88, 600.0];
        assert_eq!(format_jump_entry_time(positions[0], positions), "1:20.000");
        assert_eq!(format_jump_entry_time(positions[1], positions), "1:20.040");
        assert_eq!(format_jump_entry_time(positions[2], positions), "9:57.880");
        assert_eq!(format_jump_entry_time(positions[3], positions), "10:00");
    }
}
