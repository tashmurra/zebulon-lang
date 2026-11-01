#!/usr/bin/env python3
"""Find tested PR artifacts for an identical Git source tree, without rebuilding."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.request


ARTIFACTS = {"zebc-linux-x86_64", "zebc-windows-x86_64", "zebc-macos-universal"}
JOBS = {
    "Native build (ubuntu-24.04)",
    "Native build (windows-2025)",
    "Native build (macos-15)",
}


def find_run(get, repository, tree):
    """Only reuse complete PR builds; never follow another reuse-only run."""
    prefix = f"/repos/{repository}"
    runs = get(prefix + "/actions/workflows/ci.yml/runs?event=pull_request&status=success&per_page=100")
    for run in runs["workflow_runs"]:
        if (run["event"] != "pull_request" or run["status"] != "completed"
                or run["conclusion"] != "success"
                or run["repository"]["full_name"] != repository):
            continue
        hint = (run.get("head_commit") or {}).get("tree_id")
        if hint and hint != tree:
            continue
        # CI explicitly checks out head_sha, rather than the temporary PR merge
        # ref. The tree covers all tracked sources, toolchain pins and CI recipes.
        commit = get(prefix + "/git/commits/" + run["head_sha"])
        if commit["tree"]["sha"] != tree:
            continue
        run_path = prefix + f"/actions/runs/{run['id']}"
        jobs = get(run_path + "/jobs?filter=latest&per_page=100")["jobs"]
        native = [j for j in jobs if j["name"] in JOBS]
        if (len(native) != len(JOBS) or {j["name"] for j in native} != JOBS
                or any(j["conclusion"] != "success" for j in native)):
            continue
        artifacts = get(run_path + "/artifacts?per_page=100")["artifacts"]
        compilers = [a for a in artifacts if a["name"] in ARTIFACTS]
        if (len(compilers) != len(ARTIFACTS)
                or {a["name"] for a in compilers} != ARTIFACTS
                or any(a["expired"] or not re.fullmatch(r"sha256:[0-9a-f]{64}", a.get("digest") or "")
                       or a["workflow_run"]["head_sha"] != run["head_sha"]
                       or a["workflow_run"]["id"] != run["id"]
                       for a in compilers)):
            continue
        return {"run_id": run["id"], "run_url": run["html_url"],
                "source_commit": run["head_sha"], "source_tree": tree,
                "artifacts": compilers}
    return None


def github_get(path):
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request("https://api.github.com" + path, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def select(repository, commit, get=github_get):
    tree = subprocess.check_output(["git", "rev-parse", f"{commit}^{{tree}}"], text=True).strip()
    try:
        match = find_run(get, repository, tree)
        reason = "Matching tested PR artifacts found." if match else "No reusable PR artifacts found."
    except (OSError, KeyError, TypeError, ValueError) as error:
        # A lookup failure must never silently skip validation. Build normally.
        match = None
        reason = f"Artifact lookup unavailable ({type(error).__name__}); building normally."
    return {"target_commit": commit, "source_tree": tree, "reuse": match, "reason": reason}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"), required=False)
    parser.add_argument("--commit", default="HEAD")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--github-summary", type=Path)
    args = parser.parse_args()
    if not args.repository or len(args.repository.split("/")) != 2:
        parser.error("--repository owner/name or GITHUB_REPOSITORY is required")
    result = select(args.repository, args.commit)
    match = result["reuse"]
    if args.github_output:
        with args.github_output.open("a") as output:
            output.write(f"run_id={match['run_id'] if match else ''}\n")
    if args.github_summary:
        summary = result["reason"] + "\n"
        if match:
            summary += (f"\nReuse [PR build {match['run_id']}]({match['run_url']}) "
                        f"at `{match['source_commit']}`.\n\n"
                        f"Source tree: `{result['source_tree']}` (identical to this merge).\n\n"
                        "All three native jobs passed and compiler artifacts are available. "
                        "Download release binaries from the linked PR run; this merge does not rebuild or copy them.\n")
        with args.github_summary.open("a") as output:
            output.write(summary)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
