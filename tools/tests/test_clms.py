"""The CLMS capture signs its grant as RS256 does, orders a prepackaged file and takes its member
from the archive, without the token outside the API."""

import base64
import http.server
import io
import json
import tempfile
import threading
import unittest
import zipfile
from pathlib import Path
from unittest import mock
from urllib.parse import parse_qs
from urllib.request import Request

from tools import clms


def number(text):
    return int.from_bytes(base64.urlsafe_b64decode(text + "=" * (-len(text) % 4)), "big")


# The RSA key and the signature of RFC 7515, appendix A.2.
N = number("ofgWCuLjybRlzo0tZWJjNiuSfb4p4fAkd_wWJcyQoTbji9k0l8W26mPddxHmfHQp-Vaw-4qPCJrcS2mJPMEzP1Pt0Bm4d4QlL-yRT-SFd2lZS-pCgNMs"
           "D1W_YpRPEwOWvG6b32690r2jZ47soMZo9wGzjb_7OMg0LOL-bSf63kpaSHSXndS5z5rexMdbBYUsLA9e-KXBdQOS-UTo7WTBEMa2R2CapHg665xsmtdV"
           "MTBQY4uDZlxvb3qCo5ZwKh9kG4LT6_I5IhlJH7aGhyxXFvUK-DWNmoudF8NAco9_h9iaGNj8q2ethFkMLs91kzk2PAcDTW9gb54h4FRWyuXpoQ")
D = number("Eq5xpGnNCivDflJsRQBXHx1hdR1k6Ulwe2JZD50LpXyWPEAeP88vLNO97IjlA7_GQ5sLKMgvfTeXZx9SE-7YwVol2NXOoAJe46sui395IW_GO-pWJ1O0"
           "BkTGoVEn2bKVRUCgu-GjBVaYLU6f3l9kJfFNS3E0QbVdxzubSu3Mkqzjkn439X0M_V51gfpRLI9JYanrC4D4qAdGcopV_0ZHHzQlBjudU2QvXt4ehNYT"
           "CBr6XCLQUShb1juUO1ZdiYoFaFQT5Tw8bGUl_x_jTj3ccPDVZFD9pIuhLhBOneufuBiB4cS98l2SR_RQyGWSeWjnczT0QU91p1DhOVRuOopznQ")
SIGNING_INPUT = b"eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ"
SIGNATURE = ("cC4hiUPoj9Eetdgtv3hF80EGrhuB__dzERat0XF9g2VtQgr9PJbu3XOiZj5RZmh7AAuHIm4Bh-0Qc_lF5YKt_O8W2Fp5jujGbds9uJdbF9CUAr7t1dnZcAcQ"
             "jbKBYNX4BAynRFdiuB--f_nZLgrnbyTyWzO75vRK5h6xBArLIARNPvkSjtQBMHlb1L07Qe7K0GarZRmB_eSN9383LcOLn6_dO--xi12jzDwusC-eOkHWEsqt"
             "FZESc6BfI7noOPqvhJ1phCnvWh6IeYI2w9QOYEUipUTI8np6LbgGY9Fs98rqVt5AXLIhWkWywlVmtVrBp0igcN_IoypGlUPQGe77Rw")


