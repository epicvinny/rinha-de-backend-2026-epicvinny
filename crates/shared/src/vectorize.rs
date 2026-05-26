use crate::normalization::Constants;
use crate::types::{Payload, Vec14F};
use chrono::{DateTime, Datelike, Timelike, Utc};

#[inline]
pub fn clamp01(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

#[inline]
pub fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastDateTime {
    pub hour: u32,
    pub day_of_week: u32, // Mon=0, Sun=6
    pub epoch_seconds: i64,
}

#[inline]
fn parse_digits_2(s: &[u8], offset: usize) -> u32 {
    let d1 = (s[offset] - b'0') as u32;
    let d2 = (s[offset + 1] - b'0') as u32;
    d1 * 10 + d2
}

#[inline]
fn parse_digits_4(s: &[u8], offset: usize) -> i32 {
    let d1 = (s[offset] - b'0') as i32;
    let d2 = (s[offset + 1] - b'0') as i32;
    let d3 = (s[offset + 2] - b'0') as i32;
    let d4 = (s[offset + 3] - b'0') as i32;
    d1 * 1000 + d2 * 100 + d3 * 10 + d4
}

#[inline]
fn date_to_epoch_days(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 12 } else { month };

    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let y_of_era = y - era * 400;
    let day_of_year = ((153 * (m - 3) + 2) / 5 + day - 1) as i32;
    let day_of_era = y_of_era * 365 + y_of_era / 4 - y_of_era / 100 + day_of_year;

    let era_days = era as i64 * 146097;
    let days_from_civil_epoch = era_days + day_of_era as i64;

    days_from_civil_epoch - 719468
}

pub fn parse_iso8601(s: &str) -> Option<FastDateTime> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }

    // Check if the characters are digits where expected
    for &idx in &[0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[idx].is_ascii_digit() {
            return None;
        }
    }

    let year = parse_digits_4(bytes, 0);
    let month = parse_digits_2(bytes, 5);
    let day = parse_digits_2(bytes, 8);
    let hour = parse_digits_2(bytes, 11);
    let minute = parse_digits_2(bytes, 14);
    let second = parse_digits_2(bytes, 17);

    if month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let epoch_days = date_to_epoch_days(year, month, day);
    let mut weekday = (epoch_days + 3) % 7;
    if weekday < 0 {
        weekday += 7;
    }

    let epoch_seconds =
        epoch_days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;

    Some(FastDateTime {
        hour,
        day_of_week: weekday as u32,
        epoch_seconds,
    })
}

#[inline]
pub fn fast_parse_mcc(mcc: &str) -> usize {
    let b = mcc.as_bytes();
    if b.len() == 4 {
        let d0 = b[0].wrapping_sub(b'0') as usize;
        let d1 = b[1].wrapping_sub(b'0') as usize;
        let d2 = b[2].wrapping_sub(b'0') as usize;
        let d3 = b[3].wrapping_sub(b'0') as usize;
        if d0 < 10 && d1 < 10 && d2 < 10 && d3 < 10 {
            return d0 * 1000 + d1 * 100 + d2 * 10 + d3;
        }
    }
    0
}

pub fn vectorize(p: &Payload<'_>, c: &Constants) -> Vec14F {
    let (hour, day_of_week, epoch_seconds) =
        if let Some(fdt) = parse_iso8601(&p.transaction.requested_at) {
            (fdt.hour as f64, fdt.day_of_week as f64, fdt.epoch_seconds)
        } else {
            let dt: DateTime<Utc> = p
                .transaction
                .requested_at
                .parse()
                .expect("invalid datetime");
            (
                dt.hour() as f64,
                dt.weekday().num_days_from_monday() as f64,
                dt.timestamp(),
            )
        };

    // dim 0: amount
    let d0 = clamp01(p.transaction.amount / c.max_amount);

    // dim 1: installments
    let d1 = clamp01(p.transaction.installments as f64 / c.max_installments);

    // dim 2: amount_vs_avg
    let d2 = if p.customer.avg_amount == 0.0 {
        1.0
    } else {
        clamp01((p.transaction.amount / p.customer.avg_amount) / c.amount_vs_avg_ratio)
    };

    // dim 3: hour_of_day
    let d3 = hour / 23.0;

    // dim 4: day_of_week
    let d4 = day_of_week / 6.0;

    // dims 5 & 6: last_transaction
    let (d5, d6) = if let Some(ref lt) = p.last_transaction {
        let last_epoch = if let Some(fdt) = parse_iso8601(&lt.timestamp) {
            fdt.epoch_seconds
        } else {
            let last_dt: DateTime<Utc> = lt
                .timestamp
                .parse()
                .expect("invalid last_transaction timestamp");
            last_dt.timestamp()
        };
        let minutes = (epoch_seconds - last_epoch).abs() as f64 / 60.0;
        let d5 = clamp01(minutes / c.max_minutes);
        let d6 = clamp01(lt.km_from_current / c.max_km);
        (d5, d6)
    } else {
        (-1.0, -1.0)
    };

    // dim 7: km_from_home
    let d7 = clamp01(p.terminal.km_from_home / c.max_km);

    // dim 8: tx_count_24h
    let d8 = clamp01(p.customer.tx_count_24h as f64 / c.max_tx_count_24h);

    // dim 9: is_online
    let d9 = if p.terminal.is_online { 1.0 } else { 0.0 };

    // dim 10: card_present
    let d10 = if p.terminal.card_present { 1.0 } else { 0.0 };

    // dim 11: unknown_merchant (1 = unknown, 0 = known)
    let known = p.customer.known_merchants.contains(p.merchant.id);
    let d11 = if known { 0.0 } else { 1.0 };

    // dim 12: mcc_risk
    let d12 = *c.mcc_risk.get(p.merchant.mcc).unwrap_or(&c.mcc_default);

    // dim 13: merchant_avg_amount
    let d13 = clamp01(p.merchant.avg_amount / c.max_merchant_avg_amount);

    [d0, d1, d2, d3, d4, d5, d6, d7, d8, d9, d10, d11, d12, d13]
}

