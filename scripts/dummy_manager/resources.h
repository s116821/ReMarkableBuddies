/* Candidate per-process limits for host preparation, not a frozen device table. */
#ifndef E0T_AS_LIMIT
#define E0T_AS_LIMIT (8ULL * 1024 * 1024)
#endif
#define E0T_STACK_LIMIT (512ULL * 1024)
#define E0T_DATA_LIMIT (1024ULL * 1024)
#define E0T_HEADROOM (E0T_STACK_LIMIT + E0T_DATA_LIMIT + 512ULL * 1024)
#define E0T_COMMAND_AS_LIMIT (20ULL * 1024 * 1024)
static int manager_parent_mode;

struct limit_entry { int resource; const char *name; rlim_t value; };
static const struct limit_entry limit_table[] = {
    {RLIMIT_AS, "as", E0T_AS_LIMIT}, {RLIMIT_STACK, "stack", E0T_STACK_LIMIT},
    {RLIMIT_DATA, "data", E0T_DATA_LIMIT}, {RLIMIT_FSIZE, "file", RECORD_CAP},
    {RLIMIT_CORE, "core", 0}, {RLIMIT_CPU, "cpu", 2}
};
static struct rlimit expected_limit(unsigned index) {
    struct rlimit value = {limit_table[index].value, limit_table[index].value};
    if (manager_parent_mode && limit_table[index].resource == RLIMIT_AS)
        value.rlim_max = E0T_COMMAND_AS_LIMIT;
    return value;
}
static uint64_t initial_mappings(void) {
    char data[16384];
    int fd = open("/proc/self/maps", O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) fail("initial mappings unavailable");
    size_t used = 0;
    while (used < sizeof(data) - 1) {
        ssize_t amount = read(fd, data + used, sizeof(data) - 1 - used);
        if (amount < 0) fail("initial mappings read");
        if (!amount) break;
        used += (size_t)amount;
    }
    char extra;
    if (read(fd, &extra, 1) != 0) fail("initial mapping record cap");
    close(fd); data[used] = 0;
    uint64_t total = 0;
    char *line = data;
    unsigned count = 0;
    while (*line) {
        char *newline = strchr(line, '\n'), *end;
        if (!newline || ++count > 128) fail("initial mapping syntax/count");
        errno = 0;
        unsigned long long lower = strtoull(line, &end, 16);
        if (errno || end == line || *end != '-') fail("initial mapping lower");
        char *upper_text = end + 1;
        unsigned long long upper = strtoull(upper_text, &end, 16);
        if (errno || end == upper_text || *end != ' ' || upper <= lower
            || UINT64_MAX - total < upper - lower) fail("initial mapping upper");
        total += upper - lower; line = newline + 1;
    }
    if (!count) fail("empty initial mappings");
    return total;
}
static void apply_limits(uint64_t mapped) {
    if (E0T_AS_LIMIT <= E0T_HEADROOM || mapped > E0T_AS_LIMIT - E0T_HEADROOM)
        fail("insufficient initial mapping/headroom");
    for (unsigned i = 0; i < sizeof(limit_table) / sizeof(limit_table[0]); ++i) {
        struct rlimit wanted = expected_limit(i), actual;
        if (setrlimit(limit_table[i].resource, &wanted)
            || getrlimit(limit_table[i].resource, &actual)
            || actual.rlim_cur != wanted.rlim_cur || actual.rlim_max != wanted.rlim_max)
            fail("candidate limit set/get mismatch");
    }
}
static int inherited_limits_match(void) {
    for (unsigned i = 0; i < sizeof(limit_table) / sizeof(limit_table[0]); ++i) {
        struct rlimit actual;
        struct rlimit expected = expected_limit(i);
        if (getrlimit(limit_table[i].resource, &actual)
            || actual.rlim_cur != expected.rlim_cur || actual.rlim_max != expected.rlim_max)
            return 0;
    }
    return 1;
}
static void profile_report(uint64_t mapped) {
    printf("{\"scope\":\"per-process preparation\",\"nonce\":\"%s\",\"initial_mapped_bytes\":%llu,"
           "\"headroom_bytes\":%llu,\"limits\":{", E0T_NONCE,
           (unsigned long long)mapped, (unsigned long long)E0T_HEADROOM);
    for (unsigned i = 0; i < sizeof(limit_table) / sizeof(limit_table[0]); ++i) {
        struct rlimit actual;
        if (getrlimit(limit_table[i].resource, &actual)) fail("profile limit read");
        printf("%s\"%s\":{\"soft\":%llu,\"hard\":%llu}", i ? "," : "", limit_table[i].name,
               (unsigned long long)actual.rlim_cur, (unsigned long long)actual.rlim_max);
    }
    printf("},\"manager_parent\":%s,\"aggregate_kernel_limit\":false,\"device_packet_frozen\":false}\n",
           manager_parent_mode ? "true" : "false");
}
static void record_limits(uint64_t mapped) {
    char event[160];
    snprintf(event, sizeof(event), "limits-verified mapped=%llu as=%llu/%llu stack=%llu data=%llu file=%u core=0 cpu=2",
             (unsigned long long)mapped, (unsigned long long)E0T_AS_LIMIT,
             (unsigned long long)(manager_parent_mode ? E0T_COMMAND_AS_LIMIT : E0T_AS_LIMIT),
             (unsigned long long)E0T_STACK_LIMIT, (unsigned long long)E0T_DATA_LIMIT, RECORD_CAP);
    record(event);
}
#ifdef E0T_RESOURCE_TEST
static void resource_fixture(const char *kind) {
    if (!strcmp(kind, "allocation")) {
        void *allocation = malloc(2 * E0T_DATA_LIMIT);
        if (allocation) { free(allocation); fail("oversized fixture allocation admitted"); }
        puts("allocation-refused");
    } else if (!strcmp(kind, "file")) {
        int fd = openat(root_fd, "resource-file-fixture", O_WRONLY | O_NONBLOCK | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
        if (fd < 0) fail("file fixture ownership");
        char data[RECORD_CAP + 1] = {0};
        signal(SIGXFSZ, SIG_IGN);  /* This host fixture observes EFBIG explicitly. */
        ssize_t first = write(fd, data, sizeof(data));
        errno = 0;
        ssize_t second = write(fd, data, 1);
        int refused = second < 0 && errno == EFBIG;
        struct stat st;
        if (first != RECORD_CAP || !refused || fstat(fd, &st) || st.st_size != RECORD_CAP)
            fail("file kernel cap not observed");
        close(fd); puts("file-refused");
    } else fail("unknown host resource fixture");
}
#endif
