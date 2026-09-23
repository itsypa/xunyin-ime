//! 重启延迟文件操作 —— 处理「被占用、当前删不掉」的文件的通用兜底。
//!
//! Windows 允许对已打开的文件做同卷改名（只改目录项），却不允许删除被持锁进程占用的
//! 文件。标准做法是把删除**排进下次启动队列**：登记到
//! `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`，
//! 由会话管理器（smss.exe）在开机极早期、文件尚未被任何进程加载前执行——那时删除必然成功。
//!
//! 这是 NSIS `Delete /REBOOTOK`、MSI `InstallFinalize` 背后的同一机制，属通用安装器能力，
//! 与具体应用无关。需要管理员权限（本安装/卸载器已通过 manifest 提权）。
//!
//! # 为什么有一本进程级账本
//!
//! 「有旧文件删不掉」这个事实产生在最底层的 IO 处（解压时的 `create_or_backup`、
//! 旧版遗留清理、卸载时的文件删除），那里既没有 `Step` 上下文也没有 `Reporter`；
//! 而需要它的是最顶层——完成页要提示用户重启、quiet 模式要据此**放弃自动退出**。
//! 逐层改签名会把 `need_reboot` 穿进 `extract_entry`/`extract_all` 等与之无关的 API。
//!
//! `MoveFileExW(DELAY_UNTIL_REBOOT)` 本身就是写进程外全局状态的操作，故这里用
//! [`LEDGER`] 记账只是把它已有的全局性显式化，[`run_plan`] 在收尾处一次性读取汇总。
//!
//! [`run_plan`]: crate::installer::step::run_plan

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};

/// 一条「当前删不掉、留待重启处理」的记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingItem {
    /// 被推迟的路径（升级场景下通常是改名后的 `.old_xxxxxxxx`）。
    pub path: PathBuf,
    /// 是否成功登记到 `PendingFileRenameOperations`。
    ///
    /// `false` 表示连排队都失败了——重启也不会自动清理，属于更严重的残留，
    /// 但对用户的建议是一样的（重启后进程不再持锁，可手工删或由下次安装清掉）。
    pub scheduled: bool,
}

fn ledger() -> &'static Mutex<Vec<PendingItem>> {
    static LEDGER: OnceLock<Mutex<Vec<PendingItem>>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Vec::new()))
}

/// 取账本；忽略中毒（记账线程 panic 不该让后续记账全部失败）。
fn ledger_lock() -> std::sync::MutexGuard<'static, Vec<PendingItem>> {
    ledger().lock().unwrap_or_else(|e| e.into_inner())
}

/// 安排在下次系统启动时删除 `path`，并记入本次运行的账本。
///
/// 用于锁定文件的兜底清理：当前删不掉就登记 `MoveFileExW(path, NULL, DELAY_UNTIL_REBOOT)`，
/// 下次开机由会话管理器删除。对空目录同样有效（可删除重启时已清空的残留目录）。
///
/// 无论成功与否都会记账：调用方只需 best-effort 调用，「是否该提示用户重启」由
/// [`is_reboot_pending`] 统一回答。失败（如权限不足、路径非法）返回 `Err`，
/// 由调用方决定是否降级为普通告警——本函数不 panic、不影响主流程。
pub fn schedule_delete_on_reboot(path: &Path) -> Result<(), String> {
    // 需以 NUL 结尾的宽字符串；空目标（第二参 NULL）表示「删除」而非「移动」。
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        MoveFileExW(
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    }
    .map_err(|e| format!("排定重启删除失败 {:?}: {}", path, e));

    record_pending(path, result.is_ok());
    result
}

/// 直接记一笔「当前处理不掉的残留」而不尝试排队。
///
/// 用于连改名让路都做不到、或路径本就不该进 `PendingFileRenameOperations` 的场景：
/// 事实仍需被计入「建议重启」的依据。
pub fn record_pending(path: &Path, scheduled: bool) {
    let item = PendingItem {
        path: path.to_path_buf(),
        scheduled,
    };
    let mut guard = ledger_lock();
    // 同一路径可能被多次尝试（改名失败后重试）；只保留一条，成功状态取「曾经成功过」。
    if let Some(existing) = guard.iter_mut().find(|i| i.path == item.path) {
        existing.scheduled |= scheduled;
        return;
    }
    guard.push(item);
}

