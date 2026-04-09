# RKForge libkrun Sandbox Handoff

## Goal

Current task is to bring `src/sandbox/` to a usable MVP on the `libkrun` path, with this lifecycle:

`create -> boot -> ready -> exec -> stop -> remove`

Target capabilities:

- run Python code inside the sandbox guest
- return `stdout/stderr/exit code`
- clean up sandbox resources after execution
- phase 1 ready:
  - VMM started
- phase 2 ready:
  - guest agent is ready to accept exec/control RPC

This handoff summarizes what has already been implemented in the current repo, what is still missing, and what to do next on a Linux bare-metal machine.

## Current Status

### 1. Host runtime and `libkrun` backend wiring

Implemented:

- `MicroVmSandboxBackend` now selects backend by `VmmKind` instead of hardcoding Firecracker.
- Default path can use `libkrun`.
- `build_vm_spec(..., vmm_kind)` is correctly wired.

Relevant files:

- [src/sandbox/mod.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/mod.rs)
- [src/sandbox/vm/mod.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/mod.rs)

### 2. Shim architecture is present

Implemented:

- host runtime serializes a VM spec
- `sandbox-shim` subprocess is spawned
- `run_libkrun_shim(spec)` owns the `krun_*` path

Relevant file:

- [src/sandbox/vm/libkrun.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/libkrun.rs)

### 3. Core `libkrun` FFI mapping exists

Implemented in `configure_ctx(...)`:

- `krun_create_ctx`
- `krun_set_vm_config`
- `krun_set_kernel`
- `krun_add_disk`
- `krun_set_root_disk_remount`
- `krun_add_vsock_port2`
- `krun_start_enter`

This means the `libkrun` driver layer is already partially aligned with the BoxLite architecture.

### 4. Minimal guest agent exists

Implemented:

- hidden command `rkforge sandbox-agent`
- guest agent listens on `AF_VSOCK`
- host sends `GuestExecRequest`
- guest returns `GuestExecResponse`
- guest supports:
  - Python inline exec via `python3 -c ...`
  - generic command execution
  - timeout handling

Relevant file:

- [src/sandbox/agent.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/agent.rs)

### 5. Guest bootstrap exists using a temporary traditional route

Implemented:

- `rkforge` can run as guest `PID 1`
- if launched as `/sbin/init`, it switches into guest-init mode
- guest-init mounts minimal guest filesystems
- guest-init `exec`s:

```text
rkforge sandbox-agent --vsock-port ...
```

Important:

- this is only a validation bootstrap path
- it is not yet aligned with BoxLite's final `krun_set_exec(boxlite-guest)` model

Relevant file:

- [src/sandbox/guest.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/guest.rs)

### 6. Ready path has been improved

Originally:

- `libkrun` shim wrote `ready_file` too early

Now:

- guest-init passes `sandbox_id` and `ready_vsock_port` into `sandbox-agent`
- `sandbox-agent` actively notifies host readiness over the ready vsock bridge
- host-side ready listener writes the received `GuestReadyEvent` into `ready_file`
- `wait_ready()` still polls `ready_file`, but the file is now host-side cached output from a real guest-originated signal instead of shim-faked readiness

Relevant files:

- [src/sandbox/agent.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/agent.rs)
- [src/sandbox/guest.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/guest.rs)
- [src/sandbox/vm/libkrun.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/libkrun.rs)

### 7. Guest image build helper exists

Implemented:

- helper script to inject `rkforge` into a Linux rootfs
- installs:
  - `/usr/local/bin/rkforge`
  - `/sbin/init`
- builds an ext4 image for `RKFORGE_SANDBOX_GUEST_IMAGE`

Relevant files:

- [tools/build-sandbox-guest-rootfs.sh](/home/erasernoob/project/rk8s/project/rkforge/tools/build-sandbox-guest-rootfs.sh)
- [docs/sandbox-guest-image.md](/home/erasernoob/project/rk8s/project/rkforge/docs/sandbox-guest-image.md)

### 8. Build status

At the point of handoff:

- `cargo check` passes

