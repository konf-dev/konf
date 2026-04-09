#!/usr/bin/env python3
"""Generate a short-lived GitHub App installation token for konf-agents.

Usage:
    python3 github-app-token.py
    # or
    export KONF_GITHUB_TOKEN=$(python3 github-app-token.py)

Requires: PyJWT, requests, cryptography
    pip install PyJWT requests cryptography
"""

import os
import sys
import time

import jwt
import requests

APP_ID = "3321626"
INSTALLATION_ID = "122520629"
KEY_PATH = os.path.expanduser("~/.config/konf/github-app.pem")


def generate_token() -> str:
    with open(KEY_PATH, "r") as f:
        private_key = f.read()

    now = int(time.time())
    payload = {
        "iat": now - 60,
        "exp": now + (10 * 60),
        "iss": APP_ID,
    }
    encoded = jwt.encode(payload, private_key, algorithm="RS256")

    resp = requests.post(
        f"https://api.github.com/app/installations/{INSTALLATION_ID}/access_tokens",
        headers={
            "Authorization": f"Bearer {encoded}",
            "Accept": "application/vnd.github+json",
        },
    )
    resp.raise_for_status()
    data = resp.json()

    if "token" not in data:
        print(f"Error: {data}", file=sys.stderr)
        sys.exit(1)

    return data["token"]


if __name__ == "__main__":
    token = generate_token()
    print(token)
