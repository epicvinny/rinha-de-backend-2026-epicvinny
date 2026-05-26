use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Constants {
    pub max_amount: f64,
    pub max_installments: f64,
    pub amount_vs_avg_ratio: f64,
    pub max_minutes: f64,
    pub max_km: f64,
    pub max_tx_count_24h: f64,
    pub max_merchant_avg_amount: f64,
    pub mcc_risk: HashMap<String, f64>,
    pub mcc_default: f64,
    pub mcc_risk_i16: [i16; 10000],
}

#[derive(Deserialize)]
struct NormJson {
    max_amount: f64,
    max_installments: f64,
    amount_vs_avg_ratio: f64,
    max_minutes: f64,
    max_km: f64,
    max_tx_count_24h: f64,
    max_merchant_avg_amount: f64,
}

impl Constants {
    pub fn load(norm_path: &Path, mcc_path: &Path) -> Self {
        let norm_data =
            std::fs::read_to_string(norm_path).expect("failed to read normalization.json");
        let norm: NormJson =
            serde_json::from_str(&norm_data).expect("failed to parse normalization.json");

        let mcc_data = std::fs::read_to_string(mcc_path).expect("failed to read mcc_risk.json");
        let mcc_risk: HashMap<String, f64> =
            serde_json::from_str(&mcc_data).expect("failed to parse mcc_risk.json");

        let mut mcc_risk_i16 = [5000i16; 10000];
        for (mcc_str, &risk_val) in &mcc_risk {
            if let Ok(idx) = mcc_str.parse::<usize>() {
                if idx < 10000 {
                    mcc_risk_i16[idx] = (risk_val * 10000.0).round() as i16;
                }
            }
        }

        Constants {
            max_amount: norm.max_amount,
            max_installments: norm.max_installments,
            amount_vs_avg_ratio: norm.amount_vs_avg_ratio,
            max_minutes: norm.max_minutes,
            max_km: norm.max_km,
            max_tx_count_24h: norm.max_tx_count_24h,
            max_merchant_avg_amount: norm.max_merchant_avg_amount,
            mcc_risk,
            mcc_default: 0.5,
            mcc_risk_i16,
        }
    }

    pub fn load_embedded() -> Self {
        let norm_data = r#"{
            "max_amount": 10000,
            "max_installments": 12,
            "amount_vs_avg_ratio": 10,
            "max_minutes": 1440,
            "max_km": 1000,
            "max_tx_count_24h": 20,
            "max_merchant_avg_amount": 10000
        }"#;
        let norm: NormJson = serde_json::from_str(norm_data).unwrap();

        let mcc_data = r#"{
            "5411": 0.15, "5812": 0.30, "5912": 0.20, "5944": 0.45,
            "7801": 0.80, "7802": 0.75, "7995": 0.85, "4511": 0.35,
            "5311": 0.25, "5999": 0.50
        }"#;
        let mcc_risk: HashMap<String, f64> = serde_json::from_str(mcc_data).unwrap();

        let mut mcc_risk_i16 = [5000i16; 10000];
        for (mcc_str, &risk_val) in &mcc_risk {
            if let Ok(idx) = mcc_str.parse::<usize>() {
                if idx < 10000 {
                    mcc_risk_i16[idx] = (risk_val * 10000.0).round() as i16;
                }
            }
        }

        Constants {
            max_amount: norm.max_amount,
            max_installments: norm.max_installments,
            amount_vs_avg_ratio: norm.amount_vs_avg_ratio,
            max_minutes: norm.max_minutes,
            max_km: norm.max_km,
            max_tx_count_24h: norm.max_tx_count_24h,
            max_merchant_avg_amount: norm.max_merchant_avg_amount,
            mcc_risk,
            mcc_default: 0.5,
            mcc_risk_i16,
        }
    }
}
