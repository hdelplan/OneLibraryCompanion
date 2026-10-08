#include <sys/socket.h>
#include <sys/un.h>
#include <netinet/in.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = {.sun_family = AF_UNIX};
    if (strlen(argv[1]) >= sizeof(address.sun_path)) return 2;
    strcpy(address.sun_path, argv[1]);
    if (connect(fd, (struct sockaddr *)&address, sizeof(address))) return 2;
    uint32_t error;
    if (recv(fd, &error, sizeof(error), MSG_WAITALL) != sizeof(error)) return 2;
    close(fd);
    return ntohl(error) == 0 ? 0 : 1;
}
