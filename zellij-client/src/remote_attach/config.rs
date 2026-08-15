use std::time::Duration;

// Connection settings
pub const CONNECTION_TIMEOUT_SECS: u64 = 30;

pub fn connection_timeout() -> Duration {
    Duration::from_secs(CONNECTION_TIMEOUT_SECS)
}
