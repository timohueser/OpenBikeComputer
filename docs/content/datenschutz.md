---
title: Datenschutzerklärung
description: Welche personenbezogenen Daten openbikecomputer.com verarbeitet — Hosting bei GitHub Pages, die Verbindungen und Speicherorte des Kartenbaukastens, keine Cookies und kein Tracking. Mit englischer Fassung.
---

<!--
  ⚠  DIESE SEITE ENTHÄLT PLATZHALTERDATEN — VOR DEM LIVEGANG ERSETZEN.

  Dieselben Platzhalter wie in impressum.md (Name, Anschrift, E-Mail, Telefon) plus:

    [zuständige Landesbehörde eintragen] -> die Datenschutz-Aufsichtsbehörde des
                                  Bundeslandes, in dem der Verantwortliche wohnt. Art. 77
                                  DSGVO verlangt nur den Hinweis auf das Beschwerderecht,
                                  nicht die Benennung der Behörde — die Nennung ist
                                  Service und sollte dann aber stimmen.

  Danach den Platzhalter-Hinweis (die Callout-Box unter der Überschrift) löschen.

  ── Was hier NOCH GEPRÜFT WERDEN MUSS ──────────────────────────────────────────────

  1. Abschnitt 3 sagt bewusst NICHT mehr, dass mit GitHub ein Auftragsverarbeitungs-
     vertrag nach Art. 28 DSGVO besteht. Die GitHub Data Protection Agreement hängt am
     GitHub Customer Agreement; für einen kostenlosen persönlichen Account unter den
     normalen Terms of Service ist sie nicht ohne Weiteres einschlägig, und sie nennt
     GitHub Pages nirgends. Wenn ein Vertrag tatsächlich besteht (z. B. über eine
     kostenpflichtige Organisation), gehört er hier wieder hinein — dann aber auch der
     Fundort der Standardvertragsklauseln nach Art. 13 Abs. 1 lit. f DSGVO.
  2. Abschnitt 3 nennt keine konkrete Speicherdauer für die Server-Logfiles, weil GitHub
     für Pages keine veröffentlicht, auf die man sich berufen könnte. Art. 13 Abs. 2
     lit. a DSGVO will die Dauer ODER die Kriterien — hier stehen die Kriterien. Wenn
     GitHub eine Frist veröffentlicht, gehört sie hierher.
  3. Die Kartenkacheln (Abschnitt 6.1) werden ohne Einwilligung geladen, und das ist
     Absicht — nicht eine offene Frage. § 25 TDDDG greift hier gar nicht: Ein Kachelabruf
     überträgt Daten, er speichert oder liest nichts auf dem Endgerät. Zu prüfen ist also
     nur Art. 6 Abs. 1 lit. f DSGVO, und die Abwägung ist nicht knapp: Die
     OpenStreetMap Foundation ist ein Verein im Vereinigten Königreich (Angemessenheits-
     beschluss), betreibt kein Werbegeschäft, bildet keine Profile — und die Karte ist
     hier nicht Zierde, sondern die Funktion, wegen der die Seite geöffnet wird. Die aus
     der Google-Maps- und Google-Fonts-Diskussion bekannte "Zwei-Klick-Lösung" adressiert
     ein Risiko, das bei OSM nicht besteht; die übliche und tragfähige Umsetzung ist
     genau die hiesige: Kacheln direkt von openstreetmap.org, Leaflet lokal gebündelt
     (kein CDN) und der Empfänger hier benannt.

     Eine Klarstellung dazu, weil Generator-Bausteine das oft falsch haben: Die
     OpenStreetMap Foundation ist NICHT Auftragsverarbeiterin. Sie handelt in eigener
     Verantwortung; ein Vertrag nach Art. 28 DSGVO ist weder nötig noch möglich. Sie
     gehört als Empfängerin genannt — und ist es.

  ── Was diese Erklärung MITFÜHREN MUSS, wenn sich der Code ändert ──────────────────

  Sie ist am tatsächlichen Verhalten des Deployments entlanggeschrieben, nicht aus einem
  Generator zusammengesetzt. Wenn sich das Verhalten ändert, ändert sich die Erklärung:

  - Ein Analyse- oder Fehler-Tracking-Dienst, eine Schriftart von einem fremden CDN, ein
    Video-Embed, ein Kontaktformular oder ein Spenden-Button ändern Abschnitt 2.
  - JEDE neue Verbindung zu einem fremden Origin gehört in Abschnitt 6. Der Bestand
    heute: die Kartenkacheln (lib/map/coverageMap.ts, components/device/RideMap.svelte,
    components/device/PreviewModal.svelte) und die Firmware-Prüfung
    (lib/firmware/release.ts).
  - Abschnitt 6.3 beschreibt den Katalog, der in der Repository-Variable
    OBC_CATALOG_URL steht und den der Deploy als VITE_CATALOG_URL in den Build gibt
    (siehe .github/workflows/deploy-site.yml). Stand dieser Fassung:
    https://maps.openbikecomputer.com/cell-catalog/catalog.json, ausgeliefert von
    Cloudflare R2. Wird die Variable auf einen anderen Host umgestellt oder geleert,
    stimmt Abschnitt 6.3 nicht mehr — und das passiert in der GitHub-Oberfläche, ohne
    Commit und ohne Review. Ein Deploy-Schritt, der Variable und Seitentext gegeneinander
    prüft, wäre die einzige Absicherung, die nicht auf Disziplin beruht.

    Diese Erklärung hat genau an dieser Stelle schon einmal falsch gelegen: Sie behauptete,
    es sei "kein Katalog konfiguriert", weil der Kommentar im Deploy-Workflow das noch
    sagte — die Variable war längst gesetzt. Der Workflow-Kommentar ist keine Quelle;
    `gh variable list` ist eine.
  - Neue Speicherorte auf dem Endgerät gehören in die Tabelle in Abschnitt 7, und es ist
    jedes Mal neu zu prüfen, ob sie noch "unbedingt erforderlich" im Sinne des
    § 25 Abs. 2 Nr. 2 TDDDG sind. Sobald etwas gespeichert wird, das NICHT für die vom
    Nutzer angeforderte Funktion nötig ist (Reichweitenmessung, Wiedererkennung), wird
    eine Einwilligung fällig — und damit ein Consent-Dialog.
  - Ein Wechsel weg von GitHub Pages betrifft die Abschnitte 3 und 4 vollständig.