pub fn vectorize_round4(p: &Payload<'_>, c: &Constants) -> Vec14F {
    let v = vectorize(p, c);
    [
        round4(v[0]),
        round4(v[1]),
        round4(v[2]),
        round4(v[3]),
        round4(v[4]),
        round4(v[5]),
        round4(v[6]),
        round4(v[7]),
        round4(v[8]),
        round4(v[9]),
        round4(v[10]),
        round4(v[11]),
        round4(v[12]),
        round4(v[13]),
    ]
}

pub fn vectorize_to_i16(p: &Payload<'_>, c: &Constants) -> crate::types::Vec16I {
    let (v, _) = vectorize_to_i16_and_key(p, c);
    v
}

pub fn vectorize_to_i16_and_key(p: &Payload<'_>, c: &Constants) -> (crate::types::Vec16I, u16) {
    let (hour_u16, day_u16, epoch_seconds) =
        if let Some(fdt) = parse_iso8601(&p.transaction.requested_at) {
            (fdt.hour as u16, fdt.day_of_week as u16, fdt.epoch_seconds)
        } else {
            use chrono::{DateTime, Datelike, Timelike, Utc};
            let dt: DateTime<Utc> = p
                .transaction
                .requested_at
                .parse()
                .expect("invalid datetime");
            (
                dt.hour() as u16,
                dt.weekday().num_days_from_monday() as u16,
                dt.timestamp(),
            )
        };

    let mut out = [0i16; 16];

    // dim 0: amount
    out[0] = (clamp01(p.transaction.amount / c.max_amount) * 10000.0).round() as i16;

    // dim 1: installments
    out[1] =
        (clamp01(p.transaction.installments as f64 / c.max_installments) * 10000.0).round() as i16;

    // dim 2: amount_vs_avg
    let d2 = if p.customer.avg_amount == 0.0 {
        1.0
    } else {
        clamp01((p.transaction.amount / p.customer.avg_amount) / c.amount_vs_avg_ratio)
    };
    out[2] = (d2 * 10000.0).round() as i16;

    // dim 3: hour_of_day (hour_f / 23.0)
    out[3] = crate::cell::HOUR_Q[hour_u16 as usize];

    // dim 4: day_of_week (day_of_week_f / 6.0)
    out[4] = crate::cell::DAY_Q[day_u16 as usize];

    // dims 5 & 6: last_transaction
    if let Some(ref lt) = p.last_transaction {
        let last_epoch = if let Some(fdt) = parse_iso8601(&lt.timestamp) {
            fdt.epoch_seconds
        } else {
            use chrono::{DateTime, Utc};
            let last_dt: DateTime<Utc> = lt
                .timestamp
                .parse()
                .expect("invalid last_transaction timestamp");
            last_dt.timestamp()
        };
        let minutes = (epoch_seconds - last_epoch).abs() as f64 / 60.0;
        let d5 = clamp01(minutes / c.max_minutes);
        let d6 = clamp01(lt.km_from_current / c.max_km);
        out[5] = (d5 * 10000.0).round() as i16;
        out[6] = (d6 * 10000.0).round() as i16;
    } else {
        out[5] = -10000;
        out[6] = -10000;
    }

    // dim 7: km_from_home
    out[7] = (clamp01(p.terminal.km_from_home / c.max_km) * 10000.0).round() as i16;

    // dim 8: tx_count_24h
    out[8] =
        (clamp01(p.customer.tx_count_24h as f64 / c.max_tx_count_24h) * 10000.0).round() as i16;

    // dim 9: is_online
    let is_online_val = if p.terminal.is_online { 10000 } else { 0 };
    out[9] = is_online_val;

    // dim 10: card_present
    let card_present_val = if p.terminal.card_present { 10000 } else { 0 };
    out[10] = card_present_val;

    // dim 11: unknown_merchant (1 = unknown, 0 = known)
    let known = p.customer.known_merchants.contains(p.merchant.id);
    out[11] = if known { 0 } else { 10000 };

    // dim 12: mcc_risk
    let mcc_val = fast_parse_mcc(p.merchant.mcc);
    out[12] = if mcc_val < 10000 {
        c.mcc_risk_i16[mcc_val]
    } else {
        5000 // default for invalid MCC
    };

    // dim 13: merchant_avg_amount
    out[13] = (clamp01(p.merchant.avg_amount / c.max_merchant_avg_amount) * 10000.0).round() as i16;

    // Compute cell key
    let is_online = if is_online_val != 0 { 1u16 } else { 0u16 };
    let card_present = if card_present_val != 0 { 1u16 } else { 0u16 };
    let unknown_merch = if known { 0u16 } else { 1u16 };
    let sent = if p.last_transaction.is_some() {
        1u16
    } else {
        0u16
    };

    let key = (is_online << 12)
        | (card_present << 11)
        | (unknown_merch << 10)
        | (sent << 9)
        | (sent << 8)
        | (hour_u16 << 3)
        | day_u16;

    (out, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Datelike, Timelike, Utc};

    #[test]
    fn round4_basic() {
        assert_eq!(round4(0.5), 0.5);
        assert_eq!(round4(0.0), 0.0);
        assert_eq!(round4(1.0), 1.0);
        assert_eq!(round4(-1.0), -1.0);
        // half-away-from-zero rounding
        assert_eq!(round4(0.99995), 1.0);
        assert_eq!(round4(0.12345), 0.1235);
    }

    #[test]
    fn round4_idempotent() {
        for i in 0..=100 {
            let x = i as f64 / 100.0;
            assert_eq!(round4(round4(x)), round4(x));
        }
    }

    #[test]
    fn fast_iso8601_vs_chrono() {
        let test_cases = vec![
            "1970-01-01T00:00:00Z",
            "1970-01-01T01:02:03Z",
            "1972-02-29T12:00:00Z", // leap year
            "2000-02-29T23:59:59Z", // leap year
            "2025-01-15T14:30:00Z",
            "2026-05-24T22:30:00Z",
            "2026-12-31T23:59:59Z",
            "2100-02-28T12:00:00Z", // non-leap year divisible by 100 but not 400
            "2025-01-15T14:30:00.123Z", // with fractional seconds
        ];

        for ts in test_cases {
            let fdt = parse_iso8601(ts).expect("fast parse failed");
            let dt: DateTime<Utc> = ts.parse().expect("chrono parse failed");

            assert_eq!(fdt.hour, dt.hour(), "hour mismatch for {}", ts);
            assert_eq!(
                fdt.day_of_week,
                dt.weekday().num_days_from_monday(),
                "weekday mismatch for {}",
                ts
            );
            assert_eq!(
                fdt.epoch_seconds,
                dt.timestamp(),
                "epoch_seconds mismatch for {}",
                ts
            );
        }
    }

    #[test]
    fn fast_iso8601_invalid() {
        assert!(parse_iso8601("invalid").is_none());
        assert!(parse_iso8601("2025/01/15T14:30:00Z").is_none());
        assert!(parse_iso8601("2025-01-15T14:30:60Z").is_none()); // invalid second
        assert!(parse_iso8601("2025-01-15T24:30:00Z").is_none()); // invalid hour
        assert!(parse_iso8601("2025-13-15T14:30:00Z").is_none()); // invalid month
    }

    #[test]
    fn direct_vectorize_parity() {
        let payload = crate::types::Payload {
            id: "tx-123",
            transaction: crate::types::Transaction {
                amount: 154.22,
                installments: 3,
                requested_at: "2026-05-24T22:30:00Z",
            },
            customer: crate::types::Customer {
                avg_amount: 100.0,
                tx_count_24h: 2,
                known_merchants: crate::types::KnownMerchants::from_slice(&["merchant-abc"]),
            },
            merchant: crate::types::Merchant {
                id: "merchant-abc",
                mcc: "5411",
                avg_amount: 250.0,
            },
            terminal: crate::types::Terminal {
                is_online: true,
                card_present: true,
                km_from_home: 12.5,
            },
            last_transaction: Some(crate::types::LastTransaction {
                timestamp: "2026-05-24T22:00:00Z",
                km_from_current: 5.0,
            }),
        };

        let constants = Constants::load_embedded();
        let direct_v = vectorize_to_i16(&payload, &constants);
        let q_round4 = crate::quantize::quantize(&vectorize_round4(&payload, &constants));

        assert_eq!(direct_v, q_round4);
    }
}