def pem(*values):
    """A PKCS #1 PEM of the INTEGERs `values`; the signer reads only n and d."""

    def tlv(tag, body):
        size = len(body)
        head = bytes([size]) if size < 128 else bytes([0x80 | (size.bit_length() + 7) // 8]) + size.to_bytes((size.bit_length() + 7) // 8, "big")
        return bytes([tag]) + head + body

    der = tlv(0x30, b"".join(tlv(0x02, v.to_bytes(v.bit_length() // 8 + 1, "big")) for v in values))
    body = base64.b64encode(der).decode()
    lines = [body[i:i + 64] for i in range(0, len(body), 64)]
    return "\n".join(["-----BEGIN RSA PRIVATE KEY-----", *lines, "-----END RSA PRIVATE KEY-----"]) + "\n"


KEY = {"client_id": "client", "user_id": "user", "token_uri": "https://land.copernicus.eu/@@oauth2-token",
       "private_key": pem(0, N, 65537, D, 0, 0, 0, 0, 0)}


class Answer(io.BytesIO):
    def __init__(self, body, headers=None):
        super().__init__(body)
        self.headers = headers or {}

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


class ClmsTest(unittest.TestCase):
    def test_the_signature_is_the_rs256_signature_of_rfc_7515(self):
        signature = clms.b64url(clms.sign(KEY["private_key"], SIGNING_INPUT)).decode()
        self.assertEqual(signature, SIGNATURE)

    def test_a_finished_order_gives_the_member_of_the_archive_inside_its_archive_after_a_cut_download(self):
        inner = io.BytesIO()
        with zipfile.ZipFile(inner, "w") as z:
            z.writestr("package/DATA/clc.tif", b"raster")
            z.writestr("package/DATA/clc_FR_GLP.tif", b"other")
        outer = io.BytesIO()
        with zipfile.ZipFile(outer, "w") as z:
            z.writestr("package.zip", inner.getvalue())
        statuses = iter(["In_progress", "Finished_ok"])
        seen = []

        def urlopen(request, timeout):
            seen.append(request)
            url = request.full_url
            if url == KEY["token_uri"]:
                return Answer(json.dumps({"access_token": "secret-token", "expires_in": 3600}).encode())
            if url.endswith("@datarequest_post"):
                return Answer(json.dumps({"ErrorTaskIds": [], "TaskIds": [{"TaskID": "42"}]}).encode())
            if "@datarequest_status_get" in url:
                return Answer(json.dumps({"Status": next(statuses), "DownloadURL": "https://files.example/7.zip"}).encode())
            body = outer.getvalue()
            # The first answer ends early without an error.
            return Answer(body[:100] if len(seen) == 5 else body, {"Content-Length": str(len(body))})

        with tempfile.TemporaryDirectory() as scratch, mock.patch.object(clms, "urlopen", urlopen), \
                mock.patch.object(clms.API_OPENER, "open", urlopen), mock.patch.object(clms.time, "sleep"), \
                mock.patch("sys.stderr", io.StringIO()):
            work, out, key = Path(scratch, "work"), Path(scratch, "out"), Path(scratch, "key.json")
            work.mkdir(), out.mkdir(), key.write_text(json.dumps(KEY))
            clms.fetch("dataset", "file", "clc.tif", work, out, key)
            self.assertEqual([p.name for p in out.iterdir()], ["clc.tif"])
            self.assertEqual((out / "clc.tif").read_bytes(), b"raster")
            clms.fetch("dataset", "file", "clc.tif", work, out, key)

        token, posted, *polls, cut, download = seen
        self.assertEqual(len(polls), 2, "the second fetch finds the member and asks nothing")
        self.assertEqual(cut.full_url, download.full_url, "a cut download is tried again")
        form = parse_qs(token.data.decode())
        self.assertEqual(form["grant_type"], [clms.GRANT_TYPE])
        claims = json.loads(base64.urlsafe_b64decode(form["assertion"][0].split(".")[1] + "=="))
        self.assertEqual({k: claims[k] for k in ("iss", "sub", "aud")}, {"iss": "client", "sub": "user", "aud": KEY["token_uri"]})
        self.assertLessEqual(claims["exp"] - claims["iat"], 3600)
        self.assertEqual(json.loads(posted.data), {"Datasets": [{"DatasetID": "dataset", "FileID": "file"}]})
        self.assertEqual(posted.get_header("Authorization"), "Bearer secret-token")
        self.assertIsNone(download.get_header("Authorization"), "the token stays with the API")
        self.assertTrue(all("secret-token" not in request.full_url for request in seen))

    def test_a_failed_order_fails_with_its_message(self):
        answers = iter([{"access_token": "t"}, {"TaskIds": [{"TaskID": "9"}]}, {"Status": "Finished_nok", "Message": "no space"}])
        with mock.patch.object(clms.API_OPENER, "open", lambda request, timeout: Answer(json.dumps(next(answers)).encode())):
            with self.assertRaisesRegex(RuntimeError, "order 9 is Finished_nok: no space"):
                clms.order(clms.Session(KEY), "dataset", "file")

    def test_a_finished_order_without_an_https_link_fails(self):
        for url in ("http://files.example/7.zip", None):
            answers = iter([{"access_token": "t"}, {"TaskIds": [{"TaskID": "9"}]}, {"Status": "Finished_ok", "DownloadURL": url}])
            with mock.patch.object(clms.API_OPENER, "open", lambda request, timeout: Answer(json.dumps(next(answers)).encode())):
                with self.assertRaisesRegex(RuntimeError, "order 9 is finished, but links no https URL"):
                    clms.order(clms.Session(KEY), "dataset", "file")

    def test_an_api_call_fails_on_a_redirect_and_an_error_names_only_its_message(self):
        hits = []

        class Service(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                hits.append(self.path)
                if self.path == "/moved":
                    self.send_response(307)
                    self.send_header("Location", "/elsewhere")
                    self.end_headers()
                else:
                    body = json.dumps({"msg": "refused", "token": "secret-token"}).encode()
                    self.send_response(400)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

            def log_message(self, *_):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Service)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            api = f"http://127.0.0.1:{server.server_port}"
            post = lambda path: Request(api + path, data=b"{}", headers={"Authorization": "Bearer secret-token"})
            with self.assertRaisesRegex(RuntimeError, "307"):
                clms.call(post("/moved"), opener=clms.API_OPENER)
            with self.assertRaises(RuntimeError) as refused:
                clms.call(post("/bad"), opener=clms.API_OPENER)
        finally:
            server.shutdown()
            server.server_close()
        self.assertEqual(hits, ["/moved", "/bad"], "the redirect target is never asked")
        self.assertTrue(str(refused.exception).endswith("HTTP Error 400: Bad Request: refused"), str(refused.exception))
        self.assertNotIn("secret-token", str(refused.exception))

    def test_an_answer_with_a_transfer_encoding_or_a_bad_content_length_has_no_length(self):
        self.assertEqual(clms.length({"Content-Length": "1000"}), 1000)
        self.assertIsNone(clms.length({"Content-Length": "1000", "Transfer-Encoding": "chunked"}))
        self.assertIsNone(clms.length({"Content-Length": "1000, 1000"}))

    def test_an_archive_that_is_no_zip_goes_so_the_next_run_downloads_it_again(self):
        with tempfile.TemporaryDirectory() as scratch:
            work, out = Path(scratch, "work"), Path(scratch, "out")
            work.mkdir(), out.mkdir()
            (work / "order.zip").write_bytes(b"not a zip")
            with self.assertRaises(zipfile.BadZipFile):
                clms.fetch("dataset", "file", "clc.tif", work, out, Path(scratch, "absent-key.json"))
            self.assertFalse((work / "order.zip").exists())


if __name__ == "__main__":
    unittest.main()
