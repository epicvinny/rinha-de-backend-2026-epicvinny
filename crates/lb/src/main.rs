use lb::{run, LbConfig, Metrics};
use std::net::SocketAddr;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let listen = std::env::var("LB_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:9999".to_string())
        .parse::<SocketAddr>()
        .expect("invalid LB_LISTEN");

    let backends = std::env::var("BACKENDS").unwrap_or_else(|_| "api1:8080,api2:8080".to_string());
    let mut parts = backends.split(',').map(str::trim).filter(|s| !s.is_empty());
    let backend0 = parts
        .next()
        .expect("BACKENDS must include api1")
        .to_string();
    let backend1 = parts
        .next()
        .expect("BACKENDS must include api2")
        .to_string();
    assert!(
        parts.next().is_none(),
        "BACKENDS must contain exactly two entries"
    );

    let trace = std::env::var("LB_TRACE").ok().as_deref() == Some("1");
    let trace_every = std::env::var("LB_TRACE_EVERY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10_000)
        .max(1);
    let upstream_pool_per_backend = std::env::var("UPSTREAM_POOL_PER_BACKEND")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(32)
        .clamp(1, 256);
    let upstream_preconnect_per_backend = std::env::var("UPSTREAM_PRECONNECT_PER_BACKEND")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(upstream_pool_per_backend)
        .min(upstream_pool_per_backend);

    let config = LbConfig {
        listen,
        backends: [backend0, backend1],
        upstream_pool_per_backend,
        upstream_preconnect_per_backend,
        metrics: if trace {
            Some(Metrics::new(trace_every))
        } else {
            None
        },
    };

    eprintln!(
        "LB listening on {}; backends={},{}; upstream_pool_per_backend={}; upstream_preconnect_per_backend={}; trace={}",
        config.listen,
        config.backends[0],
        config.backends[1],
        config.upstream_pool_per_backend,
        config.upstream_preconnect_per_backend,
        trace
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current-thread runtime");

    runtime
        .block_on(run(config))
        .expect("load balancer exited with error");
}
