use flate2::read::ZlibDecoder;
use pyo3::prelude::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::{get_datetime, get_price, get_time, get_volume};

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

// XDXR (除权除息) request command
const CMD_ID_XDXR: u16 = 0x000f;

// Transaction (分笔成交) request command
const CMD_ID_TRANSACTION: u16 = 0x0fc5;

// Security quotes (实时行情快照) command
const CMD_ID_QUOTES: u32 = 0x5053e;

// Company info category (F10 目录) command
const CMD_ID_F10_CATEGORY: u16 = 0x02cf;

// Company info content (F10 内容) command
const CMD_ID_F10_CONTENT: u16 = 0x02d0;

// Finance info (财务信息) command
const CMD_ID_FINANCE: u16 = 0x0010;

// Security count (获取证券数量) command
const CMD_ID_SECURITY_COUNT: u16 = 0x044e;

// Security list (获取证券列表) command
const CMD_ID_SECURITY_LIST: u16 = 0x0450;

/// Parse a little-endian u16 from a byte slice at the given offset.
fn parse_u16_le(data: &[u8], offset: usize, field: &str) -> PyResult<u16> {
    data[offset..offset + 2]
        .try_into()
        .map(u16::from_le_bytes)
        .map_err(|_| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Malformed {} data", field))
        })
}

/// Parse a little-endian u32 from a byte slice at the given offset.
fn parse_u32_le(data: &[u8], offset: usize, field: &str) -> PyResult<u32> {
    data[offset..offset + 4]
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Malformed {} data", field))
        })
}

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
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Build request packet
        let mut req = Vec::with_capacity(38);
        req.extend_from_slice(&0x10c_u16.to_le_bytes());
        req.extend_from_slice(&0x01016408_u32.to_le_bytes());
        req.extend_from_slice(&0x1c_u16.to_le_bytes()); // data length: 28 bytes
        req.extend_from_slice(&0x1c_u16.to_le_bytes()); // data length (dup)
        req.extend_from_slice(&CMD_ID_BARS.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);

        req.extend_from_slice(&category.to_le_bytes());
        req.extend_from_slice(&1u16.to_le_bytes()); // unknown flag
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&count.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved
        req.extend_from_slice(&0u16.to_le_bytes()); // reserved

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

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
        let stream = self.require_stream()?;

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
        let data = Self::read_response(stream)?;

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

// ---------------------------------------------------------------
//  Private (non-PyO3) helpers on TdxClient
// ---------------------------------------------------------------
impl TdxClient {
    fn read_response(stream: &mut TcpStream) -> PyResult<Vec<u8>> {
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

        if zip_size != unzip_size {
            let mut decoder = ZlibDecoder::new(&body[..]);
            let mut unzipped = vec![0u8; unzip_size as usize];
            decoder.read_exact(&mut unzipped)?;
            Ok(unzipped)
        } else {
            Ok(body)
        }
    }

    // ---------------------------------------------------------------
    //  Helper: get connected stream or return PyConnectionError
    // ---------------------------------------------------------------
    fn require_stream(&mut self) -> PyResult<&mut TcpStream> {
        self.stream
            .as_mut()
            .ok_or_else(|| pyo3::exceptions::PyConnectionError::new_err("Not connected"))
    }

    // ---------------------------------------------------------------
    //  Helper: build 6-byte zero-padded stock code
    // ---------------------------------------------------------------
    fn code_bytes(code: &str) -> [u8; 6] {
        let mut buf = [0u8; 6];
        let cd = code.as_bytes();
        let len = std::cmp::min(6, cd.len());
        buf[..len].copy_from_slice(&cd[..len]);
        buf
    }
}

