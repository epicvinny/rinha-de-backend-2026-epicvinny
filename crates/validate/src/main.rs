use serde::Deserialize;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct QueryEntry<'a> {
    #[serde(borrow)]
    request: shared::types::Payload<'a>,
    expected_approved: bool,
    expected_fraud_score: f64,
}

#[derive(Deserialize)]
struct TestData<'a> {
    #[serde(borrow)]
    entries: Vec<QueryEntry<'a>>,
}

struct Score {
    n: usize,
    tp: usize,
    tn: usize,
    fp: usize,
    fn_: usize,
    err: usize,
    latencies_us: Vec<u64>,
}

impl Score {
    fn p99_ms(&self) -> f64 {
        if self.latencies_us.is_empty() {
            return 0.0;
        }
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1);
        sorted[idx] as f64 / 1000.0
    }

    fn p50_ms(&self) -> f64 {
        if self.latencies_us.is_empty() {
            return 0.0;
        }
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2] as f64 / 1000.0
    }

    fn avg_ms(&self) -> f64 {
        if self.latencies_us.is_empty() {
            return 0.0;
        }
        self.latencies_us.iter().sum::<u64>() as f64 / self.latencies_us.len() as f64 / 1000.0
    }

    fn weighted_e(&self) -> f64 {
        (self.fp as f64) * 1.0 + (self.fn_ as f64) * 3.0 + (self.err as f64) * 5.0
    }

    fn failure_rate(&self) -> f64 {
        (self.fp + self.fn_ + self.err) as f64 / self.n as f64
    }

    fn score_p99(&self) -> f64 {
        let p99 = self.p99_ms();
        let p99_max = 2000.0_f64;
        let p99_min = 1.0_f64;
        let t_max = 1000.0_f64;
        let k = 1000.0_f64;
        if p99 > p99_max {
            -3000.0
        } else {
            (k * (t_max / p99.max(p99_min)).log10()).min(3000.0)
        }
    }

    fn score_det(&self) -> (f64, Option<f64>, Option<f64>) {
        let failure_rate = self.failure_rate();
        if failure_rate > 0.15 {
            return (-3000.0, None, None);
        }
        let e = self.weighted_e();
        let eps_min = 0.001_f64;
        let eps = (e / self.n as f64).max(eps_min);
        let k = 1000.0_f64;
        let beta = 300.0_f64;
        let rate_component = k * (1.0 / eps).log10();
        let abs_penalty = -beta * (1.0 + e).log10();
        let score = (rate_component + abs_penalty).min(3000.0).max(-3000.0);
        (score, Some(rate_component), Some(abs_penalty))
    }

    fn final_score(&self) -> f64 {
        let (det, _, _) = self.score_det();
        self.score_p99() + det
    }
}

