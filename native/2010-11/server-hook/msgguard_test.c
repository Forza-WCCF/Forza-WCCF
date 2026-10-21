/* Self-test for mxhook's MSGPARSE guard: control's FUN_00412140 -> FUN_00412860 call as the game makes it. The caller
 * keeps len at esp+0xC, off at +0x10, buf at +0x28, pushes &off, buf, &len, ebx = this, esi = seat; the victim pushes
 * ebx/ebp/esi/edi, overwrites its &len slot as FUN_00412860 does at 0x41287F, then faults. The guard must return 1 to
 * the caller with len/off zeroed, ebx/esi intact, and the seat's carried-over count dropped only when no buffer is left.
 * Run under Wine beside winmm.dll, winmm_orig.dll and netguard.dll; prints PASS. */
#include <windows.h>
#include <stdio.h>

__asm__(".globl _victim\n_victim:\n push %ebx\n push %ebp\n mov 0xc(%esp), %ebp\n push %esi\n push %edi\n"
        " movl $0x1C, 0x14(%esp)\n"                                        /* the reused arg slot */
        " xor %ebx, %ebx\n xor %esi, %esi\n xor %edi, %edi\n xor %ebp, %ebp\n mov (%ebx), %eax\n"   /* the bad read */
        " pop %edi\n pop %esi\n pop %ebp\n pop %ebx\n ret $0xc\n");

/* run(self, seat, out): out = {ret, len, off, ebx after, esi after} */
__asm__(".globl _run\n_run:\n push %ebx\n push %esi\n push %edi\n push %ebp\n"
        " mov 20(%esp), %ebx\n mov 24(%esp), %esi\n mov 28(%esp), %edi\n"
        " sub $0x40, %esp\n movl $5, 0xC(%esp)\n movl $7, 0x10(%esp)\n"
        " lea 0x10(%esp), %eax\n push %eax\n lea 0x2C(%esp), %eax\n push %eax\n lea 0x14(%esp), %eax\n push %eax\n"
        " mov %esi, %eax\n mov %ebx, %ecx\n call _victim\n.globl _after_call\n_after_call:\n"
        " mov %eax, (%edi)\n mov 0xC(%esp), %eax\n mov %eax, 4(%edi)\n mov 0x10(%esp), %eax\n mov %eax, 8(%edi)\n"
        " mov %ebx, 12(%edi)\n mov %esi, 16(%edi)\n"
        " add $0x40, %esp\n pop %ebp\n pop %edi\n pop %esi\n pop %ebx\n ret\n");
extern char after_call[];
void run(DWORD *self, DWORD seat, DWORD *out);

static int check(const char *name, DWORD pend, DWORD want_count)
{
    DWORD counts[8] = {0}, pends[8] = {0}, self[19] = {0}, out[5] = {0};
    int ok;
    counts[2] = 44571;
    pends[2] = pend;
    self[17] = (DWORD)counts;
    self[18] = (DWORD)pends;
    run(self, 2, out);
    ok = out[0] == 1 && !out[1] && !out[2] && out[3] == (DWORD)self && out[4] == 2 && counts[2] == want_count;
    printf("%s %s: ret=%lu len=%lu off=%lu ebx=%s esi=%lu count=%lu\n", ok ? "PASS" : "FAIL", name, out[0], out[1],
           out[2], out[3] == (DWORD)self ? "this" : "WRONG", out[4], counts[2]);
    return ok;
}

int main(void)
{
    char v[16];
    int ok;
    snprintf(v, sizeof(v), "%lx", (DWORD)after_call - (DWORD)GetModuleHandleA(NULL));
    SetEnvironmentVariableA("MXHOOK_MSGGUARD_RET", v);
    if (!LoadLibraryA("winmm.dll")) { puts("FAIL: winmm.dll did not load"); return 1; }
    ok = check("no buffer left", 0, 0);
    ok &= check("buffer kept", 0x1234, 44571);
    puts(ok ? "PASS" : "FAIL");
    return !ok;
}
