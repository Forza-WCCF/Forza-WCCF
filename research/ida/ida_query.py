# IDAPython, headless. Run on YOUR OWN COPY of a database (an .i64 is locked while open):
#   idat.exe -A "-SD:\...\ida_query.py D:\...\out.txt CMD [CMD ...]" -LD:\...\q.log D:\...\client_Release.exe.i64
# CMD (VA in hex):
#   decomp:0x5DC4A0          Hex-Rays pseudo-C of the function containing that address
#   xrefs:0x13F1350          every code reference to that address, with the containing function's name/start
#   str:TEXT                 every string containing TEXT (no spaces; use %20 for a space), with xrefs, and the
#                            pseudo-C of each referencing function
#   callers:0x5DC4A0         every caller of the function containing that address
#   disasm:0x5DC4A0:40       40 instructions from that address
# The database is NOT saved on exit (changes are discarded).
import ida_auto, ida_hexrays, ida_funcs, idautils, idc, ida_pro, ida_bytes, ida_nalt, idaapi

ida_auto.auto_wait()
out = open(idc.ARGV[1], "w", encoding="utf-8", errors="replace")
hr = ida_hexrays.init_hexrays_plugin()


def fstart(ea):
    f = ida_funcs.get_func(ea)
    return f.start_ea if f else None


def decomp(ea):
    s = fstart(ea)
    if s is None:
        return "// no function at %X" % ea
    try:
        return "//==== %08X %s\n%s" % (s, idc.get_func_name(s), ida_hexrays.decompile(s))
    except Exception as e:
        return "//==== %08X %s FAILED %s" % (s, idc.get_func_name(s), e)


for cmd in idc.ARGV[2:]:
    out.write("\n######## %s\n" % cmd)
    kind, _, arg = cmd.partition(":")
    try:
        if kind == "decomp":
            out.write(decomp(int(arg, 16)) + "\n")
        elif kind in ("xrefs", "callers"):
            ea = int(arg, 16)
            if kind == "callers":
                ea = fstart(ea) or ea
            for x in idautils.XrefsTo(ea):
                s = fstart(x.frm)
                out.write("%08X in %s (%s)\n" % (x.frm, idc.get_func_name(x.frm), "%08X" % s if s else "-"))
        elif kind == "str":
            text = arg.replace("%20", " ")
            seen = set()
            for s in idautils.Strings():
                v = str(s)
                if text in v:
                    out.write("string %08X %r\n" % (s.ea, v))
                    for x in idautils.XrefsTo(s.ea):
                        f = fstart(x.frm)
                        out.write("  ref %08X in %s\n" % (x.frm, idc.get_func_name(x.frm)))
                        if f is not None and f not in seen:
                            seen.add(f)
                            out.write(decomp(f) + "\n")
        elif kind == "disasm":
            a, _, n = arg.partition(":")
            ea = int(a, 16)
            for _ in range(int(n or "30")):
                out.write("%08X  %s\n" % (ea, idc.generate_disasm_line(ea, 0)))
                ea = idc.next_head(ea)
        else:
            out.write("unknown command\n")
    except Exception as e:
        out.write("ERROR %s\n" % e)
out.close()
ida_pro.qexit(0)
