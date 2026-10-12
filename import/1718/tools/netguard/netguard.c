/* netguard.dll - neutralize adapter-level DHCP APIs that Sega RingEdge binaries
 * call on the host, which release the real machine's DHCP lease and kill LAN/WAN
 * connectivity until the adapter is reset.
 *
 * Blocked: IpReleaseAddress, IpRenewAddress, CreateIpForwardEntry, SetIpForwardEntry
 * (imported by control_Release.exe and Launcher.exe).
 *
 * Loaded into the game process only; it never touches the host adapter. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <iphlpapi.h>
#include <stdio.h>
#include <string.h>

static FILE *g_log;

static void lg(const char *what) {
    if (!g_log) {
        char path[MAX_PATH];
        GetModuleFileNameA(NULL, path, MAX_PATH);
        char *slash = strrchr(path, '\\');
        if (slash) slash[1] = 0; else path[0] = 0;
        strncat(path, "netguard.log", MAX_PATH - strlen(path) - 1);
        g_log = fopen(path, "a");
    }
    if (g_log) { fprintf(g_log, "netguard: blocked %s\n", what); fflush(g_log); }
}

static DWORD WINAPI StubRelease(PIP_ADAPTER_INDEX_MAP a) { (void)a; lg("IpReleaseAddress"); return NO_ERROR; }
static DWORD WINAPI StubRenew(PIP_ADAPTER_INDEX_MAP a)   { (void)a; lg("IpRenewAddress");   return NO_ERROR; }
static DWORD WINAPI StubCreate(MIB_IPFORWARDROW *r)      { (void)r; lg("CreateIpForwardEntry"); return NO_ERROR; }
static DWORD WINAPI StubSet(MIB_IPFORWARDROW *r)         { (void)r; lg("SetIpForwardEntry");    return NO_ERROR; }

static void patch_iat(const char *dll, const char *func, void *repl) {
    HMODULE mod = GetModuleHandleA(NULL);
    if (!mod) return;
    PIMAGE_DOS_HEADER dos = (PIMAGE_DOS_HEADER)mod;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return;
    PIMAGE_NT_HEADERS nt = (PIMAGE_NT_HEADERS)((BYTE *)mod + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE) return;
    PIMAGE_IMPORT_DESCRIPTOR imp = (PIMAGE_IMPORT_DESCRIPTOR)
        ((BYTE *)mod + nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress);
    if (!imp) return;
    for (; imp->Name; imp++) {
        const char *name = (const char *)((BYTE *)mod + imp->Name);
        if (_stricmp(name, dll) != 0) continue;
        PIMAGE_THUNK_DATA first = (PIMAGE_THUNK_DATA)((BYTE *)mod + imp->FirstThunk);
        PIMAGE_THUNK_DATA orig  = (PIMAGE_THUNK_DATA)((BYTE *)mod + (imp->OriginalFirstThunk
                                     ? imp->OriginalFirstThunk : imp->FirstThunk));
        for (; orig->u1.AddressOfData; orig++, first++) {
            if (IMAGE_SNAP_BY_ORDINAL(orig->u1.Ordinal)) continue;
            PIMAGE_IMPORT_BY_NAME bn = (PIMAGE_IMPORT_BY_NAME)((BYTE *)mod + orig->u1.AddressOfData);
            if (strcmp((const char *)bn->Name, func) != 0) continue;
            DWORD old;
            VirtualProtect(&first->u1.Function, sizeof(void *), PAGE_READWRITE, &old);
            first->u1.Function = (ULONG_PTR)repl;
            VirtualProtect(&first->u1.Function, sizeof(void *), old, &old);
            lg(func);
        }
    }
}

BOOL WINAPI DllMain(HINSTANCE h, DWORD reason, LPVOID r) {
    (void)r;
    if (reason != DLL_PROCESS_ATTACH) return TRUE;
    DisableThreadLibraryCalls(h);
    patch_iat("IPHLPAPI.DLL", "IpReleaseAddress",    (void *)StubRelease);
    patch_iat("IPHLPAPI.DLL", "IpRenewAddress",      (void *)StubRenew);
    patch_iat("IPHLPAPI.DLL", "CreateIpForwardEntry",(void *)StubCreate);
    patch_iat("IPHLPAPI.DLL", "SetIpForwardEntry",   (void *)StubSet);
    return TRUE;
}
