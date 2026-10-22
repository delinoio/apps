# Delcore

Delcore stellt gemeinsame Mandanten, Zugriffskontrolle und Abrechnung für Delino-Dienste bereit. Der offizielle API-Server ist `https://delcore.api.delino.io`. Diese Seite beschreibt die Überprüfung von API-Schlüsseln, nutzungsbasierte Preise, den Ablauf von Datensätzen und die Prüfung des Mandantenstatus für Dienstentwickler.

## Mandantenstatus prüfen

Ein registrierter Dienst kann Delcore fragen, ob ein Mandant aktiv, gesperrt oder gelöscht ist. Verwenden Sie diese Prüfung, wenn Ihr Dienst Daten für einen Mandanten speichert, aber nicht für jede Anfrage ein Delino-Benutzer vorhanden ist, beispielsweise wenn der Aufruf mit einem Projekt-API-Schlüssel erfolgt.

Rufen Sie `delcore.v1.DelcoreService/GetTenantStates` mit 1 bis 100 Mandanten-IDs auf. Jede ID muss eine UUID der Version 7 in Kleinbuchstaben sein. Authentifizieren Sie sich mit dem Zugriffstoken der Logto-Machine-to-Machine-Anwendung Ihres Dienstes. Der Dienst muss bei Delcore registriert sein und über den Scope `authorize` verfügen. Benutzertoken, nicht registrierte Dienste und Dienste ohne diesen Scope können den Mandantenstatus nicht lesen.

Die Antwort wiederholt jede angeforderte Mandanten-ID in der von Ihnen gesendeten Reihenfolge mit einem Status:

| Status                  | Bedeutung                                                                  |
| ----------------------- | -------------------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | Der Mandant existiert und ist weder gesperrt noch wird er gerade gelöscht. |
| `TENANT_STATE_BLOCKED`  | Ein Plattformbetreiber hat den Mandanten gesperrt.                         |
| `TENANT_STATE_DELETING` | Die Löschung steht aus.                                                    |
| `TENANT_STATE_DELETED`  | Die Löschung ist endgültig.                                                |
| `TENANT_STATE_UNKNOWN`  | Delcore hat keinen Mandanten mit dieser ID.                                |

Wenn mehr als eine Bedingung zutrifft, hat `DELETED` Vorrang vor `DELETING`, `DELETING` vor `BLOCKED` und `BLOCKED` vor `ACTIVE`.

Der Status ist unabhängig von der Abrechnung. Ein abgelaufenes Abonnement, eine fehlgeschlagene Verlängerung, ein Streitfall oder ein niedriger Kontostand versetzt einen Mandanten nicht in den Status `BLOCKED`. Um zu entscheiden, ob ein Benutzer kostenpflichtige Vorgänge ausführen darf, verwenden Sie stattdessen `Authorize`. Die Prüfung liest lediglich Daten. Sie verändert keinen Mandanten, autorisiert keine kostenpflichtige Nutzung und kontaktiert keinen Zahlungsanbieter. Zudem gibt sie keine Mandantennamen, Benutzerdetails oder Abrechnungsinformationen zurück.

## Ergebnisse und Fehler behandeln

- Erlauben Sie den Zugriff nur bei `TENANT_STATE_ACTIVE` oder wenn Ihre eigenen dokumentierten Regeln für Eigentümer oder Administratoren den Zugriff auf einen gesperrten oder zur Löschung vorgesehenen Mandanten erlauben.
- Behandeln Sie `TENANT_STATE_UNKNOWN`, Anfragefehler, Zeitüberschreitungen und die Nichtverfügbarkeit von Delcore als verweigerten Zugriff. Löschen Sie in diesen Fällen keine Daten.
- Löschen Sie bei einer Bereinigung infolge einer Mandantenlöschung Ihre aufbewahrten Mandantendaten nur nach einem expliziten `TENANT_STATE_DELETED` automatisch. `BLOCKED` und `DELETING` erlauben diese Bereinigung niemals. Der festgelegte Ablauf von Datensätzen gilt unabhängig vom Mandantenstatus.
- Ungültige IDs, eine leere Anfrage oder mehr als 100 IDs führen zu einem Fehler wegen ungültiger Argumente. Fehlende, abgelaufene oder ungültige Anmeldedaten führen zu einem Fehler wegen fehlender Authentifizierung. Ein Aufrufer, der den Mandantenstatus nicht lesen darf, erhält einen Fehler wegen verweigerter Berechtigung. Andere Fehler liefern einen Fehler mit einer Korrelations-ID zurück. Geben Sie diese ID, die Sie in den Fehlerdetails und im Header `X-Delcore-Correlation-Id` finden, bei Supportanfragen an.
- Wiederholen Sie eine fehlgeschlagene Anfrage später. Eine fehlgeschlagene Anfrage bedeutet niemals, dass ein Mandant aktiv oder gelöscht ist.

## Nutzungspreise und Ablauf von Datensätzen

