#!/usr/bin/env python3
"""Launch the embedded Linux frontend and select a local catalog region."""

from contextlib import suppress
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
import traceback
from urllib.parse import urlsplit

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.remote.client_config import ClientConfig
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.webkitgtk.options import Options

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools"))

from fixture_catalog import CatalogServer, schema_examples  # noqa: E402 — needs ROOT on the path


def main():
    if sys.platform != "linux":
        raise SystemExit("The release-launch suite requires Linux and Xvfb.")
    binary = Path(os.environ.get("OBC_DESKTOP_BINARY", ROOT / "apps/obc-desktop/target/release/obc-desktop")).resolve()
    evidence = Path(os.environ.get("OBC_DESKTOP_EVIDENCE", ROOT / "target/desktop-launch")).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    for name in ("result.json", "failure.png", "ready.png", "page.html", "driver.log", "application.log", "catalog.jsonl", "processes.txt"):
        (evidence / name).unlink(missing_ok=True)
    result = {"passed": False, "binary": str(binary)}
    browser = process = server = None
    objects = schema_examples()

    with TemporaryDirectory(prefix="obc-desktop-launch-") as scratch, (evidence / "driver.log").open("w") as driver_log:
        try:
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise RuntimeError(f"Missing executable release app: {binary}")
            server = CatalogServer(objects, log=evidence / "catalog.jsonl").start()
            catalog_url = f"{server.origin}/catalog.json"
            # WebKit launches this wrapper once. exec preserves its PID and captures Rust output.
            wrapper = Path(scratch) / "application.sh"
            pidfile = Path(scratch) / "application.pid"
            wrapper.write_text("#!/bin/sh\n" + f"echo $$ > {shlex.quote(str(pidfile))}\n" +
                               f"exec > {shlex.quote(str(evidence / 'application.log'))} 2>&1\n" +
                               'printf "automation=%s inspector=%s display=%s\\n" "$TAURI_WEBVIEW_AUTOMATION" "$WEBKIT_INSPECTOR_SERVER" "$DISPLAY"\n' +
                               f'exec {shlex.quote(str(binary))} "$@"\n')
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
            origin = urlsplit(result["url"])
            if (origin.scheme, origin.netloc) != ("tauri", "localhost"):
                raise AssertionError(f"Expected embedded custom-protocol frontend, got {result['url']}")
            search.send_keys("Switzerland")
            wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, '[aria-label="Add Switzerland (1.0 KB)"]'))).click()
            wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, '[aria-label="Switzerland is already in the map"]')))
            wait.until(EC.text_to_be_present_in_element((By.CSS_SELECTOR, '.ledger .total'), "1.0 KB"))
            if browser.find_elements(By.CSS_SELECTOR, '.catalog-error, .ledger .error, .parts .retry'):
                raise AssertionError("Catalog or region resolution failed")
            missing = set(objects) - set(server.requests)
            if missing:
                raise AssertionError(f"Native catalog requests missing: {sorted(missing)}")
            result["region"] = "Switzerland"
            result["price"] = browser.find_element(By.CSS_SELECTOR, '.ledger .total').text
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
            with suppress(Exception):
                # A session can fail before WebDriver can capture the still-open app window.
                subprocess.run(["import", "-window", "root", str(evidence / "failure.png")],
                               check=True, timeout=5)
                (evidence / "processes.txt").write_text(subprocess.check_output(
                    ["ps", "-eo", "pid,ppid,stat,comm"], text=True, timeout=5))
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
            result["requests"] = server.requests if server else []
            (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
