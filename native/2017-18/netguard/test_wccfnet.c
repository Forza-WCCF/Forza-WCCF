/* Not shipped. test_wccfnet.exe <path to wccfnet.dll>: loads the guard from its own folder and calls its four stubs with
 * NULL (they ignore the argument). It never links or loads IPHLPAPI itself, and calls a stub only once its address is
 * proven to lie inside the guard's image, so a wrong export can never reach the real IP helper. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

static int fails;
static void check(int ok, const char *what) { printf("%s: %s\n", ok ? "ok  " : "FAIL", what); if (!ok) fails++; }

static int lines_in(const char *path) {
    char buf[4096]; DWORD n = 0; int lines = 0;
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL);
    if (f == INVALID_HANDLE_VALUE) return 0;
    if (ReadFile(f, buf, sizeof buf - 1, &n, NULL)) for (DWORD i = 0; i < n; i++) lines += buf[i] == '\n';
    CloseHandle(f);
    return lines;
}

int main(int argc, char **argv) {
    static const char *names[] = { "IpReleaseAddress", "IpRenewAddress", "CreateIpForwardEntry", "SetIpForwardEntry" };
    char log[MAX_PATH], *slash;
    if (argc != 2) return 2;
    check(GetModuleHandleA("iphlpapi.dll") == NULL, "IPHLPAPI is not loaded before the guard");
    HMODULE g = LoadLibraryA(argv[1]);
    check(g != NULL, "wccfnet.dll loads");
    if (!g) return 1;
    strcpy(log, argv[1]);
    slash = strrchr(log, '\\');
    strcpy(slash ? slash + 1 : log, "wccfnet.log");
    DeleteFileA(log);
    for (int i = 0; i < 4; i++) {
        FARPROC p = GetProcAddress(g, names[i]);
        HMODULE owner = NULL;
        int inside = p && GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                         GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCSTR)p, &owner) && owner == g;
        char what[96];
        sprintf(what, "%s resolves inside wccfnet.dll", names[i]);
        check(inside, what);
        if (!inside) continue;
        sprintf(what, "%s(NULL) returns NO_ERROR", names[i]);
        check(((DWORD (WINAPI *)(void *))p)(NULL) == NO_ERROR, what);
        sprintf(what, "wccfnet.log has %d line(s)", i + 1);
        check(lines_in(log) == i + 1, what);
    }
    check(GetModuleHandleA("iphlpapi.dll") == NULL, "IPHLPAPI is still not loaded");
    printf(fails ? "wccfnet test: %d FAILED\n" : "wccfnet test: all passed\n", fails);
    return fails != 0;
}
