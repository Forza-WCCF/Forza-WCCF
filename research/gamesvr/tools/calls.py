"""Collapse Hex-Rays text: print each sub_441E50 call as 'P=<param id> value=<expr> idx=<expr>' (param id = (handle offset - 32) / 4),
other lines verbatim (loops/assignments) so loop bases stay visible.  usage: calls.py decomp.txt"""
import re, sys
src = open(sys.argv[1], encoding="utf-8").read()
src = re.sub(r"\s*\n\s*", " ", src)  # one line
src = src.replace(";", ";\n").replace("{", "{\n").replace("}", "}\n")
for line in src.splitlines():
    line = line.strip()
    m = re.match(r"(?:n12 = )?sub_441E50\(\*\(_DWORD \*\)\((?:tbl\[1\]|v6\[1\]|\w+\[1\]|\*\(_DWORD \*\)\(\w+ \+ 12\)) \+ (\d+)\), [^,]+, [^,]+, (.*), ([^,]+), \d, \"SaveDataInterface", line)
    if m:
        off = int(m.group(1))
        print("P=%-5d value=%s idx=%s" % ((off - 32) // 4, m.group(2), m.group(3)))
    elif line and "SaveDataInterface" not in line:
        print("   " + line)
    elif line:
        print("?? " + line)
