#!/usr/bin/env python3
"""Offline, opt-in repair of legacy snapshot-only implementation records (no model calls)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import uuid


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"not a regular file: {path}")
    return json.loads(path.read_bytes())


def atomic(path, data):
    fd, temporary = tempfile.mkstemp(prefix=".reclassify-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def prepare(state, status_path):
    status = load(status_path)
    if status.get("legacy_reclassification"):
        return None
    response = status.get("response", {})
    if (status.get("status") != "boundary_violation" or "attribution" in status
            or response.get("status") != "ok" or response.get("exitCode") != 0
            or response.get("complete") is not True or status.get("summary_published") is not True
            or status.get("git_after_error") or status.get("git_head_changed")):
        raise ValueError("requires legacy snapshot-only boundary violation, complete response exit 0, published summary and intact Git checks")
    run = status["run_id"]
    uuid.UUID(run)
    attempt = status["attempt_id"]
    if not attempt or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-" for c in attempt):
        raise ValueError("invalid attempt ID")
    manifest_path = state / "runs" / run / "manifest.json"
    manifest = load(manifest_path)
    if manifest.get("run_id") != run or manifest.get("command") != "implement":
        raise ValueError("manifest run/command mismatch")
    artifacts = Path(status["artifacts"])
    expected = Path(manifest["outputs"]["progress"]) / attempt
    if artifacts.resolve() != expected.resolve():
        raise ValueError("attempt artifact path mismatch")
    metadata_path = artifacts / "metadata.json"
    metadata = load(metadata_path)
    if metadata.get("run_id") != run or metadata.get("attempt_id", attempt) != attempt:
        raise ValueError("metadata identity mismatch")
    summary_path = Path(manifest["outputs"]["out"])
    if Path(str(summary_path) + ".status.json").resolve() != status_path.resolve():
        raise ValueError("summary/status identity mismatch")
    if summary_path.is_symlink() or (artifacts / "last-message.txt").is_symlink():
        raise ValueError("symlink report refused")
    summary = summary_path.read_bytes()
    last = (artifacts / "last-message.txt").read_bytes().strip()
    if not last or not summary.rstrip().endswith(last):
        raise ValueError("published summary does not contain the complete last message as its suffix")
    if (artifacts / "legacy-before-reclassification.json").exists():
        raise ValueError("existing repair backup; inspect interrupted repair before retrying")
    if (artifacts / "report.md").exists() and (artifacts / "report.md").read_bytes() != summary:
        raise ValueError("unknown existing report artifact; refusing overwrite")
    reports = manifest.setdefault("reports", [])
    if any(r.get("attempt_id") == attempt for r in reports):
        raise ValueError("already registered report; refusing to replace report identity")
    provenance = {"version": 1, "status_sha256": digest(status_path.read_bytes()),
                  "summary_sha256": digest(summary), "last_message_sha256": digest(last),
                  "criterion": "legacy snapshot-only; response ok/complete/exit=0; published summary ends with intact nonempty last-message; Git checks intact; host explicitly reclassifies un-attributed changes as warnings"}
    original = json.loads(json.dumps({"status": status, "metadata": metadata, "manifest": manifest}))
    # Do not invent model identity or a final summary: the old warning header is preserved.
    changed = dict(status, status="ok", complete=True, exitCode=0, task_type=status.get("task_type") or "implement",
                   execution_status="ok", attribution="unavailable", summary_kind="diagnostic",
                   attributed_boundary_count=0, boundary_violation_count=0, boundary_violations=[],
                   boundary_warnings={"legacy_unattributed": {"count": len(status.get("boundary_violations", [])),
                       "samples": status.get("boundary_violations", [])[:20]}}, legacy_reclassification=provenance)
    changed.pop("error", None)
    metadata.update(changed)
    reports.append({"attempt_id": attempt, "run_id": run, "path": str(artifacts / "report.md"),
                    "report_sha256": digest(summary), "result": changed, "cached": False, "model_calls": 1})
    manifest.update(current_status=changed, task_type=manifest.get("task_type") or "implement", status="complete", exitCode=0)
    return [(artifacts / "legacy-before-reclassification.json", encoded(original)),
            (artifacts / "report.md", summary), (metadata_path, encoded(metadata)),
            (status_path, encoded(changed)), (manifest_path, encoded(manifest))]


def main():
    parser = argparse.ArgumentParser(description=__doc__ + " Stop writers first; dry-run unless --apply. Requires Python 3.")
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--status", type=Path, action="append", required=True, help="repeat for each legacy summary.md.status.json")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    plans = []
    seen = set()
    # Preflight every selected record before any mutation. Each file is atomic; interrupted
    # multi-file publication can be restored from legacy-before-reclassification.json.
    for path in args.status:
        path = path.resolve()
        if path in seen:
            raise ValueError("duplicate status path")
        seen.add(path)
        plan = prepare(args.state_dir.resolve(), path)
        if plan:
            plans.append(plan)
    targets = [path for plan in plans for path, _ in plan]
    if len(set(targets)) != len(targets):
        raise ValueError("overlapping repair targets; repair one attempt per run at a time")
    if args.apply:
        for plan in plans:
            for path, data in plan:
                if path.name == "legacy-before-reclassification.json" and path.exists():
                    raise ValueError("existing repair backup; inspect interrupted repair before retrying")
                atomic(path, data)
    print(json.dumps({"eligible": len(plans), "applied": args.apply}))


if __name__ == "__main__":
    main()
