#ifndef E0T_CANCEL_ARGS_H
#define E0T_CANCEL_ARGS_H
/* Pure encoding only, no executable action. Caller must freshly correlate every
 * tuple with the same manager/case instance under sole-writer fencing, then
 * recheck it immediately before any future reviewed dispatch. This function
 * does not establish that proof or validate cancellation completion.
 */
#include "jobs.h"
/* 0 means NO COMMAND, 1 encoded nonempty batch, -1 invalid (discard outputs). */
static int e0t_cancel_encode(const struct e0t_job *jobs, size_t count, char numbers[12][11]) {
    if (count > 12 || (count && !jobs)) return -1;
    if (!count) return 0;
    for (size_t i = 0; i < count; ++i) {
        if (!jobs[i].id || jobs[i].role >= 12 || jobs[i].stop > 1 || jobs[i].running > 1) return -1;
        for (size_t previous = 0; previous < i; ++previous)
            if (jobs[previous].id == jobs[i].id || jobs[previous].role == jobs[i].role) return -1;
        int n = snprintf(numbers[i], 11, "%u", jobs[i].id);
        if (n <= 0 || n > 10) return -1;
    }
    return 1;
}

#endif
