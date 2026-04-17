# Vendored Sandbox Runtime Sources

This directory is reserved for vendored sandbox runtime dependencies.

Expected layout:

```text
vendor/
  libkrun/
  libkrunfw/
```

When both source trees are present, `rkforge` build-time sandbox runtime
preparation prefers them automatically.

Relevant environment variables:

- `RKFORGE_LIBKRUN_SRC_DIR`
- `RKFORGE_LIBKRUNFW_SRC_DIR`

Build-time dependency mode:

- default:
  - use vendored source if available
  - otherwise fall back to system-installed `libkrun` / `libkrunfw`
- `RKFORGE_SANDBOX_DEPS_STUB=1`
  - skip sandbox runtime embedding
- `RKFORGE_SANDBOX_DEPS_STUB=2`
  - require prebuilt runtime from one of:
    - `runtime/current/lib`
    - `RKFORGE_RUNTIME_LIB_DIR`
    - `RKFORGE_RUNTIME_TARBALL`
    - `RKFORGE_RUNTIME_URL`
- `RKFORGE_SANDBOX_DEPS_STUB=3`
  - force system-installed library discovery

Development/runtime assembly entrypoint:

```bash
scripts/build/build-runtime.sh
```

To package a prebuilt runtime tarball:

```bash
tools/build-sandbox-runtime.sh --archive runtime/current.tar.gz
```
