use std::path::{Path, PathBuf};

use crate::installer::step::Reporter;
use crate::meta;
use crate::util::reboot;

/// 清理选项
#[derive(Debug, Clone)]
pub struct CleanupOptions {
    /// 安装目录
    pub install_dir: PathBuf,
    /// 是否清除用户配置数据（%APPDATA%\AppID）
    pub clean_roaming: bool,
    /// 是否清除本地缓存 —— 即清单 `[localdata].cache_dirs` 声明的那几项。
    /// 清单没声明 `[localdata]` 时本开关无事可做（向导也不会显示这个勾选）。
    pub clean_local_cache: bool,
    /// 清除用户配置数据前是否先备份到桌面（仅在 clean_roaming 时生效）
    pub backup_to_desktop: bool,
    /// 静默模式下的保留用户数据标志
    pub keep_user_data: bool,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self {
            // ⚠️ 不要改回「拼 %ProgramFiles%」：那个硬猜曾让静默卸载删错地方，
            // 详见 `resolve_install_dir` 的注释。
            install_dir: resolve_install_dir(),
            clean_roaming: false,
            clean_local_cache: true,
            backup_to_desktop: true,
            keep_user_data: false,
        }
    }
}

/// 解析本机**实际的**安装目录。
///
/// ── 这里出过什么事 ──────────────────────────────────────────────────────
/// 从前有两个真相来源，而且只有 GUI 那条用对了：向导读注册表 ARP 的
/// `InstallLocation`（安装时亲手写下的那个目录），静默路径却走
/// `CleanupOptions::default()` —— 那是**硬猜** `%ProgramFiles%\<app.id>`。
///
/// 装在非默认目录时（向导允许改、`--dir` 能改、清单的 `paths.install` 模板本身
/// 就是每个应用可配的），静默卸载的结局是最糟的那种半成品：`DeleteInstallFiles`
/// 对着一个不存在的路径跑，而里面每一步都是 `let _ =` / `if exists()`，**一条
/// warning 都不产生**；与此同时 `UndoReceipt` 用的是回执里的绝对路径、
/// `RemoveOwnUninstallInfo` 用的是注册表键，**照常清得干干净净**。
/// 于是 ARP 条目没了（用户在「应用和功能」里再也找不到它、无法重试卸载），
/// 程序文件一个没删，`success == true`，退出码 0 —— winget 记成一次干净的卸载。
///
/// ── 三级来源 ────────────────────────────────────────────────────────────
/// 1. **ARP 的 `InstallLocation`**：安装时亲手写下的，最权威。
/// 2. **自身所在目录**：仅当它看起来确实是安装目录（旁边有 `uninstall.exe` 或
///    清单声明的主程序）。对 `uninstall.exe` 而言这按定义必然成立 —— 它就躺在
///    安装目录里；对 `wind-installer.exe uninstall` 则通常不成立（安装器可能在
///    任何地方），那道存在性检查正是用来把这种情形挡掉的。
/// 3. 猜：`%ProgramFiles%\<app.id>`。留着是因为总得有个兜底，但它是最后一档。
pub fn resolve_install_dir() -> PathBuf {
    pick_install_dir(
        install_dir_from_arp(),
        self_dir_if_it_looks_installed(),
        default_install_dir(),
    )
}

/// 三级来源的**判据**，与取数分开 —— 否则这段逻辑要靠摆注册表才能验。
fn pick_install_dir(arp: Option<PathBuf>, self_dir: Option<PathBuf>, fallback: PathBuf) -> PathBuf {
    arp.or(self_dir).unwrap_or(fallback)
}

/// 读 ARP 的 `InstallLocation`。空串按「没有」处理 —— 一个空的
/// `InstallLocation` 会被 `PathBuf::from("")` 变成当前工作目录，那是灾难性的删除目标。
fn install_dir_from_arp() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    let key_path = format!(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}",
        meta::app_display_name()
    );
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(&key_path, KEY_READ)
        .ok()?;
    let dir: String = key.get_value("InstallLocation").ok()?;
    if dir.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(dir))
}

/// 本进程所在目录，当它看起来确实是一个安装目录时。
fn self_dir_if_it_looks_installed() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    looks_like_install_dir(&dir).then_some(dir)
}

/// 判据：目录里有 `uninstall.exe` 或清单声明的主程序。
///
/// 只看「自己叫不叫 uninstall.exe」是不够的：`wind-installer.exe` 被改名成
/// `xxx_uninstall.exe` 分发也是支持的用法（`main.rs` 就按文件名里含 "uninstall"
/// 来切模式），那种 exe 可以躺在下载目录里，它的 parent 绝不是安装目录。
fn looks_like_install_dir(dir: &Path) -> bool {
    dir.join(crate::installer::UNINSTALLER_NAME).exists() || dir.join(meta::main_exe()).exists()
}

fn default_install_dir() -> PathBuf {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
    PathBuf::from(pf).join(meta::app_id())
}

/// 解析本机实际生效的用户数据目录：优先读清单 `[datadir]` 声明的配置文件，其次默认位置。
///
/// 卸载确认对话框显示的路径与卸载真正删除的路径必须出自这里同一次解析——用户看到
/// 「将永久删除 X」时勾的是同意删 X，下游动作却是 `remove_dir_all`，两者一旦不同就是
/// 在骗用户按下不可逆的按钮。
///
/// **必须在 `UndoReceipt` 之前调用**——撤销会把这个配置文件删掉。卸载流程应在开始时
/// 解析一次并带着结果走（见 `steps::UninstallCtx::new`）。
pub fn resolve_user_data_dir() -> PathBuf {
    // 读 conf 的实现只此一份（安装向导显示的也是它），避免「界面显示 A、实际动 B」
    if let Some(dir) = crate::installer::userdata::read_datadir_conf() {
        return dir;
    }

    // 清单未声明 [datadir] 段、或配置文件缺失/为空 —— 用默认位置
    let app_data = std::env::var("APPDATA").unwrap_or_else(|_| {
        let mut p = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
        p.push("AppData\\Roaming");
        p.to_string_lossy().to_string()
    });
    PathBuf::from(app_data).join(meta::app_id())
}

