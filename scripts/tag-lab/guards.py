#!/usr/bin/env python3
"""Offline guards against wrong LLM merges, evaluated on a saved hosted run.

Reads a results file written by hosted_eval.py with per-pair details (runs
from before details were saved only support the aggregate view). No model
calls, no cost. Each guard is a rule for which checked pairs to accept; the
accepted pairs are folded into the deterministic groups (as the product would)
and scored against the ground truth.

  python3 scripts/tag-lab/guards.py results/<haiku run>.json [--second results/<other model run>.json]

Writes results/<run>.guards.json next to the input.
"""
import argparse
import json
import pathlib

from hosted_eval import fold_pairs, load
from verify import candidate_reason, letters, words

# Extra words that do not narrow a topic ("rust" vs "rust lang", "node" vs "nodejs").
GENERIC = {"lang", "language", "js", "dev", "development", "programming", "framework", "app", "apps",
           "tool", "tools", "db", "io"}


def narrower(a, b):
    """True when one tag looks like a narrower topic than the other:
    - word subset with a non-generic extra word ("ruby" / "ruby on rails",
      "design" / "design system", "js" / "react.js")
    - one tag is the prefix or suffix of a single word of the other, with a
      non-generic remainder of 4+ letters ("sql" / "postgresql", "java" /
      "javascript")"""
    wa, wb = set(words(a)), set(words(b))
    if wa and wb and wa != wb and (wa < wb or wb < wa):
        extra = (wb - wa) if wa < wb else (wa - wb)
        if not extra <= GENERIC:
            return True
    la, lb = letters(a), letters(b)
    short, long_tag = (a, b) if len(la) <= len(lb) else (b, a)
    s = letters(short)
    if len(s) >= 3:
        for w in words(long_tag):
            if w != s and (w.startswith(s) or w.endswith(s)):
                rest = w[len(s):] if w.startswith(s) else w[:-len(s)]
                if len(rest) >= 4 and rest not in GENERIC:
                    return True
    return False


def spelled(tag):
    return tag.replace("#", " sharp").replace("+", " plus")


def symbol_artifact(a, b):
    """The string-candidate pass drops symbols, so "c#" became "c" and matched
    every tag starting with c. True when a pair is a candidate only because of
    that (the fix belongs in candidate_reason; this measures it)."""
    return candidate_reason(a, b) is not None and candidate_reason(spelled(a), spelled(b)) is None


