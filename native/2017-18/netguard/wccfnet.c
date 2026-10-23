/* Imported by the pinned WCCF runtime copies instead of IPHLPAPI.DLL.
 * Unlike launcher injection, an ordinary DLL import also protects children.
 * The four calls below are stubs; the read-only IP helper APIs are forwarded (pragmas below). No adapter changes.
 * DllMain also loads the panel (forza_panel.dll) when FORZA_PANEL names it (end of file), and, only with
 * WCCF_TRACE_NET=1, logs the main program's send/recv (debug trace, below). */
#define IPHLPAPI_DLL_LINKAGE /* these functions are defined here, not imported */
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h> /* types only: the trace takes every winsock function from the game's own WS2_32, never links it */
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

/* Debug trace, WCCF_TRACE_NET=1 only: the main program's IAT entries for send (ordinal 19) and recv (16) from
 * WS2_32/WSOCK32 point here; each call is logged (time, program, socket, ports, length, first 64 bytes) and passed on
 * unchanged with its error code kept. The games import no WSASend/WSARecv. Without the variable nothing is patched. */
typedef int (WSAAPI *xfer_fn)(SOCKET, char *, int, int);
typedef int (WSAAPI *name_fn)(SOCKET, struct sockaddr *, int *);
static xfer_fn real_send, real_recv;
static name_fn ws_sockname, ws_peername;
static int (WSAAPI *ws_lasterror)(void);
static void (WSAAPI *ws_setlasterror)(int);
static char trace_role[64];

static int port_of(name_fn f, SOCKET s) {
    struct sockaddr_in a; int n = sizeof a;
    if (!f || f(s, (struct sockaddr *)&a, &n) != 0 || a.sin_family != AF_INET) return -1;
    return ((a.sin_port & 0xff) << 8) | (a.sin_port >> 8);  /* ntohs, without linking WS2_32 */
}

static void trace(const char *dir, SOCKET s, const char *buf, int ret, int err) {
    char text[700], hex[64 * 3 + 1], asc[64 + 1];
    SYSTEMTIME t;
    int n = ret > 64 ? 64 : ret > 0 ? ret : 0;
    for (int i = 0; i < n; i++) {
        unsigned char c = (unsigned char)buf[i];
        snprintf(hex + i * 3, 4, "%02x ", c);
        asc[i] = c >= 32 && c < 127 ? (char)c : '.';
    }
    hex[n * 3] = 0; asc[n] = 0;
    GetLocalTime(&t);
    snprintf(text, sizeof text, "%02u:%02u:%02u.%03u %s %s sock=%lu local=%d peer=%d len=%d err=%d | %s| %s",
             t.wHour, t.wMinute, t.wSecond, t.wMilliseconds, trace_role, dir, (unsigned long)s,
             port_of(ws_sockname, s), port_of(ws_peername, s), ret, ret < 0 ? err : 0, hex, asc);
    logline(text);
}

static int WSAAPI traced_send(SOCKET s, char *buf, int len, int flags) {
    int ret = real_send(s, buf, len, flags), err = ws_lasterror();
    if (ret != SOCKET_ERROR || err != WSAEWOULDBLOCK) trace("send", s, buf, ret, err);
    ws_setlasterror(err);
    return ret;
}

static int WSAAPI traced_recv(SOCKET s, char *buf, int len, int flags) {
    int ret = real_recv(s, buf, len, flags), err = ws_lasterror();
    if (ret != SOCKET_ERROR || err != WSAEWOULDBLOCK) trace("recv", s, buf, ret, err);
    ws_setlasterror(err);
    return ret;
}

/* The main program's import of `ordinal` (or of `name`) from a winsock DLL now calls `hook`; the old target goes to *real. */
static int patch_import(WORD ordinal, const char *name, void *hook, void **real) {
    BYTE *mod = (BYTE *)GetModuleHandleA(NULL);
    PIMAGE_NT_HEADERS nt = (PIMAGE_NT_HEADERS)(mod + ((PIMAGE_DOS_HEADER)mod)->e_lfanew);
    DWORD rva = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress;
    int done = 0;
    if (!rva) return 0;
    for (PIMAGE_IMPORT_DESCRIPTOR imp = (PIMAGE_IMPORT_DESCRIPTOR)(mod + rva); imp->Name; imp++) {
        const char *dll = (const char *)(mod + imp->Name);
        if ((_stricmp(dll, "ws2_32.dll") && _stricmp(dll, "wsock32.dll")) || !imp->OriginalFirstThunk) continue;
        PIMAGE_THUNK_DATA orig = (PIMAGE_THUNK_DATA)(mod + imp->OriginalFirstThunk);
        PIMAGE_THUNK_DATA first = (PIMAGE_THUNK_DATA)(mod + imp->FirstThunk);
        for (; orig->u1.AddressOfData; orig++, first++) {
            if (IMAGE_SNAP_BY_ORDINAL(orig->u1.Ordinal) ? IMAGE_ORDINAL(orig->u1.Ordinal) != ordinal
                : strcmp((const char *)((PIMAGE_IMPORT_BY_NAME)(mod + orig->u1.AddressOfData))->Name, name) != 0)
                continue;
            DWORD old;
            HMODULE owner = NULL;
            /* only a resolved entry (it points into a loaded DLL), never one the loader has yet to fill */
            if (!GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                    (LPCSTR)first->u1.Function, &owner) ||
                !VirtualProtect(&first->u1.Function, sizeof(void *), PAGE_READWRITE, &old)) continue;
            *real = (void *)first->u1.Function;
            first->u1.Function = (ULONG_PTR)hook;
            VirtualProtect(&first->u1.Function, sizeof(void *), old, &old);
            done++;
        }
    }
    return done;
}

static void trace_start(void) {
    char path[MAX_PATH], text[MAX_PATH + 80], *a, *b;
    HMODULE ws = GetModuleHandleA("ws2_32.dll");      /* the game imports it: loaded already */
    int s, r;
    if (!ws) { logline("trace: WS2_32 not loaded, no trace"); return; }
    ws_sockname = (name_fn)GetProcAddress(ws, "getsockname");
    ws_peername = (name_fn)GetProcAddress(ws, "getpeername");
    ws_lasterror = (int (WSAAPI *)(void))GetProcAddress(ws, "WSAGetLastError");
    ws_setlasterror = (void (WSAAPI *)(int))GetProcAddress(ws, "WSASetLastError");
    if (!ws_lasterror || !ws_setlasterror) { logline("trace: no WSAGetLastError, no trace"); return; }
    /* the role: the program's folder and name, game\control_Release.exe, seat1\client_Release.exe */
    path[0] = 0;
    GetModuleFileNameA(NULL, path, sizeof path);
    a = strrchr(path, '\\');
    if (a) { *a = 0; b = strrchr(path, '\\'); *a = '\\'; a = b ? b + 1 : path; } else a = path;
    snprintf(trace_role, sizeof trace_role, "%s", a);
    s = patch_import(19, "send", (void *)traced_send, (void **)&real_send);
    r = patch_import(16, "recv", (void *)traced_recv, (void **)&real_recv);
    snprintf(text, sizeof text, "trace: on in %s, send hooked %d, recv hooked %d", path, s, r);
    logline(text);
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
        {
            char on[4];
            if (GetEnvironmentVariableA("WCCF_TRACE_NET", on, sizeof on) == 1 && on[0] == '1') trace_start();
        }
    }
    return TRUE;
}
