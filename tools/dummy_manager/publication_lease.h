#ifndef E0T_PUBLICATION_LEASE_H
#define E0T_PUBLICATION_LEASE_H
/* DEFERRED UNVERIFIED DRAFT, preserved at explicit user request. Not compiled,
 * tested, reviewed, wired or selected; no ready execution/cleanup claim.
 * Publication deadline only, NOT manager cleanup or a finished lease server.
 * Trusted caller captures actual entry CLOCK_MONOTONIC and ticks before/after
 * bounded work. Arbitrarily blocked syscalls remain unqualified. No IPC value
 * can extend/reset the lease. Closing file never proves jobs/processes gone.
 */
#include "fence_file.h"
#define E0T_NORMAL_WINDOW_MS UINT64_C(180000)
#define E0T_COMMAND_RESERVE_MS UINT64_C(2000)
enum e0t_publication_phase { E0T_PUBLICATION_OPEN, E0T_PUBLICATION_CLOSED, E0T_PUBLICATION_UNKNOWN };
struct e0t_publication_lease { uint64_t entered,last_seen; enum e0t_publication_phase phase; };
static int e0t_publication_tick(int root,struct e0t_request_ledger *ledger,
                               struct e0t_publication_lease *lease,uint64_t now,int peer_live) {
    if(!ledger || !lease) return -1;
    if(lease->phase==E0T_PUBLICATION_UNKNOWN) { ledger->closed=1; return -1; }
    if(lease->phase==E0T_PUBLICATION_CLOSED) {
        ledger->closed=1;
        if(e0t_fence_observe(root)==1) return 1;
        lease->phase=E0T_PUBLICATION_UNKNOWN; return -1;
    }
    int bad_clock=now<lease->entered || now<lease->last_seen;
    int close=bad_clock || peer_live!=1 || ledger->closed || now-lease->entered>=E0T_NORMAL_WINDOW_MS;
    if(!bad_clock) lease->last_seen=now;
    if(!close) return 0;
    ledger->closed=1;
    int fenced=e0t_fence_close(root);
    lease->phase=fenced==1 && !bad_clock ? E0T_PUBLICATION_CLOSED : E0T_PUBLICATION_UNKNOWN;
    return lease->phase==E0T_PUBLICATION_CLOSED ? 1 : -1;
}
static int e0t_publication_admit(const struct e0t_publication_lease *lease,uint64_t now,int peer_live) {
    return lease && lease->phase==E0T_PUBLICATION_OPEN && peer_live==1
        && now>=lease->entered && now>=lease->last_seen
        && now-lease->entered<E0T_NORMAL_WINDOW_MS-E0T_COMMAND_RESERVE_MS;
}
#endif
