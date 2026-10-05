---
lang: en
title: Privacy notice
description: How openbikecomputer.com processes personal data. English translation of the Datenschutzerklärung.
---

# Privacy notice

This page is a translation of the [Datenschutzerklärung](../datenschutz/). If the two
versions differ, the German version applies. The section numbers are the same.

## 1. Controller

The controller as defined in Art. 4(7) GDPR is:

<address>
  Timo Hüser<br>
  Scharnhorststraße 32<br>
  79331 Teningen<br>
  Germany<br>
  E-mail: <a href="mailto:openbikecomputer@proton.me">openbikecomputer@proton.me</a>
</address>

## 2. Overview

The start page, the documentation and the blog are static pages. They set no cookies. They
use no analytics services, tracking pixels, advertising, externally loaded fonts, social
media plugins or error trackers. When you open a page, the host only receives the
connection data it needs to deliver the page and to keep it secure.

The map builder at `/builder/` also loads map tiles and map data. It checks for new firmware
after you connect a device. It processes the files you select and the device data locally in
the browser. The route planner at `/plan/` sends search requests and routes to a server of
the controller. The verification console is for approved maintainers only. Sections 6 to 10
give the details.

## 3. Hosting on GitHub Pages

The website is delivered through **GitHub Pages** of GitHub, Inc., 88 Colin P. Kelly Jr.
Street, San Francisco, CA 94107, USA.

When you open a page, GitHub processes the access data that your browser transmits. This
includes the IP address, the time and target of the request, the HTTP status, the amount of
data transferred and the browser identifier (user agent). GitHub states that it logs the IP
addresses of visitors to GitHub Pages for security purposes.

The purpose is the technical delivery of the website and its security and stability. The
legal basis is Art. 6(1)(f) GDPR. The legitimate interest is a working website that is
protected against attacks. The website cannot be opened without transmitting the IP address.

The controller has no access to the GitHub access logs, receives no visitor statistics and
does not combine this data with other data. GitHub publishes no deletion period for the
GitHub Pages access logs. The period that applies is therefore the time that GitHub needs
them for the security purpose above.

## 4. Transfers to the USA

