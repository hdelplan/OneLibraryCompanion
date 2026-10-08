// SPDX-License-Identifier: GPL-3.0-only
// Fixed-port socket broker. It never reads music or accepts network traffic.
#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#include <bsm/audit.h>
#include <launch.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <syslog.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define HASH_FILE "/Library/Application Support/OneLibraryCompanion/Networking/host.cdhash"

static bool trusted(int client, const char *hash_file, uid_t owner) {
    uid_t uid; gid_t gid;
    audit_token_t token;
    socklen_t size = sizeof(token);
    if (getpeereid(client, &uid, &gid) || uid < 501 ||
        getsockopt(client, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &size) || size != sizeof(token)) return false;
    int file = open(hash_file, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (file < 0) return false;
    struct stat st;
    char hash[42] = {0};
    bool valid = !fstat(file, &st) && S_ISREG(st.st_mode) && st.st_uid == owner &&
        !(st.st_mode & 022) && st.st_size == 41 && read(file, hash, 41) == 41 && hash[40] == '\n';
    close(file);
    if (!valid) return false;
    hash[40] = 0;
    for (int i = 0; i < 40; i++) if (!hash[i] || !strchr("0123456789abcdef", hash[i])) return false;
    CFDataRef data = CFDataCreate(NULL, (const UInt8 *)&token, sizeof(token));
    const void *keys[] = {kSecGuestAttributeAudit};
    const void *values[] = {data};
    CFDictionaryRef attributes = CFDictionaryCreate(NULL, keys, values, 1,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    SecCodeRef code = NULL;
    SecRequirementRef requirement = NULL;
    CFStringRef expression = CFStringCreateWithFormat(NULL, NULL, CFSTR("cdhash H\"%s\""), hash);
    valid = SecCodeCopyGuestWithAttributes(NULL, attributes, kSecCSDefaultFlags, &code) == errSecSuccess &&
        SecRequirementCreateWithString(expression, kSecCSDefaultFlags, &requirement) == errSecSuccess &&
        SecCodeCheckValidity(code, kSecCSStrictValidate, requirement) == errSecSuccess;
    if (code) CFRelease(code);
    if (requirement) CFRelease(requirement);
    CFRelease(expression); CFRelease(attributes); CFRelease(data);
    return valid;
}

static void reply(int client, int error, int socket_fd) {
    uint32_t status = htonl((uint32_t)error);
    struct iovec io = {.iov_base = &status, .iov_len = sizeof(status)};
    union { struct cmsghdr alignment; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    struct msghdr message = {.msg_iov = &io, .msg_iovlen = 1};
    if (socket_fd >= 0) {
        message.msg_control = control.bytes;
        message.msg_controllen = sizeof(control.bytes);
        struct cmsghdr *c = CMSG_FIRSTHDR(&message);
        c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS; c->cmsg_len = CMSG_LEN(sizeof(int));
        memcpy(CMSG_DATA(c), &socket_fd, sizeof(socket_fd));
    }
    // One four-byte reply; a failed/short send closes all local descriptors.
    (void)sendmsg(client, &message, 0);
}

static int open_port(void) {
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) return -1;
    (void)fcntl(fd, F_SETFD, FD_CLOEXEC);
    struct sockaddr_in address = {.sin_len = sizeof(address), .sin_family = AF_INET,
        .sin_port = htons(111), .sin_addr = {.s_addr = htonl(INADDR_ANY)}};
    // Do not share this port with another RPC server.
    if (bind(fd, (struct sockaddr *)&address, sizeof(address))) {
        int error = errno; close(fd); errno = error; return -1;
    }
    return fd;
}

static void handle(int client) {
    int no_sigpipe = 1;
    struct timeval timeout = {.tv_sec = 2};
    (void)setsockopt(client, SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe, sizeof(no_sigpipe));
    (void)setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    (void)setsockopt(client, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    if (!trusted(client, HASH_FILE, 0)) { reply(client, EACCES, -1); return; }
    char request[4];
    if (recv(client, request, sizeof(request), MSG_WAITALL) != sizeof(request)) return;
    if (!memcmp(request, "OLC?", 4)) { reply(client, 0, -1); return; }
    if (memcmp(request, "OLC1", 4)) { reply(client, EINVAL, -1); return; }
    int fd = open_port();
    if (fd < 0) { reply(client, errno, -1); return; }
    reply(client, 0, fd);
    close(fd); // The approved app owns the socket lifetime after transfer.
}

int main(void) {
    if (geteuid() != 0) { fprintf(stderr, "Start through the installed system service.\n"); return 1; }
    int *listeners = NULL; size_t count = 0;
    int error = launch_activate_socket("Broker", &listeners, &count);
    if (error || count != 1) { free(listeners); return 1; }
    int listener = listeners[0]; free(listeners);
    for (;;) {
        struct pollfd p = {.fd = listener, .events = POLLIN};
        int result = poll(&p, 1, -1);
        if (result < 0 && errno == EINTR) continue;
        if (result <= 0 || !(p.revents & POLLIN)) break;
        int client = accept(listener, NULL, NULL);
        if (client < 0) continue;
        handle(client);
        close(client);
    }
    close(listener);
    return 1;
}
