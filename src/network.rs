use flate2::read::ZlibDecoder;
use pyo3::prelude::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::{get_datetime, get_price, get_price_checked, get_time, get_volume};
use std::net::ToSocketAddrs;

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

// Minute time data (当日分时数据) command
const CMD_ID_MINUTE_TIME: u16 = 0x051d;

// History minute time data (历史分时数据) command
const CMD_ID_HISTORY_MINUTE_TIME: u16 = 0x0fb4;

// History transaction data (历史分笔成交) command
const CMD_ID_HISTORY_TRANSACTION: u16 = 0x0fb5;

// Block info meta (板块元数据) command
const CMD_ID_BLOCK_META: u16 = 0x02c5;

// Block info data (板块详情) command
const CMD_ID_BLOCK_DATA: u16 = 0x06b9;

/// Read one TDX varint field inside a record loop, breaking the loop (via the
/// label passed as first argument) when the buffer is exhausted. `get_price`
/// silently returns `(0, pos)` at end-of-buffer, which would emit zero-filled
/// garbage records on a truncated response; `get_price_checked` returns `None`
/// there instead. The label must be passed explicitly because macro hygiene
/// hides call-site labels from the macro body. (F9)
macro_rules! read_price {
    ($label:lifetime, $data:expr, $pos:expr) => {
        match get_price_checked($data, $pos) {
            Some(v) => v,
            None => break $label,
        }
    };
}

/// Parse a little-endian u16 from a byte slice at the given offset.
/// Returns a `PyRuntimeError` (instead of panicking) when the slice is too short.
fn parse_u16_le(data: &[u8], offset: usize, field: &str) -> PyResult<u16> {
    offset
        .checked_add(2)
        .and_then(|end| data.get(offset..end))
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Malformed {} data", field))
        })
}

/// Parse a little-endian u32 from a byte slice at the given offset.
/// Returns a `PyRuntimeError` (instead of panicking) when the slice is too short.
fn parse_u32_le(data: &[u8], offset: usize, field: &str) -> PyResult<u32> {
    offset
        .checked_add(4)
        .and_then(|end| data.get(offset..end))
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Malformed {} data", field))
        })
}

/// Compute the quotes request payload length (`stock_len * 7 + 12`) without
/// silent u16 truncation. Returns `PyValueError` when it cannot fit the wire
/// field (or when `stock_len` itself exceeds u16). (F6-rust)
fn quotes_packet_len(stock_len: usize) -> PyResult<u16> {
    let len = stock_len
        .checked_mul(7)
        .and_then(|v| v.checked_add(12))
        .ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Too many stocks in one quotes request: {} (length overflow)",
                stock_len
            ))
        })?;
    if len > u16::MAX as usize {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Too many stocks in one quotes request: {} (packet payload {} bytes exceeds u16 max {})",
            stock_len, len, u16::MAX
        )));
    }
    if stock_len > u16::MAX as usize {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Too many stocks in one quotes request: {} (exceeds u16 max {})",
            stock_len, u16::MAX
        )));
    }
    Ok(len as u16)
}

/// Resolve an address string ("host:port", IP literal or hostname) into
/// candidate socket addresses.
/// NOTE: DNS resolution itself is blocking; callers must run this off the GIL
/// (inside `py.detach`, pyo3 0.27's renamed `allow_threads`). (F22)
fn resolve_addr(addr: &str) -> PyResult<Vec<std::net::SocketAddr>> {
    let addrs: Vec<std::net::SocketAddr> = addr
        .to_socket_addrs()
        .map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid address '{}': {}", addr, e))
        })?
        .collect();
    if addrs.is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "No addresses resolved for '{}'",
            addr
        )));
    }
    Ok(addrs)
}

