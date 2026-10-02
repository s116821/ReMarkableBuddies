#ifndef E0T_IPC_RECEIVE_H
#define E0T_IPC_RECEIVE_H
/* One nonblocking authenticated datagram, no server/ACK/lease/effect authority.
 * Caller owns/binds the private socket and supplies a fresh immutable expected
 * PID/start identity plus a bounded live-identity checker. No sender address is
 * used for responses. Recheck phase/lease/fence before any future dispatch.
 */
#include "request_protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>
typedef int (*e0t_peer_live)(pid_t,unsigned long long);
/* 1 validated request; 0 no datagram; -1 refuse/discard output. */
static int e0t_ipc_receive(int fd,pid_t expected_pid,unsigned long long expected_start,
                          e0t_peer_live live,struct e0t_request *output) {
    if(!output || !live || expected_pid<=1 || !expected_start) return -1;
    int type=0, domain=0, credentials=0; socklen_t size=sizeof(int);
    if(getsockopt(fd,SOL_SOCKET,SO_TYPE,&type,&size) || size!=sizeof(int) || type!=SOCK_DGRAM) return -1;
    size=sizeof(int);
    if(getsockopt(fd,SOL_SOCKET,SO_DOMAIN,&domain,&size) || size!=sizeof(int) || domain!=AF_UNIX) return -1;
    size=sizeof(int);
    if(getsockopt(fd,SOL_SOCKET,SO_PASSCRED,&credentials,&size) || size!=sizeof(int) || credentials!=1) return -1;
    int flags=fcntl(fd,F_GETFL);
    if(flags<0 || !(flags&O_NONBLOCK) || live(expected_pid,expected_start)!=1) return -1;
    char data[65];
    union { struct cmsghdr alignment;
        unsigned char bytes[CMSG_SPACE(sizeof(struct ucred))+CMSG_SPACE(4*sizeof(int))]; } control;
    memset(&control,0,sizeof(control));
    struct iovec payload={data,sizeof(data)};
    struct msghdr message={0};
    message.msg_iov=&payload; message.msg_iovlen=1;
    message.msg_control=control.bytes; message.msg_controllen=sizeof(control.bytes);
    ssize_t length=recvmsg(fd,&message,MSG_DONTWAIT|MSG_TRUNC|MSG_CMSG_CLOEXEC);
    if(length<0) return errno==EAGAIN || errno==EWOULDBLOCK ? 0 : -1;
    unsigned count=0; int bad=0;
    for(struct cmsghdr *item=CMSG_FIRSTHDR(&message);item;item=CMSG_NXTHDR(&message,item)) {
        /* Even rejected/truncated messages may install SCM_RIGHTS descriptors.
         * Close every delivered one before returning; never retain peer FDs. */
        if(item->cmsg_level==SOL_SOCKET && item->cmsg_type==SCM_RIGHTS) {
            bad=1;
            if(item->cmsg_len>=CMSG_LEN(0)) {
                size_t bytes=item->cmsg_len-CMSG_LEN(0);
                for(size_t offset=0;offset+sizeof(int)<=bytes;offset+=sizeof(int)) {
                    int passed; memcpy(&passed,(unsigned char *)CMSG_DATA(item)+offset,sizeof(passed));
                    if(passed>=0) close(passed);
                }
            }
            continue;
        }
        if(item->cmsg_level!=SOL_SOCKET || item->cmsg_type!=SCM_CREDENTIALS
           || item->cmsg_len!=CMSG_LEN(sizeof(struct ucred)) || ++count!=1) { bad=1; continue; }
        struct ucred peer; memcpy(&peer,CMSG_DATA(item),sizeof(peer));
        if(peer.pid!=expected_pid || peer.uid!=geteuid() || peer.gid!=getegid()) bad=1;
    }
    struct e0t_request parsed;
    if(bad || length>64 || message.msg_flags&(MSG_TRUNC|MSG_CTRUNC) || length<=0
       || count!=1 || !e0t_request_parse(data,(size_t)length,&parsed)
       || live(expected_pid,expected_start)!=1) return -1;
    *output=parsed; return 1;
}
#endif
