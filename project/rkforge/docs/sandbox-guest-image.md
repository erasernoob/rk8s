# Sandbox Guest Image

`libkrun` now expects the guest image to contain `rkforge` at:

- `/usr/local/bin/rkforge`

When the VM boots on the current libkrun path, libkrun's built-in init is used
to switch to the configured root filesystem and then `krun_set_exec(...)`
launches:

```text
rkforge sandbox-agent --vsock-port 26950
```

The host side of the sandbox runtime connects to the corresponding libkrun
agent socket and sends `GuestExecRequest` / `GuestExecResponse` messages.

## Build a guest image

1. Build `rkforge`:

```bash
cargo build
```

2. Prepare a base Linux rootfs that already contains `python3`.

3. Build an ext4 image:

```bash
tools/build-sandbox-guest-rootfs.sh \
  --rootfs-dir /path/to/rootfs \
  --output /tmp/rkforge-sandbox.ext4 \
  --rkforge-bin target/debug/rkforge
```

4. Point libkrun at the resulting image:

```bash
export RKFORGE_SANDBOX_GUEST_IMAGE=/tmp/rkforge-sandbox.ext4
export RKFORGE_SANDBOX_VMM=libkrun
```

Optional:

- Set `RKFORGE_SANDBOX_AGENT_VSOCK_PORT` in the guest environment if you need a
  non-default port. The default is `26950`.
- Set `RKFORGE_SANDBOX_READY_VSOCK_PORT` if the guest ready channel must use a
  non-default port. The default is `26951`.
