#include "input-observation-end-publisher.h"
#include <cstdio>
int main(int argc,char **argv) {
    if (argc!=5) return 1;
    const int result=input_observation_end::publish(QString::fromLocal8Bit(argv[1]),QString::fromLatin1(argv[2]),argv[3],argv[4]);
    std::puts(result==0 ? "input-observation-end-published" : result==2 ? "input-observation-end-publication-unknown" : "input-observation-end-refused");
    return result;
}