/// 本次运行是否留下了需要重启才能清理干净的东西。
pub fn is_reboot_pending() -> bool {
    !ledger_lock().is_empty()
}

/// 账本快照，供日志与诊断使用。
pub fn pending_items() -> Vec<PendingItem> {
    ledger_lock().clone()
}

/// 一行人类可读摘要，如 `3 个文件待重启后清理（其中 1 个未能排队）`。
/// 账本为空时返回空串。
pub fn pending_summary() -> String {
    let items = pending_items();
    if items.is_empty() {
        return String::new();
    }
    let unscheduled = items.iter().filter(|i| !i.scheduled).count();
    if unscheduled == 0 {
        format!("{} 个文件待重启后清理", items.len())
    } else {
        format!(
            "{} 个文件待重启后清理（其中 {} 个未能排入删除队列）",
            items.len(),
            unscheduled
        )
    }
}

/// 清空账本。**仅供测试**——一次真实运行内不存在「重新开始计数」的正当场景。
///
/// 之所以是 `pub` 而非 `#[cfg(test)]`：本 crate 的 `--lib` 单测二进制名含 "install"，
/// 会命中 Windows UAC 安装器检测启发式而无法在普通权限下启动（见 `ui::theme` 末注），
/// **账本**的测试因此只能放在 `tests/` 集成测试里，那里链接的是非 test 编译的 lib。
/// （本文件末尾那组走查单测不碰账本 —— 排队动作是注入的 —— 故可以内联。）
#[doc(hidden)]
#[allow(dead_code)] // 只被 tests/ 引用；bin target 看不到那边的用法
pub fn reset_ledger() {
    ledger_lock().clear();
}

// ── 目录树的重启删除 ─────────────────────────────────────────────────────────

/// 把一棵删不掉的目录树排进重启删除队列。
///
/// `MoveFileExW` 对**非空**目录无效，故必须自底向上逐项处理：先文件、再子目录、
/// 最后目录自身。只排目录一条的话，重启时它仍非空，删除会静默失败——这正是
/// 「安装目录里有文件被占用 ⇒ 重启兜底整棵树都无效」这个缺口的成因。
///
/// 三条不变量，改动时勿破：
///
/// 1. **能当场删掉的东西不进队列。** 无占用时走完这里一条记录都不留，账本为空，
///    「需要重启」的结论不受影响；队列长度只反映真正卡住的东西（实测安装目录
///    ~100 文件 / ~40 目录，即便全卡住也远低于注册表值 1MB 的标准格式上限）。
/// 2. **删不掉的文件先改名 `.old_xxxxxxxx` 再排队。** 排进
///    `PendingFileRenameOperations` 的删除要到**下次开机**才执行，而用户完全可能
///    在此之前就重装到同一目录。排原路径 = 让上一次卸载的遗留指令删掉新装的同名
///    文件（重启后「词库莫名消失」）；排一个随机后缀名则永不与新装产物撞名。
///    这与解压让路、卸载删二进制是同一套路。
/// 3. **正在运行的自身可执行文件一概不碰，连装着它的目录也不排队。** 改名后
///    `current_exe()` 仍返回旧路径（`GetModuleFileNameW` 在加载时固化），卸载器的
///    自删除流程会因此复制不到自己而静默失效；排队它的原路径又会在重装后删掉新的
///    `uninstall.exe`，把 ARP 条目指向不存在的程序。它由 `uninstaller::selfdelete`
///    负责，这里让开。
///
///    「连目录也让开」这半句是后加的，修的是一个每次卸载都会犯的错：GUI 卸载走到
///    这里时安装目录里正剩着运行中的 `uninstall.exe`，跳过它之后 `remove_dir(dir)`
///    必然因非空而失败，于是目录被排进队列、账本非空 —— `is_reboot_pending()` 为真，
///    完成页于是**每次**都提示「部分文件正被占用，需重启电脑才能彻底清除」。而自删除
///    副本随后就把整棵树删了，那条队列条目成了空转，提示也从头到尾是假的。
///    每次都出现的假提示，效果等于训练用户忽略它。
///    目录的归宿和那个 exe 是同一件事，一起让给自删除流程才自洽；自删除真失败时，
///    副本会在**自己不在树里**的身份下重跑本函数，那时 exe 与目录都会被正常排队。
///
///    这条豁免对其余调用方（`installer::legacy` 的遗留目录、`uninstaller::cleanup`
///    的 `%LOCALAPPDATA%` 条目）同样生效。那两棵树里没有自删除兜底，但也从不会命中
///    ——安装器跑在 `%TEMP%`/下载目录，卸载器不在 `%LOCALAPPDATA%`；而且即便命中，
///    被豁免掉的那条目录条目**本来就是空转**：`MoveFileExW` 对非空目录无效，而自身
///    exe 按本不变量从不排队，重启那一刻目录必然还非空。换言之这条豁免不会让任何
///    调用方少清理一个字节，它只是不再把一条注定空转的记录算进「需要重启」。
pub fn schedule_dir_on_reboot(dir: &Path) {
    schedule_dir_on_reboot_impl(dir, &is_current_exe, &schedule_delete_on_reboot);
}

