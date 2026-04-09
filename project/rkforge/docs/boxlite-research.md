# BoxLite 到 libkrun 调用链路与复刻实施报告

## 目的

本文档基于 BoxLite 当前代码实现，梳理：

1. `boxlite` 到 `libkrun` 的完整调用链路
2. host / shim / guest 三段之间的职责边界
3. 如果要在另一个容器运行时中复刻该方案，需要实现哪些模块和能力

该文档面向“对照现有实现逐项验证”和“指导继续开发”两个目标。

## 一句话结论

BoxLite 不是由宿主 runtime 直接调用 `libkrun`。它的实际路径是：

`BoxliteRuntime/LiteBox`
-> 组装 `InstanceSpec`
-> 启动 `boxlite-shim` 子进程
-> shim 中创建 `KrunContext`
-> 由 `KrunContext` 调用 `krun_*` FFI
-> `krun_start_enter()` 接管 shim 进程进入 VM

这意味着：

- 你的主容器运行时不应直接调用 `krun_start_enter()`
- 必须引入独立 shim 子进程
- 高层 runtime 只应感知 host 侧 Unix socket
- guest 侧的 vsock 细节应封装在 libkrun driver 内部

## 核心链路总览

完整主路径：

`BoxliteRuntime/LiteBox`
-> `VmmSpawnTask::build_config`
-> `ShimController::start`
-> `ShimSpawner::spawn`
-> `boxlite-shim main`
-> `vmm::create_engine(VmmKind::Libkrun)`
-> `Krun::create`
-> `KrunContext::{create,set_vm_config,add_net_path,add_virtiofs,add_disk_with_format,set_root*,set_exec,add_vsock_port,set_console_output}`
-> `instance.enter()`
-> `KrunContext::start_enter()`
-> `krun_start_enter(ctx_id)`

## 关键源码入口