impl CleanupOptions {
    /// 解析用户数据目录：见 [`resolve_user_data_dir`]。
    ///
    /// **必须在 `UndoReceipt` 之前调用**——撤销会把这个配置文件删掉。调用方应在
    /// 卸载开始时解析一次并带着结果走（见 `steps::UninstallCtx::new`）。
    pub fn user_data_dir(&self) -> PathBuf {
        resolve_user_data_dir()
    }
}

/// 本机数据目录 `%LOCALAPPDATA%\{app.id}` —— 清单 `[localdata]` 条目的作用域根。
///
/// 这里**不再拼死 `cache` 子目录**。哪些子路径是可重建的缓存、哪些是删了就没的状态，
/// 属于应用的内部布局；把 `"cache"` 写进通用安装器等于假定所有应用都按这个名字放缓存，
/// 而同一层里其余的东西（日志、本机状态标记）则一律看不见——于是卸载之后
/// `%LOCALAPPDATA%\{app.id}` 连同里面的东西整个留下来，那正是本次要修的残留。
///
/// `"cache"` 长得像通用词、不像产品名，所以躲过了 `defaults_are_domain_neutral` 那条
/// 领域中性测试。判据应当是「换一个应用它还成立吗」，而不是「它看着像不像产品名」。
pub fn local_data_dir() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
        let mut p = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
        p.push("AppData\\Local");
        p.to_string_lossy().to_string()
    });
    PathBuf::from(local_app_data).join(meta::app_id())
}

// ── 用户数据目录删除守卫 ─────────────────────────────────────────────────────
//
// 数据目录路径来自 `datadir.conf`——一个**用户可编辑的明文文件**，而下游动作是
// `remove_dir_all` + 拷贝到桌面。判错一次就是不可逆的数据损失，故删除前两层校验：
// 路径形状（通用）与内容标志（清单声明 `markers` 才启用）。

/// 不得整体删除的著名目录。取当前用户/系统环境实际值，空值自动跳过。
///
/// 只禁止**等于**，不禁止其子目录——默认数据目录 `%APPDATA%\{id}` 正是 `%APPDATA%`
/// 的子目录，一并禁掉会把正常卸载也拦下。
fn forbidden_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PUBLIC",
        "TEMP",
    ]
    .iter()
    .filter_map(|k| std::env::var_os(k).map(PathBuf::from))
    .collect();
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        for sub in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
        ] {
            v.push(profile.join(sub));
        }
    }
    v
}

/// 路径大小写不敏感相等（去尾分隔符）。
fn path_eq_ignore_case(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// 第一层：路径形状。纯函数（`forbidden` 由调用方注入），便于单测。
fn guard_shape(dir: &Path, forbidden: &[PathBuf]) -> Result<(), String> {
    // 绝对路径。这一条同时挡住 Windows 驱动器相对路径 `X:name`——它看着像绝对路径，
    // `is_absolute()` 却为 false，会解析到该盘当前目录上。
    if !dir.is_absolute() {
        return Err(format!("不是绝对路径: {}", dir.display()));
    }
    if dir
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("路径含 `..`: {}", dir.display()));
    }
    // `D:\` 的组件是 [Prefix, RootDir] = 2；`D:\Foo` 才是 3。据此排除驱动器/共享根。
    if dir.components().count() < 3 {
        return Err(format!("过于靠近驱动器根: {}", dir.display()));
    }
    if let Some(hit) = forbidden.iter().find(|f| path_eq_ignore_case(dir, f)) {
        return Err(format!("命中受保护目录: {}", hit.display()));
    }
    Ok(())
}

/// 第二层：内容标志。`markers` 为空即不检查；空目录一律放行（没东西可丢）。
fn guard_markers(dir: &Path, markers: &[String]) -> Result<(), String> {
    if markers.is_empty() {
        return Ok(());
    }
    let mut entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        // 读不出来就别删——宁可留下也不盲删。
        Err(e) => return Err(format!("无法读取目录（{}）: {}", e, dir.display())),
    };
    if entries.next().is_none() {
        return Ok(());
    }
    if markers.iter().any(|m| dir.join(m).exists()) {
        return Ok(());
    }
    Err(format!(
        "目录非空且不含本产品标志物（{}）: {}",
        markers.join(" / "),
        dir.display()
    ))
}

/// 用户数据目录是否可安全删除。两层守卫都通过才放行。
pub fn guard_user_data_dir(dir: &Path, markers: &[String]) -> Result<(), String> {
    guard_shape(dir, &forbidden_roots())?;
    guard_markers(dir, markers)
}

/// 删除安装文件
pub fn delete_install_files(install_dir: &PathBuf, r: &mut dyn Reporter) -> Result<(), String> {
    delete_install_files_with(
        install_dir,
        r,
        &reboot::schedule_delete_on_reboot,
        &reboot::schedule_dir_on_reboot,
    )
}

