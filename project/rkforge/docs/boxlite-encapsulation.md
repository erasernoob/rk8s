# BoxLite Guest Bootstrap 与 Runtime 封装报告

## 目的

本文档回答以下问题：

1. BoxLite 里的 guest image 从哪里来
2. guest rootfs 是怎么生成、缓存和复用的
3. `boxlite-guest` 是如何进入 guest rootfs 的
4. `libkrun`、`libkrunfw`、`boxlite-shim`、`boxlite-guest` 这些 runtime 资产是如何被打包、发现和隐藏细节的
5. 如果要复刻 BoxLite 的这套封装，应该照着复刻哪些抽象

## 一句话结论

BoxLite 没有把“guest image”当成最终可启动 VM 镜像直接使用。

它的实际路径是：

1. 拉取一个 OCI 基础镜像
2. 把 OCI layers 转成纯 ext4 image disk
3. 把 `boxlite-guest` 注入这个 ext4
4. 缓存成可复用的 guest rootfs base
5. 每个 box 再从这个 base 创建自己的 qcow2 COW overlay
6. shim 进程通过 libkrun 直接执行 `boxlite-guest`

因此，BoxLite 并不依赖“镜像里的 `/sbin/init -> sandbox-agent`”这类链路。它的 guest agent 是 runtime 自己注入并显式设置为 guest entrypoint 的。

## 1. guest image 从哪里来

BoxLite 的 bootstrap guest image 来源不是单独维护的 VM rootfs 文件，而是一个 OCI 镜像。

当前内置常量定义在：

