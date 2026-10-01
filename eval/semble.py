"""Score an `rpg` on semble's public code-search benchmark.

    python3 eval/semble.py [--rpg PATH] [--full] [--languages python go] [--repos aiohttp]
                           [--config repoglass.toml] [--label NAME] [--json OUT]

The benchmark (1,251 queries over 63 repositories in 19 languages, from
github.com/MinishLab/semble) is fetched at a pinned commit. By default
only SUBSET is scored, 19 repositories and 376 queries; --full scores all
63. Each
repository is indexed with `rpg index --git URL --rev COMMIT`, which
fetches it once and keeps the checkout, so later runs, including runs
with other settings, download nothing. Scoring follows semble's: a hit
matches a label by path, and by line span when the label pins one;
primary and secondary labels are pooled; NDCG at 10 and 5.

Everything lives under --home (default ~/.cache/repoglass-rs/eval); delete
it to start over.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

SEMBLE_URL = "https://github.com/MinishLab/semble.git"
#: The benchmark revision scored against; change it deliberately.
SEMBLE_REV = "0051e000fcaac69a9c5d081ebbc8d4cb8508160b"
TOP_K = 10
#: One repository per language: the median of that language's repositories
#: by files indexed at SEMBLE_REV. Size drives difficulty, so the smallest
#: overstate quality and the largest understate it. On Python repoglass's
#: saved full run this subset scored NDCG@10 0.805, as did all 63; expect
#: it within about 0.02 of the full score, and run --full before a release.
SUBSET = frozenset({
    "aeson", "alamofire", "chi", "commons-lang", "curl", "ecto", "exposed", "http4s",
    "lazy.nvim", "messagepack-csharp", "model2vec", "monolog", "nlohmann-json", "nvm",
    "rack", "redux", "serde", "vitest", "zls",
})


def fetch_benchmark(home: Path) -> Path:
    """semble's benchmarks/ directory at SEMBLE_REV, fetched once."""
    dest = home / f"semble-{SEMBLE_REV[:12]}"
    if not (dest / "benchmarks" / "repos.json").exists():
        tmp = home / f".semble-{os.getpid()}"
        subprocess.run(["rm", "-rf", str(tmp)], check=True)
        tmp.mkdir(parents=True)
        for args in (["init", "-q"], ["fetch", "-q", "--depth", "1", "--", SEMBLE_URL, SEMBLE_REV],
                     ["-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"]):
            subprocess.run(["git", *args], cwd=tmp, check=True, stdin=subprocess.DEVNULL)
        tmp.rename(dest)
    return dest / "benchmarks"


def load(bench: Path) -> tuple[dict[str, dict], list[dict]]:
    repos = {r["name"]: r for r in json.loads((bench / "repos.json").read_text())}
    tasks = []
    for f in sorted((bench / "annotations").glob("*.json")):
        for item in json.loads(f.read_text()):
            repo = item.get("repo", f.stem)
            if repo not in repos:
                continue
            labels = [t if isinstance(t, dict) else {"path": t}
                      for t in item.get("relevant", []) + item.get("secondary", [])]
            tasks.append({"repo": repo, "language": repos[repo]["language"], "query": item["query"],
                          "category": item.get("category", "semantic"), "labels": labels})
    return repos, tasks


def path_matches(found: str, label: str) -> bool:
    """Either may be a suffix of the other: a repository indexed at its
    benchmark_root returns paths below it, while labels start at the
    repository root."""
    return found == label or found.endswith(f"/{label}") or label.endswith(f"/{found}")


def covers(hit: dict, label: dict) -> bool:
    if not path_matches(hit["path"], label["path"]):
        return False
    start, end = label.get("start_line"), label.get("end_line")
    return start is None or end is None or not (hit["end_line"] < start or hit["start_line"] > end)


def ndcg(ranks: list[int], relevant: int, k: int) -> float:
    if not relevant:
        return 0.0
    dcg = sum(1 / math.log2(r + 1) for r in set(ranks) if r <= k)
    return dcg / sum(1 / math.log2(i + 2) for i in range(min(k, relevant)))


