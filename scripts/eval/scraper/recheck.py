"""Second pass for find_unfetched.py: re-search each no_text bookmark by a
quoted title phrase, which forces a lexical match, so a missing snippet is
conclusive. Bookmarks the phrase search cannot return keep their state.
"""

import json
import re
import urllib.parse

from fetch_inputs import DATA, get


def main() -> None:
    rows = json.loads((DATA / "unfetched.json").read_text())
    titles = {b["id"]: b["title"] for b in json.loads((DATA / "bookmarks.json").read_text())}
    for r in rows:
        if r["state"] != "no_text":
            continue
        words = re.findall(r"[A-Za-z0-9]+", titles.get(r["id"]) or "")[:5]
        if len(words) < 2:
            continue
        q = '"' + " ".join(words) + '"'
        hits = json.loads(get("/api/v1/bookmarks?limit=50&search=" + urllib.parse.quote(q)))
        hit = next((h for h in hits if h["id"] == r["id"]), None)
        if hit is not None:
            r["state"] = "fetched" if hit.get("snippet") else "no_text"
            r["lexical_checked"] = True
    (DATA / "unfetched.json").write_text(json.dumps(rows, indent=1))
    print(
        sum(r["state"] == "no_text" for r in rows),
        "no_text,",
        sum(r.get("lexical_checked", False) for r in rows),
        "checked lexically",
    )


if __name__ == "__main__":
    main()
