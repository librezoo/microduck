# Orange Pi Zero 3W camera bring-up

Validated on 2026-10-11: Orange Pi Zero 3W, Allwinner A733 / ISP602, an IMX219
on CAM1, and `6.6.98-sun60iw2-microduck-xbox2`. Colour capture works with the
existing kernel once the sensor's reported frame rate selects the correct ISP
tuning profile. This branch provides standalone camera tools; the Orange Pi
source and encoder are not yet integrated into `mediad` or WebRTC.

## Why the image was green

The BSP's IMX219 1920x1080 mode sets `fps_fixed = 1` despite its approximately
30 fps timing. `libisp_ini.so` then falls back to `imx219_3280_2464_isp_cfg`.
Reporting 30 selects the already available `imx219_1080p_isp_cfg` and removes
the strong green cast. Two A/B runs reproduced the result with identical NV12
conversion, including a run with saved ISP contexts temporarily removed.

[`imx219-isp-compat.c`](../../scripts/orangepi-zero3w-camera/imx219-isp-compat.c)
corrects the returned metadata only for an IMX219 device with the tested timing
tuple and 76-byte A733 `sensor_config` ABI. It preserves ioctl failures and
does not alter sensor registers or fabricate statistics. The capture process
loads it with `LD_PRELOAD`; it is not a system-wide preload.

The equivalent one-line
[`kernel patch`](../../scripts/orangepi-zero3w-camera/imx219-fps-metadata.patch)
changes the source of the metadata. It applies to Orange Pi's official
[`orange-pi-6.6-sun60iw2` source at `2ac08e8`](https://github.com/orangepi-xunlong/linux-orangepi/blob/2ac08e8c7cdc28abbdc5c9a9dd812f887ae9c79f/bsp/drivers/vin/modules/sensor/imx219.c#L707).
The patch passed `git apply --check`; a rebuilt kernel has not been boot-tested.
On a kernel already reporting 30, the compatibility library leaves it unchanged.

## Install on the board

The board must already expose the IMX219 on `/dev/video0`. From this checkout
on the Orange Pi:

```sh
sudo apt-get install gcc libc6-dev curl ca-certificates util-linux \
  gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  libgudev-1.0-0 libv4l-0
sudo sh scripts/orangepi-zero3w-camera/install.sh
sudo microduck-imx219-isp-snapshot /tmp/imx219.jpg
```

The installer downloads two pinned vendor packages, verifies SHA-256, extracts
only the three ISP libraries and `libgstvideo4linux2.so` under
`/opt/microduck-awisp`, and builds the compatibility library on the board. It
does not install the vendor Debian packages into the package manager or replace
distro GStreamer. Existing snapshot scripts and libraries are backed up under
`/opt/microduck-awisp/backups/install.*` on reinstall.

The packages come from
[`radxa/allwininer-debian`, commit `43fe44a`](https://github.com/radxa/allwininer-debian/tree/43fe44a304e8c06c6275769c4731ff82265325a7):

| Package | SHA-256 |
|---|---|
| `packages/arm64/libAWIspApi/libAWIspApi_602_1.0.0_arm64.deb` | `112012255a3f3e7d33faa63241dea8b6e3ab081d1ef63c56c0e656e5b5978fc9` |
| `packages/arm64/gst-plugins-good1.0/debian13/gstreamer1.0-plugins-good_1.26.2-deb13u1.2_arm64.deb` | `95f54109a91ea65ee87f69856d5d00611e31caaade32196cbe8fb32b4a668fff` |

The same-named Orange Pi ISP package was tested and lacks this IMX219 profile;
its fallback selected OV13850. Package names and the ISP602 banner alone are not
enough to identify the working tuning database. Vendor binaries are downloaded
from their source, not committed into this repository.

For an offline install, supply those exact `.deb` files as two arguments.
For a staged install, set `DESTDIR` to an absolute directory. To run the staged
snapshot script, set `MICRODUCK_AWISP_ROOT` to that directory's
`opt/microduck-awisp` subdirectory.

## Capture and validation

The snapshot command captures 300 frames of 1920x1080 NV12 and saves the last
JPEG at quality 95. Allow roughly 10–30 seconds as exposure and white
balance settle: indoor AE can extend the sensor frame interval beyond the
requested 30 fps. The 45-second timeout includes that slower capture and startup.
The output now matches the sensor's 1920x1080 mode; the first bring-up script
downscaled it to 640x480, discarding detail and changing the aspect ratio.
It retains full colour, uses automatic exposure, and avoids competing manual
I2C writes. Its timeout bounds a stalled capture, and a lock prevents concurrent
instances of this snapshot command. Other camera applications must also release
`/dev/video0` before a snapshot.

The accompanying `/tmp/imx219.jpg.log` must contain:

```text
find imx219_1920_1080_30_0 [imx219_1080p_isp_cfg] isp config
```

The command fails if the expected tuning profile was not selected. No
desaturation, software white balance or colour matrix is applied after the ISP.

Instrumented hardware validation of the installed snapshot recorded:

- 300 successful H3A statistics reads and zero failed reads; one error event
  occurred during startup before valid statistics became available.
- 31 successful sensor exposure/gain calls and 301 ISP register loads.
- 301 white-balance gain updates; the four-channel tuple changed from
  `492,256,256,374` to `574,256,256,400`, then settled.
- A second capture without diagnostic instrumentation also selected the correct
  profile and produced a visually correct colour frame.

The initial hypothesis that Linux 6.6 made hardware AE/AWB unusable was incorrect.
TDM subscription and debug-info ioctl warnings remain in the log, but did not
block the measured statistics, exposure or white-balance updates. A single
startup `AEWB: stats error` line must not be interpreted as continuous failure.
Colour-chart calibration and controlled lighting sweeps remain future work.

The repository installer was also run on the board with a temporary `DESTDIR`:
native compilation passed with `-Wall -Wextra -Werror`, and the staged snapshot
selected the expected profile and produced a 640x480 JPEG. A modified package
was rejected before installation, reinstall preserved the previous files in its
backup, and a held camera lock made a second snapshot fail before capture.

## Next integration step

Port the verified source setup and A733 encoding path into `mediad`; the current
standalone tools do not change the existing Radxa pipeline or updater hooks.
WebRTC remains the intended video consumer path, as described in
[`remote-webrtc.md`](../design/remote-webrtc.md). Future camera consumers need
the compatibility library or a kernel carrying the metadata fix.