/// 同上，排队动作可注入。理由见 [`remove_path_with`]。
///
/// ⚠️ 这个注入口是**先于测试**开的：本函数目前一条测试都碰不到（它开头就读
/// `meta::process_names()`，而 `meta::manifest()` 未 bootstrap 即 panic，单测里没有清单），
/// 所以此刻它一个字节都没往注册表写。但下面三处排队一旦变可达就是活的污染源，
/// 其中尾部那条还是**递归**排队、一次能排一整棵树。
///
/// 顺序反过来的话——先补测试、再回头注入化——中间会有一段「测试已经在写真注册表
/// 而没人注意」的时间，而这种污染完全静默，不会有任何东西变红。上一次就是这么攒到
/// 36 对条目的。
fn delete_install_files_with(
    install_dir: &PathBuf,
    r: &mut dyn Reporter,
    schedule_file: &dyn Fn(&Path) -> Result<(), String>,
    schedule_dir: &dyn Fn(&Path),
) -> Result<(), String> {
    // 从 meta 动态构建二进制文件列表：进程名→.exe + ACL DLL 列表
    let mut binaries: Vec<String> = meta::process_names()
        .iter()
        .map(|n| format!("{}.exe", n))
        .collect();
    for dll in meta::acl_dlls() {
        if !binaries.iter().any(|b| b == dll) {
            binaries.push(dll.to_string());
        }
    }

    for binary in &binaries {
        let binary = binary.as_str();
        let path = install_dir.join(binary);
        if path.exists() && std::fs::remove_file(&path).is_err() {
            // 被锁定删不掉：改名让路 + 排重启删。改名成功就删改名后的，失败就直接排原路径。
            let random_suffix = rand::random::<u32>();
            let old_name = format!("{}.old_{:08x}", binary, random_suffix);
            let old_path = install_dir.join(&old_name);
            let target = if std::fs::rename(&path, &old_path).is_ok() {
                old_path
            } else {
                path
            };
            let _ = schedule_file(&target);
        }
    }

    // 删除数据目录
    let data_dir = install_dir.join("data");
    if data_dir.exists() && std::fs::remove_dir_all(&data_dir).is_err() {
        r.warn(&format!("数据目录未能删除: {}", data_dir.display()));
    }

    // 删除卸载程序
    let uninstall_exe = install_dir.join("uninstall.exe");
    if uninstall_exe.exists() {
        let _ = std::fs::remove_file(&uninstall_exe);
    }

    // 清理备份文件（含本次改名让路产生的 .old_）：仍锁定删不掉的排重启删。
    if let Ok(entries) = std::fs::read_dir(install_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.contains(".old_") || name.ends_with(".bak")) {
                continue;
            }
            let p = entry.path();
            if std::fs::remove_file(&p).is_err() {
                let _ = schedule_file(&p);
            }
        }
    }

    // 尝试删除安装目录：删不掉（尚有锁定文件）就**递归**排重启删。
    //
    // 这里此前只排目录自身一条，而 `MoveFileExW` 对非空目录无效——上面单独排过队的
    // 只有 binaries 与 `.old_`/`.bak` 残留，`data/` 等数据目录下的文件一个都没排，
    // 于是重启时这个目录仍非空、删除静默失败，整棵树的重启兜底等于不存在。
    // `schedule_dir_on_reboot` 自底向上逐项处理，且能当场删掉的一律当场删、不进队列，
    // 故正常路径（`remove_dir_all` 一把成功）根本走不到它，也不会多排任何重启任务。
    if std::fs::remove_dir_all(install_dir).is_err() {
        schedule_dir(install_dir);
    }

    Ok(())
}

// 注册表清理已由 `steps::UndoReceipt` 按安装回执反向回放接管——它撤销的是安装时
// **实际写成**的键，而非按当前清单猜测的键，故升级后仍能清掉上一版留下的东西。

/// 清理用户数据。
///
/// `user_data_dir` 由调用方在卸载开始时解析并传入，而非在此现算——`UndoReceipt`
/// 会删掉数据目录配置文件，现算就只能拿到默认位置，用户自定义的数据目录会被漏掉。
pub fn cleanup_user_data(
    options: &CleanupOptions,
    user_data_dir: &Path,
    r: &mut dyn Reporter,
) -> Result<(), String> {
    if options.keep_user_data {
        return Ok(());
    }

    // 清除用户配置（删除前可选备份到桌面）
    if options.clean_roaming && user_data_dir.exists() {
        // 守卫在**备份之前**：路径可疑时连拷贝都不做——把一个非本产品的目录整份复制到
        // 桌面同样是伤害（体积、隐私），而且随后就要 remove_dir_all 它。
        let markers = meta::manifest()
            .datadir
            .as_ref()
            .map(|d| d.markers.as_slice())
            .unwrap_or(&[]);
        match guard_user_data_dir(user_data_dir, markers) {
            // 守卫失败只跳过这一块，**不中断整个清理**——%LOCALAPPDATA% 那一侧的
            // 路径由 app.id 与清单条目算出，与这个可疑路径无关，照常清理。
            // 用户明确勾了「删除用户数据」却没删成，必须让他知道 —— 从前这句写进
            // eprintln!，而本程序没有控制台，等于什么都没说。
            Err(reason) => r.warn(&format!(
                "用户数据目录未删除（路径未通过安全检查：{}），请手动确认后自行删除：{}",
                reason,
                user_data_dir.display()
            )),
            Ok(()) => {
                if options.backup_to_desktop {
                    // 目录名带本地时间戳，每次卸载生成唯一目录，避免覆盖历史备份
                    let backup_dir = desktop_dir().join(format!(
                        "{}_Backup_{}",
                        meta::app_id(),
                        local_timestamp()
                    ));
                    if let Err(e) = copy_dir_all(user_data_dir, &backup_dir) {
                        // 备份是不可逆删除前的唯一退路，它失败了还照删不误 ——
                        // 这条无论如何都得说出来。
                        r.warn(&format!(
                            "备份用户数据到桌面失败（随后仍会删除原目录）: {}",
                            e
                        ));
                    }
                }
                if let Err(e) = std::fs::remove_dir_all(user_data_dir) {
                    r.warn(&format!(
                        "用户数据目录未能删除 {}: {}",
                        user_data_dir.display(),
                        e
                    ));
                }
            }
        }
    }

    // 清理 %LOCALAPPDATA%\{app.id}：两组条目各自跟随一个勾选，见 cleanup_local_data。
    //
    // 这里原先还有一句「始终清理 WebView2 缓存」，删的是 %TEMP%\{setting_exe_stem}。
    // 那是对某个应用内部布局的硬编码猜测，且已经猜空了：它在本机根本不存在，而设置
    // 程序也早已不带 WebView2。一段永远删不到东西的清理比没有更糟——它让「这一块已经
    // 清过了」看起来是真的。应用自己的目录该由 [localdata] 声明，不由安装器猜。
    cleanup_local_data(options, r);

    Ok(())
}

