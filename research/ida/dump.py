# IDAPython (headless): wait for auto-analysis, then write every function's Hex-Rays pseudo-C to ARGV[1]
# (or only analyse and save the database when ARGV[1] == "-").
import ida_auto, ida_hexrays, idautils, idc, ida_pro
ida_auto.auto_wait()
out_path = idc.ARGV[1] if len(idc.ARGV) > 1 else "-"
if out_path != "-" and ida_hexrays.init_hexrays_plugin():
    with open(out_path, "w", encoding="utf-8", errors="replace") as out:
        for ea in idautils.Functions():
            try:
                out.write("//==== %08X %s\n%s\n" % (ea, idc.get_func_name(ea), ida_hexrays.decompile(ea)))
            except Exception as e:
                out.write("//==== %08X %s FAILED %s\n" % (ea, idc.get_func_name(ea), e))
ida_pro.qexit(0)
