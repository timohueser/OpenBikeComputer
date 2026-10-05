---
lang: de
title: Datenschutzerklärung
description: Informationen zur Datenverarbeitung auf openbikecomputer.com.
---

# Datenschutzerklärung

Englische Übersetzung: [Privacy notice](../privacy/).

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
Dateien sowie Gerätedaten lokal im Browser. Der Routenplaner unter `/plan/` sendet
Suchanfragen und Routen an einen eigenen Server. Die Verifikationskonsole ist nur für
freigegebene Maintainer bestimmt. Einzelheiten stehen in den Abschnitten 6 bis 10.

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
geprüft werden. Fällt der Angemessenheitsbeschluss weg, gelten die Standardvertragsklauseln
nach Art. 46 Abs. 2 lit. c DSGVO aus den Datenverarbeitungsvereinbarungen der beiden
Anbieter.

## 5. Verschlüsselung

Die Website wird ausschließlich über TLS-verschlüsselte Verbindungen (HTTPS)
ausgeliefert.

## 6. Verbindungen des Kartenbaukastens

Bei den folgenden Abrufen wird die IP-Adresse an den jeweiligen Server übertragen.
Das ist technisch erforderlich, damit der Server die angeforderten Daten an den
Browser zurücksenden kann. Rechtsgrundlage ist jeweils Art. 6 Abs. 1 lit. f DSGVO.

### 6.1 Kartenkacheln

Die Regionsauswahl, Tourenvorschauen und Karten aufgezeichneter Fahrten im Kartenbaukasten
und in der Desktop-App laden Kacheln von `tiles.openbikecomputer.com`. Das ist der Cloudflare
Worker, der auch den Routenplaner bedient (Abschnitt 7.1). Cloudflare erhält die IP-Adresse
und die angefragte Kachel. Daraus kann Cloudflare den Kartenausschnitt erkennen.
Die Streckendaten selbst werden nicht übertragen. Schriften und Symbole der Karte kommen
von `maps.openbikecomputer.com` (Abschnitt 6.3). Builds ohne feste Planereinstellungen lesen
auch die aktive Version aus dem Katalog auf diesem Host. Die Kacheln laden beim Öffnen
einer Karte. Das berechtigte Interesse liegt in der Anzeige der Karte für die Regionsauswahl
und die Ansicht von Strecken oder aufgezeichneten Fahrten.

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
vorgegeben. Für Übermittlungen in die USA gilt Abschnitt 4. Auch die Schriftarten,
Symbole und der Gerätekatalog des Routenplaners kommen von `maps.openbikecomputer.com`.

Der Verantwortliche hat für R2 **kein Logpush** aktiviert, exportiert oder analysiert
also keine R2-Zugriffsprotokolle. In R2 nutzt er nur zusammengefasste Betriebsmetriken
wie Anzahl und Datenmenge der Anfragen. **Network Error Logging** ist ebenfalls
deaktiviert; der Browser sendet keine entsprechenden Fehlerberichte an Cloudflare.
Unabhängig davon kann Cloudflare technische Daten in dem Umfang verarbeiten, der für
Auslieferung, Sicherheit und Betrieb des Dienstes erforderlich ist.

Das berechtigte Interesse liegt darin, die unabhängig aktualisierten und für eine
Einbindung in die Website zu großen Kartendaten bereitzustellen.

## 7. Routenplaner

Der Routenplaner unter `/plan/` ist eine eigene Anwendung. Er verbindet sich mit drei
Servern. Rechtsgrundlage ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse liegt
in der Bereitstellung des angefragten Planers. Beim Standort gilt Abschnitt 7.2.

### 7.1 Kacheln, Suche und Routenberechnung

**Kartenkacheln.** Karten- und Geländekacheln kommen von `tiles.openbikecomputer.com`.
Das ist ein Cloudflare Worker der Cloudflare, Inc. (Abschnitt 4), der die Kacheln aus
Cloudflare R2 liest. Cloudflare erhält die IP-Adresse und die angefragte Kachel und
kann daraus den Kartenausschnitt erkennen. Der Worker schreibt keine Zugriffsdaten.
Der Verantwortliche wertet keine Zugriffsprotokolle von Cloudflare aus.

