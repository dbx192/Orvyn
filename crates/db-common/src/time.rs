use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

pub fn elapsed_ms(started_at: Instant) -> u128 {
    started_at.elapsed().as_millis()
}

pub fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}
