#pragma once
#include "facts-request-publisher.h"
#ifdef CAPTURE_PUBLISHER_FIXTURE
#include <functional>
#endif
namespace capture_observation {
using namespace facts_request;
#ifdef CAPTURE_PUBLISHER_FIXTURE
inline std::function<void(int)> capturePublishedFixtureHook;
#endif
inline bool decimal(const QByteArray &value,bool zero=false) {
    if(value=="0")return zero;
    if(value.isEmpty() || value.size()>20 || value[0]=='0')return false;
    for(char c:value)if(c<'0' || c>'9')return false;
    bool ok=false;value.toULongLong(&ok);return ok;
}
inline int publishCapture(const QString &directory,const QString &nonce,const QByteArray &executableHash,const QByteArray &payloadHash) {
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
    for(int index:{1,2,3,4})if(!decimal(fields[index]))return 1;
    if(!decimal(fields[6],true))return 1;
    bool pidOk=false,devOk=false,inoOk=false,timeOk=false;
    const int pid=fields[1].toInt(&pidOk);
    const auto dev=fields[3].toULongLong(&devOk),ino=fields[4].toULongLong(&inoOk);
    const qint64 waitingAt=fields[6].toLongLong(&timeOk);
    if (!pidOk || pid<=1 || !devOk || !inoOk || dev!=qulonglong(rootStat.st_dev) || ino!=qulonglong(rootStat.st_ino) ||
        !timeOk || waitingAt<0 || waitingAt>=120000) return 1;
    Descriptor process(::open((QStringLiteral("/proc/")+QString::number(pid)).toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC));
    if (process.value<0) return 1;
    bool publicationLinked=false;
    const auto current=[&]{
        Descriptor fresh(::open(directory.toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC));
        struct stat st{};
        return fresh.value>=0 && ::fstat(fresh.value,&st)==0 && st.st_uid==::geteuid() && (st.st_mode&0777)==0700 &&
            st.st_dev==rootStat.st_dev && st.st_ino==rootStat.st_ino &&
            privateFile(root.value,"owner",32)==nonce.toLatin1() && privateFile(root.value,"facts-waiting",256)==waiting &&
            privateFile(root.value,"attempt.identity",128)==fields[1]+' '+fields[2]+'\n' && startIdentity(process.value)==fields[2] &&
            absent(root.value,"entry.closed") && absent(root.value,"restore.claim") && absent(root.value,"callback.json") &&
            absent(root.value,"facts-request") && absent(root.value,"facts-request.tmp") &&
            (publicationLinked || (absent(root.value,"capture-window.png") && absent(root.value,"capture-observation-complete.json"))) &&
            absent(root.value,"input-observation-end") && absent(root.value,"input-observation-end.tmp");
    };
    if (!absent(root.value,"facts-request") || !absent(root.value,"facts-request.tmp") || !absent(root.value,"capture-window.png") || !absent(root.value,"capture-observation-complete.json") || !absent(root.value,"input-observation-end") || !absent(root.value,"input-observation-end.tmp") || !current() || !absent(root.value,"capture-observation-request") || !absent(root.value,"capture-observation-request.tmp")) return 1;
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
    const QByteArray token=fields.mid(0,5).join(' ')+" capture-observation "+fields[7]+' '+fields[8]+'\n';
    if (token.size()>256) return 1;
    Descriptor temporary(::openat(root.value,"capture-observation-request.tmp",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600));
    struct stat held{},named{};
    if (temporary.value<0 || ::fstat(temporary.value,&held)!=0 ||
        ::write(temporary.value,token.constData(),size_t(token.size()))!=token.size()) return 1;
    if (!current() || ::fstatat(root.value,"capture-observation-request.tmp",&named,AT_SYMLINK_NOFOLLOW)!=0 ||
        named.st_dev!=held.st_dev || named.st_ino!=held.st_ino || !S_ISREG(named.st_mode) ||
        !absent(root.value,"capture-observation-request") || ::linkat(root.value,"capture-observation-request.tmp",root.value,"capture-observation-request",0)!=0) return 1;
    publicationLinked=true;
#ifdef CAPTURE_PUBLISHER_FIXTURE
    if(capturePublishedFixtureHook)capturePublishedFixtureHook(root.value);
#endif
    if (::unlinkat(root.value,"capture-observation-request.tmp",0)!=0 || !current()) return 2;
    return 0;
}

}
