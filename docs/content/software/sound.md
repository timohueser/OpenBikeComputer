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

A rider can learn about five sounds reliably. So the app groups the cues by meaning, and does not
give each cue its own sound:

| Family | Meaning | Cues |
| --- | --- | --- |
| Tick | Got it | key click, hold done |
| Heads-up | Look at the screen soon | climb starts |
| Good | Resolved | back on route, GPS back, arrived, the volume preview |
| Problem | Something went wrong | off route, GPS lost, sensor dropped, battery low |
| Urgent | Act now | recording error, storage lost, battery critical |

Rhythm is the primary signal: the number of notes and their length. Rhythm stays audible in wind,
where a change of pitch is easy to miss. Pitch direction is the secondary signal: rising notes are
good news and falling notes are bad news. When two cues occur in the same pass, the more severe
family plays.

## When a cue plays

Some events are edges already: a climb starts, the rider arrives, a hold finishes a ride. Their cue
plays once. Other facts are levels, such as off route or a live GPS fix. A level plays its cue only
when the new level holds for a few seconds, and after a loss cue the same level is silent for one
minute. A loss that still holds when that minute ends plays its cue then. A recovery cue plays
only when its loss cue played. So a rider on the edge of the route hears one cue, not a stream of
cues. The GPS and sensor cues play only while the ride runs, and GPS lost plays only when the GPS
had a fix after the ride started. The battery cues play once at each threshold. A recording error
and storage lost play each time the fault occurs, but at most once a minute. An incomplete ride log
that the device finds at start-up shows the warning card and plays no cue, because the rider cannot
act on it.
[`cues.rs`](src:firmware/obc-app/src/cues.rs) holds these rules.

## Settings

The Sound page has two rows. **Sound** is Off, Quiet or Loud. Loud drives the piezo from two pins in
opposite phase, which is louder in wind. A change to Quiet or Loud plays a preview at the new level.
**Key tones** clicks on every button press, and it is off by default. A platform that has no buzzer
does not show the page.