Die Preise pro Nutzungseinheit gelten zum erstmaligen Start der Ausführung für jeden Kunden, einschließlich bestehender Abonnenten. Melden Sie diesen Start in `IngestEvent` als `occurred_at` und behalten Sie ihn bei Wiederholungsversuchen der Ausführung und beim Nutzungsabgleich bei. Eine Preisänderung gilt für spätere Ausführungen; bei verspäteter Übermittlung und Korrekturen bleibt die ursprüngliche Preiszuordnung erhalten. Für wiederkehrende Zahlungen, ausgewählte Funktionen und Kontingente für enthaltene Nutzung gelten weiterhin die jeweiligen Abonnementbedingungen.

Die Deduplizierung von Anfragen gilt für 30 Tage ab der erstmaligen Erstellung des Datensatzes. Ein Wiederholungsversuch verlängert diesen Zeitraum nicht. Nach Ablauf kann derselbe Anfrageschlüssel einen neuen Vorgang starten, vorbehaltlich der aktuellen Berechtigungen und unabhängiger Prüfungen der Zahlungs- oder Ereignisidentität. Akzeptierte Nutzungsereignisse behalten ihre dauerhaften Identitäten, um doppelte Auswirkungen auf die Buchhaltung zu verhindern.

Nutzende Dienste müssen Datensätze zur Nutzungsübermittlung und zum Abgleich 30 Tage nach der erstmaligen Erstellung löschen, auch wenn die Ausführung oder Nutzung weiterhin unbekannt ist oder eine Meldung Delcore noch nicht erreicht hat. Wiederholen Sie die Übermittlung und die Abfrage unbekannter Nutzung bis zu diesem Ablauf und verwerfen Sie dann die ausstehende Belastung. Schätzen Sie die Nutzung nicht für Abrechnungszwecke. Dadurch kann abrechenbare Nutzung dauerhaft verloren gehen, und ein abgelaufener Deduplizierungsschlüssel für die Ausführung kann eine neue Ausführung und Belastung ermöglichen. Plattform-Auditdatensätze und Betriebsprotokolle laufen ebenfalls 30 Tage nach der Erstellung ab. Diese Beschränkungen löschen keine bereits von Delcore akzeptierten Buchhaltungseinträge und ändern nicht die Gültigkeit von API-Schlüsseln.

## Einen API-Schlüssel überprüfen

Delcore stellt wiederverwendbare persönliche und mandantenbezogene Dienst-API-Schlüssel für jeden registrierten Dienst aus. Benutzer sowie Mandanteneigentümer und -administratoren erstellen sie mit einem Delino-Anmeldetoken; den benutzerseitigen Ablauf finden Sie unter [DeliTools-API-Schlüssel](/de/services/delitools/api-keys). Ein Schlüssel beginnt mit `dlk_`, gewährt ausgewählte Berechtigungen für ausgewählte Dienste und gehört zu einem Mandanten.

Ein registrierter Dienst überprüft einen Schlüssel, den er von seinen Aufrufern erhält, mit `delcore.v1.DelcoreService/AuthorizeApiKey`. Authentifizieren Sie sich mit dem Logto-Machine-to-Machine-Zugriffstoken Ihres Dienstes. Der Dienst muss registriert sein und über den Scope `authorize` verfügen.

- Senden Sie das vollständige `secret`, die verwendete `capability` und für kostenpflichtige Vorgänge `paid`. Sie können außerdem wie bei `Authorize` einen `feature_key` und `group_values` senden.
- Delcore wertet nur die Freigaben aus, die den aufrufenden Dienst nennen. Ihr Dienst kann daher niemals die Berechtigungen eines anderen Dienstes autorisieren.
- Ein fehlerhaftes, unbekanntes, widerrufenes oder abgelaufenes Secret oder das Secret eines gelöschten Mandanten führt zu einem Fehler wegen fehlender Authentifizierung und liefert keine Schlüsseldetails zurück. Delcore antwortet auch so, wenn Ihr eigenes Diensttoken abgelehnt wird. Prüfen Sie daher Ihre Dienstanmeldedaten, wenn jeder gültige Schlüssel fehlschlägt. Behandeln Sie dies als ungültige Anmeldedaten.
- Bei einem gültigen Secret enthält die Antwort `allowed`, `reasons` und das optionale `feature` sowie `keyId`, `tenantId`, `ownerKind` und `userId`. Ein Schlüssel ohne die erforderliche Berechtigung liefert `allowed: false` mit `BLOCK_REASON_PERMISSION` zurück.
- Ein persönlicher Schlüssel benötigt außerdem die aktuelle Berechtigung seines Benutzers für die betreffende Funktion im Mandanten. Ein Mandantenschlüssel ist unabhängig von dem Benutzer, der ihn erstellt hat. Beide erfordern einen aktiven Mandanten und dieselben Abrechnungsprüfungen wie `Authorize`, wenn `paid` gesetzt ist.
- Vergleichen Sie den Mandanten, den Ihr Aufrufer gegebenenfalls angibt, mit der zurückgegebenen `tenantId` und lehnen Sie die Anfrage bei einer Abweichung ab.
- Prüfen Sie jede Anfrage. Speichern Sie kein Ergebnis im Cache. Behandeln Sie Fehler, Zeitüberschreitungen und die Nichtverfügbarkeit von Delcore als verweigerten Zugriff für neue kostenpflichtige Vorgänge. Ein Autorisierungsergebnis reserviert nichts.
