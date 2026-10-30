#ifndef E0T_COMPLETION_FILE_H
#define E0T_COMPLETION_FILE_H
/* Append-only historical outcome storage, NOT observation/phase/lease authority.
 * Caller supplies independently proved outcome; persistence cannot prove it.
 * Partial or conflicting completion remains uncertain, never replay permission.
 */
#include "intent_file.h"
static const char *e0t_outcome_line(enum e0t_request_outcome outcome) {
    switch(outcome) {
    case E0T_OUTCOME_OBSERVED: return "observed\n";
    case E0T_OUTCOME_NO_COMMAND: return "no-command\n";
    case E0T_OUTCOME_FAILED_UNKNOWN: return "unknown\n";
    case E0T_OUTCOME_CLOSED: return "closed\n";
    default: return NULL;
    }
}
static int e0t_completion_read_locked(int fd,char data[65],size_t *size,struct e0t_request *request,
                                    enum e0t_request_outcome *outcome) {
    struct stat st;
    if(fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_nlink!=1 || st.st_uid!=geteuid()
       || (st.st_mode&07777)!=0600 || st.st_size<1 || st.st_size>64
       || pread(fd,data,65,0)!=st.st_size) return 0;
    *size=(size_t)st.st_size;
    char *end=memchr(data,'\n',*size);
    if(!end) return 0;
    size_t frame=(size_t)(end-data)+1;
    if(!e0t_request_parse(data,frame,request)) return 0;
    *outcome=E0T_OUTCOME_PENDING;
    if(frame==*size) return 1;
    for(enum e0t_request_outcome candidate=E0T_OUTCOME_OBSERVED;candidate<=E0T_OUTCOME_CLOSED;++candidate) {
        const char *line=e0t_outcome_line(candidate);
        if(strlen(line)==*size-frame && !memcmp(end+1,line,*size-frame)
            && e0t_outcome_valid(request->op,candidate)) { *outcome=candidate; return 1; }
    }
    return 0;
}
/* 1 appended; 0 same historical completion; -1 unknown. No effect replay. */
static int e0t_completion_finish(int root,const char *frame,size_t length,enum e0t_request_outcome outcome) {
    struct e0t_request expected, saved;
    if(!e0t_request_parse(frame,length,&expected) || !e0t_intent_root(root)
        || !e0t_outcome_valid(expected.op,outcome)) return -1;
    char name[16];
    if(snprintf(name,sizeof(name),"request-%03u",expected.id)!=11) return -1;
    int fd=openat(root,name,O_RDWR|O_APPEND|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if(fd<0) return -1;
    char data[65]; size_t size; enum e0t_request_outcome previous;
    int result=-1;
    if(!flock(fd,LOCK_EX|LOCK_NB) && e0t_completion_read_locked(fd,data,&size,&saved,&previous)
        && e0t_request_equal(&expected,&saved) && size>=length && !memcmp(data,frame,length)) {
        if(previous!=E0T_OUTCOME_PENDING) result=previous==outcome && !fsync(fd) ? 0:-1;
        else {
            const char *line=e0t_outcome_line(outcome); size_t bytes=strlen(line);
            if(size+bytes<=64 && write(fd,line,bytes)==(ssize_t)bytes && !fsync(fd)) result=1;
        }
    }
    if(close(fd)) result=-1;
    return result;
}
#endif
