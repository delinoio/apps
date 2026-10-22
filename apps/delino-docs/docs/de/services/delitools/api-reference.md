# DeliTools-API-Referenz

Diese Seite beschreibt die beiden DeliTools-Methoden. Beide sind Connect-Prozeduren auf `delitools.v1.DelitoolsService` und akzeptieren JSON über HTTP `POST`. Feldnamen verwenden in JSON lowerCamelCase. Senden Sie `Content-Type: application/json` und `Authorization: Bearer <credential>`. Informationen zu Zugangsdaten und Mandanten finden Sie unter [DeliTools](/de/services/delitools/).

Jede Antwort enthält eine Anfrage-ID, sowohl im Antworttext als auch im Header `X-Delitools-Request-Id`. Antworten enthalten weder Token-Verbrauch noch Gebühren. Diese können Sie aus Delcore abrufen; siehe [Abrechnung](/de/services/delitools/billing).

Jeder Aufruf ist eine neue Ausführung. DeliTools hat keinen Idempotenzschlüssel und speichert keine Ergebnisse. Wenn Sie eine Anfrage wiederholen, wird sie erneut ausgeführt und kann erneut berechnet werden.

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| Feld             | Typ                     | Regeln                                                                                                                             |
| ---------------- | ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Zeichenfolge            | Für ein Delino-Anmeldetoken erforderlich. Bei einem API-Schlüssel leer oder der Mandant des Schlüssels.                            |
| `texts`          | Array von Zeichenfolgen | Erforderlich. Mindestens ein Element. Keine leere Zeichenfolge. Anzahl der Elemente und Gesamtgröße werden vom Betreiber begrenzt. |
| `targetLanguage` | Zeichenfolge            | Erforderlich. Ein BCP-47-Sprachtag, beispielsweise `fr` oder `pt-BR`.                                                              |
| `sourceLanguage` | Zeichenfolge            | Optional. Ein BCP-47-Sprachtag. Wenn es weggelassen wird, erkennt das Modell die Ausgangssprache.                                  |
| `model`          | Zeichenfolge            | Erforderlich. Eine vom Betreiber zugelassene Modellkennung.                                                                        |

Die Antwort enthält `requestId` und `translations`. `translations` enthält dieselbe Anzahl von Elementen in derselben Reihenfolge wie `texts`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"sourceLanguage":"en","targetLanguage":"fr","model":"<model-id>"}'
```

```json
{
  "requestId": "<request-id>",
  "translations": ["Bonjour", "Bonjour, bonne journée"]
}
```

- **Sprachtags.** DeliTools prüft ohne Berücksichtigung der Groß- und Kleinschreibung, ob ein Tag gemäß BCP 47 korrekt aufgebaut ist, höchstens 64 Zeichen umfasst und eine primäre Sprachkennung aus zwei oder drei Buchstaben enthält. Tags, die nur aus Subtags für den privaten Gebrauch bestehen, sowie bestandsgeschützte Tags werden abgelehnt. DeliTools prüft nicht, ob das Modell die Sprache unterstützt.
- **Alles oder nichts.** Das Ergebnis ist atomar. Wenn ein Element fehlschlägt, schlägt die gesamte Anfrage fehl und Sie erhalten keine Teilübersetzungen.
- **Keine Sonderfälle.** Eine ausgelassene Ausgangssprache, mehrsprachige Eingaben und Text, der bereits in der Zielsprache vorliegt, werden alle an das Modell übergeben. DeliTools lehnt sie nicht ab und kopiert den Ausgangstext nicht für Sie.
- **Modell.** Nur Modelle, die der Betreiber zulässt und für die Preise in Delcore hinterlegt sind, funktionieren. Es wird niemals ersatzweise ein anderes Modell verwendet. Fragen Sie den Dienstbetreiber, welche Modellkennungen verfügbar sind.

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| Feld         | Typ          | Regeln                                                                                                            |
| ------------ | ------------ | ----------------------------------------------------------------------------------------------------------------- |
| `tenantId`   | Zeichenfolge | Für ein Delino-Anmeldetoken erforderlich. Bei einem API-Schlüssel leer oder der Mandant des Schlüssels.           |
| `text`       | Zeichenfolge | Erforderlich und nicht leer. Die Größe wird vom Betreiber begrenzt.                                               |
| `jsonSchema` | Zeichenfolge | Erforderlich. Ein JSON-Schema-Dokument als JSON-Zeichenfolge, das nur die unten unterstützte Teilmenge verwendet. |

Das Modell ist fest auf `openai/gpt-6-luna` eingestellt. Sie können es nicht auswählen. Die Antwort enthält `requestId` und `json`, eine JSON-Zeichenfolge, die Ihrem Schema entspricht.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Extract" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"text":"Please send three boxes of green tea.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"},\"quantity\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"item\",\"quantity\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "<request-id>",
  "json": "{\"item\":\"green tea\",\"quantity\":3}"
}
```

