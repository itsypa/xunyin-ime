//! `schedule_dir_on_reboot` 在**无占用**时的行为。
//!
//! 这是它最重要也最容易被改坏的一条性质：卸载走到「兜底」分支时（GUI 路径下正在
//! 运行的 uninstall.exe 会让 `remove_dir_all` 必然失败，故这条分支是常态而非异常），
//! 目录树里绝大多数文件其实都删得掉。若实现改成「先排队再说」，每次卸载都会往
//! `PendingFileRenameOperations` 里塞上百条，并且让完成页无缘无故提示「需要重启」。
//!
//! 放在 `tests/` 而非源码内联单测：本 crate 的 `--lib` 单测二进制名含 "install"，
//! 会命中 Windows UAC 安装器检测启发式而无法在普通权限下启动（见 `ui::theme` 末注）。
//! 文件名同样不含 "install"/"setup"/"update"/"patch"。

#![cfg(windows)]

use std::path::{Path, PathBuf};

use wind_installer::util::reboot;

/// 测试用临时目录：正常结束时清干净，**panic 时保留现场**。
///
/// 两边都要：跑绿之后留一堆临时目录是纯噪音、在 CI runner 上还会累积；而失败时那棵树
/// 往往就是最有力的证据——本次反事实实验留下的
/// `…junc_*\target`（`keepme.txt` 没了）、`…self_*\probe_self.exe.old_dcab069a`
/// 各自一眼说明了「跟随了 junction」「自身 exe 被改名」。
///
/// `std::fs::remove_dir_all` 不跟随重解析点（CVE-2022-21658 之后的 std 保证），
/// 故对含 junction 的树也只摘链接、不会删穿目标目录。
struct TempTree(PathBuf);

