#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <linux/videodev2.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

/* A733 BSP UAPI, excluding the obsolete header under vin_test/. */
struct sensor_config {
    uint32_t width, height, hoffset, voffset, hts, vts, pclk, fps_fixed;
    uint32_t bin_factor, intg_min, intg_max;
    uint32_t intg_mid_min, intg_mid_max, intg_short_min, intg_short_max;
    uint32_t gain_min, gain_max, mbus_code, wdr_mode;
};
_Static_assert(sizeof(struct sensor_config) == 76, "A733 sensor_config ABI");
#define SENSOR_CFG_REQ _IOWR('V', BASE_VIDIOC_PRIVATE + 60, struct sensor_config)

static int (*next_ioctl)(int, unsigned long, ...);

__attribute__((constructor)) static void init(void) {
    next_ioctl = dlsym(RTLD_NEXT, "ioctl");
}

static int is_imx219(int fd) {
    char path[128], device[PATH_MAX], name[64];
    snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(path, device, sizeof(device) - 1);
    if (length < 0) return 0;
    device[length] = '\0';
    const char *base = strrchr(device, '/');
    if (!base || strncmp(base + 1, "v4l-subdev", 10)) return 0;
    if (snprintf(path, sizeof(path), "/sys/class/video4linux/%s/name", base + 1) >= (int)sizeof(path)) return 0;
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    int matched = fgets(name, sizeof(name), f) &&
        (!strcmp(name, "imx219\n") || !strcmp(name, "imx219_2\n"));
    fclose(f);
    return matched;
}

int ioctl(int fd, unsigned long request, ...) {
    va_list ap;
    va_start(ap, request);
    void *arg = va_arg(ap, void *);
    va_end(ap);
    if (!next_ioctl) { errno = ENOSYS; return -1; }
    int result = next_ioctl(fd, request, arg);
    int saved_errno = errno;
    if (result == 0 && request == SENSOR_CFG_REQ && arg) {
        struct sensor_config *s = arg;
        /* Only correct the known 1080p timing tuple. No sensor registers,
         * statistics, gains or ioctl return values are fabricated. */
        if (s->width == 1920 && s->height == 1080 && s->fps_fixed == 1 &&
            s->hts == 3444 && s->vts == 1766 && s->pclk == 182000000 &&
            s->mbus_code == 0x300f && s->wdr_mode == 0 && is_imx219(fd)) {
            s->fps_fixed = 30;
            fprintf(stderr, "microduck: corrected IMX219 1080p fps metadata 1 -> 30 for ISP profile selection\n");
        }
    }
    errno = saved_errno;
    return result;
}
