#!/usr/bin/env bash
# Build the k3fall Pi 5 SD image: Raspberry Pi OS Lite (arm64) + SSH +
# SDRplay API + SoapySDRPlay3 + k3fall (baked binary) + readsb/tar1090 ADS-B.
#
# Runs on macOS (Apple Silicon) or Linux with Docker. On Apple Silicon the
# Pi rootfs chroots natively (no qemu). Requires a docker daemon (colima ok).
#
# Usage: ./build-image.sh          # output: image/k3fall-pi5.img.xz
set -euo pipefail

RASPIOS_URL="https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2026-06-19/2026-06-18-raspios-trixie-arm64-lite.img.xz"
SDRPLAY_RUN_URL="https://sdrplay.com/download/hardware-api-linux/?wpdmdl=1906"  # 3.15, makeself
SOAPY_DEB_URL="https://luarvique.github.io/ppa/trixie/arm64/soapysdr0.8-module-sdrplay3_0.8.11_arm64.deb"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$REPO_DIR/image"
DEFAULT_USER="jsvana"
DEFAULT_PASS="k3fall"   # change after first login
HOSTNAME="k3fall"

# 1. Build the k3fall arm64 binary against trixie (matches the OS image).
docker run --rm -v "$REPO_DIR":/src:ro -v k3fall-build:/work debian:trixie bash -c '
  set -e
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config curl \
    ca-certificates libudev-dev libclang-dev libsoapysdr-dev >/dev/null
  curl -sSf https://sh.rustup.rs | sh -s -- -y -q --default-toolchain stable >/dev/null 2>&1
  . "$HOME/.cargo/env"
  cp -r /src /build && cd /build && rm -rf target
  cargo build --release --features soapy
  cp target/release/k3fall /work/k3fall-arm64'

# 2. Customize the image in a privileged container.
docker rm -f pibuild >/dev/null 2>&1 || true
docker run -d --privileged --name pibuild -v k3fall-build:/work debian:trixie sleep infinity
docker exec pibuild bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive \
  apt-get install -y -qq xz-utils curl parted mount e2fsprogs fdisk ca-certificates >/dev/null'

docker exec -i pibuild tee /work/k3fall-config.toml < "$REPO_DIR/config.example.toml" >/dev/null
docker exec -i pibuild tee /work/k3fall.service < "$REPO_DIR/deploy/k3fall.service" >/dev/null

docker exec -e RASPIOS_URL="$RASPIOS_URL" -e SDRPLAY_RUN_URL="$SDRPLAY_RUN_URL" \
  -e SOAPY_DEB_URL="$SOAPY_DEB_URL" -e DEFAULT_USER="$DEFAULT_USER" \
  -e DEFAULT_PASS="$DEFAULT_PASS" -e HOSTNAME="$HOSTNAME" pibuild bash -c '
set -euo pipefail
cd /work
curl -sSL -o base.img.xz "$RASPIOS_URL"
xz -d base.img.xz && mv base.img k3fall-pi5.img
truncate -s +1500M k3fall-pi5.img
parted -s k3fall-pi5.img resizepart 2 100%
# partition offsets (bytes) from parted; loop-mount without -P (containers lack partdev events)
BOOT_OFF=$(parted -sm k3fall-pi5.img unit B print | awk -F: "/^1:/{gsub(/B/,\"\",\$2);print \$2}")
BOOT_SIZE=$(parted -sm k3fall-pi5.img unit B print | awk -F: "/^1:/{gsub(/B/,\"\",\$4);print \$4}")
ROOT_OFF=$(parted -sm k3fall-pi5.img unit B print | awk -F: "/^2:/{gsub(/B/,\"\",\$2);print \$2}")
BOOT=$(losetup -f --show -o "$BOOT_OFF" --sizelimit "$BOOT_SIZE" k3fall-pi5.img)
ROOT=$(losetup -f --show -o "$ROOT_OFF" k3fall-pi5.img)
e2fsck -pf "$ROOT" >/dev/null || true
resize2fs "$ROOT"
mkdir -p /mnt/pi && mount "$ROOT" /mnt/pi && mount "$BOOT" /mnt/pi/boot/firmware
for d in dev proc sys; do mount --bind /$d /mnt/pi/$d; done
mount -t devpts devpts /mnt/pi/dev/pts || true
cp /etc/resolv.conf /mnt/pi/etc/resolv.conf