/// 清理 `%LOCALAPPDATA%\{app.id}` 下由**应用运行期**创建的东西。
///
/// 安装器自己往那里写的只有 `[datadir].conf_file`，它有回执、已在 `UndoReceipt`
/// 里删掉了（那一步排在本步之前）。剩下的缓存、日志、本机状态回执里没有，安装器也
/// 猜不出名字，只能由清单 `[localdata]` 声明——**整段缺省即完全跳过**。
///
/// 两组条目跟随**两个不同的勾选**：`cache_dirs` 跟「清除本地缓存」，`state_files`
/// 跟「删除用户数据」。这是本函数存在的理由——把它们合成一个开关，用户只勾了清缓存
/// 就会连状态一起丢，而他明确没勾另一个。
fn cleanup_local_data(options: &CleanupOptions, r: &mut dyn Reporter) {
    for entry in remove_local_data_entries(meta::localdata(), &local_data_dir(), options) {
        r.warn(&format!("跳过不安全的 [localdata] 条目 {:?}", entry));
    }
}

/// 真正动手的那一半：清单与作用域根都由调用方注入。
///
/// 拆出来是为了能对着一棵临时目录树断言——这是本次改动里唯一会**删除**东西的新路径，
/// 而它的三条契约（缺省一动不动、两个门控各管一组、空了才收目录）读代码都像是对的，
/// 只有真的建一棵树、删一遍、再看剩下什么，才分得清「写对了」和「看着像写对了」。
/// 返回**被拒绝**的条目（调用方负责汇报）。
///
/// 返回而不是就地 `r.warn`：本函数有 9 条单测直接对着一棵临时目录树调它，
/// 返回值让「哪些条目被拒了」也进得了断言 —— 只验「树还在」证明不了它报告过。
fn remove_local_data_entries(
    info: Option<&crate::manifest::LocalDataInfo>,
    root: &Path,
    options: &CleanupOptions,
) -> Vec<String> {
    remove_local_data_entries_with(
        info,
        root,
        options,
        &reboot::schedule_delete_on_reboot,
        &reboot::schedule_dir_on_reboot,
    )
}

/// 同上，排队动作可注入。理由见 [`remove_path_with`]。
fn remove_local_data_entries_with(
    info: Option<&crate::manifest::LocalDataInfo>,
    root: &Path,
    options: &CleanupOptions,
    schedule_file: &dyn Fn(&Path) -> Result<(), String>,
    schedule_dir: &dyn Fn(&Path),
) -> Vec<String> {
    // 清单没声明 [localdata] = 这个目录不归安装器管，一个字节都不碰（AGENTS.md 规则 2）。
    let Some(info) = info else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    let mut rejected = Vec::new();
    let entries = entries_to_remove(info, options);
    for entry in &entries {
        // 打包期 `AppManifest::validate` 已经拦过一遍；这里是运行期兜底——归档里的
        // 清单未必出自本机的打包器。拒绝的代价只是少删一项，放行的代价是
        // remove_dir_all 落到 %LOCALAPPDATA% 之外。
        match crate::manifest::safe_local_data_rel(entry.as_str()) {
            None => rejected.push(entry.to_string()),
            Some(rel) => remove_path_with(&root.join(rel), schedule_file, schedule_dir),
        }
    }

    // 目录空了才连它一起收掉。`remove_dir` 对非空目录直接失败，故「空」这个前提由 OS
    // 在删除的同一步里保证；先 read_dir 判一遍再删的话，中间那个窗口足够应用写回一个
    // 文件，删掉的就成了「刚才是空的」。这也顺带覆盖了「用户两个勾选都勾了」的常规
    // 路径——否则一个空目录会永远留在 %LOCALAPPDATA% 里。
    //
    // `!entries.is_empty()` 这一档：两个框都没勾（或清单两组都空）时，我们**根本没动过**
    // 这个目录，那就连收尾也别做——哪怕它恰好是空的。删一个空目录没有数据损失，但那是
    // 用户没勾的事，而这个目录属于应用的命名空间、不属于安装器。代价是一种窄情形会留下
    // 空目录：全新安装、应用一次没跑过（目录里只有安装器自己写的 conf，已被回执删掉），
    // 用户又把两个框都取消。那正是他要求的「什么都别动」。
    if info.remove_dir_when_empty && !entries.is_empty() {
        let _ = std::fs::remove_dir(root);
    }

    rejected
}

/// 本次要删的条目（相对作用域根），按两个勾选各自的门控筛出。
///
/// 抽成纯函数是为了能单测：「哪一组跟哪个勾选」是这段逻辑里唯一会出错、且出错后果
/// 不可逆的地方（把 `state_files` 接到缓存勾选上，用户只想清缓存却丢了状态），而它
/// 一旦接错，在界面上、在日志里、在任何一次成功的卸载里都看不出来——只有对着两个
/// 勾选的四种组合逐个断言才抓得住。
fn entries_to_remove<'a>(
    info: &'a crate::manifest::LocalDataInfo,
    options: &CleanupOptions,
) -> Vec<&'a String> {
    let mut out = Vec::new();
    if options.clean_local_cache {
        out.extend(info.cache_dirs.iter());
    }
    if options.clean_roaming {
        out.extend(info.state_files.iter());
    }
    out
}

