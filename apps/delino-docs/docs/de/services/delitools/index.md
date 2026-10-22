# DeliTools

DeliTools stellt KI-Werkzeuge für Entwickler über eine Connect-API bereit. Version 1 hat zwei synchrone Methoden: die **Übersetzung** eines Textarrays und die **Datenextraktion** aus Text in JSON, das Ihrem Schema folgt. Jeder Aufruf gibt das fertige Ergebnis oder einen Fehler in derselben Antwort zurück. Es gibt keine Jobs, die Sie abfragen müssen, und kein Ergebnis, das Sie später abrufen müssen. Alle Methoden befinden sich unter `delitools.v1.DeliToolsService`.

Rufen Sie DeliTools aus serverseitigem Code oder von der Befehlszeile aus auf, mit einem beliebigen Connect-Client oder einfachem JSON über HTTP. Fügen Sie keinen API-Schlüssel in Frontend-Code ein. Setzen Sie `DELITOOLS_URL` auf die Basis-URL Ihres DeliTools-API-Servers.

## Schnellstart

1. **Wählen Sie eine Anmeldeinformation.** Verwenden Sie Ihr Delino-Anmeldetoken, oder erstellen Sie mit `CreateKey` einen API-Schlüssel. Siehe den Abschnitt Anmeldung und API-Schlüssel.
2. **Ermitteln Sie Ihre Mandanten-ID.** Die Nutzung wird dem Delino-Mandanten berechnet, den Sie in `tenantId` angeben.
3. **Rufen Sie eine Methode auf** mit `Authorization: Bearer <credential>`.

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

In JSON verwenden Feldnamen lowerCamelCase und Enum-Werte ihre vollständigen Namen.

## Text übersetzen

`TranslateTextArray` übersetzt jede Zeichenfolge in einem Array.

| Feld             | Erforderlich | Bedeutung                                                                                                                        |
| ---------------- | ------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Ja           | Der Mandant, dem die Nutzung berechnet wird. Verwenden Sie dessen kanonische UUID Version 7: Kleinbuchstaben, mit Bindestrichen. |
| `outputLanguage` | Ja           | BCP-47-Sprachtag der Übersetzung, zum Beispiel `ko`, `pt-BR` oder `zh-Hans`.                                                     |
| `inputLanguage`  | Nein         | BCP-47-Sprachtag der Quelle. Lassen Sie ihn weg, damit eine Sprache für das gesamte Array erkannt wird.                          |
| `model`          | Ja           | Ein Modell aus der Zulassungsliste des Betreibers.                                                                               |
| `texts`          | Ja           | Die zu übersetzenden Zeichenfolgen.                                                                                              |
| `idempotencyKey` | Nein         | Siehe Idempotenz.                                                                                                                |

Die Antwort enthält `requestId` und `texts`, die Übersetzungen in der ursprünglichen Reihenfolge und mit der ursprünglichen Länge.

Regeln:

- Sprachwerte sind BCP-47-Tags. `outputLanguage` ist erforderlich. Ein Tag, das kein gültiges BCP 47 ist, nicht nur aus ASCII-Zeichen besteht oder länger als 64 Byte ist, gibt einen Fehler wegen ungültiger Eingabe zurück.
- `texts` darf nicht leer sein, und keine Zeichenfolge darf leer sein oder nur aus Leerraum bestehen.
- Wenn Sie `inputLanguage` festlegen und es mit `outputLanguage` übereinstimmt, ist die Anfrage ungültig.
- Wenn Sie `inputLanguage` weglassen, erkennt DeliTools eine Eingabesprache für das gesamte Array. Ein Array mit gemischten Sprachen wird dennoch übersetzt. Wenn die erkannte Sprache mit `outputLanguage` übereinstimmt, verarbeitet das Modell trotzdem jedes Element. DeliTools gibt das ursprüngliche Array nicht unverarbeitet zurück.
- Es werden nur Modelle aus der Zulassungsliste des Betreibers akzeptiert. Andere Modelle geben einen Fehler wegen ungültigen Modells zurück. Ein Modell, das im Katalog des Vercel AI Gateway vorhanden ist, ist nicht automatisch zugelassen.
- Der Aufruf ist nur erfolgreich, wenn jedes Element erfolgreich ist. Ein fehlgeschlagenes Element lässt die gesamte Anfrage fehlschlagen. DeliTools gibt niemals Teilübersetzungen zurück und ersetzt bei einem fehlgeschlagenen Element niemals den Quelltext.