- **Auf Ihren Text gestützt.** Das Modell muss Werte aus dem Text übernehmen. Wenn dem Text erforderliche Informationen fehlen, schlägt die Anfrage mit `ERROR_REASON_EXTRACTION_FAILED` fehl. DeliTools erfindet keine Werte.
- **Geprüfte Ausgabe.** DeliTools validiert das Ergebnis anhand Ihres Schemas. Eine fehlerhaft formatierte oder nicht konforme Ausgabe führt zu `ERROR_REASON_INVALID_OUTPUT`. DeliTools unternimmt keine weiteren Versuche, um ungültige Ausgaben zu reparieren.
- **Genauigkeit.** Ein Ergebnis, das dem Schema entspricht, kann dennoch falsch sein. Überprüfen Sie Ergebnisse, auf die es ankommt.
- **Nicht unterstützte Schemas werden zuerst abgewiesen.** DeliTools lehnt ein Schema außerhalb der unterstützten Teilmenge ab, bevor es das Modell ausführt. Eine solche Anfrage wird daher nicht berechnet.

### Unterstützte JSON-Schema-Teilmenge

Das Schema wird als JSON Schema 2020-12 gelesen. Es muss ein JSON-Objekt sein. `$schema` ist optional und nur auf der obersten Ebene zulässig, wo DeliTools seinen Wert ignoriert. Jedes Schema, einschließlich des Schemas auf der obersten Ebene, jedes Eigenschaftsschemas und des `items`-Schemas, benötigt `type`, `enum` oder `const`. Ein Schema darf höchstens 8 Ebenen tief sein und 256 Schemaknoten enthalten; seine Größe wird vom Betreiber begrenzt.

| Schlüsselwort            | Regel                                                                                                                                                                                    |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null` oder ein Array dieser Typen, beispielsweise `["string","null"]` für einen Wert, der null sein darf.                  |
| `properties`, `required` | `properties` ordnet Namen Schemas zu. Jeder Name in `required` muss in `properties` vorkommen. Höchstens 64 Eigenschaften pro Objekt; Namen in `required` dürfen sich nicht wiederholen. |
| `additionalProperties`   | Nur `true` oder `false` (kein Schema).                                                                                                                                                   |
| `items`                  | Ein Schema für alle Elemente.                                                                                                                                                            |
| `enum`, `const`          | `enum` enthält 1 bis 64 unterschiedliche Werte. `const` ist ein einzelner Wert.                                                                                                          |
| `minLength`, `maxLength` | Ganzzahlen von 0 bis 1.000.000 für Zeichenfolgen. Das Minimum darf das Maximum nicht überschreiten.                                                                                      |
| `minItems`, `maxItems`   | Ganzzahlen von 0 bis 1.000.000 für Arrays. Das Minimum darf das Maximum nicht überschreiten.                                                                                             |
| `minimum`, `maximum`     | Endliche Zahlen für `number` und `integer`. Das Minimum darf das Maximum nicht überschreiten.                                                                                            |
| `title`, `description`   | Hinweise für das Modell. Sie ändern die Validierung nicht.                                                                                                                               |

Folgendes wird nicht unterstützt und führt zu `ERROR_REASON_UNSUPPORTED_SCHEMA`: `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, `unevaluated`-Schlüsselwörter, boolesche Schemas, unbekannte Schlüsselwörter sowie Schemas, die zu groß oder zu tief verschachtelt sind. DeliTools folgt niemals Referenzen, daher kann ein Schema nicht bewirken, dass DeliTools etwas abruft.

Setzen Sie bei Objekten `"additionalProperties": false`, wenn Sie nur die von Ihnen aufgeführten Eigenschaften zulassen möchten.

## Fehler

Fehler verwenden einen standardmäßigen Connect-Code. Die Fehlerdetails enthalten einen stabilen Fehlergrund und die Anfrage-ID. Meldungen sind auf Englisch und enthalten niemals Ihre Inhalte.

