/* Preparatory shared-slot transport. Only fixed read-only version/owned-jobs/unit queries exist.
 * Writable manager actions and pending-job reconciliation are not implemented.
 */
#ifndef E0T_SYSTEMCTL_PATH
#define E0T_SYSTEMCTL_PATH "/usr/bin/systemctl"
#endif
static int command_limits_match(void) {
    for (unsigned i = 0; i < sizeof(limit_table) / sizeof(limit_table[0]); ++i) {
        struct rlimit actual, expected = expected_limit(i);
        if (limit_table[i].resource == RLIMIT_AS)
            expected.rlim_cur = expected.rlim_max = E0T_COMMAND_AS_LIMIT;
        if (getrlimit(limit_table[i].resource, &actual)
            || actual.rlim_cur != expected.rlim_cur || actual.rlim_max != expected.rlim_max) return 0;
    }
    return 1;
}
static int shared_command_lock(void) {
    int fd = openat(root_fd, "manager-slot", O_RDWR | O_NONBLOCK | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0600);
    struct stat st;
    if (fd < 0) return -1;
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1 || st.st_uid != geteuid()
        || (st.st_mode & 07777) != 0600 || st.st_size != 0 || flock(fd, LOCK_EX | LOCK_NB)) {
        close(fd); return -1;
    }
    return fd;
}
static pid_t manager_wait(pid_t child, int *status) {
#ifdef E0T_TEST_WAIT_FAULT
    (void)child; (void)status;
    return 0;  /* Host-only missing-exit-evidence fixture, not a device build. */
#else
    return waitpid(child, status, WNOHANG);
#endif
}
static int manager_readonly(enum e0t_command_kind kind) {
    if (kind != E0T_VERSION && kind != E0T_OWNED_JOBS && kind != E0T_UNIT_STATES)
        fail("manager read-only operation required");
    uint64_t deadline = now_ms() + 2000;
    uint64_t reply_deadline = deadline - 500;
    int lock = shared_command_lock();
    if (lock < 0) fail("shared manager slot unavailable");
    char previous[256];
    /* A released lock is not proof that a prior command or request terminated.
     * Any retained claim, even one with a dead owner, refuses rather than replays.
     * Future writable operations require their separate exact-job reconciler. */
    if (read_owned("manager-claim", previous, sizeof(previous)) != 0
        || read_owned("manager-child", previous, sizeof(previous)) != 0)
        fail("unresolved previous manager command");
    char claim[128];
    const char *operation = kind == E0T_OWNED_JOBS ? "jobs" : kind == E0T_UNIT_STATES ? "units" : "version";
    snprintf(claim, sizeof(claim), "%s %ld %llu\n", operation, (long)getpid(), identity);
    if (create_owned("manager-claim", claim) != 1) fail("manager intent publication");
    int channel[2];
    if (pipe2(channel, O_CLOEXEC | O_NONBLOCK)) fail("manager reply pipe");
    if (now_ms() >= reply_deadline) fail("manager preparation deadline");
    pid_t child = fork();
    if (child < 0) fail("manager fork");
    if (child == 0) {
        close(channel[0]); close(lock);
        alarm(2);  /* Independent child bound; deadline never proves OS exit. */
        struct rlimit command_as = {E0T_COMMAND_AS_LIMIT, E0T_COMMAND_AS_LIMIT};
        if (setrlimit(RLIMIT_AS, &command_as) || !command_limits_match()) _exit(90);
        char born[128];
        snprintf(born, sizeof(born), "%ld %llu\n", (long)getpid(), own_start());
        if (create_owned("manager-child", born) != 1) _exit(90);
        int flags = fcntl(channel[1], F_GETFL);
        if (flags < 0 || fcntl(channel[1], F_SETFL, flags & ~O_NONBLOCK)
            || dup2(channel[1], STDOUT_FILENO) < 0 || dup2(channel[1], STDERR_FILENO) < 0) _exit(90);
        close(channel[1]);
        int input = open("/dev/null", O_RDONLY | O_CLOEXEC);
        if (input < 0 || dup2(input, STDIN_FILENO) < 0) _exit(90);
        close(input);
        char *environment[] = {"LC_ALL=C", "SYSTEMD_COLORS=0", "SYSTEMD_LOG_TARGET=console",
                               "SYSTEMD_LOG_LEVEL=info", "PATH=/usr/bin:/bin", NULL};
        struct e0t_command_arguments arguments;
        if (e0t_command_encode(kind, 0, NULL, 0, NULL, 0, &arguments) != 1) _exit(90);
        execve(E0T_SYSTEMCTL_PATH, arguments.argv, environment);
        _exit(90);
    }
    close(channel[1]);
    char output[4097]; size_t used = 0;
    int ended = 0, status = 0, failed = 0, eof = 0;
    while (now_ms() < reply_deadline && !(ended && eof)) {
        char chunk[512];
        ssize_t amount = read(channel[0], chunk, sizeof(chunk));
        if (amount > 0) {
            if ((size_t)amount > sizeof(output) - 1 - used) { failed = 1; break; }
            memcpy(output + used, chunk, (size_t)amount); used += (size_t)amount;
        } else if (amount == 0) eof = 1;
        else if (errno != EAGAIN && errno != EINTR) { failed = 1; break; }
        if (!ended) {
            pid_t result = manager_wait(child, &status);
            if (result == child) ended = 1;
            else if (result < 0 && errno != EINTR) { failed = 1; break; }
        }
        if (!(ended && eof)) { struct timespec pace = {.tv_nsec = 10000000}; nanosleep(&pace, NULL); }
    }
    if (!(ended && eof)) failed = 1;
    if (!ended) {
        kill(child, SIGKILL);
        while (now_ms() < deadline - 100 && !ended) {
            pid_t result = manager_wait(child, &status);
            if (result == child) ended = 1;
            else if (result < 0 && errno != EINTR) break;
            if (!ended) { struct timespec pace = {.tv_nsec = 10000000}; nanosleep(&pace, NULL); }
        }
        failed = 1;
    }
    close(channel[0]); output[used] = 0;
    if (!ended || now_ms() >= deadline) fail("manager child exit or bookkeeping deadline unknown");
    /* The exact current claim/child metadata is removed only after actual wait.
     * A bad or missing publication remains unresolved for inspection. */
    char born[128], canonical[128]; long observed_pid; unsigned long long observed_start; char suffix;
    int metadata = read_owned("manager-child", born, sizeof(born));
    if (ended && metadata == 1 && sscanf(born, "%ld %llu%c", &observed_pid, &observed_start, &suffix) == 3
        && suffix == '\n' && observed_pid == child && observed_start) {
        snprintf(canonical, sizeof(canonical), "%ld %llu\n", observed_pid, observed_start);
        if (!strcmp(born, canonical) && live_identity(child, observed_start) == 0
            && read_owned("manager-claim", previous, sizeof(previous)) == 1 && !strcmp(previous, claim)) {
            if (unlinkat(root_fd, "manager-child", 0) || unlinkat(root_fd, "manager-claim", 0)) failed = 1;
        } else failed = 1;
    } else failed = 1;
    close(lock);
    if (failed || !WIFEXITED(status) || WEXITSTATUS(status) != 0) fail("bounded read-only manager command failed");
    if (kind == E0T_OWNED_JOBS) {
        struct e0t_job observed[12]; size_t count;
        if (!e0t_jobs_decode(output, used, observed, &count)) fail("invalid owned job observation");
    }
    if (kind == E0T_UNIT_STATES) {
        struct e0t_unit_snapshot observed;
        if (!e0t_state_decode(output, used, &observed)) fail("invalid unit state observation");
    }
    if (now_ms() >= deadline) fail("manager result deadline");
    int output_flags = fcntl(STDOUT_FILENO, F_GETFL);
    if (output_flags < 0 || fcntl(STDOUT_FILENO, F_SETFL, output_flags | O_NONBLOCK))
        fail("manager result mode");
    ssize_t published = write(STDOUT_FILENO, output, used);
    int restored = fcntl(STDOUT_FILENO, F_SETFL, output_flags);
    if (published != (ssize_t)used || restored) fail("manager result publication");
    if (now_ms() >= deadline) fail("manager result deadline");
    return 0;
}
