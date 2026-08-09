---
title: Datenschutzerklärung
description: Informationen zur Datenverarbeitung auf openbikecomputer.com.
---

# Datenschutzerklärung

## 1. Verantwortlicher

Verantwortlicher im Sinne des Art. 4 Nr. 7 DSGVO ist:

<address>
  Timo Hüser<br>
  Scharnhorststraße 32<br>
  79331 Teningen<br>
  Deutschland<br>
  E-Mail: <a href="mailto:openbikecomputer@proton.me">openbikecomputer@proton.me</a>
</address>

## 2. Überblick

Die Startseite, Dokumentation und der Blog sind statische Seiten. Sie setzen keine
Cookies ein und verwenden keine Analysedienste, Zählpixel, Werbung, extern geladenen
Schriftarten, Social-Media-Plugins oder Fehler-Tracker. Beim Abruf entstehen lediglich
die für die Auslieferung und Sicherheit erforderlichen Verbindungsdaten beim Hoster.

Der Kartenbaukasten unter `/builder/` lädt zusätzlich Kartenkacheln und Kartendaten,
prüft nach dem Anschluss eines Geräts auf neue Firmware und verarbeitet ausgewählte
Dateien sowie Gerätedaten lokal im Browser. Einzelheiten stehen in den Abschnitten 6
bis 9.

## 3. Hosting über GitHub Pages

Die Website wird über **GitHub Pages** der GitHub, Inc., 88 Colin P. Kelly Jr. Street,
San Francisco, CA 94107, USA, ausgeliefert.

Beim Seitenabruf verarbeitet GitHub die technisch übertragenen Zugriffsdaten. Dazu
gehören insbesondere IP-Adresse, Zeitpunkt und Ziel des Abrufs, HTTP-Status,
übertragene Datenmenge und Browserkennung (User-Agent). GitHub gibt an, die IP-Adressen
von Besucherinnen und Besuchern von GitHub Pages zu Sicherheitszwecken zu protokollieren.

Zweck ist die technische Bereitstellung sowie die Sicherheit und Stabilität der
Website. Rechtsgrundlage ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse liegt
in einem funktionsfähigen und gegen Angriffe geschützten Webangebot. Ohne Übermittlung
der IP-Adresse kann die Website nicht abgerufen werden.

Der Verantwortliche hat keinen Zugriff auf die GitHub-Zugriffsprotokolle, erhält keine
Besucherauswertungen und führt diese Daten nicht mit anderen Daten zusammen. GitHub
veröffentlicht für die Zugriffsprotokolle von GitHub Pages keine konkrete Löschfrist;
maßgeblich ist daher die Dauer, für die GitHub sie für den genannten Sicherheitszweck
benötigt.

## 4. Übermittlungen in die USA