**Suche und Routenberechnung.** Suche und Routenberechnung laufen auf einem eigenen
Server unter `releases.openbikecomputer.com`. Der Server steht bei der **Contabo GmbH**,
Aschauer Straße 32a, 81549 München, Deutschland, als Auftragsverarbeiter nach Art. 28
DSGVO. Es findet keine Übermittlung in ein Drittland statt. Der Server ruft keine externen Such- oder Routingdienste auf.

Der Server erhält:

- bei der Suche den eingegebenen Text, den sichtbaren Kartenausschnitt, das Startdatum
  und die geplante Route mit Wegpunkten, Tagesetappen und Bezeichnungen. Auf Wunsch
  kommt der Standort nach Abschnitt 7.2 hinzu.
- bei der Routenberechnung die Wegpunkte, das Profil und den sichtbaren Kartenausschnitt
  für die Kartenebenen.
- bei jeder Anfrage die IP-Adresse.

Die Anwendung schreibt keine Anfragen in ein Protokoll und speichert nichts. Der
Webserver Caddy führt kein Zugriffsprotokoll. Wenn ein Dienst hinter Caddy nicht
antwortet, zum Beispiel bei einer Aktualisierung, schreibt Caddy nur Zeitpunkt und
Fehlermeldung in das Systemprotokoll, ohne IP-Adresse und ohne angefragte Adresse. Das
Systemprotokoll löscht seine Einträge nach sieben Tagen.

### 7.2 Standort

Die Standortfunktion des Browsers wird nur ausgelöst, wenn die Nutzerin oder der Nutzer
die Standortschaltfläche betätigt und der Browser nachfragt. Das ist die Einwilligung
nach Art. 6 Abs. 1 lit. a DSGVO und § 25 Abs. 1 TDDDG. Der Standort wird dann mit jeder
Suche an den Server nach Abschnitt 7.1 gesendet, um Ergebnisse in der Nähe zu ordnen.
Er wird nicht gespeichert. Die Einwilligung kann jederzeit durch die Browsereinstellung
für Standortfreigaben mit Wirkung für die Zukunft widerrufen werden.

## 8. Verifikationskonsole

Die Verifikationskonsole unter `releases.openbikecomputer.com` dient den Maintainern des
Projekts. Sie ist nur nach einer Anmeldung nutzbar. Besucher ohne Zugang sehen nur die
Anmeldeseite.

- **Anmeldung über GitHub.** Die Anmeldung leitet zu GitHub weiter (Abschnitt 4). Die
  Konsole fragt nur die öffentliche Kennung ab und speichert die numerische
  GitHub-Kennung, den Benutzernamen und die Administratorrolle. Sie speichert weder
  E-Mail-Adresse noch Zugriffstoken. Alternativ gibt es ein lokales Administratorkonto.
- **Cookies.** Das Sitzungscookie `obc_session` (12 Stunden) und das Cookie `obc_oauth`
  (10 Minuten, nur während der Anmeldung) sind für die Anmeldung erforderlich
  (§ 25 Abs. 2 Nr. 2 TDDDG).
- **Anmeldeversuche.** Gegen wiederholte Anmeldeversuche speichert die Konsole 15 Minuten
  lang einen nicht umkehrbaren Schlüsselwert der IP-Adresse.
- **Inhalte.** Der Benutzername erscheint als Autor oder Entscheider an Anforderungen,
  Testplänen und Vorschlägen. Er bleibt zusammen mit diesen Einträgen gespeichert. Nach
  dem Entfernen eines Kontos kann der Name auf Anfrage aus den Einträgen entfernt werden.
- **Sicherung.** Eine tägliche Sicherung der Datenbank wird sieben Tage aufbewahrt.

Rechtsgrundlage ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse liegt in einem
nachvollziehbaren, gesicherten Freigabeprozess. Der Server steht bei der Contabo GmbH
(Abschnitt 7.1). Das Systemprotokoll wird wie dort beschrieben nach sieben Tagen gelöscht.

## 9. Speicherung auf dem Endgerät

Der Hell-Dunkel-Schalter der Website und des Kartenbaukastens speichert die
gewählte Darstellung lokal unter `obc-theme` in `localStorage`. Ohne Auswahl gilt die Browsereinstellung. Der
Wert wird nicht an einen Server gesendet. Er bleibt gespeichert, bis die Websitedaten
im Browser gelöscht werden.

