/* Original E0T fixture foundation. Not a device operator or native adapter.
 * Incomplete packet: no device staging/start is authorized by this source.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#ifndef E0T_NONCE
#error "A frozen 32-character lowercase hexadecimal E0T_NONCE is required"
#endif
#ifndef E0T_RUNTIME_PARENT
#define E0T_RUNTIME_PARENT "/run"
#endif
#define ROOT E0T_RUNTIME_PARENT "/buddy-e0t-" E0T_NONCE
#define RECORD_CAP 2048

static const char *const roles[] = {
    "cleanup", "controller", "guard", "stock", "fail-a", "fail-b",
    "claim", "norestart", "separate", "notify", "barrier", "queued"
};
static volatile sig_atomic_t stopping;
static int root_fd = -1;
static unsigned long long identity;
static const char *role;
static unsigned generation;

static void handle_signal(int unused) { (void)unused; stopping = 1; }
static uint64_t now_ms(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) _exit(90);
    return (uint64_t)t.tv_sec * 1000 + (uint64_t)t.tv_nsec / 1000000;
}
static void fail(const char *message) {
    fprintf(stderr, "E0T refusal: %s\n", message);
    exit(90);
}
static void nonce_check(void) {
    const char *n = E0T_NONCE;
    if (strlen(n) != 32) fail("nonce length");
    for (unsigned i = 0; i < 32; ++i)
        if (!((n[i] >= '0' && n[i] <= '9') || (n[i] >= 'a' && n[i] <= 'f')))
            fail("nonce encoding");
}
static unsigned long long own_start(void) {
    char data[4096];
    int fd = open("/proc/self/stat", O_RDONLY | O_CLOEXEC);
    if (fd < 0) fail("self identity unavailable");
    ssize_t len = read(fd, data, sizeof(data) - 1);
    close(fd);
    if (len <= 0) fail("self identity read");
    data[len] = 0;
    char *field = strrchr(data, ')');
    if (!field || field[1] != ' ') fail("self stat syntax");
    field += 2;
    for (unsigned number = 3; number < 22; ++number) {
        field = strchr(field, ' ');
        if (!field) fail("self stat fields");
        ++field;
    }
    char *end;
    errno = 0;
    unsigned long long value = strtoull(field, &end, 10);
    if (errno || end == field || *end != ' ' || !value) fail("self start identity");
    return value;
}
static void open_root(void) {
    root_fd = open(ROOT, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    struct stat st;
    if (root_fd < 0 || fstat(root_fd, &st) || st.st_uid != geteuid()
        || (st.st_mode & 07777) != 0700) fail("owned root identity/mode");
    int fd = openat(root_fd, "owner", O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    char token[33];
    if (fd < 0 || fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1
        || st.st_uid != geteuid() || st.st_size != 32) fail("owner file");
    ssize_t count = read(fd, token, sizeof(token));
    close(fd);
    if (count != 32 || memcmp(token, E0T_NONCE, 32)) fail("owner token");
}
static void record(const char *event) {
    char name[80], line[256];
    snprintf(name, sizeof(name), "events-T%u-%s", generation, role);
    int fd = openat(root_fd, name, O_WRONLY | O_NONBLOCK | O_CREAT | O_APPEND | O_NOFOLLOW | O_CLOEXEC, 0600);
    struct stat st;
    if (fd < 0 || flock(fd, LOCK_EX | LOCK_NB) || fstat(fd, &st) || !S_ISREG(st.st_mode)
        || st.st_nlink != 1 || st.st_uid != geteuid() || (st.st_mode & 07777) != 0600)
        fail("record ownership");
    int length = snprintf(line, sizeof(line), "%s T%u %s pid=%ld start=%llu ms=%llu %s\n",
                          E0T_NONCE, generation, role, (long)getpid(), identity,
                          (unsigned long long)now_ms(), event);
    if (length <= 0 || (size_t)length >= sizeof(line) || st.st_size < 0
        || st.st_size > RECORD_CAP - length) fail("record bound");
    if (write(fd, line, (size_t)length) != length || fsync(fd)) fail("record publication");
    close(fd);
}
static int control_pipe(void) {
    char name[64];
    snprintf(name, sizeof(name), "control-%s", role);
    int fd = openat(root_fd, name, O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    struct stat st;
    if (fd < 0 || fstat(fd, &st) || !S_ISFIFO(st.st_mode) || st.st_uid != geteuid()
        || (st.st_mode & 07777) != 0600) fail("control FIFO ownership");
    return fd;
}
static void notify_message(const char *message) {
    const char *name = getenv("NOTIFY_SOCKET");
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (!name || !*name || strlen(name) >= sizeof(address.sun_path)
        || (name[0] != '/' && name[0] != '@')) fail("notify context");
    memcpy(address.sun_path, name, strlen(name) + 1);
    socklen_t size = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(name) + 1);
    if (name[0] == '@') { address.sun_path[0] = 0; --size; }
    int fd = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0 || sendto(fd, message, strlen(message), MSG_NOSIGNAL,
                         (struct sockaddr *)&address, size) != (ssize_t)strlen(message))
        fail("notify publication");
    close(fd);
}
static int claim(void) {
    int fd = openat(root_fd, "claim-T2", O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (fd >= 0) {
        if (write(fd, E0T_NONCE, 32) != 32 || fsync(fd)) fail("claim durability");
        close(fd);
        record("claim-spent");
        return 42;
    }
    if (errno != EEXIST) fail("claim creation");
    fd = openat(root_fd, "claim-T2", O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
    char token[33]; struct stat st;
    if (fd < 0 || fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1
        || st.st_uid != geteuid() || st.st_size != 32 || read(fd, token, sizeof(token)) != 32
        || memcmp(token, E0T_NONCE, 32)) fail("existing claim identity");
    close(fd);
    record("stock-standin");
    return 0;
}
#include "resources.h"
#ifdef E0T_ACTORS
#include "actors.h"
#endif
int main(int argc, char **argv) {
    uint64_t entered = now_ms();
    signal(SIGALRM, SIG_DFL);
    alarm(15);  /* Process watchdog from entry, not after setup. Blocked kernel I/O
                 * remains an explicit unqualified limit, not proven interruptible. */
    nonce_check();
    uint64_t mapped = initial_mappings();
    apply_limits(mapped);
    if (!inherited_limits_match()) fail("effective limits unavailable");
    if (argc == 2 && !strcmp(argv[1], "--profile")) { profile_report(mapped); return 0; }