GitHub, Inc. und Cloudflare, Inc. haben ihren Sitz in den USA und sind nach dem
**EU-U.S. Data Privacy Framework** zertifiziert. Für zertifizierte Unternehmen hat die
Europäische Kommission ein angemessenes Datenschutzniveau festgestellt. Soweit die in
dieser Erklärung beschriebenen Daten in die USA übermittelt werden, beruht dies auf
Art. 45 Abs. 1 DSGVO. Die Zertifizierungen können über die
[Teilnehmerliste des Data Privacy Framework](https://www.dataprivacyframework.gov/list)
geprüft werden.

## 5. Verschlüsselung

Die Website wird ausschließlich über TLS-verschlüsselte Verbindungen (HTTPS)
ausgeliefert.

## 6. Verbindungen des Kartenbaukastens

Bei den folgenden Abrufen wird die IP-Adresse an den jeweiligen Server übertragen.
Das ist technisch erforderlich, damit der Server die angeforderten Daten an den
Browser zurücksenden kann. Rechtsgrundlage ist jeweils Art. 6 Abs. 1 lit. f DSGVO.

### 6.1 Kartenkacheln von OpenStreetMap

Kartenansichten laden Kacheln unmittelbar von `tile.openstreetmap.org`, betrieben von
der OpenStreetMap Foundation, St John's Innovation Centre, Cowley Road, Cambridge
CB4 0WS, Vereinigtes Königreich. Dabei erhält die OpenStreetMap Foundation die
IP-Adresse und den angefragten Kartenausschnitt. Die Kacheln erscheinen bereits in der
ersten Ansicht des Kartenbaukastens, weil die Auswahl einer Region ohne
Kartenhintergrund nicht sinnvoll bedienbar wäre.

Das berechtigte Interesse liegt in der Anzeige der für die Regionsauswahl und lokale
Vorschau erforderlichen Karte. Für das Vereinigte Königreich besteht ein
Angemessenheitsbeschluss nach Art. 45 DSGVO. Die OpenStreetMap Foundation verarbeitet
die Abrufe in eigener Verantwortung; ihre
[Datenschutzerklärung](https://osmfoundation.org/wiki/Privacy_Policy) gilt ergänzend.

### 6.2 Prüfung auf neue Firmware

Nach dem Anschluss eines OpenBikeComputer ruft der Baukasten einmalig die aktuelle
Firmware-Beschreibung unter `updates.openbikecomputer.com` ab. Die Auslieferung erfolgt
über **Cloudflare R2** der Cloudflare, Inc., 101 Townsend St., San Francisco,
CA 94107, USA.

Die Anfrage enthält weder Seriennummer noch installierte Firmware-Version. Der
Vergleich findet lokal im Browser statt. Ohne angeschlossenes Gerät wird die Datei
nicht abgerufen. Das berechtigte Interesse liegt darin, auf verfügbare, insbesondere
sicherheitsrelevante Aktualisierungen hinzuweisen.

### 6.3 Kartendaten über Cloudflare R2

Der Katalog, Vorschauen, Zellverzeichnisse und Kartenzellen werden unter
`maps.openbikecomputer.com` ebenfalls über Cloudflare R2 ausgeliefert. Der Katalog wird
beim Öffnen des Kartenbaukastens geladen; weitere Dateien werden entsprechend der
Auswahl angefordert. Aus den angefragten Zellen kann Cloudflare den ungefähren
gewählten Kartenbereich erkennen.

Der verwendete R2-Bucket besitzt **keine EU-Jurisdiktionsbeschränkung**. Cloudflare
wird damit keine ausschließlich auf die EU begrenzte Speicherung oder Verarbeitung
vorgegeben. Für Übermittlungen in die USA gilt Abschnitt 4.

Der Verantwortliche hat für R2 **kein Logpush** aktiviert, exportiert oder analysiert
also keine R2-Zugriffsprotokolle. In R2 nutzt er nur zusammengefasste Betriebsmetriken
wie Anzahl und Datenmenge der Anfragen. **Network Error Logging** ist ebenfalls
deaktiviert; der Browser sendet keine entsprechenden Fehlerberichte an Cloudflare.
Unabhängig davon kann Cloudflare technische Daten in dem Umfang verarbeiten, der für
Auslieferung, Sicherheit und Betrieb des Dienstes erforderlich ist.

Das berechtigte Interesse liegt darin, die unabhängig aktualisierten und für eine
Einbindung in die Website zu großen Kartendaten bereitzustellen.

## 7. Speicherung auf dem Endgerät

Der Kartenbaukasten verwendet `localStorage` und das *Origin Private File System*
(OPFS) des Browsers. Diese Daten bleiben auf dem verwendeten Gerät und werden nicht an
den Verantwortlichen übertragen.

| Daten | Zweck und Dauer |
| --- | --- |
| Aktuelle Karten- und Schemakonfiguration | Automatischer lokaler Arbeitsstand, damit eine Bearbeitung einen Reload übersteht; bis zum Ersetzen oder Löschen der Website-Daten. |
| Selbst gespeicherte Skins mit Namen und Farben | Nur nach Betätigung von „Save custom skin“; bis zur Löschung über die vorhandene Löschfunktion oder durch Löschen der Website-Daten. |
| Kartenzellen und Arbeitsdateien im OPFS | Für große Karten während Download und Zusammensetzen erforderlich. Standardmäßig werden die Zellen nach dem Lauf gelöscht. Sortierdateien werden am Laufende entfernt; ältere Ausgabedateien spätestens beim Beginn des nächsten Laufs oder über die Löschfunktion. |
| Optional weiterverwendete Kartenzellen | Nur wenn „Keep downloaded map cells for future builds“ aktiviert wird; bis zum Deaktivieren, bis „Delete stored map data“ gewählt wird, bis Browserdaten gelöscht werden oder eine neue Kataloggeneration die alte ersetzt. |
| Entscheidung über die Wiederverwendung von Kartenzellen | Damit die ausdrücklich gewählte Einstellung bei einem späteren Besuch erhalten bleibt; bis zur nächsten Änderung oder zum Löschen der Website-Daten. |
| Beantwortete Firmware-Hinweise mit Geräte-Seriennummer und angebotener Version | Erst nachdem ein Hinweis geschlossen oder aufgerufen wurde, damit dieselbe Frage für dasselbe Gerät nicht ständig erscheint; höchstens die 32 jüngsten Antworten, bis die Website-Daten gelöscht werden. |

Das kurzfristige Speichern und Auslesen der Arbeitsdaten sowie das Speichern eines vom
Nutzer ausdrücklich gesicherten Skins oder beantworteten Firmware-Hinweises ist für
die jeweils angeforderte Funktion erforderlich (§ 25 Abs. 2 Nr. 2 TDDDG). Für diese
Vorgänge ist keine Einwilligung erforderlich.

Die Wiederverwendung heruntergeladener Kartenzellen bei späteren Builds ist dagegen
optional und standardmäßig ausgeschaltet. Das Aktivieren der deutlich bezeichneten
Option ist die Einwilligung nach § 25 Abs. 1 TDDDG. Sie kann jederzeit durch
Deaktivieren der Option oder über „Delete stored map data“ mit Wirkung für die Zukunft
widerrufen werden. Die Rechtmäßigkeit der bisherigen Speicherung bleibt unberührt.

## 8. Lokale Verarbeitung von Dateien und Gerätedaten

Die Gerätedemo, die Umwandlung ausgewählter Routendateien und das Zusammensetzen der
Karte laufen lokal im Browser. Ausgewählte oder in das Fenster gezogene Dateien werden
nicht an einen Server übertragen. Das gilt insbesondere für Routendateien, die
Positionsdaten enthalten können.

Eine WebUSB-Verbindung entsteht erst, nachdem ein Gerät im Auswahldialog des Browsers
bestätigt wurde. Die Daten fließen unmittelbar zwischen Browser und Gerät. Für die
Vorschaubilder der Geräteübersicht werden vereinfachte Streckenverläufe im Arbeitsspeicher
gehalten, jedoch **nicht dauerhaft im Browser gespeichert**. Sie werden bei einem Reload
oder beim Ende der Browsersitzung verworfen.

## 9. Externe Links

Links zum Quelltext-Repository, zu OpenStreetMap und zu technischen Referenzen werden
erst nach einem Klick aufgerufen. Es findet kein Vorabruf statt. Nach dem Anklicken gilt
die Datenschutzerklärung des jeweiligen Anbieters. Beiträge zu GitHub-Issues oder
Pull Requests können entsprechend den dortigen Einstellungen öffentlich sein.

## 10. Kontakt per E-Mail

Bei einer Nachricht an `openbikecomputer@proton.me` werden Absenderadresse, Name,
Inhalt und freiwillig mitgeteilte Angaben zur Bearbeitung der Anfrage verarbeitet.
Rechtsgrundlage ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse liegt in der
Beantwortung von Anfragen. Soweit eine Anfrage auf die Vorbereitung eines Vertrags
gerichtet ist, gilt zusätzlich Art. 6 Abs. 1 lit. b DSGVO.

Das Postfach wird über **Proton Mail** der Proton AG, Route de la Galaise 32,
1228 Plan-les-Ouates, Genf, Schweiz, betrieben. Für die Schweiz besteht ein
Angemessenheitsbeschluss der Europäischen Kommission nach Art. 45 DSGVO. Ergänzend gilt
die [Datenschutzerklärung von Proton](https://proton.me/legal/privacy).

Offensichtlicher Spam wird unverzüglich gelöscht. Sonstige Nachrichten werden spätestens
sechs Monate nach abschließender Bearbeitung gelöscht, sofern keine gesetzlichen
Aufbewahrungspflichten oder die Geltendmachung, Ausübung oder Verteidigung von
Rechtsansprüchen eine längere Speicherung erfordern.

## 11. Rechte betroffener Personen

Soweit die gesetzlichen Voraussetzungen vorliegen, bestehen die Rechte auf Auskunft
(Art. 15 DSGVO), Berichtigung (Art. 16 DSGVO), Löschung (Art. 17 DSGVO), Einschränkung
der Verarbeitung (Art. 18 DSGVO) und Datenübertragbarkeit (Art. 20 DSGVO).

Bei Verarbeitungen auf Grundlage von Art. 6 Abs. 1 lit. f DSGVO besteht nach Art. 21
DSGVO das Recht, aus Gründen, die sich aus der besonderen Situation der betroffenen
Person ergeben, Widerspruch einzulegen. Eine formlose Nachricht an die in Abschnitt 1
genannte Adresse genügt.

Unabhängig davon besteht nach Art. 77 DSGVO das Recht, sich bei einer
Datenschutz-Aufsichtsbehörde zu beschweren, insbesondere am Aufenthaltsort, Arbeitsplatz
oder Ort des mutmaßlichen Verstoßes.

## 12. Automatisierte Entscheidungen

Eine automatisierte Entscheidungsfindung einschließlich Profiling nach Art. 22 DSGVO
findet nicht statt.

## 13. Änderungen

Diese Erklärung wird angepasst, wenn sich die Website, ihre Anbieter oder die
beschriebenen Verarbeitungen ändern.

---

*Stand: 9. August 2026*
