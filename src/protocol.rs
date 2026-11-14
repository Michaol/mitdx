//! Decoding utilities for TDX network protocol.
//! TDX uses a highly custom variable-length integer encoding to compress price arrays, and complex
//! float encodings over integers for volume, in network responses.

/// Decodes the TDX custom variable-length price differential format.
/// The data is encoded somewhat like a UTF-8 string:
/// The first byte contains the lower 6 bits (0x3F), and flags for signs and continuation.
/// Returns the decoded integer difference, and the index in buffer after consuming bytes.
pub fn get_price(data: &[u8], mut pos: usize) -> (i64, usize) {
    if pos >= data.len() {
        return (0, pos);
    }

    let mut pos_byte = 6;
    let mut bdata = data[pos];
    let mut int_data = (bdata & 0x3F) as i64;
    let sign = (bdata & 0x40) != 0;

    if (bdata & 0x80) != 0 {
        loop {
            pos += 1;
            if pos >= data.len() {
                break;
            }
            bdata = data[pos];
            // F9: an i64 shift of >= 64 panics even in release builds. Malformed
            // input with ~10+ continuation bytes reaches pos_byte >= 64, so stop
            // decoding here instead of panicking. Real TDX values never get close.
            if pos_byte >= 64 {
                break;
            }
            // wrapping_add: for well-formed values this is a plain add; it only
            // differs on hostile input that would overflow i64 (debug panic).
            int_data = int_data.wrapping_add(((bdata & 0x7F) as i64) << pos_byte);
            pos_byte += 7;

            if (bdata & 0x80) == 0 {
                break;
            }
        }
    }
    pos += 1;

    if sign {
        // wrapping_neg avoids a debug panic when int_data == i64::MIN (hostile input).
        int_data = int_data.wrapping_neg();
    }

    (int_data, pos)
}

/// Bounds-checked variant of [`get_price`] for record-parsing loops.
///
/// `get_price` silently returns `(0, pos)` once `pos` is at/past the end of the
/// buffer, which makes truncated responses emit zero-filled garbage records.
/// This variant returns `None` in that case so callers can `break` the loop.
/// The `(i64, usize)` decoding itself is unchanged. (F9)
pub fn get_price_checked(data: &[u8], pos: usize) -> Option<(i64, usize)> {
    if pos >= data.len() {
        return None;
    }
    Some(get_price(data, pos))
}

/// Decodes the TDX custom float format used for trade volume.
/// It unpacks a 32-bit integer representing volume data into a double using bit masking
/// and powers of 2. Reverse engineered from tdxpy/helper.py.
pub fn get_volume(vol: u32) -> f64 {
    let logpoint = vol >> 24;
    let hleax = (vol >> 16) & 0xFF;
    let lheax = (vol >> 8) & 0xFF;
    let lleax = vol & 0xFF;

    let dw_ecx = (logpoint as i32) * 2 - 0x7F;
    let dw_edx = (logpoint as i32) * 2 - 0x86;
    let dw_esi = (logpoint as i32) * 2 - 0x8E;
    let dw_eax = (logpoint as i32) * 2 - 0x96;

    let tmp_eax = if dw_ecx < 0 { -dw_ecx } else { dw_ecx };
    let mut dbl_xmm6 = 2.0_f64.powi(tmp_eax);
    if dw_ecx < 0 {
        dbl_xmm6 = 1.0 / dbl_xmm6;
    }

    let dbl_xmm4 = if hleax > 0x80 {
        let dwtmpeax = dw_edx + 1;
        let tmpdbl_xmm3 = 2.0_f64.powi(dwtmpeax);
        let mut dbl_xmm0 = 2.0_f64.powi(dw_edx) * 128.0;
        dbl_xmm0 += ((hleax & 0x7F) as f64) * tmpdbl_xmm3;
        dbl_xmm0
    } else if dw_edx >= 0 {
        2.0_f64.powi(dw_edx) * (hleax as f64)
    } else {
        (1.0 / 2.0_f64.powi(dw_edx)) * (hleax as f64)
    };

    let mut dbl_xmm3 = 2.0_f64.powi(dw_esi) * (lheax as f64);
    let mut dbl_xmm1 = 2.0_f64.powi(dw_eax) * (lleax as f64);

    if (hleax & 0x80) != 0 {
        dbl_xmm3 *= 2.0;
        dbl_xmm1 *= 2.0;
    }

    dbl_xmm6 + dbl_xmm4 + dbl_xmm3 + dbl_xmm1
}