Webanwendung und installierte Desktop-App verwenden lokalen Gerätespeicher. Der
Kartenbaukasten nutzt `localStorage` und das *Origin Private File System* (OPFS) des
Browsers. Diese Daten bleiben auf dem verwendeten Gerät und werden nicht an den
Verantwortlichen übertragen.

| Daten | Zweck und Dauer |
| --- | --- |
| Aktuelle Karten- und Schemakonfiguration | Automatischer lokaler Arbeitsstand, damit eine Bearbeitung einen Reload übersteht; bis zum Ersetzen oder Löschen der Website-Daten. |
| Selbst gespeicherte Skins mit Namen und Farben | Nur nach Betätigung von „Save custom skin“; bis zur Löschung über die vorhandene Löschfunktion oder durch Löschen der Website-Daten. |
| Kartenzellen und Arbeitsdateien im OPFS | Für große Karten während Download und Zusammensetzen erforderlich. Standardmäßig werden die Zellen nach dem Lauf gelöscht. Sortierdateien werden am Laufende entfernt; ältere Ausgabedateien spätestens beim Beginn des nächsten Laufs oder über die Löschfunktion. |
| Optional weiterverwendete Kartenzellen | Nur wenn „Keep downloaded map cells for future builds“ aktiviert wird; bis zum Deaktivieren, bis „Delete stored map data“ gewählt wird, bis Browserdaten gelöscht werden oder eine neue Kataloggeneration die alte ersetzt. |
| Entscheidung über die Wiederverwendung von Kartenzellen | Damit die ausdrücklich gewählte Einstellung bei einem späteren Besuch erhalten bleibt; bis zur nächsten Änderung oder zum Löschen der Website-Daten. |
| Beantwortete Firmware-Hinweise mit Geräte-Seriennummer und angebotener Version | Erst nachdem ein Hinweis geschlossen oder aufgerufen wurde, damit dieselbe Frage für dasselbe Gerät nicht ständig erscheint; höchstens die 32 jüngsten Antworten, bis die Website-Daten gelöscht werden. |
| Aktueller Reiseplan des Routenplaners (`obc-planner-routing-v2`) | Automatisch gespeicherter Entwurf mit Wegpunkten und Etappen, damit er einen Reload übersteht; bis zum Ersetzen oder Löschen der Website-Daten. Er verlässt den Browser nur als Suchanfrage nach Abschnitt 7.1. |
| Gespeicherte Planversionen des Routenplaners (`obc-planner-lab-versions-v1`) | Nur auf ausdrückliche Speicherung; bis zum Löschen über die Versionsliste oder durch Löschen der Website-Daten. |
| Gewähltes Fahrradprofil für Routen (`obcm.routeBikeType`) | Damit die gewählte Einstellung bei einem späteren Besuch erhalten bleibt; bis zur nächsten Änderung oder zum Löschen der Website-Daten. |
| Routen- und Tourenvorschauen in der Webanwendung | Aus dem angeschlossenen Gerät geladene, ausgedünnte Koordinaten zur Darstellung der Kacheln; nur im Arbeitsspeicher der laufenden Seitensitzung, bis zum Neuladen oder Schließen. |
| Routen- und Tourenvorschauen in der installierten Desktop-App | Ausgedünnte Koordinaten werden lokal zwischengespeichert, damit sie nach einem App-Neustart nicht erneut vom angeschlossenen Gerät geladen werden müssen. Der Cache ist auf 300 Einträge begrenzt und entfernt ältere, zuletzt nicht verwendete Einträge. Geänderte Objekte erhalten durch Inhaltsmerkmale einen neuen Cache-Schlüssel. Löschung ist auf der Geräteseite über „Delete saved previews“ oder durch Löschen der App-Daten möglich. |

Das kurzfristige Speichern und Auslesen der Arbeitsdaten, das Speichern des Reiseplans,
gespeicherter Planversionen und gewählter Einstellungen sowie das Speichern eines vom
Nutzer ausdrücklich gesicherten Skins oder beantworteten Firmware-Hinweises sowie der
begrenzte Vorschau-Cache der ausdrücklich installierten Desktop-App sind für die jeweils
angeforderte Funktion erforderlich (§ 25 Abs. 2 Nr. 2 TDDDG). Der Vorschau-Cache
vermeidet insbesondere wiederholte vollständige Übertragungen aller dargestellten
Routen und Touren über USB. Für diese Vorgänge ist keine Einwilligung erforderlich.

