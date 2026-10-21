//! Meter data from Energinet's Eloverblik as an external integration for
//! HomewrightHQ.
//!
//! Data arrives about a day late, so the sensors describe yesterday and a
//! running total, and the hours go into a long-term statistic of their own
//! for the Energi page. `api` started as HomewrightHQ's built-in `eloverblik`;
//! this file connects it to HomewrightHQ through the `thehome-plugin` SDK.

pub mod api;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use thehome_plugin::{async_trait, json, Entry, Error, Flow, FlowReply, Host, Integration, Map, Value};

pub use api::{ApiError, Client, MeteringPoint, BASE_URL, TZ};

pub const DOMAIN: &str = "eloverblik_ext";
const POLL: StdDuration = StdDuration::from_secs(3 * 3600);
const DAYS_BACK: i64 = 7;
/// History fetched the first time, for the Energi page.
const BACKFILL_DAYS: i64 = 365;
/// Days per request while fetching history.
const BACKFILL_CHUNK: i64 = 92;

/// Logging to HomewrightHQ's log through stderr.
#[doc(hidden)]
pub fn emit(level: &str, message: String) {
    thehome_plugin::log(level, message);
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::emit("INFO", format!($($t)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::emit("WARN", format!($($t)*)) };
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn token_schema() -> Value {
    json!({
        "type": "object",
        "required": ["refresh_token"],
        "properties": {
            "refresh_token": {
                "type": "string", "format": "password", "title": "Token fra Eloverblik",
                "description": "Opret et token på eloverblik.dk under 'Datadeling'"
            }
        }
    })
}

fn points_of(v: &Value) -> Vec<MeteringPoint> {
    v.as_array().into_iter().flatten().filter_map(|p| serde_json::from_value(p.clone()).ok()).collect()
}

#[derive(Default)]
pub struct Eloverblik {
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Eloverblik {
    fn finish(data: &Map<String, Value>, points: Vec<MeteringPoint>) -> FlowReply {
        let mut ids: Vec<String> = points.iter().map(|p| p.id.clone()).collect();
        ids.sort();
        let mut data = data.clone();
        data.insert("metering_points".into(), serde_json::to_value(&points).unwrap_or(Value::Null));
        let title = if points.len() == 1 && !points[0].address.is_empty() {
            format!("Eloverblik {}", points[0].address)
        } else {
            "Eloverblik".into()
        };
        FlowReply::create(&title, Value::Object(data)).with_unique_id(ids.join(","))
    }
}

#[async_trait]
impl Integration for Eloverblik {
    async fn config_flow(&self, _host: &Host, flow: Flow, step: &str, input: Option<Value>) -> Result<FlowReply, Error> {
        match (step, input) {
            ("user", None) | ("reauth", None) => Ok(FlowReply::form("user", token_schema())),
            ("user", Some(v)) => {
                let token = v["refresh_token"].as_str().unwrap_or_default().trim().to_string();
                let base = flow.data.get("base_url").and_then(Value::as_str).unwrap_or(BASE_URL).to_string();
                let client = Client::new(&base);
                let points = async {
                    let access = client.access_token(&token).await?;
                    client.metering_points(&access).await
                }
                .await;
                let points = match points {
                    Ok(p) if !p.is_empty() => p,
                    Ok(_) => return Ok(FlowReply::abort("no_metering_points")),
                    Err(e) => {
                        info!("{e}");
                        let code = if matches!(e, ApiError::Auth) { "invalid_auth" } else { "cannot_connect" };
                        return Ok(FlowReply::form_with_errors("user", token_schema(), json!({ "base": code })));
                    }
                };
                if flow.source == "reauth" {
                    let mut data = flow.entry.as_ref().and_then(|e| e.data.as_object().cloned()).unwrap_or_default();
                    data.insert("refresh_token".into(), json!(token));
                    return Ok(FlowReply::create("", Value::Object(data)));
                }
                let mut data = flow.data.clone();
                data.insert("refresh_token".into(), json!(token));
                if points.len() == 1 {
                    return Ok(Self::finish(&data, points));
                }
                let mut options = vec!["alle".to_string()];
                options.extend(points.iter().map(|p| p.id.clone()));
                data.insert("_points".into(), serde_json::to_value(&points).unwrap_or(Value::Null));
                let names: Vec<String> = points.iter().map(|p| format!("{} ({})", p.id, p.address)).collect();
                let form = json!({
                    "type": "form",
                    "step_id": "select",
                    "schema": { "type": "object", "required": ["metering_point"], "properties": {
                        "metering_point": { "type": "string", "title": "Målepunkt", "enum": options, "default": "alle" } } },
                    "description": format!("Vælg målepunkt: {}", names.join("; ")),
                });
                Ok(FlowReply { result: form, data: Some(data), unique_id: None })
            }
            ("select", Some(v)) => {
                let mut data = flow.data.clone();
                let points = data.remove("_points").map(|p| points_of(&p)).unwrap_or_default();
                let choice = v["metering_point"].as_str().unwrap_or("alle");
                let chosen: Vec<MeteringPoint> = points.into_iter().filter(|p| choice == "alle" || p.id == choice).collect();
                if chosen.is_empty() {
                    return Err(Error::invalid("ukendt målepunkt"));
                }
                Ok(Self::finish(&data, chosen))
            }
            (s, _) => Err(Error::invalid(format!("ukendt trin {s}"))),
        }
    }

    async fn setup_entry(&self, host: &Host, entry: Entry) -> Result<(), Error> {
        let entry_id = entry.entry_id.clone();
        let token = entry.get_str("refresh_token").ok_or_else(|| Error::auth_failed("token mangler"))?.to_string();
        let client = Client::new(entry.get_str("base_url").unwrap_or(BASE_URL));
        let points = points_of(&entry.data["metering_points"]);
        if points.is_empty() {
            return Err(Error::failed("ingen målepunkter"));
        }
        let access = match client.access_token(&token).await {
            Ok(a) => a,
            Err(ApiError::Auth) => return Err(Error::auth_failed("Eloverblik afviste tokenet")),
            Err(e) => return Err(Error::not_ready(e)),
        };

        let mut meters = vec![];
        for p in &points {
            let device = json!({
                "identifiers": [[DOMAIN, p.id]],
                "name": if p.address.is_empty() { format!("Elmåler {}", p.id) } else { format!("Elmåler {}", p.address) },
                "manufacturer": "Energinet",
                "model": format!("Målepunkt {}", p.kind),
                "serial_number": p.id,
                "entry_type": "service",
            });
            let dev = host.request("device/register", json!({ "entry_id": entry_id, "device": device })).await?;
            let dev_id = dev["id"].as_str().ok_or_else(|| Error::failed("intet enheds-id"))?.to_string();
            // Through `request`, since the SDK's Entity has no suggested_object_id.
            // The ids match the built-in integration's.
            let add = |key: &'static str, name: &'static str, state_class: &'static str| {
                let entity = json!({
                    "domain": "sensor", "unique_id": format!("{}-{key}", p.id), "name": name, "device_id": dev_id,
                    "device_class": "energy", "unit_of_measurement": "kWh", "state_class": state_class,
                    "suggested_object_id": format!("eloverblik_{}_{key}", &p.id[p.id.len().saturating_sub(6)..]),
                });
                let params = json!({ "entry_id": entry_id, "entity": entity, "handler": false });
                async move { host.request("entity/add", params).await.map(|v| v["entity_id"].as_str().map(String::from)) }
            };
            let yesterday = add("yesterday", "Forbrug i går", "total").await?;
            let total = add("total", "Samlet forbrug", "total_increasing").await?;
            let name = if p.address.is_empty() { format!("Eloverblik {}", p.id) } else { format!("Eloverblik {}", p.address) };
            meters.push(Meter {
                id: p.id.clone(),
                name,
                yesterday,
                total,
                store: host.data_dir().join(format!("total-{entry_id}-{}.json", p.id)),
                history: host.data_dir().join(format!("hours-{entry_id}-{}.json", p.id)),
            });
        }

        let host = host.clone();
        let id = entry_id.clone();
        let task = tokio::spawn(async move {
            let mut access = Some((access, Instant::now()));
            // The statistics are imported once per start, in case HomewrightHQ lost them.
            let mut imported = false;
            loop {
                // Data access tokens live 24 hours; renew after 20.
                if access.as_ref().is_some_and(|(_, at)| at.elapsed() > StdDuration::from_secs(20 * 3600)) {
                    access = None;
                }
                if access.is_none() {
                    match client.access_token(&token).await {
                        Ok(a) => access = Some((a, Instant::now())),
                        Err(ApiError::Auth) => {
                            let _ = host
                                .create_issue(
                                    &id,
                                    "auth",
                                    "error",
                                    "Eloverblik: token afvist",
                                    "Tokenet er udløbet eller tilbagekaldt. Opret et nyt på eloverblik.dk og godkend integrationen igen.",
                                )
                                .await;
                        }
                        Err(e) => warn!("{e}"),
                    }
                }
                if let Some((a, _)) = &access {
                    match update(&host, &client, a, &meters, Utc::now(), !imported).await {
                        Ok(()) => {
                            imported = true;
                            let _ = host.delete_issue(&id, "auth").await;
                        }
                        Err(ApiError::Auth) => access = None,
                        Err(e) => warn!("{e}"),
                    }
                }
                tokio::time::sleep(POLL).await;
            }
        });
        if let Some(old) = self.tasks.lock().unwrap().insert(entry_id, task) {
            old.abort();
        }
        Ok(())
    }

    async fn unload_entry(&self, _host: &Host, entry_id: &str) -> Result<(), Error> {
        if let Some(t) = self.tasks.lock().unwrap().remove(entry_id) {
            t.abort();
        }
        Ok(())
    }

    async fn diagnostics(&self, _host: &Host, entry: Entry) -> Value {
        let mut d = entry.data.clone();
        if let Value::Object(m) = &mut d {
            m.insert("refresh_token".into(), json!("**REDACTED**"));
        }
        d
    }
}

struct Meter {
    id: String,
    /// The statistic's name on the Energi page.
    name: String,
    yesterday: Option<String>,
    total: Option<String>,
    /// Where the running total is kept between restarts.
    store: PathBuf,
    /// Where the hours behind the statistic are kept.
    history: PathBuf,
}

/// The statistic a metering point's hours go into.
pub fn statistic_id(metering_point: &str) -> String {
    format!("{DOMAIN}:{metering_point}")
}

fn load<T: serde::de::DeserializeOwned + Default>(path: &PathBuf) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// The year before the last week, for meters that have not had it yet. A
/// failed request is tried again at the next poll.
async fn backfill(client: &Client, access: &str, meters: &[Meter], histories: &mut [api::History], today: NaiveDate) -> Result<bool, ApiError> {
    let ids: Vec<String> = meters.iter().zip(histories.iter()).filter(|(_, h)| !h.backfilled).map(|(m, _)| m.id.clone()).collect();
    if ids.is_empty() {
        return Ok(false);
    }
    let end = today - Duration::days(DAYS_BACK);
    let mut from = today - Duration::days(BACKFILL_DAYS);
    while from < end {
        let to = (from + Duration::days(BACKFILL_CHUNK)).min(end);
        let data = client.hourly(access, &ids, from, to).await?;
        for (m, h) in meters.iter().zip(histories.iter_mut()) {
            if let Some(series) = data.get(&m.id).filter(|_| ids.contains(&m.id)) {
                h.absorb(series);
            }
        }
        from = to;
    }
    for h in histories.iter_mut() {
        h.backfilled = true;
    }
    info!("hentede et års timer for {} målepunkter", ids.len());
    Ok(true)
}

async fn update(host: &Host, client: &Client, access: &str, meters: &[Meter], now: DateTime<Utc>, import_all: bool) -> Result<(), ApiError> {
    let today = now.with_timezone(&TZ).date_naive();
    let from = today - Duration::days(DAYS_BACK);
    let ids: Vec<String> = meters.iter().map(|m| m.id.clone()).collect();
    let data = client.hourly(access, &ids, from, today).await?;
    let mut histories: Vec<api::History> = meters.iter().map(|m| load(&m.history)).collect();
    let backfilled = match backfill(client, access, meters, &mut histories, today).await {
        Ok(b) => b,
        Err(ApiError::Auth) => return Err(ApiError::Auth),
        Err(e) => {
            warn!("historikken kunne ikke hentes, prøver igen senere: {e}");
            false
        }
    };
    for (m, history) in meters.iter().zip(histories.iter_mut()) {
        let new_hours = data.get(&m.id).is_some_and(|s| history.absorb(s));
        if new_hours || backfilled {
            if let Err(e) = save(&m.history, history) {
                warn!("kunne ikke gemme timerne: {e}");
            }
        }
        if (new_hours || backfilled || import_all) && !history.hours.is_empty() {
            let params = json!({
                "statistic_id": statistic_id(&m.id), "name": m.name, "unit_of_measurement": "kWh",
                "has_sum": true, "has_mean": false, "stats": history.rows(),
            });
            if let Err(e) = host.request("statistics/import", params).await {
                warn!("statistikken kunne ikke gemmes: {e}");
            }
        }
        let Some(series) = data.get(&m.id) else { continue };
        let yday = today.pred_opt().unwrap_or(today);
        if let Some(id) = &m.yesterday {
            // Fall back to the latest complete day if yesterday is not in yet.
            let day = (0..DAYS_BACK).map(|i| yday - Duration::days(i)).find(|d| api::day_total(series, *d).is_some());
            if let Some(day) = day {
                let (kwh, hours) = api::day_total(series, day).unwrap_or_default();
                let mut a = Map::new();
                let start = TZ.from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default()).earliest().map(|d| d.with_timezone(&Utc));
                a.insert("last_reset".into(), json!(start.map(|s| s.to_rfc3339())));
                a.insert("date".into(), json!(day.to_string()));
                a.insert("is_yesterday".into(), json!(day == yday));
                a.insert(
                    "hourly".into(),
                    Value::Array(hours.iter().map(|(t, v)| json!({ "start": t.with_timezone(&TZ).to_rfc3339(), "kwh": round3(*v) })).collect()),
                );
                if let Err(e) = host.set_state(id, format!("{kwh:.3}"), a).await {
                    warn!("kunne ikke opdatere {id}: {e}");
                }
            }
        }
        if let Some(id) = &m.total {
            let mut total: api::Total = load(&m.store);
            if total.absorb(series) > 0 {
                if let Err(e) = save(&m.store, &total) {
                    warn!("kunne ikke gemme total: {e}");
                }
            }
            if total.last_hour.is_some() {
                let mut a = Map::new();
                a.insert("data_until".into(), json!(total.last_hour.map(|t| (t + Duration::hours(1)).with_timezone(&TZ).to_rfc3339())));
                if let Err(e) = host.set_state(id, format!("{:.3}", total.kwh), a).await {
                    warn!("kunne ikke opdatere {id}: {e}");
                }
            }
        }
    }
    Ok(())
}
