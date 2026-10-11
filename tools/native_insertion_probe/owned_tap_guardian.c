/* Development-only RM2 3.28.0.172 ordinary taps: library (110,280 -> 201,1360)
 * and explicit close (737,30 -> 1347,1817). No arbitrary coordinates.
 * Conditional observed ownership, NOT an exclusive kernel lease. No human
 * input is permitted during this transaction. Never kill/restart/replay this
 * writer. Stock recovery must proceed independently of every failure here.
 * --tap NONCE BOOT_UUID OPEN_CUTOFF_BOOTTIME_MS: independently launched sole
 * writer child supervised by its independent parent. Durable no-write setup
 * lease/ready precede exact NONCE\n tap.go, then a new 1500ms contact lease.
 * --tap-close has the same arguments and supervision, using only the close point.
 * --recover NONCE BOOT_UUID: release-only, same persistent flock, dead exact
 * writer identity, unexpired lease, exact tagged contact and no other contacts.
 * tap.released is positive state evidence, not a UI acknowledgment.
 * --check-pidfd: read-only self-process capability check; signal zero only.
 * --check-input: read-only named metadata/ioctl checks; no event reads or writes.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/input.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define SLOT_COUNT 32
#define NATIVE_X 201
#define NATIVE_Y 1360
#define CLOSE_X 1347
#define CLOSE_Y 1817
#define LEASE_MS 1500
#define HOLD_MS 100
#define MAGIC 0x54415031u
struct lease {
    uint32_t magic, version, contact;
    char nonce[33], boot[37];
    uint64_t root_dev, root_ino, input_dev, input_ino, input_rdev;
    uint64_t pen_dev, pen_ino, pen_rdev;
    uint64_t lock_dev, lock_ino;
    struct input_id input_id;
    int64_t pid, start, born, expires;
    int32_t slot, tracking, x, y;
};
struct contact_state { int32_t id[SLOT_COUNT], x[SLOT_COUNT], y[SLOT_COUNT]; int selected; };
struct context {
    int root, lock, input, pen, history;
    char path[128];
    struct lease lease;
    struct stat lease_stat;
    int lease_bound;
    int invalid, down_echo, up_echo, down_frame, up_frame, selected, event_count, complete_down, writer_fenced;
};
static volatile sig_atomic_t interrupted;
static void interruption(int sig) { (void)sig; interrupted = 1; }
static int64_t boot_ms(void) {
    struct timespec t; int64_t value;
    if (clock_gettime(CLOCK_BOOTTIME,&t) || t.tv_sec<0 || t.tv_nsec<0 || t.tv_nsec>=1000000000 ||
        __builtin_mul_overflow((int64_t)t.tv_sec,1000LL,&value) ||
        __builtin_add_overflow(value,(int64_t)(t.tv_nsec/1000000),&value)) return -1;
    return value;
}
static int private_file(const struct stat *s) {
    return S_ISREG(s->st_mode) && s->st_uid==0 && (s->st_mode&07777)==0600 && s->st_nlink==1;
}
static int bytes_write(int fd,const void *bytes,size_t size) {
    const char *p=bytes;
    while(size) { ssize_t n=write(fd,p,size); if(n<0 && errno==EINTR)continue;
        if(n<=0)return 0;
        p+=n; size-=(size_t)n; }
    return 1;
}
static int exclusive_record(struct context *c,const char *name,const void *data,size_t n) {
    int fd=openat(c->root,name,O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600);
    if(fd<0)return 0;
    int ok=fchmod(fd,0600)==0 && bytes_write(fd,data,n) && fsync(fd)==0;
    if(close(fd))ok=0;
    return ok && fsync(c->root)==0;
}
static int read_exact_observed(int root,const char *name,void *bytes,size_t size,struct stat *observed) {
    int fd=openat(root,name,O_RDONLY|O_NOFOLLOW|O_CLOEXEC); struct stat s;
    if(fd<0)return 0;
    int ok=fstat(fd,&s)==0 && private_file(&s) && s.st_size==(off_t)size;
    size_t done=0;
    while(ok && done<size) { ssize_t n=read(fd,(char *)bytes+done,size-done);
        if(n<0 && errno==EINTR)continue;
        if(n<=0){ok=0;break;} done+=(size_t)n; }
    struct stat final;
    ok=ok && fstat(fd,&final)==0 && private_file(&final) && s.st_size==final.st_size &&
        s.st_mtim.tv_sec==final.st_mtim.tv_sec && s.st_mtim.tv_nsec==final.st_mtim.tv_nsec;
    if(ok && observed)*observed=final;
    close(fd); return ok;
}
static int read_exact_at(int root,const char *name,void *bytes,size_t size) {
    return read_exact_observed(root,name,bytes,size,NULL);
}
static int hex_string(const char *s,int uuid) {
    size_t n=strlen(s); int nonzero=0;
    if(n!=(uuid?36u:32u))return 0;
    for(size_t i=0;i<n;++i) {
        if(uuid && (i==8||i==13||i==18||i==23)){if(s[i]!='-')return 0;continue;}
        if(!((s[i]>='0'&&s[i]<='9')||(s[i]>='a'&&s[i]<='f')))return 0;
        nonzero|=s[i]!='0';
    }
    return nonzero;
}
static int file_text(const char *path,char *out,size_t capacity) {
    int fd=open(path,O_RDONLY|O_NOFOLLOW|O_CLOEXEC); if(fd<0)return 0;
    ssize_t n=read(fd,out,capacity); close(fd);
    if(n<=0 || (size_t)n>=capacity)return 0;
    out[n]=0; return 1;
}
static int64_t process_start(int64_t pid) {
    char path[64],text[4096]; snprintf(path,sizeof path,"/proc/%lld/stat",(long long)pid);
    if(!file_text(path,text,sizeof text))return -1;
    char *p=strrchr(text,')'), *save=NULL;
    if(!p || p[1]!=' ')return -1;
    char *field=strtok_r(p+2," ",&save);
    for(int i=1;i<20&&field;++i)field=strtok_r(NULL," ",&save);
    if(!field)return -1;
    char *end; errno=0; long long value=strtoll(field,&end,10);
    return !errno && !*end && value>0 ? value : -1;
}
/* Absence and PID reuse prove this exact old process is gone. Read errors or
 * a still-live/zombie exact identity refuse; never signal a potentially reused PID. */