## Daten extrahieren

`ExtractDataFromText` liest Text und gibt JSON zurück, das Ihrem JSON-Schema folgt. Der Dienst verwendet ein festes Modell, `gpt-6-luna`. Sie wählen das Modell nicht aus.

| Feld             | Erforderlich | Bedeutung                                                                                                                        |
| ---------------- | ------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Ja           | Der Mandant, dem die Nutzung berechnet wird. Verwenden Sie dessen kanonische UUID Version 7: Kleinbuchstaben, mit Bindestrichen. |
| `text`           | Ja           | Der Quelltext. Er darf nicht leer sein und nicht nur aus Leerraum bestehen.                                                      |
| `jsonSchema`     | Ja           | Ein JSON-Schema als Zeichenfolge, das die unterstützte Teilmenge verwendet.                                                      |
| `idempotencyKey` | Nein         | Siehe Idempotenz.                                                                                                                |

Die Antwort enthält `requestId` und `jsonData`, eine JSON-Zeichenfolge, die DeliTools anhand Ihres Schemas validiert hat.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

So funktioniert die Extraktion:

- Dem Modell wird vorgegeben, nur Informationen zu verwenden, die der Text nennt. Wenn dem Text Informationen fehlen, die das Schema verlangt, schlägt der Aufruf mit einem Fehler wegen nicht möglicher Extraktion fehl. Der Dienst erfindet keinen Wert, um das Schema zu erfüllen.
- DeliTools prüft die Modellausgabe auf dem Server anhand Ihres Schemas. Fehlerhafte oder nicht konforme Ausgabe ist ein Fehlschlag mit einem Fehler wegen ungültiger Ausgabe. Sie wird niemals als Ergebnis zurückgegeben, und DeliTools wiederholt oder repariert sie nicht.
- Eine Schemaprüfung und ein Prompt garantieren nicht, dass jeder Wert semantisch korrekt ist. Prüfen Sie extrahierte Daten, bevor Sie sich darauf verlassen. DeliTools gibt keinen Nachweis für jeden Wert zurück.
- DeliTools prüft das Schema, bevor es ein KI-Modell aufruft. Ein Schema außerhalb der unterstützten Teilmenge gibt einen Fehler wegen ungültigen Schemas zurück, ohne KI-Aufruf und ohne Berechnung.

### Schema-Teilmenge

DeliTools akzeptiert nur den Teil von JSON Schema, den die strukturierte Ausgabe des Extraktionsmodells unterstützt.

Unterstützt:

- Ein Objekt an der Wurzel.
- Die Typen `object`, `array`, `string`, `number`, `integer`, `boolean` und `null`, verschachtelt bis zu einer Tiefe von 8 Ebenen.
- `properties`, wobei jede Eigenschaft in `required` aufgeführt ist.
- `additionalProperties` mit dem Wert `false` bei jedem Objekt.
- `items` für Arrays.
- `description` bei jedem Schema.
- `enum` mit skalaren Werten.

Nicht unterstützt und vor der Ausführung abgelehnt:

- Jedes Schlüsselwort außerhalb der obigen Liste, zum Beispiel `pattern`, `minimum`, `format` oder `oneOf`.
- Optionale Eigenschaften. Jede Eigenschaft eines Objekts muss in `required` stehen.
- `additionalProperties` mit dem Wert `true` oder fehlend.
- Verweise: `$ref` und rekursive Schemas.
- Schemas oberhalb der Grenzwerte des Anbieters: 5.000 Eigenschaften, 1.000 Enum-Werte oder 120.000 Zeichen für Eigenschaftsnamen und Enum-Werte zusammen. Ein Enum mit mehr als 250 Werten darf höchstens 15.000 Zeichen an Zeichenfolgenwerten enthalten.

Ein unterstütztes Schema:

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

Ein nicht unterstütztes Schema, weil `note` optional ist und `pattern` nicht unterstützt wird:

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## Anfrageverhalten

