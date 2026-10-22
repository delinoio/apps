# DeliTools-Abrechnung

DeliTools stellt Ihrem Mandanten die KI-Tokens in Rechnung, die Ihre Anfragen verbrauchen. Delcore berechnet die Kosten für die Nutzung und verwaltet Ihren Kontostand, Ihre Gutschriften und Rechnungen. Diese Seite erklärt, wie aus der Nutzung eine Gebühr entsteht.

## Was gemessen wird

- DeliTools liest die Gesamtzahlen der **Eingabe-Tokens** und **Ausgabe-Tokens**, die der KI-Anbieter für jeden Versuch meldet. Es verwendet diese Gesamtzahlen so, wie sie gemeldet werden.
- Die gemeldeten Gesamtzahlen enthalten bereits alle Cache- oder Reasoning-Tokens. DeliTools addiert diese Anteile nicht erneut, gewährt keinen separaten Cache-Rabatt und erhebt keinen Aufschlag für Reasoning.
- DeliTools meldet die Eingabe- und Ausgabenutzung getrennt an Delcore, und Delcore berechnet sie jeweils mit eigenen Preisen.

## Preise

- Der Dienstbetreiber legt in Delcore für jedes Modell einen Preis pro Token-Richtung (Eingabe und Ausgabe) fest. Verschiedene Modelle können unterschiedliche Preise haben.
- Für Übersetzung und Extraktion gelten dieselben Preise. Die Extraktion verwendet immer `openai/gpt-6-luna`, daher werden für ihre Abrechnung die Eingabe- und Ausgabepreise dieses Modells verwendet.
- Ein Modell, für das nicht beide Preise hinterlegt sind, kann nicht ausgeführt werden. Die Anfrage schlägt mit `ERROR_REASON_MODEL_UNAVAILABLE` fehl, und es wird kein anderes Modell als Ersatz verwendet.
- Eine Preisänderung gilt für Ausführungen, die nach der Änderung starten. Für bereits gestartete Nutzung gilt weiterhin der Preis zum Startzeitpunkt.
- Ihr Tarif bestimmt die enthaltene Nutzung, Ihren Kontostand und ob Sie nach Verbrauch des Guthabens fortfahren können. Ihren Tarif und Ihre Rechnungen finden Sie in Delcore.

## Was berechnet wird

- **Jeder Versuch mit bekannter Nutzung wird berechnet.** Dazu gehören auch Versuche, deren Ergebnis ungültig ist, deren Extraktion fehlschlägt, deren Antwort vor Ihrem Abbruch eingetroffen ist oder deren Ergebnis Sie nie erreicht.
- **Wiederholungsversuche sind eigenständige Versuche.** Ein vorübergehender Anbieterfehler (ein Netzwerkfehler oder eine HTTP-Antwort mit Status 5xx oder 429) liefert keine Nutzungsdaten zurück. Daher wiederholt DeliTools den Versuch, ohne den fehlgeschlagenen Versuch zu berechnen. Eine abgeschlossene Anbieterantwort mit bekannter Nutzung beendet die Versuchsschleife und wird berechnet, auch wenn ihre Ausgabe ungültig ist, da DeliTools niemals einen erneuten Versuch unternimmt, um eine ungültige Ausgabe zu korrigieren. Eine Antwort ohne Nutzungsdaten kostet nichts.
- **Unbekannte Nutzung wird nicht berechnet.** Wenn der Anbieter keine verwertbaren Token-Gesamtzahlen meldet, schätzt DeliTools keine Gebühr und fragt die Nutzung auch später nicht ab.
- **Abgebrochene Aufrufe werden nicht berechnet.** Wenn Sie abbrechen oder das Ausführungszeitlimit abläuft, während der Anbieteraufruf noch läuft, geht kein Nutzungsbericht ein und es wird nichts berechnet, selbst wenn der Anbieter bereits mit der Arbeit begonnen hatte. Dies ist eine bekannte Einschränkung: DeliTools fragt diese Nutzung im Nachhinein nicht ab.
- **Die Nutzung muss erfasst werden.** DeliTools speichert die Nutzung jedes Versuchs, bevor es antwortet. Wenn es sie nicht speichern kann, schlägt die Anfrage mit `ERROR_REASON_DEPENDENCY_UNAVAILABLE` fehl und Sie erhalten kein Ergebnis.
- **Abgelehnte Anfragen werden nicht berechnet.** Anfragen, die bei der Validierung, Authentifizierung oder Autorisierung scheitern oder an ein Limit oder ein nicht unterstütztes Schema stoßen, erreichen den KI-Anbieter nie.
- **Jeder Aufruf ist neu.** DeliTools bietet weder einen Idempotenzschlüssel noch die erneute Auslieferung gespeicherter Ergebnisse. Wenn Sie dieselbe Anfrage erneut senden, wird sie erneut ausgeführt und erneut berechnet.

## Wann Gebühren erscheinen

DeliTools speichert die Nutzung jedes Versuchs, bevor es Ihnen antwortet, und meldet sie anschließend an Delcore. Die Nutzungsmeldung erfolgt getrennt von Ihrer Anfrage, sodass eine Gebühr kurz nach dem Aufruf in Delcore erscheinen kann. Wenn Delcore nicht verfügbar ist, versucht DeliTools die Übermittlung mit denselben Kennungen weiter, sodass Wiederholungsversuche keine doppelten Gebühren erzeugen.

Nutzungsdatensätze, die DeliTools nicht zustellen kann, werden 30 Tage nach ihrer Erstellung gelöscht. Wiederholungsversuche verlängern diesen Zeitraum nicht. Nutzung, die innerhalb dieses Zeitraums nie übermittelt wurde, wird nicht berechnet. DeliTools schätzt sie nicht.

Sie können Ihre Nutzung und Gebühren in Delcore einsehen. Die Antworten von DeliTools enthalten diese Angaben nicht.

## Guthabenprüfungen und Überschreitung des Guthabens

Delcore prüft Ihre Berechtigung und Ihren Abrechnungsstatus vor jeder Anfrage und erneut vor jedem Wiederholungsversuch. Es reserviert weder Geld noch Tokens für den Aufruf. Die Kosten stehen erst nach Abschluss des Aufrufs fest. Mehrere gleichzeitig laufende Anfragen können daher mehr als Ihr verbleibendes Guthaben oder Ihre enthaltene Nutzung verbrauchen. Wenn das Guthaben oder ein Limit ausgeschöpft ist, blockiert Delcore spätere Anfragen mit `ERROR_REASON_BILLING_BLOCKED`.

Wenn Delcore nicht erreichbar ist, lehnt DeliTools neue kostenpflichtige Anfragen ab, anstatt sie auszuführen.

## Blockierte Anfragen

`ERROR_REASON_BILLING_BLOCKED` bedeutet, dass Delcore die kostenpflichtige Nutzung für Ihren Mandanten blockiert hat. Häufige Ursachen sind ein aufgebrauchtes Guthaben, eine fehlgeschlagene Verlängerung oder ein abgelaufener bezahlter Zeitraum, ein offener Streitfall, eine Sperre durch den Betreiber oder ein Mandant, der gerade gelöscht wird. Beheben Sie die Ursache in Delcore. Laden Sie beispielsweise Ihr Guthaben auf oder verlängern Sie Ihr Abonnement. Die Mandantenverwaltung und die Behebung von Zahlungsproblemen bleiben verfügbar, während die kostenpflichtige Nutzung blockiert ist.
