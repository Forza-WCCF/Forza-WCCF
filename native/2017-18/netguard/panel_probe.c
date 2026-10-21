/* Not shipped. Stands in for forza_panel.dll in test_panel.exe: loading it does nothing. */
#include <windows.h>
BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved) { (void)inst; (void)reason; (void)reserved; return TRUE; }
