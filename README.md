# Eloverblik til TheHome

En ekstern integration til [TheHome](https://github.com/magnusdunker/thehome), der henter elforbruget fra Energinets [Eloverblik](https://eloverblik.dk).

## Installation

I TheHome: **Indstillinger → Integrationer → Fra GitHub**. Indsæt

```
https://github.com/magnusdunker/thehome-eloverblik
```

og tryk **Installér**. Tilføj derefter **Eloverblik** under **Tilføj integration**, og indsæt et token fra eloverblik.dk. Det oprettes under **Datadeling** og gælder et år. Har du flere målepunkter, vælger du ét eller dem alle.

## Hvad du får

Hvert målepunkt bliver en elmåler i TheHome med to sensorer i kWh:

- **Forbrug i går**: gårsdagens forbrug med timerne i attributten `hourly`. Er gårsdagen ikke kommet endnu, viser den den seneste hele dag, og `is_yesterday` er `false`.
- **Samlet forbrug**: en løbende total, der vokser med hver hel dag, Eloverblik leverer. Den kan bruges i energioversigten. `data_until` viser, hvor langt data rækker.

Eloverblik leverer data cirka et døgn forsinket, så integrationen henter de seneste syv dage hver tredje time. Totalen gemmes i integrationens datamappe og tæller kun hele dage med, så en dag, der stadig er på vej, ikke tælles to gange.

Afviser Eloverblik tokenet, fordi det er udløbet eller tilbagekaldt, viser TheHome det under problemer, og integrationen kan godkendes igen med et nyt token.

## Forholdet til den indbyggede integration

TheHome har også Eloverblik indbygget (`eloverblik`). Den eksterne integration har sit eget domæne, `eloverblik_ext`, så de to kan være installeret samtidig. Koden er den samme, men den eksterne kører som sit eget program og opdateres uafhængigt af TheHome.

Sensorerne foreslår de samme entity-id'er som den indbyggede, f.eks. `sensor.eloverblik_000001_total`. Slet den indbyggede opsætning, før du sætter den eksterne op, så får de id'erne, og automationer og dashboards virker uændret. Er de begge sat op, får den eksterne et `_2` bag på id'erne. Den løbende total starter forfra fra de seneste syv dage, fordi den indbyggede gemmer sin total i TheHome.

## Udvikling

Bygget i Rust på SDK'en `thehome-plugin` fra [thehome-example-integration-rust](https://github.com/magnusdunker/thehome-example-integration-rust).

- `src/api.rs`: Eloverbliks API og beregningen af dage og totaler
- `src/lib.rs`: forbindelsen til TheHome
- `tests/`: tests mod et falsk Eloverblik, og en falsk TheHome i `tests/host`

```
cargo test
cargo build --release
```

En ny version udgives med et tag, f.eks. `git tag v0.2.0 && git push origin v0.2.0`. Workflowet bygger til Windows, Linux (x86_64 og arm64, statisk musl) og macOS og lægger binærerne på releasen.

## Licens

MIT
