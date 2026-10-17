use chrono::{DateTime, Utc};
use prost_types::Timestamp;

pub fn timestamp(at: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: at.timestamp(),
        nanos: at.timestamp_subsec_nanos() as i32,
    }
}

pub fn opt_timestamp(at: Option<DateTime<Utc>>) -> Option<Timestamp> {
    at.map(timestamp)
}

/// A timestamp from a peer; now if missing or out of range.
pub fn from_timestamp(at: Option<Timestamp>) -> DateTime<Utc> {
    at.and_then(|at| DateTime::from_timestamp(at.seconds, at.nanos.max(0) as u32))
        .unwrap_or_else(Utc::now)
}
