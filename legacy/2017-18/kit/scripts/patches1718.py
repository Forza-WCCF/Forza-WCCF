"""Seat 1's client_Release.exe changes for WCCF 2017-18 (SDDY), ported from the 2010-11 kit as each proves needed.
Each entry: (file offset, original bytes, new bytes, what). apply() checks every original byte first."""

PATCHES = [
    (0x1DB8A0, "6aff68", "b001c3", "keychip check passes (error 0949); VA 0x005DC4A0, 2010-11 kit's 0x004E0AE0"),
    (0x1CE5B1, "01", "00", "I/O board in JVS mode 0 (COM4); VA 0x005CF1B1 push 1 -> push 0, 2010-11 kit's 0x004D91C9"),
    (0x1F3820, "803d6e", "32c0c3", "resolution check passes (error 0910); VA 0x005F4420, 2010-11 kit's 0x004EE102"),
    (0x1F3720, "558bec", "32c0c3", "camera firmware check passes (error 0xC05); VA 0x005F4320, 2010-11 kit's 0x004EE000"),
    (0x1F37A0, "803d6e", "32c0c3", "camera board check passes (error 5801); VA 0x005F43A0, 2010-11 kit's 0x004EE080"),
    (0x376F06, "c74604b7140000", "90909090909090", "main board error 5303 not raised; 2010-11 kit's 0x00570C2A"),
    (0x376F3D, "c7460419000000", "90909090909090", "main board error 0025 not raised; 2010-11 kit's 0x00570C61"),
    (0x376F74, "c746041a000000", "90909090909090", "main board error 0026 not raised; 2010-11 kit's 0x00570C98"),
    (0xDD7E90, "01000000", "02000000", "smooth texture scaling; 2010-11 kit's 0x00988668"),
    (0xF93088, "2003", "a005", "screen width 800 -> 1440; 2010-11 kit's 0x009FBCE8"),
    (0xF9308C, "5802", "8403", "screen height 600 -> 900; 2010-11 kit's 0x009FBCEC"),
    # picture rectangle = the whole back buffer (17-18's "Satellite" layout drew 960x600 at x=32 in a 1024 frame);
    # VA 0x0040210E: dstH=[eax+0x30] dstW=[eax+0x2C] (back buffer), dstY=0, dstX=0 - the kit's 0x00401771 / 0x0040178F
    (0x00150E, "8b513c8b71388b79348b4930", "8b50308b702c33ff33c99090", "picture fills the back buffer"),
    # online play: the Aime screen (VA 0x0077A0D0) shows OFFLINE guest play unless byte 0x0683653F is set; that byte
    # needs ALL.Net "authorized" (VA 0x00C78770, alw library state == 1) besides control's own online flag. ALL.Net's
    # real auth runs link/gateway/traceroute probes of the host network, so it stays off (ALLNET_AUTH=0) and the
    # state query answers yes, as for the controller below.
    (0x877B70, "56e84aeeffff", "b801000000c3", "ALL.Net authorized (VA 0x00C78770 returns 1)"),
    # the seat's online test (VA 0x0040FDC7) also wants ALL.Net's billing phase (status struct +0xF8, filled by
    # VA 0x004D1430 from VA 0x00C78E00) in 16..24 but not 19 (offline); with ALL.Net off it stays 0. 20 = IDLE.
    (0x00F1C7, "8bb6f8000000", "be1400000090", "ALL.Net billing phase reads IDLE (20) in the online test"),
    # AiMeDB (aimelib, VA 0x00D841E0 getaddrinfo, port 22345): the original host -> this PC, served by _aimedb.py
    (0xE2C250, "61696d652e6e616f6d696e65742e6a7000", "6c6f63616c686f73740000000000000000",
     "AiMeDB host aime.naominet.jp -> localhost"),
]

# the projector (seat 0, game\\client_Release.exe) gets only the two ALL.Net answers: its corner status icon reads the
# same online flags as seat 1's (VA 0x009C4630 via 0x0683653D/F, set at VA 0x0040FDC7), so without them it shows OFFLINE
# although the control link is up.  No picture, board or keychip patch: it is the central monitor.
ONLINE_OFFSETS = (0x877B70, 0x00F1C7)
PROJECTOR_PATCHES = [p for p in PATCHES if p[0] in ONLINE_OFFSETS]


# the server (control_Release.exe in game\): one change - its ALL.Net library's "network up" check (FUN_004B73C0)
# answers yes, so it connects to the local game server stand-in (FORCE_ONLINE=1, GAME_SVR_IP=localhost).
# With ALLNET_AUTH=0 the library never comes up and control never tries the game server (control.c 0x440C80).
CONTROL_PATCHES = [
    (0x0B67C0, "56e8aaf2ffff", "b801000000c3", "network up (FUN_004B73C0 returns 1)"),
]


def apply(data, patches=None):
    d = bytearray(data)
    for off, old, new, what in (PATCHES if patches is None else patches):
        old, new = bytes.fromhex(old), bytes.fromhex(new)
        if bytes(d[off:off + len(old)]) != old:
            raise ValueError("0x%06X is not the expected original bytes (%s)" % (off, what))
        d[off:off + len(new)] = new
    return bytes(d)
