//! How long a job waits after a failure that may pass: 30 seconds, 2 minutes, 10 minutes, then
//! every 30 minutes. A server's own `retry-after` wins.

use std::time::Duration;

/// The wait after the `attempts`-th try (1 for the first), or `after` when the server said.
pub fn delay(attempts: u32, after: Option<Duration>) -> Duration {
    if let Some(after) = after {
        return after;
    }
    Duration::from_secs(match attempts {
        0 | 1 => 30,
        2 => 2 * 60,
        3 => 10 * 60,
        _ => 30 * 60,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_grow_then_level_off_and_the_server_wins() {
        let seconds: Vec<u64> = (1..=6).map(|n| delay(n, None).as_secs()).collect();
        assert_eq!(seconds, [30, 120, 600, 1800, 1800, 1800]);
        assert_eq!(
            delay(5, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
    }
}
