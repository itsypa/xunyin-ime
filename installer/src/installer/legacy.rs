use std::path::Path;

use crate::meta;
use crate::util::reboot;

use super::step::Reporter;

/// 清理旧版遗留文件和目录（新版已移除、旧版可能残留的条目）。
/// 在解压新文件之前调用，避免旧文件干扰。
///
/// 删不掉的一律排进重启删除队列并记入 [`reboot`] 账本——旧版遗留文件被占用是
/// 升级时最常见的「清不干净」来源（如上一版的 DLL 仍被 ctfmon 加载）。此前这里
/// 只 `eprintln!`，而安装器是无控制台的 GUI 进程，等于把事实彻底丢弃。
pub fn cleanup_legacy(install_dir: &Path, r: &mut dyn Reporter) {
    for name in meta::legacy_files() {
        let path = install_dir.join(name);
        if !path.exists() {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            r.warn(&format!(
                "旧版文件 {} 删除失败（将于重启后清理）: {}",
                name, e
            ));
            let _ = reboot::schedule_delete_on_reboot(&path);
        }
    }

    for rel in meta::legacy_dirs() {
        // 内容目录不接受「旧版遗留」这个身份——它们由解包正向覆盖维护，不是上一版
        // 留下的垃圾；含 `..` 的条目更是能走出安装目录（见 `manifest::classify_legacy_dir`）。
        // 打包期已拒绝这两类清单，这里是对**已经打出去的旧安装包**的运行期兜底：
        // 宁可留下一个真正的遗留目录，也不能删掉内容层或删到安装目录之外。
        //
        // 两类分开报，与打包期同一口径：合成一句会让读日志的人以为换个名字就能过。
        if let Some(why) = crate::manifest::classify_legacy_dir(rel) {
            r.warn(&match why {
                crate::manifest::LegacyDirRejection::EscapesInstallDir => {
                    format!(
                        "忽略 legacy_dirs 中含 `..` 的条目 {}（会走出安装目录）",
                        rel
                    )
                }
                crate::manifest::LegacyDirRejection::ContentDir(_) => {
                    format!("忽略 legacy_dirs 中的内容目录 {}", rel)
                }
            });
            continue;
        }
        let path = install_dir.join(rel);
        if !path.exists() {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            r.warn(&format!(
                "旧版目录 {} 删除失败（将于重启后清理）: {}",
                rel, e
            ));
            reboot::schedule_dir_on_reboot(&path);
        }
    }
}
