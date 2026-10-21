/* Not shipped. test_panel.exe <wccfnet.dll> <panel_probe.dll> <on|off>: loads the guard, opens a visible window and pumps
 * messages for 6 s. "on" (the Launcher sets FORZA_PANEL to the probe) wants the probe loaded by then, "off" (no variable)
 * wants it never loaded. The guard's loader waits 3 s after the window shows. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    int want, loaded;
    DWORD end;
    MSG m;
    if (argc != 4) return 2;
    want = strcmp(argv[3], "on") == 0;
    if (!LoadLibraryA(argv[1])) { puts("FAIL: wccfnet.dll loads"); return 1; }
    HWND w = CreateWindowA("STATIC", "test_panel", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 0, 0, 200, 100, NULL, NULL, NULL, NULL);
    if (!w) { puts("FAIL: window"); return 1; }
    for (end = GetTickCount() + 6000; GetTickCount() < end; Sleep(50))
        while (PeekMessageA(&m, NULL, 0, 0, PM_REMOVE)) DispatchMessageA(&m);
    loaded = GetModuleHandleA("panel_probe.dll") != NULL;
    printf("%s: panel probe %s with FORZA_PANEL %s\n", loaded == want ? "ok  " : "FAIL",
           loaded ? "loaded" : "not loaded", want ? "set" : "unset");
    return loaded != want;
}
