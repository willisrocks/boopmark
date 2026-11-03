"""Run one candidate over the bookmarks that have no article text.

  python3 run.py baseline|obscura|firecrawl   one JSON line per page, resumable
  python3 run.py idle-rss                      Obscura `serve` RSS after 10 s idle

Reads data/unfetched.json (find_unfetched.py), writes data/results-<candidate>.jsonl.
Every returned page goes through the server's own Readability code
(extract/, built with `cargo build --release`). URLs stay in data/ (gitignored).

OBSCURA defaults to the nixpkgs build: `nix build nixpkgs#obscura --print-out-paths`.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.parse

from fetch_inputs import DATA

HERE = os.path.dirname(os.path.abspath(__file__))
EXTRACT = os.path.join(HERE, "extract/target/release/scraper-eval-extract")
OBSCURA = os.environ.get("OBSCURA", "obscura")
FIRECRAWL = os.environ.get("FIRECRAWL", os.path.expanduser("~/.local/bin/firecrawl"))
TIMEOUT = 120
FIRECRAWL_PACE_S = 7


# Medium marks a member-only story in its page state; the HTML then holds only the preview.
LOCKED = re.compile(r'"isLocked":true|Member-only story')


def extract(url: str, html_path: str) -> dict:
    out = subprocess.run([EXTRACT, "extract", url, html_path], check=False, capture_output=True, text=True, timeout=60)
    with open(html_path, errors="replace") as f:
        locked = bool(LOCKED.search(f.read()))
    return json.loads(out.stdout) | {"locked": locked}


def run_baseline(url: str, _tmp: str) -> dict:
    start = time.monotonic()
    out = subprocess.run([EXTRACT, "fetch", url], check=False, capture_output=True, text=True, timeout=TIMEOUT)
    r = json.loads(out.stdout)
    r["latency_ms"] = int((time.monotonic() - start) * 1000)
    return r


def run_obscura(url: str, tmp: str) -> dict:
    html = os.path.join(tmp, "page.html")
    start = time.monotonic()
    proc = subprocess.run(
        [
            "/usr/bin/time",
            "-l",
            OBSCURA,
            "--stealth",
            "fetch",
            url,
            "--dump",
            "html",
            "-q",
            "--timeout",
            "30",
            "-o",
            html,
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=TIMEOUT,
    )
    latency = int((time.monotonic() - start) * 1000)
    rss = re.search(r"(\d+)\s+maximum resident set size", proc.stderr)
    r = {"latency_ms": latency, "peak_rss_mb": round(int(rss.group(1)) / 2**20, 1) if rss else None}
    if proc.returncode != 0 or not os.path.exists(html) or os.path.getsize(html) == 0:
        tail = [l for l in proc.stderr.splitlines() if "resident" not in l and l.strip()][-1:]
        return r | {"ok": False, "text_chars": 0, "error": f"obscura exit {proc.returncode}: {' '.join(tail)[:200]}"}
    return r | {"html_bytes": os.path.getsize(html)} | extract(url, html)


def run_firecrawl(url: str, tmp: str) -> dict:
    html = os.path.join(tmp, "page.html")
    for _ in range(3):
        start = time.monotonic()
        proc = subprocess.run(
            [FIRECRAWL, "scrape", url, "--format", "rawHtml", "-o", html],
            check=False,
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            cwd=tmp,
        )
        r = {"latency_ms": int((time.monotonic() - start) * 1000)}
        # The free plan allows about 10 requests a minute.
        time.sleep(FIRECRAWL_PACE_S)
        if "Rate limit exceeded" not in proc.stderr + proc.stdout:
            break
        time.sleep(60)
    if proc.returncode != 0 or not os.path.exists(html) or os.path.getsize(html) == 0:
        err = (proc.stderr or proc.stdout).strip().splitlines()[-1:] or ["no output"]
        return r | {"ok": False, "text_chars": 0, "error": f"firecrawl exit {proc.returncode}: {err[0][:200]}"}
    return r | {"html_bytes": os.path.getsize(html)} | extract(url, html)


def idle_rss() -> None:
    proc = subprocess.Popen([OBSCURA, "serve", "--port", "9333"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(10)
        rss_kb = int(
            subprocess.run(
                ["ps", "-o", "rss=", "-p", str(proc.pid)], check=False, capture_output=True, text=True
            ).stdout
        )
        print(json.dumps({"obscura_idle_rss_mb": round(rss_kb / 1024, 1)}))
    finally:
        proc.terminate()
        proc.wait()


RUNNERS = {"baseline": run_baseline, "obscura": run_obscura, "firecrawl": run_firecrawl}


def main() -> None:
    name = sys.argv[1]
    if name == "idle-rss":
        return idle_rss()
    runner = RUNNERS[name]
    pages = [r for r in json.loads((DATA / "unfetched.json").read_text()) if r["state"] == "no_text"]
    out_path = DATA / f"results-{name}.jsonl"
    done = set()
    if out_path.exists():
        done = {json.loads(l)["id"] for l in out_path.read_text().splitlines() if l.strip()}
    with open(out_path, "a") as out, tempfile.TemporaryDirectory() as tmp:
        for i, page in enumerate(pages):
            if page["id"] in done:
                continue
            host = urllib.parse.urlsplit(page["url"]).hostname
            try:
                r = runner(page["url"], tmp)
            except subprocess.TimeoutExpired:
                r = {"ok": False, "text_chars": 0, "error": "harness timeout", "latency_ms": TIMEOUT * 1000}
            for f in os.listdir(tmp):
                os.remove(os.path.join(tmp, f))
            out.write(json.dumps({"id": page["id"], "url": page["url"], "domain": host} | r) + "\n")
            out.flush()
            print(f"{name} {i + 1}/{len(pages)} {host} ok={r.get('ok')} chars={r.get('text_chars')}", flush=True)


if __name__ == "__main__":
    main()
