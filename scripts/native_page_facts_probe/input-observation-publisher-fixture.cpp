#include "input-observation-end-publisher.h"
#include <QCoreApplication>
#include <sys/mman.h>
#include <cstdio>
using namespace input_observation_end;
static bool put(const QString &path,const QByteArray &bytes){
    QFile file(path);return file.open(QIODevice::WriteOnly|QIODevice::NewOnly) &&
        file.setPermissions(QFile::ReadOwner|QFile::WriteOwner) && file.write(bytes)==bytes.size();
}
int main(int argc,char **argv){
    QCoreApplication app(argc,argv);if(argc!=2)return 2;
    const QByteArray mode(argv[1]);
    if(!canonicalDecimal("18446744073709551615") || canonicalDecimal("18446744073709551616") ||
       canonicalDecimal("0001") || canonicalDecimal("0") || !canonicalDecimal("0",true,6) ||
       canonicalDecimal("000000",true,6) || canonicalDecimal(QByteArray(21,'9')))return 3;
    const QString root=QString::fromLocal8Bit(qgetenv("INPUT_ROOT"));
    const QString nonce=QStringLiteral("0123456789abcdef0123456789abcdef");
    if(root.isEmpty())return 4;
    struct stat st{};if(::stat(root.toUtf8().constData(),&st))return 5;
    Descriptor proc(::open("/proc/self",O_RDONLY|O_DIRECTORY|O_CLOEXEC));
    const QByteArray pid=QByteArray::number(::getpid()),started=startIdentity(proc.value);
    QList<QByteArray> fields{nonce.toLatin1(),pid,started,QByteArray::number(qulonglong(st.st_dev)),QByteArray::number(qulonglong(st.st_ino)),"waiting-input-observation","0","120000","main-dev-input-observation-120s"};
    if(mode=="zero")fields[2]="0";
    if(mode=="leading-zero")fields[1]='0'+pid;
    if(mode=="overflow")fields[2]="18446744073709551616";
    if(mode=="width")fields[3]=QByteArray(21,'9');
    if(mode=="elapsed-leading-zero")fields[6]="00";
    if(mode=="elapsed-boundary")fields[6]="120000";
    if(mode=="elapsed-last")fields[6]="119999";
    if(mode=="purpose")fields[5]="waiting-facts";
    const QByteArray payload("owned fixture; not an executable extension\n");
    if(!put(root+"/owner",nonce.toLatin1()) || !put(root+"/attempt.identity",pid+' '+started+'\n') || !put(root+"/payload.so",payload))return 6;
    auto ready=fields.join(' ')+'\n';if(mode=="oversize")ready=QByteArray(257,'x');
    if(mode=="symlink"){
        if(!put(root+"/ready-target",ready) || ::symlink("ready-target",(root+"/input-observation-ready").toUtf8().constData()))return 7;
    }else if(mode=="fifo"){
        if(::mkfifo((root+"/input-observation-ready").toUtf8().constData(),0600))return 8;
    }else if(!put(root+"/input-observation-ready",ready))return 9;
    if(mode=="mode")::chmod((root+"/input-observation-ready").toUtf8().constData(),0644);
    if(mode=="closed")put(root+"/entry.closed","closed");
    if(mode=="restore")put(root+"/restore.claim","restore");
    if(mode=="cross-facts")put(root+"/facts-request","facts");
    if(mode=="existing-end")put(root+"/input-observation-end","spent");
    if(mode=="existing-tmp")put(root+"/input-observation-end.tmp","spent");
    Descriptor data(::open((root+"/payload.so").toUtf8().constData(),O_RDONLY|O_CLOEXEC));
    void *mapping=::mmap(nullptr,size_t(payload.size()),PROT_READ,MAP_PRIVATE,data.value,0);
    if(mapping==MAP_FAILED)return 10;
    Descriptor exe(::open("/proc/self/exe",O_RDONLY|O_CLOEXEC));
    const int result=input_observation_end::publish(root,nonce,digest(exe.value,64*1024*1024),digest(data.value,16*1024*1024));
    const bool success=mode=="good" || mode=="elapsed-last";
    if((success && result!=0) || (!success && result!=1))return 11;
    if(success && input_observation_end::publish(root,nonce,digest(exe.value,64*1024*1024),digest(data.value,16*1024*1024))!=1)return 12;
    ::munmap(mapping,size_t(payload.size()));
    std::printf("observation-publisher-%s: PASS (owned host fixture only)\n",argv[1]);return 0;
}