impl TempTree {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("wind_reboot_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("测试失败，保留现场供勘验: {}", self.0.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 三层深、含空目录与多文件，覆盖「自底向上」的每一种节点。
fn tmp_tree(tag: &str) -> TempTree {
    let t = TempTree::new(tag);
    let root = t.path();
    std::fs::create_dir_all(root.join("data/dicts")).unwrap();
    std::fs::create_dir_all(root.join("data/themes/empty")).unwrap();
    std::fs::write(root.join("top.txt"), b"x").unwrap();
    std::fs::write(root.join("data/config.toml"), b"x").unwrap();
    std::fs::write(root.join("data/dicts/a.wdat"), b"x").unwrap();
    std::fs::write(root.join("data/dicts/b.wdat"), b"x").unwrap();
    std::fs::write(root.join("data/themes/t.json"), b"x").unwrap();
    t
}

/// 账本是进程级共享的而 cargo 默认并行跑测试，故全部塞进一个 `#[test]` 顺序执行。
#[test]
fn unlocked_tree_is_deleted_outright_and_queues_nothing() {
    reboot::reset_ledger();

    let tree = tmp_tree("clean");
    let root = tree.path();
    reboot::schedule_dir_on_reboot(root);

    assert!(!root.exists(), "无占用时整棵树应当场删净，而不是留给重启");
    assert!(
        !reboot::is_reboot_pending(),
        "无占用时一条重启任务都不该排——否则每次卸载都会误报「需要重启」，实际排到的是：{:?}",
        reboot::pending_items()
    );

    // 目录读不出来时（这里用「根本不存在」模拟）仍要记一笔，否则「需要重启」会漏判
    reboot::schedule_dir_on_reboot(&root.join("gone"));
    assert!(
        reboot::is_reboot_pending(),
        "列目录失败必须记账，不能静默丢弃"
    );

    reboot::reset_ledger();
}

/// 不变量 3：**正在运行的自身可执行文件一概不碰**。
///
/// 这条是整个改动里最要命的一行。GUI 卸载时 `uninstall.exe` 就在 `install_dir` 里且
/// 正在运行，`remove_dir_all` 必然失败 ⇒ 递归分支是常态而非异常。若递归实现把它
/// 改名 `.old_xxxxxxxx`，`trigger_self_delete` 的 `fs::copy(current_exe(), ...)` 会
/// 找不到源文件——因为 `current_exe()`（`GetModuleFileNameW`）返回的是**加载时固化**
/// 的旧路径，不随改名更新。自删除会因此每次都静默失效，且没有任何日志看得出来。
///
/// 实测（本机 rustc 探针，改名后进程内连续 12 次采样）：改名成功、`current_exe()`
/// 12 次全部仍返回旧路径。故这里判漏 = 自删除失效，判据必须扛得住写法差异。
#[test]
fn current_executable_is_recognized_across_spellings() {
    let me = std::env::current_exe().expect("取不到自身路径");
    assert!(reboot::is_current_exe(&me), "自身路径必须认得出来");

    // 大小写不同的同一路径（Windows 路径不区分大小写）
    let shouty = PathBuf::from(me.to_string_lossy().to_uppercase());
    assert!(
        reboot::is_current_exe(&shouty),
        "换个大小写写法就认不出来 = 自删除会失效: {shouty:?}"
    );

    // 同目录下的另一个文件不能被误判——误判会让真正该清理的文件被放过
    let sibling = me.with_file_name("definitely_not_me.exe");
    assert!(
        !reboot::is_current_exe(&sibling),
        "不该误判同目录的其他文件"
    );
}

// ── 接线测试 ────────────────────────────────────────────────────────────────
//
// 上面两条钉的是**函数**，钉不住它们在 `schedule_dir_on_reboot` 里的**接线**——而接线
// 才是判据。反事实实测：把 `!is_current_exe(&path) &&` 整段摘掉、或把 `file_type()`
// 换回 `path.is_dir()`，上面的测试全都照绿。下面两条专治这一点。

/// 子进程模式开关：值是要遍历的目录。
const CHILD_DIR_ENV: &str = "WIND_REBOOT_CHILD_WALK_DIR";
/// 子进程退出码：走查在「只剩自身 exe」的树上留下了重启账目。
const CHILD_EXIT_LEDGER_DIRTY: i32 = 2;
/// 子进程模式开关：值是睡眠毫秒数（把自己变成一个「正在运行故删不掉」的文件）。
const CHILD_SLEEP_ENV: &str = "WIND_REBOOT_CHILD_SLEEP_MS";

/// 两个子进程角色的统一入口。测试二进制被复制进临时树后重新拉起，靠环境变量分派；
/// 拉起时统一用 `--exact self_executable_is_not_touched_by_the_walk` 定位到这里。
///
/// 必须是每个 spawn 型测试的第一句：子进程不该跑真正的测试体。
fn child_mode_or_continue() {
    if let Ok(dir) = std::env::var(CHILD_DIR_ENV) {
        let root = PathBuf::from(&dir);
        // ⚠️ 走可注入版：真的那个会往 `PendingFileRenameOperations` 写。
        // `already_stashed_names_are_not_renamed_twice` 在树里种了一个**正在运行的**
        // exe，走查删不掉它 → `stash_aside` → 真排进队列，而那条队列指向
        // `%TEMP%\wind_reboot_stash_<pid>\…`，PID 会被系统复用、PFRO 开机时无条件执行。
        // 本文件的判据全在文件系统与账本上，排队只是副作用，换掉不损失任何强度。
        reboot::schedule_dir_on_reboot_with_queue(&root, &|p| {
            reboot::record_pending(p, true);
            Ok(())
        });
        // 走查放过自身 exe 之后，**装着它的那个目录也不该进队列**。
        //
        // 只能在子进程里断言：只有在这里 `is_current_exe` 才真的命中。
        //
        // ⚠️ 判据是「**这个目录**在不在账本里」，不是「账本空不空」。树里另有真的
        // 删不掉的文件时（`already_stashed_names_are_not_renamed_twice` 就是那样），
        // 账本非空是**合理**的 —— 那些文件确实要等重启。写成「账本空不空」会把那条
        // 合理的账目一并禁掉，本次第一版就是这么写的，跑全套时被那条测试抓了出来。
        // 覆盖边界：只看**根**。自身 exe 种在根（见 `plant_walk_target`），所以
        // 「自身 exe 在子目录 + 真 is_current_exe」这个组合本文件不覆盖 —— 那条由
        // `util::reboot` 里注入版的 self_exe_in_a_subdir_defers_the_whole_chain 守。
        // 现实里 uninstall.exe 确实在根，这个分工是有意的。
        if reboot::pending_items().iter().any(|i| i.path == root) {
            eprintln!(
                "子进程：装着自身 exe 的目录被排进了队列 {:?}",
                reboot::pending_items()
            );
            std::process::exit(CHILD_EXIT_LEDGER_DIRTY);
        }
        std::process::exit(0);
    }
    if let Ok(ms) = std::env::var(CHILD_SLEEP_ENV) {
        std::thread::sleep(std::time::Duration::from_millis(ms.parse().unwrap()));
        std::process::exit(0);
    }
}

/// 取路径的 8.3 短形式；该卷未生成短名时返回 `None`。
///
/// 借 `cmd` 的 `%~sI` 而不是 `GetShortPathNameW`：`windows` crate 是普通依赖而非
/// dev-dependency，集成测试链不到它。
///
/// **必须用 `raw_arg`**：`arg()` 会把内层引号转义成 `\"`，而 cmd 不认这种写法，命令
/// 静默失败、函数回落到长路径——`spawn_child` 于是悄悄不再覆盖 8.3 那条缺陷。
/// 这个坑本次就踩过一次：反事实变异跑出来是绿的，才发现覆盖早没了。
fn short_path(p: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;

    let out = std::process::Command::new("cmd")
        .raw_arg(format!("/c for %I in (\"{}\") do @echo %~sI", p.display()))
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let cand = PathBuf::from(&s);
    // 短名与长名相同（名字本来就短）也算没拿到——那样起不到区分作用
    if cand.exists() && cand != p {
        Some(cand)
    } else {
        None
    }
}

/// 把测试二进制复制到 `dest`（种下一个「将要运行的自身 exe」）。
fn plant_child(dest: &Path) {
    std::fs::copy(std::env::current_exe().unwrap(), dest).unwrap();
}

/// 从 `launch` 按 `role` 拉起子进程。
///
/// `launch` 与 `plant_child` 的 `dest` 可以是同一路径的不同写法——短路径那条覆盖
/// 就靠这个：`current_exe()` 在子进程里返回什么，取决于它是被怎么拉起来的。
fn spawn_at(launch: &Path, role: (&str, &str)) -> std::process::Child {
    std::process::Command::new(launch)
        .env(role.0, role.1)
        .args([
            "--exact",
            "self_executable_is_not_touched_by_the_walk",
            "--nocapture",
        ])
        .spawn()
        .unwrap_or_else(|e| panic!("拉起子进程 {launch:?} 失败: {e}"))
}

/// 不变量 3 的**接线**：走查必须放过正在运行的自身 exe，既不删也不改名。
///
/// 只能用子进程测——「自身 exe 在被遍历的目录里」这个前提，测试进程自己满足不了
/// （它的二进制在 `target\debug\deps`，不能拿去做实验）。故：把测试二进制复制进临时树，
/// 带 `CHILD_DIR_ENV` 把它 spawn 起来，子进程在**自己就住在里面**的那棵树上跑走查。
///
/// 判据全在文件系统上，不看账本：自身文件名原样还在 + 树里没有任何 `.old_` 产物。
/// 摘掉 `!is_current_exe(&path) &&` 这一段，子进程会把自己改名，本测试立刻变红——
/// 而那正是「GUI 卸载的自删除静默失效」在代码里的样子。
///
/// **这条走长路径，不依赖任何环境特性，CI 必跑。** 「`is_current_exe` 有没有被接上」
/// 用长路径就守得住；只有「M1 那个具体修法」才必须短路径，那部分拆在
/// [`self_executable_is_recognized_via_short_path`]。
///
/// 注：本测试**不再**往 `PendingFileRenameOperations` 里留那条指向临时根目录的记录。
/// 从前子进程收尾时会对它调一次 `MoveFileExW(DELAY_UNTIL_REBOOT)`（根目录里还剩着
/// 它自己，删不掉），在管理员/CI 下留下一条空转指令。那条指令本身无害，但它同时让
/// 账本非空、完成页每次卸载都无端提示「需要重启」—— 现在走查会把这个目录一并让给
/// 自删除流程，子进程也顺带断言了它没进队列（见 `child_mode_or_continue`）。
#[test]
fn self_executable_is_not_touched_by_the_walk() {
    child_mode_or_continue();

    let tree = TempTree::new("self");
    let child_exe = plant_walk_target(&tree);
    run_walk_child(&child_exe, tree.path());
    assert_self_survived(&child_exe, tree.path());
}

/// M1 的具体修法：`is_current_exe` 的文件名快速通道必须拿 **canonicalize 之后**的名字。
///
/// 进程以 8.3 短路径启动时 `current_exe()` 原样返回 `...\PROBE_~1.EXE`，而 `read_dir`
/// 永远给磁盘长名 `probe_self.exe`。快速通道若拿原始返回值当自身名，两边比不上，函数
/// 会**在 `canonicalize` 之前就返回 `false`**，自身 exe 随即被改名、自删除静默失效。
/// 长路径下新旧实现都认得，故这条覆盖只有短路径建得起来。
///
/// 拿不到短名就**跳过并打印**：该卷可能关掉了 8.3 短名生成
/// （`fsutil 8dot3name query <卷>`，需管理员），CI runner 的工作盘尤其不可预判。
/// 跳过不会让覆盖悄悄消失——上面那条长路径测试永远在跑，守着「接线还在不在」；
/// 这里只是少守了「修法是否退化」这一层，而它有独立的用例名，一眼看得出缺了什么。
#[test]
fn self_executable_is_recognized_via_short_path() {
    child_mode_or_continue();

    let tree = TempTree::new("shortpath");
    let child_exe = plant_walk_target(&tree);

    let Some(launch) = short_path(&child_exe) else {
        eprintln!(
            "跳过 self_executable_is_recognized_via_short_path：\
             拿不到 {child_exe:?} 的 8.3 短名（该卷多半关闭了短名生成，\
             `fsutil 8dot3name query <卷>` 可查，需管理员）。\
             接线本身仍由 self_executable_is_not_touched_by_the_walk 守着。"
        );
        return;
    };
    assert_ne!(launch, child_exe, "short_path 该给出不同的写法");

    let status = spawn_at(&launch, (CHILD_DIR_ENV, &tree.path().to_string_lossy()))
        .wait()
        .expect("等待子进程失败");
    assert_child_ok(status);
    assert_self_survived(&child_exe, tree.path());
}

/// 在树里种一个 `data/` 与一个「将要运行的自身 exe」，返回后者的**长路径**。
///
/// 副本名不含 "install"/"setup"：那会命中 Windows UAC 安装器检测启发式，副本起不来
/// （os error 740），测试会以一个与被测行为无关的理由变红。
fn plant_walk_target(tree: &TempTree) -> PathBuf {
    let root = tree.path();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/dict.wdat"), b"x").unwrap();
    let child_exe = root.join("probe_self.exe");
    plant_child(&child_exe);
    child_exe
}

fn run_walk_child(launch: &Path, root: &Path) {
    let status = spawn_at(launch, (CHILD_DIR_ENV, &root.to_string_lossy()))
        .wait()
        .expect("等待子进程失败");
    assert_child_ok(status);
}

/// 子进程的退出码就是断言结果，这里把它翻译成人话。
fn assert_child_ok(status: std::process::ExitStatus) {
    if status.code() == Some(CHILD_EXIT_LEDGER_DIRTY) {
        panic!(
            "走查把「只剩自身 exe 的目录」排进了重启队列 —— \
             完成页会因此每次都提示「需重启电脑才能彻底清除」，\
             而自删除副本随后就把整棵树删了，那条提示从头到尾是假的"
        );
    }
    assert!(status.success(), "子进程异常退出: {status:?}");
}

fn assert_self_survived(child_exe: &Path, root: &Path) {
    assert!(
        child_exe.exists(),
        "自身 exe 被走查动了——原名已不在，自删除会因此静默失效"
    );
    let stashed: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".old_"))
        .collect();
    assert!(stashed.is_empty(), "自身 exe 被改名让路了: {stashed:?}");
    assert!(
        !root.join("data").exists(),
        "走查根本没跑起来（`data/` 还在），前面的断言就成了假绿"
    );
}