[constants.rs](/home/erasernoob/project/boxlite/boxlite/src/runtime/constants.rs#L34)

```rust
pub const INIT_ROOTFS: &str = "debian:bookworm-slim";
```

在 guest rootfs 初始化阶段，BoxLite 会调用：

[guest_rootfs.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/guest_rootfs.rs#L55)

```rust
runtime.image_manager.pull(images::INIT_ROOTFS).await
```

所以 BoxLite 的 guest image 来源可以总结为：

- 逻辑来源：OCI image
- 默认镜像：`debian:bookworm-slim`
- 获取方式：走统一的 `ImageManager::pull()`

这意味着 BoxLite 不要求调用方先准备好一个“最终可启动的 guest VM rootfs 镜像文件”。

## 2. guest rootfs 是怎么来的

BoxLite 对 guest rootfs 的构建做了明确的两级封装。

### 第一级：`ImageDiskManager`

职责：

- 把 OCI image layers 解包
- merge 成目录
- 再把目录做成纯 ext4 disk

注意：这一步产出的 disk 里只有镜像内容，没有 `boxlite-guest`。

对应代码：

[image_disk.rs](/home/erasernoob/project/boxlite/boxlite/src/images/image_disk.rs#L18)

它的处理流程是：

1. `RootfsBuilder::prepare(...)` 解包/合并 image layers
2. `create_ext4_from_dir(...)` 把 merged 目录做成 ext4
3. 按 image digest 缓存到 `~/.boxlite/images/disk-images/`

缓存 key 是 image digest。

### 第二级：`GuestRootfsManager`

职责：

- 取上一步的纯 image ext4
- 复制到 staging 临时文件
- 把 `boxlite-guest` 注入 ext4
- 把结果缓存成“可启动 guest rootfs”

对应代码：

[guest.rs](/home/erasernoob/project/boxlite/boxlite/src/rootfs/guest.rs#L291)

代码里写得很明确：

- Stage 1: pure image disk
- Stage 2: image disk + guest binary -> versioned guest rootfs

版本 key 规则：

```text
{image_digest_short}-{guest_hash_short}
```

也就是：

- 同一个基础 OCI image
- 配上不同版本的 `boxlite-guest`
- 会生成不同版本的 guest rootfs base

## 3. `boxlite-guest` 是怎么进入 rootfs 的

`GuestRootfsManager::build_and_install()` 里会：

1. `find_binary("boxlite-guest")`
2. 验证 guest binary 架构
3. 计算 `boxlite-guest` SHA256
4. 用 `inject_file_into_ext4(...)` 注入到 ext4 中

关键代码：

[guest.rs](/home/erasernoob/project/boxlite/boxlite/src/rootfs/guest.rs#L427)

```rust
let guest_bin = util::find_binary("boxlite-guest")?;
crate::vmm::guest_check::validate_guest_binary(&guest_bin)?;
inject_file_into_ext4(&staged_path, &guest_bin, "boxlite/bin/boxlite-guest")?;
```

也就是说，BoxLite 的 guest agent 不是依赖基础镜像内置的 `/sbin/init` 或 service manager 去启动。

它是 runtime 在构建 guest rootfs 时主动注入的。

## 4. guest 到底怎么被执行起来

BoxLite 实际上不走“guest 内 `/sbin/init` 间接拉起 agent”这条思路。

它在 VMM 启动时直接设置 guest entrypoint 为：

```text
/boxlite/bin/boxlite-guest
```

构建入口在：

[vmm_spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/vmm_spawn.rs#L264)

真正交给 libkrun 的位置在：

[engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L170)

再落到 FFI：

[context.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/context.rs#L207)

因此，BoxLite 的验证重点不是：

- “这份 guest image 里的 `/sbin/init` 会不会拉起 sandbox-agent”

而是：

- pure image ext4 能否构建
- `boxlite-guest` 能否注入
- libkrun 能否把它作为 entrypoint 直接执行

## 5. per-box guest rootfs 是怎么复用的

BoxLite 会先构建共享 guest rootfs base，再为每个 box 创建自己的 COW overlay。

共享 base：

- ext4
- 可复用
- 缓存在 bases 目录

per-box：

- `guest-rootfs.qcow2`
- backing file 指向共享 ext4
- restart 时可复用

对应代码：

[guest_rootfs.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/guest_rootfs.rs#L88)

所以 BoxLite 的结构不是“每个 box 都重新准备完整 guest rootfs”，而是：

- 全局共享 base
- 局部独立 overlay

这也是它能把 guest bootstrap 成本摊薄的原因。

## 6. `boxlite-guest`、`boxlite-shim`、`libkrunfw` 是怎么被封装的

这部分是 BoxLite 的另一层关键封装：runtime assets 管理。

### Build-time bundling

`build.rs` 会把 native libs 和预构建二进制收集到统一 runtime 目录。

相关位置：

[build.rs](/home/erasernoob/project/boxlite/boxlite/build.rs#L128)

它通过 `DEP_*_BOXLITE_DEP` 环境变量收集：

- `libkrun`
- `libkrunfw`
- 其他 FFI 依赖

同时还会把：

- `boxlite-shim`
- `boxlite-guest`

复制到 runtime 目录里，见：

[build.rs](/home/erasernoob/project/boxlite/boxlite/build.rs#L625)

### `build-runtime.sh`

构建 runtime 目录的脚本明确声明 runtime 目录包含：

- `boxlite-shim`
- `boxlite-guest`
- `libkrunfw.*`

见：

[build-runtime.sh](/home/erasernoob/project/boxlite/scripts/build/build-runtime.sh#L15)

也就是说，在 BoxLite 设计里，这些运行时资产被当成一套统一 runtime bundle。

### embedded runtime

如果启用了 `embedded-runtime` feature，build.rs 还会生成 `include_bytes!` manifest，把这些 runtime 文件编进库里。

运行时首次访问时，再解压到本地目录：

- release：`~/.local/share/boxlite/runtimes/v{VERSION}/`
- debug：`~/.local/share/boxlite/runtimes/v{VERSION}-{HASH}/`

对应代码：

[embedded.rs](/home/erasernoob/project/boxlite/boxlite/src/runtime/embedded.rs#L1)

这个设计的意义是：

- SDK 用户不一定需要自己单独配置 runtime 路径
- BoxLite 可以自解压 runtime 资产
- 版本和内容 hash 自动参与缓存失效

## 7. runtime 怎么找到这些二进制和动态库

BoxLite 不让上层业务代码直接硬编码路径，而是统一通过 `RuntimeBinaryFinder`。

对应代码：

[binary_finder.rs](/home/erasernoob/project/boxlite/boxlite/src/util/binary_finder.rs#L63)

查找优先级：

1. `BOXLITE_RUNTIME_DIR`
2. embedded runtime cache
3. `LD_LIBRARY_PATH` / `DYLD_*`
4. `dladdr` 推导当前库所在目录

这意味着 `boxlite-shim` 和 `boxlite-guest` 在业务逻辑里都可以简单写成：

```rust
find_binary("boxlite-shim")?;
find_binary("boxlite-guest")?;
```

从而把运行时路径细节隐藏起来。

## 8. `libkrun` 和 `libkrunfw` 在封装上的区别

这两个要分开看。

### `libkrun`

`libkrun` 更偏构建期链接依赖 + runtime bundled native lib。

shim 构建脚本中明确写到：

[build-shim.sh](/home/erasernoob/project/boxlite/scripts/build/build-shim.sh#L82)

```text
krun: statically link libkrun.a
```

从 BoxLite 的设计意图看：

- shim 是 libkrun 的主要承载进程
- 上层 runtime 不直接管理 libkrun 路径
- runtime bundling 层负责把它收集进 runtime 目录

### `libkrunfw`

`libkrunfw` 是运行时 `dlopen` 的。

这点非常关键，所以 BoxLite 对它做了额外处理：

1. runtime bundle 中显式包含 `libkrunfw.*`
2. jailer 场景下，`copy_shim_to_box()` 会把 `libkrunfw` 跟 shim 一起复制到 box 的 `bin/`

见：

- [build-runtime.sh](/home/erasernoob/project/boxlite/scripts/build/build-runtime.sh#L15)
- [shim_copy.rs](/home/erasernoob/project/boxlite/boxlite/src/jailer/shim_copy.rs#L1)

BoxLite 这里的核心思路是：

- `libkrunfw` 不是让用户自己维护路径
- 而是作为 shim 运行时资产的一部分统一管理

## 9. kernel 在 BoxLite 里怎么处理

从当前 libkrun 路径的代码来看，BoxLite 并没有在主路径上显式调用 `ctx.set_kernel(...)`。

实际使用的是：

- `set_rootfs(...)`
- 或 `set_root_disk_remount(...)`

对应位置：

[engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L394)

虽然 `GuestRootfs` 结构里保留了：

- `kernel: Option<PathBuf>`
- `initrd: Option<PathBuf>`

但当前 libkrun 这条生产路径里，重点并不是让上层自己维护一个独立 kernel 文件路径。

更贴近实际的理解是：

- BoxLite 把重点放在 guest rootfs disk 构造和 guest binary 注入上
- kernel / firmware 引导更多由 libkrun / libkrunfw 这套底层模型处理

因此，如果你想复刻 BoxLite，不要先按传统 microVM 路径把精力主要花在“手工管理 kernel + initrd + `/sbin/init`”这套思路上。

## 10. BoxLite 实际封装掉了哪些细节

从上面的代码可以看出，BoxLite 把以下细节全部封在内部了。

### 1. guest image 获取

上层不关心 OCI 拉取细节，只知道有一个 bootstrap init rootfs。

### 2. rootfs 到 disk 的转换

上层不关心 layers 解包、目录 merge、ext4 生成。

### 3. guest binary 注入

上层不关心如何把 guest agent 放进 ext4。

### 4. rootfs 版本缓存

上层不关心缓存 key 如何基于 image digest + guest hash 生成。

### 5. per-box overlay 创建

上层不关心 qcow2 backing chain 如何搭起来。

### 6. runtime 资产打包

上层不关心 `boxlite-shim`、`boxlite-guest`、`libkrunfw` 在哪。

### 7. 运行时路径发现

上层不关心 runtime dir、embedded extraction、library env。

## 11. 如果你要复刻，建议复刻哪些抽象

如果你的目标不是简单“能跑 libkrun”，而是像 BoxLite 一样把 guest bootstrap 封装好，我建议复刻下面这些对象。

### `RuntimeAssetBundle`

职责：

- 管理 `shim`、`guest agent`、`libkrunfw`、辅助工具
- 统一输出一个 runtime 目录

### `BinaryFinder`

职责：

- 统一解析 runtime dir
- 隐藏二进制实际落地位置

### `ImageDiskManager`

职责：

- 把 OCI image 变成纯 ext4 基础盘
- 按 image digest 做缓存

### `GuestRootfsManager`

职责：

- 注入 guest agent
- 做版本缓存
- 给每个 box 生成/reuse COW overlay

### `KrunDriver`

职责：

- 只负责把上述产物映射到 libkrun 的 `krun_*`

不要把 rootfs 构建逻辑揉进 VMM driver。

## 12. 对你当前验证工作的建议

如果你要做“guest 是否真的能跑起来”的端到端验证，按 BoxLite 设计，应该验证这条链路：

1. bootstrap OCI image 能否拉取成功
2. pure image ext4 能否构建成功
3. `boxlite-guest` 能否注入 ext4
4. shared guest rootfs base 是否写入缓存
5. per-box guest qcow2 overlay 是否创建成功
6. shim 是否能定位到 `libkrunfw`
7. `krun_set_exec(/boxlite/bin/boxlite-guest, ...)` 后 guest 是否真正运行
8. guest 是否能在 vsock 上 bind 并向 host 发 ready

这比验证 `/sbin/init -> sandbox-agent` 更贴近 BoxLite 实际实现。

## 13. 最终判断

如果你的复刻方案满足以下条件，就已经非常接近 BoxLite 当前的 guest bootstrap 封装模型：

1. bootstrap 来源是 OCI image，而不是手工维护的最终 VM 镜像
2. OCI image 会被转换成纯 image ext4 disk
3. guest agent 是 runtime 注入进去的，不依赖镜像内 init system
4. guest rootfs 有共享 base cache
5. 每个 box 在共享 base 之上建立自己的 COW overlay
6. `boxlite-shim`、`boxlite-guest`、`libkrunfw` 通过统一 runtime bundle 管理
7. 运行时路径发现由 finder 层统一封装

如果缺少其中几项，你就还没有复刻到 BoxLite 这层“把 bootstrap 与 runtime 资产全部产品化封装”的程度。