- Beide Methoden sind synchron. Halten Sie die Verbindung offen, bis Sie ein Ergebnis oder einen Fehler erhalten.
- Jeder Erfolg und jeder Fehler enthält eine Anfrage-ID, eine UUID der Version 7. Geben Sie sie an, wenn Sie den Support kontaktieren. Erfolgsantworten enthalten nur das Ergebnis und die Anfrage-ID. Sie enthalten weder das Modell noch Token-Anzahlen oder die Berechnung. Siehe Abrechnung, um die Nutzung einzusehen.
- DeliTools wiederholt vorübergehende Fehler des KI-Anbieters bis zu einem Grenzwert, den der Betreiber festlegt. Es wiederholt keine fehlerhafte Ausgabe und wechselt niemals von sich aus zu einem anderen Modell.
- Wenn Sie den Aufruf abbrechen oder die Verbindung trennen, versucht DeliTools, den KI-Aufruf abzubrechen, und beendet weitere Wiederholungsversuche. Bereits angefallene Nutzung wird dennoch gemessen und abgerechnet.
- Der Betreiber legt die Grenzwerte für Eingabegröße, Anzahl der Übersetzungselemente, Ausführungs-Timeout, Anfragerate pro Mandant und gleichzeitige Anfragen pro Mandant fest. Dies sind Bereitstellungseinstellungen, keine festen Produktgrenzwerte. Eine Anfrage über einem Grenzwert gibt einen Grenzwertfehler zurück.

## Anmeldung und API-Schlüssel

Senden Sie mit jeder Anfrage `Authorization: Bearer <credential>`. Zwei Arten von Anmeldeinformationen funktionieren für beide Werkzeuge:

- **Ein Delino-Anmeldetoken** (ein Logto-Zugriffstoken des Nutzers) handelt als der angemeldete Nutzer.
- **Ein DeliTools-API-Schlüssel** handelt so, wie es seine Schlüsselart beschreibt.

Jede Anfrage wird außerdem anhand Ihres Mandanten in Delcore geprüft. Der Mandant muss aktiv sein und kostenpflichtige Arbeit ausführen dürfen, und die Abrechnung muss dies zulassen. Wenn DeliTools diese Prüfungen nicht abschließen kann, lehnt es neue kostenpflichtige Ausführungen ab.

### Schlüsselarten und Fähigkeiten

- Ein **persönlicher Schlüssel** gehört einem Nutzer. Seine Berechtigungen sind die Schnittmenge aus seinen Fähigkeiten und den aktuellen Berechtigungen dieses Nutzers. Wenn der Nutzer den Zugriff auf den Mandanten verliert, funktioniert der Schlüssel nicht mehr.
- Ein **Dienstschlüssel** gehört dem Mandanten. Er hängt nicht von der Mitgliedschaft des Nutzers ab, der ihn erstellt hat. Nur ein Eigentümer oder Administrator des Mandanten kann Dienstschlüssel erstellen und verwalten.
- Jeder Schlüssel hat eine oder beide Fähigkeiten: `CAPABILITY_TRANSLATOR` und `CAPABILITY_EXTRACTOR`. Ein Schlüssel kann nur die Werkzeuge seiner Fähigkeiten aufrufen.
- Nutzer verwalten ihre eigenen persönlichen Schlüssel. Die Schlüsselverwaltung erfordert ein Delino-Anmeldetoken. Ein API-Schlüssel kann keine Schlüssel erstellen oder widerrufen.

### Schlüssel verwalten

- `CreateKey`: Senden Sie `tenantId`, `kind` (`KEY_KIND_PERSONAL` oder `KEY_KIND_SERVICE`), `capabilities` und ein optionales `expiresAt`.
- `ListKeys`: Listet die Schlüssel eines Mandanten, gefiltert nach `kind`, mit `afterId` und `pageSize` für die Seitenaufteilung.
- `RevokeKey`: Senden Sie `tenantId` und `keyId`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **Einmaliges Geheimnis.** Die Antwort auf `CreateKey` enthält das `secret` einmalig. DeliTools speichert nur einen Verifizierungs-Hash und kann das Geheimnis nicht erneut anzeigen. Wenn Sie einen Schlüssel verlieren, erstellen Sie einen neuen. Bewahren Sie das Geheimnis in einer Geheimnisverwaltung auf.
- **Ablauf.** `expiresAt` ist optional. Ein Schlüssel ohne Ablaufzeit bleibt gültig, bis Sie ihn widerrufen. Ein persönlicher Schlüssel erfordert außerdem, dass sein Besitzer weiterhin Zugriff auf den Mandanten hat. Ein Dienstschlüssel hängt nicht von dem Nutzer ab, der ihn erstellt hat; das Entfernen dieses Nutzers stoppt ihn daher nicht: Widerrufen Sie den Dienstschlüssel oder versehen Sie ihn mit einer Ablaufzeit.
- **Widerruf.** Nachdem Sie einen Schlüssel widerrufen haben oder nachdem der Nutzer eines persönlichen Schlüssels oder Anmeldetokens die Berechtigung verliert, werden neue Anfragen abgelehnt, und ein laufender Aufruf unternimmt keine weiteren KI-Wiederholungsversuche. Ein bereits laufender Aufruf kann abgeschlossen werden. Seine Nutzung wird abgerechnet.