/// [`schedule_dir_on_reboot`] 的本体。返回 true 表示这棵树里留着正在运行的自身 exe，
/// 目录的最终清理**已让给自删除流程**，本次没有排队它。
///
/// `is_self` 与 `schedule` 都由调用方注入。
///
/// - `is_self`：判据本身（`is_current_exe`）已有子进程测试守着，而「接线对不对」
///   用注入就能直接测，不必每次都去起一个住在被测目录里的子进程。
/// - `schedule`：真的那个会往 `HKLM\…\PendingFileRenameOperations` 写真条目 ——
///   那是个全局共享值、开机由会话管理器无条件执行，测试每跑一次加一条不可接受。
/// [`schedule_dir_on_reboot`] 的可注入版本，**给测试用**。
///
/// 只换掉「往注册表写」那一步，`is_self` 仍是真的 `is_current_exe` —— 需要它的测试
/// （`tests/reboot_dir_tree.rs` 里那些 spawn 子进程的）正是要验「走查放过自身 exe」，
/// 把它也假掉就没什么可验的了。
///
/// 为什么要有这个入口：真的 [`schedule_delete_on_reboot`] 往
/// `HKLM\…\PendingFileRenameOperations` 写，那是进程外全局状态、开机时无条件执行。
/// 集成测试里 `already_stashed_names_are_not_renamed_twice` 会在树里种一个正在运行的
/// exe，走查删不掉它 → `stash_aside` → **真的排进队列**。那条测试的判据是文件系统
/// （名字有没有被改第二次），排队只是副作用，所以换掉不损失任何强度。
///
/// `#[doc(hidden)]`：这不是给生产代码用的入口，生产代码用 [`schedule_dir_on_reboot`]。
#[doc(hidden)]
#[allow(dead_code)] // 只有 tests/ 里的集成测试用它，编译 bin target 时就是「未使用」
pub fn schedule_dir_on_reboot_with_queue(
    dir: &Path,
    schedule: &dyn Fn(&Path) -> Result<(), String>,
) -> bool {
    schedule_dir_on_reboot_impl(dir, &is_current_exe, schedule)
}

fn schedule_dir_on_reboot_impl(
    dir: &Path,
    is_self: &dyn Fn(&Path) -> bool,
    schedule: &dyn Fn(&Path) -> Result<(), String>,
) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // 连列目录都做不到（权限/句柄问题）：至少把目录本身记一笔，
        // 让「需要重启」的结论不会因为这里读不到而丢失。
        record_pending(dir, false);
        return false;
    };

    // 这棵树（含子树）里留着自身 exe 吗？留着的话目录删不掉是**预期**的，
    // 不该记成一笔重启账。
    let mut deferred_to_self_delete = false;

    for entry in entries.flatten() {
        let path = entry.path();
        // 用 `file_type()` 而非 `path.is_dir()`：后者跟随符号链接/junction，会递归进
        // 链接目标把**目标目录**的内容删掉。`FileType::is_dir()` 对重解析点返回 false，
        // 于是链接按「文件」处理——删的是链接本身，不是它指向的东西。
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            deferred_to_self_delete |= schedule_dir_on_reboot_impl(&path, is_self, schedule);
        } else if is_self(&path) {
            // 不删、不改名、不排队 —— 只记下「这个目录删不掉是应该的」。
            deferred_to_self_delete = true;
        } else if std::fs::remove_file(&path).is_err() {
            // 走到这里的「非目录」除了普通文件，还有指向目录的重解析点（junction /
            // 目录符号链接）——对它们 `remove_file` 恒为「拒绝访问」，而 `remove_dir`
            // 直接成功且**只摘掉链接、目标目录安然无恙**。不先试这一下的话，安装目录
            // 里只要有一个 junction，每次卸载都会白白多一条重启账、完成页无端提示
            // 「需要重启」，还留下一个改了名的链接。
            if std::fs::remove_dir(&path).is_err() {
                let _ = schedule(&stash_aside(&path));
            }
        }
    }

    // 子项清空后目录本身通常就能删掉——能删就删，别往队列里塞无谓的条目（不变量 1）。
    //
    // 删不掉且树里留着自身 exe：那正是自删除流程要收拾的局面，不记账（不变量 3）。
    // 注意此时**别的**删不掉的文件仍各自排过队、记过账，「需要重启」该真就真。
    if std::fs::remove_dir(dir).is_err() && !deferred_to_self_delete {
        let _ = schedule(dir);
    }

    deferred_to_self_delete
}

