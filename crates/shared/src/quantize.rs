use crate::types::{Vec14F, Vec16I};

pub const Q_SCALE: f64 = 10000.0;
pub const SENTINEL_I16: i16 = -10000;

#[inline]
pub fn quantize(v: &Vec14F) -> Vec16I {
    let mut out = [0i16; 16];
    for i in 0..14 {
        out[i] = (v[i] * Q_SCALE).round() as i16;
    }
    // dims 14, 15 are padding zeros
    out
}

pub fn fraud_score_to_string(v: f64) -> String {
    if v == v.trunc() && v >= 0.0 {
        // integer value — emit without decimal
        format!("{}", v as i64)
    } else {
        // use compact representation
        let s = format!("{}", v);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraud_score_fmt() {
        assert_eq!(fraud_score_to_string(0.0), "0");
        assert_eq!(fraud_score_to_string(1.0), "1");
        assert_eq!(fraud_score_to_string(0.6), "0.6");
        assert_eq!(fraud_score_to_string(0.4), "0.4");
        assert_eq!(fraud_score_to_string(0.8), "0.8");
    }

    #[test]
    fn quantize_sentinel() {
        let mut v = [0.0f64; 14];
        v[5] = -1.0;
        v[6] = -1.0;
        let q = quantize(&v);
        assert_eq!(q[5], -10000);
        assert_eq!(q[6], -10000);
        assert_eq!(q[14], 0);
        assert_eq!(q[15], 0);
    }

    #[test]
    fn quantize_bounds() {
        let v = [
            0.5, 1.0, 0.0, 0.8261, 0.1667, -1.0, -1.0, 0.0432, 0.25, 0.0, 1.0, 0.0, 0.2, 0.0416,
        ];
        let q = quantize(&v);
        assert_eq!(q[0], 5000);
        assert_eq!(q[1], 10000);
        assert_eq!(q[2], 0);
        assert_eq!(q[5], -10000);
        assert_eq!(q[9], 0);
        assert_eq!(q[10], 10000);
    }
}
