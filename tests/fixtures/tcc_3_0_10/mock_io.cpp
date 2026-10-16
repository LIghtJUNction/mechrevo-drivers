// SPDX-License-Identifier: GPL-3.0-or-later
// Test-only POSIX boundary. Never call real open/ioctl/close or touch hardware.
#include <cstdlib>
#include <cstring>
#include <cstdarg>
#include <cerrno>
#include <cstdint>
#include <sys/ioctl.h>
#include "tuxedo_io_lib/tuxedo_io_ioctl.h"
static bool enabled(const char *name) { const char *v=std::getenv(name); return !v || std::strcmp(v,"0")!=0; }
extern "C" int __wrap_open(const char *path, int, ...) {
    if (!enabled("TCC_MOCK_OPEN") || std::strcmp(path,"/dev/tuxedo_io")!=0) { errno=ENOENT; return -1; }
    return 9876;
}
extern "C" int __wrap_close(int fd) { return fd==9876 ? 0 : -1; }
extern "C" int __wrap_ioctl(int fd, unsigned long request, ...) {
    if(fd!=9876) { errno=EBADF; return -1; }
    if(request==W_UW_FANAUTO) return enabled("TCC_MOCK_SUCCESS") ? 0 : -1;
    va_list ap;va_start(ap,request);int *value=va_arg(ap,int*);va_end(ap);
    if(request==R_HWCHECK_CL) {*value=0;return 0;}
    if(request==R_HWCHECK_UW) {*value=1;return 0;}
    if(request==R_UW_FANSPEED || request==R_UW_FANSPEED2 || request==R_UW_FAN_TEMP || request==R_UW_FAN_TEMP2) {
        const char *raw=std::getenv("TCC_MOCK_RAW"); *value=raw ? std::atoi(raw) : 146;
        if(!enabled("TCC_MOCK_SUCCESS")){errno=EIO;return -1;} return 0;
    }
    if(request==W_UW_FANSPEED || request==W_UW_FANSPEED2) return enabled("TCC_MOCK_SUCCESS") ? 0 : -1;
    errno=EINVAL;return -1;
}
