/* Test-only I/O controls around the exact production source. Never target-build. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
static ssize_t fixture_write(int,const void *,size_t);
static int fixture_puts(const char *);
static long fixture_syscall(long,...);
#define write fixture_write
#define puts fixture_puts
#define syscall fixture_syscall
#define main publisher_main
#include "publish_creation_arm.c"
#undef main
#undef syscall
#undef puts
#undef write
static unsigned writes;
static ssize_t fixture_write(int fd,const void *bytes,size_t n) {
    ++writes;
    if(getenv("TEST_WRITE_FAIL") && writes>1){errno=EIO;return -1;}
    if(getenv("TEST_SHORT_WRITE")||getenv("TEST_WRITE_FAIL"))n=n>1?n/2:1;
    if(getenv("TEST_SWAP_ROOT")&&writes==1) {
        char old[160];snprintf(old,sizeof old,"%s-old",rootpath);
        if(rename(rootpath,old)!=0||mkdir(rootpath,0700)!=0)return -1;
    }
    return write(fd,bytes,n);
}
static int fixture_puts(const char *s) {
    if(getenv("TEST_RECEIPT_FAIL")){errno=EIO;return EOF;}
    return puts(s);
}
static long fixture_syscall(long number,...) {
    if(getenv("TEST_UNSUPPORTED")){errno=ENOSYS;return -1;}
    va_list args;va_start(args,number);
    int a=va_arg(args,int);const char *b=va_arg(args,const char *);
    int c=va_arg(args,int);const char *d=va_arg(args,const char *);int flags=va_arg(args,int);
    va_end(args);
    return syscall(number,a,b,c,d,flags);
}
int main(int argc,char **argv){return publisher_main(argc,argv);}