-->

# Datenschutzerklärung

> **Platzhalter — noch nicht wirksam.** Diese Seite nennt einen frei erfundenen
> Verantwortlichen. Vor der Veröffentlichung müssen Name, Anschrift, E-Mail-Adresse,
> Rufnummer und die zuständige Aufsichtsbehörde durch die echten Angaben ersetzt und
> dieser Hinweis entfernt werden.
> *(Placeholder — not legally effective. The controller named below is fictional.)*

## 1. Verantwortlicher

Verantwortlicher im Sinne des Art. 4 Nr. 7 DSGVO ist:

<address>
  Jonas Falkenrath<br>
  Musterstraße 12<br>
  12345 Musterstadt<br>
  Deutschland<br>
  E-Mail: <a href="mailto:kontakt@example.com">kontakt@example.com</a><br>
  Telefon: +49 30 23125 000
</address>

Ein Datenschutzbeauftragter ist nicht bestellt; die Voraussetzungen des Art. 37 DSGVO in
Verbindung mit § 38 BDSG liegen nicht vor.

## 2. Zwei Bereiche, zwei Antworten

Dieses Angebot besteht aus zwei sehr unterschiedlichen Teilen, und es wäre irreführend,
sie in einem Satz zusammenzufassen.

**Die Website** — Startseite mit der im Browser laufenden Gerätedemo, Dokumentation und
Blog — ist eine reine Sammlung statischer Dateien. Sie setzt **keine Cookies**, bindet
**keine Analysedienste, Zählpixel, Fehler-Tracker, Werbenetzwerke oder
Social-Media-Plugins** ein, lädt **keine Web-Schriftarten und keine Inhalte von fremden
Servern** nach (Text wird in den auf Ihrem Gerät vorhandenen Systemschriften gesetzt),
speichert **nichts auf Ihrem Endgerät** und nimmt **keine Eingaben** entgegen. Was
unvermeidbar bleibt, ist die Übertragung Ihrer IP-Adresse an den Hoster — das beschreibt
Abschnitt 3.

**Der Kartenbaukasten** unter `/builder/` ist eine Anwendung und hat eine deutlich
größere Oberfläche: Er lädt Kartenkacheln von den OpenStreetMap-Servern und die
Kartendaten selbst aus einem eigenen Objektspeicher, prüft auf neue Firmware, speichert
Arbeitsstände auf Ihrem Endgerät und kann mit einem angeschlossenen Gerät sprechen. Auch
er setzt keine Cookies und misst keine Reichweite. Die Abschnitte 6 bis 9 beschreiben ihn
im Einzelnen.

## 3. Hosting und Server-Logfiles

Die Website wird über **GitHub Pages** ausgeliefert, einen Dienst der

<address>
  GitHub, Inc.<br>
  88 Colin P. Kelly Jr. Street<br>
  San Francisco, CA 94107<br>
  USA
</address>