def aggregate(path, run, truth, det):
    """For runs saved before per-pair details: test the veto rules on the
    wrong pairs the run listed, and bound their recall cost by applying them
    to every same-concept pair among the deterministic tags (the most the
    model could have accepted correctly)."""
    canon = [g["canonical"] for g in det]
    same_pairs = [(a, b) for i, a in enumerate(canon) for b in canon[i + 1:]
                  if truth["concept_of"][a] == truth["concept_of"][b]]
    wrong = [tuple(w[:2]) for w in run["pairs"]["wrong_accepted"]]
    wrong_high = [tuple(w[:2]) for w in run["pairs"]["wrong_accepted"] if w[2] == "high"]
    rules = {"symbol-aware candidates": symbol_artifact, "narrower-topic rule": narrower,
             "both": lambda a, b: symbol_artifact(a, b) or narrower(a, b)}
    out = {"source": path.name, "mode": "aggregate (no per-pair details in this run)",
           "wrong_pairs_high": len(wrong_high), "residual_same_pairs": len(same_pairs), "rules": {}}
    print(f"{path.name}: no per-pair details, aggregate view")
    print(f"wrong accepted pairs at high: {len(wrong_high)} (high or medium: {len(wrong)})")
    for name, rule in rules.items():
        caught = [p for p in wrong_high if rule(*p)]
        lost = [p for p in same_pairs if rule(*p)]
        out["rules"][name] = {"wrong_high_vetoed": len(caught), "wrong_high_left": [p for p in wrong_high if not rule(*p)],
                              "same_pairs_vetoed_upper_bound": len(lost), "same_pairs_vetoed": lost}
        print(f"  {name:<20} vetoes {len(caught)}/{len(wrong_high)} wrong pairs; would veto {len(lost)} of "
              f"{len(same_pairs)} same-concept pairs (upper bound on recall cost): {lost}")
    dest = path.with_name(path.stem + ".guards.json")
    dest.write_text(json.dumps(out, indent=1) + "\n")
    print(f"written: {dest}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--second", help="another model's results file, for a cross-model agreement guard")
    args = ap.parse_args()
    path = pathlib.Path(args.results)
    run = json.loads(path.read_text())
    rows = run.get("pairs", {}).get("details")
    truth, _, det, _, _ = load()
    if rows is None:
        return aggregate(path, run, truth, det)

    high = [r for r in rows if r["same"] and r["confidence"] == "high"]
    cannot = [(r["a"], r["b"]) for r in rows if r["answered"] and not r["same"]]
    proposals = run.get("consolidate", {}).get("details", [])

    guards = {
        "baseline: pair check high": high,
        "pair check high + echo matches": [r for r in high if r.get("echo_ok") is not False],
        "candidates only (no whole-list pairs)": [r for r in high if r["source"] != "whole-list"],
        "fix: symbol-aware candidates": [r for r in high if not (r["source"] == "candidate" and symbol_artifact(r["a"], r["b"]))],
        "veto: narrower-topic rule": [r for r in high if not narrower(r["a"], r["b"])],
        "symbol fix + narrower-topic veto": [r for r in high if not narrower(r["a"], r["b"])
                                             and not (r["source"] == "candidate" and symbol_artifact(r["a"], r["b"]))],
        "agreement: pair high AND whole-list proposed it": [r for r in high if r["source"] != "candidate"],
        "agreement: pair high AND whole-list proposed it high": [r for r in high if r["proposal_confidence"] == "high"],
        "whole-list proposals at high, no pair check": [
            {"a": p["a"], "b": p["b"], "label": p["label"]} for p in proposals if p["confidence"] == "high"],
    }
    if args.second:
        other = json.loads(pathlib.Path(args.second).read_text())["pairs"].get("details") or []
        other_high = {frozenset((r["a"], r["b"])) for r in other if r["same"] and r["confidence"] == "high"}
        other_asked = {frozenset((r["a"], r["b"])) for r in other if r["answered"]}
        guards["cross-model: both say high (pairs both were asked)"] = [
            r for r in high if frozenset((r["a"], r["b"])) in other_high]
        guards["cross-model: both say high, or only this one was asked"] = [
            r for r in high if frozenset((r["a"], r["b"])) in other_high
            or frozenset((r["a"], r["b"])) not in other_asked]

    out = {}
    for name, acc in guards.items():
        pairs = [(r["a"], r["b"]) for r in acc]
        c = sum(r["label"] == "same" for r in acc)
        w = sum(r["label"] == "different" for r in acc)
        entry = {"accepted": len(acc), "correct": c, "wrong": w, "ambiguous": len(acc) - c - w,
                 "pipeline": fold_pairs(det, truth, pairs),
                 "pipeline_with_cannot_link": fold_pairs(det, truth, pairs, cannot),
                 "wrong_pairs": [(r["a"], r["b"]) for r in acc if r["label"] == "different"]}
        out[name] = entry
    dest = path.with_name(path.stem + ".guards.json")
    dest.write_text(json.dumps({"source": path.name, "second": args.second, "guards": out}, indent=1) + "\n")
    print(f"{'guard':<56} {'acc':>4} {'ok':>4} {'wrong':>5} | {'tags':>4} {'wrong':>5} {'recall':>6} | +cannot-link: wrong recall")
    for name, e in out.items():
        p, q = e["pipeline"], e["pipeline_with_cannot_link"]
        print(f"{name:<56} {e['accepted']:>4} {e['correct']:>4} {e['wrong']:>5} | {p['tags']:>4} {p['wrong']:>5} "
              f"{p['recall']:>6} | {q['wrong']:>5} {q['recall']:>6}")
    print(f"written: {dest}")


if __name__ == "__main__":
    main()
