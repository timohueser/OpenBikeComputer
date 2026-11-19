# Brief: index the local hiking loops of one tourism office

You are an indexing agent in a small research trial. You have web access (WebSearch, WebFetch, and
`curl` in Bash). Your job: for ONE place, find the day-hike loops that the LOCAL tourism office or
the municipality recommends on its OWN website, and record them as structured data.

The kind of route we want: the loops on a trailhead board. Start = finish, about 3 to 25 km, on
foot. Not long-distance trails, not multi-day stages, not bike routes.

## Hard rules

1. **Own sites only.** A source counts only if the local office, the municipality or a local club
   (for example the local hiking club) publishes it on its own domain, or in its own PDF brochure.
2. **Never take route data from a platform.** Platforms include: outdooractive, komoot, alltrails,
   wikiloc, visorando, ich-geh-wandern, bergfex, gps-tour.info, cirkwi, ignrando, sitytrail,
   alpenvereinaktiv, tourenplaner/toubiz databases of large regional tourism bodies (for example
   schwarzwald-tourismus.info), and anything similar. You may use WebSearch to FIND the office
   site, but do not open platform pages to read routes. If an office page shows its routes through
   an embedded platform widget, an iframe, or platform links for map/GPX, record that fact (field
   `geometry.hosting = "platform"`, name the platform) and record only what the office's own page
   text says.
   To detect a platform, look at the raw HTML with curl (script/iframe sources, link targets, image
   hosts, "powered by" notes), not only at the rendered text.
3. **Skip the big regional tourism body** of the wider region. We want the small local level.
4. **Be polite.** Before the first request to a host, read its `/robots.txt` and obey it. Send the
   User-Agent `route-index-trial/0.1`. At most one request per second per host. If a site says no,
   stop and record it.
5. **Do not invent.** If a value is not on the page, the field is `null`. Copy numbers as stated.

## What to record

Write ONE JSON file at the output path you are given. Shape:

```json
{
  "place": "...",
  "sites": [{"host": "...", "publisher": "...", "role": "office|municipality|club",
             "robots_txt": "allows|disallows|none", "notes": "..."}],
  "routes": [{
    "name": "...",
    "source_url": "page or PDF where the office describes this route",
    "publisher_host": "...",
    "loop": true,
    "length_km": 0.0, "ascent_m": 0, "duration_h": 0.0,
    "start": "start place as written",
    "marking": "waymark / symbol / route number as written, or null",
    "geometry": {
      "hosting": "own_gpx|own_kml|own_pdf_map|own_map_image|platform|none",
      "url": "URL of the self-hosted GPX/KML/PDF/image, or null",
      "local_file": "path of your downloaded copy, or null",
      "platform": "platform name, or null"
    },
    "description_text": "the office's route description in words, in the original language, complete, or null",
    "waypoints": [{"name": "place, landmark, signpost or farm as written", "cue": "what the text says happens there"}]
  }],
  "skipped": [{"what": "...", "why": "..."}],
  "stats": {"pages_fetched": 0, "hosts_contacted": ["..."]}
}
```

- `waypoints` is the ordered list of every named place, landmark, signpost, hut, farm, chapel,
  summit, stream or street that the description passes, from start to finish, start and finish
  included. Take them only from `description_text`. If there is no description, it is `[]`.
- Download each self-hosted GPX, KML, PDF brochure or route map image into the `files` directory
  you are given, and put the path in `local_file`. A PDF brochure counts as a description source:
  read it (`pdftotext` if it is installed, else the Read tool) and extract the routes from it.
- Record every qualifying loop you find, up to 15. If there are more, record the 15 with the
  richest descriptions and list the rest by name in `skipped`.

## Report

When the JSON file is written, reply with at most 300 words: which sites you used, how many routes
with each `geometry.hosting` value, how many have a description, what blocked you, and anything
that surprised you. Do not paste the JSON.