Beim Abruf einer Seite verarbeitet GitHub die technischen Zugriffsdaten, die jeder
Browser übermittelt — insbesondere die **IP-Adresse** des anfragenden Geräts, Datum und
Uhrzeit des Zugriffs, die abgerufene Adresse, den übertragenen Datenumfang, den
HTTP-Statuscode sowie die vom Browser gemeldete Kennung (User-Agent). GitHub gibt für
GitHub Pages ausdrücklich an, die IP-Adresse der Besucherinnen und Besucher aus
Sicherheitsgründen zu protokollieren. Die Auslieferung erfolgt über ein Content Delivery
Network, sodass die Anfrage von einem Server in der Nähe des Abrufs beantwortet wird.

**Zweck** dieser Verarbeitung ist die technische Auslieferung der Seiten sowie die
Sicherheit und Stabilität des Angebots. **Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f
DSGVO; das berechtigte Interesse besteht darin, die Website überhaupt bereitstellen und
gegen Angriffe schützen zu können.

Die Übermittlung der IP-Adresse ist technisch unvermeidbar: Ohne sie kann keine Seite
ausgeliefert werden. Eine gesetzliche oder vertragliche Pflicht zur Bereitstellung
besteht nicht, und es entsteht Ihnen daraus kein Nachteil außer dem, dass die Seite dann
nicht abrufbar ist.

> **Hinweis zur Ehrlichkeit dieser Angabe:** Die Zugriffsdaten fallen bei GitHub im
> Rahmen des Plattformbetriebs an. Der Verantwortliche hat auf sie **weder Zugriff noch
> Einfluss**, erhält von GitHub keine Auswertung, wertet die Zugriffe nicht aus und
> führt sie mit keinen anderen Daten zusammen. Er kann die Speicherdauer auch nicht
> festlegen: Maßgeblich ist, wie lange GitHub die Daten für den Sicherheitszweck
> benötigt; eine von GitHub veröffentlichte Frist für Pages-Zugriffsprotokolle, auf die
> hier verwiesen werden könnte, existiert nicht. Insoweit verarbeitet GitHub die Daten
> zu eigenen Sicherheitszwecken in eigener Verantwortung.

## 4. Datenübermittlung in die USA

GitHub, Inc. hat ihren Sitz in den Vereinigten Staaten; mit dem Abruf der Website werden
die in Abschnitt 3 genannten Daten dorthin übermittelt.

