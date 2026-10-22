# DeliTools-API-Schlüssel

Mit API-Schlüsseln können deine Server DeliTools ohne ein Delino-Anmeldetoken aufrufen. Delcore erstellt und überprüft die Schlüssel für jeden Delino-Dienst, sodass dieselbe Schlüsselfunktion auch für andere Dienste funktioniert. Du verwaltest Schlüssel mit deinem Delino-Anmeldetoken und verwendest einen Schlüssel als Bearer-Token, wenn du einen Dienst aufrufst.

## Schlüsseltypen

| Typ                       | Handelt als                                | Wer ihn erstellen, auflisten und widerrufen kann                | Berechtigung bei der Verwendung                                                                                             |
| ------------------------- | ------------------------------------------ | --------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Persönlicher Schlüssel    | Du, innerhalb eines Mandanten              | Nur du, und nur solange du aktuell Mitglied des Mandanten bist. | Die Freigaben des Schlüssels und deine aktuellen Mandantenberechtigungen. Beide müssen den Aufruf erlauben.                 |
| Mandanten-Dienstschlüssel | Der Mandant, unabhängig von einem Benutzer | Ein aktueller Owner oder Admin des Mandanten.                   | Nur die Freigaben des Schlüssels. Er funktioniert weiterhin, wenn die Person, die ihn erstellt hat, den Mandanten verlässt. |

- Andere Benutzer können deine persönlichen Schlüssel niemals sehen oder widerrufen, auch nicht Owner und Admins. Die Rollen Billing und Member können keine Mandanten-Dienstschlüssel verwalten.
- Eine Machine-to-Machine-Identität kann keine Schlüssel verwalten. Schlüssel funktionieren niemals als Zugangsdaten für Delcore selbst, sondern nur für die Dienste, für die sie Freigaben enthalten.
- Beide Typen sind an einen Mandanten gebunden. Ein Schlüssel kann nicht für einen anderen Mandanten verwendet werden.

## Einen Schlüssel erstellen

Rufe `delcore.v1.DelcoreService/CreateApiKey` unter `https://delcore.api.delino.io` mit deinem Delino-Anmeldetoken auf.

| Feld         | Regeln                                                                                                                                                                          |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | Eine von dir generierte UUID v7. Wenn du die Anfrage mit demselben Schlüssel und derselben Eingabe wiederholst, wird das Ergebnis erneut ausgegeben.                            |
| `tenantId`   | Der Mandant, zu dem der Schlüssel gehört.                                                                                                                                       |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` oder `API_KEY_OWNER_KIND_TENANT`.                                                                                                                 |
| `name`       | 1 bis 100 Bytes, nicht leer und ohne Steuerzeichen.                                                                                                                             |
| `grants`     | 1 bis 16 Freigaben. Jede hat eine `serviceId` und 1 bis 32 `capabilities` mit jeweils bis zu 100 Bytes. Jeder Dienst kommt einmal vor, und Fähigkeiten werden nicht wiederholt. |
| `expiresAt`  | Optionaler Zeitpunkt in der Zukunft im Format RFC 3339. Ohne diesen Wert ist der Schlüssel gültig, bis du ihn widerrufst.                                                       |

Die `serviceId` und die Fähigkeiten eines Dienstes findest du im Delcore-Katalog (`GetCatalog`). Erteile für DeliTools die Freigaben für die Fähigkeiten `translate` und `extract`, die du benötigst. Delcore akzeptiert nur Fähigkeiten, die der Dienst deklariert hat. Eine falsch geschriebene Fähigkeit wird daher beim Erstellen des Schlüssels abgelehnt.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}],"expiresAt":"2027-01-01T00:00:00Z"}'
```

Die Antwort enthält die Schlüsseldetails und das `secret`, das mit `dlk_` beginnt.

- **Das Secret wird nur einmal angezeigt.** Kopiere es jetzt in deinen Secret-Manager. Delcore speichert nur einen Hash und zeigt das Secret niemals in Listen oder in einer wiederholten Antwort an.
- **Verlorenes Secret.** Wenn du das Secret verlierst oder die Antwort auf eine wiederholte Anfrage ein leeres `secret` enthält, erstelle einen neuen Schlüssel und widerrufe den alten.
- **Behandle es wie ein Passwort.** Bewahre es in serverseitigem Code auf und halte es aus Protokollen, Repositorys und Frontend-Bundles heraus.
- Jeder Mandant kann nur eine begrenzte Anzahl aktiver Schlüssel haben: 20 persönliche Schlüssel pro Benutzer und insgesamt 20 Mandantenschlüssel. Widerrufene und abgelaufene Schlüssel zählen nicht mit.