#ifdef E0T_RESOURCE_TEST
    if (argc == 3 && !strcmp(argv[1], "--resource-fixture")) { open_root(); resource_fixture(argv[2]); return 0; }
#endif
    if (argc != 3)
        fail("fixed role and case required");
    for (unsigned i = 0; i < sizeof(roles) / sizeof(roles[0]); ++i)
        if (!strcmp(argv[1], roles[i])) role = roles[i];
    if (!role) fail("role outside fixed allowlist");
    open_root();
    char case_number = 0;
    if (!strcmp(argv[2], "current")) {
        int fd = openat(root_fd, "case", O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC);
        struct stat st; char data[2];
        if (fd < 0 || fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_nlink != 1
            || st.st_uid != geteuid() || (st.st_mode & 07777) != 0600 || st.st_size != 1
            || read(fd, data, sizeof(data)) != 1) fail("case record ownership");
        close(fd); case_number = data[0];
    } else if (strlen(argv[2]) == 1) case_number = argv[2][0];
    if (case_number < '1' || case_number > '8') fail("case outside finite range");
    generation = (unsigned)(case_number - '0');
    int allowed = (!strcmp(role, "cleanup"))
        || ((!strcmp(role, "fail-a") || !strcmp(role, "fail-b") || !strcmp(role, "norestart")) && generation <= 2)
        || (!strcmp(role, "claim") && generation == 2)
        || (!strcmp(role, "separate") && generation == 3)
        || (!strcmp(role, "notify") && generation == 4)
        || ((!strcmp(role, "barrier") || !strcmp(role, "queued")) && generation == 5)
        || ((!strcmp(role, "controller") || !strcmp(role, "guard") || !strcmp(role, "stock")) && generation >= 6);
    if (!allowed) fail("role not valid for case");
    /* Manager cleanup still absent. Process/file actors require an explicit host
     * preparation build flag; it does not authorize a device packet. */
    if (!strcmp(role, "cleanup"))
        fail("actor protocol not implemented");
#ifndef E0T_ACTORS
    if (!strcmp(role, "controller") || !strcmp(role, "guard")) fail("actor protocol not enabled");
#endif
    identity = own_start(); record("started"); record_limits(mapped);
    signal(SIGTERM, handle_signal); signal(SIGINT, handle_signal);
#ifdef E0T_ACTORS
    if (!strcmp(role, "controller") || !strcmp(role, "guard")) return actor_loop(entered);
    if (!strcmp(role, "stock") && !publish_identity()) fail("stock identity publication");
#endif
    if (!strcmp(role, "fail-a") || !strcmp(role, "fail-b")) { record("marker"); return 0; }
    if (!strcmp(role, "norestart")) { record("intentional-failure"); return 42; }
    if (!strcmp(role, "claim") && claim()) return 42;
    if (!strcmp(role, "separate")) { record("noop-complete"); return 0; }
    int control = control_pipe();
    if (!strcmp(role, "notify")) {
        const char *pid = getenv("WATCHDOG_PID"), *period = getenv("WATCHDOG_USEC");
        char expected[32]; snprintf(expected, sizeof(expected), "%ld", (long)getpid());
        if (!period || strcmp(period, "3000000") || (pid && strcmp(pid, expected))) fail("watchdog context");
        notify_message("READY=1\nWATCHDOG=1"); record("ready-watchdog-sent");
    }
    uint64_t deadline = entered + 14000;  /* Leave one second for normal exit and
                                         * avoid watchdog-triggered claim restarts. */
    while (!stopping && now_ms() < deadline) {
        struct pollfd p = { .fd = control, .events = POLLIN };
        int result = poll(&p, 1, 50);
        if (result < 0 && errno != EINTR) fail("control polling");
        if (result > 0 && (p.revents & POLLIN)) {
            char message[2]; ssize_t count = read(control, message, sizeof(message));
            if (count == 1 && message[0] == 'X') { record("controlled-exit"); break; }
            fail("unsupported control message");
        }
        /* Open FIFO without a writer may signal HUP continuously. Throttle only
         * that condition; this is pacing, never completion/readiness evidence. */
        if (result > 0 && (p.revents & POLLHUP)) {
            struct timespec pacing = { .tv_nsec = 50000000 }; nanosleep(&pacing, NULL);
        }
    }
    record(stopping ? "signal-exit" : "bounded-exit"); close(control); close(root_fd);
    return 0;
}
