# Component models

Use millimetres. The models below have their XY origin at the footprint centre.
The PCB seating plane is Z = 0. Use unit scale and zero offset and rotation in
KiCad.

| Model | Source | Use |
| --- | --- | --- |
| `Laird_BMI-S-202-F-20_16.5mm.step` | [Laird assembly CAD](https://www.laird.com/media/4159) | Frame, pickup bridge and cover. |
| `MAX_F11N_provisional_envelope.step` | [u-blox MAX CAD](https://github.com/u-blox/3D-Step-Models-Library) | Nominal MAX-family appearance. The filename matches the provisional footprint. This is not a MAX-F11N-specific model. |
| `MAX_F11N_max_envelope.step` | [MAX-F11/M11 data sheet](https://content.u-blox.com/sites/default/files/documents/MAX-F11-M11-series_DataSheet_UBXDOC-304424225-21561.pdf) | Maximum-size clearance envelope. |
| `FH34SRJ-12S-0.5SH_drawing_model.step` | [Hirose dimensioned drawing](https://www.hirose.com/en/product/p/CL0580-1253-0-50) | Independent OBC model with a closed actuator. |

For GNSS clearance checks, select `MAX_F11N_max_envelope.step` in the footprint's
3D Models tab. The nominal family model does not include maximum dimensions.

Edit the connector source in Fusion: **OpenBikeComputer / Components /
Hirose FH34SRJ-12S-0.5SH**. The housing, actuator, twelve contacts and two
hold-downs are separate bodies with named sketch and feature history. Export
STEP in millimetres. Keep the footprint centre and seating plane unchanged.
The external dimensions and contact pitch follow the drawing. Undimensioned
spring, moulding and fillet details are visual approximations. The model does
not contain the restricted manufacturer CAD.

The Laird model excludes drawing surfaces. Its three solid bodies retain the
assembly geometry. The u-blox model retains its colours and body geometry.
The u-blox licence is in [LICENSE.ublox-3dmodels](../LICENSE.ublox-3dmodels).
