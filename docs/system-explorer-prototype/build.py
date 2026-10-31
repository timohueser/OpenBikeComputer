"""Assemble explorer.html from template.html, explorer.css, the site fonts and map.json."""
import base64, json, pathlib
here = pathlib.Path(__file__).resolve().parent
fonts = here.parent / "assets/fonts"
b64 = lambda f: base64.b64encode((fonts / f).read_bytes()).decode()
css = (here / "explorer.css").read_text()
css = css.replace("__SANS__", b64("atkinson-hyperlegible-next-latin.woff2")).replace("__MONO__", b64("atkinson-hyperlegible-mono-latin.woff2"))
data = json.dumps(json.loads((here / "map.json").read_text()), indent=1, ensure_ascii=False).replace("</", "<\\/")
html = (here / "template.html").read_text().replace("__CSS__", css.rstrip()).replace("__MAP_JSON__", data)
(here / "explorer.html").write_text(html)
print(here / "explorer.html", len(html.encode()), "bytes")
