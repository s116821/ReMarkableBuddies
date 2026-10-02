#ifndef E0T_COMMAND_ARGS_H
#define E0T_COMMAND_ARGS_H
/* Pure prospective arguments, NEVER dispatches. Phase/profile/fresh-tuple/
 * scoped-namespace/sole-writer checks are mandatory future caller obligations.
 * Output contains self-referencing pointers: consume in place, do not copy it.
 */
#include "baseline_jobs.h"
#include "cancel_args.h"
#ifndef E0T_SYSTEMCTL_PATH
#define E0T_SYSTEMCTL_PATH "/usr/bin/systemctl"
#endif
enum e0t_command_kind { E0T_VERSION, E0T_OWNED_JOBS, E0T_OBSERVED_JOBS, E0T_UNIT_STATES,
                        E0T_CANCEL_BATCH, E0T_STOP_CASES, E0T_STOP_CLEANUP, E0T_START_OWNED, E0T_RELOAD };
struct e0t_command_arguments {
    char units[15][128]; char ids[12][11]; char *argv[32]; size_t argc;
};
/* 0 NO COMMAND, 1 encoded, -1 invalid (discard output). No authority implied. */
static int e0t_command_encode(enum e0t_command_kind kind, unsigned start_role,
                              const struct e0t_job *cancel, size_t cancel_count,
                              const struct e0t_baseline_name *baseline, size_t baseline_count,
                              struct e0t_command_arguments *output) {
    static const char *const roles[] = {"cleanup", "controller", "guard", "stock", "fail-a", "fail-b",
                                       "claim", "norestart", "separate", "notify", "barrier", "queued"};
    if (!output || kind < E0T_VERSION || kind > E0T_RELOAD) return -1;
    struct e0t_combined_jobs validation;
    if (!e0t_combined_decode("", 0, baseline, baseline_count, &validation)) return -1;
    if (kind != E0T_CANCEL_BATCH && cancel_count) return -1;
    if (kind != E0T_OBSERVED_JOBS && baseline_count) return -1;
    memset(output, 0, sizeof(*output));
    for (unsigned i = 0; i < 12; ++i) {
        int n = snprintf(output->units[i], 128, "buddy-e0t-%s-%s.service", E0T_NONCE, roles[i]);
        if (n <= 0 || n >= 128) return -1;
    }
    if (kind == E0T_CANCEL_BATCH) {
        int encoded = e0t_cancel_encode(cancel, cancel_count, output->ids);
        if (encoded != 1) return encoded;
    }
    output->argv[output->argc++] = E0T_SYSTEMCTL_PATH;
    if (kind == E0T_VERSION) { output->argv[output->argc++] = "--version"; return 1; }
    output->argv[output->argc++] = "--no-pager";
    output->argv[output->argc++] = "--no-ask-password";
    switch (kind) {
    case E0T_OWNED_JOBS:
    case E0T_OBSERVED_JOBS:
        output->argv[output->argc++] = "--no-legend";
        output->argv[output->argc++] = "--plain";
        output->argv[output->argc++] = "--full";
        output->argv[output->argc++] = "list-jobs";
        for (unsigned i = 0; i < 12; ++i) output->argv[output->argc++] = output->units[i];
        for (size_t i = 0; i < baseline_count; ++i) {
            memcpy(output->units[12 + i], baseline[i].unit, 128);
            output->argv[output->argc++] = output->units[12 + i];
        }
        break;
    case E0T_UNIT_STATES: {
        static char *const properties[] = {"--property=Id", "--property=LoadState", "--property=ActiveState",
            "--property=SubState", "--property=MainPID", "--property=ControlPID", "--property=Job"};
        output->argv[output->argc++] = "--full";
        output->argv[output->argc++] = "--all";
        output->argv[output->argc++] = "show";
        for (unsigned i = 0; i < 7; ++i) output->argv[output->argc++] = properties[i];
        for (unsigned i = 0; i < 12; ++i) output->argv[output->argc++] = output->units[i];
        strcpy(output->units[12], "xochitl.service");
        strcpy(output->units[13], "reader-buddy.service");
        strcpy(output->units[14], "rm-sync.service");
        for (unsigned i = 12; i < 15; ++i) output->argv[output->argc++] = output->units[i];
        break;
    }
    case E0T_CANCEL_BATCH:
        output->argv[output->argc++] = "cancel";
        for (size_t i = 0; i < cancel_count; ++i) output->argv[output->argc++] = output->ids[i];
        break;
    case E0T_STOP_CASES:
    case E0T_STOP_CLEANUP:
        output->argv[output->argc++] = "--no-block";
        output->argv[output->argc++] = "stop";
        if (kind == E0T_STOP_CLEANUP) output->argv[output->argc++] = output->units[0];
        else for (unsigned i = 1; i < 12; ++i) output->argv[output->argc++] = output->units[i];
        break;
    case E0T_START_OWNED:
        if (start_role >= 12) return -1;
        output->argv[output->argc++] = "--no-block";
        output->argv[output->argc++] = "--job-mode=fail";
        output->argv[output->argc++] = "start";
        output->argv[output->argc++] = output->units[start_role];
        break;
    case E0T_RELOAD:
        output->argv[output->argc++] = "daemon-reload";
        break;
    default: return -1;
    }
    if (output->argc >= 32) return -1;
    return 1;
}
#endif
