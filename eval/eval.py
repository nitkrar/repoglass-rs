"""Score an `rpg` against the hand-written questions, and check the questions.

    python3 eval/eval.py check
    python3 eval/eval.py run [--rpg PATH] [--label NAME] [--json OUT]

`check` confirms every answer path exists under its repository and holds
every answer symbol; run it before trusting a score. `run` indexes each
repository into a private home, asks each question with k=3 under the
content filter the question names, and reports recall@1 and recall@3.
Repositories missing from roots.json, or absent on disk, are skipped.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load() -> tuple[list[dict], dict[str, Path]]:
    questions = json.loads((HERE / "questions.json").read_text())
    roots_file = HERE / "roots.json"
    raw = json.loads(roots_file.read_text()) if roots_file.exists() else {}
    roots = {name: Path(p).expanduser() for name, p in raw.items()}
    return questions, {name: root for name, root in roots.items() if root.is_dir()}


def check(questions: list[dict], roots: dict[str, Path]) -> list[str]:
    problems = []
    ids = [q["id"] for q in questions]
    problems += [f"duplicate id {i}" for i in sorted({i for i in ids if ids.count(i) > 1})]
    if len({frozenset(q) for q in questions}) != 1:
        problems.append("questions disagree on their fields")
    for q in questions:
        if q["negative"] != (not q["answer_paths"]):
            problems.append(f"{q['id']}: negative is {q['negative']} but answer_paths is {q['answer_paths']}")
        root = roots.get(q["repo"])
        if root is None:
            continue
        text = ""
        for path in q["answer_paths"]:
            if (root / path).is_file():
                text += (root / path).read_text(errors="replace")
            else:
                problems.append(f"{q['id']}: {q['repo']}/{path} does not exist")
        problems += [f"{q['id']}: no answer path holds {s!r}"
                     for s in q["answer_symbols"] if s not in text]
    return problems


def rpg(binary: str, home: str, root: Path, *args: str) -> dict:
    env = {**os.environ, "REPOGLASS_HOME": home,
           # Questions arrive seconds apart; a rescan between them would
           # be timed as search.
           "REPOGLASS_RESCAN_AFTER_SECONDS": "3600"}
    out = subprocess.run([binary, *args], cwd=root, env=env,
                         capture_output=True, text=True)
    if out.returncode:
        raise SystemExit(f"{binary} {' '.join(args)} in {root} failed:\n{out.stderr}")
    return json.loads(out.stdout)


def run(questions: list[dict], roots: dict[str, Path], binary: str) -> tuple[list[dict], dict]:
    home = tempfile.mkdtemp(prefix="rpg-eval-")
    build = {}
    for name, root in roots.items():
        t = time.perf_counter()
        rpg(binary, home, root, "index")
        build[name] = round(time.perf_counter() - t, 1)
    results = []
    for q in questions:
        root = roots.get(q["repo"])
        if root is None:
            continue
        t = time.perf_counter()
        hits = rpg(binary, home, root, "search", "-k", "3", "--code", "none",
                   "--content", q["content"], "--", q["query"])["results"]
        ms = (time.perf_counter() - t) * 1000
        paths = [h["path"] for h in hits]
        expected = set(q["answer_paths"])
        results.append({**q, "returned": paths, "ms": round(ms, 1),
                        "at1": bool(paths) and paths[0] in expected,
                        "at3": bool(expected & set(paths))})
    return results, build


def report(results: list[dict], build: dict) -> None:
    scored = [r for r in results if not r["negative"]]

    def row(label: str, rows: list[dict]) -> str:
        ms = sorted(r["ms"] for r in rows)
        return (f"{label:<22}{len(rows):>4}{sum(r['at1'] for r in rows):>6}"
                f"{sum(r['at3'] for r in rows):>6}{ms[len(ms) // 2]:>9.0f}")

    print(f"{'group':<22}{'n':>4}{'@1':>6}{'@3':>6}{'p50 ms':>9}")
    print(row("all", scored))
    for field in ("kind", "content", "repo"):
        groups = defaultdict(list)
        for r in scored:
            groups[r[field]].append(r)
        print()
        for name, rows in sorted(groups.items()):
            print(row(name, rows))
    negatives = [r for r in results if r["negative"]]
    if negatives:
        # There is no score cutoff, so a negative passes only when nothing
        # is returned at all.
        empty = sum(not r["returned"] for r in negatives)
        print(f"\nnegatives answered with nothing: {empty}/{len(negatives)}")
    print("\nindex build: " + ", ".join(f"{n} {s}s" for n, s in build.items()))


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="command", required=True)
    sub.add_parser("check")
    r = sub.add_parser("run")
    r.add_argument("--rpg", default="rpg", help="the rpg to score")
    r.add_argument("--label", default="run")
    r.add_argument("--json", type=Path, help="write each question's returned paths here")
    args = ap.parse_args()

    questions, roots = load()
    if not roots:
        print("no repository in eval/roots.json exists on disk", file=sys.stderr)
        return 1
    problems = check(questions, roots)
    for p in problems:
        print(p, file=sys.stderr)
    if args.command == "check":
        print(f"{len(questions)} questions, repositories: {', '.join(roots)}; "
              f"{len(problems)} problems")
        return 1 if problems else 0
    if problems:
        return 1
    results, build = run(questions, roots, args.rpg)
    report(results, build)
    if args.json:
        args.json.write_text(json.dumps({
            "label": args.label, "rpg": args.rpg, "build_s": build,
            "results": [{"id": r["id"], "returned": r["returned"], "ms": r["ms"]} for r in results],
        }, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
