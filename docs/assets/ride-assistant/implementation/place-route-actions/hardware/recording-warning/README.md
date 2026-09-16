# Separate recording-start warning

Tracked in [#1810](https://github.com/timohueser/OpenBikeComputer/issues/1810). Source `d55493fed`; see the parent [identity record](../identity.json). The [trace](discard-and-restart.log) records discard of test object 293 and a new Start input at 431.696881 s. The [resident-framebuffer capture](warning.png) shows Recording error / Log incomplete. A later trace records removal of the new test object 331, so the warning does not by itself prove that recording failed. Sample completeness and the cause remain unverified.
