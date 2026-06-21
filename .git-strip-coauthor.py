import sys
for line in sys.stdin:
    if line.startswith("Co-authored-by: Cursor"):
        continue
    sys.stdout.write(line)
