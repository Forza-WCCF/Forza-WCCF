/* Stand-in ICMP.DLL for the 2017-18 game programs, which LoadLibraryA("ICMP.DLL") and ping 192.168.46.254 with 5 s
 * timeouts (about 45 s in the control, 20 s in each client). IcmpSendEcho returns 0 (no reply) at once and writes
 * nothing, the same result as the timeout. No packet leaves the PC. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#pragma comment(linker, "/export:IcmpCreateFile=_IcmpCreateFile@0")
#pragma comment(linker, "/export:IcmpCloseHandle=_IcmpCloseHandle@4")
#pragma comment(linker, "/export:IcmpSendEcho=_IcmpSendEcho@32")

HANDLE WINAPI IcmpCreateFile(void) { return (HANDLE)(INT_PTR)1; }
BOOL WINAPI IcmpCloseHandle(HANDLE h) { (void)h; return TRUE; }
DWORD WINAPI IcmpSendEcho(HANDLE h, DWORD dest, LPVOID data, WORD size, LPVOID opt, LPVOID reply, DWORD replysize,
                          DWORD timeout) {
    (void)h; (void)dest; (void)data; (void)size; (void)opt; (void)reply; (void)replysize; (void)timeout;
    return 0;
}
