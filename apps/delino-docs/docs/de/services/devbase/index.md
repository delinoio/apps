# DevBase

DevBase ist ein kostenloses Verzeichnis für die Endnutzer Ihrer Anwendungen. Sie registrieren Ihre eigenen Nutzer, suchen sie nach und ändern sie über eine Connect-API. Ihre Nutzer benötigen keine Delino-Konten, und Ihre Anwendung behält ihre eigene Anmeldung. DevBase weist jedem Nutzer eine stabile ID zu, die Sie an andere Dienste übergeben können.

## Konzepte

- **Mandant:** Ein persönlicher Bereich oder eine Organisation in Delino. Delcore verwaltet Mandanten und deren Mitglieder.
- **Projekt:** Ein benanntes Verzeichnis von Endnutzern innerhalb eines Mandanten. Projekte teilen keine Nutzer, auch nicht innerhalb desselben Mandanten.
- **Endnutzer:** Eine Person oder ein Konto in Ihrer Anwendung. Ein Endnutzer verfügt über eine von Ihnen gewählte externe ID, eine von DevBase erstellte interne ID, einen optionalen Namen und eine optionale E-Mail-Adresse sowie ein JSON-Metadatenobjekt.
- **Projekt-API-Schlüssel:** Ein geheimer Schlüssel, mit dem Ihre Anwendung die Endnutzer eines Projekts lesen oder ändern kann.

## Anmeldung und Berechtigungen

Senden Sie mit jeder Anfrage einen `Authorization: Bearer <token>`-Header.

- **Entwickler** verwenden ihr Delino-Anmeldetoken. Nur ein aktueller Eigentümer oder Administrator des Mandanten kann Projekte und API-Schlüssel verwalten. Mit dem Anmeldetoken können sie außerdem Endnutzer lesen und ändern.
- **Anwendungen** verwenden einen Projekt-API-Schlüssel. Ein Schlüssel beginnt mit `dbk_`. Ein `READ_ONLY`-Schlüssel kann Nutzer nachschlagen und auflisten. Ein `CRUD`-Schlüssel kann Nutzer zusätzlich erstellen, aktualisieren und löschen. Ein Schlüssel kann niemals Projekte oder Schlüssel verwalten und funktioniert nur für sein eigenes Projekt.

Endnutzer melden sich niemals bei DevBase an. Ihre Datensätze gewähren ihnen keinen Zugriff auf Delino.

## Schnellstart

1. **Erstellen Sie ein Projekt.** Rufen Sie `CreateProject` mit Ihrer Mandanten-ID und einem Namen auf. Bei Nichtbeachtung der Groß- und Kleinschreibung sind Namen innerhalb eines Mandanten eindeutig.
2. **Stellen Sie einen Schlüssel aus.** Rufen Sie `IssueApiKey` mit der Projekt-ID und einer Berechtigung auf. Die Antwort enthält das Geheimnis einmalig. Speichern Sie es in Ihrer Geheimnisverwaltung.
3. **Registrieren Sie Nutzer.** Rufen Sie `CreateEndUser` mit dem Schlüssel auf. Speichern Sie die zurückgegebene `id` zusammen mit Ihrem eigenen Nutzerdatensatz.
4. **Schlagen Sie Nutzer nach.** Verwenden Sie `GetEndUser` mit der internen ID oder `LookupEndUser` mit der externen ID.
5. **Ändern oder löschen Sie Nutzer.** Verwenden Sie `UpdateEndUser` und `DeleteEndUser` mit der aktuellen `version`.

Alle Prozeduren befinden sich unter `devbase.v1.DevBaseService`. Rufen Sie DevBase aus serverseitigem Code oder von der Befehlszeile aus auf, mit einem beliebigen Connect-Client oder einfachem JSON über HTTP. Rufen Sie es nicht aus einem Browser auf und fügen Sie keinen Projektschlüssel in Frontend-Code ein: DevBase lehnt jeden Browser-Ursprung außer dem seiner eigenen Konsole ab. Setzen Sie `DEVBASE_URL` auf die Basis-URL Ihres DevBase-API-Servers, beispielsweise `http://127.0.0.1:46317` für einen lokalen Server.

```sh
# Create a project with your Delino login token.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateProject" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"<tenant-id>","name":"My app"}'

# Issue a CRUD key for the project. The secret appears only in this response.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/IssueApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"<project-id>","permission":"API_KEY_PERMISSION_CRUD"}'

# Register an end user with the project key.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateEndUser" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DEVBASE_KEY" \
  --data '{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"<project-id>","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}'
```

In JSON verwenden Feldnamen lowerCamelCase, Enum-Werte ihre vollständigen Namen und 64-Bit-Zahlen wie `version` Zeichenfolgen.

## Projekte

