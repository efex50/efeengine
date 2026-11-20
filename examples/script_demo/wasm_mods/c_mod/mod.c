/* script_demo mod in C (roadmap E7.4b example), the twin of ../rust_mod/src/lib.rs.
 *
 * Build by hand (needs clang with the wasm32 target and wasm-ld; see ../README.md):
 *
 *   clang --target=wasm32 -nostdlib -O2 -mbulk-memory -I../../../../crates/efe-guest/include -I. \
 *         -Wl,--no-entry -Wl,--export=efe_scratch mod.c -o mod.wasm
 *
 * `efe_api.h` is generated: `efescript api-export --c script_demo.efapi inventory.efmod -o efe_api.h`.
 * The provides are not emitted for wasm by clang (design doc 4.3): they are listed in mod.toml.
 */
#define EFE_API_IMPLEMENTATION
#define EFE_GUEST_IMPLEMENTATION
#include "efe_api.h"
#include "efe_guest.h"

EFE_SCRATCH(256);

#define MAX_N 256

static int64_t squares[MAX_N];
static char copy[256];
static uint32_t frames;

static int64_t squares_sum(int32_t n) {
    if (n < 0 || n > MAX_N) return -1;
    for (int32_t i = 0; i < n; i++) squares[i] = (int64_t)(i + 1) * (i + 1);
    /* (a second loop over the array: one loop would become the closed form n(n+1)(2n+1)/6, which
       needs __multi3, a function of compiler-rt that a -nostdlib build does not have) */
    int64_t sum = 0;
    for (int32_t i = 0; i < n; i++) sum += squares[i];
    return sum;
}

EFE_PROVIDE(sum_squares, "Sum of the squares 1..=n, collected in a static array; -1 unless n is in 0..=256.", I64,
            EFE_P(I32, "n"))
int64_t sum_squares(int32_t n) {
    return squares_sum(n);
}

EFE_PROVIDE(checksum, "Weighted byte sum of `name`: the sum of (index + 1) * byte, over a copy of the text in a static buffer.", I64,
            EFE_P(STR, "name"))
int64_t checksum(const char *name, size_t name_len) {
    if (name_len > sizeof copy) EFE_TRAP();
    for (size_t i = 0; i < name_len; i++) copy[i] = name[i];
    int64_t sum = 0;
    for (size_t i = 0; i < name_len; i++) sum += (int64_t)(i + 1) * (uint8_t)copy[i];
    return sum;
}

EFE_PROVIDE(boom, "Traps when `code` is 13, else returns `code * 2`.", I32, EFE_P(I32, "code"))
int32_t boom(int32_t code) {
    if (code == 13) EFE_TRAP();
    return code * 2;
}

/* Frame 1: score, give. */
static void first(void) {
    int64_t total = game_score_add(squares_sum(5));
    game_report(total);
    efe_Item item = {0};
    item.id = 7;
    item.count = 5;
    int32_t status = inventory_give("sword", 5, &item);
    game_report(status);
    /* the host wrote the new count through the pointer */
    game_report(((volatile efe_Item *)&item)->count);
}

/* Frame 2: read the event. */
static void second(void) {
    game_report(inventory_given_count());
    efe_Item stack;
    if (inventory_given_read(0, &stack) == 0) {
        game_report(stack.id);
        game_report(stack.count);
    }
}

EFE_EXPORT_AS("frame")
void frame(float dt) {
    (void)dt;
    switch (frames++) {
    case 0: first(); break;
    case 1: second(); break;
    default: break;
    }
}
