#include "receiver-source-facts-publisher.h"
#include <cstdio>
int main(int argc,char **argv) {
    if(argc!=5)return 1;
    const int result=receiver_source_facts::publishReceiverSourceFacts(QString::fromLocal8Bit(argv[1]),QString::fromLatin1(argv[2]),argv[3],argv[4]);
    std::puts(result==0 ? "receiver-source-facts-request-published":result==2 ? "receiver-source-facts-publication-unknown":"receiver-source-facts-refused");
    return result;
}
