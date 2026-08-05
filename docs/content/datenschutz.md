---
title: Datenschutzerklärung
description: Welche personenbezogenen Daten openbikecomputer.com verarbeitet — Hosting bei GitHub Pages, Speicherung im Browser, keine Cookies und kein Tracking. Mit nicht verbindlicher englischer Übersetzung.
---

<!--
  ⚠  DIESE SEITE ENTHÄLT PLATZHALTERDATEN — VOR DEM LIVEGANG ERSETZEN.

  Dieselben Platzhalter wie in impressum.md (Name, Anschrift, E-Mail, Telefon) plus:

    <zuständige Landesbehörde> -> die Datenschutz-Aufsichtsbehörde des Bundeslandes,
                                  in dem der Verantwortliche wohnt. Art. 77 DSGVO
                                  verlangt nur den Hinweis auf das Beschwerderecht, nicht
                                  die Benennung der Behörde — die Nennung ist Service und
                                  sollte dann aber stimmen.

  Danach den Platzhalter-Hinweis (die Callout-Box unter der Überschrift) löschen.

  Diese Erklärung beschreibt den Stand der Website zum unten genannten Datum. Sie ist
  KEIN Generator-Baustein, sondern am tatsächlichen Verhalten des Deployments
  entlanggeschrieben; wenn sich das Verhalten ändert, muss sie mitgeführt werden:

  - Ein Analyse- oder Fehler-Tracking-Dienst, eine eingebettete Schriftart von einem
    fremden CDN, ein Video-Embed, ein Kontaktformular oder ein Spenden-Button ändern
    Abschnitt 2 und brauchen jeweils einen eigenen Abschnitt.
  - Sobald OBC_CATALOG_URL / VITE_CATALOG_URL im Deploy gesetzt ist (siehe
    .github/workflows/deploy-site.yml), lädt der Kartenbaukasten Katalog-, Kartenzellen-
    und Firmware-Objekte von einem FREMDEN Origin. Damit fließt die IP-Adresse der
    Besucher an dessen Betreiber — Abschnitt 8 muss dann von "derzeit nicht
    konfiguriert" auf eine echte Empfängerbeschreibung umgestellt werden (Betreiber,
    Ort, Rechtsgrundlage, ggf. Drittlandtransfer).
  - Wenn ein Wechsel weg von GitHub Pages ansteht, betrifft das die Abschnitte 3 und 4
    vollständig.
  - Neue localStorage-Schlüssel im Builder gehören in die Tabelle in Abschnitt 6, und es
    ist jedes Mal neu zu prüfen, ob sie noch "unbedingt erforderlich" im Sinne des
    § 25 Abs. 2 Nr. 2 TDDDG sind. Sobald etwas gespeichert wird, das NICHT für die vom
    Nutzer angeforderte Funktion nötig ist (Reichweitenmessung, Wiedererkennung), wird
    eine Einwilligung fällig — und damit ein Consent-Dialog.
-->

# Datenschutzerklärung

> **Platzhalter — noch nicht wirksam.** Diese Seite nennt einen frei erfundenen
> Verantwortlichen. Vor der Veröffentlichung müssen Name, Anschrift, E-Mail-Adresse,
> Rufnummer und die zuständige Aufsichtsbehörde durch die echten Angaben ersetzt und
> dieser Hinweis entfernt werden.
> *(Placeholder — not legally effective. The controller named below is fictional.)*

Diese Website ist die Projektseite von OpenBikeComputer: eine Landingpage mit einer im
Browser laufenden Gerätedemo, eine Dokumentation, ein Blog und ein Kartenbaukasten. Sie
ist eine reine Sammlung statischer Dateien — es gibt **keine Benutzerkonten, keine
Anmeldung, kein Kontaktformular, keinen Newsletter, keine Kommentarfunktion und keinen
Server, der Eingaben entgegennimmt.**

## 1. Verantwortlicher

Verantwortlicher im Sinne des Art. 4 Nr. 7 DSGVO ist:

<address class="legal-addr">
  Jonas Falkenrath<br>
  Musterstraße 12<br>
  12345 Musterstadt<br>
  Deutschland<br>
  E-Mail: <a href="mailto:kontakt@example.com">kontakt@example.com</a><br>
  Telefon: +49 30 23125 000
</address>

Ein Datenschutzbeauftragter ist nicht bestellt; die Voraussetzungen des § 38 BDSG liegen
nicht vor.

