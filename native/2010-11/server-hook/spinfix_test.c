/* Self-test for mxhook's SPIN fix: run with MXHOOK_SPINFIX=1 under Wine beside winmm.dll, winmm_orig.dll and
 * netguard.dll (WINEDLLOVERRIDES=winmm=n,b). Forced mode makes every timeGetTime call of this exe wait SPIN_MS (10)
 * first, so 20 calls take >= ~200 ms and the returned time still advances; prints PASS. */
#include <windows.h>
#include <stdio.h>

int main(void)
{
    LARGE_INTEGER f, a, b;
    DWORD t0, t1;
    double ms;
    int i;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&a);
    t0 = timeGetTime();
    for (i = 0; i < 19; i++)
        t1 = timeGetTime();
    QueryPerformanceCounter(&b);
    ms = (double)(b.QuadPart - a.QuadPart) * 1000.0 / (double)f.QuadPart;
    printf("%s: 20 timeGetTime calls took %.1f ms, timeGetTime advanced %lu ms\n",
           ms >= 180.0 && t1 - t0 >= 170 ? "PASS" : "FAIL", ms, t1 - t0);
    return !(ms >= 180.0 && t1 - t0 >= 170);
}
