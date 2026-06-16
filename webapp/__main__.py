"""Entry point: `python -m webapp` launches the server and opens a browser."""
import argparse
import threading
import webbrowser

import uvicorn


def main():
    parser = argparse.ArgumentParser(description="OBCM Web Builder")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--no-browser", action="store_true", help="Don't auto-open a browser")
    args = parser.parse_args()

    url = f"http://{args.host if args.host != '0.0.0.0' else 'localhost'}:{args.port}"
    if not args.no_browser:
        threading.Timer(1.0, lambda: webbrowser.open(url)).start()

    print(f"OBCM Web Builder running at {url}")
    uvicorn.run("webapp.server:app", host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
