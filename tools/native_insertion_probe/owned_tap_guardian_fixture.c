/* Exact-source fault fixture. Fake evdev/boot clock only; disposable root
 * files are real. NEVER build this file as a tablet helper. */
#define _GNU_SOURCE
#include <assert.h>
#include <fcntl.h>
#include <stdarg.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <linux/input.h>
static int fake_clock(clockid_t,struct timespec *);
static int fake_ioctl(int,unsigned long,...);
static int fake_fstat(int,struct stat *);
static int fake_stat(const char *,struct stat *);
static ssize_t fake_read(int,void *,size_t);
static ssize_t fake_write(int,const void *,size_t);
static int fake_fsync(int);
static int fake_open(const char *,int,...);
static int fake_close(int);
static int fake_sleep(const struct timespec *,struct timespec *);
#define open fake_open
#define close fake_close
#define nanosleep fake_sleep
#define fsync fake_fsync
#define clock_gettime fake_clock
#define ioctl fake_ioctl
#define fstat fake_fstat
#define stat(...) fake_stat(__VA_ARGS__)
#define read fake_read
#define write fake_write
#define main guardian_main
#include "owned_tap_guardian.c"
#undef main
#undef nanosleep
#undef close
#undef open
#undef fsync
#undef write
#undef read
#undef stat
#undef fstat
#undef ioctl
#undef clock_gettime

