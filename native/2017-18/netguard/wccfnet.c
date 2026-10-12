/* Imported by the pinned WCCF runtime copies instead of IPHLPAPI.DLL.
 * Unlike launcher injection, an ordinary DLL import also protects children.
 * Read-only IP helper APIs are forwarded in wccfnet.def. No adapter changes. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <iphlpapi.h>
#include <stdio.h>
#include <string.h>

static DWORD blocked(const char *name) {
    char path[MAX_PATH], line[128];
    HMODULE self = NULL;
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                          GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                          (LPCSTR)blocked, &self) &&
        GetModuleFileNameA(self, path, sizeof path) < sizeof path) {
        char *slash = strrchr(path, '\\');
        if (slash && (size_t)(slash - path) + sizeof "\\wccfnet.log" <= sizeof path) {
            strcpy(slash, "\\wccfnet.log");
            HANDLE log = CreateFileA(path, FILE_APPEND_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_ALWAYS, 0, NULL);
            if (log != INVALID_HANDLE_VALUE) {
                int n = snprintf(line, sizeof line, "pid=%lu blocked %s\r\n",
                                 (unsigned long)GetCurrentProcessId(), name);
                DWORD written;
                WriteFile(log, line, (DWORD)n, &written, NULL);
                CloseHandle(log);
            }
        }
    }
    return NO_ERROR;
}

DWORD WINAPI IpReleaseAddress(PIP_ADAPTER_INDEX_MAP unused) {
    (void)unused; return blocked("IpReleaseAddress");
}
DWORD WINAPI IpRenewAddress(PIP_ADAPTER_INDEX_MAP unused) {
    (void)unused; return blocked("IpRenewAddress");
}
DWORD WINAPI CreateIpForwardEntry(PMIB_IPFORWARDROW unused) {
    (void)unused; return blocked("CreateIpForwardEntry");
}
DWORD WINAPI SetIpForwardEntry(PMIB_IPFORWARDROW unused) {
    (void)unused; return blocked("SetIpForwardEntry");
}
