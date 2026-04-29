use flate2::read::ZlibDecoder;
use pyo3::prelude::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::{get_datetime, get_price, get_volume};

const CONNECT_TIMEOUT: u64 = 5;
const READ_TIMEOUT: u64 = 10;
const WRITE_TIMEOUT: u64 = 10;

// --- Protocol constants ---
// Response header is always 16 bytes: 12-byte preamble + 2-byte zip_size + 2-byte unzip_size.
const RESP_HEADER_LEN: usize = 16;

// Setup command 1: initial handshake (0x9318)
const SETUP_CMD1: [u8; 13] = [
    0x0c, 0x02, 0x18, 0x93, 0x00, 0x01, 0x03, 0x00, 0x03, 0x00, 0x0d, 0x00, 0x01,
];
// Setup command 2: secondary handshake (0x9418)
const SETUP_CMD2: [u8; 13] = [
    0x0c, 0x02, 0x18, 0x94, 0x00, 0x01, 0x03, 0x00, 0x03, 0x00, 0x0d, 0x00, 0x02,
];
// Setup command 3: client identification / version string
const SETUP_CMD3: [u8; 42] = [
    0x0c, 0x03, 0x18, 0x99, 0x00, 0x01, 0x20, 0x00, 0x20, 0x00, 0xdb, 0x0f, 0xd5, 0xd0, 0xc9, 0xcc,
    0xd6, 0xa4, 0xa8, 0xaf, 0x00, 0x00, 0x00, 0x8f, 0xc2, 0x25, 0x40, 0x13, 0x00, 0x00, 0xd5, 0x00,
    0xc9, 0xcc, 0xbd, 0xf0, 0xd7, 0xea, 0x00, 0x00, 0x00, 0x02,
];

// K-line bars request command
const CMD_ID_BARS: u16 = 0x052d;

/// Parse address string safely, returning a PyResult error on invalid input.
fn parse_addr(addr: &str) -> PyResult<std::net::SocketAddr> {
    addr.parse().map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("Invalid address '{}': {}", addr, e))
    })
}

/// Drain a setup response: read header, then read+discard the body.
/// Note: Errors are intentionally swallowed here. Setup responses are handshake
/// acknowledgements that don't carry meaningful data; if parsing fails we just
/// skip the body rather than aborting the connection.
fn drain_setup_response(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut header = [0u8; RESP_HEADER_LEN];
    if stream.read(&mut header).is_ok() {
        let zip_size = u16::from_le_bytes(header[12..14].try_into().unwrap_or([0, 0]));
        if zip_size > 0 {
            let mut buf = vec![0u8; zip_size as usize];
            let _ = stream.read_exact(&mut buf);
        }
    }
    Ok(())
}

#[pyclass]
pub struct TdxClient {
    stream: Option<TcpStream>,
}

#[pymethods]
impl TdxClient {
    #[new]
    pub fn new() -> Self {
        TdxClient { stream: None }
    }

