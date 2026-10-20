# Delcore

Delcore stellt gemeinsame Mandanten, Zugriffskontrolle und Abrechnung für Delino-Dienste bereit. Der offizielle API-Server ist `https://delcore.api.delino.io`. Diese Seite beschreibt die Prüfung des Mandantenstatus für Dienstentwickler.

## Mandantenstatus prüfen

Ein registrierter Dienst kann Delcore fragen, ob ein Mandant aktiv, gesperrt oder gelöscht ist. Verwenden Sie diese Prüfung, wenn Ihr Dienst Daten für einen Mandanten speichert, aber nicht für jede Anfrage über einen Delino-Benutzer verfügt, beispielsweise wenn der Aufruf mit einem Projekt-API-Schlüssel erfolgt.

Rufen Sie `delcore.v1.DelcoreService/GetTenantStates` mit 1 bis 100 Mandanten-IDs auf. Jede ID muss eine kleingeschriebene UUID der Version 7 sein. Authentifizieren Sie sich mit dem Zugriffstoken der Logto-Machine-to-Machine-Anwendung Ihres Dienstes. Der Dienst muss bei Delcore registriert sein und über den Scope `authorize` verfügen. Benutzertoken, nicht registrierte Dienste und Dienste ohne diesen Scope können den Mandantenstatus nicht lesen.

Die Antwort enthält jede angeforderte Mandanten-ID in der Reihenfolge, in der Sie sie gesendet haben, zusammen mit einem Status:

| Status                  | Bedeutung                                                           |
| ----------------------- | ------------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | Der Mandant existiert und ist weder gesperrt noch wird er gelöscht. |
| `TENANT_STATE_BLOCKED`  | Ein Plattformbetreiber hat den Mandanten gesperrt.                  |
| `TENANT_STATE_DELETING` | Die Löschung steht aus.                                             |
| `TENANT_STATE_DELETED`  | Die Löschung ist endgültig.                                         |
| `TENANT_STATE_UNKNOWN`  | Delcore kennt keinen Mandanten mit dieser ID.                       |

Wenn mehr als eine Bedingung zutrifft, hat `DELETED` Vorrang vor `DELETING`, `DELETING` Vorrang vor `BLOCKED` und `BLOCKED` Vorrang vor `ACTIVE`.

Der Status hängt nicht von der Abrechnung ab. Ein abgelaufenes Abonnement, eine fehlgeschlagene Verlängerung, ein Streitfall oder ein niedriges Guthaben führen nicht dazu, dass ein Mandant den Status `BLOCKED` erhält. Um zu entscheiden, ob ein Benutzer kostenpflichtige Vorgänge ausführen darf, verwenden Sie stattdessen `Authorize`. Die Prüfung liest lediglich Daten. Sie verändert keinen Mandanten, autorisiert keine kostenpflichtige Nutzung und kontaktiert keinen Zahlungsanbieter. Außerdem gibt sie weder Mandantennamen noch Benutzerdetails oder Abrechnungsinformationen zurück.

## Ergebnisse und Fehler behandeln

- Erlauben Sie den Zugriff nur bei `TENANT_STATE_ACTIVE` oder wenn Ihre eigenen dokumentierten Regeln Eigentümern oder Administratoren den Zugriff auf einen gesperrten oder in Löschung befindlichen Mandanten gestatten.
- Behandeln Sie `TENANT_STATE_UNKNOWN`, Anfragefehler, Zeitüberschreitungen und ein nicht verfügbares Delcore so, als bestünde kein Zugriff. Löschen Sie in diesen Fällen keine Daten.
- Löschen Sie Ihre gespeicherten Mandantendaten nur nach einem expliziten `TENANT_STATE_DELETED` automatisch. `BLOCKED` und `DELETING` erlauben niemals eine automatische Löschung.
- Ungültige IDs, eine leere Anfrage oder mehr als 100 IDs führen zu einem Fehler wegen eines ungültigen Arguments. Fehlende, abgelaufene oder ungültige Anmeldedaten führen zu einem Fehler wegen fehlender Authentifizierung. Ein Aufrufer, der den Mandantenstatus nicht lesen darf, erhält einen Fehler wegen verweigerter Berechtigung. Andere Fehler enthalten eine Korrelations-ID. Geben Sie diese ID, die Sie in den Fehlerdetails und im Header `X-Delcore-Correlation-Id` finden, bei Supportanfragen an.
- Wiederholen Sie eine fehlgeschlagene Anfrage später. Eine fehlgeschlagene Anfrage bedeutet niemals, dass ein Mandant aktiv oder gelöscht ist.
