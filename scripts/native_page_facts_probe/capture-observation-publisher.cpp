#include "capture-observation-publisher.h"
#include <cstdio>
int main(int argc,char **argv) {
    if(argc!=5)return 1;
    const int result=capture_observation::publishCapture(QString::fromLocal8Bit(argv[1]),QString::fromLatin1(argv[2]),argv[3],argv[4]);
    std::puts(result==0 ? "capture-observation-request-published":result==2 ? "capture-observation-publication-unknown":"capture-observation-refused");
    return result;
}
