#!/usr/bin/env python3
"""Pairwise merge verification (spec: proposal B, and a candidate-pair variant).

Asks one yes/no question per tag pair and reads P(yes) from the answer
token's log-probabilities. This mimics a decision-API predicate (OpenAI
Decisions `predicate`, TypeSafe Jev `Noul`) with a local model; the
probabilities are NOT calibrated the way those vendors claim theirs are.

Pair sources:
  --from-groups out/<run>.groups.json   verify an LLM run's proposals
  --candidates                          cheap string candidates over the
                                        deterministic tags (abbreviation,
                                        numeronym, substring, typo distance,
                                        shared words)

Writes out/verify-<run>.json and prints precision/recall at thresholds.
"""
import argparse
import json
import math
import pathlib
import time

from consolidate import OUT, post

HERE = pathlib.Path(__file__).resolve().parent


def letters(tag):
    return "".join(c for c in tag.lower() if c.isalnum())


def words(tag):
    return [w for w in "".join(c if c.isalnum() else " " for c in tag.lower()).split() if w]


def is_subsequence(short, long):
    it = iter(long)
    return all(c in it for c in short)


def edit_distance(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def candidate_reason(a, b):
    la, lb = letters(a), letters(b)
    if not la or not lb:
        return None
    short, long = (la, lb) if len(la) <= len(lb) else (lb, la)
    wl = words(b if long == lb else a)
    if len(short) <= 5 and short[0] == long[0] and is_subsequence(short, long):
        return "abbreviation"
    if len(short) <= 5 and len(wl) > 1 and short == "".join(w[0] for w in wl):
        return "initials"
    if len(short) >= 3 and short[1:-1].isdigit() and short[0] == long[0] and short[-1] == long[-1] \
            and int(short[1:-1]) == len(long) - 2:
        return "numeronym"
    if len(short) >= 2 and short in long:
        return "substring"
    if len(short) >= 5 and edit_distance(la, lb) <= 2:
        return "typo"
    wa, wb = set(words(a)), set(words(b))
    if wa and wb and len(wa & wb) / len(wa | wb) >= 0.5:
        return "shared-words"
    return None


QUESTION = """Two tags from one person's bookmark library:
Tag A: {a}{ta}
Tag B: {b}{tb}

Do A and B mean the same thing, so that they should be merged into one tag? Abbreviations, \
synonyms, alternate spellings and typos count as the same. A broader and a narrower topic, or two \
related but different topics, do not. Answer with exactly one word: yes or no."""


def p_yes(base_url, model, a, b, samples):
    def t(tag):
        s = (samples.get(tag) or [""])[0]
        return f" (e.g. {json.dumps(s)})" if s else ""
    prompt = QUESTION.format(a=json.dumps(a), b=json.dumps(b), ta=t(a), tb=t(b))
    resp = post(f"{base_url}/v1/chat/completions", {"Content-Type": "application/json"}, {
        "model": model, "temperature": 0, "max_tokens": 1, "logprobs": True, "top_logprobs": 10,
        "messages": [{"role": "user", "content": prompt}],
    }, timeout=300)
    top = resp["choices"][0]["logprobs"]["content"][0]["top_logprobs"]
    py = sum(math.exp(x["logprob"]) for x in top if x["token"].strip().lower() == "yes")
    pn = sum(math.exp(x["logprob"]) for x in top if x["token"].strip().lower() == "no")
    u = resp.get("usage", {})
    return (py / (py + pn) if py + pn else 0.5), u.get("prompt_tokens", 0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--model", default="local")
    ap.add_argument("--from-groups")
    ap.add_argument("--candidates", action="store_true")
    ap.add_argument("--base-url", default="http://127.0.0.1:4211")
    args = ap.parse_args()

    truth = json.loads((HERE / "ground_truth.json").read_text())
    concept_of, ambiguous = truth["concept_of"], {frozenset(p) for p in truth["ambiguous"]}
    samples = json.loads((OUT / "samples.json").read_text())
    det = json.loads((OUT / "deterministic.json").read_text())["plural"]
    canon = [g["canonical"] for g in det]

    pairs = []
    if args.from_groups:
        for g in json.loads(pathlib.Path(args.from_groups).read_text()):
            for m in g.get("proposed", []):
                if m != g["canonical"]:
                    pairs.append((m, g["canonical"], "llm"))
    if args.candidates:
        for i, a in enumerate(canon):
            for b in canon[i + 1:]:
                r = candidate_reason(a, b)
                if r:
                    pairs.append((a, b, r))

    rows, tokens, t0 = [], 0, time.time()
    for a, b, reason in pairs:
        p, n = p_yes(args.base_url, args.model, a, b, samples)
        tokens += n
        ca, cb = concept_of[a], concept_of[b]
        label = "same" if ca == cb else "ambiguous" if frozenset((ca, cb)) in ambiguous else "different"
        rows.append({"a": a, "b": b, "reason": reason, "p_yes": round(p, 4), "label": label})
    elapsed = time.time() - t0

    # How many residual same-concept pairs exist among deterministic tags (recall denominator)?
    residual = sum(1 for i, a in enumerate(canon) for b in canon[i + 1:] if concept_of[a] == concept_of[b])
    found = sum(1 for r in rows if r["label"] == "same")
    (OUT / f"verify-{args.run}.json").write_text(json.dumps({"rows": rows, "residual_same_pairs": residual}, indent=1))
    print(f"{args.run}: pairs={len(rows)} same={found} different={sum(r['label']=='different' for r in rows)} "
          f"ambiguous={sum(r['label']=='ambiguous' for r in rows)} residual_same_pairs={residual} "
          f"prompt_tokens={tokens} ({tokens / max(len(rows), 1):.0f}/pair) {elapsed:.0f}s")
    for th in (0.5, 0.7, 0.9):
        acc = [r for r in rows if r["p_yes"] >= th]
        c = sum(r["label"] == "same" for r in acc)
        w = sum(r["label"] == "different" for r in acc)
        print(f"  p>={th}: accepted={len(acc)} correct={c} wrong={w} "
              f"precision={c / (c + w) if c + w else float('nan'):.2f} recall_of_residual={c / residual:.2f}")


if __name__ == "__main__":
    main()
