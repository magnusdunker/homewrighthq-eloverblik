# Eloverblik til Homewright

En ekstern integration til [Homewright](https://github.com/magnusdunker/homewright), der henter elforbruget fra Energinets [Eloverblik](https://eloverblik.dk), så det kan vises på Energi-siden.

## Installation

I Homewright: **Indstillinger → Integrationer → Fra GitHub**. Indsæt

```
https://github.com/magnusdunker/homewright-eloverblik
```

og tryk **Installér**. Tilføj derefter **Eloverblik** under **Tilføj integration**, og indsæt et token fra eloverblik.dk. Det oprettes under **Datadeling** og gælder et år. Har du flere målepunkter, vælger du ét eller dem alle.

## På Energi-siden

Hvert målepunkt får en statistik med forbruget time for time, `eloverblik_ext:<målepunkt>`, med navnet **Eloverblik** og adressen. Vælg den under **Energi → Opsæt → Fra nettet**. Så lander forbruget i de timer, det blev brugt i, selv om Eloverblik først leverer det dagen efter.

Første gang henter integrationen det seneste år, så Energi-siden har historik fra starten. Derefter kommer hver ny hel dag med, og retter Eloverblik en time inden for den seneste uge, bliver statistikken rettet.

Statistikken kræver en Homewright, der kender `statistics/import`, altså nyere end 0.5.2. Med en ældre Homewright virker sensorerne som før, og loggen siger, at statistikken ikke kunne gemmes.

## Sensorer

Hvert målepunkt bliver også en elmåler med to sensorer i kWh:

- **Forbrug i går**: gårsdagens forbrug med timerne i attributten `hourly`. Er gårsdagen ikke kommet endnu, viser den den seneste hele dag, og `is_yesterday` er `false`.
- **Samlet forbrug**: en løbende total, der vokser med hver hel dag, Eloverblik leverer. `data_until` viser, hvor langt data rækker.

Brug statistikken, ikke **Samlet forbrug**, på Energi-siden. Sensoren vokser én gang om dagen, så hele dagens forbrug ville lande i den time, den blev opdateret.

Eloverblik leverer data cirka et døgn forsinket, så integrationen henter de seneste syv dage hver tredje time. Totalen og timerne gemmes i integrationens datamappe.

Afviser Eloverblik tokenet, fordi det er udløbet eller tilbagekaldt, viser Homewright det under problemer, og integrationen kan godkendes igen med et nyt token.

## Forholdet til den indbyggede integration

Homewright har også Eloverblik indbygget (`eloverblik`). Den eksterne integration har sit eget domæne, `eloverblik_ext`, så de to kan være installeret samtidig. Den eksterne kører som sit eget program og opdateres uafhængigt af Homewright, og kun den laver statistikken til Energi-siden.

Sensorerne foreslår de samme entity-id'er som den indbyggede, f.eks. `sensor.eloverblik_000001_total`. Slet den indbyggede opsætning, før du sætter den eksterne op, så får de id'erne, og automationer og dashboards virker uændret. Er de begge sat op, får den eksterne et `_2` bag på id'erne. Den løbende total starter forfra fra de seneste syv dage, fordi den indbyggede gemmer sin total i Homewright.

## Udvikling

Bygget i Rust på SDK'en `thehome-plugin` fra [homewright-example-integration-rust](https://github.com/magnusdunker/homewright-example-integration-rust).

- `src/api.rs`: Eloverbliks API og beregningen af dage, totaler og timer
- `src/lib.rs`: forbindelsen til Homewright
- `tests/`: tests mod et falsk Eloverblik, og en falsk Homewright i `tests/host`

```
cargo test
cargo build --release
```

En ny version udgives med et tag, f.eks. `git tag v0.3.0 && git push origin v0.3.0`. Workflowet bygger til Windows, Linux (x86_64 og arm64, statisk musl) og macOS og lægger binærerne på releasen.

## Licens

MIT
