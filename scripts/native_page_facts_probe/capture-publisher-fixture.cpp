#define CAPTURE_PUBLISHER_FIXTURE
#include "captured-facts-publisher.h"
#include <QCoreApplication>
#include <QFile>
#include <sys/mman.h>
#include <cstdio>
using namespace capture_observation;
static bool put(const QString &path,const QByteArray &bytes) {
    QFile file(path);return file.open(QIODevice::WriteOnly|QIODevice::NewOnly) && file.setPermissions(QFile::ReadOwner|QFile::WriteOwner) && file.write(bytes)==bytes.size();
}
int main(int argc,char **argv) {
    QCoreApplication app(argc,argv);if(argc!=3)return 2;
    const QString root=QString::fromLocal8Bit(argv[1]),nonce=QStringLiteral("0123456789abcdef0123456789abcdef");const QByteArray mode(argv[2]);
    Descriptor directory(::open(root.toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_CLOEXEC));struct stat st{};if(::fstat(directory.value,&st)!=0)return 3;
    Descriptor process(::open((QStringLiteral("/proc/")+QString::number(::getpid())).toUtf8().constData(),O_RDONLY|O_DIRECTORY|O_CLOEXEC));
    const QList<QByteArray> identity={nonce.toLatin1(),QByteArray::number(::getpid()),startIdentity(process.value),QByteArray::number(qulonglong(st.st_dev)),QByteArray::number(qulonglong(st.st_ino))};
    if(!put(root+"/owner",nonce.toLatin1()) || !put(root+"/attempt.identity",identity[1]+' '+identity[2]+'\n') || !put(root+"/facts-waiting",identity.join(' ')+" waiting-facts 1 120000 main-dev-facts-120s\n"))return 4;
    Descriptor payload(::openat(directory.value,"payload.so",O_RDONLY|O_CLOEXEC)),executable(::openat(process.value,"exe",O_RDONLY|O_CLOEXEC));struct stat ps{};if(::fstat(payload.value,&ps)!=0)return 5;
    const auto payloadHash=digest(payload.value,16*1024*1024),exeHash=digest(executable.value,64*1024*1024);
    void *mapped=::mmap(nullptr,size_t(ps.st_size),PROT_READ,MAP_PRIVATE,payload.value,0);if(mapped==MAP_FAILED)return 6;
    int result=1;bool passed=false;
    if(mode.startsWith("capture-")) {
        if(mode=="capture-early")put(root+"/facts-request.tmp","early");
        if(mode=="capture-purpose")put(root+"/input-observation-end.tmp","wrong");
        if(mode=="capture-stale")put(root+"/capture-window.png","stale");
        if(mode=="capture-phase-advance")capturePublishedFixtureHook=[&](int){put(root+"/capture-window.png","SDK output");put(root+"/capture-observation-complete.json","SDK completion");};
        result=publishCapture(root,nonce,exeHash,payloadHash);
        const bool expected=mode=="capture-good" || mode=="capture-duplicate" || mode=="capture-phase-advance";
        passed=(result==0)==expected;
        if(expected)passed=passed && privateFile(directory.value,"capture-observation-request",256)==identity.join(' ')+" capture-observation 120000 main-dev-facts-120s\n";
        if(mode=="capture-duplicate")passed=passed && publishCapture(root,nonce,exeHash,payloadHash)!=0;
    } else {
        const QString document=QStringLiteral("11111111-1111-1111-1111-111111111111"),page=QStringLiteral("22222222-2222-2222-2222-222222222222");
        const auto token=identity.join(' ')+" capture-observation 120000 main-dev-facts-120s\n";
        auto png=QByteArray::fromBase64("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9d8AAAAASUVORK5CYII=");
        const auto hash=[](const QByteArray &bytes){return QCryptographicHash::hash(bytes,QCryptographicHash::Sha256).toHex();};
        QJsonObject o{{"kind","development-capture-observation"},{"version",1},{"nonce",nonce},{"attempt_pid",QString::fromLatin1(identity[1])},{"attempt_start",QString::fromLatin1(identity[2])},{"root_device",QString::fromLatin1(identity[3])},{"root_inode",QString::fromLatin1(identity[4])},
            {"setup_profile","main-dev-facts-120s"},{"setup_budget_ms",120000},{"capture_budget_ms",5000},{"accepted_ms",100},{"baseline_ms",101},{"grab_start_ms",102},{"grab_end_ms",103},{"post_read_ms",104},{"completed_ms",105},
            {"document_id",document},{"page_id",page},{"page_index",0},{"begin_epoch","1"},{"end_epoch","1"},{"width",1},{"height",1},{"dpr",1},{"image_width",1},{"image_height",1},{"png_bytes",png.size()},{"png_sha256",QString::fromLatin1(hash(png))},{"image_status","available"},
            {"gui_callback_completed",true},{"scope_current",true},{"atomic_snapshot",false},{"native_authority",false},{"render_authority",false},{"ui_acknowledged",false},{"observed_order",false}};
        if(mode=="facts-epoch")o["end_epoch"]="2";
        if(mode=="facts-late")o["completed_ms"]=5100;
        if(mode=="facts-original-expiry")o["completed_ms"]=120000;
        if(mode=="facts-dimension")o["image_width"]=2;
        if(mode=="facts-extra")o["extra"]=1;
        if(mode=="facts-authority")o["render_authority"]=true;
        if(mode=="facts-document")o["document_id"]=page;
        if(mode=="facts-page")o["page_id"]=document;
        if(mode=="facts-other-page"){o["page_index"]=1;o["page_id"]=document;}
        if(mode=="facts-identity")o["attempt_start"]="1";
        if(mode=="facts-png")png[0]=0;
        auto complete=QJsonDocument(o).toJson(QJsonDocument::Compact);
        if(mode=="facts-duplicate-json")complete.insert(1,"\"version\":1,");
        QJsonObject visual{{"kind","development-capture-visual-review"},{"version",1},{"completion_sha256",QString::fromLatin1(hash(complete))},{"png_sha256",QString::fromLatin1(hash(png))},{"request_sha256",QString::fromLatin1(hash(token))},{"visual_open_fixture",true},{"reviewer","Main"}};
        for(const auto *field:{"nonce","attempt_pid","attempt_start","root_device","root_inode","document_id","page_id","page_index"})visual[field]=o.value(field);
        if(mode=="facts-negative")visual["visual_open_fixture"]=false;
        if(mode=="facts-reviewer")visual["reviewer"]="Other";
        if(mode=="facts-review-extra")visual["extra"]=1;
        const auto decision=QJsonDocument(visual).toJson(QJsonDocument::Compact);auto visualHash=hash(decision);
        const QStringList selectedOrder=mode=="facts-other-page" ? QStringList{page,document}:QStringList{page};
        if(mode=="facts-wrong-review-hash")visualHash[0]='z';
        if(!put(root+"/capture-observation-request",token) || !put(root+"/capture-observation-complete.json",complete) || !put(root+"/capture-window.png",png))return 7;
        if(mode!="facts-missing-review" && !put(root+"/capture-visual-review.json",decision))return 7;
        if(mode=="held-replacement") {
            Admission admission(directory.value,identity,document,selectedOrder,visualHash);
            if(!admission.current())return 8;
            QFile::remove(root+"/capture-window.png");put(root+"/capture-window.png",png);
            passed=!admission.current();
        } else {
            result=publishFacts(root,nonce,exeHash,payloadHash,document,selectedOrder,visualHash);
            const bool expected=mode=="facts-good" || mode=="facts-duplicate";
            passed=(result==0)==expected;
            if(expected)passed=passed && privateFile(directory.value,"facts-request",128)==identity.join(' ')+" read-facts 120000 main-dev-facts-120s\n";
            if(mode=="facts-duplicate")passed=passed && publishFacts(root,nonce,exeHash,payloadHash,document,{page},visualHash)!=0;
        }
    }
    ::munmap(mapped,size_t(ps.st_size));
    std::printf("capture-publisher %s: %s result=%d (host fixture only)\n",argv[2],passed ? "PASS":"FAIL",result);return passed ? 0:1;
}
