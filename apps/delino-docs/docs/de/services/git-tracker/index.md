# Git Tracker

Verfolgen Sie numerische Messwerte für Git-Commits unter [git-tracker.delino.io](https://git-tracker.delino.io). Git Tracker ist kostenlos. Es unterstützt öffentliche und private GitHub-Repositorys, einschließlich Pull Requests aus Forks. Verwenden Sie es für Dateigrößen, Zeitmessungen, Durchsatz, Anzahlen und andere skalare Messwerte.

## Ein Repository verbinden

1. Melden Sie sich mit Ihrem Delino-Konto an und wählen Sie Ihren persönlichen oder Ihren Organisationsbereich aus.
2. Bitten Sie einen Bereichseigentümer, Ihnen über die Gruppen des Bereichs die Git-Tracker-Berechtigung View, Edit oder Manage zu erteilen. View erlaubt Lesezugriffe. Edit ermöglicht zusätzlich Uploads und den Abschluss von Ausführungen. Manage ermöglicht außerdem das Verbinden, die Token-Verwaltung und das dauerhafte Löschen.
3. Installieren Sie die Git Tracker GitHub App für das Repository.
4. Öffnen Sie **Repositorys verwalten**. Geben Sie `owner/repository` und ein GitHub-Zugriffstoken für Ihr verknüpftes GitHub-Konto mit Administratorzugriff auf das Repository ein. Der Dienst überprüft Ihre GitHub-Berechtigung und speichert dieses Zugriffstoken nicht.
5. Verbinden Sie das Repository. Sie benötigen außerdem Owner-/Admin-Berechtigungen für den Delino-Bereich. Ein Repository kann jeweils nur mit einem Bereich verbunden sein. Kopieren Sie seine Verbindungs-ID aus der Repository-Verwaltung für Action- und API-Aufrufe. Nach dem Trennen kann ein anderer berechtigter Bereich es unter einer neuen Verbindungs-ID verbinden; der im alten Bereich aufbewahrte Verlauf bleibt privat.

Messwerte öffentlicher Repositorys können ohne Anmeldung gelesen werden. Private Messwerte erfordern die Mitgliedschaft im Bereich und die Git-Tracker-Leseberechtigung. Der Bereichszugriff gilt für alle verbundenen Repositorys in diesem Bereich.

Das Trennen der Verbindung oder Entfernen der App stoppt Uploads und macht den aufbewahrten Verlauf privat. Berechtigte Mitglieder können ihn weiterhin lesen. Ein bestätigter Wechsel von öffentlich zu privat macht historische Daten privat. Wenn GitHub die aktuelle Sichtbarkeit nicht bestätigen kann, behält Git Tracker die zuletzt bestätigte Sichtbarkeit bei, bis die Synchronisierung erfolgreich ist.

## Ihren Datensatz definieren

Bewahren Sie eine YAML-Konfiguration in Ihrem Repository auf. IDs sind stabil; Namen können geändert werden, ohne dass der Verlauf verloren geht. Alle Spalten einer Zeile müssen gemeinsam hochgeladen werden.

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

Laden Sie JSON-Zeilen hoch, zum Beispiel eine Dateigrößentabelle im SWC-Stil:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Die Action bewahrt die dezimale Schreibweise aus JSON ohne eine binäre Gleitkommakonvertierung. Direkte Connect-Aufrufer senden Dezimalwerte als Zeichenfolgen, beispielsweise `"9007199254740993.0000000000000001"`. Negative Zahlen und endliche Dezimalzahlen werden unterstützt. Rundungen in der Anzeige ändern niemals die Entscheidung über eine Regression.

Unterstützte Einheiten sind `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; sowie `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Kompatible Einheiten werden vor dem Vergleich umgerechnet. Inkompatible Arten sind nicht vergleichbar. Beispielsweise werden 1048576 B als 1 MiB angezeigt; exakte Werte bleiben in Tabellen sichtbar.

Die Richtungen sind `lower`, `higher` und `neutral`. Neutrale Spalten erzeugen niemals Regressionen. Schwellenwerte gelten für alle Zeilen der Spalte. Wenn sowohl absolute als auch prozentuale Schwellenwerte festgelegt sind, gilt das Überschreiten eines der beiden als Regression; Gleichheit nicht. Ohne Schwellenwert werden Änderungen angezeigt, führen aber nicht zu Regressionsfehlern. Nur Änderungen in Richtung einer Verschlechterung werden beurteilt.

Die prozentuale Änderung ist `(new - baseline) / abs(baseline) * 100`. Bei einem Basiswert von null gibt es keine prozentuale Änderung. Ein absoluter Schwellenwert kann dennoch eine Entscheidung ermöglichen; bei einer rein prozentualen Regel bleibt die Beurteilung offen. Vergleiche verwenden die gespeicherte Konfiguration der Zielausführung, nicht eine spätere Konfiguration.

## Die offizielle GitHub Action verwenden

Fixieren Sie `delinoio/apps/actions/git-tracker` auf einen überprüften vollständigen Commit-SHA. Ersetzen Sie die folgenden Platzhalter durch Ihre Git-Tracker-Verbindungs-ID, die Checkout-Revision und die Action-Revision. Gewähren Sie `id-token: write` für OIDC. Ein Repository-Upload-Token ist eine Alternative; speichern Sie es als GitHub-Secret und übergeben Sie `token`.

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

Unabhängige Jobs teilen sich die standardmäßige Ausführungs-ID `repository_id/run_id/run_attempt` und können disjunkte Zeilen hochladen. Verwenden Sie für erneute Durchläufe eine neue Ausführungs-ID. Actions OIDC erfordert die Standardidentität; benutzerdefinierte Ausführungs-IDs sind für Aufrufer mit Repository-Token vorgesehen. PR-Jobs innerhalb desselben Repositorys dürfen `baseline` festlegen. OIDC-Uploads aus Forks sind auf den Head-Commit des PR beschränkt und können weder die Vergleichsbasis überschreiben noch in den Upstream-Push-Verlauf schreiben.

Jeder Upload-Job beurteilt nur seine eigenen hochgeladenen Zeilen. Regressionen werden standardmäßig gemeldet, ohne dass die CI fehlschlägt. Setzen Sie `fail-on-regression: 'true'`, damit dieser Job bei Regressionen fehlschlägt. Fehlende Vergleiche lassen die CI nicht fehlschlagen. Der Abschluss-Job meldet lediglich, dass die Messung beendet ist; er übermittelt weder Erfolg, Fehlschlag oder Abbruch der CI noch führt er eine zusammengefasste Freigabeprüfung durch. Ein leerer Abschluss ist gültig, wenn alle Jobs übersprungen wurden.

Bei Netzwerkfehlern und vorübergehenden API-Fehlern erfolgen höchstens drei Wiederholungsversuche. Standardmäßig schlägt der Job nach den Wiederholungsversuchen fehl. Setzen Sie `warn-on-api-error: 'true'`, um stattdessen eine Warnung auszugeben und den Job erfolgreich abzuschließen. Fehler bei PR-Kommentaren erzeugen nur Warnungen; sie machen gespeicherte Daten nicht ungültig und lassen die CI nicht eigenständig fehlschlagen. Die Action protokolliert einen Detail-Link und schreibt eine Job-Zusammenfassung.

Ein PR-Kommentar spiegelt die kumulierten Ergebnisse der zuletzt registrierten Ausführung für den neuesten Commit des PR wider. Ältere Durchläufe bleiben im Webverlauf verfügbar.

## Die Connect API aufrufen

Der API-Origin ist `https://git-tracker.api.delino.io`. Connect-JSON-Anfragen verwenden `Content-Type: application/json` und `Connect-Protocol-Version: 1`. Verwalter können Repository-Tokens mit einem verpflichtenden zukünftigen Ablaufdatum ausstellen und sie in der Repository-Verwaltung widerrufen.

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

Rufen Sie `/gittracker.v1.GitTrackerService/Complete` mit demselben `context` auf. Fügen Sie bei PR-Uploads `pullRequest` hinzu und verwenden Sie den Basis-Branch und den Head-Commit des PR. `baselineCommit` wählt einen expliziten vollständigen Basis-SHA aus. Historische Commits verwenden dieselbe Upload-API. Konvertieren Sie dezimale Zeichenfolgen vor dem Senden nicht über JavaScript `Number`.

## Regeln für Ausführungen und Vergleiche

Der erste Upload fixiert die Konfiguration jedes Datensatzes für diese Ausführung. Eine abweichende spätere Konfiguration wird abgelehnt. Die erneute Übermittlung einer vorhandenen Zeile mit identischen numerischen Werten ist idempotent. Widersprüchliche Werte werden abgelehnt. Ein Batch ist atomar: Bei einer ungültigen oder widersprüchlichen Zeile wird nichts aus dieser Anfrage gespeichert.

Daten erscheinen sofort mit dem Status **Messung läuft**. Sie werden erst nach dem Abschluss repräsentativ. Das repräsentative Ergebnis für einen Commit ist seine zuletzt registrierte abgeschlossene Ausführung, einschließlich Ausführungen mit Regressionen. Ein älterer Durchlauf, der später fertig wird, ersetzt keinen neuer registrierten abgeschlossenen Durchlauf. Ein wiederholter Abschluss ändert weder den Abschlusszeitpunkt noch die Reihenfolge.

Nach sechs Stunden ohne neu gespeicherte Messdaten erhält eine nicht abgeschlossene Ausführung den Status **Unterbrochen**. Identische erneute Übertragungen setzen diesen Timer nicht zurück. Abgeschlossene und unterbrochene Ausführungen lehnen weitere Uploads ab; unterbrochene Ausführungen lehnen auch den Abschluss ab. Starten Sie stattdessen eine neue Ausführung. Daten bleiben erhalten, bis Sie sie löschen.

Automatische PR-Vergleiche fixieren beim ersten Upload den Merge-Base-Commit und seine repräsentative abgeschlossene Ausführung. Eine fehlende Vergleichsbasis bleibt nicht verfügbar, selbst wenn sie später hochgeladen wird. Automatische Push-Vergleiche durchlaufen für jede Zeile separat die First-Parent-Vorgängerkette und verwenden den nächstgelegenen gemessenen Vorgänger, dessen Ausführung bis zum Stichtag des ersten Uploads abgeschlossen war. Spätere Jobs verwenden denselben Stichtag. Die Tabelle gibt jeden tatsächlich als Quelle verwendeten Commit und die zugehörige Ausführung an. Vergleiche mit einem expliziten Basis-SHA greifen nicht zeilenweise auf Vorgänger zurück.

Der Zwei-Commit-Vergleich der Website verwendet nur die repräsentativen abgeschlossenen Ausführungen der ausgewählten Commits. Neue Zeilen/Spalten sowie fehlende oder inkompatible Daten sind nicht vergleichbar und führen nicht zu einem Regressionsfehler. Gelöschte Basisdaten werden nicht rekonstruiert. Vor dem Abschluss gilt eine nicht vorhandene Zeile als noch nicht erfasst; nach dem Abschluss gilt sie als fehlend.

Diagramme verwenden gleichmäßig verteilte Commits in First-Parent-Reihenfolge. Sie verbinden tatsächlich gemessene Punkte über übersprungene Commits hinweg und schreiben niemals Werte fort oder erfinden sie. Filtern Sie nach Datensatz, Branch, Spalte, Elementen, Datum und Commit-Bereich. Verwenden Sie die Datentabelle für exakte Werte und per Tastatur zugängliche Links zu Ausführungen.

## Löschen und Fehlerbehebung

Verwalter können nach Bestätigung die Messwerte eines Repositorys, einen Datensatz oder eine einzelne Ausführung dauerhaft löschen. Ein Vergleich, dessen Vergleichsbasis gelöscht wurde, ist nicht mehr verfügbar. Wenn Delcore die Löschung eines Bereichs abschließt, löscht Git Tracker die Verbindungen, Tokens, Konfiguration und den Verlauf des Bereichs.

Prüfen Sie bei einem Berechtigungsfehler den ausgewählten Bereich und Ihre aktuelle Git-Tracker-Rolle. Überprüfen Sie bei Upload-Konflikten die Ausführungsidentität, die Datensatzkonfiguration und die vollständigen Zeilenwerte. Verwenden Sie für unterbrochene oder abgeschlossene Durchläufe eine neue Ausführungs-ID. Versuchen Sie es bei Netzwerk-/Serverfehlern erneut und bewahren Sie die Korrelations-ID der Antwort auf, wenn Sie um Hilfe bitten. Teilen Sie niemals Tokens, Zugangsdaten oder private Messwerte in öffentlichen Berichten.

Messwerte aus PR-Ausführungen bleiben einsehbar, liefern jedoch weder Punkte für Upstream-Branch-Diagramme noch automatische Vergleichsbasen. Dadurch wird verhindert, dass Messwerte aus Forks die Daten oder Konfiguration des Basis-Branches für einen gemeinsamen Commit-SHA ersetzen.