Die Wiederverwendung heruntergeladener Kartenzellen bei späteren Builds ist dagegen
optional und standardmäßig ausgeschaltet. Das Aktivieren der deutlich bezeichneten
Option ist die Einwilligung nach § 25 Abs. 1 TDDDG. Sie kann jederzeit durch
Deaktivieren der Option oder über „Delete stored map data“ mit Wirkung für die Zukunft
widerrufen werden. Die Rechtmäßigkeit der bisherigen Speicherung bleibt unberührt.

## 10. Lokale Verarbeitung von Dateien und Gerätedaten

Die Gerätedemo, die Umwandlung ausgewählter Routendateien und das Zusammensetzen der
Karte laufen lokal im Browser. Ausgewählte oder in das Fenster gezogene Dateien werden
nicht an einen Server übertragen. Das gilt insbesondere für Routendateien, die
Positionsdaten enthalten können.

Eine WebUSB-Verbindung entsteht erst, nachdem ein Gerät im Auswahldialog des Browsers
bestätigt wurde. Die Daten fließen unmittelbar zwischen Browser und Gerät. Zeigt eine
Karte Touren oder Fahrten, laden Browser und Desktop-App nur Kartenkacheln nach
Abschnitt 6.1; die Streckendaten verlassen das Gerät nicht. Für die
Vorschaubilder der Geräteübersicht werden vereinfachte Streckenverläufe im Arbeitsspeicher
gehalten, jedoch **nicht dauerhaft im Browser gespeichert**. Sie werden bei einem Reload
oder beim Ende der Browsersitzung verworfen.

## 11. Externe Links

Links zum Quelltext-Repository, zu OpenStreetMap und zu technischen Referenzen werden
erst nach einem Klick aufgerufen. Es findet kein Vorabruf statt. Nach dem Anklicken gilt
die Datenschutzerklärung des jeweiligen Anbieters. Beiträge zu GitHub-Issues oder
Pull Requests können entsprechend den dortigen Einstellungen öffentlich sein.

## 12. Kontakt per E-Mail

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

## 13. Rechte betroffener Personen

Soweit die gesetzlichen Voraussetzungen vorliegen, bestehen die Rechte auf Auskunft
(Art. 15 DSGVO), Berichtigung (Art. 16 DSGVO), Löschung (Art. 17 DSGVO), Einschränkung
der Verarbeitung (Art. 18 DSGVO) und Datenübertragbarkeit (Art. 20 DSGVO).

Bei Verarbeitungen auf Grundlage von Art. 6 Abs. 1 lit. f DSGVO besteht nach Art. 21
DSGVO das Recht, aus Gründen, die sich aus der besonderen Situation der betroffenen
Person ergeben, Widerspruch einzulegen. Eine formlose Nachricht an die in Abschnitt 1
genannte Adresse genügt.

Eine erteilte Einwilligung kann nach Art. 7 Abs. 3 DSGVO jederzeit mit Wirkung für die
Zukunft widerrufen werden. Die Rechtmäßigkeit der bisherigen Verarbeitung bleibt
unberührt.

Unabhängig davon besteht nach Art. 77 DSGVO das Recht, sich bei einer
Datenschutz-Aufsichtsbehörde zu beschweren, insbesondere am Aufenthaltsort, Arbeitsplatz
oder Ort des mutmaßlichen Verstoßes. Für den Verantwortlichen zuständig ist der
Landesbeauftragte für den Datenschutz und die Informationsfreiheit Baden-Württemberg,
Lautenschlagerstraße 20, 70173 Stuttgart.

## 14. Automatisierte Entscheidungen

Eine automatisierte Entscheidungsfindung einschließlich Profiling nach Art. 22 DSGVO
findet nicht statt.

## 15. Änderungen

Diese Erklärung wird angepasst, wenn sich die Website, ihre Anbieter oder die
beschriebenen Verarbeitungen ändern.

---

*Stand: 1. Oktober 2026*
