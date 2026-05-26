use axum::{
    extract::State,
    http::header::CONTENT_TYPE,
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Router,
};
use memmap2::MmapOptions;
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod perf;
mod search;
use search::Index;

#[derive(Clone)]
struct AppState {
    index: Arc<Index>,
    ready: Arc<AtomicBool>,
    perf: Option<Arc<perf::PerfCollector>>,
    log_search_avg: bool,
}

static REQ_COUNT: AtomicU64 = AtomicU64::new(0);
static TOTAL_US: AtomicU64 = AtomicU64::new(0);

async fn health_check(State(state): State<AppState>) -> StatusCode {
    if state.ready.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn fraud_score(
    State(state): State<AppState>,
    body_bytes: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let handler_start = Instant::now();

    if let Some(perf) = state.perf.as_deref() {
        let request = perf.begin_request();

        let parse_start = Instant::now();
        let payload: shared::types::Payload<'_> =
            serde_json::from_slice(&body_bytes).map_err(|_e| StatusCode::BAD_REQUEST)?;
        let json_parse_us = perf::elapsed_us(parse_start);

        let mut search_trace = perf::SearchTrace::default();
        let search_start = Instant::now();
        let (_approved, fraud_score_val) =
            state.index.search_with_trace(&payload, &mut search_trace);
        let search_total_us = perf::elapsed_us(search_start);

        let response_start = Instant::now();
        let response = response_for_fraud_score(fraud_score_val);
        let response_build_us = perf::elapsed_us(response_start);

        let handler_total_us = perf::elapsed_us(handler_start);
        perf.record_request(perf::RequestPerf {
            request_id: request.request_id(),
            in_flight_at_start: request.in_flight_at_start(),
            max_in_flight_seen: perf.max_in_flight_seen(),
            handler_total_us,
            json_parse_us,
            search_total_us,
            response_build_us,
            search: search_trace,
        });

        return response;
    }

    let payload: shared::types::Payload<'_> =
        serde_json::from_slice(&body_bytes).map_err(|_e| StatusCode::BAD_REQUEST)?;

    let (_approved, fraud_score_val) = state.index.search(&payload);

    if state.log_search_avg {
        let elapsed = handler_start.elapsed().as_micros() as u64;
        let count = REQ_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        let total = TOTAL_US.fetch_add(elapsed, Ordering::Relaxed) + elapsed;

        if count % 10000 == 0 {
            eprintln!(
                "Avg handler time over {} requests: {:.3} ms",
                count,
                (total as f64) / (count as f64) / 1000.0
            );
        }
    }

    response_for_fraud_score(fraud_score_val)
}

fn response_for_fraud_score(fraud_score_val: f64) -> Result<Response, StatusCode> {
    let fraud_count = (fraud_score_val * 5.0 + 0.1) as usize;
    let response_body = match fraud_count {
        0 => r#"{"approved":true,"fraud_score":0}"#,
        1 => r#"{"approved":true,"fraud_score":0.2}"#,
        2 => r#"{"approved":true,"fraud_score":0.4}"#,
        3 => r#"{"approved":false,"fraud_score":0.6}"#,
        4 => r#"{"approved":false,"fraud_score":0.8}"#,
        5 => r#"{"approved":false,"fraud_score":1}"#,
        _ => r#"{"approved":false,"fraud_score":1}"#,
    };

    Ok(Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(response_body))
        .unwrap())
}

fn main() {
    #[cfg(target_feature = "avx2")]
    eprintln!("API Compiled with AVX2 enabled.");
    #[cfg(not(target_feature = "avx2"))]
    eprintln!("WARNING: API Compiled WITHOUT AVX2 enabled!");

    let index_path = std::env::var("INDEX_PATH").unwrap_or_else(|_| "/opt/index.bin".to_string());
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .unwrap_or(8080);
    let perf = perf::PerfConfig::from_env().map(|config| {
        eprintln!(
            "PERF_TRACE enabled: every={} slow_us={} sample={}",
            config.every, config.slow_us, config.sample
        );
        Arc::new(perf::PerfCollector::new(config))
    });
    let log_search_avg = std::env::var("LOG_SEARCH_AVG").ok().as_deref() == Some("1");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async move {
        eprintln!("Loading index from {}...", index_path);
        let file = File::open(&index_path).expect("failed to open index.bin");
        let mmap = unsafe { MmapOptions::new().map(&file).expect("failed to mmap index") };

        #[cfg(unix)]
        if std::env::var("MLOCK_INDEX").ok().as_deref() == Some("1") {
            unsafe {
                let ptr = mmap.as_ptr() as *const libc::c_void;
                let len = mmap.len();
                if libc::mlock(ptr, len) == 0 {
                    eprintln!("Successfully locked index memory of size {} bytes in RAM via mlock.", len);
                } else {
                    let err = std::io::Error::last_os_error();
                    eprintln!("Warning: Failed to lock index memory in RAM: {}. Performance under memory pressure may degrade.", err);
                }
            }
        }

        let index = Arc::new(Index::new(mmap));
        let ready = Arc::new(AtomicBool::new(false));

        // Warmup in background
        let index_clone = Arc::clone(&index);
        let ready_clone = Arc::clone(&ready);
        tokio::task::spawn_blocking(move || {
            eprintln!("Warming up index...");
            index_clone.warmup();
            eprintln!("Index warm. Serving on port {}", port);
            ready_clone.store(true, Ordering::Relaxed);
        });

        let state = AppState {
            index,
            ready,
            perf,
            log_search_avg,
        };
        let app = Router::new()
            .route("/ready", get(health_check))
            .route("/fraud-score", post(fraud_score))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
            .await
            .expect("failed to bind");
        eprintln!("Listening on port {}", port);
        axum::serve(listener, app).await.expect("server error");
    });
}
