// crashme.c - not shipped: crashes at once (a write to address 0), for source\test_crash_dump.py - debugger.py must
// leave both memory snapshots of it.  cargo xtask build builds it into source\build\, 32-bit like the game.
int main(void)
{
    volatile int *p = 0;
    *p = 1;
    return 0;
}
