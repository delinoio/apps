# Git Tracker

Verfolgen Sie numerische Messwerte für Git-Commits unter [git-tracker.delino.io](https://git-tracker.delino.io). Git Tracker ist kostenlos. Es unterstützt öffentliche und private GitHub-Repositorys, einschließlich Pull Requests aus Forks. Verwenden Sie es für Dateigrößen, Zeitmessungen, Durchsatz, Anzahlen und andere skalare Messwerte.

## Ein Repository verbinden

1. Melden Sie sich mit Ihrem Delino-Konto an und wählen Sie Ihren persönlichen Bereich oder Organisationsbereich aus.
2. Bitten Sie einen Bereichseigentümer, Ihnen über die Gruppen des Bereichs die Git-Tracker-Berechtigung „Anzeigen“, „Bearbeiten“ oder „Verwalten“ zu erteilen. „Anzeigen“ erlaubt Lesezugriffe. „Bearbeiten“ fügt Uploads und Abschlüsse hinzu. „Verwalten“ fügt das Verbinden, die Tokenverwaltung und das endgültige Löschen hinzu.
3. Installieren Sie die Git Tracker GitHub App für das Repository.
4. Öffnen Sie **Repositorys verwalten**. Geben Sie `owner/repository` und ein GitHub-Zugriffstoken für Ihr verknüpftes GitHub-Konto mit Administratorzugriff auf das Repository ein. Der Dienst überprüft Ihre GitHub-Berechtigung und speichert dieses Zugriffstoken nicht.
5. Verbinden Sie das Repository. Sie benötigen außerdem Eigentümer-/Administratorberechtigungen für den Delino-Bereich. Ein Repository kann jeweils nur mit einem Bereich verbunden sein. Kopieren Sie seine Verbindungs-ID aus der Repositoryverwaltung, um sie für Action- und API-Aufrufe zu verwenden. Nach dem Trennen kann ein anderer autorisierter Bereich es unter einer neuen Verbindungs-ID verbinden; der im alten Bereich aufbewahrte Verlauf bleibt privat.

Messwerte öffentlicher Repositorys können ohne Anmeldung gelesen werden. Private Messwerte erfordern die Mitgliedschaft im Bereich und eine Git-Tracker-Leseberechtigung. Der Bereichszugriff gilt für alle verbundenen Repositorys in diesem Bereich.

Durch das Trennen oder Entfernen der App werden Uploads gestoppt und der aufbewahrte Verlauf wird privat. Autorisierte Mitglieder können ihn weiterhin lesen. Eine bestätigte Änderung von öffentlich zu privat macht historische Daten privat. Wenn GitHub die aktuelle Sichtbarkeit nicht bestätigen kann, behält Git Tracker die zuletzt bestätigte Sichtbarkeit bei, bis die Synchronisierung erfolgreich ist.

## Ihren Datensatz definieren

Speichern Sie eine YAML-Konfiguration in Ihrem Repository. IDs sind stabil; Namen können geändert werden, ohne dass der Verlauf verloren geht. Alle Spalten einer Zeile müssen gemeinsam hochgeladen werden.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

Laden Sie JSON-Zeilen hoch, beispielsweise eine Dateigrößentabelle im SWC-Stil:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Die Action behält die dezimale JSON-Schreibweise bei, ohne eine Konvertierung in binäre Gleitkommazahlen vorzunehmen. Direkte Connect-Aufrufer senden Dezimalwerte als Zeichenfolgen, beispielsweise `"9007199254740993.0000000000000001"`. Negative Zahlen und endliche Dezimalzahlen werden unterstützt. Die Rundung der Anzeige ändert niemals Regressionsentscheidungen.

Unterstützte Einheiten sind `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; und `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Kompatible Einheiten werden vor dem Vergleich umgerechnet. Inkompatible Arten sind nicht vergleichbar. Beispielsweise werden 1048576 B als 1 MiB angezeigt; die exakten Werte bleiben in Tabellen sichtbar.

Richtungen sind `lower`, `higher` und `neutral`. Neutrale Spalten erzeugen niemals Regressionen. Schwellenwerte gelten für alle Zeilen in der Spalte. Wenn sowohl absolute als auch prozentuale Schwellenwerte vorhanden sind, gilt das Überschreiten eines der beiden Werte als Regression; Gleichheit nicht. Ohne Schwellenwert werden Änderungen angezeigt, verursachen aber keine Regressionsfehler. Nur Änderungen in die ungünstigere Richtung werden bewertet.

Die prozentuale Änderung ist `(new - baseline) / abs(baseline) * 100`. Bei einem Baseline-Wert von null gibt es keine prozentuale Änderung. Ein absoluter Schwellenwert kann dennoch entscheiden; eine ausschließlich prozentuale Regel verschiebt die Bewertung. Vergleiche verwenden die gespeicherte Konfiguration der Zielausführung, nicht eine spätere Konfiguration.

## Die offizielle GitHub Action verwenden

Heften Sie `delinoio/apps/actions/git-tracker` an einen geprüften vollständigen Commit-SHA an. Ersetzen Sie die nachstehenden Platzhalter durch Ihre Git-Tracker-Verbindungs-ID, Checkout-Revision und Action-Revision. Erteilen Sie für OIDC die Berechtigung `id-token: write`. Alternativ kann ein Repository-Upload-Token verwendet werden; speichern Sie es als GitHub-Secret und übergeben Sie `token`.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

Unabhängige Jobs verwenden gemeinsam die standardmäßige Ausführungs-ID `repository_id/run_id/run_attempt` und können voneinander getrennte Zeilen hochladen. Verwenden Sie für erneute Ausführungen eine neue Ausführungs-ID. Actions OIDC erfordert die Standardidentität; benutzerdefinierte Ausführungs-IDs sind für Aufrufer mit Repository-Token vorgesehen. OIDC-Uploads aus Forks sind auf den Head des PR beschränkt und können weder Baseline-Überschreibungen angeben noch den Push-Verlauf des übergeordneten Repositorys schreiben.

Jeder Upload-Job bewertet nur seine eigenen hochgeladenen Zeilen. Regressionen werden standardmäßig gemeldet, ohne dass CI fehlschlägt. Legen Sie `fail-on-regression: 'true'` fest, damit dieser Job bei Regressionen fehlschlägt. Fehlende Vergleiche lassen CI nicht fehlschlagen. Der Abschluss-Job gibt lediglich an, dass die Messung beendet ist; er übermittelt keinen CI-Erfolgs-, Fehler- oder Abbruchstatus und führt keine zusammengefasste Zugangsprüfung durch. Ein leerer Abschluss ist gültig, wenn alle Jobs übersprungen wurden.

Netzwerkfehler und vorübergehende API-Fehler werden höchstens dreimal erneut versucht. Standardmäßig schlägt der Job nach den Wiederholungsversuchen fehl. Legen Sie `warn-on-api-error: 'true'` fest, um stattdessen zu warnen und den Job erfolgreich abzuschließen. Fehler bei PR-Kommentaren führen nur zu einer Warnung; sie machen gespeicherte Daten nicht ungültig und lassen CI nicht eigenständig fehlschlagen. Die Action protokolliert einen Detaillink und schreibt eine Job-Zusammenfassung.

Ein PR-Kommentar gibt die kumulierten Ergebnisse der neuesten registrierten Ausführung für den neuesten Commit des PR wieder. Ältere Ausführungen bleiben im Webverlauf verfügbar.

## Die Connect API aufrufen

Der API-Ursprung ist `https://git-tracker.api.delino.io`. Connect-JSON-Anfragen verwenden `Content-Type: application/json` und `Connect-Protocol-Version: 1`. Verwalter können Repository-Tokens mit einem obligatorischen zukünftigen Ablaufdatum ausstellen und sie über die Repositoryverwaltung widerrufen.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` enthält `context`, `dataset` und `rows`:

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

Rufen Sie `/gittracker.v1.GitTrackerService/Complete` mit demselben `context` auf. Fügen Sie bei PR-Uploads `pullRequest` hinzu und verwenden Sie den Basis-Branch und den Head-Commit des PR. `baselineCommit` wählt einen expliziten vollständigen Baseline-SHA aus. Historische Commits verwenden dieselbe Upload-API. Konvertieren Sie Dezimalzeichenfolgen vor dem Senden nicht mit JavaScript `Number`.

## Ausführungs- und Vergleichsregeln

Der erste Upload fixiert die Konfiguration jedes Datensatzes für diese Ausführung. Eine spätere abweichende Konfiguration wird abgelehnt. Das Wiederholen einer vorhandenen Zeile mit identischen numerischen Werten ist idempotent. Abweichende Werte werden abgelehnt. Ein Batch ist atomar: Bei einer ungültigen oder widersprüchlichen Zeile wird nichts aus dieser Anfrage gespeichert.

Daten erscheinen sofort als **Wird gemessen**. Sie werden erst nach dem Abschluss repräsentativ. Das repräsentative Ergebnis für einen Commit ist dessen neueste registrierte abgeschlossene Ausführung, einschließlich Ausführungen mit Regressionen. Eine ältere Ausführung, die verspätet abgeschlossen wird, ersetzt keine neuere registrierte abgeschlossene Ausführung. Ein wiederholter Abschluss ändert weder den Abschlusszeitpunkt noch die Reihenfolge.

Nach sechs Stunden ohne neu gespeicherte Messdaten erhält eine nicht abgeschlossene Ausführung den Status **Unterbrochen**. Identische erneute Übertragungen setzen diesen Zeitgeber nicht zurück. Abgeschlossene und unterbrochene Ausführungen lehnen weitere Uploads ab; unterbrochene Ausführungen lehnen außerdem den Abschluss ab. Starten Sie stattdessen eine neue Ausführung. Die Daten bleiben erhalten, bis Sie sie löschen.

Automatische PR-Vergleiche fixieren beim ersten Upload den Merge-Base-Commit und dessen repräsentative abgeschlossene Ausführung. Eine fehlende Baseline bleibt unverfügbar, selbst wenn sie später hochgeladen wird. Automatische Push-Vergleiche durchlaufen die First-Parent-Abstammung für jede Zeile separat und verwenden den nächstgelegenen gemessenen Vorgänger, der bis zum Stichtag des ersten Uploads abgeschlossen war. Spätere Jobs verwenden denselben Stichtag. Die Tabelle weist jeden tatsächlich verwendeten Quell-Commit und jede Ausführung aus. Vergleiche mit einem expliziten Baseline-SHA verwenden keinen zeilenweisen Rückgriff auf Vorgänger.

Der Zwei-Commit-Vergleich der Website verwendet ausschließlich die repräsentativen abgeschlossenen Ausführungen der ausgewählten Commits. Neue Zeilen/Spalten sowie fehlende oder inkompatible Daten sind nicht vergleichbar und verursachen keinen Regressionsfehler. Gelöschte Baseline-Daten werden nicht rekonstruiert. Vor dem Abschluss gilt eine nicht vorhandene Zeile als nicht erfasst; nach dem Abschluss gilt sie als fehlend.

Diagramme verwenden gleichmäßig verteilte Commits in First-Parent-Reihenfolge. Sie verbinden tatsächlich gemessene Punkte über übersprungene Commits hinweg und übernehmen oder erfinden niemals Werte. Filtern Sie nach Datensatz, Branch, Spalte, Elementen, Datumsangaben und Commit-Bereich. Verwenden Sie die Datentabelle für exakte Werte und per Tastatur zugängliche Ausführungslinks.

## Löschen und Fehlerbehebung

Verwalter können nach einer Bestätigung die Messwerte eines Repositorys, einen Datensatz oder eine einzelne Ausführung endgültig löschen. Ein Vergleich, dessen Baseline gelöscht wurde, ist anschließend nicht mehr verfügbar. Wenn Delcore die Löschung eines Bereichs abschließt, löscht Git Tracker die Verbindungen, Tokens, Konfiguration und den Verlauf dieses Bereichs.

Überprüfen Sie bei einem Berechtigungsfehler den ausgewählten Bereich und Ihre aktuelle Git-Tracker-Rolle. Prüfen Sie bei Upload-Konflikten die Ausführungsidentität, die Datensatzkonfiguration und die vollständigen Zeilenwerte. Verwenden Sie für unterbrochene oder abgeschlossene Ausführungen eine neue Ausführungs-ID. Versuchen Sie es bei Netzwerk-/Serverfehlern erneut und bewahren Sie die Korrelations-ID der Antwort auf, wenn Sie um Hilfe bitten. Geben Sie Tokens, Anmeldedaten oder private Messwerte niemals in öffentlichen Berichten weiter.

Messwerte von PR-Ausführungen bleiben einsehbar, liefern aber weder Diagrammpunkte für Branches des übergeordneten Repositorys noch automatische Baselines. Dadurch wird verhindert, dass Messwerte aus Forks die Daten oder die Konfiguration des Baseline-Branches für einen gemeinsam verwendeten Commit-SHA ersetzen.
