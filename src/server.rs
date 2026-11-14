use pyo3::prelude::*;
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant};

/// Ping multiple TDX HQ servers concurrently and return their response latencies.
/// Returns a list of tuples (ip, port, latency_ms). If a server fails, latency is returned as 9999.
#[pyfunction]
pub fn ping_servers(
    py: Python<'_>,
    servers: Vec<(String, u16)>,
) -> PyResult<Vec<(String, u16, u64)>> {
    // F13: the spawn/join loop blocks until every ping finishes (up to the
    // connect timeout per server); run it with the GIL released.
    // F22: hostnames are resolved inside each worker thread via ToSocketAddrs.
    // NOTE: DNS resolution itself is blocking, which is another reason this
    // whole section must stay off the GIL.
    let results = py.detach(|| {
        let mut handles = vec![];

        // Spawn threads for each server ping
        for (ip, port) in servers {
            let handle = thread::spawn(move || {
                let addr = format!("{}:{}", ip, port);

                // Resolve IP literals and hostnames alike; failure -> 9999.
                let sock_addrs: Vec<std::net::SocketAddr> = match addr.to_socket_addrs() {
                    Ok(iter) => iter.collect(),
                    Err(_) => return (ip, port, 9999),
                };

                // Measure TCP connect latency only: including DNS resolution
                // time would skew the fastest-server ranking for hostnames.
                let start = Instant::now();

                let mut latency = 9999u64;
                for sock_addr in sock_addrs {
                    match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(2)) {
                        Ok(stream) => {
                            // F22: disable Nagle on the probe connection too.
                            let _ = stream.set_nodelay(true);
                            latency = start.elapsed().as_millis() as u64;
                            break;
                        }
                        Err(_) => continue,
                    }
                }
                (ip, port, latency)
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
        results
    });

    Ok(results)
}
