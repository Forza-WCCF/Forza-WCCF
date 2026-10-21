/* Self-test for mxhook's SLEEP fix: with MXHOOK_SLEEPFIX=1 the hook replaces this exe's Sleep import; Sleep(16) and
 * Sleep(1) must still wait at least ~their time, and Sleep(0) must return.  Run under Wine beside winmm.dll,
 * winmm_orig.dll and netguard.dll; prints PASS. */
#include <windows.h>
#include <stdio.h>

extern void *imp_Sleep __asm__("__imp__Sleep@4");   /* this exe's IAT slot for Sleep */

static double ms_of(DWORD n)
{
    LARGE_INTEGER f, a, b;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&a);
    Sleep(n);
    QueryPerformanceCounter(&b);
    return (double)(b.QuadPart - a.QuadPart) * 1000.0 / (double)f.QuadPart;
}

int main(void)
{
    double t16 = 1e9, t1 = 1e9, t;
    int i, hooked;
    SetEnvironmentVariableA("MXHOOK_SLEEPFIX", "1");
    if (!LoadLibraryA("winmm.dll")) { puts("FAIL: winmm.dll did not load"); return 1; }
    hooked = imp_Sleep != (void *)GetProcAddress(GetModuleHandleA("kernel32.dll"), "Sleep");
    for (i = 0; i < 450; i++) {           /* past both sample windows: the replacement is what runs */
        t = ms_of(16); if (i >= 400 && t < t16) t16 = t;
        t = ms_of(1);  if (i >= 400 && t < t1) t1 = t;
    }
    ms_of(0);
    printf("%s: min Sleep(16) %.2f ms, min Sleep(1) %.2f ms\n", hooked && t16 >= 15.0 && t1 >= 0.9 ? "PASS" : "FAIL", t16, t1);
    return !(hooked && t16 >= 15.0 && t1 >= 0.9);
}
