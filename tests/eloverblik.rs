//! The parsing and total tests started as Homewright's built-in eloverblik;
//! the plugin tests play Homewright over a pipe.

mod host;

use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};
use thehome_eloverblik::api::{day_hours, day_total, parse_metering_points, parse_timeseries, History, Total};
use thehome_eloverblik::{statistic_id, ApiError, Client, Eloverblik, TZ};
use wiremock::matchers::{body_json, header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

const MP: &str = "571313100000000001";

#[test]
fn parse_fixtures() {
    let mps = parse_metering_points(&fixture("meteringpoints.json"));
    assert_eq!(mps.len(), 1);
    assert_eq!(mps[0].id, MP);
    assert_eq!(mps[0].kind, "E17");
    assert_eq!(mps[0].address, "Testvej 1, 2100 København Ø");

    let ts = parse_timeseries(&fixture("timeseries_hour.json")).unwrap();
    let s = &ts[MP];
    assert_eq!(s.len(), 72);
    let first = TZ.with_ymd_and_hms(2026, 10, 8, 0, 0, 0).unwrap().with_timezone(&Utc);
    assert_eq!(*s.keys().next().unwrap(), first);
    let (kwh, hours) = day_total(s, NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()).unwrap();
    assert_eq!(hours.len(), 24);
    // 0.2 + 0.05*i + 0.1*2 for i in 0..24
    let expected: f64 = (0..24).map(|i| 0.4 + 0.05 * f64::from(i)).sum();
    assert!((kwh - expected).abs() < 1e-6);
    assert!(day_total(s, NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()).is_none());
}

#[test]
fn dst_day_hours() {
    assert_eq!(day_hours(NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()), 25);
    assert_eq!(day_hours(NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()), 23);
    assert_eq!(day_hours(NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()), 24);
}

#[test]
fn total_only_grows_with_new_complete_days() {
    let ts = parse_timeseries(&fixture("timeseries_hour.json")).unwrap();
    let s = &ts[MP];
    let all: f64 = s.values().sum();
    let mut t = Total::default();
    assert_eq!(t.absorb(s), 72);
    assert!((t.kwh - all).abs() < 1e-9);
    // Same data again adds nothing.
    assert_eq!(t.absorb(s), 0);
    assert!((t.kwh - all).abs() < 1e-9);
    // A partial new day is held back.
    let mut s2 = s.clone();
    let next = *s.keys().last().unwrap() + chrono::Duration::hours(1);
    s2.insert(next, 1.0);
    assert_eq!(t.absorb(&s2), 0);
}

#[test]
fn history_takes_whole_days_and_corrections() {
    let ts = parse_timeseries(&fixture("timeseries_hour.json")).unwrap();
    let s = &ts[MP];
    let mut h = History::default();
    assert!(h.absorb(s));
    assert_eq!(h.hours.len(), 72);
    assert!(!h.absorb(s), "the same data again changes nothing");

    // A partial day is held back; a correction of a known hour is taken.
    let mut s2 = s.clone();
    let last = *s.keys().last().unwrap();
    s2.insert(last + chrono::Duration::hours(1), 1.0);
    assert!(!h.absorb(&s2));
    let first = *s.keys().next().unwrap();
    s2.insert(first, 9.0);
    assert!(h.absorb(&s2));
    assert_eq!(h.hours[&first], 9.0);

    // A zero the hour before the first, then a running sum.
    let rows = h.rows();
    assert_eq!(rows.len(), 73);
    assert_eq!(rows[0], json!({ "start": (first - chrono::Duration::hours(1)).to_rfc3339(), "sum": 0.0, "state": 0.0 }));
    assert_eq!(rows[1]["sum"], 9.0);
    let all: f64 = h.hours.values().sum();
    assert!((rows[72]["sum"].as_f64().unwrap() - all).abs() < 1e-6);
}

async fn mock_api(server: &MockServer, points: Value) {
    Mock::given(method("GET")).and(path("/token")).and(header("authorization", "Bearer refresh-abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("token.json")))
        .mount(server).await;
    Mock::given(method("GET")).and(path("/token"))
        .respond_with(ResponseTemplate::new(401))
        .mount(server).await;
    Mock::given(method("GET")).and(path("/meteringpoints/meteringpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(points))
        .mount(server).await;
    Mock::given(method("POST")).and(path_regex(r"^/meterdata/gettimeseries/\d{4}-\d{2}-\d{2}/\d{4}-\d{2}-\d{2}/Hour$"))
        .and(body_json(json!({ "meteringPoints": { "meteringPoint": [MP] } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("timeseries_hour.json")))
        .mount(server).await;
}

#[tokio::test]
async fn client() {
    let server = MockServer::start().await;
    mock_api(&server, fixture("meteringpoints.json")).await;
    let c = Client::new(&server.uri());
    assert!(matches!(c.access_token("wrong").await, Err(ApiError::Auth)));
    let access = c.access_token("refresh-abc").await.unwrap();
    assert_eq!(c.metering_points(&access).await.unwrap().len(), 1);
}

/// Two metering points, so the flow asks which to use.
fn two_points() -> Value {
    let mut v = fixture("meteringpoints.json");
    let mut second = v["result"][0].clone();
    second["meteringPointId"] = json!("571313100000000002");
    second["streetName"] = json!("Andenvej");
    v["result"].as_array_mut().unwrap().push(second);
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_flow() {
    let server = MockServer::start().await;
    mock_api(&server, two_points()).await;
    let h = host::FakeHost::start(Eloverblik::default());
    let flow = |source: &str, data: Value| json!({ "flow_id": "f1", "source": source, "data": data });
    let base = json!({ "base_url": server.uri() });

    let r = h.call("config_flow", json!({ "flow": flow("user", base.clone()), "step": "user", "input": null })).await.unwrap();
    assert_eq!(r["result"]["type"], "form");
    assert_eq!(r["result"]["schema"]["properties"]["refresh_token"]["format"], "password");

    let r = h.call("config_flow", json!({ "flow": flow("user", base.clone()), "step": "user", "input": { "refresh_token": "wrong" } })).await.unwrap();
    assert_eq!(r["result"]["errors"]["base"], "invalid_auth");

    // Two metering points: pick one in a second step.
    let r = h.call("config_flow", json!({ "flow": flow("user", base.clone()), "step": "user", "input": { "refresh_token": " refresh-abc " } })).await.unwrap();
    assert_eq!(r["result"]["step_id"], "select");
    assert_eq!(r["result"]["schema"]["properties"]["metering_point"]["enum"], json!(["alle", MP, "571313100000000002"]));
    assert!(r["result"]["description"].as_str().unwrap().contains("Andenvej 1"));
    let data = r["data"].clone();
    assert_eq!(data["refresh_token"], "refresh-abc");

    let r = h.call("config_flow", json!({ "flow": flow("user", data.clone()), "step": "select", "input": { "metering_point": MP } })).await.unwrap();
    assert_eq!(r["result"]["type"], "create_entry");
    assert_eq!(r["result"]["title"], "Eloverblik Testvej 1, 2100 København Ø");
    assert_eq!(r["unique_id"], MP);
    assert_eq!(r["result"]["data"]["metering_points"], json!([{ "id": MP, "kind": "E17", "address": "Testvej 1, 2100 København Ø" }]));
    assert!(r["result"]["data"].get("_points").is_none());

    let r = h.call("config_flow", json!({ "flow": flow("user", data.clone()), "step": "select", "input": { "metering_point": "alle" } })).await.unwrap();
    assert_eq!(r["result"]["title"], "Eloverblik");
    assert_eq!(r["unique_id"], format!("{MP},571313100000000002"));

    let e = h.call("config_flow", json!({ "flow": flow("user", data), "step": "select", "input": { "metering_point": "123" } })).await.unwrap_err();
    assert_eq!(e["data"]["kind"], "invalid_argument");

    // reauth keeps the entry's other data
    let mut reauth = flow("reauth", base);
    reauth["entry"] = json!({ "entry_id": "e1", "data": { "refresh_token": "old", "metering_points": [{ "id": MP }] } });
    let r = h.call("config_flow", json!({ "flow": reauth, "step": "reauth", "input": null })).await.unwrap();
    assert_eq!(r["result"]["step_id"], "user");
    let r = h.call("config_flow", json!({ "flow": reauth, "step": "user", "input": { "refresh_token": "refresh-abc" } })).await.unwrap();
    assert_eq!(r["result"]["data"], json!({ "refresh_token": "refresh-abc", "metering_points": [{ "id": MP }] }));
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_one_point_and_no_points() {
    let server = MockServer::start().await;
    mock_api(&server, fixture("meteringpoints.json")).await;
    let h = host::FakeHost::start(Eloverblik::default());
    let input = json!({ "refresh_token": "refresh-abc" });
    let r = h.call("config_flow", json!({ "flow": { "flow_id": "f", "source": "user", "data": { "base_url": server.uri() } }, "step": "user", "input": input })).await.unwrap();
    assert_eq!(r["result"]["type"], "create_entry");
    assert_eq!(r["unique_id"], MP);

    let empty = MockServer::start().await;
    mock_api(&empty, json!({ "result": [] })).await;
    let r = h.call("config_flow", json!({ "flow": { "flow_id": "f", "source": "user", "data": { "base_url": empty.uri() } }, "step": "user", "input": input })).await.unwrap();
    assert_eq!(r["result"], json!({ "type": "abort", "reason": "no_metering_points" }));

    // Nothing listening: cannot_connect.
    let r = h.call("config_flow", json!({ "flow": { "flow_id": "f", "source": "user", "data": { "base_url": "http://127.0.0.1:9" } }, "step": "user", "input": input })).await.unwrap();
    assert_eq!(r["result"]["errors"]["base"], "cannot_connect");
}

fn entry(server: &MockServer, token: &str) -> Value {
    json!({ "entry_id": "e1", "domain": "eloverblik_ext", "title": "Eloverblik",
            "data": { "refresh_token": token, "base_url": server.uri(),
                      "metering_points": [{ "id": MP, "kind": "E17", "address": "Testvej 1, 2100 København Ø" }] },
            "options": {} })
}

async fn wait_state(h: &host::FakeHost, id: &str) -> (String, Value) {
    for _ in 0..200 {
        if let Some(s) = h.state(id).filter(|s| s.0 != "unknown" && s.0 != "unavailable") {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("{id} fik aldrig en tilstand");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_setup() {
    let server = MockServer::start().await;
    mock_api(&server, fixture("meteringpoints.json")).await;
    let dir = tempfile::tempdir().unwrap();
    let h = host::FakeHost::start(Eloverblik::default());
    h.call("initialize", json!({ "api_version": 1, "data_dir": dir.path() })).await.unwrap();

    // A wrong token on an entry asks for a new one.
    let e = h.call("setup_entry", json!({ "entry": entry(&server, "nope") })).await.unwrap_err();
    assert_eq!(e["data"]["kind"], "auth_failed");

    h.call("setup_entry", json!({ "entry": entry(&server, "refresh-abc") })).await.unwrap();
    {
        let rec = h.rec.lock().unwrap();
        assert_eq!(rec.devices.len(), 1);
        let d = &rec.devices[0];
        assert_eq!(d["identifiers"], json!([["eloverblik_ext", MP]]));
        assert_eq!(d["name"], "Elmåler Testvej 1, 2100 København Ø");
        assert_eq!(d["model"], "Målepunkt E17");
        assert_eq!(d["entry_type"], "service");
    }

    let total_id = h.entity_ending("samlet_forbrug");
    let (spec, handler) = h.rec.lock().unwrap().entities[&total_id].clone();
    assert!(!handler);
    assert_eq!(spec["suggested_object_id"], "eloverblik_000001_total");
    assert_eq!(spec["state_class"], "total_increasing");
    assert_eq!(spec["unit_of_measurement"], "kWh");
    assert_eq!(spec["device_class"], "energy");

    let (state, attrs) = wait_state(&h, &total_id).await;
    let expected: f64 = (0..3).flat_map(|d| (0..24).map(move |i| 0.2 + 0.05 * f64::from(i) + 0.1 * f64::from(d))).sum();
    assert!((state.parse::<f64>().unwrap() - expected).abs() < 1e-3);
    assert_eq!(attrs["data_until"], "2026-10-11T00:00:00+02:00");

    // The fixture ends 10 October, so "yesterday" falls back to that day
    // when it is within a week; otherwise it stays unset.
    let yday_id = h.entity_ending("forbrug_i_g_r");
    let (spec, _) = h.rec.lock().unwrap().entities[&yday_id].clone();
    assert_eq!(spec["state_class"], "total");
    if let Some((s, a)) = h.state(&yday_id) {
        let day: f64 = (0..24).map(|i| 0.4 + 0.05 * f64::from(i)).sum();
        assert!((s.parse::<f64>().unwrap() - day).abs() < 1e-3);
        assert_eq!(a["date"], "2026-10-10");
        assert_eq!(a["hourly"].as_array().unwrap().len(), 24);
    }

    // The running total is saved in the data folder.
    let saved: Total = serde_json::from_str(&std::fs::read_to_string(dir.path().join(format!("total-e1-{MP}.json"))).unwrap()).unwrap();
    assert!((saved.kwh - expected).abs() < 1e-9);

    // The hours go to a statistic for the Energi page, after a year of
    // history in chunks of 92 days.
    let stat = statistic_id(MP);
    assert_eq!(stat, "eloverblik_ext:571313100000000001");
    let p = h.rec.lock().unwrap().statistics.get(&stat).cloned().expect("no statistics/import");
    assert_eq!(p["name"], "Eloverblik Testvej 1, 2100 København Ø");
    assert_eq!(p["unit_of_measurement"], "kWh");
    assert_eq!(p["has_sum"], true);
    let rows = p["stats"].as_array().unwrap();
    assert_eq!(rows.len(), 73);
    assert!((rows[72]["sum"].as_f64().unwrap() - expected).abs() < 1e-3);
    let saved: History = serde_json::from_str(&std::fs::read_to_string(dir.path().join(format!("hours-e1-{MP}.json"))).unwrap()).unwrap();
    assert!(saved.backfilled);
    let requests = server.received_requests().await.unwrap();
    let series = requests.iter().filter(|r| r.url.path().starts_with("/meterdata/gettimeseries/")).count();
    assert_eq!(series, 1 + 4, "the last week, then 358 days in four requests");

    let diag = h.call("diagnostics", json!({ "entry": entry(&server, "refresh-abc") })).await.unwrap();
    assert_eq!(diag["refresh_token"], "**REDACTED**");
    h.call("unload_entry", json!({ "entry_id": "e1" })).await.unwrap();
}
