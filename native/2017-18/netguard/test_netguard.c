/* Not shipped. test_netguard.exe <path to netguard.dll>: this program imports the four IP helper calls netguard.dll patches.
 * After the load the import slots must point inside netguard.dll. Nothing is called unless every slot
 * points into netguard.dll, so the real calls are never made. */
#include <windows.h>
#include <iphlpapi.h>
#include <stdio.h>

static int fails;
static void check(int ok, const char *what) { printf("%s: %s\n", ok ? "ok  " : "FAIL", what); if (!ok) fails++; }

int main(int argc, char **argv) {
    volatile void *slots[4];
    DWORD (WINAPI *calls[4])(void *);
    static const char *names[] = { "IpReleaseAddress", "IpRenewAddress", "CreateIpForwardEntry", "SetIpForwardEntry" };
    if (argc != 2) return 2;
    HMODULE g = LoadLibraryA(argv[1]);
    check(g != NULL, "netguard.dll loads");
    if (!g) return 1;
    slots[0] = (void *)IpReleaseAddress; slots[1] = (void *)IpRenewAddress;
    slots[2] = (void *)CreateIpForwardEntry; slots[3] = (void *)SetIpForwardEntry;
    int all = 1;
    for (int i = 0; i < 4; i++) {
        HMODULE owner = NULL;
        int in = GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                     GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCSTR)slots[i], &owner) && owner == g;
        char what[96];
        sprintf(what, "%s imports now point into netguard.dll", names[i]);
        check(in, what);
        all &= in;
        calls[i] = (DWORD (WINAPI *)(void *))slots[i];
    }
    if (all)
        for (int i = 0; i < 4; i++) {
            char what[96];
            sprintf(what, "%s(NULL) returns NO_ERROR", names[i]);
            check(calls[i](NULL) == NO_ERROR, what);
        }
    printf(fails ? "netguard test: %d FAILED\n" : "netguard test: all passed\n", fails);
    return fails != 0;
}
