"""Round 26: gzip every trace_*.json in a run dir to trace_*.json.gz (level 9) and print the sizes.

python -I gz26.py <run dir>
"""
import glob, gzip, os, shutil, sys

for f in sorted(glob.glob(os.path.join(sys.argv[1], "trace_*.json"))):
    with open(f, "rb") as a, gzip.open(f + ".gz", "wb", compresslevel=9) as b:
        shutil.copyfileobj(a, b)
    print(f"{os.path.basename(f)}: {os.path.getsize(f) / 2**20:.1f} MB -> {os.path.getsize(f + '.gz') / 2**20:.1f} MB")
