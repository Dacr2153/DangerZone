#include <tunables/global>

# Basic confinement for the Dangerzone conversion container.
#
# Podman applies SELinux labeling by default on most hosts; this profile is an
# optional AppArmor layer for distributions that enforce it. The conversion
# container drops all Linux capabilities, runs as an unprivileged user, and is
# started with the seccomp allowlist in `seccomp.json`, so this profile only
# restricts broad file-system access on top of that.
profile dangerzone-sandbox flags=(attach_disconnected,mediate_deleted) {
  #include <abstractions/base>
  #include <abstractions/fonts>

  # The conversion binary and its pinned PDFium library.
  /opt/dangerzone/** mr,

  # System libraries and LibreOffice installation.
  /usr/** mr,
  /lib/** mr,
  /lib64/** mr,

  # Read-only configuration the dynamic loader needs.
  /etc/ld.so.cache r,
  /etc/ld.so.conf r,
  /etc/ld.so.conf.d/** r,
  /etc/localtime r,

  # Scratch space: LibreOffice's user profile and staged input/output files.
  /home/dangerzone/** rwk,
  /tmp/** rwk,
  /var/tmp/** rwk,

  # Basic device and pseudo-filesystem access.
  /proc/** r,
  /sys/devices/system/cpu/online r,
  /dev/null rw,
  /dev/urandom r,
  /dev/random r,
  /dev/zero rw,
  /dev/shm/** rw,
}
