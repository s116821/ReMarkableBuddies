/* Disposable Linux launcher child: never a tablet helper. */
#define _POSIX_C_SOURCE 200809L
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(int argc,char **argv) {
    struct sigaction action;sigset_t mask;
    if(sigaction(SIGALRM,NULL,&action)||sigprocmask(SIG_SETMASK,NULL,&mask))return 99;
    unsigned remaining=alarm(0);alarm(remaining);
    printf("%ld %d %u %d %d\n",(long)getpid(),argc,remaining,
        action.sa_handler==SIG_DFL,sigismember(&mask,SIGALRM));
    for(int i=0;i<argc;++i)printf("%s\n",argv[i]);
    fflush(stdout);
    if(getenv("ALARM_CHILD_WAIT"))for(;;)pause();
    const char *exit_code=getenv("ALARM_CHILD_EXIT");
    return exit_code?atoi(exit_code):0;
}
