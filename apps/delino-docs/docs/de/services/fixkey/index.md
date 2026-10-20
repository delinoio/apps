# FixKey

FixKey korrigiert Wörter, die mit dem falschen Tastaturlayout eingegeben wurden. Es unterstützt Koreanisch Dubeolsik und US-Englisch QWERTY in beide Richtungen, ohne dass Sie die Eingabesprache Ihres Systems wechseln müssen. FixKey wird lokal auf Ihrem Desktop ausgeführt und erfordert kein Konto.

## FixKey installieren

Öffnen Sie [FixKey-Releases](https://github.com/delinoio/apps/releases?q=fixkey%40) und wählen Sie das Paket für Ihr Betriebssystem und Ihren Prozessor aus:

- macOS ARM64: DMG für Apple-Chips. Verschieben Sie FixKey in den Ordner „Programme“ und öffnen Sie es.
- Windows x64 oder ARM64: EXE-Installationsprogramm für Ihren Prozessor. Windows-Pakete verfügen über keine Authenticode-Herausgebersignatur, daher zeigt Windows möglicherweise eine Warnung wegen eines unbekannten Herausgebers oder eine SmartScreen-Warnung an.
- Linux X11 x64 oder ARM64: AppImage für Ihren Prozessor. Wayland wird nicht unterstützt.

Die Mindestanforderungen für die Laufzeitumgebung sind macOS 11 auf Apple-Chips, Windows 10 mit WebView2 oder Linux mit X11, GTK 3, WebKitGTK 4.1 und einem AT-SPI-Barrierefreiheitsbus. Dies sind Plattformvoraussetzungen. Sie gewährleisten keine Kompatibilität mit jeder Anwendung oder jedem Textfeld.

## Berechtigungen erteilen

FixKey benötigt die Bedienungshilfen- und Eingabeberechtigungen Ihres Betriebssystems, um das aktuelle Wort in einem bearbeitbaren Feld zu lesen und zu ersetzen. Folgen Sie dem Berechtigungsdialog, gewähren Sie den angeforderten Zugriff in den Systemeinstellungen und wählen Sie anschließend **Erneut prüfen**. Die Korrektur bleibt inaktiv, bis die erforderlichen Berechtigungen und mitgelieferten Wörterbücher bereit sind. Passwortfelder und geschützte Felder sind ausgeschlossen.

Überprüfen Sie unter macOS in den Systemeinstellungen **Bedienungshilfen** und **Eingabeüberwachung**. Über die Bedienungshilfen kann wirksamer Eingabezugriff gewährt werden, ohne dass ein separater Eintrag unter „Eingabeüberwachung“ vorhanden ist. Um diese Liste zu prüfen, öffnen Sie in FixKey **Allgemein** und wählen Sie **Eingabeüberwachung einrichten**. Wenn FixKey nicht aufgeführt ist, verwenden Sie **+** in der Liste der Systemberechtigungen, um die aktuelle FixKey.app aus dem Ordner „Programme“ hinzuzufügen. Kehren Sie dann zu FixKey zurück und prüfen Sie erneut. Ein separater Listeneintrag ist nicht erforderlich, wenn macOS bereits wirksamen Eingabezugriff gewährt.

## Ein Wort korrigieren

Beide Layouts sind standardmäßig aktiviert. Geben Sie Text in ein bearbeitbares Feld ein und schließen Sie ein Wort mit einem Leerzeichen oder Satzzeichen ab. FixKey gleicht Kandidaten für die aktivierten Layouts mit seinen mitgelieferten Wörterbüchern ab. Das Wort wird nur ersetzt, wenn genau eine aktivierte Sprache ihren Kandidaten erkennt und sich dieser Kandidat vom Original unterscheidet. Wenn keine oder mehr als eine Sprache einen Kandidaten erkennt, bleibt das Wort unverändert.

So können beispielsweise die Tastenfolgen `rPthrgo` und `zlqhemrk` zu `계속해` und `키보드가` werden. Diese Beispiele veranschaulichen die Layoutkorrektur; die Ergebnisse hängen vom aktuellen Feld, Eingabekontext und der Wörterbucherkennung ab.

FixKey behält das Trennzeichen nach einem korrigierten Wort bei. Die Eingabe- und Tabulatortaste lösen keine Korrektur aus. Satzzeichen, die in URLs oder E-Mail-Adressen verwendet werden, können die Auswertung verzögern, bis FixKey das gesamte Token klassifizieren kann. Apostrophe und Bindestriche zwischen Buchstaben bleiben Teil des Wortes.

Die automatische Korrektur überspringt eingefügten oder programmgesteuert eingegebenen Text, Zahlen, Emojis, URLs, E-Mail-Adressen und Tokens mit gemischten Schriftsystemen. Sie wartet, bis die koreanische Eingabezusammensetzung abgeschlossen ist. Unbekannte Felder, nicht verfügbare Berechtigungen und Felder, die nicht sicher geändert werden können, bleiben unverändert.

### Eine Korrektur rückgängig machen

Drücken Sie unmittelbar nach einer automatischen Korrektur die Rücktaste, um das ursprüngliche Wort und sein Trennzeichen wiederherzustellen. Weitere Betätigungen der Rücktaste löschen wie gewohnt. Wenn Sie die Einfügemarke verschieben, die Auswahl ändern, das Feld wechseln oder ein neues Wort beginnen, endet dieser Kontext zum sofortigen Rückgängigmachen.

### Einen Kandidaten manuell auswählen

1. Öffnen Sie **Tastenkürzel** und weisen Sie das Tastenkürzel für die manuelle Kandidatenauswahl zu. Standardmäßig ist keines zugewiesen. Wenn die Registrierung fehlschlägt, prüfen Sie den angezeigten Grund oder wählen Sie ein anderes Tastenkürzel.
2. Geben Sie ein Wort in ein bearbeitbares Feld ein und verwenden Sie das Tastenkürzel.
3. Wählen Sie mit den Pfeiltasten einen Kandidaten aus. Drücken Sie die Eingabetaste, um ihn anzuwenden, oder Esc, um die Auswahl zu schließen.

Die Auswahl zeigt die Ergebnisse der Wörterbucherkennung an. Sie können ausdrücklich einen nicht erkannten Kandidaten auswählen. Wenn Sie die Einfügemarke im Quellfeld verschieben oder die Anwendung wechseln, wird die Auswahl geschlossen. Die Auswahl ist nicht verfügbar, wenn FixKey kein sicheres Ersetzungsziel bestimmen kann.

## Korrektur konfigurieren

Unter **Allgemein** können Sie die Korrektur pausieren oder fortsetzen, aktivierte Sprachen auswählen, den Start bei der Anmeldung aktivieren sowie die Sprache und das Design der Benutzeroberfläche festlegen. Der Start bei der Anmeldung ist standardmäßig deaktiviert. Einstellungen werden sofort angewendet und lokal gespeichert.

Damit eine Anwendung unverändert bleibt, fügen Sie sie unter **Ausgeschlossene Apps** hinzu. Ausschlüsse identifizieren die Anwendung und nicht nur ihren angezeigten Namen. Eine nicht aufgelöste Anwendung bleibt aufgeführt, damit Sie sie überprüfen können.

## Datenschutz und Offline-Nutzung

FixKey verarbeitet Eingaben auf Ihrem Gerät. Eingabetext, Kandidaten, Dokumentinhalte, Fenstertitel oder Korrekturverlauf werden niemals gespeichert oder übertragen. Für Ersetzungen wird die Zwischenablage nicht verwendet. Es gibt weder Telemetrie noch eine entfernte Absturzberichterstattung oder einen automatischen Upload von Diagnosedaten.

Die Wörterbücher sind im Lieferumfang enthalten, sodass die Korrektur ab dem ersten Start offline funktioniert, sobald die Berechtigungen eingerichtet sind. Wörterbücher ändern sich mit Anwendungsupdates; es gibt weder einen separaten Wörterbuchdownload noch einen Editor für Benutzerwörterbücher. Die Netzwerknutzung ist auf die Suche nach Updates und deren Downloads beschränkt.

## FixKey aktualisieren

FixKey sucht beim Start, während der Ausführung alle 24 Stunden und auf Ihre Anforderung unter **Updates** nach stabilen Updates. Download und Installation erfordern Ihre Zustimmung. Die Signaturen der Update-Pakete werden überprüft; ungültige Signaturen verhindern die Installation. Die Korrektur wird vor der Installation beendet, und die Einstellungen bleiben erhalten.

Versuchen Sie es nach einer fehlgeschlagenen Prüfung manuell erneut oder warten Sie auf die nächste geplante Prüfung. Wenn die Installation fehlschlägt, starten Sie FixKey neu oder installieren Sie das entsprechende Paket aus den FixKey-Releases erneut. Sie können eine frühere Version manuell neu installieren; FixKey führt niemals automatisch ein Downgrade durch.

## Fehlerbehebung und Support

Wenn die Korrektur inaktiv ist, öffnen Sie **Diagnose und Hilfe** und lesen Sie den Status und den Ursachencode. Prüfen Sie die Berechtigungen erneut und testen Sie ein bearbeitbares Feld in einer anderen Anwendung. Der Status „Warten auf ein bearbeitbares Feld“ bedeutet, dass das aktuelle Feld nicht sicher identifiziert werden konnte. Prüfen Sie außerdem, ob die Korrektur pausiert oder die Anwendung ausgeschlossen ist.

Verwenden Sie **Protokollordner öffnen**, um lokale Diagnoseprotokolle einzusehen, oder **Protokolle löschen**, um sie zu entfernen. Protokolle enthalten Diagnoseergebnisse und Ursachencodes, jedoch keine Eingabeinhalte. FixKey bewahrt Protokolle höchstens sieben Tage lang und bis zu einer Gesamtgröße von 10 MiB auf.

Wenn Sie Hilfe benötigen, verwenden Sie den Link zu GitHub Issues unter **Diagnose und Hilfe** oder öffnen Sie [GitHub Issues](https://github.com/delinoio/apps/issues). Geben Sie die FixKey-Version, das Betriebssystem, die betroffene Anwendung und den Ursachencode an. Fügen Sie keine privaten Texte oder Zugangsdaten bei.
