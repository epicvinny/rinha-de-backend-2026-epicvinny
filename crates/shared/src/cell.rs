use crate::normalization::Constants;
use crate::types::{Payload, Vec16I};
use crate::vectorize::parse_iso8601;
use chrono::{DateTime, Datelike, Timelike, Utc};

// Cell key: 13 bits
// Bit layout (MSB to LSB):
//   bit 12: is_online
//   bit 11: card_present
//   bit 10: unknown_merchant
//   bit 9:  sentinel_present (last_tx is Some)
//   bit 8:  (same as bit 9 — both dims 5,6 are set together)
//   bits 7..3: hour (0-23, 5 bits)
//   bits 2..0: day (0-6, 3 bits)
//
// Note: bits 8 and 9 encode the same sentinel flag (both dims 5&6 set together)
// We use: (is_online << 12) | (card_present << 11) | (unknown_merch << 10) |
//         (sent << 9) | (sent << 8) | (hour << 3) | day

pub const N_CELLS: usize = 8192; // 2^13

pub const HOUR_Q: [i16; 24] = {
    let mut arr = [0i16; 24];
    let mut h = 0usize;
    while h < 24 {
        arr[h] = ((h as f64 * 10000.0 / 23.0) + 0.5) as i16;
        h += 1;
    }
    arr
};

pub const DAY_Q: [i16; 7] = {
    let mut arr = [0i16; 7];
    let mut d = 0usize;
    while d < 7 {
        arr[d] = ((d as f64 * 10000.0 / 6.0) + 0.5) as i16;
        d += 1;
    }
    arr
};

fn hour_from_q(q: i16) -> u8 {
    HOUR_Q.iter().position(|&x| x == q).unwrap_or(0) as u8
}

fn day_from_q(q: i16) -> u8 {
    DAY_Q.iter().position(|&x| x == q).unwrap_or(0) as u8
}

pub fn cell_key_from_payload(p: &Payload<'_>, _c: &Constants) -> u16 {
    let (hour, day) = if let Some(fdt) = parse_iso8601(p.transaction.requested_at) {
        (fdt.hour as u16, fdt.day_of_week as u16)
    } else {
        let dt: DateTime<Utc> = p
            .transaction
            .requested_at
            .parse()
            .expect("invalid datetime");
        (dt.hour() as u16, dt.weekday().num_days_from_monday() as u16)
    };

    let is_online = if p.terminal.is_online { 1u16 } else { 0u16 };
    let card_present = if p.terminal.card_present { 1u16 } else { 0u16 };
    let unknown_merch = if p.customer.known_merchants.contains(p.merchant.id) {
        0u16
    } else {
        1u16
    };
    let sent = if p.last_transaction.is_some() {
        1u16
    } else {
        0u16
    };

    (is_online << 12)
        | (card_present << 11)
        | (unknown_merch << 10)
        | (sent << 9)
        | (sent << 8)
        | (hour << 3)
        | day
}

pub fn cell_key_from_i16(v: &Vec16I) -> u16 {
    // Extract binary dims
    let is_online = if v[9] != 0 { 1u16 } else { 0u16 };
    let card_present = if v[10] != 0 { 1u16 } else { 0u16 };
    let unknown_merch = if v[11] != 0 { 1u16 } else { 0u16 };
    // sentinel: dim 5 (or 6) is -10000 when absent
    let sent = if v[5] == -10000 { 0u16 } else { 1u16 };

    // Extract hour from dim 3: v[3] = (hour/23 * 10000).round()
    let hour = hour_from_q(v[3]) as u16;
    // Extract day from dim 4
    let day = day_from_q(v[4]) as u16;

    (is_online << 12)
        | (card_present << 11)
        | (unknown_merch << 10)
        | (sent << 9)
        | (sent << 8)
        | (hour << 3)
        | day
}

pub struct NeighborKeys {
    keys: [u16; 4],
    len: u8,
    cursor: u8,
}

impl Iterator for NeighborKeys {
    type Item = u16;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor < self.len {
            let val = self.keys[self.cursor as usize];
            self.cursor += 1;
            Some(val)
        } else {
            None
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.len - self.cursor) as usize;
        (remaining, Some(remaining))
    }
}