## Abrechnung

DeliTools meldet die Nutzung an Delcore. Delcore bewertet und verrechnet sie in USD.

- **Grundlage.** Die Grundlage sind die öffentlichen Token-Preise des Modells im Modellkatalog des Vercel AI Gateway, multipliziert mit den Token, die der Aufruf tatsächlich gemessen hat. Gateway-Rabatte und Werbeguthaben verringern die Grundlage nicht.
- **Aufschlag.** Die Übersetzung kostet die Grundlage mal 1,15. Die Extraktion kostet die Grundlage mal 1,30. Beträge verwenden exakte Dezimalwerte und die Rundungsregeln von Delcore.
- **Preisänderungen.** DeliTools aktualisiert die Katalogpreise automatisch. Ein neuer Preis gilt für alle Kunden bei Anfragen, die nach seinem Inkrafttreten beginnen. Eine Anfrage behält den Preis ihres Beginns für alle ihre Wiederholungsversuche. Wenn DeliTools für ein Modell keinen Preis hat, lehnt es neue KI-Ausführungen ab.
- **Fehlschläge sind abrechenbar.** Token, die durch Wiederholungsversuche, fehlgeschlagene Aufrufe, Timeouts, Abbrüche und Ergebnisse verbraucht wurden, die Sie nicht empfangen konnten, zählen weiterhin, zusammen mit dem Aufschlag.
- **Unbekannte Nutzung.** Wenn DeliTools die Nutzung eines Aufrufs nicht sofort lesen kann, stellt es die Berechnung zurück und gleicht sie später mit dem Gateway ab. Es schätzt die Nutzung niemals für eine Berechnung. Wenn die Nutzung noch unbekannt ist, wenn der Datensatz nach 30 Tagen abläuft, wird diese Berechnung verworfen.
- **Wo Sie die Nutzung sehen.** Antworten enthalten keine Nutzung. Verwenden Sie die Delino-Abrechnungsoberfläche für Ihren Mandanten, um Nutzung und Berechnungen einzusehen.
- **Gesperrte Mandanten.** Wenn die Abrechnung Ihren Mandanten sperrt, schlagen neue Ausführungen mit einem Fehler wegen gesperrter Abrechnung fehl. Ein bereits laufender Aufruf wird nicht unterbrochen. Da DeliTools die Nutzung nach der Ausführung misst, können gleichzeitige Aufrufe ein Guthaben oder einen Grenzwert überschreiten, bevor die Sperre greift.

## Idempotenz

`idempotencyKey` ist optional. Ohne ihn ist jeder Aufruf eine neue Ausführung und kann eine neue Berechnung verursachen, auch wenn Sie nach einem Netzwerkfehler wiederholen.

Mit einem Schlüssel verfolgt DeliTools die Anfrage anhand von **Mandant, Methode und Schlüssel**. Dies gilt über verschiedene Anmeldeinformationen desselben Mandanten hinweg. DeliTools prüft Ihre aktuelle Berechtigung, bevor es Ihnen den Zustand einer Anfrage mitteilt.

| Situation                                              | Ergebnis                                                                                      |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------------- |
| Neuer Schlüssel                                        | Der Aufruf läuft normal.                                                                      |
| Gleicher Schlüssel und gleiche Eingabe, läuft noch     | Ein Verarbeitungsfehler mit der Anfrage-ID des ersten Aufrufs. Es wird nicht gewartet.        |
| Gleicher Schlüssel und gleiche Eingabe, abgeschlossen  | Ein Fehler wegen bereits verarbeiteter Anfrage. Das Ergebnis wird nicht erneut ausgegeben.    |
| Gleicher Schlüssel und andere Eingabe                  | Ein Eingabekonfliktfehler.                                                                    |
| Der erste Aufruf endete in einem unbekannten Zustand   | Ein Fehler wegen unbekannten Ausgangs. DeliTools wiederholt den KI-Aufruf nicht von sich aus. |
| Gleicher Schlüssel nach Ablauf des 30-Tage-Datensatzes | Ein neuer Aufruf, normal berechnet.                                                           |

