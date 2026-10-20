# FixKey

FixKey korrigiert Wörter, die mit dem falschen Tastaturlayout eingegeben wurden. Es unterstützt Koreanisch mit Dubeolsik und US-Englisch mit QWERTY in beide Richtungen. FixKey wird lokal auf Ihrem Desktop ausgeführt und erfordert kein Konto.

## FixKey installieren

Öffnen Sie die [FixKey-Releases](https://github.com/delinoio/apps/releases?q=fixkey%40) und wählen Sie das Paket für Ihr Betriebssystem und Ihren Prozessor aus:

- macOS ARM64: DMG für Apple-Chips. Verschieben Sie FixKey in den Ordner „Programme“ und öffnen Sie es.
- Windows x64 oder ARM64: EXE-Installationsprogramm für Ihren Prozessor. Windows-Pakete verfügen nicht über eine Authenticode-Herausgebersignatur, daher zeigt Windows möglicherweise eine Warnung wegen eines unbekannten Herausgebers oder von SmartScreen an.
- Linux X11 x64 oder ARM64: AppImage für Ihren Prozessor. Wayland wird nicht unterstützt.

Die Mindestanforderungen für die Laufzeit sind macOS 11 auf Apple-Chips, Windows 10 mit WebView2 oder Linux mit X11, GTK 3, WebKitGTK 4.1 und einem AT-SPI-Bus für Bedienungshilfen. Dies sind Plattformvoraussetzungen. Sie gewährleisten keine Kompatibilität mit jeder Anwendung oder jedem Textfeld.

## Berechtigungen erteilen

FixKey benötigt die Bedienungshilfen- und Eingabeberechtigungen Ihres Betriebssystems, um das aktuelle Wort in einem bearbeitbaren Feld zu lesen und zu ersetzen. Folgen Sie dem Berechtigungsdialog, gewähren Sie den angeforderten Zugriff in den Systemeinstellungen und wählen Sie anschließend **Erneut prüfen**. Die Korrektur bleibt deaktiviert, bis die erforderlichen Berechtigungen und mitgelieferten Wörterbücher verfügbar sind. Passwortfelder und geschützte Felder sind ausgeschlossen.

Prüfen Sie unter macOS in den Systemeinstellungen **Bedienungshilfen** und **Eingabeüberwachung**. Über die Bedienungshilfen kann wirksamer Eingabezugriff gewährt werden, ohne dass ein separater Eintrag unter „Eingabeüberwachung“ vorhanden ist. Um diese Liste zu prüfen, öffnen Sie in FixKey **Allgemein** und wählen Sie **Eingabeüberwachung einrichten**. Wenn FixKey fehlt, fügen Sie über **+** in der Berechtigungsliste des Systems die aktuelle FixKey.app aus dem Ordner „Programme“ hinzu. Kehren Sie anschließend zu FixKey zurück und führen Sie die Prüfung erneut durch. Ein separater Listeneintrag ist nicht erforderlich, wenn macOS bereits wirksamen Eingabezugriff gewährt.

## Ein Wort korrigieren

Beide Layouts sind standardmäßig aktiviert. Geben Sie Text in ein bearbeitbares Feld ein und schließen Sie ein Wort mit einem Leerzeichen oder Satzzeichen ab. FixKey prüft Kandidaten für die aktivierten Layouts anhand seiner mitgelieferten Wörterbücher. Das Wort wird nur ersetzt, wenn genau eine aktivierte Sprache ihren Kandidaten erkennt und sich dieser Kandidat vom Original unterscheidet. Wenn keine oder mehr als eine Sprache einen Kandidaten erkennt, bleibt das Wort unverändert.

Beispielsweise können die Tastenfolgen `rPthrgo` und `zlqhemrk` zu `계속해` und `키보드가` werden. Diese Beispiele veranschaulichen die Layoutkorrektur; die Ergebnisse hängen vom aktuellen Feld, Eingabekontext und der Wörterbucherkennung ab.

FixKey behält das Trennzeichen nach einem korrigierten Wort bei. Die Eingabe- und Tabulatortaste lösen keine Korrektur aus. Satzzeichen, die in URLs oder E-Mail-Adressen verwendet werden, können die Auswertung verzögern, bis FixKey das gesamte Token klassifizieren kann. Apostrophe und Bindestriche zwischen Buchstaben bleiben Teil des Wortes.

Die automatische Korrektur überspringt eingefügten oder programmgesteuert eingesetzten Text, Zahlen, Emojis, URLs, E-Mail-Adressen und Tokens mit gemischten Schriftsystemen. Sie wartet, bis die koreanische Eingabezusammensetzung abgeschlossen ist. Unbekannte Felder, nicht verfügbare Berechtigungen und Felder, die nicht sicher geändert werden können, bleiben unverändert.

Nach einer erfolgreichen automatischen Korrektur unter macOS oder Windows versucht FixKey außerdem, eine aktivierte Eingabequelle für die korrigierte Sprache auszuwählen. Unter Windows kann es die Hangul- und Lateinmodi des koreanischen Microsoft-IME verwenden, auch wenn keine separate US-QWERTY-Quelle aktiviert ist. Lassen Sie Ihren koreanischen IME auf Dubeolsik eingestellt. Wenn die Auswahl der Eingabequelle nicht verfügbar ist oder fehlschlägt, bleiben der korrigierte Text und die Möglichkeit zum sofortigen Rückgängigmachen verfügbar. Geschlossene oder nicht unterstützte Windows-IMEs werden nicht geändert. Unter Linux bleibt die aktuelle Eingabequelle erhalten. Beim sofortigen Rückgängigmachen wird die Eingabequelle nicht erneut geändert.

### Eine Korrektur rückgängig machen

Drücken Sie unmittelbar nach einer automatischen Korrektur die Rücktaste, um das ursprüngliche Wort und sein Trennzeichen wiederherzustellen. Bei weiteren Betätigungen der Rücktaste wird normal gelöscht. Wenn Sie den Cursor bewegen, die Auswahl ändern, das Feld wechseln oder ein neues Wort beginnen, endet dieser Kontext für das sofortige Rückgängigmachen.

### Einen Kandidaten manuell auswählen

1. Öffnen Sie **Tastenkürzel** und weisen Sie das Tastenkürzel für die manuelle Kandidatenauswahl zu. Standardmäßig ist keines zugewiesen. Wenn die Registrierung fehlschlägt, prüfen Sie den angezeigten Grund oder wählen Sie ein anderes Tastenkürzel.
2. Geben Sie ein Wort in ein bearbeitbares Feld ein und verwenden Sie das Tastenkürzel.
3. Wählen Sie mit den Pfeiltasten einen Kandidaten aus. Drücken Sie die Eingabetaste, um ihn anzuwenden, oder Esc, um die Auswahl zu schließen.

Die Auswahl zeigt die Ergebnisse der Wörterbucherkennung an. Sie können ausdrücklich einen nicht erkannten Kandidaten auswählen. Wenn Sie den Cursor im Ausgangsfeld bewegen oder die Anwendung wechseln, wird die Auswahl geschlossen. Die Auswahl ist nicht verfügbar, wenn FixKey kein sicheres Ersetzungsziel ermitteln kann.

## Korrektur konfigurieren

Unter **Allgemein** können Sie die Korrektur pausieren oder fortsetzen, aktivierte Sprachen auswählen, den Start bei der Anmeldung aktivieren sowie die Sprache und das Design der Benutzeroberfläche festlegen. Der Start bei der Anmeldung ist standardmäßig deaktiviert. Einstellungen werden sofort angewendet und lokal gespeichert.

Damit eine Anwendung unverändert bleibt, fügen Sie sie unter **Ausgeschlossene Apps** hinzu. Ausschlüsse identifizieren die Anwendung selbst und nicht nur ihren angezeigten Namen. Eine nicht aufgelöste Anwendung bleibt aufgelistet, damit Sie sie überprüfen können.

## Datenschutz und Offline-Nutzung

FixKey verarbeitet Eingaben auf Ihrem Gerät. Es speichert oder überträgt niemals Eingabetext, Kandidaten, Dokumentinhalte, Fenstertitel oder den Korrekturverlauf. Für Ersetzungen verwendet es nicht die Zwischenablage. Es gibt keine Telemetrie, keine externe Absturzberichterstattung und keinen automatischen Upload von Diagnosedaten.

Die Wörterbücher sind im Lieferumfang enthalten, sodass die Korrektur ab dem ersten Start offline funktioniert, sobald die Berechtigungen verfügbar sind. Wörterbücher ändern sich mit Anwendungsupdates; es gibt weder einen separaten Wörterbuchdownload noch einen Editor für Benutzerwörterbücher. Die Netzwerknutzung ist auf die Suche nach Updates und deren Download beschränkt.

## FixKey aktualisieren

FixKey sucht beim Start, während der Ausführung alle 24 Stunden und auf Ihre Anforderung unter **Updates** nach stabilen Updates. Download und Installation erfordern Ihre Zustimmung. Die Signaturen der Update-Payloads werden überprüft, und ungültige Signaturen verhindern die Installation. Die Korrektur wird vor der Installation angehalten, und die Einstellungen bleiben erhalten.

Versuchen Sie es nach einer fehlgeschlagenen Prüfung manuell erneut oder warten Sie auf die nächste geplante Prüfung. Wenn die Installation fehlschlägt, starten Sie FixKey neu oder installieren Sie das passende Paket aus den FixKey-Releases erneut. Sie können eine frühere Version manuell neu installieren; FixKey führt niemals automatisch ein Downgrade durch.

## Fehlerbehebung und Support

Wenn die Korrektur inaktiv ist, öffnen Sie **Diagnose und Hilfe** und lesen Sie den Status und den Ursachencode. Prüfen Sie die Berechtigungen erneut und testen Sie ein bearbeitbares Feld in einer anderen Anwendung. Der Status „Warten auf ein bearbeitbares Feld“ bedeutet, dass das aktuelle Feld nicht sicher identifiziert werden konnte. Prüfen Sie außerdem, ob die Korrektur pausiert oder die Anwendung ausgeschlossen ist.

Verwenden Sie **Protokollordner öffnen**, um lokale Diagnoseprotokolle einzusehen, oder **Protokolle löschen**, um sie zu entfernen. Protokolle enthalten Diagnoseergebnisse und Ursachencodes anstelle von Eingabeinhalten. FixKey bewahrt Protokolle höchstens sieben Tage lang und bis zu einer Gesamtgröße von maximal 10 MiB auf.

Wenn Sie Hilfe benötigen, verwenden Sie den Link zu GitHub Issues unter **Diagnose und Hilfe** oder öffnen Sie [GitHub Issues](https://github.com/delinoio/apps/issues). Geben Sie die FixKey-Version, das Betriebssystem, die betroffene Anwendung und den Ursachencode an. Fügen Sie keine privaten Texte oder Zugangsdaten hinzu.