- Host 侧构造 `InstanceSpec`：
  [vmm_spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/vmm_spawn.rs#L128)
- Host 侧序列化配置并启动 shim：
  [shim.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/controller/shim.rs#L260)
- 实际启动 `boxlite-shim --engine Libkrun --config ...`：
  [spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/controller/spawn.rs#L74)
- shim 入口：
  [main.rs](/home/erasernoob/project/boxlite/boxlite/src/bin/shim/main.rs#L83)
- libkrun engine：
  [engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L194)
- libkrun FFI 包装层：
  [context.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/context.rs#L13)

## 按时序拆解

### 1. Host 侧准备 `InstanceSpec`

BoxLite 在
[vmm_spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/vmm_spawn.rs#L128)
中完成 VM 启动前的配置组装。

这里会先创建两条 host 侧 Unix socket：

- `transport = unix(layout.socket_path())`
- `ready_transport = unix(layout.ready_socket_path())`

随后组装：

- `fs_shares`
- `block_devices`
- `guest_rootfs`
- `guest_entrypoint`
- `network_config`
- `cpus / memory_mib`
- `console_output / exit_file`

### 2. Guest entrypoint 初始仍然是 Unix URI

在
[vmm_spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/vmm_spawn.rs#L264)
中，guest 启动参数最开始长这样：

```text
--listen unix:///.../box.sock
--notify unix:///.../ready.sock
```

也就是说，在高层 runtime 看来，host-guest 通道先被抽象成 host 侧 Unix socket。

### 3. Host 启动 `boxlite-shim`

`ShimController::start()` 会把 `InstanceSpec` 序列化成 JSON，然后通过 `ShimSpawner`
启动子进程：

```text
boxlite-shim --engine Libkrun --config <json>
```

对应代码：

- [shim.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/controller/shim.rs#L260)
- [spawn.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/controller/spawn.rs#L114)

这一步不是实现细节，而是结构性要求。

原因是 `libkrun` 的 `krun_start_enter()` 会 takeover 当前进程，因此不能在主 runtime 进程里直接调用。

### 4. shim 进程中创建 libkrun engine

`boxlite-shim` 启动后，在
[main.rs](/home/erasernoob/project/boxlite/boxlite/src/bin/shim/main.rs#L136)
中完成：

1. 解析 CLI
2. 反序列化 `InstanceSpec`
3. 如有网络需求则先创建 `gvproxy`
4. `vmm::create_engine(args.engine, options)?`
5. `engine.create(config)?`

### 5. `Krun::create()` 把 `InstanceSpec` 映射为 `krun_*`

这里是 BoxLite 到 libkrun 的核心映射层，位于：

[engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L194)

顺序基本如下：

1. `krun_init_log`
2. `krun_create_ctx`
3. `krun_set_vm_config`
4. 配置网络
5. `krun_set_rlimits`
6. 为每个 share 调 `krun_add_virtiofs`
7. 为每个磁盘调 `krun_add_disk2`
8. 设置 rootfs：
   - virtiofs root 用 `krun_set_root`
   - disk root 用 `krun_set_root_disk_remount`
9. `krun_set_workdir`
10. `krun_set_exec`
11. 两条 `krun_add_vsock_port2`
12. 可选 `krun_set_console_output`

### 6. guest 参数在 engine 层从 Unix 改写为 vsock

这里是一个必须理解的分层点。

BoxLite 不是在 host 高层代码里直接把 guest 参数写成 `vsock://...`。真正的转换在：

[engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L156)

它会把：

```text
--listen unix:///...
--notify unix:///...
```

改成：

```text
--listen vsock://2695
--notify vsock://2696
```

对应固定端口定义在：

[constants.rs](/home/erasernoob/project/boxlite/boxlite-shared/src/constants.rs#L16)

具体是：

- `GUEST_AGENT_PORT = 2695`
- `GUEST_READY_PORT = 2696`

### 7. 真实的 libkrun FFI 调用

FFI 方法封装在：

[context.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/context.rs#L13)

你在复刻时需要重点关注这些调用：

- `krun_create_ctx`
- `krun_set_vm_config`
- `krun_set_root`
- `krun_set_root_disk_remount`
- `krun_set_exec`
- `krun_set_rlimits`
- `krun_add_net_unixstream`
- `krun_add_net_unixgram`
- `krun_add_virtiofs`
- `krun_add_vsock_port2`
- `krun_add_disk2`
- `krun_set_console_output`
- `krun_start_enter`

### 8. 进入 VM

在
[engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L13)
中，`instance.enter()` 最终会调用：

- `KrunContext::start_enter()`
- `krun_start_enter(ctx_id)`

一旦成功，shim 进程就被 libkrun takeover。

## gRPC 与 ready 通道是如何打通的

BoxLite 维护两条独立通道：

1. gRPC 主控制通道
2. ready 通知通道

### gRPC 主通道

host 侧流程：

- host 只知道 Unix socket
- libkrun 把 host Unix socket bridge 到 guest vsock `2695`
- guest agent 在 `vsock://2695` 上绑定 tonic server
- host 再通过 Unix socket 建立 tonic channel

对应位置：

- host 配置 bridge：
  [engine.rs](/home/erasernoob/project/boxlite/boxlite/src/vmm/krun/engine.rs#L421)
- guest bind vsock server：
  [server.rs](/home/erasernoob/project/boxlite/guest/src/service/server.rs#L88)
- host 用 Unix socket 建 tonic channel：
  [connection.rs](/home/erasernoob/project/boxlite/boxlite/src/portal/connection.rs#L43)

### ready 通道

ready 通道使用单独端口 `2696`。

host 侧：

- 先监听一个 Unix socket
- 等待 guest 连回来

guest 侧：

- guest server ready 后主动连接 `vsock://2696`
- libkrun 再桥接回 host 的 Unix socket

对应位置：

- host 等待 ready：
  [guest_connect.rs](/home/erasernoob/project/boxlite/boxlite/src/litebox/init/tasks/guest_connect.rs#L99)
- guest 发 ready：
  [server.rs](/home/erasernoob/project/boxlite/guest/src/service/server.rs#L203)

## 设计边界总结

从 BoxLite 的实现看，职责边界是清晰分层的。

### Host runtime 负责

- 组装 `InstanceSpec`
- 准备 host 侧 socket / 文件 / 目录
- 启动 shim 子进程
- 通过 Unix socket 与 guest agent 通信

### shim 负责

- 创建 libkrun context
- 将 `InstanceSpec` 映射成 `krun_*`
- 创建网络 backend
- 应用 seccomp / watchdog / crash capture
- 调用 `krun_start_enter()`

### guest agent 负责

- 在 VM 内绑定 vsock server
- guest 初始化
- 容器 runtime 生命周期
- ready 通知
- Shutdown / Exec / Container RPC

## 如果你要复刻，最低可行实现应该做什么

我建议先做“最小可用链路”，不要一上来复刻 BoxLite 全部功能。

### 你至少需要实现的模块

#### 1. `VmSpec`

这是你自己的虚拟机配置对象，建议字段尽量贴近 BoxLite 的 `InstanceSpec`：

- CPU / 内存
- rootfs
- block devices
- virtiofs shares
- guest entrypoint
- host transport
- ready transport
- 日志 / console / exit_file

#### 2. `ShimProcess`

单独子进程，用于：

- 读取配置
- 创建 libkrun context
- 调用 `krun_start_enter()`

这是必须项，不建议省略。

#### 3. `KrunDriver`

一层纯粹的 libkrun 映射层。

职责应当只是：

- `VmSpec -> krun_*`

不要在这里掺入容器镜像逻辑、业务逻辑、用户 API。

#### 4. `TransportBridge`

推荐复刻 BoxLite 这一层抽象：

- host 用 Unix socket
- guest 用 vsock
- 中间由 libkrun `krun_add_vsock_port2` 桥接

#### 5. `GuestAgent`

如果你的目标是“容器运行时接入 libkrun”，而不是“在 VM 里跑一个固定二进制”，那 guest agent 基本是必须的。

至少应该提供：

- `Ping`
- `Init`
- `Shutdown`

之后再扩展：

- `Container.Init`
- `Exec`
- `Attach`

#### 6. `HostSession`

host 不要直接实现 vsock client。

BoxLite 这里的做法更稳：

- host 只通过 Unix socket 建立 gRPC 连接
- vsock 只存在于 libkrun bridge 和 guest 内部

## 最小 FFI 调用集合

如果你只想先把 libkrun 接起来，第一阶段真正必须跑通的 FFI 很少：

- `krun_create_ctx`
- `krun_set_vm_config`
- `krun_add_virtiofs`
- `krun_add_disk2` 或 `krun_set_root`
- `krun_set_exec`
- `krun_add_vsock_port2`
- `krun_start_enter`

如果这几个没通，后面的容器 runtime 集成都没有意义。

## 如果要复刻 BoxLite 的容器工作流，还要补哪些能力

仅仅把 VM 拉起来不够。BoxLite 实际上还做了 guest 内容器执行链路。

你还要补这些模块：

### 1. 镜像到 rootfs / 磁盘的转换

至少包括：

- OCI image pull
- layer 解包和 merge
- rootfs 目录准备
- 或 ext4 / qcow2 root disk 构建

### 2. guest 内接入 OCI runtime

BoxLite 的 guest 里接了 `libcontainer`。

你的 runtime 如果要“复刻 boxlite”，而不是仅仅“跑一个 VM guest 程序”，就需要在 guest 里负责：

- 创建容器 rootfs
- 配置 namespace / cgroup
- 启动容器进程
- 管理 exec / attach / wait

### 3. volume 模型

你需要明确：

- 哪些目录走 virtiofs
- 哪些数据盘走 block device
- guest 中如何把 share / disk 映射到容器挂载点

### 4. guest 初始化协议

至少要定义清楚 guest 在收到 `Init` 后做什么：

- mount virtiofs
- mount root disk
- 初始化容器工作目录
- 初始化网络
- 准备 OCI runtime 所需目录

### 5. graceful shutdown

不能直接杀掉 shim，否则容易留下磁盘写回问题。

建议复刻 BoxLite 的顺序：

1. host 发 `Shutdown` RPC
2. guest flush / sync
3. shim 再退出

## BoxLite 的增强项，建议后做

这些很重要，但不是你第一阶段“接入 libkrun”的必要条件。

### 1. jailer / sandbox

- Linux seccomp
- Linux namespace
- macOS seatbelt
- rlimit / cgroup

### 2. network backend

BoxLite 的网络不是简单开个网卡，而是有独立 backend：

- `gvproxy`
- macOS 用 UnixDgram + VFKit protocol
- Linux 用 UnixStream

还有一个 BoxLite 特有处理：

- 当禁网时，用 dead socket trick 防止 libkrun 自动启用 TSI

### 3. watchdog

父进程退出时，shim 自动感知并发起 graceful shutdown。

### 4. crash 诊断

包括：

- `stderr` 文件
- `console_output`
- `exit_file`

### 5. 打包与动态库分发

尤其是：

- `libkrun`
- `libkrunfw`
- `LD_LIBRARY_PATH`
- `TMPDIR`

这是落地时常见坑，不建议低估。

### 6. `RLIMIT_NOFILE`

BoxLite 明确在 krun 配置前把 `RLIMIT_NOFILE` 调高，因为 virtiofs 很依赖。

## 对复刻方案的设计建议

这里直接给结论。

### 建议 1：不要让主 runtime 直接调用 `krun_start_enter()`

必须放进 shim 子进程。

### 建议 2：不要在高层 API 泄漏 vsock 细节

高层只应看到：

- Unix socket
- guest session

vsock 端口和转换逻辑应放进 `KrunDriver`。

### 建议 3：不要让容器 runtime 直接知道 libkrun bridge 端口

容器 runtime 只应该知道 guest agent 协议，不应该直接依赖 `2695/2696` 这类实现细节。

### 建议 4：不要先复刻完整安全层

第一阶段请聚焦主干：

- shim
- libkrun
- guest agent
- Unix/vsock bridge
- guest 内容器 runtime

### 建议 5：不要跳过 guest agent

如果你要复刻的是 BoxLite 的容器模型，guest agent 不是可选件。

没有它，你只能做到“起一个 VM 并跑一个程序”，而不是“host 上的容器运行时控制 guest 内容器生命周期”。

## 推荐实施顺序

建议按下面顺序推进。

1. 定义你自己的 `VmSpec`
2. 实现 `shim` 子进程和 `--config <json>`
3. 实现 `KrunDriver::create(spec)`
4. 跑通单个 virtiofs rootfs + 单个 guest agent
5. 跑通 `host Unix socket <-> guest vsock` 的 gRPC 主通道
6. 跑通 ready 通道
7. 接入 guest 内 OCI runtime
8. 再补 network backend、seccomp、watchdog、诊断链路

## 验证清单

你可以按下面顺序逐项验证当前实现。

### A. shim 架构

- 是否使用独立 shim 子进程执行 libkrun
- 主 runtime 是否避免直接调用 `krun_start_enter()`
- shim 是否能接收完整 VM 配置对象

### B. libkrun driver

- 是否存在单独映射层把 `VmSpec` 转换为 `krun_*`
- 是否按正确顺序完成：
  - create ctx
  - set vm config
  - set root / add disk
  - add virtiofs
  - set exec
  - add vsock ports
  - start enter

### C. 通信链路

- host 是否只连接 Unix socket
- guest 是否只 bind vsock
- 是否存在 gRPC 主通道和 ready 通道两条独立链路
- ready 是否通过“建立连接本身”作为信号

### D. guest agent

- guest 是否有最小 agent 进程
- agent 是否在 VM 内提供 Init / Ping / Shutdown
- agent 是否为未来容器 runtime 管理预留协议边界

### E. rootfs / 容器层

- rootfs 是目录型、disk 型，还是两者都支持
- OCI image 是否被转换为 guest 可用 rootfs
- 容器 runtime 是否实际运行在 guest 内

### F. 稳定性增强

- graceful shutdown 是否完成
- 崩溃诊断文件是否具备
- 动态库分发是否稳定
- Linux/macOS 的网络 backend 是否正确区分

## 最后的判断标准

如果你的当前实现满足以下条件，就说明你已经复刻了 BoxLite 与 libkrun 集成的“主干”：

1. 主 runtime 不直接碰 `krun_start_enter()`
2. 通过 shim 子进程驱动 libkrun
3. host 用 Unix socket，guest 用 vsock
4. 存在独立 guest agent
5. guest agent 负责 guest 内容器或进程控制
6. host 通过 agent 协议而不是直接通过 libkrun 控制 guest 工作负载

如果缺任一项，就还没有真正复刻到 BoxLite 这条架构线上，最多只是“用 libkrun 启动了一个 VM”。
