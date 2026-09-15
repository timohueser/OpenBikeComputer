# Public v16 catalog verification

Verified on 2026-09-15 at 10:56 UTC against acceptance commit `fb48e40c`.
The [public catalog root](https://maps.openbikecomputer.com/cell-catalog/catalog.json) is 65,304 bytes
and declares `schema.obcm_version = 16`. Its SHA-256 is
`8a63285a9af3e9baac5e9c8272e69365378435d6e4326654857bbc33814a8c7f`.
The downloaded root is byte-identical to the retained `catalog-v16-local/catalog.json`.

The retained publisher log reports 501 verified content objects before publishing the root last:
215 cells, one region, two skins, 502 total objects and 885,920,706 bytes. The publisher completed
in 6 minutes 55 seconds. The log SHA-256 is
`6f297fd2ff4121c0eb7ea3ef16eb9cbe491f34074f292bd650ecc4904393c517`.
This audit did not rebake cells, repeat publication, or publish source archives.

## Normal public download

From the repository root, with a new output directory:

```sh
python3 host/obcm-assemble/dev/fetch_region.py \
  europe/germany/baden-wuerttemberg .artifacts/catalog-v16/coarse --band coarse
```

The existing fetcher downloaded all nine coarse cells, with zero previously cached cells, for
2,142,032 bytes. It followed the public catalog, region selection and content-addressed band
index. Every cell matched its declared SHA-256. A subsequent local check also matched every
byte count and confirmed `OBCM` magic and version byte 16 for all nine files. The coarse index's
own byte count and hash passed against the root. The complete IDs, immutable public URLs, byte
counts and hashes are in [the verification record](catalog-v16-publication.json).

The normal `cells.json`, `schema.json` and `skin.json` sidecars were produced. This is a public
catalog and coarse-cell delivery check. It is not a new full map assembly, terrain download,
network-band semantic test, simulator run, snapshot sweep, or shipping image build.

## CI follow-up

After the public checks passed, only the failed `obcm-version-guard` job for
[PR #1757](https://github.com/timohueser/OpenBikeComputer/pull/1757) was requested again. Its original
job was `104344192241` in run `34957666039`, on head `1adddfe9`. The GitHub job rerun endpoint
accepted that request as [job 104353015526](https://github.com/timohueser/OpenBikeComputer/actions/runs/34957666039/job/104353015526).
No complete workflow or other PR's checks were restarted.
