#!/usr/bin/env python3
"""Checks the App Store Connect API key the way Apple does, and says what is wrong in words.

It signs a short-lived token with the key and asks Apple for a few lists. The key itself is never
printed. Reads ASC_KEY_ID, ASC_ISSUER_ID and ASC_KEY_P8 from the environment; with
`--write PATH` it also saves the key there, cleaned up (the usual paste damage is repaired: lost
line breaks, Windows line endings, stray quotes or spaces).
"""
import base64
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

BUNDLE_ID = "io.github.noxilith.EloraNotes"
API = "https://api.appstoreconnect.apple.com"


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def clean_key(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n").strip().strip("'\"").strip()
    match = re.search(r"-----BEGIN PRIVATE KEY-----(.*?)-----END PRIVATE KEY-----", text, re.S)
    if not match:
        return ""
    body = re.sub(r"\s+", "", match.group(1))
    lines = [body[i : i + 64] for i in range(0, len(body), 64)]
    return "-----BEGIN PRIVATE KEY-----\n" + "\n".join(lines) + "\n-----END PRIVATE KEY-----\n"


def der_to_raw(der: bytes) -> bytes:
    """An ECDSA signature as openssl writes it (DER) to the 64 bytes a JWT wants."""

    def read_len(pos):
        first = der[pos]
        if first < 0x80:
            return first, pos + 1
        count = first & 0x7F
        return int.from_bytes(der[pos + 1 : pos + 1 + count], "big"), pos + 1 + count

    assert der[0] == 0x30
    _, pos = read_len(1)
    numbers = []
    for _ in range(2):
        assert der[pos] == 0x02
        length, pos = read_len(pos + 1)
        numbers.append(int.from_bytes(der[pos : pos + length], "big"))
        pos += length
    return b"".join(n.to_bytes(32, "big") for n in numbers)


def token(key_id: str, issuer: str, pem: str) -> str:
    now = int(time.time())
    header = b64url(json.dumps({"alg": "ES256", "kid": key_id, "typ": "JWT"}).encode())
    payload = b64url(
        json.dumps({"iss": issuer, "iat": now, "exp": now + 600, "aud": "appstoreconnect-v1"}).encode()
    )
    signing_input = f"{header}.{payload}".encode()
    with tempfile.NamedTemporaryFile("w", suffix=".pem") as key_file:
        key_file.write(pem)
        key_file.flush()
        signed = subprocess.run(
            ["openssl", "dgst", "-sha256", "-sign", key_file.name],
            input=signing_input,
            capture_output=True,
        )
    if signed.returncode != 0:
        raise RuntimeError("openssl could not read the key: " + signed.stderr.decode().strip()[:200])
    return f"{header}.{payload}.{b64url(der_to_raw(signed.stdout))}"


def ask(jwt: str, path: str):
    request = urllib.request.Request(API + path, headers={"Authorization": "Bearer " + jwt})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        try:
            return error.code, json.load(error)
        except Exception:
            return error.code, {}


def main() -> int:
    key_id = os.environ.get("ASC_KEY_ID", "").strip()
    issuer = os.environ.get("ASC_ISSUER_ID", "").strip()
    pem = clean_key(os.environ.get("ASC_KEY_P8", ""))
    bad = []
    if not re.fullmatch(r"[A-Z0-9]{10}", key_id):
        bad.append(
            f"ASC_KEY_ID should be 10 letters and digits (the Key ID column); it is {len(key_id)} characters."
        )
    if not re.fullmatch(r"[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", issuer):
        bad.append(
            "ASC_ISSUER_ID should look like 12345678-abcd-abcd-abcd-1234567890ab (shown at the top of the "
            f"Team Keys page); it is {len(issuer)} characters. An Individual key has no Issuer ID: make a Team key."
        )
    if not pem:
        bad.append(
            "ASC_KEY_P8 does not contain a key: paste the whole .p8 file, from -----BEGIN PRIVATE KEY----- "
            "to -----END PRIVATE KEY-----."
        )
    if bad:
        print("The secrets are not in the shape Apple's keys have:")
        for line in bad:
            print(" -", line)
        return 1

    if len(sys.argv) == 3 and sys.argv[1] == "--write":
        os.makedirs(os.path.dirname(sys.argv[2]), exist_ok=True)
        with open(sys.argv[2], "w") as out:
            out.write(pem)
        os.chmod(sys.argv[2], 0o600)

    try:
        jwt = token(key_id, issuer, pem)
    except Exception as error:  # noqa: BLE001
        print("Could not sign with the key:", error)
        return 1

    status, body = ask(jwt, "/v1/apps?limit=1")
    if status == 401:
        print("Apple rejected the key (401 not authorized). The three secrets do not belong together, or the")
        print("key is not valid for this team. Check, in App Store Connect > Users and Access > Integrations >")
        print("App Store Connect API > TEAM keys (not Individual keys):")
        print(" - ASC_KEY_ID is the Key ID of the key whose .p8 you pasted;")
        print(" - ASC_ISSUER_ID is the Issuer ID at the top of that page;")
        print(" - the key is not revoked.")
        return 1
    if status == 403:
        print("Apple knows the key but will not let it list apps: it needs Admin (or App Manager) access.")
        return 1
    if status != 200:
        detail = (body.get("errors") or [{}])[0].get("detail", "")
        print(f"Apple answered {status}: {detail}")
        return 1
    print("Apple accepts the key.")

    ok = True
    q = urllib.parse.quote(BUNDLE_ID)
    status, body = ask(jwt, f"/v1/bundleIds?filter[identifier]={q}")
    team = os.environ.get("APPLE_TEAM_ID", "").strip()
    if status == 200 and body.get("data"):
        print(f"The bundle ID {BUNDLE_ID} is registered.")
        # The part of an App ID before the bundle ID is the team's own ID, so Apple says which
        # team this is, and it is used even when the APPLE_TEAM_ID secret is wrong or has a stray space.
        seed = (body["data"][0].get("attributes") or {}).get("seedId", "")
        if seed and seed != team:
            print(f"APPLE_TEAM_ID is {'empty' if not team else 'not'} this team's ID. Apple says the team is {seed}; using that.")
            team = seed
        elif seed:
            print("APPLE_TEAM_ID matches the team.")
    else:
        print(f"The bundle ID {BUNDLE_ID} is NOT registered yet (Certificates, Identifiers & Profiles > Identifiers).")
        ok = False
    status, body = ask(jwt, f"/v1/apps?filter[bundleId]={q}")
    apps = body.get("data") or []
    if status == 200 and apps:
        print("The app record exists:", apps[0]["attributes"].get("name"))
    else:
        print("There is NO app record for that bundle ID yet (App Store Connect > Apps > + New App).")
        ok = False
    env_file = os.environ.get("GITHUB_ENV")
    if env_file and team:
        with open(env_file, "a") as out:
            out.write(f"APPLE_TEAM_ID={team}\n")
    status, _ = ask(jwt, "/v1/certificates?limit=1")
    if status == 200:
        print("The key can manage signing certificates.")
    else:
        print(f"The key cannot list signing certificates ({status}): give it Admin access so the build can make one.")
        ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
