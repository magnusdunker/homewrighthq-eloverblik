# Eloverblik for HomewrightHQ

An external integration for [HomewrightHQ](https://github.com/magnusdunker/homewrighthq) that fetches electricity consumption from Energinet's [Eloverblik](https://eloverblik.dk), so it can be shown on the Energy page.

## Installation

In HomewrightHQ: **Settings → Integrations → From GitHub**. Paste

```
https://github.com/magnusdunker/homewrighthq-eloverblik
```

and press **Install**. Then add **Eloverblik** under **Add integration**, and paste a token from eloverblik.dk. You create it under **Datadeling** (data sharing), and it is valid for one year. If you have several metering points, you pick one or all of them.

## On the Energy page

Each metering point gets a statistic with hourly consumption, `eloverblik_ext:<metering point>`, named **Eloverblik** plus the address. Pick it under **Energy → Configure → From grid**. The consumption then lands in the hours it was used, even though Eloverblik only delivers it the day after.

The first time, the integration fetches history backwards, 92 days at a time, until Eloverblik has no more, up to five years at most. The Energy page can then show consumption year by year from the start. A setup from 0.2 that has fetched one year fetches the rest on its own. After that each new full day is added, and if Eloverblik corrects an hour within the last week, the statistic is corrected.

The statistic needs a HomewrightHQ that knows `statistics/import`, so newer than 0.5.2. With an older HomewrightHQ the sensors work as before, and the log says the statistic could not be saved.

## Sensors

Each metering point also becomes an electricity meter with two sensors in kWh:

- **Forbrug i går** (yesterday's consumption): yesterday's consumption with the hours in the attribute `hourly`. If yesterday has not arrived yet, it shows the latest full day, and `is_yesterday` is `false`.
- **Samlet forbrug** (total consumption): a running total that grows with each full day Eloverblik delivers. `data_until` shows how far the data goes.

Use the statistic, not **Samlet forbrug**, on the Energy page. The sensor grows once a day, so the whole day's consumption would land in the hour it was updated.

Eloverblik delivers data about a day late, so the integration fetches the last seven days every three hours. The total and the hours are stored in the integration's data folder.

If Eloverblik rejects the token because it has expired or been revoked, HomewrightHQ shows it under repairs, and the integration can be reauthenticated with a new token.

## Relation to the built-in integration

HomewrightHQ also has Eloverblik built in (`eloverblik`). The external integration has its own domain, `eloverblik_ext`, so the two can be installed at the same time. The external one runs as its own program and is updated independently of HomewrightHQ, and only it creates the statistic for the Energy page.

The sensors suggest the same entity IDs as the built-in one, e.g. `sensor.eloverblik_000001_total`. Delete the built-in setup before you set up the external one, then they get those IDs, and automations and dashboards work unchanged. If both are set up, the external one gets `_2` appended to the IDs. The running total starts over from the last seven days, because the built-in one stores its total in HomewrightHQ.

## Development

Built in Rust on the `thehome-plugin` SDK from [homewright-example-integration-rust](https://github.com/magnusdunker/homewrighthq-example-integration-rust).

- `src/api.rs`: Eloverblik's API and the calculation of days, totals and hours
- `src/lib.rs`: the connection to HomewrightHQ
- `tests/`: tests against a fake Eloverblik, and a fake HomewrightHQ in `tests/host`

```
cargo test
cargo build --release
```

A new version is released with a tag, e.g. `git tag v0.3.0 && git push origin v0.3.0`. The workflow builds for Windows, Linux (x86_64 and arm64, static musl) and macOS and puts the binaries on the release.

## License

MIT
