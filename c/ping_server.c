/*
 * Answers one ping on the name PING_NAME and exits.
 *
 *   ping_server        attach PING_NAME, reply n + 1 to one ping, detach, exit 0
 *
 * Exits 1 on an error and is killed by SIGALRM after 30 s if no ping arrives, so
 * a failed test cannot leave it running.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/dispatch.h>
#include <sys/iomsg.h>
#include <sys/neutrino.h>
#include <unistd.h>

#include "qnx_ipc_ping.h"

union recv_buf {
    uint16_t type;
    struct _pulse pulse;
    struct ping_msg ping;
    char raw[256];
};

int main(void)
{
    alarm(30);

    name_attach_t *attach = name_attach(NULL, PING_NAME, 0);
    if (attach == NULL) {
        perror("name_attach");
        return 1;
    }

    int status = 1;
    for (;;) {
        union recv_buf msg;
        struct _msg_info info;
        memset(&msg, 0, sizeof msg);
        int rcvid = MsgReceive(attach->chid, &msg, sizeof msg, &info);
        if (rcvid == -1) {
            if (errno == EINTR) {
                continue;
            }
            perror("MsgReceive");
            break;
        }

        if (rcvid == 0) {
            /* A pulse. Free the connection of a client that went away. */
            if (msg.pulse.code == _PULSE_CODE_DISCONNECT) {
                ConnectDetach(msg.pulse.scoid);
            }
            continue;
        }

        /* Messages that name_open and the I/O layer may send to a name_attach
         * channel: accept the connect, refuse the rest. */
        if (msg.type == _IO_CONNECT) {
            MsgReply(rcvid, EOK, NULL, 0);
            continue;
        }
        if (msg.type <= _IO_MAX) {
            MsgError(rcvid, ENOSYS);
            continue;
        }

        if (msg.ping.hdr.kind != PING_KIND) {
            MsgError(rcvid, ENOSYS);
            continue;
        }

        if (info.srcmsglen < (int64_t)sizeof(struct ping_msg)) {
            MsgError(rcvid, EBADMSG); /* too short to be a ping */
            continue;
        }

        struct pong reply = { .n = msg.ping.n + 1 };
        MsgReply(rcvid, sizeof reply, &reply, sizeof reply);
        status = 0;
        break;
    }

    name_detach(attach, 0);
    return status;
}
