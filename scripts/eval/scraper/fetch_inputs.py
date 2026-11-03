"""Download the user's bookmarks and the Settings search-index panel from prod.

Reads BOOPMARK_API_KEY from the environment; writes data/bookmarks.json and
data/search_index.html (both gitignored, they hold private URLs).
"""

import json
import os
import pathlib
import urllib.request

BASE = os.environ.get("BOOPMARK_URL", "https://boopmark.com")
DATA = pathlib.Path(__file__).parent / "data"


def get(path: str) -> bytes:
    req = urllib.request.Request(
        BASE + path,
        headers={"Authorization": "Bearer " + os.environ["BOOPMARK_API_KEY"], "User-Agent": "boopmark-scraper-eval"},
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read()


def main() -> None:
    DATA.mkdir(exist_ok=True)
    rows, offset = [], 0
    while True:
        page = json.loads(get(f"/api/v1/bookmarks?limit=100&offset={offset}"))
        rows += page
        if len(page) < 100:
            break
        offset += 100
    (DATA / "bookmarks.json").write_text(json.dumps(rows, indent=1))
    (DATA / "search_index.html").write_bytes(get("/settings/search-index"))
    print(f"{len(rows)} bookmarks")


if __name__ == "__main__":
    main()