## 2. Kurzfassung

- **Keine Cookies.** Diese Website setzt keine Cookies.
- **Kein Tracking, keine Reichweitenmessung, keine Werbung.** Es sind keine Analysedienste, Zählpixel, Fehler-Tracker, Werbenetzwerke oder Social-Media-Plugins eingebunden.
- **Keine externen Inhalte.** Skripte, Stylesheets und Bilder werden ausschließlich vom eigenen Server ausgeliefert, und für die Darstellung werden die auf Ihrem Gerät bereits vorhandenen Systemschriften verwendet. Es werden keine Web-Schriftarten und keine Inhalte von fremden CDNs, Kartendiensten oder Videoplattformen nachgeladen.
- **Keine Eingabe von Daten.** Es gibt kein Formular und keinen Upload; Routen und Karten werden vollständig im Browser verarbeitet und verlassen das Gerät nicht.
- Unvermeidbar bleibt, dass beim Abruf der Seiten die IP-Adresse an den Hoster übertragen wird. Das beschreibt Abschnitt 3.

## 3. Hosting und Server-Logfiles

Die Website wird über **GitHub Pages** ausgeliefert, einen Dienst der

<address class="legal-addr">
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
Sicherheitsgründen zu protokollieren.

**Zweck** dieser Verarbeitung ist die technische Auslieferung der Seiten sowie die
Sicherheit und Stabilität des Angebots (Abwehr von Angriffen und Missbrauch).
**Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f DSGVO; das berechtigte Interesse besteht
darin, die Website überhaupt bereitstellen und gegen Angriffe schützen zu können.

Die Auslieferung erfolgt über ein Content Delivery Network, sodass die Anfrage von einem
Server in der Nähe des Abrufs beantwortet wird.

GitHub verarbeitet diese Daten für den Verantwortlichen als Auftragsverarbeiter auf
Grundlage der GitHub Data Protection Agreement (Art. 28 DSGVO). Soweit GitHub die
Zugriffsdaten darüber hinaus für eigene Zwecke der Sicherheit und Missbrauchsabwehr
verwendet, geschieht das in eigener Verantwortung von GitHub.

> **Hinweis zur Ehrlichkeit dieser Angabe:** Der Verantwortliche hat auf diese Logfiles
> **keinen Zugriff** und kann ihre Speicherdauer nicht konfigurieren. Die Speicherdauer
> richtet sich nach den Vorgaben von GitHub. Es findet keine Auswertung der Zugriffe
> durch den Verantwortlichen statt, und es werden keine Zugriffsdaten mit anderen Daten
> zusammengeführt.

## 4. Datenübermittlung in die USA

GitHub, Inc. hat ihren Sitz in den Vereinigten Staaten; mit dem Abruf der Website werden
die in Abschnitt 3 genannten Daten dorthin übermittelt.

GitHub, Inc. ist nach dem **EU-U.S. Data Privacy Framework** zertifiziert. Für
zertifizierte Unternehmen hat die Europäische Kommission mit dem Angemessenheitsbeschluss
vom 10. Juli 2023 ein angemessenes Schutzniveau festgestellt; die Übermittlung stützt
sich daher auf **Art. 45 Abs. 1 DSGVO**. Ergänzend hat GitHub die
**Standardvertragsklauseln** der Kommission (Durchführungsbeschluss (EU) 2021/914)
vereinbart, die als Grundlage nach Art. 46 Abs. 2 lit. c DSGVO tragen, falls der
Angemessenheitsbeschluss entfällt.

Trotz dieser Grundlagen lässt sich nicht ausschließen, dass US-amerikanische Behörden
auf Grundlage dortiger Gesetze auf die Daten zugreifen und dass der Rechtsschutz gegen
einen solchen Zugriff nicht dem im Geltungsbereich der DSGVO entspricht.
Der Angemessenheitsbeschluss wird zudem regelmäßig überprüft und ist Gegenstand
gerichtlicher Verfahren.

## 5. Verschlüsselung

Die Website wird ausschließlich über eine mit TLS verschlüsselte Verbindung (HTTPS)
ausgeliefert. Damit sind die Inhalte auf dem Transportweg gegen Mitlesen geschützt.

## 6. Speicherung auf Ihrem Endgerät (kein Cookie-Banner)