/// Helper to decode datetime from category.
/// Category < 4 or 7, 8 means minute data.
/// Returns (year, month, day, hour, minute, pos)
pub fn get_datetime(
    category: u16,
    data: &[u8],
    mut pos: usize,
) -> Option<(u32, u32, u32, u32, u32, usize)> {
    if pos + 4 > data.len() {
        return None;
    }

    let year;
    let month;
    let day;
    let mut hour = 15;
    let mut minute = 0;

    if category < 4 || category == 7 || category == 8 {
        let zip_day = u16::from_le_bytes(data[pos..pos + 2].try_into().ok()?);
        let minutes = u16::from_le_bytes(data[pos + 2..pos + 4].try_into().ok()?);

        month = ((zip_day % 2048) / 100) as u32;
        year = ((zip_day >> 11) + 2004) as u32;
        day = ((zip_day % 2048) % 100) as u32;

        minute = (minutes % 60) as u32;
        hour = (minutes / 60) as u32;
    } else {
        let zip_day = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);

        month = (zip_day % 10000) / 100;
        year = zip_day / 10000;
        day = zip_day % 100;
    }

    pos += 4;

    Some((year, month, day, hour, minute, pos))
}

/// Helper to decode HH:MM time from u16.
/// Returns (hour, minute, pos)
pub fn get_time(data: &[u8], mut pos: usize) -> Option<(u32, u32, usize)> {
    if pos + 2 > data.len() {
        return None;
    }
    let tminutes = u16::from_le_bytes(data[pos..pos + 2].try_into().ok()?);
    let hour = (tminutes / 60) as u32;
    let minute = (tminutes % 60) as u32;
    pos += 2;
    Some((hour, minute, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode an integer into the TDX varint format (inverse of get_price),
    /// used to build round-trip test vectors.
    fn encode_price(value: i64) -> Vec<u8> {
        let sign = value < 0;
        let mut v = value.unsigned_abs();
        let mut out = Vec::new();
        let mut first = (v & 0x3F) as u8;
        v >>= 6;
        if sign {
            first |= 0x40;
        }
        if v > 0 {
            first |= 0x80;
        }
        out.push(first);
        while v > 0 {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v > 0 {
                b |= 0x80;
            }
            out.push(b);
        }
        out
    }

    // ---- get_price: single byte / sign ----

    #[test]
    fn price_single_byte_normal() {
        assert_eq!(get_price(&[0x00], 0), (0, 1));
        assert_eq!(get_price(&[0x05], 0), (5, 1));
        assert_eq!(get_price(&[0x3F], 0), (63, 1));
    }

    #[test]
    fn price_single_byte_sign() {
        assert_eq!(get_price(&[0x40], 0), (0, 1)); // -0 == 0
        assert_eq!(get_price(&[0x45], 0), (-5, 1));
        assert_eq!(get_price(&[0x7F], 0), (-63, 1));
    }

    // ---- get_price: continuation bytes ----

    #[test]
    fn price_continuation_hand_computed() {
        // 64 = 1 << 6: first byte low-6 bits 0 + cont, second byte 1.
        assert_eq!(get_price(&[0x80, 0x01], 0), (64, 2));
        // 8192 = 1 << 13: two continuation bytes.
        assert_eq!(get_price(&[0x80, 0x80, 0x01], 0), (8192, 3));
        // negative continuation: -64
        assert_eq!(get_price(&[0xC0, 0x01], 0), (-64, 2));
    }

    #[test]
    fn price_round_trip_many_values() {
        let values = [
            0,
            1,
            63,
            64,
            127,
            128,
            8191,
            8192,
            1_000_000,
            123_456_789,
            -1,
            -63,
            -64,
            -8192,
            -1_000_000,
            i64::MAX,
            -i64::MAX,
        ];
        for v in values {
            let enc = encode_price(v);
            // prefix bytes ensure pos arithmetic is exercised, not just pos=0
            let mut data = vec![0xAA, 0xBB];
            data.extend_from_slice(&enc);
            let (decoded, next) = get_price(&data, 2);
            assert_eq!(decoded, v, "round trip failed for {}", v);
            assert_eq!(next, 2 + enc.len());
        }
    }

    #[test]
    fn price_round_trip_sequential() {
        // Several varints back to back, as in a real record.
        let vals = [100i64, -250, 64, 0, 999_999];
        let mut data = Vec::new();
        for v in vals {
            data.extend_from_slice(&encode_price(v));
        }
        let mut pos = 0;
        for v in vals {
            let (decoded, next) = get_price(&data, pos);
            assert_eq!(decoded, v);
            pos = next;
        }
        assert_eq!(pos, data.len());
    }

    // ---- get_price: truncated input ----

    #[test]
    fn price_empty_or_past_end() {
        assert_eq!(get_price(&[], 0), (0, 0));
        assert_eq!(get_price(&[0x05], 1), (0, 1)); // pos == len
        assert_eq!(get_price(&[0x05], 99), (0, 99));
    }

    #[test]
    fn price_truncated_continuation_no_panic() {
        // Continuation flag set but buffer ends: must not panic.
        let (v, p) = get_price(&[0x80], 0);
        assert_eq!(v, 0);
        assert!(p >= 1);
        let (v2, p2) = get_price(&[0x80, 0x80], 0);
        assert_eq!(v2, 0);
        assert!(p2 >= 2);
    }

    #[test]
    fn price_twelve_continuation_bytes_no_panic() {
        // F9 regression: 12 continuation bytes used to shift by >= 64 and panic.
        let data = vec![0xFFu8; 13];
        let (_v, p) = get_price(&data, 0);
        assert!(p <= data.len() + 1);
        // Also the all-0x80 variant (zero payload, many continuation flags).
        let data2 = vec![0x80u8; 13];
        let (_v2, p2) = get_price(&data2, 0);
        assert!(p2 <= data2.len() + 1);
        // A long run terminated by a final non-continuation byte.
        let mut data3 = vec![0xFFu8; 12];
        data3.push(0x01);
        let (_v3, p3) = get_price(&data3, 0);
        assert!(p3 <= data3.len() + 1);
    }

    #[test]
    fn price_checked_bounds() {
        assert_eq!(get_price_checked(&[], 0), None);
        assert_eq!(get_price_checked(&[0x05], 1), None);
        assert_eq!(get_price_checked(&[0x05], 0), Some((5, 1)));
        assert_eq!(get_price_checked(&[0x80, 0x01], 0), Some((64, 2)));
    }

    // ---- get_datetime ----

    #[test]
    fn datetime_minute_branch() {
        // 2026-11-14 09:30, category 0 (minute data)
        let zip_day: u16 = ((2026 - 2004) << 11) | (11 * 100 + 14);
        let minutes: u16 = 9 * 60 + 30;
        let mut data = Vec::new();
        data.extend_from_slice(&zip_day.to_le_bytes());
        data.extend_from_slice(&minutes.to_le_bytes());
        for cat in [0u16, 1, 2, 3, 7, 8] {
            assert_eq!(
                get_datetime(cat, &data, 0),
                Some((2026, 11, 14, 9, 30, 4)),
                "category {}",
                cat
            );
        }
    }

    #[test]
    fn datetime_daily_branch() {
        // 2026-11-14, category 9 (date-only); hour/minute default to 15:00
        let zip_day: u32 = 20261114;
        let data = zip_day.to_le_bytes();
        for cat in [4u16, 5, 6, 9, 10] {
            assert_eq!(
                get_datetime(cat, &data, 0),
                Some((2026, 11, 14, 15, 0, 4)),
                "category {}",
                cat
            );
        }
    }

    #[test]
    fn datetime_with_offset() {
        let zip_day: u32 = 19910403;
        let mut data = vec![0x00, 0x01];
        data.extend_from_slice(&zip_day.to_le_bytes());
        assert_eq!(get_datetime(9, &data, 2), Some((1991, 4, 3, 15, 0, 6)));
    }

    #[test]
    fn datetime_truncated_none() {
        assert_eq!(get_datetime(0, &[], 0), None);
        assert_eq!(get_datetime(0, &[0x00, 0x01, 0x02], 0), None); // 3 < 4
        assert_eq!(get_datetime(9, &[0x00, 0x01, 0x02], 0), None);
        assert_eq!(get_datetime(0, &[0x00; 4], 1), None); // pos+4 > len
    }

    // ---- get_time ----

    #[test]
    fn time_normal() {
        let data = (9u16 * 60 + 30).to_le_bytes();
        assert_eq!(get_time(&data, 0), Some((9, 30, 2)));
        assert_eq!(get_time(&0u16.to_le_bytes(), 0), Some((0, 0, 2)));
        assert_eq!(get_time(&(23u16 * 60 + 59).to_le_bytes(), 0), Some((23, 59, 2)));
    }

    #[test]
    fn time_with_offset_and_truncated() {
        let mut data = vec![0xFF];
        data.extend_from_slice(&(14u16 * 60 + 5).to_le_bytes());
        assert_eq!(get_time(&data, 1), Some((14, 5, 3)));
        assert_eq!(get_time(&[0x00], 0), None);
        assert_eq!(get_time(&[], 0), None);
    }

    // ---- get_volume: hand-computed sanity values (cross-checked vs pytdx) ----

    #[test]
    fn volume_hand_computed() {
        // logpoint=0x40 -> dw_ecx=1 -> 2^1 = 2, all mantissa bytes 0.
        assert_eq!(get_volume(0x4000_0000), 2.0);
        // logpoint=0x3F -> dw_ecx=-1 -> 2^-1 = 0.5.
        assert_eq!(get_volume(0x3F00_0000), 0.5);
        // hleax=0x80, dw_edx=-6 -> (1/2^-6)*128 = 8192; +2 = 8194.
        assert_eq!(get_volume(0x4080_0000), 8194.0);
        // hleax=0x81 (>0x80 branch): 2 + 2^-6*128 + 1*2^-5 = 4.03125.
        assert_eq!(get_volume(0x4081_0000), 4.03125);
        // low bytes: 2 + 2^-14 + 2^-22 (exact powers of two).
        assert_eq!(
            get_volume(0x4000_0101),
            2.0 + 1.0 / 16384.0 + 1.0 / 4194304.0
        );
        // dw_edx>=0 branch: logpoint=0x43 -> 2^7=128; hleax=5 -> +5 = 133.
        assert_eq!(get_volume(0x4305_0000), 133.0);
        // zero input is 2^-127, not 0 (matches pytdx).
        assert_eq!(get_volume(0), 2.0_f64.powi(-127));
    }
}
