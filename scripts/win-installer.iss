; WinSSH Pro 的 Windows 安装包脚本（Inno Setup）。
;
; 由 `.github/workflows/release.yml` 在打完 release 二进制之后调起：
;   ISCC.exe /DAppVersion=<tag 去掉 v> /DArch=<Inno 的架构标识> /DArchLabel=<文件名里那段> /DSourceDir=<包目录> scripts\win-installer.iss
; 包目录里是 `winssh-pro.exe`、`winssh-broker.exe`、`wintun.dll` 三个文件，
; 与前几次发版打的 `.tar.gz` 里那份逐字节相同 —— 这里只负责摆位置、建快捷方式。
;
; 本脚本里所有相对路径都按**本脚本所在目录**（`scripts/`）解析，不是按
; 运行 ISCC 时的当前目录 —— 所以下面两个默认值都带 `..\`，CI 那边传的
; 一律是绝对路径（`cygpath -aw`）。
;
; 手工在本机试：在仓库根先 `cargo build --release`，再
;   ISCC.exe /DAppVersion=0.1.0 scripts\win-installer.iss
; 产物落在仓库根的 `dist\`（已在 .gitignore 里）。

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Arch
  #define Arch "x64compatible"
#endif
#ifndef ArchLabel
  #define ArchLabel "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

; broker 在 SCM 里登记的服务名，与 src/bin/broker.rs 的 `SERVICE_NAME` 一致。
#define BrokerService "WinSSHProBroker"

[Setup]
; 升级识别靠这个 AppId，装过一次之后**永远不许改** —— 改了会被当成另一个
; 程序，新版本装成并存的两份、卸载各卸各的。
AppId={{7C4A1B2E-8F3D-4E6A-9B52-3D0E5C7A1F84}
AppName=WinSSH Pro
AppVersion={#AppVersion}
AppVerName=WinSSH Pro {#AppVersion}
AppPublisher=WinSSH Pro Contributors
AppPublisherURL=https://github.com/lantongxue/winssh-pro
AppSupportURL=https://github.com/lantongxue/winssh-pro/issues
DefaultDirName={autopf}\WinSSH Pro
DefaultGroupName=WinSSH Pro
DisableProgramGroupPage=yes
; 架构标识由 CI 按目标传进来（见文件头）。**别写成 `x64`** —— Inno 6.3 起
; 那个写法是废弃的 `x64strict`：在 ARM64 Windows 上（能跑 x64 仿真）会被
; 直接拒绝，x64 那份安装包装不进 ARM 机器。x64 那支的正解是 `x64compatible`。
ArchitecturesAllowed={#Arch}
ArchitecturesInstallIn64BitMode={#Arch}
OutputDir={#OutputDir}
OutputBaseFilename=winssh-pro-{#ArchLabel}-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 应用正在跑的时候别硬装：Inno 经 Restart Manager 找出持着安装目录里文件的
; 进程，列给用户、征得同意后替他关掉（关窗缩托盘之后用户很容易忘了它还在
; 后台）。broker 服务不靠这一条，[Code] 里单独停。
CloseApplications=yes
; 主 exe 里已经用 winres 嵌了 assets/app-icon.ico（见 build.rs），
; 安装包和"应用和功能"里跟着走，不用在这里再指一遍。

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
; 中文这一份**不在** Inno 6 的安装里 —— 它是非官方的用户翻译，官方安装包只带
; 官方维护的那几十种。写在 `compiler:` 下（编译器自己的 Languages 目录）在 CI
; 上必然找不到文件、直接编译失败，所以整份 vendor 在仓库里，用相对本脚本的
; 路径引。来源：jrsoftware/issrc 的 `is-6_5_0` tag 下
; `Files/Languages/Unofficial/ChineseSimplified.isl`，正文写的是 6.4.0+。
; 升级 Inno 主版本时记得回来对一下这份译文，别让它落后于编译器。
Name: "chinesesimplified"; MessagesFile: "inno-languages\ChineseSimplified.isl"

[CustomMessages]
english.BrokerServiceTask=Install the WinSSH Pro broker service (needed by Clash TUN mode; avoids a UAC prompt on first use)
chinesesimplified.BrokerServiceTask=安装 WinSSH Pro 提权服务（Clash 的 TUN 模式要用；装上之后首次开启不再弹 UAC）
english.BrokerServiceGroup=System service:
chinesesimplified.BrokerServiceGroup=系统服务：

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
; 默认勾上：装机时安装程序本来就是管理员，顺手把 broker 注册成服务，用户第一次
; 拨 TUN 时 GUI 直连管道就行（clash::broker::ensure_running 先试连、连不上才走
; UAC），不再弹那一下。留成可取消的一项，给不用 Clash TUN、不想多一个常驻
; LocalSystem 服务的用户；不勾的话行为和以前一样 —— 第一次拨 TUN 时再经 UAC 装。
Name: "brokerservice"; Description: "{cm:BrokerServiceTask}"; GroupDescription: "{cm:BrokerServiceGroup}"

[Files]
Source: "{#SourceDir}\winssh-pro.exe"; DestDir: "{app}"; Flags: ignoreversion
; broker 是 Clash 那一档拨 TUN 时经 UAC 拉起的提权小助手（src/bin/broker.rs），
; 必须与主 exe 同目录 —— clash::broker::broker_binary_path 找的就是那儿。
Source: "{#SourceDir}\winssh-broker.exe"; DestDir: "{app}"; Flags: ignoreversion
; wintun.dll 由 EasyTier 的虚拟网卡那一档按名字加载，第一站就是 exe 所在目录，
; 少一份那档就报 "The file is not signed."（详见 build.rs 的 copy_wintun）。
Source: "{#SourceDir}\wintun.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\WinSSH Pro"; Filename: "{app}\winssh-pro.exe"
Name: "{group}\{cm:UninstallProgram,WinSSH Pro}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\WinSSH Pro"; Filename: "{app}\winssh-pro.exe"; Tasks: desktopicon

[Run]
; 勾了那一项：让 broker 自己注册服务。`--install-service` 已有同名服务时先停、
; 先删再重建并立刻启动，所以升级时也走这条，binPath 跟着刷成这次的安装目录。
Filename: "{app}\winssh-broker.exe"; Parameters: "--install-service"; StatusMsg: "{cm:BrokerServiceTask}"; Flags: runhidden waituntilterminated; Tasks: brokerservice
; 没勾、但升级前服务在跑（以前经 UAC 装过的）：把 PrepareToInstall 停掉的那份
; 照原样拉起来，不替用户把它卸了。升级默认沿用上次的安装目录（UsePreviousAppDir），
; 新 exe 落在同一个路径，服务登记的 binPath 仍然有效。
Filename: "{sys}\net.exe"; Parameters: "start {#BrokerService}"; Flags: runhidden waituntilterminated; Check: ShouldRestartBroker
; 自动更新用 /SILENT 跑安装包：`skipifsilent` 会把这一条也跳掉，装完就
; 没有进程了。改成"静默时也运行"，并由 `runasoriginaluser` 保证新进程以
; 当前登录用户身份起 —— 安装包自己在提权，不加这个的话新应用会以管理员
; 身份跑起来，配置目录也跟着变了。
Filename: "{app}\winssh-pro.exe"; Description: "{cm:LaunchProgram,WinSSH Pro}"; Flags: nowait postinstall runasoriginaluser

; 不写 [UninstallRun]：卸载前那几步（结束程序、卸 broker 服务、还原系统代理）
; 有严格的先后，统一放在 [Code] 的 CurUninstallStepChanged 里按顺序跑。
;
; 不写 [UninstallDelete]：卸载只删我们装进去的那几个文件，空了的目录 Inno 自己收。
; 别图省事写 `filesandordirs {app}` —— 用户要是装进了 `D:\Tools` 这种本来就放着
; 别的东西的目录，那一条会把整个目录连带别人的文件一起删掉。
; 用户数据（%APPDATA%\winssh-pro 下的配置、数据库、订阅页下载的 mihomo 内核）
; 也一律不动：卸载不删库，重装回来主机和凭据都还在。

[Code]
var
  BrokerRunning: Boolean;

// 卸载前的三步，**顺序不能换**，都在用户点头之后、删文件之前
// （CurUninstallStepChanged 的 usUninstall）。每一步失败都不拦卸载 ——
// 退出码不看，能做多少做多少。
//
// 1. 结束还在跑的 GUI。关窗缩到托盘之后用户早就"关掉"了这个程序，可进程还在，
//    exe 占着删不掉。只能强杀：不带 /F 的 taskkill 只是发 WM_CLOSE，而这程序
//    收下关闭请求只会缩回托盘；Inno 的 Restart Manager 那一套对"不理会关闭
//    请求"的程序同样使不上劲。`/T` 连子进程一起收 —— 非提权模式下 mihomo 是
//    GUI 的子进程，漏了它就留下一个占着端口的孤儿内核。
//
// 2. 卸 broker 服务。服务是常驻的 LocalSystem、开机自启，只卸文件不卸它，就留下
//    一个开机就起、指向已删 exe 的服务。`--uninstall-service` 同步地先停再删，
//    服务从没装过也返回成功。它停下的那一刻，broker 的 job object 连带收走提权
//    模式的 mihomo —— 所以必须排在第 3 步之前。
//
// 3. 还原系统代理。正常退出时应用自己会还原（src/main.rs 的 cx.on_release），
//    强杀跳过了这一步；平时有下次启动的 clash::system_proxy::reconcile 兜底，
//    可卸完就没有下次了 —— 注册表里的代理指着死掉的口，用户卸完就断网。所以
//    拿 `--restore-system-proxy` 再起一次主程序，只跑 reconcile 那一段。
//    reconcile 看到代理口还有人在听就会"留给它"，所以前两步得先把 mihomo 全
//    收干净，再稍等一下让端口真正释放。
//
//    限制：ExecAsOriginalUser 在卸载时不可用，这一步以卸载器的身份（管理员）跑。
//    最常见的 UAC 场景（用户本人就是管理员、点"是"）下提权后还是同一个账户，
//    HKCU 与 %APPDATA% 都对；要是标准用户输了别人的管理员密码来卸载，这一步
//    找不到快照，什么也不做。
procedure PrepareUninstall;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM winssh-pro.exe /T /F',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{app}\winssh-broker.exe'), '--uninstall-service',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(1000);
  Exec(ExpandConstant('{app}\winssh-pro.exe'), '--restore-system-proxy',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    PrepareUninstall;
end;

// 升级时 broker 服务正跑着、持着安装目录里的 winssh-broker.exe，不停掉的话
// 那个文件覆盖不了。`net stop` 是同步的（等到 STOPPED 才回），只有服务原本在跑、
// 并且被它停下来时才返回 0 —— 正好拿来记"装完要不要重新拉起"。
// 服务不存在（从没拨过 TUN，或者是全新安装）时它返回非零，什么也不发生。
// （这里不用 Pascal 的花括号注释：它不能嵌套，正文里一出现 Inno 常量就提前收口。）
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  BrokerRunning := Exec(ExpandConstant('{sys}\net.exe'), 'stop {#BrokerService}', '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
  Result := '';
end;

// 勾了 brokerservice 那一项时 `--install-service` 自己会重建并启动，这里不再插手。
function ShouldRestartBroker: Boolean;
begin
  Result := BrokerRunning and not WizardIsTaskSelected('brokerservice');
end;