Der Kartenbaukasten unter `/builder/` speichert Daten im **lokalen Speicher
(`localStorage`) Ihres Browsers**. Das sind keine Cookies: Die Daten werden nicht an
einen Server gesendet, sondern bleiben auf Ihrem Gerät.

| Gespeichert wird | Wofür |
| --- | --- |
| Die aktuelle Kartenkonfiguration | damit die begonnene Arbeit einen Reload und einen späteren Besuch überlebt |
| Die gewählte Darstellungsvariante (Skin) | damit die Auswahl erhalten bleibt |
| Vorschaubilder der ausgewählten Karten | damit sie nicht bei jedem Öffnen neu berechnet werden müssen |
| Zuletzt angebotene Firmware-Version je Gerät | damit derselbe Aktualisierungshinweis nicht bei jedem Besuch erneut erscheint |

**Rechtsgrundlage für das Speichern und Auslesen** ist § 25 Abs. 2 Nr. 2 TDDDG: Die
Speicherung ist unbedingt erforderlich, damit der ausdrücklich gewünschte Dienst — das
Zusammenstellen einer Karte — überhaupt funktioniert. Eine Einwilligung ist dafür nicht
erforderlich; deshalb erscheint auf dieser Website kein Cookie- oder Consent-Dialog.
Eine Wiedererkennung über Websites hinweg, eine Profilbildung oder eine
Reichweitenmessung findet nicht statt.

Sie können diese Daten jederzeit selbst löschen, indem Sie in Ihrem Browser die
Website-Daten für diese Domain entfernen. Der Kartenbaukasten funktioniert danach
weiter, beginnt aber wieder ohne gespeicherte Konfiguration.

## 7. Verarbeitung im Browser: Routen, Karten und die Gerätedemo

Die Gerätedemo auf der Startseite, die Umwandlung von Routendateien (z. B. GPX) und das
Zusammensetzen von Karten laufen als WebAssembly **vollständig in Ihrem Browser**.

Dateien, die Sie dabei auswählen, werden **nicht hochgeladen**. Das ist ausdrücklich
festgehalten, weil Routendateien in aller Regel Positionsdaten enthalten und damit
besonders schutzwürdig sind: Diese Daten verlassen Ihr Gerät nicht, und der
Verantwortliche erhält sie zu keinem Zeitpunkt.

## 8. Verbindung zu einem Gerät und Downloads

**Verbindung zum Gerät.** Der Kartenbaukasten kann über die WebUSB-Schnittstelle Ihres
Browsers mit einem angeschlossenen OpenBikeComputer sprechen. Die Verbindung kommt nur
zustande, wenn Sie das Gerät in dem vom Browser angezeigten Dialog selbst auswählen.
Die Daten fließen dabei ausschließlich zwischen Ihrem Browser und dem Gerät; der
Verantwortliche erhält davon nichts.

**Karten- und Firmware-Downloads.** Fertige Kartendaten und Firmware-Dateien sollen
künftig von einem eigenen Objektspeicher geladen werden. Zum unten genannten Stand ist
für diese Website **kein solcher Katalog konfiguriert**; der Kartenbaukasten stellt
daher keine Verbindungen zu Servern Dritter her. Sobald das der Fall ist, wird diese
Erklärung vor der Freischaltung um den Betreiber, den Serverstandort und die
Rechtsgrundlage ergänzt.

## 9. Links zu externen Angeboten

Die Seiten verlinken an mehreren Stellen auf das Quelltext-Repository bei GitHub. Diese
Links werden erst beim Anklicken aufgerufen — es findet kein Vorabruf statt. Nach dem
Anklicken verlassen Sie diese Website; dann gilt die Datenschutzerklärung des jeweiligen
Anbieters.

## 10. Kontaktaufnahme per E-Mail

Wenn Sie die oben genannte E-Mail-Adresse anschreiben, werden Ihre Angaben aus der
E-Mail — Absenderadresse, Name, Inhalt und alle weiteren freiwillig gemachten Angaben —
zur Bearbeitung Ihrer Anfrage verarbeitet.

**Rechtsgrundlage** ist Art. 6 Abs. 1 lit. f DSGVO (berechtigtes Interesse an der
Beantwortung von Anfragen); zielt die Anfrage auf einen Vertrag, ist es Art. 6 Abs. 1
lit. b DSGVO. Die Angabe von Daten ist weder gesetzlich noch vertraglich
vorgeschrieben — ohne sie kann eine Anfrage allerdings nicht beantwortet werden. Die
Nachrichten werden gelöscht, sobald die Anfrage abschließend bearbeitet ist und keine
Aufbewahrungspflichten entgegenstehen.

