# RA08 — Compile deterministic landmark text and photos from pinned sources

Parent: #1734. Depends on RA01. This is host-only source preparation. RA09 owns the device bytes;
RA10 owns the UI. No runtime Wiki queries, AI selection, per-place manual rewrite or new service.

Implementation dependencies: [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736).

## Source and category policy

Promote `docs/assets/ride-assistant/landmark-selection/selection.json` to one host-owned policy.
Reuse `obc-pack` ingest and `obc-bake` source/coverage orchestration. Match valid Earth coordinates,
a supported Wikipedia article, and P31/P279 closure from a pinned Wikidata snapshot. Excluded roots
win; deduplicate by QID; fix category precedence for multiply typed entities. Keep the reviewed
curiosities, historical/archaeological sites, monasteries/abbeys, cathedrals and passes. Exclude
lakes, mountains, glaciers, industrial types and the reviewed physical-state roots. No popularity
score, AI classifier, P576 demolition inference or individual exception list.

Select by actual map/polygon coverage, including border items and records without P17. The study's
P17 Switzerland count is not a geographic extractor. Keep display coordinates separate from mapped
approach points. Capture explicit OSM QID/article/entrance/access joins and hours; do not infer an
article from nearest place, owner, name similarity or surrounding region. Missing approach allows
information-only content. Source coverage and omissions must be reported, not quietly treated as no
landmarks in an area. Hand off normalized schedules with exact source identity to RA09, including
hours on sites that are not service POIs; do not carry file-local hours indexes across build stages.

## Text and language

Capture the chosen article's rendered lead at an exact revision, including source response and
license notices. Parse structural markup/references/hatnotes/infobox and pronunciation material by
fixed rules, normalize whitespace/punctuation, then take the first one or two complete sentences in
order that fit at most four device text pages. Each page uses the existing readable label/body
metrics; never shrink the font, split UTF-8 bytes or truncate a sentence to fit. If the first usable
complete sentence cannot fit the budget, report unavailable text; do not hunt for a nicer anecdote.

Choose requested pack language, then English, then the fixed fallback order de/fr/it/ga. Record and
show the actual language in details/Sources. No automatic translation or assertion that fallback
text is English. Respect supported ASCII/Latin-1/Latin Extended-A glyphs; normalize a fixed set of
punctuation. Unsupported letters require an explicit deterministic transliteration rule or omission.
Preserve names and credits independently of the prose budget. Test abbreviations, decimals,
parentheses and line wrapping. The hand-edited demo paragraphs are not production extraction.

## Images and attribution

Try P18, then chosen-article lead image, in deterministic normalized filename order. Initially
support Commons JPEG/PNG only, with bounded byte/dimension decode on host. Apply orientation, fit
inside 216 × 240 without subject crop, pad white, and use the reviewed ordered-dither RGB222 recipe.
No aesthetic ranking. A source hotel or ski-lift photo remains eligible. Missing, corrupt,
oversized, unsupported or unlicensed images produce working text-only entries with an omission
reason. Separate acquisition failure from genuine absence; pin successful and failed outcomes in a
snapshot.

Keep full article URL/revision/language, contributor attribution, exact license/version/URL and
modification notices. For each photo keep original creator, title/source, selected license and
required supplied attribution/copyright notices, plus resize/dither notice. Accept CC0 and a fixed
supported whitelist of CC BY/CC BY-SA versions after the license terms can be represented; choose
one license deterministically for dual-licensed files. Omit fair-use/local-Wikipedia, GFDL-only or
ambiguous assets. Never give every image the article's license. Distribute an attribution manifest;
Sources must be able to show the full applicable metadata.

Apply an attribution representation gate separately to each article and photo. Retain supplied
imported article notices as well as all required creator/copyright/credit information in both the
offline record and manifest. Initial bound: 8 KiB of required metadata per asset, and at most 256
readable Sources pages for the selected article/photo pair. Stream these pages; the bound is not
resident memory. Preserve exact originals. Display normalization is limited to safe typography and
percent-encoded URLs; it must not alter attribution identity or meaning. If required credits use
unsupported glyphs, exceed either bound, or cannot be represented faithfully, omit that asset and
report why. Do not add a font or generic transliteration project, silently shorten notices, or
substitute question marks. A rejected photo leaves an otherwise valid article usable. This is
implementation policy, not a blanket legal-compliance claim for arbitrary upstream assets.

Primary references: [Wikimedia Terms
§7](https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use#7._Licensing_of_Content), [Commons
reuse](https://commons.wikimedia.org/wiki/Commons:Reusing_content_outside_Wikimedia), and [CC BY-SA
4.0 §3](https://creativecommons.org/licenses/by-sa/4.0/legalcode.en).

## Output and acceptance

Produce one documented internal content manifest with source/policy hashes and bounded binary assets
for RA09. Wiki snapshot, category rules, language/text policy and image recipe must be cache inputs
even when OSM bytes do not change. Rebuild twice offline from captured bytes and compare outputs.
Test real Swiss and Irish texts, absent images, language fallback, source mismatch, colocated QIDs,
boundary records and a real unsupported-creator/long-notice asset. Recount Switzerland through this
production selection and report separate counts for candidate/text/image/approach plus exact storage
inputs. Use host whole suites and explicit captured-source fixture suites. Do not add a country
downloader daemon, translator, generic content management system or hand-maintained curated demo
list.
