#include "facts-request-publisher.h"
#include <cstdio>
int main(int argc,char **argv) {
    if (argc!=5) return 1;
    const int result=facts_request::publish(QString::fromLocal8Bit(argv[1]),QString::fromLatin1(argv[2]),argv[3],argv[4]);
    std::puts(result==0 ? "facts-request-published" : result==2 ? "facts-request-publication-unknown" : "facts-request-refused");
    return result;
}
