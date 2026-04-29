use pyo3::prelude::*;
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

/// Ping multiple TDX HQ servers concurrently and return their response latencies.
/// Returns a list of tuples (ip, port, latency_ms). If a server fails, latency is returned as 9999.
#[pyfunction]
pub fn ping_servers(servers: Vec<(String, u16)>) -> PyResult<Vec<(String, u16, u64)>> {
    let mut handles = vec![];

    // Spawn threads for each server ping
    for (ip, port) in servers {
        let handle = thread::spawn(move || {
            let addr = format!("{}:{}", ip, port);
            let start = Instant::now();

            let sock_addr = match addr.parse() {
                Ok(a) => a,
                Err(_) => return (ip, port, 9999),
            };
            match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(2)) {
                Ok(_) => {
                    let duration = start.elapsed().as_millis() as u64;
                    (ip, port, duration)
                }
                Err(_) => (ip, port, 9999),
            }
        });
        handles.push(handle);
    }

    let mut results = vec![];
    for handle in handles {
        if let Ok(res) = handle.join() {
            results.push(res);
        }
    }

    // Sort by latency ascending
    results.sort_by_key(|k| k.2);

    Ok(results)
}
