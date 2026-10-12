/* Imported by the pinned WCCF runtime copies instead of IPHLPAPI.DLL.
 * Unlike launcher injection, an ordinary DLL import also protects children.
 * The four calls below are stubs; the read-only IP helper APIs are forwarded (pragmas below). No adapter changes. */
#define IPHLPAPI_DLL_LINKAGE /* these functions are defined here, not imported */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <iphlpapi.h>
#include <stdio.h>
#include <string.h>

/* The four stubs keep their undecorated names; the rest is forwarded to the real IPHLPAPI . */
#pragma comment(linker, "/export:IpReleaseAddress=_IpReleaseAddress@4")
#pragma comment(linker, "/export:IpRenewAddress=_IpRenewAddress@4")
#pragma comment(linker, "/export:CreateIpForwardEntry=_CreateIpForwardEntry@4")
#pragma comment(linker, "/export:SetIpForwardEntry=_SetIpForwardEntry@4")
#pragma comment(linker, "/export:GetInterfaceInfo=iphlpapi.GetInterfaceInfo")
#pragma comment(linker, "/export:GetIpAddrTable=iphlpapi.GetIpAddrTable")
#pragma comment(linker, "/export:GetNetworkParams=iphlpapi.GetNetworkParams")
#pragma comment(linker, "/export:GetAdaptersInfo=iphlpapi.GetAdaptersInfo")
#pragma comment(linker, "/export:GetIpForwardTable=iphlpapi.GetIpForwardTable")
#pragma comment(linker, "/export:GetAdaptersAddresses=iphlpapi.GetAdaptersAddresses")

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
