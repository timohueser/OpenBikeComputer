---
title: Ride replay
description: Replay recorded rides and ridden trip days on the phone.
copy: ai
---

# Ride replay

**Replay ride** opens a full-screen view from a recorded ride. **Replay ridden days** opens the
same view from a trip journal. The trip replay includes recorded days only. Gaps and transfers
do not add distance or appear as ridden lines.

The replay starts paused. Play follows the rider across the terrain. Drag or pinch the map to
change the camera. The camera keeps that angle relative to the direction of travel as the rider
moves. **Reset camera** restores automatic framing.

**Overview** shows the complete ride while the rider marker continues to move. **Follow rider**
returns to the previous follow view. Drag directly on the elevation profile to change position.
This pauses playback. Play continues from that position. The profile also supports VoiceOver
adjustment. Missing elevation stays unavailable.

Photos already attached to a ride can appear at their matched positions. A photo briefly pauses
playback. Continue resumes it. The photo markers also open these moments. Replay uses available
local thumbnails and does not request photo access.

The phone renders the replay with a bundled renderer. It needs an internet connection for terrain
and imagery. Ride and photo files stay on the phone. Map tile requests reveal the viewed area to
the data providers. Their credits remain visible on the map.

Replay is in development. Provider access for public distribution is a separate step. Video export
is not part of the player.

The [native player](src:companion-ios/Packages/OBCKit/Sources/OBCUI/Replay/ReplayPlayerView.swift)
and [camera engine](src:companion-ios/Packages/OBCKit/Sources/OBCUI/Resources/Replay/track.mjs)
own the implementation.
