# macOS Process Tap PoC / helper

[English](README.md) | 简体中文

这是一个专门用于 Echoless macOS `reference="system"` 的探针与开发 helper。

Rust 实时管线可以以 `--stream-stdout` 模式启动该二进制，并将其输出的原始 Float32 PCM 作为远端参考信号消费。

## Build

```bash
./tools/macos-process-tap-poc/build.sh
```

构建脚本会把 `NSAudioCaptureUsageDescription` 嵌入命令行二进制（Apple 要求提供该用途说明字符串才能获得系统音频采集权限），并对其签名。如果存在 `Echoless Dev` 代码签名标识，就用这个稳定标识签名——从而在多次重新构建之间保持 System Audio Recording 的 TCC 授权不失效——否则回退到 ad-hoc 签名。构建结果会按指纹缓存（源码 + 签名标识），因此重新构建能保持字节级稳定。

## Run

```bash
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --seconds 10 --out /tmp/process_tap_ref.wav
```

运行期间请播放系统音频。首次使用时，macOS 应当会为该二进制或其父宿主进程请求 System Audio Recording 权限。

成功的预期迹象：

- stderr 显示回调以及不断增加的帧计数；
- 系统音频播放时，`peak` 和 `rms` 会升到零以上；
- 输出的 WAV 能连续播放出系统音频。

如果录到的是静音：

- 在 macOS 系统设置中授予 System Audio Recording 权限；
- 退出并重新运行该二进制；
- 确认系统音频确实正通过所选输出设备播放。

## Realtime stream mode

```bash
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout --mono
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout --exclude-pid 12345
```

`--stream-stdout` 会先写出一个 16 字节的小端头部（`ELTP` magic + u32 版本号 + u32 采样率 + u32 声道数），随后将原始 Float32 PCM 写到 stdout；人类可读的日志则写到 stderr。这个头部让 Rust 消费方能够按管线的采样率和声道布局做重采样与重映射。Mono 模式请求单声道；默认模式请求交错的立体声（头部会报告 tap 的实际格式）。该 helper 在收到 SIGTERM/SIGINT 时释放 tap，并在其父进程死亡时自行退出（不会残留孤立的 tap）。

Rust CLI 按以下顺序发现该 helper：

1. `ECHOLESS_PROCESS_TAP_HELPER`；
2. 位于 `echoless` 可执行文件旁边的 helper 二进制；
3. `tools/macos-process-tap-poc/.build/` 下的这个 dev 构建路径。

## Scope

该 helper 默认录制一个全局立体声 Process Tap。`--mono` 录制一个单声道的全局 tap。`--exclude-pid` 会把给定的 PID 转换为一个 Core Audio 进程对象，并将其从 tap 中排除。Rust 实时管线会传入自身的 PID，以避免 Echoless 处理后的输出被回灌到远端参考信号中。如果 Core Audio 无法转换该 PID，helper 会记录一条警告并继续运行。
