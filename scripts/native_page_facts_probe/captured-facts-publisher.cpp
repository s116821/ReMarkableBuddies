#include "captured-facts-publisher.h"
#include <cstdio>
int main(int argc,char **argv) {
    if(argc!=8)return 1;
    const int result=capture_observation::publishFacts(QString::fromLocal8Bit(argv[1]),QString::fromLatin1(argv[2]),argv[3],argv[4],
        QString::fromLatin1(argv[5]),QString::fromLatin1(argv[6]).split(','),argv[7]);
    std::puts(result==0 ? "captured-facts-request-published":result==2 ? "captured-facts-publication-unknown":"captured-facts-refused");
    return result;
}
