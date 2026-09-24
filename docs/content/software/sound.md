---
title: Sound cues
description: The five sound families, when a cue plays, and the Sound settings.
copy: ai
---

# Sound cues

The device has one piezo buzzer. A cue tells the rider that something happened, so the rider does
not have to watch the screen. The app decides which cue plays. The platform decides what it sounds
like, from one [pattern table](src:firmware/obc-platform/src/sound.rs) that the board, the
simulator and the iPhone host share.

## Five families

A rider learns about five sounds reliably, so the cues are grouped by meaning and not given one
sound each:

| Family | Meaning | Cues |
| --- | --- | --- |
| Tick | Got it | key click, hold done |
| Heads-up | Look at the screen soon | climb starts |
| Good | Resolved | back on route, GPS back, arrived, the volume preview |
| Problem | Something went wrong | off route, GPS lost, sensor dropped, battery low |
| Urgent | Act now | recording error, battery critical |

Rhythm is the primary signal: the number of notes and their length. Rhythm stays audible in wind,
where a change of pitch is easy to miss. Pitch direction is the secondary signal: rising notes are
good news and falling notes are bad news. When two cues occur in the same pass, the more severe
family plays.

## When a cue plays

Some events are edges already: a climb starts, the rider arrives, a hold finishes a ride. Their cue
plays once. Other facts are levels, such as off route or a live GPS fix. A level plays its cue only
when the new level holds for a few seconds, and after a loss cue the same level is silent for one
minute. A recovery cue plays only when its loss cue played. So a rider on the edge of the route
hears one cue, not a stream of cues. The GPS and sensor cues play only while the ride runs. The
battery cues play once at each threshold. [`cues.rs`](src:firmware/obc-app/src/cues.rs) holds these
rules.

## Settings

The Sound page has two rows. **Sound** is Off, Quiet or Loud. Loud drives the piezo from two pins in
opposite phase, which is louder in wind. A change to Quiet or Loud plays a preview at the new level.
**Key tones** clicks on every button press, and it is off by default. The page is hidden on a
platform that has no buzzer.
