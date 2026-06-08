# vcam-windows-rs

# Caution
## It is now under its early development stage.
## So, use at your own risk!

# Intro
这是一个基于 Rust 的 Windows 虚拟摄像头原型。当前实现使用 `windows-rs` 调用 Media Foundation / Win32 API，并通过自定义 COM 媒体源输出静态测试图，或通过 broker COM 控制面 + 共享内存数据面输出动态 BGRA 帧。

当前目标仍然是原型验证，不是可发布的生产级虚拟摄像头。

## 当前状态

- `cargo build --workspace` 通过。
- `vcamctl dump-frame` 可以在本地导出静态测试图 BMP。
- `vcamctl dump-com-frame` 和 `scripts/test-com-frame.ps1` 已验证可以直接通过 COM server 拉帧；feed inactive 时校验静态图内容，feed active 时校验动态样本尺寸与时间戳。
- `vcamfeed-demo stream-animated` 已验证可以通过 broker COM 建会话，并通过共享内存持续写入变化中的 BGRA 帧。
- 使用机器级 COM 注册后，Windows 已经可以枚举到该虚拟摄像头设备。
- 当前仍在继续排查部分应用中的预览黑屏问题；最近一轮修复了自定义 stream/source 事件在系统 `MFCreateMediaEvent` / `QueueEventParam*` 路径上的崩溃，并把本地调试取帧路径切回稳定的内存 sample 分配。

## 环境要求

- Windows 11 22000 或更高版本。
- 建议使用 PowerShell 7，也就是 `pwsh.exe`。
- 安装、注册、卸载、移除虚拟摄像头时请使用管理员权限。

## 构建

```powershell
cargo build --workspace
```

## 快速安装

仓库提供了统一管理脚本 [`scripts/manage-vcam.ps1`](./scripts/manage-vcam.ps1)。建议优先使用这个脚本，而不是手动逐步注册。

安装、注册并创建虚拟摄像头：

```powershell
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Install
```

卸载、移除并注销虚拟摄像头：

```powershell
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Uninstall
```

如果你只想执行其中某一步，也可以单独调用：

```powershell
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Build
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Register
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Create
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Remove
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\manage-vcam.ps1 -Action Unregister
```

常用参数：

- `-Configuration Debug|Release`：选择构建配置，默认是 `Debug`。
- `-SkipBuild`：跳过构建，直接使用现有产物。
- `-WhatIf`：仅打印步骤，不真正修改系统。

## 安装脚本行为说明

- `Install` 会先构建项目，再把 DLL 复制到 `C:\ProgramData\vcam-windows-rs\<debug|release>\vcam_server.dll`。
- 注册使用的是机器级 COM，而不是当前用户级 `HKCU` 注册。
- 创建虚拟摄像头时使用的是 `System` 生命周期，因此设备不会随着 `vcamctl create-camera` 进程退出而立刻消失。
- 重新编译后，如果安装路径和配置未变，通常重新执行 `Register` 和 `Create` 即可。
- 如果你先前用 `Release` 安装，卸载时也应使用同样的 `-Configuration Release`。
- `Uninstall` 现在会先尝试停止 `FrameServer` 和 `FrameServerMonitor`，减少 DLL 被系统占用导致删除失败的概率。

## CLI 命令

导出静态测试图：

```powershell
cargo run -p vcamctl -- dump-frame target\static-test-pattern.bmp
```

直接通过 COM server 拉一帧并导出为 BMP：

```powershell
cargo run -p vcamctl -- dump-com-frame target\com-rgb32.bmp --subtype rgb32
```

也可以一次性验证两种格式：

```powershell
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-com-frame.ps1
```

启动共享内存 animated feed，并让虚拟摄像头输出连续变化画面：

```powershell
cargo run -p vcamfeed-demo -- stream-animated --width 640 --height 480 --fps 10 --duration-seconds 10 --force-reset
```

探测当前系统是否支持创建虚拟摄像头对象：

```powershell
cargo run -p vcamctl -- probe-create
```

手动创建虚拟摄像头：

```powershell
cargo run -p vcamctl -- create-camera
```

手动移除虚拟摄像头：

```powershell
cargo run -p vcamctl -- remove-camera
```

如果你确实需要手动注册 COM：

- 开发调试时支持 `register-com` / `unregister-com`。
- 当前推荐使用脚本完成机器级注册，因为虚拟摄像头路径在仅 `HKCU` 注册时可能无法正常启动。

## 当前原型能力

- 暴露 `cdylib` COM 服务器和 Media Foundation 自定义媒体源。
- 新增 `IVcamFrameBroker` COM 类，负责 feed 会话启动、停止、状态查询和重置。
- 实现 `IMFActivate`、`IMFMediaSourceEx`、`IMFMediaStream2`、`IKsControl`、`IMFSampleAllocatorControl`。
- 程序内生成固定测试图，不依赖运行时文件 IO。
- 支持 broker COM 控制面 + 共享内存数据面的动态 BGRA 灌帧。
- 同时声明 `RGB32` 和 `NV12` 媒体类型。
- 提供 COM 注册、虚拟摄像头创建/移除、测试帧导出、直接 COM 拉帧校验的辅助 CLI，以及 animated feeder demo。

## 已知限制

- 当前最稳定的安装路径是管理员 PowerShell 7 + 机器级 COM 注册。
- 目前已经验证“本地导出测试图”、“直接通过 COM server 拉出并校验首帧”以及“设备可被系统枚举”，但“所有相机应用都能正常显示画面”仍未完全确认。
- 当前用户级 `HKCU` COM 注册不足以支撑这条虚拟摄像头激活路径。
- `Uninstall` 会尝试先停止 `FrameServer` / `FrameServerMonitor`，但卸载前仍建议先关闭正在占用该虚拟摄像头的应用，否则仍可能出现移除失败或 DLL 文件被占用。
- `FrameServer`、系统隐私设置、Windows 版本和具体调用方都可能影响最终预览结果。
- 链接阶段会出现 `LNK4104` 警告，但目前不影响构建和原型调试。