static int64_t clock_now=1100;
static struct contact_state local_kernel,*kernel_pointer=&local_kernel;
#define kernel (*kernel_pointer)
static struct input_event queue[128];
static size_t queued;
static int writes,down_prefix=-1,up_prefix=-1,source_lost,pen_active;
static struct context *intent_context;
static int intent_fault, omit_syn, truncated_echo;
static struct context *flow_context;
static int delayed_go, stop_on_down, crash_after_release, late_supervisor;
static int fake_open(const char *path,int flags,...) {
    mode_t mode=0;if(flags&O_CREAT){va_list args;va_start(args,flags);mode=va_arg(args,int);va_end(args);}
    if(flow_context) {
        if(!strcmp(path,"/dev/input/event2"))return 100;
        if(!strcmp(path,"/dev/input/event1"))return 101;
        if(!strcmp(path,"/etc/hwrevision"))return openat(flow_context->root,"model",flags);
        if(!strcmp(path,"/etc/os-release"))return openat(flow_context->root,"release",flags);
        if(!strncmp(path,"/run/rmb-qt-probe-",18))return open(flow_context->path,flags,mode);
    }
    return open(path,flags,mode);
}
static int fake_close(int fd){
    if(crash_after_release) {
        char descriptor[64],path[256];snprintf(descriptor,sizeof descriptor,"/proc/self/fd/%d",fd);
        ssize_t n=readlink(descriptor,path,sizeof path-1);
        if(n>0){path[n]=0;if(strstr(path,"/tap.released"))raise(SIGKILL);}
    }
    return fd==100||fd==101?0:close(fd);
}
static int fake_sleep(const struct timespec *duration,struct timespec *remaining) {
    if(!flow_context)return nanosleep(duration,remaining);
    clock_now+=duration->tv_sec*1000+duration->tv_nsec/1000000;
    if(late_supervisor && kernel.id[0]>=0)clock_now=5000;
    if(delayed_go && clock_now>=1800) {
        char token[34];snprintf(token,sizeof token,"%s\n",flow_context->lease.nonce);
        assert(exclusive_record(flow_context,"tap.go",token,33));delayed_go=0;
    }
    /* Yield to the real child/parent even though BOOTTIME is controlled. */
    struct timespec tiny={0,1000000};return nanosleep(&tiny,remaining);
}
static int fake_fsync(int fd) {
    int result=fsync(fd);
    if(intent_context && fd==intent_context->history && intent_fault) {
        struct context *c=intent_context;int fault=intent_fault;intent_fault=0;
        if(fault==1)clock_now=c->lease.expires;
        if(fault==2)assert(exclusive_record(c,"restore.claim","x",1));
        if(fault==3) {
            assert(renameat(c->root,"tap.lease",c->root,"old-lease")==0);
            assert(exclusive_record(c,"tap.lease",&c->lease,sizeof c->lease));
        }
    }
    return result;
}
static int fake_clock(clockid_t clock,struct timespec *t) {
    if(clock!=CLOCK_BOOTTIME)return clock_gettime(clock,t);
    if(clock_now<0){errno=EIO;return -1;}
    t->tv_sec=clock_now/1000;t->tv_nsec=(clock_now%1000)*1000000;return 0;
}
static void fake_identity(int fd,struct stat *s) {
    memset(s,0,sizeof *s);s->st_mode=S_IFCHR|0600;s->st_ino=(ino_t)fd;s->st_dev=1;s->st_rdev=(dev_t)fd;
}
static int fake_fstat(int fd,struct stat *s) {
    if(fd==100||fd==101){if(source_lost){errno=ENODEV;return -1;}fake_identity(fd,s);return 0;}
    return fstat(fd,s);
}
static int fake_stat(const char *path,struct stat *s) {
    if(!strcmp(path,"/dev/input/event2")||!strcmp(path,"/dev/input/event1")) {
        if(source_lost){errno=ENOENT;return -1;}
        fake_identity(!strcmp(path,"/dev/input/event2")?100:101,s);return 0;
    }
    return stat(path,s);
}
static int fake_ioctl(int fd,unsigned long request,...) {
    va_list args;va_start(args,request);void *p=va_arg(args,void *);va_end(args);
    if(source_lost){errno=ENODEV;return -1;}
    if(fd==100 && request==EVIOCGID){memset(p,0,sizeof(struct input_id));return 0;}
    if(fd==100 && request==EVIOCGABS(ABS_MT_TRACKING_ID)) {
        struct input_absinfo *a=p;memset(a,0,sizeof *a);a->maximum=65535;return 0;
    }
    if(fd==100 && request==EVIOCGABS(ABS_MT_SLOT)) {
        struct input_absinfo *a=p;memset(a,0,sizeof *a);a->maximum=31;a->value=kernel.selected;return 0;
    }
    if(fd==100 && request==EVIOCGMTSLOTS(sizeof(int32_t)*(SLOT_COUNT+1))) {
        int32_t *a=p;int axis=*a;
        memcpy(a+1,axis==ABS_MT_TRACKING_ID?kernel.id:axis==ABS_MT_POSITION_X?kernel.x:kernel.y,sizeof kernel.id);
        return 0;
    }
    if(fd==101 && _IOC_NR(request)==_IOC_NR(EVIOCGKEY(1))) {
        memset(p,0,_IOC_SIZE(request));if(pen_active)((unsigned char *)p)[BTN_TOUCH/8]|=1u<<(BTN_TOUCH%8);return 0;
    }
    errno=EINVAL;return -1;
}
static ssize_t fake_read(int fd,void *p,size_t size) {
    if(fd!=100)return read(fd,p,size);
    if(source_lost){errno=ENODEV;return -1;}
    if(!queued){errno=EAGAIN;return -1;}
    if(truncated_echo){truncated_echo=0;memcpy(p,queue,sizeof(*queue)-1);queued=0;return sizeof(*queue)-1;}
    size_t n=queued*sizeof(*queue);if(n>size)n=size;
    memcpy(p,queue,n);size_t consumed=n/sizeof(*queue);queued-=consumed;
    memmove(queue,queue+consumed,queued*sizeof(*queue));return (ssize_t)n;
}
static ssize_t fake_write(int fd,const void *p,size_t size) {
    if(fd!=100)return write(fd,p,size);
    ++writes;const struct input_event *events=p;
    int down=events[1].value>=0;
    int prefix=down?down_prefix:up_prefix;
    size_t count=size/sizeof(*events),accepted=prefix<0?count:(size_t)prefix;
    if(accepted>count)accepted=count;
    for(size_t i=0;i<accepted;++i) {
        struct input_event e=events[i];assert(queued<128);
        if(!(omit_syn && e.type==EV_SYN))queue[queued++]=e;
        if(e.type!=EV_ABS)continue;
        if(e.code==ABS_MT_SLOT)kernel.selected=e.value;
        if(e.code==ABS_MT_TRACKING_ID)kernel.id[kernel.selected]=e.value;
        if(e.code==ABS_MT_POSITION_X)kernel.x[kernel.selected]=e.value;
        if(e.code==ABS_MT_POSITION_Y)kernel.y[kernel.selected]=e.value;
    }
    if(down && stop_on_down)raise(SIGSTOP);
    return (ssize_t)(accepted*sizeof(*events));
}
static struct context make_context(char *path) {
    struct context c={.root=-1,.lock=-1,.input=100,.pen=101,.history=-1};
    assert(mkdtemp(path));strcpy(c.path,path);c.root=open(path,O_RDONLY|O_DIRECTORY);assert(c.root>=0);
    c.history=openat(c.root,"history",O_WRONLY|O_CREAT|O_EXCL,0600);assert(c.history>=0);
    c.lock=openat(c.root,"tap.lock",O_RDWR|O_CREAT|O_EXCL,0600);assert(c.lock>=0);
    assert(flock(c.lock,LOCK_EX|LOCK_NB)==0);
    struct stat s;assert(fstat(c.root,&s)==0);
    c.lease.magic=MAGIC;c.lease.version=1;c.lease.contact=1;strcpy(c.lease.nonce,"11111111111111111111111111111111");
    char boot[64];assert(file_text("/proc/sys/kernel/random/boot_id",boot,sizeof boot));memcpy(c.lease.boot,boot,36);
    c.lease.root_dev=s.st_dev;c.lease.root_ino=s.st_ino;
    assert(fstat(c.lock,&s)==0);c.lease.lock_dev=s.st_dev;c.lease.lock_ino=s.st_ino;
    c.lease.input_dev=1;c.lease.input_ino=100;c.lease.input_rdev=100;
    c.lease.pen_dev=1;c.lease.pen_ino=101;c.lease.pen_rdev=101;
    c.lease.pid=getpid();c.lease.start=process_start(getpid());c.lease.born=1000;c.lease.expires=2500;
    c.lease.tracking=42;c.lease.x=NATIVE_X;c.lease.y=NATIVE_Y;
    assert(exclusive_record(&c,"owner",c.lease.nonce,32));
    assert(exclusive_record(&c,"tap.lease",&c.lease,sizeof c.lease));
    struct lease persisted;assert(read_exact_observed(c.root,"tap.lease",&persisted,sizeof persisted,&c.lease_stat));
    c.lease_bound=1;
    memset(&kernel,0,sizeof kernel);for(int i=0;i<32;++i)kernel.id[i]=-1;
    clock_now=1100;queued=0;writes=0;down_prefix=-1;up_prefix=-1;source_lost=0;pen_active=0;
    return c;
}
static void cleanup(struct context *c) {
    close(c->history);close(c->lock);close(c->root);
    /* Python runner owns/removes only this disposable temporary tree. */
}
int main(int argc,char **argv) {
    assert(argc==2);char path[]="/tmp/guardian-fixture-XXXXXX";
    struct context c=make_context(path);const char *mode=argv[1];
    if(!strcmp(mode,"success")) {
        assert(write_frame(&c,1));c.complete_down=1;
        assert(conditional_up(&c,0));assert(c.down_echo&&c.up_echo&&writes==2);
        char receipt_bytes[512];int fd=openat(c.root,"tap.released",O_RDONLY);assert(fd>=0);
        ssize_t n=read(fd,receipt_bytes,sizeof receipt_bytes-1);assert(n>0);receipt_bytes[n]=0;close(fd);
        assert(strstr(receipt_bytes,"guardian-released"));
    } else if(!strcmp(mode,"partial-down-exact")) {
        down_prefix=4;assert(!write_frame(&c,1));assert(conditional_up(&c,0));assert(writes==2&&kernel.id[0]==-1);
        struct stat s;assert(fstatat(c.root,"tap.released",&s,0)!=0);assert(fstatat(c.root,"tap.cleaned",&s,0)==0);
    } else if(!strcmp(mode,"partial-down-no-point")) {
        down_prefix=2;assert(!write_frame(&c,1));assert(!conditional_up(&c,0));assert(writes==1);
    } else if(!strcmp(mode,"partial-up")) {
        assert(write_frame(&c,1));c.complete_down=1;up_prefix=2;assert(!conditional_up(&c,0));assert(writes==2);
        struct stat s;assert(fstatat(c.root,"tap.released",&s,0)!=0);
    } else if(!strcmp(mode,"dropped")) {
        assert(write_frame(&c,1));queue[queued++]=event(EV_SYN,SYN_DROPPED,0);assert(!conditional_up(&c,0));assert(writes==1);
    } else if(!strcmp(mode,"other-contact")||!strcmp(mode,"replaced")||!strcmp(mode,"point-drift")) {
        assert(write_frame(&c,1));
        if(!strcmp(mode,"other-contact"))kernel.id[1]=88;
        if(!strcmp(mode,"replaced"))kernel.id[0]=43;
        if(!strcmp(mode,"point-drift"))kernel.x[0]++;
        assert(!conditional_up(&c,0));assert(writes==1);
    } else if(!strcmp(mode,"expired")||!strcmp(mode,"clock-failure")||!strcmp(mode,"source-loss")||!strcmp(mode,"pen-contact")) {
        assert(write_frame(&c,1));
        if(!strcmp(mode,"expired"))clock_now=2500;
        if(!strcmp(mode,"clock-failure"))clock_now=-1;
        if(!strcmp(mode,"source-loss"))source_lost=1;
        if(!strcmp(mode,"pen-contact"))pen_active=1;
        assert(!conditional_up(&c,0));assert(writes==1);
    } else if(!strcmp(mode,"empty")) {
        assert(conditional_up(&c,1));assert(writes==0);
    } else if(!strcmp(mode,"live-writer")) {
        assert(!old_writer_gone(&c.lease));assert(writes==0);
    } else if(!strcmp(mode,"dead-writer")) {
        pid_t child=fork();assert(child>=0);if(!child)_exit(0);
        int status;assert(waitpid(child,&status,0)==child);c.lease.pid=child;c.lease.start=1;
        assert(old_writer_gone(&c.lease));assert(writes==0);
    } else if(!strcmp(mode,"persistent-lock")) {
        int b=openat(c.root,"tap.lock",O_RDWR);assert(b>=0);assert(flock(b,LOCK_EX|LOCK_NB)!=0);
        assert(flock(c.lock,LOCK_UN)==0);assert(flock(b,LOCK_EX|LOCK_NB)==0);close(b);assert(writes==0);
    } else if(!strcmp(mode,"expired-empty")) {
        clock_now=2500;assert(!lease_valid(&c));assert(recover_observed(&c));assert(writes==0);
    } else if(!strcmp(mode,"expired-owned")) {
        kernel.id[0]=42;kernel.x[0]=NATIVE_X;kernel.y[0]=NATIVE_Y;
        clock_now=2500;assert(!recover_observed(&c));assert(writes==0);
    } else if(!strcmp(mode,"malformed")) {
        c.lease.magic=0;assert(!lease_valid(&c));assert(!conditional_up(&c,1));assert(writes==0);
    } else if(!strcmp(mode,"closing")) {
        assert(admission_open(&c));assert(write_frame(&c,1));c.complete_down=1;
        assert(exclusive_record(&c,"restore.claim","x",1));assert(!admission_open(&c));
        assert(conditional_up(&c,0));assert(writes==2);
    } else if(!strcmp(mode,"lease-replacement")) {
        assert(renameat(c.root,"tap.lease",c.root,"old-lease")==0);
        assert(exclusive_record(&c,"tap.lease",&c.lease,sizeof c.lease));
        assert(!lease_valid(&c));assert(!conditional_up(&c,1));assert(writes==0);
    } else if(!strcmp(mode,"takeover")) {
        kernel.id[0]=42;kernel.x[0]=NATIVE_X;kernel.y[0]=NATIVE_Y;
        assert(recover_observed(&c));assert(writes==1&&c.up_echo);
        struct stat s;assert(fstatat(c.root,"tap.recovered",&s,0)==0);assert(fstatat(c.root,"tap.released",&s,0)!=0);
    } else if(!strcmp(mode,"intent-expired")||!strcmp(mode,"intent-closing")||!strcmp(mode,"intent-replaced")) {
        intent_context=&c;
        intent_fault=!strcmp(mode,"intent-expired")?1:!strcmp(mode,"intent-closing")?2:3;
        assert(!write_frame(&c,1));assert(writes==0);
    } else if(!strcmp(mode,"up-no-syn")||!strcmp(mode,"truncated-echo")) {
        assert(write_frame(&c,1));assert(drain_history(&c,1));c.complete_down=1;
        if(!strcmp(mode,"up-no-syn"))omit_syn=1;else truncated_echo=1;
        assert(!conditional_up(&c,0));
        struct stat s;assert(fstatat(c.root,"tap.released",&s,0)!=0);
    } else if(!strcmp(mode,"delayed-go")||!strcmp(mode,"supervisor-normal")||!strcmp(mode,"supervisor-stopped")||
              !strcmp(mode,"supervisor-after-release-crash")||!strcmp(mode,"supervisor-expired")||
              !strcmp(mode,"pidfd-fence")) {
        flow_context=&c;
        assert(exclusive_record(&c,"model","reMarkable2 1.0\n",15));
        const char *release="IMG_VERSION=\"3.28.0.172\"\n";
        assert(exclusive_record(&c,"release",release,strlen(release)));
        assert(unlinkat(c.root,"tap.lease",0)==0);c.lease_bound=0;
        if(!strcmp(mode,"pidfd-fence")) {
            pid_t child=fork();assert(child>=0);
            if(!child){signal(SIGTERM,SIG_IGN);raise(SIGSTOP);for(;;)pause();}
            int status;assert(waitpid(child,&status,WUNTRACED)==child && WIFSTOPPED(status));
            c.lease.pid=child;c.lease.start=process_start(child);
            assert(exclusive_record(&c,"tap.lease",&c.lease,sizeof c.lease));
            assert(read_exact_observed(c.root,"tap.lease",&c.lease,sizeof c.lease,&c.lease_stat));c.lease_bound=1;
            char token[34];snprintf(token,sizeof token,"%s\n",c.lease.nonce);
            assert(exclusive_record(&c,"restore.claim",token,33));
            assert(fence_writer(&c,c.lease.nonce,c.lease.boot));assert(c.writer_fenced && writes==0);
            assert(waitpid(child,&status,0)==child && WIFSIGNALED(status) && WTERMSIG(status)==SIGKILL);
        } else if(!strcmp(mode,"delayed-go")) {
            delayed_go=1;assert(start_tap(&c,6000));assert(c.lease.born>=1800);
            assert(c.lease.expires==c.lease.born+LEASE_MS && writes==2);
        } else {
            kernel_pointer=mmap(NULL,sizeof *kernel_pointer,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_ANONYMOUS,-1,0);
            assert(kernel_pointer!=MAP_FAILED);*kernel_pointer=local_kernel;
            assert(flock(c.lock,LOCK_UN)==0);close(c.lock);c.lock=-1;
            assert(unlinkat(c.root,"tap.lock",0)==0);
            char token[34];snprintf(token,sizeof token,"%s\n",c.lease.nonce);
            assert(exclusive_record(&c,"tap.go",token,33));
            stop_on_down=!strcmp(mode,"supervisor-stopped")||!strcmp(mode,"supervisor-expired");
            crash_after_release=!strcmp(mode,"supervisor-after-release-crash");
            late_supervisor=!strcmp(mode,"supervisor-expired");
            char *args[]={"fixture","--tap",c.lease.nonce,c.lease.boot,"6000",NULL};
            int result=supervise(5,args);struct stat marker;
            if(stop_on_down||crash_after_release) {
                assert(result==90);
                assert(late_supervisor?kernel.id[0]>=0:kernel.id[0]==-1);
                assert(fstatat(c.root,"tap.supervisor-spent",&marker,0)==0);
                assert(fstatat(c.root,"tap.supervisor-complete",&marker,0)!=0);
                if(crash_after_release)assert(fstatat(c.root,"tap.released",&marker,0)==0);
            } else {
                assert(result==0 && kernel.id[0]==-1);
                assert(fstatat(c.root,"tap.supervisor-complete",&marker,0)==0);
            }
            assert(munmap(kernel_pointer,sizeof *kernel_pointer)==0);kernel_pointer=&local_kernel;
        }
    } else assert(0);
    cleanup(&c);printf("%s PASS temporary=%s\n",mode,path);return 0;
}
