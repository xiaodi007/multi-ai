#!/usr/bin/env python3
"""Offline, opt-in backfill of the model for old codex-cli implementation attempts (no model calls).

Older runs did not use Codex's JSON stream, so their metadata has no model. Codex keeps a
session log per exec (~/.codex/sessions/**/rollout-*.jsonl) whose first line records cwd and
start time and whose turn_context records the model. An attempt is backfilled only when
exactly one codex_exec session started in the same cwd within WINDOW seconds after the
attempt began and that session used exactly one model; everything else stays unknown.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import tempfile

WINDOW = 120
BACKUP = "metadata.before-model-backfill.json"


def timestamp(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def atomic(path, data):
    fd, temporary = tempfile.mkstemp(prefix=".backfill-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def sessions(root):
    found = []
    for path in root.glob("**/rollout-*.jsonl"):
        try:
            with path.open() as stream:
                meta = json.loads(stream.readline())["payload"]
                if meta.get("originator") != "codex_exec":
                    continue
                models = set()
                for line in stream:
                    if '"turn_context"' not in line:
                        continue
                    event = json.loads(line)
                    model = (event.get("payload") or {}).get("model")
                    if event.get("type") == "turn_context" and model:
                        models.add(model)
        except (OSError, ValueError, KeyError):
            continue
        found.append((timestamp(meta["timestamp"]), meta.get("cwd"), models, path))
    return found


def attempts(state):
    for run in sorted((state / "runs").iterdir()):
        try:
            manifest = json.loads((run / "manifest.json").read_text())
        except (OSError, ValueError):
            continue
        if manifest.get("command") != "implement":
            continue
        progress = Path(manifest.get("outputs", {}).get("progress") or "")
        if not progress.is_dir():
            continue
        for attempt in sorted(progress.iterdir()):
            metadata = attempt / "metadata.json"
            if metadata.is_symlink() or not metadata.is_file():
                continue
            try:
                value = json.loads(metadata.read_text())
            except ValueError:
                continue
            if value.get("route") == "codex-cli" and not value.get("model"):
                yield metadata, value, value.get("started_at") or manifest.get("started_at")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0] + " Dry-run unless --apply.")
    parser.add_argument("--state-dir", type=Path, default=Path.home() / ".local/state/multi-ai")
    parser.add_argument("--codex-sessions", type=Path, default=Path.home() / ".codex/sessions")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    logs = sessions(args.codex_sessions)
    counts = {"unknown": 0, "backfilled": 0, "no_match": 0, "ambiguous": 0, "model_unclear": 0}
    for metadata, value, started in attempts(args.state_dir):
        counts["unknown"] += 1
        cwd = value.get("cwd")
        if not cwd or not started:
            counts["no_match"] += 1
            continue
        start = timestamp(started)
        hits = [s for s in logs if s[1] == cwd and 0 <= (s[0] - start).total_seconds() <= WINDOW]
        if not hits:
            counts["no_match"] += 1
            continue
        if len(hits) > 1:
            counts["ambiguous"] += 1
            continue
        _, _, models, path = hits[0]
        if len(models) != 1:
            counts["model_unclear"] += 1
            continue
        model = next(iter(models))
        counts["backfilled"] += 1
        print(f"{metadata.parent.name}\t{model}\t{path.name}")
        if not args.apply:
            continue
        backup = metadata.parent / BACKUP
        if backup.exists():
            raise SystemExit(f"existing backup, inspect before retrying: {backup}")
        original = metadata.read_bytes()
        atomic(backup, original)
        value.update({
            "model": model,
            "model_source": "codex-session-log",
            "model_identity_known": True,
            "model_backfill": {"session_log": str(path), "matched_by": f"cwd+start<={WINDOW}s"},
        })
        atomic(metadata, (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode())
    print(json.dumps(counts, ensure_ascii=False))
    if not args.apply:
        print("dry-run: rerun with --apply to write; originals are kept as " + BACKUP)


if __name__ == "__main__":
    main()