# --- identity + headless access ---
echo "$HOSTNAME" > /mnt/pi/etc/hostname
sed -i "s/raspberrypi/$HOSTNAME/g" /mnt/pi/etc/hosts
touch /mnt/pi/boot/firmware/ssh
HASH=$(chroot /mnt/pi openssl passwd -6 "$DEFAULT_PASS")
echo "$DEFAULT_USER:$HASH" > /mnt/pi/boot/firmware/userconf.txt

# --- SDRplay API (proprietary daemon + lib, arm64) ---
curl -sSL -A "Mozilla/5.0" -e "https://sdrplay.com/hardware-api/" -o api.run "$SDRPLAY_RUN_URL"
head -c 2 api.run | grep -q "#!" || { echo "SDRplay download did not return an installer"; exit 1; }
sh api.run --noexec --target sdrplay-extract >/dev/null
install -m644 sdrplay-extract/arm64/libsdrplay_api.so.3.* /mnt/pi/usr/local/lib/
( cd /mnt/pi/usr/local/lib && V=$(ls libsdrplay_api.so.3.* | sed "s/libsdrplay_api.so.//") && \
  ln -sf "libsdrplay_api.so.$V" libsdrplay_api.so.3 && ln -sf libsdrplay_api.so.3 libsdrplay_api.so )
