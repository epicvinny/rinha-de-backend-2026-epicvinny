use flate2::read::GzDecoder;
use rayon::prelude::*;
use serde::Deserialize;
use shared::types::Reference;
use shared::{l2sq_scalar, quantize, vectorize_round4, Constants, TopK};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;

#[derive(Deserialize)]
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("Usage: oracle --refs <refs.json.gz> --queries <queries.json> --norm <norm.json> --mcc <mcc.json> --out <output.csv>");
        std::process::exit(1);
    }

    let mut refs_path = PathBuf::new();
    let mut queries_path = PathBuf::new();
    let mut norm_path = PathBuf::new();
    let mut mcc_path = PathBuf::new();
    let mut out_path = PathBuf::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--refs" => {
                refs_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--queries" => {
                queries_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--norm" => {
                norm_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--mcc" => {
                mcc_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--out" => {
                out_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let constants = Constants::load(&norm_path, &mcc_path);

    eprintln!("Loading references...");
    let f = File::open(&refs_path).expect("failed to open refs file");
    let gz = GzDecoder::new(f);
    let reader = BufReader::new(gz);
    let refs: Vec<Reference> = serde_json::from_reader(reader).expect("failed to parse refs");
    eprintln!("Loaded {} references", refs.len());

    // Quantize all references upfront
    let ref_vecs: Vec<[i16; 16]> = refs
        .iter()
        .map(|r| {
            let v14: [f64; 14] = r.vector[..14].try_into().expect("bad vector length");
            quantize(&v14)
        })
        .collect();

    let ref_labels: Vec<u8> = refs
        .iter()
        .map(|r| if r.label == "fraud" { 1u8 } else { 0u8 })
        .collect();

    eprintln!("Loading queries...");
    let queries_data = std::fs::read_to_string(&queries_path).expect("failed to read queries");
    let queries: Vec<QueryEntry<'_>> = if queries_data.trim_start().starts_with('[') {
        serde_json::from_str(&queries_data).expect("failed to parse queries array")
    } else {
        let td: TestData<'_> =
            serde_json::from_str(&queries_data).expect("failed to parse queries object");
        td.entries
    };
    eprintln!("Loaded {} queries", queries.len());

    // Output CSV
    let mut out = csv::Writer::from_path(&out_path).expect("failed to create output CSV");
    out.write_record(&[
        "query_id",
        "fraud_count",
        "fraud_score",
        "approved",
        "expected_approved",
        "expected_fraud_score",
        "match",
    ])
    .unwrap();

    eprintln!("Running oracle...");
    let total = queries.len();
    let mut match_count = 0usize;

    for (idx, q) in queries.iter().enumerate() {
        if idx % 5000 == 0 {
            eprintln!("  {}/{}", idx, total);
        }

        let qv = vectorize_round4(&q.request, &constants);
        let qq = quantize(&qv);

        // Brute-force top-5
        let mut topk = TopK::new();
        for (id, rv) in ref_vecs.iter().enumerate() {
            let d = l2sq_scalar(&qq, rv);
            topk.insert(d, id as u32);
        }

        let ids = topk.ids();
        let fraud_count = ids
            .iter()
            .filter(|&&id| id != u32::MAX && ref_labels[id as usize] == 1)
            .count();
        let fraud_score_raw = fraud_count as f64 / 5.0;
        let fraud_score = shared::round4(fraud_score_raw);
        let approved = fraud_score < 0.6;

        let is_match =
            approved == q.expected_approved && (fraud_score - q.expected_fraud_score).abs() < 1e-9;
        if is_match {
            match_count += 1;
        }

        let fraud_count_str = fraud_count.to_string();
        let approved_str = approved.to_string();
        let expected_approved_str = q.expected_approved.to_string();
        let is_match_str = is_match.to_string();
        let f_score_str = shared::fraud_score_to_string(fraud_score);
        let exp_f_score_str = shared::fraud_score_to_string(q.expected_fraud_score);

        out.write_record(&[
            q.request.id,
            &fraud_count_str,
            &f_score_str,
            &approved_str,
            &expected_approved_str,
            &exp_f_score_str,
            &is_match_str,
        ])
        .unwrap();
    }

    out.flush().unwrap();
    eprintln!(
        "Done. Matches: {}/{} ({:.2}%)",
        match_count,
        total,
        match_count as f64 / total as f64 * 100.0
    );
    if match_count < total {
        eprintln!("WARNING: {} mismatches!", total - match_count);
        std::process::exit(1);
    }
}
