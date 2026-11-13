#!/usr/bin/env python3
"""Preview/prune rebuildable Rust caches; keep dependencies and current binaries."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


def size(path):
    if path.is_symlink():
        return 0
    if path.is_file():
        return path.stat().st_size
    return sum(p.stat().st_size for root, _, files in os.walk(path, followlinks=False)
               for p in (Path(root)/name for name in files) if not p.is_symlink())


def plan(target, max_bytes, old_days, now=None):
    target = Path(target).resolve()
    if not (target/'.rustc_info.json').is_file():
        raise ValueError('Not a recognized Cargo target directory; no files selected')
    incremental = target/'debug/incremental'
    entries = []
    if incremental.is_dir() and not incremental.is_symlink():
        entries = [(p, size(p)) for p in incremental.iterdir()
                   if p.is_dir() and not p.is_symlink()
                   and re.fullmatch(r'[A-Za-z0-9_]+-[a-z0-9]{8,64}', p.name)]
    entries.sort(key=lambda item: item[0].stat().st_mtime)
    total = sum(n for _, n in entries)
    selected = []
    remaining = total
    # Retain the hottest entry even when one crate alone exceeds the target.
    for path, count in entries[:-1]:
        if remaining <= max_bytes:
            break
        selected.append(path)
        remaining -= count
    debug = target/'debug'
    protected = {(p.stat().st_dev, p.stat().st_ino) for p in debug.iterdir()
                 if p.is_file() and os.access(p, os.X_OK)} if debug.is_dir() else set()
    deps = debug/'deps'
    cutoff = (time.time() if now is None else now)-old_days*86400
    if deps.is_dir() and not deps.is_symlink():
        for p in deps.iterdir():
            if (p.is_file() and not p.is_symlink()
                    and re.fullmatch(r'[A-Za-z0-9_]+-[0-9a-f]{16}', p.name)
                    and p.with_suffix('.d').is_file() and os.access(p, os.X_OK)):
                s = p.stat()
                if s.st_mtime < cutoff and (s.st_dev,s.st_ino) not in protected:
                    selected.append(p)
    return selected, {'incremental_before_bytes': total,
                      'incremental_after_bytes': remaining,
                      'selected_bytes': sum(size(p) for p in selected),
                      'selected_entries': len(selected)}


def apply(target, paths):
    target = Path(target).resolve()
    if not (target/'.rustc_info.json').is_file():
        raise ValueError('Cargo marker missing')
    allowed = {target/'debug/incremental',target/'debug/deps'}
    for p in paths:
        p = Path(p)
        if p.is_symlink() or p.resolve().parent not in allowed:
            raise ValueError('Unsafe cache path')
    for p in paths:
        if p.is_dir():
            shutil.rmtree(p)
        elif p.is_file():
            p.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target',type=Path)
    parser.add_argument('--max-gib',type=float,default=8)
    parser.add_argument('--old-tests-days',type=float,default=7)
    parser.add_argument('--apply',action='store_true',help='default is a read-only preview')
    args = parser.parse_args()
    if args.max_gib <= 0 or args.old_tests_days < 1:
        parser.error('Use a positive cache budget and at least one day for old test binaries')
    root = Path(__file__).resolve().parents[1]
    common = Path(subprocess.check_output(['git','-C',str(root),'rev-parse','--path-format=absolute','--git-common-dir'],text=True).strip())
    target = args.target or Path(os.environ.get('CARGO_TARGET_DIR',str(common.parent/'target')))
    paths, summary = plan(target,args.max_gib*2**30,args.old_tests_days)
    print(json.dumps({'mode':'apply' if args.apply else 'preview','target':str(target),**summary}),flush=True)
    if args.apply:
        commands = subprocess.check_output(['ps','-axo','comm='],text=True).splitlines()
        if any(Path(line.strip()).name in {'cargo','rustc'} or str(target.resolve()/'debug/deps')+'/' in line for line in commands):
            raise RuntimeError('Compilation is active; retry after it finishes')
        apply(target,paths)
        print(json.dumps({'removed_entries':len(paths),'removed_bytes':summary['selected_bytes']}),flush=True)


if __name__=='__main__':
    main()
