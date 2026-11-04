/* Plain filesystem permission publisher; no input, Qt, service, or native call.
 * A syscall success means published only. Never retry this invocation.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/fs.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#ifndef SYS_renameat2
#error "Target syscall headers must provide SYS_renameat2"
#endif
#ifndef RENAME_NOREPLACE
#error "Target filesystem headers must provide RENAME_NOREPLACE"
#endif

static int rootfd = -1;
static struct stat rootstat, tmpstat;
static char rootpath[128], token[128], waiting[128], window[160];
static const char *nonce, *document;
static long long pid, start_ticks, expiry, armed, deadline, candidate;
static int production_v2;

static int number(const char *s, long long *out) {
    char *end;
    if (!s[0] || (s[0] == '0' && s[1])) return 0;
    for (const char *p=s; *p; ++p) if (*p<'0'||*p>'9') return 0;
    errno=0; *out=strtoll(s,&end,10);
    return errno==0 && !*end && *out>0;
}
static int hexid(const char *s, int uuid) {
    size_t n=strlen(s); if (n!=(uuid?36:32)) return 0;
    int nonzero=0;
    for (size_t i=0;i<n;i++) {
        if (uuid && (i==8||i==13||i==18||i==23)) { if(s[i]!='-')return 0; continue; }
        if (!((s[i]>='0'&&s[i]<='9')||(s[i]>='a'&&s[i]<='f')))return 0;
        if(s[i]!='0')nonzero=1;
    }
    return nonzero;
}
static int private_stat(const struct stat *s) {
    return S_ISREG(s->st_mode) && s->st_uid==0 &&
        (s->st_mode&07777)==0600 && s->st_nlink==1;
}
static int unchanged(const struct stat *a,const struct stat *b) {
    return a->st_dev==b->st_dev && a->st_ino==b->st_ino && a->st_size==b->st_size &&
        a->st_mtim.tv_sec==b->st_mtim.tv_sec && a->st_mtim.tv_nsec==b->st_mtim.tv_nsec;
}
static int read_full(int fd,char *b,size_t n) {
    size_t done=0; while(done<n) {
        ssize_t k=read(fd,b+done,n-done);
        if(k<0&&errno==EINTR)continue;
        if(k<=0)return 0;
        done+=(size_t)k;
    }
    char extra; return read(fd,&extra,1)==0;
}
static int exact_file(const char *name,const char *bytes) {
    int fd=openat(rootfd,name,O_RDONLY|O_NOFOLLOW|O_CLOEXEC);
    if(fd<0)return 0;
    struct stat a,b; char buffer[160]; size_t n=strlen(bytes);
    int ok=n<=sizeof buffer && fstat(fd,&a)==0 && private_stat(&a) && a.st_size==(off_t)n &&
        read_full(fd,buffer,n) && !memcmp(buffer,bytes,n) && fstat(fd,&b)==0 &&
        private_stat(&b) && unchanged(&a,&b);
    close(fd); return ok;
}
static int root_same(void) {
    struct stat s; int fd=open(rootpath,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(fd<0)return 0;
    int ok=fstat(fd,&s)==0 && S_ISDIR(s.st_mode) && s.st_uid==0 &&
        (s.st_mode&07777)==0700 && s.st_dev==rootstat.st_dev && s.st_ino==rootstat.st_ino;
    close(fd); return ok;
}
static int open_admission(void) {
    static const char *names[]={"restore.claim","entry.closed","stop.request","stop.admitted","diagnostics.json","callback.json"};
    struct stat s;
    for(size_t i=0;i<(production_v2?sizeof(names)/sizeof(*names):4);i++) {
        if(fstatat(rootfd,names[i],&s,AT_SYMLINK_NOFOLLOW)==0 || errno!=ENOENT)return 0;
    }
    return 1;
}
static int process_same(void) {
    char path[64],buffer[4096]; snprintf(path,sizeof path,"/proc/%lld/stat",pid);
    FILE *f=fopen(path,"r"); if(!f)return 0;
    size_t n=fread(buffer,1,sizeof(buffer)-1,f); int ok=!ferror(f)&&feof(f); fclose(f);
    if(!ok)return 0;
    buffer[n]=0;
    char *p=strrchr(buffer,')'); if(!p||p[1]!=' ')return 0;
    p+=2; char *save=NULL,*field=strtok_r(p," ",&save);
    for(int i=1;i<20&&field;i++)field=strtok_r(NULL," ",&save);
    long long value; return field && number(field,&value) && value==start_ticks;
}
static int time_allowed(void) {
    struct timespec t;
    if(clock_gettime(CLOCK_BOOTTIME,&t)!=0 || t.tv_sec<0 || t.tv_nsec<0 ||
       t.tv_nsec>=1000000000)return 0;
    long long ms;
    if(__builtin_mul_overflow((long long)t.tv_sec,1000LL,&ms) ||
       __builtin_add_overflow(ms,(long long)(t.tv_nsec/1000000),&ms) ||
       ms>LLONG_MAX-999)return 0;
    long long upper_sec=(ms+999)/1000;
    if(production_v2) {
        long long remaining=candidate+130-upper_sec;
        return ms>=candidate*1000 && ms<expiry && upper_sec<candidate+90 &&
            remaining>=40 && deadline-upper_sec>=225 &&
            deadline-upper_sec>=remaining+185;
    }
    return ms>=candidate*1000 && ms<expiry && upper_sec<candidate+40 && deadline-upper_sec>=215;
}
static int sidecar_same(const char *ext) {
    char saved[64],live[256]; snprintf(saved,sizeof saved,"host-source.%s",ext);
    snprintf(live,sizeof live,"/home/root/.local/share/remarkable/xochitl/%s.%s",document,ext);
    int a=openat(rootfd,saved,O_RDONLY|O_NOFOLLOW|O_CLOEXEC);
    int b=open(live,O_RDONLY|O_NOFOLLOW|O_CLOEXEC);
    struct stat sa,sb,fa,fb; int ok=a>=0&&b>=0;
    if(ok)ok=fstat(a,&sa)==0&&private_stat(&sa)&&sa.st_size>=0&&sa.st_size<=65536 &&
        fstat(b,&sb)==0&&S_ISREG(sb.st_mode)&&sb.st_uid==0&&sb.st_nlink==1&&sa.st_size==sb.st_size;
    if(ok) {
        char ba[65536],bb[65536]; size_t n=(size_t)sa.st_size;
        ok=read_full(a,ba,n)&&read_full(b,bb,n)&&!memcmp(ba,bb,n)&&
            fstat(a,&fa)==0&&private_stat(&fa)&&unchanged(&sa,&fa)&&
            fstat(b,&fb)==0&&unchanged(&sb,&fb);
    }
    if(a>=0)close(a);
    if(b>=0)close(b);
    return ok;
}
static int fresh(void) {
    return root_same() && exact_file("owner",nonce) && open_admission() &&
        process_same() && exact_file("candidate.window",window) &&
        exact_file("creation-waiting",waiting) && time_allowed();
}
int main(int argc,char **argv) {
    /* nonce pid start expiry_ms armed_seconds deadline_seconds candidate_seconds doc_uuid */
    production_v2=argc==10 && !strcmp(argv[9],"open-document-insertion-v2");
    if((argc!=9 && !production_v2) || getuid()!=0 || !hexid(argv[1],0)||!hexid(argv[8],1))goto refused;
    nonce=argv[1]; document=argv[8];
    if(!number(argv[2],&pid)||pid<=1||!number(argv[3],&start_ticks)||!number(argv[4],&expiry)||
       !number(argv[5],&armed)||!number(argv[6],&deadline)||!number(argv[7],&candidate)||
       deadline-armed!=360||candidate<armed||deadline-candidate<(production_v2?315:255)||
       candidate>LLONG_MAX/1000-1000)goto refused;
    snprintf(rootpath,sizeof rootpath,"/run/rmb-qt-probe-%s",nonce);
    snprintf(waiting,sizeof waiting,"%s %lld %lld waiting %lld\n",nonce,pid,start_ticks,expiry);
    snprintf(token,sizeof token,"%s %lld %lld create %lld\n",nonce,pid,start_ticks,expiry);
    snprintf(window,sizeof window,"%s %lld %lld %lld %lld\n",nonce,armed,deadline,candidate,candidate+(production_v2?130:70));
    rootfd=open(rootpath,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(rootfd<0 || fstat(rootfd,&rootstat)!=0 || !root_same() || !fresh())goto refused;
    struct stat dest;
    if(fstatat(rootfd,"creation-arm",&dest,AT_SYMLINK_NOFOLLOW)==0 || errno!=ENOENT)goto refused;
    int fd=openat(rootfd,"creation-arm.tmp",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600);
    if(fd<0)goto refused;
    int ok=fchmod(fd,0600)==0;
    size_t done=0,n=strlen(token);
    while(ok&&done<n) {
        ssize_t k=write(fd,token+done,n-done);
        if(k<0&&errno==EINTR)continue;
        if(k<=0){ok=0;break;} done+=(size_t)k;
    }
    ok=ok&&fstat(fd,&tmpstat)==0&&private_stat(&tmpstat)&&tmpstat.st_size==(off_t)n&&fsync(fd)==0;
    if(close(fd)!=0)ok=0;
    if(!ok || !sidecar_same("content") || !sidecar_same("metadata") || !fresh())goto refused;
    struct stat current;
    if(fstatat(rootfd,"creation-arm.tmp",&current,AT_SYMLINK_NOFOLLOW)!=0||
       !private_stat(&current)||!unchanged(&tmpstat,&current)||!exact_file("creation-arm.tmp",token)||!fresh())goto refused;
    /* One atomic no-replace call; all errors/ambiguity refuse without fallback. */
    if(syscall(SYS_renameat2,rootfd,"creation-arm.tmp",rootfd,"creation-arm",RENAME_NOREPLACE)!=0)goto refused;
    if(puts("{\"published\":true,\"accepted\":\"not observed\",\"retry_permitted\":false}")<0) {
        close(rootfd); return 91; /* Published even if receipt output failed. */
    }
    close(rootfd); return 0;
refused:
    if(rootfd>=0)close(rootfd);
    fputs("creation permission refused or publication uncertain; never retry\n",stderr);
    return 90;
}
