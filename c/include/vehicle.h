/*
 * The vehicle-signals demo protocol, shared by the C infotainment client
 * (c/infotainment_client.c) and the Rust programs in demo/. The Rust copy is
 * demo/vsigd/src/proto.rs. Both sides assert the struct sizes as fixed numbers,
 * so a change that alters a size fails to build until that side's assertion is
 * changed too. A change that keeps the sizes (reordered or retyped fields) is
 * not caught.
 *
 * vsigd registers the name VEHICLE_NAME. A client opens it with name_open,
 * sends one struct vehicle_hello naming itself, then any number of
 * struct vehicle_get_signal requests, each answered with struct vehicle_signal.
 * vsigd replies with the reply's byte length as the MsgReply status (what
 * qnx-ipc's Request::reply does), so MsgSend returns that length on success.
 * Only -1 means failure.
 *
 * The watchdog registers WATCHDOG_NAME. Every client sends it a pulse with code
 * WATCHDOG_HEARTBEAT_CODE and its client id as the value.
 */
#ifndef VEHICLE_H
#define VEHICLE_H

#include <stdint.h>

#define VEHICLE_NAME "vehicle"
#define WATCHDOG_NAME "watchdog"

/* Message kinds (user kinds start at 0x200). */
#define VEHICLE_GET_SIGNAL 0x210
#define VEHICLE_HELLO 0x211

/* Signal ids for struct vehicle_get_signal. */
#define VEHICLE_SIGNAL_SPEED 1 /* km/h */
#define VEHICLE_SIGNAL_BRAKE 2 /* brake pressure, kPa */

/* Heartbeat pulse code; the pulse value is the client id. */
#define WATCHDOG_HEARTBEAT_CODE 10

/* Client ids (heartbeat pulse values). */
#define CLIENT_ID_SAFETY 1
#define CLIENT_ID_INFOTAINMENT 2

/* Length of the NUL-padded client name in struct vehicle_hello. */
#define VEHICLE_NAME_LEN 32

struct vehicle_hdr {
    uint16_t kind;     /* VEHICLE_GET_SIGNAL or VEHICLE_HELLO */
    uint16_t reserved; /* always zero */
};

/* Sent once per connection: who the client is. Answered with an empty reply. */
struct vehicle_hello {
    struct vehicle_hdr hdr;
    char name[VEHICLE_NAME_LEN]; /* NUL-padded, need not be NUL-terminated */
};

/* Asks for the current value of one signal. */
struct vehicle_get_signal {
    struct vehicle_hdr hdr;
    uint32_t signal_id; /* VEHICLE_SIGNAL_SPEED or VEHICLE_SIGNAL_BRAKE */
};

/* The reply to struct vehicle_get_signal: the value in thousandths of the unit. */
struct vehicle_signal {
    int64_t value_milli;
};

_Static_assert(sizeof(struct vehicle_hdr) == 4, "vehicle_hdr must be 4 bytes");
_Static_assert(sizeof(struct vehicle_hello) == 36, "vehicle_hello must be 36 bytes");
_Static_assert(sizeof(struct vehicle_get_signal) == 8, "vehicle_get_signal must be 8 bytes");
_Static_assert(sizeof(struct vehicle_signal) == 8, "vehicle_signal must be 8 bytes");

#endif /* VEHICLE_H */
