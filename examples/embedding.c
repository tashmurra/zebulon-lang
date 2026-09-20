/* Compile with -DZEB_ENTRY=<entry symbol from the bundle manifest>. */
#include <stdint.h>
#include <inttypes.h>
#include <stdio.h>
#include "zeb_game.h"
#ifndef ZEB_ENTRY
#error Supply ZEB_ENTRY from manifest.json's game_entry field
#endif
int main(void) {
    /* ABI 2 carries signed integer arguments; this example uses scalar.t. */
    uint64_t word = ZEB_ENTRY(2, 20);
    if ((uint32_t)word != 2) { fputs("unexpected result\n", stderr); return 1; }
    uint32_t bits = (uint32_t)(word >> 32);
    int64_t value = bits <= INT32_MAX ? bits : (int64_t)bits - INT64_C(4294967296);
    printf("%" PRId64 "\n", value);
    return value == 41 ? 0 : 1;
}
