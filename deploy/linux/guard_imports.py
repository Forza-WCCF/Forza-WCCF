#!/usr/bin/env python3
# Makes the guarded copy of the 2010-11 server's control: in a runtime folder's control_Release.exe the one import of
# IPHLPAPI.DLL becomes wccfnet.dll (the network guard; same length, so nothing else moves). The server runs only
# control and its match engines (which import no IP helper). Refuses a file that is not the original (the pinned hash
# of crates/forza/src/setup/e1011.rs) and skips one that is already guarded. Works on the runtime copy in place; never
# point it at your only copy.
#   python3 guard_imports.py RUNTIME_DIR
import hashlib, os, struct, sys

ORIGINAL = {
    "control_Release.exe": "23b9553096bb195fc991a1fc141d719df3c8ae88460959a88cdfc18863a1376f",
}
OLD, NEW = b"IPHLPAPI.DLL\0", b"wccfnet.dll\0\0"


def import_names(d):
    """file offsets of the import descriptors' DLL names (32-bit PE)"""
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    assert d[pe:pe + 4] == b"PE\0\0", "not a PE file"
    nsec, optsize = struct.unpack_from("<H", d, pe + 6)[0], struct.unpack_from("<H", d, pe + 20)[0]
    opt = pe + 24
    assert struct.unpack_from("<H", d, opt)[0] == 0x10B, "not a 32-bit program"
    imp_rva = struct.unpack_from("<I", d, opt + 104)[0]          # data directory 1: imports
    secs = [struct.unpack_from("<IIII", d, opt + optsize + 40 * i + 8) for i in range(nsec)]

    def off(rva):
        for vsize, va, rawsize, raw in secs:
            if va <= rva < va + max(vsize, rawsize):
                return rva - va + raw
        raise ValueError("RVA 0x%x is in no section" % rva)

    out, at = [], off(imp_rva)
    while True:
        name_rva = struct.unpack_from("<I", d, at + 12)[0]
        if not any(d[at:at + 20]):
            return out
        out.append(off(name_rva))
        at += 20


def guard(path):
    d = bytearray(open(path, "rb").read())
    names = import_names(d)
    if any(bytes(d[o:o + len(NEW)]).lower() == NEW.lower() for o in names):
        return "already guarded"
    digest = hashlib.sha256(d).hexdigest()
    if digest != ORIGINAL[os.path.basename(path)]:
        raise SystemExit("REFUSED: %s is not the original (sha256 %s)" % (path, digest))
    hits = [o for o in names if bytes(d[o:o + len(OLD)]).lower() == OLD.lower()]
    if len(hits) != 1:
        raise SystemExit("REFUSED: %s has %d IPHLPAPI.DLL imports, expected 1" % (path, len(hits)))
    d[hits[0]:hits[0] + len(OLD)] = NEW
    tmp = path + ".new"
    open(tmp, "wb").write(d)
    os.replace(tmp, path)
    return "guarded, sha256 " + hashlib.sha256(d).hexdigest()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: guard_imports.py RUNTIME_DIR")
    for name in ORIGINAL:
        print("%s: %s" % (name, guard(os.path.join(sys.argv[1], name))))
