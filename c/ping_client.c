/*
 * Sends one ping to the server named PING_NAME and prints the answer.
 *
 *   ping_client <n>    send ping n, print "pong <n + 1>\n" on stdout, exit 0
 *
 * Exits 1 on any error (message on stderr) and is killed by SIGALRM after 30 s,
 * so a failed test cannot leave it running.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/dispatch.h>
#include <sys/neutrino.h>
#include <time.h>
#include <unistd.h>

#include "qnx_ipc_ping.h"

/* The server may still be starting: retry name_open for up to 5 s. */
static int open_server(void)
{
    for (int attempt = 0; attempt < 100; attempt++) {
        int coid = name_open(PING_NAME, 0);
        if (coid != -1) {
            return coid;
        }
        struct timespec pause = { 0, 50 * 1000 * 1000 };
        nanosleep(&pause, NULL);
    }
    return -1;
}

int main(int argc, char **argv)
{
    if (argc != 2) {
        fprintf(stderr, "usage: %s <n>\n", argv[0]);
        return 2;
    }
    char *end = NULL;
    unsigned long n = strtoul(argv[1], &end, 10);
    if (*argv[1] == '\0' || *end != '\0') {
        fprintf(stderr, "%s: not a number: %s\n", argv[0], argv[1]);
        return 2;
    }

    alarm(30);

    int coid = open_server();
    if (coid == -1) {
        perror("name_open");
        return 1;
    }

    struct ping_msg ping = { .hdr = { .kind = PING_KIND, .reserved = 0 }, .n = (uint32_t)n };
    struct pong reply = { 0 };
    /* The server replies with the reply length as the status, so only -1 is an error. */
    if (MsgSend(coid, &ping, sizeof ping, &reply, sizeof reply) == -1) {
        perror("MsgSend");
        name_close(coid);
        return 1;
    }
    name_close(coid);

    printf("pong %u\n", (unsigned)reply.n);
    return 0;
}
