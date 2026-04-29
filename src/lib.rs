mod network;
mod protocol;
mod reader;
mod server;

use pyo3::prelude::*;

/// mitdx._core — High-performance TDX data reader implemented in Rust
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(reader::read_daily_bars, m)?)?;
    m.add_function(wrap_pyfunction!(reader::read_minute_bars, m)?)?;
    m.add_function(wrap_pyfunction!(server::ping_servers, m)?)?;
    m.add_class::<network::TdxClient>()?;
    Ok(())
}
