#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <linux/input.h>
#include <stdarg.h>
#include <string.h>
/* Source-executed Rust ioctl seam, private sentinel FD only; no input devices. */
static int mode, slot_calls, id_calls;
void owned_fixture_reset(int value) { mode=value;slot_calls=0;id_calls=0; }
int ioctl(int fd,unsigned long request,...) {
    va_list args;va_start(args,request);void *value=va_arg(args,void *);va_end(args);
    if(fd!=-137) {
        int (*real)(int,unsigned long,...)=dlsym(RTLD_NEXT,"ioctl");
        if(!real){errno=ENOSYS;return -1;}return real(fd,request,value);
    }
    if(request==EVIOCGABS(ABS_MT_SLOT)) {
        struct input_absinfo *a=value;memset(a,0,sizeof(*a));slot_calls++;
        if(mode==1){errno=EIO;return -1;}
        a->maximum=mode==2?30:31;a->value=mode==3&&slot_calls==2?1:0;return 0;
    }
    if(request==EVIOCGABS(ABS_MT_TRACKING_ID)) {
        struct input_absinfo *a=value;memset(a,0,sizeof(*a));
        a->value=-1;a->minimum=0;a->maximum=65535;return 0;
    }
    if(request==EVIOCGMTSLOTS(33*sizeof(int))) {
        int *a=value;int axis=a[0];
        if(axis==ABS_MT_TRACKING_ID)id_calls++;
        if(axis!=ABS_MT_TRACKING_ID&&axis!=ABS_MT_POSITION_X&&axis!=ABS_MT_POSITION_Y){
            errno=EINVAL;return -1;
        }
        for(int i=0;i<32;i++)a[i+1]=axis==ABS_MT_TRACKING_ID?-1:
            axis==ABS_MT_POSITION_X?100+i:936+i;
        if(mode==4&&axis==ABS_MT_TRACKING_ID&&id_calls==2)a[1]=99;
        return 0;
    }
    /* Wrong hand-written request numbers must fail instead of returning test data. */
    errno=ENOTTY;return -1;
}
