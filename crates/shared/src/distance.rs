use crate::types::Vec16I;

#[inline]
pub fn l2sq_scalar(query: &Vec16I, reference: &Vec16I) -> i32 {
    let mut sum = 0i32;
    for i in 0..16 {
        let d = (query[i] as i32) - (reference[i] as i32);
        sum += d * d;
    }
    sum
}

#[inline]
pub fn l2sq_avx2(query: &Vec16I, reference: &Vec16I) -> i32 {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    {
        unsafe {
            use std::arch::x86_64::*;
            let q = _mm256_loadu_si256(query.as_ptr() as *const __m256i);
            let r = _mm256_loadu_si256(reference.as_ptr() as *const __m256i);
            let diff = _mm256_sub_epi16(q, r);
            let sq_sum = _mm256_madd_epi16(diff, diff);

            let low = _mm256_castsi256_si128(sq_sum);
            let high = _mm256_extracti128_si256(sq_sum, 1);
            let sum128 = _mm_add_epi32(low, high);

            let s1 = _mm_shuffle_epi32(sum128, 0x4E);
            let sum2 = _mm_add_epi32(sum128, s1);
            let s2 = _mm_shuffle_epi32(sum2, 0xB1);
            let sum3 = _mm_add_epi32(sum2, s2);

            _mm_cvtsi128_si32(sum3)
        }
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
    {
        l2sq_scalar(query, reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_vector_distance_zero() {
        let v = [
            100i16, 200, 300, 400, 500, -10000, -10000, 100, 250, 0, 10000, 0, 2000, 416, 0, 0,
        ];
        assert_eq!(l2sq_scalar(&v, &v), 0);
        assert_eq!(l2sq_avx2(&v, &v), 0);
    }

    #[test]
    fn known_distance() {
        let a = [0i16; 16];
        let mut b = [0i16; 16];
        b[0] = 100;
        b[1] = 200;
        // distance = 100^2 + 200^2 = 10000 + 40000 = 50000
        assert_eq!(l2sq_scalar(&a, &b), 50000);
        assert_eq!(l2sq_avx2(&a, &b), 50000);
    }

    #[test]
    fn exact_match_random_range() {
        let mut a = [0i16; 16];
        let mut b = [0i16; 16];
        for i in 0..16 {
            a[i] = (i * 37) as i16 - 150;
            b[i] = (i * 73) as i16 - 320;
        }
        assert_eq!(l2sq_avx2(&a, &b), l2sq_scalar(&a, &b));
    }
}
