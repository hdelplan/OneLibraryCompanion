#ifndef PIONEER_COMPANION_BRIDGE_H
#define PIONEER_COMPANION_BRIDGE_H
#include <stddef.h>
#include <stdint.h>
uint16_t pc_start(const char *root, char *error, size_t capacity);
void pc_stop(void);
int32_t pc_local_usb(const char *id, const char *path, char *error, size_t capacity);
int32_t pc_usb_file(const char *path, char *error, size_t capacity);
#endif