/// Drain a setup response: read the full 16-byte header, then read+discard the
/// body. Errors are PROPAGATED: a broken handshake must fail `connect` cleanly.
/// Using a single `read()` for the header could leave the body in the socket on
/// TCP fragmentation, desyncing every later response. (F10)
fn drain_setup_response(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut header = [0u8; RESP_HEADER_LEN];
    stream.read_exact(&mut header)?;
    let zip_size = u16::from_le_bytes([header[12], header[13]]);
    if zip_size > 0 {
        let mut buf = vec![0u8; zip_size as usize];
        stream.read_exact(&mut buf)?;
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

    pub fn connect(&mut self, py: Python<'_>, ip: &str, port: u16) -> PyResult<bool> {
        let addr = format!("{}:{}", ip, port);
        // F13: DNS resolution, TCP connect and the handshake are all blocking;
        // run them with the GIL released so other Python threads keep running.
        let stream = py.detach(|| Self::connect_and_handshake(&addr))?;
        self.stream = Some(stream);
        Ok(true)
    }

    pub fn disconnect(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(stream) = self.stream.take() {
            py.detach(|| {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            });
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
        let req = Self::build_bars_request(category, market, code, start, count);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

            let (price_open_diff, price_close_diff, price_high_diff, price_low_diff, new_pos) =
                Self::read_bar_diffs(&data, pos);
            pos = new_pos;

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

            let (open, high, low, close, new_base) = Self::bar_prices(
                price_open_diff,
                price_close_diff,
                price_high_diff,
                price_low_diff,
                pre_diff_base,
            );
            pre_diff_base = new_base;

            let dt_str = format!("{}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, minute);
            let dict = Self::bar_dict(py, &dt_str, (open, high, low, close), vol, amount)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    pub fn get_report_file(
        &mut self,
        py: Python<'_>,
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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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
    /// Blocking connect + handshake. Runs entirely off the GIL (see `connect`).
    /// Resolves the address (IP literal or hostname), tries each resolved
    /// SocketAddr with a connect timeout, enables TCP_NODELAY, sets socket
    /// timeouts, then performs the 3-step setup handshake. (F22, F10, F13)
    fn connect_and_handshake(addr: &str) -> PyResult<TcpStream> {
        let addrs = resolve_addr(addr)?;
        let mut last_err: Option<std::io::Error> = None;
        let mut connected: Option<TcpStream> = None;
        for sock_addr in addrs {
            match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(CONNECT_TIMEOUT)) {
                Ok(s) => {
                    connected = Some(s);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let mut stream = connected.ok_or_else(|| {
            pyo3::exceptions::PyConnectionError::new_err(format!(
                "Connect failed: {}",
                last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "no address to connect".to_string())
            ))
        })?;

        // F22: disable Nagle so small request packets are sent without delay.
        stream.set_nodelay(true).map_err(|e| {
            pyo3::exceptions::PyOSError::new_err(format!("Failed to set TCP_NODELAY: {}", e))
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT)))
            .map_err(|e| {
                pyo3::exceptions::PyOSError::new_err(format!("Failed to set read timeout: {}", e))
            })?;
        stream
            .set_write_timeout(Some(Duration::from_secs(WRITE_TIMEOUT)))
            .map_err(|e| {
                pyo3::exceptions::PyOSError::new_err(format!("Failed to set write timeout: {}", e))
            })?;

        // Setup handshake sequence. Errors propagate so a broken handshake fails
        // connect cleanly instead of leaving the stream desynced. (F10)
        stream.write_all(&SETUP_CMD1)?;
        drain_setup_response(&mut stream)?;

        stream.write_all(&SETUP_CMD2)?;
        drain_setup_response(&mut stream)?;

        stream.write_all(&SETUP_CMD3)?;
        drain_setup_response(&mut stream)?;

        Ok(stream)
    }

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

    // ---------------------------------------------------------------
    //  Shared protocol helpers (extracted to remove duplicated code
    //  between the security/index bars, minute and tick methods)
    // ---------------------------------------------------------------

    /// Build the K-line request packet shared by `get_security_bars` and
    /// `get_index_bars`: both use command 0x052D with the identical layout.
    /// Byte-for-byte the same as the previous inline construction.
    fn build_bars_request(category: u16, market: u8, code: &str, start: u16, count: u16) -> Vec<u8> {
        let code_buf = Self::code_bytes(code);
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
        req
    }

    /// Read the four cumulative price diffs (open/close/high/low) of a K-line
    /// record, shared by security and index bars. Uses unchecked `get_price`
    /// exactly like the original loops: at end-of-buffer it yields `(0, pos)`
    /// and the caller's vol/amount boundary check drops the partial record.
    fn read_bar_diffs(data: &[u8], pos: usize) -> (i64, i64, i64, i64, usize) {
        let (open_diff, p1) = get_price(data, pos);
        let (close_diff, p2) = get_price(data, p1);
        let (high_diff, p3) = get_price(data, p2);
        let (low_diff, p4) = get_price(data, p3);
        (open_diff, close_diff, high_diff, low_diff, p4)
    }

    /// Reconstruct absolute OHLC prices from a record's diffs and the previous
    /// record's close base (`pre_diff_base`). Pure arithmetic shared by
    /// security and index bars: `wrapping_add` for the base updates, /1000.0
    /// scaling. Returns (open, high, low, close, next base).
    fn bar_prices(
        open_diff: i64,
        close_diff: i64,
        high_diff: i64,
        low_diff: i64,
        base: i64,
    ) -> (f64, f64, f64, f64, i64) {
        let open = ((open_diff + base) as f64) / 1000.0;
        let open_abs = open_diff.wrapping_add(base);
        let close = ((open_abs + close_diff) as f64) / 1000.0;
        let high = ((open_abs + high_diff) as f64) / 1000.0;
        let low = ((open_abs + low_diff) as f64) / 1000.0;
        let new_base = open_abs.wrapping_add(close_diff);
        (open, high, low, close, new_base)
    }

    /// Build the datetime/OHLC/vol/amount dict shared by security and index
    /// bar records. `prices` is (open, high, low, close) as returned by
    /// `bar_prices`. Returns the `Bound` dict (not `Py<PyAny>`) so index bars
    /// can append their up_count/down_count fields afterwards.
    fn bar_dict<'py>(
        py: Python<'py>,
        dt_str: &str,
        prices: (f64, f64, f64, f64),
        vol: f64,
        amount: f64,
    ) -> PyResult<Bound<'py, pyo3::types::PyDict>> {
        let (open, high, low, close) = prices;
        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("datetime", dt_str)?;
        dict.set_item("open", open)?;
        dict.set_item("high", high)?;
        dict.set_item("low", low)?;
        dict.set_item("close", close)?;
        dict.set_item("vol", vol)?;
        dict.set_item("amount", amount)?;
        Ok(dict)
    }

    /// Parse the minute-record array shared by `get_minute_time_data` and
    /// `get_history_minute_time_data`: both responses carry identical
    /// price/reserved/vol varint records with cumulative price diffs.
    /// `pos` is the first record offset (4 for today, 6 for history).
    fn parse_minute_records(
        py: Python<'_>,
        data: &[u8],
        mut pos: usize,
        num: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let mut last_price: i64 = 0;
        let mut results = Vec::with_capacity(num as usize);

        'record: for _ in 0..num {
            if pos >= data.len() {
                break;
            }

            let (price_raw, p1) = read_price!('record, &data, pos);
            pos = p1;
            let (_reserved, p2) = read_price!('record, &data, pos);
            pos = p2;
            let (vol, p3) = read_price!('record, &data, pos);
            pos = p3;

            // F9: truncated record — stop instead of emitting zero-filled data.
            if pos > data.len() {
                break;
            }

            last_price = last_price.wrapping_add(price_raw);

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("price", (last_price as f64) / 100.0)?;
            dict.set_item("vol", vol)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    /// Shared prefix of a tick (分笔) record: HH:MM time, cumulative price
    /// diff and volume. Used by both transaction loops; `None` on truncation
    /// mirrors `read_price!` breaking the caller's loop (get_price_checked).
    fn read_tick_prefix(data: &[u8], pos: usize) -> Option<(u32, u32, i64, i64, usize)> {
        let (hour, minute, p) = get_time(data, pos)?;
        let (price_raw, p1) = get_price_checked(data, p)?;
        let (vol, p2) = get_price_checked(data, p1)?;
        Some((hour, minute, price_raw, vol, p2))
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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

        'record: for _ in 0..num {
            let (hour, minute, price_raw, vol, new_pos) = match Self::read_tick_prefix(&data, pos) {
                Some(v) => v,
                None => break,
            };
            pos = new_pos;

            let (num_trades, p3) = read_price!('record, &data, pos);
            pos = p3;
            let (buyorsell, p4) = read_price!('record, &data, pos);
            pos = p4;
            let (_reserved, p5) = read_price!('record, &data, pos);
            pos = p5;

            // F9: a truncated continuation can push pos past the buffer end;
            // drop the incomplete record instead of emitting garbage.
            if pos > data.len() {
                break;
            }

            last_price = last_price.wrapping_add(price_raw);

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

        // F6-rust: compute in usize and reject (PyValueError) instead of silently
        // truncating the u16 wire fields. G1 batches at 80 stocks in Python.
        let pkg_data_len = quotes_packet_len(stock_len)?;

        let mut req = Vec::with_capacity(20 + stock_len * 7);
        req.extend_from_slice(&0x10c_u16.to_le_bytes());
        req.extend_from_slice(&0x02006320_u32.to_le_bytes());
        req.extend_from_slice(&pkg_data_len.to_le_bytes());
        req.extend_from_slice(&pkg_data_len.to_le_bytes());
        req.extend_from_slice(&CMD_ID_QUOTES.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes());
        req.extend_from_slice(&0u16.to_le_bytes());
        // Safe cast: quotes_packet_len guarantees stock_len <= 9360 < u16::MAX.
        req.extend_from_slice(&(stock_len as u16).to_le_bytes());

        for (market, code) in &stock_list {
            req.push(*market);
            let cb = Self::code_bytes(code);
            req.extend_from_slice(&cb);
        }

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

        'record: for _ in 0..num_stock {
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

            let (price, p1) = read_price!('record, &data, pos);
            pos = p1;
            let (last_close_diff, p2) = read_price!('record, &data, pos);
            pos = p2;
            let (open_diff, p3) = read_price!('record, &data, pos);
            pos = p3;
            let (high_diff, p4) = read_price!('record, &data, pos);
            pos = p4;
            let (low_diff, p5) = read_price!('record, &data, pos);
            pos = p5;

            // server time (get_price), reversed_bytes1 (get_price)
            let (server_time_raw, p6) = read_price!('record, &data, pos);
            pos = p6;
            let (_rb1, p7) = read_price!('record, &data, pos);
            pos = p7;

            // vol, cur_vol
            let (vol, p8) = read_price!('record, &data, pos);
            pos = p8;
            let (cur_vol, p9) = read_price!('record, &data, pos);
            pos = p9;

            // amount (u32 -> get_volume)
            if pos + 4 > data.len() {
                break;
            }
            let amount_raw = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
            let amount = get_volume(amount_raw);
            pos += 4;

            // s_vol, b_vol, rb2, rb3
            let (s_vol, pa) = read_price!('record, &data, pos);
            pos = pa;
            let (b_vol, pb) = read_price!('record, &data, pos);
            pos = pb;
            let (_rb2, pc) = read_price!('record, &data, pos);
            pos = pc;
            let (_rb3, pd) = read_price!('record, &data, pos);
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
                let (bid, pe) = read_price!('record, &data, pos);
                pos = pe;
                let (ask, pf) = read_price!('record, &data, pos);
                pos = pf;
                let (bid_vol, pg) = read_price!('record, &data, pos);
                pos = pg;
                let (ask_vol, ph) = read_price!('record, &data, pos);
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
                let (_, pn) = read_price!('record, &data, pos);
                pos = pn;
            }
            if pos + 4 <= data.len() {
                pos += 4;
            }

            // F9: never emit a record whose decode ran past the buffer end.
            if pos > data.len() {
                break;
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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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
        py: Python<'_>,
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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        // Response: 2 bytes count + 7 bytes (market+code) + 136 bytes struct.
        // struct (pytdx get_finance_info.py): "<fHHIIffffffffffffffffffffffffffffff"
        //   1f + 2H + 2I + 30f = 35 values = 4 + 2 + 2 + 4 + 4 + 120 = 136 bytes.
        // The last two floats are baoliu1 (exposed as meigujingzichan) and baoliu2;
        // pytdx returns both unscaled. (F14)
        let header_len = 2 + 7; // count(2) + market(1) + code(6)
        if data.len() < header_len + 136 {
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
            // gudongrenshu and meigujingzichan (= baoliu1) are not scaled
            if *name == "gudongrenshu" || *name == "meigujingzichan" {
                dict.set_item(*name, val)?;
            } else {
                dict.set_item(*name, val * 10000.0)?;
            }
        }
        // baoliu2 (offset 132, last of the 30 floats): pytdx returns it raw. (F14)
        dict.set_item("baoliu2", f(132))?;

        Ok(dict.into_any().unbind())
    }

    /// Get the total number of securities for a given market.
    /// market: 0 = Shenzhen (深圳), 1 = Shanghai (上海)
    /// Returns the total count of securities.
    pub fn get_security_count(&mut self, py: Python<'_>, market: u8) -> PyResult<u16> {
        let stream = self.require_stream()?;

        // Build request: 0c 0c 18 6c 00 01 08 00 08 00 4e 04 <market_u16> 75 c7 33 01
        let mut req = Vec::with_capacity(18);
        req.extend_from_slice(&[0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00]);
        req.extend_from_slice(&CMD_ID_SECURITY_COUNT.to_le_bytes()); // 4e 04
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

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

    // ===============================================================
    //  get_index_bars  —  指数K线 (含涨跌家数)
    // ===============================================================
    /// Get index K-line bars. Same protocol as security bars (0x052D) but response
    /// includes up_count/down_count fields (number of advancing/declining stocks).
    pub fn get_index_bars(
        &mut self,
        py: Python<'_>,
        category: u16,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        // Same packet structure as get_security_bars
        let req = Self::build_bars_request(category, market, code, start, count);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let ret_count = parse_u16_le(&data, 0, "index bar count")?;
        let mut pos = 2;
        let mut pre_diff_base: i64 = 0;

        let mut results = Vec::with_capacity(ret_count as usize);

        for _ in 0..ret_count {
            let (year, month, day, hour, minute, new_pos) =
                match get_datetime(category, &data, pos) {
                    Some(v) => v,
                    None => break,
                };
            pos = new_pos;

            let (price_open_diff, price_close_diff, price_high_diff, price_low_diff, new_pos) =
                Self::read_bar_diffs(&data, pos);
            pos = new_pos;

            // Need 12 more bytes: vol(4) + amount(4) + up_count(2) + down_count(2)
            if pos + 12 > data.len() {
                break;
            }

            let vol_raw = parse_u32_le(&data, pos, "volume")?;
            let vol = get_volume(vol_raw);
            pos += 4;

            let db_vol_raw = parse_u32_le(&data, pos, "amount")?;
            let amount = get_volume(db_vol_raw);
            pos += 4;

            // Index-specific: up_count and down_count
            let up_count = parse_u16_le(&data, pos, "up_count")?;
            pos += 2;
            let down_count = parse_u16_le(&data, pos, "down_count")?;
            pos += 2;

            let (open, high, low, close, new_base) = Self::bar_prices(
                price_open_diff,
                price_close_diff,
                price_high_diff,
                price_low_diff,
                pre_diff_base,
            );
            pre_diff_base = new_base;

            let dt_str = format!("{}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, minute);
            let dict = Self::bar_dict(py, &dt_str, (open, high, low, close), vol, amount)?;
            dict.set_item("up_count", up_count)?;
            dict.set_item("down_count", down_count)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_minute_time_data  —  当日分时数据
    // ===============================================================
    /// Get intraday minute-by-minute price/volume data for today.
    /// Returns list of dicts with keys: price, vol.
    pub fn get_minute_time_data(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 1b 08 00 01 01 0e 00 0e 00 1d 05 <H6sI>
        let mut req = Vec::with_capacity(24);
        req.extend_from_slice(&[0x0c, 0x1b, 0x08, 0x00, 0x01, 0x01, 0x0e, 0x00, 0x0e, 0x00]);
        req.extend_from_slice(&CMD_ID_MINUTE_TIME.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);
        req.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let num = parse_u16_le(&data, 0, "minute count")?;
        let pos = 4; // skip 2 bytes count + 2 bytes reserved
        Self::parse_minute_records(py, &data, pos, num)
    }

    // ===============================================================
    //  get_history_minute_time_data  —  历史分时数据
    // ===============================================================
    /// Get minute-by-minute price/volume data for a historical date.
    /// date: YYYYMMDD format integer, e.g. 20260101
    pub fn get_history_minute_time_data(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
        date: u32,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 01 30 00 01 01 0d 00 0d 00 b4 0f <I B 6s>
        let mut req = Vec::with_capacity(23);
        req.extend_from_slice(&[0x0c, 0x01, 0x30, 0x00, 0x01, 0x01, 0x0d, 0x00, 0x0d, 0x00]);
        req.extend_from_slice(&CMD_ID_HISTORY_MINUTE_TIME.to_le_bytes());
        req.extend_from_slice(&date.to_le_bytes());
        req.push(market);
        req.extend_from_slice(&code_buf);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let num = parse_u16_le(&data, 0, "history minute count")?;
        let pos = 6; // skip 2 bytes count + 4 bytes unknown (pytdx: pos += 6)
        Self::parse_minute_records(py, &data, pos, num)
    }

    // ===============================================================
    //  get_history_transaction_data  —  历史分笔成交
    // ===============================================================
    /// Get tick-level transaction data for a historical date.
    /// date: YYYYMMDD format integer, e.g. 20260101
    pub fn get_history_transaction_data(
        &mut self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
        date: u32,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let stream = self.require_stream()?;
        let code_buf = Self::code_bytes(code);

        // Packet: 0c 01 30 01 00 01 12 00 12 00 b5 0f <I H 6s H H>
        let mut req = Vec::with_capacity(28);
        req.extend_from_slice(&[0x0c, 0x01, 0x30, 0x01, 0x00, 0x01, 0x12, 0x00, 0x12, 0x00]);
        req.extend_from_slice(&CMD_ID_HISTORY_TRANSACTION.to_le_bytes());
        req.extend_from_slice(&date.to_le_bytes());
        req.extend_from_slice(&(market as u16).to_le_bytes());
        req.extend_from_slice(&code_buf);
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&count.to_le_bytes());

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        if data.len() < 2 {
            return Ok(vec![]);
        }

        let num = parse_u16_le(&data, 0, "history tick count")?;
        let mut pos = 6; // skip 2 bytes count + 4 bytes unknown
        let mut last_price: i64 = 0;

        let mut results = Vec::with_capacity(num as usize);

        'record: for _ in 0..num {
            if pos >= data.len() {
                break;
            }

            let (hour, minute, price_raw, vol, new_pos) = match Self::read_tick_prefix(&data, pos) {
                Some(v) => v,
                None => break,
            };
            pos = new_pos;

            let (buyorsell, p3) = read_price!('record, &data, pos);
            pos = p3;
            let (_reserved, p4) = read_price!('record, &data, pos);
            pos = p4;

            // F9: truncated record — stop instead of emitting zero-filled data.
            if pos > data.len() {
                break;
            }

            last_price = last_price.wrapping_add(price_raw);

            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("time", format!("{:02}:{:02}", hour, minute))?;
            dict.set_item("price", (last_price as f64) / 100.0)?;
            dict.set_item("vol", vol)?;
            dict.set_item("buyorsell", buyorsell)?;

            results.push(dict.into_any().unbind());
        }

        Ok(results)
    }

    // ===============================================================
    //  get_block_info_meta  —  板块元数据
    // ===============================================================
    /// Get block file metadata (size, hash).
    /// block_file: e.g. "block_zs.dat", "block_gn.dat", "block_fg.dat"
    pub fn get_block_info_meta(
        &mut self,
        py: Python<'_>,
        block_file: &str,
    ) -> PyResult<Py<PyAny>> {
        let stream = self.require_stream()?;

        // Packet: 0C 39 18 69 00 01 2A 00 2A 00 C5 02 <40s>
        let mut req = Vec::with_capacity(54);
        req.extend_from_slice(&[0x0C, 0x39, 0x18, 0x69, 0x00, 0x01, 0x2A, 0x00, 0x2A, 0x00]);
        req.extend_from_slice(&CMD_ID_BLOCK_META.to_le_bytes());
        let file_bytes = block_file.as_bytes();
        let mut buf = [0u8; 40]; // 0x2a - 2 = 40
        let len = std::cmp::min(40, file_bytes.len());
        buf[..len].copy_from_slice(&file_bytes[..len]);
        req.extend_from_slice(&buf);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        // Response: I(4) size + 1s + 32s hash + 1s = 38 bytes
        if data.len() < 38 {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Malformed block meta response",
            ));
        }

        let size = parse_u32_le(&data, 0, "block size")?;
        let hash_bytes = &data[5..37];
        let hash_value = String::from_utf8_lossy(hash_bytes)
            .trim_end_matches('\0')
            .to_string();

        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("size", size)?;
        dict.set_item("hash_value", hash_value)?;

        Ok(dict.into_any().unbind())
    }

    // ===============================================================
    //  get_block_info  —  板块详情数据
    // ===============================================================
    /// Get block file data chunk.
    /// block_file: e.g. "block_zs.dat"
    /// start: byte offset
    /// size: file total size
    /// Returns raw bytes (skip first 4 bytes of response body).
    pub fn get_block_info(
        &mut self,
        py: Python<'_>,
        block_file: &str,
        start: u32,
        size: u32,
    ) -> PyResult<std::borrow::Cow<'_, [u8]>> {
        let stream = self.require_stream()?;

        // Packet: 0c 37 18 6a 00 01 6e 00 6e 00 b9 06 <I I 96s>
        let mut req = Vec::with_capacity(112);
        req.extend_from_slice(&[0x0c, 0x37, 0x18, 0x6a, 0x00, 0x01, 0x6e, 0x00, 0x6e, 0x00]);
        req.extend_from_slice(&CMD_ID_BLOCK_DATA.to_le_bytes());
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&size.to_le_bytes());
        // pytdx: struct.pack("<II{0x6e-10}s", start, size, block_file)
        // 0x6e = 110 bytes total payload. cmd(2) + start(4) + size(4) + file_buf(100) = 110
        let file_bytes = block_file.as_bytes();
        let mut file_buf = [0u8; 100];
        let len = std::cmp::min(100, file_bytes.len());
        file_buf[..len].copy_from_slice(&file_bytes[..len]);
        req.extend_from_slice(&file_buf);

        // F13: blocking socket I/O runs with the GIL released; the request was
        // built above and the response is parsed into Python objects below.
        let data = py.detach(|| -> PyResult<Vec<u8>> {
            stream.write_all(&req)?;
            Self::read_response(stream)
        })?;

        // Response: skip first 4 bytes, return the rest
        if data.len() <= 4 {
            return Ok(std::borrow::Cow::Borrowed(&[]));
        }

        Ok(std::borrow::Cow::Owned(data[4..].to_vec()))
    }
}

/// Decode a GBK-encoded byte slice to a String.
/// Falls back to lossy UTF-8 if GBK decoding fails.
fn decode_gbk(bytes: &[u8]) -> String {
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_u16_le / parse_u32_le (F16) ----

    #[test]
    fn parse_u16_le_ok() {
        assert_eq!(parse_u16_le(&[0x34, 0x12], 0, "x").unwrap(), 0x1234);
        assert_eq!(
            parse_u16_le(&[0xFF, 0x34, 0x12, 0xFF], 1, "x").unwrap(),
            0x1234
        );
        assert_eq!(parse_u16_le(&[0xFF, 0xFF], 0, "x").unwrap(), u16::MAX);
    }

    #[test]
    fn parse_u16_le_malformed() {
        assert!(parse_u16_le(&[], 0, "x").is_err());
        assert!(parse_u16_le(&[0x01], 0, "x").is_err()); // 1 < 2 bytes
        assert!(parse_u16_le(&[0x01, 0x02], 1, "x").is_err()); // offset past end
        assert!(parse_u16_le(&[0x01, 0x02], usize::MAX, "x").is_err()); // no overflow panic
    }

    #[test]
    fn parse_u32_le_ok() {
        assert_eq!(
            parse_u32_le(&[0x78, 0x56, 0x34, 0x12], 0, "x").unwrap(),
            0x1234_5678
        );
        assert_eq!(
            parse_u32_le(&[0xAA, 0x78, 0x56, 0x34, 0x12], 1, "x").unwrap(),
            0x1234_5678
        );
    }

    #[test]
    fn parse_u32_le_malformed() {
        assert!(parse_u32_le(&[], 0, "x").is_err());
        assert!(parse_u32_le(&[0x01, 0x02, 0x03], 0, "x").is_err()); // 3 < 4 bytes
        assert!(parse_u32_le(&[0x01, 0x02, 0x03, 0x04], 1, "x").is_err());
        assert!(parse_u32_le(&[0x01, 0x02, 0x03, 0x04], usize::MAX, "x").is_err());
    }

    // ---- code_bytes (F16) ----

    #[test]
    fn code_bytes_exact_and_padding() {
        assert_eq!(TdxClient::code_bytes("600036"), *b"600036");
        assert_eq!(TdxClient::code_bytes("1"), [b'1', 0, 0, 0, 0, 0]);
        assert_eq!(TdxClient::code_bytes(""), [0u8; 6]);
    }

    #[test]
    fn code_bytes_truncation() {
        // Longer than 6 bytes: keep only the first 6 (pytdx <6s> behavior).
        assert_eq!(TdxClient::code_bytes("6000361234"), *b"600036");
        assert_eq!(TdxClient::code_bytes("0000000000"), *b"000000");
    }

    // ---- shared protocol helpers (bars / minute / tick) ----

    #[test]
    fn bars_request_byte_exact() {
        // Hand-built expected packet for (category=9, market=1, "600036", 0, 100).
        let expected: Vec<u8> = vec![
            0x0c, 0x01, // 0x010c u16
            0x08, 0x64, 0x01, 0x01, // 0x01016408 u32
            0x1c, 0x00, // data length: 28 bytes
            0x1c, 0x00, // data length (dup)
            0x2d, 0x05, // CMD_ID_BARS (0x052d)
            0x01, 0x00, // market = 1 (u16)
            b'6', b'0', b'0', b'0', b'3', b'6', // code, 6 bytes
            0x09, 0x00, // category = 9
            0x01, 0x00, // unknown flag
            0x00, 0x00, // start = 0
            0x64, 0x00, // count = 100
            0x00, 0x00, 0x00, 0x00, // reserved
            0x00, 0x00, 0x00, 0x00, // reserved
            0x00, 0x00, // reserved
        ];
        let req = TdxClient::build_bars_request(9, 1, "600036", 0, 100);
        assert_eq!(req.len(), 38);
        assert_eq!(req, expected);

        // Short code zero-padded, long code truncated; start/count little-endian.
        let req = TdxClient::build_bars_request(0, 0, "1", 5, 800);
        assert_eq!(&req[14..20], &[b'1', 0, 0, 0, 0, 0]);
        assert_eq!(&req[24..26], &5u16.to_le_bytes());
        assert_eq!(&req[26..28], &800u16.to_le_bytes());
        let req = TdxClient::build_bars_request(4, 1, "0000011234", 0, 1);
        assert_eq!(&req[14..20], b"000001");
    }

    #[test]
    fn bar_prices_hand_computed() {
        // First record (base 0): open=10.0, close=10.5, high=11.2, low=9.7.
        assert_eq!(
            TdxClient::bar_prices(10_000, 500, 1_200, -300, 0),
            (10.0, 11.2, 9.7, 10.5, 10_500)
        );
        // Chained record: base = previous close (10500).
        assert_eq!(
            TdxClient::bar_prices(-200, 100, 300, -400, 10_500),
            (10.3, 10.6, 9.9, 10.4, 10_400)
        );
        // Negative base: diffs can drive prices below zero.
        assert_eq!(
            TdxClient::bar_prices(2_000, -1_000, 500, -2_000, -5_000),
            (-3.0, -2.5, -5.0, -4.0, -4_000)
        );
        // Large base near i64::MAX with sane diffs: no overflow.
        let base = i64::MAX - 1_000;
        let (open, high, low, close, new_base) = TdxClient::bar_prices(100, 100, 200, -100, base);
        assert_eq!(new_base, i64::MAX - 800);
        assert_eq!(open, ((i64::MAX - 900) as f64) / 1000.0);
        assert_eq!(high, ((i64::MAX - 700) as f64) / 1000.0);
        assert_eq!(low, ((i64::MAX - 1_000) as f64) / 1000.0);
        assert_eq!(close, ((i64::MAX - 800) as f64) / 1000.0);
    }

    #[test]
    fn parse_minute_records_synthetic_truncated_and_wrapping() {
        // Records are price/reserved/vol varints; all values here fit one byte.
        // rec1: +10, 0, 30 | rec2: -5, 0, 40 | rec3: +20, 0, 50
        let data = [0x0A, 0x00, 0x1E, 0x45, 0x00, 0x28, 0x14, 0x00, 0x32];
        // The crate is built without pyo3's auto-initialize feature (extension
        // module); initialize the embedded interpreter for this GIL test.
        Python::initialize();
        Python::attach(|py| {
            let field = |rec: &Py<PyAny>, key: &str| -> f64 {
                rec.bind(py)
                    .cast::<pyo3::types::PyDict>()
                    .unwrap()
                    .get_item(key)
                    .unwrap()
                    .unwrap()
                    .extract()
                    .unwrap()
            };

            // Cumulative price: 10, 10-5=5, 5+20=25 (scaled /100).
            let records = TdxClient::parse_minute_records(py, &data, 0, 3).unwrap();
            assert_eq!(records.len(), 3);
            let prices: Vec<f64> = records.iter().map(|r| field(r, "price")).collect();
            let vols: Vec<f64> = records.iter().map(|r| field(r, "vol")).collect();
            assert_eq!(prices, vec![0.10, 0.05, 0.25]);
            assert_eq!(vols, vec![30.0, 40.0, 50.0]);

            // `pos` offset is honored (call sites pass 4 / 6).
            let mut prefixed = vec![0xFF; 4];
            prefixed.extend_from_slice(&data);
            let records = TdxClient::parse_minute_records(py, &prefixed, 4, 3).unwrap();
            assert_eq!(records.len(), 3);
            assert_eq!(field(&records[0], "price"), 0.10);

            // num larger than the buffer: stop at end, no zero-filled records.
            let records = TdxClient::parse_minute_records(py, &data, 0, 5).unwrap();
            assert_eq!(records.len(), 3);

            // Truncated mid-record (vol missing): drop the partial record.
            let records = TdxClient::parse_minute_records(py, &data[..5], 0, 2).unwrap();
            assert_eq!(records.len(), 1);

            // Truncated varint continuation: same drop behavior.
            let records = TdxClient::parse_minute_records(py, &[0x0A, 0x00, 0x1E, 0x80], 0, 2)
                .unwrap();
            assert_eq!(records.len(), 1);

            // Empty buffer / zero count.
            assert!(TdxClient::parse_minute_records(py, &[], 0, 3)
                .unwrap()
                .is_empty());
            assert!(TdxClient::parse_minute_records(py, &data, 0, 0)
                .unwrap()
                .is_empty());

            // Wrapping accumulation: i64::MAX then +1 wraps to i64::MIN.
            let mut wrap_data = vec![0xBF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
            wrap_data.extend_from_slice(&[0x00, 0x01]); // reserved=0, vol=1
            wrap_data.extend_from_slice(&[0x01, 0x00, 0x01]); // price=+1, reserved=0, vol=1
            let records = TdxClient::parse_minute_records(py, &wrap_data, 0, 2).unwrap();
            assert_eq!(records.len(), 2);
            assert_eq!(field(&records[0], "price"), (i64::MAX as f64) / 100.0);
            assert_eq!(field(&records[1], "price"), (i64::MIN as f64) / 100.0);
        });
    }

    #[test]
    fn read_tick_prefix_normal_and_truncated() {
        // 09:30 = 570 minutes, price diff +10, vol 30.
        let mut data = (9u16 * 60 + 30).to_le_bytes().to_vec();
        data.extend_from_slice(&[0x0A, 0x1E]);
        assert_eq!(
            TdxClient::read_tick_prefix(&data, 0),
            Some((9, 30, 10, 30, 4))
        );
        // Offset start.
        let mut prefixed = vec![0xFF, 0xFF];
        prefixed.extend_from_slice(&data);
        assert_eq!(
            TdxClient::read_tick_prefix(&prefixed, 2),
            Some((9, 30, 10, 30, 6))
        );
        // Truncation at every stage returns None (caller breaks its loop).
        assert_eq!(TdxClient::read_tick_prefix(&[], 0), None);
        assert_eq!(TdxClient::read_tick_prefix(&[0x00], 0), None); // time needs 2 bytes
        assert_eq!(TdxClient::read_tick_prefix(&data[..2], 0), None); // no price
        assert_eq!(TdxClient::read_tick_prefix(&data[..3], 0), None); // no vol
        // Truncated price continuation: pos runs past end, next read fails.
        assert_eq!(TdxClient::read_tick_prefix(&[0x3A, 0x02, 0x80], 0), None);
    }

    // ---- quotes_packet_len overflow guard (F6-rust / F16) ----

    #[test]
    fn quotes_packet_len_normal() {
        assert_eq!(quotes_packet_len(1).unwrap(), 19);
        assert_eq!(quotes_packet_len(80).unwrap(), 572); // G1 batch size
        // Largest stock_len whose payload still fits u16: 9360*7+12 = 65532.
        assert_eq!(quotes_packet_len(9360).unwrap(), 65532);
    }

    #[test]
    fn quotes_packet_len_overflow_rejected() {
        // 9361*7+12 = 65539 > u16::MAX: must error, not truncate.
        assert!(quotes_packet_len(9361).is_err());
        assert!(quotes_packet_len(100_000).is_err());
        // usize multiplication overflow must also error, not panic.
        assert!(quotes_packet_len(usize::MAX).is_err());
        assert!(quotes_packet_len(usize::MAX / 7 + 1).is_err());
    }

    // ---- resolve_addr (F22) ----

    #[test]
    fn resolve_addr_ip_literal() {
        let addrs = resolve_addr("127.0.0.1:7709").unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
        assert_eq!(addrs[0].port(), 7709);
    }

    #[test]
    fn resolve_addr_invalid() {
        assert!(resolve_addr("not an address").is_err());
        assert!(resolve_addr("127.0.0.1").is_err()); // missing port
        assert!(resolve_addr("127.0.0.1:99999").is_err()); // port out of range
        assert!(resolve_addr("").is_err());
    }

    // ---- loopback helpers (offline: 127.0.0.1 only) ----

    /// Create a connected (client, server) TcpStream pair on loopback.
    fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        (client, server)
    }

    /// Build a 16-byte response header with the given zip/unzip sizes.
    fn make_header(zip_size: u16, unzip_size: u16) -> [u8; RESP_HEADER_LEN] {
        let mut h = [0u8; RESP_HEADER_LEN];
        h[12..14].copy_from_slice(&zip_size.to_le_bytes());
        h[14..16].copy_from_slice(&unzip_size.to_le_bytes());
        h
    }

    // ---- drain_setup_response (F10) ----

    #[test]
    fn drain_reads_fragmented_header_and_exact_body() {
        let (mut client, mut server) = loopback_pair();
        let body = [0x11u8; 5];
        let marker = 0x42u8;
        let writer = std::thread::spawn(move || {
            let header = make_header(5, 5);
            // Deliver the header in two fragments with a pause so a single
            // read() would previously have returned a partial header.
            server.write_all(&header[..7]).unwrap();
            server.flush().unwrap();
            std::thread::sleep(Duration::from_millis(30));
            server.write_all(&header[7..]).unwrap();
            server.write_all(&body).unwrap();
            server.write_all(&[marker]).unwrap();
            server.flush().unwrap();
        });
        drain_setup_response(&mut client).unwrap();
        // The drain must consume exactly header+body: the marker is next.
        let mut next = [0u8; 1];
        client.read_exact(&mut next).unwrap();
        assert_eq!(next[0], marker);
        writer.join().unwrap();
    }

    #[test]
    fn drain_zero_body() {
        let (mut client, mut server) = loopback_pair();
        let header = make_header(0, 0);
        server.write_all(&header).unwrap();
        server.write_all(&[0x77]).unwrap();
        drain_setup_response(&mut client).unwrap();
        let mut next = [0u8; 1];
        client.read_exact(&mut next).unwrap();
        assert_eq!(next[0], 0x77);
    }

    #[test]
    fn drain_propagates_error_on_partial_header() {
        let (mut client, mut server) = loopback_pair();
        // Peer sends only part of the header, then closes the connection.
        server.write_all(&[0u8; 8]).unwrap();
        drop(server);
        // F10: this must be an error now, not a silently swallowed Ok.
        assert!(drain_setup_response(&mut client).is_err());
    }

    #[test]
    fn drain_propagates_error_on_partial_body() {
        let (mut client, mut server) = loopback_pair();
        let header = make_header(10, 10);
        server.write_all(&header).unwrap();
        server.write_all(&[0u8; 4]).unwrap(); // only 4 of 10 body bytes
        drop(server);
        assert!(drain_setup_response(&mut client).is_err());
    }

    // ---- read_response framing ----

    #[test]
    fn read_response_uncompressed() {
        let (mut client, mut server) = loopback_pair();
        let body: Vec<u8> = (0..50u8).collect();
        let header = make_header(body.len() as u16, body.len() as u16);
        server.write_all(&header).unwrap();
        server.write_all(&body).unwrap();
        let got = TdxClient::read_response(&mut client).unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn read_response_zlib_compressed() {
        let (mut client, mut server) = loopback_pair();
        let original: Vec<u8> = (0..200u8).map(|i| i % 7).collect();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&original).unwrap();
        let compressed = enc.finish().unwrap();
        assert_ne!(compressed.len(), original.len());
        let header = make_header(compressed.len() as u16, original.len() as u16);
        server.write_all(&header).unwrap();
        server.write_all(&compressed).unwrap();
        let got = TdxClient::read_response(&mut client).unwrap();
        assert_eq!(got, original);
    }

    // ---- connect_and_handshake (F10 + F22 integration, loopback mock) ----

    /// Mock TDX server: answers the three setup commands with empty responses.
    fn mock_handshake_server(listener: std::net::TcpListener) {
        let (mut sock, _) = listener.accept().unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        for cmd_len in [13usize, 13, 42] {
            let mut cmd = vec![0u8; cmd_len];
            sock.read_exact(&mut cmd).unwrap();
            sock.write_all(&make_header(0, 0)).unwrap();
        }
    }

    #[test]
    fn connect_and_handshake_success() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || mock_handshake_server(listener));
        let stream =
            TdxClient::connect_and_handshake(&format!("127.0.0.1:{}", addr.port())).unwrap();
        assert!(stream.nodelay().unwrap()); // F22: TCP_NODELAY enabled
        server.join().unwrap();
    }

    #[test]
    fn connect_and_handshake_fails_cleanly_on_broken_handshake() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut cmd = [0u8; 13];
            sock.read_exact(&mut cmd).unwrap();
            // Reply with a partial header, then close: connect must fail.
            sock.write_all(&[0u8; 6]).unwrap();
        });
        let result = TdxClient::connect_and_handshake(&format!("127.0.0.1:{}", addr.port()));
        assert!(result.is_err());
        server.join().unwrap();
    }

    #[test]
    fn connect_and_handshake_refused() {
        // Bind then drop the listener so the port is (almost certainly) closed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let result = TdxClient::connect_and_handshake(&format!("127.0.0.1:{}", addr.port()));
        assert!(result.is_err());
    }
}