class Rpg:
    def __init__(self, binary: str, home: Path, config: Path | None) -> None:
        # Questions arrive seconds apart; a rescan between them would be
        # timed as search.
        self.env = {**os.environ, "REPOGLASS_HOME": str(home), "REPOGLASS_RESCAN_AFTER_SECONDS": "3600"}
        self.binary, self.config = binary, config

    def __call__(self, command: str, repo: dict, *args: str) -> dict:
        where = ["--git", repo["url"], "--rev", repo["revision"]]
        if repo.get("benchmark_root"):
            where += ["-r", repo["benchmark_root"]]
        if self.config:
            where += ["--config", str(self.config)]
        out = subprocess.run([self.binary, command, *where, *args], env=self.env,
                             capture_output=True, text=True)
        if out.returncode:
            raise RuntimeError(f"rpg {command} on {repo['name']} failed:\n{out.stderr}")
        return json.loads(out.stdout)


def report(rows: list[dict], title: str, field: str) -> None:
    print(f"\n{title:<22}{'n':>6}{'NDCG@10':>10}{'NDCG@5':>9}{'any hit':>10}")
    groups = defaultdict(list)
    for r in rows:
        groups[r[field]].append(r)
    for name, rs in sorted(groups.items()):
        print(f"{name:<22}{len(rs):>6}{sum(x['ndcg10'] for x in rs) / len(rs):>10.3f}"
              f"{sum(x['ndcg5'] for x in rs) / len(rs):>9.3f}{sum(x['hit'] for x in rs) / len(rs):>10.0%}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpg", default="rpg", help="the rpg to score")
    ap.add_argument("--home", type=Path, default=Path("~/.cache/repoglass-rs/eval").expanduser(),
                    help="benchmark, checkouts and indexes")
    ap.add_argument("--config", type=Path, help="repoglass.toml passed to every rpg call")
    ap.add_argument("--full", action="store_true", help="all 63 repositories, not SUBSET")
    ap.add_argument("--languages", nargs="*")
    ap.add_argument("--repos", nargs="*")
    ap.add_argument("--label", default="run")
    ap.add_argument("--json", type=Path, help="write per-query results here")
    args = ap.parse_args()

    args.home.mkdir(parents=True, exist_ok=True)
    repos, tasks = load(fetch_benchmark(args.home))
    unknown = SUBSET - repos.keys()
    if unknown:
        print(f"SUBSET names repositories the benchmark lacks: {sorted(unknown)}", file=sys.stderr)
        return 1
    chosen = {n: r for n, r in repos.items()
              if (args.full or args.repos or n in SUBSET)
              and (not args.languages or r["language"] in args.languages)
              and (not args.repos or n in args.repos)}
    if not chosen:
        print("no repository matches", file=sys.stderr)
        return 1
    rpg = Rpg(args.rpg, args.home, args.config)
    rows, build, failed = [], {}, []
    for name, repo in sorted(chosen.items()):
        t = time.perf_counter()
        try:
            rpg("index", repo)
        except RuntimeError as e:
            print(e, file=sys.stderr)
            failed.append(name)
            continue
        build[name] = round(time.perf_counter() - t, 1)
        for task in (t for t in tasks if t["repo"] == name):
            t = time.perf_counter()
            hits = rpg("search", repo, "-k", str(TOP_K), "--no-code", "--", task["query"])["results"]
            ms = (time.perf_counter() - t) * 1000
            ranks = [i for label in task["labels"]
                     for i in [next((n for n, h in enumerate(hits, 1) if covers(h, label)), None)]
                     if i is not None]
            rows.append({"repo": name, "language": task["language"], "category": task["category"],
                         "query": task["query"], "ndcg10": ndcg(ranks, len(task["labels"]), 10),
                         "ndcg5": ndcg(ranks, len(task["labels"]), 5), "hit": bool(ranks),
                         "ms": round(ms, 1)})
        print(f"{name:<22}{build[name]:>7.1f}s", file=sys.stderr)
    if not rows:
        return 1
    n = len(rows)
    ms = sorted(r["ms"] for r in rows)
    print(f"\n{len(build)} repositories, {n} queries")
    print(f"NDCG@10 {sum(r['ndcg10'] for r in rows) / n:.3f}   NDCG@5 {sum(r['ndcg5'] for r in rows) / n:.3f}"
          f"   any hit {sum(r['hit'] for r in rows) / n:.0%}")
    print(f"index {sum(build.values()):.1f}s   query p50 {ms[n // 2]:.0f} ms, p95 {ms[int(n * 0.95)]:.0f} ms")
    report(rows, "category", "category")
    report(rows, "language", "language")
    if failed:
        print(f"\nnot scored (fetch or index failed): {', '.join(failed)}")
    if args.json:
        args.json.write_text(json.dumps({"label": args.label, "rpg": args.rpg, "build_s": build,
                                         "failed": failed, "results": rows}, indent=1) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
