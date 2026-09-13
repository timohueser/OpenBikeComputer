#!/usr/bin/env python3
"""Launch the embedded Linux frontend and select a local catalog region."""

from contextlib import suppress
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Thread
import traceback

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.remote.client_config import ClientConfig
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.webkitgtk.options import Options

ROOT = Path(__file__).resolve().parents[3]


def catalog_fixture():
    """Keep the producer's fine-band example; make the other bands empty."""
    examples = ROOT / "host/obc-pack/schema"
    catalog = json.loads((examples / "catalog.example.json").read_text())
    fine = json.loads((examples / "cell-index.example.json").read_text())
    region_cells = json.loads((examples / "region-cells.example.json").read_text())
    objects = {}

    def pin(name, document):
        body = json.dumps(document).encode()
        digest = hashlib.sha256(body).hexdigest()
        path = f"/{name}.{digest}.json"
        objects[path] = body
        return {"url": path, "bytes": len(body), "sha256": digest}

    catalog.pop("terrain", None)
    catalog.pop("network_terrain_revision", None)
    for ref in catalog["cell_index"]:
        doc = fine if ref["band"] == "fine" else {
            "schema_version": 2, "schema_revision": catalog["schema"]["revision"],
            "band": ref["band"], "cells": [], "known_empty": [],
        }
        ref.update(pin(ref["band"], doc))
        ref.update(cell_count=len(doc["cells"]), known_empty_count=len(doc["known_empty"]))
    region_cells["cells"] = {"fine": region_cells["cells"]["fine"]}
    region_cells.pop("terrain", None)
    region = catalog["regions"][0]
    region.pop("terrain", None)
    region.update(bytes=sum(cell["bytes"] for cell in fine["cells"]),
                  bytes_by_band={"fine": 994}, cell_count={"fine": 3},
                  partial_cell_count_by_band={"fine": 0})
    pinned = pin("region", region_cells)
    region.update({f"cells_{key}": value for key, value in pinned.items()})
    catalog["regions"] = [region]
    objects["/catalog.json"] = json.dumps(catalog).encode()
    return objects


def main():
    if sys.platform != "linux":
        raise SystemExit("The release-launch suite requires Linux and Xvfb.")
    binary = Path(os.environ.get("OBC_DESKTOP_BINARY", ROOT / "apps/obc-desktop/target/release/obc-desktop")).resolve()
    evidence = Path(os.environ.get("OBC_DESKTOP_EVIDENCE", ROOT / "target/desktop-launch")).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    for name in ("result.json", "failure.png", "ready.png", "page.html", "driver.log", "application.log", "catalog.jsonl"):
        (evidence / name).unlink(missing_ok=True)
    result = {"passed": False, "binary": str(binary)}
    browser = process = server = None
    requests = []
    objects = catalog_fixture()

    class CatalogHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            status = 200 if self.path in objects else 404
            requests.append(self.path)
            with (evidence / "catalog.jsonl").open("a") as log:
                log.write(json.dumps({"path": self.path, "status": status}) + "\n")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(objects.get(self.path, b"not found"))

        def log_message(self, *_args):
            pass

    with TemporaryDirectory(prefix="obc-desktop-launch-") as scratch, (evidence / "driver.log").open("w") as driver_log:
        try:
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise RuntimeError(f"Missing executable release app: {binary}")
            server = ThreadingHTTPServer(("127.0.0.1", 0), CatalogHandler)
            Thread(target=server.serve_forever, daemon=True).start()
            catalog_url = f"http://127.0.0.1:{server.server_port}/catalog.json"
            # WebKit launches this wrapper once. exec preserves its PID and captures Rust output.
            wrapper = Path(scratch) / "application.sh"
            pidfile = Path(scratch) / "application.pid"
            wrapper.write_text("#!/bin/sh\n" + f"echo $$ > {shlex.quote(str(pidfile))}\n" +
                               f"exec {shlex.quote(str(binary))} > {shlex.quote(str(evidence / 'application.log'))} 2>&1\n")
            wrapper.chmod(0o755)
            environment = {**os.environ, "OBC_CATALOG_URL": catalog_url, "RUST_BACKTRACE": "1",
                           "XDG_DATA_HOME": scratch, "XDG_CONFIG_HOME": scratch, "XDG_CACHE_HOME": scratch}
            process = subprocess.Popen(["tauri-driver"], env=environment, stdout=driver_log,
                                       stderr=subprocess.STDOUT, start_new_session=True)

            def listening(_):
                if process.poll() is not None:
                    raise RuntimeError("tauri-driver exited before readiness")
                try:
                    with socket.create_connection(("127.0.0.1", 4444), timeout=1):
                        return True
                except OSError:
                    return False

            WebDriverWait(None, 15).until(listening, "tauri-driver did not listen")
            options = Options()
            options.set_capability("browserName", "wry")
            options.set_capability("tauri:options", {"application": str(wrapper)})
            browser = webdriver.Remote(command_executor="http://127.0.0.1:4444", options=options,
                                       client_config=ClientConfig(remote_server_addr="http://127.0.0.1:4444", timeout=30))
            wait = WebDriverWait(browser, 30)
            search = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, '[aria-label="Search regions"]')))
            result["url"] = browser.current_url
            if not result["url"].startswith("tauri://localhost/"):
                raise AssertionError(f"Expected embedded custom-protocol frontend, got {result['url']}")
            search.send_keys("Switzerland")
            wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, '[aria-label="Add Switzerland (994 B)"]'))).click()
            wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, '[aria-label="Switzerland is already in the map"]')))
            wait.until(EC.text_to_be_present_in_element((By.CSS_SELECTOR, '.parts .price'), "994 B"))
            if browser.find_elements(By.CSS_SELECTOR, '.catalog-error, .ledger .error, .parts .retry'):
                raise AssertionError("Catalog or region resolution failed")
            missing = set(objects) - set(requests)
            if missing:
                raise AssertionError(f"Native catalog requests missing: {sorted(missing)}")
            result["region"] = "Switzerland"
            result["price"] = browser.find_element(By.CSS_SELECTOR, '.parts .price').text
            result["capabilities"] = browser.capabilities
            browser.save_screenshot(str(evidence / "ready.png"))
            (evidence / "page.html").write_text(browser.page_source)
            browser.quit()
            browser = None
            pid = int(pidfile.read_text())
            WebDriverWait(None, 10).until(lambda _: not Path(f"/proc/{pid}").exists(), "Application did not exit after session quit")
            result["application_exited"] = True
            if "panicked at" in (evidence / "application.log").read_text():
                raise AssertionError("The release app panicked")
            result["passed"] = True
        except Exception:
            result["error"] = traceback.format_exc()
            if browser:
                with suppress(Exception):
                    browser.save_screenshot(str(evidence / "failure.png"))
                    (evidence / "page.html").write_text(browser.page_source)
            raise
        finally:
            if browser:
                with suppress(Exception):
                    browser.quit()
            if process:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            if server:
                server.shutdown()
                server.server_close()
            result["requests"] = requests
            (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