/// 删一个条目，不区分文件还是目录——清单作者写 `logs` 时想的是「那一坨日志」，
/// 不该要求他先知道它在磁盘上是目录才写得对。
/// 删掉一个路径；删不掉就排进重启删除队列。两个排队动作由调用方给。
///
/// 没有「不带注入的薄壳」版本：唯一的调用方 [`remove_local_data_entries_with`] 本身
/// 就带着注入，再留一层只会给人一条绕过注入的近路。
///
/// ── 为什么必须能注入 ────────────────────────────────────────────────────
/// 真的那两个函数往 `HKLM\…\PendingFileRenameOperations` 写，那是**进程外的全局
/// 状态**，开机时由会话管理器**无条件执行**。测试里调到它们，等于在跑测试的人机器上
/// 留下真实的开机删除指令。
///
/// 这不是假想：2026-09-14 在编译机上实测，`undeletable_entries_are_recorded_in_the_reboot_ledger`
/// 每跑一轮往注册表写 2 对条目（`held.bin` 一次、`logs` 目录自身一次），攒到 **36 对**
/// 才被发现 —— 而且它是静默的，不会有任何东西变红。排的又都是
/// `%TEMP%\wind_guard_<pid>_…`，PID 会被系统复用，理论上能撞上一个真实文件。
///
/// `util::reboot` 那边早就把注入做对了（`schedule_dir_on_reboot_impl` 收 `&dyn Fn`），
/// 是**这个调用方绕过了它**。形状照抄那边，读代码的人不用学新东西。
///
/// ── 注入不损失任何测试强度 ──────────────────────────────────────────────
/// 这些测试断言的是**账本**（`reboot::is_reboot_pending()`），而
/// `schedule_delete_on_reboot` 里是 `record_pending(path, result.is_ok())` ——
/// 无论 `MoveFileExW` 成功还是失败都记账。所以断言从来不依赖那次真实调用成功：
/// 非提权环境下写 HKLM 本来就会失败，测试照样绿。假的排队函数自己记一笔账就够了。
fn remove_path_with(
    path: &Path,
    schedule_file: &dyn Fn(&Path) -> Result<(), String>,
    schedule_dir: &dyn Fn(&Path),
) {
    // 用 symlink_metadata 而非 `is_dir()`：后者跟随重解析点，遇到 junction 会递归进
    // 目标目录把**目标**删掉（本仓在 util::reboot 的递归排队里踩过同一个坑）。
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return; // 不存在：本就没什么可删
    };
    let is_dir = meta.file_type().is_dir();
    let r = if is_dir {
        std::fs::remove_dir_all(path)
    } else {
        // 普通文件，或指向目录的 junction/符号链接——后者 remove_file 会拒绝访问，
        // 而 remove_dir 能把链接本身摘掉且不碰目标。
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))
    };
    if r.is_err() {
        // 删不掉必须**记账**，不能只 eprintln——卸载器是 windows_subsystem = "windows"
        // 的无控制台进程，打出去等于丢弃（AGENTS.md §锁定文件与「需要重启」）。
        //
        // 这里的失败是高概率而非理论：WindInput 的 cache_dirs 含 logs，而 logs/tsf_log/
        // 由仍被 ctfmon 加载的 TSF DLL 直接写着，DLL 没释放时 remove_dir_all 必然失败。
        // 不记账则 need_reboot 漏判 ⇒ 完成页告诉用户卸载干净 ⇒ 那堆日志永久留下——
        // 正是本次要修的那个残留，只是下移了一层。
        //
        // 目录走递归排队：MoveFileExW 对非空目录无效，只排目录自身等于没排
        // （commit 457b542 为同一件事引入了 schedule_dir_on_reboot）。
        if is_dir {
            schedule_dir(path);
        } else {
            let _ = schedule_file(path);
        }
    }
}

/// 当前用户桌面目录（%USERPROFILE%\Desktop）
fn desktop_dir() -> PathBuf {
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
    PathBuf::from(user_profile).join("Desktop")
}

