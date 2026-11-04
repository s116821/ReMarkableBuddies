#pragma once
#include <QByteArray>
#include <QString>
#include <QFileInfo>
#include <QDir>
#include <QCryptographicHash>
#include <QRegularExpression>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/file.h>
#include <unistd.h>

namespace facts_request {
struct Descriptor {
    int value=-1;
    explicit Descriptor(int fd):value(fd){}
    ~Descriptor(){if (value>=0) ::close(value);}
    Descriptor(const Descriptor &)=delete;
};
inline QByteArray readBounded(int fd,int cap) {
    if (fd<0) return {};
    QByteArray bytes;
    char buffer[4096];
    while (bytes.size()<=cap) {
        const ssize_t length=::read(fd,buffer,sizeof buffer);
        if (length<0) return {};
        if (!length) return bytes;
        bytes.append(buffer,int(length));
    }
    return {};
}
inline QByteArray privateFile(int root,const char *name,int cap) {
    Descriptor file(::openat(root,name,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC));
    struct stat before{},after{};
    if (file.value<0 || ::fstat(file.value,&before)!=0 || !S_ISREG(before.st_mode) ||
        before.st_uid!=::geteuid() || (before.st_mode&0777)!=0600 || before.st_size<1 || before.st_size>cap) return {};
    const auto value=readBounded(file.value,cap);
    if (::fstat(file.value,&after)!=0 || before.st_dev!=after.st_dev || before.st_ino!=after.st_ino ||
        before.st_size!=after.st_size || before.st_mode!=after.st_mode || before.st_uid!=after.st_uid ||
        value.size()!=before.st_size) return {};
    return value;
}
inline QByteArray digest(int fd,qint64 cap) {
    if (fd<0) return {};
    struct stat before{},after{};
    if (::fstat(fd,&before)!=0 || !S_ISREG(before.st_mode) || before.st_size<1 || before.st_size>cap) return {};
    QCryptographicHash hash(QCryptographicHash::Sha256);
    qint64 total=0; char buffer[16384];
    while (total<=cap) {
        const ssize_t length=::read(fd,buffer,sizeof buffer);
        if (length<0) return {};
        if (!length) break;
        total+=length; hash.addData(QByteArrayView(buffer,int(length)));
    }
    if (total!=before.st_size || total>cap || ::fstat(fd,&after)!=0 || before.st_dev!=after.st_dev ||
        before.st_ino!=after.st_ino || before.st_size!=after.st_size) return {};
    return hash.result().toHex();
}
inline bool absent(int root,const char *name) {
    struct stat st{};
    return ::fstatat(root,name,&st,AT_SYMLINK_NOFOLLOW)!=0 && errno==ENOENT;
}
inline QByteArray startIdentity(int process) {
    Descriptor file(::openat(process,"stat",O_RDONLY|O_NOFOLLOW|O_CLOEXEC));
    const auto text=readBounded(file.value,2048); const int end=text.lastIndexOf(')');
    if (end<0) return {};
    return text.mid(end+2).simplified().split(' ').value(19);
}
// One fixed file publication. No fork, shell, service, input or arbitrary effect.
// Returns 0 only for the token publication, never for native observation success.
inline int publish(const QString &directory,const QString &nonce,const QByteArray &executableHash,const QByteArray &payloadHash) {
    const QRegularExpression noncePattern(QStringLiteral("^[0-9a-f]{32}$"));
    const QRegularExpression hashPattern(QStringLiteral("^[0-9a-f]{64}$"));
    if (!noncePattern.match(nonce).hasMatch() || !hashPattern.match(QString::fromLatin1(executableHash)).hasMatch() ||
        !hashPattern.match(QString::fromLatin1(payloadHash)).hasMatch() || !QDir::isAbsolutePath(directory) ||
        QDir::cleanPath(directory)!=directory || QFileInfo(directory).fileName()!=QStringLiteral("rmb-qt-probe-")+nonce) return 1;
    Descriptor root(::open(directory.toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC));
    struct stat rootStat{};
    if (root.value<0 || ::fstat(root.value,&rootStat)!=0 || rootStat.st_uid!=::geteuid() || (rootStat.st_mode&0777)!=0700) return 1;
    Descriptor lock(::openat(root.value,"admission.lock",O_RDWR|O_CREAT|O_NOFOLLOW|O_CLOEXEC,0600));
    struct stat lockStat{};
    if (lock.value<0 || ::fstat(lock.value,&lockStat)!=0 || !S_ISREG(lockStat.st_mode) ||
        lockStat.st_uid!=::geteuid() || (lockStat.st_mode&0777)!=0600 || ::flock(lock.value,LOCK_EX|LOCK_NB)!=0) return 1;
    const auto waiting=privateFile(root.value,"facts-waiting",256);
    const auto fields=waiting.trimmed().split(' ');
    if (fields.size()!=9 || fields[0]!=nonce.toLatin1() || fields[5]!="waiting-facts" ||
        fields[7]!="120000" || fields[8]!="main-dev-facts-120s" || waiting!=fields.join(' ')+'\n') return 1;
    for (int index:{1,2,3,4,6}) {
        if (fields[index].isEmpty()) return 1;
        for (const char digit:fields[index]) if (digit<'0' || digit>'9') return 1;
    }
    bool pidOk=false,devOk=false,inoOk=false,timeOk=false;
    const int pid=fields[1].toInt(&pidOk);
    const auto dev=fields[3].toULongLong(&devOk),ino=fields[4].toULongLong(&inoOk);
    const qint64 waitingAt=fields[6].toLongLong(&timeOk);
    if (!pidOk || pid<=1 || !devOk || !inoOk || dev!=qulonglong(rootStat.st_dev) || ino!=qulonglong(rootStat.st_ino) ||
        !timeOk || waitingAt<0 || waitingAt>=120000) return 1;
    Descriptor process(::open((QStringLiteral("/proc/")+QString::number(pid)).toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC));
    if (process.value<0) return 1;
    const auto current=[&]{
        Descriptor fresh(::open(directory.toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC));
        struct stat st{};
        return fresh.value>=0 && ::fstat(fresh.value,&st)==0 && st.st_uid==::geteuid() && (st.st_mode&0777)==0700 &&
            st.st_dev==rootStat.st_dev && st.st_ino==rootStat.st_ino &&
            privateFile(root.value,"owner",32)==nonce.toLatin1() && privateFile(root.value,"facts-waiting",256)==waiting &&
            privateFile(root.value,"attempt.identity",128)==fields[1]+' '+fields[2]+'\n' && startIdentity(process.value)==fields[2] &&
            absent(root.value,"entry.closed") && absent(root.value,"restore.claim") && absent(root.value,"callback.json");
    };
    if (!current() || !absent(root.value,"facts-request") || !absent(root.value,"facts-request.tmp")) return 1;
    // Following proc's executable link is intentional and hash-bound. Session
    // files themselves are always no-follow and held under the pinned directory.
    Descriptor executable(::openat(process.value,"exe",O_RDONLY|O_CLOEXEC));
    Descriptor payload(::openat(root.value,"payload.so",O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC));
    struct stat payloadStat{};
    if (payload.value<0 || ::fstat(payload.value,&payloadStat)!=0 || payloadStat.st_uid!=::geteuid() ||
        (payloadStat.st_mode&0777)!=0600 || digest(executable.value,64*1024*1024)!=executableHash ||
        digest(payload.value,16*1024*1024)!=payloadHash) return 1;
    Descriptor maps(::openat(process.value,"maps",O_RDONLY|O_NOFOLLOW|O_CLOEXEC));
    Descriptor environment(::openat(process.value,"environ",O_RDONLY|O_NOFOLLOW|O_CLOEXEC));
    const auto mapping=readBounded(maps.value,1024*1024);
    const auto env=readBounded(environment.value,65536);
    const QByteArray payloadPath=(directory+QStringLiteral("/payload.so")).toUtf8();
    bool mapped=false;
    for (const auto &line:mapping.split('\n')) if (line.endsWith(' '+payloadPath)) mapped=true;
    if (!mapped || !env.split('\0').contains(QByteArray("LD_PRELOAD=")+payloadPath) || !current()) return 1;
    const QByteArray token=fields.mid(0,5).join(' ')+" read-facts "+fields[7]+' '+fields[8]+'\n';
    if (token.size()>128) return 1;
    Descriptor temporary(::openat(root.value,"facts-request.tmp",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600));
    struct stat held{},named{};
    if (temporary.value<0 || ::fstat(temporary.value,&held)!=0 ||
        ::write(temporary.value,token.constData(),size_t(token.size()))!=token.size()) return 1;
    if (!current() || ::fstatat(root.value,"facts-request.tmp",&named,AT_SYMLINK_NOFOLLOW)!=0 ||
        named.st_dev!=held.st_dev || named.st_ino!=held.st_ino || !S_ISREG(named.st_mode) ||
        !absent(root.value,"facts-request") || ::linkat(root.value,"facts-request.tmp",root.value,"facts-request",0)!=0) return 1;
    if (::unlinkat(root.value,"facts-request.tmp",0)!=0 || !current()) return 2;
    return 0;
}
}
