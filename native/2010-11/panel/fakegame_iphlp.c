// fakegame_iphlp.exe - fakegame.exe that also imports IpRenewAddress from IPHLPAPI.DLL (never calls it), for the
// 2017-18 guard's tests (crates/launcher): as built it is an unguarded program; with its import renamed to
// wccfnet.dll, as setup1718.py renames the game's, it is a guarded one.  Tests only, never shipped.
#define IPHLP
#include "fakegame.c"
