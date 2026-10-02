#ifndef E0T_FENCE_FILE_H
#define E0T_FENCE_FILE_H
/* Pending-publication fence only, NOT a lease/server/dispatch authority.
 * Independent close consumes no request ID. Caller still must fence actual
 * queued effects, reconcile intents and prove exclusive writer/CLI exit.
 * A blocked syscall/held slot is uncertainty, not successful closure.
 */
#include "intent_file.h"
static int e0t_request_slot(int root) {
    if(!e0t_intent_root(root)) return -1;
    int fd=openat(root,"request-slot",O_RDWR|O_CREAT|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC,0600);
    struct stat st;
    if(fd<0) return -1;
    if(fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_nlink!=1 || st.st_uid!=geteuid()
       || (st.st_mode&07777)!=0600 || st.st_size || flock(fd,LOCK_EX|LOCK_NB)) {
        close(fd); return -1;
    }
    return fd;
}
static int e0t_fence_observe(int root) {
    int fd=openat(root,"requests-closed",O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if(fd<0) return errno==ENOENT ? 0 : -1;
    struct stat st; char token[33];
    int valid=!fstat(fd,&st) && S_ISREG(st.st_mode) && st.st_nlink==1
        && st.st_uid==geteuid() && (st.st_mode&07777)==0600 && st.st_size==32
        && read(fd,token,sizeof(token))==32 && !memcmp(token,E0T_NONCE,32);
    if(close(fd)) valid=0;
    return valid ? 1 : -1;
}
/* Called only while holding the actual validated publication slot. */
static int e0t_fence_publish_locked(int root) {
    int state=e0t_fence_observe(root);
    if(state<0) return -1;
    if(state==1) {
        int existing=openat(root,"requests-closed",O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
        if(existing<0) return -1;
        int synced=!fsync(existing) && !fsync(root);
        if(close(existing)) synced=0;
        return synced ? 1 : -1;
    }
    int fd=openat(root,"requests-closed",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC,0600);
    if(fd<0) return -1;
    struct stat st;
    int closed=!fstat(fd,&st) && S_ISREG(st.st_mode) && st.st_nlink==1
        && st.st_uid==geteuid() && (st.st_mode&07777)==0600 && st.st_size==0
        && write(fd,E0T_NONCE,32)==32 && !fsync(fd) && !fsync(root);
    if(close(fd)) closed=0;
    return closed ? 1 : -1;
}
static int e0t_fence_close(int root) {
    int slot=e0t_request_slot(root);
    if(slot<0) return -1;
    int closed=e0t_fence_publish_locked(root);
    if(close(slot)) closed=-1;
    return closed;
}
static enum e0t_intent_result e0t_fenced_intent(int root,const char *frame,size_t length) {
    struct e0t_request request;
    if(!e0t_request_parse(frame,length,&request)) return E0T_INTENT_UNKNOWN;
    int slot=e0t_request_slot(root);
    if(slot<0) return E0T_INTENT_UNKNOWN;
    int state=e0t_fence_observe(root);
    enum e0t_intent_result result=E0T_INTENT_UNKNOWN;
    if(state>=0 && !(state && (request.op==E0T_REQ_START || request.op==E0T_REQ_ADVANCE))
        && (request.op!=E0T_REQ_CLOSE || e0t_fence_publish_locked(root)==1))
        result=e0t_intent_publish(root,frame,length);
    if(close(slot)) result=E0T_INTENT_UNKNOWN;
    return result;
}
#endif