static int old_writer_gone(const struct lease *l) {
    char path[64]; struct stat s;
    snprintf(path,sizeof path,"/proc/%lld",(long long)l->pid);
    if(stat(path,&s))return errno==ENOENT;
    int64_t current=process_start(l->pid);
    return current>0 && current!=l->start;
}
static int same_root(struct context *c) {
    struct stat s; int fd=open(c->path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(fd<0)return 0;
    int ok=fstat(fd,&s)==0 && s.st_uid==0 && (s.st_mode&07777)==0700 &&
        (uint64_t)s.st_dev==c->lease.root_dev && (uint64_t)s.st_ino==c->lease.root_ino;
    close(fd); return ok;
}
static int same_boot(const struct lease *l) {
    char text[64]; return file_text("/proc/sys/kernel/random/boot_id",text,sizeof text) &&
        strlen(text)==37 && text[36]=='\n' && !memcmp(text,l->boot,36);
}
static int admission_open(struct context *c) {
    struct stat s;
    const char *names[]={"restore.claim","entry.closed","stop.request","stop.admitted"};
    for(size_t i=0;i<sizeof names/sizeof(*names);++i)
        if(fstatat(c->root,names[i],&s,AT_SYMLINK_NOFOLLOW)==0 || errno!=ENOENT)return 0;
    return 1;
}
static int same_input(struct context *c) {
    struct stat held,path; struct input_id id;
    return fstat(c->input,&held)==0 && stat("/dev/input/event2",&path)==0 && S_ISCHR(held.st_mode) &&
        held.st_dev==path.st_dev && held.st_ino==path.st_ino && held.st_rdev==path.st_rdev &&
        (uint64_t)held.st_dev==c->lease.input_dev && (uint64_t)held.st_ino==c->lease.input_ino &&
        (uint64_t)held.st_rdev==c->lease.input_rdev && ioctl(c->input,EVIOCGID,&id)==0 &&
        !memcmp(&id,&c->lease.input_id,sizeof id);
}
static int allowed_point(const struct lease *l) {
    return (l->x==NATIVE_X && l->y==NATIVE_Y) || (l->x==CLOSE_X && l->y==CLOSE_Y);
}
static int lease_context(struct context *c) {
    const struct lease *l=&c->lease;
    char owner[32];
    if(!read_exact_at(c->root,"owner",owner,sizeof owner) || memcmp(owner,l->nonce,32))return 0;
    struct stat held,path;
    if(fstat(c->lock,&held) || fstatat(c->root,"tap.lock",&path,AT_SYMLINK_NOFOLLOW) ||
        !private_file(&held) || !private_file(&path) || held.st_dev!=path.st_dev || held.st_ino!=path.st_ino ||
        (uint64_t)held.st_dev!=l->lock_dev || (uint64_t)held.st_ino!=l->lock_ino)return 0;
    if(c->lease_bound) {
        struct lease current;struct stat observed;
        if(!read_exact_observed(c->root,l->contact?"tap.lease":"tap.setup",&current,sizeof current,&observed) ||
            observed.st_dev!=c->lease_stat.st_dev || observed.st_ino!=c->lease_stat.st_ino ||
            memcmp(&current,l,sizeof current))return 0;
    }
    return l->magic==MAGIC && l->version==1 && l->nonce[32]==0 && l->boot[36]==0 &&
        hex_string(l->nonce,0) && hex_string(l->boot,1) && l->pid>1 && l->start>0 &&
        l->slot==0 && l->tracking>0 && allowed_point(l) &&
        l->contact<=1 && l->born>=0 && l->expires>l->born &&
        l->expires-l->born<=(l->contact?LEASE_MS:15000) &&
        same_root(c) && same_boot(l) && same_input(c);
}
static int lease_valid(struct context *c) {
    int64_t now=boot_ms();
    return lease_context(c) && now>=c->lease.born && now<c->lease.expires;
}
static int axis_values(int fd,int code,int32_t *out) {
    int32_t data[SLOT_COUNT+1]; data[0]=code;
    if(ioctl(fd,EVIOCGMTSLOTS(sizeof data),data))return 0;
    memcpy(out,data+1,SLOT_COUNT*sizeof(int32_t)); return 1;
}
static int snapshot(struct context *c,struct contact_state *state) {
    struct input_absinfo slot; int32_t again[SLOT_COUNT];
    if(ioctl(c->input,EVIOCGABS(ABS_MT_SLOT),&slot) || slot.minimum!=0 ||
        slot.maximum!=SLOT_COUNT-1 || slot.value<0 || slot.value>=SLOT_COUNT)return 0;
    state->selected=slot.value;
    return axis_values(c->input,ABS_MT_TRACKING_ID,state->id) &&
        axis_values(c->input,ABS_MT_POSITION_X,state->x) && axis_values(c->input,ABS_MT_POSITION_Y,state->y) &&
        axis_values(c->input,ABS_MT_TRACKING_ID,again) && !memcmp(state->id,again,sizeof again);
}
struct pen_observation { int query_return, query_errno, touch, tool_pen, tool_rubber; };
static int pen_released_observed(struct context *c,struct pen_observation *observed) {
    struct pen_observation result={-1,0,-1,-1,-1};
    if(observed)*observed=result;
    unsigned char keys[(KEY_MAX+8)/8]={0};
    struct stat held,path;
    if(fstat(c->pen,&held) || stat("/dev/input/event1",&path) || !S_ISCHR(held.st_mode) ||
        held.st_dev!=path.st_dev || held.st_ino!=path.st_ino || held.st_rdev!=path.st_rdev ||
        (uint64_t)held.st_dev!=c->lease.pen_dev || (uint64_t)held.st_ino!=c->lease.pen_ino ||
        (uint64_t)held.st_rdev!=c->lease.pen_rdev)return 0;
    /* Linux evdev returns the copied bitmap byte count on success. */
    result.query_return=ioctl(c->pen,EVIOCGKEY(sizeof keys),keys);
    if(result.query_return<0){result.query_errno=errno;if(observed)*observed=result;return 0;}
    int codes[]={BTN_TOUCH,BTN_TOOL_PEN,BTN_TOOL_RUBBER};
    for(size_t i=0;i<sizeof codes/sizeof(*codes);++i)
        if(result.query_return>(int)sizeof keys || codes[i]/8>=result.query_return) {
            errno=EMSGSIZE;result.query_errno=errno;if(observed)*observed=result;return 0;
        }
    result.touch=!!(keys[BTN_TOUCH/8]&(1u<<(BTN_TOUCH%8)));
    result.tool_pen=!!(keys[BTN_TOOL_PEN/8]&(1u<<(BTN_TOOL_PEN%8)));
    result.tool_rubber=!!(keys[BTN_TOOL_RUBBER/8]&(1u<<(BTN_TOOL_RUBBER%8)));
    if(observed)*observed=result;
    for(size_t i=0;i<sizeof codes/sizeof(*codes);++i)if(keys[codes[i]/8]&(1u<<(codes[i]%8)))return 0;
    return 1;
}
static int pen_released(struct context *c) {return pen_released_observed(c,NULL);}
/* -1 ambiguous/error, 0 completely released, 1 exact owned tagged contact. */
static int owned_state(const struct lease *l,const struct contact_state *s) {
    int active=0;
    for(int i=0;i<SLOT_COUNT;++i) { if(s->id[i]<-1)return -1;
        if(s->id[i]>=0) { if(i!=l->slot || s->id[i]!=l->tracking || s->x[i]!=l->x || s->y[i]!=l->y)return -1; ++active; } }
    return active==0 ? 0 : active==1 ? 1 : -1;
}
static struct input_event event(int type,int code,int value) {
    struct input_event e={0}; e.type=(uint16_t)type; e.code=(uint16_t)code; e.value=value; return e;
}
static int history_record(struct context *c,const char *label,const void *data,size_t n) {
    return bytes_write(c->history,label,strlen(label)) && bytes_write(c->history,data,n) && fsync(c->history)==0;
}
/* Never retry an event write, even when interrupted/short: a prefix may have
 * entered the kernel. Retain intent first, and then observe/conditionally clean. */
static int write_frame(struct context *c,int down) {
    struct input_event e[9]; size_t n=0;
    e[n++]=event(EV_ABS,ABS_MT_SLOT,0);
    e[n++]=event(EV_ABS,ABS_MT_TRACKING_ID,down?c->lease.tracking:-1);
    if(down) {
        e[n++]=event(EV_ABS,ABS_MT_POSITION_X,c->lease.x);
        e[n++]=event(EV_ABS,ABS_MT_POSITION_Y,c->lease.y);
        e[n++]=event(EV_ABS,ABS_MT_PRESSURE,100);
        e[n++]=event(EV_ABS,ABS_MT_TOUCH_MAJOR,17);
        e[n++]=event(EV_ABS,ABS_MT_TOUCH_MINOR,17);
        e[n++]=event(EV_ABS,ABS_MT_ORIENTATION,4);
    }
    e[n++]=event(EV_SYN,SYN_REPORT,0);
    if(!history_record(c,down?"down-intent\n":"up-intent\n",e,n*sizeof(*e)))return 0;
    /* Durable intent may block. Revalidate AFTER it, directly before the
     * nonblocking device write. Actor fencing still owns pause/crash recovery. */
    struct contact_state current;
    if(!lease_valid(c) || !pen_released(c) || !snapshot(c,&current))return 0;
    if(down ? interrupted || !admission_open(c) || owned_state(&c->lease,&current)!=0 :
        owned_state(&c->lease,&current)!=1)return 0;
    ssize_t written=write(c->input,e,n*sizeof(*e));
    return written==(ssize_t)(n*sizeof(*e));
}
/* Raw echo may omit unchanged ABS values. Any foreign slot, unexpected value,
 * malformed record, device loss or SYN_DROPPED makes ownership unresolved. */
static int drain_history(struct context *c,int phase) {
    for(int batch=0;batch<64;++batch) {
        struct input_event events[32]; ssize_t n=read(c->input,events,sizeof events);
        if(n<0 && errno==EINTR)continue;
        if(n<0 && errno==EAGAIN)return !c->invalid;
        if(n<=0 || n%(ssize_t)sizeof(*events))return 0;
        if(!history_record(c,"echo\n",events,(size_t)n))return 0;
        for(size_t i=0;i<(size_t)n/sizeof(*events);++i) {
            struct input_event *e=&events[i]; if(++c->event_count>4096)return 0;
            if(phase==0)return 0;
            if(e->type==EV_SYN) {
                if(e->code!=SYN_REPORT || e->value!=0)return 0;
                if(phase==1 && c->down_echo)c->down_frame=1;
                if(phase==2 && c->up_echo)c->up_frame=1;
                continue;
            }
            if(e->type!=EV_ABS)return 0;
            if(e->code==ABS_MT_SLOT){if(e->value!=0)return 0;c->selected=0;continue;}
            if(c->selected!=0)return 0;
            int expected;
            switch(e->code) {
            case ABS_MT_TRACKING_ID:
                if(e->value==c->lease.tracking && phase==1 && !c->down_echo){c->down_echo=1;continue;}
                if(e->value==-1 && phase==2 && !c->up_echo){c->up_echo=1;continue;}
                return 0;
            case ABS_MT_POSITION_X: expected=c->lease.x;break;
            case ABS_MT_POSITION_Y: expected=c->lease.y;break;
            case ABS_MT_PRESSURE: expected=100;break;
            case ABS_MT_TOUCH_MAJOR: case ABS_MT_TOUCH_MINOR: expected=17;break;
            case ABS_MT_ORIENTATION: expected=4;break;
            default:return 0;
            }
            if(phase!=1 || e->value!=expected)return 0;
        }
    }
    return 0;
}
static int receipt(struct context *c,const char *name,const char *status) {
    char text[512]; const struct lease *l=&c->lease;
    if(l->nonce[32]!=0 || l->boot[36]!=0 || !hex_string(l->nonce,0) || !hex_string(l->boot,1) || !allowed_point(l))return 0;
    int n=snprintf(text,sizeof text,"{\"nonce\":\"%s\",\"boot_id\":\"%s\",\"pid\":%lld,\"start_ticks\":%lld,\"slot\":0,\"tracking_id\":%d,\"native_point\":[%d,%d],\"lease_end_ms\":%lld,\"status\":\"%s\",\"exclusive_kernel_ownership\":false}\n",
        l->nonce,l->boot,(long long)l->pid,(long long)l->start,l->tracking,l->x,l->y,(long long)l->expires,status);
    return n>0 && (size_t)n<sizeof text && exclusive_record(c,name,text,(size_t)n);
}
static int conditional_up(struct context *c,int takeover) {
    struct contact_state s;
    if(!lease_valid(c) || !pen_released(c) || !snapshot(c,&s))return 0;
    int state=owned_state(&c->lease,&s);
    if(state<0)return 0;
    if(state==0) { /* No input to an empty lease. This is not a proven tap. */
        return receipt(c,"tap.empty","observed-empty-no-input");
    }
    if(!takeover && !drain_history(c,1))return 0;
    /* Recheck immediately before the ONLY up attempt. */
    if(!lease_valid(c) || !snapshot(c,&s) || owned_state(&c->lease,&s)!=1)return 0;
    if(!write_frame(c,0))return 0;
    for(int i=0;i<40;++i) {
        if(!lease_valid(c) || !drain_history(c,2))return 0;
        if(c->up_frame && snapshot(c,&s) && owned_state(&c->lease,&s)==0 && pen_released(c))
            return receipt(c,takeover?"tap.recovered":c->complete_down?"tap.released":"tap.cleaned",
                takeover?"conditional-takeover-released":c->complete_down?"guardian-released":"partial-down-cleaned-not-a-tap");
        struct timespec pause={0,1000000}; nanosleep(&pause,NULL);
    }
    return 0;
}
static int setup_device(struct context *c,int existing) {
    char model[128],release[4096];
    if(!file_text("/etc/hwrevision",model,sizeof model) || !strstr(model,"reMarkable2 1.0") ||
        !file_text("/usr/lib/os-release",release,sizeof release) ||
        (!strstr(release,"IMG_VERSION=\"3.28.0.172\"\n") && !strstr(release,"IMG_VERSION=3.28.0.172\n")))return 0;
    c->input=open("/dev/input/event2",O_RDWR|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC);
    c->pen=open("/dev/input/event1",O_RDONLY|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC);
    struct stat s; struct input_id id;
    if(c->input<0 || c->pen<0 || fstat(c->input,&s) || !S_ISCHR(s.st_mode) || ioctl(c->input,EVIOCGID,&id))return 0;
    if(!existing) { c->lease.input_dev=s.st_dev;c->lease.input_ino=s.st_ino;
        c->lease.input_rdev=s.st_rdev;c->lease.input_id=id; }
    if(fstat(c->pen,&s) || !S_ISCHR(s.st_mode))return 0;
    if(!existing) {c->lease.pen_dev=s.st_dev;c->lease.pen_ino=s.st_ino;c->lease.pen_rdev=s.st_rdev;}
    return same_input(c);
}
static int start_tap(struct context *c,int64_t cutoff) {
    struct contact_state s; struct input_absinfo ids;
    if(!setup_device(c,0) || !pen_released(c) || !drain_history(c,0) || !snapshot(c,&s) ||
        owned_state(&c->lease,&s)!=0 || ioctl(c->input,EVIOCGABS(ABS_MT_TRACKING_ID),&ids))return 0;
    int minimum=ids.minimum<1?1:ids.minimum;
    if(ids.maximum<minimum || ids.maximum>65535)return 0;
    uint32_t random; if(getrandom(&random,sizeof random,0)!=(ssize_t)sizeof random)return 0;
    c->lease.tracking=minimum+(int)(random%(uint32_t)(ids.maximum-minimum+1));
    c->selected=s.selected;
    c->lease.born=boot_ms();
    if(c->lease.born<0 || c->lease.born>INT64_MAX-15000)return 0;
    /* Ready/setup is a no-write deadline, not a renewed contact lease. */
    c->lease.contact=0;
    c->lease.expires=c->lease.born+15000;
    if(cutoff<c->lease.expires)c->lease.expires=cutoff;
    if(c->lease.expires-c->lease.born<500 || !lease_valid(c) ||
        !exclusive_record(c,"tap.setup",&c->lease,sizeof c->lease))return 0;
    struct lease persisted;
    if(!read_exact_observed(c->root,"tap.setup",&persisted,sizeof persisted,&c->lease_stat) ||
        memcmp(&persisted,&c->lease,sizeof persisted))return 0;
    c->lease_bound=1;
    if(!receipt(c,"tap.ready","guardian-ready-no-down"))return 0;
    int permission=0;
    while(lease_valid(c) && admission_open(c) && !interrupted) {
        char token[33],expected[34]; snprintf(expected,sizeof expected,"%s\n",c->lease.nonce);
        if(read_exact_at(c->root,"tap.go",token,sizeof token)) { permission=!memcmp(token,expected,33);break; }
        struct stat go; if(fstatat(c->root,"tap.go",&go,AT_SYMLINK_NOFOLLOW)==0 || errno!=ENOENT)return 0;
        struct timespec pause={0,2000000}; nanosleep(&pause,NULL);
    }
    if(!permission || interrupted || !lease_valid(c) || !admission_open(c) || !drain_history(c,0) || !pen_released(c) ||
        !snapshot(c,&s) || owned_state(&c->lease,&s)!=0 || c->lease.expires-boot_ms()<200)return 0;
    c->lease_bound=0;c->lease.contact=1;c->lease.born=boot_ms();
    if(c->lease.born<0 || c->lease.born>INT64_MAX-LEASE_MS)return 0;
    c->lease.expires=c->lease.born+LEASE_MS;
    if(cutoff<c->lease.expires)c->lease.expires=cutoff;
    if(c->lease.expires-c->lease.born<500 || !lease_valid(c) ||
        !exclusive_record(c,"tap.lease",&c->lease,sizeof c->lease) ||
        !read_exact_observed(c->root,"tap.lease",&persisted,sizeof persisted,&c->lease_stat) ||
        memcmp(&persisted,&c->lease,sizeof persisted))return 0;
    c->lease_bound=1;
    if(!exclusive_record(c,"tap.down-intent",c->lease.nonce,32))return 0;
    if(!lease_valid(c) || !admission_open(c))return 0;
    int down=write_frame(c,1); int64_t held=boot_ms();
    c->complete_down=down;
    while(down && !interrupted && lease_valid(c) && boot_ms()-held<HOLD_MS) {
        if(!drain_history(c,1)){c->invalid=1;break;}
        struct timespec pause={0,1000000}; nanosleep(&pause,NULL);
    }
    c->complete_down=down && c->down_frame && !interrupted && boot_ms()>=held+HOLD_MS;
    /* Partial/error down is an obligation, never permission to replay. Exact
     * id/point is still required; an incomplete prefix without that point is
     * honestly unresolved and must not blanket-release another contact. */
    if(c->invalid)return 0;
    int up=conditional_up(c,0);
    return c->complete_down && up;
}
static int recover_observed(struct context *c) {
    struct contact_state s;
    if(!lease_context(c) || !snapshot(c,&s) || !drain_history(c,0))return 0;
    c->selected=s.selected;
    /* Expiry never renews write permission. It can still admit a read-only
     * fresh fully released reconciliation after the exact old writer is gone. */
    if(owned_state(&c->lease,&s)==0 && pen_released(c))
        return receipt(c,"tap.empty","observed-empty-no-input");
    if(!lease_valid(c))return 0;
    /* A new reader has no pre-crash history. Takeover is explicitly conditional
     * on controlled no-human-input plus exact live descriptor/id/point state. */
    return conditional_up(c,1);
}
static int recover(struct context *c,const char *nonce,const char *boot) {
    if(!read_exact_observed(c->root,"tap.lease",&c->lease,sizeof c->lease,&c->lease_stat) ||
        c->lease.nonce[32]!=0 || c->lease.boot[36]!=0 ||
        strcmp(c->lease.nonce,nonce) || strcmp(c->lease.boot,boot) ||
        c->lease.magic!=MAGIC || c->lease.version!=1 || c->lease.pid<=1 || c->lease.start<=0 ||
        (!c->writer_fenced && !old_writer_gone(&c->lease)) || !setup_device(c,1))return 0;
    c->lease_bound=1;
    return recover_observed(c);
}
/* Actor-only fencing, never a host timeout wrapper. Stable pidfd signals this
 * exact process or none; no numeric-PID kill fallback. Unsupported kernels,
 * clock failure or unconfirmed death report unresolved while stock recovers. */
static int fence_writer(struct context *c,const char *nonce,const char *boot) {
    char permission[33],expected[34];snprintf(expected,sizeof expected,"%s\n",nonce);
    if(!read_exact_at(c->root,"restore.claim",permission,sizeof permission) ||
        memcmp(permission,expected,33) ||
        !read_exact_observed(c->root,"tap.lease",&c->lease,sizeof c->lease,&c->lease_stat) ||
        c->lease.nonce[32]!=0 || c->lease.boot[36]!=0 || strcmp(c->lease.nonce,nonce) || strcmp(c->lease.boot,boot) ||
        c->lease.pid<=1 || c->lease.pid==getpid() || c->lease.start<=0 || !setup_device(c,1))return 0;
    c->lease_bound=1;
    if(!lease_context(c))return 0;
    if(old_writer_gone(&c->lease))return 1;
#if defined(SYS_pidfd_open) && defined(SYS_pidfd_send_signal)
    int pidfd=(int)syscall(SYS_pidfd_open,(pid_t)c->lease.pid,0);
    if(pidfd<0)return 0;
    int ok=process_start(c->lease.pid)==c->lease.start && lease_context(c);
    /* No replacement/new owner may race this exact descriptor-bound fence. */
    if(ok)ok=syscall(SYS_pidfd_send_signal,pidfd,SIGTERM,NULL,0)==0;
    if(ok)ok=syscall(SYS_pidfd_send_signal,pidfd,SIGCONT,NULL,0)==0;
    int64_t origin=boot_ms();
    if(origin<0 || origin>INT64_MAX-250)ok=0;
    int killed=0;
    while(ok) {
        struct pollfd p={.fd=pidfd,.events=POLLIN};
        int n=poll(&p,1,0);
        if(n>0 && (p.revents&POLLIN)){c->writer_fenced=1;break;}
        if(n<0 && errno!=EINTR){ok=0;break;}
        int64_t now=boot_ms();
        if(now<origin || now>=origin+250){ok=0;break;}
        if(!killed && now>=origin+50) {
            if(syscall(SYS_pidfd_send_signal,pidfd,SIGKILL,NULL,0)){ok=0;break;}
            killed=1;
        }
        struct timespec pause={0,1000000};nanosleep(&pause,NULL);
    }
    close(pidfd);return ok && c->writer_fenced;
#else
    return 0;
#endif
}
static int run_guardian(int argc,char **argv) {
    struct context c={.root=-1,.lock=-1,.input=-1,.pen=-1,.history=-1};
    int close_tap=argc==5 && !strcmp(argv[1],"--tap-close");
    int tap=argc==5 && (!strcmp(argv[1],"--tap") || close_tap);
    int take=argc==4 && !strcmp(argv[1],"--recover");
    int ok=0;
    if((!tap&&!take) || getuid()!=0 || !hex_string(argv[2],0) || !hex_string(argv[3],1))goto done;
    snprintf(c.path,sizeof c.path,"/run/rmb-qt-probe-%s",argv[2]);
    c.root=open(c.path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);struct stat root;
    if(c.root<0 || fstat(c.root,&root) || root.st_uid!=0 || (root.st_mode&07777)!=0700)goto done;
    char owner[32]; if(!read_exact_at(c.root,"owner",owner,32) || memcmp(owner,argv[2],32))goto done;
    c.lock=openat(c.root,"tap.lock",O_RDWR|O_NOFOLLOW|O_CLOEXEC|(tap?(O_CREAT|O_EXCL):0),0600);
    struct stat lock;
    if(c.lock<0 || fstat(c.lock,&lock) || !private_file(&lock))goto done;
    if(take) {
        if(!fence_writer(&c,argv[2],argv[3]))goto done;
        /* pidfd-confirmed death releases the flock; it is never unlinked. */
        if(flock(c.lock,LOCK_EX|LOCK_NB))goto done;
        /* fencing opened source descriptors only, never emitted an event */
        if(c.input>=0){close(c.input);c.input=-1;}
        if(c.pen>=0){close(c.pen);c.pen=-1;}
    } else if(flock(c.lock,LOCK_EX|LOCK_NB))goto done;
    /* Persistent lock is never removed, replaced or reacquired as a new owner. */
    c.history=openat(c.root,tap?"tap.history":"tap.recovery-history",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600);
    if(c.history<0)goto done;
    if(tap) {
        c.lease.magic=MAGIC;c.lease.version=1;strcpy(c.lease.nonce,argv[2]);strcpy(c.lease.boot,argv[3]);
        c.lease.root_dev=root.st_dev;c.lease.root_ino=root.st_ino;
        c.lease.lock_dev=lock.st_dev;c.lease.lock_ino=lock.st_ino;
        c.lease.pid=getpid();c.lease.start=process_start(getpid());
        c.lease.slot=0;c.lease.x=close_tap?CLOSE_X:NATIVE_X;c.lease.y=close_tap?CLOSE_Y:NATIVE_Y;
        char *end;errno=0;long long cutoff=strtoll(argv[4],&end,10);
        if(errno || !argv[4][0] || *end || cutoff<=0)goto done;
        struct sigaction action={0};action.sa_handler=interruption;
        sigaction(SIGTERM,&action,NULL);sigaction(SIGHUP,&action,NULL);sigaction(SIGINT,&action,NULL);
        ok=start_tap(&c,cutoff);
    } else ok=recover(&c,argv[2],argv[3]);
done:
    if(!ok && c.root>=0 && c.lease.magic==MAGIC) (void)receipt(&c,"tap.unresolved","input-unresolved-or-not-performed-no-replay");
    if(c.history>=0)close(c.history);
    if(c.input>=0)close(c.input);
    if(c.pen>=0)close(c.pen);
    if(c.lock>=0)close(c.lock);
    if(c.root>=0)close(c.root);
    if(!ok)fputs("guardian refused/unresolved; never replay; continue independent stock recovery\n",stderr);
    return ok?0:90;
}
/* Independent local parent starts BEFORE the writer owns any descriptor/lock.
 * The only normal down/up writer is the child. kill(child) cannot hit a reused
 * PID: this direct child remains unreaped until waitpid confirms its death.
 * Supervisor cleanup is ALWAYS a spent/failed tap, never host-arm admission.
 */
static int supervise(int argc,char **argv) {
    char *end;errno=0;int64_t cutoff=strtoll(argv[4],&end,10);
    int64_t now=boot_ms();
    if(errno || !argv[4][0] || *end || now<0 || cutoff<=now || cutoff-now>15000)return 90;
    pid_t child=fork();if(child<0)return 90;
    if(!child)_exit(run_guardian(argc,argv));
    int status=0,abnormal=0,reaped=0;
    struct context c={.root=-1,.lock=-1,.input=-1,.pen=-1,.history=-1};
    snprintf(c.path,sizeof c.path,"/run/rmb-qt-probe-%s",argv[2]);
    while(!reaped) {
        pid_t result=waitpid(child,&status,WNOHANG);
        if(result==child){reaped=1;abnormal=!WIFEXITED(status)||WEXITSTATUS(status)!=0;break;}
        if(result<0){abnormal=1;break;}
        now=boot_ms();int64_t bound=cutoff;
        int root=open(c.path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
        if(root>=0) {
            struct lease l;
            if(read_exact_at(root,"tap.lease",&l,sizeof l) && l.magic==MAGIC && l.version==1 &&
                l.contact==1 && l.pid==child && l.born>=0 && l.born<=INT64_MAX-800 &&
                l.expires>l.born && l.expires-l.born<=LEASE_MS) {
                int64_t cap=l.born+800;
                if(l.expires-100<cap)cap=l.expires-100;
                if(cap<bound)bound=cap;
            }
            close(root);
        }
        if(now<0 || now>=bound) {
            /* Only this unreaped direct child is fenced, never an arbitrary PID. */
            (void)kill(child,SIGKILL);abnormal=1;
            int64_t fence_origin=boot_ms();
            for(int i=0;i<100;++i) {
                result=waitpid(child,&status,WNOHANG);
                if(result==child){reaped=1;break;}
                if(result<0)break;
                int64_t current=boot_ms();
                if(fence_origin<0 || current<fence_origin || current-fence_origin>=100)break;
                struct timespec pause={0,1000000};nanosleep(&pause,NULL);
            }
            break;
        }
        struct timespec pause={0,1000000};nanosleep(&pause,NULL);
    }
    if(!abnormal && reaped) {
        c.root=open(c.path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
        if(c.root<0)return 90;
        c.lock=openat(c.root,"tap.lock",O_RDWR|O_NOFOLLOW|O_CLOEXEC);
        int good=c.lock>=0 && flock(c.lock,LOCK_EX|LOCK_NB)==0 &&
            read_exact_observed(c.root,"tap.lease",&c.lease,sizeof c.lease,&c.lease_stat) &&
            c.lease.nonce[32]==0 && c.lease.boot[36]==0 && !strcmp(c.lease.nonce,argv[2]) &&
            !strcmp(c.lease.boot,argv[3]) && c.lease.pid==child && c.lease.contact==1 &&
            c.lease.x==(!strcmp(argv[1],"--tap-close")?CLOSE_X:NATIVE_X) &&
            c.lease.y==(!strcmp(argv[1],"--tap-close")?CLOSE_Y:NATIVE_Y);
        c.lease_bound=1;
        struct contact_state s;
        if(good)good=setup_device(&c,1) && lease_valid(&c) && snapshot(&c,&s) &&
            owned_state(&c.lease,&s)==0 && pen_released(&c) &&
            receipt(&c,"tap.supervisor-complete","normal-child-exit");
        if(c.input>=0)close(c.input);
        if(c.pen>=0)close(c.pen);
        if(c.lock>=0)close(c.lock);
        close(c.root);return good?0:90;
    }
    if(!reaped)return 90; /* Never emit up before confirmed death. */
    c.root=open(c.path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(c.root<0)return 90;
    c.lock=openat(c.root,"tap.lock",O_RDWR|O_NOFOLLOW|O_CLOEXEC);
    if(c.lock<0 || flock(c.lock,LOCK_EX|LOCK_NB))goto finish;
    if(!read_exact_observed(c.root,"tap.lease",&c.lease,sizeof c.lease,&c.lease_stat) ||
        c.lease.nonce[32]!=0 || c.lease.boot[36]!=0 || strcmp(c.lease.nonce,argv[2]) || strcmp(c.lease.boot,argv[3]) ||
        c.lease.pid!=child || c.lease.contact!=1)goto finish;
    c.lease_bound=1;
    c.history=openat(c.root,"tap.supervisor-history",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600);
    if(c.history<0 || !setup_device(&c,1))goto finish;
    (void)recover_observed(&c);
    (void)receipt(&c,"tap.supervisor-spent","supervisor-cleanup-never-arm");
finish:
    if(c.history>=0)close(c.history);
    if(c.input>=0)close(c.input);
    if(c.pen>=0)close(c.pen);
    if(c.lock>=0)close(c.lock);
    close(c.root);return 90;
}
static int check_pidfd(void) {
    int supported=0,error=ENOSYS;
#if defined(SYS_pidfd_open) && defined(SYS_pidfd_send_signal)
    int fd=(int)syscall(SYS_pidfd_open,getpid(),0);
    if(fd<0)error=errno;
    else {
        if(syscall(SYS_pidfd_send_signal,fd,0,NULL,0)==0){supported=1;error=0;}
        else error=errno;
        if(close(fd)!=0){supported=0;error=errno;}
    }
#endif
    int written=printf("{\"status\":\"%s\",\"pidfd_supported\":%s,\"errno\":%d,\"input_events\":0,\"signal\":0}\n",
        supported?"pidfd-self-check-supported":"pidfd-self-check-unavailable",supported?"true":"false",error);
    return supported && written>0 && fflush(stdout)==0?0:90;
}
static int diagnostic_check(const char *name,int ok,int error,int *first) {
    printf("%s{\"name\":\"%s\",\"ok\":%s,\"errno\":%d}",
        *first?"":",",name,ok?"true":"false",ok?0:error?error:EINVAL);
    *first=0;return ok;
}
static int check_input(void) {
    struct context c={.root=-1,.lock=-1,.input=-1,.pen=-1,.history=-1};
    char model[128],release[4096];struct stat input={0},pen={0};
    struct input_absinfo slot={0},tracking={0},x={0},y={0};
    struct contact_state state={0};int32_t again[SLOT_COUNT];
    int first=1,all=1,keys=0,ids=0,xs=0,ys=0,stable=0;
    struct pen_observation pen_state;
    printf("{\"checks\":[");
#define INPUT_CHECK(name,expression) do { errno=0;int passed=(expression);int error=errno; \
    all=diagnostic_check(name,passed,error,&first)&&all; } while(0)
    INPUT_CHECK("model",file_text("/etc/hwrevision",model,sizeof model) && strstr(model,"reMarkable2 1.0")!=NULL);
    INPUT_CHECK("canonical-firmware",file_text("/usr/lib/os-release",release,sizeof release) &&
        (strstr(release,"IMG_VERSION=\"3.28.0.172\"\n") || strstr(release,"IMG_VERSION=3.28.0.172\n")));
    INPUT_CHECK("touch-open-readonly",(c.input=open("/dev/input/event2",O_RDONLY|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC))>=0);
    INPUT_CHECK("pen-open-readonly",(c.pen=open("/dev/input/event1",O_RDONLY|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC))>=0);
    INPUT_CHECK("touch-character-device",fstat(c.input,&input)==0 && S_ISCHR(input.st_mode));
    INPUT_CHECK("pen-character-device",fstat(c.pen,&pen)==0 && S_ISCHR(pen.st_mode));
    c.lease.input_dev=input.st_dev;c.lease.input_ino=input.st_ino;c.lease.input_rdev=input.st_rdev;
    c.lease.pen_dev=pen.st_dev;c.lease.pen_ino=pen.st_ino;c.lease.pen_rdev=pen.st_rdev;
    INPUT_CHECK("touch-id",ioctl(c.input,EVIOCGID,&c.lease.input_id)==0);
    INPUT_CHECK("touch-identity",same_input(&c));
    INPUT_CHECK("pen-key-released",(keys=pen_released_observed(&c,&pen_state)));
    INPUT_CHECK("slot-range",ioctl(c.input,EVIOCGABS(ABS_MT_SLOT),&slot)==0 &&
        slot.minimum==0 && slot.maximum==SLOT_COUNT-1 && slot.value>=0 && slot.value<SLOT_COUNT);
    INPUT_CHECK("tracking-range",ioctl(c.input,EVIOCGABS(ABS_MT_TRACKING_ID),&tracking)==0 &&
        tracking.minimum<=1 && tracking.maximum>=1 && tracking.maximum<=65535);
    INPUT_CHECK("x-range",ioctl(c.input,EVIOCGABS(ABS_MT_POSITION_X),&x)==0 && x.minimum==0 && x.maximum==1403);
    INPUT_CHECK("y-range",ioctl(c.input,EVIOCGABS(ABS_MT_POSITION_Y),&y)==0 && y.minimum==0 && y.maximum==1871);
    INPUT_CHECK("tracking-slots",(ids=axis_values(c.input,ABS_MT_TRACKING_ID,state.id)));
    INPUT_CHECK("x-slots",(xs=axis_values(c.input,ABS_MT_POSITION_X,state.x)));
    INPUT_CHECK("y-slots",(ys=axis_values(c.input,ABS_MT_POSITION_Y,state.y)));
    INPUT_CHECK("tracking-slots-stable",(stable=axis_values(c.input,ABS_MT_TRACKING_ID,again) &&
        ids && !memcmp(again,state.id,sizeof again)));
    state.selected=slot.value;int empty=ids && stable && owned_state(&c.lease,&state)==0;
    INPUT_CHECK("touch-released",empty);
#undef INPUT_CHECK
    int good=all && keys && empty && xs && ys;
    printf("],\"status\":\"%s\",\"selected_slot\":%d,\"tracking_ids\":[",
        good?"input-readonly-check-released":"input-readonly-check-refused",slot.value);
    for(int i=0;i<SLOT_COUNT;++i)printf("%s%d",i?",":"",ids?state.id[i]:-2);
    printf("],\"axis_ranges\":{\"tracking\":[%d,%d],\"x\":[%d,%d],\"y\":[%d,%d]},"
        "\"pen_keys\":{\"ioctl_return\":%d,\"errno\":%d,\"BTN_TOUCH\":%d,\"BTN_TOOL_PEN\":%d,\"BTN_TOOL_RUBBER\":%d},"
        "\"input_events\":0,\"event_reads\":0,\"tap_ready\":false}\n",
        tracking.minimum,tracking.maximum,x.minimum,x.maximum,y.minimum,y.maximum,
        pen_state.query_return,pen_state.query_errno,pen_state.touch,pen_state.tool_pen,pen_state.tool_rubber);
    if(c.input>=0)close(c.input);
    if(c.pen>=0)close(c.pen);
    return good && !ferror(stdout) && fflush(stdout)==0?0:90;
}
int main(int argc,char **argv) {
    if(argc==2 && !strcmp(argv[1],"--check-input"))return check_input();
    if(argc==2 && !strcmp(argv[1],"--check-pidfd"))return check_pidfd();
    if(argc==5 && (!strcmp(argv[1],"--tap") || !strcmp(argv[1],"--tap-close"))) {
        if(getuid()!=0 || !hex_string(argv[2],0) || !hex_string(argv[3],1))return 90;
        return supervise(argc,argv);
    }
    return run_guardian(argc,argv);
}
