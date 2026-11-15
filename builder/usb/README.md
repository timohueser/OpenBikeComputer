# obc-usb

The native USB byte transport and the flat-store client over it, for a computer that talks to the
device over the cable. Its one program is `obc-format-card`, which `obc format-card` runs.

## Linux USB permission

macOS and Windows need no setup. On Linux, the USB device node belongs to root, so a program
cannot open the device until a udev rule gives the person at the machine access. Chrome and Edge
need the same rule for WebUSB.

```sh
sudo cp builder/usb/linux/99-openbikecomputer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
# unplug and reconnect the device
```

## Test

```sh
obc test -p obc-usb
obc exec cargo clippy -p obc-usb --all-targets -- -D warnings
```
