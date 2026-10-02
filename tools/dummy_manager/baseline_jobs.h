#ifndef E0T_BASELINE_JOBS_H
#define E0T_BASELINE_JOBS_H
/* Pure combined observer. Baseline names are frozen READ-ONLY manifest data.
 * They never become owned cancellation tuples. Absent rows are observations,
 * not cancellation/terminal attribution. Manager-instance checks are external.
 */
#include "jobs.h"
struct e0t_baseline_name { char unit[128]; };
struct e0t_baseline_observation { uint32_t id; unsigned baseline_index, stop, running; };
struct e0t_combined_jobs {
    struct e0t_job owned[12]; size_t owned_count;
    struct e0t_baseline_observation baseline_readonly[2]; size_t baseline_count;
};
static int e0t_combined_decode(const char *data, size_t length,
                              const struct e0t_baseline_name *names, size_t name_count,
                              struct e0t_combined_jobs *output) {
    if (!output || name_count > 2 || (name_count && !names) || (length && !data)
        || length > 4096 || (length && data[length - 1] != '\n')) return 0;
    size_t name_sizes[2] = {0};
    for (size_t i = 0; i < name_count; ++i) {
        while (name_sizes[i] < 128 && names[i].unit[name_sizes[i]]) ++name_sizes[i];
        if (!name_sizes[i] || name_sizes[i] == 128 || !strncmp(names[i].unit, "buddy-e0t-", 10)) return 0;
        for (size_t j = 0; j < name_sizes[i]; ++j) {
            unsigned char c = (unsigned char)names[i].unit[j];
            if (c < 33 || c > 126 || c == '*' || c == '?' || c == '[' || c == ']' || c == '/') return 0;
        }
        for (size_t j = 0; j < i; ++j)
            if (!strcmp(names[i].unit, names[j].unit)) return 0;
    }
    memset(output, 0, sizeof(*output));
    size_t offset = 0;
    while (offset < length) {
        size_t begin = offset;
        while (offset < length && data[offset] != '\n') ++offset;
        size_t end = ++offset;
        struct e0t_job one[12]; size_t count;
        if (e0t_jobs_decode(data + begin, end - begin, one, &count) && count == 1) {
            if (output->owned_count == 12) return 0;
            for (size_t i = 0; i < output->owned_count; ++i)
                if (output->owned[i].id == one[0].id || output->owned[i].role == one[0].role) return 0;
            for (size_t i = 0; i < output->baseline_count; ++i)
                if (output->baseline_readonly[i].id == one[0].id) return 0;
            output->owned[output->owned_count++] = one[0]; continue;
        }
        const char *tokens[4]; size_t sizes[4]; unsigned number = 0;
        for (size_t i = begin; i < end - 1;) {
            unsigned char c = (unsigned char)data[i];
            if (c < 32 || c > 126) return 0;
            if (c == ' ') { ++i; continue; }
            if (number == 4) return 0;
            tokens[number] = data + i; size_t start = i;
            while (i < end - 1 && data[i] != ' ') {
                if ((unsigned char)data[i] < 33 || (unsigned char)data[i] > 126) return 0;
                ++i;
            }
            sizes[number++] = i - start;
        }
        if (number != 4 || !sizes[0] || sizes[0] > 10 || (sizes[0] > 1 && tokens[0][0] == '0')) return 0;
        uint64_t id = 0;
        for (size_t i = 0; i < sizes[0]; ++i) {
            if (tokens[0][i] < '0' || tokens[0][i] > '9') return 0;
            id = id * 10 + (unsigned)(tokens[0][i] - '0'); if (id > UINT32_MAX) return 0;
        }
        if (!id) return 0;
        unsigned which = 2;
        for (size_t i = 0; i < name_count; ++i)
            if (sizes[1] == name_sizes[i] && !memcmp(tokens[1], names[i].unit, sizes[1])) which = (unsigned)i;
        if (which == 2 || output->baseline_count == 2) return 0;
        unsigned stop, running;
        if (sizes[2] == 5 && !memcmp(tokens[2], "start", 5)) stop = 0;
        else if (sizes[2] == 4 && !memcmp(tokens[2], "stop", 4)) stop = 1;
        else return 0;
        if (sizes[3] == 7 && !memcmp(tokens[3], "running", 7)) running = 1;
        else if (sizes[3] == 7 && !memcmp(tokens[3], "waiting", 7)) running = 0;
        else return 0;
        for (size_t i = 0; i < output->baseline_count; ++i)
            if (output->baseline_readonly[i].id == id || output->baseline_readonly[i].baseline_index == which) return 0;
        for (size_t i = 0; i < output->owned_count; ++i)
            if (output->owned[i].id == id) return 0;
        output->baseline_readonly[output->baseline_count++] =
            (struct e0t_baseline_observation){(uint32_t)id, which, stop, running};
    }
    return 1;  /* All output is discarded on false, including partial rows. */
}
#endif
