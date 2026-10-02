#ifndef E0T_REQUEST_HISTORY_H
#define E0T_REQUEST_HISTORY_H
/* Fixed-path historical reconstruction only. Caller must already fence actual
 * queued effects and prove exclusive writer loss/handoff. Raw completion can
 * mutate records without this slot: no atomic/current-health claim is made.
 * Output is always closed to starts/advances, including on partial failure.
 */
#include "completion_file.h"
static int e0t_history_read(int root,struct e0t_request_ledger *output) {
    if(!output) return 0;
    memset(output,0,sizeof(*output)); output->generation=1; output->closed=1;
    if(!e0t_intent_root(root)) return 0;
    int slot=openat(root,"request-slot",O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if(slot<0) return 0;
    struct stat st;
    if(fstat(slot,&st) || !S_ISREG(st.st_mode) || st.st_nlink!=1 || st.st_uid!=geteuid()
       || (st.st_mode&07777)!=0600 || st.st_size || flock(slot,LOCK_SH|LOCK_NB)) {
        close(slot); return 0;
    }
    struct e0t_request_ledger history={.generation=1}; int valid=1, gap=0;
    for(unsigned id=1;id<=96;++id) {
        char name[16];
        if(snprintf(name,sizeof(name),"request-%03u",id)!=11) { valid=0; break; }
        int fd=openat(root,name,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
        if(fd<0) {
            if(errno==ENOENT) { gap=1; continue; }
            valid=0; break;
        }
        char data[65]; size_t size; struct e0t_request request; enum e0t_request_outcome outcome;
        int record=!gap && !flock(fd,LOCK_SH|LOCK_NB)
            && e0t_completion_read_locked(fd,data,&size,&request,&outcome)
            && request.id==id && e0t_request_prepare(&history,&request)==E0T_REQUEST_NEW
            && (outcome==E0T_OUTCOME_PENDING || e0t_request_finish(&history,id,outcome));
        if(close(fd)) record=0;
        if(!record) { valid=0; break; }
    }
    if(close(slot)) valid=0;
    if(valid) { history.closed=1; *output=history; }
    return valid;
}
#endif
