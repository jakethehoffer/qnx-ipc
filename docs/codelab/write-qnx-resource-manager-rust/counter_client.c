/* Uses /dev/counter from C: reads the count with devctl(), resets it, then
 * reads the file with read(). */
#include <devctl.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* The same command numbers as in the Rust device. */
#define DCMD_COUNTER_GET   __DIOF(_DCMD_MISC, 1, uint64_t)
#define DCMD_COUNTER_RESET __DION(_DCMD_MISC, 2)

int main(void)
{
    int fd = open("/dev/counter", O_RDWR);
    if (fd == -1) {
        perror("open /dev/counter");
        return 1;
    }

    uint64_t count = 0;
    int err = devctl(fd, DCMD_COUNTER_GET, &count, sizeof count, NULL);
    if (err != EOK) {
        fprintf(stderr, "DCMD_COUNTER_GET: %s\n", strerror(err));
        return 1;
    }
    printf("devctl: the count is %" PRIu64 "\n", count);

    err = devctl(fd, DCMD_COUNTER_RESET, NULL, 0, NULL);
    if (err != EOK) {
        fprintf(stderr, "DCMD_COUNTER_RESET: %s\n", strerror(err));
        return 1;
    }
    printf("devctl: reset the count\n");

    char text[32];
    ssize_t n = read(fd, text, sizeof text - 1);
    if (n == -1) {
        perror("read /dev/counter");
        return 1;
    }
    text[n] = '\0';
    printf("read: %s", text);

    close(fd);
    return 0;
}
