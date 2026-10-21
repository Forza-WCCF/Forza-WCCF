/* Imported by the pinned WCCF runtime copies instead of IPHLPAPI.DLL.
 * Unlike launcher injection, an ordinary DLL import also protects children.
 * The four calls below are stubs; the read-only IP helper APIs are forwarded (pragmas below). No adapter changes.
 * DllMain also loads the panel (forza_panel.dll) when FORZA_PANEL names it (end of file). */
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

/* One line in wccfnet.log, next to this DLL. */
static void logline(const char *text) {
    char path[MAX_PATH], line[1024];
    HMODULE self = NULL;
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                          GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                          (LPCSTR)logline, &self) &&
        GetModuleFileNameA(self, path, sizeof path) < sizeof path) {
        char *slash = strrchr(path, '\\');
        if (slash && (size_t)(slash - path) + sizeof "\\wccfnet.log" <= sizeof path) {
            strcpy(slash, "\\wccfnet.log");
            HANDLE log = CreateFileA(path, FILE_APPEND_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_ALWAYS, 0, NULL);
            if (log != INVALID_HANDLE_VALUE) {
                int n = snprintf(line, sizeof line, "pid=%lu %s\r\n",
                                 (unsigned long)GetCurrentProcessId(), text);
                DWORD written;
                if (n > 0) WriteFile(log, line, (DWORD)(n < (int)sizeof line ? n : (int)sizeof line - 1), &written, NULL);
                CloseHandle(log);
            }
        }
    }
}

static DWORD blocked(const char *name) {
    char text[64];
    snprintf(text, sizeof text, "blocked %s", name);
    logline(text);
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

/* The panel: when the Launcher sets FORZA_PANEL (seat 1 and the projector only), load that DLL into the game once its window
 * is up, 3 s later (the same trigger as the old inject.exe). A thread does it, never DllMain itself. */
static BOOL CALLBACK own_window(HWND w, LPARAM found) {
    DWORD pid = 0;
    GetWindowThreadProcessId(w, &pid);
    if (pid == GetCurrentProcessId() && IsWindowVisible(w)) { *(int *)found = 1; return FALSE; }
    return TRUE;
}

static DWORD WINAPI panel_loader(LPVOID arg) {
    WCHAR *dll = (WCHAR *)arg;
    char utf8[MAX_PATH * 3], text[MAX_PATH * 3 + 64];
    int seen = 0;
    for (int i = 0; i < 240 && !seen; i++) {            /* 120 s */
        EnumWindows(own_window, (LPARAM)&seen);
        if (!seen) Sleep(500);
    }
    if (!seen) { logline("panel: no window in 120 s, not loaded"); HeapFree(GetProcessHeap(), 0, dll); return 0; }
    Sleep(3000);
    if (!WideCharToMultiByte(CP_UTF8, 0, dll, -1, utf8, sizeof utf8, NULL, NULL)) utf8[0] = 0;
    if (LoadLibraryW(dll)) snprintf(text, sizeof text, "panel: loaded %s", utf8);
    else snprintf(text, sizeof text, "panel: did NOT load %s (error %lu)", utf8, GetLastError());
    logline(text);
    HeapFree(GetProcessHeap(), 0, dll);
    return 0;
}

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved) {
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) {
        DWORD n;
        DisableThreadLibraryCalls(inst);
        n = GetEnvironmentVariableW(L"FORZA_PANEL", NULL, 0);          /* length with the NUL; 0 = unset */
        if (n > 1) {
            WCHAR *dll = HeapAlloc(GetProcessHeap(), 0, n * sizeof(WCHAR));
            if (dll && GetEnvironmentVariableW(L"FORZA_PANEL", dll, n) == n - 1) {
                SetEnvironmentVariableW(L"FORZA_PANEL", NULL);     /* this process only, as inject.exe did: not its children */
                HANDLE t = CreateThread(NULL, 0, panel_loader, dll, 0, NULL);
                if (t) CloseHandle(t); else HeapFree(GetProcessHeap(), 0, dll);
            } else if (dll) HeapFree(GetProcessHeap(), 0, dll);
        }
    }
    return TRUE;
}
