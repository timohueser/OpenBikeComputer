# Device test setup

Use the nRF54LM20A board image with `debug-uart`. The build and verified flash commands are in
[the board README](../../../../../firmware/obc-fw-nrf54l/README.md#build--flash).
Keep J4 connected for debugging and the virtual serial port. J3 is needed only for map transfer.
The loaded card must keep the Swiss map and original Meiringen route; do not format it for a retest.

Send these debug-sensor lines once per second over the J4 virtual serial port at 115200 baud,
with hardware flow control off:

```text
F 46723126 8194551 - -
C 225
A 602
```

Select the original Meiringen route, not an accepted Visit copy. Continue the existing recording
when the recovery screen offers it. Allow normal GPS updates to settle at progress 1,664 m.
The stationary simulator input and original route are in the [simulator evidence](simulator/README.md).
This indoor build uses the supplied fixed location; it is not a test of satellite reception.

1. Hold Up+Select. Open Find a place, then Water.
2. Time Select to the completed choices. The pill must stay present throughout preparation.
3. Check that the compass turns once per second and stays clear of the rider marker.
4. Inspect all offered choices. Each must show measured route costs.
5. Open the first Water preview. It must show the short visit, the rider, and the destination marker.
6. Go Back and open the same preview again. It must reuse the stored route without new planning.
7. Return to categories. Back resets selection to Water: Campsite is index 1, Lodging 2,
   Resupply 3, Pharmacy 4, Bike shop 5, and Train 6. Use absolute indices for scripted tests.
8. Check Train to exercise a longer complete search. There is no time cutoff.

The direct screen measurements use device RTT timestamps, from `Press on FindPlace` to the first
map frame after `find: ready`. Keep the exact flashed ELF with each trace so decoded RTT and firmware
identity agree. Repaint timing includes query work in the first frame; do not label it pure rendering.

On-road detour guidance, moving-GPS behavior, power use, and broad hardware acceptance remain pending.
Do not infer that acceptance from this stationary search and preview test.