| Fehlergrund                           | Connect-Code                                  | Bedeutung                                                                                                                                                                                                                                     |
| ------------------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                            | Ein Feld ist ungültig: leeres Array oder leere Zeichenfolge, ungültiger Sprachtag, fehlender oder fehlerhaft angegebener Mandant, fehlendes oder nicht zugelassenes Modell oder ein Schema, das kein gültiges JSON ist.                       |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                             | Die Zugangsdaten fehlen, sind fehlerhaft formatiert, abgelaufen, widerrufen oder unbekannt.                                                                                                                                                   |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                           | Den Zugangsdaten fehlt die erforderliche Funktion, die Schlüsselberechtigung oder Ihre Mandantenberechtigung, oder die gesendete `tenantId` ist nicht der Mandant des Schlüssels.                                                             |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                         | Delcore hat die kostenpflichtige Nutzung blockiert, beispielsweise aufgrund des Guthabens, einer Verlängerung, eines Streitfalls, einer Betreibersperre oder eines Mandanten, der gelöscht wird.                                              |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                          | Die Eingabe, die Anzahl der Elemente oder das Schema überschreitet die vom Betreiber festgelegten Grenzen.                                                                                                                                    |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                          | Ihr Mandant hat sein Limit für die Anfragerate oder gleichzeitige Ausführungen erreicht. Versuchen Sie es später erneut. Ein Fehler bezüglich der Anfragerate enthält einen `Retry-After`-Header.                                             |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` oder `failed_precondition` | Das Modell ist nicht zugelassen (`invalid_argument`) oder es sind keine Eingabe- und Ausgabepreise dafür hinterlegt (`failed_precondition`). Es wird nicht ersatzweise ein anderes Modell verwendet.                                          |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                            | Das Schema verwendet ein Schlüsselwort außerhalb der unterstützten Teilmenge.                                                                                                                                                                 |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` oder `internal`                 | Der KI-Anbieter meldete auch nach den zulässigen Wiederholungsversuchen weiterhin Fehler (`unavailable`) oder hat die Anfrage abgelehnt (`internal`).                                                                                         |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                    | Die Modellausgabe war fehlerhaft formatiert, abgeschnitten oder entsprach nicht den Vorgaben.                                                                                                                                                 |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                         | Der Text enthält nicht die erforderlichen Informationen.                                                                                                                                                                                      |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                           | Das vom Betreiber festgelegte Ausführungszeitlimit ist abgelaufen. Es umfasst die Prüfungen und alle Versuche.                                                                                                                                |
| `ERROR_REASON_CANCELED`               | `canceled`                                    | Die Anfrage wurde abgebrochen oder die Verbindung geschlossen.                                                                                                                                                                                |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                                 | Eine Autorisierungs-, Preis- oder Abrechnungsprüfung konnte nicht abgeschlossen werden oder DeliTools konnte die Nutzung des Aufrufs nicht erfassen. Die kostenpflichtige Nutzung wird im Fehlerfall gesperrt und Sie erhalten kein Ergebnis. |

Eine Anfrage, die das vom Betreiber festgelegte Gesamtgrößenlimit für Anfragen überschreitet, kann einen einfachen `resource_exhausted`-Fehler ohne diese Details erhalten. Was Sie tun können, erfahren Sie unter [Fehlerbehebung](/de/services/delitools/troubleshooting).

## Limits und Wiederholungsversuche

Der Betreiber konfiguriert die folgenden Limits, die sich ändern können: Eingabegröße, Anzahl der Übersetzungselemente, Schemagröße und -tiefe, Ausführungszeitlimit, Anfragerate und zulässige kurzfristige Lastspitzen pro Mandant, gleichzeitige Ausführungen pro Mandant sowie die maximale Anzahl von Wiederholungsversuchen bei vorübergehenden Fehlern. Wenden Sie sich an den Dienstbetreiber, um die für Sie geltenden Werte zu erfahren. Ein Limit ist eine Schutzmaßnahme, keine Durchsatzgarantie.

- DeliTools wiederholt nur Aufrufe mit vorübergehenden Fehlern des KI-Anbieters, etwa Netzwerkfehlern, Serverfehlern und Antworten wegen überschrittener Ratenlimits, bis zur konfigurierten Anzahl. Es wiederholt niemals Aufrufe, um ungültige Ausgaben zu reparieren.
- Vor jedem Wiederholungsversuch prüft DeliTools Ihre Autorisierung erneut. Wenn Ihr Schlüssel widerrufen wurde oder abgelaufen ist, Sie Ihre Berechtigung verloren haben oder die Abrechnung blockiert ist, stoppt DeliTools und gibt den entsprechenden Fehler zurück. Ein bereits laufender Anbieteraufruf kann abgeschlossen werden.
- Wenn Sie die Anfrage abbrechen oder die Verbindung schließen, bricht DeliTools den Anbieteraufruf ab und beendet die Wiederholungsversuche.
- Wenn Ihr Client den Aufruf schließt oder sein eigenes Zeitlimit ihn beendet, behandelt DeliTools dies als Abbruch.
- Das Ausführungszeitlimit des Betreibers umfasst die Autorisierung, die Validierung und jeden Versuch. Wenn es abläuft, bricht DeliTools den Anbieteraufruf ab und gibt `ERROR_REASON_TIMEOUT` zurück.