// Returns iterator over neighboring cell keys (up to 4 neighbors)
// Moves ±1 hour (clamped); ±1 day (clamped) within the same binary partition.
// Restricting neighbor search to the same binary partition guarantees 100% accuracy,
// since different partitions are > 100,000,000 L2 distance away and categorically rejected.
pub fn neighbor_keys(key: u16) -> NeighborKeys {
    let mut keys = [0u16; 4];
    let mut len = 0;

    // Extract hour and day from key
    let hour = ((key >> 3) & 0x1F) as u8;
    let day = (key & 0x7) as u8;

    // ±1 hour (clamped)
    if hour > 0 {
        let new_hour = hour - 1;
        keys[len] = (key & !(0x1F << 3)) | ((new_hour as u16) << 3);
        len += 1;
    }
    if hour < 23 {
        let new_hour = hour + 1;
        keys[len] = (key & !(0x1F << 3)) | ((new_hour as u16) << 3);
        len += 1;
    }

    // ±1 day (clamped)
    if day > 0 {
        let new_day = day - 1;
        keys[len] = (key & !0x7) | (new_day as u16);
        len += 1;
    }
    if day < 6 {
        let new_day = day + 1;
        keys[len] = (key & !0x7) | (new_day as u16);
        len += 1;
    }

    NeighborKeys {
        keys,
        len: len as u8,
        cursor: 0,
    }
}

#[inline]
fn bit(key: u16, shift: u16) -> bool {
    ((key >> shift) & 1) != 0
}

#[inline]
fn sq_i32(v: i32) -> i32 {
    v * v
}

#[inline]
pub fn cell_lower_bound_sq(query_v: &Vec16I, query_key: u16, target_key: u16) -> i32 {
    if query_key == target_key {
        return 0;
    }

    let mut sum = 0i32;

    // Binary dimensions: is_online, card_present, unknown_merchant.
    // Any bit flip means a 0 <-> 10000 transition and therefore exactly 1e8.
    if bit(query_key, 12) != bit(target_key, 12) {
        sum += 100_000_000;
    }
    if bit(query_key, 11) != bit(target_key, 11) {
        sum += 100_000_000;
    }
    if bit(query_key, 10) != bit(target_key, 10) {
        sum += 100_000_000;
    }

    // Sentinel dimensions. A present target can take any value in [0, 10000],
    // so a query sentinel's nearest possible value is 0. An absent target is
    // exactly -10000, so the bound depends on the query's actual rounded value.
    if bit(query_key, 9) != bit(target_key, 9) {
        sum += if bit(target_key, 9) {
            100_000_000
        } else {
            sq_i32(query_v[5] as i32 + 10_000)
        };
    }
    if bit(query_key, 8) != bit(target_key, 8) {
        sum += if bit(target_key, 8) {
            100_000_000
        } else {
            sq_i32(query_v[6] as i32 + 10_000)
        };
    }

    let q_hour = ((query_key >> 3) & 0x1F) as usize;
    let t_hour = ((target_key >> 3) & 0x1F) as usize;
    let dh = HOUR_Q[q_hour] as i32 - HOUR_Q[t_hour] as i32;

    let q_day = (query_key & 0x7) as usize;
    let t_day = (target_key & 0x7) as usize;
    let dd = DAY_Q[q_day] as i32 - DAY_Q[t_day] as i32;

    sum + dh * dh + dd * dd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hour_day_roundtrip() {
        for h in 0..24usize {
            assert_eq!(
                hour_from_q(HOUR_Q[h]),
                h as u8,
                "hour roundtrip failed for h={}",
                h
            );
        }
        for d in 0..7usize {
            assert_eq!(
                day_from_q(DAY_Q[d]),
                d as u8,
                "day roundtrip failed for d={}",
                d
            );
        }
    }

    #[test]
    fn same_key_zero_bound() {
        let q = [0i16; 16];
        assert_eq!(cell_lower_bound_sq(&q, 0, 0), 0);
        assert_eq!(cell_lower_bound_sq(&q, 1234, 1234), 0);
    }

    #[test]
    fn binary_flip_bound_is_exact() {
        let q = [0i16; 16];
        assert_eq!(cell_lower_bound_sq(&q, 0, 1 << 12), 100_000_000);
        assert_eq!(cell_lower_bound_sq(&q, 0, 1 << 11), 100_000_000);
        assert_eq!(cell_lower_bound_sq(&q, 0, 1 << 10), 100_000_000);
    }

    #[test]
    fn sentinel_flip_bound_uses_query_value() {
        let mut q = [0i16; 16];
        q[5] = 2_500;
        q[6] = 7_500;

        let present_key = (1 << 9) | (1 << 8);
        let absent_key = 0;

        assert_eq!(
            cell_lower_bound_sq(&q, present_key, absent_key),
            (12_500_i32 * 12_500_i32) + (17_500_i32 * 17_500_i32)
        );

        q[5] = -10_000;
        q[6] = -10_000;
        assert_eq!(
            cell_lower_bound_sq(&q, absent_key, present_key),
            200_000_000
        );
    }

    #[test]
    fn neighbor_keys_count() {
        // A middle key should have exactly 4 neighbors (hour ± 1, day ± 1)
        let hour = 12u16;
        let day = 3u16;
        let key = (hour << 3) | day;
        let n: Vec<_> = neighbor_keys(key).collect();
        assert_eq!(n.len(), 4);
    }
}
