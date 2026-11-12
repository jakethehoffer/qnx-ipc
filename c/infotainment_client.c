/*
 * infotainment_client: the low-priority C client of the vehicle-signals demo.
 *
 * It switches itself to SCHED_FIFO priority 10, says hello to vsigd (which is
 * written in Rust), then asks for the vehicle speed in a loop and sends the
 * watchdog a heartbeat pulse after each answer. It runs until it is killed or
 * vsigd goes away (exit 1 with a message on stderr).
 */
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/dispatch.h>
#include <sys/neutrino.h>
#include <time.h>

#include "vehicle.h"

#define CLIENT_NAME "infotainment-client"
#define CLIENT_PRIORITY 10

/* The server may still be starting: retry name_open for up to 5 s. */
static int open_name(const char *name)
{
    for (int attempt = 0; attempt < 100; attempt++) {
        int coid = name_open(name, 0);
        if (coid != -1 || errno != ENOENT) {
            return coid;
        }
        struct timespec pause = { 0, 50 * 1000 * 1000 };
        nanosleep(&pause, NULL);
    }
    return -1;
}

int main(void)
{
    /* Line-buffered, so the demo log shows each line as it happens. */
    setvbuf(stdout, NULL, _IOLBF, 0);

    struct sched_param param = { .sched_priority = CLIENT_PRIORITY };
    int rc = pthread_setschedparam(pthread_self(), SCHED_FIFO, &param);
    if (rc != EOK) {
        fprintf(stderr, CLIENT_NAME ": pthread_setschedparam: %s\n", strerror(rc));
        return 1;
    }

    int vehicle = open_name(VEHICLE_NAME);
    if (vehicle == -1) {
        perror(CLIENT_NAME ": name_open " VEHICLE_NAME);
        return 1;
    }

    struct vehicle_hello hello = { .hdr = { .kind = VEHICLE_HELLO, .reserved = 0 } };
    strncpy(hello.name, CLIENT_NAME, sizeof hello.name);
    /* vsigd answers with an empty reply, so only -1 is an error. */
    if (MsgSend(vehicle, &hello, sizeof hello, NULL, 0) == -1) {
        perror(CLIENT_NAME ": hello");
        return 1;
    }

    int watchdog = open_name(WATCHDOG_NAME);
    if (watchdog == -1) {
        perror(CLIENT_NAME ": name_open " WATCHDOG_NAME);
        return 1;
    }
    printf(CLIENT_NAME ": running at priority %d (SCHED_FIFO), reading speed\n", CLIENT_PRIORITY);

    struct vehicle_get_signal get = {
        .hdr = { .kind = VEHICLE_GET_SIGNAL, .reserved = 0 },
        .signal_id = VEHICLE_SIGNAL_SPEED,
    };
    for (;;) {
        struct vehicle_signal speed = { 0 };
        /* vsigd replies with the reply length as the status, so only -1 is an error. */
        if (MsgSend(vehicle, &get, sizeof get, &speed, sizeof speed) == -1) {
            perror(CLIENT_NAME ": get speed");
            return 1;
        }
        if (MsgSendPulse(watchdog, CLIENT_PRIORITY, WATCHDOG_HEARTBEAT_CODE, CLIENT_ID_INFOTAINMENT) == -1) {
            perror(CLIENT_NAME ": heartbeat");
            return 1;
        }
        struct timespec pause = { 0, 250 * 1000 * 1000 };
        nanosleep(&pause, NULL);
    }
}
