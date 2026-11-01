#!/usr/bin/env python3
"""Score a tag grouping against ground_truth.json.

Input: a JSON file with a list of groups, each {"canonical": str, "members": [str]},
or an object of named lists (as printed by `examples/tag_lab.rs`).

A *merge* is one member folded into its group's canonical tag. It is:
- correct    when both raw tags belong to the same concept,
- ambiguous  when the concept pair is listed in AMBIGUOUS (either answer is fine),
- wrong      otherwise.
Precision = correct / (correct + wrong); ambiguous merges are reported apart.
Recall    = correct merges / the merges a perfect grouping would make
            (raw tags minus concepts).

Usage: python3 measure.py GROUPS.json [--name key] [--show-wrong]
"""
import argparse
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent


def score(groups, truth):
    concept_of = truth["concept_of"]
    ambiguous = {frozenset(p) for p in truth["ambiguous"]}
    correct, amb, wrong = [], [], []
    for g in groups:
        canon = g["canonical"]
        for m in g["members"]:
            if m == canon:
                continue
            a, b = concept_of[m], concept_of[canon]
            if a == b:
                correct.append((m, canon))
            elif frozenset((a, b)) in ambiguous:
                amb.append((m, canon))
            else:
                wrong.append((m, canon))
    ideal = len(concept_of) - len(set(concept_of.values()))
    decided = len(correct) + len(wrong)
    return {
        "tags": len(groups),
        "merges": len(correct) + len(amb) + len(wrong),
        "correct": len(correct),
        "ambiguous": len(amb),
        "wrong": len(wrong),
        "precision": round(len(correct) / decided, 4) if decided else None,
        "recall": round(len(correct) / ideal, 4),
        "concepts": len(set(concept_of.values())),
        "wrong_merges": wrong,
        "ambiguous_merges": amb,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("groups")
    ap.add_argument("--show-wrong", action="store_true")
    args = ap.parse_args()
    truth = json.loads((HERE / "ground_truth.json").read_text())
    data = json.loads(pathlib.Path(args.groups).read_text())
    named = data if isinstance(data, dict) else {"groups": data}
    print(f"raw distinct tags: {len(truth['concept_of'])}, concepts: {len(set(truth['concept_of'].values()))}")
    for name, groups in named.items():
        s = score(groups, truth)
        print(f"{name:>10}: tags={s['tags']} merges={s['merges']} correct={s['correct']} "
              f"ambiguous={s['ambiguous']} wrong={s['wrong']} precision={s['precision']} recall={s['recall']}")
        if args.show_wrong:
            for m, c in s["wrong_merges"]:
                print(f"             WRONG {m!r} -> {c!r}")
            for m, c in s["ambiguous_merges"]:
                print(f"             ambiguous {m!r} -> {c!r}")


if __name__ == "__main__":
    main()
