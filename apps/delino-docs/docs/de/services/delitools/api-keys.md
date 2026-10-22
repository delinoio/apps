# DeliTools-API-Schlüssel

Mit API-Schlüsseln können Ihre Server DeliTools ohne ein Delino-Anmeldetoken aufrufen. Delcore erstellt und überprüft die Schlüssel für jeden Delino-Dienst, sodass dieselbe Schlüsselfunktion auch für andere Dienste funktioniert. Sie verwalten Schlüssel mit Ihrem Delino-Anmeldetoken und verwenden einen Schlüssel als Bearer-Token, wenn Sie einen Dienst aufrufen.

## Schlüsseltypen

| Typ                       | Agiert als                                 | Wer ihn erstellen, auflisten und widerrufen kann                  | Berechtigung bei der Verwendung                                                                                             |
| ------------------------- | ------------------------------------------ | ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Persönlicher Schlüssel    | Sie, innerhalb eines Mandanten             | Nur Sie, und nur solange Sie aktuell Mitglied des Mandanten sind. | Die Freigaben des Schlüssels und Ihre aktuellen Mandantenberechtigungen. Beide müssen den Aufruf erlauben.                  |
| Mandanten-Dienstschlüssel | Der Mandant, unabhängig von einem Benutzer | Ein aktueller Owner oder Admin des Mandanten.                     | Nur die Freigaben des Schlüssels. Er funktioniert weiterhin, wenn die Person, die ihn erstellt hat, den Mandanten verlässt. |

- Andere Benutzer können Ihre persönlichen Schlüssel niemals sehen oder widerrufen, auch nicht Owner und Admins. Die Rollen Billing und Member können keine Mandanten-Dienstschlüssel verwalten.
- Eine Maschine-zu-Maschine-Identität kann keine Schlüssel verwalten. Schlüssel funktionieren niemals als Zugangsdaten für Delcore selbst, sondern nur für die Dienste, für die sie Freigaben erteilen.
- Beide Typen sind an einen Mandanten gebunden. Ein Schlüssel kann nicht für einen anderen Mandanten verwendet werden.

## Einen Schlüssel erstellen

Rufen Sie `delcore.v1.DelcoreService/CreateApiKey` unter `https://delcore.api.delino.io` mit Ihrem Delino-Anmeldetoken auf.

| Feld         | Regeln                                                                                                                                                                             |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | Eine UUID v7, die Sie generieren. Wenn Sie die Anfrage mit demselben Schlüssel und denselben Eingaben wiederholen, wird das Ergebnis erneut ausgegeben.                            |
| `tenantId`   | Der Mandant, zu dem der Schlüssel gehört.                                                                                                                                          |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` oder `API_KEY_OWNER_KIND_TENANT`.                                                                                                                    |
| `name`       | 1 bis 100 Bytes, nicht leer und ohne Steuerzeichen.                                                                                                                                |
| `grants`     | 1 bis 16 Freigaben. Jede hat eine `serviceId` und 1 bis 32 `capabilities` mit jeweils bis zu 100 Bytes. Ein Dienst kommt nur einmal vor, und Capabilities werden nicht wiederholt. |
| `expiresAt`  | Optionaler Zeitpunkt in der Zukunft im RFC-3339-Format. Ohne diesen Wert ist der Schlüssel gültig, bis Sie ihn widerrufen.                                                         |

Die `serviceId` und die Capabilities eines Dienstes finden Sie im Delcore-Katalog (`GetCatalog`). Erteilen Sie für DeliTools die Freigaben für die Capabilities `translate` und `extract`, die Sie benötigen. Delcore akzeptiert nur Capabilities, die der Dienst deklariert hat. Eine falsch geschriebene Capability wird daher beim Erstellen des Schlüssels abgelehnt.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}]}'
```

In diesem Beispiel wird kein Ablaufzeitpunkt festgelegt. Der Schlüssel bleibt daher gültig, bis Sie ihn widerrufen. Um einen hinzuzufügen, setzen Sie `expiresAt` auf einen zukünftigen Zeitpunkt im RFC-3339-Format.

Die Antwort enthält die Schlüsseldetails und das `secret`, das mit `dlk_` beginnt.

- **Das Secret wird nur einmal angezeigt.** Kopieren Sie es jetzt in Ihren Secret-Manager. Delcore speichert nur einen Hash und zeigt das Secret niemals in Listen oder in einer wiederholten Antwort an.
- **Verlorenes Secret.** Wenn Sie das Secret verlieren oder wenn die Antwort auf eine wiederholte Anfrage ein leeres `secret` enthält, erstellen Sie einen neuen Schlüssel und widerrufen Sie den alten.
- **Behandeln Sie es wie ein Passwort.** Bewahren Sie es in serverseitigem Code auf und halten Sie es aus Protokollen, Repositorys und Frontend-Bundles heraus.
- Limits: Sie können 100 aktive persönliche Schlüssel pro Benutzer und Mandant sowie 100 aktive Mandantenschlüssel pro Mandant haben. Widerrufene und abgelaufene Schlüssel zählen nicht als aktiv, bleiben aber 30 Tage lang erhalten. Ein Geltungsbereich enthält höchstens 500 Schlüssel in beliebigem Status.