/// 把删不掉的文件改名让路，返回**实际要排队的路径**；改名失败则原路返回。
///
/// 同卷改名只改目录项，对已被打开/已加载为映像的文件同样成立；只有被以不含
/// `FILE_SHARE_DELETE` 方式打开的文件会失败，那时退回排原路径（见不变量 2）。
/// 调用方有两处：卸载删除安装目录二进制，以及 IME 系统副本的覆盖/删除让路
/// （`installer::ime`）——后者不在安装目录里，`delete_install_files` 够不着。
///
/// **已经是 `.old_xxxxxxxx` 的名字原样返回、不再改一次。** 这不是洁癖：卸载时
/// `delete_install_files` 的 binaries 循环会先把锁定的 `app.exe` 改成
/// `app.exe.old_aaaaaaaa` 并排队，随后 `remove_dir_all` 失败、递归兜底又碰上同一个
/// 文件。再改一次名的后果是队列里出现**两条**——先排的那条指向已不存在的路径，成了
/// 空转指令，而 `pending_summary()` 报给用户的「N 个文件待重启后清理」也随之偏大。
/// 那个随机后缀本来就是防撞的，套第二层没有任何收益。
pub(crate) fn stash_aside(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return path.to_path_buf();
    };
    if is_stashed_name(&name) {
        return path.to_path_buf();
    }
    let stashed = path.with_file_name(format!("{}.old_{:08x}", name, rand::random::<u32>()));
    if std::fs::rename(path, &stashed).is_ok() {
        stashed
    } else {
        path.to_path_buf()
    }
}

