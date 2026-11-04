#ifndef E0T_REQUEST_PROTOCOL_H
#define E0T_REQUEST_PROTOCOL_H
/* Pure IPC/intent model, NOT a durable ledger, server, lease or executor.
 * Integration must publish owned durable intent BEFORE dispatch and latch the
 * actual publication fence before acknowledgement. No cached fact is liveness.
 * Canonical frame: NONCE GENERATION REQUEST_ID OPCODE OPERAND LF (<=64 bytes).
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>
enum e0t_request_op { E0T_REQ_JOBS, E0T_REQ_UNITS, E0T_REQ_START, E0T_REQ_CANCEL,
                      E0T_REQ_STOP_CASES, E0T_REQ_ADVANCE, E0T_REQ_CLOSE };
struct e0t_request { unsigned generation, id; enum e0t_request_op op; int operand; };
enum e0t_request_outcome { E0T_OUTCOME_PENDING, E0T_OUTCOME_OBSERVED, E0T_OUTCOME_NO_COMMAND,
                         E0T_OUTCOME_FAILED_UNKNOWN, E0T_OUTCOME_CLOSED };
struct e0t_request_entry { int present; struct e0t_request request; enum e0t_request_outcome outcome; };
struct e0t_request_ledger { unsigned generation, last_id; int closed; struct e0t_request_entry entries[96]; };
enum e0t_request_decision { E0T_REQUEST_REFUSED=-1, E0T_REQUEST_RECONCILE=0,
                          E0T_REQUEST_NEW=1, E0T_REQUEST_HISTORICAL=2 };
static int e0t_request_decimal(const char *value, size_t length, unsigned maximum, unsigned *result) {
    if (!length || length > 3 || (length > 1 && value[0] == '0')) return 0;
    unsigned total = 0;
    for (size_t i=0; i<length; ++i) {
        if (value[i]<'0' || value[i]>'9') return 0;
        total=total*10+(unsigned)(value[i]-'0');
    }
    if (total>maximum) return 0;
    *result=total; return 1;
}
static int e0t_request_parse(const char *data, size_t length, struct e0t_request *request) {
    if (!data || !request || length>64 || length<40 || data[length-1]!='\n') return 0;
    const char *tokens[5]; size_t sizes[5]; unsigned number=0; size_t offset=0;
    while (offset<length-1) {
        if (number==5 || data[offset]==' ') return 0;
        tokens[number]=data+offset; size_t begin=offset;
        while (offset<length-1 && data[offset]!=' ') {
            if ((unsigned char)data[offset]<33 || (unsigned char)data[offset]>126) return 0;
            ++offset;
        }
        sizes[number++]=offset-begin;
        if (offset<length-1 && ++offset==length-1) return 0;
    }
    if (number!=5 || sizes[0]!=32 || memcmp(tokens[0],E0T_NONCE,32) || sizes[3]!=1) return 0;
    if (!e0t_request_decimal(tokens[1],sizes[1],8,&request->generation) || !request->generation
        || !e0t_request_decimal(tokens[2],sizes[2],96,&request->id) || !request->id) return 0;
    switch (*tokens[3]) {
    case 'J': request->op=E0T_REQ_JOBS; break;
    case 'U': request->op=E0T_REQ_UNITS; break;
    case 'B': request->op=E0T_REQ_START; break;
    case 'C': request->op=E0T_REQ_CANCEL; break;
    case 'S': request->op=E0T_REQ_STOP_CASES; break;
    case 'A': request->op=E0T_REQ_ADVANCE; break;
    case 'X': request->op=E0T_REQ_CLOSE; break;
    default: return 0;
    }
    request->operand=-1;
    if (sizes[4]==1 && *tokens[4]=='-')
        return request->op!=E0T_REQ_START && request->op!=E0T_REQ_ADVANCE;
    unsigned operand;
    if (!e0t_request_decimal(tokens[4],sizes[4],11,&operand)) return 0;
    if (request->op!=E0T_REQ_START && request->op!=E0T_REQ_CANCEL && request->op!=E0T_REQ_ADVANCE) return 0;
    if (request->op==E0T_REQ_ADVANCE && (operand<2 || operand>8)) return 0;
    request->operand=(int)operand; return 1;
}
static int e0t_request_equal(const struct e0t_request *a, const struct e0t_request *b) {
    return a->generation==b->generation && a->id==b->id && a->op==b->op && a->operand==b->operand;
}
static enum e0t_request_decision e0t_request_prepare(struct e0t_request_ledger *ledger,
                                                   const struct e0t_request *request) {
    if (!ledger || !request || !request->id || request->id>96 || !request->generation
        || request->generation>8 || request->op<E0T_REQ_JOBS || request->op>E0T_REQ_CLOSE
        || request->operand < -1 || request->operand > 11) return E0T_REQUEST_REFUSED;
    if ((request->op==E0T_REQ_START && request->operand<0)
        || (request->op==E0T_REQ_ADVANCE && (request->operand<2 || request->operand>8))
        || ((request->op==E0T_REQ_JOBS || request->op==E0T_REQ_UNITS
             || request->op==E0T_REQ_STOP_CASES || request->op==E0T_REQ_CLOSE) && request->operand!=-1))
        return E0T_REQUEST_REFUSED;
    struct e0t_request_entry *entry=&ledger->entries[request->id-1];
    if (entry->present) {
        if (!e0t_request_equal(&entry->request,request)) { ledger->closed=1; return E0T_REQUEST_REFUSED; }
        return entry->outcome==E0T_OUTCOME_PENDING ? E0T_REQUEST_RECONCILE : E0T_REQUEST_HISTORICAL;
    }
    if (request->generation!=ledger->generation || request->id!=ledger->last_id+1
        || (ledger->closed && request->op!=E0T_REQ_JOBS && request->op!=E0T_REQ_UNITS
            && request->op!=E0T_REQ_CANCEL && request->op!=E0T_REQ_STOP_CASES)) return E0T_REQUEST_REFUSED;
    if (request->op==E0T_REQ_ADVANCE && (request->operand!=(int)ledger->generation+1 || ledger->generation==8))
        return E0T_REQUEST_REFUSED;
    if (request->op==E0T_REQ_START || request->op==E0T_REQ_ADVANCE)
        for (unsigned i=0; i<96; ++i)
            if (ledger->entries[i].present && ledger->entries[i].outcome==E0T_OUTCOME_PENDING
                && (ledger->entries[i].request.op==E0T_REQ_START
                    || ledger->entries[i].request.op==E0T_REQ_CANCEL
                    || ledger->entries[i].request.op==E0T_REQ_STOP_CASES
                    || ledger->entries[i].request.op==E0T_REQ_ADVANCE)) return E0T_REQUEST_REFUSED;
    *entry=(struct e0t_request_entry){1,*request,E0T_OUTCOME_PENDING};
    ledger->last_id=request->id;
    if (request->op==E0T_REQ_CLOSE) ledger->closed=1;
    return E0T_REQUEST_NEW; /* Integration MUST persist this intent before effects. */
}
static int e0t_outcome_valid(enum e0t_request_op op, enum e0t_request_outcome outcome) {
    return outcome>E0T_OUTCOME_PENDING && outcome<=E0T_OUTCOME_CLOSED
        && !(outcome==E0T_OUTCOME_NO_COMMAND && op!=E0T_REQ_CANCEL)
        && !(outcome==E0T_OUTCOME_CLOSED && op!=E0T_REQ_CLOSE)
        && !(op==E0T_REQ_CLOSE && outcome!=E0T_OUTCOME_CLOSED && outcome!=E0T_OUTCOME_FAILED_UNKNOWN);
}
static int e0t_request_finish(struct e0t_request_ledger *ledger, unsigned id, enum e0t_request_outcome outcome) {
    if (!ledger || !id || id>96 || outcome<=E0T_OUTCOME_PENDING || outcome>E0T_OUTCOME_CLOSED) return 0;
    struct e0t_request_entry *entry=&ledger->entries[id-1];
    if (!entry->present || entry->outcome!=E0T_OUTCOME_PENDING) return 0;
    if (!e0t_outcome_valid(entry->request.op,outcome)) return 0;
    if (entry->request.op==E0T_REQ_ADVANCE && outcome==E0T_OUTCOME_OBSERVED
        && (ledger->closed || entry->request.generation!=ledger->generation)) return 0;
    entry->outcome=outcome;
    if (outcome==E0T_OUTCOME_FAILED_UNKNOWN) ledger->closed=1;
    if (entry->request.op==E0T_REQ_ADVANCE && outcome==E0T_OUTCOME_OBSERVED)
        ledger->generation=(unsigned)entry->request.operand;
    return 1; /* Observation outcome still supplies no job/phase/lease proof. */
}
#endif