install -m755 sdrplay-extract/arm64/sdrplay_apiService /mnt/pi/usr/local/bin/
install -m644 sdrplay-extract/inc/*.h /mnt/pi/usr/local/include/
cat > /mnt/pi/etc/udev/rules.d/66-sdrplay.rules <<EOF
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="2500",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3000",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3010",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3020",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3030",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3050",MODE:="0666"
SUBSYSTEM=="usb",ENV{DEVTYPE}=="usb_device",ATTRS{idVendor}=="1df7",ATTRS{idProduct}=="3060",MODE:="0666"
EOF
cat > /mnt/pi/etc/systemd/system/sdrplay.service <<EOF
[Unit]
Description=SDRplay API Service
After=network.target

[Service]
ExecStart=/usr/local/bin/sdrplay_apiService
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# --- Soapy sdrplay3 module (prebuilt, OpenWebRX+ PPA) ---
curl -sSL -o /mnt/pi/tmp/soapy-sdrplay3.deb "$SOAPY_DEB_URL"

chroot /mnt/pi bash -c "
set -e
export DEBIAN_FRONTEND=noninteractive PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ldconfig
apt-get update -qq >/dev/null
apt-get install -y -qq libsoapysdr0.8 soapysdr-tools avahi-daemon rtl-sdr librtlsdr0 lighttpd wget >/dev/null
dpkg -i /tmp/soapy-sdrplay3.deb >/dev/null
SoapySDRUtil --info 2>/dev/null | grep -q sdrplay

# --- readsb (build from source; build deps removed later) ---
apt-get install -y -qq build-essential pkg-config git libusb-1.0-0-dev librtlsdr-dev libncurses-dev zlib1g-dev libzstd-dev unzip jq >/dev/null
echo blacklist dvb_usb_rtl28xxu > /etc/modprobe.d/blacklist-rtl-dvb.conf
git clone -q --depth 1 https://github.com/wiedehopf/readsb.git /tmp/readsb
cd /tmp/readsb && make -j4 RTLSDR=yes >/dev/null && install -m755 readsb viewadsb /usr/local/bin/
useradd -r -M -s /usr/sbin/nologin readsb 2>/dev/null || true
usermod -aG plugdev readsb

# --- tar1090 (systemctl shim so its installer works in a chroot) ---
dpkg-divert --local --rename --add /usr/bin/systemctl >/dev/null
printf \"#!/bin/bash\ncase \\\"\\\$1\\\" in enable|disable|is-enabled|mask|unmask) exec /usr/bin/systemctl.distrib \\\"\\\$@\\\";; *) exit 0;; esac\n\" > /usr/bin/systemctl
chmod +x /usr/bin/systemctl
git clone -q --depth 1 https://github.com/wiedehopf/tar1090.git /tmp/tar1090
cd /tmp/tar1090 && bash install.sh /run/readsb >/dev/null 2>&1 || true
rm -f /usr/bin/systemctl && dpkg-divert --local --rename --remove /usr/bin/systemctl >/dev/null
test -f /usr/local/share/tar1090/html/index.html

# --- k3fall user ---
useradd -r -M -s /usr/sbin/nologin k3fall 2>/dev/null || true
usermod -aG dialout,plugdev k3fall
"

# --- readsb config + unit ---
cat > /mnt/pi/etc/default/readsb <<EOF
# Set your receiver location for max-range plausibility + future MLAT:
#   add to RECEIVER_OPTIONS: --lat 34.xxxx --lon -118.xxxx
RECEIVER_OPTIONS="--device-type rtlsdr --device-index 0 --gain auto --ppm 0"
DECODER_OPTIONS="--max-range 450 --write-json /run/readsb --json-location-accuracy 2 --quiet"
NET_OPTIONS="--net --net-heartbeat 60 --net-ri-port 30001 --net-ro-port 30002 --net-sbs-port 30003 --net-bi-port 30004,30104 --net-bo-port 30005"
# Anonymous community aggregators (no account needed). Remove to stop feeding.
FEED_OPTIONS="--net-connector feed.adsb.fi,30004,beast_reduce_plus_out --net-connector feed.airplanes.live,30004,beast_reduce_plus_out"
EOF
cat > /mnt/pi/etc/systemd/system/readsb.service <<EOF
[Unit]
Description=readsb ADS-B decoder
Wants=network.target
After=network.target

[Service]
User=readsb
EnvironmentFile=/etc/default/readsb
ExecStart=/usr/local/bin/readsb \$RECEIVER_OPTIONS \$DECODER_OPTIONS \$NET_OPTIONS \$FEED_OPTIONS
RuntimeDirectory=readsb
RuntimeDirectoryMode=0755
Restart=on-failure
RestartSec=5
Nice=-5

[Install]
WantedBy=multi-user.target
EOF

# --- k3fall binary + config + unit ---
install -m755 /work/k3fall-arm64 /mnt/pi/usr/local/bin/k3fall
mkdir -p /mnt/pi/etc/k3fall
cp /work/k3fall-config.toml /mnt/pi/etc/k3fall/config.toml
cp /work/k3fall.service /mnt/pi/etc/systemd/system/k3fall.service

chroot /mnt/pi bash -c "
set -e
export DEBIAN_FRONTEND=noninteractive PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
systemctl enable ssh sdrplay readsb k3fall >/dev/null 2>&1
ldd /usr/local/bin/k3fall | grep -q \"not found\" && exit 1 || true
apt-get purge -y -qq build-essential pkg-config git libusb-1.0-0-dev librtlsdr-dev libncurses-dev zlib1g-dev libzstd-dev unzip jq >/dev/null 2>&1
apt-get autoremove -y -qq >/dev/null 2>&1
apt-get clean
rm -rf /tmp/* /var/lib/apt/lists/*
echo > /etc/resolv.conf
"

umount -R /mnt/pi
e2fsck -pf "$ROOT" >/dev/null || true
losetup -D
xz -T0 -6 -f k3fall-pi5.img'

# 3. Copy out.
docker cp pibuild:/work/k3fall-pi5.img.xz "$OUT_DIR/"
docker rm -f pibuild >/dev/null
( cd "$OUT_DIR" && shasum -a 256 k3fall-pi5.img.xz > k3fall-pi5.img.xz.sha256 )
echo "Built: $OUT_DIR/k3fall-pi5.img.xz"
