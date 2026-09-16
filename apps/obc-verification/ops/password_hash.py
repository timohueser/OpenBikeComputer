#!/usr/bin/env python3
"""Generate the configured owner password hash without putting a password in shell history."""
import getpass
import hashlib
import secrets

password = getpass.getpass("Owner password: ")
if len(password) < 16:
    raise SystemExit("Use at least 16 characters.")
if password != getpass.getpass("Repeat password: "):
    raise SystemExit("Passwords differ.")
salt = secrets.token_hex(16)
print(salt + ':' + hashlib.scrypt(password.encode(), salt=salt.encode(), n=16384, r=8, p=1, dklen=64).hex())