## 11. Keine automatisierte Entscheidungsfindung

Eine automatisierte Entscheidungsfindung einschließlich Profiling nach Art. 22 DSGVO
findet nicht statt.

## 12. Ihre Rechte

Sie haben gegenüber dem Verantwortlichen die folgenden Rechte, soweit die jeweiligen
gesetzlichen Voraussetzungen vorliegen:

- **Auskunft** über die zu Ihnen verarbeiteten Daten (Art. 15 DSGVO)
- **Berichtigung** unrichtiger Daten (Art. 16 DSGVO)
- **Löschung** (Art. 17 DSGVO)
- **Einschränkung der Verarbeitung** (Art. 18 DSGVO)
- **Datenübertragbarkeit** (Art. 20 DSGVO)
- **Widerspruch** gegen Verarbeitungen, die auf Art. 6 Abs. 1 lit. f DSGVO beruhen (Art. 21 DSGVO)

> **Widerspruchsrecht:** Sie können der Verarbeitung nach Abschnitt 3 und Abschnitt 10
> aus Gründen, die sich aus Ihrer besonderen Situation ergeben, jederzeit widersprechen.
> Es genügt eine formlose Nachricht an die oben genannte Adresse.

Unabhängig davon haben Sie nach Art. 77 DSGVO das Recht, sich bei einer
**Datenschutz-Aufsichtsbehörde** zu beschweren, insbesondere in dem Mitgliedstaat Ihres
Aufenthaltsorts, Ihres Arbeitsplatzes oder des Orts des mutmaßlichen Verstoßes. Für den
Verantwortlichen zuständig ist **[zuständige Landesbehörde eintragen]**.

## 13. Änderungen dieser Erklärung

Diese Erklärung wird angepasst, wenn sich die beschriebene Verarbeitung ändert — etwa
weil die Website umzieht oder eine neue Funktion hinzukommt. Maßgeblich ist die jeweils
hier veröffentlichte Fassung.

## Privacy policy (English)

> This is a **non-binding convenience translation**. The German text above is the
> authoritative version.

### 1. Controller

The controller within the meaning of Art. 4(7) GDPR is:

<address class="legal-addr">
  Jonas Falkenrath<br>
  Musterstraße 12<br>
  12345 Musterstadt<br>
  Germany<br>
  Email: <a href="mailto:kontakt@example.com">kontakt@example.com</a><br>
  Phone: +49 30 23125 000
</address>

No data protection officer has been appointed; the conditions of § 38 BDSG are not met.

### 2. Summary

- **No cookies.** This website sets no cookies.
- **No tracking, no analytics, no advertising.** There are no analytics services, tracking pixels, error trackers, ad networks or social media plugins.
- **No external content.** Scripts, stylesheets and images are served exclusively from our own origin, and text is set in the system fonts already present on your device. No web fonts and no content from third-party CDNs, map services or video platforms are loaded.
- **No data entry.** There is no form and no upload; routes and maps are processed entirely in the browser and never leave your device.
- What cannot be avoided is that your IP address reaches the host when a page is requested. Section 3 describes this.

### 3. Hosting and server log files

The site is served by **GitHub Pages**, a service of GitHub, Inc., 88 Colin P. Kelly Jr.
Street, San Francisco, CA 94107, USA.

When a page is requested, GitHub processes the technical access data every browser
transmits — in particular the **IP address**, the date and time of the request, the
address requested, the volume of data transferred, the HTTP status code and the browser
identifier (user agent). GitHub explicitly states that for GitHub Pages a visitor's IP
address is logged for security purposes.

The **purpose** is the technical delivery of the pages and the security and stability of
the service. The **legal basis** is Art. 6(1)(f) GDPR; the legitimate interest is being
able to provide the website at all and to protect it against attacks. Delivery uses a
content delivery network, so requests are answered from a nearby server.

GitHub processes this data on behalf of the controller as a processor under the GitHub
Data Protection Agreement (Art. 28 GDPR). Where GitHub additionally uses access data for
its own security and abuse-prevention purposes, it does so as its own controller.

> **A note on candour:** the controller has **no access** to these log files and cannot
> configure their retention period, which is determined by GitHub. The controller
> performs no analysis of access data and merges it with no other data.