/// 走查不得跟随重解析点（junction / 目录符号链接），且应当场把链接摘掉。
///
/// 两条性质一次测完，判据都在文件系统上：
/// - **不跟随**：`file_type()` 换回 `path.is_dir()` 就会递归进链接、把**目标目录**里的
///   文件删掉。真实场景不是假想——开发机上常有指向主仓构建产物的 junction，删穿就是
///   删掉源仓的数据。
/// - **当场摘掉**（S2）：对 junction，`remove_file` 恒为「拒绝访问」而 `remove_dir`
///   直接成功。少了这一步，链接会被改名 + 排重启删，于是根目录非空、删不掉，
///   测试里表现为「根目录还在」，真实里表现为每次卸载白白多一条重启账。
#[test]
fn junctions_are_unlinked_without_following_them() {
    let tree = TempTree::new("junc");
    let base = tree.path();
    let target = base.join("target");
    let root = base.join("root");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(target.join("keepme.txt"), b"precious").unwrap();
    std::fs::write(root.join("plain.txt"), b"x").unwrap();

    let link = root.join("link");
    let ok = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "建 junction 失败，本测试无从验证（需要 NTFS）");

    reboot::schedule_dir_on_reboot(&root);

    assert!(
        target.join("keepme.txt").exists(),
        "走查跟随了 junction，把目标目录里的文件删掉了"
    );
    assert!(
        !root.exists(),
        "根目录没删掉——junction 多半是被改名排队而不是当场摘掉"
    );
}

