#ifndef E0T_INTENT_FILE_H
#define E0T_INTENT_FILE_H
/* Owned pending-intent publication only. No dispatch/completion/recovery lease.
 * Existing identical records require reconciliation, never effect replay.
 * fsync checks process-loss persistence, not cold-boot/power-loss survival on /run.
 * Sole writer and immutable owned root remain integration obligations.
 */
#include "request_protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
enum e0t_intent_result { E0T_INTENT_UNKNOWN=-1, E0T_INTENT_RECONCILE=0, E0T_INTENT_PUBLISHED=1 };
static int e0t_intent_root(int root) {
    struct stat st;
    if (fstat(root,&st) || !S_ISDIR(st.st_mode) || st.st_uid!=geteuid()
        || (st.st_mode&07777)!=0700) return 0;
    int fd=openat(root,"owner",O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if(fd<0) return 0;
    char token[33];
    int valid=!fstat(fd,&st) && S_ISREG(st.st_mode) && st.st_nlink==1
        && st.st_uid==geteuid() && (st.st_mode&07777)==0600 && st.st_size==32
        && read(fd,token,sizeof(token))==32 && !memcmp(token,E0T_NONCE,32);
    if(close(fd)) valid=0;
    return valid;
}
static enum e0t_intent_result e0t_intent_publish(int root, const char *frame, size_t length) {
    struct e0t_request request;
    if(!e0t_request_parse(frame,length,&request) || !e0t_intent_root(root)) return E0T_INTENT_UNKNOWN;
    char name[16];
    if(snprintf(name,sizeof(name),"request-%03u",request.id)!=11) return E0T_INTENT_UNKNOWN;
    int fd=openat(root,name,O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC,0600);
    if(fd<0) {
        if(errno!=EEXIST) return E0T_INTENT_UNKNOWN;
        fd=openat(root,name,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
        if(fd<0) return E0T_INTENT_UNKNOWN;
        struct stat st; char previous[65];
        int same=!flock(fd,LOCK_SH|LOCK_NB) && !fstat(fd,&st) && S_ISREG(st.st_mode)
            && st.st_nlink==1 && st.st_uid==geteuid() && (st.st_mode&07777)==0600
            && st.st_size==(off_t)length && read(fd,previous,sizeof(previous))==(ssize_t)length
            && !memcmp(previous,frame,length);
        if(close(fd)) same=0;
        return same ? E0T_INTENT_RECONCILE : E0T_INTENT_UNKNOWN;
    }
    struct stat st;
    int published=!flock(fd,LOCK_EX|LOCK_NB) && !fstat(fd,&st) && S_ISREG(st.st_mode)
        && st.st_nlink==1 && st.st_uid==geteuid() && (st.st_mode&07777)==0600
        && st.st_size==0 && write(fd,frame,length)==(ssize_t)length && !fsync(fd)
        && !fsync(root);
    if(close(fd)) published=0;
    /* Any partial/uncertain file is retained. Never unlink/retry as new intent. */
    return published ? E0T_INTENT_PUBLISHED : E0T_INTENT_UNKNOWN;
}
#endif