fn build_request_json(q: &QueryEntry) -> ureq::serde_json::Value {
    ureq::serde_json::json!({
        "id": q.request.id,
        "transaction": {
            "amount": q.request.transaction.amount,
            "installments": q.request.transaction.installments,
            "requested_at": q.request.transaction.requested_at,
        },
        "customer": {
            "avg_amount": q.request.customer.avg_amount,
            "tx_count_24h": q.request.customer.tx_count_24h,
            "known_merchants": q.request.customer.known_merchants,
        },
        "merchant": {
            "id": q.request.merchant.id,
            "mcc": q.request.merchant.mcc,
            "avg_amount": q.request.merchant.avg_amount,
        },
        "terminal": {
            "is_online": q.request.terminal.is_online,
            "card_present": q.request.terminal.card_present,
            "km_from_home": q.request.terminal.km_from_home,
        },
        "last_transaction": q.request.last_transaction.as_ref().map(|lt| ureq::serde_json::json!({
            "timestamp": lt.timestamp,
            "km_from_current": lt.km_from_current,
        })),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut queries_path = PathBuf::new();
    let mut api_url = String::from("http://localhost:9999");
    let mut out_csv = PathBuf::from("mismatches.csv");
    let mut out_html = PathBuf::from("report.html");
    let mut participant = String::from("epicvinny");
    let mut stack = String::from("Rust + axum + nginx");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--queries" => {
                queries_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--api-url" => {
                api_url = args[i + 1].clone();
                i += 2;
            }
            "--out" => {
                out_csv = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--html" => {
                out_html = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--participant" => {
                participant = args[i + 1].clone();
                i += 2;
            }
            "--stack" => {
                stack = args[i + 1].clone();
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let data = std::fs::read_to_string(&queries_path).expect("failed to read queries file");
    let queries: Vec<QueryEntry> = if data.trim_start().starts_with('[') {
        serde_json::from_str(&data).expect("failed to parse queries array")
    } else {
        let td: TestData = serde_json::from_str(&data).expect("failed to parse test data");
        td.entries
    };

    let n = queries.len();
    let client = ureq::Agent::new();
    let mut out = csv::Writer::from_path(&out_csv).expect("failed to create output CSV");
    out.write_record(&[
        "query_id",
        "expected_approved",
        "expected_score",
        "got_approved",
        "got_score",
        "match",
        "latency_us",
    ])
    .unwrap();

    let mut sc = Score {
        n,
        tp: 0,
        tn: 0,
        fp: 0,
        fn_: 0,
        err: 0,
        latencies_us: Vec::with_capacity(n),
    };

    eprintln!("Running {} queries against {}...", n, api_url);
    let test_start = Instant::now();

    for (idx, q) in queries.iter().enumerate() {
        if idx > 0 && idx % 5000 == 0 {
            eprintln!("  {}/{} ({:.1}%)", idx, n, idx as f64 / n as f64 * 100.0);
        }

        let url = format!("{}/fraud-score", api_url);
        let body_json = build_request_json(q);

        let t0 = Instant::now();
        let resp = client.post(&url).send_json(body_json.clone()).or_else(|e| {
            if matches!(e, ureq::Error::Transport(_)) {
                client.post(&url).send_json(body_json.clone())
            } else {
                Err(e)
            }
        });
        let elapsed_us = t0.elapsed().as_micros() as u64;

        match resp {
            Ok(r) => {
                let body: serde_json::Value = r.into_json().unwrap_or_default();
                let got_approved = body["approved"].as_bool().unwrap_or(false);
                let got_score = body["fraud_score"].as_f64().unwrap_or(-1.0);

                let is_match = got_approved == q.expected_approved
                    && (got_score - q.expected_fraud_score).abs() < 1e-9;

                // Confusion matrix
                let is_fraud = !q.expected_approved;
                match (is_fraud, got_approved) {
                    (true, false) => sc.tp += 1,
                    (false, true) => sc.tn += 1,
                    (false, false) => sc.fp += 1,
                    (true, true) => sc.fn_ += 1,
                }

                sc.latencies_us.push(elapsed_us);
                let exp_app = q.expected_approved.to_string();
                let exp_score = q.expected_fraud_score.to_string();
                let got_app = got_approved.to_string();
                let got_sc = got_score.to_string();
                let is_m = is_match.to_string();
                let el_us = elapsed_us.to_string();

                out.write_record(&[
                    q.request.id,
                    &exp_app,
                    &exp_score,
                    &got_app,
                    &got_sc,
                    &is_m,
                    &el_us,
                ])
                .unwrap();
            }
            Err(e) => {
                sc.err += 1;
                eprintln!("Error for {}: {}", q.request.id, e);
                let exp_app = q.expected_approved.to_string();
                let exp_score = q.expected_fraud_score.to_string();
                let el_us = elapsed_us.to_string();
                out.write_record(&[
                    q.request.id,
                    &exp_app,
                    &exp_score,
                    "ERROR",
                    "ERROR",
                    "false",
                    &el_us,
                ])
                .unwrap();
            }
        }
    }

    out.flush().unwrap();
    let total_elapsed = test_start.elapsed().as_secs_f64();

    let p99 = sc.p99_ms();
    let p50 = sc.p50_ms();
    let avg = sc.avg_ms();
    let sp99 = sc.score_p99();
    let (sdet, rate_comp, abs_pen) = sc.score_det();
    let final_score = sp99 + sdet;
    let failure_rate = sc.failure_rate() * 100.0;
    let e = sc.weighted_e();
    let eps = (e / n as f64).max(0.001);
    let matches = sc.tp + sc.tn;

    eprintln!("\n=== RESULTS ===");
    eprintln!(
        "Total: {}  |  Match: {}/{} ({:.2}%)",
        n,
        matches,
        n,
        matches as f64 / n as f64 * 100.0
    );
    eprintln!(
        "TP={} TN={} FP={} FN={} Err={}",
        sc.tp, sc.tn, sc.fp, sc.fn_, sc.err
    );
    eprintln!(
        "Failure rate: {:.2}%  |  E={:.0}  |  ε={:.4}",
        failure_rate, e, eps
    );
    eprintln!(
        "Latency — p50={:.2}ms  p99={:.2}ms  avg={:.2}ms",
        p50, p99, avg
    );
    eprintln!(
        "score_p99={:.2}  score_det={:.2}  final={:.2}",
        sp99, sdet, final_score
    );
    eprintln!("Elapsed: {:.1}s", total_elapsed);

    generate_html(
        &out_html,
        &participant,
        &stack,
        n,
        &sc,
        p50,
        p99,
        avg,
        sp99,
        sdet,
        rate_comp,
        abs_pen,
        final_score,
        failure_rate,
        e,
        eps,
        total_elapsed,
    );
    eprintln!("\nHTML report: {}", out_html.display());
}

fn score_color(score: f64) -> &'static str {
    if score >= 4000.0 {
        "#00d4aa"
    } else if score >= 2000.0 {
        "#4ade80"
    } else if score >= 0.0 {
        "#facc15"
    } else {
        "#f87171"
    }
}

fn generate_html(
    path: &PathBuf,
    participant: &str,
    stack: &str,
    n: usize,
    sc: &Score,
    p50: f64,
    p99: f64,
    avg: f64,
    sp99: f64,
    sdet: f64,
    rate_comp: Option<f64>,
    abs_pen: Option<f64>,
    final_score: f64,
    failure_rate: f64,
    e: f64,
    eps: f64,
    elapsed: f64,
) {
    let now = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    let matches = sc.tp + sc.tn;
    let accuracy = matches as f64 / n as f64 * 100.0;
    let fraud_total = sc.tp + sc.fn_;
    let legit_total = sc.tn + sc.fp;
    let det_cut = sc.failure_rate() > 0.15;
    let lat_cut = p99 > 2000.0;
    let fc = score_color(final_score);
    let rate_str = rate_comp
        .map(|v| format!("{:.2}", v))
        .unwrap_or_else(|| "N/A (cutoff)".into());
    let pen_str = abs_pen
        .map(|v| format!("{:.2}", v))
        .unwrap_or_else(|| "N/A (cutoff)".into());

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Rinha de Backend 2026 — Local Report</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0f0f1a; color: #e2e8f0; font-family: 'Segoe UI', system-ui, sans-serif; min-height: 100vh; }}
  .header {{ background: linear-gradient(135deg, #1a1a2e 0%, #16213e 50%, #0f3460 100%); padding: 40px 20px; text-align: center; border-bottom: 1px solid #2d3748; }}
  .header h1 {{ font-size: 2.2rem; font-weight: 800; letter-spacing: -0.5px; }}
  .header h1 span {{ color: #4ade80; }}
  .header p {{ color: #94a3b8; margin-top: 8px; font-size: 0.95rem; }}
  .container {{ max-width: 1100px; margin: 0 auto; padding: 32px 20px; }}
  .final-score-card {{ background: linear-gradient(135deg, #1e293b, #0f172a); border: 2px solid {fc}; border-radius: 16px; padding: 32px; text-align: center; margin-bottom: 32px; position: relative; overflow: hidden; }}
  .final-score-card::before {{ content: ''; position: absolute; top: -50%; left: -50%; width: 200%; height: 200%; background: radial-gradient(circle, {fc}15 0%, transparent 60%); pointer-events: none; }}
  .final-score-label {{ font-size: 0.85rem; font-weight: 600; letter-spacing: 2px; text-transform: uppercase; color: #94a3b8; margin-bottom: 8px; }}
  .final-score-value {{ font-size: 5rem; font-weight: 900; color: {fc}; line-height: 1; }}
  .final-score-sub {{ color: #64748b; font-size: 0.85rem; margin-top: 8px; }}
  .participant-info {{ display: flex; gap: 16px; justify-content: center; flex-wrap: wrap; margin-top: 20px; }}
  .badge {{ background: #1e293b; border: 1px solid #334155; border-radius: 8px; padding: 6px 14px; font-size: 0.82rem; color: #94a3b8; }}
  .badge strong {{ color: #e2e8f0; }}
  .grid2 {{ display: grid; grid-template-columns: 1fr 1fr; gap: 20px; margin-bottom: 20px; }}
  @media (max-width: 600px) {{ .grid2 {{ grid-template-columns: 1fr; }} }}
  .card {{ background: #1e293b; border: 1px solid #2d3748; border-radius: 12px; padding: 24px; }}
  .card-title {{ font-size: 0.75rem; font-weight: 700; letter-spacing: 1.5px; text-transform: uppercase; color: #64748b; margin-bottom: 16px; }}
  .score-row {{ display: flex; justify-content: space-between; align-items: center; padding: 10px 0; border-bottom: 1px solid #1e293b55; }}
  .score-row:last-child {{ border-bottom: none; }}
  .score-row .label {{ color: #94a3b8; font-size: 0.9rem; }}
  .score-row .value {{ font-weight: 700; font-size: 1rem; }}
  .positive {{ color: #4ade80; }}
  .negative {{ color: #f87171; }}
  .neutral {{ color: #e2e8f0; }}
  .warning {{ color: #facc15; }}
  .cut-tag {{ background: #7f1d1d; color: #fca5a5; border-radius: 4px; font-size: 0.72rem; padding: 2px 6px; margin-left: 8px; font-weight: 600; }}
  .ok-tag {{ background: #14532d; color: #86efac; border-radius: 4px; font-size: 0.72rem; padding: 2px 6px; margin-left: 8px; font-weight: 600; }}
  .stat-grid {{ display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 20px; }}
  @media (max-width: 700px) {{ .stat-grid {{ grid-template-columns: repeat(2, 1fr); }} }}
  .stat-box {{ background: #1e293b; border: 1px solid #2d3748; border-radius: 10px; padding: 16px; text-align: center; }}
  .stat-box .s-val {{ font-size: 1.8rem; font-weight: 800; line-height: 1; }}
  .stat-box .s-lbl {{ font-size: 0.72rem; color: #64748b; margin-top: 4px; letter-spacing: 0.5px; text-transform: uppercase; }}
  .confusion {{ display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 8px; }}
  .conf-cell {{ border-radius: 8px; padding: 12px; text-align: center; }}
  .conf-cell .cv {{ font-size: 1.5rem; font-weight: 800; }}
  .conf-cell .cl {{ font-size: 0.72rem; margin-top: 2px; }}
  .tp {{ background: #14532d55; border: 1px solid #22c55e44; }} .tp .cv {{ color: #4ade80; }}
  .tn {{ background: #1d4ed855; border: 1px solid #3b82f644; }} .tn .cv {{ color: #60a5fa; }}
  .fp {{ background: #92400e55; border: 1px solid #f59e0b44; }} .fp .cv {{ color: #facc15; }}
  .fn {{ background: #7f1d1d55; border: 1px solid #ef444444; }} .fn .cv {{ color: #f87171; }}
  .er {{ background: #4c1d9555; border: 1px solid #a78bfa44; }} .er .cv {{ color: #c4b5fd; }}
  .bar-container {{ margin-top: 8px; }}
  .bar-label {{ display: flex; justify-content: space-between; font-size: 0.78rem; color: #94a3b8; margin-bottom: 4px; }}
  .bar-track {{ background: #0f172a; border-radius: 99px; height: 8px; overflow: hidden; }}
  .bar-fill {{ height: 100%; border-radius: 99px; transition: width 0.3s; }}
  .footer {{ text-align: center; padding: 32px 20px; color: #334155; font-size: 0.8rem; border-top: 1px solid #1e293b; margin-top: 16px; }}
  .timestamp {{ color: #475569; font-size: 0.78rem; margin-top: 4px; }}
</style>
</head>
<body>
<div class="header">
  <h1>Rinha de Backend <span>2026</span></h1>
  <p>Fraud Detection — Local Benchmark Report</p>
</div>
<div class="container">

  <div class="final-score-card">
    <div class="final-score-label">Final Score</div>
    <div class="final-score-value">{final_score:.2}</div>
    <div class="final-score-sub">Range: −6000 to +6000 &nbsp;·&nbsp; {n} queries &nbsp;·&nbsp; {elapsed:.1}s total</div>
    <div class="participant-info">
      <div class="badge"><strong>👤</strong> {participant}</div>
      <div class="badge"><strong>🛠</strong> {stack}</div>
      <div class="badge"><strong>🕐</strong> {now}</div>
    </div>
  </div>

  <div class="stat-grid">
    <div class="stat-box">
      <div class="s-val" style="color:#4ade80">{sp99:.2}</div>
      <div class="s-lbl">score_p99{latency_cut_tag}</div>
    </div>
    <div class="stat-box">
      <div class="s-val" style="color:#60a5fa">{sdet:.2}</div>
      <div class="s-lbl">score_det{det_cut_tag}</div>
    </div>
    <div class="stat-box">
      <div class="s-val" style="color:#f59e0b">{p99:.2}ms</div>
      <div class="s-lbl">p99 latency</div>
    </div>
    <div class="stat-box">
      <div class="s-val" style="color:#e2e8f0">{accuracy:.2}%</div>
      <div class="s-lbl">accuracy ({matches}/{n})</div>
    </div>
  </div>

  <div class="grid2">
    <div class="card">
      <div class="card-title">Latency Breakdown</div>
      <div class="score-row">
        <span class="label">p50 (median)</span>
        <span class="value neutral">{p50:.3}ms</span>
      </div>
      <div class="score-row">
        <span class="label">p99</span>
        <span class="value {lat_class}">{p99:.3}ms</span>
      </div>
      <div class="score-row">
        <span class="label">avg</span>
        <span class="value neutral">{avg:.3}ms</span>
      </div>
      <div class="score-row">
        <span class="label">score_p99</span>
        <span class="value {p99_class}">{sp99:.2}</span>
      </div>
      <div class="score-row">
        <span class="label">Cutoff triggered</span>
        <span class="value {lat_cut_class}">{lat_cut}</span>
      </div>
      <div class="bar-container" style="margin-top:16px">
        <div class="bar-label"><span>p99 vs target (1ms)</span><span>{p99:.1}ms</span></div>
        <div class="bar-track"><div class="bar-fill" style="width:{lat_bar}%;background:{lat_bar_color}"></div></div>
      </div>
    </div>

    <div class="card">
      <div class="card-title">Detection Score Breakdown</div>
      <div class="score-row">
        <span class="label">Failure rate</span>
        <span class="value {fr_class}">{failure_rate:.3}%</span>
      </div>
      <div class="score-row">
        <span class="label">Weighted errors (E)</span>
        <span class="value neutral">{e:.0}</span>
      </div>
      <div class="score-row">
        <span class="label">ε = E/N</span>
        <span class="value neutral">{eps:.6}</span>
      </div>
      <div class="score-row">
        <span class="label">Rate component</span>
        <span class="value neutral">{rate_str}</span>
      </div>
      <div class="score-row">
        <span class="label">Absolute penalty</span>
        <span class="value neutral">{pen_str}</span>
      </div>
      <div class="score-row">
        <span class="label">score_det</span>
        <span class="value {det_class}">{sdet:.2}</span>
      </div>
    </div>
  </div>

  <div class="grid2">
    <div class="card">
      <div class="card-title">Confusion Matrix</div>
      <div class="confusion">
        <div class="conf-cell tp"><div class="cv">{tp}</div><div class="cl">True Positive (fraud denied ✓)</div></div>
        <div class="conf-cell tn"><div class="cv">{tn}</div><div class="cl">True Negative (legit approved ✓)</div></div>
        <div class="conf-cell fp"><div class="cv">{fp}</div><div class="cl">False Positive (legit denied ✗)</div></div>
        <div class="conf-cell fn"><div class="cv">{fn_}</div><div class="cl">False Negative (fraud approved ✗)</div></div>
      </div>
      {err_cell}
    </div>

    <div class="card">
      <div class="card-title">Dataset</div>
      <div class="score-row">
        <span class="label">Total requests</span>
        <span class="value neutral">{n}</span>
      </div>
      <div class="score-row">
        <span class="label">Fraud transactions</span>
        <span class="value warning">{fraud_total} ({fraud_pct:.1}%)</span>
      </div>
      <div class="score-row">
        <span class="label">Legit transactions</span>
        <span class="value" style="color:#60a5fa">{legit_total} ({legit_pct:.1}%)</span>
      </div>
      <div class="score-row">
        <span class="label">FP weight (× 1)</span>
        <span class="value neutral">{fp} → {fp_w:.0}</span>
      </div>
      <div class="score-row">
        <span class="label">FN weight (× 3)</span>
        <span class="value neutral">{fn_} → {fn_w:.0}</span>
      </div>
      <div class="score-row">
        <span class="label">Err weight (× 5)</span>
        <span class="value neutral">{err} → {err_w:.0}</span>
      </div>
    </div>
  </div>

</div>
<div class="footer">
  Rinha de Backend 2026 · Local Benchmark Report<br>
  <span class="timestamp">Generated at {now} · {n} total requests</span>
</div>
</body>
</html>"#,
        fc = fc,
        final_score = final_score,
        n = n,
        elapsed = elapsed,
        participant = participant,
        stack = stack,
        now = now,
        sp99 = sp99,
        sdet = sdet,
        p99 = p99,
        accuracy = accuracy,
        matches = matches,
        latency_cut_tag = if lat_cut {
            r#"<span class="cut-tag">CUT</span>"#
        } else {
            r#"<span class="ok-tag">OK</span>"#
        },
        det_cut_tag = if det_cut {
            r#"<span class="cut-tag">CUT</span>"#
        } else {
            r#"<span class="ok-tag">OK</span>"#
        },
        p50 = p50,
        avg = avg,
        lat_class = if p99 < 10.0 {
            "positive"
        } else if p99 < 100.0 {
            "warning"
        } else {
            "negative"
        },
        p99_class = if sp99 >= 2000.0 {
            "positive"
        } else if sp99 >= 0.0 {
            "warning"
        } else {
            "negative"
        },
        lat_cut = lat_cut,
        lat_cut_class = if lat_cut { "negative" } else { "positive" },
        lat_bar = ((p99 / 2000.0) * 100.0).min(100.0) as u32,
        lat_bar_color = if p99 < 10.0 {
            "#4ade80"
        } else if p99 < 100.0 {
            "#facc15"
        } else {
            "#f87171"
        },
        fr_class = if failure_rate < 1.0 {
            "positive"
        } else if failure_rate < 15.0 {
            "warning"
        } else {
            "negative"
        },
        e = e,
        eps = eps,
        rate_str = rate_str,
        pen_str = pen_str,
        det_class = if sdet >= 2000.0 {
            "positive"
        } else if sdet >= 0.0 {
            "warning"
        } else {
            "negative"
        },
        tp = sc.tp,
        tn = sc.tn,
        fp = sc.fp,
        fn_ = sc.fn_,
        failure_rate = failure_rate,
        err_cell = if sc.err > 0 {
            format!(
                r#"<div style="margin-top:8px"><div class="conf-cell er" style="grid-column:1/-1"><div class="cv">{}</div><div class="cl">HTTP Errors (weight × 5)</div></div></div>"#,
                sc.err
            )
        } else {
            String::new()
        },
        fraud_total = fraud_total,
        legit_total = legit_total,
        fraud_pct = fraud_total as f64 / n as f64 * 100.0,
        legit_pct = legit_total as f64 / n as f64 * 100.0,
        fp_w = sc.fp as f64,
        fn_w = sc.fn_ as f64 * 3.0,
        err_w = sc.err as f64 * 5.0,
        err = sc.err,
    );

    std::fs::write(path, html).expect("failed to write HTML report");
}
