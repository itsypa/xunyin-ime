use std::path::Path;

use winreg::enums::*;
use winreg::RegKey;

use crate::manifest::FontInfo;
use crate::util::reboot;

/// 系统字体注册表路径
const FONTS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";

/// 安装单个系统字体。
///
/// 调用方（`steps::InstallFonts`）逐个字体调用并按成功与否写回执。安装回执取代了
/// 此前的「InstalledFont_* 跟踪值」机制：两者作用相同（证明字体是我们装的），
/// 但回执更精确——装成几个记几条，不会因中途失败而让已装的字体漏记。
pub fn install_one(install_dir: &Path, font: &FontInfo) -> Result<(), String> {
    let font_source = install_dir.join(&font.source_rel);
    if !font_source.exists() {
        return Err(format!("Font file not found: {:?}", font_source));
    }

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let font_dest = Path::new(&windir).join("Fonts").join(&font.file);

    // 字体已存在且大小一致则跳过复制，仅确保注册表正确。
    //
    // ⚠️ 已知的窄坑，跳过复制时无从规避：上一次卸载若因文件被占用而把**这个路径**
    // 排进了 PendingFileRenameOperations（见 `remove_or_schedule`），而用户没重启就
    // 重装，这里会跳过复制、只重写注册表；下次开机队列按路径删除，注册项就指向了一个
    // 已不存在的文件（字体静默失效，再装一次即可）。
    // 队列是按路径记的，重新放一个文件回去并不能把它取消，所以这里堵不住 ——
    // `remove_or_schedule` 那侧先 `stash_aside` 改名让路，正是为了尽量不让原路径进队列。
    let same_size = font_dest.exists()
        && matches!(
            (std::fs::metadata(&font_source), std::fs::metadata(&font_dest)),
            (Ok(s), Ok(d)) if s.len() == d.len()
        );

    if !same_size {
        std::fs::copy(&font_source, &font_dest)
            .map_err(|e| format!("Failed to copy font file: {}", e))?;
    }

    register_font_in_registry(font)?;
    Ok(())
}

/// 在注册表中注册字体
fn register_font_in_registry(font: &FontInfo) -> Result<(), String> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let fonts_key = hklm
        .open_subkey_with_flags(FONTS_KEY, KEY_WRITE)
        .map_err(|e| format!("Failed to open Fonts key: {}", e))?;

    fonts_key
        .set_value(&font.display_name, &font.file)
        .map_err(|e| format!("Failed to register font: {}", e))?;

    Ok(())
}

/// 按回执记录的文件名与显示名卸载单个字体。
///
/// 不读清单——回执的存在本身即证明这个字体是我们装的，故升级后清单里已删除的
/// 字体仍能被上一版回执正确清掉。
pub fn uninstall_font_file(file: &str, display_name: &str) -> Result<(), String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let font_path = Path::new(&windir).join("Fonts").join(file);

    if font_path.exists() {
        // ⚠️ 删不掉**不算**「未能清除」，而是「需要重启」。
        //
        // 字体是登录时由 GDI 按注册表加载进会话字体表的（install_one 只拷文件 + 写
        // 注册表，不调 AddFontResourceW），所以「装完用过一阵再卸载」时这个文件
        // 大概率正被占用，remove_file 报 ERROR_SHARING_VIOLATION。那是**正常路径**。
        // 从前这里直接 `?` 返回 Err，一旦卸载结果接上真实判定，每次正常卸载都会
        // 显示「有项目未能清除」——比谎报成功更糟。排进重启删除队列才是对的归类：
        // 用户看到的是成功页上那句「需重启电脑才能彻底清除」。
        remove_or_schedule(&font_path)?;
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(fonts_key) = hklm.open_subkey_with_flags(FONTS_KEY, KEY_WRITE) {
        // 值不在就算了（被清理工具或用户删过）；真删不掉则要说出来 ——
        // 留着它，重装同名字体时系统会指向一个已不存在的文件。
        if let Err(e) = fonts_key.delete_value(display_name) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("字体注册项未能删除 {:?}: {}", display_name, e));
            }
        }
    }

    Ok(())
}

