#!/usr/bin/env python3
"""Offline regression tests for the decision to skip native CI builds."""
import copy
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

import ci_reuse


REPO = "example/compiler"
TREE = "a" * 40
HEAD = "b" * 40


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.run = {"id": 42, "event": "pull_request", "status": "completed",
                    "conclusion": "success", "repository": {"full_name": REPO},
                    "head_sha": HEAD, "html_url": "https://github.com/example/compiler/actions/runs/42"}
        self.tree = TREE
        self.runs = [self.run]
        self.jobs = [{"name": name, "conclusion": "success"} for name in sorted(ci_reuse.JOBS)]
        self.artifacts = [{"id": index, "name": name, "expired": False,
                           "digest": "sha256:" + "c" * 64,
                           "workflow_run": {"id": 42, "head_sha": HEAD}}
                          for index, name in enumerate(sorted(ci_reuse.ARTIFACTS))]

    def get(self, path):
        self.assertTrue(path.startswith(f"/repos/{REPO}/"))
        if "/workflows/ci.yml/runs?" in path:
            self.assertIn("event=pull_request", path)
            return {"workflow_runs": self.runs}
        if "/git/commits/" in path:
            return {"tree": {"sha": self.tree}}
        if "/jobs?" in path:
            self.assertIn("filter=latest", path)
            return {"jobs": self.jobs}
        if "/artifacts?" in path:
            return {"artifacts": self.artifacts}
        self.fail(f"Unexpected API request: {path}")

    def find(self):
        return ci_reuse.find_run(self.get, REPO, TREE)

    def test_same_tree_reuses_different_commit_with_provenance(self):
        match = self.find()
        self.assertEqual(match["source_tree"], TREE)
        self.assertEqual(match["source_commit"], HEAD)
        self.assertEqual(match["run_id"], 42)
        self.assertEqual(len(match["artifacts"]), 3)

    def test_changed_merge_tree_builds(self):
        self.tree = "d" * 40
        self.assertIsNone(self.find())

    def test_non_pr_failed_pending_or_other_repository_cannot_supply_artifacts(self):
        for field, value in [("event", "push"), ("event", "workflow_dispatch"),
                             ("status", "in_progress"), ("conclusion", "failure"),
                             ("conclusion", "cancelled"), ("repository", {"full_name": "other/repo"})]:
            with self.subTest(field=field, value=value):
                run = copy.deepcopy(self.run)
                run[field] = value
                self.runs = [run]
                self.assertIsNone(self.find())

    def test_missing_duplicate_or_unsuccessful_platform_builds(self):
        original = copy.deepcopy(self.jobs)
        variants = [original[:-1], original + [original[0]]]
        for conclusion in ["skipped", "failure", "cancelled", None]:
            jobs = copy.deepcopy(original)
            jobs[0]["conclusion"] = conclusion
            variants.append(jobs)
        for jobs in variants:
            with self.subTest(jobs=jobs):
                self.jobs = jobs
                self.assertIsNone(self.find())

    def test_incomplete_expired_or_wrong_origin_artifacts_build(self):
        original = copy.deepcopy(self.artifacts)
        variants = [original[:-1], original + [original[0]]]
        for field, value in [("expired", True), ("digest", None), ("digest", "sha256:bad"),
                             ("workflow_run", {"id": 42, "head_sha": "d" * 40}),
                             ("workflow_run", {"id": 43, "head_sha": HEAD})]:
            artifacts = copy.deepcopy(original)
            artifacts[0][field] = value
            variants.append(artifacts)
        for artifacts in variants:
            with self.subTest(artifacts=artifacts):
                self.artifacts = artifacts
                self.assertIsNone(self.find())

    def test_no_runs_builds(self):
        self.runs = []
        self.assertIsNone(self.find())

    def test_ignores_unrelated_jobs_and_artifacts(self):
        self.jobs.append({"name": "Select tested artifacts", "conclusion": "success"})
        self.artifacts.append({"name": "unrelated report"})
        self.assertIsNotNone(self.find())

    def test_tree_hint_avoids_unnecessary_requests(self):
        self.run["head_commit"] = {"tree_id": "d" * 40}
        calls = []

        def get(path):
            calls.append(path)
            return self.get(path)

        self.assertIsNone(ci_reuse.find_run(get, REPO, TREE))
        self.assertEqual(len(calls), 1)

    def test_hint_is_checked_against_commit(self):
        self.run["head_commit"] = {"tree_id": TREE}
        self.tree = "d" * 40
        self.assertIsNone(self.find())

    def test_finds_matching_older_run(self):
        unrelated = copy.deepcopy(self.run)
        unrelated["head_commit"] = {"tree_id": "d" * 40}
        self.runs = [unrelated, self.run]
        self.assertEqual(self.find()["run_id"], 42)

    def test_lookup_failures_build_instead_of_skipping_validation(self):
        for error in [urllib.error.HTTPError("url", 403, "forbidden", {}, None),
                      urllib.error.URLError("offline"), TimeoutError(),
                      KeyError("invalid response"), TypeError("invalid response"),
                      ValueError("invalid JSON")]:
            with self.subTest(error=error), patch("ci_reuse.subprocess.check_output", return_value=TREE + "\n"):
                def get(path):
                    raise error
                result = ci_reuse.select(REPO, "HEAD", get)
                self.assertIsNone(result["reuse"])
                self.assertIn("building normally", result["reason"])

    def test_workflow_outputs_and_summary_control_reuse(self):
        for match in [self.find(), None]:
            with self.subTest(reuse=bool(match)), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "outputs"
                summary = Path(directory) / "summary"
                result = {"source_tree": TREE, "reuse": match, "reason": "selection result"}
                args = ["ci_reuse.py", "--repository", REPO, "--github-output", str(output),
                        "--github-summary", str(summary)]
                with patch("sys.argv", args), patch("ci_reuse.select", return_value=result), contextlib.redirect_stdout(io.StringIO()):
                    ci_reuse.main()
                self.assertEqual(output.read_text(), "run_id=42\n" if match else "run_id=\n")
                if match:
                    self.assertIn(match["run_url"], summary.read_text())
                    self.assertIn(HEAD, summary.read_text())
                    self.assertIn(TREE, summary.read_text())
                else:
                    self.assertNotIn("Reuse [PR build", summary.read_text())


if __name__ == "__main__":
    unittest.main()