- Ein Projektname umfasst 1 bis 256 Byte und darf am Anfang oder Ende kein Leerzeichen enthalten.
- `UpdateProject` ändert den Namen. `DeleteProject` löscht das Projekt, alle seine Nutzer und alle seine Schlüssel in einem Schritt.
- Ein Mandant kann bis zu 100 Projekte haben.

## Projekt-API-Schlüssel

- Ein Projekt kann bis zu 20 aktive Schlüssel haben. Ein Schlüssel ist aktiv, bis er widerrufen wird oder abläuft.
- Legen Sie beim Ausstellen eines Schlüssels `expire_time` fest, wenn er ablaufen muss. Ohne diesen Wert läuft der Schlüssel nicht automatisch ab.
- DevBase speichert nur einen Hash des Geheimnisses. Wenn Sie ein Geheimnis verlieren, widerrufen Sie diesen Schlüssel und stellen Sie einen neuen aus. Um einen Schlüssel zu rotieren, stellen Sie zuerst den neuen Schlüssel aus und widerrufen Sie anschließend den alten Schlüssel.
- Wenn die Antwort auf `IssueApiKey` verloren geht, wiederholen Sie die Anfrage mit demselben Idempotenzschlüssel. Die wiederholte Antwort enthält die Schlüssel-ID und die Berechtigung, aber nicht das Geheimnis.
- `RevokeApiKey` wird sofort wirksam. Ein widerrufener oder abgelaufener Schlüssel gibt `unauthenticated` zurück.

## Endnutzer

| Feld          | Regel                                                                                                                          |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `external_id` | Erforderlich. 1 bis 256 Byte. Groß-/Kleinschreibung wird beachtet. Im Projekt eindeutig. Kein Leerzeichen am Anfang oder Ende. |
| `name`        | Optional. Bis zu 256 Byte.                                                                                                     |
| `email`       | Optional. Eine einfache Adresse mit bis zu 254 Byte. Mehrere Nutzer können dieselbe E-Mail-Adresse haben.                      |
| `metadata`    | Ein JSON-Objekt mit bis zu 16 KiB. Eine vollständige Anfrage darf bis zu 64 KiB groß sein.                                     |

- Dieselbe externe ID kann in verschiedenen Projekten vorhanden sein. Ein Projekt kann bis zu 1.000.000 Nutzer haben.
- Die Erstellung erfolgt ausdrücklich. DevBase bietet weder Upsert noch eine Suche nach E-Mail-Adresse, Name oder Metadaten.
- `UpdateEndUser` verwendet `update_mask` mit `external_id`, `name`, `email` und `metadata`. Ein maskiertes Feld `name` oder `email` mit einem leeren Wert löscht den jeweiligen Wert. Ein maskiertes Feld `metadata` ersetzt das gesamte Objekt, und ein leeres Objekt löscht es. Sie können die externe ID nicht löschen. Sie können sie ändern, wobei die interne ID gleich bleibt.
- `DeleteEndUser` entfernt den Nutzer sofort und dauerhaft. Sie können die externe ID umgehend erneut verwenden. Der nächste Nutzer mit dieser ID erhält eine neue interne ID und hat keine Verbindung zum gelöschten Nutzer.
- DevBase informiert andere Dienste nicht über Löschungen. Löschen Sie nachgelagerte Daten in Ihrer eigenen Anwendung.

## Listen

`ListProjects`, `ListApiKeys` und `ListEndUsers` geben Elemente in der Reihenfolge ihrer Erstellung zurück. Die standardmäßige Seitengröße beträgt 50 und die maximale 100. Wiederholen Sie den Aufruf mit `next_page_token`, bis dieser leer ist. Behandeln Sie das Token als undurchsichtig. Listen enthalten keine Gesamtanzahl. Wenn Sie während des Lesens der Seiten Elemente erstellen oder löschen, kann eine Seite diese Elemente übersehen oder überspringen. Ein Element, das während des gesamten Lesevorgangs vorhanden ist, erscheint einmal.

## Wiederholungsversuche und Konflikte

