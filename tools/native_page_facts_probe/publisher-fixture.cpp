#include "facts-request-publisher.h"
#include <QCoreApplication>
#include <QFile>
#include <QDir>
#include <sys/mman.h>
#include <cstdio>
static bool put(const QString &path,const QByteArray &bytes) {
 QFile file(path); if (!file.open(QIODevice::WriteOnly|QIODevice::NewOnly)) return false;
 return file.setPermissions(QFile::ReadOwner|QFile::WriteOwner) && file.write(bytes)==bytes.size();
}
int main(int argc,char **argv) {
 QCoreApplication app(argc,argv);
 if (argc!=3) return 3;
 const QString root=QString::fromLocal8Bit(argv[1]); const QByteArray mode(argv[2]);
 const QString nonce=QStringLiteral("0123456789abcdef0123456789abcdef");
 facts_request::Descriptor directory(::open(root.toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_CLOEXEC));
 struct stat st{}; if (::fstat(directory.value,&st)!=0) return 4;
 const int pid=::getpid();
 facts_request::Descriptor process(::open((QStringLiteral("/proc/")+QString::number(pid)).toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_CLOEXEC));
 const QByteArray start=facts_request::startIdentity(process.value);
 const QByteArray identity=nonce.toLatin1()+' '+QByteArray::number(pid)+' '+start+' '+QByteArray::number(qulonglong(st.st_dev))+' '+QByteArray::number(qulonglong(st.st_ino));
 QByteArray waiting=identity+" waiting-facts 1 20000\n";
 if (mode=="malformed") waiting.append('\n');
 if (mode=="late") waiting=identity+" waiting-facts 20000 20000\n";
 if (!put(root+"/owner",nonce.toLatin1()) || !put(root+"/attempt.identity",QByteArray::number(pid)+' '+start+'\n') || !put(root+"/facts-waiting",waiting)) return 5;
 facts_request::Descriptor payload(::openat(directory.value,"payload.so",O_RDONLY|O_CLOEXEC));
 facts_request::Descriptor executable(::openat(process.value,"exe",O_RDONLY|O_CLOEXEC));
 auto payloadHash=facts_request::digest(payload.value,16*1024*1024);
 auto executableHash=facts_request::digest(executable.value,64*1024*1024);
 struct stat payloadSt{}; if (::fstat(payload.value,&payloadSt)!=0) return 6;
 // This is data, not an ELF/preload extension. Mapping it validates proc/maps
 // binding mechanically without executing any owned or native payload code.
 void *mapped=::mmap(nullptr,size_t(payloadSt.st_size),PROT_READ,MAP_PRIVATE,payload.value,0);
 if (mapped==MAP_FAILED) return 7;
 if (mode=="unmapped") { ::munmap(mapped,size_t(payloadSt.st_size)); mapped=nullptr; }
 if (mode=="wrong-executable") executableHash[0]=executableHash[0]=='a' ? 'b' : 'a';
 if (mode=="wrong-payload") payloadHash[0]=payloadHash[0]=='a' ? 'b' : 'a';
 if (mode=="closed") put(root+"/entry.closed","closed");
 if (mode=="restoring") put(root+"/restore.claim","restore");
 if (mode=="callback") put(root+"/callback.json","callback");
 if (mode=="request-existing") put(root+"/facts-request","existing");
 if (mode=="temporary-existing") put(root+"/facts-request.tmp","existing");
 if (mode=="owner-mode") ::chmod((root+"/owner").toUtf8().constData(),0644);
 if (mode=="waiting-mode") ::chmod((root+"/facts-waiting").toUtf8().constData(),0644);
 if (mode=="directory-mode") ::chmod(root.toUtf8().constData(),0755);
 if (mode=="waiting-symlink") { QFile::rename(root+"/facts-waiting",root+"/waiting-original"); ::symlink("waiting-original",(root+"/facts-waiting").toUtf8().constData()); }
 if (mode=="payload-fifo") { QFile::remove(root+"/payload.so"); ::mkfifo((root+"/payload.so").toUtf8().constData(),0600); }
 if (mode=="generation") { QFile::remove(root+"/attempt.identity"); put(root+"/attempt.identity","1 1\n"); }
 facts_request::Descriptor heldLock(mode=="locked" ? ::openat(directory.value,"admission.lock",O_RDWR|O_CREAT|O_CLOEXEC,0600) : -1);
 if (mode=="locked" && (heldLock.value<0 || ::flock(heldLock.value,LOCK_EX|LOCK_NB)!=0)) return 8;
 const int result=facts_request::publish(root,nonce,executableHash,payloadHash);
 const bool expected=mode=="good" || mode=="duplicate";
 bool token=true;
 if (result==0) token=facts_request::privateFile(directory.value,"facts-request",128)==identity+" read-facts\n" && !QFileInfo::exists(root+"/facts-request.tmp");
 bool duplicate=true;
 if (mode=="duplicate") duplicate=facts_request::publish(root,nonce,executableHash,payloadHash)!=0;
 const bool passed=(result==0)==expected && token && duplicate;
 std::printf("publisher-%s: %s (result=%d; fixed-token=%d; duplicate-refused=%d)\n",argv[2],passed ? "PASS" : "FAIL",result,token,duplicate);
 if (mapped) ::munmap(mapped,size_t(payloadSt.st_size));
 return passed ? 0 : 1;
}
