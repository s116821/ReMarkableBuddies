/* Pure preparatory decoder, no manager calls or cancellation authority.
 * Input is bounded --no-legend --plain --full output in locale C, no colors.
 * Rows are observations, never submission receipts or terminal-state proof.
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
struct e0t_job { uint32_t id; unsigned role; unsigned stop; unsigned running; };
static int e0t_jobs_decode(const char *data, size_t length, struct e0t_job rows[12], size_t *count) {
    static const char *roles[] = {"cleanup", "controller", "guard", "stock", "fail-a", "fail-b",
                                 "claim", "norestart", "separate", "notify", "barrier", "queued"};
    *count = 0;
    if (length > 4096 || (length && data[length - 1] != '\n')) return 0;
    for (size_t i = 0; i < length; ++i)
        if ((unsigned char)data[i] < 32 && data[i] != '\n') return 0;
        else if ((unsigned char)data[i] > 126) return 0;
    size_t offset = 0;
    while (offset < length) {
        const char *tokens[4]; size_t sizes[4]; unsigned number = 0;
        while (offset < length && data[offset] != '\n') {
            if (data[offset] == ' ') { ++offset; continue; }
            if (number == 4) return 0;
            tokens[number] = data + offset;
            size_t begin = offset;
            while (offset < length && data[offset] != ' ' && data[offset] != '\n') ++offset;
            sizes[number++] = offset - begin;
        }
        ++offset;
        if (number != 4 || *count == 12 || !sizes[0] || sizes[0] > 10) return 0;
        uint64_t id = 0;
        for (size_t i = 0; i < sizes[0]; ++i) {
            if (tokens[0][i] < '0' || tokens[0][i] > '9') return 0;
            id = id * 10 + (unsigned)(tokens[0][i] - '0');
            if (id > UINT32_MAX) return 0;
        }
        if (!id || (sizes[0] > 1 && tokens[0][0] == '0')) return 0;
        unsigned role_index = 12;
        for (unsigned i = 0; i < 12; ++i) {
            char unit[96];
            int n = snprintf(unit, sizeof(unit), "buddy-e0t-%s-%s.service", E0T_NONCE, roles[i]);
            if (n > 0 && (size_t)n == sizes[1] && !memcmp(unit, tokens[1], sizes[1])) role_index = i;
        }
        if (role_index == 12) return 0;
        unsigned stop, running;
        if (sizes[2] == 5 && !memcmp(tokens[2], "start", 5)) stop = 0;
        else if (sizes[2] == 4 && !memcmp(tokens[2], "stop", 4)) stop = 1;
        else return 0;
        if (sizes[3] == 7 && !memcmp(tokens[3], "running", 7)) running = 1;
        else if (sizes[3] == 7 && !memcmp(tokens[3], "waiting", 7)) running = 0;
        else return 0;
        for (size_t i = 0; i < *count; ++i)
            if (rows[i].id == id || rows[i].role == role_index) return 0;
        rows[(*count)++] = (struct e0t_job){(uint32_t)id, role_index, stop, running};
    }
    return 1;
}
