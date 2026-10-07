#!/usr/bin/env python3
"""Generate a bootstrap password hash, or recover the local admin account."""
import argparse
import getpass
import hashlib
from pathlib import Path
import secrets
import sqlite3


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reset", type=Path, metavar="DATA_DIR", help="replace the local admin password in an existing database")
    args = parser.parse_args()
    password = getpass.getpass("Admin password: ")
    if not 16 <= len(password.encode("utf-16-le")) // 2 <= 1024:
        raise SystemExit("Use between 16 and 1024 characters.")
    if password != getpass.getpass("Repeat password: "):
        raise SystemExit("Passwords differ.")
    salt = secrets.token_hex(16)
    encoded = salt + ':' + hashlib.scrypt(password.encode(), salt=salt.encode(), n=16384, r=8, p=1, dklen=64).hex()
    if args.reset is None:
        print(encoded)
        return
    database = (args.reset / "verification.sqlite").resolve()
    with sqlite3.connect(database.as_uri() + "?mode=rw", uri=True) as connection:
        connection.execute("BEGIN IMMEDIATE")
        connection.execute("INSERT INTO local_admin(id,password_hash) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET password_hash=excluded.password_hash", (encoded,))
        connection.execute("DELETE FROM sessions WHERE json_extract(actor,'$.provider')='local' OR json_extract(actor,'$.provider') IS NULL")
        connection.execute("DELETE FROM login_attempts")
    print("Local admin password replaced. Local admin sessions ended.")


if __name__ == "__main__":
    main()
