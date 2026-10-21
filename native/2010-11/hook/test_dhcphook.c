/* Not shipped. test_dhcphook.exe <path to wccfk.dll>: this program imports IpReleaseAddress and IpRenewAddress. After the
 * hook loads, both import slots must point inside wccfk.dll. Nothing is called unless both do, so the real calls are never made. */
#include <windows.h>
#include <iphlpapi.h>
#include <stdio.h>

static int fails;
static void check(int ok, const char *what) { printf("%s: %s\n", ok ? "ok  " : "FAIL", what); if (!ok) fails++; }

int main(int argc, char **argv) {
    volatile void *slots[2];
    static const char *names[] = { "IpReleaseAddress", "IpRenewAddress" };
    DWORD (WINAPI *calls[2])(void *);
    int all = 1, i;
    HMODULE g;
    if (argc != 2) return 2;
    g = LoadLibraryA(argv[1]);
    check(g != NULL, "wccfk.dll loads");
    if (!g) return 1;
    slots[0] = (void *)IpReleaseAddress; slots[1] = (void *)IpRenewAddress;
    for (i = 0; i < 2; i++) {
        HMODULE owner = NULL;
        char what[96];
        int in = GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                     GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCSTR)slots[i], &owner) && owner == g;
        sprintf(what, "%s import points into wccfk.dll", names[i]);
        check(in, what);
        all &= in;
        calls[i] = (DWORD (WINAPI *)(void *))slots[i];
    }
    if (all)
        for (i = 0; i < 2; i++) {
            char what[96];
            sprintf(what, "%s(NULL) returns NO_ERROR", names[i]);
            check(calls[i](NULL) == NO_ERROR, what);
        }
    printf(fails ? "dhcp hook test: %d FAILED\n" : "dhcp hook test: all passed\n", fails);
    return fails != 0;
}
