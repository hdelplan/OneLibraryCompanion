#define main olc_daemon_main
#include "../helper.c"
#undef main
// Exercises the production audit-token/signature check without installing a daemon.
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un address = {.sun_family = AF_UNIX};
    if (strlen(argv[1]) >= sizeof(address.sun_path)) return 2;
    strcpy(address.sun_path, argv[1]);
    if (bind(listener, (struct sockaddr *)&address, sizeof(address)) || listen(listener, 4)) return 2;
    int client = accept(listener, NULL, NULL);
    if (client < 0) return 2;
    reply(client, trusted(client, argv[2], geteuid()) ? 0 : EACCES, -1);
    close(client); close(listener); unlink(argv[1]);
    return 0;
}
