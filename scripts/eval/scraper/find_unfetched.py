"""Find which bookmarks have no article text, through the search API only.

The search API returns a snippet for a bookmark exactly when the lexical leg
matched it and its article text is stored (search_repo.rs, `snippet` CASE).
So each bookmark is searched by its own title: a lexical hit without a snippet
has no article text. Bookmarks the search does not return are reported as
unknown. Reads BOOPMARK_API_KEY; writes data/unfetched.json (gitignored).
"""

import json
import re
import time
import urllib.parse

from fetch_inputs import DATA, get

STOP = {"or", "and", "the", "a", "an", "of", "to", "in", "for", "on", "with", "is", "how", "you"}


def query_for(title: str) -> str:
    words = [w for w in re.findall(r"[A-Za-z0-9]+", title or "") if w.lower() not in STOP]
    return " ".join(words[:6])


def main() -> None:
    bookmarks = json.loads((DATA / "bookmarks.json").read_text())
    out = []
    for b in bookmarks:
        q = query_for(b["title"]) or urllib.parse.urlsplit(b["url"]).hostname
        hits = json.loads(get("/api/v1/bookmarks?limit=50&search=" + urllib.parse.quote(q)))
        hit = next((h for h in hits if h["id"] == b["id"]), None)
        state = "unknown" if hit is None else ("fetched" if hit.get("snippet") else "no_text")
        out.append({"id": b["id"], "url": b["url"], "state": state})
        time.sleep(0.2)
    (DATA / "unfetched.json").write_text(json.dumps(out, indent=1))
    counts = {}
    for r in out:
        counts[r["state"]] = counts.get(r["state"], 0) + 1
    print(counts)


if __name__ == "__main__":
    main()