    pub fn connect(&mut self, ip: &str, port: u16) -> PyResult<bool> {
        let addr = format!("{}:{}", ip, port);
        let sock_addr = parse_addr(&addr)?;

        match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(CONNECT_TIMEOUT)) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT)))
                    .map_err(|e| {
                        pyo3::exceptions::PyOSError::new_err(format!(
                            "Failed to set read timeout: {}",
                            e
                        ))
                    })?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(WRITE_TIMEOUT)))
                    .map_err(|e| {
                        pyo3::exceptions::PyOSError::new_err(format!(
                            "Failed to set write timeout: {}",
                            e
                        ))
                    })?;

                // Setup handshake sequence
                stream.write_all(&SETUP_CMD1)?;
                drain_setup_response(&mut stream)?;

                stream.write_all(&SETUP_CMD2)?;
                drain_setup_response(&mut stream)?;

                stream.write_all(&SETUP_CMD3)?;
                drain_setup_response(&mut stream)?;

                self.stream = Some(stream);
                Ok(true)
            }
            Err(e) => Err(pyo3::exceptions::PyConnectionError::new_err(format!(
                "Connect failed: {}",
                e
            ))),
        }
    }

    pub fn disconnect(&mut self) -> PyResult<()> {
        if let Some(stream) = self.stream.take() {
            stream.shutdown(std::net::Shutdown::Both).ok();
        }
        Ok(())
    }

    /// category: 0..11, market: 0/1, code: "600036", start: offset, count: num
    pub fn get_security_bars(
        &mut self,
        py: Python<'_>,
        category: u16,
        market: u16,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = match self.stream.as_mut() {
            Some(s) => s,
            None => {
                return Err(pyo3::exceptions::PyConnectionError::new_err(
                    "Not connected",
                ))
            }
        };

        // Build request packet
        let mut req = Vec::with_capacity(38);
        req.extend_from_slice(&0x10c_u16.to_le_bytes());
        req.extend_from_slice(&0x01016408_u32.to_le_bytes());
        req.extend_from_slice(&0x1c_u16.to_le_bytes()); // data length
        req.extend_from_slice(&0x1c_u16.to_le_bytes()); // data length (dup)
        req.extend_from_slice(&CMD_ID_BARS.to_le_bytes());
        req.extend_from_slice(&market.to_le_bytes());

        // Stock code: 6 ASCII bytes, zero-padded
        let mut code_bytes = [0u8; 6];
        let cd = code.as_bytes();
        let len = std::cmp::min(6, cd.len());
        code_bytes[..len].copy_from_slice(&cd[..len]);
        req.extend_from_slice(&code_bytes);

        req.extend_from_slice(&category.to_le_bytes());
        req.extend_from_slice(&1u16.to_le_bytes()); // unknown flag
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&count.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved
        req.extend_from_slice(&0u16.to_le_bytes()); // reserved

        stream.write_all(&req)?;

        // Read response header
        let mut header = [0u8; RESP_HEADER_LEN];
        stream.read_exact(&mut header)?;

        let zip_size =
            u16::from_le_bytes(header[12..14].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed response header")
            })?);
        let unzip_size =
            u16::from_le_bytes(header[14..16].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed response header")
            })?);

        let mut body = vec![0u8; zip_size as usize];
        stream.read_exact(&mut body)?;

        // Decompress if needed
        let data = if zip_size != unzip_size {
            let mut decoder = ZlibDecoder::new(&body[..]);
            let mut unzipped = vec![0u8; unzip_size as usize];
            decoder.read_exact(&mut unzipped)?;
            unzipped
        } else {
            body
        };

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let ret_count = u16::from_le_bytes(
            data[0..2]
                .try_into()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Malformed bar data"))?,
        );
        let mut pos = 2;
        let mut pre_diff_base: i64 = 0;

        let mut results = Vec::with_capacity(ret_count as usize);

        for _ in 0..ret_count {
            let (year, month, day, hour, minute, new_pos) = match get_datetime(category, &data, pos)
            {
                Some(v) => v,
                None => break,
            };
            pos = new_pos;

            let (mut price_open_diff, p1) = get_price(&data, pos);
            pos = p1;
            let (price_close_diff, p2) = get_price(&data, pos);
            pos = p2;
            let (price_high_diff, p3) = get_price(&data, pos);
            pos = p3;
            let (price_low_diff, p4) = get_price(&data, pos);
            pos = p4;

            // Boundary check: need 8 more bytes (vol u32 + amount u32)
            if pos + 8 > data.len() {
                break;
            }

            let vol_raw =
                u32::from_le_bytes(data[pos..pos + 4].try_into().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Malformed volume data")
                })?);
            let vol = get_volume(vol_raw);
            pos += 4;

            let db_vol_raw =
                u32::from_le_bytes(data[pos..pos + 4].try_into().map_err(|_| {
                    pyo3::exceptions::PyRuntimeError::new_err("Malformed amount data")
                })?);
            let amount = get_volume(db_vol_raw);
            pos += 4;

            let open = ((price_open_diff + pre_diff_base) as f64) / 1000.0;
            price_open_diff += pre_diff_base;
            let close = ((price_open_diff + price_close_diff) as f64) / 1000.0;
            let high = ((price_open_diff + price_high_diff) as f64) / 1000.0;
            let low = ((price_open_diff + price_low_diff) as f64) / 1000.0;

            pre_diff_base = price_open_diff + price_close_diff;

            let dt_str = format!("{}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, minute);

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("datetime", dt_str)?;
            dict.set_item("open", open)?;
            dict.set_item("high", high)?;
            dict.set_item("low", low)?;
            dict.set_item("close", close)?;
            dict.set_item("vol", vol)?;
            dict.set_item("amount", amount)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    pub fn get_report_file(
        &mut self,
        filename: &str,
        offset: u32,
    ) -> PyResult<(u32, std::borrow::Cow<'_, [u8]>)> {
        let stream = match self.stream.as_mut() {
            Some(s) => s,
            None => {
                return Err(pyo3::exceptions::PyConnectionError::new_err(
                    "Not connected",
                ))
            }
        };

        // Build report file request packet
        let mut req = Vec::with_capacity(120);
        req.extend_from_slice(&[0x0C, 0x12, 0x34, 0x00, 0x00, 0x00]);
        let raw_data_len: u16 = 110;
        req.extend_from_slice(&raw_data_len.to_le_bytes());
        req.extend_from_slice(&raw_data_len.to_le_bytes());

        req.extend_from_slice(&0x06B9_u16.to_le_bytes()); // command: get report file
        req.extend_from_slice(&offset.to_le_bytes());
        req.extend_from_slice(&0x7530_u32.to_le_bytes()); // 30000 max chunk size

        // Filename: 100 bytes, zero-padded
        let mut fname_buf = [0u8; 100];
        let bytes = filename.as_bytes();
        let len = std::cmp::min(100, bytes.len());
        fname_buf[..len].copy_from_slice(&bytes[..len]);
        req.extend_from_slice(&fname_buf);

        stream.write_all(&req)?;

        // Read response header
        let mut header = [0u8; RESP_HEADER_LEN];
        stream.read_exact(&mut header)?;

        let zip_size =
            u16::from_le_bytes(header[12..14].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed response header")
            })?);
        let unzip_size =
            u16::from_le_bytes(header[14..16].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed response header")
            })?);

        let mut body = vec![0u8; zip_size as usize];
        if zip_size > 0 {
            stream.read_exact(&mut body)?;
        }

        // Decompress if needed
        let data = if zip_size != unzip_size {
            let mut decoder = ZlibDecoder::new(&body[..]);
            let mut unzipped = vec![0u8; unzip_size as usize];
            decoder.read_exact(&mut unzipped)?;
            unzipped
        } else {
            body
        };

        if data.len() < 4 {
            return Ok((0, std::borrow::Cow::Borrowed(&[])));
        }

        let chunk_size =
            u32::from_le_bytes(data[0..4].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed chunk header")
            })?);
        Ok((chunk_size, std::borrow::Cow::Owned(data[4..].to_vec())))
    }
}
