# Delcore

Delcore stellt gemeinsame Mandanten, Zugriffskontrolle und Abrechnung für Delino-Dienste bereit. Der offizielle API-Server ist `https://delcore.api.delino.io`. Diese Seite beschreibt nutzungsabhängige Preise, den Ablauf von Datensätzen und die Prüfung des Mandantenstatus für Dienstentwickler.

## Mandantenstatus prüfen

Ein registrierter Dienst kann Delcore fragen, ob ein Mandant aktiv, gesperrt oder gelöscht ist. Verwenden Sie diese Prüfung, wenn Ihr Dienst Daten für einen Mandanten speichert, aber nicht für jede Anfrage einen Delino-Benutzer hat, beispielsweise wenn der Aufruf mit einem Projekt-API-Schlüssel erfolgt.

Rufen Sie `delcore.v1.DelcoreService/GetTenantStates` mit 1 bis 100 Mandanten-IDs auf. Jede ID muss eine UUID der Version 7 in Kleinbuchstaben sein. Authentifizieren Sie sich mit dem Zugriffstoken der Logto-Machine-to-Machine-Anwendung Ihres Dienstes. Der Dienst muss bei Delcore registriert sein und über den Scope `authorize` verfügen. Benutzertoken, nicht registrierte Dienste und Dienste ohne diesen Scope können den Mandantenstatus nicht lesen.

Die Antwort enthält jede angeforderte Mandanten-ID in der Reihenfolge, in der Sie sie gesendet haben, mit einem Status:

| Status                  | Bedeutung                                                                  |
| ----------------------- | -------------------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | Der Mandant existiert und ist weder gesperrt noch zur Löschung vorgesehen. |
| `TENANT_STATE_BLOCKED`  | Ein Plattformbetreiber hat den Mandanten gesperrt.                         |
| `TENANT_STATE_DELETING` | Die Löschung steht aus.                                                    |
| `TENANT_STATE_DELETED`  | Die Löschung ist endgültig.                                                |
| `TENANT_STATE_UNKNOWN`  | Delcore hat keinen Mandanten mit dieser ID.                                |

Wenn mehr als eine Bedingung zutrifft, hat `DELETED` Vorrang vor `DELETING`, `DELETING` vor `BLOCKED` und `BLOCKED` vor `ACTIVE`.

Der Status hängt nicht von der Abrechnung ab. Ein abgelaufenes Abonnement, eine fehlgeschlagene Verlängerung, ein Streitfall oder ein niedriger Kontostand führen nicht dazu, dass ein Mandant den Status `BLOCKED` erhält. Um zu entscheiden, ob ein Benutzer kostenpflichtige Vorgänge ausführen darf, verwenden Sie stattdessen `Authorize`. Die Prüfung liest nur Daten. Sie verändert keinen Mandanten, autorisiert keine kostenpflichtige Nutzung und kontaktiert keinen Zahlungsanbieter. Außerdem gibt sie keine Mandantennamen, Benutzerdetails oder Abrechnungsinformationen zurück.

## Ergebnisse und Fehler behandeln

- Erlauben Sie den Zugriff nur bei `TENANT_STATE_ACTIVE` oder wenn Ihre eigenen dokumentierten Regeln für Eigentümer oder Administratoren den Zugriff auf einen gesperrten oder zur Löschung vorgesehenen Mandanten erlauben.
- Behandeln Sie `TENANT_STATE_UNKNOWN`, Anfragefehler, Zeitüberschreitungen und ein nicht verfügbares Delcore als verweigerten Zugriff. Löschen Sie in diesen Fällen keine Daten.
- Löschen Sie bei einer durch die Mandantenlöschung ausgelösten Bereinigung Ihre aufbewahrten Mandantendaten nur nach einem ausdrücklichen `TENANT_STATE_DELETED` automatisch. `BLOCKED` und `DELETING` erlauben diese Bereinigung niemals. Feste Ablauffristen für Datensätze gelten unabhängig vom Mandantenstatus.
- Ungültige IDs, eine leere Anfrage oder mehr als 100 IDs führen zu einem Fehler wegen ungültiger Argumente. Fehlende, abgelaufene oder ungültige Anmeldedaten führen zu einem Fehler wegen fehlender Authentifizierung. Ein Aufrufer, der den Mandantenstatus nicht lesen darf, erhält einen Fehler wegen verweigerter Berechtigung. Andere Fehler liefern eine Fehlermeldung mit einer Korrelations-ID zurück. Geben Sie diese ID, die Sie in den Fehlerdetails und im Header `X-Delcore-Correlation-Id` finden, an, wenn Sie Support anfordern.
- Wiederholen Sie eine fehlgeschlagene Anfrage später. Eine fehlgeschlagene Anfrage bedeutet niemals, dass ein Mandant aktiv oder gelöscht ist.

## Nutzungspreise und Ablauf von Datensätzen

Die Preise pro Nutzungseinheit gelten zum Zeitpunkt des erstmaligen Ausführungsstarts für jeden Kunden, einschließlich bestehender Abonnenten. Melden Sie diesen Start in `IngestEvent` als `occurred_at` und behalten Sie ihn bei Ausführungswiederholungen und beim Nutzungsabgleich bei. Eine Preisänderung gilt für spätere Ausführungen; verspätete Übermittlungen und Korrekturen behalten die ursprüngliche Preiszuordnung bei. Für Preise wiederkehrender Zahlungen, ausgewählte Funktionen und gewährte Inklusivnutzung gelten weiterhin die jeweiligen Abonnementbedingungen.

Die Deduplizierung von Anfragen gilt für 30 Tage ab der erstmaligen Erstellung des Datensatzes. Ein erneuter Versuch verlängert diesen Zeitraum nicht. Nach Ablauf kann derselbe Anfrageschlüssel einen neuen Vorgang starten, vorbehaltlich der aktuellen Berechtigungen und unabhängiger Prüfungen der Zahlungs- oder Ereignisidentität. Akzeptierte Nutzungsereignisse behalten ihre dauerhaften Identitäten, um doppelte Auswirkungen auf die Buchhaltung zu verhindern.

Nutzende Dienste müssen Datensätze zur Nutzungsübermittlung und zum Nutzungsabgleich 30 Tage nach ihrer erstmaligen Erstellung löschen, selbst wenn die Ausführung oder Nutzung weiterhin unbekannt ist oder eine Meldung Delcore noch nicht erreicht hat. Wiederholen Sie die Übermittlung und die Abfrage unbekannter Nutzung bis zu diesem Ablauf und verwerfen Sie dann die ausstehende Belastung. Schätzen Sie die Nutzung nicht zu Abrechnungszwecken. Dadurch kann abrechenbare Nutzung dauerhaft verloren gehen, und ein abgelaufener Deduplizierungsschlüssel für die Ausführung kann eine neue Ausführung und Belastung ermöglichen. Plattform-Auditdatensätze und Betriebsprotokolle laufen ebenfalls 30 Tage nach ihrer Erstellung ab. Diese Begrenzungen löschen keine bereits von Delcore akzeptierten Buchhaltungseinträge und ändern nicht die Gültigkeit von API-Schlüsseln.
