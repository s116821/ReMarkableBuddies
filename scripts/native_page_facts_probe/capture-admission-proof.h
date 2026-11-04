#pragma once
#include "capture-observation-publisher.h"
#include <QJsonDocument>
#include <QJsonObject>
#include <QStringList>
#include <cmath>
#include <memory>
namespace capture_observation {
inline bool id(const QString &value) {
    return value!=QStringLiteral("00000000-0000-0000-0000-000000000000") &&
        QRegularExpression(QStringLiteral("\\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\\z")).match(value).hasMatch();
}
inline bool integer(const QJsonValue &value,qint64 maximum) {
    return value.isDouble() && std::isfinite(value.toDouble()) && value.toDouble()>=0 &&
        value.toDouble()<=double(maximum) && std::floor(value.toDouble())==value.toDouble();
}
inline bool exactFields(const QJsonObject &object,const QStringList &fields) {
    if(object.size()!=fields.size())return false;
    for(const auto &field:fields)if(!object.contains(field))return false;
    return true;
}
struct HeldPrivate {
    int root;const char *name;Descriptor fd;QByteArray bytes;struct stat initial{};
    HeldPrivate(int directory,const char *filename,int cap):root(directory),name(filename),
        fd(::openat(directory,filename,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC)),bytes(privateFile(directory,filename,cap)) {
        if(fd.value>=0)::fstat(fd.value,&initial);
    }
    bool current() const {
        struct stat held{},named{};
        return !bytes.isEmpty() && fd.value>=0 && ::fstat(fd.value,&held)==0 && ::fstatat(root,name,&named,AT_SYMLINK_NOFOLLOW)==0 &&
            S_ISREG(held.st_mode) && held.st_uid==::geteuid() && (held.st_mode&0777)==0600 &&
            held.st_dev==initial.st_dev && held.st_ino==initial.st_ino && held.st_dev==named.st_dev && held.st_ino==named.st_ino &&
            held.st_size==bytes.size() && named.st_size==held.st_size && privateFile(root,name,bytes.size())==bytes;
    }
};
inline bool completionValid(const QJsonObject &o,const QList<QByteArray> &identity,const QString &document,const QStringList &order) {
    const QStringList fields=QStringLiteral("kind version nonce attempt_pid attempt_start root_device root_inode setup_profile setup_budget_ms capture_budget_ms accepted_ms baseline_ms grab_start_ms grab_end_ms post_read_ms completed_ms document_id page_id page_index begin_epoch end_epoch width height dpr image_width image_height png_bytes png_sha256 image_status gui_callback_completed scope_current atomic_snapshot native_authority render_authority ui_acknowledged observed_order").split(' ');
    if(!exactFields(o,fields) || identity.size()!=5 || !id(document) || order.isEmpty() || order.size()>256)return false;
    for(int i=0;i<order.size();++i)if(!id(order[i]) || order.indexOf(order[i])!=i)return false;
    for(const auto &field:{"kind","nonce","attempt_pid","attempt_start","root_device","root_inode","setup_profile","document_id","page_id","begin_epoch","end_epoch","png_sha256","image_status"})if(!o.value(field).isString())return false;
    if(o.value("kind")!="development-capture-observation" || o.value("setup_profile")!="main-dev-facts-120s" || o.value("image_status")!="available")return false;
    const QStringList identityFields={"nonce","attempt_pid","attempt_start","root_device","root_inode"};
    for(int i=0;i<5;++i)if(o.value(identityFields[i]).toString().toLatin1()!=identity[i] || (i>0 && !decimal(identity[i])))return false;
    for(const auto *field:{"begin_epoch","end_epoch"})if(!decimal(o.value(field).toString().toLatin1()))return false;
    if(o.value("begin_epoch")!=o.value("end_epoch"))return false;
    for(const auto *field:{"version","setup_budget_ms","capture_budget_ms","accepted_ms","baseline_ms","grab_start_ms","grab_end_ms","post_read_ms","completed_ms","page_index","width","height","image_width","image_height","png_bytes"})if(!integer(o.value(field),2147483647))return false;
    if(o.value("version").toInt()!=1 || o.value("setup_budget_ms").toInt()!=120000 || o.value("capture_budget_ms").toInt()!=5000)return false;
    qint64 last=o.value("accepted_ms").toInteger();
    for(const auto *field:{"baseline_ms","grab_start_ms","grab_end_ms","post_read_ms","completed_ms"}){const auto time=o.value(field).toInteger();if(time<last)return false;last=time;}
    if(last>=120000 || last>=o.value("accepted_ms").toInteger()+5000)return false;
    const auto index=o.value("page_index").toInt(-1);
    if(index<0 || index>=order.size() || o.value("document_id").toString()!=document || o.value("page_id").toString()!=order[index])return false;
    for(const auto *field:{"gui_callback_completed","scope_current"})if(!o.value(field).isBool() || !o.value(field).toBool())return false;
    for(const auto *field:{"atomic_snapshot","native_authority","render_authority","ui_acknowledged","observed_order"})if(!o.value(field).isBool() || o.value(field).toBool())return false;
    const double dpr=o.value("dpr").toDouble(-1),width=o.value("width").toDouble(),height=o.value("height").toDouble();
    const auto iw=o.value("image_width").toInteger(),ih=o.value("image_height").toInteger(),bytes=o.value("png_bytes").toInteger();
    return o.value("dpr").isDouble() && std::isfinite(dpr) && dpr>0 && width>0 && height>0 && width*height*dpr*dpr<=4194304 &&
        iw>0 && ih>0 && double(iw)*ih<=4194304 && double(iw)==std::floor(width*dpr+0.5) && double(ih)==std::floor(height*dpr+0.5) &&
        bytes>=45 && bytes<=8388608 && QRegularExpression(QStringLiteral("\\A[0-9a-f]{64}\\z")).match(o.value("png_sha256").toString()).hasMatch();
}
struct Admission {
    HeldPrivate request,complete,image,visual;
    bool valid=false;
    Admission(int root,const QList<QByteArray> &identity,const QString &document,const QStringList &order,const QByteArray &visualHash):
        request(root,"capture-observation-request",256),complete(root,"capture-observation-complete.json",8192),
        image(root,"capture-window.png",8388608),visual(root,"capture-visual-review.json",4096) {
        if(!request.current() || !complete.current() || !image.current() || !visual.current() ||
            request.bytes!=identity.join(' ')+" capture-observation 120000 main-dev-facts-120s\n")return;
        const auto json=QJsonDocument::fromJson(complete.bytes);const auto object=json.object();
        // SDK compact serialization is canonical here; duplicate JSON keys and
        // alternative representations cannot disappear into a permissive parser.
        if(!json.isObject() || json.toJson(QJsonDocument::Compact)!=complete.bytes || !completionValid(object,identity,document,order))return;
        const auto pngHash=QCryptographicHash::hash(image.bytes,QCryptographicHash::Sha256).toHex();
        if(pngHash!=object.value("png_sha256").toString().toLatin1() || image.bytes.size()!=object.value("png_bytes").toInteger() ||
            image.bytes.left(16)!=QByteArray::fromHex("89504e470d0a1a0a0000000d49484452") || image.bytes.right(12)!=QByteArray::fromHex("0000000049454e44ae426082"))return;
        const auto dimension=[&](int offset){quint32 value=0;for(int i=0;i<4;++i)value=(value<<8)|quint8(image.bytes[offset+i]);return value;};
        if(dimension(16)!=object.value("image_width").toInteger() || dimension(20)!=object.value("image_height").toInteger())return;
        const auto decision=QJsonDocument::fromJson(visual.bytes).object();
        const QStringList fields=QStringLiteral("kind version nonce attempt_pid attempt_start root_device root_inode document_id page_id page_index completion_sha256 png_sha256 request_sha256 visual_open_fixture reviewer").split(' ');
        if(!exactFields(decision,fields) || QCryptographicHash::hash(visual.bytes,QCryptographicHash::Sha256).toHex()!=visualHash ||
            decision.value("kind")!="development-capture-visual-review" || !integer(decision.value("version"),1) || decision.value("version").toInt()!=1 ||
            !decision.value("visual_open_fixture").isBool() || !decision.value("visual_open_fixture").toBool() || decision.value("reviewer")!="Main" ||
            !integer(decision.value("page_index"),255) || decision.value("page_index")!=object.value("page_index"))return;
        for(const auto *field:{"nonce","attempt_pid","attempt_start","root_device","root_inode","document_id","page_id"})if(decision.value(field)!=object.value(field))return;
        if(decision.value("completion_sha256").toString().toLatin1()!=QCryptographicHash::hash(complete.bytes,QCryptographicHash::Sha256).toHex() ||
            decision.value("png_sha256").toString().toLatin1()!=pngHash || decision.value("request_sha256").toString().toLatin1()!=QCryptographicHash::hash(request.bytes,QCryptographicHash::Sha256).toHex())return;
        valid=true;
    }
    bool current() const {return valid && request.current() && complete.current() && image.current() && visual.current();}
};
}
