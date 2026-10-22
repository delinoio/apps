# DeliTools

DeliTools bietet zwei kostenpflichtige APIs für Entwickler: die Übersetzung einer Liste von Texten und die Extraktion eines JSON-Werts aus Text. Sie rufen sie mit einem beliebigen Connect-Client oder mit einfachem JSON über HTTP aus Ihrem eigenen serverseitigen Code auf. Delcore verwaltet Ihre API-Schlüssel, Berechtigungen und Abrechnung, sodass Sie für alles dasselbe Delino-Konto und denselben Mandanten verwenden.

## Konzepte

- **Übersetzung:** Senden Sie eine Liste von Texten, eine Zielsprache und ein Modell. Sie erhalten die Übersetzungen in derselben Reihenfolge und Anzahl.
- **Extraktion:** Senden Sie einen Text und ein JSON-Schema. Sie erhalten ein JSON-Dokument, das dem Schema entspricht.
- **Mandant:** ein persönlicher Bereich oder eine Organisation in Delino. Delcore rechnet die Nutzung über den Mandanten ab.
- **Zugangsdaten:** ein Delino-Anmeldetoken, ein persönlicher API-Schlüssel oder ein Dienst-API-Schlüssel für einen Mandanten. Siehe [API-Schlüssel](/de/services/delitools/api-keys).
- **Anfrage-ID:** Jede Antwort enthält eine Anfrage-ID. Geben Sie diese an, wenn Sie den Support kontaktieren.

## Anmeldung und Berechtigungen

Senden Sie bei jeder Anfrage einen Header `Authorization: Bearer <credential>`.

| Zugangsdaten               | Format                                         | Mandant                                                            |
| -------------------------- | ---------------------------------------------- | ------------------------------------------------------------------ |
| Delino-Anmeldetoken        | Ein Delino-Zugriffstoken                       | Setzen Sie `tenantId` in der Anfrage. Die Angabe ist erforderlich. |
| Persönlicher API-Schlüssel | Ein geheimer Schlüssel, der mit `dlk_` beginnt | Der Mandant des Schlüssels. `tenantId` ist optional.               |
| Mandanten-Dienstschlüssel  | Ein geheimer Schlüssel, der mit `dlk_` beginnt | Der Mandant des Schlüssels. `tenantId` ist optional.               |

Wenn Sie `tenantId` zusammen mit einem API-Schlüssel setzen, muss der Wert dem Mandanten dieses Schlüssels entsprechen. Andernfalls schlägt die Anfrage mit `ERROR_REASON_PERMISSION_DENIED` fehl. Jede Anfrage muss zum jeweiligen Zeitpunkt von Delcore zugelassen werden: Die Zugangsdaten benötigen die Berechtigung `translate` oder `extract`, der Mandant muss aktiv sein und die üblichen Abrechnungsprüfungen bestehen. Diese APIs sind kostenpflichtig, und Delcore sperrt sie, wenn Ihr Mandant ein Abrechnungsproblem hat, etwa ein aufgebrauchtes Guthaben.

## Schnellstart

1. **Besorgen Sie sich Zugangsdaten.** Verwenden Sie Ihr Delino-Anmeldetoken oder erstellen Sie einen API-Schlüssel, wie unter [API-Schlüssel](/de/services/delitools/api-keys) beschrieben.
2. **Ermitteln Sie Ihre Mandanten-ID und ein Modell.** Die Mandanten-ID ist Teil Ihres Delino-Kontos. Der Dienstbetreiber legt fest, welche Übersetzungsmodelle verfügbar sind.
3. **Rufen Sie die API auf.** Alle Prozeduren befinden sich unter `delitools.v1.DelitoolsService`. Setzen Sie `DELITOOLS_URL` auf die Basis-URL des DeliTools-API-Servers, zum Beispiel `http://127.0.0.1:46322` für einen lokalen Server.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

In JSON verwenden Feldnamen lowerCamelCase. In der [API-Referenz](/de/services/delitools/api-reference) finden Sie alle Felder, die unterstützte Schema-Teilmenge und die Fehlerursachen.

Rufen Sie DeliTools aus serverseitigem Code oder über die Kommandozeile auf. Fügen Sie keinen API-Schlüssel in Frontend-Code ein: DeliTools unterstützt keine Aufrufe aus Browsern.

## Ihre Inhalte und der KI-Anbieter

DeliTools führt Ihre Anfrage über Vercel AI Gateway aus. **Die von Ihnen übermittelten Texte und Schemas werden an Vercel AI Gateway und an den Modellanbieter hinter dem von Ihnen gewählten Modell gesendet.** Wie diese Anbieter mit Ihren Inhalten umgehen, einschließlich einer etwaigen Speicherung, richtet sich nach deren eigenen Bedingungen und Einstellungen, nicht nach denen von DeliTools.

- DeliTools speichert Ihre Texte, Schemas oder Ergebnisse nicht und schreibt sie nicht in seine Protokolle. Dies beschreibt ausschließlich DeliTools. Es ist keine Garantie dafür, was Vercel AI Gateway oder der Modellanbieter aufbewahrt.
- Übermitteln Sie keine Inhalte, die Sie nicht mit diesen Anbietern teilen dürfen. Prüfen Sie deren Bedingungen, bevor Sie sensible oder regulierte Daten übermitteln.
- DeliTools bewahrt nur die Nutzungsaufzeichnungen auf, die für die Abrechnung Ihres Mandanten benötigt werden, und entfernt sie 30 Tage nach ihrer Erstellung.
- Es wird nicht garantiert, dass die Modellausgabe korrekt ist. Prüfen Sie die Ergebnisse, bevor Sie sich darauf verlassen.

## Weitere Leitfäden

- [API-Referenz](/de/services/delitools/api-reference): Felder, Sprach-Tags, Schema-Teilmenge, Fehler und Limits.
- [API-Schlüssel](/de/services/delitools/api-keys): persönliche Schlüssel und Mandantenschlüssel, Berechtigungen, Ablauf und Widerruf.
- [Abrechnung](/de/services/delitools/billing): wie aus der Token-Nutzung Kosten entstehen.
- [Fehlerbehebung](/de/services/delitools/troubleshooting): Client-Konfiguration und Behebung von Fehlern.