GitHub, Inc. and Cloudflare, Inc. are based in the USA. Both are certified under the
**EU-U.S. Data Privacy Framework**. For certified companies, the European Commission has
decided that the level of data protection is adequate. Where the data described in this
notice is transferred to the USA, the transfer rests on Art. 45(1) GDPR. You can check the
certifications in the
[Data Privacy Framework participant list](https://www.dataprivacyframework.gov/list). If
the adequacy decision ceases to apply, the Standard Contractual Clauses under Art. 46(2)(c)
GDPR in the data processing agreements of the two providers apply.

## 5. Encryption

The website is delivered only over TLS-encrypted connections (HTTPS).

## 6. Connections of the map builder

For the following requests, your IP address is transmitted to the server concerned. This is
technically necessary so that the server can send the requested data back to the browser.
The legal basis in each case is Art. 6(1)(f) GDPR.

### 6.1 Map tiles

The region picker, tour previews and recorded ride maps in the map builder and desktop
app load tiles from `tiles.openbikecomputer.com`. This is the Cloudflare Worker that also
serves the route planner (section 7.1). Cloudflare receives your IP address and the requested
tile. From this, Cloudflare can see the map area. The route data itself is not transmitted.
The map's fonts and symbols come from `maps.openbikecomputer.com` (section 6.3). Builds
without fixed planner settings also read the active release from the catalog on that host.
The tiles load when you open a map. The legitimate interest is to show the map needed to
select a region and view routes or recorded rides.

### 6.2 Check for new firmware

After you connect an OpenBikeComputer, the builder requests the current firmware description
once from `updates.openbikecomputer.com`. It is delivered through **Cloudflare R2** of
Cloudflare, Inc., 101 Townsend St., San Francisco, CA 94107, USA.

The request contains neither the serial number nor the installed firmware version. The
comparison takes place locally in the browser. Without a connected device, the file is not
requested. The legitimate interest is to point out available updates, in particular updates
that matter for security.

### 6.3 Map data through Cloudflare R2

The catalog, previews, cell directories and map cells are also delivered through Cloudflare
R2 from `maps.openbikecomputer.com`. The catalog loads when you open the map builder. The
builder requests further files according to your selection. From the requested cells,
Cloudflare can see the approximate map area that you selected.

The R2 bucket in use has **no EU jurisdiction restriction**. Cloudflare is therefore not
required to limit storage or processing to the EU. Section 4 applies to transfers to the
USA. The fonts, symbols and device catalog of the route planner also come from
`maps.openbikecomputer.com`.

The controller has **not enabled Logpush** for R2. The controller therefore does not export
or analyze R2 access logs. In R2 the controller uses only summarized operating metrics, such
as the number and size of requests. **Network Error Logging** is also disabled. The browser
sends no such error reports to Cloudflare. Regardless of this, Cloudflare can process
technical data to the extent that the delivery, the security and the operation of the
service require.

The legitimate interest is to provide map data that is updated independently and is too
large to include in the website.

## 7. Route planner

The route planner at `/plan/` is a separate application. It connects to three servers. The
legal basis is Art. 6(1)(f) GDPR. The legitimate interest is to provide the planner that you
requested. Section 7.2 applies to your location.

### 7.1 Tiles, search and route calculation

**Map tiles.** Map and terrain tiles come from `tiles.openbikecomputer.com`. This is a
Cloudflare Worker of Cloudflare, Inc. (section 4). It reads the tiles from Cloudflare R2.
Cloudflare receives your IP address and the requested tile. From this, Cloudflare can see
the map area. The Worker writes no access data. The controller does not analyze Cloudflare
access logs.

**Search and route calculation.** Search and route calculation run on a server of the
controller at `releases.openbikecomputer.com`. The server is hosted by **Contabo GmbH**,
Aschauer Straße 32a, 81549 Munich, Germany, as a processor under Art. 28 GDPR. No transfer
to a third country takes place. The server calls no external search or routing services.

The server receives:

- for a search: the text you enter, the visible map area, the start date and the planned
  route with waypoints, daily stages and labels. If you ask for it, your location under
  section 7.2 is added.
- for a route calculation: the waypoints, the profile and the visible map area for the map
  layers.
- with every request: your IP address.

The application writes no requests to a log and stores nothing. The web server Caddy keeps
no access log. If a service behind Caddy does not respond, for example during an update,
Caddy writes only the time and the error message to the system log. It writes no IP address
and no requested address. The system log deletes its entries after seven days.

### 7.2 Location

The location function of your browser runs only when you press the location button and the
browser asks you. This is your consent under Art. 6(1)(a) GDPR and § 25(1) TDDDG. The
location is then sent with every search to the server in section 7.1, to sort results near
you. It is not stored. You can withdraw your consent at any time with effect for the future
in the location permission settings of your browser.

## 8. Verification console

The verification console at `releases.openbikecomputer.com` serves the maintainers of the
project. You can use it only after you sign in. Visitors without access see only the sign-in
page.

- **Sign-in with GitHub.** The sign-in redirects to GitHub (section 4). The console requests
  only the public identity. It stores the numeric GitHub ID, the user name and the
  administrator role. It stores no e-mail address and no access token. A local administrator
  account is available as an alternative.
- **Cookies.** The session cookie `obc_session` (12 hours) and the cookie `obc_oauth`
  (10 minutes, only during sign-in) are necessary for the sign-in (§ 25(2) no. 2 TDDDG).
- **Sign-in attempts.** To limit repeated sign-in attempts, the console stores a key value of
  the IP address for 15 minutes. The key value cannot be reversed.
- **Content.** The user name appears as author or decider on requirements, test plans and
  proposals. It stays stored together with these records. After an account is removed, the
  name can be removed from the records on request.
- **Backup.** A daily backup of the database is kept for seven days.

The legal basis is Art. 6(1)(f) GDPR. The legitimate interest is a traceable, secured
approval process. The server is hosted by Contabo GmbH (section 7.1). The system log is
deleted after seven days, as described there.

## 9. Storage on your device

The light/dark switch of the website and of the map builder stores your chosen appearance
locally in `localStorage` under `obc-theme`. Without a choice, your browser setting applies.
The value is not sent to a server. It stays stored until you delete the website data in your
browser.

The web application and the installed desktop app use local device storage. The map builder
uses `localStorage` and the browser's *Origin Private File System* (OPFS). This data stays on
the device that you use. It is not transmitted to the controller.

| Data | Purpose and duration |
| --- | --- |
| Current map and scheme configuration | Automatic local working state, so that an edit survives a reload. Kept until it is replaced or you delete the website data. |
| Skins you save yourself, with names and colors | Only after you press "Save custom skin". Kept until you delete them with the delete function or delete the website data. |
| Map cells and working files in OPFS | Needed for large maps during download and assembly. By default the cells are deleted after the run. Sort files are removed at the end of the run. Older output files are removed at the start of the next run at the latest, or with the delete function. |
| Map cells kept for reuse (optional) | Only if you enable "Keep downloaded map cells for future builds". Kept until you disable the option, until you choose "Delete stored map data", until you delete browser data, or until a new catalog generation replaces the old one. |
| Your choice to reuse map cells | So that the setting you chose is kept at a later visit. Kept until the next change or until you delete the website data. |
| Answered firmware notices, with device serial number and offered version | Only after you close or open a notice, so that the same question does not appear again for the same device. At most the 32 most recent answers. Kept until you delete the website data. |
| Current trip plan of the route planner (`obc-planner-routing-v2`) | Automatically saved draft with waypoints and stages, so that it survives a reload. Kept until it is replaced or you delete the website data. It leaves the browser only as a search request under section 7.1. |
| Saved plan versions of the route planner (`obc-planner-lab-versions-v1`) | Only when you save one. Kept until you delete it in the version list or delete the website data. |
| Chosen bike profile for routes (`obcm.routeBikeType`) | So that the setting you chose is kept at a later visit. Kept until the next change or until you delete the website data. |
| Route and tour previews in the web application | Thinned-out coordinates loaded from the connected device, to draw the tiles. Held only in the memory of the running page session, until you reload or close it. |
| Route and tour previews in the installed desktop app | Thinned-out coordinates are cached locally, so that they need not be loaded again from the connected device after an app restart. The cache holds at most 300 entries and removes older entries that were not used recently. Changed objects get a new cache key from content marks. You can delete the cache on the device page with "Delete saved previews" or by deleting the app data. |

Saving and reading the working data for a short time, saving the trip plan, saved plan
versions and chosen settings, saving a skin or an answered firmware notice that you
explicitly saved, and the limited preview cache of the explicitly installed desktop app are
necessary for the function that you requested (§ 25(2) no. 2 TDDDG). The preview cache in
particular avoids repeated full transfers of all displayed routes and tours over USB. These
processes need no consent.

Reusing downloaded map cells in later builds is optional and off by default. When you enable
the clearly labeled option, this is your consent under § 25(1) TDDDG. You can withdraw it at
any time with effect for the future by disabling the option or with "Delete stored map data".
The lawfulness of the earlier storage remains unaffected.

## 10. Local processing of files and device data

The device demo, the conversion of selected route files and the assembly of the map run
locally in the browser. Files that you select or drag into the window are not transmitted to
a server. This applies in particular to route files, which can contain position data.

A WebUSB connection exists only after you confirm a device in the browser's selection
dialog. The data flows directly between the browser and the device. When a map shows tours
or rides, the browser and the desktop app load only map tiles under section 6.1. The route
data does not leave the device. For the preview images of the device overview, simplified
route lines are held in memory, but **not stored permanently in the browser**. They are
discarded when you reload or when the browser session ends.

## 11. External links

Links to the source code repository, to OpenStreetMap and to technical references are opened
only after a click. There is no prefetch. After you click, the privacy policy of the
provider concerned applies. Contributions to GitHub issues or pull requests can be public,
according to the settings there.

## 12. Contact by e-mail

When you send a message to `openbikecomputer@proton.me`, the sender address, the name, the
content and any details that you give voluntarily are processed to handle your request. The
legal basis is Art. 6(1)(f) GDPR. The legitimate interest is to answer requests. If a
request aims at the preparation of a contract, Art. 6(1)(b) GDPR also applies.

The mailbox is operated through **Proton Mail** of Proton AG, Route de la Galaise 32, 1228
Plan-les-Ouates, Geneva, Switzerland. For Switzerland, an adequacy decision of the European
Commission exists under Art. 45 GDPR. The
[privacy policy of Proton](https://proton.me/legal/privacy) applies in addition.

Obvious spam is deleted immediately. Other messages are deleted six months after the matter
is closed at the latest. A longer storage period applies only if statutory retention duties
or the assertion, exercise or defense of legal claims require it.

## 13. Your rights

Where the legal requirements are met, you have the right of access (Art. 15 GDPR),
rectification (Art. 16 GDPR), erasure (Art. 17 GDPR), restriction of processing (Art. 18
GDPR) and data portability (Art. 20 GDPR).

Where processing rests on Art. 6(1)(f) GDPR, you have the right under Art. 21 GDPR to object
on grounds relating to your particular situation. An informal message to the address in
section 1 is enough.

You can withdraw a consent that you gave at any time under Art. 7(3) GDPR with effect for
the future. The lawfulness of the earlier processing remains unaffected.

Independently of this, you have the right under Art. 77 GDPR to lodge a complaint with a
data protection supervisory authority, in particular at your place of residence, your place
of work or the place of the alleged infringement. The authority responsible for the
controller is the Landesbeauftragte für den Datenschutz und die Informationsfreiheit
Baden-Württemberg, Lautenschlagerstraße 20, 70173 Stuttgart, Germany.

## 14. Automated decisions

No automated decision-making, including profiling, under Art. 22 GDPR takes place.

## 15. Changes

This notice is updated when the website, its providers or the processing described here
change.

---

*Version: 1 October 2026*