This means the current source tree is in a compilable state.

## Important Architectural Note

BoxLite's real production bootstrap model is not `/sbin/init -> agent`.

Per:

- [docs/boxlite-research.md](/home/erasernoob/project/rk8s/project/rkforge/docs/boxlite-research.md)
- [docs/boxlite-encapsulation.md](/home/erasernoob/project/rk8s/project/rkforge/docs/boxlite-encapsulation.md)

BoxLite does this instead:

1. pull OCI bootstrap image
2. convert layers to pure ext4 disk
3. inject guest binary
4. cache shared base rootfs
5. create per-box overlay
6. use `krun_set_exec(...)` to directly execute guest binary

Current RKForge implementation is **not yet aligned** with this final bootstrap model.

However, the temporary `/sbin/init -> sandbox-agent` path is intentional and useful as an intermediate milestone for validating:

- VM can boot
- guest can run
- ready path works
- exec path works

After successful end-to-end validation on bare metal, next step should be to evolve toward the BoxLite-style bootstrap path.

## What Is Not Done Yet

### 1. Real end-to-end validation has not been completed

Reason:

- current machine is a VMware guest
- it does not expose nested hardware virtualization
- `/dev/kvm` is missing
- CPU flags `vmx/svm` are missing

Observed environment:

- `systemd-detect-virt` returned `vmware`
- `/dev/kvm` not present
- `/dev/vhost-vsock` present, but that alone is not enough

Conclusion:

- this VMware guest is not suitable for real `libkrun` VM launch validation

### 2. `libkrun` runtime assets are not installed

Checked on current machine:

- no `libkrun.so`
- no `libkrunfw.so`

So on the bare-metal machine, these must be installed or downloaded first.

### 3. `rkforge` binary was not built yet

At the time of environment inspection:

- `target/debug/rkforge` did not exist

So on the bare-metal machine, you must build it first.

### 4. Candidate guest rootfs input is not ready

Local archive checked:

- `ubuntu-latest.tar`

Problem:

- it is not a plain rootfs tar
- it contains a `lower/` prefix layout
- it also did not clearly show `python3`

So it should **not** be assumed to be directly usable with the current guest image build helper.

### 5. Control plane is still minimal

Still missing:

- `Ping`
- `Shutdown`
- graceful stop/remove via RPC
- guest-side flush/sync before shutdown

Current `stop()` still kills shim/vmm directly.

### 6. BoxLite-style bootstrap encapsulation is not implemented

Still missing:

- bootstrap OCI image -> pure ext4 conversion pipeline
- guest binary injection as a managed runtime asset
- `krun_set_exec(...)` direct guest entrypoint path
- shared guest rootfs cache
- per-box overlay/cow image model
- runtime bundle / binary finder for shim + guest + `libkrunfw`

## Recommended Next Steps On Bare Metal

### Step 1. Verify host machine can actually run libkrun

Run:

```bash
systemd-detect-virt
ls -l /dev/kvm
egrep -o 'vmx|svm' /proc/cpuinfo | sort -u
```

Expected:

- ideally bare metal, or at least nested virtualization enabled
- `/dev/kvm` must exist
- `vmx` or `svm` must appear

If these are missing, stop here. Real `libkrun` validation is blocked.

### Step 2. Install or provide `libkrun` and `libkrunfw`

At minimum, you need usable library files and these env vars:

```bash
export RKFORGE_LIBKRUN_LIBRARY=/path/to/libkrun.so
export RKFORGE_LIBKRUNFW_PATH=/path/to/libkrunfw.so
```

This was missing on the VMware machine.

### Step 3. Build RKForge

Run:

```bash
cargo build
```

After that, confirm:

```bash
ls -lh target/debug/rkforge
```

### Step 4. Prepare a proper guest rootfs

Need a Linux rootfs directory or tar that contains at least:

- `/bin/sh`
- `python3`
- standard userspace layout

Preferred:

- Debian/Ubuntu rootfs tar or unpacked rootfs directory

Do not assume `ubuntu-latest.tar` from this workspace is directly usable.