DeliTools speichert den Schlüssel, einen Hash der Eingabe und den Zustand. Es speichert weder die Eingabe noch das Ergebnis. Die Deduplizierung gilt nur 30 Tage nach dem ersten Aufruf. Speichern Sie das Ergebnis, wenn Sie es erhalten: Ein wiederholter Aufruf mit demselben Schlüssel kann es nicht erneut zurückgeben.

## Fehler

Jeder Fehler hat einen Connect-Code, einen stabilen Grund, eine Anfrage-ID und eine englische Meldung. Der Grund und die Anfrage-ID befinden sich im Fehlerdetail `delitools.v1.ErrorDetail`. Verwenden Sie in Ihrem Code den Grund, nicht die Meldung.

| Grund                                | Connect-Code          | Bedeutung                                                                                                                                               |
| ------------------------------------ | --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | Die Anfrage ist ungültig, zum Beispiel ein leeres Array oder identische Sprachen.                                                                       |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | Das Schema liegt außerhalb der unterstützten Teilmenge.                                                                                                 |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | Das Modell steht nicht auf der Zulassungsliste.                                                                                                         |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | Die Anmeldeinformation fehlt, ist ungültig, abgelaufen oder widerrufen.                                                                                 |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | Die Anmeldeinformation oder der Nutzer darf dieses Werkzeug in diesem Mandanten nicht verwenden.                                                        |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | Die Abrechnung erlaubt für den Mandanten keine neue kostenpflichtige Ausführung.                                                                        |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | Für das Modell ist kein öffentlicher Preis verfügbar, daher führt DeliTools es nicht aus.                                                               |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | Ein konfigurierter Grenzwert wurde überschritten, etwa Eingabegröße, Elemente, Rate oder Parallelität.                                                  |
| `ERROR_REASON_PROCESSING`            | `aborted`             | Eine Anfrage mit demselben Idempotenzschlüssel läuft noch.                                                                                              |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | Eine Anfrage mit demselben Idempotenzschlüssel und derselben Eingabe ist bereits abgeschlossen.                                                         |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | Der Idempotenzschlüssel wurde mit einer anderen Eingabe verwendet.                                                                                      |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | Der Ausgang der ersten Ausführung ist unbekannt.                                                                                                        |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | Der Text enthält keine Informationen, die das Schema verlangt.                                                                                          |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | Das Modell hat fehlerhafte Ausgabe oder Ausgabe zurückgegeben, die nicht zum Schema passt.                                                              |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | Der KI-Anbieter ist nach den zulässigen Wiederholungsversuchen ausgefallen.                                                                             |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | Das Ausführungs-Timeout ist verstrichen.                                                                                                                |
| `ERROR_REASON_CANCELED`              | `canceled`            | Der Aufruf wurde abgebrochen.                                                                                                                           |
| `ERROR_REASON_INTERNAL`              | `internal`            | Ein interner Fehler, zum Beispiel im Speicher.                                                                                                          |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | Die Autorisierungs- oder Abrechnungsprüfung des Mandanten konnte nicht abgeschlossen werden, daher werden neue kostenpflichtige Ausführungen abgelehnt. |

Behandlung:

- Korrigieren Sie bei den Fehlern `invalid_argument`, `permission_denied`, `unauthenticated` und `failed_precondition` die Anfrage, bevor Sie sie wiederholen. Beheben Sie bei einem Fehler wegen gesperrter Abrechnung die Abrechnungssperre.
- Ein Anbieter-, Timeout- oder interner Fehler kann später wiederholt werden. Anbieter- und Timeout-Fehlschläge können dennoch abgerechnet werden. Verwenden Sie einen Idempotenzschlüssel, um eine zweite Ausführung zu vermeiden, wenn die erste möglicherweise erfolgreich war. Ein Fehler `unavailable` mit `ERROR_REASON_INTERNAL` bedeutet, dass DeliTools die Autorisierungs- oder Abrechnungsprüfung nicht abschließen konnte. Werten Sie Code und Grund gemeinsam aus, weil `ERROR_REASON_INTERNAL` auch mit `internal` auftritt.
- Fehler wegen unbekannten Ausgangs, laufender Verarbeitung und bereits verarbeiteter Anfrage enthalten die Anfrage-ID des ersten Aufrufs. Kontaktieren Sie den Support mit dieser ID, wenn der Ausgang unbekannt bleibt.

