use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn format_relative(diff_secs: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3600;
    const DAY: i64 = 86400;
    const WEEK: i64 = 604800;
    const MONTH: i64 = 2592000;
    const YEAR: i64 = 31536000;

    match diff_secs {
        d if d < MINUTE => "just now".to_string(),
        d if d < HOUR => format_unit(d / MINUTE, "min"),
        d if d < DAY => format_unit(d / HOUR, "hour"),
        d if d < WEEK => format_unit(d / DAY, "day"),
        d if d < MONTH => format_unit(d / WEEK, "week"),
        d if d < YEAR => format_unit(d / MONTH, "month"),
        d => format_unit(d / YEAR, "year"),
    }
}

fn format_unit(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {} ago", unit)
    } else {
        format!("{} {}s ago", count, unit)
    }
}
