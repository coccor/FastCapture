//! `capture_id` (spec §6.1): a ULID, 48 bits of milliseconds since 1970 then 80 random bits, in
//! Crockford base32. IDs sort by time, and are monotonic within this process: two in the same
//! millisecond differ by one in their random part.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RANDOM_MASK: u128 = (1 << 80) - 1;

static LAST: Mutex<Option<(u64, u128)>> = Mutex::new(None);

/// A new ULID.
pub fn new() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (millis, random) = next(*last, millis, || {
        let mut bytes = [0u8; 10];
        crate::platform::random::fill(&mut bytes);
        bytes
            .iter()
            .fold(0u128, |value, byte| value << 8 | u128::from(*byte))
    });
    *last = Some((millis, random));
    encode(millis, random)
}

/// The time and random part after `last` at `now`: fresh random bits in a new millisecond,
/// otherwise the last ones plus one (moving to the next millisecond if they overflow, or if the
/// clock went back).
fn next(last: Option<(u64, u128)>, now: u64, random: impl FnOnce() -> u128) -> (u64, u128) {
    match last {
        Some((last_millis, last_random)) if now <= last_millis => {
            if last_random == RANDOM_MASK {
                (last_millis + 1, 0)
            } else {
                (last_millis, last_random + 1)
            }
        }
        _ => (now, random() & RANDOM_MASK),
    }
}

fn encode(millis: u64, random: u128) -> String {
    let value = (u128::from(millis) & ((1 << 48) - 1)) << 80 | (random & RANDOM_MASK);
    (0..26)
        .rev()
        .map(|index| ALPHABET[((value >> (index * 5)) & 31) as usize] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulids_are_26_crockford_characters_that_sort_by_time() {
        assert_eq!(encode(0, 0), "00000000000000000000000000");
        assert_eq!(encode(1, 0), "00000000010000000000000000");
        assert_eq!(
            encode((1 << 48) - 1, RANDOM_MASK),
            "7ZZZZZZZZZZZZZZZZZZZZZZZZZ"
        );
        assert!(encode(1_792_281_600_000, 5) < encode(1_792_281_600_001, 0));
        let id = new();
        assert_eq!(id.len(), 26);
        assert!(id.bytes().all(|byte| ALPHABET.contains(&byte)));
    }

    #[test]
    fn ids_in_one_millisecond_count_up() {
        assert_eq!(next(None, 10, || 7), (10, 7));
        assert_eq!(next(Some((10, 7)), 10, || 99), (10, 8));
        assert_eq!(next(Some((10, 7)), 9, || 99), (10, 8), "clock went back");
        assert_eq!(next(Some((10, RANDOM_MASK)), 10, || 99), (11, 0));
        assert_eq!(next(Some((10, 7)), 11, || 99), (11, 99));
        let (a, b) = (new(), new());
        assert!(a < b, "{a} then {b}");
    }
}