## Datenaufbewahrung

DeliTools speichert Ihren Eingabetext, Ihr Schema oder die Ergebnisse niemals. Es hält sie außerdem aus Logs, Traces und Audit-Ereignissen heraus. Anmeldeinformationen und Geheimnisse sind von der Telemetrie ausgeschlossen.

DeliTools speichert nur die Daten, die es benötigt, um den Dienst zu betreiben und abzurechnen: Verifizierungs-Hashes von API-Schlüsseln, wer und welcher Mandant zu einer Anfrage gehört, Eingabe-Hashes für die Idempotenz, Anfragezustände, Versuchskennungen, gemessene Token-Nutzung, Preis-Snapshots sowie sichere Audit- und Betriebsdatensätze.

| Daten                                                | Aufbewahrt für                                   |
| ---------------------------------------------------- | ------------------------------------------------ |
| Idempotenzdatensätze                                 | 30 Tage ab der ersten Erstellung                 |
| Datensätze zur Zustellung und Abstimmung der Nutzung | 30 Tage ab der ersten Erstellung                 |
| Plattform-Auditdatensätze und Betriebslogs           | 30 Tage ab der Erstellung                        |
| API-Schlüssel                                        | Nicht durch die 30-Tage-Aufbewahrung gelöscht    |
| Angenommene Buchungseinträge in Delcore              | Nach den eigenen Aufbewahrungsregeln von Delcore |

Die 30-Tage-Aufbewahrung gilt nicht für API-Schlüssel. Die Aufbewahrungsbereinigung löscht sie nie. Ablauf und Widerruf verhindern nur, dass sich ein Schlüssel authentifiziert; sein Verifizierungs-Hash und seine Zuordnung bleiben gespeichert.

Die 30 Tage beginnen, wenn der Datensatz erstmals erstellt wird. Wiederholungsversuche und Zustandsänderungen verlängern sie nicht. DeliTools löscht einen Datensatz, wenn er in jedem Zustand abläuft, einschließlich nicht abgeschlossener Anfragen und noch nicht zugestellter Nutzung. Sie akzeptieren zwei Risiken:

- Nachdem ein Idempotenzdatensatz abgelaufen ist, wird ein wiederholter Aufruf mit demselben Schlüssel ausgeführt und erneut berechnet.
- Nutzung, die beim Ablauf ihres Datensatzes noch aussteht, geht verloren, und DeliTools berechnet sie nicht.

## Datenverarbeitung durch externe Anbieter

Um seine Arbeit zu erledigen, sendet DeliTools Ihren Text und bei der Extraktion Ihr Schema an das Vercel AI Gateway und von dort an den Anbieter des Modells, das Ihre Anfrage ausführt. Diese Dienste verarbeiten Daten nach ihren eigenen Richtlinien. Lesen Sie diese, bevor Sie Daten an DeliTools senden:

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models): Jede Modellseite listet ihre Anbieter auf.
- Die Datenrichtlinie des Anbieters des Modells, das Sie verwenden.

DeliTools verspricht bei keinem Anbieter eine Datenaufbewahrung von null. Es erkennt keine sensiblen Daten und entfernt sie nicht aus Ihrer Eingabe. Übermitteln Sie keine Daten, die Sie nicht mit diesen Anbietern teilen dürfen.

## Grenzwerte

Der Betreiber des DeliTools-API-Servers legt die Grenzwerte für Eingabegröße, die Anzahl der Elemente einer Übersetzung, das Ausführungs-Timeout, Anfragen pro Minute und Mandant, gleichzeitige Anfragen pro Mandant, die Anzahl der KI-Wiederholungsversuche und die Zulassungsliste der Übersetzungsmodelle fest. Dies sind Bereitstellungseinstellungen und können sich ändern. Sie sind keine festen Produktgrenzwerte, und DeliTools gibt keine Garantie für Durchsatz oder Verfügbarkeit. Eine Anfrage, die einen Grenzwert überschreitet, gibt `ERROR_REASON_LIMIT` zurück oder, bei einem ungültigen Modell, `ERROR_REASON_INVALID_MODEL`. Fragen Sie den Betreiber Ihrer Bereitstellung nach den aktuellen Werten.