/// 本地时间戳 YYYYMMDD_HHMMSS（用于备份目录名，使每次备份唯一、不覆盖历史）
fn local_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// 递归复制目录（用于卸载前备份用户数据到桌面）
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod guard_tests {
    use super::*;

    fn forbidden() -> Vec<PathBuf> {
        vec![
            PathBuf::from(r"C:\Windows"),
            PathBuf::from(r"C:\Users\Someone"),
            PathBuf::from(r"C:\Users\Someone\Desktop"),
        ]
    }

    /// 正常数据目录必须放行——守卫拦错了等于卸载删不干净。
    #[test]
    fn normal_data_dirs_pass() {
        for ok in [r"C:\Users\Someone\AppData\Roaming\Demo", r"D:\MyData\Demo"] {
            assert!(
                guard_shape(Path::new(ok), &forbidden()).is_ok(),
                "应放行: {ok}"
            );
        }
    }

    /// 驱动器根、UNC 共享根：一旦删下去就是整盘。
    #[test]
    fn drive_and_share_roots_rejected() {
        for bad in [r"D:\", r"C:\", r"\\server\share"] {
            assert!(
                guard_shape(Path::new(bad), &forbidden()).is_err(),
                "应拒绝: {bad}"
            );
        }
    }

    /// 驱动器相对路径 `X:name` 看着像绝对路径，实则落到该盘当前目录。
    #[test]
    fn drive_relative_and_traversal_rejected() {
        assert!(guard_shape(Path::new("C:data"), &forbidden()).is_err());
        assert!(guard_shape(Path::new(r"data\Demo"), &forbidden()).is_err());
        assert!(guard_shape(Path::new(r"D:\a\..\..\Windows"), &forbidden()).is_err());
    }

    /// 著名目录本身不可整体删；大小写与尾分隔符不应绕过。
    #[test]
    fn forbidden_roots_rejected_case_insensitively() {
        for bad in [r"C:\Windows", r"c:\windows\", r"C:\Users\Someone\Desktop"] {
            assert!(
                guard_shape(Path::new(bad), &forbidden()).is_err(),
                "应拒绝: {bad}"
            );
        }
    }

    /// 但受保护目录的**子目录**必须放行——默认数据目录正是 `%APPDATA%` 的子目录。
    #[test]
    fn children_of_forbidden_roots_pass() {
        assert!(guard_shape(Path::new(r"C:\Users\Someone\AppData"), &forbidden()).is_ok());
    }

    #[test]
    fn markers_empty_means_no_content_check() {
        let dir = tmpdir("markers_off");
        std::fs::write(dir.join("random.txt"), b"x").unwrap();
        assert!(guard_markers(&dir, &[]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非空但不含标志物 = 大概率不是我们的目录（用户把 conf 改成了 D:\Documents 之类）。
    #[test]
    fn non_empty_without_markers_rejected() {
        let dir = tmpdir("markers_miss");
        std::fs::write(dir.join("holiday.jpg"), b"x").unwrap();
        let markers = vec!["config.toml".to_string(), "schemas".to_string()];
        assert!(guard_markers(&dir, &markers).is_err());

        // 命中任一标志物即放行（子目录形式也算）。
        std::fs::create_dir_all(dir.join("schemas")).unwrap();
        assert!(guard_markers(&dir, &markers).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空目录放行：没东西可丢，拦下来只会留垃圾。全新装未启动过就是这种状态。
    #[test]
    fn empty_dir_passes_marker_check() {
        let dir = tmpdir("markers_empty");
        assert!(guard_markers(&dir, &["config.toml".to_string()]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── [localdata] 两组条目各自的门控 ───────────────────────────────────────
    //
    // 四种勾选组合逐个断言。只测「都勾」会让一条接错的线照样通过：两组都进结果时，
    // 无论 state_files 挂在哪个开关上，结果都一样。

    fn localdata() -> crate::manifest::LocalDataInfo {
        crate::manifest::LocalDataInfo {
            cache_dirs: vec!["cache".into(), "logs".into()],
            state_files: vec!["state.toml".into()],
            remove_dir_when_empty: true,
        }
    }

    // 逐字段构造，不走 `..Default::default()`：`CleanupOptions::default()` 会读
    // `meta::app_id()`，而那是个未初始化就 panic 的全局 OnceLock——单测里没有清单。
    fn opts(cache: bool, roaming: bool) -> CleanupOptions {
        CleanupOptions {
            install_dir: PathBuf::from(r"C:\Program Files\Demo"),
            clean_local_cache: cache,
            clean_roaming: roaming,
            backup_to_desktop: false,
            keep_user_data: false,
        }
    }

    /// 只勾「清除本地缓存」时**绝不能**碰 state_files——用户明确没勾另一个，
    /// 而那一组删了就没了。这是本模块最不能错的一条。
    #[test]
    fn cache_checkbox_never_touches_state_files() {
        let info = localdata();
        let got = entries_to_remove(&info, &opts(true, false));
        assert_eq!(got, vec!["cache", "logs"]);
    }

    /// 反向同理：只勾「删除用户数据」不该顺手清缓存。缓存删了无害，但界面上没说要删。
    #[test]
    fn user_data_checkbox_only_takes_state_files() {
        let info = localdata();
        let got = entries_to_remove(&info, &opts(false, true));
        assert_eq!(got, vec!["state.toml"]);
    }

    #[test]
    fn both_unchecked_removes_nothing() {
        let info = localdata();
        assert!(entries_to_remove(&info, &opts(false, false)).is_empty());
    }

    #[test]
    fn both_checked_takes_every_entry() {
        let info = localdata();
        let got = entries_to_remove(&info, &opts(true, true));
        assert_eq!(got, vec!["cache", "logs", "state.toml"]);
    }

    // ── 真正落地的删除：对着临时目录树验收 ───────────────────────────────────

    /// 建一棵典型的本机数据树：cache/ 与 logs/ 是缓存，state.toml 是状态。
    fn local_tree(tag: &str) -> PathBuf {
        let root = tmpdir(tag);
        for d in ["cache", "logs"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("x.bin"), b"x").unwrap();
        }
        std::fs::write(root.join("state.toml"), b"pos=1").unwrap();
        root
    }

    /// **缺省一动不动**（AGENTS.md 规则 2）。清单没声明 [localdata] 时，即便两个勾选
    /// 都勾上、目录就在那里，也一个字节都不能碰——这个目录不归安装器管。
    #[test]
    fn absent_localdata_touches_nothing() {
        let root = local_tree("absent");
        remove_local_data_entries(None, &root, &opts(true, true));
        assert!(root.join("cache").exists(), "缺省时不该删缓存");
        assert!(root.join("state.toml").exists(), "缺省时不该删状态");
        assert!(root.exists(), "缺省时连目录本身也不该收");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只勾「清除本地缓存」：缓存那几项落地消失，状态**必须还在盘上**。
    /// 门控测试只证明了筛选结果，这一条证明的是「筛完之后删的确实是它们」。
    #[test]
    fn cache_gate_leaves_state_files_on_disk() {
        let root = local_tree("cache_only");
        remove_local_data_entries(Some(&localdata()), &root, &opts(true, false));
        assert!(!root.join("cache").exists(), "缓存该被删掉");
        assert!(
            !root.join("logs").exists(),
            "日志也在 cache_dirs 里，该被删掉"
        );
        assert!(
            root.join("state.toml").exists(),
            "用户没勾「删除用户数据」，状态文件必须留下"
        );
        // 还有东西在，remove_dir 会失败，目录因此保留——这正是非递归 remove_dir 的用意。
        assert!(root.exists(), "目录非空时不该被收掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 两个都勾且树被清空 → 目录本身一并收掉。这是「卸载后留一个空目录」那个残留的收口。
    #[test]
    fn empty_root_is_removed_after_both_gates() {
        let root = local_tree("both");
        remove_local_data_entries(Some(&localdata()), &root, &opts(true, true));
        assert!(!root.exists(), "清空后目录本身也该没了");
    }

    /// 目录里还有清单没列到的东西时，绝不能顺手删掉整个目录。`remove_dir` 对非空目录
    /// 直接失败，这条契约由 OS 保证——测试钉的是「我们没有绕过它去 remove_dir_all」。
    #[test]
    fn unlisted_leftovers_keep_the_dir_alive() {
        let root = local_tree("leftover");
        std::fs::write(root.join("something_else.dat"), b"user").unwrap();
        remove_local_data_entries(Some(&localdata()), &root, &opts(true, true));
        assert!(root.exists(), "还有没列到的文件时不能收掉目录");
        assert!(
            root.join("something_else.dat").exists(),
            "没被声明的东西一律不动"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 两个框都没勾 = 我们根本没动过这个目录，那么连「空了就收掉」也不该做。
    /// 删一个空目录没有数据损失，但那是用户没勾的事。
    #[test]
    fn both_unchecked_leaves_even_an_empty_dir_alone() {
        let root = tmpdir("empty_untouched"); // 空目录
        remove_local_data_entries(Some(&localdata()), &root, &opts(false, false));
        assert!(
            root.exists(),
            "两个框都没勾时，连空目录也不该收——那是用户没勾的事"
        );
        // 对照：勾了缓存（我们确实动过这个目录）→ 空了就该收掉
        remove_local_data_entries(Some(&localdata()), &root, &opts(true, false));
        assert!(!root.exists(), "勾了之后清空的目录该被收掉");
    }

    /// 运行期兜底：归档里的清单未必出自本机的打包器，故守卫要在删之前再跑一次。
    /// 用 `..` 逃出作用域根去删兄弟目录——放行的话删的就是 %LOCALAPPDATA% 下的别人。
    #[test]
    fn unsafe_entries_are_skipped_at_runtime() {
        let root = local_tree("unsafe");
        let sibling = root.parent().unwrap().join("wind_guard_sibling_victim");
        let _ = std::fs::remove_dir_all(&sibling);
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("keep.txt"), b"keep").unwrap();

        let evil = crate::manifest::LocalDataInfo {
            // 取样刻意**跨过分支中心**：`..` / `.` / `C:x` 各是一个分支的正中央，
            // 只挑它们，得到的是「守卫有几个分支」的信心，不是「守卫封闭」的信心——
            // 两个 P0 当初正是从分支之间的缝里穿过去的。后四条就是那些缝。
            cache_dirs: vec![
                "../wind_guard_sibling_victim".into(),
                ".".into(),
                "C:x".into(),
                "./C:x".into(), // 非首分量带盘符
                "logs/C:x".into(),
                " C:x".into(),
                "...".into(), // 尾点剥完指回根
                ". .".into(),
            ],
            state_files: vec![],
            remove_dir_when_empty: false,
        };
        remove_local_data_entries(Some(&evil), &root, &opts(true, false));

        assert!(sibling.join("keep.txt").exists(), "`..` 条目必须被拒");
        assert!(root.join("cache").exists(), "`.` 条目被放行就会删光整个根");
        assert!(root.exists());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    /// 守卫修复的**落地**验收：拿真实目录树跑一遍两个 P0 的具体形态。
    ///
    /// 只断言 `classify_local_data_entry` 返回什么是不够的——上一版守卫的毛病恰恰是
    /// 「校验的串」与「拿去 join 的串」不是同一个，那种错误在枚举层面看不出来。这里
    /// 直接过 `remove_local_data_entries`，看树还在不在。
    ///
    /// 修复前的实测结果（已复现）：`...` 让 remove_dir_all 把整棵树删光；`logs/C:x`
    /// 拼出 `C:x` 落到 C: 盘当前目录。
    ///
    /// **承重范围要说清楚**：注入验证显示，去掉尾点归零那条检查会让本测试变红，但去掉
    /// 盘符检查**不会**——带盘符的条目逃到作用域根**之外**，根里的东西反而毫发无损，
    /// 这种「删错了地方」在「树还在不在」上照不出来。盘符那一档的承重测试是
    /// `tests/manifest_parse.rs::drive_prefix_in_any_component_is_rejected`（它直接断言
    /// `safe_local_data_rel` 拿不到值）。这里保留那几个条目，是为了确认它们至少不会
    /// **顺带**破坏根内的东西。
    #[test]
    fn p0_forms_cannot_destroy_the_tree() {
        let root = local_tree("p0");
        let evil = crate::manifest::LocalDataInfo {
            cache_dirs: vec![
                "...".into(), // 剥掉尾点后指回根本身
                ". .".into(),
                "logs/C:x".into(), // 非首分量带盘符 → push 截断缓冲区
                "./C:x".into(),
                " C:x".into(),
                "cache/C:evil".into(),
            ],
            state_files: vec![],
            remove_dir_when_empty: false,
        };
        let rejected = remove_local_data_entries(Some(&evil), &root, &opts(true, false));

        // 被拒的条目必须**报出来**，不能悄悄跳过：用户以为清理干净了，实际有一条
        // 没执行。这里六条全都该被拒。
        assert_eq!(rejected.len(), 6, "被拒条目没有全部上报: {rejected:?}");

        assert!(root.exists(), "作用域根被删掉了——`...` 那一档又漏了");
        assert!(
            root.join("cache").join("x.bin").exists(),
            "根下的内容被删掉了"
        );
        assert!(root.join("logs").exists(), "logs 被误删");
        assert!(root.join("state.toml").exists(), "状态文件被误删");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 删不掉时必须**记账**，不能只 eprintln——卸载器是无控制台进程，打出去等于丢弃，
    /// 而 need_reboot 会因此漏判、完成页告诉用户「卸载干净」。
    ///
    /// 用 `share_mode(0)` 独占打开目录里的一个文件来制造真实的删除失败：这正是现场
    /// 的形态（logs/tsf_log 由仍被 ctfmon 加载的 TSF DLL 占着）。
    #[test]
    fn undeletable_entries_are_recorded_in_the_reboot_ledger() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = local_tree("locked");
        reboot::reset_ledger();
        assert!(!reboot::is_reboot_pending(), "前置：账本应为空");

        let locked = root.join("logs").join("held.bin");
        std::fs::write(&locked, b"x").unwrap();
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0) // 不共享读/写/删除 —— 谁也删不掉它
            .open(&locked)
            .expect("独占打开失败");

        // ⚠️ 走注入版。用真的那个会往注册表写两对真实的开机删除指令
        // （`held.bin` 与 `logs` 各一），指向 `%TEMP%\wind_guard_<pid>_…` —— PID 会被
        // 系统复用，而 PFRO 开机时无条件执行。实测攒到 36 对才被发现。
        remove_path_with(
            &root.join("logs"),
            &fake_schedule_file,
            &fake_schedule_dir,
        );

        assert!(
            reboot::is_reboot_pending(),
            "删不掉却没记账：need_reboot 会漏判，完成页就会谎报卸载干净"
        );
        drop(_guard);
        reboot::reset_ledger();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 假的排队动作：**自己记一笔账，但不碰注册表**。
    ///
    /// 断言的是账本，而真函数里是 `record_pending(path, result.is_ok())` —— 无论
    /// `MoveFileExW` 成没成功都记账。所以把真调用换掉一点强度都不损失，换来的是
    /// 测试不再往 `HKLM\…\PendingFileRenameOperations` 写真实的开机删除指令。
    ///
    /// 传 `true`（而不是 `result.is_ok()`）是有意的：这里模拟的是「成功排进队列」，
    /// 需要验「排不进队列」那一支的测试应另写一个返回 `Err` 的版本。
    fn fake_schedule_file(p: &Path) -> Result<(), String> {
        reboot::record_pending(p, true);
        Ok(())
    }

    /// 目录版。真的那个会自底向上递归，这里只记目录自身 —— 本模块的断言只问
    /// 「账本非空吗」，递归与否不影响；真正验递归行为的是 `reboot` 自己那组测试。
    ///
    /// ⚠️ **做变异检验时改的是这一个，不是上面那个。**
    /// `undeletable_entries_are_recorded_in_the_reboot_ledger` 喂给 `remove_path_with`
    /// 的是 `logs` **目录**，所以只走 `schedule_dir` 这一支；把
    /// [`fake_schedule_file`] 改成不记账，测试照样绿 —— 那是个**无效变异**
    /// （实测过），不是测试的漏洞。改本函数才会红。
    fn fake_schedule_dir(p: &Path) {
        reboot::record_pending(p, true);
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wind_guard_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

/// 安装目录三级来源的测试。
///
/// 测的是**判据**（`pick_install_dir`），不是取数：读注册表那半边要摆 ARP 状态才能验，
/// 单测够不着。但真正出过事的正是判据——静默路径当初压根没问过前两级。
#[cfg(test)]
mod install_dir_tests {
    use super::pick_install_dir;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// ARP 里有就用 ARP —— 那是安装时亲手写下的目录，比任何推断都硬。
    #[test]
    fn arp_wins_over_everything() {
        let got = pick_install_dir(Some(p(r"D:\Apps\Demo")), Some(p(r"C:\elsewhere")), p(r"C:\PF\Demo"));
        assert_eq!(got, p(r"D:\Apps\Demo"));
    }

    /// ARP 读不到（键被上一次半截卸载删了、或压根是便携安装）时退到自身所在目录。
    /// 对 `uninstall.exe` 而言这一级按定义必然正确。
    #[test]
    fn self_dir_is_the_second_source() {
        let got = pick_install_dir(None, Some(p(r"D:\Apps\Demo")), p(r"C:\PF\Demo"));
        assert_eq!(got, p(r"D:\Apps\Demo"));
    }

    /// 两级都没有才猜。**这一级从前是唯一的一级**，静默卸载因此去删了一个猜出来的目录。
    #[test]
    fn guessing_is_the_last_resort() {
        let got = pick_install_dir(None, None, p(r"C:\PF\Demo"));
        assert_eq!(got, p(r"C:\PF\Demo"));
    }

    /// 顺序不能倒：自身目录再像，也压不过 ARP 记录的那个。
    /// （升级换过目录时，旧目录里可能还躺着一个 uninstall.exe。）
    #[test]
    fn self_dir_never_overrides_arp() {
        let got = pick_install_dir(Some(p(r"D:\new")), Some(p(r"C:\old")), p(r"C:\PF\Demo"));
        assert_ne!(got, p(r"C:\old"), "自身目录压过了 ARP");
    }
}