### 4. Transfers to the USA

GitHub, Inc. is based in the United States, so requesting the website transfers the data
described in section 3 there.

GitHub, Inc. is certified under the **EU-U.S. Data Privacy Framework**. For certified
organisations the European Commission established an adequate level of protection in its
adequacy decision of 10 July 2023, so the transfer relies on **Art. 45(1) GDPR**. In
addition, GitHub has agreed to the Commission's **Standard Contractual Clauses**
(Implementing Decision (EU) 2021/914), which serve as a basis under Art. 46(2)(c) GDPR
should the adequacy decision cease to apply.

Despite these safeguards it cannot be ruled out that US authorities access the data
under US law, and that the legal remedies against such access do not match those
available within the scope of the GDPR. The adequacy decision is also subject to
periodic review and to pending litigation.

### 5. Encryption

The website is served exclusively over a TLS-encrypted connection (HTTPS).

### 6. Storage on your device (no cookie banner)

The map builder at `/builder/` stores data in your browser's **local storage**. This is
not a cookie: the data is never sent to a server, it stays on your device. Stored are
your current map configuration, the selected skin, preview thumbnails of selected maps,
and the firmware version last offered per device (so the same update prompt does not
reappear on every visit).

The **legal basis for storing and reading** this data is § 25(2)(2) TDDDG: the storage
is strictly necessary for the service you explicitly requested — assembling a map — to
work at all. No consent is required, which is why this website shows no cookie or
consent dialog. There is no cross-site recognition, profiling or audience measurement.

You can delete this data at any time by clearing the site data for this domain in your
browser. The map builder keeps working afterwards, simply without a saved
configuration.

### 7. Processing in the browser: routes, maps and the device demo

The device demo on the landing page, the conversion of route files (e.g. GPX) and the
assembly of maps run as WebAssembly **entirely in your browser**.

Files you select are **not uploaded**. This is stated explicitly because route files
normally contain location data and are therefore particularly sensitive: that data never
leaves your device and the controller never receives it.

### 8. Connecting a device, and downloads

**Device connection.** The map builder can talk to a connected OpenBikeComputer through
your browser's WebUSB interface. The connection is only established if you pick the
device yourself in the dialog shown by the browser. Data flows solely between your
browser and the device; the controller receives none of it.

**Map and firmware downloads.** Prepared map data and firmware files are intended to be
loaded from a dedicated object store in future. As of the date given below **no such
catalog is configured** for this website, so the map builder makes no connections to
third-party servers. As soon as it does, this policy will be extended — before that
feature goes live — to name the operator, the server location and the legal basis.

### 9. Links to external services

The pages link to the source repository on GitHub in several places. These links are
only followed when clicked — nothing is fetched in advance. Once clicked you leave this
website and the privacy policy of the respective provider applies.

### 10. Contact by email

If you write to the email address given above, the information in your email — sender
address, name, content and any other details you volunteer — is processed in order to
handle your enquiry.

The **legal basis** is Art. 6(1)(f) GDPR (legitimate interest in answering enquiries),
or Art. 6(1)(b) GDPR where the enquiry concerns a contract. Providing data is neither
required by law nor by contract — but without it an enquiry cannot be answered. Messages
are deleted once the enquiry has been dealt with and no retention obligations apply.

### 11. No automated decision-making

There is no automated decision-making, including profiling, within the meaning of
Art. 22 GDPR.

### 12. Your rights

Subject to the respective statutory conditions you have the right to **access**
(Art. 15), **rectification** (Art. 16), **erasure** (Art. 17), **restriction of
processing** (Art. 18), **data portability** (Art. 20) and to **object** to processing
based on Art. 6(1)(f) GDPR (Art. 21).

> **Right to object:** you may object to the processing described in sections 3 and 10
> at any time on grounds relating to your particular situation. An informal message to
> the address above is enough.

Independently of this, Art. 77 GDPR gives you the right to lodge a complaint with a
**data protection supervisory authority**, in particular in the Member State of your
residence, place of work or the place of the alleged infringement. The authority
competent for the controller is **[zuständige Landesbehörde eintragen]**.

### 13. Changes to this policy

This policy is updated when the processing it describes changes — for instance because
the site moves or a new feature is added. The version published here at the time is the
one that applies.

---

*Stand / last reviewed: 5. August 2026*
