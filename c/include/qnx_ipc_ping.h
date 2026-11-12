/*
 * The ping protocol shared by the C programs in c/ and the Rust interop test
 * (crates/qnx-ipc/tests/common/ping.rs). The same layout is written twice and
 * both sides assert the sizes, so a change on one side fails to build.
 *
 * A typed qnx-ipc message on the wire is a 4-byte header followed by the payload.
 * The client sends struct ping_msg and the server replies with struct pong
 * (n + 1). The server replies with the reply's byte length as the MsgReply
 * status, which is what qnx-ipc's Request::reply does, so MsgSend returns that
 * length on success. Only -1 means failure.
 */
#ifndef QNX_IPC_PING_H
#define QNX_IPC_PING_H

#include <stdint.h>

#define PING_NAME "qnx-ipc-ping"
#define PING_KIND 0x201 /* user kinds start at 0x200 */

struct ping_hdr {
    uint16_t kind;     /* PING_KIND */
    uint16_t reserved; /* always zero */
};

struct ping_msg {
    struct ping_hdr hdr;
    uint32_t n;
};

struct pong {
    uint32_t n;
};

_Static_assert(sizeof(struct ping_hdr) == 4, "ping_hdr must be 4 bytes");
_Static_assert(sizeof(struct ping_msg) == 8, "ping_msg must be 8 bytes");
_Static_assert(sizeof(struct pong) == 4, "pong must be 4 bytes");

#endif /* QNX_IPC_PING_H */
