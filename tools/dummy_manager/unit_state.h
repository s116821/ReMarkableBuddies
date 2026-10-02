#ifndef E0T_UNIT_STATE_H
#define E0T_UNIT_STATE_H
/* Pure seven-property observer. Complete parsing is not a cleanup receipt.
 * Original-service observations are disjoint from owned job/cancel tuples.
 * Actual CLI absent-unit fields/status and 4096-byte fit remain target gates.
 */
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
_Static_assert(sizeof(E0T_NONCE) == 33, "compiled nonce length");
struct e0t_unit_state { unsigned load, active, sub; uint32_t main_pid, control_pid, job; };
struct e0t_unit_snapshot {
    struct e0t_unit_state owned[12];
    struct e0t_unit_state original_readonly[3];
};
static int e0t_state_choice(const char *value, size_t size, const char *const *choices, size_t count) {
    for (size_t i = 0; i < count; ++i)
        if (strlen(choices[i]) == size && !memcmp(value, choices[i], size)) return (int)i;
    return -1;
}
static int e0t_state_uint(const char *value, size_t size, int empty, uint32_t *result) {
    if (!size) { *result = 0; return empty; }
    if (size > 10 || (size > 1 && value[0] == '0')) return 0;
    uint64_t total = 0;
    for (size_t i = 0; i < size; ++i) {
        if (value[i] < '0' || value[i] > '9') return 0;
        total = total * 10 + (unsigned)(value[i] - '0');
        if (total > UINT32_MAX) return 0;
    }
    *result = (uint32_t)total; return 1;
}
static int e0t_state_id(const char *value, size_t size) {
    static const char *const roles[] = {"cleanup", "controller", "guard", "stock", "fail-a", "fail-b",
                                       "claim", "norestart", "separate", "notify", "barrier", "queued"};
    static const char *const originals[] = {"xochitl.service", "reader-buddy.service", "rm-sync.service"};
    for (unsigned i = 0; i < 12; ++i) {
        char name[96]; int n = snprintf(name, sizeof(name), "buddy-e0t-%s-%s.service", E0T_NONCE, roles[i]);
        if (n > 0 && (size_t)n == size && !memcmp(name, value, size)) return (int)i;
    }
    int original = e0t_state_choice(value, size, originals, 3);
    return original < 0 ? -1 : original + 12;
}
static int e0t_state_decode(const char *data, size_t length, struct e0t_unit_snapshot *output) {
    static const char *const keys[] = {"Id", "LoadState", "ActiveState", "SubState", "MainPID", "ControlPID", "Job"};
    static const char *const load[] = {"stub", "loaded", "not-found", "bad-setting", "error", "merged", "masked"};
    static const char *const active[] = {"active", "reloading", "inactive", "failed", "activating", "deactivating", "maintenance"};
    static const char *const sub[] = {"dead", "condition", "start-pre", "start", "start-post", "running", "exited",
        "reload", "reload-signal", "reload-notify", "stop", "stop-watchdog", "stop-sigterm", "stop-sigkill", "stop-post",
        "final-watchdog", "final-sigterm", "final-sigkill", "failed", "dead-before-auto-restart",
        "failed-before-auto-restart", "dead-resources-pinned", "auto-restart", "auto-restart-queued", "cleaning"};
    if (!output || !length || length > 4096 || data[length - 1] != '\n') return 0;
    for (size_t i = 0; i < length; ++i)
        if (((unsigned char)data[i] < 32 && data[i] != '\n') || (unsigned char)data[i] > 126) return 0;
    memset(output, 0, sizeof(*output));
    size_t offset = 0; unsigned fields = 0, seen = 0; int index = -1;
    struct e0t_unit_state current = {0};
    while (offset < length) {
        size_t begin = offset;
        while (offset < length && data[offset] != '\n') ++offset;
        size_t size = offset++ - begin;
        if (size) {
            const char *line = data + begin, *equals = memchr(line, '=', size);
            if (!equals) return 0;
            int key = e0t_state_choice(line, (size_t)(equals - line), keys, 7);
            if (key < 0 || (fields & (1u << key))) return 0;
            fields |= 1u << key;
            const char *value = equals + 1; size_t value_size = size - (size_t)(value - line);
            int choice;
            switch (key) {
            case 0: index = e0t_state_id(value, value_size); if (index < 0) return 0; break;
            case 1: choice = e0t_state_choice(value, value_size, load, 7); if (choice < 0) return 0; current.load = (unsigned)choice; break;
            case 2: choice = e0t_state_choice(value, value_size, active, 7); if (choice < 0) return 0; current.active = (unsigned)choice; break;
            case 3: choice = e0t_state_choice(value, value_size, sub, 25); if (choice < 0) return 0; current.sub = (unsigned)choice; break;
            case 4: if (!e0t_state_uint(value, value_size, 0, &current.main_pid)) return 0; break;
            case 5: if (!e0t_state_uint(value, value_size, 0, &current.control_pid)) return 0; break;
            case 6: if (!e0t_state_uint(value, value_size, 1, &current.job)) return 0; break;
            }
        }
        if (!size || offset == length) {
            if (fields != 127 || index < 0 || (seen & (1u << index))) return 0;
            seen |= 1u << index;
            if (index < 12) output->owned[index] = current;
            else output->original_readonly[index - 12] = current;
            fields = 0; index = -1; current = (struct e0t_unit_state){0};
        }
    }
    return seen == 32767;  /* Caller discards every output on false. */
}
#endif