GitHub, Inc. ist nach dem **EU-U.S. Data Privacy Framework** zertifiziert. Für
zertifizierte Unternehmen hat die Europäische Kommission mit dem Angemessenheitsbeschluss
vom 10. Juli 2023 ein angemessenes Schutzniveau festgestellt; die Übermittlung stützt
sich daher auf **Art. 45 Abs. 1 DSGVO**. Die aktuelle Zertifizierung ist über die
Teilnehmerliste unter [dataprivacyframework.gov/list](https://www.dataprivacyframework.gov/list) abrufbar.

Trotz dieser Grundlage lässt sich nicht ausschließen, dass US-amerikanische Behörden auf
Grundlage dortiger Gesetze auf die Daten zugreifen und dass der Rechtsschutz gegen einen
solchen Zugriff nicht dem im Geltungsbereich der DSGVO entspricht. Der
Angemessenheitsbeschluss wird zudem regelmäßig überprüft und ist Gegenstand
gerichtlicher Verfahren.

## 5. Verschlüsselung

Die Website wird über eine mit TLS verschlüsselte Verbindung (HTTPS) ausgeliefert. Damit
sind die Inhalte auf dem Transportweg gegen Mitlesen geschützt.

## 6. Der Kartenbaukasten: Verbindungen zu fremden Servern

Der Kartenbaukasten unter `/builder/` stellt im Betrieb Verbindungen zu Servern her, die
nicht vom Verantwortlichen betrieben werden. Dabei wird jeweils Ihre IP-Adresse an den
angefragten Server übertragen — das ist bei einem Abruf technisch nicht vermeidbar.

### 6.1 Kartenkacheln von OpenStreetMap

Wo der Baukasten eine Landkarte zeigt — die Abdeckungskarte bei der Auswahl einer
Region, die Vorschau einer Route und die Kartenansicht einer aufgezeichneten Fahrt —
lädt er die Kartenkacheln direkt von den Servern der **OpenStreetMap Foundation**
(St John's Innovation Centre, Cowley Road, Cambridge CB4 0WS, Vereinigtes Königreich)
unter `tile.openstreetmap.org`. Dabei erfährt die OpenStreetMap Foundation Ihre
IP-Adresse und den angefragten Kartenausschnitt.

Die Abdeckungskarte ist die **erste Ansicht** des Baukastens: Kacheln werden daher
bereits beim Öffnen von `/builder/` geladen, nicht erst nach einer Auswahl. Die
Startseite dieser Website, die Dokumentation und der Blog laden keine Kacheln.

**Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse besteht
darin, überhaupt eine Karte anzeigen zu können — ohne Kartenhintergrund ist die Auswahl
einer Region nicht bedienbar. Für das Vereinigte Königreich besteht ein
Angemessenheitsbeschluss der Europäischen Kommission (Art. 45 DSGVO). Die
OpenStreetMap Foundation ist nicht Auftragsverarbeiterin, sondern Empfängerin in eigener
Verantwortung; ihre Datenschutzerklärung ist unter
[wiki.osmfoundation.org](https://wiki.osmfoundation.org/wiki/Privacy_Policy) abrufbar.

### 6.2 Prüfung auf neue Firmware

Sobald ein OpenBikeComputer angeschlossen ist, prüft der Baukasten einmalig, ob für das
Gerät eine neuere Firmware veröffentlicht ist. Dazu ruft er eine Datei unter
`updates.openbikecomputer.com` ab. Diese Domain gehört dem Verantwortlichen; die Dateien
werden über den Objektspeicher **Cloudflare R2** der Cloudflare, Inc. (101 Townsend St.,
San Francisco, CA 94107, USA) ausgeliefert, die dabei Ihre IP-Adresse verarbeitet.
Cloudflare, Inc. ist nach dem EU-U.S. Data Privacy Framework zertifiziert; im Übrigen
gilt Abschnitt 4 entsprechend.

Der Abruf ist eine reine **Leseanfrage ohne Parameter**: Es werden weder die
Seriennummer noch die Firmware-Version noch sonstige Angaben zu Ihrem Gerät an den
Server übertragen. Der Vergleich zwischen der veröffentlichten und der auf Ihrem Gerät
laufenden Version findet vollständig in Ihrem Browser statt. Solange kein Gerät
angeschlossen ist, findet der Abruf nicht statt.

**Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse besteht
darin, Nutzerinnen und Nutzer auf sicherheitsrelevante Aktualisierungen ihres Geräts
hinweisen zu können.

### 6.3 Kartendaten

Die fertigen Kartendaten liegen nicht auf dieser Website, sondern in einem eigenen
Objektspeicher unter `maps.openbikecomputer.com`. Auch diese Domain gehört dem
Verantwortlichen, und auch hier ist es **Cloudflare R2** der Cloudflare, Inc.
(101 Townsend St., San Francisco, CA 94107, USA), das die Dateien ausliefert und dabei
Ihre IP-Adresse verarbeitet. Für die Übermittlung in die USA gilt Abschnitt 6.2
entsprechend.

Von dort geladen werden: das Verzeichnis der verfügbaren Regionen (der Katalog), die
Vorschaubilder der Darstellungsvarianten, die Verzeichnisse der Kartenzellen und
schließlich die Kartenzellen selbst. Der Katalog wird **unmittelbar beim Öffnen** von
`/builder/` abgerufen, damit die Seite überhaupt anzeigen kann, welche Regionen es gibt;
alles Weitere folgt Ihren Klicks, und eine große Karte kann dabei aus vielen einzelnen
Zellen bestehen.

Die Abrufe sind Leseanfragen ohne Anmeldung und ohne Kennung; welche Regionen und Zellen
angefragt werden, ergibt sich aus Ihrer Auswahl. **Rechtsgrundlage** ist Art. 6 Abs. 1
lit. f DSGVO; das berechtigte Interesse besteht darin, Kartendaten überhaupt bereitstellen
zu können, ohne sie in die Website selbst einbauen zu müssen — sie ändern sich
unabhängig von ihr und sind zu groß dafür.

## 7. Speicherung auf Ihrem Endgerät (kein Cookie-Banner)

Der Kartenbaukasten speichert Daten **auf Ihrem Endgerät** — im lokalen Speicher
(`localStorage`) und im Dateispeicher (*Origin Private File System*) Ihres Browsers. Das
sind keine Cookies, und die Daten werden **nicht an einen Server gesendet**; sie bleiben
auf Ihrem Gerät. Website, Dokumentation und Blog speichern nichts.

| Gespeichert wird | Wofür |
| --- | --- |
| Die aktuelle Kartenkonfiguration | damit die begonnene Arbeit einen Reload und einen späteren Besuch überlebt |
| Selbst angelegte Darstellungsvarianten (Skins) mit Namen und Farben | damit eine eigene Gestaltung wiederverwendet werden kann |
| Heruntergeladene Kartenzellen und daraus zusammengesetzte Kartendateien (Dateispeicher) | damit eine große Karte nicht bei jedem Schritt neu geladen und neu gebaut werden muss |
| Vereinfachte Streckenverläufe (Koordinatenlisten) der auf dem angeschlossenen Gerät gespeicherten Routen und Fahrten, zusammen mit der Seriennummer des Geräts | damit die Vorschaubilder der Geräteübersicht nicht bei jedem Öffnen erneut über das Kabel geladen werden müssen |
| Die zuletzt angebotene Firmware-Version, zusammen mit der Seriennummer des Geräts | damit derselbe Aktualisierungshinweis nicht bei jedem Besuch erneut erscheint |

Zwei Einträge verdienen eine ausdrückliche Erwähnung, weil ihre Bezeichnung sie
harmloser klingen ließe, als sie sind: Die **Streckenverläufe sind Positionsdaten** —
sie beschreiben, wo Sie gefahren sind — und die **Seriennummer ist eine dauerhafte
Kennung Ihres Geräts**. Beides bleibt auf Ihrem Endgerät. Die Seriennummer wird nur
verwendet, um die gespeicherten Einträge dem richtigen Gerät zuzuordnen; eine
Wiedererkennung über Websites hinweg, eine Profilbildung oder eine Reichweitenmessung
findet nicht statt, und keiner dieser Werte wird an einen Server übertragen.

**Rechtsgrundlage für das Speichern und Auslesen** ist § 25 Abs. 2 Nr. 2 TDDDG: Die
Speicherung ist unbedingt erforderlich, damit der von Ihnen ausdrücklich gewünschte
Dienst — das Zusammenstellen einer Karte und die Verwaltung eines angeschlossenen
Geräts — funktioniert. Eine Einwilligung ist dafür nicht erforderlich; deshalb erscheint
auf dieser Website kein Cookie- oder Consent-Dialog. Soweit dabei personenbezogene Daten
verarbeitet werden, ist **Rechtsgrundlage Art. 6 Abs. 1 lit. f DSGVO**; das berechtigte
Interesse besteht an einer benutzbaren Anwendung, die begonnene Arbeit nicht verwirft.

Sie können diese Daten jederzeit selbst löschen, indem Sie in Ihrem Browser die
Website-Daten für diese Domain entfernen. Der Kartenbaukasten funktioniert danach
weiter, beginnt aber wieder ohne gespeicherte Arbeitsstände.

## 8. Verarbeitung im Browser: Routen, Karten und die Gerätedemo

Die Gerätedemo auf der Startseite, die Umwandlung von Routendateien (z. B. GPX) und das
Zusammensetzen von Karten laufen als WebAssembly **vollständig in Ihrem Browser**.

Dateien, die Sie dazu auswählen oder in das Fenster ziehen, werden **nicht an einen
Server übertragen**: Sie werden lokal gelesen und lokal verarbeitet. Das ist ausdrücklich
festgehalten, weil Routendateien in aller Regel Positionsdaten enthalten und damit
besonders schutzwürdig sind — diese Daten verlassen Ihr Gerät nicht, und der
Verantwortliche erhält sie zu keinem Zeitpunkt.

## 9. Verbindung zu einem Gerät

Der Kartenbaukasten kann über die WebUSB-Schnittstelle Ihres Browsers mit einem
angeschlossenen OpenBikeComputer sprechen. Die Verbindung kommt nur zustande, wenn Sie
das Gerät in dem vom Browser angezeigten Dialog selbst auswählen. Die Daten fließen dabei
ausschließlich zwischen Ihrem Browser und dem Gerät; der Verantwortliche erhält davon
nichts. Was aus dieser Verbindung auf Ihrem Endgerät gespeichert wird, steht in
Abschnitt 7.

## 10. Links zu externen Angeboten

Die Seiten verlinken an mehreren Stellen auf fremde Angebote — insbesondere auf das
Quelltext-Repository bei GitHub, auf OpenStreetMap und auf einzelne technische
Referenzen. Diese Links werden erst beim Anklicken aufgerufen; es findet kein Vorabruf
statt. Nach dem Anklicken verlassen Sie diese Website, und es gilt die
Datenschutzerklärung des jeweiligen Anbieters.

Für das Repository gilt zusätzlich: Wenn Sie sich dort beteiligen — etwa an einem Issue
oder einer Pull-Request-Diskussion —, ist das eine Nutzung von GitHub, und die dort von
Ihnen veröffentlichten Angaben sind öffentlich.

## 11. Kontaktaufnahme per E-Mail

Wenn Sie die oben genannte E-Mail-Adresse anschreiben, werden Ihre Angaben aus der
E-Mail — Absenderadresse, Name, Inhalt und alle weiteren freiwillig gemachten Angaben —
zur Bearbeitung Ihrer Anfrage verarbeitet.

**Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f DSGVO (berechtigtes Interesse an der
Beantwortung von Anfragen); zielt die Anfrage auf einen Vertrag, ist es Art. 6 Abs. 1
lit. b DSGVO. Die Angabe von Daten ist weder gesetzlich noch vertraglich
vorgeschrieben — ohne sie kann eine Anfrage allerdings nicht beantwortet werden. Die
Nachrichten werden gelöscht, sobald die Anfrage abschließend bearbeitet ist und keine
Aufbewahrungspflichten entgegenstehen.

## 12. Keine automatisierte Entscheidungsfindung

Eine automatisierte Entscheidungsfindung einschließlich Profiling nach Art. 22 DSGVO
findet nicht statt.

## 13. Ihre Rechte

Sie haben gegenüber dem Verantwortlichen die folgenden Rechte, soweit die jeweiligen
gesetzlichen Voraussetzungen vorliegen:

- **Auskunft** über die zu Ihnen verarbeiteten Daten (Art. 15 DSGVO)
- **Berichtigung** unrichtiger Daten (Art. 16 DSGVO)
- **Löschung** (Art. 17 DSGVO)
- **Einschränkung der Verarbeitung** (Art. 18 DSGVO)
- **Datenübertragbarkeit** (Art. 20 DSGVO)

Unabhängig davon haben Sie nach Art. 77 DSGVO das Recht, sich bei einer
**Datenschutz-Aufsichtsbehörde** zu beschweren, insbesondere in dem Mitgliedstaat Ihres
Aufenthaltsorts, Ihres Arbeitsplatzes oder des Orts des mutmaßlichen Verstoßes. Für den
Verantwortlichen zuständig ist **[zuständige Landesbehörde eintragen]**.

## 14. Widerspruchsrecht nach Art. 21 DSGVO

**Sie haben das Recht, aus Gründen, die sich aus Ihrer besonderen Situation ergeben,
jederzeit gegen die Verarbeitung Sie betreffender personenbezogener Daten Widerspruch
einzulegen, die auf Grundlage von Art. 6 Abs. 1 lit. f DSGVO erfolgt.** Das betrifft
alle in dieser Erklärung auf diese Vorschrift gestützten Verarbeitungen — die
Server-Logfiles (Abschnitt 3), die Verbindungen des Kartenbaukastens (Abschnitt 6), die
Speicherung auf Ihrem Endgerät (Abschnitt 7) und die Beantwortung von Anfragen
(Abschnitt 11).

Nach einem Widerspruch werden die betreffenden Daten nicht mehr verarbeitet, es sei
denn, der Verantwortliche kann zwingende schutzwürdige Gründe für die Verarbeitung
nachweisen, die Ihre Interessen, Rechte und Freiheiten überwiegen, oder die Verarbeitung
dient der Geltendmachung, Ausübung oder Verteidigung von Rechtsansprüchen. Für einen
Widerspruch genügt eine formlose Nachricht an die in Abschnitt 1 genannte Adresse.

## 15. Änderungen dieser Erklärung

Diese Erklärung wird angepasst, wenn sich die beschriebene Verarbeitung ändert — etwa
weil die Website umzieht oder eine neue Funktion hinzukommt. Maßgeblich ist die jeweils
hier veröffentlichte Fassung.

## Privacy policy (English)

> Both versions describe the same processing. In case of any difference in
> interpretation, the German version above governs.

### 1. Controller

The controller within the meaning of Art. 4(7) GDPR is:

<address>
  Jonas Falkenrath<br>
  Musterstraße 12<br>
  12345 Musterstadt<br>
  Germany<br>
  Email: <a href="mailto:kontakt@example.com">kontakt@example.com</a><br>
  Phone: +49 30 23125 000
</address>

No data protection officer has been appointed; the conditions of Art. 37 GDPR in
conjunction with § 38 BDSG are not met.

### 2. Two parts, two answers

This offering consists of two very different parts, and summarising them in one sentence
would be misleading.

**The website** — landing page with the in-browser device demo, documentation and blog —
is a plain collection of static files. It sets **no cookies**, embeds **no analytics
services, tracking pixels, error trackers, ad networks or social media plugins**, loads
**no web fonts and no content from third-party servers** (text is set in the system fonts
already present on your device), stores **nothing on your device**, and accepts **no
input**. What cannot be avoided is that your IP address reaches the host — section 3
describes this.

**The map builder** at `/builder/` is an application and has a considerably larger
surface: it loads map tiles from the OpenStreetMap servers and the map data itself from a
dedicated object store, checks for new firmware, stores work in progress on your device,
and can talk to a connected device. It too sets no cookies and measures no audience.
Sections 6 to 9 describe it in detail.

### 3. Hosting and server log files

The site is served by **GitHub Pages**, a service of GitHub, Inc., 88 Colin P. Kelly Jr.
Street, San Francisco, CA 94107, USA.

When a page is requested, GitHub processes the technical access data every browser
transmits — in particular the **IP address**, the date and time of the request, the
address requested, the volume of data transferred, the HTTP status code and the browser
identifier (user agent). GitHub explicitly states that for GitHub Pages a visitor's IP
address is logged for security purposes. Delivery uses a content delivery network, so
requests are answered from a nearby server.

The **purpose** is the technical delivery of the pages and the security and stability of
the service. The **legal basis** is Art. 6(1)(f) GDPR; the legitimate interest is being
able to provide the website at all and to protect it against attacks.

Transmitting your IP address is technically unavoidable: without it no page can be
delivered. There is no statutory or contractual obligation to provide it, and the only
consequence of not doing so is that the site cannot be retrieved.

> **A note on candour:** this access data arises at GitHub as part of running the
> platform. The controller has **neither access to it nor influence over it**, receives
> no analysis of it from GitHub, does not evaluate visits and merges the data with
> nothing else. Nor can the controller set the retention period: what governs is how long
> GitHub needs the data for its security purpose, and GitHub publishes no retention
> period for Pages access logs that could be cited here. To that extent GitHub processes
> the data for its own security purposes as its own controller.

### 4. Transfers to the USA

GitHub, Inc. is based in the United States, so requesting the website transfers the data
described in section 3 there.

GitHub, Inc. is certified under the **EU-U.S. Data Privacy Framework**. For certified
organisations the European Commission established an adequate level of protection in its
adequacy decision of 10 July 2023, so the transfer relies on **Art. 45(1) GDPR**. The
current certification can be checked in the participant list at
[dataprivacyframework.gov/list](https://www.dataprivacyframework.gov/list).

Despite this basis it cannot be ruled out that US authorities access the data under US
law, and that the legal remedies against such access do not match those available within
the scope of the GDPR. The adequacy decision is also subject to periodic review and to
pending litigation.

### 5. Encryption

The website is served over a TLS-encrypted connection (HTTPS).

### 6. The map builder: connections to third-party servers

In operation, the map builder at `/builder/` connects to servers not operated by the
controller. Each such request transmits your IP address to the server addressed — with a
retrieval that is technically unavoidable.

#### 6.1 Map tiles from OpenStreetMap

Wherever the builder shows a map — the coverage map when selecting a region, the preview
of a route, and the map view of a recorded ride — it loads the map tiles directly from
the servers of the **OpenStreetMap Foundation** (St John's Innovation Centre, Cowley
Road, Cambridge CB4 0WS, United Kingdom) at `tile.openstreetmap.org`. In doing so the
OpenStreetMap Foundation learns your IP address and the map section requested.

The coverage map is the builder's **first screen**, so tiles are loaded as soon as
`/builder/` opens rather than after a selection. This site's landing page, the
documentation and the blog load no tiles.

The **legal basis** is Art. 6(1)(f) GDPR; the legitimate interest is being able to show a
map at all — without a map background, selecting a region is not operable. An adequacy
decision of the European Commission is in place for the United Kingdom (Art. 45 GDPR). The
OpenStreetMap Foundation is not a processor but a recipient acting on its own
responsibility; its privacy policy is available at
[wiki.osmfoundation.org](https://wiki.osmfoundation.org/wiki/Privacy_Policy).

#### 6.2 Checking for new firmware

Once an OpenBikeComputer is connected, the builder checks once whether newer firmware has
been published for the device. To do so it retrieves a file from
`updates.openbikecomputer.com`. That domain belongs to the controller; the files are
served from the **Cloudflare R2** object store of Cloudflare, Inc. (101 Townsend St., San
Francisco, CA 94107, USA), which processes your IP address in doing so. Cloudflare, Inc.
is certified under the EU-U.S. Data Privacy Framework; section 4 otherwise applies
accordingly.

The retrieval is a plain **read request with no parameters**: neither the serial number
nor the firmware version nor any other detail about your device is transmitted to the
server. The comparison between the published version and the one running on your device
happens entirely in your browser. While no device is connected, the request does not
happen at all.

The **legal basis** is Art. 6(1)(f) GDPR; the legitimate interest is being able to inform
users about security-relevant updates to their device.

#### 6.3 Map data

The prepared map data does not live on this website but in a dedicated object store at
`maps.openbikecomputer.com`. That domain also belongs to the controller, and here too it is
**Cloudflare R2** of Cloudflare, Inc. (101 Townsend St., San Francisco, CA 94107, USA) that
serves the files and processes your IP address in doing so. Section 6.2 applies accordingly
to the transfer to the USA.

Loaded from there are: the index of available regions (the catalog), the preview images of
the skins, the indexes of map cells, and finally the map cells themselves. The catalog is
retrieved **immediately when `/builder/` opens**, so the page can show which regions exist
at all; everything after that follows your clicks, and one large map may consist of many
individual cells.

These are read requests without sign-in and without any identifier; which regions and cells
are requested follows from your selection. The **legal basis** is Art. 6(1)(f) GDPR; the
legitimate interest is being able to offer map data at all without building it into the
website itself — it changes independently of the site and is far too large for it.

### 7. Storage on your device (no cookie banner)

The map builder stores data **on your device** — in your browser's local storage
(`localStorage`) and file storage (*Origin Private File System*). This is not a cookie,
and the data is **never sent to a server**; it stays on your device. The website, the
documentation and the blog store nothing.

Stored are: your current map configuration; skins you created yourself, with their names
and colours; downloaded map cells and the map files assembled from them (file storage);
simplified tracks — lists of coordinates — of the routes and rides held on the connected
device, together with that device's serial number, so the previews on the device overview
need not be fetched over the cable again; and the firmware version last offered, again
together with the device's serial number, so the same update prompt does not reappear on
every visit.

Two of these deserve to be named explicitly, because their labels would make them sound
more harmless than they are: the **tracks are location data** — they describe where you
rode — and the **serial number is a persistent identifier of your device**. Both stay on
your device. The serial number is used only to attribute stored entries to the right
device; there is no cross-site recognition, profiling or audience measurement, and none of
these values is transmitted to a server.

The **legal basis for storing and reading** this data is § 25(2)(2) TDDDG: the storage is
strictly necessary for the service you explicitly requested — assembling a map and
managing a connected device — to work. No consent is required, which is why this website
shows no cookie or consent dialog. Where personal data is processed in the course of
this, the **legal basis is Art. 6(1)(f) GDPR**; the legitimate interest is a usable
application that does not discard work in progress.

You can delete this data at any time by clearing the site data for this domain in your
browser. The map builder keeps working afterwards, simply without saved work.

### 8. Processing in the browser: routes, maps and the device demo

The device demo on the landing page, the conversion of route files (e.g. GPX) and the
assembly of maps run as WebAssembly **entirely in your browser**.

Files you select or drop into the window are **not transmitted to any server**: they are
read locally and processed locally. This is stated explicitly because route files normally
contain location data and are therefore particularly sensitive — that data never leaves
your device and the controller never receives it.

### 9. Connecting a device

The map builder can talk to a connected OpenBikeComputer through your browser's WebUSB
interface. The connection is only established if you pick the device yourself in the
dialog shown by the browser. Data flows solely between your browser and the device; the
controller receives none of it. What is stored on your device as a result is covered in
section 7.

### 10. Links to external services

The pages link to third-party offerings in several places — in particular the source
repository on GitHub, OpenStreetMap, and individual technical references. These links are
only followed when clicked; nothing is fetched in advance. Once clicked you leave this
website and the privacy policy of the respective provider applies.

For the repository there is more: if you take part there — in an issue or a pull request
discussion — that is a use of GitHub, and what you post there is public.

### 11. Contact by email

If you write to the email address given above, the information in your email — sender
address, name, content and any other details you volunteer — is processed in order to
handle your enquiry.

The **legal basis** is Art. 6(1)(f) GDPR (legitimate interest in answering enquiries), or
Art. 6(1)(b) GDPR where the enquiry concerns a contract. Providing data is neither
required by law nor by contract — but without it an enquiry cannot be answered. Messages
are deleted once the enquiry has been dealt with and no retention obligations apply.

### 12. No automated decision-making

There is no automated decision-making, including profiling, within the meaning of
Art. 22 GDPR.

### 13. Your rights

Subject to the respective statutory conditions you have the right to **access**
(Art. 15), **rectification** (Art. 16), **erasure** (Art. 17), **restriction of
processing** (Art. 18) and **data portability** (Art. 20).

Independently of this, Art. 77 GDPR gives you the right to lodge a complaint with a
**data protection supervisory authority**, in particular in the Member State of your
residence, place of work or the place of the alleged infringement. The authority
competent for the controller is **[zuständige Landesbehörde eintragen]**.

### 14. Right to object under Art. 21 GDPR

**You have the right to object at any time, on grounds relating to your particular
situation, to the processing of personal data concerning you which is based on
Art. 6(1)(f) GDPR.** This covers every processing in this policy that relies on that
provision — the server log files (section 3), the map builder's connections (section 6),
the storage on your device (section 7) and the answering of enquiries (section 11).

Following an objection the data concerned will no longer be processed, unless the
controller can demonstrate compelling legitimate grounds for the processing which override
your interests, rights and freedoms, or the processing serves the establishment, exercise
or defence of legal claims. An informal message to the address in section 1 is enough.

### 15. Changes to this policy

This policy is updated when the processing it describes changes — for instance because the
site moves or a new feature is added. The version published here at the time is the one
that applies.

---

*Stand / last reviewed: 5. August 2026*
