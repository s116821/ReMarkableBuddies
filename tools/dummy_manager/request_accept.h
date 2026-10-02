#ifndef E0T_REQUEST_ACCEPT_H
#define E0T_REQUEST_ACCEPT_H
/* Intent/model acceptance only, NOT permission to dispatch. Normal traffic
 * reserves one close +12 cleanup IDs. Independent close has no ID dependency.
 * Caller owns sole writer/phase/lease/current manager proof; queued effects
 * require their separate actual fence. No cached request is current liveness.
 */
#include "fence_file.h"
#include "completion_file.h"
#define E0T_NORMAL_REQUEST_MAX 83U
static int e0t_request_record_matches(int root,const struct e0t_request_entry *entry) {
    if(!e0t_intent_root(root)) return 0;
    char name[16];
    if(snprintf(name,sizeof(name),"request-%03u",entry->request.id)!=11) return 0;
    int fd=openat(root,name,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if(fd<0) return 0;
    char data[65]; size_t size; struct e0t_request saved; enum e0t_request_outcome outcome;
    int matches=!flock(fd,LOCK_SH|LOCK_NB) && e0t_completion_read_locked(fd,data,&size,&saved,&outcome)
        && e0t_request_equal(&saved,&entry->request) && outcome==entry->outcome;
    if(close(fd)) matches=0;
    return matches;
}
static enum e0t_request_decision e0t_accept_intent(int root,struct e0t_request_ledger *ledger,
                                                  const char *frame,size_t length,int cleanup) {
    if(!ledger) return E0T_REQUEST_REFUSED;
    struct e0t_request request;
    if(!e0t_request_parse(frame,length,&request)) {
        ledger->closed=1; (void)e0t_fence_close(root); return E0T_REQUEST_REFUSED;
    }
    struct e0t_request_ledger proposed=*ledger;
    enum e0t_request_decision decision=e0t_request_prepare(&proposed,&request);
    if(cleanup && (!ledger->closed || e0t_fence_observe(root)!=1
       || (request.op!=E0T_REQ_JOBS && request.op!=E0T_REQ_UNITS
           && request.op!=E0T_REQ_CANCEL && request.op!=E0T_REQ_STOP_CASES))) decision=E0T_REQUEST_REFUSED;
    if(!cleanup && decision==E0T_REQUEST_NEW && (ledger->closed
       || (request.id>E0T_NORMAL_REQUEST_MAX && request.op!=E0T_REQ_CLOSE))) decision=E0T_REQUEST_REFUSED;
    if(decision==E0T_REQUEST_REFUSED) {
        ledger->closed=1; (void)e0t_fence_close(root); return decision;
    }
    if(decision!=E0T_REQUEST_NEW) {
        if(!e0t_request_record_matches(root,&ledger->entries[request.id-1])) {
            ledger->closed=1; (void)e0t_fence_close(root); return E0T_REQUEST_REFUSED;
        }
        return decision; /* Historical or unresolved, never a new dispatch. */
    }
    if(e0t_fenced_intent(root,frame,length)!=E0T_INTENT_PUBLISHED) {
        ledger->closed=1; (void)e0t_fence_close(root); return E0T_REQUEST_REFUSED;
    }
    *ledger=proposed; return E0T_REQUEST_NEW; /* Only durable pending intent. */
}
/* Normal IPC may reconcile old IDs, but cannot allocate after close. */
static enum e0t_request_decision e0t_request_accept(int root,struct e0t_request_ledger *ledger,
                                                  const char *frame,size_t length) {
    return e0t_accept_intent(root,ledger,frame,length,0);
}
/* Trusted internal cleanup only. NEVER choose this from an IPC frame field. */
static enum e0t_request_decision e0t_cleanup_accept(int root,struct e0t_request_ledger *ledger,
                                                  const char *frame,size_t length) {
    return e0t_accept_intent(root,ledger,frame,length,1);
}
#endif
