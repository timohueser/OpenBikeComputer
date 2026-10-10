# Syntax highlighting assets

These files ship with the docs and blog. The site needs no CDN connection.

| File | Source | License |
| --- | --- | --- |
| `highlight.min.js` | [Highlight.js 11.11.1 common build](https://github.com/highlightjs/cdn-release/tree/11.11.1/build) | BSD-3-Clause; see `LICENSE` |
| `riscvasm.min.js` | [RISC-V grammar](https://github.com/highlightjs/highlightjs-riscvasm/tree/fba4769fd2547b1525a9655f82086108ac59a1a9/dist) | CC0-1.0; see `LICENSE-riscvasm` |

To update, copy the browser builds and license files from these sources. Keep the
version and source links in this table current. Do not edit the minified files.

The common build includes Rust, C, Bash, and TOML (`ini`). The RISC-V grammar adds
`riscv`. `../code-highlighting.js` uses the fence language and leaves unlabelled,
`text`, and unknown languages as plain text. Write `riscv` for new RISC-V fences;
`asm` also selects this grammar for the FLPR tutorial.
