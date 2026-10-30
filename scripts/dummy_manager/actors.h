/* Original preparatory T6-T8 process/file actors. No manager command writer. */
#include <dirent.h>
#include <sys/wait.h>
#ifndef E0T_CGROUP_PARENT
#define E0T_CGROUP_PARENT "/sys/fs/cgroup/systemd/system.slice"
#endif

static int read_owned(const char *name, char *data, size_t capacity) {
    int fd = openat(root_fd, name, O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return errno == ENOENT ? 0 : -1;
    struct stat st;
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1
        || st.st_uid != geteuid() || (st.st_mode & 07777) != 0600
        || st.st_size < 0 || (uint64_t)st.st_size >= capacity) { close(fd); return -1; }
    ssize_t length = read(fd, data, capacity - 1);
    close(fd);
    if (length < 0 || length != st.st_size) return -1;
    data[length] = 0; return 1;
}
static int create_owned(const char *name, const char *data) {
    int fd = openat(root_fd, name, O_WRONLY | O_NONBLOCK | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (fd < 0) return errno == EEXIST ? 0 : -1;
    size_t length = strlen(data);
    int result = write(fd, data, length) == (ssize_t)length && !fsync(fd) ? 1 : -1;
    close(fd); return result;
}
static void identity_name(char *name, size_t capacity, const char *actor) {
    snprintf(name, capacity, "identity-T%u-%s", generation, actor);
}
static int read_identity(const char *actor, pid_t *pid, unsigned long long *start) {
    char name[80], data[128], canonical[128], suffix;
    long number;
    identity_name(name, sizeof(name), actor);
    int status = read_owned(name, data, sizeof(data));
    if (status != 1) return status;
    if (sscanf(data, "%ld %llu%c", &number, start, &suffix) != 3 || suffix != '\n'
        || number <= 1 || number > INT32_MAX || !*start) return -1;
    snprintf(canonical, sizeof(canonical), "%ld %llu\n", number, *start);
    if (strcmp(data, canonical)) return -1;
    *pid = (pid_t)number; return 1;
}
/* 1 matching live identity; 0 gone/reused/zombie; -1 unreadable/unknown. */
static int live_identity(pid_t pid, unsigned long long expected) {
    char path[64], data[4096];
    snprintf(path, sizeof(path), "/proc/%ld/stat", (long)pid);
    int fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return errno == ENOENT ? 0 : -1;
    ssize_t count = read(fd, data, sizeof(data) - 1); close(fd);
    if (count <= 0) return -1;
    data[count] = 0;
    char *field = strrchr(data, ')');
    if (!field || field[1] != ' ') return -1;
    field += 2;
    if (*field == 'Z') return 0;
    for (unsigned number = 3; number < 22; ++number) {
        field = strchr(field, ' '); if (!field) return -1; ++field;
    }
    char *end; errno = 0;
    unsigned long long actual = strtoull(field, &end, 10);
    if (errno || end == field || *end != ' ' || !actual) return -1;
    return actual == expected;
}
static int publish_identity(void) {
#ifdef E0T_TEST_IDENTITY_FAULT
    if (!strcmp(role, "stock")) return 0;  /* Host fault build only. */
#endif
    char name[80], data[128];
    identity_name(name, sizeof(name), role);
    snprintf(data, sizeof(data), "%ld %llu\n", (long)getpid(), identity);
    return create_owned(name, data) == 1;
}
/* Unit starts/jobs must already be fenced by the external operator. This is
 * an independent fresh cgroup observation, not a manager-job completion claim. */
static int peer_cgroup_empty(const char *peer) {
    char path[256];
    snprintf(path, sizeof(path), E0T_CGROUP_PARENT "/buddy-e0t-" E0T_NONCE "-%s.service", peer);
    int fd = open(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return errno == ENOENT ? 1 : -1;
    DIR *directory = fdopendir(fd);
    if (!directory) { close(fd); return -1; }
    struct dirent *entry;
    int result = 1;
    errno = 0;
    while ((entry = readdir(directory))) {
        if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
        struct stat st;
        if (fstatat(fd, entry->d_name, &st, AT_SYMLINK_NOFOLLOW) || S_ISDIR(st.st_mode)
            || S_ISLNK(st.st_mode)) { result = -1; break; }
    }
    if (errno) result = -1;
    int processes = openat(fd, "cgroup.procs", O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    char byte;
    if (processes < 0 || read(processes, &byte, 1) != 0) result = -1;
    if (processes >= 0) close(processes);
    closedir(directory); return result;
}
static int stock_liveness(pid_t *pid, unsigned long long *start) {
    int result = read_identity("stock", pid, start);
    return result == 1 ? live_identity(*pid, *start) : result;
}
static int stock_state(void) {
    char name[80], data[32];
    snprintf(name, sizeof(name), "state-T%u", generation);
    return read_owned(name, data, sizeof(data)) == 1 && !strcmp(data, "stock\n");
}
static int publication_lock(void) {
    char name[80]; snprintf(name, sizeof(name), "publication-lock-T%u", generation);
    int fd = openat(root_fd, name, O_RDWR | O_NONBLOCK | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0600);
    struct stat st;
    if (fd < 0) return -1;
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1 || st.st_uid != geteuid()
        || (st.st_mode & 07777) != 0600 || st.st_size != 0 || flock(fd, LOCK_EX | LOCK_NB)) {
        close(fd); return -1;
    }
    return fd;
}
static int fresh_protection(const char *peer) {
    pid_t peer_pid; unsigned long long peer_start;
    return read_identity(peer, &peer_pid, &peer_start) == 1
        && live_identity(peer_pid, peer_start) == 1;
}
static int begin_restoration(const char *peer, int spend_claim) {
    int lock = publication_lock();
    if (lock < 0) return -1;
    char name[80], data[128];
    int result = -1;
    if (fresh_protection(peer)) {
        snprintf(name, sizeof(name), "closed-T%u", generation);
        int closure = create_owned(name, E0T_NONCE);
        if (closure >= 0 && read_owned(name, data, sizeof(data)) == 1 && !strcmp(data, E0T_NONCE)) {
            result = 1;
            if (spend_claim) {
                snprintf(name, sizeof(name), "restore-claim-T%u", generation);
                snprintf(data, sizeof(data), "%s %ld %llu\n", role, (long)getpid(), identity);
                result = create_owned(name, data) == 1 ? 1 : -1;
            }
        }
    }
    close(lock); return result;
}
static int activation_publication(const char *peer, int permitted) {
    int lock = publication_lock();
    if (lock < 0) return -1;
    char closed[80], state[80], temporary[80], data[128];
    snprintf(closed, sizeof(closed), "closed-T%u", generation);
    snprintf(state, sizeof(state), "state-T%u", generation);
    snprintf(temporary, sizeof(temporary), "activation-partial-T%u-%s", generation, role);
    int result = -1;
    if (permitted && fresh_protection(peer) && read_owned(closed, data, sizeof(data)) == 0
        && read_owned(state, data, sizeof(data)) == 1
        && (!strcmp(data, "stock\n") || !strcmp(data, "injected\n"))
        && create_owned(temporary, "injected\n") == 1
        && !renameat(root_fd, temporary, root_fd, state)) result = 1;
    close(lock); return result;
}
static int restore_actor_locked(const char *peer, pid_t *child, uint64_t deadline) {
    char name[80], claim[80], closed[80], data[128];
    snprintf(claim, sizeof(claim), "restore-claim-T%u", generation);
    snprintf(closed, sizeof(closed), "closed-T%u", generation);
    snprintf(data, sizeof(data), "%s %ld %llu\n", role, (long)getpid(), identity);
    int closure = create_owned(closed, E0T_NONCE);
    if (closure < 0) return -1;
    char closure_value[64];
    if (read_owned(closed, closure_value, sizeof(closure_value)) != 1
        || strcmp(closure_value, E0T_NONCE)) return -1;
    pid_t peer_pid; unsigned long long peer_start;
    if (read_identity(peer, &peer_pid, &peer_start) != 1
        || live_identity(peer_pid, peer_start) != 0 || peer_cgroup_empty(peer) != 1)
        return -1;
    /* A prior claim consumes the one spawn even if its receipt was lost. */
    int existing = read_owned(claim, name, sizeof(name));
    if (existing != 0) {
        pid_t stock_pid; unsigned long long stock_start;
        char receipt[80], expected[128], saved[128];
        if (existing != 1 || !stock_state() || stock_liveness(&stock_pid, &stock_start) != 1) return -1;
        int owner_matches = 0;
        for (unsigned i = 0; i < 2; ++i) {
            const char *owner = i ? "guard" : "controller";
            pid_t owner_pid; unsigned long long owner_start;
            if (read_identity(owner, &owner_pid, &owner_start) != 1) continue;
            snprintf(expected, sizeof(expected), "%s %ld %llu\n", owner, (long)owner_pid, owner_start);
            if (!strcmp(name, expected)) owner_matches = 1;
        }
        if (!owner_matches) return -1;
        snprintf(receipt, sizeof(receipt), "receipt-T%u", generation);
        snprintf(expected, sizeof(expected), "%ld %llu\n", (long)stock_pid, stock_start);
        return read_owned(receipt, saved, sizeof(saved)) == 1 && !strcmp(saved, expected) ? 1 : -1;
    }
    pid_t old_pid; unsigned long long old_start;
    int old = stock_liveness(&old_pid, &old_start);
    if (old != 0) return -1;  /* Unknown or live old child is never a free slot. */
    char initial[80], state_value[32];
    snprintf(initial, sizeof(initial), "state-T%u", generation);
    if (read_owned(initial, state_value, sizeof(state_value)) != 1
        || (strcmp(state_value, "injected\n") && strcmp(state_value, "stock\n"))) return -1;
    if (create_owned(claim, data) != 1 || now_ms() >= deadline) return -1;
    /* The immutable claim is spent before fork. Crash gaps stay unknown. */
    *child = fork();
    if (*child < 0) return -1;
    if (*child == 0) {
#ifdef E0T_TEST_INHERITANCE_FAULT
        struct rlimit altered = {1, 1};
        if (setrlimit(RLIMIT_CPU, &altered)) _exit(90);
#endif
        if (!inherited_limits_match()) _exit(90);
        char number[2] = {(char)('0' + generation), 0};
        execl("/proc/self/exe", ROOT "/helper", "stock", number, (char *)NULL);
        _exit(90);
    }
    while (now_ms() < deadline && now_ms() + 50 < deadline) {
        int child_status;
        pid_t ended = waitpid(*child, &child_status, WNOHANG);
        if (ended == *child) { *child = -1; return -1; }
        if (ended < 0 && errno != EINTR) return -1;
        pid_t stock_pid; unsigned long long stock_start;
        int status = stock_liveness(&stock_pid, &stock_start);
        if (status == 1 && stock_pid == *child) {
            char state[80], temporary[80];
            snprintf(state, sizeof(state), "state-T%u", generation);
            snprintf(temporary, sizeof(temporary), "state-partial-T%u-%s", generation, role);
            if (create_owned(temporary, "stock\n") != 1
                || renameat(root_fd, temporary, root_fd, state)) return -1;
            snprintf(name, sizeof(name), "receipt-T%u", generation);
            snprintf(data, sizeof(data), "%ld %llu\n", (long)stock_pid, stock_start);
            if (create_owned(name, data) != 1 || live_identity(stock_pid, stock_start) != 1)
                return -1;
            return 1;
        }
        if (status < 0) return -1;
        struct timespec pace = {.tv_nsec = 10000000}; nanosleep(&pace, NULL);
    }
    return -1;
}
static int restore_actor(const char *peer, pid_t *child, uint64_t deadline) {
    int lock = publication_lock();
    if (lock < 0) return -1;
    int result = restore_actor_locked(peer, child, deadline);
    close(lock); return result;
}
static int actor_loop(uint64_t entered) {
    const char *peer = !strcmp(role, "controller") ? "guard" : "controller";
    if (!publish_identity()) fail("actor identity publication");
    int control = control_pipe();
    uint64_t deadline = entered + 14000;
    pid_t child = -1;
    int restored = 0, failed = 0, protected = 0, restoring = 0;
    while (!stopping && now_ms() < deadline) {
        pid_t peer_pid; unsigned long long peer_start;
        int peer_record = read_identity(peer, &peer_pid, &peer_start);
        if (!protected && peer_record == 1 && live_identity(peer_pid, peer_start) == 1) {
            protected = 1; record("protection-observed");
        }
        if (!protected && now_ms() > entered + 2000) { failed = 1; record("protection-unavailable"); break; }
        if (protected && !restored && !failed && peer_record != 1) {
            failed = 1; record("peer-identity-unknown");
        }
        if (protected && !restored && !failed && peer_record == 1
            && live_identity(peer_pid, peer_start) < 0) {
            failed = 1; record("peer-identity-unknown");
        }
        if (protected && !restored && !failed && peer_record == 1
            && live_identity(peer_pid, peer_start) == 0) {
            restored = restore_actor(peer, &child, deadline) == 1;
            failed = !restored;
            record(restored ? "restored" : "restoration-unknown");
        }
        struct pollfd p = {.fd = control, .events = POLLIN};
        int polled = poll(&p, 1, 50);
        if (polled < 0 && errno != EINTR) fail("actor polling");
        if (polled > 0 && (p.revents & POLLIN)) {
            char command[2];
            if (read(control, command, sizeof(command)) != 1) fail("actor command framing");
            if ((command[0] == 'B' || command[0] == 'C') && protected && !restored && !failed) {
                if (begin_restoration(peer, command[0] == 'C') == 1) {
                    restoring = 1;
                    record(command[0] == 'C' ? "restore-claim-spent" : "restoration-begun");
                } else record("actor-command-refused");
            }
            else if (command[0] == 'L') {
                record(activation_publication(peer, protected && !failed && !restored && !restoring) == 1
                       ? "activation-published" : "late-publication-refused");
            } else if (command[0] == 'Q') {
                pid_t stock_pid; unsigned long long stock_start;
                record(restored && stock_state() && stock_liveness(&stock_pid, &stock_start) == 1
                       ? "fresh-restoration-live" : "fresh-restoration-unknown");
            } else if (command[0] == 'K' && child > 1) {
                kill(child, SIGTERM); waitpid(child, NULL, 0); child = -1;
                record("owned-stock-stopped");
            } else if (command[0] == 'X') break;
            else record("actor-command-refused");
        }
        if (polled > 0 && (p.revents & POLLHUP)) {
            struct timespec pace = {.tv_nsec = 50000000}; nanosleep(&pace, NULL);
        }
    }
    if (child > 1) { kill(child, SIGTERM); waitpid(child, NULL, 0); }
    close(control);
    return failed ? 90 : 0;
}
