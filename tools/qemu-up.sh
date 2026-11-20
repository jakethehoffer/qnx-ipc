#!/usr/bin/env bash
# Build (once) and boot a minimal QNX 8.0 x86_64 QEMU target, headless, with SSH
# forwarded to 127.0.0.1:$QNX_SSH_PORT. Uses QEMU user-mode networking, so no bridge,
# libvirt or sudo is needed. Safe to run again: it reuses a running target.
set -euo pipefail
: "${QNX_HOST:?run 'source tools/env.sh' first}"

img="$QNX_IMAGE_DIR"
out="$img/output"
port="$QNX_SSH_PORT"
key="$QNX_SSH_KEY"

# ssh-keygen does not create the key's directory, and a fresh host has no ~/.ssh.
mkdir -p "$(dirname "$key")"
[ -f "$key" ] || ssh-keygen -q -t ed25519 -N '' -C qnx-target -f "$key"
mkdir -p "$img/local/snippets"

# The SDP 8.0 OpenSSH sshd needs its sshd-auth helper at /usr/libexec/sshd-auth, but
# mkqnximage's default image only ships sshd-session, so sshd exits at boot with
# "/usr/libexec/sshd-auth does not exist". Put the helper in the boot image at that
# absolute path (a relative path would land under /proc/boot).
snippet="$img/local/snippets/ifs_files.custom"
wanted='# Added by qnx-ipc tools/qemu-up.sh: sshd needs its auth helper at this path.
[perms=555] /usr/libexec/sshd-auth=usr/libexec/sshd-auth'
if [ "$(cat "$snippet" 2>/dev/null)" != "$wanted" ]; then
  printf '%s\n' "$wanted" > "$snippet"
  rebuild=1
fi

if [ ! -f "$out/ifs.bin" ] || [ "${1:-}" = "--rebuild" ] || [ -n "${rebuild:-}" ]; then
  [ -f "$out/qemu.pid" ] && kill "$(cat "$out/qemu.pid")" 2>/dev/null && sleep 2
  echo "Building QNX 8.0 QEMU image in $img"
  (cd "$img" && mkqnximage --clean --type=qemu --arch=x86_64 --ssh-ident="$key.pub" \
     --ram=1536M --cpu=2 --graphics=no --hostname=qnx-ipc --noprompt --force)
fi

if [ -f "$out/qemu.pid" ] && kill -0 "$(cat "$out/qemu.pid")" 2>/dev/null; then
  echo "QNX target already running (pid $(cat "$out/qemu.pid"))"
else
  # Just after WSL starts, udev can take a few seconds to give the kvm group
  # access to /dev/kvm. Starting before then falls back to a far slower guest.
  for _ in $(seq 1 15); do
    { [ ! -e /dev/kvm ] || [ -w /dev/kvm ]; } && break
    sleep 1
  done
  accel=(-cpu max)
  [ -w /dev/kvm ] && accel=(-enable-kvm -cpu host)
  if [ "${accel[0]}" = -enable-kvm ]; then
    echo "Starting the QNX target with KVM"
  else
    echo "Starting the QNX target without KVM (no writable /dev/kvm): expect it to be slow" >&2
  fi
  rm -f "$out/qemu.out"
  # The disk is opened as the raw image, not through mkqnximage's 512-byte
  # disk-qemu.vmdk descriptor: QEMU adds a line to that file on every boot that
  # writes to the disk, and after about 20 boots it overflows, the extent line
  # is cut off and QNX can no longer mount /boot, /system or /data.
  # QNX_QEMU_RAM (MB) lets a host short of memory give the target less. 1024 is
  # enough for every test and the demo.
  qemu-system-x86_64 "${accel[@]}" -smp 2 -m "${QNX_QEMU_RAM:-1536}" \
    -drive "file=$out/disk-qemu,format=raw,if=ide,id=drv0" \
    -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:${port}-:22" \
    -device virtio-net-pci,netdev=net0 \
    -object rng-random,filename=/dev/urandom,id=rng0 -device virtio-rng-pci,rng=rng0 \
    -kernel "$out/ifs.bin" \
    -display none -monitor none -serial "file:$out/qemu.out" \
    -pidfile "$out/qemu.pid" -daemonize
fi

ssh_opts=(-i "$key" -p "$port" -o BatchMode=yes -o StrictHostKeyChecking=no
          -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=3)
for _ in $(seq 1 90); do
  if ssh "${ssh_opts[@]}" root@127.0.0.1 true 2>/dev/null; then
    echo "QNX target up: ssh -i $key -p $port root@127.0.0.1"
    exit 0
  fi
  sleep 2
done
echo "QNX target did not answer on SSH. Boot log: $out/qemu.out" >&2
exit 1