/// 删一个文件；删不掉就排进重启删除队列并记账。
///
/// 抽出来是为了测得到：`uninstall_font_file` 动的是 `C:\Windows\Fonts`，单测碰不得，
/// 而「删不掉时到底是报错还是排队」正是这次要改的那一点。
fn remove_or_schedule(path: &Path) -> Result<(), String> {
    remove_or_schedule_with(path, reboot::schedule_delete_on_reboot)
}

/// [`remove_or_schedule`] 的本体，排队动作由调用方注入。
///
/// 注入是为了测得到，而且是为了**不把测试写进真实系统**：`schedule_delete_on_reboot`
/// 调的是 `MoveFileEx(MOVEFILE_DELAY_UNTIL_REBOOT)`，它往
/// `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`
/// 里写真条目——那是个全局共享值，开机时由会话管理器无条件执行。测试每跑一次加一条，
/// 而排的是 `%TEMP%\...<pid>...`，pid 会复用：概率极低，但撞上就是开机删掉一个真实文件，
/// 且没有任何日志。
///
/// 这里真正要证明的是**我们自己的分支判断**（排得进 → Ok；排不进 → Err），不是
/// Win32 的行为；记账与「是否提示重启」的耦合由 `schedule_delete_on_reboot` 自己的
/// 测试负责。职责分开，测试也就不必碰注册表。
fn remove_or_schedule_with(
    path: &Path,
    schedule: impl Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let Err(e) = std::fs::remove_file(path) else {
        return Ok(());
    };
    // 先改名让路，再排原地删除 —— 与 ime.rs 的两处排队点同一手法。
    //
    // 不让路的话有一条窄但现实的坏路径：卸载时字体被占用、以**原名**排进队列 →
    // 用户没重启就重装 → `install_one` 看到 `font_dest.exists()` 且大小一致，
    // 跳过拷贝只重写注册表 → 下次重启队列把那个文件删了 → 字体注册项指向一个
    // 已不存在的文件。「卸了重装试试」恰好是用户遇到输入法问题时的常见动作。
    // `stash_aside` 改名失败时原样返回原路径，所以换上它严格不劣。
    //
    // 记账由 `schedule_delete_on_reboot` 自己完成（它内部就调 record_pending），
    // 这里不要再记一次：虽然 record_pending 按路径去重、不会真的多出一条，
    // 但两处都写会让人以为账本里有两笔。
    if schedule(&reboot::stash_aside(path)).is_ok() {
        // 排进队列了：这不是「未能清除」，是「需要重启」。run_plan 收尾读账本，
        // 完成页显示的是成功 + 重启提示。
        Ok(())
    } else {
        Err(format!("删除文件失败 {:?}: {}", path, e))
    }
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod remove_or_schedule_tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    // 变异检验已做：
    //   · 删不掉时改回直接 Err（不排队）→ locked_file_is_queued_not_reported_as_failure 变红
    //   · 排不进队列时也返回 Ok → unqueueable_file_is_a_real_failure 变红
    //   · 排队前不 stash_aside（直接排原路径）→ queues_the_stashed_path 变红
    //   · 正常删掉也去排队 → deleting_normally_never_queues 变红
    //
    // 这些测试一个字节都不落进注册表：排队动作是注入的。真 API 会往全局的
    // PendingFileRenameOperations 里写，开机时无条件执行 —— 测试不该往那里加料。

    /// 记下自己被调用时拿到的路径；`ok` 决定这次排队成不成功。
    struct FakeScheduler {
        seen: RefCell<Vec<PathBuf>>,
        ok: bool,
    }

    impl FakeScheduler {
        fn new(ok: bool) -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
                ok,
            }
        }
        fn schedule(&self, p: &Path) -> Result<(), String> {
            self.seen.borrow_mut().push(p.to_path_buf());
            if self.ok {
                Ok(())
            } else {
                Err("排不进去".into())
            }
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wind_font_test_{}_{}.bin", std::process::id(), tag))
    }

    /// 用 `share_mode(0)` 独占打开制造**真实**的删除失败 —— 这一半必须是真的，
    /// 它正是现场的形态（字体被 GDI 占着）。守卫在返回值 drop 时释放。
    fn locked(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::write(path, b"x").unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0) // 不共享读/写/删除 —— 谁也删不掉、也改不了名
            .open(path)
            .expect("独占打开失败")
    }

    #[test]
    fn deleting_normally_never_queues() {
        let path = temp_path("plain");
        std::fs::write(&path, b"x").unwrap();
        let fake = FakeScheduler::new(true);

        assert!(remove_or_schedule_with(&path, |p| fake.schedule(p)).is_ok());
        assert!(!path.exists(), "文件没被删掉");
        assert!(
            fake.seen.borrow().is_empty(),
            "正常删掉的文件不该排进重启队列 —— 那会让用户白重启一次"
        );
    }

    /// 字体被 GDI 占着删不掉，是**正常路径**（登录时按注册表加载，装完用过再卸就会这样）。
    /// 这种局面必须归类成「需要重启」而不是「未能清除」：后者会让每一次正常卸载都
    /// 显示有残留，还会连带不再自删除安装目录。
    #[test]
    fn locked_file_is_queued_not_reported_as_failure() {
        let path = temp_path("locked");
        let guard = locked(&path);
        let fake = FakeScheduler::new(true);

        let result = remove_or_schedule_with(&path, |p| fake.schedule(p));

        assert!(
            result.is_ok(),
            "被占用的字体不该报成失败，应排进重启队列: {result:?}"
        );
        assert_eq!(fake.seen.borrow().len(), 1, "该排且只排一次");

        drop(guard);
        let _ = std::fs::remove_file(&path);
    }

    /// 排队**失败**那一支必须是真失败。
    ///
    /// 这条在真 API 下永远走不到（提权进程里 MoveFileEx 总能排进去），
    /// 于是 `remove_or_schedule` 的两个出口里有一个从来没被执行过。
    #[test]
    fn unqueueable_file_is_a_real_failure() {
        let path = temp_path("unqueueable");
        let guard = locked(&path);
        let fake = FakeScheduler::new(false);

        let result = remove_or_schedule_with(&path, |p| fake.schedule(p));

        assert!(
            result.is_err(),
            "既删不掉又排不进队列，那就是真的没清掉，不能报成功"
        );
        assert!(
            result.unwrap_err().contains("删除文件失败"),
            "错误里得说清是哪一步没成"
        );

        drop(guard);
        let _ = std::fs::remove_file(&path);
    }

    /// 排进队列的必须是 `stash_aside` 让路之后的路径。
    ///
    /// 排原路径会留下一条窄但现实的坏路径：卸载时字体被占用、以原名入队 → 用户没重启
    /// 就重装 → `install_one` 看到文件在且大小一致，跳过拷贝 → 下次开机队列把它删了 →
    /// 注册项指向一个已不存在的文件。
    ///
    /// ⚠️ 这里**不能**用 share_mode(0)：那种硬占用连改名都做不到（Windows 改名同样
    /// 需要 DELETE 访问权），`stash_aside` 会退回原路径，这条断言就测不到东西了。
    /// 用一个只读目录制造「删不掉但改得动」——恰好是 stash_aside 唯一帮得上忙的形态。
    #[test]
    fn queues_the_stashed_path() {
        let dir = std::env::temp_dir().join(format!("wind_font_stash_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // 让 remove_file 必然失败、而 rename 仍可行：把目标做成一个非空目录。
        // （remove_file 对目录报错；rename 对同卷目录可行。）
        let path = dir.join("victim_dir");
        std::fs::create_dir_all(path.join("inner")).unwrap();

        let fake = FakeScheduler::new(true);
        let result = remove_or_schedule_with(&path, |p| fake.schedule(p));
        assert!(result.is_ok(), "{result:?}");

        let seen = fake.seen.borrow();
        assert_eq!(seen.len(), 1);
        let queued = seen[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            queued.starts_with("victim_dir.old_"),
            "排的应当是让路后的名字，实际是 {queued:?}"
        );

        drop(seen);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
