"""Build the directory a directory-surface demonstration works in.

Fifty invoices and the twelve month folders they belong in. The work is the work anybody has: a pile
of files, and a rule that says where each one goes. For a person it is a minute of dragging; for an
agent that decides every step with a model it is fifty decisions, and that is the number the
comparison exists to show.

The names carry the rule — invoice-2026-03-0147.pdf belongs in 2026-03 — because a rule that has to be
looked up somewhere else is a different (and bigger) task.
"""
import shutil
import sys
from pathlib import Path

COUNT = 50


def build(root: Path, fresh=True):
    if fresh and root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True, exist_ok=True)
    for month in range(1, 13):
        (root / f"2026-{month:02d}").mkdir(exist_ok=True)
    for i in range(1, COUNT + 1):
        # Spread the invoices over the year, and vary the padding so the names are not one template
        # repeated: a model that matches on the digits alone will be wrong somewhere.
        month = ((i * 7) % 12) + 1
        number = 100 + i * 13
        name = f"invoice-2026-{month:02d}-{number:04d}.pdf"
        (root / name).write_bytes(b"%PDF-1.4\n" + b"invoice " + name.encode() + b"\n")
    files = [p for p in root.iterdir() if p.is_file()]
    return len(files), len([p for p in root.iterdir() if p.is_dir()])


if __name__ == "__main__":
    target = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/invoices")
    files, dirs = build(target)
    print(f"  {target}: {files} files, {dirs} folders")
