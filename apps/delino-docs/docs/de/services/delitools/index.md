# DeliTools

DeliTools bietet zwei kostenpflichtige APIs für Entwickler: die Übersetzung einer Liste von Texten und die Extraktion eines JSON-Werts aus Text. Sie rufen sie aus Ihrem eigenen serverseitigen Code mit einem beliebigen Connect-Client oder mit einfachem JSON über HTTP auf. Delcore verwaltet Ihre API-Schlüssel, Berechtigungen und Abrechnung, sodass Sie für alles dasselbe Delino-Konto und denselben Mandanten verwenden.

## Konzepte

- **Übersetzung:** Senden Sie eine Liste von Texten, eine Zielsprache und ein Modell. Sie erhalten die Übersetzungen in derselben Reihenfolge und Anzahl.
- **Extraktion:** Senden Sie einen Text und ein JSON Schema. Sie erhalten ein JSON-Dokument, das dem Schema entspricht.
- **Mandant:** ein persönlicher Bereich oder eine Organisation in Delino. Delcore rechnet die Nutzung über den Mandanten ab.
- **Zugangsdaten:** ein Delino-Anmeldetoken, ein persönlicher API-Schlüssel oder ein Dienst-API-Schlüssel eines Mandanten. Siehe [API-Schlüssel](/de/services/delitools/api-keys).
- **Anfrage-ID:** Jede Antwort enthält eine Anfrage-ID. Geben Sie diese an, wenn Sie den Support kontaktieren.

## Anmeldung und Berechtigungen

Senden Sie bei jeder Anfrage einen `Authorization: Bearer <credential>`-Header mit.

| Zugangsdaten                    | Format                                         | Mandant                                                          |
| ------------------------------- | ---------------------------------------------- | ---------------------------------------------------------------- |
| Delino-Anmeldetoken             | Ein Delino-Zugriffstoken                       | Setzen Sie `tenantId` in der Anfrage. Das Feld ist erforderlich. |
| Persönlicher API-Schlüssel      | Ein geheimer Schlüssel, der mit `dlk_` beginnt | Der Mandant des Schlüssels. `tenantId` ist optional.             |
| Dienstschlüssel eines Mandanten | Ein geheimer Schlüssel, der mit `dlk_` beginnt | Der Mandant des Schlüssels. `tenantId` ist optional.             |

Wenn Sie `tenantId` zusammen mit einem API-Schlüssel angeben, muss der Wert dem Mandanten dieses Schlüssels entsprechen. Andernfalls schlägt die Anfrage mit `ERROR_REASON_PERMISSION_DENIED` fehl. Jede Anfrage muss zum Zeitpunkt ihrer Ausführung von Delcore zugelassen werden: Die Zugangsdaten benötigen die Berechtigung `translate` oder `extract`, der Mandant muss aktiv sein und die üblichen Abrechnungsprüfungen bestehen. Diese APIs sind kostenpflichtig. Delcore sperrt sie, wenn bei Ihrem Mandanten ein Abrechnungsproblem vorliegt, etwa ein aufgebrauchtes Guthaben.

## Schnellstart

1. **Beschaffen Sie sich Zugangsdaten.** Verwenden Sie Ihr Delino-Anmeldetoken oder erstellen Sie einen API-Schlüssel wie unter [API-Schlüssel](/de/services/delitools/api-keys) beschrieben.
2. **Ermitteln Sie Ihre Mandanten-ID und ein Modell.** Die Mandanten-ID ist Teil Ihres Delino-Kontos. Der Dienstbetreiber legt fest, welche Übersetzungsmodelle verfügbar sind.
3. **Rufen Sie die API auf.** Alle Prozeduren gehören zu `delitools.v1.DelitoolsService`. Setzen Sie `DELITOOLS_URL` auf die Basis-URL des DeliTools-API-Servers, die Sie von Ihrem Dienstbetreiber erhalten.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

Dieses Beispiel verwendet einen API-Schlüssel, der seinen Mandanten bereits mitführt. Bei einem Delino-Anmeldetoken fügen Sie `"tenantId":"<tenant-id>"` zum JSON-Body hinzu.

In JSON verwenden Feldnamen lowerCamelCase. Alle Felder, die unterstützte Teilmenge von JSON Schema und die Fehlergründe finden Sie in der [API-Referenz](/de/services/delitools/api-reference).

Rufen Sie DeliTools aus serverseitigem Code oder über die Befehlszeile auf. Hinterlegen Sie keinen API-Schlüssel im Frontend-Code: DeliTools unterstützt keine Aufrufe aus Browsern.

## Ihre Inhalte und der KI-Anbieter

DeliTools führt Ihre Anfrage über Vercel AI Gateway aus. **Die von Ihnen übermittelten Texte und Schemas werden an Vercel AI Gateway und an den Modellanbieter hinter dem von Ihnen gewählten Modell gesendet.** Der Umgang mit Ihren Inhalten, einschließlich einer etwaigen Speicherung, richtet sich nach deren eigenen Bedingungen und Einstellungen, nicht nach denen von DeliTools.

- DeliTools speichert Ihre Texte, Schemas oder Ergebnisse nicht und schreibt sie nicht in seine Protokolle. Diese Aussage bezieht sich ausschließlich auf DeliTools. Sie ist keine Garantie dafür, welche Daten Vercel AI Gateway oder der Modellanbieter aufbewahrt.
- Übermitteln Sie keine Inhalte, die Sie nicht mit diesen Anbietern teilen dürfen. Prüfen Sie deren Bedingungen, bevor Sie sensible oder regulierte Daten übermitteln.
- DeliTools bewahrt nur die Nutzungsdatensätze auf, die für die Abrechnung Ihres Mandanten erforderlich sind, und entfernt sie 30 Tage nach ihrer Erstellung.
- Die Korrektheit der Modellausgabe ist nicht garantiert. Prüfen Sie die Ergebnisse, bevor Sie sich darauf verlassen.

## Weitere Anleitungen

- [API-Referenz](/de/services/delitools/api-reference): Felder, Sprach-Tags, unterstützte Teilmenge von JSON Schema, Fehler und Grenzwerte.
- [API-Schlüssel](/de/services/delitools/api-keys): persönliche Schlüssel und Mandantenschlüssel, Berechtigungen, Ablauf und Widerruf.
- [Abrechnung](/de/services/delitools/billing): wie aus der Token-Nutzung Gebühren entstehen.
- [Fehlerbehebung](/de/services/delitools/troubleshooting): Client-Konfiguration und Behebung von Fehlern.