/// 已经是 `.old_xxxxxxxx` 的名字不得再改一次（S3）。
///
/// 真实序列：`delete_install_files` 的 binaries 循环先把锁定的 `app.exe` 改名
/// `app.exe.old_aaaaaaaa` 并排队，随后 `remove_dir_all` 失败、递归兜底又碰上同一个
/// 文件。若再改一次名，队列里就有**两条**——先排的那条指向已不存在的路径（空转指令），
/// 而 `pending_summary()` 报给用户的「N 个文件待重启后清理」也随之偏大。
///
/// 造「删不掉但改得动名」的文件只有一个办法：让它是个**正在运行的 exe**。普通文件
/// 无论怎么开句柄，挡住 delete 的同时也会挡住 rename（rename 一样要 DELETE 权限），
/// 那样就分不清「早返回了」还是「改名本来就失败」，测试会假绿。
#[test]
fn already_stashed_names_are_not_renamed_twice() {
    child_mode_or_continue();

    let tree = TempTree::new("stash");
    let root = tree.path();

    // 这一个「正在运行」故删不掉、但改得动名——正是走查会去 stash_aside 的那种文件
    let locked = root.join("probe_locked.exe.old_aaaaaaaa");
    plant_child(&locked);
    let mut sleeper = spawn_at(&locked, (CHILD_SLEEP_ENV, "8000"));

    let walker = root.join("probe_walk.exe");
    plant_child(&walker);
    run_walk_child(&walker, root);

    let names: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "probe_locked.exe.old_aaaaaaaa"),
        "既有的让路名被改掉了，队列里那条旧记录随即变成空转指令: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.matches(".old_").count() > 1),
        "让路名被套了第二层: {names:?}"
    );

    // 必须在 `tree` 析构之前收掉：它还占着 root 里的一个文件，不然清理会失败
    let _ = sleeper.kill();
    let _ = sleeper.wait();
}
