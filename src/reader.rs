//! TDX VIPDOC binary file reader
//!
//! Parses .day (daily bar) and .lc1/.lc5 (minute bar) files from TDX vipdoc directory.
//! Each daily record is 32 bytes; each minute record is also 32 bytes but with a different layout.

use memmap2::Mmap;
use pyo3::prelude::*;
use std::fs::File;
use std::path::Path;

// ---- Daily Bar (.day file) ----
// 32 bytes per record, little-endian:
//   date:     u32  (YYYYMMDD as integer)
//   open:     u32  (price in 分/百, needs coefficient)
//   high:     u32
//   low:      u32
//   close:    u32
//   amount:   f32  (turnover in yuan)
//   volume:   u32  (shares traded)
//   _reserved: u32

const DAILY_RECORD_SIZE: usize = 32;

/// Read daily bars from a .day file. Returns a list of dicts.
///
/// Each dict has keys: date, open, high, low, close, amount, volume.
/// Price values are divided by the given `price_coeff` (default 0.01 → 分→元).
/// Amount values are divided by `vol_coeff` (default 0.01).
#[pyfunction]
#[pyo3(signature = (filepath, price_coeff=0.01, vol_coeff=0.01))]
pub fn read_daily_bars(
    py: Python<'_>,
    filepath: &str,
    price_coeff: f64,
    vol_coeff: f64,
) -> PyResult<Vec<PyObject>> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(pyo3::exceptions::PyFileNotFoundError::new_err(format!(
            "File not found: {}",
            filepath
        )));
    }

    let file = File::open(path)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Cannot open file: {}", e)))?;

    let mmap = unsafe { Mmap::map(&file) }
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Cannot mmap file: {}", e)))?;

    let data = &mmap[..];
    let record_count = data.len() / DAILY_RECORD_SIZE;
    let mut results = Vec::with_capacity(record_count);

    for i in 0..record_count {
        let offset = i * DAILY_RECORD_SIZE;
        let chunk = &data[offset..offset + DAILY_RECORD_SIZE];

        let date = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let open = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let high = u32::from_le_bytes(chunk[8..12].try_into().unwrap());
        let low = u32::from_le_bytes(chunk[12..16].try_into().unwrap());
        let close = u32::from_le_bytes(chunk[16..20].try_into().unwrap());
        let amount = f32::from_le_bytes(chunk[20..24].try_into().unwrap());
        let volume = u32::from_le_bytes(chunk[24..28].try_into().unwrap());
        // chunk[28..32] is reserved

        let date_str = format!(
            "{}-{:02}-{:02}",
            date / 10000,
            (date % 10000) / 100,
            date % 100,
        );

        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("date", &date_str)?;
        dict.set_item("open", open as f64 * price_coeff)?;
        dict.set_item("high", high as f64 * price_coeff)?;
        dict.set_item("low", low as f64 * price_coeff)?;
        dict.set_item("close", close as f64 * price_coeff)?;
        dict.set_item("amount", amount as f64)?;
        dict.set_item("volume", volume as f64 * vol_coeff)?;

        results.push(dict.into_any().unbind());
    }

    Ok(results)
}

// ---- Minute Bar (.lc1 / .lc5 file) ----
// 32 bytes per record, little-endian:
//   date:    u16  (days since 2004-01-01?? actually encoded)
//   time:    u16  (HHMM as integer)
//   open:    f32
//   high:    f32
//   low:     f32
//   close:   f32
//   amount:  f32
//   volume:  u32

const MINUTE_RECORD_SIZE: usize = 32;

/// Read minute bars from a .lc1 or .lc5 file. Returns a list of dicts.
///
/// Each dict has keys: datetime, open, high, low, close, amount, volume.
#[pyfunction]
#[pyo3(signature = (filepath,))]
pub fn read_minute_bars(py: Python<'_>, filepath: &str) -> PyResult<Vec<PyObject>> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(pyo3::exceptions::PyFileNotFoundError::new_err(format!(
            "File not found: {}",
            filepath
        )));
    }

    let file = File::open(path)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Cannot open file: {}", e)))?;

    let mmap = unsafe { Mmap::map(&file) }
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Cannot mmap file: {}", e)))?;

    let data = &mmap[..];
    let record_count = data.len() / MINUTE_RECORD_SIZE;
    let mut results = Vec::with_capacity(record_count);

    for i in 0..record_count {
        let offset = i * MINUTE_RECORD_SIZE;
        let chunk = &data[offset..offset + MINUTE_RECORD_SIZE];

        let raw_date = u16::from_le_bytes(chunk[0..2].try_into().unwrap());
        let raw_time = u16::from_le_bytes(chunk[2..4].try_into().unwrap());

        // TDX minute date encoding:
        // year  = (raw_date >> 11) + 2004
        // month = (raw_date >> 7) & 0x0F  (but value in range, may need % 100)
        // day   = raw_date & 0x1F
        let year = (raw_date >> 11) + 2004;
        let month = (raw_date >> 7) & 0x0F;
        let day = raw_date & 0x1F;

        let hour = raw_time / 60;
        let minute = raw_time % 60;

        let open = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let high = f32::from_le_bytes(chunk[8..12].try_into().unwrap());
        let low = f32::from_le_bytes(chunk[12..16].try_into().unwrap());
        let close = f32::from_le_bytes(chunk[16..20].try_into().unwrap());
        let amount = f32::from_le_bytes(chunk[20..24].try_into().unwrap());
        let volume = u32::from_le_bytes(chunk[24..28].try_into().unwrap());
        // chunk[28..32] is reserved

        let datetime_str = format!("{}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, minute,);

        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("datetime", &datetime_str)?;
        dict.set_item("open", open as f64)?;
        dict.set_item("high", high as f64)?;
        dict.set_item("low", low as f64)?;
        dict.set_item("close", close as f64)?;
        dict.set_item("amount", amount as f64)?;
        dict.set_item("volume", volume as f64)?;

        results.push(dict.into_any().unbind());
    }

    Ok(results)
}
