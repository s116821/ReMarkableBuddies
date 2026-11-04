#ifndef E0T_IPC_SOCKET_H
#define E0T_IPC_SOCKET_H
/* Fixed owned endpoint preparation, no server/ACK/effects/lease.
 * Caller is single-threaded and owns immutable root/parent paths. Failed bind
 * setup never removes a possibly created endpoint; exact cleanup is external.
 * Linux-reported socket buffer expectations remain an actual target gate.
 */
#include "intent_file.h"
#include <sys/socket.h>
#include <sys/un.h>
#ifndef E0T_RUNTIME_PARENT
#define E0T_RUNTIME_PARENT "/run"
#endif
#define E0T_REQUEST_SOCKET E0T_RUNTIME_PARENT "/buddy-e0t-" E0T_NONCE "/request.sock"
static int e0t_ipc_same_root(int root) {
    if(!e0t_intent_root(root)) return 0;
    int path=open(E0T_RUNTIME_PARENT "/buddy-e0t-" E0T_NONCE,O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(path<0) return 0;
    struct stat supplied, expected;
    int same=!fstat(root,&supplied) && !fstat(path,&expected)
        && supplied.st_dev==expected.st_dev && supplied.st_ino==expected.st_ino;
    if(close(path)) same=0;
    return same;
}
static int e0t_ipc_bind(int root,struct stat *published) {
    if(!published || !e0t_ipc_same_root(root)) return -1;
    struct stat initial;
    if(!fstatat(root,"request.sock",&initial,AT_SYMLINK_NOFOLLOW) || errno!=ENOENT) return -1;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    if(sizeof(E0T_REQUEST_SOCKET)>sizeof(address.sun_path)) return -1;
    memcpy(address.sun_path,E0T_REQUEST_SOCKET,sizeof(E0T_REQUEST_SOCKET));
    int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC,0);
    if(fd<0) return -1;
    int one=1, requested=4096, actual; socklen_t size=sizeof(actual);
    if(setsockopt(fd,SOL_SOCKET,SO_PASSCRED,&one,sizeof(one))
       || setsockopt(fd,SOL_SOCKET,SO_RCVBUF,&requested,sizeof(requested))
       || setsockopt(fd,SOL_SOCKET,SO_SNDBUF,&requested,sizeof(requested))
       || getsockopt(fd,SOL_SOCKET,SO_RCVBUF,&actual,&size) || size!=sizeof(actual) || actual!=8192) {
        close(fd); return -1;
    }
    size=sizeof(actual);
    if(getsockopt(fd,SOL_SOCKET,SO_SNDBUF,&actual,&size) || size!=sizeof(actual) || actual!=8192) {
        close(fd); return -1;
    }
    mode_t previous=umask(0177);
    int bound=bind(fd,(struct sockaddr *)&address,sizeof(address));
    umask(previous);
    struct stat endpoint;
    if(bound || !e0t_ipc_same_root(root) || fstatat(root,"request.sock",&endpoint,AT_SYMLINK_NOFOLLOW)
       || !S_ISSOCK(endpoint.st_mode) || endpoint.st_nlink!=1 || endpoint.st_uid!=geteuid()
       || (endpoint.st_mode&07777)!=0600) { close(fd); return -1; }
    *published=endpoint; return fd;
}
#endif
