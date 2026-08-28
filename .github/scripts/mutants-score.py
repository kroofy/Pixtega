#!/usr/bin/env python3
"""Score a cargo-mutants run against the project >= 90% threshold.

Reads the mutants.out/ directory that `cargo mutants` writes next to the
workspace root, computes

    score = caught / (caught + missed + timeout)

and:
  - writes mutants-score.json (machine-readable record: score, counts,
    threshold, git SHA, timestamp) for the workflow artifact,
  - appends a summary table to $GITHUB_STEP_SUMMARY when running in
    GitHub Actions,
  - exits non-zero when the score is below the threshold.

Unviable mutants (mutated code that does not compile) are excluded from the
denominator, matching the usual mutation-score definition. Timeouts count
against the score: a mutant that only dies to the harness timeout was not
killed by an assertion, and .cargo/mutants.toml already grants generous
bounded timeouts so spurious timeouts are unlikely.
"""

import datetime
import json
import os
import pathlib
import subprocess
import sys

THRESHOLD_PERCENT = 90.0
OUT_DIR = pathlib.Path("mutants.out")
SCORE_FILE = pathlib.Path("mutants-score.json")


def count_lines(name: str) -> int:
    """Count non-empty lines in a mutants.out/ result file.

    Args:
        name: Filename relative to mutants.out/ (e.g., "caught.txt").

    Returns:
        Number of non-empty lines, or 0 if the file does not exist.
    """
    path = OUT_DIR / name
    if not path.exists():
        return 0
    return sum(1 for line in path.read_text().splitlines() if line.strip())


def git_sha() -> str:
    """Get the current git commit SHA.

    Returns:
        Git commit SHA from GITHUB_SHA environment variable if running in
        GitHub Actions, otherwise from `git rev-parse HEAD`.
    """
    sha = os.environ.get("GITHUB_SHA")
    if sha:
        return sha
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True
    ).stdout.strip()


def main() -> int:
    """Score a cargo-mutants run against the project threshold.

    Reads mutants.out/ results, computes the mutation score, writes
    mutants-score.json, appends a summary to GITHUB_STEP_SUMMARY if in
    Actions, and exits non-zero if the score is below the threshold.

    Returns:
        0 if the score meets or exceeds the threshold, 1 otherwise.
    """
    if not OUT_DIR.is_dir():
        print(f"error: {OUT_DIR}/ not found; did cargo mutants run?", file=sys.stderr)
        return 1

    caught = count_lines("caught.txt")
    missed = count_lines("missed.txt")
    timeout = count_lines("timeout.txt")
    unviable = count_lines("unviable.txt")
    tested = caught + missed + timeout
    if tested == 0:
        print("error: zero tested mutants; refusing to report a score", file=sys.stderr)
        return 1

    score = 100.0 * caught / tested
    passed = score >= THRESHOLD_PERCENT

    record = {
        "score_percent": round(score, 2),
        "threshold_percent": THRESHOLD_PERCENT,
        "passed": passed,
        "caught": caught,
        "missed": missed,
        "timeout": timeout,
        "unviable_excluded": unviable,
        "tested": tested,
        "git_sha": git_sha(),
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }
    SCORE_FILE.write_text(json.dumps(record, indent=2) + "\n")

    verdict = "PASS" if passed else "FAIL"
    lines = [
        "## Mutation score",
        "",
        f"**{score:.2f}%** (threshold {THRESHOLD_PERCENT:.0f}%) — **{verdict}**",
        "",
        "| caught | missed | timeout | unviable (excluded) | tested |",
        "| --- | --- | --- | --- | --- |",
        f"| {caught} | {missed} | {timeout} | {unviable} | {tested} |",
        "",
        f"Commit `{record['git_sha']}` at {record['timestamp_utc']}.",
        "Full report (missed-mutant diffs, outcomes.json) in the",
        "`mutants-report` artifact of this run.",
        "",
    ]
    summary = "\n".join(lines)
    print(summary)

    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with open(summary_path, "a") as fh:
            fh.write(summary)

    if not passed:
        print(
            f"error: mutation score {score:.2f}% is below the project "
            f"threshold of {THRESHOLD_PERCENT:.0f}%; see mutants.out/missed.txt",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
