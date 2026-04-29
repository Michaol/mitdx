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
            int_data += ((bdata & 0x7F) as i64) << pos_byte;
            pos_byte += 7;

            if (bdata & 0x80) == 0 {
                break;
            }
        }
    }
    pos += 1;

    if sign {
        int_data = -int_data;
    }

    (int_data, pos)
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