Freigaben und Ablaufzeitpunkt werden beim Erstellen eines Schlüssels festgelegt. Es gibt keinen Aufruf zum Bearbeiten, Verlängern oder Rotieren. Um einen Schlüssel zu ändern, erstellen Sie einen Ersatz mit den neuen Freigaben oder dem neuen Ablaufzeitpunkt, stellen Ihre Anwendung darauf um und widerrufen anschließend den alten Schlüssel.

## Schlüssel auflisten und widerrufen

- `ListApiKeys` nimmt `tenantId` und `ownerKind` entgegen. Es gibt Schlüsseldetails zurück, einschließlich widerrufener und abgelaufener Schlüssel, aber niemals Secrets. Listen persönlicher Schlüssel zeigen nur Ihre eigenen Schlüssel. Für Mandantenlisten ist die Rolle Owner oder Admin erforderlich. Verwenden Sie `page.afterId` mit dem zurückgegebenen `nextAfterId`, um weitere Einträge abzurufen.
- `RevokeApiKey` nimmt einen `requestKey`, eine `tenantId` und eine `keyId` entgegen. Der Widerruf wirkt sich sofort auf neue Anfragen aus. Ein erneuter Widerruf ist unbedenklich.

## Einen Schlüssel verwenden

Senden Sie das Secret als `Authorization: Bearer dlk_...` an den Dienst, für den der Schlüssel Freigaben erteilt. Bei DeliTools funktioniert dies für `Translate` und `Extract`, wie in der [API-Referenz](/de/services/delitools/api-reference) beschrieben.

- `tenantId` ist bei Verwendung eines Schlüssels optional. Wenn Sie sie mitsenden, muss sie dem Mandanten des Schlüssels entsprechen.
- Ein Schlüssel kann nur die Capabilities verwenden, für die Sie für diesen Dienst Freigaben erteilt haben. Ein Schlüssel für einen Dienst funktioniert niemals für einen anderen.
- Delcore überprüft den Schlüssel bei jeder Anfrage und erneut vor jedem KI-Wiederholungsversuch. Ein Widerruf, das Ablaufen sowie der Verlust einer Berechtigung oder des Abrechnungszugangs verhindern neue Anfragen und weitere Wiederholungsversuche. Ein bereits laufender Anbieteraufruf kann abgeschlossen werden.
- Ein persönlicher Schlüssel funktioniert nicht mehr, solange Sie die Capability im Mandanten nicht mehr verwenden dürfen, zum Beispiel nachdem Sie aus dem Mandanten entfernt wurden oder eine Gruppenberechtigung verloren haben. Wenn Sie entfernt werden, wird der Schlüssel nicht gelöscht.
- Ein Mandantenschlüssel hängt nicht von der Mitgliedschaft der Person ab, die ihn erstellt hat.
- Aufrufe mit einem Schlüssel werden dem Mandanten des Schlüssels zu denselben Preisen wie Aufrufe mit einem Anmeldetoken berechnet. Siehe [Abrechnung](/de/services/delitools/billing).
- Wenn Sie einen Mandanten löschen, löscht Delcore dessen Schlüssel, sobald die Löschung endgültig wird. Persönliche Schlüssel werden gelöscht, sobald der Austritt des Benutzers akzeptiert wurde. Ein Schlüssel eines gelöschten Mandanten ist ungültig.

## Häufige Fehler

| Situation                                                                                                         | Ergebnis                                                                   |
| ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Das Secret ist falsch geschrieben, unbekannt, widerrufen oder abgelaufen                                          | `unauthenticated`. Delcore gibt nicht an, welcher dieser Fälle zutrifft.   |
| Dem Schlüssel fehlt der Dienst oder die Capability                                                                | `permission_denied`                                                        |
| `tenantId` stimmt nicht mit dem Mandanten des Schlüssels überein                                                  | `permission_denied`                                                        |
| Ihre Rolle kann diesen Schlüsseltyp nicht verwalten                                                               | `permission_denied` von Delcore beim Erstellen, Auflisten oder Widerrufen. |
| Sie widerrufen den persönlichen Schlüssel eines anderen Benutzers oder einen unbekannten Schlüssel                | `not_found` von Delcore.                                                   |
| Sie erstellen einen Schlüssel in einem Mandanten, der gerade gelöscht wird, oder überschreiten das Schlüssellimit | `failed_precondition` von Delcore.                                         |
| Ungültige Freigabe, ungültiger Name, Dienst, Capability oder Ablaufzeitpunkt                                      | `invalid_argument` von Delcore beim Erstellen des Schlüssels.              |
| Die Abrechnung des Mandanten ist gesperrt                                                                         | DeliTools gibt `ERROR_REASON_BILLING_BLOCKED` zurück.                      |
