/* Not shipped. test_icmp.exe <path to ICMP.DLL>: loads the stand-in, checks its three exports, and that IcmpSendEcho
 * returns 0 at once (well under the game's 5 s timeout) and leaves the reply buffer untouched. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

static int fails;
static void check(int ok, const char *what) { printf("%s: %s\n", ok ? "ok  " : "FAIL", what); if (!ok) fails++; }

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    HMODULE m = LoadLibraryA(argv[1]);
    check(m != NULL, "ICMP.DLL loads");
    if (!m) return 1;
    HANDLE (WINAPI *create)(void) = (HANDLE (WINAPI *)(void))GetProcAddress(m, "IcmpCreateFile");
    BOOL (WINAPI *close)(HANDLE) = (BOOL (WINAPI *)(HANDLE))GetProcAddress(m, "IcmpCloseHandle");
    DWORD (WINAPI *echo)(HANDLE, DWORD, LPVOID, WORD, LPVOID, LPVOID, DWORD, DWORD) =
        (DWORD (WINAPI *)(HANDLE, DWORD, LPVOID, WORD, LPVOID, LPVOID, DWORD, DWORD))GetProcAddress(m, "IcmpSendEcho");
    check(create && close && echo, "exports IcmpCreateFile, IcmpCloseHandle, IcmpSendEcho");
    if (!(create && close && echo)) return 1;
    HANDLE h = create();
    check(h != INVALID_HANDLE_VALUE && h != NULL, "IcmpCreateFile returns a usable handle");
    unsigned char reply[64], blank[64];
    memset(reply, 0xA5, sizeof reply);
    memcpy(blank, reply, sizeof reply);
    DWORD t0 = GetTickCount();
    DWORD n = echo(h, 0xFE2EA8C0, "x", 1, NULL, reply, sizeof reply, 5000);
    DWORD ms = GetTickCount() - t0;
    check(n == 0, "IcmpSendEcho returns 0");
    check(ms < 500, "IcmpSendEcho returns at once (timeout argument 5000 ms)");
    check(memcmp(reply, blank, sizeof reply) == 0, "reply buffer untouched");
    check(close(h), "IcmpCloseHandle succeeds");
    printf(fails ? "icmp test: %d FAILED\n" : "icmp test: all passed\n", fails);
    return fails != 0;
}
