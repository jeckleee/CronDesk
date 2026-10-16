# CronDesk

[![构建与发布](https://github.com/jeckleee/CronDesk/actions/workflows/build.yml/badge.svg)](https://github.com/jeckleee/CronDesk/actions/workflows/build.yml)

CronDesk 是一个使用 **Rust 编写的轻量桌面工具**，在 Windows 和 macOS 上管理定时任务与持续运行的服务。采用原生界面，不依赖 Electron，任务配置和执行日志保存在本地 SQLite 数据库中。

**[下载最新版本](https://github.com/jeckleee/CronDesk/releases/latest)** · [全部版本](https://github.com/jeckleee/CronDesk/releases) · [构建记录](https://github.com/jeckleee/CronDesk/actions) · [反馈问题](https://github.com/jeckleee/CronDesk/issues)

## 功能

| 定时任务 | 服务管理 |
| --- | --- |
| 支持标准五段 crontab 规则 | 托管 `node aa.js` 等持续运行的命令 |
| 设置任务名称、命令和执行目录 | 设置服务名称、命令和执行目录 |
| 自动调度、手动执行、暂停、停止 | 启动、停止、重启 |
| 查看下次执行时间和成功、失败统计 | 查看状态、PID 和运行时长 |
| 保存每次执行记录及标准输出、错误输出 | 保存每次启动记录及持续输出 |
| 同一任务不会同时执行多个实例 | 可选随 CronDesk 启动、异常退出后有限次数重试 |

两类日志均支持单独清空或按类别全部清空。删除任务或服务时，其历史记录和输出日志也会一起删除。

## 安装与升级

在 [Releases](https://github.com/jeckleee/CronDesk/releases) 页面选择安装包：

- **Windows 11 x64**：下载 `CronDesk-<版本>-windows-x64-setup.exe`，双击安装。
- **macOS**：下载 `CronDesk-<版本>-macos-universal.dmg`，打开后将 `CronDesk.app` 拖入 `Applications`。同一个安装包支持 Apple Silicon 和 Intel。

Windows 默认安装到当前用户目录，无需管理员权限。软件自身不需要安装 Rust；命令依赖的 Node.js、Python、uv 或 PowerShell 7 等运行环境需要自行安装，并确保可通过 PATH 或绝对路径访问。

当前安装包尚未使用商业代码签名证书，macOS 应用未经过 Apple 公证，系统可能显示来源验证提示。

### 升级已有版本

1. 从系统托盘选择退出 CronDesk。仅关闭窗口会继续在后台运行。
2. Windows 直接运行新版安装程序；macOS 用新版应用替换原应用。
3. 启动新版，继续使用已有任务和服务。

升级不会覆盖用户数据库。从仅包含定时任务的 0.1.0 版本首次升级时，会先创建 `crondesk.before-0.2.0.sqlite` 备份，再增加服务相关数据表。0.2.x 之间升级继续使用原数据库。

## 定时任务

1. 打开顶部的「定时任务」选项卡，点击「新建任务」。
2. 填写任务名称、Cron 规则和脚本命令。
3. 选择执行目录；留空时使用界面显示的当前用户主目录。
4. 启用自动调度并保存，也可以随时点击「执行」手动运行。

Cron 规则由五段组成：

```text
分 时 日 月 周
```

| 示例 | 含义 |
| --- | --- |
| `*/5 * * * *` | 每 5 分钟执行 |
| `0 9 * * *` | 每天 09:00 执行 |
| `0 9 * * 1-5` | 周一至周五 09:00 执行 |
| `0 4 1 * *` | 每月 1 日 04:00 执行 |

使用电脑本地时区。睡眠、关机或退出 CronDesk 期间错过的计划不会补跑。日期字段中的步长按日历字段计算，例如 `*/3` 不代表连续间隔 72 小时。

Windows 默认通过 `cmd.exe` 执行，macOS 默认通过 `/bin/sh` 执行。命令使用当前用户权限和进程环境。

### PowerShell 7 的 profile 与环境变量

如果环境变量在 PowerShell 7 的 profile 中设置，默认的 `cmd.exe` 不会加载它。定时任务可显式调用 PowerShell 7，例如：

```powershell
pwsh.exe -NoLogo -NonInteractive -Command "uv run python -m my_package.my_job"
```

需要加载 profile 时不要加 `-NoProfile`。profile 中应使用 `$env:变量名` 设置要传给子进程的环境变量，普通 PowerShell 变量不会自动传给 Python 或 Node.js。profile 也应避免需要用户输入的交互操作。

服务编辑窗口可直接选择「PowerShell 7」，并勾选「加载 PowerShell profile」。不要把 API Key 等凭据写进任务命令、日志或提交到仓库。

## 服务

切换到「服务」选项卡，创建服务，例如：

```text
服务名称：Node API
启动命令：node aa.js
服务执行目录：aa.js 所在的项目文件夹
```

保存后点击「启动」。需要修改配置时先停止服务，修改后重新启动。

- 使用前台命令，由 CronDesk 管理其进程。不要在命令中自行转入后台。
- 「运行中」表示进程仍然存活，不代表应用的 HTTP 接口等业务功能已通过健康检查。
- 可勾选「打开 CronDesk 时自动启动此服务」。此选项不等同于操作系统开机自启。
- 异常重试可配置间隔和次数上限；正常退出不重试，手动停止会取消重试。手动启动或重启会重置重试计数。
- 停止时先尝试正常终止，5 秒后仍未结束则强制终止进程树。

服务由 CronDesk 应用托管，未注册为 Windows 系统服务或 macOS launchd 服务。关闭窗口后在托盘中继续运行，彻底退出 CronDesk 时停止。

## 日志与清理

- 列表中点击某个任务或服务的「日志」，查看该对象的记录。
- 页面右侧的总日志入口展示对应类别的所有记录。
- 点击「清空日志」并确认后，删除当前范围内的所有执行记录和输出，保留任务或服务配置。
- 删除任务或服务时，也会同步删除其关联日志；删除与清空操作无法在应用内撤销。
- 清理范围内仍有运行中的任务、服务或等待重试的服务时，需要先停止再清理。
- 定时任务的成功、失败统计依据现存记录计算，清理后同步更新。以后执行仍会生成新日志。

定时任务每次最多保存约 2 MiB 输出。服务每个保留最近约 100 MiB 输出，日志查看器显示选中运行记录最近的 2 MiB。旧版已经删除对象但留下的日志，可通过对应类别的总日志入口统一清空。

## 数据位置

| 平台 | 数据库路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\CronDesk\crondesk.sqlite` |
| macOS | `~/Library/Application Support/CronDesk/crondesk.sqlite` |

安装目录与数据目录分开。备份时请先彻底退出 CronDesk，再复制数据目录。数据库可能含有命令参数及日志中的敏感信息，不应提交到 GitHub。

## 本地开发与构建

技术栈：Rust、egui/eframe、rusqlite/SQLite、croner。

安装当前稳定版 Rust 和对应平台的编译工具。Windows 使用 MSVC 工具链与 C++ Build Tools，macOS 使用 Xcode Command Line Tools。

```bash
git clone https://github.com/jeckleee/CronDesk.git
cd CronDesk
cargo run --locked
```

检查代码：

```bash
cargo fmt --all -- --check
cargo clippy --locked --bin crondesk -- -D warnings
```

### Windows 安装包

先安装 NSIS，然后在 PowerShell 中执行：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --locked --target x86_64-pc-windows-msvc
$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
New-Item -ItemType Directory -Force dist | Out-Null
$binary = (Resolve-Path target\x86_64-pc-windows-msvc\release\crondesk.exe).Path
$output = Join-Path (Get-Location).Path "dist\CronDesk-$version-windows-x64-setup.exe"
& "${env:ProgramFiles(x86)}\NSIS\makensis.exe" /INPUTCHARSET UTF8 "/DVERSION=$version" "/DAPP_EXE=$binary" "/DOUTPUT_FILE=$output" installer/windows.nsi
```

### macOS 通用安装包

在 macOS 上执行：

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=11.0
export CARGO_TARGET_DIR="$PWD/target"
cargo build --release --locked --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin
bash installer/macos.sh
```

打包脚本合并两个架构、生成应用包、进行临时签名，并生成与校验 `.dmg`。产物位于 `dist/`。

## GitHub Actions 与自动发布

工作流文件：[`.github/workflows/build.yml`](.github/workflows/build.yml)。

每次推送代码，或在 Actions 页面手动运行，都会：

1. 在 Windows 和 macOS 构建节点上分别编译、打包。
2. 将安装包上传为 Actions Artifacts，保留 30 天。
3. 等待两个平台均成功后，自动创建 GitHub Release，并附上 `.exe` 和 `.dmg`。

基础版本读取自 `Cargo.toml`。Release 标签使用 `v<基础版本>-build.<构建序号>`，例如 `v0.2.1-build.3`，避免同一版本多次推送冲突。重跑同一次构建会更新对应 Release 的附件。构建失败时不会发布不完整的新版本。

发布使用 GitHub 自动提供的 `GITHUB_TOKEN`，无需额外设置个人访问令牌。只有发布任务申请仓库内容写权限。
