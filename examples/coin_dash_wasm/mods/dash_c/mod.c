/* dash_c: C wasm mod for coin_dash_wasm.
 *
 * Responsibilities:
 * - Patrol enemies along their lanes (`Patrol`, `Transform`, `Velocity`, `Sprite`) and emit
 *   `game.patrol_turn` whenever an enemy reverses at the end of its lane.
 * - Spin and bob coins (`Coin`, `Transform`, `Sprite`).
 * - Read `game.coin_spawned` events emitted by `dash_rust`.
 * - Provide helpers (`next_lane`, `coin_phase`, `check_coin_spot`, `provide_calls`) called by
 *   `dash_rust` (declared in `mod.toml` `[[provide]]` for the translator fallback).
 */
#define EFE_API_IMPLEMENTATION
#define EFE_GUEST_IMPLEMENTATION
#include "efe_api.h"
#include "efe_guest.h"

EFE_SCRATCH(256);

static int32_t g_provide_calls = 0;
static int32_t g_events_seen = 0;

EFE_PROVIDE(next_lane, "Return the next patrol lane index `(lane + 1) mod 3`.", I32, EFE_P(I32, "lane"))
int32_t next_lane(int32_t lane) {
    g_provide_calls++;
    int32_t next = (lane + 1) % 3;
    if (next < 0) next += 3;
    return next;
}

EFE_PROVIDE(coin_phase, "Return the initial animation phase for coin `index` of `total`.", F32,
            EFE_P(I32, "index"), EFE_P(I32, "total"))
float coin_phase(int32_t index, int32_t total) {
    g_provide_calls++;
    if (total <= 0) return 0.0f;
    return (float)index / (float)total;
}

EFE_PROVIDE(check_coin_spot, "Return true when `tr.position` is a valid coin spawn spot away from the start.", BOOL,
            EFE_PS("tr", "Transform"))
bool check_coin_spot(const efe_Transform *tr) {
    g_provide_calls++;
    float x = tr->position[0];
    float y = tr->position[1];
    if (x < -300.0f || x > 300.0f || y < -220.0f || y > 220.0f) return false;
    return (x * x + y * y) > 1600.0f;
}

EFE_PROVIDE(provide_calls, "Total number of times dash_c's provided helpers have been called.", I32)
int32_t provide_calls(void) {
    return g_provide_calls;
}

static void handle_coin_spawned_events(void) {
    int32_t n = game_coin_spawned_count();
    if (n <= 0) return;
    efe_CoinSpawned ev;
    for (int32_t i = 0; i < n; i++) {
        if (game_coin_spawned_read((uint32_t)i, &ev) == 0) {
            if (g_events_seen == 0) {
                static const char msg[] = "received game.coin_spawned event";
                engine_log_info(msg, sizeof(msg) - 1);
            }
            g_events_seen++;
        }
    }
}

static void update_patrol(void) {
    efe_entity enemies[16];
    int64_t n = engine_world_query(EFE_COMPONENT_Patrol, enemies, 16);
    if (n <= 0) return;

    efe_Transform tr;
    efe_Patrol p;
    efe_Velocity vel;
    efe_Sprite sp;

    for (int64_t i = 0; i < n; i++) {
        efe_entity e = enemies[i];
        if (engine_world_get(e, EFE_COMPONENT_Transform, &tr) != 0) continue;
        if (engine_world_get(e, EFE_COMPONENT_Patrol, &p) != 0) continue;
        if (engine_world_get(e, EFE_COMPONENT_Velocity, &vel) != 0) continue;

        float dx = tr.position[0] - p.origin[0];
        float dy = tr.position[1] - p.origin[1];
        float along = dx * p.axis[0] + dy * p.axis[1];

        float new_dir = p.dir;
        if (along > p.range) {
            new_dir = -1.0f;
        } else if (along < -p.range) {
            new_dir = 1.0f;
        }
        if (new_dir != p.dir) {
            p.dir = new_dir;
            game_patrol_turn_send();
        }

        vel.linear[0] = p.axis[0] * p.dir * p.speed;
        vel.linear[1] = p.axis[1] * p.dir * p.speed;
        vel.angular = 0.0f;
        engine_world_set(e, EFE_COMPONENT_Velocity, &vel);
        engine_world_set(e, EFE_COMPONENT_Patrol, &p);

        if (p.axis[0] != 0.0f) {
            if (engine_world_get(e, EFE_COMPONENT_Sprite, &sp) == 0) {
                sp.flip = (p.dir < 0.0f) ? 1u : 0u;
                engine_world_set(e, EFE_COMPONENT_Sprite, &sp);
            }
        }
    }
}

static void animate_coins(void) {
    efe_entity coins[32];
    int64_t n = engine_world_query(EFE_COMPONENT_Coin, coins, 32);
    if (n <= 0) return;

    float t = (float)engine_time_elapsed();
    efe_Coin c;
    efe_Transform tr;
    efe_Sprite sp;

    for (int64_t i = 0; i < n; i++) {
        efe_entity e = coins[i];
        if (engine_world_get(e, EFE_COMPONENT_Coin, &c) != 0) continue;
        if (engine_world_get(e, EFE_COMPONENT_Transform, &tr) != 0) continue;
        if (engine_world_get(e, EFE_COMPONENT_Sprite, &sp) != 0) continue;

        float spin_phase = t * 8.0f + c.phase * 4.0f;
        if (spin_phase < 0.0f) spin_phase = 0.0f;
        uint32_t frame = ((uint32_t)spin_phase) & 3u;
        float u0 = (float)frame * 0.25f;
        sp.uv_min[0] = u0;
        sp.uv_min[1] = 0.0f;
        sp.uv_max[0] = u0 + 0.25f;
        sp.uv_max[1] = 1.0f;
        engine_world_set(e, EFE_COMPONENT_Sprite, &sp);

        tr.position[1] = c.base_y + engine_math_sin(t * 3.0f + c.phase * 6.0f) * 2.0f;
        engine_world_set(e, EFE_COMPONENT_Transform, &tr);
    }
}

EFE_EXPORT_AS("init")
void init(void) {
    static const char msg[] = "dash_c initialized";
    engine_log_info(msg, sizeof(msg) - 1);
}

EFE_EXPORT_AS("frame")
void frame(float dt) {
    (void)dt;
    handle_coin_spawned_events();
    update_patrol();
    animate_coins();
}