### Step 5. Build a guest ext4 image

If using a plain rootfs directory:

```bash
tools/build-sandbox-guest-rootfs.sh \
  --rootfs-dir /path/to/rootfs \
  --output /tmp/rkforge-sandbox.ext4 \
  --rkforge-bin target/debug/rkforge
```

If using a plain rootfs tar:

```bash
tools/build-sandbox-guest-rootfs.sh \
  --rootfs-tar /path/to/rootfs.tar \
  --output /tmp/rkforge-sandbox.ext4 \
  --rkforge-bin target/debug/rkforge
```

### Step 6. Set validation env vars

```bash
export RKFORGE_SANDBOX_VMM=libkrun
export RKFORGE_SANDBOX_GUEST_IMAGE=/tmp/rkforge-sandbox.ext4
export RKFORGE_LIBKRUN_LIBRARY=/path/to/libkrun.so
export RKFORGE_LIBKRUNFW_PATH=/path/to/libkrunfw.so
```

Optional if needed:

```bash
export RKFORGE_SANDBOX_KERNEL=/path/to/kernel
export RKFORGE_SANDBOX_INITRD=/path/to/initrd
```

Note:

- current code can use explicit kernel if configured
- but long-term design should move closer to BoxLite's encapsulated bootstrap model

### Step 7. Perform the first real end-to-end validation

Recommended first acceptance target:

- VM boots
- guest agent starts
- guest sends ready
- host marks sandbox ready
- `exec_python("print('hello')")` returns output

Suggested CLI shape to exercise:

```bash
cargo run -- sandbox create
cargo run -- sandbox exec <sandbox_id> --python "print('hello')"
cargo run -- sandbox stop <sandbox_id>
cargo run -- sandbox remove <sandbox_id>
```

If CLI ergonomics differ, adapt accordingly. The core target is ready + exec.

### Step 8. Only after end-to-end boot succeeds, implement control RPC

Next coding priority after first real VM validation:

1. add `Ping`
2. add `Shutdown`
3. make `stop/remove` graceful
4. then move bootstrap toward BoxLite's `krun_set_exec(...)` model

This order is deliberate:

- first prove current libkrun VM path works
- then improve control plane
- then align bootstrap/productization with BoxLite

## Current File Set Touched During This Work

- [src/sandbox/mod.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/mod.rs)
- [src/sandbox/protocol.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/protocol.rs)
- [src/sandbox/vm/mod.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/mod.rs)
- [src/sandbox/vm/libkrun.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/libkrun.rs)
- [src/sandbox/vm/firecracker.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/vm/firecracker.rs)
- [src/sandbox/agent.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/agent.rs)
- [src/sandbox/guest.rs](/home/erasernoob/project/rk8s/project/rkforge/src/sandbox/guest.rs)
- [src/main.rs](/home/erasernoob/project/rk8s/project/rkforge/src/main.rs)
- [src/args.rs](/home/erasernoob/project/rk8s/project/rkforge/src/args.rs)
- [tools/build-sandbox-guest-rootfs.sh](/home/erasernoob/project/rk8s/project/rkforge/tools/build-sandbox-guest-rootfs.sh)
- [docs/sandbox-guest-image.md](/home/erasernoob/project/rk8s/project/rkforge/docs/sandbox-guest-image.md)

## Short Summary For The Next Assistant

The repo already contains a compilable partial `libkrun` sandbox path with:

- backend selection
- shim subprocess
- `krun_*` driver mapping
- minimal guest agent
- temporary guest-init bootstrap via `/sbin/init`
- guest-originated ready notification

What has **not** yet been proven is real VM launch on suitable hardware.

The immediate next task on bare metal is:

1. verify `/dev/kvm` and `vmx/svm`
2. install/provide `libkrun` + `libkrunfw`
3. build `rkforge`
4. prepare a proper guest rootfs with Python
5. build guest ext4
6. run the first real end-to-end validation

After that:

- add `Ping/Shutdown`
- make stop/remove graceful
- then align bootstrap with the BoxLite `krun_set_exec(guest-binary)` model