/// 名字是否已是本模块（或 `delete_install_files`）产出的让路名：`.old_` + 8 位十六进制。
fn is_stashed_name(name: &str) -> bool {
    match name.rsplit_once(".old_") {
        Some((stem, suffix)) => {
            !stem.is_empty() && suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// 该路径是否就是当前进程的可执行文件（见不变量 3）。
///
/// 比较走 `canonicalize` 而非字符串：两边一个来自 `GetModuleFileNameW`、一个由
/// `install_dir.join(名字)` 拼出，可能在 8.3 短名、junction 挂载点、大小写上写法不同，
/// 而**判漏的代价是自删除静默失效**——那种失败没有任何日志能看出来。
///
/// **文件名快速通道的名字必须取自 `canonicalize` 之后的路径**，不能取 `current_exe()`
/// 的原始返回值：进程若是以 8.3 短路径启动的（`install directory` 这类带空格的目录
/// 必有短名），`current_exe()` 原样返回 `...\INSTAL~1\UNINST~1.EXE`，而 `read_dir`
/// 永远给磁盘长名 `uninstall.exe`——两边比不上，函数会**在 canonicalize 之前就返回
/// `false`**，自删除随即静默失效（实测复现过）。canonicalize 后两边都是磁盘长名。
///
/// 取不到自身路径时返回 `false`：那只是让自身 exe 与其他文件同等对待（改名+排队），
/// 不会影响其余条目的处理；反过来返回 `true` 又会放过一个同名文件。两害相权取其轻。
///
/// `pub` 的理由同 [`reset_ledger`]：本 crate 的 `--lib` 单测跑不起来（二进制名含
/// "install"，命中 UAC 安装器启发式），这条不变量只能在 `tests/` 里断言。
#[doc(hidden)]
pub fn is_current_exe(path: &Path) -> bool {
    static SELF_EXE: OnceLock<Option<(std::ffi::OsString, PathBuf)>> = OnceLock::new();
    let Some((self_name, self_real)) = SELF_EXE
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let real = std::fs::canonicalize(&exe).unwrap_or(exe);
            let name = real.file_name()?.to_os_string();
            Some((name, real))
        })
        .as_ref()
    else {
        return false;
    };

    // 先比文件名（无 IO）——绝大多数条目在这里就否掉了，不必为每个文件做一次
    // `canonicalize` 系统调用。
    if path.file_name().map(|n| n.eq_ignore_ascii_case(self_name)) != Some(true) {
        return false;
    }
    std::fs::canonicalize(path).as_deref().unwrap_or(path) == self_real.as_path()
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod dir_walk_tests {
    use super::*;
    use std::cell::RefCell;

    // 变异检验已做（每条都实跑过，归因是跑出来的不是推出来的）：
    //   · 去掉 `&& !deferred_to_self_delete`（退回修复前：自身 exe 在场也排目录）
    //     → self_exe_in_tree_defers_the_directory 变红
    //   · `is_self` 恒 true（矫枉过正：每个文件都当自身 exe，真残留也不排了）
    //     → other_locked_files_are_still_queued 变红
    //   · `deferred_to_self_delete` 初始化成 true（没有自身 exe 也豁免目录）
    //     → plain_tree_is_deleted_and_queues_nothing 变红
    //   · 子目录的 deferred 不向上传播（`|=` 改成丢弃）
    //     → self_exe_in_a_subdir_defers_the_whole_chain 变红
    //   · 把 deferred 这个 gate **下放到文件分支**（`remove_dir(&path).is_err()
    //     && !deferred_to_self_delete`）→ other_locked_files_are_still_queued 变红。
    //     这条是后人最容易写出来的过度抑制（读起来像「整棵树都让给自删除了，
    //     那就都别排了」），而它能不能被抓**取决于 read_dir 的枚举顺序** ——
    //     实测：只放一个 `held.bin` 时变异**逃逸**（NTFS 按文件名给，它排在
    //     `uninstall.exe` 之前，处理它时 deferred 还是 false），加上 `zz_held.bin`
    //     两边夹住之后稳定变红。两次都实跑过。
    //
    // ⚠️ 上面第二、三条容易被写成同一条，但它们**抓手不同**：`deferred` 只在末尾
    // 那一处被读，对循环里 `schedule(&stash_aside(&path))` 没有任何 gate，所以
    // 「初始化成 true」不会让锁定文件漏排，`other_locked_files_are_still_queued`
    // 对它是绿的（实测），抓住它的是对照组那条的 `assert!(!deferred)`。
    // 本轮初稿把两者的描述与测试名配错了 —— 账本写错比没写更坏，它会让人以为
    // 某条性质有守卫。归因必须来自实跑。
    //
    // 这些测试一个字节都不落进注册表：排队动作是注入的。真的那个会往全局的
    // PendingFileRenameOperations 里写，开机时无条件执行。
    //
    // ⚠️ 做变异检验时替换文本必须在文件里唯一，命中多处一律视为无效 ——
    // 否则会连测试一起改、得到一次假绿（教训出处见 uninstaller/selfdelete.rs）。

    /// 记下被要求排队的路径，一律报成功；不碰注册表。
    struct FakeQueue(RefCell<Vec<PathBuf>>);

    impl FakeQueue {
        fn new() -> Self {
            Self(RefCell::new(Vec::new()))
        }
        fn schedule(&self, p: &Path) -> Result<(), String> {
            self.0.borrow_mut().push(p.to_path_buf());
            Ok(())
        }
        /// 被排队的文件名（只看名字，临时目录前缀无关紧要）
        fn names(&self) -> Vec<String> {
            self.0
                .borrow()
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        }
    }

    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wind_walk_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 独占打开一个文件，让它既删不掉也改不了名。
    fn lock(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::write(path, b"x").unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .expect("独占打开失败")
    }

    #[test]
    fn plain_tree_is_deleted_and_queues_nothing() {
        // 对照组：没有自身 exe、也没有占用 —— 一路当场删掉，账本与队列都该是空的。
        let root = tree("plain");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/dict.wdat"), b"x").unwrap();

        let q = FakeQueue::new();
        let deferred = schedule_dir_on_reboot_impl(&root, &|_| false, &|p| q.schedule(p));

        assert!(!deferred, "没有自身 exe，不该说「让给自删除」");
        assert!(q.names().is_empty(), "白排了队: {:?}", q.names());
        assert!(!root.exists(), "能删掉的树没被删掉");
    }

    /// 本次修的那条：树里留着正在运行的自身 exe 时，**目录本身也不排队**。
    ///
    /// 从前会排 —— 于是账本非空、`is_reboot_pending()` 为真、完成页每次卸载都提示
    /// 「需重启电脑才能彻底清除」，而自删除副本随后就把整棵树删了，提示从头到尾是假的。
    #[test]
    fn self_exe_in_tree_defers_the_directory() {
        let root = tree("self");
        let me = root.join("uninstall.exe");
        std::fs::write(&me, b"x").unwrap();
        std::fs::write(root.join("readme.txt"), b"x").unwrap();

        let q = FakeQueue::new();
        let deferred = schedule_dir_on_reboot_impl(&root, &|p| p == me, &|p| q.schedule(p));

        assert!(deferred, "该说「让给自删除」");
        assert!(
            q.names().is_empty(),
            "目录或自身 exe 被排了队: {:?}",
            q.names()
        );
        assert!(me.exists(), "自身 exe 被动了");
        assert!(!root.join("readme.txt").exists(), "其余文件该照删");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 不能矫枉过正：自身 exe 在场只豁免**目录自己**，别的删不掉的文件照排、
    /// 「需要重启」该真就真。
    ///
    /// ⚠️ 两个锁定文件一前一后夹住 `uninstall.exe`，这是**刻意**的，别精简成一个。
    ///
    /// 要防的回归形态是把 `deferred` 这个 gate 下放到文件分支：
    /// `if remove_dir(&path).is_err() && !deferred_to_self_delete { schedule(...) }`
    /// —— 它读起来很像「整棵树都让给自删除了，那就都别排了」，是后人最容易写出来的
    /// 过度抑制。而 `read_dir` 在 NTFS 上按文件名（UTF-16 大写）顺序给，只放一个
    /// `held.bin` 的话它排在 `uninstall.exe` **之前**，处理它时 `deferred` 还是 false、
    /// 照样排队，测试全绿、变异逃逸 —— 那条断言等于押在枚举顺序上，而且押输。
    /// 夹住之后，无论先给谁，总有一个是在 `deferred` 已置真之后才处理的。
    #[test]
    fn other_locked_files_are_still_queued() {
        let root = tree("mixed");
        let me = root.join("uninstall.exe");
        std::fs::write(&me, b"x").unwrap();
        // 名字一前一后夹住 "uninstall.exe"
        let held_a = root.join("aa_held.bin");
        let held_z = root.join("zz_held.bin");
        let guard_a = lock(&held_a);
        let guard_z = lock(&held_z);

        let q = FakeQueue::new();
        let deferred = schedule_dir_on_reboot_impl(&root, &|p| p == me, &|p| q.schedule(p));

        assert!(deferred);
        let names = q.names();
        assert_eq!(
            names.len(),
            2,
            "两个锁定文件都该排队，与枚举顺序无关: {names:?}"
        );
        // share_mode(0) 连 rename 都挡，stash_aside 原路返回，故名字不带 .old_ 后缀；
        // 用 starts_with 兼容两种情形。
        for stem in ["aa_held.bin", "zz_held.bin"] {
            assert!(
                names.iter().any(|n| n.starts_with(stem)),
                "{stem} 没被排队: {names:?}"
            );
        }
        assert!(
            !names.iter().any(|n| n == "uninstall.exe"),
            "自身 exe 被排队了: {names:?}"
        );

        drop(guard_a);
        drop(guard_z);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 自身 exe 在子目录里时，豁免要一路传到根 —— 否则根目录照样被排队，
    /// 假提示原样回来。
    #[test]
    fn self_exe_in_a_subdir_defers_the_whole_chain() {
        let root = tree("subdir");
        let sub = root.join("bin");
        std::fs::create_dir_all(&sub).unwrap();
        let me = sub.join("uninstall.exe");
        std::fs::write(&me, b"x").unwrap();

        let q = FakeQueue::new();
        let deferred = schedule_dir_on_reboot_impl(&root, &|p| p == me, &|p| q.schedule(p));

        assert!(deferred, "子目录的豁免没传上来");
        assert!(q.names().is_empty(), "链条上有东西被排队: {:?}", q.names());
        assert!(me.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
