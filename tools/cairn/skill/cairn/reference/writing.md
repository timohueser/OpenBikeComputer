# Writing

Read this before you write a system doc, an issue, a findings list, or a message that explains code to the user.

**The reader** is a developer who is new to this project. They know Rust and embedded basics, but not our code and not our words. Write for them. Agents read plain text as well as people do.

Few rules, and examples. The rules below cover the failures we saw; the examples show them.

## A document

1. Say what a thing does in plain words, then give its code name in backticks, and only when the reader needs to find it.
2. Explain each project term the first time you use it, in a few words, in brackets or in the sentence. Give no private meaning to an everyday word.
3. Plain before short. Use no figures of speech, no slogans, and no "turns X into Y". A longer concrete sentence is better than a short abstract one.
4. Name the actor: "the presenter sends the rows", not "the rows are sent".
5. Break a long noun chain with a preposition or a verb.
6. State the fact, not its importance or a slogan about it.
7. Use one word for one meaning in the whole document.

| Rule | Bad | Good |
| --- | --- | --- |
| 3 | This system turns what the device must show into rows on the panel. | This system draws everything the device shows, the map and the screens. It sends only the changed parts to the display. |
| 3 | …so a frame on a computer is the frame on the panel. | All of them use the same drawing code. So a frame drawn in a test on a computer has exactly the pixels the device shows. |
| 2 | *glass*: What the panel shows now. | "the image on the panel" (no new word needed) |
| 1 | It also gives every screen its drawing words (`Surface`). | It also gives the screens a set of drawing functions (`Surface`): lines, shapes and text. |
| 2 | Each pixel is RGB565 until `FbDevice64` packs it to one device-64 byte. | The renderer draws in 16-bit color (RGB565). When a pixel is stored in the frame, it is reduced to the panel's 64 colors, one byte per pixel. |
| 1 | `collect` walks the visible chunks once. It culls each feature, projects its points to 16-bit screen coordinates and stores one `Span`. | The collector goes once through the parts of the map that are on screen. It skips features that are not visible, converts the points of the others to screen pixels, and keeps each one for drawing. |
| 5 | The row-hash self-diff. | Compares each row of the new frame with the last frame, using a hash of each row, to find the rows that changed. |
| 5 | The mutate-and-restore composite. | Draws an overlay into the frame, sends it, then puts the original pixels back. |
| 4 | A feature reaches the renderer as neutral metadata and an opaque token. | The map reader gives the renderer only what it needs to draw a feature, and a handle to fetch its points. |
| 6 | Damage is not a contract: each pairing owns its own strategy. | The interface does not say how a presenter finds the changed parts. The LS021 presenter compares a hash of each row. |
| 2 | A hold on a button adds a transient overlay, the hold bulge. | When the rider holds a button, a small shape appears at the edge of the screen until the hold ends. |
| 3 | An unchanged row costs no transfer. The panel holds its image with no scan traffic. | The device sends a row to the panel only when the row changed. Between updates, the panel keeps its image on its own. |
| 3 | `purpose:` Turns a map scene and screen drawing calls into the rows that the panel shows. | `purpose:` Draws the map and the screens, and updates the display panel. |

## A message to the user

1. Start with the answer or the result.
2. Explain a code term in a few words the first time you use it.
3. Use no process words (peanut, hay, leaf, Goldfish) unless the user used them.
4. Each finding says what happens to the user or the product, not only what the code does.

| Rule | Bad | Good |
| --- | --- | --- |
| 4 | `flpr_blob_carries_the_same_pin_map` checks the mask and the six shifts, not plane order or the odd/even interleave. | The test that compares the C and Rust code checks only which pins each color uses. It does not check the order of the pixel data, so a mistake there would reach the device untested. |
| 2 | Test the real C `pack_word` on the host against the golden words. | Test the C code on a computer against output we know is correct. |
| 3 | The `viewport.rs` citations from one peanut were off by a fixed 273. | One code summary gave wrong line numbers for `viewport.rs`. I checked every claim about that file myself. |
| 1 | Three items that need your decision. 1. The Rust wire pack does not run on the device. What I found: … | The draft is ready, and I have one question first: the device sends pixel data with C code that has no test. Should we test the C code, or keep the Rust copy? |
