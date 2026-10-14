//! Energinet's Eloverblik customer API and the time series math. The same
//! as in TheHome's built-in `eloverblik`.
//!
//! A refresh token (made by the user on eloverblik.dk, valid one year) is
//! exchanged for a data access token (valid 24 hours), which is used to list
//! metering points and fetch hourly time series.

use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as _};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const BASE_URL: &str = "https://api.eloverblik.dk/customerapi/api";
pub const TZ: chrono_tz::Tz = chrono_tz::Europe::Copenhagen;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("adgangen blev afvist (forkert eller udløbet token)")]
    Auth,
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeteringPoint {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub address: String,
}

/// Hourly consumption: hour start (UTC) -> kWh.
pub type Series = BTreeMap<DateTime<Utc>, f64>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("TheHome-eloverblik/", env!("CARGO_PKG_VERSION")))
            .timeout(StdDuration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { http, base: base.trim_end_matches('/').to_string() }
    }

    async fn check(resp: reqwest::Response) -> Result<Value, ApiError> {
        let s = resp.status();
        if s.as_u16() == 401 || s.as_u16() == 403 {
            return Err(ApiError::Auth);
        }
        if !s.is_success() {
            return Err(anyhow!("Eloverblik: HTTP {s}").into());
        }
        Ok(resp.json().await.context("Eloverblik: ugyldig JSON")?)
    }

    /// Exchange the refresh token for a data access token.
    pub async fn access_token(&self, refresh_token: &str) -> Result<String, ApiError> {
        let resp = self
            .http
            .get(format!("{}/token", self.base))
            .bearer_auth(refresh_token)
            .send()
            .await
            .map_err(|e| anyhow!("Eloverblik: {}", e.without_url()))?;
        let v = Self::check(resp).await?;
        v["result"].as_str().map(String::from).ok_or_else(|| anyhow!("Eloverblik: token mangler i svar").into())
    }

    pub async fn metering_points(&self, token: &str) -> Result<Vec<MeteringPoint>, ApiError> {
        let resp = self
            .http
            .get(format!("{}/meteringpoints/meteringpoints", self.base))
            .query(&[("includeAll", "false")])
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| anyhow!("Eloverblik: {}", e.without_url()))?;
        let v = Self::check(resp).await?;
        Ok(parse_metering_points(&v))
    }

    pub async fn hourly(&self, token: &str, ids: &[String], from: NaiveDate, to: NaiveDate) -> Result<BTreeMap<String, Series>, ApiError> {
        let resp = self
            .http
            .post(format!("{}/meterdata/gettimeseries/{}/{}/Hour", self.base, from.format("%Y-%m-%d"), to.format("%Y-%m-%d")))
            .bearer_auth(token)
            .json(&json!({ "meteringPoints": { "meteringPoint": ids } }))
            .send()
            .await
            .map_err(|e| anyhow!("Eloverblik: {}", e.without_url()))?;
        let v = Self::check(resp).await?;
        parse_timeseries(&v).map_err(ApiError::Other)
    }
}

pub fn parse_metering_points(v: &Value) -> Vec<MeteringPoint> {
    v["result"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["meteringPointId"].as_str()?.to_string();
            let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
            let address = format!("{} {}, {} {}", s("streetName"), s("buildingNumber"), s("postcode"), s("cityName"))
                .trim_matches(|c| c == ' ' || c == ',')
                .to_string();
            Some(MeteringPoint { id, kind: s("typeOfMP"), address })
        })
        .collect()
}

fn resolution(s: &str) -> Option<Duration> {
    match s {
        "PT15M" => Some(Duration::minutes(15)),
        "PT1H" => Some(Duration::hours(1)),
        "P1D" => Some(Duration::days(1)),
        _ => None,
    }
}

/// Parse a `gettimeseries` response into hourly series per metering point.
/// Quarter-hour points are summed into hours.
pub fn parse_timeseries(v: &Value) -> anyhow::Result<BTreeMap<String, Series>> {
    let mut out: BTreeMap<String, Series> = BTreeMap::new();
    for r in v["result"].as_array().into_iter().flatten() {
        if r["success"].as_bool() == Some(false) {
            crate::warn!("{}: {}", r["id"], r["errorText"].as_str().unwrap_or("fejl"));
            continue;
        }
        for ts in r["MyEnergyData_MarketDocument"]["TimeSeries"].as_array().into_iter().flatten() {
            let id = ts["mRID"].as_str().or_else(|| r["id"].as_str()).unwrap_or_default().to_string();
            let series = out.entry(id).or_default();
            for p in ts["Period"].as_array().into_iter().flatten() {
                let start = p["timeInterval"]["start"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                    .context("timeInterval.start")?;
                let step = p["resolution"].as_str().and_then(resolution).unwrap_or(Duration::hours(1));
                for pt in p["Point"].as_array().into_iter().flatten() {
                    let pos: i32 = pt["position"].as_str().and_then(|s| s.parse().ok()).or_else(|| pt["position"].as_i64().map(|n| n as i32)).unwrap_or(1);
                    let qty = pt["out_Quantity.quantity"].as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| pt["out_Quantity.quantity"].as_f64());
                    let Some(qty) = qty else { continue };
                    let t = start + step * (pos - 1);
                    let hour = t.with_minute(0).unwrap_or(t);
                    *series.entry(hour).or_insert(0.0) += qty;
                }
            }
        }
    }
    Ok(out)
}

/// Persistent running total per metering point.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Total {
    pub kwh: f64,
    /// Start of the last hour included in `kwh`.
    pub last_hour: Option<DateTime<Utc>>,
}

impl Total {
    /// Add hours after `last_hour`. Only whole local days are added so a day
    /// that is still arriving is not counted twice.
    pub fn absorb(&mut self, series: &Series) -> usize {
        let mut by_day: BTreeMap<NaiveDate, Vec<(DateTime<Utc>, f64)>> = BTreeMap::new();
        for (t, v) in series {
            if self.last_hour.is_none_or(|l| *t > l) {
                by_day.entry(t.with_timezone(&TZ).date_naive()).or_default().push((*t, *v));
            }
        }
        let mut added = 0;
        for (day, hours) in by_day {
            let expected = day_hours(day);
            if hours.len() < expected {
                break;
            }
            for (t, v) in hours {
                self.kwh += v;
                self.last_hour = Some(t);
                added += 1;
            }
        }
        added
    }
}

/// Number of hours in a local day (23/24/25).
pub fn day_hours(day: NaiveDate) -> usize {
    let start = TZ.from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default()).earliest();
    let end = day.succ_opt().and_then(|d| TZ.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default()).earliest());
    match (start, end) {
        (Some(s), Some(e)) => (e - s).num_hours() as usize,
        _ => 24,
    }
}

/// The hours of one day: hour start (UTC) and kWh.
pub type DayHours = Vec<(DateTime<Utc>, f64)>;

/// Consumption for one local day, if complete.
pub fn day_total(series: &Series, day: NaiveDate) -> Option<(f64, DayHours)> {
    let hours: DayHours = series
        .iter()
        .filter(|(t, _)| t.with_timezone(&TZ).date_naive() == day)
        .map(|(t, v)| (*t, *v))
        .collect();
    if hours.len() < day_hours(day) {
        return None;
    }
    Some((hours.iter().map(|(_, v)| v).sum(), hours))
}
