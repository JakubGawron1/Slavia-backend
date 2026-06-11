//! Sinclair 2025–2028 (IWF) — stałe zgodne z `@slavia/shared` / test-vectors/sinclair.json.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinclairGender {
    Male,
    Female,
}

impl SinclairGender {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("male") | Some("m") => Some(Self::Male),
            Some("female") | Some("f") => Some(Self::Female),
            _ => None,
        }
    }
}

const MALE_A: f64 = 0.7023570715147177;
const MALE_B: f64 = 201.0;
const FEMALE_A: f64 = 0.6734030019259942;
const FEMALE_B: f64 = 164.0;

pub fn sinclair_coefficient(bodyweight_kg: f64, gender: SinclairGender) -> f64 {
    if !bodyweight_kg.is_finite() || bodyweight_kg <= 0.0 {
        return f64::NAN;
    }
    let (a, b) = match gender {
        SinclairGender::Male => (MALE_A, MALE_B),
        SinclairGender::Female => (FEMALE_A, FEMALE_B),
    };
    if bodyweight_kg >= b {
        return 1.0;
    }
    let log_ratio = (bodyweight_kg / b).log10();
    10f64.powf(a * log_ratio * log_ratio)
}

pub fn sinclair_total(
    competition_total_kg: f64,
    bodyweight_kg: f64,
    gender: SinclairGender,
) -> f64 {
    if !competition_total_kg.is_finite() || competition_total_kg <= 0.0 {
        return f64::NAN;
    }
    let c = sinclair_coefficient(bodyweight_kg, gender);
    if c.is_nan() {
        return f64::NAN;
    }
    competition_total_kg * c
}

#[derive(Debug, Serialize)]
pub struct SinclairRankingRow {
    pub athlete_id: String,
    pub full_name: String,
    pub gender: Option<String>,
    pub bodyweight_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub sinclair_total: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SinclairVectorCase {
    total: f64,
    bodyweight: f64,
    gender: String,
    #[serde(rename = "expectedTotal")]
    expected_total: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SinclairVectorFile {
    cases: Vec<SinclairVectorCase>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_shared_test_vectors() {
        const JSON: &str = include_str!("embed/sinclair-test-vectors.json");
        let file: SinclairVectorFile = serde_json::from_str(JSON).expect("parse vectors");
        for case in file.cases {
            let gender = SinclairGender::parse(Some(&case.gender)).expect("gender");
            let got = sinclair_total(case.total, case.bodyweight, gender);
            match case.expected_total {
                Some(expected) => assert!(
                    (got - expected).abs() < 0.01,
                    "total={} bw={} gender={}: got {got}, want {expected}",
                    case.total,
                    case.bodyweight,
                    case.gender,
                ),
                None => assert!(got.is_nan(), "expected NaN for {:?}", case),
            }
        }
    }
}