Freigaben und Ablaufzeitpunkt werden beim Erstellen eines Schlüssels festgelegt. Es gibt keinen Aufruf zum Bearbeiten, Verlängern oder Rotieren. Um einen Schlüssel zu ändern, erstelle einen Ersatz mit den neuen Freigaben oder dem neuen Ablaufzeitpunkt, stelle deine Anwendung darauf um und widerrufe anschließend den alten Schlüssel.

## Schlüssel auflisten und widerrufen

- `ListApiKeys` nimmt `tenantId` und `ownerKind` entgegen. Es gibt Schlüsseldetails zurück, einschließlich widerrufener und abgelaufener Schlüssel, aber niemals Secrets. Persönliche Auflistungen zeigen nur deine eigenen Schlüssel. Mandantenauflistungen erfordern die Rolle Owner oder Admin. Verwende `page.afterId` mit der zurückgegebenen `nextAfterId`, um weitere Einträge abzurufen.
- `RevokeApiKey` nimmt einen `requestKey`, eine `tenantId` und eine `keyId` entgegen. Der Widerruf gilt sofort für neue Anfragen. Ein erneuter Widerruf ist unbedenklich.

## Einen Schlüssel verwenden

Sende das Secret als `Authorization: Bearer dlk_...` an den Dienst, für den der Schlüssel Freigaben enthält. Für DeliTools funktioniert dies für `Translate` und `Extract`, wie in der [API-Referenz](/de/services/delitools/api-reference) beschrieben.

- `tenantId` ist bei einem Schlüssel optional. Wenn du sie mitsendest, muss sie dem Mandanten des Schlüssels entsprechen.
- Ein Schlüssel kann nur die Fähigkeiten verwenden, für die du Freigaben für diesen Dienst erteilt hast. Ein Schlüssel für einen Dienst funktioniert niemals für einen anderen.
- Delcore überprüft den Schlüssel bei jeder Anfrage und erneut vor jedem KI-Wiederholungsversuch. Widerruf, Ablauf und der Verlust von Berechtigungen oder Abrechnungszugriff stoppen neue Anfragen und weitere Wiederholungsversuche. Ein bereits laufender Anbieteraufruf kann abgeschlossen werden.
- Ein persönlicher Schlüssel funktioniert nicht mehr, solange du die Fähigkeit im Mandanten nicht mehr verwenden darfst, beispielsweise nachdem du aus dem Mandanten entfernt wurdest oder eine Gruppenberechtigung verloren hast. Wenn du entfernt wirst, wird der Schlüssel nicht gelöscht.
- Ein Mandantenschlüssel hängt nicht von der Mitgliedschaft der Person ab, die ihn erstellt hat.
- Aufrufe mit einem Schlüssel werden dem Mandanten des Schlüssels zu denselben Preisen wie Aufrufe mit einem Anmeldetoken in Rechnung gestellt. Siehe [Abrechnung](/de/services/delitools/billing).
- Wenn du einen Mandanten löschst, löscht Delcore dessen Schlüssel, sobald die Löschung endgültig wird. Persönliche Schlüssel werden gelöscht, sobald der Austritt des Benutzers akzeptiert wurde. Ein Schlüssel eines gelöschten Mandanten ist ungültig.

## Häufige Fehler

| Situation                                                                                                         | Ergebnis                                                                   |
| ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Das Secret ist falsch geschrieben, unbekannt, widerrufen oder abgelaufen                                          | `unauthenticated`. Delcore gibt nicht an, welcher dieser Fälle zutrifft.   |
| Dem Schlüssel fehlt der Dienst oder die Fähigkeit                                                                 | `permission_denied`                                                        |
| `tenantId` stimmt nicht mit dem Mandanten des Schlüssels überein                                                  | `permission_denied`                                                        |
| Deine Rolle darf diesen Schlüsseltyp nicht verwalten                                                              | `permission_denied` von Delcore beim Erstellen, Auflisten oder Widerrufen. |
| Du widerrufst den persönlichen Schlüssel eines anderen Benutzers oder einen unbekannten Schlüssel                 | `not_found` von Delcore.                                                   |
| Du erstellst einen Schlüssel in einem Mandanten, der gerade gelöscht wird, oder überschreitest das Schlüssellimit | `failed_precondition` von Delcore.                                         |
| Ungültige Freigabe, ungültiger Name, Dienst, Fähigkeit oder Ablaufzeitpunkt                                       | `invalid_argument` von Delcore beim Erstellen des Schlüssels.              |
| Die Abrechnung des Mandanten ist gesperrt                                                                         | DeliTools gibt `ERROR_REASON_BILLING_BLOCKED` zurück.                      |