// ===============================================================
//  New protocol methods exposed to Python
// ===============================================================
#[pymethods]
impl TdxClient {
    // ===============================================================
    //  get_xdxr_info  —  除权除息信息
    // ===============================================================
    pub fn get_xdxr_info(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 1f 18 76 00 01 0b 00 0b 00 0f 00 01 00 <B6s>
        let mut req: Vec<u8> = vec![0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00];
        req.extend_from_slice(&CMD_ID_XDXR.to_le_bytes());
        req.extend_from_slice(&1u16.to_le_bytes()); // count = 1
        req.push(market);
        req.extend_from_slice(&code_buf);

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 11 {
            return Ok(vec![]);
        }

        let mut pos: usize = 9; // skip 9 preamble bytes
        if pos + 2 > data.len() {
            return Ok(vec![]);
        }
        let num = u16::from_le_bytes(
            data[pos..pos + 2]
                .try_into()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Malformed xdxr count"))?,
        );
        pos += 2;

        let mut results = Vec::with_capacity(num as usize);

        for _ in 0..num {
            // skip market(1) + code(6) = 7  +  1 reserved byte
            if pos + 8 > data.len() {
                break;
            }
            pos += 8;

            // datetime: category 9 means date-only
            let (year, month, day, _hour, _minute, new_pos) = match get_datetime(9, &data, pos) {
                Some(v) => v,
                None => break,
            };
            pos = new_pos;

            if pos >= data.len() {
                break;
            }
            let category = data[pos];
            pos += 1;

            // 16 bytes of category-specific floats / ints
            if pos + 16 > data.len() {
                break;
            }

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("year", year)?;
            dict.set_item("month", month)?;
            dict.set_item("day", day)?;
            dict.set_item("category", category)?;

            let cat_name = match category {
                1 => "除权除息",
                2 => "送配股上市",
                3 => "非流通股上市",
                4 => "未知股本变动",
                5 => "股本变化",
                6 => "增发新股",
                7 => "股份回购",
                8 => "增发新股上市",
                9 => "转配股上市",
                10 => "可转债上市",
                11 => "扩缩股",
                12 => "非流通股缩股",
                13 => "送认购权证",
                14 => "送认沽权证",
                _ => "未知",
            };
            dict.set_item("name", cat_name)?;

            if category == 1 {
                // fenhong, peigujia, songzhuangu, peigu  (4 x f32)
                let fenhong = f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
                let peigujia =
                    f32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap_or([0; 4]));
                let songzhuangu =
                    f32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap_or([0; 4]));
                let peigu =
                    f32::from_le_bytes(data[pos + 12..pos + 16].try_into().unwrap_or([0; 4]));
                dict.set_item("fenhong", fenhong)?;
                dict.set_item("peigujia", peigujia)?;
                dict.set_item("songzhuangu", songzhuangu)?;
                dict.set_item("peigu", peigu)?;
            } else if category == 11 || category == 12 {
                // _, _, suogu, _  (u32, u32, f32, u32)
                let suogu =
                    f32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap_or([0; 4]));
                dict.set_item("suogu", suogu)?;
            } else if category == 13 || category == 14 {
                // xingquanjia, _, fenshu, _  (f32, u32, f32, u32)
                let xingquanjia =
                    f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
                let fenshu =
                    f32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap_or([0; 4]));
                dict.set_item("xingquanjia", xingquanjia)?;
                dict.set_item("fenshu", fenshu)?;
            } else {
                // panqianliutong, qianzongguben, panhouliutong, houzongguben (4 x u32 -> get_volume)
                let raw0 = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
                let raw1 = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap_or([0; 4]));
                let raw2 = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap_or([0; 4]));
                let raw3 =
                    u32::from_le_bytes(data[pos + 12..pos + 16].try_into().unwrap_or([0; 4]));
                let v = |r: u32| if r == 0 { 0.0 } else { get_volume(r) };
                dict.set_item("panqianliutong", v(raw0))?;
                dict.set_item("qianzongguben", v(raw1))?;
                dict.set_item("panhouliutong", v(raw2))?;
                dict.set_item("houzongguben", v(raw3))?;
            }
            pos += 16;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_transaction_data  —  分笔成交
    // ===============================================================
    pub fn get_transaction_data(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 17 08 01 01 01 0e 00 0e 00 c5 0f <H6sHH>
        let mut req: Vec<u8> = vec![0x0c, 0x17, 0x08, 0x01, 0x01, 0x01, 0x0e, 0x00, 0x0e, 0x00];
        req.extend_from_slice(&CMD_ID_TRANSACTION.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&count.to_le_bytes());

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let num = u16::from_le_bytes(
            data[0..2]
                .try_into()
                .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("Malformed tick count"))?,
        );
        let mut pos: usize = 2;
        let mut last_price: i64 = 0;
        let mut results = Vec::with_capacity(num as usize);

        for _ in 0..num {
            let (hour, minute, new_pos) = match get_time(&data, pos) {
                Some(v) => v,
                None => break,
            };
            pos = new_pos;

            let (price_raw, p1) = get_price(&data, pos);
            pos = p1;
            let (vol, p2) = get_price(&data, pos);
            pos = p2;
            let (num_trades, p3) = get_price(&data, pos);
            pos = p3;
            let (buyorsell, p4) = get_price(&data, pos);
            pos = p4;
            let (_reserved, p5) = get_price(&data, pos);
            pos = p5;

            last_price += price_raw;

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("time", format!("{:02}:{:02}", hour, minute))?;
            dict.set_item("price", (last_price as f64) / 100.0)?;
            dict.set_item("vol", vol)?;
            dict.set_item("num", num_trades)?;
            dict.set_item("buyorsell", buyorsell)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_security_quotes  —  实时行情快照 (五档盘口)
    // ===============================================================
    pub fn get_security_quotes(
        &mut self,
        py: Python<'_>,
        stock_list: Vec<(u8, String)>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let stock_len = stock_list.len();
        if stock_len == 0 {
            return Ok(vec![]);
        }

        let pkg_data_len = (stock_len * 7 + 12) as u16;

        let mut req = Vec::with_capacity(20 + stock_len * 7);
        req.extend_from_slice(&0x10c_u16.to_le_bytes());
        req.extend_from_slice(&0x02006320_u32.to_le_bytes());
        req.extend_from_slice(&pkg_data_len.to_le_bytes());
        req.extend_from_slice(&pkg_data_len.to_le_bytes());
        req.extend_from_slice(&CMD_ID_QUOTES.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes());
        req.extend_from_slice(&0u16.to_le_bytes());
        req.extend_from_slice(&(stock_len as u16).to_le_bytes());

        for (market, code) in &stock_list {
            req.push(*market);
            let cb = Self::code_bytes(code);
            req.extend_from_slice(&cb);
        }

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 4 {
            return Ok(vec![]);
        }

        // skip 2 bytes (b1 cb), then read count
        let mut pos: usize = 2;
        let num_stock =
            u16::from_le_bytes(data[pos..pos + 2].try_into().map_err(|_| {
                pyo3::exceptions::PyRuntimeError::new_err("Malformed quotes count")
            })?);
        pos += 2;

        let mut results = Vec::with_capacity(num_stock as usize);

        for _ in 0..num_stock {
            if pos + 9 > data.len() {
                break;
            }
            let market = data[pos];
            let code_raw = &data[pos + 1..pos + 7];
            let code_str = std::str::from_utf8(code_raw)
                .unwrap_or("")
                .trim_end_matches('\0');
            // active1 = u16 at pos+7..pos+9, skip
            pos += 9;

            let (price, p1) = get_price(&data, pos);
            pos = p1;
            let (last_close_diff, p2) = get_price(&data, pos);
            pos = p2;
            let (open_diff, p3) = get_price(&data, pos);
            pos = p3;
            let (high_diff, p4) = get_price(&data, pos);
            pos = p4;
            let (low_diff, p5) = get_price(&data, pos);
            pos = p5;

            // server time (get_price), reversed_bytes1 (get_price)
            let (server_time_raw, p6) = get_price(&data, pos);
            pos = p6;
            let (_rb1, p7) = get_price(&data, pos);
            pos = p7;

            // vol, cur_vol
            let (vol, p8) = get_price(&data, pos);
            pos = p8;
            let (cur_vol, p9) = get_price(&data, pos);
            pos = p9;

            // amount (u32 -> get_volume)
            if pos + 4 > data.len() {
                break;
            }
            let amount_raw = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
            let amount = get_volume(amount_raw);
            pos += 4;

            // s_vol, b_vol, rb2, rb3
            let (s_vol, pa) = get_price(&data, pos);
            pos = pa;
            let (b_vol, pb) = get_price(&data, pos);
            pos = pb;
            let (_rb2, pc) = get_price(&data, pos);
            pos = pc;
            let (_rb3, pd) = get_price(&data, pos);
            pos = pd;

            let cal = |base: i64, diff: i64| -> f64 { (base + diff) as f64 / 100.0 };

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("market", market)?;
            dict.set_item("code", code_str)?;
            dict.set_item("price", cal(price, 0))?;
            dict.set_item("last_close", cal(price, last_close_diff))?;
            dict.set_item("open", cal(price, open_diff))?;
            dict.set_item("high", cal(price, high_diff))?;
            dict.set_item("low", cal(price, low_diff))?;
            dict.set_item("servertime", server_time_raw)?;
            dict.set_item("vol", vol)?;
            dict.set_item("cur_vol", cur_vol)?;
            dict.set_item("amount", amount)?;
            dict.set_item("s_vol", s_vol)?;
            dict.set_item("b_vol", b_vol)?;

            // 5-level bid/ask
            for level in 1..=5u8 {
                let (bid, pe) = get_price(&data, pos);
                pos = pe;
                let (ask, pf) = get_price(&data, pos);
                pos = pf;
                let (bid_vol, pg) = get_price(&data, pos);
                pos = pg;
                let (ask_vol, ph) = get_price(&data, pos);
                pos = ph;

                dict.set_item(format!("bid{}", level), cal(price, bid))?;
                dict.set_item(format!("ask{}", level), cal(price, ask))?;
                dict.set_item(format!("bid_vol{}", level), bid_vol)?;
                dict.set_item(format!("ask_vol{}", level), ask_vol)?;
            }

            // trailing bytes: u16 + 4*get_price + (i16 + u16)
            if pos >= data.len() {
                break;
            }
            if pos + 2 <= data.len() {
                pos += 2;
            }
            for _ in 0..4 {
                let (_, pn) = get_price(&data, pos);
                pos = pn;
            }
            if pos + 4 <= data.len() {
                pos += 4;
            }

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_company_info_category  —  F10 公司资料目录
    // ===============================================================
    pub fn get_company_info_category(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 0f 10 9b 00 01 0e 00 0e 00 cf 02 <H6sI>
        let mut req: Vec<u8> = vec![0x0c, 0x0f, 0x10, 0x9b, 0x00, 0x01, 0x0e, 0x00, 0x0e, 0x00];
        req.extend_from_slice(&CMD_ID_F10_CATEGORY.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);
        req.extend_from_slice(&0u32.to_le_bytes());

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let num = u16::from_le_bytes(data[0..2].try_into().map_err(|_| {
            pyo3::exceptions::PyRuntimeError::new_err("Malformed F10 category count")
        })?);
        let mut pos: usize = 2;
        let mut results = Vec::with_capacity(num as usize);

        for _ in 0..num {
            if pos + 152 > data.len() {
                break;
            }
            // name: 64 bytes (GBK), filename: 80 bytes (ASCII), start: u32, length: u32
            let name_raw = &data[pos..pos + 64];
            let filename_raw = &data[pos + 64..pos + 144];
            let start_offset =
                u32::from_le_bytes(data[pos + 144..pos + 148].try_into().unwrap_or([0; 4]));
            let length =
                u32::from_le_bytes(data[pos + 148..pos + 152].try_into().unwrap_or([0; 4]));
            pos += 152;

            // Decode GBK name (trim nulls)
            let null_pos_n = name_raw.iter().position(|&b| b == 0).unwrap_or(64);
            let name = decode_gbk(&name_raw[..null_pos_n]);

            let null_pos_f = filename_raw.iter().position(|&b| b == 0).unwrap_or(80);
            let filename = String::from_utf8_lossy(&filename_raw[..null_pos_f]).to_string();

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("name", name)?;
            dict.set_item("filename", filename)?;
            dict.set_item("start", start_offset)?;
            dict.set_item("length", length)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_company_info_content  —  F10 公司资料内容
    // ===============================================================
    pub fn get_company_info_content(
        &mut self,
        market: u8,
        code: &str,
        filename: &str,
        start: u32,
        length: u32,
    ) -> PyResult<String> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // filename: 80 bytes zero-padded
        let mut fname_buf = [0u8; 80];
        let fb = filename.as_bytes();
        let flen = std::cmp::min(80, fb.len());
        fname_buf[..flen].copy_from_slice(&fb[..flen]);

        // Packet: 0c 07 10 9c 00 01 68 00 68 00 d0 02 <H6sH80sIII>
        let mut req: Vec<u8> = vec![0x0c, 0x07, 0x10, 0x9c, 0x00, 0x01, 0x68, 0x00, 0x68, 0x00];
        req.extend_from_slice(&CMD_ID_F10_CONTENT.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);
        req.extend_from_slice(&0u16.to_le_bytes()); // padding
        req.extend_from_slice(&fname_buf);
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&length.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 12 {
            return Ok(String::new());
        }

        // skip 10 bytes, then read content_length u16
        // Note: TDX protocol limits F10 content to 64KB per request (u16 wire field).
        let content_length = u16::from_le_bytes(data[10..12].try_into().map_err(|_| {
            pyo3::exceptions::PyRuntimeError::new_err("Malformed F10 content header")
        })?) as usize;
        let pos = 12;

        if pos + content_length > data.len() {
            // return what we can
            let content = &data[pos..];
            return Ok(decode_gbk(content));
        }

        let content = &data[pos..pos + content_length];
        Ok(decode_gbk(content))
    }

    // ===============================================================
    //  get_finance_info  —  财务信息摘要
    // ===============================================================
    pub fn get_finance_info(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 1f 18 76 00 01 0b 00 0b 00 10 00 01 00 <B6s>
        let mut req: Vec<u8> = vec![0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00];
        req.extend_from_slice(&CMD_ID_FINANCE.to_le_bytes());
        req.extend_from_slice(&1u16.to_le_bytes()); // count = 1
        req.push(market);
        req.extend_from_slice(&code_buf);

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        // Response: 2 bytes count + 7 bytes (market+code) + 136 bytes struct
        // struct: <fHHIIffffffffffffffffffffffffffffff>
        //   f32 + u16 + u16 + u32 + u32 + 29×f32 = 4+2+2+4+4+116 = 132... actually
        //   1f + 2H + 2I + 29f = 4 + 4 + 8 + 116 = 132 bytes
        let header_len = 2 + 7; // count(2) + market(1) + code(6)
        if data.len() < header_len + 132 {
            let dict = pyo3::types::PyDict::new(py);
            return Ok(dict.into_any().unbind());
        }

        let pos = header_len;
        let b = &data[pos..];

        // Helper to read f32 at offset
        let f = |off: usize| -> f64 {
            f32::from_le_bytes(b[off..off + 4].try_into().unwrap_or([0; 4])) as f64
        };
        let h = |off: usize| -> u32 {
            u16::from_le_bytes(b[off..off + 2].try_into().unwrap_or([0; 2])) as u32
        };
        let i = |off: usize| -> u32 {
            u32::from_le_bytes(b[off..off + 4].try_into().unwrap_or([0; 4]))
        };

        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("market", market)?;
        dict.set_item("code", code)?;

        // f32 liutongguben (offset 0)
        dict.set_item("liutongguben", f(0) * 10000.0)?;
        // u16 province (offset 4)
        dict.set_item("province", h(4))?;
        // u16 industry (offset 6)
        dict.set_item("industry", h(6))?;
        // u32 updated_date (offset 8)
        dict.set_item("updated_date", i(8))?;
        // u32 ipo_date (offset 12)
        dict.set_item("ipo_date", i(12))?;
        // 29 x f32 starting at offset 16
        let fields = [
            "zongguben",
            "guojiagu",
            "faqirenfarengu",
            "farengu",
            "bgu",
            "hgu",
            "zhigonggu",
            "zongzichan",
            "liudongzichan",
            "gudingzichan",
            "wuxingzichan",
            "gudongrenshu",
            "liudongfuzhai",
            "changqifuzhai",
            "zibengongjijin",
            "jingzichan",
            "zhuyingshouru",
            "zhuyinglirun",
            "yingshouzhangkuan",
            "yingyelirun",
            "touzishouyu",
            "jingyingxianjinliu",
            "zongxianjinliu",
            "cunhuo",
            "lirunzonghe",
            "shuihoulirun",
            "jinglirun",
            "weifenpeilirun",
            "meigujingzichan",
        ];
        for (idx, name) in fields.iter().enumerate() {
            let off = 16 + idx * 4;
            let val = f(off);
            // gudongrenshu and meigujingzichan are not scaled
            if *name == "gudongrenshu" || *name == "meigujingzichan" {
                dict.set_item(*name, val)?;
            } else {
                dict.set_item(*name, val * 10000.0)?;
            }
        }

        Ok(dict.into_any().unbind())
    }

    /// Get the total number of securities for a given market.
    /// market: 0 = Shenzhen (深圳), 1 = Shanghai (上海)
    /// Returns the total count of securities.
    pub fn get_security_count(&mut self, market: u8) -> PyResult<u16> {
        let stream = self.require_stream()?;

        // Build request: 0c 0c 18 6c 00 01 08 00 08 00 4e 04 <market_u16> 75 c7 33 01
        let mut req = Vec::with_capacity(18);
        req.extend_from_slice(&[0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00]);
        req.extend_from_slice(&CMD_ID_SECURITY_COUNT.to_le_bytes()); // 4e 04
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 2 {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Malformed security count response: expected at least 2 bytes",
            ));
        }

        let count = parse_u16_le(&data, 0, "count")?;
        Ok(count)
    }

    /// Get a batch of securities for a given market, starting from `start`.
    /// market: 0 = Shenzhen (深圳), 1 = Shanghai (上海)
    /// start: offset (typically 0, 1000, 2000, ...)
    /// Returns a list of dicts with keys: code, name, volunit, decimal_point, pre_close.
    pub fn get_security_list(
        &mut self,
        py: Python<'_>,
        market: u8,
        start: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;

        // Build request: 0c 01 18 64 01 01 06 00 06 00 50 04 <market_u16> <start_u16>
        let mut req = Vec::with_capacity(16);
        req.extend_from_slice(&[0x0c, 0x01, 0x18, 0x64, 0x01, 0x01, 0x06, 0x00, 0x06, 0x00]);
        req.extend_from_slice(&CMD_ID_SECURITY_LIST.to_le_bytes()); // 50 04
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&start.to_le_bytes());

        stream.write_all(&req)?;
        let data = Self::read_response(stream)?;

        if data.len() < 2 {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Malformed security list response: expected at least 2 bytes",
            ));
        }

        let num = parse_u16_le(&data, 0, "list header")? as usize;

        let mut pos = 2;
        let mut results = Vec::with_capacity(num);

        for _ in 0..num {
            // Each record is 29 bytes:
            //   6s  code (ASCII)          offset 0..6
            //   H   volunit               offset 6..8
            //   8s  name (GBK encoded)    offset 8..16
            //   4s  reserved1 (ignored)   offset 16..20
            //   B   decimal_point         offset 20
            //   I   pre_close_raw         offset 21..25
            //   4s  reserved2 (ignored)   offset 25..29
            if pos + 29 > data.len() {
                break;
            }

            let record = &data[pos..pos + 29];
            pos += 29;

            // code: 6 bytes ASCII, strip null bytes
            let code_bytes = &record[0..6];
            let code = String::from_utf8_lossy(code_bytes)
                .trim_end_matches('\0')
                .to_string();

            // volunit: u16 LE at offset 6
            let volunit = parse_u16_le(record, 6, "volunit")?;

            // name: 8 bytes GBK at offset 8, strip null bytes
            let name_bytes = &record[8..16];
            let name = decode_gbk(name_bytes)
                .trim_end_matches('\0')
                .to_string();

            // reserved1: 4 bytes at offset 16 — TDX protocol reserved field, pytdx ignores it
            // decimal_point: u8 at offset 20
            let decimal_point = record[20];

            // pre_close_raw: u32 LE at offset 21
            let pre_close_raw = parse_u32_le(record, 21, "pre_close")?;
            let pre_close = get_volume(pre_close_raw);

            // reserved2: 4 bytes at offset 25 — TDX protocol reserved field, pytdx ignores it

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("code", code)?;
            dict.set_item("volunit", volunit)?;
            dict.set_item("name", name)?;
            dict.set_item("decimal_point", decimal_point)?;
            dict.set_item("pre_close", pre_close)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }
}

/// Decode a GBK-encoded byte slice to a String.
/// Falls back to lossy UTF-8 if GBK decoding fails.
fn decode_gbk(bytes: &[u8]) -> String {
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}