- Jede Änderung benötigt einen `idempotency_key` mit 8 bis 128 Zeichen aus `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` und `-`. Eine UUID eignet sich gut.
- Wenn Sie eine Anfrage innerhalb von 24 Stunden mit demselben Schlüssel und demselben Inhalt wiederholen, erhalten Sie das erste Ergebnis, und DevBase ändert nichts erneut. Verwenden Sie dies, um verlorene Antworten zu behandeln. Nach 24 Stunden gilt der Schlüssel als neue Anfrage.
- Derselbe Schlüssel mit anderem Inhalt gibt `already_exists` zurück. Jeder Aufrufer hat eigene Schlüssel, sodass ein anderer Aufrufer Ihr Ergebnis weder sehen noch übernehmen kann.
- Eine wiederholte Anfrage wird erneut anhand der aktuellen Berechtigungen geprüft. Ihr Zugriff kann zwischen dem ersten Aufruf und der Wiederholung enden.
- Nachdem Sie einen Nutzer gelöscht haben, gibt eine Wiederholung der Erstellungs- oder Aktualisierungsanfrage dieses Nutzers `not_found` zurück, und DevBase erstellt den Nutzer nicht erneut. Eine Wiederholung der Löschanfrage ist erfolgreich. Nachdem Sie ein Projekt gelöscht haben, funktionieren die Schlüssel dieses Projekts nicht mehr.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject` und `DeleteEndUser` benötigen die aktuelle `version`. Wenn jemand das Element zuerst geändert hat, erhalten Sie `aborted`. Lesen Sie das Element erneut und wiederholen Sie den Vorgang mit der neuen Version.

## Grenzwerte

| Grenzwert                       | Standardwert                        |
| ------------------------------- | ----------------------------------- |
| Projekte pro Mandant            | 100                                 |
| Nutzer pro Projekt              | 1.000.000                           |
| Aktive Schlüssel pro Projekt    | 20                                  |
| Nutzeranfragen pro Projekt      | 1.000 pro Sekunde, Spitze von 2.000 |
| Verwaltungsanfragen pro Mandant | 50 pro Sekunde, Spitze von 100      |

Nutzeranfragen werden für alle Schlüssel und Anmeldetoken eines Projekts gemeinsam gezählt. Verwaltungsanfragen werden für einen Mandanten gemeinsam gezählt. Ein neuer Schlüssel erhöht die Kapazität nicht. Wenn Sie einen Grenzwert überschreiten, erhalten Sie `resource_exhausted`. Bei einer Ratenbegrenzung geben der Fehler und der `Retry-After`-Header an, wie lange Sie warten müssen. Ein Betreiber kann diese Grenzwerte ändern. Sie schützen den Dienst und sind keine Geschwindigkeitsgarantie.

## Fehler

| Code                 | Bedeutung                                                                                                                    |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `unauthenticated`    | Das Token oder der Schlüssel fehlt, ist ungültig, abgelaufen oder widerrufen.                                                |
| `permission_denied`  | Sie verfügen nicht über die erforderliche Rolle oder Schlüsselberechtigung, oder der Mandantenstatus verhindert den Vorgang. |
| `not_found`          | Das Element existiert nicht, gehört zu einem anderen Projekt oder wurde gelöscht.                                            |
| `invalid_argument`   | Ein Eingabewert ist ungültig.                                                                                                |
| `already_exists`     | Ein Name oder eine externe ID existiert bereits, oder ein Idempotenzschlüssel wurde mit anderem Inhalt verwendet.            |
| `aborted`            | Die Version ist veraltet.                                                                                                    |
| `resource_exhausted` | Eine Ratenbegrenzung oder ein Ressourcengrenzwert gilt.                                                                      |
| `unavailable`        | Eine Abhängigkeit oder die Datenbank ist ausgefallen. Eine Änderung wurde möglicherweise angewendet oder auch nicht.         |

Jede Antwort und jeder von DevBase selbst zurückgegebene Fehler verfügt über eine Trace-ID. Geben Sie sie an, wenn Sie den Support kontaktieren. Eine Anfrage mit mehr als 256 KiB wird bereits zuvor von der Transportschicht mit `resource_exhausted` und ohne Trace-ID abgelehnt; reduzieren Sie stattdessen die Größe der Anfrage. Wenn Sie bei einer Änderung `unavailable` erhalten, wiederholen Sie dieselbe Anfrage mit demselben Idempotenzschlüssel, um herauszufinden, ob sie angewendet wurde.

## Mandantenstatus

DevBase ist kostenlos. Ihr Tarif, Ihr Guthaben und Ihre Rechnungen schränken es niemals ein. Nur eine Sperrung durch den Betreiber und die Löschung des Mandanten können dies tun.

- Während ein Mandant gesperrt ist oder gelöscht wird, funktionieren Projekt-API-Schlüssel nicht. Eigentümer und Administratoren können weiterhin Projekte und Schlüssel lesen, Schlüssel widerrufen sowie Nutzer und Projekte löschen. Sie können nichts erstellen oder ändern, keine Schlüssel ausstellen und keine Nutzer lesen.
- Wenn DevBase den Mandantenstatus nicht bestätigen kann, lehnt es die Anfrage mit `unavailable` oder `permission_denied` ab und stellt keine Vermutungen an.
- Nachdem ein Mandant dauerhaft gelöscht wurde, löscht DevBase automatisch dessen Projekte, Nutzer und Schlüssel.

## Was DevBase nicht tut

DevBase meldet Ihre Nutzer nicht an, erstellt keine Delino-Konten für sie und handelt nicht in ihrem Namen. Es teilt keine Nutzer zwischen Projekten, verschiebt keine Projekte zwischen Mandanten, importiert oder exportiert keine Daten und durchsucht keine Nutzer. Sicherungen, sofern vorhanden, sind von der sofortigen Löschung ausgenommen, die DevBase auf seine aktiven Daten anwendet.
